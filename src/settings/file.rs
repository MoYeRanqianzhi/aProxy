//! settings.json 的严格解析：把文件读成 [`Settings`]，连同诊断。
//!
//! - **严格**：不认识的键、类型或取值不对，都是 error 级诊断，带文件与键路径。settings.json
//!   是所有版本共写的文件，一个被悄悄忽略的键意味着用户以为生效的设置其实没有生效。
//! - **`defaults` 段**与 config.toml 用同一个解析器（[`crate::config::file`]）：它收的就是
//!   config.toml 的键，两处对「一个键长什么样」的看法不会分叉。
//! - **`ui` 段**是桌面界面私有的：这里只检查它是对象，从不读取、也不校验里面的内容——界面
//!   加一个偏好，不应让同一台机器上的旧版命令行起不来。
//! - **不读文件系统**：输入是文本，输出是值，可以单测。
//!
//! 诊断的 `message` 是英文；显示端按码去说各自语言的话。

use std::collections::{HashMap, HashSet};
use std::path::Path;

use aproxy_ipc::diag::{Diagnostic, Level};
use serde_json::{Map, Value, json};

use super::Settings;
use crate::config::file::FileConfig;
use crate::config::quote_key;

/// 这个版本读写的 settings.json 的 schema。
pub const SCHEMA: u64 = 1;

/// `language` 的取值。
pub const LANGUAGES: &[&str] = &["system", "en", "zh-CN"];

/// `defaults` 里不收的 config.toml 键：它们属于单个实例，两个实例不可能相同。
const NOT_IN_DEFAULTS: &[&str] = &["listen_addr", "log_file", "format"];

