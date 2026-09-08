//! `aproxy stop` / `aproxy restart` 共用的实例定位与停止逻辑。
//!
//! 两个命令共享完全相同的 target 语义（省略/PORT/all/ALIAS/idle [SECS]），
//! 差异仅在「停止后」：stop 到此为止，restart 用原启动参数立即拉起。
//! `--force` 改变停止方式：跳过 IPC 优雅关闭直接 TerminateProcess（零等待）。

use aproxy::daemon;
use aproxy::settings;

use crate::commands::{config_path_key, resolve_config_target};
use crate::util::now_unix;

/// 停止方式
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum StopMode {
    /// IPC shutdown + 轮询确认退出（优雅，在途请求 10s 宽限强退）
    Graceful,
    /// TerminateProcess 立即终止（零等待；镜像名验证防 PID 复用误杀）
    Force,
}

/// stop/restart 的共享 target 解析：枚举出要停止的实例清单。
/// 空 Vec = 该 target 下没有运行中的实例（提示已输出；restart 据此不启动）。
/// 错误（未知别名等）直接打印并 exit 1。
pub(crate) async fn resolve_stop_targets(
    target: Option<String>,
    threshold: Option<u64>,
) -> Vec<daemon::InstanceInfo> {
    // idle 保留字：停止全部空闲超阈值的实例（阈值可临时覆盖 settings 配置）
    if target
        .as_deref()
        .is_some_and(|t| t.eq_ignore_ascii_case("idle"))
    {
        let idle_secs = threshold.unwrap_or(settings::load().idle_timeout_secs);
        let instances = daemon::list_instances().await;
        let now = now_unix();
        let idle_targets: Vec<_> = instances
            .into_iter()
            // last_activity_secs 为 0 = 未上报（旧版本守护），不纳入 idle 停止
            .filter(|i| {
                i.last_activity_secs > 0 && now.saturating_sub(i.last_activity_secs) >= idle_secs
            })
            .collect();
        if idle_targets.is_empty() {
            println!("没有闲置超过 {idle_secs} 秒的实例。");
        }
        return idle_targets;
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
                match instances
                    .iter()
                    .find(|i| config_path_key(&i.config_path) == key)
                {
                    Some(info) => return vec![info.clone()],
                    None => {
                        println!("别名 {target}（配置 {}）当前未在运行。", cfg_path.display());
                        std::process::exit(1);
                    }
                }
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
        return match daemon::ipc_ping(&port).await {
            Ok(info) => vec![info],
            Err(_) => {
                println!("端口 {port} 上没有运行中的 aProxy 实例。");
                println!("（若该端口被其他程序占用，与本工具无关）");
                std::process::exit(1);
            }
        };
    }

    let instances = daemon::list_instances().await;
    match target.as_deref() {
        Some("all") => {
            if instances.is_empty() {
                println!("没有运行中的 aProxy 实例。");
            }
            instances
        }
        _ => match instances.len() {
            0 => {
                println!("没有运行中的 aProxy 实例。");
                Vec::new()
            }
            1 => vec![instances[0].clone()],
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

/// `aproxy stop` 入口（restart 复用同一套 resolve/stop_instance）。
pub(crate) async fn handle_stop_cmd(target: Option<String>, threshold: Option<u64>, force: bool) {
    let mode = if force {
        StopMode::Force
    } else {
        StopMode::Graceful
    };
    let targets = resolve_stop_targets(target, threshold).await;
    for info in &targets {
        stop_instance(info, mode).await;
    }
}

/// 停止单个实例。
/// Graceful：发 IPC shutdown，轮询确认退出（宽限 10 秒 + 余量）。
/// Force：立即 TerminateProcess（镜像名验证后），不等任何确认——进程对象
/// 的销毁是异步的，但信号已发，调用方（restart）由 IPC ping 就绪判定兜底。
pub(crate) async fn stop_instance(info: &daemon::InstanceInfo, mode: StopMode) -> bool {
    let port = daemon::port_of(&info.listen_addr).to_string();
    match mode {
        StopMode::Graceful => match daemon::ipc_request(&port, &daemon::IpcRequest::Shutdown).await
        {
            Ok(_) => {
                if daemon::wait_until_gone(&port, std::time::Duration::from_secs(12)).await {
                    println!("已停止 pid {}（端口 {}）", info.pid, port);
                    true
                } else {
                    println!(
                        "pid {} 已收到停止请求但尚未退出，可用 aproxy status 稍后确认，或 taskkill /PID {} /F 强制结束",
                        info.pid, info.pid
                    );
                    false
                }
            }
            Err(e) => {
                println!("pid {} 无响应（可能已停止）: {}", info.pid, e);
                false
            }
        },
        StopMode::Force => match daemon::force_terminate(info.pid) {
            Ok(()) => {
                println!("已强制终止 pid {}（端口 {}）", info.pid, port);
                true
            }
            Err(e) => {
                eprintln!("强制终止 pid {} 失败: {e}", info.pid);
                false
            }
        },
    }
}
