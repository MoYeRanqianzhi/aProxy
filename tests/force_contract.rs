//! 永久兼容契约 C1–C8（强制安装）的钉子：台账的位置与 JSON 格式、三个平台的启动
//! 标记、唯一中间名、安装锁的位置、`--version` 首行、发布清单、命令行写法与退出码。
//! 设计全文见 `.agents/plan/release-0.2.0.md` 的「W2b 强制安装：永久兼容契约」一节。
//!
//! **改这些测试就是破坏契约。** 从 0.2.0 起，任意版本的 `aproxy install --force` 都要能
//! 把任意版本装进同一个 home、停掉任意版本启动的进程；这里每一条断言都是别的版本已经
//! 写进二进制、收不回来的假设。某条断言挡住了你的改动，该改的是你的改动，不是断言——
//! 契约只许追加可选字段（追加时在这里补新的断言，旧的一条不动）。
//!
//! 隔离：会运行 aproxy 的测试都把 APROXY_HOME 设为临时目录（见 [`temp_home`]）。

use aproxy::install::force::{self, Manifest, VersionLine};
use aproxy::ledger::{self, LedgerEntry};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// 新建一个临时 home，并确认它确实在临时目录里、不是用户真正在用的 `~/.aproxy`：
/// 这些测试会执行强制安装，home 指错了就会结束用户的进程、替换用户的二进制。
fn temp_home() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let real = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let home = real(dir.path());
    assert!(
        home.starts_with(real(&std::env::temp_dir())),
        "测试 home 必须在临时目录里: {}",
        home.display()
    );
    if let Some(user_home) = dirs::home_dir() {
        assert!(
            !home.starts_with(real(&user_home.join(".aproxy"))),
            "测试 home 绝不能是用户的 ~/.aproxy: {}",
            home.display()
        );
    }
    dir
}

/// 在隔离的 home 里运行 aproxy（标准输入为空、不是终端）。
fn aproxy_in(home: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_aproxy"))
        .args(args)
        .env("APROXY_HOME", home)
        .env("NO_PROXY", "127.0.0.1,localhost")
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

// ---------------------------------------------------------------------------
// C1、C2、C4：位置与名字
// ---------------------------------------------------------------------------

#[test]
fn c1_c2_c4_locations_and_names() {
    let home = Path::new("/h");
    // C2：bin 的位置与二进制的名字
    let exe = if cfg!(windows) {
        "aproxy.exe"
    } else {
        "aproxy"
    };
    assert_eq!(aproxy::install::swap::bin_dir_in(home), home.join("bin"));
    assert_eq!(
        aproxy::install::swap::bin_path_in(home),
        home.join("bin").join(exe)
    );
    // C4：安装状态目录与安装锁
    assert_eq!(aproxy::install::install_dir_in(home), home.join("install"));
    assert_eq!(aproxy::install::LOCK_FILE, "lock");
    assert_eq!(
        aproxy::install::lock_path_in(home),
        home.join("install").join("lock")
    );
    // C3：台账的位置与文件名
    assert_eq!(ledger::LEDGER_DIR, "ledger");
    assert_eq!(ledger::ledger_dir_in(home), home.join("ledger"));
    assert_eq!(
        ledger::entry_path_in(home, 4242),
        home.join("ledger").join("4242.json")
    );
}

/// C1：home 的解析规则（纯函数 `home_from`、`user_dir_from`，环境变量由测试直接给值——
/// 不改本进程的环境，也就不可能把 home 指到用户真正在用的目录）。
#[test]
fn c1_home_resolution_rules() {
    use aproxy::settings::{HOME_ENV, USER_DIR_ENV, home_from, user_dir_from};
    use std::ffi::OsStr;
    assert_eq!(HOME_ENV, "APROXY_HOME");
    assert_eq!(
        USER_DIR_ENV,
        if cfg!(windows) { "USERPROFILE" } else { "HOME" }
    );

    let abs = if cfg!(windows) {
        r"C:\aproxy-home"
    } else {
        "/aproxy-home"
    };
    let user = PathBuf::from(if cfg!(windows) {
        r"C:\Users\someone"
    } else {
        "/home/someone"
    });
    let default = user.join(".aproxy");
    let resolve = |value: Option<&str>| home_from(value.map(OsStr::new), || Some(user.clone()));
    // APROXY_HOME 设了就不该去取用户目录
    let never = || -> Option<PathBuf> { panic!("APROXY_HOME 设了就不该去取用户目录") };

    // 设了：原样使用，不 trim（尾随的空白也是路径的一部分）
    assert_eq!(
        home_from(Some(OsStr::new(abs)), never),
        Ok(PathBuf::from(abs))
    );
    let trailing = format!("{abs} ");
    assert_eq!(
        home_from(Some(OsStr::new(&trailing)), never),
        Ok(PathBuf::from(&trailing))
    );
    // 按原始字节读：不是合法 UTF-8 也照用
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let raw = OsStr::from_bytes(b"/aproxy-\xff-home");
        assert_eq!(home_from(Some(raw), never), Ok(PathBuf::from(raw)));
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;
        // 落单的代理项 0xD800：合法的 Windows 文件名字符，却不是合法的 UTF-16/UTF-8
        let mut wide: Vec<u16> = r"C:\aproxy-".encode_utf16().collect();
        wide.push(0xD800);
        let raw = std::ffi::OsString::from_wide(&wide);
        assert!(raw.to_str().is_none());
        assert_eq!(home_from(Some(&raw), never), Ok(PathBuf::from(&raw)));
    }

    // 没设、为空、只含空白：视为未设 → <用户目录>/.aproxy
    for unset in [None, Some(""), Some(" "), Some("\t \n"), Some("\u{3000}")] {
        assert_eq!(resolve(unset), Ok(default.clone()), "{unset:?}");
    }
    // 不是绝对路径：报错（要读 home 的命令一律拒绝运行）。`~` 不展开；开头的空白让它成了
    // 相对路径
    let leading = format!(" {abs}");
    let mut relative = vec!["aproxy-home", "./aproxy-home", ".", "~/.aproxy", &leading];
    // Windows 上有根无盘符、有盘符无根都不是绝对路径
    if cfg!(windows) {
        relative.extend([r"\aproxy-home", "C:aproxy-home"]);
    }
    for value in relative {
        let err = resolve(Some(value)).expect_err(value);
        assert!(err.contains(HOME_ENV), "{err}");
    }
    // 用户目录也取不到：报错
    assert!(home_from(None, || None).is_err());

    // 用户目录：环境变量非空就原样用它（只含空白也算非空），否则问系统
    let system = PathBuf::from(if cfg!(windows) {
        r"C:\Users\from-system"
    } else {
        "/home/from-system"
    });
    let ask_system = || Some(system.clone());
    let refuse = || -> Option<PathBuf> { panic!("环境变量非空时不该去问系统") };
    assert_eq!(
        user_dir_from(Some(user.as_os_str()), refuse),
        Some(user.clone())
    );
    assert_eq!(
        user_dir_from(Some(OsStr::new(" ")), refuse),
        Some(PathBuf::from(" "))
    );
    assert_eq!(
        user_dir_from(Some(OsStr::new("")), ask_system),
        Some(system.clone())
    );
    assert_eq!(user_dir_from(None, ask_system), Some(system.clone()));
}

