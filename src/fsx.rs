//! 文件系统的小工具：原子写。注册表（.pid）、恢复记录（.restore）、安装状态、settings.json、
//! 看护者的 claim 都经这一处写盘。台账（永久兼容契约 C3）有自己冻结的写者，不走这里。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// 原子地把 `bytes` 写到 `path`：在同目录下 create_new 一个临时文件，写完 `sync_all`，再 rename
/// 覆盖目标。父目录不存在时先建好。
///
/// 为什么要原子：这些文件都有并发的读者（status、看护者、安装器……）。原地写时读者可能读到
/// 写到一半的 JSON，把记录当损坏处理——注册表被误删、claim 被当成「无人在任」而补种出第二个
/// 看护者。rename 在同一卷内是原子的（Windows 上经 MoveFileEx 的替换语义覆盖已存在的文件）。
/// rename 之前先 `sync_all`：不然断电后可能留下一个名字已换、内容却还没落盘的空文件。
///
/// 临时文件名是 `<目标文件名>.tmp-<pid>-<纳秒>`（W2 设计稿 §4.1 的命名），纳秒取当前时间、在
/// 本进程内严格递增：同一目标的并发写者（两个进程、或同一进程的两个线程）各写各的临时文件，
/// 不会把对方写了一半的内容截断后再 rename 出去。create_new 保证万一撞名也只会失败、不会
/// 覆盖别人的临时文件。rename 或写入失败时删掉临时文件；进程恰好在写临时文件与 rename 之间
/// 崩溃时会留下一个临时文件，读者只认目标文件名，不受影响。
///
/// unix 上临时文件一创建就是 0600：这些记录可能含密钥（恢复记录里有启动参数，其中可能有
/// `--api-key`），rename 之前也不能被别的用户读到。
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    write_atomic_with(path, bytes, false)
}

/// 同 [`write_atomic`]，给含密钥的文件用：Windows 上临时文件一创建就换上只含当前用户、
/// SYSTEM 与 Administrators 的 DACL（见 [`restrict_to_owner`]），rename 之后目标文件带着它；
/// 文件默认继承所在目录的 ACL，home 放在别人读得到的目录里时，密钥就跟着可读。unix 上临时
/// 文件本来就是 0600，与 [`write_atomic`] 相同。
pub fn write_atomic_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    write_atomic_with(path, bytes, true)
}

fn write_atomic_with(path: &Path, bytes: &[u8], private: bool) -> std::io::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = temp_path_for(path);
    let written = create_new_private(&tmp).and_then(|mut file| {
        // 先收紧权限、再写内容：内容落进文件的那一刻，别人已经读不到它
        #[cfg(windows)]
        if private {
            restrict_to_owner(&tmp)?;
        }
        #[cfg(not(windows))]
        let _ = private;
        file.write_all(bytes)?;
        file.sync_all()
    });
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// 把文件的 DACL 换成只含三项的显式列表：当前用户、SYSTEM、Administrators，各自完全控制，
/// 并且不再继承上级目录的权限。
#[cfg(windows)]
pub fn restrict_to_owner(path: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1, SE_FILE_OBJECT,
        SetNamedSecurityInfoW,
    };
    use windows_sys::Win32::Security::{
        ACL, DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl,
        PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
    };

    // D:P = 受保护的 DACL（不继承）；每个 ACE 是「允许、完全控制」
    let sddl = format!(
        "D:P(A;;FA;;;{})(A;;FA;;;SY)(A;;FA;;;BA)",
        current_user_sid()?
    );
    let sddl: Vec<u16> = sddl.encode_utf16().chain(std::iter::once(0)).collect();
    let name: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            std::ptr::null_mut(),
        ) == 0
        {
            return Err(std::io::Error::last_os_error());
        }
        let (mut present, mut defaulted) = (0, 0);
        let mut dacl: *mut ACL = std::ptr::null_mut();
        let result = if GetSecurityDescriptorDacl(
            descriptor,
            &mut present,
            &mut dacl,
            &mut defaulted,
        ) == 0
        {
            Err(std::io::Error::last_os_error())
        } else {
            match SetNamedSecurityInfoW(
                name.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                dacl,
                std::ptr::null(),
            ) {
                0 => Ok(()),
                code => Err(std::io::Error::from_raw_os_error(code as i32)),
            }
        };
        LocalFree(descriptor as _);
        result
    }
}

