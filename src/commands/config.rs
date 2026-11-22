//! `aproxy config show|get|set|unset`：查看与修改一份 config.toml。
//!
//! 读出口不给密钥的明文：`--json` 里密钥是占位对象 `{"secret":true}`，给人看的输出里是
//! `<secret>`；URL 里的口令打码。

use super::config_text as text;
use aproxy_ipc::i18n::current;

use std::path::{Path, PathBuf};

use aproxy::cli::{ConfigArgs, ConfigCmd};
use aproxy::config::edit::{Change, EditError};
use aproxy::config::{self, Source, view};
use aproxy::settings;
use aproxy_ipc::code::{EXIT_CONFIG, EXIT_FAILED};
use serde_json::{Value, json};

use super::print_doc;

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
    let json = match &cmd {
        ConfigCmd::Show { json, .. }
        | ConfigCmd::Get { json, .. }
        | ConfigCmd::Set { json, .. }
        | ConfigCmd::Unset { json, .. } => *json,
    };
    let path = match config.as_deref() {
        Some(target) => settings::resolve_alias(target)
            .map(PathBuf::from)
            .unwrap_or_else(|| settings::expand_path(target)),
        None => super::require_cfg_path(super::resolve_cfg_path(None), json),
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

/// 整条命令失败（见 [`super::Failure`]）。
fn fail(
    json: bool,
    code: &'static str,
    message: impl Fn(aproxy_ipc::i18n::Lang) -> String,
    details: Value,
) -> ! {
    super::Failure::rendered(code, message, details).exit(json)
}

/// 读配置文件的文本；不存在时是 None。读不出来（权限、不是 UTF-8）时失败退出。
fn read_text(path: &Path) -> Option<String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            eprintln!("{}", text::read(current(), &e, path.display()));
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
            eprintln!("{}", text::read(current(), &e, path.display()));
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
            |lang| text::specified_missing(lang, path.display()),
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
            println!("{}", text::withheld(current()));
            for item in &resolved.withheld {
                println!(
                    "{}",
                    text::targets(
                        current(),
                        &item.key,
                        &item.default_target,
                        if item.target.is_empty() {
                            text::unset(current())
                        } else {
                            &item.target
                        }
                    )
                );
            }
        }
    } else {
        println!("# {}", path.display());
        if text.is_none() {
            eprintln!("{}", text::create(current()));
        }
        print!("{}", view::to_document(&view::for_display(values)));
    }

    for d in resolved.diagnostics.iter().filter(|d| !d.is_error()) {
        eprintln!(
            "{}",
            text::warning(
                current(),
                &aproxy::ops::start::render_diagnostic(d, current())
            )
        );
    }
    if !resolved.is_valid() {
        // 这份配置现在启动不了
        eprintln!();
        eprintln!("{}", text::invalid(current()));
        let errors: Vec<_> = resolved.errors().cloned().collect();
        print_diagnostics(&errors);
        if resolved.config.base_url.is_empty() {
            eprintln!();
            eprintln!("{}", text::base_url(current()));
            eprintln!("  aproxy config set base_url https://api.anthropic.com");
        }
        let unknown: Vec<&str> = errors
            .iter()
            .filter(|d| d.code == "config_unknown_key")
            .filter_map(|d| d.key.as_deref())
            .collect();
        if !unknown.is_empty() {
            eprintln!();
            eprintln!("{}", text::unknown_keys(current()));
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
            |lang| text::bad_key(lang, key),
            json!({"reason": "bad_key_path"}),
        );
    }
    fail(
        json,
        "config_unknown_key",
        |lang| text::unknown(lang, key),
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
            |lang| super::missing_config(lang, path),
            json!({"target": path.display().to_string()}),
        );
    };
    let resolved = resolve(path);
    let Some(file_values) = view::file_value(&text) else {
        // 语法错误：文件里的值一个都读不出来，给出的只会是默认值
        let errors: Vec<_> = resolved.errors().cloned().collect();
        eprintln!("{}", text::syntax(current()));
        print_diagnostics(&errors);
        fail(
            json,
            "config_invalid",
            |lang| text::cannot_get(lang, key, path.display()),
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
            eprintln!("{}", text::origin(current(), source_label(source.copied())));
        }
        None => eprintln!("{}", text::unset(current())),
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
            |lang| text::not_toml(lang, key, &e),
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
            |lang| text::bad_key(lang, key.as_ref()),
            json!({"reason": "bad_key_path"}),
        ),
        Err(EditError::Syntax(diagnostics)) => {
            eprintln!("{}", text::not_modified(current()));
            print_diagnostics(&diagnostics);
            fail(
                json,
                "config_invalid",
                |lang| text::repair(lang, path.display()),
                json!({"diagnostics": diagnostics}),
            )
        }
        Err(EditError::Invalid(diagnostics)) => {
            print_diagnostics(&diagnostics);
            fail(
                json,
                "config_invalid",
                |lang| text::refused(lang).to_string(),
                json!({"diagnostics": diagnostics}),
            )
        }
        Err(EditError::Conflict) => fail(
            json,
            "write_conflict",
            |lang| text::conflict(lang, path.display()),
            json!({"file": path.display().to_string()}),
        ),
        Err(EditError::LockBusy) => fail(
            json,
            "write_conflict",
            |lang| settings::describe_edit_error(&EditError::LockBusy, path, lang),
            json!({"file": path.display().to_string()}),
        ),
        Err(EditError::Io(e)) => {
            eprintln!("{}", text::save_error(current(), &e));
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
        println!("{}", text::saved(current(), applied.file.display()));
    } else {
        println!("{}", text::unchanged(current(), applied.file.display()));
    }
    if !ports.is_empty() {
        eprintln!("{}", text::restart(current()));
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
        eprintln!("  {}", aproxy::ops::start::render_diagnostic(d, current()));
    }
}

