//! 配置管理：`~/.aproxy/config.toml`，单一 upstream URL，完整透传。
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

/// 配置文件内容
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// 上游 API 的 base URL，例如 `https://api.anthropic.com`
    /// 末尾斜杠会被自动去除以避免拼接时产生 `//`。
    pub upstream_url: String,
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
}

fn default_listen_addr() -> String {
    "127.0.0.1:12345".to_string()
}

fn default_keepalive_secs() -> u64 {
    15
}

impl Default for Config {
    fn default() -> Self {
        Self {
            upstream_url: String::new(),
            listen_addr: default_listen_addr(),
            api_key: None,
            extra_headers: HashMap::new(),
            override_headers: HashMap::new(),
            keepalive_interval_secs: default_keepalive_secs(),
            proxy: None,
            proxy_username: None,
            proxy_password: None,
        }
    }
}

impl Config {
    /// 归一化：去除 upstream 末尾斜杠；清理空 api_key；清理空代理配置。
    pub fn normalized(mut self) -> Self {
        self.upstream_url = self.upstream_url.trim_end_matches('/').to_string();
        if let Some(k) = &self.api_key {
            if k.trim().is_empty() {
                self.api_key = None;
            } else {
                self.api_key = Some(k.trim().to_string());
            }
        }
        // 清理空键
        self.extra_headers.retain(|k, _| !k.trim().is_empty());
        self.override_headers.retain(|k, _| !k.trim().is_empty());
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

    /// 校验：upstream 必须非空且为 http/https URL；proxy 若配置必须为受支持的代理 URL。
    pub fn validate(&self) -> Result<(), String> {
        if self.upstream_url.trim().is_empty() {
            return Err("upstream_url 不能为空，请在 ~/.aproxy/config.toml 中配置".to_string());
        }
        if !(self.upstream_url.starts_with("http://") || self.upstream_url.starts_with("https://")) {
            return Err(format!(
                "upstream_url 必须以 http:// 或 https:// 开头，当前值: {}",
                self.upstream_url
            ));
        }
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
}

/// 返回配置文件路径：`~/.aproxy/config.toml`。
pub fn config_path() -> PathBuf {
    let home = dirs::home_dir().expect("无法获取用户主目录");
    home.join(".aproxy").join("config.toml")
}

/// 返回配置目录：`~/.aproxy/`。
pub fn config_dir() -> PathBuf {
    let home = dirs::home_dir().expect("无法获取用户主目录");
    home.join(".aproxy")
}

/// 加载配置：若文件不存在则返回默认配置（upstream 为空，后续 validate 会提示）。
pub fn load() -> Config {
    load_from(&config_path())
}

fn load_from(path: &std::path::Path) -> Config {
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

fn save_to(path: &std::path::Path, cfg: &Config) -> std::io::Result<()> {
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
            upstream_url: "".to_string(),
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validate_rejects_non_http() {
        let cfg = Config {
            upstream_url: "ftp://example.com".to_string(),
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validate_accepts_https() {
        let cfg = Config {
            upstream_url: "https://api.anthropic.com".to_string(),
            ..Default::default()
        };
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn normalized_trims_trailing_slash() {
        let cfg = Config {
            upstream_url: "https://api.anthropic.com/".to_string(),
            ..Default::default()
        }
        .normalized();
        assert_eq!(cfg.upstream_url, "https://api.anthropic.com");
    }

    #[test]
    fn normalized_trims_api_key_and_empties() {
        // api_key 前后空白被裁剪
        let cfg = Config {
            upstream_url: "https://api.example.com".to_string(),
            api_key: Some("  sk-test  ".to_string()),
            ..Default::default()
        }
        .normalized();
        assert_eq!(cfg.api_key.as_deref(), Some("sk-test"));

        // 全空白或空串视为未设置
        for k in ["   ", ""] {
            let cfg = Config {
                upstream_url: "https://api.example.com".to_string(),
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
            upstream_url: "https://api.example.com".to_string(),
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
            upstream_url: "https://api.example.com".to_string(),
            listen_addr: "127.0.0.1:9999".to_string(),
            ..Default::default()
        };
        cfg.api_key = Some("sk-test".to_string());
        cfg.extra_headers.insert("x-extra".to_string(), "1".to_string());
        cfg.override_headers.insert("authorization".to_string(), "Bearer x".to_string());
        save_to(&path, &cfg).unwrap();
        let loaded = load_from(&path);
        assert_eq!(loaded.upstream_url, cfg.upstream_url);
        assert_eq!(loaded.listen_addr, cfg.listen_addr);
        assert_eq!(loaded.api_key, cfg.api_key);
        assert_eq!(loaded.extra_headers.get("x-extra").unwrap(), "1");
        assert_eq!(loaded.override_headers.get("authorization").unwrap(), "Bearer x");
    }

    #[test]
    fn load_missing_returns_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.toml");
        let cfg = load_from(&path);
        assert!(cfg.upstream_url.is_empty());
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
        assert_eq!(cfg.upstream_url, "");
        assert_eq!(cfg.listen_addr, "127.0.0.1:12345");
        assert_eq!(cfg.keepalive_interval_secs, 15);
        assert!(cfg.api_key.is_none());
        assert!(cfg.extra_headers.is_empty());
        assert!(cfg.override_headers.is_empty());
        assert!(cfg.proxy.is_none());
        assert!(cfg.proxy_username.is_none());
        assert!(cfg.proxy_password.is_none());
    }

    #[test]
    fn proxy_normalized_trims_and_empties() {
        let cfg = Config {
            upstream_url: "https://api.example.com".to_string(),
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
            upstream_url: "https://api.example.com".to_string(),
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
                upstream_url: "https://api.example.com".to_string(),
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
                upstream_url: "https://api.example.com".to_string(),
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
                upstream_url: "https://api.example.com".to_string(),
                proxy: Some(p.to_string()),
                ..Default::default()
            };
            assert!(cfg.validate().is_err(), "应拒绝代理 {p}");
        }
    }

    #[test]
    fn validate_rejects_creds_without_proxy_url() {
        let cfg = Config {
            upstream_url: "https://api.example.com".to_string(),
            proxy_username: Some("alice".to_string()),
            ..Default::default()
        };
        assert!(cfg.validate().is_err());

        // 仅有 proxy URL 时凭据字段不填应通过
        let cfg = Config {
            upstream_url: "https://api.example.com".to_string(),
            proxy: Some("http://127.0.0.1:7890".to_string()),
            ..Default::default()
        };
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn proxy_survives_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let cfg = Config {
            upstream_url: "https://api.example.com".to_string(),
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
