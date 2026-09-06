//! 代理核心：将本地请求完整透传到上游，并在失败时无限重试。
//!
//! 设计要点：
//! - 单一 upstream base URL，其余路径与查询参数完整透传（包含 `/health` 等，上游若有同名路径亦完整透传）
//! - 网络错误 / 4xx / 5xx / 错误 JSON 内容 → 全部无限重试，梯度延迟（`retry` 模块）
//!   原型阶段 4xx 同样视为易发瞬时错误（如限流、临时鉴权波动、上游误报）
//! - 非流式与流式统一缓冲策略：
//!   1. 对上游响应先完整缓冲（spool）到内存（上限 `MAX_SPOOL_BYTES`），期间任何网络中断
//!      均视为 `NetworkError` 触发重试——满足“流式中断可重试”的需求。
//!   2. 缓冲完成后，再做“是否可重试”的判定：
//!      - HTTP 状态码可重试（4xx / 5xx，`is_retryable_status`）
//!      - 非流式：`is_error_body` 命中（任意状态码下只要 body 语义为报错就重试）
//!      - 流式：`is_stream_error_body` 命中（扫描 SSE data 行 / NDJSON）
//!   3. 仅当整轮缓冲成功且判定为不可重试时，才将缓冲体回放给客户端；
//!      若判定为流式（`is_streaming_response`），则以 chunked 流式分块原样回放，
//!      逐块产出保持与上游字节完全一致，SSE 解析器语义不受影响。
//! - 首轮快速路径：attempt 1 的结果先做判定，成功则直接原样回放（status 与全部
//!   响应头保真）；仅当需要重试时才进入重试通道——首轮成功是常态路径，保真优先。
//! - 重试期间对客户端的保活：仅在「需要重试」且客户端接受 SSE 时，才立即返回
//!   SSE 流式骨架，由后台任务从 attempt 2 继续无限重试，并在重试间隙向下游发送
//!   SSE 注释保活（`: keepalive ...\n\n`），防止客户端因 idle 超时而断开；
//!   后台任务在客户端断开（channel 关闭）时立即退出，不空转。
//! - 头处理：`api_key` 快捷覆盖 `Authorization: Bearer` 与 `x-api-key`（Anthropic 风格），
//!   `extra_headers` 追加缺失头，`override_headers` 无条件覆盖（兼容非 Bearer 鉴权与额外头需求）

