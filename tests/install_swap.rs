//! install 交换原语的端到端：唯一中间名（永久兼容契约 C2）、残留清理、核对失败
//! 回滚、Windows 入口兜底脚本。
//!
//! 全程在 tempdir 当 home 里演练：旧/新二进制都用 CARGO_BIN_EXE 副本（同版本无妨——
//! 测的是文件布局与改名步骤，不是版本差异）。

use std::path::{Path, PathBuf};
#[cfg(windows)]
use std::process::Command;

use aproxy::install::swap;

#[cfg(unix)]
fn make_executable(p: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(windows)]
fn make_executable(_p: &Path) {}

/// 备料一份二进制到 home 的 staging，返回 staged 路径。
fn stage(home: &Path, tag: &str) -> PathBuf {
    let from = home.join(format!("from-{tag}"));
    std::fs::copy(env!("CARGO_BIN_EXE_aproxy"), &from).unwrap();
    make_executable(&from);
    aproxy::install::staging::stage_from_in(home, &from, env!("CARGO_PKG_VERSION"))
        .unwrap()
        .path
}

/// 规范位置放一份「旧版本」：真二进制末尾追加一段标记（PE 与 ELF 都容得下尾部附加
/// 数据，照样能运行），交换之后据此分辨 bin 与让位的旧二进制各是哪一份。
fn seed_marked_bin(home: &Path) -> Vec<u8> {
    let bin = swap::bin_path_in(home);
    std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
    let mut bytes = std::fs::read(env!("CARGO_BIN_EXE_aproxy")).unwrap();
    bytes.extend_from_slice(b"\nOLD-BINARY-MARKER\n");
    std::fs::write(&bin, &bytes).unwrap();
    make_executable(&bin);
    bytes
}

/// 规范位置的二进制可运行且自报本构建的版本。
fn bin_works(home: &Path) -> bool {
    let bin = swap::bin_path_in(home);
    bin.is_file()
        && aproxy::install::staging::probe_version(&bin)
            .map(|v| v == env!("CARGO_PKG_VERSION"))
            .unwrap_or(false)
}

/// bin 里的中间文件残留（按文件名排序）
fn residues(home: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(swap::bin_dir_in(home))
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(swap::is_residue_name)
        })
        .collect();
    out.sort();
    out
}

fn swap_ok(home: &Path, staged: &Path) -> swap::SwapOutcome {
    swap::swap_in(home, staged, env!("CARGO_PKG_VERSION"), None, |_| Ok(())).unwrap()
}

/// 升级形态：旧二进制让位到唯一名并先报给 before_retire，新二进制就位，没有临时名
/// 残留；staged 原样保留（它是 swapping 中断的恢复源，由 cleaning 删）。
#[test]
fn swap_retires_the_old_binary_to_a_unique_name() {
    let home = tempfile::tempdir().unwrap();
    let old_bytes = seed_marked_bin(home.path());
    let staged = stage(home.path(), "upgrade");

    let mut announced = None;
    let outcome = swap::swap_in(
        home.path(),
        &staged,
        env!("CARGO_PKG_VERSION"),
        None,
        |old| {
            announced = Some(old.to_path_buf());
            Ok(())
        },
    )
    .unwrap();

    assert!(bin_works(home.path()), "落位后 bin 应可运行");
    let old = outcome.old_path.expect("bin 里原有二进制，应有让位的旧名");
    assert_eq!(Some(&old), announced.as_ref(), "让位之前就报给调用方");
    assert_eq!(old.parent(), Some(swap::bin_dir_in(home.path()).as_path()));
    assert_eq!(
        std::fs::read(&old).unwrap(),
        old_bytes,
        "旧名里是交换前的二进制"
    );
    assert_ne!(
        std::fs::read(swap::bin_path_in(home.path())).unwrap(),
        old_bytes,
        "bin 里应是新二进制"
    );
    assert_eq!(
        residues(home.path()),
        vec![old],
        "除了让位的旧二进制没有别的残留"
    );
    assert!(staged.is_file(), "staged 留给 cleaning 删");
}

