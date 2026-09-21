//! 配置管理：`~/.aproxy/config.toml`，单一 upstream base URL，完整透传。
//!
//! 额外能力（兼容设计）：
//! - `api_key` 快捷：等效覆盖 `Authorization: Bearer <key>`
//! - `extra_headers`：仅当上游请求未携带该头时追加
//! - `override_headers`：无条件覆盖（用于非 Bearer 鉴权或额外头）
//! - `keepalive_interval_secs`：流式重试期间的保活心跳间隔，0 表示关闭
//! - `proxy`：上游请求经配置的代理转发（与常见代理配置一致，支持 http/https/socks5，
//!   可在 URL 内嵌 user:pass，也可用 `proxy_username`/`proxy_password` 单独指定）；
//!   未配置时保留 reqwest 默认的系统代理（读取 HTTP_PROXY/HTTPS_PROXY/ALL_PROXY 环境变量）

use std::{collections::HashMap, path::PathBuf};

use serde::{Deserialize, Serialize};

/// 请求体大小上限的内置默认值（MB）。settings.json 与 config.toml 均可覆盖
/// （toml > settings > 本值）。
pub const DEFAULT_MAX_BODY_MB: u64 = 128;
/// 磁盘缓存的内置默认值（开）。语义见 Config::disk_cache。
pub const DEFAULT_DISK_CACHE: bool = true;
/// 仅转发模式的内置默认值（关）。语义与代价见 Config::forward_only——
/// 开启即放弃本产品最核心的重试保障，故默认必须为关。
pub const DEFAULT_FORWARD_ONLY: bool = false;

