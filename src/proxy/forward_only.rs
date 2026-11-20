//! 仅转发模式：无格式程序时双向真流式直通；需要整包的层在其输入处缓冲，不重试、不保活。

use super::*;
use crate::history::{BodyWriter, Phase, Recorder};

/// 请求体开头留多少字节来读 `model` 与 `stream`（见 [`RequestTap`]）
const SHAPE_PREFIX: usize = 64 * 1024;

/// 请求体转发途中顺带做的记录：每一块交给请求历史一份，并留住开头的一段——本模式不缓冲
/// 请求体。仅有历史时从这一段取样；格式程序要确切标记时额外顺序扫描到 EOF。请求体流结束或被
/// 丢弃时，把读到的结果与字节数记进请求记录。
struct RequestTap {
    rec: Recorder,
    history: Option<BodyWriter>,
    /// 只改写编码头时，两份历史按各自的编码解读同一串上传字节。
    sent_history: Option<BodyWriter>,
    /// 完整上传解析结束时才通知响应方向，不能把没读到当作 false。
    flag_out: Option<Arc<std::sync::OnceLock<bool>>>,
    /// 超过样本上限时只保留有界的增量语法状态，不等待尚未到达的输入。
    parser: Option<super::live_flag::LiveFlag>,
    /// 仅完成通知协商摘要时顺手哈希，不把请求体留作重放。
    digest: Option<(sha2::Sha256, Arc<std::sync::OnceLock<String>>)>,
    /// 上传结束后的解析结果，同时用于程序和请求记录。
    exact: Option<bool>,
    /// 请求体开头的至多 [`SHAPE_PREFIX`] 字节
    prefix: Vec<u8>,
    /// 已经收到的请求体字节数
    seen: u64,
    accept_sse: bool,
    trigger: crate::config::KeepaliveTrigger,
    /// 最后释放，先让两个历史写者析构并结算原上传，再允许封存通知。
    join: Option<Arc<UpstreamJoin>>,
}

impl RequestTap {
    /// 两种消费实时上传的路径共用同一份原始历史、客户端流标记与按需摘要。
    fn new(
        upload: &ClientUpload,
        state: &AppState,
        ctx: Option<&ExchangeCtx>,
        rec: &Recorder,
    ) -> Self {
        let need_flag = ctx.is_some_and(|ctx| ctx.client.stream.is_none())
            && [
                crate::config::file::Stage::Stream,
                crate::config::file::Stage::Response,
                crate::config::file::Stage::Notify,
            ]
            .into_iter()
            .any(|stage| state.has_format(stage));
        let flag_out = ctx
            .and_then(|c| c.live_stream.clone())
            .filter(|_| need_flag);
        let digest = ctx
            .filter(|_| {
                state
                    .formats
                    .iter()
                    .any(|p| p.may_need_upload_digest(&[crate::config::file::Stage::Notify]))
            })
            .and_then(|c| {
                use axum::body::HttpBody as _;
                let hash = sha2::Sha256::default();
                // HTTP 可以不拉取已知为空的上传；此时已确认 EOF，不是传到一半的 null。
                if upload.body.is_end_stream() || upload.body.size_hint().exact() == Some(0) {
                    ExchangeCtx::finish_digest(&c.digest, hash);
                    None
                } else {
                    Some((hash, c.digest.clone()))
                }
            });
        let history = rec.request_body(request_history_encoding(
            upload.encoding.as_ref(),
            ctx.is_some(),
        ));
        Self {
            history,
            sent_history: None,
            rec: rec.clone(),
            prefix: Vec::new(),
            seen: 0,
            accept_sse: upload.accept,
            trigger: upload.trigger,
            join: None,
            flag_out,
            digest,
            parser: None,
            exact: None,
        }
    }
    /// 边转发边取样。程序要确切标记时要看到整个 JSON 的结尾，后面的重复键或语法错误也算数。
    fn chunk(&mut self, chunk: &Bytes) {
        if let Some((hash, _)) = &mut self.digest {
            sha2::Digest::update(hash, chunk);
        }
        if self.flag_out.is_some()
            && self.parser.is_none()
            && chunk.len() > SHAPE_PREFIX - self.prefix.len()
        {
            let mut parser = super::live_flag::LiveFlag::new();
            parser.feed(&self.prefix);
            self.parser = Some(parser);
        }
        if let Some(parser) = &mut self.parser {
            parser.feed(chunk);
        }
        if let Some(history) = self.sent_history.as_mut() {
            history.feed_copy(chunk);
        }
        self.seen += chunk.len() as u64;
        let room = SHAPE_PREFIX.saturating_sub(self.prefix.len());
        self.prefix
            .extend_from_slice(&chunk[..chunk.len().min(room)]);
        if let Some(history) = self.history.as_mut() {
            history.feed_copy(chunk);
        }
    }
    /// 只在客户端上传抵达 EOF 时结算，不让取消的流被认作完整 JSON。
    fn finish(&mut self) {
        if let Some((hash, out)) = self.digest.take() {
            ExchangeCtx::finish_digest(&out, hash);
        }
        if let Some(out) = self.flag_out.take() {
            // 无论正文是否超过历史前缀，都只把完整文档的判定发布给后续层。
            let parser = self.parser.take().unwrap_or_else(|| {
                let mut parser = super::live_flag::LiveFlag::new();
                parser.feed(&self.prefix);
                parser
            });
            self.exact = parser.finish();
            if let Some(flag) = self.exact {
                let _ = out.set(flag);
            }
        }
    }
}

