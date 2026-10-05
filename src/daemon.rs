//! 守护进程编排：后台启动、实例注册表、IPC 控制。
//!
//! 进程模型：`aproxy`（默认）= 后台启动——父进程做预检与就绪等待，
//! 实际服务由分离的子进程（`--daemon-child`）承载。子进程以 CREATE_NO_WINDOW
//! 启动、不继承控制台，终端关闭/常规内存清理不会连带杀掉守护进程。
//!
//! IPC（铁律：控制通道绝不占用代理端口，代理端口完全用于透传——避免控制
//! 路径与客户端请求路径巧合重叠造成严重 bug）：每个实例一条 Windows 命名
//! 管道 `\\.\pipe\aproxy-<home_id>-<port>`（unix 为 `<run 目录>/<port>.sock`），
//! 承载 `ping`/`shutdown` 等控制。端点按 run 目录划分命名空间（见
//! [`endpoint_for_in`]）：一个 home 的命令只能控制本 home 的实例，测试 home
//! 里的 `aproxy stop <端口>` 碰不到用户正在用的同端口实例。端口冲突的两种
//! 情况由此区分：
//! - 管道 ping 通 → 该端口已有 aProxy 实例在运行；
//! - 管道不通但 TCP bind 失败 → 端口被其他程序占用。
//!
//! 实例注册表：`~/.aproxy/run/<port>.pid`（JSON），仅供 `status`/`stop` 枚举
//! 实例；存活一律以 IPC ping 为准，不信任注册文件与 pid 本身。

use serde::{Deserialize, Serialize};
use std::{io, path::PathBuf, sync::Arc, time::Duration};

/// 实例注册表目录：`<home>/run/`。`APROXY_RUN_DIR` 环境变量可整体改指
/// 别处（粒度优先于 APROXY_HOME 派生）——集成测试用它与 tempdir 隔离
/// （守护/看护子进程经 spawn_detached 继承环境），高级用户亦可借此自定义
/// 运行数据位置。
pub fn run_dir() -> PathBuf {
    if let Ok(d) = std::env::var("APROXY_RUN_DIR") {
        return PathBuf::from(d);
    }
    crate::settings::home().join("run")
}

/// 守护进程日志目录：`<home>/logs/`
pub fn logs_dir() -> PathBuf {
    crate::settings::home().join("logs")
}

/// spool 临时文件目录基址：`<home>/spool/<端口>/`。每实例独立子目录，
/// 启动时清空自己的子目录即可回收崩溃残留，互不干扰。
pub fn spool_dir_for(port: &str) -> PathBuf {
    crate::settings::home().join("spool").join(port)
}

/// 清空实例的 spool 目录（启动时调用）：删除崩溃/强杀残留的 *.spooltmp。
/// 目录不存在视为首次运行（创建之）。仅在 bind 监听端口之前调用——此后
/// 该端口目录归本进程独占，运行中不清理（清理职责在临时文件的使用方）。
pub fn clean_spool_dir(port: &str) {
    let dir = spool_dir_for(port);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::warn!(error = %e, dir = %dir.display(), "spool 目录创建失败（磁盘缓存将不可用，回退纯内存）");
        return;
    }
    match std::fs::read_dir(&dir) {
        Ok(entries) => {
            for entry in entries.flatten() {
                // 只删本实例的 spool 临时文件；意外出现的其他内容保留（不扩大删除面）
                if entry
                    .path()
                    .extension()
                    .is_some_and(|ext| ext == "spooltmp")
                    && let Err(e) = std::fs::remove_file(entry.path())
                {
                    tracing::warn!(error = %e, path = %entry.path().display(), "spool 残留清理失败");
                }
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, dir = %dir.display(), "spool 目录读取失败");
        }
    }
}

/// 实例的 IPC 端点：windows 为命名管道名，unix 为 UDS 路径。
/// 端口号在一个 run 目录内唯一区分实例（同端口=同实例）。
pub fn endpoint_for(port: &str) -> String {
    endpoint_for_in(&run_dir(), port)
}

/// 同 endpoint_for，但按**显式 run_dir** 派生。库层函数（install 的
/// flow/restart、看门狗）收了 run_dir 参数就必须全程用它——若内部回落到
/// 进程级 APROXY_HOME 派生，测试进程与多 home 场景下会找到别的 home 的端点。
///
/// 端点属于 run 目录：unix 的 socket 本来就在 run 目录里；Windows 的命名管道
/// 是全机共享的名字空间，所以名字里带上 run 目录的标识（`home_id`）。否则
/// 隔离 home（APROXY_HOME）里的 `aproxy stop <端口>` 会停掉另一个 home 在同一
/// 端口上的实例——例如用临时 home 做实验时停掉用户正在用的生产实例。
pub fn endpoint_for_in(run_dir: &std::path::Path, port: &str) -> String {
    #[cfg(windows)]
    {
        format!(r"\\.\pipe\aproxy-{}-{port}", home_id(run_dir))
    }
    #[cfg(unix)]
    {
        run_dir.join(format!("{port}.sock")).display().to_string()
    }
}

/// run 目录的标识：规范化路径的 SHA-256 前 16 个十六进制字符。Windows 的
/// 全机名字（控制管道）靠它区分 home。规范化让同一目录的不同写法（大小写、
/// 分隔符、相对路径、subst 盘符、目录联接）得到同一个标识。
///
/// 目录还不存在时规范化「最近的已存在祖先」再接上其余部分：守护可能在 run
/// 目录建出来之前就算端点名（注册表写失败时），客户端在目录存在后再算——
/// 两边若一个走规范化、一个走字面绝对路径，经联接或 subst 访问的 home 会
/// 得到两个不同的标识，客户端就再也找不到这个实例。
pub fn home_id(run_dir: &std::path::Path) -> String {
    use sha2::{Digest, Sha256};
    let absolute = std::path::absolute(run_dir).unwrap_or_else(|_| run_dir.to_path_buf());
    let mut resolved = None;
    for ancestor in absolute.ancestors() {
        if let Ok(real) = std::fs::canonicalize(ancestor) {
            // ancestors() 产出的都是 absolute 的前缀，strip_prefix 必然成功
            let rest = absolute
                .strip_prefix(ancestor)
                .unwrap_or(std::path::Path::new(""));
            resolved = Some(if rest.as_os_str().is_empty() {
                real
            } else {
                real.join(rest)
            });
            break;
        }
    }
    let text = resolved.unwrap_or(absolute).display().to_string();
    #[cfg(windows)]
    let text = {
        let text = text.strip_prefix(r"\\?\").unwrap_or(&text);
        text.replace('/', r"\").to_lowercase()
    };
    let digest = Sha256::digest(text.as_bytes());
    digest[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// 从监听地址提取端口（IPC 端点名、日志/注册文件名共用）
pub fn port_of(listen_addr: &str) -> &str {
    listen_addr.rsplit(':').next().unwrap_or("unknown")
}

/// 实例的注册记录：`run/<端口>.pid` 的内容，也是 IPC 状态快照里的 `instance`。
///
/// 字段名与 0.1.0 写下的逐字一致，且全部必填：0.1.0 在原地升级途中会读新版本
/// 的记录（它要求 pid/version/listen_addr/config_path/base_url/started_at 都在，
/// 解析不了就把文件删掉），新版本也直接读 0.1.0 的记录（0.1.0 写的字段是这里的
/// 超集，多出来的被忽略）。改名、删字段或新增必填字段都会破坏其中一个方向，
/// 见 tests/compat_v0_1_0.rs；新字段只能是可缺省的。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct InstanceRecord {
    pub pid: u32,
    /// 守护进程自身的创建时间戳：守护注册时自查自写（Windows 为
    /// GetProcessTimes 的 FILETIME，100ns；Linux 为 /proc/<pid>/stat 的
    /// starttime，时钟滴答），只在同平台内与实测值比对，是不透明的身份锚点。
    ///
    /// 进程身份 = 「这个 pid 现在是不是写下这条记录的那个守护」，与二进制
    /// 叫什么无关：pid 被系统回收再分配给任何进程后，新进程的创建时间必然
    /// 不同。看门狗的收养/处决/选举与 `stop --force` 都以「pid + 本字段」
    /// 比对防 pid 复用误杀（见 watchdog::record_identity）。0 = 平台读不到
    /// 创建时间（无 /proc 的 unix），这样的记录无法核验身份，一律不据它动手。
    pub process_start: u64,
    pub version: String,
    pub listen_addr: String,
    pub config_path: String,
    /// 脱敏后的上游地址，只用于展示
    pub base_url: String,
    /// 启动时刻（Unix 秒）
    pub started_at: u64,
    /// 本实例守护日志文件的绝对路径（随机命名或用户自定义 log_file）。
    /// 客户端（aproxy logs / start 成功提示）一律向实例或注册表索取，不按端口
    /// 拼路径——端口是易变标识。空串 = 前台实例（日志走控制台，无文件）。
    pub log_path: String,
}

/// IPC 状态快照：v1 每个成功应答的 `result`，所有 op 都回它。
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct InstanceStatus {
    pub instance: InstanceRecord,
    /// 实例所属的 run 目录。端口被另一个 home 的实例占着时，据此认出是谁
    pub run_dir: String,
    pub state: InstanceState,
    pub activity: Activity,
    /// 实例支持的 op。调用较新的、有副作用的 op 之前先查这里
    pub ops: Vec<String>,
}

/// 实例此刻所处的阶段。
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InstanceState {
    Serving,
    /// install 广播 prepare_swap 之后、重启之前（内存态，重启自然清除）。
    /// 安装器的 ACK 判定就是读到这个状态
    SwapPrepared,
    /// 已收到 shutdown，正在优雅退出
    Stopping,
    /// 更新版本的实例报出、本版本不认识的状态：按「不在正常服务」对待
    #[serde(other)]
    Unknown,
}

/// 实例的实时观测值（应答时现读）。
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Activity {
    /// 最近一次收到客户端请求的时刻（Unix 秒）；还没收到过请求时为启动时刻
    pub last_request_at: u64,
    /// 累计收到的客户端请求数
    pub requests_total: u64,
    /// 累计上游重试次数（首轮之后的所有尝试）
    pub retries_total: u64,
    /// 最近一次上游失败；从未失败为 null
    pub last_error: Option<LastError>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct LastError {
    /// 已脱敏的简短摘要
    pub message: String,
    /// 发生时刻（Unix 秒）
    pub at: u64,
}

/// 控制协议版本，请求与应答都带它（字段 `v`）。只有语义不兼容的变更才递增；
/// 加 op、加可选参数、加应答字段、加错误码、加状态值都不递增——双方都忽略
/// 不认识的字段，不认识的状态读作 `Unknown`。0.1.0 的格式没有 `v` 字段，
/// 由 compat_0_1_0 单独应对。
pub const IPC_VERSION: u32 = 1;

/// 控制操作。线上是一行 JSON：`{"v":1,"op":"<snake_case 名>"}`，可带
/// `args` 对象（现有 op 都没有参数）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IpcOp {
    /// 读状态快照（status、start 预检、就绪等待）
    Ping,
    /// 优雅停止：应答先发出，随后开始退出
    Shutdown,
    /// install 广播：进入二进制更换阶段。实例把它写进可观测的状态
    /// （`swap_prepared`），应答里立即可见——表达的是「进入阶段」而非口头 ok
    PrepareSwap,
}

impl IpcOp {
    const ALL: [IpcOp; 3] = [IpcOp::Ping, IpcOp::Shutdown, IpcOp::PrepareSwap];

    pub fn name(self) -> &'static str {
        match self {
            IpcOp::Ping => "ping",
            IpcOp::Shutdown => "shutdown",
            IpcOp::PrepareSwap => "prepare_swap",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|op| op.name() == name)
    }

    /// 发往实例的请求行（不含换行）
    pub fn request_line(self) -> String {
        serde_json::json!({ "v": IPC_VERSION, "op": self.name() }).to_string()
    }
}

/// v1 应答的线上形状。成功带 `result`，失败带 `error`。
#[derive(Serialize, Deserialize)]
struct WireResponse {
    v: u32,
    ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    result: Option<InstanceStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<WireError>,
}

/// 失败应答。`code` 是稳定的 snake_case 字符串，供程序判断；`message` 只给人看，
/// 不要解析它。
#[derive(Serialize, Deserialize)]
struct WireError {
    code: String,
    message: String,
    /// `unsupported_version` 时列出实例支持的版本
    #[serde(default, skip_serializing_if = "Option::is_none")]
    supported: Option<Vec<u32>>,
}

/// 错误码。客户端遇到不认识的码按一般的「实例拒绝」处理。
pub mod error_code {
    /// 不是 JSON、缺 `v`/`op`、行过长、`args` 不合法
    pub const BAD_REQUEST: &str = "bad_request";
    /// `v` 不是实例支持的版本（应答带 `supported`）
    pub const UNSUPPORTED_VERSION: &str = "unsupported_version";
    /// 实例没有这个 op（没有副作用，可以放心用来探测能力）
    pub const UNKNOWN_OP: &str = "unknown_op";
    /// op 此刻不允许（例如已在退出时收到 prepare_swap）
    pub const INVALID_STATE: &str = "invalid_state";
}

/// 一次 IPC 请求的失败，按「能据此下什么结论」分类。
#[derive(Debug)]
pub enum IpcError {
    /// 端点不存在：确定没有实例在这里（不必重试；stop 据此确认已退出）
    Unreachable(String),
    /// 管道忙、超时、读写出错：可能有实例，只是此刻没给出应答（挂死也是这样）
    Transient(String),
    /// 有应答但无法采信：解析不了、协议版本不符、自报 pid 与连接对端不符
    Protocol(String),
    /// 实例明确拒绝了请求
    Remote { code: String, message: String },
}

impl std::fmt::Display for IpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreachable(e) | Self::Transient(e) | Self::Protocol(e) => f.write_str(e),
            Self::Remote { code, message } => write!(f, "实例拒绝了请求（{code}）：{message}"),
        }
    }
}

