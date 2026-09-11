//! install 交换原语的端到端（Windows 双 rename 舞 + fallback 入口脚本）。
//!
//! 全程在 tempdir APROXY_HOME 里演练：旧/新二进制都用 CARGO_BIN_EXE 副本
//! （同版本无妨——测的是舞步与文件布局，不是版本差异）。unix 单步 rename
//! 分支 CI check 覆盖编译面，行为面入 TODO unix 实测项。

use std::path::Path;
#[cfg(windows)]
use std::process::Command;

#[cfg(unix)]
fn make_executable(p: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(windows)]
fn make_executable(_p: &Path) {}

/// 备料一份二进制到 home 的 staging，返回 staged 路径。
fn stage(home: &Path, tag: &str) -> std::path::PathBuf {
    let from = home.join(format!("from-{tag}"));
    std::fs::copy(env!("CARGO_BIN_EXE_aproxy"), &from).unwrap();
    make_executable(&from);
    aproxy::install::staging::stage_from_in(home, &from, env!("CARGO_PKG_VERSION"))
        .unwrap()
        .path
}

/// 规范位置的二进制可运行且自报 aproxy 版本。
fn bin_works(home: &Path) -> bool {
    let bin = aproxy::install::swap::bin_path_in(home);
    bin.is_file()
        && aproxy::install::staging::probe_version(&bin)
            .map(|v| v == env!("CARGO_PKG_VERSION"))
            .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Windows：copy + 双 rename 舞（防线 1）+ .old 固定名 + fallback 脚本（防线 0）
// ---------------------------------------------------------------------------
#[cfg(windows)]
mod windows_swap {
    use super::*;
    use aproxy::install::swap;

    #[test]
    fn swap_over_existing_binary_full_choreography() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join("bin");
        std::fs::create_dir_all(&dir).unwrap();
        // 旧二进制在场（上次安装的形态）
        let old_bin = dir.join("aproxy.exe");
        std::fs::copy(env!("CARGO_BIN_EXE_aproxy"), &old_bin).unwrap();

        let staged = stage(home.path(), "upgrade");
        let outcome = swap::swap_in(home.path(), &staged).unwrap();

        // 双 rename 舞后的终态：bin = 新、.old = 旧、.new 消失、脚本在场
        assert!(bin_works(home.path()), "落位后 aproxy.exe 应可运行");
        assert_eq!(outcome.old_path, Some(dir.join("aproxy.old.exe")));
        assert!(dir.join("aproxy.old.exe").is_file(), "旧二进制应为 .old");
        assert!(!dir.join("aproxy.exe.new").exists(), ".new 应已消费");
        assert!(dir.join("aproxy.bat").is_file(), "fallback bat 应常驻");
        assert!(dir.join("aproxy").is_file(), "fallback sh 应常驻");
    }

    #[test]
    fn first_install_no_old_binary() {
        let home = tempfile::tempdir().unwrap();
        let staged = stage(home.path(), "fresh");
        let outcome = swap::swap_in(home.path(), &staged).unwrap();
        assert_eq!(outcome.old_path, None, "首次安装无 .old");
        assert!(bin_works(home.path()));
        assert!(!home.path().join("bin").join("aproxy.old.exe").exists());
    }

    #[test]
    fn residual_old_from_previous_failed_cleaning_is_cleared() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join("bin");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::copy(env!("CARGO_BIN_EXE_aproxy"), dir.join("aproxy.exe")).unwrap();
        // 上次 cleaning 失败残留的 .old（非锁定文件，直接删得掉）
        std::fs::write(dir.join("aproxy.old.exe"), b"stale-residual").unwrap();

        let staged = stage(home.path(), "residual");
        swap::swap_in(home.path(), &staged).unwrap();
        assert!(bin_works(home.path()));
        // 残留被清位、新 .old 是刚退位的旧二进制（不再是 stale 内容）
        let old = std::fs::read(dir.join("aproxy.old.exe")).unwrap();
        assert_ne!(old, b"stale-residual", "残留 .old 应被清走");
        assert!(old.len() > 1000, "新 .old 应为刚退位的真实二进制");
    }

    #[test]
    fn fallback_bat_serves_old_binary_when_exe_missing() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join("bin");
        std::fs::create_dir_all(&dir).unwrap();
        // 升级形态：旧二进制在场——swap 后 .old 才有 fallback 目标
        std::fs::copy(env!("CARGO_BIN_EXE_aproxy"), dir.join("aproxy.exe")).unwrap();
        let staged = stage(home.path(), "fallback");
        swap::swap_in(home.path(), &staged).unwrap();
        assert!(dir.join("aproxy.old.exe").is_file());

        // 模拟 swapping 空窗：aproxy.exe 暂时缺席（.old 在场）——bat 应调到 .old
        std::fs::rename(dir.join("aproxy.exe"), dir.join("aproxy.exe.away")).unwrap();
        let out = Command::new("cmd")
            .args([
                "/c",
                &dir.join("aproxy.bat").display().to_string(),
                "--version",
            ])
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success() && stdout.contains(env!("CARGO_PKG_VERSION")),
            "空窗期 fallback bat 应调到 .old 并输出版本: {stdout}"
        );
        std::fs::rename(dir.join("aproxy.exe.away"), dir.join("aproxy.exe")).unwrap();
    }
}

// ---------------------------------------------------------------------------
// 通用面：规范布局（unix 分支行为实测入 TODO，CI check 覆盖编译面）
// ---------------------------------------------------------------------------
#[test]
fn swap_lays_out_canonical_layout() {
    let home = tempfile::tempdir().unwrap();
    // 升级形态：旧二进制在场（.old 语义有意义的场景）
    let dir = home.path().join("bin");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(
        env!("CARGO_BIN_EXE_aproxy"),
        aproxy::install::swap::bin_path_in(home.path()),
    )
    .unwrap();
    make_executable(&aproxy::install::swap::bin_path_in(home.path()));

    let staged = stage(home.path(), "layout");
    let outcome = aproxy::install::swap::swap_in(home.path(), &staged).unwrap();
    assert!(bin_works(home.path()));
    #[cfg(windows)]
    assert!(outcome.old_path.is_some());
    #[cfg(not(windows))]
    {
        // unix：无 .old 概念；staged 被 rename 走（同一 inode 挂到 bin）
        assert_eq!(outcome.old_path, None);
        assert!(!staged.exists());
    }
}
