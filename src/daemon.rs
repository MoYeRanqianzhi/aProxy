//! 守护进程编排：后台启动、实例注册表、IPC 控制。
//!
//! 进程模型：`aproxy`（默认）= 后台启动——父进程做预检与就绪等待，
//! 实际服务由分离的子进程（`aproxy __serve`）承载。子进程以 CREATE_NO_WINDOW
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
//! 实例注册表：`~/.aproxy/run/<port>.instance.json`（JSON），仅供 `status`/`stop` 枚举
//! 实例；存活一律以 IPC ping 为准，不信任注册文件与 pid 本身。

use serde::{Deserialize, Serialize};
use std::{io, path::PathBuf, sync::Arc, time::Duration};

/// 现在的时刻（Unix 毫秒）。记录、快照与事件里的时刻都用这个单位。
pub fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 运行期记录的目录：`<home>/run/`。home 是唯一的根，寻址只有 `APROXY_HOME` 这一个变量。
pub fn run_dir() -> PathBuf {
    aproxy_ipc::home::home().join("run")
}

/// 守护进程日志目录：`<home>/logs/`
pub fn logs_dir() -> PathBuf {
    aproxy_ipc::home::home().join("logs")
}

/// spool 临时文件的根目录：`<home>/spool/`。每个实例进程在它下面有自己的一个子目录
/// （见 [`own_spool_dir`]）。
pub fn spool_root() -> PathBuf {
    aproxy_ipc::home::home().join("spool")
}

/// 一个进程的 spool 子目录名：`<pid>-<启动标记的短哈希>`（哈希是 8 位小写十六进制）。
///
/// 按进程而不是按端口分：目录按端口命名时，同一端口上第二次启动的进程会在 bind 失败之前
/// 清掉正在运行的那个实例的缓存文件。带上启动标记（台账用的那一个，见
/// [`aproxy_ipc::ledger::start_marker`]）是为了防 pid 复用——pid 相同而标记不同的目录
/// 属于早已不在的进程。读不到标记的平台按空串算哈希，名字仍然确定。
pub fn spool_dir_name(pid: u32, start: Option<&str>) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(start.unwrap_or("").as_bytes());
    format!(
        "{pid}-{:02x}{:02x}{:02x}{:02x}",
        digest[0], digest[1], digest[2], digest[3]
    )
}

/// 本进程的 spool 目录：`<home>/spool/<pid>-<启动标记的短哈希>/`。
pub fn own_spool_dir() -> PathBuf {
    let pid = std::process::id();
    spool_root().join(spool_dir_name(
        pid,
        aproxy_ipc::ledger::start_marker(pid).as_deref(),
    ))
}

/// 建出实例自己的 spool 目录（开始接收请求之前调用）。失败只告警：磁盘缓存不可用时
/// 请求体与响应体留在内存里。
pub fn prepare_spool_dir(dir: &std::path::Path) {
    if let Err(e) = std::fs::create_dir_all(dir) {
        tracing::warn!(error = %e, dir = %dir.display(), "spool 目录创建失败（磁盘缓存将不可用，回退纯内存）");
    }
}

/// 删掉一个 spool 目录里的临时文件（`*.spooltmp`），目录空了就连目录一起删，返回目录
/// 是否已删。只删这一种文件：意外出现的其他内容保留，目录也就留着（不扩大删除面）。
pub fn remove_spool_dir(dir: &std::path::Path) -> bool {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "spooltmp")
            && let Err(e) = std::fs::remove_file(&path)
        {
            tracing::warn!(error = %e, path = %path.display(), "spool 残留清理失败");
        }
    }
    std::fs::remove_dir(dir).is_ok()
}

/// 清掉别的进程留下的 spool 目录，返回删掉的目录。spool 里是请求与响应的全文，进程崩溃
/// 或被强杀之后不该一直留在盘上。只看名字是 `<pid>-<8 位十六进制>` 的子目录，删除的条件
/// 是下面两条之一：
/// - 这个 pid 现在的启动标记算出来的名字与目录名不同（pid 已被别的进程复用）；
/// - 读不到这个 pid 的启动标记，并且能确认这个 pid 上已经没有进程。
///
/// 读不到标记、又确认不了进程不在时（本平台没有定义标记，或没有权限）保留。别的名字
/// （包括旧版本按端口命名的目录）不碰。
pub fn collect_stale_spool_in(root: &std::path::Path) -> Vec<PathBuf> {
    let mut removed = Vec::new();
    for entry in std::fs::read_dir(root).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(pid) = spool_dir_owner(&name) else {
            continue;
        };
        let stale = match aproxy_ipc::ledger::start_marker(pid) {
            Some(marker) => spool_dir_name(pid, Some(&marker)) != name,
            None => crate::process::process_exited(pid) == Some(true),
        };
        let path = entry.path();
        if stale && path.is_dir() && remove_spool_dir(&path) {
            removed.push(path);
        }
    }
    removed
}

/// 目录名是 `<pid>-<8 位小写十六进制>` 时返回其中的 pid。
fn spool_dir_owner(name: &str) -> Option<u32> {
    let (pid, hash) = name.split_once('-')?;
    (hash.len() == 8
        && hash
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)))
    .then_some(())?;
    pid.parse().ok()
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
/// 名字怎样取、为什么带上 run 目录的标识，见 [`aproxy_ipc::endpoint::instance`]。
pub fn endpoint_for_in(run_dir: &std::path::Path, port: &str) -> String {
    aproxy_ipc::endpoint::instance(run_dir, port)
}

/// 从监听地址提取端口（IPC 端点名、日志/注册文件名共用）
pub fn port_of(listen_addr: &str) -> &str {
    listen_addr.rsplit(':').next().unwrap_or("unknown")
}

/// 实例记录的 schema。读到更高的 schema 时不解读内容，那份记录算读不懂。
pub const INSTANCE_SCHEMA: u32 = 1;

/// 实例记录：`run/<端口>.instance.json` 的内容去掉开头的 `schema`，也是 IPC 状态快照里的
/// `instance`。实例在 bind 之后写下它，正常退出时删掉。
///
/// 原地升级时新旧两个版本同时在跑，会读对方写下的记录：已有字段不改名、不删、不改类型，
/// 以后新增的字段只能是可缺省的。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct InstanceRecord {
    pub pid: u32,
    /// 实例进程的启动标记（台账用的同一个，见 [`aproxy_ipc::ledger::start_marker`]），实例
    /// 注册时现算现写，只与现算的值比对，是不透明的身份锚点。
    ///
    /// 进程身份 = 「这个 pid 现在是不是写下这条记录的那个实例」，与二进制叫什么无关：pid
    /// 被系统回收再分配给任何进程后，新进程的标记必然不同。看门狗的收养/处决/选举与
    /// `stop --force` 都以「pid + 本字段」比对防 pid 复用误杀（见 process::record_identity）。
    /// 空串 = 平台算不出标记，这样的记录无法核验身份，一律不据它动手。
    pub start: String,
    /// 这一次运行的标识：同一个端口上先后启动的实例各不相同。界面与请求记录据此认出
    /// 「实例重启过」
    pub instance_id: String,
    pub version: String,
    pub listen_addr: String,
    /// 配置文件的绝对路径；没有配置文件、只靠覆盖参数与 settings `defaults` 启动的实例为 null
    pub config_path: Option<String>,
    /// 脱敏后的上游地址，只用于展示
    pub base_url: String,
    /// 启动时刻（Unix 毫秒）
    pub started_at: u64,
    /// 本实例日志文件的绝对路径（随机命名或用户自定义 log_file）。客户端（aproxy logs /
    /// start 成功提示）一律向实例或注册表索取，不按端口拼路径——端口是易变标识。
    /// 前台实例的日志走控制台，为 null
    pub log_path: Option<String>,
    /// 前台实例：占着启动它的终端，没有日志文件，也没有期望状态记录
    pub foreground: bool,
}

/// 实例记录在盘上的形状：`schema` 在前，其余就是 [`InstanceRecord`]。
#[derive(Serialize, Deserialize)]
struct InstanceFile {
    schema: u32,
    #[serde(flatten)]
    record: InstanceRecord,
}

/// 解析一份实例记录的文本。读不懂（不是这个形状，或 schema 比本版本高）→ None。
pub fn parse_instance_file(text: &str) -> Option<InstanceRecord> {
    serde_json::from_str::<InstanceFile>(text)
        .ok()
        .filter(|file| file.schema <= INSTANCE_SCHEMA)
        .map(|file| file.record)
}

/// 新的一次运行的标识：启动时刻的纳秒数（十六进制）接 pid 的低 16 位，与随机日志名同源。
pub fn new_instance_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{nanos:x}{:04x}", std::process::id() & 0xffff)
}

/// IPC 状态快照：v1 每个成功应答的 `result`，所有 op 都回它。
///
/// 应答只增字段：以后追加的字段在这里一律带 `#[serde(default)]`，读不到就是缺省值；
/// 不认识的字段忽略。
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct InstanceStatus {
    pub instance: InstanceRecord,
    pub state: InstanceState,
    /// 实例的行为摘要：默认的投递方式、缓存、保活与规则数
    pub behavior: Behavior,
    pub activity: Activity,
    /// 实例支持的 op。调用较新的、有副作用的 op 之前先查这里
    pub ops: Vec<String>,
    /// 实例认得的 op 参数，名字是 `<op>.<参数>`，例如 `shutdown.keep_restore`。依赖某个
    /// 参数生效的调用方先查这里，再决定带不带它
    #[serde(default)]
    pub features: Vec<String>,
}

/// 实例的行为摘要（快照里的 `behavior`）。枚举值按字符串传：更新的版本多出一种取值时，
/// 今天的读者照样读得出整份快照。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Behavior {
    /// 请求的默认投递方式：`retry`（缓冲后发往上游、失败重试）或 `forward`（边收边转）
    pub delivery: String,
    /// 大于 1 MiB 的请求体与响应体是否落盘
    pub disk_cache: bool,
    pub keepalive: KeepaliveBehavior,
    /// 生效的重试规则条数
    pub retry_rules: u32,
}

/// 保活的摘要（`behavior.keepalive`）。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct KeepaliveBehavior {
    /// 哪些请求走保活：`accept`、`body_stream` 或 `any`
    pub trigger: String,
    /// 心跳间隔（秒）；0 = 保活关闭
    pub interval_secs: u64,
}

impl Behavior {
    /// 从生效的配置取摘要。
    pub fn of(cfg: &crate::config::Config) -> Self {
        Behavior {
            delivery: if cfg.forward_only_enabled() {
                "forward"
            } else {
                "retry"
            }
            .to_string(),
            disk_cache: cfg.disk_cache_enabled(),
            keepalive: KeepaliveBehavior {
                trigger: cfg.keepalive_trigger().as_str().to_string(),
                interval_secs: cfg.keepalive_interval().as_secs(),
            },
            retry_rules: cfg.retry_rules.len() as u32,
        }
    }
}

/// 能力名：shutdown 认 `keep_restore` 参数。
pub const FEATURE_SHUTDOWN_KEEP_RESTORE: &str = "shutdown.keep_restore";
/// 能力名：shutdown 认 `if_idle_secs` 参数。
pub const FEATURE_SHUTDOWN_IF_IDLE_SECS: &str = "shutdown.if_idle_secs";
/// 能力名：prepare_swap 认 `ttl_secs` 参数。
pub const FEATURE_PREPARE_SWAP_TTL_SECS: &str = "prepare_swap.ttl_secs";

/// 实例此刻所处的阶段。
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InstanceState {
    /// 控制端点已经在应答，但还没开始接收请求
    Starting,
    Serving,
    /// install 广播 prepare_swap 之后、重启之前（内存态：重启自然清除，到期也会自己回到
    /// `serving`）。安装器的 ACK 判定就是读到这个状态
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
    /// 最近一次收到客户端请求的时刻（Unix 毫秒）；还没收到过请求时为启动时刻
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
    /// 发生时刻（Unix 毫秒）
    pub at: u64,
}

