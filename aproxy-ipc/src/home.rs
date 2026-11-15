//! aProxy 主目录（home）的解析：永久兼容契约 C1。
//!
//! **永久兼容契约：本文件的算法无论如何都不得改动。** 一个 home 下的全部东西——配置、
//! `bin/`、台账、安装锁、运行记录——都相对它派生；同一个环境里两个版本只要解析出不同的
//! home，旧版本的强制安装就会去结束另一个 home 里的进程，真正要修的那个一个都没停。规则是
//! 两个纯函数（[`home_from`]、[`user_dir_from`]），仓库里的 `tests/force_contract.rs` 逐条
//! 钉住。
//!
//! 放在本包里：aproxy 本体与一切要找到「同一个 home」的程序（桌面界面、第三方客户端）
//! 用同一份算法，而不是各抄一份。

use std::ffi::OsStr;
use std::path::PathBuf;

/// 指定 aProxy 主目录的环境变量（永久兼容契约 C1）。
pub const HOME_ENV: &str = "APROXY_HOME";

/// 给出用户目录的环境变量（永久兼容契约 C1）：Windows `USERPROFILE`，unix `HOME`——与三个
/// 安装脚本用的 `%USERPROFILE%`、`$HOME` 相同，脚本装进去的 home 与二进制认的 home 才是同一个。
#[cfg(windows)]
pub const USER_DIR_ENV: &str = "USERPROFILE";
/// 给出用户目录的环境变量（永久兼容契约 C1）：unix 上是 `HOME`，理由同上。
#[cfg(unix)]
pub const USER_DIR_ENV: &str = "HOME";

/// aProxy 主目录（算法见 [`resolve_home`]）。config/settings/bin/install/logs/run/ledger
/// 全部相对它派生——测试与多份部署经环境变量整体重定向（子进程经 spawn 继承环境，隔离
/// 白送）。
///
/// 进程入口在执行任何要读 home 的命令之前已用 [`resolve_home`] 核对过，核对不过就报错
/// 退出、走不到这里；所以这里解析失败是程序错误，直接 panic。
pub fn home() -> PathBuf {
    resolve_home().unwrap_or_else(|e| panic!("{e}"))
}

/// 按永久兼容契约 C1 解析 aProxy 主目录。Err 是给人看的说明（APROXY_HOME 不是绝对路径，
/// 或者用户目录也取不到）。
///
/// **永久兼容契约 C1：这套算法无论如何都不得改动。** 强制安装要停的是「这个 home 下」的
/// 全部进程；同一个环境里两个版本只要解析出不同的 home，旧版本的强制安装就会去结束另一个
/// home 里的进程，真正要修的那个一个都没停。规则本身见 [`home_from`] 与 [`user_dir_from`]，
/// 它们是纯函数，`tests/force_contract.rs` 逐条钉住；这里只读环境变量、问系统。
///
/// 一个 home 不支持跨操作系统或跨 pid 命名空间共用（WSL 与 Windows、容器与宿主）：台账
/// 按 pid 记进程，pid 只在同一个系统、同一个命名空间里有意义。
pub fn resolve_home() -> Result<PathBuf, String> {
    home_from(std::env::var_os(HOME_ENV).as_deref(), user_dir)
}

/// C1 的用户目录（规则见 [`user_dir_from`]）。home 的默认位置与路径参数里 `~` 的展开（aproxy
/// 的 `settings::expand_path`）都只从这里取：两处各问各的（例如一处读 USERPROFILE、一处问系统），在
/// 两者不同的机器上，`--config ~/a.toml` 与默认的 `~/.aproxy` 就落在不同的用户目录下。
pub fn user_dir() -> Option<PathBuf> {
    user_dir_from(std::env::var_os(USER_DIR_ENV).as_deref(), system_user_dir)
}

