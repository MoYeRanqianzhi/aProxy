//! `aproxy restore`：一键恢复先前非正常退出的实例。

use aproxy::daemon;
use aproxy::ops::restore::{Restore, restore_entry};

/// `aproxy restore`：一键恢复先前非正常退出（崩溃/断电/系统重启）的实例。
///
/// 依据 run/<端口>.restore 恢复记录（守护 bind 成功时写入、优雅退出时删除）。
/// 空清单时静默成功而非报错——本命令的典型用法是配置为开机自启，重启后
/// 「之前全关了」是正常状态而非故障。幂等：已在运行的实例跳过，可重复执行。
pub(crate) async fn handle_restore_cmd() {
    let entries = daemon::list_restore_entries();
    if entries.is_empty() {
        println!("没有需要恢复的实例。");
        return;
    }
    let exe = std::env::current_exe().expect("无法定位自身可执行文件");
    for entry in entries {
        match restore_entry(&entry, &exe).await {
            Restore::AlreadyRunning => println!("端口 {} 已在运行，跳过。", entry.port),
            Restore::ConfigMissing { config } => println!(
                "端口 {} 的配置文件已不存在（{}），跳过并清理该记录。",
                entry.port, config
            ),
            Restore::Restored {
                pid,
                port,
                moved: true,
            } => println!(
                "已恢复 pid {pid}（端口 {port}；配置的端口已从 {} 变更，原端口的恢复记录已清理）",
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
