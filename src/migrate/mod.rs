//! 旧文件只在独立的迁移命令里改写。识别与改写共用键表，避免普通命令把拼写错误误报为旧配置。

use crate::{config, settings};
use aproxy_ipc::{diag::Diagnostic, i18n::Lang};
use serde::Serialize;
use serde_json::{Value, json};
use std::path::Path;
use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, TableLike};

mod files;
pub use files::run;
#[path = "../i18n/migrate.rs"]
mod text;

const SETTINGS: &[(&str, &str)] = &[
    ("max_body_mb", "defaults.max_request_mb"),
    ("disk_cache", "defaults.disk_cache"),
    ("forward_only", "defaults.delivery"),
    ("bounded_retry_paths", "defaults.retry.rules"),
    ("allowed_hosts", "defaults.allowed_hosts"),
    ("allowed_origins", "defaults.allowed_origins"),
    ("keepalive_trigger", "defaults.keepalive.trigger"),
    ("watchdog", "watchdog.enabled"),
    ("watchdog_heartbeat_secs", "watchdog.heartbeat_secs"),
    ("watchdog_stale_after_cycles", "watchdog.stale_after_cycles"),
    ("watchdog_max_restarts", "watchdog.max_restarts"),
    ("watchdog_idle_exit_secs", "watchdog.idle_exit_secs"),
    ("log_rotate_mb", "logs.rotate_mb"),
    ("idle_timeout_secs", "instances.idle_threshold_secs"),
    ("download_chain", "update.download_chain"),
    ("download_proxy", "update.download_proxy"),
    ("skill_auto_update", "update.skill_auto_update"),
];
const CONFIG: &[(&str, &str)] = &[
    ("max_body_mb", "max_request_mb"),
    ("spool_limit_mb", "max_response_mb"),
    ("forward_only", "delivery"),
    ("connect_timeout_secs", "timeouts.connect_secs"),
    ("read_timeout_secs", "timeouts.first_byte_secs"),
    ("read_timeout_secs", "timeouts.read_idle_secs"),
    ("keepalive_interval_secs", "keepalive.interval_secs"),
    ("keepalive_trigger", "keepalive.trigger"),
    ("keepalive_heartbeat", "keepalive.heartbeat"),
    ("max_retry_backoff_secs", "retry.max_backoff_secs"),
    ("bounded_retry_paths", "retry.rules"),
];
const FORMAT: &[(&str, &str)] = &[
    ("mode", "process"),
    ("idle_timeout_secs", "worker_idle_secs"),
    ("every_attempt", "stages"),
];
const MANUAL: &[&str] = &[
    "request_transform",
    "response_transform",
    "heartbeat_transform",
];

/// 文件类型由发现入口决定，不靠扩展名猜：别名可以指向任意文件名。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// 主目录的 settings.json。
    Settings,
    /// 一个实例的 TOML 配置。
    Config,
}

