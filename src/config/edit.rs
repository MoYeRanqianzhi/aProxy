//! config.toml 的写回：只改目标键，注释、键的顺序与其余内容原样保留。
//!
//! 一次写入的步骤：
//! 1. 在本 home 的 `run/config-write.lock` 上持锁（锁原语与安装锁相同）。本 home 的 aproxy
//!    写者由此串行：命令行与界面同时改同一个文件，谁也不会盖掉对方的改动。
//! 2. 读入文件、应用改动、校验结果：这次改动引入了新的 error 级诊断就拒绝写入。文件里原有
//!    的、与这次改动无关的错误不挡路，否则一个坏文件永远没法逐项修好。
//! 3. 落盘之前再读一次文件：与第 2 步读到的不一样，说明有不拿锁的外部写者（编辑器），重新
//!    读入、重新应用，最多 [`MAX_ATTEMPTS`] 次。
//! 4. 原子地写回（临时文件、`sync_all`、rename；unix 上 0600）。目标是符号链接时写到它指向
//!    的真实文件。
//!
//! `set` 不需要一张「已知键」的表：不认识的键、类型不对的值，都会让第 2 步的校验多出一条
//! 诊断，于是被拒绝。`unset` 则接受任何键路径——删除总是安全的，这也是清掉一个不认识的键
//! 的办法。
//!
//! 读出口给的是占位对象而不是密钥（见 [`super::view`]），所以读出来再写回的值里会带着
//! 占位：`set` 把它们换回文件里的原值，对不上就拒绝。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use aproxy_ipc::diag::Diagnostic;
use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, TableLike, Value};

use super::resolve::{Defaults, Overrides, resolve_text};
use super::{KeySegment, parse_key_path};

/// 发现外部修改后重新读入、重新应用的次数上限。
pub const MAX_ATTEMPTS: u32 = 3;

/// 等写锁的时限。持锁的时间只有一次读、一次写，等这么久还拿不到，说明持有者卡住了。
const LOCK_WAIT: Duration = Duration::from_secs(10);

/// 写锁的位置：`<home>/run/config-write.lock`。
pub fn lock_path_in(home: &Path) -> PathBuf {
    home.join("run").join("config-write.lock")
}

/// 对一个键的一次改动。键是点号路径，例如 `retry.max_backoff_secs`、`extra_headers."x-foo"`；
/// 数组表（`format`、`retry.rules`）只能整体读写，路径里不能带下标。
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

/// 写入没有发生的原因。
#[derive(Debug)]
pub enum EditError {
    /// 键路径写法不对，或带了下标
    BadKey(String),
    /// 文件现在有语法错误：没法只改一个键，先修好语法
    Syntax(Vec<Diagnostic>),
    /// 这次改动会引入这些 error 级诊断
    Invalid(Vec<Diagnostic>),
    /// 文件一直在被外部修改，重试之后仍然冲突
    Conflict,
    /// 写锁在时限内没有等到
    LockBusy,
    Io(std::io::Error),
}

impl From<std::io::Error> for EditError {
    fn from(e: std::io::Error) -> Self {
        EditError::Io(e)
    }
}

/// 一次写入的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    /// 实际写的文件（目标是符号链接时，是它指向的那个）
    pub file: PathBuf,
    /// 文件内容有没有变。改动没有改变任何东西（例如删一个不存在的键）时不写文件
    pub changed: bool,
}

/// 把 `changes` 应用到 `path` 处的配置文件。文件不存在时从空文件开始、写入时新建。
/// `home` 定写锁的位置；`defaults` 是校验结果时用的全局默认那一层。
pub fn apply(
    path: &Path,
    home: &Path,
    defaults: &Defaults,
    changes: &[Change],
) -> Result<Applied, EditError> {
    apply_observed(path, home, defaults, changes, || {})
}

/// 同 [`apply`]。`before_recheck` 在「算出新内容」与「落盘前再读一次」之间被调用，
/// 测试用它扮演外部写者。
fn apply_observed(
    path: &Path,
    home: &Path,
    defaults: &Defaults,
    changes: &[Change],
    before_recheck: impl FnMut(),
) -> Result<Applied, EditError> {
    rewrite_locked(path, home, false, before_recheck, |old, file| {
        apply_to_text(old.unwrap_or(""), file, defaults, changes)
    })
}

