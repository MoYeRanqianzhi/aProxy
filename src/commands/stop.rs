//! `aproxy stop`，以及它与 `aproxy restart` 共用的目标选择与停止结果的渲染。
//!
//! 两个命令共享完全相同的 target 语义（省略/PORT/all/ALIAS/idle [SECS]），
//! 差异仅在「停止后」：stop 到此为止（并删掉期望状态记录），restart 按原来的拉起规格立即拉起。
//! `--force` 改变停止方式：跳过 IPC 优雅关闭直接终止进程（零等待），终止前
//! 按「pid + 进程创建时间」核验身份防 pid 复用误杀（与二进制名无关）。
//! 选目标与停止本身在操作层（`aproxy::ops::target`、`aproxy::ops::stop`）。
//!
//! 结果按实例逐项记下（见 [`super::Results`]）：stop 的结局是 `stopped`、`already_stopped`、
//! `killed`、`skipped`、`failed` 之一，退出码与 `--json` 的文档由这份清单汇总得出。选不出
//! 目标时整条命令失败：目标不存在是 `target_not_found`（4），多个实例在运行而没指定目标是
//! `target_ambiguous`（2）。

use aproxy::daemon::{self, InstanceRecord};
use aproxy::ops::stop::{
    IdleStop, StopFailure, StopMode, Stopped, forget_all_stopped, forget_stopped_config,
    forget_stopped_port, stop_if_idle, stop_instance,
};
use aproxy::ops::target::{self, Empty, SelectError, Selection};
use serde_json::json;

use super::{Failure, Results, port_value, say};

/// 别名、端口、配置文件都对不上：`target_not_found`（4）。
pub(crate) fn unknown_target(target: &str) -> Failure {
    Failure::new(
        "target_not_found",
        format!(
            "未知的别名或端口号: {target}\n用 aproxy alias list 查看已有别名，或 aproxy alias add {target} <路径> 添加"
        ),
        json!({"target": target}),
    )
}

/// 没指定目标，而运行中的实例不止一个：`target_ambiguous`（2）。`cmd` 是命令名，用来在
/// 说明里给出可以照抄的命令行。
pub(crate) fn ambiguous(cmd: &str, ports: &[String]) -> Failure {
    let mut message = format!(
        "有 {} 个实例在运行，必须指定端口号、别名或 all：",
        ports.len()
    );
    for port in ports {
        message.push_str(&format!("\n  aproxy {cmd} {port}"));
    }
    let ports: Vec<_> = ports.iter().map(|port| port_value(port)).collect();
    Failure::new("target_ambiguous", message, json!({"ports": ports}))
}

/// 实例的进程还在，却不应答控制通道：没带 `--force` 时处理不了它，`instance_unresponsive`（1）。
/// `alias` 是用户写的别名（按端口指定、或随 all 一起选中时为 None）。
pub(crate) fn unresponsive(alias: Option<&str>, port: &str, pid: u32) -> Failure {
    let who = match alias {
        Some(alias) => format!("别名 {alias} 的实例（端口 {port}，pid {pid}）"),
        None => format!("端口 {port} 的实例（pid {pid}）"),
    };
    Failure::new(
        "instance_unresponsive",
        format!("{who}进程仍在，但不应答控制通道，无法优雅停止；用 --force 强制结束"),
        json!({"port": port_value(port), "pid": pid}),
    )
}

/// 选中目标时被跳过的不应答实例，逐个记为失败：命令要处理它们而没能处理，不能报成成功。
/// stop 与 restart 共用。
pub(crate) fn fail_unresponsive_skipped(selection: &Selection, results: &mut Results) {
    for info in &selection.unresponsive_skipped {
        let port = daemon::port_of(&info.listen_addr);
        let failure = unresponsive(None, port, info.pid);
        eprintln!("{}", failure.message);
        results.failed(port, "failed", &failure);
    }
}

/// `--idle-threshold-secs` 只对目标 `idle` 有意义；配在别的目标上是用法错误（`usage`，退出码 2），
/// 而不是悄悄不起作用。stop 与 restart 共用，`cmd` 是命令名。
pub(crate) fn require_idle_for_threshold(
    cmd: &str,
    target: Option<&str>,
    threshold: Option<u64>,
    json: bool,
) {
    if threshold.is_some() && !target.is_some_and(|t| t.eq_ignore_ascii_case("idle")) {
        Failure::new(
            "usage",
            format!(
                "--idle-threshold-secs 只能与目标 idle 一起用，例如: aproxy {cmd} idle --idle-threshold-secs 600"
            ),
            json!({"reason": "idle_threshold_without_idle"}),
        )
        .exit(json)
    }
}

