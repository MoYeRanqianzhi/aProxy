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
    /// 大体通过有界管道交给增量 JSON 解析器，不积攒整个上传；小时直接复用头部样本。
    parser: Option<(tokio::io::DuplexStream, tokio::task::JoinHandle<bool>)>,
    /// 上传结束后的解析结果，同时用于程序和请求记录。
    exact: Option<bool>,
    /// 请求体开头的至多 [`SHAPE_PREFIX`] 字节
    prefix: Vec<u8>,
    /// 已经收到的请求体字节数
    seen: u64,
    accept_sse: bool,
    trigger: crate::config::KeepaliveTrigger,
}

impl RequestTap {
    /// 边转发边取样。程序要确切标记时要看到整个 JSON 的结尾，后面的重复键或语法错误也算数。
    async fn chunk(&mut self, chunk: &Bytes) {
        if self.flag_out.is_some()
            && self.parser.is_none()
            && chunk.len() > SHAPE_PREFIX - self.prefix.len()
        {
            let (mut writer, reader) = tokio::io::duplex(SHAPE_PREFIX);
            let job = tokio::task::spawn_blocking(move || {
                json_requests_stream(serde_json::Deserializer::from_reader(
                    std::io::BufReader::new(tokio_util::io::SyncIoBridge::new(reader)),
                ))
            });
            // 解析器遇到非法 JSON 可以提前结束；关闭读端不是上传失败。
            let _ = writer.write_all(&self.prefix).await;
            self.parser = Some((writer, job));
        }
        if let Some((writer, _)) = &mut self.parser {
            let _ = writer.write_all(chunk).await;
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
    async fn finish(&mut self) {
        if let Some(out) = self.flag_out.take() {
            let flag = if let Some((writer, job)) = self.parser.take() {
                drop(writer);
                job.await.expect("stream flag parser")
            } else {
                json_requests_stream(serde_json::Deserializer::from_slice(&self.prefix))
            };
            self.exact = Some(flag);
            let _ = out.set(flag);
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

/// 响应转发途中顺带做的记录：每一块交给请求历史一份，数着已经转给客户端多少字节。响应流
/// 结束、出错或被丢弃（客户端断开）时，把这唯一的一次尝试记完。
struct ResponseTap {
    /// 输出长度由响应链单独记录；尝试长度始终只算原上游字节。
    transformed: bool,
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
        if !self.transformed {
            self.rec.response_bytes(self.forwarded);
        }
        self.rec.try_ended(
            1,
            self.forwarded,
            self.failure.as_ref().map(AttemptFailure::for_history),
        );
    }
}

/// 没有 request 层就直通；需要完整输入的程序已经检查过上限并保存了最终正文。
pub(super) enum ForwardBody<'a> {
    /// 尚未读取的客户端上传。
    Live(ClientUpload),
    /// 请求链改写后的内存或磁盘体，不再记录为客户端原体。
    Prepared(&'a RequestBody),
}
/// forward 只经过一次请求方向；即使程序要求重试也不重发，拒绝仍保留它给的客户端响应。
pub(super) async fn forward_after_chain(
    mut state: AppState,
    mut req: OutboundRequest,
    mut pending: Option<ClientUpload>,
    trace: RequestTrace,
) -> Response {
    let mut chain = RequestChain::new(&req);
    let mut r = Responder::buffered();
    let head = req.method == http::Method::HEAD;
    trace.rec.phase(Phase::Transforming);
    match chain
        .run(&mut state, &mut req, 1, &mut r, &mut pending)
        .await
    {
        Ok(changed) => {
            // 正文被替换时，上游不会再拉客户端上传。此时仍需检查原始请求的容量、记录与流标记。
            if req.replacement.is_some()
                && let Some(upload) = pending.take()
                && let Err(t) = upload.buffer(&state, &mut req, &trace.rec).await
            {
                return t.into_response(&trace);
            }
            if pending.is_none() {
                record_try(
                    &trace.rec,
                    1,
                    sent_head(&trace.rec, &req).as_ref(),
                    !changed,
                    &req,
                );
            }
        }
        Err(ChainError::Stop(stop)) => {
            finish_chain_stop(&state, &mut r, &trace, &req, 1, head, *stop).await;
            return r.into_response(&trace);
        }
        Err(ChainError::Retry(f)) => {
            record_try(&trace.rec, 1, None, true, &req);
            trace.rec.try_ended(1, 0, Some(f.for_history()));
            f.record_in(&state.stats);
            let terminal = Terminal::new("format_failed", f.message, f.details.into());
            terminal.record_in(&state.stats);
            return terminal.into_response(&trace);
        }
        Err(ChainError::Disconnected) => {
            unreachable!("buffered responder follows handler lifetime")
        }
    }
    trace.rec.phase(Phase::Attempting);
    let ctx = req.ctx.take();
    forward_only_proxy(
        state,
        &req.method,
        &req.target_url,
        &req.headers,
        pending.map_or_else(|| ForwardBody::Prepared(req.sent_body()), ForwardBody::Live),
        ctx,
        trace,
    )
    .await
}
/// 不使用重试、保活或内容拦截。没有对应方向的转换程序时，上传和下载都是真流式。
pub(super) async fn forward_only_proxy(
    state: AppState,
    method: &http::Method,
    target_url: &str,
    headers: &HeaderMap,
    body: ForwardBody<'_>,
    mut ctx: Option<ExchangeCtx>,
    mut trace: RequestTrace,
) -> Response {
    // 本模式恰好尝试一次：请求一发出就算一次上游尝试
    trace.attempts = 1;
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
    match body {
        ForwardBody::Prepared(b) => match b.apply_to(builder).await {
            Ok(prepared) => builder = prepared,
            Err(e) => {
                let t = Terminal::local_storage_failed(StorageOp::ReadRequest, &e.to_string());
                t.record_in(&state.stats);
                rec.try_ended(1, 0, Some(t.for_history()));
                return t.into_response(&trace);
            }
        },
        ForwardBody::Live(upload) => {
            let formatted = ctx.is_some();
            let need_flag = ctx.as_ref().is_some_and(|ctx| ctx.client.stream.is_none())
                && [
                    crate::config::file::Stage::Stream,
                    crate::config::file::Stage::Response,
                    crate::config::file::Stage::Notify,
                ]
                .into_iter()
                .any(|stage| state.has_format(stage));
            let flag_out = need_flag.then(|| Arc::new(std::sync::OnceLock::new()));
            if let Some(ctx) = &mut ctx {
                ctx.live_stream = flag_out.clone();
            }
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
            let sent_history = rec.try_started(
                1,
                sent.as_ref(),
                !changed,
                request_history_encoding(sent_encoding.as_deref(), formatted),
            );
            let tap = RequestTap {
                history: rec.request_body(request_history_encoding(
                    upload.encoding.as_ref(),
                    formatted,
                )),
                sent_history,
                rec: rec.clone(),
                prefix: Vec::new(),
                seen: 0,
                accept_sse: upload.accept,
                trigger: upload.trigger,
                flag_out,
                parser: None,
                exact: None,
            };
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
                            tap.finish().await;
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
                        tap.chunk(&chunk).await;
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
                rec.try_ended(1, 0, None);
                return body_too_large(limit).into_response(&trace);
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
        transformed: !status.is_client_error()
            && !status.is_server_error()
            && ctx.is_some()
            && (state.has_format(crate::config::file::Stage::Stream)
                || state.has_format(crate::config::file::Stage::Response)),
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
    let transformed = tap.transformed;
    let stream_state = state.clone();
    let stream =
        futures_util::stream::unfold((resp, tap, false), move |(mut resp, mut tap, done)| {
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
                        tap.failure = Some(failure.clone());
                        Some((Err(failure), (resp, tap, true)))
                    }
                }
            }
        });

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
            ctx.as_ref().expect("response chain context"),
            &meta,
            1,
            status,
            headers,
            content,
        )
        .await
        {
            Ok(output) => match output.response(&state, false).await {
                Ok(response) => response,
                Err(t) => {
                    t.record_in(&state.stats);
                    t.into_response(&trace)
                }
            },
            Err(t) => {
                t.record_in(&state.stats);
                t.into_response(&trace)
            }
        };
    }
    // 与 build_replay_response 同款收尾：header 逐条经 from_bytes 校验过、status
    // 已降级兜底，body() 的错误态不可能出现（框架保证，不做不可达分支）
    resp_builder
        .body(Body::from_stream(
            stream.map(|r| r.map_err(|f| std::io::Error::other(f.message))),
        ))
        .unwrap()
        .into_response()
}