/// 在写锁之内改写一个文件：读入、交给 `transform` 算出新内容、落盘前再读一次核对没有外部
/// 修改（有就从头再来，最多 [`MAX_ATTEMPTS`] 次）、原子写回。config.toml 与 settings.json
/// 共用这一套，两种文件的写者也就排在同一把锁上。
///
/// `transform` 拿到的是文件现在的内容（不存在时为 None）与实际要写的路径，返回新内容；新
/// 内容与旧内容相同时不写文件。`private` 为真时按含密钥的文件来写（见
/// [`crate::fsx::write_atomic_private`]）。
pub(crate) fn rewrite_locked<E: From<EditError>>(
    path: &Path,
    home: &Path,
    private: bool,
    mut before_recheck: impl FnMut(),
    mut transform: impl FnMut(Option<&str>, &Path) -> Result<String, E>,
) -> Result<Applied, E> {
    let _lock = lock(home)?;
    let file = real_target(path);
    for _ in 0..MAX_ATTEMPTS {
        let before = read_optional(&file).map_err(EditError::Io)?;
        let new = transform(before.as_deref(), &file)?;
        if before.as_deref().unwrap_or("") == new {
            return Ok(Applied {
                file,
                changed: false,
            });
        }
        before_recheck();
        if read_optional(&file).map_err(EditError::Io)? != before {
            continue;
        }
        write_with_retry(&file, new.as_bytes(), private).map_err(EditError::Io)?;
        return Ok(Applied {
            file,
            changed: true,
        });
    }
    Err(EditError::Conflict.into())
}

/// 把 `changes` 应用到文本上并校验结果，返回新的文本。不碰文件系统。
/// `path` 只用来填诊断的 `file`。
pub fn apply_to_text(
    old: &str,
    path: &Path,
    defaults: &Defaults,
    changes: &[Change],
) -> Result<String, EditError> {
    let mut document = match old.parse::<DocumentMut>() {
        Ok(document) => document,
        Err(_) => return Err(EditError::Syntax(super::file::parse(old, path).1)),
    };
    // 占位对象对应的是改动之前文件里的值
    let before = super::view::file_value(old);
    for change in changes {
        match change {
            Change::Set { key, value } => {
                let segments = plain_path(key)?;
                let value = keep_placeholders(value, before.as_ref(), key, path)?;
                set(&mut document, &segments, value);
            }
            Change::Unset { key } => unset(&mut document, &plain_path(key)?),
        }
    }
    let new = document.to_string();

    let errors = |text: &str| -> Vec<Diagnostic> {
        resolve_text(Some(text), path, &Overrides::default(), defaults, false)
            .diagnostics
            .into_iter()
            .filter(Diagnostic::is_error)
            .collect()
    };
    // 行列会随改动移动，不参与「是不是同一个问题」的比较
    let same = |a: &Diagnostic, b: &Diagnostic| {
        a.code == b.code && a.key == b.key && a.message == b.message
    };
    let existing = errors(old);
    let introduced: Vec<Diagnostic> = errors(&new)
        .into_iter()
        .filter(|d| !existing.iter().any(|e| same(e, d)))
        .collect();
    if introduced.is_empty() {
        Ok(new)
    } else {
        Err(EditError::Invalid(introduced))
    }
}

/// 要写进 `key` 的值里有占位对象时，把它们换回 `before`（改动之前的文件）里的原值；没有
/// 占位时原样返回，值的写法（引号、数字的进制）不动。
fn keep_placeholders(
    value: &Value,
    before: Option<&serde_json::Value>,
    key: &str,
    file: &Path,
) -> Result<Value, EditError> {
    use super::view;
    let json = view::toml_to_json(value);
    if !view::has_placeholder(&json) {
        return Ok(value.clone());
    }
    let old = match before {
        Some(before) => view::get(before, key)?,
        None => None,
    };
    let restored = view::restore_placeholders(json, old, key, file)?;
    // 占位对象都换成了字符串，结果里没有 null，总能写成 TOML
    view::json_to_toml(&restored).ok_or_else(|| EditError::BadKey(key.to_string()))
}

/// 解析键路径，拒绝带下标的写法。
fn plain_path(key: &str) -> Result<Vec<KeySegment>, EditError> {
    match parse_key_path(key) {
        Some(segments) if segments.iter().all(|s| s.index.is_none()) => Ok(segments),
        _ => Err(EditError::BadKey(key.to_string())),
    }
}

