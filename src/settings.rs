//! 内部配置（`~/.aproxy/settings.json`）：程序管理的状态性配置。
//!
//! 与人类可读可写的 config.toml 平行并存：toml 可多份（多开场景各自指定），
//! settings.json 全局唯一。JSON 格式对程序读写更精确；理论上人类可读，
//! 但内容与格式随版本演进，不建议手改——别名等请走命令行管理。

use serde::{Deserialize, Serialize};
use std::{collections::HashMap, path::PathBuf};

/// settings.json 路径：`~/.aproxy/settings.json`
pub fn settings_path() -> PathBuf {
    settings_path_in(&crate::config::config_dir())
}

/// 同上，根目录可指定（测试注入用——单测绝不能读写真实 settings.json）
pub fn settings_path_in(config_dir: &std::path::Path) -> PathBuf {
    config_dir.join("settings.json")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    /// 配置文件别名表：名称 → config.toml 的绝对路径。
    /// 供 `aproxy start/stop <别名>` 快捷定位配置。
    #[serde(default)]
    pub aliases: HashMap<String, String>,
    /// 默认配置文件路径（绝对路径或 `~` 展开）：未指定时启动用
    /// `~/.aproxy/config.toml`。让用户可以把日常主力配置换成任意文件。
    #[serde(default)]
    pub default_config: Option<String>,
    /// 配置目录列表：doctor 等检查会扫描这些目录下的 *.toml 文件
    /// （只查该层，不递归子目录）。默认含 ~/.aproxy/ 与 ~/.aproxy/configs/，
    /// 两者始终参与检查；用户手动添加这两者也不会报错（去重静默）。
    #[serde(default)]
    pub config_dirs: Vec<String>,
    /// 守护日志运行期轮转阈值（MB）：超过即截断；0 表示不轮转。全局治理项
    /// （所有实例共用同一日志策略，故放 settings 而非各 config.toml）。默认 8。
    #[serde(default = "default_log_rotate_mb")]
    pub log_rotate_mb: u64,
    /// 实例空闲判定阈值（秒）：`aproxy status --idle` 筛选与 `aproxy stop idle`
    /// 的判定依据（距最近一次收到客户端请求）。默认 1800（30 分钟）。
    #[serde(default = "default_idle_timeout_secs")]
    pub idle_timeout_secs: u64,
    /// 请求体大小上限（MB，全局默认值）：超出即 413。各 config.toml 可用
    /// `max_body_mb` 按实例覆盖（toml > settings > 内置默认）。默认 128。
    #[serde(default = "default_max_body_mb")]
    pub max_body_mb: u64,
    /// 磁盘缓存（全局默认值）：开启时，超过内存驻留阈值的请求体与响应 spool
    /// 溢写到磁盘临时文件，进程内存与负载大小解耦（用 page cache 换进程内存）。
    /// 各 config.toml 可用 `disk_cache` 按实例覆盖。默认 true；
    /// 低并发实例可关闭以保持全内存行为。
    #[serde(default = "default_disk_cache")]
    pub disk_cache: bool,
    /// 看门狗总开关：开启时 `aproxy start`/守护自检会确保存在一个全局看护进程
    /// （`aproxy watchdog`），守护崩溃/挂死时按 .restore 记录自动重拉。
    /// 看门狗是系统级单例（一个看护进程看护全部实例），故只在 settings 配置，
    /// config.toml 不参与。默认 true；false 时回到无看护行为（已运行实例不受
    /// 影响——守护与看护者完全解耦）。
    #[serde(default = "default_watchdog")]
    pub watchdog: bool,
    /// 看门狗心跳扫描周期（秒）：看护者每此间隔醒来扫描一次全部实例的共享内存
    /// 心跳表；心跳「新鲜」判定也以此为基准（3×周期内算新鲜）。仅调优用。
    /// 默认 30。
    #[serde(default = "default_watchdog_heartbeat_secs")]
    pub watchdog_heartbeat_secs: u64,
    /// 挂死判定容忍周期数：心跳过期（阈值 = 扫描周期×(N+1)，即 N 倍容忍已被
    /// 过期窗口吸收）且一轮 IPC ping（内部含 3 次探测）无响应，才判定实例挂死
    /// 并杀+重拉。1 = 一轮超时即启动二意见确认。误杀调节阀，默认 1。
    #[serde(default = "default_watchdog_stale_after_cycles")]
    pub watchdog_stale_after_cycles: u64,
    /// crashloop 上限：同一实例连续重拉失败达此次数后放弃（指数退避封顶 300s），
    /// 保留 .restore 记录等人工 `aproxy restore`。默认 5。
    #[serde(default = "default_watchdog_max_restarts")]
    pub watchdog_max_restarts: u32,
    /// 看护者闲置自灭等待（秒）：全部实例清零（优雅停止/无人看护对象）后，看护者
    /// 再等待此时长即自行退出并清理 claim，系统回到零常驻。0 = 永不自灭。
    /// 默认 300。
    #[serde(default = "default_watchdog_idle_exit_secs")]
    pub watchdog_idle_exit_secs: u64,
}

