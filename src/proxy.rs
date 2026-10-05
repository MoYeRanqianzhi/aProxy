//! 代理核心：将本地请求完整透传到上游，并在失败时无限重试。
//!
//! 设计要点：
//! - 单一 upstream base URL，其余路径与查询参数完整透传（包含 `/health` 等，上游若有同名路径亦完整透传）
//! - 网络错误 / 4xx / 5xx / 错误 JSON 内容 → 全部无限重试，梯度延迟（`retry` 模块）
//!   原型阶段 4xx 同样视为易发瞬时错误（如限流、临时鉴权波动、上游误报）
//! - 非流式与流式统一缓冲策略（内存 + 磁盘双模，`disk_cache` 配置）：
//!   1. 对上游响应先完整缓冲（spool），期间任何网络中断均视为 `NetworkError`
//!      触发重试——满足“流式中断可重试”的需求。缓冲超过内存驻留阈值（1 MiB）
//!      时溢写到磁盘临时文件（`disk_cache` 开启时）：进程内存与负载大小解耦，
//!      page cache 提供读写弹性（SSD 上 IO 开销为噪声级）；关闭时全程纯内存
//!      （与旧行为一致）。
//!   2. 缓冲完成后，再做“是否可重试”的判定：
//!      - HTTP 状态码可重试（4xx / 5xx，`is_retryable_status`）
//!      - 非流式：`is_error_body` 命中（任意状态码下只要 body 语义为报错就重试）
//!      - 流式：`is_stream_error_body` 命中（扫描 SSE data 行 / NDJSON）。
//!        磁盘模式用增量行扫描（`StreamErrorScanner`，语义与行级判定一致）；
//!        「整体 body 单 JSON 兜底」仅在内存模式执行——错误 JSON 均为 KB 级，
//!        不会溢写到磁盘。
//!   3. 仅当整轮缓冲成功且判定为不可重试时，才将缓冲体回放给客户端；
//!      流式判定（或磁盘模式）以 chunked 流式分块原样回放，逐块产出保持与
//!      上游字节完全一致，SSE 解析器语义不受影响。
//! - 请求体同样双模：小请求体驻留内存（Bytes 零拷贝重试共享），大请求体
//!   溢写磁盘、每次重试重新流式读取。上限 `max_body_mb`（默认 128，0=不限）。
//! - 首轮快速路径：attempt 1 的结果先做判定，成功则直接原样回放（status 与全部
//!   响应头保真）；仅当需要重试时才进入重试通道——首轮成功是常态路径，保真优先。
//! - 保活通道（`proxy_with_keepalive`）：「保活适用」的请求（`keepalive_trigger`
//!   判定命中：Accept 含 text/event-stream 和/或客户端原始请求体顶层
//!   `"stream": true`；且 keepalive 开启、非 forward_only）从首轮起就由后台任务
//!   驱动。响应的「提交点」是一个三态机——未提交 → 提交上游真实头 / 提交骨架头：
//!   - 上游回 2xx + 未压缩的 text/event-stream 头 → 立即把上游真实 status 与
//!     响应头转给客户端（未配置 response_transform 时）
//!   - 一个 keepalive 间隔内仍无可提交的结果，或需要重试 → 提交骨架头
//!     （200 + text/event-stream）
//!   - 未提交前首轮就成功完成 → 走与上面相同的保真快速路径
//!
//!   提交之后，无论在等上游响应头、缓冲上游流还是退避，都每
//!   `keepalive_interval_secs` 发一行 SSE 注释（`: keepalive\n\n`）；完整缓冲、
//!   判定无误后才把成功那一次的原样字节写进同一个响应，期间的重试客户端只见到
//!   心跳。客户端断开（任一阶段）即中止在途上游请求、不再发起新请求。
//!   不适用保活的请求（如 stream:false 的普通 JSON 请求）沿用首轮快速路径 +
//!   `proxy_without_keepalive`：成功后一次性回放，期间不向客户端写任何字节。
//! - 入站来源校验（handler 第一步）：Host 不在 `allowed_hosts`、或带了不在
//!   `allowed_origins` 里的 Origin 的请求本地 403——不转发、不注入 api_key、
//!   不进重试循环（防 DNS 重绑定与网页跨站调用；语义见 `Config` 同名字段）
//! - 头处理：`api_key` 快捷覆盖 `Authorization: Bearer` 与 `x-api-key`（Anthropic 风格），
//!   `extra_headers` 追加缺失头，`override_headers` 无条件覆盖（兼容非 Bearer 鉴权与额外头需求）
//! - 磁盘临时文件生命周期：请求体随请求结束（Drop）删除；响应 spool 在回放流
//!   结束/客户端断开/重试丢弃时删除；进程崩溃残留由启动时 `clean_spool_dir`
//!   回收（每实例独立子目录，互不干扰）。

use axum::{
    Router,
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use bytes::Bytes;
use futures_util::{Stream, StreamExt as _};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, atomic::Ordering as AtomicOrdering},
    task::{Poll, ready},
    time::Duration,
};
use tokio::io::{AsyncRead as _, AsyncReadExt as _, AsyncWriteExt as _};
use tokio_util::io::ReaderStream;

use crate::{
    config::Config,
    retry,
    transform::{ExchangeCtx, HeartbeatSource},
};

/// 需要过滤的 hop-by-hop 头，避免透传导致协议错误或与 hyper/reqwest 的
/// 自动管理（content-length / transfer-encoding / host / connection）冲突。
pub(crate) const HOP_HEADERS: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "host",
    "content-length",
];

pub(crate) fn is_hop_header(name: &str) -> bool {
    HOP_HEADERS.contains(&name.to_ascii_lowercase().as_str())
}

/// 响应侧的 hop-by-hop 过滤：同 `is_hop_header`，但**在上游未用
/// transfer-encoding 分帧时排除 content-length**（即把它保留下来）。
///
/// 保留的理由与磁盘回放分支显式回填 content-length 完全一致：仅转发模式转发的
/// 字节未经任何变换（不解码、不重写、不拼接），上游声明的长度仍然精确——剥掉它
/// 只会让本可定长的响应退化为 chunked，白白丢掉客户端的长度可见性。
/// 请求侧不能照此办理：那侧的 content-length 描述的是客户端发来的字节，
/// 而 hyper/reqwest 会按实际发送的 body 自行管理，必须交由它们决定。
///
/// `upstream_has_te` 必须取自**上游原始响应头**是否含 transfer-encoding。
/// 「上游声明的长度仍然精确」这条前提在 CL 与 TE 并存时不成立（协议违规但现实
/// 中存在）：hyper 客户端会保留 CL 头、改按 TE 分帧，于是我们手里的 CL 只是
/// 上游的一句声明，与实际响应字节数可能不符。后果最坏的那种是**静默**的——
/// CL 声称的字节数大于实际时，hyper 服务端按 CL 分帧、把缺额一直等下去，
/// 客户端拿不到响应却毫无线索（RFC 7230 §3.3.3 因此要求：带 TE 的消息转发前
/// 必须移除收到的 content-length，交由接收方重新分帧）。
fn is_hop_response_header(name: &str, upstream_has_te: bool) -> bool {
    if name.eq_ignore_ascii_case("content-length") {
        return upstream_has_te;
    }
    is_hop_header(name)
}

/// 共享状态
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub client: reqwest::Client,
    /// 最近一次收到客户端请求的时刻（Unix 秒）。AtomicU64 供 IPC ping 读取
    /// 展示/判闲置，请求热路径上仅一次 store（Relaxed 足够：只用于粗粒度的
    /// 空闲判定，无需跨线程因果序）。
    pub last_activity_secs: Arc<std::sync::atomic::AtomicU64>,
    /// IPC 实时观测源（请求数/重试数/最近错误）：热路径各一条 Relaxed 原子
    /// 操作，量级同上；serve_forever 组装进 IpcStats 供 IPC 响应读取。
    pub stats: Arc<crate::daemon::IpcStats>,
    /// 磁盘缓存的 spool 临时目录；None = disk_cache 关闭（全程纯内存）。
    /// 生产路径 `~/.aproxy/spool/<端口>/`，测试可经 Config::spool_dir_override 注入。
    pub spool_dir: Option<PathBuf>,
    /// 受限重试路径的预编译正则（源 `config.bounded_retry_paths`，空 = 功能
    /// 关闭）。启动时编译一次随 AppState 共享，请求热路径只做 is_match。
    pub bounded_retry_patterns: Arc<Vec<regex::Regex>>,
    /// 外部转换器进程池（request/response 各一，源 `config.request_transform`
    /// / `response_transform`）；None = 未配置，热路径零开销。pub(crate)：池是
    /// 内部编排细节（TransformPool 的接口不对外），AppState 虽 pub 但不泄漏它。
    pub(crate) request_pool: Option<Arc<crate::transform::TransformPool>>,
    pub(crate) response_pool: Option<Arc<crate::transform::TransformPool>>,
    /// 心跳转换器进程池（`heartbeat_transform`）；None = 只发固定心跳
    pub(crate) heartbeat_pool: Option<Arc<crate::transform::TransformPool>>,
    /// 入站来源校验策略（源 `config.allowed_hosts` / `allowed_origins` 与监听
    /// 地址）：启动时归一一次，热路径只做小集合比较。
    inbound: Arc<InboundPolicy>,
}

/// 入站来源校验策略：Host 白名单（防 DNS 重绑定）与 Origin 白名单（防网页
/// 跨站调用）。语义的权威说明见 `Config::allowed_hosts` / `allowed_origins`。
///
/// 被拒的请求在 handler 第一步就以 403 本地返回：**绝不转发、绝不注入
/// api_key、绝不进入重试循环**。这是唯一不经上游就终结请求的入口，而它只作用
/// 于从未转发过的请求，「无限重试」对放行的请求毫无改变。
struct InboundPolicy {
    /// None = 不做 Host 校验；Some = 只放行这些主机名（已归一：去端口、小写、
    /// IPv6 带方括号）
    hosts: Option<Vec<String>>,
    /// None = 不做 Origin 校验；Some = 只放行这些 Origin（已归一：去末尾 `/`、
    /// 小写）。Some(空) = 拒绝一切携带 Origin 的请求（内置默认）
    origins: Option<Vec<String>>,
}

/// 被拒请求的拒绝原因（决定 403 文案与日志）
enum InboundRejection {
    /// Host 不在白名单（值为请求的 Host，已截断）
    Host(String),
    /// Origin 不在白名单（值为请求的 Origin，已截断）
    Origin(String),
}

/// 回环名单：Host 校验生效时恒放行。这三者只能指向本机，攻击者的域名无论
/// 怎样重绑定都不会以它们出现在 Host 头里。
const LOOPBACK_HOSTS: [&str; 3] = ["localhost", "127.0.0.1", "[::1]"];

/// 主机名归一：去端口、小写；IPv6 统一为带方括号形态（Host 头里必然带括号，
/// 配置条目可能写成裸 `::1`）。Host 头与配置条目走同一函数，比较才对称。
/// 配置条目误写成 URL（`http://myhost:8080/`）时取其主机部分——Host 头里
/// 不会出现 `://`，这一步只对配置生效。
fn normalize_host(raw: &str) -> String {
    let mut s = raw.trim().to_ascii_lowercase();
    if let Some((_, rest)) = s.split_once("://") {
        s = rest.split('/').next().unwrap_or("").to_string();
    }
    if let Some(rest) = s.strip_prefix('[') {
        return match rest.find(']') {
            Some(end) => format!("[{}]", &rest[..end]),
            None => s,
        };
    }
    if s.matches(':').count() >= 2 {
        return format!("[{s}]");
    }
    match s.split_once(':') {
        Some((host, _port)) => host.to_string(),
        None => s,
    }
}

/// Origin 归一：去首尾空白与末尾 `/`、小写（浏览器发出的 Origin 不带末尾
/// `/`，用户照抄地址栏时常带上）
fn normalize_origin(raw: &str) -> String {
    raw.trim().trim_end_matches('/').to_ascii_lowercase()
}

/// 日志/文案里回显攻击者可控的头值：截断到固定长度，防止超长值撑爆日志行。
/// 调用方只传 `HeaderValue::to_str` 成功的值或 URI authority——两者都只含
/// 可见 ASCII（无控制字符注入面），按字节截断也不会切断多字节字符。
fn truncate_for_display(s: &str) -> String {
    const MAX: usize = 128;
    if s.len() <= MAX {
        s.to_string()
    } else {
        format!("{}…", &s[..MAX])
    }
}

impl InboundPolicy {
    fn from_config(config: &Config) -> Self {
        let wildcard = |list: &[String]| list.iter().any(|e| e.trim() == "*");
        let host_entries = config.allowed_hosts();
        let loopback = config.listen_is_loopback();
        let hosts = if wildcard(host_entries) || (!loopback && host_entries.is_empty()) {
            None
        } else {
            let mut set: Vec<String> = LOOPBACK_HOSTS.iter().map(|h| h.to_string()).collect();
            // 监听地址自身的主机部分：监听 127.0.0.2 / 局域网主机名时，客户端用的
            // 正是它。通配地址（0.0.0.0、[::]）不是任何客户端会写进 Host 的名字
            let listen_host = normalize_host(&config.listen_addr);
            if !listen_host.is_empty() && listen_host != "0.0.0.0" && listen_host != "[::]" {
                set.push(listen_host);
            }
            set.extend(
                host_entries
                    .iter()
                    .map(|h| normalize_host(h))
                    .filter(|h| !h.is_empty()),
            );
            Some(set)
        };
        let origin_entries = config.allowed_origins();
        let origins = if wildcard(origin_entries) {
            None
        } else {
            Some(origin_entries.iter().map(|o| normalize_origin(o)).collect())
        };
        Self { hosts, origins }
    }

    /// 校验一个入站请求；放行返回 None。
    ///
    /// Host 取 Host 头（多值时逐个校验，任一不在白名单即拒）；没有 Host 头时
    /// 退回请求 URI 的 authority（HTTP/2 的 `:authority`）；两者都没有则放行——
    /// 浏览器发出的每个 HTTP/1.1 请求都带 Host（明文 HTTP 下浏览器不用 h2c，
    /// 不存在只有 `:authority` 的浏览器请求），无 Host 的请求只可能来自自己
    /// 拼报文的本机程序，DNS 重绑定这一威胁与它无关。非 ASCII 的 Host 一律拒绝。
    fn check(&self, headers: &HeaderMap, uri: &http::Uri) -> Option<InboundRejection> {
        if let Some(allowed) = &self.hosts {
            let mut values: Vec<&str> = Vec::new();
            for v in headers.get_all(http::header::HOST) {
                match v.to_str() {
                    Ok(s) => values.push(s),
                    Err(_) => return Some(InboundRejection::Host("<非 ASCII>".to_string())),
                }
            }
            let authority = uri.authority().map(|a| a.as_str());
            if values.is_empty()
                && let Some(a) = authority
            {
                values.push(a);
            }
            for v in values {
                if !allowed.contains(&normalize_host(v)) {
                    return Some(InboundRejection::Host(truncate_for_display(v)));
                }
            }
        }
        if let Some(allowed) = &self.origins {
            for v in headers.get_all(http::header::ORIGIN) {
                let Ok(s) = v.to_str() else {
                    return Some(InboundRejection::Origin("<非 ASCII>".to_string()));
                };
                if !allowed.contains(&normalize_origin(s)) {
                    return Some(InboundRejection::Origin(truncate_for_display(s)));
                }
            }
        }
        None
    }
}

/// 入站拒绝的 403 响应：文案点名对应配置项与修复方法（用户只看得到这一句，
/// 必须能照着改好）。同时 warn 留痕——路径经脱敏（查询串可能带 key）。
fn inbound_rejection_response(
    rejection: InboundRejection,
    method: &http::Method,
    uri: &http::Uri,
) -> Response {
    let path = crate::config::mask_base_url(uri.path_and_query().map_or("/", |pq| pq.as_str()));
    let body = match &rejection {
        InboundRejection::Host(host) => {
            tracing::warn!(method = %method, path = %path, host = %host, "入站请求被拒绝：Host 不在 allowed_hosts 白名单（未转发上游）");
            format!(
                "aProxy 拒绝了该请求（403，未转发上游）：Host「{host}」不在允许列表内。\n\
                 为防 DNS 重绑定，监听回环地址时默认只放行 localhost / 127.0.0.1 / [::1]。\n\
                 若这是你信任的客户端，请把该主机名（不含端口）加入 config.toml 的 allowed_hosts\n\
                 （或 settings.json 的 allowed_hosts 全局默认），例如 allowed_hosts = [\"主机名\"]；\n\
                 写 allowed_hosts = [\"*\"] 可关闭 Host 校验。\n"
            )
        }
        InboundRejection::Origin(origin) => {
            tracing::warn!(method = %method, path = %path, origin = %origin, "入站请求被拒绝：Origin 不在 allowed_origins 白名单（未转发上游）");
            format!(
                "aProxy 拒绝了该请求（403，未转发上游）：来自浏览器页面的请求（Origin「{origin}」）默认不放行，\n\
                 以防网页借本机代理调用上游、花费你的额度。\n\
                 若这是你信任的浏览器/Electron 客户端，请把该 Origin 原样加入 config.toml 的 allowed_origins\n\
                 （或 settings.json 的 allowed_origins 全局默认），例如 allowed_origins = [\"{origin}\"]；\n\
                 写 allowed_origins = [\"*\"] 可关闭 Origin 校验。\n"
            )
        }
    };
    (StatusCode::FORBIDDEN, body).into_response()
}

impl AppState {
    pub fn new(config: Config) -> Self {
        // 超时策略：不设总时限（会掐断超过时限的慢流式生成，导致无限重试永不成功），
        // 只限制连接建立与两次读到数据之间的间隔（均可经 config.toml 调整）。
        // 注意 read_timeout 同样钳制首字节等待——LLM 上游排队时 TTFB 可达数十秒，
        // 阈值过小（如 60s）会把「慢但活着」的上游变成确定性无限重试。spool 设计
        // 本身容忍慢流。0 表示该项不设限。
        let mut builder = reqwest::Client::builder()
            .pool_idle_timeout(Duration::from_secs(90))
            .tcp_keepalive(Duration::from_secs(30))
            // 透明代理不跟随重定向：跟随会把 Authorization/api_key 与请求体外带到
            // 3xx 指向的任意主机，且 3xx 永远到不了客户端；禁用后 3xx 作为普通
            // 成功响应原样回放（is_retryable_status 本就排除 3xx）。
            .redirect(reqwest::redirect::Policy::none());
        if config.connect_timeout_secs > 0 {
            builder = builder.connect_timeout(Duration::from_secs(config.connect_timeout_secs));
        }
        if config.read_timeout_secs > 0 {
            builder = builder.read_timeout(Duration::from_secs(config.read_timeout_secs));
        }

        // 显式配置代理：所有上游请求经该代理转发；reqwest 在 .proxy() 时会自动关闭
        // 系统代理（不再读取 HTTP_PROXY 等环境变量），避免两者互相干扰。
        // 代理 URL 已在 Config::validate() 校验，此处 expect 不会失败。
        if let Some(proxy_url) = config.proxy.as_deref() {
            let mut proxy = reqwest::Proxy::all(proxy_url).expect("代理 URL 已在 validate() 校验");
            // 单独配置的用户名/密码优先于 URL 内嵌凭据；仅配置密码而无用户名则忽略
            if let Some(username) = config.proxy_username.as_deref() {
                proxy = proxy.basic_auth(username, config.proxy_password.as_deref().unwrap_or(""));
            }
            builder = builder.proxy(proxy);
        }

        let client = builder.build().expect("构建 reqwest client 失败");
        // 磁盘缓存目录：关闭（disk_cache=false）时为 None——全程纯内存；
        // 开启时用测试注入覆盖，否则按端口取 ~/.aproxy/spool/<端口>/
        let spool_dir = if config.disk_cache_enabled() {
            Some(config.spool_dir_override.clone().unwrap_or_else(|| {
                crate::daemon::spool_dir_for(crate::daemon::port_of(&config.listen_addr))
            }))
        } else {
            None
        };
        // 活动时间戳与观测计数同属一个 IpcStats：serve_forever 把这一份整体交给
        // IPC 线程读。**两者必须同源**——本项目曾在此各建一份（IpcStats 里另有一个
        // 活动时间戳），而 serve_forever 只把活动时间戳共享进 IPC 那份，
        // 于是「请求 / 重试 / 最近错误」三项对任何实例都恒显示 0（活动时间戳正常
        // 反而掩盖了该缺陷，直到 2026-09-14 端到端实测才发现）。
        let last_activity_secs = Arc::new(std::sync::atomic::AtomicU64::new(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        ));
        // 受限重试路径模式：启动时编译一次（模式已在 validate() 校验过合法性，
        // 此处 expect 不会失败——与上方 proxy URL 的信任同一来源）
        let bounded_retry_patterns = Arc::new(
            config
                .bounded_retry_paths()
                .iter()
                .map(|p| {
                    Config::compile_bounded_retry_pattern(p)
                        .expect("受限重试路径模式已在 validate() 校验")
                })
                .collect(),
        );
        // 外部转换器进程池：按配置构造（未配置 = None，热路径零开销）。
        // 仅 spawn 模式惰性启动了 reaper 之外无任何后台任务，构造本身零开销
        let request_pool = config
            .request_transform
            .as_ref()
            .map(|t| Arc::new(crate::transform::TransformPool::new(Arc::new(t.clone()))));
        let response_pool = config
            .response_transform
            .as_ref()
            .map(|t| Arc::new(crate::transform::TransformPool::new(Arc::new(t.clone()))));
        let heartbeat_pool = config
            .heartbeat_transform
            .as_ref()
            .map(|t| Arc::new(crate::transform::TransformPool::new(Arc::new(t.clone()))));
        let inbound = Arc::new(InboundPolicy::from_config(&config));
        Self {
            spool_dir,
            config: Arc::new(config),
            client,
            stats: Arc::new(crate::daemon::IpcStats {
                last_activity_secs: last_activity_secs.clone(),
                ..Default::default()
            }),
            last_activity_secs,
            bounded_retry_patterns,
            request_pool,
            response_pool,
            heartbeat_pool,
            inbound,
        }
    }

