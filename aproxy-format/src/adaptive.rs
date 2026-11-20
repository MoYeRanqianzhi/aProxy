//! 按完整上传摘要记交付结果。关闭工作进程只会失去偏好，不影响代理自身的重试保证。
use crate::config;
use aproxy_envelope as w;
use serde::Deserialize;
use std::{
    collections::{BTreeSet, HashMap},
    time::{Duration, Instant},
};

/// 决策参数与进程内缓存分开，任何 TOML 字段都不能注入历史记录。
#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Settings {
    /// 连续几次失败或 forward 升级后固定为 retry。
    failure_threshold: u32,
    /// 单进程保留的不同摘要数上限。
    max_entries: usize,
    /// 不再访问的摘要经过该秒数后从索引和缓存同时删除。
    ttl_secs: u64,
    /// 固定 retry 后经过该时长才开始恢复观测。
    hold_secs: u64,
    /// 一组完整样本才能决定是否恢复 forward。
    recovery_samples: u32,
    /// 低延迟且正常结束的样本需达到的比例。
    recovery_success_ratio: f64,
    /// 单个恢复样本可接受的请求全程耗时。
    recovery_latency_ms: u64,
}
impl Default for Settings {
    /// 短暂出错可当次升级，只有重复失败才影响下一次请求。
    fn default() -> Self {
        Self {
            failure_threshold: 2,
            max_entries: 4096,
            ttl_secs: 900,
            hold_secs: 60,
            recovery_samples: 10,
            recovery_success_ratio: 0.9,
            recovery_latency_ms: 1000,
        }
    }
}
/// 记录大小固定，不保留请求文本、响应或增长的样本列表。
struct Record {
    /// 最近一次真实访问的单调时间，也是过期索引的键。
    seen: Instant,
    /// 时间分辨率内多次访问的先后次序，不能用摘要排序替代。
    order: u64,
    /// 尚未固定模式时的连续失败数，成功归零。
    failures: u32,
    /// 从此时起固定 retry；None 表示允许尝试 forward。
    pinned: Option<Instant>,
    /// 当前恢复窗口中已收到的样本数。
    samples: u32,
    /// 窗口内正常、未升级并及时完成的样本数。
    healthy: u32,
}
/// 单个持久进程拥有的摘要与过期索引；不同 worker 不假装共享内存。
pub struct Adaptive {
    /// 固定在本次握手的判定阈值。
    settings: Settings,
    /// 解码后的 32 字节摘要保证无效数据不能被存成用户文本。
    records: HashMap<[u8; 32], Record>,
    /// 按最后访问时间淘汰，不为查到一个摘要而扫描整个表。
    age: BTreeSet<(Instant, u64, [u8; 32])>,
    /// 单进程访问序列，只用于同一时刻的次序。
    order: u64,
}
/// null 或不规范的摘要不具有身份，不能与完整空体的 SHA-256 混同。
fn digest(ctx: &w::Context) -> Option<[u8; 32]> {
    let s = ctx.digest.as_ref()?.as_ref()?.as_bytes();
    if s.len() != 64 || !s.iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    let mut key = [0; 32];
    for (out, pair) in key.iter_mut().zip(s.as_chunks::<2>().0) {
        *out = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    Some(key)
}
impl Adaptive {
    /// 在公布健康之前校验所有阈值；空文件使用和无文件相同的默认值。
    pub fn load(path: Option<&str>) -> Result<Self, &'static str> {
        let settings: Settings = match path {
            Some(p) => config::load(p)?,
            None => Default::default(),
        };
        if settings.failure_threshold == 0
            || settings.max_entries == 0
            || settings.ttl_secs == 0
            || settings.recovery_samples == 0
            || !(settings.recovery_success_ratio > 0.0 && settings.recovery_success_ratio <= 1.0)
        {
            return Err("自适应阈值、容量、寿命或恢复比例无效");
        }
        Ok(Self {
            settings,
            records: HashMap::new(),
            age: BTreeSet::new(),
            order: 0,
        })
    }
    /// 刷新已有条目而不占第二个位；只淘汰确已过期或为新身份腾空间的最旧记录。
    fn touch(&mut self, key: [u8; 32], now: Instant) -> &mut Record {
        while let Some(&(at, _, id)) = self.age.first() {
            if now.duration_since(at) < Duration::from_secs(self.settings.ttl_secs) {
                break;
            }
            self.age.pop_first();
            self.records.remove(&id);
        }
        if let Some(r) = self.records.get(&key) {
            self.age.remove(&(r.seen, r.order, key));
        } else if self.records.len() == self.settings.max_entries
            && let Some((_, _, old)) = self.age.pop_first()
        {
            self.records.remove(&old);
        }
        self.order += 1;
        self.age.insert((now, self.order, key));
        let r = self.records.entry(key).or_insert(Record {
            seen: now,
            order: self.order,
            failures: 0,
            pinned: None,
            samples: 0,
            healthy: 0,
        });
        r.seen = now;
        r.order = self.order;
        r
    }
    /// 没有可用摘要时弃权；有摘要也只报告投递策略，不报告摘要或用户内容。
    pub fn request(&mut self, m: &w::RequestMessage, r: &mut w::Reply) {
        if let Some(key) = digest(&m.context) {
            let pinned = self.touch(key, Instant::now()).pinned.is_some();
            let mode = if pinned { "retry" } else { "forward" };
            r.delivery = Some(mode.into());
            r.report = w::ReportField::Present(w::Report::new(
                if pinned {
                    "Voted for retry delivery after previous failures"
                } else {
                    "Voted for forward delivery"
                }
                .into(),
            ));
        }
    }
    /// 代理只在还能改变决定时咨询；不能用这票覆盖已经处于 retry 的请求或另一条停止策略。
    pub fn decide(&self, m: &w::DecideMessage, r: &mut w::Reply) {
        if m.delivery == w::Delivery::Forward && m.outcome == "failure" {
            r.decision = Some(w::Decision::Retry);
            r.report = w::ReportField::Present(w::Report::new(
                "Voted to upgrade a failed forward attempt to retry".into(),
            ));
        }
    }
    /// 完成通知是最终事实；升级后最终成功也表明先前的 forward 不适合该摘要。
    pub fn completed(&mut self, m: &w::Completion, now: Instant) {
        let Some(key) = digest(&m.context) else {
            return;
        };
        let ok = m.outcome == "ok"
            && !m.upgraded
            && m.stream_completed != Some(false)
            && m.upstream_status.is_none_or(|s| s < 400);
        let Settings {
            failure_threshold,
            hold_secs,
            recovery_samples,
            recovery_success_ratio,
            recovery_latency_ms,
            ..
        } = self.settings;
        let r = self.touch(key, now);
        if let Some(at) = r.pinned {
            if now.duration_since(at) < Duration::from_secs(hold_secs) {
                return;
            }
            r.samples += 1;
            r.healthy += u32::from(ok && m.duration_ms <= recovery_latency_ms);
            if r.samples == recovery_samples {
                if f64::from(r.healthy) / f64::from(r.samples) >= recovery_success_ratio {
                    r.pinned = None;
                    r.failures = 0;
                }
                r.samples = 0;
                r.healthy = 0;
            }
        } else if ok {
            r.failures = 0;
        } else {
            r.failures = r.failures.saturating_add(1);
            if r.failures >= failure_threshold {
                r.pinned = Some(now);
            }
        }
    }
}