fn default_log_rotate_mb() -> u64 {
    8
}

fn default_idle_timeout_secs() -> u64 {
    1800
}

fn default_max_body_mb() -> u64 {
    crate::config::DEFAULT_MAX_BODY_MB
}

fn default_disk_cache() -> bool {
    crate::config::DEFAULT_DISK_CACHE
}

pub(crate) fn default_watchdog() -> bool {
    true
}

pub(crate) fn default_watchdog_heartbeat_secs() -> u64 {
    30
}

pub(crate) fn default_watchdog_stale_after_cycles() -> u64 {
    1
}

pub(crate) fn default_watchdog_max_restarts() -> u32 {
    5
}

pub(crate) fn default_watchdog_idle_exit_secs() -> u64 {
    300
}

// 手动 Default：serde 的字段默认值（log_rotate_mb=8、idle_timeout_secs=1800）
// 只作用于反序列化，derive 出的 Default 会给数值字段填 0——「默认 0」与
// 「默认 8/1800」语义不同（0=关闭轮转），必须与反序列化保持一致。
impl Default for Settings {
    fn default() -> Self {
        Self {
            aliases: HashMap::new(),
            default_config: None,
            config_dirs: Vec::new(),
            log_rotate_mb: default_log_rotate_mb(),
            idle_timeout_secs: default_idle_timeout_secs(),
            max_body_mb: default_max_body_mb(),
            disk_cache: default_disk_cache(),
            watchdog: default_watchdog(),
            watchdog_heartbeat_secs: default_watchdog_heartbeat_secs(),
            watchdog_stale_after_cycles: default_watchdog_stale_after_cycles(),
            watchdog_max_restarts: default_watchdog_max_restarts(),
            watchdog_idle_exit_secs: default_watchdog_idle_exit_secs(),
        }
    }
}

/// 加载：文件不存在 → 默认空配置；损坏 → 警告后回退默认（内部配置损坏
/// 不应阻断启动，别名丢了可以重新 add）。
pub fn load() -> Settings {
    load_from(&settings_path())
}

/// 同上，路径可指定（测试注入用）
pub fn load_from(path: &std::path::Path) -> Settings {
    match std::fs::read_to_string(path) {
        Ok(content) => match serde_json::from_str::<Settings>(&content) {
            Ok(s) => s,
            Err(e) => {
                eprintln!(
                    "警告: settings.json 解析失败（{}），别名等内部配置已回退为空",
                    e
                );
                Settings::default()
            }
        },
        Err(_) => Settings::default(),
    }
}

/// 解析默认配置文件路径：settings.json 的 `default_config` 优先（`~` 展开），
/// 未指定/指定失效时回退 `~/.aproxy/config.toml`。
pub fn default_config_path() -> PathBuf {
    default_config_path_in(&load())
}

/// 同上，Settings 已加载时直接解析（避免重复读文件）
pub fn default_config_path_in(settings: &Settings) -> PathBuf {
    settings
        .default_config
        .as_deref()
        .map(expand_path)
        .unwrap_or_else(crate::config::config_path)
}

