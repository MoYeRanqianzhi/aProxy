//! 尝试循环：一个请求从请求转换、逐次发往上游、判定，到把结果交给客户端的全过程。
//!
//! 只有一个循环（[`run_attempts`]），两种「出口」（[`Responder`]）决定结果怎样交给客户端：
//! - [`Responder::Buffered`]（不保活）：拿到可交付的结果之前不向客户端写任何字节，成功后一次性
//!   保真回放。客户端断开时 hyper drop 掉整个 handler future（连同在途的上游请求），这里不用
//!   自己感知（计费保护的第 3 层）。
//! - [`Responder::Keepalive`]：经提交点三态机交付（见 keepalive 模块）。等待期间按节拍发心跳，
//!   一个间隔内没有可提交的结果就先提交骨架头；客户端断开即返回。
//!
//! 每次尝试的结果由 [`classify`] 归成一种 [`Outcome`]——成功、重试、受限路径用尽、两种确定性的
//! 本地失败——各出口按 Outcome 收场。判定与「最近错误」的文案因此只有一份。

use super::*;
use crate::config::file::Stage;
use crate::history::{Phase, Recorder, SentHead};

/// 从 forward_once 的完整响应里拆出、要交付给客户端的一份上游响应。
pub(super) struct UpstreamResponse {
    pub(super) status: StatusCode,
    /// 已过滤 hop-by-hop 的待透传头
    pub(super) headers: HeaderMap,
    /// 原始 reqwest 头：流式嗅探用
    pub(super) raw_headers: reqwest::header::HeaderMap,
    pub(super) body: SpooledBody,
}

/// 一次尝试的判定结果。
pub(super) enum Outcome {
    /// 可以交付：成功的上游响应
    Deliver(UpstreamResponse),
    /// 需要重试。`failure` 是这次没成功的原因，记进实例的「最近错误」（status 与界面里
    /// 看得到）。`rule` 是让它接着重试的那条重试规则的序号（从 1 起）：命中了规则、还没到它
    /// 给的次数，或者命中的是豁免；没有规则命中（按默认无限重试）时为 None。
    /// `response_bytes` 是这次尝试从上游收到的响应体字节数
    Retry {
        failure: AttemptFailure,
        rule: Option<usize>,
        response_bytes: u64,
        /// 有响应的失败也可被 decide 选择原样返回。
        response: Option<UpstreamResponse>,
    },
    /// 一条重试规则给的尝试次数用完了：不再重试，交付这次失败的响应——客户端拿到真实的
    /// 404 自行处理，好过永远等不到终态（compact 事故，见记忆 bounded-retry-paths）。
    /// `rule` 为命中规则的序号（从 1 起）；None 表示本请求的覆盖给出了上限。
    Exhausted {
        response: UpstreamResponse,
        failure: AttemptFailure,
        rule: Option<usize>,
    },
    /// 上游响应体超出 spool 上限：确定性失败，重试无意义
    TooLarge,
    /// 本地磁盘缓存读写失败：确定性失败。带着出问题的是哪一步与原因
    SpoolFailed(StorageOp, String),
}

/// 判定一次尝试的结果。只看结果本身、第几次尝试与路径已经匹配的重试规则（`rules`，见
/// `AppState::retry_rules_for`），不碰任何状态，所以可以单测；重试与否的细则在
/// `needs_retry_response`（状态码、错误体、流内错误事件）。
///
/// 规则只封顶「上游有响应」的失败：网络错误是真正的瞬时类，仍然无限重试，而且它没有响应
/// 可供交付。第一条状态码也匹配的规则生效：第 N 次尝试失败且 N ≥ 它的 `max_attempts` 就停；
/// `max_attempts = 0` 的规则是豁免——它命中了，排在后面的规则就不再看，照常重试。
pub(super) fn classify(
    attempt: u32,
    result: ForwardResult,
    rules: &[(usize, &CompiledRetryRule)],
    override_max: Option<u32>,
) -> Outcome {
    match result {
        ForwardResult::NetworkError(failure) => Outcome::Retry {
            response_bytes: failure.received_bytes(),
            response: None,
            failure,
            rule: None,
        },
        ForwardResult::TooLarge => Outcome::TooLarge,
        ForwardResult::SpoolFailed(op, e) => Outcome::SpoolFailed(op, e),
        ForwardResult::Response {
            status,
            headers,
            raw_headers,
            body,
            disk_scan,
        } => {
            let failed = needs_retry_response(attempt, &status, &raw_headers, &body, &disk_scan);
            let response = UpstreamResponse {
                status,
                headers,
                raw_headers,
                body,
            };
            if !failed {
                return Outcome::Deliver(response);
            }
            let failure = AttemptFailure::response(status);
            let limit = override_max
                .map(|n| (None, n))
                .or_else(|| first_matching_rule(rules, status.as_u16()).map(|(i, n)| (Some(i), n)));
            match limit {
                Some((rule, max)) if max > 0 && attempt >= max => Outcome::Exhausted {
                    response,
                    failure,
                    rule,
                },
                // 进入下一轮时 body 随 response 一起 Drop：磁盘临时文件删除
                matched => Outcome::Retry {
                    failure,
                    rule: matched.and_then(|(rule, _)| rule),
                    // 这次尝试收到了完整的响应体，只是内容算失败
                    response_bytes: response.body.len(),
                    response: Some(response),
                },
            }
        }
    }
}

