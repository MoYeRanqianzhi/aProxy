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
    let delivery = Delivery::new(rec.clone(), in_flight);
    let trace = RequestTrace::recorded(request_id, rec);
    let bodiless = method == http::Method::HEAD;

    // 这个请求的全部日志都带 request_id：与响应头 x-aproxy-request-id、错误体里的是同一个串，
    // 拿着其中任何一个都能在日志里找到这个请求的经过
    let span = tracing::info_span!("request", request_id = %trace.request_id);
    let response = handle(state, req, method, uri, trace)
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
    trace: RequestTrace,
) -> Response {
    // WebSocket 升级：代理按请求—响应转发，承载不了升级后的双向流。Upgrade 是逐跳头，转发时
    // 会被剥掉，上游看到的是一个普通请求，多半回 4xx，然后被无限重试——不如当场说清楚。
    // 只拦 websocket：别的升级（例如 curl --http2 对明文地址带的 `Upgrade: h2c`）按 HTTP 的
    // 规定可以忽略，照常以 HTTP/1.1 应答。
    if let Some(upgrade) = websocket_upgrade(req.headers()) {
        tracing::warn!(
            code = %"upgrade_unsupported",
            upgrade = %upgrade,
            "拒绝了连接升级请求（不支持，未转发上游）"
        );
        return Terminal::upgrade_unsupported(&upgrade).into_response(&trace);
    }

    let mut headers = req.headers().clone();

    // 在本地侧先应用覆盖/追加，避免重试间重复计算
    apply_header_overrides(&mut headers, &state.config);

    // 仅转发模式：分支点必须在 read_request_body **之前**——缓冲一旦发生，本模式
    // 就丢掉了全部意义（内存随负载增长、下游要等请求体读完才见到首字节）；
    // 同时它位于 requests_total.fetch_add 与 apply_header_overrides **之后**，
    // 所以 status 的请求计数与鉴权/头改写行为与常规模式完全一致。
    // 这条路径整体不进重试循环、保活心跳、错误内容拦截、spool 与 keepalive_trigger
    // 判定，也不做响应体解码（本模式不对响应内容做任何检查，无需解码）。
    if state.config.forward_only_enabled() {
        let target_url = upstream_url(&state.config, &uri);
        // 日志里的 URL 一律经 redact::url：base_url 可能内嵌 user:pass，客户端
        // 查询串可能带 key（Gemini 式 ?key=），日志常被整段粘贴求助
        tracing::info!(method = %method, target = %aproxy_ipc::redact::url(&target_url), "代理请求（仅转发）");
        return forward_only_proxy(state, method, target_url, headers, req.into_body(), trace)
            .await;
    }

    // 缓冲请求体以支持重试重放；上限 max_body_mb（默认 128 MB，0=不限），
    // 超出直接 413。disk_cache 开启时超过内存驻留阈值（1 MiB）溢写磁盘，
    // 大请求体的进程内存在途占用恒定。
    let body_limit = state.config.body_limit_bytes();
    let history = trace.rec.request_body(body_encoding(
        headers
            .get(http::header::CONTENT_ENCODING)
            .map(|v| v.as_bytes()),
    ));
    let (req_body, model) = match read_request_body(
        req.into_body(),
        body_limit,
        state.spool_dir.as_deref(),
        history,
    )
    .await
    {
        Ok(b) => b,
        Err(ReadBodyError::TooLarge) => return body_too_large_response(body_limit, &trace),
        // 客户端没把请求体发完：不是上游失败，不记「最近错误」
        Err(ReadBodyError::ClientAborted(e)) => {
            tracing::error!(code = %"client_body_aborted", error = %e, "读取请求体失败：客户端在上传途中断开");
            return Terminal::client_body_aborted().into_response(&trace);
        }
        Err(ReadBodyError::Storage(e)) => {
            let terminal = Terminal::local_storage_failed(StorageOp::WriteRequest, &e.to_string());
            terminal.record_in(&state.stats);
            tracing::error!(code = %terminal.code(), error = %e, "请求体写入磁盘缓存失败");
            return terminal.into_response(&trace);
        }
    };

    let path_and_query = uri.path_and_query().map(|pq| pq.as_str()).unwrap_or("/");
    // 是否「保活适用」（走保活通道：先提交响应头、再以 SSE 注释心跳维持连接）：
    // keepalive 开启且 keepalive_trigger 判定命中。必须在请求转换前判定——
    // 判定语义是「客户端视角」，转换器改写 Accept/请求体不影响保活选择
    let keepalive_dur = state.config.keepalive_interval();
    let keepalive_enabled = state.config.keepalive_enabled() && keepalive_dur.as_secs() > 0;
    // 「客户端是不是在要一个流」不管保活开没开都判：请求记录的 `stream` 用的是同一个结论
    let wants_stream =
        keepalive_triggered(state.config.keepalive_trigger(), &headers, &req_body).await;
    let keepalive_applies = keepalive_enabled && wants_stream;
    trace.rec.request_bytes(match &req_body {
        RequestBody::Memory(bytes) => bytes.len() as u64,
        RequestBody::Disk { len, .. } => *len,
    });
    trace.rec.request_shape(model, wants_stream);
    if keepalive_applies {
        trace.rec.keepalive();
    }

    let req = OutboundRequest {
        method,
        target_url: upstream_url(&state.config, &uri),
        headers,
        body: req_body,
        // 交给 format 的信封里是同一个请求标识
        ctx: ExchangeCtx::new(trace.request_id.to_string()),
        retransform: None,
    };
    let max_spool_bytes = spool_limit_bytes(&state.config);

    // 保活适用：整个尝试循环（从请求转换起）交给后台任务，经保活出口交付——请求转换也
    // 处在心跳节拍内，format 慢（timeout_secs = 0 时没有上界）时客户端照样在一个间隔后
    // 收到骨架头与心跳，而不是在拿到任何字节之前干等。req 所有权移交，请求体随任务结束
    // 自动 Drop 删除磁盘临时文件。
    if keepalive_applies {
        let (resp_tx, resp_rx) = tokio::sync::oneshot::channel::<Response>();
        let path_and_query = path_and_query.to_string();
        let task_trace = trace.clone();
        tokio::spawn(
            async move {
                let mut r = Responder::keepalive(&state, resp_tx, task_trace.rec.clone());
                run_attempts(
                    &state,
                    req,
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
            tracing::error!(code = %"internal", "保活通道后台任务未交出响应即结束");
            Terminal::internal("the keepalive task ended without a response").into_response(&trace)
        });
    }

    // 不保活：尝试循环就在本 future 里跑，成功后一次性回放。客户端断开时 hyper drop
    // 整个 future，在途的上游请求随之中止
    let mut r = Responder::buffered();
    run_attempts(
        &state,
        req,
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
