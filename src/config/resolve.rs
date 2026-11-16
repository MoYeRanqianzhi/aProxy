//! 配置的分层解析：启动时的覆盖 > config.toml > settings 的全局默认 > 内置默认。
//!
//! 这是把「一份 config.toml」变成「一个实例实际用的配置」的唯一管线：启动、诊断与展示都走
//! 它，所以它们对同一份文件的看法不会分叉。产出三样：
//! - `config`：运行时配置；
//! - `sources`：每个键的值来自哪一层（头表按头名各记各的）；
//! - `diagnostics`：文件里的问题（见 [`super::file`]）与合并之后的语义问题
//!   （见 [`Config::problems`]）。有 error 级诊断时这份配置不能用来启动。
//!
//! 合并规则：标量与数组整体取最高一层；`extra_headers`、`override_headers` 按头名合并（头名
//! 不分大小写，同名以 config.toml 为准，两张表各自合并）。`listen_addr`、`log_file`、`format`
//! 属于单个实例，不从全局默认继承。

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use aproxy_ipc::diag::{Diagnostic, Level};
use serde::Serialize;
use serde_json::{Map, Value};

use super::file::{self, Delivery, FileConfig, FormatEntry, Process, RetryRule, Stage};
use super::{Config, KeepaliveTrigger, TransformConfig, TransformMode};
use crate::settings::Settings;

/// 启动时在命令行上给的覆盖，优先于文件。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Overrides {
    pub base_url: Option<String>,
    pub listen_addr: Option<String>,
    pub proxy: Option<String>,
    pub api_key: Option<String>,
    pub log_file: Option<String>,
}

/// 一个键的值来自哪一层。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// 启动时的覆盖
    Override,
    /// config.toml
    Toml,
    /// settings.json 的全局默认
    Settings,
    /// 环境变量（只有 `proxy`：没配置代理时 HTTP 客户端读 `HTTPS_PROXY` 等变量）
    Env,
    /// 内置默认
    Default,
}

/// 全局默认这一层：值与它们所在的文件（诊断要指明问题出在哪个文件）。
#[derive(Debug, Clone, Default)]
pub struct Defaults {
    pub file: PathBuf,
    pub values: FileConfig,
    /// 读这一层时就发现的问题
    pub diagnostics: Vec<Diagnostic>,
}

impl Defaults {
    /// 从 settings.json 取全局默认。settings 里与内置默认相同的值不算「来自 settings」：
    /// settings 的这些字段总有值，分不出用户写没写，只能按值判断。
    pub fn from_settings(settings: &Settings, file: &Path) -> Self {
        let mut values = FileConfig::default();
        let mut diagnostics = Vec::new();
        if settings.max_body_mb != super::DEFAULT_MAX_BODY_MB {
            values.max_request_mb = Some(settings.max_body_mb);
        }
        if settings.disk_cache != super::DEFAULT_DISK_CACHE {
            values.disk_cache = Some(settings.disk_cache);
        }
        if settings.forward_only != super::DEFAULT_FORWARD_ONLY {
            values.delivery = Some(if settings.forward_only {
                Delivery::Forward
            } else {
                Delivery::Retry
            });
        }
        if !settings.bounded_retry_paths.is_empty() {
            values.retry.rules = Some(
                settings
                    .bounded_retry_paths
                    .iter()
                    .map(|path| RetryRule {
                        path: Some(path.clone()),
                        status: None,
                        max_attempts: crate::retry::BOUNDED_RETRY_MAX_ATTEMPTS,
                    })
                    .collect(),
            );
        }
        if !settings.allowed_hosts.is_empty() {
            values.allowed_hosts = Some(settings.allowed_hosts.clone());
        }
        if !settings.allowed_origins.is_empty() {
            values.allowed_origins = Some(settings.allowed_origins.clone());
        }
        match KeepaliveTrigger::parse(&settings.keepalive_trigger) {
            Some(trigger) if trigger != super::DEFAULT_KEEPALIVE_TRIGGER => {
                values.keepalive.trigger = Some(trigger)
            }
            Some(_) => {}
            None => diagnostics.push(Diagnostic {
                code: "config_invalid_value",
                severity: Level::Error,
                file: file.display().to_string(),
                key: Some("keepalive_trigger".to_string()),
                line: None,
                column: None,
                message: format!(
                    "keepalive_trigger must be \"accept\", \"body_stream\" or \"any\", not {:?}",
                    settings.keepalive_trigger
                ),
                details: reason_details("not_one_of"),
            }),
        }
        Defaults {
            file: file.to_path_buf(),
            values,
            diagnostics,
        }
    }
}

