//! 重试通道：不保活与保活两条尝试循环。

use super::*;

/// 不适用保活的请求的重试通道：从 attempt 2 起无限重试（attempt 1 已在 proxy_handler
/// 完成），响应在成功后一次性返回。受限重试路径（`bounded_retry`）例外：有响应的失败
/// 达到 [`retry::BOUNDED_RETRY_MAX_ATTEMPTS`] 次后透传最后一次失败响应。
pub(super) async fn proxy_without_keepalive(
    state: AppState,
    req: OutboundRequest,
    max_spool_bytes: usize,
    bounded_retry: bool,
) -> Response {
    let OutboundRequest {
        mut method,
        mut target_url,
        mut headers,
        body: mut req_body,
        ctx,
        mut retransform,
    } = req;
    let mut attempt: u32 = 1;
    let max_backoff = state.config.max_retry_backoff_secs;
    loop {
        attempt += 1;
        // 每轮重试开始处刷新活动时间戳：last_activity_secs 的语义是「最近一次
        // 收到请求**或仍在处理**」——长退避（封顶默认 320s）可能超过
        // idle_timeout_secs（默认 1800s 之内更长的自定义值更危险），不刷新的话
        // 无限重试中的实例会被 stop idle / status --idle 误判为闲置并强退，
        // 恰恰杀掉最需要「不中断」保障的实例（M1）。放在重试轮开头而非退避
        // sleep 之后：退避等待期间的实例也视为活跃。
        state.last_activity_secs.store(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            std::sync::atomic::Ordering::Relaxed,
        );
        state
            .stats
            .retries_total
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let delay = retry::delay_for_attempt(attempt - 1, max_backoff);
        if !delay.is_zero() {
            tracing::warn!(attempt, delay_ms = delay.as_millis() as u64, "重试延迟");
            tokio::time::sleep(delay).await;
        } else {
            tracing::warn!(attempt, "立即重试");
        }

        // every_attempt：用客户端的原始请求重新转换出这次要发的请求（失败沿用上一次的）
        if let Some(rt) = retransform.as_mut()
            && let Some(t) = rt.next(&state, &ctx, attempt).await
        {
            method = t.method;
            target_url = t.url;
            headers = t.headers;
            req_body = t.body;
        }

        let result = forward_once(
            &state,
            &method,
            &target_url,
            &headers,
            &req_body,
            max_spool_bytes,
            None,
        )
        .await;
        if let Some(rt) = retransform.as_mut() {
            rt.record(&result);
        }

        match result {
            ForwardResult::NetworkError(e) => {
                state.note_upstream_failure(&format!("网络错误: {e}"));
                tracing::warn!(attempt, error = %e, "上游网络错误，重试");
                continue;
            }
            ForwardResult::TooLarge => {
                state.note_upstream_failure("上游响应体超出 spool 上限");
                tracing::error!(attempt, "上游响应体超出 spool 上限，终止重试");
                return too_large_response();
            }
            ForwardResult::SpoolFailed(e) => {
                state.note_upstream_failure(&format!("本地 spool 故障: {e}"));
                tracing::error!(attempt, error = %e, "本地 spool 故障，终止重试");
                return spool_failed_response(&e);
            }
            ForwardResult::Response {
                status,
                headers: resp_headers,
                raw_headers,
                body,
                disk_scan,
            } => {
                if !needs_retry_response(attempt, &status, &raw_headers, &body, &disk_scan) {
                    // 成功：交给响应转换器后保真回放（下方 bounded 透传分支是错误
                    // 响应，不经 format——用户已定）
                    return replay_success(
                        &state,
                        &target_url,
                        &ctx,
                        attempt,
                        status,
                        resp_headers,
                        &raw_headers,
                        body,
                    )
                    .await;
                }
                // 受限重试路径：有响应的失败达到尝试上限后不再重试，把最后
                // 一次失败响应原样透传——客户端拿到真实 404 自行处理，好过
                // 永远等不到终态（compact 事故）。仅封顶「上游有响应」的失败：
                // 网络错误是真正的瞬时类，仍无限重试，且它没有响应可供回放。
                if bounded_retry && attempt >= retry::BOUNDED_RETRY_MAX_ATTEMPTS {
                    tracing::warn!(
                        attempt,
                        status = %status,
                        "受限重试路径达到尝试上限，透传最后一次上游响应"
                    );
                    state.note_upstream_failure(&format!(
                        "上游返回 {status}（受限重试路径，达上限透传）"
                    ));
                    return replay_failure_as_is(status, resp_headers, &raw_headers, body).await;
                }
                state.note_upstream_failure(&format!("上游返回 {status}（错误内容，重试）"));
                // 进入下一轮时 body Drop：磁盘临时文件删除
            }
        }
    }
}