/// C1：APROXY_HOME 不是绝对路径时，要读 home 的命令一律拒绝运行、什么都不建；`--version`
/// 不受影响；强制安装以 10 结束（什么都没动）。在一个空的临时目录里运行：一旦哪个命令把
/// 相对路径当成了 home，就会在这里建出东西来。用户目录的环境变量也指向这个临时目录，
/// 即使实现错把它当成「未设」，落脚的也是临时目录，碰不到用户真正在用的 home。
#[test]
fn c1_a_relative_aproxy_home_is_refused() {
    let cwd = temp_home();
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_aproxy"))
            .args(args)
            .current_dir(cwd.path())
            .env("APROXY_HOME", "relative-home")
            .env("USERPROFILE", cwd.path())
            .env("HOME", cwd.path())
            .env("NO_PROXY", "127.0.0.1,localhost")
            .stdin(Stdio::null())
            .output()
            .unwrap()
    };

    let version = run(&["--version"]);
    assert_eq!(version.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&version.stdout).starts_with("aproxy v"));

    // 报错退出，而不是崩溃（进程入口就核对，不等到第一次读 home 时 panic）
    let status = run(&["status"]);
    let status_err = String::from_utf8_lossy(&status.stderr);
    assert_ne!(status.status.code(), Some(0), "{status_err}");
    assert!(status_err.contains("APROXY_HOME"), "{status_err}");
    assert!(!status_err.contains("panicked"), "{status_err}");

    let forced = run(&["install", "--force", "--from", "no-such-file", "--yes"]);
    let forced_err = String::from_utf8_lossy(&forced.stderr);
    assert_eq!(
        forced.status.code(),
        Some(force::EXIT_UNCHANGED),
        "{forced_err}"
    );
    assert!(forced_err.contains("APROXY_HOME"), "{forced_err}");

    let created: Vec<_> = std::fs::read_dir(cwd.path())
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    assert!(created.is_empty(), "相对路径不得被当成 home: {created:?}");
}

#[test]
fn c2_unique_intermediate_names() {
    use aproxy::install::swap::{OLD_PREFIX, TMP_PREFIX, is_residue_name, unique_name};
    assert_eq!(OLD_PREFIX, "aproxy.old-");
    assert_eq!(TMP_PREFIX, "aproxy.tmp-");
    // `<前缀><纳秒时间戳>-<pid>[.exe]`
    for prefix in [OLD_PREFIX, TMP_PREFIX] {
        let name = unique_name(prefix);
        let rest = name.strip_prefix(prefix).unwrap();
        let rest = if cfg!(windows) {
            rest.strip_suffix(".exe").expect(&name)
        } else {
            rest
        };
        let (nanos, pid) = rest.split_once('-').expect(&name);
        // 19 位十进制纳秒：位数固定，字典序就是时间序——入口兜底脚本（aproxy.bat 与 sh）
        // 跨版本按字典序挑最新的 aproxy.old-*，改成微秒或不补齐位数都会挑错
        assert_eq!(nanos.len(), 19, "{name}");
        assert!(nanos.bytes().all(|b| b.is_ascii_digit()), "{name}");
        assert_eq!(pid, std::process::id().to_string(), "{name}");
        assert!(is_residue_name(&name), "{name}");
    }
    // 别的版本留下的中间文件，任何版本都认得、清得掉
    for name in [
        "aproxy.old-1760256000123456789-4242.exe",
        "aproxy.tmp-1760256000123456789-4242",
    ] {
        assert!(is_residue_name(name), "{name}");
    }
    for name in ["aproxy.exe", "aproxy", "aproxy.bat", "aproxy.old.exe"] {
        assert!(!is_residue_name(name), "{name}");
    }
}

