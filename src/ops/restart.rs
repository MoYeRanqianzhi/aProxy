//! 重启一个运行中的实例：预检 → 停止（留着期望状态记录）→ 按同一份拉起规格拉起 → 等就绪。
//!
//! 语义边界（用户定调）：**只负责重启，不负责启动**——没在运行的 target 由目标解析
//! （[`super::target::select`]）报告，这里绝不顺手拉起。
//!
//! **先预检、后停止**：restart 的主要用途是「改完 config.toml 让它生效」，配置笔误恰恰
//! 最常出现在这一步。若先停旧实例、再由新实例去读配置，配置有错时新实例秒死，一个健康的
//! 服务就此中断。因此停止之前先按新实例将走的同一条解析路径干跑一遍，换端口时再探测
//! 新端口可否绑定；预检不通过就不碰旧实例。
//!
//! **记录留着**：停止用的是带 `keep_restore` 的 shutdown，期望状态记录从头到尾都在。
//! restart 在「旧实例已停、新实例未就绪」的窗口里失败或崩溃，看护者与 `aproxy restore`
//! 仍能按这份记录把实例拉回来。代价是看护者可能抢先拉起它：两边用的是同一份规格，新实例
//! 发现端口已被同一配置的实例占着时，按成功处理。

use std::path::{Path, PathBuf};

use crate::daemon::{self, InstanceRecord, InstanceStatus};
use crate::spawn::{self, SpawnError};
use crate::spec::{self, InstanceSpec};

use super::start::resolve_runtime_config;
use super::stop::{Intent, StopFailure, StopMode, Stopped, stop_with_intent};

/// 重启成功。
#[derive(Debug, Clone)]
pub struct Restarted {
    /// 旧实例是怎样停下的
    pub stopped: Stopped,
    /// 旧实例的 pid
    pub old_pid: u32,
    /// 新实例就绪时的状态。监听地址以它为准：配置改了端口时与旧实例不同
    pub live: InstanceStatus,
}

/// 重启失败所处的阶段——决定旧实例此刻是否还在运行（用户最关心的是「服务还在不在」）。
#[derive(Debug)]
pub enum RestartFailure {
    /// 这个实例没有期望状态记录，没有可用来重新拉起它的规格：前台实例（它的日志在控制台，
    /// 重启要在那个终端里做），或记录已被删除。旧实例未被触碰
    Unsupported,
    /// 预检未通过：旧实例未被触碰，保持运行
    Precheck(PrecheckError),
    /// 停止旧实例未确认：旧实例可能仍在运行，未启动新实例（不冒险叠加启动）
    Stop(StopFailure),
    /// 旧实例已停止，新实例未能启动或就绪：该端口的服务已中断。期望状态记录还在，看护者
    /// 会继续按它重拉，排除原因后执行 `aproxy restore` 也能拉回
    Start {
        stopped: Stopped,
        error: SpawnError,
        /// 实例的配置文件；没有配置文件的实例为 None
        config_path: Option<PathBuf>,
    },
}

/// 预检不通过的原因。
#[derive(Debug)]
pub enum PrecheckError {
    /// 期望状态记录读不懂（损坏，或由更新的版本写下）。`reason` 是给人看的原因
    RecordUnreadable { reason: String },
    /// 配置文件已不存在
    ConfigMissing { path: PathBuf },
    /// 配置解析或校验不通过。`reason` 是给人看的完整说明
    Config { reason: String },
    /// 新配置的监听端口上已有另一个 aProxy 实例在运行
    PortTaken { port: String, pid: u32 },
    /// 新配置的监听地址绑定不了
    Bind {
        listen_addr: String,
        error: std::io::Error,
    },
}