/// 尝试循环的出口：结果怎样交给客户端（见模块文档）。
pub(super) enum Responder {
    Buffered {
        /// 循环交出的响应；每条出口路径都会设上（Buffered 不会因客户端断开提前返回）
        out: Option<Response>,
    },
    Keepalive {
        sink: KeepaliveSink,
        /// 整个请求共用一个节拍：既是「一个间隔内仍无可提交的结果就提交骨架」的计时器，
        /// 也是提交后的心跳节拍
        ticker: Option<tokio::time::Interval>,
        /// 上游真实的 SSE 响应头能不能先行提交：转换能改头，决定也可能否决整次响应。
        real_head_allowed: bool,
    },
}

/// 保活出口等待上游响应头时用的通知（status 与已过滤的头）
type HeadTx = tokio::sync::oneshot::Sender<(StatusCode, HeaderMap)>;
type HeadRx = tokio::sync::oneshot::Receiver<(StatusCode, HeaderMap)>;

impl Responder {
    pub(super) fn buffered() -> Self {
        Self::Buffered { out: None }
    }

    /// 保活出口：`resp_tx` 是 handler 正在等的 oneshot。首个 tick 在请求开始一个间隔之后；
    /// 被长回放、慢客户端耽搁后不补发积压的 tick（Delay）。
    pub(super) fn keepalive(
        state: &AppState,
        resp_tx: tokio::sync::oneshot::Sender<Response>,
        rec: Recorder,
        enabled: bool,
    ) -> Self {
        let interval = state.config.keepalive_interval();
        Self::Keepalive {
            sink: KeepaliveSink::Pending {
                resp_tx: Some(resp_tx),
                skeleton_due: false,
                beat: Heartbeat {
                    fixed: Bytes::copy_from_slice(state.config.keepalive_heartbeat()),
                    dynamic: None,
                    rec,
                },
            },
            ticker: enabled.then(|| Self::ticker(interval)),
            // 不能用某个旧 worker 的 hello 排除成功咨询：其他会话可以在上游体到齐前重新握手。
            real_head_allowed: !state.has_format(Stage::Response)
                && !state.has_format(Stage::Decide)
                && !state.has_format(Stage::Stream),
        }
    }

    /// 从改变间隔的时刻重新计时，不把之前累积的心跳挤在一起。
    fn ticker(interval: Duration) -> tokio::time::Interval {
        let mut t = tokio::time::interval_at(tokio::time::Instant::now() + interval, interval);
        t.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        t
    }

    /// 每层回信到达后、处理下一层之前更新未提交的出口；已提交的流不能再换帧格式。
    pub(super) fn configure_beats(&mut self, cfg: &Config, accept: bool, body_stream: bool) {
        let Self::Keepalive {
            sink: KeepaliveSink::Pending {
                beat, skeleton_due, ..
            },
            ticker,
            ..
        } = self
        else {
            return;
        };
        let triggered = match cfg.keepalive_trigger.as_deref() {
            Some("always") => true,
            Some("accept") => accept,
            Some("body_stream") => body_stream,
            _ => accept || body_stream,
        };
        let interval = cfg.keepalive_interval();
        let enabled = triggered && !interval.is_zero();
        if ticker.as_ref().map(|t| t.period()) != enabled.then_some(interval) {
            *ticker = enabled.then(|| Self::ticker(interval));
            *skeleton_due = false;
            beat.rec.set_keepalive(enabled);
        }
        beat.fixed = Bytes::copy_from_slice(cfg.keepalive_heartbeat());
    }

    /// 已经向客户端提交了响应头（只有保活出口会提交）：此后 status 与响应头不能再改，
    /// 只能往 SSE 体里写
    pub(super) fn is_committed(&self) -> bool {
        match self {
            Self::Buffered { .. } => false,
            Self::Keepalive { sink, .. } => sink.is_committed(),
        }
    }

    /// 等 `fut` 完成。None = 客户端已断开（只有保活出口会感知到；`fut` 随之被 drop，在途的
    /// 上游连接关闭）。保活出口在等待期间照常心跳或提交骨架头；`head_rx` 是本次尝试的响应头
    /// 通知，见 [`Responder::head_channel`]。
    pub(super) async fn wait<F: std::future::Future>(
        &mut self,
        fut: F,
        head_rx: Option<HeadRx>,
        during: &str,
    ) -> Option<F::Output> {
        match self {
            Self::Buffered { .. } => Some(fut.await),
            Self::Keepalive {
                sink,
                ticker,
                real_head_allowed,
            } => match ticker {
                Some(ticker) => drive(sink, ticker, fut, head_rx, *real_head_allowed, during).await,
                None => tokio::select! {result=fut => Some(result), _=sink.client_gone() => None},
            },
        }
    }

    /// 已确认的非 SSE 成功保留原媒体类型直到被否决。暂停的是提交而非断开检测；
    /// 积欠的骨架心跳保留给可能发生的下一次尝试，而不是写进这个响应。
    async fn wait_after_head<F: std::future::Future>(
        &mut self,
        fut: F,
        preserve_non_sse: bool,
        during: &str,
    ) -> Option<F::Output> {
        if preserve_non_sse
            && let Self::Keepalive { sink, .. } = self
            && !sink.is_committed()
        {
            tokio::select! {result=fut => Some(result), _=sink.client_gone() => None}
        } else {
            self.wait(fut, None, during).await
        }
    }