/// 分层解析的结果。
#[derive(Debug, Clone)]
pub struct Resolved {
    pub config: Config,
    /// 键路径 → 来源。只列有值的键：没设置、也没有内置默认的键（例如 `api_key`）不在里面。
    /// 头表按头名各记一项：`extra_headers.<头名>`。
    pub sources: BTreeMap<String, Source>,
    pub diagnostics: Vec<Diagnostic>,
}

impl Resolved {
    /// 没有 error 级诊断：这份配置可以用来启动
    pub fn is_valid(&self) -> bool {
        !aproxy_ipc::diag::has_errors(&self.diagnostics)
    }

    /// error 级的诊断
    pub fn errors(&self) -> impl Iterator<Item = &Diagnostic> {
        self.diagnostics.iter().filter(|d| d.is_error())
    }
}

/// 解析 `path` 处的配置。文件不存在时当作空文件（只剩覆盖与默认两层）；文件存在却读不出来
/// 是 I/O 错误，交给调用方去说。
pub fn resolve(
    path: &Path,
    overrides: &Overrides,
    defaults: &Defaults,
) -> std::io::Result<Resolved> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e),
    };
    Ok(resolve_text(
        text.as_deref(),
        path,
        overrides,
        defaults,
        env_proxy_is_set(),
    ))
}

/// HTTP 客户端在没配置代理时会读的环境变量里，有没有设了值的。
fn env_proxy_is_set() -> bool {
    [
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
        "ALL_PROXY",
        "all_proxy",
    ]
    .iter()
    .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()))
}

