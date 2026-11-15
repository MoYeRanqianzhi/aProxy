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

/// 停止单个实例。
///
/// Graceful：发 IPC shutdown，轮询确认退出。
/// Force：立即终止（pid + 创建时间核验后，见 [`daemon::force_terminate`]），不等任何
/// 确认——进程对象的销毁是异步的，但信号已发，接着要拉起新实例的调用方由新实例的就绪判定
/// 兜底。
pub async fn stop_instance(info: &InstanceRecord, mode: StopMode) -> Result<Stopped, StopFailure> {
    let port = daemon::port_of(&info.listen_addr).to_string();
    match mode {
        StopMode::Graceful => match daemon::ipc_request(&port, daemon::IpcOp::Shutdown).await {
            Ok(_) => {
                if daemon::wait_until_gone(&port, GRACEFUL_WAIT).await {
                    Ok(Stopped::Graceful)
                } else {
                    Err(StopFailure::StillRunning)
                }
            }
            // 发不出 shutdown：区分「其间已自行退出」与「进程还在、只是不应答」，
            // 前者就是停止的结果，后者只能强制
            Err(e) => match record_identity(info.pid, info.process_start) {
                RecordIdentity::Gone | RecordIdentity::Reused => Ok(Stopped::AlreadyGone),
                RecordIdentity::Alive(_) => Err(StopFailure::Unresponsive {
                    error: e.to_string(),
                }),
            },
        },
        // 被强杀的守护来不及做优雅退出的自清，.restore 留着就是「崩溃」信号：
        // 看门狗收到死亡事件即刻按它重拉，aproxy restore 也会复活它，stop
        // 语义落空。所以 .restore 必须在终止**之前**摘掉——看门狗被死亡事件
        // 即时唤醒，终止之后再删必然与它赛跑。终止失败（身份核验不过、无权
        // 限）时原样放回，实例照旧受崩溃恢复保护。restart --force 同样摘掉：
        // 否则看门狗与 restart 会同时拉起新实例争抢端口；新实例起不来时与
        // 优雅重启的失败同态（无恢复记录），调用方要把恢复命令告诉用户。
        StopMode::Force => {
            let saved = daemon::list_restore_entries()
                .into_iter()
                .find(|e| e.port == port);
            daemon::remove_restore_file(&info.listen_addr);
            match daemon::force_terminate(info) {
                Ok(()) => {
                    // 其余残留按守护自清的顺序收掉（.pid、socket、心跳）
                    daemon::remove_registry_files(&info.listen_addr);
                    Ok(Stopped::Forced)
                }
                Err(error) => {
                    let restore_error = saved.and_then(|entry| {
                        daemon::write_restore_file(&info.listen_addr, &entry.args, &entry.log_path)
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
