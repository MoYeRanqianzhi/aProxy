//! 所有来自页面的名字都是前置条件。每次执行仍从本 home 解析文件、期望状态或实际进程。
use super::{DesktopHome, collect_config_secrets, collect_settings_secrets};
use crate::{
    config, daemon, find,
    ops::{restart, restore, start, stop},
    settings, spawn, spec,
};
use aproxy_ipc::{
    desktop::*,
    enumerate::{self, Liveness},
    redact::Secrets,
};
use std::{collections::BTreeMap, path::PathBuf};

pub(super) struct Offered {
    pub(super) path: PathBuf,
    pub(super) implicit: bool,
    choice: Choice,
}

/// 无法可信地解析文件时，诊断仍可定位到行列，但不引用可能含凭据的原文或键名。
pub(super) fn issues(ds: &[aproxy_ipc::diag::Diagnostic], secrets: Option<&Secrets>) -> Vec<Issue> {
    use aproxy_ipc::diag::Level;
    ds.iter()
        .map(|d| Issue {
            code: DiagCode::classify(d.code),
            severity: match d.severity {
                Level::Error => Severity::Error,
                Level::Warning => Severity::Warning,
                Level::Info => Severity::Info,
            },
            key: secrets.and_then(|s| d.key.as_ref().map(|k| s.replace(k))),
            file: secrets.map_or_else(|| "***".to_owned(), |s| s.replace(&d.file)),
            line: d.line.map(|n| SafeUint::saturating(n.into())),
            column: d.column.map(|n| SafeUint::saturating(n.into())),
        })
        .collect()
}
fn config_error(err: &start::ConfigError) -> Fault {
    match err {
        start::ConfigError::Missing(_) => Fault::DesktopTarget {},
        _ => Fault::DesktopConfig {
            diagnostics: issues(err.diagnostics(), None),
        },
    }
}
fn start_error(e: spawn::SpawnError, old_stopped: bool) -> Fault {
    Fault::DesktopStart {
        reason: match e {
            spawn::SpawnError::Spawn(_) => StartReason::Spawn,
            _ => StartReason::NotReady,
        },
        old_stopped,
    }
}
fn stop_error(port: u16, e: stop::StopFailure) -> Fault {
    match e {
        stop::StopFailure::GenerationChanged => Fault::DesktopChanged {},
        stop::StopFailure::Unresponsive { .. } => Fault::DesktopUnresponsive { port },
        stop::StopFailure::StillRunning => Fault::DesktopStop {
            port,
            reason: StopReason::StillRunning,
        },
        stop::StopFailure::ForceFailed { .. } => Fault::DesktopStop {
            port,
            reason: StopReason::Unverified,
        },
    }
}