// ---------------------------------------------------------------------------
// C4：安装锁的原语。测试自己用系统接口加锁、与 `lock_install_in` 互相试探：两个版本只有
// 用同一种锁、锁同一段字节，才互相看得见
// ---------------------------------------------------------------------------

/// 测试自己打开锁文件，读写；Windows 上共享模式与契约相同（READ|WRITE）。
fn open_lock_natively(path: &Path) -> std::fs::File {
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(0x1 | 0x2);
    }
    options.open(path).unwrap()
}

/// unix：不等待地 `flock`，`shared` 为 LOCK_SH，否则 LOCK_EX。返回是否拿到。
#[cfg(unix)]
fn flock_natively(file: &std::fs::File, shared: bool) -> bool {
    use std::os::unix::io::AsRawFd;
    let kind = if shared { libc::LOCK_SH } else { libc::LOCK_EX };
    unsafe { libc::flock(file.as_raw_fd(), kind | libc::LOCK_NB) == 0 }
}

/// Windows：不等待地 `LockFileEx` 锁 `[offset, offset + len)`，`shared` 时不带
/// LOCKFILE_EXCLUSIVE_LOCK。返回是否拿到。
#[cfg(windows)]
fn lock_file_ex_natively(file: &std::fs::File, shared: bool, offset: u64, len: u64) -> bool {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY, LockFileEx,
    };
    use windows_sys::Win32::System::IO::{OVERLAPPED, OVERLAPPED_0, OVERLAPPED_0_0};
    let mut overlapped = OVERLAPPED {
        Internal: 0,
        InternalHigh: 0,
        Anonymous: OVERLAPPED_0 {
            Anonymous: OVERLAPPED_0_0 {
                Offset: offset as u32,
                OffsetHigh: (offset >> 32) as u32,
            },
        },
        hEvent: 0,
    };
    let flags = if shared { 0 } else { LOCKFILE_EXCLUSIVE_LOCK };
    unsafe {
        LockFileEx(
            file.as_raw_handle() as isize,
            flags | LOCKFILE_FAIL_IMMEDIATELY,
            0,
            len as u32,
            (len >> 32) as u32,
            &mut overlapped,
        ) != 0
    }
}

/// C4：原生持锁时 `lock_install_in` 拿不到；`lock_install_in` 持锁时原生拿不到。试探用
/// 共享锁：实现若不是排他锁，就会与共享锁共存。Windows 上分别只试第 0 个字节与最后一个
/// 字节：实现锁的范围必须从 0 起、长到 u64::MAX，两头都覆盖到。
#[test]
fn c4_lock_primitive_is_flock_or_lock_file_ex() {
    use aproxy::install::{lock_install_in, lock_path_in};
    let home = temp_home();
    let home = home.path();
    let path = lock_path_in(home);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let briefly = std::time::Duration::from_millis(300);
    let surely = std::time::Duration::from_secs(5);

    // 原生持锁 → lock_install_in 拿不到
    #[cfg(unix)]
    {
        let native = open_lock_natively(&path);
        assert!(flock_natively(&native, true));
        assert!(
            lock_install_in(home, briefly).is_err(),
            "原生 flock(LOCK_SH) 持锁时，lock_install_in 不得拿到锁"
        );
    }
    #[cfg(windows)]
    for (offset, len) in [(0, 1), (u64::MAX - 1, 1)] {
        let native = open_lock_natively(&path);
        assert!(lock_file_ex_natively(&native, true, offset, len));
        assert!(
            lock_install_in(home, briefly).is_err(),
            "原生 LockFileEx 共享锁住第 {offset} 字节时，lock_install_in 不得拿到锁"
        );
    }

    // lock_install_in 持锁 → 原生拿不到
    let held = lock_install_in(home, surely).expect("没人持锁时应拿到");
    // Windows：共享模式不带 FILE_SHARE_DELETE，持锁期间删不掉锁文件
    #[cfg(windows)]
    assert!(
        std::fs::remove_file(&path).is_err(),
        "持锁期间锁文件不得被删"
    );
    // 共享模式带 READ|WRITE：别的版本照样打得开、能来试锁
    let native = open_lock_natively(&path);
    #[cfg(unix)]
    assert!(
        !flock_natively(&native, true),
        "lock_install_in 持锁时，原生 flock(LOCK_SH) 不得拿到锁"
    );
    #[cfg(windows)]
    for (offset, len) in [(0, 1), (u64::MAX - 1, 1)] {
        assert!(
            !lock_file_ex_natively(&native, true, offset, len),
            "lock_install_in 持锁时，原生 LockFileEx 不得锁住第 {offset} 字节"
        );
    }
    drop(held);

    // 释放之后原生排他锁拿得到
    #[cfg(unix)]
    assert!(flock_natively(&native, false));
    #[cfg(windows)]
    assert!(lock_file_ex_natively(&native, false, 0, u64::MAX));
}

