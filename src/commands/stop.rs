//! `aproxy stop [PORT|all|ALIAS|idle [SECS]]`：停止实例。

use aproxy::daemon;
use aproxy::settings;

use crate::commands::{config_path_key, resolve_config_target};
use crate::util::now_unix;

/// `aproxy stop [PORT|all|ALIAS|idle [SECS]]`：单个实例可省略参数；多实例必须
/// 指定端口号、all、配置别名（按实例注册的 config_path 匹配）或 idle。
pub(crate) async fn handle_stop_cmd(target: Option<String>, threshold: Option<u64>) {
    // idle 保留字：停止全部空闲超阈值的实例（阈值可临时覆盖 settings 配置）
    if target
        .as_deref()
        .is_some_and(|t| t.eq_ignore_ascii_case("idle"))
    {
        let idle_secs = threshold.unwrap_or(settings::load().idle_timeout_secs);
        let instances = daemon::list_instances().await;
        let now = now_unix();
        let idle_targets: Vec<_> = instances
            .iter()
            // last_activity_secs 为 0 = 未上报（旧版本守护），不纳入 idle 停止
            .filter(|i| {
                i.last_activity_secs > 0 && now.saturating_sub(i.last_activity_secs) >= idle_secs
            })
            .collect();
        if idle_targets.is_empty() {
            println!("没有闲置超过 {idle_secs} 秒的实例。");
            return;
        }
        println!("闲置超过 {idle_secs} 秒的实例 ({}):", idle_targets.len());
        for info in &idle_targets {
            stop_instance(info).await;
        }
        return;
    }

    // 别名定位：非数字且非 all 的 target 先查别名表/default——按 config_path 匹配
    // 运行实例（别名不依赖端口号，端口变了别名依然有效）
    if let Some(target) = target.as_deref()
        && target != "all"
        && target.parse::<u16>().is_err()
    {
        match resolve_config_target(target) {
            Some(cfg_path) => {
                let key = config_path_key(&cfg_path.display().to_string());
                let instances = daemon::list_instances().await;
                let found = instances
                    .iter()
                    .find(|i| config_path_key(&i.config_path) == key);
                match found {
                    Some(info) => stop_instance(info).await,
                    None => {
                        println!("别名 {target}（配置 {}）当前未在运行。", cfg_path.display());
                        std::process::exit(1);
                    }
                }
                return;
            }
            None => {
                eprintln!(
                    "未知的别名或端口号: {target}\n用 aproxy alias list 查看已有别名，或 aproxy alias add {target} <路径> 添加"
                );
                std::process::exit(1);
            }
        }
    }

    // 指定端口时不依赖注册表：直接按端口 IPC 定位（注册表丢失也能停）
    if let Some(target) = target.as_deref()
        && target != "all"
    {
        let port = daemon::port_of(target).to_string();
        let info = match daemon::ipc_ping(&port).await {
            Ok(info) => info,
            Err(_) => {
                println!("端口 {port} 上没有运行中的 aProxy 实例。");
                println!("（若该端口被其他程序占用，与本工具无关）");
                std::process::exit(1);
            }
        };
        stop_instance(&info).await;
        return;
    }

    let instances = daemon::list_instances().await;
    match target.as_deref() {
        Some("all") => {
            if instances.is_empty() {
                println!("没有运行中的 aProxy 实例。");
                return;
            }
            for info in &instances {
                stop_instance(info).await;
            }
        }
        _ => match instances.len() {
            0 => println!("没有运行中的 aProxy 实例。"),
            1 => stop_instance(&instances[0]).await,
            n => {
                eprintln!("有 {n} 个实例在运行，必须指定端口号或 all：");
                for info in &instances {
                    eprintln!("  aproxy stop {}", daemon::port_of(&info.listen_addr));
                }
                std::process::exit(1);
            }
        },
    }
}

/// 停止单个实例：发 IPC shutdown，轮询确认退出（宽限 10 秒 + 余量）。
async fn stop_instance(info: &daemon::InstanceInfo) {
    let port = daemon::port_of(&info.listen_addr).to_string();
    match daemon::ipc_request(&port, &daemon::IpcRequest::Shutdown).await {
        Ok(_) => {
            if daemon::wait_until_gone(&port, std::time::Duration::from_secs(12)).await {
                println!("已停止 pid {}（端口 {}）", info.pid, port);
            } else {
                println!(
                    "pid {} 已收到停止请求但尚未退出，可用 aproxy status 稍后确认，或 taskkill /PID {} /F 强制结束",
                    info.pid, info.pid
                );
            }
        }
        Err(e) => println!("pid {} 无响应（可能已停止）: {}", info.pid, e),
    }
}
