//! 文件系统的小工具：原子写，与 home 的访问权限。
//!
//! - 原子写：注册表、期望状态记录、安装状态、settings.json、看护者记录都经这一处写盘。
//!   台账（永久兼容契约 C3）有自己冻结的写者，不走这里。
//! - home 的访问权限：home 里有明文密钥（配置、settings、期望状态记录）、日志与落盘的
//!   请求内容，只该让它的属主访问。aproxy 往 home 里写东西之前先过 [`create_dir_all`]
//!   或原子写，两者都保证 home 是「只让属主访问」的：不存在就这样建出来，属于本用户而
//!   权限过宽就收紧（见 [`secure_home_in`]）。

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
/// unix 上临时文件一创建就是 0600：这些记录可能含密钥（期望状态记录里有拉起规格，其中可能有
/// `--api-key`），rename 之前也不能被别的用户读到。
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    write_atomic_with(path, bytes, false)
}

/// 同 [`write_atomic`]，给含密钥的文件用：Windows 上临时文件创建时就带着只含当前用户、
/// SYSTEM 与 Administrators 的 DACL（见 [`Descriptor::owner_only`]），rename 之后目标文件
/// 带着它；文件默认继承所在目录的 ACL，配置放在别人读得到的目录里时，密钥就跟着可读。
/// unix 上临时文件本来就是 0600，与 [`write_atomic`] 相同。
pub fn write_atomic_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    write_atomic_with(path, bytes, true)
}

fn write_atomic_with(path: &Path, bytes: &[u8], private: bool) -> std::io::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        create_dir_all(parent)?;
    }
    let tmp = temp_path_for(path);
    let written = create_new_private(&tmp, private).and_then(|mut file| {
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

// ---------------------------------------------------------------------------
// home 的访问权限
// ---------------------------------------------------------------------------

/// 建目录，连同缺的上级。`dir` 在本进程的 home 里（或就是 home）时，先保证 home 只让属主
/// 访问（见 [`secure_home_in`]）——往 home 里建目录的地方都用它，不直接用
/// `std::fs::create_dir_all`，否则 home 会按 umask 或上级目录的 ACL 被建出来。
pub fn create_dir_all(dir: impl AsRef<Path>) -> std::io::Result<()> {
    let dir = dir.as_ref();
    ensure_private_home_for(dir);
    std::fs::create_dir_all(dir)
}

/// `path` 在本进程的 home 里（或就是 home）时，保证 home 存在并且只让属主访问。做不到时
/// 只记一条告警：随后的写入照常进行，建不出 home 的话它自己会失败并报出原因。
///
/// 每个进程对一个 home 只做一次（之后只看它还在不在）：权限检查在 Windows 上要读一遍
/// DACL，而实例每隔几秒就写一次记录。
pub fn ensure_private_home_for(path: &Path) {
    static SECURED: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());
    let already = SECURED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .any(|home| path.starts_with(home) && home.exists());
    if already {
        return;
    }
    let Ok(home) = aproxy_ipc::home::resolve_home() else {
        return;
    };
    if !path.starts_with(&home) {
        return;
    }
    match secure_home_in(&home) {
        Ok(_) => {
            let mut secured = SECURED.lock().unwrap_or_else(|e| e.into_inner());
            if !secured.contains(&home) {
                secured.push(home);
            }
        }
        Err(e) => tracing::warn!(
            home = %home.display(),
            error = %e,
            "could not restrict the home directory to its owner"
        ),
    }
}

/// 一个目录或文件现在让谁访问。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Access {
    /// 只有属主（Windows 上另有 SYSTEM 与 Administrators）
    Private,
    /// 还有别人。`owned`：它是否属于当前用户（Windows 上属于 Administrators 组也算），
    /// 不属于的话 aproxy 不改它的权限。`who`：给人看的一句说明——unix 是权限位
    /// （`mode 0755`），Windows 是多出来的账户
    TooOpen { owned: bool, who: String },
}

/// 读 `path` 的访问权限。路径不存在时是 `NotFound` 错误。
pub fn access_of(path: &Path) -> std::io::Result<Access> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = std::fs::metadata(path)?;
        if meta.mode() & 0o077 == 0 {
            return Ok(Access::Private);
        }
        Ok(Access::TooOpen {
            // SAFETY: geteuid 没有前置条件，也不会失败
            owned: meta.uid() == unsafe { libc::geteuid() },
            who: format!("mode {:04o}", meta.mode() & 0o7777),
        })
    }
    #[cfg(windows)]
    {
        windows_access_of(path)
    }
}