impl DesktopHome {
    // 令牌密钥每次列举都替换。重算文件候选而非永久保存每次列举的路径，窗口的表恒定大小。
    fn offers(&self, nonce: [u8; 16]) -> R<(Vec<Offered>, Option<Secrets>)> {
        let settings_path = settings::settings_path_in(&self.home);
        let loaded = settings::read(&settings_path);
        if !loaded.is_valid() {
            return Err(Fault::DesktopConfig {
                diagnostics: issues(&loaded.diagnostics, None),
            });
        }
        let cfg = loaded.settings;
        let default = settings::default_config_path_for(&cfg, &self.home);
        let mut paths = vec![(
            ChoiceKind::Default,
            "default".to_owned(),
            default.clone(),
            cfg.default_config.is_none(),
        )];
        let mut discovered = BTreeMap::new();
        for f in find::discover_in(&cfg, &self.home) {
            if let Ok(canon) = f.path.canonicalize() {
                discovered.entry(canon).or_insert(f.path);
            }
        }
        for p in discovered.into_values() {
            paths.push((
                ChoiceKind::Config,
                p.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                p,
                false,
            ));
        }
        let mut aliases = cfg.aliases.iter().collect::<Vec<_>>();
        aliases.sort();
        for (alias, p) in aliases {
            paths.push((
                ChoiceKind::Alias,
                alias.clone(),
                settings::expand_path(p),
                false,
            ));
        }
        let mut values = Vec::new();
        let mut trusted = match std::fs::read(&settings_path) {
            Ok(b) => serde_json::from_slice(&b)
                .map(|v| collect_settings_secrets(&v, &mut values))
                .is_ok(),
            Err(e) => e.kind() == std::io::ErrorKind::NotFound,
        };
        for (_, _, p, _) in &paths {
            match std::fs::read_to_string(p) {
                Ok(t) => match config::view::file_value(&t) {
                    Some(v) if !config::file::parse(&t, p).1.iter().any(|d| d.is_error()) => {
                        collect_config_secrets(&v, &mut values)
                    }
                    _ => trusted = false,
                },
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => trusted = false,
            }
        }
        let secrets = Secrets::new(values);
        let display = |s: &str| {
            if trusted {
                secrets.replace(s)
            } else {
                "***".into()
            }
        };
        let canonical_default = default.canonicalize().unwrap_or(default);
        let mut result = Vec::new();
        for (kind, name, p, implicit) in paths {
            let exists = p.is_file();
            // 不存在的默认文件也有明确的规范父目录；符号链接改指后重算出的令牌随之改变。
            let path = p.canonicalize().unwrap_or_else(|_| {
                p.parent()
                    .and_then(|d| d.canonicalize().ok())
                    .zip(p.file_name())
                    .map_or_else(|| p.clone(), |(d, n)| d.join(n))
            });
            let identity = serde_json::to_vec(&(nonce, kind, &name, &p, &path, implicit))
                .map_err(|_| Fault::DesktopTarget {})?;
            let id = self.token(&identity);
            let choice = Choice {
                target: StartTarget { kind, id },
                path: display(&p.to_string_lossy()),
                is_default: kind == ChoiceKind::Default || path == canonical_default,
                exists,
                label: display(&name),
            };
            result.push(Offered {
                path,
                implicit,
                choice,
            });
        }
        Ok((result, trusted.then_some(secrets)))
    }
    pub(super) fn display_secrets(&self) -> Option<Secrets> {
        self.offers([0; 16]).ok()?.1
    }

