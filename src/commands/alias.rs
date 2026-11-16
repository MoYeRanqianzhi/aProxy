//! `aproxy alias add/remove/list`：配置别名管理（存于 settings.json）。

use aproxy::cli::AliasCmd;
use aproxy::ops::alias::{self, AliasError};

/// `aproxy alias add/remove/list`：别名管理，存于 settings.json（内部配置，
/// 唯一；toml 配置可多份平行并存）。
pub(crate) fn handle_alias_cmd(cmd: AliasCmd) {
    match cmd {
        AliasCmd::Add { name, path } => match alias::add(&name, path.as_deref()) {
            Ok(added) => {
                if added.replaced {
                    println!("已更新别名 {} -> {}", name, added.path.display());
                } else {
                    println!("已添加别名 {} -> {}", name, added.path.display());
                }
                println!("启动: aproxy start {name}    停止: aproxy stop {name}");
            }
            Err(e) => fail(&name, &e),
        },
        AliasCmd::Remove { name } => match alias::remove(&name) {
            Ok(path) => println!("已删除别名 {name}（原指向 {path}）"),
            Err(e) => fail(&name, &e),
        },
        AliasCmd::List => {
            let entries = alias::list();
            if entries.is_empty() {
                println!("暂无别名。添加: aproxy alias add <名称> <config.toml 路径>");
                return;
            }
            println!("别名 ({}):", entries.len());
            for (name, path) in entries {
                println!("  {name} -> {path}");
            }
        }
    }
}

/// 说明别名操作失败的原因，并以 1 退出。
fn fail(name: &str, e: &AliasError) -> ! {
    match e {
        AliasError::InvalidName { reason } => eprintln!("别名无效: {reason}"),
        AliasError::ConfigMissing { path } => eprintln!("配置文件不存在: {}", path.display()),
        AliasError::NotFound => eprintln!("别名 {name} 不存在。用 aproxy alias list 查看。"),
        AliasError::SettingsUnreadable { reason } => eprintln!("{reason}"),
        AliasError::SaveFailed { reason } => eprintln!("{reason}"),
    }
    std::process::exit(1)
}
