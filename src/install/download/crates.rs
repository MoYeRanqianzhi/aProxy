//! crates.io 系通道：cargo-binstall（模板指向的预编译产物）与 cargo
//! （.crate 源码本地编译）。共享 .crate 获取与解包底座。
//!
//! - **binstall 通道**：下载 `.crate` → 读 `[package.metadata.binstall]`
//!   模板（pkg-url/pkg-fmt，缺省按本仓库资产命名兜底）→ 下载预编译二进制。
//!   诚实局限（计划定调）：模板大多仍指向 GitHub Releases——国内经常退化
//!   为 github 链（失败无害，链条继续）；对 skill 不可用（调用方跳过）。
//! - **cargo 通道**：`.crate` 解包 → `cargo build --release`（用户工具链，
//!   跟随 ~/.cargo 的 source replacement 镜像）→ 产物提取。skill 文件随
//!   crate 分发（include 白名单）→ 从解包目录提取。

use super::issue::Issue;

use super::{Artifact, DownloadCtx, Fetched};
use std::path::Path;

/// crates.io .crate 下载 URL。
fn crate_url(version: &str) -> String {
    format!("https://crates.io/api/v1/crates/aproxy/{version}/download")
}

/// 下载并解包 .crate 到 dest_dir（内含 `aproxy-{version}/` 目录）。
/// 返回解包出的 crate 源码目录。
async fn fetch_and_unpack_crate(ctx: &DownloadCtx, dest_dir: &Path) -> Result<PathBuf, Issue> {
    crate::fsx::create_dir_all(dest_dir).map_err(|e| Issue::CrateDir { e: e.to_string() })?;
    let tgz = dest_dir.join("crate.tgz");
    let result = async {
        super::download_to(
            &ctx.client,
            &crate_url(&ctx.version),
            &tgz,
            Artifact::Binary,
        )
        .await?;
        // crates.io checksum：下载 URL 本身即信任锚（官方 CDN）；
        // .crate 的 sha512 随 API 元数据分发——此处直接以 CDN 内容为准
        //（与 cargo 工具链同一信任级别）
        let f = std::fs::File::open(&tgz).map_err(|e| Issue::CrateOpen { e: e.to_string() })?;
        let gz = flate2::read::GzDecoder::new(f);
        let mut archive = tar::Archive::new(gz);
        archive
            .unpack(dest_dir)
            .map_err(|e| Issue::CrateUnpack { e: e.to_string() })?;
        let src = dest_dir.join(format!("aproxy-{}", ctx.version));
        if !src.is_dir() {
            return Err(Issue::CrateMissing);
        }
        Ok(src)
    }
    .await;
    let _ = std::fs::remove_file(&tgz);
    result
}

use std::path::PathBuf;

/// binstall 通道：.crate → 读 binstall 模板 → 下载预编译二进制。
/// 本仓库资产命名「不带版本号」（用户定调）——显式配置的模板优先；
/// 无模板时按本仓库命名兜底（`aproxy-<target><binary-ext>`，pkg-fmt=bin）。
/// 模板只描述带界面的默认产物：装无界面版本时一律按本仓库命名取
/// `aproxy-headless-<target><binary-ext>`。
pub async fn fetch(ctx: &DownloadCtx, artifact: Artifact, dest: &Path) -> Result<Fetched, Issue> {
    if artifact != Artifact::Binary {
        return Err(Issue::BinstallSkip);
    }
    let work = dest.parent().unwrap().join("binstall-work");
    let _ = std::fs::remove_dir_all(&work);
    let src = fetch_and_unpack_crate(ctx, &work).await?;
    let manifest = std::fs::read_to_string(src.join("Cargo.toml.toml"))
        .or_else(|_| std::fs::read_to_string(src.join("Cargo.toml")))
        .map_err(|e| Issue::CrateManifest { e: e.to_string() })?;
    // 模板提取（手写解析避免引 toml 依赖的二次成本——只找 [package.metadata.binstall] 的 pkg-url/pkg-fmt 两键）
    let (pkg_url, _pkg_fmt) = parse_binstall_template(&manifest);
    let binary_ext = if cfg!(windows) { ".exe" } else { "" };
    let url = match pkg_url {
        Some(t) if ctx.variant != crate::install::force::VARIANT_HEADLESS => t
            .replace("{name}", "aproxy")
            .replace("{repo}", "MoYeRanqianzhi/aProxy")
            .replace("{version}", &ctx.version)
            .replace("{target}", ctx.target)
            .replace("{binary-ext}", binary_ext)
            .replace("{format}", "bin"),
        // 无显式模板或无界面版本：本仓库发布资产命名（不带版本号，bin 格式，baseline 档）
        _ => format!(
            "https://github.com/MoYeRanqianzhi/aProxy/releases/download/v{}/{}",
            ctx.version,
            Artifact::Binary.asset_name(ctx.target, ctx.variant, "")
        ),
    };
    super::download_to(&ctx.client, &url, dest, Artifact::Binary).await?;
    let sum = crate::install::staging::hash_for_install(dest)?;
    let _ = std::fs::remove_dir_all(&work);
    Ok(Fetched {
        path: dest.to_path_buf(),
        sha256: sum,
        source: "cargo-binstall",
    })
}

