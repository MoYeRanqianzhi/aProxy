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
    let mut items: Vec<_> = find::discover(&settings)
        .into_iter()
        .filter(|d| match filter {
            find::AliasFilter::All => true,
            find::AliasFilter::Aliased => !d.aliases.is_empty(),
            find::AliasFilter::Unaliased => d.aliases.is_empty(),
        })
        .filter(|d| query.as_deref().is_none_or(|q| d.matches_query(q)))
        .filter(|d| port.is_none_or(|p| d.matches_port(p)))
        .collect();
    // --port 过滤下不可解析的配置必然不匹配，无需特判；空结果统一提示
    if let Some(p) = port {
        items.retain(|d| d.matches_port(p));
    }
    find::print_list(&items);
}
