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

/// 安装者的全部状态都放在这里：`<home>/install/`。状态文件（state.rs）、备料与
/// 下载暂存（staging.rs）、一键安装脚本的下载暂存都在它下面，此外不写任何地方；
/// skill 目录自己的 `.dl`、`.staging` 例外，留在 `<home>/skills/` 下——它们要与
/// 目标目录同卷才能原子替换。
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

/// 持有中的安装锁，析构即释放。锁是文件句柄上的操作系统锁（Windows 的 LockFileEx、
/// unix 的 flock），持有者进程死亡时由系统自动释放，所以崩溃不会留下打不开的死锁。
#[must_use = "锁在析构时释放，要让它活到 bin/ 改完"]
pub struct InstallLock {
    _file: std::fs::File,
}

/// 拿安装锁，最多等 `wait`：别人持有时每 100 毫秒再试一次，到时仍拿不到 → Err（谁也
/// 不许无限等——强制安装要按退出码规则结束，普通安装落 failed 现场等续作）。
///
/// 同一个进程只能持有一份：锁绑在打开的文件上，同一进程再开一次、再锁一次，会被自己
/// 先前那份挡住。
///
/// Windows 上打开时不给 FILE_SHARE_DELETE：持锁期间别人删不掉这个文件。否则删掉之后
/// 有人新建一个同名文件再锁上，就会有两个持有者。unix 没有对应的办法，靠所有版本都
/// 不删它（强制安装清空 `<home>/install/` 时也跳过它）。
pub fn lock_install_in(home: &Path, wait: std::time::Duration) -> Result<InstallLock, String> {
    let path = lock_path_in(home);
    std::fs::create_dir_all(install_dir_in(home))
        .map_err(|e| format!("{} 创建失败: {e}", install_dir_in(home).display()))?;
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_SHARE_READ | FILE_SHARE_WRITE：别人照样能打开来试锁，但删不掉
        options.share_mode(0x1 | 0x2);
    }
    let file = options
        .open(&path)
        .map_err(|e| format!("安装锁 {} 打开失败: {e}", path.display()))?;
    let deadline = std::time::Instant::now() + wait;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(InstallLock { _file: file }),
            Err(std::fs::TryLockError::WouldBlock) => {}
            Err(std::fs::TryLockError::Error(e)) => {
                return Err(format!("安装锁 {} 加锁失败: {e}", path.display()));
            }
        }
        if std::time::Instant::now() >= deadline {
            return Err(format!(
                "安装锁 {} 被别的进程持有，{} 秒内没有等到（另一个安装正在修改 bin/？）",
                path.display(),
                wait.as_secs()
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
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