/// C1 的 home 规则。`env_home` 是 APROXY_HOME 的原始值（按原始字节读，不要求 UTF-8）：
/// - 没设、为空、或只含空白（Unicode White_Space）：视为未设，home 是
///   `<用户目录>/.aproxy`，用户目录由 `user_dir` 给出（只在这时调用）；取不到就 Err；
/// - 否则原样使用，不 trim（首尾空白也是路径的一部分）；必须是绝对路径，不是就 Err——
///   相对路径会随工作目录解析成不同的目录，`~` 也不展开。
///
/// 纯函数，环境变量由调用方读好传进来：std::env 是全局可变态，测试直接改进程环境会互相
/// 污染，也可能把 home 指到用户真正在用的目录。
pub fn home_from(
    env_home: Option<&OsStr>,
    user_dir: impl FnOnce() -> Option<PathBuf>,
) -> Result<PathBuf, String> {
    let blank = |value: &OsStr| value.to_str().is_some_and(|s| s.trim().is_empty());
    match env_home {
        Some(value) if !blank(value) => {
            let home = PathBuf::from(value);
            if home.is_absolute() {
                Ok(home)
            } else {
                Err(format!(
                    "{HOME_ENV} 必须是绝对路径，现在是 {:?}：相对路径会随工作目录解析成不同的目录。改成绝对路径，或者删掉这个环境变量（默认用 <用户目录>/.aproxy）。",
                    home.display().to_string()
                ))
            }
        }
        _ => user_dir().map(|dir| dir.join(".aproxy")).ok_or_else(|| {
            format!(
                "取不到用户目录（{USER_DIR_ENV} 为空，系统里也查不到），无法确定 aproxy 的主目录；请把 {HOME_ENV} 设为绝对路径。"
            )
        }),
    }
}

/// C1 的用户目录规则。`env_dir` 是 [`USER_DIR_ENV`] 的原始值：非空就原样用它；否则用
/// `system_dir()`——系统记录的用户目录（见 [`system_user_dir`]）。
pub fn user_dir_from(
    env_dir: Option<&OsStr>,
    system_dir: impl FnOnce() -> Option<PathBuf>,
) -> Option<PathBuf> {
    match env_dir {
        Some(dir) if !dir.is_empty() => Some(PathBuf::from(dir)),
        _ => system_dir(),
    }
}

/// 系统记录的用户目录（C1 在 USERPROFILE 为空时的退路）：Windows 的 FOLDERID_Profile。
#[cfg(windows)]
fn system_user_dir() -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::{FOLDERID_Profile, KF_FLAG_DEFAULT, SHGetKnownFolderPath};
    let mut raw: *mut u16 = std::ptr::null_mut();
    let hr =
        unsafe { SHGetKnownFolderPath(&FOLDERID_Profile, KF_FLAG_DEFAULT as u32, 0, &mut raw) };
    let dir = (hr == 0 && !raw.is_null()).then(|| {
        // 返回的是以 0 结尾的 UTF-16 串
        let len = (0..).take_while(|&i| unsafe { *raw.add(i) } != 0).count();
        PathBuf::from(std::ffi::OsString::from_wide(unsafe {
            std::slice::from_raw_parts(raw, len)
        }))
    });
    // 无论成败都要用 CoTaskMemFree 释放（失败时它是 NULL，释放 NULL 什么都不做）
    unsafe { CoTaskMemFree(raw as *const std::ffi::c_void) };
    dir.filter(|dir| !dir.as_os_str().is_empty())
}

/// 系统记录的用户目录（C1 在 HOME 为空时的退路）：unix 上本用户（getuid）passwd 条目里的
/// 家目录，经 getpwuid_r 读。
#[cfg(unix)]
fn system_user_dir() -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    // 缓冲区不够大时 getpwuid_r 返回 ERANGE，加倍再试（有上限）
    let mut buf = vec![0u8; 1024];
    loop {
        let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
        let mut found: *mut libc::passwd = std::ptr::null_mut();
        let rc = unsafe {
            libc::getpwuid_r(
                libc::getuid(),
                &mut entry,
                buf.as_mut_ptr().cast(),
                buf.len(),
                &mut found,
            )
        };
        if rc == libc::ERANGE && buf.len() < 1 << 20 {
            buf.resize(buf.len() * 2, 0);
            continue;
        }
        if rc != 0 || found.is_null() || entry.pw_dir.is_null() {
            return None;
        }
        let dir = unsafe { std::ffi::CStr::from_ptr(entry.pw_dir) }.to_bytes();
        return (!dir.is_empty()).then(|| PathBuf::from(OsStr::from_bytes(dir)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// C1 的系统退路在本机取得到、是绝对路径（规则本身钉在 仓库的 tests/force_contract.rs）。只读
    /// 这个路径，不碰它下面的任何东西。
    #[test]
    fn the_system_user_dir_is_an_absolute_path() {
        let dir = system_user_dir().expect("系统里应查得到本用户的用户目录");
        assert!(dir.is_absolute(), "{}", dir.display());
    }
}
