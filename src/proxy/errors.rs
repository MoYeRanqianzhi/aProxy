//! 数据面的错误出口：aproxy 自己结束一个请求时说的话，只在这里构造。
//!
//! 三样东西，都以码表（`aproxy_ipc::code`）里的码为准：
//! - [`AttemptFailure`]：一次上游尝试没有成功的原因（尝试失败码）。默认全部重试；它也是
//!   终态里 `last_code` 的来源。
//! - [`Terminal`]：一个请求的终态（数据面终态码）。响应头还没写出时是一个 JSON 错误响应，
//!   已经写出时是 SSE 的 `event: error`——两者是同一个 JSON，都由 [`Terminal::payload`]
//!   经 serde 序列化，源码里只有这一处构造 `event: error`。
//! - [`mark_given_up`]：aproxy 放弃之后原样透传的上游响应，体保真，只加两个头。
//!
//! 两个头：`x-aproxy-request-id` 与 `x-aproxy-error`。带 `x-aproxy-error` 的响应就是 aproxy
//! 已经放弃、不会再重试的最终结果；正常透传的响应不带。

use super::*;
use crate::history::Recorder;
use serde_json::{Map, Value, json};

/// 响应头：这个请求的标识，与错误体、日志里的 `request_id` 相同
pub(super) const REQUEST_ID_HEADER: &str = "x-aproxy-request-id";
/// 响应头：aproxy 结束这个请求所用的终态码
pub(super) const ERROR_HEADER: &str = "x-aproxy-error";

/// 一个请求在数据面上的身份：标识，以及到目前为止已经进行的上游尝试数。终态的错误体
/// 两样都要带。
#[derive(Debug, Clone)]
pub(super) struct RequestTrace {
    /// 请求标识 `<instance_id>-<序号>`，对外是不透明的串
    pub(super) request_id: Arc<str>,
    /// 已进行的上游尝试数（还没发往上游就结束的请求为 0）
    pub(super) attempts: u32,
    /// 只有 decide 链才计请求总耗时；在开始读取客户端的上传之前取单调时钟。
    pub(super) decide_started: Option<tokio::time::Instant>,
    /// 只让实际持有 forward 响应体的读者参与收尾，弱引用不会延长尝试的生命。
    pub(super) upstream_join: std::sync::Weak<UpstreamJoin>,
    /// 这个请求在请求历史里的记录；不进历史的请求（入站校验拒掉的）是空句柄
    pub(super) rec: Recorder,
}

impl RequestTrace {
    /// 不进请求历史的请求。
    pub(super) fn new(request_id: impl Into<Arc<str>>) -> Self {
        Self::recorded(request_id, Recorder::none())
    }

    /// 在请求历史里有记录的请求。
    pub(super) fn recorded(request_id: impl Into<Arc<str>>, rec: Recorder) -> Self {
        Self {
            request_id: request_id.into(),
            attempts: 0,
            decide_started: None,
            upstream_join: Default::default(),
            rec,
        }
    }
}

/// 一次上游尝试没有成功的原因。
#[derive(Debug, Clone)]
pub(super) struct AttemptFailure {
    /// 尝试失败码（码表里 attempt 族的名字）
    pub(super) code: &'static str,
    /// 码表登记的 `details`
    pub(super) details: Map<String, Value>,
    /// 一句英文说明，进日志与实例的「最近错误」
    pub(super) message: String,
    /// 上游给了响应时的状态码
    pub(super) status: Option<u16>,
}

impl AttemptFailure {
    pub(super) fn new(code: &'static str, message: String, details: Value) -> Self {
        debug_assert!(
            aproxy_ipc::code::lookup(code).is_some(),
            "unregistered code: {code}"
        );
        Self {
            code,
            details: as_map(details),
            message,
            status: None,
        }
    }

    /// 失败之前已经收到的响应体字节数：响应体读到一半断开、或读着读着超时的失败带着它
    /// （`details.received_bytes`），别的失败是 0。
    pub(super) fn received_bytes(&self) -> u64 {
        self.details
            .get("received_bytes")
            .and_then(Value::as_u64)
            .unwrap_or(0)
    }

