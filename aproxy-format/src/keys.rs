//! 每个上游 origin 单独选 key，只有返回请求的头部能看到值，私有状态与报告只写公开 ID。
use crate::config;
use crate::config::Fault;
use aproxy_envelope as w;
use serde::Deserialize;
use std::time::{Duration, Instant, SystemTime};

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
    /// 每次从可用密钥中均匀抽取，不令失败项占据抽样区间。
    Random,
}
/// 明文始终从环境读取，不放进配置与工作进程回传的状态。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Key {
    /// 报告与状态中的公开名字。
    id: String,
    /// 开始冷却的单调时刻和持续时间，极大 Retry-After 也不会令时钟加法溢出。
    #[serde(skip)]
    cooldown: Option<(Instant, Duration)>,
    /// 环境变量名，不是认证值。
    env: String,
    /// 握手前验证并取好的出站值。只交给所选 origin 的请求头，不参与 state 或报告。
    #[serde(skip)]
    value: String,
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
    cursor: u128,
}
/// 目的地未知时拒绝请求，不能回退到第一池。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Keys {
    /// 没指定策略时均等轮转。
    #[serde(default)]
    strategy: Strategy,
    /// 启用时才请求完整正文；缺少稳定前缀时仍使用配置的常规选择策略。
    #[serde(default)]
    pub sticky: bool,
    /// 仅这些上游状态触发身份冷却，链内故障不能被当成坏 key。
    #[serde(default = "default_statuses")]
    cooldown_status: Vec<u16>,
    /// 一次冷却的最短秒数；上游可用 Retry-After 要求更长间隔。
    #[serde(default = "default_cooldown")]
    cooldown_secs: u64,
    /// 离线演练可固定同一时刻；普通工作进程每次读取当前单调时钟与 HTTP 日期时钟。
    #[serde(skip)]
    pub(super) sample_time: Option<(Instant, SystemTime)>,
    /// 按目标来源隔离密钥。
    pool: Vec<Pool>,
}
/// 默认只隔离鉴权失败与限流，不因全站 503 把一池 key 全部冷却。
fn default_statuses() -> Vec<u16> {
    vec![401, 403, 429]
}
/// 上游没给 Retry-After 时使用的冷却窗口。
fn default_cooldown() -> u64 {
    30
}
impl Key {
    /// 已经过了窗口即可重新使用；比较经过时间，不需要构造无限远的截止时刻。
    fn remaining(&self, now: Instant) -> Duration {
        self.cooldown.map_or(Duration::ZERO, |(start, span)| {
            span.saturating_sub(now.saturating_duration_since(start))
        })
    }
}
impl Pool {
    /// 只在可用集合中取票；给定随机源让测试核对边界而不靠抽样碰运气。
    fn select(
        &mut self,
        strategy: &Strategy,
        sticky: Option<u64>,
        now: Instant,
        random: impl FnOnce(u128) -> u128,
    ) -> Option<&Key> {
        let weight = |k: &Key| {
            if *strategy == Strategy::Weighted {
                k.weight
            } else {
                1
            }
        };
        let eligible = || self.keys.iter().filter(|k| k.remaining(now).is_zero());
        if let Some(ticket) = sticky {
            return crate::selection::preferred(b"keys", ticket, eligible(), |k| {
                (&k.id, weight(k))
            });
        }
        let total: u128 = eligible().map(|k| u128::from(weight(k))).sum();
        if total == 0 {
            return None;
        }
        let mut slot = if *strategy == Strategy::Random {
            random(total)
        } else {
            let index = self.cursor % total;
            self.cursor = (index + 1) % total;
            index
        };
        eligible().find(|k| {
            let n = u128::from(weight(k));
            if slot < n {
                true
            } else {
                slot -= n;
                false
            }
        })
    }
}
impl Keys {
    /// 结构检查只列出凭据的来源名称，不读环境值。
    pub fn credential_names(&self) -> impl Iterator<Item = &str> {
        self.pool
            .iter()
            .flat_map(|p| p.keys.iter().map(|k| k.env.as_str()))
    }
    /// 在 hello 前检查份额总数、认证名与 origin 的唯一性。
    pub fn load(
        path: config::Source<'_>,
        variables: &config::Variables,
    ) -> Result<Self, config::LoadIssue> {
        let mut cfg: Self = config::load(path)?;
        let mut origins = std::collections::HashSet::new();
        if cfg.pool.is_empty() {
            return Err("at least one credential pool is required".into());
        }
        if cfg.cooldown_status.iter().any(|s| !(100..=599).contains(s)) {
            return Err("cooldown status must be between 100 and 599".into());
        }
        for pool in &mut cfg.pool {
            let url = config::endpoint(&pool.origin)?;
            if url.path() != "/" || url.query().is_some() || !origins.insert(url.origin()) {
                return Err("invalid or duplicate credential pool origin".into());
            }
            pool.origin = url.origin().ascii_serialization();
            for key in &mut pool.keys {
                key.value = config::credential(&key.env, &pool.scheme, variables)
                    .map_err(|f| format!("environment variable {:?}: {}", key.env, f.wire()))?;
            }
            pool.header = config::credential_header(&pool.header)?.to_string();
            let origin_bytes = config::state_string_bytes(&pool.origin);
            if pool
                .keys
                .iter()
                .any(|k| origin_bytes + config::state_string_bytes(&k.id) + 3 > config::STATE_BYTES)
            {
                return Err("state generated from credential identifiers exceeds 64 KiB".into());
            }
            let mut ids = std::collections::HashSet::new();
            if pool.keys.is_empty()
                || pool
                    .keys
                    .iter()
                    .any(|k| k.id.is_empty() || !ids.insert(&k.id) || k.weight == 0)
            {
                return Err(
                    "empty credential pool, duplicate identifier, or invalid weight".into(),
                );
            }
        }
        Ok(cfg)
    }
    /// 给上一轮的池和 key 记账，不能因 route 已换目的地而冷却新的渠道。
    fn charge(&mut self, state: Option<&str>, retry: &w::Retry, now: Instant, wall: SystemTime) {
        let head = retry.last_response.as_ref();
        let status = head.map(|h| h.status).or_else(|| {
            if retry.last_failure.code != "upstream_status" {
                return None;
            }
            u16::try_from(
                retry
                    .last_failure
                    .details
                    .as_ref()?
                    .get("status")?
                    .as_u64()?,
            )
            .ok()
        });
        if !status.is_some_and(|s| self.cooldown_status.contains(&s)) {
            return;
        }
        let Some((origin, id)) =
            state.and_then(|s| serde_json::from_str::<(String, String)>(s).ok())
        else {
            return;
        };
        if let Some(key) = self
            .pool
            .iter_mut()
            .find(|p| p.origin == origin)
            .and_then(|p| p.keys.iter_mut().find(|k| k.id == id))
        {
            let advised = head
                .and_then(|h| h.headers.get("retry-after"))
                .and_then(|v| crate::selection::retry_after(v, wall))
                .unwrap_or_default();
            key.cooldown = Some((
                now,
                Duration::from_secs(self.cooldown_secs)
                    .max(advised)
                    .max(key.remaining(now)),
            ));
        }
    }
    /// 只向当前 origin 返回凭据；失败不推进游标，下一次可继续原轮次。
    pub fn request(
        &mut self,
        m: &w::RequestMessage,
        retry: Option<&w::Retry>,
        r: &mut w::Reply,
    ) -> Result<(), Fault> {
        let (now, wall) = self
            .sample_time
            .unwrap_or_else(|| (Instant::now(), SystemTime::now()));
        if let Some(retry) = retry {
            self.charge(m.context.state.as_deref(), retry, now, wall);
        }
        let sticky = if self.sticky {
            m.request
                .body
                .as_bytes()
                .and_then(|b| serde_json::from_slice(b).ok())
                .as_ref()
                .and_then(crate::selection::conversation_ticket)
        } else {
            None
        };
        let origin = config::endpoint(&m.request.url)
            .map_err(|_| Fault::UpstreamUrl)?
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
            .ok_or(Fault::NoPool)?;
        let header = p.header.clone();
        let Some(key) = p.select(&self.strategy, sticky, now, |n| rand::random_range(0..n)) else {
            r.fail = Some(w::Fail::new(
                true,
                "all keys for this upstream are cooling down".into(),
            ));
            return Ok(());
        };
        let id = key.id.clone();
        headers.insert(header, key.value.clone());
        let mut patch = w::RequestPatch::new();
        patch.headers = Some(headers);
        r.request = Some(patch);
        r.state = Some(serde_json::json!([p.origin, id]).to_string());
        r.report = config::report(
            "Selected an upstream key",
            w::StandardAction::KeySelect { key_id: id },
        );
        Ok(())
    }
}

