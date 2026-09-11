//! install 下载链条：按**有序链条**逐级尝试获取产物（二进制/skill），
//! 第一级成功即用。默认链 github → npm → cargo-binstall → cargo。
//!
//! - settings `download_chain` 配置后**完全按数组执行**（严格数组语义，
//!   与 config_dirs 的自动补默认相反），文档提醒链条写全。
//! - url 模板通道（可选）：占位符 `{version}/{asset}/{target}/{variant}`
//!   由通用填充器替换——具体 CDN 域名绝不硬编码进二进制（用户定调），
//!   jsDelivr 等常见 CDN 只在文档/skill 中作为可填示例提及。
//! - 校验矩阵（信任锚）：github=sha256 强校验（release 附 .sha256）；
//!   npm=registry integrity（sha512）强校验；url 模板=弱档（能拼出
//!   `.sha256` 则验，否则提示「非官方镜像、未校验」后接受）；
//!   cargo-binstall=模板 checksum（有则验）；cargo(build)=crates.io
//!   checksum + 本地编译自证。
//! - **下载代理与请求代理绝对分离**（用户定调）：`settings.download_proxy`
//!   与 CLI --download-proxy 仅管 install 下载，config.toml 的 `proxy`
//!   是上游请求转发。两者都未配置时 reqwest 自然回退环境变量。

pub mod crates;
pub mod github;
pub mod npmpkg;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// 产物类型：二进制与 skill 各自独立跑链（skill 支线并行，失败不影响安装）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Artifact {
    /// 平台二进制 `aproxy-<target>[-v3][.exe]`
    Binary,
    /// skill 总包 `aproxy-skills.zip`（第 8 步接线）
    Skills,
}

impl Artifact {
    /// GitHub Release 资产名（发布资产命名用户定调：文件名不带版本号，
    /// 版本由 release tag 承载）。
    pub fn asset_name(self, target: &str, variant: &str) -> String {
        match self {
            Artifact::Binary => {
                let exe = if cfg!(windows) { ".exe" } else { "" };
                format!("aproxy-{target}{variant}{exe}")
            }
            Artifact::Skills => "aproxy-skills.zip".to_string(),
        }
    }
}

/// 链条步骤（settings `download_chain` 数组元素 + 内置默认链）。
/// serde 手写（计划定调的用户友好写法）：渠道名为裸字符串
/// （"github"/"npm"/"cargo-binstall"/"cargo"），url 模板为对象
/// `{"url": "https://…/{version}/{asset}"}`——derive 的 externally-tagged
/// 形式会强迫 url 变体写成 `{"url": {"url": …}}`，不符直觉。
#[derive(Debug, Clone, PartialEq)]
pub enum ChainStep {
    /// GitHub Releases 直下 + .sha256 强校验（国外最优；国内常需下载代理）
    Github,
    /// npm registry（跟随 ~/.npmrc 的 registry——配了 npmmirror 自动走镜像；
    /// 不依赖用户装 node，registry 纯 HTTP API）
    Npm,
    /// cargo-binstall 约定（读 .crate 的 binstall 模板；多数模板仍指向
    /// GitHub——国内常退化为 github 链，失败无害链条继续；对 skill 不可用）
    Binstall,
    /// crates.io 拉 .crate 本地编译（跟随 ~/.cargo source replacement；
    /// 需 Rust 工具链，置链尾）
    Cargo,
    /// url 模板通道：{version}/{asset}/{target}/{variant} 占位符。
    /// 校验弱档：能拼出 <asset>.sha256 则验，否则提示未校验后接受。
    Url { url: String },
}

// serde：受控词表映射（未知名报错并列出全部取值——settings 校验提示）
impl<'de> Deserialize<'de> for ChainStep {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Name(String),
            Url { url: String },
        }
        match Raw::deserialize(d).map_err(serde::de::Error::custom)? {
            Raw::Name(s) => match s.as_str() {
                "github" => Ok(ChainStep::Github),
                "npm" => Ok(ChainStep::Npm),
                "cargo-binstall" => Ok(ChainStep::Binstall),
                "cargo" => Ok(ChainStep::Cargo),
                other => Err(serde::de::Error::custom(format!(
                    "未知下载渠道 \"{other}\"（取值：github | npm | cargo-binstall | cargo，或 {{\"url\": \"模板\"}}）"
                ))),
            },
            Raw::Url { url } => Ok(ChainStep::Url { url }),
        }
    }
}

