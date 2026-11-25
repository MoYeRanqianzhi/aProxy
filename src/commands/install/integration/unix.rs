//! Unix 的 shell 配置只追加一行，不解码、重写或换行归一化用户原有的字节。
use super::Step;
use std::{
    env, fs,
    io::{self, Write},
    os::unix::fs::PermissionsExt,
    path::Path,
};

fn printable(path: &Path) -> io::Result<&str> {
    path.to_str()
        .filter(|s| !s.chars().any(char::is_control))
        .ok_or(io::ErrorKind::InvalidInput.into())
}

fn add_line(step: &mut Step, file: &Path, line: &str, equivalent: &[String]) -> io::Result<()> {
    let old = match fs::read(file) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e),
    };
    if old
        .split(|&b| b == b'\n')
        .map(<[u8]>::trim_ascii)
        .any(|b| b == line.as_bytes() || equivalent.iter().any(|other| other.as_bytes() == b))
    {
        return Ok(());
    }
    fs::create_dir_all(file.parent().unwrap())?;
    let mut append = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)?;
    let sep = if old.is_empty() || old.ends_with(b"\n") {
        ""
    } else {
        "\n"
    };
    append.write_all(format!("{sep}{line}\n").as_bytes())?;
    step.touched.push(file.display().to_string());
    Ok(())
}

pub(super) fn path(step: &mut Step, home: &Path) -> io::Result<()> {
    let search = env::var_os("PATH").unwrap_or_default();
    if env::split_paths(&search).any(|p| p == home.join("bin")) {
        return Ok(());
    }
    let profile = aproxy_ipc::home::user_dir()
        .filter(|p| p.is_absolute())
        .ok_or(io::ErrorKind::NotFound)?;
    let zsh = cfg!(target_os = "macos")
        || env::var_os("SHELL").is_some_and(|s| Path::new(&s).file_name() == Some("zsh".as_ref()))
        || profile.join(".zshrc").try_exists()?;
    let fish = env::split_paths(&search).any(|p| {
        p.join("fish")
            .metadata()
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    });
    profiles(step, home, &profile, zsh, fish)
}

fn profiles(step: &mut Step, home: &Path, profile: &Path, zsh: bool, fish: bool) -> io::Result<()> {
    let env_file = home.join("env");
    let bin = home.join("bin");
    let bin = printable(&bin)?;
    if bin.contains(':') {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let quoted = aproxy::cli::quoted_arg(bin);
    let env_path = printable(&env_file)?;
    let command = format!(". {}", aproxy::cli::quoted_arg(env_path));
    let source = format!("{command} # aProxy");
    // 只认不需要执行就能判定等价的手写引用，不把注释或条件分支当成已配置。
    let mut equivalent = vec![command];
    if !env_path.contains(['"', '$', '`', '\\']) {
        equivalent.push(format!(". \"{env_path}\""));
    }
    if env_path
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"/_.-".contains(&b))
    {
        equivalent.push(format!(". {env_path}"));
    }
    let body = format!(
        "case \":${{PATH-}}:\" in\n  *:{quoted}:*) ;;\n  *) export PATH={quoted}${{PATH:+:\"$PATH\"}} ;;\nesac\n"
    );
    step.write(&env_file, body.as_bytes())?;
    add_line(step, &profile.join(".profile"), &source, &equivalent)?;
    for name in [".bashrc", ".bash_profile"] {
        let p = profile.join(name);
        if p.try_exists()? {
            add_line(step, &p, &source, &equivalent)?;
        }
    }
    if zsh {
        add_line(step, &profile.join(".zshenv"), &source, &equivalent)?;
    }
    if fish {
        // fish 单引号内仍解释反斜杠；不用 fish_add_path 的 universal 变量，以免额外持久化。
        let q = format!("'{}'", bin.replace('\\', "\\\\").replace('\'', "\\'"));
        add_line(
            step,
            &profile.join(".config/fish/conf.d/aproxy.fish"),
            &format!("contains -- {q} $PATH; or set -gx PATH {q} $PATH # aProxy"),
            &[],
        )?;
    }
    Ok(())
}

