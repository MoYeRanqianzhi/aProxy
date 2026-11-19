//! 仅转发模式：真流式直通，不重试、不缓冲、不保活。

use super::*;
use crate::history::{BodyWriter, Phase, Recorder};

/// 请求体开头留多少字节来读 `model` 与 `stream`（见 [`RequestTap`]）
const SHAPE_PREFIX: usize = 64 * 1024;

/// 请求体转发途中顺带做的记录：每一块交给请求历史一份，并留住开头的一段——本模式不缓冲
/// 请求体，`model` 与 `stream` 只能从这一段里读，`model` 通常就在最前面。请求体流结束或被
/// 丢弃时，把读到的结果与字节数记进请求记录。
struct RequestTap {
    rec: Recorder,
    history: Option<BodyWriter>,
    /// 请求体开头的至多 [`SHAPE_PREFIX`] 字节
    prefix: Vec<u8>,
    /// 已经收到的请求体字节数
    seen: u64,
    accept_sse: bool,
    trigger: crate::config::KeepaliveTrigger,
}

impl RequestTap {
    fn chunk(&mut self, chunk: &Bytes) {
        self.seen += chunk.len() as u64;
        let room = SHAPE_PREFIX.saturating_sub(self.prefix.len());
        self.prefix
            .extend_from_slice(&chunk[..chunk.len().min(room)]);
        if let Some(history) = self.history.as_mut() {
            history.feed_copy(chunk);
        }
    }
}

impl Drop for RequestTap {
    fn drop(&mut self) {
        use crate::config::KeepaliveTrigger;
        let (model, body_stream) =
            probe_shape(serde_json::Deserializer::from_slice(&self.prefix), true);
        let body_stream = body_stream == Some(true);
        // 与缓冲路径同一套判定（见 keepalive_triggered），只是请求体那一半看的是开头一段
        let stream = match self.trigger {
            KeepaliveTrigger::Accept => self.accept_sse,
            KeepaliveTrigger::BodyStream => body_stream,
            KeepaliveTrigger::Any => self.accept_sse || body_stream,
        };
        // 写者先结束，体的状态才是「收完了」
        drop(self.history.take());
        self.rec.request_bytes(self.seen);
        self.rec.request_shape(model, stream);
    }
}

/// 响应转发途中顺带做的记录：每一块交给请求历史一份，数着已经转给客户端多少字节。响应流
/// 结束、出错或被丢弃（客户端断开）时，把这唯一的一次尝试记完。
struct ResponseTap {
    rec: Recorder,
    history: Option<BodyWriter>,
    /// 已经转给客户端的响应体字节数
    forwarded: u64,
    /// 这次尝试算失败的原因：上游回了错误状态码，或响应流中途断了
    failure: Option<AttemptFailure>,
}

