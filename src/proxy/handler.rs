//! 入口：路由与请求处理的分派（入站校验 → 计数 → 仅转发分叉 → 读请求体 → 选通道）。

use super::*;
use crate::history::Phase;
use tracing::Instrument as _;

/// 构建路由：全量透传（无额外健康检查路径，避免与上游 `/health` 等冲突）
pub fn router(state: AppState) -> Router {
    Router::new().fallback(proxy_handler).with_state(state)
}

/// 核心代理处理器：完整透传 + 无限重试 + 流式 spool 后回放 + 保活心跳
pub(super) async fn proxy_handler(State(state): State<AppState>, req: Request) -> Response {
    let decide_started = state
        .has_format(crate::config::file::Stage::Decide)
        .then(tokio::time::Instant::now);
    let method = req.method().clone();
    let uri = req.uri().clone();

    // 入站来源校验：必须是第一步——先于请求体读取（被拒请求不值得缓冲/落盘
    // 任何字节）、先于 apply_header_overrides（绝不给它注入 api_key）、先于
    // 仅转发分支与重试循环（绝不转发）。也先于活动时间戳与请求计数：被拒的
    // 请求不是代理流量，不应让探测者把实例「刷」成活跃、也不计入请求数。
    // 它们的错误响应同样带请求标识，用单独的序号编（见 AppState::rejected_request_id）。
    if let Some(rejection) = state.inbound.check(req.headers(), &uri) {
        let trace = RequestTrace::new(state.rejected_request_id());
        return inbound_rejection_response(rejection, &method, &uri, &trace);
    }

    // 活动时间戳：收到请求即更新（stop idle / status 筛选的判定依据）
    state.touch_activity();
    // IPC 观测计数；计数顺带是请求标识的后半段（实例内第 N 个请求，唯一且与 status 的
    // 计数一致）
    let request_seq = state.stats.note_request();
    // 在途数：从这里算起，到响应体交付完（或客户端断开、响应被丢弃）为止
    let in_flight = state.stats.enter();
    let request_id: Arc<str> = state.request_id(request_seq).into();
    // 请求历史里的一条记录，从这里记到响应交付完。头记的是客户端发来的原样（入库时打码），
    // 不是下面覆盖、追加之后的
    let forward = state.config.forward_only_enabled();
    let rec = state.history.begin(
        request_seq,
        request_id.clone(),
        crate::history::Start {
            method: method.as_str(),
            path_and_query: uri.path_and_query().map(|pq| pq.as_str()).unwrap_or("/"),
            headers: req.headers(),
            delivery: if forward { "forward" } else { "retry" },
            phase: if forward {
                Phase::Attempting
            } else {
                Phase::ReadingBody
            },
        },
    );
    // 处理函数被取消（客户端在拿到响应之前断开）时，它随 future 一起被丢弃，照样收尾
    let mut delivery = Delivery::new(rec.clone(), in_flight);
    let mut trace = RequestTrace::recorded(request_id, rec);
    trace.decide_started = decide_started;
    if state.has_format(crate::config::file::Stage::Stream) {
        let join = Arc::new(UpstreamJoin::default());
        trace.upstream_join = Arc::downgrade(&join);
        delivery.join = Some(join);
    }
    let bodiless = method == http::Method::HEAD;

    // 这个请求的全部日志都带 request_id：与响应头 x-aproxy-request-id、错误体里的是同一个串，
    // 拿着其中任何一个都能在日志里找到这个请求的经过
    let span = tracing::info_span!("request", request_id = %trace.request_id);
    let response = handle(state, req, method, uri, trace, &mut delivery)
        .instrument(span)
        .await;
    delivery.attach(response, bodiless)
}

