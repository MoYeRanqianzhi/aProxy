//! 普通安装由 home 中已安装的版本执行。窗口只接收结构化、有限长的投影，不转发子进程原文。
use super::DesktopHome;
use aproxy_ipc::desktop::*;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    io::Read,
    path::{Path, PathBuf},
    process::{ExitStatus, Stdio},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, BufReader},
    process::Command,
};
const LINE: usize = 1024 * 1024;
#[derive(Default)]
pub(super) struct Checks {
    serial: u64,
    offer: Option<Offer>,
    busy: bool,
}
struct Offer {
    token: Token,
    version: String,
    fingerprint: [u8; 32],
}
fn protocol() -> Fault {
    Fault::DesktopInstallProtocol {}
}
fn port(v: &Value) -> Option<u16> {
    v.as_u64().and_then(|n| n.try_into().ok())
}
fn ports(v: &Value) -> Option<Vec<u16>> {
    v.as_array()?.iter().map(port).collect()
}
fn version(v: &Value) -> R<String> {
    let t = v.as_str().filter(|s| s.len() <= 256).ok_or_else(protocol)?;
    semver::Version::parse(t).map_err(|_| protocol())?;
    Ok(t.into())
}
fn optional_version(v: &Value) -> R<Option<String>> {
    if v.is_null() {
        Ok(None)
    } else {
        version(v).map(Some)
    }
}
fn child_error(v: &Value) -> Fault {
    use InstallCode as C;
    let d = &v["details"];
    let Some(code) = v["code"].as_str().and_then(C::classify) else {
        return protocol();
    };
    match code {
        C::InstallInProgress => Fault::InstallInProgress {},
        C::InstallVersionNotFound => Fault::InstallVersionNotFound {},
        C::InstallBelowFloor => Fault::InstallBelowFloor {},
        C::InstallDownloadFailed => Fault::InstallDownloadFailed {},
        C::InstallChecksumMismatch => Fault::InstallChecksumMismatch {},
        C::InstallProbeFailed => Fault::InstallProbeFailed {},
        C::InstallSwapFailed => Fault::InstallSwapFailed {},
        C::InstallRolledBack => Fault::InstallRolledBack {},
        C::InstallFailed => Fault::InstallFailed {},
        C::InstallUnmanagedInstances => Fault::InstallUnmanagedInstances {},
        C::InstallForegroundInstances => ports(&d["ports"]).map_or_else(protocol, |ports| {
            Fault::InstallForegroundInstances { ports }
        }),
        C::InstallRestartFailed => {
            port(&d["port"]).map_or_else(protocol, |port| Fault::InstallRestartFailed { port })
        }
        C::InstallPrecheckFailed => {
            let Some(port) = port(&d["port"]) else {
                return protocol();
            };
            let mut diagnostics = Vec::new();
            for issue in d["diagnostics"].as_array().into_iter().flatten() {
                let Some(c) = issue["code"].as_str() else {
                    return protocol();
                };
                diagnostics.push(Issue {
                    code: DiagCode::classify(c),
                    severity: match issue["severity"].as_str() {
                        Some("warning") => Severity::Warning,
                        Some("info") => Severity::Info,
                        _ => Severity::Error,
                    },
                    key: None,
                    file: "***".into(),
                    line: issue["line"].as_u64().map(SafeUint::saturating),
                    column: issue["column"].as_u64().map(SafeUint::saturating),
                });
            }
            Fault::InstallPrecheckFailed { port, diagnostics }
        }
    }
}
async fn fingerprint(path: PathBuf) -> R<[u8; 32]> {
    tokio::task::spawn_blocking(move || {
        let mut f = std::fs::File::open(path).map_err(|_| Fault::DesktopBinary {})?;
        let mut sha = Sha256::new();
        let mut buf = [0; 65536];
        loop {
            let n = f.read(&mut buf).map_err(|_| Fault::DesktopBinary {})?;
            if n == 0 {
                break;
            }
            sha.update(&buf[..n]);
        }
        Ok(sha.finalize().into())
    })
    .await
    .map_err(|_| Fault::DesktopBinary {})?
}
fn command(home: &Path, exe: &Path) -> Command {
    let mut c = Command::new(exe);
    c.current_dir(home)
        .env("APROXY_HOME", home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    c.creation_flags(0x08000000);
    c
}
// 过长或非法的行不让子进程堵在写管道上：标记协议失败，继续丢弃到 EOF，再等真实退出。
async fn lines(mut cmd: Command, mut consume: impl FnMut(Value) -> R<()>) -> R<ExitStatus> {
    let mut child = cmd.spawn().map_err(|_| Fault::DesktopBinary {})?;
    let mut reader = BufReader::new(child.stdout.take().expect("管道已配置"));
    let mut bad = false;
    loop {
        let mut bytes = Vec::new();
        let n = match (&mut reader)
            .take((LINE + 1) as u64)
            .read_until(b'\n', &mut bytes)
            .await
        {
            Ok(n) => n,
            Err(_) => {
                bad = true;
                break;
            }
        };
        if n == 0 {
            break;
        }
        if n > LINE {
            bad = true;
            let _ = tokio::io::copy(&mut reader, &mut tokio::io::sink()).await;
            break;
        }
        if !bad {
            bad = serde_json::from_slice(&bytes)
                .map_err(|_| protocol())
                .and_then(&mut consume)
                .is_err();
        }
    }
    let code = child.wait().await.map_err(|_| protocol())?;
    if bad { Err(protocol()) } else { Ok(code) }
}
fn check_doc(v: Value, success: bool) -> R<UpdateInfo> {
    if !success {
        return Err(child_error(&v["error"]));
    }
    if !v["error"].is_null() {
        return Err(protocol());
    }
    let channel = serde_json::from_value(v["channel"].clone()).map_err(|_| protocol())?;
    let current = version(&v["current"])?;
    let latest = optional_version(&v["latest"])?;
    let update_available = v["update_available"].as_bool().ok_or_else(protocol)?;
    if update_available
        && latest.as_ref().is_none_or(|s| {
            semver::Version::parse(s).unwrap() <= semver::Version::parse(&current).unwrap()
        })
    {
        return Err(protocol());
    }
    let published_at = v["published_at"]
        .as_str()
        .and_then(|s| {
            time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()
        })
        .and_then(|t| u64::try_from(t.unix_timestamp_nanos() / 1_000_000).ok())
        .map(SafeUint::saturating);
    Ok(UpdateInfo {
        channel,
        current,
        latest,
        update_available,
        variant: serde_json::from_value(v["variant"].clone()).map_err(|_| protocol())?,
        checked_at: SafeUint::saturating(aproxy_ipc::registry::now_millis()),
        check: None,
        published_at,
    })
}
fn fold(job: &mut Job, terminal: &mut Option<R<InstallEnd>>, v: Value) -> R<bool> {
    let kind = v["event"].as_str().ok_or_else(protocol)?;
    if terminal.is_some() {
        return Err(protocol());
    }
    match kind {
        "plan" => {
            job.targets = Some(ports(&v["ports"]).ok_or_else(protocol)?);
        }
        "phase" => {
            let p = v["phase"].as_str().ok_or_else(protocol)?;
            job.phase = Some(
                serde_json::from_value(Value::String(p.into())).unwrap_or(InstallPhase::Unknown),
            );
            job.phase_at = Some(SafeUint::saturating(v["at"].as_u64().ok_or_else(protocol)?));
        }
        "progress" => {
            job.download = Some(Download {
                bytes: SafeUint::saturating(v["bytes"].as_u64().ok_or_else(protocol)?),
                total: if v["total"].is_null() {
                    None
                } else {
                    Some(SafeUint::saturating(
                        v["total"].as_u64().ok_or_else(protocol)?,
                    ))
                },
            });
        }
        "instance" => {
            let port = port(&v["port"]).ok_or_else(protocol)?;
            let outcome = match v["outcome"].as_str().ok_or_else(protocol)? {
                "restarted" => InstanceOutcome::Restarted,
                "rolled_back" => InstanceOutcome::RolledBack,
                "failed" => InstanceOutcome::Failed,
                _ => InstanceOutcome::Unknown,
            };
            if let Some(i) = job.instances.iter_mut().find(|i| i.port == port) {
                i.outcome = outcome
            } else {
                job.instances.push(InstallInstance { port, outcome })
            }
        }
        "result" => {
            let outcome = match v["outcome"].as_str() {
                Some("installed") => InstallOutcome::Installed,
                Some("up_to_date") => InstallOutcome::UpToDate,
                Some("failed") => {
                    *terminal = Some(Err(child_error(&v["error"])));
                    return Ok(false);
                }
                _ => return Err(protocol()),
            };
            *terminal = Some(Ok(InstallEnd {
                outcome,
                version: optional_version(&v["version"])?,
            }));
            return Ok(false);
        }
        _ => return Ok(false),
    }
    Ok(true)
}
impl UpdateOps for DesktopHome {
    fn check_update(&self, channel: ChannelChoice) -> Task<'_, UpdateInfo> {
        Box::pin(async move {
            let n = {
                let mut st = self.updates.lock().unwrap();
                if st.busy {
                    return Err(Fault::DesktopBusy {});
                }
                st.serial = st.serial.wrapping_add(1);
                st.offer = None;
                st.serial
            };
            let exe = crate::install::swap::bin_path_in(&self.home);
            let hash = fingerprint(exe.clone()).await?;
            let mut c = command(&self.home, &exe);
            c.args(["install", "--check", "--json"]);
            if channel == ChannelChoice::Pre {
                c.arg("--pre");
            }
            let mut doc = None;
            let code = lines(c, |value| {
                if doc.replace(value).is_some() {
                    Err(protocol())
                } else {
                    Ok(())
                }
            })
            .await?;
            let mut result = check_doc(doc.ok_or_else(protocol)?, code.success())?;
            if fingerprint(exe).await? != hash {
                return Err(Fault::DesktopChanged {});
            }
            let mut st = self.updates.lock().unwrap();
            if st.serial != n {
                return Err(Fault::DesktopChanged {});
            }
            if let Some(version) = result.latest.as_ref().filter(|_| result.update_available) {
                let token =
                    self.token(&serde_json::to_vec(&(n, version, hash)).expect("版本可序列化"));
                st.offer = Some(Offer {
                    token: token.clone(),
                    version: version.clone(),
                    fingerprint: hash,
                });
                result.check = Some(token)
            }
            Ok(result)
        })
    }
    fn install_update(&self, check: Token, progress: Progress) -> Task<'_, InstallEnd> {
        Box::pin(async move {
            {
                let st = self.updates.lock().unwrap();
                if st.busy {
                    return Err(Fault::DesktopBusy {});
                }
                if !st.offer.as_ref().is_some_and(|o| o.token == check) {
                    return Err(Fault::DesktopChanged {});
                }
            }
            let exe = crate::install::swap::bin_path_in(&self.home);
            let hash = fingerprint(exe.clone()).await?;
            let offer = {
                let mut st = self.updates.lock().unwrap();
                if st.busy {
                    return Err(Fault::DesktopBusy {});
                }
                if !st.offer.as_ref().is_some_and(|o| o.token == check) {
                    return Err(Fault::DesktopChanged {});
                }
                let o = st.offer.take().unwrap();
                if hash != o.fingerprint {
                    return Err(Fault::DesktopChanged {});
                }
                st.busy = true;
                o
            };
            let mut job = Job {
                id: self.token(check.as_bytes()),
                requested_version: offer.version.clone(),
                targets: None,
                phase: None,
                phase_at: None,
                download: None,
                instances: vec![],
                end: None,
                fault: None,
            };
            progress(job.clone());
            let mut c = command(&self.home, &exe);
            c.args(["install", "--json", &offer.version]);
            let mut terminal = None;
            let status = lines(c, |value| {
                if fold(&mut job, &mut terminal, value)? {
                    progress(job.clone())
                }
                Ok(())
            })
            .await;
            let result = status.and_then(|status| match terminal {
                Some(Ok(end)) if status.success() => Ok(end),
                Some(Err(fault)) if !status.success() => Err(fault),
                _ => Err(protocol()),
            });
            match &result {
                Ok(end) => job.end = Some(end.clone()),
                Err(f) => job.fault = Some(f.clone()),
            }
            progress(job);
            self.updates.lock().unwrap().busy = false;
            result
        })
    }
}
