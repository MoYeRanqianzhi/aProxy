//! Instance protocol v1: snapshots, operations and their frame codecs. Both ends use
//! these definitions so that an operation and its advertised capability cannot drift.

use serde::{Deserialize, Serialize};

/// 实例记录：`run/<端口>.instance.json` 的内容去掉开头的 `schema`，也是 IPC 状态快照里的
/// `instance`。实例在 bind 之后写下它，正常退出时删掉。
///
/// 原地升级时新旧两个版本同时在跑，会读对方写下的记录：已有字段不改名、不删、不改类型，
/// 以后新增的字段只能是可缺省的。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct InstanceRecord {
    /// Process ID claimed by the instance; the client compares it to the transport peer.
    pub pid: u32,
    /// 实例进程的启动标记（台账用的同一个，见 [`crate::ledger::start_marker`]），实例
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
    /// Version of the instance that wrote this record.
    pub version: String,
    /// Listening address used to locate the control endpoint.
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

/// IPC 状态快照：v1 每个成功应答的 `result`，所有 op 都回它。
///
/// 应答只增字段：以后追加的字段在这里一律带 `#[serde(default)]`，读不到就是缺省值；
/// 不认识的字段忽略。
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct InstanceStatus {
    /// Identity and paths reported by this run.
    pub instance: InstanceRecord,
    /// Lifecycle state at the time of sampling.
    pub state: InstanceState,
    /// 实例的行为摘要：默认的投递方式、缓存、保活与规则数
    pub behavior: Behavior,
    /// Activity counters sampled with this snapshot.
    pub activity: Activity,
    /// 每个配置条目的协商与熔断状态，配置顺序不变；早期实例没有这个字段。
    #[serde(default)]
    pub formats: Vec<serde_json::Value>,
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
    /// Heartbeat settings advertised by the instance.
    pub keepalive: KeepaliveBehavior,
    /// 生效的重试规则条数
    pub retry_rules: u32,
    /// 请求历史的设置
    #[serde(default)]
    pub history: HistoryBehavior,
}

/// 请求历史的摘要（`behavior.history`）。
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct HistoryBehavior {
    /// 全部请求体与响应体加起来最多占多少内存（MiB）；0 = 不保留体
    pub body_budget_mb: u64,
}

/// 保活的摘要（`behavior.keepalive`）。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct KeepaliveBehavior {
    /// 哪些请求走保活：`accept`、`body_stream` 或 `any`
    pub trigger: String,
    /// 心跳间隔（秒）；0 = 保活关闭
    pub interval_secs: u64,
}

/// 能力名：shutdown 认 `keep_restore` 参数。
pub const FEATURE_SHUTDOWN_KEEP_RESTORE: &str = "shutdown.keep_restore";
/// 能力名：shutdown 认 `if_idle_secs` 参数。
pub const FEATURE_SHUTDOWN_IF_IDLE_SECS: &str = "shutdown.if_idle_secs";
/// 能力名：prepare_swap 认 `ttl_secs` 参数。
pub const FEATURE_PREPARE_SWAP_TTL_SECS: &str = "prepare_swap.ttl_secs";
/// The instance redacts client-supplied text at insertion, including diagnostics.
pub const FEATURE_REDACTED_CLIENT_FIELDS: &str = "redacted_client_fields";

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

/// 实例的实时观测值（应答时现读）。时刻都是 Unix 毫秒。
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Activity {
    /// 最近一次收到客户端请求的时刻；还没收到过请求时为启动时刻
    pub last_request_at: u64,
    /// 累计收到的客户端请求数
    pub requests_total: u64,
    /// 累计上游重试次数（首轮之后的所有尝试）
    pub retries_total: u64,
    /// 经过至少一次重试、最终成功的请求数
    #[serde(default)]
    pub rescued_total: u64,
    /// 失败后由 decide 从 forward 改为 retry 的请求总数，不含初选。
    #[serde(default)]
    pub upgraded_total: u64,
    /// 已收到、响应还没交付完的请求数（等上游、退避、重试、回放都算）
    #[serde(default)]
    pub in_flight: u64,
    /// 其中正处在第二次及以后的尝试里（含退避等待）的请求数
    #[serde(default)]
    pub retrying: u64,
    /// 失败的次数，按码分：失败的上游尝试记尝试失败码，aproxy 自己结束的请求记终态码
    #[serde(default)]
    pub by_code: std::collections::BTreeMap<String, u64>,
    /// 最近一小时的走势：[`BUCKET_COUNT`] 个桶，每桶 [`BUCKET_WIDTH_SECS`] 秒，旧的在前
    #[serde(default)]
    pub buckets: ActivityBuckets,
    /// 最近一次失败；从未失败为 null
    pub last_error: Option<LastError>,
}