/// 重启单个实例：读期望状态记录里的拉起规格 → 预检 → 停止 → 拉起 → 就绪判定。`exe` 是
/// 用来拉起新实例的二进制。
///
/// 就绪的判定按新 pid 在注册表里定位（见 [`crate::spawn`]）：restart 的用途之一就是「改了
/// config.toml（含换端口）后让新配置生效」，新实例监听哪个端口由它读到的配置决定（实测
/// 踩坑：改端口重启误报「未就绪」而实例已在新端口运行）。新实例提前退出时立即报告，不空等
/// 到时限。
pub async fn restart_instance(
    info: &InstanceRecord,
    mode: StopMode,
    exe: &Path,
) -> Result<Restarted, RestartFailure> {
    let port = daemon::port_of(&info.listen_addr).to_string();
    let run_dir = daemon::run_dir();
    let spec = match spec::read_restore_in(&run_dir, &port) {
        Ok(Some(entry)) => entry.spec,
        Ok(None) => return Err(RestartFailure::Unsupported),
        Err(reason) => {
            return Err(RestartFailure::Precheck(PrecheckError::RecordUnreadable {
                reason: reason.to_string(),
            }));
        }
    };

    let new_addr = precheck(info, &port, &spec)
        .await
        .map_err(RestartFailure::Precheck)?;

    let stopped = stop_with_intent(info, mode, Intent::Restart)
        .await
        .map_err(RestartFailure::Stop)?;

    let home = aproxy_ipc::home::home();
    match spawn::spawn_from_spec(&home, exe, &spec, spawn::READY_TIMEOUT).await {
        Ok(live) => {
            // 配置改了端口：新实例按新端口写了自己的记录，旧端口那份被它接替
            let new_port = daemon::port_of(&live.instance.listen_addr).to_string();
            daemon::retire_moved_port_records_in(&run_dir, &port, &new_port);
            Ok(Restarted {
                stopped,
                old_pid: info.pid,
                live,
            })
        }
        Err(error) => {
            // 看护者抢先按同一份记录把它拉起来了：我们的子进程绑不上端口而退出，但服务
            // 已在，用的也是同一份规格
            if let Some(live) = raced_by_watchdog(&new_addr, &spec, info.pid).await {
                return Ok(Restarted {
                    stopped,
                    old_pid: info.pid,
                    live,
                });
            }
            Err(RestartFailure::Start {
                stopped,
                error,
                config_path: spec.config_file(),
            })
        }
    }
}

/// 新实例没起来时，看它要监听的端口上是不是已经有了一个用同一配置的新实例。
async fn raced_by_watchdog(
    new_addr: &str,
    spec: &InstanceSpec,
    old_pid: u32,
) -> Option<InstanceStatus> {
    let port = daemon::port_of(new_addr);
    if port == "0" {
        return None;
    }
    let live = daemon::ipc_ping(port).await.ok()?;
    // 两边都没有配置文件也算同一配置：同一份规格拉起的就是这样的实例
    let key = |path: &Option<String>| path.as_deref().map(super::target::config_path_key);
    let same_config = key(&live.instance.config_path) == key(&spec.config_path);
    (live.instance.pid != old_pid && same_config).then_some(live)
}

/// 停止旧实例之前的干跑预检：用新实例将走的同一条路径解析配置（规格里的覆盖 →
/// [`resolve_runtime_config`]：严格解析、settings 的全局默认、校验），监听地址变了再探测
/// 新地址可否绑定。只读，无副作用（bind 探测的 listener 随即释放）。通过返回新实例将
/// 监听的地址（端口为 0、由系统分配时，规格里钉过端口就是那个端口）。
async fn precheck(
    info: &InstanceRecord,
    old_port: &str,
    spec: &InstanceSpec,
) -> Result<String, PrecheckError> {
    let cfg_path = spec.config_file();
    if let Some(path) = &cfg_path
        && !path.is_file()
    {
        return Err(PrecheckError::ConfigMissing { path: path.clone() });
    }
    let cfg = resolve_runtime_config(&spec.overrides, cfg_path.as_deref())
        .map_err(|reason| PrecheckError::Config { reason })?;
    // 配置解析出的端口仍是 0 时，新实例回到规格里钉住的端口
    let mut new_addr = cfg.listen_addr.clone();
    if daemon::port_of(&new_addr) == "0"
        && let Some(pinned) = spec.pinned_port
        && let Some((host, _)) = new_addr.rsplit_once(':')
    {
        new_addr = format!("{host}:{pinned}");
    }
    // 监听地址探测：只在地址变了时做（同地址必然被旧实例自己占着，stop 后
    // 才释放，此刻无从探测）。端口 0 由系统分配，无需探测
    let new_port = daemon::port_of(&new_addr).to_string();
    if new_addr != info.listen_addr && new_port != "0" {
        // 换到的端口上已有 aProxy 实例：新实例 bind 必败（IPC 探测不触碰代理端口）
        if new_port != old_port
            && let Ok(other) = daemon::ipc_ping(&new_port).await
        {
            return Err(PrecheckError::PortTaken {
                port: new_port,
                pid: other.instance.pid,
            });
        }
        if let Err(error) = tokio::net::TcpListener::bind(&new_addr).await
            // 同端口只换地址（如 127.0.0.1 → 0.0.0.0）时「已占用」可能正是旧
            // 实例自己，stop 后即释放，不据此拦截；其他错误（地址不可用、权限
            // 不足/系统保留端口）与旧实例无关，照拦
            && !(new_port == old_port && error.kind() == std::io::ErrorKind::AddrInUse)
        {
            return Err(PrecheckError::Bind {
                listen_addr: new_addr,
                error,
            });
        }
    }
    Ok(new_addr)
}
