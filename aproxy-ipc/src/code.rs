//! 错误码表：aproxy 对外说出的每一个码，只在这里登记一次。
//!
//! 码是一切渲染的输入：HTTP 错误体与 SSE 终态事件、IPC 的错误对象、命令行的 `--json` 与
//! 退出码、配置诊断、请求记录，都从这张表取名字与元数据；给人看的文字（各语言）由调用方
//! 按码去说。
//!
//! 规则：码是 snake_case，在整张表里唯一，**只增不改**——已经发布的码不改名、不换族、
//! 不改 HTTP 状态与退出码，`details` 的键不改名、不换类型，只许追加新的码与新的可选键。
//! 读者遇到不认识的码要能照常工作（按族的兜底去显示）。
//!
//! 每个码登记：名字、所属的族、一句英文说明、`details` 每个键的名字与类型，以及按族附带的
//! 元数据——数据面终态码的 HTTP 状态、操作码与安装码的退出码、配置诊断码的级别。
//!
//! [`table_json`] 把整张表导出成 JSON，界面的多语言文案与仓库里的文档都从它生成。

use serde::Serialize;

/// 码所属的族。一个码可以属于不止一个族（`internal` 在数据面、IPC、操作三处都用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Family {
    /// 数据面终态码：一个请求最终以它结束，写进 HTTP 错误体或 SSE 终态事件
    Terminal,
    /// 尝试失败码：一次上游尝试没有成功的原因
    Attempt,
    /// IPC 协议码：控制通道的错误对象
    Ipc,
    /// 操作码：命令行与界面发起的操作失败的原因，决定退出码
    Operation,
    /// 配置诊断码：config.toml 与 settings.json 的问题
    Diagnostic,
    /// 记录码：持久记录读不懂的原因
    Record,
    /// 安装码：install 的失败原因
    Install,
    /// 桌面窗口内的故障对象，不映射到 CLI 退出码。
    Desktop,
}

/// `details` 里一个键的类型。界面按它决定怎样显示（字节数换算成 MiB、毫秒换算成秒等）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DetailType {
    /// 字符串
    String,
    /// 字符串数组
    StringList,
    /// 整数（端口、pid、状态码、计数）
    Integer,
    /// 整数数组
    IntegerList,
    /// 字节数
    Bytes,
    /// 时长，毫秒
    Millis,
    /// 时刻，Unix 毫秒
    TimestampMs,
    /// 文件系统路径（字符串）
    Path,
    /// 另一个码的名字
    Code,
    /// 配置诊断对象的数组
    Diagnostics,
}

/// `details` 里的一个键。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Detail {
    /// 键名
    pub key: &'static str,
    /// 值的类型
    #[serde(rename = "type")]
    pub ty: DetailType,
    /// true = 这个键不一定出现
    pub optional: bool,
}

/// 数据面终态码对应的 HTTP 状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "status")]
pub enum HttpStatus {
    /// 固定的状态码
    Fixed(u16),
    /// 有上游（或 format）给的响应就原样透传它的状态码，没有时用这个
    PassthroughOr(u16),
    /// 不产生 HTTP 状态：响应头已经写出，只作为 SSE 终态事件出现
    SseOnly,
}

/// 操作码与安装码对应的退出码。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "code")]
pub enum ExitCode {
    /// 固定的退出码
    Fixed(u8),
    /// 子进程报了退出码就用它的，没有时用这个
    ChildOr(u8),
}

/// 配置诊断的级别。只有 `error` 影响退出码。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
    Info,
    /// 级别随情形而定，由产生诊断的地方给出
    Varies,
}

/// 表里的一个码。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Code {
    /// 码的名字，snake_case，全表唯一
    pub name: &'static str,
    /// 所属的族
    pub families: &'static [Family],
    /// 一句英文说明：什么时候出现。给文档与界面兜底用，不是契约
    pub summary: &'static str,
    /// 数据面终态码的 HTTP 状态；别的族为 None
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http: Option<HttpStatus>,
    /// 操作码、安装码的退出码；别的族为 None
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit: Option<ExitCode>,
    /// 配置诊断码的级别；别的族为 None
    #[serde(skip_serializing_if = "Option::is_none")]
    pub severity: Option<Severity>,
    /// `details` 里的键
    pub details: &'static [Detail],
}

