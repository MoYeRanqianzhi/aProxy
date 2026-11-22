//! 人写的窗口设置与机器书签分开：读不懂的文档绝不重置，只报告位置，不把原文送上页面。
mod settings;
use super::state::{Store, safe};
use aproxy_ipc::desktop::*;
use std::{
    collections::BTreeSet,
    fs::File,
    io::{ErrorKind, Read},
};
use toml_edit::{DocumentMut, Item, Value};

const LIMIT: usize = 1024 * 1024;
const TEMPLATE: &str = r#"lang = "system"        # "system", or a language the window offers: "en", "zh-CN"
mode = "system"        # system | light | dark
palette = "standard"   # soft | standard | vivid
mica = false
mascot = "tumbler"     # a built-in mascot, or one an enabled extension provides
theme = "builtin"      # builtin, or the id of an installed theme package

[extensions]
enabled = []           # installed extensions to load, in this order

# [extensions.settings."<id>"] holds each extension's own settings.
"#;

/// 缺键与尚未创建文档时采用同一套值。
pub(super) fn defaults() -> Preferences {
    Preferences {
        lang: "system".into(),
        mode: Mode::System,
        palette: Palette::Standard,
        mica: false,
        mascot: "tumbler".into(),
        theme: "builtin".into(),
    }
}
/// 语言标签是包显示文本与窗口设置共用的小词表，不接受任意 BCP-47 扩展。
pub(super) fn language(tag: &str) -> bool {
    let mut parts = tag.split('-');
    let first = parts.next().unwrap_or_default();
    (2..=3).contains(&first.len())
        && first.bytes().all(|c| c.is_ascii_alphabetic())
        && parts.clone().count() <= 3
        && parts.all(|p| (1..=8).contains(&p.len()) && p.bytes().all(|c| c.is_ascii_alphanumeric()))
}
/// Windows 按第一个点以前的部分识别设备，即使名字后面还有扩展名。
pub(super) fn device(name: &str) -> bool {
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(stem.as_str(), "con" | "prn" | "aux" | "nul")
        || matches!(
            stem.as_bytes(),
            [b'c', b'o', b'm', b'1'..=b'9'] | [b'l', b'p', b't', b'1'..=b'9']
        )
}
/// 安装目录名也写进 TOML；两处必须按同一套规则识别包。
pub(super) fn package_id(id: &str) -> bool {
    (3..=100).contains(&id.len())
        && id.as_bytes()[0].is_ascii_alphanumeric()
        && id.as_bytes()[id.len() - 1].is_ascii_alphanumeric()
        && !id.contains("..")
        && !device(id)
        && slug(id)
}
fn slug(id: &str) -> bool {
    (1..=100).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'-'))
}
fn issue(kind: TomlIssueKind) -> TomlIssue {
    TomlIssue {
        kind,
        line: None,
        column: None,
        key: None,
    }
}
fn syntax(before: &str) -> TomlIssue {
    TomlIssue {
        line: Some(before.bytes().filter(|&b| b == b'\n').count() + 1),
        column: Some(before.rsplit('\n').next().unwrap_or("").chars().count() + 1),
        ..issue(TomlIssueKind::Syntax)
    }
}

