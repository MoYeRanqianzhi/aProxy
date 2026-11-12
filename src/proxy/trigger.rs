//! 保活判定：客户端是否要流（Accept 与请求体顶层的 "stream": true）。

use super::*;

// ---------------------------------------------------------------------------
// 保活触发判定（`keepalive_trigger`）
//
// 真实 Claude Code 的流式主请求是 `Accept: application/json` + 请求体
// `"stream": true`（2026-10-04 实测），只看 Accept 判不出它是流式请求。请求体
// 判定只认 JSON 顶层对象的 `stream` 键为字面量 `true`——OpenAI / Anthropic 等
// 协议通用的流式开关，与 URL 无关。
// ---------------------------------------------------------------------------

/// serde 访问器：只读 JSON 顶层对象的 `stream` 键，其余一切值经 `IgnoredAny`
/// 跳过——**不**把请求体物化成 `serde_json::Value`，内存占用与请求体大小无关
/// （几十 MB 的上下文 + base64 图片也只是顺序扫一遍）。
///
/// `top_level = true` 解释整个请求体：只有对象才可能为真，其他形态（数组、
/// 字符串……）一律为假；`top_level = false` 解释 `stream` 键的值：只有字面量
/// `true` 为真（`"true"`、`1` 都不算——上游协议也不认它们）。重复键按最后一次
/// 出现为准，与主流 JSON 解析器（JS `JSON.parse`、Python `json`）一致。
pub(super) struct StreamFlagProbe {
    pub(super) top_level: bool,
}

impl<'de> serde::de::DeserializeSeed<'de> for StreamFlagProbe {
    type Value = bool;
    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<bool, D::Error> {
        d.deserialize_any(self)
    }
}

impl<'de> serde::de::Visitor<'de> for StreamFlagProbe {
    type Value = bool;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("任意 JSON 值")
    }

    fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<bool, E> {
        Ok(!self.top_level && v)
    }
    fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<bool, E> {
        Ok(false)
    }
    fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<bool, E> {
        Ok(false)
    }
    fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<bool, E> {
        Ok(false)
    }
    fn visit_str<E: serde::de::Error>(self, _: &str) -> Result<bool, E> {
        Ok(false)
    }
    fn visit_unit<E: serde::de::Error>(self) -> Result<bool, E> {
        Ok(false)
    }
    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<bool, A::Error> {
        while seq.next_element::<serde::de::IgnoredAny>()?.is_some() {}
        Ok(false)
    }
    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<bool, A::Error> {
        use serde::de::IgnoredAny;
        if !self.top_level {
            while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
            return Ok(false);
        }
        let mut stream = false;
        // 顶层键名逐个分配 String：顶层键只有寥寥几个，嵌套对象的键全走 IgnoredAny
        while let Some(key) = map.next_key::<String>()? {
            if key == "stream" {
                stream = map.next_value_seed(StreamFlagProbe { top_level: false })?;
            } else {
                map.next_value::<IgnoredAny>()?;
            }
        }
        Ok(stream)
    }
}

/// 以 `StreamFlagProbe` 扫完整个 JSON 文本；任何解析错误（非 JSON、截断、尾随
/// 垃圾）都按「不是流式请求」处理——这样的请求体上游本就不会当流式请求处理。
pub(super) fn json_requests_stream<'de, R: serde_json::de::Read<'de>>(
    mut de: serde_json::Deserializer<R>,
) -> bool {
    use serde::de::DeserializeSeed as _;
    matches!(
        StreamFlagProbe { top_level: true }.deserialize(&mut de),
        Ok(true)
    ) && de.end().is_ok()
}

/// 客户端原始请求体是否为顶层 `"stream": true` 的 JSON 对象。内存请求体
/// （≤ 1 MiB）就地扫描；溢写磁盘的大请求体在阻塞线程池里从文件流式读——
/// 几十 MB 的同步解析不能占住 async worker 线程。
pub(super) async fn request_body_wants_stream(body: &RequestBody) -> bool {
    match body {
        RequestBody::Memory(b) => json_requests_stream(serde_json::Deserializer::from_slice(b)),
        RequestBody::Disk { path, .. } => {
            let path = path.clone();
            tokio::task::spawn_blocking(move || {
                let Ok(file) = std::fs::File::open(&path) else {
                    return false;
                };
                json_requests_stream(serde_json::Deserializer::from_reader(
                    std::io::BufReader::with_capacity(64 * 1024, file),
                ))
            })
            .await
            .unwrap_or(false)
        }
    }
}

/// 按 `keepalive_trigger` 判定本请求是否「保活适用」（keepalive 开关与
/// forward_only 由调用方另行把关）。必须在请求转换之前调用：判定是客户端视角
/// 的——转换器改写 Accept 或请求体（如协议转换去掉 stream 字段）不应改变
/// 「客户端在等一个流」这一事实。
pub(super) async fn keepalive_triggered(
    trigger: crate::config::KeepaliveTrigger,
    headers: &HeaderMap,
    body: &RequestBody,
) -> bool {
    use crate::config::KeepaliveTrigger;
    let accept_sse = || {
        headers
            .get(http::header::ACCEPT)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|s| s.to_ascii_lowercase().contains("text/event-stream"))
    };
    match trigger {
        KeepaliveTrigger::Accept => accept_sse(),
        KeepaliveTrigger::BodyStream => request_body_wants_stream(body).await,
        // Accept 命中就不必扫请求体
        KeepaliveTrigger::Any => accept_sse() || request_body_wants_stream(body).await,
    }
}
