//! aproxy 外部转换器的信封契约：aproxy 与 format 程序之间的唯一接口。
//!
//! 一次转换 = 一行 JSON 信封（stdin 进 stdout 出）。aproxy 把请求（或上游响应）
//! 装进信封写给 format 进程，format 改写后写一行信封回来。本 crate 是两个
//! 二进制（aproxy 与 aproxy-format）的共同契约——单源维护，漂移会在编译期暴露。
//!
//! 协议要点（写 format 程序的完整指南见 aproxy-format skill）：
//! - 单行进出：JSON 序列化天然转义换行，信封内的 body 再长也不会破帧
//! - `body` 与 `body_b64` 互斥（[`TransformEnvelope::from_line`] 校验）
//! - `error` 字段非空 = format 表达「该请求转换失败」（进程不崩，exit 0）
//! - 非零 exit code = 进程级失败（崩溃/挂死），由 aproxy 侧按失败语义处理

use std::collections::BTreeMap;

use base64::Engine;
use serde::{Deserialize, Serialize};

/// 单请求转换信封：请求侧与响应侧同构（一行 JSON，stdin 进 stdout 出）。
///
/// 请求侧与响应侧的差异在字段的**用法**而非结构：
/// - 请求侧：`url` 是 aproxy 计算的上游地址，format 改写它即实现协议转换的
///   路径/域名迁移（多渠道聚合的核心机制）；输出缺省 `url` = 维持原地址。
/// - 响应侧：输入信封的 `url` 是请求侧最终发往上游的地址（format 按它反查
///   渠道表——响应转换器与请求转换器是不同进程，无共享状态，这是设计铁律）；
///   响应不发往 `url`，纯作标识。输出缺省 `method` 无意义（响应侧忽略）。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct TransformEnvelope {
    /// 请求侧：转换后的上游 URL（format 可改写）。
    /// None/缺省 = 维持 aproxy 计算的原始 target_url。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// HTTP 方法原样透传；format 输出缺省 = aproxy 沿用原方法。
    /// （多数 format 只回传 body/headers/url，不回传 method——缺省必须合法。）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    /// HTTP 头整表进出。键为 HTTP 小写规范名（HeaderMap 迭代即小写）；
    /// BTreeMap 提供稳定输出序（HTTP 头顺序无语义）。
    /// hop-by-hop 头与 content-length 不进信封（由 aproxy 按实际字节回填）。
    /// format 改写整个表即支持多 key 轮换等场景。
    pub headers: BTreeMap<String, String>,
    /// UTF-8 body 文本。与 `body_b64` 互斥；两者都缺 = 空 body。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// 非 UTF-8 body 的 base64（标准字母表、无换行）携带。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_b64: Option<String>,
    /// 池槽位号（0..pool_max）；spawn 一次性模式恒 0。
    /// 轮换类 format 可用它做每 worker 起始偏移（如 `worker_id % key数`），
    /// 避免池内各 worker 轮换起点重合。
    #[serde(default)]
    pub worker_id: u32,
    /// aproxy 配置 transform 的 `extra` 原样透传（格式无要求，format 自解）。
    #[serde(default)]
    pub extra: String,
    /// 输出侧：format 表达「该请求转换失败」的人类可读原因。
    /// 非空 error = 该请求转换失败（进程必须仍 exit 0，否则按进程失败处理）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// 信封解析/取值错误。
#[derive(Debug)]
pub enum EnvelopeError {
    /// JSON 行解析失败（空行、非 JSON、截断）。
    Json(serde_json::Error),
    /// `body` 与 `body_b64` 同时出现（互斥校验）。
    BodyFieldsConflict,
    /// `body_b64` 解码失败。
    Base64(base64::DecodeError),
}

impl std::fmt::Display for EnvelopeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EnvelopeError::Json(e) => write!(f, "信封 JSON 解析失败: {e}"),
            EnvelopeError::BodyFieldsConflict => {
                write!(f, "body 与 body_b64 互斥，不能同时出现")
            }
            EnvelopeError::Base64(e) => write!(f, "body_b64 解码失败: {e}"),
        }
    }
}

impl std::error::Error for EnvelopeError {}

impl TransformEnvelope {
    /// body 字节视图：`body`（UTF-8 文本）优先，其次 `body_b64` 解码；均缺 = 空。
    pub fn body_bytes(&self) -> Result<Vec<u8>, EnvelopeError> {
        if let Some(b) = &self.body {
            return Ok(b.clone().into_bytes());
        }
        match &self.body_b64 {
            Some(b64) => base64::engine::general_purpose::STANDARD
                .decode(b64)
                .map_err(EnvelopeError::Base64),
            None => Ok(Vec::new()),
        }
    }