impl Activity {
    /// Whole seconds since the last request at this sample time. A clock rollback
    /// gives zero, not a huge idle interval that could select an active instance.
    pub fn idle_secs_at(&self, now_ms: u64) -> u64 {
        now_ms.saturating_sub(self.last_request_at) / 1000
    }
}

/// 每个时间桶的宽度（秒）。
pub const BUCKET_WIDTH_SECS: u64 = 120;
/// 保留多少个时间桶。
pub const BUCKET_COUNT: usize = 30;

/// 按时间桶的计数（快照里的 `activity.buckets`）。三个数组等长，旧的在前，最后一个是
/// 当前这个桶。
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct ActivityBuckets {
    /// Width shared by the three counter series.
    pub width_secs: u64,
    /// 收到的请求数
    pub requests: Vec<u64>,
    /// 重试次数（首轮之后的尝试）
    pub retries: Vec<u64>,
    /// 失败的上游尝试数
    pub failures: Vec<u64>,
}

/// 最近一次失败。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct LastError {
    /// 码：失败的上游尝试是尝试失败码，aproxy 自己结束的请求是终态码
    #[serde(default)]
    pub code: String,
    /// 这个码带的数据（键与类型见码表）
    #[serde(default)]
    pub details: serde_json::Value,
    /// 已脱敏的一句英文说明，只给日志与兜底显示用
    pub message: String,
    /// 发生时刻
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
    /// 实例支持的全部 op 的名字（快照里的 `ops`）：这里的三个控制 op，加上读请求历史的
    /// 四个（见 [`ReadOp`]）
    pub const NAMES: [&'static str; 7] = [
        "ping",
        "shutdown",
        "prepare_swap",
        "requests",
        "request",
        "body",
        "subscribe",
    ];

    /// 本版本认得的 op 参数（快照里的 `features`）
    pub const FEATURES: [&'static str; 4] = [
        FEATURE_SHUTDOWN_IF_IDLE_SECS,
        FEATURE_SHUTDOWN_KEEP_RESTORE,
        FEATURE_PREPARE_SWAP_TTL_SECS,
        FEATURE_REDACTED_CLIENT_FIELDS,
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

    /// Stable operation name advertised in a snapshot.
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
        reject_unknown_args(args, known)?;
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

/// 带了 `known` 之外的参数就拒绝，列出它们。
fn reject_unknown_args(
    args: Option<&serde_json::Map<String, serde_json::Value>>,
    known: &[&str],
) -> Result<(), Rejection> {
    let unknown: Vec<String> = args
        .into_iter()
        .flat_map(|args| args.keys())
        .filter(|key| !known.contains(&key.as_str()))
        .cloned()
        .collect();
    if unknown.is_empty() {
        Ok(())
    } else {
        Err(Rejection::UnsupportedArg(unknown))
    }
}

/// 读请求历史的操作。都没有副作用；成功应答的 `result` 各有各的形状（不是状态快照）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadOp {
    /// 列请求记录，新的在前：`{"requests":[RequestRecord…],"more":bool}`。`before` 是一个
    /// 请求标识，给了就只列比它更早的；`limit` 是一次最多几条
    /// （1 到 [`MAX_LIST`]，不给是 [`DEFAULT_LIST`]）
    Requests {
        before: Option<String>,
        limit: Option<u64>,
    },
    /// 一个请求的明细（RequestDetail）：记录、请求头、每次尝试发出与收到的头；体只给大小与
    /// 状态
    Request { request_id: String },
    /// 读一个体的一块：`{"total_bytes","offset","data_b64","more","complete"}`。`attempt` 在
    /// `part` 是发出的体或响应体时必填；`offset` 不给是 0；`limit` 是这一块最多多少字节
    /// （1 到 [`MAX_BODY_READ`]，不给就是上限）
    Body {
        request_id: String,
        part: Part,
        attempt: Option<u32>,
        offset: Option<u64>,
        limit: Option<u64>,
    },
    /// 订阅事件：先回 `{"snapshot":…,"requests":[在途的 RequestRecord]}`，之后每行一个事件，
    /// 直到任一方关闭连接
    Subscribe,
}

