//! Text for control-channel failures. The instance always selects English; a
//! terminal can render known refusal parameters without trusting the peer's prose.
use super::Lang;

crate::message!(reply_format(error: &str), en: "Invalid reply format: {error}", zh: "应答格式不符：{error}");

crate::message!(pid_mismatch(claimed: u32, peer: u32), en: "Reply claims pid {claimed}, but the connection peer is pid {peer}", zh: "应答者自报 pid {claimed} 与连接对端进程 {peer} 不一致");

crate::message!(timeout(endpoint: &str), en: "{endpoint}: request timed out", zh: "{endpoint}: 请求超时");

crate::message!(silence(), en: "No subscription events received for 45 seconds", zh: "订阅连接 45 秒没有收到任何事件");

crate::message!(event_json(error: &str), en: "Event is not JSON: {error}", zh: "事件不是 JSON：{error}");

crate::message!(closed(), en: "Connection closed before the reply was complete", zh: "连接在应答完整之前关闭");

crate::message!(reply_json(error: &str), en: "Reply is not JSON: {error}", zh: "应答不是 JSON：{error}");

crate::message!(version(v: u64, supported: u32), en: "The instance uses control protocol v{v}; this program supports v{supported}. Use the same aProxy version as the instance.", zh: "实例使用控制协议 v{v}，本程序只懂 v{supported}：请用与实例同版本的 aproxy 操作它");

crate::message!(no_version(), en: "Reply has no protocol version v", zh: "应答没有协议版本 v");

crate::message!(no_result(), en: "Reply has neither result nor error", zh: "应答缺少 result 或 error");

crate::message!(oversize(), en: "Reply exceeds the length limit", zh: "应答超过长度上限");

crate::message!(connect(error: &str), en: "Cannot connect ({error})", zh: "无法连接（{error}）");

crate::message!(foreign_owner(server: u32), en: "Control endpoint belongs to another user (uid {server}); its reply is not trusted", zh: "控制端点上的进程属于另一个用户（uid {server}），不予采信");

crate::message!(identity(), en: "invalid request identity", zh: "请求身份无效");

crate::message!(unverified_refusal(), en: "cannot verify subscription refusal", zh: "无法核验订阅拒绝应答");

crate::message!(subscription_ack(), en: "invalid subscription acknowledgment", zh: "订阅确认应答无效");

crate::message!(peer_pid(), en: "cannot verify peer pid", zh: "无法核验对端进程的 pid");

crate::message!(pipe_owner(), en: "cannot verify pipe server owner", zh: "无法核验管道服务端的属主");

crate::message!(run_changed(), en: "instance run changed", zh: "实例已重新启动，不再是原来那次运行");

crate::message!(refused(code: &str, message: &str), en: "Instance refused the request ({code}): {message}", zh: "实例拒绝了请求（{code}）：{message}");

crate::message!(incomplete(), en: "the request line is too long or incomplete", zh: "请求行太长或不完整");

crate::message!(not_json(), en: "the request is not JSON", zh: "请求不是 JSON");

crate::message!(missing_v(), en: "the request has no v", zh: "请求缺少 v");

crate::message!(missing_op(), en: "the request has no op", zh: "请求缺少 op");

crate::message!(args_object(), en: "args must be an object", zh: "args 必须为对象");

crate::message!(required(arg: &str), en: "arg {arg} is required", zh: "缺少必填参数 {arg}");

crate::message!(invalid(arg: &str), en: "arg {arg} has the wrong type or value", zh: "参数 {arg} 的类型或值不正确");

crate::message!(unsupported_version(version: &str, supported: &str), en: "unsupported protocol version {version}; this instance supports {supported}", zh: "不支持协议版本 {version}；此实例支持 {supported}");

crate::message!(control_error(reason:&str), en: "format control failed: {reason}", zh: "format 控制调用失败：{reason}");
crate::message!(unknown_format(name: &str), en: "no configured format named {name}", zh: "没有名为 {name} 的 format 条目");
crate::message!(unknown_op(op: &str), en: "unknown op: {op}", zh: "未知操作：{op}");

crate::message!(unknown_arg(args: &str), en: "unknown arg: {args}", zh: "未知参数：{args}");

crate::message!(restart_in_progress(), en: "a selected format is already restarting", zh: "所选 format 已在重启");
crate::message!(invalid_state(state: &str), en: "not allowed while the instance is {state}", zh: "实例处于 {state} 状态时不允许此操作");

crate::message!(idle_condition(idle_ms: u64), en: "the instance has been idle for only {idle_ms} ms", zh: "实例只闲置了 {idle_ms} 毫秒");

crate::message!(not_found(request_id: &str, reason: &str), en: "nothing to return for request {request_id}: {reason}", zh: "请求 {request_id} 没有可返回的数据：{reason}");

crate::message!(busy(limit: u64), en: "all {limit} streaming connections are in use", zh: "{limit} 条流式连接已全部占用");

