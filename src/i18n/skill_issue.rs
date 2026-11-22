//! skill 的本地故障在 io::Error 内仍携带类别，不解析系统或旧版本的报错文字。
use aproxy_ipc::{i18n::Lang, message};
use serde::{Deserialize, Serialize};
message!(damaged(), en: "Installed skills do not match the manifest. Run aproxy skill install to repair them.", zh: "已安装的 skill 与清单不符，请执行 aproxy skill install 修复");

message!(absent(), en: "Skills are not installed. Run aproxy skill install.", zh: "未安装 skill，请执行 aproxy skill install");

message!(legacy(), en: "Installed skills have no manifest. Run aproxy skill install to update them.", zh: "已安装的 skill 没有清单，请执行 aproxy skill install 更新");

message!(unknown_file(), en: "The skill or file name is not in the manifest.", zh: "skill 名或文件名不在清单中");

message!(unknown_name(), en: "The skill name is not in the manifest.", zh: "skill 名不在清单中");

message!(lock_links(), en: "The skill lock file has other hard links.", zh: "skill 锁文件有其他硬链接");

message!(busy(), en: "Another process is installing skills. Try again later.", zh: "另一个进程正在安装 skill，请稍后重试");

message!(workspace_key(), en: "The skill workspace credentials are invalid. Move skills/ aside and run aproxy skill install.", zh: "skill 工作区凭据无效；请移走 skills/ 后执行 aproxy skill install");

message!(workspace_contents(), en: "Unexpected files in the skill workspace.", zh: "skill 工作区内容异常");

message!(manifest(), en: "Invalid skill manifest.", zh: "skill 清单无效");

message!(missing(), en: "The skill package has no manifest.json. Versions up to and including 0.2.0-alpha.4 did not ship a manifest. Use a later release that includes one.", zh: "skill 包缺少 manifest.json；0.2.0-alpha.4 及更早的发布版不带清单，请使用带清单的后续版本");

message!(archive(), en: "Invalid skill archive.", zh: "skill 压缩包无效");

message!(entries(), en: "The skill archive has too many entries.", zh: "skill 压缩包条目过多");

message!(size(), en: "The skill archive exceeds the size limit.", zh: "skill 压缩包过大");

message!(version(), en: "The skill manifest version does not match the requested version.", zh: "skill 清单版本与请求版本不符");

message!(file_missing(), en: "The skill package is missing a file listed in the manifest.", zh: "skill 包缺少清单中的文件");

message!(file_size(), en: "The skill file size does not match the manifest.", zh: "skill 文件大小与清单不符");

message!(digest(), en: "Skill file verification failed.", zh: "skill 文件校验失败");

message!(entry(), en: "Unsupported skill archive entry type, size or compression method.", zh: "skill 包条目的类型、大小或压缩方式不合要求");

message!(marker(), en: "manifest.json is not a regular file. Move it aside and run aproxy skill install.", zh: "manifest.json 不是普通文件；请移走它后执行 aproxy skill install");

message!(backup(), en: "The skill backup directory is not in the manifest.", zh: "skill 备份目录不在清单中");

message!(unowned(name: &str), en: "Directory {name} was not part of the previous installation. Move it aside, or move skills/ aside and run aproxy skill install.", zh: "目录 {name} 不属于上次安装；请移走该目录，或移走 skills/ 后执行 aproxy skill install");

message!(recovery(path: &str), en: "Could not restore the skill directory. Its backup is at {path}.", zh: "skill 目录恢复失败，备份保存在 {path}");

message!(exists(path: &str), en: "The skill recovery target already exists: {path}", zh: "skill 恢复目标已存在：{path}");

message!(locking(e:&str), en: "Could not lock the skill installation: {e}", zh: "skill 加锁失败：{e}");

message!(stage(e:&str), en: "Could not create the skill download directory: {e}", zh: "skill 下载目录创建失败：{e}");

message!(install(e:&str), en: "Skill installation failed: {e}", zh: "skill 安装失败：{e}");
message!(tar_limit(), en:"The uncompressed documentation tar exceeds the size limit.", zh:"文档 tar 解压大小超限");
message!(tar_name(), en:"Invalid crate documentation path.", zh:"crate 文档路径无效");
message!(tar_count(), en:"The crate documentation exceeds the file count or size limit.", zh:"crate 文档数量或大小超限");
/// 读写、校验、恢复与发布 skill 时产生的诊断；持久化时只保存排版需要的事实。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "id", content = "args", rename_all = "snake_case")]
pub enum Issue {
    /// 文档 tar 解压大小超限。
    TarLimit,
    /// crate 文档路径无效。
    TarName,
    /// crate 文档数量或大小超限。
    TarCount,

