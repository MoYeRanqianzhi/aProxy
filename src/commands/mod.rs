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
pub(crate) mod install;
pub(crate) mod logs;
pub(crate) mod restart;
pub(crate) mod restore;
pub(crate) mod start;
pub(crate) mod status;
pub(crate) mod stop;

use std::path::PathBuf;

use aproxy::settings;

/// 实际生效的配置文件路径：`--config` 显式指定 > settings.json 的 `default_config`
/// （用户可把日常主力配置换成任意文件）> 默认 `~/.aproxy/config.toml`。
///
/// default_config 失效（文件被删/移动）返回 Err（给用户的报错）：静默回退默认
/// 配置会让用户在错误的配置文件上排障。但只有真正要读配置的命令（启动、
/// `config` 查看/修改内容）才兑现这个 Err（见 `require_cfg_path`）——status、
/// stop、doctor、自动拉起的 `install --continue` 等与配置文件无关的命令不能被
/// 一个失效的 default_config 拦住，报错里推荐的 `aproxy config --clear-default`
/// 自己更必须能执行。
///
/// 显式 `--config` 在此展开 `~` 并按当前工作目录绝对化：这个路径会被转发给
/// 守护子进程、写进注册表与 .restore，日后可能在别的工作目录被读取（restore
/// 会把找不到配置文件的记录当失效清理掉）。
pub(crate) fn resolve_cfg_path(cli: &crate::cli::Cli) -> Result<PathBuf, String> {
    if let Some(p) = &cli.config {
        return Ok(match p.to_str() {
            Some(s) => settings::expand_path(s),
            None => std::path::absolute(p).unwrap_or_else(|_| p.clone()),
        });
    }
    match settings::load().default_config {
        Some(p) => {
            let path = settings::expand_path(&p);
            if path.is_file() {
                Ok(path)
            } else {
                Err(format!(
                    "settings.json 指定的默认配置文件不存在: {}\n用 aproxy config --set-default <路径> 重新指定，或 aproxy config --clear-default 取消",
                    path.display()
                ))
            }
        }
        None => Ok(aproxy::config::config_path()),
    }
}

/// 兑现 `resolve_cfg_path` 的结果：失效即打印原因并以 1 退出。
pub(crate) fn require_cfg_path(cfg_path: Result<PathBuf, String>) -> PathBuf {
    cfg_path.unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1);
    })
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser as _;

    #[test]
    fn explicit_config_path_is_made_absolute() {
        // 相对 --config 必须在 CLI 边界绝对化：它会被写进 .restore，restore 若在
        // 别的工作目录运行，相对路径会被当成「配置已不存在」而清掉记录
        let cli =
            crate::cli::Cli::try_parse_from(["aproxy", "--config", "rel.toml", "status"]).unwrap();
        let path = resolve_cfg_path(&cli).unwrap();
        assert!(path.is_absolute(), "{}", path.display());
        assert_eq!(path, std::env::current_dir().unwrap().join("rel.toml"));

        let cli =
            crate::cli::Cli::try_parse_from(["aproxy", "--config", "~/x.toml", "status"]).unwrap();
        let path = resolve_cfg_path(&cli).unwrap();
        assert_eq!(path, dirs::home_dir().unwrap().join("x.toml"));
    }

    #[test]
    fn skills_only_accepts_a_version() {
        // latest 查询失败时的提示让用户改用「--skills-only <具体版本号>」，
        // 解析层必须接受这种写法
        assert!(
            crate::cli::Cli::try_parse_from(["aproxy", "install", "0.1.0", "--skills-only"])
                .is_ok()
        );
        assert!(
            crate::cli::Cli::try_parse_from(["aproxy", "install", "--skills-only", "--abort"])
                .is_err(),
            "与 --abort 等仍互斥"
        );
    }
}
