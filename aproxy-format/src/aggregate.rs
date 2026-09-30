//! 渠道路由与 key 轮换。轮换计数在进程内存——persistent 模式天然成立；
//! spawn 模式每次新进程计数恒 0（退化为总是首个 key），聚合场景必须用
//! persistent 模式（文档与 skill 已明示）。

use std::sync::atomic::{AtomicU64, Ordering};

use crate::config::{AggConfig, ChannelConfig, KeyStrategy, weighted_index_table};

/// 聚合状态：配置 + 每渠道轮换计数器。
pub struct AggState {
    pub cfg: AggConfig,
    counters: Vec<AtomicU64>,
}

impl AggState {
    pub fn new(cfg: AggConfig) -> Self {
        let n = cfg.channels.len();
        Self {
            counters: (0..n).map(|_| AtomicU64::new(0)).collect(),
            cfg,
        }
    }

    /// 按 model 路由渠道：先应用别名映射，再按渠道的 models glob 匹配
    /// （声明顺序取第一个命中；渠道 models 缺省 = 匹配全部）。
    pub fn route(&self, model: &str) -> Option<&ChannelConfig> {
        let upstream_model = self
            .cfg
            .models
            .get(model)
            .map(String::as_str)
            .unwrap_or(model);
        self.cfg.channels.iter().find(|c| match &c.models {
            None => true,
            Some(patterns) => patterns
                .iter()
                .any(|p| glob::Pattern::new(p).is_ok_and(|g| g.matches(upstream_model))),
        })
    }

    /// 从渠道 key 池按策略选一个 key（进程内单调计数轮转）。
    pub fn pick_key(&self, channel_idx: usize) -> String {
        let c = &self.cfg.channels[channel_idx];
        let counter = self.counters[channel_idx].fetch_add(1, Ordering::Relaxed);
        match c.strategy {
            KeyStrategy::RoundRobin => c.keys[(counter % c.keys.len() as u64) as usize].clone(),
            KeyStrategy::Weighted => {
                let table = weighted_index_table(&c.weights);
                c.keys[table[(counter % table.len() as u64) as usize]].clone()
            }
        }
    }
}

/// 从请求 body 提取 model 字段（非 JSON / 无 model 返回 None）。
pub fn extract_model(body: &serde_json::Value) -> Option<&str> {
    body.get("model").and_then(|m| m.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ChannelConfig, ClientFormat};
    use switchyard_translation::WireFormat;

    fn channel(name: &str, models: Option<Vec<String>>) -> ChannelConfig {
        ChannelConfig {
            name: name.to_string(),
            format: WireFormat::AnthropicMessages,
            url: format!("https://{name}.example.com"),
            keys: vec!["k".to_string()],
            strategy: Default::default(),
            weights: vec![],
            models,
            preserve_path: false,
        }
    }

    fn state(channels: Vec<ChannelConfig>) -> AggState {
        AggState::new(AggConfig {
            client_format: ClientFormat::Auto,
            models: Default::default(),
            channels,
        })
    }

    #[test]
    fn routes_by_glob_pattern() {
        let s = state(vec![
            channel("anthropic", Some(vec!["claude-*".to_string()])),
            channel("fallback", None),
        ]);
        assert_eq!(s.route("claude-sonnet-4").unwrap().name, "anthropic");
        assert_eq!(s.route("gpt-x").unwrap().name, "fallback");
    }

    #[test]
    fn route_miss_is_none() {
        let s = state(vec![channel(
            "anthropic",
            Some(vec!["claude-*".to_string()]),
        )]);
        assert!(s.route("gpt-x").is_none());
    }

    #[test]
    fn round_robin_cycles_keys() {
        let mut s = state(vec![channel("a", None)]);
        s.cfg.channels[0].keys = vec!["k1".to_string(), "k2".to_string(), "k3".to_string()];
        let seq: Vec<String> = (0..7).map(|_| s.pick_key(0)).collect();
        assert_eq!(
            seq,
            ["k1", "k2", "k3", "k1", "k2", "k3", "k1"],
            "轮询序列钉死：3 key 循环"
        );
    }

    #[test]
    fn weighted_round_robin_sticks_to_expanded_sequence() {
        let mut s = state(vec![channel("a", None)]);
        s.cfg.channels[0].keys = vec!["A".to_string(), "B".to_string()];
        s.cfg.channels[0].strategy = KeyStrategy::Weighted;
        s.cfg.channels[0].weights = vec![2, 1];
        let seq: Vec<String> = (0..6).map(|_| s.pick_key(0)).collect();
        assert_eq!(
            seq,
            ["A", "A", "B", "A", "A", "B"],
            "加权 [2,1] 展开序列 AAB 循环"
        );
    }
}