    /// 本次尝试要不要上游响应头的通知：保活出口在提交之前要（可提交的上游真实头先行提交、
    /// 「2xx 非 SSE」暂停骨架提交，见 drive），其余情况不要
    fn head_channel(&self) -> (Option<HeadTx>, Option<HeadRx>) {
        match self {
            Self::Keepalive {
                sink,
                ticker: Some(_),
                ..
            } if !sink.is_committed() => {
                let (tx, rx) = tokio::sync::oneshot::channel();
                (Some(tx), Some(rx))
            }
            _ => (None, None),
        }
    }

    /// 只有当前还在发送保活心跳时才覆盖上游编码；层间视图与实际发送使用同一判定。
    pub(super) fn needs_identity_encoding(&self) -> bool {
        matches!(
            self,
            Self::Keepalive {
                ticker: Some(_),
                ..
            }
        )
    }

    /// 发往上游的请求头的出口要求：保活出口会往响应体里插心跳注释，只有未压缩的体才能这样
    /// 插（往 gzip / br 流里插明文，客户端解压必坏），所以要求上游以 identity 编码回应。客户端
    /// 照旧拿到合法响应（未压缩永远是可接受的编码），代价只是上游到本机这一段多传些字节。
    /// 放在请求转换之后，覆盖 format 可能写入的同名头（override_headers 里配的同名头也被覆盖，
    /// 启动时 server.rs 会 warn 一次）。
    fn prepare_upstream_headers(&self, headers: &mut HeaderMap) {
        if self.needs_identity_encoding() {
            headers.insert(
                http::header::ACCEPT_ENCODING,
                HeaderValue::from_static("identity"),
            );
        }
    }

    /// 配置了心跳转换时，保活出口改由心跳转换器逐拍生成心跳（请求转换完成之后才有它要的请求）
    fn arm_dynamic_heartbeat(&mut self, state: &AppState, req: &OutboundRequest) {
        if let Self::Keepalive { sink, .. } = self
            && state.has_format(Stage::Heartbeat)
        {
            sink.arm_dynamic_heartbeat(Arc::new(DynamicHeartbeat {
                pools: state.formats.clone(),
                ctx: req.ctx.as_ref().expect("format context").clone(),
                last: Default::default(),
                seq: Default::default(),
                attempt: Default::default(),
                pending: Default::default(),
                warned: Default::default(),
            }));
        }
    }

    /// 告诉动态心跳当前是第几次上游尝试
    fn note_attempt(&self, attempt: u32) {
        if let Self::Keepalive { sink, .. } = self {
            sink.note_attempt(attempt);
        }
    }

    /// 尚未提交时交出完整响应（保活出口：客户端已断开时随之丢弃）
    pub(super) fn respond(&mut self, resp: Response) {
        match self {
            Self::Buffered { out } => *out = Some(resp),
            Self::Keepalive { sink, .. } => sink.respond(resp),
        }
    }

    /// 往已提交的响应体里写一块。false = 客户端已断开
    pub(super) async fn send(&mut self, chunk: Result<Bytes, std::io::Error>) -> bool {
        match self {
            Self::Buffered { .. } => false,
            Self::Keepalive { sink, .. } => sink.send(chunk).await,
        }
    }

    /// 终态：尚未提交 → JSON 错误响应；已提交 → SSE 终态 error 事件（状态行已发出、不可
    /// 再改，客户端借此明确知道代理放弃了这条请求，而不是永远等下去）。两者是同一个 JSON
    pub(super) async fn finish(&mut self, terminal: Terminal, trace: &RequestTrace) {
        if self.is_committed() {
            self.send(Ok(terminal.sse_event(trace))).await;
        } else {
            self.respond(terminal.into_response(trace));
        }
    }

    fn log_client_gone(&self, during: &str) {
        if let Self::Keepalive { sink, .. } = self {
            sink.log_client_gone(during);
        }
    }

    /// 不保活出口交出的响应。每条出口路径都会设上；万一漏了，给客户端一个明确的 500，
    /// 而不是让连接悬空
    pub(super) fn into_response(self, trace: &RequestTrace) -> Response {
        match self {
            Self::Buffered { out: Some(resp) } => resp,
            _ => {
                tracing::error!(code = %"internal", "尝试循环没有交出响应");
                Terminal::internal("the attempt loop ended without a response").into_response(trace)
            }
        }
    }
}

/// 把要发往上游的请求行与头交给请求历史，返回入库的那一份（发出的请求不变时，各次尝试
/// 共用它）。只留真正发出去的头：逐跳头在发送时会被剥掉。
pub(super) fn sent_head(rec: &Recorder, req: &OutboundRequest) -> Option<Arc<SentHead>> {
    rec.sent_head(
        req.method.as_str(),
        &req.target_url,
        req.headers
            .iter()
            .filter(|(name, _)| !is_hop_header(name.as_str()))
            .map(|(name, value)| (name.as_str(), value.as_bytes())),
    )
}

