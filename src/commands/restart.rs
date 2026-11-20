//! `aproxy restart [PORT|all|ALIAS|idle [SECS]]`：重启运行中的实例。
//!
//! 语义边界（用户定调）：**只负责重启，不负责启动**——没在运行的 target 报
//! `target_not_running`（退出码 4），绝不顺手拉起。重启本身（预检 → 停止 → 按原来的拉起规格
//! 拉起 → 等就绪）在操作层 `aproxy::ops::restart`，先预检后停止的理由也写在那里；这里选目标、
//! 逐个重启、把结果说给用户。`restart all` 逐实例处理，失败的跳过、其余照常重启，最后汇总：
//! 结局是 `restarted`、`failed` 之一，有成有败时退出码 5。

use aproxy::daemon::{self, InstanceRecord};
use aproxy::ops::restart::{PrecheckError, RestartFailure, restart_instance};
use aproxy::ops::stop::{StopFailure, StopMode};
use aproxy::ops::target::{self, Empty, SelectError};
use serde_json::json;

use super::stop::{
    ambiguous, fail_unresponsive_skipped, require_idle_for_threshold, stop_failure, stopped_line,
    unknown_target, unresponsive,
};
use super::{Failure, Results, port_value, say};

/// 目标没在运行：restart 不启动它，`target_not_running`（4）。`target` 为 None 表示命令行上
/// 没写目标。
fn not_running(target: Option<&str>, message: String) -> Failure {
    Failure::new(
        "target_not_running",
        format!("{message}\nrestart 只重启运行中的实例，不启动新实例；要启动用 aproxy start"),
        json!({"target": target}),
    )
}

