//! 与 0.1.0 互通的兼容层：只为「0.1.0 → 新版本」的原地升级存在，0.1.x 线保留，
//! 0.2.0 删除整个模块（届时在 CHANGELOG 写明升级下限）。
//!
//! 为什么需要：用户用 0.1.0 的 `aproxy install` 升级时，有一段时间新旧两边同时
//! 在跑——0.1.0 的安装器、留在 PATH 上的 0.1.0 CLI 要控制新实例，新版本的
//! `install --continue` 要停掉、确认还在跑 0.1.0 的实例。两边的差别有两处：
//! - 端点名：0.1.0 的 Windows 控制管道名只含端口（`\\.\pipe\aproxy-<端口>`），
//!   新版本按 run 目录加了命名空间（见 `daemon::endpoint_for_in`）。新守护在旧名
//!   字上也应答（S1），否则 0.1.0 一侧找不到新实例，它的注册表普查还会把新实例
//!   的记录当死记录删掉；新客户端在命名空间端点找不到实例时，按严格条件回退到
//!   旧名字（S2）。unix 的 socket 一直在 run 目录里，路径没变，不需要这一层。
//! - 线上格式：0.1.0 的请求与应答没有 `v` 字段，应答是 `{ok, info, proto}`。
//!   守护对没有 `v` 的请求回 0.1.0 形状的应答（S3）；客户端把没有 `v` 的应答
//!   按 0.1.0 形状解析（S4）。新客户端发出的 v1 请求，0.1.0 的守护照样认得
//!   （它按 `op` 解析、忽略多出的 `v`），所以客户端不需要另一套请求格式。
//!
//! 完整分析见 .agents/plan/ipc-v1.md 的 (c) 节。

use crate::daemon::{
    Activity, InstanceRecord, InstanceState, InstanceStatus, IpcError, IpcOp, LastError,
};

/// 0.1.0 的 IPC 协议号（它的应答里 `proto` 字段的值）
const LEGACY_PROTO: u32 = 2;

/// 0.1.0 的应答载荷（当年 `.pid` 文件也是这个结构）。字段与 serde 属性照抄
/// v0.1.0，不要随新版本修改。
#[derive(serde::Serialize, serde::Deserialize)]
struct LegacyInfo {
    pid: u32,
    version: String,
    listen_addr: String,
    config_path: String,
    base_url: String,
    started_at: u64,
    #[serde(default)]
    last_activity_secs: u64,
    #[serde(default)]
    proto_version: u32,
    #[serde(default)]
    requests_total: u64,
    #[serde(default)]
    retries_total: u64,
    #[serde(default)]
    last_error: Option<String>,
    #[serde(default)]
    last_error_at: u64,
    #[serde(default)]
    swap_phase: bool,
    #[serde(default)]
    log_path: String,
    #[serde(default)]
    process_start: u64,
}

/// 0.1.0 的应答
#[derive(serde::Serialize, serde::Deserialize)]
struct LegacyResponse {
    ok: bool,
    #[serde(default)]
    info: Option<LegacyInfo>,
    #[serde(default)]
    proto: u32,
}

/// S3：没有 `v` 的请求来自 0.1.0 的 CLI 或安装器。0.1.0 的 `stats` 与 `ping`
/// 是同一回事。认不出的 op 返回 None（调用方回 `legacy_reply(None)`）。
pub(crate) fn legacy_op(request: &serde_json::Value) -> Option<IpcOp> {
    match request.get("op")?.as_str()? {
        "ping" | "stats" => Some(IpcOp::Ping),
        "shutdown" => Some(IpcOp::Shutdown),
        "prepare_swap" => Some(IpcOp::PrepareSwap),
        _ => None,
    }
}

/// S3：给 0.1.0 客户端的应答行。None = 拒绝（0.1.0 的拒绝不带原因）。
pub(crate) fn legacy_reply(status: Option<&InstanceStatus>) -> String {
    let info = status.map(|s| {
        let record = &s.instance;
        LegacyInfo {
            pid: record.pid,
            version: record.version.clone(),
            listen_addr: record.listen_addr.clone(),
            config_path: record.config_path.clone(),
            base_url: record.base_url.clone(),
            started_at: record.started_at,
            last_activity_secs: s.activity.last_request_at,
            proto_version: LEGACY_PROTO,
            requests_total: s.activity.requests_total,
            retries_total: s.activity.retries_total,
            last_error: s.activity.last_error.as_ref().map(|e| e.message.clone()),
            last_error_at: s.activity.last_error.as_ref().map_or(0, |e| e.at),
            // 0.1.0 的安装器以 swap_phase == true 判定 ACK
            swap_phase: s.state == InstanceState::SwapPrepared,
            log_path: record.log_path.clone(),
            process_start: record.process_start,
        }
    });
    let reply = LegacyResponse {
        ok: info.is_some(),
        info,
        proto: LEGACY_PROTO,
    };
    serde_json::to_string(&reply).expect("序列化 0.1.0 应答失败")
}