/// 控制协议版本，请求与应答都带它（字段 `v`）。只有语义不兼容的变更才递增；
/// 加 op、加可选参数、加应答字段、加错误码、加状态值都不递增——双方都忽略
/// 不认识的字段，不认识的状态读作 `Unknown`。
pub const IPC_VERSION: u32 = 1;

/// prepare_swap 不带 `ttl_secs` 时，更换阶段保持多久（秒）。
pub const DEFAULT_SWAP_TTL_SECS: u64 = 600;

/// 控制操作。线上是一行 JSON：`{"v":1,"op":"<snake_case 名>"}`，可带
/// `args` 对象。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IpcOp {
    /// 读状态快照（status、start 预检、就绪等待）
    Ping,
    /// 优雅停止：应答先发出，随后开始退出。重复调用结果相同。
    ///
    /// `keep_restore` 为 false（`aproxy stop` 的语义）时，实例在应答之前先删掉自己的期望
    /// 状态记录：用户要它停下，restore 与看护者不该再把它拉回来。为 true（restart、安装的
    /// 滚动重启）时记录留着：调用方接着就要用它拉起新实例，中途崩溃了记录也还在。
    ///
    /// `if_idle_secs` 给了时，只有实例已经闲置至少这么多秒才停，否则回
    /// `condition_not_met`（`details.idle_ms` 是它实际闲置了多久）、什么都不做。判断与停止
    /// 在实例里一次完成：调用方先查闲置再发停止的话，中间到达的请求会被打断。
    Shutdown {
        keep_restore: bool,
        if_idle_secs: Option<u64>,
    },
    /// install 广播：进入二进制更换阶段。实例把它写进可观测的状态
    /// （`swap_prepared`），应答里立即可见——表达的是「进入阶段」而非口头 ok。
    /// 阶段保持 `ttl_secs` 秒（不给按 [`DEFAULT_SWAP_TTL_SECS`]），到期自己回到 `serving`：
    /// 安装者中途没了的话，实例不会永远停在这个状态。重复调用重新计时
    PrepareSwap { ttl_secs: Option<u64> },
}

impl IpcOp {
    /// 全部 op 的名字（快照里的 `ops`）
    const NAMES: [&'static str; 3] = ["ping", "shutdown", "prepare_swap"];

    /// 本版本认得的 op 参数（快照里的 `features`）
    const FEATURES: [&'static str; 3] = [
        FEATURE_SHUTDOWN_IF_IDLE_SECS,
        FEATURE_SHUTDOWN_KEEP_RESTORE,
        FEATURE_PREPARE_SWAP_TTL_SECS,
    ];

    /// `aproxy stop` 用的 shutdown：不留期望状态记录
    pub const STOP: IpcOp = IpcOp::Shutdown {
        keep_restore: false,
        if_idle_secs: None,
    };

    /// 为了马上再拉起而停（restart、滚动重启）：期望状态记录留着
    pub const STOP_KEEPING_RESTORE: IpcOp = IpcOp::Shutdown {
        keep_restore: true,
        if_idle_secs: None,
    };

    /// 进入更换阶段，按默认时长保持
    pub const PREPARE_SWAP: IpcOp = IpcOp::PrepareSwap { ttl_secs: None };

    pub fn name(self) -> &'static str {
        match self {
            IpcOp::Ping => "ping",
            IpcOp::Shutdown { .. } => "shutdown",
            IpcOp::PrepareSwap { .. } => "prepare_swap",
        }
    }

    /// 按名字与 `args` 还原一个 op。不认识的名字、不认识的参数、类型或取值不对的参数，
    /// 各有各的拒绝。
    fn from_request(
        name: &str,
        args: Option<&serde_json::Map<String, serde_json::Value>>,
    ) -> Result<Self, Rejection> {
        let known: &[&str] = match name {
            "ping" => &[],
            "shutdown" => &["keep_restore", "if_idle_secs"],
            "prepare_swap" => &["ttl_secs"],
            _ => return Err(Rejection::UnknownOp(name.to_string())),
        };
        let unknown: Vec<String> = args
            .into_iter()
            .flat_map(|args| args.keys())
            .filter(|key| !known.contains(&key.as_str()))
            .cloned()
            .collect();
        if !unknown.is_empty() {
            return Err(Rejection::UnsupportedArg(unknown));
        }
        let arg = |key: &str| args.and_then(|args| args.get(key));
        let boolean = |key: &'static str| match arg(key) {
            None => Ok(None),
            Some(value) => value.as_bool().map(Some).ok_or(Rejection::BadRequest {
                reason: "invalid_arg",
                arg: Some(key),
            }),
        };
        let seconds =
            |key: &'static str, min: u64| match arg(key) {
                None => Ok(None),
                Some(value) => value.as_u64().filter(|secs| *secs >= min).map(Some).ok_or(
                    Rejection::BadRequest {
                        reason: "invalid_arg",
                        arg: Some(key),
                    },
                ),
            };
        Ok(match name {
            "ping" => IpcOp::Ping,
            "shutdown" => IpcOp::Shutdown {
                keep_restore: boolean("keep_restore")?.unwrap_or(false),
                if_idle_secs: seconds("if_idle_secs", 0)?,
            },
            // 时长为 0 的更换阶段一进入就过期，等于没进入：不接受
            _ => IpcOp::PrepareSwap {
                ttl_secs: seconds("ttl_secs", 1)?,
            },
        })
    }

    /// 发往实例的请求行（不含换行）。参数只在不是缺省值时才带：不认识它的实例会拒绝整个
    /// 请求，所以带之前先查快照的 `features`。
    pub fn request_line(self) -> String {
        let mut args = serde_json::Map::new();
        match self {
            IpcOp::Ping => {}
            IpcOp::Shutdown {
                keep_restore,
                if_idle_secs,
            } => {
                if let Some(secs) = if_idle_secs {
                    args.insert("if_idle_secs".into(), secs.into());
                }
                if keep_restore {
                    args.insert("keep_restore".into(), true.into());
                }
            }
            IpcOp::PrepareSwap { ttl_secs } => {
                if let Some(secs) = ttl_secs {
                    args.insert("ttl_secs".into(), secs.into());
                }
            }
        }
        let mut line = serde_json::Map::new();
        line.insert("v".into(), IPC_VERSION.into());
        line.insert("op".into(), self.name().into());
        if !args.is_empty() {
            line.insert("args".into(), args.into());
        }
        serde_json::Value::Object(line).to_string()
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

/// 失败应答。`code` 是稳定的 snake_case 字符串，供程序判断；`details` 是这个码带的数据
/// （键与类型见码表 [`aproxy_ipc::code`]）；`message` 是英文，只给日志看，不要解析它。
#[derive(Serialize, Deserialize)]
struct WireError {
    code: String,
    message: String,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    details: serde_json::Value,
}

/// 错误码。客户端遇到不认识的码按一般的「实例拒绝」处理。
pub mod error_code {
    /// 不是 JSON、缺 `v`/`op`、行过长、参数的类型或取值不对（`details.reason`）
    pub const BAD_REQUEST: &str = "bad_request";
    /// `v` 不是实例支持的版本（`details.supported` 列出支持的）
    pub const UNSUPPORTED_VERSION: &str = "unsupported_version";
    /// 实例没有这个 op（没有副作用，可以放心用来探测能力）
    pub const UNKNOWN_OP: &str = "unknown_op";
    /// 带了实例不认识的参数（`details.args` 列出它们）；什么都没做
    pub const UNSUPPORTED_ARG: &str = "unsupported_arg";
    /// op 此刻不允许（例如已在退出时收到 prepare_swap；`details.state` 是此刻的状态）
    pub const INVALID_STATE: &str = "invalid_state";
    /// 带条件的 op 条件不成立（`shutdown{if_idle_secs}` 时实例不够闲；`details.idle_ms`）
    pub const CONDITION_NOT_MET: &str = "condition_not_met";
}

/// 实例对一个请求的拒绝：码、英文说明与 `details`。
#[derive(Debug, Clone, PartialEq, Eq)]
enum Rejection {
    /// `reason` 的词表：`incomplete_line`、`not_json`、`missing_v`、`missing_op`、
    /// `args_not_object`、`invalid_arg`（这时 `arg` 是那个参数的名字）
    BadRequest {
        reason: &'static str,
        arg: Option<&'static str>,
    },
    /// 请求里的 `v`（原样）不受支持
    UnsupportedVersion(String),
    UnknownOp(String),
    UnsupportedArg(Vec<String>),
    /// 此刻的状态不允许这个 op
    InvalidState(&'static str),
    /// 实例只闲置了 `idle_ms` 毫秒
    ConditionNotMet {
        idle_ms: u64,
    },
}

impl Rejection {
    fn bad(reason: &'static str) -> Self {
        Rejection::BadRequest { reason, arg: None }
    }

    /// 要回给客户端的那一行应答（不含换行）
    fn reply(&self) -> String {
        use serde_json::json;
        let (code, message, details) = match self {
            Rejection::BadRequest { reason, arg } => {
                let message = match (*reason, arg) {
                    ("incomplete_line", _) => "the request line is too long or incomplete".into(),
                    ("not_json", _) => "the request is not JSON".into(),
                    ("missing_v", _) => "the request has no v".into(),
                    ("missing_op", _) => "the request has no op".into(),
                    ("args_not_object", _) => "args must be an object".into(),
                    (_, Some(arg)) => format!("arg {arg} has the wrong type or value"),
                    (other, None) => other.to_string(),
                };
                let details = match arg {
                    Some(arg) => json!({ "reason": reason, "arg": arg }),
                    None => json!({ "reason": reason }),
                };
                (error_code::BAD_REQUEST, message, details)
            }
            Rejection::UnsupportedVersion(v) => (
                error_code::UNSUPPORTED_VERSION,
                format!("unsupported protocol version {v}; this instance supports {IPC_VERSION}"),
                json!({ "supported": [IPC_VERSION] }),
            ),
            Rejection::UnknownOp(op) => (
                error_code::UNKNOWN_OP,
                format!("unknown op: {op}"),
                json!({ "op": op }),
            ),
            Rejection::UnsupportedArg(args) => (
                error_code::UNSUPPORTED_ARG,
                format!("unknown arg: {}", args.join(", ")),
                json!({ "args": args }),
            ),
            Rejection::InvalidState(state) => (
                error_code::INVALID_STATE,
                format!("not allowed while the instance is {state}"),
                json!({ "state": state }),
            ),
            Rejection::ConditionNotMet { idle_ms } => (
                error_code::CONDITION_NOT_MET,
                format!("the instance has been idle for only {idle_ms} ms"),
                json!({ "idle_ms": idle_ms }),
            ),
        };
        serde_json::to_string(&WireResponse {
            v: IPC_VERSION,
            ok: false,
            result: None,
            error: Some(WireError {
                code: code.to_string(),
                message,
                details,
            }),
        })
        .expect("序列化 IPC 应答失败")
    }
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
    /// 实例明确拒绝了请求。`details` 是这个码带的数据（没有时为 null）
    Remote {
        code: String,
        message: String,
        details: serde_json::Value,
    },
}

impl std::fmt::Display for IpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreachable(e) | Self::Transient(e) | Self::Protocol(e) => f.write_str(e),
            Self::Remote { code, message, .. } => {
                write!(f, "实例拒绝了请求（{code}）：{message}")
            }
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

/// 按 run 目录与端口寻址：端点带 run 目录的命名空间（见 `endpoint_for_in`）。
async fn request_in(
    run_dir: &std::path::Path,
    port: &str,
    op: IpcOp,
) -> Result<InstanceStatus, IpcError> {
    request_raw(&endpoint_for_in(run_dir, port), op).await
}

/// 按显式端点发送 IPC 请求并等待应答（3 秒超时）。按实例寻址请用
/// `ipc_request_in`：它负责端点命名空间。
async fn request_raw(endpoint: &str, op: IpcOp) -> Result<InstanceStatus, IpcError> {
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
    // 应答者自报的 pid 必须就是连接对端的进程：普查与 stop 都按这个 pid 认
    // 实例，而端点名可以预测，任何本机进程都可能抢先占住它、冒充实例应答
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

/// 解析一行 v1 应答。
fn parse_reply(line: &str) -> Result<InstanceStatus, IpcError> {
    let value: serde_json::Value = serde_json::from_str(line)
        .map_err(|e| IpcError::Protocol(format!("应答不是 JSON：{e}")))?;
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
            details: error.details,
        }),
        _ => Err(IpcError::Protocol("应答缺少 result 或 error".to_string())),
    }
}

