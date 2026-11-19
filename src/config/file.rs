//! config.toml 的严格解析：把文件读成「这个文件说了什么」（[`FileConfig`]），连同诊断。
//!
//! 这一层只管文件本身：
//! - **严格**：任何不认识的键都是 error 级诊断 `config_unknown_key`，带键路径与行列——包括
//!   重试规则与 `[[format]]` 条目里的键。以后新增的键在旧版本上是拒绝启动，而不是被悄悄忽略
//!   （一条不认识的匹配器若被忽略，规则就比用户写的更宽）。
//! - **不补默认值**：没写的键是 None。默认值、settings 的全局默认与启动时的覆盖怎样叠在
//!   一起，是分层解析的事；只有 `[[format]]` 条目与重试规则这两种「整条替换」的东西在这里
//!   就把自己的默认值填好。
//! - **不读文件系统**：输入是文本，输出是值，可以单测。
//!
//! 诊断的 `message` 是英文；显示端按码去说各自语言的话。

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use aproxy_ipc::diag::{Diagnostic, Level, line_column};
use serde_json::{Map, Value, json};
use toml_edit::{Item, TableLike};

use super::KeepaliveTrigger;

/// 请求的默认投递方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// 边收边转，不缓冲、不重试
    Forward,
    /// 缓冲之后发往上游，失败就重试
    Retry,
}

impl Delivery {
    pub fn as_str(self) -> &'static str {
        match self {
            Delivery::Forward => "forward",
            Delivery::Retry => "retry",
        }
    }
}

/// format 程序参与的阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Stage {
    Request,
    Retry,
    Decide,
    Response,
    Stream,
    Heartbeat,
    Notify,
}

impl Stage {
    pub fn as_str(self) -> &'static str {
        match self {
            Stage::Request => "request",
            Stage::Retry => "retry",
            Stage::Decide => "decide",
            Stage::Response => "response",
            Stage::Stream => "stream",
            Stage::Heartbeat => "heartbeat",
            Stage::Notify => "notify",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "request" => Stage::Request,
            "retry" => Stage::Retry,
            "decide" => Stage::Decide,
            "response" => Stage::Response,
            "stream" => Stage::Stream,
            "heartbeat" => Stage::Heartbeat,
            "notify" => Stage::Notify,
            _ => return None,
        })
    }
}

/// format 程序的进程方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Process {
    /// 每条消息起一个进程
    Spawn,
    /// 常驻进程池
    Persistent,
}

/// 一条 `[[format]]`。没写的键已经填上默认值。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatEntry {
    /// 在列表内唯一；没写时取 `command` 的文件名主干
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    /// 追加给程序的环境变量。值只写不读
    pub env: BTreeMap<String, String>,
    pub process: Process,
    pub stages: Vec<Stage>,
    /// 程序出错与熔断时：true = 请求随之失败，false = 跳过它
    pub required: bool,
    pub pool_max: u32,
    pub worker_idle_secs: u64,
    pub timeout_secs: u64,
    pub decide_timeout_secs: u64,
    /// 连续进程级失败达到这个数就熔断；0 = 不熔断
    pub breaker_failures: u32,
    pub breaker_cooldown_secs: u64,
}

impl FormatEntry {
    pub fn has_stage(&self, stage: Stage) -> bool {
        self.stages.contains(&stage)
    }
}

/// 一条 `[[retry.rules]]`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryRule {
    /// 对客户端请求的「路径?查询串」整体匹配的正则（自动锚定）；没写 = 任何路径
    pub path: Option<String>,
    /// 上游状态码；没写 = 任何状态码
    pub status: Option<Vec<u16>>,
    /// 第 N 次尝试失败、命中这条规则且 N ≥ 它，就停；0 = 不限（写在宽规则前面当豁免）
    pub max_attempts: u32,
}

/// `[timeouts]`。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileTimeouts {
    pub connect_secs: Option<u64>,
    pub first_byte_secs: Option<u64>,
    pub read_idle_secs: Option<u64>,
}

/// `[keepalive]`。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileKeepalive {
    pub interval_secs: Option<u64>,
    pub trigger: Option<KeepaliveTrigger>,
    pub heartbeat: Option<String>,
}

/// `[retry]`。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileRetry {
    pub max_backoff_secs: Option<u64>,
    pub rules: Option<Vec<RetryRule>>,
}

/// `[history]`。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileHistory {
    pub body_budget_mb: Option<u64>,
}

/// 一份 config.toml 说了什么。没写的键是 None。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileConfig {
    pub base_url: Option<String>,
    pub listen_addr: Option<String>,
    pub api_key: Option<String>,
    pub extra_headers: Option<BTreeMap<String, String>>,
    pub override_headers: Option<BTreeMap<String, String>>,
    pub proxy: Option<String>,
    pub proxy_username: Option<String>,
    pub proxy_password: Option<String>,
    pub max_request_mb: Option<u64>,
    pub max_response_mb: Option<u64>,
    pub disk_cache: Option<bool>,
    pub delivery: Option<Delivery>,
    pub allowed_hosts: Option<Vec<String>>,
    pub allowed_origins: Option<Vec<String>>,
    pub log_file: Option<String>,
    pub timeouts: FileTimeouts,
    pub keepalive: FileKeepalive,
    pub retry: FileRetry,
    pub history: FileHistory,
    pub format: Option<Vec<FormatEntry>>,
}

/// `[[format]]` 各键的默认值（没写时用）。
pub const DEFAULT_FORMAT_POOL_MAX: u32 = 4;
pub const DEFAULT_FORMAT_WORKER_IDLE_SECS: u64 = 300;
pub const DEFAULT_FORMAT_TIMEOUT_SECS: u64 = 30;
pub const DEFAULT_FORMAT_DECIDE_TIMEOUT_SECS: u64 = 2;
pub const DEFAULT_FORMAT_BREAKER_FAILURES: u32 = 5;
pub const DEFAULT_FORMAT_BREAKER_COOLDOWN_SECS: u64 = 30;

