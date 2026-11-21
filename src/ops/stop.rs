//! 停止一个实例。`aproxy stop` 与 `aproxy restart` 的前半段共用。

use std::time::Duration;

use crate::daemon::{self, InstanceRecord};
use crate::process::{RecordIdentity, record_identity};

/// 停止方式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopMode {
    /// IPC shutdown + 轮询确认退出（优雅，在途请求 10s 宽限强退）
    Graceful,
    /// 立即终止进程（零等待；pid + 创建时间核验防 PID 复用误杀，与二进制名无关）
    Force,
}

impl StopMode {
    /// 命令行的 `--force` 对应的停止方式
    pub fn from_force(force: bool) -> Self {
        if force {
            StopMode::Force
        } else {
            StopMode::Graceful
        }
    }
}

/// 实例确实停了，是怎样停的。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stopped {
    /// 收到停止请求后自行退出
    Graceful,
    /// 停止请求发不出去，核验发现进程已经不在（其间自行退出了）
    AlreadyGone,
    /// 被强制终止
    Forced,
}

/// 没能确认实例停下。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopFailure {
    /// restart 选中的实例已被另一代替换，端口不再能指向所选实例。
    GenerationChanged,
    /// 停止请求已送达，但在时限内没有退出：进程可能还在
    StillRunning,
    /// 停止请求发不出去，而进程还在（不应答控制通道）。`error` 是 IPC 的错误文字
    Unresponsive { error: String },
    /// 强制终止失败（身份核验不过、无权限）。恢复记录已放回，实例照旧受崩溃恢复保护；
    /// 放回失败时 `restore_error` 带着原因
    ForceFailed {
        error: String,
        restore_error: Option<String>,
    },
}

/// 优雅停止后等进程退出的时限：守护自己的 10 秒宽限强退，加一点余量。
const GRACEFUL_WAIT: Duration = Duration::from_secs(12);

/// 停下之后，这个实例还该不该被拉回来。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intent {
    /// 用户要它停下（`aproxy stop`）：期望状态记录一并删掉，restore 与看护者不再拉它
    Stop,
    /// 停下是为了马上再拉起（restart）：期望状态记录留着，调用方中途崩溃了，restore 与
    /// 看护者照样能把它拉回来
    Restart,
}

/// 停止单个实例（`aproxy stop` 的语义：期望状态记录一并删掉）。
pub async fn stop_instance(info: &InstanceRecord, mode: StopMode) -> Result<Stopped, StopFailure> {
    stop_with_intent(info, mode, Intent::Stop).await
}

/// 停止单个实例。
///
/// Graceful：发 IPC shutdown，轮询确认退出。
/// Force：立即终止（pid + 创建时间核验后，见 [`daemon::force_terminate`]），不等任何
/// 确认——进程对象的销毁是异步的，但信号已发，接着要拉起新实例的调用方由新实例的就绪判定
/// 兜底。
pub async fn stop_with_intent(
    info: &InstanceRecord,
    mode: StopMode,
    intent: Intent,
) -> Result<Stopped, StopFailure> {
    stop_in(
        &aproxy_ipc::home::home(),
        info,
        mode,
        intent,
        Authority::Cli,
    )
    .await
}

/// 来自桌面的选择可能早已过期；控制请求核验的是即将发送请求的连接，而非页面传来的端口。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Authority {
    /// 沿用命令行的目标选择、确认与强制语义。
    Cli,
    /// 对窗口上一次观察到的实例，执行前再次核验代际与控制连接。
    Observed,
}