/// 原子保存：同目录 tmp 文件 + rename 覆盖（与实例注册表同思路，
/// 避免并发读到半截 JSON）。
pub fn save(settings: &Settings) -> std::io::Result<()> {
    save_to(&settings_path(), settings)
}

/// 同上，路径可指定（测试注入用）
pub fn save_to(path: &std::path::Path, settings: &Settings) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(settings).expect("序列化 settings 失败");
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// 解析别名 → 配置文件路径；无此别名返回 None。
pub fn resolve_alias(name: &str) -> Option<String> {
    load().aliases.get(name).cloned()
}

/// 别名参数路径的展开与归一：`~` 开头展开为用户主目录，再转绝对路径
/// （不用 canonicalize，避免 Windows \\?\ verbatim 前缀混进 settings）。
pub fn expand_path(path: &str) -> PathBuf {
    let p = if path == "~" || path.starts_with("~/") || path.starts_with("~\\") {
        if let Some(home) = dirs::home_dir() {
            home.join(path.trim_start_matches('~').trim_start_matches(['/', '\\']))
        } else {
            PathBuf::from(path)
        }
    } else {
        PathBuf::from(path)
    };
    std::path::absolute(&p).unwrap_or(p)
}

/// 路径的匹配键：绝对化 + 剥 \\?\ verbatim 前缀 + 分隔符统一 + 小写。
/// 供别名匹配运行实例、配置目录去重等所有「两个路径写法是否指同一文件」
/// 的比较（Windows 文件系统大小写不敏感；不同来源记录的写法可能不同）。
pub fn path_match_key(p: &str) -> String {
    let abs = std::path::absolute(p)
        .map(|a| a.display().to_string())
        .unwrap_or_else(|_| p.to_string());
    let abs = abs
        .strip_prefix(r"\\?\UNC\")
        .map(|rest| format!(r"\\{rest}"))
        .or_else(|| abs.strip_prefix(r"\\?\").map(String::from))
        .unwrap_or(abs);
    #[cfg(windows)]
    {
        abs.replace('/', "\\").to_lowercase()
    }
    #[cfg(not(windows))]
    {
        abs
    }
}

/// 别名合法性与保留字冲突：纯数字会被 start/stop 当端口解析；all/idle/default
/// 是 start/stop/status 的保留目标，均禁止作为别名（"defult" 一并拒绝并提示
/// 正确拼写，避免用户注册后实际无法按预期解析）。
pub fn validate_alias_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("别名不能为空".to_string());
    }
    if name.parse::<u16>().is_ok() {
        return Err("别名不能是纯数字（会与端口号解析冲突）".to_string());
    }
    let lower = name.to_ascii_lowercase();
    let reserved: &[(&str, &str)] = &[
        ("all", "\"all\" 是 stop 的保留目标（停止全部实例）"),
        ("idle", "\"idle\" 是 stop/status 的保留目标（空闲实例筛选）"),
        (
            "default",
            "\"default\" 指代默认配置文件（settings 的 default_config），是保留字",
        ),
        (
            "defult",
            "\"defult\" 是 \"default\" 的常见笔误，两者都保留以免混淆",
        ),
    ];
    for (word, why) in reserved {
        if lower == *word {
            return Err(format!("{why}，不能用作别名"));
        }
    }
    Ok(())
}

/// 默认配置的保留字解析：`default`（含笔误 `defult`，大小写不敏感）→
/// settings.default_config 或 ~/.aproxy/config.toml。供 start/stop 使用。
pub fn resolve_default_target(target: &str) -> Option<PathBuf> {
    let lower = target.to_ascii_lowercase();
    if lower == "default" || lower == "defult" {
        Some(default_config_path())
    } else {
        None
    }
}

/// 固定参与检查的两个默认配置目录（始终在列表中，用户重复添加也静默去重）
pub fn default_config_dirs() -> Vec<String> {
    let root = crate::config::config_dir();
    vec![
        root.display().to_string(),
        root.join("configs").display().to_string(),
    ]
}

/// 生效的配置目录列表：用户配置的 + 两个默认目录，展开路径并去重
/// （大小写/分隔符归一后比较，Windows 文件系统大小写不敏感）。
pub fn effective_config_dirs(settings: &Settings) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for raw in settings
        .config_dirs
        .iter()
        .chain(default_config_dirs().iter())
    {
        let expanded = expand_path(raw);
        let key = path_match_key(&expanded.display().to_string());
        if !out
            .iter()
            .any(|p| path_match_key(&p.display().to_string()) == key)
        {
            out.push(expanded);
        }
    }
    out
}