/// 锁内读出的可编辑文档，保存时沿用原来的行尾习惯。
pub(super) struct DesktopToml {
    doc: DocumentMut,
    original: Option<String>,
}
impl DesktopToml {
    /// 同时限制读入大小与解析器输入；损坏的文件不触发写盘。
    pub(super) fn read(store: &Store) -> Result<Self, TomlIssue> {
        let path = store.dir.join("desktop.toml");
        let io = || issue(TomlIssueKind::Io);
        safe(&path, StateFile::Preferences, false).map_err(|_| io())?;
        let mut bytes = Vec::new();
        let original = match File::open(path) {
            Ok(f) => {
                f.take((LIMIT + 1) as u64)
                    .read_to_end(&mut bytes)
                    .map_err(|_| io())?;
                if bytes.len() > LIMIT {
                    return Err(issue(TomlIssueKind::TooLarge));
                }
                Some(
                    std::str::from_utf8(&bytes)
                        .map_err(|e| {
                            syntax(std::str::from_utf8(&bytes[..e.valid_up_to()]).unwrap())
                        })?
                        .to_owned(),
                )
            }
            Err(e) if e.kind() == ErrorKind::NotFound => None,
            Err(_) => return Err(io()),
        };
        let text = original.as_deref().unwrap_or(TEMPLATE);
        let doc = text
            .parse::<DocumentMut>()
            .map_err(|e| syntax(text.get(..e.span().map_or(0, |s| s.start)).unwrap_or(text)))?;
        Ok(Self { doc, original })
    }
    /// 所有写者受同一份文件的上限约束，包括扩展设置。
    pub(super) fn save(&self, store: &Store) -> R<()> {
        let mut text = self.doc.to_string();
        let original = self.original.as_deref().unwrap_or(TEMPLATE);
        if original.contains("\r\n") {
            // toml_edit 统一输出 LF。对混用行尾的手写文档，按未改行的内容顺序找回原来的分隔符。
            let lines: Vec<_> = original.split_terminator('\n').collect();
            let mut positions = std::collections::HashMap::<&str, Vec<usize>>::new();
            for (n, line) in lines.iter().enumerate() {
                positions
                    .entry(line.trim_end_matches('\r'))
                    .or_default()
                    .push(n);
            }
            let mut rendered = String::with_capacity(text.len());
            let mut next = 0;
            for line in text.split_terminator('\n') {
                let line = line.trim_end_matches('\r');
                let found = positions
                    .get(line)
                    .and_then(|v| v.get(v.partition_point(|&i| i < next)))
                    .copied();
                let old = found.unwrap_or(next).min(lines.len() - 1);
                rendered.push_str(line);
                if lines[old].ends_with('\r') {
                    rendered.push('\r');
                }
                rendered.push('\n');
                if let Some(i) = found {
                    next = i + 1;
                }
            }
            text = rendered;
        }
        if !original.is_empty() && !original.ends_with('\n') && text.ends_with('\n') {
            text.pop();
            if text.ends_with('\r') {
                text.pop();
            }
        }
        if text.len() > LIMIT {
            return Err(Fault::DesktopToml(issue(TomlIssueKind::TooLarge)));
        }
        crate::fsx::write_atomic_private(&store.dir.join("desktop.toml"), text.as_bytes())
            .map_err(|_| Fault::DesktopToml(issue(TomlIssueKind::Io)))
    }
    /// 来源记录提交失败时恢复精确原文；原来没有文档则不留下模板。
    pub(super) fn restore(&self, store: &Store) -> std::io::Result<()> {
        let path = store.dir.join("desktop.toml");
        match &self.original {
            Some(text) => crate::fsx::write_atomic_private(&path, text.as_bytes()),
            None => std::fs::remove_file(path),
        }
    }
    /// 缺键正常，只有已出现的坏值才提示；未知字段保留在语法树里。
    pub(super) fn preferences(&self) -> (Preferences, Vec<String>, Vec<TomlIssue>) {
        let mut p = defaults();
        let mut enabled = Vec::new();
        let mut issues = Vec::new();
        let mut bad = |key: &str| {
            issues.push(TomlIssue {
                key: Some(key.into()),
                ..issue(TomlIssueKind::Value)
            })
        };
        for (key, v) in self.doc.iter() {
            match key {
                "lang" => match v.as_str() {
                    Some(s) if s == "system" || language(s) => p.lang = s.into(),
                    _ => bad(key),
                },
                "mascot" => match v.as_str() {
                    Some(s) if slug(s) => p.mascot = s.into(),
                    _ => bad(key),
                },
                "theme" => match v.as_str() {
                    Some(s) if package_id(s) => p.theme = s.into(),
                    _ => bad(key),
                },
                "mode" => match v.as_str() {
                    Some("system") => (),
                    Some("light") => p.mode = Mode::Light,
                    Some("dark") => p.mode = Mode::Dark,
                    _ => bad(key),
                },
                "palette" => match v.as_str() {
                    Some("standard") => (),
                    Some("vivid") => p.palette = Palette::Vivid,
                    Some("soft") => p.palette = Palette::Soft,
                    _ => bad(key),
                },
                "mica" => match v.as_bool() {
                    Some(b) => p.mica = b,
                    None => bad(key),
                },
                "extensions" => match v.as_table_like() {
                    None => bad(key),
                    Some(t) => {
                        if let Some(a) = t.get("enabled") {
                            let mut seen = BTreeSet::new();
                            match a.as_array().and_then(|a| {
                                a.iter()
                                    .map(|x| {
                                        x.as_str()
                                            .filter(|id| package_id(id) && seen.insert(*id))
                                            .map(str::to_owned)
                                    })
                                    .collect()
                            }) {
                                Some(ids) => enabled = ids,
                                None => bad("extensions.enabled"),
                            }
                        }
                    }
                },
                _ => (),
            }
        }
        (p, enabled, issues)
    }
    /// 修改同一数组而非重建所有值；保留未动条目的注释，插入顺序就是加载顺序。
    pub(super) fn enable(&mut self, id: &str, enable: bool) -> R<bool> {
        let (_, ids, issues) = self.preferences();
        let invalid = issues
            .iter()
            .any(|i| i.key.as_deref() == Some("extensions.enabled"));
        if !invalid && ids.iter().any(|s| s == id) == enable {
            return Ok(false);
        }
        let item = self
            .doc
            .entry("extensions")
            .or_insert_with(|| Item::Table(toml_edit::Table::new()));
        let table = item.as_table_like_mut().ok_or_else(|| {
            Fault::DesktopToml(TomlIssue {
                key: Some("extensions".into()),
                ..issue(TomlIssueKind::Value)
            })
        })?;
        if invalid || !table.contains_key("enabled") {
            table.insert("enabled", Item::Value(toml_edit::Array::new().into()));
        }
        let a = table
            .get_mut("enabled")
            .and_then(Item::as_array_mut)
            .unwrap();
        if enable {
            a.push(id)
        } else if let Some(i) = ids.iter().position(|s| s == id) {
            a.remove(i);
        }
        Ok(true)
    }
    /// 卸载主题恢复内置外观；按用户选择清理这个扩展的设置，不影响邻居。
    pub(super) fn uninstall(&mut self, id: &str, remove_data: bool) -> R<bool> {
        let mut changed = self.enable(id, false)?;
        if self.doc.get("theme").and_then(Item::as_str) == Some(id) {
            self.set("theme", "builtin".into());
            changed = true;
        }
        if remove_data
            && let Some(table) = self
                .doc
                .get_mut("extensions")
                .and_then(Item::as_table_like_mut)
                .and_then(|t| t.get_mut("settings"))
                .and_then(Item::as_table_like_mut)
        {
            changed |= table.remove(id).is_some();
        }
        Ok(changed)
    }
    /// 只核验提交的键；用户可以逐项修复文档，不被别处的坏值挡住。
    fn set(&mut self, key: &str, mut value: Value) {
        if let Some(old) = self.doc.get(key).and_then(Item::as_value) {
            *value.decor_mut() = old.decor().clone();
        }
        self.doc[key] = Item::Value(value);
    }
    /// 只替换值，留住键两侧的排版与行末注释。
    pub(super) fn patch(&mut self, p: PreferencesPatch) -> R<()> {
        if let Some(s) = p.lang {
            if s != "system" && !language(&s) {
                return Err(Fault::DesktopBadInput {});
            }
            self.set("lang", s.into());
        }
        if let Some(s) = p.mascot {
            if !slug(&s) {
                return Err(Fault::DesktopBadInput {});
            }
            self.set("mascot", s.into());
        }
        if let Some(s) = p.theme {
            if !package_id(&s) {
                return Err(Fault::DesktopBadInput {});
            }
            self.set("theme", s.into());
        }
        if let Some(v) = p.mica {
            self.set("mica", v.into());
        }
        if let Some(v) = p.mode {
            self.set(
                "mode",
                match v {
                    Mode::System => "system",
                    Mode::Light => "light",
                    Mode::Dark => "dark",
                }
                .into(),
            );
        }
        if let Some(v) = p.palette {
            self.set(
                "palette",
                match v {
                    Palette::Soft => "soft",
                    Palette::Standard => "standard",
                    Palette::Vivid => "vivid",
                }
                .into(),
            );
        }
        Ok(())
    }
}
