//! 先在独占的同卷目录中展开整个白名单；公开目录直到最后才变动，清单最后落位。

use super::manifest::{MARKER, MAX_MANIFEST, MAX_RATIO, Manifest};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{self, Read, Write},
    path::Path,
};

/// 限制压缩包目录表占用；合法包的原文总量上限还要低一半。
const MAX_ARCHIVE: u64 = 32 << 20;
/// 老发布包没有清单，不能用本地计算的摘要冒充随版本发布的白名单。
const MISSING: &str =
    "skill 包缺少 manifest.json；0.2.0-alpha.4 及更早的发布版不带清单，请使用带清单的后续版本";

/// 打包校验与安装共用解包器：目标目录须由调用者独占且为空。返回额外条目数供报告。
fn unpack(zip: &Path, dest: &Path, version: &str) -> io::Result<(Manifest, usize)> {
    let f = fs::File::open(zip)?;
    if f.metadata()?.len() > MAX_ARCHIVE {
        return Err(io::Error::other("skill 压缩包过大"));
    }
    let mut archive = zip::ZipArchive::new(f).map_err(|_| io::Error::other("skill 压缩包无效"))?;
    let bytes = {
        let mut entry = archive
            .by_name(MARKER)
            .map_err(|_| io::Error::other(MISSING))?;
        check_entry(&entry, MAX_MANIFEST)?;
        let mut buf = Vec::new();
        (&mut entry).take(MAX_MANIFEST + 1).read_to_end(&mut buf)?;
        buf
    };
    let manifest = Manifest::parse(&bytes)?;
    if manifest.version != version {
        return Err(io::Error::other("skill 清单版本与请求版本不符"));
    }
    for (key, record) in &manifest.files {
        let mut entry = archive
            .by_name(key)
            .map_err(|_| io::Error::other("skill 包缺少清单中的文件"))?;
        check_entry(&entry, record.size)?;
        if entry.size() != record.size {
            return Err(io::Error::other("skill 文件大小与清单不符"));
        }
        let path = dest.join(key);
        fs::create_dir_all(path.parent().unwrap())?;
        let mut out = fs::File::create_new(path)?;
        let mut digest = Sha256::new();
        let mut count = 0;
        let mut buf = [0; 8192];
        let mut reader = (&mut entry).take(record.size + 1);
        loop {
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            count += n as u64;
            if count > record.size {
                break;
            }
            digest.update(&buf[..n]);
            out.write_all(&buf[..n])?;
        }
        if count != record.size || super::manifest::hex(&digest.finalize()) != record.sha256 {
            return Err(io::Error::other("skill 文件校验失败"));
        }
    }
    // 保留未知属性；本地副本与发布的清单逐字节相同。
    fs::write(dest.join(MARKER), bytes)?;
    let ignored = archive.len() - manifest.files.len() - 1;
    Ok((manifest, ignored))
}

/// ZIP 的名字和类型位分别检查：FIFO、设备、链接不能因为名字像普通文件就被写出。
fn check_entry<R: Read>(entry: &zip::read::ZipFile<'_, R>, limit: u64) -> io::Result<()> {
    let kind = entry.unix_mode().unwrap_or(0) & 0o170000;
    if !entry.is_file()
        || !matches!(kind, 0 | 0o100000)
        || entry.size() > limit
        || entry.size() > entry.compressed_size().saturating_mul(MAX_RATIO)
    {
        return Err(io::Error::other("skill 包条目的类型、大小或压缩比不合要求"));
    }
    Ok(())
}

/// 移走旧清单后才开始切换目录，最后发布标记。若某一步失败，逆序恢复已经移动的项目。
fn replace(
    root: &Path,
    stage: tempfile::TempDir,
    manifest: &Manifest,
    previous: Option<&Manifest>,
) -> io::Result<()> {
    let src = stage.path().join("incoming");
    let old = stage.path().join("previous");
    fs::create_dir(&old)?;
    let incoming = manifest.names();
    let mut names = incoming.clone();
    if let Some(prev) = previous {
        names.extend(prev.names());
    }
    let mut moved = Vec::new();
    let mut move_one = |from: std::path::PathBuf, to: std::path::PathBuf| -> io::Result<()> {
        // Windows 上 agent 正在短暂读取文档时可能阻止目录改名，留给它关闭句柄的机会。
        for delay in [100, 300] {
            match fs::rename(&from, &to) {
                Ok(()) => {
                    moved.push((from, to));
                    return Ok(());
                }
                Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                    std::thread::sleep(std::time::Duration::from_millis(delay))
                }
                Err(e) => return Err(e),
            }
        }
        fs::rename(&from, &to)?;
        moved.push((from, to));
        Ok(())
    };
    let result = (|| {
        for name in std::iter::once(MARKER).chain(names) {
            match fs::symlink_metadata(root.join(name)) {
                Ok(_) => move_one(root.join(name), old.join(name))?,
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        for name in incoming.into_iter().chain(std::iter::once(MARKER)) {
            move_one(src.join(name), root.join(name))?;
        }
        Ok(())
    })();
    if result.is_err() {
        for (from, to) in moved.into_iter().rev() {
            if fs::rename(to, from).is_err() {
                return Err(io::Error::other(format!(
                    "skill 目录恢复失败，备份保存在 {}",
                    stage.keep().display()
                )));
            }
        }
    }
    result
}

/// 校验完所有文件才替换旧目录，未登记的其他目录仍属于用户。
pub(super) fn install(home: &Path, zip: &Path, version: &str) -> io::Result<(Manifest, usize)> {
    let root = home.join("skills");
    crate::fsx::create_dir_all(&root)?;
    super::installed::directory(&root)?;
    let stage = super::workspace(&root)?;
    let src = stage.path().join("incoming");
    fs::create_dir(&src)?;
    let (m, ignored) = unpack(zip, &src, version)?;
    // 损坏的旧清单不能授权清理任何目录，但不妨碍重新安装修复当前这组文件。
    let prev = super::installed::load(&root).ok();
    replace(&root, stage, &m, prev.as_ref())?;
    Ok((m, ignored))
}

/// 发布端与安装端使用同一个解包器；发布物不能带额外条目，所有正文都必须能按 UTF-8 阅读。
pub fn verify_package(zip: &Path, version: &str) -> io::Result<()> {
    let temp = super::workspace(&std::env::temp_dir())?;
    let (m, ignored) = unpack(zip, temp.path(), version)?;
    if ignored != 0 {
        return Err(io::Error::other("skill 发布包含有未登记的条目"));
    }
    for key in m.files.keys() {
        fs::read_to_string(temp.path().join(key))
            .map_err(|_| io::Error::other("skill 正文不是 UTF-8 文本"))?;
    }
    Ok(())
}
