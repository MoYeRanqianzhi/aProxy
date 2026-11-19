//! `aproxy restart [PORT|all|ALIAS|idle [SECS]]`：重启运行中的实例。
//!
//! 语义边界（用户定调）：**只负责重启，不负责启动**——未运行的 target 提示
//! 「未启动」并退出 1，绝不顺手拉起。重启本身（预检 → 停止 → 按原来的拉起规格拉起 → 等就绪）
//! 在操作层 `aproxy::ops::restart`，先预检后停止的理由也写在那里；这里选目标、逐个重启、
//! 把结果说给用户。`restart all` 逐实例处理，失败的跳过、其余照常重启，最后汇总并以非零
//! 退出。

use aproxy::daemon;
use aproxy::ops::restart::{PrecheckError, RestartFailure, restart_instance};
use aproxy::ops::stop::StopMode;

use crate::commands::stop::{report_stop_failure, report_stopped, select_or_exit};
use crate::server::bind_error_message;

/// `aproxy restart` 入口。target/threshold/force 语义与 stop 完全一致。
pub(crate) async fn handle_restart_cmd(
    target: Option<String>,
    threshold: Option<u64>,
    force: bool,
) {
    super::stop::require_idle_for_threshold(target.as_deref(), threshold);
    let mode = StopMode::from_force(force);
    let targets = select_or_exit(target.as_deref(), threshold, force).await;
    if targets.is_empty() {
        return; // 无运行实例：选目标时已输出提示（restart 不启动新实例）
    }
    let exe = std::env::current_exe().expect("无法定位自身可执行文件");
    let mut failures: Vec<(String, RestartFailure)> = Vec::new();
    for info in &targets {
        let port = daemon::port_of(&info.listen_addr).to_string();
        match restart_instance(info, mode, &exe).await {
            Ok(done) => {
                report_stopped(info, done.stopped);
                let new_port = daemon::port_of(&done.live.instance.listen_addr);
                println!("已重启（端口 {new_port}）");
                println!(
                    "  pid: {}（旧 pid {}）",
                    done.live.instance.pid, done.old_pid
                );
                println!("  监听: http://{}", done.live.instance.listen_addr);
            }
            Err(failure) => {
                match &failure {
                    RestartFailure::Unsupported => {
                        eprintln!(
                            "端口 {port} 未重启：这个实例没有可用来重新拉起它的记录（前台运行的实例要在它自己的终端里重启），旧实例保持运行。"
                        );
                    }
                    RestartFailure::Precheck(reason) => {
                        eprintln!(
                            "端口 {port} 未重启：预检未通过，旧实例保持运行。\n{}",
                            precheck_reason(reason)
                        );
                    }
                    RestartFailure::Stop(stop_failure) => {
                        report_stop_failure(info, stop_failure);
                        eprintln!(
                            "端口 {port} 停止未确认，重启中止（未启动新实例，以免与旧实例争抢端口）。"
                        );
                    }
                    RestartFailure::Start {
                        stopped,
                        error,
                        config_path,
                    } => {
                        report_stopped(info, *stopped);
                        // 期望状态记录还在：看护者会继续按它重拉，用户排除原因后也可以
                        // 自己执行 restore
                        eprintln!(
                            "端口 {port} 重启失败（旧实例已停止，该端口服务已中断）：{error}{}\n修正上述问题后执行 aproxy restore 恢复服务（看护者开着的话也会自动重试）。",
                            error.startup_log().map(|l| l.excerpt()).unwrap_or_default(),
                        );
                        if let Some(path) = config_path {
                            eprintln!("  配置: {}", path.display());
                        }
                    }
                }
                failures.push((port, failure));
            }
        }
    }
    if failures.is_empty() {
        return;
    }
    // 多实例时汇总：逐条失败信息可能已被后续实例的输出冲散
    if targets.len() > 1 {
        eprintln!(
            "\n{} 个实例中有 {} 个重启失败（已跳过，其余照常重启）：",
            targets.len(),
            failures.len()
        );
        for (port, failure) in &failures {
            let state = match failure {
                RestartFailure::Unsupported => "没有可用来重新拉起它的记录，旧实例保持运行",
                RestartFailure::Precheck(_) => "预检未通过，旧实例保持运行",
                RestartFailure::Stop(_) => "停止未确认，未启动新实例",
                RestartFailure::Start { .. } => "旧实例已停止、新实例未就绪，服务已中断",
            };
            eprintln!("  端口 {port}：{state}");
        }
    }
    std::process::exit(1);
}

/// 预检不通过的原因，说给用户听。
fn precheck_reason(e: &PrecheckError) -> String {
    match e {
        PrecheckError::RecordUnreadable { reason } => {
            format!("这个实例的期望状态记录读不懂：{reason}")
        }
        PrecheckError::ConfigMissing { path } => {
            format!("配置文件已不存在（{}）", path.display())
        }
        PrecheckError::Config { reason } => reason.clone(),
        PrecheckError::PortTaken { port, pid } => {
            format!("新配置的监听端口 {port} 上已有 aProxy 实例（pid {pid}）在运行")
        }
        PrecheckError::Bind { listen_addr, error } => bind_error_message(listen_addr, error),
    }
}