/// 提取 [package.metadata.binstall] 的 pkg-url / pkg-fmt（保守行扫描）。
fn parse_binstall_template(manifest: &str) -> (Option<String>, Option<String>) {
    let mut section = false;
    let mut url = None;
    let mut fmt = None;
    for line in manifest.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            section = trimmed == "[package.metadata.binstall]";
            continue;
        }
        if section {
            if let Some(v) = trimmed.strip_prefix("pkg-url") {
                url = quoted(v);
            } else if let Some(v) = trimmed.strip_prefix("pkg-fmt") {
                fmt = quoted(v);
            }
        }
    }
    (url, fmt)
}

fn quoted(after_key: &str) -> Option<String> {
    let v = after_key.trim().strip_prefix('=')?.trim();
    let v = v.trim_matches('"');
    (!v.is_empty()).then(|| v.to_string())
}

/// cargo 通道：.crate 解包 → cargo build --release → 产物提取。
/// 跟随 ~/.cargo 的 source replacement（rsproxy 等镜像自动生效——
/// cargo 工具链自身行为）；产物 = 本地编译自证。无界面版本加
/// `--no-default-features`；带界面的版本在 Linux 上要 WebKitGTK 的开发包才编得过。
pub async fn fetch_build(
    ctx: &DownloadCtx,
    artifact: Artifact,
    dest: &Path,
) -> Result<Fetched, Issue> {
    match artifact {
        Artifact::Binary => {}
        Artifact::Skills => {
            // skill 随 crate 分发：从 .crate 提取 .claude/skills/（include 白名单
            // 打包进 .crate），只转换容器，再与其他来源一样按清单解包。
            return fetch_build_skills(ctx, dest).await;
        }
    }
    let work = dest.parent().unwrap().join("cargo-build");
    let _ = std::fs::remove_dir_all(&work);
    let src = fetch_and_unpack_crate(ctx, &work).await?;
    let target_dir = work.join("target-out");
    let out_bin = target_dir.join("release").join(binary_name());
    // 阻塞编译放 blocking 线程（tokio worker 不可久占）
    let src_buf = src.clone();
    let target_buf = target_dir.clone();
    let headless = ctx.variant == crate::install::force::VARIANT_HEADLESS;
    let build = tokio::task::spawn_blocking(move || {
        let mut command = std::process::Command::new("cargo");
        #[cfg(windows)]
        std::os::windows::process::CommandExt::creation_flags(
            &mut command,
            windows_sys::Win32::System::Threading::CREATE_NO_WINDOW,
        );
        let status = command
            .args(["build", "--release", "--locked"])
            .args(headless.then_some("--no-default-features"))
            .arg("--manifest-path")
            .arg(src_buf.join("Cargo.toml"))
            .env("CARGO_TARGET_DIR", &target_buf)
            .status()
            .map_err(|e| Issue::CargoMissing { e: e.to_string() })?;
        if !status.success() {
            return Err(Issue::CargoFailed);
        }
        Ok(())
    })
    .await
    .map_err(|e| Issue::Build { e: e.to_string() })?;
    build?;
    if !out_bin.is_file() {
        return Err(Issue::MissingBuild {
            path: out_bin.display().to_string(),
        });
    }
    std::fs::copy(&out_bin, dest).map_err(|e| Issue::Copy { e: e.to_string() })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dest, std::fs::Permissions::from_mode(0o755));
    }
    let sum = crate::install::staging::hash_for_install(dest)?;
    Ok(Fetched {
        path: dest.to_path_buf(),
        sha256: sum,
        source: "cargo",
    })
}

/// crate 保留原有文档路径，发布包把同一份清单放在它们旁边。只取普通文件，
/// 不把 tar 里的任意路径或链接展开到磁盘，也不为没有清单的旧版临时签发清单。
async fn fetch_build_skills(ctx: &DownloadCtx, dest: &Path) -> Result<Fetched, Issue> {
    let tgz = dest.with_extension("crate");
    super::download_to(
        &ctx.client,
        &crate_url(&ctx.version),
        &tgz,
        Artifact::Skills,
    )
    .await?;
    let input = std::fs::File::open(tgz).map_err(|e| Issue::CrateSkillOpen { e: e.to_string() })?;
    let out = std::fs::File::create(dest).map_err(|e| Issue::Zip { e: e.to_string() })?;
    translate_skills(input, out, &ctx.version)
        .map_err(|e| Issue::Skill(Box::new(crate::install::skills::issue::Issue::from_io(&e))))?;
    Ok(Fetched {
        path: dest.to_path_buf(),
        sha256: crate::install::staging::hash_for_install(dest)?,
        source: "cargo",
    })
}

/// 发布前检查和在线安装使用完全相同的容器转换和解压预算。
pub(crate) fn translate_skills(
    input: impl std::io::Read,
    out: impl std::io::Write + std::io::Seek,
    version: &str,
) -> std::io::Result<()> {
    crate_skills_zip(input, out, version, super::MAX_SKILL_TAR)
}