    /// 请求历史要的那几样。
    pub(super) fn for_history(&self) -> crate::history::Failure<'_> {
        crate::history::Failure {
            code: self.code,
            details: &self.details,
            message: &self.message,
            has_response: self.status.is_some(),
        }
    }

    /// 连不上上游：DNS、被拒绝、TLS。`reason` 是上游库给的原文，已去 URL、已打码
    pub(super) fn connect_failed(reason: String) -> Self {
        Self::new(
            "upstream_connect_failed",
            format!("could not connect to the upstream: {reason}"),
            json!({ "reason": reason }),
        )
    }

    /// 连接上游超时
    pub(super) fn connect_timeout(timeout: Duration) -> Self {
        let timeout_ms = millis(timeout);
        Self::new(
            "upstream_connect_timeout",
            format!("connecting to the upstream timed out after {timeout_ms} ms"),
            json!({ "timeout_ms": timeout_ms }),
        )
    }

    /// 请求已发出，等不到响应头
    pub(super) fn first_byte_timeout(timeout: Duration) -> Self {
        let timeout_ms = millis(timeout);
        Self::new(
            "upstream_first_byte_timeout",
            format!("no response headers from the upstream within {timeout_ms} ms"),
            json!({ "timeout_ms": timeout_ms }),
        )
    }

    /// 两次读到响应体数据之间超时
    pub(super) fn read_idle_timeout(timeout: Duration, received_bytes: u64) -> Self {
        let timeout_ms = millis(timeout);
        Self::new(
            "upstream_read_idle_timeout",
            format!(
                "the upstream sent nothing for {timeout_ms} ms after {received_bytes} bytes of the response body"
            ),
            json!({ "timeout_ms": timeout_ms, "received_bytes": received_bytes }),
        )
    }

    /// 连接建立之后断开：响应头之前（`received_bytes` 为 0）或响应体中途
    pub(super) fn body_interrupted(received_bytes: u64, reason: String) -> Self {
        Self::new(
            "upstream_body_interrupted",
            format!(
                "the upstream connection broke after {received_bytes} bytes of the response body: {reason}"
            ),
            json!({ "received_bytes": received_bytes, "reason": reason }),
        )
    }

    /// 上游回了一个算失败的响应：4xx / 5xx 记 `upstream_status`，2xx 但体是错误记
    /// `upstream_error_body`
    pub(super) fn response(status: StatusCode) -> Self {
        let code = status.as_u16();
        let mut failure = if status.is_client_error() || status.is_server_error() {
            Self::new(
                "upstream_status",
                format!("the upstream returned {code}"),
                json!({ "status": code }),
            )
        } else {
            Self::new(
                "upstream_error_body",
                format!("the upstream returned {code} with an error in the body"),
                json!({ "status": code }),
            )
        };
        failure.status = Some(code);
        failure
    }

    /// 记进实例的观测值：按码计数、进时间桶，并成为「最近一次失败」（status 与界面里
    /// 看得到）
    pub(super) fn record_in(&self, stats: &crate::daemon::IpcStats) {
        stats.note_failed_attempt(self.code, &self.details, &self.message);
    }
}

/// 一个请求的终态：aproxy 自己结束了它。
#[derive(Debug, Clone)]
pub(super) struct Terminal {
    /// 数据面终态码（码表里 terminal 族的名字）
    code: &'static str,
    /// 一句英文说明，不是契约
    message: String,
    /// 码表登记的 `details`
    details: Map<String, Value>,
    /// 最后一次上游响应的状态码（有的话）
    upstream_status: Option<u16>,
}

impl Terminal {
    pub(super) fn new(code: &'static str, message: String, details: Value) -> Self {
        debug_assert!(
            aproxy_ipc::code::lookup(code).is_some_and(|c| c.http.is_some()),
            "not a data-plane terminal code: {code}"
        );
        Self {
            code,
            message,
            details: as_map(details),
            upstream_status: None,
        }
    }

