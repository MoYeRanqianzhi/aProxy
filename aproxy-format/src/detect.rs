//! 协议检测启发式：switchyard 不提供 detect API（0.3.0 已核实），auto 模式
//! 的客户端协议由本模块按**请求** JSON 形态嗅探。启发式为本仓库自研，用单测
//! 钉死语义。
//!
//! 没有响应侧检测：上游按渠道协议回复，检测响应体得到的永远是渠道协议，
//! 推断不出客户端协议——auto 的响应侧直接取渠道协议，正确性由请求侧「auto
//! 只放行同协议」保证（见 main.rs 的 handle_request）。

use serde_json::Value;
use switchyard_translation::WireFormat;

/// 请求协议检测：按三格式 wire 形态的特征字段判别，均不命中返回 None
/// （不猜——调用方输出 error 行）。
pub fn detect_request(body: &Value) -> Option<WireFormat> {
    let obj = body.as_object()?;
    // Anthropic Messages：system + messages + max_tokens 三特征
    if obj.contains_key("system") && obj.contains_key("messages") && obj.contains_key("max_tokens")
    {
        return Some(WireFormat::AnthropicMessages);
    }
    // OpenAI Responses：input / instructions 是其独有入口字段
    if obj.contains_key("input") || obj.contains_key("instructions") {
        return Some(WireFormat::OpenAiResponses);
    }
    // OpenAI Chat：messages + model（且未被上两者命中）
    if obj.contains_key("messages") {
        return Some(WireFormat::OpenAiChat);
    }
    None
}

/// SSE 流的形态粗判：content-type 判定不可得时按 data: 行形态。
pub fn looks_like_sse(body: &str) -> bool {
    body.lines()
        .any(|l| l.starts_with("data:") || l.starts_with("event:"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_anthropic_messages_request() {
        let body: Value = serde_json::from_str(
            r#"{"system":"s","messages":[{"role":"user","content":"hi"}],"max_tokens":100,"model":"m"}"#,
        )
        .unwrap();
        assert_eq!(detect_request(&body), Some(WireFormat::AnthropicMessages));
    }

    #[test]
    fn detects_openai_responses_request() {
        let body: Value = serde_json::from_str(r#"{"input":"hi","model":"gpt"}"#).unwrap();
        assert_eq!(detect_request(&body), Some(WireFormat::OpenAiResponses));
    }

    #[test]
    fn detects_openai_chat_request() {
        let body: Value =
            serde_json::from_str(r#"{"messages":[{"role":"user","content":"hi"}],"model":"gpt"}"#)
                .unwrap();
        assert_eq!(detect_request(&body), Some(WireFormat::OpenAiChat));
    }

    #[test]
    fn unknown_request_shape_returns_none() {
        let body: Value = serde_json::from_str(r#"{"foo":1}"#).unwrap();
        assert_eq!(detect_request(&body), None);
        assert_eq!(detect_request(&Value::Null), None);
        assert_eq!(detect_request(&Value::Array(vec![])), None);
    }

    #[test]
    fn sse_shape_detection() {
        assert!(looks_like_sse("data: {\"a\":1}\n\nevent: done\n\n"));
        assert!(looks_like_sse("event: message_start\n\n"));
        assert!(!looks_like_sse("{\"ok\":1}"));
        assert!(!looks_like_sse(""));
    }
}