/// [`secure_home_in`] 做了什么。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Secured {
    /// home 原来不存在，已建出来，只让属主访问
    Created,
    /// home 属于当前用户而权限过宽，已收紧
    Tightened,
    /// 没有改动：已经只让属主访问，或者它不属于当前用户
    Unchanged,
}

/// 保证 `home` 存在并且只让属主访问：
/// - 不存在：建出来。unix 上权限是 0700；Windows 上是受保护的显式 DACL——当前用户、
///   SYSTEM、Administrators 各自完全控制，子项继承。缺的上级目录按系统默认权限建。
/// - 存在、属于当前用户、别人也能访问：收紧到上面的权限。别的版本或安装脚本建的 home、
///   放在公共目录里的 home，就是这样被纠正的。
/// - 存在而不属于当前用户（例如 `sudo aproxy …` 操作别人的 home）：不动。
pub fn secure_home_in(home: &Path) -> std::io::Result<Secured> {
    let access = match access_of(home) {
        Ok(access) => access,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if let Some(parent) = home.parent() {
                std::fs::create_dir_all(parent)?;
            }
            match create_dir_owner_only(home) {
                Ok(()) => return Ok(Secured::Created),
                // 另一个进程恰好先建了出来：按已有的目录处理
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => access_of(home)?,
                Err(e) => return Err(e),
            }
        }
        Err(e) => return Err(e),
    };
    match access {
        Access::TooOpen { owned: true, .. } => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(home, std::fs::Permissions::from_mode(0o700))?;
            }
            #[cfg(windows)]
            set_dacl(home, &Descriptor::owner_only(true)?)?;
            Ok(Secured::Tightened)
        }
        Access::Private | Access::TooOpen { owned: false, .. } => Ok(Secured::Unchanged),
    }
}

/// 建一个只让属主访问的目录（上级必须已经存在）。权限在创建的那一刻就定下，不是建好再改。
fn create_dir_owner_only(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().mode(0o700).create(dir)
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Storage::FileSystem::CreateDirectoryW;
        let descriptor = Descriptor::owner_only(true)?;
        let attributes = descriptor.attributes();
        let name = wide(dir);
        // SAFETY: name 以 0 结尾；attributes 指向的描述符在 descriptor 存活期间有效
        if unsafe { CreateDirectoryW(name.as_ptr(), &attributes) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
}

/// 路径转成以 0 结尾的 UTF-16。
#[cfg(windows)]
fn wide(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// 由 SDDL 文本建出的安全描述符，`Drop` 时归还。
#[cfg(windows)]
pub(crate) struct Descriptor(windows_sys::Win32::Security::PSECURITY_DESCRIPTOR);

// SAFETY: 描述符是一块建好之后不再改动的堆内存，只被读取（交给系统调用拷贝），在线程之间
// 移动或共享都不会产生数据竞争
#[cfg(windows)]
unsafe impl Send for Descriptor {}
#[cfg(windows)]
unsafe impl Sync for Descriptor {}

#[cfg(windows)]
impl Descriptor {
    pub(crate) fn from_sddl(sddl: &str) -> std::io::Result<Self> {
        use windows_sys::Win32::Security::Authorization::{
            ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
        };
        let sddl: Vec<u16> = sddl.encode_utf16().chain(std::iter::once(0)).collect();
        let mut descriptor = std::ptr::null_mut();
        // SAFETY: sddl 以 0 结尾；descriptor 指向本栈上的指针，成功时由系统填入
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Self(descriptor))
    }

    /// 只让属主访问：受保护的 DACL（不继承上级），里面只有三项——当前用户、SYSTEM、
    /// Administrators，各自完全控制。`inheritable` 给目录用：三项都由子项继承。
    pub(crate) fn owner_only(inheritable: bool) -> std::io::Result<Self> {
        // D:P = 受保护的 DACL；OICI = 文件与子目录都继承；FA = 完全控制
        let flags = if inheritable { "OICI" } else { "" };
        Self::from_sddl(&format!(
            "D:P(A;{flags};FA;;;{})(A;{flags};FA;;;SY)(A;{flags};FA;;;BA)",
            current_user_sid()?
        ))
    }

    /// 创建对象时用的安全属性。返回值里的指针指向本描述符，不得活过它。
    pub(crate) fn attributes(&self) -> windows_sys::Win32::Security::SECURITY_ATTRIBUTES {
        windows_sys::Win32::Security::SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<windows_sys::Win32::Security::SECURITY_ATTRIBUTES>()
                as u32,
            lpSecurityDescriptor: self.0,
            bInheritHandle: 0,
        }
    }
}