/// 同 [`resolve`]，但文件内容与环境都由参数给出：不读文件系统，也不读环境变量。
/// `text` 为 None 表示文件不存在。
pub fn resolve_text(
    text: Option<&str>,
    path: &Path,
    overrides: &Overrides,
    defaults: &Defaults,
    env_proxy: bool,
) -> Resolved {
    let (f, mut diagnostics) = match text {
        Some(text) => file::parse(text, path),
        None => (FileConfig::default(), Vec::new()),
    };
    diagnostics.extend(defaults.diagnostics.iter().cloned());
    let d = &defaults.values;
    let o = overrides;
    let builtin = Config::default();
    let mut sources = BTreeMap::new();
    let s = &mut sources;

    let base_url = pick(s, "base_url", &o.base_url, &f.base_url, &d.base_url);
    let listen_addr = pick(s, "listen_addr", &o.listen_addr, &f.listen_addr, &None);
    let api_key = pick(s, "api_key", &o.api_key, &f.api_key, &d.api_key);
    let proxy = pick(s, "proxy", &o.proxy, &f.proxy, &d.proxy);
    if proxy.is_none() && env_proxy {
        s.insert("proxy".to_string(), Source::Env);
    }
    let proxy_username = pick(
        s,
        "proxy_username",
        &None,
        &f.proxy_username,
        &d.proxy_username,
    );
    let proxy_password = pick(
        s,
        "proxy_password",
        &None,
        &f.proxy_password,
        &d.proxy_password,
    );
    let log_file = pick(s, "log_file", &o.log_file, &f.log_file, &None);
    let delivery = pick_or(s, "delivery", &f.delivery, &d.delivery, Delivery::Retry);
    let trigger = pick_or(
        s,
        "keepalive.trigger",
        &f.keepalive.trigger,
        &d.keepalive.trigger,
        super::DEFAULT_KEEPALIVE_TRIGGER,
    );
    let heartbeat = pick_or(
        s,
        "keepalive.heartbeat",
        &f.keepalive.heartbeat,
        &d.keepalive.heartbeat,
        super::DEFAULT_KEEPALIVE_HEARTBEAT.to_string(),
    );
    let formats = pick_or(s, "format", &f.format, &None, Vec::new());
    let (request_transform, response_transform, heartbeat_transform) =
        assign_formats(&formats, text, path, &mut diagnostics);

    let config = Config {
        base_url: base_url.unwrap_or_default(),
        listen_addr: listen_addr.unwrap_or_else(|| {
            s.insert("listen_addr".to_string(), Source::Default);
            builtin.listen_addr.clone()
        }),
        api_key,
        extra_headers: merge_headers(s, "extra_headers", &f.extra_headers, &d.extra_headers),
        override_headers: merge_headers(
            s,
            "override_headers",
            &f.override_headers,
            &d.override_headers,
        ),
        keepalive_interval_secs: pick_or(
            s,
            "keepalive.interval_secs",
            &f.keepalive.interval_secs,
            &d.keepalive.interval_secs,
            builtin.keepalive_interval_secs,
        ),
        keepalive_trigger: Some(trigger.as_str().to_string()),
        keepalive_heartbeat: Some(heartbeat),
        proxy,
        proxy_username,
        proxy_password,
        max_retry_backoff_secs: pick_or(
            s,
            "retry.max_backoff_secs",
            &f.retry.max_backoff_secs,
            &d.retry.max_backoff_secs,
            builtin.max_retry_backoff_secs,
        ),
        spool_limit_mb: pick_or(
            s,
            "max_response_mb",
            &f.max_response_mb,
            &d.max_response_mb,
            builtin.spool_limit_mb,
        ),
        connect_timeout_secs: pick_or(
            s,
            "timeouts.connect_secs",
            &f.timeouts.connect_secs,
            &d.timeouts.connect_secs,
            builtin.connect_timeout_secs,
        ),
        first_byte_timeout_secs: pick_or(
            s,
            "timeouts.first_byte_secs",
            &f.timeouts.first_byte_secs,
            &d.timeouts.first_byte_secs,
            builtin.first_byte_timeout_secs,
        ),
        read_timeout_secs: pick_or(
            s,
            "timeouts.read_idle_secs",
            &f.timeouts.read_idle_secs,
            &d.timeouts.read_idle_secs,
            builtin.read_timeout_secs,
        ),
        max_body_mb: Some(pick_or(
            s,
            "max_request_mb",
            &f.max_request_mb,
            &d.max_request_mb,
            super::DEFAULT_MAX_BODY_MB,
        )),
        disk_cache: Some(pick_or(
            s,
            "disk_cache",
            &f.disk_cache,
            &d.disk_cache,
            super::DEFAULT_DISK_CACHE,
        )),
        forward_only: Some(delivery == Delivery::Forward),
        retry_rules: pick_or(s, "retry.rules", &f.retry.rules, &d.retry.rules, Vec::new()),
        allowed_hosts: Some(pick_or(
            s,
            "allowed_hosts",
            &f.allowed_hosts,
            &d.allowed_hosts,
            Vec::new(),
        )),
        allowed_origins: Some(pick_or(
            s,
            "allowed_origins",
            &f.allowed_origins,
            &d.allowed_origins,
            Vec::new(),
        )),
        request_transform,
        response_transform,
        heartbeat_transform,
        log_file,
        spool_dir_override: None,
    }
    .normalized();

    // 合并之后的语义问题。文件解析已经报过的（同码同键）不再报一遍
    for problem in config.problems() {
        if diagnostics
            .iter()
            .any(|d| d.code == problem.code && d.key.as_deref() == Some(problem.key.as_str()))
        {
            continue;
        }
        // 规则与头表里的条目，来源记在它们所属的那个键上
        let source_key = if problem.key.starts_with("retry.rules[") {
            "retry.rules"
        } else {
            problem.key.as_str()
        };
        let source = sources.get(source_key).copied();
        let position = match (source, text) {
            (Some(Source::Toml), Some(text)) => file::locate(text, &problem.key),
            _ => None,
        };
        let (file, message) = match source {
            Some(Source::Settings) => (defaults.file.as_path(), problem.message),
            Some(Source::Override) => (
                path,
                format!("{} (given on the command line)", problem.message),
            ),
            _ => (path, problem.message),
        };
        diagnostics.push(Diagnostic {
            code: problem.code,
            severity: Level::Error,
            file: file.display().to_string(),
            key: Some(problem.key),
            line: position.map(|p| p.0),
            column: position.map(|p| p.1),
            message,
            details: problem.reason.map(reason_details).unwrap_or_default(),
        });
    }

    Resolved {
        config,
        sources,
        diagnostics,
    }
}