#[cfg(test)]
/// 用确定的时钟和票号覆盖各策略，不用真实等待或要求随机样本恰好包含某个键。
mod tests {
    use super::*;
    /// 只测试选取与冷却，不访问环境；两个 origin 故意复用同一个公开 ID。
    fn pools() -> Keys {
        toml::from_str("[[pool]]\norigin='https://one.test'\nheader='authorization'\nkeys=[{id='a',env='A',weight=2},{id='b',env='B'}]\n[[pool]]\norigin='https://two.test'\nheader='authorization'\nkeys=[{id='a',env='C'}]").unwrap()
    }
    /// 顺序、权重与随机票都只在可用集合中解释；粘性命中不消耗轮转游标。
    #[test]
    fn selectors_cover_the_eligible_range() {
        let now = Instant::now();
        for (strategy, expected) in [
            (Strategy::RoundRobin, ["a", "b", "a"]),
            (Strategy::Weighted, ["a", "a", "b"]),
        ] {
            let mut k = pools();
            let p = &mut k.pool[0];
            for id in expected {
                assert_eq!(
                    p.select(&strategy, None, now, |_| unreachable!())
                        .unwrap()
                        .id,
                    id
                );
            }
        }
        let mut k = pools();
        let p = &mut k.pool[0];
        for (ticket, id) in [(0, "a"), (1, "b")] {
            assert_eq!(
                p.select(&Strategy::Random, None, now, |n| {
                    assert_eq!(n, 2);
                    ticket
                })
                .unwrap()
                .id,
                id
            );
        }
        for _ in 0..3 {
            assert_eq!(
                p.select(&Strategy::Weighted, Some(0), now, |_| unreachable!())
                    .unwrap()
                    .id,
                "b"
            );
        }
        assert_eq!(p.cursor, 0);
    }
    /// 收到新渠道的输入时仍处罚上次的密钥；过期瞬间重回可选，仅配置状态触发冷却。
    #[test]
    fn cooldown_uses_previous_identity_status_and_retry_after() {
        let mut k = pools();
        let now = Instant::now();
        let wall = SystemTime::UNIX_EPOCH;
        let mut retry = w::Retry::new(2, w::Failure::new("upstream_body_interrupted".into()), 0);
        retry.last_response = Some(w::ResponseHead::new(
            429,
            [("retry-after".into(), "600".into())].into(),
        ));
        let state = Some(r#"["https://one.test","a"]"#);
        k.charge(state, &retry, now, wall);
        assert_eq!(k.pool[0].keys[0].remaining(now), Duration::from_secs(600));
        assert_eq!(k.pool[1].keys[0].remaining(now), Duration::ZERO);
        for (secs, id) in [(599, "b"), (600, "a")] {
            assert_eq!(
                k.pool[0]
                    .select(
                        &Strategy::RoundRobin,
                        Some(
                            (0..100u64)
                                .find(|&t| crate::selection::preferred(
                                    b"keys",
                                    t,
                                    ["a", "b"].iter(),
                                    |id| (*id, 1)
                                ) == Some(&"a"))
                                .unwrap()
                        ),
                        now + Duration::from_secs(secs),
                        |_| unreachable!()
                    )
                    .unwrap()
                    .id,
                id
            );
        }
        retry.last_response.as_mut().unwrap().status = 503;
        k.charge(Some(r#"["https://one.test","b"]"#), &retry, now, wall);
        assert!(k.pool[0].keys[1].cooldown.is_none());
        k.cooldown_status = vec![503];
        k.charge(Some(r#"["https://one.test","b"]"#), &retry, now, wall);
        assert!(
            k.pool[0]
                .select(&Strategy::Random, None, now, |_| unreachable!())
                .is_none()
        );
        assert_eq!(
            crate::selection::retry_after("Thu, 01 Jan 1970 00:00:05 GMT", wall),
            Some(Duration::from_secs(5))
        );
        assert_eq!(crate::selection::retry_after("-2", wall), None);
    }
}
