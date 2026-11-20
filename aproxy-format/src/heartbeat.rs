//! 心跳只看客户端的原始路径，不根据上游 URL 或共享进程中上一条请求的协议猜测。
use crate::config;
use aproxy_envelope as w;
use serde::Deserialize;
use std::collections::HashSet;

/// 同一实例可把多个入口明确映射到受支持的协议心跳；未命中的路径保留上一层字节。
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Heartbeat {
    /// 顺序不应改变匹配结果，因此加载时拒绝重复路径。
    #[serde(default)]
    preset: Vec<Preset>,
}
/// 路径和协议同时指定，防止把某一协议的事件送给另一种流解析器。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Preset {
    /// 与原请求去查询参数后的路径完全相等。
    path: String,
    /// 只接受有独立心跳事件、不需要伪造响应身份的协议。
    protocol: String,
    /// 提供方定义的事件名，不把任意配置字符串拼入 SSE 头。
    event: String,
}
/// Anthropic 把 ping 定义为独立事件，可以在消息身份尚未知时发出。
const PING: &str = "event: ping\ndata: {\"type\":\"ping\"}\n\n";
impl Heartbeat {
    /// 无配置使用标准 Messages 入口；显式配置只启用列出的路径。
    pub fn load(file: Option<config::Source<'_>>) -> Result<Self, &'static str> {
        let config: Self = match file {
            Some(path) => config::load(path)?,
            None => Self {
                preset: vec![Preset {
                    path: "/v1/messages".into(),
                    protocol: "anthropic_messages".into(),
                    event: "ping".into(),
                }],
            },
        };
        let mut paths = HashSet::new();
        for p in &config.preset {
            if !p.path.starts_with('/') || p.path.contains(['?', '#']) || !paths.insert(&p.path) {
                return Err("心跳路径无效或重复");
            }
            if (p.protocol.as_str(), p.event.as_str()) != ("anthropic_messages", "ping") {
                return Err("不支持此协议的心跳事件");
            }
        }
        Ok(config)
    }
    /// 省略 data 就是逐字节保持输入，未知协议不能插入虚构的流事件。
    pub fn beat(&self, h: &w::HeartbeatMessage, r: &mut w::Reply) {
        let path = h.client.path.split('?').next().unwrap_or_default();
        if self.preset.iter().any(|p| p.path == path) {
            r.data = w::Data::Text(PING.into());
        }
    }
}
/// 每条可发出的预设都必须以完整空行收尾，不包含模型输出或用量。
#[cfg(test)]
mod tests {
    use super::*;
    /// 检查事件名与载荷相符，而非只检查任意一种换行。
    #[test]
    fn ping_is_a_complete_no_content_event() {
        let event = PING.strip_suffix("\n\n").unwrap();
        let (name, body) = event.split_once("\ndata: ").unwrap();
        assert_eq!(name, "event: ping");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(body).unwrap(),
            serde_json::json!({"type":"ping"})
        );
    }
}
