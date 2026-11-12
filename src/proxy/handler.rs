//! 入口：路由与请求处理的分派（入站校验 → 计数 → 仅转发分叉 → 读请求体 → 选通道）。

use super::*;

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
    if let Some(rejection) = state.inbound.check(req.headers(), &uri) {
        return inbound_rejection_response(rejection, &method, &uri);
    }

    let mut headers = req.headers().clone();

    // 活动时间戳：收到请求即更新（stop idle / status 筛选的判定依据）
    state.touch_activity();
    // IPC 观测计数（一条 Relaxed fetch_add，与上面 store 同量级）；计数顺带
    // 充当转换信封的 request_id（实例内第 N 个请求，唯一且与 status 的计数一致）
    let request_seq = state
        .stats
        .requests_total
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        + 1;

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
        // 日志里的 URL 一律经 mask_base_url：base_url 可能内嵌 user:pass，客户端
        // 查询串可能带 key（Gemini 式 ?key=），日志常被整段粘贴求助
        tracing::info!(method = %method, target = %crate::config::mask_base_url(&target_url), "代理请求（仅转发）");
        return forward_only_proxy(state, method, target_url, headers, req.into_body()).await;
    }

    // 缓冲请求体以支持重试重放；上限 max_body_mb（默认 128 MB，0=不限），
    // 超出直接 413。disk_cache 开启时超过内存驻留阈值（1 MiB）溢写磁盘，
    // 大请求体的进程内存在途占用恒定。
    let body_limit = state.config.body_limit_bytes();
    let req_body =
        match read_request_body(req.into_body(), body_limit, state.spool_dir.as_deref()).await {
            Ok(b) => b,
            Err(ReadBodyError::TooLarge) => return body_too_large_response(body_limit),
            Err(ReadBodyError::Io(e)) => {
                tracing::error!(error = %e, "读取请求体失败");
                return (
                    StatusCode::BAD_REQUEST,
                    format!("请求体读取失败（连接中断或本地磁盘缓存写入失败）: {e}"),
                )
                    .into_response();
            }
        };

    let path_and_query = uri.path_and_query().map(|pq| pq.as_str()).unwrap_or("/");
    let bounded_retry = state.bounded_retry_matches(path_and_query);
    // 是否「保活适用」（走保活通道：先提交响应头、再以 SSE 注释心跳维持连接）：
    // keepalive 开启且 keepalive_trigger 判定命中。必须在请求转换前判定——
    // 判定语义是「客户端视角」，转换器改写 Accept/请求体不影响保活选择
    let keepalive_dur = state.config.keepalive_interval();
    let keepalive_enabled = state.config.keepalive_enabled() && keepalive_dur.as_secs() > 0;
    let keepalive_applies = keepalive_enabled
        && keepalive_triggered(state.config.keepalive_trigger(), &headers, &req_body).await;

    let req = OutboundRequest {
        method,
        target_url: upstream_url(&state.config, &uri),
        headers,
        body: req_body,
        ctx: ExchangeCtx::new(request_seq.to_string()),
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
        tokio::spawn(async move {
            let mut r = Responder::keepalive(&state, resp_tx);
            run_attempts(
                &state,
                req,
                &path_and_query,
                max_spool_bytes,
                bounded_retry,
                &mut r,
            )
            .await;
        });
        // 客户端在提交前断开时 hyper 会 drop 本 future（连同 resp_rx），后台任务经
        // resp_tx.closed() 感知并中止上游请求。Err 只在后台任务未交出响应就结束时
        // 出现（panic）——给客户端一个明确的 502 而不是让连接悬空
        return resp_rx.await.unwrap_or_else(|_| {
            tracing::error!("保活通道后台任务未交出响应即结束");
            (StatusCode::BAD_GATEWAY, "aProxy 内部错误：保活通道异常结束").into_response()
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
        bounded_retry,
        &mut r,
    )
    .await;
    r.into_response()
}
