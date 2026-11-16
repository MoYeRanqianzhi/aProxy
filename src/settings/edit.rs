//! settings.json 的写回：在 JSON 值上按键路径打补丁，其余内容与键的顺序原样保留。
//!
//! 与 config.toml 的写回（[`crate::config::edit`]）共用一套做法：同一把写锁、落盘前检测外部
//! 修改、原子写；这次改动引入了新的 error 级诊断就拒绝写入，文件里原有的、与这次改动无关的
//! 错误不挡路；`unset` 接受任何键路径。另外三件 settings.json 自己的事：
//! - 文件含密钥（`defaults` 里可以有 `api_key`、代理口令、鉴权头），按含密钥的文件来写；
//! - 写回的值里出现密钥的占位对象（`{"secret":true}`，读出口给的）表示「保持原值」，按同一
//!   键路径换回原值，对不上就拒绝，不去猜；
//! - schema 比这个版本高的文件一个字节都不动。

use std::path::Path;

use aproxy_ipc::diag::{Diagnostic, Level};
use serde_json::{Map, Value};

use super::file::{self, SCHEMA};
use crate::config::edit::{Applied, EditError, rewrite_locked};
use crate::config::{KeySegment, parse_key_path, quote_key};

/// 对一个键的一次改动。键是点号路径，例如 `watchdog.enabled`、`aliases.work`、
/// `defaults.extra_headers."x-foo"`；路径里不能带数组下标。
#[derive(Debug, Clone)]
pub enum Change {
    Set { key: String, value: Value },
    Unset { key: String },
}

impl Change {
    pub fn set(key: &str, value: impl Into<Value>) -> Self {
        Change::Set {
            key: key.to_string(),
            value: value.into(),
        }
    }

    pub fn unset(key: &str) -> Self {
        Change::Unset {
            key: key.to_string(),
        }
    }
}

/// 把 `changes` 应用到 `path` 处的 settings.json。文件不存在时新建，只写给出的键与 `schema`。
/// `home` 定写锁的位置。
pub fn apply(path: &Path, home: &Path, changes: &[Change]) -> Result<Applied, EditError> {
    rewrite_locked(
        path,
        home,
        true,
        || {},
        |old, file| apply_to_text(old, file, changes),
    )
}

/// 把 `changes` 应用到文本上并校验结果，返回新的文本。不碰文件系统。`old` 为 None 表示
/// 文件不存在；`path` 只用来填诊断的 `file`。
pub fn apply_to_text(
    old: Option<&str>,
    path: &Path,
    changes: &[Change],
) -> Result<String, EditError> {
    let old_text = old.unwrap_or("{}");
    let (_, existing) = file::parse(old_text, path);
    // 语法错误、不是对象、schema 太新：没法只改一个键，也不该动一份读不懂的文件
    if let Some(blocking) = existing.iter().find(|d| {
        d.code == "config_syntax" || d.code == "settings_schema_too_new" || d.key.is_none()
    }) {
        return Err(EditError::Syntax(vec![blocking.clone()]));
    }
    let mut root: Value = serde_json::from_str(old_text)
        .map_err(|_| EditError::Syntax(existing.iter().take(1).cloned().collect()))?;
    let before = root.clone();

    for change in changes {
        match change {
            Change::Set { key, value } => {
                let path_segments = plain_path(key)?;
                let value = restore_placeholders(
                    value.clone(),
                    lookup(&before, &path_segments),
                    key,
                    path,
                )?;
                set(&mut root, &path_segments, value);
            }
            Change::Unset { key } => unset(&mut root, &plain_path(key)?),
        }
    }
    if root == before {
        return Ok(old.unwrap_or("").to_string());
    }
    // 写者总是写上 schema，放在最前面
    if let Value::Object(map) = &mut root
        && !map.contains_key("schema")
    {
        let mut with_schema = Map::new();
        with_schema.insert("schema".to_string(), SCHEMA.into());
        with_schema.append(map);
        *map = with_schema;
    }
    let mut new = serde_json::to_string_pretty(&root).expect("JSON 值总能序列化");
    new.push('\n');

    let errors = |diagnostics: Vec<Diagnostic>| -> Vec<Diagnostic> {
        diagnostics
            .into_iter()
            .filter(Diagnostic::is_error)
            .collect()
    };
    let same = |a: &Diagnostic, b: &Diagnostic| {
        a.code == b.code && a.key == b.key && a.message == b.message
    };
    let existing = errors(existing);
    let introduced: Vec<Diagnostic> = errors(file::parse(&new, path).1)
        .into_iter()
        .filter(|d| !existing.iter().any(|e| same(e, d)))
        .collect();
    if introduced.is_empty() {
        Ok(new)
    } else {
        Err(EditError::Invalid(introduced))
    }
}

