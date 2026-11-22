//! 拉起规格与期望状态记录：一个后台实例「怎样启动」的结构化描述，以及「它应当在运行」
//! 这件事的落盘形式。
//!
//! 拉起规格 [`InstanceSpec`] 取代了把启动者的命令行原样存盘、原样转发的做法：命令行落盘会
//! 把启动语法连同明文 key 一起冻结成跨版本的契约，而且任何人列一下进程就能看到 key。规格
//! 是一个 JSON 对象，经文件交给子进程（`aproxy __serve --home <dir> --spec <file>`），
//! 密钥因此不进任何命令行。
//!
//! 同一份规格有两个去处：
//! - `run/<纳秒>-<pid>.spec.json`：拉起方写给子进程的交接文件，子进程就绪或失败之后由
//!   拉起方删掉；
//! - `run/<端口>.restore.json`：期望状态记录，内嵌规格。实例启动时写，只有 stop 删它。
//!   进程不在而记录还在，正是 restart、restore 与看护者要恢复的情形。
//!
//! 两种文件都只有本用户能读（见 [`crate::fsx::write_atomic_private`]）。读者遇到读不懂的
//! 记录（解析失败，或 `schema` 高于自己认识的）只报告、绝不删除：写下它的可能是更新的版本。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[cfg(test)]
use aproxy_ipc::registry::parse_record;
use aproxy_ipc::registry::{RESTORE_SUFFIX, read_optional_record, read_record};
pub use aproxy_ipc::registry::{SCHEMA, Unreadable, restore_path_in};

use crate::config::Overrides;
use serde::{Deserialize, Serialize};

/// 拉起规格。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct InstanceSpec {
    pub schema: u32,
    /// 配置文件的绝对路径。null = 没有配置文件，只靠覆盖参数与 settings.json 的全局默认
    /// 启动；对这样的规格不做「配置文件还在不在」的检查
    pub config_path: Option<String>,
    /// 启动时在命令行上给过的覆盖，键就是 config.toml 的键
    #[serde(default)]
    pub overrides: Overrides,
    /// 配置解析出的端口为 0（由系统分配）的实例，第一次启动后实际拿到的端口。再次拉起时，
    /// 配置解析出的端口仍是 0 才用它：客户端配的是这个端口，恢复要回到同一个端口；配置
    /// 已经改成固定端口时以配置为准。它不放进 `overrides`，因为覆盖的优先级高于配置，放在
    /// 那里用户以后改的端口永远不生效
    #[serde(default)]
    pub pinned_port: Option<u16>,
    /// 启动者环境里的代理变量与 `PATH`：null 表示启动者没设，拉起时在子进程里删掉它。
    /// 重拉出来的实例因此走同一条网络路径，也找得到同一批 format 程序
    #[serde(default)]
    pub env: BTreeMap<String, Option<String>>,
    /// 实例的日志文件。新启动时由拉起方生成随机名，restart、restore、看护者重拉都续写
    /// 同一个文件。配置里写了 `log_file` 时实例用配置的路径，这个值只是没写时的去处
    pub log_path: String,
}

impl InstanceSpec {
    /// 配置文件路径；没有配置文件的规格返回 None。
    pub fn config_file(&self) -> Option<PathBuf> {
        self.config_path.as_deref().map(PathBuf::from)
    }
}

/// 期望状态记录的落盘形式。
#[derive(Serialize, Deserialize)]
struct RestoreRecord {
    pub schema: u32,
    pub spec: InstanceSpec,
    /// 写下的时刻（Unix 毫秒）
    pub written_at: u64,
}

/// 交接文件的文件名后缀：`<纳秒>-<pid>.spec.json`。
const SPEC_SUFFIX: &str = ".spec.json";

/// 为一次新的启动建规格：记下此刻环境里的代理变量与 `PATH`。
pub fn capture(config_path: Option<&Path>, overrides: Overrides, log_path: &Path) -> InstanceSpec {
    InstanceSpec {
        schema: SCHEMA,
        config_path: config_path.map(|p| p.display().to_string()),
        overrides,
        pinned_port: None,
        env: capture_env(),
        log_path: log_path.display().to_string(),
    }
}
// ---------------------------------------------------------------------------
// 环境快照
// ---------------------------------------------------------------------------