    /// 请求是否命中受限重试路径：`路径?查询串` 整体匹配任一配置模式
    /// （模式在 Config::compile_bounded_retry_pattern 自动锚定，普通路径
    /// 即精准匹配）。
    pub fn bounded_retry_matches(&self, path_and_query: &str) -> bool {
        self.bounded_retry_patterns
            .iter()
            .any(|re| re.is_match(path_and_query))
    }

    /// 记录一次上游失败摘要（覆盖式，只保留最近一次；IPC/status 展示用）
    fn note_upstream_failure(&self, msg: &str) {
        self.stats.record_error(msg);
    }

    /// 实例闲置时长（秒）：距最近一次收到客户端请求。
    pub fn idle_secs(&self) -> u64 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        now.saturating_sub(
            self.last_activity_secs
                .load(std::sync::atomic::Ordering::Relaxed),
        )
    }
}

// ---------------------------------------------------------------------------
// 双模缓冲：内存（Bytes）与磁盘（临时文件）统一抽象
//
// 收集阶段超内存阈值即溢写磁盘；消费阶段（重试重放/回放给客户端）统一按
// Bytes 块产出。磁盘模式的内存在途占用为流式缓冲级（常数），与负载大小解耦。
// ---------------------------------------------------------------------------

/// 内存驻留阈值：缓冲超过即溢写磁盘（disk_cache 开启时）。
/// 1 MiB：覆盖真实 agent 流量（SSE/JSON 错误体 KB 级）使其零磁盘开销，
/// 大请求体/大响应才进入磁盘路径。
pub(crate) const RESIDENT_LIMIT: usize = 1024 * 1024;

/// 请求体缓冲：支持无限重试的字节重放源。
///
/// - Memory：单份 Bytes，重试间零拷贝共享（小请求体常态路径）
/// - Disk：溢写临时文件，每次重试重新流式读取（在途内存恒定）；
///   Drop 时删除临时文件（重试循环 continue / 客户端断开 / 请求完成全覆盖）
pub enum RequestBody {
    Memory(Bytes),
    Disk {
        path: PathBuf,
        /// 文件总字节数（回填 content-length 用）
        len: u64,
    },
}

/// 读取请求体错误：TooLarge=超出上限（413）；Io=连接中断/磁盘写入失败
enum ReadBodyError {
    TooLarge,
    Io(std::io::Error),
}

/// 读取请求体：内存缓冲，超 `RESIDENT_LIMIT` 且 disk_cache 可用则溢写磁盘。
/// `limit` 超出返回 TooLarge；客户端中断或写盘失败返回 Io。
///
/// 溢写文件在交给 `RequestBody::Disk` 之前由 `SpoolPath` 持有：显式的出错分支
/// 之外，客户端在上传途中断开时 hyper 会直接 drop 整个 handler future（本函数
/// 停在某个 await 上、不会走到任何返回分支），守卫的 Drop 照样删掉半截文件，
/// 不留 `req-*.spooltmp` 等到下次启动才回收。
async fn read_request_body(
    body: axum::body::Body,
    limit: usize,
    spool_dir: Option<&Path>,
) -> Result<RequestBody, ReadBodyError> {
    let mut body = body.into_data_stream();
    let mut mem: Vec<u8> = Vec::new();
    // 落盘状态：写句柄 + 删除责任守卫 + 已写字节数。各出错分支直接 return，
    // 守卫随之 drop 删除文件
    let mut disk: Option<(tokio::fs::File, SpoolPath, u64)> = None;
    loop {
        let chunk = match body.next().await {
            Some(Ok(c)) => c,
            Some(Err(e)) => return Err(ReadBodyError::Io(std::io::Error::other(e))),
            None => break,
        };
        // 上限判定：磁盘模式下基于累计总量；内存模式基于 Vec 长度
        let total = disk.as_ref().map_or(mem.len(), |(_, _, l)| *l as usize);
        if limit != usize::MAX && total + chunk.len() > limit {
            return Err(ReadBodyError::TooLarge);
        }
        match &mut disk {
            Some((file, _, len)) => {
                if file.write_all(&chunk).await.is_err() {
                    return Err(ReadBodyError::Io(std::io::Error::other(
                        "请求体磁盘缓存写入失败",
                    )));
                }
                *len += chunk.len() as u64;
            }
            None => {
                if mem.len() + chunk.len() > RESIDENT_LIMIT {
                    // 溢写决策：有目录才落盘；无目录（disk_cache 关闭/目录不可用）
                    // 继续内存缓冲（limit 语义不受影响）
                    if let Some(dir) = spool_dir {
                        let path = SpoolPath(dir.join(format!(
                            "req-{}-{}.spooltmp",
                            std::process::id(),
                            unique_seq()
                        )));
                        // 目录不可用（create 失败）：内存退化。写入失败时 file 与
                        // 守卫一起在本块末尾 drop——半截文件随之删除
                        if let Ok(mut file) = tokio::fs::File::create(&path.0).await
                            && file.write_all(&mem).await.is_ok()
                            && file.write_all(&chunk).await.is_ok()
                        {
                            disk = Some((file, path, (mem.len() + chunk.len()) as u64));
                            mem = Vec::new();
                            continue;
                        }
                    }
                }
                mem.extend_from_slice(&chunk);
            }
        }
    }
    Ok(match disk {
        Some((mut file, path, len)) => {
            // 写完整后 flush 确保后续重试读到全量数据（flush 只推缓冲到 OS，
            // 不等待物理落盘——page cache 一致性由 OS 保证）
            if file.flush().await.is_err() {
                return Err(ReadBodyError::Io(std::io::Error::other(
                    "请求体磁盘缓存写入失败",
                )));
            }
            drop(file);
            // 删除责任从守卫移交给 RequestBody（其 Drop 负责删文件）
            RequestBody::Disk {
                path: path.into_path(),
                len,
            }
        }
        None => RequestBody::Memory(Bytes::from(mem)),
    })
}

impl RequestBody {
    pub fn is_empty(&self) -> bool {
        match self {
            RequestBody::Memory(b) => b.is_empty(),
            RequestBody::Disk { len, .. } => *len == 0,
        }
    }

    /// 把请求体应用为 reqwest body：内存零拷贝；磁盘流式 + content-length。
    pub async fn apply_to(
        &self,
        mut builder: reqwest::RequestBuilder,
    ) -> Result<reqwest::RequestBuilder, std::io::Error> {
        match self {
            RequestBody::Memory(b) => {
                if !b.is_empty() {
                    builder = builder.body(b.clone());
                }
                Ok(builder)
            }
            RequestBody::Disk { path, len } => {
                let file = tokio::fs::File::open(path).await?;
                let stream = ReaderStream::with_capacity(file, 64 * 1024);
                Ok(builder
                    .header(http::header::CONTENT_LENGTH, len.to_string())
                    .body(reqwest::Body::wrap_stream(stream)))
            }
        }
    }
}

/// Drop 时删除临时文件（重试循环 continue / 客户端断开 / 请求完成全覆盖）
impl Drop for RequestBody {
    fn drop(&mut self) {
        if let RequestBody::Disk { path, .. } = self
            && !path.as_os_str().is_empty()
        {
            let _ = std::fs::remove_file(path);
        }
    }
}

// ---------------------------------------------------------------------------
// 保活触发判定（`keepalive_trigger`）
//
// 真实 Claude Code 的流式主请求是 `Accept: application/json` + 请求体
// `"stream": true`（2026-10-04 实测），只看 Accept 判不出它是流式请求。请求体
// 判定只认 JSON 顶层对象的 `stream` 键为字面量 `true`——OpenAI / Anthropic 等
// 协议通用的流式开关，与 URL 无关。
// ---------------------------------------------------------------------------

/// serde 访问器：只读 JSON 顶层对象的 `stream` 键，其余一切值经 `IgnoredAny`
/// 跳过——**不**把请求体物化成 `serde_json::Value`，内存占用与请求体大小无关
/// （几十 MB 的上下文 + base64 图片也只是顺序扫一遍）。
///
/// `top_level = true` 解释整个请求体：只有对象才可能为真，其他形态（数组、
/// 字符串……）一律为假；`top_level = false` 解释 `stream` 键的值：只有字面量
/// `true` 为真（`"true"`、`1` 都不算——上游协议也不认它们）。重复键按最后一次
/// 出现为准，与主流 JSON 解析器（JS `JSON.parse`、Python `json`）一致。
struct StreamFlagProbe {
    top_level: bool,
}

impl<'de> serde::de::DeserializeSeed<'de> for StreamFlagProbe {
    type Value = bool;
    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<bool, D::Error> {
        d.deserialize_any(self)
    }
}

impl<'de> serde::de::Visitor<'de> for StreamFlagProbe {
    type Value = bool;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("任意 JSON 值")
    }

    fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<bool, E> {
        Ok(!self.top_level && v)
    }
    fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<bool, E> {
        Ok(false)
    }
    fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<bool, E> {
        Ok(false)
    }
    fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<bool, E> {
        Ok(false)
    }
    fn visit_str<E: serde::de::Error>(self, _: &str) -> Result<bool, E> {
        Ok(false)
    }
    fn visit_unit<E: serde::de::Error>(self) -> Result<bool, E> {
        Ok(false)
    }
    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<bool, A::Error> {
        while seq.next_element::<serde::de::IgnoredAny>()?.is_some() {}
        Ok(false)
    }
    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<bool, A::Error> {
        use serde::de::IgnoredAny;
        if !self.top_level {
            while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
            return Ok(false);
        }
        let mut stream = false;
        // 顶层键名逐个分配 String：顶层键只有寥寥几个，嵌套对象的键全走 IgnoredAny
        while let Some(key) = map.next_key::<String>()? {
            if key == "stream" {
                stream = map.next_value_seed(StreamFlagProbe { top_level: false })?;
            } else {
                map.next_value::<IgnoredAny>()?;
            }
        }
        Ok(stream)
    }
}

/// 以 `StreamFlagProbe` 扫完整个 JSON 文本；任何解析错误（非 JSON、截断、尾随
/// 垃圾）都按「不是流式请求」处理——这样的请求体上游本就不会当流式请求处理。
fn json_requests_stream<'de, R: serde_json::de::Read<'de>>(
    mut de: serde_json::Deserializer<R>,
) -> bool {
    use serde::de::DeserializeSeed as _;
    matches!(
        StreamFlagProbe { top_level: true }.deserialize(&mut de),
        Ok(true)
    ) && de.end().is_ok()
}

/// 客户端原始请求体是否为顶层 `"stream": true` 的 JSON 对象。内存请求体
/// （≤ 1 MiB）就地扫描；溢写磁盘的大请求体在阻塞线程池里从文件流式读——
/// 几十 MB 的同步解析不能占住 async worker 线程。
async fn request_body_wants_stream(body: &RequestBody) -> bool {
    match body {
        RequestBody::Memory(b) => json_requests_stream(serde_json::Deserializer::from_slice(b)),
        RequestBody::Disk { path, .. } => {
            let path = path.clone();
            tokio::task::spawn_blocking(move || {
                let Ok(file) = std::fs::File::open(&path) else {
                    return false;
                };
                json_requests_stream(serde_json::Deserializer::from_reader(
                    std::io::BufReader::with_capacity(64 * 1024, file),
                ))
            })
            .await
            .unwrap_or(false)
        }
    }
}

/// 按 `keepalive_trigger` 判定本请求是否「保活适用」（keepalive 开关与
/// forward_only 由调用方另行把关）。必须在请求转换之前调用：判定是客户端视角
/// 的——转换器改写 Accept 或请求体（如协议转换去掉 stream 字段）不应改变
/// 「客户端在等一个流」这一事实。
async fn keepalive_triggered(
    trigger: crate::config::KeepaliveTrigger,
    headers: &HeaderMap,
    body: &RequestBody,
) -> bool {
    use crate::config::KeepaliveTrigger;
    let accept_sse = || {
        headers
            .get(http::header::ACCEPT)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|s| s.to_ascii_lowercase().contains("text/event-stream"))
    };
    match trigger {
        KeepaliveTrigger::Accept => accept_sse(),
        KeepaliveTrigger::BodyStream => request_body_wants_stream(body).await,
        // Accept 命中就不必扫请求体
        KeepaliveTrigger::Any => accept_sse() || request_body_wants_stream(body).await,
    }
}

/// 尚未交付的 spool 临时文件路径：Drop 时删除文件。缓冲在收集途中被丢弃的
/// 一切路径——客户端断开让在途的 forward_once future 整体被 drop、TooLarge、
/// 读取中断、写盘失败——都经此清理，不留半截 `.spooltmp` 等到下次启动才回收；
/// 正常收尾经 `into_path` 把路径交给 `SpooledBody`（文件从此由它负责）。
///
/// 删除时写句柄可能仍在 tokio 阻塞池的在途写操作里持有：Rust 标准库在 Windows
/// 上以 FILE_SHARE_DELETE 打开文件，删除照样成功（最后一个句柄关闭时文件消失），
/// unix 上 unlink 本就不受打开句柄影响。
struct SpoolPath(PathBuf);

impl SpoolPath {
    /// 交出路径并解除删除责任（取走后自身成空路径，Drop 不再删）
    fn into_path(mut self) -> PathBuf {
        std::mem::take(&mut self.0)
    }
}

impl Drop for SpoolPath {
    fn drop(&mut self) {
        if !self.0.as_os_str().is_empty() {
            let _ = std::fs::remove_file(&self.0);
        }
    }
}

/// 响应 spool 缓冲：内存累积，超 `RESIDENT_LIMIT` 溢写磁盘。
/// 完成后统一为 `SpooledBody` 消费（判定/回放）；中途丢弃时磁盘文件经
/// `SpoolPath` 的 Drop 删除。
enum SpoolBuffer {
    Memory(Vec<u8>),
    Disk {
        // 字段顺序即 drop 顺序：先关写句柄，再由 SpoolPath 删文件
        file: tokio::fs::File,
        path: SpoolPath,
        len: u64,
    },
    /// 收集中途写盘失败：内部状态不可用，forward_once 以 SpoolFailed 终态返回
    /// （本地磁盘故障与上游无关，重试无意义）
    Poisoned,
}

impl SpoolBuffer {
    fn len(&self) -> u64 {
        match self {
            SpoolBuffer::Memory(mem) => mem.len() as u64,
            SpoolBuffer::Disk { len, .. } => *len,
            SpoolBuffer::Poisoned => 0,
        }
    }

    /// 收集完成：内存 → SpooledBody::Memory（重试判定走整体扫描旧路径，
    /// disk_scan=None）；磁盘 → flush 后关写句柄产出 SpooledBody::Disk +
    /// （增量扫描结论, 头部快照）。消费 scanner。
    ///
    /// Err = 磁盘模式收尾 flush 失败（磁盘满/IO 故障），调用方按 SpoolFailed
    /// 处理；半截文件已在此删除。
    async fn finish(
        self,
        scanner: StreamErrorScanner,
    ) -> Result<(SpooledBody, Option<(bool, Vec<u8>)>), String> {
        match self {
            SpoolBuffer::Memory(mem) => Ok((SpooledBody::Memory(Bytes::from(mem)), None)),
            SpoolBuffer::Disk {
                mut file,
                path,
                len,
            } => {
                // drop 前必须 flush：tokio File 的 poll_write 把写操作派发到阻塞池后
                // **立刻**报 Ok，真实写入结果只在下一次 write/flush 时浮现——前面
                // 各块的错误会被后一块的 write_all 捕获（走 Poisoned），但**最后
                // 一块**之后再没有写操作，不 flush 就 drop 会把它的错误连同未完成
                // 的写一起吞掉：回放读到比 len 短的文件，DiskSpoolStream 把提前 EOF
                // 当正常结束，客户端拿到被静默截断的「成功」响应。内存→磁盘切换块
                // 恰是最后一块时同理——切换后本变体就是 Disk，同样由这里兜住。
                // flush 只把写推进 OS（不等物理落盘），与 read_request_body 同款。
                if let Err(e) = file.flush().await {
                    drop(file); // 先关句柄，再由 SpoolPath 的 Drop 删半截文件
                    drop(path);
                    return Err(format!("spool 磁盘写入失败（收尾 flush）: {e}"));
                }
                drop(file); // 关写句柄，头部快照以只读重新打开
                let head = match tokio::fs::File::open(&path.0).await {
                    Ok(mut f) => {
                        let mut buf = vec![0u8; 1024];
                        match f.read(&mut buf).await {
                            Ok(n) => {
                                buf.truncate(n);
                                buf
                            }
                            Err(_) => Vec::new(),
                        }
                    }
                    Err(_) => Vec::new(),
                };
                let error = scanner.finish();
                Ok((
                    SpooledBody::Disk {
                        path: path.into_path(),
                        len,
                    },
                    Some((error, head)),
                ))
            }
            // 不可达：Poisoned 在 forward_once 收集循环中提前返回。防御性产出
            // 空 body 成功（与磁盘判定缺失同样按不重试处理，服务不因内部
            // 不变量被破坏而失能）
            SpoolBuffer::Poisoned => {
                Ok((SpooledBody::Memory(Bytes::new()), Some((false, Vec::new()))))
            }
        }
    }
}

/// 扫描器可依赖的行长度上界：错误 JSON 行远小于此；超出即截断该行（丢一行
/// 启发式判定正确性，换内存上界恒定）。
const SCAN_MAX_LINE: usize = 512 * 1024;

/// 磁盘模式的流式错误扫描器：增量逐行判定（SSE data 行 / NDJSON 行），
/// 与 `retry::is_stream_error_body` 的行级语义一致（见其测试锚定：
/// 流尾 error 事件必须命中、CRLF/前导空格变体兼容）。
struct StreamErrorScanner {
    /// 跨 chunk 的未成行尾巴
    tail: Vec<u8>,
    /// 已判定出现 data 行（SSE 模式锁定）
    saw_data_line: bool,
    /// SSE 模式下的判定结果（锁定后不再改写）
    sse_has_error: bool,
    /// 已锁定结论
    decided: bool,
    /// 非 SSE 的 NDJSON 命中
    ndjson_error: bool,
}

impl StreamErrorScanner {
    fn new() -> Self {
        Self {
            tail: Vec::new(),
            saw_data_line: false,
            sse_has_error: false,
            decided: false,
            ndjson_error: false,
        }
    }

    fn feed(&mut self, chunk: &[u8]) {
        if self.decided {
            return;
        }
        self.tail.extend_from_slice(chunk);
        // 行缓存上界：超出即剪裁（丢判定正确性，换内存上界恒定）
        if self.tail.len() > SCAN_MAX_LINE {
            let drain = self.tail.len() - SCAN_MAX_LINE;
            self.tail.drain(..drain);
        }
        while let Some(pos) = self.tail.iter().position(|&b| b == b'\n') {
            let mut line: Vec<u8> = self.tail.drain(..=pos).collect();
            line.pop(); // 去 \n
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            self.scan_line(&line);
            if self.decided {
                self.tail.clear();
                return;
            }
        }
    }

    fn scan_line(&mut self, line: &[u8]) {
        let Ok(text) = std::str::from_utf8(line) else {
            return;
        };
        let trimmed = text.trim();
        let data = trimmed
            .strip_prefix("data:")
            .or_else(|| trimmed.strip_prefix("data :"))
            .map(str::trim);
        if let Some(data) = data {
            self.saw_data_line = true;
            if !data.is_empty() && data != "[DONE]" && retry::is_error_body(data.as_bytes()) {
                self.sse_has_error = true;
                self.decided = true;
            }
        } else if trimmed.starts_with('{') && retry::is_error_body(trimmed.as_bytes()) {
            self.ndjson_error = true;
            self.decided = true;
        }
    }

    /// 流结束后的最终判定：语义对齐 is_stream_error_body 的行级部分——
    /// 出现过 data 行则只以 SSE 结论为准；否则 NDJSON 命中为真。
    /// （「整体 body 单 JSON 兜底」不在此处：仅内存模式保留，见
    /// needs_retry_response 的 Memory 分支。差异依据：错误 JSON 均为 KB 级，
    /// 超过内存驻留阈值的响应几乎不可能是错误 JSON，且「首行即 JSON」的
    /// NDJSON 已由 scan_line 覆盖。）
    fn finish(mut self) -> bool {
        if self.decided {
            return self.sse_has_error || self.ndjson_error;
        }
        // 未成行的最后一行按行扫描（is_stream_error_body 的 lines() 同样
        // 会产出无换行结尾的最后一行）
        if !self.tail.is_empty() {
            let tail = std::mem::take(&mut self.tail);
            self.scan_line(&tail);
        }
        if self.saw_data_line {
            self.sse_has_error
        } else {
            self.ndjson_error
        }
    }
}

