//! 实例重启原语：读期望状态记录里的拉起规格 → 优雅停止（记录留着）→ 用指定的二进制
//! 按同一份规格拉起 → IPC 就绪；新二进制起不来时用回退二进制按同一份规格把实例拉回。
//!
//! 两个消费方共享同一原语：
//! - **broadcast 的 ACK 收敛**：未表达 PrepareSwap 的旧实例被 restart 到
//!   安装器版本（顺带完成混版本舰队收敛）；
//! - **restarting 阶段的滚动重启**：逐实例 stop → 新 exe 拉起。
//!
//! 「用安装器自身 exe」是关键：拉起用 `current_exe()`——安装器跑在哪个
//! 镜像上，重启出的实例就是哪个版本。交棒之后广播由目标版本执行，此刻它跑在
//! staging 里，收敛重启出的实例也就跑在 staging 的副本上；交换后的滚动重启
//! 会把快照里的每个实例换到 bin，cleaning 删 staging 时它已不再被占用。
//!
//! ## 失败时的服务回滚（不是状态机回滚）
//!
//! 升级是「不中断运行」使命里最脆弱的一环：新二进制若起不来（新版本对旧配置的校验
//! 更严、被杀软拦截、时限内没就绪），实例就停在了「旧的已停、新的没起」。本原语靠两点
//! 保住它：
//!
//! 1. 停止用带 `keep_restore` 的 shutdown，**期望状态记录从头到尾都在**：安装进程在
//!    窗口内崩溃或断电，续作、`aproxy restore` 与看护者都能按它把实例拉回。安装状态
//!    文件因此不必存启动参数（它不含密钥），只记端口；
//! 2. 新二进制起不来 → 用回退二进制（旧版本）按同一份规格把实例拉回，并置
//!    install.state 的 `halted`，由调用方中止滚动、不再动剩余实例。
//!
//! 与 install-v1 的 forward-fix 铁律（swapping 之后只进不退）兼容：回滚的
//! 只是**这一个实例的服务**——bin 里的二进制、状态机阶段都不回退（阶段照常
//! 落 failed 保留现场），用户排除原因后重新执行 install 继续向前。

use super::{flow_issue::FlowIssue, reason::Cause};
use aproxy_ipc::i18n::Lang;

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::state::InstallState;
use crate::spec::InstanceSpec;

/// 实例重启失败的分类——调用方据此决定「中止滚动」还是「照常失败可续作」。
#[derive(Debug, Clone)]
pub enum RestartError {
    /// 实例未被本次重启动过（没有期望状态记录、停止超时等）：服务状态与调用
    /// 前一致，或停止未确认但期望状态记录还在，续作可重试。
    NotRestarted(Box<Cause>),
    /// 新二进制起不来，已用回退二进制按同一份规格拉回（服务恢复，版本未升级）。
    RolledBack {
        reason: Box<Cause>,
        pid: u32,
        fallback: PathBuf,
    },
    /// 新二进制起不来，回退也失败：实例下线，期望状态记录还在（排除原因后
    /// `aproxy restore` 可恢复）。
    Down {
        reason: Box<Cause>,
        fallback_error: Box<Cause>,
    },
}

impl InstallState {
    /// 记录一次真正结束的实例滚动。没有停止或拉起实例的错误不虚构结局；
    /// 失败结局与 halted 一起由外层落盘，成功结局由逐实例的进度写入保存。
    pub(super) fn record_restart(&mut self, port: &str, result: &Result<u32, RestartError>) {
        let outcome = match result {
            Ok(_) => "restarted",
            Err(RestartError::RolledBack { .. }) => "rolled_back",
            Err(RestartError::Down { .. }) => "failed",
            Err(RestartError::NotRestarted(_)) => return,
        };
        if let Ok(port) = port.parse() {
            self.instance_outcomes.push(super::state::InstanceOutcome {
                port,
                outcome: outcome.into(),
            });
        }
    }
}

impl RestartError {
    /// 新版本拉不起实例才中止滚动；停止前的失败不声称换过二进制或回退过实例。
    pub(super) fn failure(
        self,
        port: &str,
        wrap: impl FnOnce(Cause) -> FlowIssue,
    ) -> super::InstallFailure {
        let rolled = !matches!(self, Self::NotRestarted(_));
        let cause = wrap(self.into_cause());
        if rolled && let Ok(port) = port.parse::<u16>() {
            let mut error = super::InstallFailure::because("install_restart_failed", cause.into());
            *error.details = serde_json::json!({"port":port});
            error
        } else {
            cause.into()
        }
    }
    fn into_cause(self) -> Cause {
        match self {
            Self::NotRestarted(c) => *c,
            Self::RolledBack {
                reason,
                pid,
                fallback,
            } => FlowIssue::RolledBack {
                reason,
                path: fallback.display().to_string(),
                pid,
            }
            .into(),
            Self::Down {
                reason,
                fallback_error,
            } => FlowIssue::Down {
                reason,
                fallback_error,
            }
            .into(),
        }
    }
}
impl std::fmt::Display for RestartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.clone().into_cause().render(Lang::En))
    }
}