impl Drop for RequestTap {
    fn drop(&mut self) {
        use crate::config::KeepaliveTrigger;
        let (model, body_stream) =
            probe_shape(serde_json::Deserializer::from_slice(&self.prefix), true);
        let body_stream = self.exact.unwrap_or(body_stream == Some(true));
        // 与缓冲路径同一套判定；没有确切结果时，历史仍按原有的开头样本展示。
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

/// 程序已经选定替换正文：仍要消费原始上传以检查限额并结算历史、摘要，但绝不保留重放体。
pub(super) async fn drain_original(
    upload: ClientUpload,
    state: &AppState,
    req: &mut OutboundRequest,
    rec: &Recorder,
) -> Result<bool, Terminal> {
    let mut tap = RequestTap::new(&upload, state, req.ctx.as_ref(), rec);
    let limit = state.config.body_limit_bytes();
    let mut input = upload.body.into_data_stream();
    while let Some(part) = input.next().await {
        let part = part.map_err(|_| Terminal::client_body_aborted())?;
        tap.chunk(&part);
        if tap.seen > limit as u64 {
            return Err(body_too_large(limit));
        }
    }
    tap.finish();
    req.body_stream = tap.exact;
    Ok(req
        .ctx
        .as_ref()
        .and_then(|c| c.wire_client().stream)
        .unwrap_or(false))
}

/// 小实体可解码后整体检查；越过驻留界限时只向单行扫描器移交前缀，不落盘也不保留整串。
enum PassiveCheck {
    /// 原始表示，不同于程序改写后交给客户端的字节。
    Resident(Vec<u8>),
    /// 与 retry 磁盘路径的原始 SSE / NDJSON 行判定一致，不解码超大下载。
    Lines(StreamErrorScanner),
}
impl PassiveCheck {
    /// 只有跨过界限时才付逐行扫描的成本；分小段喂前缀，不把一大块误当成一条过长的行。
    fn feed(&mut self, bytes: &[u8]) {
        if let Self::Resident(prefix) = self {
            if bytes.len() <= RESIDENT_LIMIT - prefix.len() {
                prefix.extend_from_slice(bytes);
                return;
            }
            let mut lines = StreamErrorScanner::new();
            for piece in prefix.chunks(16 * 1024) {
                lines.feed(piece);
            }
            *self = Self::Lines(lines);
        }
        if let Self::Lines(lines) = self {
            for piece in bytes.chunks(16 * 1024) {
                lines.feed(piece);
            }
        }
    }
    /// 只有 HTTP 实体完整时才判伪成功；中断可能截在合法 JSON 当中，不能被误报为错误体。
    fn finish(self, headers: &HeaderMap) -> bool {
        match self {
            Self::Resident(raw) => {
                let encoding = request_encoding(headers, true);
                let decoded = crate::decode::for_inspection(
                    encoding.as_deref().and_then(|v| v.to_str().ok()),
                    &raw,
                );
                let body = decoded.as_deref().unwrap_or(&raw);
                if retry::is_streaming_response(headers, body) {
                    retry::is_stream_error_body(body)
                } else {
                    retry::is_error_body(body)
                }
            }
            Self::Lines(lines) => lines.finish(),
        }
    }
}

/// 响应转发途中顺带做的记录：每一块交给请求历史一份，数着已经转给客户端多少字节。响应流
/// 结束、出错或被丢弃（客户端断开）时，把这唯一的一次尝试记完。
struct ResponseTap {
    /// 管道接收端只活到 HTTP 出口选定响应。在此之前上游读错交回尝试循环，之后只能截断。
    precommit: Option<tokio::sync::oneshot::Sender<ForwardResult>>,
    /// 原始字节数和耗时在 Drop 中先落定，随后释放这张封存票。
    _join: Option<Arc<UpstreamJoin>>,
    /// 输出长度由响应链单独记录；尝试长度始终只算原上游字节。
    transformed: bool,
    /// 请求方向本地失败不算上游请求，但占一次逻辑尝试。
    attempt: u32,
    rec: Recorder,
    history: Option<BodyWriter>,
    /// 已经转给客户端的响应体字节数
    forwarded: u64,
    /// 这次尝试算失败的原因：上游回了错误状态码，或响应流中途断了
    failure: Option<AttemptFailure>,
    /// 只在成功状态检查一次；状态错误和断流本身已足以说明失败。
    check: Option<PassiveCheck>,
}
impl ResponseTap {
    /// 定长实体最后一块交付后 hyper 可以不再轮询 EOF，按实际累计长度在那一块完成旁路判定。
    fn complete(&mut self, state: &AppState, status: StatusCode, headers: &HeaderMap) {
        if self.check.take().is_some_and(|c| c.finish(headers)) {
            let failure = AttemptFailure::response(status);
            failure.record_in(&state.stats);
            self.rec.settle(
                crate::history::Outcome::UpstreamError,
                Some("upstream_failed"),
            );
            self.failure = Some(failure);
        }
    }
}

impl Drop for ResponseTap {
    fn drop(&mut self) {
        drop(self.history.take());
        if !self.transformed {
            self.rec.response_bytes(self.forwarded);
        }
        self.rec.try_ended(
            self.attempt,
            self.forwarded,
            self.failure.as_ref().map(AttemptFailure::for_history),
        );
    }
}

/// 不使用重试、保活或内容拦截。没有对应方向的转换程序时，上传和下载都是真流式。
#[allow(clippy::too_many_arguments)]
pub(super) async fn forward_only_proxy(
    state: AppState,
    method: &http::Method,
    target_url: &str,
    headers: &HeaderMap,
    upload: ClientUpload,
    ctx: Option<ExchangeCtx>,
    trace: &RequestTrace,
    attempt: u32,
) -> Result<Response, Terminal> {
    // 请求历史：发出的体就是客户端的请求体（边收边发，同一份），所以这次尝试记为
    // 「与原始请求体相同」，体只存一份
    let rec = trace.rec.clone();
    let reqwest_method =
        reqwest::Method::from_bytes(method.as_str().as_bytes()).unwrap_or(reqwest::Method::GET);

    let mut builder = state.client.request(reqwest_method, target_url);

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

    // 客户端侧请求体中断标记：与 too_large 同款——适配器产出的 Err 会被 reqwest
    // 当作「body 出错」进而中止上游请求，`send()` 随之返回 Err，两种来源在下游
    // 长得一模一样，只能靠标记区分。客户端在上传途中断开（裸 TCP 声明
    // Content-Length 却提前 close、进程被杀……）必须与「上游请求失败」分开：
    // 后者会把排障矛头指向根本没有收到完整请求体的上游。缓冲路径的同一事件映射为
    // `ReadBodyError::ClientAborted` → 400 且**不碰** last_error（见 `read_request_body` 的
    // 错误分支），两条路径归因必须一致——仅转发模式此前是异常的那一侧。
    let client_gone = Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let formatted = ctx.is_some();
        let sent = rec.sent_head(
            method.as_str(),
            target_url,
            headers
                .iter()
                .filter(|(name, _)| !is_hop_header(name.as_str()))
                .map(|(name, value)| (name.as_str(), value.as_bytes())),
        );
        let sent_encoding = request_encoding(headers, formatted);
        let changed = formatted && sent_encoding.as_deref() != upload.encoding.as_ref();
        // try_started 对相同的实体直接引用已登记的原始体，必须在创建尝试前登记它。
        let mut tap = RequestTap::new(&upload, &state, ctx.as_ref(), &rec);
        tap.join = trace.upstream_join.upgrade();
        tap.sent_history = rec.try_started(
            attempt,
            sent.as_ref(),
            !changed,
            request_history_encoding(sent_encoding.as_deref(), formatted),
        );
        let body = upload.body;
        let flag = too_large.clone();
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
                    let Some(chunk) = stream.next().await else {
                        tap.finish();
                        return None;
                    };
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
    }
    let resp = match send_with_timeout(&state, builder.send()).await {
        Ok(r) => r,
        Err(failure) => {
            // 错误文本与目标 URL 都会进日志 / 最近错误 / 502 正文，已在源头归类、脱敏
            let masked_target = state.history.redacted_url(target_url);
            // send() 的失败有三种来源：上游真的失败，或请求体适配器产出 Err 令
            // reqwest 主动中止（超限 / 客户端上传中断，各有一个标记）。判定顺序
            // 是刻意的：超限排在最前——它同时也会让适配器产出 Err，而「你的请求体
            // 太大」比「你断开了」更接近事实、更可操作。两个标记都没置位才是真·上游失败。
            if too_large.load(AtomicOrdering::Relaxed) {
                rec.try_ended(attempt, 0, None);
                return Err(body_too_large(limit));
            }
            // 客户端上传途中断开：**不记 last_error、不打「上游请求失败」**——
            // 那一头根本没有失败，是我们应客户端断开而主动中止了上游请求。
            // 也不回 502：发起断开的客户端读不到响应，这里的状态码只是给日志与
            // 中间观测看的，取与缓冲路径同一事件一致的 400。
            if client_gone.load(AtomicOrdering::Relaxed) {
                tracing::error!(code = %"client_body_aborted", error = %failure.message, target = %masked_target, "客户端请求体传输中断，已中止上游请求");
                rec.try_ended(attempt, 0, None);
                return Err(Terminal::client_body_aborted());
            }
            // 上游失败不重试（本模式的定义），但必须记进实例的观测值，否则 status 的
            // 「最近错误」对这类实例永久显示「无」。这一次尝试记一次失败的尝试，随后的
            // 终态成为「最近一次失败」
            let terminal = Terminal::upstream_failed(&failure);
            failure.record_in(&state.stats);
            terminal.record_in(&state.stats);
            rec.try_ended(attempt, 0, Some(failure.for_history()));
            rec.decided(attempt, "default".to_string(), false);
            tracing::warn!(code = %terminal.code(), last_code = failure.code, error = %failure.message, target = %masked_target, "上游请求失败（仅转发模式不重试）");
            return Err(terminal);
        }
    };

    forward_response(
        state,
        method,
        target_url,
        ctx.as_ref(),
        trace,
        attempt,
        resp,
        None,
    )
    .await
}

/// 收到上游头后才选择真流式交付；保留原尝试序号，不因请求层在本地失败过而少算。
#[allow(clippy::too_many_arguments)]
pub(super) async fn forward_response(
    state: AppState,
    method: &http::Method,
    target_url: &str,
    ctx: Option<&ExchangeCtx>,
    trace: &RequestTrace,
    attempt: u32,
    resp: reqwest::Response,
    precommit: Option<tokio::sync::oneshot::Sender<ForwardResult>>,
) -> Result<Response, Terminal> {
    let rec = &trace.rec;
    let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    // 请求历史：上游实际回的响应头（逐跳头也在内），响应体边转边交一份。上游回了错误
    // 状态码时照样原样转给客户端，记录的结局是「上游错误」
    let mut tap = ResponseTap {
        precommit,
        _join: trace.upstream_join.upgrade(),
        transformed: status.is_success()
            && ctx.is_some()
            && (state.has_format(crate::config::file::Stage::Stream)
                || state.has_format(crate::config::file::Stage::Response)),
        attempt,
        history: rec.response_head(
            attempt,
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
        check: status
            .is_success()
            .then(|| PassiveCheck::Resident(Vec::new())),
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

    // 响应原块直回客户端；旁路最多保留驻留上限的原始字节，越过上限只扫描行，
    // 判定只进历史与通知，不改变交付的内容。客户端断开时本 Body 被 drop，reqwest 连接随之
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
    let expected = resp.content_length();
    let transformed = tap.transformed;
    let stream_state = state.clone();
    let stream = futures_util::stream::unfold(
        (resp, tap, false),
        move |(mut resp, mut tap, done)| {
            let state = stream_state.clone();
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
                        if let Some(check) = &mut tap.check {
                            check.feed(&chunk);
                        }
                        if expected == Some(tap.forwarded) {
                            tap.complete(&state, status, resp.headers());
                        }
                        Some((Ok(chunk), (resp, tap, false)))
                    }
                    Ok(None) => {
                        tap.complete(&state, status, resp.headers());
                        None
                    }
                    Err(failure) => {
                        // 已有 decide 且响应仍在整包层中，错误可以回到尝试循环；
                        // 接收端在交付前关闭，此后即使后台流正在预取也不能撤回响应。
                        let handed_back = tap.precommit.take().is_some_and(|tx| {
                            !tx.is_closed()
                                && tx
                                    .send(ForwardResult::NetworkError(
                                        failure.clone(),
                                        Some(Box::new((status, resp.headers().clone()))),
                                    ))
                                    .is_ok()
                        });
                        if !handed_back {
                            failure.record_in(&state.stats);
                            tracing::warn!(code=%failure.code, error=%failure.message, forwarded_bytes=forwarded, "forward upstream response interrupted");
                            tap.rec.settle(
                                crate::history::Outcome::UpstreamError,
                                Some("upstream_failed"),
                            );
                            tap.rec.stream_aborted();
                        }
                        tap.failure = Some(failure.clone());
                        Some((Err(failure), (resp, tap, true)))
                    }
                }
            }
        },
    );