impl std::error::Error for IpcError {}

/// 探测端口上是否有 aProxy 实例（纯 IPC，不触碰任何 TCP 端口）。
/// Ok = 实例在运行；Err = 端点上没有可识别的 aProxy（无实例，或该端口被
/// 其他程序占用——由调用方结合 TCP bind 结果区分这两种情况）。
/// 只找本进程 run 目录里的实例（见 `endpoint_for_in`）。
pub async fn ipc_ping(port: &str) -> Result<InstanceStatus, IpcError> {
    ipc_ping_in(&run_dir(), port).await
}

/// 同 ipc_ping，但按显式 run_dir 寻址（库层调用者用这个）。
///
/// 端点不存在、应答无法采信、实例明确拒绝都立即返回；只有 `Transient`
/// 连续 3 次（间隔 200ms）才算失败。单次 `Transient` 可能是 Windows 命名管道
/// 瞬时 busy（serve 重建监听实例的零监听窗口）或 3 秒超时，调用方据 Err
/// 下结论（注册表普查据它转入进程身份核验），单次抖动不该左右结论。
pub async fn ipc_ping_in(
    run_dir: &std::path::Path,
    port: &str,
) -> Result<InstanceStatus, IpcError> {
    const ATTEMPTS: usize = 3;
    const RETRY_INTERVAL: Duration = Duration::from_millis(200);
    let mut attempt = 0;
    loop {
        match request_in(run_dir, port, IpcOp::Ping).await {
            Err(IpcError::Transient(_)) if attempt + 1 < ATTEMPTS => {
                attempt += 1;
                tokio::time::sleep(RETRY_INTERVAL).await;
            }
            other => return other,
        }
    }
}

/// 发送一次 IPC 请求并等待应答（3 秒超时），找本进程 run 目录里的实例。
pub async fn ipc_request(port: &str, op: IpcOp) -> Result<InstanceStatus, IpcError> {
    ipc_request_in(&run_dir(), port, op).await
}

/// 同 ipc_request，按显式 run_dir 寻址。
pub async fn ipc_request_in(
    run_dir: &std::path::Path,
    port: &str,
    op: IpcOp,
) -> Result<InstanceStatus, IpcError> {
    request_in(run_dir, port, op).await
}

/// 按 run 目录与端口寻址：先找命名空间端点；那里没有实例时，按 0.1.0 兼容
/// 规则试旧端点（条件见 compat_0_1_0::legacy_endpoint_for）。
async fn request_in(
    run_dir: &std::path::Path,
    port: &str,
    op: IpcOp,
) -> Result<InstanceStatus, IpcError> {
    match request_raw(&endpoint_for_in(run_dir, port), op).await {
        Err(IpcError::Unreachable(e)) => {
            match crate::compat_0_1_0::legacy_endpoint_for(run_dir, port).await {
                Some(legacy) => request_raw(&legacy, op).await,
                None => Err(IpcError::Unreachable(e)),
            }
        }
        other => other,
    }
}

/// 按显式端点发送 IPC 请求并等待应答（3 秒超时）。按实例寻址请用
/// `ipc_request_in`：它负责端点命名空间与 0.1.0 兼容回退。
pub(crate) async fn request_raw(endpoint: &str, op: IpcOp) -> Result<InstanceStatus, IpcError> {
    let request = op.request_line();
    let fut = imp::exchange(endpoint, &request);
    let (line, peer) = match tokio::time::timeout(Duration::from_secs(3), fut).await {
        Ok(Ok(reply)) => reply,
        Ok(Err(IpcError::Unreachable(e))) => {
            return Err(IpcError::Unreachable(format!("{endpoint}: {e}")));
        }
        Ok(Err(IpcError::Transient(e))) => {
            return Err(IpcError::Transient(format!("{endpoint}: {e}")));
        }
        Ok(Err(e)) => return Err(e),
        Err(_) => return Err(IpcError::Transient(format!("{endpoint}: 请求超时"))),
    };
    let status = parse_reply(&line).map_err(|e| match e {
        IpcError::Protocol(m) => IpcError::Protocol(format!("{endpoint}: {m}")),
        other => other,
    })?;
    // 应答者自报的 pid 必须就是连接对端的进程：普查、0.1.0 兼容回退与 stop
    // 都按这个 pid 认实例，而端点名可以预测，任何本机进程都可能抢先占住它、
    // 冒充实例应答
    if let Some(peer) = peer
        && status.instance.pid != peer
    {
        return Err(IpcError::Protocol(format!(
            "{endpoint}: 应答者自报 pid {} 与连接对端进程 {peer} 不一致",
            status.instance.pid
        )));
    }
    Ok(status)
}

/// 解析一行应答。带 `v` 的是 v1；不带的来自 0.1.0 的守护（S4）。
fn parse_reply(line: &str) -> Result<InstanceStatus, IpcError> {
    let value: serde_json::Value = serde_json::from_str(line)
        .map_err(|e| IpcError::Protocol(format!("应答不是 JSON：{e}")))?;
    if value.get("v").is_none() {
        return crate::compat_0_1_0::parse_legacy_reply(value);
    }
    let reply: WireResponse = serde_json::from_value(value)
        .map_err(|e| IpcError::Protocol(format!("应答格式不符：{e}")))?;
    if reply.v != IPC_VERSION {
        return Err(IpcError::Protocol(format!(
            "实例使用控制协议 v{}，本程序只懂 v{IPC_VERSION}：请用与实例同版本的 aproxy 操作它",
            reply.v
        )));
    }
    match (reply.ok, reply.result, reply.error) {
        (true, Some(status), _) => Ok(status),
        (false, _, Some(error)) => Err(IpcError::Remote {
            code: error.code,
            message: error.message,
        }),
        _ => Err(IpcError::Protocol("应答缺少 result 或 error".to_string())),
    }
}

/// 本实例已独占创建、尚未开始接受连接的控制端点。守护先创建它、再写注册表
/// 与恢复记录（见 server::serve_forever）：有 `.pid` 记录就意味着端点在应答
/// （挂死除外）。创建失败时守护直接启动失败，不带着一个谁也控制不了的实例
/// 继续跑。
pub struct IpcEndpoint(imp::Listener);

/// 创建本实例的控制端点。端点已被占用（同一 home 同端口的另一个实例，或抢先
/// 占住名字的其他进程）时报错，不与对方分摊或抢夺。
pub async fn bind_ipc(port: &str) -> io::Result<IpcEndpoint> {
    imp::bind(&endpoint_for(port)).await.map(IpcEndpoint)
}

/// 控制服务应答所需的一切，所有连接共享一份。
pub struct ControlState {
    /// 本实例的注册记录（应答里的 `instance`）
    pub record: InstanceRecord,
    /// 本实例所属的 run 目录（应答里的 `run_dir`）
    pub run_dir: String,
    /// 代理层的实时观测源，应答时现读
    pub stats: Arc<IpcStats>,
    /// 收到 shutdown 时置位，服务主循环据此优雅退出；已置位即「正在退出」
    pub on_shutdown: tokio::sync::watch::Sender<bool>,
}

/// 在已创建的端点上提供控制服务，直到进程退出。
pub async fn serve_ipc(endpoint: IpcEndpoint, port: &str, state: Arc<ControlState>) {
    #[cfg(windows)]
    crate::compat_0_1_0::serve_legacy_pipe(port, state.clone());
    #[cfg(unix)]
    let _ = port;
    imp::accept_loop(endpoint.0, state).await
}

/// 在给定端点名上创建并提供控制服务（0.1.0 兼容管道与测试用）。只有创建
/// 失败会返回。
#[cfg(any(windows, test))]
pub(crate) async fn serve_endpoint(endpoint: String, state: Arc<ControlState>) -> io::Result<()> {
    let listener = imp::bind(&endpoint).await?;
    imp::accept_loop(listener, state).await;
    Ok(())
}

/// 实例的实时观测数据源：代理热路径写入，IPC 响应读取。
/// 全部 Relaxed——计数器只用于展示与告警，无跨线程因果序需求。
#[derive(Debug, Default)]
pub struct IpcStats {
    /// 最近一次收到客户端请求的时刻（即 AppState::last_activity_secs，
    /// 同一 Arc 由 serve_forever 组装进来）
    pub last_activity_secs: Arc<std::sync::atomic::AtomicU64>,
    /// 累计收到客户端请求数
    pub requests_total: std::sync::atomic::AtomicU64,
    /// 累计上游重试次数（首轮之后的所有尝试）
    pub retries_total: std::sync::atomic::AtomicU64,
    /// 最近一次上游失败的简短摘要（打码后的短串；None = 从未失败）
    pub last_error: std::sync::Mutex<Option<(String, u64)>>,
    /// 二进制更换阶段（PrepareSwap 广播后置位，重启自然清除）
    pub swap_phase: std::sync::atomic::AtomicBool,
}

impl IpcStats {
    /// 记录一次上游失败（覆盖式：只保留最近一次）。`error` 须已打码。
    pub fn record_error(&self, error: &str) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        // 截断到 200 字符：错误摘要进 IPC 响应与 status 展示，防止上游返回
        // 超长错误文本把响应行撑爆
        let mut brief: String = error.chars().take(200).collect();
        if brief.is_empty() {
            brief = "未知错误".to_string();
        }
        if let Ok(mut slot) = self.last_error.lock() {
            *slot = Some((brief, now));
        }
    }
}

