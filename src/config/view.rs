//! config.toml 的读出口：把文件里写了的值、分层解析之后生效的值写成 JSON，交出去之前把
//! 密钥换成占位对象；再把这样的 JSON 写成给人看的 TOML。
//!
//! 形状就是 config.toml 自己的形状：键路径在 JSON 里与在 TOML 里是同一条。密钥的规则只有
//! [`redact_table`] 这一份，settings.json 的 `defaults` 段（同样的键）也用它。
//!
//! 占位对象可以原样写回：[`restore_placeholders`] 把写回的值里的占位对象换回原值，对不上
//! 就拒绝，而不是去猜。

use aproxy_ipc::i18n::diagnostic::Reason;
use std::path::Path;

use aproxy_ipc::diag::{Diagnostic, Level};
use aproxy_ipc::redact;
use serde_json::{Map, Value, json};
use toml_edit::{ArrayOfTables, DocumentMut, InlineTable, Item, Table, TableLike, Value as Toml};

use super::edit::EditError;
use super::{Resolved, SECRET_ARG_FLAGS, parse_key_path, quote_key};

/// 文件里写了的值，写成 JSON。`text` 有语法错误时返回 None。
///
/// TOML 的日期时间没有对应的 JSON 类型，写成字符串（config.toml 没有这种类型的键，出现了
/// 也只是一个会被校验报出来的值）。
pub fn file_value(text: &str) -> Option<Value> {
    let document = text.parse::<DocumentMut>().ok()?;
    Some(table_to_json(document.as_table()))
}

fn table_to_json(table: &dyn TableLike) -> Value {
    let mut map = Map::new();
    for (name, item) in table.iter() {
        let value = match item {
            Item::None => continue,
            Item::Value(value) => toml_to_json(value),
            Item::Table(table) => table_to_json(table),
            Item::ArrayOfTables(tables) => {
                Value::Array(tables.iter().map(|t| table_to_json(t)).collect())
            }
        };
        map.insert(name.to_string(), value);
    }
    Value::Object(map)
}

/// 一个 TOML 值写成 JSON。
pub fn toml_to_json(value: &Toml) -> Value {
    match value {
        Toml::String(text) => Value::String(text.value().clone()),
        Toml::Integer(number) => Value::from(*number.value()),
        // NaN 与无穷大在 JSON 里没有写法
        Toml::Float(number) => serde_json::Number::from_f64(*number.value())
            .map(Value::Number)
            .unwrap_or(Value::Null),
        Toml::Boolean(flag) => Value::Bool(*flag.value()),
        Toml::Datetime(time) => Value::String(time.value().to_string()),
        Toml::Array(items) => Value::Array(items.iter().map(toml_to_json).collect()),
        Toml::InlineTable(table) => table_to_json(table),
    }
}