/// 设置一个键。路径上的表不管写成 `[x]`、点号键还是内联表都能走进去；缺的表新建成 `[x]`。
fn set(document: &mut DocumentMut, path: &[KeySegment], value: Value) {
    let Some((last, parents)) = path.split_last() else {
        return;
    };
    let mut table: &mut dyn TableLike = document.as_table_mut();
    for segment in parents {
        let is_table = table
            .get(&segment.name)
            .is_some_and(|item| item.is_table_like());
        if !is_table {
            table.insert(&segment.name, Item::Table(Table::new()));
        }
        table = table
            .get_mut(&segment.name)
            .and_then(Item::as_table_like_mut)
            .expect("the preceding step established a table");
    }
    let value = one_line(value);
    match table.get_mut(&last.name) {
        // 原来就是一个值：换掉值，留着它前后的空白与行尾注释
        Some(Item::Value(existing)) if !is_table_array(&value) => {
            let decor = existing.decor().clone();
            *existing = value;
            *existing.decor_mut() = decor;
        }
        _ => {
            table.insert(&last.name, into_item(value));
        }
    }
}

/// 带换行的字符串写成一行、换行写成 `\n`：默认的写法是多行字符串，它在开头多出一个换行，
/// 对 `keepalive.heartbeat` 这种「每个字节都算数」的值很难看清到底写了什么。
pub(crate) fn one_line(value: Value) -> Value {
    match value.as_str() {
        // JSON 的字符串字面量也是合法的 TOML 基本字符串（转义写法是它的子集）
        Some(text) if text.contains(['\n', '\r']) => serde_json::to_string(text)
            .ok()
            .and_then(|literal| literal.parse::<Value>().ok())
            .unwrap_or(value),
        _ => value,
    }
}

/// 值是不是「表的数组」：非空，且每个元素都是内联表。
fn is_table_array(value: &Value) -> bool {
    value
        .as_array()
        .is_some_and(|array| !array.is_empty() && array.iter().all(Value::is_inline_table))
}

/// 表的数组写成 `[[x]]` 的形式（每个条目一段，比一行内联表好读好改），其余的值原样。
fn into_item(value: Value) -> Item {
    if !is_table_array(&value) {
        return Item::Value(value);
    }
    let mut tables = ArrayOfTables::new();
    for entry in value.as_array().into_iter().flatten() {
        if let Some(inline) = entry.as_inline_table() {
            tables.push(inline.clone().into_table());
        }
    }
    Item::ArrayOfTables(tables)
}

/// 删除一个键。路径上任何一段不存在，就什么都不做。
fn unset(document: &mut DocumentMut, path: &[KeySegment]) {
    let Some((last, parents)) = path.split_last() else {
        return;
    };
    let mut table: &mut dyn TableLike = document.as_table_mut();
    for segment in parents {
        let Some(next) = table
            .get_mut(&segment.name)
            .and_then(Item::as_table_like_mut)
        else {
            return;
        };
        table = next;
    }
    table.remove(&last.name);
}

/// 拿写锁，最多等 [`LOCK_WAIT`]。
/// 在同一主目录内串行化配置和设置的写入，包括写前备份和写后复查。
pub(crate) fn lock(home: &Path) -> Result<crate::install::InstallLock, EditError> {
    let path = lock_path_in(home);
    let deadline = Instant::now() + LOCK_WAIT;
    loop {
        // 打开失败也重试：Windows 上别的进程以不兼容的共享模式开着锁文件时，打开会暂时失败
        let attempt = crate::install::try_lock_path(&path);
        match attempt {
            Ok(Some(lock)) => return Ok(lock),
            Ok(None) | Err(_) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok(None) => return Err(EditError::LockBusy),
            Err(e) => return Err(EditError::Io(e)),
        }
    }
}

/// 目标是符号链接时，写它指向的真实文件：rename 到链接自己的路径会把链接换成普通文件。
fn real_target(path: &Path) -> PathBuf {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
        }
        _ => path.to_path_buf(),
    }
}