    pub(super) fn code(&self) -> &'static str {
        self.code
    }

    /// 记进实例的观测值：按码计数，并成为「最近一次失败」
    pub(super) fn record_in(&self, stats: &crate::daemon::IpcStats) {
        stats.note_terminal(self.code, &self.details, &self.message);
    }

    /// 链内结束的尝试没有上游响应，明细只描述已脱敏的终止原因。
    pub(super) fn for_history(&self) -> crate::history::Failure<'_> {
        crate::history::Failure {
            code: self.code,
            details: &self.details,
            message: &self.message,
            has_response: false,
        }
    }

    /// Host 不在允许范围
    pub(super) fn inbound_host_rejected(host: &str) -> Self {
        Self::new(
            "inbound_host_rejected",
            format!(
                "the Host {host:?} is not allowed; add it (without the port) to allowed_hosts in config.toml, or set allowed_hosts = [\"*\"] to turn the check off"
            ),
            json!({ "host": host }),
        )
    }

    /// 带了不允许的 Origin
    pub(super) fn inbound_origin_rejected(origin: &str) -> Self {
        Self::new(
            "inbound_origin_rejected",
            format!(
                "requests from the web page origin {origin:?} are not allowed; add it to allowed_origins in config.toml, or set allowed_origins = [\"*\"] to turn the check off"
            ),
            json!({ "origin": origin }),
        )
    }

    /// 请求体超过上限
    pub(super) fn request_too_large(limit_bytes: usize) -> Self {
        Self::new(
            "request_too_large",
            format!(
                "the request body exceeds the limit of {limit_bytes} bytes; raise max_request_mb in config.toml"
            ),
            json!({ "limit_bytes": limit_bytes }),
        )
    }

    /// 读客户端请求体时中途失败
    pub(super) fn client_body_aborted() -> Self {
        Self::new(
            "client_body_aborted",
            "reading the request body failed: the client stopped sending".to_string(),
            json!({}),
        )
    }

    /// Upgrade（WebSocket）请求
    pub(super) fn upgrade_unsupported(upgrade: &str) -> Self {
        Self::new(
            "upgrade_unsupported",
            format!("connection upgrades ({upgrade}) are not supported"),
            json!({ "upgrade": upgrade }),
        )
    }

    /// 请求体或响应体落盘、读回失败。`op` 说明是哪一步
    pub(super) fn local_storage_failed(op: StorageOp, error: &str) -> Self {
        Self::new(
            "local_storage_failed",
            format!("local disk cache failed ({}): {error}", op.as_str()),
            json!({ "op": op.as_str() }),
        )
    }

    /// 上游响应超过缓冲上限
    pub(super) fn response_too_large(limit_bytes: usize) -> Self {
        Self::new(
            "response_too_large",
            format!(
                "the upstream response exceeds the limit of {limit_bytes} bytes; raise max_response_mb in config.toml"
            ),
            json!({ "limit_bytes": limit_bytes }),
        )
    }

    /// 允许的尝试次数用尽。`by` 说明是谁给的次数，`last` 是最后一次失败
    pub(super) fn retry_exhausted(by: &str, attempts: u32, last: &AttemptFailure) -> Self {
        let what = match last.status {
            Some(status) => format!("upstream returned {status}"),
            None => last.message.clone(),
        };
        let mut terminal = Self::new(
            "retry_exhausted",
            format!("{what} on all {attempts} attempts allowed by {by}"),
            json!({ "by": by, "last_code": last.code }),
        );
        terminal.upstream_status = last.status;
        terminal
    }

    /// 程序终止重试，保留最近一次尝试的真正故障码，不把程序自定的状态冒充上游状态。
    pub(super) fn retry_stopped(name: &str, last: &AttemptFailure) -> Self {
        let mut t = Self::new(
            "retry_stopped",
            format!("format '{name}' stopped retrying: {}", last.message),
            json!({"format":name,"last_code":last.code}),
        );
        t.upstream_status = last.status;
        t
    }
    /// 请求侧 format 明确拒绝了这个请求
    pub(super) fn format_rejected(format: &str, reason: &str, details: Value) -> Self {
        Self::new(
            "format_rejected",
            format!("format '{format}' rejected the request: {reason}"),
            details,
        )
    }

    /// 不重试的请求（仅转发）的上游尝试失败
    pub(super) fn upstream_failed(last: &AttemptFailure) -> Self {
        let mut terminal = Self::new(
            "upstream_failed",
            format!("{} (this request is not retried)", last.message),
            json!({ "last_code": last.code }),
        );
        terminal.upstream_status = last.status;
        terminal
    }

    /// 流已提交，上游的压缩编码无法携带，解码又失败了
    pub(super) fn decode_failed(encoding: &str, secrets: &aproxy_ipc::redact::Secrets) -> Self {
        // Debug 转义会改变引号和反斜杠的拼写，必须先替换原值。
        let encoding = secrets.replace(encoding);
        Self::new(
            "decode_failed",
            format!(
                "the upstream response used the content-encoding {encoding:?}, which the already committed stream cannot carry, and decoding it failed"
            ),
            json!({ "encoding": encoding }),
        )
    }

    /// 程序缺陷
    pub(super) fn internal(what: &str) -> Self {
        Self::new("internal", format!("internal error: {what}"), json!({}))
    }

    /// 错误体与 SSE 终态事件共用的 JSON：
    /// `{"type":"error","error":{"type":"aproxy_<码>","message":…,"aproxy":{…}}}`。
    /// `error.aproxy` 里是给程序读的部分：`code`、`request_id`、`attempts`、`upstream_status`
    /// （没有就省略）、`details`。
    pub(super) fn payload(&self, trace: &RequestTrace) -> Value {
        let mut aproxy = Map::new();
        aproxy.insert("code".into(), json!(self.code));
        aproxy.insert("request_id".into(), json!(&*trace.request_id));
        aproxy.insert("attempts".into(), json!(trace.attempts));
        if let Some(status) = self.upstream_status {
            aproxy.insert("upstream_status".into(), json!(status));
        }
        aproxy.insert("details".into(), Value::Object(self.details.clone()));
        json!({
            "type": "error",
            "error": {
                "type": format!("aproxy_{}", self.code),
                "message": self.message,
                "aproxy": aproxy,
            },
        })
    }

    /// 响应头还没写出时的终态：JSON 错误响应，带两个头。状态码取码表登记的值
    /// （「透传，否则 N」的码在没有可透传的响应时用 N）。请求记录的结局随之定为
    /// 「aproxy 自己结束了它」。
    pub(super) fn into_response(self, trace: &RequestTrace) -> Response {
        trace
            .rec
            .settle(crate::history::Outcome::AproxyError, Some(self.code));
        let status = match aproxy_ipc::code::lookup(self.code).and_then(|c| c.http) {
            Some(
                aproxy_ipc::code::HttpStatus::Fixed(status)
                | aproxy_ipc::code::HttpStatus::PassthroughOr(status),
            ) => StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            // 只作 SSE 事件的码不该走到这里；走到了也给一个明确的 500
            Some(aproxy_ipc::code::HttpStatus::SseOnly) | None => StatusCode::INTERNAL_SERVER_ERROR,
        };
        let mut response = (status, axum::Json(self.payload(trace))).into_response();
        mark_given_up(response.headers_mut(), self.code, trace);
        response
    }

    /// 响应头已经写出之后的终态：SSE 的 `event: error`，data 是压成一行的同一个 JSON。
    /// 状态行已发出、不可再改，客户端借此明确知道 aproxy 放弃了这条请求，而不是永远等下去。
    /// 请求记录的结局随之定为「aproxy 自己结束了它」。
    pub(super) fn sse_event(&self, trace: &RequestTrace) -> Bytes {
        trace
            .rec
            .settle(crate::history::Outcome::AproxyError, Some(self.code));
        // serde_json 的紧凑输出不含换行（字符串里的换行被转义），整个 data 就是一行
        Bytes::from(format!("event: error\ndata: {}\n\n", self.payload(trace)))
    }
}

