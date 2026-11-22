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
    /// 最终正文若有改写，游标、前缀与上游只持同一份可重放体的引用。
    pub(super) replacement: Option<Arc<RequestBody>>,
    /// 客户端原本的编码，用于比较首次发出体与历史所存原体。
    pub(super) client_encoding: Option<HeaderValue>,
    /// 保活判定看客户端给出的 Accept，不看上游配置覆盖之后的头。
    pub(super) client_accept: bool,
    /// 为初始保活判定扫描过客户端正文时，保存其流标记。
    pub(super) body_stream: Option<bool>,
    /// 各阶段共享的标识与按程序隔离的 state。
    pub(super) ctx: Option<ExchangeCtx>,
}
impl OutboundRequest {
    /// 正文未改写时直接借用原请求，未配置 format 的路径不分配共享体。
    pub(super) fn sent_body(&self) -> &RequestBody {
        self.replacement.as_deref().unwrap_or(&self.body)
    }
}
/// 只在程序确实给出覆盖时保留入口配置；重跑时只恢复可覆盖的叶键，不撤销投递升级。
#[derive(Clone)]
struct OverrideInput {
    /// 共享当时的配置，不因普通的无覆盖请求复制配置或凭据。
    config: Arc<Config>,
    /// 连接超时属于连接池；恢复原 client 也保留其可复用的连接。
    client: reqwest::Client,
    /// 程序设置的次数上限独立于配置文件中的重试规则。
    max_attempts: Option<u32>,
}
impl OverrideInput {
    /// 在改写前记录重试段或失败层的有效输入。
    fn capture(state: &AppState, max_attempts: Option<u32>) -> Self {
        Self {
            config: state.config.clone(),
            client: state.client.clone(),
            max_attempts,
        }
    }
    /// 还原程序可以修改的字段。已提交的 SSE 保活不能因切换渠道而变回未提交配置。
    fn restore(self, state: &mut AppState, chain: &mut RequestChain, responder: &mut Responder) {
        let cfg = Arc::make_mut(&mut state.config);
        cfg.connect_timeout_secs = self.config.connect_timeout_secs;
        cfg.first_byte_timeout_secs = self.config.first_byte_timeout_secs;
        cfg.read_timeout_secs = self.config.read_timeout_secs;
        cfg.max_retry_backoff_secs = self.config.max_retry_backoff_secs;
        state.client = self.client;
        chain.max_attempts = self.max_attempts;
        if !responder.is_committed() {
            cfg.keepalive_interval_secs = self.config.keepalive_interval_secs;
            cfg.keepalive_trigger
                .clone_from(&self.config.keepalive_trigger);
            cfg.keepalive_heartbeat
                .clone_from(&self.config.keepalive_heartbeat);
            responder.configure_beats(cfg, chain.accept, chain.body_stream.unwrap_or(false));
        }
    }
}
/// 首个 retry 层前的输入。磁盘正文只留句柄，不因排队或保留前缀而整份进内存。
#[derive(Clone)]
struct Prefix {
    /// 重试段第一次改变覆盖之前的有效配置；前缀已经生效的值属于输入。
    overrides: Option<OverrideInput>,
    /// 重试段的首层位置。
    cursor: usize,
    /// 正文之外的协议字段。
    input: wire::Request,
    /// 与缓存输入同一时刻的已解析 HTTP 方法。
    method: http::Method,
    /// 上游可见的完整头表。
    headers: HeaderMap,
    /// 前缀改写体；缺省时仍是客户端原体。
    body: Option<Arc<RequestBody>>,
    /// 回到这个前缀时，正文和编码是否仍是客户端原样。
    prepare_original: bool,
    /// 对应的请求历史版本。
    version: u64,
}
/// 每次尝试从自己的游标继续；只有上一轮整条链成功时才回到重试段入口。
pub(super) struct RequestChain {
    /// 上次已成功的各层留下的当前输入。
    input: wire::Request,
    /// 首个 retry 程序之前的缓存，与该层后续改写互不影响。
    prefix: Option<Prefix>,
    /// 失败回信仍可限制当次尝试，但重跑同一层之前必须撤掉该回信的覆盖。
    failed_overrides: Option<OverrideInput>,
    /// 当前改写体与重试前缀、上游重放共享；未改写时从客户端原体读取。
    body: Option<Arc<RequestBody>>,
    /// 未被补丁整表替换的头保留原来的多值和非 ASCII 字节。
    headers: HeaderMap,
    /// 正文补丁的版本、下一次可分配的版本和上次交给上游的版本。
    body_version: u64,
    next_body_version: u64,
    sent_body_version: u64,
    /// 即使字节不变，编码的差异也会影响历史解码后实际保留的内容。
    sent_encoding: Option<HeaderValue>,
    /// 保活触发条件只用客户端原样的 Accept 和请求体，不受上游改写影响。
    accept: bool,
    pub(super) body_stream: Option<bool>,
    /// 原体直到遇到需要 full 的层才准备，之后不解码程序指定的表示。
    prepare_original: bool,
    /// 尚未成功的程序在配置里的位置。
    cursor: usize,
    /// 当前跑首轮 request 方向还是重跑 retry 方向，层失败时不变。
    stage: Stage,
    /// 下一轮的 retry 信封带着上一次失败；保活也只读这份故障。
    pub(super) last: Option<wire::Failure>,
    /// 只属于紧邻的上次尝试，不交给心跳；链内失败时清空。
    pub(super) last_response: Option<wire::ResponseHead>,
    /// retry.elapsed_ms 使用单调时钟，系统对时不会让它倒退。
    started: tokio::time::Instant,
    /// 程序对此请求给出的重试上限，0 是显式无限制。
    pub(super) max_attempts: Option<u32>,
    /// 第一次有效表态或 decide 升级后锁定；从失败层重入时也不能重复选。
    pub(super) delivery_chosen: bool,
}
/// 主动结束时交付程序的响应以及通过容量校验的 HTTP 头。
pub(super) type ProgramResponse = (wire::Response, HeaderMap);

