//! `aproxy find`：从配置目录列表发现全部配置并列出。

use aproxy::find;
use aproxy::settings;

/// `aproxy find [查询] [--aliased|--unaliased] [--port 端口]`：
/// 从配置目录列表发现全部配置并列出。
pub(crate) fn handle_find_cmd(
    aliased: bool,
    unaliased: bool,
    query: Option<String>,
    port: Option<u16>,
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
    print_list(&items);
}

/// 列出发现的配置（人类可读）。空结果时也明确提示。
fn print_list(items: &[find::DiscoveredConfig]) {
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
                aproxy::daemon::port_of(&c.listen_addr),
                aproxy::config::mask_base_url(&c.base_url),
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