/// 配置文件内容
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// 上游 API 的 base URL，例如 `https://api.anthropic.com`
    /// 末尾斜杠会被自动去除以避免拼接时产生 `//`。
    /// 旧版字段名 `upstream_url` 仍可读取（alias），保存时写为新名 `base_url`。
    #[serde(default, alias = "upstream_url")]
    pub base_url: String,
    /// 本地代理监听地址，默认 `127.0.0.1:12345`（仅本地可访问，避免局域网暴露）。
    #[serde(default = "default_listen_addr")]
    pub listen_addr: String,
    /// 快捷 api_key：若设置，等效覆盖 `Authorization: Bearer <api_key>`。
    #[serde(default)]
    pub api_key: Option<String>,
    /// 额外头：仅在请求未携带该头时追加（大小写不敏感判定）。
    #[serde(default)]
    pub extra_headers: HashMap<String, String>,
    /// 覆盖头：无条件覆盖请求头（大小写不敏感匹配，写入时保留配置中的大小写）。
    #[serde(default)]
    pub override_headers: HashMap<String, String>,
    /// 流式重试保活心跳间隔（秒），0 表示关闭。默认 15 秒。
    #[serde(default = "default_keepalive_secs")]
    pub keepalive_interval_secs: u64,
    /// 上游代理 URL，例如 `http://127.0.0.1:7890`、`socks5://user:pass@127.0.0.1:7890`。
    /// 未设置时使用系统/环境变量代理。
    #[serde(default)]
    pub proxy: Option<String>,
    /// 代理用户名（可选；优先于代理 URL 内嵌的 user:pass）。
    #[serde(default)]
    pub proxy_username: Option<String>,
    /// 代理密码（可选；优先于代理 URL 内嵌的密码）。
    #[serde(default)]
    pub proxy_password: Option<String>,
    /// 重试退避的最大等待时间（秒）。指数退避 5s→10s→20s→…增长到该值后封顶；
    /// 设为 0 表示所有重试零延迟（立即重试）。默认 320 秒。
    #[serde(default = "default_max_backoff_secs")]
    pub max_retry_backoff_secs: u64,
    /// 上游响应缓冲（spool）上限（MB）。超过即视为不可重试的确定性失败。
    /// 默认 256；小内存机器可调小，转发超大文件可调大。
    #[serde(default = "default_spool_limit_mb")]
    pub spool_limit_mb: u64,
    /// 上游连接建立超时（秒）。默认 30；慢网络/高延迟上游可调大。
    #[serde(default = "default_connect_timeout_secs")]
    pub connect_timeout_secs: u64,
    /// 上游两次读到数据之间的超时（秒，同样钳制首字节等待）。默认 300；
    /// LLM 上游排队久（TTFB 数十秒）可调大，调过小会把「慢但活着」的上游
    /// 变成确定性无限重试。
    #[serde(default = "default_read_timeout_secs")]
    pub read_timeout_secs: u64,
    /// 请求体大小上限（MB），超出直接 413。未设置时用 settings.json 的
    /// `max_body_mb`（全局默认，内置 128）；设 0 表示不设限。
    /// None = toml 未显式配置（由 main 启动时注入 settings 值）。
    #[serde(default)]
    pub max_body_mb: Option<u64>,
    /// 磁盘缓存：开启时超过内存驻留阈值（1 MiB）的请求体与响应 spool 溢写
    /// 到磁盘临时文件——进程内存与负载大小解耦（SSD 上 IO 开销为噪声级）。
    /// 未设置时用 settings.json 的 `disk_cache`（全局默认，内置 true）。
    /// 高并发大请求体实例务必开启；低并发实例可关闭保持全内存行为。
    /// None = toml 未显式配置（由 main 启动时注入 settings 值）。
    #[serde(default)]
    pub disk_cache: Option<bool>,
    /// 仅转发模式：请求体边收边转发上游、上游响应边收边回客户端，全程不缓冲、
    /// 不落盘、不重试，也不发保活心跳。进程内存与负载大小完全解耦，下游拿到
    /// 真·增量流（首字节即转发，不必等上游 spool 完成）。
    /// **代价是失去重试能力**——这是模式的定义而非缺陷：上游中途断开时响应就
    /// 到此截断（不注入任何上游未发出的字节），代理不会重放请求；上游请求失败
    /// 直接 502。要让出核心保障，请确认你确实接受这一点。
    /// 适用场景：需要本地改写请求头（鉴权/追加头）+ 需要真流式 + 对该 API 的
    /// 稳定性有把握（不频繁中断）。
    /// 本模式下**不生效**的配置项：`disk_cache`、`spool_limit_mb`、
    /// `keepalive_interval_secs`、`max_retry_backoff_secs`（重试与 spool 两条
    /// 链路整体不进）；`max_body_mb` 仍强制——流式途中计数超限即 413。
    /// 未设置时用 settings.json 的 `forward_only`（全局默认，内置 false）。
    /// None = toml 未显式配置（由 main 启动时注入 settings 值）。
    #[serde(default)]
    pub forward_only: Option<bool>,
    /// 受限重试路径（正则数组）：命中任一模式的请求，上游「有响应的失败」达到
    /// `retry::BOUNDED_RETRY_MAX_ATTEMPTS` 次尝试后不再重试，把最后一次上游
    /// 响应原样透传给客户端。**空 = 功能关闭**（一切路径照旧无限重试）。
    ///
    /// 动机：部分上游对特定端点确定性报错，无限重试只会让客户端永远等不到
    /// 终态。哪些端点属于这一类完全因上游而异——同一软件换个上游结论就翻转，
    /// 因此哪些路径受限**交由用户按自己的上游配置**，不内置任何具体 URL。
    ///
    /// 匹配语义：每个模式是对「`路径?查询串` 整体」的正则，编译时自动锚定
    /// 两端（`^(?:模式)$`）——不含元字符的普通路径即精准匹配；查询串必须
    /// 显式出现在模式里（`?` 是正则元字符，字面量写 `\?`；toml 建议用单引号
    /// 字符串免转义）；通配用 `.*` 等。非法正则在 validate() 即报错，不会
    /// 静默失效。未设置时用 settings.json 的 `bounded_retry_paths`（全局
    /// 默认，内置空）。
    #[serde(default)]
    pub bounded_retry_paths: Option<Vec<String>>,
    /// spool 临时文件目录覆盖（serde skip，不落盘）。仅测试注入用：集成测试
    /// 进程内构建 AppState 时若无此覆盖，会按端口写入真实 ~/.aproxy/spool/。
    /// 生产路径为 None，实际目录 = ~/.aproxy/spool/<端口>/。
    #[serde(skip)]
    pub spool_dir_override: Option<PathBuf>,
}

/// 请求体上限（字节）：toml 显式值 > settings 注入值 > 内置 128。
/// 0 = 不设限（与其他超时/上限配置的 0 语义一致）。
/// None（未注入 settings 值，如测试直连构建）按内置默认。
pub(crate) fn body_limit_bytes(max_body_mb: Option<u64>) -> usize {
    match max_body_mb {
        Some(0) => usize::MAX,
        Some(mb) => (mb as usize).saturating_mul(1024 * 1024),
        None => (DEFAULT_MAX_BODY_MB as usize).saturating_mul(1024 * 1024),
    }
}

fn default_listen_addr() -> String {
    "127.0.0.1:12345".to_string()
}

fn default_keepalive_secs() -> u64 {
    15
}

fn default_max_backoff_secs() -> u64 {
    320
}

