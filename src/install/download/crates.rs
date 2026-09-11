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

use super::{Artifact, DownloadCtx, Fetched};
use std::path::Path;

/// crates.io .crate 下载 URL。
fn crate_url(version: &str) -> String {
    format!("https://crates.io/api/v1/crates/aproxy/{version}/download")
}

/// 下载并解包 .crate 到 dest_dir（内含 `aproxy-{version}/` 目录）。
/// 返回解包出的 crate 源码目录。
async fn fetch_and_unpack_crate(ctx: &DownloadCtx, dest_dir: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dest_dir).map_err(|e| format!("crate 目录创建失败: {e}"))?;
    let tgz = dest_dir.join("crate.tgz");
    let result = async {
        super::download_to(&ctx.client, &crate_url(&ctx.version), &tgz).await?;
        // crates.io checksum：下载 URL 本身即信任锚（官方 CDN）；
        // .crate 的 sha512 随 API 元数据分发——此处直接以 CDN 内容为准
        //（与 cargo 工具链同一信任级别）
        let f = std::fs::File::open(&tgz).map_err(|e| format!("crate 打开失败: {e}"))?;
        let gz = flate2::read::GzDecoder::new(f);
        let mut archive = tar::Archive::new(gz);
        archive
            .unpack(dest_dir)
            .map_err(|e| format!("crate 解包失败: {e}"))?;
        let src = dest_dir.join(format!("aproxy-{}", ctx.version));
        if !src.is_dir() {
            return Err("crate 解包后未找到源码目录".to_string());
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
pub async fn fetch(ctx: &DownloadCtx, artifact: Artifact, dest: &Path) -> Result<Fetched, String> {
    if artifact != Artifact::Binary {
        return Err("cargo-binstall 通道不支持 skill（该级跳过）".to_string());
    }
    let work = dest.parent().unwrap().join("binstall-work");
    let _ = std::fs::remove_dir_all(&work);
    let src = fetch_and_unpack_crate(ctx, &work).await?;
    let manifest = std::fs::read_to_string(src.join("Cargo.toml.toml"))
        .or_else(|_| std::fs::read_to_string(src.join("Cargo.toml")))
        .map_err(|e| format!("crate manifest 读取失败: {e}"))?;
    // 模板提取（手写解析避免引 toml 依赖的二次成本——只找 [package.metadata.binstall] 的 pkg-url/pkg-fmt 两键）
    let (pkg_url, _pkg_fmt) = parse_binstall_template(&manifest);
    let binary_ext = if cfg!(windows) { ".exe" } else { "" };
    let url = match pkg_url {
        Some(t) => t
            .replace("{name}", "aproxy")
            .replace("{repo}", "MoYeRanqianzhi/aProxy")
            .replace("{version}", &ctx.version)
            .replace("{target}", ctx.target)
            .replace("{binary-ext}", binary_ext)
            .replace("{format}", "bin"),
        // 无显式模板：本仓库发布资产命名（不带版本号，bin 格式）
        None => format!(
            "https://github.com/MoYeRanqianzhi/aProxy/releases/download/v{}/aproxy-{}{}",
            ctx.version, ctx.target, binary_ext
        ),
    };
    super::download_to(&ctx.client, &url, dest).await?;
    let sum = crate::install::staging::sha256_hex(dest)?;
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
/// cargo 工具链自身行为）；产物 = 本地编译自证。
pub async fn fetch_build(
    ctx: &DownloadCtx,
    artifact: Artifact,
    dest: &Path,
) -> Result<Fetched, String> {
    match artifact {
        Artifact::Binary => {}
        Artifact::Skills => {
            // skill 随 crate 分发：从 .crate 提取 .claude/skills/（include 白名单
            // 打包进 .crate）——打 zip 供统一落位（skills::install_skill_dir
            // 吃 zip）
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
    let build = tokio::task::spawn_blocking(move || {
        let status = std::process::Command::new("cargo")
            .args(["build", "--release", "--locked"])
            .arg("--manifest-path")
            .arg(src_buf.join("Cargo.toml"))
            .env("CARGO_TARGET_DIR", &target_buf)
            .status()
            .map_err(|e| format!("cargo 不可用（该级需 Rust 工具链）: {e}"))?;
        if !status.success() {
            return Err("cargo build 失败（编译输出见 stderr）".to_string());
        }
        Ok(())
    })
    .await
    .map_err(|e| format!("编译任务失败: {e}"))?;
    build?;
    if !out_bin.is_file() {
        return Err(format!("编译产物不存在: {}", out_bin.display()));
    }
    std::fs::copy(&out_bin, dest).map_err(|e| format!("产物复制失败: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dest, std::fs::Permissions::from_mode(0o755));
    }
    let sum = crate::install::staging::sha256_hex(dest)?;
    Ok(Fetched {
        path: dest.to_path_buf(),
        sha256: sum,
        source: "cargo",
    })
}

/// 从 .crate 提取 skill 文件打包成 zip（统一走 skills 落位）。
async fn fetch_build_skills(ctx: &DownloadCtx, dest: &Path) -> Result<Fetched, String> {
    let work = dest.parent().unwrap().join("cargo-skill");
    let _ = std::fs::remove_dir_all(&work);
    let src = fetch_and_unpack_crate(ctx, &work).await?;
    let skill_src = src.join(".claude").join("skills").join("aproxy-cli");
    if !skill_src.is_dir() {
        return Err("crate 内未找到 skill 文件（include 白名单未包含？）".to_string());
    }
    // 目录 → zip
    let f = std::fs::File::create(dest).map_err(|e| format!("zip 创建失败: {e}"))?;
    let mut w = zip::ZipWriter::new(f);
    add_skill_tree(&mut w, &skill_src, &skill_src)?;
    w.finish().map_err(|e| format!("zip 收尾失败: {e}"))?;
    let sum = crate::install::staging::sha256_hex(dest)?;
    Ok(Fetched {
        path: dest.to_path_buf(),
        sha256: sum,
        source: "cargo",
    })
}

fn binary_name() -> &'static str {
    if cfg!(windows) {
        "aproxy.exe"
    } else {
        "aproxy"
    }
}

/// 递归把 skill 目录树写入 zip（条目名带 `aproxy-cli/` 前缀——与
/// skills::install_skill_dir 的解包布局一致）。
fn add_skill_tree(
    w: &mut zip::ZipWriter<std::fs::File>,
    base: &Path,
    dir: &Path,
) -> Result<(), String> {
    let rd = std::fs::read_dir(dir).map_err(|e| format!("skill 目录读取失败: {e}"))?;
    for entry in rd.flatten() {
        let p = entry.path();
        let rel = p
            .strip_prefix(base)
            .map_err(|e| format!("skill 路径处理失败: {e}"))?;
        if p.is_dir() {
            add_skill_tree(w, base, &p)?;
        } else {
            w.start_file(
                format!("aproxy-cli/{}", rel.display()),
                zip::write::SimpleFileOptions::default(),
            )
            .map_err(|e| format!("zip 写入失败: {e}"))?;
            let data = std::fs::read(&p).map_err(|e| format!("skill 文件读取失败: {e}"))?;
            std::io::Write::write_all(w, &data).map_err(|e| format!("zip 写入失败: {e}"))?;
        }
    }
    Ok(())
}
