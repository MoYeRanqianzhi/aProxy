//! 全局设置（`<home>/settings.json`）：别名、默认配置、config.toml 键的全局默认、看护者、
//! 日志、安装更新等。config.toml 可以有多份（多开时各指各的），settings.json 一个 home 只有
//! 一份。
//!
//! - [`file`]：严格解析——文件里写了什么，连同带键路径的诊断。
//! - [`edit`]：写回——按键路径打补丁，不动其余内容。
//! - 本文件：内存里的 [`Settings`]、加载（[`load`]、[`read`]），以及别名与路径的工具。
//!
//! [`Settings`] 的字段名是程序一侧的名字，与文件里的键不一一同名（文件按段分组：
//! `watchdog.enabled`、`logs.rotate_mb`、`instances.idle_threshold_secs`、`update.*`）；
//! 对应关系见 [`file::parse`]。

pub mod edit;
pub mod file;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use aproxy_ipc::diag::Diagnostic;

use crate::config::file::FileConfig;

/// settings.json 路径：`<home>/settings.json`
pub fn settings_path() -> PathBuf {
    settings_path_in(&crate::config::config_dir())
}

/// 同上，根目录可指定（测试注入用——单测绝不能读写真实 settings.json）
pub fn settings_path_in(config_dir: &std::path::Path) -> PathBuf {
    config_dir.join("settings.json")
}

/// 内存里的全局设置。由 [`file::parse`] 从 settings.json 读出；没有文件时是内置默认。
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// 界面与命令行输出的语言：`system`（跟随系统）、`en` 或 `zh-CN`。
    pub language: String,
    /// config.toml 键的全局默认（文件里的 `defaults` 段）：没写在某份 config.toml 里的键
    /// 取这里的值。不含 `listen_addr`、`log_file`、`format`——它们属于单个实例。凭据类的
    /// 默认值只发给它所属的目标，见 `config::resolve`。
    pub defaults: FileConfig,
    /// 配置文件别名表：名称 → config.toml 的绝对路径。
    /// 供 `aproxy start/stop <别名>` 快捷定位配置。
    pub aliases: HashMap<String, String>,
    /// 默认配置文件路径（绝对路径或 `~` 展开）：未指定时启动用
    /// `~/.aproxy/config.toml`。让用户可以把日常主力配置换成任意文件。
    pub default_config: Option<String>,
    /// 配置目录列表：doctor 等检查会扫描这些目录下的 *.toml 文件
    /// （只查该层，不递归子目录）。默认含 ~/.aproxy/ 与 ~/.aproxy/configs/，
    /// 两者始终参与检查；用户手动添加这两者也不会报错（去重静默）。
    pub config_dirs: Vec<String>,
    /// 守护日志运行期轮转阈值（MB）：超过即截断；0 表示不轮转。全局治理项
    /// （所有实例共用同一日志策略，故放 settings 而非各 config.toml）。默认 8。
    pub log_rotate_mb: u64,
    /// 实例空闲判定阈值（秒）：`aproxy status --idle` 筛选与 `aproxy stop idle`
    /// 的判定依据（距最近一次收到客户端请求）。默认 1800（30 分钟）。
    pub idle_timeout_secs: u64,
    /// 看门狗总开关：开启时 `aproxy start`/守护自检会确保存在一个全局看护进程
    /// （同二进制以内部入口 `__watchdog` 分离启动），实例崩溃/挂死时按
    /// 期望状态记录自动重拉。
    /// 看门狗是系统级单例（一个看护进程看护全部实例），故只在 settings 配置，
    /// config.toml 不参与。默认 true；false 时回到无看护行为（已运行实例不受
    /// 影响——守护与看护者完全解耦）。
    pub watchdog: bool,
    /// 看门狗心跳扫描周期（秒）：看护者每此间隔醒来扫描一次全部实例的共享内存
    /// 心跳表；心跳「新鲜」判定也以此为基准（3×周期内算新鲜）。仅调优用。
    /// 默认 30。
    pub watchdog_heartbeat_secs: u64,
    /// 挂死判定容忍周期数：心跳过期（阈值 = 扫描周期×(N+1)，即 N 倍容忍已被
    /// 过期窗口吸收）且一轮 IPC ping（内部含 3 次探测）无响应，才判定实例挂死
    /// 并杀+重拉。1 = 一轮超时即启动二意见确认。误杀调节阀，默认 1。
    pub watchdog_stale_after_cycles: u64,
    /// crashloop 上限：同一实例连续重拉失败达此次数后放弃（指数退避封顶 300s），
    /// 保留期望状态记录等人工 `aproxy restore`。默认 5。
    pub watchdog_max_restarts: u32,
    /// 看护者闲置自灭等待（秒）：全部实例清零（优雅停止/无人看护对象）后，看护者
    /// 再等待此时长即自行退出并清理 claim，系统回到零常驻。0 = 永不自灭。
    /// 默认 300。
    pub watchdog_idle_exit_secs: u64,
    /// install 下载链条（可选，未配置 = 内置默认链 github → npm →
    /// cargo-binstall → cargo）。**严格数组语义**（与 config_dirs 相反）：
    /// 配置后完全按数组执行，绝不自动追加默认项——写少了会增加失败概率，
    /// 建议写全。元素为渠道名（"github"/"npm"/"cargo-binstall"/"cargo"）
    /// 或 url 模板对象（{"url": "https://…/{version}/{asset}"}，CDN 域名
    /// 不硬编码进二进制，由用户自指定）。
    pub download_chain: Option<Vec<crate::install::download::ChainStep>>,
    /// skill 文档自动更新总开关：install 时随二进制并行下载更新
    /// ~/.aproxy/skills/。默认 true；false 时 install 完全跳过 skill 任务。
    pub skill_auto_update: bool,
    /// 下载代理 URL（仅 install 下载使用）：与 config.toml 的 `proxy`
    /// （上游请求转发）**绝对分离**——两套命名、两套配置。未配置时回退
    /// 环境变量（HTTPS_PROXY 等系统代理）。可用 CLI --download-proxy
    /// 按次覆盖。错误信息与 config --show 同样对内嵌凭据打码。
    pub download_proxy: Option<String>,
}

