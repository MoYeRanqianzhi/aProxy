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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Settings {
    /// 配置文件别名表：名称 → config.toml 的绝对路径。
    /// 供 `aproxy start/stop <别名>` 快捷定位配置。
    #[serde(default)]
    pub aliases: HashMap<String, String>,
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

/// 别名合法性与端口号/all 保留字冲突：纯数字会被 start/stop 当端口解析，
/// "all" 是 stop 的保留目标，均禁止作为别名。
pub fn validate_alias_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("别名不能为空".to_string());
    }
    if name == "all" {
        return Err("\"all\" 是 stop 的保留目标，不能用作别名".to_string());
    }
    if name.parse::<u16>().is_ok() {
        return Err("别名不能是纯数字（会与端口号解析冲突）".to_string());
    }
    Ok(())
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
    fn validate_alias_name_rejects_reserved() {
        assert!(validate_alias_name("").is_err());
        assert!(validate_alias_name("all").is_err());
        assert!(validate_alias_name("12345").is_err());
        assert!(validate_alias_name("openrouter").is_ok());
        assert!(validate_alias_name("my-cfg").is_ok());
    }
}