fn default_spool_limit_mb() -> u64 {
    256
}

fn default_connect_timeout_secs() -> u64 {
    30
}

fn default_read_timeout_secs() -> u64 {
    300
}

impl Default for Config {
    fn default() -> Self {
        Self {
            base_url: String::new(),
            listen_addr: default_listen_addr(),
            api_key: None,
            extra_headers: HashMap::new(),
            override_headers: HashMap::new(),
            keepalive_interval_secs: default_keepalive_secs(),
            proxy: None,
            proxy_username: None,
            proxy_password: None,
            max_retry_backoff_secs: default_max_backoff_secs(),
            spool_limit_mb: default_spool_limit_mb(),
            connect_timeout_secs: default_connect_timeout_secs(),
            read_timeout_secs: default_read_timeout_secs(),
            max_body_mb: None,
            disk_cache: None,
            forward_only: None,
            bounded_retry_paths: None,
            spool_dir_override: None,
        }
    }
}

impl Config {
    /// 归一化：去除 base_url 末尾斜杠；清理空 api_key；清理空代理配置。
    pub fn normalized(mut self) -> Self {
        self.base_url = self.base_url.trim_end_matches('/').to_string();
        if let Some(k) = &self.api_key {
            if k.trim().is_empty() {
                self.api_key = None;
            } else {
                self.api_key = Some(k.trim().to_string());
            }
        }
        // 头 k/v 统一 trim（与 CLI 路径 parse_kv 行为一致），trim 后 key 为空则剔除；
        // trim 后同键冲突时保留先出现者——HashMap::collect 对坍缩键的赢家是非确定的
        self.extra_headers = {
            let mut m = HashMap::new();
            for (k, v) in &self.extra_headers {
                let k = k.trim();
                if k.is_empty() {
                    continue;
                }
                m.entry(k.to_string())
                    .or_insert_with(|| v.trim().to_string());
            }
            m
        };
        self.override_headers = {
            let mut m = HashMap::new();
            for (k, v) in &self.override_headers {
                let k = k.trim();
                if k.is_empty() {
                    continue;
                }
                m.entry(k.to_string())
                    .or_insert_with(|| v.trim().to_string());
            }
            m
        };
        // 代理：空白视为未设置
        self.proxy = self
            .proxy
            .take()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        self.proxy_username = self
            .proxy_username
            .take()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        self.proxy_password = self
            .proxy_password
            .take()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        self
    }

    /// 校验：base_url 必须非空且为 http/https URL；proxy 若配置必须为受支持的代理 URL。
    pub fn validate(&self) -> Result<(), String> {
        if self.base_url.trim().is_empty() {
            return Err("base_url 不能为空，请在 ~/.aproxy/config.toml 中配置".to_string());
        }
        self.validate_base_url()?;
        if let Some(p) = &self.proxy {
            let url = url::Url::parse(p).map_err(|e| format!("proxy 配置无效 ({p}): {e}"))?;
            let scheme = url.scheme();
            if !matches!(
                scheme,
                "http" | "https" | "socks4" | "socks4a" | "socks5" | "socks5h"
            ) {
                return Err(format!(
                    "proxy 仅支持 http/https/socks4/socks5 协议，当前值: {p}"
                ));
            }
            if url.host_str().is_none() {
                return Err(format!("proxy 缺少主机地址: {p}"));
            }
        } else if self.proxy_username.is_some() || self.proxy_password.is_some() {
            // 仅当显式配置代理 URL 时用户名/密码才有意义，否则是配置遗漏
            return Err("配置了 proxy_username/proxy_password 但未配置 proxy URL".to_string());
        }
        // 受限重试路径：正则合法性在此把关（编译入口与运行期同一，保证
        // 「校验通过 = 运行期编译必成功」），非法模式启动即报明确错误而非
        // 静默不匹配
        for p in self.bounded_retry_paths() {
            Self::compile_bounded_retry_pattern(p)
                .map_err(|e| format!("bounded_retry_paths 含非法正则 {p:?}: {e}"))?;
        }
        Ok(())
    }

    /// 校验 base_url 自身的格式（scheme、无 query/fragment）。空值是否允许由调用方
    /// 决定——启动时禁止，而 `aproxy config` 允许分多次配置的中间态，故单独拆出。
    pub fn validate_base_url(&self) -> Result<(), String> {
        // scheme 判定大小写不敏感（"HTTP://" 亦合法）
        let scheme_lower = self.base_url.trim().to_ascii_lowercase();
        if !(scheme_lower.starts_with("http://") || scheme_lower.starts_with("https://")) {
            return Err(format!(
                "base_url 必须以 http:// 或 https:// 开头，当前值: {}",
                self.base_url
            ));
        }
        // 拒绝带 query/fragment 的 base_url：拼接 path 时会把路径拼进 query，静默错路由
        if self.base_url.contains('?') || self.base_url.contains('#') {
            return Err(format!(
                "base_url 不应包含 ? 或 #（路径拼接会错路由），当前值: {}",
                self.base_url
            ));
        }
        Ok(())
    }

