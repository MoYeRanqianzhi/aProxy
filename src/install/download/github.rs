//! github 通道（默认链第 1 级）：GitHub Releases 资产直下 + `.sha256`
//! 强校验（信任锚 = 仓库所有者的 release 身份）。
//!
//! latest 查询**不用** `/releases/latest`，也**不取列表首项**：
//! - 同一仓库有两条发版线——主线 `v*` 与 aproxy-format 的 `format-v*`。
//!   `/releases/latest` 返回「最新的非 prerelease release」，可能是
//!   format-v*（2026-09-30 实测即如此）；列表按创建时间（= tag 所在提交
//!   时间）倒序，同提交的两条线谁排第一是偶然，format tag 打在更新的提交
//!   上就一定排第一。
//! - 因此：拉一页足够大的列表（per_page=100），只认 `^v\d` 的 tag、跳过
//!   draft、要求含本平台所需资产，再按通道（stable 只要正式版 / pre 含
//!   预发布）以 **semver 取最大**——结果与列表顺序无关。
//!
//! 指定版本时按 tag `v<version>` 精确定位（`/releases/tags/v<version>`）。

use super::{Artifact, Channel, DownloadCtx, Fetched, download_to};
use std::path::Path;

const REPO: &str = "MoYeRanqianzhi/aProxy";

/// release 的资产（name → browser_download_url）。
#[derive(Debug)]
struct Release {
    assets: Vec<(String, String)>,
}

/// GitHub API GET → JSON（匿名 API，仓库公开，60/h 限流够用）。
async fn api_get(ctx: &DownloadCtx, url: &str) -> Result<serde_json::Value, String> {
    let resp = ctx
        .client
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("GitHub API 请求失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("GitHub API HTTP {}", resp.status()));
    }
    resp.json()
        .await
        .map_err(|e| format!("GitHub API 解析失败: {e}"))
}

/// 按版本精确定位 release（tag `v<version>`）。
async fn release_by_version(ctx: &DownloadCtx, version: &str) -> Result<Release, String> {
    let item = api_get(
        ctx,
        &format!("https://api.github.com/repos/{REPO}/releases/tags/v{version}"),
    )
    .await?;
    let mut assets = Vec::new();
    if let Some(list) = item["assets"].as_array() {
        for a in list {
            if let (Some(name), Some(url)) =
                (a["name"].as_str(), a["browser_download_url"].as_str())
            {
                assets.push((name.to_string(), url.to_string()));
            }
        }
    }
    Ok(Release { assets })
}

/// 主线 tag 判定：`^v\d`（`v0.1.0`、`v0.1.0-alpha.17`），排除 `format-v*`
/// 等其他发版线与非版本 tag。返回去掉 `v` 的版本串。
fn mainline_version(tag: &str) -> Option<&str> {
    let rest = tag.strip_prefix('v')?;
    rest.chars().next()?.is_ascii_digit().then_some(rest)
}

/// 纯函数选版：在 releases 列表 JSON（GitHub `/releases` 数组）中按通道取
/// semver 最大的主线版本。过滤规则（逐条，任何一条不满足即跳过）：
///
/// 1. 非 draft（匿名 API 本就看不到 draft，带 token 的环境才可能出现——
///    draft 资产不可公开下载）；
/// 2. tag 匹配 `^v\d` 且去 `v` 后是合法 semver（`format-v*` 等其他发版线、
///    手打的非版本 tag 一律不认）；
/// 3. 含 `required_asset`（本平台 baseline 二进制或 skill 总包）——发布
///    workflow 先建 release 后传资产，选中资产未齐的 release 只会 404；
/// 4. 通道：Stable 只要正式版——GitHub `prerelease` 标志与 semver 预发布
///    后缀**任一**表明是预发布即排除（两处信号可能因手工编辑而不一致，取
///    保守并集）；Pre 不按此过滤（含正式版，取全体最大）。
///
/// 返回 None = 通道内没有可用版本（由调用方决定「保持现状」，绝不跨通道
/// 偷偷回退）。
pub fn pick_latest(
    releases: &serde_json::Value,
    channel: Channel,
    required_asset: &str,
) -> Option<String> {
    let mut best: Option<(semver::Version, String)> = None;
    for item in releases.as_array()? {
        if item["draft"].as_bool().unwrap_or(false) {
            continue;
        }
        let Some(ver_str) = item["tag_name"].as_str().and_then(mainline_version) else {
            continue;
        };
        let Ok(ver) = semver::Version::parse(ver_str) else {
            continue;
        };
        let has_asset = item["assets"].as_array().is_some_and(|list| {
            list.iter()
                .any(|a| a["name"].as_str() == Some(required_asset))
        });
        if !has_asset {
            continue;
        }
        let is_pre = item["prerelease"].as_bool().unwrap_or(false) || !ver.pre.is_empty();
        if channel == Channel::Stable && is_pre {
            continue;
        }
        if best.as_ref().is_none_or(|(b, _)| ver > *b) {
            best = Some((ver, ver_str.to_string()));
        }
    }
    best.map(|(_, s)| s)
}