impl Serialize for ChainStep {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            ChainStep::Github => s.serialize_str("github"),
            ChainStep::Npm => s.serialize_str("npm"),
            ChainStep::Binstall => s.serialize_str("cargo-binstall"),
            ChainStep::Cargo => s.serialize_str("cargo"),
            ChainStep::Url { url } => {
                use serde::ser::SerializeStruct;
                let mut st = s.serialize_struct("ChainStepUrl", 1)?;
                st.serialize_field("url", url)?;
                st.end()
            }
        }
    }
}

impl ChainStep {
    pub fn name(&self) -> &'static str {
        match self {
            ChainStep::Github => "github",
            ChainStep::Npm => "npm",
            ChainStep::Binstall => "cargo-binstall",
            ChainStep::Cargo => "cargo",
            ChainStep::Url { .. } => "url",
        }
    }
}

/// 内置默认链（download_chain 未配置时）。
pub fn default_chain() -> Vec<ChainStep> {
    vec![
        ChainStep::Github,
        ChainStep::Npm,
        ChainStep::Binstall,
        ChainStep::Cargo,
    ]
}

/// 生效链条：settings.download_chain（严格数组，不补默认）或内置默认链。
pub fn effective_chain(settings: &crate::settings::Settings) -> Vec<ChainStep> {
    settings
        .download_chain
        .clone()
        .unwrap_or_else(default_chain)
}

/// 下载上下文：target 三元组（构建时烙进二进制）、指令集变体、HTTP 客户端
/// （含下载代理装配）。
pub struct DownloadCtx {
    pub client: reqwest::Client,
    pub version: String,
    pub target: &'static str,
    pub variant: &'static str,
}

impl DownloadCtx {
    /// 组装：target 取构建三元组；变体按运行时指令集自动选择（AVX2 →
    /// -v3，失败回退 baseline；`--variant` 手动覆盖）；代理按
    /// download_proxy（显式）→ 环境变量（reqwest 默认）两级。
    pub fn build(
        version: &str,
        download_proxy: Option<&str>,
        variant_override: Option<&str>,
    ) -> Result<Self, String> {
        let variant = match variant_override {
            Some("v3") => "-v3",
            Some("baseline") => "",
            Some(other) => return Err(format!("未知变体 {other}（取值 v3 | baseline）")),
            None => {
                // AVX2 检测仅 x86 可用（aarch64 等 target 上该宏编译失败——
                // CI unix job 曾因此全红）；非 x86 目标恒 baseline
                #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
                let variant = if std::arch::is_x86_feature_detected!("avx2") {
                    "-v3"
                } else {
                    ""
                };
                #[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
                let variant = "";
                variant
            }
        };
        let mut builder = reqwest::Client::builder()
            .user_agent(format!("aproxy-install/{version}"))
            // 超时防网络挂死：连接 15s（不可达的渠道快速放弃进入下一级），
            // 整体 120s（大文件慢链宽容）——skill 支线 join 的预算以此兜底
            .connect_timeout(std::time::Duration::from_secs(15))
            .timeout(std::time::Duration::from_secs(120));
        if let Some(proxy) = download_proxy {
            // 代理 URL 可能内嵌凭据（user:pass@host）——错误信息打码（计划
            // 定调：与 config --show 的 base_url 同一口径）
            let p = reqwest::Proxy::all(proxy)
                .map_err(|_| "下载代理 URL 非法（凭据已隐去）".to_string())?;
            builder = builder.proxy(p);
        }
        let client = builder
            .build()
            .map_err(|e| format!("HTTP 客户端构建失败: {e}"))?;
        Ok(Self {
            client,
            version: version.to_string(),
            target: runtime_target(),
            variant,
        })
    }

    /// url 模板填充：{version}/{asset}/{target}/{variant} 占位符替换。
    pub fn fill_template(&self, template: &str, artifact: Artifact) -> String {
        let asset = artifact.asset_name(self.target, self.variant);
        template
            .replace("{version}", &self.version)
            .replace("{asset}", &asset)
            .replace("{target}", self.target)
            .replace("{variant}", self.variant)
    }
}

/// 一级通道的获取结果：产物已落盘 dest + 完整性信息。
#[derive(Debug)]
pub struct Fetched {
    pub path: PathBuf,
    /// sha256（强校验通道为产物实测值；url 模板弱档无校验文件时也是实测值）
    pub sha256: String,
    pub source: &'static str,
}

