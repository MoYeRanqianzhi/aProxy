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
    io::{self, Read, Write},
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

// 临时文件和目标在同一目录，创建时已私有。写入或同步失败只丢弃临时文件，永远
// 不把半份原文留在权威备份名下。TempPath 在未发布时负责清理。
fn staged(path: &Path, bytes: &[u8]) -> io::Result<tempfile::TempPath> {
    let mut f = tempfile::Builder::new()
        .make_in(path.parent().unwrap(), |p| fsx::create_new_private(p, true))?;
    f.write_all(bytes)?;
    f.as_file().sync_all()?;
    Ok(f.into_temp_path())
}

// 用文件句柄比较已有的备份。Unix 不跟随符号链接，Windows 拒绝其他写者和删除者，
// 因而收紧 ACL 的路径和刚刚比较的文件对应。内容不同的备份连权限也不改。
fn reuse(path: &Path, src: &[u8], dry: bool) -> io::Result<bool> {
    if !fs::symlink_metadata(path)?.is_file() {
        return Ok(false);
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
        };
        options
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .share_mode(FILE_SHARE_READ);
    }
    let mut f = options.open(path)?;
    let metadata = f.metadata()?;
    if !metadata.is_file() {
        return Ok(false);
    }
    let mut bytes = Vec::new();
    f.read_to_end(&mut bytes)?;
    if bytes != src {
        return Ok(false);
    }
    if dry {
        return Ok(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        // 不改其他属主或多链接文件的权限，否则会影响不在迁移范围内的文件名。
        // SAFETY: geteuid 不取指针，也没有调用前置条件。
        if metadata.uid() != unsafe { libc::geteuid() } || metadata.nlink() != 1 {
            return Ok(false);
        }
        Ok(f.set_permissions(fs::Permissions::from_mode(0o600)).is_ok())
    }
    #[cfg(windows)]
    {
        Ok(fsx::secure_private_file(path).is_ok())
    }
}

fn preserve(
    path: &Path,
    src: &str,
    dry: bool,
    observe: &impl Fn(Point, &Path),
) -> io::Result<bool> {
    let path = backup(path);
    match fs::symlink_metadata(&path) {
        Ok(_) => return reuse(&path, src.as_bytes(), dry),
        Err(e) if e.kind() == io::ErrorKind::NotFound => (),
        Err(e) => return Err(e),
    }
    if dry {
        return Ok(true);
    }
    let temp = staged(&path, src.as_bytes())?;
    observe(Point::PublishBackup, &path);
    match temp.persist_noclobber(&path) {
        Ok(()) => Ok(true),
        Err(e) if e.error.kind() == io::ErrorKind::AlreadyExists => {
            reuse(&path, src.as_bytes(), false)
        }
        Err(e) => Err(e.error),
    }
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum Point {
    PublishBackup,
    Replace,
    Validate,
    Restore,
}

fn replace(
    path: &Path,
    bytes: &[u8],
    expected: &[u8],
    point: Point,
    observe: &impl Fn(Point, &Path),
) -> io::Result<bool> {
    let mut temp = staged(path, bytes)?;
    observe(point, path);
    // 不拿 home 锁的编辑器还可能在最后一次比较与 rename 之间写入；准备和 sync 临时
    // 文件都在比较之前完成。恢复走同一条路径，不能用备份覆盖已经观察到的外部修改。
    let mut f = fs::File::open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if f.metadata()?.nlink() > 1 {
            return Ok(false);
        }
    }
    let mut current = Vec::new();
    f.read_to_end(&mut current)?;
    if current != expected {
        return Ok(false);
    }
    fs::rename(&temp, path)?;
    temp.disable_cleanup(true);
    Ok(true)
}

