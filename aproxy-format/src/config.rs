//! 聚合配置：渠道表 / 模型别名 / key 策略。
//!
//! 配置文件由用户在 transform 的 `extra` 里传路径（或 `run --config` 显式指定）。

use std::collections::BTreeMap;

use serde::Deserialize;
use switchyard_translation::WireFormat;

/// key 选择策略。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyStrategy {
    /// 轮询（默认）。
    #[default]
    RoundRobin,
    /// 加权轮询（按 weights 展开序列轮转）。
    Weighted,
}

/// 客户端协议声明：`"auto"`（逐请求检测）或显式 wire format。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientFormat {
    Auto,
    Explicit(WireFormat),
}

impl<'de> Deserialize<'de> for ClientFormat {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        if value.as_str() == Some("auto") {
            return Ok(ClientFormat::Auto);
        }
        WireFormat::deserialize(value)
            .map(ClientFormat::Explicit)
            .map_err(serde::de::Error::custom)
    }
}

/// 单个渠道：一个上游端点 + 它的协议 + key 池。
#[derive(Debug, Clone, Deserialize)]
pub struct ChannelConfig {
    pub name: String,
    /// 该渠道上游协议（switchyard wire format 字符串）。
    pub format: WireFormat,
    /// 上游完整 URL（非 preserve_path 时完整替换信封 url）。
    pub url: String,
    /// key 池（必填非空）。
    pub keys: Vec<String>,
    /// key 策略，默认轮询。
    #[serde(default)]
    pub strategy: KeyStrategy,
    /// 加权轮询的权重（与 keys 等长；仅 strategy=weighted 时使用）。
    #[serde(default)]
    pub weights: Vec<u64>,
    /// 该渠道服务的模型 glob（如 ["claude-*"]）；缺省 = 全部模型。
    #[serde(default)]
    pub models: Option<Vec<String>>,
    /// true = 在 channel.url 后拼接原请求的路径与查询串；false（默认）=
    /// url 完整替换。
    #[serde(default)]
    pub preserve_path: bool,
}

/// 聚合配置根。
#[derive(Debug, Clone, Deserialize)]
pub struct AggConfig {
    /// 客户端协议：auto = 逐请求检测。
    pub client_format: ClientFormat,
    /// 可选模型别名映射（客户端名 → 上游名）。
    #[serde(default)]
    pub models: BTreeMap<String, String>,
    /// 渠道表。
    #[serde(default, rename = "channel")]
    pub channels: Vec<ChannelConfig>,
}

impl AggConfig {
    /// 校验：keys 非空、weights 与 keys 等长（strategy=weighted 时）。
    pub fn validate(&self) -> Result<(), String> {
        if self.channels.is_empty() {
            return Err("至少需要一个 [[channel]]".to_string());
        }
        for c in &self.channels {
            if c.keys.is_empty() {
                return Err(format!("渠道 {} 的 keys 不能为空", c.name));
            }
            if c.strategy == KeyStrategy::Weighted {
                if c.weights.len() != c.keys.len() {
                    return Err(format!(
                        "渠道 {} 的 weights 长度 ({}) 必须与 keys ({}) 一致",
                        c.name,
                        c.weights.len(),
                        c.keys.len()
                    ));
                }
                if c.weights.iter().any(|w| *w == 0) {
                    return Err(format!("渠道 {} 的 weights 不能含 0", c.name));
                }
            }
        }
        Ok(())
    }
}

/// 加权展开序列：weights=[2,1] → 索引 [0,0,1]，轮转序列 A A B（与
/// aproxy-format 单测钉死的序列语义一致）。
pub fn weighted_index_table(weights: &[u64]) -> Vec<usize> {
    let mut table = Vec::new();
    for (i, w) in weights.iter().enumerate() {
        for _ in 0..*w {
            table.push(i);
        }
    }
    table
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_format_parses_auto_and_explicit() {
        let auto: ClientFormat = serde_json::from_str("\"auto\"").unwrap();
        assert_eq!(auto, ClientFormat::Auto);
        // switchyard 的 WireFormat serde 用 snake_case 原生形态
        let explicit: ClientFormat = serde_json::from_str("\"anthropic_messages\"").unwrap();
        assert_eq!(
            explicit,
            ClientFormat::Explicit(WireFormat::AnthropicMessages)
        );
        assert!(serde_json::from_str::<ClientFormat>("\"bogus\"").is_err());
    }

    #[test]
    fn weighted_table_expands_and_validates() {
        assert_eq!(weighted_index_table(&[2, 1]), vec![0, 0, 1]);
        assert_eq!(weighted_index_table(&[1, 1]), vec![0, 1]);
        assert!(weighted_index_table(&[]).is_empty());
    }

    #[test]
    fn validate_rejects_empty_keys_and_weight_mismatch() {
        let cfg: AggConfig = toml::from_str(
            r#"
client_format = "auto"
[[channel]]
name = "a"
format = "anthropic_messages"
url = "https://a.example.com"
keys = []
"#,
        )
        .unwrap();
        assert!(cfg.validate().is_err());

        let cfg: AggConfig = toml::from_str(
            r#"
client_format = "auto"
[[channel]]
name = "a"
format = "anthropic_messages"
url = "https://a.example.com"
keys = ["k1", "k2"]
strategy = "weighted"
weights = [1]
"#,
        )
        .unwrap();
        let err = cfg.validate().unwrap_err();
        assert!(err.contains("weights"), "{err}");
    }

    #[test]
    fn full_config_roundtrip() {
        let cfg: AggConfig = toml::from_str(
            r#"
client_format = "auto"
[models]
"claude-sonnet" = "claude-sonnet-4-5"
[[channel]]
name = "official"
format = "anthropic_messages"
url = "https://api.anthropic.com"
keys = ["sk-1", "sk-2"]
[[channel]]
name = "relay"
format = "openai_chat"
url = "https://relay.example.com/v1/chat/completions"
keys = ["sk-r1"]
models = ["gpt-*"]
preserve_path = false
"#,
        )
        .unwrap();
        assert!(cfg.validate().is_ok());
        assert_eq!(cfg.channels.len(), 2);
        assert_eq!(
            cfg.models.get("claude-sonnet").unwrap(),
            "claude-sonnet-4-5"
        );
        assert_eq!(cfg.channels[0].format, WireFormat::AnthropicMessages);
        assert_eq!(cfg.channels[1].format, WireFormat::OpenAiChat);
    }
}
