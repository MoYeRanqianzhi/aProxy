//! `aproxy stop` / `aproxy restart` 共用的实例定位与停止逻辑。
//!
//! 两个命令共享完全相同的 target 语义（省略/PORT/all/ALIAS/idle [SECS]），
//! 差异仅在「停止后」：stop 到此为止，restart 用原启动参数立即拉起。
//! `--force` 改变停止方式：跳过 IPC 优雅关闭直接终止进程（零等待），终止前
//! 按「pid + 进程创建时间」核验身份防 pid 复用误杀（与二进制名无关）。

use aproxy::daemon;
use aproxy::settings;

use crate::commands::{config_path_key, resolve_config_target};
use crate::util::now_unix;

/// 停止方式
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum StopMode {
    /// IPC shutdown + 轮询确认退出（优雅，在途请求 10s 宽限强退）
    Graceful,
    /// 立即终止进程（零等待；pid + 创建时间核验防 PID 复用误杀）
    Force,
}

/// stop/restart 的共享 target 解析：枚举出要停止的实例清单。
/// 空 Vec = 该 target 下没有运行中的实例（提示已输出；restart 据此不启动）。
/// 错误（未知别名等）直接打印并 exit 1。
///
/// `force` 时把「进程仍在、却不应答控制通道」的实例也纳入（按端口指定时回退
/// 注册表记录；`all` 时并入普查结果）：它们正是只能强制终止的那一类，而
/// force_terminate 终止前按 pid + 创建时间核验身份，不依赖 IPC。
pub(crate) async fn resolve_stop_targets(
    target: Option<String>,
    threshold: Option<u64>,
    force: bool,
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
                let survey = daemon::survey_instances().await;
                let matches = |i: &&daemon::InstanceInfo| config_path_key(&i.config_path) == key;
                if let Some(info) = survey.responsive.iter().find(matches) {
                    return vec![info.clone()];
                }
                match survey.unresponsive.iter().find(matches) {
                    Some(info) if force => return vec![info.clone()],
                    Some(info) => {
                        eprintln!(
                            "别名 {target} 的实例（端口 {}，pid {}）进程仍在，但不应答控制通道，无法优雅停止；用 --force 强制结束",
                            daemon::port_of(&info.listen_addr),
                            info.pid
                        );
                        std::process::exit(1);
                    }
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
            Err(_) => match unresponsive_record(&port) {
                Some(info) if force => vec![info],
                Some(info) => {
                    eprintln!(
                        "端口 {port} 的实例（pid {}）进程仍在，但不应答控制通道，无法优雅停止；用 --force 强制结束",
                        info.pid
                    );
                    std::process::exit(1);
                }
                None => {
                    println!("端口 {port} 上没有运行中的 aProxy 实例。");
                    println!("（若该端口被其他程序占用，与本工具无关）");
                    std::process::exit(1);
                }
            },
        };
    }

    let survey = daemon::survey_instances().await;
    let mut instances = survey.responsive;
    if force {
        instances.extend(survey.unresponsive);
    } else if !survey.unresponsive.is_empty() {
        eprintln!(
            "另有 {} 个实例进程仍在但不应答控制通道，优雅停止不涉及它们；aproxy status 可查看，加 --force 一并处理",
            survey.unresponsive.len()
        );
    }
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
                eprintln!("有 {n} 个实例在运行，必须指定端口号、别名或 all：");
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
    let targets = resolve_stop_targets(target, threshold, force).await;
    // 任一目标没能确认停止 → 退出 1：脚本据此判断，不能把「请求已发、进程还在」
    // 报成成功
    let mut all_stopped = true;
    for info in &targets {
        all_stopped &= stop_instance(info, mode).await;
    }
    if !all_stopped {
        std::process::exit(1);
    }
}

/// 按端口读注册表记录，且记录的进程经「pid + 创建时间」核验仍在——不应答
/// 控制通道的实例（多半挂死）只能这样定位。
fn unresponsive_record(port: &str) -> Option<daemon::InstanceInfo> {
    let info = daemon::read_instance_file_in(&daemon::run_dir(), port)?;
    matches!(
        aproxy::watchdog::record_identity(info.pid, info.process_start),
        aproxy::watchdog::RecordIdentity::Alive(_)
    )
    .then_some(info)
}

/// 停止单个实例。
/// Graceful：发 IPC shutdown，轮询确认退出（宽限 10 秒 + 余量）。
/// Force：立即终止（pid + 创建时间核验后，见 daemon::force_terminate），不等
/// 任何确认——进程对象的销毁是异步的，但信号已发，调用方（restart）由新实例
/// 的就绪判定兜底。
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
                    // 强制结束指向 `stop --force`：跨平台，且终止前按 pid + 创建
                    // 时间核验身份，不会像手敲 taskkill/kill 那样误杀复用了该 pid
                    // 的其他进程
                    println!(
                        "pid {} 已收到停止请求但尚未退出，可用 aproxy status 稍后确认，或 aproxy stop {} --force 强制结束",
                        info.pid, port
                    );
                    false
                }
            }
            // 发不出 shutdown：区分「其间已自行退出」与「进程还在、只是不应答」，
            // 前者就是停止的结果，后者只能 --force
            Err(e) => match aproxy::watchdog::record_identity(info.pid, info.process_start) {
                aproxy::watchdog::RecordIdentity::Gone
                | aproxy::watchdog::RecordIdentity::Reused => {
                    println!("pid {}（端口 {}）已不在运行", info.pid, port);
                    true
                }
                _ => {
                    println!(
                        "pid {} 不应答停止请求（{e}），可用 aproxy stop {} --force 强制结束",
                        info.pid, port
                    );
                    false
                }
            },
        },
        // 被强杀的守护来不及做优雅退出的自清，.restore 留着就是「崩溃」信号：
        // 看门狗收到死亡事件即刻按它重拉，aproxy restore 也会复活它，stop
        // 语义落空。所以 .restore 必须在终止**之前**摘掉——看门狗被死亡事件
        // 即时唤醒，终止之后再删必然与它赛跑。终止失败（身份核验不过、无权
        // 限）时原样放回，实例照旧受崩溃恢复保护。restart --force 同样摘掉：
        // 否则看门狗与 restart 会同时拉起新实例争抢端口；新实例起不来时与
        // 优雅重启的失败同态（无恢复记录），由 restart 的失败提示给出恢复命令。
        StopMode::Force => {
            let saved = daemon::list_restore_entries()
                .into_iter()
                .find(|e| e.port == port);
            daemon::remove_restore_file(&info.listen_addr);
            match daemon::force_terminate(info).await {
                Ok(()) => {
                    // 其余残留按守护自清的顺序收掉（.pid、socket、心跳）
                    crate::server::remove_registry_files(&info.listen_addr);
                    println!("已强制终止 pid {}（端口 {}）", info.pid, port);
                    true
                }
                Err(e) => {
                    if let Some(entry) = saved
                        && let Err(werr) = daemon::write_restore_file(
                            &info.listen_addr,
                            &entry.args,
                            &entry.log_path,
                        )
                    {
                        eprintln!("放回端口 {port} 的恢复记录失败: {werr}");
                    }
                    eprintln!("强制终止 pid {} 失败: {e}", info.pid);
                    false
                }
            }
        }
    }
}