/// 只在指定 home 执行停止或重启的停机阶段，保留调用者对期望状态的原有语义。
pub async fn stop_in(
    home: &std::path::Path,
    info: &InstanceRecord,
    mode: StopMode,
    intent: Intent,
    authority: Authority,
) -> Result<Stopped, StopFailure> {
    let port = daemon::port_of(&info.listen_addr).to_string();
    let run_dir = home.join("run");
    match mode {
        StopMode::Graceful => {
            let sent = if intent == Intent::Restart {
                match daemon::ipc_ping_in(&run_dir, &port).await {
                    Ok(live)
                        if live.instance.instance_id != info.instance_id
                            || (authority == Authority::Observed
                                && (live.instance.pid != info.pid
                                    || live.instance.start != info.start)) =>
                    {
                        return Err(StopFailure::GenerationChanged);
                    }
                    Ok(live) => shutdown_for_restart(&run_dir, &port, &live, authority).await,
                    Err(e) => Err(e),
                }
            } else if authority == Authority::Observed {
                aproxy_ipc::client::shutdown_for_run_in(&run_dir, &port, info, false).await
            } else {
                daemon::ipc_request_in(&run_dir, &port, daemon::IpcOp::STOP)
                    .await
                    .map(|_| ())
            };
            if authority == Authority::Observed {
                match &sent {
                    Err(daemon::IpcError::RunChanged) => {
                        return Err(StopFailure::GenerationChanged);
                    }
                    Err(e) => {
                        return Err(StopFailure::Unresponsive {
                            error: e.to_string(),
                        });
                    }
                    Ok(()) => {}
                }
            }
            settle_graceful(info, &run_dir, &port, intent, sent).await
        }
        // 强制终止之后进程不在、期望状态记录还在，正是「崩溃」的样子：看护者收到死亡事件
        // 即刻按记录重拉，aproxy restore 也会复活它。所以 stop 的强制终止要在终止**之前**
        // 删掉记录——看护者被死亡事件即时唤醒，终止之后再删必然与它赛跑。终止失败（身份
        // 核验不过、无权限）时原样放回，实例照旧受崩溃恢复保护。
        // restart 的强制终止不删：调用方接着用这份记录拉起新实例；看护者抢先拉起的话，
        // 两边拉的是同一份规格，由调用方按「端口已被同一配置的实例占着」收敛。
        StopMode::Force => {
            if authority == Authority::Observed {
                match daemon::ipc_probe_in(&run_dir, &port).await {
                    Err(daemon::IpcError::Unreachable(_) | daemon::IpcError::Transient(_)) => {}
                    _ => return Err(StopFailure::GenerationChanged),
                }
                if !daemon::read_instance_file_in(&run_dir, &port).is_some_and(|r| {
                    r.instance_id == info.instance_id && r.pid == info.pid && r.start == info.start
                }) || record_identity(info.pid, &info.start) != RecordIdentity::Alive
                {
                    return Err(StopFailure::GenerationChanged);
                }
            }
            let saved = match intent {
                Intent::Stop => {
                    let saved = crate::spec::read_restore_in(&run_dir, &port).ok().flatten();
                    crate::spec::remove_restore_in(&run_dir, &port);
                    saved
                }
                Intent::Restart => None,
            };
            match daemon::force_terminate(info) {
                Ok(()) => {
                    // 实例来不及自清的运行期文件（实例记录、socket、心跳）代它收掉
                    if authority == Authority::Cli {
                        daemon::remove_stopped_files_in(&run_dir, &port);
                    } else if daemon::read_instance_file_in(&run_dir, &port).is_some_and(|r| {
                        r.pid == info.pid
                            && r.start == info.start
                            && r.instance_id == info.instance_id
                    }) {
                        // Unix 端点可能已绑定给尚未发布记录的后继；让下次 bind 自行回收残留 socket。
                        let _ =
                            std::fs::remove_file(daemon::instance_file_path_in(&run_dir, &port));
                        crate::watchdog::remove_heartbeat_file(&run_dir, &port);
                    }
                    Ok(Stopped::Forced)
                }
                Err(error) => {
                    let restore_error = saved
                        .filter(|_| {
                            authority == Authority::Cli
                                || matches!(crate::spec::read_restore_in(&run_dir, &port), Ok(None))
                        })
                        .and_then(|entry| {
                            crate::spec::write_restore_in(&run_dir, &port, &entry.spec)
                                .err()
                                .map(|e| e.to_string())
                        });
                    Err(StopFailure::ForceFailed {
                        error,
                        restore_error,
                    })
                }
            }
        }
    }
}