#[cfg(windows)]
impl Drop for Descriptor {
    fn drop(&mut self) {
        // SAFETY: 指针来自 ConvertStringSecurityDescriptorToSecurityDescriptorW，只在这里
        // 释放一次
        unsafe {
            windows_sys::Win32::Foundation::LocalFree(self.0 as _);
        }
    }
}

/// 把 `path` 的 DACL 换成 `descriptor` 里的那一份，并且不再继承上级目录的权限。目录上的
/// 可继承项会传给已有的子项。
#[cfg(windows)]
fn set_dacl(path: &Path, descriptor: &Descriptor) -> std::io::Result<()> {
    use windows_sys::Win32::Security::Authorization::{SE_FILE_OBJECT, SetNamedSecurityInfoW};
    use windows_sys::Win32::Security::{
        ACL, DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl,
        PROTECTED_DACL_SECURITY_INFORMATION,
    };
    let name = wide(path);
    let (mut present, mut defaulted) = (0, 0);
    let mut dacl: *mut ACL = std::ptr::null_mut();
    // SAFETY: descriptor 是有效的自相对描述符；dacl 指向它内部，在它存活期间有效；name 以
    // 0 结尾
    unsafe {
        if GetSecurityDescriptorDacl(descriptor.0, &mut present, &mut dacl, &mut defaulted) == 0 {
            return Err(std::io::Error::last_os_error());
        }
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
    }
}

/// SID 写成 `S-1-…` 的文本。
///
/// # Safety
/// `sid` 必须指向一个有效的 SID。
#[cfg(windows)]
unsafe fn sid_text(sid: windows_sys::Win32::Foundation::PSID) -> std::io::Result<String> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
    let mut text: *mut u16 = std::ptr::null_mut();
    // SAFETY: 调用方保证 sid 有效；text 成功时由系统分配，下面归还
    unsafe {
        if ConvertSidToStringSidW(sid, &mut text) == 0 {
            return Err(std::io::Error::last_os_error());
        }
        let out = wide_to_string(text);
        LocalFree(text as _);
        Ok(out)
    }
}

