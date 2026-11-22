//! 扩展包按一整份有界快照核验。发现只产生公开投影，不因文件出现在目录里就运行代码。
mod authority;
pub(super) use authority::Runtime;
mod format;
mod tree;
use super::DesktopHome;
use aproxy_ipc::{
    desktop::{
        ExtField as Field, ExtKey, ExtKind, Fault, InvalidPackage, InvalidReason as Why,
        PackageInfo, Principal,
    },
    redact::Secrets,
};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs::File, io::Read, path::Path};
use tree::{Dir, Stamp};

type Problem = (Why, Field);
fn fail(why: Why, field: Field) -> Problem {
    (why, field)
}
const COUNT: usize = 24;
const TOTAL: usize = 512 * 1024;
const MANIFEST: u64 = 16 * 1024;
fn invalid(id: Option<String>, (reason, field): Problem) -> InvalidPackage {
    InvalidPackage {
        id,
        fault: Fault::DesktopExtensionPackage { reason, field },
    }
}
type OpenFiles = BTreeMap<String, (File, Stamp)>;
fn layout(dir: &Dir) -> Result<(bool, OpenFiles), Problem> {
    let mut files = BTreeMap::new();
    let mut assets = false;
    let names = dir
        .names(11, Why::FileCount)
        .map_err(|r| fail(r, Field::Directory))?;
    for os in names {
        let name = os
            .to_str()
            .filter(|n| format::safe_file(n))
            .ok_or(fail(Why::Path, Field::Directory))?;
        if name == "assets" {
            assets = true;
            let a = dir
                .child(name)
                .map_err(|r| fail(r, Field::Assets))?
                .ok_or(fail(Why::Io, Field::Assets))?;
            for os in a
                .names(8, Why::FileCount)
                .map_err(|r| fail(r, Field::Assets))?
            {
                let f = os
                    .to_str()
                    .filter(|s| format::safe_file(s))
                    .ok_or(fail(Why::Path, Field::Assets))?;
                let file = a.file(f).map_err(|r| fail(r, Field::Assets))?;
                if !f.strip_suffix(".bin").is_some_and(|v| format::key(v, 32)) {
                    return Err(fail(Why::Layout, Field::Assets));
                }
                files.insert(format!("assets/{f}"), file);
            }
        } else {
            let file = dir.file(name).map_err(|r| fail(r, Field::Directory))?;
            if !matches!(name, "manifest.json" | "main.mjs" | "theme.txt") {
                return Err(fail(Why::Layout, Field::Directory));
            }
            files.insert(name.into(), file);
        }
        if files.len() > 10 {
            return Err(fail(Why::FileCount, Field::Directory));
        }
    }
    Ok((assets, files))
}
enum Payload {
    Theme(String),
    Script(Vec<u8>, BTreeMap<String, Vec<u8>>),
}
#[derive(Default)]
struct Snapshot {
    packages: Vec<PackageInfo>,
    invalid: Vec<InvalidPackage>,
    payload: Option<Payload>,
}
fn package(
    root: &Dir,
    id: &str,
    secrets: Option<&Secrets>,
    capture: bool,
) -> Result<(PackageInfo, Option<Payload>), Problem> {
    let dir = root
        .child(id)
        .map_err(|r| fail(r, Field::Directory))?
        .ok_or(fail(Why::Io, Field::Directory))?;
    let (assets, files) = layout(&dir)?;
    let mut bytes = BTreeMap::new();
    let mut total = 0;
    let mut digest = Sha256::new().chain_update(b"aproxy-extension-v1\0");
    let secrets = secrets.ok_or(fail(Why::PrivateData, Field::Directory))?;
    for (name, (file, stamp)) in &files {
        let (limit, field) = match name.as_str() {
            "manifest.json" => (MANIFEST, Field::Manifest),
            "theme.txt" => (MANIFEST, Field::Theme),
            "main.mjs" => (256 * 1024, Field::Entry),
            _ => (64 * 1024, Field::Assets),
        };
        if stamp.bytes > limit {
            return Err(fail(Why::FileBytes, field));
        }
        total += stamp.bytes as usize;
        if total > TOTAL {
            return Err(fail(Why::PackageBytes, Field::Directory));
        }
        let mut data = Vec::new();
        file.take(limit + 1)
            .read_to_end(&mut data)
            .map_err(|_| fail(Why::Io, field))?;
        if data.len() as u64 != stamp.bytes || tree::observed(file).as_ref() != Ok(stamp) {
            return Err(fail(Why::Io, field));
        }
        if secrets.find(&data).is_some() || secrets.find(name.as_bytes()).is_some() {
            return Err(fail(Why::PrivateData, field));
        }
        digest.update((name.len() as u32).to_be_bytes());
        digest.update(name.as_bytes());
        digest.update(stamp.bytes.to_be_bytes());
        digest.update(&data);
        bytes.insert(name.as_str(), data);
    }
    // 从父目录重新定位后核对句柄身份与大小、修改时刻，避免把并发换包前后的内容拼在一起。
    let again = root
        .child(id)
        .map_err(|r| fail(r, Field::Directory))?
        .ok_or(fail(Why::Io, Field::Directory))?;
    let (assets2, after) = layout(&again)?;
    if !root
        .still_child(id, &dir)
        .map_err(|r| fail(r, Field::Directory))?
        || assets2 != assets
        || !files
            .iter()
            .map(|(p, (_, s))| (p, s))
            .eq(after.iter().map(|(p, (_, s))| (p, s)))
    {
        return Err(fail(Why::Io, Field::Directory));
    }
    let m = format::manifest(
        bytes
            .get("manifest.json")
            .ok_or(fail(Why::Layout, Field::Manifest))?,
        id,
        secrets,
    )?;
    let payload = if m.kind == ExtKind::Theme {
        if assets || bytes.contains_key("main.mjs") {
            return Err(fail(Why::Layout, Field::Theme));
        }
        let theme = format::theme(
            bytes
                .get("theme.txt")
                .ok_or(fail(Why::Layout, Field::Entry))?,
            secrets,
        )?;
        capture.then_some(Payload::Theme(theme))
    } else {
        if bytes.contains_key("theme.txt") {
            return Err(fail(Why::Layout, Field::Directory));
        }
        format::text(
            bytes
                .get("main.mjs")
                .ok_or(fail(Why::Layout, Field::Entry))?,
            Field::Entry,
        )?;
        capture.then(|| {
            Payload::Script(
                bytes.remove("main.mjs").unwrap(),
                bytes
                    .into_iter()
                    .filter_map(|(p, b)| {
                        Some((
                            p.strip_prefix("assets/")?.strip_suffix(".bin")?.to_owned(),
                            b,
                        ))
                    })
                    .collect(),
            )
        })
    };
    use std::fmt::Write;
    let mut hex = String::with_capacity(64);
    for b in digest.finalize() {
        write!(hex, "{b:02x}").expect("writing to a String")
    }
    let digest = hex;
    if secrets.find(digest.as_bytes()).is_some() {
        return Err(fail(Why::PrivateData, Field::Id));
    }
    Ok((
        PackageInfo {
            principal: Principal {
                key: ExtKey {
                    id: id.to_owned(),
                    digest,
                },
                name: m.name,
                version: m.version,
                kind: m.kind,
            },
            permissions: m.permissions,
            enabled: false,
            grants: Vec::new(),
        },
        payload,
    ))
}
fn discover(
    home: &Path,
    secrets: Option<&Secrets>,
    wanted: Option<&ExtKey>,
) -> Result<Snapshot, Why> {
    let home = Dir::home(home)?;
    let Some(desktop) = home.child("desktop")? else {
        return Ok(Default::default());
    };
    let Some(root) = desktop.child("extensions")? else {
        return Ok(Default::default());
    };
    let names = root.names(COUNT, Why::PackageCount)?;
    let mut payload = None;
    let mut good = Vec::new();
    let mut bad = Vec::new();
    for os in &names {
        let Some(id) = os
            .to_str()
            .filter(|n| format::key(n, 48) && !n.ends_with('-'))
        else {
            bad.push(invalid(None, fail(Why::Path, Field::Directory)));
            continue;
        };
        let show = secrets
            .filter(|s| s.find(id.as_bytes()).is_none())
            .map(|_| id.to_owned());
        if names
            .iter()
            .any(|other| other != os && other.to_str().is_some_and(|s| s.eq_ignore_ascii_case(id)))
        {
            bad.push(invalid(show, fail(Why::Path, Field::Directory)));
            continue;
        }
        match package(&root, id, secrets, wanted.is_some_and(|k| k.id == id)) {
            Ok((value, body)) => {
                if wanted == Some(&value.principal.key) {
                    payload = body;
                }
                good.push(value)
            }
            Err(err) => bad.push(invalid(show, err)),
        }
    }
    if names != root.names(COUNT, Why::PackageCount)?
        || !desktop.still_child("extensions", &root)?
        || !home.still_child("desktop", &desktop)?
    {
        return Err(Why::Io);
    }
    Ok(Snapshot {
        packages: good,
        invalid: bad,
        payload,
    })
}
impl DesktopHome {
    /// 在当前 home 的私有脱敏上下文中核验包；返回清单而非可执行载荷，损坏的包不影响邻居。
    pub fn extension_catalog(&self) -> (Vec<PackageInfo>, Vec<InvalidPackage>) {
        let tree = self.extension_snapshot(None);
        (tree.packages, tree.invalid)
    }
    fn extension_snapshot(&self, wanted: Option<&ExtKey>) -> Snapshot {
        discover(&self.home, self.display_secrets().as_ref(), wanted).unwrap_or_else(|r| Snapshot {
            invalid: vec![invalid(None, fail(r, Field::Root))],
            ..Default::default()
        })
    }
}