/// 首次安装：bin 里没有二进制，不产生旧名，before_retire 不被调用。
#[test]
fn first_install_has_no_old_binary() {
    let home = tempfile::tempdir().unwrap();
    let staged = stage(home.path(), "fresh");
    let outcome = swap::swap_in(
        home.path(),
        &staged,
        env!("CARGO_PKG_VERSION"),
        None,
        |_| panic!("没有旧二进制，不该让位"),
    )
    .unwrap();
    assert_eq!(outcome.old_path, None);
    assert!(bin_works(home.path()));
    assert!(residues(home.path()).is_empty());
}

/// before_retire 拒绝（普通安装写状态失败）→ 什么都不动：bin 原样、没有残留。
#[test]
fn refusal_before_retiring_leaves_bin_untouched() {
    let home = tempfile::tempdir().unwrap();
    let old_bytes = seed_marked_bin(home.path());
    let staged = stage(home.path(), "refused");
    let err = swap::swap_in(
        home.path(),
        &staged,
        env!("CARGO_PKG_VERSION"),
        None,
        |_| Err("写不了状态".to_string()),
    )
    .unwrap_err();
    assert!(err.contains("写不了状态"), "{err}");
    assert_eq!(
        std::fs::read(swap::bin_path_in(home.path())).unwrap(),
        old_bytes
    );
    assert!(
        residues(home.path()).is_empty(),
        "{:?}",
        residues(home.path())
    );
}

/// 落位后核对版本不符 → 换回原来的二进制。
#[test]
fn version_mismatch_after_placing_rolls_back() {
    let home = tempfile::tempdir().unwrap();
    let old_bytes = seed_marked_bin(home.path());
    let staged = stage(home.path(), "mismatch");
    let err = swap::swap_in(home.path(), &staged, "99.0.0", None, |_| Ok(())).unwrap_err();
    assert!(err.contains("已换回原来的二进制"), "{err}");
    assert_eq!(
        std::fs::read(swap::bin_path_in(home.path())).unwrap(),
        old_bytes,
        "bin 应换回交换前的二进制"
    );
}

/// 上一次留下的残留：删得掉的清掉，keep 指名的保留（续作时回滚要用的旧二进制）。keep 在
/// 就是续作重入：旧二进制已经让位到 keep（unix 上 bin 仍是旧二进制、keep 是它的硬链接），
/// 不再让位出新的旧名，before_retire 也不再调用。
#[test]
fn residues_are_swept_except_the_kept_one() {
    let home = tempfile::tempdir().unwrap();
    let old_bytes = seed_marked_bin(home.path());
    let dir = swap::bin_dir_in(home.path());
    let stale_old = dir.join(swap::unique_name(swap::OLD_PREFIX));
    let stale_tmp = dir.join(swap::unique_name(swap::TMP_PREFIX));
    let kept = dir.join(swap::unique_name(swap::OLD_PREFIX));
    for p in [&stale_old, &stale_tmp] {
        std::fs::write(p, b"residue").unwrap();
    }
    std::fs::write(&kept, &old_bytes).unwrap();
    let staged = stage(home.path(), "sweep");
    let outcome = swap::swap_in(
        home.path(),
        &staged,
        env!("CARGO_PKG_VERSION"),
        Some(&kept),
        |_| -> Result<(), String> { panic!("续作重入不该再让位") },
    )
    .unwrap();
    assert!(bin_works(home.path()));
    assert!(!stale_old.exists() && !stale_tmp.exists(), "残留应被清掉");
    assert_eq!(outcome.old_path.as_deref(), Some(kept.as_path()));
    assert_eq!(std::fs::read(&kept).unwrap(), old_bytes, "keep 指名的不删");
    assert_eq!(residues(home.path()), vec![kept]);
}