use axum::{
    Router,
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use bytes::Bytes;
use futures_util::StreamExt as _;
use std::{sync::Arc, time::Duration};

use crate::{config::Config, retry};

/// 需要过滤的 hop-by-hop 头，避免透传导致协议错误或与 hyper/reqwest 的
/// 自动管理（content-length / transfer-encoding / host / connection）冲突。
const HOP_HEADERS: &[&str] = &[
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

fn is_hop_header(name: &str) -> bool {
    HOP_HEADERS.contains(&name.to_ascii_lowercase().as_str())
}

/// 共享状态
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub client: reqwest::Client,
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
        Self {
            config: Arc::new(config),
            client,
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
    let mut headers = req.headers().clone();

    // 在本地侧先应用覆盖/追加，避免重试间重复计算
    apply_header_overrides(&mut headers, &state.config);

    // 缓冲请求体以支持重试重放；10 MiB 上限，超出则直接返回 413。
    // 注意 to_bytes 的 Err 同时覆盖超限与连接中断两类原因，状态码取主流场景
    // （超限 413），消息保持中性不断言原因。
    let body_bytes = match axum::body::to_bytes(req.into_body(), 10 * 1024 * 1024).await {
        Ok(b) => b,
        Err(e) => {
            tracing::error!(error = %e, "读取请求体失败");
            return (
                StatusCode::PAYLOAD_TOO_LARGE,
                format!("请求体读取失败（超出 10 MiB 上限或连接中断）: {e}"),
            )
                .into_response();
        }
    };

    let path_and_query = uri.path_and_query().map(|pq| pq.as_str()).unwrap_or("/");

    let upstream_base = state.config.base_url.trim_end_matches('/');
    let target_url = format!("{}{}", upstream_base, path_and_query);

    tracing::info!(method = %method, path = %path_and_query, target = %target_url, "代理请求");

    // 是否启用 SSE 保活心跳（仅当客户端接受 SSE 且配置启用）
    let client_wants_sse = headers
        .get(http::header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_ascii_lowercase().contains("text/event-stream"))
        .unwrap_or(false);
    let keepalive_dur = state.config.keepalive_interval();
    let keepalive_enabled = state.config.keepalive_enabled() && keepalive_dur.as_secs() > 0;

    // 首轮（attempt 1）先行：成功则完整保真回放（status/headers 不失真），
    // 需要重试才进入重试通道——首轮成功是常态路径。
    let max_spool_bytes = spool_limit_bytes(&state.config);
    let first = forward_once(
        &state.client,
        &method,
        &target_url,
        &headers,
        body_bytes.clone(),
        max_spool_bytes,
    )
    .await;

    let needs_retry = match &first {
        ForwardResult::NetworkError(e) => {
            tracing::warn!(error = %e, "首轮上游网络错误，进入重试");
            true
        }
        // 超过 spool 上限重试无意义（确定性失败），直接终态回放 502
        ForwardResult::TooLarge => false,
        ForwardResult::Response {
            status,
            raw_headers,
            body,
            ..
        } => should_retry_response(1, status, raw_headers, body),
    };

    if !needs_retry {
        return match first {
            ForwardResult::TooLarge => (
                StatusCode::BAD_GATEWAY,
                "上游响应体超出 spool 上限，无法回放（重试无意义）",
            )
                .into_response(),
            ForwardResult::Response {
                status,
                headers: resp_headers,
                raw_headers,
                body,
            } => {
                let is_streaming = retry::is_streaming_response(&raw_headers, &body);
                if is_streaming {
                    build_stream_replay_response(status, resp_headers, body)
                } else {
                    build_response(status, resp_headers, body)
                }
            }
            ForwardResult::NetworkError(_) => unreachable!("NetworkError 必定 needs_retry"),
        };
    }

    // 需要重试：按 keepalive 条件选择通道
    if keepalive_enabled && client_wants_sse {
        return proxy_with_keepalive(state, method, target_url, headers, body_bytes).await;
    }
    proxy_without_keepalive(state, method, target_url, headers, body_bytes).await
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
    let is_streaming = retry::is_streaming_response(raw_headers, body);

    // 1. HTTP 状态码可重试：无论是否流式，都重试
    if retry::is_retryable_status(status.as_u16()) {
        tracing::warn!(attempt, status = %status, is_streaming, "上游返回可重试状态码，重试");
        tracing::warn!(preview = %preview_body(body, 500), "错误响应预览");
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
        tracing::warn!(attempt, is_streaming, preview = %preview_body(body, 1000), "上游返回错误内容，重试");
        return true;
    }

    false
}

/// 错误响应体的日志预览。上游可能返回压缩/二进制错误体（如 zstd/gzip 压缩的
/// 错误页——reqwest 未开自动解压以保真透传），直接 from_utf8_lossy 会把控制
/// 字节渲染成整片乱码污染日志。判定：替换符/控制字符占比超阈值视为二进制，
/// 改为 hex 摘要（可直接识别压缩 magic：zstd 28 b5 2f fd、gzip 1f 8b 等）。
fn preview_body(body: &[u8], limit: usize) -> String {
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
            "（二进制/压缩内容，共 {} 字节，hex 前 48: {}…）",
            body.len(),
            hex.trim_end()
        )
    } else {
        let s = text.trim_end();
        if body.len() > limit {
            format!("{s}…（截断，共 {} 字节）", body.len())
        } else {
            s.to_string()
        }
    }
}