/// 解析键路径，拒绝带下标的写法。
fn plain_path(key: &str) -> Result<Vec<KeySegment>, EditError> {
    match parse_key_path(key) {
        Some(segments) if segments.iter().all(|s| s.index.is_none()) => Ok(segments),
        _ => Err(EditError::BadKey(key.to_string())),
    }
}

/// 路径处现在的值。
fn lookup<'a>(root: &'a Value, path: &[KeySegment]) -> Option<&'a Value> {
    path.iter()
        .try_fold(root, |value, segment| value.get(&segment.name))
}

/// 设置一个键。路径上缺的对象新建；路径上有一段不是对象时，换成对象（写前校验会判断结果
/// 是否成立）。
fn set(root: &mut Value, path: &[KeySegment], value: Value) {
    let Some((last, parents)) = path.split_last() else {
        return;
    };
    let mut current = root;
    for segment in parents {
        if !current.is_object() {
            *current = Value::Object(Map::new());
        }
        current = current
            .as_object_mut()
            .expect("上一步保证了这里是对象")
            .entry(segment.name.clone())
            .or_insert_with(|| Value::Object(Map::new()));
    }
    if !current.is_object() {
        *current = Value::Object(Map::new());
    }
    if let Some(map) = current.as_object_mut() {
        map.insert(last.name.clone(), value);
    }
}

/// 删除一个键。路径上任何一段不存在，就什么都不做。
fn unset(root: &mut Value, path: &[KeySegment]) {
    let Some((last, parents)) = path.split_last() else {
        return;
    };
    let mut current = root;
    for segment in parents {
        let Some(next) = current.get_mut(&segment.name) else {
            return;
        };
        current = next;
    }
    if let Some(map) = current.as_object_mut() {
        // shift_remove：保持其余键的顺序
        map.shift_remove(&last.name);
    }
}

/// 把写回的值里的占位对象换回原值：占位对象在 `new` 里的位置，对应 `old` 里同一位置的值。
/// `old` 里那个位置没有值（或者它本身不是字符串）时对不上，报 `secret_placeholder_unmatched`。
fn restore_placeholders(
    new: Value,
    old: Option<&Value>,
    key: &str,
    file: &Path,
) -> Result<Value, EditError> {
    if aproxy_ipc::redact::is_placeholder(&new) {
        return match old {
            Some(original @ Value::String(_)) => Ok(original.clone()),
            _ => Err(EditError::Invalid(vec![Diagnostic {
                code: "secret_placeholder_unmatched",
                severity: Level::Error,
                file: file.display().to_string(),
                key: Some(key.to_string()),
                line: None,
                column: None,
                message: format!(
                    "{key} was written back as a secret placeholder, but there is no stored value to keep there; enter the secret again"
                ),
                details: Map::new(),
            }])),
        };
    }
    match new {
        Value::Object(map) => {
            let mut out = Map::new();
            for (name, value) in map {
                let child_key = format!("{key}.{}", quote_key(&name));
                let value = restore_placeholders(
                    value,
                    old.and_then(|old| old.get(&name)),
                    &child_key,
                    file,
                )?;
                out.insert(name, value);
            }
            Ok(Value::Object(out))
        }
        other => Ok(other),
    }
}

