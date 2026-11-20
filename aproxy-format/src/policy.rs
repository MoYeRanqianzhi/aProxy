//! 尝试判定只使用本帧的脱离请求正文的事实；没有规则命中时沿用代理的保底决定。
use crate::config;
use aproxy_envelope as w;
use serde::Deserialize;
use switchyard_translation::WireFormat as F;

/// 匹配按声明顺序进行，单条规则内的条件为且。
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Policy {
    /// 非标准客户端路径可显式给出错误响应的协议。
    protocol: Option<F>,
    /// 空规则表不改变任何决定，也不订阅成功响应。
    rule: Vec<Rule>,
}
/// 模板只引用固定诊断，绝不回显上游响应体或凭据。
#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum Template {
    /// 请求超过模型上下文大小。
    ContextTooLong,
    /// 路由到用户未配置的模型。
    ModelNotConfigured,
}
/// 判定的谓词与动作在加载时校验，避免无意给出无效的复合表态。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Rule {
    /// 仅用于报告的公开标识；缺省时报告一基序号。
    id: Option<String>,
    /// 状态列表内是或，不给列表时不要求已收到头。
    #[serde(default)]
    status: Vec<u16>,
    /// 稳定的失败代码，不能用错误消息冒充分类。
    failure: Option<String>,
    /// 只对确认可见的响应字节找正匹配。
    #[serde(default, deserialize_with = "pattern")]
    body_regex: Option<regex::bytes::Regex>,
    /// 有效规则显式给 retry 或 stop。
    decision: w::Decision,
    /// 只有 retry 才能切换渠道。
    reroute: Option<bool>,
    /// 固定等待可选，省略让代理继续其保底退避。
    delay_ms: Option<w::RetryDelay>,
    /// 上游等待提示可以延长既定的等待，不缩短它。
    #[serde(default)]
    respect_retry_after: bool,
    /// 仅 stop 可替换失败响应。
    response: Option<Template>,
}
/// 一次编译、复用匹配器；无效正则不引用原模式。
fn pattern<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<regex::bytes::Regex>, D::Error> {
    regex::bytes::Regex::new(&String::deserialize(d)?)
        .map(Some)
        .map_err(|_| serde::de::Error::custom("错误体正则无效"))
}
impl Rule {
    /// 明确选择成功状态或纯正文检测时才需要看到成功响应。
    fn success(&self) -> bool {
        self.failure.is_none()
            && (self.status.iter().any(|s| *s < 400)
                || self.status.is_empty() && self.body_regex.is_some())
    }
    /// 截断预览的前后片段并不相邻；不能把空洞两端拼出一个并不存在的命中。
    fn matches(&self, m: &w::DecideMessage) -> bool {
        if m.outcome == "success" && !self.success() {
            return false;
        }
        if self
            .failure
            .as_ref()
            .is_some_and(|f| !m.failure.as_ref().is_some_and(|v| v.code == *f))
        {
            return false;
        }
        if !self.status.is_empty()
            && !m
                .response
                .as_ref()
                .is_some_and(|v| self.status.contains(&v.response.status))
        {
            return false;
        }
        let Some(re) = &self.body_regex else {
            return true;
        };
        let Some(r) = &m.response else {
            return false;
        };
        let Some(b) = r.response.body.as_bytes() else {
            return false;
        };
        match r.truncated {
            None => false,
            Some(None) => re.is_match(b),
            Some(Some(_)) => {
                let (head, tail) = b.split_at(b.len().min(64 * 1024));
                re.is_match(head) || re.is_match(tail)
            }
        }
    }
}
impl Template {
    /// 按最初的客户端协议生成完整的新响应；未知路径仍给两族都能读取的嵌套 error。
    fn response(self, protocol: Option<F>) -> w::ResponsePatch {
        let (status, code, ty, openai, msg) = match self {
            Self::ContextTooLong => (
                400,
                "context_length_exceeded",
                "invalid_request_error",
                "invalid_request_error",
                "The request exceeds the model context limit.",
            ),
            Self::ModelNotConfigured => (
                404,
                "model_not_found",
                "not_found_error",
                "invalid_request_error",
                "The requested model is not configured.",
            ),
        };
        let body = if matches!(protocol, Some(F::OpenAiChat | F::OpenAiResponses)) {
            serde_json::json!({"error":{"type":openai,"code":code,"param":null,"message":msg}})
        } else {
            serde_json::json!({"type":"error","error":{"type":ty,"message":msg}})
        };
        let mut p = w::ResponsePatch::new();
        p.status = Some(status);
        p.headers = Some([("content-type".into(), "application/json".into())].into());
        p.body = w::Body::Text(body.to_string());
        p
    }
}
impl Policy {
    /// 无配置的 policy 只弃权；配置错误在 hello 前报告。
    pub fn load(path: Option<&str>) -> Result<Self, &'static str> {
        let p: Self = match path {
            Some(p) => config::load(p)?,
            None => Self::default(),
        };
        let mut ids = std::collections::HashSet::new();
        for rule in &p.rule {
            if rule.status.iter().any(|s| !(100..=599).contains(s))
                || rule.failure.as_ref().is_some_and(String::is_empty)
                || rule
                    .id
                    .as_ref()
                    .is_some_and(|s| s.is_empty() || !ids.insert(s))
            {
                return Err("判定规则的状态码、错误类或标识无效");
            }
            if rule.decision == w::Decision::Retry && rule.response.is_some()
                || rule.decision == w::Decision::Stop
                    && (rule.reroute.is_some()
                        || rule.delay_ms.is_some()
                        || rule.respect_retry_after)
            {
                return Err("判定规则的动作与决定不一致");
            }
        }
        Ok(p)
    }
    /// 仅为实际会用到的 2xx / 3xx 观察协商成功特性。
    pub fn needs_success(&self) -> bool {
        self.rule.iter().any(Rule::success)
    }
    /// 首个匹配的整条规则给决定；保底的延迟、路由各自独立。
    pub fn decide(&self, m: &w::DecideMessage, r: &mut w::Reply) {
        let Some((i, rule)) = self
            .rule
            .iter()
            .enumerate()
            .find(|(_, rule)| rule.matches(m))
        else {
            return;
        };
        r.decision = Some(rule.decision.clone());
        r.reroute = rule.reroute;
        r.delay_ms = rule.delay_ms;
        if rule.respect_retry_after
            && let Some(d) = m
                .response
                .as_ref()
                .and_then(|v| v.response.headers.get("retry-after"))
                .and_then(|v| crate::selection::retry_after(v, std::time::SystemTime::now()))
        {
            let ms = d.as_millis().min(u128::from(w::MAX_DELAY_MS)) as u64;
            let base = rule
                .delay_ms
                .map(w::RetryDelay::millis)
                .or(m.fallback.delay_ms)
                .unwrap_or_default();
            r.delay_ms = w::RetryDelay::new(ms.max(base.min(w::MAX_DELAY_MS))).ok();
        }
        if let Some(template) = rule.response {
            let proto =
                self.protocol
                    .or_else(|| match m.client.as_ref()?.path.split('?').next()? {
                        "/v1/messages" => Some(F::AnthropicMessages),
                        "/v1/chat/completions" => Some(F::OpenAiChat),
                        "/v1/responses" => Some(F::OpenAiResponses),
                        _ => None,
                    });
            r.response = Some(template.response(proto));
        }
        let id = rule.id.clone().unwrap_or_else(|| (i + 1).to_string());
        let mut report = w::Report::new("Matched a decision rule".into());
        report.actions.push(w::Action::custom(format!(
            "rule {id}: {}",
            if rule.decision == w::Decision::Retry {
                "retry"
            } else {
                "stop"
            }
        )));
        r.report = w::ReportField::Present(report);
    }
}

