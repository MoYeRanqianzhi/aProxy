//! 代理按英文写入 HTTP、SSE 和快照；清点时只从已脱敏的公开参数还原有足够证据的说明。
use crate::transform::fault_text as fault;
use aproxy_ipc::{i18n::Lang, message, wire::LastError};
use std::borrow::Cow;

message!(connect(reason: &str), en: "could not connect to the upstream: {reason}", zh: "无法连接上游：{reason}");

message!(connect_timeout(timeout_ms: u64), en: "connecting to the upstream timed out after {timeout_ms} ms", zh: "连接上游在 {timeout_ms} 毫秒后超时");

message!(first_byte(timeout_ms: u64), en: "no response headers from the upstream within {timeout_ms} ms", zh: "等待 {timeout_ms} 毫秒仍未收到上游响应头");

message!(idle(timeout_ms: u64, received_bytes: u64), en: "the upstream sent nothing for {timeout_ms} ms after {received_bytes} bytes of the response body", zh: "已收到 {received_bytes} 字节响应体，上游连续 {timeout_ms} 毫秒没有再发送数据");

message!(interrupted(received_bytes: u64, reason: &str), en: "the upstream connection broke after {received_bytes} bytes of the response body: {reason}", zh: "上游连接在收到 {received_bytes} 字节响应体后中断：{reason}");

message!(status(code: u16), en: "the upstream returned {code}", zh: "上游返回 {code}");

message!(error_body(code: u16), en: "the upstream returned {code} with an error in the body", zh: "上游返回 {code}，响应体报告错误");

message!(host(host: &str), en: "the Host {host:?} is not allowed; add it (without the port) to allowed_hosts in config.toml, or set allowed_hosts = [\"*\"] to turn the check off", zh: "不允许 Host {host:?}；请在 config.toml 的 allowed_hosts 中加入主机名（不带端口），或以 allowed_hosts = [\"*\"] 关闭检查");

message!(origin(origin: &str), en: "requests from the web page origin {origin:?} are not allowed; add it to allowed_origins in config.toml, or set allowed_origins = [\"*\"] to turn the check off", zh: "不允许来自网页源 {origin:?} 的请求；请将它加入 config.toml 的 allowed_origins，或以 allowed_origins = [\"*\"] 关闭检查");

message!(request_size(limit_bytes: u64), en: "the request body exceeds the limit of {limit_bytes} bytes; raise max_request_mb in config.toml", zh: "请求体超过 {limit_bytes} 字节；可在 config.toml 中调大 max_request_mb");

message!(response_size(limit_bytes: u64), en: "the upstream response exceeds the limit of {limit_bytes} bytes; raise max_response_mb in config.toml", zh: "上游响应超过 {limit_bytes} 字节；可在 config.toml 中调大 max_response_mb");

message!(aborted(), en: "reading the request body failed: the client stopped sending", zh: "读取请求体失败：客户端已停止发送");

message!(upgrade(upgrade: &str), en: "connection upgrades ({upgrade}) are not supported", zh: "不支持连接升级（{upgrade}）");

message!(decode(encoding: &str), en: "the upstream response used the content-encoding {encoding:?}, which the already committed stream cannot carry, and decoding it failed", zh: "上游响应使用了 {encoding:?} 编码，已提交的流不能携带该编码，解码也未成功");

message!(decision(name: &str), en: "format '{name}' rejected the successful attempt", zh: "格式程序 {name} 拒绝了本次成功的尝试");

message!(format_error(safe:&str,e:&str), en:"format '{safe}': {e}", zh:"格式程序 {safe}：{e}");

/// 完整的英文必须和快照一致；旧版本、未知分类、截断的证据和没有保留全部参数的终态照录原文。
/// 例如磁盘故障的 details 只记录操作不记录系统报错；重排它会丢掉诊断。
pub fn recent(error: &LastError, lang: Lang) -> Cow<'_, str> {
    if lang == Lang::En {
        return Cow::Borrowed(&error.message);
    }
    let render = |lang| {
        let d = &error.details;
        Some(match error.code.as_str() {
            "upstream_connect_failed" => connect(lang, d["reason"].as_str()?),
            "upstream_connect_timeout" => connect_timeout(lang, d["timeout_ms"].as_u64()?),
            "upstream_first_byte_timeout" => first_byte(lang, d["timeout_ms"].as_u64()?),
            "upstream_read_idle_timeout" => idle(
                lang,
                d["timeout_ms"].as_u64()?,
                d["received_bytes"].as_u64()?,
            ),
            "upstream_body_interrupted" => {
                interrupted(lang, d["received_bytes"].as_u64()?, d["reason"].as_str()?)
            }
            "upstream_status" => status(lang, d["status"].as_u64()?.try_into().ok()?),
            "upstream_error_body" => error_body(lang, d["status"].as_u64()?.try_into().ok()?),
            "inbound_host_rejected" => host(lang, d["host"].as_str()?),
            "inbound_origin_rejected" => origin(lang, d["origin"].as_str()?),
            "request_too_large" => request_size(lang, d["limit_bytes"].as_u64()?),
            "response_too_large" => response_size(lang, d["limit_bytes"].as_u64()?),
            "client_body_aborted" => aborted(lang).to_owned(),
            "upgrade_unsupported" => upgrade(lang, d["upgrade"].as_str()?),
            "decode_failed" => decode(lang, d["encoding"].as_str()?),
            "format_decision" => decision(lang, d["format"].as_str()?),
            "format_error" | "format_failed" => {
                let fault = match d["kind"].as_str()? {
                    "open" => fault::open,
                    "spawn" => fault::spawn,
                    "io" => fault::io,
                    "died" => fault::died,
                    "timeout" => fault::timeout,
                    "protocol" => fault::protocol,
                    "invalid_output" => fault::invalid,
                    _ => return None,
                };
                format_error(lang, d["format"].as_str()?, fault(lang))
            }
            _ => return None,
        })
    };
    if render(Lang::En).as_deref() == Some(error.message.as_str())
        && let Some(s) = render(lang)
    {
        return Cow::Owned(s.chars().take(200).collect());
    }
    Cow::Borrowed(&error.message)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reconstruct_only_complete_matching_evidence() {
        let mut e = LastError {
            code: "upstream_status".into(),
            details: serde_json::json!({"status":503}),
            message: "the upstream returned 503".into(),
            at: 1,
        };
        assert_eq!(recent(&e, Lang::En), e.message);
        assert_eq!(recent(&e, Lang::Zh), "上游返回 503");
        e.message = "old writer evidence".into();
        assert_eq!(recent(&e, Lang::Zh), e.message);
        e.details = serde_json::json!({"status": "secret input must not appear"});
        assert_eq!(recent(&e, Lang::Zh), e.message);
        e.code = "future".into();
        assert_eq!(recent(&e, Lang::Zh), e.message);
    }
}
