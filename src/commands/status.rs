//! `aproxy status [--idle|--busy] [--json]`：只读清点 home 里的实例、记录与其他进程。

#[path = "../i18n/status.rs"]
pub(super) mod text;
use aproxy_ipc::i18n::current;

use aproxy::daemon;
use aproxy::ops::enumerate::{
    Enumerated, Enumeration, Liveness, RESTORE_REASON_CONFIG_MISSING, WATCHDOG_RUNNING, enumerate,
};

use crate::util::humanize_duration;

/// 清点一次并按请求输出文字或 JSON，使两种格式都以同一份只读清点结果为准。
/// idle/busy 按运行中实例的最近活动时间与 settings.idle_timeout_secs 判定。
pub(crate) async fn handle_status_cmd(idle_only: bool, busy_only: bool, json: bool) {
    let idle_threshold_secs = aproxy::settings::load().idle_timeout_secs;
    let enumeration = enumerate().await;
    let now = daemon::now_millis();
    let had_live = enumeration
        .instances
        .iter()
        .any(|entry| entry.liveness == Liveness::Live);
    // 只有运行中的实例能按闲置与否归类；不应答、无法采信应答、等待恢复的都要报。
    let filtered_enumeration = Enumeration {
        instances: enumeration
            .instances
            .into_iter()
            .filter(|entry| {
                if entry.liveness != Liveness::Live || !(idle_only || busy_only) {
                    return true;
                }
                let status = entry.status.as_ref().unwrap();
                let idle_secs = status.activity.idle_secs_at(now);
                (idle_secs >= idle_threshold_secs) == idle_only
            })
            .collect(),
        ..enumeration
    };
    if json {
        super::print_doc(serde_json::to_value(&filtered_enumeration).unwrap());
        return;
    }

    // 按存活状态分块，保留清点结果中各组内的端口顺序。
    let mut unresponsive = Vec::new();
    let mut live = Vec::new();
    let mut incompatible = Vec::new();
    let mut down = Vec::new();
    for entry in &filtered_enumeration.instances {
        match entry.liveness {
            Liveness::Unresponsive => unresponsive.extend(entry.instance.as_ref()),
            Liveness::Live => live.push(entry),
            Liveness::Incompatible | Liveness::Unknown => incompatible.push(entry),
            Liveness::Down => down.push(entry),
        }
    }
    print_unresponsive(&unresponsive);
    if !live.is_empty() {
        print_live(&live, now);
    } else if had_live {
        let which = if idle_only {
            text::idle(current())
        } else {
            text::active(current())
        };
        println!("{}", text::filtered(current(), which, idle_threshold_secs));
    }

    if !incompatible.is_empty() {
        println!("{}", text::incompatible(current(), incompatible.len()));
        for entry in &incompatible {
            let port = entry.port;
            let (pid, version) = match &entry.instance {
                Some(info) => (info.pid.to_string(), info.version.as_str()),
                None => ("?".to_string(), "?"),
            };
            let code = entry
                .reason
                .as_ref()
                .map_or("unknown", |reason| reason.code.as_str());
            println!(
                "{}",
                text::incompatible_row(current(), port, &pid, version, code)
            );
        }
    }
    if !down.is_empty() {
        println!("{}", text::pending(current(), down.len()));
        for entry in &down {
            let port = entry.port;
            if entry.restore_reason.as_deref() == Some(RESTORE_REASON_CONFIG_MISSING) {
                println!("{}", text::missing(current(), port));
            } else {
                println!("{}", text::down_port(current(), port));
            }
            println!("{}", text::forget(current(), port));
        }
    }
    if !had_live && unresponsive.is_empty() && incompatible.is_empty() && down.is_empty() {
        println!("{}", text::none(current()));
    }

    if !filtered_enumeration.unreadable.is_empty() {
        println!(
            "{}",
            text::unreadable(current(), filtered_enumeration.unreadable.len())
        );
        for entry in &filtered_enumeration.unreadable {
            println!("  {}  [{}]", entry.file, entry.reason.code);
        }
    }
    if filtered_enumeration.gc_paused.is_some() {
        println!("{}", text::gc_paused(current()));
    }
    if let Some(w) = &filtered_enumeration.watchdog {
        let since_scan = humanize_duration(now.saturating_sub(w.last_scan_at) / 1000);
        if w.state == WATCHDOG_RUNNING {
            println!(
                "{}",
                text::watchdog(current(), w.pid, &w.version, &since_scan)
            );
        } else {
            println!(
                "{}",
                text::watchdog_stale(current(), w.pid, &w.version, &since_scan)
            );
        }
    }
    if !filtered_enumeration.processes.is_empty() {
        println!(
            "{}",
            text::processes(current(), filtered_enumeration.processes.len())
        );
        for process in &filtered_enumeration.processes {
            println!(
                "  pid {}  {}  v{}",
                process.pid, process.role, process.version
            );
        }
    }
}

