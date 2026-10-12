//! install/upgrade：二进制安装与升级。
//!
//! 设计全文见 `.agents/plan/install-v1.md`（四条铁律：先标记、先下载后
//! rename、ACK 齐了才交换、逐个重启最后删除）。本模块当前承载状态文件
//! 与状态机；下载链条、交换原语、IPC 广播等随计划分步落位。

pub mod announce;
pub mod broadcast;
pub mod download;
pub mod flow;
pub mod force;
pub mod restart;
pub mod skills;
pub mod staging;
pub mod state;
pub mod swap;

use std::path::{Path, PathBuf};

/// 二进制安装的一切状态都只放在这里：`<home>/install/`。状态文件（state.rs）、备料与
/// 下载暂存（staging.rs）、一键安装脚本的下载暂存、unix 的安装宣告（announce.rs）都在
/// 它下面，此外不写任何地方，没有例外。skill 文档的下载暂存（`<home>/skills/` 下的
/// `.dl`、`.staging`）不属于二进制安装：它们要与目标目录同卷才能原子替换；强制安装不碰
/// skill，所以不受影响。
///
/// 收在一处是永久兼容契约 C4 的路径部分：强制安装（不走交接、不依赖任何版本读懂
/// 别的版本的状态）可以随时整目录清空它，而不必认识里面的内容。
///
/// 0.1.0 把状态写在 `run/install.state`、备料放在 `<home>/staging/`，新位置读不到
/// 它们是有意的：0.1.0 留下的安装现场与 0.2.0 起的安装器互不相认，自然隔离。
pub fn install_dir_in(home: &Path) -> PathBuf {
    home.join("install")
}

/// 文件 `file` 是否在目录 `dir` 里或其下。安装的管辖检查（实例的二进制是否在
/// `<home>/bin/` 下）与强制安装第 7 步的报告（哪些进程从 bin 运行）都用它判断。
///
/// 比较的是解析后的真实路径：两边都先 canonicalize，再按 settings::path_match_key 归一
/// （去 `\\?\` 前缀；Windows 上统一分隔符并转小写）。同一个目录可以有好几种写法——
/// Windows 的 8.3 短名（`C:\Users\RUNNER~1\…` 与 `C:\Users\runneradmin\…`）、符号链接与
/// junction、`\\?\` 前缀。进程的镜像路径（QueryFullProcessImageNameW）保留的是启动时
/// 用的写法，经短名启动的进程报出来就带着短名（本机实测）；`<home>` 可能来自展开成长名
/// 的用户目录，也可能来自写着短名的 APROXY_HOME。不解析，两边就会对不上：管辖检查把
/// bin 里的实例当成管辖外而拒绝安装，第 7 步漏报。
///
/// 解析的是 `file` 所在的目录而不是文件本身：镜像文件可能已被改名或删除（交换时让位、
/// unix 上被 rename 覆盖），它所在的目录还在。解析不了的（已不存在、没有权限）退回原样。
pub fn file_is_within(file: &Path, dir: &Path) -> bool {
    let Some(parent) = file.parent() else {
        return false;
    };
    let key = |p: &Path| {
        let real = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        let mut key = crate::settings::path_match_key(&real.display().to_string());
        // 以分隔符收尾再比前缀：`…\bin2` 不能算在 `…\bin` 下
        if !key.ends_with(std::path::MAIN_SEPARATOR) {
            key.push(std::path::MAIN_SEPARATOR);
        }
        key
    };
    key(parent).starts_with(&key(dir))
}

/// 安装锁的文件名：`<home>/install/lock`。
///
/// 永久兼容契约 C4：位置、文件名与「操作系统级排他锁」的语义无论如何都不得改动。
/// 任何版本修改 `<home>/bin/` 期间（换二进制、删让位的旧二进制、写入口兜底脚本）都
/// 持有它；强制安装先按台账结束全部安装者、再拿这把锁，然后才动 bin/。某个版本换了
/// 锁的位置或方式，它与别的版本就会同时改 bin/，互相踩坏对方的交换。
pub const LOCK_FILE: &str = "lock";

/// 安装锁文件：`<home>/install/lock`（见 [`LOCK_FILE`]）。
pub fn lock_path_in(home: &Path) -> PathBuf {
    install_dir_in(home).join(LOCK_FILE)
}

