//! switchyard-translation 接线：请求/响应协议转换。
//!
//! switchyard 是 sans-I/O 的纯转换库（无 HTTP 客户端依赖）；本模块把信封的
//! body 字节喂进 decode/encode，收回转换产物。转换方向全部由
//! 请求阶段确定的协议对指定，响应侧反向使用同一对协议。

use serde_json::Value;
use switchyard_translation::WireFormat;

/// 请求转换：`from` 格式的请求体 → `to` 格式。格式相同则原样返回。
/// 错误统一为人类可读文案（sync helpers 的 TranslationError 直接 Display）。
pub fn translate_request(from: WireFormat, to: WireFormat, body: &Value) -> Result<Value, String> {
    if from == to {
        return Ok(body.clone());
    }
    let req = switchyard_translation::decode_request(from, body).map_err(|e| e.to_string())?;
    let mut out = switchyard_translation::encode_request(&req, to).map_err(|e| e.to_string())?;
    // Chat 上游只在明确请求时给流的最终用量，其他协议的客户端不需要也无法替它设置这个开关。
    if to == WireFormat::OpenAiChat && body["stream"] == true {
        out["stream_options"]["include_usage"] = true.into();
    }
    Ok(out)
}

/// 非流式响应转换：`from` 格式的缓冲响应 → `to` 格式。格式相同则原样返回。
pub fn translate_response(from: WireFormat, to: WireFormat, body: &Value) -> Result<Value, String> {
    if from == to {
        return Ok(body.clone());
    }
    let agg = switchyard_translation::decode_aggregated_response(body, from)
        .map_err(|e| e.to_string())?;
    switchyard_translation::encode_aggregated_response(&agg, to, None).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use switchyard_translation::WireFormat;

    /// OpenAiResponses 此前在所有测试层零执行——真实转换往返钉死
    #[test]
    fn responses_request_translates_to_anthropic_and_back() {
        let req: Value = serde_json::from_str(
            r#"{"model":"gpt-5o","input":"tell me a joke","instructions":"be funny","max_output_tokens":64}"#,
        )
        .unwrap();
        // 请求方向：openai_responses → anthropic_messages（真实 switchyard 解码/编码）
        let translated = translate_request(
            WireFormat::OpenAiResponses,
            WireFormat::AnthropicMessages,
            &req,
        )
        .expect("responses→anthropic 请求转换失败");
        assert!(
            translated.get("messages").is_some(),
            "anthropic 产物应有 messages: {translated}"
        );
        // 响应方向：anthropic 聚合响应 → openai_responses（反向真实转换）
        let resp: Value = serde_json::from_str(
            r#"{"role":"assistant","content":[{"type":"text","text":"ha"}],"model":"gpt-5o","stop_reason":"end_turn"}"#,
        )
        .unwrap();
        let back = translate_response(
            WireFormat::AnthropicMessages,
            WireFormat::OpenAiResponses,
            &resp,
        )
        .expect("anthropic→responses 响应转换失败");
        assert!(
            back.get("output").is_some() || back.get("output_text").is_some(),
            "responses 产物应有 output 形态: {back}"
        );
    }

    #[test]
    fn same_format_translation_is_identity() {
        let body: Value = serde_json::from_str(r#"{"input":"x","model":"m"}"#).unwrap();
        let out = translate_request(
            WireFormat::OpenAiResponses,
            WireFormat::OpenAiResponses,
            &body,
        )
        .unwrap();
        assert_eq!(out, body, "同格式转换恒等");
    }
}