/// 读出口用的形状：密钥换成占位对象，URL 里的 userinfo 与查询串的值打码。`value` 是整份
/// settings（文件里的键，或补上内置默认之后的）。
pub fn redacted(mut value: Value) -> Value {
    use aproxy_ipc::redact;
    let secret = |value: &mut Value| {
        if let Value::String(text) = value {
            *value = redact::placeholder(text);
        }
    };
    let url = |value: &mut Value| {
        if let Value::String(text) = value {
            *value = Value::String(redact::url(text));
        }
    };
    if let Some(defaults) = value.get_mut("defaults").and_then(Value::as_object_mut) {
        for key in ["api_key", "proxy_password"] {
            if let Some(item) = defaults.get_mut(key) {
                secret(item);
            }
        }
        for key in ["base_url", "proxy"] {
            if let Some(item) = defaults.get_mut(key) {
                url(item);
            }
        }
        for table in ["extra_headers", "override_headers"] {
            if let Some(headers) = defaults.get_mut(table).and_then(Value::as_object_mut) {
                for (name, item) in headers.iter_mut() {
                    if redact::is_secret_header(name) {
                        secret(item);
                    }
                }
            }
        }
    }
    if let Some(item) = value
        .get_mut("update")
        .and_then(|update| update.get_mut("download_proxy"))
    {
        url(item);
    }
    value
}

/// 内置默认值，写成 settings.json 的形状（没有 `defaults`、`aliases` 里的条目与 `ui` 的
/// 内容——它们默认是空的）。
pub fn builtin_value() -> Value {
    let d = super::Settings::default();
    serde_json::json!({
        "schema": SCHEMA,
        "language": d.language,
        "default_config": null,
        "aliases": {},
        "config_dirs": [],
        "defaults": {},
        "watchdog": {
            "enabled": d.watchdog,
            "heartbeat_secs": d.watchdog_heartbeat_secs,
            "stale_after_cycles": d.watchdog_stale_after_cycles,
            "max_restarts": d.watchdog_max_restarts,
            "idle_exit_secs": d.watchdog_idle_exit_secs,
        },
        "logs": { "rotate_mb": d.log_rotate_mb },
        "instances": { "idle_threshold_secs": d.idle_timeout_secs },
        "update": {
            "download_chain": null,
            "download_proxy": null,
            "skill_auto_update": d.skill_auto_update,
        },
        "ui": {},
    })
}

/// 生效的设置：文件里写了的键盖在内置默认上（对象逐键合并，其余整体替换）。`text` 为 None
/// 表示文件不存在。文件不是合法的 JSON 对象时返回 None。
pub fn effective_value(text: Option<&str>) -> Option<Value> {
    let mut value = builtin_value();
    if let Some(text) = text {
        let file: Value = serde_json::from_str(text).ok()?;
        if !file.is_object() {
            return None;
        }
        merge(&mut value, file);
    }
    Some(value)
}

fn merge(base: &mut Value, over: Value) {
    match (base, over) {
        (Value::Object(base), Value::Object(over)) => {
            for (name, value) in over {
                match base.get_mut(&name) {
                    Some(slot) => merge(slot, value),
                    None => {
                        base.insert(name, value);
                    }
                }
            }
        }
        (base, over) => *base = over,
    }
}

