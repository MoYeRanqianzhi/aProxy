//! staging 备料区：新二进制在落位 bin/ 之前的完整落盘与校验。
//!
//! 铁律 2（先下载后 rename）：任何文件交换之前，新二进制必须已完整落盘
//! staging 并通过校验。staging 与 bin 同在 APROXY_HOME 下——Windows rename
//! 原子性的同卷前提；备料文件保留至 done/abort 才清理（它是 swapping 中断
//! 的恢复源）。

use std::path::{Path, PathBuf};

/// staging 备料目录：`<home>/install/staging/<目标版本>/`。在线渠道的下载暂存
/// （`.dl` 半截文件、cargo 渠道的构建目录）也落在这里，见 download::fetch_artifact。
pub fn staging_dir(target_version: &str) -> PathBuf {
    staging_dir_in(&crate::settings::home(), target_version)
}

/// 同上，主目录可指定（测试注入用）。
pub fn staging_dir_in(home: &Path, target_version: &str) -> PathBuf {
    super::install_dir_in(home)
        .join("staging")
        .join(target_version)
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

/// 读取二进制自报版本：在本机执行它的 `--version`，按永久格式（C5，见
/// `crate::VERSION_LINE`）取版本号。不能运行、或输出不是这一格式 → Err——它
/// 不配作为安装源。
pub fn probe_version(bin: &Path) -> Result<String, String> {
    let out = std::process::Command::new(bin)
        .arg("--version")
        .output()
        .map_err(|e| format!("{} 无法执行: {e}", bin.display()))?;
    if !out.status.success() {
        return Err(format!("{} --version 退出非零", bin.display()));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    if let Some(version) = parse_version_line(&stdout) {
        return Ok(version);
    }
    // 0.1.x 输出 `aproxy <版本>`（没有 v）。`--from` 指向 0.1.x 的二进制时，按版本
    // 下限说明它为什么装不了、怎样回到 0.1.x，而不只是报格式不对
    let legacy = stdout
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("aproxy "))
        .and_then(|rest| rest.split(char::is_whitespace).next())
        .and_then(|version| semver::Version::parse(version).ok());
    if let Some(Err(e)) = legacy.map(|version| super::check_floor(&version)) {
        return Err(format!("{}：{e}", bin.display()));
    }
    Err(format!(
        "{} --version 的输出不是 `aproxy v<版本> (…)`（0.2.0 起每个版本都这样输出）: {stdout:?}",
        bin.display()
    ))
}

/// 从 `--version` 的输出里取版本号（永久兼容契约 C5）：首行 `aproxy v` 之后、
/// 到空白或行尾为止，且必须是合法的 semver。不是这一格式 → None。
pub fn parse_version_line(output: &str) -> Option<String> {
    let rest = output.lines().next()?.strip_prefix("aproxy v")?;
    let version = rest.split(char::is_whitespace).next()?;
    semver::Version::parse(version).ok()?;
    Some(version.to_string())
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
pub fn ensure_executable(bin: &Path) {
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
            staging_dir_in(home, "0.2.0-alpha.9"),
            home.join("install").join("staging").join("0.2.0-alpha.9")
        );
        // 目标平台二进制名（两平台各自断言期望值）
        if cfg!(windows) {
            assert_eq!(binary_name(), "aproxy.exe");
        } else {
            assert_eq!(binary_name(), "aproxy");
        }
    }

    #[test]
    fn version_line_parses_only_the_permanent_format() {
        assert_eq!(
            parse_version_line("aproxy v0.2.0 (desktop; x86_64-pc-windows-msvc; x86-64-v3)\n")
                .as_deref(),
            Some("0.2.0")
        );
        assert_eq!(
            parse_version_line("aproxy v0.2.0-alpha.1 (headless; aarch64-unknown-linux-musl)\r\n")
                .as_deref(),
            Some("0.2.0-alpha.1")
        );
        // 后面的括号段可有可无，版本到行尾为止也行
        assert_eq!(
            parse_version_line("aproxy v1.0.0").as_deref(),
            Some("1.0.0")
        );
        // 0.1.x 的格式（没有 v）、版本不是 semver、不在首行、v 后面先有空白：都不认
        for bad in [
            "aproxy 0.1.0\n",
            "aproxy vnext (desktop; x)\n",
            "\naproxy v0.2.0\n",
            "aproxy v 0.2.0\n",
            "",
        ] {
            assert_eq!(parse_version_line(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn stage_from_rejects_missing_source() {
        let dir = tempfile::tempdir().unwrap();
        let err = stage_from_in(dir.path(), &dir.path().join("no-such.exe"), "0.1.0").unwrap_err();
        assert!(err.contains("不存在"), "{err}");
    }
}
