//! 发往上游：单次尝试（forward_once）、上游地址拼接、错误脱敏与请求日志。

use super::*;

/// 「代理请求」日志：请求转换之后、首轮之前各通道各记一次。URL 一律经
/// redact::url：base_url 可能内嵌 user:pass，查询串可能带 key（Gemini 式
/// ?key=），日志常被整段粘贴求助
pub(super) fn log_proxied_request(method: &http::Method, path_and_query: &str, target_url: &str) {
    tracing::info!(
        method = %method,
        path = %aproxy_ipc::redact::url(path_and_query),
        target = %aproxy_ipc::redact::url(target_url),
        "代理请求"
    );
}

// Response 变体比其他变体大（status + 双头表 + SpooledBody）：body 本身已是
// 引用计数 Bytes 或文件句柄，装箱只能省这点栈空间，而本枚举只在单一路径按值
// 传递一次，不构成热点。保持扁平匹配更直接。
#[allow(clippy::large_enum_variant)]
pub(super) enum ForwardResult {
    /// 没拿到完整的上游响应：连不上、超时、连接中途断开。原因已归成尝试失败码
    NetworkError(AttemptFailure),
    /// 上游响应体超出 spool 上限：确定性失败，重试无意义，各通道直接终态处理。
    TooLarge,
    /// 本地磁盘 spool 写入失败（磁盘满/IO 故障）：磁盘模式专属的确定性失败。
    /// 与 NetworkError 不同——同一轮重试无法恢复（文件句柄已坏），但下一轮
    /// 可能恢复；交由调用方选择终态或继续。带着出问题的是哪一步与原因。
    SpoolFailed(StorageOp, String),
    /// 完整缓冲后的响应；`raw_headers` 保留原始 reqwest 头用于流式嗅探，
    /// `headers` 为已过滤 hop-by-hop 后的待透传头。`disk_scan` 仅为磁盘模式
    /// 增量扫描结论（Some((是否错误, 头部快照)))；内存模式恒 None。
    Response {
        status: StatusCode,
        headers: HeaderMap,
        raw_headers: reqwest::header::HeaderMap,
        body: SpooledBody,
        disk_scan: Option<(bool, Vec<u8>)>,
    },
}

/// 响应体 spool 上限（字节）：配置 spool_limit_mb（MB）换算，防止失控/恶意上游把内存或
/// 磁盘打爆。超过上限的响应无法完整缓冲，直接按 TooLarge 终态处理。0 表示不设限
/// （与请求体上限的 0 同义）。
pub(super) fn spool_limit_bytes(config: &Config) -> usize {
    match config.spool_limit_mb {
        0 => usize::MAX,
        mb => (mb as usize).saturating_mul(1024 * 1024),
    }
}