/// github 通道获取：下载 asset + `<asset>.sha256` 比对（强校验，不匹配即拒）。
pub async fn fetch(ctx: &DownloadCtx, artifact: Artifact, dest: &Path) -> Result<Fetched, String> {
    let asset = artifact.asset_name(ctx.target, ctx.variant);
    let release = release_by_version(ctx, &ctx.version).await?;
    let url = release
        .assets
        .iter()
        .find(|(n, _)| *n == asset)
        .map(|(_, u)| u.clone())
        .ok_or_else(|| format!("release v{} 无资产 {asset}", ctx.version))?;
    download_to(&ctx.client, &url, dest).await?;
    // .sha256 强校验
    let sum_url = format!("{url}.sha256");
    let expected = ctx
        .client
        .get(&sum_url)
        .send()
        .await
        .map_err(|e| format!(".sha256 下载失败: {e}"))?
        .error_for_status()
        .map_err(|e| format!(".sha256 资产不可用: {e}"))?
        .text()
        .await
        .map_err(|e| format!(".sha256 读取失败: {e}"))?;
    let expected = expected
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_lowercase();
    let actual = crate::install::staging::sha256_hex(dest)?;
    if expected != actual {
        let _ = std::fs::remove_file(dest);
        return Err(format!("sha256 不匹配（期望 {expected}，实际 {actual}）"));
    }
    Ok(Fetched {
        path: dest.to_path_buf(),
        sha256: actual,
        source: "github",
    })
}

