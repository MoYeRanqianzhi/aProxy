//! `aproxy settings show|get|set|unset`：查看与修改 settings.json。
//!
//! 这组命令在 settings.json 无效时照样能用——它们就是用来查看与修复它的。读出口一律不给
//! 密钥的明文：密钥显示成占位对象 `{"secret":true}`，URL 里的口令打码；把占位对象原样写回
//! 等于不改。

use super::super::boot;
use aproxy_ipc::i18n::current;
use std::path::Path;

use aproxy::cli::SettingsCmd;
use aproxy::config::edit::EditError;
use aproxy::config::parse_key_path;
use aproxy::settings::{self, edit};
use aproxy_ipc::code::EXIT_CONFIG;
use aproxy_ipc::diag::Diagnostic;
use serde_json::{Value, json};

use super::{Failure, print_doc, say};

/// 查看或修改当前 home 的设置；只有写入全局默认值时才需要异步查询运行中的实例。
pub(crate) async fn handle_settings_cmd(cmd: SettingsCmd) {
    let path = settings::settings_path();
    match cmd {
        SettingsCmd::Show { json } => {
            let values = edit::redacted(effective_or_exit(&path, json));
            let loaded = settings::read(&path);
            if json {
                print_doc(json!({
                    "file": path.display().to_string(),
                    "values": values,
                    "diagnostics": loaded.diagnostics,
                }));
            } else {
                println!("# {}", path.display());
                print_json(&values);
            }
            exit_if_invalid(&path, &loaded);
        }
        SettingsCmd::Get { key, json } => {
            let effective = edit::redacted(effective_or_exit(&path, json));
            // 没设置、也没有内置默认的键（例如 defaults.api_key），两种输出都给 null。
            let value = edit::get(&effective, &key)
                .unwrap_or_else(|e| fail(&e, &path, json))
                .unwrap_or(&Value::Null);
            if json {
                print_doc(json!({"key": key, "value": value}));
            } else {
                print_json(value);
            }
        }
        SettingsCmd::Set { key, value, json } => {
            let value = parse_value(&key, &value, json);
            apply(&path, edit::Change::Set { key, value }, json).await;
        }
        SettingsCmd::Unset { key, json } => apply(&path, edit::Change::Unset { key }, json).await,
    }
}

/// 生效的设置（文件里的键盖在内置默认上）。读文件失败以 1 退出；读出来不是合法的 JSON
/// 对象时列出诊断并以 3 退出：这种文件只能手工修。
fn effective_or_exit(path: &Path, json: bool) -> Value {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            eprintln!("读取 settings.json 失败: {e}\n文件: {}", path.display());
            crate::util::exit(1)
        }
    };
    match edit::effective_value(text.as_deref()) {
        Some(value) => value,
        None => {
            let message =
                "settings.json 不是合法的 JSON 对象，只能手工修复（或删除它，回到内置默认）:";
            let errors: Vec<_> = settings::read(path)
                .diagnostics
                .into_iter()
                .filter(Diagnostic::is_error)
                .collect();
            if json {
                print_errors(&errors);
                Failure::new("config_invalid", message, json!({"diagnostics": errors})).exit(json);
            }
            eprintln!("{message}");
            print_errors(&errors);
            crate::util::exit(EXIT_CONFIG as i32)
        }
    }
}

/// 文件有 error 级诊断时列出它们与修法，以退出码 3 结束。
fn exit_if_invalid(path: &Path, loaded: &settings::Loaded) {
    if loaded.is_valid() {
        return;
    }
    eprintln!();
    eprintln!("{}", boot::settings_errors(current()));
    print_errors(&loaded.diagnostics);
    print_repair_hints(path);
    crate::util::exit(EXIT_CONFIG as i32)
}

/// 只列出阻止使用这份设置的诊断，警告不当作错误报告。
fn print_errors(diagnostics: &[Diagnostic]) {
    for d in diagnostics.iter().filter(|d| d.is_error()) {
        eprintln!("  {}", aproxy::ops::start::render_diagnostic(d));
    }
}