    pub(super) async fn resolve_choice(&self, t: &StartTarget) -> R<Offered> {
        let nonce = self
            .listing
            .lock()
            .unwrap()
            .ok_or(Fault::DesktopChanged {})?;
        let host = self.clone();
        let value = tokio::task::spawn_blocking(move || host.offers(nonce))
            .await
            .map_err(|_| Fault::DesktopUnavailable {})??
            .0
            .into_iter()
            .find(|v| v.choice.target == *t)
            .ok_or(Fault::DesktopChanged {})?;
        if *self.listing.lock().unwrap() != Some(nonce) {
            return Err(Fault::DesktopChanged {});
        }
        if !value.implicit && !value.choice.exists {
            return Err(Fault::DesktopTarget {});
        }
        Ok(value)
    }
    pub(super) async fn selected_run(&self, r: &Run, force: bool) -> R<daemon::InstanceRecord> {
        self.selected_member(r, force)
            .await?
            .instance
            .ok_or(Fault::DesktopChanged {})
    }
    /// 读和写都基于同一份经 PID 核验的成员；返回快照使列表不必再经一次发现。
    async fn selected_member(&self, r: &Run, force: bool) -> R<enumerate::Enumerated> {
        let member = enumerate::enumerate_in(&self.home)
            .await
            .instances
            .into_iter()
            .find(|i| i.port == r.port)
            .ok_or(Fault::DesktopChanged {})?;
        member
            .instance
            .as_ref()
            .filter(|rec| rec.instance_id == r.instance_id)
            .ok_or(Fault::DesktopChanged {})?;
        if force && member.liveness != Liveness::Unresponsive {
            return Err(Fault::DesktopForceRefused { port: r.port });
        }
        if !force && member.liveness != Liveness::Live {
            return Err(Fault::DesktopUnresponsive { port: r.port });
        }
        Ok(member)
    }
    fn ready(&self, rec: daemon::InstanceRecord) -> R<Run> {
        let port = daemon::port_of(&rec.listen_addr)
            .parse()
            .map_err(|_| Fault::DesktopUnavailable {})?;
        let k = Run {
            port,
            instance_id: rec.instance_id.clone(),
        };
        let found = enumerate::Enumerated {
            port,
            liveness: Liveness::Live,
            instance: Some(rec),
            status: None,
            reason: None,
            restore_pending: false,
            restore_reason: None,
        };
        let f = self
            .inspect(&[found])
            .remove(&port)
            .ok_or(Fault::DesktopUnavailable {})?;
        Projection::new(&f.secrets)
            .run(&k.key())
            .ok_or(Fault::DesktopUnavailable {})
    }
    async fn pending(&self, port: u16, expected: &Token) -> R<spec::RestoreEntry> {
        let dir = self.home.join("run");
        let port = port.to_string();
        if aproxy_ipc::registry::port_has_instance_in(&dir, &port).await {
            return Err(Fault::DesktopChanged {});
        }
        let e = spec::read_restore_in(&dir, &port)
            .map_err(|_| Fault::DesktopChanged {})?
            .ok_or(Fault::DesktopTarget {})?;
        if self.restore_token(&e) != *expected {
            return Err(Fault::DesktopChanged {});
        }
        Ok(e)
    }
}
/// 只认可实例协议的拒绝词表；不把端点、管道诊断或未知远端错误原文交给页面。
fn format_fault(e: aproxy_ipc::client::IpcError) -> Fault {
    use aproxy_ipc::client::IpcError;
    match e {
        IpcError::RunChanged => Fault::DesktopChanged {},
        IpcError::Remote { code, details, .. } => match code.as_str() {
            "unknown_op" => Fault::DesktopFormat {
                reason: "unsupported".into(),
            },
            "unknown_format" => Fault::DesktopFormat {
                reason: "unknown-format".into(),
            },
            "format_control_error" => {
                serde_json::from_value::<aproxy_ipc::wire::ControlIssue>(details["reason"].clone())
                    .map(|r| Fault::DesktopFormat {
                        reason: r.as_str().into(),
                    })
                    .unwrap_or(Fault::DesktopUnavailable {})
            }
            "invalid_state" => Fault::DesktopBusy {},
            "bad_request" => Fault::DesktopBadInput {},
            _ => Fault::DesktopUnavailable {},
        },
        _ => Fault::DesktopUnavailable {},
    }
}
impl DesktopHome {
    /// 有副作用的操作既检查页面选择，也在已持有的 IPC 连接上比对对端，端口复用不能重定向调用。
    async fn format_op(&self, run: &Run, op: aproxy_ipc::wire::FormatOp) -> R<serde_json::Value> {
        let rec = self.selected_run(run, false).await?;
        aproxy_ipc::client::format_for_run_in(
            &self.home.join("run"),
            &run.port.to_string(),
            &aproxy_ipc::client::RunIdentity {
                pid: rec.pid,
                instance_id: rec.instance_id,
            },
            &op,
        )
        .await
        .map_err(format_fault)
    }
}
impl InstanceOps for DesktopHome {
    fn format_list(&self, r: Run) -> Task<'_, Vec<FormatView>> {
        Box::pin(async move {
            let m = self.selected_member(&r, false).await?;
            m.status
                .ok_or(Fault::DesktopUnavailable {})?
                .formats
                .into_iter()
                .map(|mut v| {
                    // 快照的熔断器是结构体；桌面只暴露三种状态，不透传进程配置或后续协议字段。
                    let o = v.as_object_mut().ok_or(Fault::DesktopUnavailable {})?;
                    let key = if o
                        .get("stages")
                        .and_then(|v| v.as_array())
                        .is_some_and(|s| s.len() == 1 && s[0] == "notify")
                    {
                        "notify_breaker"
                    } else {
                        "breaker"
                    };
                    let breaker = o
                        .get(key)
                        .and_then(|v| v.get("state"))
                        .cloned()
                        .ok_or(Fault::DesktopUnavailable {})?;
                    o.insert("breaker".into(), breaker);
                    let ts = o
                        .get("restarted_at")
                        .and_then(|v| v.as_u64())
                        .map(SafeUint::saturating);
                    o.insert("restartedAt".into(), serde_json::json!(ts));
                    serde_json::from_value(v).map_err(|_| Fault::DesktopUnavailable {})
                })
                .collect()
        })
    }
    fn format_restart(
        &self,
        r: Run,
        names: Option<Vec<String>>,
    ) -> Task<'_, Vec<aproxy_ipc::wire::RestartOutcome>> {
        Box::pin(async move {
            let reply = self
                .format_op(&r, aproxy_ipc::wire::FormatOp::Restart { names })
                .await?;
            serde_json::from_value::<aproxy_ipc::wire::RestartReport>(reply)
                .map(|r| r.entries)
                .map_err(|_| Fault::DesktopUnavailable {})
        })
    }
    fn format_call(
        &self,
        r: Run,
        name: String,
        method: String,
        params: serde_json::Value,
    ) -> Task<'_, serde_json::Value> {
        Box::pin(async move {
            if serde_json::to_vec(&params).unwrap().len() > 64 * 1024 {
                return Err(Fault::DesktopFormat {
                    reason: "too-large".into(),
                });
            }
            let reply = self
                .format_op(
                    &r,
                    aproxy_ipc::wire::FormatOp::Call {
                        name,
                        method,
                        params,
                    },
                )
                .await?;
            match serde_json::from_value(reply).map_err(|_| Fault::DesktopUnavailable {})? {
                aproxy_ipc::wire::ControlReply::Result { result } => Ok(result),
                aproxy_ipc::wire::ControlReply::Fail { fail } => Err(Fault::DesktopFormatFail {
                    code: fail.code,
                    reason: fail.reason,
                }),
            }
        })
    }
    fn list_configs(&self) -> Task<'_, Vec<Choice>> {
        Box::pin(async move {
            let mut nonce = [0; 16];
            getrandom::fill(&mut nonce).map_err(|_| Fault::DesktopUnavailable {})?;
            let host = self.clone();
            let (offered, _) = tokio::task::spawn_blocking(move || host.offers(nonce))
                .await
                .map_err(|_| Fault::DesktopUnavailable {})??;
            *self.listing.lock().unwrap() = Some(nonce);
            Ok(offered.into_iter().map(|v| v.choice).collect())
        })
    }
    fn start_instance(&self, target: StartTarget) -> Task<'_, Run> {
        Box::pin(async move {
            let offer = self.resolve_choice(&target).await?;
            let file = (!offer.implicit || offer.path.exists()).then_some(offer.path.as_path());
            let ov = config::Overrides::default();
            let cfg = start::resolve_runtime_config_in(&self.home, &ov, file)
                .map_err(|e| config_error(&e))?;
            let port = daemon::port_of(&cfg.listen_addr)
                .parse()
                .unwrap_or_default();
            let live = match start::preflight_in(&self.home, &cfg.listen_addr)
                .await
                .map_err(|e| match e {
                    start::PreflightError::Startup(e) => start_error(e, false),
                    _ => Fault::DesktopPort { port },
                })? {
                start::Preflight::AlreadyRunning(s) | start::Preflight::BecameReady(s) => *s,
                start::Preflight::Clear => {
                    self.resolve_choice(&target).await?;
                    let log = start::daemon_log_path_in(&self.home, cfg.log_file.as_deref());
                    let spec = spec::capture(file, ov, &log);
                    start::launch_in(&self.home, &self.exe, &spec)
                        .await
                        .map_err(|e| start_error(e, false))?
                }
            };
            self.ready(live.instance)
        })
    }
    fn stop_instance(&self, r: Run) -> Task<'_, ()> {
        Box::pin(async move {
            let info = self.selected_run(&r, false).await?;
            stop::stop_in(
                &self.home,
                &info,
                stop::StopMode::Graceful,
                stop::Intent::Stop,
                stop::Authority::Observed,
            )
            .await
            .map(|_| ())
            .map_err(|e| stop_error(r.port, e))
        })
    }
    fn force_stop_instance(&self, r: Run) -> Task<'_, ()> {
        Box::pin(async move {
            let info = self.selected_run(&r, true).await?;
            stop::stop_in(
                &self.home,
                &info,
                stop::StopMode::Force,
                stop::Intent::Stop,
                stop::Authority::Observed,
            )
            .await
            .map(|_| ())
            .map_err(|e| stop_error(r.port, e))
        })
    }
    fn restart_instance(&self, r: Run) -> Task<'_, Run> {
        Box::pin(async move {
            let info = self.selected_run(&r, false).await?;
            let live = restart::restart_in(
                &self.home,
                &info,
                stop::StopMode::Graceful,
                &self.exe,
                stop::Authority::Observed,
            )
            .await
            .map_err(|err| match err {
                restart::RestartFailure::Stop(e) => stop_error(r.port, e),
                restart::RestartFailure::Start { error, .. } => start_error(error, true),
                restart::RestartFailure::Starting => Fault::DesktopBusy {},
                restart::RestartFailure::Unsupported => Fault::DesktopNoSpec { port: r.port },
                restart::RestartFailure::Precheck(e) => match e {
                    restart::PrecheckError::Config(c) => config_error(&c),
                    restart::PrecheckError::ConfigMissing { .. }
                    | restart::PrecheckError::RecordUnreadable { .. } => {
                        Fault::DesktopNoSpec { port: r.port }
                    }
                    restart::PrecheckError::Bind { listen_addr, .. } => Fault::DesktopPort {
                        port: daemon::port_of(&listen_addr).parse().unwrap_or(r.port),
                    },
                    restart::PrecheckError::PortTaken { port, .. } => Fault::DesktopPort {
                        port: port.parse().unwrap_or(r.port),
                    },
                },
            })?;
            self.ready(live.live.instance)
        })
    }
    fn restore_instance(&self, port: u16, expected: Token) -> Task<'_, Run> {
        Box::pin(async move {
            let entry = self.pending(port, &expected).await?;
            match restore::restore_observed_in(&self.home, &entry, &self.exe)
                .await
                .ok_or(Fault::DesktopChanged {})?
            {
                restore::Restore::AlreadyRunning => Err(Fault::DesktopChanged {}),
                restore::Restore::ConfigMissing { .. } => Err(Fault::DesktopTarget {}),
                restore::Restore::Failed(e) => Err(start_error(e, false)),
                restore::Restore::Restored { pid, port, .. } => {
                    let live = daemon::ipc_ping_in(&self.home.join("run"), &port)
                        .await
                        .map_err(|_| Fault::DesktopChanged {})?;
                    if live.instance.pid != pid {
                        return Err(Fault::DesktopChanged {});
                    }
                    self.ready(live.instance)
                }
            }
        })
    }
    fn forget_restore(&self, port: u16, expected: Token) -> Task<'_, ()> {
        Box::pin(async move {
            let entry = self.pending(port, &expected).await?;
            match stop::forget_port_in(&self.home.join("run"), &entry.port, Some(&entry)).await {
                stop::Forget::Removed => Ok(()),
                stop::Forget::Absent => Err(Fault::DesktopTarget {}),
                stop::Forget::Changed => Err(Fault::DesktopChanged {}),
                stop::Forget::Failed => Err(Fault::DesktopUnavailable {}),
            }
        })
    }
}