fn default_log_rotate_mb() -> u64 {
    8
}

fn default_idle_timeout_secs() -> u64 {
    1800
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

fn default_true() -> bool {
    true
}

// 手动 Default：内置默认值不是各类型的零值（日志轮转 8 MB、空闲阈值 1800 秒……），而 0 在
// 这些键上另有含义（0 = 关闭轮转）。
impl Default for Settings {
    fn default() -> Self {
        Self {
            language: "system".to_string(),
            defaults: FileConfig::default(),
            aliases: HashMap::new(),
            default_config: None,
            config_dirs: Vec::new(),
            log_rotate_mb: default_log_rotate_mb(),
            idle_timeout_secs: default_idle_timeout_secs(),
            watchdog: default_watchdog(),
            watchdog_heartbeat_secs: default_watchdog_heartbeat_secs(),
            watchdog_stale_after_cycles: default_watchdog_stale_after_cycles(),
            watchdog_max_restarts: default_watchdog_max_restarts(),
            watchdog_idle_exit_secs: default_watchdog_idle_exit_secs(),
            download_chain: None,
            skill_auto_update: default_true(),
            download_proxy: None,
        }
    }
}

/// 读一次 settings.json 的结果。
#[derive(Debug, Clone)]
pub struct Loaded {
    /// 文件有效时是文件里的设置；没有文件时是内置默认。文件无效时也是内置默认：半对半错的
    /// 设置不拿来做事
    pub settings: Settings,
    /// 文件里的问题；空 = 有效（或没有文件）
    pub diagnostics: Vec<Diagnostic>,
}

impl Loaded {
    pub fn is_valid(&self) -> bool {
        !aproxy_ipc::diag::has_errors(&self.diagnostics)
    }
}

/// 严格地读 `path` 处的 settings.json：文件不存在 → 内置默认、没有诊断；读不出来或内容有
/// 问题 → 内置默认，连同诊断。
pub fn read(path: &Path) -> Loaded {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Loaded {
                settings: Settings::default(),
                diagnostics: Vec::new(),
            };
        }
        Err(e) => {
            return Loaded {
                settings: Settings::default(),
                diagnostics: vec![Diagnostic {
                    code: "config_syntax",
                    severity: aproxy_ipc::diag::Level::Error,
                    file: path.display().to_string(),
                    key: None,
                    line: None,
                    column: None,
                    message: format!("cannot read the file: {e}"),
                    details: Default::default(),
                }],
            };
        }
    };
    let (settings, diagnostics) = file::parse(&text, path);
    if aproxy_ipc::diag::has_errors(&diagnostics) {
        Loaded {
            settings: Settings::default(),
            diagnostics,
        }
    } else {
        Loaded {
            settings,
            diagnostics,
        }
    }
}

