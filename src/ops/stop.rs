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
    let port = daemon::port_of(&info.listen_addr).to_string();
    let run_dir = daemon::run_dir();
    match mode {
        StopMode::Graceful => match graceful_shutdown(&run_dir, &port, intent).await {
            Ok(_) => {
                if daemon::wait_until_gone(&port, GRACEFUL_WAIT).await {
                    Ok(Stopped::Graceful)
                } else {
                    Err(StopFailure::StillRunning)
                }
            }
            // 发不出 shutdown：区分「其间已自行退出」与「进程还在、只是不应答」，
            // 前者就是停止的结果，后者只能强制
            Err(e) => match record_identity(info.pid, &info.start) {
                RecordIdentity::Gone | RecordIdentity::Reused => {
                    // 进程自己没了、没来得及兑现这次 stop：用户要它停下，记录由这里删
                    if intent == Intent::Stop {
                        crate::spec::remove_restore_in(&run_dir, &port);
                    }
                    Ok(Stopped::AlreadyGone)
                }
                RecordIdentity::Alive => Err(StopFailure::Unresponsive {
                    error: e.to_string(),
                }),
            },
        },
        // 强制终止之后进程不在、期望状态记录还在，正是「崩溃」的样子：看护者收到死亡事件
        // 即刻按记录重拉，aproxy restore 也会复活它。所以 stop 的强制终止要在终止**之前**
        // 删掉记录——看护者被死亡事件即时唤醒，终止之后再删必然与它赛跑。终止失败（身份
        // 核验不过、无权限）时原样放回，实例照旧受崩溃恢复保护。
        // restart 的强制终止不删：调用方接着用这份记录拉起新实例；看护者抢先拉起的话，
        // 两边拉的是同一份规格，由调用方按「端口已被同一配置的实例占着」收敛。
        StopMode::Force => {
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
                    daemon::remove_registry_files(&info.listen_addr);
                    Ok(Stopped::Forced)
                }
                Err(error) => {
                    let restore_error = saved.and_then(|entry| {
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

/// 发 shutdown。要留着期望状态记录（[`Intent::Restart`]）时先看实例认不认 `keep_restore`：
/// 不认这个参数的旧实例会静默忽略它、照常删掉记录，所以对它们发不带参数的 shutdown，
/// 等它退出后由这里把记录原样写回。
async fn graceful_shutdown(
    run_dir: &std::path::Path,
    port: &str,
    intent: Intent,
) -> Result<(), daemon::IpcError> {
    if intent == Intent::Stop {
        return daemon::ipc_request_in(run_dir, port, daemon::IpcOp::STOP)
            .await
            .map(|_| ());
    }
    let live = daemon::ipc_ping_in(run_dir, port).await?;
    if live
        .features
        .iter()
        .any(|f| f == daemon::FEATURE_SHUTDOWN_KEEP_RESTORE)
    {
        return daemon::ipc_request_in(
            run_dir,
            port,
            daemon::IpcOp::Shutdown { keep_restore: true },
        )
        .await
        .map(|_| ());
    }
    let saved = crate::spec::read_restore_in(run_dir, port).ok().flatten();
    daemon::ipc_request_in(run_dir, port, daemon::IpcOp::STOP).await?;
    if let Some(entry) = saved {
        // 旧实例在应答之前就删了记录；它退出之前写回会被它的退出清理再删一次，所以等
        // 端点消失再写。等不到也写：记录在总比不在安全
        daemon::wait_until_gone(port, GRACEFUL_WAIT).await;
        let _ = crate::spec::write_restore_in(run_dir, port, &entry.spec);
    }
    Ok(())
}

/// 一个端口上有没有本 home 的实例进程：应答控制通道的，或不应答但进程经核验仍在的。
async fn port_has_instance(run_dir: &std::path::Path, port: &str) -> bool {
    if daemon::ipc_ping_in(run_dir, port).await.is_ok() {
        return true;
    }
    daemon::read_instance_file_in(run_dir, port)
        .is_some_and(|info| record_identity(info.pid, &info.start) == RecordIdentity::Alive)
}

/// stop 一个已经不在运行的端口：它若还留着期望状态记录（崩溃之后、看护者还没拉回来，
/// 或看护者关着），删掉记录，restore 与看护者就不再拉它。返回是否删了记录。
/// 端口上还有实例进程（包括不应答的）时不动：那要先把进程停下。
pub async fn forget_stopped_port(port: &str) -> bool {
    let run_dir = daemon::run_dir();
    if port_has_instance(&run_dir, port).await {
        return false;
    }
    let had = crate::spec::restore_path_in(&run_dir, port).is_file();
    crate::spec::remove_restore_in(&run_dir, port);
    had
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
        if same && !port_has_instance(&run_dir, &entry.port).await {
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
        if !port_has_instance(&run_dir, &entry.port).await {
            crate::spec::remove_restore_in(&run_dir, &entry.port);
            removed.push(entry.port);
        }
    }
    (removed, listing.unreadable)
}
