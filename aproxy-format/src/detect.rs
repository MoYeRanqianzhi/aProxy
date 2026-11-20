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
    let tools = obj
        .get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten();
    let chat = chat_role
        || tools
            .clone()
            .any(|t| t["type"] == "function" && t["function"]["name"].is_string())
        || messages.iter().any(|m| m["tool_calls"].is_array());
    let anthropic = obj.contains_key("system")
        || tools
            .clone()
            .any(|t| t["name"].is_string() && t["input_schema"].is_object())
        || messages.iter().any(|m| {
            m["content"].as_array().is_some_and(|parts| {
                parts.iter().any(|p| {
                    matches!(
                        p["type"].as_str(),
                        Some("tool_result" | "tool_use" | "thinking" | "redacted_thinking")
                    )
                })
            })
        });
    // 共享的文本消息不够定协议；若两类专有证据同时存在，也不能靠检查顺序猜测。
    match (chat, anthropic, obj.contains_key("max_tokens")) {
        (false, true, true) => Some(WireFormat::AnthropicMessages),
        (true, false, _) | (false, false, false) => Some(WireFormat::OpenAiChat),
        _ => None,
    }
}

/// SSE 流的形态粗判：content-type 判定不可得时按 data: 行形态。
pub fn looks_like_sse(body: &str) -> bool {
    body.lines()
        .any(|l| l.starts_with("data:") || l.starts_with("event:"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 非标准路径只在工具定义或历史消息携带排他证据时决定；互相矛盾的证据仍拒绝。
    #[test]
    fn tool_shapes_disambiguate_custom_routes() {
        let mut body = serde_json::json!({"model":"m","max_tokens":1,"messages":[{"role":"user","content":"hi"}]});
        assert_eq!(detect_request(&body), None);
        body["tools"] =
            serde_json::json!([{"type":"function","function":{"name":"read","parameters":{}}}]);
        assert_eq!(detect_request(&body), Some(WireFormat::OpenAiChat));
        body["messages"][0]["content"] =
            serde_json::json!([{"type":"tool_result","tool_use_id":"one","content":"hi"}]);
        assert_eq!(detect_request(&body), None);
        body.as_object_mut().unwrap().remove("tools");
        assert_eq!(detect_request(&body), Some(WireFormat::AnthropicMessages));
    }
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
