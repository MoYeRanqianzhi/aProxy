//! 本地清单是读取白名单，不是文件未被改动的证明。输出的必须是刚校验的那块内存。
use super::manifest::{self, MARKER, MAX_MANIFEST, Manifest};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
};

/// 不回显解析错误或正文：即使被换成含凭据的文件，诊断也只给修复办法。
const DAMAGED: &str = "已安装的 skill 与清单不符，请执行 aproxy skill install 修复";
/// 空目录或不存在的安装没有可读文档。
pub const ABSENT: &str = "未安装 skill，请执行 aproxy skill install";
/// 旧安装不能作为可信来源，先刷新它的完成标记。
pub const LEGACY: &str = "已安装的 skill 没有清单，请执行 aproxy skill install 更新";

/// 路径检查与打开后的句柄检查都拒绝所有重解析点，不只拒绝符号链接。
fn linked(meta: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    meta.file_type().is_symlink()
}
/// 检查目录本身，不向链接后的目录递归。保留 NotFound 以区分未安装状态。
pub(super) fn directory(path: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if linked(&meta) || !meta.is_dir() {
        return Err(io::Error::other(DAMAGED));
    }
    Ok(())
}
/// 先检查每层路径，再以不跟随链接的方式打开普通文件；Unix 上换成 FIFO 也不能阻塞读者。
fn open(root: &Path, key: &str) -> io::Result<fs::File> {
    directory(root)?;
    let mut path = root.to_path_buf();
    let mut parts = key.split('/').peekable();
    while let Some(p) = parts.next() {
        path.push(p);
        if parts.peek().is_some() {
            directory(&path)?;
        }
    }
    let meta = fs::symlink_metadata(&path)?;
    if linked(&meta) || !meta.is_file() || !path.canonicalize()?.starts_with(root.canonicalize()?) {
        return Err(io::Error::other(DAMAGED));
    }
    let mut opts = fs::OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        opts.custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let f = opts.open(path)?;
    let meta = f.metadata()?;
    if linked(&meta) || !meta.is_file() {
        return Err(io::Error::other(DAMAGED));
    }
    Ok(f)
}
/// 清单也不能经链接读取；只读上限加一，可分辨恰等于上限和越界。
pub(super) fn load(root: &Path) -> io::Result<Manifest> {
    let f = open(root, MARKER)?;
    let mut buf = Vec::new();
    f.take(MAX_MANIFEST + 1).read_to_end(&mut buf)?;
    Manifest::parse(&buf).map_err(|_| io::Error::other(DAMAGED))
}
/// 只在缺标记时区分旧安装和空目录，临时工作区与锁不代表已经装好文档。
fn legacy(root: &Path) -> io::Result<bool> {
    match directory(root) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        r => r?,
    }
    for e in fs::read_dir(root)? {
        let e = e?;
        if !manifest::reserved(&e.file_name().to_string_lossy()) && e.file_type()?.is_dir() {
            return Ok(true);
        }
    }
    Ok(false)
}

