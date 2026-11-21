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

pub mod archive;
pub mod installed;
pub mod manifest;

use super::download::{self, Artifact, DownloadCtx};
use crate::install::state::SkillPhase;
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
    /// 单独执行 skill 更新时按失败码退出；随二进制安装的支线只记阶段。
    pub failure: Option<super::InstallFailure>,
}

/// 支线主任务：按链条下载 skills zip → 原子落位。失败重试已在链条级
/// （每级 3 次）——此处整体只跑一遍，失败即 failed（非强制，不拖主流程）。
pub async fn update_skills(
    ctx: &DownloadCtx,
    chain: &[download::ChainStep],
    home: &Path,
) -> SkillOutcome {
    // 下载目录独立于解包区（.staging）——install_skill_dir 开头会清空
    // .staging，若 zip 下载进同一目录会在解包前被自己删掉（实测暴露）
    let dl = home.join("skills").join(".dl");
    let attempt = 1u32;
    let result = download::fetch_artifact(ctx, chain, Artifact::Skills, &dl).await;
    let outcome = match result {
        Ok(fetched) => match archive::install(home, &fetched.path, &ctx.version) {
            Ok((manifest, ignored)) => {
                if ignored != 0 {
                    tracing::warn!(ignored, "已跳过 skill 包中未登记的条目");
                }
                tracing::info!(skills = ?manifest.names(), "skill 文档已随版本落位");
                SkillOutcome {
                    phase: SkillPhase::Done,
                    failure: None,
                    attempt,
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "skill 落位失败（下次 install 重试）");
                SkillOutcome {
                    phase: SkillPhase::Failed,
                    failure: Some(e.to_string().into()),
                    attempt,
                }
            }
        },
        Err(e) => {
            tracing::warn!(error = %e, "skill 下载失败（非强制，不影响安装；--skills-only 可单独重试）");
            SkillOutcome {
                phase: SkillPhase::Failed,
                failure: Some(e),
                attempt,
            }
        }
    };
    let _ = std::fs::remove_dir_all(&dl);
    outcome
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
