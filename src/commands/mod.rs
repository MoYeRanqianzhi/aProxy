//! 子命令模块：每个 `aproxy <cmd>` 一个文件，与 CLI 定义（crate::cli）分离。
//!
//! 共享解析逻辑：
//! - `resolve_config_target`：start/stop 共用的「别名 | default 保留字 | 路径」
//!   target 解析
//! - `config_path_key`：配置路径的运行实例匹配键（统一走
//!   settings::path_match_key，别名匹配、配置目录去重、doctor 扫描共用同一比较规则）

pub(crate) mod alias;
pub(crate) mod config;
pub(crate) mod doctor;
pub(crate) mod find;
pub(crate) mod logs;
pub(crate) mod restart;
pub(crate) mod restore;
pub(crate) mod start;
pub(crate) mod status;
pub(crate) mod stop;

use std::path::PathBuf;

use aproxy::settings;

pub(crate) fn resolve_config_target(target: &str) -> Option<PathBuf> {
    if let Some(p) = settings::resolve_alias(target) {
        return Some(PathBuf::from(p));
    }
    if let Some(p) = settings::resolve_default_target(target) {
        return Some(p);
    }
    let p = settings::expand_path(target);
    if p.is_file() {
        return Some(p);
    }
    None
}

pub(crate) use settings::path_match_key as config_path_key;
