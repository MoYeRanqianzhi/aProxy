//! `aproxy config show|get|set|unset`：查看与修改一份 config.toml。
//!
//! 读出口不给密钥的明文：`--json` 里密钥是占位对象 `{"secret":true}`，给人看的输出里是
//! `<secret>`；URL 里的口令打码。

use std::path::{Path, PathBuf};

use aproxy::cli::{ConfigArgs, ConfigCmd};
use aproxy::config::edit::{Change, EditError};
use aproxy::config::{self, Source, view};
use aproxy::settings;
use aproxy_ipc::code::{EXIT_CONFIG, EXIT_FAILED, EXIT_NOT_FOUND, EXIT_USAGE};
use serde_json::{Value, json};

use aproxy_ipc::redact::placeholder_text;

/// 转换器 args 的展示打码：密钥旗标之后的值（或 `--flag=值` 的值）写成占位的文本形式；
/// 其余参数走 URL 脱敏出口（参数里出现的 `user:pass@host`、`?key=` 一并遮掉，普通参数原样）。
fn mask_transform_args(args: &[String]) -> Vec<String> {
    let is_secret_flag = |s: &str| {
        aproxy::config::SECRET_ARG_FLAGS
            .iter()
            .any(|f| f.eq_ignore_ascii_case(s))
    };
    let mut out = Vec::with_capacity(args.len());
    let mut mask_next = false;
    for arg in args {
        if mask_next {
            out.push(placeholder_text(arg));
            mask_next = false;
        } else if is_secret_flag(arg) {
            out.push(arg.clone());
            mask_next = true;
        } else if let Some((flag, value)) = arg.split_once('=')
            && is_secret_flag(flag)
        {
            out.push(format!("{flag}={}", placeholder_text(value)));
        } else {
            out.push(aproxy_ipc::redact::url(arg));
        }
    }
    out
}

pub(crate) async fn handle_config_cmd(args: ConfigArgs) {
    let ConfigArgs { config, cmd } = args;
    // 给了 --config：别名优先；不是别名就当路径，文件可以还不存在（对一个新路径 set，就是
    // 在建一份新实例的配置）。没给：默认配置文件
    let explicit = config.is_some();
    let path = match config.as_deref() {
        Some(target) => settings::resolve_alias(target)
            .map(PathBuf::from)
            .unwrap_or_else(|| settings::expand_path(target)),
        None => super::require_cfg_path(super::resolve_cfg_path(None)),
    };
    match cmd {
        ConfigCmd::Show { effective, json } => show(&path, explicit, effective, json),
        ConfigCmd::Get { key, json } => get(&path, &key, json),
        ConfigCmd::Set { key, value, json } => {
            let value = parse_value(&key, &value, json);
            write(&path, Change::Set { key, value }, json).await
        }
        ConfigCmd::Unset { key, json } => write(&path, Change::Unset { key }, json).await,
    }
}

/// 整条命令失败：原因写到 stderr；`--json` 时 stdout 上是错误文档
/// `{"schema":1,"error":{"code","message","details"}}`。以 `exit` 退出。
fn fail(json: bool, code: &'static str, exit: u8, message: String, details: Value) -> ! {
    eprintln!("{message}");
    if json {
        print_doc(json!({"error": {"code": code, "message": message, "details": details}}));
    }
    crate::util::exit(exit as i32)
}

/// 把 `--json` 的文档写到 stdout：一行，`schema` 在最前面。
fn print_doc(body: Value) {
    let mut doc = serde_json::Map::new();
    doc.insert("schema".to_string(), 1.into());
    if let Value::Object(body) = body {
        doc.extend(body);
    }
    println!("{}", Value::Object(doc));
}

/// 读配置文件的文本；不存在时是 None。读不出来（权限、不是 UTF-8）时失败退出。
fn read_text(path: &Path) -> Option<String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            eprintln!("读取配置文件失败: {e}\n文件: {}", path.display());
            crate::util::exit(1)
        }
    }
}

fn global_defaults() -> config::resolve::Defaults {
    config::resolve::Defaults::from_settings(&settings::load(), &settings::settings_path())
}