/// 解析一份 config.toml 的文本。`file` 只用来填诊断的 `file` 字段。
///
/// 语法错误时返回空的 [`FileConfig`] 与一条 `config_syntax`；否则尽量读出每个键，把遇到的
/// 问题都报出来（一次看到全部，而不是改一个再报下一个）。
pub fn parse(text: &str, file: &Path) -> (FileConfig, Vec<Diagnostic>) {
    let mut ctx = Ctx {
        text,
        file: file.display().to_string(),
        diagnostics: Vec::new(),
    };
    let document = match toml_edit::Document::parse(text) {
        Ok(document) => document,
        Err(e) => {
            let position = e.span().map(|span| line_column(text, span.start));
            ctx.diagnostics.push(Diagnostic {
                code: "config_syntax",
                severity: Level::Error,
                file: ctx.file.clone(),
                key: None,
                line: position.map(|p| p.0),
                column: position.map(|p| p.1),
                message: format!("TOML syntax error: {}", e.message()),
                details: Map::new(),
            });
            return (FileConfig::default(), ctx.diagnostics);
        }
    };

    let root = Tbl::new(document.as_table(), String::new());
    let mut config = FileConfig {
        base_url: root.string(&mut ctx, "base_url"),
        listen_addr: root.string(&mut ctx, "listen_addr"),
        api_key: root.string(&mut ctx, "api_key"),
        extra_headers: root.string_map(&mut ctx, "extra_headers"),
        override_headers: root.string_map(&mut ctx, "override_headers"),
        proxy: root.string(&mut ctx, "proxy"),
        proxy_username: root.string(&mut ctx, "proxy_username"),
        proxy_password: root.string(&mut ctx, "proxy_password"),
        max_request_mb: root.integer(&mut ctx, "max_request_mb"),
        max_response_mb: root.integer(&mut ctx, "max_response_mb"),
        disk_cache: root.boolean(&mut ctx, "disk_cache"),
        delivery: root.one_of(
            &mut ctx,
            "delivery",
            &[("forward", Delivery::Forward), ("retry", Delivery::Retry)],
        ),
        allowed_hosts: root.string_list(&mut ctx, "allowed_hosts"),
        allowed_origins: root.string_list(&mut ctx, "allowed_origins"),
        log_file: root.string(&mut ctx, "log_file"),
        ..FileConfig::default()
    };

    if let Some(t) = root.table(&mut ctx, "timeouts") {
        config.timeouts = FileTimeouts {
            connect_secs: t.integer(&mut ctx, "connect_secs"),
            first_byte_secs: t.integer(&mut ctx, "first_byte_secs"),
            read_idle_secs: t.integer(&mut ctx, "read_idle_secs"),
        };
        t.finish(&mut ctx);
    }
    if let Some(t) = root.table(&mut ctx, "keepalive") {
        config.keepalive = FileKeepalive {
            interval_secs: t.integer(&mut ctx, "interval_secs"),
            trigger: read_keepalive_trigger(&mut ctx, &t),
            heartbeat: t.string(&mut ctx, "heartbeat"),
        };
        t.finish(&mut ctx);
    }
    if let Some(t) = root.table(&mut ctx, "retry") {
        config.retry.max_backoff_secs = t.integer(&mut ctx, "max_backoff_secs");
        if let Some(rules) = t.tables(&mut ctx, "rules") {
            config.retry.rules = Some(
                rules
                    .iter()
                    .filter_map(|rule| read_retry_rule(&mut ctx, rule))
                    .collect(),
            );
        }
        t.finish(&mut ctx);
    }
    if let Some(t) = root.table(&mut ctx, "history") {
        config.history.body_budget_mb = t.integer(&mut ctx, "body_budget_mb");
        t.finish(&mut ctx);
    }
    if let Some(entries) = root.tables(&mut ctx, "format") {
        let formats: Vec<FormatEntry> = entries
            .iter()
            .filter_map(|entry| read_format(&mut ctx, entry))
            .collect();
        check_format_chain(&mut ctx, &formats, &entries);
        config.format = Some(formats);
    }
    root.finish(&mut ctx);
    (config, ctx.diagnostics)
}

/// `keepalive.trigger`。`always` 只能由 format 按请求给：写进配置会破坏非流式的客户端
/// （每个请求都先收到 SSE 的头），所以这里单独拒绝它，说明原因。
fn read_keepalive_trigger(ctx: &mut Ctx, t: &Tbl) -> Option<KeepaliveTrigger> {
    let (value, span) = t.raw_string(ctx, "trigger")?;
    if value == "always" {
        ctx.push(
            "config_invalid_value",
            &t.key_path("trigger"),
            span,
            "keepalive.trigger = \"always\" cannot be set in the config file: it would break non-streaming clients. Only a format can ask for it, per request".to_string(),
            json!({ "reason": "per_request_only" }),
        );
        return None;
    }
    let parsed = KeepaliveTrigger::parse(&value);
    if parsed.is_none() {
        ctx.push(
            "config_invalid_value",
            &t.key_path("trigger"),
            span,
            format!(
                "keepalive.trigger must be \"accept\", \"body_stream\" or \"any\", not {value:?}"
            ),
            json!({ "reason": "not_one_of" }),
        );
    }
    parsed
}

