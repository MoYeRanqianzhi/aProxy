//! format 请求方向的重试游标、结果与响应方向的逐层补丁。
use super::*;
use crate::{
    config::file::Stage,
    transform::{self, Fault},
};
use aproxy_envelope as wire;
use serde_json::json;

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
    prefix: Option<(usize, wire::Request, HeaderMap, u64)>,
    /// 未被补丁整表替换的头保留原来的多值和非 ASCII 字节。
    headers: HeaderMap,
    /// 正文补丁的版本、下一次可分配的版本和上次交给上游的版本。
    body_version: u64,
    next_body_version: u64,
    sent_body_version: u64,
    /// 保活触发条件只用客户端原样的 Accept 和请求体，不受上游改写影响。
    accept: bool,
    body_stream: bool,
    /// 尚未成功的程序在配置里的位置。
    cursor: usize,
    /// 当前跑首轮 request 方向还是重跑 retry 方向，层失败时不变。
    stage: Stage,
    /// 下一轮的 retry 信封带着上一次失败；保活也只读这份故障。
    pub(super) last: Option<wire::Failure>,
    /// retry.elapsed_ms 使用单调时钟，系统对时不会让它倒退。
    started: tokio::time::Instant,
    /// 程序对此请求给出的重试上限，0 是显式无限制。
    pub(super) max_attempts: Option<u32>,
}
/// 程序可以否决请求、使本次尝试失败，或无法读取本地 spool；上游尚未收到请求。
pub(super) enum ChainError {
    /// 进入普通重试循环，保持失败层与它的输入。
    Retry(AttemptFailure),
    /// 等子进程期间客户端断开，不计失败也不再访问上游。
    Disconnected,
    /// 请求立即结束，未提交时可用 format 给出的客户端响应。
    Stop(Terminal, Option<wire::Response>),
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
        input.headers = transform::headers_to_btreemap(&req.headers);
        input.body = wire::Body::from_bytes(bytes);
        Ok(Self {
            input,
            prefix: None,
            headers: req.headers.clone(),
            body_version: 0,
            next_body_version: 1,
            sent_body_version: 0,
            accept: accepts_event_stream(&req.headers),
            body_stream: request_body_wants_stream(&req.body).await,
            cursor: 0,
            stage: Stage::Request,
            last: None,
            started: tokio::time::Instant::now(),
            max_attempts: None,
        })
    }
    /// 依次派发；在完成一层之后才移动游标。一个请求的局部配置不能通过共享的实例泄漏。
    pub(super) async fn run(
        &mut self,
        state: &mut AppState,
        req: &mut OutboundRequest,
        attempt: u32,
        responder: &mut Responder,
    ) -> Result<bool, ChainError> {
        let ctx = req.ctx.as_ref().expect("format context");
        if self.cursor == state.formats.len() {
            let Some((i, p, h, v)) = &self.prefix else {
                return Ok(false);
            };
            self.cursor = *i;
            self.input = p.clone();
            self.headers = h.clone();
            self.body_version = *v;
            self.stage = Stage::Retry;
        }
        while self.cursor < state.formats.len() {
            let pool = state.formats[self.cursor].clone();
            if self.prefix.is_none() && pool.entry.has_stage(Stage::Retry) {
                self.prefix = Some((
                    self.cursor,
                    self.input.clone(),
                    self.headers.clone(),
                    self.body_version,
                ));
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
                match responder
                    .wait(
                        pool.call(frame, pool.entry.timeout_secs, false),
                        None,
                        "request format",
                    )
                    .await
                    .ok_or(ChainError::Disconnected)?
                {
                    Ok(reply) => {
                        if self.stage == Stage::Request
                            && reply
                                .delivery
                                .as_ref()
                                .is_some_and(|v| !matches!(v.as_str(), Some("forward" | "retry")))
                        {
                            tracing::warn!(format=%safe,"invalid format delivery ignored");
                        }
                        if let wire::ReportField::Present(report) = reply.report {
                            ctx.rec.format_report(
                                name,
                                stage,
                                (self.stage == Stage::Retry).then_some(attempt),
                                report,
                            );
                        }
                        ctx.absorb(name, reply.state);
                        if let Some(f) = reply.fail {
                            let reason: String = state
                                .history
                                .secrets()
                                .replace(&f.reason)
                                .chars()
                                .take(512)
                                .collect();
                            let mut details = json!({"format":safe,"stage":stage});
                            if let Some(code) = f.code {
                                details["fail_code"] =
                                    json!(state.history.secrets().replace(&code));
                            }
                            return if f.retry {
                                Err(ChainError::Retry(AttemptFailure::new(
                                    "format_fail",
                                    reason,
                                    details,
                                )))
                            } else {
                                Err(ChainError::Stop(
                                    Terminal::format_rejected(&safe, &reason, details),
                                    f.response,
                                ))
                            };
                        }
                        if let Some(patch) = reply.request {
                            if let Some(h) = &patch.headers {
                                self.headers = transform::headers_from_btreemap(h);
                            }
                            if patch.body.as_bytes().is_some() {
                                self.body_version = self.next_body_version;
                                self.next_body_version += 1;
                            }
                            self.input.apply(patch);
                        }
                        if let Some(overrides) = reply.overrides {
                            self.apply_overrides(state, overrides, responder);
                        }
                    }
                    Err(Fault::Open { kind, probe_at }) if pool.entry.required => {
                        return Err(ChainError::Stop(
                            Terminal::new(
                                "format_unavailable",
                                format!("format '{safe}' is unavailable (circuit open)"),
                                json!({"format":safe,"stage":stage,"kind":kind,"probe_at":probe_at}),
                            ),
                            None,
                        ));
                    }
                    Err(e) if pool.entry.required => {
                        return Err(ChainError::Retry(AttemptFailure::new(
                            "format_error",
                            format!("format '{safe}': {e}"),
                            json!({"format":safe,"stage":stage,"kind":e.kind()}),
                        )));
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
        req.headers = self.headers.clone();
        let changed = self.body_version != self.sent_body_version;
        if changed {
            self.sent_body_version = self.body_version;
            req.body = transform::spool_request_body(
                self.input.body.as_bytes().unwrap(),
                state.spool_dir.as_deref(),
            )
            .await;
        }
        Ok(changed)
    }
    /// 只允许 hello 列明的叶键；单个非法键不作废同一回信里的其他键，也不记录它的值。
    fn apply_overrides(
        &mut self,
        state: &mut AppState,
        overrides: wire::Overrides,
        responder: &mut Responder,
    ) {
        for (group, value) in overrides {
            let Some(map) = value.as_object() else {
                tracing::warn!("invalid format override ignored");
                continue;
            };
            for (k, v) in map {
                let n = v.as_u64();
                let cfg = Arc::make_mut(&mut state.config);
                match (group.as_str(), k.as_str()) {
                    ("timeouts", "connect_secs") if n.is_some() => {
                        cfg.connect_timeout_secs = n.unwrap();
                        state.client = AppState::upstream_client(cfg);
                    }
                    ("timeouts", "first_byte_secs") if n.is_some() => {
                        cfg.first_byte_timeout_secs = n.unwrap()
                    }
                    ("timeouts", "read_idle_secs") if n.is_some() => {
                        cfg.read_timeout_secs = n.unwrap()
                    }
                    ("retry", "max_backoff_secs") if n.is_some() => {
                        cfg.max_retry_backoff_secs = n.unwrap()
                    }
                    ("retry", "max_attempts") if n.is_some_and(|n| u32::try_from(n).is_ok()) => {
                        self.max_attempts = Some(n.unwrap() as u32)
                    }
                    ("keepalive", _) if responder.is_committed() => {}
                    ("keepalive", "interval_secs")
                        if n.is_some_and(|n| {
                            tokio::time::Instant::now()
                                .checked_add(Duration::from_secs(n))
                                .is_some()
                        }) =>
                    {
                        cfg.keepalive_interval_secs = n.unwrap()
                    }
                    ("keepalive", "trigger")
                        if v.as_str() == Some("always")
                            || v.as_str()
                                .and_then(crate::config::KeepaliveTrigger::parse)
                                .is_some() =>
                    {
                        cfg.keepalive_trigger = v.as_str().map(String::from)
                    }
                    ("keepalive", "heartbeat")
                        if v.as_str()
                            .is_some_and(|s| crate::config::heartbeat_problem(s).is_none()) =>
                    {
                        cfg.keepalive_heartbeat = v.as_str().map(String::from)
                    }
                    _ => tracing::warn!("invalid or unknown format override ignored"),
                }
            }
            if group == "keepalive" {
                responder.configure_beats(&state.config, self.accept, self.body_stream);
            }
        }
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
    let encoding = encoding_to_undo(&headers);
    let decoded = if let Some(enc) = &encoding {
        let mut out = Vec::new();
        if crate::decode::decode_strict(
            enc,
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
    input.headers = heads;
    input.body = wire::Body::from_bytes(decoded);
    let mut rewrite_body = false;
    let mut rewrite_headers = false;
    for p in state
        .formats
        .iter()
        .rev()
        .filter(|p| p.entry.has_stage(Stage::Response))
    {
        let name = &p.entry.name;
        let m = wire::Stage::Response(wire::ResponseMessage::new(
            ctx.context(name),
            wire::RequestMeta::new(req.method.to_string(), req.target_url.clone()),
            input.clone(),
        ));
        let result = match p.call(m.clone(), p.entry.timeout_secs, false).await {
            Err(e) if !matches!(e, Fault::Open { .. }) => {
                p.call(m, p.entry.timeout_secs, true).await
            }
            other => other,
        };
        let r = match result {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(format=%state.history.secrets().replace(name),error=%e,"response format layer skipped");
                continue;
            }
        };
        if let wire::ReportField::Present(report) = r.report {
            ctx.rec.format_report(name, "response", None, report);
        }
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
        headers = transform::headers_from_btreemap(&input.headers);
    }
    if rewrite_body || rewrite_headers && encoding.is_some() {
        body = transform::spool_response_body(
            input.body.as_bytes().unwrap_or_default(),
            state.spool_dir.as_deref(),
        )
        .await;
    }
    if rewrite_body || rewrite_headers {
        headers.remove(http::header::CONTENT_ENCODING);
        headers.remove(http::header::CONTENT_LENGTH);
    }
    (status, headers, body)
}