/// C4：打开锁文件也在重试里。Windows 上别的进程（杀毒软件、另一个版本）以不共享的方式
/// 开着它时，打开会失败——那也要等，不能立即放弃。
#[cfg(windows)]
#[test]
fn c4_lock_waits_while_the_lock_file_is_opened_without_sharing() {
    use std::os::windows::fs::OpenOptionsExt;
    let home = temp_home();
    let path = aproxy::install::lock_path_in(home.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let unshared = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0)
        .open(&path)
        .unwrap();
    let release = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(500));
        drop(unshared);
    });
    let lock = aproxy::install::lock_install_in(home.path(), std::time::Duration::from_secs(10));
    release.join().unwrap();
    assert!(lock.is_ok(), "{:?}", lock.err());
}

// ---------------------------------------------------------------------------
// C3：台账
// ---------------------------------------------------------------------------

#[test]
fn c3_ledger_entry_json() {
    assert_eq!(ledger::LEDGER_FORMAT, 1);
    assert_eq!(ledger::ROLE_INSTALLER, "installer");
    assert_eq!(ledger::ROLE_WATCHDOG, "watchdog");
    assert_eq!(ledger::ROLE_INSTANCE, "instance");
    assert_eq!(ledger::ROLE_DESKTOP, "desktop");

    // 全部字段：落盘就是这一行，字段名、顺序、类型都冻结
    let full = r#"{"ledger":1,"pid":4242,"start":"w:133730123456789012","role":"instance","version":"0.2.0","port":12345,"label":"C:\\Users\\me\\.aproxy\\config.toml","exe":"C:\\Users\\me\\.aproxy\\bin\\aproxy.exe"}"#;
    let entry = LedgerEntry::parse(full).expect("完整条目可解析");
    assert_eq!(
        entry,
        LedgerEntry {
            ledger: 1,
            pid: 4242,
            start: "w:133730123456789012".into(),
            role: "instance".into(),
            version: "0.2.0".into(),
            port: Some(12345),
            label: Some(r"C:\Users\me\.aproxy\config.toml".into()),
            exe: Some(r"C:\Users\me\.aproxy\bin\aproxy.exe".into()),
            config: None,
        }
    );
    assert_eq!(entry.to_json(), full);

    // 追加的可选字段 config（2026-10-12，实例配置文件的绝对路径）：写在 exe 之后
    let with_config = r#"{"ledger":1,"pid":4242,"start":"w:133730123456789012","role":"instance","version":"0.2.0","port":12345,"label":"main","config":"C:\\Users\\me\\.aproxy\\config.toml"}"#;
    let entry = LedgerEntry::parse(with_config).expect("带 config 的条目可解析");
    assert_eq!(
        entry.config.as_deref(),
        Some(r"C:\Users\me\.aproxy\config.toml")
    );
    assert_eq!(entry.to_json(), with_config);

    // 可选字段缺省时不写出，也能读
    let minimal = r#"{"ledger":1,"pid":7,"start":"l:6f1c2d3e-0000-4000-8000-000000000001:123456","role":"watchdog","version":"0.2.0"}"#;
    let entry = LedgerEntry::parse(minimal).expect("最小条目可解析");
    assert_eq!(entry.port, None);
    assert_eq!(entry.label, None);
    assert_eq!(entry.exe, None);
    assert_eq!(entry.config, None);
    assert_eq!(entry.to_json(), minimal);
}

#[test]
fn c3_readers_tolerate_the_future_and_skip_garbage() {
    // 以后的版本追加的字段与角色：今天的读者照样读得出来，不认识的角色排在最后结束
    let future = r#"{"ledger":1,"pid":9,"start":"m:1760256000.5","role":"tray","version":"9.0.0","port":1,"since":"2031-01-01","nested":{"a":[1,2]}}"#;
    let entry = LedgerEntry::parse(future).expect("带未知字段与未知角色的条目可解析");
    assert_eq!(entry.role, "tray");
    assert_eq!(entry.port, Some(1));
    let order = |role: &str| {
        LedgerEntry {
            role: role.into(),
            ..entry.clone()
        }
        .stop_order()
    };
    // 安装者 → 看护者 → 实例 → 桌面界面 → 不认识的角色
    assert!(order("installer") < order("watchdog"));
    assert!(order("watchdog") < order("instance"));
    assert!(order("instance") < order("desktop"));
    assert!(order("desktop") < order("tray"));

    // 缺必填字段、类型不对、不是 JSON：解析不了（读者跳过，不当成条目）
    for bad in [
        r#"{"pid":1,"start":"w:1","role":"instance","version":"0.2.0"}"#,
        r#"{"ledger":1,"pid":"1","start":"w:1","role":"instance","version":"0.2.0"}"#,
        r#"{"ledger":1,"pid":1,"role":"instance","version":"0.2.0"}"#,
        "not json",
    ] {
        assert_eq!(LedgerEntry::parse(bad), None, "{bad}");
    }

    // 读者只看 `*.json`、只看本平台写下的条目（`start` 的前缀是本平台的）：写到一半的
    // `<pid>.json.tmp`、解析不了的文件、别的平台写的条目（WSL 与 Windows 共用了 home）
    // 都跳过，且不删——那边的 pid 在这边没有意义，按它核验只会把那边活着的进程当成死的
    let own = if cfg!(windows) {
        ledger::START_WINDOWS
    } else if cfg!(target_os = "macos") {
        ledger::START_MACOS
    } else {
        ledger::START_LINUX
    };
    let foreign = [
        ledger::START_WINDOWS,
        ledger::START_LINUX,
        ledger::START_MACOS,
    ]
    .into_iter()
    .find(|p| *p != own)
    .unwrap();
    let with_start = |pid: u32, start: &str| {
        future
            .replace("\"pid\":9", &format!("\"pid\":{pid}"))
            .replace("m:1760256000.5", start)
    };
    let home = temp_home();
    let dir = ledger::ledger_dir_in(home.path());
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("9.json"), with_start(9, &format!("{own}1"))).unwrap();
    std::fs::write(dir.join("10.json.tmp"), with_start(10, &format!("{own}1"))).unwrap();
    std::fs::write(dir.join("11.json"), "{ half").unwrap();
    std::fs::write(dir.join("12.json"), with_start(12, &format!("{foreign}1"))).unwrap();
    let found = ledger::read_all_in(home.path());
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].entry.pid, 9);
    for kept in ["10.json.tmp", "11.json", "12.json"] {
        assert!(dir.join(kept).exists(), "{kept}");
    }
}