/// [`access_of`] 的 Windows 实现：DACL 里的允许项只该给当前用户、SYSTEM、Administrators
/// （以及只对创建者、属主自己生效的两个占位账户，与同一用户的沙箱化应用）；给了别的账户
/// 就是过宽。没有 DACL 等于
/// 谁都能访问。
#[cfg(windows)]
fn windows_access_of(path: &Path) -> std::io::Result<Access> {
    use windows_sys::Win32::Foundation::{LocalFree, PSID};
    use windows_sys::Win32::Security::Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT};
    use windows_sys::Win32::Security::{
        ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, DACL_SECURITY_INFORMATION, GetAce,
        OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
    };
    /// ACE_HEADER.AceType 的取值：允许访问
    const ACCESS_ALLOWED: u8 = 0;
    const SYSTEM: &str = "S-1-5-18";
    const ADMINISTRATORS: &str = "S-1-5-32-544";
    /// CREATOR OWNER 与 OWNER RIGHTS：只对创建者、属主自己生效，不让别人多出权限
    const OWNER_PLACEHOLDERS: [&str; 2] = ["S-1-3-0", "S-1-3-4"];
    /// 应用容器与能力 SID（`S-1-15-2-…`、`S-1-15-3-…`）：它们指的是同一个用户运行的沙箱化
    /// 应用，不是别的用户；浏览器之类的软件常把它们加在用户目录与临时目录上
    const APP_CONTAINER_PREFIX: &str = "S-1-15-";

    let user = current_user_sid()?;
    let name = wide(path);
    let mut owner: PSID = std::ptr::null_mut();
    let mut dacl: *mut ACL = std::ptr::null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: name 以 0 结尾；owner 与 dacl 成功时指向 descriptor 内部，在下面 LocalFree
    // 之前有效
    unsafe {
        let code = GetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            std::ptr::null_mut(),
            &mut dacl,
            std::ptr::null_mut(),
            &mut descriptor,
        );
        if code != 0 {
            return Err(std::io::Error::from_raw_os_error(code as i32));
        }
        let result = (|| {
            let owner = sid_text(owner)?;
            let owned = owner == user || owner == ADMINISTRATORS;
            let mut others: Vec<String> = Vec::new();
            if dacl.is_null() {
                others.push("everyone (no DACL)".to_string());
            } else {
                for index in 0..u32::from((*dacl).AceCount) {
                    let mut ace = std::ptr::null_mut();
                    if GetAce(dacl, index, &mut ace) == 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    if (*(ace as *const ACE_HEADER)).AceType != ACCESS_ALLOWED {
                        continue;
                    }
                    let allowed = ace as *const ACCESS_ALLOWED_ACE;
                    if (*allowed).Mask == 0 {
                        continue;
                    }
                    let sid = sid_text(std::ptr::addr_of!((*allowed).SidStart) as PSID)?;
                    let expected = sid == user
                        || sid == SYSTEM
                        || sid == ADMINISTRATORS
                        || OWNER_PLACEHOLDERS.contains(&sid.as_str())
                        || sid.starts_with(APP_CONTAINER_PREFIX);
                    let label = account_label(&sid);
                    if !expected && !others.contains(&label) {
                        others.push(label);
                    }
                }
            }
            Ok(if others.is_empty() {
                Access::Private
            } else {
                Access::TooOpen {
                    owned,
                    who: others.join(", "),
                }
            })
        })();
        LocalFree(descriptor as _);
        result
    }
}

/// 给人看的账户名：几个常见的公共账户写名字，其余写 SID 原文。
#[cfg(windows)]
fn account_label(sid: &str) -> String {
    match sid {
        "S-1-1-0" => "Everyone".to_string(),
        "S-1-5-11" => "Authenticated Users".to_string(),
        "S-1-5-32-545" => "Users".to_string(),
        other => other.to_string(),
    }
}

