//! 先在独占的同卷目录中展开整个白名单；公开目录直到最后才变动，清单最后落位。

use super::issue::Issue;
use super::manifest::{MARKER, MAX_MANIFEST, Manifest};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{self, Read, Seek, SeekFrom, Write},
    path::Path,
};

/// 限制压缩包目录表占用；合法包的原文总量上限还要低一半。
pub(crate) const MAX_ARCHIVE: u64 = 32 << 20;

/// 索引里也包含不在清单中的条目；构造 ZIP 索引之前限制总数，不能只限制展开的文件数。
const MAX_ENTRIES: u64 = 2 * super::manifest::MAX_FILES as u64 + 1;

/// 发布包无前缀、无尾注。只允许一个候选结束记录，避免 ZIP 读者解析失败后回溯到未检查的索引。
/// ZIP64 使用定位器中的绝对偏移，两个计数字段均从 64 位记录核对，不把占位的 0xffff 当成数量。
fn check_directory_budget(f: &mut fs::File, limit: u64) -> io::Result<()> {
    let invalid = || io::Error::other(Issue::Archive);
    let len = f.metadata()?.len();
    let span = len.min(22 + (1 << 16));
    f.seek(SeekFrom::End(-(span as i64)))?;
    let mut tail = vec![0; span as usize];
    f.read_exact(&mut tail)?;
    let end = tail.len().checked_sub(22).ok_or_else(invalid)?;
    if &tail[end..end + 4] != b"PK\x05\x06"
        || tail[end + 20..] != [0, 0]
        || tail[..end].windows(4).any(|w| w == b"PK\x05\x06")
    {
        return Err(invalid());
    }
    let counts = if end >= 20 && &tail[end - 20..end - 16] == b"PK\x06\x07" {
        // 库只在传统字段饱和时切到 ZIP64；两种读法都不得超过预算。
        for off in [8, 10] {
            let n = u16::from_le_bytes(tail[end + off..end + off + 2].try_into().unwrap());
            if n != u16::MAX && u64::from(n) > limit {
                return Err(io::Error::other(Issue::Entries));
            }
        }
        let start = u64::from_le_bytes(tail[end - 12..end - 4].try_into().unwrap());
        // 小型发布包不用 ZIP64 的可扩展数据区，固定头也不会按外部长度分配内存。
        if start.checked_add(56 + 20 + 22) != Some(len) {
            return Err(invalid());
        }
        f.seek(SeekFrom::Start(start))?;
        let mut record = [0u8; 56];
        f.read_exact(&mut record)?;
        if &record[..4] != b"PK\x06\x06"
            || u64::from_le_bytes(record[4..12].try_into().unwrap()) != 44
        {
            return Err(invalid());
        }
        [
            u64::from_le_bytes(record[24..32].try_into().unwrap()),
            u64::from_le_bytes(record[32..40].try_into().unwrap()),
        ]
    } else {
        [
            u16::from_le_bytes(tail[end + 8..end + 10].try_into().unwrap()) as u64,
            u16::from_le_bytes(tail[end + 10..end + 12].try_into().unwrap()) as u64,
        ]
    };
    if counts.iter().any(|&n| n > limit) {
        return Err(io::Error::other(Issue::Entries));
    }
    if counts[0] != counts[1] {
        return Err(invalid());
    }
    Ok(())
}