/// 分层解析 `path` 处的配置（文件不存在时只剩全局默认与内置默认两层）。
fn resolve(path: &Path) -> config::Resolved {
    match config::resolve(path, &config::Overrides::default(), &global_defaults()) {
        Ok(resolved) => resolved,
        Err(e) => {
            eprintln!("读取配置文件失败: {e}\n文件: {}", path.display());
            crate::util::exit(1)
        }
    }
}

/// `config show`：文件里写了的值；`--effective` 时是生效的值、各自的来源与没有继承的凭据。
/// 文件有 error 级诊断时照样显示，然后以 3 退出。
fn show(path: &Path, explicit: bool, effective: bool, json: bool) {
    let text = read_text(path);
    // 显式 --config 指向的文件不存在时，展示的只会是默认值（抬头却是用户给的路径）——纯粹
    // 的排障误导，报错而不是照样显示。默认路径豁免：还没有 config.toml 是首次配置之前的
    // 正常状态
    if text.is_none() && explicit {
        fail(
            json,
            "target_not_found",
            EXIT_NOT_FOUND,
            format!("指定的配置文件不存在: {}", path.display()),
            json!({"target": path.display().to_string()}),
        );
    }
    let resolved = resolve(path);
    let file_values = text.as_deref().and_then(view::file_value);
    let values = view::redacted(if effective {
        view::effective_value(&resolved, file_values.as_ref())
    } else {
        file_values.clone().unwrap_or_else(|| json!({}))
    });

    if json {
        let mut doc = json!({
            "file": path.display().to_string(),
            "values": values,
        });
        if effective {
            doc["sources"] = json!(resolved.sources);
            doc["withheld"] = json!(resolved.withheld);
        }
        doc["diagnostics"] = json!(resolved.diagnostics);
        print_doc(doc);
    } else if effective {
        print_effective(path, &resolved);
        // 全局默认里没有继承的凭据：列出来，不然用户只会看到上游回 401
        if !resolved.withheld.is_empty() {
            println!();
            println!("全局默认里的这些凭据没有继承（它们属于另一个目标）:");
            for item in &resolved.withheld {
                println!(
                    "  {}  默认值属于 {}，这份配置指向 {}",
                    item.key,
                    item.default_target,
                    if item.target.is_empty() {
                        "(未设置)"
                    } else {
                        item.target.as_str()
                    }
                );
            }
        }
    } else {
        println!("# {}", path.display());
        if text.is_none() {
            eprintln!("配置文件还不存在；用 aproxy config set <键> <值> 写入第一个键时创建。");
        }
        print!("{}", view::to_document(&view::for_display(values)));
    }

    for d in resolved.diagnostics.iter().filter(|d| !d.is_error()) {
        eprintln!("警告: {}", aproxy::ops::start::render_diagnostic(d));
    }
    if !resolved.is_valid() {
        // 这份配置现在启动不了
        eprintln!();
        eprintln!("这份配置有错误，现在不能用来启动:");
        let errors: Vec<_> = resolved.errors().cloned().collect();
        print_diagnostics(&errors);
        if resolved.config.base_url.is_empty() {
            eprintln!();
            eprintln!("提示: base_url 为空，请设置:");
            eprintln!("  aproxy config set base_url https://api.anthropic.com");
        }
        let unknown: Vec<&str> = errors
            .iter()
            .filter(|d| d.code == "config_unknown_key")
            .filter_map(|d| d.key.as_deref())
            .collect();
        if !unknown.is_empty() {
            eprintln!();
            eprintln!("不认识的键可以这样删除:");
            for key in unknown {
                eprintln!("  aproxy config unset '{key}'");
            }
        }
        crate::util::exit(EXIT_CONFIG as i32)
    }
}

/// 键路径写法不对是用法错误（2），写法对但不认识是 `config_unknown_key`（3）。
fn require_known_key(key: &str, json: bool) -> view::KeyKind {
    if let Some(kind) = view::key_kind(key) {
        return kind;
    }
    let plain = config::parse_key_path(key)
        .is_some_and(|segments| segments.iter().all(|s| s.index.is_none()));
    if !plain {
        fail(
            json,
            "usage",
            EXIT_USAGE,
            format!(
                "键路径写法不对: {key}（点号分隔，例如 timeouts.connect_secs、extra_headers.\"x-foo\"；数组表只能整体读写）"
            ),
            json!({"reason": "bad_key_path"}),
        );
    }
    fail(
        json,
        "config_unknown_key",
        EXIT_CONFIG,
        format!("不认识的键: {key}（aproxy config show --effective 列出全部的键）"),
        json!({"key": key}),
    )
}