/// 生效的值，写成 config.toml 的形状：每个有值的键都在（没设置、也没有内置默认的键，例如
/// `api_key`，不在）。`file` 是文件里写了的值（[`file_value`]）：`format` 只能写在文件里，
/// 原样取自那里。
///
/// 密钥是明文，交出去之前要过 [`redacted`]。
pub fn effective_value(resolved: &Resolved, file: Option<&Value>) -> Value {
    let cfg = &resolved.config;
    let mut root = Map::new();
    let mut put = |key: &str, value: Option<Value>| {
        if let Some(value) = value {
            root.insert(key.to_string(), value);
        }
    };
    let text = |value: &Option<String>| value.clone().map(Value::String);
    let headers = |table: &std::collections::HashMap<String, String>| {
        // 按头名排序：HashMap 的顺序每次运行都不一样
        let mut names: Vec<&String> = table.keys().collect();
        names.sort();
        let map: Map<String, Value> = names
            .into_iter()
            .map(|name| (name.clone(), Value::String(table[name].clone())))
            .collect();
        Value::Object(map)
    };

    put(
        "base_url",
        (!cfg.base_url.is_empty()).then(|| cfg.base_url.clone().into()),
    );
    put("listen_addr", Some(cfg.listen_addr.clone().into()));
    put("api_key", text(&cfg.api_key));
    put("extra_headers", Some(headers(&cfg.extra_headers)));
    put("override_headers", Some(headers(&cfg.override_headers)));
    put("proxy", text(&cfg.proxy));
    put("proxy_username", text(&cfg.proxy_username));
    put("proxy_password", text(&cfg.proxy_password));
    put(
        "max_request_mb",
        Some(cfg.max_body_mb.unwrap_or(super::DEFAULT_MAX_BODY_MB).into()),
    );
    put("max_response_mb", Some(cfg.spool_limit_mb.into()));
    put("disk_cache", Some(cfg.disk_cache_enabled().into()));
    put(
        "delivery",
        Some(
            if cfg.forward_only_enabled() {
                "forward"
            } else {
                "retry"
            }
            .into(),
        ),
    );
    put("allowed_hosts", Some(json!(cfg.allowed_hosts())));
    put("allowed_origins", Some(json!(cfg.allowed_origins())));
    put("log_file", text(&cfg.log_file));
    put(
        "timeouts",
        Some(json!({
            "connect_secs": cfg.connect_timeout_secs,
            "first_byte_secs": cfg.first_byte_timeout_secs,
            "read_idle_secs": cfg.read_timeout_secs,
        })),
    );
    put(
        "keepalive",
        Some(json!({
            "interval_secs": cfg.keepalive_interval_secs,
            "trigger": cfg.keepalive_trigger().as_str(),
            "heartbeat": String::from_utf8_lossy(cfg.keepalive_heartbeat()),
        })),
    );
    let rules: Vec<Value> = cfg
        .retry_rules
        .iter()
        .map(|rule| {
            let mut entry = Map::new();
            if let Some(path) = &rule.path {
                entry.insert("path".to_string(), path.clone().into());
            }
            if let Some(status) = &rule.status {
                entry.insert("status".to_string(), json!(status));
            }
            entry.insert("max_attempts".to_string(), rule.max_attempts.into());
            Value::Object(entry)
        })
        .collect();
    put(
        "retry",
        Some(json!({
            "max_backoff_secs": cfg.max_retry_backoff_secs,
            "rules": rules,
        })),
    );
    put(
        "history",
        Some(json!({ "body_budget_mb": cfg.history_body_budget_mb })),
    );
    put(
        "format",
        Some(
            file.and_then(|file| file.get("format"))
                .filter(|formats| formats.is_array())
                .cloned()
                .unwrap_or_else(|| json!([])),
        ),
    );
    Value::Object(root)
}

/// 读出口用的形状：密钥换成占位对象，URL 里的 userinfo 与查询串的值打码。`value` 是一份
/// config.toml 形状的 JSON（[`file_value`] 或 [`effective_value`]）。
pub fn redacted(mut value: Value) -> Value {
    if let Some(root) = value.as_object_mut() {
        redact_table(root);
    }
    value
}

/// 就地把一张 config.toml 形状的表里的密钥换成占位对象：
/// - `api_key`、`proxy_password`；两张头表里名字像鉴权的头；
/// - `base_url`、`proxy` 里的 userinfo 与查询串的值（打码，不是占位：其余部分要看得见）；
/// - `[[format]]` 的 `env` 的每个值，`args` 里密钥旗标之后的值。`--flag=值` 这种写在一个
///   参数里的，值的部分换成占位的文本形式。
///
/// 类型不对的值（校验会报出来的）不动。
pub fn redact_table(table: &mut Map<String, Value>) {
    let secret = |value: &mut Value| {
        if let Value::String(text) = value {
            *value = redact::placeholder(text);
        }
    };
    for key in ["api_key", "proxy_password"] {
        if let Some(item) = table.get_mut(key) {
            secret(item);
        }
    }
    for key in ["base_url", "proxy"] {
        if let Some(Value::String(text)) = table.get_mut(key) {
            *text = redact::url(text);
        }
    }
    for headers in ["extra_headers", "override_headers"] {
        if let Some(headers) = table.get_mut(headers).and_then(Value::as_object_mut) {
            for (name, item) in headers.iter_mut() {
                if redact::is_secret_header(name) {
                    secret(item);
                }
            }
        }
    }
    let formats = table.get_mut("format").and_then(Value::as_array_mut);
    for entry in formats.into_iter().flatten() {
        if let Some(env) = entry.get_mut("env").and_then(Value::as_object_mut) {
            env.values_mut().for_each(secret);
        }
        if let Some(args) = entry.get_mut("args").and_then(Value::as_array_mut) {
            let mut after_flag = false;
            for arg in args.iter_mut() {
                let Value::String(text) = arg else {
                    after_flag = false;
                    continue;
                };
                if after_flag {
                    after_flag = false;
                    *arg = redact::placeholder(text);
                } else if is_secret_flag(text) {
                    after_flag = true;
                } else if let Some((flag, secret)) = text.split_once('=')
                    && is_secret_flag(flag)
                {
                    *text = format!("{flag}={}", redact::placeholder_text(secret));
                }
            }
        }
    }
}

fn is_secret_flag(arg: &str) -> bool {
    SECRET_ARG_FLAGS
        .iter()
        .any(|flag| flag.eq_ignore_ascii_case(arg))
}

