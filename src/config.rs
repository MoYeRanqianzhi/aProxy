//! 配置管理：`~/.aproxy/config.toml`，单一 upstream base URL，完整透传。
//!
//! 额外能力（兼容设计）：
//! - `api_key` 快捷：等效覆盖 `Authorization: Bearer <key>`
//! - `extra_headers`：仅当上游请求未携带该头时追加
//! - `override_headers`：无条件覆盖（用于非 Bearer 鉴权或额外头）
//! - `keepalive_interval_secs`：流式重试期间的保活心跳间隔，0 表示关闭
//! - `keepalive_trigger`：哪些请求走保活通道（看 Accept 头 / 请求体 stream:true / 任一）
//! - `proxy`：上游请求经配置的代理转发（与常见代理配置一致，支持 http/https/socks5，
//!   可在 URL 内嵌 user:pass，也可用 `proxy_username`/`proxy_password` 单独指定）；
//!   未配置时保留 reqwest 默认的系统代理（读取 HTTP_PROXY/HTTPS_PROXY/ALL_PROXY 环境变量）

use std::{collections::HashMap, path::PathBuf};

use serde::{Deserialize, Serialize};

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
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
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
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TransformConfig {
    /// format 程序命令（必填非空，validate 拦截）。不走 shell，直接按
    /// argv 数组执行——无注入面。`~` 前缀在加载时展开为用户主目录
    /// （`~/.aproxy/bin/aproxy-format` 可直接使用）；裸文件名走 PATH。
    pub command: String,
    /// format 程序参数（如官方示例的 `["run", "--config", "...toml"]`）。
    #[serde(default)]
    pub args: Vec<String>,
    /// 运行模式。默认 spawn（一次性）；聚合类 format 必须 persistent。
    #[serde(default)]
    pub mode: TransformMode,
    /// persistent 模式池上限（并发 worker 数，超限排队）。未配置取
    /// `DEFAULT_TRANSFORM_POOL_MAX`（4）；spawn 模式无意义。
    #[serde(default)]
    pub pool_max: Option<u32>,
    /// persistent 模式 worker 空闲回收秒数。0 = 永不回收。未配置取
    /// `DEFAULT_TRANSFORM_IDLE_TIMEOUT_SECS`（300）；spawn 模式无意义。
    #[serde(default)]
    pub idle_timeout_secs: Option<u64>,
    /// 单请求转换超时秒数。0 = 不限。未配置取
    /// `DEFAULT_TRANSFORM_TIMEOUT_SECS`（30）。超时的 worker 不可信
    /// （可能仍在消化旧输入），kill 后剔除。
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    /// 原样透传进信封 `extra` 字段的字符串（格式无要求，format 自解）。
    /// 官方示例用它传聚合配置文件路径。
    #[serde(default)]
    pub extra: Option<String>,
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

/// 配置文件内容
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// 上游 API 的 base URL，例如 `https://api.anthropic.com`
    /// 末尾斜杠会被自动去除以避免拼接时产生 `//`。
    /// 旧版字段名 `upstream_url` 仍可读取（alias），保存时写为新名 `base_url`。
    #[serde(default, alias = "upstream_url")]
    pub base_url: String,
    /// 本地代理监听地址，默认 `127.0.0.1:12345`（仅本地可访问，避免局域网暴露）。
    #[serde(default = "default_listen_addr")]
    pub listen_addr: String,
    /// 快捷 api_key：若设置，等效覆盖 `Authorization: Bearer <api_key>`。
    #[serde(default)]
    pub api_key: Option<String>,
    /// 额外头：仅在请求未携带该头时追加（大小写不敏感判定）。
    #[serde(default)]
    pub extra_headers: HashMap<String, String>,
    /// 覆盖头：无条件覆盖请求头（大小写不敏感匹配，写入时保留配置中的大小写）。
    #[serde(default)]
    pub override_headers: HashMap<String, String>,
    /// 流式重试保活心跳间隔（秒），0 表示关闭。默认 15 秒。
    #[serde(default = "default_keepalive_secs")]
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
    #[serde(default)]
    pub keepalive_trigger: Option<String>,
    /// 上游代理 URL，例如 `http://127.0.0.1:7890`、`socks5://user:pass@127.0.0.1:7890`。
    /// 未设置时使用系统/环境变量代理。
    #[serde(default)]
    pub proxy: Option<String>,
    /// 代理用户名（可选；优先于代理 URL 内嵌的 user:pass）。
    #[serde(default)]
    pub proxy_username: Option<String>,
    /// 代理密码（可选；优先于代理 URL 内嵌的密码）。
    #[serde(default)]
    pub proxy_password: Option<String>,
    /// 重试退避的最大等待时间（秒）。指数退避 5s→10s→20s→…增长到该值后封顶；
    /// 设为 0 表示所有重试零延迟（立即重试）。默认 320 秒。
    #[serde(default = "default_max_backoff_secs")]
    pub max_retry_backoff_secs: u64,
    /// 上游响应缓冲（spool）上限（MB）。超过即视为不可重试的确定性失败。
    /// 默认 256；小内存机器可调小，转发超大文件可调大。
    #[serde(default = "default_spool_limit_mb")]
    pub spool_limit_mb: u64,
    /// 上游连接建立超时（秒）。默认 30；慢网络/高延迟上游可调大。
    #[serde(default = "default_connect_timeout_secs")]
    pub connect_timeout_secs: u64,
    /// 上游两次读到数据之间的超时（秒，同样钳制首字节等待）。默认 300；
    /// LLM 上游排队久（TTFB 数十秒）可调大，调过小会把「慢但活着」的上游
    /// 变成确定性无限重试。
    #[serde(default = "default_read_timeout_secs")]
    pub read_timeout_secs: u64,
    /// 请求体大小上限（MB），超出直接 413。未设置时用 settings.json 的
    /// `max_body_mb`（全局默认，内置 128）；设 0 表示不设限。
    /// None = toml 未显式配置（由 main 启动时注入 settings 值）。
    #[serde(default)]
    pub max_body_mb: Option<u64>,
    /// 磁盘缓存：开启时超过内存驻留阈值（1 MiB）的请求体与响应 spool 溢写
    /// 到磁盘临时文件——进程内存与负载大小解耦（SSD 上 IO 开销为噪声级）。
    /// 未设置时用 settings.json 的 `disk_cache`（全局默认，内置 true）。
    /// 高并发大请求体实例务必开启；低并发实例可关闭保持全内存行为。
    /// None = toml 未显式配置（由 main 启动时注入 settings 值）。
    #[serde(default)]
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
    #[serde(default)]
    pub forward_only: Option<bool>,
    /// 受限重试路径（正则数组）：命中任一模式的请求，上游「有响应的失败」达到
    /// `retry::BOUNDED_RETRY_MAX_ATTEMPTS` 次尝试后不再重试，把最后一次上游
    /// 响应原样透传给客户端。**空 = 功能关闭**（一切路径照旧无限重试）。
    ///
    /// 动机：部分上游对特定端点确定性报错，无限重试只会让客户端永远等不到
    /// 终态。哪些端点属于这一类完全因上游而异——同一软件换个上游结论就翻转，
    /// 因此哪些路径受限**交由用户按自己的上游配置**，不内置任何具体 URL。
    ///
    /// 匹配语义：每个模式是对「`路径?查询串` 整体」的正则，编译时自动锚定
    /// 两端（`^(?:模式)$`）——不含元字符的普通路径即精准匹配；查询串必须
    /// 显式出现在模式里（`?` 是正则元字符，字面量写 `\?`；toml 建议用单引号
    /// 字符串免转义）；通配用 `.*` 等。非法正则在 validate() 即报错，不会
    /// 静默失效。未设置时用 settings.json 的 `bounded_retry_paths`（全局
    /// 默认，内置空）。
    #[serde(default)]
    pub bounded_retry_paths: Option<Vec<String>>,
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
    #[serde(default)]
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
    #[serde(default)]
    pub allowed_origins: Option<Vec<String>>,
    /// 请求转换器（外部 format 程序）：请求体缓冲完成后交给它改写
    /// （body/headers/url/method），重试全程重放转换后的产物。
    /// **失败语义**：转换失败（进程崩溃/超时/error 行/协议错误）→ 502 + 原因，
    /// 不重试（按约定，请求未发往上游；各类失败的性质见 `transform::TransformError`
    /// 的分类说明）。与 forward_only 互斥（后者不缓冲请求体，转换器需要
    /// 全量 body）——共存时 validate() 启动报错。仅 toml 每实例配置，无
    /// settings.json 全局默认层（设计决策：转换是场景特定功能）。
    #[serde(default)]
    pub request_transform: Option<TransformConfig>,
    /// 响应转换器（外部 format 程序）：重试判定为「成功」后、回放前交给它
    /// 改写响应 body/headers。**失败语义**：转换失败 → 透传上游原始响应 +
    /// warn（响应已在手，可用性优先）——与请求侧的 502 语义不同，各按其性质。
    /// 仅 toml 每实例配置。
    #[serde(default)]
    pub response_transform: Option<TransformConfig>,
    /// 守护日志文件路径（自定义去向）：不设时写入 `~/.aproxy/logs/` 下按启动
    /// 时刻随机命名的文件（文件名不含端口——端口是易变标识，换端口重启后
    /// 日志照样按实例连续可查，实际路径由实例经 IPC 上报，客户端不拼路径）。
    /// 支持 `~` 展开；**相对路径相对 APROXY_HOME 解析**（守护进程的工作目录
    /// 不可靠，主目录是唯一稳定基准）。CLI `--log-file`（仅本次运行）优先于
    /// 本字段。**有意不设 settings.json 全局默认层**——日志去向是单实例语义
    /// （每个实例的配置文件各管各的），与 max_body_mb 等四个全局默认字段
    /// 形成对照，这是设计决策而非遗漏。
    #[serde(default)]
    pub log_file: Option<String>,
    /// spool 临时文件目录覆盖（serde skip，不落盘）。仅测试注入用：集成测试
    /// 进程内构建 AppState 时若无此覆盖，会按端口写入真实 ~/.aproxy/spool/。
    /// 生产路径为 None，实际目录 = ~/.aproxy/spool/<端口>/。
    #[serde(skip)]
    pub spool_dir_override: Option<PathBuf>,
}

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
            bounded_retry_paths: None,
            allowed_hosts: None,
            allowed_origins: None,
            request_transform: None,
            response_transform: None,
            log_file: None,
            spool_dir_override: None,
        }
    }
}

