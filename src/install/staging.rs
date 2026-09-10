//! staging 备料区：新二进制在落位 bin/ 之前的完整落盘与校验。
//!
//! 铁律 2（先下载后 rename）：任何文件交换之前，新二进制必须已完整落盘
//! staging 并通过校验。staging 与 bin 同在 APROXY_HOME 下——Windows rename
//! 原子性的同卷前提；备料文件保留至 done/abort 才清理（它是 swapping 中断
//! 的恢复源）。

use std::path::{Path, PathBuf};

/// staging 备料目录：`<home>/staging/<目标版本>/`。
pub fn staging_dir(target_version: &str) -> PathBuf {
    staging_dir_in(&crate::settings::home(), target_version)
}

/// 同上，主目录可指定（测试注入用）。
pub fn staging_dir_in(home: &Path, target_version: &str) -> PathBuf {
    home.join("staging").join(target_version)
}

/// 目标平台的二进制文件名（unix 无 .exe 后缀）。
pub fn binary_name() -> &'static str {
    if cfg!(windows) {
        "aproxy.exe"
    } else {
        "aproxy"
    }
}

/// sha256 十六进制摘要。校验通用件：github 渠道 `.sha256` 资产强校验、
/// npm integrity（sha512 另算）换算的底座、`--from` 的完整性记录。
pub fn sha256_hex(path: &Path) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let mut file =
        std::fs::File::open(path).map_err(|e| format!("{} 无法读取: {e}", path.display()))?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)
        .map_err(|e| format!("{} 读取失败: {e}", path.display()))?;
    let digest = hasher.finalize();
    Ok(digest.iter().map(|b| format!("{b:02x}")).collect())
}

/// 读取二进制自报版本（`--version` 输出中第一个以数字开头的 token，
/// 如 `aproxy 0.1.0-alpha.9` → `0.1.0-alpha.9`）。不能运行或无版本输出
/// → Err——它不配作为安装源。
pub fn probe_version(bin: &Path) -> Result<String, String> {
    let out = std::process::Command::new(bin)
        .arg("--version")
        .output()
        .map_err(|e| format!("{} 无法执行: {e}", bin.display()))?;
    if !out.status.success() {
        return Err(format!("{} --version 退出非零", bin.display()));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    stdout
        .split_whitespace()
        .find(|t| t.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .map(String::from)
        .ok_or_else(|| format!("{} --version 输出无版本号: {stdout:?}", bin.display()))
}

/// 备料结果：staging 里的新二进制 + 完整性摘要（进 install.state）。
#[derive(Debug, Clone, PartialEq)]
pub struct StagedBinary {
    pub path: PathBuf,
    pub sha256: String,
}

/// `--from`/`--adopt` 流水线的收敛点：复制源二进制到 staging → unix 补可
/// 执行位 → sha256 → `--version` 试跑校验（信任锚 = 源自证：能运行且自报
/// 版本与目标一致）。四步全过才算 downloaded——绝不带病进入交换阶段。
pub fn stage_from_in(
    home: &Path,
    from: &Path,
    target_version: &str,
) -> Result<StagedBinary, String> {
    if !from.is_file() {
        return Err(format!("源二进制不存在: {}", from.display()));
    }
    let dir = staging_dir_in(home, target_version);
    std::fs::create_dir_all(&dir).map_err(|e| format!("staging 目录创建失败: {e}"))?;
    let staged = dir.join(binary_name());
    std::fs::copy(from, &staged).map_err(|e| format!("复制到 staging 失败: {e}"))?;
    ensure_executable(&staged);
    let sum = sha256_hex(&staged)?;
    // 校验对象是 staged 副本本身（不是源）——校验的就是要装的东西
    let probed = probe_version(&staged)?;
    if probed != target_version {
        let _ = std::fs::remove_file(&staged);
        return Err(format!(
            "版本不匹配：{} 自报 {probed}，目标 {target_version}",
            from.display()
        ));
    }
    Ok(StagedBinary {
        path: staged,
        sha256: sum,
    })
}

/// unix 侧复制后必须补可执行位（GitHub tarball/staging 复制均不保留 755，
/// 否则 `--version` 试跑直接失败）；Windows 无此概念，no-op。
fn ensure_executable(bin: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(bin, std::fs::Permissions::from_mode(0o755));
    }
    #[cfg(windows)]
    let _ = bin;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_known_vector() {
        // 标准测试向量：sha256("abc")
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("abc");
        std::fs::write(&f, b"abc").unwrap();
        assert_eq!(
            sha256_hex(&f).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert!(sha256_hex(&dir.path().join("no-such")).is_err());
    }

    #[test]
    fn staging_paths_are_home_derived() {
        let home = Path::new("/tmp/fake-home");
        assert_eq!(
            staging_dir_in(home, "0.1.0-alpha.9"),
            home.join("staging").join("0.1.0-alpha.9")
        );
        // 目标平台二进制名（两平台各自断言期望值）
        if cfg!(windows) {
            assert_eq!(binary_name(), "aproxy.exe");
        } else {
            assert_eq!(binary_name(), "aproxy");
        }
    }

    #[test]
    fn stage_from_rejects_missing_source() {
        let dir = tempfile::tempdir().unwrap();
        let err = stage_from_in(dir.path(), &dir.path().join("no-such.exe"), "0.1.0").unwrap_err();
        assert!(err.contains("不存在"), "{err}");
    }
}
