//! github 通道（默认链第 1 级）：GitHub Releases 资产直下 + `.sha256`
//! 强校验（信任锚 = 仓库所有者的 release 身份）。
//!
//! 列表用 releases API 取**第一个** release 而非 releases/latest——<1.0
//! 时代整线是 prerelease，latest 端点在仅有 prerelease 时会落空（计划
//! 版本与变体节）；指定版本时按 tag `v<version>` 精确定位。

use super::{Artifact, DownloadCtx, Fetched, download_to};
use std::path::Path;

const REPO: &str = "MoYeRanqianzhi/aProxy";

/// release 的 tag 与资产（name → browser_download_url）。
#[derive(Debug)]
struct Release {
    tag: String,
    assets: Vec<(String, String)>,
}

/// 列出 releases（匿名 API，仓库公开，60/h 限流够用）。
/// `tag` 为 None 时取列表第一个（版本序，含 prerelease）。
async fn list_releases(ctx: &DownloadCtx, tag: Option<&str>) -> Result<Release, String> {
    let url = match tag {
        Some(t) => format!("https://api.github.com/repos/{REPO}/releases/tags/v{t}"),
        None => format!("https://api.github.com/repos/{REPO}/releases"),
    };
    let resp = ctx
        .client
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("GitHub API 请求失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("GitHub API HTTP {}", resp.status()));
    }
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("GitHub API 解析失败: {e}"))?;
    // tags 端点返回单对象；列表端点返回数组（取第一个）
    let item = if body.is_array() {
        body.as_array()
            .and_then(|a| a.first())
            .cloned()
            .ok_or_else(|| "GitHub Releases 列表为空".to_string())?
    } else {
        body
    };
    let tag = item["tag_name"]
        .as_str()
        .ok_or("release 缺 tag_name")?
        .strip_prefix('v')
        .unwrap_or_default()
        .to_string();
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
    Ok(Release { tag, assets })
}

/// github 通道获取：下载 asset + `<asset>.sha256` 比对（强校验，不匹配即拒）。
pub async fn fetch(ctx: &DownloadCtx, artifact: Artifact, dest: &Path) -> Result<Fetched, String> {
    let asset = artifact.asset_name(ctx.target, ctx.variant);
    let release = list_releases(ctx, Some(&ctx.version)).await?;
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

/// 查询最新版本（tags 列表第一个；供 `install latest` / `--skills-only` 无参）。
pub async fn latest_version(ctx: &DownloadCtx) -> Result<String, String> {
    let release = list_releases(ctx, None).await?;
    if release.tag.is_empty() {
        return Err("未能从 GitHub Releases 解析最新版本".to_string());
    }
    Ok(release.tag)
}