/// 列出可采信的实时状态；运行时长、闲置时长与最近错误的时刻使用同一次清点的时间。
fn print_live(live: &[&Enumerated], now: u64) {
    println!("{}", text::running(current(), live.len()));
    let cli_version = env!("CARGO_PKG_VERSION");
    for entry in live {
        // 清点只在控制通道给出可采信的应答时标为 Live，此时两份数据都在。
        let info = entry.instance.as_ref().unwrap();
        let status = entry.status.as_ref().unwrap();
        let activity = &status.activity;
        let idle_secs = activity.idle_secs_at(now);
        println!(
            "{}",
            text::live(
                current(),
                daemon::port_of(&info.listen_addr),
                info.pid,
                &info.version,
                &humanize_duration(now.saturating_sub(info.started_at) / 1000),
                &humanize_duration(idle_secs)
            )
        );
        println!(
            "{}",
            text::endpoints(
                current(),
                &info.listen_addr,
                &aproxy_ipc::redact::url(&info.base_url),
                config_label(&info.config_path)
            )
        );
        println!(
            "{}",
            text::launch_line(current(), &info.launch_reason, &info.launch_note)
        );
        let err_text = match &activity.last_error {
            // 码在前，便于按码检索；没有码的（更旧的实例）只显示说明
            Some(e) => text::recent(
                current(),
                &if e.code.is_empty() {
                    String::new()
                } else {
                    format!("{}: ", e.code)
                },
                &aproxy::proxy::failure_text::recent(e, current()),
                &humanize_duration(now.saturating_sub(e.at) / 1000),
            ),
            None => text::no_error(current()).to_string(),
        };
        println!(
            "{}",
            text::requests(
                current(),
                activity.requests_total,
                activity.in_flight,
                activity.retries_total,
                activity.rescued_total,
                &err_text
            )
        );
        match status.state {
            daemon::InstanceState::Serving => {}
            daemon::InstanceState::Starting => println!("{}", text::starting(current())),
            // install 广播 prepare_swap 之后：滚动重启中该实例随时会被停止、
            // 用新 exe 重拉——外部不要在此窗口 stop/kill
            daemon::InstanceState::SwapPrepared => {
                println!("{}", text::swapping(current()));
            }
            daemon::InstanceState::Stopping => println!("{}", text::stopping(current())),
            daemon::InstanceState::Unknown => {
                println!("{}", text::unknown(current()));
            }
        }
        // 混版本检测：CLI 与实例版本不一致说明该实例还在跑旧二进制（替换 exe
        // 后重启该实例即升级）——滚动升级的事实源。推荐 restart 而非 stop+start：
        // stop&&start 的无参 start 走 default_config，别名启动的实例会被指到
        // 错误配置；restart 按期望状态记录里的拉起规格拉起，不会指错。
        if info.version != cli_version {
            println!(
                "{}",
                text::version(
                    current(),
                    &info.version,
                    cli_version,
                    daemon::port_of(&info.listen_addr)
                )
            );
        }
    }
}

/// 列出「进程仍在、却不应答控制通道」的实例：优雅停止走不通，只能 --force
/// （终止前核验 pid + 创建时间，不会误杀）。开着看门狗时，挂死检测会自行处决
/// 并重拉它们。
fn print_unresponsive(unresponsive: &[&daemon::InstanceRecord]) {
    if unresponsive.is_empty() {
        return;
    }
    println!("{}", text::unresponsive(current(), unresponsive.len()));
    for info in unresponsive {
        let port = daemon::port_of(&info.listen_addr);
        println!(
            "{}",
            text::unresponsive_row(
                current(),
                port,
                info.pid,
                &info.version,
                config_label(&info.config_path)
            )
        );
        println!(
            "{}",
            text::launch_line(current(), &info.launch_reason, &info.launch_note)
        );
    }
}

/// 展示用的配置文件路径；没有配置文件的实例显示「（没有配置文件）」。
fn config_label(config_path: &Option<String>) -> &str {
    config_path.as_deref().unwrap_or(text::no_config(current()))
}