impl Code {
    /// HTTP 错误体里 `error.type` 的值：`aproxy_` 加码
    pub fn http_error_type(&self) -> String {
        format!("aproxy_{}", self.name)
    }
}

// --- 通用退出码（命令行）。10 起归强制安装（永久兼容契约 C7），不在这里 ---

/// 成功，或目标状态已经达成
pub const EXIT_OK: u8 = 0;
/// 操作失败
pub const EXIT_FAILED: u8 = 1;
/// 用法错误：解析错误、该指定目标时没指定、home 无效
pub const EXIT_USAGE: u8 = 2;
/// 配置无效：config.toml 或 settings.json 有 error 级诊断
pub const EXIT_CONFIG: u8 = 3;
/// 目标未运行或不存在
pub const EXIT_NOT_FOUND: u8 = 4;
/// 部分成功
pub const EXIT_PARTIAL: u8 = 5;

// --- 登记用的简写 ---

const fn req(key: &'static str, ty: DetailType) -> Detail {
    Detail {
        key,
        ty,
        optional: false,
    }
}

const fn opt(key: &'static str, ty: DetailType) -> Detail {
    Detail {
        key,
        ty,
        optional: true,
    }
}

const fn terminal(
    name: &'static str,
    http: HttpStatus,
    summary: &'static str,
    details: &'static [Detail],
) -> Code {
    Code {
        name,
        families: &[Family::Terminal],
        summary,
        http: Some(http),
        exit: None,
        severity: None,
        details,
    }
}

const fn plain(
    name: &'static str,
    family: &'static [Family],
    summary: &'static str,
    details: &'static [Detail],
) -> Code {
    Code {
        name,
        families: family,
        summary,
        http: None,
        exit: None,
        severity: None,
        details,
    }
}

const fn exiting(
    name: &'static str,
    family: &'static [Family],
    exit: ExitCode,
    summary: &'static str,
    details: &'static [Detail],
) -> Code {
    Code {
        name,
        families: family,
        summary,
        http: None,
        exit: Some(exit),
        severity: None,
        details,
    }
}

const fn diagnostic(
    name: &'static str,
    severity: Severity,
    summary: &'static str,
    details: &'static [Detail],
) -> Code {
    Code {
        name,
        families: &[Family::Diagnostic],
        summary,
        http: None,
        exit: None,
        severity: Some(severity),
        details,
    }
}

use DetailType as T;
use ExitCode::{ChildOr, Fixed as Exit};
use HttpStatus::{Fixed as Http, PassthroughOr, SseOnly};

const ATTEMPT: &[Family] = &[Family::Attempt];
const IPC: &[Family] = &[Family::Ipc];
const OPERATION: &[Family] = &[Family::Operation];
const RECORD: &[Family] = &[Family::Record];
const INSTALL: &[Family] = &[Family::Install];