    if transformed {
        let meta = aproxy_envelope::RequestMeta::new(method.to_string(), target_url.to_owned());
        let headers = std::mem::take(
            resp_builder
                .headers_mut()
                .expect("validated upstream headers"),
        );
        let content =
            streams::Content::Flow(stream.map(|r| r.map_err(streams::End::attempt)).boxed());
        return match streams::chain(
            &state,
            ctx.expect("response chain context"),
            &meta,
            attempt,
            None,
            status,
            headers,
            content,
        )
        .await
        {
            Ok(output) => match output.response(&state, false).await {
                Ok(response) => Ok(response),
                Err(t) => {
                    t.record_in(&state.stats);
                    Err(t)
                }
            },
            Err(t) => {
                t.record_in(&state.stats);
                Err(t)
            }
        };
    }
    // 与 build_replay_response 同款收尾：header 逐条经 from_bytes 校验过、status
    // 已降级兜底，body() 的错误态不可能出现（框架保证，不做不可达分支）
    Ok(resp_builder
        .body(Body::from_stream(
            stream.map(|r| r.map_err(|f| std::io::Error::other(f.message))),
        ))
        .unwrap()
        .into_response())
}

/// 上传探针的资源隔离与最终结果。
#[cfg(test)]
mod tests {
    use super::*;