/// 持有中的安装锁，析构即释放。锁是文件句柄上的操作系统锁（见 [`lock_install_in`]），
/// 持有者进程死亡时由系统自动释放，所以崩溃不会留下打不开的死锁。
#[must_use = "锁在析构时释放，要让它活到 bin/ 改完"]
pub struct InstallLock {
    file: std::fs::File,
}

/// 析构时先显式解锁，再随 File 关闭句柄。只关句柄系统也会解锁，但：Windows 上
/// LockFileEx 的文档说那种解锁何时发生「取决于可用的系统资源」，建议显式解锁；unix 上
/// flock 跟着打开的文件走，本进程恰好 fork 出、还没 exec 的子进程手里那份副本会让锁
/// 多留一会儿。显式解锁之后，等着拿锁的下一个安装者不必多等。
impl Drop for InstallLock {
    #[cfg(unix)]
    fn drop(&mut self) {
        use std::os::unix::io::AsRawFd;
        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
    }

    #[cfg(windows)]
    fn drop(&mut self) {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::UnlockFileEx;
        // OVERLAPPED 全零即从第 0 字节起，范围与加锁时相同
        let mut overlapped: windows_sys::Win32::System::IO::OVERLAPPED =
            unsafe { std::mem::zeroed() };
        unsafe {
            UnlockFileEx(
                self.file.as_raw_handle() as isize,
                0,
                u32::MAX,
                u32::MAX,
                &mut overlapped,
            );
        }
    }
}

/// 拿安装锁，最多等 `wait`：打开锁文件、加锁，任何一步不成都每 100 毫秒从头再试一次，
/// 到时仍不成 → Err（谁也不许无限等——强制安装要按退出码规则结束，普通安装落 failed
/// 现场等续作）。
///
/// 永久兼容契约 C4：锁原语无论如何都不得改动——
/// - unix：`flock(fd, LOCK_EX|LOCK_NB)`；
/// - Windows：`LockFileEx(LOCKFILE_EXCLUSIVE_LOCK|LOCKFILE_FAIL_IMMEDIATELY)`，从第 0 字节
///   起、长度 u64::MAX；CreateFile 的共享模式为 FILE_SHARE_READ|FILE_SHARE_WRITE，不带
///   FILE_SHARE_DELETE。
///
/// 两个版本只有用同一种锁、锁同一段字节，才互相看得见。所以直接调用系统接口，不用 std
/// 的 `File::try_lock`：std 文档明说它「目前」对应 flock 与 LockFileEx、以后可能改变，
/// 也没写明 Windows 上锁哪一段。某天它在 unix 上换成 fcntl/OFD 锁（在 Linux 上与 flock
/// 互不排斥），或在 Windows 上换了锁的范围，新旧版本就会同时修改 bin/，踩坏对方的交换。
///
/// 打开也在重试里：Windows 上别的进程（杀毒软件、另一个版本）以不兼容的共享模式开着
/// 这个文件时，打开会暂时失败，那也是「等一会儿」而不是「立即放弃」。
///
/// 同一个进程只能持有一份：锁绑在打开的文件上，同一进程再开一次、再锁一次，会被自己
/// 先前那份挡住。
///
/// Windows 上不给 FILE_SHARE_DELETE：持锁期间别人删不掉这个文件。否则删掉之后有人新建
/// 一个同名文件再锁上，就会有两个持有者。unix 没有对应的办法，靠所有版本都不删它
/// （强制安装清空 `<home>/install/` 时也跳过它）。
pub fn lock_install_in(home: &Path, wait: std::time::Duration) -> Result<InstallLock, String> {
    let dir = install_dir_in(home);
    let path = lock_path_in(home);
    let deadline = std::time::Instant::now() + wait;
    loop {
        let failure = match std::fs::create_dir_all(&dir).and_then(|()| open_lock_file(&path)) {
            Err(e) => format!("安装锁 {} 打开失败: {e}", path.display()),
            Ok(file) => match try_lock_exclusive(&file) {
                Ok(true) => return Ok(InstallLock { file }),
                Ok(false) => format!(
                    "安装锁 {} 被别的进程持有，{} 秒内没有等到（另一个安装正在修改 bin/？）",
                    path.display(),
                    wait.as_secs()
                ),
                Err(e) => format!("安装锁 {} 加锁失败: {e}", path.display()),
            },
        };
        if std::time::Instant::now() >= deadline {
            return Err(failure);
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// 打开（必要时新建）锁文件，读写；Windows 上共享模式见 [`lock_install_in`]。
fn open_lock_file(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};
        options.share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE);
    }
    options.open(path)
}