/// 停止请求发出之后的收尾：送达了就等实例退出；发不出去就区分「其间已自行退出」与
/// 「进程还在、只是不应答」，前者就是停止的结果，后者只能强制。
async fn settle_graceful(
    info: &InstanceRecord,
    run_dir: &std::path::Path,
    port: &str,
    intent: Intent,
    sent: Result<(), daemon::IpcError>,
) -> Result<Stopped, StopFailure> {
    match sent {
        Ok(()) => {
            if daemon::wait_until_gone_in(run_dir, port, GRACEFUL_WAIT).await {
                Ok(Stopped::Graceful)
            } else {
                Err(StopFailure::StillRunning)
            }
        }
        Err(e) => match record_identity(info.pid, &info.start) {
            RecordIdentity::Gone | RecordIdentity::Reused => {
                // 进程自己没了、没来得及兑现这次 stop：用户要它停下，记录由这里删
                if intent == Intent::Stop {
                    crate::spec::remove_restore_in(run_dir, port);
                }
                Ok(Stopped::AlreadyGone)
            }
            RecordIdentity::Alive => Err(StopFailure::Unresponsive {
                error: e.to_string(),
            }),
        },
    }
}

/// `stop idle` 对一个实例的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdleStop {
    /// 停了
    Stopped(Stopped),
    /// 被选中之后它又收到了请求，已经不够闲：没有停它。`idle_secs` 是它实际闲置了多久
    Busy { idle_secs: u64 },
}

/// 只在实例已经闲置至少 `idle_secs` 秒时优雅停止它（`aproxy stop idle` 的语义）。
///
/// 判断与停止交给实例一次完成（shutdown 的 `if_idle_secs`）：命令行先查闲置、再发停止的话，
/// 两步之间到达的请求会被打断。不认这个参数的旧实例退回无条件的停止——调用方已经按它
/// 刚才上报的闲置时长选中了它。
pub async fn stop_if_idle(info: &InstanceRecord, idle_secs: u64) -> Result<IdleStop, StopFailure> {
    let port = daemon::port_of(&info.listen_addr).to_string();
    let run_dir = daemon::run_dir();
    let conditional = daemon::ipc_ping_in(&run_dir, &port)
        .await
        .is_ok_and(|live| {
            live.features
                .iter()
                .any(|f| f == daemon::FEATURE_SHUTDOWN_IF_IDLE_SECS)
        });
    if !conditional {
        return stop_instance(info, StopMode::Graceful)
            .await
            .map(IdleStop::Stopped);
    }
    let op = daemon::IpcOp::Shutdown {
        keep_restore: false,
        if_idle_secs: Some(idle_secs),
    };
    let sent = match daemon::ipc_request_in(&run_dir, &port, op).await {
        Err(daemon::IpcError::Remote { code, details, .. })
            if code == daemon::error_code::CONDITION_NOT_MET =>
        {
            return Ok(IdleStop::Busy {
                idle_secs: details["idle_ms"].as_u64().unwrap_or(0) / 1000,
            });
        }
        other => other.map(|_| ()),
    };
    settle_graceful(info, &run_dir, &port, Intent::Stop, sent)
        .await
        .map(IdleStop::Stopped)
}

/// 发 shutdown。要留着期望状态记录（[`Intent::Restart`]）时先看实例认不认 `keep_restore`：
/// 不认这个参数的实例发不带参数的 shutdown（它会删掉记录），等它退出后由这里把记录原样
/// 写回。
async fn shutdown_for_restart(
    run_dir: &std::path::Path,
    port: &str,
    live: &daemon::InstanceStatus,
    authority: Authority,
) -> Result<(), daemon::IpcError> {
    let send = |keep_restore| async move {
        if authority == Authority::Observed {
            aproxy_ipc::client::shutdown_for_run_in(run_dir, port, &live.instance, keep_restore)
                .await
        } else {
            let op = if keep_restore {
                daemon::IpcOp::STOP_KEEPING_RESTORE
            } else {
                daemon::IpcOp::STOP
            };
            daemon::ipc_request_in(run_dir, port, op).await.map(|_| ())
        }
    };
    if live
        .features
        .iter()
        .any(|f| f == daemon::FEATURE_SHUTDOWN_KEEP_RESTORE)
    {
        return send(true).await;
    }
    let saved = crate::spec::read_restore_in(run_dir, port).ok().flatten();
    send(false).await?;
    if let Some(entry) = saved {
        // 旧实例在应答之前就删了记录；它退出之前写回会被它的退出清理再删一次，所以等
        // 端点消失再写。等不到也写：记录在总比不在安全
        daemon::wait_until_gone_in(run_dir, port, GRACEFUL_WAIT).await;
        if authority == Authority::Cli
            || matches!(crate::spec::read_restore_in(run_dir, port), Ok(None))
        {
            let _ = crate::spec::write_restore_in(run_dir, port, &entry.spec);
        }
    }
    Ok(())
}