/// 查询通道内最新版本（供 `install latest` / `--skills-only` 无参）。
/// `artifact` 决定「资产已齐」的判据：二进制取本平台 **baseline** 资产名
/// （每个 target 都发布 baseline；-v3 只覆盖部分 target，不能作必备判据）。
///
/// Ok(None) = GitHub 可达但通道内无版本（调用方保持现状，不转 npm——GitHub
/// 是发版的权威来源）；Err = API 不可达/限流（调用方转 npm 兜底）。
pub async fn latest_version(
    ctx: &DownloadCtx,
    channel: Channel,
    artifact: Artifact,
) -> Result<Option<String>, String> {
    // 一页 100 条：两条发版线合计远低于此量级（主线每版一条、format 线
    // 更稀疏）；即便未来超出，被挤出首页的也只会是最旧的 release
    let body = api_get(
        ctx,
        &format!("https://api.github.com/repos/{REPO}/releases?per_page=100"),
    )
    .await?;
    if !body.is_array() {
        return Err("GitHub Releases 列表格式异常（非数组）".to_string());
    }
    Ok(pick_latest(
        &body,
        channel,
        &artifact.asset_name(ctx.target, ""),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const ASSET: &str = "aproxy-x86_64-unknown-linux-gnu";

    /// 构造一条 release（默认非 draft、含 ASSET）
    fn rel(tag: &str, prerelease: bool) -> serde_json::Value {
        json!({
            "tag_name": tag,
            "draft": false,
            "prerelease": prerelease,
            "assets": [{"name": ASSET, "browser_download_url": "https://x/a"}]
        })
    }

    #[test]
    fn format_line_never_selected_regardless_of_order() {
        // 现网形态：format-v* 与主线同刻创建、排在列表任意位置，且 format 线
        // 不是 prerelease——首项/`/releases/latest` 语义都会误选它
        for list in [
            json!([rel("format-v0.1.1", false), rel("v0.1.0-alpha.17", true)]),
            json!([rel("v0.1.0-alpha.17", true), rel("format-v0.1.1", false)]),
        ] {
            assert_eq!(
                pick_latest(&list, Channel::Pre, ASSET).as_deref(),
                Some("0.1.0-alpha.17")
            );
            // stable 通道：主线只有预发布 → 无可选（不得回退去拿 format 线或预发布）
            assert_eq!(pick_latest(&list, Channel::Stable, ASSET), None);
        }
    }

    #[test]
    fn semver_max_not_list_order() {
        // 列表按创建时间倒序，与版本序无关：较旧提交上补发的 0.1.1 补丁可能
        // 排在 0.2.0-alpha.1 之后，也可能之前
        let list = json!([
            rel("v0.1.1", false),
            rel("v0.2.0-alpha.1", true),
            rel("v0.1.0", false),
            rel("v0.1.0-alpha.17", true),
        ]);
        // stable：只在正式版中取最大，0.2.0-alpha.1 > 0.1.1 也不选
        assert_eq!(
            pick_latest(&list, Channel::Stable, ASSET).as_deref(),
            Some("0.1.1")
        );
        // pre：含正式版取全体最大
        assert_eq!(
            pick_latest(&list, Channel::Pre, ASSET).as_deref(),
            Some("0.2.0-alpha.1")
        );
        // 预发布序号按数字比较：alpha.10 > alpha.9
        let list = json!([rel("v0.1.0-alpha.9", true), rel("v0.1.0-alpha.10", true)]);
        assert_eq!(
            pick_latest(&list, Channel::Pre, ASSET).as_deref(),
            Some("0.1.0-alpha.10")
        );
        // 正式版高于同号预发布
        let list = json!([rel("v0.1.0-rc.1", true), rel("v0.1.0", false)]);
        assert_eq!(
            pick_latest(&list, Channel::Pre, ASSET).as_deref(),
            Some("0.1.0")
        );
    }

    #[test]
    fn prerelease_signals_are_unioned_for_stable() {
        // GitHub 标志与 semver 后缀任一表明预发布即排除出 stable：手工把
        // alpha 的 prerelease 勾掉（标志 false）也不能让它混进稳定通道
        let list = json!([rel("v0.2.0-alpha.1", false), rel("v0.1.0", false)]);
        assert_eq!(
            pick_latest(&list, Channel::Stable, ASSET).as_deref(),
            Some("0.1.0")
        );
        // 反向：无后缀但被标为 prerelease 的也排除
        let list = json!([rel("v0.3.0", true), rel("v0.1.0", false)]);
        assert_eq!(
            pick_latest(&list, Channel::Stable, ASSET).as_deref(),
            Some("0.1.0")
        );
    }

    #[test]
    fn draft_missing_asset_and_junk_tags_skipped() {
        let mut draft = rel("v0.9.0", false);
        draft["draft"] = json!(true);
        let mut no_asset = rel("v0.8.0", false);
        no_asset["assets"] = json!([]);
        let mut other_asset = rel("v0.7.0", false);
        other_asset["assets"] = json!([{"name": "aproxy-skills.zip", "browser_download_url": "u"}]);
        let list = json!([
            draft,
            no_asset,
            other_asset,
            rel("vnext", false),
            rel("v1", false),
            rel("release-2", false),
            json!({"draft": false, "prerelease": false}),
            rel("v0.1.0", false),
        ]);
        // draft / 资产未齐（发布进行中）/ 只有别的资产 / 非版本 tag / 缺
        // tag_name 全部跳过，落到唯一合格的 0.1.0
        assert_eq!(
            pick_latest(&list, Channel::Stable, ASSET).as_deref(),
            Some("0.1.0")
        );
        // 资产判据随 artifact 变：skill 总包场景下 0.7.0 合格
        assert_eq!(
            pick_latest(&list, Channel::Stable, "aproxy-skills.zip").as_deref(),
            Some("0.7.0")
        );
    }

    #[test]
    fn empty_or_malformed_list() {
        assert_eq!(pick_latest(&json!([]), Channel::Pre, ASSET), None);
        assert_eq!(
            pick_latest(&json!({"message": "x"}), Channel::Pre, ASSET),
            None
        );
        // 只有 format 线
        assert_eq!(
            pick_latest(&json!([rel("format-v0.1.0", false)]), Channel::Pre, ASSET),
            None
        );
    }
}