/// 下载 URL 到文件（流式落盘，不整载内存）。
pub async fn download_to(client: &reqwest::Client, url: &str, dest: &Path) -> Result<(), String> {
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("请求失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let mut file =
        std::fs::File::create(dest).map_err(|e| format!("{} 创建失败: {e}", dest.display()))?;
    let mut stream = resp;
    use std::io::Write;
    while let Some(chunk) = stream.chunk().await.map_err(|e| format!("下载中断: {e}"))? {
        file.write_all(&chunk)
            .map_err(|e| format!("写入失败: {e}"))?;
    }
    file.flush().map_err(|e| format!("flush 失败: {e}"))?;
    Ok(())
}

/// 文件的 sha256 十六进制（与 staging::sha256_hex 同算法，此为字节版）。
pub fn sha256_bytes(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// 运行时检测本平台的 target 三元组（与构建矩阵 11 变体命名对齐）。
/// musl 判定取编译期 cfg：分发二进制的 target 在构建时固定，产物自证。
pub fn runtime_target() -> &'static str {
    #[cfg(all(windows, target_env = "msvc"))]
    {
        #[cfg(target_arch = "x86_64")]
        return "x86_64-pc-windows-msvc";
        #[cfg(target_arch = "x86")]
        return "i686-pc-windows-msvc";
        #[cfg(target_arch = "aarch64")]
        return "aarch64-pc-windows-msvc";
    }
    #[cfg(all(target_os = "linux", target_env = "musl"))]
    {
        #[cfg(target_arch = "x86_64")]
        return "x86_64-unknown-linux-musl";
        #[cfg(target_arch = "aarch64")]
        return "aarch64-unknown-linux-musl";
    }
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        #[cfg(target_arch = "x86_64")]
        return "x86_64-unknown-linux-gnu";
        #[cfg(target_arch = "aarch64")]
        return "aarch64-unknown-linux-gnu";
    }
    #[cfg(target_os = "macos")]
    {
        #[cfg(target_arch = "x86_64")]
        return "x86_64-apple-darwin";
        #[cfg(target_arch = "aarch64")]
        return "aarch64-apple-darwin";
    }
    #[cfg(not(any(
        all(windows, target_env = "msvc"),
        all(target_os = "linux", any(target_env = "musl", target_env = "gnu")),
        target_os = "macos"
    )))]
    {
        // 兜底：未识别平台（如 linux-gnu 之外的 unix 变体）——链条仍可尝试
        "unknown-target"
    }
}

/// 按链条顺序获取产物。每级**通道内重试 3 次**（计划定调），全失败 →
/// Err 汇总各级错误（status=failed 语义由调用方处理——skill 失败不影响
/// 安装成功）。
pub async fn fetch_artifact(
    ctx: &DownloadCtx,
    chain: &[ChainStep],
    artifact: Artifact,
    dest_dir: &Path,
) -> Result<Fetched, String> {
    std::fs::create_dir_all(dest_dir).map_err(|e| format!("下载目录创建失败: {e}"))?;
    let asset = artifact.asset_name(ctx.target, ctx.variant);
    let mut errors = Vec::new();
    for step in chain {
        for attempt in 0..3 {
            let dest = dest_dir.join(format!(
                "{asset}.{}",
                if attempt == 0 {
                    "dl".to_string()
                } else {
                    format!("dl{attempt}")
                }
            ));
            let r = match step {
                ChainStep::Github => github::fetch(ctx, artifact, &dest).await,
                ChainStep::Npm => npmpkg::fetch(ctx, artifact, &dest).await,
                ChainStep::Binstall => crates::fetch(ctx, artifact, &dest).await,
                ChainStep::Cargo => crates::fetch_build(ctx, artifact, &dest).await,
                ChainStep::Url { url } => fetch_url_template(ctx, url, artifact, &dest).await,
            };
            match r {
                Ok(fetched) => {
                    // 原子改名：dl → 最终名（半截文件绝不冒充成品）
                    let final_dest = dest_dir.join(&asset);
                    std::fs::rename(&dest, &final_dest).map_err(|e| format!("落位失败: {e}"))?;
                    return Ok(Fetched {
                        path: final_dest,
                        ..fetched
                    });
                }
                Err(e) => {
                    let _ = std::fs::remove_file(&dest);
                    if attempt == 2 {
                        errors.push(format!("[{}] {e}", step.name()));
                        break;
                    }
                }
            }
        }
    }
    Err(format!(
        "下载链条全失败（asset {asset}）:\n  {}",
        errors.join("\n  ")
    ))
}