/// 收集一段响应体到 SpoolBuffer（内存累积 + 溢写磁盘）。
/// 磁盘不可用（目录创建失败/写失败）时回退纯内存继续收集——磁盘满不应使
/// 代理失能，最多退化为内存模式；唯有「已落盘后中途写失败」才 Poisoned。
async fn spool_chunk(buf: &mut SpoolBuffer, chunk: &[u8], spool_dir: Option<&Path>) {
    match buf {
        SpoolBuffer::Memory(mem) => {
            if mem.len() + chunk.len() <= RESIDENT_LIMIT {
                mem.extend_from_slice(chunk);
                return;
            }
            // 超阈值：已有内存迁移到磁盘；磁盘不可用则留在内存（退化）
            let Some(dir) = spool_dir else {
                mem.extend_from_slice(chunk);
                return;
            };
            // 文件一创建就交给 SpoolPath 守卫：下面两次 write_all（含 1 MiB 的
            // 内存前缀）期间本 future 随时可能被 drop（客户端断开），守卫保证
            // 迁移中途的半截文件同样被删
            let path = SpoolPath(dir.join(format!(
                "spool-{}-{}.spooltmp",
                std::process::id(),
                unique_seq()
            )));
            match tokio::fs::File::create(&path.0).await {
                Ok(mut file) => {
                    if file.write_all(mem).await.is_ok() && file.write_all(chunk).await.is_ok() {
                        let len = (mem.len() + chunk.len()) as u64;
                        *buf = SpoolBuffer::Disk { file, path, len };
                        return;
                    }
                    // 写失败：文件句柄已创建但内容不完整，删除（关句柄后守卫
                    // 随作用域结束删文件）；保持 Memory 退化
                    //（create 成功 write 失败极罕见，但半截文件绝不能留下）
                    drop(file);
                    mem.extend_from_slice(chunk);
                }
                Err(_) => mem.extend_from_slice(chunk),
            }
        }
        SpoolBuffer::Disk { file, len, .. } => {
            // 已落盘后写失败：进程内 spool 无法回退（数据已在盘上），只能
            // Poisoned。被替换掉的 Disk 变体随之 drop：先关句柄、再删半截文件
            if file.write_all(chunk).await.is_err() {
                *buf = SpoolBuffer::Poisoned;
                return;
            }
            *len += chunk.len() as u64;
        }
        SpoolBuffer::Poisoned => {}
    }
}

/// 临时文件名唯一序号：进程内单调即可（同进程不重名；跨进程由 pid 区分）
pub(crate) fn unique_seq() -> u64 {
    use std::sync::atomic::AtomicU64;
    static SEQ: AtomicU64 = AtomicU64::new(0);
    SEQ.fetch_add(1, AtomicOrdering::Relaxed)
}

/// 完整收集后的响应体：消费方式统一（判定 + 回放），磁盘模式自带 Drop 清理
pub enum SpooledBody {
    Memory(Bytes),
    Disk { path: PathBuf, len: u64 },
}

impl Drop for SpooledBody {
    fn drop(&mut self) {
        if let SpooledBody::Disk { path, .. } = self {
            // 消费式回放接管文件后 path 已被置空，此处只删「未消费即丢弃」的
            // （重试 continue / TooLarge 判定后 / 客户端断开）
            if !path.as_os_str().is_empty()
                && let Err(e) = std::fs::remove_file(&*path)
            {
                tracing::warn!(error = %e, path = %path.display(), "spool 临时文件删除失败");
            }
            *path = PathBuf::new();
        }
    }
}

impl SpooledBody {
    pub fn len(&self) -> u64 {
        match self {
            SpooledBody::Memory(b) => b.len() as u64,
            SpooledBody::Disk { len, .. } => *len,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 内存模式的字节视图（整体判定用）。仅内存模式调用——磁盘模式的判定
    /// 走增量扫描结论（forward_once 的 disk_scan 字段）。
    pub fn memory_bytes(&self) -> &[u8] {
        match self {
            SpooledBody::Memory(b) => b,
            SpooledBody::Disk { .. } => &[],
        }
    }

    /// 摘出内存字节（Drop 语义下的安全消费：经 &mut swap，不走字段 move）。
    /// 磁盘模式返回空（调用方保证不会走到）。
    fn take_memory(&mut self) -> Bytes {
        match self {
            SpooledBody::Memory(b) => std::mem::take(b),
            SpooledBody::Disk { .. } => Bytes::new(),
        }
    }

    /// 回放流：内存按 8 KiB 零拷贝切块；磁盘流式读 64 KiB 块，EOF/Drop 时
    /// 删除临时文件。读取失败产出 Err（本地磁盘故障，回放中断）。
    pub async fn into_stream(
        &mut self,
    ) -> futures_util::stream::BoxStream<'static, Result<Bytes, std::io::Error>> {
        match self {
            SpooledBody::Memory(b) => {
                const CHUNK_SIZE: usize = 8 * 1024;
                let b = std::mem::take(b);
                futures_util::stream::iter(
                    b.chunks(CHUNK_SIZE)
                        .map(|c| Ok(b.slice_ref(c)))
                        .collect::<Vec<_>>(),
                )
                .boxed()
            }
            SpooledBody::Disk { path, len } => {
                // mem::take 剥离 Drop 责任：文件由 DiskSpoolStream 负责
                //（EOF/Drop 删除），此处 SpooledBody 已成空壳
                let path = std::mem::take(path);
                let len = *len;
                match tokio::fs::File::open(&path).await {
                    Ok(file) => DiskSpoolStream {
                        inner: Some((file, len)),
                        path,
                    }
                    .boxed(),
                    Err(e) => {
                        // 文件丢失（外部清理等）：回放为空 body，日志留痕
                        tracing::warn!(error = %e, path = %path.display(), "spool 临时文件读取失败，回放为空");
                        futures_util::stream::empty().boxed()
                    }
                }
            }
        }
    }
}

/// 磁盘 spool 的回放流：从临时文件流式读出（64 KiB 块）；EOF、提前丢弃
/// （Drop）或读取错误时删除文件。Windows 语义要求先关句柄再删，故 finish
/// 先 take 掉 File。
struct DiskSpoolStream {
    /// Some((file, 剩余字节))；None = 已结束
    inner: Option<(tokio::fs::File, u64)>,
    path: PathBuf,
}

impl DiskSpoolStream {
    fn finish(&mut self) {
        self.inner = None; // 关句柄
        let _ = std::fs::remove_file(&self.path);
    }
}

impl Drop for DiskSpoolStream {
    fn drop(&mut self) {
        // EOF 后重复调用幂等（文件已删，remove 的 NotFound 忽略）
        self.finish();
    }
}

impl Stream for DiskSpoolStream {
    type Item = Result<Bytes, std::io::Error>;
    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        let Some((file, remaining)) = this.inner.as_mut() else {
            return Poll::Ready(None);
        };
        if *remaining == 0 {
            this.finish();
            return Poll::Ready(None);
        }
        let want = (*remaining).min(64 * 1024) as usize;
        let mut buf = vec![0u8; want];
        let mut rb = tokio::io::ReadBuf::new(&mut buf);
        match ready!(std::pin::Pin::new(file).poll_read(cx, &mut rb)) {
            Err(e) => {
                this.finish();
                Poll::Ready(Some(Err(e)))
            }
            Ok(()) => {
                let filled = rb.filled();
                if filled.is_empty() {
                    // 提前 EOF（文件比记录的 len 短）：按结束处理
                    this.finish();
                    return Poll::Ready(None);
                }
                *remaining -= filled.len() as u64;
                Poll::Ready(Some(Ok(Bytes::copy_from_slice(filled))))
            }
        }
    }
}

/// 构建路由：全量透传（无额外健康检查路径，避免与上游 `/health` 等冲突）
pub fn router(state: AppState) -> Router {
    Router::new().fallback(proxy_handler).with_state(state)
}

/// 将配置中的头覆盖/追加逻辑应用到待发往上游的 HeaderMap（大小写不敏感判定）。
fn apply_header_overrides(headers: &mut HeaderMap, config: &Config) {
    // api_key 快捷：等效覆盖 Authorization: Bearer <key>（大小写不敏感覆盖）。
    // 同时覆盖 x-api-key（Anthropic 风格上游使用该头携带原始 key，若只覆盖
    // Authorization，客户端原带的 x-api-key 会原样漏到上游造成鉴权混乱）。
    if let Some(key) = config.api_key.as_deref() {
        let val = format!("Bearer {key}");
        if let Ok(v) = HeaderValue::from_str(&val) {
            headers.remove(http::header::AUTHORIZATION);
            headers.insert(http::header::AUTHORIZATION, v);
        }
        if let Ok(raw) = HeaderValue::from_str(key) {
            headers.remove("x-api-key");
            headers.insert(HeaderName::from_static("x-api-key"), raw);
        }
    }

    // override_headers：无条件覆盖
    for (k, v) in &config.override_headers {
        let Ok(name) = HeaderName::from_bytes(k.as_bytes()) else {
            continue;
        };
        let Ok(val) = HeaderValue::from_str(v) else {
            continue;
        };
        headers.remove(&name);
        headers.insert(name, val);
    }

    // extra_headers：仅当未携带时追加
    for (k, v) in &config.extra_headers {
        let Ok(name) = HeaderName::from_bytes(k.as_bytes()) else {
            continue;
        };
        if headers.contains_key(&name) {
            continue;
        }
        let Ok(val) = HeaderValue::from_str(v) else {
            continue;
        };
        headers.insert(name, val);
    }
}

/// 核心代理处理器：完整透传 + 无限重试 + 流式 spool 后回放 + 保活心跳
async fn proxy_handler(State(state): State<AppState>, req: Request) -> Response {
    let method = req.method().clone();
    let uri = req.uri().clone();

    // 入站来源校验：必须是第一步——先于请求体读取（被拒请求不值得缓冲/落盘
    // 任何字节）、先于 apply_header_overrides（绝不给它注入 api_key）、先于
    // 仅转发分支与重试循环（绝不转发）。也先于活动时间戳与请求计数：被拒的
    // 请求不是代理流量，不应让探测者把实例「刷」成活跃、也不计入请求数。
    if let Some(rejection) = state.inbound.check(req.headers(), &uri) {
        return inbound_rejection_response(rejection, &method, &uri);
    }

    let mut headers = req.headers().clone();

    // 活动时间戳：收到请求即更新（stop idle / status 筛选的判定依据）
    state.last_activity_secs.store(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        std::sync::atomic::Ordering::Relaxed,
    );
    // IPC 观测计数（一条 Relaxed fetch_add，与上面 store 同量级）；计数顺带
    // 充当转换信封的 request_id（实例内第 N 个请求，唯一且与 status 的计数一致）
    let request_seq = state
        .stats
        .requests_total
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        + 1;

    // 在本地侧先应用覆盖/追加，避免重试间重复计算
    apply_header_overrides(&mut headers, &state.config);

    // 仅转发模式：分支点必须在 read_request_body **之前**——缓冲一旦发生，本模式
    // 就丢掉了全部意义（内存随负载增长、下游要等请求体读完才见到首字节）；
    // 同时它位于 requests_total.fetch_add 与 apply_header_overrides **之后**，
    // 所以 status 的请求计数与鉴权/头改写行为与常规模式完全一致。
    // 这条路径整体不进重试循环、保活心跳、错误内容拦截、spool 与 keepalive_trigger
    // 判定，也不做响应体解码（本模式不对响应内容做任何检查，无需解码）。
    if state.config.forward_only_enabled() {
        let target_url = upstream_url(&state.config, &uri);
        // 日志里的 URL 一律经 mask_base_url：base_url 可能内嵌 user:pass，客户端
        // 查询串可能带 key（Gemini 式 ?key=），日志常被整段粘贴求助
        tracing::info!(method = %method, target = %crate::config::mask_base_url(&target_url), "代理请求（仅转发）");
        return forward_only_proxy(state, method, target_url, headers, req.into_body()).await;
    }

    // 缓冲请求体以支持重试重放；上限 max_body_mb（默认 128 MB，0=不限），
    // 超出直接 413。disk_cache 开启时超过内存驻留阈值（1 MiB）溢写磁盘，
    // 大请求体的进程内存在途占用恒定。
    let body_limit = state.config.body_limit_bytes();
    let req_body =
        match read_request_body(req.into_body(), body_limit, state.spool_dir.as_deref()).await {
            Ok(b) => b,
            Err(ReadBodyError::TooLarge) => return body_too_large_response(body_limit),
            Err(ReadBodyError::Io(e)) => {
                tracing::error!(error = %e, "读取请求体失败");
                return (
                    StatusCode::BAD_REQUEST,
                    format!("请求体读取失败（连接中断或本地磁盘缓存写入失败）: {e}"),
                )
                    .into_response();
            }
        };

    let path_and_query = uri.path_and_query().map(|pq| pq.as_str()).unwrap_or("/");
    let bounded_retry = state.bounded_retry_matches(path_and_query);
    // 是否「保活适用」（走保活通道：先提交响应头、再以 SSE 注释心跳维持连接）：
    // keepalive 开启且 keepalive_trigger 判定命中。必须在请求转换前判定——
    // 判定语义是「客户端视角」，转换器改写 Accept/请求体不影响保活选择
    let keepalive_dur = state.config.keepalive_interval();
    let keepalive_enabled = state.config.keepalive_enabled() && keepalive_dur.as_secs() > 0;
    let keepalive_applies = keepalive_enabled
        && keepalive_triggered(state.config.keepalive_trigger(), &headers, &req_body).await;

    let req = OutboundRequest {
        method,
        target_url: upstream_url(&state.config, &uri),
        headers,
        body: req_body,
        ctx: ExchangeCtx::new(request_seq.to_string()),
    };
    let max_spool_bytes = spool_limit_bytes(&state.config);

    // 保活适用：从请求转换起就交给保活通道（它自己处理请求转换、首轮快速路径
    // 与提交点）。请求转换放进通道里做，是为了让它同样处在心跳节拍内——format
    // 慢（timeout_secs = 0 时没有上界）时客户端照样在一个间隔后收到骨架头与
    // 心跳，而不是在拿到任何字节之前干等。req 所有权移交，请求体随通道结束
    // 自动 Drop 删除磁盘临时文件。
    if keepalive_applies {
        return proxy_with_keepalive(
            state,
            req,
            path_and_query.to_string(),
            max_spool_bytes,
            bounded_retry,
        )
        .await;
    }

    let OutboundRequest {
        method,
        target_url,
        headers,
        body: req_body,
        ctx,
    } = match apply_request_transform(&state, req).await {
        Ok(req) => req,
        Err(msg) => return request_transform_failed_response(&msg),
    };
    log_proxied_request(&method, path_and_query, &target_url);

    // 首轮（attempt 1）先行：成功则完整保真回放（status/headers 不失真），
    // 需要重试才进入重试通道——首轮成功是常态路径。
    let first = forward_once(
        &state,
        &method,
        &target_url,
        &headers,
        &req_body,
        max_spool_bytes,
        None,
    )
    .await;

    let needs_retry = match &first {
        ForwardResult::NetworkError(e) => {
            state.note_upstream_failure(&format!("网络错误: {e}"));
            tracing::warn!(error = %e, "首轮上游网络错误，进入重试");
            true
        }
        // spool 上限 / 本地磁盘故障重试无意义（确定性失败），直接终态回放 502
        ForwardResult::TooLarge => {
            state.note_upstream_failure("上游响应体超出 spool 上限");
            false
        }
        ForwardResult::SpoolFailed(e) => {
            state.note_upstream_failure(&format!("本地 spool 故障: {e}"));
            tracing::error!(error = %e, "首轮本地 spool 故障，终态返回");
            false
        }
        ForwardResult::Response {
            status,
            raw_headers,
            body,
            disk_scan,
            ..
        } => {
            let retry = needs_retry_response(1, status, raw_headers, body, disk_scan);
            if retry {
                state.note_upstream_failure(&format!("上游返回 {status}（错误内容，重试）"));
            }
            retry
        }
    };

    if !needs_retry {
        return match first {
            ForwardResult::TooLarge => too_large_response(),
            ForwardResult::SpoolFailed(e) => spool_failed_response(&e),
            ForwardResult::Response {
                status,
                headers: resp_headers,
                raw_headers,
                body,
                ..
            } => {
                replay_success(
                    &state,
                    &target_url,
                    &ctx,
                    1,
                    status,
                    resp_headers,
                    &raw_headers,
                    body,
                )
                .await
            }
            ForwardResult::NetworkError(_) => unreachable!("NetworkError 必定 needs_retry"),
        };
    }

    // 需要重试（不适用保活）：成功后一次性回放（req_body 所有权移交，随通道
    // 结束自动 Drop 删除磁盘临时文件）
    let req = OutboundRequest {
        method,
        target_url,
        headers,
        body: req_body,
        ctx,
    };
    proxy_without_keepalive(state, req, max_spool_bytes, bounded_retry).await
}

/// 发往上游的请求：请求转换（若配置）之后的最终形态，重试循环的每一轮原样重放。
/// `ctx` 随请求带到响应转换（同一 request_id、请求转换留下的 state）。
struct OutboundRequest {
    method: http::Method,
    target_url: String,
    headers: HeaderMap,
    body: RequestBody,
    ctx: ExchangeCtx,
}

/// 请求转换器：缓冲完成后交给外部 format 程序改写（body/headers/url/method
/// 全部可变）——转换**一次**，产物被重试循环的每一轮 forward_once 原样重放，
/// 重试循环零分支。位置约束：必须在首轮 forward_once 之前（否则重放的是未
/// 转换请求）、bounded_retry 匹配之后（bounded 对原始路径判定——路径集合是
/// 客户端视角，转换是实例级配置，语义不同源）。未配置时原样返回。
///
/// 失败返回给客户端看的原因（已记日志与最近一次失败）：请求未发往上游，按约定
/// 不重试（失败按成因分类，各类性质见 `transform::TransformError` 的分类说明）。
/// 调用方按响应是否已提交决定回 502 还是写终态 error 事件。
async fn apply_request_transform(
    state: &AppState,
    req: OutboundRequest,
) -> Result<OutboundRequest, String> {
    let Some(pool) = state.request_pool.as_ref() else {
        return Ok(req);
    };
    let OutboundRequest {
        method,
        target_url,
        headers,
        body,
        ctx,
    } = req;
    match crate::transform::transform_request(
        pool,
        &ctx,
        &method,
        &target_url,
        &headers,
        body,
        state.spool_dir.as_deref(),
    )
    .await
    {
        Ok(t) => {
            tracing::info!(url = %crate::config::mask_base_url(&t.url), "请求已由外部转换器改写");
            Ok(OutboundRequest {
                method: t.method,
                target_url: t.url,
                headers: t.headers,
                body: t.body,
                ctx,
            })
        }
        Err(e) => {
            let msg = format!("请求转换失败: {e}");
            state.note_upstream_failure(&msg);
            tracing::warn!(error = %e, "请求转换失败，终态返回（请求侧转换失败按约定不重试）");
            Err(msg)
        }
    }
}

/// 请求转换失败的 502 终态（尚未向客户端提交任何字节时）
fn request_transform_failed_response(msg: &str) -> Response {
    (
        StatusCode::BAD_GATEWAY,
        format!("{msg}（请求侧转换失败不重试，未发往上游）"),
    )
        .into_response()
}

/// 已提交的保活响应以终态 SSE error 事件收场（状态行已发出、不可再改）。
/// message 经 JSON 序列化，任意文案（含引号、换行、中文）都不会破坏事件帧。
fn terminal_error_event(error_type: &str, message: &str) -> Bytes {
    let data = serde_json::json!({
        "type": "error",
        "error": { "type": error_type, "message": message },
    });
    Bytes::from(format!("event: error\ndata: {data}\n\n"))
}

/// 「代理请求」日志：请求转换之后、首轮之前各通道各记一次。URL 一律经
/// mask_base_url：base_url 可能内嵌 user:pass，查询串可能带 key（Gemini 式
/// ?key=），日志常被整段粘贴求助
fn log_proxied_request(method: &http::Method, path_and_query: &str, target_url: &str) {
    tracing::info!(
        method = %method,
        path = %crate::config::mask_base_url(path_and_query),
        target = %crate::config::mask_base_url(target_url),
        "代理请求"
    );
}

/// 上游响应体超出 spool 上限的 502 终态（尚未向客户端提交任何字节时）
fn too_large_response() -> Response {
    (
        StatusCode::BAD_GATEWAY,
        "上游响应体超出 spool 上限，无法回放（重试无意义）",
    )
        .into_response()
}

/// 本地磁盘 spool 故障的 502 终态（尚未向客户端提交任何字节时）
fn spool_failed_response(e: &str) -> Response {
    (
        StatusCode::BAD_GATEWAY,
        format!("本地磁盘缓存写入失败，无法回放: {e}"),
    )
        .into_response()
}

/// 成功响应的保真回放（尚未向客户端提交任何字节的出口共用：首轮快速路径、
/// 非保活重试通道、保活通道里提交前就成功的首轮）：先交响应转换器（失败透传
/// 原样），再按转换后产物重算流式判定（format 可改 content-type/body 形态），
/// 最后以上游 status 与响应头原样回放。
// 参数分两组：请求侧（target_url + ctx，响应转换要用）与本次尝试的成功响应
// （attempt/status/头/体，来自 ForwardResult::Response 的拆解）。两个调用点
// 都是刚拆开 ForwardResult 就调用，再包一层结构体只是把拆开的字段原样装回去
#[allow(clippy::too_many_arguments)]
async fn replay_success(
    state: &AppState,
    target_url: &str,
    ctx: &ExchangeCtx,
    attempt: u32,
    status: StatusCode,
    resp_headers: HeaderMap,
    raw_headers: &reqwest::header::HeaderMap,
    body: SpooledBody,
) -> Response {
    let (resp_headers, body) =
        transform_response_if_configured(state, target_url, ctx, resp_headers, body).await;
    replay_transformed(attempt, status, resp_headers, raw_headers, body).await
}

