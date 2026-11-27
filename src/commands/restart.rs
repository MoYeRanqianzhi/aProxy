//! `aproxy restart [PORT|all|ALIAS|idle [SECS]]`：重启运行中的实例。
//!
//! 语义边界（用户定调）：**只负责重启，不负责启动**——没在运行的 target 报
//! `target_not_running`（退出码 4），绝不顺手拉起。重启本身（预检 → 停止 → 按原来的拉起规格
//! 拉起 → 等就绪）在操作层 `aproxy::ops::restart`，先预检后停止的理由也写在那里；这里选目标、
//! 逐个重启、把结果说给用户。`restart all` 逐实例处理，失败的跳过、其余照常重启，最后汇总：
//! 结局是 `restarted`、`failed` 之一，有成有败时退出码 5。

#[path = "../i18n/restart.rs"]
mod restart_text;
use super::stop::text as stop_text;
use aproxy::daemon::{self, InstanceRecord};
use aproxy::ops::restart::{PrecheckError, RestartFailure, restart_instance};
use aproxy::ops::stop::{StopFailure, StopMode};
use aproxy::ops::target::{self, Empty, SelectError};
use aproxy_ipc::i18n::{Lang, current};
use serde_json::json;

use super::stop::{
    ambiguous, fail_skipped, require_idle_for_threshold, stop_failure, stopped_line,
    unknown_target, unresponsive,
};
use super::{Failure, Results, port_value, say};

/// 目标没在运行：restart 不启动它，`target_not_running`（4）。`target` 为 None 表示命令行上
/// 没写目标。
fn not_running(target: Option<&str>, message: impl Fn(Lang) -> String) -> Failure {
    Failure::rendered(
        "target_not_running",
        |lang| restart_text::not_running(lang, &message(lang)),
        json!({"target": target}),
    )
}