/// 一个值里有没有占位对象。
pub fn has_placeholder(value: &Value) -> bool {
    match value {
        Value::Object(map) => redact::is_placeholder(value) || map.values().any(has_placeholder),
        Value::Array(items) => items.iter().any(has_placeholder),
        Value::String(text) => is_inline_placeholder(text),
        _ => false,
    }
}

/// `--flag=<secret …>`：写在一个参数里的密钥读出来时的样子。
fn is_inline_placeholder(arg: &str) -> bool {
    arg.split_once('=')
        .is_some_and(|(flag, value)| is_secret_flag(flag) && value.starts_with("<secret"))
}

/// 把写回的值里的占位对象换回原值。`new` 是要写进键 `key` 的值，`old` 是文件里这个键现在
/// 的值（没有时为 None），`file` 只用来填诊断。
///
/// 怎样算「对得上」：
/// - 标量按键对应；表（两张头表、format 的 `env`）按键名对应；
/// - `format` 的条目按 `name` 对应；条目里的 `args` 按「同一个密钥旗标的第几次出现」对应。
///
/// 对不上——原来没有值、原值不是字符串、数组里说不清对应哪一项——报
/// `secret_placeholder_unmatched`，要求重新输入。
pub fn restore_placeholders(
    new: Value,
    old: Option<&Value>,
    key: &str,
    file: &Path,
) -> Result<Value, EditError> {
    if redact::is_placeholder(&new) {
        return match old {
            Some(original @ Value::String(_)) => Ok(original.clone()),
            _ => Err(unmatched(key, file)),
        };
    }
    match new {
        Value::Object(map) => {
            let mut out = Map::new();
            for (name, value) in map {
                let child = format!("{key}.{}", quote_key(&name));
                let value =
                    restore_placeholders(value, old.and_then(|old| old.get(&name)), &child, file)?;
                out.insert(name, value);
            }
            Ok(Value::Object(out))
        }
        Value::Array(entries) if key == "format" => {
            let old_entries = old.and_then(Value::as_array);
            let mut out = Vec::with_capacity(entries.len());
            for (index, entry) in entries.into_iter().enumerate() {
                let name = entry.get("name").and_then(Value::as_str);
                let old_entry = old_entries.and_then(|entries| {
                    entries.iter().find(|old| {
                        name.is_some() && old.get("name").and_then(Value::as_str) == name
                    })
                });
                out.push(restore_format_entry(
                    entry,
                    old_entry,
                    &format!("format[{index}]"),
                    file,
                )?);
            }
            Ok(Value::Array(out))
        }
        // 别的数组没有「同一项」的说法：里面出现占位对象就是对不上
        Value::Array(items) => items
            .into_iter()
            .enumerate()
            .map(|(index, item)| restore_placeholders(item, None, &format!("{key}[{index}]"), file))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        other => Ok(other),
    }
}

/// 一个 `[[format]]` 条目：`args` 有自己的对应规则，其余的键照常。
fn restore_format_entry(
    entry: Value,
    old: Option<&Value>,
    key: &str,
    file: &Path,
) -> Result<Value, EditError> {
    let Value::Object(map) = entry else {
        return restore_placeholders(entry, old, key, file);
    };
    let mut out = Map::new();
    for (name, value) in map {
        let child = format!("{key}.{name}");
        let old_value = old.and_then(|old| old.get(&name));
        let value = match value {
            Value::Array(args) if name == "args" => {
                Value::Array(restore_args(args, old_value, &child, file)?)
            }
            other => restore_placeholders(other, old_value, &child, file)?,
        };
        out.insert(name, value);
    }
    Ok(Value::Object(out))
}