/// 本实例已独占创建、尚未开始接受连接的控制端点。守护先创建它、再写注册表
/// 与恢复记录（见 server::serve_forever）：有实例记录就意味着端点在应答
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
    /// 本实例的行为摘要（应答里的 `behavior`）
    pub behavior: Behavior,
    /// 本实例所属的 run 目录（期望状态记录写在这里）
    pub run_dir: String,
    /// 代理层的实时观测源，应答时现读
    pub stats: Arc<IpcStats>,
    /// 收到 shutdown 时置位，服务主循环据此优雅退出；已置位即「正在退出」
    pub on_shutdown: tokio::sync::watch::Sender<bool>,
    /// 「用户已经要它停下」：收到过不带 `keep_restore` 的 shutdown。期望状态记录的写与删
    /// 都在这把锁下进行，置位之后不再写——否则启动途中收到 stop 的实例，会在记录被删掉
    /// 之后又把它写回来，随后被 restore 或看护者拉起
    pub stop_requested: std::sync::Mutex<bool>,
}

impl ControlState {
    /// 本实例的端口（记录与端点都按它命名）
    fn port(&self) -> &str {
        port_of(&self.record.listen_addr)
    }

    /// 最近一次收到客户端请求的时刻（Unix 毫秒）；还没收到过请求时是启动时刻
    fn last_request_at(&self) -> u64 {
        self.stats
            .last_activity_ms
            .load(std::sync::atomic::Ordering::Relaxed)
            .max(self.record.started_at)
    }

    /// 写本实例的期望状态记录。已经收到过不带 `keep_restore` 的 shutdown 时不写，返回
    /// `Ok(false)`。
    pub fn write_restore(&self, spec: &crate::spec::InstanceSpec) -> io::Result<bool> {
        let stopped = self
            .stop_requested
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if *stopped {
            return Ok(false);
        }
        crate::spec::write_restore_in(std::path::Path::new(&self.run_dir), self.port(), spec)?;
        Ok(true)
    }

    /// 兑现不带 `keep_restore` 的 shutdown：记下「用户要它停下」，删掉期望状态记录。
    fn forget_restore(&self) {
        let mut stopped = self
            .stop_requested
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *stopped = true;
        crate::spec::remove_restore_in(std::path::Path::new(&self.run_dir), self.port());
    }
}

/// 在已创建的端点上提供控制服务，直到进程退出。
pub async fn serve_ipc(endpoint: IpcEndpoint, state: Arc<ControlState>) {
    imp::accept_loop(endpoint.0, state).await
}

/// 在给定端点名上创建并提供控制服务（测试用）。只有创建失败会返回。
#[cfg(test)]
async fn serve_endpoint(endpoint: String, state: Arc<ControlState>) -> io::Result<()> {
    let listener = imp::bind(&endpoint).await?;
    imp::accept_loop(listener, state).await;
    Ok(())
}

/// 实例的实时观测数据源：代理热路径写入，IPC 响应读取。
/// 全部 Relaxed——计数器只用于展示与告警，无跨线程因果序需求。
#[derive(Debug, Default)]
pub struct IpcStats {
    /// 最近一次收到客户端请求的时刻（Unix 毫秒；即 AppState::last_activity_ms，
    /// 同一 Arc 由 serve_forever 组装进来）
    pub last_activity_ms: Arc<std::sync::atomic::AtomicU64>,
    /// 累计收到客户端请求数
    pub requests_total: std::sync::atomic::AtomicU64,
    /// 累计上游重试次数（首轮之后的所有尝试）
    pub retries_total: std::sync::atomic::AtomicU64,
    /// 最近一次上游失败的简短摘要与发生时刻（打码后的短串、Unix 毫秒；None = 从未失败）
    pub last_error: std::sync::Mutex<Option<(String, u64)>>,
    /// 二进制更换阶段的到期时刻（Unix 毫秒）：prepare_swap 时写下，晚于现在就算在这个
    /// 阶段里。0 = 没进入过；重启自然清除
    pub swap_until_ms: std::sync::atomic::AtomicU64,
}

