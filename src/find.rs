//! `aproxy find`：从配置目录列表（settings.config_dirs + 两个默认目录）发现
//! 全部 config（*.toml），按别名归属与关键字/端口过滤后列出。

use crate::settings::{self, Settings};
use std::path::PathBuf;

/// 一条发现记录：路径 + 解析出的配置（无法解析时 None）+ 指向它的别名列表
#[derive(Debug, Clone)]
pub struct DiscoveredConfig {
    pub path: PathBuf,
    pub config: Option<crate::config::Config>,
    pub aliases: Vec<String>,
    /// 是否为默认配置文件（settings.default_config 或 ~/.aproxy/config.toml）
    pub is_default: bool,
}

/// 扫描配置目录列表，发现全部 toml 配置（只查各目录该层，不递归子目录；
/// 跨目录重复路径按匹配键去重）。
pub fn discover(settings: &Settings) -> Vec<DiscoveredConfig> {
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
        &crate::config::config_path().display().to_string(),
    )];
    if let Some(p) = &settings.default_config {
        default_keys.push(settings::path_match_key(
            &settings::expand_path(p).display().to_string(),
        ));
    }

    let mut out: Vec<DiscoveredConfig> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for dir in settings::effective_config_dirs(settings) {
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
            out.push(DiscoveredConfig {
                path,
                config,
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

/// 列出发现的配置（人类可读）。空结果时也明确提示。
pub fn print_list(items: &[DiscoveredConfig]) {
    if items.is_empty() {
        println!(
            "没有找到配置文件。配置目录列表见 settings.json 的 config_dirs（默认 ~/.aproxy/ 与 ~/.aproxy/configs/）。"
        );
        return;
    }
    println!("找到 {} 个配置:", items.len());
    for d in items {
        let alias_part = if d.aliases.is_empty() {
            String::new()
        } else {
            format!("  别名: {}", d.aliases.join(", "))
        };
        let default_part = if d.is_default { "  [默认]" } else { "" };
        match &d.config {
            Some(c) => println!(
                "  {}  端口 {}  上游 {}{}{}",
                d.path.display(),
                crate::daemon::port_of(&c.listen_addr),
                crate::config::mask_base_url(&c.base_url),
                alias_part,
                default_part
            ),
            None => println!(
                "  {}  （无法解析为有效 aProxy 配置）{alias_part}{default_part}",
                d.path.display()
            ),
        }
    }
}
