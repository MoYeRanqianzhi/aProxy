//! 按客户端模型选渠道。地址与凭据作为一组改写，不能把旧上游的鉴权透传到另一个 origin。
use crate::config;
use crate::config::Fault;
use aproxy_envelope as w;
use serde::Deserialize;
use std::collections::BTreeMap;

/// 只允许每个模式给出一种匹配方式，正则在读取配置时编译。
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Matcher {
    /// 名称必须完全一致。
    Exact(String),
    /// 适合一组共享前缀的模型。
    Prefix(String),
    /// 自定义模型集合，不在请求热路径编译表达式。
    Regex(#[serde(deserialize_with = "read_regex")] regex::Regex),
}
/// 不把无效表达式（可能误填了凭据）原样写入诊断。
fn read_regex<'de, D: serde::Deserializer<'de>>(d: D) -> Result<regex::Regex, D::Error> {
    regex::Regex::new(&String::deserialize(d)?)
        .map_err(|_| serde::de::Error::custom("模型正则无效"))
}
/// 渠道在同优先级内缺省占一份，最低优先级数字先尝试。
fn unit_weight() -> u64 {
    1
}
/// 路由项只存环境变量名，绝不把密钥放进可见的渠道标识。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Channel {
    /// 配置中的公开标识。
    id: String,
    /// 数字越小越先尝试；同组内不靠配置先后决定所有流量。
    #[serde(default)]
    priority: u32,
    /// 相同优先级下，此渠道作为首次选择的份额。
    #[serde(default = "unit_weight")]
    weight: u64,
    /// 完整的上游 URL，必要时在它的路径之后拼接客户端路径。
    url: String,
    /// 空集合为兜底。
    #[serde(default)]
    models: Vec<Matcher>,
    /// 按客户端原路径与查询串访问配置的根地址。
    #[serde(default)]
    preserve_path: bool,
    /// 只允许这些名称的查询参数跟随跨 origin 请求；列表为空时原查询串不会离开它的来源。
    #[serde(default)]
    carry_query: std::collections::BTreeSet<String>,
    /// 可交给后置 keys，也可在本渠道直接指定凭据。
    credential_env: Option<String>,
    /// 省略时使用 Bearer authorization。
    auth_header: Option<String>,
}
/// 按先别名、后渠道的顺序处理模型，未命中不能悄悄使用其他目的地。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Route {
    /// 原模型名到上游模型名。
    #[serde(default)]
    aliases: BTreeMap<String, String>,
    /// 开启后与 keys 独立计算相同的用户前缀票号，不传递摘要或会话正文。
    #[serde(default)]
    sticky: bool,
    /// 加载后按优先级稳定排序，组内保留配置顺序作为故障转移次序。
    channel: Vec<Channel>,
}
impl Route {
    /// 只在配置了别名时支持可选的缓冲回程：客户端可保留它请求的模型名。
    pub fn stages(&self) -> &'static [&'static str] {
        if self.aliases.is_empty() {
            &["request", "retry"]
        } else {
            &["request", "retry", "response"]
        }
    }
    /// 使用请求阶段记下的别名，两个别名同指一个模型时也不会反查到另一个客户端的名称。
    pub fn response(&self, m: &w::ResponseMessage, r: &mut w::Reply) -> Result<(), Fault> {
        let Some((_, Some(alias), _)) = m
            .context
            .state
            .as_deref()
            .and_then(|v| serde_json::from_str::<(String, Option<String>, u64)>(v).ok())
        else {
            return Ok(());
        };
        let Some(mapped) = self.aliases.get(&alias) else {
            return Ok(());
        };
        let json = m
            .response
            .headers
            .get("content-type")
            .and_then(|v| v.split(';').next())
            .and_then(|v| v.trim().split_once('/'))
            .is_some_and(|(family, subtype)| {
                family.eq_ignore_ascii_case("application")
                    && (subtype.eq_ignore_ascii_case("json")
                        || subtype.rsplit_once('+').is_some_and(|(name, suffix)| {
                            !name.is_empty() && suffix.eq_ignore_ascii_case("json")
                        }))
            });
        if !json {
            return Ok(());
        }
        let Ok(mut body) = serde_json::from_slice::<serde_json::Value>(
            m.response.body.as_bytes().unwrap_or_default(),
        ) else {
            return Ok(());
        };
        if body["model"].as_str() == Some(mapped) {
            body["model"] = alias.into();
            let mut p = w::ResponsePatch::new();
            p.body = w::Body::Text(body.to_string());
            r.response = Some(p);
        }
        Ok(())
    }
    /// 在握手之前发现重复标识、非法 URL 和无效头名。
    pub fn load(path: &str) -> Result<Self, &'static str> {
        let mut cfg: Self = config::load(path)?;
        let mut ids = std::collections::HashSet::new();
        if cfg.channel.is_empty() {
            return Err("至少需要一个渠道");
        }
        // 未匹配别名时编码 null；预先取最长的别名，不必逐项枚举渠道与别名的笛卡尔积。
        let alias_bytes = cfg
            .aliases
            .keys()
            .map(|k| config::state_string_bytes(k))
            .max()
            .unwrap_or(0)
            .max(4);
        let mut total = 0u64;
        for c in &cfg.channel {
            total = total
                .checked_add(c.weight)
                .filter(|_| c.weight != 0)
                .ok_or("渠道权重须为正数且总和不能溢出")?;
            if config::state_string_bytes(&c.id) + alias_bytes + 24 > config::STATE_BYTES {
                return Err("渠道标识与别名生成的状态超过 64 KiB");
            }
            config::endpoint(&c.url)?;
            if c.id.is_empty() || !ids.insert(&c.id) {
                return Err("渠道标识为空或重复");
            }
            if let Some(h) = &c.auth_header {
                config::credential_header(h)?;
            }
        }
        cfg.channel.sort_by_key(|c| c.priority);
        Ok(cfg)
    }
    /// 每组先按权重确定起点，再遍历其余匹配项。重试沿同一票号继续，避免两个坏高优先级渠道相互切换。
    fn choose(&self, model: &str, ticket: u64, previous: Option<&str>) -> Option<&Channel> {
        let mut eligible: Vec<_> = self
            .channel
            .iter()
            .filter(|c| {
                c.models.is_empty()
                    || c.models.iter().any(|p| match p {
                        Matcher::Exact(s) => s == model,
                        Matcher::Prefix(s) => model.starts_with(s),
                        Matcher::Regex(re) => re.is_match(model),
                    })
            })
            .collect();
        if eligible.is_empty() {
            return None;
        }
        for group in eligible.chunk_by_mut(|a, b| a.priority == b.priority) {
            let mut slot = ticket % group.iter().map(|c| c.weight).sum::<u64>();
            let first = group
                .iter()
                .position(|c| {
                    if slot < c.weight {
                        true
                    } else {
                        slot -= c.weight;
                        false
                    }
                })
                .expect("非空组的正权重总和已验证");
            group.rotate_left(first);
        }
        let i = previous
            .and_then(|id| eligible.iter().position(|c| c.id == id))
            .map_or(0, |i| (i + 1) % eligible.len());
        Some(eligible[i])
    }
    /// 用当前请求体匹配渠道；只返回必要字段，公开报告不带 URL 或凭据。
    pub fn request(
        &self,
        m: &w::RequestMessage,
        retry: Option<&w::Retry>,
        r: &mut w::Reply,
    ) -> Result<(), Fault> {
        let mut body: serde_json::Value =
            serde_json::from_slice(m.request.body.as_bytes().unwrap_or_default())
                .map_err(|_| Fault::new("请求体不是 JSON", "request body is not valid JSON"))?;
        let original = body["model"]
            .as_str()
            .ok_or(Fault::new("请求体缺少 model", "request body has no model"))?;
        let model = self
            .aliases
            .get(original)
            .map(String::as_str)
            .unwrap_or(original);
        let prior = retry
            .and_then(|_| m.context.state.as_deref())
            .and_then(|v| serde_json::from_str::<(String, Option<String>, u64)>(v).ok());
        let ticket = prior
            .as_ref()
            .map(|p| p.2)
            .or_else(|| {
                self.sticky
                    .then(|| crate::selection::conversation_ticket(&body))
                    .flatten()
            })
            .unwrap_or_else(rand::random);
        let channel = self
            .choose(model, ticket, prior.as_ref().map(|p| p.0.as_str()))
            .ok_or(Fault::new(
                "模型未命中任何渠道",
                "no channel matches the model",
            ))?;
        let mut url =
            config::endpoint(&channel.url).map_err(|e| Fault::new(e, "invalid channel URL"))?;
        let incoming =
            config::endpoint(&m.request.url).map_err(|e| Fault::new(e, "invalid upstream URL"))?;
        if channel.preserve_path {
            url.set_path(&format!(
                "{}{}",
                url.path().trim_end_matches('/'),
                m.client
                    .path
                    .split_once('?')
                    .map_or(m.client.path.as_str(), |(path, _)| path)
            ));
            if url.origin() == incoming.origin() {
                url.set_query(incoming.query());
            }
        }
        if url.origin() != incoming.origin() && !channel.carry_query.is_empty() {
            // 显式目标 URL 的参数不接受覆盖；原查询中同名的多个允许值则按顺序一起传递。
            let fixed: std::collections::BTreeSet<_> =
                url.query_pairs().map(|(k, _)| k.into_owned()).collect();
            for (k, v) in incoming.query_pairs() {
                if channel.carry_query.contains(k.as_ref()) && !fixed.contains(k.as_ref()) {
                    url.query_pairs_mut().append_pair(&k, &v);
                }
            }
        }
        let mut headers = m.request.headers.clone();
        config::strip_credentials(&mut headers);
        for c in &self.channel {
            if let Some(name) = &c.auth_header {
                headers.remove(&name.to_ascii_lowercase());
            }
        }
        if let Some(env) = &channel.credential_env {
            let header = channel
                .auth_header
                .as_deref()
                .unwrap_or("authorization")
                .to_ascii_lowercase();
            let value = config::credential(
                env,
                if header == "authorization" {
                    "Bearer"
                } else {
                    ""
                },
            )?;
            headers.insert(header, value);
        }
        let mut patch = w::RequestPatch::new();
        patch.url = Some(url.into());
        patch.headers = Some(headers);
        r.state = Some(
            serde_json::json!([channel.id, (original != model).then_some(original), ticket])
                .to_string(),
        );
        r.report = config::report(
            "Selected an upstream channel",
            w::StandardAction::Route {
                channel: channel.id.clone(),
                model_from: (original != model).then(|| original.to_owned()),
                model_to: (original != model).then(|| model.to_owned()),
            },
        );
        if original != model {
            body["model"] = model.to_owned().into();
            patch.body = w::Body::Text(body.to_string());
        }
        r.request = Some(patch);
        Ok(())
    }
}

