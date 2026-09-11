//! npm 通道（默认链第 2 级）：registry 纯 HTTP API——**不依赖用户装 node**
//! （计划定调）。关键机制：跟随 `~/.npmrc` 的 registry 配置——配了
//! npmmirror 的用户自动走镜像，这是「npm 更大概率可用」的机制化落地。
//!
//! 数据流：拉包元数据（各版本 tarball URL 与 dist.integrity sha512）→
//! 下载 tgz → tar.gz 解包提取产物（二进制在平台包 `bin/`，skill 在主包）。

use super::{Artifact, DownloadCtx, Fetched, download_to};
use std::path::Path;

/// 平台包名（与 npm/aproxy/bin/aproxy.js 的 platformPackage 及组包脚本
/// MAPPINGS 同一规则）。
fn platform_package(ctx: &DownloadCtx) -> String {
    #[cfg(windows)]
    {
        let arch = match ctx.target {
            t if t.starts_with("x86_64") => "x64",
            t if t.starts_with("i686") => "ia32",
            _ => "arm64",
        };
        format!("@meowo/aproxy-windows-{arch}")
    }
    #[cfg(target_os = "macos")]
    {
        let arch = if ctx.target.starts_with("x86_64") {
            "x64"
        } else {
            "arm64"
        };
        format!("@meowo/aproxy-darwin-{arch}")
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let arch = if ctx.target.starts_with("x86_64") {
            "x64"
        } else {
            "arm64"
        };
        let libc = if ctx.target.contains("musl") {
            "-musl"
        } else {
            ""
        };
        format!("@meowo/aproxy-linux-{arch}{libc}")
    }
}

/// 读 ~/.npmrc 的 registry= 行（缺省官方 registry）。逐行扫 `registry=`
/// 前缀（npmrc 是 INI 风格键值，value 可能带引号——strip 引号）。
fn registry_from_npmrc() -> String {
    let default = "https://registry.npmjs.org".to_string();
    let Some(path) = dirs::home_dir().map(|h| h.join(".npmrc")) else {
        return default;
    };
    std::fs::read_to_string(path)
        .ok()
        .and_then(|content| {
            content.lines().find_map(|line| {
                let line = line.trim();
                let value = line.strip_prefix("registry=")?;
                let value = value.trim().trim_matches(['"', '\'']);
                (!value.is_empty()).then(|| value.trim_end_matches('/').to_string())
            })
        })
        .unwrap_or(default)
}

/// registry 元数据：指定版本的 tarball URL + integrity（base64 编码的
/// sha512）。
#[derive(Debug)]
struct TarballMeta {
    url: String,
    integrity_sha512: Option<String>,
}

/// 拉包元数据。`artifact` 决定查主包（skill）还是平台包（二进制）。
async fn fetch_meta(ctx: &DownloadCtx, artifact: Artifact) -> Result<TarballMeta, String> {
    let package = match artifact {
        Artifact::Binary => platform_package(ctx),
        Artifact::Skills => "@meowo/aproxy".to_string(),
    };
    let registry = registry_from_npmrc();
    let url = format!("{registry}/{package}");
    let resp = ctx
        .client
        .get(&url)
        .header("Accept", "application/vnd.npm.install-v1+json")
        .send()
        .await
        .map_err(|e| format!("registry 请求失败（{}）: {e}", registry))?
        .error_for_status()
        .map_err(|e| format!("registry 无此包/版本（{}）: {e}", registry))?;
    let meta: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("registry 元数据解析失败: {e}"))?;
    let version_meta = &meta["versions"][&ctx.version];
    if version_meta.is_null() {
        return Err(format!(
            "registry 上 {package} 无版本 {}（{}）",
            ctx.version, registry
        ));
    }
    let tarball = version_meta["dist"]["tarball"]
        .as_str()
        .ok_or("元数据缺 dist.tarball")?
        .to_string();
    let integrity = version_meta["dist"]["integrity"].as_str().map(String::from);
    Ok(TarballMeta {
        url: tarball,
        integrity_sha512: integrity,
    })
}

/// base64 解码（integrity 是 `sha512-<base64>` 格式）。
fn b64_decode(s: &str) -> Result<Vec<u8>, String> {
    // 手写 base64：sha2 已有，避免引入 base64 依赖；integrity 字符集
    // 受控（npm registry 生成），非法输入按错误处理
    let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let vals: Vec<u8> = s
        .bytes()
        .filter(|b| !b" \t\r\n=".contains(b))
        .map(|b| {
            alphabet
                .iter()
                .position(|a| *a == b)
                .map(|p| p as u8)
                .ok_or_else(|| "integrity 含非法 base64 字符".to_string())
        })
        .collect::<Result<_, _>>()?;
    let mut out = Vec::with_capacity(vals.len() * 3 / 4);
    for chunk in vals.chunks(4) {
        let mut acc: u32 = 0;
        for (i, v) in chunk.iter().enumerate() {
            acc |= (*v as u32) << (18 - i * 6);
        }
        out.push((acc >> 16) as u8);
        if chunk.len() > 2 {
            out.push((acc >> 8) as u8);
        }
        if chunk.len() > 3 {
            out.push(acc as u8);
        }
    }
    Ok(out)
}

