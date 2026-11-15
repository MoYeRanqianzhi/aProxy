//! 重启一个运行中的实例：预检 → 停止 → 用原启动参数拉起 → 等就绪。
//!
//! 语义边界（用户定调）：**只负责重启，不负责启动**——没在运行的 target 由目标解析
//! （[`super::target::select`]）报告，这里绝不顺手拉起。
//!
//! **先预检、后停止**：restart 的主要用途是「改完 config.toml 让它生效」，配置笔误恰恰
//! 最常出现在这一步。若先停旧实例、再由新守护去读配置，配置有错时新守护秒死，而旧实例
//! 的 .restore 已随优雅退出删除——看门狗与 `aproxy restore` 都无从恢复，一个健康的服务
//! 就此下线。因此停止之前先按新守护将走的同一条解析路径干跑一遍，换端口时再探测新端口
//! 可否绑定；预检不通过就不碰旧实例。

use std::path::{Path, PathBuf};

use clap::Parser;

use crate::cli::Cli;
use crate::daemon::{self, InstanceRecord, InstanceStatus};
use crate::spawn::{self, SpawnError};

use super::start::resolve_runtime_config;
use super::stop::{StopFailure, StopMode, Stopped, stop_instance};

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
    /// 预检未通过：旧实例未被触碰，保持运行
    Precheck(PrecheckError),
    /// 停止旧实例未确认：旧实例可能仍在运行，未启动新实例（不冒险叠加启动）
    Stop(StopFailure),
    /// 旧实例已停止，新实例未能启动或就绪：该端口的服务已中断。旧实例的 .restore 已随
    /// 停止删除（优雅退出时守护自删，强杀时在终止前摘掉），看门狗与 `aproxy restore` 都
    /// 不会自动拉起它，要由用户修正后用 `config_path` 手动启动
    Start {
        stopped: Stopped,
        error: SpawnError,
        config_path: PathBuf,
    },
}

/// 预检不通过的原因。
#[derive(Debug)]
pub enum PrecheckError {
    /// 原启动参数无法被当前版本解析（例如旧版本留下的参数本版本已不认识）。`reason` 是
    /// 解析器的报错，不含参数本身——其中可能有 `--api-key` 明文
    ArgsUnparsable { reason: String },
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

/// 重启单个实例：捕获原始启动参数 → 预检 → 停止 → 拉起 → 就绪判定。`exe` 是用来拉起
/// 新实例的二进制。
///
/// 就绪的判定按新 pid 在注册表里定位（见 [`crate::spawn`]）：restart 的用途之一就是「改了
/// config.toml（含换端口）后让新配置生效」，新守护监听哪个端口由它读到的配置决定（实测
/// 踩坑：改端口重启误报「未就绪」而实例已在新端口运行）；顺带覆盖 --listen 0。新实例提前
/// 退出时立即报告，不空等到时限。
pub async fn restart_instance(
    info: &InstanceRecord,
    mode: StopMode,
    exe: &Path,
) -> Result<Restarted, RestartFailure> {
    let port = daemon::port_of(&info.listen_addr).to_string();

    // 原始启动参数在 .restore（守护 bind 时写入、优雅退出即删）——必须在
    // stop 之前捕获，否则 --api-key/--listen 等仅本次生效的参数会丢失。
    // .restore 缺失（记录丢失或损坏后被清理）回退到注册表的 config_path。
    let args: Vec<String> = daemon::list_restore_entries()
        .into_iter()
        .find(|e| e.port == port)
        .map(|e| e.args)
        .unwrap_or_else(|| vec!["--config".to_string(), info.config_path.clone()]);

    let config_path = precheck(info, &port, &args)
        .await
        .map_err(RestartFailure::Precheck)?;

    let stopped = stop_instance(info, mode)
        .await
        .map_err(RestartFailure::Stop)?;

    match spawn::spawn_instance(&daemon::run_dir(), exe, &args, spawn::READY_TIMEOUT).await {
        Ok(live) => Ok(Restarted {
            stopped,
            old_pid: info.pid,
            live,
        }),
        Err(error) => Err(RestartFailure::Start {
            stopped,
            error,
            config_path,
        }),
    }
}

/// 停止旧实例之前的干跑预检：用新守护将走的同一条路径解析配置（原启动参数 → clap →
/// [`resolve_runtime_config`]：严格 TOML 加载、CLI 覆盖、settings 注入、validate），监听
/// 地址变了再探测新地址可否绑定。只读，无副作用（bind 探测的 listener 随即释放）。通过
/// 返回新守护将使用的配置文件路径。
async fn precheck(
    info: &InstanceRecord,
    old_port: &str,
    args: &[String],
) -> Result<PathBuf, PrecheckError> {
    // 原参数就是守护当初的命令行（去掉 --daemon-child），新守护会照原样再
    // 解析一次——这里解析失败（例如旧版本留下的参数本版本已不认识），新守护
    // 必然同样秒死。错误信息不回显参数本身：其中可能含 --api-key 明文
    let cli =
        Cli::try_parse_from(std::iter::once("aproxy".to_string()).chain(args.iter().cloned()))
            .map_err(|e| PrecheckError::ArgsUnparsable {
                reason: e.to_string().trim().to_string(),
            })?;
    // 守护的配置路径：原参数里的 --config（start 总会注入绝对路径）；缺失时
    // 取注册表里该守护实际加载的路径
    let cfg_path = cli
        .config
        .clone()
        .unwrap_or_else(|| PathBuf::from(&info.config_path));
    if !cfg_path.is_file() {
        return Err(PrecheckError::ConfigMissing { path: cfg_path });
    }
    let cfg = resolve_runtime_config(&cli, &cfg_path)
        .map_err(|reason| PrecheckError::Config { reason })?;

    // 监听地址探测：只在地址变了时做（同地址必然被旧实例自己占着，stop 后
    // 才释放，此刻无从探测）。端口 0 由系统分配，无需探测
    let new_addr = cfg.listen_addr.as_str();
    let new_port = daemon::port_of(new_addr);
    if new_addr != info.listen_addr && new_port != "0" {
        // 换到的端口上已有 aProxy 实例：新守护 bind 必败（IPC 探测不触碰代理端口）
        if new_port != old_port
            && let Ok(other) = daemon::ipc_ping(new_port).await
        {
            return Err(PrecheckError::PortTaken {
                port: new_port.to_string(),
                pid: other.instance.pid,
            });
        }
        if let Err(error) = tokio::net::TcpListener::bind(new_addr).await
            // 同端口只换地址（如 127.0.0.1 → 0.0.0.0）时「已占用」可能正是旧
            // 实例自己，stop 后即释放，不据此拦截；其他错误（地址不可用、权限
            // 不足/系统保留端口）与旧实例无关，照拦
            && !(new_port == old_port && error.kind() == std::io::ErrorKind::AddrInUse)
        {
            return Err(PrecheckError::Bind {
                listen_addr: new_addr.to_string(),
                error,
            });
        }
    }
    Ok(cfg_path)
}
