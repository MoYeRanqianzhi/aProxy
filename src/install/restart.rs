//! 实例重启原语：取恢复参数 → 优雅停止 → spawn 指定二进制 → IPC 就绪；
//! 新二进制起不来时用回退二进制按原参数把实例拉回。
//!
//! 两个消费方共享同一原语：
//! - **broadcast 的 ACK 收敛**：未表达 PrepareSwap 的旧实例被 restart 到
//!   安装器版本（顺带完成混版本舰队收敛）；
//! - **restarting 阶段的滚动重启**：逐实例 stop → 新 exe spawn。
//!
//! 「用安装器自身 exe」是关键：respawn 用 `current_exe()`——安装器跑在哪个
//! 镜像上，重启出的实例就是哪个版本。交棒之后广播由目标版本执行，此刻它跑在
//! staging 里，收敛重启出的实例也就跑在 staging 的副本上；交换后的滚动重启
//! 会把快照里的每个实例换到 bin，cleaning 删 staging 时它已不再被占用。
//!
//! ## 失败时的服务回滚（不是状态机回滚）
//!
//! 升级是「不中断运行」使命里最脆弱的一环：实例被优雅停止后，守护退出会
//! 删掉自己的 `.restore`；新二进制若在 bind 前就退出（新版本对旧配置的校验
//! 更严、被杀软拦截、8 秒内没就绪），既不会写 `.restore` 也不会写注册表——
//! 实例从此无人知晓，`aproxy restore` 也救不回。本原语为此做三件事：
//!
//! 1. **停止之前**把恢复参数记进 install.state 的在途清单（安装进程在窗口内
//!    崩溃/断电时，续作靠它把实例拉回）；
//! 2. **停止之后立即**回写 `.restore`（新实例 bind 成功时会用同样的参数
//!    覆盖它；失败时它就是 `aproxy restore` 与看护者的恢复依据）；
//! 3. 新二进制起不来 → 用回退二进制（旧版本）按原参数把实例拉回，并置
//!    install.state 的 `halted`，由调用方中止滚动、不再动剩余实例。
//!
//! 与 install-v1 的 forward-fix 铁律（swapping 之后只进不退）兼容：回滚的
//! 只是**这一个实例的服务**——bin 里的二进制、状态机阶段都不回退（阶段照常
//! 落 failed 保留现场），用户排除原因后重新执行 install 继续向前。

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::state::{InstallState, PendingRestore};

/// 实例重启失败的分类——调用方据此决定「中止滚动」还是「照常失败可续作」。
#[derive(Debug, Clone, PartialEq)]
pub enum RestartError {
    /// 实例未被本次重启动过（取恢复参数失败、停止超时等）：服务状态与调用
    /// 前一致，或停止未确认但在途记录已留存，续作可重试。
    NotRestarted(String),
    /// 新二进制起不来，已用回退二进制按原参数拉回（服务恢复，版本未升级）。
    RolledBack {
        reason: String,
        pid: u32,
        fallback: PathBuf,
    },
    /// 新二进制起不来，回退也失败：实例下线，`.restore` 已保住（排除原因后
    /// `aproxy restore` 可恢复）。
    Down {
        reason: String,
        fallback_error: String,
    },
}

impl std::fmt::Display for RestartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RestartError::NotRestarted(e) => write!(f, "{e}"),
            RestartError::RolledBack {
                reason,
                pid,
                fallback,
            } => write!(
                f,
                "新版本启动失败（{reason}）；已用旧二进制 {} 按原参数拉回实例（pid {pid}），\
                 滚动已中止，其余实例未动",
                fallback.display()
            ),
            RestartError::Down {
                reason,
                fallback_error,
            } => write!(
                f,
                "新版本启动失败（{reason}），旧二进制拉回也失败（{fallback_error}）；\
                 实例当前下线，恢复记录（.restore）已保留，排除原因后执行 `aproxy restore` 恢复"
            ),
        }
    }
}

