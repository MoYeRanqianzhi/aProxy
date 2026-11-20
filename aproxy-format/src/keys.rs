//! 每个上游 origin 单独选 key，只有返回请求的头部能看到值，私有状态与报告只写公开 ID。
use crate::config;
use crate::config::Fault;
use aproxy_envelope as w;
use serde::Deserialize;

/// 密钥条目的权重缺省为一次轮转份额。
fn one() -> u64 {
    1
}
/// 选择顺序由同一持久工作进程维护。
#[derive(Deserialize, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Strategy {
    /// 每个可用 key 各用一次。
    #[default]
    RoundRobin,
    /// 按配置份额轮转，不展开与权重成比例的数组。
    Weighted,
}
/// 明文始终从环境读取，不放进配置与工作进程回传的状态。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Key {
    /// 报告与状态中的公开名字。
    id: String,
    /// 环境变量名，不是认证值。
    env: String,
    /// 加权轮转时的连续份额。
    #[serde(default = "one")]
    weight: u64,
}
/// 同主机不同端口亦视为不同认证范围。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Pool {
    /// 只接受 origin，不允许路径或查询串扩大匹配范围。
    origin: String,
    /// 此池使用的认证头名。
    header: String,
    /// 例如 Bearer；x-api-key 使用空串。
    #[serde(default)]
    scheme: String,
    /// 按列出顺序轮转。
    keys: Vec<Key>,
    /// 每个 worker 独立持有游标，不随请求状态走。
    #[serde(skip)]
    cursor: u64,
}
/// 目的地未知时拒绝请求，不能回退到第一池。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Keys {
    /// 没指定策略时均等轮转。
    #[serde(default)]
    strategy: Strategy,
    /// 按目标来源隔离密钥。
    pool: Vec<Pool>,
}
impl Keys {
    /// 在 hello 前检查份额总数、认证名与 origin 的唯一性。
    pub fn load(path: &str) -> Result<Self, &'static str> {
        let mut cfg: Self = config::load(path)?;
        let mut origins = std::collections::HashSet::new();
        if cfg.pool.is_empty() {
            return Err("至少需要一个密钥池");
        }
        for pool in &mut cfg.pool {
            let url = config::endpoint(&pool.origin)?;
            if url.path() != "/" || url.query().is_some() || !origins.insert(url.origin()) {
                return Err("密钥池的 origin 无效或重复");
            }
            pool.origin = url.origin().ascii_serialization();
            pool.header = config::credential_header(&pool.header)?.to_string();
            let origin_bytes = config::state_string_bytes(&pool.origin);
            if pool
                .keys
                .iter()
                .any(|k| origin_bytes + config::state_string_bytes(&k.id) + 3 > config::STATE_BYTES)
            {
                return Err("密钥标识生成的状态超过 64 KiB");
            }
            let mut ids = std::collections::HashSet::new();
            if pool.keys.is_empty()
                || pool
                    .keys
                    .iter()
                    .any(|k| k.id.is_empty() || !ids.insert(&k.id) || k.weight == 0)
                || pool
                    .keys
                    .iter()
                    .try_fold(0u64, |n, k| n.checked_add(k.weight))
                    .is_none()
            {
                return Err("密钥池为空、标识重复或权重无效");
            }
        }
        Ok(cfg)
    }
    /// 只向当前 origin 返回凭据；失败不推进游标，下一次可继续原轮次。
    pub fn request(&mut self, m: &w::RequestMessage, r: &mut w::Reply) -> Result<(), Fault> {
        let origin = config::endpoint(&m.request.url)
            .map_err(|e| Fault::new(e, "invalid upstream URL"))?
            .origin()
            .ascii_serialization();
        let mut headers = m.request.headers.clone();
        config::strip_credentials(&mut headers);
        for p in &self.pool {
            headers.remove(&p.header);
        }
        let p = self
            .pool
            .iter_mut()
            .find(|p| p.origin == origin)
            .ok_or(Fault::new(
                "上游没有对应的密钥池",
                "no credential pool matches the upstream origin",
            ))?;
        let weight = |k: &Key| {
            if self.strategy == Strategy::Weighted {
                k.weight
            } else {
                1
            }
        };
        let total: u64 = p.keys.iter().map(weight).sum();
        let mut slot = p.cursor % total;
        let key = p
            .keys
            .iter()
            .find(|k| {
                let w = weight(k);
                if slot < w {
                    true
                } else {
                    slot -= w;
                    false
                }
            })
            .expect("已验证非空且权重总和有效");
        headers.insert(p.header.clone(), config::credential(&key.env, &p.scheme)?);
        p.cursor = (p.cursor % total + 1) % total;
        let mut patch = w::RequestPatch::new();
        patch.headers = Some(headers);
        r.request = Some(patch);
        r.state = Some(serde_json::json!([p.origin, key.id]).to_string());
        r.report = config::report(
            "Selected an upstream key",
            w::StandardAction::KeySelect {
                key_id: key.id.clone(),
            },
        );
        Ok(())
    }
}