impl Kind {
    fn section(self) -> &'static str {
        match self {
            Self::Settings => "compatibility.md#settingsjson-keys-renamed-in-020",
            Self::Config => "compatibility.md#config-keys-renamed-in-020",
        }
    }
    fn errors(self, src: &str, path: &Path) -> Vec<Diagnostic> {
        match self {
            Self::Settings => settings::file::parse(src, path).1,
            Self::Config => config::file::parse(src, path).1,
        }
        .into_iter()
        .filter(Diagnostic::is_error)
        .collect()
    }
    fn legacy(self, d: &Diagnostic, watchdog_bool: bool) -> bool {
        let key = d.key.as_deref().unwrap_or("");
        if self == Self::Settings && key == "watchdog" {
            return d.code == "config_type" && watchdog_bool;
        }
        if d.code != "config_unknown_key" {
            return false;
        }
        match self {
            Self::Settings => SETTINGS.iter().any(|(old, _)| *old == key),
            Self::Config => {
                CONFIG.iter().any(|(old, _)| *old == key)
                    || MANUAL.contains(&key)
                    || format_key(key).is_some_and(|(_, k)| {
                        k == "extra" || FORMAT.iter().any(|(old, _)| *old == k)
                    })
            }
        }
    }
    /// 保留原诊断，并把提示一并交给体检或启动结果的接收端，避免它稍后从磁盘猜测旧现场。
    pub(crate) fn append_hint(self, path: &Path, diagnostics: &mut Vec<Diagnostic>) {
        if diagnostics.iter().any(Diagnostic::is_error) && self.legacy_file(path) {
            diagnostics.push(Diagnostic {
                code: "config_legacy_shape",
                severity: aproxy_ipc::diag::Level::Info,
                file: path.display().to_string(),
                key: None,
                line: None,
                column: None,
                message: aproxy_ipc::i18n::diagnostic::Reason::LegacyConfig,
                details: Default::default(),
            });
        }
    }
    /// 只在严格解析的错误全部来自旧形状时提示；不因一个碰巧同名的坏键引导用户写盘。
    pub fn legacy_file(self, path: &Path) -> bool {
        std::fs::read_to_string(path).is_ok_and(|src| {
            let errors = self.errors(&src, path);
            let boolean = self == Self::Settings
                && serde_json::from_str::<Value>(&src)
                    .ok()
                    .is_some_and(|v| v["watchdog"].is_boolean());
            !errors.is_empty() && errors.iter().all(|d| self.legacy(d, boolean))
        })
    }
}
fn format_key(path: &str) -> Option<(usize, &str)> {
    let (index, key) = path.strip_prefix("format[")?.split_once("].")?;
    Some((index.parse().ok()?, key))
}

/// 文件级结果；只有候选全文通过校验才发布其中的改动。
#[derive(Serialize)]
pub struct ResultFile {
    /// 被检查的实际路径，相同链接目标只出现一次。
    #[serde(serialize_with = "display_path")]
    pub path: std::path::PathBuf,
    /// 稳定的机器结果词，终端显示时再翻译。
    pub outcome: &'static str,
    /// 每个目标键及其脱敏后的值。
    pub changes: Vec<Change>,
    /// 文件未改写或已回滚的原因，不含文件原文。
    pub issues: Vec<Issue>,
}
// 文件操作保留 OsStr 的原始字节，展示与其他 CLI 路径一样使用可读的 Unicode 替代。
fn display_path<S: serde::Serializer>(path: &Path, s: S) -> Result<S::Ok, S::Error> {
    s.collect_str(&path.display())
}
/// 一个机械转换；一项旧值可产生多行（例如两个超时）。
#[derive(Serialize)]
pub struct Change {
    /// 用户要替换的旧键路径。
    pub old: Option<String>,
    /// 替换后的键路径。
    pub new: String,
    /// 用读出口的规则脱敏的目标值。
    pub value: Value,
}
/// 位置指向原文而不是重新排版后的候选文件。
#[derive(Serialize)]
pub struct Issue {
    /// 出问题的原始键，整份文件的故障没有键。
    pub key: Option<String>,
    /// 原始文件的行号，从 1 起。
    pub line: Option<u32>,
    /// 按输出读者的语言生成，不包含用户的值。
    pub message: String,
    /// 说明手工取舍的兼容性文档小节。
    pub section: &'static str,
}

struct Plan {
    next: Option<String>,
    changes: Vec<Change>,
    issues: Vec<Issue>,
}
impl Plan {
    fn issue(&mut self, kind: Kind, key: &str, line: Option<u32>, message: impl Into<String>) {
        self.issues.push(Issue {
            key: (!key.is_empty()).then(|| key.to_owned()),
            line,
            message: message.into(),
            section: kind.section(),
        });
    }
}