/// Only these arguments participate in rendering. Ignore extension fields instead of
/// copying an entire peer object to disk. Unrecognized data has no private projection.
pub(crate) fn fields(code: &str, d: &serde_json::Value) -> Option<&'static [&'static str]> {
    use crate::wire::error_code::*;
    fn token(v: &serde_json::Value) -> Option<&str> {
        v.as_str().filter(|s| {
            !s.is_empty()
                && s.len() <= 128
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-.".contains(&b))
        })
    }
    Some(match code {
        BAD_REQUEST => match d["reason"].as_str()? {
            "incomplete_line" | "not_json" | "missing_v" | "missing_op" | "args_not_object"
                if d.get("arg").is_none() =>
            {
                &["reason"]
            }
            "missing_arg" | "invalid_arg"
                if matches!(
                    d["arg"].as_str()?,
                    "request_id"
                        | "offset"
                        | "limit"
                        | "before"
                        | "part"
                        | "attempt"
                        | "keep_restore"
                        | "if_idle_secs"
                        | "name"
                        | "params"
                        | "method"
                        | "names"
                        | "ttl_secs"
                ) =>
            {
                &["reason", "arg"]
            }
            _ => return None,
        },
        UNSUPPORTED_VERSION
            if d["supported"].as_array().is_some_and(|a| {
                !a.is_empty()
                    && a.len() <= 16
                    && a.iter().all(|n| {
                        n.as_u64()
                            .is_some_and(|v| (1..=u32::MAX as u64).contains(&v))
                    })
            }) =>
        {
            &["supported"]
        }
        "format_control_error"
            if matches!(
                d["reason"].as_str()?,
                "no-control" | "busy" | "timeout" | "protocol" | "too-large" | "spawn"
            ) =>
        {
            &["reason"]
        }
        "unknown_format"
            if d["name"]
                .as_str()
                .is_some_and(|s| s.len() <= 64 && !s.chars().any(char::is_control)) =>
        {
            &["name"]
        }
        UNKNOWN_OP if token(&d["op"]).is_some() => &["op"],
        UNSUPPORTED_ARG
            if d["args"].as_array().is_some_and(|a| {
                !a.is_empty() && a.len() <= 32 && a.iter().all(|v| token(v).is_some())
            }) =>
        {
            &["args"]
        }
        INVALID_STATE if matches!(d["state"].as_str()?, "starting" | "stopping") => &["state"],
        CONDITION_NOT_MET if d["idle_ms"].is_u64() => &["idle_ms"],
        NOT_FOUND
            if token(&d["request_id"]).is_some()
                && matches!(
                    d["reason"].as_str()?,
                    "not_retained" | "unknown_request" | "evicted" | "no_such_part"
                ) =>
        {
            &["request_id", "reason"]
        }
        BUSY if d["limit"]
            .as_u64()
            .is_some_and(|v| (1..=u32::MAX as u64).contains(&v)) =>
        {
            &["limit"]
        }
        _ => return None,
    })
}

/// Reconstruct only known details. The request version is local to this client;
/// the peer advertises its supported versions, not the one it was sent.
pub(crate) fn remote(lang: Lang, code: &str, details: &serde_json::Value) -> Option<String> {
    use crate::wire::{IPC_VERSION, error_code};
    fields(code, details)?;
    Some(match code {
        error_code::BAD_REQUEST => match details["reason"].as_str()? {
            "incomplete_line" => incomplete(lang).into(),
            "not_json" => not_json(lang).into(),
            "missing_v" => missing_v(lang).into(),
            "missing_op" => missing_op(lang).into(),
            "args_not_object" => args_object(lang).into(),
            "missing_arg" => required(lang, details["arg"].as_str()?),
            "invalid_arg" => invalid(lang, details["arg"].as_str()?),
            _ => return None,
        },
        error_code::UNSUPPORTED_VERSION => {
            let supported = details["supported"]
                .as_array()?
                .iter()
                .map(|v| Some(v.as_u64()?.to_string()))
                .collect::<Option<Vec<_>>>()?;
            if supported.is_empty() {
                return None;
            }
            unsupported_version(lang, &IPC_VERSION.to_string(), &supported.join(", "))
        }
        "format_control_error" => control_error(lang, details["reason"].as_str()?),
        "unknown_format" => unknown_format(lang, details["name"].as_str()?),
        error_code::UNKNOWN_OP => unknown_op(lang, details["op"].as_str()?),
        error_code::UNSUPPORTED_ARG => unknown_arg(
            lang,
            &details["args"]
                .as_array()?
                .iter()
                .map(serde_json::Value::as_str)
                .collect::<Option<Vec<_>>>()?
                .join(", "),
        ),
        error_code::INVALID_STATE if details["reason"] == "restart-in-progress" => {
            restart_in_progress(lang).to_owned()
        }
        error_code::INVALID_STATE => invalid_state(lang, details["state"].as_str()?),
        error_code::CONDITION_NOT_MET => idle_condition(lang, details["idle_ms"].as_u64()?),
        error_code::NOT_FOUND => not_found(
            lang,
            details["request_id"].as_str()?,
            details["reason"].as_str()?,
        ),
        error_code::BUSY => busy(lang, details["limit"].as_u64()?),
        _ => return None,
    })
}