/// 成功响应的保真回放（响应转换已做完）：按转换后的头与体重算是否流式，
/// 原样回放 status/头/体。format 可能改写 content-type 与 body 形态，所以
/// 流式判定必须在转换之后。
async fn replay_transformed(
    attempt: u32,
    status: StatusCode,
    resp_headers: HeaderMap,
    raw_headers: &reqwest::header::HeaderMap,
    body: SpooledBody,
) -> Response {
    let is_streaming = match &body {
        // 内存模式：整体判定（含 body 嗅探，与旧行为一致）
        SpooledBody::Memory(_) => retry::is_streaming_response(&resp_headers, body.memory_bytes()),
        // 磁盘模式：content-type 判定（全量嗅探需读回整个文件，而磁盘回放本就
        // 是 chunked 流式）
        SpooledBody::Disk { .. } => retry::is_streaming_content_type(&resp_headers),
    };
    tracing::info!(attempt, status = %status, is_streaming, "上游成功，回放响应");
    build_replay_response(status, resp_headers, raw_headers, body, is_streaming).await
}

/// 响应侧转换入口（三出口共用）：未配置时原样返回；转换失败时 warn + 原样
/// 返回（透传上游原始响应——响应已在手，可用性优先，用户已定语义）。
/// is_streaming 由调用方按返回值重算（format 可改 content-type/body 形态）。
async fn transform_response_if_configured(
    state: &AppState,
    upstream_url: &str,
    ctx: &ExchangeCtx,
    headers: HeaderMap,
    body: SpooledBody,
) -> (HeaderMap, SpooledBody) {
    let Some(pool) = state.response_pool.as_ref() else {
        return (headers, body);
    };
    let content_encoding = headers
        .get(http::header::CONTENT_ENCODING)
        .and_then(|v| v.to_str().ok().map(|s| s.to_string()));
    let out = crate::transform::transform_response(
        pool,
        ctx,
        upstream_url,
        content_encoding.as_deref(),
        headers,
        body,
        state.spool_dir.as_deref(),
    )
    .await;
    if let Some(e) = &out.error {
        tracing::warn!(error = %e, "响应转换失败，透传上游原始响应");
    }
    // 失败时 transform_response 已把 headers/body 还原为原始值（透传语义），
    // 两分支同构返回
    (out.headers, out.body)
}

/// 统一的重试判定入口：内存模式走整体判定（旧行为），磁盘模式走增量扫描
/// 结论 + 头部快照预览。三处（首轮/两个重试通道）共用，保证判定语义一致。
fn needs_retry_response(
    attempt: u32,
    status: &StatusCode,
    raw_headers: &reqwest::header::HeaderMap,
    body: &SpooledBody,
    disk_scan: &Option<(bool, Vec<u8>)>,
) -> bool {
    match (body, disk_scan) {
        (SpooledBody::Memory(b), _) => should_retry_response(attempt, status, raw_headers, b),
        // 真实响应大小取自 spool 长度（`head` 只是 1 KiB 头部快照，其长度不能
        // 当响应大小用——预览日志的「共 N 字节」正因此改成显式传入）
        (SpooledBody::Disk { len, .. }, Some((error, head))) => {
            should_retry_response_disk(attempt, status, raw_headers, *error, head, *len as usize)
        }
        // 不可能：磁盘模式必有增量扫描结果。防御性回退到「不重试」并留痕，
        // 而非 panic——代理服务不能因内部不变量被破坏而失能
        (SpooledBody::Disk { .. }, None) => {
            tracing::error!("内部不变量破坏：磁盘 spool 缺少扫描结果，按成功处理");
            false
        }
    }
}

/// 判定一次上游响应是否需要重试，并输出对应的诊断日志。
///
/// 判定顺序：可重试状态码（4xx/5xx）→ 内容错误（流式扫描 data 行，或非流式错误 JSON）。
/// 首轮与两个重试通道共用，保证三处判定语义一致。
fn should_retry_response(
    attempt: u32,
    status: &StatusCode,
    raw_headers: &reqwest::header::HeaderMap,
    body: &[u8],
) -> bool {
    // 压缩响应体（上游按客户端 accept-encoding 压缩，而 reqwest 为保真透传刻意
    // 不解压）在**原始字节**上做任何内容判定都是无效的：JSON 解析与 SSE 行扫描
    // 会静默失败，预览只能打 hex。故检查一律走解码副本——转发给客户端的字节
    // 不受影响（`decode` 模块文档记录了这组失效的完整清单与事故由来）。
    // 无 content-encoding 时 for_inspection 立即返回 None，不产生任何拷贝。
    let inspected = crate::decode::for_inspection(
        header_opt(raw_headers, http::header::CONTENT_ENCODING),
        body,
    );
    let body = inspected.as_deref().unwrap_or(body);

    let is_streaming = retry::is_streaming_response(raw_headers, body);

    // 1. HTTP 状态码可重试：无论是否流式，都重试
    if retry::is_retryable_status(status.as_u16()) {
        tracing::warn!(attempt, status = %status, is_streaming, "上游返回可重试状态码，重试");
        warn_preview(raw_headers, body, 500, body.len());
        return true;
    }

    // 2. 内容错误检测：对所有错误都重试——原型期 4xx 亦视为易发生的瞬时错误。
    //    即使状态码未命中可重试（如 200 携带 error JSON），只要 body 语义为报错就重试。
    let is_error = if is_streaming {
        retry::is_stream_error_body(body)
    } else {
        retry::is_error_body(body)
    };
    if is_error {
        tracing::warn!(attempt, is_streaming, "上游返回错误内容，重试");
        warn_preview(raw_headers, body, 1000, body.len());
        return true;
    }

    false
}

/// 磁盘模式的重试判定：状态码/流式判定用响应头；内容错误用收集期间的增量
/// 扫描结论；预览用头部快照（磁盘全量不可得，头部 1 KiB 足够排障）。
///
/// `total_bytes` 是 spool 里的**实际响应字节数**（`SpooledBody::Disk::len`）：
/// 内存模式那份 body 就是全部内容，长度自证；磁盘模式给到这里的只是 1 KiB 快照，
/// 快照长度与响应大小无关，日志里的字节数必须由调用方从 spool 长度补进来。
fn should_retry_response_disk(
    attempt: u32,
    status: &StatusCode,
    raw_headers: &reqwest::header::HeaderMap,
    scan_error: bool,
    head_snapshot: &[u8],
    total_bytes: usize,
) -> bool {
    let is_streaming = retry::is_streaming_content_type(raw_headers);

    // 预览用解码：头部快照正是压缩流的开头，解出的前缀足以看清错误页内容
    //（截断流取已解出部分）。**判定**不在此列——`scan_error` 来自原始字节的
    // 增量扫描结论；为 >1 MiB 的响应改造成流式解码，收益与风险不成比例，
    // 见 `decode` 模块文档的「已知边界」。
    let inspected = crate::decode::for_inspection(
        header_opt(raw_headers, http::header::CONTENT_ENCODING),
        head_snapshot,
    );
    let head = inspected.as_deref().unwrap_or(head_snapshot);

    // 1. HTTP 状态码可重试：无论是否流式，都重试
    if retry::is_retryable_status(status.as_u16()) {
        tracing::warn!(attempt, status = %status, is_streaming, "上游返回可重试状态码，重试");
        warn_preview(raw_headers, head, 500, total_bytes);
        return true;
    }

    // 2. 内容错误：增量扫描结论（SSE data 行 / NDJSON 行级判定）
    if scan_error {
        tracing::warn!(attempt, is_streaming, "上游返回错误内容，重试");
        warn_preview(raw_headers, head, 1000, total_bytes);
        return true;
    }

    false
}

/// 取响应头值的字符串视图；缺失或非 UTF-8 → `None`。
/// 保留 `None`（而非直接给占位符）是因为调用方需要区分「有没有这个头」——
/// 解码与否正取决于此（`"-"` 会被当成一个未知编码）。
fn header_opt(
    headers: &reqwest::header::HeaderMap,
    name: http::header::HeaderName,
) -> Option<&str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

/// 错误响应预览日志：预览内容 + `content-type` / `content-encoding`。
///
/// 两个头在任何情况下都打——**解码失败退化为 hex 摘要时，它们正是判断「上游
/// 到底回了什么」的关键线索**：brotli 没有 magic number，光看字节连「这是压缩体」
/// 都认不出（2026-09-14 事故：Cloudflare 的 brotli 404 页在日志里只剩一串 hex）。
/// 缺失时以 `"-"` 占位，日志字段不留空以便 grep 与扫读。
/// `total_bytes` 是「共 N 字节」的口径，由调用方按自己掌握的信息给出：
/// 内存模式传整份 body 的长度（body 就在手上，长度即响应大小）；磁盘模式只能
/// 传真值——传 `SpooledBody::Disk::len`（spool 里的实际字节数）。见 `preview_body`。
fn warn_preview(
    raw_headers: &reqwest::header::HeaderMap,
    body: &[u8],
    limit: usize,
    total_bytes: usize,
) {
    tracing::warn!(
        content_type = %header_opt(raw_headers, http::header::CONTENT_TYPE).unwrap_or("-"),
        content_encoding = %header_opt(raw_headers, http::header::CONTENT_ENCODING).unwrap_or("-"),
        preview = %preview_body(body, limit, total_bytes),
        "错误响应预览"
    );
}

/// 错误响应体的日志预览。**压缩体已在调用方按 `content-encoding` 解码**
/// （见 `decode` 模块），走到这里的是解不出的、本就二进制/压缩但编码未知的体；
/// 直接 from_utf8_lossy 会把控制字节渲染成整片乱码污染日志。判定：替换符/控制
/// 字符占比超阈值视为二进制，改为 hex 摘要（hex 可识别压缩 magic：zstd
/// 28 b5 2f fd、gzip 1f 8b 等，据此判断解码为何没成功）。
/// `total_bytes` 是日志里「共 N 字节」用的**实际响应字节数**，不是本函数手里
/// 这份字节的长度。两者只在内存模式相同；磁盘模式传入的是 1 KiB **头部快照**
/// （且可能已被 `content-encoding` 解码成更短的副本），其长度只反映快照本身，
/// 与响应大小无关——照快照长度打印会让读日志的人以为上游只回了这么点数据，
/// 把排障方向带偏（磁盘模式 > 1 MiB 的响应恰恰最需要看真实量级）。
fn preview_body(body: &[u8], limit: usize, total_bytes: usize) -> String {
    let head = &body[..body.len().min(limit)];
    // lossy 渲染后统计非文本占比：U+FFFD 与 C0 控制字符
    let text = String::from_utf8_lossy(head);
    let total = text.chars().count().max(1);
    let weird = text
        .chars()
        .filter(|&c| c == '\u{FFFD}' || (c.is_control() && c != '\t' && c != '\n' && c != '\r'))
        .count();
    if weird * 10 >= total {
        // 二进制/压缩内容：hex 前 48 字节足够识别压缩 magic 与排障
        let hex: String = head.iter().take(48).map(|b| format!("{b:02x} ")).collect();
        format!(
            "（二进制/压缩内容，共 {total_bytes} 字节，hex 前 48: {}…）",
            hex.trim_end()
        )
    } else {
        let s = text.trim_end();
        if body.len() > limit {
            format!("{s}…（截断，共 {total_bytes} 字节）")
        } else {
            s.to_string()
        }
    }
}

/// 不适用保活的请求的重试通道：从 attempt 2 起无限重试（attempt 1 已在 proxy_handler
/// 完成），响应在成功后一次性返回。受限重试路径（`bounded_retry`）例外：有响应的失败
/// 达到 [`retry::BOUNDED_RETRY_MAX_ATTEMPTS`] 次后透传最后一次失败响应。
async fn proxy_without_keepalive(
    state: AppState,
    req: OutboundRequest,
    max_spool_bytes: usize,
    bounded_retry: bool,
) -> Response {
    let OutboundRequest {
        method,
        target_url,
        headers,
        body: req_body,
        ctx,
    } = req;
    let mut attempt: u32 = 1;
    let max_backoff = state.config.max_retry_backoff_secs;
    loop {
        attempt += 1;
        // 每轮重试开始处刷新活动时间戳：last_activity_secs 的语义是「最近一次
        // 收到请求**或仍在处理**」——长退避（封顶默认 320s）可能超过
        // idle_timeout_secs（默认 1800s 之内更长的自定义值更危险），不刷新的话
        // 无限重试中的实例会被 stop idle / status --idle 误判为闲置并强退，
        // 恰恰杀掉最需要「不中断」保障的实例（M1）。放在重试轮开头而非退避
        // sleep 之后：退避等待期间的实例也视为活跃。
        state.last_activity_secs.store(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            std::sync::atomic::Ordering::Relaxed,
        );
        state
            .stats
            .retries_total
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let delay = retry::delay_for_attempt(attempt - 1, max_backoff);
        if !delay.is_zero() {
            tracing::warn!(attempt, delay_ms = delay.as_millis() as u64, "重试延迟");
            tokio::time::sleep(delay).await;
        } else {
            tracing::warn!(attempt, "立即重试");
        }

        let result = forward_once(
            &state,
            &method,
            &target_url,
            &headers,
            &req_body,
            max_spool_bytes,
            None,
        )
        .await;

        match result {
            ForwardResult::NetworkError(e) => {
                state.note_upstream_failure(&format!("网络错误: {e}"));
                tracing::warn!(attempt, error = %e, "上游网络错误，重试");
                continue;
            }
            ForwardResult::TooLarge => {
                state.note_upstream_failure("上游响应体超出 spool 上限");
                tracing::error!(attempt, "上游响应体超出 spool 上限，终止重试");
                return too_large_response();
            }
            ForwardResult::SpoolFailed(e) => {
                state.note_upstream_failure(&format!("本地 spool 故障: {e}"));
                tracing::error!(attempt, error = %e, "本地 spool 故障，终止重试");
                return spool_failed_response(&e);
            }
            ForwardResult::Response {
                status,
                headers: resp_headers,
                raw_headers,
                body,
                disk_scan,
            } => {
                if !needs_retry_response(attempt, &status, &raw_headers, &body, &disk_scan) {
                    // 成功：交给响应转换器后保真回放（下方 bounded 透传分支是错误
                    // 响应，不经 format——用户已定）
                    return replay_success(
                        &state,
                        &target_url,
                        &ctx,
                        attempt,
                        status,
                        resp_headers,
                        &raw_headers,
                        body,
                    )
                    .await;
                }
                // 受限重试路径：有响应的失败达到尝试上限后不再重试，把最后
                // 一次失败响应原样透传——客户端拿到真实 404 自行处理，好过
                // 永远等不到终态（compact 事故）。仅封顶「上游有响应」的失败：
                // 网络错误是真正的瞬时类，仍无限重试，且它没有响应可供回放。
                if bounded_retry && attempt >= retry::BOUNDED_RETRY_MAX_ATTEMPTS {
                    tracing::warn!(
                        attempt,
                        status = %status,
                        "受限重试路径达到尝试上限，透传最后一次上游响应"
                    );
                    state.note_upstream_failure(&format!(
                        "上游返回 {status}（受限重试路径，达上限透传）"
                    ));
                    return replay_failure_as_is(status, resp_headers, &raw_headers, body).await;
                }
                state.note_upstream_failure(&format!("上游返回 {status}（错误内容，重试）"));
                // 进入下一轮时 body Drop：磁盘临时文件删除
            }
        }
    }
}

/// 「只收到心跳的等待中断开」提示的下限：客户端在已提交、但还没收到任何上游
/// 真实字节的响应上等了至少这么久才断开，多半是它自己的流空闲超时到点——不少
/// agent 客户端按 SSE **事件**计时，而注释心跳在 SSE 解析层就被丢弃、重置不了它
/// （Claude Code 600s、Codex 300s、Qwen Code 240s，前两者黑盒实测）。下限只为
/// 滤掉开头几十秒内的主动取消；已知的事件级超时都远高于它，各客户端到点断开时
/// 都能提示到，而不像旧阈值（590s）那样只覆盖 Claude Code 一家。
const HEARTBEAT_ONLY_HINT_MIN: Duration = Duration::from_secs(60);

/// 保活通道的响应出口——「提交点」状态机的两个状态：
/// 尚未向客户端写出任何字节（Pending）→ 响应头已发出（Committed）。
/// 提交只发生一次、不可撤回：之后 status 与响应头再也改不了，只能往 SSE 体里写。
enum KeepaliveSink {
    Pending {
        /// handler 正 await 这个 oneshot 等 `Response`。Option 只为提交时能把
        /// Sender 取走（取走后立刻转为 Committed；send 失败 = 客户端已断开）
        resp_tx: Option<tokio::sync::oneshot::Sender<Response>>,
        /// 有个 tick 本该提交骨架、却因当次尝试收到「2xx 但非 SSE」的响应头而
        /// 暂停跳过了（见 `drive`）。暂停只限那一次尝试：它若需要重试，下一段
        /// 驱动一开始就补提交，绝不把「首字节 ≤ 一个保活间隔」再往后拖一整拍
        skeleton_due: bool,
        /// 心跳来源，提交时带进 Committed
        beat: Heartbeat,
    },
    Committed {
        tx: tokio::sync::mpsc::Sender<Result<Bytes, std::io::Error>>,
        /// 客户端断开信号：哨兵（`ClientGoneGuard`）随响应 Body 被 hyper drop 时置位
        gone_rx: tokio::sync::watch::Receiver<bool>,
        /// 提交时刻：断开提示的计时起点（客户端从这一刻起开始等事件）
        at: tokio::time::Instant,
        /// 至今只发过响应头与心跳、还没写出任何上游真实字节（回放 / 终态 error
        /// 事件都算真实字节）。只有这种等待中的断开才可能是客户端的流空闲超时
        /// 到点，断开提示据此判定
        heartbeats_only: bool,
        beat: Heartbeat,
        /// 「真实字节已开始写出」闸门：动态心跳在独立任务里写通道，必须与回放
        /// 互斥，否则心跳可能插进回放的事件中间。`send` 写第一块真实字节前在
        /// 锁内置位；心跳任务在锁内确认未置位才写（try_send 不阻塞，持锁极短）
        replay_gate: Arc<std::sync::Mutex<bool>>,
    },
}

/// 心跳的来源：config 的固定字节（keepalive_heartbeat 生效值），以及配置了
/// heartbeat_transform、且请求转换已完成时由心跳转换器逐拍生成的动态心跳。
#[derive(Clone)]
struct Heartbeat {
    fixed: Bytes,
    dynamic: Option<Arc<DynamicHeartbeat>>,
}

/// 一个请求的动态心跳生成器（在各拍的独立任务间共享）。
struct DynamicHeartbeat {
    pool: Arc<crate::transform::TransformPool>,
    ctx: ExchangeCtx,
    source: HeartbeatSource,
    /// 已发起的心跳调用次数（信封 heartbeat.seq）
    seq: std::sync::atomic::AtomicU64,
    /// 当前第几次上游尝试，重试循环每轮更新
    attempt: std::sync::atomic::AtomicU32,
    /// 有一次调用还没回来：这一拍不再排队调用，直接发固定心跳
    in_flight: std::sync::atomic::AtomicBool,
    /// 本请求已就心跳转换失败 warn 过（只 warn 一次，免得每拍刷屏）
    warned: std::sync::atomic::AtomicBool,
}

/// 在独立任务里跑完一次心跳转换并写出结果。不在 drive 的 select 里直接
/// await：drive 一返回就会 drop 在途的转换，persistent worker 在往返中途被
/// drop 会被剔除（进程被杀），每拍都可能发生。
fn spawn_dynamic_heartbeat(
    d: Arc<DynamicHeartbeat>,
    tx: tokio::sync::mpsc::Sender<Result<Bytes, std::io::Error>>,
    gate: Arc<std::sync::Mutex<bool>>,
    fixed: Bytes,
    elapsed: Duration,
) {
    tokio::spawn(async move {
        let info = aproxy_envelope::HeartbeatInfo {
            seq: d.seq.fetch_add(1, AtomicOrdering::Relaxed) + 1,
            elapsed_ms: elapsed.as_millis() as u64,
            attempt: d.attempt.load(AtomicOrdering::Relaxed),
        };
        let bytes =
            match crate::transform::transform_heartbeat(&d.pool, &d.ctx, &d.source, info).await {
                Ok(b) => b,
                Err(why) => {
                    if !d.warned.swap(true, AtomicOrdering::Relaxed) {
                        tracing::warn!(
                            request_id = %d.ctx.request_id,
                            error = %why,
                            "心跳转换失败，这一拍改发固定心跳（本请求之后的失败不再重复告警）"
                        );
                    }
                    Some(fixed)
                }
            };
        if let Some(bytes) = bytes {
            let replaying = gate
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !*replaying {
                let _ = tx.try_send(Ok(bytes));
            }
        }
        d.in_flight.store(false, AtomicOrdering::Release);
    });
}

impl KeepaliveSink {
    fn is_committed(&self) -> bool {
        matches!(self, Self::Committed { .. })
    }

    /// 客户端断开时完成。未提交：hyper 因断开 drop 了 handler future，oneshot
    /// 接收端随之销毁；已提交：响应 Body 被 drop，哨兵置位（或发送端已随哨兵
    /// 销毁——`wait_for` 返回 Err，同样视为断开）。
    async fn client_gone(&mut self) {
        match self {
            Self::Pending {
                resp_tx: Some(tx), ..
            } => tx.closed().await,
            // Sender 已被取走只发生在提交失败的瞬间，调用方随即返回，不会再等
            Self::Pending { resp_tx: None, .. } => std::future::pending().await,
            Self::Committed { gone_rx, .. } => {
                let _ = gone_rx.wait_for(|gone| *gone).await;
            }
        }
    }

