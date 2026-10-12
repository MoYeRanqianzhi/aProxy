//! 发布清单 `aproxy-manifest.json`（永久兼容契约 C6，结构见 src/install/force.rs 的 `Manifest`）
//! 的生成与核对。release.yml 在全部产物构建完之后调用它：
//!
//! ```text
//! cargo run --locked --no-default-features --example release-manifest -- <版本> <产物目录> <输出文件>
//! ```
//!
//! 产物目录里只有各构建 job 上传的二进制与它们的 `<文件>.sha256`。每个二进制的变体、目标三元组
//! 与指令集档不看文件名、也不另外传，而是从二进制里嵌着的 `--version` 首行（C5）读出来，用
//! aproxy 自己的 `VersionLine::parse` 解析：`VERSION_LINE` 是 concat! 拼成的一个字符串字面量，
//! 原样留在只读数据段里，strip 不动它。这样清单写的就是二进制自己声明的身份，在构建机上跑不了
//! 的交叉编译产物也读得到。文件名再按命名规则核对一遍：普通在线安装的 github、cargo-binstall、
//! url 模板渠道按文件名找资产，名字不对它们就 404。
//!
//! 写完之后用强制安装读清单的同一段代码（`Manifest::parse`）读回来逐条核对：每个元素都读得懂、
//! 字段齐全、文件存在且 sha256 相符、同一个目标的同一种变体与指令集档只有一条、每个出现的变体都
//! 有基线档、目录里的每个二进制都在清单里、清单与资产都在读者的大小上限之内。任何一条不满足就以
//! 非零退出，release 不会带着一份旧版本读不懂或指错文件的清单发出去。

use aproxy::install::download::Artifact;
use aproxy::install::force::{
    ASSET_LIMIT, CPU_BASELINE, CPU_X86_64_V3, MANIFEST_ASSET, MANIFEST_FORMAT, MANIFEST_LIMIT,
    Manifest, ManifestAsset, VARIANT_DESKTOP, VARIANT_HEADLESS, VersionLine,
};
use sha2::{Digest, Sha256};
use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [version, dir, out] = args.as_slice() else {
        eprintln!("用法：release-manifest <版本> <产物目录> <输出文件>");
        std::process::exit(2);
    };
    let (dir, out) = (Path::new(dir), Path::new(out));
    let result = generate(version, dir, out).and_then(|()| verify(version, dir, out));
    if let Err(e) = result {
        eprintln!("::error::{MANIFEST_ASSET}：{e}");
        std::process::exit(1);
    }
}

/// 产物目录里的二进制：除了 `.sha256` 以外的每个文件，按文件名排序。
fn binaries(dir: &Path) -> Result<Vec<String>, String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("读不了 {}: {e}", dir.display()))?;
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| format!("读不了 {}: {e}", dir.display()))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|name| format!("文件名不是 UTF-8：{name:?}"))?;
        if !entry.path().is_file() {
            return Err(format!("产物目录里不该有子目录：{name}"));
        }
        if !name.ends_with(".sha256") {
            names.push(name);
        }
    }
    names.sort();
    if names.is_empty() {
        return Err(format!("{} 里一个二进制都没有", dir.display()));
    }
    Ok(names)
}

/// 二进制里嵌着的版本行：`aproxy v<版本> (` 开头、到第一个 `)` 为止。版本行的括号组里只有变体、
/// 目标三元组与指令集档，所以限定长度与字符；出现多次而内容相同（链接器可能留下副本）算一个。
fn embedded_version_line(bytes: &[u8], version: &str) -> Result<String, String> {
    let pattern = format!(
        r"aproxy v{} \([0-9A-Za-z_.;\- ]{{1,200}}\)",
        regex::escape(version)
    );
    let re = regex::bytes::Regex::new(&pattern).expect("版本行的正则由程序拼出，必定合法");
    let mut lines: Vec<String> = Vec::new();
    for found in re.find_iter(bytes) {
        let line = String::from_utf8_lossy(found.as_bytes()).into_owned();
        if !lines.contains(&line) {
            lines.push(line);
        }
    }
    match lines.as_slice() {
        [line] => Ok(line.clone()),
        [] => Err(format!(
            "没有找到 `aproxy v{version} (…)` 版本行（不是 v{version} 的 aproxy？）"
        )),
        many => Err(format!("找到 {} 个不同的版本行：{many:?}", many.len())),
    }
}