#[cfg(test)]
/// 逐个验证判定的条件、缺省表态及不同客户端的终态实体。
mod tests {
    use super::*;
    use serde_json::json;
    /// 生成带完整预览的观测；模拟的 URL 刻意不是客户端路径。
    fn message(status: u16, body: &str) -> w::DecideMessage {
        serde_json::from_value(json!({"request_id":"r","attempt":1,"elapsed_ms":10,"delivery":"retry","outcome":if status<400{"success"}else{"failure"},"failure":if status<400{serde_json::Value::Null}else{json!({"code":"upstream_status"})},"request":{"method":"POST","url":"https://up.test/other"},"response":{"status":status,"headers":{"retry-after":"8"},"body":body,"truncated":null},"fallback":{"decision":"retry","by":"default","delay_ms":2500}})).unwrap()
    }
    /// 各条件取交集，命中首项就结束；重试提示不能缩短退避，且受协议上限约束。
    #[test]
    fn ordered_rules_match_failure_status_body_and_wait() {
        let p:Policy=toml::from_str("[[rule]]\nid='limited'\nstatus=[429]\nfailure='upstream_status'\nbody_regex='busy'\ndecision='retry'\nrespect_retry_after=true\nreroute=true\n[[rule]]\nfailure='upstream_connect_failed'\ndecision='retry'\ndelay_ms=0\nreroute=false").unwrap();
        assert!(!p.needs_success());
        let mut m = message(429, "busy");
        let mut reply = w::Reply::new(1);
        p.decide(&m, &mut reply);
        assert_eq!(reply.reroute, Some(true));
        assert_eq!(reply.delay_ms.unwrap().millis(), 8000);
        for (header, expected) in [
            ("0", Some(2500)),
            ("99999999", Some(w::MAX_DELAY_MS)),
            ("invalid", None),
        ] {
            m.response
                .as_mut()
                .unwrap()
                .response
                .headers
                .insert("retry-after".into(), header.into());
            let mut r = w::Reply::new(2);
            p.decide(&m, &mut r);
            assert_eq!(r.delay_ms.map(w::RetryDelay::millis), expected);
        }
        let mut wrong = message(503, "busy");
        let mut r = w::Reply::new(1);
        p.decide(&wrong, &mut r);
        assert!(r.decision.is_none());
        wrong.failure = Some(w::Failure::new("upstream_connect_failed".into()));
        wrong.response = None;
        p.decide(&wrong, &mut r);
        assert_eq!(r.reroute, Some(false));
        assert_eq!(r.delay_ms.unwrap().millis(), 0);
        let mut r = w::Reply::new(1);
        p.decide(&message(429, "different"), &mut r);
        assert!(r.decision.is_none());
    }
    /// 预览中间被省略时，拼接处横跨的单词不是可用证据；两边独立命中仍有效。
    #[test]
    fn success_preview_requires_positive_evidence() {
        let p: Policy =
            toml::from_str("[[rule]]\nstatus=[200]\nbody_regex='BROKEN'\ndecision='retry'")
                .unwrap();
        assert!(p.needs_success());
        let mut m = message(200, &format!("{}BROKEN", "x".repeat(64 * 1024 - 3)));
        let mut r = w::Reply::new(1);
        p.decide(&m, &mut r);
        assert_eq!(r.decision, Some(w::Decision::Retry));
        let obs = m.response.as_mut().unwrap();
        obs.truncated = Some(Some(400));
        let mut r = w::Reply::new(1);
        p.decide(&m, &mut r);
        assert!(r.decision.is_none());
        for body in [
            format!("{}BROKENtail", "x".repeat(64 * 1024 - 6)),
            format!("{}BROKEN", "x".repeat(64 * 1024)),
            format!("{}xBROKEN", "x".repeat(64 * 1024)),
        ] {
            m.response.as_mut().unwrap().response.body = w::Body::Text(body);
            let mut r = w::Reply::new(1);
            p.decide(&m, &mut r);
            assert_eq!(r.decision, Some(w::Decision::Retry));
        }
    }
    /// 每种错误模板为客户端生成独立错误而非复用上游错误页，响应只带自己的类型头。
    #[test]
    fn stop_templates_follow_the_client_not_the_target() {
        for (name, status) in [("context_too_long", 400), ("model_not_configured", 404)] {
            let p: Policy =
                toml::from_str(&format!("[[rule]]\ndecision='stop'\nresponse='{name}'")).unwrap();
            for path in ["/v1/messages?q=1", "/v1/chat/completions", "/v1/responses"] {
                let mut m = message(429, "not for the client");
                m.client = Some(w::Client::new(path.into(), None));
                let mut r = w::Reply::new(1);
                p.decide(&m, &mut r);
                let out = r.response.unwrap();
                assert_eq!(out.status, Some(status));
                assert_eq!(out.headers.unwrap().len(), 1);
                let b: serde_json::Value =
                    serde_json::from_slice(out.body.as_bytes().unwrap()).unwrap();
                assert_eq!(b["type"] == "error", path.starts_with("/v1/messages"));
                assert!(b["error"]["message"].is_string());
                assert!(!b.to_string().contains("not for the client"));
            }
        }
    }
}