    /// 提交响应头：把以 mpsc 接收端为体的流式响应交给 handler 返回给 hyper。
    /// 返回 false = 客户端已断开（handler 已被 drop，没人接收这个 Response）。
    /// 已提交时调用是空操作（返回 true）。
    fn commit(&mut self, status: StatusCode, headers: HeaderMap) -> bool {
        let Self::Pending {
            resp_tx: slot,
            beat,
            ..
        } = self
        else {
            return true;
        };
        let beat = beat.clone();
        let Some(resp_tx) = slot.take() else {
            return false;
        };
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(32);
        let (gone_tx, gone_rx) = tokio::sync::watch::channel(false);
        let gone_guard = ClientGoneGuard { tx: gone_tx };
        // 哨兵 move 进 map 闭包：Body 被客户端断开而 drop 时闭包销毁 → 置位断开信号
        let stream = tokio_stream::wrappers::ReceiverStream::new(rx).map(move |chunk| {
            let _ = &gone_guard;
            chunk
        });
        let mut resp = Response::builder().status(status);
        for (name, value) in headers.iter() {
            resp = resp.header(name, value);
        }
        // header 逐条来自已校验的 HeaderMap，body() 的错误态不可能出现
        let resp = resp
            .body(Body::from_stream(stream))
            .unwrap()
            .into_response();
        if resp_tx.send(resp).is_err() {
            return false;
        }
        *self = Self::Committed {
            tx,
            gone_rx,
            at: tokio::time::Instant::now(),
            heartbeats_only: true,
            beat,
            replay_gate: Arc::default(),
        };
        true
    }

    /// 提交骨架头（200 + text/event-stream，不设 connection 头——hyper 按协议
    /// 自动管理）并立刻发首个心跳：客户端的空闲计时从收到首批字节起算。
    /// false = 客户端已断开。
    fn commit_skeleton(&mut self) -> bool {
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::CONTENT_TYPE,
            HeaderValue::from_static("text/event-stream"),
        );
        headers.insert(
            http::header::CACHE_CONTROL,
            HeaderValue::from_static("no-cache"),
        );
        self.commit(StatusCode::OK, headers) && self.heartbeat()
    }

    /// 发一个心跳。用 try_send 而非 send：通道满说明客户端还有没读走的字节，
    /// 这一拍可以省；而在这里阻塞会连带停住对上游响应的读取。配置了心跳转换器
    /// 且上一次调用已回来时，这一拍交给它生成（独立任务，结果稍后写出）；否则
    /// 发固定心跳。false = 客户端已断开（接收端随响应 Body 销毁）。
    fn heartbeat(&self) -> bool {
        match self {
            Self::Committed {
                tx,
                beat,
                at,
                replay_gate,
                ..
            } => {
                if tx.is_closed() {
                    return false;
                }
                if let Some(d) = &beat.dynamic
                    && !d.in_flight.swap(true, AtomicOrdering::AcqRel)
                {
                    spawn_dynamic_heartbeat(
                        d.clone(),
                        tx.clone(),
                        replay_gate.clone(),
                        beat.fixed.clone(),
                        at.elapsed(),
                    );
                    return true;
                }
                !matches!(
                    tx.try_send(Ok(beat.fixed.clone())),
                    Err(tokio::sync::mpsc::error::TrySendError::Closed(_))
                )
            }
            Self::Pending { .. } => true,
        }
    }

    /// 请求转换完成后启用动态心跳（之前各拍不知道最终请求，只发固定心跳）
    fn arm_dynamic_heartbeat(&mut self, d: Arc<DynamicHeartbeat>) {
        match self {
            Self::Pending { beat, .. } | Self::Committed { beat, .. } => beat.dynamic = Some(d),
        }
    }

    /// 告诉动态心跳当前是第几次上游尝试
    fn note_attempt(&self, attempt: u32) {
        let (Self::Pending { beat, .. } | Self::Committed { beat, .. }) = self;
        if let Some(d) = &beat.dynamic {
            d.attempt.store(attempt, AtomicOrdering::Relaxed);
        }
    }

    /// 往已提交的响应体里写一块（成功回放 / 终态 error 事件）。
    /// false = 客户端已断开（或尚未提交——调用方保证只在提交后调用）。
    async fn send(&mut self, chunk: Result<Bytes, std::io::Error>) -> bool {
        match self {
            Self::Committed {
                tx,
                heartbeats_only,
                replay_gate,
                ..
            } => {
                if *heartbeats_only {
                    // 第一块真实字节：先关闸，之后到达的动态心跳一律丢弃
                    *replay_gate
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
                }
                *heartbeats_only = false;
                tx.send(chunk).await.is_ok()
            }
            Self::Pending { .. } => false,
        }
    }

    /// 记下「有个 tick 因暂停而跳过了骨架提交」（已提交时无意义，忽略）
    fn mark_skeleton_due(&mut self) {
        if let Self::Pending { skeleton_due, .. } = self {
            *skeleton_due = true;
        }
    }

    /// 取走「待补提交骨架」标记
    fn take_skeleton_due(&mut self) -> bool {
        match self {
            Self::Pending { skeleton_due, .. } => std::mem::take(skeleton_due),
            Self::Committed { .. } => false,
        }
    }

    /// 尚未提交时交出完整响应（首轮快速路径 / 502 终态）。客户端已断开时
    /// send 失败，响应随之丢弃。
    fn respond(self, resp: Response) {
        if let Self::Pending {
            resp_tx: Some(tx), ..
        } = self
        {
            let _ = tx.send(resp);
        }
    }

    /// 客户端断开的日志（含流空闲超时提示，见 `log_client_gone_after`）
    fn log_client_gone(&self, during: &str) {
        let heartbeat_only_wait = match self {
            Self::Committed {
                at,
                heartbeats_only: true,
                ..
            } => Some(at.elapsed()),
            _ => None,
        };
        log_client_gone_after(during, heartbeat_only_wait);
    }
}

/// 客户端断开的日志。`heartbeat_only_wait` = 客户端在「只收到心跳」的状态下
/// 等了多久（None = 尚未提交，或已经开始收到上游真实字节）；等了至少
/// `HEARTBEAT_ONLY_HINT_MIN` 时追加一条 warn，提示检查客户端的流空闲超时。
/// 只是日志文案：行为对任何客户端都一样（断开即中止上游），不点名、不按客户端
/// 特判——各客户端的具体写法放在用户文档里；等待秒数一并打出，便于用户对照
/// 自己客户端的超时取值。
fn log_client_gone_after(during: &str, heartbeat_only_wait: Option<Duration>) {
    tracing::info!(during, "客户端已断开，中止上游请求（保活通道）");
    if let Some(waited) = heartbeat_only_wait
        && waited >= HEARTBEAT_ONLY_HINT_MIN
    {
        tracing::warn!(
            waited_secs = waited.as_secs(),
            "客户端在只收到心跳的等待中断开（已等约 {} 秒）。若不是主动取消，多半是客户端的流空闲超时到点：\
             不少客户端按 SSE 事件计时，aProxy 的注释心跳不算事件、重置不了它，超过该超时的重试期或长生成\
             都会被客户端断开重发。请调大或关闭客户端的流空闲超时，常见客户端的写法见 README「接入 agent 客户端」",
            waited.as_secs()
        );
    }
}

/// 响应头的 content-type 是否为 SSE（`text/event-stream`）——保活心跳（SSE 注释）
/// 只在 SSE 里合法，提交点的各条规则都以此为界
fn is_event_stream(headers: &HeaderMap) -> bool {
    headers
        .get(http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.to_ascii_lowercase().contains("text/event-stream"))
}

/// 响应声明的、回放前必须解掉的编码；无该头或只有 identity 层 → None
fn encoding_to_undo(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(http::header::CONTENT_ENCODING)?;
    // 非 ASCII 的头值原样交给严格解码器，由它按「不支持的编码」报错
    let value = raw.to_str().map_or_else(
        |_| String::from_utf8_lossy(raw.as_bytes()).to_string(),
        str::to_string,
    );
    value
        .split(',')
        .map(str::trim)
        .any(|layer| !layer.is_empty() && !layer.eq_ignore_ascii_case("identity"))
        .then_some(value)
}

/// 上游响应头能否原样提交给客户端（保活通道尚未提交时）：2xx、
/// `text/event-stream`、且未经压缩。未压缩是硬条件——提交后要往体里插
/// `: keepalive` 注释，压缩流里插入明文会让客户端解压失败。保活适用的请求已
/// 要求上游以 identity 回应，这里兜住无视该要求仍压缩的上游（不提交真实头，
/// 等间隔到点提交骨架；成功体回放前再完整解码，见 `decode_for_committed_replay`）。
fn head_is_committable(status: StatusCode, headers: &HeaderMap) -> bool {
    status.is_success() && is_event_stream(headers) && encoding_to_undo(headers).is_none()
}

/// 在保活通道里驱动一个 future（一次上游尝试 / 一段退避等待）直到完成，
/// 与心跳节拍、客户端断开信号一起 select——这正是「心跳全程覆盖」的实现点：
/// 上游请求进行中（等响应头、缓冲上游流）与退避期间同样按间隔发心跳。
///
/// - 开始时：上一次尝试因暂停而跳过了骨架提交（`skeleton_due`）→ 立即补提交
/// - 每个 tick：已提交 → 发一个心跳；尚未提交 → 提交骨架头（一个间隔内上游没
///   给出可提交的结果）——除非本次尝试处于暂停中，此时只记下 `skeleton_due`
/// - `head_rx`（尚未提交时每次尝试都传入）收到上游响应头：
///   - 2xx 但不是 text/event-stream（stream:true 的 NDJSON、或上游无视 stream
///     回了普通 JSON）→ **本次尝试进行期间**暂停骨架提交：骨架是 SSE、心跳是
///     SSE 注释，提交了就把非 SSE 的流改坏（content-type 被改写、正文混入
///     `: keepalive` 行）。本次尝试成功 → 不提交，走保真快速路径原样回放；
///     需要重试（如 200 + error JSON）→ 暂停随本次尝试结束，下一段驱动开头
///     补提交骨架，之后照常心跳。取舍：上游先回 2xx 非 SSE 头、再迟迟不发体
///     时，客户端在本次尝试期间收不到任何字节，最长约 read_timeout_secs
///     （默认 300s，两次读到数据的间隔上限）——非 SSE 的流本就没有可用的
///     保活手段，这与不保活路径的等待特性一致
///   - 可直接提交（2xx + 未压缩 SSE，且 `real_head_allowed`：未配置
///     response_transform）→ 立即提交上游真实 status 与响应头
/// - 客户端断开或心跳发送失败 → 返回 None：`fut` 随本函数返回被 drop，在途的
///   reqwest 连接关闭，上游停止生成（计费保护）
async fn drive<F: std::future::Future>(
    sink: &mut KeepaliveSink,
    ticker: &mut tokio::time::Interval,
    fut: F,
    mut head_rx: Option<tokio::sync::oneshot::Receiver<(StatusCode, HeaderMap)>>,
    real_head_allowed: bool,
    during: &str,
) -> Option<F::Output> {
    if sink.take_skeleton_due() {
        tracing::info!("上一次尝试暂停期间已到保活间隔，补提交骨架头（保活通道）");
        if !sink.commit_skeleton() {
            sink.log_client_gone(during);
            return None;
        }
    }
    let mut skeleton_paused = false;
    tokio::pin!(fut);
    loop {
        tokio::select! {
            // 结果优先：同一轮里 fut 已完成就不再因并发就绪的 tick/头部通知先行
            // 提交——未提交时尝试一完成即走保真快速路径
            biased;
            out = &mut fut => return Some(out),
            () = sink.client_gone() => {
                sink.log_client_gone(during);
                return None;
            }
            head = async { head_rx.as_mut().expect("前置条件保证 Some").await }, if head_rx.is_some() => {
                // 每次尝试只通知一次（Err = forward_once 没拿到响应头就结束了）
                head_rx = None;
                if let Ok((status, headers)) = head
                    && !sink.is_committed()
                {
                    if status.is_success() && !is_event_stream(&headers) {
                        skeleton_paused = true;
                        tracing::info!(status = %status, "上游回了 2xx 非 SSE 响应头：本次尝试期间暂停提交骨架头（保活通道）");
                    } else if real_head_allowed && head_is_committable(status, &headers) {
                        if !sink.commit(status, headers) {
                            sink.log_client_gone(during);
                            return None;
                        }
                        tracing::info!(status = %status, "上游 SSE 响应头已到，先行提交给客户端（保活通道）");
                    }
                }
            }
            _ = ticker.tick() => {
                let alive = if sink.is_committed() {
                    sink.heartbeat()
                } else if skeleton_paused {
                    sink.mark_skeleton_due();
                    true
                } else {
                    tracing::info!("一个保活间隔内上游未给出可提交的结果，先提交骨架头（保活通道）");
                    sink.commit_skeleton()
                };
                if !alive {
                    sink.log_client_gone(during);
                    return None;
                }
            }
        }
    }
}

/// 同步溢写写入器（在阻塞线程里写解码结果）：先写内存，超 `RESIDENT_LIMIT` 且
/// 有 spool 目录时整体迁到磁盘临时文件——与响应 spool 同一阈值与命名；中途
/// 失败或被放弃时由 `SpoolPath` 删除半截文件。
enum SpillWriter {
    Memory {
        buf: Vec<u8>,
        dir: Option<PathBuf>,
    },
    Disk {
        // 字段顺序即 drop 顺序：先关句柄，再由 SpoolPath 删文件
        file: std::fs::File,
        path: SpoolPath,
        len: u64,
    },
}

impl std::io::Write for SpillWriter {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Memory { buf, dir } => match dir {
                Some(dir) if buf.len() + data.len() > RESIDENT_LIMIT => {
                    let path = SpoolPath(dir.join(format!(
                        "spool-{}-{}.spooltmp",
                        std::process::id(),
                        unique_seq()
                    )));
                    let mut file = std::fs::File::create(&path.0)?;
                    file.write_all(buf)?;
                    file.write_all(data)?;
                    let len = (buf.len() + data.len()) as u64;
                    *self = Self::Disk { file, path, len };
                }
                _ => buf.extend_from_slice(data),
            },
            Self::Disk { file, len, .. } => {
                file.write_all(data)?;
                *len += data.len() as u64;
            }
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Memory { .. } => Ok(()),
            Self::Disk { file, .. } => file.flush(),
        }
    }
}

impl SpillWriter {
    fn into_body(self) -> std::io::Result<SpooledBody> {
        use std::io::Write as _;
        Ok(match self {
            Self::Memory { buf, .. } => SpooledBody::Memory(Bytes::from(buf)),
            Self::Disk {
                mut file,
                path,
                len,
            } => {
                file.flush()?;
                drop(file);
                SpooledBody::Disk {
                    path: path.into_path(),
                    len,
                }
            }
        })
    }
}

/// 已提交响应的回放体解码：已提交的响应头里没有 content-encoding（骨架头不带；
/// 上游真实头只在未压缩时才提交），而上游无视 `accept-encoding: identity` 仍回了
/// 压缩体——原样写进去客户端必然解不开，只能**完整、严格**解码后回放
/// （`decode::decode_strict`：截断/损坏/未知编码一律失败，绝不回放半截内容）。
///
/// 在阻塞线程池里流式解码：输入是内存 Bytes 或 spool 文件，输出按
/// `SpillWriter` 内存/磁盘双模落地，内存占用与 body 大小无关；解码输出以
/// spool 上限封顶（回放体与缓冲体同一量级约束，也挡住解压炸弹）。原压缩体
/// 随本函数结束 drop（磁盘文件随之删除）。
async fn decode_for_committed_replay(
    body: SpooledBody,
    encoding: String,
    spool_dir: Option<PathBuf>,
    max_output: usize,
) -> Result<SpooledBody, String> {
    tokio::task::spawn_blocking(move || {
        let mut out = SpillWriter::Memory {
            buf: Vec::new(),
            dir: spool_dir,
        };
        let limit = max_output as u64;
        match &body {
            SpooledBody::Memory(b) => {
                crate::decode::decode_strict(&encoding, &b[..], &mut out, limit)?
            }
            SpooledBody::Disk { path, .. } => {
                let file = std::fs::File::open(path)
                    .map_err(|e| format!("读取 spool 临时文件失败: {e}"))?;
                crate::decode::decode_strict(&encoding, file, &mut out, limit)?
            }
        };
        out.into_body()
            .map_err(|e| format!("解码结果写入 spool 失败: {e}"))
    })
    .await
    .map_err(|e| format!("解码任务异常结束: {e}"))?
}

/// 失败响应的原样回放（受限重试路径达上限、且尚未向客户端提交任何字节时）：
/// 不经响应转换器（错误响应不进 format——用户已定），按原始头判定流式与否后
/// 保真回放上游 status/头/体。
async fn replay_failure_as_is(
    status: StatusCode,
    resp_headers: HeaderMap,
    raw_headers: &reqwest::header::HeaderMap,
    body: SpooledBody,
) -> Response {
    let is_streaming = match &body {
        SpooledBody::Memory(_) => retry::is_streaming_response(raw_headers, body.memory_bytes()),
        SpooledBody::Disk { .. } => retry::is_streaming_content_type(raw_headers),
    };
    build_replay_response(status, resp_headers, raw_headers, body, is_streaming).await
}