/// 一条重试规则。必填的 `max_attempts` 缺了，或某个匹配器写错了，整条规则不要：半条规则
/// 比没有规则更危险（少一个匹配器 = 更宽）。
fn read_retry_rule(ctx: &mut Ctx, rule: &Tbl) -> Option<RetryRule> {
    let before = ctx.error_count();
    let path = rule.string(ctx, "path");
    if let Some(pattern) = &path
        && let Err(e) = super::Config::compile_bounded_retry_pattern(pattern)
    {
        ctx.push(
            "config_invalid_value",
            &rule.key_path("path"),
            rule.span_of("path"),
            format!("not a valid regular expression: {e}"),
            json!({ "reason": "invalid_regex" }),
        );
    }
    let status = rule.integer_list(ctx, "status").map(|codes| {
        codes
            .into_iter()
            .filter_map(|code| match u16::try_from(code) {
                Ok(code) if (100..=599).contains(&code) => Some(code),
                _ => {
                    ctx.push(
                        "config_invalid_value",
                        &rule.key_path("status"),
                        rule.span_of("status"),
                        format!("{code} is not an HTTP status code (100-599)"),
                        json!({ "reason": "out_of_range" }),
                    );
                    None
                }
            })
            .collect()
    });
    // 只定了形状、还没实现的匹配器：明说不支持，而不是当成不认识的键
    if rule.touch("body") {
        ctx.push(
            "config_not_supported",
            &rule.key_path("body"),
            rule.span_of("body"),
            "the body matcher of retry rules is not implemented in this version".to_string(),
            json!({}),
        );
    }
    let max_attempts = rule
        .integer(ctx, "max_attempts")
        .and_then(|n| narrow(ctx, rule, "max_attempts", n));
    if max_attempts.is_none() && !rule.has("max_attempts") {
        ctx.push(
            "config_missing_key",
            &rule.key_path("max_attempts"),
            rule.span(),
            "a retry rule needs max_attempts (0 means no limit)".to_string(),
            json!({}),
        );
    }
    rule.finish(ctx);
    (ctx.error_count() == before).then(|| RetryRule {
        path,
        status,
        max_attempts: max_attempts.unwrap_or(0),
    })
}

/// 一条 `[[format]]`。有 error 级问题的条目不要：带着一个配错的程序启动，不如不启动。
fn read_format(ctx: &mut Ctx, entry: &Tbl) -> Option<FormatEntry> {
    let before = ctx.error_count();
    let command = entry.string(ctx, "command");
    match &command {
        None if !entry.has("command") => ctx.push(
            "config_missing_key",
            &entry.key_path("command"),
            entry.span(),
            "a format needs command".to_string(),
            json!({}),
        ),
        Some(command) if command.trim().is_empty() => ctx.push(
            "config_invalid_value",
            &entry.key_path("command"),
            entry.span_of("command"),
            "command must not be empty".to_string(),
            json!({ "reason": "empty" }),
        ),
        #[cfg(windows)]
        Some(command)
            if Path::new(command.trim().trim_end_matches([' ', '.']))
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("cmd") || ext.eq_ignore_ascii_case("bat")) =>
        {
            ctx.push(
                "config_invalid_value",
                &entry.key_path("command"),
                entry.span_of("command"),
                "batch files cannot be format commands; put the interpreter in command and the script in args".into(),
                json!({ "reason": "batch_file" }),
            )
        }
        _ => {}
    }
    let name = entry.string(ctx, "name").or_else(|| {
        // 默认名：command 的文件名主干（`/opt/x/aproxy-format.exe` → `aproxy-format`）
        command.as_deref().and_then(|command| {
            Path::new(command.trim())
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
        })
    });
    if name.as_deref().is_some_and(|name| name.trim().is_empty()) {
        ctx.push(
            "config_invalid_value",
            &entry.key_path("name"),
            entry.span_of("name"),
            "name must not be empty".to_string(),
            json!({ "reason": "empty" }),
        );
    }
    let args = entry.string_list(ctx, "args").unwrap_or_default();
    let env = entry.string_map(ctx, "env").unwrap_or_default();
    let process = entry
        .one_of(
            ctx,
            "process",
            &[
                ("spawn", Process::Spawn),
                ("persistent", Process::Persistent),
            ],
        )
        .unwrap_or(Process::Spawn);

    let mut stages = Vec::new();
    match entry.string_list(ctx, "stages") {
        None if !entry.has("stages") => ctx.push(
            "config_missing_key",
            &entry.key_path("stages"),
            entry.span(),
            "a format needs stages".to_string(),
            json!({}),
        ),
        None => {}
        Some(names) => {
            if names.is_empty() {
                ctx.push(
                    "config_invalid_value",
                    &entry.key_path("stages"),
                    entry.span_of("stages"),
                    "stages must name at least one stage".to_string(),
                    json!({ "reason": "empty" }),
                );
            }
            for stage in names {
                match Stage::parse(&stage) {
                    Some(stage) if !stages.contains(&stage) => stages.push(stage),
                    Some(_) => {}
                    // 只定了形状、还没实现的阶段
                    None if stage == "admit" => ctx.push(
                        "config_not_supported",
                        &entry.key_path("stages"),
                        entry.span_of("stages"),
                        "the admit stage is not implemented in this version".to_string(),
                        json!({}),
                    ),
                    None => ctx.push(
                        "config_invalid_value",
                        &entry.key_path("stages"),
                        entry.span_of("stages"),
                        format!(
                            "{stage:?} is not a stage; the stages are request, retry, decide, response, stream, heartbeat and notify"
                        ),
                        json!({ "reason": "not_one_of" }),
                    ),
                }
            }
        }
    }

    let required = entry.boolean(ctx, "required").unwrap_or(true);
    let pool_max = entry
        .integer(ctx, "pool_max")
        .and_then(|n| narrow(ctx, entry, "pool_max", n))
        .unwrap_or(DEFAULT_FORMAT_POOL_MAX);
    if pool_max == 0 {
        ctx.push(
            "config_invalid_value",
            &entry.key_path("pool_max"),
            entry.span_of("pool_max"),
            "pool_max must be at least 1".to_string(),
            json!({ "reason": "out_of_range" }),
        );
    }
    let worker_idle_secs = entry
        .integer(ctx, "worker_idle_secs")
        .unwrap_or(DEFAULT_FORMAT_WORKER_IDLE_SECS);
    let timeout_secs = entry
        .integer(ctx, "timeout_secs")
        .unwrap_or(DEFAULT_FORMAT_TIMEOUT_SECS);
    let decide_timeout_secs = entry
        .integer(ctx, "decide_timeout_secs")
        .unwrap_or(DEFAULT_FORMAT_DECIDE_TIMEOUT_SECS);
    let breaker_failures = entry
        .integer(ctx, "breaker_failures")
        .and_then(|n| narrow(ctx, entry, "breaker_failures", n))
        .unwrap_or(DEFAULT_FORMAT_BREAKER_FAILURES);
    let breaker_cooldown_secs = entry
        .integer(ctx, "breaker_cooldown_secs")
        .unwrap_or(DEFAULT_FORMAT_BREAKER_COOLDOWN_SECS);
    if breaker_cooldown_secs == 0 {
        // 冷却为 0 等于熔断之后每条消息都去试探，熔断形同虚设
        ctx.push(
            "config_invalid_value",
            &entry.key_path("breaker_cooldown_secs"),
            entry.span_of("breaker_cooldown_secs"),
            "breaker_cooldown_secs must not be 0; set breaker_failures = 0 to turn the breaker off"
                .to_string(),
            json!({ "reason": "out_of_range" }),
        );
    }

    // 一个程序自己的阶段之间的约束
    let has = |stage| stages.contains(&stage);
    if has(Stage::Response) && has(Stage::Stream) {
        ctx.push(
            "config_conflict",
            &entry.key_path("stages"),
            entry.span_of("stages"),
            "a format cannot declare both response and stream".to_string(),
            json!({}),
        );
    }
    if (has(Stage::Stream) || has(Stage::Notify)) && process != Process::Persistent {
        ctx.push(
            "config_conflict",
            &entry.key_path("process"),
            entry.span_of("stages"),
            "a format that declares stream or notify must use process = \"persistent\"".to_string(),
            json!({}),
        );
    }
    entry.finish(ctx);
    (ctx.error_count() == before).then(|| FormatEntry {
        name: name.unwrap_or_default().trim().to_string(),
        command: command.unwrap_or_default().trim().to_string(),
        args,
        env,
        process,
        stages,
        required,
        pool_max,
        worker_idle_secs,
        timeout_secs,
        decide_timeout_secs,
        breaker_failures,
        breaker_cooldown_secs,
    })
}

