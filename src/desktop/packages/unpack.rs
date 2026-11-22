//! 目录和 ZIP 共用一张路径表与同一个写入口；按实际字节数计费，不依赖归档里的大小承诺。
use super::*;
use serde_json::{Map, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Cursor, Write},
};
fn junk(path: &str, dir: bool) -> bool {
    path.split('/').any(|p| p == "__MACOSX")
        || (!dir
            && matches!(
                path.rsplit('/').next(),
                Some(".DS_Store" | "Thumbs.db" | "desktop.ini")
            ))
}
/// 每个父目录也参与大小写碰撞检查，防止只有文件名不同时漏过共用的目录别名。
pub(super) struct Tree<'a> {
    stage: &'a Path,
    paths: BTreeMap<String, (String, bool)>,
    files: BTreeMap<String, u64>,
    total: u64,
}
type Contents = (
    PackageManifest,
    Option<Map<String, Value>>,
    Vec<PackageFile>,
    u64,
);
impl<'a> Tree<'a> {
    /// 为独占暂存目录建立清单；所有输入共用相同的计数和路径检查。
    pub(super) fn new(stage: &'a Path) -> Self {
        Self {
            stage,
            paths: BTreeMap::new(),
            files: BTreeMap::new(),
            total: 0,
        }
    }
    fn path(&mut self, s: &str, dir: bool) -> R<()> {
        if !valid_path(s) {
            return Err(bad("path", Some(s)));
        }
        let mut prefix = String::new();
        for c in s.split('/') {
            if !prefix.is_empty() {
                prefix.push('/')
            }
            prefix.push_str(c);
            let is_dir = prefix != s || dir;
            let k = prefix.to_lowercase();
            if let Some((old, d)) = self.paths.get(&k) {
                if old != &prefix || !is_dir || !d {
                    return Err(bad("collision", Some(s)));
                }
            } else {
                self.paths.insert(k, (prefix.clone(), is_dir));
            }
        }
        Ok(())
    }
    fn put(&mut self, path: &str, declared: u64, reader: impl Read) -> R<()> {
        self.path(path, false)?;
        if self.files.len() as u64 >= COUNT {
            return Err(limit("files", COUNT, Some(path)));
        }
        let (cap, resource) = match path {
            "manifest.json" | "theme.json" => (JSON, "json-bytes"),
            "theme.css" => (1 << 20, "css-bytes"),
            _ => (FILE, "file-bytes"),
        };
        if declared > cap {
            return Err(limit(resource, cap, Some(path)));
        }
        if declared > TOTAL - self.total {
            return Err(limit("total-bytes", TOTAL, Some(path)));
        }
        let mut b = Vec::new();
        reader
            .take(declared + 1)
            .read_to_end(&mut b)
            .map_err(|_| bad("content", Some(path)))?;
        if b.len() as u64 != declared {
            return Err(bad("size-mismatch", Some(path)));
        }
        if matches!(path, "main.mjs" | "theme.css")
            && (std::str::from_utf8(&b).is_err() || (path == "main.mjs" && b.contains(&0)))
        {
            return Err(bad("text", Some(path)));
        }
        let target = self.stage.join(path);
        fs::create_dir_all(target.parent().unwrap())
            .and_then(|()| fs::File::create_new(target)?.write_all(&b))
            .map_err(|_| bad("io", Some(path)))?;
        self.files.insert(path.into(), declared);
        self.total += declared;
        Ok(())
    }
    fn mkdir(&mut self, path: &str) -> R<()> {
        self.path(path, true)?;
        fs::create_dir_all(self.stage.join(path)).map_err(|_| bad("io", Some(path)))
    }
    /// 逐项核验目录源，读取时以已打开的父目录定位，拒绝链接。
    pub(super) fn directory(&mut self, d: &Dir, prefix: &str) -> R<()> {
        for name in d.names().map_err(|e| file_error(e, Some(prefix)))? {
            let name = name.to_str().ok_or_else(|| bad("path", None))?;
            if name == "__MACOSX" {
                continue;
            }
            let p = if prefix.is_empty() {
                name.into()
            } else {
                format!("{prefix}/{name}")
            };
            if !valid_path(&p) {
                return Err(bad("path", Some(&p)));
            }
            let n = d
                .node(name)
                .map_err(|e| file_error(e, Some(&p)))?
                .ok_or_else(|| bad("missing", Some(&p)))?;
            match n {
                Node::Dir(child) => {
                    self.mkdir(&p)?;
                    self.directory(&child, &p)?
                }
                Node::File(f) if !junk(&p, false) => {
                    let n = f.metadata().map_err(|_| bad("io", Some(&p)))?.len();
                    self.put(&p, n, f)?
                }
                _ => (),
            }
        }
        Ok(())
    }
    /// 先核对中央目录的路径、编码及大小，再逐项解压到暂存区。
    pub(super) fn zip(&mut self, bytes: &[u8]) -> R<()> {
        let mut z = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| bad("archive", None))?;
        if z.has_duplicate_names() {
            return Err(bad("collision", None));
        }
        z.validate_header_consistency()
            .map_err(|_| bad("archive", None))?;
        let mut entries = Vec::new();
        let mut roots = BTreeSet::new();
        let mut flat = false;
        let (mut count, mut total) = (0, 0);
        for i in 0..z.len() {
            let f = z.by_index_raw(i).map_err(|_| bad("archive", None))?;
            let n = std::str::from_utf8(f.name_raw()).map_err(|_| bad("path", None))?;
            let dir = n.ends_with('/');
            let p = n.strip_suffix('/').unwrap_or(n);
            if junk(p, dir) {
                continue;
            }
            if !valid_path(p) {
                return Err(bad("path", Some(p)));
            }
            if f.encrypted() {
                return Err(bad("encrypted", Some(p)));
            }
            if !matches!(
                f.compression(),
                zip::CompressionMethod::Stored | zip::CompressionMethod::Deflated
            ) {
                return Err(bad("compression", Some(p)));
            }
            if f.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000) {
                return Err(bad("link", Some(p)));
            }
            if dir && f.size() != 0 {
                return Err(bad("file-type", Some(p)));
            }
            if !dir {
                count += 1;
                if count > COUNT {
                    return Err(limit("files", COUNT, Some(p)));
                }
                if f.size() > FILE {
                    return Err(limit("file-bytes", FILE, Some(p)));
                }
                total += f.size();
                if total > TOTAL {
                    return Err(limit("total-bytes", TOTAL, Some(p)));
                }
            }
            roots.insert(p.split('/').next().unwrap().to_owned());
            flat |= p == "manifest.json" && !dir;
            entries.push((i, p.to_owned(), dir, f.size()));
        }
        let wrapper = (!flat && roots.len() == 1).then(|| format!("{}/", roots.first().unwrap()));
        for (i, raw, dir, size) in entries {
            let p = match &wrapper {
                Some(w) if dir && raw == w.trim_end_matches('/') => continue,
                Some(w) => raw
                    .strip_prefix(w)
                    .ok_or_else(|| bad("missing", Some("manifest.json")))?,
                None => &raw,
            };
            if dir {
                self.mkdir(p)?
            } else {
                let f = z.by_index(i).map_err(|_| bad("archive", Some(p)))?;
                self.put(p, size, f)?
            }
        }
        Ok(())
    }
    /// 从暂存副本核验清单与入口文件，保证安装的正是页面检查的内容。
    pub(super) fn finish(self) -> R<Contents> {
        let dir = Dir::open(self.stage).map_err(|e| file_error(e, None))?;
        let m = manifest::parse(&read_bound(
            file(&dir, "manifest.json")?,
            JSON,
            "json-bytes",
            Some("manifest.json"),
        )?)?;
        for (p, dir) in self.paths.values() {
            if p == "manifest.json" && !dir {
                continue;
            }
            if p == "assets" && *dir {
                continue;
            }
            if p.starts_with("assets/")
                && (*dir
                    || m.kind == PackageKind::Extension
                    || p.rsplit_once('.').is_some_and(|(_, e)| {
                        matches!(
                            e.to_ascii_lowercase().as_str(),
                            "png"
                                | "jpg"
                                | "jpeg"
                                | "webp"
                                | "gif"
                                | "avif"
                                | "svg"
                                | "woff2"
                                | "woff"
                                | "ttf"
                                | "otf"
                        )
                    }))
            {
                continue;
            }
            if !dir
                && !p.contains('/')
                && (["README", "LICENSE", "LICENCE", "COPYING"]
                    .iter()
                    .any(|n| p.to_ascii_uppercase().starts_with(n))
                    || match m.kind {
                        PackageKind::Theme => matches!(p.as_str(), "theme.json" | "theme.css"),
                        PackageKind::Extension => p == "main.mjs",
                    })
            {
                continue;
            }
            return Err(bad("unexpected-file", Some(p)));
        }
        let theme = match m.kind {
            PackageKind::Theme => Some(
                manifest::object(
                    &read_bound(
                        file(&dir, "theme.json")?,
                        JSON,
                        "json-bytes",
                        Some("theme.json"),
                    )?,
                    128,
                )
                .ok_or_else(|| bad("json", Some("theme.json")))?,
            ),
            PackageKind::Extension => {
                file(&dir, "main.mjs")?;
                None
            }
        };
        Ok((
            m,
            theme,
            self.files
                .into_iter()
                .map(|(path, bytes)| PackageFile { path, bytes })
                .collect(),
            self.total,
        ))
    }
}
