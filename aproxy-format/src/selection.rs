//! 渠道和密钥各自从请求恢复同一个粘性票号，不读对方的 state，也不保存对话。
use serde_json::Value;
use sha2::{Digest, Sha256};

/// 首条用户消息的首段文本是稳定前缀；忽略模型、后续轮次、系统提示与供应商的文本包装。
pub fn conversation_ticket(body: &Value) -> Option<u64> {
    let messages = body.get("messages").or_else(|| body.get("input"))?;
    let usable = |v: &Value| v.as_str().is_some_and(|s| !s.is_empty());
    let text = messages.as_str().filter(|s| !s.is_empty()).or_else(|| {
        messages
            .as_array()?
            .iter()
            .filter(|m| m["role"] == "user")
            .find_map(|m| {
                let c = &m["content"];
                c.as_str().filter(|s| !s.is_empty()).or_else(|| {
                    c.as_array()?
                        .iter()
                        .find(|part| {
                            matches!(part["type"].as_str(), Some("text" | "input_text"))
                                && usable(&part["text"])
                        })
                        .and_then(|p| p["text"].as_str())
                })
            })
    })?;
    let hash = Sha256::digest(&text.as_bytes()[..text.floor_char_boundary(4096)]);
    Some(u64::from_be_bytes(hash[..8].try_into().unwrap()))
}

/// 每个成员独立抽样指数到达时间，最小者胜。成员的加入、冷却与配置排序不改变其余成员的分数。
/// 域分隔避免同一会话总把同序号的渠道与密钥配成对；不对总权重取模或把大权重截到随机票范围。
pub fn preferred<'a, T: 'a>(
    domain: &[u8],
    ticket: u64,
    members: impl Iterator<Item = &'a T>,
    identity: impl Fn(&T) -> (&str, u64),
) -> Option<&'a T> {
    use sha2::{Digest, Sha256};
    members
        .map(|member| {
            let (id, weight) = identity(member);
            let bits = Sha256::new()
                .chain_update(domain)
                .chain_update([0])
                .chain_update(ticket.to_be_bytes())
                .chain_update(id.as_bytes())
                .finalize();
            // 52 位有效票落在开区间 (0,1)，ln 的两个端点均不会引入无穷或零分数。
            let uniform = ((u64::from_be_bytes(bits[..8].try_into().unwrap()) >> 12) + 1) as f64
                / ((1u64 << 52) + 1) as f64;
            (-uniform.ln() / weight as f64, id, member)
        })
        .min_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(b.1)))
        .map(|(_, _, member)| member)
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
    /// 两种角色的抽样独立，极大权重按比例分配，成员变化只会改变原胜者或新胜者的会话。
    #[test]
    fn rendezvous_is_independent_stable_and_weighted_at_large_scales() {
        let candidates = [
            ("a", u64::MAX / 3),
            ("b", u64::MAX / 3 * 2),
            ("c", u64::MAX / 2),
        ];
        let pick = |domain: &[u8], ticket: u64, ids: &[usize]| {
            preferred(
                domain,
                ticket,
                ids.iter().map(|&i| &candidates[i]),
                |&(id, w)| (id, w),
            )
            .unwrap()
            .0
        };
        let mut both = 0;
        let mut key_a = 0;
        let mut route_a = 0;
        for ticket in 0..4096 {
            let k = pick(b"keys", ticket, &[0, 1]);
            let r = pick(b"route", ticket, &[0, 1]);
            key_a += usize::from(k == "a");
            route_a += usize::from(r == "a");
            both += usize::from(k == "a" && r == "a");
            for (domain, old) in [(b"keys".as_slice(), k), (b"route".as_slice(), r)] {
                let expanded = pick(domain, ticket, &[0, 1, 2]);
                assert!(expanded == old || expanded == "c");
                assert_eq!(pick(domain, ticket, &[1, 0]), old);
                let cooled = pick(domain, ticket, &[0, 2]);
                assert!(expanded == "b" || expanded == cooled);
            }
        }
        for count in [key_a, route_a] {
            assert!((1250..1490).contains(&count), "{count}");
        }
        assert!((375..535).contains(&both), "{both}");
    }
    /// 三种协议的同一首轮得到同一票，追加助手或用户轮次及模型重命名都不改变它。
    #[test]
    fn stable_user_prefix_is_protocol_independent() {
        let first = conversation_ticket(&json!({"input":"hello"})).unwrap();
        for v in [
            json!({"messages":[{"role":"user","content":[{"type":"image"}]},{"role":"user","content":[{"type":"text","text":""},{"type":"text","text":"hello"}]}]}),
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
        let prefix = "x".repeat(4095);
        assert_eq!(
            conversation_ticket(&json!({"input":format!("{prefix}界")})),
            conversation_ticket(&json!({"input":prefix}))
        );
    }
}