/// 程序可以否决请求、使本次尝试失败，或无法读取本地 spool；上游尚未收到请求。
pub(super) enum ChainError {
    /// 进入普通重试循环，保持失败层与它的输入。
    Retry(AttemptFailure),
    /// 等子进程期间客户端断开，不计失败也不再访问上游。
    Disconnected,
    /// 请求立即结束，未提交时可用 format 给出的客户端响应。
    Stop(Box<(Terminal, Option<ProgramResponse>)>),
}
impl ChainError {
    /// 终态可能携带完整响应；只在确定结束时为这份诊断分配，不撑大普通往返。
    fn stop(terminal: Terminal, response: Option<ProgramResponse>) -> Self {
        Self::Stop(Box::new((terminal, response)))
    }
}
impl RequestChain {
    /// 游标只记录元数据，原始正文直到已分配 worker 才展开。
    pub(super) fn new(req: &OutboundRequest) -> Self {
        let mut input = wire::Request::new(req.method.to_string(), req.target_url.clone());
        input.headers = transform::headers_to_btreemap(&req.headers);
        Self {
            input,
            prefix: None,
            failed_overrides: None,
            body: None,
            headers: req.headers.clone(),
            body_version: 0,
            next_body_version: 1,
            sent_body_version: 0,
            sent_encoding: req.client_encoding.clone(),
            accept: req.client_accept,
            body_stream: req.body_stream,
            prepare_original: true,
            cursor: 0,
            stage: Stage::Request,
            last: None,
            last_response: None,
            started: tokio::time::Instant::now(),
            max_attempts: None,
            delivery_chosen: false,
        }
    }
    /// 在进程接纳之后、首帧写出之前准备原体；所有后续层和重试都复用同一份表示。
    async fn prepare_body(
        &mut self,
        state: &AppState,
        body: &mut RequestBody,
        current: &wire::HelloReply,
    ) -> Result<(), ChainError> {
        if self.prepare_original
            && let Some(encoding) = request_encoding(&self.headers, true).map(|v| v.into_owned())
            && state.formats.iter().enumerate().any(|(i, pool)| {
                let needs_full = |hello: &wire::HelloReply| {
                    [Stage::Request, Stage::Retry].into_iter().any(|stage| {
                        pool.entry.has_stage(stage)
                            && matches!(
                                hello.request_body.get(stage.as_str()),
                                None | Some(wire::BodyNeed::Full)
                            )
                    })
                };
                if i == self.cursor {
                    needs_full(current)
                } else {
                    // 尚未借到的后续层以最近一次成功 hello 为准，不为预测将来换进程而占池。
                    pool.available_for_message()
                        && pool
                            .negotiated
                            .lock()
                            .unwrap()
                            .as_deref()
                            .is_some_and(needs_full)
                }
            })
        {
            self.prepare_original = false;
            if let Some(prefix) = &mut self.prefix {
                prefix.prepare_original = false;
            }
            let limit = state.config.body_limit_bytes() as u64;
            let dir = state.spool_dir.clone();
            let original = std::mem::replace(body, RequestBody::Memory(Bytes::new()));
            let (cancel, alive) = tokio::sync::oneshot::channel::<()>();
            let task = tokio::task::spawn_blocking(move || match encoding.to_str() {
                Ok(enc) => original.decode_for_format(enc, dir, limit, || cancel.is_closed()),
                Err(_) => (original, false),
            });
            let (prepared, decoded) = task.await.map_err(|e| {
                ChainError::stop(
                    Terminal::local_storage_failed(StorageOp::ReadRequest, &e.to_string()),
                    None,
                )
            })?;
            drop(alive);
            *body = prepared;
            if decoded {
                self.headers.remove(http::header::CONTENT_ENCODING);
                self.input.headers.remove("content-encoding");
                self.body_version = self.next_body_version;
                self.next_body_version += 1;
                // 首个 retry 前缀可能在排队前已记下；它和原请求共用这份解码后的体。
                if let Some(prefix) = &mut self.prefix {
                    prefix.headers.remove(http::header::CONTENT_ENCODING);
                    prefix.input.headers.remove("content-encoding");
                    prefix.version = self.body_version;
                }
            }
        }
        Ok(())
    }
    /// 依次派发；在完成一层之后才移动游标。一个请求的局部配置不能通过共享的实例泄漏。
    pub(super) async fn run(
        &mut self,
        state: &mut AppState,
        req: &mut OutboundRequest,
        attempt: u32,
        responder: &mut Responder,
        pending: &mut Option<ClientUpload>,
    ) -> Result<bool, ChainError> {
        if self.cursor == state.formats.len() {
            let Some(p) = &mut self.prefix else {
                return Ok(false);
            };
            self.cursor = p.cursor;
            req.method = p.method.clone();
            self.input = p.input.clone();
            self.headers = p.headers.clone();
            self.body_version = p.version;
            self.body = p.body.clone();
            self.prepare_original = p.prepare_original;
            self.stage = Stage::Retry;
            if let Some(previous) = p.overrides.take() {
                previous.restore(state, self, responder);
            }
        } else if let Some(previous) = self.failed_overrides.take() {
            previous.restore(state, self, responder);
        }
        while self.cursor < state.formats.len() {
            let pool = state.formats[self.cursor].clone();
            if self.prefix.is_none() && pool.entry.has_stage(Stage::Retry) {
                self.prefix = Some(Prefix {
                    overrides: None,
                    cursor: self.cursor,
                    method: req.method.clone(),
                    input: self.input.clone(),
                    headers: self.headers.clone(),
                    body: self.body.clone(),
                    version: self.body_version,
                    prepare_original: self.prepare_original,
                });
            }
            if pool.entry.has_stage(self.stage) {
                let name = &pool.entry.name;
                let safe = state.history.secrets().replace(name);
                let stage = self.stage.as_str();
                let identity = responder.needs_identity_encoding();
                let exchange = async {
                    let turn = match pool.turn(false).await {
                        Ok(turn) => turn,
                        Err(f) => return Ok(Err(f)),
                    };
                    let need = turn
                        .hello()
                        .request_body
                        .get(stage)
                        .unwrap_or(&wire::BodyNeed::Full);
                    if *need != wire::BodyNeed::None
                        && let Some(upload) = pending.take()
                    {
                        let rec = req.ctx.as_ref().expect("format context").rec.clone();
                        upload
                            .buffer(state, req, &rec)
                            .await
                            .map_err(|t| ChainError::stop(t, None))?;
                        self.body_stream = req.body_stream;
                    }
                    if pending.is_none() {
                        self.prepare_body(state, &mut req.body, turn.hello())
                            .await?;
                    }
                    let ctx = req.ctx.as_ref().expect("format context");
                    let mut input = self.input.clone();
                    // 前缀留程序给的原值；保活可被后续覆盖开关，每次派发再叠加当时的代理头。
                    if identity {
                        input
                            .headers
                            .insert("accept-encoding".into(), "identity".into());
                    }
                    if *need == wire::BodyNeed::Full {
                        let bytes = match self.body.as_deref().unwrap_or(&req.body) {
                            RequestBody::Memory(b) => b.to_vec(),
                            RequestBody::Disk { path, .. } => {
                                tokio::fs::read(path).await.map_err(|e| {
                                    ChainError::stop(
                                        Terminal::local_storage_failed(
                                            StorageOp::ReadRequest,
                                            &e.to_string(),
                                        ),
                                        None,
                                    )
                                })?
                            }
                        };
                        self.body_stream.get_or_insert_with(|| {
                            json_requests_stream(serde_json::Deserializer::from_slice(&bytes))
                        });
                        input.body = wire::Body::from_bytes(bytes);
                    }
                    let m = wire::RequestMessage::new(
                        ctx.body_context(name, need),
                        if state.config.forward_only_enabled() {
                            wire::Delivery::Forward
                        } else {
                            wire::Delivery::Retry
                        },
                        input,
                        ctx.wire_client(),
                    );
                    let frame = if self.stage == Stage::Retry {
                        let mut retry = wire::Retry::new(
                            attempt,
                            self.last.clone().expect("retry follows a failure"),
                            self.started.elapsed().as_millis() as u64,
                        );
                        retry.last_response = self.last_response.clone();
                        wire::Stage::Retry(wire::RetryMessage::new(m, retry))
                    } else {
                        wire::Stage::Request(m)
                    };
                    Ok(turn.send(frame, pool.entry.timeout_secs).await)
                };
                match responder
                    .wait(exchange, None, "request format")
                    .await
                    .ok_or(ChainError::Disconnected)??
                {
                    Ok((reply, headers)) => {
                        let ctx = req.ctx.as_ref().expect("format context");
                        if self.stage == Stage::Request
                            && let Some(vote) = &reply.delivery
                        {
                            match vote.as_str() {
                                Some("forward")
                                    if !self.delivery_chosen && responder.is_committed() =>
                                {
                                    // 已经发出真实的 HTTP 头；不接受需要撤回心跳或改状态行的表态。
                                    self.delivery_chosen = true;
                                    tracing::debug!(format=%safe, "forward delivery vote ignored after response commitment");
                                }
                                Some(value @ ("forward" | "retry")) if !self.delivery_chosen => {
                                    self.delivery_chosen = true;
                                    let forward = value == "forward";
                                    let cfg = Arc::make_mut(&mut state.config);
                                    cfg.forward_only = Some(forward);
                                    ctx.rec.set_delivery(forward, false);
                                    responder.configure_beats(
                                        cfg,
                                        self.accept,
                                        self.body_stream.unwrap_or(false),
                                    );
                                }
                                Some("forward" | "retry") => (),
                                _ => tracing::warn!(format=%safe,"invalid format delivery ignored"),
                            }
                        }
                        if let wire::ReportField::Present(report) = reply.report {
                            ctx.rec.format_report(
                                (self.cursor, name),
                                stage,
                                (self.stage == Stage::Retry).then_some(attempt),
                                report,
                            );
                        }
                        ctx.absorb(name, reply.state);
                        if let Some(overrides) = reply.overrides {
                            if pending.is_none()
                                && self.body_stream.is_none()
                                && overrides.contains_key("keepalive")
                            {
                                self.body_stream = Some(request_body_wants_stream(&req.body).await);
                            }
                            if let Some(prefix) = &mut self.prefix {
                                prefix.overrides.get_or_insert_with(|| {
                                    OverrideInput::capture(state, self.max_attempts)
                                });
                            }
                            if reply.fail.as_ref().is_some_and(|f| f.retry) {
                                self.failed_overrides =
                                    Some(OverrideInput::capture(state, self.max_attempts));
                            }
                            self.apply_overrides(state, overrides, responder);
                        }
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
                            tracing::warn!(format=%safe,stage,reason=%reason,"format rejected attempt");
                            return if f.retry {
                                Err(ChainError::Retry(AttemptFailure::new(
                                    "format_fail",
                                    reason,
                                    details,
                                )))
                            } else {
                                Err(ChainError::stop(
                                    Terminal::format_rejected(&safe, &reason, details),
                                    f.response
                                        .map(|r| (r, headers.expect("validated terminal headers"))),
                                ))
                            };
                        }
                        if let Some(mut patch) = reply.request {
                            if let Some(method) = patch.method.as_ref().and_then(|s| s.parse().ok())
                            {
                                req.method = method;
                            }
                            if let Some(h) = &mut patch.headers {
                                let next = headers.expect("validated request headers");
                                self.prepare_original &= request_encoding(&self.headers, true)
                                    == request_encoding(&next, true);
                                self.headers = next;
                                *h = transform::headers_to_btreemap(&self.headers);
                            }
                            if let Some(bytes) = std::mem::take(&mut patch.body).as_bytes() {
                                self.prepare_original = false;
                                self.body = Some(Arc::new(
                                    responder
                                        .wait(
                                            transform::spool_request_body(
                                                bytes,
                                                state.spool_dir.as_deref(),
                                            ),
                                            None,
                                            "format request spool",
                                        )
                                        .await
                                        .ok_or(ChainError::Disconnected)?,
                                ));
                                self.body_version = self.next_body_version;
                                self.next_body_version += 1;
                            }
                            self.input.apply(patch);
                        }
                    }
                    Err(Fault::Open { kind, probe_at }) if pool.entry.required => {
                        tracing::warn!(format=%safe,stage,code="format_unavailable",kind,probe_at,"format request unavailable");
                        return Err(ChainError::stop(
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
                            super::failure_text::format_error(
                                aproxy_ipc::i18n::Lang::En,
                                &safe,
                                &e.to_string(),
                            ),
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
        req.target_url = self.input.url.clone();
        req.headers = self.headers.clone();
        let encoding = request_encoding(&req.headers, true);
        let changed = self.body_version != self.sent_body_version
            || encoding.as_deref() != self.sent_encoding.as_ref();
        self.sent_encoding = encoding.map(|v| v.into_owned());
        self.sent_body_version = self.body_version;
        req.replacement = self.body.clone();
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
                match (group.as_str(), k.as_str(), n) {
                    ("timeouts", "connect_secs", Some(n)) => {
                        cfg.connect_timeout_secs = n;
                        state.client = AppState::upstream_client(cfg);
                    }
                    ("timeouts", "first_byte_secs", Some(n)) => cfg.first_byte_timeout_secs = n,
                    ("timeouts", "read_idle_secs", Some(n)) => cfg.read_timeout_secs = n,
                    ("retry", "max_backoff_secs", Some(n)) => cfg.max_retry_backoff_secs = n,
                    ("retry", "max_attempts", Some(n)) if u32::try_from(n).is_ok() => {
                        self.max_attempts = Some(n as u32)
                    }
                    ("keepalive", "interval_secs", Some(n))
                        if {
                            tokio::time::Instant::now()
                                .checked_add(Duration::from_secs(n))
                                .is_some()
                        } =>
                    {
                        if responder.is_committed() {
                            continue;
                        }
                        cfg.keepalive_interval_secs = n
                    }
                    ("keepalive", "trigger", _)
                        if v.as_str() == Some("always")
                            || v.as_str()
                                .and_then(crate::config::KeepaliveTrigger::parse)
                                .is_some() =>
                    {
                        if responder.is_committed() {
                            continue;
                        }
                        cfg.keepalive_trigger = v.as_str().map(String::from)
                    }
                    ("keepalive", "heartbeat", _)
                        if v.as_str()
                            .is_some_and(|s| crate::config::heartbeat_problem(s).is_none()) =>
                    {
                        if responder.is_committed() {
                            continue;
                        }
                        cfg.keepalive_heartbeat = v.as_str().map(String::from)
                    }
                    _ => tracing::warn!("invalid or unknown format override ignored"),
                }
            }
            if group == "keepalive" {
                responder.configure_beats(
                    &state.config,
                    self.accept,
                    self.body_stream.unwrap_or(false),
                );
            }
        }
    }
}
/// 只对最终交付的成功响应逆序派发。整条链跳过时原样交付；一旦程序接受解码后的输入，输出编码以它的补丁为准。
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
    let meta = wire::RequestMeta::new(req.method.to_string(), req.target_url.clone());
    for (layer, p) in state.formats.iter().enumerate().rev() {
        if p.entry.has_stage(Stage::Response) {
            (status, headers, body) =
                transform_response_layer(state, ctx, &meta, layer, status, headers, body).await;
        }
    }
    (status, headers, body)
}

/// 仅在走到声明 response 的这一层时处理整体正文，内侧或外侧的流仍按块运转。
pub(super) async fn transform_response_layer(
    state: &AppState,
    ctx: &ExchangeCtx,
    meta: &wire::RequestMeta,
    layer: usize,
    mut status: StatusCode,
    mut headers: HeaderMap,
    mut body: SpooledBody,
) -> (StatusCode, HeaderMap, SpooledBody) {
    let p = &state.formats[layer];
    // 每个层界按当前头解释当前的体：上一层可以刚刚重新编码，而外层也可能完全跳过。
    let raw_encoding = request_encoding(&headers, true);
    let encoding = raw_encoding
        .as_deref()
        .map(|v| v.to_str().unwrap_or("unsupported"))
        .filter(|v| {
            v.split(',')
                .map(str::trim)
                .any(|e| !e.is_empty() && !e.eq_ignore_ascii_case("identity"))
        });
    let mut decoded_spool = None;
    let name = &p.entry.name;
    let mut fresh = false;
    let reply = loop {
        let result = match p.turn(fresh).await {
            Err(f) => Err(f),
            Ok(turn) => {
                // 已拿到进程才展开当前响应；重发走同一入口，无故障时不准备第二帧。
                let mut raw = match decoded_spool.as_ref().unwrap_or(&body) {
                    SpooledBody::Memory(b) => b.to_vec(),
                    SpooledBody::Disk { path, .. } => match tokio::fs::read(path).await {
                        Ok(b) => b,
                        Err(e) => {
                            tracing::warn!(error=%e,"could not read response spool; format skipped");
                            turn.unused().await;
                            return (status, headers, body);
                        }
                    },
                };
                if let Some(enc) = encoding
                    && decoded_spool.is_none()
                {
                    let mut decoded = Vec::new();
                    if !format_encoding_depth_allowed(enc)
                        || crate::decode::decode_strict(
                            enc,
                            raw.as_slice(),
                            &mut decoded,
                            spool_limit_bytes(&state.config) as u64,
                        )
                        .is_err()
                    {
                        tracing::warn!("could not decode response body; format skipped");
                        turn.unused().await;
                        return (status, headers, body);
                    }
                    raw = decoded;
                    decoded_spool = Some(
                        transform::spool_response_body(&raw, state.spool_dir.as_deref()).await,
                    );
                }
                let mut current = wire::Response::new(status.as_u16());
                current.headers = transform::headers_to_btreemap(&headers);
                if decoded_spool.is_some() {
                    current.headers.remove("content-encoding");
                }
                current.body = wire::Body::from_bytes(raw);
                let mut m = wire::ResponseMessage::new(ctx.context(name), meta.clone(), current);
                m.client = Some(ctx.wire_client());
                turn.send(wire::Stage::Response(m), p.entry.timeout_secs)
                    .await
            }
        };
        match result {
            Ok(r) => break Some(r),
            Err(e) if !fresh && !matches!(e, Fault::Open { .. }) => fresh = true,
            Err(e) => {
                tracing::warn!(format=%state.history.secrets().replace(name),error=%e,"response format layer skipped");
                break None;
            }
        }
    };
    let Some((r, prepared_headers)) = reply else {
        return (status, headers, body);
    };
    if let wire::ReportField::Present(report) = r.report {
        ctx.rec
            .format_report((layer, name), "response", None, report);
    }
    ctx.absorb(name, r.state);
    let (code, replacement) = if let Some(f) = r.fail {
        let Some(response) = f.response else {
            return (status, headers, body);
        };
        (Some(response.status), Some(response.body))
    } else if let Some(patch) = r.response {
        (
            patch.status,
            (patch.body != wire::Body::Absent).then_some(patch.body),
        )
    } else {
        (None, None)
    };
    if let Some(code) = code {
        status = StatusCode::from_u16(code).expect("validated response status");
    }
    if decoded_spool.is_some() {
        headers.remove(http::header::CONTENT_ENCODING);
        headers.remove(http::header::CONTENT_LENGTH);
    }
    if let Some(h) = prepared_headers {
        headers = h;
        headers.remove(http::header::CONTENT_LENGTH);
    }
    if let Some(bytes) = replacement {
        body = transform::spool_response_body(
            bytes.as_bytes().unwrap_or_default(),
            state.spool_dir.as_deref(),
        )
        .await;
        headers.remove(http::header::CONTENT_LENGTH);
    } else if let Some(decoded) = decoded_spool {
        body = decoded;
    }
    (status, headers, body)
}