/// url 模板通道：填充模板下载产物；能拼出 `<产物url>.sha256` 则强校验，
/// 否则弱档接受（调用方由 sha256 是否为占位可知——此处直接在无校验文件时
/// 计算实际 sha256 并如实返回，信任锚 = 用户自己选择的源）。
async fn fetch_url_template(
    ctx: &DownloadCtx,
    template: &str,
    artifact: Artifact,
    dest: &Path,
) -> Result<Fetched, String> {
    let url = ctx.fill_template(template, artifact);
    download_to(&ctx.client, &url, dest).await?;
    // 弱档校验尝试：<url>.sha256 拼得出且下载成功 → 比对
    let asset = artifact.asset_name(ctx.target, ctx.variant);
    let sum = crate::install::staging::sha256_hex(dest)?;
    match ctx
        .client
        .get(format!("{url}.sha256"))
        .send()
        .await
        .ok()
        .filter(|r| r.status().is_success())
    {
        Some(resp) => {
            let expected = resp.text().await.unwrap_or_default();
            let expected = expected
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .to_lowercase();
            if expected != sum {
                let _ = std::fs::remove_file(dest);
                return Err(format!("sha256 不匹配（期望 {expected}，实际 {sum}）"));
            }
        }
        None => {
            eprintln!("[警告] 来源为非官方镜像（url 模板），产物未经独立校验（asset {asset}）");
        }
    }
    Ok(Fetched {
        path: dest.to_path_buf(),
        sha256: sum,
        source: "url",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chain_step_serde_roundtrip() {
        // 渠道名 kebab-case
        let s: ChainStep = serde_json::from_str("\"cargo-binstall\"").unwrap();
        assert_eq!(s, ChainStep::Binstall);
        let s: ChainStep = serde_json::from_str("\"github\"").unwrap();
        assert_eq!(s, ChainStep::Github);
        // url 模板对象
        let s: ChainStep =
            serde_json::from_str(r#"{"url": "https://mirror.example/{version}/{asset}"}"#).unwrap();
        assert_eq!(
            s,
            ChainStep::Url {
                url: "https://mirror.example/{version}/{asset}".into()
            }
        );
        // settings 往返
        let json = serde_json::to_string(&ChainStep::Npm).unwrap();
        assert_eq!(json, "\"npm\"");
    }

    #[test]
    fn effective_chain_strict_semantics() {
        // 未配置 = 默认链
        assert_eq!(
            effective_chain(&crate::settings::Settings::default()),
            default_chain()
        );
        // 配置后严格按数组——写少了就少跑，绝不追加默认项
        let s = crate::settings::Settings {
            download_chain: Some(vec![ChainStep::Github]),
            ..crate::settings::Settings::default()
        };
        assert_eq!(effective_chain(&s), vec![ChainStep::Github]);
    }

    #[test]
    fn asset_names_and_template_fill() {
        // .exe 后缀由编译期平台决定（asset_name 的契约），断言随之动态拼
        let exe = if cfg!(windows) { ".exe" } else { "" };
        let ctx = DownloadCtx {
            client: reqwest::Client::new(),
            version: "0.1.0-alpha.9".into(),
            target: "x86_64-pc-windows-msvc",
            variant: "-v3",
        };
        assert_eq!(
            Artifact::Binary.asset_name(ctx.target, ctx.variant),
            format!("aproxy-x86_64-pc-windows-msvc-v3{exe}")
        );
        assert_eq!(
            ctx.fill_template(
                "https://m/{version}/{asset}/{target}/{variant}",
                Artifact::Binary
            ),
            format!("https://m/0.1.0-alpha.9/aproxy-x86_64-pc-windows-msvc-v3{exe}/x86_64-pc-windows-msvc/-v3")
        );
    }

    #[tokio::test]
    async fn chain_all_fail_reports_every_level() {
        let ctx = DownloadCtx {
            client: reqwest::Client::new(),
            version: "x".into(),
            target: "x86_64-pc-windows-msvc",
            variant: "",
        };
        // github/npm 走真实网络（不可达即失败）+ url 模板指向本地不存在的
        // 端口——链条全失败时错误应汇总各级
        let dir = tempfile::tempdir().unwrap();
        let chain = vec![
            ChainStep::Url {
                url: "http://127.0.0.1:9/{version}/{asset}".into(),
            },
            ChainStep::Url {
                url: "http://127.0.0.1:9/v2/{asset}".into(),
            },
        ];
        let err = fetch_artifact(&ctx, &chain, Artifact::Binary, dir.path())
            .await
            .unwrap_err();
        assert!(err.contains("全失败") && err.contains("[url]"), "{err}");
    }
}