/// 解析 settings.json 的文本。`file` 只用来填诊断的 `file` 字段。
///
/// 有 error 级诊断时，返回的 [`Settings`] 里读不出来的键是内置默认值；调用方不应拿这样一份
/// 半对半错的设置去做事（见 [`super::load`]）。
pub fn parse(text: &str, file: &Path) -> (Settings, Vec<Diagnostic>) {
    let mut ctx = Ctx {
        file: file.display().to_string(),
        diagnostics: Vec::new(),
    };
    let mut settings = Settings::default();
    let root: Value = match serde_json::from_str(text) {
        Ok(value) => value,
        Err(e) => {
            ctx.diagnostics.push(Diagnostic {
                code: "config_syntax",
                severity: Level::Error,
                file: ctx.file.clone(),
                key: None,
                line: u32::try_from(e.line()).ok().filter(|l| *l > 0),
                column: u32::try_from(e.column()).ok().filter(|c| *c > 0),
                message: format!("JSON syntax error: {e}"),
                details: Map::new(),
            });
            return (settings, ctx.diagnostics);
        }
    };
    let Some(root) = root.as_object() else {
        ctx.push(
            "config_type",
            "",
            "settings.json must be a JSON object".to_string(),
            json!({}),
        );
        return (settings, ctx.diagnostics);
    };
    let root = Obj::new(root, String::new());

    // schema：缺省按 1 读。更高的 schema 是更新的 aproxy 写的，里面的键这个版本未必读得对，
    // 所以到此为止，不再往下报一串「不认识的键」
    match root.get("schema") {
        None => {}
        Some(value) => match value.as_u64() {
            Some(SCHEMA) => {}
            Some(schema) if schema > SCHEMA => {
                ctx.push(
                    "settings_schema_too_new",
                    "schema",
                    format!(
                        "settings.json has schema {schema}, written by a newer aproxy; this version reads schema {SCHEMA}. Upgrade aproxy, or go back to the version you need with `aproxy install --force <version>`"
                    ),
                    json!({}),
                );
                return (settings, ctx.diagnostics);
            }
            _ => ctx.push(
                "config_invalid_value",
                "schema",
                format!("schema must be {SCHEMA}"),
                json!({ "reason": "out_of_range" }),
            ),
        },
    }

    if let Some(language) = root.string(&mut ctx, "language") {
        if LANGUAGES.contains(&language.as_str()) {
            settings.language = language;
        } else {
            ctx.push(
                "config_invalid_value",
                "language",
                format!("language must be \"system\", \"en\" or \"zh-CN\", not {language:?}"),
                json!({ "reason": "not_one_of" }),
            );
        }
    }
    settings.default_config = root.nullable_string(&mut ctx, "default_config");
    if let Some(aliases) = root.object(&mut ctx, "aliases") {
        settings.aliases = read_aliases(&mut ctx, &aliases);
    }
    if let Some(dirs) = root.string_list(&mut ctx, "config_dirs") {
        settings.config_dirs = dirs;
    }
    if let Some(defaults) = root.object(&mut ctx, "defaults") {
        settings.defaults = read_defaults(&mut ctx, &defaults, file);
    }
    if let Some(w) = root.object(&mut ctx, "watchdog") {
        if let Some(v) = w.boolean(&mut ctx, "enabled") {
            settings.watchdog = v;
        }
        if let Some(v) = w.integer(&mut ctx, "heartbeat_secs") {
            // 0 秒的扫描周期会让看护者空转
            if v == 0 {
                ctx.push(
                    "config_invalid_value",
                    &w.key_path("heartbeat_secs"),
                    "watchdog.heartbeat_secs must be at least 1".to_string(),
                    json!({ "reason": "out_of_range" }),
                );
            } else {
                settings.watchdog_heartbeat_secs = v;
            }
        }
        if let Some(v) = w.integer(&mut ctx, "stale_after_cycles") {
            // 看护者把 0 按 1 处理；在这里报错才不会让用户误以为写下的值生效了。
            if v == 0 {
                ctx.push(
                    "config_invalid_value",
                    &w.key_path("stale_after_cycles"),
                    "watchdog.stale_after_cycles must be at least 1".to_string(),
                    json!({ "reason": "out_of_range" }),
                );
            } else {
                settings.watchdog_stale_after_cycles = v;
            }
        }
        if let Some(v) = w.integer(&mut ctx, "max_restarts") {
            match u32::try_from(v) {
                Ok(v) => settings.watchdog_max_restarts = v,
                Err(_) => ctx.push(
                    "config_invalid_value",
                    &w.key_path("max_restarts"),
                    format!("{v} is too large"),
                    json!({ "reason": "out_of_range" }),
                ),
            }
        }
        if let Some(v) = w.integer(&mut ctx, "idle_exit_secs") {
            settings.watchdog_idle_exit_secs = v;
        }
        w.finish(&mut ctx);
    }
    if let Some(logs) = root.object(&mut ctx, "logs") {
        if let Some(v) = logs.integer(&mut ctx, "rotate_mb") {
            settings.log_rotate_mb = v;
        }
        logs.finish(&mut ctx);
    }
    if let Some(instances) = root.object(&mut ctx, "instances") {
        if let Some(v) = instances.integer(&mut ctx, "idle_threshold_secs") {
            settings.idle_timeout_secs = v;
        }
        instances.finish(&mut ctx);
    }
    if let Some(update) = root.object(&mut ctx, "update") {
        if let Some(chain) = update.get("download_chain").filter(|v| !v.is_null()) {
            // 元素是渠道名或 `{"url": …}`，它的读法在下载模块里
            match serde_json::from_value(chain.clone()) {
                Ok(chain) => settings.download_chain = Some(chain),
                Err(e) => ctx.push(
                    "config_invalid_value",
                    &update.key_path("download_chain"),
                    format!(
                        "update.download_chain must be an array of \"github\", \"npm\", \"cargo-binstall\", \"cargo\" or {{\"url\": …}}: {e}"
                    ),
                    json!({ "reason": "not_one_of" }),
                ),
            }
        }
        settings.download_proxy = update.nullable_string(&mut ctx, "download_proxy");
        if let Some(v) = update.boolean(&mut ctx, "skill_auto_update") {
            settings.skill_auto_update = v;
        }
        update.finish(&mut ctx);
    }
    // ui：只要求是对象
    if let Some(ui) = root.get("ui")
        && !ui.is_object()
    {
        ctx.push(
            "config_type",
            "ui",
            "ui must be an object".to_string(),
            json!({}),
        );
    }
    root.finish(&mut ctx);
    (settings, ctx.diagnostics)
}