/// 读文件；不存在时是 None。
fn read_optional(path: &Path) -> std::io::Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// 原子写。Windows 上目标被别的进程短暂开着（编辑器、杀毒软件）时 rename 会被拒绝，
/// 退避着重试几次。
fn write_with_retry(path: &Path, bytes: &[u8], private: bool) -> std::io::Result<()> {
    let write = |path: &Path, bytes: &[u8]| {
        if private {
            crate::fsx::write_atomic_private(path, bytes)
        } else {
            crate::fsx::write_atomic(path, bytes)
        }
    };
    let mut delay = Duration::from_millis(20);
    for _ in 0..5 {
        match write(path, bytes) {
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                std::thread::sleep(delay);
                delay *= 2;
            }
            result => return result,
        }
    }
    write(path, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults() -> Defaults {
        Defaults::default()
    }

    fn edit(old: &str, changes: &[Change]) -> Result<String, EditError> {
        apply_to_text(old, Path::new("config.toml"), &defaults(), changes)
    }

    fn array(text: &str) -> Value {
        text.parse().unwrap()
    }

    const COMMENTED: &str = r#"# 上游
base_url = "https://a.example"   # 行尾注释
keepalive = { interval_secs = 15, trigger = "any" }
timeouts.connect_secs = 30

# 头
[extra_headers]
anthropic-version = "2023-06-01"  # 版本

[retry]
# 退避
max_backoff_secs = 320
"#;

    /// 只改目标键：注释、空行与键的顺序都在；表写成内联表、点号键、表头三种形式都改得到。
    #[test]
    fn set_changes_only_the_target_key() {
        let new = edit(
            COMMENTED,
            &[
                Change::set("base_url", "https://b.example"),
                Change::set("keepalive.interval_secs", 5),
                Change::set("timeouts.connect_secs", 10),
                Change::set("extra_headers.anthropic-version", "2024-01-01"),
                Change::set("retry.max_backoff_secs", 60),
            ],
        )
        .unwrap();
        assert_eq!(
            new,
            r#"# 上游
base_url = "https://b.example"   # 行尾注释
keepalive = { interval_secs = 5, trigger = "any" }
timeouts.connect_secs = 10

# 头
[extra_headers]
anthropic-version = "2024-01-01"  # 版本

[retry]
# 退避
max_backoff_secs = 60
"#
        );
    }

    /// 新键：顶层的键进文件开头那一段，已有的表里加一行，没有的表新建。
    #[test]
    fn set_adds_keys_and_tables() {
        let new = edit(
            COMMENTED,
            &[
                Change::set("disk_cache", false),
                Change::set("timeouts.read_idle_secs", 120),
                Change::set("extra_headers.\"x.custom\"", "1"),
                Change::set("history.body_budget_mb", 8),
            ],
        )
        .unwrap();
        // 原有的每一行都还在
        for line in COMMENTED.lines() {
            assert!(new.contains(line), "丢了一行: {line}\n{new}");
        }
        let (config, diagnostics) = crate::config::file::parse(&new, Path::new("config.toml"));
        assert_eq!(diagnostics, []);
        assert_eq!(config.disk_cache, Some(false));
        assert_eq!(config.timeouts.connect_secs, Some(30));
        assert_eq!(config.timeouts.read_idle_secs, Some(120));
        assert_eq!(config.extra_headers.unwrap()["x.custom"], "1");
        assert_eq!(config.history.body_budget_mb, Some(8));
        // 顶层的新键不会掉进文件末尾的表里
        assert!(new.find("disk_cache").unwrap() < new.find("[extra_headers]").unwrap());
        assert!(new.contains("[history]\nbody_budget_mb = 8\n"), "{new}");
    }

    /// 从空文件开始。
    #[test]
    fn set_on_an_empty_file() {
        let new = edit(
            "",
            &[
                Change::set("base_url", "https://a.example"),
                Change::set("keepalive.heartbeat", ": ping\n\n"),
            ],
        )
        .unwrap();
        assert_eq!(
            new,
            "base_url = \"https://a.example\"\n\n[keepalive]\nheartbeat = \": ping\\n\\n\"\n"
        );
    }

    /// 数组表整体替换，写成 `[[x]]` 的形式；空数组清空它。
    #[test]
    fn table_arrays_are_replaced_as_a_whole() {
        let old = "base_url = \"https://a.example\"\n\n[[retry.rules]]\n# 旧规则\npath = \"/old\"\nmax_attempts = 2\n\n[[format]]\ncommand = \"old\"\nstages = [\"request\"]\n";
        let new = edit(
            old,
            &[
                Change::set(
                    "retry.rules",
                    array(
                        "[{ status = [404], max_attempts = 1 }, { path = '/x', max_attempts = 0 }]",
                    ),
                ),
                Change::set("format", array("[]")),
            ],
        )
        .unwrap();
        assert!(!new.contains("/old") && !new.contains("command"), "{new}");
        assert_eq!(new.matches("[[retry.rules]]").count(), 2, "{new}");
        let (config, diagnostics) = crate::config::file::parse(&new, Path::new("config.toml"));
        assert_eq!(diagnostics, []);
        assert_eq!(config.retry.rules.unwrap().len(), 2);
        assert_eq!(config.format, Some(vec![]));
    }

    /// 不认识的键、类型不对的值、语义上不成立的值，都因为多出一条诊断而被拒绝。
    #[test]
    fn set_rejects_changes_that_introduce_errors() {
        let old = "base_url = \"https://a.example\"\n";
        let cases: [(Change, &str, &str); 5] = [
            (
                Change::set("forward_only", true),
                "config_unknown_key",
                "forward_only",
            ),
            (
                Change::set("timeouts.total_secs", 1),
                "config_unknown_key",
                "timeouts.total_secs",
            ),
            (
                Change::set("disk_cache", "yes"),
                "config_type",
                "disk_cache",
            ),
            (
                Change::set("base_url", "ftp://a.example"),
                "config_invalid_value",
                "base_url",
            ),
            (
                Change::set("keepalive.heartbeat", "data: x"),
                "config_invalid_value",
                "keepalive.heartbeat",
            ),
        ];
        for (change, code, key) in cases {
            match edit(old, &[change]) {
                Err(EditError::Invalid(diagnostics)) => {
                    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
                    assert_eq!(
                        (diagnostics[0].code, diagnostics[0].key.as_deref()),
                        (code, Some(key))
                    );
                }
                other => panic!("{key}: {other:?}"),
            }
        }
    }

    /// 文件里原有的错误不挡住与它无关的改动，于是能逐项修好。
    #[test]
    fn existing_errors_do_not_block_unrelated_changes() {
        let old = "forward_only = true\nmax_body_mb = 1\n";
        // base_url 缺失、两个不认识的键：三个原有的错误
        let new = edit(old, &[Change::set("base_url", "https://a.example")]).unwrap();
        let new = edit(&new, &[Change::unset("forward_only")]).unwrap();
        let new = edit(&new, &[Change::unset("max_body_mb")]).unwrap();
        assert_eq!(new, "base_url = \"https://a.example\"\n");
    }

    /// unset 接受任何键：不认识的键删得掉，不存在的键什么都不改。
    #[test]
    fn unset_accepts_any_key() {
        let old = "base_url = \"https://a.example\"\nbogus = 1\n\n[timeouts]\nconnect_secs = 5\nnope = 2\n\n[extra_headers]\n\"x.y\" = \"v\"\n";
        let new = edit(
            old,
            &[
                Change::unset("bogus"),
                Change::unset("timeouts.nope"),
                Change::unset("extra_headers.\"x.y\""),
                Change::unset("never.was.here"),
                Change::unset("base_url.deeper"),
            ],
        )
        .unwrap();
        assert_eq!(
            new,
            "base_url = \"https://a.example\"\n\n[timeouts]\nconnect_secs = 5\n\n[extra_headers]\n"
        );
        assert_eq!(edit(old, &[Change::unset("absent")]).unwrap(), old);
    }

    #[test]
    fn bad_key_paths_are_refused() {
        for key in ["", "a..b", "format[0].command", "a b", "\"open"] {
            assert!(
                matches!(edit("", &[Change::unset(key)]), Err(EditError::BadKey(_))),
                "{key}"
            );
        }
    }

    /// 语法错误的文件不改：没法只动一个键。
    #[test]
    fn a_file_with_syntax_errors_is_left_alone() {
        match edit(
            "base_url = \n",
            &[Change::set("base_url", "https://a.example")],
        ) {
            Err(EditError::Syntax(diagnostics)) => {
                assert_eq!(diagnostics[0].code, "config_syntax")
            }
            other => panic!("{other:?}"),
        }
    }

    /// 落盘：文件不存在时新建，没有变化时不写，临时文件不留下。
    #[test]
    fn apply_writes_the_file() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("nested").join("config.toml");
        let applied = apply(
            &path,
            home.path(),
            &defaults(),
            &[Change::set("base_url", "https://a.example")],
        )
        .unwrap();
        assert_eq!(
            applied,
            Applied {
                file: path.clone(),
                changed: true
            }
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "base_url = \"https://a.example\"\n"
        );
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        let applied = apply(
            &path,
            home.path(),
            &defaults(),
            &[Change::set("base_url", "https://a.example")],
        )
        .unwrap();
        assert!(!applied.changed);
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            modified
        );
        // 删一个不存在的文件里的键：不新建文件
        let other = home.path().join("other.toml");
        let applied = apply(&other, home.path(), &defaults(), &[Change::unset("x")]).unwrap();
        assert!(!applied.changed && !other.exists());
        let names: Vec<String> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["config.toml"]);
        assert!(lock_path_in(home.path()).is_file());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    /// 被拒绝的改动不动文件。
    #[test]
    fn a_rejected_change_leaves_the_file_untouched() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("config.toml");
        std::fs::write(&path, COMMENTED).unwrap();
        let result = apply(
            &path,
            home.path(),
            &defaults(),
            &[
                Change::set("base_url", "https://b.example"),
                Change::set("bogus", 1),
            ],
        );
        assert!(matches!(result, Err(EditError::Invalid(_))), "{result:?}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), COMMENTED);
    }

    /// 多个写者并发地各改各的键：一个更新都不丢。
    #[test]
    fn concurrent_writers_do_not_lose_updates() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("config.toml");
        std::fs::write(&path, "# 保留\nbase_url = \"https://a.example\"\n").unwrap();
        // 规模刻意小：每次写入都落盘（sync_all），写者串行拿锁，全部写入加起来的时间
        // 要远小于写锁的等待时限，否则负载高的机器上排在后面的线程会等到超时
        const WRITERS: usize = 4;
        const ROUNDS: usize = 4;
        std::thread::scope(|scope| {
            for writer in 0..WRITERS {
                let (path, home) = (&path, home.path());
                scope.spawn(move || {
                    for round in 0..ROUNDS {
                        let key = format!("extra_headers.x-w{writer}-r{round}");
                        apply(path, home, &defaults(), &[Change::set(&key, "1")]).unwrap();
                    }
                });
            }
        });
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("# 保留\n"));
        let (config, diagnostics) = crate::config::file::parse(&text, &path);
        assert_eq!(diagnostics, []);
        assert_eq!(config.extra_headers.unwrap().len(), WRITERS * ROUNDS);
    }

    /// 外部写者（不拿锁）在读与写之间改了文件：重新读入、重新应用，两边的改动都在。
    #[test]
    fn an_external_modification_is_merged_by_retrying() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("config.toml");
        std::fs::write(&path, "base_url = \"https://a.example\"\n").unwrap();
        let mut interfered = false;
        let applied = apply_observed(
            &path,
            home.path(),
            &defaults(),
            &[Change::set("disk_cache", false)],
            || {
                if !std::mem::replace(&mut interfered, true) {
                    std::fs::write(
                        &path,
                        "base_url = \"https://a.example\"\nmax_request_mb = 7 # 编辑器写的\n",
                    )
                    .unwrap();
                }
            },
        )
        .unwrap();
        assert!(applied.changed);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "base_url = \"https://a.example\"\nmax_request_mb = 7 # 编辑器写的\ndisk_cache = false\n"
        );
    }

    /// 每次重试都撞上外部修改：放弃，文件是外部写者最后写的样子。
    #[test]
    fn a_persistent_external_writer_is_a_conflict() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("config.toml");
        std::fs::write(&path, "base_url = \"https://a.example\"\n").unwrap();
        let mut rounds = 0;
        let result = apply_observed(
            &path,
            home.path(),
            &defaults(),
            &[Change::set("disk_cache", false)],
            || {
                rounds += 1;
                std::fs::write(
                    &path,
                    format!("base_url = \"https://a.example\"\nmax_request_mb = {rounds}\n"),
                )
                .unwrap();
            },
        );
        assert!(matches!(result, Err(EditError::Conflict)), "{result:?}");
        assert_eq!(rounds, MAX_ATTEMPTS);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            format!("base_url = \"https://a.example\"\nmax_request_mb = {MAX_ATTEMPTS}\n")
        );
    }

    /// 目标是符号链接：写到它指向的文件，链接还是链接。
    #[cfg(unix)]
    #[test]
    fn a_symlink_target_is_written_through() {
        let home = tempfile::tempdir().unwrap();
        let real = home.path().join("real.toml");
        let link = home.path().join("config.toml");
        std::fs::write(&real, "base_url = \"https://a.example\"\n").unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let applied = apply(
            &link,
            home.path(),
            &defaults(),
            &[Change::set("disk_cache", false)],
        )
        .unwrap();
        assert_eq!(applied.file, std::fs::canonicalize(&real).unwrap());
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(
            std::fs::read_to_string(&real)
                .unwrap()
                .contains("disk_cache = false")
        );
    }
}
