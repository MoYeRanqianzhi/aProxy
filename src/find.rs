//! `aproxy find`：从配置目录列表（settings.config_dirs + 两个默认目录）发现
//! 全部 config（*.toml），按别名归属与关键字/端口过滤后列出。

use crate::settings::{self, Settings};
use std::path::PathBuf;

/// 一条发现记录：路径 + 解析出的配置（无法解析时 None）+ 指向它的别名列表
#[derive(Debug, Clone)]
pub struct DiscoveredConfig {
    pub path: PathBuf,
    pub config: Option<crate::config::Config>,
    /// 按启动实例时的分层管线（配置文件与 settings.json 的全局默认）解析所得的 error 级诊断；
    /// 没有错误或文件读不出来时为空。
    pub errors: Vec<aproxy_ipc::diag::Diagnostic>,
    pub aliases: Vec<String>,
    /// 是否为默认配置文件（settings.default_config 或 ~/.aproxy/config.toml）
    pub is_default: bool,
}

/// 扫描配置目录列表，发现全部 toml 配置（只查各目录该层，不递归子目录；
/// 跨目录重复路径按匹配键去重）。
pub fn discover(settings: &Settings) -> Vec<DiscoveredConfig> {
    discover_in(settings, &crate::config::config_dir())
}

/// 同上，默认配置与默认目录取自给定主目录（doctor 按被检查的 settings.json
/// 所在目录推导，见 `settings::default_config_dirs_in`）。
pub fn discover_in(settings: &Settings, home: &std::path::Path) -> Vec<DiscoveredConfig> {
    let alias_map: std::collections::HashMap<String, Vec<String>> = {
        let mut m: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        for (name, raw) in &settings.aliases {
            let key = settings::path_match_key(&settings::expand_path(raw).display().to_string());
            m.entry(key).or_default().push(name.clone());
        }
        for v in m.values_mut() {
            v.sort();
        }
        m
    };
    let mut default_keys: Vec<String> = vec![settings::path_match_key(
        &home.join("config.toml").display().to_string(),
    )];
    if let Some(p) = &settings.default_config {
        default_keys.push(settings::path_match_key(
            &settings::expand_path(p).display().to_string(),
        ));
    }

    let mut out: Vec<DiscoveredConfig> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let defaults = crate::config::resolve::Defaults::from_settings(
        settings,
        &crate::settings::settings_path_in(home),
    );
    for dir in settings::effective_config_dirs_in(settings, home) {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("toml") {
                continue;
            }
            let key = settings::path_match_key(&path.display().to_string());
            if !seen.insert(key.clone()) {
                continue;
            }
            let aliases = alias_map.get(&key).cloned().unwrap_or_default();
            let is_default = default_keys.contains(&key);
            let config = crate::doctor::parse_config_file(&path);
            // 展示和端口过滤仍读取文件本身；只有诊断需要合并全局默认，才能反映启动时的错误。
            let errors =
                crate::config::resolve(&path, &crate::config::Overrides::default(), &defaults)
                    .map(|resolved| resolved.errors().cloned().collect())
                    .unwrap_or_default();
            out.push(DiscoveredConfig {
                path,
                config,
                errors,
                aliases,
                is_default,
            });
        }
    }
    out.sort_by_key(|a| a.path.display().to_string());
    out
}

/// 过滤：别名归属（Some(true)=仅已配别名、Some(false)=仅未配别名、None=全部）
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AliasFilter {
    All,
    Aliased,
    Unaliased,
}

impl DiscoveredConfig {
    /// 关键字匹配：路径片段或别名包含（大小写不敏感）；关键字为空恒真
    pub fn matches_query(&self, query: &str) -> bool {
        let q = query.to_lowercase();
        if q.is_empty() {
            return true;
        }
        self.path.display().to_string().to_lowercase().contains(&q)
            || self.aliases.iter().any(|a| a.to_lowercase().contains(&q))
    }
    /// 端口匹配：配置的 listen_addr 端口等于指定值（不可解析的配置恒否）
    pub fn matches_port(&self, port: u16) -> bool {
        self.config
            .as_ref()
            .and_then(|c| crate::daemon::port_of(&c.listen_addr).parse::<u16>().ok())
            .is_some_and(|p| p == port)
    }
}