/// 在请求历史里记下第 `attempt` 次尝试开始。`same_body` 为假（这次发出的体与上一次不同）
/// 时把体也交过去：内存里的整块交，不拷贝；落盘的在阻塞线程池里读。
pub(super) fn record_try(
    rec: &Recorder,
    attempt: u32,
    sent: Option<&Arc<SentHead>>,
    same_body: bool,
    req: &OutboundRequest,
) {
    let raw = request_encoding(&req.headers, req.ctx.is_some());
    let encoding = request_history_encoding(raw.as_deref(), req.ctx.is_some());
    let Some(mut history) = rec.try_started(attempt, sent, same_body, encoding) else {
        return;
    };
    match req.sent_body() {
        RequestBody::Memory(bytes) => history.feed(bytes),
        RequestBody::Disk { path, .. } => {
            // 现在就把文件打开：请求结束时它会被删掉，已经打开的句柄照样读得完
            let Ok(mut file) = std::fs::File::open(path) else {
                return;
            };
            tokio::task::spawn_blocking(move || {
                use std::io::Read as _;
                while history.is_live() {
                    let mut block = vec![0u8; 64 * 1024];
                    match file.read(&mut block) {
                        Ok(n) if n > 0 => {
                            block.truncate(n);
                            history.feed(&Bytes::from(block));
                        }
                        _ => break,
                    }
                }
            });
        }
    }
}

/// 请求方向的终态不依赖交付方式；保活已提交时发事件，否则保留程序的状态、头和正文。
pub(super) async fn finish_chain_stop(
    state: &AppState,
    r: &mut Responder,
    trace: &RequestTrace,
    req: &OutboundRequest,
    attempt: u32,
    client_head: bool,
    stop: (Terminal, Option<ProgramResponse>),
) {
    let (t, custom) = stop;
    let rec = &trace.rec;
    t.record_in(&state.stats);
    record_try(rec, attempt, None, true, req);
    rec.try_ended(attempt, 0, Some(t.for_history()));
    if let Some((response, mut headers)) = custom {
        rec.settle(crate::history::Outcome::FormatResponse, Some(t.code()));
        let bytes = response.body.as_bytes().unwrap_or_default();
        if r.is_committed() {
            let data = if terminal_is_sse(&headers) {
                Bytes::copy_from_slice(bytes)
            } else if let Some(event) = terminal_json_event(bytes) {
                event
            } else {
                r.finish(t, trace).await;
                return;
            };
            let size = data.len() as u64;
            if r.send(Ok(data)).await {
                rec.response_bytes(size);
            }
        } else if let Ok(status) = StatusCode::from_u16(response.status) {
            mark_given_up(&mut headers, t.code(), trace);
            if delivers_body(client_head, status) {
                rec.response_bytes(bytes.len() as u64);
            }
            r.respond(build_response(
                status,
                headers,
                Bytes::copy_from_slice(bytes),
            ));
        }
    } else {
        r.finish(t, trace).await;
    }
}

