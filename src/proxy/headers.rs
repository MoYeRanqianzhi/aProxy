//! 请求与响应头：hop-by-hop 过滤、api_key / extra_headers / override_headers 注入。

use super::*;

/// 需要过滤的 hop-by-hop 头，避免透传导致协议错误或与 hyper/reqwest 的
/// 自动管理（content-length / transfer-encoding / host / connection）冲突。
pub(crate) const HOP_HEADERS: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "host",
    "content-length",
];

pub(crate) fn is_hop_header(name: &str) -> bool {
    HOP_HEADERS.contains(&name.to_ascii_lowercase().as_str())
}

/// 响应侧的 hop-by-hop 过滤：同 `is_hop_header`，但**在上游未用
/// transfer-encoding 分帧时排除 content-length**（即把它保留下来）。
///
/// 保留的理由与磁盘回放分支显式回填 content-length 完全一致：仅转发模式转发的
/// 字节未经任何变换（不解码、不重写、不拼接），上游声明的长度仍然精确——剥掉它
/// 只会让本可定长的响应退化为 chunked，白白丢掉客户端的长度可见性。
/// 请求侧不能照此办理：那侧的 content-length 描述的是客户端发来的字节，
/// 而 hyper/reqwest 会按实际发送的 body 自行管理，必须交由它们决定。
///
/// `upstream_has_te` 必须取自**上游原始响应头**是否含 transfer-encoding。
/// 「上游声明的长度仍然精确」这条前提在 CL 与 TE 并存时不成立（协议违规但现实
/// 中存在）：hyper 客户端会保留 CL 头、改按 TE 分帧，于是我们手里的 CL 只是
/// 上游的一句声明，与实际响应字节数可能不符。后果最坏的那种是**静默**的——
/// CL 声称的字节数大于实际时，hyper 服务端按 CL 分帧、把缺额一直等下去，
/// 客户端拿不到响应却毫无线索（RFC 7230 §3.3.3 因此要求：带 TE 的消息转发前
/// 必须移除收到的 content-length，交由接收方重新分帧）。
pub(super) fn is_hop_response_header(name: &str, upstream_has_te: bool) -> bool {
    if name.eq_ignore_ascii_case("content-length") {
        return upstream_has_te;
    }
    is_hop_header(name)
}

/// 将配置中的头覆盖/追加逻辑应用到待发往上游的 HeaderMap（大小写不敏感判定）。
pub(super) fn apply_header_overrides(headers: &mut HeaderMap, config: &Config) {
    // api_key 快捷：等效覆盖 Authorization: Bearer <key>（大小写不敏感覆盖）。
    // 同时覆盖 x-api-key（Anthropic 风格上游使用该头携带原始 key，若只覆盖
    // Authorization，客户端原带的 x-api-key 会原样漏到上游造成鉴权混乱）。
    if let Some(key) = config.api_key.as_deref() {
        let val = format!("Bearer {key}");
        if let Ok(v) = HeaderValue::from_str(&val) {
            headers.remove(http::header::AUTHORIZATION);
            headers.insert(http::header::AUTHORIZATION, v);
        }
        if let Ok(raw) = HeaderValue::from_str(key) {
            headers.remove("x-api-key");
            headers.insert(HeaderName::from_static("x-api-key"), raw);
        }
    }

    // override_headers：无条件覆盖
    for (k, v) in &config.override_headers {
        let Ok(name) = HeaderName::from_bytes(k.as_bytes()) else {
            continue;
        };
        let Ok(val) = HeaderValue::from_str(v) else {
            continue;
        };
        headers.remove(&name);
        headers.insert(name, val);
    }

    // extra_headers：仅当未携带时追加
    for (k, v) in &config.extra_headers {
        let Ok(name) = HeaderName::from_bytes(k.as_bytes()) else {
            continue;
        };
        if headers.contains_key(&name) {
            continue;
        }
        let Ok(val) = HeaderValue::from_str(v) else {
            continue;
        };
        headers.insert(name, val);
    }
}
