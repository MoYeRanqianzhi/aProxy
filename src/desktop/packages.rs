//! 安装页展示的是暂存副本的事实；确认之后只交换目录，绝不回到用户选择的源路径再读一次。
pub mod github;
mod lifecycle;
mod manifest;
use tokio::time::Instant;
pub mod scheme;
mod tree;
mod unpack;
use super::{
    DesktopHome,
    preferences::{self, DesktopToml},
    state::{Store, safe},
};
use aproxy_ipc::desktop::*;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};
use tree::{Dir, Node, Why};
const FILE: u64 = 64 << 20;
const JSON: u64 = 64 << 10;
const TOTAL: u64 = 256 << 20;
const COUNT: u64 = 2000;
const TTL: Duration = Duration::from_secs(30 * 60);
fn issue(reason: &str, path: Option<&str>) -> PackageIssue {
    PackageIssue {
        reason: reason.into(),
        path: path.map(|s| manifest::clean(s).chars().take(200).collect()),
        field: None,
        limit: None,
        max: None,
    }
}
fn bad(reason: &str, path: Option<&str>) -> Fault {
    Fault::DesktopPackage(issue(reason, path))
}
fn field(k: &str) -> Fault {
    Fault::DesktopPackage(PackageIssue {
        field: Some(k.into()),
        ..issue("field", Some("manifest.json"))
    })
}
fn limit(k: &str, max: u64, path: Option<&str>) -> Fault {
    Fault::DesktopPackage(PackageIssue {
        limit: Some(k.into()),
        max: Some(max),
        ..issue("limit", path)
    })
}
fn file_error(e: Why, path: Option<&str>) -> Fault {
    bad(
        match e {
            Why::Io => "io",
            Why::Link => "link",
            Why::FileType => "file-type",
        },
        path,
    )
}
/// 只认可以在三个桌面平台上表示的相对文件名；不依赖当前机器的路径折叠规则。
fn valid_path(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 200
        && s.split('/').count() <= 8
        && s.split('/').all(|c| {
            !c.is_empty()
                && c.len() <= 100
                && !c.ends_with([' ', '.'])
                && !preferences::device(c)
                && !c
                    .chars()
                    .any(|x| x.is_control() || "\\:*?\"<>|".contains(x))
        })
}
fn root(store: &Store) -> R<PathBuf> {
    let p = store.dir.join("extensions");
    safe(&p, StateFile::Packages, true)?;
    Ok(p)
}
fn read_bound(r: impl Read, max: u64, resource: &str, path: Option<&str>) -> R<Vec<u8>> {
    let mut b = Vec::new();
    r.take(max + 1)
        .read_to_end(&mut b)
        .map_err(|_| bad("io", path))?;
    if b.len() as u64 > max {
        return Err(limit(resource, max, path));
    }
    Ok(b)
}
fn file(dir: &Dir, name: &str) -> R<fs::File> {
    match dir.node(name).map_err(|e| file_error(e, Some(name)))? {
        Some(Node::File(f)) => Ok(f),
        Some(_) => Err(bad("file-type", Some(name))),
        None => Err(bad("missing", Some(name))),
    }
}
fn installed_manifest(dir: &Dir, id: &str) -> R<PackageManifest> {
    let m = manifest::parse(&read_bound(
        file(dir, "manifest.json")?,
        JSON,
        "json-bytes",
        Some("manifest.json"),
    )?)?;
    if m.id != id {
        return Err(field("id"));
    }
    Ok(m)
}
fn source_name(name: &str) -> R<String> {
    if name.is_empty() || name.contains(['/', '\\', '\0']) {
        return Err(Fault::DesktopBadInput {});
    }
    Ok(manifest::clean(name).chars().take(200).collect())
}