/// 整张码表。顺序就是文档里的顺序：按族分段，族内按登记的先后。
pub const TABLE: &[Code] = &[
    plain(
        "desktop_package",
        &[Family::Desktop],
        "Package validation or a package operation failed.",
        &[
            req("reason", T::String),
            opt("field", T::String),
            opt("path", T::Path),
            opt("limit", T::String),
            opt("max", T::Integer),
        ],
    ),
    plain(
        "desktop_toml",
        &[Family::Desktop],
        "The desktop preferences file could not be edited.",
        &[
            req("kind", T::String),
            opt("line", T::Integer),
            opt("column", T::Integer),
            opt("key", T::String),
        ],
    ),
    // ----- 数据面终态码 -----
    terminal(
        "inbound_host_rejected",
        Http(403),
        "The Host header is not in the allowed set.",
        &[req("host", T::String)],
    ),
    terminal(
        "inbound_origin_rejected",
        Http(403),
        "The request carries an Origin that is not allowed.",
        &[req("origin", T::String)],
    ),
    terminal(
        "request_too_large",
        Http(413),
        "The request body exceeds max_request_mb.",
        &[req("limit_bytes", T::Bytes)],
    ),
    terminal(
        "client_body_aborted",
        Http(400),
        "Reading the client's request body failed midway.",
        &[],
    ),
    terminal(
        "upgrade_unsupported",
        Http(501),
        "Upgrade (WebSocket) requests are not supported.",
        &[req("upgrade", T::String)],
    ),
    terminal(
        "local_storage_failed",
        Http(507),
        "Writing the request or response body to disk failed.",
        &[req("op", T::String)],
    ),
    terminal(
        "response_too_large",
        Http(502),
        "The upstream response exceeds max_response_mb.",
        &[req("limit_bytes", T::Bytes)],
    ),
    terminal(
        "retry_exhausted",
        PassthroughOr(502),
        "The attempts allowed by a retry rule or a per-request override are used up.",
        &[req("by", T::String), req("last_code", T::Code)],
    ),
    terminal(
        "retry_stopped",
        PassthroughOr(502),
        "A format's decide stage stopped the retries.",
        &[req("format", T::String), req("last_code", T::Code)],
    ),
    terminal(
        "format_rejected",
        PassthroughOr(422),
        "A request-side format answered fail with retry:false.",
        &[
            req("format", T::String),
            req("stage", T::String),
            opt("fail_code", T::String),
        ],
    ),
    terminal(
        "format_unavailable",
        Http(503),
        "A required format's circuit is open, so a request-direction stage cannot use it.",
        &[
            req("format", T::String),
            req("stage", T::String),
            req("kind", T::String),
            req("probe_at", T::TimestampMs),
        ],
    ),
    terminal(
        "format_failed",
        Http(502),
        "A forward request's request-side format failed or asked for a retry, and the request was not upgraded to retry.",
        &[
            req("format", T::String),
            req("stage", T::String),
            opt("kind", T::String),
            opt("fail_code", T::String),
        ],
    ),
    terminal(
        "upstream_failed",
        Http(502),
        "A forward request's upstream attempt failed and the request was not upgraded to retry.",
        &[req("last_code", T::Code)],
    ),
    terminal(
        "decode_failed",
        SseOnly,
        "The stream is already committed, the upstream's content encoding cannot be carried, and decoding failed.",
        &[req("encoding", T::String)],
    ),
    terminal(
        "shutting_down",
        Http(503),
        "The instance is shutting down.",
        &[],
    ),
    // `internal` 在三个族里是同一个码：数据面 500、IPC 的错误对象、操作的退出码 1
    Code {
        name: "internal",
        families: &[Family::Terminal, Family::Ipc, Family::Operation],
        summary: "A defect in aproxy itself.",
        http: Some(Http(500)),
        exit: Some(Exit(EXIT_FAILED)),
        severity: None,
        details: &[],
    },
    // ----- 尝试失败码 -----
    plain(
        "upstream_connect_failed",
        ATTEMPT,
        "Could not connect to the upstream: DNS, refused, TLS.",
        &[req("reason", T::String)],
    ),
    plain(
        "upstream_connect_timeout",
        ATTEMPT,
        "Connecting to the upstream timed out.",
        &[req("timeout_ms", T::Millis)],
    ),
    plain(
        "upstream_first_byte_timeout",
        ATTEMPT,
        "The request was sent but no response headers arrived in time.",
        &[req("timeout_ms", T::Millis)],
    ),
    plain(
        "upstream_read_idle_timeout",
        ATTEMPT,
        "Too long passed between two reads of the response body.",
        &[
            req("timeout_ms", T::Millis),
            req("received_bytes", T::Bytes),
        ],
    ),
    plain(
        "upstream_body_interrupted",
        ATTEMPT,
        "The response body broke off midway.",
        &[req("received_bytes", T::Bytes), req("reason", T::String)],
    ),
    plain(
        "upstream_status",
        ATTEMPT,
        "The upstream answered 4xx or 5xx.",
        &[req("status", T::Integer)],
    ),
    plain(
        "upstream_error_body",
        ATTEMPT,
        "The upstream answered 2xx but the body is an error.",
        &[req("status", T::Integer)],
    ),
    plain(
        "format_error",
        ATTEMPT,
        "A format failed at the process level or produced invalid output.",
        &[
            req("format", T::String),
            req("stage", T::String),
            req("kind", T::String),
        ],
    ),
    plain(
        "format_fail",
        ATTEMPT,
        "A format answered fail with retry:true.",
        &[
            req("format", T::String),
            req("stage", T::String),
            opt("fail_code", T::String),
        ],
    ),
    plain(
        "format_decision",
        ATTEMPT,
        "A format's decide stage judged a successful attempt as failed.",
        &[req("format", T::String)],
    ),
    // ----- IPC 协议码 -----
    plain(
        "bad_request",
        IPC,
        "Not JSON, missing v or op, line too long, or an argument of the wrong type or value.",
        &[req("reason", T::String), opt("arg", T::String)],
    ),
    plain(
        "unsupported_version",
        IPC,
        "The protocol version v is not supported.",
        &[req("supported", T::IntegerList)],
    ),
    plain(
        "unknown_op",
        IPC,
        "There is no such op. Nothing was done.",
        &[req("op", T::String)],
    ),
    plain(
        "unsupported_arg",
        IPC,
        "The request carries arguments this instance does not know.",
        &[req("args", T::StringList)],
    ),
    plain(
        "invalid_state",
        IPC,
        "Not allowed in the instance's current state.",
        &[req("state", T::String)],
    ),
    plain(
        "condition_not_met",
        IPC,
        "The condition of a conditional op does not hold.",
        &[opt("idle_ms", T::Millis)],
    ),
    plain(
        "busy",
        IPC,
        "All streaming connections are in use.",
        &[req("limit", T::Integer)],
    ),
    plain(
        "not_found",
        IPC,
        "The request, or the body asked for, cannot be retrieved.",
        &[
            req("request_id", T::String),
            req("reason", T::String),
            opt("cause", T::String),
        ],
    ),
    // ----- 操作码 -----
    exiting(
        "usage",
        OPERATION,
        Exit(EXIT_USAGE),
        "A usage error found outside the argument parser.",
        &[req("reason", T::String)],
    ),
    exiting(
        "home_invalid",
        OPERATION,
        Exit(EXIT_USAGE),
        "APROXY_HOME does not satisfy the home rules.",
        &[req("value", T::String)],
    ),
    exiting(
        "target_ambiguous",
        OPERATION,
        Exit(EXIT_USAGE),
        "Several instances are running and no target was given.",
        &[req("ports", T::IntegerList)],
    ),
    exiting(
        "target_not_found",
        OPERATION,
        Exit(EXIT_NOT_FOUND),
        "The alias, port or config file does not exist.",
        &[req("target", T::String)],
    ),
    exiting(
        "target_not_running",
        OPERATION,
        Exit(EXIT_NOT_FOUND),
        "The target is not running.",
        &[req("target", T::String)],
    ),
    exiting(
        "config_invalid",
        OPERATION,
        Exit(EXIT_CONFIG),
        "The configuration has error-level diagnostics.",
        &[req("diagnostics", T::Diagnostics)],
    ),
    exiting(
        "command_failed",
        OPERATION,
        Exit(EXIT_FAILED),
        "The command could not complete.",
        &[],
    ),
    exiting(
        "write_conflict",
        OPERATION,
        Exit(EXIT_FAILED),
        "The config file was modified externally while writing, and retrying still conflicted.",
        &[req("file", T::Path)],
    ),
    exiting(
        "port_in_use",
        OPERATION,
        Exit(EXIT_FAILED),
        "The port is taken.",
        &[req("port", T::Integer), req("owner", T::String)],
    ),
    exiting(
        "start_failed",
        OPERATION,
        ChildOr(EXIT_FAILED),
        "The daemon could not start.",
        &[opt("code", T::Integer), req("startup_log", T::StringList)],
    ),
    exiting(
        "ready_timeout",
        OPERATION,
        Exit(EXIT_FAILED),
        "The instance did not reach serving within the time limit.",
        &[req("timeout_ms", T::Millis)],
    ),
    exiting(
        "restart_unsupported",
        OPERATION,
        Exit(EXIT_FAILED),
        "The target has no desired-state record (a foreground instance).",
        &[req("port", T::Integer)],
    ),
    exiting(
        "instance_unresponsive",
        OPERATION,
        Exit(EXIT_FAILED),
        "The instance's process is there but it does not answer.",
        &[req("port", T::Integer), req("pid", T::Integer)],
    ),
    exiting(
        "instance_incompatible",
        OPERATION,
        Exit(EXIT_FAILED),
        "The instance answered, but its protocol is incompatible or its identity does not match.",
        &[req("port", T::Integer), req("reason", T::String)],
    ),
    exiting(
        "stop_failed",
        OPERATION,
        Exit(EXIT_FAILED),
        "The instance could not be stopped.",
        &[req("port", T::Integer), req("reason", T::String)],
    ),
    exiting(
        "no_log_file",
        OPERATION,
        Exit(EXIT_FAILED),
        "A foreground instance has no log file.",
        &[req("port", T::Integer)],
    ),
    exiting(
        "desktop_unavailable",
        OPERATION,
        Exit(EXIT_FAILED),
        "The desktop command was run on a headless build.",
        &[req("variant", T::String)],
    ),
    exiting(
        "skill_not_installed",
        OPERATION,
        Exit(EXIT_NOT_FOUND),
        "This skill is not installed locally.",
        &[req("name", T::String)],
    ),
    exiting(
        "skill_file_not_found",
        OPERATION,
        Exit(EXIT_NOT_FOUND),
        "The file is not in the skill's manifest.",
        &[req("name", T::String), req("file", T::Path)],
    ),
    // ----- 配置诊断码 -----
    diagnostic(
        "config_syntax",
        Severity::Error,
        "TOML or JSON syntax error.",
        &[],
    ),
    diagnostic(
        "config_type",
        Severity::Error,
        "A value has the wrong type.",
        &[],
    ),
    diagnostic(
        "config_invalid_value",
        Severity::Error,
        "A value is not allowed.",
        &[req("reason", T::String)],
    ),
    diagnostic(
        "config_missing_key",
        Severity::Error,
        "A required key is missing.",
        &[],
    ),
    diagnostic(
        "config_unknown_key",
        Severity::Error,
        "A key that is not recognized.",
        &[],
    ),
    diagnostic(
        "config_not_supported",
        Severity::Error,
        "A form whose shape is defined but which is not implemented yet.",
        &[],
    ),
    diagnostic(
        "format_name_reserved",
        Severity::Error,
        "The format name is reserved for selecting all entries.",
        &[req("name", T::String)],
    ),
    diagnostic(
        "config_conflict",
        Severity::Error,
        "A combination of settings that exclude each other.",
        &[],
    ),
    diagnostic(
        "secret_placeholder_unmatched",
        Severity::Error,
        "A secret placeholder written back does not match any original value.",
        &[],
    ),
    diagnostic(
        "default_credential_withheld",
        Severity::Warning,
        "A credential in the settings defaults was not inherited because the target differs, and this config has none of its own.",
        &[req("key", T::String), req("reason", T::String)],
    ),
    diagnostic(
        "default_header_shared",
        Severity::Info,
        "The defaults' header tables contain non-secret headers, which are sent to every target.",
        &[],
    ),
    diagnostic(
        "forward_default_unguarded",
        Severity::Warning,
        crate::i18n::diagnostic::forward_default_unguarded(crate::i18n::Lang::En),
        &[],
    ),
    diagnostic(
        "forward_response_buffered",
        Severity::Info,
        crate::i18n::diagnostic::forward_response_buffered(crate::i18n::Lang::En),
        &[req("format", T::String)],
    ),
    diagnostic(
        "settings_schema_too_new",
        Severity::Error,
        "settings.json has a schema newer than this version understands.",
        &[],
    ),
    diagnostic(
        "format_handshake_failed",
        Severity::Varies,
        "A format's handshake failed at startup.",
        &[req("format", T::String), req("kind", T::String)],
    ),
    diagnostic(
        "format_stage_unsupported",
        Severity::Error,
        "A format does not support a stage the config declares for it.",
        &[req("format", T::String), req("stages", T::StringList)],
    ),
    diagnostic(
        "file_permissions_too_open",
        Severity::Warning,
        "The group or others can access the home or a config file.",
        &[],
    ),
    diagnostic(
        "listen_not_loopback",
        Severity::Warning,
        "The config listens on a non-loopback address, so other hosts can reach the proxy.",
        &[req("listen_addr", T::String)],
    ),
    diagnostic(
        "port_shared",
        Severity::Warning,
        "Several config files listen on the same port, so they cannot run at the same time.",
        &[req("port", T::Integer), req("files", T::StringList)],
    ),
    diagnostic(
        "config_dir_missing",
        Severity::Warning,
        "A directory listed in settings.json `config_dirs` does not exist.",
        &[req("dir", T::Path)],
    ),
    diagnostic(
        "config_file_unreadable",
        Severity::Warning,
        "A config file named by an alias or by `default_config` does not exist or cannot be read.",
        &[req("reason", T::String)],
    ),
    diagnostic(
        "watchdog_slow_hang_detection",
        Severity::Warning,
        "settings.json sets `watchdog.heartbeat_secs` so high that a hung instance is detected only after minutes.",
        &[req("heartbeat_secs", T::Integer)],
    ),
    diagnostic(
        "watchdog_restarts_disabled",
        Severity::Warning,
        "settings.json sets `watchdog.max_restarts` to 0, so the watchdog never restarts a crashed or hung instance.",
        &[],
    ),
    diagnostic(
        "install_restore_unreadable",
        Severity::Error,
        "The desired-state record needed for the rolling restart cannot be read.",
        &[],
    ),
    diagnostic(
        "install_config_unreadable",
        Severity::Error,
        "The instance configuration needed for the rolling restart is missing or unreadable.",
        &[],
    ),
    // ----- 记录码 -----
    plain(
        "record_unreadable",
        RECORD,
        "A persistent record cannot be parsed.",
        &[],
    ),
    plain(
        "record_schema_too_new",
        RECORD,
        "A persistent record has a schema newer than this version understands.",
        &[req("schema", T::Integer)],
    ),
    // ----- 安装码 -----
    exiting(
        "install_in_progress",
        INSTALL,
        Exit(EXIT_FAILED),
        "Another install is in progress in this home.",
        &[],
    ),
    exiting(
        "install_version_not_found",
        INSTALL,
        Exit(EXIT_NOT_FOUND),
        "The requested version does not exist.",
        &[],
    ),
    exiting(
        "install_below_floor",
        INSTALL,
        Exit(EXIT_FAILED),
        "The requested version is older than the oldest one this home can be moved to.",
        &[],
    ),
    exiting(
        "install_download_failed",
        INSTALL,
        Exit(EXIT_FAILED),
        "Downloading the binary failed.",
        &[],
    ),
    exiting(
        "install_checksum_mismatch",
        INSTALL,
        Exit(EXIT_FAILED),
        "The downloaded binary does not match its checksum.",
        &[],
    ),
    exiting(
        "install_probe_failed",
        INSTALL,
        Exit(EXIT_FAILED),
        "The new binary did not run, or reported a version other than the one being installed.",
        &[],
    ),
    exiting(
        "install_swap_failed",
        INSTALL,
        Exit(EXIT_FAILED),
        "Replacing the binary in bin/ failed.",
        &[],
    ),
    exiting(
        "install_rolled_back",
        INSTALL,
        Exit(EXIT_FAILED),
        "The install failed and the previous binary was put back.",
        &[],
    ),
    exiting(
        "install_restart_failed",
        INSTALL,
        Exit(EXIT_FAILED),
        "An instance did not start on the new binary, so the rolling restart stopped.",
        &[req("port", T::Integer)],
    ),
    exiting(
        "install_failed",
        INSTALL,
        Exit(EXIT_FAILED),
        "The install failed for a reason that no more specific code covers.",
        &[],
    ),
    exiting(
        "install_unmanaged_instances",
        INSTALL,
        Exit(EXIT_FAILED),
        "Live instances started by an older version are in this home and cannot be restarted by this install.",
        &[],
    ),
    exiting(
        "install_foreground_instances",
        INSTALL,
        Exit(EXIT_FAILED),
        "Live foreground instances are in this home; an install cannot restart a foreground instance.",
        &[req("ports", T::IntegerList)],
    ),
    exiting(
        "install_precheck_failed",
        INSTALL,
        Exit(EXIT_CONFIG),
        "The precheck before the rolling restart did not pass.",
        &[req("port", T::Integer), req("diagnostics", T::Diagnostics)],
    ),
];

