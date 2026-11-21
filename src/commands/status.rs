//! `aproxy status [--idle|--busy] [--json]`：只读清点 home 里的实例、记录与其他进程。

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
        let which = if idle_only { "闲置" } else { "活跃" };
        println!("没有{which}的 aProxy 实例（阈值 {idle_threshold_secs} 秒）。");
    }

    if !incompatible.is_empty() {
        println!(
            "无法采信应答的 aProxy 实例 ({})——协议版本不同或身份对不上，用与实例同版本的 aproxy 操作它:",
            incompatible.len()
        );
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
            println!("  端口 {port}  pid {pid}  v{version}  原因 {code}");
        }
    }
    if !down.is_empty() {
        println!(
            "等待恢复的实例 ({})——没在运行，留着期望状态记录，aproxy restore 或看护者会拉起它:",
            down.len()
        );
        for entry in &down {
            let port = entry.port;
            if entry.restore_reason.as_deref() == Some(RESTORE_REASON_CONFIG_MISSING) {
                println!("  端口 {port}（配置文件找不到）");
            } else {
                println!("  端口 {port}");
            }
            println!("    不再需要它时执行 aproxy stop {port}");
        }
    }
    if !had_live && unresponsive.is_empty() && incompatible.is_empty() && down.is_empty() {
        println!("没有运行中的 aProxy 实例。");
    }

    if !filtered_enumeration.unreadable.is_empty() {
        println!(
            "读不懂的记录 ({})，原样保留（多半由更新版本的 aproxy 写下，请用那个版本处理）:",
            filtered_enumeration.unreadable.len()
        );
        for entry in &filtered_enumeration.unreadable {
            println!("  {}  [{}]", entry.file, entry.reason.code);
        }
    }
    if filtered_enumeration.gc_paused.is_some() {
        println!("日志清理已暂停：run/ 里有读不懂的记录。");
    }
    if let Some(w) = &filtered_enumeration.watchdog {
        let since_scan = humanize_duration(now.saturating_sub(w.last_scan_at) / 1000);
        if w.state == WATCHDOG_RUNNING {
            println!(
                "看护者: pid {}  v{}  最近扫描 {since_scan} 前",
                w.pid, w.version
            );
        } else {
            println!(
                "看护者: pid {}  v{}  已 {since_scan} 没有扫描（可能卡住了）",
                w.pid, w.version
            );
        }
    }
    if !filtered_enumeration.processes.is_empty() {
        println!(
            "台账里的其他 aProxy 进程 ({}):",
            filtered_enumeration.processes.len()
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
    println!("运行中的 aProxy 实例 ({}):", live.len());
    let cli_version = env!("CARGO_PKG_VERSION");
    for entry in live {
        // 清点只在控制通道给出可采信的应答时标为 Live，此时两份数据都在。
        let info = entry.instance.as_ref().unwrap();
        let status = entry.status.as_ref().unwrap();
        let activity = &status.activity;
        let idle_secs = activity.idle_secs_at(now);
        println!(
            "  端口 {}  pid {}  v{}  已运行 {}  闲置 {}",
            daemon::port_of(&info.listen_addr),
            info.pid,
            info.version,
            humanize_duration(now.saturating_sub(info.started_at) / 1000),
            humanize_duration(idle_secs)
        );
        println!(
            "    监听 http://{}   上游 {}   配置 {}",
            info.listen_addr,
            aproxy_ipc::redact::url(&info.base_url),
            config_label(&info.config_path)
        );
        let err_text = match &activity.last_error {
            // 码在前，便于按码检索；没有码的（更旧的实例）只显示说明
            Some(e) => format!(
                "\"{}{}\"（{} 前）",
                if e.code.is_empty() {
                    String::new()
                } else {
                    format!("{}: ", e.code)
                },
                e.message,
                humanize_duration(now.saturating_sub(e.at) / 1000)
            ),
            None => "无".to_string(),
        };
        println!(
            "    请求 {}  在途 {}  重试 {}  救回 {}  最近错误: {}",
            activity.requests_total,
            activity.in_flight,
            activity.retries_total,
            activity.rescued_total,
            err_text
        );
        match status.state {
            daemon::InstanceState::Serving => {}
            daemon::InstanceState::Starting => println!("    正在启动（还没开始接收请求）"),
            // install 广播 prepare_swap 之后：滚动重启中该实例随时会被停止、
            // 用新 exe 重拉——外部不要在此窗口 stop/kill
            daemon::InstanceState::SwapPrepared => {
                println!("    二进制更换中（install 滚动重启阶段，请勿手动干预此实例）");
            }
            daemon::InstanceState::Stopping => println!("    正在退出"),
            daemon::InstanceState::Unknown => {
                println!("    状态未知（实例比当前 CLI 新，用同版本的 aproxy 查看）");
            }
        }
        // 混版本检测：CLI 与实例版本不一致说明该实例还在跑旧二进制（替换 exe
        // 后重启该实例即升级）——滚动升级的事实源。推荐 restart 而非 stop+start：
        // stop&&start 的无参 start 走 default_config，别名启动的实例会被指到
        // 错误配置；restart 按期望状态记录里的拉起规格拉起，不会指错。
        if info.version != cli_version {
            println!(
                "    注意: 实例版本 v{} 与当前 CLI v{cli_version} 不同，替换二进制后执行 aproxy restart {} 可完成升级",
                info.version,
                daemon::port_of(&info.listen_addr)
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
    println!(
        "无响应的 aProxy 实例 ({})——进程仍在，但不应答控制通道:",
        unresponsive.len()
    );
    for info in unresponsive {
        let port = daemon::port_of(&info.listen_addr);
        println!(
            "  端口 {port}  pid {}  v{}  配置 {}\n    可用 aproxy stop {port} --force 结束（或 aproxy restart {port} --force 重启）",
            info.pid,
            info.version,
            config_label(&info.config_path)
        );
    }
}

/// 展示用的配置文件路径；没有配置文件的实例显示「（没有配置文件）」。
fn config_label(config_path: &Option<String>) -> &str {
    config_path.as_deref().unwrap_or("（没有配置文件）")
}