/// 续作重入：上一次已经落位、只差推进相位时中断——bin 里已是新二进制，keep 是上一次让位
/// 的旧二进制。这一次不能再让位：那会把新二进制当成旧的记下来，之后的回滚拉起的仍是新
/// 版本。只核对，旧二进制仍是 keep。
#[test]
fn a_resumed_swap_after_placing_does_not_retire_the_new_binary() {
    let home = tempfile::tempdir().unwrap();
    let old_bytes = seed_marked_bin(home.path());
    let staged = stage(home.path(), "resume");
    let first = swap_ok(home.path(), &staged);
    let kept = first.old_path.expect("bin 里原有二进制，应有让位的旧名");
    let again = swap::swap_in(
        home.path(),
        &staged,
        env!("CARGO_PKG_VERSION"),
        Some(&kept),
        |_| -> Result<(), String> { panic!("续作重入不该再让位") },
    )
    .unwrap();
    assert_eq!(again.old_path.as_deref(), Some(kept.as_path()));
    assert_eq!(
        std::fs::read(&kept).unwrap(),
        old_bytes,
        "旧名里仍是交换前的二进制"
    );
    assert!(bin_works(home.path()));
    assert_eq!(residues(home.path()), vec![kept]);
}

/// 续作重入（Windows 的形态）：上一次让位之后、落位之前中断——bin 缺失，旧二进制在 keep。
/// 直接落位，旧二进制仍是 keep。
#[test]
fn a_resumed_swap_with_bin_missing_places_the_new_binary() {
    let home = tempfile::tempdir().unwrap();
    let old_bytes = seed_marked_bin(home.path());
    let bin = swap::bin_path_in(home.path());
    let kept = swap::bin_dir_in(home.path()).join(swap::unique_name(swap::OLD_PREFIX));
    std::fs::rename(&bin, &kept).unwrap();
    let staged = stage(home.path(), "resume-missing");
    let outcome = swap::swap_in(
        home.path(),
        &staged,
        env!("CARGO_PKG_VERSION"),
        Some(&kept),
        |_| -> Result<(), String> { panic!("续作重入不该再让位") },
    )
    .unwrap();
    assert!(bin_works(home.path()));
    assert_eq!(outcome.old_path.as_deref(), Some(kept.as_path()));
    assert_eq!(std::fs::read(&kept).unwrap(), old_bytes);
}

/// 唯一名的格式（永久兼容契约 C2）：`<前缀><纳秒>-<pid>[.exe]`，同一进程内不重复。
#[test]
fn unique_names_follow_the_contract_format() {
    for prefix in [swap::OLD_PREFIX, swap::TMP_PREFIX] {
        let name = swap::unique_name(prefix);
        let rest = name.strip_prefix(prefix).unwrap();
        let rest = if cfg!(windows) {
            rest.strip_suffix(".exe").expect(&name)
        } else {
            rest
        };
        let (nanos, pid) = rest.split_once('-').expect(&name);
        assert!(
            nanos.len() >= 19 && nanos.bytes().all(|b| b.is_ascii_digit()),
            "{name}"
        );
        assert_eq!(pid, std::process::id().to_string(), "{name}");
        assert_ne!(swap::unique_name(prefix), name);
    }
}

// ---------------------------------------------------------------------------
// Windows：被占用的残留、入口兜底脚本
// ---------------------------------------------------------------------------
#[cfg(windows)]
mod windows_swap {
    use super::*;