/// 当前进程所属用户的 SID，写成 `S-1-5-21-…` 的文本。
#[cfg(windows)]
fn current_user_sid() -> std::io::Result<String> {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LocalFree};
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    unsafe {
        let mut token: HANDLE = 0;
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(std::io::Error::last_os_error());
        }
        // TOKEN_USER 后面跟着 SID 本身，长度不定：给一块够大的、按 8 字节对齐的缓冲
        let mut buffer = [0u64; 32];
        let mut needed = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            std::mem::size_of_val(&buffer) as u32,
            &mut needed,
        );
        let error = std::io::Error::last_os_error();
        CloseHandle(token);
        if ok == 0 {
            return Err(error);
        }
        let user = &*(buffer.as_ptr() as *const TOKEN_USER);
        let mut text: *mut u16 = std::ptr::null_mut();
        if ConvertSidToStringSidW(user.User.Sid, &mut text) == 0 {
            return Err(std::io::Error::last_os_error());
        }
        let sid = wide_to_string(text);
        LocalFree(text as _);
        Ok(sid)
    }
}

/// 文件的 DACL，写成 SDDL 文本（例如 `D:P(A;;FA;;;S-1-5-…)(A;;FA;;;SY)(A;;FA;;;BA)`）。
/// 诊断与测试用它核对权限。
#[cfg(windows)]
pub fn dacl_sddl(path: &Path) -> std::io::Result<String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::{
        ConvertSecurityDescriptorToStringSecurityDescriptorW, GetNamedSecurityInfoW,
        SDDL_REVISION_1, SE_FILE_OBJECT,
    };
    use windows_sys::Win32::Security::{DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR};

    let name: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        let code = GetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut descriptor,
        );
        if code != 0 {
            return Err(std::io::Error::from_raw_os_error(code as i32));
        }
        let mut text: *mut u16 = std::ptr::null_mut();
        let ok = ConvertSecurityDescriptorToStringSecurityDescriptorW(
            descriptor,
            SDDL_REVISION_1,
            DACL_SECURITY_INFORMATION,
            &mut text,
            std::ptr::null_mut(),
        );
        let error = std::io::Error::last_os_error();
        LocalFree(descriptor as _);
        if ok == 0 {
            return Err(error);
        }
        let sddl = wide_to_string(text);
        LocalFree(text as _);
        Ok(sddl)
    }
}

/// 读一个以 0 结尾的 UTF-16 字符串。
///
/// # Safety
/// `text` 必须指向一个以 0 结尾的 UTF-16 序列。
#[cfg(windows)]
unsafe fn wide_to_string(text: *const u16) -> String {
    let mut len = 0;
    // SAFETY: 调用方保证序列以 0 结尾
    unsafe {
        while *text.add(len) != 0 {
            len += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(text, len))
    }
}

fn create_new_private(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// `<目标文件名>.tmp-<pid>-<纳秒>`，与目标同目录（rename 只在同一卷内原子）
fn temp_path_for(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".tmp-{}-{}", std::process::id(), unique_nanos()));
    path.with_file_name(name)
}