/// 别名表。名字不合规（纯数字、保留字）的别名是 error：`start <别名>` 不会按用户想的那样
/// 解析。指向的文件在不在不属于这里——那是随时会变的现场，由 doctor 去看。
fn read_aliases(ctx: &mut Ctx, aliases: &Obj) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for (name, value) in aliases.map {
        let key = aliases.key_path(&quote_key(name));
        let Some(path) = value.as_str() else {
            ctx.push(
                "config_type",
                &key,
                format!("{key} must be a string"),
                json!({}),
            );
            continue;
        };
        if let Some(reason) = super::alias_name_problem(name) {
            ctx.push(
                "config_invalid_value",
                &key,
                format!("{name:?} cannot be used as an alias: {reason}"),
                json!({ "reason": "reserved_name" }),
            );
            continue;
        }
        if path.trim().is_empty() {
            ctx.push(
                "config_invalid_value",
                &key,
                format!("alias {name:?} points at an empty path"),
                json!({ "reason": "empty" }),
            );
            continue;
        }
        out.insert(name.clone(), path.to_string());
    }
    out
}

/// `defaults` 段：config.toml 键的全局默认。把这个 JSON 对象写成一份 TOML 文本，交给
/// config.toml 的解析器读；它报的诊断改挂到 settings.json 上（键路径加 `defaults.` 前缀，
/// 行列对那份临时文本才有意义，去掉）。
fn read_defaults(ctx: &mut Ctx, defaults: &Obj, file: &Path) -> FileConfig {
    let mut document = toml_edit::DocumentMut::new();
    for (name, value) in defaults.map {
        if NOT_IN_DEFAULTS.contains(&name.as_str()) {
            ctx.push(
                "config_unknown_key",
                &defaults.key_path(name),
                format!(
                    "{name} cannot have a global default: it belongs to a single instance, so set it in that instance's config.toml"
                ),
                json!({}),
            );
            continue;
        }
        match to_toml(value) {
            Ok(value) => {
                document.insert(name, toml_edit::Item::Value(value));
            }
            Err(inner) => {
                let mut key = defaults.key_path(&quote_key(name));
                key.push_str(&inner);
                ctx.push(
                    "config_type",
                    &key,
                    format!("{key} has a value that a config key cannot hold (null, or a number out of range)"),
                    json!({}),
                );
            }
        }
    }
    let (values, diagnostics) = crate::config::file::parse(&document.to_string(), file);
    for mut d in diagnostics {
        d.key = d.key.map(|key| defaults.key_path(&key));
        d.line = None;
        d.column = None;
        ctx.diagnostics.push(d);
    }

    // 凭据要有归属：有 key 或密钥头却没有 base_url，这把 key 就会被继承到每一个配置、发给
    // 每一个上游；代理口令同理
    let has_secret_header = [&values.extra_headers, &values.override_headers]
        .into_iter()
        .flatten()
        .flat_map(|headers| headers.keys())
        .any(|name| aproxy_ipc::redact::is_secret_header(name));
    if (values.api_key.is_some() || has_secret_header) && values.base_url.is_none() {
        ctx.push(
            "config_missing_key",
            &defaults.key_path("base_url"),
            "defaults has an api_key or a credential header but no base_url: set defaults.base_url first, so the credential is only sent to that upstream".to_string(),
            json!({}),
        );
    }
    if (values.proxy_username.is_some() || values.proxy_password.is_some())
        && values.proxy.is_none()
    {
        ctx.push(
            "config_missing_key",
            &defaults.key_path("proxy"),
            "defaults has proxy credentials but no proxy: set defaults.proxy first".to_string(),
            json!({}),
        );
    }

    // 值本身的语义问题（URL 的形状、心跳的分帧……）：与 config.toml 同一套校验。拿这一层
    // 单独解析一份空配置，留下指向这一层的问题
    let layer = crate::config::resolve::Defaults {
        file: file.to_path_buf(),
        values: values.clone(),
    };
    let resolved = crate::config::resolve::resolve_text(
        None,
        Path::new(""),
        &crate::config::Overrides::default(),
        &layer,
        false,
    );
    for d in resolved.diagnostics {
        let from_this_layer = d
            .key
            .as_deref()
            .is_some_and(|key| key.starts_with("defaults."));
        let already = ctx
            .diagnostics
            .iter()
            .any(|have| have.code == d.code && have.key == d.key);
        // 只要 error 级的：告警（例如凭据没有被继承）说的是某一份配置，不是这一层写错了
        if d.is_error() && from_this_layer && !already {
            ctx.diagnostics.push(d);
        }
    }
    values
}

