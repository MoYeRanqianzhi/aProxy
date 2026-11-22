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
/// TOML 使用带点的公开叶键，协议回信则按组组织；两种表示只在这里转换。
type Leaves = BTreeMap<String, serde_json::Value>;
/// 原始模型的规则优先于渠道默认值，避免模型别名合并后失去独立设置。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelRule {
    /// 别名映射以前的客户端模型名。
    model: String,
    /// 在本请求有效的稀疏覆盖表。
    overrides: Leaves,
}
/// 配置边界只接受协议公开的叶键，错误不含原始值（心跳文本可能有私有内容）。
fn valid_leaves(map: &Leaves) -> bool {
    map.iter().all(|(key, v)| match key.as_str() {
        "timeouts.connect_secs"
        | "timeouts.first_byte_secs"
        | "timeouts.read_idle_secs"
        | "retry.max_backoff_secs" => v.as_u64().is_some(),
        "retry.max_attempts" => v.as_u64().is_some_and(|n| u32::try_from(n).is_ok()),
        "keepalive.interval_secs" => v.as_u64().is_some_and(|n| {
            std::time::Instant::now()
                .checked_add(std::time::Duration::from_secs(n))
                .is_some()
        }),
        "keepalive.trigger" => matches!(
            v.as_str(),
            Some("always" | "accept" | "body_stream" | "any")
        ),
        "keepalive.heartbeat" => v.as_str().is_some_and(|h| {
            let s = h.replace("\r\n", "\n").replace('\r', "\n");
            s.ends_with('\n')
                && (s.ends_with("\n\n") || s.lines().all(|l| l.is_empty() || l.starts_with(':')))
        }),
        _ => false,
    })
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
    /// 选中该渠道时生效，仅回送本次 hello 接受的键。
    #[serde(default)]
    overrides: Leaves,
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
    /// 每个路由单元自己的执行环境，仅用于该渠道显式配置的凭据。
    #[serde(skip)]
    pub variables: config::Variables,
    /// 原模型名到上游模型名。
    #[serde(default)]
    aliases: BTreeMap<String, String>,
    /// 开启后与 keys 独立计算相同的用户前缀票号，不传递摘要或会话正文。
    #[serde(default)]
    sticky: bool,
    /// 同一个原始模型只允许一条精确规则。
    #[serde(default)]
    rule: Vec<ModelRule>,
    /// 仅来自当前进程的 hello，绝不从角色配置伪造能力。
    #[serde(skip)]
    offered: std::collections::BTreeSet<String>,
    /// 加载后按优先级稳定排序，组内保留配置顺序作为故障转移次序。
    channel: Vec<Channel>,
}
/// 针对路径的每一层解码都检查边界；已编码的分隔符也不能把普通段变成父段。
/// 八层后仍有可解码字符时保守拒绝，限制重复扫描的成本。实际转发始终使用未解码的路径。
pub(super) fn safe_child_path(path: &[u8]) -> bool {
    let unsafe_layer = |p: &[u8]| {
        p.iter().any(|b| *b == b'\\' || b.is_ascii_control())
            || p.split(|b| *b == b'/').any(|s| matches!(s, b"." | b".."))
    };
    if unsafe_layer(path) {
        return false;
    }
    if !path.contains(&b'%') {
        return true;
    }
    let mut layer = path.to_vec();
    for depth in 0..=8 {
        let mut read = 0;
        let mut write = 0;
        while read < layer.len() {
            if layer[read] == b'%'
                && let Some(pair) = layer.get(read + 1..read + 3)
                && let (Some(hi), Some(lo)) = (
                    (pair[0] as char).to_digit(16),
                    (pair[1] as char).to_digit(16),
                )
            {
                if depth == 8 {
                    return false;
                }
                layer[write] = (hi * 16 + lo) as u8;
                read += 3;
            } else {
                layer[write] = layer[read];
                read += 1;
            }
            write += 1;
        }
        if write == layer.len() {
            return true;
        }
        layer.truncate(write);
        if unsafe_layer(&layer) {
            return false;
        }
    }
    true
}
/// 保留客户端路径时只允许普通路径段。URL 写入时会解释点段与反斜线，必须先检查原文。
fn preserved_path(raw: &str) -> Result<&str, Fault> {
    let path = raw.split_once('?').map_or(raw, |(p, _)| p);
    if !path.starts_with('/') || !safe_child_path(path.as_bytes()) {
        return Err(Fault::Path);
    }
    Ok(path)
}
impl Route {
    /// 结构检查只列出凭据的来源名称，不读环境值。
    pub fn credential_names(&self) -> impl Iterator<Item = &str> {
        self.channel
            .iter()
            .filter_map(|c| c.credential_env.as_deref())
    }
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
    pub fn load(path: config::Source<'_>) -> Result<Self, &'static str> {
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
        let mut models = std::collections::HashSet::new();
        if cfg
            .rule
            .iter()
            .any(|r| r.model.is_empty() || !models.insert(&r.model) || !valid_leaves(&r.overrides))
            || cfg.channel.iter().any(|c| !valid_leaves(&c.overrides))
        {
            return Err("模型规则重复、为空或覆盖项无效");
        }
        for c in &cfg.channel {
            if c.weight == 0 {
                return Err("渠道权重须为正数");
            }
            if config::state_string_bytes(&c.id) + alias_bytes + 24 > config::STATE_BYTES {
                return Err("渠道标识与别名生成的状态超过 64 KiB");
            }
            config::endpoint(&c.url)?;
            if c.id.is_empty() || !ids.insert(&c.id) {
                return Err("渠道标识为空或重复");
            }
            if let Some(name) = &c.credential_env {
                config::credential_name(name).map_err(|fault| fault.wire())?;
            }
            if let Some(h) = &c.auth_header {
                config::credential_header(h)?;
            }
        }
        cfg.channel.sort_by_key(|c| c.priority);
        Ok(cfg)
    }
    /// hello 所给的清单是能力边界；即使配置知道更多叶键也不能写入回信。
    pub fn negotiate(&mut self, names: &[String]) {
        self.offered = names.iter().cloned().collect();
    }
    /// 按渠道、再按模型逐个覆写叶键；没有双方同意的项就整个省略。
    fn overrides(&self, channel: &Channel, original: &str) -> Option<w::Overrides> {
        let entries = channel.overrides.iter().chain(
            self.rule
                .iter()
                .find(|r| r.model == original)
                .into_iter()
                .flat_map(|r| &r.overrides),
        );
        let mut wire = w::Overrides::new();
        for (name, v) in entries {
            if self.offered.contains(name) {
                let (group, leaf) = name.split_once('.').expect("加载时校验叶键");
                wire.entry(group).or_insert_with(|| serde_json::json!({}))[leaf] = v.clone();
            }
        }
        (!wire.is_empty()).then_some(wire)
    }
    /// 只对初始优先级组按权重确定首选，然后按配置顺序遍历其余匹配项。重试沿同一票号继续，避免两个坏高优先级渠道相互切换。
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
        if let Some(group) = eligible
            .chunk_by_mut(|a, b| a.priority == b.priority)
            .next()
        {
            let selected =
                crate::selection::preferred(b"route", ticket, group.iter().copied(), |c| {
                    (&c.id, c.weight)
                })
                .expect("组非空");
            let first = group.iter().position(|c| c.id == selected.id).unwrap();
            group[..=first].rotate_right(1);
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
                .map_err(|_| Fault::RequestJson)?;
        let original = body["model"].as_str().ok_or(Fault::MissingModel)?;
        let model = self
            .aliases
            .get(original)
            .map(String::as_str)
            .unwrap_or(original);
        let prior = retry
            .and(m.context.state.as_deref())
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
            .ok_or(Fault::NoChannel)?;
        let mut url = config::endpoint(&channel.url).map_err(|_| Fault::ChannelUrl)?;
        let incoming = config::endpoint(&m.request.url).map_err(|_| Fault::UpstreamUrl)?;
        if channel.preserve_path {
            url.set_path(&format!(
                "{}{}",
                url.path().trim_end_matches('/'),
                preserved_path(&m.client.path)?
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
                &self.variables,
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
        r.overrides = self.overrides(channel, original);
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
        let route = Route::load(config::Source::Worker(file.to_str().unwrap())).unwrap();
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
    /// 首选位于中间时，其余候选按配置顺序，而不是从首选之后循环排列。
    #[test]
    fn failover_follows_configuration_after_middle_winner() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("channels");
        std::fs::write(
            &f,
            ["a", "b", "c", "d", "e"]
                .iter()
                .map(|id| {
                    format!(
                        "[[channel]]\nid='{id}'\nurl='https://{id}.test'\npriority={}\n",
                        if *id >= "d" { 2 } else { 1 }
                    )
                })
                .collect::<String>(),
        )
        .unwrap();
        let route = Route::load(config::Source::Worker(f.to_str().unwrap())).unwrap();
        let t = (0..100)
            .find(|&t| route.choose("m", t, None).unwrap().id == "b")
            .unwrap();
        for (prev, next) in [
            (None, "b"),
            (Some("b"), "a"),
            (Some("a"), "c"),
            (Some("c"), "d"),
            (Some("d"), "e"),
            (Some("e"), "b"),
        ] {
            assert_eq!(route.choose("m", t, prev).unwrap().id, next);
        }
    }
    /// 拒绝会被 URL 归一化的完整段，不影响普通文件名和查询中的点号。
    #[test]
    fn preserved_path_only_accepts_literal_child_segments() {
        for segment in [".", "..", "%2E", ".%2E", "%2e.", "%2e%2E", "a\\b"] {
            assert!(preserved_path(&format!("/safe/{segment}/leaf")).is_err());
        }
        // 点号的十六进制数字本身也可被编码，不能只替换固定的 %252e 拼法。
        for path in [
            "/a/%252e%252E/b",
            "/a/%25%32%45./b",
            "/a%252f..%252fb",
            "/a/%2525252525252525252e/b",
        ] {
            assert!(preserved_path(path).is_err());
        }
        for path in [
            "/a/.../leaf",
            "/.well-known/v1",
            "/%2ejson",
            "/%252ejson",
            "/%E7%A7%9F%E6%88%B7",
        ] {
            assert_eq!(
                preserved_path(&format!("{path}?q=..")).map_err(|e| e.wire()),
                Ok(path)
            );
        }
    }
    /// 只回双方同意的叶键；模型覆盖渠道同名项，别名对应的上游模型不误中原始模型规则。
    #[test]
    fn overrides_are_sparse_negotiated_and_model_specific() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("route.toml");
        let cfg = "[[channel]]\nid='a'\nurl='https://a.test'\n[channel.overrides]\n'retry.max_attempts'=4\n'keepalive.interval_secs'=2\n'keepalive.trigger'='always'\n[[rule]]\nmodel='small'\n[rule.overrides]\n'retry.max_attempts'=1\n 'timeouts.first_byte_secs'=12\n";
        std::fs::write(&file, cfg).unwrap();
        let mut route = Route::load(config::Source::Worker(file.to_str().unwrap())).unwrap();
        assert!(route.overrides(&route.channel[0], "small").is_none());
        route.negotiate(&[
            "retry.max_attempts".into(),
            "keepalive.trigger".into(),
            "timeouts.first_byte_secs".into(),
        ]);
        assert_eq!(
            serde_json::Value::Object(route.overrides(&route.channel[0], "small").unwrap()),
            serde_json::json!({"retry":{"max_attempts":1},"keepalive":{"trigger":"always"},"timeouts":{"first_byte_secs":12}})
        );
        assert_eq!(
            serde_json::Value::Object(route.overrides(&route.channel[0], "mapped").unwrap()),
            serde_json::json!({"retry":{"max_attempts":4},"keepalive":{"trigger":"always"}})
        );
        for bad in [
            "'retry.max_attempts'=4294967296",
            "'keepalive.trigger'='unexpected'",
            "'keepalive.heartbeat'='data: partial'",
            "'timeouts.connect_secs'=-1",
            "'retry.unknown'=0",
        ] {
            std::fs::write(&file, cfg.replace("'retry.max_attempts'=4", bad)).unwrap();
            assert!(
                Route::load(config::Source::Worker(file.to_str().unwrap())).is_err(),
                "{bad}"
            );
        }
    }
}

/// 路由加载只检查凭据来源的名字，不以离线机器是否设置了这个变量作为有效性条件。
#[cfg(test)]
#[test]
fn route_validates_credential_source_before_hello() {
    for (name, valid) in [
        ("", false),
        ("BROKEN=NAME", false),
        ("NOT_SET_FOR_THIS_TEST", true),
    ] {
        let text =
            format!("[[channel]]\nid='one'\nurl='https://relay.test'\ncredential_env='{name}'");
        assert_eq!(Route::load(config::Source::Offline(&text)).is_ok(), valid);
    }
}
