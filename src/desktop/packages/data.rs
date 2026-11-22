//! 扩展数据只认已安装的扩展 ID。所有配额检查、写入和删除都在同一把桌面状态锁下完成。
use super::*;
use serde_json::{Map, Value};
const STORAGE: u64 = 4 << 20;
const QUOTA: u64 = 512 << 20;
const FILES: u64 = 1000;
/// 验证磁盘上的入口与包种类，不能把主题或遗留数据目录当作已安装扩展。
pub(super) fn extension(store: &Store, id: &str) -> R<()> {
    let d = lifecycle::read_dir_id(store, id)?;
    let m = installed_manifest(&d, id)?;
    if m.kind != PackageKind::Extension {
        return Err(bad("not-extension", None));
    }
    file(&d, "main.mjs")?;
    Ok(())
}
fn valid_name(s: &str) -> bool {
    (1..=128).contains(&s.len())
        && !s.starts_with('.')
        && !s.ends_with('.')
        && !preferences::device(s)
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}
fn name(s: &str) -> R<()> {
    if valid_name(s) {
        Ok(())
    } else {
        Err(Fault::DesktopBadInput {})
    }
}
fn storage_key(s: &str) -> bool {
    (1..=256).contains(&s.chars().count()) && !s.chars().any(char::is_control)
}
fn data_dir(store: &Store, id: &str, create: bool) -> R<Option<Dir>> {
    let mut d = Dir::open_dir(&store.dir, true).map_err(|e| file_error(e, None))?;
    for k in ["extension-data", id] {
        let Some(child) = d.directory(k, create).map_err(|e| file_error(e, None))? else {
            return Ok(None);
        };
        d = child;
    }
    Ok(Some(d))
}
fn files(d: &Dir, create: bool) -> R<Option<Dir>> {
    d.directory("files", create)
        .map_err(|e| file_error(e, None))
}
fn storage(d: &Dir) -> R<Map<String, Value>> {
    match d.node("storage.json").map_err(|e| file_error(e, None))? {
        None => Ok(Map::new()),
        Some(Node::Dir(_)) => Err(bad("file-type", None)),
        Some(Node::File(f)) => {
            let b = read_bound(f, STORAGE, "storage-bytes", None)?;
            serde_json::from_slice::<Map<String, Value>>(&b)
                .ok()
                .filter(|m| m.keys().all(|k| storage_key(k)))
                .ok_or_else(|| bad("corrupt-storage", None))
        }
    }
}
/// 从该扩展的存储文件读出对象；尚未写入时返回空对象，不创建目录。
pub(super) fn storage_read(store: &Store, id: &str) -> R<Map<String, Value>> {
    extension(store, id)?;
    match data_dir(store, id, false)? {
        Some(d) => storage(&d),
        None => Ok(Map::new()),
    }
}
/// 只在一次加锁的写入中合并改动并计算总配额，显式重置可以恢复损坏存储。
pub(super) fn storage_write(
    store: &Store,
    id: &str,
    changes: Map<String, Value>,
    reset: bool,
) -> R<Map<String, Value>> {
    extension(store, id)?;
    let d = data_dir(store, id, true)?.unwrap();
    let mut m = if reset { Map::new() } else { storage(&d)? };
    for (k, v) in changes {
        if !storage_key(&k) {
            return Err(Fault::DesktopBadInput {});
        }
        if v.is_null() {
            m.remove(&k);
        } else {
            m.insert(k, v);
        }
    }
    let bytes = serde_json::to_vec(&m).map_err(|_| Fault::DesktopBadInput {})?;
    if bytes.len() as u64 > STORAGE {
        return Err(limit("storage-bytes", STORAGE, None));
    }
    if let Some(f) = files(&d, false)?
        && inventory(&f)?.bytes.saturating_add(bytes.len() as u64) > QUOTA
    {
        return Err(limit("extension-bytes", QUOTA, None));
    }
    d.write("storage.json", &bytes)
        .map_err(|e| file_error(e, None))?;
    Ok(m)
}
fn info(name: &str, md: &fs::Metadata) -> R<FileInfo> {
    let modified = md
        .modified()
        .map_err(|_| bad("io", None))?
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    Ok(FileInfo {
        name: name.into(),
        bytes: md.len(),
        modified_at: modified.min(SafeUint::MAX as u128) as u64,
    })
}
struct Inventory {
    count: u64,
    bytes: u64,
    list: Vec<FileInfo>,
}
fn inventory(d: &Dir) -> R<Inventory> {
    let mut v = Inventory {
        count: 0,
        bytes: 0,
        list: vec![],
    };
    for n in d.names().map_err(|e| file_error(e, None))? {
        match d.node(&n) {
            Ok(Some(Node::File(f))) => {
                let md = f.metadata().map_err(|_| bad("io", None))?;
                v.count += 1;
                v.bytes = v.bytes.saturating_add(md.len());
                if let Some(s) = n.to_str().filter(|s| valid_name(s)) {
                    v.list.push(info(s, &md)?);
                }
            }
            Err(Why::Io) => return Err(bad("io", None)),
            _ => (),
        }
    }
    v.list.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(v)
}
/// 仅列出有效名称的普通文件；不把不受支持的手写条目暴露给页面。
pub(super) fn list(store: &Store, id: &str) -> R<Vec<FileInfo>> {
    extension(store, id)?;
    if let Some(d) = data_dir(store, id, false)?
        && let Some(f) = files(&d, false)?
    {
        return Ok(inventory(&f)?.list);
    }
    Ok(vec![])
}
/// 返回原始字节，避免大文件经过 JSON 数组编码。
pub(super) fn read(store: &Store, id: &str, n: &str) -> R<Vec<u8>> {
    name(n)?;
    extension(store, id)?;
    let missing = || bad("missing", Some(n));
    let d = data_dir(store, id, false)?.ok_or_else(missing)?;
    let f = files(&d, false)?.ok_or_else(missing)?;
    read_bound(file(&f, n)?, FILE, "file-bytes", Some(n))
}
/// 对替换计入净增量，通过同目录换名一次交付完整文件。
pub(super) fn write(store: &Store, id: &str, n: &str, bytes: Vec<u8>) -> R<FileInfo> {
    name(n)?;
    extension(store, id)?;
    if bytes.len() as u64 > FILE {
        return Err(limit("file-bytes", FILE, Some(n)));
    }
    let d = data_dir(store, id, true)?.unwrap();
    let storage = match d.node("storage.json").map_err(|e| file_error(e, None))? {
        Some(Node::File(f)) => f.metadata().map_err(|_| bad("io", None))?.len(),
        Some(_) => return Err(bad("file-type", None)),
        None => 0,
    };
    let f = files(&d, true)?.unwrap();
    let inv = inventory(&f)?;
    // 以文件系统的实际查找为准：Windows 上不同大小写的名字也可能是在替换同一个文件。
    let old = match f.node(n).map_err(|e| file_error(e, Some(n)))? {
        Some(Node::File(v)) => Some(v.metadata().map_err(|_| bad("io", Some(n)))?.len()),
        Some(_) => return Err(bad("file-type", Some(n))),
        None => None,
    };
    if inv.count + u64::from(old.is_none()) > FILES {
        return Err(limit("extension-files", FILES, None));
    }
    if inv
        .bytes
        .saturating_sub(old.unwrap_or(0))
        .saturating_add(storage)
        .saturating_add(bytes.len() as u64)
        > QUOTA
    {
        return Err(limit("extension-bytes", QUOTA, None));
    }
    f.write(n, &bytes).map_err(|e| file_error(e, Some(n)))?;
    info(n, &file(&f, n)?.metadata().map_err(|_| bad("io", Some(n)))?)
}
/// 删除单个普通文件；重复删除已不存在的条目也是成功。
pub(super) fn delete(store: &Store, id: &str, n: &str) -> R<()> {
    name(n)?;
    extension(store, id)?;
    if let Some(d) = data_dir(store, id, false)?
        && let Some(f) = files(&d, false)?
    {
        f.delete(n).map_err(|e| file_error(e, Some(n)))?;
    }
    Ok(())
}
