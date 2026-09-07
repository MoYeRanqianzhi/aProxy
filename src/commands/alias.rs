//! `aproxy alias add/remove/list`：配置别名管理（存于 settings.json）。

use aproxy::config;
use aproxy::settings;

use crate::cli::AliasCmd;

/// `aproxy alias add/remove/list`：别名管理，存于 settings.json（内部配置，
/// 唯一；toml 配置可多份平行并存）。
pub(crate) fn handle_alias_cmd(cmd: AliasCmd) {
    match cmd {
        AliasCmd::Add { name, path } => {
            if let Err(e) = settings::validate_alias_name(&name) {
                eprintln!("别名无效: {e}");
                std::process::exit(1);
            }
            let path = match path {
                Some(p) => settings::expand_path(&p),
                None => config::config_path(),
            };
            if !path.exists() {
                eprintln!("配置文件不存在: {}", path.display());
                std::process::exit(1);
            }
            let mut s = settings::load();
            let replacing = s.aliases.contains_key(&name);
            s.aliases.insert(name.clone(), path.display().to_string());
            match settings::save(&s) {
                Ok(()) => {
                    if replacing {
                        println!("已更新别名 {} -> {}", name, path.display());
                    } else {
                        println!("已添加别名 {} -> {}", name, path.display());
                    }
                    println!("启动: aproxy start {name}    停止: aproxy stop {name}");
                }
                Err(e) => {
                    eprintln!("保存 settings.json 失败: {e}");
                    std::process::exit(1);
                }
            }
        }
        AliasCmd::Remove { name } => {
            let mut s = settings::load();
            match s.aliases.remove(&name) {
                Some(path) => match settings::save(&s) {
                    Ok(()) => println!("已删除别名 {name}（原指向 {path}）"),
                    Err(e) => {
                        eprintln!("保存 settings.json 失败: {e}");
                        std::process::exit(1);
                    }
                },
                None => {
                    eprintln!("别名 {name} 不存在。用 aproxy alias list 查看。");
                    std::process::exit(1);
                }
            }
        }
        AliasCmd::List => {
            let s = settings::load();
            if s.aliases.is_empty() {
                println!("暂无别名。添加: aproxy alias add <名称> <config.toml 路径>");
                return;
            }
            println!("别名 ({}):", s.aliases.len());
            let mut names: Vec<_> = s.aliases.iter().collect();
            names.sort();
            for (name, path) in names {
                println!("  {name} -> {path}");
            }
        }
    }
}
