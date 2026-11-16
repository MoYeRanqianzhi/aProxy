//! `aproxy restore`：一键恢复先前非正常退出的实例。

use aproxy::daemon;
use aproxy::ops::restore::{Restore, restore_entry};

/// `aproxy restore`：一键恢复应当在运行、却不在运行的实例（崩溃/断电/系统重启之后）。
///
/// 依据期望状态记录（实例启动时写，只有 stop 删它）。空清单时静默成功而非报错——本命令
/// 的典型用法是配置为开机自启，重启后「之前全停了」是正常状态而非故障。幂等：已在运行的
/// 实例跳过，可重复执行。读不懂的记录（多半是更新的版本写的）报告出来、原样留着。
pub(crate) async fn handle_restore_cmd() {
    let listing = aproxy::spec::list_restore_in(&daemon::run_dir());
    for unreadable in &listing.unreadable {
        eprintln!(
            "跳过读不懂的记录 {}: {}",
            unreadable.path.display(),
            unreadable.reason
        );
    }
    let entries = listing.entries;
    if entries.is_empty() {
        println!("没有需要恢复的实例。");
        return;
    }
    let exe = std::env::current_exe().expect("无法定位自身可执行文件");
    for entry in entries {
        match restore_entry(&entry, &exe).await {
            Restore::AlreadyRunning => println!("端口 {} 已在运行，跳过。", entry.port),
            Restore::ConfigMissing { config } => eprintln!(
                "端口 {} 的配置文件找不到（{}），这次没有恢复；记录已保留，文件回来之后再执行 aproxy restore。不再需要这个实例时执行 aproxy stop {}",
                entry.port, config, entry.port
            ),
            Restore::Restored {
                pid,
                port,
                moved: true,
            } => println!(
                "已恢复 pid {pid}（端口 {port}；配置的端口已从 {} 变更，原端口的记录已由新端口接替）",
                entry.port
            ),
            Restore::Restored {
                pid,
                port,
                moved: false,
            } => println!("已恢复 pid {pid}（端口 {port}）"),
            Restore::Failed(e) => eprintln!(
                "端口 {} 恢复失败：{e}{}",
                entry.port,
                e.startup_log().map(|l| l.excerpt()).unwrap_or_default()
            ),
        }
    }
}