/// `config get`：一个键的生效值，与它来自哪一层。密钥给的是占位。
fn get(path: &Path, key: &str, json: bool) {
    require_known_key(key, json);
    let Some(text) = read_text(path) else {
        fail(
            json,
            "target_not_found",
            EXIT_NOT_FOUND,
            format!("配置文件不存在: {}", path.display()),
            json!({"target": path.display().to_string()}),
        );
    };
    let resolved = resolve(path);
    let Some(file_values) = view::file_value(&text) else {
        // 语法错误：文件里的值一个都读不出来，给出的只会是默认值
        let errors: Vec<_> = resolved.errors().cloned().collect();
        eprintln!("配置文件有语法错误:");
        print_diagnostics(&errors);
        fail(
            json,
            "config_invalid",
            EXIT_CONFIG,
            format!("读不出 {key}：先修复 {}", path.display()),
            json!({"diagnostics": errors}),
        );
    };
    let effective = view::redacted(view::effective_value(&resolved, Some(&file_values)));
    // 来源表里的键是规范写法（头名按需加引号），与用户怎么写这个键无关
    let canonical = config::parse_key_path(key)
        .map(|segments| {
            segments
                .iter()
                .map(|s| config::quote_key(&s.name))
                .collect::<Vec<_>>()
                .join(".")
        })
        .unwrap_or_default();
    let source = resolved.sources.get(&canonical);
    if json {
        let mut doc = json!({
            "key": key,
            "value": view::get(&effective, key).ok().flatten(),
        });
        if let Some(source) = source {
            doc["source"] = json!(source);
        }
        print_doc(doc);
        return;
    }
    let shown = view::for_display(effective);
    match view::get(&shown, key)
        .ok()
        .flatten()
        .and_then(view::json_to_toml)
    {
        Some(value) => {
            println!("{value}");
            eprintln!("（{}）", source_label(source.copied()));
        }
        None => eprintln!("(未设置)"),
    }
}

/// 命令行上的值 → TOML 值。
/// - `-`：从 stdin 读一行（传密钥用，不进命令行历史）。
/// - 字符串类型的键可以直接写原文；以 `"` 开头时按 TOML 字符串字面量解析（用来写换行等）。
///   写回一个占位（`{ secret = true }`）表示保持原值。
/// - 其余的键只收 TOML 值。
fn parse_value(key: &str, raw: &str, json: bool) -> toml_edit::Value {
    let kind = require_known_key(key, json);
    let raw = if raw == "-" {
        crate::util::read_secret_line().unwrap_or_else(|e| {
            eprintln!("{e}");
            crate::util::exit(1)
        })
    } else {
        raw.to_string()
    };
    match value_from_text(kind, &raw) {
        Ok(value) => value,
        Err(e) => fail(
            json,
            "usage",
            EXIT_USAGE,
            format!(
                "{key} 的值要写成 TOML（例如 true、30、\"text\"、[\"a\"]、{{ k = 1 }}），解析失败: {e}"
            ),
            json!({"reason": "bad_value"}),
        ),
    }
}

fn value_from_text(kind: view::KeyKind, raw: &str) -> Result<toml_edit::Value, String> {
    let parsed = raw.trim().parse::<toml_edit::Value>();
    if kind == view::KeyKind::Text {
        return match parsed {
            Ok(value) if raw.starts_with('"') => Ok(value),
            // 以引号开头却不是一个完整的字符串字面量：多半是引号没配对，不当原文收下
            Err(e) if raw.starts_with('"') => Err(e.to_string()),
            Ok(value) if aproxy_ipc::redact::is_placeholder(&view::toml_to_json(&value)) => {
                Ok(value)
            }
            _ => Ok(toml_edit::Value::from(raw)),
        };
    }
    parsed.map_err(|e| e.to_string())
}