/// 加载本 home 的设置。文件不存在 → 内置默认。文件无效时不失败：返回这个进程上一次读到的
/// 有效设置，没有就返回内置默认，并告警一次。
///
/// 无效的 settings.json 要不要挡住一个命令，由命令入口决定（见二进制的 main）：停实例、
/// 看状态、安装任何时候都要能用；长期运行的看护者则不能因为用户改坏了文件就丢掉它一直在
/// 用的设置。所以这里只负责「给一份能用的」。
pub fn load() -> Settings {
    load_from(&settings_path())
}

/// 同上，路径可指定（测试注入用）
pub fn load_from(path: &Path) -> Settings {
    // 上一次读到的有效设置，按路径记
    static LAST_VALID: std::sync::Mutex<Option<HashMap<PathBuf, Settings>>> =
        std::sync::Mutex::new(None);
    let loaded = read(path);
    let mut last = LAST_VALID.lock().unwrap_or_else(|e| e.into_inner());
    let last = last.get_or_insert_with(HashMap::new);
    if loaded.is_valid() {
        last.insert(path.to_path_buf(), loaded.settings.clone());
        return loaded.settings;
    }
    // 一个命令里会多次加载，同一份无效的文件只说一遍
    static WARNED: std::sync::Once = std::sync::Once::new();
    WARNED.call_once(|| {
        let first = loaded
            .diagnostics
            .first()
            .map(|d| d.message.clone())
            .unwrap_or_default();
        tracing::warn!(
            path = %path.display(),
            problems = loaded.diagnostics.len(),
            first = %first,
            "settings.json 无效，沿用上一次读到的有效设置（没有则用内置默认）"
        );
    });
    last.get(path).cloned().unwrap_or(loaded.settings)
}

/// 解析默认配置文件路径：settings.json 的 `default_config` 优先（`~` 展开），
/// 未指定/指定失效时回退 `~/.aproxy/config.toml`。
pub fn default_config_path() -> PathBuf {
    default_config_path_in(&load())
}

/// 同上，Settings 已加载时直接解析（避免重复读文件）
pub fn default_config_path_in(settings: &Settings) -> PathBuf {
    default_config_path_for(settings, &crate::config::config_dir())
}

/// 同上，未指定默认配置时回退到给定主目录下的 `config.toml`（见
/// `default_config_dirs_in` 为什么要由调用方给出主目录）。
pub fn default_config_path_for(settings: &Settings, home: &Path) -> PathBuf {
    settings
        .default_config
        .as_deref()
        .map(expand_path)
        .unwrap_or_else(|| home.join("config.toml"))
}

/// 把一组改动写进本 home 的 settings.json（见 [`edit::apply`]）。失败时 Err 是给用户看的
/// 完整说明。
pub fn update(changes: &[edit::Change]) -> Result<crate::config::edit::Applied, String> {
    let path = settings_path();
    edit::apply(&path, &crate::config::config_dir(), changes)
        .map_err(|e| describe_edit_error(&e, &path))
}

/// 写 settings.json 失败的原因，写成给用户看的话。
pub fn describe_edit_error(error: &crate::config::edit::EditError, path: &Path) -> String {
    use crate::config::edit::EditError;
    let list = |diagnostics: &[Diagnostic]| {
        diagnostics
            .iter()
            .map(|d| format!("\n  {}", crate::ops::start::render_diagnostic(d)))
            .collect::<String>()
    };
    match error {
        EditError::BadKey(key) => format!(
            "键路径写法不对: {key}（点号分隔，例如 watchdog.enabled、aliases.work、defaults.extra_headers.\"x-foo\"）"
        ),
        EditError::Syntax(diagnostics) => format!(
            "settings.json 现在读不懂，没有修改（请先手工修复或删除 {}）:{}",
            path.display(),
            list(diagnostics)
        ),
        EditError::Invalid(diagnostics) => {
            format!(
                "这次修改会让 settings.json 无效，没有保存:{}",
                list(diagnostics)
            )
        }
        EditError::Conflict => format!(
            "settings.json 一直在被别的程序修改，没有保存（关掉正在编辑它的程序后重试）: {}",
            path.display()
        ),
        EditError::LockBusy => {
            "另一个 aproxy 正在写配置，等了 10 秒仍未结束，没有保存（稍后重试）".to_string()
        }
        EditError::Io(e) => format!("保存 settings.json 失败: {e}"),
    }
}

