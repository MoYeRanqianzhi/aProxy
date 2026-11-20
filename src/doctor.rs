//! `aproxy doctor`：用与启动相同的解析管线逐文件体检。把跨文件的冲突附在各自的配置上，
//! 让读者从一份配置就能看到它的问题，包括哪些问题会阻止启动。

use crate::config::{self, Config, resolve::Defaults};
use crate::fsx::{Access, access_of};
use crate::ops::enumerate::{self, GcPaused};
use crate::settings::{self, Settings};
use aproxy_ipc::diag::{Diagnostic, Level, has_errors};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io;
use std::path::Path;

/// 被检查的一份文件；没有诊断也列出，便于确认检查范围。
#[derive(Debug, serde::Serialize)]
pub struct FileReport {
    /// 展示路径；运行记录沿用清点结果中相对于主目录的路径。
    pub file: String,
    /// 区分设置、配置、主目录与记录，供显示端选择修复提示。
    pub kind: &'static str,
    /// 保留解析器给出的键和位置，也收录只有跨文件检查才能发现的问题。
    pub diagnostics: Vec<Diagnostic>,
}

impl FileReport {
    /// 先登记检查对象，没有问题的文件也有一个空的诊断列表。
    fn new(file: String, kind: &'static str) -> Self {
        Self {
            file,
            kind,
            diagnostics: Vec::new(),
        }
    }

    /// 体检自己的诊断指向当前对象，没有解析器提供的源码行列。
    fn push(
        &mut self,
        code: &'static str,
        severity: Level,
        key: Option<&str>,
        message: impl Into<String>,
        details: Value,
    ) {
        self.diagnostics.push(Diagnostic {
            code,
            severity,
            file: self.file.clone(),
            key: key.map(str::to_owned),
            line: None,
            column: None,
            message: message.into(),
            details: match details {
                Value::Object(obj) => obj,
                _ => Default::default(),
            },
        });
    }
}

/// 一次只读体检的完整结果，不另外缓存计数，以免与文件里的诊断不一致。
#[derive(Debug)]
pub struct Report {
    /// 按展示顺序保存：设置、按路径排序的配置、主目录、读不懂的记录。
    pub files: Vec<FileReport>,
    /// 日志清理是否受阻与状态命令采用同一份清点结果。
    pub gc_paused: Option<GcPaused>,
}

impl Report {
    /// 所有文件按同一个级别词表汇总。
    fn count(&self, level: Level) -> usize {
        self.files
            .iter()
            .flat_map(|f| &f.diagnostics)
            .filter(|d| d.severity == level)
            .count()
    }
    /// 会阻断启动的诊断数，决定体检退出码。
    pub fn errors(&self) -> usize {
        self.count(Level::Error)
    }
    /// 需要用户确认、但不会阻断启动的问题数。
    pub fn warnings(&self) -> usize {
        self.count(Level::Warning)
    }
    /// 单列提示数，不把正常的配置行为算作告警。
    pub fn infos(&self) -> usize {
        self.count(Level::Info)
    }
}

/// 执行完整体检。主目录取 settings.json 所在目录，而不是进程环境的 APROXY_HOME：默认配置
/// （`config.toml`）与两个默认扫描目录都由它推导。生产中两者一致；检查另一个
/// 主目录（测试的临时目录）时，读进程环境会扫到开发机真实的 ~/.aproxy/。
pub async fn run(settings_path: &Path) -> Report {
    let home = settings_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(config::config_dir);
    let loaded = settings::read(settings_path);
    let defaults = Defaults::from_settings(&loaded.settings, settings_path);
    let mut files = vec![settings_report(
        settings_path,
        &home,
        &loaded.settings,
        loaded.diagnostics,
    )];
    files.extend(config_reports(&home, &loaded.settings, &defaults));
    files.push(home_report(&home));

    let found = enumerate::enumerate_in(&home).await;
    for item in found.unreadable {
        let mut file = FileReport::new(item.file, "record");
        let code = match item.reason.code.as_str() {
            enumerate::CODE_RECORD_SCHEMA_TOO_NEW => enumerate::CODE_RECORD_SCHEMA_TOO_NEW,
            _ => enumerate::CODE_RECORD_UNREADABLE,
        };
        file.push(
            code,
            Level::Warning,
            None,
            "record cannot be read; it is left in place",
            item.reason.details,
        );
        files.push(file);
    }
    Report {
        files,
        gc_paused: found.gc_paused,
    }
}

