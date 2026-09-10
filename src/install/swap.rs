//! 交换原语：staging → bin 的二进制落位（跨平台对照见 `.agents/plan/install-v1.md`）。
//!
//! 两套语义，状态机/恢复逻辑平台无关，平台分支只收口在本模块：
//!
//! - **Windows**：镜像锁（禁写删、允许 rename）。copy 新 exe → `aproxy.exe.new`
//!   → rename 旧 → `.old` → rename `.new` → `aproxy.exe`。双 rename 之间
//!   「bin 完全无二进制」窗口缩到一次系统调用（防线 1）；`.old` 固定名保留
//!   （fallback 执行目标，保持 exe 后缀）；入口脚本常驻（防线 0，PATHEXT
//!   保证 exe 在场时零参与）。
//! - **unix**：只锁 inode。staging 文件单步 rename 原子覆盖 bin/aproxy——
//!   不存在空窗（无需 fallback 脚本）、无 `.old`（旧 inode 挂在运行中进程上
//!   自动消亡）、安装进程无需换镜像（直接续跑，无 relaying）。

use std::path::{Path, PathBuf};

/// 安装二进制的唯一管辖目录：`<home>/bin/`。
pub fn bin_dir_in(home: &Path) -> PathBuf {
    home.join("bin")
}

/// 规范位置的二进制全路径。
pub fn bin_path_in(home: &Path) -> PathBuf {
    bin_dir_in(home).join(crate::install::staging::binary_name())
}

/// Windows：旧二进制固定名（保持 exe 后缀使其可作为 fallback 执行目标；
/// 镜像锁只禁写删不禁运行）。只保留最近 1 份，连续升级互相覆盖。unix 不存在。
pub fn old_path_in(home: &Path) -> PathBuf {
    bin_dir_in(home).join("aproxy.old.exe")
}

/// Windows：bin 下的临时名（copy 中间态，不占镜像锁；它的存在本身就是
/// 「swapping 进行中」的标记——恢复检测的现场特征之一）。unix 不存在。
pub fn new_tmp_path_in(home: &Path) -> PathBuf {
    bin_dir_in(home).join("aproxy.exe.new")
}

/// 交换结果：旧二进制去向（Windows = `.old` 路径；unix = None，进
/// install.state.old_path 供 cleaning 删除）。
#[derive(Debug, Clone, PartialEq)]
pub struct SwapOutcome {
    pub old_path: Option<PathBuf>,
}

/// 统一交换入口（平台收口点）。调用前置条件：staged 已备料并通过校验
/// （铁律 2）；bin 里可能没有旧二进制（首次安装）——两种情况都支持。
pub fn swap_in(home: &Path, staged: &Path) -> Result<SwapOutcome, String> {
    #[cfg(windows)]
    {
        swap_windows_in(home, staged)
    }
    #[cfg(not(windows))]
    {
        swap_unix_in(home, staged)
    }
}

// ---------------------------------------------------------------------------
// Windows：copy + 双 rename 舞
// ---------------------------------------------------------------------------

