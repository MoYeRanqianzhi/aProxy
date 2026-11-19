//! 配置：一个实例实际用的运行时配置 [`Config`]，以及得到它的整条路。
//!
//! - [`mod@file`]：config.toml 的严格解析——文件里写了什么，连同带行列的诊断。
//! - [`mod@resolve`]：分层解析——启动时的覆盖 > config.toml > settings.json 的全局默认 > 内置默认，
//!   产出 [`Config`]、每个键的来源与诊断。启动、诊断与展示都走它。
//! - [`mod@edit`]：写回——只改目标键，保留注释与键序。
//!
//! [`Config`] 的字段名是运行时一侧的名字，其中一部分沿用了 0.1.x 的叫法，与 config.toml 里的
//! 键名不同。对应关系（左边是字段，右边是文件里的键）：
//! `max_body_mb` ← `max_request_mb`；`spool_limit_mb` ← `max_response_mb`；
//! `forward_only` ← `delivery = "forward"`；`connect_timeout_secs`、`first_byte_timeout_secs`、
//! `read_timeout_secs` ← `timeouts.connect_secs`、`timeouts.first_byte_secs`、
//! `timeouts.read_idle_secs`；`keepalive_interval_secs`、`keepalive_trigger`、
//! `keepalive_heartbeat` ← `keepalive.interval_secs`、`keepalive.trigger`、`keepalive.heartbeat`；
//! `max_retry_backoff_secs` ← `retry.max_backoff_secs`；`retry_rules` ← `[[retry.rules]]`；
//! `request_transform`、`response_transform`、`heartbeat_transform` ← `[[format]]` 里声明了
//! `request`、`response`、`heartbeat` 阶段的程序。字段注释里说到「配置」时用的是字段名。
//!
//! 经 [`resolve()`] 得到的 [`Config`] 里，带全局默认的 `Option` 字段都已经填好；`None` 只出现在
//! 不经解析、直接构造出来的配置上（测试），所以消费点一律走访问方法取生效值。

pub mod edit;
pub mod file;
pub mod resolve;
pub mod view;

use std::{collections::HashMap, path::PathBuf};

pub use file::RetryRule;
pub use resolve::{Overrides, Resolved, Source, resolve};

/// 心跳的内置默认值：SSE 注释，合法 SSE 客户端按规范忽略。
pub const DEFAULT_KEEPALIVE_HEARTBEAT: &str = ": keepalive\n\n";

/// 心跳写完后客户端 SSE 解析器必须停在事件边界，否则心跳会和随后回放的
/// 第一个上游事件拼在一起（例如心跳 `data: x\n` 没有空行收尾，解析器会把它
/// 并进下一个事件的 data）。SSE 的行尾可以是 CRLF、LF 或单独的 CR。规则：
/// - 非空（不想要心跳字节请把 `keepalive.interval_secs` 设为 0）；
/// - 以行尾结束（半行会粘到回放的第一行上）；
/// - 含字段行（不以 `:` 开头的非空行，如 `event:`、`data:`）时以空行收尾，
///   让解析器把这个事件派发掉、清空缓冲。只含注释行与空行时无此要求。
///
/// 返回违反的规则（一句英文，接在键名之后读），合法时 None。
pub fn heartbeat_problem(hb: &str) -> Option<&'static str> {
    if hb.is_empty() {
        return Some("must not be empty (set keepalive.interval_secs = 0 to turn heartbeats off)");
    }
    let normalized = hb.replace("\r\n", "\n").replace('\r', "\n");
    if !normalized.ends_with('\n') {
        return Some("must end with a line break, or the half line joins the first replayed line");
    }
    let has_field = normalized
        .split('\n')
        .any(|line| !line.is_empty() && !line.starts_with(':'));
    if has_field && !normalized.ends_with("\n\n") {
        return Some(
            "must end with a blank line when it has field lines such as event: or data:, or it merges with the first replayed event",
        );
    }
    None
}

/// 请求体大小上限的内置默认值（MB）。settings.json 与 config.toml 均可覆盖
/// （toml > settings > 本值）。
pub const DEFAULT_MAX_BODY_MB: u64 = 128;
/// 磁盘缓存的内置默认值（开）。语义见 Config::disk_cache。
pub const DEFAULT_DISK_CACHE: bool = true;
/// 仅转发模式的内置默认值（关）。语义与代价见 Config::forward_only——
/// 开启即放弃本产品最核心的重试保障，故默认必须为关。
pub const DEFAULT_FORWARD_ONLY: bool = false;
/// 外部转换器持续模式池上限的内置默认值（`pool_max` 未配置时生效）。
/// 转换是「改 JSON 结构」微秒级操作，小池即可撑高并发；默认 4。
pub const DEFAULT_TRANSFORM_POOL_MAX: u32 = 4;
/// 外部转换器持续模式 worker 空闲回收的内置默认值（秒，`idle_timeout_secs`
/// 未配置时生效）。0 = 永不回收（worker 跟随实例生命周期）。
pub const DEFAULT_TRANSFORM_IDLE_TIMEOUT_SECS: u64 = 300;
/// 外部转换器单请求转换超时的内置默认值（秒，`timeout_secs` 未配置时生效）。
/// 转换耗时极低，30s 已极宽裕；0 = 不限。
pub const DEFAULT_TRANSFORM_TIMEOUT_SECS: u64 = 30;

/// 保活触发条件（`keepalive_trigger` 的取值）：哪些请求在等待上游与重试期间走
/// 「先向客户端提交响应头、再用 SSE 注释心跳保活」的通道。语义详见
/// `Config::keepalive_trigger` 字段说明。
///
/// 配置层存的是原始字符串（toml / settings.json 都是），由本枚举的 `parse`
/// 统一解释：非法值要在 validate() 里点名字段报错——若直接让 serde 解析成
/// 枚举，settings.json 里的一个笔误会让整个文件解析失败、静默回退成全默认
/// （连别名一起丢），用户看不到是哪个字段错了。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeepaliveTrigger {
    /// 客户端 Accept 头含 `text/event-stream`（0.1.0 之前的唯一判定）
    Accept,
    /// 客户端原始请求体是 JSON 对象且顶层 `"stream": true`
    BodyStream,
    /// 两者任一（内置默认）
    Any,
}

impl KeepaliveTrigger {
    /// 配置里的写法 → 枚举；不认识的写法返回 None（validate 据此报错）。
    /// 只认小写原文，与 `mode = "persistent"` 等枚举字段的严格程度一致。
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "accept" => Some(Self::Accept),
            "body_stream" => Some(Self::BodyStream),
            "any" => Some(Self::Any),
            _ => None,
        }
    }

    /// 枚举 → 配置里的写法（`parse` 的逆）
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Accept => "accept",
            Self::BodyStream => "body_stream",
            Self::Any => "any",
        }
    }
}

/// 保活触发条件的内置默认值：`any`。真实 Claude Code 的流式主请求是
/// `Accept: application/json` + 请求体 `"stream": true`（2026-10-04 实测），
/// 只看 Accept 的旧判定让它永远进不了保活通道——默认必须把请求体也算进来。
pub const DEFAULT_KEEPALIVE_TRIGGER: KeepaliveTrigger = KeepaliveTrigger::Any;

/// 外部转换器的运行模式（`request_transform`/`response_transform` 的 `mode`）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TransformMode {
    /// 每请求一次性 spawn（默认）：请求时启动进程、写一行信封、读一行回信、
    /// 进程退出。任何能读写 stdin/stdout 的程序都可用，无需循环支持。
    #[default]
    Spawn,
    /// 持续进程池：worker 进程以 `while` 循环逐行处理（一次一个请求、输入
    /// 输出有序），按需扩容至 `pool_max`，空闲超时回收。聚合类 format
    /// （轮换/计数状态在进程内存）必须用本模式。
    Persistent,
}

/// 外部转换器配置（`request_transform` / `response_transform` 各一份）。
/// 指向外部 format 程序：stdin 收一行 JSON 信封、stdout 回一行 JSON 信封。
/// 信封契约见 aproxy-envelope crate；format 编写指南见 aproxy-format skill。
#[derive(Debug, Clone, Default)]
pub struct TransformConfig {
    /// 这个程序在配置里的名字（`[[format]]` 的 `name`）。错误体与日志用它指明是哪个程序。
    pub name: String,
    /// 追加给程序的环境变量（`[[format]]` 的 `env`）。
    pub env: std::collections::BTreeMap<String, String>,
    /// format 程序命令（必填非空，validate 拦截）。不走 shell，直接按
    /// argv 数组执行——无注入面。`~` 前缀在加载时展开为用户主目录
    /// （`~/.aproxy/bin/aproxy-format` 可直接使用）；裸文件名走 PATH。
    pub command: String,
    /// format 程序参数（如官方示例的 `["run", "--config", "...toml"]`）。
    pub args: Vec<String>,
    /// 运行模式。默认 spawn（一次性）；聚合类 format 必须 persistent。
    pub mode: TransformMode,
    /// persistent 模式池上限（并发 worker 数，超限排队）。未配置取
    /// `DEFAULT_TRANSFORM_POOL_MAX`（4）；spawn 模式无意义。
    pub pool_max: Option<u32>,
    /// persistent 模式 worker 空闲回收秒数。0 = 永不回收。未配置取
    /// `DEFAULT_TRANSFORM_IDLE_TIMEOUT_SECS`（300）；spawn 模式无意义。
    pub idle_timeout_secs: Option<u64>,
    /// 单请求转换超时秒数。0 = 不限。未配置取
    /// `DEFAULT_TRANSFORM_TIMEOUT_SECS`（30）。超时的 worker 不可信
    /// （可能仍在消化旧输入），kill 后剔除。
    pub timeout_secs: Option<u64>,
    /// 原样透传进信封 `extra` 字段的字符串（格式无要求，format 自解）。
    /// 官方示例用它传聚合配置文件路径。
    pub extra: Option<String>,
    /// 只对 request_transform 有意义：每次重试之前用客户端的原始请求再跑一次
    /// 请求转换（信封 `stage = "retry"`，附上一次失败的状态码或原因），回信就是
    /// 这次尝试发往上游的请求。默认 false = 每个请求只转换一次、各次重试复用。
    /// 这一次的转换失败不会让请求终止（那会违背无限重试），沿用上一次发出的请求。
    pub every_attempt: bool,
}

