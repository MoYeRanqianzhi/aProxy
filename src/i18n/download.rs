//! 下载渠道的故障按参数保留；内外层使用同一语言，写入机器文档时固定为英文。
use crate::install::{InstallFailure, reason::Cause};
use aproxy_ipc::{i18n::Lang, message};
use serde::{Deserialize, Serialize};

// 渠道标识、版本、路径和外部诊断均以文字存储。渲染只借用；skill 故障落盘前逐参数去除 URL。
macro_rules! phrase {
    ($l:ident,(),$en:literal,$zh:literal)=>{{message!(sentence(),en:$en,zh:$zh);sentence($l).to_owned()}};
    ($l:ident,($($v:ident:$t:ident),+),$en:literal,$zh:literal)=>{{message!(sentence($($v:&str),+),en:$en,zh:$zh);sentence($l,$($v),+)}};
}
macro_rules! reasons {
    ($($id:ident $({ $($v:ident:$t:ident),+ })?,en:$en:literal,zh:$zh:literal;)+)=>{
        /// 普通下载原语的故障；展示时才套上渠道和公共失败码。
        #[derive(Debug,Clone,Serialize,Deserialize)]
        #[serde(tag="id",content="args",rename_all="snake_case")]
        pub enum Issue { $(#[doc=$zh] $id $({ $($v:$t),+ })?,)+
            /// 尚无已知分类的系统证据，不从措辞推断类别。
            External(String),
            /// crate 里的文档校验包含 ZIP、tar 或清单的具体分类。
            Skill(Box<crate::install::skills::issue::Issue>),
        }
        impl Issue {
            /// 告知对应读者发生了什么；没有进程全局语言依赖。
            pub fn render(&self,lang:Lang)->String {match self {$(Self::$id $({ $($v),+ })?=>phrase!(lang,($($($v:$t),+)?),$en,$zh),)+ Self::External(s)=>s.clone(),Self::Skill(e)=>crate_skill(lang,&e.render(lang))}}
            /// 文档来源的 URL 可能带有签名或用户信息，在保留私有参数前也要去除。
            pub(crate) fn scrub(&mut self, clean:&impl Fn(&mut String)) {match self {$(Self::$id $({ $($v),+ })?=>{ $($(clean($v);)+)? },)+ Self::External(s)=>clean(s),Self::Skill(e)=>{ let mut s=e.render(Lang::En);let before=s.clone();clean(&mut s);if before!=s {**e=crate::install::skills::issue::Issue::External(s)} }}}
        }
    };
}
message!(crate_skill(e:&str),en:"Could not read crate documentation: {e}",zh:"crate 文档读取失败：{e}");
reasons! {
Semver { target: String, e: String }, en: "Invalid version {target}: semver parsing failed: {e}", zh: "版本号 {target} 非法（semver 解析失败: {e}）";
Downgrade { target: String, current: String }, en: "Target version {target} is older than the current version {current}. Use --allow-downgrade to downgrade.", zh: "目标版本 {target} 低于当前 {current}。确认要降级请加 --allow-downgrade。";
Cpu { other: String }, en: "Unknown CPU level {other}; expected x86-64-v3 or baseline.", zh: "未知指令集档 {other}（取值 x86-64-v3 | baseline）";
Proxy, en: "Invalid download proxy URL (credentials withheld).", zh: "下载代理 URL 非法（凭据已隐去）";
Client { e: String }, en: "Could not build the HTTP client: {e}", zh: "HTTP 客户端构建失败: {e}";
Request { e: String }, en: "Request failed: {e}", zh: "请求失败: {e}";
Http { status: String }, en: "HTTP {status}", zh: "HTTP {status}";
Create { path: String, e: String }, en: "Could not create {path}: {e}", zh: "{path} 创建失败: {e}";
Limit, en: "Document download exceeds the size limit.", zh: "文档下载大小超限";
Interrupted { e: String }, en: "Download interrupted: {e}", zh: "下载中断: {e}";
Write { e: String }, en: "Write failed: {e}", zh: "写入失败: {e}";
Flush { e: String }, en: "Flush failed: {e}", zh: "flush 失败: {e}";
Directory { e: String }, en: "Could not create the download directory: {e}", zh: "下载目录创建失败: {e}";
Publish { e: String }, en: "Could not put the downloaded file in place: {e}", zh: "落位失败: {e}";
Checksum { expected: String, actual: String }, en: "SHA-256 mismatch (expected {expected}, got {actual})", zh: "sha256 不匹配（期望 {expected}，实际 {actual}）";
GhApi { e: String }, en: "GitHub API request failed: {e}", zh: "GitHub API 请求失败: {e}";
GhParse { e: String }, en: "Could not parse the GitHub API response: {e}", zh: "GitHub API 解析失败: {e}";
ShaFetch { e: String }, en: "Could not download .sha256: {e}", zh: ".sha256 下载失败: {e}";
ShaStatus { e: String }, en: ".sha256 asset is unavailable: {e}", zh: ".sha256 资产不可用: {e}";
GhStatus { status: String }, en: "GitHub API HTTP {status}", zh: "GitHub API HTTP {status}";
GhAsset { version: String, asset: String }, en: "Release v{version} has no asset named {asset}", zh: "release v{version} 无资产 {asset}";
GhList, en: "Expected an array in the GitHub Releases response.", zh: "GitHub Releases 列表格式异常（非数组）";
RegistryRequest { registry: String, e: String }, en: "Registry request failed ({registry}): {e}", zh: "registry 请求失败（{registry}）: {e}";
RegistryVersion { registry: String, e: String }, en: "Package or version unavailable from registry ({registry}): {e}", zh: "registry 无此包/版本（{registry}）: {e}";
RegistryMain { registry: String, e: String }, en: "Main package unavailable from registry ({registry}): {e}", zh: "registry 无主包（{registry}）: {e}";
RegistryJson { e: String }, en: "Could not parse registry metadata: {e}", zh: "registry 元数据解析失败: {e}";
RegistryMissing { package: String, version: String, registry: String }, en: "Package {package} has no version {version} in registry {registry}.", zh: "registry 上 {package} 无版本 {version}（{registry}）";
Tarball, en: "Registry metadata has no dist.tarball.", zh: "元数据缺 dist.tarball";
Tags, en: "Registry metadata has no dist-tags.", zh: "元数据缺 dist-tags";
Base64, en: "Invalid base64 character in integrity.", zh: "integrity 含非法 base64 字符";
TgzOpen { e: String }, en: "Could not open tgz: {e}", zh: "tgz 打开失败: {e}";
TarRead { e: String }, en: "Could not unpack tar: {e}", zh: "tar 解包失败: {e}";
TarEntry { e: String }, en: "Could not read tar entry: {e}", zh: "tar 条目读取失败: {e}";
TarPath { e: String }, en: "Could not read tar path: {e}", zh: "tar 路径读取失败: {e}";
Extract { e: String }, en: "Could not extract tar entry: {e}", zh: "tar 条目提取失败: {e}";
NotInTgz { inner_path: String }, en: "No entry {inner_path} in tgz", zh: "tgz 内未找到 {inner_path}";
Integrity { integrity: String }, en: "Unsupported integrity format: {integrity}", zh: "不支持的 integrity 格式: {integrity}";
TgzRead { e: String }, en: "Could not read tgz: {e}", zh: "tgz 读取失败: {e}";
Sha512, en: "Integrity SHA-512 mismatch (npm registry verification failed).", zh: "integrity sha512 不匹配（npm registry 校验失败）";
CrateDir { e: String }, en: "Could not create crate directory: {e}", zh: "crate 目录创建失败: {e}";
CrateOpen { e: String }, en: "Could not open crate: {e}", zh: "crate 打开失败: {e}";
CrateUnpack { e: String }, en: "Could not unpack crate: {e}", zh: "crate 解包失败: {e}";
CrateMissing, en: "The unpacked crate has no source directory.", zh: "crate 解包后未找到源码目录";
BinstallSkip, en: "The cargo-binstall source does not support skills; skipping this source.", zh: "cargo-binstall 通道不支持 skill（该级跳过）";
CrateManifest { e: String }, en: "Could not read crate manifest: {e}", zh: "crate manifest 读取失败: {e}";
CargoMissing { e: String }, en: "Cargo is unavailable (this source requires a Rust toolchain): {e}", zh: "cargo 不可用（该级需 Rust 工具链）: {e}";
CargoFailed, en: "cargo build failed; see the build output on stderr.", zh: "cargo build 失败（编译输出见 stderr）";
Build { e: String }, en: "Build task failed: {e}", zh: "编译任务失败: {e}";
MissingBuild { path: String }, en: "Build artifact not found: {path}", zh: "编译产物不存在: {path}";
Copy { e: String }, en: "Could not copy artifact: {e}", zh: "产物复制失败: {e}";
CrateSkillOpen { e: String }, en: "Could not open the skill crate: {e}", zh: "crate 打开失败：{e}";
Zip { e: String }, en: "Could not create zip: {e}", zh: "zip 创建失败：{e}";
}
impl From<String> for Issue {
    fn from(s: String) -> Self {
        Self::External(s)
    }
}
impl From<Issue> for Cause {
    fn from(e: Issue) -> Self {
        Self::Download(e)
    }
}
impl From<Issue> for InstallFailure {
    fn from(e: Issue) -> Self {
        Self::because("install_failed", e.into())
    }
}
impl std::fmt::Display for Issue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.render(Lang::En))
    }
}