/// 每个 skill 的文件相对其目录列举，可直接作为 show 的文件参数。
#[derive(Debug, Serialize)]
pub struct Skill {
    /// 白名单键的首段，不推测其他目录的用途。
    pub name: String,
    /// 这组文档实际描述的版本。
    pub version: String,
    /// 与正在运行的二进制严格比较，包括预发布标记。
    pub matches_binary: bool,
    /// 供有目录访问权限的读者使用的位置。
    pub location: PathBuf,
    /// 按字节排序的文件相对路径。
    pub files: Vec<String>,
}
/// 只读清单的结果：缺清单的旧安装不会被误当成空的有效白名单。
#[derive(Debug)]
pub enum Listing {
    /// 没有安装文档。
    Absent,
    /// 仍是没有清单的目录布局。
    Legacy,
    /// 已验证格式的清单，仅代表文件应该是什么，不担保它们的当前内容。
    Installed(Vec<Skill>),
}
/// 从清单而非扫描文件系统列举文件；同一 skill 的文件按键排序。
pub fn list(home: &Path) -> io::Result<Listing> {
    let root = home.join("skills");
    let m = match load(&root) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Ok(if legacy(&root)? {
                Listing::Legacy
            } else {
                Listing::Absent
            });
        }
        Err(_) => return Err(io::Error::other(DAMAGED)),
    };
    let mut skills = BTreeMap::new();
    for key in m.files.keys() {
        let (name, file) = key.split_once('/').unwrap();
        skills
            .entry(name)
            .or_insert_with(|| Skill {
                name: name.into(),
                version: m.version.clone(),
                matches_binary: m.version == env!("CARGO_PKG_VERSION"),
                location: root.join(name),
                files: Vec::new(),
            })
            .files
            .push(file.into());
    }
    Ok(Listing::Installed(skills.into_values().collect()))
}
/// 名字和相对文件都必须精确命中；只有清单中的键才进入路径构造。
pub fn show(home: &Path, name: &str, file: Option<&str>) -> io::Result<Vec<u8>> {
    let root = home.join("skills");
    let m = load(&root).map_err(|e| {
        if e.kind() == io::ErrorKind::NotFound {
            io::Error::other(if legacy(&root).unwrap_or(false) {
                LEGACY
            } else {
                ABSENT
            })
        } else {
            io::Error::other(DAMAGED)
        }
    })?;
    let file = file.unwrap_or("SKILL.md");
    let (key, record) = m
        .files
        .get_key_value(&format!("{name}/{file}"))
        .filter(|(k, _)| k.split_once('/') == Some((name, file)))
        .ok_or_else(|| io::Error::other("skill 名或文件名不在清单中"))?;
    let read = || -> io::Result<Vec<u8>> {
        let f = open(&root, key)?;
        if f.metadata()?.len() != record.size {
            return Err(io::Error::other(DAMAGED));
        }
        let mut buf = Vec::new();
        f.take(record.size + 1).read_to_end(&mut buf)?;
        if buf.len() as u64 != record.size
            || manifest::hex(&Sha256::digest(&buf)) != record.sha256
            || std::str::from_utf8(&buf).is_err()
        {
            return Err(io::Error::other(DAMAGED));
        }
        Ok(buf)
    };
    read().map_err(|_| io::Error::other(DAMAGED))
}

/// 实际的链接类型由操作系统决定；没有建链接权限时测试要保留可见的原因。
#[cfg(test)]
mod link_tests {
    use super::super::{
        archive,
        archive_tests::{docs, manifest_for, package},
    };
    use super::*;
    /// 目标内容完全匹配也不允许文件链接或目录链接；Windows 无建链接权限时明确报告跳过。
    #[test]
    fn rejects_link_at_file_manifest_and_root() {
        #[cfg(unix)]
        use std::os::unix::fs::{symlink as link_file, symlink as link_dir};
        #[cfg(windows)]
        use std::os::windows::fs::{symlink_dir as link_dir, symlink_file as link_file};
        let h = tempfile::tempdir().unwrap();
        let home = h.path();
        let f = docs();
        let zip = package(home, Some(&manifest_for(&f)), &f);
        archive::install(home, &zip, "1.0.0").unwrap();
        let root = home.join("skills");
        let doc = root.join(f[0].0);
        let target = home.join("original");
        fs::rename(&doc, &target).unwrap();
        if let Err(e) = link_file(&target, &doc) {
            eprintln!("跳过符号链接检查：{e}");
            return;
        }
        assert!(show(home, "aproxy-cli", None).is_err());
        fs::remove_file(&doc).unwrap();
        fs::rename(&target, &doc).unwrap();
        let marker = root.join(MARKER);
        fs::rename(&marker, &target).unwrap();
        link_file(&target, &marker).unwrap();
        assert!(list(home).is_err());
        fs::remove_file(&marker).unwrap();
        fs::rename(&target, &marker).unwrap();
        fs::rename(&root, &target).unwrap();
        link_dir(&target, &root).unwrap();
        assert!(show(home, "aproxy-cli", None).is_err());
    }
    /// Windows 的目录联接可以将清单中的整棵子树重定向，即使内容一样也不能跟随。
    #[test]
    #[cfg(windows)]
    fn rejects_junction_inside_skill() {
        let h = tempfile::tempdir().unwrap();
        let home = h.path();
        let f = docs();
        let zip = package(home, Some(&manifest_for(&f)), &f);
        archive::install(home, &zip, "1.0.0").unwrap();
        let dir = home.join("skills/aproxy-cli/references");
        let other = home.join("references");
        fs::rename(&dir, &other).unwrap();
        let result=std::process::Command::new("powershell.exe").args(["-NoProfile","-NonInteractive","-Command","$ErrorActionPreference='Stop'; New-Item -ItemType Junction -Path $env:TEST_LINK -Target $env:TEST_TARGET | Out-Null"]).env("TEST_LINK",dir).env("TEST_TARGET",other).output().unwrap();
        if !result.status.success() {
            eprintln!(
                "跳过目录联接检查：{}",
                String::from_utf8_lossy(&result.stderr)
            );
            return;
        }
        assert!(show(home, "aproxy-cli", Some("references/a.md")).is_err());
    }
}