/// 一次上游尝试：发出请求、完整缓冲响应（内存/磁盘 spool）并做增量错误扫描。
///
/// `on_head`：拿到上游响应头（status + 已滤 hop-by-hop 的头）时立即通知调用方，
/// 早于缓冲响应体——保活通道据此在首轮把上游真实头先行提交给客户端。接收方已
/// 不关心（已提交、已结束）时发送失败，忽略即可。
///
/// `rec` 与 `attempt`：请求历史里这个请求的记录，以及这是第几次尝试。响应头一到就记下，
/// 响应体边收边交一份——中途断开的那次尝试，已经收到的部分也留得下来。
#[allow(clippy::too_many_arguments)]
pub(super) async fn forward_once(
    state: &AppState,
    method: &http::Method,
    url: &str,
    headers: &HeaderMap,
    req_body: &RequestBody,
    max_spool_bytes: usize,
    on_head: Option<tokio::sync::oneshot::Sender<(StatusCode, HeaderMap)>>,
    rec: &crate::history::Recorder,
    attempt: u32,
) -> ForwardResult {
    let spool_dir = state.spool_dir.as_deref();
    let reqwest_method =
        reqwest::Method::from_bytes(method.as_str().as_bytes()).unwrap_or(reqwest::Method::GET);

    let mut builder = state.client.request(reqwest_method, url);

    // 透传请求头（过滤 hop-by-hop），已在 proxy_handler 中应用了覆盖/追加
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

    // 请求体字节源：内存零拷贝 clone / 磁盘流式重读。磁盘模式显式回填
    // content-length（hop 过滤剥掉了原头，而我们持有精确字节数，保真回放）。
    builder = match req_body.apply_to(builder).await {
        Ok(b) => b,
        Err(e) => {
            // 请求体临时文件打不开 = 本地确定性故障，重试同样打不开
            return ForwardResult::SpoolFailed(StorageOp::ReadRequest, e.to_string());
        }
    };

    let resp = match send_with_timeout(&state.config, builder.send()).await {
        Ok(r) => r,
        // 在源头归类并脱敏：失败原因会流向日志、status 的最近错误与各通道
        Err(failure) => return ForwardResult::NetworkError(failure),
    };

    let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let raw_headers = resp.headers().clone();

    // 过滤后的响应头（透传用）
    let mut resp_headers = HeaderMap::new();
    for (name, value) in resp.headers().iter() {
        let name_str = name.as_str();
        if is_hop_header(name_str) {
            continue;
        }
        if let Ok(n) = HeaderName::from_bytes(name_str.as_bytes())
            && let Ok(v) = HeaderValue::from_bytes(value.as_bytes())
        {
            // append 而非 insert：Set-Cookie 等同名多值头不能坍缩为最后一个
            resp_headers.append(n, v);
        }
    }
    if let Some(tx) = on_head {
        let _ = tx.send((status, resp_headers.clone()));
    }
    // 请求历史里记原始的响应头（逐跳头也在内：这是上游实际回的）。写者随本函数返回而结束
    let mut history = rec.response_head(
        attempt,
        status.as_u16(),
        raw_headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_bytes())),
        body_encoding(
            raw_headers
                .get(reqwest::header::CONTENT_ENCODING)
                .map(|v| v.as_bytes()),
        ),
    );

    // 关键：完整 spool（分块累积）——任何流式中断都会在此处以 Err 形式暴露，从而触发重试；
    // 超出上限按 TooLarge 终态处理；磁盘写入失败按 SpoolFailed 处理
    let mut spool = SpoolBuffer::Memory(Vec::new());
    let mut scanner = StreamErrorScanner::new();
    let mut resp_stream = resp;
    loop {
        match next_chunk(&mut resp_stream, &state.config, spool.len()).await {
            Ok(Some(chunk)) => {
                // 各提前返回路径上 spool 随之 drop，半截磁盘文件由 SpoolPath 删除
                if spool.len() + chunk.len() as u64 > max_spool_bytes as u64 {
                    return ForwardResult::TooLarge;
                }
                if let Some(history) = history.as_mut() {
                    history.feed_copy(&chunk);
                }
                scanner.feed(&chunk);
                spool_chunk(&mut spool, &chunk, spool_dir).await;
                if matches!(spool, SpoolBuffer::Poisoned) {
                    return ForwardResult::SpoolFailed(
                        StorageOp::WriteResponse,
                        "writing the response body to the disk cache failed".to_string(),
                    );
                }
            }
            Ok(None) => break,
            Err(failure) => return ForwardResult::NetworkError(failure),
        }
    }

    // 收尾 flush 失败与收集途中写失败同属本地磁盘故障，走同一条 SpoolFailed 路径
    let (body, disk_scan) = match spool.finish(scanner).await {
        Ok(v) => v,
        Err(e) => return ForwardResult::SpoolFailed(StorageOp::WriteResponse, e),
    };
    ForwardResult::Response {
        status,
        headers: resp_headers,
        raw_headers,
        body,
        disk_scan,
    }
}

/// 发出请求并等响应头，最多等 `timeouts.first_byte_secs`（0 = 不限）。时限从发出请求起算，
/// 包含连接建立与请求体上传；到点时这次尝试以 `upstream_first_byte_timeout` 失败，在途的
/// 请求随 future 被丢弃而中止。
pub(super) async fn send_with_timeout(
    config: &Config,
    send: impl Future<Output = Result<reqwest::Response, reqwest::Error>>,
) -> Result<reqwest::Response, AttemptFailure> {
    let limit = Duration::from_secs(config.first_byte_timeout_secs);
    if limit.is_zero() {
        return send.await.map_err(|e| send_failure(e, config));
    }
    match tokio::time::timeout(limit, send).await {
        Ok(result) => result.map_err(|e| send_failure(e, config)),
        Err(_) => Err(AttemptFailure::first_byte_timeout(limit)),
    }
}