/// 保留严格解析的全部诊断，再说明语法合法但容易忽略的共享头、看护策略和目录问题。
fn settings_report(
    path: &Path,
    home: &Path,
    settings: &Settings,
    diagnostics: Vec<Diagnostic>,
) -> FileReport {
    let mut file = FileReport {
        file: path.display().to_string(),
        kind: "settings",
        diagnostics,
    };
    for (table, headers) in [
        ("extra_headers", &settings.defaults.extra_headers),
        ("override_headers", &settings.defaults.override_headers),
    ] {
        for name in headers
            .iter()
            .flat_map(|h| h.keys())
            .filter(|n| !aproxy_ipc::redact::is_secret_header(n))
        {
            let key = format!("defaults.{table}.{}", config::quote_key(name));
            let message = format!(
                "header {name} is sent to every target; if it carries a credential, move it into the configs that need it",
            );
            file.push(
                "default_header_shared",
                Level::Info,
                Some(&key),
                message,
                json!({}),
            );
        }
    }
    if !has_errors(&file.diagnostics) {
        let n = settings.watchdog_heartbeat_secs;
        if n > 600 {
            // 两个设置都能写满 u64，放宽后再计算延迟。
            let delay = u128::from(n) * (u128::from(settings.watchdog_stale_after_cycles) + 1);
            let message = format!(
                "watchdog.heartbeat_secs is {n}: a hung instance is detected only after about {delay} seconds",
            );
            file.push(
                "watchdog_slow_hang_detection",
                Level::Warning,
                Some("watchdog.heartbeat_secs"),
                message,
                json!({"heartbeat_secs": n}),
            );
        }
        if settings.watchdog_max_restarts == 0 {
            file.push(
                "watchdog_restarts_disabled",
                Level::Warning,
                Some("watchdog.max_restarts"),
                "watchdog.max_restarts is 0: crashed or hung instances are not restarted",
                json!({}),
            );
        }
    }
    let builtins = settings::default_config_dirs_in(home);
    for raw in &settings.config_dirs {
        let dir = settings::expand_path(raw);
        let path = dir.display().to_string();
        // 默认目录不存在（全新安装）不算问题；用户自加目录不存在提示。
        if !dir.is_dir()
            && !builtins
                .iter()
                .any(|d| settings::path_match_key(d) == settings::path_match_key(&path))
        {
            file.push(
                "config_dir_missing",
                Level::Warning,
                Some("config_dirs"),
                format!("config directory {path} does not exist"),
                json!({"dir": path}),
            );
        }
    }
    file
}

/// 主目录的访问权限告警：里面有明文密钥（配置、settings.json、期望状态记录）、日志与落盘的
/// 请求内容，别的用户不该读得到。属于当前用户的 home，aproxy 下一次往里写东西时会自己
/// 收紧；不属于的它不改，只能由属主处理。体检只读，这里不动权限。
fn home_report(home: &Path) -> FileReport {
    let mut file = FileReport::new(home.display().to_string(), "home");
    if let Ok(Access::TooOpen { owned, who }) = access_of(home) {
        let remedy = if owned {
            "aproxy tightens it on its next write"
        } else {
            "it belongs to another user, so aproxy leaves it alone"
        };
        file.push(
            "file_permissions_too_open",
            Level::Warning,
            None,
            format!("the group or others can access the home ({who}); {remedy}"),
            json!({}),
        );
    }
    file
}

