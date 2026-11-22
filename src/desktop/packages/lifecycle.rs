//! 包目录、安装来源与窗口设置在同一把锁里一起提交。旧目录在元数据写完前保留，失败才有东西可恢复。
use super::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    installed_at: u64,
    source: PackageSource,
    github: Option<GithubFacts>,
    kind: Option<PackageKind>,
}
#[derive(Serialize, Deserialize)]
struct Records {
    version: u8,
    packages: BTreeMap<String, Record>,
}
struct Ledger {
    path: PathBuf,
    original: Option<Vec<u8>>,
    records: Records,
}
impl Ledger {
    fn read(store: &Store) -> R<Self> {
        let path = store.dir.join("packages.json");
        safe(&path, StateFile::Packages, false)?;
        let original = match fs::File::open(&path) {
            Ok(f) => Some(read_bound(f, 32 << 20, "records-bytes", None)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => return Err(bad("io", None)),
        };
        let records = original
            .as_deref()
            .and_then(|b| serde_json::from_slice::<Records>(b).ok())
            .filter(|v| v.version == 1)
            .unwrap_or_else(|| Records {
                version: 1,
                packages: BTreeMap::new(),
            });
        Ok(Self {
            path,
            original,
            records,
        })
    }
    // TOML 后写：拒绝手工文档的写入时，要还原原始记录字节，包括之前读不懂的版本。
    fn commit(&self, store: &Store, toml: Option<&DesktopToml>) -> R<()> {
        let bytes = serde_json::to_vec(&self.records).map_err(|_| bad("io", None))?;
        if bytes.len() > 32 << 20 {
            return Err(limit("records-bytes", 32 << 20, None));
        }
        crate::fsx::write_atomic_private(&self.path, &bytes).map_err(|_| bad("io", None))?;
        if let Some(doc) = toml
            && let Err(e) = doc.save(store)
        {
            match &self.original {
                Some(b) => crate::fsx::write_atomic_private(&self.path, b),
                None => fs::remove_file(&self.path),
            }
            .map_err(|_| bad("rollback", None))?;
            return Err(e);
        }
        Ok(())
    }
}
fn known(id: &str) -> R<()> {
    if preferences::package_id(id) {
        Ok(())
    } else {
        Err(Fault::DesktopBadInput {})
    }
}
fn data_path(store: &Store, id: &str) -> R<PathBuf> {
    known(id)?;
    let root = store.dir.join("extension-data");
    safe(&root, StateFile::Packages, true)?;
    let d = root.join(id);
    safe(&d, StateFile::Packages, true)?;
    Ok(d)
}
fn data_bytes(store: &Store, id: &str) -> R<u64> {
    let p = data_path(store, id)?;
    if !p.try_exists().map_err(|_| bad("io", None))? {
        return Ok(0);
    }
    let mut dirs = vec![Dir::open(&p).map_err(|e| file_error(e, None))?];
    let mut total = 0u64;
    while let Some(d) = dirs.pop() {
        for name in d.names().map_err(|e| file_error(e, None))? {
            let Some(n) = name.to_str() else { continue };
            match d.node(n) {
                Ok(Some(Node::Dir(child))) => dirs.push(child),
                Ok(Some(Node::File(f))) => {
                    total = total.saturating_add(f.metadata().map_err(|_| bad("io", None))?.len())
                }
                Err(Why::Io) => return Err(bad("io", None)),
                _ => (),
            }
        }
    }
    Ok(total.min(SafeUint::MAX))
}
fn inspect(
    dir: &Dir,
    id: &str,
    record: Option<&Record>,
    enabled: &[String],
    size: u64,
) -> PackageEntry {
    let mut kind = record
        .and_then(|r| r.kind)
        .unwrap_or(PackageKind::Extension);
    let result = (|| {
        let m = installed_manifest(dir, id)?;
        kind = m.kind;
        let theme = match kind {
            PackageKind::Extension => {
                let f = file(dir, "main.mjs")?;
                let b = read_bound(f, FILE, "file-bytes", Some("main.mjs"))?;
                if std::str::from_utf8(&b).is_err() || b.contains(&0) {
                    return Err(bad("text", Some("main.mjs")));
                }
                None
            }
            PackageKind::Theme => Some(
                manifest::object(
                    &read_bound(
                        file(dir, "theme.json")?,
                        JSON,
                        "json-bytes",
                        Some("theme.json"),
                    )?,
                    128,
                )
                .ok_or_else(|| bad("json", Some("theme.json")))?,
            ),
        };
        Ok((m, theme))
    })();
    let (manifest, theme, issue) = match result {
        Ok((m, t)) => (Some(m), t, None),
        Err(Fault::DesktopPackage(i)) => (
            None,
            None,
            Some(EntryIssue {
                reason: if i.reason == "io" {
                    "io"
                } else if i.path.as_deref() == Some("manifest.json") {
                    "manifest"
                } else {
                    "entry"
                },
            }),
        ),
        Err(_) => unreachable!(),
    };
    PackageEntry {
        kind,
        id: id.into(),
        enabled: manifest.is_some()
            && kind == PackageKind::Extension
            && enabled.iter().any(|x| x == id),
        manifest,
        theme,
        issue,
        installed_at: record.map(|r| r.installed_at),
        source: record.map(|r| r.source.clone()),
        github: record.and_then(|r| r.github.clone()),
        data_bytes: size,
    }
}
pub(super) fn read_dir_id(store: &Store, id: &str) -> R<Dir> {
    known(id)?;
    let base = Dir::open(&root(store)?).map_err(|e| file_error(e, None))?;
    match base.node(id).map_err(|e| file_error(e, None))? {
        Some(Node::Dir(d)) => Ok(d),
        _ => Err(bad("not-installed", None)),
    }
}
fn remove_later(p: &Path) {
    if let Err(e) = fs::remove_dir_all(p) {
        tracing::warn!(error=%e,"Could not remove retired package directory");
    }
}
fn rename(a: &Path, b: &Path) -> R<()> {
    fs::rename(a, b).map_err(|_| bad("io", None))
}
/// 首次枚举时收走进程崩溃留下的内部目录；先检查后列出的窗口仍然拥有自己的暂存令牌。
pub(super) fn list(host: &DesktopHome, store: &Store) -> R<Vec<PackageEntry>> {
    let path = root(store)?;
    let mut out = Vec::new();
    if !path.try_exists().map_err(|_| bad("io", None))? {
        host.listed.store(true, Ordering::Relaxed);
        return Ok(out);
    }
    let first = !host.listed.load(Ordering::Relaxed);
    let live = host
        .package
        .lock()
        .unwrap()
        .as_ref()
        .map(|p| p.stage.path().to_owned());
    let base = Dir::open(&path).map_err(|e| file_error(e, None))?;
    let records = Ledger::read(store)?;
    let (_, enabled, _) = DesktopToml::read(store)
        .map_err(Fault::DesktopToml)?
        .preferences();
    for n in base.names().map_err(|e| file_error(e, None))? {
        let Some(id) = n.to_str() else { continue };
        if first && (id.starts_with(".staging-") || id.starts_with(".old-")) {
            let p = path.join(id);
            if Some(&p) != live.as_ref() && safe(&p, StateFile::Packages, true).is_ok() {
                remove_later(&p)
            }
            continue;
        }
        if !preferences::package_id(id) {
            continue;
        }
        if let Some(Node::Dir(d)) = match base.node(id) {
            Ok(n) => n,
            Err(Why::Link | Why::FileType) => None,
            Err(e) => return Err(file_error(e, None)),
        } {
            out.push(inspect(
                &d,
                id,
                records.records.packages.get(id),
                &enabled,
                data_bytes(store, id)?,
            ));
        }
    }
    host.listed.store(true, Ordering::Relaxed);
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}
/// 只交付检查时那份副本；任意失败不会把别的待安装操作当作同一次确认。
pub(super) fn install(
    host: &DesktopHome,
    store: &Store,
    token: &str,
    enable: bool,
) -> R<PackageEntry> {
    let mut guard = host.package.lock().unwrap();
    current(&mut guard, token, Instant::now())?;
    let pending = guard.take().unwrap();
    let c = pending.check;
    let id = &c.manifest.id;
    let kind = c.manifest.kind;
    let path = root(store)?;
    let dest = path.join(id);
    safe(&dest, StateFile::Packages, true)?;
    let old = path.join(format!(".old-{token}"));
    let upgrade = dest.try_exists().map_err(|_| bad("io", None))?;
    if upgrade {
        let m = installed_manifest(&read_dir_id(store, id)?, id)?;
        if m.kind != kind {
            return Err(bad("kind-change", None));
        }
        if c.installed.as_ref().is_none_or(|i| i.version != m.version) {
            return Err(bad("changed", None));
        }
    } else if c.installed.is_some() {
        return Err(bad("changed", None));
    }
    let mut doc = DesktopToml::read(store).map_err(Fault::DesktopToml)?;
    let changed = !upgrade && kind == PackageKind::Extension && doc.enable(id, enable)?;
    let (_, enabled, _) = doc.preferences();
    let size = data_bytes(store, id)?;
    let mut ledger = Ledger::read(store)?;
    let when = crate::watchdog::now_millis();
    ledger.records.packages.insert(
        id.clone(),
        Record {
            kind: Some(kind),
            installed_at: when,
            source: c.source.clone(),
            github: pending.github.clone(),
        },
    );
    if upgrade {
        rename(&dest, &old)?
    }
    let result = rename(pending.stage.path(), &dest).and_then(|()| {
        if let Err(e) = ledger.commit(store, changed.then_some(&doc)) {
            fs::rename(&dest, pending.stage.path()).map_err(|_| bad("rollback", None))?;
            return Err(e);
        }
        Ok(())
    });
    if let Err(e) = result {
        if upgrade {
            fs::rename(&old, &dest).map_err(|_| bad("rollback", None))?
        }
        return Err(e);
    }
    if upgrade {
        remove_later(&old)
    }
    Ok(PackageEntry {
        id: id.clone(),
        kind,
        enabled: kind == PackageKind::Extension && enabled.contains(id),
        manifest: Some(c.manifest),
        theme: c.theme,
        source: Some(c.source),
        github: pending.github,
        installed_at: Some(when),
        data_bytes: size,
        issue: None,
    })
}
/// 即使包被手改坏了也能停用；启用必须先确认种类与入口仍有效。
pub(super) fn set_enabled(store: &Store, id: &str, on: bool) -> R<PackageEntry> {
    let d = read_dir_id(store, id)?;
    let mut doc = DesktopToml::read(store).map_err(Fault::DesktopToml)?;
    let ledger = Ledger::read(store)?;
    let mut entry = inspect(
        &d,
        id,
        ledger.records.packages.get(id),
        &doc.preferences().1,
        data_bytes(store, id)?,
    );
    if on && let Some(e) = entry.issue.clone() {
        return Err(bad(e.reason, None));
    }
    if entry.kind != PackageKind::Extension {
        return Err(bad("not-extension", None));
    }
    if doc.enable(id, on)? {
        doc.save(store)?
    }
    entry.enabled = on;
    Ok(entry)
}
/// 暂存旧数据直到包记录与 TOML 都提交；取消保留数据的选择不能误删相邻扩展。
pub(super) fn uninstall(store: &Store, id: &str, remove_data: bool) -> R<()> {
    drop(read_dir_id(store, id)?);
    let path = root(store)?;
    let dest = path.join(id);
    let data = data_path(store, id)?;
    let mut doc = DesktopToml::read(store).map_err(Fault::DesktopToml)?;
    let changed = doc.uninstall(id, remove_data)?;
    let mut ledger = Ledger::read(store)?;
    ledger.records.packages.remove(id);
    let backup = tempfile::Builder::new()
        .prefix(".old-")
        .tempdir_in(path)
        .map_err(|_| bad("io", None))?
        .keep();
    let pack = backup.join("package");
    let trash = backup.join("data");
    let purge = remove_data && data.try_exists().map_err(|_| bad("io", None))?;
    rename(&dest, &pack)?;
    let result = (|| {
        if purge {
            rename(&data, &trash)?;
        }
        if let Err(e) = ledger.commit(store, changed.then_some(&doc)) {
            if purge {
                fs::rename(&trash, &data).map_err(|_| bad("rollback", None))?
            }
            return Err(e);
        }
        Ok(())
    })();
    if let Err(e) = result {
        fs::rename(&pack, &dest).map_err(|_| bad("rollback", None))?;
        return Err(e);
    }
    remove_later(&backup);
    Ok(())
}
