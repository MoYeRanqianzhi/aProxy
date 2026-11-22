//! `aproxy find`：从配置目录列表发现全部配置并列出。

#[path = "../i18n/find.rs"]
mod text;
use aproxy_ipc::i18n::current;

use aproxy::find;
use aproxy::settings;
use serde_json::json;

/// `aproxy find [查询] [--aliased|--unaliased] [--port 端口] [--json]`：
/// 从配置目录列表发现全部配置并列出。
pub(crate) fn handle_find_cmd(
    aliased: bool,
    unaliased: bool,
    query: Option<String>,
    port: Option<u16>,
    json: bool,
) {
    let settings = settings::load();
    let filter = if aliased {
        find::AliasFilter::Aliased
    } else if unaliased {
        find::AliasFilter::Unaliased
    } else {
        find::AliasFilter::All
    };
    let items: Vec<_> = find::discover(&settings)
        .into_iter()
        .filter(|d| match filter {
            find::AliasFilter::All => true,
            find::AliasFilter::Aliased => !d.aliases.is_empty(),
            find::AliasFilter::Unaliased => d.aliases.is_empty(),
        })
        .filter(|d| query.as_deref().is_none_or(|q| d.matches_query(q)))
        // --port 过滤：不可解析端口（listen_addr 缺失/损坏）的配置 matches_port
        // 必为 false，无需特判
        .filter(|d| port.is_none_or(|p| d.matches_port(p)))
        .collect();
    if items.is_empty() {
        super::say(json, text::empty(current()));
        if !json {
            return;
        }
    }
    if json {
        let configs: Vec<_> = items
            .iter()
            .map(|d| {
                let config = d.config.as_ref();
                json!({
                    "path": d.path.display().to_string(),
                    "aliases": d.aliases,
                    "listen_addr": config.map(|c| &c.listen_addr),
                    "base_url": config.map(|c| aproxy_ipc::redact::url(&c.base_url)),
                    "errors": d.errors,
                })
            })
            .collect();
        super::print_doc(json!({"configs": configs}));
        return;
    }
    print_list(&items);
}

/// 列出找到的配置（人类可读），保留别名与默认配置标记。
fn print_list(items: &[find::DiscoveredConfig]) {
    println!("{}", text::found(current(), items.len()));
    for d in items {
        let alias_part = if d.aliases.is_empty() {
            String::new()
        } else {
            text::aliases(current(), &d.aliases.join(", "))
        };
        let default_part = if d.is_default {
            text::default(current())
        } else {
            ""
        };
        match &d.config {
            Some(c) => println!(
                "{}",
                text::valid(
                    current(),
                    d.path.display(),
                    aproxy::daemon::port_of(&c.listen_addr),
                    &aproxy_ipc::redact::url(&c.base_url),
                    &alias_part,
                    default_part
                )
            ),
            None => println!(
                "{}",
                text::invalid(current(), d.path.display(), &alias_part, default_part)
            ),
        }
    }
}