/// `config set` / `config unset`：只改这一个键，文件里的注释、键的顺序与其余内容原样保留。
/// 会引入新错误的改动不写入；文件里原有的、与这次改动无关的错误不挡路，可以逐项修。
async fn write(path: &Path, change: Change, json: bool) {
    let key = match &change {
        Change::Set { key, .. } | Change::Unset { key } => key.clone(),
    };
    let applied = match config::edit::apply(
        path,
        &aproxy_ipc::home::home(),
        &global_defaults(),
        &[change],
    ) {
        Ok(applied) => applied,
        Err(EditError::BadKey(key)) => fail(
            json,
            "usage",
            EXIT_USAGE,
            format!(
                "键路径写法不对: {key}（点号分隔，例如 timeouts.connect_secs、extra_headers.\"x-foo\"；数组表只能整体读写）"
            ),
            json!({"reason": "bad_key_path"}),
        ),
        Err(EditError::Syntax(diagnostics)) => {
            eprintln!("现有配置文件有语法错误，没有修改:");
            print_diagnostics(&diagnostics);
            fail(
                json,
                "config_invalid",
                EXIT_CONFIG,
                format!("先修复或删除 {}", path.display()),
                json!({"diagnostics": diagnostics}),
            )
        }
        Err(EditError::Invalid(diagnostics)) => {
            print_diagnostics(&diagnostics);
            fail(
                json,
                "config_invalid",
                EXIT_CONFIG,
                "这次修改会让配置无效，没有保存".to_string(),
                json!({"diagnostics": diagnostics}),
            )
        }
        Err(EditError::Conflict) => fail(
            json,
            "write_conflict",
            EXIT_FAILED,
            format!(
                "配置文件一直在被别的程序修改，没有保存（关掉正在编辑它的程序后重试）: {}",
                path.display()
            ),
            json!({"file": path.display().to_string()}),
        ),
        Err(EditError::LockBusy) => fail(
            json,
            "write_conflict",
            EXIT_FAILED,
            "另一个 aproxy 正在写配置，等了 10 秒仍未结束，没有保存（稍后重试）".to_string(),
            json!({"file": path.display().to_string()}),
        ),
        Err(EditError::Io(e)) => {
            eprintln!("保存配置失败: {e}");
            crate::util::exit(EXIT_FAILED as i32)
        }
    };

    // 用着这份配置的运行中的实例：它们读的是启动时的配置，要重启才用上这次改动
    let ports = if applied.changed {
        instances_using(&applied.file, path).await
    } else {
        Vec::new()
    };
    if json {
        // 写完之后文件里这个键的值（删掉了就是 null），密钥给的是占位
        let value = std::fs::read_to_string(&applied.file)
            .ok()
            .and_then(|text| view::file_value(&text))
            .map(view::redacted)
            .and_then(|values| view::get(&values, &key).ok().flatten().cloned());
        print_doc(json!({
            "file": applied.file.display().to_string(),
            "key": key,
            "value": value,
            "changed": applied.changed,
            "instances_to_restart": ports,
        }));
        return;
    }
    if applied.changed {
        println!("已保存到 {}", applied.file.display());
    } else {
        println!("没有变化: {}", applied.file.display());
    }
    if !ports.is_empty() {
        eprintln!("运行中的实例要重启才用上这次改动:");
        for port in ports {
            eprintln!("  aproxy restart {port}");
        }
    }
}

/// 用着这份配置文件的运行中的实例的端口，从小到大。`written` 是实际写的文件（目标是符号
/// 链接时是它指向的那个），`given` 是定位到的路径：实例记下的可能是任何一个。
async fn instances_using(written: &Path, given: &Path) -> Vec<u16> {
    let keys = [
        settings::path_match_key(&written.display().to_string()),
        settings::path_match_key(&given.display().to_string()),
    ];
    let survey = aproxy::daemon::survey_instances().await;
    let mut ports: Vec<u16> = survey
        .responsive
        .iter()
        .map(|status| &status.instance)
        .chain(survey.unresponsive.iter())
        .filter(|instance| {
            instance
                .config_path
                .as_deref()
                .is_some_and(|path| keys.contains(&settings::path_match_key(path)))
        })
        .filter_map(|instance| aproxy::daemon::port_of(&instance.listen_addr).parse().ok())
        .collect();
    ports.sort_unstable();
    ports.dedup();
    ports
}

/// 逐条打印诊断（stderr）：`文件:行:列: 键: 说明 [码]`。
fn print_diagnostics(diagnostics: &[aproxy_ipc::diag::Diagnostic]) {
    for d in diagnostics {
        eprintln!("  {}", aproxy::ops::start::render_diagnostic(d));
    }
}