#[cfg(test)]
/// 以显式单调时刻验证冷却与恢复，既不睡眠，也不会因宿主调度速度改变判断。
mod tests {
    use super::*;
    /// 构造只含摘要和结果的完成帧；从未包含请求正文。
    fn event(id: u8, ok: bool, ms: u64) -> w::Completion {
        let mut ctx = w::Context::new("r".into());
        ctx.digest = Some(Some(format!("{id:02x}").repeat(32)));
        w::Completion::new(
            ctx,
            w::Delivery::Forward,
            1,
            ms,
            if ok { "ok" } else { "upstream_error" }.into(),
        )
    }
    /// 恢复必须有实际成功的样本，零比例或非有限值不得在握手时通过。
    #[test]
    fn recovery_needs_positive_finite_ratio() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("adaptive.toml");
        for (ratio, valid) in [
            ("0.0", false),
            ("-0.1", false),
            ("nan", false),
            ("1.1", false),
            ("0.01", true),
            ("1.0", true),
        ] {
            std::fs::write(&path, format!("recovery_success_ratio={ratio}")).unwrap();
            assert_eq!(Adaptive::load(path.to_str()).is_ok(), valid);
        }
    }
    /// 正常完成清零，升级与流中断各算一次；保留期后要求完整且健康的恢复窗口。
    #[test]
    fn repeated_failures_hold_retry_until_a_healthy_window() {
        let mut a = Adaptive::load(None).unwrap();
        a.settings.hold_secs = 10;
        a.settings.recovery_samples = 2;
        a.settings.recovery_success_ratio = 1.0;
        a.settings.recovery_latency_ms = 30;
        let t = Instant::now();
        let good = event(1, true, 21);
        let bad = event(1, false, 21);
        a.completed(&bad, t);
        a.completed(&good, t);
        assert_eq!(a.records[&[1; 32]].failures, 0);
        let mut upgraded = event(1, true, 5);
        upgraded.upgraded = true;
        a.completed(&upgraded, t);
        assert!(a.records[&[1; 32]].pinned.is_none());
        let mut cut = event(1, true, 5);
        cut.stream_completed = Some(false);
        a.completed(&cut, t);
        assert!(a.records[&[1; 32]].pinned.is_some());
        a.completed(&good, t + Duration::from_secs(9));
        assert_eq!(a.records[&[1; 32]].samples, 0);
        let later = t + Duration::from_secs(10);
        a.completed(&event(1, true, 31), later);
        a.completed(&good, later);
        assert!(a.records[&[1; 32]].pinned.is_some());
        a.completed(&good, later);
        assert!(a.records[&[1; 32]].pinned.is_some());
        a.completed(&good, later);
        assert!(a.records[&[1; 32]].pinned.is_none());
    }
    /// 投票不是已经交付的事实；报告需让另一个程序赢得表决的场景也成立。
    #[test]
    fn delivery_reports_describe_votes() {
        let mut a = Adaptive::load(None).unwrap();
        let mut m:w::RequestMessage=serde_json::from_value(serde_json::json!({"request_id":"r","digest":"ab".repeat(32),"delivery":"retry","request":{"method":"POST","url":"https://a.test"},"client":{"path":"/"}})).unwrap();
        let mut r = w::Reply::new(1);
        a.request(&m, &mut r);
        assert_eq!(
            serde_json::to_value(&r).unwrap()["report"]["summary"],
            "Voted for forward delivery"
        );
        m.delivery = w::Delivery::Forward;
        let d:w::DecideMessage=serde_json::from_value(serde_json::json!({"request_id":"r","attempt":1,"elapsed_ms":0,"delivery":m.delivery,"outcome":"failure","request":{"method":"POST","url":"https://a.test"},"fallback":{"decision":"stop","by":"default","delay_ms":0}})).unwrap();
        let mut r = w::Reply::new(2);
        a.decide(&d, &mut r);
        assert_eq!(
            serde_json::to_value(r).unwrap()["report"]["summary"],
            "Voted to upgrade a failed forward attempt to retry"
        );
    }
    /// 容量淘汰最久未用项，过期时连索引一起清理；无效摘要永不进表。
    #[test]
    fn expiry_lru_and_null_digest_bound_storage() {
        let mut a = Adaptive::load(None).unwrap();
        a.settings.max_entries = 2;
        a.settings.ttl_secs = 5;
        let t = Instant::now();
        for k in [1, 2, 1, 3] {
            a.completed(&event(k, false, 0), t);
        }
        assert!(!a.records.contains_key(&[2; 32]));
        assert_eq!(a.age.len(), 2);
        a.completed(&event(4, false, 0), t + Duration::from_secs(8));
        assert_eq!((a.records.len(), a.age.len()), (1, 1));
        let mut invalid = event(0, false, 0);
        for value in [None, Some("private".into()), Some("+a".repeat(32))] {
            invalid.context.digest = Some(value);
            a.completed(&invalid, t + Duration::from_secs(9));
        }
        assert_eq!(a.records.len(), 1);
    }
}