/// `args` 里的占位：第 n 次出现的密钥旗标 F 之后的占位对象，对应原来的 `args` 里第 n 次出现
/// 的 F 之后的值；`F=<secret …>` 对应原来第 n 个 `F=值`。
fn restore_args(
    args: Vec<Value>,
    old: Option<&Value>,
    key: &str,
    file: &Path,
) -> Result<Vec<Value>, EditError> {
    // 原来的 args 里，每个密钥旗标依次带的值：分开写的（旗标、值各一个参数）与写在一起的
    // （`旗标=值`，记整个参数）各一份
    let mut old_separate: Vec<(String, String)> = Vec::new();
    let mut old_inline: Vec<(String, String)> = Vec::new();
    let mut pending: Option<String> = None;
    for arg in old.and_then(Value::as_array).into_iter().flatten() {
        let Some(text) = arg.as_str() else {
            pending = None;
            continue;
        };
        if let Some(flag) = pending.take() {
            old_separate.push((flag, text.to_string()));
        } else if is_secret_flag(text) {
            pending = Some(text.to_ascii_lowercase());
        } else if let Some((flag, _)) = text.split_once('=')
            && is_secret_flag(flag)
        {
            old_inline.push((flag.to_ascii_lowercase(), text.to_string()));
        }
    }
    // 取 `list` 里旗标 `flag` 的第 `nth` 个值
    let pick = |list: &[(String, String)], flag: &str, nth: usize| {
        list.iter()
            .filter(|(name, _)| name == flag)
            .nth(nth)
            .map(|(_, value)| Value::String(value.clone()))
    };

    let mut seen_separate: std::collections::HashMap<String, usize> = Default::default();
    let mut seen_inline: std::collections::HashMap<String, usize> = Default::default();
    let mut pending: Option<String> = None;
    let mut out = Vec::with_capacity(args.len());
    for (index, arg) in args.into_iter().enumerate() {
        let child = format!("{key}[{index}]");
        let after = pending.take();
        if redact::is_placeholder(&arg) {
            // 占位对象只能跟在一个密钥旗标后面
            let flag = after.ok_or_else(|| unmatched(&child, file))?;
            let nth = seen_separate.get(&flag).copied().unwrap_or(1) - 1;
            out.push(pick(&old_separate, &flag, nth).ok_or_else(|| unmatched(&child, file))?);
            continue;
        }
        if let Some(text) = arg.as_str() {
            if after.is_none() && is_secret_flag(text) {
                let flag = text.to_ascii_lowercase();
                *seen_separate.entry(flag.clone()).or_default() += 1;
                pending = Some(flag);
            } else if after.is_none()
                && let Some((flag, value)) = text.split_once('=')
                && is_secret_flag(flag)
            {
                let flag = flag.to_ascii_lowercase();
                let nth = *seen_inline
                    .entry(flag.clone())
                    .and_modify(|n| *n += 1)
                    .or_insert(0);
                if value.starts_with("<secret") {
                    out.push(pick(&old_inline, &flag, nth).ok_or_else(|| unmatched(&child, file))?);
                    continue;
                }
            }
        }
        out.push(arg);
    }
    Ok(out)
}

fn unmatched(key: &str, file: &Path) -> EditError {
    EditError::Invalid(vec![Diagnostic {
        code: "secret_placeholder_unmatched",
        severity: Level::Error,
        file: file.display().to_string(),
        key: Some(key.to_string()),
        line: None,
        column: None,
        message: Reason::NoStoredSecret {
            key: key.to_owned(),
        },
        details: Map::new(),
    }])
}

/// 一个键路径处的值。路径写法不对、或带了下标时是 `BadKey`；那里没有值时是 None。
pub fn get<'a>(value: &'a Value, key: &str) -> Result<Option<&'a Value>, EditError> {
    match parse_key_path(key) {
        Some(segments) if segments.iter().all(|s| s.index.is_none()) => Ok(segments
            .iter()
            .try_fold(value, |value, segment| value.get(&segment.name))),
        _ => Err(EditError::BadKey(key.to_string())),
    }
}

/// 一个键收什么样的值。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyKind {
    /// 字符串：命令行上可以直接写原文，不必加引号
    Text,
    /// 其余的类型：整数、布尔、数组、表
    Other,
}

/// config.toml 认识的键与它收的值的类型；不认识的键是 None。数组表（`format`、
/// `retry.rules`）只到整体这一层，条目里面的键不在这里。
///
/// 这张表与解析器（[`super::file::parse`]）各写了一遍键名；`known_keys_match_the_parser`
/// 这个测试核对两边一致。
pub fn key_kind(key: &str) -> Option<KeyKind> {
    let segments = parse_key_path(key)?;
    if segments.iter().any(|segment| segment.index.is_some()) {
        return None;
    }
    let names: Vec<&str> = segments.iter().map(|s| s.name.as_str()).collect();
    match names.as_slice() {
        [
            "base_url" | "listen_addr" | "api_key" | "proxy" | "proxy_username" | "proxy_password"
            | "delivery" | "log_file",
        ]
        | ["extra_headers" | "override_headers", _]
        | ["keepalive", "trigger" | "heartbeat"] => Some(KeyKind::Text),
        [
            "extra_headers" | "override_headers" | "max_request_mb" | "max_response_mb"
            | "disk_cache" | "allowed_hosts" | "allowed_origins" | "timeouts" | "keepalive"
            | "retry" | "history" | "format",
        ]
        | [
            "timeouts",
            "connect_secs" | "first_byte_secs" | "read_idle_secs",
        ]
        | ["keepalive", "interval_secs"]
        | ["retry", "max_backoff_secs" | "rules"]
        | ["history", "body_budget_mb"] => Some(KeyKind::Other),
        _ => None,
    }
}