#[cfg(all(feature = "desktop", not(target_os = "macos")))]
pub(super) fn shortcut(step: &mut Step, home: &Path, profile: &Path) -> io::Result<()> {
    let data = env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| profile.join(".local/share"));
    desktop_file(step, home, profile, &data)
}

#[cfg(all(feature = "desktop", not(target_os = "macos")))]
fn desktop_file(step: &mut Step, home: &Path, profile: &Path, data: &Path) -> io::Result<()> {
    let exe = aproxy::install::swap::bin_path_in(home);
    let exe = printable(&exe)?;
    // Exec 的程序名不能有 =，引号内不能放字段码。不要让桌面把路径中的 %f 当成文件参数。
    if exe.contains(['%', '=']) {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let mut token = String::from("\"");
    for c in exe.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            token.push('\\');
        }
        token.push(c);
    }
    token.push('"');
    let token = token.replace('\\', "\\\\");
    let suffix = super::suffix(home, profile);
    let entry = format!(
        "[Desktop Entry]\nType=Application\nName=aProxy{suffix}\nExec={token} desktop --installed-home\nIcon=aproxy\nTerminal=false\nCategories=Network;Development;\nStartupWMClass=Aproxy\n"
    );
    // Icon 键不带尺寸；由桌面按缩放倍率从同一主题选最合适的图。
    for (px, png) in [
        (128, include_bytes!("icon.png").as_slice()),
        (256, include_bytes!("icon-256.png").as_slice()),
        (512, include_bytes!("icon-512.png").as_slice()),
    ] {
        step.write(
            &data.join(format!("icons/hicolor/{px}x{px}/apps/aproxy.png")),
            png,
        )?;
    }
    step.write(
        &data.join(format!("applications/aproxy{suffix}.desktop")),
        entry.as_bytes(),
    )
}