/// 规格里记下的环境变量名。Windows 上变量名不分大小写，每个变量只记一个大写的名字；
/// unix 上大小写是两个变量，HTTP 客户端两种都读，所以各记一条。TLS 相关的变量不记：
/// 客户端用 rustls 与内置的根证书，它们不影响它。
#[cfg(windows)]
const ENV_NAMES: &[&str] = &["HTTPS_PROXY", "HTTP_PROXY", "ALL_PROXY", "NO_PROXY", "PATH"];
#[cfg(not(windows))]
const ENV_NAMES: &[&str] = &[
    "HTTPS_PROXY",
    "https_proxy",
    "HTTP_PROXY",
    "http_proxy",
    "ALL_PROXY",
    "all_proxy",
    "NO_PROXY",
    "no_proxy",
    "PATH",
];

/// 记下当前进程环境里 [`ENV_NAMES`] 的值；没设的记 None。值不是合法 Unicode 的变量按没设
/// 处理（JSON 存不下它，而代理地址与 `PATH` 里出现这种值本身就不能用）。
pub fn capture_env() -> BTreeMap<String, Option<String>> {
    // std::env::var 在 Windows 上本来就不分大小写地查找
    ENV_NAMES
        .iter()
        .map(|name| ((*name).to_string(), std::env::var(name).ok()))
        .collect()
}

// ---------------------------------------------------------------------------
// 交接文件
// ---------------------------------------------------------------------------

/// 把规格写成交接文件 `run/<纳秒>-<pid>.spec.json`，返回路径。名字里的 pid 是拉起方的：
/// 残留的交接文件靠它判断写者是否还在。
pub fn write_spec_file(run_dir: &Path, spec: &InstanceSpec) -> std::io::Result<PathBuf> {
    write_launch_file(run_dir, spec, None)
}
/// 只在单次交接文件中携带待核对的运行配置摘要，不改变恢复记录，也不复制有效配置中的凭据。
pub(crate) fn write_launch_file(
    run_dir: &Path,
    spec: &InstanceSpec,
    revision: Option<[u8; 32]>,
) -> std::io::Result<PathBuf> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let path = run_dir.join(format!("{nanos}-{}{SPEC_SUFFIX}", std::process::id()));
    #[derive(Serialize)]
    struct Launch<'a> {
        #[serde(flatten)]
        spec: &'a InstanceSpec,
        #[serde(skip_serializing_if = "Option::is_none")]
        expected_revision: Option<[u8; 32]>,
    }
    crate::fsx::write_atomic_private(
        &path,
        &serde_json::to_vec(&Launch {
            spec,
            expected_revision: revision,
        })?,
    )?;
    Ok(path)
}

/// 一次读取规格及其可选的启动前置条件；期望状态和跨版本恢复都不继承此条件。
pub fn read_launch_file(path: &Path) -> Result<(InstanceSpec, Option<[u8; 32]>), Unreadable> {
    #[derive(Deserialize)]
    struct Launch {
        #[serde(flatten)]
        spec: InstanceSpec,
        expected_revision: Option<[u8; 32]>,
    }
    let v: Launch = read_record(path, SCHEMA)?;
    Ok((v.spec, v.expected_revision))
}
/// 读交接文件。
pub fn read_spec_file(path: &Path) -> Result<InstanceSpec, Unreadable> {
    read_record(path, SCHEMA)
}

// ---------------------------------------------------------------------------
// 期望状态记录
// ---------------------------------------------------------------------------

/// 一条读得懂的期望状态记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreEntry {
    /// 文件名里的端口
    pub port: String,
    pub spec: InstanceSpec,
    /// 写下的时刻（Unix 毫秒）
    pub written_at: u64,
}

/// 一条读不懂的记录：在哪、为什么。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnreadableRecord {
    pub path: PathBuf,
    pub reason: Unreadable,
}

/// run 目录里全部期望状态记录的清点结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RestoreListing {
    /// 读得懂的，按端口排序
    pub entries: Vec<RestoreEntry>,
    /// 读不懂的：原样留在盘上，交给调用方报告
    pub unreadable: Vec<UnreadableRecord>,
}

/// 进程内的停机锁不保护其他实例或窗口。发布、比较删除和迁移退役都持有同一个短期文件锁。
pub(crate) fn restore_lock(run_dir: &Path) -> std::io::Result<crate::install::InstallLock> {
    crate::install::wait_lock_path(
        &run_dir.join(".restore.lock"),
        std::time::Duration::from_secs(2),
    )
}

/// 写（或覆盖）一个端口的期望状态记录。
pub fn write_restore_in(run_dir: &Path, port: &str, spec: &InstanceSpec) -> std::io::Result<()> {
    let _lock = restore_lock(run_dir)?;
    let record = RestoreRecord {
        schema: SCHEMA,
        spec: spec.clone(),
        written_at: now_millis(),
    };
    let json = serde_json::to_string(&record).expect("desired-state records must serialize");
    crate::fsx::write_atomic_private(&restore_path_in(run_dir, port), json.as_bytes())
}