    /// 未收完的上传停在一个任意长的属性名中间；其它请求的文件任务不依赖其 EOF。
    #[test]
    fn idle_live_upload_does_not_occupy_a_shared_blocking_thread() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .max_blocking_threads(1)
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let out = Arc::new(std::sync::OnceLock::new());
            let mut tap = RequestTap {
                rec: Recorder::none(),
                history: None,
                sent_history: None,
                flag_out: Some(out.clone()),
                parser: None,
                digest: None,
                exact: None,
                prefix: Vec::new(),
                seen: 0,
                accept_sse: false,
                trigger: crate::config::KeepaliveTrigger::BodyStream,
                join: None,
            };
            tap.chunk(&Bytes::from_static(b"{\""));
            let part = Bytes::from(vec![b'x'; SHAPE_PREFIX]);
            for _ in 0..16 {
                tap.chunk(&part);
            }
            let result =
                tokio::time::timeout(Duration::from_secs(5), tokio::task::spawn_blocking(|| 7))
                    .await;
            tap.chunk(&Bytes::from_static(br#"":null,"stream":true}"#));
            tap.finish();
            assert_eq!(
                result
                    .expect("idle upload held the shared blocking pool")
                    .unwrap(),
                7
            );
            assert_eq!(out.get(), Some(&true));
        });
    }
}