/// 实例的拉起规格，取自它的期望状态记录。没有记录的实例（前台实例，或 stop 之后
/// 残留在快照里的端口）无从重启。
fn restore_spec(run_dir: &Path, port: &str) -> Result<InstanceSpec, Cause> {
    let spec = match crate::spec::read_restore_in(run_dir, port) {
        Ok(Some(entry)) => entry.spec,
        Ok(None) => {
            return Err(FlowIssue::NoRestore {
                port: port.to_string(),
            }
            .into());
        }
        Err(reason) => {
            return Err(FlowIssue::Unreadable {
                port: port.to_string(),
                reason: Box::new(Cause::Diagnostic(Box::new(reason.diagnostic()))),
            }
            .into());
        }
    };
    // 配置文件不在的规格拉不起来：新旧二进制都会在读配置时失败
    if let Some(cfg) = spec.config_file()
        && !cfg.exists()
    {
        return Err(FlowIssue::MissingConfig {
            path: cfg.display().to_string(),
        }
        .into());
    }
    Ok(spec)
}

/// 优雅停止并等待**进程真正终止**（Shutdown → 实例优雅退出）。期望状态记录留着：实例认
/// `keep_restore` 时带上它；不认的旧实例会在退出时删掉记录，由调用方在停止之后写回。
///
/// 两段确认，缺一不可：
/// 1. IPC 消失（管道/ socket 没了 = 服务停止）——**不等于进程终止**：守护
///    的 shutdown 流程先关 IPC 再做清理最后进程退出，中间有窗口；
/// 2. 进程终止（从 shutdown 应答取实际接到请求的 pid，轮询 process_exited 确认
///    退出）——Windows 镜像锁随终止释放，cleaning 删让位的旧二进制（= 被停实例
///    的运行镜像）依赖此确认，否则删除被锁失败残留。
///
/// 超时 = Err（调用方决定处置，绝不强杀——绝对避免服务中断原则贯穿
/// install 全流程）。
pub async fn stop_and_wait(run_dir: &Path, port: &str, timeout: Duration) -> Result<(), Cause> {
    // 以接受停止的实例为准；不依赖它的注册表记录何时删除，也不为缺失的 pid 放行。
    let pid =
        crate::daemon::ipc_request_in(run_dir, port, crate::daemon::IpcOp::STOP_KEEPING_RESTORE)
            .await
            .map_err(|e| Cause::Control(Box::new(e)))?
            .instance
            .pid;
    let deadline = std::time::Instant::now() + timeout;
    loop {
        // 只有端点消失才算退出：挂死的实例端点还在、只是不应答
        if let Err(crate::daemon::IpcError::Unreachable(_)) =
            crate::daemon::ipc_ping_in(run_dir, port).await
        {
            break;
        }
        if std::time::Instant::now() >= deadline {
            return Err(FlowIssue::StopTimeout {
                port: port.to_string(),
            }
            .into());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    wait_for_exit(port, pid, deadline, crate::process::process_exited).await
}

/// IPC 消失只证明不再接单；查不到状态也不能证明进程已经释放了端口与镜像。
async fn wait_for_exit(
    port: &str,
    pid: u32,
    deadline: std::time::Instant,
    mut exited: impl FnMut(u32) -> Option<bool>,
) -> Result<(), Cause> {
    while exited(pid) != Some(true) {
        if std::time::Instant::now() >= deadline {
            return Err(FlowIssue::ExitTimeout {
                port: port.to_string(),
                pid,
            }
            .into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Ok(())
}

/// 按新 pid 等待就绪，保留只有 spawn 本身失败时才显示的二进制路径。
async fn spawn_and_wait_ready(
    home: &Path,
    exe: &Path,
    spec: &InstanceSpec,
    on_timeout: crate::spawn::OnReadyTimeout,
) -> Result<u32, Box<Cause>> {
    crate::spawn::spawn_from_spec_with_timeout_action(
        home,
        exe,
        spec,
        crate::spawn::READY_TIMEOUT,
        on_timeout,
    )
    .await
    .map(|live| live.instance.pid)
    .map_err(|e| {
        let launch = Cause::Launch(Box::new(e.evidence()));
        Box::new(if matches!(e, crate::spawn::SpawnError::Spawn(_)) {
            FlowIssue::AtPath {
                message: Box::new(launch),
                path: exe.display().to_string(),
            }
            .into()
        } else {
            launch
        })
    })
}

/// 一步到位：读拉起规格 → 优雅停止（等进程终止，期望状态记录留着）→ 用 `exe` 按同一份
/// 规格重启。返回新 pid。halted 写进 `home` 的安装状态文件，实例的注册表与期望状态记录
/// 在 `run_dir`。
///
/// `fallback`：新二进制起不来时拉回实例用的旧二进制。None = 用实例停止前
/// 的进程镜像路径——只在镜像文件**尚未被交换**时准确（swap 前的 ACK 收敛
/// 用）；swap 之后调用方必须显式传旧二进制（install.state 的 old_path：Windows
/// 是让位的原文件、unix 是交换前保留的硬链接或副本），因为原镜像路径此时指向的
/// 已是新文件。
///
/// 实例本就不在跑（续作接手的「已停未恢复」实例，记录还在）→ 跳过停止直接拉起。
///
/// 停止预算 20s：实例的优雅退出含 **10 秒宽限强退**（server.rs 的 select
/// 双臂——在途连接挂起时 10s 后 process::exit），进程终止等待必须覆盖它
/// 并留余量，否则 rolling 阶段每一步都在与宽限计时竞速。
pub async fn restart_instance(
    home: &Path,
    run_dir: &Path,
    state: &mut InstallState,
    port: &str,
    exe: &Path,
    fallback: Option<&Path>,
) -> Result<u32, RestartError> {
    let spec = restore_spec(run_dir, port).map_err(|e| RestartError::NotRestarted(Box::new(e)))?;
    let live = crate::daemon::ipc_ping_in(run_dir, port).await.ok();
    let image = live
        .as_ref()
        .and_then(|i| crate::process::process_image_path(i.instance.pid));

    if live.is_some() {
        stop_and_wait(run_dir, port, Duration::from_secs(20))
            .await
            .map_err(|e| RestartError::NotRestarted(Box::new(e)))?;
    }
    // 记录必须在：不认 `keep_restore` 的旧实例退出时会删掉它。新实例就绪时会用同样的
    // 规格覆盖它；起不来时它就是续作、`aproxy restore` 与看护者重拉的依据
    let ensure_restore = || {
        if !crate::spec::restore_path_in(run_dir, port).is_file()
            && let Err(e) = crate::spec::write_restore_in(run_dir, port, &spec)
        {
            tracing::warn!(port = %port, error = %e, "Failed to write the desired-state record");
        }
    };
    ensure_restore();

    let failure = match spawn_and_wait_ready(
        home,
        exe,
        &spec,
        crate::spawn::OnReadyTimeout::StopBeforeRollback,
    )
    .await
    {
        Ok(pid) => return Ok(pid),
        Err(f) => f,
    };
    tracing::error!(port = %port, error = %failure.render(Lang::En), "The new binary failed to start the instance; rolling back");
    ensure_restore();

    // 端口上已有可用实例（看护者按期望状态记录抢先重拉 / 未就绪的子进程最终
    // 起来了）：服务已在，不再叠加回退——verifying 会核对版本
    if let Ok(info) = crate::daemon::ipc_ping_in(run_dir, port).await {
        return Ok(info.instance.pid);
    }

    // 从这里起是实例级失败：无论回退成败，滚动都必须中止（见 halted 字段）
    state.halted = true;
    let fallback = fallback.map(Path::to_path_buf).or(image);
    let result = match fallback.filter(|p| p.is_file()) {
        None => Err(RestartError::Down {
            reason: failure,
            fallback_error: Box::new(FlowIssue::NoFallback.into()),
        }),
        Some(old) => match spawn_and_wait_ready(
            home,
            &old,
            &spec,
            crate::spawn::OnReadyTimeout::LeaveRunning,
        )
        .await
        {
            Ok(pid) => {
                tracing::warn!(port = %port, pid, old = %old.display(), "The old binary restored the instance");
                Err(RestartError::RolledBack {
                    reason: failure,
                    pid,
                    fallback: old,
                })
            }
            Err(f2) => {
                ensure_restore();
                Err(RestartError::Down {
                    reason: failure,
                    fallback_error: f2,
                })
            }
        },
    };
    let _ = super::state::write_in(home, state);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn rolling_stop_waits_for_positive_exit_evidence() {
        let pid = std::process::id();
        let mut answers = [None, Some(false), Some(true)].into_iter();
        wait_for_exit(
            "0",
            pid,
            std::time::Instant::now() + Duration::from_secs(20),
            |_| answers.next().unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(answers.len(), 0);
        let err = wait_for_exit("0", pid, std::time::Instant::now(), |_| None)
            .await
            .unwrap_err();
        assert_eq!(
            err.render(Lang::En),
            Cause::from(FlowIssue::ExitTimeout {
                port: "0".into(),
                pid
            })
            .render(Lang::En)
        );
    }
}