/// 删除一个端口的期望状态记录；没有时什么都不做。这是「用户要它停下」的落盘动作，只有
/// stop 的各条路径与「记录被新端口的记录接替」可以调用。
pub fn remove_restore_in(run_dir: &Path, port: &str) {
    if run_dir.is_dir()
        && let Ok(_lock) = restore_lock(run_dir)
    {
        let _ = std::fs::remove_file(restore_path_in(run_dir, port));
    }
}

/// 读一个端口的期望状态记录。`Ok(None)` = 没有这份记录。
pub fn read_restore_in(run_dir: &Path, port: &str) -> Result<Option<RestoreEntry>, Unreadable> {
    let path = restore_path_in(run_dir, port);
    let Some(record): Option<RestoreRecord> = read_optional_record(&path, SCHEMA)? else {
        return Ok(None);
    };
    Ok(Some(RestoreEntry {
        port: port.to_string(),
        spec: record.spec,
        written_at: record.written_at,
    }))
}

/// 清点 run 目录里的期望状态记录。只读：读不懂的记录留在原处，列进 `unreadable`。
pub fn list_restore_in(run_dir: &Path) -> RestoreListing {
    let mut listing = RestoreListing::default();
    let Ok(entries) = std::fs::read_dir(run_dir) else {
        return listing;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(port) = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_suffix(RESTORE_SUFFIX))
            .map(str::to_string)
        else {
            continue;
        };
        match read_record::<RestoreRecord>(&path, SCHEMA) {
            Ok(record) => listing.entries.push(RestoreEntry {
                port,
                spec: record.spec,
                written_at: record.written_at,
            }),
            // 清点与删除之间没有锁：记录恰在这一刻被 stop 删掉不算读不懂
            Err(Unreadable::Io(_)) if !path.exists() => {}
            Err(reason) => listing.unreadable.push(UnreadableRecord { path, reason }),
        }
    }
    listing.entries.sort_by(|a, b| a.port.cmp(&b.port));
    listing.unreadable.sort_by(|a, b| a.path.cmp(&b.path));
    listing
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> InstanceSpec {
        InstanceSpec {
            schema: SCHEMA,
            config_path: Some("/home/u/.aproxy/configs/work.toml".to_string()),
            overrides: Overrides {
                listen_addr: Some("127.0.0.1:12346".to_string()),
                api_key: Some("sk-ant-SECRET".to_string()),
                ..Overrides::default()
            },
            pinned_port: None,
            env: BTreeMap::from([
                (
                    "HTTPS_PROXY".to_string(),
                    Some("http://127.0.0.1:7890".to_string()),
                ),
                ("HTTP_PROXY".to_string(), None),
                ("PATH".to_string(), Some("/usr/bin".to_string())),
            ]),
            log_path: "/home/u/.aproxy/logs/19ac3f2e8b5d-1a2b.log".to_string(),
        }
    }

    #[test]
    fn spec_serializes_to_the_documented_shape() {
        let json: serde_json::Value = serde_json::to_value(sample()).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "schema": 1,
                "config_path": "/home/u/.aproxy/configs/work.toml",
                // 只含给过的覆盖
                "overrides": {"listen_addr": "127.0.0.1:12346", "api_key": "sk-ant-SECRET"},
                "pinned_port": null,
                // 没设的变量写 null，拉起时据此在子进程里删掉它
                "env": {
                    "HTTPS_PROXY": "http://127.0.0.1:7890",
                    "HTTP_PROXY": null,
                    "PATH": "/usr/bin"
                },
                "log_path": "/home/u/.aproxy/logs/19ac3f2e8b5d-1a2b.log"
            })
        );
        // 没有配置文件的规格：config_path 为 null
        let mut bare = sample();
        bare.config_path = None;
        let json = serde_json::to_value(&bare).unwrap();
        assert!(json["config_path"].is_null());
        assert_eq!(bare.config_file(), None);
    }

    #[test]
    fn spec_reads_back_and_tolerates_added_fields() {
        let spec = sample();
        let text = serde_json::to_string(&spec).unwrap();
        assert_eq!(parse_record::<InstanceSpec>(&text, SCHEMA).unwrap(), spec);

        // 以后的版本加的字段，今天的读者忽略；缺省的可选字段取默认
        let text = r#"{"schema":1,"config_path":null,"log_path":"/x.log","added_later":{"a":1}}"#;
        let spec: InstanceSpec = parse_record(text, SCHEMA).unwrap();
        assert_eq!(spec.config_path, None);
        assert_eq!(spec.overrides, Overrides::default());
        assert_eq!(spec.pinned_port, None);
        assert!(spec.env.is_empty());
    }

    #[test]
    fn newer_schema_and_garbage_are_unreadable_not_misread() {
        // schema 更高：即使别的字段形状变了，也报「太新」而不是「损坏」
        let text = r#"{"schema":2,"config_path":["now","a","list"]}"#;
        assert_eq!(
            parse_record::<InstanceSpec>(text, SCHEMA),
            Err(Unreadable::SchemaTooNew(2))
        );
        assert!(matches!(
            parse_record::<InstanceSpec>("{not json", SCHEMA),
            Err(Unreadable::Parse(_))
        ));
        assert!(matches!(
            parse_record::<InstanceSpec>(r#"{"config_path":null,"log_path":"x"}"#, SCHEMA),
            Err(Unreadable::Parse(_))
        ));
        assert!(matches!(
            parse_record::<InstanceSpec>(r#"{"schema":1}"#, SCHEMA),
            Err(Unreadable::Parse(_))
        ));
    }

    #[test]
    fn spec_file_round_trips_and_is_named_after_the_writer() {
        let dir = tempfile::tempdir().unwrap();
        let spec = sample();
        let path = write_spec_file(dir.path(), &spec).unwrap();
        let name = path.file_name().unwrap().to_str().unwrap();
        assert!(
            name.ends_with(&format!("-{}.spec.json", std::process::id())),
            "{name}"
        );
        assert_eq!(read_spec_file(&path).unwrap(), spec);
        assert!(matches!(
            read_spec_file(&dir.path().join("gone.spec.json")),
            Err(Unreadable::Io(_))
        ));
    }

    #[test]
    fn restore_records_list_sorted_and_never_delete_what_they_cannot_read() {
        let dir = tempfile::tempdir().unwrap();
        let run = dir.path();
        assert_eq!(list_restore_in(run), RestoreListing::default());
        assert_eq!(read_restore_in(run, "12345"), Ok(None));

        let mut spec = sample();
        write_restore_in(run, "12346", &spec).unwrap();
        spec.pinned_port = Some(40123);
        write_restore_in(run, "12345", &spec).unwrap();
        // 读不懂的两种：损坏的，与更新版本写的
        let broken = run.join("7.restore.json");
        std::fs::write(&broken, "{").unwrap();
        let newer = run.join("8.restore.json");
        std::fs::write(&newer, r#"{"schema":9,"spec":"reshaped"}"#).unwrap();
        // 不相干的文件不算
        std::fs::write(run.join("12345.instance.json"), "{}").unwrap();
        std::fs::write(run.join("12345.restore"), "[]").unwrap();

        let listing = list_restore_in(run);
        assert_eq!(
            listing
                .entries
                .iter()
                .map(|e| e.port.as_str())
                .collect::<Vec<_>>(),
            ["12345", "12346"]
        );
        assert_eq!(listing.entries[0].spec.pinned_port, Some(40123));
        assert!(listing.entries[0].written_at > 0);
        assert_eq!(
            listing
                .unreadable
                .iter()
                .map(|u| (u.path.clone(), u.reason.clone()))
                .collect::<Vec<_>>()[1],
            (newer.clone(), Unreadable::SchemaTooNew(9))
        );
        assert_eq!(listing.unreadable[0].path, broken);
        assert!(matches!(listing.unreadable[0].reason, Unreadable::Parse(_)));
        // 读不懂的记录原样留着
        assert!(broken.exists() && newer.exists());
        assert_eq!(read_restore_in(run, "8"), Err(Unreadable::SchemaTooNew(9)));

        // 覆盖写与删除
        let entry = read_restore_in(run, "12345").unwrap().unwrap();
        assert_eq!(entry.spec, spec);
        remove_restore_in(run, "12345");
        remove_restore_in(run, "12345");
        assert_eq!(read_restore_in(run, "12345"), Ok(None));
    }

    #[test]
    fn captured_env_names_every_tracked_variable() {
        let env = capture_env();
        for name in ENV_NAMES {
            assert!(env.contains_key(*name), "{name}");
        }
        // PATH 在任何测试环境里都有
        assert!(env["PATH"].is_some());
    }

    #[cfg(unix)]
    #[test]
    fn records_are_private_to_the_owner() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let spec_path = write_spec_file(dir.path(), &sample()).unwrap();
        write_restore_in(dir.path(), "12345", &sample()).unwrap();
        for path in [spec_path, restore_path_in(dir.path(), "12345")] {
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "{}", path.display());
        }
    }
}