impl TransformConfig {
    /// 池上限生效值（persistent 模式；未配置回退内置默认）。
    pub fn effective_pool_max(&self) -> u32 {
        self.pool_max.unwrap_or(DEFAULT_TRANSFORM_POOL_MAX).max(1)
    }

    /// 空闲回收生效值（秒；未配置回退内置默认，0 = 永不）。
    pub fn effective_idle_timeout_secs(&self) -> u64 {
        self.idle_timeout_secs
            .unwrap_or(DEFAULT_TRANSFORM_IDLE_TIMEOUT_SECS)
    }

    /// 单请求转换超时生效值（秒；未配置回退内置默认，0 = 不限）。
    pub fn effective_timeout_secs(&self) -> u64 {
        self.timeout_secs.unwrap_or(DEFAULT_TRANSFORM_TIMEOUT_SECS)
    }

    /// 信封 `extra` 生效值（未配置 = 空字符串）。
    pub fn effective_extra(&self) -> &str {
        self.extra.as_deref().unwrap_or("")
    }
}

/// 一个实例的运行时配置。由 [`resolve()`] 从各层合并得到；字段名与 config.toml 键名的对应
/// 见模块文档。
#[derive(Debug, Clone)]
pub struct Config {
    /// 上游 API 的 base URL，例如 `https://api.anthropic.com`
    /// 末尾斜杠会被自动去除以避免拼接时产生 `//`。
    pub base_url: String,
    /// 本地代理监听地址，默认 `127.0.0.1:12345`（仅本地可访问，避免局域网暴露）。
    pub listen_addr: String,
    /// 快捷 api_key：若设置，等效覆盖 `Authorization: Bearer <api_key>`。
    pub api_key: Option<String>,
    /// 额外头：仅在请求未携带该头时追加（大小写不敏感判定）。
    pub extra_headers: HashMap<String, String>,
    /// 覆盖头：无条件覆盖请求头（大小写不敏感匹配，写入时保留配置中的大小写）。
    pub override_headers: HashMap<String, String>,
    /// 流式重试保活心跳间隔（秒），0 表示关闭。默认 15 秒。
    pub keepalive_interval_secs: u64,
    /// 保活触发条件：`"accept"` | `"body_stream"` | `"any"`。命中的请求（且
    /// keepalive_interval_secs > 0、非 forward_only）在等待上游与重试期间
    /// 先向客户端提交响应头，再每 keepalive_interval_secs 发一行 SSE 注释
    /// （`: keepalive`），防止客户端因首字节/空闲超时放弃请求：
    /// - 上游回 2xx + `text/event-stream`（未压缩）时立即转发上游真实 status
    ///   与响应头（去掉 content-length 与 hop-by-hop），之后照旧完整缓冲、校验，
    ///   无误再回放；配置了 response_transform 时不转发上游头
    /// - 上游一个 keepalive 间隔内没给出可提交的结果、或需要重试时，先提交
    ///   骨架头（200 + text/event-stream）
    /// - 提交之后的重试都在同一个响应里进行，客户端只见到心跳
    ///
    /// 判定在请求转换（request_transform）**之前**、基于客户端视角：
    /// - `accept`：客户端 Accept 头含 text/event-stream（旧行为）
    /// - `body_stream`：客户端原始请求体是 JSON 对象且顶层 `"stream": true`
    ///   （OpenAI / Anthropic 等协议通用的流式开关；判定只看请求体字段，与
    ///   URL 无关；溢写到磁盘的大请求体同样流式只看顶层键）
    /// - `any`：两者任一
    ///
    /// 未设置时用 settings.json 的 `keepalive_trigger`（全局默认，内置 `any`）。
    /// 存原始字符串、由 `KeepaliveTrigger::parse` 解释，非法值 validate() 报错。
    pub keepalive_trigger: Option<String>,
    /// 保活心跳：每个 keepalive 间隔原样写给客户端的字节。未设置时为 SSE 注释
    /// `": keepalive\n\n"`（合法 SSE 客户端按规范忽略）。aProxy 不认识任何协议，
    /// 心跳该长什么样由用户按客户端决定：例如某些客户端的 SSE 解析器遇注释会
    /// 出错、只容得下空行（`"\n"`）；也可以写成一个完整事件。validate() 只把关
    /// 一条与协议无关的规则：写完心跳后客户端的 SSE 解析器必须停在事件边界，
    /// 否则它会和随后回放的第一个上游事件拼成一个事件（见 `heartbeat_problem`）。
    pub keepalive_heartbeat: Option<String>,
    /// 上游代理 URL，例如 `http://127.0.0.1:7890`、`socks5://user:pass@127.0.0.1:7890`。
    /// 未设置时使用系统/环境变量代理。
    pub proxy: Option<String>,
    /// 代理用户名（可选；优先于代理 URL 内嵌的 user:pass）。
    pub proxy_username: Option<String>,
    /// 代理密码（可选；优先于代理 URL 内嵌的密码）。
    pub proxy_password: Option<String>,
    /// 重试退避的最大等待时间（秒）。指数退避 5s→10s→20s→…增长到该值后封顶；
    /// 设为 0 表示所有重试零延迟（立即重试）。默认 320 秒。
    pub max_retry_backoff_secs: u64,
    /// 上游响应缓冲（spool）上限（MB）。超过即视为不可重试的确定性失败。
    /// 默认 256；小内存机器可调小，转发超大文件可调大；0 = 不设限。
    pub spool_limit_mb: u64,
    /// 上游连接建立超时（秒）。默认 30；慢网络/高延迟上游可调大。
    pub connect_timeout_secs: u64,
    /// 上游两次读到数据之间的超时（秒；等响应头的时限是 `first_byte_timeout_secs`）。
    /// 默认 300；0 = 不设限。调过小会把「慢但活着」的上游变成确定性无限重试。
    pub read_timeout_secs: u64,
    /// 请求体大小上限（MB），超出直接 413。未设置时用 settings.json 的
    /// `max_body_mb`（全局默认，内置 128）；设 0 表示不设限。
    /// None = toml 未显式配置（由 main 启动时注入 settings 值）。
    pub max_body_mb: Option<u64>,
    /// 磁盘缓存：开启时超过内存驻留阈值（1 MiB）的请求体与响应 spool 溢写
    /// 到磁盘临时文件——进程内存与负载大小解耦（SSD 上 IO 开销为噪声级）。
    /// 未设置时用 settings.json 的 `disk_cache`（全局默认，内置 true）。
    /// 高并发大请求体实例务必开启；低并发实例可关闭保持全内存行为。
    /// None = toml 未显式配置（由 main 启动时注入 settings 值）。
    pub disk_cache: Option<bool>,
    /// 仅转发模式：请求体边收边转发上游、上游响应边收边回客户端，全程不缓冲、
    /// 不落盘、不重试，也不发保活心跳。进程内存与负载大小完全解耦，下游拿到
    /// 真·增量流（首字节即转发，不必等上游 spool 完成）。
    /// **代价是失去重试能力**——这是模式的定义而非缺陷：上游中途断开时响应就
    /// 到此截断（不注入任何上游未发出的字节），代理不会重放请求；上游请求失败
    /// 直接 502。要让出核心保障，请确认你确实接受这一点。
    /// 适用场景：需要本地改写请求头（鉴权/追加头）+ 需要真流式 + 对该 API 的
    /// 稳定性有把握（不频繁中断）。
    /// 本模式下**不生效**的配置项：`disk_cache`、`spool_limit_mb`、
    /// `keepalive_interval_secs`、`max_retry_backoff_secs`（重试与 spool 两条
    /// 链路整体不进）；`max_body_mb` 仍强制——流式途中计数超限即 413。
    /// 未设置时用 settings.json 的 `forward_only`（全局默认，内置 false）。
    /// None = toml 未显式配置（由 main 启动时注入 settings 值）。
    pub forward_only: Option<bool>,
    /// 重试规则表（config.toml 的 `[[retry.rules]]`）：按书写顺序匹配，第一条全部匹配器都成立的
    /// 规则生效；第 N 次尝试得到「有上游响应的失败」、命中规则且 N ≥ 它的 `max_attempts` 时不再
    /// 重试，把最后一次上游响应原样透传。**空 = 一切请求无限重试**。
    ///
    /// 动机：部分上游对特定端点确定性报错，无限重试只会让客户端永远等不到终态。哪些端点属于
    /// 这一类因上游而异，所以交由用户配置，不内置任何具体 URL。
    ///
    /// `path` 是对「`路径?查询串` 整体」的正则，编译时自动锚定两端（`^(?:模式)$`）：不含元字符
    /// 的普通路径即精准匹配；查询串必须显式出现在模式里。`max_attempts = 0` 表示不限，写在宽
    /// 规则前面当豁免。网络类失败不受规则封顶。
    pub retry_rules: Vec<RetryRule>,
    /// 等上游响应头的时限（秒，config.toml 的 `timeouts.first_byte_secs`）。0 = 不设限。
    pub first_byte_timeout_secs: u64,
    /// 请求历史里全部请求体与响应体加起来最多占多少内存（MiB，config.toml 的
    /// `history.body_budget_mb`）。超出时从最旧的请求开始丢体，记录本身留着；0 = 不保留体。
    pub history_body_budget_mb: u64,
    /// 入站 Host 白名单（防 DNS 重绑定）：Host 校验生效时，额外放行的主机名。
    ///
    /// 威胁：恶意网页把自己的域名重绑定到 127.0.0.1 后，浏览器会把它当「同源」
    /// 请求发到本代理（Host 头是攻击者的域名）——代理照常注入 api_key 转发，
    /// 攻击者就能花你的额度并读到响应。CLI 类 agent 一律用 localhost /
    /// 127.0.0.1 访问，Host 校验对它们零影响。
    ///
    /// 语义（Host 头去掉端口后，按 ASCII 大小写不敏感全等比较；条目里写的
    /// 端口同样忽略）：
    /// - 列表含 `"*"` → 关闭 Host 校验（任何 Host 都放行）
    /// - 校验生效的条件：监听地址是回环（127.0.0.0/8、::1、localhost），或本
    ///   列表非空。监听非回环地址（0.0.0.0、局域网 IP）且列表为空时**不做**
    ///   Host 校验——局域网/容器客户端的 Host 各式各样，默认拦截会破坏现有
    ///   用法（启动时另有非回环告警）
    /// - 校验生效时放行：localhost / 127.0.0.1 / [::1]、监听地址自身的主机部分
    ///   （0.0.0.0、[::] 这类通配地址除外）、以及本列表的条目
    /// - 空列表 = 与未配置相同（内置默认策略）——toml 写 `allowed_hosts = []`
    ///   可把 settings.json 的全局列表恢复成内置默认
    ///
    /// 未设置时用 settings.json 的 `allowed_hosts`（全局默认，内置空）。
    pub allowed_hosts: Option<Vec<String>>,
    /// 入站 Origin 白名单（防网页借本机代理调用上游）：默认拒绝**任何**携带
    /// Origin 头的请求——只有浏览器（及 Electron/WebView 类客户端）会发 Origin，
    /// CLI 类 agent 不发，拒绝它们对 CLI 零影响，却能挡住网页对本代理的跨站
    /// 调用（no-cors 的简单 POST 也会带 Origin）。
    ///
    /// 语义：条目与请求的 Origin 头按 ASCII 大小写不敏感全等比较（条目末尾的
    /// `/` 忽略），写法与浏览器发出的完全一致，如 `"http://localhost:5173"`；
    /// 含 `"*"` → 关闭 Origin 校验。空列表 = 与未配置相同（拒绝一切带 Origin
    /// 的请求）。与监听地址无关（非回环监听同样生效）。
    ///
    /// 未设置时用 settings.json 的 `allowed_origins`（全局默认，内置空）。
    pub allowed_origins: Option<Vec<String>>,
    /// 请求转换器（外部 format 程序）：请求体缓冲完成后交给它改写
    /// （body/headers/url/method），重试全程重放转换后的产物。
    /// **失败语义**：转换失败（进程崩溃/超时/error 行/协议错误）→ 502 + 原因，
    /// 不重试（按约定，请求未发往上游；各类失败的性质见 `transform::TransformError`
    /// 的分类说明）。与 forward_only 互斥（后者不缓冲请求体，转换器需要
    /// 全量 body）——共存时 validate() 启动报错。仅 toml 每实例配置，无
    /// settings.json 全局默认层（设计决策：转换是场景特定功能）。
    pub request_transform: Option<TransformConfig>,
    /// 响应转换器（外部 format 程序）：重试判定为「成功」后、回放前交给它
    /// 改写响应 body/headers。**失败语义**：转换失败 → 透传上游原始响应 +
    /// warn（响应已在手，可用性优先）——与请求侧的 502 语义不同，各按其性质。
    /// 仅 toml 每实例配置。
    pub response_transform: Option<TransformConfig>,
    /// 心跳转换器（外部 format 程序）：保活响应提交之后、真实字节回放之前，
    /// 每个 keepalive 间隔调用一次（信封 `stage = "heartbeat"`），回信的 body
    /// 就是这一拍写给客户端的字节（空 = 本拍不写）。用于按请求内容生成心跳：
    /// aProxy 不认识协议，能被客户端当作「事件」的心跳只能由懂协议的 format 生成。
    /// **失败语义**：调用失败、超时、回信不合 keepalive_heartbeat 的事件边界规则
    /// → 这一拍改发固定心跳（同一请求只 warn 一次）；上一拍的调用还没回来时，
    /// 这一拍直接发固定心跳而不排队——心跳不得被慢 format 拖住。请求转换完成
    /// 之前的各拍同样发固定心跳（此时还不知道最终请求）。仅 toml 每实例配置。
    pub heartbeat_transform: Option<TransformConfig>,
    /// 守护日志文件路径（自定义去向）：不设时写入 `~/.aproxy/logs/` 下按启动
    /// 时刻随机命名的文件（文件名不含端口——端口是易变标识，换端口重启后
    /// 日志照样按实例连续可查，实际路径由实例经 IPC 上报，客户端不拼路径）。
    /// 支持 `~` 展开；**相对路径相对 APROXY_HOME 解析**（守护进程的工作目录
    /// 不可靠，主目录是唯一稳定基准）。CLI `--log-file`（仅本次运行）优先于
    /// 本字段。**有意不设 settings.json 全局默认层**——日志去向是单实例语义
    /// （每个实例的配置文件各管各的），与 max_body_mb 等四个全局默认字段
    /// 形成对照，这是设计决策而非遗漏。
    pub log_file: Option<String>,
    /// spool 临时文件目录覆盖（serde skip，不落盘）。仅测试注入用：集成测试
    /// 进程内构建 AppState 时若无此覆盖，会按端口写入真实 ~/.aproxy/spool/。
    /// 生产路径为 None，实际目录 = ~/.aproxy/spool/<pid>-<启动标记短哈希>/。
    pub spool_dir_override: Option<PathBuf>,
}