/// `aproxy restart` 入口。target/threshold/force 语义与 stop 完全一致。
pub(crate) async fn handle_restart_cmd(
    target: Option<String>,
    threshold: Option<u64>,
    force: bool,
    formats: Vec<String>,
    launch: aproxy::spec::Launch,
    json: bool,
) -> u8 {
    let format_op = if formats.is_empty() {
        None
    } else {
        if force || threshold.is_some() {
            return Failure::rendered(
                "usage",
                |l| restart_text::format_flags(l).to_owned(),
                json!({"reason":"format_selection"}),
            )
            .report(json);
        }
        let all = formats.iter().any(|n| n == "all");
        if (all && formats.len() != 1)
            || formats
                .iter()
                .any(|n| n.is_empty() || n.len() > 64 || n.chars().any(char::is_control))
        {
            return Failure::rendered(
                "usage",
                |l| restart_text::format_selection(l).to_owned(),
                json!({"reason":"format_selection"}),
            )
            .report(json);
        }
        Some(daemon::FormatOp::Restart {
            names: (!all).then_some(formats),
        })
    };
    if let Err(f) = require_idle_for_threshold("restart", target.as_deref(), threshold) {
        return f.report(json);
    }
    let mode = StopMode::from_force(force);
    let mut results = Results::new(target.as_deref());
    let selection = match target::select(target.as_deref(), threshold, force).await {
        Ok(selection) => selection,
        Err(SelectError::UnknownTarget { target }) => return unknown_target(&target).report(json),
        Err(SelectError::Ambiguous { ports }) => return ambiguous("restart", &ports).report(json),
        Err(SelectError::NotRunning {
            target,
            config_path,
        }) => {
            return not_running(Some(&target), |lang| {
                stop_text::no_alias(lang, &target, config_path.display())
            })
            .report(json);
        }
        Err(SelectError::NothingOnPort { port }) => {
            return not_running(Some(&port), |lang| restart_text::no_port(lang, &port))
                .report(json);
        }
        Err(SelectError::Incompatible { port, reason }) => {
            let failure = super::incompatible_instance(&port, &reason);
            eprintln!("{}", failure.terminal());
            results.failed(&port, "failed", &failure);
            return results.finish(json);
        }
        Err(SelectError::Unresponsive { target, port, pid }) => {
            let failure = unresponsive(target.as_deref(), &port, pid);
            eprintln!("{}", failure.terminal());
            results.failed(&port, "failed", &failure);
            return results.finish(json);
        }
    };
    fail_skipped(&selection, &mut results);
    match selection.empty {
        // 没写目标却一个运行中的实例都没有：要重启的那个实例不存在
        Some(Empty::NoneRunning)
            if target.is_none() && selection.unresponsive_skipped.is_empty() =>
        {
            return not_running(None, |lang| super::status::text::none(lang).to_owned())
                .report(json);
        }
        // all：没有运行中的实例就是没有要重启的，无事可做
        Some(Empty::NoneRunning) if selection.unresponsive_skipped.is_empty() => {
            say(json, super::status::text::none(current()))
        }
        Some(Empty::NoneIdle { idle_secs }) => {
            say(json, stop_text::none_idle(current(), idle_secs))
        }
        _ => {}
    }
    if let Some(op) = format_op {
        for info in &selection.instances {
            restart_formats(info, &op, &mut results, json).await;
        }
        say(json, restart_text::format_config(current()));
        return results.finish(json);
    }
    let exe = std::env::current_exe().expect("cannot locate the current executable");
    // 多实例时最后再汇总一遍：逐条失败信息可能已被后续实例的输出冲散
    let mut failed: Vec<(String, &'static str)> = Vec::new();
    for info in &selection.instances {
        let port = daemon::port_of(&info.listen_addr).to_string();
        match restart_instance(info, mode, &exe, &launch).await {
            Ok(done) => {
                say(json, stopped_line(info, done.stopped));
                // 这一项的端口是新实例实际监听的：配置改了端口时它与旧实例的不同
                let new_port = daemon::port_of(&done.live.instance.listen_addr);
                say(
                    json,
                    restart_text::done(
                        current(),
                        new_port,
                        done.live.instance.pid,
                        done.old_pid,
                        &done.live.instance.listen_addr,
                    ),
                );
                results.ok(new_port, "restarted", None);
            }
            Err(failure) => {
                let state = match &failure {
                    RestartFailure::Starting => restart_text::starting_state(current()),
                    RestartFailure::Unsupported => restart_text::unsupported_state(current()),
                    RestartFailure::Precheck(_) => restart_text::precheck_state(current()),
                    RestartFailure::Stop(StopFailure::GenerationChanged) => {
                        restart_text::changed_state(current())
                    }
                    RestartFailure::Stop(_) => restart_text::stop_state(current()),
                    RestartFailure::Start { .. } => restart_text::start_state(current()),
                };
                let failure = restart_failure(info, &port, failure, json);
                eprintln!("{}", failure.terminal());
                results.failed(&port, "failed", &failure);
                failed.push((port, state));
            }
        }
    }
    if !failed.is_empty() && selection.instances.len() > 1 {
        eprintln!(
            "{}",
            restart_text::failed_count(current(), selection.instances.len(), failed.len())
        );
        for (port, state) in &failed {
            eprintln!("{}", restart_text::state(current(), port, state));
        }
    }
    results.finish(json)
}

/// 只在已核验身份的连接上操作，失败没有退回到重启实例的路径。计数与输出项都是逐条目。
async fn restart_formats(
    info: &InstanceRecord,
    op: &daemon::FormatOp,
    results: &mut Results,
    json: bool,
) {
    use aproxy_ipc::{
        client::{IpcError, RunIdentity, TransportIssue, format_for_run_in},
        wire::{RestartOutcome, RestartReport},
    };
    let port = daemon::port_of(&info.listen_addr);
    let expected = RunIdentity {
        pid: info.pid,
        instance_id: info.instance_id.clone(),
    };
    let report = format_for_run_in(&daemon::run_dir(), port, &expected, op)
        .await
        .and_then(|v| {
            serde_json::from_value::<RestartReport>(v).map_err(|e| {
                IpcError::Protocol(TransportIssue::ReplyFormat {
                    error: e.to_string(),
                })
            })
        });
    match report {
        Ok(report) => {
            let safe = |s: &str| s.chars().filter(|c| !c.is_control()).collect::<String>();
            for entry in report.entries {
                let mut item = match &entry {
                    RestartOutcome::Restarted {
                        name,
                        program,
                        version,
                    } => {
                        say(
                            json,
                            restart_text::format_done(
                                current(),
                                port,
                                &safe(name),
                                &safe(program.as_deref().unwrap_or("")),
                                &safe(version.as_deref().unwrap_or("")),
                            ),
                        );
                        results.ok += 1;
                        results.item(port, "restarted")
                    }
                    RestartOutcome::HandshakeFailed { name, reason } => {
                        eprintln!(
                            "{}",
                            restart_text::format_failed(
                                current(),
                                port,
                                &safe(name),
                                &safe(reason)
                            )
                        );
                        results.failed.push(aproxy_ipc::code::EXIT_FAILED);
                        results.item(port, "handshake-failed")
                    }
                };
                item.extend(
                    serde_json::to_value(entry)
                        .unwrap()
                        .as_object()
                        .unwrap()
                        .clone(),
                );
                item.insert("configReloaded".into(), report.config_reloaded.into());
                results.items.push(item.into());
            }
        }
        Err(e) => {
            let reason = match &e {
                IpcError::Remote { code, .. } => code.as_str(),
                IpcError::RunChanged => "run_changed",
                _ => "ipc",
            };
            let failure = Failure::rendered(
                "format_restart_failed",
                |l| {
                    if reason == "unknown_op" {
                        restart_text::format_unsupported(l, port)
                    } else {
                        restart_text::format_refused(l, port, &e.render(l))
                    }
                },
                json!({"port":port_value(port),"reason":reason}),
            );
            eprintln!("{}", failure.terminal());
            results.failed(port, "failed", &failure);
        }
    }
}

/// 一个实例没能重启：失败所处的阶段决定码与说明。旧实例已经停下的那一种，先说它是怎样停下的
/// （`json` 为真时这行写到 stderr）。
fn restart_failure(
    info: &InstanceRecord,
    port: &str,
    failure: RestartFailure,
    json: bool,
) -> Failure {
    match failure {
        RestartFailure::Starting => Failure::rendered(
            "command_failed",
            |lang| restart_text::starting(lang, port),
            json!({}),
        ),
        RestartFailure::Unsupported => Failure::rendered(
            "restart_unsupported",
            |lang| restart_text::unsupported(lang, port),
            json!({"port": port_value(port)}),
        ),
        RestartFailure::Precheck(reason) => precheck_failure(port, reason)
            .context(|lang, cause| restart_text::precheck(lang, port, cause)),
        RestartFailure::Stop(stop @ StopFailure::GenerationChanged) => stop_failure(info, &stop),
        RestartFailure::Stop(stop) => stop_failure(info, &stop)
            .context(|lang, cause| restart_text::stop_failed(lang, port, cause)),
        RestartFailure::Start {
            stopped,
            error,
            config_path,
        } => {
            say(json, stopped_line(info, stopped));
            // 期望状态记录还在：看护者会继续按它重拉，用户排除原因后也可以自己执行 restore
            let message = |lang| {
                let mut message =
                    restart_text::failed(lang, port, &error.render(lang), &error.excerpt(lang));
                if let Some(path) = &config_path {
                    message.push_str(&restart_text::config(lang, path.display()));
                }
                message
            };
            super::spawn_failure(&error, message)
        }
    }
}

/// 预检不通过的原因：配置无效是 `config_invalid`（3），配置文件不在是 `target_not_found`（4），
/// 记录读不懂是 `restart_unsupported`（没有能用来重新拉起它的规格），端口的问题是
/// `port_in_use` 或 `start_failed`（1）。
fn precheck_failure(port: &str, reason: PrecheckError) -> Failure {
    match reason {
        PrecheckError::RecordUnreadable { reason } => Failure::rendered(
            "restart_unsupported",
            |lang| restart_text::unreadable(lang, &reason.diagnostic().render(lang)),
            json!({"port": port_value(port)}),
        ),
        PrecheckError::ConfigMissing { path } => Failure::rendered(
            "target_not_found",
            |lang| restart_text::missing(lang, path.display()),
            json!({"target": path.display().to_string()}),
        ),
        PrecheckError::Config(error) => super::start::config_failure(&error),
        PrecheckError::PortTaken { port, pid } => Failure::rendered(
            "port_in_use",
            |lang| restart_text::taken(lang, pid, &port),
            json!({"port": port_value(&port), "owner": "aproxy"}),
        ),
        PrecheckError::Bind { listen_addr, error } => super::bind_failure(&listen_addr, &error),
    }
}