/// 打包校验与安装共用解包器：目标目录须由调用者独占且为空。返回额外条目数供报告。
fn unpack(zip: &Path, dest: &Path, version: &str) -> io::Result<(Manifest, usize)> {
    let mut f = fs::File::open(zip)?;
    if f.metadata()?.len() > MAX_ARCHIVE {
        return Err(io::Error::other(Issue::Size));
    }
    check_directory_budget(&mut f, MAX_ENTRIES)?;
    let mut archive = zip::ZipArchive::with_config(
        zip::read::Config {
            archive_offset: zip::read::ArchiveOffset::Known(0),
        },
        f,
    )
    .map_err(|_| io::Error::other(Issue::Archive))?;
    archive
        .validate_header_consistency()
        .map_err(|_| io::Error::other(Issue::Archive))?;
    let bytes = {
        let mut entry = archive.by_name(MARKER).map_err(|e| match e {
            // 老发布包没有清单，不能用本地计算的摘要冒充随版本发布的白名单。
            zip::result::ZipError::FileNotFound => io::Error::other(Issue::Missing),
            _ => io::Error::other(Issue::Archive),
        })?;
        check_entry(&entry, MAX_MANIFEST)?;
        let mut buf = Vec::new();
        (&mut entry).take(MAX_MANIFEST + 1).read_to_end(&mut buf)?;
        buf
    };
    let manifest = Manifest::parse(&bytes)?;
    if manifest.version != version {
        return Err(io::Error::other(Issue::Version));
    }
    for (key, record) in &manifest.files {
        let mut entry = archive
            .by_name(key)
            .map_err(|_| io::Error::other(Issue::FileMissing))?;
        check_entry(&entry, record.size)?;
        if entry.size() != record.size {
            return Err(io::Error::other(Issue::FileSize));
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
            return Err(io::Error::other(Issue::Digest));
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
        // 打包器只写 stored；数据长度就是占用的字节数，不依赖压缩头声称的比值。
        || entry.compression() != zip::CompressionMethod::Stored
        || entry.size() != entry.compressed_size()
    {
        return Err(io::Error::other(Issue::Entry));
    }
    Ok(())
}

/// 移走旧清单后才开始切换目录，最后发布标记。若某一步失败，逆序恢复已经移动的项目。
fn replace(
    root: &Path,
    stage: &Path,
    manifest: &Manifest,
    previous: Option<&Manifest>,
) -> io::Result<()> {
    let src = stage.join("incoming");
    let old = stage.join("previous");
    let incoming = manifest.names();
    let owned = previous.map(Manifest::names).unwrap_or_default();
    for name in incoming.difference(&owned) {
        match fs::symlink_metadata(root.join(name)) {
            Ok(_) => {
                return Err(io::Error::other(Issue::Unowned((*name).into())));
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    // 清单位置也可能被用户目录占据；这个名字只预留给标记文件，不授权删除目录树。
    match fs::symlink_metadata(root.join(MARKER)) {
        Ok(m) if !m.is_file() => {
            return Err(io::Error::other(Issue::Marker));
        }
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    let mut names = incoming.clone();
    if let Some(prev) = previous {
        names.extend(prev.names());
    }
    fs::create_dir(&old)?;
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
                return Err(io::Error::other(Issue::Recovery(
                    stage.display().to_string(),
                )));
            }
        }
    }
    result
}

/// 只用公开路径和两个完整清单判断落位进度：新清单仍在 incoming 表示尚未提交。
/// 先退回已发布的新目录，再恢复旧目录，最后恢复旧标记；恢复本身被中断也能重入。
pub(super) fn restore_interrupted(root: &Path, stage: &Path) -> io::Result<()> {
    let old = stage.join("previous");
    let src = stage.join("incoming");
    if !old.try_exists()? || !src.join(MARKER).try_exists()? {
        return Ok(());
    }
    let incoming = super::installed::load(&src)?;
    for name in incoming.names() {
        if !src.join(name).try_exists()? {
            fs::rename(root.join(name), src.join(name))?;
        }
    }
    let prev = super::installed::load(&old).ok();
    let owned = prev.as_ref().map(Manifest::names).unwrap_or_default();
    // 不覆盖用户在中断期间放到目的地的内容；无法证明归属的备份保留给人工恢复。
    for item in fs::read_dir(&old)?.take(super::manifest::MAX_FILES + 2) {
        let name = item?.file_name();
        if name == MARKER {
            continue;
        }
        if !name.to_str().is_some_and(|n| owned.contains(n)) {
            return Err(io::Error::other(Issue::Backup));
        }
        restore_one(&old.join(&name), &root.join(&name))?;
    }
    match fs::symlink_metadata(old.join(MARKER)) {
        Ok(_) => restore_one(&old.join(MARKER), &root.join(MARKER))?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    Ok(())
}
/// 恢复改名不依赖各系统的覆盖语义，遇到冲突不删除目标。
fn restore_one(from: &Path, to: &Path) -> io::Result<()> {
    match fs::symlink_metadata(to) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => fs::rename(from, to),
        Ok(_) => Err(io::Error::other(Issue::Exists(to.display().to_string()))),
        Err(e) => Err(e),
    }
}
/// 单测用独占的空目录直接检查发布结果；真正的安装由锁内工作区管理整个事务。
#[cfg(test)]
pub(super) fn install(home: &Path, zip: &Path, version: &str) -> io::Result<(Manifest, usize)> {
    let root = home.join("skills");
    crate::fsx::create_dir_all(&root)?;
    super::installed::directory(&root)?;
    let stage = super::workspace(&root)?;
    publish(&root, stage.path(), zip, version)
}
/// 校验完所有文件才替换旧目录，未登记的其他目录仍属于用户。
pub(super) fn publish(
    root: &Path,
    stage: &Path,
    zip: &Path,
    version: &str,
) -> io::Result<(Manifest, usize)> {
    let src = stage.join("incoming");
    fs::create_dir(&src)?;
    let (m, ignored) = unpack(zip, &src, version)?;
    // 清单损坏时没有任何目录获准被替换。
    let prev = super::installed::load(root).ok();
    replace(root, stage, &m, prev.as_ref())?;
    Ok((m, ignored))
}

/// 发布端与安装端使用同一个解包器；发布物不能带额外条目，所有正文都必须能按 UTF-8 阅读。
pub fn verify_package(zip: &Path, version: &str, packaged_crate: Option<&Path>) -> io::Result<()> {
    let expected = verified_marker(zip, version)?;
    if let Some(path) = packaged_crate {
        let input = fs::File::open(path)?;
        if input.metadata()?.len() > MAX_ARCHIVE {
            return Err(io::Error::other("document download exceeds the size limit"));
        }
        let tmp = tempfile::NamedTempFile::new()?;
        crate::install::download::crates::translate_skills(input, tmp.as_file(), version)?;
        if verified_marker(tmp.path(), version)? != expected {
            return Err(io::Error::other(
                "the crate documentation manifest differs from the release bundle",
            ));
        }
    }
    Ok(())
}
/// 只在所有正文都通过安装端校验后返回原清单字节，让渠道比较也涵盖未知字段。
fn verified_marker(zip: &Path, version: &str) -> io::Result<Vec<u8>> {
    let temp = super::workspace(&std::env::temp_dir())?;
    let (m, ignored) = unpack(zip, temp.path(), version)?;
    if ignored != 0 {
        return Err(io::Error::other(
            "the skill release package contains unlisted entries",
        ));
    }
    for key in m.files.keys() {
        fs::read_to_string(temp.path().join(key))
            .map_err(|_| io::Error::other("skill documentation is not UTF-8 text"))?;
    }
    fs::read(temp.path().join(MARKER))
}

#[cfg(test)]
mod directory_tests {
    use super::*;
    /// 最后一项发布改名失败时，已发布的新目录与所有旧项逐项退回，标记也保持原字节。
    #[test]
    fn publication_rename_failure_rolls_back() {
        use super::super::archive_tests::{docs, manifest_for, package};
        let h = tempfile::tempdir().unwrap();
        let f = docs();
        let m = manifest_for(&f);
        let z = package(h.path(), Some(&m), &f);
        let (manifest, _) = install(h.path(), &z, "1.0.0").unwrap();
        let root = h.path().join("skills");
        let marker = fs::read(root.join(MARKER)).unwrap();
        let stage = super::super::workspace(&root).unwrap();
        let src = stage.path().join("incoming");
        fs::create_dir(&src).unwrap();
        for name in manifest.names() {
            fs::create_dir(src.join(name)).unwrap();
        }
        // incoming 没有完成标记：前面的目录改名真实成功，只有最后一个 rename 返回 NotFound。
        assert_eq!(
            replace(&root, stage.path(), &manifest, Some(&manifest))
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound
        );
        assert_eq!(fs::read(root.join(MARKER)).unwrap(), marker);
        for (k, b) in f {
            assert_eq!(fs::read(root.join(k)).unwrap(), b);
        }
    }
    /// 用微型结束记录单测分配前的边界；64 位计数不经过 usize 或 u16 截断。
    #[test]
    fn budgets_before_index_allocation() {
        for wide in [false, true] {
            for count in [2u64, 3, (1 << 32) + 2] {
                if !wide && count > u16::MAX.into() {
                    continue;
                }
                let mut f = tempfile::tempfile().unwrap();
                let mut end = [0u8; 22];
                end[..4].copy_from_slice(b"PK\x05\x06");
                if wide {
                    let mut rec = [0u8; 56];
                    rec[..4].copy_from_slice(b"PK\x06\x06");
                    rec[4] = 44;
                    for off in [24, 32] {
                        rec[off..off + 8].copy_from_slice(&count.to_le_bytes());
                    }
                    f.write_all(&rec).unwrap();
                    let mut loc = [0; 20];
                    loc[..4].copy_from_slice(b"PK\x06\x07");
                    loc[16] = 1;
                    f.write_all(&loc).unwrap();
                } else {
                    for off in [8, 10] {
                        end[off..off + 2].copy_from_slice(&(count as u16).to_le_bytes());
                    }
                }
                f.write_all(&end).unwrap();
                assert_eq!(check_directory_budget(&mut f, 2).is_ok(), count == 2);
                if wide && count == 2 {
                    f.seek(SeekFrom::End(-14)).unwrap();
                    f.write_all(&[3, 0, 3, 0]).unwrap();
                    assert!(check_directory_budget(&mut f, 2).is_err());
                }
            }
        }
    }
}