impl IpcStats {
    /// 记录一次上游失败（覆盖式：只保留最近一次）。`error` 须已打码。
    pub fn record_error(&self, error: &str) {
        let now = now_millis();
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

/// 注册表中登记的全部实例记录（只读 `*.instance.json`，不做 IPC 探活；读不懂的跳过），按 pid
/// 升序。看门狗选举的输入：存活与身份由调用方用进程级手段（pid + 记录里
/// 的启动标记比对现算的标记）判定——实例 IPC 不可达恰恰是需要
/// 看护的信号，不能作为「死」的依据参与选举。
pub fn registry_instances_in(run_dir: &std::path::Path) -> Vec<InstanceRecord> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(run_dir) else {
        return out;
    };
    for entry in entries.flatten() {
        if !entry
            .file_name()
            .to_string_lossy()
            .ends_with(INSTANCE_FILE_SUFFIX)
        {
            continue;
        }
        if let Ok(content) = std::fs::read_to_string(entry.path())
            && let Some(info) = parse_instance_file(&content)
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

/// 读取单个端口的注册表记录（只读，不探活；不存在/读不懂 → None）。
pub fn read_instance_file_in(run_dir: &std::path::Path, port: &str) -> Option<InstanceRecord> {
    let content = std::fs::read_to_string(instance_file_path_in(run_dir, port)).ok()?;
    parse_instance_file(&content)
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
    crate::process::terminate_marked_process(info.pid, &info.start)
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

/// 分离拉起时对子进程环境的调整。默认值 = 原样继承环境与工作目录。
#[derive(Debug, Clone, Copy, Default)]
pub struct Launch<'a> {
    /// 要改的环境变量：`Some` 设成这个值，`None` 在子进程里删掉。其余变量原样继承
    pub env: &'a [(String, Option<String>)],
    /// 子进程的工作目录；None = 继承
    pub cwd: Option<&'a std::path::Path>,
}

/// 以分离进程运行 `exe`，环境与工作目录原样继承。见 [`spawn_detached_with`]。
pub fn spawn_detached(exe: &std::path::Path, args: &[String]) -> io::Result<u32> {
    spawn_detached_with(exe, args, &Launch::default())
}

/// 以分离进程运行 `exe`：不继承句柄、没有控制台，并脱离启动者——关掉启动它的终端、
/// 终端向前台进程组发的 Ctrl+C、启动者所在的作业对象被结束，都不该连带结束它。
/// 返回子进程 pid（拉起即返回，不等待）。
///
/// Windows 上不走 std::process::Command：std 的 CreateProcessW 调用固定
/// bInheritHandles=TRUE 且无稳定 API 可关闭（CommandExt::inherit_handles
/// 仍 unstable）。这意味着调用方（cargo test / agent 脚本 / CI）经
/// Command::output() 捕获输出运行 `aproxy start` 时，其 stdout/stderr 管道
/// 写端句柄会被常驻守护进程继承，EOF 永不到来，调用方永久挂死。因此这里
/// 手写 CreateProcessW，以 bInheritHandles=FALSE 启动，不继承任何句柄。
///
/// 脱离的手段：
/// - Windows：CREATE_NO_WINDOW（没有控制台，控制台的进程树清理碰不到它），外加
///   CREATE_BREAKAWAY_FROM_JOB 脱出启动者的作业对象。作业不允许脱出时带着这个标志的
///   CreateProcess 会直接失败，所以失败就去掉它再试一次，并在 startup.log 记一条告警：
///   子进程仍在启动者的作业里，作业被结束时它会跟着结束。
/// - unix：子进程里 `setsid()`，自成会话与进程组，终端的信号到不了它。不与
///   `process_group(0)` 同用：先成了进程组组长，`setsid()` 会以 EPERM 失败。
pub fn spawn_detached_with(
    exe: &std::path::Path,
    args: &[String],
    launch: &Launch<'_>,
) -> io::Result<u32> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Foundation::{CloseHandle, GetLastError};
        use windows_sys::Win32::System::Threading::{
            CREATE_BREAKAWAY_FROM_JOB, CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT,
            CreateProcessW, PROCESS_INFORMATION, STARTUPINFOW,
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
        // 没有要改的变量时传 NULL（继承），省掉一次整块复制
        let env_block = (!launch.env.is_empty()).then(|| windows_env_block(launch.env));
        let env_ptr = env_block
            .as_ref()
            .map_or(std::ptr::null(), |block| block.as_ptr().cast());
        let cwd_wide: Option<Vec<u16>> = launch
            .cwd
            .map(|dir| dir.as_os_str().encode_wide().chain(Some(0)).collect());
        let cwd_ptr = cwd_wide
            .as_ref()
            .map_or(std::ptr::null(), |dir| dir.as_ptr());

        let mut si: STARTUPINFOW = unsafe { std::mem::zeroed() };
        si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
        let mut create = |flags: u32| -> Result<(), u32> {
            let ok = unsafe {
                CreateProcessW(
                    exe_wide.as_ptr(),
                    cmdline_wide.as_mut_ptr(),
                    std::ptr::null(), // lpProcessAttributes：默认安全描述符
                    std::ptr::null(), // lpThreadAttributes：默认安全描述符
                    0,                // bInheritHandles=FALSE：核心目的，杜绝句柄泄漏
                    flags,
                    env_ptr,
                    cwd_ptr,
                    &si,
                    &mut pi,
                )
            };
            if ok == 0 {
                Err(unsafe { GetLastError() })
            } else {
                Ok(())
            }
        };
        let base = CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT;
        if create(base | CREATE_BREAKAWAY_FROM_JOB).is_err() {
            // 作业不允许脱出（或别的原因）：去掉标志再试。这一次的错误才是要报的错误——
            // 可执行文件不在、没有权限这类原因两次都会失败
            create(base).map_err(|code| io::Error::from_raw_os_error(code as i32))?;
            append_startup_log(&format!(
                "warning: pid {} could not leave the launcher's job object and stays inside it; ending that job ends this process too",
                pi.dwProcessId
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
        use std::os::unix::process::CommandExt;
        use std::process::{Command, Stdio};
        let mut cmd = Command::new(exe);
        cmd.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        for (name, value) in launch.env {
            match value {
                Some(value) => cmd.env(name, value),
                None => cmd.env_remove(name),
            };
        }
        if let Some(dir) = launch.cwd {
            cmd.current_dir(dir);
        }
        // SAFETY: 闭包在 fork 之后、exec 之前的子进程里运行，只调用 setsid 这一个
        // async-signal-safe 的系统调用，不分配内存、不碰锁
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        Ok(cmd.spawn()?.id())
    }
}

/// 在当前进程的环境上套用 `changes`，写成 CreateProcessW 要的环境块：UTF-16 的
/// `名=值\0` 逐条相接，末尾再加一个 `\0`。变量名不分大小写：设置或删除一个变量时，
/// 先去掉任何大小写写法的同名变量。条目按名字的大写形式排序（系统对环境块的约定）。
#[cfg(windows)]
fn windows_env_block(changes: &[(String, Option<String>)]) -> Vec<u16> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStrExt;

    let mut vars: Vec<(OsString, OsString)> = std::env::vars_os().collect();
    for (name, value) in changes {
        vars.retain(|(existing, _)| !existing.to_string_lossy().eq_ignore_ascii_case(name));
        if let Some(value) = value {
            vars.push((name.into(), value.into()));
        }
    }
    vars.sort_by_cached_key(|(name, _)| name.to_string_lossy().to_uppercase());
    let mut block = Vec::new();
    for (name, value) in &vars {
        block.extend(name.encode_wide());
        block.push(u16::from(b'='));
        block.extend(value.encode_wide());
        block.push(0);
    }
    if vars.is_empty() {
        // 空环境块是两个 `\0`
        block.push(0);
    }
    block.push(0);
    block
}

// ---------------------------------------------------------------------------
// 实例注册表（~/.aproxy/run/<port>.instance.json）
// ---------------------------------------------------------------------------

/// 实例记录的文件名后缀：文件是 `<端口>.instance.json`。
pub const INSTANCE_FILE_SUFFIX: &str = ".instance.json";

/// 注册表文件路径：`<run_dir>/<port>.instance.json`
pub fn instance_file_path(listen_addr: &str) -> PathBuf {
    instance_file_path_in(&run_dir(), listen_addr)
}

/// 同上，目录可指定（测试注入用）
pub fn instance_file_path_in(run_dir: &std::path::Path, listen_addr: &str) -> PathBuf {
    run_dir.join(format!("{}{INSTANCE_FILE_SUFFIX}", port_of(listen_addr)))
}

/// 写入实例注册（bind 成功后调用，避免留下死记录）
pub fn write_instance_file(info: &InstanceRecord) -> io::Result<()> {
    write_instance_file_in(&run_dir(), info)
}

/// 同上，目录可指定（测试注入用）
///
/// 一行 JSON，`schema` 在前。原子写、只有本用户可读（crate::fsx::write_atomic_private）：
/// 直接写目标文件时，并发的读者可能读到写到一半的 JSON，把这个实例当成不存在。
pub fn write_instance_file_in(run_dir: &std::path::Path, info: &InstanceRecord) -> io::Result<()> {
    let path = instance_file_path_in(run_dir, &info.listen_addr);
    let file = InstanceFile {
        schema: INSTANCE_SCHEMA,
        record: info.clone(),
    };
    let json = serde_json::to_string(&file).expect("序列化实例信息失败");
    crate::fsx::write_atomic_private(&path, (json + "\n").as_bytes())
}

/// 删除实例注册（服务退出时调用）
pub fn remove_instance_file(listen_addr: &str) {
    let _ = std::fs::remove_file(instance_file_path(listen_addr));
}

/// 列出注册表中应答控制通道的实例（见 `survey_instances_in`）。
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

/// 普查本 home 的注册表。只读：不删任何文件（清理见 [`collect_garbage_in`]）。
pub async fn survey_instances() -> InstanceSurvey {
    survey_instances_in(&run_dir()).await
}

// ---------------------------------------------------------------------------
// 清理：只在实例开始服务之后与看护者的周期里做，读路径（status、选目标、界面轮询）
// 不删任何文件——查看状态的命令不该有副作用，而且它们可能由比写者更旧的版本执行。
// ---------------------------------------------------------------------------

/// 清理只碰早于这个年龄（秒）的日志、交接文件与临时文件：更新的可能正被某个进程写着、
/// 或刚写下还没被引用。
const GC_MIN_AGE_SECS: u64 = 600;

/// 一轮清理做了什么。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GcReport {
    /// 删掉的文件
    pub removed: Vec<PathBuf>,
    /// 日志清理是否因为有读不懂的记录而暂停（引用集不完整，不敢删）
    pub logs_paused: bool,
}

/// 一轮清理。规则：
/// - **实例记录** `<端口>.instance.json`：读得懂、并且写下它的进程经「pid + 启动标记」核验已经不在，
///   才删。读不懂的（损坏，或更新的版本写的）绝不删。
/// - **期望状态记录**：不归清理管，只有 stop 能删。
/// - **心跳文件** `<端口>.heart`（unix）：这个端口没有活着的实例、且文件早于
///   [`GC_MIN_AGE_SECS`] 时删（活实例每 10 秒写它一次，旧成这样的只能是崩溃留下的）。
/// - **交接文件与原子写的临时文件**（`*.spec.json`、`*.tmp-<pid>-<纳秒>`）：正常情况下
///   写者自己立即删掉或改名；写者崩溃留下的，在它的 pid 已不在、且文件早于
///   [`GC_MIN_AGE_SECS`] 时删。交接文件里有明文密钥，不能一直留在盘上。
/// - **实例日志**：只删同时满足三条的——名字是内置的随机名形状
///   （`<十六进制>-<四位十六进制>.log`）、早于 [`GC_MIN_AGE_SECS`]、不被任何实例记录或
///   期望状态记录引用。run 目录里有读不懂的记录时，引用集不完整，这一轮不删任何日志。
///   自定义的 `log_file` 与 startup.log 等固定名的日志永远不删。
pub fn collect_garbage_in(run_dir: &std::path::Path, logs_dir: &std::path::Path) -> GcReport {
    let mut report = GcReport::default();
    let mut referenced_logs: Vec<String> = Vec::new();
    let mut unreadable = false;
    // 有活实例的端口，与见到的心跳文件（等实例记录都看完再判断它们）
    let mut live_ports: Vec<String> = Vec::new();
    let mut hearts: Vec<(String, PathBuf)> = Vec::new();

    let restores = crate::spec::list_restore_in(run_dir);
    unreadable |= !restores.unreadable.is_empty();
    referenced_logs.extend(restores.entries.into_iter().map(|e| e.spec.log_path));

    for entry in std::fs::read_dir(run_dir).into_iter().flatten().flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(INSTANCE_FILE_SUFFIX) {
            let record = std::fs::read_to_string(&path)
                .ok()
                .and_then(|text| parse_instance_file(&text));
            match record {
                None => unreadable = true,
                Some(info) => {
                    // 没登记启动标记（空串，平台算不出）的记录核验不了身份：只在这个 pid 上
                    // 确实没有进程时才当它的写者已经不在，否则宁可留着
                    let alive = if info.start.is_empty() {
                        crate::process::process_exited(info.pid) != Some(true)
                    } else {
                        crate::process::record_identity(info.pid, &info.start)
                            == crate::process::RecordIdentity::Alive
                    };
                    if alive {
                        live_ports.push(port_of(&info.listen_addr).to_string());
                        referenced_logs.extend(info.log_path);
                    } else if std::fs::remove_file(&path).is_ok() {
                        report.removed.push(path);
                    }
                }
            }
        } else if let Some(port) = name.strip_suffix(".heart") {
            hearts.push((port.to_string(), path));
        } else if let Some(writer) = leftover_writer_pid(&name)
            && older_than(&path, GC_MIN_AGE_SECS)
            && crate::process::process_exited(writer) != Some(false)
            && std::fs::remove_file(&path).is_ok()
        {
            report.removed.push(path);
        }
    }

    for (port, path) in hearts {
        if !live_ports.contains(&port)
            && older_than(&path, GC_MIN_AGE_SECS)
            && std::fs::remove_file(&path).is_ok()
        {
            report.removed.push(path);
        }
    }

    if unreadable {
        report.logs_paused = true;
        return report;
    }
    // 引用集按 path_match_key 归一比较（大小写/分隔符规则与别名匹配共用）：记录里存的
    // 路径字符串与 read_dir 枚举出的表示在 Windows 上可能有分隔符差异，字面比较会误删
    let referenced: std::collections::HashSet<String> = referenced_logs
        .iter()
        .filter(|p| !p.is_empty())
        .map(|p| crate::settings::path_match_key(p))
        .collect();
    for entry in std::fs::read_dir(logs_dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if is_random_log_name(&entry.file_name().to_string_lossy())
            && older_than(&path, GC_MIN_AGE_SECS)
            && !referenced.contains(&crate::settings::path_match_key(
                &path.display().to_string(),
            ))
            && std::fs::remove_file(&path).is_ok()
        {
            report.removed.push(path);
        }
    }
    report
}

/// run 目录里一个文件若是交接文件或原子写的临时文件，返回名字里写者的 pid。
/// 交接文件是 `<纳秒>-<pid>.spec.json`，临时文件是 `<目标名>.tmp-<pid>-<纳秒>`。
fn leftover_writer_pid(name: &str) -> Option<u32> {
    if let Some(stem) = name.strip_suffix(".spec.json") {
        return stem.rsplit_once('-')?.1.parse().ok();
    }
    let (_, tail) = name.rsplit_once(".tmp-")?;
    let (pid, nanos) = tail.split_once('-')?;
    nanos.chars().all(|c| c.is_ascii_digit()).then_some(())?;
    pid.parse().ok()
}

/// 名字是不是内置的随机日志名：`<1 到 16 位小写十六进制>-<4 位小写十六进制>.log`。
fn is_random_log_name(name: &str) -> bool {
    let is_hex = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
    };
    name.strip_suffix(".log")
        .and_then(|stem| stem.split_once('-'))
        .is_some_and(|(nanos, pid)| {
            nanos.len() <= 16 && is_hex(nanos) && pid.len() == 4 && is_hex(pid)
        })
}

/// 文件的修改时间是否早于 `secs` 秒之前。读不到修改时间的按「不够旧」处理。
fn older_than(path: &std::path::Path, secs: u64) -> bool {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|age| age.as_secs() >= secs)
}

/// 同上，目录可指定（测试注入用）
pub async fn list_instances_in(dir: &std::path::Path) -> Vec<InstanceStatus> {
    survey_instances_in(dir).await.responsive
}

/// 逐条读注册表、IPC ping 验活。只读，不删任何文件。ping 失败时按「pid + 创建时间」
/// 核验记录的进程：仍在 → 归入 unresponsive；已退出、pid 已被复用、或记录没登记创建时间
/// （证明不了身份）→ 不列出，残留的记录留给清理（[`collect_garbage_in`]）。
/// **不应答 ≠ 已死**：挂死的实例进程还在，看门狗的挂死接管要靠这条记录核验身份，
/// `stop --force` 也要靠它定位。读不懂的记录跳过。
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
        if !entry
            .file_name()
            .to_string_lossy()
            .ends_with(INSTANCE_FILE_SUFFIX)
        {
            continue;
        }
        let info = std::fs::read_to_string(entry.path())
            .ok()
            .and_then(|c| parse_instance_file(&c));
        // 读不懂的记录（损坏，或更新的版本写的）定位不了端口，跳过
        let Some(info) = info else {
            continue;
        };
        let port = port_of(&info.listen_addr).to_string();
        match ipc_ping_in(dir, &port).await {
            // ping 应答携带实例的实时状态（含最近活动时刻）——实例记录
            // 是启动时刻的快照，闲置判定/展示必须用实时值
            Ok(live) if live.instance.pid == info.pid => out.push(live),
            // 应答的不是记录里的进程。新实例先建端点、后写记录，记录可能正被
            // 重写——重读一次再比。仍对不上时，记录的进程若还在就按无响应
            // 列出（它不在自己的端点上应答）；否则不列出、也不删：此刻删除
            // 可能恰好删掉应答者刚写下的记录
            Ok(live) => {
                if read_instance_file_in(dir, &port).is_some_and(|f| f.pid == live.instance.pid) {
                    out.push(live);
                } else if crate::process::record_identity(info.pid, &info.start)
                    == crate::process::RecordIdentity::Alive
                {
                    unresponsive.push(info);
                }
            }
            Err(_) => {
                if crate::process::record_identity(info.pid, &info.start)
                    == crate::process::RecordIdentity::Alive
                {
                    unresponsive.push(info);
                }
            }
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
/// respawn 的就绪判定专用：就绪只需「新 pid 的注册记录已出现」，读文件即可，无需 IPC。
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
        if !entry
            .file_name()
            .to_string_lossy()
            .ends_with(INSTANCE_FILE_SUFFIX)
        {
            return None;
        }
        std::fs::read_to_string(entry.path())
            .ok()
            .and_then(|c| parse_instance_file(&c))
            .filter(|info| info.pid == pid)
    })
}

