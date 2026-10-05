//! `aproxy status [--idle|--busy]`：列出运行中的实例。

use aproxy::config::mask_base_url;
use aproxy::daemon;
use aproxy::settings;

use crate::util::{humanize_duration, humanize_uptime, now_unix};

/// `aproxy status [--idle|--busy]`：列出注册表中的实例（存活以 IPC 探测为准）。
/// idle/busy 按实例的最近活动时间与 settings.idle_timeout_secs 判定。
pub(crate) async fn handle_status_cmd(idle_only: bool, busy_only: bool) {
    let threshold = settings::load().idle_timeout_secs;
    let survey = daemon::survey_instances().await;
    // 不应答的实例无论 --idle/--busy 都要报：它们多半挂死了，用户最需要知道
    print_unresponsive(&survey.unresponsive);
    let instances = survey.responsive;
    if instances.is_empty() {
        if survey.unresponsive.is_empty() {
            println!("没有运行中的 aProxy 实例。");
        }
        return;
    }
    let now = now_unix();
    let is_idle =
        |info: &daemon::InstanceInfo| now.saturating_sub(info.last_activity_secs) >= threshold;
    let filtered: Vec<_> = instances
        .into_iter()
        .filter(|info| !idle_only || is_idle(info))
        .filter(|info| !busy_only || !is_idle(info))
        .collect();
    if filtered.is_empty() {
        let which = if idle_only {
            "闲置"
        } else if busy_only {
            "活跃"
        } else {
            unreachable!("idle 与 busy 互斥，二者必有一为假");
        };
        println!("没有{which}的 aProxy 实例（阈值 {} 秒）。", threshold);
        return;
    }
    println!("运行中的 aProxy 实例 ({}):", filtered.len());
    let cli_version = env!("CARGO_PKG_VERSION");
    for info in &filtered {
        let idle_secs = now.saturating_sub(info.last_activity_secs);
        println!(
            "  端口 {}  pid {}  v{}  已运行 {}  闲置 {}",
            daemon::port_of(&info.listen_addr),
            info.pid,
            info.version,
            humanize_uptime(info.started_at),
            humanize_duration(idle_secs)
        );
        println!(
            "    监听 http://{}   上游 {}   配置 {}",
            info.listen_addr,
            mask_base_url(&info.base_url),
            info.config_path
        );
        // 观测计数（IPC v2 起 ping 携带；v1 实例读出全 0——展示为「—」而非
        // 误导性的 0）
        if info.proto_version >= 2 {
            let err_text = match (&info.last_error, info.last_error_at) {
                (Some(msg), at) if at > 0 => {
                    format!(
                        "\"{}\"（{} 前）",
                        msg,
                        humanize_duration(now.saturating_sub(at))
                    )
                }
                (Some(msg), _) => format!("\"{msg}\""),
                (None, _) => "无".to_string(),
            };
            println!(
                "    请求 {}  重试 {}  最近错误: {}",
                info.requests_total, info.retries_total, err_text
            );
        }
        // 二进制更换阶段（install 的 PrepareSwap 广播后置位，IPC v3 观测）：
        // 滚动重启中该实例随时会被 stop+新 exe 重拉——外部不要在此窗口 stop/kill
        if info.swap_phase {
            println!("    二进制更换中（install 滚动重启阶段，请勿手动干预此实例）");
        }
        // 混版本检测：CLI 与实例版本不一致说明该实例还在跑旧二进制（替换 exe
        // 后重启该实例即升级）——滚动升级的事实源。推荐 restart 而非 stop+start：
        // stop&&start 的无参 start 走 default_config，别名启动的实例会被指到
        // 错误配置；restart 按 .restore 原参数拉起，不会指错。
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
fn print_unresponsive(unresponsive: &[daemon::InstanceInfo]) {
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
            info.pid, info.version, info.config_path
        );
    }
}