/// 生效设置里一个键路径处的值（见 [`effective_value`]）。
pub fn get<'a>(effective: &'a Value, key: &str) -> Result<Option<&'a Value>, EditError> {
    Ok(lookup(effective, &plain_path(key)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn edit(old: Option<&str>, changes: &[Change]) -> Result<String, EditError> {
        apply_to_text(old, Path::new("settings.json"), changes)
    }

    fn invalid(result: Result<String, EditError>) -> Vec<String> {
        match result {
            Err(EditError::Invalid(diagnostics)) => diagnostics
                .iter()
                .map(|d| format!("{} {}", d.code, d.key.as_deref().unwrap_or("-")))
                .collect(),
            other => panic!("{other:?}"),
        }
    }

    /// 新建的文件只有给出的键与 schema。
    #[test]
    fn a_new_file_holds_only_the_given_keys_and_schema() {
        let new = edit(None, &[Change::set("watchdog.enabled", false)]).unwrap();
        assert_eq!(
            new,
            "{\n  \"schema\": 1,\n  \"watchdog\": {\n    \"enabled\": false\n  }\n}\n"
        );
        // 什么都没改：不产生文件
        assert_eq!(edit(None, &[Change::unset("aliases.nope")]).unwrap(), "");
    }

    /// 只改目标键：其余的键、它们的顺序、`ui` 里的内容都原样。
    #[test]
    fn set_and_unset_touch_only_their_key() {
        let old = r#"{"ui":{"z":1,"a":[null]},"aliases":{"b":"b.toml","a":"a.toml"},"logs":{"rotate_mb":8}}"#;
        let new = edit(
            Some(old),
            &[
                Change::set("logs.rotate_mb", 0),
                Change::set("aliases.c", "c.toml"),
                Change::unset("aliases.b"),
                Change::set("instances.idle_threshold_secs", 60),
            ],
        )
        .unwrap();
        let value: Value = serde_json::from_str(&new).unwrap();
        assert_eq!(
            value,
            json!({
                "schema": 1,
                "ui": {"z": 1, "a": [null]},
                "aliases": {"a": "a.toml", "c": "c.toml"},
                "logs": {"rotate_mb": 0},
                "instances": {"idle_threshold_secs": 60},
            })
        );
        // 键的顺序：schema 在最前，原有的键保持原来的先后，新键在后
        let order: Vec<usize> = [
            "\"schema\"",
            "\"ui\"",
            "\"aliases\"",
            "\"logs\"",
            "\"instances\"",
        ]
        .iter()
        .map(|key| new.find(key).unwrap())
        .collect();
        assert!(order.windows(2).all(|pair| pair[0] < pair[1]), "{new}");
        assert!(
            new.find("\"z\"").unwrap() < new.find("\"a\": [").unwrap(),
            "{new}"
        );
    }

    /// 会引入新错误的改动不写；原有的、无关的错误不挡路，于是能逐项修好。
    #[test]
    fn changes_are_validated_but_existing_errors_do_not_block() {
        let old = r#"{"watchdog": false, "max_body_mb": 64}"#;
        assert_eq!(
            invalid(edit(Some(old), &[Change::set("language", "fr")])),
            ["config_invalid_value language"]
        );
        assert_eq!(
            invalid(edit(Some(old), &[Change::set("logs.level", "debug")])),
            ["config_unknown_key logs.level"]
        );
        // 逐项修：先删不认识的键，再把 watchdog 改成对象
        let step = edit(Some(old), &[Change::unset("max_body_mb")]).unwrap();
        let step = edit(Some(&step), &[Change::set("watchdog.enabled", false)]).unwrap();
        let (settings, diagnostics) = file::parse(&step, Path::new("settings.json"));
        assert_eq!(diagnostics, []);
        assert!(!settings.watchdog);
    }

    /// 凭据要有归属：先设 `defaults.api_key` 被拒，报错说先设 `defaults.base_url`；按这个顺序
    /// 再设就成功。
    #[test]
    fn default_credentials_need_the_target_first() {
        let result = edit(None, &[Change::set("defaults.api_key", "sk-test")]);
        let Err(EditError::Invalid(diagnostics)) = &result else {
            panic!("{result:?}")
        };
        assert_eq!(diagnostics[0].key.as_deref(), Some("defaults.base_url"));
        assert!(
            diagnostics[0]
                .message
                .contains("set defaults.base_url first")
        );

        let step = edit(
            None,
            &[Change::set("defaults.base_url", "https://api.example.com")],
        )
        .unwrap();
        let step = edit(Some(&step), &[Change::set("defaults.api_key", "sk-test")]).unwrap();
        let (settings, diagnostics) = file::parse(&step, Path::new("settings.json"));
        assert_eq!(diagnostics, []);
        assert_eq!(settings.defaults.api_key.as_deref(), Some("sk-test"));
        // 反过来：还有 key 时删掉 base_url 被拒
        assert_eq!(
            invalid(edit(Some(&step), &[Change::unset("defaults.base_url")])),
            ["config_missing_key defaults.base_url"]
        );
    }

    /// 读出口给占位，占位写回不改原值；对不上的占位被拒绝。
    #[test]
    fn placeholders_written_back_keep_the_stored_secret() {
        let old = json!({
            "defaults": {
                "base_url": "https://user:pw@api.example.com/v1?key=q",
                "api_key": "sk-abcdefghijklmnopqrstuvwxyz-0123",
                "proxy": "http://127.0.0.1:7890",
                "proxy_password": "short",
                "extra_headers": {"x-team": "core", "X-Api-Key": "header-secret"},
            },
            "update": {"download_proxy": "http://u:p@dl.example:1"},
        });
        let shown = redacted(old.clone());
        assert_eq!(
            shown,
            json!({
                "defaults": {
                    "base_url": "https://***@api.example.com/v1?key=***",
                    "api_key": {"secret": true, "hint": "sk-a…0123"},
                    "proxy": "http://127.0.0.1:7890",
                    "proxy_password": {"secret": true},
                    "extra_headers": {"x-team": "core", "X-Api-Key": {"secret": true}},
                },
                "update": {"download_proxy": "http://***@dl.example:1"},
            })
        );
        assert!(!shown.to_string().contains("header-secret"));

        let text = old.to_string();
        // 单个键的占位
        let same = edit(
            Some(&text),
            &[Change::set("defaults.api_key", json!({"secret": true}))],
        )
        .unwrap();
        assert_eq!(same, text, "占位写回等于不改");
        // 整段写回：占位换回原值，别的键照改
        let mut whole = shown["defaults"].clone();
        whole["base_url"] = json!("https://api.example.com/v1");
        whole["extra_headers"]["x-team"] = json!("platform");
        let new = edit(Some(&text), &[Change::set("defaults", whole)]).unwrap();
        let value: Value = serde_json::from_str(&new).unwrap();
        assert_eq!(value["defaults"]["api_key"], old["defaults"]["api_key"]);
        assert_eq!(value["defaults"]["proxy_password"], "short");
        assert_eq!(
            value["defaults"]["extra_headers"]["X-Api-Key"],
            "header-secret"
        );
        assert_eq!(value["defaults"]["extra_headers"]["x-team"], "platform");
        assert_eq!(value["defaults"]["base_url"], "https://api.example.com/v1");

        // 没有原值可保持的地方写占位：拒绝
        assert_eq!(
            invalid(edit(
                Some(&text),
                &[Change::set(
                    "defaults.override_headers",
                    json!({"authorization": {"secret": true}})
                )]
            )),
            ["secret_placeholder_unmatched defaults.override_headers.authorization"]
        );
        assert_eq!(
            invalid(edit(
                None,
                &[Change::set("defaults.api_key", json!({"secret": true}))]
            )),
            ["secret_placeholder_unmatched defaults.api_key"]
        );
    }

    /// 读不懂的文件一个字节都不动：语法错误、不是对象、schema 太新。
    #[test]
    fn unreadable_files_are_left_alone() {
        for old in ["{ nope", "[]", r#"{"schema": 2, "new_thing": 1}"#] {
            let result = edit(Some(old), &[Change::set("language", "en")]);
            assert!(
                matches!(result, Err(EditError::Syntax(_))),
                "{old}: {result:?}"
            );
            let result = edit(Some(old), &[Change::unset("anything")]);
            assert!(
                matches!(result, Err(EditError::Syntax(_))),
                "{old}: {result:?}"
            );
        }
    }

    #[test]
    fn unset_accepts_any_key_and_bad_paths_are_refused() {
        let old = r#"{"schema":1,"bogus":{"deep":1},"language":"en"}"#;
        let new = edit(
            Some(old),
            &[
                Change::unset("bogus"),
                Change::unset("never.was.here"),
                Change::unset("language.deeper"),
            ],
        )
        .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&new).unwrap(),
            json!({"schema": 1, "language": "en"})
        );
        assert_eq!(edit(Some(old), &[Change::unset("absent")]).unwrap(), old);
        for key in ["", "a..b", "config_dirs[0]", "a b"] {
            assert!(
                matches!(edit(None, &[Change::unset(key)]), Err(EditError::BadKey(_))),
                "{key}"
            );
        }
    }

    /// 没有文件时，生效值就是内置默认；文件里写了的键盖在上面。
    #[test]
    fn effective_values_fill_in_the_builtin_defaults() {
        let effective = effective_value(None).unwrap();
        assert_eq!(
            get(&effective, "watchdog.heartbeat_secs").unwrap(),
            Some(&json!(30))
        );
        assert_eq!(get(&effective, "language").unwrap(), Some(&json!("system")));
        assert_eq!(get(&effective, "defaults.api_key").unwrap(), None);
        // 内置默认的形状本身是一份有效的 settings.json，读回来就是内置默认
        let (settings, diagnostics) =
            file::parse(&effective.to_string(), Path::new("settings.json"));
        assert_eq!(diagnostics, []);
        assert_eq!(settings, super::super::Settings::default());

        let effective = effective_value(Some(
            r#"{"watchdog":{"enabled":false},"aliases":{"w":"w.toml"}}"#,
        ))
        .unwrap();
        assert_eq!(
            get(&effective, "watchdog.enabled").unwrap(),
            Some(&json!(false))
        );
        assert_eq!(
            get(&effective, "watchdog.max_restarts").unwrap(),
            Some(&json!(5))
        );
        assert_eq!(
            get(&effective, "aliases.w").unwrap(),
            Some(&json!("w.toml"))
        );
        assert_eq!(effective_value(Some("{ nope")), None);
    }

    /// 落盘：新建、原子写；Windows 上文件的 DACL 是显式的三项，unix 上是 0600。
    #[test]
    fn apply_writes_a_private_file() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("settings.json");
        let applied = apply(&path, home.path(), &[Change::set("language", "en")]).unwrap();
        assert!(applied.changed);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "{\n  \"schema\": 1,\n  \"language\": \"en\"\n}\n"
        );
        let applied = apply(&path, home.path(), &[Change::set("language", "en")]).unwrap();
        assert!(!applied.changed);
        #[cfg(windows)]
        {
            let sddl = crate::fsx::dacl_sddl(&path).unwrap();
            assert!(sddl.starts_with("D:P"), "{sddl}");
            assert_eq!(sddl.matches("(A;;FA;;;").count(), 3, "{sddl}");
            assert!(sddl.contains(";;;SY)") && sddl.contains(";;;BA)"), "{sddl}");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    /// 多个写者并发地各加各的别名：一个更新都不丢。
    #[test]
    fn concurrent_writers_do_not_lose_updates() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("settings.json");
        const WRITERS: usize = 8;
        std::thread::scope(|scope| {
            for writer in 0..WRITERS {
                let (path, home) = (&path, home.path());
                scope.spawn(move || {
                    for round in 0..4 {
                        let key = format!("aliases.w{writer}r{round}");
                        apply(path, home, &[Change::set(&key, "x.toml")]).unwrap();
                    }
                });
            }
        });
        let (settings, diagnostics) = file::parse(&std::fs::read_to_string(&path).unwrap(), &path);
        assert_eq!(diagnostics, []);
        assert_eq!(settings.aliases.len(), WRITERS * 4);
    }
}
