//! 代理核心：将本地请求完整透传到上游，并在失败时无限重试。
//!
//! 设计要点：
//! - 单一 upstream URL，其余路径与查询参数完整透传（包含 `/health` 等，上游若有同名路径亦完整透传）
//! - 网络错误 / 4xx / 5xx / 错误 JSON 内容 → 全部无限重试，梯度延迟（`retry` 模块）
//!   原型阶段 4xx 同样视为易发瞬时错误（如限流、临时鉴权波动、上游误报）
//! - 非流式与流式统一缓冲策略：
//!   1. 对上游响应先完整缓冲（spool）到内存，期间任何网络中断（`bytes().await` 失败）
//!      均视为 `NetworkError` 触发重试——满足“流式中断可重试”的需求。
//!   2. 缓冲完成后，再做“是否可重试”的判定：
//!      - HTTP 状态码可重试（4xx / 5xx，`is_retryable_status`）
//!      - 非流式：`is_error_body` 命中（任意状态码下只要 body 语义为报错就重试）
//!      - 流式：`is_stream_error_body` 命中（扫描 SSE data 行 / NDJSON）
//!   3. 仅当整轮缓冲成功且判定为不可重试时，才将缓冲体回放给客户端；
//!      若判定为流式（`is_streaming_response`），则以 chunked 流式分块原样回放，
//!      逐块产出保持与上游字节完全一致，SSE 解析器语义不受影响。
//! - 重试期间对客户端的保活：若上游迟迟不成功，代理不在重试循环中静默等待，而是
//!   在重试间隙向下游（agent 客户端）发送 SSE 注释保活（`: keepalive ...\n\n`），
//!   防止客户端因 idle 超时而断开；同时缓冲与回放流程用分块流式消除首字节假死
//! - 头处理：`api_key` 快捷覆盖 `Authorization: Bearer`，`extra_headers` 追加缺失头，
//!   `override_headers` 无条件覆盖（兼容非 Bearer 鉴权与额外头需求）