/// 注册表中登记的全部实例记录（只读 *.pid 文件，不做 IPC 探活），按 pid
/// 升序。看门狗选举的输入：存活与身份由调用方用进程级手段（pid + 记录里
/// 的 process_start 比对实测创建时间）判定——实例 IPC 不可达恰恰是需要
/// 看护的信号，不能作为「死」的依据参与选举。
pub fn registry_instances_in(run_dir: &std::path::Path) -> Vec<InstanceRecord> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(run_dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("pid") {
            continue;
        }
        if let Ok(content) = std::fs::read_to_string(&path)
            && let Ok(info) = serde_json::from_str::<InstanceRecord>(&content)
        {
            out.push(info);
        }
    }
    out.sort_by_key(|i| i.pid);
    out
}

/// 注册表中登记的全部实例 PID（去重升序，只读文件不探活）。
pub fn registry_pids_in(run_dir: &std::path::Path) -> Vec<u32> {
    let mut out: Vec<u32> = registry_instances_in(run_dir)
        .iter()
        .map(|i| i.pid)
        .collect();
    out.dedup();
    out
}

/// 读取单个端口的注册表记录（只读，不探活；不存在/损坏 → None）。
pub fn read_instance_file_in(run_dir: &std::path::Path, port: &str) -> Option<InstanceRecord> {
    let content = std::fs::read_to_string(instance_file_path_in(run_dir, port)).ok()?;
    serde_json::from_str(&content).ok()
}

/// 强制终止实例进程（`--force`）：跳过 IPC 优雅关闭直接终止。`info` 必须
/// 是刚经 IPC ping 从该端口拿到的实例信息（stop 的 target 解析一律经 ping，
/// 应答者自报的 pid 即端点归属证明）。
///
/// 身份判定与二进制名称无关（改名运行的官方资产 `aproxy-<target>` 同样可
/// 强杀）：「pid + 创建时间」核验后才终止（Windows 在同一进程句柄上先核验
/// 后终止，中间不存在 pid 复用窗口）。不符 = pid 已被复用给别的进程；未登记
/// 创建时间（0）= 证明不了身份——两者都拒绝，杀错进程不可逆。
///
/// 返回 Err 的信息已含原因，调用方直接展示。
pub fn force_terminate(info: &InstanceRecord) -> Result<(), String> {
    crate::watchdog::terminate_verified_process(info.pid, info.process_start)
}

/// 向 startup.log 追加一行（带 unix 时间戳前缀）。看门狗的 crashloop 放弃、
/// 假死接管等重大事件写这里——用户排查「实例为什么没被拉起」的第一个入口。
pub fn append_startup_log(line: &str) {
    let _ = std::fs::create_dir_all(logs_dir());
    let path = logs_dir().join("startup.log");
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        use std::io::Write;
        let _ = writeln!(f, "[unix+{ts}s] {line}");
    }
}

