//! `aproxy stop`，以及它与 `aproxy restart` 共用的目标选择与停止结果的渲染。
//!
//! 两个命令共享完全相同的 target 语义（省略/PORT/all/ALIAS/idle [SECS]），
//! 差异仅在「停止后」：stop 到此为止（并删掉期望状态记录），restart 按原来的拉起规格立即拉起。
//! `--force` 改变停止方式：跳过 IPC 优雅关闭直接终止进程（零等待），终止前
//! 按「pid + 进程创建时间」核验身份防 pid 复用误杀（与二进制名无关）。
//! 选目标与停止本身在操作层（`aproxy::ops::target`、`aproxy::ops::stop`）。

use aproxy::daemon::{self, InstanceRecord};
use aproxy::ops::stop::{
    IdleStop, StopFailure, StopMode, Stopped, forget_all_stopped, forget_stopped_config,
    forget_stopped_port, stop_if_idle, stop_instance,
};
use aproxy::ops::target::{self, Empty, SelectError};

/// stop/restart 共用：选出要处理的实例。
/// 空 Vec = 该 target 下没有运行中的实例（提示已输出；restart 据此不启动）。
/// 选不出目标（未知别名、实例不应答而没带 `--force` 等）时说明原因并以 1 退出。
pub(crate) async fn select_or_exit(
    target: Option<&str>,
    threshold: Option<u64>,
    force: bool,
) -> Vec<InstanceRecord> {
    let selection = match target::select(target, threshold, force).await {
        Ok(selection) => selection,
        Err(e) => {
            report_select_error(&e);
            std::process::exit(1);
        }
    };
    if selection.unresponsive_skipped > 0 {
        eprintln!(
            "另有 {} 个实例进程仍在但不应答控制通道，优雅停止不涉及它们；aproxy status 可查看，加 --force 一并处理",
            selection.unresponsive_skipped
        );
    }
    match selection.empty {
        Some(Empty::NoneRunning) => println!("没有运行中的 aProxy 实例。"),
        Some(Empty::NoneIdle { idle_secs }) => println!("没有闲置超过 {idle_secs} 秒的实例。"),
        None => {}
    }
    selection.instances
}

/// 「没在运行」类的说明走标准输出（它是对「停什么」的回答），其余走标准错误。
fn report_select_error(e: &SelectError) {
    match e {
        SelectError::UnknownTarget { target } => eprintln!(
            "未知的别名或端口号: {target}\n用 aproxy alias list 查看已有别名，或 aproxy alias add {target} <路径> 添加"
        ),
        SelectError::NotRunning {
            target,
            config_path,
        } => println!(
            "别名 {target}（配置 {}）当前未在运行。",
            config_path.display()
        ),
        SelectError::NothingOnPort { port } => {
            println!("端口 {port} 上没有运行中的 aProxy 实例。");
            println!("（若该端口被其他程序占用，与本工具无关）");
        }
        SelectError::Unresponsive {
            target: Some(target),
            port,
            pid,
        } => eprintln!(
            "别名 {target} 的实例（端口 {port}，pid {pid}）进程仍在，但不应答控制通道，无法优雅停止；用 --force 强制结束"
        ),
        SelectError::Unresponsive {
            target: None,
            port,
            pid,
        } => eprintln!(
            "端口 {port} 的实例（pid {pid}）进程仍在，但不应答控制通道，无法优雅停止；用 --force 强制结束"
        ),
        SelectError::Ambiguous { ports } => {
            eprintln!(
                "有 {} 个实例在运行，必须指定端口号、别名或 all：",
                ports.len()
            );
            for port in ports {
                eprintln!("  aproxy stop {port}");
            }
        }
    }
}