/// 保活通道：「保活适用」的请求（判定见 `keepalive_triggered`）从首轮起都走
/// 这里。后台任务驱动全部尝试，handler 只等一个 oneshot 交出 `Response`。
///
/// 提交点（`KeepaliveSink`）只由时间与上游响应头推动，「需要重试」本身不触发
/// 提交——重试几次后很快成功的请求，客户端照样拿到上游真实的 status 与头：
/// - **未提交**（每一轮尝试、每一段退避都适用）：
///   - 上游回 2xx + 未压缩 text/event-stream 头，且未配置 response_transform
///     （format 可能改写 status/头，不能先发）→ 立即提交上游真实 status 与响应头
///     （content-length 与 hop-by-hop 已由 forward_once 滤掉）
///   - 上游回 2xx 但非 SSE 的头 → 本次尝试期间暂停骨架提交（详见 `drive`）
///   - 一个 keepalive 间隔到点仍无可提交的结果 → 提交骨架头（200 + SSE）；
///     首字节因此始终 ≤ 一个保活间隔（暂停中的那次尝试除外）
///   - 某次尝试成功 → 保真快速路径（与不适用保活的请求同一出口），不提交
///   - 受限重试路径达上限 → 与不保活路径一致，原样回放最后一次失败响应
/// - **已提交**：此后的尝试与退避都经 `drive` 驱动、按间隔发心跳；完整缓冲并
///   判定无误后，把成功那一次的字节写进同一个响应（response_transform 只有
///   body 转换生效；上游无视 identity 的压缩体先完整解码）。TooLarge /
///   SpoolFailed / 解码失败 / 受限重试路径达上限以终态 SSE error 事件收场——
///   状态行已发出、不可再改，静默结束与「上游成功返回空 body」在客户端视角
///   不可区分。
///
/// 客户端断开在任一阶段（等响应头、缓冲、退避、回放）都立即中止上游请求、
/// 不再发起新请求：前三者经 `drive` 返回 None，回放经 send 失败。
async fn proxy_with_keepalive(
    state: AppState,
    req: OutboundRequest,
    path_and_query: String,
    max_spool_bytes: usize,
    bounded_retry: bool,
) -> Response {
    let (resp_tx, resp_rx) = tokio::sync::oneshot::channel::<Response>();
    tokio::spawn(async move {
        let keepalive_dur = state.config.keepalive_interval();
        let max_backoff = state.config.max_retry_backoff_secs;
        let mut sink = KeepaliveSink::Pending {
            resp_tx: Some(resp_tx),
            skeleton_due: false,
            beat: Heartbeat {
                fixed: Bytes::copy_from_slice(state.config.keepalive_heartbeat()),
                dynamic: None,
            },
        };
        // 整个请求共用一个节拍，首个 tick 在请求开始一个间隔之后：它既是「一个
        // 间隔内仍无可提交结果就提交骨架」的计时器，也是提交后的心跳节拍。
        // Delay：被长回放/慢客户端耽搁后不补发积压的 tick
        let mut ticker =
            tokio::time::interval_at(tokio::time::Instant::now() + keepalive_dur, keepalive_dur);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let real_head_allowed = state.response_pool.is_none();

        // 请求转换同样经 drive 驱动：一个间隔内没转完就先提交骨架头、此后照常
        // 心跳。失败时尚未提交 → 与不保活路径一致回 502；已提交 → 终态 error 事件
        let transformed = drive(
            &mut sink,
            &mut ticker,
            apply_request_transform(&state, req),
            None,
            false,
            "请求转换",
        )
        .await;
        let OutboundRequest {
            method,
            target_url,
            mut headers,
            body: req_body,
            ctx,
        } = match transformed {
            None => return,
            Some(Ok(req)) => req,
            Some(Err(msg)) => {
                if sink.is_committed() {
                    sink.send(Ok(terminal_error_event("proxy_transform_failed", &msg)))
                        .await;
                } else {
                    sink.respond(request_transform_failed_response(&msg));
                }
                return;
            }
        };
        // 保活通道会往响应体里插 `: keepalive` 注释——只有未压缩的体才能这样
        // 插入（往 gzip/br 流里插明文，客户端解压必坏）。故对保活适用的请求
        // 要求上游以 identity 编码回应：客户端照旧拿到合法响应（未压缩永远是
        // 可接受的编码），代价只是上游到本机这一段多传一些字节。放在请求转换
        // 之后，覆盖 format 可能写入的同名头（override_headers 里配的同名头也被
        // 覆盖，启动时 server.rs 会 warn 一次）。
        headers.insert(
            http::header::ACCEPT_ENCODING,
            HeaderValue::from_static("identity"),
        );
        log_proxied_request(&method, &path_and_query, &target_url);
        if let Some(pool) = &state.heartbeat_pool {
            sink.arm_dynamic_heartbeat(Arc::new(DynamicHeartbeat {
                pool: pool.clone(),
                ctx: ctx.clone(),
                source: HeartbeatSource::new(&method, &target_url, &headers, &req_body),
                seq: Default::default(),
                attempt: Default::default(),
                in_flight: Default::default(),
                warned: Default::default(),
            }));
        }

        let mut attempt: u32 = 0;
        loop {
            attempt += 1;
            sink.note_attempt(attempt);
            if attempt > 1 {
                // 与非保活通道同款：每轮重试刷新活动时间戳（语义 = 仍在处理中），
                // 防止无限重试中的实例被 stop idle 误判闲置强退（M1）
                state.last_activity_secs.store(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0),
                    std::sync::atomic::Ordering::Relaxed,
                );
                state
                    .stats
                    .retries_total
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let delay = retry::delay_for_attempt(attempt - 1, max_backoff);
                if delay.is_zero() {
                    tracing::warn!(attempt, "立即重试（保活通道）");
                } else {
                    tracing::warn!(
                        attempt,
                        delay_ms = delay.as_millis() as u64,
                        "重试延迟（保活通道）"
                    );
                    let backoff = tokio::time::sleep(delay);
                    if drive(&mut sink, &mut ticker, backoff, None, false, "退避等待")
                        .await
                        .is_none()
                    {
                        return;
                    }
                }
            }

            // 尚未提交时每次尝试都要响应头通知：可提交的上游真实头先行提交、
            // 「2xx 非 SSE」暂停骨架提交（见 drive）；已提交后不再需要
            let (head_tx, head_rx) = if sink.is_committed() {
                (None, None)
            } else {
                let (tx, rx) = tokio::sync::oneshot::channel();
                (Some(tx), Some(rx))
            };
            let attempt_fut = forward_once(
                &state,
                &method,
                &target_url,
                &headers,
                &req_body,
                max_spool_bytes,
                head_tx,
            );
            let Some(result) = drive(
                &mut sink,
                &mut ticker,
                attempt_fut,
                head_rx,
                real_head_allowed,
                "等待上游",
            )
            .await
            else {
                return;
            };

            match result {
                ForwardResult::NetworkError(e) => {
                    // 网络错误是真正的瞬时类：受限路径也不封顶，且它没有响应可供回放
                    state.note_upstream_failure(&format!("网络错误: {e}"));
                    tracing::warn!(attempt, error = %e, "上游网络错误，重试（保活通道）");
                }
                ForwardResult::TooLarge => {
                    state.note_upstream_failure("上游响应体超出 spool 上限");
                    tracing::error!(attempt, "上游响应体超出 spool 上限，终止重试（保活通道）");
                    if sink.is_committed() {
                        let err_event = Bytes::from_static(
                            b"event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"proxy_spool_limit\",\"message\":\"upstream response exceeded proxy spool limit\"}}\n\n",
                        );
                        sink.send(Ok(err_event)).await;
                    } else {
                        sink.respond(too_large_response());
                    }
                    return;
                }
                ForwardResult::SpoolFailed(e) => {
                    state.note_upstream_failure(&format!("本地 spool 故障: {e}"));
                    tracing::error!(attempt, error = %e, "本地 spool 故障，终止重试（保活通道）");
                    if sink.is_committed() {
                        let err_event = Bytes::from_static(
                            b"event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"proxy_spool_failed\",\"message\":\"local disk cache write failed\"}}\n\n",
                        );
                        sink.send(Ok(err_event)).await;
                    } else {
                        sink.respond(spool_failed_response(&e));
                    }
                    return;
                }
                ForwardResult::Response {
                    status,
                    headers: resp_headers,
                    raw_headers,
                    body,
                    disk_scan,
                } => {
                    let upstream_sse = is_event_stream(&resp_headers);
                    if !needs_retry_response(attempt, &status, &raw_headers, &body, &disk_scan) {
                        // 响应转换（若配置）放在提交判定之前，并尽量经 drive 驱动：
                        // format 转换期间照常心跳、一个间隔到点仍未提交就先提交骨架头
                        // （timeout_secs = 0 时转换耗时没有上界，不能让客户端在这段
                        // 时间收不到任何字节）。例外是「尚未提交 + 上游回的是 2xx 非
                        // SSE」：骨架是 SSE、心跳是 SSE 注释，此时提交会把非 SSE 的
                        // 响应改坏（与 drive 的「2xx 非 SSE 暂停骨架提交」同一道理），
                        // 只能直接等转换完成。转换失败时透传原样（不发 error 事件——
                        // 那是响应不可用的终态模板；此处响应在手，仅转换失败）
                        let transform = transform_response_if_configured(
                            &state,
                            &target_url,
                            &ctx,
                            resp_headers,
                            body,
                        );
                        let transformed = if !sink.is_committed() && !upstream_sse {
                            Some(transform.await)
                        } else {
                            drive(&mut sink, &mut ticker, transform, None, false, "响应转换").await
                        };
                        let Some((resp_headers, body)) = transformed else {
                            return;
                        };
                        if !sink.is_committed() {
                            // 提交前就成功：保真快速路径（上游 status 与转换后的响应头）
                            let resp = replay_transformed(
                                attempt,
                                status,
                                resp_headers,
                                &raw_headers,
                                body,
                            )
                            .await;
                            sink.respond(resp);
                            return;
                        }
                        // 已提交：状态行与响应头不可再改——format 对 headers 的改写
                        // 无效，仅 body 转换生效
                        if !is_event_stream(&resp_headers) {
                            // 不撤回、照常回放：客户端已在等这个响应，丢掉成功结果
                            // 只会更糟。日志给出可行动的配置建议
                            tracing::warn!(
                                attempt,
                                content_type = %resp_headers
                                    .get(http::header::CONTENT_TYPE)
                                    .and_then(|v| v.to_str().ok())
                                    .unwrap_or("-"),
                                "已以 text/event-stream 提交的保活响应里回放的是非 SSE 的成功响应体（响应头已发出，无法撤回）；\
                                 若该实例的上游会对 stream:true 请求回非 SSE 的流（如 NDJSON）且常需重试，\
                                 建议为该实例设置 keepalive_trigger = \"accept\""
                            );
                        }
                        let mut body = match encoding_to_undo(&resp_headers) {
                            None => body,
                            Some(encoding) => {
                                // 解码大体可能耗时，同样在心跳节拍内进行
                                let decoded = drive(
                                    &mut sink,
                                    &mut ticker,
                                    decode_for_committed_replay(
                                        body,
                                        encoding.clone(),
                                        state.spool_dir.clone(),
                                        max_spool_bytes,
                                    ),
                                    None,
                                    false,
                                    "回放前解码",
                                )
                                .await;
                                let Some(decoded) = decoded else {
                                    return;
                                };
                                match decoded {
                                    Ok(decoded) => {
                                        tracing::info!(
                                            attempt,
                                            encoding = %encoding,
                                            "上游无视 identity 仍压缩了响应：已提交的响应头不含 content-encoding，回放前已完整解码（保活通道）"
                                        );
                                        decoded
                                    }
                                    Err(e) => {
                                        state.note_upstream_failure(&format!(
                                            "回放前解码上游压缩响应失败（{encoding}）: {e}"
                                        ));
                                        tracing::warn!(
                                            attempt,
                                            encoding = %encoding,
                                            error = %e,
                                            "已提交的响应无法回放上游压缩体且解码失败，以终态 error 事件收场（保活通道）"
                                        );
                                        let err_event = Bytes::from_static(
                                            b"event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"proxy_decode_failed\",\"message\":\"upstream response used a content-encoding the committed stream cannot carry, and decoding it failed\"}}\n\n",
                                        );
                                        sink.send(Ok(err_event)).await;
                                        return;
                                    }
                                }
                            }
                        };
                        tracing::info!(
                            attempt,
                            status = %status,
                            bytes = body.len(),
                            "上游成功，回放到已提交的响应（保活通道）"
                        );
                        // 空 body 直接结束流，不注入任何上游未发送的字节
                        if body.is_empty() {
                            return;
                        }
                        let mut stream = body.into_stream().await;
                        while let Some(item) = stream.next().await {
                            if !sink.send(item).await {
                                // 客户端在回放中断开：Drop 链负责删临时文件
                                sink.log_client_gone("回放");
                                return;
                            }
                        }
                        return;
                    }
                    // 受限重试路径：有响应的失败达到尝试上限后不再重试
                    if bounded_retry && attempt >= retry::BOUNDED_RETRY_MAX_ATTEMPTS {
                        tracing::warn!(
                            attempt,
                            status = %status,
                            "受限重试路径达到尝试上限，终止重试（保活通道）"
                        );
                        if !sink.is_committed() {
                            // 尚未向客户端写出任何字节：与不保活路径一致，原样
                            // 回放最后一次失败响应——客户端拿到真实 404 自行处理
                            state.note_upstream_failure(&format!(
                                "上游返回 {status}（受限重试路径，达上限透传）"
                            ));
                            sink.respond(
                                replay_failure_as_is(status, resp_headers, &raw_headers, body)
                                    .await,
                            );
                            return;
                        }
                        // 已提交：状态行已发出，真实 status/headers 无法回放，以终态
                        // error 事件收场——客户端明确感知代理放弃了这条请求，而非永远等
                        state.note_upstream_failure(&format!(
                            "上游返回 {status}（受限重试路径，达上限终止）"
                        ));
                        drop(body);
                        let err_event = Bytes::from(format!(
                            "event: error\ndata: {{\"type\":\"error\",\"error\":{{\"type\":\"upstream_error\",\"message\":\"upstream returned {status}; bounded retry path exhausted after {attempt} attempts\"}}}}\n\n"
                        ));
                        sink.send(Ok(err_event)).await;
                        return;
                    }
                    state.note_upstream_failure(&format!("上游返回 {status}（错误内容，重试）"));
                    // 进入下一轮时 body Drop：磁盘临时文件删除
                }
            }
            // 需要重试：不在这里提交——提交只由 tick（约一个保活间隔）与可提交的
            // 上游响应头推动，下一轮尝试很快成功时客户端仍拿到上游真实头
        }
    });
    // 客户端在提交前断开时 hyper 会 drop 本 future（连同 resp_rx），后台任务经
    // resp_tx.closed() 感知并中止上游请求。Err 只在后台任务未交出响应就结束时
    // 出现（panic）——给客户端一个明确的 502 而不是让连接悬空
    resp_rx.await.unwrap_or_else(|_| {
        tracing::error!("保活通道后台任务未交出响应即结束");
        (StatusCode::BAD_GATEWAY, "aProxy 内部错误：保活通道异常结束").into_response()
    })
}

/// 客户端断开哨兵：持有 watch 发送端，Drop 时置位断开信号。
/// 被 move 进 keepalive 响应 Body 的流闭包，Body 随客户端断开被 hyper drop 时触发。
struct ClientGoneGuard {
    tx: tokio::sync::watch::Sender<bool>,
}

impl Drop for ClientGoneGuard {
    fn drop(&mut self) {
        let _ = self.tx.send(true);
    }
}

// Response 变体比其他变体大（status + 双头表 + SpooledBody）：body 本身已是
// 引用计数 Bytes 或文件句柄，装箱只能省这点栈空间，而本枚举只在单一路径按值
// 传递一次，不构成热点。保持扁平匹配更直接。
#[allow(clippy::large_enum_variant)]
enum ForwardResult {
    NetworkError(String),
    /// 上游响应体超出 spool 上限：确定性失败，重试无意义，各通道直接终态处理。
    TooLarge,
    /// 本地磁盘 spool 写入失败（磁盘满/IO 故障）：磁盘模式专属的确定性失败。
    /// 与 NetworkError 不同——同一轮重试无法恢复（文件句柄已坏），但下一轮
    /// 可能恢复；交由调用方选择终态或继续。
    SpoolFailed(String),
    /// 完整缓冲后的响应；`raw_headers` 保留原始 reqwest 头用于流式嗅探，
    /// `headers` 为已过滤 hop-by-hop 后的待透传头。`disk_scan` 仅为磁盘模式
    /// 增量扫描结论（Some((是否错误, 头部快照)))；内存模式恒 None。
    Response {
        status: StatusCode,
        headers: HeaderMap,
        raw_headers: reqwest::header::HeaderMap,
        body: SpooledBody,
        disk_scan: Option<(bool, Vec<u8>)>,
    },
}

/// 响应体 spool 上限（字节）：配置 spool_limit_mb（MB，最小 1）换算，
/// 防止失控/恶意上游把内存打爆。超过上限的响应无法完整缓冲，直接按
/// TooLarge 终态处理。
fn spool_limit_bytes(config: &Config) -> usize {
    (config.spool_limit_mb.max(1) as usize).saturating_mul(1024 * 1024)
}

/// 一次上游尝试：发出请求、完整缓冲响应（内存/磁盘 spool）并做增量错误扫描。
///
/// `on_head`：拿到上游响应头（status + 已滤 hop-by-hop 的头）时立即通知调用方，
/// 早于缓冲响应体——保活通道据此在首轮把上游真实头先行提交给客户端。接收方已
/// 不关心（已提交、已结束）时发送失败，忽略即可。
async fn forward_once(
    state: &AppState,
    method: &http::Method,
    url: &str,
    headers: &HeaderMap,
    req_body: &RequestBody,
    max_spool_bytes: usize,
    on_head: Option<tokio::sync::oneshot::Sender<(StatusCode, HeaderMap)>>,
) -> ForwardResult {
    let spool_dir = state.spool_dir.as_deref();
    let reqwest_method =
        reqwest::Method::from_bytes(method.as_str().as_bytes()).unwrap_or(reqwest::Method::GET);

    let mut builder = state.client.request(reqwest_method, url);

    // 透传请求头（过滤 hop-by-hop），已在 proxy_handler 中应用了覆盖/追加
    for (name, value) in headers.iter() {
        let name_str = name.as_str();
        if is_hop_header(name_str) {
            continue;
        }
        if let Ok(n) = reqwest::header::HeaderName::from_bytes(name_str.as_bytes())
            && let Ok(v) = reqwest::header::HeaderValue::from_bytes(value.as_bytes())
        {
            builder = builder.header(n, v);
        }
    }

    // 请求体字节源：内存零拷贝 clone / 磁盘流式重读。磁盘模式显式回填
    // content-length（hop 过滤剥掉了原头，而我们持有精确字节数，保真回放）。
    builder = match req_body.apply_to(builder).await {
        Ok(b) => b,
        Err(e) => {
            // 请求体临时文件打不开 = 本地确定性故障，重试同样打不开
            return ForwardResult::SpoolFailed(e.to_string());
        }
    };

    let resp = match builder.send().await {
        Ok(r) => r,
        // 在源头脱敏：NetworkError 的文本会流向日志、status 的最近错误与各通道
        Err(e) => return ForwardResult::NetworkError(redact_reqwest_error(e)),
    };

    let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let raw_headers = resp.headers().clone();

    // 过滤后的响应头（透传用）
    let mut resp_headers = HeaderMap::new();
    for (name, value) in resp.headers().iter() {
        let name_str = name.as_str();
        if is_hop_header(name_str) {
            continue;
        }
        if let Ok(n) = HeaderName::from_bytes(name_str.as_bytes())
            && let Ok(v) = HeaderValue::from_bytes(value.as_bytes())
        {
            // append 而非 insert：Set-Cookie 等同名多值头不能坍缩为最后一个
            resp_headers.append(n, v);
        }
    }
    if let Some(tx) = on_head {
        let _ = tx.send((status, resp_headers.clone()));
    }

    // 关键：完整 spool（分块累积）——任何流式中断都会在此处以 Err 形式暴露，从而触发重试；
    // 超出上限按 TooLarge 终态处理；磁盘写入失败按 SpoolFailed 处理
    let mut spool = SpoolBuffer::Memory(Vec::new());
    let mut scanner = StreamErrorScanner::new();
    let mut resp_stream = resp;
    loop {
        match resp_stream.chunk().await {
            Ok(Some(chunk)) => {
                // 各提前返回路径上 spool 随之 drop，半截磁盘文件由 SpoolPath 删除
                if spool.len() + chunk.len() as u64 > max_spool_bytes as u64 {
                    return ForwardResult::TooLarge;
                }
                scanner.feed(&chunk);
                spool_chunk(&mut spool, &chunk, spool_dir).await;
                if matches!(spool, SpoolBuffer::Poisoned) {
                    return ForwardResult::SpoolFailed("spool 磁盘写入失败".to_string());
                }
            }
            Ok(None) => break,
            Err(e) => {
                return ForwardResult::NetworkError(format!(
                    "读取上游响应体失败: {}",
                    redact_reqwest_error(e)
                ));
            }
        }
    }

    // 收尾 flush 失败与收集途中写失败同属本地磁盘故障，走同一条 SpoolFailed 路径
    let (body, disk_scan) = match spool.finish(scanner).await {
        Ok(v) => v,
        Err(e) => return ForwardResult::SpoolFailed(e),
    };
    ForwardResult::Response {
        status,
        headers: resp_headers,
        raw_headers,
        body,
        disk_scan,
    }
}

/// reqwest 错误的脱敏渲染：它的 Display 会把请求 URL 原样拼在末尾
/// （`error sending request for url (URL)`），URL 的查询串里可能带客户端的 key
/// （`?key=...`），而这段文本会进日志、status 的最近错误和 502 正文。先剥掉 URL
/// 再按原有形态附上脱敏版本——保留「for url (...)」便于排障时看出打到了哪里。
fn redact_reqwest_error(e: reqwest::Error) -> String {
    let url = e.url().map(|u| crate::config::mask_base_url(u.as_str()));
    let e = e.without_url();
    match url {
        Some(u) => format!("{e} for url ({u})"),
        None => e.to_string(),
    }
}

/// 上游目标 URL：base_url（归一时已去尾斜杠，此处再 trim 一次以容忍直接构造的
/// Config）+ 原样的 path/query。两条转发路径共用，避免拼接规则各写一遍而漂移。
fn upstream_url(config: &Config, uri: &http::Uri) -> String {
    let path_and_query = uri.path_and_query().map(|pq| pq.as_str()).unwrap_or("/");
    format!(
        "{}{}",
        config.base_url.trim_end_matches('/'),
        path_and_query
    )
}

/// 请求体上限的人类可读文案（0 = 不设限 → 「不设限」，否则「N MiB」）。
/// 缓冲路径与仅转发路径的 413 共用，保证两条路径对用户说同一句话。
fn limit_text(body_limit: usize) -> String {
    if body_limit == usize::MAX {
        "不设限".to_string()
    } else {
        format!("{} MiB", body_limit / 1024 / 1024)
    }
}

/// 请求体超限的 413 响应：判定、日志与文案收敛在一处，两条路径共用。
fn body_too_large_response(body_limit: usize) -> Response {
    let limit_text = limit_text(body_limit);
    tracing::warn!(limit = %limit_text, "请求体超出上限");
    (
        StatusCode::PAYLOAD_TOO_LARGE,
        format!("请求体超出上限（{limit_text}），可在 settings.json 的 max_body_mb 或 config.toml 的 max_body_mb 调整"),
    )
        .into_response()
}