/// 一个请求的全部尝试：请求转换 → 逐次发往上游并判定 → 按出口收场。返回时结果已交给
/// `r`（不保活出口取 [`Responder::into_response`]），或客户端已断开。`trace` 是这个请求的
/// 标识与尝试计数，终态的错误体要带。
///
/// 请求转换也经 `r.wait` 驱动：保活出口里它同样处在心跳节拍内——format 慢（timeout_secs = 0
/// 时没有上界）时客户端照样在一个间隔后收到骨架头与心跳，而不是在拿到任何字节之前干等。
pub(super) async fn run_attempts(
    state: &AppState,
    req: OutboundRequest,
    path_and_query: &str,
    max_spool_bytes: usize,
    r: &mut Responder,
    mut trace: RequestTrace,
) {
    // 规则对客户端的原始路径判定（在请求转换之前）：哪些端点受限是用户按客户端发出的
    // 请求写的，format 改写 URL 不影响它
    let rules = state.retry_rules_for(path_and_query);
    let rec = trace.rec.clone();
    let client_head = req.method == http::Method::HEAD;
    let mut req = req;
    let needs_chain = state.has_format(Stage::Request) || state.has_format(Stage::Retry);
    let mut local = needs_chain.then(|| state.clone());
    let mut chain = if needs_chain {
        Some(RequestChain::new(&req))
    } else {
        None
    };
    r.arm_dynamic_heartbeat(state, &req);
    r.prepare_upstream_headers(&mut req.headers);
    log_proxied_request(&state.history, &req.method, path_and_query, &req.target_url);
    let mut sent = sent_head(&rec, &req);
    let mut same_body = request_encoding(&req.headers, req.ctx.is_some()).as_deref()
        == req.client_encoding.as_ref();
    let deciding = state.has_format(Stage::Decide);
    let started = deciding.then(tokio::time::Instant::now);
    let mut next_delay = None;
    let mut reroute = true;
    let mut attempt: u32 = 0;
    // 「重试中」的计数守卫：第二次尝试开始时拿到，交付之前放掉；别的出口随函数返回丢弃
    let mut retrying: Option<crate::daemon::Counted> = None;
    loop {
        attempt += 1;
        r.note_attempt(attempt);
        if attempt > 1 {
            // 每轮重试刷新活动时间戳：last_activity_secs 的语义是「最近一次收到请求或仍在
            // 处理」——长退避可能超过 idle_timeout_secs，不刷新的话无限重试中的实例会被
            // stop idle / status --idle 误判为闲置并强退，恰恰杀掉最需要「不中断」保障的
            // 实例（M1）。放在重试轮开头而非退避之后：退避等待期间的实例也视为活跃。
            state.touch_activity();
            state.stats.note_retry();
            // 从第二次尝试起算「重试中」，直到交付或放弃
            if retrying.is_none() {
                retrying = Some(state.stats.begin_retrying());
            }
            let delay = next_delay.take().unwrap_or_else(|| {
                retry::delay_for_attempt(
                    attempt - 1,
                    local
                        .as_ref()
                        .unwrap_or(state)
                        .config
                        .max_retry_backoff_secs,
                )
            });
            rec.backoff(delay);
            if delay.is_zero() {
                tracing::warn!(attempt, "立即重试");
            } else {
                tracing::warn!(attempt, delay_ms = delay.as_millis() as u64, "重试延迟");
                if r.wait(tokio::time::sleep(delay), None, "退避等待")
                    .await
                    .is_none()
                {
                    return;
                }
            }
        }

        let mut failed = None;
        if reroute && let (Some(c), Some(local)) = (chain.as_mut(), local.as_mut()) {
            rec.phase(Phase::Transforming);
            match c.run(local, &mut req, attempt, r).await {
                Err(ChainError::Disconnected) => return,
                Ok(changed) => {
                    r.prepare_upstream_headers(&mut req.headers);
                    sent = sent_head(&rec, &req);
                    same_body = !changed;
                }
                Err(ChainError::Retry(f)) => failed = Some(f),
                Err(ChainError::Stop(stop)) => {
                    finish_chain_stop(state, r, &trace, &req, attempt, client_head, *stop).await;
                    return;
                }
            }
        }
        let state = local.as_ref().unwrap_or(state);
        record_try(
            &rec,
            attempt,
            if failed.is_none() {
                sent.as_ref()
            } else {
                None
            },
            same_body,
            &req,
        );
        same_body = true;
        let chain_failed = failed.is_some();
        let result = if let Some(f) = failed {
            ForwardResult::NetworkError(f)
        } else {
            trace.attempts += 1;
            let (head_tx, head_rx) = r.head_channel();
            let attempt_fut = forward_once(
                state,
                &req.method,
                &req.target_url,
                &req.headers,
                req.sent_body(),
                max_spool_bytes,
                head_tx,
                &rec,
                attempt,
            );
            let Some(result) = r.wait(attempt_fut, head_rx, "等待上游").await else {
                return;
            };
            result
        };
        let override_max = chain.as_ref().and_then(|c| c.max_attempts);
        let mut outcome = classify(attempt, result, &rules, override_max);
        let mut voted_by = None;
        reroute = true;
        if let Some(started) = started {
            let query = decide::consult(
                state,
                &req,
                &outcome,
                attempt,
                started.elapsed(),
                override_max,
            );
            let preserve_non_sse =
                matches!(&outcome, Outcome::Deliver(v) if !is_event_stream(&v.headers));
            let Some(votes) = r
                .wait_after_head(query, preserve_non_sse, "format decide")
                .await
            else {
                return;
            };
            reroute = chain_failed || votes.reroute.unwrap_or(true);
            if let Some(verdict) = votes.decision {
                let (original, previous, received) = match outcome {
                    Outcome::Retry {
                        response,
                        failure,
                        response_bytes,
                        ..
                    } => (response, Some(failure), response_bytes),
                    Outcome::Exhausted {
                        response, failure, ..
                    } => {
                        let len = response.body.len();
                        (Some(response), Some(failure), len)
                    }
                    Outcome::Deliver(r) => {
                        let len = r.body.len();
                        (Some(r), None, len)
                    }
                    _ => unreachable!("local terminal outcomes are not submitted to decide"),
                };
                let name = match &verdict {
                    decide::Verdict::Stop { name, .. } | decide::Verdict::Retry { name, .. } => {
                        name
                    }
                };
                let by = format!("format:{name}");
                let retry = matches!(verdict, decide::Verdict::Retry { .. });
                tracing::info!(
                    attempt,
                    by,
                    decision = if retry { "retry" } else { "stop" },
                    "format decided attempt"
                );
                if !retry
                    && previous.is_none()
                    && matches!(verdict, decide::Verdict::Stop { patch: None, .. })
                {
                    rec.decided(attempt, by, false);
                    outcome = Outcome::Deliver(original.expect("successful upstream response"));
                } else {
                    let failure = previous.unwrap_or_else(|| {
                        let mut f = AttemptFailure::new(
                            "format_decision",
                            format!("format '{name}' rejected the successful attempt"),
                            serde_json::json!({"format":name}),
                        );
                        f.status = original.as_ref().map(|r| r.status.as_u16());
                        f
                    });
                    match verdict {
                        decide::Verdict::Retry { delay, .. } => {
                            next_delay = delay;
                            voted_by = Some(by);
                            outcome = Outcome::Retry {
                                response: original,
                                failure,
                                rule: None,
                                response_bytes: received,
                            };
                        }
                        decide::Verdict::Stop { name, patch } => {
                            let terminal = Terminal::retry_stopped(&name, &failure);
                            rec.try_ended(attempt, received, Some(failure.for_history()));
                            rec.decided(attempt, by, false);
                            failure.record_in(&state.stats);
                            terminal.record_in(&state.stats);
                            decide::finish(
                                state,
                                r,
                                &trace,
                                terminal,
                                original,
                                patch,
                                client_head,
                            )
                            .await;
                            return;
                        }
                    }
                }
            }
        }
        match outcome {
            Outcome::Retry {
                failure,
                rule,
                response_bytes,
                ..
            } => {
                if chain.is_some() || state.has_format(Stage::Heartbeat) {
                    let mut f = aproxy_envelope::Failure::new(failure.code.into());
                    f.details = Some(failure.details.clone());
                    if let Some(c) = &mut chain {
                        c.last = Some(f.clone());
                    }
                    if let Responder::Keepalive { sink, .. } = r {
                        let beat = match sink {
                            KeepaliveSink::Committed { beat, .. }
                            | KeepaliveSink::Pending { beat, .. } => beat,
                        };
                        if let Some(d) = &beat.dynamic {
                            *d.last.lock().unwrap() = Some(f);
                        }
                    }
                }
                if voted_by.is_none()
                    && let Some(c) = &chain
                    && c.max_attempts.is_some_and(|n| n != 0 && attempt >= n)
                {
                    let t = Terminal::retry_exhausted("override", attempt, &failure);
                    failure.record_in(&state.stats);
                    t.record_in(&state.stats);
                    rec.try_ended(attempt, response_bytes, Some(failure.for_history()));
                    tracing::warn!(
                        attempt,
                        by = "override",
                        decision = "stop",
                        "retry attempt limit reached"
                    );
                    rec.decided(attempt, "override".into(), false);
                    r.finish(t, &trace).await;
                    return;
                }
                failure.record_in(&state.stats);
                rec.try_ended(attempt, response_bytes, Some(failure.for_history()));
                // 谁让它接着重试的：命中的重试规则，或者默认的无限重试
                let by = voted_by.unwrap_or_else(|| {
                    rule.map_or_else(
                        || {
                            if override_max.is_some() {
                                "override"
                            } else {
                                "default"
                            }
                            .into()
                        },
                        |rule| format!("rule:{rule}"),
                    )
                });
                tracing::warn!(
                    attempt,
                    by,
                    decision = "retry",
                    code = %failure.code,
                    details = %serde_json::Value::Object(failure.details.clone()),
                    reason = %failure.message,
                    "本次尝试失败，重试"
                );
                rec.decided(attempt, by, true);
                // 需要重试时不在这里提交——保活出口的提交只由节拍与可提交的上游响应头推动，
                // 下一轮尝试很快成功时客户端仍拿到上游真实头
            }
            Outcome::Deliver(response) => {
                rec.try_ended(attempt, response.body.len(), None);
                // 回放期间它不再是「重试中」；经过重试才成功的算救回一个
                if retrying.take().is_some() {
                    state.stats.note_rescued();
                }
                deliver(state, r, &req, &trace, response, max_spool_bytes).await;
                return;
            }
            Outcome::Exhausted {
                response,
                failure,
                rule,
            } => {
                let status = response.status;
                // `details.by` 说明是谁给的次数：第几条重试规则
                let by = rule.map_or_else(|| "override".into(), |i| format!("rule:{i}"));
                let terminal = Terminal::retry_exhausted(&by, attempt, &failure);
                rec.try_ended(attempt, response.body.len(), Some(failure.for_history()));
                // 这次尝试也是一次失败的尝试；随后的终态成为「最近一次失败」
                failure.record_in(&state.stats);
                terminal.record_in(&state.stats);
                tracing::warn!(attempt, code = %terminal.code(), last_code = failure.code, status = %status, by, decision = "stop", "retry attempt limit reached");
                rec.decided(attempt, by, false);
                if !r.is_committed() {
                    // 尚未向客户端写出任何字节：原样交付最后一次失败响应（状态码与体保真），
                    // 只加两个头，让调用方分得清「aproxy 已经放弃」与上游的普通回应
                    let UpstreamResponse {
                        status,
                        headers,
                        raw_headers,
                        body,
                    } = response;
                    rec.phase(Phase::Replaying);
                    rec.response_bytes(body.len());
                    let mut resp = replay_failure_as_is(status, headers, &raw_headers, body).await;
                    mark_given_up(resp.headers_mut(), terminal.code(), &trace);
                    // 客户端拿到的是上游的失败响应原样，不是 aproxy 自产的错误体
                    rec.settle(
                        crate::history::Outcome::UpstreamError,
                        Some(terminal.code()),
                    );
                    r.respond(resp);
                } else {
                    // 已提交：状态行已发出，真实 status 与头无法交付，以终态 error 事件收场
                    drop(response);
                    r.send(Ok(terminal.sse_event(&trace))).await;
                }
                return;
            }
            Outcome::TooLarge => {
                rec.try_ended(attempt, 0, None);
                let terminal = Terminal::response_too_large(max_spool_bytes);
                terminal.record_in(&state.stats);
                tracing::error!(
                    attempt,
                    code = %terminal.code(),
                    "上游响应体超出 spool 上限，终止重试"
                );
                r.finish(terminal, &trace).await;
                return;
            }
            Outcome::SpoolFailed(op, e) => {
                let terminal = Terminal::local_storage_failed(op, &e);
                rec.try_ended(
                    attempt,
                    0,
                    (op == StorageOp::ReadRequest).then(|| terminal.for_history()),
                );
                terminal.record_in(&state.stats);
                tracing::error!(attempt, code = %terminal.code(), error = %e, "本地 spool 故障，终止重试");
                r.finish(terminal, &trace).await;
                return;
            }
        }
    }
}