#[test]
fn c3_start_marker_formats() {
    assert_eq!(ledger::START_WINDOWS, "w:");
    assert_eq!(ledger::START_LINUX, "l:");
    assert_eq!(ledger::START_MACOS, "m:");
    // 本平台的推导方式：本进程的标记现算两次相同，且是冻结的形状
    #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
    {
        let marker = ledger::start_marker(std::process::id()).expect("本进程的启动标记可查");
        assert_eq!(
            ledger::start_marker(std::process::id()).as_deref(),
            Some(marker.as_str())
        );
        let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
        // Windows：`w:<创建时间 FILETIME 的 64 位无符号十进制>`。测试自己调用
        // GetProcessTimes 拼出期望值：换成 Unix 毫秒、高低位颠倒，都过不了这里
        #[cfg(windows)]
        {
            use windows_sys::Win32::Foundation::FILETIME;
            use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
            assert!(marker.strip_prefix("w:").is_some_and(digits), "{marker}");
            let zero = FILETIME {
                dwLowDateTime: 0,
                dwHighDateTime: 0,
            };
            let (mut creation, mut exit, mut kernel, mut user) = (zero, zero, zero, zero);
            let ok = unsafe {
                GetProcessTimes(
                    GetCurrentProcess(),
                    &mut creation,
                    &mut exit,
                    &mut kernel,
                    &mut user,
                )
            };
            assert_ne!(ok, 0, "GetProcessTimes 失败");
            let filetime =
                (u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime);
            assert_eq!(marker, format!("w:{filetime}"));
        }
        // Linux：`l:<boot_id 原文>:<stat 第 22 字段>`，boot_id 是带连字符的小写 UUID。测试
        // 自己解析 /proc/self/stat 拼出期望值：换成第 21 字段、除以 CLK_TCK，都过不了这里
        #[cfg(target_os = "linux")]
        {
            let boot_id = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").unwrap();
            let stat = std::fs::read_to_string("/proc/self/stat").unwrap();
            // 第 2 字段 comm 在括号里、可能含空格与括号：从最后一个 `)` 之后切分，切出来的
            // 第一个是第 3 字段，第 22 字段就是其后第 19 个（从 0 数）
            let fields: Vec<&str> = stat[stat.rfind(')').unwrap() + 1..]
                .split_whitespace()
                .collect();
            let starttime = fields[22 - 3];
            assert!(digits(starttime), "{stat}");
            assert_eq!(marker, format!("l:{}:{starttime}", boot_id.trim()));
        }
        // macOS：`m:<pbi_start_tvsec>.<pbi_start_tvusec>`，两段都是不补零的十进制
        #[cfg(target_os = "macos")]
        {
            let (sec, usec) = marker
                .strip_prefix("m:")
                .and_then(|rest| rest.split_once('.'))
                .expect(&marker);
            assert!(digits(sec) && digits(usec), "{marker}");
            assert!(usec == "0" || !usec.starts_with('0'), "{marker}");
            assert!(usec.parse::<u32>().unwrap() < 1_000_000, "{marker}");
        }
        // 自己的条目判为在运行；同一个 pid 配上别的标记，就不是它
        let mine =
            LedgerEntry::for_current_process(ledger::ROLE_INSTALLER, None, None, None).unwrap();
        assert!(ledger::is_alive(&mine));
        let stranger = LedgerEntry {
            start: format!("{marker}0"),
            ..mine
        };
        assert!(!ledger::is_alive(&stranger));
    }
}

// ---------------------------------------------------------------------------
// C5：`--version` 首行
// ---------------------------------------------------------------------------

