//! 实例与离线工具按同一工作目录、环境覆盖和搜索顺序定位格式程序。
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

/// `~` 前缀展开：`~/x`、`~\x`、裸 `~` → 用户目录；其余原样。用户目录是 C1 的那一个
/// （见 `crate::home::user_dir`）：`~` 在 aproxy 里处处指同一个目录，配置里的 `~` 与命令行
/// 路径参数里的 `~`、home 的默认位置 `~/.aproxy` 不会落在两个地方。
/// 展开产物统一用 `/` 分隔（Windows API 同样接受）。
pub fn expand_tilde(s: &str) -> String {
    let home = || {
        crate::home::user_dir()
            .map(|p| {
                p.display()
                    .to_string()
                    .trim_end_matches(['/', '\\'])
                    .to_string()
            })
            .unwrap_or_default()
    };
    if s == "~" {
        return home();
    }
    if let Some(rest) = s.strip_prefix("~/").or_else(|| s.strip_prefix("~\\")) {
        return format!("{}/{}", home(), rest);
    }
    s.to_string()
}

/// 程序名只在 home/bin 与 PATH 中查，不让后台进程的当前目录意外参与解析。
pub fn resolve(command: &str, home: &Path, env: &BTreeMap<String, String>) -> Option<PathBuf> {
    let env_path = env.iter().rev().find(|(key, _)| {
        if cfg!(windows) {
            key.eq_ignore_ascii_case("PATH")
        } else {
            key.as_str() == "PATH"
        }
    });
    let inherited = std::env::var_os("PATH");
    let path = env_path
        .map(|(_, value)| std::ffi::OsStr::new(value))
        .or(inherited.as_deref());
    program_path(command, home, path)
}
/// 以给定的 PATH 快照定位可执行文件；未设置 PATH 不应搜索当前目录。
fn program_path(command: &str, home: &Path, path: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    let expanded = expand_tilde(command);
    let p = Path::new(&expanded);
    if p.file_name().is_none_or(|name| name != p.as_os_str()) {
        return Some(home.join(p));
    }
    #[cfg(windows)]
    let exe = p.extension().is_none().then(|| p.with_extension("exe"));
    #[cfg(windows)]
    let p = exe.as_deref().unwrap_or(p);
    for dir in
        std::iter::once(home.join("bin")).chain(path.into_iter().flat_map(std::env::split_paths))
    {
        let file = home.join(dir).join(p);
        if let Ok(meta) = file.metadata()
            && meta.is_file()
        {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if meta.permissions().mode() & 0o111 == 0 {
                    continue;
                }
            }
            return Some(file);
        }
    }
    None
}

/// 命令搜索忽略不可执行的影子，home/bin 胜过显式 PATH；不存在的 PATH 不补当前目录。
#[cfg(test)]
#[test]
fn command_lookup_uses_runnable_candidates_and_the_supplied_path() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let bin = home.join("bin");
    let other = home.join("path");
    std::fs::create_dir(&bin).unwrap();
    std::fs::create_dir(&other).unwrap();
    #[cfg(windows)]
    {
        std::fs::write(bin.join("tool"), b"shadow").unwrap();
        std::fs::write(other.join("tool.exe"), b"fixture").unwrap();
        assert_eq!(
            program_path("tool", home, Some(other.as_os_str())).unwrap(),
            other.join("tool.exe")
        );
        assert!(program_path("tool", home, None).is_none());
        std::fs::write(bin.join("tool.exe"), b"fixture").unwrap();
        assert_eq!(
            program_path("tool", home, Some(other.as_os_str())).unwrap(),
            bin.join("tool.exe")
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let earlier = home.join("first");
        std::fs::create_dir(&earlier).unwrap();
        for d in [&bin, &earlier, &other] {
            std::fs::write(d.join("tool"), b"fixture").unwrap();
        }
        std::fs::set_permissions(other.join("tool"), std::fs::Permissions::from_mode(0o700))
            .unwrap();
        let path = std::env::join_paths([earlier, other.clone()]).unwrap();
        assert_eq!(
            program_path("tool", home, Some(&path)).unwrap(),
            other.join("tool")
        );
        assert!(program_path("tool", home, None).is_none());
    }
}
