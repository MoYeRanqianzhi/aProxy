//! 在一把写锁内备份、替换、按启动的分层规则复读。预演只读，不创建 home、锁或备份。
use super::{Issue, Kind, Plan, ResultFile, plan, text};
use crate::{
    config::{self, resolve::Defaults},
    find, fsx, settings,
};
use aproxy_ipc::i18n::Lang;
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

impl ResultFile {
    fn empty(path: &Path) -> Self {
        Self {
            path: path.into(),
            outcome: "current",
            changes: vec![],
            issues: vec![],
        }
    }
    fn problem(&mut self, kind: Kind, io: bool, why: String) {
        self.outcome = if io { "failed" } else { "needs-review" };
        self.changes.clear();
        self.issues.push(Issue {
            key: None,
            line: None,
            section: kind.section(),
            message: why,
        });
    }
}
fn backup(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".0.1.bak");
    name.into()
}

// 不用普通 copy：目标可能在共享目录里。第一次创建就用私有权限且排他创建，已有备份
// 只比对，不截断；中途写失败留下的备份也会让下次迁移停下来，不会被当作可靠的原文。
fn preserve(path: &Path, src: &str, dry: bool) -> io::Result<bool> {
    let path = backup(path);
    if !dry {
        match fsx::create_new_private(&path, true) {
            Ok(mut f) => {
                f.write_all(src.as_bytes())?;
                f.sync_all()?;
                return Ok(true);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => (),
            Err(e) => return Err(e),
        }
    }
    match fs::symlink_metadata(&path) {
        Ok(m) if m.is_file() => Ok(fs::read(path)? == src.as_bytes()),
        Ok(_) => Ok(false),
        Err(e) if dry && e.kind() == io::ErrorKind::NotFound => Ok(true),
        Err(e) => Err(e),
    }
}

/// 检查指定 home；每个目标单独成败，不联系正在运行的进程，也不读取期望状态记录。
pub fn run(home: &Path, dry: bool, lang: Lang) -> Vec<ResultFile> {
    let sp = settings::settings_path_in(home);
    let _lock = if dry {
        None
    } else {
        match config::edit::lock(home) {
            Ok(l) => Some(l),
            Err(e) => {
                let mut f = ResultFile::empty(&sp);
                f.problem(
                    Kind::Settings,
                    true,
                    settings::describe_edit_error(&e, &sp, lang),
                );
                return vec![f];
            }
        }
    };
    let mut reports = vec![];
    let mut written = vec![];
    let mut seen = HashSet::new();
    let mut inventory = settings::Settings::default();
    let mut defaults = Defaults::default();
    let mut settings_valid = true;
    let run_dir = fs::canonicalize(home.join("run")).unwrap_or_else(|_| home.join("run"));
    // 发现路径时只用 settings 中可读的别名和目录；旧键出错不应让目录变成空集。
    let mut targets = BTreeMap::new();
    for kind in [Kind::Settings, Kind::Config] {
        if kind == Kind::Settings {
            targets.insert(sp.clone(), Ok(()));
        } else {
            settings_valid = reports
                .iter()
                .all(|r: &ResultFile| matches!(r.outcome, "current" | "migrated"));
            for raw in inventory
                .aliases
                .values()
                .chain(inventory.default_config.iter())
            {
                targets.insert(settings::expand_path(raw), Ok(()));
            }
            find::scan(&inventory, home, |p, r| {
                targets.insert(p, r);
            });
        }
        for (path, found) in std::mem::take(&mut targets) {
            let resolved = found.and_then(|_| fs::canonicalize(&path));
            let path = match resolved {
                Ok(path) => path,
                Err(e)
                    if kind == Kind::Settings
                        && e.kind() == io::ErrorKind::NotFound
                        && matches!(fs::symlink_metadata(&path), Err(e) if e.kind()==io::ErrorKind::NotFound) =>
                {
                    continue;
                }
                Err(e) => {
                    let mut report = ResultFile::empty(&path);
                    report.problem(kind, true, text::io(lang, e.kind()));
                    reports.push(report);
                    continue;
                }
            };
            if !seen.insert(path.clone()) {
                continue;
            }
            let mut report = ResultFile::empty(&path);
            let result = (|| -> io::Result<()> {
                if path.starts_with(&run_dir)
                    || path
                        .file_name()
                        .is_some_and(|n| n.to_string_lossy().ends_with(".0.1.bak"))
                {
                    report.problem(kind, false, text::reserved(lang).into());
                    return Ok(());
                }
                if !fs::metadata(&path)?.is_file() {
                    return Err(io::ErrorKind::InvalidInput.into());
                }
                let src = fs::read_to_string(&path)?;
                let Plan {
                    next,
                    changes,
                    issues,
                } = plan(kind, &src, &path, lang);
                if kind == Kind::Settings {
                    inventory = settings::file::parse(&src, &path).0;
                }
                report.changes = changes;
                report.issues = issues;
                if !report.issues.is_empty() {
                    report.outcome = "needs-review";
                }
                if let Some(next) = next {
                    if kind == Kind::Config {
                        if !settings_valid {
                            report.problem(kind, false, text::invalid_settings(lang).into());
                            return Ok(());
                        }
                        let resolved = config::resolve::resolve_text(
                            Some(&next),
                            &path,
                            &config::Overrides::default(),
                            &defaults,
                            config::resolve::env_proxy_is_set(),
                        );
                        for d in resolved.errors() {
                            let key = d
                                .key
                                .as_deref()
                                .map(|k| super::source_key(&report.changes, k));
                            report.issues.push(Issue {
                                key: key.map(str::to_owned),
                                line: key.and_then(|k| config::file::locate(&src, k)).map(|x| x.0),
                                message: d.message.render(lang),
                                section: kind.section(),
                            });
                        }
                        if !report.issues.is_empty() {
                            report.outcome = "needs-review";
                            report.changes.clear();
                            return Ok(());
                        }
                    }
                    if !preserve(&path, &src, dry)? {
                        report.problem(kind, false, text::different_backup(lang).into());
                        return Ok(());
                    }
                    if !dry {
                        if fs::read(&path)? != src.as_bytes() {
                            report.problem(kind, false, text::changed(lang).into());
                            return Ok(());
                        }
                        fsx::write_atomic_private(&path, next.as_bytes())?;
                        written.push((reports.len(), kind, next.clone(), src));
                    }
                    report.outcome = "migrated";
                    if kind == Kind::Settings {
                        defaults =
                            Defaults::from_settings(&settings::file::parse(&next, &path).0, &path);
                    }
                } else if kind == Kind::Settings && report.outcome == "current" {
                    defaults = Defaults::from_settings(&inventory, &path);
                }
                Ok(())
            })();
            if let Err(e) = result {
                report.problem(kind, true, text::io(lang, e.kind()));
            }
            reports.push(report);
        }
    }
    // 全部替换以后才从磁盘复读，不能把内存里的合法候选当成写后校验。外部写者不拿锁，
    // 因此恢复前也要认出自己的结果，不能用更早的备份覆盖它刚写下的内容。
    for (index, kind, next, before) in written {
        let r = &mut reports[index];
        let path = r.path.clone();
        let result = (|| -> io::Result<()> {
            if fs::read(&path)? != next.as_bytes() {
                r.problem(kind, false, text::changed(lang).into());
                return Ok(());
            }
            let valid = match kind {
                Kind::Settings => Ok(settings::read(&path).is_valid()),
                Kind::Config => {
                    let settings = settings::read(&sp);
                    config::resolve(
                        &path,
                        &config::Overrides::default(),
                        &Defaults::from_settings(&settings.settings, &sp),
                    )
                    .map(|r| settings.is_valid() && r.is_valid())
                }
            };
            if !matches!(valid, Ok(true)) {
                let original = fs::read(backup(&path))?;
                if original != before.as_bytes() || fs::read(&path)? != next.as_bytes() {
                    r.problem(kind, false, text::changed(lang).into());
                } else {
                    fsx::write_atomic_private(&path, &original)?;
                    r.problem(kind, valid.is_err(), text::restored(lang).into());
                    valid?;
                }
            }
            Ok(())
        })();
        if let Err(e) = result {
            r.problem(kind, true, text::io(lang, e.kind()));
        }
    }
    reports
}