/// 一个 JSON 值写成 TOML 值。`null` 没有对应的写法，返回 None；数组与表里的 `null` 略去。
pub fn json_to_toml(value: &Value) -> Option<Toml> {
    Some(match value {
        Value::Null => return None,
        Value::Bool(flag) => Toml::from(*flag),
        Value::Number(number) => match number.as_i64() {
            Some(integer) => Toml::from(integer),
            None => Toml::from(number.as_f64()?),
        },
        Value::String(text) => super::edit::one_line(Toml::from(text.as_str())),
        Value::Array(items) => Toml::Array(items.iter().filter_map(json_to_toml).collect()),
        Value::Object(map) => {
            let mut table = InlineTable::new();
            for (name, value) in map {
                if let Some(value) = json_to_toml(value) {
                    table.insert(name, value);
                }
            }
            Toml::InlineTable(table)
        }
    })
}

/// 给人看的形状：占位对象写成它的文本形式（`<secret>`、`<secret sk-a…f3a9>`），format 的
/// `args` 里其余的参数过一遍 URL 打码（参数里常有带口令的地址）。这个形状只用来显示，
/// 不能写回。`value` 是 [`redacted`] 之后的。
pub fn for_display(mut value: Value) -> Value {
    redact_arg_urls(&mut value);
    placeholders_as_text(&mut value);
    value
}

/// format 沿用生效文档里的文件形状；条目未通过解析时，只展示解析器实际接受的程序。
/// 参数里的 URL 同样打码，但 JSON 中的密钥仍是占位对象，不转换成面向终端的文本。
pub fn override_values(resolved: &Resolved, file: Option<&Value>) -> Value {
    let mut values = effective_value(resolved, file);
    if resolved.errors().any(|d| {
        d.key
            .as_deref()
            .is_some_and(|k| k == "format" || k.starts_with("format["))
    }) {
        values["format"] = json!(resolved.config.formats);
    }
    let mut values = redacted(values);
    redact_arg_urls(&mut values);
    values
}

fn redact_arg_urls(value: &mut Value) {
    let formats = value.get_mut("format").and_then(Value::as_array_mut);
    for entry in formats.into_iter().flatten() {
        let args = entry.get_mut("args").and_then(Value::as_array_mut);
        for arg in args.into_iter().flatten() {
            if let Value::String(text) = arg
                && !is_inline_placeholder(text)
            {
                *text = redact::url(text);
            }
        }
    }
}

fn placeholders_as_text(value: &mut Value) {
    if redact::is_placeholder(value) {
        *value = Value::String(match value.get("hint").and_then(Value::as_str) {
            Some(hint) => format!("<secret {hint}>"),
            None => "<secret>".to_string(),
        });
        return;
    }
    match value {
        Value::Object(map) => map.values_mut().for_each(placeholders_as_text),
        Value::Array(items) => items.iter_mut().for_each(placeholders_as_text),
        _ => {}
    }
}

/// 一份 config.toml 形状的 JSON 写成 TOML 文档：表写成 `[x]`，表的数组写成 `[[x]]`。
/// 根不是对象时返回空串。
pub fn to_document(value: &Value) -> String {
    let mut document = DocumentMut::new();
    if let Some(map) = value.as_object() {
        fill_table(document.as_table_mut(), map);
    }
    document.to_string()
}