/// 发布资产的命名规则只有一处：普通在线安装按名字找资产用的 `Artifact::asset_name`。这里
/// 直接调用它，两边不会各写一份、悄悄走样（名字不对，普通安装就 404，而这里看不出来）。
fn asset_name(target: &str, variant: &str, cpu: &str) -> String {
    let cpu_suffix = if cpu == CPU_X86_64_V3 { "-v3" } else { "" };
    Artifact::Binary.asset_name(target, variant, cpu_suffix)
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// 读每个二进制、认出它的身份，写出清单（不带 BOM 的 UTF-8，单行）。
fn generate(version: &str, dir: &Path, out: &Path) -> Result<(), String> {
    let mut assets = Vec::new();
    for file in binaries(dir)? {
        let fail = |e: String| format!("{file}：{e}");
        let bytes = std::fs::read(dir.join(&file)).map_err(|e| fail(format!("读取失败: {e}")))?;
        let line = embedded_version_line(&bytes, version).map_err(fail)?;
        let parsed = VersionLine::parse(&line)
            .ok_or_else(|| fail(format!("版本行 `{line}` 不是 C5 的格式")))?;
        let variant = parsed
            .variant
            .ok_or_else(|| fail(format!("版本行 `{line}` 没有变体段")))?;
        if variant != VARIANT_DESKTOP && variant != VARIANT_HEADLESS {
            return Err(fail(format!("版本行 `{line}` 的变体 {variant} 不认识")));
        }
        let target = parsed
            .target
            .ok_or_else(|| fail(format!("版本行 `{line}` 没有目标段")))?;
        let cpu = if parsed.x86_64_v3 {
            CPU_X86_64_V3
        } else {
            CPU_BASELINE
        };
        let expected = asset_name(&target, &variant, cpu);
        if file != expected {
            return Err(fail(format!(
                "版本行是 `{line}`，按命名规则文件名应为 {expected}"
            )));
        }
        let sha256 = sha256_hex(&bytes);
        let sidecar = format!("{file}.sha256");
        let recorded = std::fs::read_to_string(dir.join(&sidecar))
            .map_err(|e| fail(format!("{sidecar} 读取失败: {e}")))?;
        let recorded = recorded.split_whitespace().next().unwrap_or_default();
        if !recorded.eq_ignore_ascii_case(&sha256) {
            return Err(fail(format!(
                "{sidecar} 写的是 {recorded}，文件实际是 {sha256}"
            )));
        }
        println!("{file}: {line}");
        assets.push(ManifestAsset {
            target,
            variant,
            cpu: cpu.to_string(),
            file,
            sha256,
        });
    }
    let manifest = Manifest {
        manifest: MANIFEST_FORMAT,
        version: version.to_string(),
        assets,
    };
    let text = serde_json::to_string(&manifest).map_err(|e| format!("序列化失败: {e}"))?;
    std::fs::write(out, text).map_err(|e| format!("{} 写入失败: {e}", out.display()))
}

/// 用强制安装读清单的代码读回刚写出的清单，逐条核对（见模块文档）。
fn verify(version: &str, dir: &Path, path: &Path) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{} 读取失败: {e}", path.display()))?;
    if bytes.len() as u64 > MANIFEST_LIMIT {
        return Err(format!(
            "{} 字节，超过读者 {MANIFEST_LIMIT} 字节的上限",
            bytes.len()
        ));
    }
    let text = String::from_utf8(bytes).map_err(|_| "不是 UTF-8".to_string())?;
    if text.starts_with('\u{feff}') {
        return Err("开头有 BOM（写者不得带 BOM）".to_string());
    }
    let manifest = Manifest::parse(&text)?;
    if manifest.manifest != MANIFEST_FORMAT {
        return Err(format!(
            "manifest 字段是 {}，应为 {MANIFEST_FORMAT}",
            manifest.manifest
        ));
    }
    if manifest.version != version {
        return Err(format!(
            "version 字段是 {}，应为 {version}",
            manifest.version
        ));
    }
    // 读者跳过读不懂的元素：读回的条数少于写进去的，就是有元素旧版本读不懂
    let raw: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("解析失败: {e}"))?;
    let written = raw["assets"].as_array().map_or(0, Vec::len);
    if manifest.assets.len() != written {
        return Err(format!(
            "写了 {written} 个资产，读者只读懂 {} 个",
            manifest.assets.len()
        ));
    }

    let files = binaries(dir)?;
    for asset in &manifest.assets {
        let fields = [
            &asset.target,
            &asset.variant,
            &asset.cpu,
            &asset.file,
            &asset.sha256,
        ];
        if fields.iter().any(|f| f.is_empty()) {
            return Err(format!("{} 有空字段", asset.file));
        }
        if asset.variant != VARIANT_DESKTOP && asset.variant != VARIANT_HEADLESS {
            return Err(format!("{} 的变体 {} 不认识", asset.file, asset.variant));
        }
        if asset.cpu != CPU_BASELINE && asset.cpu != CPU_X86_64_V3 {
            return Err(format!("{} 的指令集档 {} 不认识", asset.file, asset.cpu));
        }
        if !files.contains(&asset.file) {
            return Err(format!("{} 不在产物目录里", asset.file));
        }
        let bytes = std::fs::read(dir.join(&asset.file))
            .map_err(|e| format!("{} 读取失败: {e}", asset.file))?;
        if bytes.len() as u64 > ASSET_LIMIT {
            return Err(format!(
                "{} 有 {} 字节，超过读者 {ASSET_LIMIT} 字节的上限",
                asset.file,
                bytes.len()
            ));
        }
        let actual = sha256_hex(&bytes);
        if asset.sha256.len() != 64 || !asset.sha256.eq_ignore_ascii_case(&actual) {
            return Err(format!(
                "{} 的 sha256 写的是 {}，文件实际是 {actual}",
                asset.file, asset.sha256
            ));
        }
        // find 返回第一条匹配的：返回的不是它，就是同一组合出现了不止一次
        let found = manifest.find(&asset.target, &asset.variant, &asset.cpu);
        if found.map(|a| &a.file) != Some(&asset.file) {
            return Err(format!(
                "{} {} {} 在清单里不止一条",
                asset.target, asset.variant, asset.cpu
            ));
        }
        if manifest
            .select(&asset.target, &asset.variant, false)
            .is_none()
        {
            return Err(format!(
                "{} 的 {} 版本没有 {CPU_BASELINE} 档",
                asset.target, asset.variant
            ));
        }
    }
    for file in &files {
        if !manifest.assets.iter().any(|a| &a.file == file) {
            return Err(format!("{file} 在产物目录里，却不在清单里"));
        }
    }
    println!(
        "{MANIFEST_ASSET}：v{version}，{} 个资产，读回核对通过",
        manifest.assets.len()
    );
    Ok(())
}