/// 非 SSE 客户端的重试通道：从 attempt 2 起无限重试（attempt 1 已在 proxy_handler 完成），
/// 响应在成功后一次性返回。
async fn proxy_without_keepalive(
    state: AppState,
    method: http::Method,
    target_url: String,
    headers: HeaderMap,
    body_bytes: Bytes,
) -> Response {
    let mut attempt: u32 = 1;
    let max_backoff = state.config.max_retry_backoff_secs;
    let max_spool_bytes = spool_limit_bytes(&state.config);
    loop {
        attempt += 1;
        let delay = retry::delay_for_attempt(attempt - 1, max_backoff);
        if !delay.is_zero() {
            tracing::warn!(attempt, delay_ms = delay.as_millis() as u64, "重试延迟");
            tokio::time::sleep(delay).await;
        } else {
            tracing::warn!(attempt, "立即重试");
        }

        let result = forward_once(
            &state.client,
            &method,
            &target_url,
            &headers,
            body_bytes.clone(),
            max_spool_bytes,
        )
        .await;

        match result {
            ForwardResult::NetworkError(e) => {
                tracing::warn!(attempt, error = %e, "上游网络错误，重试");
                continue;
            }
            ForwardResult::TooLarge => {
                tracing::error!(attempt, "上游响应体超出 spool 上限，终止重试");
                return (
                    StatusCode::BAD_GATEWAY,
                    "上游响应体超出 spool 上限，无法回放（重试无意义）",
                )
                    .into_response();
            }
            ForwardResult::Response {
                status,
                headers: resp_headers,
                raw_headers,
                body,
            } => {
                if should_retry_response(attempt, &status, &raw_headers, &body) {
                    continue;
                }

                // 成功：按是否流式选择回放方式，保证“原样流式”
                let is_streaming = retry::is_streaming_response(&raw_headers, &body);
                tracing::info!(attempt, status = %status, is_streaming, "重试后成功");
                if is_streaming {
                    return build_stream_replay_response(status, resp_headers, body);
                } else {
                    return build_response(status, resp_headers, body);
                }
            }
        }
    }
}

