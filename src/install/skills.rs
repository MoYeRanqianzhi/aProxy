//! skill 更新支线（非强制）：install 时随二进制**并行下载更新**
//! `~/.aproxy/skills/aproxy-cli/`。
//!
//! 非强制语义（用户定调）：skill 不是二进制的依赖——下载失败重试后放弃，
//! **安装照常成功**，主流程任何阶段都不等待/不受影响（唯一同步点：install
//! 进程结束前 join 收尾并落终态）。下载是幂等覆盖操作，不建断电恢复状态机
//! （半截文件下次重下即愈）；`--continue` 续作时 failed 不再自动重试
//! （避免每次续作都拖一遍下载），`aproxy skill install` 手动重试。
//!
//! 落位 = 目录原子替换：解压到 staging 子目录 → 旧目录 rename 走 → 新目录
//! rename 进 → 删旧；Windows 上 agent 正读 skill 文件的冲突短重试。

pub mod archive;
pub mod installed;
pub mod manifest;

use super::download::{self, Artifact, DownloadCtx};
use std::path::Path;

/// 从下载开始直到发布完成一直持有独立的文件锁；进程退出后内核释放，不按 pid 猜测过期。
fn skill_lock(home: &Path) -> Result<super::InstallLock, String> {
    let root = home.join("skills");
    crate::fsx::create_dir_all(&root)
        .and_then(|()| installed::directory(&root))
        .map_err(|e| e.to_string())?;
    super::try_lock_path(&root.join(".lock"))
        .map_err(|e| format!("skill 加锁失败：{e}"))?
        .ok_or_else(|| "另一个进程正在安装 skill，请稍后重试".into())
}

/// 只动 skill 的安装入口：下载与解包都在同卷独占的工作区，残留的半成品不复用也不清理。
pub async fn install(
    ctx: &DownloadCtx,
    chain: &[download::ChainStep],
    home: &Path,
) -> Result<usize, super::InstallFailure> {
    let _lock = skill_lock(home)?;
    let dl = tempfile::Builder::new()
        .prefix(".staging-dl-")
        .tempdir_in(home.join("skills"))
        .map_err(|e| format!("skill 下载目录创建失败：{e}"))?;
    let fetched = download::fetch_artifact(ctx, chain, Artifact::Skills, dl.path()).await?;
    let (manifest, ignored) = archive::install(home, &fetched.path, &ctx.version)
        .map_err(|e| format!("skill 安装失败：{e}"))?;
    if ignored != 0 {
        tracing::warn!(ignored, "已跳过 skill 包中未登记的条目");
    }
    tracing::info!(skills=?manifest.names(),"skill 文档已随版本落位");
    Ok(manifest.files.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 随发布出去的每个 skill（仓库的 `.claude/skills/*/SKILL.md`），description 都不超过
    /// 1024 个字符：这是 Agent Skills 规范与 skill-creator 校验的上限，按上限校验的客户端
    /// 会拒收或截断，整份 skill 可能加载失败。description 只写成单行（`description: …`）。
    #[test]
    fn shipped_skill_descriptions_fit_the_1024_character_limit() {
        let skills = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(".claude")
            .join("skills");
        let mut checked = 0;
        for dir in std::fs::read_dir(&skills).unwrap().flatten() {
            let doc_path = dir.path().join("SKILL.md");
            let doc = std::fs::read_to_string(&doc_path).unwrap();
            let description = doc
                .lines()
                .find_map(|line| line.strip_prefix("description:"))
                .unwrap_or_else(|| panic!("{} 没有单行的 description", doc_path.display()))
                .trim();
            let chars = description.chars().count();
            assert!(
                chars <= 1024,
                "{} 的 description 有 {chars} 个字符，超过 1024",
                doc_path.display()
            );
            checked += 1;
        }
        assert!(checked >= 2, "应至少检查 aproxy-cli 与 aproxy-format");
    }
}

#[cfg(test)]
mod archive_tests;