/// 转换器 args 里「后一个参数是密钥」的旗标（比较时大小写不敏感）。format 程序常见的传 key
/// 写法：`--api-key sk-...` 或 `--api-key=sk-...`。
pub const SECRET_ARG_FLAGS: &[&str] = &[
    "--api-key",
    "--api_key",
    "--apikey",
    "--key",
    "--token",
    "--secret",
    "--password",
];

/// `history.body_budget_mb` 的内置默认值（MiB）。
pub const DEFAULT_HISTORY_BODY_BUDGET_MB: u64 = 64;

/// 请求体上限（字节）：toml 显式值 > settings 注入值 > 内置 128。
/// 0 = 不设限（与其他超时/上限配置的 0 语义一致）。
/// None（未注入 settings 值，如测试直连构建）按内置默认。
pub(crate) fn body_limit_bytes(max_body_mb: Option<u64>) -> usize {
    match max_body_mb {
        Some(0) => usize::MAX,
        Some(mb) => (mb as usize).saturating_mul(1024 * 1024),
        None => (DEFAULT_MAX_BODY_MB as usize).saturating_mul(1024 * 1024),
    }
}

fn default_listen_addr() -> String {
    "127.0.0.1:12345".to_string()
}

fn default_keepalive_secs() -> u64 {
    15
}

fn default_max_backoff_secs() -> u64 {
    320
}

fn default_spool_limit_mb() -> u64 {
    256
}

fn default_connect_timeout_secs() -> u64 {
    30
}

fn default_read_timeout_secs() -> u64 {
    300
}

impl Default for Config {
    fn default() -> Self {
        Self {
            base_url: String::new(),
            listen_addr: default_listen_addr(),
            api_key: None,
            extra_headers: HashMap::new(),
            override_headers: HashMap::new(),
            keepalive_interval_secs: default_keepalive_secs(),
            keepalive_trigger: None,
            keepalive_heartbeat: None,
            proxy: None,
            proxy_username: None,
            proxy_password: None,
            max_retry_backoff_secs: default_max_backoff_secs(),
            spool_limit_mb: default_spool_limit_mb(),
            connect_timeout_secs: default_connect_timeout_secs(),
            read_timeout_secs: default_read_timeout_secs(),
            max_body_mb: None,
            disk_cache: None,
            forward_only: None,
            retry_rules: Vec::new(),
            first_byte_timeout_secs: default_read_timeout_secs(),
            history_body_budget_mb: DEFAULT_HISTORY_BODY_BUDGET_MB,
            allowed_hosts: None,
            allowed_origins: None,
            request_transform: None,
            response_transform: None,
            heartbeat_transform: None,
            log_file: None,
            spool_dir_override: None,
        }
    }
}