fn fill_table(table: &mut Table, map: &Map<String, Value>) {
    for (name, value) in map {
        match value {
            Value::Object(child) => {
                let mut nested = Table::new();
                fill_table(&mut nested, child);
                table.insert(name, Item::Table(nested));
            }
            Value::Array(items) if !items.is_empty() && items.iter().all(Value::is_object) => {
                let mut tables = ArrayOfTables::new();
                for item in items {
                    let mut nested = Table::new();
                    fill_table(
                        &mut nested,
                        item.as_object()
                            .expect("both values were checked to be objects"),
                    );
                    tables.push(nested);
                }
                table.insert(name, Item::ArrayOfTables(tables));
            }
            other => {
                if let Some(value) = json_to_toml(other) {
                    table.insert(name, Item::Value(value));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::resolve::{Defaults, Overrides, resolve_text};

    const FILE: &str = r#"# 注释
base_url = "https://user:pw@relay.example.com/v1?key=Q"
api_key = "sk-0123456789abcdefghijklmnop"
proxy_password = "short"
max_request_mb = 64
disk_cache = false
allowed_hosts = ["a.example", "b.example"]

[extra_headers]
x-team = "core"
x-api-token = "tok-0123456789abcdefghijklmn"

[timeouts]
connect_secs = 5

[[format]]
name = "keys"
stages = ["request"]
command = "aproxy-format"
args = ["keys", "--key", "KEY-ONE-0123456789abcdefghij", "--token=TOKEN-TWO", "--key", "k2", "https://u:p@h/x"]
env = { UPSTREAM_KEY = "env-secret-0123456789abcdefgh", MODE = "x" }
"#;

    fn file() -> Value {
        file_value(FILE).unwrap()
    }

    /// TOML 的各种写法（表、内联表、表的数组）读成同一种 JSON 形状，键的顺序照文件。
    #[test]
    fn a_file_reads_as_json_of_the_same_shape() {
        let value = file();
        assert_eq!(value["max_request_mb"], json!(64));
        assert_eq!(value["disk_cache"], json!(false));
        assert_eq!(value["allowed_hosts"], json!(["a.example", "b.example"]));
        assert_eq!(value["extra_headers"]["x-team"], json!("core"));
        assert_eq!(value["timeouts"], json!({"connect_secs": 5}));
        assert_eq!(value["format"][0]["env"]["MODE"], json!("x"));
        let keys: Vec<&String> = value.as_object().unwrap().keys().collect();
        assert_eq!(&keys[..3], ["base_url", "api_key", "proxy_password"]);
        assert!(file_value("a = ").is_none(), "语法错误读不出来");
    }

    /// 读出口：密钥成了占位对象，URL 打码，别的值原样。
    #[test]
    fn secrets_are_replaced_by_placeholders() {
        let value = redacted(file());
        assert_eq!(
            value["base_url"],
            "https://***@relay.example.com/v1?key=***"
        );
        assert_eq!(
            value["api_key"],
            json!({"secret": true, "hint": "sk-0…mnop"})
        );
        assert_eq!(value["proxy_password"], json!({"secret": true}));
        assert_eq!(value["extra_headers"]["x-team"], "core");
        assert_eq!(
            value["extra_headers"]["x-api-token"],
            json!({"secret": true, "hint": "tok-…klmn"})
        );
        let format = &value["format"][0];
        assert_eq!(
            format["args"],
            json!([
                "keys",
                "--key",
                {"secret": true, "hint": "KEY-…ghij"},
                "--token=<secret>",
                "--key",
                {"secret": true},
                "https://u:p@h/x"
            ])
        );
        assert_eq!(
            format["env"],
            json!({
                "UPSTREAM_KEY": {"secret": true, "hint": "env-…efgh"},
                "MODE": {"secret": true}
            })
        );
        let text = value.to_string();
        for secret in [
            "sk-0123456789",
            "user:pw",
            "KEY-ONE",
            "TOKEN-TWO",
            "env-secret",
        ] {
            assert!(!text.contains(secret), "{secret} 漏了: {text}");
        }
    }

    /// 给人看的形状：占位写成文本，参数里的地址也打码；写成的 TOML 能再解析回来。
    #[test]
    fn the_display_form_is_toml_with_textual_placeholders() {
        let shown = for_display(redacted(file()));
        assert_eq!(shown["api_key"], "<secret sk-0…mnop>");
        assert_eq!(shown["proxy_password"], "<secret>");
        assert_eq!(shown["format"][0]["args"][6], "https://***@h/x");
        assert_eq!(shown["format"][0]["args"][3], "--token=<secret>");
        let text = to_document(&shown);
        assert!(text.contains("[extra_headers]"), "{text}");
        assert!(text.contains("[[format]]"), "{text}");
        assert!(text.contains("api_key = \"<secret sk-0…mnop>\""), "{text}");
        assert_eq!(file_value(&text).unwrap(), shown, "{text}");
    }

    /// 带换行的字符串写成一行。
    #[test]
    fn multi_line_strings_are_written_on_one_line() {
        let value = json_to_toml(&json!(": ping\n\n")).unwrap();
        assert_eq!(value.to_string(), r#"": ping\n\n""#);
        assert!(json_to_toml(&Value::Null).is_none());
        assert_eq!(
            json_to_toml(&json!({"a": [1, null, true], "b": null}))
                .unwrap()
                .to_string(),
            "{ a = [1, true] }"
        );
    }

    fn restore(new: Value, key: &str) -> Result<Value, Vec<String>> {
        let old = file();
        restore_placeholders(new, get(&old, key).unwrap(), key, Path::new("config.toml")).map_err(
            |e| match e {
                EditError::Invalid(diagnostics) => diagnostics
                    .iter()
                    .map(|d| format!("{} {}", d.code, d.key.as_deref().unwrap_or("-")))
                    .collect(),
                other => panic!("{other:?}"),
            },
        )
    }

    /// 读出来的值原样写回：每个占位都换回了原值，结果与文件里的一样。
    #[test]
    fn what_was_read_can_be_written_back_unchanged() {
        let original = file();
        let read = redacted(file());
        for key in ["api_key", "proxy_password", "extra_headers", "format"] {
            assert_eq!(
                restore(read[key].clone(), key).as_ref(),
                Ok(&original[key]),
                "{key}"
            );
        }
    }

    /// format 条目换了顺序、args 里插了别的参数，占位仍然对得上：条目按名字，密钥旗标按
    /// 第几次出现。
    #[test]
    fn format_placeholders_follow_the_entry_name_and_the_flag_occurrence() {
        let new = json!([
            {"name": "other", "stages": ["response"], "command": "x"},
            {
                "name": "keys",
                "stages": ["request"],
                "command": "aproxy-format",
                "args": ["keys", "--verbose", "--key", {"secret": true}, "--key", {"secret": true},
                         "--token=<secret>", "--key", "a-new-key"],
                "env": {"UPSTREAM_KEY": {"secret": true}, "ADDED": "plain"}
            }
        ]);
        let restored = restore(new, "format").unwrap();
        assert_eq!(
            restored[1]["args"],
            json!([
                "keys",
                "--verbose",
                "--key",
                "KEY-ONE-0123456789abcdefghij",
                "--key",
                "k2",
                "--token=TOKEN-TWO",
                "--key",
                "a-new-key"
            ])
        );
        assert_eq!(
            restored[1]["env"],
            json!({"UPSTREAM_KEY": "env-secret-0123456789abcdefgh", "ADDED": "plain"})
        );
    }

    /// 对不上的占位被拒绝，诊断指到具体的位置。
    #[test]
    fn unmatched_placeholders_are_rejected() {
        let placeholder = json!({"secret": true});
        // 原来没有值
        assert_eq!(
            restore(placeholder.clone(), "proxy_username"),
            Err(vec![
                "secret_placeholder_unmatched proxy_username".to_string()
            ])
        );
        // 头表里原来没有的头
        assert_eq!(
            restore(json!({"authorization": placeholder}), "override_headers"),
            Err(vec![
                "secret_placeholder_unmatched override_headers.authorization".to_string()
            ])
        );
        // 没有这个名字的 format 条目
        assert_eq!(
            restore(
                json!([{"name": "renamed", "env": {"UPSTREAM_KEY": placeholder}}]),
                "format"
            ),
            Err(vec![
                "secret_placeholder_unmatched format[0].env.UPSTREAM_KEY".to_string()
            ])
        );
        // 同一个旗标的第三次出现：原来只有两次
        assert_eq!(
            restore(
                json!([{"name": "keys", "args": ["--key", placeholder, "--key", placeholder, "--key", placeholder]}]),
                "format"
            ),
            Err(vec![
                "secret_placeholder_unmatched format[0].args[5]".to_string()
            ])
        );
        // 不跟在密钥旗标后面的占位
        assert_eq!(
            restore(json!([{"name": "keys", "args": [placeholder]}]), "format"),
            Err(vec![
                "secret_placeholder_unmatched format[0].args[0]".to_string()
            ])
        );
        // 别的数组里的占位
        assert_eq!(
            restore(json!([placeholder]), "allowed_hosts"),
            Err(vec![
                "secret_placeholder_unmatched allowed_hosts[0]".to_string()
            ])
        );
        assert!(has_placeholder(&json!({"a": [{"secret": true}]})));
        assert!(has_placeholder(&json!(["--key=<secret>"])));
        assert!(!has_placeholder(
            &json!({"secret": false, "a": ["--key=x"]})
        ));
    }

    fn resolved(text: Option<&str>) -> Resolved {
        resolve_text(
            text,
            Path::new("config.toml"),
            &Overrides::default(),
            &Defaults::default(),
            false,
        )
    }

    /// 生效的值：文件里没写的键是内置默认，没有默认的键不出现，format 取自文件。
    #[test]
    fn effective_values_cover_every_key_with_a_value() {
        let value = effective_value(&resolved(Some(FILE)), Some(&file()));
        assert_eq!(value["listen_addr"], "127.0.0.1:12345");
        assert_eq!(value["max_request_mb"], 64);
        assert_eq!(value["max_response_mb"], 256);
        assert_eq!(value["delivery"], "retry");
        assert_eq!(
            value["timeouts"],
            json!({"connect_secs": 5, "first_byte_secs": 300, "read_idle_secs": 300})
        );
        assert_eq!(value["keepalive"]["heartbeat"], ": keepalive\n\n");
        assert_eq!(
            value["retry"],
            json!({"max_backoff_secs": 320, "rules": []})
        );
        assert_eq!(value["history"]["body_budget_mb"], 64);
        assert_eq!(value["format"][0]["name"], "keys");
        assert!(value.get("proxy").is_none() && value.get("log_file").is_none());

        // 没有文件：只有内置默认，base_url 没有值
        let value = effective_value(&resolved(None), None);
        assert!(value.get("base_url").is_none() && value.get("api_key").is_none());
        assert_eq!(value["format"], json!([]));
        assert_eq!(value["extra_headers"], json!({}));
    }

    /// 生效值里每个叶子键都有来源记录（头表按头名），反过来来源表里的键也都在生效值里：
    /// 两边说的是同一批键。`proxy` 例外——它可以来自环境变量，那时配置里没有值。
    #[test]
    fn every_effective_key_has_a_source() {
        let resolved = resolved(Some(
            "base_url = \"https://a.example\"\napi_key = \"k\"\nlog_file = \"x.log\"\n\
             proxy = \"http://p.example:1\"\nproxy_username = \"u\"\nproxy_password = \"p\"\n\
             [extra_headers]\nx-a = \"1\"\n[[retry.rules]]\nstatus = [500]\nmax_attempts = 2\n",
        ));
        assert!(resolved.is_valid(), "{:?}", resolved.diagnostics);
        let value = effective_value(&resolved, None);
        let mut leaves = Vec::new();
        for (name, item) in value.as_object().unwrap() {
            match item {
                Value::Object(map) => leaves.extend(
                    map.keys()
                        .map(|child| format!("{name}.{}", quote_key(child))),
                ),
                _ => leaves.push(name.clone()),
            }
        }
        for key in &leaves {
            assert!(resolved.sources.contains_key(key), "{key} 没有来源");
            assert!(key_kind(key).is_some(), "{key} 不在键表里");
        }
        for key in resolved.sources.keys() {
            assert!(leaves.contains(key), "来源表里的 {key} 不在生效值里");
        }
    }

    /// 键表与解析器一致：表里的每个键解析器都认，表外的键解析器不认。
    #[test]
    fn known_keys_match_the_parser() {
        let unknown = |text: &str| {
            crate::config::file::parse(text, Path::new("config.toml"))
                .1
                .iter()
                .any(|d| d.code == "config_unknown_key")
        };
        let text_keys = [
            "base_url",
            "listen_addr",
            "api_key",
            "proxy",
            "proxy_username",
            "proxy_password",
            "delivery",
            "log_file",
            "extra_headers.x-a",
            "override_headers.\"x.b\"",
            "keepalive.trigger",
            "keepalive.heartbeat",
        ];
        for key in text_keys {
            assert_eq!(key_kind(key), Some(KeyKind::Text), "{key}");
            assert!(!unknown(&format!("{key} = \"x\"\n")), "{key}");
        }
        let other_keys = [
            "max_request_mb",
            "max_response_mb",
            "disk_cache",
            "allowed_hosts",
            "allowed_origins",
            "timeouts.connect_secs",
            "timeouts.first_byte_secs",
            "timeouts.read_idle_secs",
            "keepalive.interval_secs",
            "retry.max_backoff_secs",
            "retry.rules",
            "history.body_budget_mb",
            "format",
        ];
        for key in other_keys {
            assert_eq!(key_kind(key), Some(KeyKind::Other), "{key}");
            assert!(!unknown(&format!("{key} = 1\n")), "{key}");
        }
        for key in [
            "extra_headers",
            "override_headers",
            "timeouts",
            "keepalive",
            "retry",
            "history",
        ] {
            assert_eq!(key_kind(key), Some(KeyKind::Other), "{key}");
            assert!(!unknown(&format!("[{key}]\n")), "{key}");
        }
        for key in [
            "nope",
            "timeouts.total_secs",
            "keepalive.enabled",
            "retry.max_attempts",
            "history.nope",
            "forward_only",
        ] {
            assert_eq!(key_kind(key), None, "{key}");
            assert!(unknown(&format!("{key} = 1\n")), "{key}");
        }
        // 标量下面再写一层：解析器报的是类型不对，键表里同样没有这个键
        assert_eq!(key_kind("base_url.x"), None);
        assert_eq!(key_kind("format[0].name"), None);
        assert_eq!(key_kind("bad..path"), None);
    }
}