/// 先按路径归并默认配置、别名和非递归目录扫描的结果，再按展示顺序解析每个文件一次。
fn config_reports(home: &Path, settings: &Settings, defaults: &Defaults) -> Vec<FileReport> {
    let default = settings::default_config_path_for(settings, home);
    let key = |p: &Path| settings::path_match_key(&p.display().to_string());
    let mut targets = HashMap::from([(key(&default), (default.clone(), Vec::new()))]);
    let mut aliases: Vec<_> = settings.aliases.iter().collect();
    aliases.sort_by_key(|(name, _)| *name);
    for (name, raw) in aliases {
        let path = settings::expand_path(raw);
        targets
            .entry(key(&path))
            .or_insert_with(|| (path, Vec::new()))
            .1
            .push(name.as_str());
    }
    for dir in settings::effective_config_dirs_in(settings, home) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "toml") {
                targets
                    .entry(key(&path))
                    .or_insert_with(|| (path, Vec::new()));
            }
        }
    }
    let mut targets: Vec<_> = targets.into_values().collect();
    targets.sort_by_key(|(p, _)| p.display().to_string());
    let mut files = Vec::new();
    let mut ports: HashMap<u16, Vec<usize>> = HashMap::new();
    for (path, aliases) in targets {
        // 启动允许没有文件时只用默认层；体检必须区分缺失的指定文件与存在的空文件。
        let resolved = std::fs::metadata(&path)
            .and_then(|_| config::resolve(&path, &config::Overrides::default(), defaults));
        let mut file = FileReport::new(path.display().to_string(), "config");
        match resolved {
            Err(e) => {
                if path == default && settings.default_config.is_none() && aliases.is_empty() {
                    continue;
                }
                let (reason, mut message) = if e.kind() == io::ErrorKind::NotFound {
                    ("not_found", "config file does not exist".to_owned())
                } else {
                    ("unreadable", format!("config file cannot be read: {e}"))
                };
                if !aliases.is_empty() {
                    message.push_str(&format!(" (alias {})", aliases.join(", ")));
                }
                file.push(
                    "config_file_unreadable",
                    Level::Warning,
                    None,
                    message,
                    json!({"reason": reason}),
                );
            }
            Ok(resolved) => {
                file.diagnostics = resolved.diagnostics;
                if !has_errors(&file.diagnostics) {
                    let cfg = resolved.config;
                    if cfg.forward_only_enabled() {
                        use config::file::Stage;
                        // required 的请求层不可用会直接终止请求，不会无声地改走直通。
                        if cfg.formats.iter().any(|f| {
                            f.has_stage(Stage::Decide)
                                || (!f.required && f.has_stage(Stage::Request))
                        }) {
                            let code = aproxy_ipc::code::lookup("forward_default_unguarded")
                                .expect("registered diagnostic");
                            file.push(
                                code.name,
                                Level::Warning,
                                Some("delivery"),
                                code.summary,
                                json!({}),
                            );
                        }
                        let secrets = cfg.known_secrets();
                        for (index, f) in cfg
                            .formats
                            .iter()
                            .enumerate()
                            .filter(|(_, f)| f.has_stage(Stage::Response))
                        {
                            let code = aproxy_ipc::code::lookup("forward_response_buffered")
                                .expect("registered diagnostic");
                            file.push(
                                code.name,
                                Level::Info,
                                Some(&format!("format[{index}].stages")),
                                code.summary,
                                json!({"format":secrets.replace(&f.name)}),
                            );
                        }
                    }
                    let addr = &cfg.listen_addr;
                    if !cfg.listen_is_loopback() {
                        let suffix = if cfg.injects_credentials() {
                            ", and the API key it injects"
                        } else {
                            ""
                        };
                        let message = format!(
                            "listen_addr {addr} is not a loopback address: any host that can reach this port can use the proxy{suffix}",
                        );
                        file.push(
                            "listen_not_loopback",
                            Level::Warning,
                            Some("listen_addr"),
                            message,
                            json!({"listen_addr": addr}),
                        );
                    }
                    if let Ok(port) = crate::daemon::port_of(addr).parse::<u16>()
                        && port != 0
                    {
                        ports.entry(port).or_default().push(files.len());
                    }
                }
                if let Ok(Access::TooOpen { who, .. }) = access_of(&path) {
                    file.push(
                        "file_permissions_too_open",
                        Level::Warning,
                        None,
                        format!("the group or others can access this file ({who})"),
                        json!({}),
                    );
                }
            }
        }
        files.push(file);
    }
    // 同一端口的文件单独启动都合法；只提示不能同时启动的其余文件，沿用路径排序。
    for (port, group) in ports.into_iter().filter(|(_, group)| group.len() > 1) {
        for &i in &group {
            let others: Vec<_> = group
                .iter()
                .filter(|&&j| j != i)
                .map(|&j| files[j].file.clone())
                .collect();
            let message = format!(
                "port {port} is also used by {}; these configs cannot run at the same time",
                others.join(", ")
            );
            files[i].push(
                "port_shared",
                Level::Warning,
                Some("listen_addr"),
                message,
                json!({"port": port, "files": others}),
            );
        }
    }
    files
}

