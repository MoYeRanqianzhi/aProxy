//! Windows 只读写当前用户的原始 Path；快捷方式交给 Shell Link 保存，目标不经过命令解释器。
use super::Step;
use std::{env, io, path::Path};
use winreg::{
    RegKey, RegValue,
    enums::{HKEY_CURRENT_USER, KEY_SET_VALUE, REG_EXPAND_SZ, REG_SZ},
    types::ToRegValue,
};

const USER_PATH: &str = r"HKCU\Environment\Path";

fn same(a: &str, b: &str) -> bool {
    a.trim_end_matches('\\').to_lowercase() == b.trim_end_matches('\\').to_lowercase()
}

/// 先比较后写回；把原始值作为输入，测试可以检查原始类型和字节而不打开真实用户的环境键。
fn append(old: Option<RegValue<'_>>, bin: &str) -> io::Result<Option<RegValue<'static>>> {
    let value = old.unwrap_or_else(|| "".to_reg_value());
    if !matches!(value.vtype, REG_SZ | REG_EXPAND_SZ) || !value.bytes.len().is_multiple_of(2) {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let wide: Vec<_> = value
        .bytes
        .as_chunks::<2>()
        .0
        .iter()
        .copied()
        .map(u16::from_le_bytes)
        .collect();
    let raw = String::from_utf16(&wide).map_err(|_| io::ErrorKind::InvalidData)?;
    let raw = raw.trim_end_matches('\0');
    if raw.contains('\0') || bin.contains(';') {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    // 只展开一份用于比较的副本。即使尚未重开终端，也认得注册表中指向同一目录的 %变量%。
    let search = if value.vtype == REG_EXPAND_SZ && raw.contains('%') {
        use windows_sys::Win32::System::Environment::ExpandEnvironmentStringsW;
        let src: Vec<_> = raw.encode_utf16().chain([0]).collect();
        let n = unsafe { ExpandEnvironmentStringsW(src.as_ptr(), std::ptr::null_mut(), 0) };
        if n == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut out = vec![0; n as usize];
        let count = unsafe { ExpandEnvironmentStringsW(src.as_ptr(), out.as_mut_ptr(), n) };
        if count == 0 {
            return Err(io::Error::last_os_error());
        }
        if count > n {
            return Err(io::ErrorKind::InvalidData.into());
        }
        std::borrow::Cow::Owned(
            String::from_utf16(&out[..(count - 1) as usize])
                .map_err(|_| io::ErrorKind::InvalidData)?,
        )
    } else {
        std::borrow::Cow::Borrowed(raw)
    };
    if env::split_paths(search.as_ref() as &str).any(|p| p.to_str().is_some_and(|p| same(p, bin))) {
        return Ok(None);
    }
    if value.vtype == REG_EXPAND_SZ && bin.contains('%') {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let sep = if raw.is_empty() || raw.ends_with(';') {
        ""
    } else {
        ";"
    };
    // 不展开旧条目：%USERPROFILE% 等变量必须继续由每次登录的环境解释。
    let bytes = format!("{raw}{sep}{bin}")
        .to_reg_value()
        .bytes
        .into_owned()
        .into();
    Ok(Some(RegValue {
        bytes,
        vtype: value.vtype,
    }))
}

pub(super) fn path(step: &mut Step, home: &Path) -> io::Result<()> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SETTINGCHANGE,
    };
    let bin = home.join("bin");
    let bin = bin.to_str().ok_or(io::ErrorKind::InvalidInput)?;
    if env::split_paths(&env::var_os("PATH").unwrap_or_default())
        .any(|p| p.to_str().is_some_and(|p| same(p, bin)))
    {
        return Ok(());
    }
    let user = RegKey::predef(HKEY_CURRENT_USER);
    update_user_path(
        step,
        bin,
        || {
            user.open_subkey("Environment")
                .and_then(|k| k.get_raw_value("Path"))
        },
        |raw| {
            user.create_subkey_with_flags("Environment", KEY_SET_VALUE)?
                .0
                .set_raw_value("Path", raw)
        },
        || {
            let environment: Vec<_> = "Environment\0".encode_utf16().collect();
            if unsafe {
                SendMessageTimeoutW(
                    HWND_BROADCAST,
                    WM_SETTINGCHANGE,
                    0,
                    environment.as_ptr() as isize,
                    SMTO_ABORTIFHUNG,
                    1000,
                    std::ptr::null_mut(),
                )
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        },
    )
}

/// 存储和通知分开注入：通知失败时注册表已写入，仍须向用户列出改过的值。
fn update_user_path(
    step: &mut Step,
    bin: &str,
    read: impl FnOnce() -> io::Result<RegValue<'static>>,
    write: impl FnOnce(&RegValue<'_>) -> io::Result<()>,
    notify: impl FnOnce() -> io::Result<()>,
) -> io::Result<()> {
    let old = match read() {
        Ok(v) => Some(v),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(e),
    };
    if let Some(raw) = append(old, bin)? {
        write(&raw)?;
        step.touched.push(USER_PATH.into());
        notify()?;
    }
    Ok(())
}

#[cfg(feature = "desktop")]
mod link {
    use super::*;
    use ::windows::{
        Win32::{
            System::Com::{
                CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
                CoTaskMemFree, CoUninitialize, IPersistFile,
            },
            UI::Shell::{
                FOLDERID_Programs, IShellLinkW, KF_FLAG_DEFAULT, SHGetKnownFolderPath, ShellLink,
            },
        },
        core::{HSTRING, Interface},
    };
    use std::{ffi::OsString, os::windows::ffi::OsStringExt, path::PathBuf};

    pub(super) fn programs() -> io::Result<PathBuf> {
        unsafe {
            let p = SHGetKnownFolderPath(&FOLDERID_Programs, KF_FLAG_DEFAULT, None)?;
            let folder = OsString::from_wide(p.as_wide());
            CoTaskMemFree(Some(p.as_ptr().cast()));
            Ok(folder.into())
        }
    }

    /// COM 初始化与释放配对在同一线程；不解析目标、不触发 Shell 的搜索或界面。
    fn with_link<T>(f: impl FnOnce(IShellLinkW, IPersistFile) -> io::Result<T>) -> io::Result<T> {
        struct Apartment;
        impl Drop for Apartment {
            fn drop(&mut self) {
                unsafe { CoUninitialize() }
            }
        }
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
            let _apartment = Apartment;
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
            let file = link.cast()?;
            f(link, file)
        }
    }

    /// Shell 会在相同目标的两次保存中写入不同的追踪数据。按启动字段判断，不比较二进制序列化。
    fn matches_link(dest: &Path, exe: &Path, home: &Path) -> io::Result<bool> {
        use ::windows::Win32::{
            System::Com::STGM_READ,
            UI::Shell::{
                PropertiesSystem::{IPropertyStore, PSGetPropertyKeyFromName},
                SLGP_RAWPATH,
            },
        };
        with_link(|link, file| unsafe {
            file.Load(&HSTRING::from(dest.as_os_str()), STGM_READ)?;
            let mut buf = vec![0; 32768];
            let read = |b: &[u16]| {
                OsString::from_wide(&b[..b.iter().position(|&u| u == 0).unwrap_or(b.len())])
            };
            link.GetPath(&mut buf, std::ptr::null_mut(), SLGP_RAWPATH.0 as u32)?;
            if read(&buf) != exe.as_os_str() {
                return Ok(false);
            }
            link.GetArguments(&mut buf)?;
            if read(&buf) != "desktop --installed-home" {
                return Ok(false);
            }
            link.GetWorkingDirectory(&mut buf)?;
            if read(&buf) != home.as_os_str() {
                return Ok(false);
            }
            let mut index = 0;
            link.GetIconLocation(&mut buf, &mut index)?;
            if index != 0 || read(&buf) != exe.as_os_str() {
                return Ok(false);
            }
            let store: IPropertyStore = link.cast()?;
            let mut id = Default::default();
            PSGetPropertyKeyFromName(::windows::core::w!("System.AppUserModel.ID"), &mut id)?;
            for i in 0..store.GetCount()? {
                let mut key = Default::default();
                store.GetAt(i, &mut key)?;
                if key == id {
                    return Ok(false);
                }
            }
            Ok(true)
        })
    }

    pub(super) fn save(step: &mut Step, home: &Path, dest: &Path) -> io::Result<()> {
        let exe = aproxy::install::swap::bin_path_in(home);
        if dest.try_exists()? && matches_link(dest, &exe, home).unwrap_or(false) {
            return Ok(());
        }
        // 相对目标信息以菜单所在目录为基准；先在同目录写临时项，成功才原子替换可见项。
        let parent = dest.parent().unwrap();
        std::fs::create_dir_all(parent)?;
        let temp = tempfile::Builder::new()
            .suffix(".lnk")
            .tempfile_in(parent)?;
        with_link(|link, file| unsafe {
            let exe = HSTRING::from(exe.as_os_str());
            link.SetPath(&exe)?;
            link.SetArguments(&HSTRING::from("desktop --installed-home"))?;
            link.SetIconLocation(&exe, 0)?;
            link.SetWorkingDirectory(&HSTRING::from(home.as_os_str()))?;
            file.Save(&HSTRING::from(temp.path().as_os_str()), true)?;
            Ok(())
        })?;
        temp.persist(dest)?;
        step.touched.push(dest.display().to_string());
        Ok(())
    }

    #[cfg(test)]
    #[test]
    fn round_trip_without_shell_resolution() {
        use ::windows::Win32::{System::Com::STGM_READ, UI::Shell::SLGP_RAWPATH};
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("a 'home");
        let exe = aproxy::install::swap::bin_path_in(&home);
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, []).unwrap();
        let dest = temp.path().join("Programs/aProxy.lnk");
        let step = Step::run(|s| save(s, &home, &dest));
        assert_eq!(step.status, "changed", "{}", step.reason);
        let again = Step::run(|s| save(s, &home, &dest));
        assert_eq!(again.status, "unchanged", "{}", again.reason);
        with_link(|link, file| unsafe {
            file.Load(&HSTRING::from(dest.as_os_str()), STGM_READ)?;
            // 窗口没有设置显式 AppUserModelID，快捷方式也应让 Shell 自己分组。
            use ::windows::Win32::UI::Shell::PropertiesSystem::{
                IPropertyStore, PSGetPropertyKeyFromName,
            };
            let store: IPropertyStore = link.cast()?;
            let mut app_id = Default::default();
            PSGetPropertyKeyFromName(::windows::core::w!("System.AppUserModel.ID"), &mut app_id)?;
            for i in 0..store.GetCount()? {
                let mut key = Default::default();
                store.GetAt(i, &mut key)?;
                assert_ne!(key, app_id);
            }
            let mut buf = vec![0; 32768];
            let read = |buf: &[u16]| {
                OsString::from_wide(&buf[..buf.iter().position(|&x| x == 0).unwrap()])
            };
            link.GetPath(&mut buf, std::ptr::null_mut(), SLGP_RAWPATH.0 as u32)?;
            assert_eq!(std::fs::canonicalize(read(&buf))?, exe.canonicalize()?);
            link.GetArguments(&mut buf)?;
            assert_eq!(read(&buf), "desktop --installed-home");
            link.GetWorkingDirectory(&mut buf)?;
            assert_eq!(std::fs::canonicalize(read(&buf))?, home.canonicalize()?);
            let mut idx = -1;
            link.GetIconLocation(&mut buf, &mut idx)?;
            assert_eq!(idx, 0);
            assert_eq!(std::fs::canonicalize(read(&buf))?, exe.canonicalize()?);
            Ok(())
        })
        .unwrap();
    }
}

#[cfg(feature = "desktop")]
pub(super) fn shortcut(step: &mut Step, home: &Path, profile: &Path) -> io::Result<()> {
    let name = format!("aProxy{}.lnk", super::suffix(home, profile));
    link::save(step, home, &link::programs()?.join(name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use winreg::types::FromRegValue;
    #[test]
    fn broadcast_failure_keeps_the_write_in_the_report() {
        let mut saved = String::new();
        let step = Step::run(|s| {
            update_user_path(
                s,
                r"C:\A Proxy\bin",
                || Err(io::ErrorKind::NotFound.into()),
                |raw| {
                    saved = String::from_reg_value(raw)?;
                    Ok(())
                },
                || Err(io::ErrorKind::TimedOut.into()),
            )
        });
        assert_eq!(saved, r"C:\A Proxy\bin");
        assert_eq!(step.status, "failed");
        assert_eq!(step.touched, [USER_PATH]);
        let step = Step::run(|s| {
            update_user_path(
                s,
                &saved,
                || Ok(r"C:\A Proxy\bin".to_reg_value()),
                |_| panic!("unexpected write"),
                || panic!("unexpected broadcast"),
            )
        });
        assert_eq!(step.status, "unchanged");
    }
    #[test]
    fn raw_user_path_keeps_type_and_tokens() {
        let bin = r"C:\A Proxy\bin";
        for (raw, ty) in [("", REG_SZ), (r"%USERPROFILE%\bin", REG_EXPAND_SZ)] {
            let mut value = raw.to_reg_value();
            value.vtype = ty.clone();
            let edited = append(Some(value), bin).unwrap().unwrap();
            assert_eq!(edited.vtype, ty);
            assert_eq!(
                String::from_reg_value(&edited).unwrap(),
                if raw.is_empty() {
                    bin.into()
                } else {
                    format!("{raw};{bin}")
                }
            );
            assert!(append(Some(edited), bin).unwrap().is_none());
        }
        assert!(
            append(Some(r"C:\a PROXY\BIN\".to_reg_value()), bin)
                .unwrap()
                .is_none()
        );
        assert_eq!(append(None, bin).unwrap().unwrap().vtype, REG_SZ);
        let mut expandable = r"%SystemRoot%\bin".to_reg_value();
        expandable.vtype = REG_EXPAND_SZ;
        assert!(
            append(
                Some(expandable),
                &format!(r"{}\bin", env::var("SystemRoot").unwrap())
            )
            .unwrap()
            .is_none()
        );
    }
}
