//! 回放：把缓冲好的上游响应保真地交给客户端（含提交后的解码与失败响应透传）。

use super::*;

/// 成功响应的保真回放（响应转换已做完）：按转换后的头与体重算是否流式，
/// 原样回放 status/头/体。format 可能改写 content-type 与 body 形态，所以
/// 流式判定必须在转换之后。
pub(super) async fn replay_transformed(
    attempt: u32,
    status: StatusCode,
    resp_headers: HeaderMap,
    raw_headers: &reqwest::header::HeaderMap,
    body: SpooledBody,
) -> Response {
    let is_streaming = match &body {
        // 内存模式：整体判定（含 body 嗅探，与旧行为一致）
        SpooledBody::Memory(_) => retry::is_streaming_response(&resp_headers, body.memory_bytes()),
        // 磁盘模式：content-type 判定（全量嗅探需读回整个文件，而磁盘回放本就
        // 是 chunked 流式）
        SpooledBody::Disk { .. } => retry::is_streaming_content_type(&resp_headers),
    };
    tracing::info!(attempt, status = %status, is_streaming, "上游成功，回放响应");
    build_replay_response(status, resp_headers, raw_headers, body, is_streaming).await
}

/// 已提交响应的回放体解码：已提交的响应头里没有 content-encoding（骨架头不带；
/// 上游真实头只在未压缩时才提交），而上游无视 `accept-encoding: identity` 仍回了
/// 压缩体——原样写进去客户端必然解不开，只能**完整、严格**解码后回放
/// （`decode::decode_strict`：截断/损坏/未知编码一律失败，绝不回放半截内容）。
///
/// 在阻塞线程池里流式解码：输入是内存 Bytes 或 spool 文件，输出按
/// `SpillWriter` 内存/磁盘双模落地，内存占用与 body 大小无关；解码输出以
/// spool 上限封顶（回放体与缓冲体同一量级约束，也挡住解压炸弹）。原压缩体
/// 随本函数结束 drop（磁盘文件随之删除）。
pub(super) async fn decode_for_committed_replay(
    body: SpooledBody,
    encoding: String,
    spool_dir: Option<PathBuf>,
    max_output: usize,
) -> Result<SpooledBody, String> {
    tokio::task::spawn_blocking(move || {
        let mut out = SpillWriter::Memory {
            buf: Vec::new(),
            dir: spool_dir,
        };
        let limit = max_output as u64;
        match &body {
            SpooledBody::Memory(b) => {
                crate::decode::decode_strict(&encoding, &b[..], &mut out, limit)?
            }
            SpooledBody::Disk { path, .. } => {
                let file = std::fs::File::open(path)
                    .map_err(|e| format!("读取 spool 临时文件失败: {e}"))?;
                crate::decode::decode_strict(&encoding, file, &mut out, limit)?
            }
        };
        out.into_body()
            .map_err(|e| format!("解码结果写入 spool 失败: {e}"))
    })
    .await
    .map_err(|e| format!("解码任务异常结束: {e}"))?
}

/// 失败响应的原样回放（受限重试路径达上限、且尚未向客户端提交任何字节时）：
/// 不经响应转换器（错误响应不进 format——用户已定），按原始头判定流式与否后
/// 保真回放上游 status/头/体。
pub(super) async fn replay_failure_as_is(
    status: StatusCode,
    resp_headers: HeaderMap,
    raw_headers: &reqwest::header::HeaderMap,
    body: SpooledBody,
) -> Response {
    let is_streaming = match &body {
        SpooledBody::Memory(_) => retry::is_streaming_response(raw_headers, body.memory_bytes()),
        SpooledBody::Disk { .. } => retry::is_streaming_content_type(raw_headers),
    };
    build_replay_response(status, resp_headers, raw_headers, body, is_streaming).await
}

pub(super) fn build_response(status: StatusCode, headers: HeaderMap, body: Bytes) -> Response {
    let mut resp = Response::builder().status(status);
    for (name, value) in headers.iter() {
        resp = resp.header(name, value);
    }
    resp.body(Body::from(body)).unwrap().into_response()
}

/// 统一回放入口：内存模式按流式判定选路（旧行为，保真优先）；磁盘模式一律
/// chunked 流式回放（回放即流式读临时文件），上游带 content-length 时回填
/// 保真（hop 过滤剥掉了它，而我们持有精确字节数）。
pub(super) async fn build_replay_response(
    status: StatusCode,
    headers: HeaderMap,
    raw_headers: &reqwest::header::HeaderMap,
    mut body: SpooledBody,
    is_streaming: bool,
) -> Response {
    // 磁盘模式一律 chunked 流式回放（回放即流式读临时文件）；上游带
    // content-length 时回填保真（hop 过滤剥掉了它，我们持有精确字节数）
    if matches!(body, SpooledBody::Disk { .. }) {
        let replay_len = body.len();
        let mut resp = Response::builder().status(status);
        for (name, value) in headers.iter() {
            resp = resp.header(name, value);
        }
        if replay_len > 0 && raw_headers.get(http::header::CONTENT_LENGTH).is_some() {
            resp = resp.header(http::header::CONTENT_LENGTH, replay_len.to_string());
        }
        let stream = body.into_stream().await;
        resp.body(Body::from_stream(stream))
            .unwrap()
            .into_response()
    } else if is_streaming {
        // 内存流式：8 KiB 零拷贝分块 chunked 回放（与旧行为一致）
        let b = body.take_memory();
        build_stream_replay_response(status, headers, b)
    } else {
        // 内存非流式：Body::from 一次性回放（content-length 由 hyper 自动
        // 设置，不走 chunked——保真优先）
        let b = body.take_memory();
        build_response(status, headers, b)
    }
}

/// 流式回放：将已完整 spool 的 body 按块以 chunked 流式产出，字节与上游完全一致。
///
/// 逐块大小 8 KiB，对 SSE 而言任意切分均安全（解析器按行缓冲），且保证
/// `Transfer-Encoding: chunked`，客户端仍以流式增量消费，避免因一次性
/// `Body::from(bytes)` 被某些客户端误判为非流式而产生的潜在 bug。
pub(super) fn build_stream_replay_response(
    status: StatusCode,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let mut resp = Response::builder().status(status);
    for (name, value) in headers.iter() {
        resp = resp.header(name, value);
    }

    if body.is_empty() {
        return resp.body(Body::empty()).unwrap().into_response();
    }

    // 将 Bytes 按 8 KiB 零拷贝切块：slice_ref 是引用计数切片，共享同一底层
    // 分配——峰值内存为响应体一份而非深拷贝的两份。代价是任一未消费的块
    // 都会把整份分配钉到流结束/断开；body 本就因重试保障全量驻留 spool，
    // 此处无额外驻留
    const CHUNK_SIZE: usize = 8 * 1024;
    let chunks: Vec<Bytes> = body.chunks(CHUNK_SIZE).map(|c| body.slice_ref(c)).collect();

    let stream = futures_util::stream::iter(chunks.into_iter().map(Ok::<Bytes, std::io::Error>));
    let body = Body::from_stream(stream);
    resp.body(body).unwrap().into_response()
}
