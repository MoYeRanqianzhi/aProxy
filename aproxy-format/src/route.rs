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
/// 路由项只存环境变量名，绝不把密钥放进可见的渠道标识。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Channel {
    /// 配置中的公开标识。
    id: String,
    /// 完整的上游 URL，必要时在它的路径之后拼接客户端路径。
    url: String,
    /// 空集合为兜底。
    #[serde(default)]
    models: Vec<Matcher>,
    /// 按客户端原路径与查询串访问配置的根地址。
    #[serde(default)]
    preserve_path: bool,
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
    /// 顺序用于区分特定匹配和最后的兜底。
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
        let Some((_, Some(alias))) = m
            .context
            .state
            .as_deref()
            .and_then(|v| serde_json::from_str::<(String, Option<String>)>(v).ok())
        else {
            return Ok(());
        };
        let Some(mapped) = self.aliases.get(&alias) else {
            return Ok(());
        };
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
        let cfg: Self = config::load(path)?;
        let mut ids = std::collections::HashSet::new();
        if cfg.channel.is_empty() {
            return Err("至少需要一个渠道");
        }
        for c in &cfg.channel {
            config::endpoint(&c.url)?;
            if c.id.is_empty() || !ids.insert(&c.id) {
                return Err("渠道标识为空或重复");
            }
            if let Some(h) = &c.auth_header {
                config::credential_header(h)?;
            }
        }
        Ok(cfg)
    }
    /// 用当前请求体匹配渠道；只返回必要字段，公开报告不带 URL 或凭据。
    pub fn request(&self, m: &w::RequestMessage, r: &mut w::Reply) -> Result<(), Fault> {
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
        let channel = self
            .channel
            .iter()
            .find(|c| {
                c.models.is_empty()
                    || c.models.iter().any(|p| match p {
                        Matcher::Exact(s) => s == model,
                        Matcher::Prefix(s) => model.starts_with(s),
                        Matcher::Regex(re) => re.is_match(model),
                    })
            })
            .ok_or(Fault::new(
                "模型未命中任何渠道",
                "no channel matches the model",
            ))?;
        let mut url =
            config::endpoint(&channel.url).map_err(|e| Fault::new(e, "invalid channel URL"))?;
        if channel.preserve_path {
            let incoming = config::endpoint(&m.request.url)
                .map_err(|e| Fault::new(e, "invalid upstream URL"))?;
            url.set_path(&format!(
                "{}{}",
                url.path().trim_end_matches('/'),
                incoming.path()
            ));
            url.set_query(incoming.query());
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
            serde_json::json!([channel.id, (original != model).then_some(original)]).to_string(),
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