/// 条目之间的约束：名字在列表内唯一；请求方向上，声明了 `retry` 的程序之后（离上游更近的
/// 一侧）参与请求方向的程序也都要声明 `retry`——重试时前段的程序重新给出了请求，后段若不
/// 重做，发出去的就是上一次的结果。
fn check_format_chain(ctx: &mut Ctx, formats: &[FormatEntry], tables: &[Tbl]) {
    // 有问题的条目已被丢掉，formats 与 tables 的下标未必对应：按名字、命令找回它的表，只为
    // 给诊断定位；找不到就不带位置
    let position = |format: &FormatEntry| {
        tables
            .iter()
            .find(|t| {
                t.peek_string("command").as_deref().map(str::trim) == Some(format.command.as_str())
                    && t.peek_string("name")
                        .map(|name| name.trim().to_string())
                        .is_none_or(|name| name == format.name)
            })
            .map(|t| (t.path.clone(), t.span()))
    };
    let mut seen = HashSet::new();
    let mut retry_seen: Option<&str> = None;
    for format in formats {
        let (path, span) = position(format).unwrap_or_else(|| ("format".to_string(), None));
        if !seen.insert(format.name.as_str()) {
            ctx.push(
                "config_conflict",
                &format!("{path}.name"),
                span.clone(),
                format!(
                    "two formats are named {:?}; names must be unique in the list",
                    format.name
                ),
                json!({}),
            );
        }
        if format.has_stage(Stage::Retry) {
            retry_seen.get_or_insert(format.name.as_str());
        } else if let Some(earlier) = retry_seen
            && format.has_stage(Stage::Request)
        {
            ctx.push(
                "config_conflict",
                &format!("{path}.stages"),
                span,
                format!(
                    "format {:?} comes after {earlier:?}, which declares retry, so it must declare retry too: on a retry the earlier format produces a new request and this one has to process it again",
                    format.name
                ),
                json!({}),
            );
        }
    }
}

/// 把读到的整数收窄成 u32；超出时报 `config_invalid_value`。
fn narrow(ctx: &mut Ctx, table: &Tbl, key: &'static str, value: u64) -> Option<u32> {
    match u32::try_from(value) {
        Ok(value) => Some(value),
        Err(_) => {
            ctx.push(
                "config_invalid_value",
                &table.key_path(key),
                table.span_of(key),
                format!("{value} is too large"),
                json!({ "reason": "out_of_range" }),
            );
            None
        }
    }
}

/// 键路径 `key` 在原文里的位置（行、列，从 1 起）。路径里的某一段不在文件里时，落在已经
/// 找到的最深一段上（例如条目没写 `process`，就指到那个条目）；一段都找不到、或文件有语法
/// 错误时没有位置。
pub fn locate(text: &str, key: &str) -> Option<(u32, u32)> {
    let document = toml_edit::Document::parse(text).ok()?;
    let segments = super::parse_key_path(key)?;
    let mut table: &dyn TableLike = document.as_table();
    let mut found = None;
    for segment in &segments {
        let Some((key, item)) = table.get_key_value(&segment.name) else {
            break;
        };
        found = key.span().or(found);
        let next: Option<&dyn TableLike> = match segment.index {
            None => item.as_table_like(),
            Some(index) => {
                if let Some(tables) = item.as_array_of_tables() {
                    tables.get(index).map(|t| t as &dyn TableLike)
                } else {
                    item.as_array()
                        .and_then(|array| array.get(index))
                        .and_then(toml_edit::Value::as_inline_table)
                        .map(|t| t as &dyn TableLike)
                }
            }
        };
        let Some(next) = next else { break };
        if segment.index.is_some() {
            // 数组里的一个条目：位置取它的第一个键
            let first = next
                .iter()
                .filter_map(|(name, _)| next.get_key_value(name).and_then(|(key, _)| key.span()))
                .min_by_key(|span| span.start);
            found = first.or(found);
        }
        table = next;
    }
    found.map(|span| line_column(text, span.start))
}