/// 不等待地加排他锁（永久兼容契约 C4 的 unix 原语）：Ok(true) 拿到了，Ok(false) 别人
/// 持有（或被信号打断，下一轮再试），Err 是加锁本身出错。
#[cfg(unix)]
fn try_lock_exclusive(file: &std::fs::File) -> std::io::Result<bool> {
    use std::os::unix::io::AsRawFd;
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(true);
    }
    let err = std::io::Error::last_os_error();
    match err.raw_os_error() {
        Some(libc::EWOULDBLOCK) | Some(libc::EINTR) => Ok(false),
        _ => Err(err),
    }
}

/// 不等待地加排他锁（永久兼容契约 C4 的 Windows 原语）：Ok(true) 拿到了，Ok(false) 别人
/// 持有，Err 是加锁本身出错。
#[cfg(windows)]
fn try_lock_exclusive(file: &std::fs::File) -> std::io::Result<bool> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::ERROR_LOCK_VIOLATION;
    use windows_sys::Win32::Storage::FileSystem::{
        LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY, LockFileEx,
    };
    // OVERLAPPED 全零：Offset 与 OffsetHigh 为 0，即从第 0 字节锁起；长度 u64::MAX 拆成
    // 低 32 位与高 32 位两个参数
    let mut overlapped: windows_sys::Win32::System::IO::OVERLAPPED = unsafe { std::mem::zeroed() };
    let locked = unsafe {
        LockFileEx(
            file.as_raw_handle() as isize,
            LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
            0,
            u32::MAX,
            u32::MAX,
            &mut overlapped,
        )
    };
    if locked != 0 {
        return Ok(true);
    }
    let err = std::io::Error::last_os_error();
    if err.raw_os_error() == Some(ERROR_LOCK_VIOLATION as i32) {
        Ok(false)
    } else {
        Err(err)
    }
}

/// 可安装版本的下限。0.1.x 与 0.2.0 起的版本互不兼容（控制协议、安装状态的位置、
/// `--version` 的格式都变了）：0.2.0 起的安装器驱动不了 0.1.x 的二进制，也联系不上
/// 0.1.x 的实例，装回 0.1.x 只会在交换二进制之后半途坏掉。所以在线安装与 `--from`
/// 都拒绝低于它的目标，`--allow-downgrade` 也越不过它。
///
/// 按「主.次.修订」比较：0.2.0 的预发布（alpha/beta/rc，以及开发构建自报的
/// 0.2.0-alpha.0）属于 0.2.0 这条线。按 semver 它们低于 0.2.0，直接比较 Version
/// 会把它们一起拒掉。
pub const INSTALL_FLOOR: semver::Version = semver::Version::new(0, 2, 0);