/// 带保活的重试通道：仅在上游首轮已失败后进入。立即以 SSE 流响应并在流中发送
/// `: keepalive\n\n` 注释，后台从 attempt 2 起无限重试，成功后将上游 body 分块转发。
///
/// 注意：此通道的骨架响应已先行发出（200 + text/event-stream），上游真实 status
/// 与响应头无法再回放——这是「先保活、后成功」的固有取舍；首轮成功走的是
/// proxy_handler 的保真快速路径，不受影响。
async fn proxy_with_keepalive(
    state: AppState,
    method: http::Method,
    target_url: String,
    headers: HeaderMap,
    body_bytes: Bytes,
) -> Response {
    let keepalive_dur = state.config.keepalive_interval();
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(32);

    // 客户端断开信号：watch(false→true)。哨兵被 move 进响应 Body 的流闭包，
    // hyper 因客户端断开而 drop Body 时闭包随之销毁，哨兵 Drop 中置位；
    // 后台任务据此立即中止 in-flight 的上游请求（避免断开后上游继续生成白自计费）。
    let (gone_tx, mut gone_rx) = tokio::sync::watch::channel(false);
    let gone_guard = ClientGoneGuard { tx: gone_tx };

    // 心跳为 SSE 注释（": keepalive\n\n"），合法 SSE 客户端按规范忽略
    let heartbeat = || Bytes::from_static(b": keepalive\n\n");

    // 后台任务：无限重试上游，期间按 keepalive_dur 发送 SSE 注释；成功后将上游响应分块转发。
    // 所有 send 都检查客户端是否已断开（channel 关闭 → 立即退出，不空转）。
    let state_bg = state.clone();
    let max_spool_bytes = spool_limit_bytes(&state.config);
    tokio::spawn(async move {
        // 骨架发出后立即发首个心跳：客户端 idle 计时从收到字节起算
        if tx.send(Ok(heartbeat())).await.is_err() {
            return;
        }

        let mut attempt: u32 = 1;
        let max_backoff = state.config.max_retry_backoff_secs;
        loop {
            attempt += 1;
            let delay = retry::delay_for_attempt(attempt - 1, max_backoff);
            if !delay.is_zero() {
                // 在延迟期间按 keepalive_dur 切片发送心跳，避免客户端 idle 超时
                let mut elapsed = Duration::ZERO;
                while elapsed < delay {
                    let slice = std::cmp::min(keepalive_dur, delay - elapsed);
                    tokio::time::sleep(slice).await;
                    elapsed += slice;
                    if elapsed < delay {
                        // 仅在尚未到下一轮重试时发送心跳
                        if tx.send(Ok(heartbeat())).await.is_err() {
                            return;
                        }
                    }
                }
            } else {
                tracing::warn!(attempt, "立即重试（保活通道）");
            }

            // in-flight 期间与客户端断开信号竞速：断开即丢弃 forward_once future，
            // reqwest 连接随之关闭，上游（如 LLM API）会因连接断开停止生成——
            // 这是「客户端断开后本条请求立即断开」的关键点，防止计费浪费。
            let client_gone = async {
                // 信号置位或哨兵随 Body 提前销毁（channel 关闭）都视为断开
                let _ = gone_rx.wait_for(|v| *v).await;
            };
            let result = tokio::select! {
                r = forward_once(
                    &state_bg.client,
                    &method,
                    &target_url,
                    &headers,
                    body_bytes.clone(),
                    max_spool_bytes,
                ) => r,
                _ = client_gone => {
                    tracing::info!("客户端已断开，中止 in-flight 上游请求（保活通道）");
                    return;
                }
            };

            match result {
                ForwardResult::NetworkError(e) => {
                    tracing::warn!(attempt, error = %e, "上游网络错误，重试（保活通道）");
                    if tx.send(Ok(heartbeat())).await.is_err() {
                        return;
                    }
                    continue;
                }
                ForwardResult::TooLarge => {
                    tracing::error!(attempt, "上游响应体超出 spool 上限，终止重试（保活通道）");
                    // 骨架 200 已发出、状态行不可再改：静默结束流与「上游成功返回
                    // 空 body」在客户端视角不可区分。发一个终态错误事件让客户端
                    // 明确感知代理放弃了这条请求。
                    let err_event = Bytes::from_static(
                        b"event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"proxy_spool_limit\",\"message\":\"upstream response exceeded proxy spool limit\"}}\n\n",
                    );
                    let _ = tx.send(Ok(err_event)).await;
                    return;
                }
                ForwardResult::Response {
                    status,
                    raw_headers,
                    body,
                    ..
                } => {
                    if should_retry_response(attempt, &status, &raw_headers, &body) {
                        if tx.send(Ok(heartbeat())).await.is_err() {
                            return;
                        }
                        continue;
                    }

                    let is_streaming = retry::is_streaming_response(&raw_headers, &body);
                    tracing::info!(attempt, status = %status, is_streaming, "重试后成功（保活通道）");

                    // 成功：将完整 body 按块转发；若为 SSE，保持 SSE 语义（心跳为注释，不影响解析）。
                    // 空 body 直接结束流，不注入任何上游未发送的字节。
                    if body.is_empty() {
                        return;
                    }
                    const CHUNK_SIZE: usize = 8 * 1024;
                    for chunk in body.as_ref().chunks(CHUNK_SIZE) {
                        let b = Bytes::copy_from_slice(chunk);
                        if tx.send(Ok(b)).await.is_err() {
                            return;
                        }
                    }
                    return;
                }
            }
        }
    });

    // SSE 流式骨架；仅在重试间隙注入 ": keepalive\n\n"（SSE 注释，客户端会忽略）。
    // 哨兵 move 进 map 闭包：Body 被客户端断开而 drop 时闭包销毁 → 置位断开信号。
    let rx_stream = tokio_stream::wrappers::ReceiverStream::new(rx).map(move |r| {
        let _ = &gone_guard;
        r.map_err(|e| std::io::Error::other(e.to_string()))
    });
    let body = Body::from_stream(rx_stream);

    let mut resp = Response::builder().status(StatusCode::OK);
    resp = resp.header(
        http::header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    resp = resp.header(
        http::header::CACHE_CONTROL,
        HeaderValue::from_static("no-cache"),
    );
    // 不设置 connection 头：hyper 按协议自动管理
    resp.body(body).unwrap().into_response()
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

// Response 变体比其他变体大（status + 双头表 + Bytes 共约百余字节）：body 本身
// 已是引用计数的 Bytes，装箱只能省这点栈空间，而本枚举只在单一路径按值传递
// 一次，不构成热点。保持扁平匹配更直接。
#[allow(clippy::large_enum_variant)]
enum ForwardResult {
    NetworkError(String),
    /// 上游响应体超出 `MAX_SPOOL_BYTES`：确定性失败，重试无意义，各通道直接终态处理。
    TooLarge,
    /// 完整缓冲后的响应；`raw_headers` 保留原始 reqwest 头用于流式嗅探，
    /// `headers` 为已过滤 hop-by-hop 后的待透传头。
    Response {
        status: StatusCode,
        headers: HeaderMap,
        raw_headers: reqwest::header::HeaderMap,
        body: Bytes,
    },
}

/// 响应体 spool 上限（字节）：配置 spool_limit_mb（MB，最小 1）换算，
/// 防止失控/恶意上游把内存打爆。超过上限的响应无法完整缓冲，直接按
/// TooLarge 终态处理。
fn spool_limit_bytes(config: &Config) -> usize {
    (config.spool_limit_mb.max(1) as usize).saturating_mul(1024 * 1024)
}

async fn forward_once(
    client: &reqwest::Client,
    method: &http::Method,
    url: &str,
    headers: &HeaderMap,
    body: Bytes,
    max_spool_bytes: usize,
) -> ForwardResult {
    let reqwest_method =
        reqwest::Method::from_bytes(method.as_str().as_bytes()).unwrap_or(reqwest::Method::GET);

    let mut builder = client.request(reqwest_method, url);

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

    if !body.is_empty() {
        // Bytes 直接复用（reqwest From<Bytes> 零拷贝），避免每次重试多拷贝至多 10 MiB
        builder = builder.body(body.clone());
    }

    let resp = match builder.send().await {
        Ok(r) => r,
        Err(e) => return ForwardResult::NetworkError(e.to_string()),
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

    // 关键：完整 spool（分块累积）——任何流式中断都会在此处以 Err 形式暴露，从而触发重试；
    // 超出上限按 TooLarge 终态处理
    let mut spooled: Vec<u8> = Vec::new();
    let mut resp_stream = resp;
    loop {
        match resp_stream.chunk().await {
            Ok(Some(chunk)) => {
                if spooled.len() + chunk.len() > max_spool_bytes {
                    return ForwardResult::TooLarge;
                }
                spooled.extend_from_slice(&chunk);
            }
            Ok(None) => break,
            Err(e) => return ForwardResult::NetworkError(format!("读取上游响应体失败: {e}")),
        }
    }
    let body_bytes = Bytes::from(spooled);

    ForwardResult::Response {
        status,
        headers: resp_headers,
        raw_headers,
        body: body_bytes,
    }
}

fn build_response(status: StatusCode, headers: HeaderMap, body: Bytes) -> Response {
    let mut resp = Response::builder().status(status);
    for (name, value) in headers.iter() {
        resp = resp.header(name, value);
    }
    resp.body(Body::from(body)).unwrap().into_response()
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

    // 将 Bytes 按 8 KiB 切块（copy_from_slice 深拷贝；量级小，成本可忽略）
    const CHUNK_SIZE: usize = 8 * 1024;
    let chunks: Vec<Bytes> = body
        .chunks(CHUNK_SIZE)
        .map(Bytes::copy_from_slice)
        .collect();

    let stream = futures_util::stream::iter(chunks.into_iter().map(Ok::<Bytes, std::io::Error>));
    let body = Body::from_stream(stream);
    resp.body(body).unwrap().into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    // ---- preview_body：二进制/压缩体不污染日志（回归：zstd 错误体曾被
    // from_utf8_lossy 渲染成整片乱码）----

    #[test]
    fn preview_of_binary_shows_hex_not_mojibake() {
        // zstd magic 开头的压缩体（真实日志中出现过）
        let mut zstd_like = vec![0x28, 0xb5, 0x2f, 0xfd];
        zstd_like.extend((0..200u8).map(|i| i.wrapping_mul(37)));
        let p = preview_body(&zstd_like, 500);
        assert!(p.contains("二进制/压缩内容"), "{p}");
        assert!(p.contains("28 b5 2f fd"), "hex 应含 zstd magic: {p}");
        assert!(!p.contains('\u{FFFD}'), "不应输出替换符: {p}");
    }

    #[test]
    fn preview_of_text_is_plain_and_truncated() {
        let text = "上游过载：请稍后重试".repeat(50);
        let p = preview_body(text.as_bytes(), 100);
        assert!(p.contains("上游过载"), "{p}");
        assert!(p.contains("截断"), "{p}");
        // 正常 JSON 错误体
        let json = br#"{"type":"error","error":{"type":"overloaded"}}"#;
        assert_eq!(preview_body(json, 500), String::from_utf8_lossy(json));
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
}