/// 通过了入站校验、编好请求标识之后的处理。
async fn handle(
    state: AppState,
    req: Request,
    method: http::Method,
    uri: http::Uri,
    mut trace: RequestTrace,
    delivery: &mut Delivery,
) -> Response {
    // WebSocket 升级：代理按请求—响应转发，承载不了升级后的双向流。Upgrade 是逐跳头，转发时
    // 会被剥掉，上游看到的是一个普通请求，多半回 4xx，然后被无限重试——不如当场说清楚。
    // 只拦 websocket：别的升级（例如 curl --http2 对明文地址带的 `Upgrade: h2c`）按 HTTP 的
    // 规定可以忽略，照常以 HTTP/1.1 应答。
    if let Some(upgrade) = websocket_upgrade(req.headers()) {
        tracing::warn!(
            code = %"upgrade_unsupported",
            upgrade = %upgrade,
            "Rejected an unsupported connection upgrade; request was not forwarded"
        );
        return Terminal::upgrade_unsupported(&upgrade).into_response(&trace);
    }

    let mut headers = req.headers().clone();

    // 在本地侧先应用覆盖/追加，避免重试间重复计算
    apply_header_overrides(&mut headers, &state.config);

    use crate::config::file::Stage;
    use axum::body::HttpBody as _;
    let forward = state.config.forward_only_enabled();
    let formatted = !state.formats.is_empty();
    let trigger = state.config.keepalive_trigger();
    // 不含 format 的直通请求沿用转发头取历史摘要；程序眼中的客户端元数据则始终来自入站头。
    let source = if forward && !formatted {
        &headers
    } else {
        req.headers()
    };
    let client_accept = original_accept(source, trigger, formatted);
    let client_encoding = request_encoding(source, formatted).map(|v| v.into_owned());
    let target_url = upstream_url(&state.config, &uri);
    let path_and_query = uri.path_and_query().map_or("/", |p| p.as_str());
    let head_stream = match trigger {
        crate::config::KeepaliveTrigger::Accept => Some(client_accept),
        crate::config::KeepaliveTrigger::Any if client_accept => Some(true),
        _ if req.body().is_end_stream() || req.body().size_hint().exact() == Some(0) => Some(false),
        _ => None,
    };
    let ctx = formatted.then(|| {
        ExchangeCtx::new(
            trace.request_id.clone(),
            path_and_query,
            method == http::Method::HEAD,
            head_stream,
            trace.rec.clone(),
        )
    });
    // 只有默认 forward 可以边上传边选路。默认 retry 要在请求层之前知道 body_stream，
    // 才能在慢程序持有该帧时保活；decide 可能要求重发，即使它不读正文也要先留体。
    let live = forward
        && !state.has_format(Stage::Decide)
        && !state.formats.iter().any(|p| {
            p.entry.has_stage(Stage::Request)
                && p.negotiated.lock().unwrap().as_ref().is_none_or(|h| {
                    h.request_body.get("request") != Some(&aproxy_envelope::BodyNeed::None)
                })
        });
    // 允许双工的上传可能比已交付的整个响应更长；即使没有 stream 层，历史也得等上传真正退出。
    if live && delivery.join.is_none() {
        let join = Arc::new(UpstreamJoin::default());
        trace.upstream_join = Arc::downgrade(&join);
        delivery.join = Some(join);
    }
    let upload = ClientUpload {
        body: req.into_body(),
        trigger,
        accept: client_accept,
        encoding: client_encoding.clone(),
    };
    if live && !state.has_format(Stage::Request) {
        delivery.arm_notify(&state, ctx.as_ref());
        tracing::info!(method = %method, target = %state.history.redacted_url(&target_url), "Proxy request (forward only)");
        trace.attempts = 1;
        return forward_only_proxy(
            state,
            &method,
            &target_url,
            &headers,
            upload,
            ctx,
            &trace,
            1,
        )
        .await
        .unwrap_or_else(|t| t.into_response(&trace));
    }
    let mut req = OutboundRequest {
        method,
        target_url,
        headers,
        body: RequestBody::Memory(Bytes::new()),
        replacement: None,
        client_encoding,
        client_accept,
        body_stream: None,
        ctx,
    };
    let (pending, wants_stream) = if live {
        (Some(upload), head_stream.unwrap_or(false))
    } else {
        match upload.buffer(&state, &mut req, &trace.rec).await {
            Ok(stream) => (None, stream),
            Err(t) => return t.into_response(&trace),
        }
    };
    delivery.arm_notify(&state, req.ctx.as_ref());
    let keepalive_applies = !forward
        && state.config.keepalive_enabled()
        && !state.config.keepalive_interval().is_zero()
        && wants_stream;
    if keepalive_applies {
        trace.rec.keepalive();
    }
    let max_spool_bytes = spool_limit_bytes(&state.config);

    // 保活适用：整个尝试循环（从请求转换起）交给后台任务，经保活出口交付——请求转换也
    // 处在心跳节拍内，format 慢（timeout_secs = 0 时没有上界）时客户端照样在一个间隔后
    // 收到骨架头与心跳，而不是在拿到任何字节之前干等。req 所有权移交，请求体随任务结束
    // 自动 Drop 删除磁盘临时文件。
    if keepalive_applies
        || state.has_format(crate::config::file::Stage::Request)
        || state.has_format(crate::config::file::Stage::Retry)
        || state.has_format(crate::config::file::Stage::Decide)
    {
        let (resp_tx, resp_rx) = tokio::sync::oneshot::channel::<Response>();
        let path_and_query = path_and_query.to_string();
        let task_trace = trace.clone();
        tokio::spawn(
            async move {
                let mut r = Responder::keepalive(
                    &state,
                    resp_tx,
                    task_trace.rec.clone(),
                    keepalive_applies,
                );
                run_attempts(
                    &state,
                    req,
                    pending,
                    &path_and_query,
                    max_spool_bytes,
                    &mut r,
                    task_trace,
                )
                .await;
            }
            // 后台任务的日志也挂在这个请求的 span 下，带同一个 request_id
            .instrument(tracing::Span::current()),
        );
        // 客户端在提交前断开时 hyper 会 drop 本 future（连同 resp_rx），后台任务经
        // resp_tx.closed() 感知并中止上游请求。Err 只在后台任务未交出响应就结束时
        // 出现（panic）——给客户端一个明确的 500 而不是让连接悬空
        return resp_rx.await.unwrap_or_else(|_| {
            tracing::error!(code = %"internal", "Keepalive task ended without delivering a response");
            Terminal::internal("the keepalive task ended without a response").into_response(&trace)
        });
    }

    // 不保活：尝试循环就在本 future 里跑，成功后一次性回放。客户端断开时 hyper drop
    // 整个 future，在途的上游请求随之中止
    let mut r = Responder::buffered();
    run_attempts(
        &state,
        req,
        pending,
        path_and_query,
        max_spool_bytes,
        &mut r,
        trace.clone(),
    )
    .await;
    r.into_response(&trace)
}

/// 请求要求把连接升级成 WebSocket 时，返回 `Upgrade` 头的值（展示用，截短）；不是则 None。
/// `Upgrade` 的值是逗号分隔的协议列表，协议名不分大小写，可以带 `/版本`。
fn websocket_upgrade(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(http::header::UPGRADE)?.to_str().ok()?;
    value
        .split(',')
        .any(|protocol| {
            protocol
                .trim()
                .split('/')
                .next()
                .is_some_and(|name| name.eq_ignore_ascii_case("websocket"))
        })
        .then(|| value.chars().take(64).collect())
}
