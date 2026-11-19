//! format 请求方向的重试游标、结果与响应方向的逐层补丁。
use super::*;
use crate::{config::file::Stage, transform};
use aproxy_envelope as wire;

/// 发往上游的最终请求。无格式程序时不创建会话状态，也不为重试复制请求体。
pub(super) struct OutboundRequest {
    /// 当前层生效的 HTTP 方法。
    pub(super) method: http::Method,
    /// 完整的上游地址，可能由路由程序改写。
    pub(super) target_url: String,
    /// 头覆盖和各层补丁之后的头。
    pub(super) headers: HeaderMap,
    /// 磁盘或内存形式的可重放请求体。
    pub(super) body: RequestBody,
    /// 各阶段共享的标识与按程序隔离的 state。
    pub(super) ctx: Option<ExchangeCtx>,
}
/// 每次尝试从自己的游标继续；只有上一轮整条链成功时才回到重试段入口。
pub(super) struct RequestChain {
    /// 上次已成功的各层留下的当前输入。
    input: wire::Request,
    /// 首个 retry 程序之前的缓存，与该层后续改写互不影响。
    prefix: Option<(usize, wire::Request)>,
    /// 尚未成功的程序在配置里的位置。
    cursor: usize,
    /// 当前跑首轮 request 方向还是重跑 retry 方向，层失败时不变。
    stage: Stage,
    /// 下一轮的 retry 信封带着上一次失败；保活也只读这份故障。
    pub(super) last: Option<wire::Failure>,
    /// retry.elapsed_ms 使用单调时钟，系统对时不会让它倒退。
    started: tokio::time::Instant,
}
impl RequestChain {
    /// 只有请求方向有程序时才读整份请求体。失败属于存储，不计程序的熔断。
    pub(super) async fn new(req: &OutboundRequest) -> Result<Self, Terminal> {
        let bytes = match &req.body {
            RequestBody::Memory(b) => b.to_vec(),
            RequestBody::Disk { path, .. } => tokio::fs::read(path).await.map_err(|e| {
                Terminal::local_storage_failed(StorageOp::ReadRequest, &e.to_string())
            })?,
        };
        let mut input = wire::Request::new(req.method.to_string(), req.target_url.clone());
        input.headers = Some(transform::headers_to_btreemap(&req.headers));
        input.body = wire::Body::from_bytes(bytes);
        Ok(Self {
            input,
            prefix: None,
            cursor: 0,
            stage: Stage::Request,
            last: None,
            started: tokio::time::Instant::now(),
        })
    }
    /// 依次派发；在完成一层之后才移动游标。一个请求的局部配置不能通过共享的实例泄漏。
    pub(super) async fn run(
        &mut self,
        state: &AppState,
        req: &mut OutboundRequest,
        attempt: u32,
    ) -> Result<bool, Terminal> {
        let ctx = req.ctx.as_ref().expect("format context");
        if self.cursor == state.formats.len() {
            let Some((i, p)) = &self.prefix else {
                return Ok(false);
            };
            self.cursor = *i;
            self.input = p.clone();
            self.stage = Stage::Retry;
        }
        let mut changed = false;
        while self.cursor < state.formats.len() {
            let pool = state.formats[self.cursor].clone();
            if self.prefix.is_none() && pool.entry.has_stage(Stage::Retry) {
                self.prefix = Some((self.cursor, self.input.clone()));
            }
            if pool.entry.has_stage(self.stage) {
                let name = &pool.entry.name;
                let safe = state.history.secrets().replace(name);
                let stage = self.stage.as_str();
                let m = wire::RequestMessage::new(
                    ctx.context(name),
                    wire::Delivery::Retry,
                    self.input.clone(),
                    ctx.client.clone(),
                );
                let frame = if self.stage == Stage::Retry {
                    wire::Stage::Retry(wire::RetryMessage::new(
                        m,
                        wire::Retry::new(
                            attempt,
                            self.last.clone().expect("retry follows a failure"),
                            self.started.elapsed().as_millis() as u64,
                        ),
                    ))
                } else {
                    wire::Stage::Request(m)
                };
                match pool.call(frame, pool.entry.timeout_secs, false).await {
                    Ok(reply) => {
                        ctx.absorb(name, reply.state);
                        if let Some(f) = reply.fail {
                            let reason = state.history.secrets().replace(&f.reason);
                            return Err(Terminal::format_rejected(&safe, stage, &reason));
                        }
                        if let Some(patch) = reply.request {
                            self.input.apply(patch);
                            changed = true;
                        }
                    }
                    Err(e) if pool.entry.required => {
                        return Err(Terminal::format_failed(
                            &safe,
                            stage,
                            e.kind(),
                            &e.to_string(),
                        ));
                    }
                    Err(e) => {
                        tracing::warn!(format=%safe,stage,error=%e,"optional format layer skipped")
                    }
                }
            }
            self.cursor += 1;
        }
        // 游标也可能是从失败层恢复；此前各层留下的 input 必须交付，不能只看本轮是否有补丁。
        req.method = self.input.method.parse().expect("validated method");
        req.target_url = self.input.url.clone();
        req.headers = transform::headers_from_btreemap(self.input.headers.as_ref().unwrap());
        req.body = transform::spool_request_body(
            self.input.body.as_bytes().unwrap(),
            state.spool_dir.as_deref(),
        )
        .await;
        Ok(changed || self.stage == Stage::Retry)
    }
}
/// 只对最终交付的成功响应逆序派发。整条链都跳过或不改内容时原样交付原始字节（包括压缩）。
pub(super) async fn transform_response_if_configured(
    state: &AppState,
    req: &OutboundRequest,
    mut status: StatusCode,
    mut headers: HeaderMap,
    mut body: SpooledBody,
) -> (StatusCode, HeaderMap, SpooledBody) {
    if !state.has_format(Stage::Response) {
        return (status, headers, body);
    }
    let ctx = req.ctx.as_ref().expect("format context");
    let raw = match &body {
        SpooledBody::Memory(b) => b.to_vec(),
        SpooledBody::Disk { path, .. } => match tokio::fs::read(path).await {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(error=%e,"could not read response spool; format skipped");
                return (status, headers, body);
            }
        },
    };
    let mut input = wire::Response::new(status.as_u16());
    let mut heads = transform::headers_to_btreemap(&headers);
    let decoded = if let Some(enc) = encoding_to_undo(&headers) {
        let mut out = Vec::new();
        if crate::decode::decode_strict(
            &enc,
            raw.as_slice(),
            &mut out,
            spool_limit_bytes(&state.config) as u64,
        )
        .is_err()
        {
            tracing::warn!("could not decode response body; format skipped");
            return (status, headers, body);
        }
        heads.remove("content-encoding");
        out
    } else {
        raw
    };
    input.headers = Some(heads);
    input.body = wire::Body::from_bytes(decoded);
    let mut rewrite_body = false;
    let mut rewrite_headers = false;
    for p in state
        .formats
        .iter()
        .filter(|p| p.entry.has_stage(Stage::Response))
    {
        let name = &p.entry.name;
        let m = wire::Stage::Response(wire::ResponseMessage::new(
            ctx.context(name),
            wire::Request::new(req.method.to_string(), req.target_url.clone()),
            input.clone(),
        ));
        let r = match p.call(m, p.entry.timeout_secs, false).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(format=%state.history.secrets().replace(name),error=%e,"response format layer skipped");
                continue;
            }
        };
        ctx.absorb(name, r.state);
        if let Some(f) = r.fail {
            if let Some(response) = f.response {
                input = response;
                rewrite_body = true;
                rewrite_headers = true;
            }
        } else if let Some(patch) = r.response {
            rewrite_headers |= patch.headers.is_some();
            rewrite_body |= patch.body.as_bytes().is_some();
            input.apply(patch);
        }
    }
    status = StatusCode::from_u16(input.status).expect("validated response status");
    if rewrite_headers {
        headers = transform::headers_from_btreemap(&input.headers.unwrap_or_default());
    }
    if rewrite_body
        || rewrite_headers
            && body.len() != 0
            && headers.get(http::header::CONTENT_ENCODING).is_none()
    {
        body = transform::spool_response_body(
            input.body.as_bytes().unwrap_or_default(),
            state.spool_dir.as_deref(),
        )
        .await;
        headers.remove(http::header::CONTENT_ENCODING);
    }
    (status, headers, body)
}