#[test]
fn c5_version_line() {
    let parse = |line: &str| VersionLine::parse(line);
    assert_eq!(
        parse("aproxy v0.2.0 (desktop; x86_64-pc-windows-msvc; x86-64-v3)"),
        Some(VersionLine {
            version: "0.2.0".into(),
            variant: Some("desktop".into()),
            target: Some("x86_64-pc-windows-msvc".into()),
            x86_64_v3: true,
        })
    );
    assert_eq!(
        parse("aproxy v0.2.0-alpha.1 (headless; aarch64-unknown-linux-musl)\n"),
        Some(VersionLine {
            version: "0.2.0-alpha.1".into(),
            variant: Some("headless".into()),
            target: Some("aarch64-unknown-linux-musl".into()),
            x86_64_v3: false,
        })
    );
    // 只读首行；版本是 `aproxy v` 之后到空白为止的合法 semver
    assert_eq!(
        parse("aproxy v1.2.3 (desktop; x86_64-apple-darwin)\nsomething else").map(|l| l.version),
        Some("1.2.3".into())
    );
    // 读者容错：只取版本号之后的第一个括号组，`)` 之后的内容一概不看（写者不得在那里
    // 追加，但哪天有版本追加了，旧读者照样认得出它的变体与目标）；组内各段两端的空白
    // 不影响；没有括号组时只有版本
    assert_eq!(
        parse("aproxy v0.9.0 (headless; x86_64-unknown-linux-gnu; x86-64-v3) [lts] (extra; words)"),
        Some(VersionLine {
            version: "0.9.0".into(),
            variant: Some("headless".into()),
            target: Some("x86_64-unknown-linux-gnu".into()),
            x86_64_v3: true,
        })
    );
    assert_eq!(
        parse("aproxy v0.9.0 (desktop;x86_64-apple-darwin ) build-a1b2c3"),
        Some(VersionLine {
            version: "0.9.0".into(),
            variant: Some("desktop".into()),
            target: Some("x86_64-apple-darwin".into()),
            x86_64_v3: false,
        })
    );
    assert_eq!(
        parse("aproxy v0.9.0"),
        Some(VersionLine {
            version: "0.9.0".into(),
            variant: None,
            target: None,
            x86_64_v3: false,
        })
    );
    assert_eq!(
        aproxy::install::staging::parse_version_line("aproxy v1.2.3 (desktop; t)").as_deref(),
        Some("1.2.3")
    );
    // 0.1.x 的格式（没有 v）与别的东西都不是
    for line in ["aproxy 0.1.0", "aproxy vX.Y", "aproxyv0.2.0", "", "0.2.0"] {
        assert_eq!(parse(line), None, "{line:?}");
    }

    // 本构建自己的输出：正是这个格式，变体与目标是本构建的；退出码 0；不读写 home——
    // APROXY_HOME 指向一个还不存在的目录，跑完它仍不存在（`--version` 有了副作用，这里
    // 就会看到它建出来的东西，也不会碰到用户真正在用的 home）
    let parent = temp_home();
    let home = parent.path().join("home-that-must-not-appear");
    for flag in ["--version", "-V"] {
        let out = Command::new(env!("CARGO_BIN_EXE_aproxy"))
            .arg(flag)
            .env("APROXY_HOME", &home)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert_eq!(out.status.code(), Some(0), "{flag}: {stdout}");
        let line = parse(&stdout).expect(&stdout);
        assert_eq!(line.version, env!("CARGO_PKG_VERSION"));
        assert!(
            matches!(line.variant.as_deref(), Some("desktop" | "headless")),
            "{stdout}"
        );
        assert_eq!(line.target, VersionLine::own().target, "{stdout}");
        assert!(!home.exists(), "{flag} 不得建出 home");
    }
}

// ---------------------------------------------------------------------------
// C6：发布清单
// ---------------------------------------------------------------------------