// ---------------------------------------------------------------------------
// 读表的工具：每读一个键就记下「这个键认识」，读完后剩下的都是不认识的键
// ---------------------------------------------------------------------------

/// 解析过程中的共享状态：原文（算行列用）、文件名、攒下的诊断。
struct Ctx<'a> {
    text: &'a str,
    file: String,
    diagnostics: Vec<Diagnostic>,
}

impl Ctx<'_> {
    /// 记一条 error 级诊断。`span` 是原文里的字节范围，取它的起点算行列
    fn push(
        &mut self,
        code: &'static str,
        key: &str,
        span: Option<std::ops::Range<usize>>,
        message: String,
        details: Value,
    ) {
        let position = span.map(|span| line_column(self.text, span.start));
        self.diagnostics.push(Diagnostic {
            code,
            severity: Level::Error,
            file: self.file.clone(),
            key: Some(key.to_string()),
            line: position.map(|p| p.0),
            column: position.map(|p| p.1),
            message,
            details: match details {
                Value::Object(map) => map,
                _ => Map::new(),
            },
        });
    }

    fn error_count(&self) -> usize {
        self.diagnostics.len()
    }
}

/// 一张表（`[x]`、点号键形成的表，或内联表 `{…}`），连同它的键路径与已经读过的键。
struct Tbl<'a> {
    table: &'a dyn TableLike,
    /// 这张表的键路径：根为空串，其余如 `timeouts`、`format[1]`、`retry.rules[0]`
    path: String,
    seen: std::cell::RefCell<HashSet<&'static str>>,
}

impl<'a> Tbl<'a> {
    fn new(table: &'a dyn TableLike, path: String) -> Self {
        Tbl {
            table,
            path,
            seen: Default::default(),
        }
    }

    /// 键 `key` 的完整路径
    fn key_path(&self, key: &str) -> String {
        if self.path.is_empty() {
            key.to_string()
        } else {
            format!("{}.{key}", self.path)
        }
    }

