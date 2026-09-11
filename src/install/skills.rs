//! skill 更新支线（非强制）：install 时随二进制**并行下载更新**
//! `~/.aproxy/skills/aproxy-cli/`。
//!
//! 非强制语义（用户定调）：skill 不是二进制的依赖——下载失败重试后放弃，
//! **安装照常成功**，主流程任何阶段都不等待/不受影响（唯一同步点：install
//! 进程结束前 join 收尾并落终态）。下载是幂等覆盖操作，不建断电恢复状态机
//! （半截文件下次重下即愈）；`--continue` 续作时 failed 不再自动重试
//! （避免每次续作都拖一遍下载），`--skills-only` 手动重试。
//!
//! 落位 = 目录原子替换：解压到 staging 子目录 → 旧目录 rename 走 → 新目录
//! rename 进 → 删旧；Windows 上 agent 正读 skill 文件的冲突短重试。

use super::download::{self, Artifact, DownloadCtx};
use crate::install::state::{SkillPhase, SkillState};
use std::path::{Path, PathBuf};

/// skill 安装目录：`~/.aproxy/skills/aproxy-cli/`（agent 侧目录归各 agent
/// 管，install 只落规范位置——链接/复制到 agent 的 skills 目录由用户/agent
/// 自行执行，落位后输出提示）。
pub fn skill_dir_in(home: &Path) -> PathBuf {
    home.join("skills").join("aproxy-cli")
}

/// 支线任务结果（终态，写入 install.state.skill）。
#[derive(Debug)]
pub struct SkillOutcome {
    pub phase: SkillPhase,
    pub attempt: u32,
}

/// 支线主任务：按链条下载 skills zip → 原子落位。失败重试已在链条级
/// （每级 3 次）——此处整体只跑一遍，失败即 failed（非强制，不拖主流程）。
pub async fn update_skills(
    ctx: &DownloadCtx,
    chain: &[download::ChainStep],
    home: &Path,
) -> SkillOutcome {
    let staging = home.join("skills").join(".staging");
    let attempt = 1u32;
    match download::fetch_artifact(ctx, chain, Artifact::Skills, &staging).await {
        Ok(fetched) => match install_skill_dir(home, &fetched.path) {
            Ok(()) => SkillOutcome {
                phase: SkillPhase::Done,
                attempt,
            },
            Err(e) => {
                tracing::warn!(error = %e, "skill 落位失败（下次 install 重试）");
                SkillOutcome {
                    phase: SkillPhase::Failed,
                    attempt,
                }
            }
        },
        Err(e) => {
            tracing::warn!(error = %e, "skill 下载失败（非强制，不影响安装；--skills-only 可单独重试）");
            SkillOutcome {
                phase: SkillPhase::Failed,
                attempt,
            }
        }
    }
}

/// zip 落位为 skill 目录（原子替换）：解包到 .staging/<名>/ → 旧目录 rename
/// 走 → 新目录 rename 进 → 清理。Windows 上 agent 正读文件导致的 rename
/// 冲突短重试 3 次，失败放弃（下次 install 再覆盖）。
fn install_skill_dir(home: &Path, zip: &Path) -> Result<(), String> {
    let staging = home.join("skills").join(".staging").join("unpack");
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| format!("skill staging 创建失败: {e}"))?;
    unpack_zip(zip, &staging)?;

    let dir = skill_dir_in(home);
    std::fs::create_dir_all(dir.parent().unwrap())
        .map_err(|e| format!("skills 目录创建失败: {e}"))?;
    // 旧目录 rename 走（Windows 冲突短重试）
    let old = home.join("skills").join(".staging").join("old");
    let _ = std::fs::remove_dir_all(&old);
    if dir.exists() {
        let mut renamed = false;
        for attempt in 0..3 {
            if std::fs::rename(&dir, &old).is_ok() {
                renamed = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(300 * (attempt as u64 + 1)));
        }
        if !renamed {
            let _ = std::fs::remove_dir_all(&staging);
            return Err("旧 skill 目录被占用（agent 正读？），放弃本次覆盖".into());
        }
    }
    // 新目录 rename 进（同卷原子）
    std::fs::rename(&staging, &dir).map_err(|e| format!("skill 目录落位失败: {e}"))?;
    let _ = std::fs::remove_dir_all(&old);
    let _ = std::fs::remove_dir_all(home.join("skills").join(".staging"));
    Ok(())
}

/// zip 解包（**路径穿越防护**：entry 名剥盘符/绝对前缀后拼接，`..` 段
/// 拒绝——zip 是系统边界输入，恶意条目不得逃出目标目录）。
fn unpack_zip(zip_path: &Path, dest: &Path) -> Result<(), String> {
    let f = std::fs::File::open(zip_path).map_err(|e| format!("zip 打开失败: {e}"))?;
    let mut archive = zip::ZipArchive::new(f).map_err(|e| format!("zip 解析失败: {e}"))?;
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| format!("zip 条目读取失败: {e}"))?;
        let raw = entry.name().to_string();
        // 归一路径：拒绝绝对路径与 ..
        let rel = raw.replace('\\', "/");
        if rel.starts_with('/') || rel.contains(':') || rel.split('/').any(|seg| seg == "..") {
            return Err(format!("zip 内含非法路径条目: {raw}（疑似路径穿越）"));
        }
        let target = dest.join(&rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&target).map_err(|e| format!("目录创建失败: {e}"))?;
        } else {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("目录创建失败: {e}"))?;
            }
            let mut out = std::fs::File::create(&target)
                .map_err(|e| format!("{} 创建失败: {e}", target.display()))?;
            std::io::copy(&mut entry, &mut out).map_err(|e| format!("zip 条目提取失败: {e}"))?;
        }
    }
    Ok(())
}

/// skill 子状态初值（安装开始时主流程写入 install.state——中途可观察
/// 「下载中」；终态由主流程 join 后统一落盘，避免与主流程并发写状态文件）。
pub fn initial_skill_state(version: &str) -> SkillState {
    SkillState {
        status: SkillPhase::Downloading,
        attempt: 1,
        version: version.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_dir_is_home_derived() {
        assert_eq!(
            skill_dir_in(Path::new("/tmp/h")),
            Path::new("/tmp/h/skills/aproxy-cli")
        );
    }

    #[test]
    fn unpack_zip_rejects_path_traversal() {
        // 手工构造含 .. 条目的 zip（内存中写 zip）
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("evil.zip");
        let f = std::fs::File::create(&zip_path).unwrap();
        let mut w = zip::ZipWriter::new(f);
        w.start_file("../evil.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        std::io::Write::write_all(&mut w, b"payload").unwrap();
        w.finish().unwrap();
        let dest = dir.path().join("out");
        std::fs::create_dir_all(&dest).unwrap();
        let err = unpack_zip(&zip_path, &dest).unwrap_err();
        assert!(err.contains("路径穿越"), "{err}");
        assert!(!dir.path().join("evil.txt").exists(), "穿越文件不得落盘");
    }
}