/// `aproxy stop` 入口。
///
/// stop 的含义是「这个实例不该在运行」，所以目标已经不在运行时，这个状态已经达成（结局
/// `already_stopped`，退出码 0）；它若还留着期望状态记录（崩溃之后还没被拉回来），删掉记录
/// 就是这次 stop 的全部工作：restore 与看护者不再拉它。
pub(crate) async fn handle_stop_cmd(
    target: Option<String>,
    threshold: Option<u64>,
    force: bool,
    json: bool,
) {
    require_idle_for_threshold("stop", target.as_deref(), threshold, json);
    let mode = StopMode::from_force(force);
    let mut results = Results::new(target.as_deref());
    let selection = match target::select(target.as_deref(), threshold, force).await {
        Ok(selection) => selection,
        Err(SelectError::UnknownTarget { target }) => unknown_target(&target).exit(json),
        Err(SelectError::Ambiguous { ports }) => ambiguous("stop", &ports).exit(json),
        Err(SelectError::NothingOnPort { port }) => {
            if forget_stopped_port(&port).await {
                say(
                    json,
                    format!(
                        "端口 {port} 的实例已不在运行；已删除它的期望状态记录，restore 与看护者不会再拉起它。"
                    ),
                );
            } else {
                say(
                    json,
                    format!(
                        "端口 {port} 上没有运行中的 aProxy 实例。\n（若该端口被其他程序占用，与本工具无关）"
                    ),
                );
            }
            results.ok(&port, "already_stopped", None);
            results.finish(json)
        }
        Err(SelectError::NotRunning {
            target,
            config_path,
        }) => {
            let ports = forget_stopped_config(&config_path).await;
            if ports.is_empty() {
                say(
                    json,
                    format!(
                        "别名 {target}（配置 {}）当前未在运行。",
                        config_path.display()
                    ),
                );
                // 这一项说的不是某个端口：别名的实例没在运行，也没留下记录
                results.ok("", "already_stopped", None);
            } else {
                say(
                    json,
                    format!(
                        "配置 {} 的实例已不在运行；已删除它的期望状态记录（端口 {}），restore 与看护者不会再拉起它。",
                        config_path.display(),
                        ports.join("、")
                    ),
                );
                for port in &ports {
                    results.ok(port, "already_stopped", None);
                }
            }
            results.finish(json)
        }
        Err(SelectError::Unresponsive { target, port, pid }) => {
            let failure = unresponsive(target.as_deref(), &port, pid);
            eprintln!("{}", failure.message);
            results.failed(&port, "failed", &failure);
            results.finish(json)
        }
    };
    fail_unresponsive_skipped(&selection, &mut results);
    match selection.empty {
        Some(Empty::NoneRunning) if selection.unresponsive_skipped.is_empty() => {
            say(json, "没有运行中的 aProxy 实例。")
        }
        Some(Empty::NoneIdle { idle_secs }) => {
            say(json, format!("没有闲置超过 {idle_secs} 秒的实例。"))
        }
        _ => {}
    }
    // stop idle 的优雅停止带着闲置条件：被选中之后又收到请求的实例不停（强制终止没有条件
    // 可带，照旧按选中时的闲置时长）
    let idle_secs = (mode == StopMode::Graceful
        && target
            .as_deref()
            .is_some_and(|t| t.eq_ignore_ascii_case("idle")))
    .then(|| threshold.unwrap_or(aproxy::settings::load().idle_timeout_secs));
    for info in &selection.instances {
        let port = daemon::port_of(&info.listen_addr);
        let outcome = match idle_secs {
            Some(secs) => stop_if_idle(info, secs).await,
            None => stop_instance(info, mode).await.map(IdleStop::Stopped),
        };
        match outcome {
            Ok(IdleStop::Stopped(stopped)) => {
                say(json, stopped_line(info, stopped));
                results.ok(port, stopped_outcome(stopped), None);
            }
            // 选中之后又收到了请求：不再闲置，跳过它是 idle 目标要的结果，计为成功
            Ok(IdleStop::Busy { idle_secs }) => {
                say(
                    json,
                    format!(
                        "pid {}（端口 {port}）刚才又收到了请求（已闲置 {idle_secs} 秒），不再算闲置，未停止",
                        info.pid
                    ),
                );
                results.ok(port, "skipped", Some("not_idle"));
            }
            Err(failure) => {
                let failure = stop_failure(info, &failure);
                eprintln!("{}", failure.message);
                results.failed(port, "failed", &failure);
            }
        }
    }
    // stop all：连同已经不在运行的实例留下的记录一起清掉，之后 restore 没有可拉的东西
    if target.as_deref() == Some("all") {
        let (ports, unreadable) = forget_all_stopped().await;
        if !ports.is_empty() {
            say(
                json,
                format!(
                    "已删除不在运行的实例留下的期望状态记录（端口 {}）。",
                    ports.join("、")
                ),
            );
        }
        for port in &ports {
            results.ok(port, "already_stopped", None);
        }
        for record in &unreadable {
            eprintln!(
                "保留了读不懂的记录 {}（{}）：多半由更新版本的 aproxy 写下，请用那个版本处理。",
                record.path.display(),
                record.reason
            );
        }
    }
    results.finish(json)
}

