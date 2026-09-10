//! 实例重启原语：优雅停止 → 按恢复记录原参数 spawn 指定二进制 → IPC 就绪。
//!
//! 两个消费方共享同一原语：
//! - **broadcast 的 ACK 收敛**：未表达 PrepareSwap 的旧实例被 restart 到
//!   安装器版本（顺带完成混版本舰队收敛）；
//! - **restarting 阶段的滚动重启**（第 5 步）：逐实例 stop → 新 exe spawn。
//!
//! 「用安装器自身 exe」是关键：respawn 用 `current_exe()`——安装器跑在哪个
//! 镜像上，重启出的实例就是哪个版本。

use std::path::Path;
use std::time::Duration;

/// 优雅停止并等待 IPC 消失（Shutdown → 实例自行优雅退出：写日志、清理
/// 注册表与 socket、进程退出）。超时 = Err（调用方决定处置，绝不强杀——
/// 绝对避免服务中断原则贯穿 install 全流程）。
pub async fn stop_and_wait(port: &str, timeout: Duration) -> Result<(), String> {
    crate::daemon::ipc_request(port, &crate::daemon::IpcRequest::Shutdown).await?;
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if crate::daemon::ipc_ping(port).await.is_err() {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err(format!("实例 {port} 未在预期时间内退出（仍拒绝强杀）"));
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// 按恢复记录原参数 spawn `exe` 的守护并等待注册表就绪（8s，与
/// start/restore/看门狗 respawn 同一判定：新守护 bind 成功才写注册表，
/// 出现 = 就绪）。返回新 pid。
///
/// 就绪判定按注册表定位而非 ping 端口：配置可能已被用户改动（换端口），
/// 新守护监听新端口时 ping 旧端口永远不通；spawn 返回的 pid 是唯一可靠锚点。
pub fn start_from_restore(run_dir: &Path, port: &str, exe: &Path) -> Result<u32, String> {
    let entries = crate::daemon::list_restore_entries_in(run_dir);
    let entry = entries
        .iter()
        .find(|e| e.port == port)
        .ok_or_else(|| format!("实例 {port} 无恢复记录（非本安装启动的实例？）"))?;
    // 配置文件已被删的记录无法忠实恢复
    if let Some(cfg) = entry
        .args
        .iter()
        .position(|a| a == "--config")
        .and_then(|i| entry.args.get(i + 1))
        && !Path::new(cfg).exists()
    {
        return Err(format!("配置文件已不存在: {cfg}"));
    }
    let mut args = entry.args.clone();
    args.push("--daemon-child".to_string());
    let pid = crate::daemon::spawn_detached(exe, &args).map_err(|e| format!("spawn 失败: {e}"))?;
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    loop {
        if crate::daemon::registry_contains_pid_in(run_dir, pid) {
            return Ok(pid);
        }
        if std::time::Instant::now() >= deadline {
            return Err(format!("实例 {port} 重启后未在预期时间内就绪"));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// 一步到位：优雅停止 → 用 `exe` 按原参数重启。返回新 pid。
pub async fn restart_instance(run_dir: &Path, port: &str, exe: &Path) -> Result<u32, String> {
    stop_and_wait(port, Duration::from_secs(10)).await?;
    start_from_restore(run_dir, port, exe)
}