/// 落盘缓存出问题的是哪一步（`local_storage_failed` 的 `details.op`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StorageOp {
    /// 把客户端的请求体写进磁盘缓存
    WriteRequest,
    /// 从磁盘缓存读回请求体（发往上游、交给 format）
    ReadRequest,
    /// 把上游的响应体写进磁盘缓存
    WriteResponse,
}

impl StorageOp {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            StorageOp::WriteRequest => "write_request",
            StorageOp::ReadRequest => "read_request",
            StorageOp::WriteResponse => "write_response",
        }
    }
}

/// 给响应加上两个头：请求标识，以及 aproxy 结束这个请求所用的码。自产的错误响应与放弃之后
/// 原样透传的上游响应都加（后者的体与状态码保真，头让调用方分得清「aproxy 已经放弃」与
/// 「上游的普通回应」）。
pub(super) fn mark_given_up(headers: &mut HeaderMap, code: &str, trace: &RequestTrace) {
    // 请求标识是十六进制、连字符与数字，码是 snake_case，都是合法的头值
    if let Ok(value) = HeaderValue::from_str(&trace.request_id) {
        headers.insert(HeaderName::from_static(REQUEST_ID_HEADER), value);
    }
    if let Ok(value) = HeaderValue::from_str(code) {
        headers.insert(HeaderName::from_static(ERROR_HEADER), value);
    }
}

