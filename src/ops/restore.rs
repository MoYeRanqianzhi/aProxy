//! 恢复应当在运行、却不在运行的实例（崩溃、断电、系统重启之后）。
//!
//! 依据是期望状态记录 `run/<端口>.restore.json`（见 [`crate::spec`]：实例启动时写，只有
//! stop 删它）。清单由 [`crate::spec::list_restore_in`] 给出，这里处理其中一条；清单为空
//! 不是错误——本操作的典型用法是配置为开机自启，「之前全停了」是正常状态。

use std::path::Path;

use crate::daemon;
use crate::spawn::{self, SpawnError};
use crate::spec::RestoreEntry;

/// 一条期望状态记录的处理结果。
#[derive(Debug)]
pub enum Restore {
    /// 记录的端口上已有实例在运行（被手工拉起，或已被看护者拉回）：不重复启动
    AlreadyRunning,
    /// 记录里的配置文件此刻找不到。记录保留：文件可能只是暂时不在（网络盘还没挂上、
    /// 移动硬盘还没插），回来之后再执行一次就能恢复。`config` 是记录里的路径
    ConfigMissing { config: String },
    /// 已恢复。`port` 是实例实际监听的端口；`moved` 为 true 表示它与记录的端口不同
    /// （配置里的端口改过），原端口的记录已由新端口的记录接替
    Restored { pid: u32, port: String, moved: bool },
    /// 没能拉起或没等到就绪。记录保留
    Failed(SpawnError),
}

/// 按一条期望状态记录拉起实例。幂等：已在运行的跳过，可重复执行。`exe` 是用来拉起实例的
/// 二进制。
pub async fn restore_entry(entry: &RestoreEntry, exe: &Path) -> Restore {
    // 幂等：记录存在而实例已在运行时，不重复启动
    if daemon::ipc_ping(&entry.port).await.is_ok() {
        return Restore::AlreadyRunning;
    }
    // 没有配置文件的规格（只靠覆盖参数与全局默认启动的实例）不做这项检查
    if let Some(cfg) = entry.spec.config_file()
        && !cfg.exists()
    {
        return Restore::ConfigMissing {
            config: cfg.display().to_string(),
        };
    }
    // 就绪的判定按新 pid 在注册表里定位实际端口（见 crate::spawn）。不能 ping 记录里的
    // 端口：实例监听哪个端口由它此刻读到的配置决定——用户改了 config.toml 的端口却没
    // restart，新实例就落在别的端口，ping 原端口只会误报「未就绪」
    let home = aproxy_ipc::home::home();
    match spawn::spawn_from_spec(&home, exe, &entry.spec, spawn::READY_TIMEOUT).await {
        Ok(live) => {
            let port = daemon::port_of(&live.instance.listen_addr).to_string();
            // 实例落在新端口：它已按实际端口写了自己的记录，原端口那份被接替。不删的话
            // stop 新端口之后，下一次 restore（或看护者）会凭旧记录把实例再拉起来
            let moved =
                daemon::retire_moved_port_records_in(&daemon::run_dir(), &entry.port, &port);
            Restore::Restored {
                pid: live.instance.pid,
                port,
                moved,
            }
        }
        Err(e) => Restore::Failed(e),
    }
}