// serde_json 的值不保留位置。在已经通过语法检查的输入里沿结构扫描；用反序列化器读字符串，
// 所以转义的键、密钥值里形似键的文字、同名的嵌套键都不会把行号指到别处。
fn json_lines(src: &str) -> std::collections::HashMap<String, u32> {
    use std::collections::HashMap;
    fn skip(s: &str, i: &mut usize) {
        while s.as_bytes().get(*i).is_some_and(u8::is_ascii_whitespace) {
            *i += 1
        }
    }
    fn walk(
        src: &str,
        i: &mut usize,
        path: &str,
        lines: &mut HashMap<String, u32>,
        breaks: &[usize],
    ) {
        skip(src, i);
        match src.as_bytes()[*i] {
            b'{' | b'[' => {
                let obj = src.as_bytes()[*i] == b'{';
                *i += 1;
                skip(src, i);
                let mut index = 0;
                while !matches!(src.as_bytes()[*i], b'}' | b']') {
                    let offset = *i;
                    let child = if obj {
                        let mut token =
                            serde_json::Deserializer::from_str(&src[*i..]).into_iter::<String>();
                        let key = token.next().unwrap().unwrap();
                        *i += token.byte_offset();
                        skip(src, i);
                        *i += 1;
                        if path.is_empty() {
                            config::quote_key(&key)
                        } else {
                            format!("{path}.{}", config::quote_key(&key))
                        }
                    } else {
                        format!("{path}[{index}]")
                    };
                    lines.insert(
                        child.clone(),
                        1 + breaks.partition_point(|p| *p < offset) as u32,
                    );
                    walk(src, i, &child, lines, breaks);
                    skip(src, i);
                    index += 1;
                    if src.as_bytes()[*i] == b',' {
                        *i += 1;
                        skip(src, i)
                    }
                }
                *i += 1;
            }
            _ => {
                let mut token = serde_json::Deserializer::from_str(&src[*i..])
                    .into_iter::<serde::de::IgnoredAny>();
                token.next().unwrap().unwrap();
                *i += token.byte_offset();
            }
        }
    }
    let mut lines = HashMap::new();
    let breaks = src.match_indices('\n').map(|(i, _)| i).collect::<Vec<_>>();
    walk(src, &mut 0, "", &mut lines, &breaks);
    lines
}
fn converted(old: &str, value: &Value) -> Option<Value> {
    match old {
        "forward_only" => Some(json!(if value.as_bool()? { "forward" } else { "retry" })),
        "bounded_retry_paths" => value
            .as_array()?
            .iter()
            .map(|p| Some(json!({"path":p.as_str()?,"max_attempts":3})))
            .collect::<Option<Vec<_>>>()
            .map(Value::Array),
        _ => Some(value.clone()),
    }
}