fn reason_details(reason: &str) -> Map<String, Value> {
    let mut details = Map::new();
    details.insert("reason".to_string(), Value::String(reason.to_string()));
    details
}

/// 取最高一层的值并记下来源；三层都没有时返回 None、不记来源。
fn pick<T: Clone>(
    sources: &mut BTreeMap<String, Source>,
    key: &str,
    over: &Option<T>,
    toml: &Option<T>,
    settings: &Option<T>,
) -> Option<T> {
    let (value, source) = if let Some(value) = over {
        (value, Source::Override)
    } else if let Some(value) = toml {
        (value, Source::Toml)
    } else if let Some(value) = settings {
        (value, Source::Settings)
    } else {
        return None;
    };
    sources.insert(key.to_string(), source);
    Some(value.clone())
}

/// 同 [`pick`]，用于没有启动覆盖、但有内置默认的键。
fn pick_or<T: Clone>(
    sources: &mut BTreeMap<String, Source>,
    key: &str,
    toml: &Option<T>,
    settings: &Option<T>,
    default: T,
) -> T {
    pick(sources, key, &None, toml, settings).unwrap_or_else(|| {
        sources.insert(key.to_string(), Source::Default);
        default
    })
}

/// 头表按头名合并：全局默认打底，config.toml 的同名头（不分大小写）取代它。
fn merge_headers(
    sources: &mut BTreeMap<String, Source>,
    table: &str,
    toml: &Option<BTreeMap<String, String>>,
    settings: &Option<BTreeMap<String, String>>,
) -> HashMap<String, String> {
    let mut merged: Vec<(&String, &String, Source)> = Vec::new();
    for (name, value) in settings.iter().flatten() {
        merged.push((name, value, Source::Settings));
    }
    for (name, value) in toml.iter().flatten() {
        merged.retain(|(existing, _, _)| !existing.eq_ignore_ascii_case(name));
        merged.push((name, value, Source::Toml));
    }
    merged
        .into_iter()
        .map(|(name, value, source)| {
            sources.insert(format!("{table}.{}", super::quote_key(name)), source);
            (name.clone(), value.clone())
        })
        .collect()
}

