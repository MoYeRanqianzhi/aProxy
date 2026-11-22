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

#[path = "../i18n/stop.rs"]
pub(super) mod text;
use aproxy_ipc::i18n::current;

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
    Failure::rendered(
        "target_not_found",
        |lang| text::unknown(lang, target),
        json!({"target": target}),
    )
}

/// 没指定目标，而运行中的实例不止一个：`target_ambiguous`（2）。`cmd` 是命令名，用来在
/// 说明里给出可以照抄的命令行。
pub(crate) fn ambiguous(cmd: &str, ports: &[String]) -> Failure {
    let message = |lang| {
        let mut message = text::ambiguous(lang, ports.len());
        for port in ports {
            message.push_str(&format!("\n  aproxy {cmd} {port}"));
        }
        message
    };
    let numbers: Vec<_> = ports.iter().map(|port| port_value(port)).collect();
    Failure::rendered("target_ambiguous", message, json!({"ports": numbers}))
}

/// 实例的进程还在，却不应答控制通道：没带 `--force` 时处理不了它，`instance_unresponsive`（1）。
/// `alias` 是用户写的别名（按端口指定、或随 all 一起选中时为 None）。
pub(crate) fn unresponsive(alias: Option<&str>, port: &str, pid: u32) -> Failure {
    let who = |lang| match alias {
        Some(alias) => text::alias_pid(lang, alias, port, pid),
        None => text::port_pid(lang, port, pid),
    };
    Failure::rendered(
        "instance_unresponsive",
        |lang| text::unresponsive(lang, &who(lang)),
        json!({"port": port_value(port), "pid": pid}),
    )
}

/// 选中目标时被跳过的不应答实例，逐个记为失败：命令要处理它们而没能处理，不能报成成功。
/// stop 与 restart 共用。
pub(crate) fn fail_unresponsive_skipped(selection: &Selection, results: &mut Results) {
    for info in &selection.unresponsive_skipped {
        let port = daemon::port_of(&info.listen_addr);
        let failure = unresponsive(None, port, info.pid);
        eprintln!("{}", failure.terminal());
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
        Failure::rendered(
            "usage",
            |lang| text::idle_only(lang, cmd),
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
                say(json, text::forgotten(current(), &port));
            } else {
                say(json, text::no_port(current(), &port));
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
                    text::no_alias(current(), &target, config_path.display()),
                );
                // 这一项说的不是某个端口：别名的实例没在运行，也没留下记录
                results.ok("", "already_stopped", None);
            } else {
                say(
                    json,
                    text::forgot_config(
                        current(),
                        config_path.display(),
                        &ports.join(if current() == aproxy_ipc::i18n::Lang::Zh {
                            "、"
                        } else {
                            ", "
                        }),
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
            eprintln!("{}", failure.terminal());
            results.failed(&port, "failed", &failure);
            results.finish(json)
        }
    };
    fail_unresponsive_skipped(&selection, &mut results);
    match selection.empty {
        Some(Empty::NoneRunning) if selection.unresponsive_skipped.is_empty() => {
            say(json, super::status::text::none(current()))
        }
        Some(Empty::NoneIdle { idle_secs }) => say(json, text::none_idle(current(), idle_secs)),
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
                say(json, text::not_idle(current(), info.pid, port, idle_secs));
                results.ok(port, "skipped", Some("not_idle"));
            }
            Err(failure) => {
                let failure = stop_failure(info, &failure);
                eprintln!("{}", failure.terminal());
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
                text::cleaned(
                    current(),
                    &ports.join(if current() == aproxy_ipc::i18n::Lang::Zh {
                        "、"
                    } else {
                        ", "
                    }),
                ),
            );
        }
        for port in &ports {
            results.ok(port, "already_stopped", None);
        }
        for record in &unreadable {
            eprintln!(
                "{}",
                text::retain(
                    current(),
                    record.path.display(),
                    &record.reason.diagnostic().render(current())
                )
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
        Stopped::Graceful => text::stopped(current(), info.pid, port),
        Stopped::AlreadyGone => text::gone(current(), info.pid, port),
        Stopped::Forced => text::killed(current(), info.pid, port),
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
        StopFailure::GenerationChanged => Failure::rendered(
            "command_failed",
            |lang| text::generation(lang, port),
            json!({"port":port_value(port),"reason":"generation_changed"}),
        ),
        StopFailure::StillRunning => Failure::rendered(
            "stop_failed",
            |lang| text::still_running(lang, info.pid, port),
            json!({"port": port_value(port), "reason": "still_running"}),
        ),
        StopFailure::Unresponsive { error } => Failure::rendered(
            "instance_unresponsive",
            |lang| text::no_reply(lang, info.pid, port, &error.render(lang)),
            json!({"port": port_value(port), "pid": info.pid}),
        ),
        StopFailure::ForceFailed {
            error,
            restore_error,
        } => {
            let message = |lang| {
                let mut message = String::new();
                if let Some(werr) = restore_error {
                    message.push_str(&text::restore_error(lang, port, werr));
                }
                message.push_str(&text::kill_error(lang, info.pid, &error.render(lang)));
                message
            };
            Failure::rendered(
                "stop_failed",
                message,
                json!({"port": port_value(port), "reason": "kill_failed"}),
            )
        }
    }
}
