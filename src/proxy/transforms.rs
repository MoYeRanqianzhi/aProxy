//! 外部转换器（format）的接线：请求转换、按次重新转换、响应转换。

use super::*;

/// 发往上游的请求：请求转换（若配置）之后的最终形态，重试循环的每一轮原样重放。
/// `ctx` 随请求带到响应转换（同一 request_id、请求转换留下的 state）。
pub(super) struct OutboundRequest {
    pub(super) method: http::Method,
    pub(super) target_url: String,
    pub(super) headers: HeaderMap,
    pub(super) body: RequestBody,
    pub(super) ctx: ExchangeCtx,
    /// 请求转换开了 `every_attempt` 时由 [`apply_request_transform`] 填上：每次重试
    /// 之前据此重新转换（见 [`PerAttemptTransform`]）
    pub(super) retransform: Option<PerAttemptTransform>,
}

/// 请求转换的「每次尝试」模式（`[request_transform] every_attempt = true`）：留着客户端
/// 的原始请求，每次重试之前用它再跑一次请求转换（信封 `stage = "retry"`，附上一次
/// 失败的状态码或原因），回信替换这次尝试发出的请求。用途是故障转移：format 换
/// key、换渠道，无限重试就不会一直打同一个坏掉的上游。
///
/// 这一次的转换失败（format 拒绝、超时、输出不合法）不终止请求——那会违背无限重试；
/// 沿用上一次发出的请求，每个请求只告警一次。代价：原始请求体在请求的整个生命期
/// 留在内存里（转换本来就要整份 body）。
pub(super) struct PerAttemptTransform {
    pub(super) method: http::Method,
    pub(super) url: String,
    pub(super) headers: HeaderMap,
    pub(super) body: Bytes,
    /// 上一次尝试的失败（`attempt` 在调用时填）
    pub(super) last: aproxy_envelope::RetryInfo,
    pub(super) warned: bool,
}

impl PerAttemptTransform {
    /// 记下一次尝试的结果；下一次重新转换时随信封交给 format。成功的结果也会记，
    /// 无妨：成功之后不会再有重试。
    pub(super) fn record(&mut self, result: &ForwardResult) {
        let (status, error) = match result {
            ForwardResult::NetworkError(failure) => (None, failure.message.clone()),
            ForwardResult::Response { status, .. } => (
                Some(status.as_u16()),
                format!("the upstream returned {}", status.as_u16()),
            ),
            ForwardResult::TooLarge | ForwardResult::SpoolFailed(..) => return,
        };
        self.last.status = status;
        self.last.error = Some(error);
    }

    /// 第 `attempt` 次尝试（2 起）之前重新转换。返回这次要发的请求；失败返回 None，
    /// 调用方沿用上一次的。
    pub(super) async fn next(
        &mut self,
        state: &AppState,
        ctx: &ExchangeCtx,
        attempt: u32,
    ) -> Option<crate::transform::TransformedRequest> {
        let pool = state.request_pool.as_ref()?;
        let mut retry = self.last.clone();
        retry.attempt = attempt;
        match crate::transform::transform_request(
            pool,
            ctx,
            &self.method,
            &self.url,
            &self.headers,
            RequestBody::Memory(self.body.clone()),
            Some(retry),
            state.spool_dir.as_deref(),
        )
        .await
        {
            Ok(t) => {
                tracing::info!(
                    attempt,
                    url = %aproxy_ipc::redact::url(&t.url),
                    "重试前请求已由外部转换器重新改写"
                );
                Some(t)
            }
            Err(e) => {
                if !self.warned {
                    self.warned = true;
                    tracing::warn!(
                        attempt,
                        error = %e,
                        "重试前的请求转换失败，沿用上一次发出的请求（本请求只告警这一次）"
                    );
                }
                None
            }
        }
    }
}