    /// 系统或外部解析器的原始说明。
    External(String),
    /// 已安装的 skill 与清单不符，请执行 aproxy skill install 修复。
    Damaged,
    /// 未安装 skill，请执行 aproxy skill install。
    Absent,
    /// 已安装的 skill 没有清单，请执行 aproxy skill install 更新。
    Legacy,
    /// skill 名或文件名不在清单中。
    UnknownFile,
    /// skill 名不在清单中。
    UnknownName,
    /// skill 锁文件有其他硬链接。
    LockLinks,
    /// 另一个进程正在安装 skill，请稍后重试。
    Busy,
    /// skill 工作区凭据无效；请移走 skills/ 后执行 aproxy skill install。
    WorkspaceKey,
    /// skill 工作区内容异常。
    WorkspaceContents,
    /// skill 清单无效。
    Manifest,
    /// skill 包缺少 manifest.json；0.2.0-alpha.4 及更早的发布版不带清单，请使用带清单的后续版本。
    Missing,
    /// skill 压缩包无效。
    Archive,
    /// skill 压缩包条目过多。
    Entries,
    /// skill 压缩包过大。
    Size,
    /// skill 清单版本与请求版本不符。
    Version,
    /// skill 包缺少清单中的文件。
    FileMissing,
    /// skill 文件大小与清单不符。
    FileSize,
    /// skill 文件校验失败。
    Digest,
    /// skill 包条目的类型、大小或压缩方式不合要求。
    Entry,
    /// manifest.json 不是普通文件；请移走它后执行 aproxy skill install。
    Marker,
    /// skill 备份目录不在清单中。
    Backup,
    /// 拒绝覆盖安装器不拥有的目录。
    Unowned(String),
    /// 回滚未完成，原文件的备份仍在。
    Recovery(String),
    /// 恢复目的地已被占用。
    Exists(String),
    /// skill 加锁失败。
    Locking(Box<Self>),
    /// skill 下载目录创建失败。
    Stage(Box<Self>),
    /// skill 安装失败。
    Install(Box<Self>),
}
impl Issue {
    /// io 保留的本地分类由显示端选语言；没有分类的系统错误按证据原样显示。
    pub fn from_io(e: &std::io::Error) -> Self {
        e.get_ref()
            .and_then(|s| s.downcast_ref::<Self>())
            .cloned()
            .unwrap_or_else(|| Self::External(e.to_string()))
    }
    /// 对给定接收端说明文件故障，文件与 skill 名按原值带出。
    pub fn render(&self, lang: Lang) -> String {
        match self {
            Self::External(s) => s.clone(),
            Self::TarLimit => tar_limit(lang).to_owned(),
            Self::TarName => tar_name(lang).to_owned(),
            Self::TarCount => tar_count(lang).to_owned(),

            Self::Damaged => damaged(lang).to_owned(),
            Self::Absent => absent(lang).to_owned(),
            Self::Legacy => legacy(lang).to_owned(),
            Self::UnknownFile => unknown_file(lang).to_owned(),
            Self::UnknownName => unknown_name(lang).to_owned(),
            Self::LockLinks => lock_links(lang).to_owned(),
            Self::Busy => busy(lang).to_owned(),
            Self::WorkspaceKey => workspace_key(lang).to_owned(),
            Self::WorkspaceContents => workspace_contents(lang).to_owned(),
            Self::Manifest => manifest(lang).to_owned(),
            Self::Missing => missing(lang).to_owned(),
            Self::Archive => archive(lang).to_owned(),
            Self::Entries => entries(lang).to_owned(),
            Self::Size => size(lang).to_owned(),
            Self::Version => version(lang).to_owned(),
            Self::FileMissing => file_missing(lang).to_owned(),
            Self::FileSize => file_size(lang).to_owned(),
            Self::Digest => digest(lang).to_owned(),
            Self::Entry => entry(lang).to_owned(),
            Self::Marker => marker(lang).to_owned(),
            Self::Backup => backup(lang).to_owned(),
            Self::Unowned(s) => unowned(lang, s),
            Self::Recovery(s) => recovery(lang, s),
            Self::Exists(s) => exists(lang, s),
            Self::Locking(e) => locking(lang, &e.render(lang)),
            Self::Stage(e) => stage(lang, &e.render(lang)),
            Self::Install(e) => install(lang, &e.render(lang)),
        }
    }
}
impl std::fmt::Display for Issue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.render(Lang::En))
    }
}
impl std::error::Error for Issue {}
impl From<Issue> for super::super::InstallFailure {
    fn from(e: Issue) -> Self {
        Self::because("install_failed", super::super::reason::Cause::Skill(e))
    }
}
