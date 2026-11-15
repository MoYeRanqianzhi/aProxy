//! 配置别名管理。别名存于 settings.json（内部配置，唯一；toml 配置可多份平行并存）。

use std::path::PathBuf;

use crate::config;
use crate::settings;

/// 添加（或更新）别名成功。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Added {
    /// 别名指向的配置文件（已展开为绝对路径）
    pub path: PathBuf,
    /// true = 这个名字原先就有，这次是改了指向
    pub replaced: bool,
}

/// 别名操作失败的原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AliasError {
    /// 别名不合规。`reason` 说明哪里不合规
    InvalidName { reason: String },
    /// 要指向的配置文件不存在
    ConfigMissing { path: PathBuf },
    /// 要删除的别名不存在
    NotFound,
    /// settings.json 读不出或解析不了，拒绝在它上面写回。`reason` 是完整说明
    SettingsUnreadable { reason: String },
    /// settings.json 写不进去
    SaveFailed { reason: String },
}

/// 添加别名；同名已存在则改它的指向。`path` 省略时指向默认配置文件。
pub fn add(name: &str, path: Option<&str>) -> Result<Added, AliasError> {
    settings::validate_alias_name(name).map_err(|e| AliasError::InvalidName {
        reason: e.to_string(),
    })?;
    let path = match path {
        Some(p) => settings::expand_path(p),
        None => config::config_path(),
    };
    if !path.exists() {
        return Err(AliasError::ConfigMissing { path });
    }
    let mut s = settings::load_for_update().map_err(|e| AliasError::SettingsUnreadable {
        reason: e.to_string(),
    })?;
    let replaced = s
        .aliases
        .insert(name.to_string(), path.display().to_string())
        .is_some();
    settings::save(&s).map_err(|e| AliasError::SaveFailed {
        reason: e.to_string(),
    })?;
    Ok(Added { path, replaced })
}

/// 删除别名，返回它原先指向的路径。
pub fn remove(name: &str) -> Result<String, AliasError> {
    let mut s = settings::load_for_update().map_err(|e| AliasError::SettingsUnreadable {
        reason: e.to_string(),
    })?;
    let path = s.aliases.remove(name).ok_or(AliasError::NotFound)?;
    settings::save(&s).map_err(|e| AliasError::SaveFailed {
        reason: e.to_string(),
    })?;
    Ok(path)
}

/// 全部别名及其指向，按别名排序。
pub fn list() -> Vec<(String, String)> {
    let mut entries: Vec<_> = settings::load().aliases.into_iter().collect();
    entries.sort();
    entries
}