/// `aproxy stop` 入口。
///
/// stop 的含义是「这个实例不该在运行」，所以目标已经不在运行、却还留着期望状态记录时
/// （崩溃之后还没被拉回来），删掉记录就是这次 stop 的全部工作：restore 与看护者不再拉它。
pub(crate) async fn handle_stop_cmd(target: Option<String>, threshold: Option<u64>, force: bool) {
    let mode = StopMode::from_force(force);
    // 已停的目标：按端口或配置文件找到它留下的记录
    match target::select(target.as_deref(), threshold, force).await {
        Err(SelectError::NothingOnPort { port }) if forget_stopped_port(&port).await => {
            println!(
                "端口 {port} 的实例已不在运行；已删除它的期望状态记录，restore 与看护者不会再拉起它。"
            );
            return;
        }
        Err(SelectError::NotRunning { config_path, .. }) => {
            let ports = forget_stopped_config(&config_path).await;
            if !ports.is_empty() {
                println!(
                    "配置 {} 的实例已不在运行；已删除它的期望状态记录（端口 {}），restore 与看护者不会再拉起它。",
                    config_path.display(),
                    ports.join("、")
                );
                return;
            }
        }
        _ => {}
    }
    let targets = select_or_exit(target.as_deref(), threshold, force).await;
    // 任一目标没能确认停止 → 退出 1：脚本据此判断，不能把「请求已发、进程还在」
    // 报成成功
    let mut all_stopped = true;
    // stop idle 的优雅停止带着闲置条件：被选中之后又收到请求的实例不停（强制终止没有条件
    // 可带，照旧按选中时的闲置时长）
    let idle_secs = (mode == StopMode::Graceful
        && target
            .as_deref()
            .is_some_and(|t| t.eq_ignore_ascii_case("idle")))
    .then(|| threshold.unwrap_or(aproxy::settings::load().idle_timeout_secs));
    for info in &targets {
        let outcome = match idle_secs {
            Some(secs) => stop_if_idle(info, secs).await,
            None => stop_instance(info, mode).await.map(IdleStop::Stopped),
        };
        match outcome {
            Ok(IdleStop::Stopped(stopped)) => report_stopped(info, stopped),
            Ok(IdleStop::Busy { idle_secs }) => println!(
                "pid {}（端口 {}）刚才又收到了请求（已闲置 {idle_secs} 秒），不再算闲置，未停止",
                info.pid,
                daemon::port_of(&info.listen_addr)
            ),
            Err(failure) => {
                report_stop_failure(info, &failure);
                all_stopped = false;
            }
        }
    }
    // stop all：连同已经不在运行的实例留下的记录一起清掉，之后 restore 没有可拉的东西
    if target.as_deref() == Some("all") {
        let (ports, unreadable) = forget_all_stopped().await;
        if !ports.is_empty() {
            println!(
                "已删除不在运行的实例留下的期望状态记录（端口 {}）。",
                ports.join("、")
            );
        }
        for record in &unreadable {
            eprintln!(
                "保留了读不懂的记录 {}（{}）：多半由更新版本的 aproxy 写下，请用那个版本处理。",
                record.path.display(),
                record.reason
            );
        }
    }
    if !all_stopped {
        std::process::exit(1);
    }
}

/// 一个实例停下之后的说明。
pub(crate) fn report_stopped(info: &InstanceRecord, stopped: Stopped) {
    let port = daemon::port_of(&info.listen_addr);
    match stopped {
        Stopped::Graceful => println!("已停止 pid {}（端口 {}）", info.pid, port),
        Stopped::AlreadyGone => println!("pid {}（端口 {}）已不在运行", info.pid, port),
        Stopped::Forced => println!("已强制终止 pid {}（端口 {}）", info.pid, port),
    }
}

/// 一个实例没能确认停下时的说明。
pub(crate) fn report_stop_failure(info: &InstanceRecord, failure: &StopFailure) {
    let port = daemon::port_of(&info.listen_addr);
    match failure {
        // 强制结束指向 `stop --force`：跨平台，且终止前按 pid + 创建
        // 时间核验身份，不会像手敲 taskkill/kill 那样误杀复用了该 pid
        // 的其他进程
        StopFailure::StillRunning => println!(
            "pid {} 已收到停止请求但尚未退出，可用 aproxy status 稍后确认，或 aproxy stop {} --force 强制结束",
            info.pid, port
        ),
        StopFailure::Unresponsive { error } => println!(
            "pid {} 不应答停止请求（{error}），可用 aproxy stop {} --force 强制结束",
            info.pid, port
        ),
        StopFailure::ForceFailed {
            error,
            restore_error,
        } => {
            if let Some(werr) = restore_error {
                eprintln!("放回端口 {port} 的恢复记录失败: {werr}");
            }
            eprintln!("强制终止 pid {} 失败: {error}", info.pid);
        }
    }
}