    /// 是否启用保活心跳
    pub fn keepalive_enabled(&self) -> bool {
        self.keepalive_interval_secs > 0
    }

    /// 保活间隔
    pub fn keepalive_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.keepalive_interval_secs)
    }

    /// 请求体大小上限（字节）。max_body_mb 已在启动时注入 settings 值（toml
    /// 覆盖 settings），此处仅处理未注入（测试直连）的回退。
    pub fn body_limit_bytes(&self) -> usize {
        body_limit_bytes(self.max_body_mb)
    }

    /// 磁盘缓存是否启用（同 body_limit_bytes 的注入/回退语义）
    pub fn disk_cache_enabled(&self) -> bool {
        self.disk_cache.unwrap_or(DEFAULT_DISK_CACHE)
    }

    /// 仅转发模式是否启用（同 body_limit_bytes 的注入/回退语义：toml 显式值 >
    /// settings 注入值 > 内置 false）。
    ///
    /// 所有消费点都必须走本方法而非 `Option::unwrap()`——doctor 的
    /// parse_config_file、find::discover 与大量测试直接构建的 AppState 都不经
    /// settings 注入，`None` 在这些路径上是常态。
    pub fn forward_only_enabled(&self) -> bool {
        self.forward_only.unwrap_or(DEFAULT_FORWARD_ONLY)
    }

    /// 受限重试路径模式列表（同 body_limit_bytes 的注入/回退语义：toml 显式
    /// 值 > settings 注入值 > 内置空 = 功能关闭）。
    /// 消费点必须走本方法而非 `Option::unwrap()`——不经 settings 注入的构建
    /// 路径（doctor/find/测试）上 `None` 是常态。
    pub fn bounded_retry_paths(&self) -> &[String] {
        self.bounded_retry_paths.as_deref().unwrap_or(&[])
    }

    /// 编译一个受限重试路径模式：对「`路径?查询串` 整体」匹配，自动锚定两端
    /// ——不含元字符的普通路径即精准匹配，通配需显式写 `.*`。validate() 与
    /// AppState::new 共用此入口，两处语义不可能分叉。
    pub fn compile_bounded_retry_pattern(pattern: &str) -> Result<regex::Regex, regex::Error> {
        regex::Regex::new(&format!("^(?:{pattern})$"))
    }
}

/// 返回配置文件路径：`~/.aproxy/config.toml`。
pub fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

/// 返回配置目录：aProxy 主目录（`APROXY_HOME`，未设 = `~/.aproxy`，
/// 见 `settings::home`）。
pub fn config_dir() -> PathBuf {
    crate::settings::home()
}

/// 加载配置：若文件不存在则返回默认配置（base_url 为空，后续 validate 会提示）。
pub fn load() -> Config {
    load_from(&config_path())
}

/// base_url 展示打码：内嵌 userinfo（`https://user:pass@host`）时隐去密码段。
/// 无凭据（绝大多数情况）或解析失败时原样返回。
pub fn mask_base_url(raw: &str) -> String {
    let Ok(url) = url::Url::parse(raw) else {
        return raw.to_string();
    };
    if url.password().is_none() {
        return raw.to_string();
    }
    let host = url.host_str().unwrap_or("");
    let port = url.port().map(|p| format!(":{p}")).unwrap_or_default();
    let path = url.path();
    format!(
        "{}://{}:***@{}{}{}",
        url.scheme(),
        url.username(),
        host,
        port,
        path
    )
}

/// 加载指定路径的配置：语义同 `load`（不存在/解析失败回退默认配置）。
/// 多开场景由 `--config <PATH>` 显式指定路径；是否要求文件存在由调用方决定。
pub fn load_from(path: &std::path::Path) -> Config {
    match std::fs::read_to_string(path) {
        Ok(content) => match toml::from_str::<Config>(&content) {
            Ok(cfg) => cfg.normalized(),
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "配置文件解析失败，使用默认配置");
                Config::default()
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Config::default(),
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "读取配置文件失败，使用默认配置");
            Config::default()
        }
    }
}

/// 保存配置到默认路径（自动创建目录）。
pub fn save(cfg: &Config) -> std::io::Result<()> {
    save_to(&config_path(), cfg)
}