/// S4：没有 `v` 的应答来自 0.1.0 的守护，映射成 v1 的状态快照。
pub(crate) fn parse_legacy_reply(reply: serde_json::Value) -> Result<InstanceStatus, IpcError> {
    let reply: LegacyResponse = serde_json::from_value(reply)
        .map_err(|e| IpcError::Protocol(format!("应答格式不符：{e}")))?;
    let info = match (reply.ok, reply.info) {
        (true, Some(info)) => info,
        _ => {
            return Err(IpcError::Remote {
                code: "legacy_rejected".to_string(),
                message: "0.1.0 的实例拒绝了请求（它的应答不带原因）".to_string(),
            });
        }
    };
    Ok(InstanceStatus {
        instance: InstanceRecord {
            pid: info.pid,
            process_start: info.process_start,
            version: info.version,
            listen_addr: info.listen_addr,
            config_path: info.config_path,
            base_url: info.base_url,
            started_at: info.started_at,
            log_path: info.log_path,
        },
        // 0.1.0 的应答不报 run 目录
        run_dir: String::new(),
        state: if info.swap_phase {
            InstanceState::SwapPrepared
        } else {
            InstanceState::Serving
        },
        activity: Activity {
            last_request_at: info.last_activity_secs.max(info.started_at),
            requests_total: info.requests_total,
            retries_total: info.retries_total,
            last_error: info.last_error.map(|message| LastError {
                message,
                at: info.last_error_at,
            }),
        },
        ops: [IpcOp::Ping, IpcOp::Shutdown, IpcOp::PrepareSwap]
            .iter()
            .map(|op| op.name().to_string())
            .collect(),
    })
}

/// 0.1.0 的控制管道名（只含端口，全机共享）
#[cfg(windows)]
fn legacy_pipe(port: &str) -> String {
    format!(r"\\.\pipe\aproxy-{port}")
}

/// S1：新守护在 0.1.0 的管道名上也提供控制服务（尽力而为）。名字已被占用
/// （例如别的 home 里同端口的 0.1.0 实例）时只记一条 warn，不影响本实例。
#[cfg(windows)]
pub(crate) fn serve_legacy_pipe(port: &str, state: std::sync::Arc<crate::daemon::ControlState>) {
    let endpoint = legacy_pipe(port);
    tokio::spawn(async move {
        if let Err(e) = crate::daemon::serve_endpoint(endpoint, state).await {
            tracing::warn!(
                error = %e,
                "0.1.0 兼容控制管道创建失败：不影响本实例，只是 0.1.0 的 CLI 与安装器找不到它"
            );
        }
    });
}

/// S2：命名空间端点上找不到实例时，本 run 目录若登记了该端口的实例，试 0.1.0
/// 的管道名；应答者自报的 pid 与进程创建时间都和记录一致才返回该端点。
/// 0.1.0 在 Windows 上总会登记创建时间；记录里是 0 就无从核验，不认——与
/// 看门狗、stop --force 对「0 = 无法核验」的处理一致。应答者自报的 pid 已由
/// `request_raw` 与管道对端进程核对过。
#[cfg(windows)]
pub(crate) async fn legacy_endpoint_for(run_dir: &std::path::Path, port: &str) -> Option<String> {
    let record = crate::daemon::read_instance_file_in(run_dir, port)?;
    let endpoint = legacy_pipe(port);
    let live = crate::daemon::request_raw(&endpoint, IpcOp::Ping)
        .await
        .ok()?
        .instance;
    let same_process = record.process_start != 0
        && live.pid == record.pid
        && live.process_start == record.process_start;
    same_process.then_some(endpoint)
}

/// unix 的 socket 路径与 0.1.0 相同，不需要回退
#[cfg(unix)]
pub(crate) async fn legacy_endpoint_for(_run_dir: &std::path::Path, _port: &str) -> Option<String> {
    None
}
