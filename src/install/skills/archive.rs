//! 先在独占的同卷目录中展开整个白名单；公开目录直到最后才变动，清单最后落位。

use super::manifest::{MARKER, MAX_MANIFEST, Manifest};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{self, Read, Seek, SeekFrom, Write},
    path::Path,
};

/// 限制压缩包目录表占用；合法包的原文总量上限还要低一半。
const MAX_ARCHIVE: u64 = 32 << 20;
/// 老发布包没有清单，不能用本地计算的摘要冒充随版本发布的白名单。
const MISSING: &str =
    "skill 包缺少 manifest.json；0.2.0-alpha.4 及更早的发布版不带清单，请使用带清单的后续版本";

/// 索引里也包含不在清单中的条目；构造 ZIP 索引之前限制总数，不能只限制展开的文件数。
const MAX_ENTRIES: u64 = 2 * super::manifest::MAX_FILES as u64 + 1;

/// 发布包无前缀、无尾注。只允许一个候选结束记录，避免 ZIP 读者解析失败后回溯到未检查的索引。
/// ZIP64 使用定位器中的绝对偏移，两个计数字段均从 64 位记录核对，不把占位的 0xffff 当成数量。
fn check_directory_budget(f: &mut fs::File, limit: u64) -> io::Result<()> {
    let invalid = || io::Error::other("skill 压缩包无效");
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
                return Err(io::Error::other("skill 压缩包条目过多"));
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
        return Err(io::Error::other("skill 压缩包条目过多"));
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
        return Err(io::Error::other("skill 压缩包过大"));
    }
    check_directory_budget(&mut f, MAX_ENTRIES)?;
    let mut archive = zip::ZipArchive::with_config(
        zip::read::Config {
            archive_offset: zip::read::ArchiveOffset::Known(0),
        },
        f,
    )
    .map_err(|_| io::Error::other("skill 压缩包无效"))?;
    archive
        .validate_header_consistency()
        .map_err(|_| io::Error::other("skill 压缩包无效"))?;
    let bytes = {
        let mut entry = archive.by_name(MARKER).map_err(|e| match e {
            zip::result::ZipError::FileNotFound => io::Error::other(MISSING),
            _ => io::Error::other("skill 压缩包无效"),
        })?;
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
        // 打包器只写 stored；数据长度就是占用的字节数，不依赖压缩头声称的比值。
        || entry.compression() != zip::CompressionMethod::Stored
        || entry.size() != entry.compressed_size()
    {
        return Err(io::Error::other(
            "skill 包条目的类型、大小或压缩方式不合要求",
        ));
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
    let owned = previous.map(Manifest::names).unwrap_or_default();
    for name in incoming.difference(&owned) {
        match fs::symlink_metadata(root.join(name)) {
            Ok(_) => {
                return Err(io::Error::other(format!(
                    "目录 {name} 不属于上次安装；请移走该目录，或移走 skills/ 后执行 aproxy skill install"
                )));
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
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
    // 清单损坏时没有任何目录获准被替换。
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

#[cfg(test)]
mod directory_tests {
    use super::*;
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
