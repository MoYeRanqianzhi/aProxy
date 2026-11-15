//! 恢复先前非正常退出（崩溃、断电、系统重启）的实例。
//!
//! 依据 run/<端口>.restore 恢复记录（守护 bind 成功时写入、优雅退出时删除）。记录清单由
//! [`crate::daemon::list_restore_entries`] 给出，这里处理其中一条；清单为空不是错误——
//! 本操作的典型用法是配置为开机自启，「之前全关了」是正常状态。

use std::path::Path;

use crate::daemon::{self, RestoreEntry};
use crate::spawn::{self, SpawnError};

/// 一条恢复记录的处理结果。
#[derive(Debug)]
pub enum Restore {
    /// 记录的端口上已有实例在运行（被手工拉起，或同端口重启过）：不重复启动
    AlreadyRunning,
    /// 记录里的配置文件已不存在，无法忠实恢复：记录已清理，下次不再重试。`config` 是
    /// 记录里的路径
    ConfigMissing { config: String },
    /// 已恢复。`port` 是实例实际监听的端口；`moved` 为 true 表示它与记录的端口不同
    /// （配置里的端口改过），原端口的恢复记录已清理
    Restored { pid: u32, port: String, moved: bool },
    /// 没能拉起或没等到就绪
    Failed(SpawnError),
}

/// 按一条恢复记录拉起实例。幂等：已在运行的跳过，可重复执行。`exe` 是用来拉起实例的
/// 二进制。
pub async fn restore_entry(entry: &RestoreEntry, exe: &Path) -> Restore {
    // 幂等：记录存在但实例已被手工拉起（或同端口重启过）时，不重复启动
    if daemon::ipc_ping(&entry.port).await.is_ok() {
        return Restore::AlreadyRunning;
    }
    // 配置文件已被删除的记录无法忠实恢复，清理之（下次 restore 不再重试）
    let cfg_arg = entry
        .args
        .iter()
        .position(|a| a == "--config")
        .and_then(|i| entry.args.get(i + 1));
    if let Some(cfg) = cfg_arg
        && !Path::new(cfg).exists()
    {
        daemon::remove_restore_file(&entry.port);
        return Restore::ConfigMissing {
            config: cfg.clone(),
        };
    }
    // 就绪的判定按新 pid 在注册表里定位实际端口（见 crate::spawn）。不能 ping 记录里的
    // 端口：恢复用的是记录里的原参数，但实例监听哪个端口由它此刻读到的配置决定——用户改了
    // config.toml 端口却没 restart、或 listen 端口为 0，新实例都落在别的端口，ping 原端口
    // 只会误报「未就绪」
    let run_dir = daemon::run_dir();
    match spawn::spawn_instance(&run_dir, exe, &entry.args, spawn::READY_TIMEOUT).await {
        Ok(live) => {
            let port = daemon::port_of(&live.instance.listen_addr).to_string();
            // 实例落在新端口：原端口的 .restore 是已死旧进程的残留，新实例已按实际端口写了
            // 自己的记录。不清理的话 stop 新端口后下一次 restore（或看护者）会凭它把实例
            // 再拉起来
            let moved = daemon::retire_moved_port_records_in(&run_dir, &entry.port, &port);
            Restore::Restored {
                pid: live.instance.pid,
                port,
                moved,
            }
        }
        Err(e) => Restore::Failed(e),
    }
}
