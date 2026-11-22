//! `aproxy alias add/remove/list`：配置别名管理（存于 settings.json）。

#[path = "../i18n/alias.rs"]
mod text;
use aproxy_ipc::i18n::current;

use aproxy::cli::AliasCmd;
use aproxy::ops::alias::{self, AliasError};
use serde_json::json;

/// `aproxy alias add/remove/list`：别名管理，存于 settings.json（内部配置，
/// 唯一；toml 配置可多份平行并存）。
pub(crate) fn handle_alias_cmd(cmd: AliasCmd) {
    match cmd {
        AliasCmd::Add { name, path } => match alias::add(&name, path.as_deref()) {
            Ok(added) => {
                if added.replaced {
                    println!("{}", text::updated(current(), &name, added.path.display()));
                } else {
                    println!("{}", text::added(current(), &name, added.path.display()));
                }
                println!("{}", text::usage(current(), &name));
            }
            Err(e) => fail(&name, &e),
        },
        AliasCmd::Remove { name } => match alias::remove(&name) {
            Ok(path) => println!("{}", text::removed(current(), &name, &path)),
            Err(e) => fail(&name, &e),
        },
        AliasCmd::List { json } => {
            let entries = alias::list();
            if entries.is_empty() {
                super::say(json, text::empty(current()));
                if !json {
                    return;
                }
            }
            if json {
                // 给人看的列表照登记时的写法；文档给展开后的绝对路径（`~` 已展开），程序拿去
                // 就能用。文件不在了别名照样列出，`exists` 为 false
                let aliases: Vec<_> = entries
                    .into_iter()
                    .map(|(name, raw)| {
                        let path = aproxy::settings::expand_path(&raw);
                        json!({
                            "name": name,
                            "path": path.display().to_string(),
                            "exists": path.is_file(),
                        })
                    })
                    .collect();
                super::print_doc(json!({"aliases": aliases}));
                return;
            }
            println!("{}", text::count(current(), entries.len()));
            for (name, path) in entries {
                println!("  {name} -> {path}");
            }
        }
    }
}

/// 说明别名操作失败的原因，并以 1 退出。
fn fail(name: &str, e: &AliasError) -> ! {
    match e {
        AliasError::InvalidName { reason } => {
            eprintln!("{}", text::invalid(current(), &reason.render(current())))
        }
        AliasError::ConfigMissing { path } => {
            eprintln!("{}", super::config_text::missing(current(), path.display()))
        }
        AliasError::NotFound => eprintln!("{}", text::not_found(current(), name)),
        AliasError::SettingsUnreadable { reason, path } => {
            let why = reason
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_else(|| text::invalid_json(current()).to_owned());
            eprintln!("{}", text::cannot_edit(current(), &why, path.display()));
        }
        AliasError::SaveFailed { reason } => eprintln!(
            "{}",
            aproxy::settings::describe_edit_error(
                reason,
                &aproxy::settings::settings_path(),
                current()
            )
        ),
    }
    std::process::exit(1)
}