/// 按一份期望状态记录拉起的实例落在了另一个端口（配置里的端口改了）时，退役旧端口的
/// 残留记录：删除旧端口的期望状态记录与实例记录。这是期望状态记录「只有 stop 能删」
/// 之外唯一的例外：新端口的记录接替了它。
///
/// 为什么必须删：新实例已按实际端口写了自己的记录；旧端口的期望状态记录若残留，用户
/// `stop <新端口>` 后看护者（或下一次 `aproxy restore`）会凭它把实例再拉起来，stop 被撤销
/// 且可无限重复（watchdog-01 实测）。
/// 旧端口的实例记录是已死旧进程的记录，留着只会让看护者的闲置自灭永远等不到
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
        && crate::process::record_identity(rec.pid, &rec.start)
            == crate::process::RecordIdentity::Alive
    {
        return false;
    }
    crate::spec::remove_restore_in(run_dir, old_port);
    let _ = std::fs::remove_file(instance_file_path_in(run_dir, old_port));
    true
}

/// 本进程删过控制 socket（退出流程）：unix 的接受循环据此不再把它重建出来。
#[cfg(unix)]
static SOCKET_RETIRED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 删除实例的 IPC 端点文件（优雅退出/看护摘除时调用）。
/// Windows 命名管道由内核回收（no-op）；unix 的 UDS socket 是真实文件，
/// bind 前的 remove_file 已自愈残留，此处显式清理让 run/ 目录不留死端点。
pub fn remove_socket_file(port: &str) {
    #[cfg(unix)]
    {
        SOCKET_RETIRED.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = std::fs::remove_file(endpoint_for(port));
    }
    #[cfg(windows)]
    {
        let _ = port;
    }
}

/// 实例退出时的运行期文件清理：实例记录、心跳与控制端点文件（后两者在 Windows 上由
/// 内核回收，调用是 no-op）。被强制终止的实例来不及自清，由终止它的一方代为执行。
///
/// 期望状态记录不在这里删：它表达「这个实例应当在运行」，只有 stop 能删（见
/// [`crate::spec`]）。实例因信号、崩溃或系统关机退出时记录留着，restore 与看护者据此把它
/// 拉回来。
pub fn remove_registry_files(listen_addr: &str) {
    remove_instance_file(listen_addr);
    crate::watchdog::remove_heartbeat_file(&run_dir(), port_of(listen_addr));
    remove_socket_file(port_of(listen_addr));
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
    // 内存撑爆。快照只有几百字节到几千字节；上限留给以后一行带一批请求记录的 op
    const MAX_RESPONSE_LINE: u64 = 16 * 1024 * 1024;
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
    let op = match parse_request(&line) {
        Ok(op) => op,
        Err(rejection) => return write_line(&mut writer, &rejection.reply()).await,
    };
    let stopping = *state.on_shutdown.borrow();
    match op {
        IpcOp::Ping => {}
        IpcOp::PrepareSwap { ttl_secs } => {
            if stopping {
                let reply = Rejection::InvalidState("stopping").reply();
                return write_line(&mut writer, &reply).await;
            }
            // 先置位再组装应答：应答里立即是 swap_prepared，安装器拿这一个应答
            // 就完成 ACK 判定，无需再补一次 ping
            let ttl_ms = ttl_secs
                .unwrap_or(DEFAULT_SWAP_TTL_SECS)
                .saturating_mul(1000);
            state.stats.swap_until_ms.store(
                now_millis().saturating_add(ttl_ms),
                std::sync::atomic::Ordering::Relaxed,
            );
        }
        IpcOp::Shutdown {
            keep_restore,
            if_idle_secs,
        } => {
            // 条件只在还没开始退出时判断：已经在退出的实例，重复的 shutdown 结果相同
            if let Some(secs) = if_idle_secs
                && !stopping
            {
                let idle_ms = now_millis().saturating_sub(state.last_request_at());
                if idle_ms < secs.saturating_mul(1000) {
                    let reply = Rejection::ConditionNotMet { idle_ms }.reply();
                    return write_line(&mut writer, &reply).await;
                }
            }
            // 记录先删、应答后发：调用方拿到应答时，「它不会再被拉回来」已经成立
            if !keep_restore {
                state.forget_restore();
            }
        }
    }
    let mut status = status_snapshot(&state);
    if matches!(op, IpcOp::Shutdown { .. }) {
        status.state = InstanceState::Stopping;
    }
    let reply = serde_json::to_string(&WireResponse {
        v: IPC_VERSION,
        ok: true,
        result: Some(status),
        error: None,
    })
    .expect("序列化 IPC 应答失败");
    write_line(&mut writer, &reply).await?;
    // 应答先发出去再触发停止：客户端立刻拿到确认，服务随后优雅退出
    if matches!(op, IpcOp::Shutdown { .. }) {
        let _ = state.on_shutdown.send(true);
    }
    Ok(())
}

/// 解析请求行。Err 是要回给客户端的拒绝。
fn parse_request(line: &str) -> Result<IpcOp, Rejection> {
    // 行未正常终止：超过长度上限被截断，或对端在发完整请求前就断开
    if !line.ends_with('\n') {
        return Err(Rejection::bad("incomplete_line"));
    }
    let Ok(request) = serde_json::from_str::<serde_json::Value>(line) else {
        return Err(Rejection::bad("not_json"));
    };
    let Some(v) = request.get("v") else {
        return Err(Rejection::bad("missing_v"));
    };
    if v.as_u64() != Some(u64::from(IPC_VERSION)) {
        return Err(Rejection::UnsupportedVersion(v.to_string()));
    }
    let Some(name) = request.get("op").and_then(|op| op.as_str()) else {
        return Err(Rejection::bad("missing_op"));
    };
    let args = match request.get("args") {
        None => None,
        Some(serde_json::Value::Object(args)) => Some(args),
        Some(_) => return Err(Rejection::bad("args_not_object")),
    };
    IpcOp::from_request(name, args)
}