/// 读 toml → Config（normalized）。无法解析（语法错/字段错）返回 None——
/// 由调用方生成对应提示。
pub fn parse_config_file(path: &Path) -> Option<Config> {
    let content = std::fs::read_to_string(path).ok()?;
    let (_, diagnostics) = config::file::parse(&content, path);
    if aproxy_ipc::diag::has_errors(&diagnostics) {
        return None;
    }
    let resolved = config::resolve::resolve_text(
        Some(&content),
        path,
        &config::Overrides::default(),
        &config::resolve::Defaults::default(),
        false,
    );
    Some(resolved.config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn write_cfg(dir: &Path, name: &str, listen: &str, base: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(
            &p,
            format!("base_url = \"{base}\"\nlisten_addr = \"{listen}\"\n"),
        )
        .unwrap();
        p
    }

    fn settings_path_in_tmp(dir: &Path) -> PathBuf {
        dir.join("settings.json")
    }

    /// 把设置写进待查的临时主目录，路径交给 JSON 编码而不是手拼转义。
    fn write_settings(home: &Path, value: Value) -> PathBuf {
        let path = settings_path_in_tmp(home);
        std::fs::write(&path, value.to_string()).unwrap();
        path
    }

    /// 查找恰好一份报告，兼顾 Windows 路径不同的分隔符与大小写。
    fn one_file<'a>(report: &'a Report, path: &Path) -> &'a FileReport {
        let key = settings::path_match_key(&path.display().to_string());
        let mut found = report
            .files
            .iter()
            .filter(|f| settings::path_match_key(&f.file) == key);
        let file = found.next().unwrap();
        assert!(found.next().is_none(), "{report:?}");
        file
    }

    /// 默认 forward 才有此风险；只要有 decide 或可选 request 就报告，required request 不算。
    #[tokio::test]
    async fn doctor_warns_forward_default_unguarded() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("config.toml");
        let settings = settings_path_in_tmp(dir.path());
        for (delivery, stages, required, expected) in [
            ("forward", "'request'", true, false),
            ("forward", "'request'", false, true),
            ("forward", "'decide'", true, true),
            ("forward", "'request','decide'", false, true),
            ("retry", "'request','decide'", false, false),
        ] {
            std::fs::write(&cfg,format!("base_url='http://localhost'\ndelivery='{delivery}'\n[[format]]\ncommand='observer'\nstages=[{stages}]\nrequired={required}\n")).unwrap();
            let report = run(&settings).await;
            assert_eq!(report.errors(), 0, "{report:?}");
            let ds: Vec<_> = one_file(&report, &cfg)
                .diagnostics
                .iter()
                .filter(|d| d.code == "forward_default_unguarded")
                .collect();
            assert_eq!(ds.len(), usize::from(expected), "{report:?}");
            if expected {
                assert_eq!(ds[0].severity, Level::Warning);
                assert!(ds[0].details.is_empty());
            }
        }
    }
    /// 每一层整包响应都单独列出，流层不算；诊断里的显示名与程序快照一样替换密钥。
    #[tokio::test]
    async fn doctor_notes_forward_response_buffered() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let settings = settings_path_in_tmp(dir.path());
        for delivery in ["retry", "forward"] {
            std::fs::write(&path,format!("base_url='http://localhost'\napi_key='test-secret'\ndelivery='{delivery}'\n[[format]]\ncommand='pack-test-secret'\nstages=['response']\n[[format]]\ncommand='incremental'\nprocess='persistent'\nstages=['stream']\n[[format]]\ncommand='another'\nstages=['response']\n")).unwrap();
            let report = run(&settings).await;
            assert_eq!(report.errors(), 0, "{report:?}");
            let ds: Vec<_> = one_file(&report, &path)
                .diagnostics
                .iter()
                .filter(|d| d.code == "forward_response_buffered")
                .collect();
            if delivery == "retry" {
                assert!(ds.is_empty());
            } else {
                assert_eq!(ds.len(), 2);
                assert!(ds.iter().all(|d| d.severity == Level::Info));
                assert_eq!(ds[0].details["format"], "pack-<secret>");
                assert_eq!(ds[1].details["format"], "another");
            }
        }
    }

    /// 两个别名指到同端口时各自指出另一份文件；语法错则保留解析器的错误与位置。
    #[tokio::test]
    async fn doctor_reports_alias_conflicts_and_bad_syntax() {
        let dir = tempfile::tempdir().unwrap();
        // 只比较配置里的地址，不在这个端口上启动实例或探测服务。
        let addr = "127.0.0.1:59841";
        let a = write_cfg(dir.path(), "a.toml", addr, "https://a.example.com");
        let b = write_cfg(dir.path(), "b.toml", addr, "https://b.example.com");
        let bad = dir.path().join("bad.toml");
        std::fs::write(&bad, "not = [valid toml").unwrap();
        // default_config 指向本测试的配置：默认配置检查不去读开发机真实的 ~/.aproxy/config.toml。
        let path = write_settings(
            dir.path(),
            json!({"aliases":{"a":a, "b":b, "bad":bad}, "default_config":a}),
        );
        let report = run(&path).await;
        assert_eq!(
            report.errors(),
            one_file(&report, &bad)
                .diagnostics
                .iter()
                .filter(|d| d.is_error())
                .count()
        );
        assert!(
            one_file(&report, &bad)
                .diagnostics
                .iter()
                .any(|d| d.code == "config_syntax" && d.is_error())
        );
        for (path, other) in [(&a, &b), (&b, &a)] {
            let d = one_file(&report, path)
                .diagnostics
                .iter()
                .find(|d| d.code == "port_shared")
                .unwrap();
            assert_eq!(d.severity, Level::Warning);
            assert_eq!(d.details["port"], 59841);
            assert_eq!(d.details["files"], json!([one_file(&report, other).file]));
        }
    }

    /// 目录扫描不能重复列出默认或别名配置，也不能跳过没被引用的无效配置。
    #[tokio::test]
    async fn doctor_unaliased_scan_excludes_aliased_and_defaults() {
        let root = tempfile::tempdir().unwrap();
        // 目录 A：被别名覆盖的 x.toml；目录 B（用户自加）：孤儿 y.toml。
        let dir_a = root.path().join("configs");
        let dir_b = root.path().join("b");
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::create_dir_all(&dir_b).unwrap();
        let x = write_cfg(&dir_a, "x.toml", "127.0.0.1:0", "https://x.example.com");
        let y = write_cfg(&dir_b, "y.toml", "127.0.0.1:0", "https://y.example.com");
        // 缺 base_url → validate 必然失败（doctor 的校验覆盖项）。
        let bad = write_cfg(&dir_b, "badval.toml", "127.0.0.1:0", "");
        let report = run(&write_settings(
            root.path(),
            json!({"aliases":{"x":x}, "default_config":x, "config_dirs":[dir_b]}),
        ))
        .await;
        assert!(!has_errors(&one_file(&report, &x).diagnostics));
        assert!(!has_errors(&one_file(&report, &y).diagnostics));
        assert!(has_errors(&one_file(&report, &bad).diagnostics));
        assert_eq!(
            report.files.iter().filter(|f| f.kind == "config").count(),
            3
        );
    }

    /// 主目录取被检查的 settings.json 所在目录：其下未设 default_config 时的
    /// 默认配置（config.toml）由此推导——非回环监听告警应指向临时目录里的
    /// 这份，而不是进程环境 APROXY_HOME（开发机上即真实 ~/.aproxy）里的那份。
    #[tokio::test]
    async fn doctor_home_follows_settings_location_not_process_env() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = write_cfg(
            dir.path(),
            "config.toml",
            "192.168.1.10:0",
            "https://h.example.com",
        );
        let report = run(&write_settings(dir.path(), json!({}))).await;
        assert!(
            one_file(&report, &cfg)
                .diagnostics
                .iter()
                .any(|d| d.code == "listen_not_loopback")
        );
        assert_eq!(one_file(&report, dir.path()).kind, "home");
        assert_eq!(report.files.len(), 3);
    }

    /// 别的用户能访问的主目录给一条告警；体检只读，不改权限。
    #[cfg(unix)]
    #[test]
    fn doctor_warns_when_others_can_access_the_home() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        // 临时目录按 umask 建出来，多半是 0755：先收成只让属主访问。
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(home_report(dir.path()).diagnostics.is_empty());
        assert!(
            home_report(&dir.path().join("missing"))
                .diagnostics
                .is_empty()
        );
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o750)).unwrap();
        let f = home_report(dir.path());
        assert_eq!(f.diagnostics.len(), 1);
        let d = &f.diagnostics[0];
        assert_eq!(
            (d.code, d.severity),
            ("file_permissions_too_open", Level::Warning)
        );
        assert!(
            d.message.contains("mode 0750") && d.message.contains("tightens it on its next write")
        );
        let mode = std::fs::metadata(dir.path()).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o750, "体检不改权限");
    }

    /// 空主目录不用创建默认配置才能通过；有合规的别名配置时也不报问题。
    #[tokio::test]
    async fn doctor_clean_when_all_good() {
        let dir = tempfile::tempdir().unwrap();
        // 「一切正常」包括主目录只让属主访问：临时目录在有的机器上还给了别的账户权限。
        crate::fsx::secure_home_in(dir.path()).unwrap();
        let path = settings_path_in_tmp(dir.path());
        let empty = run(&path).await;
        assert_eq!(empty.files.len(), 2);
        assert!(empty.files.iter().all(|f| f.diagnostics.is_empty()));
        let a = dir.path().join("a.toml");
        crate::fsx::write_atomic_private(
            &a,
            b"base_url = 'https://a.example.com'\nlisten_addr = '127.0.0.1:0'\n",
        )
        .unwrap();
        write_settings(dir.path(), json!({"aliases":{"a":a}, "default_config":a}));
        let report = run(&path).await;
        assert_eq!(report.files.len(), 3);
        assert_eq!(
            (report.errors(), report.warnings(), report.infos()),
            (0, 0, 0),
            "{report:?}"
        );
    }

    /// 别名配置监听 0.0.0.0 且配了 api_key → warning（措辞升级）；回环的
    /// 别名配置不报。都是 warning 级：对外开放可能是用户的意图，不阻断。
    #[tokio::test]
    async fn doctor_warns_non_loopback_listen() {
        let dir = tempfile::tempdir().unwrap();
        let open = dir.path().join("open.toml");
        std::fs::write(
            &open,
            "base_url = 'https://a.example.com'\nlisten_addr = '0.0.0.0:0'\napi_key = 'sk-test'\n",
        )
        .unwrap();
        let local = write_cfg(
            dir.path(),
            "local.toml",
            "127.0.0.1:0",
            "https://b.example.com",
        );
        // default_config 指向回环配置：默认配置检查只读这份临时文件。
        let report = run(&write_settings(
            dir.path(),
            json!({"aliases":{"open":open,"local":local},"default_config":local}),
        ))
        .await;
        let exposed: Vec<_> = report
            .files
            .iter()
            .flat_map(|f| &f.diagnostics)
            .filter(|d| d.code == "listen_not_loopback")
            .collect();
        assert_eq!(exposed.len(), 1);
        let d = exposed[0];
        assert_eq!(d.file, one_file(&report, &open).file);
        assert_eq!(d.severity, Level::Warning);
        assert!(d.message.contains("and the API key it injects"));
        assert!(!format!("{report:?}").contains("sk-test"));
        assert_eq!(report.errors(), 0);
    }

    /// 单实例用户只有默认配置也应发现暴露；被别名引用后仍只查一次。
    #[tokio::test]
    async fn doctor_checks_default_config_exposure_once() {
        let dir = tempfile::tempdir().unwrap();
        let main = write_cfg(
            dir.path(),
            "main.toml",
            "192.168.1.10:0",
            "https://m.example.com",
        );
        for aliases in [json!({}), json!({"m":main})] {
            let report = run(&write_settings(
                dir.path(),
                json!({"default_config":main,"aliases":aliases}),
            ))
            .await;
            let exposed: Vec<_> = one_file(&report, &main)
                .diagnostics
                .iter()
                .filter(|d| d.code == "listen_not_loopback")
                .collect();
            assert_eq!(exposed.len(), 1);
            assert!(exposed[0].message.contains("192.168.1.10:0"));
            assert!(!exposed[0].message.contains("API key"));
        }
    }

    /// 0 周期 / 0 容忍由解析器拒绝；max_restarts=0 与过大周期合法，体检提示后果。
    #[tokio::test]
    async fn doctor_watchdog_field_bounds() {
        let dir = tempfile::tempdir().unwrap();
        for field in ["heartbeat_secs", "stale_after_cycles"] {
            let path = write_settings(dir.path(), json!({"watchdog":{(field):0}}));
            let diagnostics = settings::read(&path).diagnostics;
            assert!(diagnostics.iter().any(|d| d.is_error()
                && d.code == "config_invalid_value"
                && d.key == Some(format!("watchdog.{field}"))));
        }
        let path = write_settings(
            dir.path(),
            json!({"watchdog":{"heartbeat_secs":3600, "stale_after_cycles":2, "max_restarts":0}}),
        );
        let report = run(&path).await;
        let diags = &one_file(&report, &path).diagnostics;
        assert_eq!(diags.len(), 2);
        assert!(diags.iter().all(|d| d.severity == Level::Warning));
        assert!(
            diags
                .iter()
                .any(|d| d.code == "watchdog_slow_hang_detection"
                    && d.message.contains("10800 seconds")
                    && d.details["heartbeat_secs"] == 3600)
        );
        assert!(diags.iter().any(|d| d.code == "watchdog_restarts_disabled"
            && d.key.as_deref() == Some("watchdog.max_restarts")));
    }

    /// 共享头只报非密钥头名；缺目录与多个别名指到同一个缺文件，分别归到正确的报告里。
    #[tokio::test]
    async fn doctor_shared_headers_missing_dirs_and_dangling_aliases() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing");
        let gone = dir.path().join("gone.toml");
        let path = write_settings(
            dir.path(),
            json!({
                "defaults": {
                    "base_url": "https://a.example.com",
                    "extra_headers": {"x.team": "blue", "authorization": "secret-value"},
                    "override_headers": {"x.team": "green", "x-api-key": "secret-value"}
                },
                "config_dirs":[missing], "aliases":{"z":gone,"a":gone}
            }),
        );
        let report = run(&path).await;
        let diags = &one_file(&report, &path).diagnostics;
        assert_eq!(report.infos(), 2, "{report:?}");
        for table in ["extra_headers", "override_headers"] {
            assert!(diags.iter().any(|d| d.code == "default_header_shared"
                && d.severity == Level::Info
                && d.key == Some(format!("defaults.{table}.\"x.team\""))));
        }
        assert!(!format!("{report:?}").contains("secret-value"));
        assert!(diags.iter().any(|d| d.code == "config_dir_missing"
            && Path::new(d.details["dir"].as_str().unwrap()) == missing));
        let d = &one_file(&report, &gone).diagnostics[0];
        assert_eq!(d.code, "config_file_unreadable");
        assert_eq!(d.details["reason"], "not_found");
        assert!(d.message.ends_with("(alias a, z)"));
    }

    #[test]
    fn find_discovers_filters_and_dedupes() {
        let root = tempfile::tempdir().unwrap();
        let cfgs = root.path().join("cfgs");
        std::fs::create_dir_all(&cfgs).unwrap();
        let a = write_cfg(
            &cfgs,
            "alpha.toml",
            "127.0.0.1:59845",
            "https://alpha.example.com",
        );
        let b = write_cfg(
            &cfgs,
            "beta.toml",
            "127.0.0.1:59846",
            "https://beta.example.com",
        );
        std::fs::write(
            settings_path_in_tmp(root.path()),
            format!(
                r#"{{"aliases": {{"alpha": "{}"}}, "config_dirs": ["{}"]}}"#,
                a.display()
                    .to_string()
                    .replace(std::path::MAIN_SEPARATOR, "/"),
                cfgs.display()
                    .to_string()
                    .replace(std::path::MAIN_SEPARATOR, "/")
            ),
        )
        .unwrap();
        let settings = settings::load_from(&settings_path_in_tmp(root.path()));

        // 主目录取本测试的临时目录（默认扫描根随之落在这里，不碰开发机真实的
        // ~/.aproxy/）；只统计用户自加目录内的发现
        let all: Vec<_> = crate::find::discover_in(&settings, root.path())
            .into_iter()
            .filter(|d| d.path.starts_with(&cfgs))
            .collect();
        assert_eq!(all.len(), 2, "用户目录内应恰好发现两个: {all:?}");
        // --aliased：只有 alpha
        let aliased: Vec<_> = all.iter().filter(|d| !d.aliases.is_empty()).collect();
        assert_eq!(aliased.len(), 1);
        assert_eq!(aliased[0].aliases, vec!["alpha"]);
        // --unaliased：只有 beta
        let unaliased: Vec<_> = all.iter().filter(|d| d.aliases.is_empty()).collect();
        assert_eq!(unaliased.len(), 1);
        // 关键字过滤：alpha 命中文件名与别名
        assert!(all[0].matches_query("alpha") || all[1].matches_query("alpha"));
        // 端口过滤
        assert!(all.iter().any(|d| d.matches_port(59846)));
        assert!(!all.iter().any(|d| d.matches_port(59999)));
        // b 文件不再被第二次发现（同目录去重）
        assert_eq!(all.iter().filter(|d| d.path == b).count(), 1);
    }
}