/// 解析别名 → 配置文件路径；无此别名返回 None。
pub fn resolve_alias(name: &str) -> Option<String> {
    load().aliases.get(name).cloned()
}

/// 别名参数路径的展开与归一：`~` 开头展开为用户目录（C1 的用户目录，见 [`aproxy_ipc::home::user_dir`]），
/// 再转绝对路径（不用 canonicalize，避免 Windows \\?\ verbatim 前缀混进 settings）。
pub fn expand_path(path: &str) -> PathBuf {
    let p = if path == "~" || path.starts_with("~/") || path.starts_with("~\\") {
        if let Some(home) = aproxy_ipc::home::user_dir() {
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

/// 一个名字不能当别名的原因（一句英文，给诊断用）；可以用时为 None。规则同
/// [`validate_alias_name`]。
pub fn alias_name_problem(name: &str) -> Option<&'static str> {
    if name.trim().is_empty() {
        return Some("it is empty");
    }
    if name.parse::<u16>().is_ok() {
        return Some("a number is read as a port");
    }
    match name.to_ascii_lowercase().as_str() {
        "all" | "idle" => Some("it is a reserved target of stop and status"),
        "default" | "defult" => Some("it is reserved for the default config"),
        _ => None,
    }
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

/// 固定参与检查的两个默认配置目录（始终在列表中，用户重复添加也静默去重）：
/// 主目录本身与其下的 `configs/`。
pub fn default_config_dirs() -> Vec<String> {
    default_config_dirs_in(&crate::config::config_dir())
}

/// 同上，主目录由调用方给出——doctor/find 按「被检查的 settings.json 所在目录」
/// 推导主目录，而不是读进程环境的 APROXY_HOME：两者在生产中一致，但测试与
/// 「检查另一个主目录」时不一致，读进程环境会扫到开发机真实的 ~/.aproxy/。
pub fn default_config_dirs_in(home: &Path) -> Vec<String> {
    vec![
        home.display().to_string(),
        home.join("configs").display().to_string(),
    ]
}

/// 生效的配置目录列表：用户配置的 + 两个默认目录，展开路径并去重
/// （大小写/分隔符归一后比较，Windows 文件系统大小写不敏感）。
pub fn effective_config_dirs(settings: &Settings) -> Vec<PathBuf> {
    effective_config_dirs_in(settings, &crate::config::config_dir())
}

/// 同上，默认目录取自给定主目录（见 `default_config_dirs_in`）。
pub fn effective_config_dirs_in(settings: &Settings, home: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for raw in settings
        .config_dirs
        .iter()
        .chain(default_config_dirs_in(home).iter())
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

/// settings.json 的 error 级问题清单（空 = 无问题），每条一行文字：文件内容的诊断，以及
/// 指向不存在的文件的别名。doctor 汇总输出用。
pub fn check_settings_errors_in(path: &Path) -> Vec<String> {
    let loaded = read(path);
    let mut errors: Vec<String> = loaded
        .diagnostics
        .iter()
        .filter(|d| d.is_error())
        .map(|d| match &d.key {
            Some(key) => format!("settings.json: {key}: {} [{}]", d.message, d.code),
            None => format!("settings.json: {} [{}]", d.message, d.code),
        })
        .collect();
    errors.extend(missing_alias_targets(&loaded.settings));
    errors
}

/// 指向不存在的文件的别名，每个一行文字。这是现场的状态（文件可以随时被移走），不是
/// settings.json 写错了，所以不算进它的诊断。
pub fn missing_alias_targets(settings: &Settings) -> Vec<String> {
    let mut names: Vec<&String> = settings.aliases.keys().collect();
    names.sort();
    names
        .into_iter()
        .filter(|name| !expand_path(&settings.aliases[*name]).is_file())
        .map(|name| {
            format!(
                "别名 \"{name}\" 指向的配置文件不存在: {}（start/stop 该别名会失败）",
                settings.aliases[name]
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_missing_returns_default() {
        let dir = tempfile::tempdir().unwrap();
        let s = load_from(&settings_path_in(dir.path()));
        assert!(s.aliases.is_empty());
    }

    #[test]
    fn expand_path_tilde_and_absolute() {
        let home = aproxy_ipc::home::user_dir().unwrap();
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
    fn effective_config_dirs_dedupes_and_appends_defaults() {
        // 空配置 → 两个默认目录
        let s = Settings::default();
        let dirs = effective_config_dirs(&s);
        assert_eq!(dirs.len(), 2);
        // 重复添加默认目录 → 静默去重不报错；分隔符变体也去重
        let mut s = Settings::default();
        let root = crate::config::config_dir().display().to_string();
        s.config_dirs = vec![root.clone(), root.replace('\\', "/")];
        let dirs = effective_config_dirs(&s);
        assert_eq!(dirs.len(), 2, "重复/变体写法应去重: {dirs:?}");
    }

    // 大小写变体去重只在 Windows 成立（文件系统大小写不敏感，path_match_key
    // 才做 lowercase 归一）；Linux 上 /ROOT 与 /root 是不同目录，去重反而错误
    #[cfg(windows)]
    #[test]
    fn effective_config_dirs_dedupes_case_variants_on_windows() {
        let mut s = Settings::default();
        let root = crate::config::config_dir().display().to_string();
        s.config_dirs = vec![root.to_uppercase()];
        let dirs = effective_config_dirs(&s);
        assert_eq!(
            dirs.len(),
            2,
            "大小写变体也应去重（Windows 语义）: {dirs:?}"
        );
    }

    #[test]
    fn missing_file_is_defaults_and_valid() {
        let dir = tempfile::tempdir().unwrap();
        let loaded = read(&settings_path_in(dir.path()));
        assert!(loaded.is_valid());
        assert_eq!(loaded.settings, Settings::default());
        assert_eq!(
            load_from(&settings_path_in(dir.path())),
            Settings::default()
        );
    }

    #[test]
    fn builtin_defaults() {
        let s = Settings::default();
        assert_eq!(s.language, "system");
        assert_eq!(s.defaults, FileConfig::default());
        assert!(s.watchdog);
        assert_eq!(
            (
                s.log_rotate_mb,
                s.idle_timeout_secs,
                s.watchdog_heartbeat_secs
            ),
            (8, 1800, 30)
        );
        assert!(s.skill_auto_update);
    }

    /// 无效的文件：`read` 给内置默认与诊断；`load_from` 沿用这个进程上一次读到的有效设置。
    #[test]
    fn an_invalid_file_falls_back_to_the_last_valid_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = settings_path_in(dir.path());
        // 从没读到过有效的：内置默认
        std::fs::write(&path, r#"{"watchdog": false}"#).unwrap();
        let loaded = read(&path);
        assert!(!loaded.is_valid());
        assert_eq!(loaded.settings, Settings::default());
        assert_eq!(load_from(&path), Settings::default());

        std::fs::write(
            &path,
            r#"{"watchdog": {"enabled": false, "heartbeat_secs": 7}, "aliases": {"w": "w.toml"}}"#,
        )
        .unwrap();
        let valid = load_from(&path);
        assert!(!valid.watchdog);
        assert_eq!(valid.watchdog_heartbeat_secs, 7);

        // 改坏之后：沿用上一次的
        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(load_from(&path), valid);
        // 另一个路径不受它影响
        let other = tempfile::tempdir().unwrap();
        let other_path = settings_path_in(other.path());
        std::fs::write(&other_path, "{ not json").unwrap();
        assert_eq!(load_from(&other_path), Settings::default());
    }

    #[test]
    fn check_reports_diagnostics_and_missing_alias_targets() {
        let dir = tempfile::tempdir().unwrap();
        let path = settings_path_in(dir.path());
        assert!(
            check_settings_errors_in(&path).is_empty(),
            "没有文件不是错误"
        );

        let present = dir.path().join("present.toml");
        std::fs::write(&present, "").unwrap();
        let json = serde_json::json!({
            "aliases": {"ok": present.display().to_string(), "gone": dir.path().join("gone.toml").display().to_string()}
        });
        std::fs::write(&path, json.to_string()).unwrap();
        let errors = check_settings_errors_in(&path);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("\"gone\"") && errors[0].contains("不存在"));

        std::fs::write(&path, r#"{"nope": 1, "language": "fr"}"#).unwrap();
        let errors = check_settings_errors_in(&path);
        assert_eq!(errors.len(), 2, "{errors:?}");
        assert!(
            errors
                .iter()
                .any(|e| e.contains("nope") && e.contains("[config_unknown_key]"))
        );
    }

    #[test]
    fn alias_name_problems_match_the_validator() {
        for name in [
            "", " ", "12345", "all", "IDLE", "default", "Defult", "work", "my-relay",
        ] {
            assert_eq!(
                alias_name_problem(name).is_some(),
                validate_alias_name(name).is_err(),
                "{name:?}"
            );
        }
    }
}