    /// 从 body 字节构造信封：合法 UTF-8 走 `body`，否则 `body_b64`。
    /// method/headers/url 由调用方按需补充（缺省合法）。
    pub fn from_body_bytes(bytes: &[u8]) -> Self {
        match std::str::from_utf8(bytes) {
            Ok(text) => TransformEnvelope {
                body: Some(text.to_string()),
                ..Default::default()
            },
            Err(_) => TransformEnvelope {
                body_b64: Some(base64::engine::general_purpose::STANDARD.encode(bytes)),
                ..Default::default()
            },
        }
    }

    /// 序列化为一行（不含换行符，调用方写 stdin 时补 `\n`）。
    pub fn to_line(&self) -> serde_json::Result<String> {
        serde_json::to_string(self)
    }

    /// 从一行反序列化；`body`/`body_b64` 同时出现 → Err（互斥校验）。
    /// 空行/空白行同样 Err——调用方（池与 format 循环）应把空行当协议错误。
    pub fn from_line(line: &str) -> Result<Self, EnvelopeError> {
        let env: TransformEnvelope = serde_json::from_str(line).map_err(EnvelopeError::Json)?;
        if env.body.is_some() && env.body_b64.is_some() {
            return Err(EnvelopeError::BodyFieldsConflict);
        }
        Ok(env)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_preserves_all_fields() {
        let mut env = TransformEnvelope {
            url: Some("https://up.example.com/v1/messages".to_string()),
            method: Some("POST".to_string()),
            worker_id: 2,
            extra: "{\"keys\":[\"a\"]}".to_string(),
            ..Default::default()
        };
        env.headers
            .insert("authorization".to_string(), "Bearer k1".to_string());
        env.headers
            .insert("content-type".to_string(), "application/json".to_string());
        env.body = Some("{\"model\":\"m\"}".to_string());

        let line = env.to_line().unwrap();
        let back = TransformEnvelope::from_line(&line).unwrap();
        assert_eq!(env, back);
    }

    #[test]
    fn body_and_body_b64_conflict_rejected() {
        let line = r#"{"method":"POST","headers":{},"body":"x","body_b64":"eA=="}"#;
        assert!(matches!(
            TransformEnvelope::from_line(line),
            Err(EnvelopeError::BodyFieldsConflict)
        ));
    }

    #[test]
    fn non_utf8_body_carries_b64_faithfully() {
        // gzip magic 头两字节：非法 UTF-8，必须走 body_b64
        let bytes: &[u8] = &[0x1f, 0x8b, 0x00, 0xff];
        let env = TransformEnvelope::from_body_bytes(bytes);
        assert!(env.body.is_none());
        assert!(env.body_b64.is_some());
        assert_eq!(env.body_bytes().unwrap(), bytes);
    }

    #[test]
    fn utf8_body_stays_text() {
        let env = TransformEnvelope::from_body_bytes(b"{\"ok\":1}");
        assert_eq!(env.body.as_deref(), Some("{\"ok\":1}"));
        assert!(env.body_b64.is_none());
        assert_eq!(env.body_bytes().unwrap(), b"{\"ok\":1}");
    }

    #[test]
    fn empty_and_bad_lines_error() {
        assert!(TransformEnvelope::from_line("").is_err());
        assert!(TransformEnvelope::from_line("   ").is_err());
        assert!(TransformEnvelope::from_line("{not json").is_err());
    }

    #[test]
    fn minimal_line_defaults_all_optionals() {
        // format 只回 body 是合法输出：method/headers 缺省合法
        let back = TransformEnvelope::from_line(r#"{"headers":{},"body":"y"}"#).unwrap();
        assert_eq!(back.method, None);
        assert_eq!(back.worker_id, 0);
        assert_eq!(back.extra, "");
        assert_eq!(back.url, None);
    }

    #[test]
    fn error_field_roundtrips() {
        let line = r#"{"headers":{},"error":"model 未命中任何渠道"}"#;
        let back = TransformEnvelope::from_line(line).unwrap();
        assert_eq!(back.error.as_deref(), Some("model 未命中任何渠道"));
        // 序列化时 None 字段不输出（信封保持精简）
        let env = TransformEnvelope::default();
        assert!(!env.to_line().unwrap().contains("error"));
    }
}