/// 交付一次成功的上游响应：先交响应转换器（若配置；失败时透传原样），再按出口交付。
///
/// 响应转换放在提交判定之前，并尽量经 `r.wait` 驱动：format 转换期间照常心跳、一个间隔到点
/// 仍未提交就先提交骨架头（timeout_secs = 0 时转换耗时没有上界，不能让客户端在这段时间收不到
/// 任何字节）。例外是「尚未提交 + 上游回的是 2xx 非 SSE」：骨架是 SSE、心跳是 SSE 注释，此时
/// 提交会把非 SSE 的响应改坏（与 drive 的「2xx 非 SSE 暂停骨架提交」同一道理），只能直接等转换
/// 完成。不保活出口永远不会提交，所以总是直接等。
async fn deliver(
    state: &AppState,
    r: &mut Responder,
    req: &OutboundRequest,
    trace: &RequestTrace,
    response: UpstreamResponse,
    max_spool_bytes: usize,
) {
    let attempt = trace.attempts;
    let UpstreamResponse {
        status,
        headers,
        raw_headers,
        body,
    } = response;
    if let Responder::Keepalive { sink, .. } = r {
        sink.cancel_dynamic();
    }
    let upstream_sse = is_event_stream(&headers);
    let rec = &trace.rec;
    if state.has_format(Stage::Stream) {
        rec.phase(Phase::Transforming);
        let meta =
            aproxy_envelope::RequestMeta::new(req.method.to_string(), req.target_url.clone());
        let setup = streams::chain(
            state,
            req.ctx.as_ref().expect("format context"),
            &meta,
            attempt,
            status,
            headers,
            streams::Content::Saved(body),
        );
        let result = r.wait_after_head(setup, !upstream_sse, "stream open").await;
        let Some(result) = result else { return };
        match result {
            Ok(response) if !r.is_committed() => {
                rec.phase(Phase::Replaying);
                r.respond(response);
            }
            Ok(response) => {
                rec.phase(Phase::Replaying);
                let mut stream = streams::committed_body(response, max_spool_bytes);
                // 等流层下一块期间也要观察断开；不能等卡住的程序再产出一块才释放上游。
                let Responder::Keepalive { sink, .. } = r else {
                    unreachable!()
                };
                while let Some(chunk) =
                    tokio::select! {biased; _=sink.client_gone()=>None, item=stream.next()=>item}
                {
                    if !sink.send(chunk).await {
                        break;
                    }
                }
            }
            Err(t) => {
                t.record_in(&state.stats);
                r.finish(t, trace).await;
            }
        }
        return;
    }
    if state.has_format(Stage::Response) {
        rec.phase(Phase::Transforming);
    }
    let transform = transform_response_if_configured(state, req, status, headers, body);
    let transformed = r
        .wait_after_head(transform, !upstream_sse, "响应转换")
        .await;
    let Some((status, headers, body)) = transformed else {
        return;
    };
    if !r.is_committed() {
        // 提交前就成功：保真快速路径（上游 status 与转换后的响应头）
        rec.phase(Phase::Replaying);
        rec.response_bytes(body.len());
        let resp = replay_transformed(attempt, status, headers, &raw_headers, body).await;
        r.respond(resp);
        return;
    }

    // 已提交（只有保活出口会走到这里）：状态行与响应头不可再改——format 对 headers 的改写
    // 无效，仅 body 转换生效
    if !is_event_stream(&headers) {
        // 不撤回、照常回放：客户端已在等这个响应，丢掉成功结果只会更糟。日志给出可行动的
        // 配置建议
        tracing::warn!(
            attempt,
            content_type = %headers
                .get(http::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("-"),
            "已以 text/event-stream 提交的保活响应里回放的是非 SSE 的成功响应体（响应头已发出，无法撤回）；\
             若该实例的上游会对 stream:true 请求回非 SSE 的流（如 NDJSON）且常需重试，\
             建议为该实例设置 keepalive_trigger = \"accept\""
        );
    }
    let mut body = match encoding_to_undo(&headers) {
        None => body,
        Some(encoding) => {
            // 已提交的响应头里不带 content-encoding，上游无视 identity 仍压缩了响应时只能解码
            // 后回放；解码大体可能耗时，同样在心跳节拍内进行
            let decode = decode_for_committed_replay(
                body,
                encoding.clone(),
                state.spool_dir.clone(),
                max_spool_bytes,
            );
            let Some(decoded) = r.wait(decode, None, "回放前解码").await else {
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
                    let terminal = Terminal::decode_failed(&encoding);
                    terminal.record_in(&state.stats);
                    tracing::warn!(
                        attempt,
                        code = %terminal.code(),
                        encoding = %encoding,
                        error = %e,
                        "已提交的响应无法回放上游压缩体且解码失败，以终态 error 事件收场（保活通道）"
                    );
                    r.send(Ok(terminal.sse_event(trace))).await;
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
    rec.phase(Phase::Replaying);
    let mut stream = body.into_stream().await;
    // 已经交给客户端的响应体字节数，记进请求记录
    let mut replayed = 0u64;
    while let Some(item) = stream.next().await {
        let bytes = item.as_ref().map_or(0, |chunk| chunk.len() as u64);
        if !r.send(item).await {
            // 客户端在回放中断开：Drop 链负责删临时文件
            r.log_client_gone("回放");
            break;
        }
        replayed += bytes;
    }
    rec.response_bytes(replayed);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(status: u16, body: &'static str) -> ForwardResult {
        ForwardResult::Response {
            status: StatusCode::from_u16(status).unwrap(),
            headers: HeaderMap::new(),
            raw_headers: reqwest::header::HeaderMap::new(),
            body: SpooledBody::Memory(Bytes::from_static(body.as_bytes())),
            disk_scan: None,
        }
    }

    #[test]
    fn success_is_delivered() {
        let out = classify(1, response(200, r#"{"id":"msg_1"}"#), &[], None);
        assert!(matches!(out, Outcome::Deliver(r) if r.status == 200));
    }

    #[test]
    fn error_status_and_error_content_are_retried() {
        let out = classify(1, response(503, "overloaded"), &[], None);
        assert!(matches!(
            out,
            Outcome::Retry { ref failure, .. }
                if failure.code == "upstream_status" && failure.status == Some(503)
        ));
        // 200 但内容是错误：同样重试
        let out = classify(
            1,
            response(
                200,
                r#"{"type":"error","error":{"type":"overloaded_error"}}"#,
            ),
            &[],
            None,
        );
        // 状态码是成功的，失败在体里：记的是另一个码
        assert!(matches!(
            out,
            Outcome::Retry { ref failure, .. }
                if failure.code == "upstream_error_body" && failure.status == Some(200)
        ));
    }

    fn rule(status: Option<&[u16]>, max_attempts: u32) -> CompiledRetryRule {
        CompiledRetryRule {
            path: None,
            status: status.map(<[u16]>::to_vec),
            max_attempts,
        }
    }

    #[test]
    fn network_errors_are_retried_without_limit_even_under_a_rule() {
        let limit = rule(None, 3);
        for attempt in [1, 3, 100] {
            let out = classify(
                attempt,
                ForwardResult::NetworkError(AttemptFailure::connect_failed(
                    "connection refused".into(),
                )),
                &[(1, &limit)],
                None,
            );
            assert!(
                matches!(
                    out,
                    Outcome::Retry { ref failure, .. }
                        if failure.code == "upstream_connect_failed"
                            && failure.message.contains("connection refused")
                ),
                "第 {attempt} 次"
            );
        }
    }

    #[test]
    fn a_rule_stops_at_its_attempt_limit() {
        let limit = rule(None, 3);
        let rules = [(1, &limit)];
        for attempt in 1..3 {
            let out = classify(attempt, response(404, "not found"), &rules, None);
            assert!(matches!(out, Outcome::Retry { .. }), "第 {attempt} 次");
        }
        let out = classify(3, response(404, "not found"), &rules, None);
        assert!(matches!(
            out,
            Outcome::Exhausted { response, failure, rule: Some(1) }
                if response.status == 404 && failure.status == Some(404)
        ));
        // 计数口径是尝试序号：已经超过上限的尝试同样停
        let out = classify(7, response(404, "not found"), &rules, None);
        assert!(matches!(out, Outcome::Exhausted { .. }));
        // 没有规则时不封顶
        let out = classify(13, response(404, "not found"), &[], None);
        assert!(matches!(out, Outcome::Retry { .. }));
        // 规则不改变成功的响应
        let out = classify(9, response(200, "{}"), &rules, None);
        assert!(matches!(out, Outcome::Deliver(_)));
    }

    /// 第一条状态码也匹配的规则生效；`max_attempts = 0` 的规则是豁免，挡住后面的宽规则。
    #[test]
    fn the_first_matching_rule_decides() {
        let exempt_429 = rule(Some(&[429]), 0);
        let only_404 = rule(Some(&[404, 410]), 1);
        let wide = rule(None, 2);
        // 序号是规则在整张表里的位置，不是在这个子集里的位置
        let rules = [(2, &exempt_429), (3, &only_404), (5, &wide)];

        // 429 命中豁免：永远重试，不落到宽规则上
        let out = classify(50, response(429, "slow down"), &rules, None);
        assert!(matches!(out, Outcome::Retry { .. }));
        // 404 命中第二条：第 1 次就停
        let out = classify(1, response(404, "nope"), &rules, None);
        assert!(matches!(out, Outcome::Exhausted { rule: Some(3), .. }));
        // 其余状态码落到宽规则：第 2 次停
        let out = classify(1, response(500, "boom"), &rules, None);
        assert!(matches!(out, Outcome::Retry { .. }));
        let out = classify(2, response(500, "boom"), &rules, None);
        assert!(matches!(out, Outcome::Exhausted { rule: Some(5), .. }));
        // 200 但内容是错误：状态码是 200，同样按规则匹配
        let body = r#"{"type":"error","error":{"type":"overloaded_error"}}"#;
        let out = classify(2, response(200, body), &rules, None);
        assert!(matches!(
            out,
            Outcome::Exhausted { rule: Some(5), ref failure, .. } if failure.code == "upstream_error_body"
        ));
    }

    #[test]
    fn local_spool_failures_end_the_request() {
        assert!(matches!(
            classify(1, ForwardResult::TooLarge, &[], None),
            Outcome::TooLarge
        ));
        assert!(matches!(
            classify(
                1,
                ForwardResult::SpoolFailed(StorageOp::WriteResponse, "disk full".into()),
                &[], None
            ),
            Outcome::SpoolFailed(StorageOp::WriteResponse, e) if e == "disk full"
        ));
    }
}