/// 保活通道：「保活适用」的请求（判定见 `keepalive_triggered`）从首轮起都走
/// 这里。后台任务驱动全部尝试，handler 只等一个 oneshot 交出 `Response`。
///
/// 提交点（`KeepaliveSink`）只由时间与上游响应头推动，「需要重试」本身不触发
/// 提交——重试几次后很快成功的请求，客户端照样拿到上游真实的 status 与头：
/// - **未提交**（每一轮尝试、每一段退避都适用）：
///   - 上游回 2xx + 未压缩 text/event-stream 头，且未配置 response_transform
///     （format 可能改写 status/头，不能先发）→ 立即提交上游真实 status 与响应头
///     （content-length 与 hop-by-hop 已由 forward_once 滤掉）
///   - 上游回 2xx 但非 SSE 的头 → 本次尝试期间暂停骨架提交（详见 `drive`）
///   - 一个 keepalive 间隔到点仍无可提交的结果 → 提交骨架头（200 + SSE）；
///     首字节因此始终 ≤ 一个保活间隔（暂停中的那次尝试除外）
///   - 某次尝试成功 → 保真快速路径（与不适用保活的请求同一出口），不提交
///   - 受限重试路径达上限 → 与不保活路径一致，原样回放最后一次失败响应
/// - **已提交**：此后的尝试与退避都经 `drive` 驱动、按间隔发心跳；完整缓冲并
///   判定无误后，把成功那一次的字节写进同一个响应（response_transform 只有
///   body 转换生效；上游无视 identity 的压缩体先完整解码）。TooLarge /
///   SpoolFailed / 解码失败 / 受限重试路径达上限以终态 SSE error 事件收场——
///   状态行已发出、不可再改，静默结束与「上游成功返回空 body」在客户端视角
///   不可区分。
///
/// 客户端断开在任一阶段（等响应头、缓冲、退避、回放）都立即中止上游请求、
/// 不再发起新请求：前三者经 `drive` 返回 None，回放经 send 失败。
pub(super) async fn proxy_with_keepalive(
    state: AppState,
    req: OutboundRequest,
    path_and_query: String,
    max_spool_bytes: usize,
    bounded_retry: bool,
) -> Response {
    let (resp_tx, resp_rx) = tokio::sync::oneshot::channel::<Response>();
    tokio::spawn(async move {
        let keepalive_dur = state.config.keepalive_interval();
        let max_backoff = state.config.max_retry_backoff_secs;
        let mut sink = KeepaliveSink::Pending {
            resp_tx: Some(resp_tx),
            skeleton_due: false,
            beat: Heartbeat {
                fixed: Bytes::copy_from_slice(state.config.keepalive_heartbeat()),
                dynamic: None,
            },
        };
        // 整个请求共用一个节拍，首个 tick 在请求开始一个间隔之后：它既是「一个
        // 间隔内仍无可提交结果就提交骨架」的计时器，也是提交后的心跳节拍。
        // Delay：被长回放/慢客户端耽搁后不补发积压的 tick
        let mut ticker =
            tokio::time::interval_at(tokio::time::Instant::now() + keepalive_dur, keepalive_dur);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let real_head_allowed = state.response_pool.is_none();

        // 请求转换同样经 drive 驱动：一个间隔内没转完就先提交骨架头、此后照常
        // 心跳。失败时尚未提交 → 与不保活路径一致回 502；已提交 → 终态 error 事件
        let transformed = drive(
            &mut sink,
            &mut ticker,
            apply_request_transform(&state, req),
            None,
            false,
            "请求转换",
        )
        .await;
        let OutboundRequest {
            mut method,
            mut target_url,
            mut headers,
            body: mut req_body,
            ctx,
            mut retransform,
        } = match transformed {
            None => return,
            Some(Ok(req)) => req,
            Some(Err(msg)) => {
                if sink.is_committed() {
                    sink.send(Ok(terminal_error_event("proxy_transform_failed", &msg)))
                        .await;
                } else {
                    sink.respond(request_transform_failed_response(&msg));
                }
                return;
            }
        };
        // 保活通道会往响应体里插 `: keepalive` 注释——只有未压缩的体才能这样
        // 插入（往 gzip/br 流里插明文，客户端解压必坏）。故对保活适用的请求
        // 要求上游以 identity 编码回应：客户端照旧拿到合法响应（未压缩永远是
        // 可接受的编码），代价只是上游到本机这一段多传一些字节。放在请求转换
        // 之后，覆盖 format 可能写入的同名头（override_headers 里配的同名头也被
        // 覆盖，启动时 server.rs 会 warn 一次）。
        headers.insert(
            http::header::ACCEPT_ENCODING,
            HeaderValue::from_static("identity"),
        );
        log_proxied_request(&method, &path_and_query, &target_url);
        if let Some(pool) = &state.heartbeat_pool {
            sink.arm_dynamic_heartbeat(Arc::new(DynamicHeartbeat {
                pool: pool.clone(),
                ctx: ctx.clone(),
                source: HeartbeatSource::new(&method, &target_url, &headers, &req_body),
                seq: Default::default(),
                attempt: Default::default(),
                in_flight: Default::default(),
                warned: Default::default(),
            }));
        }

        let mut attempt: u32 = 0;
        loop {
            attempt += 1;
            sink.note_attempt(attempt);
            if attempt > 1 {
                // 与非保活通道同款：每轮重试刷新活动时间戳（语义 = 仍在处理中），
                // 防止无限重试中的实例被 stop idle 误判闲置强退（M1）
                state.last_activity_secs.store(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0),
                    std::sync::atomic::Ordering::Relaxed,
                );
                state
                    .stats
                    .retries_total
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let delay = retry::delay_for_attempt(attempt - 1, max_backoff);
                if delay.is_zero() {
                    tracing::warn!(attempt, "立即重试（保活通道）");
                } else {
                    tracing::warn!(
                        attempt,
                        delay_ms = delay.as_millis() as u64,
                        "重试延迟（保活通道）"
                    );
                    let backoff = tokio::time::sleep(delay);
                    if drive(&mut sink, &mut ticker, backoff, None, false, "退避等待")
                        .await
                        .is_none()
                    {
                        return;
                    }
                }
            }

            // every_attempt：重试前用客户端的原始请求重新转换（同样经 drive 驱动，转换
            // 期间心跳照常）。失败沿用上一次的请求
            if attempt > 1
                && let Some(rt) = retransform.as_mut()
            {
                match drive(
                    &mut sink,
                    &mut ticker,
                    rt.next(&state, &ctx, attempt),
                    None,
                    false,
                    "请求转换（重试前）",
                )
                .await
                {
                    None => return,
                    Some(Some(t)) => {
                        method = t.method;
                        target_url = t.url;
                        headers = t.headers;
                        // 与首次转换之后同理：保活通道要求上游以 identity 编码回应
                        headers.insert(
                            http::header::ACCEPT_ENCODING,
                            HeaderValue::from_static("identity"),
                        );
                        req_body = t.body;
                    }
                    Some(None) => {}
                }
            }

            // 尚未提交时每次尝试都要响应头通知：可提交的上游真实头先行提交、
            // 「2xx 非 SSE」暂停骨架提交（见 drive）；已提交后不再需要
            let (head_tx, head_rx) = if sink.is_committed() {
                (None, None)
            } else {
                let (tx, rx) = tokio::sync::oneshot::channel();
                (Some(tx), Some(rx))
            };
            let attempt_fut = forward_once(
                &state,
                &method,
                &target_url,
                &headers,
                &req_body,
                max_spool_bytes,
                head_tx,
            );
            let Some(result) = drive(
                &mut sink,
                &mut ticker,
                attempt_fut,
                head_rx,
                real_head_allowed,
                "等待上游",
            )
            .await
            else {
                return;
            };
            if let Some(rt) = retransform.as_mut() {
                rt.record(&result);
            }

            match result {
                ForwardResult::NetworkError(e) => {
                    // 网络错误是真正的瞬时类：受限路径也不封顶，且它没有响应可供回放
                    state.note_upstream_failure(&format!("网络错误: {e}"));
                    tracing::warn!(attempt, error = %e, "上游网络错误，重试（保活通道）");
                }
                ForwardResult::TooLarge => {
                    state.note_upstream_failure("上游响应体超出 spool 上限");
                    tracing::error!(attempt, "上游响应体超出 spool 上限，终止重试（保活通道）");
                    if sink.is_committed() {
                        let err_event = Bytes::from_static(
                            b"event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"proxy_spool_limit\",\"message\":\"upstream response exceeded proxy spool limit\"}}\n\n",
                        );
                        sink.send(Ok(err_event)).await;
                    } else {
                        sink.respond(too_large_response());
                    }
                    return;
                }
                ForwardResult::SpoolFailed(e) => {
                    state.note_upstream_failure(&format!("本地 spool 故障: {e}"));
                    tracing::error!(attempt, error = %e, "本地 spool 故障，终止重试（保活通道）");
                    if sink.is_committed() {
                        let err_event = Bytes::from_static(
                            b"event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"proxy_spool_failed\",\"message\":\"local disk cache write failed\"}}\n\n",
                        );
                        sink.send(Ok(err_event)).await;
                    } else {
                        sink.respond(spool_failed_response(&e));
                    }
                    return;
                }
                ForwardResult::Response {
                    status,
                    headers: resp_headers,
                    raw_headers,
                    body,
                    disk_scan,
                } => {
                    let upstream_sse = is_event_stream(&resp_headers);
                    if !needs_retry_response(attempt, &status, &raw_headers, &body, &disk_scan) {
                        // 响应转换（若配置）放在提交判定之前，并尽量经 drive 驱动：
                        // format 转换期间照常心跳、一个间隔到点仍未提交就先提交骨架头
                        // （timeout_secs = 0 时转换耗时没有上界，不能让客户端在这段
                        // 时间收不到任何字节）。例外是「尚未提交 + 上游回的是 2xx 非
                        // SSE」：骨架是 SSE、心跳是 SSE 注释，此时提交会把非 SSE 的
                        // 响应改坏（与 drive 的「2xx 非 SSE 暂停骨架提交」同一道理），
                        // 只能直接等转换完成。转换失败时透传原样（不发 error 事件——
                        // 那是响应不可用的终态模板；此处响应在手，仅转换失败）
                        let transform = transform_response_if_configured(
                            &state,
                            &target_url,
                            &ctx,
                            resp_headers,
                            body,
                        );
                        let transformed = if !sink.is_committed() && !upstream_sse {
                            Some(transform.await)
                        } else {
                            drive(&mut sink, &mut ticker, transform, None, false, "响应转换").await
                        };
                        let Some((resp_headers, body)) = transformed else {
                            return;
                        };
                        if !sink.is_committed() {
                            // 提交前就成功：保真快速路径（上游 status 与转换后的响应头）
                            let resp = replay_transformed(
                                attempt,
                                status,
                                resp_headers,
                                &raw_headers,
                                body,
                            )
                            .await;
                            sink.respond(resp);
                            return;
                        }
                        // 已提交：状态行与响应头不可再改——format 对 headers 的改写
                        // 无效，仅 body 转换生效
                        if !is_event_stream(&resp_headers) {
                            // 不撤回、照常回放：客户端已在等这个响应，丢掉成功结果
                            // 只会更糟。日志给出可行动的配置建议
                            tracing::warn!(
                                attempt,
                                content_type = %resp_headers
                                    .get(http::header::CONTENT_TYPE)
                                    .and_then(|v| v.to_str().ok())
                                    .unwrap_or("-"),
                                "已以 text/event-stream 提交的保活响应里回放的是非 SSE 的成功响应体（响应头已发出，无法撤回）；\
                                 若该实例的上游会对 stream:true 请求回非 SSE 的流（如 NDJSON）且常需重试，\
                                 建议为该实例设置 keepalive_trigger = \"accept\""
                            );
                        }
                        let mut body = match encoding_to_undo(&resp_headers) {
                            None => body,
                            Some(encoding) => {
                                // 解码大体可能耗时，同样在心跳节拍内进行
                                let decoded = drive(
                                    &mut sink,
                                    &mut ticker,
                                    decode_for_committed_replay(
                                        body,
                                        encoding.clone(),
                                        state.spool_dir.clone(),
                                        max_spool_bytes,
                                    ),
                                    None,
                                    false,
                                    "回放前解码",
                                )
                                .await;
                                let Some(decoded) = decoded else {
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
                                        let err_event = Bytes::from_static(
                                            b"event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"proxy_decode_failed\",\"message\":\"upstream response used a content-encoding the committed stream cannot carry, and decoding it failed\"}}\n\n",
                                        );
                                        sink.send(Ok(err_event)).await;
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
                            if !sink.send(item).await {
                                // 客户端在回放中断开：Drop 链负责删临时文件
                                sink.log_client_gone("回放");
                                return;
                            }
                        }
                        return;
                    }
                    // 受限重试路径：有响应的失败达到尝试上限后不再重试
                    if bounded_retry && attempt >= retry::BOUNDED_RETRY_MAX_ATTEMPTS {
                        tracing::warn!(
                            attempt,
                            status = %status,
                            "受限重试路径达到尝试上限，终止重试（保活通道）"
                        );
                        if !sink.is_committed() {
                            // 尚未向客户端写出任何字节：与不保活路径一致，原样
                            // 回放最后一次失败响应——客户端拿到真实 404 自行处理
                            state.note_upstream_failure(&format!(
                                "上游返回 {status}（受限重试路径，达上限透传）"
                            ));
                            sink.respond(
                                replay_failure_as_is(status, resp_headers, &raw_headers, body)
                                    .await,
                            );
                            return;
                        }
                        // 已提交：状态行已发出，真实 status/headers 无法回放，以终态
                        // error 事件收场——客户端明确感知代理放弃了这条请求，而非永远等
                        state.note_upstream_failure(&format!(
                            "上游返回 {status}（受限重试路径，达上限终止）"
                        ));
                        drop(body);
                        let err_event = Bytes::from(format!(
                            "event: error\ndata: {{\"type\":\"error\",\"error\":{{\"type\":\"upstream_error\",\"message\":\"upstream returned {status}; bounded retry path exhausted after {attempt} attempts\"}}}}\n\n"
                        ));
                        sink.send(Ok(err_event)).await;
                        return;
                    }
                    state.note_upstream_failure(&format!("上游返回 {status}（错误内容，重试）"));
                    // 进入下一轮时 body Drop：磁盘临时文件删除
                }
            }
            // 需要重试：不在这里提交——提交只由 tick（约一个保活间隔）与可提交的
            // 上游响应头推动，下一轮尝试很快成功时客户端仍拿到上游真实头
        }
    });
    // 客户端在提交前断开时 hyper 会 drop 本 future（连同 resp_rx），后台任务经
    // resp_tx.closed() 感知并中止上游请求。Err 只在后台任务未交出响应就结束时
    // 出现（panic）——给客户端一个明确的 502 而不是让连接悬空
    resp_rx.await.unwrap_or_else(|_| {
        tracing::error!("保活通道后台任务未交出响应即结束");
        (StatusCode::BAD_GATEWAY, "aProxy 内部错误：保活通道异常结束").into_response()
    })
}