/// 组装此刻的状态快照（计数器、最近错误在应答瞬间现读）。
fn status_snapshot(state: &ControlState) -> InstanceStatus {
    use std::sync::atomic::Ordering::Relaxed;
    let stats = &state.stats;
    let phase = if *state.on_shutdown.borrow() {
        InstanceState::Stopping
    } else if stats.swap_until_ms.load(Relaxed) > now_millis() {
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
        state: phase,
        behavior: state.behavior.clone(),
        activity: Activity {
            last_request_at: state.last_request_at(),
            requests_total: stats.requests_total.load(Relaxed),
            retries_total: stats.retries_total.load(Relaxed),
            last_error,
        },
        ops: IpcOp::NAMES.iter().map(|name| name.to_string()).collect(),
        features: IpcOp::FEATURES
            .iter()
            .map(|name| name.to_string())
            .collect(),
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

    pub struct Listener {
        listener: UnixListener,
        path: std::path::PathBuf,
    }

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
        UnixListener::bind(path).map(|listener| Listener {
            listener,
            path: path.to_path_buf(),
        })
    }

    /// 接受循环，兼管 socket 文件。文件被别的进程删掉时，监听还在、名字没了，
    /// 客户端从此找不到本实例。0.1.0 的看护者就会这么做：它处理实例的死亡事件
    /// 时删 `run/<端口>.sock`，迁移到新版本时它若还没退役，那条路径上可能已是
    /// 新实例的 socket。所以每秒看一眼，名字没了就在原路径重建；本进程自己在
    /// 退出（已收到 shutdown，或退出流程删了 socket）时不重建。
    pub async fn accept_loop(listener: Listener, state: Arc<ControlState>) {
        let Listener { mut listener, path } = listener;
        let mut check = tokio::time::interval(Duration::from_secs(1));
        check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                accepted = listener.accept() => {
                    let stream = match accepted {
                        Ok((stream, _)) => stream,
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
                _ = check.tick() => {
                    let retiring = *state.on_shutdown.borrow()
                        || super::SOCKET_RETIRED.load(std::sync::atomic::Ordering::Relaxed);
                    let missing = matches!(
                        std::fs::symlink_metadata(&path),
                        Err(e) if e.kind() == io::ErrorKind::NotFound
                    );
                    if missing && !retiring {
                        match UnixListener::bind(&path) {
                            Ok(fresh) => {
                                listener = fresh;
                                tracing::warn!(path = %path.display(), "控制 socket 文件被删除，已在原路径重建");
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, "控制 socket 文件被删除，重建失败，1 秒后再试");
                            }
                        }
                    }
                }
            }
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
            start: String::new(),
            instance_id: "19ac3f2e8b5d1a2b".into(),
            version: "0.0.0-test".into(),
            listen_addr: format!("127.0.0.1:{port}"),
            config_path: Some("C:/tmp/config.toml".into()),
            base_url: "https://api.example.com".into(),
            started_at: 1_700_000_000_000,
            log_path: None,
            foreground: false,
        }
    }

    fn sample_behavior() -> Behavior {
        Behavior {
            delivery: "retry".into(),
            disk_cache: true,
            keepalive: KeepaliveBehavior {
                trigger: "any".into(),
                interval_secs: 15,
            },
            retry_rules: 1,
        }
    }

    /// 测试用的控制服务状态；返回的 Receiver 用来观察 shutdown 是否被触发
    fn control(record: InstanceRecord) -> (Arc<ControlState>, tokio::sync::watch::Receiver<bool>) {
        let (on_shutdown, rx) = tokio::sync::watch::channel(false);
        let state = ControlState {
            record,
            behavior: sample_behavior(),
            run_dir: "/test/run".into(),
            stats: Arc::new(IpcStats::default()),
            on_shutdown,
            stop_requested: std::sync::Mutex::new(false),
        };
        (Arc::new(state), rx)
    }

    /// 一份只为占位的拉起规格：日志指向 `log_path`
    fn sample_spec(log_path: &str) -> crate::spec::InstanceSpec {
        crate::spec::InstanceSpec {
            schema: crate::spec::SCHEMA,
            config_path: None,
            overrides: Default::default(),
            pinned_port: None,
            env: Default::default(),
            log_path: log_path.to_string(),
        }
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
    use crate::process::tests::{spawn_unrelated_child, wait_child_exit};

    /// stop --force 的身份策略：登记时间不符（pid 复用）拒绝；无时间戳的旧
    /// 记录且端点无人应答（归属无从证明）拒绝；身份一致即终止——被杀的进程
    /// 名字与 aProxy 无关，证明判定不看名称
    #[cfg(any(windows, target_os = "linux"))]
    #[tokio::test]
    async fn force_terminate_checks_identity_not_name() {
        let mut child = spawn_unrelated_child();
        let pid = child.id();
        let start = aproxy_ipc::ledger::start_marker(pid).expect("子进程的启动标记可查");
        let mut info = sample_info(&format!("597{:02}", std::process::id() % 50));
        info.pid = pid;

        info.start = format!("{start}0");
        let err = force_terminate(&info).unwrap_err();
        assert!(
            err.contains("已不是记录里的那个进程"),
            "pid 复用应以身份不符拒绝: {err}"
        );

        info.start = String::new();
        let err = force_terminate(&info).unwrap_err();
        assert!(err.contains("核验"), "未登记启动标记应拒绝: {err}");
        std::thread::sleep(Duration::from_millis(200));
        assert!(
            child.try_wait().unwrap().is_none(),
            "拒绝后子进程必须仍在运行"
        );

        info.start = start;
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
    /// 进程 → 期望状态记录与实例记录一并删除；旧端口已被另一个在世实例接手（身份
    /// 核验通过，或旧版本记录 pid 在世无从核验）→ 记录归它，不动
    #[cfg(any(windows, target_os = "linux"))]
    #[test]
    fn retire_moved_port_records_only_touches_dead_owner() {
        let dir = tempfile::tempdir().unwrap();
        let write = |port: &str, pid: u32, start: &str| {
            let mut info = sample_info(port);
            info.pid = pid;
            info.start = start.to_string();
            write_instance_file_in(dir.path(), &info).unwrap();
            crate::spec::write_restore_in(dir.path(), port, &sample_spec("")).unwrap();
        };
        let has = |port: &str| {
            (
                crate::spec::restore_path_in(dir.path(), port).is_file(),
                instance_file_path_in(dir.path(), port).is_file(),
            )
        };

        write("59741", u32::MAX - 777, "w:123");
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
        let start = aproxy_ipc::ledger::start_marker(pid).expect("子进程的启动标记可查");
        write("59743", pid, &start);
        assert!(!retire_moved_port_records_in(dir.path(), "59743", "59744"));
        assert_eq!(has("59743"), (true, true), "在世实例接手的端口不得退役");
        // 未登记启动标记：证明不了是哪个进程，按无主记录退役
        write("59745", pid, "");
        assert!(retire_moved_port_records_in(dir.path(), "59745", "59746"));
        assert_eq!(has("59745"), (false, false));
        // 登记的标记不符 = pid 已复用，记录的进程已死 → 退役
        write("59747", pid, &format!("{start}0"));
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
        // 不是实例记录后缀的文件不参与检索
        std::fs::write(dir.path().join("59901.restore"), "args").unwrap();
        assert!(registry_contains_pid_in(dir.path(), 4242));
    }

    #[test]
    fn registry_contains_pid_skips_corrupt_and_missing_dir() {
        let dir = tempfile::tempdir().unwrap();
        // 损坏记录（不可解析）跳过，不 panic 不误报
        std::fs::write(dir.path().join("59902.instance.json"), "not-json").unwrap();
        assert!(!registry_contains_pid_in(dir.path(), 1));
        // 目录不存在 → false（respawn 轮询的前置态）
        assert!(!registry_contains_pid_in(
            dir.path().join("no-such").as_path(),
            1
        ));
    }

    /// 每个 op 的请求行钉在这里，并且都能被服务端的解析器还原成同一个 op。
    #[test]
    fn request_lines_are_pinned_and_round_trip_through_the_server_parser() {
        let cases = [
            (IpcOp::Ping, r#"{"v":1,"op":"ping"}"#),
            (IpcOp::STOP, r#"{"v":1,"op":"shutdown"}"#),
            (
                IpcOp::STOP_KEEPING_RESTORE,
                r#"{"v":1,"op":"shutdown","args":{"keep_restore":true}}"#,
            ),
            (
                IpcOp::Shutdown {
                    keep_restore: false,
                    if_idle_secs: Some(600),
                },
                r#"{"v":1,"op":"shutdown","args":{"if_idle_secs":600}}"#,
            ),
            (
                IpcOp::Shutdown {
                    keep_restore: true,
                    if_idle_secs: Some(0),
                },
                r#"{"v":1,"op":"shutdown","args":{"if_idle_secs":0,"keep_restore":true}}"#,
            ),
            (IpcOp::PREPARE_SWAP, r#"{"v":1,"op":"prepare_swap"}"#),
            (
                IpcOp::PrepareSwap {
                    ttl_secs: Some(120),
                },
                r#"{"v":1,"op":"prepare_swap","args":{"ttl_secs":120}}"#,
            ),
        ];
        for (op, line) in cases {
            assert_eq!(op.request_line(), line);
            assert_eq!(parse_request(&format!("{line}\n")).unwrap(), op, "{line}");
        }
        // 空的 args 等于没带；显式的 keep_restore:false 等于缺省
        for line in [
            r#"{"v":1,"op":"shutdown","args":{}}"#,
            r#"{"v":1,"op":"shutdown","args":{"keep_restore":false}}"#,
        ] {
            assert_eq!(
                parse_request(&format!("{line}\n")).unwrap(),
                IpcOp::STOP,
                "{line}"
            );
        }
        // 快照里列出的 op 名与各 op 的名字一致；features 的名字是 <op>.<参数>
        assert_eq!(
            IpcOp::NAMES,
            [
                IpcOp::Ping.name(),
                IpcOp::STOP.name(),
                IpcOp::PREPARE_SWAP.name()
            ]
        );
        assert_eq!(
            IpcOp::FEATURES,
            [
                "shutdown.if_idle_secs",
                "shutdown.keep_restore",
                "prepare_swap.ttl_secs"
            ]
        );
    }

    /// 期望状态记录的写与删在同一把锁下：收到过不带 `keep_restore` 的 shutdown 之后不再写。
    #[tokio::test]
    async fn stop_deletes_the_restore_record_and_blocks_later_writes() {
        let dir = tempfile::tempdir().unwrap();
        let record = sample_info("59761");
        let make = || {
            let (on_shutdown, _rx) = tokio::sync::watch::channel(false);
            ControlState {
                record: record.clone(),
                behavior: sample_behavior(),
                run_dir: dir.path().display().to_string(),
                stats: Arc::new(IpcStats::default()),
                on_shutdown,
                stop_requested: std::sync::Mutex::new(false),
            }
        };
        let path = crate::spec::restore_path_in(dir.path(), "59761");
        let spec = sample_spec("");

        // 启动时写入
        let state = make();
        assert!(state.write_restore(&spec).unwrap());
        assert!(path.is_file());
        // stop：记录删掉，之后的写入被拒（启动途中收到 stop 的实例不会把记录写回来）
        state.forget_restore();
        assert!(!path.exists());
        assert!(!state.write_restore(&spec).unwrap());
        assert!(!path.exists());

        // 带 keep_restore 的 shutdown 不走 forget_restore：经真实的请求处理验证
        let state = Arc::new(make());
        assert!(state.write_restore(&spec).unwrap());
        let (client, server) = tokio::io::duplex(4096);
        let handler = tokio::spawn(handle_conn(server, state.clone()));
        let line = IpcOp::STOP_KEEPING_RESTORE.request_line();
        let reply = exchange_over(client, &line).await.unwrap();
        handler.await.unwrap().unwrap();
        assert_eq!(parse_reply(&reply).unwrap().state, InstanceState::Stopping);
        assert!(path.is_file(), "keep_restore 的 shutdown 不得删记录");
        // 同一个实例随后收到 stop：记录删掉
        let (client, server) = tokio::io::duplex(4096);
        let handler = tokio::spawn(handle_conn(server, state.clone()));
        exchange_over(client, &IpcOp::STOP.request_line())
            .await
            .unwrap();
        handler.await.unwrap().unwrap();
        assert!(!path.exists(), "stop 应删掉记录");
    }

    /// 每种拒绝的应答行钉在这里：码、英文说明与 `details` 的键。
    #[test]
    fn rejections_are_pinned() {
        let reply = |line: &str| parse_request(line).unwrap_err().reply();
        let cases = [
            (
                "{\"v\":1,\"op\":\"what\"}\n",
                r#"{"v":1,"ok":false,"error":{"code":"unknown_op","message":"unknown op: what","details":{"op":"what"}}}"#,
            ),
            (
                "{\"v\":2,\"op\":\"ping\"}\n",
                r#"{"v":1,"ok":false,"error":{"code":"unsupported_version","message":"unsupported protocol version 2; this instance supports 1","details":{"supported":[1]}}}"#,
            ),
            (
                "{\"v\":1,\"op\":\"shutdown\",\"args\":{\"drain_secs\":5,\"keep_restore\":true,\"later\":1}}\n",
                r#"{"v":1,"ok":false,"error":{"code":"unsupported_arg","message":"unknown arg: drain_secs, later","details":{"args":["drain_secs","later"]}}}"#,
            ),
            (
                "{\"v\":1,\"op\":\"ping\",\"args\":{\"x\":1}}\n",
                r#"{"v":1,"ok":false,"error":{"code":"unsupported_arg","message":"unknown arg: x","details":{"args":["x"]}}}"#,
            ),
            (
                "{\"v\":1,\"op\":\"shutdown\",\"args\":{\"keep_restore\":\"yes\"}}\n",
                r#"{"v":1,"ok":false,"error":{"code":"bad_request","message":"arg keep_restore has the wrong type or value","details":{"reason":"invalid_arg","arg":"keep_restore"}}}"#,
            ),
            (
                "{\"v\":1,\"op\":\"shutdown\",\"args\":{\"if_idle_secs\":-1}}\n",
                r#"{"v":1,"ok":false,"error":{"code":"bad_request","message":"arg if_idle_secs has the wrong type or value","details":{"reason":"invalid_arg","arg":"if_idle_secs"}}}"#,
            ),
            (
                "{\"v\":1,\"op\":\"prepare_swap\",\"args\":{\"ttl_secs\":0}}\n",
                r#"{"v":1,"ok":false,"error":{"code":"bad_request","message":"arg ttl_secs has the wrong type or value","details":{"reason":"invalid_arg","arg":"ttl_secs"}}}"#,
            ),
            (
                "not json\n",
                r#"{"v":1,"ok":false,"error":{"code":"bad_request","message":"the request is not JSON","details":{"reason":"not_json"}}}"#,
            ),
            (
                // 没有 v 的请求（例如 0.1.0 的 CLI 发来的）一样按 v1 拒绝
                "{\"op\":\"ping\"}\n",
                r#"{"v":1,"ok":false,"error":{"code":"bad_request","message":"the request has no v","details":{"reason":"missing_v"}}}"#,
            ),
            (
                "{\"v\":1}\n",
                r#"{"v":1,"ok":false,"error":{"code":"bad_request","message":"the request has no op","details":{"reason":"missing_op"}}}"#,
            ),
            (
                "{\"v\":1,\"op\":\"ping\",\"args\":3}\n",
                r#"{"v":1,"ok":false,"error":{"code":"bad_request","message":"args must be an object","details":{"reason":"args_not_object"}}}"#,
            ),
            (
                // 行没有正常结束（超长被截断，或对端提前断开）
                "{\"v\":1,\"op\":\"ping\"}",
                r#"{"v":1,"ok":false,"error":{"code":"bad_request","message":"the request line is too long or incomplete","details":{"reason":"incomplete_line"}}}"#,
            ),
        ];
        for (line, expected) in cases {
            assert_eq!(reply(line), expected, "{line}");
        }
        assert_eq!(
            Rejection::InvalidState("stopping").reply(),
            r#"{"v":1,"ok":false,"error":{"code":"invalid_state","message":"not allowed while the instance is stopping","details":{"state":"stopping"}}}"#
        );
        assert_eq!(
            Rejection::ConditionNotMet { idle_ms: 1500 }.reply(),
            r#"{"v":1,"ok":false,"error":{"code":"condition_not_met","message":"the instance has been idle for only 1500 ms","details":{"idle_ms":1500}}}"#
        );
        // 每个码都在码表里，details 的键也都是码表里登记过的
        for (line, _) in cases {
            let value: serde_json::Value = serde_json::from_str(&reply(line)).unwrap();
            let code = aproxy_ipc::code::lookup(value["error"]["code"].as_str().unwrap())
                .expect("码表里应有这个码");
            for key in value["error"]["details"].as_object().unwrap().keys() {
                assert!(
                    code.details.iter().any(|d| d.key == key),
                    "{} 的 details 里有码表没登记的键 {key}",
                    code.name
                );
            }
        }
    }

    /// 成功应答的形状钉在这里：快照的字段名、顺序与类型。
    #[test]
    fn snapshot_shape_is_pinned() {
        let (state, _rx) = control(sample_info("12345"));
        state.stats.requests_total.store(120, Ordering::Relaxed);
        state.stats.retries_total.store(7, Ordering::Relaxed);
        let reply = serde_json::to_string(&WireResponse {
            v: IPC_VERSION,
            ok: true,
            result: Some(status_snapshot(&state)),
            error: None,
        })
        .unwrap();
        assert_eq!(
            reply,
            concat!(
                r#"{"v":1,"ok":true,"result":{"#,
                r#""instance":{"pid":42,"start":"","instance_id":"19ac3f2e8b5d1a2b","version":"0.0.0-test","listen_addr":"127.0.0.1:12345","config_path":"C:/tmp/config.toml","base_url":"https://api.example.com","started_at":1700000000000,"log_path":null,"foreground":false},"#,
                r#""state":"serving","#,
                r#""behavior":{"delivery":"retry","disk_cache":true,"keepalive":{"trigger":"any","interval_secs":15},"retry_rules":1},"#,
                r#""activity":{"last_request_at":1700000000000,"requests_total":120,"retries_total":7,"last_error":null},"#,
                r#""ops":["ping","shutdown","prepare_swap"],"#,
                r#""features":["shutdown.if_idle_secs","shutdown.keep_restore","prepare_swap.ttl_secs"]}}"#
            )
        );
        // 客户端读回同一份
        let status = parse_reply(&reply).unwrap();
        assert_eq!(status.behavior, sample_behavior());
        assert_eq!(status.state, InstanceState::Serving);
    }

    #[test]
    fn client_reads_v1_errors_and_unknown_states() {
        let err = parse_reply(
            r#"{"v":1,"ok":false,"error":{"code":"unsupported_arg","message":"x","details":{"args":["a"]},"later":1}}"#,
        )
        .unwrap_err();
        assert!(
            matches!(&err, IpcError::Remote { code, details, .. }
                if code == "unsupported_arg" && details["args"][0] == "a"),
            "{err}"
        );
        // 没有 details 的错误也读得出
        let err = parse_reply(r#"{"v":1,"ok":false,"error":{"code":"unknown_op","message":"x"}}"#)
            .unwrap_err();
        assert!(
            matches!(&err, IpcError::Remote { code, details, .. }
                if code == "unknown_op" && details.is_null()),
            "{err}"
        );
        assert!(matches!(
            parse_reply(r#"{"v":2,"ok":true}"#),
            Err(IpcError::Protocol(_))
        ));
        // 更新版本的实例：不认识的状态读作 Unknown，不认识的字段（顶层、快照里、instance 里、
        // behavior 里）一律忽略，没见过的枚举值照样读得出
        let mut instance = serde_json::to_value(sample_info("1")).unwrap();
        instance["added_later"] = serde_json::json!({"x": 1});
        let newer = serde_json::json!({"v": 1, "ok": true, "extra": 1, "result": {
            "instance": instance, "state": "draining",
            "behavior": {"delivery": "adaptive", "disk_cache": false,
                         "keepalive": {"trigger": "path", "interval_secs": 0, "later": 1},
                         "retry_rules": 0, "history": {"body_budget_mb": 64}},
            "formats": [{"name": "adaptive"}],
            "activity": {"last_request_at": 1, "requests_total": 0, "retries_total": 0,
                         "last_error": null, "in_flight": 2},
            "ops": ["ping", "subscribe"], "features": ["subscribe.filter"],
            "future_field": true}});
        let status = parse_reply(&newer.to_string()).unwrap();
        assert_eq!(status.state, InstanceState::Unknown);
        assert_eq!(status.behavior.delivery, "adaptive");
        assert_eq!(status.ops, ["ping", "subscribe"]);
        // `starting` 是认得的状态
        let mut starting = newer.clone();
        starting["result"]["state"] = "starting".into();
        assert_eq!(
            parse_reply(&starting.to_string()).unwrap().state,
            InstanceState::Starting
        );
        // 没有 v 的应答（0.1.0 守护的形状）无法采信
        assert!(matches!(
            parse_reply(r#"{"ok":true,"info":{"pid":7},"proto":2}"#),
            Err(IpcError::Protocol(_))
        ));
    }

    /// 实例记录的形状是跨版本读的：文件名、字段名、顺序与类型钉在这里。快照里的 `instance`
    /// 是同一个对象去掉 `schema`。
    #[test]
    fn instance_record_shape_is_pinned() {
        let record = InstanceRecord {
            pid: 4321,
            start: "w:133712345678901234".into(),
            instance_id: "19ac3f2e8b5d1a2b".into(),
            version: "0.2.0".into(),
            listen_addr: "127.0.0.1:12345".into(),
            config_path: Some("/home/u/.aproxy/config.toml".into()),
            base_url: "https://api.example.com".into(),
            started_at: 1_760_190_000_123,
            log_path: Some("/home/u/.aproxy/logs/19ac3f2e8b5d-1a2b.log".into()),
            foreground: false,
        };
        let in_snapshot = r#"{"pid":4321,"start":"w:133712345678901234","instance_id":"19ac3f2e8b5d1a2b","version":"0.2.0","listen_addr":"127.0.0.1:12345","config_path":"/home/u/.aproxy/config.toml","base_url":"https://api.example.com","started_at":1760190000123,"log_path":"/home/u/.aproxy/logs/19ac3f2e8b5d-1a2b.log","foreground":false}"#;
        assert_eq!(serde_json::to_string(&record).unwrap(), in_snapshot);

        let dir = tempfile::tempdir().unwrap();
        write_instance_file_in(dir.path(), &record).unwrap();
        let on_disk = std::fs::read_to_string(dir.path().join("12345.instance.json")).unwrap();
        assert_eq!(
            on_disk,
            format!(
                "{{\"schema\":1,{}
",
                &in_snapshot[1..]
            ),
            "盘上的形状是 schema 在前、其余与快照里的 instance 相同"
        );
        assert_eq!(parse_instance_file(&on_disk), Some(record));

        // 前台实例、没有配置文件：两个路径都是 null
        let foreground = parse_instance_file(
            r#"{"schema":1,"pid":1,"start":"","instance_id":"a","version":"0.2.0","listen_addr":"127.0.0.1:1","config_path":null,"base_url":"https://x","started_at":1,"log_path":null,"foreground":true,"later":1}"#,
        )
        .expect("以后追加的字段读得过去");
        assert!(foreground.foreground);
        assert_eq!((foreground.config_path, foreground.log_path), (None, None));
        // schema 更高的记录不解读；旧版本的形状读不懂
        assert_eq!(
            parse_instance_file(&on_disk.replace("\"schema\":1", "\"schema\":2")),
            None
        );
        assert_eq!(
            parse_instance_file(
                r#"{"pid":7,"process_start":1,"version":"0.1.0","listen_addr":"127.0.0.1:1","config_path":"c","base_url":"b","started_at":1,"log_path":""}"#
            ),
            None
        );
    }

    #[test]
    fn instance_file_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let info = sample_info("45678");
        write_instance_file_in(dir.path(), &info).unwrap();
        let path = instance_file_path_in(dir.path(), "127.0.0.1:45678");
        assert_eq!(path.file_name().unwrap(), "45678.instance.json");
        let loaded = parse_instance_file(&std::fs::read_to_string(&path).unwrap()).unwrap();
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
        assert_eq!(status.behavior, sample_behavior());
        assert_eq!(status.features.len(), 3);
        assert_eq!(status.ops, ["ping", "shutdown", "prepare_swap"]);
        assert_eq!(status.activity.requests_total, 7);
        assert_eq!(status.activity.retries_total, 2);
        let last = status.activity.last_error.expect("应带最近错误");
        assert!(last.message.contains("502") && last.at > 0);
        // 还没收到过请求：最近活动取启动时刻，不是 0
        assert_eq!(status.activity.last_request_at, status.instance.started_at);

        // 实例不认识的参数：整个请求被拒，什么都没做
        let (client, conn) = tokio::io::duplex(4096);
        let handler = tokio::spawn(handle_conn(conn, state.clone()));
        let reply = exchange_over(client, r#"{"v":1,"op":"shutdown","args":{"drain_secs":5}}"#)
            .await
            .unwrap();
        handler.await.unwrap().unwrap();
        assert!(
            matches!(parse_reply(&reply), Err(IpcError::Remote { code, .. }) if code == "unsupported_arg")
        );
        assert!(!*rx.borrow(), "被拒的 shutdown 不得触发停止");

        // 带条件的 shutdown：实例刚收到过请求，不够闲 → 拒绝，照常服务
        state
            .stats
            .last_activity_ms
            .store(now_millis(), Ordering::Relaxed);
        let err = request_raw(
            &endpoint,
            IpcOp::Shutdown {
                keep_restore: false,
                if_idle_secs: Some(3600),
            },
        )
        .await
        .unwrap_err();
        assert!(
            matches!(&err, IpcError::Remote { code, details, .. }
                if code == "condition_not_met" && details["idle_ms"].as_u64().is_some_and(|ms| ms < 60_000)),
            "{err}"
        );
        assert!(!*rx.borrow(), "条件不成立时不得触发停止");
        assert_eq!(
            request_raw(&endpoint, IpcOp::Ping).await.unwrap().state,
            InstanceState::Serving
        );

        // prepare_swap：应答里立即是 swap_prepared（安装器的 ACK 判定），之后也是
        let swap = request_raw(&endpoint, IpcOp::PREPARE_SWAP).await.unwrap();
        assert_eq!(swap.state, InstanceState::SwapPrepared);
        let again = request_raw(&endpoint, IpcOp::Ping).await.unwrap();
        assert_eq!(again.state, InstanceState::SwapPrepared);
        // 到期之后自己回到 serving：安装者中途没了，实例不会永远停在更换阶段
        state
            .stats
            .swap_until_ms
            .store(now_millis() - 1, Ordering::Relaxed);
        assert_eq!(
            request_raw(&endpoint, IpcOp::Ping).await.unwrap().state,
            InstanceState::Serving
        );
        // 带时长的 prepare_swap 重新进入，按给的时长计
        let before = now_millis();
        let swap = request_raw(
            &endpoint,
            IpcOp::PrepareSwap {
                ttl_secs: Some(120),
            },
        )
        .await
        .unwrap();
        assert_eq!(swap.state, InstanceState::SwapPrepared);
        let until = state.stats.swap_until_ms.load(Ordering::Relaxed);
        assert!(
            (before + 120_000..=now_millis() + 120_000).contains(&until),
            "{until}"
        );

        // 带条件的 shutdown：条件成立（闲置 0 秒总是成立）就停。应答先到（状态 stopping），
        // 随后置位停止信号
        let stopping = request_raw(
            &endpoint,
            IpcOp::Shutdown {
                keep_restore: false,
                if_idle_secs: Some(0),
            },
        )
        .await
        .unwrap();
        assert_eq!(stopping.state, InstanceState::Stopping);
        rx.changed().await.unwrap();
        assert!(*rx.borrow());
        // 已经在退出：重复的 shutdown 结果相同，条件不再判断
        state
            .stats
            .last_activity_ms
            .store(now_millis(), Ordering::Relaxed);
        let again = request_raw(
            &endpoint,
            IpcOp::Shutdown {
                keep_restore: false,
                if_idle_secs: Some(3600),
            },
        )
        .await
        .unwrap();
        assert_eq!(again.state, InstanceState::Stopping);
        // 退出中不再进入更换阶段
        let err = request_raw(&endpoint, IpcOp::PREPARE_SWAP)
            .await
            .unwrap_err();
        assert!(
            matches!(&err, IpcError::Remote { code, details, .. }
                if code == "invalid_state" && details["state"] == "stopping"),
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
        // 普查与 stop 都按应答里的 pid 认实例
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

    #[cfg(unix)]
    #[tokio::test]
    async fn deleted_socket_file_is_recreated_while_serving() {
        // 别的进程删了本实例的 socket 文件（还没退役的 0.1.0 看护者会这么做）：
        // 一两秒内在原路径重建，客户端照常连得上
        let dir = tempfile::tempdir().unwrap();
        let endpoint = test_endpoint(dir.path(), "heal");
        let mut record = sample_info("0");
        record.pid = std::process::id();
        let server = tokio::spawn(serve_endpoint(endpoint.clone(), control(record).0));
        tokio::time::sleep(Duration::from_millis(100)).await;
        std::fs::remove_file(&endpoint).unwrap();
        let mut healed = false;
        for _ in 0..30 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            if request_raw(&endpoint, IpcOp::Ping).await.is_ok() {
                healed = true;
                break;
            }
        }
        assert!(healed, "被删的控制 socket 应在原路径重建");
        server.abort();
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
        stale.start = "w:123".into();
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
    async fn listing_skips_dead_records_without_deleting_anything() {
        // 一条指向不存在实例的注册记录、一条读不懂的记录：列举不返回它们，也不删它们
        let dir = tempfile::tempdir().unwrap();
        let dead = sample_info("59801");
        write_instance_file_in(dir.path(), &dead).unwrap();
        let path = instance_file_path_in(dir.path(), "127.0.0.1:59801");
        let garbled = dir.path().join("59809.instance.json");
        std::fs::write(&garbled, "{not json").unwrap();
        let listed = list_instances_in(dir.path()).await;
        assert!(listed.is_empty(), "{listed:?}");
        assert!(path.exists() && garbled.exists(), "读路径不删任何文件");
    }

    #[tokio::test]
    async fn survey_keeps_records_of_live_but_unresponsive_instances() {
        // 进程还在、端点却无人应答（挂死的形态）：用本测试进程自身冒充——
        // pid 与创建时间都核验得上，但 59802 上没有 IPC 端点
        let dir = tempfile::tempdir().unwrap();
        let pid = std::process::id();
        let start = aproxy_ipc::ledger::start_marker(pid).expect("读不到自身的启动标记");
        let mut hung = sample_info("59802");
        hung.pid = pid;
        hung.start = start.clone();
        write_instance_file_in(dir.path(), &hung).unwrap();
        let path = instance_file_path_in(dir.path(), "127.0.0.1:59802");

        let survey = survey_instances_in(dir.path()).await;
        assert!(survey.responsive.is_empty());
        assert_eq!(survey.unresponsive.len(), 1, "挂死实例应归入 unresponsive");
        assert_eq!(survey.unresponsive[0].pid, pid);
        assert!(path.exists(), "进程仍在的记录不能因 ping 失败被删");

        // 同一 pid、创建时间对不上 = pid 已被复用，记录的进程早已死亡 → 不列出；记录留给清理
        hung.start = format!("{start}0");
        write_instance_file_in(dir.path(), &hung).unwrap();
        let survey = survey_instances_in(dir.path()).await;
        assert!(survey.unresponsive.is_empty());
        assert!(path.exists(), "读路径不删任何文件");
    }

    /// 把一个文件的修改时间拨到 `secs` 秒之前
    fn age(path: &std::path::Path, secs: u64) {
        let file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        file.set_modified(std::time::SystemTime::now() - Duration::from_secs(secs))
            .unwrap();
    }

    #[test]
    fn garbage_collection_follows_the_cleanup_rules() {
        let home = tempfile::tempdir().unwrap();
        let run = home.path().join("run");
        let logs = home.path().join("logs");
        std::fs::create_dir_all(&run).unwrap();
        std::fs::create_dir_all(&logs).unwrap();
        let old = GC_MIN_AGE_SECS + 60;
        let touch = |path: &std::path::Path, secs: u64| {
            std::fs::write(path, "x").unwrap();
            age(path, secs);
        };

        // 实例记录：活着的（本测试进程）留下，写者已不在的删掉
        let me = std::process::id();
        let mut live = sample_info("59811");
        live.pid = me;
        live.start = aproxy_ipc::ledger::start_marker(me).unwrap();
        let live_log = logs.join("1f3a2b-00ab.log");
        live.log_path = Some(live_log.display().to_string().replace('\\', "/"));
        write_instance_file_in(&run, &live).unwrap();
        let mut dead = sample_info("59812");
        dead.pid = u32::MAX - 779;
        dead.start = "w:123".into();
        let dead_log = logs.join("2a2a2a-00cd.log");
        dead.log_path = Some(dead_log.display().to_string());
        write_instance_file_in(&run, &dead).unwrap();
        // 期望状态记录：不归清理管；它引用的日志留着
        let crash_log = logs.join("5c6d7e-11cd.log");
        crate::spec::write_restore_in(
            &run,
            "59813",
            &sample_spec(&crash_log.display().to_string()),
        )
        .unwrap();
        // 交接文件与临时文件：写者不在且够旧才删
        let dead_writer = u32::MAX - 780;
        let stale_spec = run.join(format!("1700000000000000000-{dead_writer}.spec.json"));
        let fresh_spec = run.join(format!("1700000000000000001-{dead_writer}.spec.json"));
        let own_spec = run.join(format!("1700000000000000002-{me}.spec.json"));
        let stale_tmp = run.join(format!(
            "59811.instance.json.tmp-{dead_writer}-1700000000000000003"
        ));
        // 心跳文件：端口上没有活实例且够旧才删
        let dead_heart = run.join("59812.heart");
        let live_heart = run.join("59811.heart");
        touch(&dead_heart, old);
        touch(&live_heart, old);
        touch(&stale_spec, old);
        touch(&fresh_spec, 0);
        touch(&own_spec, old);
        touch(&stale_tmp, old);
        // 日志
        let orphan = logs.join("9900aa-22ef.log");
        let young_orphan = logs.join("9900ab-22ef.log");
        let custom = logs.join("my-instance.log");
        let fixed = logs.join("startup.log");
        for path in [&live_log, &dead_log, &crash_log, &orphan, &custom, &fixed] {
            touch(path, old);
        }
        touch(&young_orphan, 0);
        std::fs::write(logs.join("not-a-log.txt"), "x").unwrap();

        // 有读不懂的记录时：实例记录与残留文件照常清，日志一个都不删
        let unreadable = run.join("7.restore.json");
        std::fs::write(&unreadable, r#"{"schema":9}"#).unwrap();
        let report = collect_garbage_in(&run, &logs);
        assert!(report.logs_paused);
        assert!(
            orphan.exists() && dead_log.exists(),
            "引用集不完整时不得删日志"
        );
        assert!(unreadable.exists(), "读不懂的记录绝不删");
        assert!(
            !instance_file_path_in(&run, "59812").exists(),
            "写者已不在的实例记录应删"
        );
        assert!(
            instance_file_path_in(&run, "59811").exists(),
            "活实例的记录不删"
        );
        assert!(
            !stale_spec.exists() && !stale_tmp.exists(),
            "写者不在的旧残留应删"
        );
        assert!(fresh_spec.exists(), "不够旧的不删");
        assert!(own_spec.exists(), "写者还在的不删");
        assert!(!dead_heart.exists(), "没有活实例的端口留下的旧心跳文件应删");
        assert!(live_heart.exists(), "活实例的心跳文件不删");
        assert!(crate::spec::restore_path_in(&run, "59813").exists());

        std::fs::remove_file(&unreadable).unwrap();
        let report = collect_garbage_in(&run, &logs);
        assert!(!report.logs_paused);
        let mut removed = report.removed.clone();
        removed.sort();
        // 死实例的日志在它的记录被清掉之后不再被引用
        let mut expected = vec![dead_log.clone(), orphan.clone()];
        expected.sort();
        assert_eq!(removed, expected);
        assert!(
            live_log.exists(),
            "活实例引用的日志不删（路径分隔符不同也认得出）"
        );
        assert!(crash_log.exists(), "期望状态记录引用的日志不删");
        assert!(young_orphan.exists(), "不够旧的不删");
        assert!(
            custom.exists() && fixed.exists(),
            "不是随机名形状的日志永远不删"
        );
        assert!(logs.join("not-a-log.txt").exists());
    }

    #[test]
    fn stale_spool_dirs_are_removed_and_live_ones_kept() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("spool");
        let make = |name: &str, files: &[&str]| {
            let dir = root.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            for file in files {
                std::fs::write(dir.join(file), "x").unwrap();
            }
            dir
        };
        let me = std::process::id();
        let marker = aproxy_ipc::ledger::start_marker(me);
        let own_name = spool_dir_name(me, marker.as_deref());
        // 活着的进程自己的目录
        let own = make(&own_name, &["req-1-1.spooltmp"]);
        // pid 上已经没有进程
        let dead = make(
            &format!("{}-0a0b0c0d", u32::MAX - 781),
            &["spool-1-1.spooltmp"],
        );
        // 里面有不认识的文件：临时文件删掉，目录与那个文件留着
        let mixed = make(
            &format!("{}-0a0b0c0d", u32::MAX - 782),
            &["res-1-1.spooltmp", "notes.txt"],
        );
        // 名字不是这个形状的不碰（旧版本按端口命名的目录也在其中）
        let by_port = make("59820", &["spool-9-9.spooltmp"]);
        let odd = make(&format!("{me}-XYZ"), &["a.spooltmp"]);

        let mut expected = vec![dead.clone()];
        // pid 还在而启动标记对不上：只有读得到标记的平台才判得出
        let reused = marker.as_ref().map(|_| {
            let wrong = if own_name.ends_with("00000000") {
                "11111111"
            } else {
                "00000000"
            };
            make(&format!("{me}-{wrong}"), &["req-2-2.spooltmp"])
        });
        expected.extend(reused.clone());
        expected.sort();

        let mut removed = collect_stale_spool_in(&root);
        removed.sort();
        assert_eq!(removed, expected);
        assert!(own.join("req-1-1.spooltmp").exists(), "活进程的目录不碰");
        assert!(!dead.exists());
        assert!(mixed.join("notes.txt").exists() && !mixed.join("res-1-1.spooltmp").exists());
        assert!(by_port.join("spool-9-9.spooltmp").exists());
        assert!(odd.join("a.spooltmp").exists());
        // 根目录不存在时什么也不做
        assert!(collect_stale_spool_in(&home.path().join("absent")).is_empty());
    }

    #[test]
    fn spool_dir_name_is_stable_and_tied_to_the_start_marker() {
        let a = spool_dir_name(4242, Some("133801234567890000"));
        assert_eq!(a, spool_dir_name(4242, Some("133801234567890000")));
        assert_ne!(a, spool_dir_name(4242, Some("133801234567890001")));
        assert_eq!(spool_dir_owner(&a), Some(4242));
        assert_eq!(spool_dir_owner(&spool_dir_name(7, None)), Some(7));
        for name in ["59820", "7-abc", "7-ABCDEF01", "x-0a0b0c0d", "7-0a0b0c0d0"] {
            assert_eq!(spool_dir_owner(name), None, "{name}");
        }
    }

    #[test]
    fn leftover_and_log_names_are_recognized_strictly() {
        assert_eq!(leftover_writer_pid("1700-4242.spec.json"), Some(4242));
        assert_eq!(
            leftover_writer_pid("12345.instance.json.tmp-4242-1700"),
            Some(4242)
        );
        assert_eq!(leftover_writer_pid("settings.json.tmp-7-9"), Some(7));
        for name in [
            "12345.instance.json",
            "12345.restore.json",
            "x.tmp-abc-1",
            "x.tmp-1-abc",
            "watchdog.json",
        ] {
            assert_eq!(leftover_writer_pid(name), None, "{name}");
        }
        for name in ["19ac3f2e8b5d-1a2b.log", "0-0000.log"] {
            assert!(is_random_log_name(name), "{name}");
        }
        for name in [
            "startup.log",
            "watchdog.log",
            "my-instance.log",
            "19AC-1A2B.log",
            "19ac-1a2.log",
            "19ac3f2e8b5d19ac3-1a2b.log",
            "19ac-1a2b.txt",
        ] {
            assert!(!is_random_log_name(name), "{name}");
        }
    }
}
