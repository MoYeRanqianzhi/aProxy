//! 窗口私有文件没有恢复进程或打开路径的权力：书签只保存当时已脱敏的摘要。
use super::DesktopHome;
use aproxy_ipc::desktop::*;
use serde::{Serialize, de::DeserializeOwned};
use std::{
    collections::BTreeSet,
    fs,
    io::Read,
    path::{Path, PathBuf},
};
const BYTES: usize = 256 * 1024;
const COUNT: usize = 100;
fn defaults() -> Preferences {
    Preferences {
        lang: "system".into(),
        mode: Mode::System,
        palette: Palette::Standard,
        mica: false,
        mascot: "tumbler".into(),
        theme: None,
    }
}
fn limit(resource: LimitResource, max: usize) -> Fault {
    Fault::DesktopLimit {
        resource,
        max: SafeUint::saturating(max as u64),
    }
}
fn prefs(p: &Preferences) -> R<()> {
    let mut subtags = p.lang.split('-');
    let first = subtags.next().unwrap_or_default();
    let lang =
        (2..=8).contains(&first.len()) || (matches!(first, "i" | "x") && p.lang.contains('-'));
    if p.lang.len() > 35
        || !lang
        || !first.bytes().all(|b| b.is_ascii_alphabetic())
        || !subtags
            .all(|t| !t.is_empty() && t.len() <= 8 && t.bytes().all(|b| b.is_ascii_alphanumeric()))
        || !(1..=64).contains(&p.mascot.len())
        || !p
            .mascot
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err(Fault::DesktopBadInput {});
    }
    if let Some(theme) = &p.theme {
        if theme.len() > 16 * 1024 {
            return Err(limit(LimitResource::ThemeBytes, 16 * 1024));
        }
        if !theme
            .strip_prefix("aproxy-theme-v1:")
            .is_some_and(|s| serde_json::from_str::<serde_json::Value>(s).is_ok())
        {
            return Err(Fault::DesktopBadInput {});
        }
    }
    Ok(())
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
fn name(f: StateFile) -> &'static str {
    match f {
        StateFile::Preferences => "preferences.json",
        StateFile::Bookmarks => "bookmarks.json",
    }
}
fn safe(path: &Path, f: StateFile, dir: bool) -> R<()> {
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
struct Store {
    dir: PathBuf,
    _lock: crate::install::InstallLock,
}
impl Store {
    fn open(home: &Path, f: StateFile) -> R<Self> {
        let dir = home.join("desktop");
        safe(&dir, f, true)?;
        let lock = dir.join("state.lock");
        safe(&lock, f, false)?;
        let guard = crate::install::try_lock_path(&lock)
            .map_err(|_| Fault::DesktopStateIo { file: f })?
            .ok_or(Fault::DesktopConflict {})?;
        Ok(Self { dir, _lock: guard })
    }
    fn save<T: Serialize + ?Sized>(&self, f: StateFile, data: &T) -> R<()> {
        let path = self.dir.join(name(f));
        safe(&path, f, false)?;
        let bytes = serde_json::to_vec(data).map_err(|_| Fault::DesktopStateIo { file: f })?;
        if bytes.len() > BYTES {
            return Err(limit(
                match f {
                    StateFile::Bookmarks => LimitResource::BookmarkBytes,
                    StateFile::Preferences => LimitResource::PreferenceBytes,
                },
                BYTES,
            ));
        }
        crate::fsx::write_atomic_private(&path, &bytes)
            .map_err(|_| Fault::DesktopStateIo { file: f })
    }
    fn read<T: DeserializeOwned + Serialize>(
        &self,
        f: StateFile,
        default: impl FnOnce() -> T,
        valid: impl Fn(&T) -> bool,
    ) -> R<(T, bool)> {
        let path = self.dir.join(name(f));
        safe(&path, f, false)?;
        let file = match fs::File::open(path) {
            Ok(v) => v,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((default(), false)),
            Err(_) => return Err(Fault::DesktopStateIo { file: f }),
        };
        let mut bytes = vec![];
        file.take((BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| Fault::DesktopStateIo { file: f })?;
        if bytes.len() <= BYTES
            && let Ok(v) = serde_json::from_slice(&bytes)
            && valid(&v)
        {
            return Ok((v, false));
        }
        let initial = default();
        self.save(f, &initial)?;
        Ok((initial, true))
    }
    fn preferences(&self) -> R<(Preferences, bool)> {
        self.read(StateFile::Preferences, defaults, |p| prefs(p).is_ok())
    }
    fn bookmarks(&self) -> R<(Vec<Bookmark>, bool)> {
        self.read(StateFile::Bookmarks, Vec::new, |b| valid_bookmarks(b))
    }
}
// 磁盘可能由旧版本写过；读出口仍按当前凭据重新投影，不把磁盘文件直接当作桥的应答。
fn public_preferences(mut p: Preferences, s: Option<&aproxy_ipc::redact::Secrets>) -> Preferences {
    let Some(s) = s else { return defaults() };
    if s.replace(&p.lang) != p.lang {
        p.lang = "system".into()
    }
    if s.replace(&p.mascot) != p.mascot {
        p.mascot = "tumbler".into()
    }
    fn contains_secret(v: &serde_json::Value, s: &aproxy_ipc::redact::Secrets) -> bool {
        match v {
            serde_json::Value::String(t) => s.find(t.as_bytes()).is_some(),
            serde_json::Value::Array(a) => a.iter().any(|x| contains_secret(x, s)),
            serde_json::Value::Object(o) => o
                .iter()
                .any(|(k, v)| s.find(k.as_bytes()).is_some() || contains_secret(v, s)),
            _ => false,
        }
    }
    if let Some(theme) = &p.theme
        && theme
            .strip_prefix("aproxy-theme-v1:")
            .and_then(|t| serde_json::from_str(t).ok())
            .is_some_and(|v| contains_secret(&v, s))
    {
        p.theme = None
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
impl StateOps for DesktopHome {
    fn read_desktop_state(&self) -> Task<'_, DesktopState> {
        Box::pin(async move {
            let store = Store::open(&self.home, StateFile::Preferences)?;
            let (preferences, p) = store.preferences()?;
            let (bookmarks, b) = store.bookmarks()?;
            let reset = [(p, StateFile::Preferences), (b, StateFile::Bookmarks)]
                .into_iter()
                .filter_map(|(v, k)| v.then_some(k))
                .collect();
            let mask = self.display_secrets();
            Ok(DesktopState {
                preferences: public_preferences(preferences, mask.as_ref()),
                bookmarks: public_pins(bookmarks, mask.as_ref()),
                reset,
            })
        })
    }
    fn set_preferences(&self, patch: PreferencesPatch) -> Task<'_, Preferences> {
        Box::pin(async move {
            let f = StateFile::Preferences;
            let store = Store::open(&self.home, f)?;
            let (mut p, reset) = store.preferences()?;
            if reset {
                return Err(Fault::DesktopStateReset { file: f });
            }
            if let Some(v) = patch.lang {
                p.lang = v
            }
            if let Some(v) = patch.mode {
                p.mode = v
            }
            if let Some(v) = patch.palette {
                p.palette = v
            }
            if let Some(v) = patch.mica {
                p.mica = v
            }
            if let Some(v) = patch.mascot {
                p.mascot = v
            }
            if let Some(v) = patch.theme {
                p.theme = v
            }
            prefs(&p)?;
            store.save(f, &p)?;
            Ok(public_preferences(p, self.display_secrets().as_ref()))
        })
    }
    fn set_pinned(&self, change: PinChange) -> Task<'_, Vec<Bookmark>> {
        Box::pin(async move {
            let f = StateFile::Bookmarks;
            let store = Store::open(&self.home, f)?;
            let (mut list, reset) = store.bookmarks()?;
            if reset {
                return Err(Fault::DesktopStateReset { file: f });
            }
            match change {
                PinChange::Unpin(r) => list.retain(|b| b.r#ref != r),
                PinChange::Pin(b) => {
                    if !valid_bookmarks(std::slice::from_ref(&b)) {
                        return Err(Fault::DesktopBadInput {});
                    }
                    if list.iter().any(|v| v.r#ref == b.r#ref) {
                        return Ok(public_pins(list, self.display_secrets().as_ref()));
                    }
                    if list.len() >= COUNT {
                        return Err(limit(LimitResource::BookmarkCount, COUNT));
                    }
                    let safe = public_pins(vec![b], self.display_secrets().as_ref())
                        .pop()
                        .ok_or(Fault::DesktopUnavailable {})?;
                    list.insert(0, safe);
                }
            }
            store.save(f, &list)?;
            Ok(public_pins(list, self.display_secrets().as_ref()))
        })
    }
    fn about(&self) -> R<About> {
        Ok(About {
            version: env!("CARGO_PKG_VERSION").into(),
            variant: if cfg!(feature = "desktop") {
                Variant::Desktop
            } else {
                Variant::Headless
            },
            home: self.display_home.clone(),
        })
    }
}