/// 暂存目录由句柄持有；失败、取消与窗口退出都会释放未交付的副本。
pub(super) struct Pending {
    stage: tempfile::TempDir,
    check: PackageCheck,
    expires: Instant,
    github: Option<GithubFacts>,
}
impl Pending {
    fn expired(&self, now: Instant) -> bool {
        now >= self.expires
    }
}
/// 过期和已消费的句柄在页面看来一样；从不按外部提供的 token 拼接本机路径。
fn current<'a>(pending: &'a mut Option<Pending>, token: &str, now: Instant) -> R<&'a mut Pending> {
    if pending.as_ref().is_some_and(|p| p.expired(now)) {
        pending.take();
    }
    pending
        .as_mut()
        .filter(|p| p.check.token == token)
        .ok_or_else(|| bad("token", None))
}
enum Source {
    Bytes(String, Vec<u8>),
    Path(PathBuf),
}
impl DesktopHome {
    fn check_package(&self, source: Source) -> Task<'_, PackageCheck> {
        Box::pin(self.state_work(StateFile::Packages, move |host, store| {
            let mut pending = host.package.lock().unwrap();
            pending.take();
            let root = root(&store)?;
            fs::create_dir_all(&root).map_err(|_| bad("io", None))?;
            let stage = tempfile::Builder::new()
                .prefix(".staging-")
                .rand_bytes(32)
                .tempdir_in(&root)
                .map_err(|_| bad("io", None))?;
            let token = stage
                .path()
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .trim_start_matches(".staging-")
                .to_owned();
            let mut tree = unpack::Tree::new(stage.path());
            let mut unzip = |name: String, bytes: Vec<u8>| -> R<PackageSource> {
                if bytes.len() as u64 > FILE {
                    return Err(limit("zip-bytes", FILE, None));
                }
                let source = PackageSource::File {
                    name,
                    sha256: Sha256::digest(&bytes)
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect(),
                    bytes: bytes.len() as u64,
                };
                tree.zip(&bytes)?;
                Ok(source)
            };
            let source = match source {
                Source::Bytes(name, data) => unzip(source_name(&name)?, data)?,
                Source::Path(p) => {
                    if !p.is_absolute() {
                        return Err(Fault::DesktopBadInput {});
                    }
                    let name = p
                        .file_name()
                        .and_then(|s| s.to_str())
                        .ok_or(Fault::DesktopBadInput {})?;
                    // 拖入路径的父目录在枚举期间保持打开；末级的链接也按链接本身检查。
                    let parent = Dir::open(p.parent().ok_or(Fault::DesktopBadInput {})?)
                        .map_err(|e| file_error(e, None))?;
                    match parent
                        .node(name)
                        .map_err(|e| file_error(e, None))?
                        .ok_or_else(|| bad("missing", None))?
                    {
                        Node::File(f) => {
                            unzip(source_name(name)?, read_bound(f, FILE, "zip-bytes", None)?)?
                        }
                        Node::Dir(d) => {
                            tree.directory(&d, "")?;
                            PackageSource::Directory {
                                name: source_name(name)?,
                            }
                        }
                    }
                }
            };
            let (manifest, theme, files, total_bytes) = tree.finish()?;
            let base = Dir::open(&root).map_err(|e| file_error(e, None))?;
            let installed = match base.node(&manifest.id).map_err(|e| file_error(e, None))? {
                None => None,
                Some(Node::Dir(d)) => {
                    let old = installed_manifest(&d, &manifest.id)?;
                    if old.kind != manifest.kind {
                        return Err(bad("kind-change", None));
                    }
                    let (_, enabled, _) = DesktopToml::read(&store)
                        .map_err(Fault::DesktopToml)?
                        .preferences();
                    Some(Installed {
                        version: old.version,
                        enabled: manifest.kind == PackageKind::Extension
                            && enabled.contains(&manifest.id),
                    })
                }
                Some(_) => return Err(bad("file-type", None)),
            };
            let check = PackageCheck {
                token,
                manifest,
                theme,
                files,
                total_bytes,
                source,
                installed,
            };
            *pending = Some(Pending {
                stage,
                check: check.clone(),
                expires: Instant::now() + TTL,
                github: None,
            });
            Ok(check)
        }))
    }
}
impl PackageOps for DesktopHome {
    fn package_list(&self) -> Task<'_, Vec<PackageEntry>> {
        Box::pin(self.state_work(StateFile::Packages, move |host, s| {
            lifecycle::list(&host, &s)
        }))
    }
    fn package_install(&self, token: String, enable: bool) -> Task<'_, PackageEntry> {
        Box::pin(self.state_work(StateFile::Packages, move |host, s| {
            lifecycle::install(&host, &s, &token, enable)
        }))
    }
    fn package_set_enabled(&self, id: String, enabled: bool) -> Task<'_, PackageEntry> {
        Box::pin(self.state_work(StateFile::Packages, move |_, s| {
            lifecycle::set_enabled(&s, &id, enabled)
        }))
    }
    fn package_uninstall(&self, id: String, remove_data: bool) -> Task<'_, ()> {
        Box::pin(self.state_work(StateFile::Packages, move |_, s| {
            lifecycle::uninstall(&s, &id, remove_data)
        }))
    }
    fn package_verify(&self, token: String) -> Task<'_, GithubFacts> {
        Box::pin(async move {
            let t = token.clone();
            let (manifest, source, proxy) = self
                .state_work(StateFile::Packages, move |host, _| {
                    let mut guard = host.package.lock().unwrap();
                    let c = &current(&mut guard, &t, Instant::now())?.check;
                    let proxy =
                        crate::settings::load_from(&crate::settings::settings_path_in(&host.home))
                            .download_proxy;
                    Ok((c.manifest.clone(), c.source.clone(), proxy))
                })
                .await?;
            let facts = github::facts(
                manifest.repository.as_deref(),
                &manifest.version,
                &source,
                "https://api.github.com",
                proxy.as_deref(),
                Duration::from_secs(15),
            )
            .await;
            // 网络等待不占状态锁。后来的检查、取消或安装都可以先完成，不把过期的事实写给别的包。
            self.state_work(StateFile::Packages, move |host, _| {
                let mut guard = host.package.lock().unwrap();
                current(&mut guard, &token, Instant::now())?.github = Some(facts.clone());
                Ok(facts)
            })
            .await
        })
    }
    fn package_check_file(&self, name: String, bytes: Vec<u8>) -> Task<'_, PackageCheck> {
        self.check_package(Source::Bytes(name, bytes))
    }
    fn package_check_path(&self, path: String) -> Task<'_, PackageCheck> {
        self.check_package(Source::Path(path.into()))
    }
    fn package_discard(&self, token: String) -> Task<'_, ()> {
        Box::pin(self.state_work(StateFile::Packages, move |host, _| {
            let mut p = host.package.lock().unwrap();
            if p.as_ref()
                .is_some_and(|v| v.check.token == token || v.expired(Instant::now()))
            {
                p.take()
                    .unwrap()
                    .stage
                    .close()
                    .map_err(|_| bad("io", None))?;
            }
            Ok(())
        }))
    }
}