/// 怎样修：每个出问题的键给一条删除它的命令，外加整个删掉文件这条退路。
pub(crate) fn print_repair_hints(path: &std::path::Path) {
    let loaded = settings::read(path);
    let mut keys: Vec<&str> = loaded
        .diagnostics
        .iter()
        .filter(|d| d.is_error() && d.code != "config_missing_key")
        .filter_map(|d| d.key.as_deref())
        // 带下标的路径（数组里的条目）不能单独删，删它所在的那个键
        .map(|key| key.split('[').next().unwrap_or(key))
        .collect();
    keys.dedup();
    eprintln!();
    if !keys.is_empty() {
        eprintln!("{}", boot::repair_keys(current()));
        for key in keys {
            eprintln!("  aproxy settings unset '{key}'");
        }
    }
    eprintln!("{}", boot::remove_settings(current(), path.display()));
}

/// 写一个键。`--json` 文档里的 `value` 取写完之后这个键的生效值：删掉一个键之后露出来的
/// 可能是内置默认，而不是 null。
async fn apply(path: &Path, change: edit::Change, json: bool) {
    let key = match &change {
        edit::Change::Set { key, .. } | edit::Change::Unset { key } => key,
    };
    let applied = edit::apply(
        path,
        &aproxy_ipc::home::home(),
        std::slice::from_ref(&change),
    )
    .unwrap_or_else(|e| fail(&e, path, json));
    let ports = if applied.changed && in_defaults(key) {
        running_ports().await
    } else {
        Vec::new()
    };
    if json {
        let effective = edit::redacted(effective_or_exit(&applied.file, json));
        let value = edit::get(&effective, key).unwrap_or_else(|e| fail(&e, path, json));
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
        say(json, format_args!("已保存到 {}", applied.file.display()));
    } else {
        say(json, format_args!("没有变化: {}", applied.file.display()));
    }
    if !ports.is_empty() {
        eprintln!("运行中的实例要重启才用上这次改动:");
        for port in ports {
            eprintln!("  aproxy restart {port}");
        }
    }
}

/// 这个键是不是 `defaults` 或它下面的键。按解析后的第一段判断：键路径的名字可以加引号，
/// `"defaults".max_request_mb` 与 `defaults.max_request_mb` 是同一个键。
fn in_defaults(key: &str) -> bool {
    parse_key_path(key)
        .and_then(|segments| segments.into_iter().next())
        .is_some_and(|first| first.name == "defaults")
}

/// 运行中的全部实例的端口，从小到大。实例只在启动时读 settings.json 的全局默认，与它用哪份
/// 配置文件无关，所以改了 `defaults` 时每个实例都要重启才用上；不应答的实例也算在内。
async fn running_ports() -> Vec<u16> {
    let survey = aproxy::daemon::survey_instances().await;
    let mut ports: Vec<u16> = survey
        .responsive
        .iter()
        .map(|status| &status.instance)
        .chain(survey.unresponsive.iter())
        .filter_map(|instance| aproxy::daemon::port_of(&instance.listen_addr).parse().ok())
        .collect();
    ports.sort_unstable();
    ports.dedup();
    ports
}

/// 编辑失败的两种输出共用错误码与说明；I/O 错误以 1 退出，由命令出口补齐错误文档。
fn fail(error: &EditError, path: &Path, json: bool) -> ! {
    let message = settings::describe_edit_error(error, path);
    let (code, details) = match error {
        EditError::BadKey(_) => ("usage", json!({"reason": "bad_key_path"})),
        EditError::Syntax(d) | EditError::Invalid(d) => {
            ("config_invalid", json!({"diagnostics": d}))
        }
        EditError::Conflict | EditError::LockBusy => (
            "write_conflict",
            json!({"file": path.display().to_string()}),
        ),
        EditError::Io(_) => {
            eprintln!("{message}");
            crate::util::exit(1)
        }
    };
    Failure::new(code, message, details).exit(json)
}

fn print_json(value: &Value) {
    println!(
        "{}",
        serde_json::to_string_pretty(value).expect("JSON 值总能序列化")
    );
}