/// 解析 settings.json 的检查（error 级问题清单；空 = 无问题）。
/// 「每一次软件运行都检查」的入口（main 调用），也在 doctor 中汇总输出。
/// 只做静态合法性判断：JSON 语法由 load_from 的损坏回退捕获——此处重读原文件
/// 区分「文件坏」与「内容非法」，不回退不吞错。
pub fn check_settings_errors_in(path: &std::path::Path) -> Vec<String> {
    let mut errors = Vec::new();
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        // 文件不存在 = 全新安装，不是错误
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return errors,
        Err(e) => {
            errors.push(format!("settings.json 无法读取: {e}"));
            return errors;
        }
    };
    let settings: Settings = match serde_json::from_str(&content) {
        Ok(s) => s,
        Err(e) => {
            errors.push(format!(
                "settings.json JSON 语法错误: {e}（别名等功能不可用）"
            ));
            return errors;
        }
    };
    for (name, path) in &settings.aliases {
        if let Err(e) = validate_alias_name(name) {
            errors.push(format!(
                "别名 \"{name}\" 非法: {e}（start/stop 该别名将无法按预期工作）"
            ));
        }
        if path.trim().is_empty() {
            errors.push(format!("别名 \"{name}\" 指向的路径为空"));
            continue;
        }
        if !expand_path(path).is_file() {
            errors.push(format!(
                "别名 \"{name}\" 指向的配置文件不存在: {path}（start/stop 该别名会失败）"
            ));
        }
    }
    errors
}

