//! 重试策略：无限重试，梯度延迟。
//!
//! - 第 1-3 次重试：0 延迟（立即重试，应对瞬时抖动）
//! - 第 4 次起：5s, 10s, 20s, 40s, 80s, 160s, 320s，之后固定 320s（约 5 分钟）
//! - attempt 从 1 开始计数（第 1 次重试对应 attempt=1）

/// 根据重试次数计算延迟时长。
///
/// `attempt` 为 1-based：第一次重试为 1，第二次为 2……
/// 返回 `std::time::Duration`。
pub fn delay_for_attempt(attempt: u32) -> std::time::Duration {
    if attempt <= 3 {
        std::time::Duration::ZERO
    } else {
        // attempt 4 -> 5s, 5 -> 10s, 6 -> 20s, ...
        let exp = attempt - 4;
        let secs = 5u64.saturating_mul(1u64 << exp.min(6));
        let capped = secs.min(320);
        std::time::Duration::from_secs(capped)
    }
}

/// 判断 HTTP 状态码是否应重试。
///
/// 原型阶段：对所有错误状态码无限重试（4xx / 5xx / 429），因为 4xx 在
/// 实际运行中亦为易发的瞬时错误（如限流、临时鉴权波动、上游误报），与
/// “拦截一切报错内容并重试”的目标一致。只有 2xx / 3xx 视为成功。
pub fn is_retryable_status(status: u16) -> bool {
    (400..=599).contains(&status)
}

/// 判断响应体是否表示“错误内容”而非正常业务响应。
///
/// 启发式：若响应体为 JSON 且包含明确的错误字段，则视为可重试的错误。
/// 该检查用于“自动拦截一切内容为报错的内容并重试”的需求：
/// 即使 HTTP 状态码为 200，若 body 本身是错误 JSON，也应重试。
///
/// 规则：
/// - body 必须能解析为 JSON 对象
/// - 且包含 `error` 键（值为对象或字符串）或 `type == "error"`，则判定为错误内容
pub fn is_error_body(body: &[u8]) -> bool {
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(body) else {
        return false;
    };
    let Some(obj) = v.as_object() else {
        return false;
    };
    if obj.contains_key("error") {
        return true;
    }
    if obj.get("type").and_then(|t| t.as_str()) == Some("error") {
        return true;
    }
    false
}

/// 判断流式缓冲体中是否包含错误事件。
///
/// 流式场景下，即使 HTTP 状态为 200，流内的某个 SSE/NDJSON 数据块也可能
/// 携带错误（如 `data: {"type":"error",...}`）。若直接管道透传，客户端
/// 已收到部分数据便无法重试；因此需要在缓冲完成后扫描整个流内容，命中
/// 错误即视为可重试。
///
/// 检测策略（启发式，按优先级）：
/// 1. 扫描 SSE `data:` 行：对每一行 `data: <json>` 尝试 `is_error_body`
/// 2. 若未发现 SSE 结构，则按 NDJSON 逐行尝试 `is_error_body`
/// 3. 兜底：对整体 body 尝试 `is_error_body`
pub fn is_stream_error_body(body: &[u8]) -> bool {
    // SSE 路径：只要出现过 `data:` 就认为是 SSE 流，扫描其 data 载荷
    if let Ok(text) = std::str::from_utf8(body) {
        let mut saw_data_line = false;
        let mut sse_has_error = false;
        for line in text.lines() {
            let trimmed = line.trim();
            // SSE 允许 `data:` 后紧跟空格或直接跟 JSON
            let Some(rest) = trimmed
                .strip_prefix("data:")
                .or_else(|| trimmed.strip_prefix("data :"))
            else {
                continue;
            };
            let data = rest.trim();
            if data.is_empty() || data == "[DONE]" {
                continue;
            }
            saw_data_line = true;
            if is_error_body(data.as_bytes()) {
                sse_has_error = true;
                break;
            }
        }
        if saw_data_line {
            return sse_has_error;
        }

        // NDJSON 路径：逐行 JSON 检测
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            // 仅对看起来像 JSON 的行做检测，避免对普通 text 误判
            if line.starts_with('{') && is_error_body(line.as_bytes()) {
                return true;
            }
        }
    }

    // 兜底：整体作为 JSON 检测
    is_error_body(body)
}