/// 仅转发模式的核心路径：请求体边收边发上游、上游响应边收边回客户端，
/// **不缓冲、不落盘、不重试、不检查响应内容**。
///
/// 这是模式的定义而非实现偷懒：任何缓冲都会让内存占用随负载增长，任何重放都
/// 需要先持有完整 body，两者都与「真·增量流（首字节即转发）」互斥。因此这里
/// 刻意不进重试循环、SSE 保活骨架、错误内容拦截（`is_error_body` /
/// `is_stream_error_body`）、spool 与 `keepalive_trigger` 判定，并且**不做
/// decode 模块的解码**——本模式不对响应体做任何内容检查，没有解码的用武之地。
///
/// 本模式下不生效的配置项：`disk_cache`、`spool_limit_mb`、
/// `keepalive_interval_secs`、`max_retry_backoff_secs`；`max_body_mb` 仍强制。
async fn forward_only_proxy(
    state: AppState,
    method: http::Method,
    target_url: String,
    headers: HeaderMap,
    body: axum::body::Body,
) -> Response {
    let reqwest_method =
        reqwest::Method::from_bytes(method.as_str().as_bytes()).unwrap_or(reqwest::Method::GET);

    let mut builder = state.client.request(reqwest_method, &target_url);

    // 请求头透传：与 forward_once 相同的「过滤 hop-by-hop + 逐个 header」写法
    //（覆盖/追加已在 proxy_handler 中应用）
    for (name, value) in headers.iter() {
        let name_str = name.as_str();
        if is_hop_header(name_str) {
            continue;
        }
        if let Ok(n) = reqwest::header::HeaderName::from_bytes(name_str.as_bytes())
            && let Ok(v) = reqwest::header::HeaderValue::from_bytes(value.as_bytes())
        {
            builder = builder.header(n, v);
        }
    }

    // 请求体：一律流式（不做空 body 特判——空 body 的流只是立刻结束）。计数
    // 适配器边收边数，累计超过 max_body_mb 即产出 Err——Err 会让 reqwest 立刻
    // 中止上游请求（不再继续拉请求体），我们据此回 413。limit 为 usize::MAX
    //（max_body_mb=0）时永不触发，这是本模式唯一仍强制的限制。
    let limit = state.config.body_limit_bytes();
    let too_large = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = too_large.clone();
    // 客户端侧请求体中断标记：与 too_large 同款——适配器产出的 Err 会被 reqwest
    // 当作「body 出错」进而中止上游请求，`send()` 随之返回 Err，两种来源在下游
    // 长得一模一样，只能靠标记区分。客户端在上传途中断开（裸 TCP 声明
    // Content-Length 却提前 close、进程被杀……）必须与「上游请求失败」分开：
    // 后者会把排障矛头指向根本没有收到完整请求体的上游。缓冲路径的同一事件映射为
    // `ReadBodyError::Io` → 400 且**不碰** last_error（见 `read_request_body` 的
    // 错误分支），两条路径归因必须一致——仅转发模式此前是异常的那一侧。
    let client_gone = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let gone_flag = client_gone.clone();
    // unfold 状态 = (上游流, 已见字节数)；产出的就是同一份 chunk（移动，不拷贝、
    // 不累积），故在途内存与负载大小无关
    let counted = futures_util::stream::unfold(
        (body.into_data_stream(), 0usize),
        move |(mut stream, seen)| {
            let flag = flag.clone();
            let gone_flag = gone_flag.clone();
            async move {
                let chunk = stream.next().await?;
                let chunk = match chunk {
                    Ok(c) => c,
                    Err(e) => {
                        // 这里的 Err 只可能来自客户端侧（body 即客户端请求体）；
                        // 原样透传给 reqwest 以中止上游请求，真实原因由标记承载
                        gone_flag.store(true, AtomicOrdering::Relaxed);
                        return Some((Err(e), (stream, seen)));
                    }
                };
                let seen = seen.saturating_add(chunk.len());
                if seen > limit {
                    flag.store(true, AtomicOrdering::Relaxed);
                    // axum::Error 满足 wrap_stream 的错误约束（Into<BoxError>），
                    // 无需 map_err；真实原因由 too_large 标记承载
                    return Some((
                        Err(axum::Error::new(std::io::Error::other(
                            "请求体超出上限（仅转发模式流式中断）",
                        ))),
                        (stream, seen),
                    ));
                }
                Some((Ok(chunk), (stream, seen)))
            }
        },
    );
    // 以 chunked 发往上游：我们不再持有完整长度（长度要读完才知道，而读完即
    // 缓冲），content-length 无从回填——这是本模式的代价之一，也是必然后果。
    builder = builder.body(reqwest::Body::wrap_stream(counted));

    let resp = match builder.send().await {
        Ok(r) => r,
        Err(e) => {
            // 错误文本与目标 URL 都会进日志 / 最近错误 / 502 正文，一律先脱敏
            let e = redact_reqwest_error(e);
            let masked_target = crate::config::mask_base_url(&target_url);
            // send() 的失败有三种来源：上游真的失败，或请求体适配器产出 Err 令
            // reqwest 主动中止（超限 / 客户端上传中断，各有一个标记）。判定顺序
            // 是刻意的：超限排在最前——它同时也会让适配器产出 Err，而「你的请求体
            // 太大」比「你断开了」更接近事实、更可操作。两个标记都没置位才是真·上游失败。
            if too_large.load(AtomicOrdering::Relaxed) {
                return body_too_large_response(limit);
            }
            // 客户端上传途中断开：**不记 last_error、不打「上游请求失败」**——
            // 那一头根本没有失败，是我们应客户端断开而主动中止了上游请求。
            // 也不回 502：发起断开的客户端读不到响应，这里的状态码只是给日志与
            // 中间观测看的，取与缓冲路径同一事件一致的 400。
            if client_gone.load(AtomicOrdering::Relaxed) {
                tracing::error!(error = %e, target = %masked_target, "客户端请求体传输中断，已中止上游请求");
                return (
                    StatusCode::BAD_REQUEST,
                    "请求体读取失败（客户端在上传途中断开）",
                )
                    .into_response();
            }
            // 上游失败不重试（本模式的定义），但必须记进 note_upstream_failure，
            // 否则 status 的「最近错误」对这类实例永久显示「无」
            let reason = format!("上游请求失败: {e}");
            state.note_upstream_failure(&reason);
            tracing::warn!(error = %e, target = %masked_target, "上游请求失败（仅转发模式不重试）");
            return (
                StatusCode::BAD_GATEWAY,
                format!("{reason}（仅转发模式不重试）"),
            )
                .into_response();
        }
    };

    let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut resp_builder = Response::builder().status(status);
    // 上游是否用 transfer-encoding 分帧（判定取自**上游原始响应头**，而非我们
    // 正在拼的下游头表）：决定 content-length 能否保留，理由见 is_hop_response_header
    let upstream_has_te = resp.headers().contains_key(http::header::TRANSFER_ENCODING);
    // 状态码与响应头原样透传（过滤 hop-by-hop，常规情况下保留 content-length）。append
    // 而非 insert：Set-Cookie 等同名多值头不能坍缩为最后一个
    for (name, value) in resp.headers().iter() {
        let name_str = name.as_str();
        if is_hop_response_header(name_str, upstream_has_te) {
            continue;
        }
        if let Ok(n) = HeaderName::from_bytes(name_str.as_bytes())
            && let Ok(v) = HeaderValue::from_bytes(value.as_bytes())
        {
            resp_builder = resp_builder.header(n, v);
        }
    }

    // 响应流：直回客户端。成功块只累加已转发字节数（供中断日志定位断点），不
    // 复制、不缓存、不检查内容。客户端断开时本 Body 被 drop，reqwest 连接随之
    // 关闭、上游生成立即停止——既有的计费保护靠 Drop 天然成立，无需额外代码。
    //
    // 每转发一个 chunk 顺带刷新 last_activity_secs：本模式下小时级长流是常态
    //（这正是它存在的理由），而该时间戳的语义是「最近一次收到请求**或仍在处理**」
    //（与常规路径重试轮开头的刷新同义）。不刷新的话，一个正在传输的实例会被
    // stop idle / status --idle 判为闲置并强退——idle_timeout_secs 默认 1800s，
    // 任何长流都会越线，恰在传输中途被掐断。覆盖范围与残余窗口：chunk 持续到达
    // 即视为活跃；上游长时间一个字节都不发（真正的停滞，read_timeout 才是该管
    // 这件事的地方）期间不刷新——那时实例确实没在推进任何数据，被判闲置符合语义。
    // 代价是每 chunk 一次 SystemTime::now() + 一条 Relaxed store，相对该 chunk
    // 的网络读写开销可忽略。
    let forwarded = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let counter = forwarded.clone();
    let stream = resp.bytes_stream().map(move |chunk| match chunk {
        Ok(c) => {
            counter.fetch_add(c.len() as u64, AtomicOrdering::Relaxed);
            state.last_activity_secs.store(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0),
                AtomicOrdering::Relaxed,
            );
            Ok(c)
        }
        Err(e) => {
            // 上游响应流中断：**直接截断**——绝不注入任何上游未发出的字节（那
            // 是伪造数据），也不重试（本模式已放弃重试能力）。错误记入
            // note_upstream_failure 并带上已转发字节数，便于判断断在了哪里。
            let forwarded_bytes = counter.load(AtomicOrdering::Relaxed);
            let e = redact_reqwest_error(e);
            let reason = format!("上游响应流中断: {e}");
            state.note_upstream_failure(&reason);
            tracing::warn!(
                error = %e,
                forwarded_bytes,
                "上游响应流中断，连接就此截断（仅转发模式不重试）"
            );
            Err(std::io::Error::other(reason))
        }
    });

    // 与 build_replay_response 同款收尾：header 逐条经 from_bytes 校验过、status
    // 已降级兜底，body() 的错误态不可能出现（框架保证，不做不可达分支）
    resp_builder
        .body(Body::from_stream(stream))
        .unwrap()
        .into_response()
}

fn build_response(status: StatusCode, headers: HeaderMap, body: Bytes) -> Response {
    let mut resp = Response::builder().status(status);
    for (name, value) in headers.iter() {
        resp = resp.header(name, value);
    }
    resp.body(Body::from(body)).unwrap().into_response()
}

/// 统一回放入口：内存模式按流式判定选路（旧行为，保真优先）；磁盘模式一律
/// chunked 流式回放（回放即流式读临时文件），上游带 content-length 时回填
/// 保真（hop 过滤剥掉了它，而我们持有精确字节数）。
async fn build_replay_response(
    status: StatusCode,
    headers: HeaderMap,
    raw_headers: &reqwest::header::HeaderMap,
    mut body: SpooledBody,
    is_streaming: bool,
) -> Response {
    // 磁盘模式一律 chunked 流式回放（回放即流式读临时文件）；上游带
    // content-length 时回填保真（hop 过滤剥掉了它，我们持有精确字节数）
    if matches!(body, SpooledBody::Disk { .. }) {
        let replay_len = body.len();
        let mut resp = Response::builder().status(status);
        for (name, value) in headers.iter() {
            resp = resp.header(name, value);
        }
        if replay_len > 0 && raw_headers.get(http::header::CONTENT_LENGTH).is_some() {
            resp = resp.header(http::header::CONTENT_LENGTH, replay_len.to_string());
        }
        let stream = body.into_stream().await;
        resp.body(Body::from_stream(stream))
            .unwrap()
            .into_response()
    } else if is_streaming {
        // 内存流式：8 KiB 零拷贝分块 chunked 回放（与旧行为一致）
        let b = body.take_memory();
        build_stream_replay_response(status, headers, b)
    } else {
        // 内存非流式：Body::from 一次性回放（content-length 由 hyper 自动
        // 设置，不走 chunked——保真优先）
        let b = body.take_memory();
        build_response(status, headers, b)
    }
}

