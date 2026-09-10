//! install staging 备料的端到端校验（真实 aproxy 二进制 `--version` 试跑）。
//!
//! lib 单测覆盖纯逻辑（sha256 向量/路径派生/缺失源拒绝）；此处用
//! CARGO_BIN_EXE_aproxy 走完整 `--from` 备料链：复制 → chmod（unix）→
//! sha256 → 试跑校验，版本不匹配必须拒绝且清理 staged 副本。
//!
//! 隔离：APROXY_HOME 相关函数全部显式传 tempdir，绝不触碰真实 ~/.aproxy。

use std::path::Path;

#[cfg(unix)]
fn make_executable(p: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(windows)]
fn make_executable(_p: &Path) {}

#[test]
fn stage_from_real_binary_end_to_end() {
    let home = tempfile::tempdir().unwrap();
    let from = home.path().join("from").join("aproxy-copy");
    std::fs::create_dir_all(from.parent().unwrap()).unwrap();
    std::fs::copy(env!("CARGO_BIN_EXE_aproxy"), &from).unwrap();
    // 集成环境里 CARGO_BIN_EXE 副本默认可执行；unix 显式补齐对齐真实场景
    make_executable(&from);

    let target = env!("CARGO_PKG_VERSION");
    let staged = aproxy::install::staging::stage_from_in(home.path(), &from, target).unwrap();

    // staged 副本落在规范 staging 布局下且内容完整
    let expected_dir = home.path().join("staging").join(target);
    assert_eq!(
        staged.path.parent().unwrap(),
        expected_dir,
        "staged 应落在 <home>/staging/<版本>/"
    );
    assert!(staged.path.is_file());
    // 摘要与磁盘内容一致（防 TOCTOU 式的记录与实体脱节）
    assert_eq!(
        staged.sha256,
        aproxy::install::staging::sha256_hex(&staged.path).unwrap()
    );
    // staged 副本自身可运行且自报目标版本
    assert_eq!(
        aproxy::install::staging::probe_version(&staged.path).unwrap(),
        target
    );
}

#[test]
fn stage_from_rejects_version_mismatch_and_cleans_up() {
    let home = tempfile::tempdir().unwrap();
    let from = home.path().join("aproxy-copy");
    std::fs::copy(env!("CARGO_BIN_EXE_aproxy"), &from).unwrap();
    make_executable(&from);

    let err = aproxy::install::staging::stage_from_in(home.path(), &from, "0.0.0-definitely-not")
        .unwrap_err();
    assert!(err.contains("版本不匹配"), "{err}");
    // 不带病留在 staging：拒绝时已清理 staged 副本
    let leftover = home
        .path()
        .join("staging")
        .join("0.0.0-definitely-not")
        .join(aproxy::install::staging::binary_name());
    assert!(!leftover.exists(), "被拒的 staged 副本应已清理");
}
