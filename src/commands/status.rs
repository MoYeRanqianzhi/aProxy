//! `aproxy status [--idle|--busy]`：列出运行中的实例。

use aproxy::daemon;
use aproxy::ops::list::{ActivityFilter, enumerate};

use crate::util::humanize_duration;

/// `aproxy status [--idle|--busy]`：列出注册表中的实例（存活以 IPC 探测为准）。
/// idle/busy 按实例的最近活动时间与 settings.idle_timeout_secs 判定。
pub(crate) async fn handle_status_cmd(idle_only: bool, busy_only: bool) {
    let filter = if idle_only {
        ActivityFilter::Idle
    } else if busy_only {
        ActivityFilter::Busy
    } else {
        ActivityFilter::All
    };
    let listing = enumerate(filter).await;
    // 不应答的实例无论 --idle/--busy 都要报：它们多半挂死了，用户最需要知道
    print_unresponsive(&listing.unresponsive);
    if listing.responsive_total == 0 {
        if listing.unresponsive.is_empty() {
            println!("没有运行中的 aProxy 实例。");
        }
        return;
    }
    let now = listing.now;
    let filtered = &listing.instances;
    if filtered.is_empty() {
        let which = match filter {
            ActivityFilter::Idle => "闲置",
            ActivityFilter::Busy => "活跃",
            ActivityFilter::All => unreachable!("不筛选时，有应答的实例就不会一个都不剩"),
        };
        println!(
            "没有{which}的 aProxy 实例（阈值 {} 秒）。",
            listing.idle_threshold_secs
        );
        return;
    }
    println!("运行中的 aProxy 实例 ({}):", filtered.len());
    let cli_version = env!("CARGO_PKG_VERSION");
    for status in filtered {
        let info = &status.instance;
        let activity = &status.activity;
        let idle_secs = listing.idle_secs(status);
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
fn print_unresponsive(unresponsive: &[daemon::InstanceRecord]) {
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