fn as_map(details: Value) -> Map<String, Value> {
    match details {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trace(attempts: u32) -> RequestTrace {
        RequestTrace {
            request_id: Arc::from("19ac3f2e8b5d1a2b-41"),
            attempts,
            decide_started: None,
            upstream_join: Default::default(),
            rec: Recorder::none(),
        }
    }

    #[test]
    fn decode_error_redacts_before_debug_escaping() {
        let key = r#"secret\with"quote"#;
        let terminal = Terminal::decode_failed(key, &aproxy_ipc::redact::Secrets::new([key]));
        assert_eq!(terminal.details["encoding"], "<secret>");
        assert!(terminal.message.contains("\"<secret>\""));
    }

    /// 每个构造函数给出的 `details` 的键，都是码表为这个码登记的键；必填的键一个不少。
    #[test]
    fn details_match_the_code_table() {
        let failure = AttemptFailure::response(StatusCode::NOT_FOUND);
        let terminals = [
            Terminal::inbound_host_rejected("evil.example"),
            Terminal::inbound_origin_rejected("https://evil.example"),
            Terminal::request_too_large(1024),
            Terminal::client_body_aborted(),
            Terminal::upgrade_unsupported("websocket"),
            Terminal::local_storage_failed(StorageOp::WriteResponse, "disk full"),
            Terminal::response_too_large(1024),
            Terminal::retry_exhausted("rule:1", 3, &failure),
            Terminal::format_rejected("keys", "no", json!({"format":"keys", "stage":"request"})),
            Terminal::upstream_failed(&failure),
            Terminal::decode_failed("br", &Default::default()),
            Terminal::internal("x"),
        ];
        let failures = [
            AttemptFailure::connect_failed("refused".into()),
            AttemptFailure::connect_timeout(Duration::from_secs(1)),
            AttemptFailure::first_byte_timeout(Duration::from_secs(1)),
            AttemptFailure::read_idle_timeout(Duration::from_secs(1), 10),
            AttemptFailure::body_interrupted(10, "reset".into()),
            AttemptFailure::response(StatusCode::SERVICE_UNAVAILABLE),
            AttemptFailure::response(StatusCode::OK),
        ];
        let all = terminals
            .iter()
            .map(|t| (t.code, &t.details))
            .chain(failures.iter().map(|f| (f.code, &f.details)));
        for (code, details) in all {
            let entry = aproxy_ipc::code::lookup(code).unwrap_or_else(|| panic!("{code}"));
            for key in details.keys() {
                assert!(
                    entry.details.iter().any(|d| d.key == key),
                    "{code}: details.{key} 没有登记"
                );
            }
            for detail in entry.details.iter().filter(|d| !d.optional) {
                assert!(
                    details.contains_key(detail.key),
                    "{code}: 缺必填的 details.{}",
                    detail.key
                );
            }
        }
    }

    #[test]
    fn payload_has_the_documented_shape() {
        let last = AttemptFailure::response(StatusCode::NOT_FOUND);
        let terminal = Terminal::retry_exhausted("rule:1", 3, &last);
        assert_eq!(
            terminal.payload(&trace(3)),
            json!({
                "type": "error",
                "error": {
                    "type": "aproxy_retry_exhausted",
                    "message": "upstream returned 404 on all 3 attempts allowed by rule:1",
                    "aproxy": {
                        "code": "retry_exhausted",
                        "request_id": "19ac3f2e8b5d1a2b-41",
                        "attempts": 3,
                        "upstream_status": 404,
                        "details": {"by": "rule:1", "last_code": "upstream_status"},
                    },
                },
            })
        );
        // 没有上游响应时不带 upstream_status
        let payload = Terminal::request_too_large(8).payload(&trace(0));
        assert!(payload["error"]["aproxy"].get("upstream_status").is_none());
        assert_eq!(payload["error"]["aproxy"]["attempts"], 0);
    }

    #[test]
    fn response_carries_status_json_and_both_headers() {
        let response = Terminal::request_too_large(8).into_response(&trace(0));
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(
            response.headers()[http::header::CONTENT_TYPE],
            "application/json"
        );
        assert_eq!(response.headers()[REQUEST_ID_HEADER], "19ac3f2e8b5d1a2b-41");
        assert_eq!(response.headers()[ERROR_HEADER], "request_too_large");
        // 「透传，否则 N」的码在没有可透传的响应时用 N
        let last = AttemptFailure::connect_failed("refused".into());
        let response = Terminal::retry_exhausted("rule:1", 3, &last).into_response(&trace(3));
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        assert_eq!(
            Terminal::internal("x").into_response(&trace(0)).status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    /// SSE 终态事件是一帧：`event: error`、一行 data、空行；data 与错误体是同一个 JSON。
    /// 说明文字里的引号、换行、非 ASCII 字符都不会破坏这一帧。
    #[test]
    fn sse_event_is_one_frame_with_the_same_json() {
        let terminal = Terminal::format_rejected(
            "keys",
            "line one\nline \"two\"\r\n中文",
            json!({"format":"keys", "stage":"request"}),
        );
        let event = terminal.sse_event(&trace(2));
        let text = std::str::from_utf8(&event).unwrap();
        let data = text
            .strip_prefix("event: error\ndata: ")
            .and_then(|rest| rest.strip_suffix("\n\n"))
            .expect("一帧：event 行、data 行、空行");
        assert!(!data.contains('\n') && !data.contains('\r'), "{data:?}");
        let parsed: Value = serde_json::from_str(data).unwrap();
        assert_eq!(parsed, terminal.payload(&trace(2)));
    }
}
