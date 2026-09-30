//! switchyard-translation 接线：请求/响应协议转换。
//!
//! switchyard 是 sans-I/O 的纯转换库（无 HTTP 客户端依赖）；本模块把信封的
//! body 字节喂进 decode/encode，收回转换产物。转换方向全部由
//! (client_format, channel.format) 配置推导——不依赖跨进程状态。
//!
//! **流式（SSE）v1 划界**：SSE 响应直通原样不做协议转换——switchyard 的
//! 流 API 产出 wire 事件对象（需按目标协议自行分帧，event 名与 [DONE] 终态
//! 等帧格式是协议特定语义），v1 不引入这层分帧。非流式请求/响应转换完整可用。

use serde_json::Value;
use switchyard_translation::WireFormat;

use crate::detect;

/// 请求转换：`from` 格式的请求体 → `to` 格式。格式相同则原样返回。
/// 错误统一为人类可读文案（sync helpers 的 TranslationError 直接 Display）。
pub fn translate_request(from: WireFormat, to: WireFormat, body: &Value) -> Result<Value, String> {
    if from == to {
        return Ok(body.clone());
    }
    let req = switchyard_translation::decode_request(from, body).map_err(|e| e.to_string())?;
    switchyard_translation::encode_request(&req, to).map_err(|e| e.to_string())
}

/// 非流式响应转换：`from` 格式的缓冲响应 → `to` 格式。格式相同则原样返回。
pub fn translate_response(from: WireFormat, to: WireFormat, body: &Value) -> Result<Value, String> {
    if from == to {
        return Ok(body.clone());
    }
    let agg = switchyard_translation::decode_aggregated_response(body, from)
        .map_err(|e| e.to_string())?;
    switchyard_translation::encode_aggregated_response(&agg, to, None).map_err(|e| e.to_string())
}

/// 请求协议解析：显式声明优先，auto 时按 body 形态检测（不猜——None 由调用
/// 方走 error 行）。
pub fn resolve_client_format(
    client_format: crate::config::ClientFormat,
    body: &Value,
) -> Option<WireFormat> {
    match client_format {
        crate::config::ClientFormat::Explicit(f) => Some(f),
        crate::config::ClientFormat::Auto => detect::detect_request(body),
    }
}