#[test]
fn c6_release_manifest() {
    assert_eq!(force::MANIFEST_ASSET, "aproxy-manifest.json");
    assert_eq!(force::MANIFEST_FORMAT, 1);
    assert_eq!(force::VARIANT_DESKTOP, "desktop");
    assert_eq!(force::VARIANT_HEADLESS, "headless");
    assert_eq!(force::CPU_BASELINE, "baseline");
    assert_eq!(force::CPU_X86_64_V3, "x86-64-v3");

    let sha = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    let text = format!(
        r#"{{
  "manifest": 1,
  "version": "0.2.0",
  "published": "2031-01-01",
  "assets": [
    {{"target": "x86_64-pc-windows-msvc", "variant": "desktop", "cpu": "baseline", "file": "aproxy-x86_64-pc-windows-msvc.exe", "sha256": "{sha}"}},
    {{"target": "x86_64-pc-windows-msvc", "variant": "desktop", "cpu": "x86-64-v3", "file": "aproxy-x86_64-pc-windows-msvc-v3.exe", "sha256": "{sha}"}},
    {{"target": "x86_64-unknown-linux-gnu", "variant": "headless", "cpu": "baseline", "file": "aproxy-x86_64-unknown-linux-gnu-headless", "sha256": "{sha}", "size": 1}},
    {{"target": "x86_64-unknown-linux-gnu", "variant": "tray", "cpu": "baseline", "file": "future-variant", "sha256": "{sha}"}},
    {{"target": "x86_64-unknown-linux-gnu", "variant": "headless", "cpu": "x86-64-v4", "file": "future-cpu", "sha256": "{sha}"}}
  ]
}}"#
    );
    let manifest = Manifest::parse(&text).expect("清单可解析（未知字段忽略）");
    assert_eq!(manifest.manifest, 1);
    assert_eq!(manifest.version, "0.2.0");
    assert_eq!(manifest.assets.len(), 5);
    let asset = &manifest.assets[1];
    assert_eq!(asset.target, "x86_64-pc-windows-msvc");
    assert_eq!(asset.variant, "desktop");
    assert_eq!(asset.cpu, "x86-64-v3");
    assert_eq!(asset.file, "aproxy-x86_64-pc-windows-msvc-v3.exe");
    assert_eq!(asset.sha256, sha);

    let pick = |target: &str, variant: &str, v3: bool| {
        manifest
            .select(target, variant, v3)
            .map(|a| a.file.as_str())
    };
    assert_eq!(
        pick("x86_64-pc-windows-msvc", "desktop", true),
        Some("aproxy-x86_64-pc-windows-msvc-v3.exe")
    );
    assert_eq!(
        pick("x86_64-pc-windows-msvc", "desktop", false),
        Some("aproxy-x86_64-pc-windows-msvc.exe")
    );
    // 没有 v3 档退回基线档；不认识的变体与指令集档不参与选择
    assert_eq!(
        pick("x86_64-unknown-linux-gnu", "headless", true),
        Some("aproxy-x86_64-unknown-linux-gnu-headless")
    );
    assert_eq!(pick("x86_64-unknown-linux-gnu", "desktop", true), None);
    assert_eq!(pick("x86_64-pc-windows-msvc", "headless", true), None);

    // 读者容错：开头的 UTF-8 BOM 去掉；assets 里读不懂的元素（别种资产、缺字段、类型
    // 不对、不是对象）逐个跳过，其余照读。以后清单里多了什么，旧版本都只是选不到它
    let tolerant = format!(
        "\u{feff}{}",
        r#"{"manifest":1,"version":"0.3.0","assets":[
            {"target":"t","variant":"headless","cpu":"baseline","file":"first","sha256":"SHA"},
            {"kind":"skills","file":"aproxy-skills.zip","sha256":"SHA"},
            {"target":"t","variant":"desktop","cpu":"baseline","file":"no-sha"},
            {"target":"t","variant":"desktop","cpu":"baseline","file":7,"sha256":"SHA"},
            "aproxy-format.zip",
            null,
            {"target":"t","variant":"desktop","cpu":"baseline","file":"last","sha256":"SHA","signature":{"alg":"x"}}
        ]}"#
        .replace("SHA", sha)
    );
    let manifest = Manifest::parse(&tolerant).expect("带 BOM、含读不懂元素的清单照样可读");
    assert_eq!(manifest.version, "0.3.0");
    let files: Vec<&str> = manifest.assets.iter().map(|a| a.file.as_str()).collect();
    assert_eq!(files, ["first", "last"]);

    // 清单本身的必填字段缺了、assets 不是数组，就不是清单
    for bad in [
        r#"{"version":"0.2.0","assets":[]}"#,
        r#"{"manifest":1,"assets":[]}"#,
        r#"{"manifest":1,"version":"0.2.0"}"#,
        r#"{"manifest":1,"version":"0.2.0","assets":{}}"#,
    ] {
        assert!(Manifest::parse(bad).is_err(), "{bad}");
    }
}

// ---------------------------------------------------------------------------
// C7：命令行写法与退出码
// ---------------------------------------------------------------------------

#[test]
fn c7_exit_codes() {
    assert_eq!(force::EXIT_DONE, 0);
    assert_eq!(force::EXIT_UNCHANGED, 10);
    assert_eq!(force::EXIT_STOPPED, 11);
    assert_eq!(force::EXIT_NOT_RESTORED, 12);
}