/// `aproxy restart` 入口。target/threshold/force 语义与 stop 完全一致。
pub(crate) async fn handle_restart_cmd(
    target: Option<String>,
    threshold: Option<u64>,
    force: bool,
    json: bool,
) {
    require_idle_for_threshold("restart", target.as_deref(), threshold, json);
    let mode = StopMode::from_force(force);
    let mut results = Results::new(target.as_deref());
    let selection = match target::select(target.as_deref(), threshold, force).await {
        Ok(selection) => selection,
        Err(SelectError::UnknownTarget { target }) => unknown_target(&target).exit(json),
        Err(SelectError::Ambiguous { ports }) => ambiguous("restart", &ports).exit(json),
        Err(SelectError::NotRunning {
            target,
            config_path,
        }) => not_running(
            Some(&target),
            format!(
                "别名 {target}（配置 {}）当前未在运行。",
                config_path.display()
            ),
        )
        .exit(json),
        Err(SelectError::NothingOnPort { port }) => not_running(
            Some(&port),
            format!("端口 {port} 上没有运行中的 aProxy 实例。"),
        )
        .exit(json),
        Err(SelectError::Unresponsive { target, port, pid }) => {
            let failure = unresponsive(target.as_deref(), &port, pid);
            eprintln!("{}", failure.message);
            results.failed(&port, "failed", &failure);
            results.finish(json)
        }
    };
    fail_unresponsive_skipped(&selection, &mut results);
    match selection.empty {
        // 没写目标却一个运行中的实例都没有：要重启的那个实例不存在
        Some(Empty::NoneRunning)
            if target.is_none() && selection.unresponsive_skipped.is_empty() =>
        {
            not_running(None, "没有运行中的 aProxy 实例。".to_string()).exit(json)
        }
        // all：没有运行中的实例就是没有要重启的，无事可做
        Some(Empty::NoneRunning) if selection.unresponsive_skipped.is_empty() => {
            say(json, "没有运行中的 aProxy 实例。")
        }
        Some(Empty::NoneIdle { idle_secs }) => {
            say(json, format!("没有闲置超过 {idle_secs} 秒的实例。"))
        }
        _ => {}
    }
    let exe = std::env::current_exe().expect("无法定位自身可执行文件");
    // 多实例时最后再汇总一遍：逐条失败信息可能已被后续实例的输出冲散
    let mut failed: Vec<(String, &'static str)> = Vec::new();
    for info in &selection.instances {
        let port = daemon::port_of(&info.listen_addr).to_string();
        match restart_instance(info, mode, &exe).await {
            Ok(done) => {
                say(json, stopped_line(info, done.stopped));
                // 这一项的端口是新实例实际监听的：配置改了端口时它与旧实例的不同
                let new_port = daemon::port_of(&done.live.instance.listen_addr);
                say(
                    json,
                    format!(
                        "已重启（端口 {new_port}）\n  pid: {}（旧 pid {}）\n  监听: http://{}",
                        done.live.instance.pid, done.old_pid, done.live.instance.listen_addr
                    ),
                );
                results.ok(new_port, "restarted", None);
            }
            Err(failure) => {
                let state = match &failure {
                    RestartFailure::Starting => {
                        "Instance is still starting; no restart was attempted."
                    }
                    RestartFailure::Unsupported => "没有可用来重新拉起它的记录，旧实例保持运行",
                    RestartFailure::Precheck(_) => "预检未通过，旧实例保持运行",
                    RestartFailure::Stop(StopFailure::GenerationChanged) => {
                        "The current instance was not stopped."
                    }
                    RestartFailure::Stop(_) => "停止未确认，未启动新实例",
                    RestartFailure::Start { .. } => "旧实例已停止、新实例未就绪，服务已中断",
                };
                let failure = restart_failure(info, &port, failure, json);
                eprintln!("{}", failure.message);
                results.failed(&port, "failed", &failure);
                failed.push((port, state));
            }
        }
    }
    if !failed.is_empty() && selection.instances.len() > 1 {
        eprintln!(
            "\n{} 个实例中有 {} 个重启失败（已跳过，其余照常重启）：",
            selection.instances.len(),
            failed.len()
        );
        for (port, state) in &failed {
            eprintln!("  端口 {port}：{state}");
        }
    }
    results.finish(json)
}

/// 一个实例没能重启：失败所处的阶段决定码与说明。旧实例已经停下的那一种，先说它是怎样停下的
/// （`json` 为真时这行写到 stderr）。
fn restart_failure(
    info: &InstanceRecord,
    port: &str,
    failure: RestartFailure,
    json: bool,
) -> Failure {
    match failure {
        RestartFailure::Starting => Failure::new(
            "command_failed",
            format!("Instance on port {port} is still starting; retry after it begins serving."),
            json!({}),
        ),
        RestartFailure::Unsupported => Failure::new(
            "restart_unsupported",
            format!(
                "端口 {port} 未重启：这个实例没有可用来重新拉起它的记录（前台运行的实例要在它自己的终端里重启），旧实例保持运行。"
            ),
            json!({"port": port_value(port)}),
        ),
        RestartFailure::Precheck(reason) => {
            let mut failure = precheck_failure(port, reason);
            failure.message = format!(
                "端口 {port} 未重启：预检未通过，旧实例保持运行。\n{}",
                failure.message
            );
            failure
        }
        RestartFailure::Stop(stop @ StopFailure::GenerationChanged) => stop_failure(info, &stop),
        RestartFailure::Stop(stop) => {
            let mut failure = stop_failure(info, &stop);
            failure.message = format!(
                "{}\n端口 {port} 停止未确认，重启中止（未启动新实例，以免与旧实例争抢端口）。",
                failure.message
            );
            failure
        }
        RestartFailure::Start {
            stopped,
            error,
            config_path,
        } => {
            say(json, stopped_line(info, stopped));
            // 期望状态记录还在：看护者会继续按它重拉，用户排除原因后也可以自己执行 restore
            let mut message = format!(
                "端口 {port} 重启失败（旧实例已停止，该端口服务已中断）：{error}{}\n修正上述问题后执行 aproxy restore 恢复服务（看护者开着的话也会自动重试）。",
                error.startup_log().map(|l| l.excerpt()).unwrap_or_default(),
            );
            if let Some(path) = config_path {
                message.push_str(&format!("\n  配置: {}", path.display()));
            }
            super::spawn_failure(&error, message)
        }
    }
}

/// 预检不通过的原因：配置无效是 `config_invalid`（3），配置文件不在是 `target_not_found`（4），
/// 记录读不懂是 `restart_unsupported`（没有能用来重新拉起它的规格），端口的问题是
/// `port_in_use` 或 `start_failed`（1）。
fn precheck_failure(port: &str, reason: PrecheckError) -> Failure {
    match reason {
        PrecheckError::RecordUnreadable { reason } => Failure::new(
            "restart_unsupported",
            format!("这个实例的期望状态记录读不懂：{reason}"),
            json!({"port": port_value(port)}),
        ),
        PrecheckError::ConfigMissing { path } => Failure::new(
            "target_not_found",
            format!("配置文件已不存在（{}）", path.display()),
            json!({"target": path.display().to_string()}),
        ),
        PrecheckError::Config(error) => super::start::config_failure(&error),
        PrecheckError::PortTaken { port, pid } => Failure::new(
            "port_in_use",
            format!("新配置的监听端口 {port} 上已有 aProxy 实例（pid {pid}）在运行"),
            json!({"port": port_value(&port), "owner": "aproxy"}),
        ),
        PrecheckError::Bind { listen_addr, error } => super::bind_failure(&listen_addr, &error),
    }
}