// 启动把不存在的设置视为内置默认。其余读取错误必须保留 io::Error，即使回滚成功也退出 1。
fn startup_settings(path: &Path) -> io::Result<(settings::Settings, bool)> {
    match fs::read_to_string(path) {
        Ok(src) => {
            let (layer, ds) = settings::file::parse(&src, path);
            Ok((layer, !aproxy_ipc::diag::has_errors(&ds)))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok((settings::Settings::default(), true)),
        Err(e) => Err(e),
    }
}

/// 检查指定 home；每个目标单独成败，不联系正在运行的进程，也不读取期望状态记录。
pub fn run(home: &Path, paths: &[PathBuf], dry: bool, lang: Lang) -> Vec<ResultFile> {
    run_with(home, paths, dry, lang, &|_, _| {})
}

fn run_with(
    home: &Path,
    paths: &[PathBuf],
    dry: bool,
    lang: Lang,
    observe: &impl Fn(Point, &Path),
) -> Vec<ResultFile> {
    let sp = settings::settings_path_in(home);
    let _lock = if dry {
        None
    } else {
        match config::edit::lock(home) {
            Ok(l) => Some(l),
            Err(e) => {
                let kind = if paths.is_empty() {
                    Kind::Settings
                } else {
                    Kind::Config
                };
                let targets = if paths.is_empty() {
                    std::slice::from_ref(&sp)
                } else {
                    paths
                };
                let mut seen = HashSet::new();
                return targets
                    .iter()
                    .map(|p| fs::canonicalize(p).unwrap_or_else(|_| p.clone()))
                    .filter(|p| seen.insert(p.clone()))
                    .map(|p| {
                        let mut f = ResultFile::empty(&p);
                        f.problem(kind, true, settings::describe_edit_error(&e, &p, lang));
                        f
                    })
                    .collect();
            }
        }
    };
    let mut reports = vec![];
    let mut written = vec![];
    let mut seen = HashSet::new();
    let mut inventory = settings::Settings::default();
    let mut defaults = Defaults::default();
    let mut settings_layer = Ok(true);
    let explicit_settings = (!paths.is_empty())
        .then(|| fs::canonicalize(&sp).ok())
        .flatten();
    let run_dir = fs::canonicalize(home.join("run")).unwrap_or_else(|_| home.join("run"));
    // 同样是 NotFound，设置里留下的旧引用需要人工清理，命令行点名的文件则是读取失败。
    enum Target {
        File,
        Reference,
        DirectoryError(io::Error),
    }
    // 发现路径时只用 settings 中可读的别名和目录；旧键出错不应让目录变成空集。
    let mut targets = BTreeMap::new();
    for kind in [Kind::Settings, Kind::Config] {
        if kind == Kind::Settings {
            if paths.is_empty() {
                targets.insert(sp.clone(), Target::File);
            } else {
                // 明确给出的配置不扩展为整座 home；只为启动校验读取生效的设置层。
                settings_layer = startup_settings(&sp).map(|(layer, valid)| {
                    defaults = Defaults::from_settings(&layer, &sp);
                    valid
                });
            }
        } else if !paths.is_empty() {
            targets.extend(paths.iter().cloned().map(|p| (p, Target::File)));
        } else {
            settings_layer = Ok(reports
                .iter()
                .all(|r: &ResultFile| matches!(r.outcome, "current" | "migrated")));
            for raw in inventory
                .aliases
                .values()
                .chain(inventory.default_config.iter())
            {
                targets.insert(settings::expand_path(raw), Target::Reference);
            }
            let dirs = settings::effective_config_dirs_in(&inventory, home);
            find::scan(dirs, |path, result| match result {
                Ok(()) => {
                    targets.entry(path).or_insert(Target::File);
                }
                Err(e) => {
                    // 默认目录本来就可以不存在；只有用户在 config_dirs 里写下的目录才需要修复。
                    // 生效目录按用户项优先去重，保留的是该项展开后的路径。
                    if e.kind() != io::ErrorKind::NotFound
                        || inventory
                            .config_dirs
                            .iter()
                            .any(|raw| settings::expand_path(raw) == path)
                    {
                        targets.insert(path, Target::DirectoryError(e));
                    }
                }
            });
        }
        for (path, target) in std::mem::take(&mut targets) {
            let (resolved, missing) = match target {
                Target::File => (fs::canonicalize(&path), None),
                Target::Reference => (fs::canonicalize(&path), Some(text::missing_target(lang))),
                Target::DirectoryError(e) => (Err(e), Some(text::missing_dir(lang))),
            };
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
                    if let Some(message) = missing.filter(|_| e.kind() == io::ErrorKind::NotFound) {
                        report.outcome = "needs-review";
                        report.issues.push(Issue {
                            key: None,
                            line: None,
                            section: "compatibility/migrate.md#converting-the-files-with-aproxy-migrate",
                            message: message.into(),
                        });
                    } else {
                        report.problem(kind, true, text::io(lang, e.kind()));
                    }
                    reports.push(report);
                    continue;
                }
            };
            if !seen.insert(path.clone()) {
                continue;
            }
            let mut report = ResultFile::empty(&path);
            let result = (|| -> io::Result<()> {
                if explicit_settings.as_ref() == Some(&path) {
                    report.problem(kind, false, text::settings_target(lang).into());
                    return Ok(());
                }
                if path.starts_with(&run_dir)
                    || path.file_name().is_some_and(|n| {
                        n.as_encoded_bytes()
                            .last_chunk::<8>()
                            .is_some_and(|end| end.eq_ignore_ascii_case(b".0.1.bak"))
                    })
                {
                    report.problem(kind, false, text::reserved(lang).into());
                    return Ok(());
                }
                let meta = fs::metadata(&path)?;
                if !meta.is_file() {
                    return Err(io::ErrorKind::InvalidInput.into());
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;
                    if meta.nlink() > 1 {
                        report.problem(kind, false, text::linked(lang).into());
                        return Ok(());
                    }
                }
                let src = fs::read_to_string(&path)?;
                let Plan {
                    next,
                    changes,
                    issues,
                } = plan(kind, &src, &path, lang);
                if kind == Kind::Settings {
                    inventory = settings::file::parse(&src, &path).0;
                    // 严格设置读取器拒绝混合类型数组；发现文件仍可使用每个独立的字符串。
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&src)
                        && let Some(dirs) = v["config_dirs"].as_array()
                    {
                        inventory.config_dirs = dirs
                            .iter()
                            .filter_map(|d| d.as_str().map(str::to_owned))
                            .collect();
                    }
                }
                report.changes = changes;
                report.issues = issues;
                if !report.issues.is_empty() {
                    report.outcome = "needs-review";
                }
                if let Some(next) = next {
                    if kind == Kind::Config {
                        if let Err(e) = &settings_layer {
                            report.problem(kind, true, text::io(lang, e.kind()));
                            return Ok(());
                        }
                        if matches!(settings_layer, Ok(false)) {
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
                                message: text::invalid(lang).into(),
                                section: kind.section(),
                            });
                        }
                        if !report.issues.is_empty() {
                            report.outcome = "needs-review";
                            report.changes.clear();
                            return Ok(());
                        }
                    }
                    if !preserve(&path, &src, dry, observe)? {
                        report.problem(kind, false, text::different_backup(lang).into());
                        return Ok(());
                    }
                    if !dry {
                        if !replace(
                            &path,
                            next.as_bytes(),
                            src.as_bytes(),
                            Point::Replace,
                            observe,
                        )? {
                            report.problem(kind, false, text::changed(lang).into());
                            return Ok(());
                        }
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
            observe(Point::Validate, &path);
            let valid = (|| -> io::Result<bool> {
                let current = fs::read_to_string(&path)?;
                if current != next {
                    return Ok(false);
                }
                if kind == Kind::Settings {
                    return Ok(!aproxy_ipc::diag::has_errors(
                        &settings::file::parse(&current, &path).1,
                    ));
                }
                let (settings, valid) = startup_settings(&sp)?;
                Ok(valid
                    && config::resolve::resolve_text(
                        Some(&current),
                        &path,
                        &config::Overrides::default(),
                        &Defaults::from_settings(&settings, &sp),
                        config::resolve::env_proxy_is_set(),
                    )
                    .is_valid())
            })();
            if !matches!(valid, Ok(true)) {
                // 先记录读故障；恢复失败或发现外部编辑都不能降低它的优先级。
                let failed = valid.is_err();
                if let Err(e) = valid {
                    r.problem(kind, true, text::io(lang, e.kind()));
                }
                let original = fs::read(backup(&path))?;
                if original != before.as_bytes()
                    || !replace(&path, &original, next.as_bytes(), Point::Restore, observe)?
                {
                    r.problem(kind, failed, text::changed(lang).into());
                } else {
                    r.problem(kind, failed, text::restored(lang).into());
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

#[cfg(test)]
mod tests {
    use super::*;
    // 在已同步、尚未发表的边界改变文件，而非靠 sleep 猜调度。所有路径只属于本次测试。
    #[test]
    fn guarded_publication_and_revalidation_preserve_the_right_bytes() {
        for (event, external, outcome, code) in [
            (Point::PublishBackup, "competing backup", "needs-review", 3),
            (Point::Replace, "external config", "needs-review", 3),
            (Point::Validate, "invalid settings", "needs-review", 3),
            (Point::Validate, "unreadable settings", "failed", 1),
            (Point::Restore, "external config", "needs-review", 3),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let home = dir.path();
            let path = home.join("client.toml");
            let settings = home.join("settings.json");
            let old = "base_url='http://localhost'\nmax_body_mb=3\n";
            fs::write(&path, old).unwrap();
            fs::write(&settings, "{}").unwrap();
            let result = run_with(
                home,
                std::slice::from_ref(&path),
                false,
                Lang::En,
                &|point, target| {
                    if point == Point::Validate {
                        assert_ne!(fs::read_to_string(target).unwrap(), old);
                        assert_eq!(fs::read_to_string(backup(target)).unwrap(), old);
                        if event == Point::Restore || external == "invalid settings" {
                            fs::write(&settings, "{").unwrap();
                        }
                        if external == "unreadable settings" {
                            fs::write(&settings, [0xff]).unwrap();
                        }
                    }
                    if point == event && point != Point::Validate {
                        if point == Point::PublishBackup {
                            assert!(!target.exists());
                        }
                        fs::write(target, external).unwrap();
                    }
                },
            );
            assert_eq!(result.len(), 1);
            assert_eq!(result[0].outcome, outcome, "{}", serde_json::json!(result));
            assert_eq!(super::super::exit_code(&result), code);
            assert_eq!(
                fs::read_to_string(&path).unwrap(),
                if matches!(event, Point::Replace | Point::Restore) {
                    external
                } else {
                    old
                }
            );
            assert_eq!(
                fs::read_to_string(backup(&path)).unwrap(),
                if event == Point::PublishBackup {
                    external
                } else {
                    old
                }
            );
        }
    }
}
