//! `aproxy settings show|get|set|unset`：查看与修改 settings.json。
//!
//! 这组命令在 settings.json 无效时照样能用——它们就是用来查看与修复它的。读出口一律不给
//! 密钥的明文：密钥显示成占位对象 `{"secret":true}`，URL 里的口令打码；把占位对象原样写回
//! 等于不改。

use aproxy::cli::SettingsCmd;
use aproxy::config::edit::EditError;
use aproxy::config::parse_key_path;
use aproxy::settings::{self, edit};
use aproxy_ipc::code::{EXIT_CONFIG, EXIT_USAGE};
use serde_json::Value;

pub(crate) fn handle_settings_cmd(cmd: SettingsCmd) {
    let path = settings::settings_path();
    match cmd {
        SettingsCmd::Show => {
            let effective = effective_or_exit(&path);
            println!("# {}", path.display());
            print_json(&edit::redacted(effective));
            exit_if_invalid(&path);
        }
        SettingsCmd::Get { key } => {
            let effective = edit::redacted(effective_or_exit(&path));
            match edit::get(&effective, &key) {
                Ok(Some(value)) => print_json(value),
                // 没设置、也没有内置默认的键（例如 defaults.api_key）
                Ok(None) => println!("null"),
                Err(e) => fail(&e, &path),
            }
        }
        SettingsCmd::Set { key, value } => {
            let value = parse_value(&key, &value);
            apply(&path, &[edit::Change::Set { key, value }]);
        }
        SettingsCmd::Unset { key } => apply(&path, &[edit::Change::Unset { key }]),
    }
}

/// 生效的设置（文件里的键盖在内置默认上）。文件读不出来、或不是 JSON 对象时，打印诊断并
/// 以退出码 3 结束：这种文件只能手工修。
fn effective_or_exit(path: &std::path::Path) -> Value {
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
            eprintln!(
                "settings.json 不是合法的 JSON 对象，只能手工修复（或删除它，回到内置默认）:"
            );
            print_errors(path);
            crate::util::exit(EXIT_CONFIG as i32)
        }
    }
}

/// 文件有 error 级诊断时列出它们与修法，以退出码 3 结束。
fn exit_if_invalid(path: &std::path::Path) {
    if settings::read(path).is_valid() {
        return;
    }
    eprintln!();
    eprintln!("settings.json 有错误:");
    print_errors(path);
    print_repair_hints(path);
    crate::util::exit(EXIT_CONFIG as i32)
}

fn print_errors(path: &std::path::Path) {
    for d in settings::read(path)
        .diagnostics
        .iter()
        .filter(|d| d.is_error())
    {
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
        eprintln!("逐项修复（删掉出问题的键，或用 aproxy settings set <键> <值> 改对）:");
        for key in keys {
            eprintln!("  aproxy settings unset '{key}'");
        }
    }
    eprintln!(
        "也可以删除整个文件回到内置默认（别名与全局默认会一并丢失）: {}",
        path.display()
    );
}

fn apply(path: &std::path::Path, changes: &[edit::Change]) {
    match edit::apply(path, &aproxy_ipc::home::home(), changes) {
        Ok(applied) if applied.changed => println!("已保存到 {}", applied.file.display()),
        Ok(applied) => println!("没有变化: {}", applied.file.display()),
        Err(e) => fail(&e, path),
    }
}

fn fail(error: &EditError, path: &std::path::Path) -> ! {
    eprintln!("{}", settings::describe_edit_error(error, path));
    crate::util::exit(match error {
        EditError::BadKey(_) => EXIT_USAGE as i32,
        EditError::Syntax(_) | EditError::Invalid(_) => EXIT_CONFIG as i32,
        EditError::Conflict | EditError::LockBusy | EditError::Io(_) => 1,
    })
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
fn parse_value(key: &str, raw: &str) -> Value {
    let raw = if raw == "-" {
        let mut line = String::new();
        if let Err(e) = std::io::stdin().read_line(&mut line) {
            eprintln!("从 stdin 读取值失败: {e}");
            crate::util::exit(1)
        }
        line.trim_end_matches(['\r', '\n']).to_string()
    } else {
        raw.to_string()
    };
    let parsed = serde_json::from_str::<Value>(&raw);
    if is_string_key(key) {
        return match parsed {
            Ok(value) if raw.starts_with('"') || aproxy_ipc::redact::is_placeholder(&value) => {
                value
            }
            _ => Value::String(raw),
        };
    }
    parsed.unwrap_or_else(|e| {
        eprintln!(
            "{key} 的值要写成 JSON（例如 true、30、\"text\"、[\"a\"]、{{\"k\":1}}），解析失败: {e}"
        );
        crate::util::exit(EXIT_USAGE as i32)
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
        assert_eq!(parse_value("language", "en"), json!("en"));
        assert_eq!(parse_value("defaults.api_key", "12345"), json!("12345"));
        assert_eq!(parse_value("defaults.api_key", "true"), json!("true"));
        assert_eq!(
            parse_value("defaults.extra_headers.\"x.y\"", "a b"),
            json!("a b")
        );
        // 以引号开头：按字符串字面量解析
        assert_eq!(
            parse_value("defaults.keepalive.heartbeat", r#"": ping\n\n""#),
            json!(": ping\n\n")
        );
        // 占位对象原样保留，表示保持原值
        assert_eq!(
            parse_value("defaults.api_key", r#"{"secret":true,"hint":"sk-a…f3a9"}"#),
            json!({"secret": true, "hint": "sk-a…f3a9"})
        );
        // 其余的键：JSON
        assert_eq!(parse_value("watchdog.enabled", "false"), json!(false));
        assert_eq!(parse_value("logs.rotate_mb", "16"), json!(16));
        assert_eq!(parse_value("config_dirs", r#"["~/a"]"#), json!(["~/a"]));
        assert_eq!(
            parse_value("defaults", r#"{"disk_cache":false}"#),
            json!({"disk_cache": false})
        );
        assert!(!is_string_key("aliases"));
        assert!(!is_string_key("defaults.max_request_mb"));
        assert!(!is_string_key("bad..path"));
    }
}