impl Config {
    /// 归一化：去除 base_url 末尾斜杠；清理空 api_key；清理空代理配置。
    pub fn normalized(mut self) -> Self {
        self.base_url = normalize_base_url(&self.base_url);
        if let Some(k) = &self.api_key {
            if k.trim().is_empty() {
                self.api_key = None;
            } else {
                self.api_key = Some(k.trim().to_string());
            }
        }
        // 头 k/v 统一 trim（与 CLI 路径 parse_kv 行为一致），trim 后 key 为空则剔除；
        // trim 后同键冲突时保留先出现者——HashMap::collect 对坍缩键的赢家是非确定的
        self.extra_headers = {
            let mut m = HashMap::new();
            for (k, v) in &self.extra_headers {
                let k = k.trim();
                if k.is_empty() {
                    continue;
                }
                m.entry(k.to_string())
                    .or_insert_with(|| v.trim().to_string());
            }
            m
        };
        self.override_headers = {
            let mut m = HashMap::new();
            for (k, v) in &self.override_headers {
                let k = k.trim();
                if k.is_empty() {
                    continue;
                }
                m.entry(k.to_string())
                    .or_insert_with(|| v.trim().to_string());
            }
            m
        };
        // 代理：空白视为未设置
        self.proxy = self
            .proxy
            .take()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        self.proxy_username = self
            .proxy_username
            .take()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        self.proxy_password = self
            .proxy_password
            .take()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        // 转换器：command 统一 trim，trim 后为空视为未配置（与代理空白语义一致）
        self.request_transform = trim_transform_command(self.request_transform.take());
        self.response_transform = trim_transform_command(self.response_transform.take());
        self.heartbeat_transform = trim_transform_command(self.heartbeat_transform.take());
        self
    }

    /// 这份配置的全部语义问题（空 = 可以启动）。键是 config.toml 里的键路径，码取自码表的
    /// 诊断族；`message` 是英文，里面的 URL 已打码。分层解析把它们变成带文件与行列的诊断。
    pub fn problems(&self) -> Vec<Problem> {
        let mut out = Vec::new();
        let mut push =
            |code: &'static str, key: &str, reason: Option<&'static str>, message: String| {
                out.push(Problem {
                    code,
                    key: key.to_string(),
                    reason,
                    message,
                })
            };
        let base = aproxy_ipc::redact::url(&self.base_url);
        if self.base_url.trim().is_empty() {
            push(
                "config_missing_key",
                "base_url",
                None,
                "base_url is not set: it is the upstream API this instance forwards to".to_string(),
            );
        } else if let Err(reason) = self.base_url_problem() {
            let message = match reason {
                "not_http_url" => {
                    format!("base_url must start with http:// or https://, not {base}")
                }
                _ => format!(
                    "base_url must not contain ? or #: the request path is appended to it, so the path would end up in the query ({base})"
                ),
            };
            push("config_invalid_value", "base_url", Some(reason), message);
        }
        // 监听地址必须带端口：缺端口或端口越界时 bind 的失败不是「端口被占用」，提前说清楚
        if crate::daemon::port_of(&self.listen_addr)
            .parse::<u16>()
            .is_err()
        {
            push(
                "config_invalid_value",
                "listen_addr",
                Some("no_port"),
                format!(
                    "listen_addr has no port or the port is not valid: {} (example: 127.0.0.1:12345)",
                    self.listen_addr
                ),
            );
        }
        if let Some(p) = &self.proxy {
            let shown = aproxy_ipc::redact::url(p);
            match url::Url::parse(p) {
                Err(e) => push(
                    "config_invalid_value",
                    "proxy",
                    Some("not_a_url"),
                    format!("proxy is not a URL ({shown}): {e}"),
                ),
                Ok(url) => {
                    if !matches!(
                        url.scheme(),
                        "http" | "https" | "socks4" | "socks4a" | "socks5" | "socks5h"
                    ) {
                        push(
                            "config_invalid_value",
                            "proxy",
                            Some("unsupported_scheme"),
                            format!(
                                "proxy must be an http, https, socks4 or socks5 URL, not {shown}"
                            ),
                        );
                    } else if url.host_str().is_none() {
                        push(
                            "config_invalid_value",
                            "proxy",
                            Some("no_host"),
                            format!("proxy has no host: {shown}"),
                        );
                    }
                }
            }
        } else if self.proxy_username.is_some() || self.proxy_password.is_some() {
            // 只有配置了代理 URL，用户名与密码才有意义，否则是配置遗漏
            push(
                "config_missing_key",
                "proxy",
                None,
                "proxy_username or proxy_password is set but proxy is not".to_string(),
            );
        }
        if let Some(hb) = &self.keepalive_heartbeat
            && let Some(why) = heartbeat_problem(hb)
        {
            push(
                "config_invalid_value",
                "keepalive.heartbeat",
                Some("breaks_sse_framing"),
                format!("keepalive.heartbeat {why}"),
            );
        }
        if let Some(t) = &self.keepalive_trigger
            && KeepaliveTrigger::parse(t).is_none()
        {
            push(
                "config_invalid_value",
                "keepalive.trigger",
                Some("not_one_of"),
                format!(
                    "keepalive.trigger must be \"accept\", \"body_stream\" or \"any\", not {t:?}"
                ),
            );
        }
        // 头：名字与值要能写进一个 HTTP 请求，否则每个请求都会在组装时失败
        if let Some(key) = &self.api_key
            && http::HeaderValue::from_str(&format!("Bearer {key}")).is_err()
        {
            push(
                "config_invalid_value",
                "api_key",
                Some("invalid_header_value"),
                "api_key contains characters that cannot be sent in an HTTP header".to_string(),
            );
        }
        for (table, headers) in [
            ("extra_headers", &self.extra_headers),
            ("override_headers", &self.override_headers),
        ] {
            let mut names: Vec<&String> = headers.keys().collect();
            names.sort();
            for name in names {
                let key = format!("{table}.{}", quote_key(name));
                if http::HeaderName::from_bytes(name.as_bytes()).is_err() {
                    push(
                        "config_invalid_value",
                        &key,
                        Some("invalid_header_name"),
                        format!("{name:?} is not a valid HTTP header name"),
                    );
                } else if http::HeaderValue::from_str(&headers[name]).is_err() {
                    push(
                        "config_invalid_value",
                        &key,
                        Some("invalid_header_value"),
                        format!(
                            "the value of header {name} contains characters that cannot be sent in an HTTP header"
                        ),
                    );
                }
            }
        }
        for (index, rule) in self.retry_rules.iter().enumerate() {
            if let Some(pattern) = &rule.path
                && let Err(e) = Self::compile_bounded_retry_pattern(pattern)
            {
                push(
                    "config_invalid_value",
                    &format!("retry.rules[{index}].path"),
                    Some("invalid_regex"),
                    format!("not a valid regular expression: {e}"),
                );
            }
        }
        // forward 不缓冲请求体，转换器需要全量 body：两者同开是配置矛盾
        if self.forward_only_enabled()
            && (self.request_transform.is_some()
                || self.response_transform.is_some()
                || self.heartbeat_transform.is_some())
        {
            push(
                "config_conflict",
                "delivery",
                None,
                "delivery = \"forward\" cannot be combined with a format: forward does not buffer the request body, and a format needs all of it".to_string(),
            );
        }
        out
    }

    /// 校验：没有语义问题时 Ok，否则 Err 带第一个问题的说明（见 [`Config::problems`]）。
    pub fn validate(&self) -> Result<(), String> {
        match self.problems().into_iter().next() {
            None => Ok(()),
            Some(problem) => Err(problem.message),
        }
    }