/// JSON 值 → TOML 值。对象成内联表，数组成数组。TOML 装不下的值（null、超出 i64 的整数、
/// 非有限的浮点数）返回 Err，带着它在这个值内部的路径（例如 `.x[2]`，就是它自己时为空串）。
fn to_toml(value: &Value) -> Result<toml_edit::Value, String> {
    Ok(match value {
        Value::Null => return Err(String::new()),
        Value::Bool(b) => (*b).into(),
        Value::String(s) => s.as_str().into(),
        Value::Number(n) => match (n.as_i64(), n.as_f64()) {
            (Some(i), _) => i.into(),
            (None, Some(f)) if n.is_f64() && f.is_finite() => f.into(),
            _ => return Err(String::new()),
        },
        Value::Array(items) => {
            let mut array = toml_edit::Array::new();
            for (index, item) in items.iter().enumerate() {
                array.push(to_toml(item).map_err(|inner| format!("[{index}]{inner}"))?);
            }
            array.into()
        }
        Value::Object(map) => {
            let mut table = toml_edit::InlineTable::new();
            for (name, item) in map {
                let value =
                    to_toml(item).map_err(|inner| format!(".{}{inner}", quote_key(name)))?;
                table.insert(name, value);
            }
            table.into()
        }
    })
}

/// 解析过程中的共享状态：文件名与攒下的诊断。
struct Ctx {
    file: String,
    diagnostics: Vec<Diagnostic>,
}

impl Ctx {
    /// 记一条 error 级诊断。`key` 为空串表示问题不属于某个键
    fn push(&mut self, code: &'static str, key: &str, message: String, details: Value) {
        self.diagnostics.push(Diagnostic {
            code,
            severity: Level::Error,
            file: self.file.clone(),
            key: (!key.is_empty()).then(|| key.to_string()),
            line: None,
            column: None,
            message,
            details: match details {
                Value::Object(map) => map,
                _ => Map::new(),
            },
        });
    }
}

/// 一个 JSON 对象，连同它的键路径与已经读过的键：读完后剩下的都是不认识的键。
struct Obj<'a> {
    map: &'a Map<String, Value>,
    /// 根为空串，其余如 `watchdog`、`defaults`
    path: String,
    seen: std::cell::RefCell<HashSet<&'static str>>,
}

impl<'a> Obj<'a> {
    fn new(map: &'a Map<String, Value>, path: String) -> Self {
        Obj {
            map,
            path,
            seen: Default::default(),
        }
    }

    fn key_path(&self, key: &str) -> String {
        if self.path.is_empty() {
            key.to_string()
        } else {
            format!("{}.{key}", self.path)
        }
    }

