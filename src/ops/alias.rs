//! 配置别名管理。别名存于 settings.json（内部配置，唯一；toml 配置可多份平行并存）。

use std::path::PathBuf;

use crate::config;
use crate::settings;
use crate::settings::edit::Change;

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
    /// settings.json 没有写成。`reason` 是完整说明
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
    let replaced = stored_alias(name)?.is_some();
    settings::update(&[Change::set(&alias_key(name), path.display().to_string())])
        .map_err(|reason| AliasError::SaveFailed { reason })?;
    Ok(Added { path, replaced })
}

/// 删除别名，返回它原先指向的路径。
pub fn remove(name: &str) -> Result<String, AliasError> {
    let path = stored_alias(name)?.ok_or(AliasError::NotFound)?;
    settings::update(&[Change::unset(&alias_key(name))])
        .map_err(|reason| AliasError::SaveFailed { reason })?;
    Ok(path)
}

/// 别名在 settings.json 里的键路径
fn alias_key(name: &str) -> String {
    format!("aliases.{}", config::quote_key(name))
}

/// settings.json 里这个别名现在指向哪里。直接看文件里的值，不要求整份文件有效：文件里
/// 别处有问题时，别名照样查得到、删得掉。文件读不出来或不是 JSON 对象时是
/// [`AliasError::SettingsUnreadable`]。
fn stored_alias(name: &str) -> Result<Option<String>, AliasError> {
    let path = settings::settings_path();
    let unreadable = |reason: String| AliasError::SettingsUnreadable {
        reason: format!(
            "settings.json 读不懂（{reason}），本次不做修改。请先修复或删除该文件: {}",
            path.display()
        ),
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(unreadable(e.to_string())),
    };
    let effective = settings::edit::effective_value(text.as_deref())
        .ok_or_else(|| unreadable("不是合法的 JSON 对象".to_string()))?;
    Ok(effective
        .get("aliases")
        .and_then(|aliases| aliases.get(name))
        .and_then(|value| value.as_str())
        .map(str::to_string))
}

/// 全部别名及其指向，按别名排序。
pub fn list() -> Vec<(String, String)> {
    let mut entries: Vec<_> = settings::load().aliases.into_iter().collect();
    entries.sort();
    entries
}