impl Config {
    /// 归一化：去除 base_url 末尾斜杠；清理空 api_key；清理空代理配置。
    pub fn normalized(mut self) -> Self {
        self.base_url = self.base_url.trim_end_matches('/').to_string();
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
        self
    }

    /// 校验：base_url 必须非空且为 http/https URL；proxy 若配置必须为受支持的代理 URL。
    pub fn validate(&self) -> Result<(), String> {
        if self.base_url.trim().is_empty() {
            return Err("base_url 不能为空，请在 ~/.aproxy/config.toml 中配置".to_string());
        }
        self.validate_base_url()?;
        if let Some(p) = &self.proxy {
            let url = url::Url::parse(p).map_err(|e| format!("proxy 配置无效 ({p}): {e}"))?;
            let scheme = url.scheme();
            if !matches!(
                scheme,
                "http" | "https" | "socks4" | "socks4a" | "socks5" | "socks5h"
            ) {
                return Err(format!(
                    "proxy 仅支持 http/https/socks4/socks5 协议，当前值: {p}"
                ));
            }
            if url.host_str().is_none() {
                return Err(format!("proxy 缺少主机地址: {p}"));
            }
        } else if self.proxy_username.is_some() || self.proxy_password.is_some() {
            // 仅当显式配置代理 URL 时用户名/密码才有意义，否则是配置遗漏
            return Err("配置了 proxy_username/proxy_password 但未配置 proxy URL".to_string());
        }
        // 受限重试路径：正则合法性在此把关（编译入口与运行期同一，保证
        // 「校验通过 = 运行期编译必成功」），非法模式启动即报明确错误而非
        // 静默不匹配
        for p in self.bounded_retry_paths() {
            Self::compile_bounded_retry_pattern(p).map_err(|e| {
                // 原样显示模式（不用 {:?}——Debug 会把 \? 转义成 \\?，用户拿
                // 错误消息回填 toml 会得到语义不同的正则）
                format!("bounded_retry_paths 含非法正则 \"{p}\": {e}")
            })?;
        }
        // 保活触发条件：只认三种写法，其余启动即拒绝并点名字段（错误消息里
        // 带上用户写的原值，便于对照修改）
        if let Some(t) = &self.keepalive_trigger
            && KeepaliveTrigger::parse(t).is_none()
        {
            return Err(format!(
                "keepalive_trigger 取值无效 \"{t}\"：只能是 \"accept\"、\"body_stream\" 或 \"any\""
            ));
        }
        // 转换器：command 非空；persistent 模式 pool_max >= 1；与 forward_only
        // 互斥（后者不缓冲请求体，转换器需要全量 body——两者同开是配置矛盾）
        for (name, t) in [
            ("request_transform", &self.request_transform),
            ("response_transform", &self.response_transform),
        ] {
            let Some(t) = t else { continue };
            if t.command.trim().is_empty() {
                return Err(format!("{name} 的 command 不能为空"));
            }
            if t.mode == TransformMode::Persistent {
                let pm = t.pool_max.unwrap_or(DEFAULT_TRANSFORM_POOL_MAX);
                if pm == 0 {
                    return Err(format!("{name} 的 pool_max 必须 >= 1，当前值: {pm}"));
                }
            }
        }
        if self.forward_only_enabled()
            && (self.request_transform.is_some() || self.response_transform.is_some())
        {
            return Err(
                "forward_only 与外部转换器互斥：forward_only 不缓冲请求体，转换器需要全量 body（两者只能留一个）"
                    .to_string(),
            );
        }
        Ok(())
    }