/// 把 `[[format]]` 的条目派给现有的三个转换位置：请求、响应、心跳各至多一个程序。
/// 这个版本还不支持的写法——同一阶段多个程序，以及 decide、stream、notify 阶段——报
/// `config_not_supported`，而不是带着一个不会被调用的程序启动。
fn assign_formats(
    formats: &[FormatEntry],
    text: Option<&str>,
    path: &Path,
    diagnostics: &mut Vec<Diagnostic>,
) -> (
    Option<TransformConfig>,
    Option<TransformConfig>,
    Option<TransformConfig>,
) {
    let mut slots: [Option<TransformConfig>; 3] = [None, None, None];
    for (index, format) in formats.iter().enumerate() {
        let mut unsupported = |message: String| {
            let key = format!("format[{index}].stages");
            let position = text.and_then(|text| file::locate(text, &key));
            diagnostics.push(Diagnostic {
                code: "config_not_supported",
                severity: Level::Error,
                file: path.display().to_string(),
                key: Some(key),
                line: position.map(|p| p.0),
                column: position.map(|p| p.1),
                message,
                details: Map::new(),
            });
        };
        if format.has_stage(Stage::Retry) && !format.has_stage(Stage::Request) {
            unsupported(format!(
                "format {:?} declares retry without request, which is not implemented in this version",
                format.name
            ));
        }
        for stage in &format.stages {
            let slot = match stage {
                Stage::Request => 0,
                Stage::Response => 1,
                Stage::Heartbeat => 2,
                Stage::Retry => continue,
                Stage::Decide | Stage::Stream | Stage::Notify => {
                    unsupported(format!(
                        "the {} stage is not implemented in this version",
                        stage.as_str()
                    ));
                    continue;
                }
            };
            if slots[slot].is_some() {
                unsupported(format!(
                    "more than one format for the {} stage is not implemented in this version",
                    stage.as_str()
                ));
                continue;
            }
            slots[slot] = Some(TransformConfig {
                name: format.name.clone(),
                env: format.env.clone(),
                command: format.command.clone(),
                args: format.args.clone(),
                mode: match format.process {
                    Process::Spawn => TransformMode::Spawn,
                    Process::Persistent => TransformMode::Persistent,
                },
                pool_max: Some(format.pool_max),
                idle_timeout_secs: Some(format.worker_idle_secs),
                timeout_secs: Some(format.timeout_secs),
                extra: None,
                every_attempt: *stage == Stage::Request && format.has_stage(Stage::Retry),
            });
        }
    }
    let [request, response, heartbeat] = slots;
    (request, response, heartbeat)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults(values: FileConfig) -> Defaults {
        Defaults {
            file: PathBuf::from("settings.json"),
            values,
            diagnostics: Vec::new(),
        }
    }

    fn run(text: &str, overrides: &Overrides, values: FileConfig) -> Resolved {
        resolve_text(
            Some(text),
            Path::new("config.toml"),
            overrides,
            &defaults(values),
            false,
        )
    }

    fn brief(resolved: &Resolved) -> Vec<String> {
        resolved
            .diagnostics
            .iter()
            .map(|d| {
                format!(
                    "{} {} {} {}",
                    d.code,
                    d.key.as_deref().unwrap_or("-"),
                    d.file,
                    d.line.map_or("-".to_string(), |l| l.to_string())
                )
            })
            .collect()
    }

    /// 四层各自生效，来源记对；环境变量只在没有配置代理时算数。
    #[test]
    fn the_four_layers_and_their_sources() {
        let text = r#"
base_url = "https://toml.example"
api_key = "toml-key"
max_request_mb = 8

[timeouts]
connect_secs = 5
"#;
        let overrides = Overrides {
            base_url: Some("https://override.example/".to_string()),
            ..Overrides::default()
        };
        let settings = FileConfig {
            base_url: Some("https://settings.example".to_string()),
            max_request_mb: Some(1),
            disk_cache: Some(false),
            timeouts: file::FileTimeouts {
                read_idle_secs: Some(60),
                ..Default::default()
            },
            ..FileConfig::default()
        };
        let resolved = resolve_text(
            Some(text),
            Path::new("config.toml"),
            &overrides,
            &defaults(settings),
            true,
        );
        assert_eq!(brief(&resolved), Vec::<String>::new());
        let c = &resolved.config;
        // 覆盖 > toml > settings > 内置默认
        assert_eq!(c.base_url, "https://override.example");
        assert_eq!(c.api_key.as_deref(), Some("toml-key"));
        assert_eq!(c.max_body_mb, Some(8));
        assert_eq!(c.connect_timeout_secs, 5);
        assert_eq!(c.disk_cache, Some(false));
        assert_eq!(c.read_timeout_secs, 60);
        assert_eq!(c.first_byte_timeout_secs, 300);
        assert_eq!(c.listen_addr, "127.0.0.1:12345");
        assert_eq!(c.keepalive_trigger(), KeepaliveTrigger::Any);
        assert!(!c.forward_only_enabled());

        let source = |key: &str| resolved.sources.get(key).copied();
        assert_eq!(source("base_url"), Some(Source::Override));
        assert_eq!(source("api_key"), Some(Source::Toml));
        assert_eq!(source("max_request_mb"), Some(Source::Toml));
        assert_eq!(source("timeouts.connect_secs"), Some(Source::Toml));
        assert_eq!(source("disk_cache"), Some(Source::Settings));
        assert_eq!(source("timeouts.read_idle_secs"), Some(Source::Settings));
        assert_eq!(source("timeouts.first_byte_secs"), Some(Source::Default));
        assert_eq!(source("listen_addr"), Some(Source::Default));
        assert_eq!(source("delivery"), Some(Source::Default));
        assert_eq!(source("retry.rules"), Some(Source::Default));
        assert_eq!(source("proxy"), Some(Source::Env));
        // 没设置、也没有内置默认的键不在里面
        assert_eq!(source("log_file"), None);
        assert_eq!(source("proxy_password"), None);

        // 配置了代理时，环境变量不算数
        let resolved = resolve_text(
            Some("base_url = \"https://a.example\"\nproxy = \"http://127.0.0.1:7890\""),
            Path::new("config.toml"),
            &Overrides::default(),
            &defaults(FileConfig::default()),
            true,
        );
        assert_eq!(resolved.sources["proxy"], Source::Toml);
    }

    /// 文件不存在：只剩覆盖与默认两层，缺 base_url 是一条指向这个文件的诊断。
    #[test]
    fn a_missing_file_is_an_empty_file() {
        let resolved = resolve_text(
            None,
            Path::new("config.toml"),
            &Overrides::default(),
            &defaults(FileConfig::default()),
            false,
        );
        assert_eq!(
            brief(&resolved),
            ["config_missing_key base_url config.toml -"]
        );
        assert!(!resolved.is_valid());

        let overrides = Overrides {
            base_url: Some("https://a.example".to_string()),
            ..Overrides::default()
        };
        let resolved = resolve_text(
            None,
            Path::new("config.toml"),
            &overrides,
            &defaults(FileConfig::default()),
            false,
        );
        assert!(resolved.is_valid());
    }

    /// 头表按头名合并，头名不分大小写，同名以 config.toml 为准；两张表各自合并。
    #[test]
    fn header_tables_merge_by_name() {
        let text = r#"
base_url = "https://a.example"
[extra_headers]
X-Shared = "from-toml"
x-toml = "1"
"#;
        let table = |pairs: &[(&str, &str)]| {
            Some(
                pairs
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect::<BTreeMap<_, _>>(),
            )
        };
        let settings = FileConfig {
            extra_headers: table(&[("x-shared", "from-settings"), ("x-settings", "2")]),
            override_headers: table(&[("x-shared", "override-table")]),
            ..FileConfig::default()
        };
        let resolved = run(text, &Overrides::default(), settings);
        assert_eq!(brief(&resolved), Vec::<String>::new());
        let mut extra: Vec<_> = resolved.config.extra_headers.iter().collect();
        extra.sort();
        assert_eq!(
            extra,
            [
                (&"X-Shared".to_string(), &"from-toml".to_string()),
                (&"x-settings".to_string(), &"2".to_string()),
                (&"x-toml".to_string(), &"1".to_string()),
            ]
        );
        assert_eq!(
            resolved.config.override_headers["x-shared"],
            "override-table"
        );
        assert_eq!(resolved.sources["extra_headers.X-Shared"], Source::Toml);
        assert_eq!(
            resolved.sources["extra_headers.x-settings"],
            Source::Settings
        );
        assert_eq!(
            resolved.sources["override_headers.x-shared"],
            Source::Settings
        );
        assert!(!resolved.sources.contains_key("extra_headers.x-shared"));
    }

    /// 数组整体替换：config.toml 写了规则，全局默认的规则就不再生效。
    #[test]
    fn arrays_are_replaced_as_a_whole() {
        let settings = FileConfig {
            retry: file::FileRetry {
                rules: Some(vec![RetryRule {
                    path: Some("/from-settings".into()),
                    status: None,
                    max_attempts: 3,
                }]),
                ..Default::default()
            },
            allowed_hosts: Some(vec!["settings-host".into()]),
            ..FileConfig::default()
        };
        let text = "base_url = \"https://a.example\"\nallowed_hosts = []\n";
        let resolved = run(text, &Overrides::default(), settings.clone());
        assert_eq!(resolved.config.retry_rules.len(), 1);
        assert_eq!(resolved.sources["retry.rules"], Source::Settings);
        // 空数组也是「写了」：把全局列表恢复成内置默认
        assert!(resolved.config.allowed_hosts().is_empty());
        assert_eq!(resolved.sources["allowed_hosts"], Source::Toml);

        let text =
            "base_url = \"https://a.example\"\n[[retry.rules]]\nstatus = [404]\nmax_attempts = 1\n";
        let resolved = run(text, &Overrides::default(), settings);
        assert_eq!(
            resolved.config.retry_rules,
            [RetryRule {
                path: None,
                status: Some(vec![404]),
                max_attempts: 1,
            }]
        );
        assert_eq!(resolved.sources["retry.rules"], Source::Toml);
    }

    /// 语义问题带着它的来源：来自 config.toml 的指到行，来自 settings 的指向 settings.json，
    /// 来自命令行的在说明里写明。
    #[test]
    fn problems_point_at_the_layer_they_come_from() {
        let text = "base_url = \"ftp://a.example\"\nlisten_addr = \"127.0.0.1\"\n\n[keepalive]\nheartbeat = \"data: x\"\n";
        let resolved = run(text, &Overrides::default(), FileConfig::default());
        assert_eq!(
            brief(&resolved),
            [
                "config_invalid_value base_url config.toml 1",
                "config_invalid_value listen_addr config.toml 2",
                "config_invalid_value keepalive.heartbeat config.toml 5",
            ]
        );
        assert_eq!(resolved.diagnostics[0].details["reason"], "not_http_url");

        let settings = FileConfig {
            retry: file::FileRetry {
                rules: Some(vec![RetryRule {
                    path: Some("(".into()),
                    status: None,
                    max_attempts: 3,
                }]),
                ..Default::default()
            },
            ..FileConfig::default()
        };
        let overrides = Overrides {
            proxy: Some("gopher://user:secret@proxy.example".to_string()),
            ..Overrides::default()
        };
        let resolved = run("base_url = \"https://a.example\"", &overrides, settings);
        assert_eq!(
            brief(&resolved),
            [
                "config_invalid_value proxy config.toml -",
                "config_invalid_value retry.rules[0].path settings.json -",
            ]
        );
        let proxy = &resolved.diagnostics[0];
        assert!(proxy.message.ends_with("(given on the command line)"));
        // 说明里的 URL 已打码
        assert!(!proxy.message.contains("secret"), "{}", proxy.message);
    }

    /// 文件解析已经报过的问题不报第二遍。
    #[test]
    fn file_diagnostics_are_not_repeated() {
        let text =
            "base_url = \"https://a.example\"\n[[retry.rules]]\npath = \"(\"\nmax_attempts = 1\n";
        let resolved = run(text, &Overrides::default(), FileConfig::default());
        assert_eq!(
            brief(&resolved),
            ["config_invalid_value retry.rules[0].path config.toml 3"]
        );
    }

    /// `[[format]]` 派给请求、响应、心跳三个位置；声明了 retry 的请求程序每次重试都重跑。
    #[test]
    fn formats_fill_the_three_slots() {
        let text = r#"
base_url = "https://a.example"

[[format]]
name = "keys"
command = "aproxy-format"
args = ["keys"]
process = "persistent"
stages = ["request", "retry"]
pool_max = 1
worker_idle_secs = 0
timeout_secs = 7
env = { KEYS_FILE = "keys.toml" }

[[format]]
command = "/opt/back.exe"
stages = ["response", "heartbeat"]
"#;
        let resolved = run(text, &Overrides::default(), FileConfig::default());
        assert_eq!(brief(&resolved), Vec::<String>::new());
        let c = &resolved.config;
        let request = c.request_transform.as_ref().unwrap();
        assert_eq!(request.name, "keys");
        assert_eq!(request.mode, TransformMode::Persistent);
        assert!(request.every_attempt);
        assert_eq!(request.effective_pool_max(), 1);
        assert_eq!(request.effective_idle_timeout_secs(), 0);
        assert_eq!(request.effective_timeout_secs(), 7);
        assert_eq!(request.env["KEYS_FILE"], "keys.toml");
        let response = c.response_transform.as_ref().unwrap();
        assert_eq!(
            (response.name.as_str(), response.every_attempt),
            ("back", false)
        );
        assert_eq!(c.heartbeat_transform.as_ref().unwrap().name, "back");
        assert_eq!(resolved.sources["format"], Source::Toml);
    }

    /// 这个版本还不支持的 format 写法是 error，指到条目的 stages。
    #[test]
    fn unsupported_format_shapes_are_errors() {
        let text = r#"base_url = "https://a.example"
[[format]]
name = "a"
command = "f"
process = "persistent"
stages = ["request", "decide", "notify"]
[[format]]
name = "b"
command = "f"
stages = ["request"]
"#;
        let resolved = run(text, &Overrides::default(), FileConfig::default());
        assert_eq!(
            brief(&resolved),
            [
                "config_not_supported format[0].stages config.toml 6",
                "config_not_supported format[0].stages config.toml 6",
                "config_not_supported format[1].stages config.toml 10",
            ]
        );
    }

    /// forward 与 format 现在还不能同配。
    #[test]
    fn forward_and_format_conflict() {
        let text = "base_url = \"https://a.example\"\ndelivery = \"forward\"\n[[format]]\ncommand = \"f\"\nstages = [\"request\"]\n";
        let resolved = run(text, &Overrides::default(), FileConfig::default());
        assert_eq!(brief(&resolved), ["config_conflict delivery config.toml 2"]);
        assert!(resolved.config.forward_only_enabled());
    }

    /// settings.json 的全局默认：与内置默认相同的值不算来自 settings；受限重试路径变成规则。
    #[test]
    fn settings_become_the_defaults_layer() {
        let settings = Settings::default();
        let layer = Defaults::from_settings(&settings, Path::new("settings.json"));
        assert_eq!(layer.values, FileConfig::default());
        assert!(layer.diagnostics.is_empty());

        let settings = Settings {
            max_body_mb: 16,
            disk_cache: false,
            forward_only: true,
            bounded_retry_paths: vec!["/v1/count".to_string()],
            allowed_hosts: vec!["h".to_string()],
            keepalive_trigger: "accept".to_string(),
            ..Settings::default()
        };
        let layer = Defaults::from_settings(&settings, Path::new("settings.json"));
        let resolved = resolve_text(
            Some("base_url = \"https://a.example\""),
            Path::new("config.toml"),
            &Overrides::default(),
            &layer,
            false,
        );
        assert_eq!(brief(&resolved), Vec::<String>::new());
        let c = &resolved.config;
        assert_eq!(c.max_body_mb, Some(16));
        assert!(!c.disk_cache_enabled());
        assert!(c.forward_only_enabled());
        assert_eq!(c.keepalive_trigger(), KeepaliveTrigger::Accept);
        assert_eq!(c.allowed_hosts(), ["h".to_string()]);
        assert_eq!(
            c.retry_rules,
            [RetryRule {
                path: Some("/v1/count".into()),
                status: None,
                max_attempts: 3,
            }]
        );
        assert_eq!(resolved.sources["delivery"], Source::Settings);

        let settings = Settings {
            keepalive_trigger: "sometimes".to_string(),
            ..Settings::default()
        };
        let layer = Defaults::from_settings(&settings, Path::new("settings.json"));
        let resolved = resolve_text(
            Some("base_url = \"https://a.example\""),
            Path::new("config.toml"),
            &Overrides::default(),
            &layer,
            false,
        );
        assert_eq!(
            brief(&resolved),
            ["config_invalid_value keepalive_trigger settings.json -"]
        );
    }

    /// 每条诊断的码都在码表的诊断族里，`details` 的键也都登记过。
    #[test]
    fn diagnostics_match_the_code_table() {
        let text = "base_url = \"ftp://x\"\nlisten_addr = \"x\"\nproxy_username = \"u\"\ndelivery = \"forward\"\n[extra_headers]\n\"bad name\" = \"v\"\n[[format]]\ncommand = \"f\"\nstages = [\"request\"]\n";
        let resolved = run(text, &Overrides::default(), FileConfig::default());
        assert!(resolved.diagnostics.len() >= 5, "{:?}", brief(&resolved));
        for d in &resolved.diagnostics {
            let entry = aproxy_ipc::code::lookup(d.code).unwrap_or_else(|| panic!("{}", d.code));
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
