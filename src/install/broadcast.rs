//! PrepareSwap 广播与 ACK 收敛（铁律 3：ACK 齐了才交换）。
//!
//! ACK 判定的本质（用户定调）：实例把「进入二进制更换阶段」写进自己的
//! 可观测状态（swap_phase），而不是口头 ok。安装器发 PrepareSwap，新实例
//! 在响应里直接回置位后的状态（一个请求完成表达+确认）；旧实例（无此 op）
//! serde 反序列化失败回 ok:false = 未表达。
//!
//! 重试收敛策略（用户定调）：3 轮 × 每轮 3 次 ping（间隔 500ms）；轮间对
//! 未 ACK 实例执行 restart（用安装器自身 exe——顺带把落后实例拉到安装器
//! 版本，旧实例没有 PrepareSwap op 的兼容问题由此收敛）；终失败 → Err
//! （调用方走 abort——绝不强杀，绝对避免服务中断原则贯穿始终）。

use std::path::Path;
use std::time::Duration;

/// 单实例 ACK 判定：PrepareSwap 响应 ok 且 swap_phase == true。
/// 内部 3 次探测（间隔 500ms）吸收命名管道瞬时 busy 等瞬态失败。
/// unix 端点按显式 run_dir 派生（与库层其他 IPC 调用同一纪律）。
pub async fn ack_one(run_dir: &Path, port: &str) -> Result<(), String> {
    let endpoint = crate::daemon::endpoint_for_in(run_dir, port);
    let mut last = String::new();
    for attempt in 0..3 {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        match crate::daemon::ipc_request_to(&endpoint, &crate::daemon::IpcRequest::PrepareSwap)
            .await
        {
            // 响应里的 info 是实例置位后组装的——ok 即 ACK 完成
            Ok(resp) if resp.ok => {
                return Ok(());
            }
            Ok(_) => last = "实例未表达 PrepareSwap（旧版本？）".to_string(),
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// 全实例广播 + 重试收敛。`ports` 为当前运行实例端口清单（ACK 阶段快照，
/// 写入 install.state.instance_snapshot）。
/// 返回 Ok(()) = 全部 ACK；Err(失败端口) = 终失败（调用方 abort）。
pub async fn broadcast_prepare_swap(run_dir: &Path, ports: &[String]) -> Result<(), Vec<String>> {
    let exe = std::env::current_exe().map_err(|e| vec![format!("无法定位自身可执行文件: {e}")])?;
    let mut unacked: Vec<String> = Vec::new();
    for round in 0..3 {
        unacked.clear();
        for port in ports {
            if ack_one(run_dir, port).await.is_err() {
                unacked.push(port.clone());
            }
        }
        if unacked.is_empty() {
            return Ok(());
        }
        // 最后一轮不再 restart（没有下一轮 ACK 了）——直接终失败
        if round == 2 {
            break;
        }
        // 轮间收敛：未表达 = 旧实例/故障实例，restart 到安装器版本。
        // restart 自身失败也计入未 ACK（下轮再试）
        for port in &unacked {
            if let Err(e) = super::restart::restart_instance(run_dir, port, &exe).await {
                tracing::warn!(port = %port, error = %e, "ACK 收敛 restart 失败");
            }
        }
    }
    Err(unacked)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn ack_one_fails_cleanly_on_dead_port() {
        // 无实例的端口：PrepareSwap 不可达 → 未 ACK（不发火、不 panic）
        let dir = tempfile::tempdir().unwrap();
        assert!(ack_one(dir.path(), "59987").await.is_err());
    }

    #[tokio::test]
    async fn broadcast_reports_unacked_ports() {
        let dir = tempfile::tempdir().unwrap();
        // 无任何实例运行：两个端口全部未 ACK → Err 携带完整清单
        let ports = vec!["59988".to_string(), "59989".to_string()];
        let err = broadcast_prepare_swap(dir.path(), &ports)
            .await
            .unwrap_err();
        assert_eq!(err.len(), 2, "全部未 ACK 应逐一上报: {err:?}");
    }
}