    /// base_url 自身的格式问题（scheme、无 query/fragment），返回诊断的 `reason`。空值是否
    /// 允许由调用方决定：启动时禁止，而配置可以分多次写成，中间态合法。
    pub fn base_url_problem(&self) -> Result<(), &'static str> {
        // scheme 判定大小写不敏感（"HTTP://" 亦合法）
        let scheme_lower = self.base_url.trim().to_ascii_lowercase();
        if !(scheme_lower.starts_with("http://") || scheme_lower.starts_with("https://")) {
            return Err("not_http_url");
        }
        // 拒绝带 query/fragment 的 base_url：拼接 path 时会把路径拼进 query，静默错路由
        if self.base_url.contains('?') || self.base_url.contains('#') {
            return Err("has_query_or_fragment");
        }
        Ok(())
    }

    /// 是否启用保活心跳
    pub fn keepalive_enabled(&self) -> bool {
        self.keepalive_interval_secs > 0
    }

    /// `override_headers` 里显式配置、却会被保活通道覆盖掉的 accept-encoding 值。
    ///
    /// 保活适用的请求发往上游时 accept-encoding 一律改为 `identity`（往压缩流里
    /// 插明文心跳会让客户端解压失败），所以这里配的非 identity 值只对不走保活的
    /// 请求生效。返回 Some(配置值) 供启动时 warn 一次；保活关闭、仅转发模式
    /// （不走保活通道）、或本就配的 identity 时返回 None。
    pub fn accept_encoding_override_shadowed_by_keepalive(&self) -> Option<&str> {
        if self.forward_only_enabled() || !self.keepalive_enabled() {
            return None;
        }
        self.override_headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("accept-encoding"))
            .map(|(_, v)| v.as_str())
            .filter(|v| !v.trim().eq_ignore_ascii_case("identity"))
    }

    /// 保活间隔
    pub fn keepalive_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.keepalive_interval_secs)
    }

    /// 保活触发条件生效值（同 body_limit_bytes 的注入/回退语义：toml 显式值 >
    /// settings 注入值 > 内置 `any`）。消费点必须走本方法——不经 settings 注入
    /// 的构建路径（doctor/find/测试）上 `None` 是常态。非法写法在 validate()
    /// 拦截；未经 validate 的直连构建（测试）遇到非法值按内置默认处理。
    pub fn keepalive_trigger(&self) -> KeepaliveTrigger {
        self.keepalive_trigger
            .as_deref()
            .and_then(KeepaliveTrigger::parse)
            .unwrap_or(DEFAULT_KEEPALIVE_TRIGGER)
    }

    /// 心跳字节生效值：toml 显式值，否则内置的 SSE 注释。非法写法在 validate()
    /// 拦截，消费点直接取用。
    pub fn keepalive_heartbeat(&self) -> &[u8] {
        self.keepalive_heartbeat
            .as_deref()
            .unwrap_or(DEFAULT_KEEPALIVE_HEARTBEAT)
            .as_bytes()
    }

    /// 请求体大小上限（字节）。max_body_mb 已在启动时注入 settings 值（toml
    /// 覆盖 settings），此处仅处理未注入（测试直连）的回退。
    pub fn body_limit_bytes(&self) -> usize {
        body_limit_bytes(self.max_body_mb)
    }

    /// 磁盘缓存是否启用（同 body_limit_bytes 的注入/回退语义）
    pub fn disk_cache_enabled(&self) -> bool {
        self.disk_cache.unwrap_or(DEFAULT_DISK_CACHE)
    }

    /// 仅转发模式是否启用（同 body_limit_bytes 的注入/回退语义：toml 显式值 >
    /// settings 注入值 > 内置 false）。
    ///
    /// 所有消费点都必须走本方法而非 `Option::unwrap()`——doctor 的
    /// parse_config_file、find::discover 与大量测试直接构建的 AppState 都不经
    /// settings 注入，`None` 在这些路径上是常态。
    pub fn forward_only_enabled(&self) -> bool {
        self.forward_only.unwrap_or(DEFAULT_FORWARD_ONLY)
    }

    /// 这份配置里 aproxy 自己知道的密钥原文：请求历史入库时把它们从体与头里换掉。
    ///
    /// 收的是 `api_key`、代理口令（单独配的与写在代理 URL 里的）、两张头表里密钥头的值
    /// （`Bearer xxx` 这样带方案的，连同后面的凭据单独再收一份）、format 的 `env` 值与 args 里
    /// 密钥旗标之后的值。过短的由 [`aproxy_ipc::redact::Secrets`] 自己丢掉。
    pub fn known_secrets(&self) -> aproxy_ipc::redact::Secrets {
        let mut known: Vec<String> = Vec::new();
        known.extend(self.api_key.clone());
        known.extend(self.proxy_password.clone());
        if let Some(password) = self
            .proxy
            .as_deref()
            .and_then(|proxy| url::Url::parse(proxy).ok())
            .and_then(|proxy| proxy.password().map(str::to_string))
        {
            known.push(password);
        }
        for (name, value) in self.extra_headers.iter().chain(&self.override_headers) {
            if aproxy_ipc::redact::is_secret_header(name) {
                known.push(value.clone());
                if let Some((_, credential)) = value.trim().rsplit_once(' ') {
                    known.push(credential.to_string());
                }
            }
        }
        let is_secret_flag =
            |arg: &str| SECRET_ARG_FLAGS.iter().any(|f| f.eq_ignore_ascii_case(arg));
        let formats = [
            &self.request_transform,
            &self.response_transform,
            &self.heartbeat_transform,
        ];
        for format in formats.into_iter().flatten() {
            known.extend(format.env.values().cloned());
            let mut next_is_secret = false;
            for arg in &format.args {
                if std::mem::take(&mut next_is_secret) {
                    known.push(arg.clone());
                } else if is_secret_flag(arg) {
                    next_is_secret = true;
                } else if let Some((flag, value)) = arg.split_once('=')
                    && is_secret_flag(flag)
                {
                    known.push(value.to_string());
                }
            }
        }
        aproxy_ipc::redact::Secrets::new(known)
    }

    /// 编译重试规则的 `path` 模式：对「`路径?查询串` 整体」匹配，自动锚定两端
    /// ——不含元字符的普通路径即精准匹配，通配需显式写 `.*`。配置的解析与
    /// AppState::new 共用此入口，两处语义不可能分叉。
    pub fn compile_bounded_retry_pattern(pattern: &str) -> Result<regex::Regex, regex::Error> {
        regex::Regex::new(&format!("^(?:{pattern})$"))
    }

    /// 入站 Host 白名单条目（同 bounded_retry_paths 的注入/回退语义：toml 显式
    /// 值 > settings 注入值 > 内置空 = 内置默认策略）。消费点必须走本方法而非
    /// `Option::unwrap()`——不经 settings 注入的构建路径上 `None` 是常态。
    pub fn allowed_hosts(&self) -> &[String] {
        self.allowed_hosts.as_deref().unwrap_or(&[])
    }

    /// 入站 Origin 白名单条目（注入/回退语义同 allowed_hosts；内置空 = 拒绝
    /// 一切携带 Origin 的请求）。
    pub fn allowed_origins(&self) -> &[String] {
        self.allowed_origins.as_deref().unwrap_or(&[])
    }

    /// 监听地址是否为回环：IP 字面量按 `is_loopback`（127.0.0.0/8、::1）判定；
    /// 主机名形态只认 `localhost`。其余（0.0.0.0、[::]、局域网 IP、其他主机名）
    /// 一律视为非回环——判定结果决定 Host 校验的默认开关与启动告警，宁可多告警
    /// 也不把对外暴露的监听误判为「仅本机」。
    pub fn listen_is_loopback(&self) -> bool {
        let addr = self.listen_addr.trim();
        if let Ok(sa) = addr.parse::<std::net::SocketAddr>() {
            return sa.ip().is_loopback();
        }
        let host = addr.rsplit_once(':').map_or(addr, |(h, _)| h);
        host.eq_ignore_ascii_case("localhost")
    }

    /// 本实例是否会给入站请求注入凭据：api_key，或 extra/override_headers 里配置
    /// 了 Authorization / x-api-key。用于非回环监听告警的措辞分级——注入凭据的
    /// 实例对外监听，等于把 key 交给每一个能连上端口的主机。
    pub(crate) fn injects_credentials(&self) -> bool {
        self.api_key.is_some()
            || self
                .override_headers
                .keys()
                .chain(self.extra_headers.keys())
                .any(|k| {
                    k.eq_ignore_ascii_case("authorization") || k.eq_ignore_ascii_case("x-api-key")
                })
    }

    /// 非回环监听的告警文案；回环监听返回 None。start 输出、守护的
    /// startup.log 共用这一份文案；体检按相同的监听与凭据判定给出结构化诊断。
    pub fn listen_exposure_warning(&self) -> Option<String> {
        if self.listen_is_loopback() {
            return None;
        }
        let addr = &self.listen_addr;
        let port = addr.rsplit(':').next().unwrap_or("12345");
        Some(if self.injects_credentials() {
            format!(
                "监听地址 {addr} 不是回环地址，且本实例配置了 api_key（或 Authorization / x-api-key 鉴权头）：代理会把它注入每一个入站请求——任何能连到该端口的主机都能用你的 key 调用上游、费用记在你的账上，等于把 key 共享给整个局域网。只给本机用请改回 127.0.0.1:{port}"
            )
        } else {
            format!(
                "监听地址 {addr} 不是回环地址：局域网/同网段内任何能连到该端口的主机都能经本代理访问上游。只给本机用请改回 127.0.0.1:{port}；确需对外开放请只在可信网络中使用"
            )
        })
    }
}

/// 转换器 command 归一化：trim + `~` 展开；trim 后为空 = 配置视为未设置。
///
/// `~` 展开是**必须的**：transform 的 command 不经 shell 直接 spawn（tokio
/// Command 不做任何路径展开），而官方文档/skill 推荐的写法恰是
/// `~/.aproxy/bin/aproxy-format`——不展开的话该字符串按「当前目录下名为
/// `~` 的相对路径」解析，照抄示例的用户每请求必 502。
fn trim_transform_command(t: Option<TransformConfig>) -> Option<TransformConfig> {
    t.map(|mut t| {
        t.command = expand_tilde(t.command.trim());
        t
    })
    .filter(|t| !t.command.is_empty())
}