#[cfg(windows)]
fn swap_windows_in(home: &Path, staged: &Path) -> Result<SwapOutcome, String> {
    let bin = bin_path_in(home);
    let old = old_path_in(home);
    let new_tmp = new_tmp_path_in(home);
    let dir = bin_dir_in(home);
    std::fs::create_dir_all(&dir).map_err(|e| format!("bin 目录创建失败: {e}"))?;

    // 防线 0：fallback 入口脚本幂等 ensure（只靠 swap 后放来不及保护本次空窗）
    ensure_fallback_scripts_in(home).map_err(|e| format!("fallback 脚本写入失败: {e}"))?;

    // 1. copy staged → .new（不占镜像锁，副本本身完整可见——rename 原子性
    //    保证脚本永远调到完整文件，不存在半成品态）
    std::fs::copy(staged, &new_tmp).map_err(|e| format!("staged 复制到 bin 失败: {e}"))?;

    // 2. rename 旧 → .old。bin 里没有旧二进制（首次安装）则跳过。
    //    .old 残留（上次 cleaning 失败）先清位：删除失败（被锁）→ rename
    //    走开让位；两者皆败 → abort，此刻 bin 尚未动过，现场无损
    if bin.exists() {
        if old.exists() {
            clear_residual_old(&old)?;
        }
        std::fs::rename(&bin, &old).map_err(|e| {
            let _ = std::fs::remove_file(&new_tmp);
            format!("旧二进制 rename 失败（被占用？）: {e}")
        })?;
    }

    // 3. rename .new → aproxy.exe（窗口从此刻起才存在，且只持续一次系统调用）
    std::fs::rename(&new_tmp, &bin).map_err(|e| format!("新二进制落位失败: {e}"))?;

    // 4. 落位验证（staged 已验证过同内容，此处防 copy/rename 环节意外；
    //    失败回滚——运行中的旧安装进程自身镜像允许再次 rename）
    if let Err(e) = crate::install::staging::probe_version(&bin) {
        let _ = std::fs::rename(&bin, &new_tmp);
        if old.exists() {
            let _ = std::fs::rename(&old, &bin);
        }
        return Err(format!("落位验证失败（已回滚）: {e}"));
    }

    Ok(SwapOutcome {
        old_path: old.exists().then_some(old),
    })
}

/// `.old` 残留清位：删除失败（上次升级的进程仍锁着它）→ rename 走开让位
/// （带时间戳，永不冲突）；再失败 → 报错（bin 未动，现场无损，可重试）。
#[cfg(windows)]
fn clear_residual_old(old: &Path) -> Result<(), String> {
    match std::fs::remove_file(old) {
        Ok(()) => Ok(()),
        Err(remove_err) => {
            let aside = old.with_extension(format!(
                "exe.old-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0)
            ));
            std::fs::rename(old, &aside)
                .map_err(|e| format!(".old 残留清位失败（删除: {remove_err}，让位: {e}）"))
        }
    }
}

// ---------------------------------------------------------------------------
// unix：单步 rename 原子覆盖
// ---------------------------------------------------------------------------

#[cfg(not(windows))]
fn swap_unix_in(home: &Path, staged: &Path) -> Result<SwapOutcome, String> {
    let bin = bin_path_in(home);
    std::fs::create_dir_all(bin_dir_in(home)).map_err(|e| format!("bin 目录创建失败: {e}"))?;
    // staging 与 bin 同在 home 下（同卷），rename 原子覆盖成立；旧 inode 由
    // 运行中进程挂着自动消亡——无空窗、无 .old、无需清位
    std::fs::rename(staged, &bin).map_err(|e| format!("新二进制落位失败: {e}"))?;
    crate::install::staging::ensure_executable(&bin);
    // 落位验证：失败无回滚必要（旧文件已被覆盖，但旧 inode 语义下 bin 内容
    // 就是新文件——验证失败 = staged 本身坏，stage_from 已拦；此处只报错）
    if let Err(e) = crate::install::staging::probe_version(&bin) {
        return Err(format!("落位验证失败: {e}"));
    }
    Ok(SwapOutcome { old_path: None })
}

// ---------------------------------------------------------------------------
// 防线 0：安全中间态入口脚本（Windows 专属，unix 无空窗即无 fallback）
// ---------------------------------------------------------------------------

/// 幂等确保 fallback 入口脚本在场（bootstrap 首装 + install 每次 swapping
/// 前都调；内容固定，重复写覆盖为同一内容）。`aproxy.ps1` 不做——PowerShell
/// 命令发现不含 .ps1，无价值。
#[cfg(windows)]
pub fn ensure_fallback_scripts_in(home: &Path) -> std::io::Result<()> {
    let dir = bin_dir_in(home);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("aproxy.bat"), FALLBACK_BAT)?;
    std::fs::write(dir.join("aproxy"), FALLBACK_SH)?;
    Ok(())
}

