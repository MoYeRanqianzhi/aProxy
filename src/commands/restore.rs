//! `aproxy restore`：一键恢复先前非正常退出的实例。

#[path = "../i18n/restore.rs"]
mod restore_text;
use aproxy::ops::restore::{Restore, restore_entry_in};
use aproxy_ipc::i18n::current;

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
pub(crate) async fn handle_restore_cmd(home: &std::path::Path, json: bool) {
    let listing = aproxy::spec::list_restore_in(&home.join("run"));
    for unreadable in &listing.unreadable {
        eprintln!(
            "{}",
            restore_text::skip(
                current(),
                unreadable.path.display(),
                &unreadable.reason.diagnostic().render(current())
            )
        );
    }
    let mut results = Results::new(None);
    let entries = listing.entries;
    if entries.is_empty() {
        say(json, restore_text::none(current()));
        results.finish(json)
    }
    let exe = std::env::current_exe().expect("cannot locate the current executable");
    for entry in entries {
        match restore_entry_in(home, &entry, &exe).await {
            Restore::AlreadyRunning => {
                say(json, restore_text::running(current(), &entry.port));
                results.ok(&entry.port, "already_running", None);
            }
            Restore::ConfigMissing { config } => {
                eprintln!("{}", restore_text::missing(current(), &entry.port, &config));
                results.deferred(&entry.port, "config_missing");
            }
            // 这一项的端口是实例实际监听的：配置改过端口时与记录的不同
            Restore::Restored { pid, port, moved } => {
                if moved {
                    say(
                        json,
                        restore_text::moved(current(), pid, &port, &entry.port),
                    );
                } else {
                    say(json, restore_text::restored(current(), pid, &port));
                }
                results.ok(&port, "restored", None);
            }
            Restore::Failed(e) => {
                let failure = super::spawn_failure(&e, |lang| {
                    restore_text::failed(lang, &entry.port, &e.render(lang), &e.excerpt(lang))
                });
                eprintln!("{}", failure.terminal());
                results.failed(&entry.port, "failed", &failure);
            }
        }
    }
    results.finish(json)
}