/// stop 一个已经不在运行的端口：它若还留着期望状态记录（崩溃之后、看护者还没拉回来，
/// 或看护者关着），删掉记录，restore 与看护者就不再拉它。返回是否删了记录。
/// 端口上还有实例进程（包括不应答的）时不动：那要先把进程停下。
pub async fn forget_stopped_port(port: &str) -> bool {
    forget_port_in(&daemon::run_dir(), port, None).await == Forget::Removed
}

/// 端口的期望记录不同于正在运行的进程；核验之后只删除前者。
#[derive(PartialEq, Eq)]
pub enum Forget {
    /// 已删除选中的期望记录。
    Removed,
    /// 已经没有记录。
    Absent,
    /// 端口已有实例、期望被替换、或无法确认所有权。
    Changed,
    /// 仍是选中的记录，但操作系统拒绝删除。
    Failed,
}

/// 带上观察时的整份记录可拒绝删除后继实例的期望；CLI 直接按端口操作则不给前置条件。
pub async fn forget_port_in(
    run_dir: &std::path::Path,
    port: &str,
    expected: Option<&crate::spec::RestoreEntry>,
) -> Forget {
    if aproxy_ipc::registry::port_has_instance_in(run_dir, port).await {
        return Forget::Changed;
    }
    if let Some(expected) = expected {
        match crate::spec::read_restore_in(run_dir, port) {
            Ok(Some(e)) if &e == expected => {}
            Ok(None) => return Forget::Absent,
            _ => return Forget::Changed,
        }
    }
    let path = crate::spec::restore_path_in(run_dir, port);
    let existed = path.is_file();
    match std::fs::remove_file(path) {
        Ok(()) => Forget::Removed,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Forget::Absent,
        Err(_) if expected.is_none() && existed => Forget::Removed,
        Err(_) => Forget::Failed,
    }
}

/// stop 一个按配置文件（别名或路径）指定、已经不在运行的目标：删掉规格指向这个配置文件、
/// 端口上又没有实例进程的期望状态记录。返回被删记录的端口。
pub async fn forget_stopped_config(config_path: &std::path::Path) -> Vec<String> {
    let run_dir = daemon::run_dir();
    let key = super::target::config_path_key(&config_path.display().to_string());
    let mut removed = Vec::new();
    for entry in crate::spec::list_restore_in(&run_dir).entries {
        let same = entry
            .spec
            .config_path
            .as_deref()
            .is_some_and(|p| super::target::config_path_key(p) == key);
        if same && !aproxy_ipc::registry::port_has_instance_in(&run_dir, &entry.port).await {
            crate::spec::remove_restore_in(&run_dir, &entry.port);
            removed.push(entry.port);
        }
    }
    removed
}

/// `stop all` 的收尾：删掉全部「端口上已经没有实例进程」的期望状态记录，返回它们的端口，
/// 以及读不懂、因而原样留下的记录（多半由更新的版本写下，交给调用方报告）。
pub async fn forget_all_stopped() -> (Vec<String>, Vec<crate::spec::UnreadableRecord>) {
    let run_dir = daemon::run_dir();
    let listing = crate::spec::list_restore_in(&run_dir);
    let mut removed = Vec::new();
    for entry in listing.entries {
        if !aproxy_ipc::registry::port_has_instance_in(&run_dir, &entry.port).await {
            crate::spec::remove_restore_in(&run_dir, &entry.port);
            removed.push(entry.port);
        }
    }
    (removed, listing.unreadable)
}