    /// 取一个键的值，并记下这个键认识
    fn get(&self, key: &'static str) -> Option<&'a Item> {
        self.seen.borrow_mut().insert(key);
        self.table.get(key).filter(|item| !item.is_none())
    }

    /// 记下这个键认识，返回它在不在
    fn touch(&self, key: &'static str) -> bool {
        self.get(key).is_some()
    }

    /// 这个键在不在（不改变「认识」的记录）
    fn has(&self, key: &str) -> bool {
        self.table.get(key).is_some_and(|item| !item.is_none())
    }

    /// 键 `key` 在原文里的位置
    fn span_of(&self, key: &str) -> Option<std::ops::Range<usize>> {
        self.table
            .get_key_value(key)
            .and_then(|(key, _)| key.span())
    }

    /// 这张表在原文里的位置：取它第一个键的位置（表头本身的位置 toml_edit 不总是给）
    fn span(&self) -> Option<std::ops::Range<usize>> {
        self.table
            .iter()
            .filter_map(|(key, _)| self.span_of(key))
            .min_by_key(|span| span.start)
    }

    /// 不记诊断、不改「认识」的记录，只看一个字符串键的值
    fn peek_string(&self, key: &str) -> Option<String> {
        self.table
            .get(key)
            .and_then(Item::as_str)
            .map(str::to_string)
    }

    fn wrong_type(&self, ctx: &mut Ctx, key: &str, expected: &str) {
        ctx.push(
            "config_type",
            &self.key_path(key),
            self.span_of(key),
            format!("{} must be {expected}", self.key_path(key)),
            json!({}),
        );
    }

    fn raw_string(
        &self,
        ctx: &mut Ctx,
        key: &'static str,
    ) -> Option<(String, Option<std::ops::Range<usize>>)> {
        let item = self.get(key)?;
        match item.as_str() {
            Some(value) => Some((value.to_string(), self.span_of(key))),
            None => {
                self.wrong_type(ctx, key, "a string");
                None
            }
        }
    }

    fn string(&self, ctx: &mut Ctx, key: &'static str) -> Option<String> {
        self.raw_string(ctx, key).map(|(value, _)| value)
    }

    /// 非负整数
    fn integer(&self, ctx: &mut Ctx, key: &'static str) -> Option<u64> {
        let item = self.get(key)?;
        match item.as_integer() {
            Some(value) => match u64::try_from(value) {
                Ok(value) => Some(value),
                Err(_) => {
                    ctx.push(
                        "config_invalid_value",
                        &self.key_path(key),
                        self.span_of(key),
                        format!("{} must not be negative", self.key_path(key)),
                        json!({ "reason": "out_of_range" }),
                    );
                    None
                }
            },
            None => {
                self.wrong_type(ctx, key, "an integer");
                None
            }
        }
    }

    fn boolean(&self, ctx: &mut Ctx, key: &'static str) -> Option<bool> {
        let item = self.get(key)?;
        let value = item.as_bool();
        if value.is_none() {
            self.wrong_type(ctx, key, "true or false");
        }
        value
    }

    /// 取值只能是列出的几个字符串之一
    fn one_of<T: Copy>(
        &self,
        ctx: &mut Ctx,
        key: &'static str,
        choices: &[(&str, T)],
    ) -> Option<T> {
        let (value, span) = self.raw_string(ctx, key)?;
        let found = choices
            .iter()
            .find(|(name, _)| *name == value)
            .map(|(_, v)| *v);
        if found.is_none() {
            let names: Vec<String> = choices
                .iter()
                .map(|(name, _)| format!("{name:?}"))
                .collect();
            ctx.push(
                "config_invalid_value",
                &self.key_path(key),
                span,
                format!(
                    "{} must be one of {}, not {value:?}",
                    self.key_path(key),
                    names.join(", ")
                ),
                json!({ "reason": "not_one_of" }),
            );
        }
        found
    }

    fn string_list(&self, ctx: &mut Ctx, key: &'static str) -> Option<Vec<String>> {
        let item = self.get(key)?;
        let values: Option<Vec<String>> = item.as_array().and_then(|array| {
            array
                .iter()
                .map(|value| value.as_str().map(str::to_string))
                .collect()
        });
        if values.is_none() {
            self.wrong_type(ctx, key, "an array of strings");
        }
        values
    }

    fn integer_list(&self, ctx: &mut Ctx, key: &'static str) -> Option<Vec<i64>> {
        let item = self.get(key)?;
        let values: Option<Vec<i64>> = item
            .as_array()
            .and_then(|array| array.iter().map(toml_edit::Value::as_integer).collect());
        if values.is_none() {
            self.wrong_type(ctx, key, "an array of integers");
        }
        values
    }

    /// 字符串到字符串的表（头表、env）。键是用户起的名字，不做「认识与否」的检查
    fn string_map(&self, ctx: &mut Ctx, key: &'static str) -> Option<BTreeMap<String, String>> {
        let item = self.get(key)?;
        let values: Option<BTreeMap<String, String>> = item.as_table_like().and_then(|table| {
            table
                .iter()
                .map(|(name, value)| Some((name.to_string(), value.as_str()?.to_string())))
                .collect()
        });
        if values.is_none() {
            self.wrong_type(ctx, key, "a table of strings");
        }
        values
    }

    /// 子表
    fn table(&self, ctx: &mut Ctx, key: &'static str) -> Option<Tbl<'a>> {
        let item = self.get(key)?;
        match item.as_table_like() {
            Some(table) => Some(Tbl::new(table, self.key_path(key))),
            None => {
                self.wrong_type(ctx, key, "a table");
                None
            }
        }
    }

    /// 表的数组：`[[x]]` 的写法，或内联表的数组 `x = [{…}, {…}]`
    fn tables(&self, ctx: &mut Ctx, key: &'static str) -> Option<Vec<Tbl<'a>>> {
        let item = self.get(key)?;
        let path = |index: usize| format!("{}[{index}]", self.key_path(key));
        if let Some(tables) = item.as_array_of_tables() {
            return Some(
                tables
                    .iter()
                    .enumerate()
                    .map(|(i, table)| Tbl::new(table as &dyn TableLike, path(i)))
                    .collect(),
            );
        }
        let inline: Option<Vec<Tbl>> = item.as_array().and_then(|array| {
            array
                .iter()
                .enumerate()
                .map(|(i, value)| {
                    value
                        .as_inline_table()
                        .map(|table| Tbl::new(table as &dyn TableLike, path(i)))
                })
                .collect()
        });
        if inline.is_none() {
            self.wrong_type(ctx, key, "an array of tables");
        }
        inline
    }

    /// 读完了：剩下没读过的键都是不认识的
    fn finish(&self, ctx: &mut Ctx) {
        let seen = self.seen.borrow();
        let unknown: Vec<String> = self
            .table
            .iter()
            .map(|(key, _)| key.to_string())
            .filter(|key| !seen.contains(key.as_str()))
            .collect();
        for key in unknown {
            ctx.push(
                "config_unknown_key",
                &self.key_path(&key),
                self.span_of(&key),
                format!("unknown key {}", self.key_path(&key)),
                json!({}),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_text(text: &str) -> (FileConfig, Vec<Diagnostic>) {
        parse(text, Path::new("config.toml"))
    }

    /// Windows 的批处理不用隐式 cmd 启动；显式解释器作为真正的子进程不受这个限制。
    #[cfg(windows)]
    #[test]
    fn windows_format_batch_targets_are_rejected_at_validation() {
        for command in ["tool.cmd", "TOOL.BaT", "tool.CMD."] {
            let (_, d) = parse_text(&format!(
                "[[format]]\ncommand='{command}'\nstages=['request']"
            ));
            assert!(d.iter().any(|d| {
                d.code == "config_invalid_value"
                    && d.message
                        .contains("interpreter in command and the script in args")
            }));
        }
        assert!(
            parse_text(
                "[[format]]\ncommand='interpreter.exe'\nargs=['script']\nstages=['request']"
            )
            .1
            .is_empty()
        );
    }
    /// 解析出的诊断，压成 `码 键路径 行号` 方便比对
    fn brief(diagnostics: &[Diagnostic]) -> Vec<String> {
        diagnostics
            .iter()
            .map(|d| {
                format!(
                    "{} {} {}",
                    d.code,
                    d.key.as_deref().unwrap_or("-"),
                    d.line.map_or("-".to_string(), |l| l.to_string())
                )
            })
            .collect()
    }

    /// 设计稿 §7.2 的样例
    const SAMPLE: &str = r#"base_url = "https://relay.example.com"
# delivery 用默认的 "retry"

[timeouts]
first_byte_secs = 600          # 长时间的非流式生成

[[retry.rules]]
path = '/v1/messages/count_tokens(\?.*)?'
max_attempts = 3

[[format]]
name = "adaptive"
command = "aproxy-format"
args = ["adaptive", "--config", "adaptive.toml"]
process = "persistent"
stages = ["request", "decide", "notify"]
required = false
pool_max = 1
worker_idle_secs = 0

[[format]]
name = "convert"
command = "aproxy-format"
args = ["convert", "--from", "anthropic_messages", "--to", "openai_chat"]
process = "persistent"
stages = ["request", "stream"]

[[format]]
name = "keys"
command = "aproxy-format"
args = ["keys", "--config", "keys.toml"]
process = "persistent"
stages = ["request", "retry"]
"#;

    #[test]
    fn the_documented_sample_parses() {
        let (config, diagnostics) = parse_text(SAMPLE);
        assert_eq!(brief(&diagnostics), Vec::<String>::new());
        assert_eq!(
            config.base_url.as_deref(),
            Some("https://relay.example.com")
        );
        assert_eq!(config.delivery, None, "没写的键是 None，不补默认值");
        assert_eq!(config.timeouts.first_byte_secs, Some(600));
        assert_eq!(config.timeouts.connect_secs, None);
        assert_eq!(
            config.retry.rules,
            Some(vec![RetryRule {
                path: Some(r"/v1/messages/count_tokens(\?.*)?".into()),
                status: None,
                max_attempts: 3,
            }])
        );
        let formats = config.format.unwrap();
        assert_eq!(
            formats.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
            ["adaptive", "convert", "keys"]
        );
        let adaptive = &formats[0];
        assert_eq!(adaptive.process, Process::Persistent);
        assert_eq!(
            adaptive.stages,
            [Stage::Request, Stage::Decide, Stage::Notify]
        );
        assert!(!adaptive.required);
        assert_eq!((adaptive.pool_max, adaptive.worker_idle_secs), (1, 0));
        // 没写的键取默认值
        let convert = &formats[1];
        assert!(convert.required);
        assert_eq!(convert.pool_max, DEFAULT_FORMAT_POOL_MAX);
        assert_eq!(convert.timeout_secs, DEFAULT_FORMAT_TIMEOUT_SECS);
        assert_eq!(convert.breaker_failures, DEFAULT_FORMAT_BREAKER_FAILURES);
    }

    #[test]
    fn every_key_of_the_table_is_read() {
        let (config, diagnostics) = parse_text(
            r#"
base_url = "https://api.example.com"
listen_addr = "127.0.0.1:12346"
api_key = "sk-x"
proxy = "http://127.0.0.1:7890"
proxy_username = "u"
proxy_password = "p"
max_request_mb = 64
max_response_mb = 0
disk_cache = false
delivery = "forward"
allowed_hosts = ["my-host"]
allowed_origins = ["http://localhost:5173"]
log_file = "logs/a.log"
keepalive = { interval_secs = 10, trigger = "accept", heartbeat = ": ping\n\n" }

[extra_headers]
anthropic-version = "2023-06-01"

[override_headers]
"x-api-key" = "k"

[timeouts]
connect_secs = 5
read_idle_secs = 120

[retry]
max_backoff_secs = 60
rules = [{ status = [404, 400], max_attempts = 0 }]

[history]
body_budget_mb = 8
"#,
        );
        assert_eq!(brief(&diagnostics), Vec::<String>::new());
        assert_eq!(config.listen_addr.as_deref(), Some("127.0.0.1:12346"));
        assert_eq!(config.max_response_mb, Some(0));
        assert_eq!(config.disk_cache, Some(false));
        assert_eq!(config.delivery, Some(Delivery::Forward));
        assert_eq!(config.allowed_hosts, Some(vec!["my-host".to_string()]));
        assert_eq!(
            config.extra_headers.unwrap()["anthropic-version"],
            "2023-06-01"
        );
        assert_eq!(config.override_headers.unwrap()["x-api-key"], "k");
        assert_eq!(config.timeouts.connect_secs, Some(5));
        assert_eq!(config.timeouts.read_idle_secs, Some(120));
        // 内联表
        assert_eq!(config.keepalive.interval_secs, Some(10));
        assert_eq!(config.keepalive.trigger, Some(KeepaliveTrigger::Accept));
        assert_eq!(config.keepalive.heartbeat.as_deref(), Some(": ping\n\n"));
        assert_eq!(config.retry.max_backoff_secs, Some(60));
        // 内联表的数组也是规则；0 = 不限
        assert_eq!(
            config.retry.rules,
            Some(vec![RetryRule {
                path: None,
                status: Some(vec![404, 400]),
                max_attempts: 0,
            }])
        );
        assert_eq!(config.history.body_budget_mb, Some(8));
    }

    /// 不认识的键一律是 error，带键路径与行号——顶层、子表、规则与 format 条目里都一样。
    /// 0.1.x 的旧键也只是「不认识」。
    #[test]
    fn unknown_keys_are_errors_everywhere() {
        let (_, diagnostics) = parse_text(
            r#"base_url = "https://api.example.com"
forward_only = true
max_body_mb = 1

[timeouts]
total_secs = 1

[keepalive]
enabled = true

[[retry.rules]]
max_attempts = 3
header = "x"

[[format]]
command = "f"
stages = ["request"]
mode = "persistent"

[request_transform]
command = "f"
"#,
        );
        assert_eq!(
            brief(&diagnostics),
            [
                "config_unknown_key timeouts.total_secs 6",
                "config_unknown_key keepalive.enabled 9",
                "config_unknown_key retry.rules[0].header 13",
                "config_unknown_key format[0].mode 18",
                "config_unknown_key forward_only 2",
                "config_unknown_key max_body_mb 3",
                "config_unknown_key request_transform 20",
            ]
        );
        assert!(
            diagnostics.iter().all(|d| d.severity == Level::Error
                && d.file == "config.toml"
                && d.column.is_some())
        );
        // 普通键从行首起；表头里的键在方括号之后
        assert_eq!(diagnostics[4].column, Some(1));
        assert_eq!(diagnostics[6].column, Some(2));
    }

    #[test]
    fn syntax_errors_carry_a_position() {
        let (config, diagnostics) = parse_text("base_url = \"x\"\nlisten_addr = \n");
        assert_eq!(config, FileConfig::default());
        assert_eq!(diagnostics.len(), 1);
        let d = &diagnostics[0];
        assert_eq!((d.code, d.key.as_deref()), ("config_syntax", None));
        assert_eq!(d.line, Some(2));
        assert!(d.column.is_some());
    }

    #[test]
    fn wrong_types_and_values() {
        let (config, diagnostics) = parse_text(
            r#"base_url = 1
max_request_mb = "big"
max_response_mb = -1
disk_cache = "yes"
delivery = "fast"
allowed_hosts = "host"
extra_headers = { a = 1 }
timeouts = 5

[keepalive]
trigger = "always"
"#,
        );
        assert_eq!(
            brief(&diagnostics),
            [
                "config_type base_url 1",
                "config_type extra_headers 7",
                "config_type max_request_mb 2",
                "config_invalid_value max_response_mb 3",
                "config_type disk_cache 4",
                "config_invalid_value delivery 5",
                "config_type allowed_hosts 6",
                "config_type timeouts 8",
                "config_invalid_value keepalive.trigger 11",
            ]
        );
        // 读不出来的键都是 None，不带进半对半错的值
        assert_eq!(config, FileConfig::default());
        let always = diagnostics.last().unwrap();
        assert_eq!(always.details["reason"], "per_request_only");
    }

    #[test]
    fn retry_rules_are_all_or_nothing() {
        let (config, diagnostics) = parse_text(
            r#"
[[retry.rules]]
path = "/ok"
max_attempts = 2

[[retry.rules]]
path = "/missing-max"

[[retry.rules]]
path = "("
max_attempts = 1

[[retry.rules]]
status = [99, 200, 600]
max_attempts = 1

[[retry.rules]]
body = "overloaded"
max_attempts = 1
"#,
        );
        assert_eq!(
            brief(&diagnostics),
            [
                "config_missing_key retry.rules[1].max_attempts 7",
                "config_invalid_value retry.rules[2].path 10",
                "config_invalid_value retry.rules[3].status 14",
                "config_invalid_value retry.rules[3].status 14",
                "config_not_supported retry.rules[4].body 18",
            ]
        );
        // 有问题的规则整条不要：少一个匹配器的规则比用户写的更宽
        assert_eq!(
            config.retry.rules,
            Some(vec![RetryRule {
                path: Some("/ok".into()),
                status: None,
                max_attempts: 2,
            }])
        );
    }

    #[test]
    fn format_entries_are_checked() {
        let (config, diagnostics) = parse_text(
            r#"
[[format]]
stages = ["request"]

[[format]]
command = "/opt/tools/keys.exe"
stages = ["request", "admit", "bogus"]

[[format]]
command = "both"
stages = ["response", "stream"]
process = "persistent"

[[format]]
command = "streamer"
stages = ["stream"]

[[format]]
command = "cool"
stages = ["request"]
breaker_cooldown_secs = 0
pool_max = 0
"#,
        );
        assert_eq!(
            brief(&diagnostics),
            [
                "config_missing_key format[0].command 3",
                "config_not_supported format[1].stages 7",
                "config_invalid_value format[1].stages 7",
                "config_conflict format[2].stages 11",
                "config_conflict format[3].process 16",
                "config_invalid_value format[4].pool_max 22",
                "config_invalid_value format[4].breaker_cooldown_secs 21",
            ]
        );
        assert_eq!(config.format, Some(vec![]), "有问题的条目都不要");
    }

    #[test]
    fn format_names_default_to_the_command_stem_and_must_be_unique() {
        let (config, diagnostics) = parse_text(
            r#"
[[format]]
command = "/opt/tools/aproxy-format.exe"
stages = ["request"]

[[format]]
name = "aproxy-format"
command = "other"
stages = ["response"]
"#,
        );
        let formats = config.format.unwrap();
        assert_eq!(formats[0].name, "aproxy-format");
        assert_eq!(brief(&diagnostics), ["config_conflict format[1].name 7"]);
    }

    /// 请求方向上，声明了 retry 的程序之后的程序也要声明 retry；只参与响应方向的不受限。
    #[test]
    fn formats_after_a_retry_format_must_declare_retry() {
        let (_, diagnostics) = parse_text(
            r#"
[[format]]
name = "route"
command = "f"
stages = ["request", "retry"]

[[format]]
name = "convert"
command = "f"
stages = ["request"]

[[format]]
name = "observe"
command = "f"
process = "persistent"
stages = ["notify"]

[[format]]
name = "keys"
command = "f"
stages = ["request", "retry"]
"#,
        );
        assert_eq!(brief(&diagnostics), ["config_conflict format[1].stages 8"]);
        assert!(diagnostics[0].message.contains("\"route\""));
    }

    #[test]
    fn locate_finds_keys_in_every_table_form() {
        let text = r#"base_url = "x"
keepalive = { interval_secs = 1 }
timeouts.connect_secs = 5

[extra_headers]
"x.y" = "v"

[[format]]
command = "a"

[[format]]
# 注释
command = "b"
stages = []

[retry]
rules = [{ max_attempts = 1 }, { path = "/x", max_attempts = 2 }]
"#;
        assert_eq!(locate(text, "base_url"), Some((1, 1)));
        assert_eq!(locate(text, "keepalive.interval_secs"), Some((2, 15)));
        assert_eq!(locate(text, "timeouts.connect_secs"), Some((3, 10)));
        assert_eq!(locate(text, "extra_headers.\"x.y\""), Some((6, 1)));
        assert_eq!(locate(text, "format[1].stages"), Some((14, 1)));
        // 没写的键落在条目上；不存在的条目落在数组的键上
        assert_eq!(locate(text, "format[1].process"), Some((13, 1)));
        assert_eq!(locate(text, "format[5].stages"), Some((8, 3)));
        assert_eq!(locate(text, "retry.rules[1].path"), Some((17, 34)));
        assert_eq!(locate(text, "nope"), None);
        assert_eq!(locate("x = ", "x"), None);
    }

    /// 每条诊断的码都在码表的诊断族里。
    #[test]
    fn every_code_is_a_registered_diagnostic_code() {
        let texts = [
            "x = ",
            "unknown = 1",
            "base_url = 1",
            "max_request_mb = -1",
            "[[retry.rules]]\npath = \"/x\"",
            "[[retry.rules]]\nbody = \"x\"\nmax_attempts = 1",
            "[[format]]\ncommand = \"f\"\nstages = [\"response\", \"stream\"]\nprocess = \"persistent\"",
        ];
        for text in texts {
            let (_, diagnostics) = parse_text(text);
            assert!(!diagnostics.is_empty(), "{text}");
            for d in diagnostics {
                let entry =
                    aproxy_ipc::code::lookup(d.code).unwrap_or_else(|| panic!("{}", d.code));
                assert!(
                    entry
                        .families
                        .contains(&aproxy_ipc::code::Family::Diagnostic),
                    "{}",
                    d.code
                );
            }
        }
    }
}