/// 按名字查码。
pub fn lookup(name: &str) -> Option<&'static Code> {
    TABLE.iter().find(|c| c.name == name)
}

/// 整张表的 JSON（一个数组，每个码一个对象），供界面与文档生成使用。
pub fn table_json() -> serde_json::Value {
    serde_json::to_value(TABLE)
        .expect("the code table contains only strings, numbers, and enums and must serialize")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_snake_case(s: &str) -> bool {
        !s.is_empty()
            && s.starts_with(|c: char| c.is_ascii_lowercase())
            && !s.ends_with('_')
            && !s.contains("__")
            && s.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    }

    #[test]
    fn names_are_unique_snake_case() {
        let mut seen = std::collections::HashSet::new();
        for code in TABLE {
            assert!(is_snake_case(code.name), "{}", code.name);
            assert!(seen.insert(code.name), "重复的码: {}", code.name);
            assert!(!code.families.is_empty(), "{}", code.name);
            assert!(!code.summary.is_empty(), "{}", code.name);
            let mut keys = std::collections::HashSet::new();
            for detail in code.details {
                assert!(is_snake_case(detail.key), "{}.{}", code.name, detail.key);
                assert!(keys.insert(detail.key), "{}.{}", code.name, detail.key);
            }
        }
    }

    /// 每个码都按族带着该有的元数据，不带不该有的。
    #[test]
    fn metadata_follows_the_family() {
        for code in TABLE {
            let has = |f| code.families.contains(&f);
            assert_eq!(
                code.http.is_some(),
                has(Family::Terminal),
                "{}: 数据面终态码（且只有它）带 HTTP 状态",
                code.name
            );
            assert_eq!(
                code.exit.is_some(),
                has(Family::Operation) || has(Family::Install),
                "{}: 操作码与安装码（且只有它们）带退出码",
                code.name
            );
            assert_eq!(
                code.severity.is_some(),
                has(Family::Diagnostic),
                "{}: 配置诊断码（且只有它）带级别",
                code.name
            );
            if has(Family::Install) {
                assert!(code.name.starts_with("install_"), "{}", code.name);
            }
        }
    }

    /// 通用退出码只用 0–5：10 起归强制安装（永久兼容契约 C7）。
    #[test]
    fn exit_codes_stay_in_the_general_range() {
        for code in TABLE {
            if let Some(ExitCode::Fixed(n) | ExitCode::ChildOr(n)) = code.exit {
                assert!((1..=5).contains(&n), "{}: {n}", code.name);
            }
        }
        assert_eq!(
            [
                EXIT_OK,
                EXIT_FAILED,
                EXIT_USAGE,
                EXIT_CONFIG,
                EXIT_NOT_FOUND,
                EXIT_PARTIAL
            ],
            [0, 1, 2, 3, 4, 5]
        );
    }

    /// `last_code` 这样的键，值是另一个码的名字：表里得有能填进去的码。
    #[test]
    fn lookup_and_error_type() {
        let code = lookup("retry_exhausted").unwrap();
        assert_eq!(code.http, Some(HttpStatus::PassthroughOr(502)));
        assert_eq!(code.http_error_type(), "aproxy_retry_exhausted");
        assert!(lookup("no_such_code").is_none());
        let internal = lookup("internal").unwrap();
        assert_eq!(
            internal.families,
            [Family::Terminal, Family::Ipc, Family::Operation]
        );
    }

    /// 导出的 JSON 的形状：界面按这些键读。
    #[test]
    fn exported_json_shape() {
        let json = table_json();
        let entries = json.as_array().unwrap();
        assert_eq!(entries.len(), TABLE.len());
        let find = |name: &str| {
            entries
                .iter()
                .find(|e| e["name"] == name)
                .unwrap_or_else(|| panic!("{name}"))
        };
        assert_eq!(
            find("request_too_large"),
            &serde_json::json!({
                "name": "request_too_large",
                "families": ["terminal"],
                "summary": "The request body exceeds max_request_mb.",
                "http": {"kind": "fixed", "status": 413},
                "details": [{"key": "limit_bytes", "type": "bytes", "optional": false}],
            })
        );
        assert_eq!(
            find("retry_exhausted")["http"],
            serde_json::json!({"kind": "passthrough_or", "status": 502})
        );
        assert_eq!(
            find("decode_failed")["http"],
            serde_json::json!({"kind": "sse_only"})
        );
        assert_eq!(
            find("start_failed")["exit"],
            serde_json::json!({"kind": "child_or", "code": 1})
        );
        assert_eq!(find("config_syntax")["severity"], "error");
        assert_eq!(find("format_handshake_failed")["severity"], "varies");
    }
}
