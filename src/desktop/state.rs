//! 窗口私有文件没有恢复进程或打开路径的权力：书签只保存当时已脱敏的摘要。
use super::DesktopHome;
use super::preferences::DesktopToml;
use aproxy_ipc::desktop::*;
use std::{
    collections::BTreeSet,
    fs,
    io::Read,
    path::{Path, PathBuf},
};
const BYTES: usize = 256 * 1024;
const COUNT: usize = 100;
fn limit(resource: LimitResource, max: usize) -> Fault {
    Fault::DesktopLimit {
        resource,
        max: SafeUint::saturating(max as u64),
    }
}
fn valid_bookmarks(list: &[Bookmark]) -> bool {
    let mut seen = BTreeSet::new();
    let s = Default::default();
    let p = Projection::new(&s);
    list.len() <= COUNT
        && list.iter().all(|b| {
            b.method.len() <= 256
                && b.path.len() <= 256
                && b.model.as_ref().is_none_or(|m| m.len() <= 256)
                && seen.insert(&b.r#ref)
                && p.check_ref(&b.r#ref).is_ok()
        })
}
pub(super) fn safe(path: &Path, f: StateFile, dir: bool) -> R<()> {
    let meta = match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        other => other.map_err(|_| Fault::DesktopStateIo { file: f })?,
    };
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return Err(Fault::DesktopStateIo { file: f });
        }
    }
    if meta.file_type().is_symlink() || (dir && !meta.is_dir()) || (!dir && !meta.is_file()) {
        return Err(Fault::DesktopStateIo { file: f });
    }
    Ok(())
}
pub(super) struct Store {
    pub(super) dir: PathBuf,
    _lock: crate::install::InstallLock,
}
impl Store {
    pub(super) fn open(home: &Path, f: StateFile) -> R<Self> {
        let dir = home.join("desktop");
        safe(&dir, f, true)?;
        let lock = dir.join("state.lock");
        safe(&lock, f, false)?;
        let guard = crate::install::wait_lock_path(&lock, std::time::Duration::from_secs(1))
            .map_err(|_| Fault::DesktopStateIo { file: f })?;
        Ok(Self { dir, _lock: guard })
    }
    fn save_bookmarks(&self, data: &[Bookmark]) -> R<()> {
        let f = StateFile::Bookmarks;
        let path = self.dir.join("bookmarks.json");
        safe(&path, f, false)?;
        let bytes = serde_json::to_vec(data).map_err(|_| Fault::DesktopStateIo { file: f })?;
        if bytes.len() > BYTES {
            return Err(limit(LimitResource::BookmarkBytes, BYTES));
        }
        crate::fsx::write_atomic_private(&path, &bytes)
            .map_err(|_| Fault::DesktopStateIo { file: f })
    }
    fn bookmarks(&self) -> R<(Vec<Bookmark>, bool)> {
        let f = StateFile::Bookmarks;
        let path = self.dir.join("bookmarks.json");
        safe(&path, f, false)?;
        let file = match fs::File::open(path) {
            Ok(v) => v,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((Vec::new(), false)),
            Err(_) => return Err(Fault::DesktopStateIo { file: f }),
        };
        let mut bytes = vec![];
        file.take((BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| Fault::DesktopStateIo { file: f })?;
        if bytes.len() <= BYTES
            && let Ok(v) = serde_json::from_slice::<Vec<Bookmark>>(&bytes)
            && valid_bookmarks(&v)
        {
            return Ok((v, false));
        }
        self.save_bookmarks(&[])?;
        Ok((Vec::new(), true))
    }
}
// 磁盘可能由旧版本写过；读出口仍按当前凭据重新投影，不把磁盘文件直接当作桥的应答。
fn public_preferences(mut p: Preferences, s: Option<&aproxy_ipc::redact::Secrets>) -> Preferences {
    let Some(s) = s else {
        return super::preferences::defaults();
    };
    if s.replace(&p.lang) != p.lang {
        p.lang = "system".into()
    }
    if s.replace(&p.mascot) != p.mascot {
        p.mascot = "tumbler".into()
    }
    if s.find(p.theme.as_bytes()).is_some() {
        p.theme = "builtin".into();
    }
    p
}
fn public_pins(mut list: Vec<Bookmark>, s: Option<&aproxy_ipc::redact::Secrets>) -> Vec<Bookmark> {
    let Some(s) = s else { return Vec::new() };
    let p = Projection::new(s);
    list.retain_mut(|b| {
        if p.check_ref(&b.r#ref).is_err() {
            return false;
        }
        b.method = s.replace(&b.method);
        b.path = s.replace(&aproxy_ipc::redact::url(&b.path));
        b.model = b.model.take().map(|v| s.replace(&v));
        true
    });
    list
}
impl DesktopHome {
    async fn state_work<T: Send + 'static>(
        &self,
        f: StateFile,
        work: impl FnOnce(DesktopHome, Store) -> R<T> + Send + 'static,
    ) -> R<T> {
        let _queue = self.state_serial.lock().await;
        let host = self.clone();
        tokio::task::spawn_blocking(move || {
            let store = Store::open(&host.home, f)?;
            work(host, store)
        })
        .await
        .map_err(|_| Fault::DesktopStateIo { file: f })?
    }
}
impl StateOps for DesktopHome {
    fn read_desktop_state(&self) -> Task<'_, DesktopState> {
        Box::pin(self.state_work(StateFile::Preferences, move |host, store| {
            let (bookmarks, repaired) = store.bookmarks()?;
            let mask = host.display_secrets();
            let (preferences, enabled_extensions, desktop_toml) = match DesktopToml::read(&store) {
                Ok(doc) => {
                    let (p, mut enabled, issues) = doc.preferences();
                    enabled.retain(|id| {
                        mask.as_ref()
                            .is_some_and(|s| s.find(id.as_bytes()).is_none())
                    });
                    (
                        Some(public_preferences(p, mask.as_ref())),
                        Some(enabled),
                        issues,
                    )
                }
                Err(issue) => (None, None, vec![issue]),
            };
            Ok(DesktopState {
                preferences,
                enabled_extensions,
                desktop_toml,
                bookmarks: public_pins(bookmarks, mask.as_ref()),
                reset: if repaired {
                    vec![StateFile::Bookmarks]
                } else {
                    vec![]
                },
            })
        }))
    }
    fn set_preferences(&self, patch: PreferencesPatch) -> Task<'_, Preferences> {
        Box::pin(self.state_work(StateFile::Preferences, move |host, store| {
            let mut doc = DesktopToml::read(&store).map_err(Fault::DesktopToml)?;
            doc.patch(patch)?;
            doc.save(&store)?;
            Ok(public_preferences(
                doc.preferences().0,
                host.display_secrets().as_ref(),
            ))
        }))
    }
    fn set_pinned(&self, change: PinChange) -> Task<'_, Vec<Bookmark>> {
        Box::pin(self.state_work(StateFile::Bookmarks, move |host, store| {
            let (mut list, reset) = store.bookmarks()?;
            if reset {
                return Err(Fault::DesktopStateReset {
                    file: StateFile::Bookmarks,
                });
            }
            match change {
                PinChange::Unpin(r) => list.retain(|b| b.r#ref != r),
                PinChange::Pin(b) => {
                    if !valid_bookmarks(std::slice::from_ref(&b)) {
                        return Err(Fault::DesktopBadInput {});
                    }
                    if list.iter().any(|v| v.r#ref == b.r#ref) {
                        return Ok(public_pins(list, host.display_secrets().as_ref()));
                    }
                    if list.len() >= COUNT {
                        return Err(limit(LimitResource::BookmarkCount, COUNT));
                    }
                    let safe = public_pins(vec![b], host.display_secrets().as_ref())
                        .pop()
                        .ok_or(Fault::DesktopUnavailable {})?;
                    list.insert(0, safe);
                }
            }
            store.save_bookmarks(&list)?;
            Ok(public_pins(list, host.display_secrets().as_ref()))
        }))
    }
    fn about(&self) -> R<About> {
        Ok(About {
            version: env!("CARGO_PKG_VERSION").into(),
            variant: if cfg!(feature = "desktop") {
                Variant::Desktop
            } else {
                Variant::Headless
            },
            home: self
                .display_secrets()
                .map_or_else(|| "***".into(), |s| s.replace(&self.home.to_string_lossy())),
        })
    }
}