/// C7：`aproxy restore` 子命令永远存在——任何版本的强制安装换好二进制之后，都调用新版本的
/// `restore` 拉回实例。空的 home 里没有可恢复的实例，它以 0 结束；改了名或删掉了，这里
/// 就是 clap 的用法错误 2。
#[test]
fn c7_restore_always_exists() {
    let home = temp_home();
    let out = aproxy_in(home.path(), &["restore"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// 这些写法都被接受（参数解析通过，用法错误的退出码是 2），并且在动任何东西之前
/// 就以 10 结束：源文件不存在、版本低于 0.2.0、下载代理连不上。
#[test]
fn c7_accepted_command_lines_end_with_10_before_touching_anything() {
    let home = temp_home();
    let missing = home.path().join("no-such-aproxy");
    let missing = missing.to_str().unwrap();
    // 下载代理指向本机一个没人监听的端口：在线取目标立即失败，不会真的联网
    let dead_proxy = "http://127.0.0.1:9";
    let cases: Vec<Vec<&str>> = vec![
        vec!["install", "--force", "--from", missing, "--yes"],
        vec!["upgrade", "--force", "--from", missing, "--yes"],
        vec![
            "install",
            "--force",
            "--from",
            missing,
            "--yes",
            "--variant",
            "headless",
        ],
        vec!["install", "--force", "0.1.0", "--yes"],
        vec!["upgrade", "--force", "0.1.9", "--yes"],
        vec![
            "install",
            "--force",
            "latest",
            "--yes",
            "--variant",
            "desktop",
            "--cpu",
            "baseline",
            "--pre",
            "--download-proxy",
            dead_proxy,
        ],
        vec![
            "install",
            "--force",
            "0.2.0",
            "--yes",
            "--cpu",
            "x86-64-v3",
            "--download-proxy",
            dead_proxy,
        ],
        vec![
            "install",
            "--force",
            "--yes",
            "--download-proxy",
            dead_proxy,
        ],
    ];
    for args in cases {
        let out = aproxy_in(home.path(), &args);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(10), "{args:?}: {stderr}");
    }
    assert!(
        !aproxy::install::swap::bin_dir_in(home.path()).exists(),
        "以 10 结束的强制安装不碰 bin"
    );
}

/// 没有 --yes、标准输入又不是终端：不卡着等输入，以 10 结束，并提示加 --yes。
#[test]
fn c7_confirmation_needs_yes_when_not_interactive() {
    let home = temp_home();
    let source: PathBuf = home
        .path()
        .join(format!("aproxy-source{}", std::env::consts::EXE_SUFFIX));
    std::fs::copy(env!("CARGO_BIN_EXE_aproxy"), &source).unwrap();
    let out = aproxy_in(
        home.path(),
        &["install", "--force", "--from", source.to_str().unwrap()],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(10), "{stderr}");
    assert!(stderr.contains("--yes"), "{stderr}");
    assert!(!aproxy::install::swap::bin_dir_in(home.path()).exists());
}

// ---------------------------------------------------------------------------
// C8：流程顺序
// ---------------------------------------------------------------------------

/// C8 第 1 步：home 不存在就以 10 结束（多半是 APROXY_HOME 写错了），也不把它建出来——
/// 强制安装只修已经装着的 home，照样装下去只会在一个新目录里「成功」。
#[test]
fn c8_a_missing_home_ends_with_10_and_is_not_created() {
    let parent = temp_home();
    let home = parent.path().join("no-such-home");
    let out = aproxy_in(
        &home,
        &["install", "--force", "--from", "no-such-file", "--yes"],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(force::EXIT_UNCHANGED), "{stderr}");
    assert!(stderr.contains("APROXY_HOME"), "{stderr}");
    assert!(!home.exists(), "不存在的 home 不得被建出来");
}

/// C8 第 3 步「先按台账结束安装者，再拿安装锁」：台账里有一个正持有安装锁的安装者时，强制
/// 安装先结束它（锁随进程死亡释放），再拿到锁、照常完成。顺序反过来，就会一直等这把锁、
/// 等满之后以失败结束。
#[test]
fn c8_installers_are_stopped_before_the_lock_is_taken() {
    let home = temp_home();
    let home = home.path();
    // bin 里装着当前构建；要换上的是它的副本，末尾追加一段内容以便区分（PE 与 ELF 都容忍
    // 文件末尾的附加数据）
    let bin = aproxy::install::swap::bin_path_in(home);
    std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
    std::fs::copy(env!("CARGO_BIN_EXE_aproxy"), &bin).unwrap();
    let next = home.join(format!("aproxy-next{}", std::env::consts::EXE_SUFFIX));
    let mut bytes = std::fs::read(env!("CARGO_BIN_EXE_aproxy")).unwrap();
    bytes.extend_from_slice(b"\nC8-NEXT-BINARY\n");
    std::fs::write(&next, &bytes).unwrap();
    std::fs::set_permissions(&next, std::fs::metadata(&bin).unwrap().permissions()).unwrap();

    // 持锁的安装者：本测试程序自己，只运行下面那个辅助函数
    let ready = home.join("holder-ready");
    let mut holder = Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "c8_lock_holding_installer_helper"])
        .env("APROXY_TEST_LOCK_HOLDER", home)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !ready.exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let holder_ready = ready.exists();

    let out = holder_ready.then(|| {
        aproxy_in(
            home,
            &[
                "install",
                "--force",
                "--from",
                next.to_str().unwrap(),
                "--yes",
            ],
        )
    });
    // 结束测试自己的子进程放在断言之前：断言失败也不留下进程
    let holder_stopped = holder.try_wait().unwrap().is_some();
    let _ = holder.kill();
    let _ = holder.wait();

    assert!(holder_ready, "持锁的安装者没有就绪");
    let out = out.unwrap();
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(out.status.code(), Some(force::EXIT_DONE), "{output}");
    assert!(holder_stopped, "持锁的安装者应已被结束: {output}");
    assert_eq!(std::fs::read(&bin).unwrap(), bytes);
}

/// 上一个测试的「持锁的安装者」：以安装者身份登记进给定 home 的台账、拿住安装锁、通知
/// 就绪，然后睡着等被结束。平时（没有那个环境变量）立即返回。
#[test]
#[ignore = "只作为子进程运行，见 c8_installers_are_stopped_before_the_lock_is_taken"]
fn c8_lock_holding_installer_helper() {
    let Some(home) = std::env::var_os("APROXY_TEST_LOCK_HOLDER") else {
        return;
    };
    let home = PathBuf::from(home);
    let real = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    assert!(
        real(&home).starts_with(real(&std::env::temp_dir())),
        "持锁的安装者只在临时目录里的 home 运行: {}",
        home.display()
    );
    let _entry = ledger::register(
        &home,
        ledger::ROLE_INSTALLER,
        None,
        Some("lock holder"),
        None,
    );
    let _lock = aproxy::install::lock_install_in(&home, std::time::Duration::from_secs(5)).unwrap();
    std::fs::write(home.join("holder-ready"), b"").unwrap();
    std::thread::sleep(std::time::Duration::from_secs(120));
}
