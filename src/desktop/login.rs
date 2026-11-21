//! 按主目录标识当前用户的登录项。只有固定的本机可执行文件和参数，绝不执行页面提供的命令。
use super::DesktopHome;
use aproxy_ipc::desktop::*;
use sha2::{Digest, Sha256};
use std::{io, path::Path};
fn fault(reason: RegistrationReason) -> Fault {
    Fault::DesktopRegistration { reason }
}
fn os(e: io::Error) -> Fault {
    fault(if e.kind() == io::ErrorKind::PermissionDenied {
        RegistrationReason::Permission
    } else {
        RegistrationReason::SystemUnavailable
    })
}
fn encoded(p: &Path) -> R<&str> {
    p.to_str()
        .filter(|t| !t.chars().any(char::is_control))
        .ok_or_else(|| fault(RegistrationReason::UnsupportedPath))
}
struct Item {
    label: String,
    expected: String,
    binary: bool,
}
impl DesktopHome {
    /// 隔离系统登记的测试入口。调用者只可选择随机十六进制标记，不能指定系统键或用户目录。
    #[doc(hidden)]
    pub fn with_isolated_login_slot(mut self, random: &str) -> R<Self> {
        if !(16..=64).contains(&random.len()) || !random.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Fault::DesktopBadInput {});
        }
        self.login_test_slot = Some(random.into());
        Ok(self)
    }
    fn login_item(&self) -> R<Item> {
        let home = self.home.canonicalize().map_err(os)?;
        let display = platform::path(&home)?;
        let exe = crate::install::swap::bin_path_in(Path::new(&display));
        let native = encoded(&exe)?;
        let identity = if cfg!(windows) {
            display.to_lowercase()
        } else {
            display.clone()
        };
        let hash = Sha256::digest(identity.as_bytes());
        let short = hash[..8]
            .iter()
            .map(|n| format!("{n:02x}"))
            .collect::<String>();
        let label = format!("io.aproxy.restore.{short}");
        let expected = platform::document(&label, native, &display)?;
        Ok(Item {
            label,
            expected,
            binary: exe.is_file(),
        })
    }
    fn registration(&self, change: Option<bool>) -> R<Registration> {
        let item = self.login_item()?;
        let store = platform::Store::new(self, &item)?;
        let actual = store.read()?;
        let matches = actual.as_deref() == Some(item.expected.as_str());
        if let Some(enable) = change {
            if enable && !item.binary {
                return Err(fault(RegistrationReason::BinaryMissing));
            }
            if actual.is_some() && !matches {
                return Err(fault(RegistrationReason::EntryMismatch));
            }
            if enable != matches {
                store.write(enable, &item.expected)?;
            } else if enable {
                store.ensure_loaded()?;
            }
            let verified = store.read()?;
            if (enable && (verified.as_deref() != Some(&item.expected) || !store.loaded()?))
                || (!enable && verified.is_some())
            {
                return Err(fault(RegistrationReason::VerificationFailed));
            }
        }
        let actual = if change.is_some() {
            store.read()?
        } else {
            actual
        };
        let exact = actual.as_deref() == Some(item.expected.as_str());
        let enabled = item.binary && exact && store.loaded()?;
        Ok(Registration {
            enabled,
            issue: if !item.binary {
                Some(RegistrationIssue::BinaryMissing)
            } else if actual.is_some() && !enabled {
                Some(RegistrationIssue::EntryMismatch)
            } else {
                None
            },
        })
    }
}
impl DesktopOps for DesktopHome {
    fn read_autostart(&self) -> Task<'_, Registration> {
        Box::pin(async move { self.registration(None) })
    }
    fn set_autostart(&self, enabled: bool) -> Task<'_, Registration> {
        Box::pin(async move { self.registration(Some(enabled)) })
    }
}
#[cfg(windows)]
mod platform {
    use super::*;
    use windows_sys::Win32::System::{
        LibraryLoader::{GetModuleHandleW, GetProcAddress},
        SystemInformation::GetSystemDirectoryW,
    };
    use winreg::{
        RegKey,
        enums::{HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SZ},
    };
    pub(super) fn path(p: &Path) -> R<String> {
        let s = encoded(p)?;
        let s = if let Some(unc) = s.strip_prefix(r"\\?\UNC\") {
            format!(r"\\{unc}")
        } else {
            s.strip_prefix(r"\\?\").unwrap_or(s).to_owned()
        };
        if s.contains('"') {
            return Err(fault(RegistrationReason::UnsupportedPath));
        }
        Ok(s)
    }
    // CommandLineToArgvW 在右引号之前把两根反斜杠读成一根，卷根等以反斜杠结尾的参数也要往返一致。
    fn quote(s: &str) -> String {
        let mut out = String::from("\"");
        let mut n = 0;
        for ch in s.chars() {
            if ch == '\\' {
                n += 1;
                continue;
            }
            if ch == '"' {
                out.extend(std::iter::repeat_n('\\', 2 * n + 1));
            } else {
                out.extend(std::iter::repeat_n('\\', n));
            }
            n = 0;
            out.push(ch);
        }
        out.extend(std::iter::repeat_n('\\', 2 * n));
        out.push('"');
        out
    }
    pub(super) fn document(_label: &str, exe: &str, home: &str) -> R<String> {
        let direct = format!("{} restore-at-login --home {}", quote(exe), quote(home));
        let k = "kernel32.dll\0".encode_utf16().collect::<Vec<_>>();
        // 旧版系统没有伪控制台；动态查符号而非静态导入，保留直接登记的可用性。
        let conpty = unsafe {
            let h = GetModuleHandleW(k.as_ptr());
            h != 0 && GetProcAddress(h, c"CreatePseudoConsole".as_ptr().cast()).is_some()
        };
        let cmd = if conpty {
            let mut buf = [0; 260];
            let n = unsafe { GetSystemDirectoryW(buf.as_mut_ptr(), buf.len() as u32) } as usize;
            if n == 0 || n >= buf.len() {
                return Err(fault(RegistrationReason::SystemUnavailable));
            }
            let sys = String::from_utf16(&buf[..n])
                .map_err(|_| fault(RegistrationReason::UnsupportedPath))?;
            format!(
                "{} --headless {direct}",
                quote(&format!(r"{sys}\conhost.exe"))
            )
        } else {
            direct
        };
        if cmd.encode_utf16().count() > 260 {
            return Err(fault(RegistrationReason::UnsupportedPath));
        }
        Ok(cmd)
    }
    pub(super) struct Store {
        key: String,
        name: String,
    }
    impl Store {
        pub(super) fn new(h: &DesktopHome, i: &Item) -> R<Self> {
            Ok(Self {
                key: h.login_test_slot.as_ref().map_or_else(
                    || r"Software\Microsoft\Windows\CurrentVersion\Run".into(),
                    |s| format!(r"Software\aProxy-test-{s}"),
                ),
                name: i.label.clone(),
            })
        }
        pub(super) fn read(&self) -> R<Option<String>> {
            let hk = RegKey::predef(HKEY_CURRENT_USER);
            let got = hk
                .open_subkey_with_flags(&self.key, KEY_QUERY_VALUE)
                .and_then(|k| k.get_raw_value(&self.name));
            match got {
                Ok(v) if v.vtype == REG_SZ => {
                    use winreg::types::FromRegValue;
                    Ok(Some(String::from_reg_value(&v).map_err(os)?))
                }
                Ok(_) => Ok(Some(String::new())),
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(os(e)),
            }
        }
        pub(super) fn write(&self, on: bool, text: &str) -> R<()> {
            let hk = RegKey::predef(HKEY_CURRENT_USER);
            if on {
                let (k, _) = hk
                    .create_subkey_with_flags(&self.key, KEY_SET_VALUE)
                    .map_err(os)?;
                k.set_value(&self.name, &text).map_err(os)
            } else {
                hk.open_subkey_with_flags(&self.key, KEY_SET_VALUE)
                    .and_then(|k| k.delete_value(&self.name))
                    .map_err(os)
            }
        }
        pub(super) fn loaded(&self) -> R<bool> {
            Ok(true)
        }
        pub(super) fn ensure_loaded(&self) -> R<()> {
            Ok(())
        }
    }
}
#[cfg(not(windows))]
mod platform {
    use super::*;
    use std::{fs, path::PathBuf};
    pub(super) fn path(p: &Path) -> R<String> {
        Ok(encoded(p)?.to_owned())
    }
    #[cfg(not(target_os = "macos"))]
    fn desktop_arg(s: &str) -> String {
        let mut out = String::from("\"");
        for ch in s.chars() {
            if matches!(ch, '"' | '$' | '`' | '\\') {
                out.push('\\')
            }
            out.push(ch);
        }
        out.push('"');
        out.replace('\\', "\\\\")
    }
    #[cfg(not(target_os = "macos"))]
    pub(super) fn document(_label: &str, exe: &str, home: &str) -> R<String> {
        if exe.contains('%') || home.contains('%') {
            return Err(fault(RegistrationReason::UnsupportedPath));
        }
        Ok(format!(
            "[Desktop Entry]\nType=Application\nName=aProxy\nExec={} restore-at-login --home {}\nTerminal=false\nHidden=false\n",
            desktop_arg(exe),
            desktop_arg(home)
        ))
    }
    #[cfg(target_os = "macos")]
    pub(super) fn document(label: &str, exe: &str, home: &str) -> R<String> {
        let xml = |v: &str| {
            v.replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
        };
        Ok(format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict><key>Label</key><string>{label}</string><key>ProgramArguments</key><array><string>{}</string><string>restore-at-login</string><string>--home</string><string>{}</string></array><key>RunAtLoad</key><true/></dict></plist>\n",
            xml(exe),
            xml(home)
        ))
    }
    pub(super) struct Store {
        file: PathBuf,
        #[cfg(target_os = "macos")]
        isolated: bool,
        #[cfg(target_os = "macos")]
        label: String,
    }
    impl Store {
        pub(super) fn new(h: &DesktopHome, i: &Item) -> R<Self> {
            let dir = if let Some(s) = &h.login_test_slot {
                h.home.join(format!("login-test-{s}"))
            } else {
                platform_dir()?
            };
            let ext = if cfg!(target_os = "macos") {
                "plist"
            } else {
                "desktop"
            };
            Ok(Self {
                file: dir.join(format!("{}.{ext}", i.label)),
                #[cfg(target_os = "macos")]
                isolated: h.login_test_slot.is_some(),
                #[cfg(target_os = "macos")]
                label: i.label.clone(),
            })
        }
        pub(super) fn read(&self) -> R<Option<String>> {
            use std::io::Read;
            match fs::symlink_metadata(&self.file) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
                Err(e) => return Err(os(e)),
                Ok(m) if !m.is_file() || m.len() > 64 * 1024 => return Ok(Some(String::new())),
                Ok(_) => {}
            }
            let mut bytes = Vec::new();
            fs::File::open(&self.file)
                .and_then(|f| f.take(64 * 1024 + 1).read_to_end(&mut bytes))
                .map_err(os)?;
            Ok(Some(String::from_utf8(bytes).unwrap_or_default()))
        }
        pub(super) fn write(&self, on: bool, text: &str) -> R<()> {
            if on {
                crate::fsx::write_atomic_private(&self.file, text.as_bytes()).map_err(os)?;
                self.ensure_loaded()
            } else {
                #[cfg(target_os = "macos")]
                if !self.isolated && self.loaded()? && !self.launchctl("bootout", true)? {
                    return Err(fault(RegistrationReason::SystemUnavailable));
                }
                fs::remove_file(&self.file).map_err(os)
            }
        }
        #[cfg(not(target_os = "macos"))]
        pub(super) fn loaded(&self) -> R<bool> {
            Ok(true)
        }
        #[cfg(not(target_os = "macos"))]
        pub(super) fn ensure_loaded(&self) -> R<()> {
            Ok(())
        }
        #[cfg(target_os = "macos")]
        pub(super) fn loaded(&self) -> R<bool> {
            if self.isolated {
                return Ok(true);
            }
            self.launchctl("print", false)
        }
        #[cfg(target_os = "macos")]
        pub(super) fn ensure_loaded(&self) -> R<()> {
            if !self.loaded()? && !self.launchctl("bootstrap", true)? {
                return Err(fault(RegistrationReason::SystemUnavailable));
            }
            Ok(())
        }
        #[cfg(target_os = "macos")]
        fn launchctl(&self, action: &str, with_file: bool) -> R<bool> {
            use std::process::{Command, Stdio};
            let domain = format!("gui/{}", unsafe { libc::getuid() });
            let mut c = Command::new("/bin/launchctl");
            c.arg(action);
            if with_file {
                c.arg(domain).arg(&self.file);
            } else {
                c.arg(format!("{domain}/{}", self.label));
            }
            c.stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| s.success())
                .map_err(os)
        }
    }
    #[cfg(not(target_os = "macos"))]
    fn platform_dir() -> R<PathBuf> {
        let cfg = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| aproxy_ipc::home::user_dir().map(|p| p.join(".config")))
            .ok_or_else(|| fault(RegistrationReason::SystemUnavailable))?;
        Ok(cfg.join("autostart"))
    }
    #[cfg(target_os = "macos")]
    fn platform_dir() -> R<PathBuf> {
        aproxy_ipc::home::user_dir()
            .map(|p| p.join("Library/LaunchAgents"))
            .ok_or_else(|| fault(RegistrationReason::SystemUnavailable))
    }
}