    /// 校验 base_url 自身的格式（scheme、无 query/fragment）。空值是否允许由调用方
    /// 决定——启动时禁止，而 `aproxy config` 允许分多次配置的中间态，故单独拆出。
    pub fn validate_base_url(&self) -> Result<(), String> {
        // scheme 判定大小写不敏感（"HTTP://" 亦合法）
        let scheme_lower = self.base_url.trim().to_ascii_lowercase();
        if !(scheme_lower.starts_with("http://") || scheme_lower.starts_with("https://")) {
            return Err(format!(
                "base_url 必须以 http:// 或 https:// 开头，当前值: {}",
                self.base_url
            ));
        }
        // 拒绝带 query/fragment 的 base_url：拼接 path 时会把路径拼进 query，静默错路由
        if self.base_url.contains('?') || self.base_url.contains('#') {
            return Err(format!(
                "base_url 不应包含 ? 或 #（路径拼接会错路由），当前值: {}",
                self.base_url
            ));
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

    /// 受限重试路径模式列表（同 body_limit_bytes 的注入/回退语义：toml 显式
    /// 值 > settings 注入值 > 内置空 = 功能关闭）。
    /// 消费点必须走本方法而非 `Option::unwrap()`——不经 settings 注入的构建
    /// 路径（doctor/find/测试）上 `None` 是常态。
    pub fn bounded_retry_paths(&self) -> &[String] {
        self.bounded_retry_paths.as_deref().unwrap_or(&[])
    }

    /// 编译一个受限重试路径模式：对「`路径?查询串` 整体」匹配，自动锚定两端
    /// ——不含元字符的普通路径即精准匹配，通配需显式写 `.*`。validate() 与
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
    fn injects_credentials(&self) -> bool {
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
    /// startup.log 与 doctor 共用这一份文案，三处口径一致。
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

/// `~` 前缀展开：`~/x`、`~\x`、裸 `~` → 用户主目录；其余原样。
/// 展开产物统一用 `/` 分隔（Windows API 同样接受）。
pub fn expand_tilde(s: &str) -> String {
    let home = || {
        dirs::home_dir()
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
/// 见 `settings::home`）。
pub fn config_dir() -> PathBuf {
    crate::settings::home()
}

/// 加载配置：若文件不存在则返回默认配置（base_url 为空，后续 validate 会提示）。
pub fn load() -> Config {
    load_from(&config_path())
}

/// 严格加载：文件存在但 TOML 解析失败时返回 Err（含解析错误详情），
/// 其余语义同 `load_from`。启动路径必须用这个——宽松版会把解析失败静默
/// 回退成默认配置（base_url 空），启动报错于是变成误导性的「base_url 不能
/// 为空」，而用户文件里明明写了（2026-09-22 大审查实测实锤）；守护模式下
/// 真相只存在于日志文件里，主报错带偏排障方向。
pub fn load_from_strict(path: &std::path::Path) -> Result<Config, String> {
    match std::fs::read_to_string(path) {
        Ok(content) => match toml::from_str::<Config>(&content) {
            Ok(cfg) => Ok(cfg.normalized()),
            Err(e) => Err(format!(
                "配置文件解析失败（TOML 语法错误）: {e}\n文件: {}",
                path.display()
            )),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
        Err(e) => Err(format!("读取配置文件失败: {e}\n文件: {}", path.display())),
    }
}

/// URL 凭据脱敏——所有「可能被粘贴分享」的输出面（守护日志、status 的最近
/// 错误、start/config 的控制台输出、实例注册表）展示 URL 时的**唯一出口**。
/// 名称沿用最早只用于 base_url 时的叫法，现在 base_url、代理 URL、上游目标
/// URL、请求的 `路径?查询串`、reqwest 错误里的 URL 一律走它。
///
/// 规则：
/// - userinfo：用户名或密码任一存在即整体替换为 `***@`——令牌常以用户名形态
///   出现（`https://TOKEN@host`），只遮密码不够
/// - 查询串：保留键名，非空值替换为 `***`（`?key=***&beta=***`）——Gemini 式
///   客户端把 key 放在 `?key=` 里
/// - 片段：非空即整体替换为 `***`
/// - 其余部分逐字保留（不经 URL 解析器重新序列化，不会凭空多出尾斜杠等，
///   展示与用户写的一致）
///
/// 输入可以是完整 URL，也可以是以 `/` 开头的 `路径?查询串`。对解析失败的
/// 畸形 URL（如密码里含未编码的 `/` `?`）退回保守策略：最后一个 `@` 之前的
/// 内容全部视为 userinfo 遮掉——宁可多遮，也不按「看似合法」的切分把密码
/// 后半段漏出来。结果幂等（对输出再脱敏不变），下游重复调用安全。
pub fn mask_base_url(raw: &str) -> String {
    // scheme 必须是合法的 scheme 记号（字母开头，仅字母数字 + - .）——否则
    // `/v1/x?key=S&r=http://y` 这类查询串里带 `://` 的路径会被误认成 URL，
    // 整段查询串（含 key 的值）原样漏出
    let scheme_end = raw.find("://").filter(|&i| {
        let s = &raw[..i];
        s.starts_with(|c: char| c.is_ascii_alphabetic())
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    });
    let (scheme, rest) = match scheme_end {
        Some(i) => raw.split_at(i + 3),
        None => ("", raw),
    };
    // userinfo 的范围：合法 URL 的 authority 止于第一个 `/` `?` `#`（路径里的
    // `@` 合法且常见，如 Vertex 的 `models/claude-x@20250101`，不能误伤）；
    // `/` 开头的纯路径没有 authority；其余（解析失败的 URL、无 scheme 的
    // `user:pass@host`）按最后一个 `@` 保守切分
    let userinfo_end = if scheme.is_empty() && rest.starts_with('/') {
        None
    } else if !scheme.is_empty() && url::Url::parse(raw).is_ok() {
        let authority_len = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        rest[..authority_len].rfind('@')
    } else {
        rest.rfind('@')
    };
    let (userinfo_mask, rest) = match userinfo_end {
        Some(at) => ("***@", &rest[at + 1..]),
        None => ("", rest),
    };
    let (rest, fragment) = match rest.split_once('#') {
        Some((r, f)) => (r, Some(f)),
        None => (rest, None),
    };
    let (path, query) = match rest.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (rest, None),
    };
    let mut out = String::with_capacity(raw.len());
    out.push_str(scheme);
    out.push_str(userinfo_mask);
    out.push_str(path);
    if let Some(q) = query {
        out.push('?');
        let pairs: Vec<String> = q
            .split('&')
            .map(|pair| match pair.split_once('=') {
                Some((k, v)) if !v.is_empty() => format!("{k}=***"),
                _ => pair.to_string(),
            })
            .collect();
        out.push_str(&pairs.join("&"));
    }
    if let Some(f) = fragment {
        out.push('#');
        if !f.is_empty() {
            out.push_str("***");
        }
    }
    out
}

/// 加载指定路径的配置：语义同 `load`（不存在/解析失败回退默认配置）。
/// 多开场景由 `--config <PATH>` 显式指定路径；是否要求文件存在由调用方决定。
pub fn load_from(path: &std::path::Path) -> Config {
    match std::fs::read_to_string(path) {
        Ok(content) => match toml::from_str::<Config>(&content) {
            Ok(cfg) => cfg.normalized(),
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "配置文件解析失败，使用默认配置");
                Config::default()
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Config::default(),
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "读取配置文件失败，使用默认配置");
            Config::default()
        }
    }
}

/// 保存配置到默认路径（自动创建目录）。
pub fn save(cfg: &Config) -> std::io::Result<()> {
    save_to(&config_path(), cfg)
}

/// 保存配置到指定路径（自动创建目录）。多开场景配合 `--config <PATH>` 使用。
pub fn save_to(path: &std::path::Path, cfg: &Config) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = toml::to_string_pretty(cfg).expect("序列化配置失败");
    std::fs::write(path, content)
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn save_and_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut cfg = Config {
            base_url: "https://api.example.com".to_string(),
            listen_addr: "127.0.0.1:9999".to_string(),
            ..Default::default()
        };
        cfg.api_key = Some("sk-test".to_string());
        cfg.extra_headers
            .insert("x-extra".to_string(), "1".to_string());
        cfg.override_headers
            .insert("authorization".to_string(), "Bearer x".to_string());
        save_to(&path, &cfg).unwrap();
        let loaded = load_from(&path);
        assert_eq!(loaded.base_url, cfg.base_url);
        assert_eq!(loaded.listen_addr, cfg.listen_addr);
        assert_eq!(loaded.api_key, cfg.api_key);
        assert_eq!(loaded.extra_headers.get("x-extra").unwrap(), "1");
        assert_eq!(
            loaded.override_headers.get("authorization").unwrap(),
            "Bearer x"
        );
    }

    #[test]
    fn load_missing_returns_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.toml");
        let cfg = load_from(&path);
        assert!(cfg.base_url.is_empty());
        assert_eq!(cfg.listen_addr, default_listen_addr());
    }

    #[test]
    fn strict_load_reports_syntax_error_and_accepts_missing() {
        // 严格加载：语法错误必须报 Err（含详情）——启动路径依赖它把「toml
        // 写坏了」与「字段缺失」区分开；文件不存在与宽松版同为默认配置。
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broken.toml");
        std::fs::write(
            &path,
            "base_url = \"https://x.example.com\"\nbroken = {unclosed",
        )
        .unwrap();
        let err = load_from_strict(&path).unwrap_err();
        assert!(err.contains("解析失败"), "{err}");
        assert!(err.contains("unclosed"), "错误应含 TOML 解析详情: {err}");
        assert!(err.contains(&path.display().to_string()), "{err}");

        // 文件不存在：Ok(默认)——「还没写配置」不是错误（validate 再把关）
        let missing = dir.path().join("absent.toml");
        let cfg = load_from_strict(&missing).unwrap();
        assert!(cfg.base_url.is_empty());

        // 合法文件：与宽松版同结果
        std::fs::write(&path, "base_url = \"https://x.example.com\"").unwrap();
        let strict = load_from_strict(&path).unwrap();
        assert_eq!(strict.base_url, "https://x.example.com");
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
    fn tuning_fields_roundtrip() {
        // 三个调参字段写入读出 + 旧配置文件（无字段）读出默认值
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cfg.toml");
        let cfg = Config {
            base_url: "https://api.example.com".to_string(),
            spool_limit_mb: 64,
            connect_timeout_secs: 60,
            read_timeout_secs: 600,
            ..Default::default()
        };
        save_to(&path, &cfg).unwrap();
        let loaded = load_from(&path);
        assert_eq!(loaded.spool_limit_mb, 64);
        assert_eq!(loaded.connect_timeout_secs, 60);
        assert_eq!(loaded.read_timeout_secs, 600);
        std::fs::write(&path, "base_url = \"https://api.example.com\"").unwrap();
        let legacy = load_from(&path);
        assert_eq!(legacy.spool_limit_mb, 256);
        assert_eq!(legacy.connect_timeout_secs, 30);
        assert_eq!(legacy.read_timeout_secs, 300);
    }

    #[test]
    fn body_limit_and_disk_cache_override_semantics() {
        // toml 覆盖字段：显式值/0（不设限）写入读出；未配置（None）时
        // body_limit_bytes 回退内置默认 128MB，disk_cache_enabled 回退 true。
        // None 语义是「运行时由 settings 注入」，此处验证的是注入前的回退。
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cfg.toml");
        let cfg = Config {
            base_url: "https://api.example.com".to_string(),
            max_body_mb: Some(0),
            disk_cache: Some(false),
            ..Default::default()
        };
        save_to(&path, &cfg).unwrap();
        let loaded = load_from(&path);
        assert_eq!(loaded.max_body_mb, Some(0));
        assert_eq!(loaded.disk_cache, Some(false));
        assert_eq!(loaded.body_limit_bytes(), usize::MAX, "0 = 不设限");
        assert!(!loaded.disk_cache_enabled());

        // 未配置：回退内置默认（128MB / 开启）
        std::fs::write(&path, "base_url = \"https://api.example.com\"").unwrap();
        let legacy = load_from(&path);
        assert_eq!(legacy.max_body_mb, None);
        assert_eq!(
            legacy.body_limit_bytes(),
            128 * 1024 * 1024,
            "None 回退内置 128MB"
        );
        assert!(legacy.disk_cache_enabled(), "None 回退内置开启");
    }

    #[test]
    fn forward_only_override_semantics() {
        // toml 覆盖字段：Some(true)/Some(false) 落盘往返；未配置（None）时
        // forward_only_enabled 回退内置默认 false。None 语义是「运行时由
        // settings 注入」，此处验证的是注入前的回退（doctor / find / 测试
        // 直接构建 AppState 时正是这条路径，故绝不能对 None 做 unwrap）。
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cfg.toml");
        for value in [true, false] {
            let cfg = Config {
                base_url: "https://api.example.com".to_string(),
                forward_only: Some(value),
                ..Default::default()
            };
            save_to(&path, &cfg).unwrap();
            let loaded = load_from(&path);
            assert_eq!(loaded.forward_only, Some(value), "显式值应落盘往返");
            assert_eq!(loaded.forward_only_enabled(), value);
        }

        // 未配置：回退内置默认（关闭）
        std::fs::write(&path, "base_url = \"https://api.example.com\"").unwrap();
        let legacy = load_from(&path);
        assert_eq!(legacy.forward_only, None, "旧配置文件应读出 None");
        assert!(
            !legacy.forward_only_enabled(),
            "None 回退内置关闭（默认不得启用仅转发模式）"
        );
    }

    #[test]
    fn keepalive_trigger_override_and_validate() {
        // 三种合法写法落盘往返、访问器解释一致；未配置回退内置 any（Claude Code
        // 的流式请求是 Accept: application/json + stream:true，默认必须覆盖它）；
        // 非法写法 validate 点名字段拒绝
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cfg.toml");
        for (raw, want) in [
            ("accept", KeepaliveTrigger::Accept),
            ("body_stream", KeepaliveTrigger::BodyStream),
            ("any", KeepaliveTrigger::Any),
        ] {
            let cfg = Config {
                base_url: "https://api.example.com".to_string(),
                keepalive_trigger: Some(raw.to_string()),
                ..Default::default()
            };
            save_to(&path, &cfg).unwrap();
            let loaded = load_from(&path);
            assert_eq!(loaded.keepalive_trigger.as_deref(), Some(raw));
            assert_eq!(loaded.keepalive_trigger(), want);
            assert_eq!(want.as_str(), raw, "as_str 必须是 parse 的逆");
            assert!(loaded.validate().is_ok());
        }

        std::fs::write(&path, "base_url = \"https://api.example.com\"").unwrap();
        let legacy = load_from(&path);
        assert_eq!(legacy.keepalive_trigger, None, "旧配置文件应读出 None");
        assert_eq!(legacy.keepalive_trigger(), KeepaliveTrigger::Any);

        for bad in ["Any", "stream", "", "accept "] {
            let cfg = Config {
                base_url: "https://api.example.com".to_string(),
                keepalive_trigger: Some(bad.to_string()),
                ..Default::default()
            };
            let err = cfg.validate().unwrap_err();
            assert!(
                err.contains("keepalive_trigger") && err.contains(&format!("\"{bad}\"")),
                "错误应点名字段并带原值: {err}"
            );
        }
    }

    #[test]
    fn bounded_retry_paths_matching_semantics() {
        // 匹配语义钉死：模式对「路径?查询串」整体生效且自动锚定——
        // 普通路径即精准匹配（不含查询串时不命中带查询串的请求），
        // 通配必须显式写正则。
        let m = |pattern: &str, target: &str| {
            Config::compile_bounded_retry_pattern(pattern)
                .unwrap()
                .is_match(target)
        };
        // 精准匹配：路径本身命中，路径不带查询串的模式不命中带查询串的请求
        assert!(m("/v1/messages/count_tokens", "/v1/messages/count_tokens"));
        assert!(!m(
            "/v1/messages/count_tokens",
            "/v1/messages/count_tokens?beta=true"
        ));
        assert!(!m(
            "/v1/messages/count_tokens",
            "/v1/messages/count_tokens_extra"
        ));
        assert!(!m("/v1/messages/count_tokens", "/v1/messages"));
        // 查询串必须显式出现在模式中（`?` 是正则元字符，字面量需转义 `\?`）
        assert!(m(
            "/v1/messages/count_tokens\\?.*",
            "/v1/messages/count_tokens?beta=true"
        ));
        assert!(!m(
            "/v1/messages/count_tokens\\?.*",
            "/v1/messages/count_tokens"
        ));
        // 通配：`.*` 覆盖任意后缀；前缀式模式可同时命中带/不带查询串的请求
        assert!(m(
            "/v1/messages/count_tokens.*",
            "/v1/messages/count_tokens"
        ));
        assert!(m(
            "/v1/messages/count_tokens.*",
            "/v1/messages/count_tokens?beta=true"
        ));
        assert!(!m("/v1/messages/count_tokens.*", "/v1/messages"));
        // 自动锚定：模式不会作为子串命中其他路径
        assert!(!m("/messages", "/v1/messages/count_tokens"));
        // 顶层交替依赖包裹的非捕获组 (?:...)：锚定必须罩住整个交替，否则
        // ^/a|/b$ 的右支会作为子串命中任意以 /b 结尾的路径（误封顶）
        assert!(m("/a|/b", "/a"));
        assert!(m("/a|/b", "/b"));
        assert!(!m("/a|/b", "/x/y/b"));
        // 用户自带的 ^ $ 锚点在包裹内仍合法且语义不变
        assert!(m("^/x$", "/x"));
        assert!(!m("^/x$", "/x/y"));
    }

    #[test]
    fn bounded_retry_paths_accessor_and_validate() {
        // 访问器注入/回退语义与 validate 的非法正则拒绝
        let mut cfg = Config {
            base_url: "https://api.example.com".to_string(),
            ..Default::default()
        };
        assert!(
            cfg.bounded_retry_paths().is_empty(),
            "None 回退空 = 功能关闭"
        );

        cfg.bounded_retry_paths = Some(vec!["/v1/messages/count_tokens".to_string()]);
        assert_eq!(cfg.bounded_retry_paths().len(), 1);
        cfg.validate().expect("合法正则应通过校验");

        let bad = Config {
            base_url: "https://api.example.com".to_string(),
            bounded_retry_paths: Some(vec!["[unclosed".to_string()]),
            ..Default::default()
        };
        let err = bad.validate().unwrap_err();
        assert!(err.contains("bounded_retry_paths"), "错误应指明字段: {err}");
        assert!(err.contains("[unclosed"), "错误应包含非法模式原文: {err}");
    }

    #[test]
    fn bounded_retry_paths_toml_roundtrip() {
        // toml 往返：该字段唯一入口就是手编文件（无 CLI 旗标），单引号字面量
        // 写法（正则含 \? 时免转义）必须保真；旧配置（无该字段）读出 None 且
        // 访问器回退空。
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cfg.toml");
        let raw = concat!(
            "base_url = \"https://api.example.com\"\n",
            "bounded_retry_paths = ['/v1/messages/count_tokens', '/v1/messages/count_tokens\\?.*']\n",
        );
        std::fs::write(&path, raw).unwrap();
        let loaded = load_from(&path);
        assert_eq!(
            loaded.bounded_retry_paths(),
            [
                "/v1/messages/count_tokens".to_string(),
                "/v1/messages/count_tokens\\?.*".to_string()
            ],
            "单引号 toml 字面量里的 \\? 必须原样保真（不能被 TOML 转义吃掉）"
        );
        // 往返后模式仍可编译且语义不变
        let re = Config::compile_bounded_retry_pattern(&loaded.bounded_retry_paths()[1]).unwrap();
        assert!(re.is_match("/v1/messages/count_tokens?beta=true"));

        // 旧配置（无该字段）：None + 访问器回退空
        std::fs::write(&path, "base_url = \"https://api.example.com\"").unwrap();
        let legacy = load_from(&path);
        assert_eq!(legacy.bounded_retry_paths, None);
        assert!(legacy.bounded_retry_paths().is_empty());
    }

    // ---- mask_base_url：URL 凭据脱敏的统一出口 ----

    #[test]
    fn mask_url_masks_any_userinfo() {
        // 无凭据：逐字原样（不经解析器重新序列化，不凭空多出尾斜杠）
        assert_eq!(
            mask_base_url("https://api.example.com"),
            "https://api.example.com"
        );
        assert_eq!(
            mask_base_url("http://127.0.0.1:7890"),
            "http://127.0.0.1:7890"
        );
        assert_eq!(mask_base_url(""), "");
        // 用户名 + 密码
        assert_eq!(
            mask_base_url("http://alice:secret@127.0.0.1:7890"),
            "http://***@127.0.0.1:7890"
        );
        // 仅用户名也打码：令牌常以用户名形态出现（https://TOKEN@host）
        assert_eq!(
            mask_base_url("https://TOKENUSER@api.example.com/v1"),
            "https://***@api.example.com/v1"
        );
        assert_eq!(
            mask_base_url("socks5://alice@127.0.0.1:1080"),
            "socks5://***@127.0.0.1:1080"
        );
    }

    #[test]
    fn mask_url_keeps_query_keys_and_masks_values() {
        assert_eq!(
            mask_base_url("http://alice:pw@h:1?p=x"),
            "http://***@h:1?p=***"
        );
        assert_eq!(
            mask_base_url("http://127.0.0.1:9/v1/models?key=QKEY999&a=1&flag&empty="),
            "http://127.0.0.1:9/v1/models?key=***&a=***&flag&empty="
        );
        // 值里含 `=`：只按第一个 `=` 切分，键名完整保留
        assert_eq!(mask_base_url("/x?sig=a=b=c"), "/x?sig=***");
        // 纯路径（日志里的 path 字段）
        assert_eq!(
            mask_base_url("/v1/messages?beta=true"),
            "/v1/messages?beta=***"
        );
        assert_eq!(mask_base_url("/v1/messages"), "/v1/messages");
        // 片段整体遮掉
        assert_eq!(mask_base_url("https://h/p#tok=abc"), "https://h/p#***");
    }

    #[test]
    fn mask_url_does_not_mangle_at_sign_in_path() {
        // 路径里的 `@` 合法（Vertex 的模型 ID 形如 claude-x@20250101）：不得被
        // 当成 userinfo 分隔符把主机名遮掉
        let vertex = "https://us-east5-aiplatform.googleapis.com/v1/projects/p/models/claude-3@20240229:streamRawPredict";
        assert_eq!(mask_base_url(vertex), vertex);
        assert_eq!(
            mask_base_url("/v1/models/claude@2024?key=K"),
            "/v1/models/claude@2024?key=***"
        );
    }

    #[test]
    fn mask_url_query_containing_scheme_is_still_masked() {
        // 回归：查询串里出现 `://` 时不得被误认成 URL 的 scheme 分隔，否则整段
        // 查询串（含 key 的值）原样漏出
        assert_eq!(
            mask_base_url("/v1/x?key=SECRET&r=http://y"),
            "/v1/x?key=***&r=***"
        );
    }

    #[test]
    fn mask_url_malformed_inputs_fail_safe() {
        // 畸形 URL（密码含未编码的 `/` `?`，解析失败）：最后一个 `@` 之前全部遮掉，
        // 绝不按「看似合法」的切分漏出密码后半段
        let out = mask_base_url("http://user:pa/ss@host:7890");
        assert_eq!(out, "http://***@host:7890");
        let out = mask_base_url("http://user:pa?ss@host");
        assert!(!out.contains("pa") && !out.contains("ss@"), "{out}");
        // 缺 scheme 的 user:pass@host（validate 会拒，但错误消息与 --show 仍要展示）
        assert_eq!(
            mask_base_url("user:pass@127.0.0.1:7890"),
            "***@127.0.0.1:7890"
        );
        // scheme 打错（http//）同样不漏
        let out = mask_base_url("http//user:pass@host");
        assert!(!out.contains("pass"), "{out}");
        // 无凭据的非 URL 原样
        assert_eq!(mask_base_url("not a url"), "not a url");
    }

    #[test]
    fn mask_url_is_idempotent() {
        // 注册表里已是脱敏值，status 展示时会再脱敏一次：结果必须不变
        for raw in [
            "http://alice:secret@127.0.0.1:7890/v1?key=K&a=1#frag",
            "https://TOKEN@api.example.com",
            "/v1/x?key=SECRET",
            "user:pass@host",
        ] {
            let once = mask_base_url(raw);
            assert_eq!(mask_base_url(&once), once, "幂等失败: {raw}");
        }
    }

    // ---- 监听地址回环判定与非回环告警 ----

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
    fn allowed_hosts_and_origins_roundtrip_and_default() {
        // 两个入站白名单：toml 往返保真；旧配置（无字段）读出 None、访问器回退空
        // （= 内置默认策略）
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cfg.toml");
        let raw = concat!(
            "base_url = \"https://api.example.com\"\n",
            "allowed_hosts = [\"myproxy.local\", \"*\"]\n",
            "allowed_origins = [\"http://localhost:5173\"]\n",
        );
        std::fs::write(&path, raw).unwrap();
        let loaded = load_from(&path);
        assert_eq!(loaded.allowed_hosts(), ["myproxy.local", "*"]);
        assert_eq!(loaded.allowed_origins(), ["http://localhost:5173"]);

        std::fs::write(&path, "base_url = \"https://api.example.com\"").unwrap();
        let legacy = load_from(&path);
        assert_eq!(legacy.allowed_hosts, None);
        assert_eq!(legacy.allowed_origins, None);
        assert!(legacy.allowed_hosts().is_empty() && legacy.allowed_origins().is_empty());
    }

    #[test]
    fn log_file_roundtrip_and_default() {
        // log_file：显式值落盘往返；旧配置文件（无该字段）读出 None
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cfg.toml");
        let cfg = Config {
            base_url: "https://api.example.com".to_string(),
            log_file: Some("/var/log/aproxy/inst-a.log".to_string()),
            ..Default::default()
        };
        save_to(&path, &cfg).unwrap();
        let loaded = load_from(&path);
        assert_eq!(
            loaded.log_file.as_deref(),
            Some("/var/log/aproxy/inst-a.log"),
            "显式值应落盘往返"
        );

        // 旧配置（无该字段）：None = 内置随机命名
        std::fs::write(&path, "base_url = \"https://api.example.com\"").unwrap();
        let legacy = load_from(&path);
        assert_eq!(legacy.log_file, None, "旧配置文件应读出 None");
    }

    #[test]
    fn max_backoff_roundtrip_and_zero() {
        // 自定义封顶写入读出；0（所有重试零延迟）也必须能落盘往返
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cfg.toml");
        let mut cfg = Config {
            base_url: "https://api.example.com".to_string(),
            ..Default::default()
        };
        cfg.max_retry_backoff_secs = 60;
        save_to(&path, &cfg).unwrap();
        assert_eq!(load_from(&path).max_retry_backoff_secs, 60);
        cfg.max_retry_backoff_secs = 0;
        save_to(&path, &cfg).unwrap();
        assert_eq!(load_from(&path).max_retry_backoff_secs, 0);
        // 旧版配置文件（无该字段）读出默认 320
        std::fs::write(&path, "base_url = \"https://api.example.com\"").unwrap();
        assert_eq!(load_from(&path).max_retry_backoff_secs, 320);
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
    fn legacy_upstream_url_field_still_loads() {
        // 旧版配置文件字段名 upstream_url 应经 alias 正常读取，保存时写为新名 base_url
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "upstream_url = \"https://legacy.example.com\"\nlisten_addr = \"127.0.0.1:12345\"\n",
        )
        .unwrap();
        let loaded = load_from(&path);
        assert_eq!(loaded.base_url, "https://legacy.example.com");

        // roundtrip 后应写为新字段名
        save_to(&path, &loaded).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("base_url"), "保存后应写新字段名 base_url");
        assert!(!content.contains("upstream_url"), "保存后不应再写旧字段名");
    }

    #[test]
    fn proxy_survives_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let cfg = Config {
            base_url: "https://api.example.com".to_string(),
            proxy: Some("socks5://127.0.0.1:1080".to_string()),
            proxy_username: Some("alice".to_string()),
            proxy_password: Some("secret".to_string()),
            ..Default::default()
        };
        save_to(&path, &cfg).unwrap();
        let loaded = load_from(&path);
        assert_eq!(loaded.proxy, cfg.proxy);
        assert_eq!(loaded.proxy_username, cfg.proxy_username);
        assert_eq!(loaded.proxy_password, cfg.proxy_password);
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
    fn transform_config_toml_roundtrip_and_legacy_default() {
        // toml 往返：mode 字符串、args、extra 均保真；旧配置（无字段）读出 None
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cfg.toml");
        let raw = concat!(
            "base_url = \"https://api.example.com\"\n",
            "[request_transform]\n",
            "command = \"aproxy-format\"\n",
            "args = [\"run\", \"--config\", \"agg.toml\"]\n",
            "mode = \"persistent\"\n",
            "pool_max = 8\n",
            "extra = \"{\\\"keys\\\":[\\\"k1\\\"]}\"\n",
        );
        std::fs::write(&path, raw).unwrap();
        let loaded = load_from(&path);
        let rt = loaded
            .request_transform
            .as_ref()
            .expect("应读出 request_transform");
        assert_eq!(rt.command, "aproxy-format");
        assert_eq!(
            rt.args,
            vec![
                "run".to_string(),
                "--config".to_string(),
                "agg.toml".to_string()
            ]
        );
        assert_eq!(rt.mode, TransformMode::Persistent);
        assert_eq!(rt.effective_pool_max(), 8);
        assert!(rt.extra.as_deref().unwrap_or("").contains("k1"));
        assert!(
            loaded.response_transform.is_none(),
            "未配置的 response_transform 应为 None"
        );

        // 旧配置文件（无两字段）：读出 None（升级安全）
        std::fs::write(&path, "base_url = \"https://api.example.com\"").unwrap();
        let legacy = load_from(&path);
        assert!(legacy.request_transform.is_none());
        assert!(legacy.response_transform.is_none());
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
    fn validate_rejects_transform_without_command() {
        let cfg = Config {
            base_url: "https://api.example.com".to_string(),
            request_transform: Some(TransformConfig {
                command: String::new(),
                ..Default::default()
            }),
            ..Default::default()
        };
        let err = cfg.validate().unwrap_err();
        assert!(err.contains("request_transform"), "错误应指明字段: {err}");
        assert!(err.contains("command"), "{err}");
    }

    #[test]
    fn validate_rejects_persistent_pool_max_zero() {
        let cfg = Config {
            base_url: "https://api.example.com".to_string(),
            request_transform: Some(TransformConfig {
                command: "f".to_string(),
                mode: TransformMode::Persistent,
                pool_max: Some(0),
                ..Default::default()
            }),
            ..Default::default()
        };
        let err = cfg.validate().unwrap_err();
        assert!(err.contains("pool_max"), "{err}");
    }

    #[test]
    fn validate_rejects_forward_only_with_transform() {
        // 互斥：forward_only 不缓冲请求体，转换器需要全量 body——同开是配置矛盾，
        // 必须启动即报错而非静默丢弃其一
        for name in ["request", "response"] {
            let cfg = Config {
                base_url: "https://api.example.com".to_string(),
                forward_only: Some(true),
                request_transform: (name == "request").then(|| TransformConfig {
                    command: "f".to_string(),
                    ..Default::default()
                }),
                response_transform: (name == "response").then(|| TransformConfig {
                    command: "f".to_string(),
                    ..Default::default()
                }),
                ..Default::default()
            };
            let err = cfg.validate().unwrap_err();
            assert!(
                err.contains("互斥"),
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
}