/// 每次软件运行调用的 settings.json 检查：发现问题立刻向 stderr 报出
/// （不退出——管理命令 status/stop 不能因内部配置损坏而不可用）。
pub fn check_and_report() {
    for e in check_settings_errors_in(&settings_path()) {
        eprintln!("[ERROR] {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_roundtrip() {
        // 目录注入：绝不读写真实 ~/.aproxy/settings.json
        let dir = tempfile::tempdir().unwrap();
        let path = settings_path_in(dir.path());
        let mut s = Settings::default();
        s.aliases
            .insert("openrouter".into(), "C:/tmp/or.toml".into());
        s.aliases.insert("anthropic".into(), "D:/x.toml".into());
        save_to(&path, &s).unwrap();
        let loaded = load_from(&path);
        assert_eq!(
            loaded.aliases.get("openrouter").map(String::as_str),
            Some("C:/tmp/or.toml")
        );
        assert_eq!(loaded.aliases.len(), 2);
    }

    #[test]
    fn load_missing_returns_default() {
        let dir = tempfile::tempdir().unwrap();
        let s = load_from(&settings_path_in(dir.path()));
        assert!(s.aliases.is_empty());
    }

    #[test]
    fn load_corrupted_falls_back_to_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = settings_path_in(dir.path());
        std::fs::write(&path, "{corrupted").unwrap();
        let s = load_from(&path);
        assert!(s.aliases.is_empty(), "损坏的 settings 应回退为空别名表");
    }

    #[test]
    fn expand_path_tilde_and_absolute() {
        let home = dirs::home_dir().unwrap();
        let expanded = expand_path("~/x/a.toml");
        assert!(expanded.starts_with(&home));
        assert!(expanded.is_absolute());
        // 已绝对路径原样（转 absolute 不改变语义）
        let abs = expand_path("C:/tmp/b.toml");
        assert!(abs.is_absolute());
    }

    #[test]
    fn default_config_path_resolution() {
        // 未指定 → ~/.aproxy/config.toml
        let s = Settings::default();
        assert_eq!(default_config_path_in(&s), crate::config::config_path());
        // 指定 → 展开后使用（~ 展开 + 绝对化）
        let s = Settings {
            default_config: Some("~/my-cfg.toml".into()),
            ..Default::default()
        };
        let p = default_config_path_in(&s);
        assert!(p.ends_with("my-cfg.toml"));
        assert!(p.is_absolute());
        // 绝对路径原样
        let s = Settings {
            default_config: Some("D:/work/main.toml".into()),
            ..Default::default()
        };
        assert_eq!(
            default_config_path_in(&s).display().to_string(),
            std::path::absolute("D:/work/main.toml")
                .unwrap()
                .display()
                .to_string()
        );
    }

    #[test]
    fn default_config_survives_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = settings_path_in(dir.path());
        let s = Settings {
            default_config: Some("C:/tmp/main.toml".into()),
            ..Default::default()
        };
        save_to(&path, &s).unwrap();
        let loaded = load_from(&path);
        assert_eq!(loaded.default_config.as_deref(), Some("C:/tmp/main.toml"));
        // 旧版 settings.json（无该字段）也能加载
        std::fs::write(&path, r#"{"aliases":{}}"#).unwrap();
        assert!(load_from(&path).default_config.is_none());
    }

    #[test]
    fn validate_alias_name_rejects_reserved() {
        assert!(validate_alias_name("").is_err());
        assert!(validate_alias_name("all").is_err());
        assert!(validate_alias_name("ALL").is_err());
        assert!(validate_alias_name("12345").is_err());
        // default/defult/idle 均为保留字（大小写不敏感）
        for bad in ["default", "DEFAULT", "defult", "Defult", "idle", "IDLE"] {
            assert!(validate_alias_name(bad).is_err(), "{bad} 应被拒绝");
        }
        assert!(validate_alias_name("openrouter").is_ok());
        assert!(validate_alias_name("my-cfg").is_ok());
    }

    #[test]
    fn resolve_default_target_matches_default_and_typo() {
        // default/defult（大小写不敏感）解析为默认配置路径
        for good in ["default", "DEFAULT", "defult", "Defult"] {
            assert_eq!(
                resolve_default_target(good),
                Some(crate::config::config_path()),
                "{good} 应解析为默认配置"
            );
        }
        // 非保留字不命中
        assert_eq!(resolve_default_target("openrouter"), None);
        assert_eq!(resolve_default_target("my-cfg"), None);
    }

    #[test]
    fn idle_timeout_roundtrip_and_default() {
        // 默认 1800；自定义值落盘往返；旧 settings（无字段）读出默认
        let dir = tempfile::tempdir().unwrap();
        let path = settings_path_in(dir.path());
        let s = Settings::default();
        assert_eq!(s.idle_timeout_secs, 1800);
        let s = Settings {
            idle_timeout_secs: 600,
            ..Default::default()
        };
        save_to(&path, &s).unwrap();
        assert_eq!(load_from(&path).idle_timeout_secs, 600);
        std::fs::write(&path, r#"{"aliases":{}}"#).unwrap();
        assert_eq!(load_from(&path).idle_timeout_secs, 1800);
    }

    #[test]
    fn body_limit_and_disk_cache_roundtrip_and_default() {
        // 新字段默认值（128MB / 开启）；自定义落盘往返；旧 settings（无字段）
        // 读出默认——保证升级不改变行为
        let dir = tempfile::tempdir().unwrap();
        let path = settings_path_in(dir.path());
        let s = Settings::default();
        assert_eq!(s.max_body_mb, 128);
        assert!(s.disk_cache);
        let s = Settings {
            max_body_mb: 256,
            disk_cache: false,
            ..Default::default()
        };
        save_to(&path, &s).unwrap();
        let loaded = load_from(&path);
        assert_eq!(loaded.max_body_mb, 256);
        assert!(!loaded.disk_cache);
        // 旧版 settings.json（无这两个字段）也能加载且取默认
        std::fs::write(&path, r#"{"aliases":{}}"#).unwrap();
        let loaded = load_from(&path);
        assert_eq!(loaded.max_body_mb, 128);
        assert!(loaded.disk_cache);
    }

    #[test]
    fn watchdog_fields_roundtrip_and_default() {
        // 五字段默认值；自定义落盘往返；旧 settings（无字段）读出默认——
        // 升级 alpha.5 的 settings.json 时看门狗自动按默认开启
        let dir = tempfile::tempdir().unwrap();
        let path = settings_path_in(dir.path());
        let s = Settings::default();
        assert!(s.watchdog);
        assert_eq!(s.watchdog_heartbeat_secs, 30);
        assert_eq!(s.watchdog_stale_after_cycles, 1);
        assert_eq!(s.watchdog_max_restarts, 5);
        assert_eq!(s.watchdog_idle_exit_secs, 300);
        let s = Settings {
            watchdog: false,
            watchdog_heartbeat_secs: 10,
            watchdog_stale_after_cycles: 3,
            watchdog_max_restarts: 2,
            watchdog_idle_exit_secs: 0,
            ..Default::default()
        };
        save_to(&path, &s).unwrap();
        let loaded = load_from(&path);
        assert!(!loaded.watchdog);
        assert_eq!(loaded.watchdog_heartbeat_secs, 10);
        assert_eq!(loaded.watchdog_stale_after_cycles, 3);
        assert_eq!(loaded.watchdog_max_restarts, 2);
        assert_eq!(loaded.watchdog_idle_exit_secs, 0);
        // 旧版 settings.json（无字段）全取默认
        std::fs::write(&path, r#"{"aliases":{}}"#).unwrap();
        let loaded = load_from(&path);
        assert!(loaded.watchdog);
        assert_eq!(loaded.watchdog_heartbeat_secs, 30);
        assert_eq!(loaded.watchdog_stale_after_cycles, 1);
        assert_eq!(loaded.watchdog_max_restarts, 5);
        assert_eq!(loaded.watchdog_idle_exit_secs, 300);
    }

    #[test]
    fn effective_config_dirs_dedupes_and_appends_defaults() {
        // 空配置 → 两个默认目录
        let s = Settings::default();
        let dirs = effective_config_dirs(&s);
        assert_eq!(dirs.len(), 2);
        // 重复添加默认目录 → 静默去重不报错；分隔符/大小写变体也去重
        let mut s = Settings::default();
        let root = crate::config::config_dir().display().to_string();
        s.config_dirs = vec![root.clone(), root.replace('\\', "/").to_uppercase()];
        let dirs = effective_config_dirs(&s);
        assert_eq!(dirs.len(), 2, "重复/变体写法应去重: {dirs:?}");
    }

    #[test]
    fn check_settings_reports_bad_aliases_and_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = settings_path_in(dir.path());

        // 不存在 = 全新安装，无错误
        assert!(check_settings_errors_in(&path).is_empty());

        // JSON 语法错误 → error
        std::fs::write(&path, "{bad json").unwrap();
        let errs = check_settings_errors_in(&path);
        assert!(
            errs.len() == 1 && errs[0].contains("JSON 语法错误"),
            "{errs:?}"
        );

        // 别名为纯数字 / All（大小写变体）/ 指向不存在文件 → error
        std::fs::write(
            &path,
            r#"{"aliases": {"12345": "C:/tmp/a.toml", "All": "C:/tmp/b.toml", "good": "Z:/no/such.toml"}}"#,
        )
        .unwrap();
        let errs = check_settings_errors_in(&path);
        assert!(errs.iter().any(|e| e.contains("12345")), "{errs:?}");
        assert!(errs.iter().any(|e| e.contains("All")), "{errs:?}");
        assert!(errs.iter().any(|e| e.contains("不存在")), "{errs:?}");

        // 合法配置 → 无错误（JSON 内路径用正斜杠，避免反斜杠转义）
        let cfg = dir.path().join("real.toml");
        std::fs::write(&cfg, "base_url = \"https://x.example.com\"").unwrap();
        std::fs::write(
            &path,
            format!(
                r#"{{"aliases": {{"good": "{}"}}}}"#,
                cfg.display()
                    .to_string()
                    .replace(std::path::MAIN_SEPARATOR, "/")
            ),
        )
        .unwrap();
        assert!(check_settings_errors_in(&path).is_empty());
    }
}