#[cfg(test)]
/// 注入固定票号验证匹配、份额和故障转移，而非靠随机请求数估计配置对错。
mod tests {
    use super::*;
    /// 低优先级虽写在配置首行仍然只在前组全部失败后入选；三种 matcher 分别控制资格。
    #[test]
    fn priorities_and_weights_only_apply_to_matching_models() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("route.toml");
        std::fs::write(&file,"[[channel]]\nid='last'\nurl='https://last.test'\npriority=8\nmodels=[{regex='^m[0-9]$'}]\n[[channel]]\nid='a'\nurl='https://a.test'\nweight=2\nmodels=[{exact='m1'}]\n[[channel]]\nid='b'\nurl='https://b.test'\nmodels=[{prefix='m'}]").unwrap();
        let route = Route::load(file.to_str().unwrap()).unwrap();
        for (model, ticket, prev, id) in [
            ("m1", 0, None, "a"),
            ("m1", 1, None, "a"),
            ("m1", 2, None, "b"),
            ("m1", 2, Some("b"), "a"),
            ("m1", 2, Some("a"), "last"),
            ("m1", 2, Some("last"), "b"),
            ("m2", 0, None, "b"),
            ("m2", 0, Some("b"), "last"),
            ("model", 0, Some("b"), "b"),
        ] {
            assert_eq!(route.choose(model, ticket, prev).unwrap().id, id);
        }
        assert!(route.choose("unknown", 0, None).is_none());
    }
}