fn plan(kind: Kind, src: &str, path: &Path, lang: Lang) -> Plan {
    let mut p = Plan {
        next: None,
        changes: vec![],
        issues: vec![],
    };
    let errors = kind.errors(src, path);
    if errors.is_empty() {
        return p;
    }
    let root = match kind {
        Kind::Settings => serde_json::from_str(src).ok(),
        Kind::Config => config::view::file_value(src),
    };
    let lines = if kind == Kind::Settings && root.is_some() {
        json_lines(src)
    } else {
        Default::default()
    };
    let line = |key: &str| match kind {
        Kind::Settings => lines.get(key).copied(),
        Kind::Config => config::file::locate(src, key).map(|p| p.0),
    };
    let boolean = root.as_ref().is_some_and(|v| v["watchdog"].is_boolean());
    for d in &errors {
        let k = d.key.as_deref().unwrap_or("");
        if kind.legacy(d, boolean) {
            if MANUAL.contains(&k) || format_key(k).is_some_and(|(_, f)| f == "extra") {
                p.issue(kind, k, d.line.or_else(|| line(k)), text::manual(lang));
            }
        } else if kind == Kind::Settings
            && k == "defaults.base_url"
            && d.code == "config_missing_key"
        {
            let defaults = &root.as_ref().unwrap()["defaults"];
            if defaults.get("api_key").is_some() {
                p.issue(
                    kind,
                    "defaults.api_key",
                    line("defaults.api_key"),
                    text::credential(lang),
                );
            }
            for header in ["extra_headers", "override_headers"] {
                if let Some(h) = defaults[header].as_object() {
                    for name in h.keys().filter(|n| aproxy_ipc::redact::is_secret_header(n)) {
                        let k = format!("defaults.{header}.{}", config::quote_key(name));
                        p.issue(kind, &k, line(&k), text::credential(lang));
                    }
                }
            }
        } else {
            let msg = if kind == Kind::Settings
                && k == "api_key"
                && root
                    .as_ref()
                    .is_some_and(|v| v["defaults"]["base_url"].is_null())
            {
                text::credential(lang)
            } else if d.code == "config_unknown_key" {
                text::unknown(lang)
            } else {
                text::invalid(lang)
            };
            p.issue(kind, k, d.line.or_else(|| line(k)), msg);
        }
    }
    let Some(root) = root.filter(Value::is_object) else {
        return p;
    };
    let mut edits = Vec::new();
    let mut doc = if kind == Kind::Config {
        Some(src.parse::<DocumentMut>().expect("parsed TOML"))
    } else {
        None
    };
    let mut moves = Vec::new();
    for &(old, new) in if kind == Kind::Settings {
        SETTINGS
    } else {
        CONFIG
    } {
        if let Some(v) = root.get(old)
            && (old != "watchdog" || boolean)
        {
            moves.push((old, new, None, v));
        }
    }
    if kind == Kind::Config
        && let Some(formats) = root["format"].as_array()
    {
        for (i, f) in formats.iter().enumerate() {
            for &(old, new) in FORMAT {
                if let Some(v) = f.get(old) {
                    moves.push((old, new, Some(i), v))
                }
            }
        }
    }
    for (old, new, index, v) in moves {
        let prefix = index.map(|i| format!("format[{i}].")).unwrap_or_default();
        let from = format!("{prefix}{old}");
        let to = format!("{prefix}{new}");
        let local = index.map_or(&root, |i| &root["format"][i]);
        let existing = new.split('.').try_fold(local, |v, k| v.get(k));
        let result = if old == "bounded_retry_paths"
            && doc.as_ref().is_some_and(|d| {
                !d[old]
                    .as_array()
                    .is_some_and(|a| a.iter().all(|v| v.is_str()))
            }) {
            None
        } else if old == "every_attempt" {
            v.as_bool().and_then(|yes| {
                let mut stages = existing?.as_array()?.clone();
                if !stages.iter().all(Value::is_string) {
                    return None;
                }
                if (!yes && stages.contains(&json!("retry")))
                    || (yes && !stages.contains(&json!("request")))
                {
                    return None;
                }
                if yes && !stages.contains(&json!("retry")) {
                    let pos = stages.iter().position(|s| s == "request")?;
                    stages.insert(pos + 1, json!("retry"));
                }
                Some(Value::Array(stages))
            })
        } else {
            converted(old, v)
        };
        let Some(value) = result else {
            p.issue(kind, &from, line(&from), text::invalid(lang));
            continue;
        };
        if existing.is_some() && old != "every_attempt" {
            p.issue(kind, &from, line(&from), text::collision(lang, &to));
            continue;
        }
        if let Some(d) = doc.as_mut() {
            let t = if let Some(i) = index {
                d["format"][i].as_table_like_mut().unwrap()
            } else {
                d.as_table_mut() as &mut dyn TableLike
            };
            if !toml_move(t, old, new, &value) {
                p.issue(kind, &from, line(&from), text::collision(lang, &to));
                continue;
            }
        } else {
            edits.push(settings::edit::Change::unset(old));
            edits.push(settings::edit::Change::set(new, value.clone()));
        }
        p.changes.push(Change {
            old: Some(from),
            new: to,
            value,
        });
    }
    if let Some(d) = doc.as_mut() {
        d.remove("read_timeout_secs");
    }
    let candidate = if let Some(d) = doc {
        Some(d.to_string())
    } else if edits.is_empty() {
        None
    } else {
        match settings::edit::apply_to_text(Some(src), path, &edits) {
            Ok(s) => Some(s),
            Err(config::edit::EditError::Invalid(ds)) => {
                check_candidate(&mut p, kind, &ds, &line, lang);
                None
            }
            Err(_) => None,
        }
    };
    if let Some(s) = candidate {
        let ds = kind.errors(&s, path);
        check_candidate(&mut p, kind, &ds, &line, lang);
        if p.issues.is_empty() && !p.changes.is_empty() {
            for c in &mut p.changes {
                if c.new == "update.download_proxy"
                    && let Some(url) = c.value.as_str()
                {
                    c.value = json!(aproxy_ipc::redact::url(url));
                }
                if c.new == "update.download_chain"
                    && let Some(chain) = c.value.as_array_mut()
                {
                    for step in chain {
                        if let Some(url) = step["url"].as_str() {
                            step["url"] = json!(aproxy_ipc::redact::url(url));
                        }
                    }
                }
            }
            if kind == Kind::Settings && root.get("schema").is_none() {
                p.changes.push(Change {
                    old: None,
                    new: "schema".into(),
                    value: json!(settings::file::SCHEMA),
                });
            }
            p.next = Some(s);
        }
    }
    if p.next.is_none() {
        p.changes.clear();
    }
    p
}
// 校验读的是候选的新键，报告必须指回用户仍能在原文找到的旧键。
fn source_key<'a>(changes: &'a [Change], dest: &'a str) -> &'a str {
    changes
        .iter()
        .find(|c| {
            dest == c.new
                || dest
                    .strip_prefix(&c.new)
                    .is_some_and(|s| s.starts_with(['[', '.']))
        })
        .and_then(|c| c.old.as_deref())
        .unwrap_or(dest)
}
fn check_candidate(
    p: &mut Plan,
    kind: Kind,
    ds: &[Diagnostic],
    line: &impl Fn(&str) -> Option<u32>,
    lang: Lang,
) {
    for d in ds {
        let dest = d.key.as_deref().unwrap_or("");
        if dest == "defaults.base_url" && d.code == "config_missing_key" && kind == Kind::Settings {
            continue;
        }
        let key = source_key(&p.changes, dest).to_owned();
        if !p
            .issues
            .iter()
            .any(|issue| issue.key.as_deref().unwrap_or("") == key)
        {
            p.issue(kind, &key, line(&key), text::invalid(lang));
        }
    }
}