/// 请求转换器：缓冲完成后交给外部 format 程序改写（body/headers/url/method
/// 全部可变）——默认转换**一次**，产物被重试循环的每一轮 forward_once 原样重放；
/// `every_attempt` 打开时顺带留下原始请求，供每次重试前重新转换（见
/// [`PerAttemptTransform`]）。位置约束：必须在首轮 forward_once 之前（否则重放的是未
/// 转换请求）、重试规则的路径匹配之后（规则对原始路径判定——路径集合是
/// 客户端视角，转换是实例级配置，语义不同源）。未配置时原样返回。
///
/// 失败返回这个请求的终态（已记日志与最近一次失败）：请求未发往上游，按约定
/// 不重试（失败按成因归成不同的码，见 [`request_transform_terminal`]）。
/// 调用方按响应是否已提交决定回错误响应还是写终态 error 事件。
pub(super) async fn apply_request_transform(
    state: &AppState,
    req: OutboundRequest,
) -> Result<OutboundRequest, Terminal> {
    let Some(pool) = state.request_pool.as_ref() else {
        return Ok(req);
    };
    let OutboundRequest {
        method,
        target_url,
        headers,
        body,
        ctx,
        ..
    } = req;
    // every_attempt：原始请求留一份给每次重试重新转换。磁盘上的体读进内存（转换
    // 本来就要整份 body），首次转换也改用内存这份，不必再读一遍
    let every_attempt = state
        .config
        .request_transform
        .as_ref()
        .is_some_and(|t| t.every_attempt);
    let (body, retransform) = if every_attempt {
        let bytes = match &body {
            RequestBody::Memory(b) => b.clone(),
            RequestBody::Disk { path, .. } => match tokio::fs::read(path).await {
                Ok(v) => Bytes::from(v),
                Err(e) => {
                    let terminal =
                        Terminal::local_storage_failed(StorageOp::ReadRequest, &e.to_string());
                    state.note_upstream_failure(&terminal.summary());
                    tracing::error!(code = %terminal.code(), error = %e, "请求转换前读取落盘的请求体失败");
                    return Err(terminal);
                }
            },
        };
        let original = PerAttemptTransform {
            method: method.clone(),
            url: target_url.clone(),
            headers: headers.clone(),
            body: bytes.clone(),
            last: Default::default(),
            warned: false,
        };
        (RequestBody::Memory(bytes), Some(original))
    } else {
        (body, None)
    };
    match crate::transform::transform_request(
        pool,
        &ctx,
        &method,
        &target_url,
        &headers,
        body,
        None,
        state.spool_dir.as_deref(),
    )
    .await
    {
        Ok(t) => {
            tracing::info!(url = %aproxy_ipc::redact::url(&t.url), "请求已由外部转换器改写");
            Ok(OutboundRequest {
                method: t.method,
                target_url: t.url,
                headers: t.headers,
                body: t.body,
                ctx,
                retransform,
            })
        }
        Err(e) => {
            let terminal = request_transform_terminal(&request_format_name(state), &e);
            state.note_upstream_failure(&terminal.summary());
            tracing::warn!(code = %terminal.code(), error = %e, "请求转换失败，终态返回（请求侧转换失败按约定不重试）");
            Err(terminal)
        }
    }
}

/// 请求侧 format 程序在错误体与日志里的名字：配置里它的 `name`（没写时是 `command` 的文件名
/// 主干）。不经配置文件、直接构造出来的配置可能没有名字，那时用 `request` 指代它。
fn request_format_name(state: &AppState) -> String {
    state
        .config
        .request_transform
        .as_ref()
        .map(|t| t.name.as_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("request")
        .to_string()
}

/// 请求转换失败对应的终态：
/// - format 用信封的 error 行明确拒绝了这个请求 → `format_rejected`；
/// - 落盘的请求体读不回来（本地磁盘的问题，与 format 无关）→ `local_storage_failed`；
/// - 其余是 format 程序本身出了问题（起不来、管道断了、超时、中途退出、输出不合协议）→
///   `format_failed`，`kind` 说明是哪一种。
fn request_transform_terminal(format: &str, e: &crate::transform::TransformError) -> Terminal {
    use crate::transform::TransformError as E;
    let kind = match e {
        E::Rejected(reason) => {
            return Terminal::format_rejected(format, "request", reason);
        }
        E::Spool(io) => {
            return Terminal::local_storage_failed(StorageOp::ReadRequest, &io.to_string());
        }
        E::Spawn(_) => "spawn",
        E::Io(_) | E::Closed => "io",
        E::TimedOut => "timeout",
        E::WorkerDied => "died",
        E::Protocol(_) | E::Serialize(_) => "protocol",
    };
    Terminal::format_failed(format, "request", kind, &e.to_string())
}

/// 响应侧转换入口（尝试循环交付成功响应时调用）：未配置时原样返回；转换失败时 warn + 原样
/// 返回（透传上游原始响应——响应已在手，可用性优先，用户已定语义）。
/// is_streaming 由调用方按返回值重算（format 可改 content-type/body 形态）。
pub(super) async fn transform_response_if_configured(
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
