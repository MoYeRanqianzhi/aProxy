//! 协议检测启发式：switchyard 不提供 detect API（0.3.0 已核实），auto 模式
//! 的客户端协议由本模块按**请求** JSON 形态嗅探。启发式为本仓库自研，用单测
//! 钉死语义。
//!
//! 响应方向从请求阶段保存的私有状态取客户端协议，不拿上游响应形态反推客户端。

use serde_json::Value;
use switchyard_translation::WireFormat;

/// 请求协议检测：按三格式 wire 形态的特征字段判别，均不命中返回 None
/// （不猜——调用方输出 error 行）。
pub fn detect_request(body: &Value) -> Option<WireFormat> {
    let obj = body.as_object()?;
    // Responses 的入口与 messages 互斥；混用两种形态不能用检查顺序决定回程。
    if obj.contains_key("input") || obj.contains_key("instructions") {
        return (!obj.contains_key("messages")).then_some(WireFormat::OpenAiResponses);
    }
    let messages = obj.get("messages")?.as_array()?;
    obj.get("model")?.as_str()?;
    // 顶层 system 是 Messages 的标志；Chat 的 system/developer/tool 在消息列表中。
    let chat_role = messages.iter().any(|m| {
        matches!(
            m["role"].as_str(),
            Some("system" | "developer" | "tool" | "function")
        )
    });
    if obj.contains_key("system") {
        return (obj.contains_key("max_tokens") && !chat_role)
            .then_some(WireFormat::AnthropicMessages);
    }
    // Messages 要求 max_tokens，Chat 也允许相同字段；只有共有字段时请调用方显式给协议。
    (!obj.contains_key("max_tokens") || chat_role).then_some(WireFormat::OpenAiChat)
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