/// 保存配置到指定路径（自动创建目录）。多开场景配合 `--config <PATH>` 使用。
pub fn save_to(path: &std::path::Path, cfg: &Config) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = toml::to_string_pretty(cfg).expect("序列化配置失败");
    std::fs::write(path, content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_rejects_empty() {
        let cfg = Config {
            base_url: "".to_string(),
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validate_rejects_non_http() {
        let cfg = Config {
            base_url: "ftp://example.com".to_string(),
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validate_accepts_https() {
        let cfg = Config {
            base_url: "https://api.anthropic.com".to_string(),
            ..Default::default()
        };
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn normalized_trims_trailing_slash() {
        let cfg = Config {
            base_url: "https://api.anthropic.com/".to_string(),
            ..Default::default()
        }
        .normalized();
        assert_eq!(cfg.base_url, "https://api.anthropic.com");
    }

    #[test]
    fn normalized_trims_api_key_and_empties() {
        // api_key 前后空白被裁剪
        let cfg = Config {
            base_url: "https://api.example.com".to_string(),
            api_key: Some("  sk-test  ".to_string()),
            ..Default::default()
        }
        .normalized();
        assert_eq!(cfg.api_key.as_deref(), Some("sk-test"));

        // 全空白或空串视为未设置
        for k in ["   ", ""] {
            let cfg = Config {
                base_url: "https://api.example.com".to_string(),
                api_key: Some(k.to_string()),
                ..Default::default()
            }
            .normalized();
            assert!(cfg.api_key.is_none(), "api_key {:?} 应被置空", k);
        }
    }

    #[test]
    fn normalized_removes_blank_extra_header_keys() {
        let cfg = Config {
            base_url: "https://api.example.com".to_string(),
            extra_headers: HashMap::from([
                ("x-keep".to_string(), "1".to_string()),
                ("   ".to_string(), "2".to_string()),
                ("".to_string(), "3".to_string()),
            ]),
            ..Default::default()
        }
        .normalized();
        assert_eq!(cfg.extra_headers.len(), 1);
        assert_eq!(cfg.extra_headers.get("x-keep").unwrap(), "1");
    }

    #[test]
    fn default_port_is_12345() {
        assert_eq!(default_listen_addr(), "127.0.0.1:12345");
        assert_eq!(Config::default().listen_addr, "127.0.0.1:12345");
    }

    #[test]
    fn save_and_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut cfg = Config {
            base_url: "https://api.example.com".to_string(),
            listen_addr: "127.0.0.1:9999".to_string(),
            ..Default::default()
        };
        cfg.api_key = Some("sk-test".to_string());
        cfg.extra_headers
            .insert("x-extra".to_string(), "1".to_string());
        cfg.override_headers
            .insert("authorization".to_string(), "Bearer x".to_string());
        save_to(&path, &cfg).unwrap();
        let loaded = load_from(&path);
        assert_eq!(loaded.base_url, cfg.base_url);
        assert_eq!(loaded.listen_addr, cfg.listen_addr);
        assert_eq!(loaded.api_key, cfg.api_key);
        assert_eq!(loaded.extra_headers.get("x-extra").unwrap(), "1");
        assert_eq!(
            loaded.override_headers.get("authorization").unwrap(),
            "Bearer x"
        );
    }

    #[test]
    fn load_missing_returns_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.toml");
        let cfg = load_from(&path);
        assert!(cfg.base_url.is_empty());
        assert_eq!(cfg.listen_addr, default_listen_addr());
    }

    #[test]
    fn keepalive_defaults() {
        let cfg = Config::default();
        assert!(cfg.keepalive_enabled());
        assert_eq!(cfg.keepalive_interval_secs, 15);
    }

    #[test]
    fn proxy_defaults_to_none() {
        let cfg = Config::default();
        assert!(cfg.proxy.is_none());
        assert!(cfg.proxy_username.is_none());
        assert!(cfg.proxy_password.is_none());
    }

    #[test]
    fn default_completeness() {
        let cfg = Config::default();
        assert_eq!(cfg.base_url, "");
        assert_eq!(cfg.listen_addr, "127.0.0.1:12345");
        assert_eq!(cfg.keepalive_interval_secs, 15);
        assert!(cfg.api_key.is_none());
        assert!(cfg.extra_headers.is_empty());
        assert!(cfg.override_headers.is_empty());
        assert!(cfg.proxy.is_none());
        assert!(cfg.proxy_username.is_none());
        assert!(cfg.proxy_password.is_none());
        assert_eq!(cfg.max_retry_backoff_secs, 320);
        assert_eq!(cfg.spool_limit_mb, 256);
        assert_eq!(cfg.connect_timeout_secs, 30);
        assert_eq!(cfg.read_timeout_secs, 300);
    }

    #[test]
    fn tuning_fields_roundtrip() {
        // 三个调参字段写入读出 + 旧配置文件（无字段）读出默认值
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cfg.toml");
        let cfg = Config {
            base_url: "https://api.example.com".to_string(),
            spool_limit_mb: 64,
            connect_timeout_secs: 60,
            read_timeout_secs: 600,
            ..Default::default()
        };
        save_to(&path, &cfg).unwrap();
        let loaded = load_from(&path);
        assert_eq!(loaded.spool_limit_mb, 64);
        assert_eq!(loaded.connect_timeout_secs, 60);
        assert_eq!(loaded.read_timeout_secs, 600);
        std::fs::write(&path, "base_url = \"https://api.example.com\"").unwrap();
        let legacy = load_from(&path);
        assert_eq!(legacy.spool_limit_mb, 256);
        assert_eq!(legacy.connect_timeout_secs, 30);
        assert_eq!(legacy.read_timeout_secs, 300);
    }

    #[test]
    fn body_limit_and_disk_cache_override_semantics() {
        // toml 覆盖字段：显式值/0（不设限）写入读出；未配置（None）时
        // body_limit_bytes 回退内置默认 128MB，disk_cache_enabled 回退 true。
        // None 语义是「运行时由 settings 注入」，此处验证的是注入前的回退。
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cfg.toml");
        let cfg = Config {
            base_url: "https://api.example.com".to_string(),
            max_body_mb: Some(0),
            disk_cache: Some(false),
            ..Default::default()
        };
        save_to(&path, &cfg).unwrap();
        let loaded = load_from(&path);
        assert_eq!(loaded.max_body_mb, Some(0));
        assert_eq!(loaded.disk_cache, Some(false));
        assert_eq!(loaded.body_limit_bytes(), usize::MAX, "0 = 不设限");
        assert!(!loaded.disk_cache_enabled());

        // 未配置：回退内置默认（128MB / 开启）
        std::fs::write(&path, "base_url = \"https://api.example.com\"").unwrap();
        let legacy = load_from(&path);
        assert_eq!(legacy.max_body_mb, None);
        assert_eq!(
            legacy.body_limit_bytes(),
            128 * 1024 * 1024,
            "None 回退内置 128MB"
        );
        assert!(legacy.disk_cache_enabled(), "None 回退内置开启");
    }

    #[test]
    fn forward_only_override_semantics() {
        // toml 覆盖字段：Some(true)/Some(false) 落盘往返；未配置（None）时
        // forward_only_enabled 回退内置默认 false。None 语义是「运行时由
        // settings 注入」，此处验证的是注入前的回退（doctor / find / 测试
        // 直接构建 AppState 时正是这条路径，故绝不能对 None 做 unwrap）。
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cfg.toml");
        for value in [true, false] {
            let cfg = Config {
                base_url: "https://api.example.com".to_string(),
                forward_only: Some(value),
                ..Default::default()
            };
            save_to(&path, &cfg).unwrap();
            let loaded = load_from(&path);
            assert_eq!(loaded.forward_only, Some(value), "显式值应落盘往返");
            assert_eq!(loaded.forward_only_enabled(), value);
        }

        // 未配置：回退内置默认（关闭）
        std::fs::write(&path, "base_url = \"https://api.example.com\"").unwrap();
        let legacy = load_from(&path);
        assert_eq!(legacy.forward_only, None, "旧配置文件应读出 None");
        assert!(
            !legacy.forward_only_enabled(),
            "None 回退内置关闭（默认不得启用仅转发模式）"
        );
    }

    #[test]
    fn bounded_retry_paths_matching_semantics() {
        // 匹配语义钉死：模式对「路径?查询串」整体生效且自动锚定——
        // 普通路径即精准匹配（不含查询串时不命中带查询串的请求），
        // 通配必须显式写正则。
        let m = |pattern: &str, target: &str| {
            Config::compile_bounded_retry_pattern(pattern)
                .unwrap()
                .is_match(target)
        };
        // 精准匹配：路径本身命中，路径不带查询串的模式不命中带查询串的请求
        assert!(m("/v1/messages/count_tokens", "/v1/messages/count_tokens"));
        assert!(!m(
            "/v1/messages/count_tokens",
            "/v1/messages/count_tokens?beta=true"
        ));
        assert!(!m(
            "/v1/messages/count_tokens",
            "/v1/messages/count_tokens_extra"
        ));
        assert!(!m("/v1/messages/count_tokens", "/v1/messages"));
        // 查询串必须显式出现在模式中（`?` 是正则元字符，字面量需转义 `\?`）
        assert!(m(
            "/v1/messages/count_tokens\\?.*",
            "/v1/messages/count_tokens?beta=true"
        ));
        assert!(!m(
            "/v1/messages/count_tokens\\?.*",
            "/v1/messages/count_tokens"
        ));
        // 通配：`.*` 覆盖任意后缀；前缀式模式可同时命中带/不带查询串的请求
        assert!(m(
            "/v1/messages/count_tokens.*",
            "/v1/messages/count_tokens"
        ));
        assert!(m(
            "/v1/messages/count_tokens.*",
            "/v1/messages/count_tokens?beta=true"
        ));
        assert!(!m("/v1/messages/count_tokens.*", "/v1/messages"));
        // 自动锚定：模式不会作为子串命中其他路径
        assert!(!m("/messages", "/v1/messages/count_tokens"));
    }

    #[test]
    fn bounded_retry_paths_accessor_and_validate() {
        // 访问器注入/回退语义与 validate 的非法正则拒绝
        let mut cfg = Config {
            base_url: "https://api.example.com".to_string(),
            ..Default::default()
        };
        assert!(
            cfg.bounded_retry_paths().is_empty(),
            "None 回退空 = 功能关闭"
        );

        cfg.bounded_retry_paths = Some(vec!["/v1/messages/count_tokens".to_string()]);
        assert_eq!(cfg.bounded_retry_paths().len(), 1);
        cfg.validate().expect("合法正则应通过校验");

        let bad = Config {
            base_url: "https://api.example.com".to_string(),
            bounded_retry_paths: Some(vec!["[unclosed".to_string()]),
            ..Default::default()
        };
        let err = bad.validate().unwrap_err();
        assert!(err.contains("bounded_retry_paths"), "错误应指明字段: {err}");
        assert!(err.contains("[unclosed"), "错误应包含非法模式原文: {err}");
    }

    #[test]
    fn max_backoff_roundtrip_and_zero() {
        // 自定义封顶写入读出；0（所有重试零延迟）也必须能落盘往返
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cfg.toml");
        let mut cfg = Config {
            base_url: "https://api.example.com".to_string(),
            ..Default::default()
        };
        cfg.max_retry_backoff_secs = 60;
        save_to(&path, &cfg).unwrap();
        assert_eq!(load_from(&path).max_retry_backoff_secs, 60);
        cfg.max_retry_backoff_secs = 0;
        save_to(&path, &cfg).unwrap();
        assert_eq!(load_from(&path).max_retry_backoff_secs, 0);
        // 旧版配置文件（无该字段）读出默认 320
        std::fs::write(&path, "base_url = \"https://api.example.com\"").unwrap();
        assert_eq!(load_from(&path).max_retry_backoff_secs, 320);
    }

    #[test]
    fn proxy_normalized_trims_and_empties() {
        let cfg = Config {
            base_url: "https://api.example.com".to_string(),
            proxy: Some("  http://127.0.0.1:7890  ".to_string()),
            proxy_username: Some("  user  ".to_string()),
            proxy_password: Some("  pass  ".to_string()),
            ..Default::default()
        }
        .normalized();
        assert_eq!(cfg.proxy.as_deref(), Some("http://127.0.0.1:7890"));
        assert_eq!(cfg.proxy_username.as_deref(), Some("user"));
        assert_eq!(cfg.proxy_password.as_deref(), Some("pass"));

        // 纯空白视为未设置
        let cfg = Config {
            base_url: "https://api.example.com".to_string(),
            proxy: Some("   ".to_string()),
            proxy_username: Some("".to_string()),
            ..Default::default()
        }
        .normalized();
        assert!(cfg.proxy.is_none());
        assert!(cfg.proxy_username.is_none());
    }

    #[test]
    fn validate_accepts_proxy_schemes() {
        for p in [
            "http://127.0.0.1:7890",
            "https://proxy.example.com:8443",
            "socks5://127.0.0.1:1080",
            "socks4://127.0.0.1:1080",
            "socks5://user:pass@127.0.0.1:1080",
        ] {
            let cfg = Config {
                base_url: "https://api.example.com".to_string(),
                proxy: Some(p.to_string()),
                ..Default::default()
            };
            assert!(cfg.validate().is_ok(), "应接受代理 {p}");
        }
    }

    #[test]
    fn validate_accepts_socks_proxy_variants() {
        // socks5h/socks4a（DNS 走代理解析的变体）也应被接受
        for p in ["socks5h://127.0.0.1:1080", "socks4a://127.0.0.1:1080"] {
            let cfg = Config {
                base_url: "https://api.example.com".to_string(),
                proxy: Some(p.to_string()),
                ..Default::default()
            };
            assert!(cfg.validate().is_ok(), "应接受代理 {p}");
        }
    }

    #[test]
    fn validate_rejects_bad_proxy() {
        for p in [
            "ftp://127.0.0.1:21", // 不支持的协议
            "not-a-url",          // 缺少协议
            "http://",            // 缺少主机
        ] {
            let cfg = Config {
                base_url: "https://api.example.com".to_string(),
                proxy: Some(p.to_string()),
                ..Default::default()
            };
            assert!(cfg.validate().is_err(), "应拒绝代理 {p}");
        }
    }

    #[test]
    fn validate_rejects_creds_without_proxy_url() {
        let cfg = Config {
            base_url: "https://api.example.com".to_string(),
            proxy_username: Some("alice".to_string()),
            ..Default::default()
        };
        assert!(cfg.validate().is_err());

        // 仅有 proxy URL 时凭据字段不填应通过
        let cfg = Config {
            base_url: "https://api.example.com".to_string(),
            proxy: Some("http://127.0.0.1:7890".to_string()),
            ..Default::default()
        };
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn validate_rejects_base_url_with_query_or_fragment() {
        // 带 query/fragment 的 base_url 拼接 path 时会把路径拼进 query，静默错路由
        for p in [
            "https://api.example.com?v=1",
            "https://api.example.com#frag",
        ] {
            let cfg = Config {
                base_url: p.to_string(),
                ..Default::default()
            };
            assert!(
                cfg.validate().is_err(),
                "应拒绝含 query/fragment 的 base_url {p}"
            );
        }
        // 带路径前缀仍合法（如反向代理子路径）
        let cfg = Config {
            base_url: "https://api.example.com/api".to_string(),
            ..Default::default()
        };
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn validate_accepts_uppercase_scheme() {
        for p in ["HTTP://api.example.com", "Https://api.example.com"] {
            let cfg = Config {
                base_url: p.to_string(),
                ..Default::default()
            };
            assert!(cfg.validate().is_ok(), "大写 scheme {p} 应被接受");
        }
    }

    #[test]
    fn normalized_trims_header_keys_and_values() {
        // 配置文件路径进来的头 k/v 统一 trim，与 CLI parse_kv 行为一致；trim 后空 key 剔除
        let cfg = Config {
            base_url: "https://api.example.com".to_string(),
            extra_headers: HashMap::from([
                ("  x-a  ".to_string(), "  v1  ".to_string()),
                ("   ".to_string(), "dropped".to_string()),
            ]),
            override_headers: HashMap::from([("x-b".to_string(), "".to_string())]),
            ..Default::default()
        }
        .normalized();
        assert_eq!(cfg.extra_headers.get("x-a").map(String::as_str), Some("v1"));
        assert!(cfg.extra_headers.get("x-a") != cfg.extra_headers.get("  x-a  "));
        assert!(!cfg.extra_headers.contains_key("   "));
        assert_eq!(
            cfg.override_headers.get("x-b").map(String::as_str),
            Some("")
        );
    }

    #[test]
    fn legacy_upstream_url_field_still_loads() {
        // 旧版配置文件字段名 upstream_url 应经 alias 正常读取，保存时写为新名 base_url
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "upstream_url = \"https://legacy.example.com\"\nlisten_addr = \"127.0.0.1:12345\"\n",
        )
        .unwrap();
        let loaded = load_from(&path);
        assert_eq!(loaded.base_url, "https://legacy.example.com");

        // roundtrip 后应写为新字段名
        save_to(&path, &loaded).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("base_url"), "保存后应写新字段名 base_url");
        assert!(!content.contains("upstream_url"), "保存后不应再写旧字段名");
    }

    #[test]
    fn proxy_survives_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let cfg = Config {
            base_url: "https://api.example.com".to_string(),
            proxy: Some("socks5://127.0.0.1:1080".to_string()),
            proxy_username: Some("alice".to_string()),
            proxy_password: Some("secret".to_string()),
            ..Default::default()
        };
        save_to(&path, &cfg).unwrap();
        let loaded = load_from(&path);
        assert_eq!(loaded.proxy, cfg.proxy);
        assert_eq!(loaded.proxy_username, cfg.proxy_username);
        assert_eq!(loaded.proxy_password, cfg.proxy_password);
    }
}
