//! `aproxy restore`：一键恢复先前非正常退出的实例。

use aproxy::daemon;
use aproxy::ops::restore::{Restore, restore_entry};

use super::{Results, say};

/// `aproxy restore`：一键恢复应当在运行、却不在运行的实例（崩溃/断电/系统重启之后）。
///
/// 依据期望状态记录（实例启动时写，只有 stop 删它）。空清单时静默成功而非报错——本命令
/// 的典型用法是配置为开机自启，重启后「之前全停了」是正常状态而非故障。幂等：已在运行的
/// 实例跳过，可重复执行。读不懂的记录（多半是更新的版本写的）报告出来、原样留着。
///
/// 每条记录一项结局：`restored`、`already_running`、`config_missing`、`failed`。全部失败时
/// 退出码 1；有成有败，或有配置文件暂时不在的记录（`config_missing`，它没被恢复，但记录还
/// 留着等文件回来）时退出码 5。
pub(crate) async fn handle_restore_cmd(json: bool) {
    let listing = aproxy::spec::list_restore_in(&daemon::run_dir());
    for unreadable in &listing.unreadable {
        eprintln!(
            "跳过读不懂的记录 {}: {}",
            unreadable.path.display(),
            unreadable.reason
        );
    }
    let mut results = Results::new(None);
    let entries = listing.entries;
    if entries.is_empty() {
        say(json, "没有需要恢复的实例。");
        results.finish(json)
    }
    let exe = std::env::current_exe().expect("无法定位自身可执行文件");
    for entry in entries {
        match restore_entry(&entry, &exe).await {
            Restore::AlreadyRunning => {
                say(json, format!("端口 {} 已在运行，跳过。", entry.port));
                results.ok(&entry.port, "already_running", None);
            }
            Restore::ConfigMissing { config } => {
                eprintln!(
                    "端口 {} 的配置文件找不到（{}），这次没有恢复；记录已保留，文件回来之后再执行 aproxy restore。不再需要这个实例时执行 aproxy stop {}",
                    entry.port, config, entry.port
                );
                results.deferred(&entry.port, "config_missing");
            }
            // 这一项的端口是实例实际监听的：配置改过端口时与记录的不同
            Restore::Restored { pid, port, moved } => {
                if moved {
                    say(
                        json,
                        format!(
                            "已恢复 pid {pid}（端口 {port}；配置的端口已从 {} 变更，原端口的记录已由新端口接替）",
                            entry.port
                        ),
                    );
                } else {
                    say(json, format!("已恢复 pid {pid}（端口 {port}）"));
                }
                results.ok(&port, "restored", None);
            }
            Restore::Failed(e) => {
                let failure = super::spawn_failure(
                    &e,
                    format!(
                        "端口 {} 恢复失败：{e}{}",
                        entry.port,
                        e.startup_log().map(|l| l.excerpt()).unwrap_or_default()
                    ),
                );
                eprintln!("{}", failure.message);
                results.failed(&entry.port, "failed", &failure);
            }
        }
    }
    results.finish(json)
}
