//! 渠道和密钥各自从请求恢复同一个粘性票号，不读对方的 state，也不保存对话。
use serde_json::Value;
use sha2::{Digest, Sha256};

/// 首条用户消息的首段文本是稳定前缀；忽略模型、后续轮次、系统提示与供应商的文本包装。
pub fn conversation_ticket(body: &Value) -> Option<u64> {
    let messages = body.get("messages").or_else(|| body.get("input"))?;
    let content = if messages.is_string() {
        messages
    } else {
        messages
            .as_array()?
            .iter()
            .find(|m| m["role"] == "user")?
            .get("content")?
    };
    let text = content.as_str().or_else(|| {
        content.as_array()?.iter().find_map(|b| {
            matches!(b["type"].as_str(), Some("text" | "input_text"))
                .then(|| b["text"].as_str())
                .flatten()
        })
    })?;
    if text.is_empty() {
        return None;
    }
    let hash = Sha256::digest(&text.as_bytes()[..text.len().min(4096)]);
    Some(u64::from_be_bytes(hash[..8].try_into().unwrap()))
}

/// 秒数和 HTTP 日期都相对于收到响应的时刻解释，过期日期不产生负等待。
pub fn retry_after(value: &str, now: std::time::SystemTime) -> Option<std::time::Duration> {
    let cap = std::time::Duration::from_millis(aproxy_envelope::MAX_DELAY_MS);
    let value = value.trim();
    if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) {
        return Some(std::time::Duration::from_secs(value.parse().unwrap_or(u64::MAX)).min(cap));
    }
    Some(
        httpdate::parse_http_date(value)
            .ok()?
            .duration_since(now)
            .unwrap_or_default()
            .min(cap),
    )
}

#[cfg(test)]
/// 粘性依赖会话之前不变的部分，而不是整个会话每轮都变的摘要。
mod tests {
    use super::*;
    use serde_json::json;
    /// 可解析但超过一天的日期和秒数以及过长的数字串都饱和，其他坏值不构成等待建议。
    #[test]
    fn retry_after_caps_digits_and_dates_at_one_day() {
        let now = std::time::UNIX_EPOCH;
        let cap = Some(std::time::Duration::from_secs(86400));
        for s in [
            "86400",
            "86401",
            &"9".repeat(200),
            "Sat, 03 Jan 1970 00:00:00 GMT",
        ] {
            assert_eq!(retry_after(s, now), cap);
        }
        assert_eq!(retry_after("0", now), Some(std::time::Duration::ZERO));
        assert_eq!(retry_after("9x", now), None);
    }
    /// 三种协议的同一首轮得到同一票，追加助手或用户轮次及模型重命名都不改变它。
    #[test]
    fn stable_user_prefix_is_protocol_independent() {
        let first = conversation_ticket(&json!({"input":"hello"})).unwrap();
        for v in [
            json!({"messages":[{"role":"system","content":"policy"},{"role":"user","content":"hello"},{"role":"assistant","content":"more"}]}),
            json!({"messages":[{"role":"user","content":[{"type":"text","text":"hello"}]}]}),
            json!({"model":"alias","input":[{"role":"user","content":[{"type":"input_text","text":"hello"}]},{"role":"user","content":"new"}]}),
        ] {
            assert_eq!(conversation_ticket(&v), Some(first));
        }
        assert_ne!(
            conversation_ticket(&json!({"input":"different"})),
            Some(first)
        );
        assert_eq!(conversation_ticket(&json!({"messages":[]})), None);
    }
}