/// `~` 前缀展开：`~/x`、`~\x`、裸 `~` → 用户目录；其余原样。用户目录是 C1 的那一个
/// （见 `aproxy_ipc::home::user_dir`）：`~` 在 aproxy 里处处指同一个目录，配置里的 `~` 与命令行
/// 路径参数里的 `~`、home 的默认位置 `~/.aproxy` 不会落在两个地方。
/// 展开产物统一用 `/` 分隔（Windows API 同样接受）。
pub fn expand_tilde(s: &str) -> String {
    let home = || {
        aproxy_ipc::home::user_dir()
            .map(|p| {
                p.display()
                    .to_string()
                    .trim_end_matches(['/', '\\'])
                    .to_string()
            })
            .unwrap_or_default()
    };
    if s == "~" {
        return home();
    }
    if let Some(rest) = s.strip_prefix("~/").or_else(|| s.strip_prefix("~\\")) {
        return format!("{}/{}", home(), rest);
    }
    s.to_string()
}

/// 返回配置文件路径：`~/.aproxy/config.toml`。
pub fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

/// 返回配置目录：aProxy 主目录（`APROXY_HOME`，未设 = `~/.aproxy`，
/// 见 `aproxy_ipc::home::home`）。
pub fn config_dir() -> PathBuf {
    aproxy_ipc::home::home()
}

/// 一个 URL 拆出来的几段。只做拆分，不解码、不校验字符。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UrlParts {
    /// 小写
    pub scheme: String,
    /// `用户[:口令]`，没有时为 None
    pub userinfo: Option<String>,
    /// 小写；IPv6 带方括号
    pub host: String,
    pub port: Option<String>,
    /// 主机之后的全部内容（路径、查询串、片段），可能为空
    pub rest: String,
}

impl UrlParts {
    /// 去掉 userinfo 之后的 URL
    pub(crate) fn without_userinfo(&self) -> String {
        let port = self
            .port
            .as_ref()
            .map(|p| format!(":{p}"))
            .unwrap_or_default();
        format!("{}://{}{port}{}", self.scheme, self.host, self.rest)
    }
}

/// 把 `scheme://[userinfo@]host[:port][rest]` 拆开；不是这个形状时返回 None。
pub(crate) fn split_url(url: &str) -> Option<UrlParts> {
    let (scheme, after) = url.split_once("://")?;
    if scheme.is_empty()
        || !scheme
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"+-.".contains(&b))
    {
        return None;
    }
    let end = after.find(['/', '?', '#']).unwrap_or(after.len());
    let (authority, rest) = after.split_at(end);
    let (userinfo, host_port) = match authority.rsplit_once('@') {
        Some((userinfo, host_port)) => (Some(userinfo.to_string()), host_port),
        None => (None, authority),
    };
    // IPv6 字面量带方括号，里面的冒号不是端口分隔符
    let (host, port) = match host_port.rfind(':') {
        Some(colon) if !host_port[colon..].contains(']') => (
            &host_port[..colon],
            Some(host_port[colon + 1..].to_string()),
        ),
        _ => (host_port, None),
    };
    if host.is_empty() {
        return None;
    }
    Some(UrlParts {
        scheme: scheme.to_ascii_lowercase(),
        userinfo,
        host: host.to_ascii_lowercase(),
        port,
        rest: rest.to_string(),
    })
}

/// base_url 的规范化：去掉首尾空白与末尾斜杠，scheme 与主机名转小写，去掉默认端口
/// （http 的 80、https 的 443）。userinfo、路径原样保留。拆不开的（不是 URL）只去空白与
/// 末尾斜杠——它的问题由校验去报。
pub fn normalize_base_url(raw: &str) -> String {
    let trimmed = raw.trim().trim_end_matches('/');
    let Some(mut parts) = split_url(trimmed) else {
        return trimmed.to_string();
    };
    let default_port = match parts.scheme.as_str() {
        "http" => Some("80"),
        "https" => Some("443"),
        _ => None,
    };
    if parts.port.as_deref() == default_port {
        parts.port = None;
    }
    let userinfo = parts
        .userinfo
        .as_ref()
        .map(|u| format!("{u}@"))
        .unwrap_or_default();
    let port = parts
        .port
        .as_ref()
        .map(|p| format!(":{p}"))
        .unwrap_or_default();
    format!(
        "{}://{userinfo}{}{port}{}",
        parts.scheme, parts.host, parts.rest
    )
}

/// 配置的一个语义问题（见 [`Config::problems`]）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// 码表里诊断族的名字
    pub code: &'static str,
    /// config.toml 里的键路径
    pub key: String,
    /// `config_invalid_value` 的 `details.reason`
    pub reason: Option<&'static str>,
    /// 一句英文说明，URL 已打码
    pub message: String,
}

/// 键路径里的一段：名字，以及（数组里的条目）下标。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeySegment {
    pub name: String,
    pub index: Option<usize>,
}

/// 解析键路径：点号分隔，名字里有点号等字符时加引号，数组条目带下标，例如
/// `retry.max_backoff_secs`、`extra_headers."x.y"`、`format[1].stages`。写法不对时返回 None。
pub fn parse_key_path(path: &str) -> Option<Vec<KeySegment>> {
    let mut segments = Vec::new();
    let mut rest = path;
    loop {
        let name;
        if let Some(quoted) = rest.strip_prefix('"') {
            // 带引号的一段：反斜杠转义它后面的一个字符
            let mut out = String::new();
            let mut chars = quoted.char_indices();
            let end = loop {
                match chars.next()? {
                    (i, '"') => break i,
                    (_, '\\') => out.push(chars.next()?.1),
                    (_, c) => out.push(c),
                }
            };
            name = out;
            rest = &quoted[end + 1..];
        } else {
            let end = rest.find(['.', '[']).unwrap_or(rest.len());
            name = rest[..end].to_string();
            let bare = !name.is_empty()
                && name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
            if !bare {
                return None;
            }
            rest = &rest[end..];
        }
        let mut index = None;
        if let Some(after) = rest.strip_prefix('[') {
            let close = after.find(']')?;
            index = Some(after[..close].parse().ok()?);
            rest = &after[close + 1..];
        }
        segments.push(KeySegment { name, index });
        if rest.is_empty() {
            return Some(segments);
        }
        rest = rest.strip_prefix('.')?;
    }
}