/// 判断上游响应是否为流式。
///
/// 依据 1：`Content-Type` 包含 `text/event-stream` / `application/x-ndjson`
/// 依据 2：body 文本中出现 SSE 标志 `data:`（兼容上游未正确设置 Content-Type 的情况）
pub fn is_streaming_response(headers: &reqwest::header::HeaderMap, body: &[u8]) -> bool {
    if let Some(ct) = headers
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
    {
        let lower = ct.to_ascii_lowercase();
        if lower.contains("text/event-stream") || lower.contains("application/x-ndjson") {
            return true;
        }
    }
    // 嗅探 body：包含 SSE data 行则视为流式
    if let Ok(text) = std::str::from_utf8(body) {
        for line in text.lines() {
            let t = line.trim();
            if t.starts_with("data:") || t.starts_with("data :") {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delay_first_three_zero() {
        assert_eq!(delay_for_attempt(1), std::time::Duration::ZERO);
        assert_eq!(delay_for_attempt(2), std::time::Duration::ZERO);
        assert_eq!(delay_for_attempt(3), std::time::Duration::ZERO);
    }

    #[test]
    fn delay_exponential_then_cap() {
        assert_eq!(delay_for_attempt(4), std::time::Duration::from_secs(5));
        assert_eq!(delay_for_attempt(5), std::time::Duration::from_secs(10));
        assert_eq!(delay_for_attempt(6), std::time::Duration::from_secs(20));
        assert_eq!(delay_for_attempt(7), std::time::Duration::from_secs(40));
        assert_eq!(delay_for_attempt(8), std::time::Duration::from_secs(80));
        assert_eq!(delay_for_attempt(9), std::time::Duration::from_secs(160));
        assert_eq!(delay_for_attempt(10), std::time::Duration::from_secs(320));
        assert_eq!(delay_for_attempt(11), std::time::Duration::from_secs(320));
        assert_eq!(delay_for_attempt(100), std::time::Duration::from_secs(320));
    }

    #[test]
    fn retryable_status() {
        // 原型：所有错误状态码都重试（4xx / 5xx）
        assert!(is_retryable_status(400));
        assert!(is_retryable_status(401));
        assert!(is_retryable_status(403));
        assert!(is_retryable_status(404));
        assert!(is_retryable_status(429));
        assert!(is_retryable_status(500));
        assert!(is_retryable_status(502));
        assert!(is_retryable_status(503));
        assert!(!is_retryable_status(200));
        assert!(!is_retryable_status(201));
        assert!(!is_retryable_status(301));
    }

    #[test]
    fn error_body_detection() {
        assert!(is_error_body(br#"{"error": "overloaded"}"#));
        assert!(is_error_body(br#"{"error": {"message": "x"}}"#));
        assert!(is_error_body(br#"{"type": "error", "message": "x"}"#));
        assert!(!is_error_body(br#"{"content": "hello"}"#));
        assert!(!is_error_body(br#"not json"#));
        assert!(!is_error_body(br#"[]"#));
    }

    #[test]
    fn stream_error_body_sse() {
        let sse = b"data: {\"type\":\"content_block_delta\",\"text\":\"hi\"}\n\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded\"}}\n\n";
        assert!(is_stream_error_body(sse));
    }

    #[test]
    fn stream_error_body_sse_ok() {
        let sse = b"data: {\"type\":\"content_block_delta\",\"text\":\"hi\"}\n\ndata: {\"type\":\"message_delta\",\"stop_reason\":\"end_turn\"}\n\ndata: [DONE]\n\n";
        assert!(!is_stream_error_body(sse));
    }

    #[test]
    fn stream_error_body_ndjson() {
        let ndjson = b"{\"type\":\"content\"}\n{\"type\":\"error\",\"error\":\"x\"}\n";
        assert!(is_stream_error_body(ndjson));
    }

    #[test]
    fn stream_error_body_non_json() {
        assert!(!is_stream_error_body(b"hello world"));
    }

    #[test]
    fn streaming_response_by_content_type() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            "text/event-stream".parse().unwrap(),
        );
        assert!(is_streaming_response(&headers, b""));
    }

    #[test]
    fn streaming_response_by_body_sniff() {
        let headers = reqwest::header::HeaderMap::new();
        let body = b"data: {\"hello\":1}\n\n";
        assert!(is_streaming_response(&headers, body));
    }

    #[test]
    fn streaming_response_negative() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            "application/json".parse().unwrap(),
        );
        assert!(!is_streaming_response(&headers, br#"{"ok":true}"#));
    }
}
