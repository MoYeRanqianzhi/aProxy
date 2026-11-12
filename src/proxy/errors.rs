//! aproxy 自己产生的终态响应与流终态事件。

use super::*;

/// 已提交的保活响应以终态 SSE error 事件收场（状态行已发出、不可再改）。
/// message 经 JSON 序列化，任意文案（含引号、换行、中文）都不会破坏事件帧。
pub(super) fn terminal_error_event(error_type: &str, message: &str) -> Bytes {
    let data = serde_json::json!({
        "type": "error",
        "error": { "type": error_type, "message": message },
    });
    Bytes::from(format!("event: error\ndata: {data}\n\n"))
}

/// 上游响应体超出 spool 上限的 502 终态（尚未向客户端提交任何字节时）
pub(super) fn too_large_response() -> Response {
    (
        StatusCode::BAD_GATEWAY,
        "上游响应体超出 spool 上限，无法回放（重试无意义）",
    )
        .into_response()
}

/// 本地磁盘 spool 故障的 502 终态（尚未向客户端提交任何字节时）
pub(super) fn spool_failed_response(e: &str) -> Response {
    (
        StatusCode::BAD_GATEWAY,
        format!("本地磁盘缓存写入失败，无法回放: {e}"),
    )
        .into_response()
}