#[cfg(all(feature = "desktop", target_os = "macos"))]
pub(super) fn shortcut(step: &mut Step, home: &Path, profile: &Path) -> io::Result<()> {
    let suffix = super::suffix(home, profile);
    let contents = profile.join(format!("Applications/aProxy{suffix}.app/Contents"));
    let name = printable(home)?
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    // LaunchServices 从包内的原生入口启动事件循环本身。符号链接指向稳定文件名，不钉住交换前的 inode。
    let info = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleExecutable</key><string>aProxy</string>
<key>CFBundleName</key><string>aProxy</string>
<key>CFBundleDisplayName</key><string>aProxy</string>
<key>CFBundleIdentifier</key><string>io.github.moyeranqianzhi.aproxy{suffix}</string>
<key>CFBundleIconFile</key><string>aProxy.icns</string>
<key>LSEnvironment</key><dict><key>APROXY_HOME</key><string>{name}</string><key>APROXY_BUNDLE_LAUNCH</key><string>1</string></dict>
</dict></plist>
"#
    );
    step.write(
        &contents.join("Resources/aProxy.icns"),
        include_bytes!("icon.icns"),
    )?;
    let macos = contents.join("MacOS");
    fs::create_dir_all(&macos)?;
    let link = macos.join("aProxy");
    let exe = aproxy::install::swap::bin_path_in(home);
    if fs::read_link(&link).ok().as_ref() != Some(&exe) {
        let temp = tempfile::tempdir_in(&macos)?;
        let staged = temp.path().join("aProxy");
        std::os::unix::fs::symlink(exe, &staged)?;
        fs::rename(staged, &link)?;
        step.touched.push(link.display().to_string());
    }
    step.write(&contents.join("Info.plist"), info.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn profile_bytes_and_sourcing_survive_a_second_adoption() {
        let tmp = tempfile::tempdir().unwrap();
        let profile = tmp.path();
        let home = profile.join("a 'home*");
        fs::write(profile.join(".bashrc"), b"#\xff").unwrap();
        fs::write(
            profile.join(".bash_profile"),
            format!(". \"{}/env\"\n", home.display()),
        )
        .unwrap();
        let run = || Step::run(|s| profiles(s, &home, profile, true, true));
        assert_eq!(run().touched.len(), 5);
        assert_eq!(run().status, "unchanged");
        assert!(
            fs::read(profile.join(".bashrc"))
                .unwrap()
                .starts_with(b"#\xff\n")
        );
        let out = std::process::Command::new("/bin/sh")
            .args(["-ec", r#". "$1"; . "$1"; printf '%s' "$PATH""#, "sh"])
            .arg(profile.join(".profile"))
            .env("PATH", "/usr/bin")
            .output()
            .unwrap();
        assert!(out.status.success());
        assert_eq!(
            out.stdout,
            format!("{}:/usr/bin", home.join("bin").display()).as_bytes()
        );
        let bare = tempfile::tempdir().unwrap();
        let step = Step::run(|s| profiles(s, &home, bare.path(), false, false));
        assert_eq!(step.touched.len(), 1);
        assert!(!bare.path().join(".bashrc").exists());
    }
    // 不依赖图像解码库：PNG 签名后的首块是 IHDR，宽和高各占四个大端字节。
    #[cfg(feature = "desktop")]
    fn check_png_size(png: &[u8], px: u32) {
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(&png[12..16], b"IHDR");
        assert_eq!(&png[16..20], &px.to_be_bytes());
        assert_eq!(&png[20..24], &px.to_be_bytes());
    }

    #[cfg(all(feature = "desktop", not(target_os = "macos")))]
    #[test]
    fn xdg_application_entry_and_theme_icon() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path();
        let home = p.join(".aproxy");
        let run = || Step::run(|s| desktop_file(s, &home, p, p));
        assert_eq!(run().status, "changed");
        assert_eq!(run().status, "unchanged");
        let text = fs::read_to_string(p.join("applications/aproxy.desktop")).unwrap();
        assert!(text.contains(&format!(
            "Exec=\"{}\" desktop --installed-home",
            home.join("bin/aproxy").display()
        )));
        for key in [
            "Name=aProxy\n",
            "Icon=aproxy\n",
            "Categories=Network;Development;\n",
            "StartupWMClass=Aproxy\n",
        ] {
            assert!(text.contains(key));
        }
        for px in [128_u32, 256, 512] {
            let png = fs::read(p.join(format!("icons/hicolor/{px}x{px}/apps/aproxy.png"))).unwrap();
            check_png_size(&png, px);
        }
    }
    #[cfg(all(feature = "desktop", target_os = "macos"))]
    #[test]
    fn native_bundle_tracks_installed_path() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path();
        let home = p.join(".aproxy");
        let run = || Step::run(|s| shortcut(s, &home, p));
        assert_eq!(run().touched.len(), 3);
        assert_eq!(run().status, "unchanged");
        let c = p.join("Applications/aProxy.app/Contents");
        assert_eq!(
            fs::read_link(c.join("MacOS/aProxy")).unwrap(),
            home.join("bin/aproxy")
        );
        let icns = fs::read(c.join("Resources/aProxy.icns")).unwrap();
        assert_eq!(&icns[..4], b"icns");
        assert_eq!(
            u32::from_be_bytes(icns[4..8].try_into().unwrap()) as usize,
            icns.len()
        );
        let mut frames = &icns[8..];
        for (kind, px) in [
            (b"icp4", 16),
            (b"icp5", 32),
            (b"icp6", 64),
            (b"ic07", 128),
            (b"ic08", 256),
            (b"ic09", 512),
            (b"ic10", 1024),
        ] {
            assert_eq!(&frames[..4], kind);
            let len = u32::from_be_bytes(frames[4..8].try_into().unwrap()) as usize;
            check_png_size(&frames[8..len], px);
            frames = &frames[len..];
        }
        assert!(frames.is_empty());
        let info = fs::read_to_string(c.join("Info.plist")).unwrap();
        for key in ["CFBundleName", "CFBundleDisplayName", "CFBundleExecutable"] {
            assert!(info.contains(&format!("<key>{key}</key><string>aProxy</string>")));
        }
        assert!(info.contains("<key>LSEnvironment</key>"));
    }
}