impl ReadOp {
    /// Stable operation name advertised in a snapshot.
    pub fn name(&self) -> &'static str {
        match self {
            ReadOp::Requests { .. } => "requests",
            ReadOp::Request { .. } => "request",
            ReadOp::Body { .. } => "body",
            ReadOp::Subscribe => "subscribe",
        }
    }

    /// `part` 在线上的名字
    fn part_name(part: Part) -> &'static str {
        match part {
            Part::Request => "request",
            Part::Sent => "sent",
            Part::Response => "response",
        }
    }

    /// 按名字与 `args` 还原一个 op。`name` 已经确认是这四个之一。
    fn from_request(
        name: &str,
        args: Option<&serde_json::Map<String, serde_json::Value>>,
    ) -> Result<Self, Rejection> {
        let known: &[&str] = match name {
            "requests" => &["before", "limit"],
            "request" => &["request_id"],
            "body" => &["request_id", "part", "attempt", "offset", "limit"],
            _ => &[],
        };
        reject_unknown_args(args, known)?;
        let invalid = |arg: &'static str| Rejection::BadRequest {
            reason: "invalid_arg",
            arg: Some(arg),
        };
        let arg = |key: &str| args.and_then(|args| args.get(key));
        let text = |key: &'static str| match arg(key) {
            None => Ok(None),
            Some(value) => value
                .as_str()
                .map(|s| Some(s.to_string()))
                .ok_or(invalid(key)),
        };
        let required_text = |key: &'static str| {
            text(key)?.ok_or(Rejection::BadRequest {
                reason: "missing_arg",
                arg: Some(key),
            })
        };
        // 取值在 `range` 之内的整数
        let integer = |key: &'static str, range: std::ops::RangeInclusive<u64>| match arg(key) {
            None => Ok(None),
            Some(value) => value
                .as_u64()
                .filter(|n| range.contains(n))
                .map(Some)
                .ok_or(invalid(key)),
        };
        Ok(match name {
            "requests" => ReadOp::Requests {
                before: text("before")?,
                limit: integer("limit", 1..=MAX_LIST as u64)?,
            },
            "request" => ReadOp::Request {
                request_id: required_text("request_id")?,
            },
            "body" => {
                let part = match required_text("part")?.as_str() {
                    "request" => Part::Request,
                    "sent" => Part::Sent,
                    "response" => Part::Response,
                    _ => return Err(invalid("part")),
                };
                let attempt = integer("attempt", 1..=u64::from(u32::MAX))?.map(|n| n as u32);
                if part != Part::Request && attempt.is_none() {
                    return Err(Rejection::BadRequest {
                        reason: "missing_arg",
                        arg: Some("attempt"),
                    });
                }
                ReadOp::Body {
                    request_id: required_text("request_id")?,
                    part,
                    attempt,
                    offset: integer("offset", 0..=u64::MAX)?,
                    limit: integer("limit", 1..=MAX_BODY_READ)?,
                }
            }
            _ => ReadOp::Subscribe,
        })
    }

    /// 发往实例的请求行（不含换行）。没给的参数不带。
    pub fn request_line(&self) -> String {
        let mut args = serde_json::Map::new();
        match self {
            ReadOp::Requests { before, limit } => {
                if let Some(before) = before {
                    args.insert("before".into(), before.as_str().into());
                }
                if let Some(limit) = limit {
                    args.insert("limit".into(), (*limit).into());
                }
            }
            ReadOp::Request { request_id } => {
                args.insert("request_id".into(), request_id.as_str().into());
            }
            ReadOp::Body {
                request_id,
                part,
                attempt,
                offset,
                limit,
            } => {
                args.insert("request_id".into(), request_id.as_str().into());
                args.insert("part".into(), Self::part_name(*part).into());
                if let Some(attempt) = attempt {
                    args.insert("attempt".into(), (*attempt).into());
                }
                if let Some(offset) = offset {
                    args.insert("offset".into(), (*offset).into());
                }
                if let Some(limit) = limit {
                    args.insert("limit".into(), (*limit).into());
                }
            }
            ReadOp::Subscribe => {}
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

/// 一个解析好的请求：控制 op，或读请求历史的 op。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Parsed {
    Control(IpcOp),
    Read(ReadOp),
}

/// v1 应答的线上形状。成功带 `result`，失败带 `error`。
#[derive(Serialize, Deserialize)]
pub struct WireResponse {
    /// Protocol version of the response.
    pub v: u32,
    /// Whether the operation succeeded.
    pub ok: bool,
    /// On success, the snapshot returned by a control operation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<InstanceStatus>,
    /// On failure, the typed rejection instead of a result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<WireError>,
}

/// 失败应答。`code` 是稳定的 snake_case 字符串，供程序判断；`details` 是这个码带的数据
/// （键与类型见码表 [`crate::code`]）；`message` 是英文，只给日志看，不要解析它。
#[derive(Serialize, Deserialize)]
pub struct WireError {
    /// Machine-readable rejection code.
    pub code: String,
    /// Human-readable fallback diagnostic; do not parse.
    pub message: String,
    /// Additional data belonging to the rejection code.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub details: serde_json::Value,
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
    /// 要的请求或体取不到（`details.request_id`、`details.reason`；`reason` 是
    /// `not_retained` 时 `details.cause` 说明为什么没留）
    pub const NOT_FOUND: &str = "not_found";
    /// 流式连接已经满了（`details.limit` 是上限）
    pub const BUSY: &str = "busy";
}

/// 实例对一个请求的拒绝：码、英文说明与 `details`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejection {
    /// `reason` 的词表：`incomplete_line`、`not_json`、`missing_v`、`missing_op`、
    /// `args_not_object`、`invalid_arg`（参数的类型或取值不对）、`missing_arg`（缺必填的
    /// 参数）；后两种的 `arg` 是那个参数的名字
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
    /// 要的请求或体取不到。`reason` 的词表：`unknown_request`（没有这个请求，或它的记录
    /// 已经被淘汰）、`no_such_part`（这个请求没有这个体）、`evicted`（体为了腾地方被丢掉
    /// 了）、`not_retained`（体没有保留，`cause` 说明原因）
    NotFound {
        request_id: String,
        reason: &'static str,
        cause: Option<&'static str>,
    },
    /// 流式连接已经有 `limit` 条
    Busy {
        limit: usize,
    },
}