/// 目标版本不低于 [`INSTALL_FLOOR`]；低于时的错误说明为什么、怎样回到 0.1.x。
pub fn check_floor(target: &semver::Version) -> Result<(), String> {
    let floor = &INSTALL_FLOOR;
    if (target.major, target.minor, target.patch) >= (floor.major, floor.minor, floor.patch) {
        return Ok(());
    }
    Err(format!(
        "目标版本 {target} 低于 {floor}，不能安装：0.1.x 与 0.2.0 起的版本互不兼容（控制协议与磁盘布局都变了），\
         --allow-downgrade 也越不过这条下限。要回到 0.1.x：先用当前版本执行 `aproxy stop all` 停掉全部实例，\
         删掉 bin 目录里的 aproxy 二进制，再用 0.1.x 的安装脚本重新安装"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floor_rejects_0_1_x_and_admits_the_whole_0_2_0_line() {
        let v = |s: &str| semver::Version::parse(s).unwrap();
        for below in ["0.1.0", "0.1.99", "0.1.0-alpha.17", "0.0.1"] {
            let err = check_floor(&v(below)).unwrap_err();
            assert!(
                err.contains("0.1.x") && err.contains("--allow-downgrade"),
                "{err}"
            );
        }
        // 0.2.0 的预发布（含开发构建的 alpha.0 与最小的 0.2.0-0）都在线内
        for ok in [
            "0.2.0-0",
            "0.2.0-alpha.0",
            "0.2.0-rc.1",
            "0.2.0",
            "0.3.0",
            "1.0.0",
        ] {
            assert!(check_floor(&v(ok)).is_ok(), "{ok}");
        }
        assert!(check_floor(&v(env!("CARGO_PKG_VERSION"))).is_ok());
    }

    #[test]
    fn file_is_within_compares_directories_not_spellings() {
        let home = tempfile::tempdir().unwrap();
        let bin = home.path().join("bin");
        let bin2 = home.path().join("bin2");
        std::fs::create_dir_all(bin.join("nested")).unwrap();
        std::fs::create_dir_all(&bin2).unwrap();
        // 文件本身不必存在：比较的是它所在的目录
        assert!(file_is_within(&bin.join("aproxy.exe"), &bin));
        assert!(file_is_within(&bin.join("nested").join("x"), &bin));
        assert!(
            !file_is_within(&bin2.join("aproxy.exe"), &bin),
            "bin2 不在 bin 下"
        );
        assert!(!file_is_within(&home.path().join("aproxy.exe"), &bin));
        // `..` 绕一圈回来仍是同一个目录
        let detour = bin.join("..").join("bin").join("aproxy.exe");
        assert!(file_is_within(&detour, &bin), "{}", detour.display());
        // 目录不存在时按原样比较
        let gone = home.path().join("gone");
        assert!(file_is_within(&gone.join("x"), &gone));
    }

    /// Windows 的 8.3 短名：同一个目录的短名写法与长名写法必须判成同一个目录（CI 的临时
    /// 目录就是 `C:\Users\RUNNER~1\…`，经短名启动的进程镜像路径也带着短名）。
    #[cfg(windows)]
    #[test]
    fn file_is_within_treats_8dot3_short_names_as_the_same_directory() {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        let short = |p: &Path| -> PathBuf {
            let wide: Vec<u16> = p.as_os_str().encode_wide().chain(Some(0)).collect();
            let mut buf = vec![0u16; 1024];
            let len = unsafe {
                windows_sys::Win32::Storage::FileSystem::GetShortPathNameW(
                    wide.as_ptr(),
                    buf.as_mut_ptr(),
                    buf.len() as u32,
                )
            } as usize;
            assert!(
                len > 0 && len < buf.len(),
                "GetShortPathNameW 失败: {}",
                p.display()
            );
            PathBuf::from(std::ffi::OsString::from_wide(&buf[..len]))
        };
        let home = tempfile::tempdir().unwrap();
        // 名字长于 8 个字符、带连字符：卷上开着 8.3 短名生成时，它会得到 `APROXY~1` 式的短名
        let long_bin = home.path().join("aproxy-managed-bin");
        let long_other = home.path().join("aproxy-managed-bin-other");
        std::fs::create_dir_all(&long_bin).unwrap();
        std::fs::create_dir_all(&long_other).unwrap();
        let short_bin = short(&long_bin);
        let short_other = short(&long_other);
        if short_bin
            .as_os_str()
            .eq_ignore_ascii_case(long_bin.as_os_str())
        {
            eprintln!(
                "跳过：{} 所在的卷没有生成 8.3 短名（GetShortPathNameW 返回的就是长名），造不出短名写法",
                long_bin.display()
            );
            return;
        }
        let ctx = format!(
            "长名 {}，短名 {}；另一个目录 长名 {}，短名 {}",
            long_bin.display(),
            short_bin.display(),
            long_other.display(),
            short_other.display()
        );
        for file_dir in [&long_bin, &short_bin] {
            for dir in [&long_bin, &short_bin] {
                assert!(
                    file_is_within(&file_dir.join("aproxy.exe"), dir),
                    "{} 应在 {} 里（{ctx}）",
                    file_dir.display(),
                    dir.display()
                );
            }
            for other in [&long_other, &short_other] {
                assert!(
                    !file_is_within(&other.join("aproxy.exe"), file_dir),
                    "{} 不应在 {} 里（{ctx}）",
                    other.display(),
                    file_dir.display()
                );
            }
        }
    }
}