/// 只转换容器，不改清单和文档的字节。从 tar 取原始路径名，不用本机的路径分隔符重拼。
fn crate_skills_zip(
    input: impl std::io::Read,
    out: impl std::io::Write + std::io::Seek,
    version: &str,
    tar_limit: u64,
) -> std::io::Result<()> {
    use crate::install::skills::manifest::{MAX_FILE, MAX_FILES, MAX_MANIFEST, MAX_TOTAL};
    use std::io::Read;
    let mut tar = tar::Archive::new(super::TarBudget(
        flate2::read::GzDecoder::new(input).take(tar_limit),
    ));
    let prefix = format!("aproxy-{version}/.claude/skills/");
    let mut zip = zip::ZipWriter::new(out);
    let mut total = 0;
    let mut count = 0;
    for entry in tar.entries()? {
        let mut entry = entry?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry.path_bytes();
        let Some(key) = path.strip_prefix(prefix.as_bytes()) else {
            continue;
        };
        let key = std::str::from_utf8(key)
            .map_err(|_| std::io::Error::other(crate::install::skills::issue::Issue::TarName))?;
        let limit = if key == "manifest.json" {
            MAX_MANIFEST
        } else {
            MAX_FILE
        };
        count += 1;
        // 路径还会进入 ZIP 的两个目录头；约束名字开销，防止转换产物先于校验超限。
        if key.len() > 1024
            || entry.size() > limit
            || entry.size() > MAX_TOTAL + MAX_MANIFEST - total
            || count > MAX_FILES + 1
        {
            return Err(std::io::Error::other(
                crate::install::skills::issue::Issue::TarCount,
            ));
        }
        total += entry.size();
        zip.start_file(
            key,
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored),
        )?;
        std::io::copy(&mut entry, &mut zip)?;
    }
    zip.finish()?;
    Ok(())
}

fn binary_name() -> &'static str {
    if cfg!(windows) {
        "aproxy.exe"
    } else {
        "aproxy"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// crate 的同名旁支与普通源码都不进文档包；清单和正文必须保留原始字节。
    #[test]
    fn crate_channel_preserves_the_shipped_manifest_and_bytes() {
        use std::io::{Cursor, Read};
        let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::fast(),
        ));
        for (name, body) in [
            (".claude/skills/manifest.json", &b"original manifest"[..]),
            (".claude/skills/s/SKILL.md", &b"text\r\n"[..]),
            (".claude/skills-old/s/SKILL.md", b"skip"),
            ("src/lib.rs", b"skip"),
        ] {
            let mut h = tar::Header::new_gnu();
            h.set_mode(0o644);
            h.set_size(body.len() as u64);
            h.set_cksum();
            tar.append_data(&mut h, format!("aproxy-1.0.0/{name}"), body)
                .unwrap();
        }
        let data = tar.into_inner().unwrap().finish().unwrap();
        let mut zip = Cursor::new(Vec::new());
        // 正文都很小，限额只在读到后面的无关源码与 tar 填充时触发。
        let err = crate_skills_zip(data.as_slice(), Cursor::new(Vec::new()), "1.0.0", 3 * 1024)
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("uncompressed documentation tar exceeds the size limit")
        );
        crate_skills_zip(
            data.as_slice(),
            &mut zip,
            "1.0.0",
            super::super::MAX_SKILL_TAR,
        )
        .unwrap();
        let mut archive = zip::ZipArchive::new(zip).unwrap();
        assert_eq!(archive.len(), 2);
        for (key, want) in [
            ("manifest.json", "original manifest"),
            ("s/SKILL.md", "text\r\n"),
        ] {
            let mut s = String::new();
            archive
                .by_name(key)
                .unwrap()
                .read_to_string(&mut s)
                .unwrap();
            assert_eq!(s, want);
        }
    }

    /// 只认 [package.metadata.binstall] 本身的 pkg-url / pkg-fmt。按目标的 overrides（根
    /// Cargo.toml 给 musl 目标指向无界面资产的那两段）不在其中：它们给的是用户手里的
    /// cargo-binstall，本渠道的无界面地址由 fetch 自己拼
    #[test]
    fn binstall_template_ignores_target_overrides() {
        let overrides_only = r#"
[package]
name = "aproxy"

[package.metadata.binstall.overrides.x86_64-unknown-linux-musl]
pkg-url = "{ repo }/releases/download/v{ version }/{ name }-headless-{ target }{ binary-ext }"
pkg-fmt = "bin"
"#;
        assert_eq!(parse_binstall_template(overrides_only), (None, None));
        let main_section = r#"
[package.metadata.binstall]
pkg-url = "https://example.invalid/{version}/{target}"
pkg-fmt = "bin"

[package.metadata.binstall.overrides.aarch64-unknown-linux-musl]
pkg-url = "https://example.invalid/musl"
"#;
        assert_eq!(
            parse_binstall_template(main_section),
            (
                Some("https://example.invalid/{version}/{target}".to_string()),
                Some("bin".to_string())
            )
        );
    }
}