/// 等待实例退出：只有端点消失（`Unreachable`）才算已退出，超时返回 false。
/// 忙、超时、应答无法采信都不算——挂死的实例正是「端点还在、却不应答」，
/// 据此报「已停止」会掩盖一个仍占着端口的进程。
pub async fn wait_until_gone(port: &str, timeout: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if let Err(IpcError::Unreachable(_)) = ipc_ping(port).await {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// 以分离进程重新执行自身承载服务。
///
/// CREATE_NO_WINDOW：不继承父控制台、无窗口——关闭终端、控制台进程树清理
/// 都不会连带杀掉守护进程（这正是前台进程被「常规清理」关掉的根因）。
///
/// Windows 上不走 std::process::Command：std 的 CreateProcessW 调用固定
/// bInheritHandles=TRUE 且无稳定 API 可关闭（CommandExt::inherit_handles
/// 仍 unstable）。这意味着调用方（cargo test / agent 脚本 / CI）经
/// Command::output() 捕获输出运行 `aproxy start` 时，其 stdout/stderr 管道
/// 写端句柄会被常驻守护进程继承，EOF 永不到来，调用方永久挂死。因此这里
/// 手写 CreateProcessW，以 bInheritHandles=FALSE 启动，不继承任何句柄。
/// 刻意不加 CREATE_BREAKAWAY_FROM_JOB：作业对象不允许 breakaway 时该标志
/// 会让 CreateProcess 直接失败，把「父环境关闭可能连带杀掉守护」这一罕见
/// 场景恶化成「根本启动不了」，得不偿失。
/// 返回子进程 pid（spawn 即返回，不等待——守护不随父进程生命周期）。
pub fn spawn_detached(exe: &std::path::Path, args: &[String]) -> io::Result<u32> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Foundation::{CloseHandle, GetLastError};
        use windows_sys::Win32::System::Threading::{
            CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT, CreateProcessW, PROCESS_INFORMATION,
            STARTUPINFOW,
        };

        /// Windows 命令行引号规则：含空格/制表符/引号的参数包引号；
        /// 参数内的 `"` 转成 `""`，引号前的连续 `\` 翻倍（闭引号前同样翻倍）。
        fn quote_arg(arg: &str) -> String {
            if !arg.is_empty()
                && !arg
                    .chars()
                    .any(|c| matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0b' | '\x0c' | '"'))
            {
                return arg.to_string();
            }
            let mut out = String::with_capacity(arg.len() + 2);
            out.push('"');
            let mut backslashes = 0usize;
            for c in arg.chars() {
                match c {
                    '\\' => backslashes += 1,
                    '"' => {
                        out.push_str(&"\\".repeat(backslashes * 2 + 1));
                        out.push('"');
                        backslashes = 0;
                    }
                    _ => {
                        out.push_str(&"\\".repeat(backslashes));
                        out.push(c);
                        backslashes = 0;
                    }
                }
            }
            // 闭引号前的 `\` 同样要翻倍，否则会被当作转义引号
            out.push_str(&"\\".repeat(backslashes * 2));
            out.push('"');
            out
        }

        let mut cmdline = quote_arg(&exe.display().to_string());
        for arg in args {
            cmdline.push(' ');
            cmdline.push_str(&quote_arg(arg));
        }
        let exe_wide: Vec<u16> = exe.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut cmdline_wide: Vec<u16> = cmdline.encode_utf16().chain(Some(0)).collect();

        let mut si: STARTUPINFOW = unsafe { std::mem::zeroed() };
        si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
        let ok = unsafe {
            CreateProcessW(
                exe_wide.as_ptr(),
                cmdline_wide.as_mut_ptr(),
                std::ptr::null(), // lpProcessAttributes：默认安全描述符
                std::ptr::null(), // lpThreadAttributes：默认安全描述符
                0,                // bInheritHandles=FALSE：核心目的，杜绝句柄泄漏
                CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT,
                std::ptr::null(), // lpEnvironment=NULL：继承父进程环境
                std::ptr::null(), // lpCurrentDirectory=NULL：维持继承 cwd 的现状
                &si,
                &mut pi,
            )
        };
        if ok == 0 {
            return Err(io::Error::from_raw_os_error(
                unsafe { GetLastError() } as i32
            ));
        }
        let pid = pi.dwProcessId;
        // 守护进程只关心 pid；句柄持有会导致进程退出通知与资源泄漏
        unsafe {
            CloseHandle(pi.hProcess);
            CloseHandle(pi.hThread);
        }
        Ok(pid)
    }
    #[cfg(unix)]
    {
        use std::process::{Command, Stdio};
        let mut cmd = Command::new(exe);
        cmd.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        Ok(cmd.spawn()?.id())
    }
}

// ---------------------------------------------------------------------------
// 实例注册表（~/.aproxy/run/<port>.pid）
// ---------------------------------------------------------------------------

/// 注册表文件路径：`<run_dir>/<port>.pid`
pub fn instance_file_path(listen_addr: &str) -> PathBuf {
    instance_file_path_in(&run_dir(), listen_addr)
}

/// 同上，目录可指定（测试注入用）
pub fn instance_file_path_in(run_dir: &std::path::Path, listen_addr: &str) -> PathBuf {
    run_dir.join(format!("{}.pid", port_of(listen_addr)))
}

/// 写入实例注册（bind 成功后调用，避免留下死记录）
pub fn write_instance_file(info: &InstanceRecord) -> io::Result<()> {
    write_instance_file_in(&run_dir(), info)
}

/// 同上，目录可指定（测试注入用）
///
/// 原子写：先写同目录临时文件再 rename 覆盖目标。直接写目标文件时，
/// 并发的 list_instances 可能读到写到一半的 JSON，把记录当损坏清理掉。
/// rename 在同卷内是原子操作（Windows 上经 MoveFileEx 的替换语义覆盖
/// 已存在文件）；临时文件名按端口隔离，实例间不会互踩。
pub fn write_instance_file_in(run_dir: &std::path::Path, info: &InstanceRecord) -> io::Result<()> {
    std::fs::create_dir_all(run_dir)?;
    let path = instance_file_path_in(run_dir, &info.listen_addr);
    let json = serde_json::to_string_pretty(info).expect("序列化实例信息失败");
    let tmp = path.with_extension("pid.tmp");
    std::fs::write(&tmp, json)?;
    match std::fs::rename(&tmp, &path) {
        Ok(()) => Ok(()),
        Err(e) => {
            // rename 失败时清掉残留临时文件，避免堆积成永久垃圾
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// 删除实例注册（服务退出时调用）
pub fn remove_instance_file(listen_addr: &str) {
    let _ = std::fs::remove_file(instance_file_path(listen_addr));
}

/// 列出注册表中应答控制通道的实例（见 `survey_instances_in`）。
/// 顺带清理孤儿日志（status 是唯一可靠的清理时机）。
pub async fn list_instances() -> Vec<InstanceStatus> {
    survey_instances().await.responsive
}

/// 注册表普查结果：应答的实例，与「进程仍在、却不应答控制通道」的实例。
pub struct InstanceSurvey {
    /// IPC ping 成功：ping 应答里的实时状态
    pub responsive: Vec<InstanceStatus>,
    /// ping 失败但记录的进程经「pid + 创建时间」核验仍在（多为挂死）：信息
    /// 取自注册表记录（启动时刻的快照）。aproxy 无法经 IPC 优雅停止它们，只能
    /// `stop --force`（终止前同样核验身份）
    pub unresponsive: Vec<InstanceRecord>,
}

/// 普查注册表并顺带清理孤儿日志——不应答实例的日志同样计入引用集（进程还在
/// 写它），不能当孤儿删掉。
pub async fn survey_instances() -> InstanceSurvey {
    let survey = survey_instances_in(&run_dir()).await;
    let referenced: Vec<InstanceRecord> = survey
        .responsive
        .iter()
        .map(|status| &status.instance)
        .chain(survey.unresponsive.iter())
        .cloned()
        .collect();
    cleanup_orphan_logs_in(&logs_dir(), &referenced, &list_restore_entries());
    survey
}

/// 清理孤儿日志：日志文件名随机化后不再携带归属信息，判据改为「引用集」——
/// 活实例上报的 log_path ∪ .restore 记录的 log_path 之外的 .log 一律删除。
/// logs 目录是工具专属目录，目录内 .log 一律视为实例日志（防御保留非 .log
/// 文件与 startup.log）。两条推论：(a) 旧版按端口命名的日志升级后会被视为
/// 孤儿清理（alpha 阶段无兼容义务）；(b) 崩溃实例的日志由 .restore 引用
/// 保留到恢复成功——恢复后记录重写为新实例的路径，旧日志按孤儿收敛。
/// 自定义 log_file 落在其他目录时，扫描天然不触及。空串 log_path（前台）
/// 不参与引用集。目录与待恢复清单参数化（测试注入用）。
fn cleanup_orphan_logs_in(
    logs_dir: &std::path::Path,
    live: &[InstanceRecord],
    pending_restore: &[RestoreEntry],
) {
    // 引用集按 path_match_key 归一比较（大小写/分隔符规则与别名匹配共用）：
    // 注册表存的路径字符串与 read_dir 枚举出的表示在 Windows 上可能有
    // 分隔符差异，字面比较会漏保活
    let referenced: std::collections::HashSet<String> = live
        .iter()
        .map(|i| i.log_path.clone())
        .chain(pending_restore.iter().map(|e| e.log_path.clone()))
        .filter(|p| !p.is_empty())
        .map(|p| crate::settings::path_match_key(&p))
        .collect();

    let Ok(entries) = std::fs::read_dir(logs_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if path.extension().and_then(|e| e.to_str()) != Some("log") || stem == "startup" {
            continue;
        }
        let key = crate::settings::path_match_key(&path.display().to_string());
        if !referenced.contains(&key) {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// 同上，目录可指定（测试注入用）
pub async fn list_instances_in(dir: &std::path::Path) -> Vec<InstanceStatus> {
    survey_instances_in(dir).await.responsive
}

/// 逐条读注册表、IPC ping 验活。ping 失败时按「pid + 创建时间」核验记录的
/// 进程：仍在 → 记录保留、归入 unresponsive；已退出或 pid 已被复用 → 清理
/// 残留记录。**不应答 ≠ 已死**：挂死的实例进程还在，看门狗的挂死接管要靠
/// 这条记录核验身份，`stop --force` 也要靠它定位——因为一次 ping 失败就删掉
/// 它，实例就从 aproxy 的视野里消失了。未登记创建时间（0）的记录证明不了
/// 身份，同样清理。
pub async fn survey_instances_in(dir: &std::path::Path) -> InstanceSurvey {
    let mut out = Vec::new();
    let mut unresponsive = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return InstanceSurvey {
            responsive: out,
            unresponsive,
        };
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("pid") {
            continue;
        }
        let info = std::fs::read_to_string(&path)
            .ok()
            .and_then(|c| serde_json::from_str::<InstanceRecord>(&c).ok());
        let Some(info) = info else {
            // 损坏记录无法定位端口发 IPC，直接清理
            let _ = std::fs::remove_file(&path);
            continue;
        };
        let port = port_of(&info.listen_addr).to_string();
        match ipc_ping_in(dir, &port).await {
            // ping 应答携带实例的实时状态（含最近活动时刻）——注册表 .pid
            // 是启动时刻的快照，闲置判定/展示必须用实时值
            Ok(live) if live.instance.pid == info.pid => out.push(live),
            // 应答的不是记录里的进程。新实例先建端点、后写记录，记录可能正被
            // 重写——重读一次再比。仍对不上时，记录的进程若还在就按无响应
            // 列出（它不在自己的端点上应答）；否则不列出、也不删：此刻删除
            // 可能恰好删掉应答者刚写下的记录
            Ok(live) => {
                if read_instance_file_in(dir, &port).is_some_and(|f| f.pid == live.instance.pid) {
                    out.push(live);
                } else if matches!(
                    crate::watchdog::record_identity(info.pid, info.process_start),
                    crate::watchdog::RecordIdentity::Alive(_)
                ) {
                    unresponsive.push(info);
                }
            }
            Err(_) => match crate::watchdog::record_identity(info.pid, info.process_start) {
                crate::watchdog::RecordIdentity::Alive(_) => unresponsive.push(info),
                // 实例不在了（或无从核验）：注册已失效，清理
                _ => {
                    let _ = std::fs::remove_file(&path);
                }
            },
        }
    }
    out.sort_by(|a, b| a.instance.listen_addr.cmp(&b.instance.listen_addr));
    unresponsive.sort_by(|a, b| a.listen_addr.cmp(&b.listen_addr));
    InstanceSurvey {
        responsive: out,
        unresponsive,
    }
}

/// 只读检索注册表：是否存在 pid 匹配的实例记录。
/// respawn 的就绪判定专用——不复用 `list_instances_in`（它对 ping 失败的
/// 条目有删除副作用：A 实例重拉的就绪轮询会顺带清掉同注册表里 B/C 死实例
/// 的记录，其死亡事件随后被混合态误判为优雅退出而失去自动恢复）。就绪只需
/// 「新 pid 的注册记录已出现」，读文件即可，无需 IPC。
pub fn registry_contains_pid_in(dir: &std::path::Path, pid: u32) -> bool {
    registry_find_pid_in(dir, pid).is_some()
}

/// 只读检索注册表：取 pid 匹配的实例记录（无副作用，理由同上）。
/// 重拉/恢复/重启按 spawn 返回的 pid 定位新实例，从记录里读它**实际**监听
/// 的地址——配置可能已改端口（或 listen 端口为 0 由系统分配），新实例未必
/// 落在原端口。
pub fn registry_find_pid_in(dir: &std::path::Path, pid: u32) -> Option<InstanceRecord> {
    let entries = std::fs::read_dir(dir).ok()?;
    entries.flatten().find_map(|entry| {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("pid") {
            return None;
        }
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|c| serde_json::from_str::<InstanceRecord>(&c).ok())
            .filter(|info| info.pid == pid)
    })
}

/// 新 spawn 的守护实例未能就绪的原因
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpawnNotReady {
    /// 进程在就绪前已退出（配置错误、bind 失败等，原因落在 startup.log）
    Exited,
    /// 超时：进程仍在但未就绪（或进程状态不可判）
    TimedOut,
}

/// 等待 spawn 出的守护（pid 来自 spawn 返回值，唯一可靠锚点）就绪：按 pid
/// 在注册表定位实际端口（配置可能已换端口/端口 0），再 IPC ping 实际端口
/// 确认应答者就是它。成功返回 ping 到的实时状态（`instance.listen_addr`
/// 即实际监听地址）。进程提前退出立即返回 Exited，不空等
/// 到超时。只读检索注册表，无 list_instances_in 的删除副作用。
pub async fn wait_spawned_instance_ready(
    run_dir: &std::path::Path,
    pid: u32,
    timeout: Duration,
) -> Result<InstanceStatus, SpawnNotReady> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Some(rec) = registry_find_pid_in(run_dir, pid)
            && let Ok(live) = ipc_ping_in(run_dir, port_of(&rec.listen_addr)).await
            && live.instance.pid == pid
        {
            return Ok(live);
        }
        if crate::watchdog::process_exited(pid) == Some(true) {
            return Err(SpawnNotReady::Exited);
        }
        if std::time::Instant::now() > deadline {
            return Err(SpawnNotReady::TimedOut);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// 实例重拉/恢复后实际落在了另一个端口（用户改了 config.toml 的端口却没
/// restart，或 listen 端口为 0 每次由系统分配）时，退役旧端口的残留记录：
/// 删除 `<旧端口>.restore` 与 `<旧端口>.pid`。
///
/// 为什么必须删：新实例已按实际端口写了自己的 .restore/.pid；旧端口的
/// .restore 若残留，用户 `stop <新端口>` 后看护者（或下一次 `aproxy restore`）
/// 会凭它把实例再拉起来，stop 被撤销且可无限重复（watchdog-01 实测）。
/// 旧端口的 .pid 是已死旧进程的记录，留着只会让看护者的闲置自灭永远等不到
/// 「注册表空」。
///
/// 保护：旧端口若已被另一个仍在运行的实例接手（其注册表记录核验为同一
/// 进程），记录归它，一律不动。返回是否执行了退役。
pub fn retire_moved_port_records_in(
    run_dir: &std::path::Path,
    old_port: &str,
    new_port: &str,
) -> bool {
    if old_port == new_port {
        return false;
    }
    if let Some(rec) = read_instance_file_in(run_dir, old_port)
        && matches!(
            crate::watchdog::record_identity(rec.pid, rec.process_start),
            crate::watchdog::RecordIdentity::Alive(_)
        )
    {
        return false;
    }
    remove_restore_file_in(run_dir, old_port);
    let _ = std::fs::remove_file(instance_file_path_in(run_dir, old_port));
    true
}

/// 删除实例的 IPC 端点文件（优雅退出/看护摘除时调用）。
/// Windows 命名管道由内核回收（no-op）；unix 的 UDS socket 是真实文件，
/// bind 前的 remove_file 已自愈残留，此处显式清理让 run/ 目录不留死端点。
pub fn remove_socket_file(port: &str) {
    #[cfg(unix)]
    {
        let _ = std::fs::remove_file(endpoint_for(port));
    }
    #[cfg(windows)]
    {
        let _ = port;
    }
}

// ---------------------------------------------------------------------------
// 自愈恢复记录（~/.aproxy/run/<port>.restore）
//
// 与 .pid 注册表互补：.pid 只在实例存活期间存在，而崩溃/断电/系统重启时守护
// 进程来不及清理任何文件——restore 记录承载「期望在运行」的语义：守护 bind
// 成功时写入当时的启动参数，优雅退出（含 `aproxy stop`）时删除，非正常死亡
// 则保留，`aproxy restore` 据此一键拉起。注意：参数可能含 --api-key 等明文，
// 与 config.toml 同级存放（用户主目录内），不额外加密。
// ---------------------------------------------------------------------------

/// 恢复记录文件路径：`<run_dir>/<port>.restore`
pub fn restore_file_path(listen_addr: &str) -> PathBuf {
    restore_file_path_in(&run_dir(), listen_addr)
}

/// 同上，目录可指定（测试注入用）
pub fn restore_file_path_in(run_dir: &std::path::Path, listen_addr: &str) -> PathBuf {
    run_dir.join(format!("{}.restore", port_of(listen_addr)))
}

/// 写入恢复记录：`args` 为实例的启动参数（不含 --daemon-child，恢复时统一追加），
/// `log_path` 为本次启动的守护日志路径（崩溃后排障线索，前台实例落空串）。
/// bind 成功后调用；同端口重复启动时覆盖旧记录。
pub fn write_restore_file(listen_addr: &str, args: &[String], log_path: &str) -> io::Result<()> {
    write_restore_file_in(&run_dir(), listen_addr, args, log_path)
}

/// 同上，目录可指定（测试注入用）。原子写语义同 write_instance_file_in。
pub fn write_restore_file_in(
    run_dir: &std::path::Path,
    listen_addr: &str,
    args: &[String],
    log_path: &str,
) -> io::Result<()> {
    std::fs::create_dir_all(run_dir)?;
    let path = restore_file_path_in(run_dir, listen_addr);
    let json = serde_json::to_string(&RestoreRecord {
        args: args.to_vec(),
        log_path: log_path.to_string(),
    })
    .expect("序列化恢复记录失败");
    let tmp = path.with_extension("restore.tmp");
    std::fs::write(&tmp, json)?;
    match std::fs::rename(&tmp, &path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// 删除恢复记录（实例优雅退出时调用；无记录时静默）
pub fn remove_restore_file(listen_addr: &str) {
    remove_restore_file_in(&run_dir(), listen_addr)
}

/// 同上，目录可指定（测试注入用）
pub fn remove_restore_file_in(run_dir: &std::path::Path, listen_addr: &str) {
    let _ = std::fs::remove_file(restore_file_path_in(run_dir, listen_addr));
}

/// 一条恢复记录：端口 + 启动参数 + 崩溃前那次启动的日志路径。
/// log_path 供孤儿日志清理引用（崩溃实例的日志保留到恢复成功，作排障线索）；
/// 前台实例无日志文件，落空串（崩溃恢复后是守护形态，新随机名）。
#[derive(Debug, Clone, PartialEq)]
pub struct RestoreEntry {
    pub port: String,
    pub args: Vec<String>,
    pub log_path: String,
}

/// .restore 文件的落盘格式（结构体 JSON）。两个字段都必填：缺字段的记录
/// 无法忠实恢复实例，按损坏处理（列举时清理）。
#[derive(Serialize, Deserialize)]
struct RestoreRecord {
    args: Vec<String>,
    log_path: String,
}

/// 列出全部恢复记录；损坏记录直接清理。不做存活校验——记录的本义就是
/// 「实例可能已死」。
pub fn list_restore_entries() -> Vec<RestoreEntry> {
    list_restore_entries_in(&run_dir())
}

/// 同上，目录可指定（测试注入用）
pub fn list_restore_entries_in(dir: &std::path::Path) -> Vec<RestoreEntry> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("restore") {
            continue;
        }
        let Some(port) = path.file_stem().and_then(|s| s.to_str()).map(String::from) else {
            continue;
        };
        let parsed = std::fs::read_to_string(&path).ok().and_then(|c| {
            serde_json::from_str::<RestoreRecord>(&c)
                .map(|r| RestoreEntry {
                    port,
                    args: r.args,
                    log_path: r.log_path,
                })
                .ok()
        });
        match parsed {
            Some(entry) => out.push(entry),
            None => {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
    out.sort_by(|a, b| a.port.cmp(&b.port));
    out
}

// ---------------------------------------------------------------------------
// 通用协议层：请求-响应各一行 JSON。两个平台的传输实现共用这套编解码。
// ---------------------------------------------------------------------------

/// 在已建立的连接上完成一次「发请求行、收响应行」。
async fn exchange_over<S>(stream: S, req_line: &str) -> Result<String, IpcError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    // 应答行长度上限：端点名可以预测，应答者未必是 aProxy，不能让它把客户端
    // 内存撑爆。真实应答只有几百字节
    const MAX_RESPONSE_LINE: u64 = 1024 * 1024;
    let failed = |e: io::Error| IpcError::Transient(e.to_string());
    let (reader, mut writer) = tokio::io::split(stream);
    writer
        .write_all(req_line.as_bytes())
        .await
        .map_err(failed)?;
    writer.write_all(b"\n").await.map_err(failed)?;
    let mut line = String::new();
    BufReader::new(reader)
        .take(MAX_RESPONSE_LINE)
        .read_line(&mut line)
        .await
        .map_err(failed)?;
    if !line.ends_with('\n') {
        return Err(if line.len() as u64 >= MAX_RESPONSE_LINE {
            IpcError::Protocol("应答超过长度上限".to_string())
        } else {
            IpcError::Transient("连接在应答完整之前关闭".to_string())
        });
    }
    Ok(line)
}

/// 请求用的是哪种格式，应答就回哪种。
#[derive(Clone, Copy, Debug)]
enum Dialect {
    V1,
    /// 0.1.0 的 CLI 与安装器（请求没有 `v`），见 compat_0_1_0
    Legacy,
}

/// 处理一条 IPC 连接：读请求行 → 执行 → 回应答行。
async fn handle_conn<S>(stream: S, state: Arc<ControlState>) -> io::Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
    let (reader, mut writer) = tokio::io::split(stream);
    let mut line = String::new();
    // 请求行长度上限：控制端点是系统边界输入（同用户本地进程均可打开写入），
    // read_line 会无界累积直到遇到 \n，恶意/异常客户端可借此把守护进程内存
    // 吃到 OOM。超过上限即按无效请求回绝后断开。
    const MAX_REQUEST_LINE: u64 = 64 * 1024;
    // 读请求的时限：连上却迟迟不发完一行的客户端会一直占着这个处理任务
    // （Windows 上还占着一个管道实例，实例数有上限，占满后新连接全部失败）。
    // 正常客户端连上即发，几毫秒内完成
    const REQUEST_READ_TIMEOUT: Duration = Duration::from_secs(5);
    let mut limited = BufReader::new(reader).take(MAX_REQUEST_LINE);
    match tokio::time::timeout(REQUEST_READ_TIMEOUT, limited.read_line(&mut line)).await {
        Ok(read) => {
            read?;
        }
        Err(_) => return Ok(()),
    }
    let (dialect, op) = match parse_request(&line) {
        Ok(parsed) => parsed,
        Err(reply) => return write_line(&mut writer, &reply).await,
    };
    if op == IpcOp::PrepareSwap {
        if *state.on_shutdown.borrow() {
            let reply = match dialect {
                Dialect::V1 => {
                    error_reply(error_code::INVALID_STATE, "实例正在退出，不再进入更换阶段")
                }
                Dialect::Legacy => crate::compat_0_1_0::legacy_reply(None),
            };
            return write_line(&mut writer, &reply).await;
        }
        // 先置位再组装应答：应答里立即是 swap_prepared，安装器拿这一个应答
        // 就完成 ACK 判定，无需再补一次 ping
        state
            .stats
            .swap_phase
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
    let mut status = status_snapshot(&state);
    if op == IpcOp::Shutdown {
        status.state = InstanceState::Stopping;
    }
    let reply = match dialect {
        Dialect::V1 => serde_json::to_string(&WireResponse {
            v: IPC_VERSION,
            ok: true,
            result: Some(status),
            error: None,
        })
        .expect("序列化 IPC 应答失败"),
        Dialect::Legacy => crate::compat_0_1_0::legacy_reply(Some(&status)),
    };
    write_line(&mut writer, &reply).await?;
    // 应答先发出去再触发停止：客户端立刻拿到确认，服务随后优雅退出
    if op == IpcOp::Shutdown {
        let _ = state.on_shutdown.send(true);
    }
    Ok(())
}

/// 解析请求行。Err 里是要直接回给客户端的拒绝应答。
fn parse_request(line: &str) -> Result<(Dialect, IpcOp), String> {
    // 行未正常终止：超过长度上限被截断，或对端在发完整请求前就断开
    if !line.ends_with('\n') {
        return Err(error_reply(error_code::BAD_REQUEST, "请求行过长或不完整"));
    }
    let Ok(request) = serde_json::from_str::<serde_json::Value>(line) else {
        return Err(error_reply(error_code::BAD_REQUEST, "请求不是 JSON"));
    };
    let Some(v) = request.get("v") else {
        return match crate::compat_0_1_0::legacy_op(&request) {
            Some(op) => Ok((Dialect::Legacy, op)),
            None => Err(crate::compat_0_1_0::legacy_reply(None)),
        };
    };
    if v.as_u64() != Some(u64::from(IPC_VERSION)) {
        let reply = WireResponse {
            v: IPC_VERSION,
            ok: false,
            result: None,
            error: Some(WireError {
                code: error_code::UNSUPPORTED_VERSION.to_string(),
                message: format!("实例只支持控制协议 v{IPC_VERSION}，收到 v{v}"),
                supported: Some(vec![IPC_VERSION]),
            }),
        };
        return Err(serde_json::to_string(&reply).expect("序列化 IPC 应答失败"));
    }
    let Some(name) = request.get("op").and_then(|op| op.as_str()) else {
        return Err(error_reply(error_code::BAD_REQUEST, "请求缺少 op"));
    };
    if request.get("args").is_some_and(|args| !args.is_object()) {
        return Err(error_reply(error_code::BAD_REQUEST, "args 必须是对象"));
    }
    match IpcOp::from_name(name) {
        Some(op) => Ok((Dialect::V1, op)),
        None => Err(error_reply(
            error_code::UNKNOWN_OP,
            &format!("实例没有 op \"{name}\""),
        )),
    }
}

fn error_reply(code: &str, message: &str) -> String {
    serde_json::to_string(&WireResponse {
        v: IPC_VERSION,
        ok: false,
        result: None,
        error: Some(WireError {
            code: code.to_string(),
            message: message.to_string(),
            supported: None,
        }),
    })
    .expect("序列化 IPC 应答失败")
}

/// 组装此刻的状态快照（计数器、最近错误在应答瞬间现读）。
fn status_snapshot(state: &ControlState) -> InstanceStatus {
    use std::sync::atomic::Ordering::Relaxed;
    let stats = &state.stats;
    let phase = if *state.on_shutdown.borrow() {
        InstanceState::Stopping
    } else if stats.swap_phase.load(Relaxed) {
        InstanceState::SwapPrepared
    } else {
        InstanceState::Serving
    };
    let last_error = stats.last_error.lock().ok().and_then(|slot| {
        slot.as_ref().map(|(message, at)| LastError {
            message: message.clone(),
            at: *at,
        })
    });
    InstanceStatus {
        instance: state.record.clone(),
        run_dir: state.run_dir.clone(),
        state: phase,
        activity: Activity {
            last_request_at: stats
                .last_activity_secs
                .load(Relaxed)
                .max(state.record.started_at),
            requests_total: stats.requests_total.load(Relaxed),
            retries_total: stats.retries_total.load(Relaxed),
            last_error,
        },
        ops: IpcOp::ALL.iter().map(|op| op.name().to_string()).collect(),
    }
}

async fn write_line<W>(writer: &mut W, line: &str) -> io::Result<()>
where
    W: tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::AsyncWriteExt;
    writer.write_all(line.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await
}

// ---------------------------------------------------------------------------
// 平台实现：windows 命名管道 / unix domain socket，上层不感知差异。
// ---------------------------------------------------------------------------
#[cfg(windows)]
mod imp {
    use super::{ControlState, IpcError, exchange_over, handle_conn};
    use std::{io, sync::Arc, time::Duration};
    use tokio::net::windows::named_pipe::{
        ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions,
    };

    /// 端点不存在（无实例）时返回可读错误。成功时一并返回管道服务端的进程
    /// pid（取不到为 None），供调用方核对应答者自报的 pid。
    ///
    /// ERROR_PIPE_BUSY(231) 重试：serve 循环在 connect() 完成、重建下一个
    /// 监听实例之间存在零监听窗口，管道名存在但无空闲实例，此时 CreateFile
    /// 返回 busy 而非「端点不存在」。tokio 文档明确要求客户端对该错误
    /// sleep 后重试；封顶 2 秒（上层 ipc_request 的 3 秒超时之内）。
    pub async fn exchange(
        endpoint: &str,
        req_line: &str,
    ) -> Result<(String, Option<u32>), IpcError> {
        const ERROR_PIPE_BUSY: i32 = 231;
        const BUSY_RETRY_CAP: Duration = Duration::from_secs(2);
        const BUSY_RETRY_INTERVAL: Duration = Duration::from_millis(50);
        let deadline = tokio::time::Instant::now() + BUSY_RETRY_CAP;
        let client = loop {
            match ClientOptions::new().open(endpoint) {
                Ok(client) => break client,
                Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                    if tokio::time::Instant::now() >= deadline {
                        return Err(IpcError::Transient(format!("无法连接（{e}）")));
                    }
                    tokio::time::sleep(BUSY_RETRY_INTERVAL).await;
                }
                // 管道名不存在：确定没有实例在这个端点上听
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    return Err(IpcError::Unreachable(format!("无法连接（{e}）")));
                }
                Err(e) => return Err(IpcError::Transient(format!("无法连接（{e}）"))),
            }
        };
        let peer = server_pid(&client);
        Ok((exchange_over(client, req_line).await?, peer))
    }

    fn server_pid(client: &NamedPipeClient) -> Option<u32> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::Pipes::GetNamedPipeServerProcessId;
        let mut pid = 0u32;
        // SAFETY: 句柄来自仍存活的 client，pid 指向本栈上的 u32
        let ok = unsafe { GetNamedPipeServerProcessId(client.as_raw_handle() as isize, &mut pid) };
        (ok != 0 && pid != 0).then_some(pid)
    }

    /// 已独占创建的端点：第一个管道实例在手，名字从此归本进程。
    pub struct Listener {
        endpoint: String,
        first: NamedPipeServer,
    }

    /// `first_pipe_instance`：名字已被别的进程占着就失败，而不是加入它、和它
    /// 分摊连接——那样一半的 stop/ping 会落到别人手里。
    pub async fn bind(endpoint: &str) -> io::Result<Listener> {
        let first = ServerOptions::new()
            .first_pipe_instance(true)
            .create(endpoint)?;
        Ok(Listener {
            endpoint: endpoint.to_string(),
            first,
        })
    }

    /// 接受循环：为每个连接 spawn 处理任务，并始终备好下一个监听实例。不会
    /// 返回——单个连接出错只丢掉那一个实例，控制通道本身要一直在。
    pub async fn accept_loop(listener: Listener, state: Arc<ControlState>) {
        let Listener {
            endpoint,
            first: mut server,
        } = listener;
        loop {
            let connected = server.connect().await;
            // 先建好下一个实例，再交出（或丢掉）当前这个：管道名只在至少还有
            // 一个实例时存在，名字一消失，客户端就会把「找不到」当成实例已退出
            let next = loop {
                match ServerOptions::new().create(&endpoint) {
                    Ok(next) => break next,
                    Err(e) => {
                        tracing::warn!(error = %e, "控制管道新建监听实例失败，1 秒后重试");
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }
            };
            let current = std::mem::replace(&mut server, next);
            match connected {
                Ok(()) => {
                    let state = state.clone();
                    tokio::spawn(async move {
                        let _ = handle_conn(current, state).await;
                    });
                }
                // 客户端在服务端接上之前就断开等：只影响这一个连接
                Err(e) => tracing::debug!(error = %e, "控制管道连接未建立，换一个监听实例继续"),
            }
        }
    }
}

#[cfg(unix)]
mod imp {
    use super::{ControlState, IpcError, exchange_over, handle_conn};
    use std::{io, path::Path, sync::Arc, time::Duration};
    use tokio::net::UnixListener;

    /// 成功时一并返回对端进程 pid（SO_PEERCRED 一类机制；取不到为 None），
    /// 供调用方核对应答者自报的 pid。
    pub async fn exchange(
        endpoint: &str,
        req_line: &str,
    ) -> Result<(String, Option<u32>), IpcError> {
        let client = tokio::net::UnixStream::connect(endpoint)
            .await
            .map_err(|e| {
                // socket 文件不存在，或文件还在但没有进程在听（崩溃残留）：
                // 确定没有实例在这个端点上
                let msg = format!("无法连接（{e}）");
                match e.kind() {
                    io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused => {
                        IpcError::Unreachable(msg)
                    }
                    _ => IpcError::Transient(msg),
                }
            })?;
        let peer = client
            .peer_cred()
            .ok()
            .and_then(|cred| cred.pid())
            .and_then(|pid| u32::try_from(pid).ok())
            .filter(|&pid| pid != 0);
        Ok((exchange_over(client, req_line).await?, peer))
    }

    pub struct Listener(UnixListener);

    /// 独占创建 socket。路径上已有文件时先探测：连得上（或积压队列满）说明有
    /// 进程正在用它（同一 home 同端口的另一个实例），拒绝——删掉它的 socket 等于
    /// 把它的控制通道抢走，它从此 stop/status 不可达；其余连接失败都说明没有
    /// 进程在听（崩溃残留），删掉重建。
    pub async fn bind(endpoint: &str) -> io::Result<Listener> {
        let path = Path::new(endpoint);
        // 端点先于注册表创建，首次启动时 run 目录可能还不存在
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        match tokio::net::UnixStream::connect(path).await {
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) if e.kind() != io::ErrorKind::WouldBlock => {
                let _ = std::fs::remove_file(path);
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    format!("已有进程在 {endpoint} 上提供控制通道"),
                ));
            }
        }
        UnixListener::bind(path).map(Listener)
    }

    pub async fn accept_loop(listener: Listener, state: Arc<ControlState>) {
        let Listener(listener) = listener;
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(conn) => conn,
                Err(_) => {
                    // 持续性 accept 错误（如 fd 耗尽的 EMFILE）不会自行恢复，
                    // 立即重试会形成占满 CPU 的紧死循环；睡一拍给错误源恢复机会。
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
            };
            let state = state.clone();
            tokio::spawn(async move {
                let _ = handle_conn(stream, state).await;
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    fn sample_info(port: &str) -> InstanceRecord {
        InstanceRecord {
            pid: 42,
            process_start: 0,
            version: "0.0.0-test".into(),
            listen_addr: format!("127.0.0.1:{port}"),
            config_path: "C:/tmp/config.toml".into(),
            base_url: "https://api.example.com".into(),
            started_at: 1_700_000_000,
            log_path: String::new(),
        }
    }

    /// 测试用的控制服务状态；返回的 Receiver 用来观察 shutdown 是否被触发
    fn control(record: InstanceRecord) -> (Arc<ControlState>, tokio::sync::watch::Receiver<bool>) {
        let (on_shutdown, rx) = tokio::sync::watch::channel(false);
        let state = ControlState {
            record,
            run_dir: "/test/run".into(),
            stats: Arc::new(IpcStats::default()),
            on_shutdown,
        };
        (Arc::new(state), rx)
    }

    #[test]
    fn port_of_extracts_trailing_port() {
        assert_eq!(port_of("127.0.0.1:12345"), "12345");
        assert_eq!(port_of("[::1]:8080"), "8080");
        assert_eq!(port_of("no-port"), "no-port");
    }

    // ---------------- 进程身份（pid + 创建时间，与二进制名称无关） ----------------
    //
    // 被测对象是名字与 aProxy 无关的子进程（Windows PING.EXE / Linux sleep），
    // 由测试自己 spawn 并回收：旧的镜像名关卡会一律拒绝它，新判定只看 pid +
    // 创建时间。macOS 读不到创建时间（不在本轮支持范围），只在 Windows/Linux
    // 编译运行。端口用 597xx 段：没有其他测试在用，避免与「端点必须无人应答」
    // 的测试（59xxx）互相干扰。

    #[cfg(any(windows, target_os = "linux"))]
    fn spawn_unrelated_child() -> std::process::Child {
        #[cfg(windows)]
        let mut cmd = {
            let mut c = std::process::Command::new("ping");
            c.args(["-n", "60", "127.0.0.1"]);
            c
        };
        #[cfg(target_os = "linux")]
        let mut cmd = {
            let mut c = std::process::Command::new("sleep");
            c.arg("60");
            c
        };
        cmd.stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn 测试子进程失败")
    }

    #[cfg(any(windows, target_os = "linux"))]
    fn wait_child_exit(child: &mut std::process::Child) -> bool {
        for _ in 0..50 {
            if child.try_wait().unwrap().is_some() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        false
    }

    /// stop --force 的身份策略：登记时间不符（pid 复用）拒绝；无时间戳的旧
    /// 记录且端点无人应答（归属无从证明）拒绝；身份一致即终止——被杀的进程
    /// 名字与 aProxy 无关，证明判定不看名称
    #[cfg(any(windows, target_os = "linux"))]
    #[tokio::test]
    async fn force_terminate_checks_identity_not_name() {
        let mut child = spawn_unrelated_child();
        let pid = child.id();
        let start = crate::watchdog::process_start_time(pid).expect("子进程创建时间可查");
        let mut info = sample_info(&format!("597{:02}", std::process::id() % 50));
        info.pid = pid;

        info.process_start = start.wrapping_add(1);
        let err = force_terminate(&info).unwrap_err();
        assert!(err.contains("不符"), "pid 复用应以身份不符拒绝: {err}");

        info.process_start = 0;
        let err = force_terminate(&info).unwrap_err();
        assert!(err.contains("核验"), "未登记创建时间应拒绝: {err}");
        std::thread::sleep(Duration::from_millis(200));
        assert!(
            child.try_wait().unwrap().is_none(),
            "拒绝后子进程必须仍在运行"
        );

        info.process_start = start;
        force_terminate(&info).expect("身份一致应终止成功");
        assert!(wait_child_exit(&mut child), "终止后子进程应退出");
    }

    #[test]
    fn registry_find_pid_returns_record_with_actual_addr() {
        let dir = tempfile::tempdir().unwrap();
        let mut info = sample_info("59731");
        info.pid = 5151;
        write_instance_file_in(dir.path(), &info).unwrap();
        let found = registry_find_pid_in(dir.path(), 5151).expect("应按 pid 找到记录");
        assert_eq!(found.listen_addr, "127.0.0.1:59731");
        assert!(registry_find_pid_in(dir.path(), 5152).is_none());
        assert!(read_instance_file_in(dir.path(), "59731").is_some());
        assert!(read_instance_file_in(dir.path(), "59732").is_none());
    }

    /// 重拉/恢复落到新端口后退役旧端口记录：同端口不动；旧端口记录属于已死
    /// 进程 → .restore 与 .pid 一并删除；旧端口已被另一个在世实例接手（身份
    /// 核验通过，或旧版本记录 pid 在世无从核验）→ 记录归它，不动
    #[cfg(any(windows, target_os = "linux"))]
    #[test]
    fn retire_moved_port_records_only_touches_dead_owner() {
        let dir = tempfile::tempdir().unwrap();
        let write = |port: &str, pid: u32, process_start: u64| {
            let mut info = sample_info(port);
            info.pid = pid;
            info.process_start = process_start;
            write_instance_file_in(dir.path(), &info).unwrap();
            write_restore_file_in(dir.path(), port, &[], "").unwrap();
        };
        let has = |port: &str| {
            (
                restore_file_path_in(dir.path(), port).is_file(),
                instance_file_path_in(dir.path(), port).is_file(),
            )
        };

        write("59741", u32::MAX - 777, 123);
        assert!(!retire_moved_port_records_in(dir.path(), "59741", "59741"));
        assert_eq!(has("59741"), (true, true), "同端口不得退役");
        assert!(retire_moved_port_records_in(dir.path(), "59741", "59742"));
        assert_eq!(
            has("59741"),
            (false, false),
            "已死进程的旧端口记录应整体退役"
        );

        let mut child = spawn_unrelated_child();
        let pid = child.id();
        let start = crate::watchdog::process_start_time(pid).expect("子进程创建时间可查");
        write("59743", pid, start);
        assert!(!retire_moved_port_records_in(dir.path(), "59743", "59744"));
        assert_eq!(has("59743"), (true, true), "在世实例接手的端口不得退役");
        // 未登记创建时间：证明不了是哪个进程，按无主记录退役
        write("59745", pid, 0);
        assert!(retire_moved_port_records_in(dir.path(), "59745", "59746"));
        assert_eq!(has("59745"), (false, false));
        // 登记时间不符 = pid 已复用，记录的进程已死 → 退役
        write("59747", pid, start.wrapping_add(1));
        assert!(retire_moved_port_records_in(dir.path(), "59747", "59748"));
        assert_eq!(has("59747"), (false, false));

        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn registry_contains_pid_matches_by_parsed_record() {
        let dir = tempfile::tempdir().unwrap();
        let mut info = sample_info("59901");
        info.pid = 4242;
        write_instance_file_in(dir.path(), &info).unwrap();

        assert!(registry_contains_pid_in(dir.path(), 4242));
        assert!(!registry_contains_pid_in(dir.path(), 4243));
        // 非 .pid 后缀的文件不参与检索
        std::fs::write(dir.path().join("59901.restore"), "args").unwrap();
        assert!(registry_contains_pid_in(dir.path(), 4242));
    }

    #[test]
    fn registry_contains_pid_skips_corrupt_and_missing_dir() {
        let dir = tempfile::tempdir().unwrap();
        // 损坏记录（不可解析）跳过，不 panic 不误报
        std::fs::write(dir.path().join("59902.pid"), "not-json").unwrap();
        assert!(!registry_contains_pid_in(dir.path(), 1));
        // 目录不存在 → false（respawn 轮询的前置态）
        assert!(!registry_contains_pid_in(
            dir.path().join("no-such").as_path(),
            1
        ));
    }

    #[test]
    fn request_lines_round_trip_through_the_server_parser() {
        for op in IpcOp::ALL {
            let (dialect, parsed) = parse_request(&(op.request_line() + "\n")).unwrap();
            assert!(matches!(dialect, Dialect::V1));
            assert_eq!(parsed, op);
        }
        // 0.1.0 的请求没有 v：按 0.1.0 的格式应答，它的 stats 等同 ping
        let (dialect, op) = parse_request("{\"op\":\"stats\"}\n").unwrap();
        assert!(matches!(dialect, Dialect::Legacy));
        assert_eq!(op, IpcOp::Ping);
    }

    #[test]
    fn bad_requests_get_stable_error_codes() {
        let reply = |line: &str| -> serde_json::Value {
            serde_json::from_str(&parse_request(line).unwrap_err()).unwrap()
        };
        let code = |line: &str| {
            reply(line)["error"]["code"]
                .as_str()
                .unwrap_or("")
                .to_string()
        };
        assert_eq!(code("{\"v\":1,\"op\":\"what\"}\n"), "unknown_op");
        assert_eq!(code("{\"v\":2,\"op\":\"ping\"}\n"), "unsupported_version");
        assert_eq!(
            reply("{\"v\":2,\"op\":\"ping\"}\n")["error"]["supported"],
            serde_json::json!([1])
        );
        assert_eq!(code("not json\n"), "bad_request");
        assert_eq!(code("{\"v\":1}\n"), "bad_request");
        assert_eq!(
            code("{\"v\":1,\"op\":\"ping\",\"args\":3}\n"),
            "bad_request"
        );
        // 行没有正常结束（超长被截断，或对端提前断开）
        assert_eq!(code("{\"v\":1,\"op\":\"ping\"}"), "bad_request");
        // 0.1.0 请求里认不出的 op：回 0.1.0 形状的拒绝
        let legacy = reply("{\"op\":\"what\"}\n");
        assert_eq!(legacy["ok"], false);
        assert!(legacy.get("v").is_none(), "{legacy}");
    }

    #[test]
    fn client_reads_v1_errors_unknown_states_and_0_1_0_replies() {
        let err = parse_reply(r#"{"v":1,"ok":false,"error":{"code":"unknown_op","message":"x"}}"#)
            .unwrap_err();
        assert!(
            matches!(&err, IpcError::Remote { code, .. } if code == "unknown_op"),
            "{err}"
        );
        assert!(matches!(
            parse_reply(r#"{"v":2,"ok":true}"#),
            Err(IpcError::Protocol(_))
        ));
        // 更新版本的实例：不认识的状态读作 Unknown，不认识的字段忽略
        let newer = serde_json::json!({"v": 1, "ok": true, "extra": 1, "result": {
            "instance": sample_info("1"), "run_dir": "r", "state": "draining",
            "activity": {"last_request_at": 1, "requests_total": 0, "retries_total": 0,
                         "last_error": null},
            "ops": ["ping"], "future_field": true}});
        let status = parse_reply(&newer.to_string()).unwrap();
        assert_eq!(status.state, InstanceState::Unknown);
        // 0.1.0 的应答（没有 v）：swap_phase 映射成状态，身份字段原样带回
        let legacy = r#"{"ok":true,"info":{"pid":7,"version":"0.1.0","listen_addr":"127.0.0.1:1","config_path":"c","base_url":"b","started_at":5,"swap_phase":true,"process_start":9},"proto":2}"#;
        let status = parse_reply(legacy).unwrap();
        assert_eq!((status.instance.pid, status.instance.process_start), (7, 9));
        assert_eq!(status.state, InstanceState::SwapPrepared);
        assert!(matches!(
            parse_reply(r#"{"ok":false,"info":null,"proto":2}"#),
            Err(IpcError::Remote { .. })
        ));
    }

    #[test]
    fn instance_file_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let info = sample_info("45678");
        write_instance_file_in(dir.path(), &info).unwrap();
        let path = instance_file_path_in(dir.path(), "127.0.0.1:45678");
        assert_eq!(path.file_name().unwrap(), "45678.pid");
        let loaded: InstanceRecord =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(loaded.pid, 42);
        assert_eq!(loaded.listen_addr, "127.0.0.1:45678");
    }

    #[tokio::test]
    async fn control_ops_round_trip_over_a_real_endpoint() {
        let dir = tempfile::tempdir().unwrap();
        let endpoint = test_endpoint(dir.path(), "roundtrip");
        let mut record = sample_info("0");
        record.pid = std::process::id();
        let (state, mut rx) = control(record);
        state.stats.requests_total.store(7, Ordering::Relaxed);
        state.stats.retries_total.store(2, Ordering::Relaxed);
        state
            .stats
            .record_error("上游返回 502 Bad Gateway（错误内容，重试）");
        let server = tokio::spawn(serve_endpoint(endpoint.clone(), state.clone()));
        tokio::time::sleep(Duration::from_millis(100)).await;

        // ping：状态快照带实时观测值与能力清单
        let status = request_raw(&endpoint, IpcOp::Ping).await.unwrap();
        assert_eq!(status.instance.pid, std::process::id());
        assert_eq!(status.state, InstanceState::Serving);
        assert_eq!(status.run_dir, "/test/run");
        assert_eq!(status.ops, ["ping", "shutdown", "prepare_swap"]);
        assert_eq!(status.activity.requests_total, 7);
        assert_eq!(status.activity.retries_total, 2);
        let last = status.activity.last_error.expect("应带最近错误");
        assert!(last.message.contains("502") && last.at > 0);
        // 还没收到过请求：最近活动取启动时刻，不是 0
        assert_eq!(status.activity.last_request_at, status.instance.started_at);

        // prepare_swap：应答里立即是 swap_prepared（安装器的 ACK 判定），之后也是
        let swap = request_raw(&endpoint, IpcOp::PrepareSwap).await.unwrap();
        assert_eq!(swap.state, InstanceState::SwapPrepared);
        let again = request_raw(&endpoint, IpcOp::Ping).await.unwrap();
        assert_eq!(again.state, InstanceState::SwapPrepared);

        // 0.1.0 客户端（请求不带 v）拿到 0.1.0 形状的应答，swap_phase 即 ACK
        let (line, _) = imp::exchange(&endpoint, r#"{"op":"ping"}"#).await.unwrap();
        let legacy: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert!(legacy.get("v").is_none(), "{line}");
        assert_eq!(legacy["ok"], true);
        assert_eq!(legacy["proto"], 2);
        assert_eq!(legacy["info"]["pid"], std::process::id());
        assert_eq!(legacy["info"]["swap_phase"], true);
        assert_eq!(legacy["info"]["requests_total"], 7);

        // shutdown：应答先到（状态 stopping），随后置位停止信号
        let stopping = request_raw(&endpoint, IpcOp::Shutdown).await.unwrap();
        assert_eq!(stopping.state, InstanceState::Stopping);
        rx.changed().await.unwrap();
        assert!(*rx.borrow());
        // 退出中不再进入更换阶段
        let err = request_raw(&endpoint, IpcOp::PrepareSwap)
            .await
            .unwrap_err();
        assert!(
            matches!(&err, IpcError::Remote { code, .. } if code == "invalid_state"),
            "{err}"
        );

        server.abort();
    }

    #[test]
    fn ipc_stats_record_error_overwrites_and_truncates() {
        let stats = IpcStats::default();
        assert!(stats.last_error.lock().unwrap().is_none());
        stats.record_error("第一次错误");
        stats.record_error("第二次错误");
        {
            let slot = stats.last_error.lock().unwrap();
            let (msg, _) = slot.as_ref().unwrap();
            assert!(msg.contains("第二次"), "应覆盖为最近一次: {msg}");
        }
        // 超长错误截断到 200 字符（按 char，多字节不撕裂）
        let long = "长".repeat(500);
        stats.record_error(&long);
        let slot = stats.last_error.lock().unwrap();
        let (msg, _) = slot.as_ref().unwrap();
        assert_eq!(msg.chars().count(), 200);
    }

    #[tokio::test]
    async fn ipc_ping_fails_when_no_instance() {
        // 不存在的端点：ping 必须报错（= 端口上没有 aProxy）
        let run = tempfile::tempdir().unwrap();
        let port = format!("599{:02}", std::process::id() % 100);
        assert!(ipc_ping_in(run.path(), &port).await.is_err());
    }

    #[test]
    fn home_id_separates_homes_and_is_stable_across_spellings() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        // 两个 home 同一端口的端点必须不同：Windows 管道名全机共享，相同就
        // 意味着一个 home 的命令能控制另一个 home 的实例
        assert_ne!(
            endpoint_for_in(&a.path().join("run"), "12345"),
            endpoint_for_in(&b.path().join("run"), "12345")
        );
        // run 目录建出前后是同一个标识：守护可能先于目录创建算出端点名
        let run = a.path().join("run");
        let before = home_id(&run);
        std::fs::create_dir_all(&run).unwrap();
        assert_eq!(before, home_id(&run));
        // 同一目录的另一种写法得到同一个标识
        assert_eq!(home_id(&run), home_id(&a.path().join(".").join("run")));
        #[cfg(windows)]
        {
            let spelled = run.display().to_string().to_uppercase().replace('\\', "/");
            assert_eq!(home_id(&run), home_id(std::path::Path::new(&spelled)));
        }
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn legacy_pipe_is_reached_only_for_a_matching_local_record() {
        // 扮演一个只在 0.1.0 旧管道名上应答的实例。「端口」取非数字的测试名，
        // 旧管道名就不可能与机器上真实实例的管道重名
        let run = tempfile::tempdir().unwrap();
        let port = format!("legacy-test-{}", std::process::id());
        let legacy = format!(r"\\.\pipe\aproxy-{port}");
        let serve_legacy =
            |info: InstanceRecord| tokio::spawn(serve_endpoint(legacy.clone(), control(info).0));
        // pid 必须是本测试进程：客户端会核对应答者自报的 pid 与管道服务端进程
        let mut live = sample_info(&port);
        live.pid = std::process::id();
        live.process_start = 1000;
        let server = serve_legacy(live.clone());
        tokio::time::sleep(Duration::from_millis(100)).await;

        // 本 run 目录没登记该端口：旧管道上的应答者可能属于任何 home，不认
        assert!(ipc_ping_in(run.path(), &port).await.is_err());

        // 登记的是另一个进程（别的 home 在同端口上的实例）：不认
        let mut other = live.clone();
        other.pid = live.pid + 1;
        write_instance_file_in(run.path(), &other).unwrap();
        assert!(ipc_ping_in(run.path(), &port).await.is_err());

        // pid 相同、创建时间不同（pid 已被复用）：不认
        let mut reused = live.clone();
        reused.process_start = 2000;
        write_instance_file_in(run.path(), &reused).unwrap();
        assert!(ipc_ping_in(run.path(), &port).await.is_err());

        // 记录与应答者一致：经旧管道找到它（升级窗口里新 CLI 找到本 home
        // 里还在跑 0.1.0 的实例）
        write_instance_file_in(run.path(), &live).unwrap();
        assert_eq!(
            ipc_ping_in(run.path(), &port).await.unwrap().instance.pid,
            live.pid
        );

        // 记录与应答者都没有创建时间：pid 相同也无从排除复用，不认
        server.abort();
        let _ = server.await;
        let mut unverifiable = live.clone();
        unverifiable.process_start = 0;
        let server = serve_legacy(unverifiable.clone());
        tokio::time::sleep(Duration::from_millis(100)).await;
        write_instance_file_in(run.path(), &unverifiable).unwrap();
        assert!(ipc_ping_in(run.path(), &port).await.is_err());

        server.abort();
    }

    /// 测试专用端点：Windows 管道名带测试名与进程号（不会与真实实例重名），
    /// unix 的 socket 放在测试目录里
    fn test_endpoint(dir: &std::path::Path, name: &str) -> String {
        #[cfg(windows)]
        {
            let _ = dir;
            format!(r"\\.\pipe\aproxy-test-{name}-{}", std::process::id())
        }
        #[cfg(unix)]
        {
            dir.join(format!("{name}.sock")).display().to_string()
        }
    }

    #[tokio::test]
    async fn client_rejects_a_responder_whose_pid_is_not_the_peer() {
        // 冒充者：在端点上应答，自报的 pid（42）却不是它自己。客户端必须拒收——
        // 普查、stop 与 0.1.0 回退都按应答里的 pid 认实例
        let dir = tempfile::tempdir().unwrap();
        let endpoint = test_endpoint(dir.path(), "peer");
        let server = tokio::spawn(serve_endpoint(
            endpoint.clone(),
            control(sample_info("0")).0,
        ));
        tokio::time::sleep(Duration::from_millis(100)).await;
        let err = request_raw(&endpoint, IpcOp::Ping).await.unwrap_err();
        assert!(err.to_string().contains("不一致"), "{err}");
        server.abort();
    }

    #[tokio::test]
    async fn binding_a_live_endpoint_fails_and_leaves_its_owner_answering() {
        // 同一 home 同端口的第二个实例：不得与第一个分摊连接（Windows），也不得
        // 删掉它的 socket 抢过来（unix），只能创建失败；第一个照常应答
        let dir = tempfile::tempdir().unwrap();
        let endpoint = test_endpoint(dir.path(), "dup");
        let mut info = sample_info("0");
        info.pid = std::process::id();
        let server = tokio::spawn(serve_endpoint(endpoint.clone(), control(info).0));
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(imp::bind(&endpoint).await.is_err());
        assert!(request_raw(&endpoint, IpcOp::Ping).await.is_ok());
        server.abort();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn binding_over_a_stale_socket_file_succeeds() {
        // 崩溃残留：socket 文件还在，但没有进程在听——清掉重建
        let dir = tempfile::tempdir().unwrap();
        let endpoint = test_endpoint(dir.path(), "stale");
        drop(std::os::unix::net::UnixListener::bind(&endpoint).unwrap());
        assert!(std::path::Path::new(&endpoint).exists());
        // 客户端眼里，崩溃实例留下的 socket 就是「这里没有实例」：stop 据此确认
        // 已退出，普查据此转入进程身份核验
        let probe = request_raw(&endpoint, IpcOp::Ping).await;
        assert!(
            matches!(probe, Err(IpcError::Unreachable(_))),
            "{:?}",
            probe.err()
        );
        let bound = imp::bind(&endpoint).await;
        assert!(bound.is_ok(), "{:?}", bound.err());
    }

    #[tokio::test]
    async fn survey_does_not_credit_a_record_with_another_processs_answer() {
        // 记录说端口上是进程 X，端点上应答的却是另一个进程：不能把应答者当成
        // 这条记录的实例列出，也不能删记录（它可能正被应答者重写）
        let dir = tempfile::tempdir().unwrap();
        let port = format!("survey-{}", std::process::id());
        let mut live = sample_info(&port);
        live.pid = std::process::id();
        let server = tokio::spawn(serve_endpoint(
            endpoint_for_in(dir.path(), &port),
            control(live.clone()).0,
        ));
        tokio::time::sleep(Duration::from_millis(100)).await;

        let mut stale = live.clone();
        stale.pid = u32::MAX - 778;
        stale.process_start = 123;
        write_instance_file_in(dir.path(), &stale).unwrap();
        let survey = survey_instances_in(dir.path()).await;
        assert!(survey.responsive.is_empty(), "{:?}", survey.responsive);
        assert!(survey.unresponsive.is_empty(), "{:?}", survey.unresponsive);
        assert!(read_instance_file_in(dir.path(), &port).is_some());

        // 记录与应答者一致：照常列出
        write_instance_file_in(dir.path(), &live).unwrap();
        let survey = survey_instances_in(dir.path()).await;
        assert_eq!(survey.responsive.len(), 1);
        assert_eq!(survey.responsive[0].instance.pid, live.pid);
        server.abort();
    }

    #[tokio::test]
    async fn list_instances_cleans_dead_records() {
        // 写一条指向不存在实例的注册记录 → list 应清理它且不返回
        let dir = tempfile::tempdir().unwrap();
        let dead = sample_info("59801");
        write_instance_file_in(dir.path(), &dead).unwrap();
        let path = instance_file_path_in(dir.path(), "127.0.0.1:59801");
        let listed = list_instances_in(dir.path()).await;
        assert!(
            listed
                .iter()
                .all(|i| i.instance.listen_addr != "127.0.0.1:59801")
        );
        assert!(!path.exists(), "死亡实例的注册记录应被清理");
    }

    #[tokio::test]
    async fn survey_keeps_records_of_live_but_unresponsive_instances() {
        // 进程还在、端点却无人应答（挂死的形态）：用本测试进程自身冒充——
        // pid 与创建时间都核验得上，但 59802 上没有 IPC 端点
        let dir = tempfile::tempdir().unwrap();
        let pid = std::process::id();
        let start = crate::watchdog::process_start_time(pid).expect("读不到自身创建时间");
        let mut hung = sample_info("59802");
        hung.pid = pid;
        hung.process_start = start;
        write_instance_file_in(dir.path(), &hung).unwrap();
        let path = instance_file_path_in(dir.path(), "127.0.0.1:59802");

        let survey = survey_instances_in(dir.path()).await;
        assert!(survey.responsive.is_empty());
        assert_eq!(survey.unresponsive.len(), 1, "挂死实例应归入 unresponsive");
        assert_eq!(survey.unresponsive[0].pid, pid);
        assert!(path.exists(), "进程仍在的记录不能因 ping 失败被删");

        // 同一 pid、创建时间对不上 = pid 已被复用，记录的进程早已死亡 → 清理
        hung.process_start = start + 1;
        write_instance_file_in(dir.path(), &hung).unwrap();
        let survey = survey_instances_in(dir.path()).await;
        assert!(survey.unresponsive.is_empty());
        assert!(!path.exists(), "pid 被复用的记录应被清理");
    }

    #[test]
    fn restore_file_roundtrip_and_cleanup() {
        let dir = tempfile::tempdir().unwrap();
        let args = vec![
            "--config".to_string(),
            "C:/tmp/cfg.toml".to_string(),
            "--api-key".to_string(),
            "sk-test".to_string(),
        ];
        write_restore_file_in(dir.path(), "127.0.0.1:59805", &args, "C:/tmp/old.log").unwrap();
        let path = restore_file_path_in(dir.path(), "127.0.0.1:59805");
        assert_eq!(path.file_name().unwrap(), "59805.restore");

        // 覆盖写：同端口再次启动应以最新参数为准（log_path 同步覆盖）
        let args2 = vec!["--config".to_string(), "C:/tmp/new.toml".to_string()];
        write_restore_file_in(dir.path(), "127.0.0.1:59805", &args2, "C:/tmp/new.log").unwrap();

        let entries = list_restore_entries_in(dir.path());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].port, "59805");
        assert_eq!(entries[0].args, args2);
        assert_eq!(entries[0].log_path, "C:/tmp/new.log");

        remove_restore_file_in(dir.path(), "127.0.0.1:59805");
        assert!(!path.exists());
        assert!(list_restore_entries_in(dir.path()).is_empty());
    }

    #[test]
    fn restore_list_cleans_corrupted_record() {
        let dir = tempfile::tempdir().unwrap();
        let path = restore_file_path_in(dir.path(), "127.0.0.1:59806");
        std::fs::write(&path, "{not-json").unwrap();
        assert!(list_restore_entries_in(dir.path()).is_empty());
        assert!(!path.exists(), "损坏的恢复记录应被清理");
        // 缺字段同样无法忠实恢复：只有 args 的数组形态不是有效记录
        std::fs::write(&path, r#"["--config","C:/tmp/c.toml"]"#).unwrap();
        assert!(list_restore_entries_in(dir.path()).is_empty());
        assert!(!path.exists(), "缺字段的恢复记录应被清理");
    }

    #[test]
    fn restore_entry_without_config_is_listed() {
        // 无 --config 参数的实例（纯默认配置）也应能列出与恢复
        let dir = tempfile::tempdir().unwrap();
        write_restore_file_in(dir.path(), "127.0.0.1:59807", &[], "").unwrap();
        let entries = list_restore_entries_in(dir.path());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].port, "59807");
        assert!(entries[0].args.is_empty());
    }

    #[test]
    fn orphan_log_cleanup_keeps_referenced_and_startup() {
        // 新判据（引用集）：日志名随机化后不再携带归属，保留 = 活实例上报的
        // log_path ∪ .restore 记录的 log_path ∪ startup.log；其余 .log 删除；
        // 非 .log 文件一律不动。
        let logs = tempfile::tempdir().unwrap();
        let live_log = logs.path().join("1f3a2b-00ab.log");
        let crash_log = logs.path().join("5c6d7e-11cd.log");
        let stale_log = logs.path().join("9900aa-22ef.log");
        for path in [&live_log, &crash_log, &stale_log] {
            std::fs::write(path, "x").unwrap();
        }
        std::fs::write(logs.path().join("startup.log"), "x").unwrap();
        std::fs::write(logs.path().join("weird.log"), "x").unwrap();
        std::fs::write(logs.path().join("not-a-log.txt"), "x").unwrap();

        let mut live = sample_info("59811");
        live.log_path = live_log.display().to_string();
        let pending = vec![RestoreEntry {
            port: "59812".into(),
            args: vec![],
            log_path: crash_log.display().to_string(),
        }];
        cleanup_orphan_logs_in(logs.path(), &[live], &pending);
        assert!(live_log.exists(), "活实例引用的日志不应被删除");
        assert!(crash_log.exists(), ".restore 引用的崩溃日志不应被删除");
        assert!(
            logs.path().join("startup.log").exists(),
            "startup.log 不应被孤儿清理删除"
        );
        assert!(
            logs.path().join("not-a-log.txt").exists(),
            "非 .log 文件不应被删除（防御保留）"
        );
        assert!(!stale_log.exists(), "无引用的孤儿日志应被删除");

        // 大小写/分隔符差异经 path_match_key 归一后同样引用（Windows 注册表
        // 路径表示与 read_dir 枚举可能不同形，字面比较会误删活实例日志）
        let logs2 = tempfile::tempdir().unwrap();
        let mut live2 = sample_info("59811");
        live2.log_path = logs2
            .path()
            .join("CaseCheck-00aa.log")
            .display()
            .to_string()
            .replace('\\', "/");
        std::fs::write(logs2.path().join("CaseCheck-00aa.log"), "x").unwrap();
        cleanup_orphan_logs_in(logs2.path(), &[live2], &[]);
        assert!(
            logs2.path().join("CaseCheck-00aa.log").exists(),
            "归一后同一路径（分隔符差异）不应被误删"
        );

        // 空串 log_path（前台实例）不参与引用集：其旧日志按无主处理
        let logs3 = tempfile::tempdir().unwrap();
        let orphan = logs3.path().join("ff00aa-33ff.log");
        std::fs::write(&orphan, "x").unwrap();
        cleanup_orphan_logs_in(logs3.path(), &[sample_info("59811")], &[]);
        assert!(!orphan.exists(), "空串 log_path 的实例不应保住孤儿日志");
    }
}