/// 把一个名字写成 TOML 键路径里的一段：裸键原样，其余加引号（`x-api-key` 是裸键，
/// `a.b` 要写成 `"a.b"`）。
pub fn quote_key(name: &str) -> String {
    let bare = !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if bare {
        name.to_string()
    } else {
        format!("\"{}\"", name.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tilde_expands_to_the_c1_user_dir() {
        // 与路径参数、home 的默认位置同一来源；产物用 `/` 分隔，只认开头的 `~`
        let user = aproxy_ipc::home::user_dir().unwrap().display().to_string();
        let user = user.trim_end_matches(['/', '\\']);
        assert_eq!(expand_tilde("~"), user);
        assert_eq!(expand_tilde("~/bin/x"), format!("{user}/bin/x"));
        assert_eq!(expand_tilde(r"~\bin"), format!("{user}/bin"));
        assert_eq!(expand_tilde("~user/x"), "~user/x");
        assert_eq!(expand_tilde("/opt/~/x"), "/opt/~/x");
    }

    #[test]
    fn validate_rejects_empty() {
        let cfg = Config {
            base_url: "".to_string(),
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validate_rejects_non_http() {
        let cfg = Config {
            base_url: "ftp://example.com".to_string(),
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validate_accepts_https() {
        let cfg = Config {
            base_url: "https://api.anthropic.com".to_string(),
            ..Default::default()
        };
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn normalized_trims_trailing_slash() {
        let cfg = Config {
            base_url: "https://api.anthropic.com/".to_string(),
            ..Default::default()
        }
        .normalized();
        assert_eq!(cfg.base_url, "https://api.anthropic.com");
    }

    #[test]
    fn normalized_trims_api_key_and_empties() {
        // api_key 前后空白被裁剪
        let cfg = Config {
            base_url: "https://api.example.com".to_string(),
            api_key: Some("  sk-test  ".to_string()),
            ..Default::default()
        }
        .normalized();
        assert_eq!(cfg.api_key.as_deref(), Some("sk-test"));

        // 全空白或空串视为未设置
        for k in ["   ", ""] {
            let cfg = Config {
                base_url: "https://api.example.com".to_string(),
                api_key: Some(k.to_string()),
                ..Default::default()
            }
            .normalized();
            assert!(cfg.api_key.is_none(), "api_key {:?} 应被置空", k);
        }
    }

    #[test]
    fn normalized_removes_blank_extra_header_keys() {
        let cfg = Config {
            base_url: "https://api.example.com".to_string(),
            extra_headers: HashMap::from([
                ("x-keep".to_string(), "1".to_string()),
                ("   ".to_string(), "2".to_string()),
                ("".to_string(), "3".to_string()),
            ]),
            ..Default::default()
        }
        .normalized();
        assert_eq!(cfg.extra_headers.len(), 1);
        assert_eq!(cfg.extra_headers.get("x-keep").unwrap(), "1");
    }

    #[test]
    fn default_port_is_12345() {
        assert_eq!(default_listen_addr(), "127.0.0.1:12345");
        assert_eq!(Config::default().listen_addr, "127.0.0.1:12345");
    }

    #[test]
    fn accept_encoding_override_shadowed_only_when_keepalive_rewrites_it() {
        let with = |value: &str| Config {
            override_headers: HashMap::from([("Accept-Encoding".to_string(), value.to_string())]),
            ..Default::default()
        };
        // 保活开启（默认 15s）+ 非 identity：被覆盖，名字大小写不敏感
        assert_eq!(
            with("gzip, br").accept_encoding_override_shadowed_by_keepalive(),
            Some("gzip, br")
        );
        // 本就配 identity：与改写结果一致，无需提示
        assert_eq!(
            with(" Identity ").accept_encoding_override_shadowed_by_keepalive(),
            None
        );
        // 保活关闭 / 仅转发模式：不走保活通道，配置值照常生效
        let mut off = with("gzip");
        off.keepalive_interval_secs = 0;
        assert_eq!(off.accept_encoding_override_shadowed_by_keepalive(), None);
        let mut fwd = with("gzip");
        fwd.forward_only = Some(true);
        assert_eq!(fwd.accept_encoding_override_shadowed_by_keepalive(), None);
        // 未配置该头
        assert_eq!(
            Config::default().accept_encoding_override_shadowed_by_keepalive(),
            None
        );
    }

    #[test]
    fn keepalive_defaults() {
        let cfg = Config::default();
        assert!(cfg.keepalive_enabled());
        assert_eq!(cfg.keepalive_interval_secs, 15);
    }

    #[test]
    fn proxy_defaults_to_none() {
        let cfg = Config::default();
        assert!(cfg.proxy.is_none());
        assert!(cfg.proxy_username.is_none());
        assert!(cfg.proxy_password.is_none());
    }

    #[test]
    fn default_completeness() {
        let cfg = Config::default();
        assert_eq!(cfg.base_url, "");
        assert_eq!(cfg.listen_addr, "127.0.0.1:12345");
        assert_eq!(cfg.keepalive_interval_secs, 15);
        assert!(cfg.api_key.is_none());
        assert!(cfg.extra_headers.is_empty());
        assert!(cfg.override_headers.is_empty());
        assert!(cfg.proxy.is_none());
        assert!(cfg.proxy_username.is_none());
        assert!(cfg.proxy_password.is_none());
        assert_eq!(cfg.max_retry_backoff_secs, 320);
        assert_eq!(cfg.spool_limit_mb, 256);
        assert_eq!(cfg.connect_timeout_secs, 30);
        assert_eq!(cfg.read_timeout_secs, 300);
    }

    #[test]
    fn keepalive_heartbeat_default_and_boundary_rule() {
        let base = || Config {
            base_url: "https://x.example.com".into(),
            ..Config::default()
        };
        assert_eq!(base().keepalive_heartbeat(), b": keepalive\n\n");
        // 合法：空行、注释、CRLF 收尾、完整事件
        for ok in [
            "\n",
            ": ping\n",
            ": a\r\n\r\n",
            "event: ping\ndata: {}\n\n",
            "data: x\r\n\r\n",
        ] {
            let cfg = Config {
                keepalive_heartbeat: Some(ok.to_string()),
                ..base()
            };
            assert!(cfg.validate().is_ok(), "{ok:?} 应合法");
            assert_eq!(cfg.keepalive_heartbeat(), ok.as_bytes());
        }
        // 非法：空、半行、字段行缺空行收尾（会和回放的第一个事件拼在一起）
        for bad in ["", ": keepalive", "data: x\n", "event: ping\ndata: {}\n"] {
            let err = Config {
                keepalive_heartbeat: Some(bad.to_string()),
                ..base()
            }
            .validate()
            .unwrap_err();
            assert!(err.contains("keepalive.heartbeat"), "{bad:?}: {err}");
        }
    }

    #[test]
    fn listen_loopback_detection() {
        let at = |addr: &str| Config {
            listen_addr: addr.to_string(),
            ..Default::default()
        };
        for addr in [
            "127.0.0.1:12345",
            "127.0.0.5:12345",
            "[::1]:12345",
            "localhost:12345",
            "LOCALHOST:1",
        ] {
            assert!(at(addr).listen_is_loopback(), "{addr} 应判为回环");
        }
        for addr in [
            "0.0.0.0:12345",
            "[::]:12345",
            "192.168.1.10:12345",
            "myhost.lan:12345",
            ":12345",
        ] {
            assert!(!at(addr).listen_is_loopback(), "{addr} 应判为非回环");
        }
    }

    #[test]
    fn listen_exposure_warning_escalates_with_credentials() {
        let loopback = Config::default();
        assert!(
            loopback.listen_exposure_warning().is_none(),
            "回环监听不告警"
        );

        let open = Config {
            listen_addr: "0.0.0.0:12345".to_string(),
            ..Default::default()
        };
        let w = open.listen_exposure_warning().expect("非回环必须告警");
        assert!(
            w.contains("0.0.0.0:12345") && w.contains("127.0.0.1:12345"),
            "{w}"
        );
        assert!(!w.contains("api_key"), "未注入凭据时不应使用 key 措辞: {w}");

        // 配了 api_key：措辞升级，点明「等于把 key 共享给局域网」
        let with_key = Config {
            api_key: Some("sk-test".to_string()),
            ..open.clone()
        };
        let w = with_key.listen_exposure_warning().unwrap();
        assert!(
            w.contains("api_key") && w.contains("共享给整个局域网"),
            "{w}"
        );
        assert!(!w.contains("sk-test"), "告警文案不得带出 key 本身: {w}");

        // 鉴权头同理（override/extra，大小写不敏感）
        let with_header = Config {
            override_headers: HashMap::from([("X-Api-Key".to_string(), "k".to_string())]),
            ..open
        };
        assert!(
            with_header
                .listen_exposure_warning()
                .unwrap()
                .contains("共享给整个局域网")
        );
    }

    #[test]
    fn proxy_normalized_trims_and_empties() {
        let cfg = Config {
            base_url: "https://api.example.com".to_string(),
            proxy: Some("  http://127.0.0.1:7890  ".to_string()),
            proxy_username: Some("  user  ".to_string()),
            proxy_password: Some("  pass  ".to_string()),
            ..Default::default()
        }
        .normalized();
        assert_eq!(cfg.proxy.as_deref(), Some("http://127.0.0.1:7890"));
        assert_eq!(cfg.proxy_username.as_deref(), Some("user"));
        assert_eq!(cfg.proxy_password.as_deref(), Some("pass"));

        // 纯空白视为未设置
        let cfg = Config {
            base_url: "https://api.example.com".to_string(),
            proxy: Some("   ".to_string()),
            proxy_username: Some("".to_string()),
            ..Default::default()
        }
        .normalized();
        assert!(cfg.proxy.is_none());
        assert!(cfg.proxy_username.is_none());
    }

    #[test]
    fn validate_accepts_proxy_schemes() {
        for p in [
            "http://127.0.0.1:7890",
            "https://proxy.example.com:8443",
            "socks5://127.0.0.1:1080",
            "socks4://127.0.0.1:1080",
            "socks5://user:pass@127.0.0.1:1080",
        ] {
            let cfg = Config {
                base_url: "https://api.example.com".to_string(),
                proxy: Some(p.to_string()),
                ..Default::default()
            };
            assert!(cfg.validate().is_ok(), "应接受代理 {p}");
        }
    }

    #[test]
    fn validate_accepts_socks_proxy_variants() {
        // socks5h/socks4a（DNS 走代理解析的变体）也应被接受
        for p in ["socks5h://127.0.0.1:1080", "socks4a://127.0.0.1:1080"] {
            let cfg = Config {
                base_url: "https://api.example.com".to_string(),
                proxy: Some(p.to_string()),
                ..Default::default()
            };
            assert!(cfg.validate().is_ok(), "应接受代理 {p}");
        }
    }

    #[test]
    fn validate_rejects_bad_proxy() {
        for p in [
            "ftp://127.0.0.1:21", // 不支持的协议
            "not-a-url",          // 缺少协议
            "http://",            // 缺少主机
        ] {
            let cfg = Config {
                base_url: "https://api.example.com".to_string(),
                proxy: Some(p.to_string()),
                ..Default::default()
            };
            assert!(cfg.validate().is_err(), "应拒绝代理 {p}");
        }
    }

    #[test]
    fn validate_rejects_creds_without_proxy_url() {
        let cfg = Config {
            base_url: "https://api.example.com".to_string(),
            proxy_username: Some("alice".to_string()),
            ..Default::default()
        };
        assert!(cfg.validate().is_err());

        // 仅有 proxy URL 时凭据字段不填应通过
        let cfg = Config {
            base_url: "https://api.example.com".to_string(),
            proxy: Some("http://127.0.0.1:7890".to_string()),
            ..Default::default()
        };
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn validate_rejects_base_url_with_query_or_fragment() {
        // 带 query/fragment 的 base_url 拼接 path 时会把路径拼进 query，静默错路由
        for p in [
            "https://api.example.com?v=1",
            "https://api.example.com#frag",
        ] {
            let cfg = Config {
                base_url: p.to_string(),
                ..Default::default()
            };
            assert!(
                cfg.validate().is_err(),
                "应拒绝含 query/fragment 的 base_url {p}"
            );
        }
        // 带路径前缀仍合法（如反向代理子路径）
        let cfg = Config {
            base_url: "https://api.example.com/api".to_string(),
            ..Default::default()
        };
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn validate_accepts_uppercase_scheme() {
        for p in ["HTTP://api.example.com", "Https://api.example.com"] {
            let cfg = Config {
                base_url: p.to_string(),
                ..Default::default()
            };
            assert!(cfg.validate().is_ok(), "大写 scheme {p} 应被接受");
        }
    }

    #[test]
    fn normalized_trims_header_keys_and_values() {
        // 配置文件路径进来的头 k/v 统一 trim，与 CLI parse_kv 行为一致；trim 后空 key 剔除
        let cfg = Config {
            base_url: "https://api.example.com".to_string(),
            extra_headers: HashMap::from([
                ("  x-a  ".to_string(), "  v1  ".to_string()),
                ("   ".to_string(), "dropped".to_string()),
            ]),
            override_headers: HashMap::from([("x-b".to_string(), "".to_string())]),
            ..Default::default()
        }
        .normalized();
        assert_eq!(cfg.extra_headers.get("x-a").map(String::as_str), Some("v1"));
        assert!(cfg.extra_headers.get("x-a") != cfg.extra_headers.get("  x-a  "));
        assert!(!cfg.extra_headers.contains_key("   "));
        assert_eq!(
            cfg.override_headers.get("x-b").map(String::as_str),
            Some("")
        );
    }

    #[test]
    fn transform_config_defaults_and_effective_values() {
        // 未配置的池/空闲/超时参数回退 DEFAULT_* 常量；0 值显式配置原样保留
        // （语义由消费方解释：idle 0=永不回收、timeout 0=不限）
        let t = TransformConfig {
            command: "aproxy-format".to_string(),
            ..Default::default()
        };
        assert_eq!(t.mode, TransformMode::Spawn);
        assert_eq!(t.effective_pool_max(), DEFAULT_TRANSFORM_POOL_MAX);
        assert_eq!(
            t.effective_idle_timeout_secs(),
            DEFAULT_TRANSFORM_IDLE_TIMEOUT_SECS
        );
        assert_eq!(t.effective_timeout_secs(), DEFAULT_TRANSFORM_TIMEOUT_SECS);
        assert_eq!(t.effective_extra(), "");

        let t = TransformConfig {
            command: "f".to_string(),
            pool_max: Some(8),
            idle_timeout_secs: Some(0),
            timeout_secs: Some(0),
            extra: Some("{\"keys\":[\"a\"]}".to_string()),
            ..Default::default()
        };
        assert_eq!(t.effective_pool_max(), 8);
        assert_eq!(t.effective_idle_timeout_secs(), 0);
        assert_eq!(t.effective_timeout_secs(), 0);
        assert_eq!(t.effective_extra(), "{\"keys\":[\"a\"]}");
    }

    #[test]
    fn normalized_blank_transform_command_becomes_none() {
        // command 纯空白 = 配置视为未设置；有值则 trim 保留
        let cfg = Config {
            base_url: "https://api.example.com".to_string(),
            request_transform: Some(TransformConfig {
                command: "  ".to_string(),
                ..Default::default()
            }),
            response_transform: Some(TransformConfig {
                command: "  aproxy-format  ".to_string(),
                ..Default::default()
            }),
            ..Default::default()
        }
        .normalized();
        assert!(cfg.request_transform.is_none());
        assert_eq!(
            cfg.response_transform.as_ref().unwrap().command,
            "aproxy-format"
        );
    }

    #[test]
    fn validate_rejects_forward_only_with_transform() {
        // 互斥：forward_only 不缓冲请求体，转换器需要全量 body——同开是配置矛盾，
        // 必须启动即报错而非静默丢弃其一
        for name in ["request", "response", "heartbeat"] {
            let t = || TransformConfig {
                command: "f".to_string(),
                ..Default::default()
            };
            let cfg = Config {
                base_url: "https://api.example.com".to_string(),
                forward_only: Some(true),
                request_transform: (name == "request").then(t),
                response_transform: (name == "response").then(t),
                heartbeat_transform: (name == "heartbeat").then(t),
                ..Default::default()
            };
            let err = cfg.validate().unwrap_err();
            assert!(
                err.contains("cannot be combined with a format"),
                "{name} 转换器 + forward_only 应报互斥: {err}"
            );
        }

        // 互不共存单开各侧均合法
        let cfg = Config {
            base_url: "https://api.example.com".to_string(),
            request_transform: Some(TransformConfig {
                command: "f".to_string(),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(cfg.validate().is_ok());
    }

    /// 每个语义问题带着码表里的码与 config.toml 的键路径；说明里的 URL 已打码。
    #[test]
    fn problems_carry_codes_and_key_paths() {
        let cfg = Config {
            base_url: "ftp://user:secret@a.example".to_string(),
            listen_addr: "127.0.0.1".to_string(),
            api_key: Some("bad\nkey".to_string()),
            proxy_username: Some("u".to_string()),
            keepalive_heartbeat: Some("data: x".to_string()),
            extra_headers: HashMap::from([
                ("bad name".to_string(), "v".to_string()),
                ("x-ok".to_string(), "bad\nvalue".to_string()),
            ]),
            retry_rules: vec![
                RetryRule {
                    path: Some("/ok".to_string()),
                    status: None,
                    max_attempts: 1,
                },
                RetryRule {
                    path: Some("(".to_string()),
                    status: None,
                    max_attempts: 1,
                },
            ],
            ..Default::default()
        };
        let problems = cfg.problems();
        let brief: Vec<String> = problems
            .iter()
            .map(|p| format!("{} {} {}", p.code, p.key, p.reason.unwrap_or("-")))
            .collect();
        assert_eq!(
            brief,
            [
                "config_invalid_value base_url not_http_url",
                "config_invalid_value listen_addr no_port",
                "config_missing_key proxy -",
                "config_invalid_value keepalive.heartbeat breaks_sse_framing",
                "config_invalid_value api_key invalid_header_value",
                "config_invalid_value extra_headers.\"bad name\" invalid_header_name",
                "config_invalid_value extra_headers.x-ok invalid_header_value",
                "config_invalid_value retry.rules[1].path invalid_regex",
            ]
        );
        assert!(
            !problems[0].message.contains("secret"),
            "{}",
            problems[0].message
        );
        for problem in &problems {
            assert!(aproxy_ipc::code::lookup(problem.code).is_some());
        }

        let ok = Config {
            base_url: "https://a.example".to_string(),
            ..Default::default()
        };
        assert_eq!(ok.problems(), []);
    }

    #[test]
    fn key_paths_parse_and_quote() {
        let seg = |name: &str, index: Option<usize>| KeySegment {
            name: name.to_string(),
            index,
        };
        assert_eq!(
            parse_key_path("base_url"),
            Some(vec![seg("base_url", None)])
        );
        assert_eq!(
            parse_key_path("retry.rules[2].path"),
            Some(vec![
                seg("retry", None),
                seg("rules", Some(2)),
                seg("path", None)
            ])
        );
        assert_eq!(
            parse_key_path(r#"extra_headers."x.y \"z\"""#),
            Some(vec![seg("extra_headers", None), seg("x.y \"z\"", None)])
        );
        for bad in [
            "", ".a", "a.", "a..b", "a b", "a[x]", "a[1", "\"open", "a\"b\"",
        ] {
            assert_eq!(parse_key_path(bad), None, "{bad:?}");
        }
        // quote_key 写出的一段，parse_key_path 读得回来
        for name in [
            "x-api-key",
            "a.b",
            "has space",
            "quo\"te",
            "back\\slash",
            "",
        ] {
            let path = format!("t.{}", quote_key(name));
            assert_eq!(
                parse_key_path(&path),
                Some(vec![seg("t", None), seg(name, None)]),
                "{path}"
            );
        }
    }

    #[test]
    fn base_url_normalization() {
        for (raw, expected) in [
            ("https://api.example.com", "https://api.example.com"),
            ("  HTTPS://API.Example.com/  ", "https://api.example.com"),
            (
                "https://api.example.com:443/v1/",
                "https://api.example.com/v1",
            ),
            ("http://LocalHost:80", "http://localhost"),
            ("http://localhost:8080//", "http://localhost:8080"),
            // userinfo 与路径的大小写原样保留
            (
                "https://User:PW@Host.example/Path",
                "https://User:PW@host.example/Path",
            ),
            ("http://[::1]:80/x", "http://[::1]/x"),
            ("http://[FE80::1]", "http://[fe80::1]"),
            // 不是 URL：只去空白与末尾斜杠
            ("not a url/", "not a url"),
            ("", ""),
        ] {
            assert_eq!(normalize_base_url(raw), expected, "{raw:?}");
        }
        let parts = split_url("socks5://u:p@Proxy.example:1080").unwrap();
        assert_eq!(
            (
                parts.scheme.as_str(),
                parts.host.as_str(),
                parts.port.as_deref()
            ),
            ("socks5", "proxy.example", Some("1080"))
        );
        assert_eq!(parts.without_userinfo(), "socks5://proxy.example:1080");
        assert_eq!(split_url("//no-scheme"), None);
        assert_eq!(split_url("http://"), None);
    }
}