// 普通值直接搬 Item，保留引号、数值写法以及注释；形状改变时只重建目标值。
fn toml_move(table: &mut dyn TableLike, old: &str, new: &str, value: &Value) -> bool {
    let Some(source) = table.get(old) else {
        return false;
    };
    // 同一张表内只换键名，不把该键排到文件末尾。连 Key 的装饰一起保留后面的条目。
    let tail: Vec<_> = if !new.contains('.') && old != "every_attempt" {
        table
            .iter()
            .skip_while(|(k, _)| *k != old)
            .skip(1)
            .map(|(k, _)| table.key(k).unwrap().clone())
            .collect()
    } else {
        vec![]
    };
    let mut item = source.clone();
    let decor = source.as_value().map(|v| v.decor().clone());
    let key_decor = table.key(old).map(|k| k.leaf_decor().clone());
    if old == "bounded_retry_paths" {
        let mut rules = ArrayOfTables::new();
        let patterns = source.as_array().expect("validated string array");
        for (index, pattern) in patterns.iter().enumerate() {
            let mut t = Table::new();
            let mut literal = pattern.clone();
            literal.decor_mut().clear();
            t["path"] = Item::Value(literal);
            t["max_attempts"] = toml_edit::value(3);
            // 数组元素内部的注释跟随对应的规则，不把逗号或布局空白当成注释写入标题。
            let mut notes = String::new();
            for decor in [
                pattern.decor().prefix(),
                pattern.decor().suffix(),
                (index + 1 == patterns.len()).then_some(patterns.trailing()),
            ]
            .into_iter()
            .flatten()
            {
                for line in decor
                    .as_str()
                    .unwrap_or("")
                    .lines()
                    .filter(|l| l.trim_start().starts_with('#'))
                {
                    notes.push_str(line);
                    notes.push('\n');
                }
            }
            if !notes.is_empty() {
                t.key_mut("path")
                    .unwrap()
                    .leaf_decor_mut()
                    .set_prefix(notes);
            }
            rules.push(t);
        }
        item = if rules.is_empty() {
            toml_edit::value(toml_edit::Array::new())
        } else {
            Item::ArrayOfTables(rules)
        };
    } else if old == "forward_only" {
        item = toml_edit::value(value.as_str().unwrap());
    } else if old == "every_attempt" {
        let Some(stages) = table.get_mut(new).and_then(Item::as_array_mut) else {
            return false;
        };
        if value.as_array().unwrap().len() > stages.len() {
            let at = stages
                .iter()
                .position(|v| v.as_str() == Some("request"))
                .unwrap();
            stages.insert(at + 1, "retry");
        }
        let prefix = key_decor
            .as_ref()
            .and_then(|d| d.prefix())
            .and_then(|s| s.as_str())
            .unwrap_or("");
        let suffix = decor
            .as_ref()
            .and_then(|d| d.suffix())
            .and_then(|s| s.as_str())
            .unwrap_or("");
        if prefix.contains('#') || suffix.contains('#') {
            let mut k = table.key_mut(new).unwrap();
            let d = k.leaf_decor_mut();
            let existing = d.prefix().and_then(|s| s.as_str()).unwrap_or("");
            d.set_prefix(format!("{prefix}{}\n{existing}", suffix.trim()));
        }
        table.remove(old);
        return true;
    }
    // 一个读取超时分成两项；注释只跟第一项走，避免把原文复制两遍。
    let secondary = old == "read_timeout_secs" && new == "timeouts.read_idle_secs";
    if secondary {
        if let Some(v) = item.as_value_mut() {
            v.decor_mut().clear();
        }
    } else if let (Some(v), Some(d)) = (item.as_value_mut(), decor.as_ref()) {
        *v.decor_mut() = d.clone();
    }
    if old != "read_timeout_secs" {
        table.remove(old);
    }
    let (target, key) = if let Some((parent, key)) = new.split_once('.') {
        let parent = table.entry(parent).or_insert(Item::Table(Table::new()));
        let Some(t) = parent.as_table_like_mut() else {
            return false;
        };
        (t, key)
    } else {
        (&mut *table, new)
    };
    if target.contains_key(key) {
        return false;
    }
    // 行内父表（例如 retry = {...}）不能容纳表数组的标题，沿用行内数组表示同一组规则。
    if item.is_array_of_tables()
        && let Some(first) = item.as_array_of_tables_mut().and_then(|a| a.get_mut(0))
    {
        if let Some(d) = key_decor.as_ref() {
            first
                .decor_mut()
                .set_prefix(d.prefix().cloned().unwrap_or_default());
        }
        if let Some(d) = decor.as_ref() {
            first
                .decor_mut()
                .set_suffix(d.suffix().cloned().unwrap_or_default());
        }
    }
    target.insert(key, item);
    if let Some(mut k) = target.key_mut(key)
        && !secondary
        && let Some(d) = key_decor
    {
        *k.leaf_decor_mut() = d;
    }
    for key in tail {
        let item = target.remove(key.get()).unwrap();
        target.entry_format(&key).or_insert(item);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    fn mapped(kind: Kind, src: &str) -> Value {
        let p = plan(kind, src, Path::new("sample"), Lang::En);
        assert!(p.issues.is_empty(), "{src}: {}", json!(p.issues));
        let s = p.next.unwrap();
        match kind {
            Kind::Settings => serde_json::from_str(&s).unwrap(),
            Kind::Config => config::view::file_value(&s).unwrap(),
        }
    }
    #[test]
    fn settings_table_rows() {
        for (old, new, from, to) in [
            (
                "max_body_mb",
                "/defaults/max_request_mb",
                json!(15),
                json!(15),
            ),
            (
                "disk_cache",
                "/defaults/disk_cache",
                json!(true),
                json!(true),
            ),
            (
                "forward_only",
                "/defaults/delivery",
                json!(false),
                json!("retry"),
            ),
            (
                "bounded_retry_paths",
                "/defaults/retry/rules",
                json!(["a", "b"]),
                json!([{"path":"a","max_attempts":3},{"path":"b","max_attempts":3}]),
            ),
            (
                "allowed_hosts",
                "/defaults/allowed_hosts",
                json!(["localhost"]),
                json!(["localhost"]),
            ),
            (
                "allowed_origins",
                "/defaults/allowed_origins",
                json!(["https://a.test"]),
                json!(["https://a.test"]),
            ),
            (
                "keepalive_trigger",
                "/defaults/keepalive/trigger",
                json!("accept"),
                json!("accept"),
            ),
            ("watchdog", "/watchdog/enabled", json!(false), json!(false)),
            (
                "watchdog_heartbeat_secs",
                "/watchdog/heartbeat_secs",
                json!(10),
                json!(10),
            ),
            (
                "watchdog_stale_after_cycles",
                "/watchdog/stale_after_cycles",
                json!(2),
                json!(2),
            ),
            (
                "watchdog_max_restarts",
                "/watchdog/max_restarts",
                json!(0),
                json!(0),
            ),
            (
                "watchdog_idle_exit_secs",
                "/watchdog/idle_exit_secs",
                json!(200),
                json!(200),
            ),
            ("log_rotate_mb", "/logs/rotate_mb", json!(20), json!(20)),
            (
                "idle_timeout_secs",
                "/instances/idle_threshold_secs",
                json!(123),
                json!(123),
            ),
            (
                "download_chain",
                "/update/download_chain",
                json!(["npm"]),
                json!(["npm"]),
            ),
            (
                "download_proxy",
                "/update/download_proxy",
                json!("http://proxy.test"),
                json!("http://proxy.test"),
            ),
            (
                "skill_auto_update",
                "/update/skill_auto_update",
                json!(false),
                json!(false),
            ),
        ] {
            let result = mapped(
                Kind::Settings,
                &json!({old:from,"ui":{"other":1}}).to_string(),
            );
            assert_eq!(result.pointer(new), Some(&to), "{old}");
            assert_eq!(result["ui"]["other"], 1);
            assert_eq!(result["schema"], 1);
        }
    }
    #[test]
    fn config_table_rows() {
        for (src, new, to) in [
            ("max_body_mb=7", "/max_request_mb", json!(7)),
            ("spool_limit_mb=8", "/max_response_mb", json!(8)),
            ("forward_only=true", "/delivery", json!("forward")),
            ("connect_timeout_secs=3", "/timeouts/connect_secs", json!(3)),
            ("read_timeout_secs=4", "/timeouts/first_byte_secs", json!(4)),
            ("read_timeout_secs=4", "/timeouts/read_idle_secs", json!(4)),
            (
                "keepalive_interval_secs=2",
                "/keepalive/interval_secs",
                json!(2),
            ),
            (
                "keepalive_trigger='any'",
                "/keepalive/trigger",
                json!("any"),
            ),
            (
                "keepalive_heartbeat=': ping'",
                "/keepalive/heartbeat",
                json!(": ping"),
            ),
            (
                "max_retry_backoff_secs=10",
                "/retry/max_backoff_secs",
                json!(10),
            ),
            (
                "bounded_retry_paths=['a']",
                "/retry/rules",
                json!([{"path":"a","max_attempts":3}]),
            ),
            (
                "[[format]]\ncommand='tool'\nstages=['request']\nmode='spawn'",
                "/format/0/process",
                json!("spawn"),
            ),
            (
                "[[format]]\ncommand='tool'\nstages=['request']\nidle_timeout_secs=5",
                "/format/0/worker_idle_secs",
                json!(5),
            ),
            (
                "format=[{command='tool',stages=['request'],every_attempt=true}]",
                "/format/0/stages",
                json!(["request", "retry"]),
            ),
        ] {
            assert_eq!(mapped(Kind::Config, src).pointer(new), Some(&to), "{src}");
        }
    }
    #[test]
    fn formatted_values_keep_their_place_and_comments() {
        let src = "base_url='http://localhost'\n# body cap\nmax_body_mb = 0x10 # cap\ndisk_cache=true\nread_timeout_secs=9 # both waits\nbounded_retry_paths=[\n # first pattern\n '/a',\n # second pattern\n '/b',\n]\n";
        let p = plan(Kind::Config, src, Path::new("sample"), Lang::En);
        let next = p.next.unwrap();
        assert!(
            next.contains("# body cap\nmax_request_mb = 0x10 # cap\ndisk_cache=true"),
            "{next}"
        );
        for note in ["# both waits", "# first pattern", "# second pattern"] {
            assert_eq!(next.matches(note).count(), 1, "{next}");
        }
        assert_eq!(next.matches("[[retry.rules]]").count(), 2, "{next}");
    }
    #[test]
    fn review_points_are_located_in_original_file() {
        for (kind, src, keys) in [
            (
                Kind::Config,
                "max_body_mb=1\n[request_transform]\n[response_transform]\n[heartbeat_transform]",
                vec![
                    ("request_transform", 2),
                    ("response_transform", 3),
                    ("heartbeat_transform", 4),
                ],
            ),
            (
                Kind::Config,
                "[[format]]\ncommand='tool'\nstages=['request']\nextra='private'",
                vec![("format[0].extra", 4)],
            ),
            (
                Kind::Config,
                "max_body_mb='large'\nkeepalive_trigger='always'",
                vec![("max_body_mb", 1), ("keepalive_trigger", 2)],
            ),
            (
                Kind::Config,
                "delivery='retry'\nforward_only=true",
                vec![("forward_only", 2)],
            ),
            (
                Kind::Config,
                "max_body_mb=2\nunknown_key=true",
                vec![("unknown_key", 2)],
            ),
            (
                Kind::Settings,
                "{\n \"ui\":{\"api_key\":3},\n \"defaults\":{\n \"api_key\":\"secret\",\n \"extra_headers\":{\"Authorization\":\"secret\"}},\n \"forward_only\":false}",
                vec![
                    ("defaults.api_key", 4),
                    ("defaults.extra_headers.Authorization", 5),
                ],
            ),
            (
                Kind::Settings,
                "{\n\"api_key\":\"secret\"}",
                vec![("api_key", 2)],
            ),
            (
                Kind::Settings,
                "{\n\"watchdog\":\"false\",\n\"max_body_mb\":\"2\"}",
                vec![("watchdog", 2), ("max_body_mb", 3)],
            ),
            (
                Kind::Settings,
                "{\n\"defaults\":{\"max_request_mb\":64},\n\"max_body_mb\":32}",
                vec![("max_body_mb", 3)],
            ),
        ] {
            let p = plan(kind, src, Path::new("sample"), Lang::En);
            assert!(p.next.is_none() && p.changes.is_empty(), "{src}");
            for (k, n) in keys {
                assert!(
                    p.issues.iter().any(|i| i.key.as_deref() == Some(k)
                        && i.line == Some(n)
                        && i.section == kind.section()),
                    "{src}: {}",
                    json!(p.issues)
                );
            }
            assert!(!json!(p.issues).to_string().contains("secret"));
        }
    }
}