/// 一个实例停下之后的结局：优雅停下是 `stopped`，核验时已不在是 `already_stopped`，强制
/// 终止是 `killed`。
fn stopped_outcome(stopped: Stopped) -> &'static str {
    match stopped {
        Stopped::Graceful => "stopped",
        Stopped::AlreadyGone => "already_stopped",
        Stopped::Forced => "killed",
    }
}

/// 一个实例停下之后给人看的一行。
pub(crate) fn stopped_line(info: &InstanceRecord, stopped: Stopped) -> String {
    let port = daemon::port_of(&info.listen_addr);
    match stopped {
        Stopped::Graceful => format!("已停止 pid {}（端口 {port}）", info.pid),
        Stopped::AlreadyGone => format!("pid {}（端口 {port}）已不在运行", info.pid),
        Stopped::Forced => format!("已强制终止 pid {}（端口 {port}）", info.pid),
    }
}

/// 一个实例没能确认停下：请求发不出去是 `instance_unresponsive`，其余是 `stop_failed`，
/// `details.reason` 是 `still_running`（请求已送达、时限内没退出）或 `kill_failed`（强制
/// 终止失败）。
pub(crate) fn stop_failure(info: &InstanceRecord, failure: &StopFailure) -> Failure {
    let port = daemon::port_of(&info.listen_addr);
    // 强制结束指向 `stop --force`：跨平台，且终止前按 pid + 创建时间核验身份，不会像手敲
    // taskkill/kill 那样误杀复用了该 pid 的其他进程
    match failure {
        StopFailure::GenerationChanged => Failure::new(
            "command_failed",
            format!(
                "The selected instance on port {port} was replaced; the current instance was not stopped."
            ),
            json!({"port":port_value(port),"reason":"generation_changed"}),
        ),
        StopFailure::StillRunning => Failure::new(
            "stop_failed",
            format!(
                "pid {} 已收到停止请求但尚未退出，可用 aproxy status 稍后确认，或 aproxy stop {port} --force 强制结束",
                info.pid
            ),
            json!({"port": port_value(port), "reason": "still_running"}),
        ),
        StopFailure::Unresponsive { error } => Failure::new(
            "instance_unresponsive",
            format!(
                "pid {} 不应答停止请求（{error}），可用 aproxy stop {port} --force 强制结束",
                info.pid
            ),
            json!({"port": port_value(port), "pid": info.pid}),
        ),
        StopFailure::ForceFailed {
            error,
            restore_error,
        } => {
            let mut message = String::new();
            if let Some(werr) = restore_error {
                message.push_str(&format!("放回端口 {port} 的恢复记录失败: {werr}\n"));
            }
            message.push_str(&format!("强制终止 pid {} 失败: {error}", info.pid));
            Failure::new(
                "stop_failed",
                message,
                json!({"port": port_value(port), "reason": "kill_failed"}),
            )
        }
    }
}