/// 当前时间的纳秒数，在本进程内严格递增（同一纳秒内的两次调用拿到相邻的两个值）
fn unique_nanos() -> u64 {
    static LAST: AtomicU64 = AtomicU64::new(0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut prev = LAST.load(Ordering::Relaxed);
    loop {
        let next = now.max(prev + 1);
        match LAST.compare_exchange_weak(prev, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return next,
            Err(actual) => prev = actual,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_creates_parents_and_replaces_existing_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a").join("b").join("record.json");
        write_atomic(&path, b"first").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"first");
        write_atomic(&path, b"second, longer than the first").unwrap();
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"second, longer than the first"
        );
        // 不留临时文件
        let names: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("record.json")]);
    }

    #[test]
    fn concurrent_writers_never_leave_a_torn_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shared.json");
        write_atomic(&path, b"initial").unwrap();
        let contents: Vec<Vec<u8>> = (0..8u8).map(|i| vec![b'a' + i; 64 * 1024]).collect();
        std::thread::scope(|scope| {
            for content in &contents {
                let path = &path;
                scope.spawn(move || {
                    for _ in 0..20 {
                        // Windows 上 rename 覆盖一个正被另一次 rename 替换的文件可能短暂失败
                        // （共享冲突）；这里只关心「读到的永远是某一次完整的写入」
                        let _ = write_atomic(path, content);
                    }
                });
            }
        });
        let last = std::fs::read(&path).unwrap();
        assert!(
            last == b"initial" || contents.contains(&last),
            "读到的必须是某一次完整写入的内容"
        );
    }

    #[test]
    fn temp_names_are_unique_and_next_to_the_target() {
        let path = Path::new("dir").join("x.json");
        let (a, b) = (temp_path_for(&path), temp_path_for(&path));
        assert_ne!(a, b);
        assert_eq!(a.parent(), path.parent());
        assert!(
            a.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(&format!("x.json.tmp-{}-", std::process::id()))
        );
    }

    #[cfg(unix)]
    #[test]
    fn written_files_are_private_to_the_owner() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secret.json");
        write_atomic(&path, b"{}").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    /// 私有写：Windows 上目标文件的 DACL 只有当前用户、SYSTEM、Administrators 三项，且不继承；
    /// 覆盖已有文件时同样如此。
    #[cfg(windows)]
    #[test]
    fn private_write_leaves_an_explicit_three_entry_dacl() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secret.json");
        // 先有一个普通权限（继承目录）的文件
        std::fs::write(&path, b"old").unwrap();
        let inherited = dacl_sddl(&path).unwrap();
        assert!(
            !inherited.starts_with("D:P"),
            "普通文件继承目录的权限: {inherited}"
        );

        write_atomic_private(&path, b"new").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        let sddl = dacl_sddl(&path).unwrap();
        let user = current_user_sid().unwrap();
        assert!(sddl.starts_with("D:P"), "不继承上级目录: {sddl}");
        // `D:P` 之后可能还带着 AI 之类的标志，ACE 从第一个括号开始
        let mut aces: Vec<&str> = sddl[sddl.find('(').unwrap_or(sddl.len())..]
            .split(['(', ')'])
            .filter(|ace| !ace.is_empty())
            .collect();
        aces.sort();
        assert_eq!(aces.len(), 3, "{sddl}");
        assert!(aces.contains(&"A;;FA;;;SY"), "{sddl}");
        assert!(aces.contains(&"A;;FA;;;BA"), "{sddl}");
        // 剩下的那一条是当前用户。SDDL 给一批众所周知的账户起了两个字母的别名，当前用户恰好
        // 是其中之一时（例如内置的 Administrator 写成 LA）读回来的是别名而不是 SID 原文，
        // 这种情况只确认它是「完全控制」的允许项
        let own: Vec<&str> = aces
            .iter()
            .copied()
            .filter(|ace| !matches!(*ace, "A;;FA;;;SY" | "A;;FA;;;BA"))
            .collect();
        assert_eq!(own.len(), 1, "{sddl}");
        let trustee = own[0]
            .strip_prefix("A;;FA;;;")
            .unwrap_or_else(|| panic!("当前用户那一条应是完全控制的允许项: {sddl}"));
        assert!(
            trustee == user || !trustee.starts_with("S-"),
            "第三条应属于当前用户 {user}: {sddl}"
        );

        // 普通的原子写不动权限模型：新文件继承目录
        let plain = dir.path().join("plain.json");
        write_atomic(&plain, b"x").unwrap();
        assert!(!dacl_sddl(&plain).unwrap().starts_with("D:P"));
    }
}
