//! 实例重启原语：取恢复参数 → 优雅停止 → spawn 指定二进制 → IPC 就绪。
//!
//! 两个消费方共享同一原语：
//! - **broadcast 的 ACK 收敛**：未表达 PrepareSwap 的旧实例被 restart 到
//!   安装器版本（顺带完成混版本舰队收敛）；
//! - **restarting 阶段的滚动重启**：逐实例 stop → 新 exe spawn。
//!
//! 「用安装器自身 exe」是关键：respawn 用 `current_exe()`——安装器跑在哪个
//! 镜像上，重启出的实例就是哪个版本（接力交棒后由接管者用自己的新 bin）。

use std::path::Path;
use std::time::Duration;

/// 读取实例的恢复记录参数（.restore）。**必须在 stop 之前调用**——守护的
/// 优雅退出会删除 .restore（先 .restore 后 .pid 的退出清理顺序），停止后
/// 再读就只剩「无恢复记录」的错误了。
pub fn restore_args(run_dir: &Path, port: &str) -> Result<Vec<String>, String> {
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
    Ok(entry.args.clone())
}

/// 优雅停止并等待**进程真正终止**（Shutdown → 实例优雅退出）。
///
/// 两段确认，缺一不可：
/// 1. IPC 消失（管道/ socket 没了 = 服务停止）——**不等于进程终止**：守护
///    的 shutdown 流程先关 IPC 再做清理最后进程退出，中间有窗口；
/// 2. 进程终止（stop 前从注册表取 pid 锚点，轮询 process_start_time 变
///    None）——Windows 镜像锁随终止释放，cleaning 删 `.old`（= 被停实例
///    的运行镜像）依赖此确认，否则删除被锁失败残留。
///
/// 超时 = Err（调用方决定处置，绝不强杀——绝对避免服务中断原则贯穿
/// install 全流程）。
pub async fn stop_and_wait(run_dir: &Path, port: &str, timeout: Duration) -> Result<(), String> {
    // 停止前取 pid 锚点（注册表由守护 bind 时写入，停止后即删）
    let pid = crate::daemon::list_instances_in(run_dir)
        .await
        .iter()
        .find(|i| crate::daemon::port_of(&i.listen_addr) == port)
        .map(|i| i.pid);
    crate::daemon::ipc_request(port, &crate::daemon::IpcRequest::Shutdown).await?;
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if crate::daemon::ipc_ping(port).await.is_err() {
            break;
        }
        if std::time::Instant::now() >= deadline {
            return Err(format!("实例 {port} 未在预期时间内退出（仍拒绝强杀）"));
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    if let Some(pid) = pid {
        loop {
            match crate::watchdog::process_exited(pid) {
                Some(true) => break,
                Some(false) => {}
                None => break, // 不可判（权限等）——保守放行，后续步骤自会暴露
            }
            if std::time::Instant::now() >= deadline {
                return Err(format!(
                    "实例 {port} 的进程（pid {pid}）未在预期时间内终止（仍拒绝强杀）"
                ));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    Ok(())
}

/// spawn `exe` 按 `args` 启动守护（追加 --daemon-child）并等待注册表就绪
/// （8s，与 start/restore/看门狗 respawn 同一判定：新守护 bind 成功才写
/// 注册表，出现 = 就绪）。返回新 pid。
///
/// 就绪判定按注册表定位而非 ping 端口：配置可能已被用户改动（换端口），
/// 新守护监听新端口时 ping 旧端口永远不通；spawn 返回的 pid 是唯一可靠锚点。
pub fn spawn_and_wait_ready(run_dir: &Path, exe: &Path, args: &[String]) -> Result<u32, String> {
    let mut full = args.to_vec();
    full.push("--daemon-child".to_string());
    let pid = crate::daemon::spawn_detached(exe, &full).map_err(|e| format!("spawn 失败: {e}"))?;
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    loop {
        if crate::daemon::registry_contains_pid_in(run_dir, pid) {
            return Ok(pid);
        }
        if std::time::Instant::now() >= deadline {
            // 附带进程状态：已退出 = 子进程启动即死（bind 失败等）；仍活着 =
            // 启动中但注册表未落（写失败/路径不一致）
            let state = match crate::watchdog::process_exited(pid) {
                Some(true) => "进程已退出",
                Some(false) => "进程存活",
                None => "进程状态不可判",
            };
            return Err(format!(
                "实例重启后未在预期时间内就绪（pid {pid}，{state}）"
            ));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// 一步到位：取恢复参数（停止前！）→ 优雅停止（等进程终止）→ 用 `exe`
/// 重启。返回新 pid。
///
/// 停止预算 20s：守护的优雅退出含 **10 秒宽限强退**（server.rs 的 select
/// 双臂——在途连接挂起时 10s 后 process::exit），进程终止等待必须覆盖它
/// 并留余量，否则 rolling 阶段每一步都在与宽限计时竞速。
pub async fn restart_instance(run_dir: &Path, port: &str, exe: &Path) -> Result<u32, String> {
    let args = restore_args(run_dir, port)?;
    stop_and_wait(run_dir, port, Duration::from_secs(20)).await?;
    spawn_and_wait_ready(run_dir, exe, &args)
}