/// 来源的中文说法（展示在每个值后面）。
fn source_label(source: Option<Source>) -> &'static str {
    match source {
        Some(Source::Override) => "命令行",
        Some(Source::Toml) => "config.toml",
        Some(Source::Settings) => "settings.json 全局默认",
        Some(Source::Env) => "环境变量",
        Some(Source::Default) => "内置默认",
        None => "未设置",
    }
}

/// 打印一份配置的生效值：每个键一行，后面注明它来自哪一层。密钥只显示打码后的样子。
fn print_effective(path: &Path, resolved: &config::Resolved) {
    let cfg = &resolved.config;
    let line = |key: &str, value: String| {
        println!(
            "{key:<26} = {value}  （{}）",
            source_label(resolved.sources.get(key).copied())
        );
    };
    let quoted = |s: &str| format!("\"{s}\"");
    let optional = |v: Option<String>| v.unwrap_or_else(|| "(未设置)".to_string());
    // 0 在这几个键上有特别的含义，写在值后面
    let with_zero = |n: u64, zero: &str| {
        if n == 0 {
            format!("0（{zero}）")
        } else {
            n.to_string()
        }
    };
    let list = |items: &[String], empty: &str| {
        if items.is_empty() {
            format!("[]（{empty}）")
        } else {
            // 原样列出（不用 {:?}：Debug 会把正则里的 \? 写成 \\?，对照 toml 会被误导）
            format!("[{}]", items.join(", "))
        }
    };

    println!("配置文件: {}", path.display());
    line("base_url", quoted(&aproxy_ipc::redact::url(&cfg.base_url)));
    line("listen_addr", quoted(&cfg.listen_addr));
    line(
        "api_key",
        optional(cfg.api_key.as_deref().map(placeholder_text)),
    );
    line(
        "delivery",
        if cfg.forward_only_enabled() {
            "\"forward\"（边收边转：不缓冲、不重试）".to_string()
        } else {
            "\"retry\"".to_string()
        },
    );
    line(
        "max_request_mb",
        with_zero(
            cfg.max_body_mb.unwrap_or(config::DEFAULT_MAX_BODY_MB),
            "不设限",
        ),
    );
    line("max_response_mb", with_zero(cfg.spool_limit_mb, "不设限"));
    line("disk_cache", cfg.disk_cache_enabled().to_string());
    line(
        "allowed_hosts",
        list(
            cfg.allowed_hosts(),
            "内置默认：回环监听只放行 localhost / 127.0.0.1 / [::1]",
        ),
    );
    line(
        "allowed_origins",
        list(
            cfg.allowed_origins(),
            "内置默认：拒绝一切携带 Origin 的浏览器请求",
        ),
    );
    line(
        "log_file",
        optional(cfg.log_file.clone())
            + if cfg.log_file.is_none() {
                "（~/.aproxy/logs/ 下按启动随机命名，实际路径看 aproxy status）"
            } else {
                ""
            },
    );
    line(
        "timeouts.connect_secs",
        with_zero(cfg.connect_timeout_secs, "不设限"),
    );
    line(
        "timeouts.first_byte_secs",
        with_zero(cfg.first_byte_timeout_secs, "不设限"),
    );
    line(
        "timeouts.read_idle_secs",
        with_zero(cfg.read_timeout_secs, "不设限"),
    );
    line(
        "history.body_budget_mb",
        with_zero(cfg.history_body_budget_mb, "不保留请求体与响应体"),
    );
    line(
        "keepalive.interval_secs",
        with_zero(cfg.keepalive_interval_secs, "保活关闭"),
    );
    line(
        "keepalive.trigger",
        format!(
            "\"{}\"（accept=看 Accept 头 / body_stream=看请求体 stream:true / any=任一）",
            cfg.keepalive_trigger().as_str()
        ),
    );
    // 心跳字节通常含换行：按字符串字面量形式（{:?}）打印，换行显示为 `\n`，与 toml 里的
    // 写法一一对应
    line(
        "keepalive.heartbeat",
        format!("{:?}", String::from_utf8_lossy(cfg.keepalive_heartbeat())),
    );
    line(
        "retry.max_backoff_secs",
        with_zero(cfg.max_retry_backoff_secs, "所有重试零延迟"),
    );
    line(
        "retry.rules",
        if cfg.retry_rules.is_empty() {
            "[]（一切请求无限重试）".to_string()
        } else {
            format!("{} 条", cfg.retry_rules.len())
        },
    );
    for (index, rule) in cfg.retry_rules.iter().enumerate() {
        let path = rule.path.as_deref().unwrap_or("(任何路径)");
        let status = match &rule.status {
            Some(codes) => codes
                .iter()
                .map(u16::to_string)
                .collect::<Vec<_>>()
                .join(","),
            None => "(任何状态码)".to_string(),
        };
        let limit = match rule.max_attempts {
            0 => "不限（豁免）".to_string(),
            n => format!("第 {n} 次失败后透传"),
        };
        println!("  {}. path = {path}  status = {status}  {limit}", index + 1);
    }

    // format 程序：请求、响应、心跳各至多一个
    let formats = [
        ("request", &cfg.request_transform),
        ("response", &cfg.response_transform),
        ("heartbeat", &cfg.heartbeat_transform),
    ];
    if formats.iter().all(|(_, t)| t.is_none()) {
        line("format", "[]".to_string());
    }
    for (stage, t) in formats {
        let Some(t) = t else { continue };
        println!("format \"{}\"（{stage} 阶段）:", t.name);
        println!("  command          = \"{}\"", t.command);
        if !t.args.is_empty() {
            // 多 key 轮换类 format 常把 key 直接写在 args 里
            println!(
                "  args             = [{}]",
                mask_transform_args(&t.args).join(", ")
            );
        }
        if !t.env.is_empty() {
            // 环境变量的值只写不读：只列名字
            let names: Vec<&str> = t.env.keys().map(String::as_str).collect();
            println!("  env              = {}（值不显示）", names.join(", "));
        }
        match t.mode {
            config::TransformMode::Spawn => {
                println!("  process          = spawn（每条消息起一个进程）")
            }
            config::TransformMode::Persistent => println!(
                "  process          = persistent（进程池 pool_max={}，空闲 {}）",
                t.effective_pool_max(),
                match t.effective_idle_timeout_secs() {
                    0 => "永不回收".to_string(),
                    secs => format!("{secs}s 后回收"),
                }
            ),
        }
        println!(
            "  timeout_secs     = {}",
            with_zero(t.effective_timeout_secs(), "不限")
        );
        if t.every_attempt {
            println!("  每次重试前用原始请求重新转换（声明了 retry 阶段）");
        }
    }

    line(
        "proxy",
        optional(cfg.proxy.as_deref().map(aproxy_ipc::redact::url)),
    );
    line("proxy_username", optional(cfg.proxy_username.clone()));
    line(
        "proxy_password",
        optional(cfg.proxy_password.as_deref().map(placeholder_text)),
    );
    for (table, headers) in [
        ("extra_headers", &cfg.extra_headers),
        ("override_headers", &cfg.override_headers),
    ] {
        if headers.is_empty() {
            println!("{table:<26} = (空)");
            continue;
        }
        println!("{table}:");
        let mut names: Vec<&String> = headers.keys().collect();
        names.sort();
        for name in names {
            // 名字像鉴权的头，值是密钥：与 api_key 一样只显示占位
            let source = resolved
                .sources
                .get(&format!("{table}.{}", config::quote_key(name)))
                .copied();
            let value = if aproxy_ipc::redact::is_secret_header(name) {
                placeholder_text(&headers[name])
            } else {
                quoted(&headers[name])
            };
            println!("  {name} = {value}  （{}）", source_label(source));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transform_args_mask_values_after_secret_flags() {
        let args: Vec<String> = [
            "run",
            "--api-key",
            "SUPERSECRET2",
            "--TOKEN=tok-abcdefgh",
            "--config",
            "C:/agg.toml",
            "https://user:pw@h/x?key=Q",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let masked = mask_transform_args(&args);
        assert_eq!(
            masked,
            [
                "run",
                "--api-key",
                "<secret>",
                "--TOKEN=<secret>",
                "--config",
                "C:/agg.toml",
                "https://***@h/x?key=***",
            ]
        );
        // 旗标在末尾（缺值）不越界，原样保留
        assert_eq!(mask_transform_args(&["--key".to_string()]), ["--key"]);
    }
}