/// 实例的恢复参数与日志路径：`.restore` 优先（实例在跑时它是权威）；缺失
/// 时取 install.state 的在途记录（已被本安装停止、尚未恢复的实例——`.restore`
/// 已随守护优雅退出删除）。**必须在 stop 之前调用**。
fn restore_entry(
    run_dir: &Path,
    state: &InstallState,
    port: &str,
) -> Result<(Vec<String>, String), String> {
    let (args, log_path) = match crate::daemon::list_restore_entries_in(run_dir)
        .into_iter()
        .find(|e| e.port == port)
    {
        Some(e) => (e.args, e.log_path),
        None => state
            .pending_restores
            .iter()
            .find(|p| p.port == port)
            .map(|p| (p.args.clone(), p.log_path.clone()))
            .ok_or_else(|| format!("实例 {port} 无恢复记录（非本安装启动的实例？）"))?,
    };
    // 配置文件已被删的记录无法忠实恢复
    if let Some(cfg) = args
        .iter()
        .position(|a| a == "--config")
        .and_then(|i| args.get(i + 1))
        && !Path::new(cfg).exists()
    {
        return Err(format!("配置文件已不存在: {cfg}"));
    }
    Ok((args, log_path))
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
        .find(|i| crate::daemon::port_of(&i.instance.listen_addr) == port)
        .map(|i| i.instance.pid);
    crate::daemon::ipc_request_in(run_dir, port, crate::daemon::IpcOp::Shutdown)
        .await
        .map_err(|e| e.to_string())?;
    let deadline = std::time::Instant::now() + timeout;
    loop {
        // 只有端点消失才算退出：挂死的实例端点还在、只是不应答
        if let Err(crate::daemon::IpcError::Unreachable(_)) =
            crate::daemon::ipc_ping_in(run_dir, port).await
        {
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

/// spawn 未就绪的细分：回滚前要知道「子进程是否还活着」——活着就可能
/// 占着端口，必须先清掉它，回退二进制才 bind 得上。
struct SpawnFailure {
    /// spawn 成功时的子进程 pid（spawn 本身失败为 None）
    pid: Option<u32>,
    /// spawn 返回后立刻读到的子进程创建时间（身份锚点，见 watchdog::record_identity）：
    /// 回收时按「pid + 创建时间」核验再终止——8 秒等待期间子进程若已退出、pid 被系统
    /// 复用给别的进程，届时现读的创建时间必然不符，绝不误杀。None = 读不到（进程瞬间
    /// 退出或平台不支持），此时不终止。
    start: Option<u64>,
    /// 截止时子进程仍存活（启动中但未写注册表 / 挂死）
    alive: bool,
    message: String,
}

/// spawn `exe` 按 `args` 启动守护（追加 --daemon-child）并等待注册表就绪
/// （8s，与 start/restore/看门狗 respawn 同一判定：新守护 bind 成功才写
/// 注册表，出现 = 就绪）。返回新 pid。子进程提前退出即刻判失败（不空等
/// 满 8 秒——配置校验失败这类确定性错误在毫秒级就会退出）。
///
/// 就绪判定按注册表定位而非 ping 端口：配置可能已被用户改动（换端口），
/// 新守护监听新端口时 ping 旧端口永远不通；spawn 返回的 pid 是唯一可靠锚点。
async fn spawn_and_wait_ready(
    run_dir: &Path,
    exe: &Path,
    args: &[String],
) -> Result<u32, SpawnFailure> {
    let mut full = args.to_vec();
    full.push("--daemon-child".to_string());
    let pid = crate::daemon::spawn_detached(exe, &full).map_err(|e| SpawnFailure {
        pid: None,
        start: None,
        alive: false,
        message: format!("spawn 失败（{}）: {e}", exe.display()),
    })?;
    let start = crate::watchdog::process_start_time(pid);
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    loop {
        if crate::daemon::registry_contains_pid_in(run_dir, pid) {
            return Ok(pid);
        }
        let exited = crate::watchdog::process_exited(pid) == Some(true);
        // 退出后再读一次注册表：bind 成功 → 写注册表 → 随即退出的极端时序
        // 不该被误判（虽然随后的 ping 会暴露它，判定口径保持与就绪一致）
        if exited && !crate::daemon::registry_contains_pid_in(run_dir, pid) {
            return Err(SpawnFailure {
                pid: Some(pid),
                start,
                alive: false,
                message: format!("新进程启动即退出（pid {pid}，详见守护日志/startup.log）"),
            });
        }
        if std::time::Instant::now() >= deadline {
            let alive = crate::watchdog::process_exited(pid) == Some(false);
            return Err(SpawnFailure {
                pid: Some(pid),
                start,
                alive,
                message: format!(
                    "实例重启后未在预期时间内就绪（pid {pid}，{}）",
                    if alive {
                        "进程存活"
                    } else {
                        "进程状态不可判"
                    }
                ),
            });
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// 清掉 8 秒内没就绪、仍存活的子进程（本函数刚 spawn 的、从未写注册表的
/// 进程——它不承载任何已知流量；不清掉它，回退二进制可能因端口被占而
/// bind 失败）。身份锚点是 spawn 时读到的「pid + 创建时间」：
/// `terminate_verified_process` 在同一进程句柄上先核对创建时间再终止，pid 已被
/// 复用时必然拒绝；读不到创建时间就不终止（宁可回退因端口被占而失败，也不误杀）。
/// 终止失败只记录，回退照常尝试（失败会如实上报为 Down）。
async fn reap_unready_child(pid: u32, start: Option<u64>) {
    let Some(start) = start else {
        tracing::warn!(
            pid,
            "未就绪的新进程读不到创建时间，无法核验身份，不终止；回退仍尝试"
        );
        return;
    };
    if let Err(e) = crate::watchdog::terminate_verified_process(pid, start) {
        tracing::warn!(pid, error = %e, "未就绪的新进程终止失败，回退仍尝试");
        return;
    }
    for _ in 0..50 {
        if crate::watchdog::process_exited(pid) != Some(false) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// 一步到位：取恢复参数（停止前！）→ 记在途 → 优雅停止（等进程终止）→
/// 回写 `.restore` → 用 `exe` 重启。返回新 pid。
///
/// `fallback`：新二进制起不来时拉回实例用的旧二进制。None = 用实例停止前
/// 的进程镜像路径——只在镜像文件**尚未被交换**时准确（swap 前的 ACK 收敛
/// 用）；swap 之后调用方必须显式传旧二进制（Windows 的 `.old`、unix 交换前
/// 保留的副本），因为原镜像路径此时指向的已是新文件。
///
/// 实例本就不在跑（续作接手的「已停未恢复」实例）→ 跳过停止直接拉起。
///
/// 停止预算 20s：守护的优雅退出含 **10 秒宽限强退**（server.rs 的 select
/// 双臂——在途连接挂起时 10s 后 process::exit），进程终止等待必须覆盖它
/// 并留余量，否则 rolling 阶段每一步都在与宽限计时竞速。
pub async fn restart_instance(
    run_dir: &Path,
    state: &mut InstallState,
    port: &str,
    exe: &Path,
    fallback: Option<&Path>,
) -> Result<u32, RestartError> {
    let (args, log_path) =
        restore_entry(run_dir, state, port).map_err(RestartError::NotRestarted)?;
    let live = crate::daemon::ipc_ping_in(run_dir, port).await.ok();
    let image = live
        .as_ref()
        .and_then(|i| crate::watchdog::process_image_path(i.instance.pid));

    // 在途记录先于停止落盘（窗口内崩溃的唯一留存，见模块文档）
    state.pending_restores.retain(|p| p.port != port);
    state.pending_restores.push(PendingRestore {
        port: port.to_string(),
        args: args.clone(),
        log_path: log_path.clone(),
    });
    super::state::write_in(run_dir, state)
        .map_err(|e| RestartError::NotRestarted(format!("install.state 写入失败: {e}")))?;

    if live.is_some() {
        stop_and_wait(run_dir, port, Duration::from_secs(20))
            .await
            .map_err(RestartError::NotRestarted)?;
    }
    // 停止后立即回写：守护优雅退出刚删掉它；新实例 bind 时会以同样的参数
    // 覆盖，起不来时它就是 `aproxy restore` / 看护者重拉的依据
    let rewrite_restore = || {
        if let Err(e) = crate::daemon::write_restore_file_in(run_dir, port, &args, &log_path) {
            tracing::warn!(port = %port, error = %e, "恢复记录回写失败（在途记录仍在 install.state）");
        }
    };
    rewrite_restore();

    let failure = match spawn_and_wait_ready(run_dir, exe, &args).await {
        Ok(pid) => {
            state.pending_restores.retain(|p| p.port != port);
            let _ = super::state::write_in(run_dir, state);
            return Ok(pid);
        }
        Err(f) => f,
    };
    tracing::error!(port = %port, error = %failure.message, "新二进制未能拉起实例，转入回滚");
    if failure.alive
        && let Some(pid) = failure.pid
    {
        reap_unready_child(pid, failure.start).await;
    }
    rewrite_restore();

    // 端口上已有可用实例（看护者按 .restore 抢先重拉 / 未就绪的子进程最终
    // 起来了）：服务已在，不再叠加回退——verifying 会核对版本
    if let Ok(info) = crate::daemon::ipc_ping_in(run_dir, port).await {
        state.pending_restores.retain(|p| p.port != port);
        let _ = super::state::write_in(run_dir, state);
        return Ok(info.instance.pid);
    }

    // 从这里起是实例级失败：无论回退成败，滚动都必须中止（见 halted 字段）
    state.halted = true;
    let fallback = fallback.map(Path::to_path_buf).or(image);
    let result = match fallback.filter(|p| p.is_file()) {
        None => Err(RestartError::Down {
            reason: failure.message,
            fallback_error: "没有可用的旧二进制".to_string(),
        }),
        Some(old) => match spawn_and_wait_ready(run_dir, &old, &args).await {
            Ok(pid) => {
                tracing::warn!(port = %port, pid, old = %old.display(), "已用旧二进制拉回实例");
                state.pending_restores.retain(|p| p.port != port);
                Err(RestartError::RolledBack {
                    reason: failure.message,
                    pid,
                    fallback: old,
                })
            }
            Err(f2) => {
                rewrite_restore();
                Err(RestartError::Down {
                    reason: failure.message,
                    fallback_error: f2.message,
                })
            }
        },
    };
    let _ = super::state::write_in(run_dir, state);
    result
}