/// cmd 主力：`%~dp0` 相对定位，exe 在场调 exe（常态 PATHEXT 优先 .EXE，
/// 本脚本零参与），空窗 fallback `.old.exe`（被旧安装进程锁定也只禁写删
/// 不禁运行）。
#[cfg(windows)]
const FALLBACK_BAT: &str = r#"@echo off
rem aProxy safe-entry: PATHEXT resolves .EXE before .BAT, so this script
rem only runs during the upgrade gap (aproxy.exe missing) and falls back
rem to the previous binary.
if exist "%~dp0aproxy.exe" (
  "%~dp0aproxy.exe" %*
) else if exist "%~dp0aproxy.old.exe" (
  "%~dp0aproxy.old.exe" %*
) else (
  echo aproxy.exe not found in %~dp0 - upgrade in progress or broken install
  exit /b 1
)
"#;

/// Git Bash/MSYS 补充（无后缀——PATH 解析能命中；cmd/PowerShell 不会调它）。
#[cfg(windows)]
const FALLBACK_SH: &str = r#"#!/bin/sh
# aProxy safe-entry (Git Bash/MSYS): prefer the real binary, fall back to
# the previous one during the upgrade gap.
dir=$(dirname "$0")
if [ -x "$dir/aproxy.exe" ]; then
  exec "$dir/aproxy.exe" "$@"
elif [ -x "$dir/aproxy.old.exe" ]; then
  exec "$dir/aproxy.old.exe" "$@"
else
  echo "aproxy.exe not found in $dir - upgrade in progress or broken install" >&2
  exit 1
fi
"#;

// ---------------------------------------------------------------------------
// 接力（Windows 无 exec 的替代；unix 跳过 relaying 直接续跑）
// ---------------------------------------------------------------------------

/// 旧安装进程（跑在 .old 镜像上）spawn 新二进制 `install --continue` 续跑，
/// 返回新进程 pid。环境经 spawn 继承——APROXY_HOME 重定向自动传播，
/// 新进程定位到同一份 install.state。
#[cfg(windows)]
pub fn spawn_continuator(new_bin: &Path) -> std::io::Result<u32> {
    crate::daemon::spawn_detached(new_bin, &["install".to_string(), "--continue".to_string()])
}

/// 旧进程确认接管：轮询状态文件 phase 已推进到 relaying 及以后（新进程把
/// phase 写进状态文件的动作与 updated_at 刷新是同一次原子写——推进本身就是
/// 自证存活）。确认后旧进程自行退出（=「旧二进制自动停止」）。phase 停在
/// relaying 但新进程随后死亡属于「relaying 中断」恢复场景，由看门狗/CLI
/// 兜底拉起 --continue，不是接力确认的职责。
#[cfg(windows)]
pub fn wait_for_takeover(run_dir: &Path, deadline: std::time::Instant) -> bool {
    use crate::install::state::{InstallPhase, load_in};
    let takeover_reached = |p: InstallPhase| {
        matches!(
            p,
            InstallPhase::Relaying
                | InstallPhase::Restarting
                | InstallPhase::Verifying
                | InstallPhase::Cleaning
                | InstallPhase::Done
        )
    };
    loop {
        if std::time::Instant::now() >= deadline {
            return false;
        }
        if let Some(s) = load_in(run_dir)
            && takeover_reached(s.phase)
        {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bin_layout_paths() {
        let home = Path::new("/tmp/fake-home");
        assert_eq!(bin_dir_in(home), home.join("bin"));
        if cfg!(windows) {
            assert_eq!(bin_path_in(home), home.join("bin").join("aproxy.exe"));
            assert_eq!(old_path_in(home), home.join("bin").join("aproxy.old.exe"));
            assert_eq!(
                new_tmp_path_in(home),
                home.join("bin").join("aproxy.exe.new")
            );
        } else {
            assert_eq!(bin_path_in(home), home.join("bin").join("aproxy"));
        }
    }
}