impl Rejection {
    /// Reject a malformed frame with the protocol reason and no argument name.
    pub fn bad(reason: &'static str) -> Self {
        Rejection::BadRequest { reason, arg: None }
    }

    /// 要回给客户端的那一行应答（不含换行）
    pub fn reply(&self) -> String {
        use serde_json::json;
        let (code, message, details) = match self {
            Rejection::BadRequest { reason, arg } => {
                let message = match (*reason, arg) {
                    ("incomplete_line", _) => "the request line is too long or incomplete".into(),
                    ("not_json", _) => "the request is not JSON".into(),
                    ("missing_v", _) => "the request has no v".into(),
                    ("missing_op", _) => "the request has no op".into(),
                    ("args_not_object", _) => "args must be an object".into(),
                    ("missing_arg", Some(arg)) => format!("arg {arg} is required"),
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
            Rejection::NotFound {
                request_id,
                reason,
                cause,
            } => {
                let details = match cause {
                    Some(cause) => {
                        json!({ "request_id": request_id, "reason": reason, "cause": cause })
                    }
                    None => json!({ "request_id": request_id, "reason": reason }),
                };
                (
                    error_code::NOT_FOUND,
                    format!("nothing to return for request {request_id}: {reason}"),
                    details,
                )
            }
            Rejection::Busy { limit } => (
                error_code::BUSY,
                format!("all {limit} streaming connections are in use"),
                json!({ "limit": limit }),
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

/// 读体时一块最多多少字节（原始字节，base64 之前）
pub const MAX_BODY_READ: u64 = 1024 * 1024;
/// 列记录时一次最多多少条
pub const MAX_LIST: usize = 200;
/// Maximum size in bytes of any reply or event, including its terminating newline.
pub const MAX_REPLY_LINE: usize = 16 * 1024 * 1024;

/// 列记录时不给 `limit` 就是多少条
pub const DEFAULT_LIST: usize = 50;

/// 要读的是哪个体。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// 客户端发来的请求体
    Request,
    /// 某次尝试发往上游的体
    Sent,
    /// 某次尝试从上游收到的响应体
    Response,
}

/// 解析请求行。Err 是要回给客户端的拒绝。
pub fn parse_request(line: &str) -> Result<Parsed, Rejection> {
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
    match name {
        "requests" | "request" | "body" | "subscribe" => {
            ReadOp::from_request(name, args).map(Parsed::Read)
        }
        _ => IpcOp::from_request(name, args).map(Parsed::Control),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
            assert_eq!(
                parse_request(&format!("{line}\n")).unwrap(),
                Parsed::Control(op),
                "{line}"
            );
        }
        // 空的 args 等于没带；显式的 keep_restore:false 等于缺省
        for line in [
            r#"{"v":1,"op":"shutdown","args":{}}"#,
            r#"{"v":1,"op":"shutdown","args":{"keep_restore":false}}"#,
        ] {
            assert_eq!(
                parse_request(&format!("{line}\n")).unwrap(),
                Parsed::Control(IpcOp::STOP),
                "{line}"
            );
        }
        // 快照里列出的 op 名与各 op 的名字一致；features 的名字是 <op>.<参数>
        assert_eq!(
            IpcOp::NAMES,
            [
                IpcOp::Ping.name(),
                IpcOp::STOP.name(),
                IpcOp::PREPARE_SWAP.name(),
                ReadOp::Requests {
                    before: None,
                    limit: None
                }
                .name(),
                ReadOp::Request {
                    request_id: String::new()
                }
                .name(),
                ReadOp::Body {
                    request_id: String::new(),
                    part: Part::Request,
                    attempt: None,
                    offset: None,
                    limit: None
                }
                .name(),
                ReadOp::Subscribe.name(),
            ]
        );
        assert_eq!(
            IpcOp::FEATURES,
            [
                "shutdown.if_idle_secs",
                "shutdown.keep_restore",
                "prepare_swap.ttl_secs",
                "redacted_client_fields"
            ]
        );
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
            let code = crate::code::lookup(value["error"]["code"].as_str().unwrap())
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
    /// 读类 op 的请求行钉在这里，并且实例的解析还原得出同一个 op。
    #[test]
    fn read_request_lines_are_pinned_and_round_trip_through_the_server_parser() {
        let cases = [
            (
                ReadOp::Requests {
                    before: None,
                    limit: None,
                },
                r#"{"v":1,"op":"requests"}"#,
            ),
            (
                ReadOp::Requests {
                    before: Some("19ac3f2e8b5d1a2b-42".into()),
                    limit: Some(200),
                },
                r#"{"v":1,"op":"requests","args":{"before":"19ac3f2e8b5d1a2b-42","limit":200}}"#,
            ),
            (
                ReadOp::Request {
                    request_id: "19ac3f2e8b5d1a2b-42".into(),
                },
                r#"{"v":1,"op":"request","args":{"request_id":"19ac3f2e8b5d1a2b-42"}}"#,
            ),
            (
                ReadOp::Body {
                    request_id: "19ac3f2e8b5d1a2b-42".into(),
                    part: Part::Request,
                    attempt: None,
                    offset: None,
                    limit: None,
                },
                r#"{"v":1,"op":"body","args":{"request_id":"19ac3f2e8b5d1a2b-42","part":"request"}}"#,
            ),
            (
                ReadOp::Body {
                    request_id: "19ac3f2e8b5d1a2b-42".into(),
                    part: Part::Response,
                    attempt: Some(3),
                    offset: Some(1048576),
                    limit: Some(1),
                },
                r#"{"v":1,"op":"body","args":{"request_id":"19ac3f2e8b5d1a2b-42","part":"response","attempt":3,"offset":1048576,"limit":1}}"#,
            ),
            (ReadOp::Subscribe, r#"{"v":1,"op":"subscribe"}"#),
        ];
        for (op, line) in cases {
            assert_eq!(op.request_line(), line);
            assert_eq!(
                parse_request(&format!("{line}\n")).unwrap(),
                Parsed::Read(op),
                "{line}"
            );
        }
    }
}