use axum::{
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Router,
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
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(300))
            .pool_idle_timeout(Duration::from_secs(90))
            .tcp_keepalive(Duration::from_secs(30))
            .build()
            .expect("构建 reqwest client 失败");
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
    // api_key 快捷：等效覆盖 Authorization: Bearer <key>（大小写不敏感覆盖）
    if let Some(key) = config.api_key.as_deref() {
        let val = format!("Bearer {key}");
        if let Ok(v) = HeaderValue::from_str(&val) {
            headers.remove(http::header::AUTHORIZATION);
            headers.insert(http::header::AUTHORIZATION, v);
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

    // 缓冲请求体以支持重试重放；10 MiB 上限，超出则直接返回 413
    let body_bytes = match axum::body::to_bytes(req.into_body(), 10 * 1024 * 1024).await {
        Ok(b) => b,
        Err(e) => {
            tracing::error!(error = %e, "读取请求体失败");
            return (StatusCode::BAD_REQUEST, format!("读取请求体失败: {e}")).into_response();
        }
    };

    let path_and_query = uri
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or("/");

    let upstream_base = state.config.upstream_url.trim_end_matches('/');
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

    // 若需保活，则以流式 Body 立即响应，后台在重试间隙穿插 SSE 注释心跳，避免客户端 idle 超时
    if keepalive_enabled && client_wants_sse {
        return proxy_with_keepalive(state.clone(), method, target_url, headers, body_bytes).await;
    }

    // 非 SSE 客户端：仍走原有的重试循环（响应在成功后一次性返回）
    proxy_without_keepalive(state, method, target_url, headers, body_bytes).await
}

async fn proxy_without_keepalive(
    state: AppState,
    method: http::Method,
    target_url: String,
    headers: HeaderMap,
    body_bytes: Bytes,
) -> Response {
    let mut attempt: u32 = 0;
    loop {
        attempt += 1;
        if attempt > 1 {
            let delay = retry::delay_for_attempt(attempt - 1);
            if !delay.is_zero() {
                tracing::warn!(attempt, delay_ms = delay.as_millis() as u64, "重试延迟");
                tokio::time::sleep(delay).await;
            } else {
                tracing::warn!(attempt, "立即重试");
            }
        }

        let result = forward_once(&state.client, &method, &target_url, &headers, body_bytes.clone()).await;

        match result {
            ForwardResult::NetworkError(e) => {
                tracing::warn!(attempt, error = %e, "上游网络错误，重试");
                continue;
            }
            ForwardResult::Response {
                status,
                headers: resp_headers,
                raw_headers,
                body,
            } => {
                let is_streaming = retry::is_streaming_response(&raw_headers, &body);

                // 1. HTTP 状态码可重试：无论是否流式，都重试
                if retry::is_retryable_status(status.as_u16()) {
                    tracing::warn!(attempt, status = %status, is_streaming, "上游返回可重试状态码，重试");
                    let preview = String::from_utf8_lossy(&body[..body.len().min(500)]);
                    tracing::warn!(preview = %preview, "错误响应预览");
                    continue;
                }

                // 2. 内容错误检测：对所有错误都重试——原型期 4xx 亦视为易发生的瞬时错误。
                //    即使状态码未命中可重试（如 200 携带 error JSON），只要 body 语义为报错就重试。
                let is_error = if is_streaming {
                    retry::is_stream_error_body(&body)
                } else {
                    retry::is_error_body(&body)
                };
                if is_error {
                    let preview = String::from_utf8_lossy(&body[..body.len().min(1000)]);
                    tracing::warn!(attempt, is_streaming, preview = %preview, "上游返回错误内容，重试");
                    continue;
                }

                // 3. 成功：按是否流式选择回放方式，保证“原样流式”
                if attempt > 1 {
                    tracing::info!(attempt, status = %status, is_streaming, "重试后成功");
                }
                if is_streaming {
                    return build_stream_replay_response(status, resp_headers, body);
                } else {
                    return build_response(status, resp_headers, body);
                }
            }
        }
    }
}

/// 带保活的代理：立即以 SSE 流响应，在重试间隙发送 `: keepalive\n\n`，成功后无缝拼接上游响应
async fn proxy_with_keepalive(
    state: AppState,
    method: http::Method,
    target_url: String,
    headers: HeaderMap,
    body_bytes: Bytes,
) -> Response {
    let keepalive_dur = state.config.keepalive_interval();
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(32);

    // 后台任务：无限重试上游，期间按 keepalive_dur 发送 SSE 注释；成功后将上游响应分块转发
    let state_bg = state.clone();
    tokio::spawn(async move {
        let mut attempt: u32 = 0;
        loop {
            attempt += 1;
            if attempt > 1 {
                let delay = retry::delay_for_attempt(attempt - 1);
                if !delay.is_zero() {
                    // 在延迟期间按 keepalive_dur 切片发送心跳，避免客户端 idle 超时
                    let mut elapsed = Duration::ZERO;
                    while elapsed < delay {
                        let slice = std::cmp::min(keepalive_dur, delay - elapsed);
                        tokio::time::sleep(slice).await;
                        elapsed += slice;
                        if elapsed < delay {
                            // 仅在尚未到下一轮重试时发送心跳
                            let hb = Bytes::from_static(b": keepalive\n\n");
                            if tx.send(Ok(hb)).await.is_err() {
                                return;
                            }
                        }
                    }
                } else {
                    tracing::warn!(attempt, "立即重试（保活通道）");
                }
            }

            let result = forward_once(&state_bg.client, &method, &target_url, &headers, body_bytes.clone()).await;

            match result {
                ForwardResult::NetworkError(e) => {
                    tracing::warn!(attempt, error = %e, "上游网络错误，重试（保活通道）");
                    let hb = Bytes::from_static(b": keepalive\n\n");
                    let _ = tx.send(Ok(hb)).await;
                    continue;
                }
                ForwardResult::Response {
                    status,
                    headers: _resp_headers,
                    raw_headers,
                    body,
                } => {
                    let is_streaming = retry::is_streaming_response(&raw_headers, &body);

                    if retry::is_retryable_status(status.as_u16()) {
                        tracing::warn!(attempt, status = %status, is_streaming, "上游返回可重试状态码，重试（保活通道）");
                        let hb = Bytes::from_static(b": keepalive\n\n");
                        let _ = tx.send(Ok(hb)).await;
                        continue;
                    }

                    let is_error = if is_streaming {
                        retry::is_stream_error_body(&body)
                    } else {
                        retry::is_error_body(&body)
                    };
                    if is_error {
                        tracing::warn!(attempt, is_streaming, "上游返回错误内容，重试（保活通道）");
                        let hb = Bytes::from_static(b": keepalive\n\n");
                        let _ = tx.send(Ok(hb)).await;
                        continue;
                    }

                    if attempt > 1 {
                        tracing::info!(attempt, status = %status, is_streaming, "重试后成功（保活通道）");
                    }

                    // 成功：将完整 body 按块转发；若为 SSE，保持 SSE 语义（心跳为注释，不影响解析）
                    if body.is_empty() {
                        let _ = tx.send(Ok(Bytes::from_static(b"data: [DONE]\n\n"))).await;
                    } else {
                        const CHUNK_SIZE: usize = 8 * 1024;
                        for chunk in body.as_ref().chunks(CHUNK_SIZE) {
                            let b = Bytes::copy_from_slice(chunk);
                            if tx.send(Ok(b)).await.is_err() {
                                return;
                            }
                        }
                    }
                    return;
                }
            }
        }
    });

    // 立即返回 SSE 流；首轮成功时不注入任何额外字节，仅在重试间隙注入 ": keepalive\n\n"（SSE 注释，客户端会忽略）
    let rx_stream = tokio_stream::wrappers::ReceiverStream::new(rx)
        .map(|r| r.map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string())));
    let body = Body::from_stream(rx_stream);

    let mut resp = Response::builder().status(StatusCode::OK);
    resp = resp.header(http::header::CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
    resp = resp.header(http::header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    resp = resp.header(http::header::CONNECTION, HeaderValue::from_static("keep-alive"));
    resp.body(body).unwrap().into_response()
}

enum ForwardResult {
    NetworkError(String),
    /// 完整缓冲后的响应；`raw_headers` 保留原始 reqwest 头用于流式嗅探，
    /// `headers` 为已过滤 hop-by-hop 后的待透传头。
    Response {
        status: StatusCode,
        headers: HeaderMap,
        raw_headers: reqwest::header::HeaderMap,
        body: Bytes,
    },
}

async fn forward_once(
    client: &reqwest::Client,
    method: &http::Method,
    url: &str,
    headers: &HeaderMap,
    body: Bytes,
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
        if let Ok(n) = reqwest::header::HeaderName::from_bytes(name_str.as_bytes()) {
            if let Ok(v) = reqwest::header::HeaderValue::from_bytes(value.as_bytes()) {
                builder = builder.header(n, v);
            }
        }
    }

    if !body.is_empty() {
        builder = builder.body(body.to_vec());
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
        if let Ok(n) = HeaderName::from_bytes(name_str.as_bytes()) {
            if let Ok(v) = HeaderValue::from_bytes(value.as_bytes()) {
                resp_headers.insert(n, v);
            }
        }
    }

    // 关键：完整 spool——任何流式中断都会在此处以 Err 形式暴露，从而触发重试
    let body_bytes = match resp.bytes().await {
        Ok(b) => b,
        Err(e) => return ForwardResult::NetworkError(format!("读取上游响应体失败: {e}")),
    };

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

    // 将 Bytes 按 8 KiB 切块，零拷贝（Bytes::slice 引用同一内存）
    const CHUNK_SIZE: usize = 8 * 1024;
    let chunks: Vec<Bytes> = body
        .chunks(CHUNK_SIZE)
        .map(Bytes::copy_from_slice)
        .collect();

    let stream = futures_util::stream::iter(chunks.into_iter().map(Ok::<Bytes, std::io::Error>));
    let body = Body::from_stream(stream);
    resp.body(body).unwrap().into_response()
}