/// 流式回放：将已完整 spool 的 body 按块以 chunked 流式产出，字节与上游完全一致。
///
/// 逐块大小 8 KiB，对 SSE 而言任意切分均安全（解析器按行缓冲），且保证
/// `Transfer-Encoding: chunked`，客户端仍以流式增量消费，避免因一次性
/// `Body::from(bytes)` 被某些客户端误判为非流式而产生的潜在 bug。
fn build_stream_replay_response(status: StatusCode, headers: HeaderMap, body: Bytes) -> Response {
    let mut resp = Response::builder().status(status);
    for (name, value) in headers.iter() {
        resp = resp.header(name, value);
    }

    if body.is_empty() {
        return resp.body(Body::empty()).unwrap().into_response();
    }

    // 将 Bytes 按 8 KiB 零拷贝切块：slice_ref 是引用计数切片，共享同一底层
    // 分配——峰值内存为响应体一份而非深拷贝的两份。代价是任一未消费的块
    // 都会把整份分配钉到流结束/断开；body 本就因重试保障全量驻留 spool，
    // 此处无额外驻留
    const CHUNK_SIZE: usize = 8 * 1024;
    let chunks: Vec<Bytes> = body.chunks(CHUNK_SIZE).map(|c| body.slice_ref(c)).collect();

    let stream = futures_util::stream::iter(chunks.into_iter().map(Ok::<Bytes, std::io::Error>));
    let body = Body::from_stream(stream);
    resp.body(body).unwrap().into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    // ---- 流式回放零拷贝：切块必须共享原 body 的底层分配（slice_ref 引用
    // 计数切片），而非 copy_from_slice 深拷贝——深拷贝会让峰值内存瞬时
    // 翻倍（50MB SSE 实测曾达 64MB）。指针区间断言：任何拷贝都会落到
    // 原分配之外的新地址 ----

    #[tokio::test]
    async fn replay_chunks_are_zero_copy_views_of_source() {
        // 20 KiB → 3 块（8K + 8K + 4K），含非整除边界
        let body = Bytes::from(vec![0xA5u8; 20 * 1024]);
        let base = body.as_ptr() as usize;
        let end = base + body.len();

        let resp = build_stream_replay_response(StatusCode::OK, HeaderMap::new(), body.clone());
        // 逐帧轮询响应体：axum 的 Body::from_stream 原样透传我们产出的 Bytes
        //（不再复制），故帧数据指针可直接检验
        let mut streamed = resp.into_body();
        let mut frames = Vec::new();
        while let Some(frame) = http_body_util::BodyExt::frame(&mut streamed)
            .await
            .transpose()
            .unwrap()
        {
            if let Some(data) = frame.data_ref() {
                let p = data.as_ptr() as usize;
                assert!(
                    (base..end).contains(&p),
                    "回放块指针 {p:#x} 必须落在原 body 分配 [{base:#x}, {end:#x}) 内（零拷贝）"
                );
                frames.push(data.to_vec());
            }
        }
        assert_eq!(frames.len(), 3, "20 KiB 应切为 3 块");
        // 各块按序还原 == 原 body（切块只分不变）
        let reassembled: Vec<u8> = frames.into_iter().flatten().collect();
        assert_eq!(reassembled, body.as_ref());
    }

    // ---- 保活触发：请求体顶层 "stream": true 的流式判定（只看顶层、只认字面
    // 量 true、解析失败一律为假） ----

    #[test]
    fn stream_flag_probe_reads_only_top_level_literal_true() {
        let probe =
            |s: &str| json_requests_stream(serde_json::Deserializer::from_slice(s.as_bytes()));
        for yes in [
            r#"{"stream":true}"#,
            r#"{"model":"m","messages":[{"role":"user","content":"hi"}],"stream":true}"#,
            " \n{ \"stream\" : true }\r\n ",
            // 重复键以最后一次为准
            r#"{"stream":false,"stream":true}"#,
        ] {
            assert!(probe(yes), "应判为流式: {yes}");
        }
        // 转义写法的键名与字面键名等价：键名里的 e 写成 JSON Unicode 转义
        // （反斜杠 + u0065）。反斜杠在运行时拼出：写成源码字面量时，转义序列
        // 容易被编辑/生成工具提前还原成字母 e，测试就悄悄退化成与上面第一条
        // 重复——下方断言守住这个前提
        let backslash = char::from(92);
        let escaped = format!("{{\"str{backslash}u0065am\":true}}");
        assert!(
            escaped.contains(backslash) && !escaped.contains("stream"),
            "前提：键名确实是转义写法: {escaped}"
        );
        assert!(probe(&escaped), "转义键名应判为流式: {escaped}");
        for no in [
            r#"{"stream":false}"#,
            r#"{"model":"m"}"#,
            r#"{"stream":"true"}"#,
            r#"{"stream":1}"#,
            r#"{"stream":null}"#,
            r#"{"stream":[true]}"#,
            r#"{"stream":{"enabled":true}}"#,
            r#"{"stream":true,"stream":false}"#,
            // 嵌套层的 stream 不算
            r#"{"messages":[{"stream":true}]}"#,
            r#"{"meta":{"stream":true},"stream":false}"#,
            // 顶层不是对象
            r#"[{"stream":true}]"#,
            r#""stream""#,
            "true",
            // 非 JSON / 截断 / 尾随垃圾 / 空体
            r#"{"stream":true"#,
            r#"{"stream":true} x"#,
            "stream=true",
            "",
        ] {
            assert!(!probe(no), "不应判为流式: {no:?}");
        }
    }

    #[tokio::test]
    async fn stream_flag_probe_streams_disk_spooled_body() {
        // 溢写磁盘的大请求体：stream 键排在数 MB 字段之后，从文件流式扫描
        let dir = tempfile::tempdir().unwrap();
        let big = "y".repeat(3 * 1024 * 1024);
        for (stream, want) in [("true", true), ("false", false)] {
            let path = dir.path().join(format!("req-{stream}.spooltmp"));
            let json =
                format!(r#"{{"model":"m","messages":[{{"content":"{big}"}}],"stream":{stream}}}"#);
            std::fs::write(&path, &json).unwrap();
            let body = RequestBody::Disk {
                path,
                len: json.len() as u64,
            };
            assert_eq!(
                request_body_wants_stream(&body).await,
                want,
                "stream={stream}"
            );
        }
    }

    // ---- 保活通道：上游真实头的可提交条件 ----

    #[test]
    fn head_is_committable_requires_2xx_uncompressed_sse() {
        let headers = |pairs: &[(&'static str, &'static str)]| {
            let mut h = HeaderMap::new();
            for (k, v) in pairs {
                h.insert(*k, HeaderValue::from_static(v));
            }
            h
        };
        let sse = headers(&[("content-type", "text/event-stream; charset=utf-8")]);
        assert!(head_is_committable(StatusCode::OK, &sse));
        assert!(head_is_committable(
            StatusCode::OK,
            &headers(&[
                ("content-type", "text/event-stream"),
                ("content-encoding", "identity")
            ])
        ));
        // 非 2xx：要重试，不能先提交上游的错误状态行
        assert!(!head_is_committable(StatusCode::TOO_MANY_REQUESTS, &sse));
        // 非 SSE：心跳注释只在 SSE 里合法
        assert!(!head_is_committable(
            StatusCode::OK,
            &headers(&[("content-type", "application/json")])
        ));
        assert!(!head_is_committable(
            StatusCode::OK,
            &headers(&[("content-type", "application/x-ndjson")])
        ));
        // 压缩流：往里插明文心跳会让客户端解压失败
        assert!(!head_is_committable(
            StatusCode::OK,
            &headers(&[
                ("content-type", "text/event-stream"),
                ("content-encoding", "gzip")
            ])
        ));
    }

    // ---- 断开提示：只收到心跳的等待 ≥60s 后断开才提示检查客户端的流空闲超时 ----

    #[derive(Clone, Default)]
    struct CapturedLogs(Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for CapturedLogs {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// 已提交、提交于 `waited` 之前的保活出口（测试构造用；通道两端随返回值存活）
    fn committed_sink(
        waited: Duration,
        heartbeats_only: bool,
    ) -> (
        KeepaliveSink,
        tokio::sync::mpsc::Receiver<Result<Bytes, std::io::Error>>,
        tokio::sync::watch::Sender<bool>,
    ) {
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        let (gone_tx, gone_rx) = tokio::sync::watch::channel(false);
        let sink = KeepaliveSink::Committed {
            tx,
            gone_rx,
            at: tokio::time::Instant::now() - waited,
            heartbeats_only,
            beat: Heartbeat {
                fixed: Bytes::from_static(b": keepalive\n\n"),
                dynamic: None,
            },
            replay_gate: Arc::default(),
        };
        (sink, rx, gone_tx)
    }

    #[test]
    fn heartbeat_only_disconnect_logs_generic_client_hint() {
        let logs = CapturedLogs::default();
        let sink_logs = logs.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || sink_logs.clone())
            .with_ansi(false)
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);
        let text = || String::from_utf8_lossy(&logs.0.lock().unwrap()).to_string();
        const HINT: &str = "流空闲超时";

        // 尚未提交 / 只收心跳但等待不足 60s（多为主动取消）/ 已开始回放真实字节：
        // 只记断开，不提示
        KeepaliveSink::Pending {
            resp_tx: None,
            skeleton_due: false,
            beat: Heartbeat {
                fixed: Bytes::new(),
                dynamic: None,
            },
        }
        .log_client_gone("等待上游");
        let (short, _rx1, _g1) = committed_sink(Duration::from_secs(30), true);
        short.log_client_gone("等待上游");
        let (replaying, _rx2, _g2) = committed_sink(Duration::from_secs(900), false);
        replaying.log_client_gone("回放");
        assert!(text().contains("客户端已断开"), "{}", text());
        assert!(!text().contains(HINT), "以上三种都不应提示: {}", text());

        // 只收心跳等了 300s（Codex 的默认事件级超时）后断开：提示，且不点名客户端
        let (codex_like, _rx3, _g3) = committed_sink(Duration::from_secs(300), true);
        codex_like.log_client_gone("退避等待");
        let out = text();
        assert!(
            out.contains(HINT) && out.contains("300"),
            "只收心跳 ≥60s 后断开应提示并带上等待秒数: {out}"
        );
        assert!(
            !out.contains("Claude Code") && !out.contains("CLAUDE_STREAM_IDLE_TIMEOUT_MS"),
            "提示不点名具体客户端: {out}"
        );
    }

    #[tokio::test]
    async fn dynamic_heartbeat_arriving_after_replay_started_is_dropped() {
        // 心跳转换器慢（400ms）：这一拍的调用还没回来，回放已写出第一块真实
        // 字节。迟到的动态心跳必须丢弃——写出去就插进了回放的事件中间
        let name = if cfg!(windows) {
            "format-echo.exe"
        } else {
            "format-echo"
        };
        let mut dir = std::env::current_exe().unwrap();
        let echo = loop {
            let parent = dir.parent().expect("format-echo 未编译").to_path_buf();
            if parent.join("examples").join(name).exists() {
                break parent.join("examples").join(name);
            }
            dir = parent;
        };
        let pool = Arc::new(crate::transform::TransformPool::new(Arc::new(
            crate::config::TransformConfig {
                command: echo.display().to_string(),
                args: vec!["heartbeat".to_string(), "400".to_string()],
                timeout_secs: Some(10),
                ..Default::default()
            },
        )));
        let (mut sink, mut rx, _gone) = committed_sink(Duration::ZERO, true);
        sink.arm_dynamic_heartbeat(Arc::new(DynamicHeartbeat {
            pool,
            ctx: ExchangeCtx::new("1".to_string()),
            source: HeartbeatSource::new(
                &http::Method::POST,
                "http://upstream.invalid/v1/messages",
                &HeaderMap::new(),
                &RequestBody::Memory(Bytes::new()),
            ),
            seq: Default::default(),
            attempt: Default::default(),
            in_flight: Default::default(),
            warned: Default::default(),
        }));
        assert!(sink.heartbeat(), "发起这一拍的动态心跳（在途）");
        let real = Bytes::from_static(b"data: real\n\n");
        assert!(sink.send(Ok(real.clone())).await);
        assert_eq!(rx.recv().await.unwrap().unwrap(), real);
        // 等心跳转换回来（400ms + 进程启动）后，通道里不应再出现任何字节
        tokio::time::sleep(Duration::from_millis(2000)).await;
        assert!(rx.try_recv().is_err(), "回放开始后到达的心跳不应写出");
    }

    #[tokio::test]
    async fn sink_send_ends_heartbeat_only_state() {
        // 写出任何真实字节（回放 / 终态 error 事件）后不再是「只收心跳」：此后的
        // 断开与客户端流空闲超时无关
        let (mut sink, mut rx, _gone) = committed_sink(Duration::ZERO, true);
        assert!(
            sink.send(Ok(Bytes::from_static(
                b"data: x

"
            )))
            .await
        );
        assert!(rx.recv().await.is_some());
        assert!(matches!(
            sink,
            KeepaliveSink::Committed {
                heartbeats_only: false,
                ..
            }
        ));
    }

    // ---- 入站 Host 归一：Host 头与配置条目必须归一到同一形态才能比较 ----

    #[test]
    fn normalize_host_forms() {
        for (raw, want) in [
            ("127.0.0.1:12345", "127.0.0.1"),
            ("LocalHost", "localhost"),
            ("localhost:1", "localhost"),
            ("[::1]:12345", "[::1]"),
            ("[::1]", "[::1]"),
            ("::1", "[::1]"),
            ("MyProxy.Local:999", "myproxy.local"),
            // 配置条目误写成 URL：取主机部分
            ("http://myhost:8080/", "myhost"),
            ("https://[::1]:9/x", "[::1]"),
            ("evil.example.com", "evil.example.com"),
        ] {
            assert_eq!(normalize_host(raw), want, "{raw}");
        }
        assert_eq!(
            normalize_origin(" http://LOCALHOST:5173/ "),
            "http://localhost:5173"
        );
    }

    // ---- 响应 spool 收尾 flush：最后一块的写失败不得被吞 ----

    #[tokio::test]
    async fn spool_finish_surfaces_error_of_last_write() {
        // 注入写失败：以**只读**句柄充当 spool 写句柄，阻塞池里的真实写入必然
        // 失败（Windows 拒绝访问 / unix EBADF），而 tokio File 的 poll_write
        // 派发后立刻报 Ok——这正是缺陷成立的前提：最后一块之后再无写操作，
        // 只有收尾 flush 能观察到这个错误
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("spool-flush-test.spooltmp");
        std::fs::write(&path, b"").unwrap();
        let file = tokio::fs::File::open(&path).await.unwrap();
        let mut buf = SpoolBuffer::Disk {
            file,
            path: SpoolPath(path.clone()),
            len: 0,
        };
        spool_chunk(&mut buf, b"final chunk", Some(dir.path())).await;
        assert!(
            matches!(buf, SpoolBuffer::Disk { len: 11, .. }),
            "前提：write_all 本身报 Ok，错误留到下一次 write/flush 才浮现"
        );

        let result = buf.finish(StreamErrorScanner::new()).await;
        let err = result.err().expect("收尾 flush 必须暴露最后一块的写失败");
        assert!(err.contains("spool 磁盘写入失败"), "{err}");
        assert!(!path.exists(), "失败的 spool 半截文件必须删除");
    }

    #[tokio::test]
    async fn dropped_disk_spool_buffer_deletes_partial_file() {
        // 收集途中被整体丢弃（客户端断开让在途 forward_once 被 drop 即此情形）：
        // 半截 spool 文件必须当场删除，不能留到下次启动才由 clean_spool_dir 回收
        let dir = tempfile::tempdir().unwrap();
        let mut buf = SpoolBuffer::Memory(Vec::new());
        spool_chunk(&mut buf, &vec![b'x'; RESIDENT_LIMIT + 1], Some(dir.path())).await;
        assert!(matches!(buf, SpoolBuffer::Disk { .. }), "前提：已溢写磁盘");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        drop(buf);
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            0,
            "被丢弃的 spool 缓冲必须删除自己的临时文件"
        );
    }

    #[tokio::test]
    async fn spool_chunk_dropped_mid_disk_transition_leaves_no_file() {
        // 内存 → 磁盘迁移本身要好几次 await（建文件、写 1 MiB 内存前缀、写本块）；
        // future 恰在这期间被 drop（客户端断开）时，已建出的文件也必须删除。
        // 逐次 poll，文件一出现就丢弃 future——正落在迁移中途
        let dir = tempfile::tempdir().unwrap();
        let mut buf = SpoolBuffer::Memory(vec![b'x'; RESIDENT_LIMIT]);
        let chunk = vec![b'y'; 1024];
        let count = || std::fs::read_dir(dir.path()).unwrap().count();
        {
            let fut = spool_chunk(&mut buf, &chunk, Some(dir.path()));
            tokio::pin!(fut);
            loop {
                let polled = std::future::poll_fn(|cx| Poll::Ready(fut.as_mut().poll(cx))).await;
                assert!(polled.is_pending(), "前提：迁移不应在文件出现前就一次完成");
                if count() > 0 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        }
        // 阻塞池里在途的写操作可能还持着句柄：删除已发出，等它收尾
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while count() > 0 {
            assert!(
                std::time::Instant::now() < deadline,
                "迁移中途被丢弃的 spool 文件必须删除"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[tokio::test]
    async fn request_body_dropped_mid_upload_leaves_no_file() {
        // 客户端上传到一半断开：hyper 直接 drop handler future，read_request_body
        // 停在「等下一块」的 await 上、走不到任何显式出错分支——已溢写的
        // req-*.spooltmp 也必须当场删除。首块超过驻留阈值触发溢写，之后流
        // 永远 pending（客户端不再发送）
        use futures_util::StreamExt as _;
        let dir = tempfile::tempdir().unwrap();
        let first = Bytes::from(vec![b'x'; RESIDENT_LIMIT + 1]);
        let stream = futures_util::stream::iter([Ok::<_, std::io::Error>(first)])
            .chain(futures_util::stream::pending());
        let count = || std::fs::read_dir(dir.path()).unwrap().count();
        {
            let fut = read_request_body(Body::from_stream(stream), usize::MAX, Some(dir.path()));
            tokio::pin!(fut);
            // 逐次 poll：文件出现后再推进一段时间，让溢写写完、future 停到
            // 「等下一块」上（这正是客户端上传中途停住再断开的形态）
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            let mut settled_polls = 0;
            while settled_polls < 50 {
                let polled = std::future::poll_fn(|cx| Poll::Ready(fut.as_mut().poll(cx))).await;
                assert!(polled.is_pending(), "前提：请求体流未结束，读取不应完成");
                assert!(std::time::Instant::now() < deadline, "溢写文件始终未出现");
                if count() > 0 {
                    settled_polls += 1;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while count() > 0 {
            assert!(
                std::time::Instant::now() < deadline,
                "上传中途被丢弃的请求体临时文件必须删除"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[tokio::test]
    async fn spool_finish_disk_success_keeps_full_content() {
        // 对照组：正常写句柄下 finish 成功，长度与文件内容一致（flush 不改变
        // 成功路径的语义）
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("spool-flush-ok.spooltmp");
        let file = tokio::fs::File::create(&path).await.unwrap();
        let mut buf = SpoolBuffer::Disk {
            file,
            path: SpoolPath(path.clone()),
            len: 0,
        };
        spool_chunk(&mut buf, b"hello ", Some(dir.path())).await;
        spool_chunk(&mut buf, b"world", Some(dir.path())).await;
        let (body, scan) = buf
            .finish(StreamErrorScanner::new())
            .await
            .expect("正常写入的 finish 应成功");
        assert_eq!(body.len(), 11);
        assert!(scan.is_some(), "磁盘模式必带增量扫描结论");
        assert_eq!(std::fs::read(&path).unwrap(), b"hello world");
    }

    // ---- preview_body：二进制/压缩体不污染日志（回归：zstd 错误体曾被
    // from_utf8_lossy 渲染成整片乱码）----

    #[test]
    fn preview_of_binary_shows_hex_not_mojibake() {
        // zstd magic 开头的压缩体（真实日志中出现过）
        let mut zstd_like = vec![0x28, 0xb5, 0x2f, 0xfd];
        zstd_like.extend((0..200u8).map(|i| i.wrapping_mul(37)));
        let p = preview_body(&zstd_like, 500, zstd_like.len());
        assert!(p.contains("二进制/压缩内容"), "{p}");
        assert!(p.contains("28 b5 2f fd"), "hex 应含 zstd magic: {p}");
        assert!(!p.contains('\u{FFFD}'), "不应输出替换符: {p}");
    }

    #[test]
    fn preview_of_text_is_plain_and_truncated() {
        let text = "上游过载：请稍后重试".repeat(50);
        let p = preview_body(text.as_bytes(), 100, text.len());
        assert!(p.contains("上游过载"), "{p}");
        assert!(p.contains("截断"), "{p}");
        // 正常 JSON 错误体
        let json = br#"{"type":"error","error":{"type":"overloaded"}}"#;
        assert_eq!(
            preview_body(json, 500, json.len()),
            String::from_utf8_lossy(json)
        );
    }

    #[test]
    fn preview_byte_count_is_the_response_size_not_the_snapshot_size() {
        // 磁盘模式回归：喂进来的是 1 KiB 头部快照，字节数必须报**响应真实大小**
        //（spool 长度），否则日志会把 10 MB 的响应说成 1 KiB，误导排障
        let snapshot = "上游过载：请稍后重试".repeat(60); // > limit，触发截断分支
        let p = preview_body(snapshot.as_bytes(), 100, 10 * 1024 * 1024);
        assert!(p.contains("截断"), "{p}");
        assert!(
            p.contains("共 10485760 字节"),
            "截断提示应报响应真实大小而非快照长度: {p}"
        );
        assert!(
            !p.contains(&format!("共 {} 字节", snapshot.len())),
            "不得把快照长度当成响应大小: {p}"
        );
        // 二进制分支同样按真实大小口径
        let bin = [0x28u8, 0xb5, 0x2f, 0xfd].repeat(500);
        let p = preview_body(&bin, 500, 7_000_000);
        assert!(p.contains("共 7000000 字节"), "{p}");
    }

    // ---- is_hop_header：hop-by-hop 头识别（大小写不敏感）----

    #[test]
    fn hop_headers_are_all_recognized() {
        for name in [
            "connection",
            "keep-alive",
            "proxy-authenticate",
            "proxy-authorization",
            "te",
            "trailer",
            "transfer-encoding",
            "upgrade",
            "host",
            "content-length",
        ] {
            assert!(is_hop_header(name), "{name} 应为 hop 头");
        }
    }

    #[test]
    fn non_hop_headers_are_not_filtered() {
        for name in ["x-custom", "authorization", "content-type", "accept"] {
            assert!(!is_hop_header(name), "{name} 不应被当作 hop 头");
        }
    }

    #[test]
    fn hop_matching_is_case_insensitive() {
        for name in [
            "Connection",
            "Keep-Alive",
            "HOST",
            "Content-Length",
            "Transfer-Encoding",
            "Proxy-Authorization",
            "TE",
            "Upgrade",
        ] {
            assert!(is_hop_header(name), "大小写不应影响 {name} 的 hop 判定");
        }
    }

    // ---- is_hop_response_header：响应侧过滤 = hop 头去掉 content-length
    //（仅当上游未用 transfer-encoding 分帧时）----

    #[test]
    fn response_hop_filter_keeps_content_length() {
        // 字节未经变换，上游声明的长度仍精确：剥掉只会让定长响应退化为 chunked
        for name in ["content-length", "Content-Length", "CONTENT-LENGTH"] {
            assert!(
                !is_hop_response_header(name, false),
                "{name} 在响应侧必须保留（上游未分帧，字节未经变换）"
            );
            assert!(is_hop_header(name), "但它在请求侧仍是 hop 头");
        }
        // 其余 hop 头响应侧照旧过滤
        for name in [
            "connection",
            "keep-alive",
            "proxy-authenticate",
            "proxy-authorization",
            "te",
            "trailer",
            "transfer-encoding",
            "upgrade",
            "host",
        ] {
            assert!(
                is_hop_response_header(name, false),
                "{name} 在响应侧应被过滤"
            );
        }
        // 普通端到端头不受影响
        for name in ["content-type", "set-cookie", "x-custom"] {
            assert!(!is_hop_response_header(name, false), "{name} 不应被过滤");
        }
    }

    #[test]
    fn response_hop_filter_drops_content_length_when_upstream_chunked() {
        // 上游同时带 CL 与 TE（协议违规但现实存在）：hyper 客户端按 TE 分帧，
        // 我们手里的 CL 只是上游的一句声明，与实际字节数可能不符——必须连同
        // transfer-encoding 一起剥掉，交由 hyper 重新分帧（RFC 7230 §3.3.3）。
        // CL 声称值大于实际时的后果是**静默**的：服务端按 CL 等缺额，客户端
        // 拿不到完整响应也看不出原因。
        for name in ["content-length", "Content-Length", "CONTENT-LENGTH"] {
            assert!(
                is_hop_response_header(name, true),
                "{name} 在上游用 TE 分帧时必须剥离"
            );
        }
        // 上游带 TE 时其余头的判定不受影响
        assert!(is_hop_response_header("transfer-encoding", true));
        assert!(!is_hop_response_header("content-type", true));
    }

    // ---- apply_header_overrides：api_key / override_headers / extra_headers 三层头策略 ----

    #[test]
    fn api_key_sets_bearer_authorization() {
        let cfg = Config {
            base_url: "https://api.example.com".to_string(),
            api_key: Some("sk-test".to_string()),
            ..Default::default()
        };
        let mut headers = HeaderMap::new();
        apply_header_overrides(&mut headers, &cfg);
        assert_eq!(headers.get("authorization").unwrap(), "Bearer sk-test");
    }

    #[test]
    fn api_key_overwrites_existing_authorization() {
        let cfg = Config {
            base_url: "https://api.example.com".to_string(),
            api_key: Some("sk-test".to_string()),
            ..Default::default()
        };
        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            HeaderValue::from_static("Basic dXNlcjpwYXNz"),
        );
        apply_header_overrides(&mut headers, &cfg);
        assert_eq!(headers.get("authorization").unwrap(), "Bearer sk-test");
    }

    #[test]
    fn missing_api_key_adds_no_authorization() {
        let cfg = Config {
            base_url: "https://api.example.com".to_string(),
            ..Default::default()
        };
        let mut headers = HeaderMap::new();
        headers.insert("x-custom", HeaderValue::from_static("keep"));
        apply_header_overrides(&mut headers, &cfg);
        assert!(
            headers.get("authorization").is_none(),
            "未配置 api_key 不应新增 Authorization"
        );
        assert_eq!(headers.get("x-custom").unwrap(), "keep", "其余头应保持不变");
    }

    #[test]
    fn override_headers_unconditionally_replace_existing() {
        // 配置键 "X-Foo" 应覆盖已有 "x-foo"（大小写不敏感），且同名多值全部收敛为单个新值
        let cfg = Config {
            base_url: "https://api.example.com".to_string(),
            override_headers: HashMap::from([("X-Foo".to_string(), "new".to_string())]),
            ..Default::default()
        };
        let mut headers = HeaderMap::new();
        headers.append("x-foo", HeaderValue::from_static("old1"));
        headers.append("x-foo", HeaderValue::from_static("old2"));
        apply_header_overrides(&mut headers, &cfg);
        assert_eq!(
            headers.get_all("x-foo").iter().count(),
            1,
            "覆盖后应只剩一个值"
        );
        assert_eq!(headers.get("x-foo").unwrap(), "new");
    }

    #[test]
    fn override_headers_add_missing_headers() {
        let cfg = Config {
            base_url: "https://api.example.com".to_string(),
            override_headers: HashMap::from([("x-added".to_string(), "v".to_string())]),
            ..Default::default()
        };
        let mut headers = HeaderMap::new();
        apply_header_overrides(&mut headers, &cfg);
        assert_eq!(
            headers.get("x-added").unwrap(),
            "v",
            "override 对缺失头应直接新增"
        );
    }

    #[test]
    fn extra_headers_only_fill_missing() {
        let cfg = Config {
            base_url: "https://api.example.com".to_string(),
            extra_headers: HashMap::from([
                ("x-extra".to_string(), "added".to_string()),
                ("x-present".to_string(), "ignored".to_string()),
            ]),
            ..Default::default()
        };
        let mut headers = HeaderMap::new();
        // 大小写不同的同名头也应保留原值，不被 extra_headers 覆盖
        headers.insert("X-PRESENT", HeaderValue::from_static("keep"));
        apply_header_overrides(&mut headers, &cfg);
        assert_eq!(headers.get("x-extra").unwrap(), "added", "缺失头应被追加");
        assert_eq!(
            headers.get("x-present").unwrap(),
            "keep",
            "已存在头不应被覆盖"
        );
    }

    #[test]
    fn override_headers_beat_api_key_for_authorization() {
        // 优先级：api_key 先应用、override_headers 后应用 → authorization 最终取 override 值
        let cfg = Config {
            base_url: "https://api.example.com".to_string(),
            api_key: Some("sk-api".to_string()),
            override_headers: HashMap::from([(
                "Authorization".to_string(),
                "Bearer sk-override".to_string(),
            )]),
            ..Default::default()
        };
        let mut headers = HeaderMap::new();
        apply_header_overrides(&mut headers, &cfg);
        assert_eq!(headers.get("authorization").unwrap(), "Bearer sk-override");
    }

    #[test]
    fn extra_headers_do_not_overwrite_existing_authorization() {
        // extra_headers 优先级最低（只补缺失）：api_key 已写入 authorization 时不再覆盖
        let cfg = Config {
            base_url: "https://api.example.com".to_string(),
            api_key: Some("sk-api".to_string()),
            extra_headers: HashMap::from([(
                "Authorization".to_string(),
                "should-not-win".to_string(),
            )]),
            ..Default::default()
        };
        let mut headers = HeaderMap::new();
        apply_header_overrides(&mut headers, &cfg);
        assert_eq!(headers.get("authorization").unwrap(), "Bearer sk-api");
    }

    #[test]
    fn invalid_names_and_values_are_silently_skipped() {
        // 非法头名（含空格/控制字符）与非法头值（含控制字符）应被静默跳过，不 panic
        let cfg = Config {
            base_url: "https://api.example.com".to_string(),
            api_key: Some("bad\nkey".to_string()), // 含换行控制符 → HeaderValue 非法，api_key 应被跳过
            override_headers: HashMap::from([
                ("bad name".to_string(), "v".to_string()), // 含空格 → HeaderName 非法
                ("x-good".to_string(), "ok".to_string()),
            ]),
            extra_headers: HashMap::from([("x-bad-val".to_string(), "bad\rval".to_string())]),
            ..Default::default()
        };
        let mut headers = HeaderMap::new();
        apply_header_overrides(&mut headers, &cfg);
        assert!(
            headers.get("authorization").is_none(),
            "非法 api_key 不应写入 Authorization"
        );
        assert!(headers.get("bad name").is_none(), "非法头名应跳过");
        assert_eq!(
            headers.get("x-good").unwrap(),
            "ok",
            "合法 override 应正常生效"
        );
        assert!(headers.get("x-bad-val").is_none(), "非法头值应跳过");
    }

    // ---- 压缩响应体：检查必须走解码副本 ----
    //
    // 上游按客户端 accept-encoding 压缩响应，而 reqwest 为保真透传刻意不解压，
    // 于是判定与预览都跑在压缩字节上——JSON 解析必然失败、SSE 行扫描全失效、
    // 预览只剩 hex。2026-09-14 由 opencode.ai 的 brotli 404 页实测暴露。

    /// 构造带 content-encoding 的响应头（判定只看这一个头）
    fn headers_with_encoding(encoding: &str) -> reqwest::header::HeaderMap {
        let mut h = reqwest::header::HeaderMap::new();
        h.insert(reqwest::header::CONTENT_ENCODING, encoding.parse().unwrap());
        h
    }

    fn gzip_bytes(data: &[u8]) -> Vec<u8> {
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut enc, data).unwrap();
        enc.finish().unwrap()
    }

    #[test]
    fn compressed_error_json_with_200_is_retried() {
        // 回归：压缩后「200 携带 error JSON」曾完全检测不到——状态码 200 不触发
        // 重试，而 is_error_body 对压缩字节解析 JSON 必然失败
        let payload = br#"{"type":"error","error":{"type":"overloaded_error"}}"#;
        let headers = headers_with_encoding("gzip");
        assert!(
            should_retry_response(1, &StatusCode::OK, &headers, &gzip_bytes(payload)),
            "gzip 压缩的错误 JSON（HTTP 200）必须判定为需重试"
        );
        // 反向锚定：同一份 payload 不压缩时本就命中——证明样本有效，断言不是恒真
        assert!(
            should_retry_response(
                1,
                &StatusCode::OK,
                &reqwest::header::HeaderMap::new(),
                payload
            ),
            "未压缩的同一 payload 必须命中（样本有效性锚点）"
        );
    }

    #[test]
    fn brotli_cloudflare_404_page_is_retried_and_readable() {
        // 事故原形：Cloudflare 对不存在的路径返回 **brotli** 压缩的 HTML 404 页。
        // brotli 没有 magic number，日志里只剩一串 hex，连「是压缩体」都认不出
        let page = b"<!DOCTYPE html><html><head><title>404 Not Found</title></head>\
<body><h1>404 Not Found</h1></body></html>";
        let mut enc = brotli::CompressorWriter::new(Vec::new(), 4096, 5, 22);
        std::io::Write::write_all(&mut enc, page).unwrap();
        let compressed = enc.into_inner();

        let mut headers = headers_with_encoding("br");
        headers.insert(reqwest::header::CONTENT_TYPE, "text/html".parse().unwrap());
        assert!(
            should_retry_response(1, &StatusCode::NOT_FOUND, &headers, &compressed),
            "404 必须重试"
        );

        // 修复目标本身：预览必须读得出正文（此前只剩 hex 摘要）
        let decoded = crate::decode::for_inspection(Some("br"), &compressed)
            .expect("br 应可解码（检查副本）");
        let preview = preview_body(&decoded, 500, decoded.len());
        assert!(
            preview.contains("404 Not Found"),
            "预览应含 404 页正文，实际: {preview}"
        );
    }

    #[test]
    fn uncompressed_responses_are_judged_without_decoding() {
        // 无 content-encoding 时不得解码（否则是一次无谓的整体拷贝）：
        // 传入「看起来像压缩体」的明文字节，判定仍按明文语义走
        let plain = br#"{"content":"hello"}"#;
        assert!(!should_retry_response(
            1,
            &StatusCode::OK,
            &reqwest::header::HeaderMap::new(),
            plain
        ));
    }
}
