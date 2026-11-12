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
    /// 需要重试。`note` 记进实例的「最近错误」（status 与界面里看得到）
    Retry { note: String },
    /// 受限重试路径用完了尝试次数：不再重试，交付这次失败的响应——客户端拿到真实的
    /// 404 自行处理，好过永远等不到终态（compact 事故，见记忆 bounded-retry-paths）
    Exhausted(UpstreamResponse),
    /// 上游响应体超出 spool 上限：确定性失败，重试无意义
    TooLarge,
    /// 本地磁盘缓存写入失败：确定性失败
    SpoolFailed(String),
}

/// 判定一次尝试的结果。只看结果本身、第几次尝试与是否命中受限重试路径，不碰任何状态，
/// 所以可以单测；重试与否的细则在 `needs_retry_response`（状态码、错误体、流内错误事件）。
///
/// 受限路径只封顶「上游有响应」的失败：网络错误是真正的瞬时类，仍然无限重试，而且它
/// 没有响应可供交付。
pub(super) fn classify(attempt: u32, result: ForwardResult, bounded_retry: bool) -> Outcome {
    match result {
        ForwardResult::NetworkError(e) => Outcome::Retry {
            note: format!("网络错误: {e}"),
        },
        ForwardResult::TooLarge => Outcome::TooLarge,
        ForwardResult::SpoolFailed(e) => Outcome::SpoolFailed(e),
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
                Outcome::Deliver(response)
            } else if bounded_retry && attempt >= retry::BOUNDED_RETRY_MAX_ATTEMPTS {
                Outcome::Exhausted(response)
            } else {
                // 进入下一轮时 body 随 response 一起 Drop：磁盘临时文件删除
                Outcome::Retry {
                    note: format!("上游返回 {status}（错误内容，重试）"),
                }
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
        ticker: tokio::time::Interval,
        /// 上游真实的 SSE 响应头能不能先行提交：配置了响应转换时不能（转换可能改头）
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
    ) -> Self {
        let interval = state.config.keepalive_interval();
        let mut ticker = tokio::time::interval_at(tokio::time::Instant::now() + interval, interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        Self::Keepalive {
            sink: KeepaliveSink::Pending {
                resp_tx: Some(resp_tx),
                skeleton_due: false,
                beat: Heartbeat {
                    fixed: Bytes::copy_from_slice(state.config.keepalive_heartbeat()),
                    dynamic: None,
                },
            },
            ticker,
            real_head_allowed: state.response_pool.is_none(),
        }
    }

    /// 已经向客户端提交了响应头（只有保活出口会提交）：此后 status 与响应头不能再改，
    /// 只能往 SSE 体里写
    fn is_committed(&self) -> bool {
        match self {
            Self::Buffered { .. } => false,
            Self::Keepalive { sink, .. } => sink.is_committed(),
        }
    }

    /// 等 `fut` 完成。None = 客户端已断开（只有保活出口会感知到；`fut` 随之被 drop，在途的
    /// 上游连接关闭）。保活出口在等待期间照常心跳或提交骨架头；`head_rx` 是本次尝试的响应头
    /// 通知，见 [`Responder::head_channel`]。
    async fn wait<F: std::future::Future>(
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
            } => drive(sink, ticker, fut, head_rx, *real_head_allowed, during).await,
        }
    }

    /// 本次尝试要不要上游响应头的通知：保活出口在提交之前要（可提交的上游真实头先行提交、
    /// 「2xx 非 SSE」暂停骨架提交，见 drive），其余情况不要
    fn head_channel(&self) -> (Option<HeadTx>, Option<HeadRx>) {
        match self {
            Self::Keepalive { sink, .. } if !sink.is_committed() => {
                let (tx, rx) = tokio::sync::oneshot::channel();
                (Some(tx), Some(rx))
            }
            _ => (None, None),
        }
    }

    /// 发往上游的请求头的出口要求：保活出口会往响应体里插心跳注释，只有未压缩的体才能这样
    /// 插（往 gzip / br 流里插明文，客户端解压必坏），所以要求上游以 identity 编码回应。客户端
    /// 照旧拿到合法响应（未压缩永远是可接受的编码），代价只是上游到本机这一段多传些字节。
    /// 放在请求转换之后，覆盖 format 可能写入的同名头（override_headers 里配的同名头也被覆盖，
    /// 启动时 server.rs 会 warn 一次）。
    fn prepare_upstream_headers(&self, headers: &mut HeaderMap) {
        if let Self::Keepalive { .. } = self {
            headers.insert(
                http::header::ACCEPT_ENCODING,
                HeaderValue::from_static("identity"),
            );
        }
    }

    /// 配置了心跳转换时，保活出口改由心跳转换器逐拍生成心跳（请求转换完成之后才有它要的请求）
    fn arm_dynamic_heartbeat(&mut self, state: &AppState, req: &OutboundRequest) {
        if let (Self::Keepalive { sink, .. }, Some(pool)) = (self, &state.heartbeat_pool) {
            sink.arm_dynamic_heartbeat(Arc::new(DynamicHeartbeat {
                pool: pool.clone(),
                ctx: req.ctx.clone(),
                source: HeartbeatSource::new(&req.method, &req.target_url, &req.headers, &req.body),
                seq: Default::default(),
                attempt: Default::default(),
                in_flight: Default::default(),
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
    fn respond(&mut self, resp: Response) {
        match self {
            Self::Buffered { out } => *out = Some(resp),
            Self::Keepalive { sink, .. } => sink.respond(resp),
        }
    }

    /// 往已提交的响应体里写一块。false = 客户端已断开
    async fn send(&mut self, chunk: Result<Bytes, std::io::Error>) -> bool {
        match self {
            Self::Buffered { .. } => false,
            Self::Keepalive { sink, .. } => sink.send(chunk).await,
        }
    }

    /// 终态：尚未提交 → 完整响应；已提交 → SSE 终态 error 事件（状态行已发出、不可再改，
    /// 客户端借此明确知道代理放弃了这条请求，而不是永远等下去）
    async fn finish(&mut self, response: impl FnOnce() -> Response, event: Bytes) {
        if self.is_committed() {
            self.send(Ok(event)).await;
        } else {
            self.respond(response());
        }
    }

    fn log_client_gone(&self, during: &str) {
        if let Self::Keepalive { sink, .. } = self {
            sink.log_client_gone(during);
        }
    }

    /// 不保活出口交出的响应。每条出口路径都会设上；万一漏了，给客户端一个明确的 502，
    /// 而不是让连接悬空
    pub(super) fn into_response(self) -> Response {
        match self {
            Self::Buffered { out: Some(resp) } => resp,
            _ => {
                tracing::error!("尝试循环没有交出响应");
                (
                    StatusCode::BAD_GATEWAY,
                    "aProxy 内部错误：尝试循环没有交出响应",
                )
                    .into_response()
            }
        }
    }
}

/// 一个请求的全部尝试：请求转换 → 逐次发往上游并判定 → 按出口收场。返回时结果已交给
/// `r`（不保活出口取 [`Responder::into_response`]），或客户端已断开。
///
/// 请求转换也经 `r.wait` 驱动：保活出口里它同样处在心跳节拍内——format 慢（timeout_secs = 0
/// 时没有上界）时客户端照样在一个间隔后收到骨架头与心跳，而不是在拿到任何字节之前干等。
pub(super) async fn run_attempts(
    state: &AppState,
    req: OutboundRequest,
    path_and_query: &str,
    max_spool_bytes: usize,
    bounded_retry: bool,
    r: &mut Responder,
) {
    let mut req = match r
        .wait(apply_request_transform(state, req), None, "请求转换")
        .await
    {
        None => return,
        Some(Ok(req)) => req,
        Some(Err(msg)) => {
            let event = terminal_error_event("proxy_transform_failed", &msg);
            r.finish(|| request_transform_failed_response(&msg), event)
                .await;
            return;
        }
    };
    r.prepare_upstream_headers(&mut req.headers);
    log_proxied_request(&req.method, path_and_query, &req.target_url);
    r.arm_dynamic_heartbeat(state, &req);

    let max_backoff = state.config.max_retry_backoff_secs;
    let mut attempt: u32 = 0;
    loop {
        attempt += 1;
        r.note_attempt(attempt);
        if attempt > 1 {
            // 每轮重试刷新活动时间戳：last_activity_secs 的语义是「最近一次收到请求或仍在
            // 处理」——长退避可能超过 idle_timeout_secs，不刷新的话无限重试中的实例会被
            // stop idle / status --idle 误判为闲置并强退，恰恰杀掉最需要「不中断」保障的
            // 实例（M1）。放在重试轮开头而非退避之后：退避等待期间的实例也视为活跃。
            state.touch_activity();
            state
                .stats
                .retries_total
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let delay = retry::delay_for_attempt(attempt - 1, max_backoff);
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
            // every_attempt：重试前用客户端的原始请求重新转换出这次要发的请求（失败沿用
            // 上一次的）
            if let Some(rt) = req.retransform.as_mut() {
                let next = rt.next(state, &req.ctx, attempt);
                match r.wait(next, None, "请求转换（重试前）").await {
                    None => return,
                    Some(Some(t)) => {
                        req.method = t.method;
                        req.target_url = t.url;
                        req.headers = t.headers;
                        req.body = t.body;
                        r.prepare_upstream_headers(&mut req.headers);
                    }
                    Some(None) => {}
                }
            }
        }

        let (head_tx, head_rx) = r.head_channel();
        let attempt_fut = forward_once(
            state,
            &req.method,
            &req.target_url,
            &req.headers,
            &req.body,
            max_spool_bytes,
            head_tx,
        );
        let Some(result) = r.wait(attempt_fut, head_rx, "等待上游").await else {
            return;
        };
        if let Some(rt) = req.retransform.as_mut() {
            rt.record(&result);
        }

        match classify(attempt, result, bounded_retry) {
            Outcome::Retry { note } => {
                state.note_upstream_failure(&note);
                tracing::warn!(attempt, reason = %note, "本次尝试失败，重试");
                // 需要重试时不在这里提交——保活出口的提交只由节拍与可提交的上游响应头推动，
                // 下一轮尝试很快成功时客户端仍拿到上游真实头
            }
            Outcome::Deliver(response) => {
                deliver(state, r, &req, attempt, response, max_spool_bytes).await;
                return;
            }
            Outcome::Exhausted(response) => {
                let status = response.status;
                tracing::warn!(attempt, status = %status, "受限重试路径达到尝试上限，不再重试");
                if !r.is_committed() {
                    // 尚未向客户端写出任何字节：原样交付最后一次失败响应
                    state.note_upstream_failure(&format!(
                        "上游返回 {status}（受限重试路径，达上限透传）"
                    ));
                    let UpstreamResponse {
                        status,
                        headers,
                        raw_headers,
                        body,
                    } = response;
                    r.respond(replay_failure_as_is(status, headers, &raw_headers, body).await);
                } else {
                    // 已提交：状态行已发出，真实 status 与头无法交付，以终态 error 事件收场
                    state.note_upstream_failure(&format!(
                        "上游返回 {status}（受限重试路径，达上限终止）"
                    ));
                    drop(response);
                    let event = Bytes::from(format!(
                        "event: error\ndata: {{\"type\":\"error\",\"error\":{{\"type\":\"upstream_error\",\"message\":\"upstream returned {status}; bounded retry path exhausted after {attempt} attempts\"}}}}\n\n"
                    ));
                    r.send(Ok(event)).await;
                }
                return;
            }
            Outcome::TooLarge => {
                state.note_upstream_failure("上游响应体超出 spool 上限");
                tracing::error!(attempt, "上游响应体超出 spool 上限，终止重试");
                let event = Bytes::from_static(
                    b"event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"proxy_spool_limit\",\"message\":\"upstream response exceeded proxy spool limit\"}}\n\n",
                );
                r.finish(too_large_response, event).await;
                return;
            }
            Outcome::SpoolFailed(e) => {
                state.note_upstream_failure(&format!("本地 spool 故障: {e}"));
                tracing::error!(attempt, error = %e, "本地 spool 故障，终止重试");
                let event = Bytes::from_static(
                    b"event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"proxy_spool_failed\",\"message\":\"local disk cache write failed\"}}\n\n",
                );
                r.finish(|| spool_failed_response(&e), event).await;
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
    attempt: u32,
    response: UpstreamResponse,
    max_spool_bytes: usize,
) {
    let UpstreamResponse {
        status,
        headers,
        raw_headers,
        body,
    } = response;
    let upstream_sse = is_event_stream(&headers);
    let transform =
        transform_response_if_configured(state, &req.target_url, &req.ctx, headers, body);
    let transformed = if !r.is_committed() && !upstream_sse {
        Some(transform.await)
    } else {
        r.wait(transform, None, "响应转换").await
    };
    let Some((headers, body)) = transformed else {
        return;
    };
    if !r.is_committed() {
        // 提交前就成功：保真快速路径（上游 status 与转换后的响应头）
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
                    state.note_upstream_failure(&format!(
                        "回放前解码上游压缩响应失败（{encoding}）: {e}"
                    ));
                    tracing::warn!(
                        attempt,
                        encoding = %encoding,
                        error = %e,
                        "已提交的响应无法回放上游压缩体且解码失败，以终态 error 事件收场（保活通道）"
                    );
                    let event = Bytes::from_static(
                        b"event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"proxy_decode_failed\",\"message\":\"upstream response used a content-encoding the committed stream cannot carry, and decoding it failed\"}}\n\n",
                    );
                    r.send(Ok(event)).await;
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
        if !r.send(item).await {
            // 客户端在回放中断开：Drop 链负责删临时文件
            r.log_client_gone("回放");
            return;
        }
    }
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
        let out = classify(1, response(200, r#"{"id":"msg_1"}"#), false);
        assert!(matches!(out, Outcome::Deliver(r) if r.status == 200));
    }

    #[test]
    fn error_status_and_error_content_are_retried() {
        let out = classify(1, response(503, "overloaded"), false);
        assert!(matches!(out, Outcome::Retry { ref note } if note.contains("503")));
        // 200 但内容是错误：同样重试
        let out = classify(
            1,
            response(
                200,
                r#"{"type":"error","error":{"type":"overloaded_error"}}"#,
            ),
            false,
        );
        assert!(matches!(out, Outcome::Retry { .. }));
    }

    #[test]
    fn network_errors_are_retried_without_limit_even_on_bounded_paths() {
        for attempt in [1, retry::BOUNDED_RETRY_MAX_ATTEMPTS, 100] {
            let out = classify(
                attempt,
                ForwardResult::NetworkError("connection refused".into()),
                true,
            );
            assert!(
                matches!(out, Outcome::Retry { ref note } if note.contains("connection refused")),
                "第 {attempt} 次"
            );
        }
    }

    #[test]
    fn bounded_paths_stop_at_the_attempt_limit() {
        let max = retry::BOUNDED_RETRY_MAX_ATTEMPTS;
        for attempt in 1..max {
            let out = classify(attempt, response(404, "not found"), true);
            assert!(matches!(out, Outcome::Retry { .. }), "第 {attempt} 次");
        }
        let out = classify(max, response(404, "not found"), true);
        assert!(matches!(out, Outcome::Exhausted(r) if r.status == 404));
        // 不受限的路径不封顶
        let out = classify(max + 10, response(404, "not found"), false);
        assert!(matches!(out, Outcome::Retry { .. }));
    }

    #[test]
    fn local_spool_failures_end_the_request() {
        assert!(matches!(
            classify(1, ForwardResult::TooLarge, false),
            Outcome::TooLarge
        ));
        assert!(matches!(
            classify(1, ForwardResult::SpoolFailed("disk full".into()), false),
            Outcome::SpoolFailed(e) if e == "disk full"
        ));
    }
}