/// tgz 解包：提取 tgz 内的指定文件（tar.gz 内 `package/<路径>`）到 dest。
fn extract_from_tgz(tgz: &Path, inner_path: &str, dest: &Path) -> Result<(), String> {
    let f = std::fs::File::open(tgz).map_err(|e| format!("tgz 打开失败: {e}"))?;
    let gz = flate2::read::GzDecoder::new(f);
    let mut archive = tar::Archive::new(gz);
    for entry in archive
        .entries()
        .map_err(|e| format!("tar 解包失败: {e}"))?
    {
        let mut entry = entry.map_err(|e| format!("tar 条目读取失败: {e}"))?;
        let name = entry
            .path()
            .map_err(|e| format!("tar 路径读取失败: {e}"))?
            .to_string_lossy()
            .to_string();
        // npm tarball 内部统一带 package/ 前缀（组包脚本同款）
        if name == format!("package/{inner_path}") || name == format!("package/{inner_path}/") {
            // **tar 路径穿越防护**：dest 是调用方指定的绝对路径，不拼接
            // entry 内路径——穿越无从谈起
            let mut out = std::fs::File::create(dest)
                .map_err(|e| format!("{} 创建失败: {e}", dest.display()))?;
            std::io::copy(&mut entry, &mut out).map_err(|e| format!("tar 条目提取失败: {e}"))?;
            return Ok(());
        }
    }
    Err(format!("tgz 内未找到 {inner_path}"))
}

/// npm 通道获取：元数据 → tgz 下载 + integrity 强校验 → 解包提取产物。
pub async fn fetch(ctx: &DownloadCtx, artifact: Artifact, dest: &Path) -> Result<Fetched, String> {
    let meta = fetch_meta(ctx, artifact).await?;
    let tgz_dest = dest.with_extension("tgz");
    let result = async {
        download_to(&ctx.client, &meta.url, &tgz_dest).await?;
        // integrity（sha512）强校验
        if let Some(integrity) = &meta.integrity_sha512 {
            let Some(encoded) = integrity.strip_prefix("sha512-") else {
                return Err(format!("不支持的 integrity 格式: {integrity}"));
            };
            let expected = b64_decode(encoded)?;
            let data = std::fs::read(&tgz_dest).map_err(|e| format!("tgz 读取失败: {e}"))?;
            use sha2::Digest;
            let mut h = sha2::Sha512::new();
            h.update(&data);
            let actual: Vec<u8> = h.finalize().iter().copied().collect();
            if expected != actual {
                return Err("integrity sha512 不匹配（npm registry 校验失败）".to_string());
            }
        }
        // 解包提取：二进制在平台包 `bin/aproxy[.exe]`；skill 在主包
        // `skills/aproxy-cli/`（tgz 提取是整目录树，此处取 zip 后整目录——
        // 第 8 步接线；二进制为单文件直取）
        match artifact {
            Artifact::Binary => {
                let inner = if cfg!(windows) {
                    "bin/aproxy.exe"
                } else {
                    "bin/aproxy"
                };
                extract_from_tgz(&tgz_dest, inner, dest)?;
            }
            Artifact::Skills => {
                // 主包内 skill 目录打的是 zip（组包脚本同款）；单文件场景
                // 先提取 SKILL.md 供校验，整目录提取随第 8 步
                let inner = "skills/aproxy-cli.zip";
                extract_from_tgz(&tgz_dest, inner, dest)?;
            }
        }
        let sum = crate::install::staging::sha256_hex(dest)?;
        Ok(Fetched {
            path: dest.to_path_buf(),
            sha256: sum,
            source: "npm",
        })
    }
    .await;
    let _ = std::fs::remove_file(&tgz_dest);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_package_matches_build_mappings() {
        // platform_package 的 OS 段由编译期平台决定（下载总是当前平台），
        // arch/libc 段由 ctx.target 解析——测试只断言 target 驱动的部分，
        // 跨平台编译均成立
        let mk = |target: &'static str| {
            platform_package(&DownloadCtx {
                client: reqwest::Client::new(),
                version: "x".into(),
                target,
                variant: "",
            })
        };
        // arch 段随 target 前缀变化，与 OS 段无关
        assert_eq!(mk("x86_64-foo"), mk("x86_64-bar"));
        assert_ne!(mk("x86_64-foo"), mk("aarch64-foo"));
        // musl 判定只来自 target 字符串
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            assert!(mk("x86_64-unknown-linux-musl").ends_with("-musl"));
            assert!(!mk("x86_64-unknown-linux-gnu").ends_with("-musl"));
        }
        // 完整形态：@meowo/aproxy-<os>-<arch>[libc]
        assert!(mk("x86_64-any").starts_with("@meowo/aproxy-"));
    }

    #[test]
    fn b64_decode_known_vector() {
        // base64("sha512 test") 手算向量
        assert_eq!(b64_decode("c2hhNTEyIHRlc3Q=").unwrap(), b"sha512 test");
        assert!(b64_decode("!!!").is_err());
    }

    #[test]
    fn npmrc_registry_following() {
        // 无 ~/.npmrc 或缺 registry 行 → 官方默认（函数依赖真实 home，
        // 单测只验格式合法性）
        let registry = registry_from_npmrc();
        assert!(registry.starts_with("https://") || registry.starts_with("http://"));
    }
}