/// 读响应体的下一块，最多等 `timeouts.read_idle_secs`（0 = 不限）：每读到一块就重新计时，
/// 所以它限制的是两次读到数据之间的间隔，不是整个响应的时长。到点时以
/// `upstream_read_idle_timeout` 失败。`received_bytes` 是此前已经收到的响应体字节数。
pub(super) async fn next_chunk(
    resp: &mut reqwest::Response,
    config: &Config,
    received_bytes: u64,
) -> Result<Option<Bytes>, AttemptFailure> {
    let limit = Duration::from_secs(config.read_timeout_secs);
    let read = async {
        resp.chunk()
            .await
            .map_err(|e| body_failure(e, received_bytes, config))
    };
    if limit.is_zero() {
        return read.await;
    }
    match tokio::time::timeout(limit, read).await {
        Ok(result) => result,
        Err(_) => Err(AttemptFailure::read_idle_timeout(limit, received_bytes)),
    }
}

/// 把「发出请求、等响应头」阶段的 reqwest 错误归成尝试失败码：
/// - 连接阶段超时 → `upstream_connect_timeout`；
/// - 连接阶段的其他失败（DNS、被拒绝、TLS）→ `upstream_connect_failed`；
/// - 连接已建立、等响应头超时 → `upstream_first_byte_timeout`；
/// - 其余（连接建立之后、响应头之前断开等）→ `upstream_body_interrupted`，已收 0 字节。
pub(super) fn send_failure(e: reqwest::Error, config: &Config) -> AttemptFailure {
    let (connect, timeout) = (e.is_connect(), e.is_timeout());
    let reason = redact_reqwest_error(e);
    match (connect, timeout) {
        (true, true) => {
            AttemptFailure::connect_timeout(Duration::from_secs(config.connect_timeout_secs))
        }
        (true, false) => AttemptFailure::connect_failed(reason),
        (false, true) => {
            AttemptFailure::first_byte_timeout(Duration::from_secs(config.first_byte_timeout_secs))
        }
        (false, false) => AttemptFailure::body_interrupted(0, reason),
    }
}

/// 把「读响应体」阶段的 reqwest 错误归成尝试失败码：超时 → `upstream_read_idle_timeout`，
/// 其余 → `upstream_body_interrupted`。`received_bytes` 是出错之前已经收到的响应体字节数。
pub(super) fn body_failure(
    e: reqwest::Error,
    received_bytes: u64,
    config: &Config,
) -> AttemptFailure {
    if e.is_timeout() {
        AttemptFailure::read_idle_timeout(
            Duration::from_secs(config.read_timeout_secs),
            received_bytes,
        )
    } else {
        AttemptFailure::body_interrupted(received_bytes, redact_reqwest_error(e))
    }
}

/// reqwest 错误的脱敏渲染：它的 Display 会把请求 URL 原样拼在末尾
/// （`error sending request for url (URL)`），URL 的查询串里可能带客户端的 key
/// （`?key=...`），而这段文本会进日志、status 的最近错误和 502 正文。先剥掉 URL
/// 再按原有形态附上脱敏版本——保留「for url (...)」便于排障时看出打到了哪里。
pub(super) fn redact_reqwest_error(e: reqwest::Error) -> String {
    let url = e.url().map(|u| aproxy_ipc::redact::url(u.as_str()));
    let e = e.without_url();
    match url {
        Some(u) => format!("{e} for url ({u})"),
        None => e.to_string(),
    }
}

/// 上游目标 URL：base_url（归一时已去尾斜杠，此处再 trim 一次以容忍直接构造的
/// Config）+ 原样的 path/query。两条转发路径共用，避免拼接规则各写一遍而漂移。
pub(super) fn upstream_url(config: &Config, uri: &http::Uri) -> String {
    let path_and_query = uri.path_and_query().map(|pq| pq.as_str()).unwrap_or("/");
    format!(
        "{}{}",
        config.base_url.trim_end_matches('/'),
        path_and_query
    )
}