    /// 不带 FILE_SHARE_DELETE 打开（杀毒软件扫描、还在运行的旧镜像都是这种占法）：
    /// 持有期间文件删不掉、也改不了名
    fn hold_without_share_delete(path: &Path) -> std::fs::File {
        use std::os::windows::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0x1) // FILE_SHARE_READ
            .open(path)
            .unwrap()
    }

    /// 上一次让位的旧二进制还被占着（删不掉）：照样交换成功，它原样留着。
    #[test]
    fn a_held_residue_does_not_block_the_swap() {
        let home = tempfile::tempdir().unwrap();
        seed_marked_bin(home.path());
        let held = swap::bin_dir_in(home.path()).join(swap::unique_name(swap::OLD_PREFIX));
        std::fs::copy(env!("CARGO_BIN_EXE_aproxy"), &held).unwrap();
        let handle = hold_without_share_delete(&held);
        let staged = stage(home.path(), "held");
        let outcome = swap_ok(home.path(), &staged);
        assert!(bin_works(home.path()));
        assert!(held.exists(), "被占着的残留删不掉，跳过");
        assert_ne!(outcome.old_path.as_ref(), Some(&held));
        drop(handle);
    }

    /// bin 本身被不带共享删除权限的句柄占着：退避重试之后仍改不了名 → 失败、bin 原样。
    #[test]
    fn a_held_bin_fails_without_changing_anything() {
        let home = tempfile::tempdir().unwrap();
        let old_bytes = seed_marked_bin(home.path());
        let handle = hold_without_share_delete(&swap::bin_path_in(home.path()));
        let staged = stage(home.path(), "held-bin");
        let err = swap::swap_in(
            home.path(),
            &staged,
            env!("CARGO_PKG_VERSION"),
            None,
            |_| Ok(()),
        )
        .unwrap_err();
        assert!(err.contains("bin 未改动"), "{err}");
        drop(handle);
        assert_eq!(
            std::fs::read(swap::bin_path_in(home.path())).unwrap(),
            old_bytes
        );
        assert!(
            residues(home.path()).is_empty(),
            "{:?}",
            residues(home.path())
        );
    }

    /// 交换之前写好入口兜底脚本；`aproxy.exe` 缺席时 bat 转调最近一次让位的旧二进制
    /// （名字倒序第一个）——更早的那个是坏文件，选错就跑不起来。
    #[test]
    fn fallback_bat_runs_the_newest_retired_binary() {
        let home = tempfile::tempdir().unwrap();
        seed_marked_bin(home.path());
        let dir = swap::bin_dir_in(home.path());
        let staged = stage(home.path(), "fallback");
        swap_ok(home.path(), &staged);
        assert!(dir.join("aproxy.bat").is_file() && dir.join("aproxy").is_file());
        // 一个更早让位、已经坏掉的旧二进制（时间戳更小）。交换之后才放进来，免得被清残留
        // 顺手删掉
        std::fs::write(
            dir.join(format!("{}1000000000000000000-1.exe", swap::OLD_PREFIX)),
            b"not an executable",
        )
        .unwrap();

        // 模拟交换空窗：aproxy.exe 暂时缺席
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
        std::fs::rename(dir.join("aproxy.exe.away"), dir.join("aproxy.exe")).unwrap();
        assert!(
            out.status.success() && stdout.starts_with(aproxy::VERSION_LINE),
            "空窗期 bat 应转调最近一次让位的旧二进制: {stdout} {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

// ---------------------------------------------------------------------------
// unix：保留旧二进制（硬链接）供回滚
// ---------------------------------------------------------------------------

/// unix 的旧名必须是**交换前**的旧二进制（不是新文件的又一个链接），且可执行——
/// 滚动重启中实例在新版本下起不来时，就靠它按原参数拉回实例。连续两次交换各留一个
/// 唯一名，第二次交换时第一次的旧名作为残留被清掉。
#[cfg(unix)]
#[test]
fn unix_swap_keeps_the_previous_binary_for_rollback() {
    let home = tempfile::tempdir().unwrap();
    let bin = swap::bin_path_in(home.path());
    std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
    // 一个可辨识的「旧版本」：能回答 --version 的脚本
    std::fs::write(
        &bin,
        b"#!/bin/sh\necho 'aproxy v0.2.0-old (headless; fake)'\n",
    )
    .unwrap();
    make_executable(&bin);

    let staged = stage(home.path(), "upgrade");
    let first = swap_ok(home.path(), &staged).old_path.unwrap();
    assert_eq!(
        aproxy::install::staging::probe_version(&first).unwrap(),
        "0.2.0-old",
        "旧名应是交换前的旧二进制且可执行"
    );
    assert!(bin_works(home.path()), "bin 应为新二进制");

    let staged = stage(home.path(), "again");
    let second = swap_ok(home.path(), &staged).old_path.unwrap();
    assert_ne!(first, second);
    assert!(!first.exists(), "上一次的旧名作为残留被清掉");
    assert_eq!(
        aproxy::install::staging::probe_version(&second).unwrap(),
        env!("CARGO_PKG_VERSION")
    );
}