/// 来源的终端说法（展示在每个值后面）。
fn source_label(source: Option<Source>) -> &'static str {
    match source {
        Some(Source::Override) => text::command_line(current()),
        Some(Source::Toml) => "config.toml",
        Some(Source::Settings) => text::global(current()),
        Some(Source::Env) => text::environment(current()),
        Some(Source::Default) => text::builtin(current()),
        None => text::not_set(current()),
    }
}

/// 打印一份配置的生效值：每个键一行，后面注明它来自哪一层。密钥只显示打码后的样子。
fn print_effective(path: &Path, resolved: &config::Resolved) {
    let cfg = &resolved.config;
    let line = |key: &str, value: String| {
        println!(
            "{}",
            text::value(
                current(),
                key,
                &value,
                source_label(resolved.sources.get(key).copied())
            )
        );
    };
    let quoted = |s: &str| format!("\"{s}\"");
    let optional = |v: Option<String>| v.unwrap_or_else(|| text::unset(current()).to_string());
    // 0 在这几个键上有特别的含义，写在值后面
    let with_zero = |n: u64, zero: &str| {
        if n == 0 {
            text::zero(current(), zero)
        } else {
            n.to_string()
        }
    };
    let list = |items: &[String], empty: &str| {
        if items.is_empty() {
            text::empty_list(current(), empty)
        } else {
            // 原样列出（不用 {:?}：Debug 会把正则里的 \? 写成 \\?，对照 toml 会被误导）
            format!("[{}]", items.join(", "))
        }
    };

    println!("{}", text::file(current(), path.display()));
    line("base_url", quoted(&aproxy_ipc::redact::url(&cfg.base_url)));
    line("listen_addr", quoted(&cfg.listen_addr));
    line(
        "api_key",
        optional(cfg.api_key.as_deref().map(placeholder_text)),
    );
    line(
        "delivery",
        if cfg.forward_only_enabled() {
            text::forward(current()).to_string()
        } else {
            "\"retry\"".to_string()
        },
    );
    line(
        "max_request_mb",
        with_zero(
            cfg.max_body_mb.unwrap_or(config::DEFAULT_MAX_BODY_MB),
            text::unlimited(current()),
        ),
    );
    line(
        "max_response_mb",
        with_zero(cfg.spool_limit_mb, text::unlimited(current())),
    );
    line("disk_cache", cfg.disk_cache_enabled().to_string());
    line(
        "allowed_hosts",
        list(cfg.allowed_hosts(), text::local_hosts(current())),
    );
    line(
        "allowed_origins",
        list(cfg.allowed_origins(), text::no_origins(current())),
    );
    line(
        "log_file",
        optional(cfg.log_file.clone())
            + if cfg.log_file.is_none() {
                text::log_path(current())
            } else {
                ""
            },
    );
    line(
        "timeouts.connect_secs",
        with_zero(cfg.connect_timeout_secs, text::unlimited(current())),
    );
    line(
        "timeouts.first_byte_secs",
        with_zero(cfg.first_byte_timeout_secs, text::unlimited(current())),
    );
    line(
        "timeouts.read_idle_secs",
        with_zero(cfg.read_timeout_secs, text::unlimited(current())),
    );
    line(
        "history.body_budget_mb",
        with_zero(cfg.history_body_budget_mb, text::no_bodies(current())),
    );
    line(
        "keepalive.interval_secs",
        with_zero(cfg.keepalive_interval_secs, text::no_keepalive(current())),
    );
    line(
        "keepalive.trigger",
        text::keepalive(current(), cfg.keepalive_trigger().as_str()),
    );
    // 心跳字节通常含换行：按字符串字面量形式（{:?}）打印，换行显示为 `\n`，与 toml 里的
    // 写法一一对应
    line(
        "keepalive.heartbeat",
        format!("{:?}", String::from_utf8_lossy(cfg.keepalive_heartbeat())),
    );
    line(
        "retry.max_backoff_secs",
        with_zero(cfg.max_retry_backoff_secs, text::no_delay(current())),
    );
    line(
        "retry.rules",
        if cfg.retry_rules.is_empty() {
            text::no_rules(current()).to_string()
        } else {
            text::rules(current(), cfg.retry_rules.len())
        },
    );
    for (index, rule) in cfg.retry_rules.iter().enumerate() {
        let path = rule.path.as_deref().unwrap_or(text::any_path(current()));
        let status = match &rule.status {
            Some(codes) => codes
                .iter()
                .map(u16::to_string)
                .collect::<Vec<_>>()
                .join(","),
            None => text::any_status(current()).to_string(),
        };
        let limit = match rule.max_attempts {
            0 => text::exempt(current()).to_string(),
            n => text::after(current(), n),
        };
        println!("  {}. path = {path}  status = {status}  {limit}", index + 1);
    }

    // 每条配置只展示一次，阶段顺序按文件列出；凭据不参与展示。
    if cfg.formats.is_empty() {
        line("format", "[]".to_string());
    }
    for t in &cfg.formats {
        let stage = t
            .stages
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        println!("{}", text::stage(current(), &t.name, &stage));
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
            println!("{}", text::env_names(current(), &names.join(", ")));
        }
        match t.process {
            config::file::Process::Spawn => {
                println!("{}", text::spawn(current()))
            }
            config::file::Process::Persistent => println!(
                "{}",
                text::pool(
                    current(),
                    t.pool_max,
                    &match t.worker_idle_secs {
                        0 => text::never(current()).to_owned(),
                        secs => text::reap(current(), secs),
                    }
                )
            ),
        }
        println!(
            "  timeout_secs     = {}",
            with_zero(t.timeout_secs, text::no_limit(current()))
        );
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
            println!("{}", text::empty_table(current(), table));
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
            println!(
                "{}",
                text::header(current(), name, &value, source_label(source))
            );
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