    /// 取一个键的值，并记下这个键认识
    fn get(&self, key: &'static str) -> Option<&'a Value> {
        self.seen.borrow_mut().insert(key);
        self.map.get(key)
    }

    fn wrong_type(&self, ctx: &mut Ctx, key: &str, expected: &str) {
        let path = self.key_path(key);
        ctx.push(
            "config_type",
            &path,
            format!("{path} must be {expected}"),
            json!({}),
        );
    }

    fn string(&self, ctx: &mut Ctx, key: &'static str) -> Option<String> {
        let value = self.get(key)?;
        let string = value.as_str().map(str::to_string);
        if string.is_none() {
            self.wrong_type(ctx, key, "a string");
        }
        string
    }

    /// 字符串或 null（null 与没写一样）
    fn nullable_string(&self, ctx: &mut Ctx, key: &'static str) -> Option<String> {
        match self.get(key)? {
            Value::Null => None,
            Value::String(s) => Some(s.clone()),
            _ => {
                self.wrong_type(ctx, key, "a string or null");
                None
            }
        }
    }

    /// 非负整数
    fn integer(&self, ctx: &mut Ctx, key: &'static str) -> Option<u64> {
        let value = self.get(key)?;
        let integer = value.as_u64();
        if integer.is_none() {
            self.wrong_type(ctx, key, "a non-negative integer");
        }
        integer
    }

    fn boolean(&self, ctx: &mut Ctx, key: &'static str) -> Option<bool> {
        let value = self.get(key)?;
        let boolean = value.as_bool();
        if boolean.is_none() {
            self.wrong_type(ctx, key, "true or false");
        }
        boolean
    }

    fn string_list(&self, ctx: &mut Ctx, key: &'static str) -> Option<Vec<String>> {
        let value = self.get(key)?;
        let list: Option<Vec<String>> = value.as_array().and_then(|items| {
            items
                .iter()
                .map(|item| item.as_str().map(str::to_string))
                .collect()
        });
        if list.is_none() {
            self.wrong_type(ctx, key, "an array of strings");
        }
        list
    }

    fn object(&self, ctx: &mut Ctx, key: &'static str) -> Option<Obj<'a>> {
        let value = self.get(key)?;
        match value.as_object() {
            Some(map) => Some(Obj::new(map, self.key_path(key))),
            None => {
                self.wrong_type(ctx, key, "an object");
                None
            }
        }
    }

    /// 读完了：剩下没读过的键都是不认识的
    fn finish(&self, ctx: &mut Ctx) {
        let seen = self.seen.borrow();
        for key in self.map.keys() {
            // schema 与 ui 在根上单独处理过
            let handled =
                seen.contains(key.as_str()) || (self.path.is_empty() && key.as_str() == "ui");
            if !handled {
                let path = self.key_path(&quote_key(key));
                ctx.push(
                    "config_unknown_key",
                    &path,
                    format!("unknown key {path}"),
                    json!({}),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::download::ChainStep;

    fn parse_text(text: &str) -> (Settings, Vec<Diagnostic>) {
        parse(text, Path::new("settings.json"))
    }

    /// 诊断压成 `码 键路径`
    fn brief(diagnostics: &[Diagnostic]) -> Vec<String> {
        diagnostics
            .iter()
            .map(|d| format!("{} {}", d.code, d.key.as_deref().unwrap_or("-")))
            .collect()
    }

    /// 设计稿 §8.1 的样例（密钥换成一个具体的值）
    const SAMPLE: &str = r#"{"schema":1,
 "language":"system",
 "default_config":null,
 "aliases":{"work":"C:\\Users\\u\\.aproxy\\configs\\work.toml"},
 "config_dirs":[],
 "defaults":{"base_url":"https://api.example.com","api_key":"sk-test","extra_headers":{"x-team":"core"},"retry":{"max_backoff_secs":120}},
 "watchdog":{"enabled":true,"heartbeat_secs":30,"stale_after_cycles":1,"max_restarts":5,"idle_exit_secs":300},
 "logs":{"rotate_mb":8},
 "instances":{"idle_threshold_secs":1800},
 "update":{"download_chain":null,"download_proxy":null,"skill_auto_update":true},
 "ui":{}}"#;

    #[test]
    fn the_documented_sample_parses() {
        let (settings, diagnostics) = parse_text(SAMPLE);
        assert_eq!(brief(&diagnostics), Vec::<String>::new());
        assert_eq!(settings.language, "system");
        assert_eq!(settings.default_config, None);
        assert_eq!(
            settings.aliases["work"],
            r"C:\Users\u\.aproxy\configs\work.toml"
        );
        assert_eq!(
            settings.defaults.base_url.as_deref(),
            Some("https://api.example.com")
        );
        assert_eq!(settings.defaults.api_key.as_deref(), Some("sk-test"));
        assert_eq!(
            settings.defaults.extra_headers.as_ref().unwrap()["x-team"],
            "core"
        );
        assert_eq!(settings.defaults.retry.max_backoff_secs, Some(120));
        // 样例写的都是内置默认值
        let builtin = Settings::default();
        assert_eq!(settings.watchdog, builtin.watchdog);
        assert_eq!(settings.log_rotate_mb, builtin.log_rotate_mb);
        assert_eq!(settings.idle_timeout_secs, builtin.idle_timeout_secs);
        assert_eq!(settings.download_chain, None);
    }

    #[test]
    fn an_empty_object_is_all_defaults() {
        let (settings, diagnostics) = parse_text("{}");
        assert_eq!(diagnostics, []);
        assert_eq!(settings, Settings::default());
    }

    #[test]
    fn every_section_is_read() {
        let (settings, diagnostics) = parse_text(
            r#"{
  "language": "zh-CN",
  "default_config": "~/main.toml",
  "config_dirs": ["~/more"],
  "defaults": {
    "max_request_mb": 16,
    "delivery": "forward",
    "timeouts": {"connect_secs": 5},
    "keepalive": {"trigger": "accept"},
    "retry": {"rules": [{"path": "/v1/count", "max_attempts": 3}]},
    "allowed_hosts": ["h"]
  },
  "watchdog": {"enabled": false, "heartbeat_secs": 10, "stale_after_cycles": 2, "max_restarts": 9, "idle_exit_secs": 0},
  "logs": {"rotate_mb": 0},
  "instances": {"idle_threshold_secs": 60},
  "update": {"download_chain": ["npm", {"url": "https://cdn.example/{asset}"}], "download_proxy": "http://127.0.0.1:7890", "skill_auto_update": false},
  "ui": {"anything": [1, {"goes": null}]}
}"#,
        );
        assert_eq!(brief(&diagnostics), Vec::<String>::new());
        assert_eq!(settings.language, "zh-CN");
        assert_eq!(settings.default_config.as_deref(), Some("~/main.toml"));
        assert_eq!(settings.config_dirs, ["~/more"]);
        assert_eq!(settings.defaults.max_request_mb, Some(16));
        assert_eq!(
            settings.defaults.delivery,
            Some(crate::config::file::Delivery::Forward)
        );
        assert_eq!(settings.defaults.timeouts.connect_secs, Some(5));
        assert_eq!(settings.defaults.retry.rules.as_ref().unwrap().len(), 1);
        assert!(!settings.watchdog);
        assert_eq!(settings.watchdog_heartbeat_secs, 10);
        assert_eq!(settings.watchdog_stale_after_cycles, 2);
        assert_eq!(settings.watchdog_max_restarts, 9);
        assert_eq!(settings.watchdog_idle_exit_secs, 0);
        assert_eq!(settings.log_rotate_mb, 0);
        assert_eq!(settings.idle_timeout_secs, 60);
        assert_eq!(
            settings.download_chain,
            Some(vec![
                ChainStep::Npm,
                ChainStep::Url {
                    url: "https://cdn.example/{asset}".to_string()
                }
            ])
        );
        assert_eq!(
            settings.download_proxy.as_deref(),
            Some("http://127.0.0.1:7890")
        );
        assert!(!settings.skill_auto_update);
    }

    /// 不认识的键是 error，带键路径——根上、各段里、`defaults` 里都一样；0.1.x 的平铺字段
    /// 也只是「不认识」。
    #[test]
    fn unknown_keys_are_errors_with_their_path() {
        let (_, diagnostics) = parse_text(
            r#"{
  "max_body_mb": 64,
  "watchdog": {"enabled": true, "interval": 3},
  "logs": {"level": "debug"},
  "update": {"channel": "beta"},
  "defaults": {"forward_only": true, "timeouts": {"total_secs": 1}, "listen_addr": "127.0.0.1:1", "format": []}
}"#,
        );
        assert_eq!(
            brief(&diagnostics),
            [
                "config_unknown_key defaults.listen_addr",
                "config_unknown_key defaults.format",
                "config_unknown_key defaults.timeouts.total_secs",
                "config_unknown_key defaults.forward_only",
                "config_unknown_key watchdog.interval",
                "config_unknown_key logs.level",
                "config_unknown_key update.channel",
                "config_unknown_key max_body_mb",
            ]
        );
        assert!(
            diagnostics.iter().all(|d| d.severity == Level::Error
                && d.file == "settings.json"
                && d.line.is_none())
        );
    }

    /// 0.1.x 写的 `{"watchdog": false}`：类型不对，指出键。
    #[test]
    fn the_flat_0_1_shape_is_rejected() {
        let (_, diagnostics) = parse_text(r#"{"watchdog": false, "log_rotate_mb": 8}"#);
        assert_eq!(
            brief(&diagnostics),
            ["config_type watchdog", "config_unknown_key log_rotate_mb"]
        );
    }

    #[test]
    fn a_newer_schema_is_refused_with_the_way_out() {
        let (settings, diagnostics) =
            parse_text(r#"{"schema": 2, "brand_new_section": {}, "watchdog": {"enabled": false}}"#);
        // 只报这一条，不为读不懂的内容报一串未知键
        assert_eq!(brief(&diagnostics), ["settings_schema_too_new schema"]);
        let message = &diagnostics[0].message;
        assert!(
            message.contains("Upgrade aproxy") && message.contains("install --force"),
            "{message}"
        );
        assert_eq!(settings, Settings::default());

        let (_, diagnostics) = parse_text(r#"{"schema": 0}"#);
        assert_eq!(brief(&diagnostics), ["config_invalid_value schema"]);
        let (_, diagnostics) = parse_text(r#"{"schema": "1"}"#);
        assert_eq!(brief(&diagnostics), ["config_invalid_value schema"]);
    }

    #[test]
    fn syntax_errors_carry_a_position() {
        let (settings, diagnostics) = parse_text("{\n  \"language\": \n}");
        assert_eq!(settings, Settings::default());
        assert_eq!(brief(&diagnostics), ["config_syntax -"]);
        assert_eq!(diagnostics[0].line, Some(3));
        let (_, diagnostics) = parse_text("[]");
        assert_eq!(brief(&diagnostics), ["config_type -"]);
    }

    #[test]
    fn wrong_types_and_values() {
        let (settings, diagnostics) = parse_text(
            r#"{
  "language": "fr",
  "default_config": 1,
  "aliases": {"ok": "a.toml", "12345": "b.toml", "all": "c.toml", "bad": 7, "empty": " "},
  "config_dirs": "dir",
  "watchdog": {"enabled": "yes", "heartbeat_secs": 0, "max_restarts": -1},
  "logs": [],
  "update": {"download_chain": ["ftp"], "download_proxy": 5},
  "ui": []
}"#,
        );
        let mut got = brief(&diagnostics);
        got.sort();
        assert_eq!(
            got,
            [
                "config_invalid_value aliases.12345",
                "config_invalid_value aliases.all",
                "config_invalid_value aliases.empty",
                "config_invalid_value language",
                "config_invalid_value update.download_chain",
                "config_invalid_value watchdog.heartbeat_secs",
                "config_type aliases.bad",
                "config_type config_dirs",
                "config_type default_config",
                "config_type logs",
                "config_type ui",
                "config_type update.download_proxy",
                "config_type watchdog.enabled",
                "config_type watchdog.max_restarts",
            ]
        );
        // 读得出来的照读，读不出来的是默认值
        assert_eq!(settings.aliases.len(), 1);
        assert_eq!(settings.language, "system");
        assert_eq!(settings.watchdog_heartbeat_secs, 30);
    }

    /// 挂死容忍周期不能写成零；诊断指到该键，解析结果保留可用的默认值。
    #[test]
    fn zero_stale_after_cycles_is_rejected() {
        let (settings, diagnostics) = parse_text(r#"{"watchdog":{"stale_after_cycles":0}}"#);
        assert_eq!(
            brief(&diagnostics),
            ["config_invalid_value watchdog.stale_after_cycles"]
        );
        let d = &diagnostics[0];
        assert!(d.is_error());
        assert_eq!(d.message, "watchdog.stale_after_cycles must be at least 1");
        assert_eq!(d.details["reason"], "out_of_range");
        assert_eq!(
            settings.watchdog_stale_after_cycles,
            Settings::default().watchdog_stale_after_cycles
        );
    }

    /// `ui` 里放什么都不报错。
    #[test]
    fn ui_is_opaque() {
        let (_, diagnostics) = parse_text(
            r#"{"ui": {"theme": 7, "schema": 99, "defaults": {"base_url": 1}, "x": [null, {"y": {}}]}}"#,
        );
        assert_eq!(diagnostics, []);
    }

    /// `defaults` 里的值与 config.toml 同一套校验，诊断挂在 `defaults.` 下。
    #[test]
    fn defaults_are_validated_like_config_toml() {
        let (_, diagnostics) = parse_text(
            r#"{"defaults": {
  "base_url": "ftp://a.example",
  "max_request_mb": "big",
  "disk_cache": null,
  "proxy": "gopher://p.example",
  "keepalive": {"trigger": "always", "heartbeat": "data: x"},
  "retry": {"rules": [{"path": "(", "max_attempts": 1}, {"path": "/x"}]},
  "extra_headers": {"bad name": "v"}
}}"#,
        );
        let mut got = brief(&diagnostics);
        got.sort();
        assert_eq!(
            got,
            [
                "config_invalid_value defaults.base_url",
                "config_invalid_value defaults.extra_headers.\"bad name\"",
                "config_invalid_value defaults.keepalive.heartbeat",
                "config_invalid_value defaults.keepalive.trigger",
                "config_invalid_value defaults.proxy",
                "config_invalid_value defaults.retry.rules[0].path",
                "config_missing_key defaults.retry.rules[1].max_attempts",
                "config_type defaults.disk_cache",
                "config_type defaults.max_request_mb",
            ]
        );
    }

    /// 凭据要有归属：有 key 或密钥头却没有 `base_url`、有代理口令却没有 `proxy`，都是 error。
    #[test]
    fn default_credentials_need_a_target() {
        let cases = [
            (r#"{"defaults": {"api_key": "k"}}"#, "defaults.base_url"),
            (
                r#"{"defaults": {"extra_headers": {"X-Api-Key": "k"}}}"#,
                "defaults.base_url",
            ),
            (
                r#"{"defaults": {"override_headers": {"authorization": "Bearer k"}}}"#,
                "defaults.base_url",
            ),
            (r#"{"defaults": {"proxy_password": "p"}}"#, "defaults.proxy"),
        ];
        for (text, key) in cases {
            let (_, diagnostics) = parse_text(text);
            assert_eq!(
                brief(&diagnostics),
                [format!("config_missing_key {key}")],
                "{text}"
            );
        }
        // 报错直接说先设哪个键
        let (_, diagnostics) = parse_text(cases[0].0);
        assert!(
            diagnostics[0]
                .message
                .contains("set defaults.base_url first")
        );

        // 不是密钥的头不需要归属
        let (_, diagnostics) = parse_text(r#"{"defaults": {"extra_headers": {"x-team": "core"}}}"#);
        assert_eq!(diagnostics, []);
        let (_, diagnostics) = parse_text(
            r#"{"defaults": {"base_url": "https://a.example", "api_key": "k", "proxy": "http://p.example:1", "proxy_password": "p"}}"#,
        );
        assert_eq!(diagnostics, []);
    }

    /// 每条诊断的码都在码表的诊断族里，`details` 的键也都登记过。
    #[test]
    fn diagnostics_match_the_code_table() {
        let texts = [
            "{",
            r#"{"schema": 9}"#,
            r#"{"language": "fr", "nope": 1, "ui": 3}"#,
            r#"{"aliases": {"all": "x"}, "watchdog": {"heartbeat_secs": 0}}"#,
            r#"{"defaults": {"api_key": "k", "base_url": 3, "listen_addr": "x"}}"#,
            r#"{"update": {"download_chain": [1]}}"#,
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
                        .contains(&aproxy_ipc::code::Family::Diagnostic)
                );
                for key in d.details.keys() {
                    assert!(
                        entry.details.iter().any(|detail| detail.key == key),
                        "{} has no detail {key}",
                        d.code
                    );
                }
            }
        }
    }
}