/// 当前进程所属用户的 SID，写成 `S-1-5-21-…` 的文本。
#[cfg(windows)]
pub(crate) fn current_user_sid() -> std::io::Result<String> {
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

/// 新建一个只写的文件，已存在就失败。权限在创建的那一刻就定下——建好再改的话，中间有一段
/// 别人打得开它的窗口，而打开时拿到的权限在句柄上一直有效，之后写进去的内容照样读得到：
/// - unix：恒为 0600；
/// - Windows：`private` 时带只让属主访问的 DACL（见 [`Descriptor::owner_only`]），否则继承
///   所在目录的权限。
pub(crate) fn create_new_private(path: &Path, private: bool) -> std::io::Result<std::fs::File> {
    #[cfg(windows)]
    if private {
        use std::os::windows::io::FromRawHandle;
        use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
        use windows_sys::Win32::Storage::FileSystem::{
            CREATE_NEW, CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_WRITE, FILE_SHARE_DELETE,
            FILE_SHARE_READ, FILE_SHARE_WRITE,
        };
        let descriptor = Descriptor::owner_only(false)?;
        let attributes = descriptor.attributes();
        let name = wide(path);
        // SAFETY: name 以 0 结尾；attributes 指向的描述符在 descriptor 存活期间有效。共享
        // 方式与 std 打开文件时相同（读、写、删都共享），rename 才不受这个句柄妨碍
        let handle = unsafe {
            CreateFileW(
                name.as_ptr(),
                FILE_GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                &attributes,
                CREATE_NEW,
                FILE_ATTRIBUTE_NORMAL,
                0,
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: handle 是刚建出来、只有这里持有的文件句柄
        return Ok(unsafe { std::fs::File::from_raw_handle(handle as _) });
    }
    #[cfg(not(windows))]
    let _ = private;
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

    /// 含密钥的临时文件从建出来的那一刻起就只让属主访问——不是先按目录的权限建、再收紧。
    #[cfg(windows)]
    #[test]
    fn private_temp_file_is_restricted_from_the_moment_it_exists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secret.json.tmp-1-1");
        let file = create_new_private(&path, true).unwrap();
        // 还没有写入任何内容
        let sddl = dacl_sddl(&path).unwrap();
        assert!(sddl.starts_with("D:P"), "{sddl}");
        assert_eq!(sddl.matches("(A;;FA;;;").count(), 3, "{sddl}");
        assert_eq!(access_of(&path).unwrap(), Access::Private);
        drop(file);
        assert_eq!(
            create_new_private(&path, true).unwrap_err().kind(),
            std::io::ErrorKind::AlreadyExists,
            "已有同名文件时失败，不覆盖"
        );
    }

    /// home 不存在时建出来就只让属主访问；属于本用户而权限过宽的被收紧；已经收紧的不动。
    #[cfg(unix)]
    #[test]
    fn home_is_created_private_and_a_loose_one_is_tightened() {
        use std::os::unix::fs::PermissionsExt;
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o7777;
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("nested").join("home");
        assert_eq!(
            access_of(&home).unwrap_err().kind(),
            std::io::ErrorKind::NotFound
        );

        assert_eq!(secure_home_in(&home).unwrap(), Secured::Created);
        assert_eq!(mode(&home), 0o700);
        assert_eq!(access_of(&home).unwrap(), Access::Private);
        assert_eq!(secure_home_in(&home).unwrap(), Secured::Unchanged);

        std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            access_of(&home).unwrap(),
            Access::TooOpen {
                owned: true,
                who: "mode 0755".to_string()
            }
        );
        assert_eq!(secure_home_in(&home).unwrap(), Secured::Tightened);
        assert_eq!(mode(&home), 0o700);
        assert_eq!(secure_home_in(&home).unwrap(), Secured::Unchanged);
    }

    /// 同上，Windows：新建的 home 带受保护的 DACL，三项都由子项继承；给了别的账户权限的
    /// home 被收紧，已有的子项跟着收紧。
    #[cfg(windows)]
    #[test]
    fn home_is_created_private_and_a_loose_one_is_tightened() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("nested").join("home");
        assert_eq!(
            access_of(&home).unwrap_err().kind(),
            std::io::ErrorKind::NotFound
        );

        assert_eq!(secure_home_in(&home).unwrap(), Secured::Created);
        let sddl = dacl_sddl(&home).unwrap();
        assert!(sddl.starts_with("D:P"), "不继承上级目录: {sddl}");
        assert_eq!(sddl.matches("(A;OICI;FA;;;").count(), 3, "{sddl}");
        assert_eq!(access_of(&home).unwrap(), Access::Private);
        let inside = home.join("config.toml");
        std::fs::write(&inside, b"x").unwrap();
        assert_eq!(access_of(&inside).unwrap(), Access::Private);
        assert_eq!(secure_home_in(&home).unwrap(), Secured::Unchanged);

        // 放宽：除了当前用户，再让 Everyone 读（WD = Everyone，FR = 读）
        let loose = Descriptor::from_sddl(&format!(
            "D:(A;OICI;FA;;;{})(A;OICI;FR;;;WD)",
            current_user_sid().unwrap()
        ))
        .unwrap();
        set_dacl(&home, &loose).unwrap();
        assert_eq!(
            access_of(&home).unwrap(),
            Access::TooOpen {
                owned: true,
                who: "Everyone".to_string()
            }
        );
        assert!(matches!(
            access_of(&inside).unwrap(),
            Access::TooOpen { .. }
        ));

        assert_eq!(secure_home_in(&home).unwrap(), Secured::Tightened);
        assert_eq!(access_of(&home).unwrap(), Access::Private);
        assert_eq!(
            access_of(&inside).unwrap(),
            Access::Private,
            "已有的子项跟着收紧"
        );
        assert_eq!(secure_home_in(&home).unwrap(), Secured::Unchanged);

        // 给沙箱化应用的权限不算「别的用户」（AC = ALL APPLICATION PACKAGES）
        let sandboxed = Descriptor::from_sddl(&format!(
            "D:(A;OICI;FA;;;{})(A;OICI;FR;;;AC)",
            current_user_sid().unwrap()
        ))
        .unwrap();
        set_dacl(&home, &sandboxed).unwrap();
        assert_eq!(access_of(&home).unwrap(), Access::Private);
    }
}