/// 命令行上的值 → JSON 值。
/// - `-`：从 stdin 读一行（传密钥用，不进命令行历史）。
/// - 字符串类型的键可以直接写原文；以 `"` 开头时按 JSON 字符串字面量解析（用来写换行等）。
///   写回一个占位对象表示保持原值。
/// - 其余的键只收 JSON 值。
fn parse_value(key: &str, raw: &str, json: bool) -> Value {
    let raw = if raw == "-" {
        crate::util::read_secret_line().unwrap_or_else(|e| {
            eprintln!("{e}");
            crate::util::exit(1)
        })
    } else {
        raw.to_string()
    };
    let parsed = serde_json::from_str::<Value>(&raw);
    if is_string_key(key) {
        return match parsed {
            Ok(value) if raw.starts_with('"') || aproxy_ipc::redact::is_placeholder(&value) => {
                value
            }
            // 默认配置文件的路径展开 `~` 并按当前目录绝对化：它日后会在别的工作目录被读取
            _ if key == "default_config" => {
                Value::String(settings::expand_path(&raw).display().to_string())
            }
            _ => Value::String(raw),
        };
    }
    parsed.unwrap_or_else(|e| {
        let message = format!(
            "{key} 的值要写成 JSON（例如 true、30、\"text\"、[\"a\"]、{{\"k\":1}}），解析失败: {e}"
        );
        Failure::new("usage", message, json!({"reason": "bad_value"})).exit(json)
    })
}

/// 这个键的值是不是字符串（于是命令行上可以直接写原文、不必加引号）。
fn is_string_key(key: &str) -> bool {
    let Some(segments) = parse_key_path(key) else {
        return false;
    };
    let names: Vec<&str> = segments.iter().map(|s| s.name.as_str()).collect();
    matches!(
        names.as_slice(),
        ["language"]
            | ["default_config"]
            | ["aliases", _]
            | ["update", "download_proxy"]
            | [
                "defaults",
                "base_url" | "api_key" | "proxy" | "proxy_username" | "proxy_password" | "delivery"
            ]
            | ["defaults", "keepalive", "trigger" | "heartbeat"]
            | ["defaults", "extra_headers" | "override_headers", _]
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn string_keys_take_raw_text_and_others_take_json() {
        // 字符串类型的键：原文
        assert_eq!(parse_value("language", "en", false), json!("en"));
        assert_eq!(
            parse_value("defaults.api_key", "12345", false),
            json!("12345")
        );
        assert_eq!(
            parse_value("defaults.api_key", "true", false),
            json!("true")
        );
        assert_eq!(
            parse_value("defaults.extra_headers.\"x.y\"", "a b", false),
            json!("a b")
        );
        // 以引号开头：按字符串字面量解析
        assert_eq!(
            parse_value("defaults.keepalive.heartbeat", r#"": ping\n\n""#, false),
            json!(": ping\n\n")
        );
        // 占位对象原样保留，表示保持原值
        assert_eq!(
            parse_value(
                "defaults.api_key",
                r#"{"secret":true,"hint":"sk-a…f3a9"}"#,
                false
            ),
            json!({"secret": true, "hint": "sk-a…f3a9"})
        );
        // 其余的键：JSON
        assert_eq!(
            parse_value("watchdog.enabled", "false", false),
            json!(false)
        );
        assert_eq!(parse_value("logs.rotate_mb", "16", false), json!(16));
        assert_eq!(
            parse_value("config_dirs", r#"["~/a"]"#, false),
            json!(["~/a"])
        );
        assert_eq!(
            parse_value("defaults", r#"{"disk_cache":false}"#, false),
            json!({"disk_cache": false})
        );
        assert!(!is_string_key("aliases"));
        // 键名加不加引号都是同一个键
        assert!(in_defaults("defaults"));
        assert!(in_defaults("defaults.max_request_mb"));
        assert!(in_defaults("\"defaults\".max_request_mb"));
        assert!(!in_defaults("defaultsx.max_request_mb"));
        assert!(!in_defaults("watchdog.enabled"));
        assert!(!in_defaults("bad..path"));
        assert!(!is_string_key("defaults.max_request_mb"));
        assert!(!is_string_key("bad..path"));
    }
}