impl Drop for ResponseTap {
    fn drop(&mut self) {
        drop(self.history.take());
        self.rec.response_bytes(self.forwarded);
        self.rec.try_ended(
            1,
            self.forwarded,
            self.failure.as_ref().map(AttemptFailure::for_history),
        );
    }
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
pub(super) async fn forward_only_proxy(
    state: AppState,
    method: http::Method,
    target_url: String,
    headers: HeaderMap,
    body: axum::body::Body,
    mut trace: RequestTrace,
) -> Response {
    // 本模式恰好尝试一次：请求一发出就算一次上游尝试
    trace.attempts = 1;
    // 请求历史：发出的体就是客户端的请求体（边收边发，同一份），所以这次尝试记为
    // 「与原始请求体相同」，体只存一份
    let rec = trace.rec.clone();
    let tap = RequestTap {
        history: rec.request_body(body_encoding(
            headers
                .get(http::header::CONTENT_ENCODING)
                .map(|v| v.as_bytes()),
        )),
        rec: rec.clone(),
        prefix: Vec::new(),
        seen: 0,
        accept_sse: accepts_event_stream(&headers),
        trigger: state.config.keepalive_trigger(),
    };
    let sent = rec.sent_head(
        method.as_str(),
        &target_url,
        headers
            .iter()
            .filter(|(name, _)| !is_hop_header(name.as_str()))
            .map(|(name, value)| (name.as_str(), value.as_bytes())),
    );
    rec.try_started(1, sent.as_ref(), true, None);
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
    // `ReadBodyError::ClientAborted` → 400 且**不碰** last_error（见 `read_request_body` 的
    // 错误分支），两条路径归因必须一致——仅转发模式此前是异常的那一侧。
    let client_gone = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let gone_flag = client_gone.clone();
    // unfold 状态 = (上游流, 已见字节数, 请求历史的记录)；产出的就是同一份 chunk（移动，
    // 不拷贝、不累积），故在途内存与负载大小无关
    let counted = futures_util::stream::unfold(
        (body.into_data_stream(), 0usize, tap),
        move |(mut stream, seen, mut tap)| {
            let flag = flag.clone();
            let gone_flag = gone_flag.clone();
            async move {
                // 流结束时 tap 随这个 future 一起被丢弃，请求体的记录就此收尾
                let chunk = stream.next().await?;
                let chunk = match chunk {
                    Ok(c) => c,
                    Err(e) => {
                        // 这里的 Err 只可能来自客户端侧（body 即客户端请求体）；
                        // 原样透传给 reqwest 以中止上游请求，真实原因由标记承载
                        gone_flag.store(true, AtomicOrdering::Relaxed);
                        return Some((Err(e), (stream, seen, tap)));
                    }
                };
                tap.chunk(&chunk);
                let seen = seen.saturating_add(chunk.len());
                if seen > limit {
                    flag.store(true, AtomicOrdering::Relaxed);
                    // 先把客户端剩下的请求体读掉，再用 Err 中止上游请求、回 413：
                    // 理由见 REJECTED_BODY_DRAIN_TOTAL。此间上游请求停在等下一块
                    // 数据上，不会多发任何字节
                    drain_rejected_body(&mut stream).await;
                    // axum::Error 满足 wrap_stream 的错误约束（Into<BoxError>），
                    // 无需 map_err；真实原因由 too_large 标记承载
                    return Some((
                        Err(axum::Error::new(std::io::Error::other(
                            "the request body exceeds the limit",
                        ))),
                        (stream, seen, tap),
                    ));
                }
                Some((Ok(chunk), (stream, seen, tap)))
            }
        },
    );
    // 以 chunked 发往上游：我们不再持有完整长度（长度要读完才知道，而读完即
    // 缓冲），content-length 无从回填——这是本模式的代价之一，也是必然后果。
    builder = builder.body(reqwest::Body::wrap_stream(counted));

    let resp = match send_with_timeout(&state, builder.send()).await {
        Ok(r) => r,
        Err(failure) => {
            // 错误文本与目标 URL 都会进日志 / 最近错误 / 502 正文，已在源头归类、脱敏
            let masked_target = state.history.redacted_url(&target_url);
            // send() 的失败有三种来源：上游真的失败，或请求体适配器产出 Err 令
            // reqwest 主动中止（超限 / 客户端上传中断，各有一个标记）。判定顺序
            // 是刻意的：超限排在最前——它同时也会让适配器产出 Err，而「你的请求体
            // 太大」比「你断开了」更接近事实、更可操作。两个标记都没置位才是真·上游失败。
            if too_large.load(AtomicOrdering::Relaxed) {
                rec.try_ended(1, 0, None);
                return body_too_large_response(limit, &trace);
            }
            // 客户端上传途中断开：**不记 last_error、不打「上游请求失败」**——
            // 那一头根本没有失败，是我们应客户端断开而主动中止了上游请求。
            // 也不回 502：发起断开的客户端读不到响应，这里的状态码只是给日志与
            // 中间观测看的，取与缓冲路径同一事件一致的 400。
            if client_gone.load(AtomicOrdering::Relaxed) {
                tracing::error!(code = %"client_body_aborted", error = %failure.message, target = %masked_target, "客户端请求体传输中断，已中止上游请求");
                rec.try_ended(1, 0, None);
                return Terminal::client_body_aborted().into_response(&trace);
            }
            // 上游失败不重试（本模式的定义），但必须记进实例的观测值，否则 status 的
            // 「最近错误」对这类实例永久显示「无」。这一次尝试记一次失败的尝试，随后的
            // 终态成为「最近一次失败」
            let terminal = Terminal::upstream_failed(&failure);
            failure.record_in(&state.stats);
            terminal.record_in(&state.stats);
            rec.try_ended(1, 0, Some(failure.for_history()));
            rec.decided(1, "default".to_string(), false);
            tracing::warn!(code = %terminal.code(), last_code = failure.code, error = %failure.message, target = %masked_target, "上游请求失败（仅转发模式不重试）");
            return terminal.into_response(&trace);
        }
    };

    let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    // 请求历史：上游实际回的响应头（逐跳头也在内），响应体边转边交一份。上游回了错误
    // 状态码时照样原样转给客户端，记录的结局是「上游错误」
    let mut tap = ResponseTap {
        history: rec.response_head(
            1,
            status.as_u16(),
            resp.headers()
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_bytes())),
            body_encoding(
                resp.headers()
                    .get(reqwest::header::CONTENT_ENCODING)
                    .map(|v| v.as_bytes()),
            ),
        ),
        rec: rec.clone(),
        forwarded: 0,
        failure: None,
    };
    rec.phase(Phase::Forwarding);
    if status.is_client_error() || status.is_server_error() {
        tap.failure = Some(AttemptFailure::response(status));
        rec.settle(
            crate::history::Outcome::UpstreamError,
            Some("upstream_failed"),
        );
    }
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
    // 即视为活跃；上游长时间一个字节都不发（真正的停滞，读空闲时限才是该管
    // 这件事的地方）期间不刷新——那时实例确实没在推进任何数据，被判闲置符合语义。
    // 代价是每 chunk 一次 SystemTime::now() + 一条 Relaxed store，相对该 chunk
    // 的网络读写开销可忽略。
    //
    // 逐块读、逐块交出：两次读到数据之间的时限（`timeouts.read_idle_secs`）在每一块上重新
    // 计时。流在第一个错误之后结束（`done`）。`tap` 跟着状态走：流结束、出错之后或客户端
    // 断开时它被丢弃，这次尝试就此记完。
    let stream =
        futures_util::stream::unfold((resp, tap, false), move |(mut resp, mut tap, done)| {
            let state = state.clone();
            async move {
                if done {
                    return None;
                }
                let forwarded = tap.forwarded;
                match next_chunk(&mut resp, &state, forwarded).await {
                    Ok(Some(chunk)) => {
                        state.touch_activity();
                        tap.forwarded += chunk.len() as u64;
                        if let Some(history) = tap.history.as_mut() {
                            history.feed_copy(&chunk);
                        }
                        Some((Ok(chunk), (resp, tap, false)))
                    }
                    Ok(None) => None,
                    Err(failure) => {
                        // 上游响应流中断：**直接截断**——绝不注入任何上游未发出的字节（那
                        // 是伪造数据），也不重试（本模式已放弃重试能力）。错误记进实例的
                        // 观测值并带上已转发字节数，便于判断断在了哪里。
                        failure.record_in(&state.stats);
                        tracing::warn!(
                            code = %failure.code,
                            error = %failure.message,
                            forwarded_bytes = forwarded,
                            "上游响应流中断，连接就此截断（仅转发模式不重试）"
                        );
                        // 请求记录：这次尝试断在半路，结局是「上游错误」
                        tap.rec.settle(
                            crate::history::Outcome::UpstreamError,
                            Some("upstream_failed"),
                        );
                        let message = failure.message.clone();
                        tap.failure = Some(failure);
                        Some((Err(std::io::Error::other(message)), (resp, tap, true)))
                    }
                }
            }
        });

    // 与 build_replay_response 同款收尾：header 逐条经 from_bytes 校验过、status
    // 已降级兜底，body() 的错误态不可能出现（框架保证，不做不可达分支）
    resp_builder
        .body(Body::from_stream(stream))
        .unwrap()
        .into_response()
}
