//! 强制安装 `aproxy install --force`（永久兼容契约 C7、C8）的端到端测试：真实的实例与
//! 看护者被按台账结束、二进制换好、实例经新版本的 restore 回来；没回来的实例（12）；
//! 二进制换不成（11）；台账里只结束核验通过的进程；install/ 被清空而锁留下；在线取
//! 目标走发布清单（本地假 GitHub）。契约本身的钉子在 `force_contract.rs`。
//!
//! 隔离：每个测试一个临时 home（[`TestHome::new`] 核对它在临时目录里、不是用户的
//! `~/.aproxy`），所有 aproxy 进程都带着它的 APROXY_HOME 运行。结束进程只按这个临时
//! home 的台账（核验启动标记）或测试自己持有的子进程句柄，绝不按名字。

use aproxy::install::force::{self, Manifest, ManifestAsset, OnlineSpec};
use aproxy::install::swap::{OLD_PREFIX, bin_dir_in, bin_path_in};
use aproxy::ledger::{self, LedgerEntry};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// 追加在 `--from` 源二进制末尾的标记：PE 与 ELF 都容忍文件末尾的附加数据，二进制照样
/// 能跑，而内容与原来的不同——由此判断 bin 里换上的是哪一个。
const NEXT_MARKER: &[u8] = b"\nFORCE-INSTALL-NEXT-BINARY\n";

/// bind 试探选可用端口（与别的测试文件错开基址；原子计数让同一进程里并发的测试不互撞）
fn free_port() -> u16 {
    use std::sync::atomic::{AtomicU16, Ordering};
    static SEQ: AtomicU16 = AtomicU16::new(0);
    let base = 29000u16 + (std::process::id() % 400) as u16 * 20;
    for i in 0..200u16 {
        let candidate = base + SEQ.fetch_add(1, Ordering::Relaxed) + i * 200;
        if std::net::TcpListener::bind(("127.0.0.1", candidate)).is_ok() {
            return candidate;
        }
    }
    panic!("200 次内没有找到可用端口");
}

/// 一个隔离的 aproxy home。析构时结束它台账里仍在运行的进程（测试拉起的实例、看护者，
/// 以及被 restore 拉回来的实例）：台账条目只可能来自带着这个 APROXY_HOME 运行的进程，
/// 结束前还要核验启动标记。
struct TestHome {
    dir: tempfile::TempDir,
}

impl TestHome {
    fn new() -> Self {
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
        Self { dir }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    /// 在这个 home 里运行 `exe`（标准输入为空、不是终端）
    fn command(&self, exe: &Path, args: &[&str]) -> Command {
        let mut cmd = Command::new(exe);
        cmd.args(args)
            .env("APROXY_HOME", self.path())
            .env("NO_PROXY", "127.0.0.1,localhost")
            .stdin(Stdio::null());
        cmd
    }

    fn settings(&self, json: &str) {
        std::fs::write(self.path().join("settings.json"), json).unwrap();
    }

    /// 把当前构建放进 `<home>/bin/`（已安装的形态），返回它的路径
    fn seed_bin(&self) -> PathBuf {
        let bin = bin_path_in(self.path());
        std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
        std::fs::copy(env!("CARGO_BIN_EXE_aproxy"), &bin).unwrap();
        make_executable(&bin);
        bin
    }

    /// `--from` 的源：当前构建的副本，末尾追加 [`NEXT_MARKER`]
    fn next_binary(&self) -> PathBuf {
        let path = self
            .path()
            .join(format!("aproxy-next{}", std::env::consts::EXE_SUFFIX));
        let mut bytes = std::fs::read(env!("CARGO_BIN_EXE_aproxy")).unwrap();
        bytes.extend_from_slice(NEXT_MARKER);
        std::fs::write(&path, bytes).unwrap();
        make_executable(&path);
        path
    }

    /// 用 bin 里的二进制在后台启动一个实例（`aproxy start --config <文件>`），等它在台账
    /// 里登记，返回它的条目
    fn start_instance(&self, name: &str, port: u16) -> LedgerEntry {
        let cfg = self.path().join(format!("{name}.toml"));
        std::fs::write(
            &cfg,
            format!(
                "base_url = \"https://{name}.example.com\"\nlisten_addr = \"127.0.0.1:{port}\"\n"
            ),
        )
        .unwrap();
        let out = self
            .command(
                &bin_path_in(self.path()),
                &["start", "--config", cfg.to_str().unwrap()],
            )
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "start 失败: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        self.wait_for(Duration::from_secs(10), |e| {
            e.role == ledger::ROLE_INSTANCE && e.port == Some(port)
        })
        .expect("实例应已登记台账")
    }

    /// 等台账里出现一条满足 `matches` 且仍在运行的条目
    fn wait_for(
        &self,
        timeout: Duration,
        matches: impl Fn(&LedgerEntry) -> bool,
    ) -> Option<LedgerEntry> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(found) = ledger::read_all_in(self.path())
                .into_iter()
                .map(|f| f.entry)
                .find(|e| matches(e) && ledger::is_alive(e))
            {
                return Some(found);
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// 用当前构建执行 `install --force <args…> --yes`，返回退出码与全部输出
    fn force_install(&self, args: &[&str]) -> (Option<i32>, String) {
        let mut full = vec!["install", "--force"];
        full.extend_from_slice(args);
        full.push("--yes");
        let out = self
            .command(Path::new(env!("CARGO_BIN_EXE_aproxy")), &full)
            .output()
            .unwrap();
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        (out.status.code(), text)
    }

    /// `<home>/install/` 里的条目名（排好序）
    fn install_dir_names(&self) -> Vec<String> {
        let mut names: Vec<String> =
            std::fs::read_dir(aproxy::install::install_dir_in(self.path()))
                .map(|dir| {
                    dir.flatten()
                        .map(|e| e.file_name().to_string_lossy().into_owned())
                        .collect()
                })
                .unwrap_or_default();
        names.sort();
        names
    }

    /// bin 里让位的旧二进制（`aproxy.old-…`）
    fn retired(&self) -> Vec<PathBuf> {
        std::fs::read_dir(bin_dir_in(self.path()))
            .map(|dir| {
                dir.flatten()
                    .map(|e| e.path())
                    .filter(|p| {
                        p.file_name()
                            .and_then(|n| n.to_str())
                            .is_some_and(|n| n.starts_with(OLD_PREFIX))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

impl Drop for TestHome {
    fn drop(&mut self) {
        for found in ledger::read_all_in(self.path()) {
            let _ = ledger::terminate(&found.entry, Duration::from_secs(5));
        }
    }
}

fn make_executable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    #[cfg(windows)]
    let _ = path;
}

/// Windows：路径的 8.3 短名写法（GetShortPathNameW）。卷上没有生成短名时，返回的与
/// 原路径相同。经短名启动的进程，镜像路径也带着短名——CI 的临时目录就是
/// `C:\Users\RUNNER~1\…`，测试要能在本机造出同样的情形。
#[cfg(windows)]
fn short_path(path: &Path) -> PathBuf {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
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
        path.display()
    );
    PathBuf::from(std::ffi::OsString::from_wide(&buf[..len]))
}

/// 一个测试自己启动、持有句柄的长时间运行的进程（结束它只用这个句柄）
fn sleeper() -> Child {
    #[cfg(windows)]
    let mut cmd = {
        let mut cmd = Command::new("ping");
        cmd.args(["-n", "300", "127.0.0.1"]);
        cmd
    };
    #[cfg(unix)]
    let mut cmd = {
        let mut cmd = Command::new("sleep");
        cmd.arg("300");
        cmd
    };
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

/// 替 `pid` 在 `home` 的台账里写一条条目
fn write_entry(home: &Path, pid: u32, start: String, role: &str) -> PathBuf {
    ledger::write_entry_in(
        home,
        &LedgerEntry {
            ledger: ledger::LEDGER_FORMAT,
            pid,
            start,
            role: role.into(),
            version: "0.2.0".into(),
            port: None,
            label: Some("test".into()),
            exe: None,
        },
    )
    .unwrap()
}

/// 等子进程退出（最多 `timeout`），返回它是否已退出
fn exited_within(child: &mut Child, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if child.try_wait().unwrap().is_some() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn ping_instance(home: &Path, port: u16) -> Option<aproxy::daemon::InstanceStatus> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(aproxy::daemon::ipc_ping_in(
            &home.join("run"),
            &port.to_string(),
        ))
        .ok()
}

// ---------------------------------------------------------------------------
// 1. 主流程：真实的实例与看护者（从 bin 运行）→ 按台账结束 → 换二进制（原来的让位成
//    唯一中间名）→ 新版本的 restore 把实例拉回同一端口 → 退出码 0，install/ 只剩锁
// ---------------------------------------------------------------------------
#[test]
fn force_install_stops_everything_swaps_and_restores() {
    let home = TestHome::new();
    home.settings(r#"{"watchdog": true}"#);
    home.seed_bin();
    let port = free_port();
    let instance = home.start_instance("main", port);
    let watchdog = home
        .wait_for(Duration::from_secs(15), |e| e.role == ledger::ROLE_WATCHDOG)
        .expect("看护者应随 start 拉起并登记台账");
    let next = home.next_binary();

    let (code, output) = home.force_install(&["--from", next.to_str().unwrap()]);
    assert_eq!(code, Some(force::EXIT_DONE), "{output}");

    // 原来的实例与看护者都已被结束，条目已删
    assert!(!ledger::is_alive(&instance), "{output}");
    assert!(!ledger::is_alive(&watchdog), "{output}");
    for old in [&instance, &watchdog] {
        assert!(!ledger::entry_path_in(home.path(), old.pid).exists());
    }
    // bin 里是新二进制；原来的让位成唯一中间名，仍然可见
    let bin = bin_path_in(home.path());
    assert_eq!(std::fs::read(&bin).unwrap(), std::fs::read(&next).unwrap());
    let retired = home.retired();
    assert_eq!(retired.len(), 1, "{retired:?}");
    assert_eq!(
        std::fs::read(&retired[0]).unwrap(),
        std::fs::read(env!("CARGO_BIN_EXE_aproxy")).unwrap()
    );
    // 实例已经回来：同一端口上一个新进程，跑的是 bin 里的新二进制，IPC 应答
    let back = home
        .wait_for(Duration::from_secs(5), |e| {
            e.role == ledger::ROLE_INSTANCE && e.port == Some(port)
        })
        .expect("实例应已由 restore 拉回");
    assert_ne!(back.pid, instance.pid);
    // restore 的输出经文件转写到强制安装自己的输出里
    assert!(
        output.contains(&format!("已恢复 pid {}", back.pid)),
        "{output}"
    );
    let image = aproxy::watchdog::process_image_path(back.pid).expect("新实例的镜像路径可查");
    assert_eq!(
        std::fs::canonicalize(image).unwrap(),
        std::fs::canonicalize(&bin).unwrap()
    );
    assert!(ping_instance(home.path(), port).is_some(), "{output}");
    // install/ 只剩安装锁：本次的工作目录也已删掉
    assert_eq!(home.install_dir_names(), vec!["lock"], "{output}");
}

// ---------------------------------------------------------------------------
// 2. 被结束的实例没回来（它的配置文件已被删，restore 拉不起它）→ 二进制照样换好，
//    退出码 12，输出按端口列出它
// ---------------------------------------------------------------------------
#[test]
fn an_instance_that_does_not_come_back_is_listed_with_12() {
    let home = TestHome::new();
    home.settings(r#"{"watchdog": false}"#);
    home.seed_bin();
    let port = free_port();
    let instance = home.start_instance("gone", port);
    std::fs::remove_file(home.path().join("gone.toml")).unwrap();
    let next = home.next_binary();

    let (code, output) = home.force_install(&["--from", next.to_str().unwrap()]);
    assert_eq!(code, Some(force::EXIT_NOT_RESTORED), "{output}");
    assert!(output.contains(&format!("端口 {port}")), "{output}");
    assert!(!ledger::is_alive(&instance));
    assert_eq!(
        std::fs::read(bin_path_in(home.path())).unwrap(),
        std::fs::read(&next).unwrap()
    );
}

// ---------------------------------------------------------------------------
// 3. 台账：安装者与不认识的角色都按台账结束；pid 对得上、启动标记对不上的进程（pid
//    已被复用给别人）不结束、只删条目；进程已不在的条目删掉；解析不了的文件不动。
//    install/ 里旧安装留下的一切被清空，锁留下。从 bin 运行、不在台账里的进程只报告
// ---------------------------------------------------------------------------
#[test]
fn only_verified_ledger_processes_are_stopped() {
    let home = TestHome::new();
    home.seed_bin();
    let mut installer = sleeper();
    let mut future = sleeper();
    let mut stranger = sleeper();
    let marker = |child: &Child| ledger::start_marker(child.id()).expect("子进程的启动标记可查");
    let installer_entry = write_entry(
        home.path(),
        installer.id(),
        marker(&installer),
        ledger::ROLE_INSTALLER,
    );
    let future_entry = write_entry(home.path(), future.id(), marker(&future), "tray");
    // 同一个 pid、别的启动标记：台账里写的那个进程早已不在，pid 现在属于别人
    let stranger_entry = write_entry(
        home.path(),
        stranger.id(),
        format!("{}0", marker(&stranger)),
        ledger::ROLE_INSTANCE,
    );
    // 不存在的 pid（Linux 的 pid_max 远小于它；Windows 上打开它报参数错误）
    let dead_entry = write_entry(
        home.path(),
        u32::MAX - 7,
        marker(&installer),
        ledger::ROLE_WATCHDOG,
    );
    let garbage = ledger::ledger_dir_in(home.path()).join("garbage.json");
    std::fs::write(&garbage, "{ not a ledger entry").unwrap();
    // 一个旧的普通安装留下的现场，与一个死掉的强制安装留下的工作目录
    let install_dir = aproxy::install::install_dir_in(home.path());
    std::fs::create_dir_all(install_dir.join("staging").join("0.2.0")).unwrap();
    std::fs::write(
        install_dir.join("staging").join("0.2.0").join("aproxy"),
        b"x",
    )
    .unwrap();
    std::fs::write(aproxy::install::state::state_path_in(home.path()), b"{}").unwrap();
    std::fs::create_dir_all(install_dir.join("force").join("1")).unwrap();
    std::fs::write(install_dir.join("force").join("1").join("aproxy"), b"x").unwrap();
    // 从 bin 运行、不在台账里的进程（0.1.x 留下的那种）：本测试程序自己的副本，睡着
    let stray_exe = bin_dir_in(home.path()).join(format!("stray{}", std::env::consts::EXE_SUFFIX));
    std::fs::copy(std::env::current_exe().unwrap(), &stray_exe).unwrap();
    make_executable(&stray_exe);
    // Windows 上经 8.3 短名启动（卷上有短名时）：它的镜像路径带着短名，报告仍须认得它
    #[cfg(windows)]
    let stray_launch = short_path(&stray_exe);
    #[cfg(not(windows))]
    let stray_launch = stray_exe.clone();
    let mut stray = Command::new(&stray_launch)
        .args(["--ignored", "--exact", "stray_helper_sleeps"])
        .env("APROXY_TEST_STRAY", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let next = home.next_binary();

    let (code, output) = home.force_install(&["--from", next.to_str().unwrap()]);
    // 结束测试自己的子进程放在断言之前：断言失败也不留下进程
    let installer_gone = exited_within(&mut installer, Duration::from_secs(5));
    let future_gone = exited_within(&mut future, Duration::from_secs(5));
    let stranger_alive = stranger.try_wait().unwrap().is_none();
    let stray_alive = stray.try_wait().unwrap().is_none();
    for child in [&mut installer, &mut future, &mut stranger, &mut stray] {
        let _ = child.kill();
        let _ = child.wait();
    }

    assert_eq!(code, Some(force::EXIT_DONE), "{output}");
    assert!(installer_gone, "安装者应按台账被结束: {output}");
    assert!(future_gone, "不认识的角色也应被结束: {output}");
    assert!(stranger_alive, "启动标记对不上的进程绝不能被结束: {output}");
    assert!(stray_alive, "不在台账里的进程只报告、不结束: {output}");
    for entry in [
        &installer_entry,
        &future_entry,
        &stranger_entry,
        &dead_entry,
    ] {
        assert!(!entry.exists(), "{} 应已删除", entry.display());
    }
    assert!(garbage.exists(), "解析不了的文件不归强制安装处理");
    // 枚举进程、查镜像路径只在 Windows 与 Linux 上做得到
    if cfg!(any(windows, target_os = "linux")) {
        assert!(
            output.contains(&stray.id().to_string()) && output.contains("不在台账里"),
            "{output}"
        );
    }
    assert_eq!(home.install_dir_names(), vec!["lock"], "{output}");
    assert_eq!(
        std::fs::read(bin_path_in(home.path())).unwrap(),
        std::fs::read(&next).unwrap()
    );
}

/// 第 3 个测试的「从 bin 运行、不在台账里的进程」：那个测试把本测试程序复制进 bin，
/// 带上 `--ignored` 与环境变量只运行这一个函数。平时（没有那个环境变量）立即返回。
#[test]
#[ignore = "只作为子进程运行，见 only_verified_ledger_processes_are_stopped"]
fn stray_helper_sleeps() {
    if std::env::var_os("APROXY_TEST_STRAY").is_some() {
        std::thread::sleep(Duration::from_secs(120));
    }
}

// ---------------------------------------------------------------------------
// 4. 什么都不动的退出码 10：没有确认（标准输入不是终端、没有 --yes）、源是 0.1.x 的
//    二进制、源的变体与 --variant 不符。台账里的进程照常运行，bin 不变，不留目录
// ---------------------------------------------------------------------------
#[test]
fn refusals_end_with_10_and_change_nothing() {
    let home = TestHome::new();
    let bin = home.seed_bin();
    let original = std::fs::read(&bin).unwrap();
    let mut running = sleeper();
    let marker = ledger::start_marker(running.id()).unwrap();
    let entry = write_entry(home.path(), running.id(), marker, ledger::ROLE_INSTANCE);
    let next = home.next_binary();
    let next = next.to_str().unwrap();
    // 0.1.x 的 `--version` 输出（Windows 为 .cmd，unix 为 sh 脚本）
    let legacy = if cfg!(windows) {
        let path = home.path().join("legacy.cmd");
        std::fs::write(&path, "@echo aproxy 0.1.0\r\n").unwrap();
        path
    } else {
        let path = home.path().join("legacy");
        std::fs::write(&path, "#!/bin/sh\necho \"aproxy 0.1.0\"\n").unwrap();
        make_executable(&path);
        path
    };
    let other_variant = match force::VersionLine::own().variant.as_deref() {
        Some(force::VARIANT_DESKTOP) => force::VARIANT_HEADLESS,
        _ => force::VARIANT_DESKTOP,
    };

    let exe = Path::new(env!("CARGO_BIN_EXE_aproxy"));
    let no_confirmation = home
        .command(exe, &["install", "--force", "--from", next])
        .output()
        .unwrap();
    let (legacy_code, legacy_out) = home.force_install(&["--from", legacy.to_str().unwrap()]);
    let (variant_code, variant_out) =
        home.force_install(&["--from", next, "--variant", other_variant]);
    let still_running = running.try_wait().unwrap().is_none();
    let _ = running.kill();
    let _ = running.wait();

    let no_confirmation_err = String::from_utf8_lossy(&no_confirmation.stderr);
    assert_eq!(
        no_confirmation.status.code(),
        Some(force::EXIT_UNCHANGED),
        "{no_confirmation_err}"
    );
    assert!(
        no_confirmation_err.contains("--yes"),
        "{no_confirmation_err}"
    );
    assert_eq!(legacy_code, Some(force::EXIT_UNCHANGED), "{legacy_out}");
    assert!(legacy_out.contains("低于 0.2.0"), "{legacy_out}");
    assert_eq!(variant_code, Some(force::EXIT_UNCHANGED), "{variant_out}");
    assert!(variant_out.contains(other_variant), "{variant_out}");

    assert!(still_running, "退出码 10 不结束任何进程");
    assert!(entry.exists());
    assert_eq!(std::fs::read(&bin).unwrap(), original);
    assert!(
        !aproxy::install::install_dir_in(home.path()).exists(),
        "{:?}",
        home.install_dir_names()
    );
}

// ---------------------------------------------------------------------------
// 5. 二进制换不成 → 退出码 11，bin 原样。Windows：bin 被不带共享删除权限的句柄占着；
//    unix：bin 目录只读
// ---------------------------------------------------------------------------
#[cfg(windows)]
#[test]
fn a_held_bin_ends_with_11_and_keeps_the_old_binary() {
    use std::os::windows::fs::OpenOptionsExt;
    let home = TestHome::new();
    let bin = home.seed_bin();
    let original = std::fs::read(&bin).unwrap();
    let next = home.next_binary();
    // FILE_SHARE_READ，不带 FILE_SHARE_DELETE：改名与删除都会失败
    let held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0x1)
        .open(&bin)
        .unwrap();
    let (code, output) = home.force_install(&["--from", next.to_str().unwrap()]);
    drop(held);
    assert_eq!(code, Some(force::EXIT_STOPPED), "{output}");
    assert_eq!(std::fs::read(&bin).unwrap(), original);
    assert!(home.retired().is_empty(), "{:?}", home.retired());
}

#[cfg(unix)]
#[test]
fn a_read_only_bin_dir_ends_with_11_and_keeps_the_old_binary() {
    use std::os::unix::fs::PermissionsExt;
    let home = TestHome::new();
    let bin = home.seed_bin();
    let original = std::fs::read(&bin).unwrap();
    let next = home.next_binary();
    let dir = bin_dir_in(home.path());
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    // 以 root 运行时目录权限拦不住写入，这个场景造不出来
    let probe = dir.join("write-probe");
    if std::fs::write(&probe, b"x").is_ok() {
        let _ = std::fs::remove_file(&probe);
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        eprintln!("以 root 运行：只读目录拦不住写入，跳过");
        return;
    }
    let (code, output) = home.force_install(&["--from", next.to_str().unwrap()]);
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(code, Some(force::EXIT_STOPPED), "{output}");
    assert_eq!(std::fs::read(&bin).unwrap(), original);
}

// ---------------------------------------------------------------------------
// 6. Windows：上一次让位的旧二进制被不带共享删除权限的句柄占着（删不掉的残留）→
//    下一次强制安装照样成功，残留原样留着
// ---------------------------------------------------------------------------
#[cfg(windows)]
#[test]
fn a_held_retired_binary_does_not_block_the_next_force_install() {
    use std::os::windows::fs::OpenOptionsExt;
    let home = TestHome::new();
    home.seed_bin();
    let residue = bin_dir_in(home.path()).join("aproxy.old-1760256000123456789-4242.exe");
    std::fs::copy(env!("CARGO_BIN_EXE_aproxy"), &residue).unwrap();
    let held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0x1)
        .open(&residue)
        .unwrap();
    let next = home.next_binary();
    let (code, output) = home.force_install(&["--from", next.to_str().unwrap()]);
    drop(held);
    assert_eq!(code, Some(force::EXIT_DONE), "{output}");
    assert!(residue.exists(), "删不掉的残留跳过");
    assert_eq!(
        std::fs::read(bin_path_in(home.path())).unwrap(),
        std::fs::read(&next).unwrap()
    );
}

// ---------------------------------------------------------------------------
// 7. 输出接到读端已经关闭的管道（`| head` 读够就走、调用方先退出）：写失败一律忽略，
//    退出码照样按实际发生的事给（永久兼容契约 C7）。第 1 步失败仍是 10、不留目录；
//    完整的一次仍是 0——restore 的输出经文件转写，它自己不会因写失败死在半路，实例照样
//    回来。要两个实例：restore 拉起第一个之后才第一次输出，只有一个实例的话，它就算
//    死在那次输出上，实例也已经回来了，测不出差别
// ---------------------------------------------------------------------------
#[test]
fn a_closed_output_pipe_does_not_change_the_exit_code() {
    let home = TestHome::new();
    home.settings(r#"{"watchdog": false}"#);
    home.seed_bin();
    let ports = [free_port(), free_port()];
    let instances = [
        home.start_instance("first", ports[0]),
        home.start_instance("second", ports[1]),
    ];
    let next = home.next_binary();
    let exe = Path::new(env!("CARGO_BIN_EXE_aproxy"));
    // 标准输出与标准错误都接到读端先关掉的管道：子进程的第一次写就失败
    let run = |args: &[&str]| {
        let (reader, writer) = std::io::pipe().unwrap();
        drop(reader);
        home.command(exe, args)
            .stdout(writer.try_clone().unwrap())
            .stderr(writer)
            .status()
            .unwrap()
            .code()
    };

    let missing = home.path().join("no-such-binary");
    let missing = missing.to_str().unwrap();
    assert_eq!(
        run(&["install", "--force", "--from", missing, "--yes"]),
        Some(force::EXIT_UNCHANGED)
    );
    assert!(
        !aproxy::install::install_dir_in(home.path()).exists(),
        "退出码 10 不留目录: {:?}",
        home.install_dir_names()
    );
    for instance in &instances {
        assert!(ledger::is_alive(instance), "退出码 10 不结束任何进程");
    }

    assert_eq!(
        run(&[
            "install",
            "--force",
            "--from",
            next.to_str().unwrap(),
            "--yes"
        ]),
        Some(force::EXIT_DONE)
    );
    for (instance, port) in instances.iter().zip(ports) {
        assert!(!ledger::is_alive(instance));
        let back = home
            .wait_for(Duration::from_secs(5), |e| {
                e.role == ledger::ROLE_INSTANCE && e.port == Some(port)
            })
            .unwrap_or_else(|| panic!("端口 {port} 的实例应已由 restore 拉回"));
        assert_ne!(back.pid, instance.pid);
    }
    assert_eq!(home.install_dir_names(), vec!["lock"]);
}

// ---------------------------------------------------------------------------
// 8. 在线取目标（本地假 GitHub）：latest 只认带发布清单的主线版本；资产按清单选、
//    sha256 比对；linux-gnu 产物跑不起来改用 musl；各种不符都拒绝
// ---------------------------------------------------------------------------

/// 假 GitHub 上的一个 release：(tag, 是否预发布, [(资产名, 内容所在文件)])
type FakeRelease = (String, bool, Vec<(String, PathBuf)>);

/// 一个假的 GitHub，返回仓库的 API 基址（`<基址>/releases…`）。资产的下载地址指回
/// 同一个服务器。
async fn serve_fake_github(releases: Vec<FakeRelease>) -> String {
    use axum::extract::Path as UrlPath;
    use axum::http::StatusCode;
    use axum::routing::get;
    use std::collections::HashMap;
    use std::sync::Arc;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let mut files: HashMap<String, PathBuf> = HashMap::new();
    let mut items = Vec::new();
    for (index, (tag, prerelease, assets)) in releases.into_iter().enumerate() {
        let list: Vec<serde_json::Value> = assets
            .iter()
            .map(|(name, _)| {
                serde_json::json!({
                    "name": name,
                    "browser_download_url": format!("{base}/dl/{index}/{name}"),
                })
            })
            .collect();
        for (name, file) in assets {
            files.insert(format!("{index}/{name}"), file);
        }
        items.push(serde_json::json!({
            "tag_name": tag,
            "draft": false,
            "prerelease": prerelease,
            "assets": list,
        }));
    }
    let items = Arc::new(items);
    let by_tag = items.clone();
    let files = Arc::new(files);
    let router = axum::Router::new()
        .route(
            "/repos/test/releases",
            get(move || {
                let items = items.clone();
                async move { axum::Json(serde_json::Value::Array((*items).clone())) }
            }),
        )
        .route(
            "/repos/test/releases/tags/:tag",
            get(move |UrlPath(tag): UrlPath<String>| {
                let items = by_tag.clone();
                async move {
                    items
                        .iter()
                        .find(|item| item["tag_name"] == tag.as_str())
                        .map(|item| axum::Json(item.clone()))
                        .ok_or(StatusCode::NOT_FOUND)
                }
            }),
        )
        .route(
            "/dl/:index/:name",
            get(move |UrlPath((index, name)): UrlPath<(String, String)>| {
                let files = files.clone();
                async move {
                    let path = files
                        .get(&format!("{index}/{name}"))
                        .ok_or(StatusCode::NOT_FOUND)?;
                    tokio::fs::read(path)
                        .await
                        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
                }
            }),
        );
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    format!("{base}/repos/test")
}

/// 把清单写成文件（交给假 GitHub 当资产）
fn manifest_file(dir: &Path, name: &str, version: &str, assets: Vec<ManifestAsset>) -> PathBuf {
    let path = dir.join(name);
    let manifest = Manifest {
        manifest: force::MANIFEST_FORMAT,
        version: version.into(),
        assets,
    };
    std::fs::write(&path, serde_json::to_string(&manifest).unwrap()).unwrap();
    path
}

fn asset(target: &str, variant: &str, file: &str, content: &Path) -> ManifestAsset {
    ManifestAsset {
        target: target.into(),
        variant: variant.into(),
        cpu: force::CPU_BASELINE.into(),
        file: file.into(),
        sha256: aproxy::install::staging::sha256_hex(content).unwrap(),
    }
}

fn own_variant() -> String {
    force::VersionLine::own().variant.unwrap()
}

fn online_spec(version: Option<&str>, target: &str, variant: &str) -> OnlineSpec {
    OnlineSpec {
        version: version.map(str::to_string),
        channel: aproxy::install::download::Channel::Pre,
        target: target.into(),
        variant: variant.into(),
        cpu: None,
        prefer_v3: false,
    }
}

fn local_client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn online_latest_takes_the_newest_release_that_carries_a_manifest() {
    let home = TestHome::new();
    let files = home.path().join("files");
    std::fs::create_dir_all(&files).unwrap();
    let real = PathBuf::from(env!("CARGO_BIN_EXE_aproxy"));
    let version = env!("CARGO_PKG_VERSION");
    let target = env!("APROXY_BUILD_TARGET");
    let variant = own_variant();
    let good = manifest_file(
        &files,
        "good.json",
        version,
        vec![asset(target, &variant, "aproxy-bin", &real)],
    );
    let older = manifest_file(
        &files,
        "older.json",
        "0.2.0-alpha.0",
        vec![asset(target, &variant, "aproxy-bin", &real)],
    );
    let api = serve_fake_github(vec![
        // 更高、但没有清单：不选
        (
            "v99.0.0".into(),
            false,
            vec![("aproxy-bin".into(), real.clone())],
        ),
        // 别的发版线：不选
        (
            "format-v99.0.0".into(),
            false,
            vec![(force::MANIFEST_ASSET.into(), good.clone())],
        ),
        (
            format!("v{version}"),
            true,
            vec![
                (force::MANIFEST_ASSET.into(), good.clone()),
                ("aproxy-bin".into(), real.clone()),
            ],
        ),
        (
            "v0.1.0".into(),
            false,
            vec![(force::MANIFEST_ASSET.into(), older)],
        ),
    ])
    .await;
    let dest = home
        .path()
        .join(format!("got{}", std::env::consts::EXE_SUFFIX));
    let got = force::acquire_online(
        &local_client(),
        &api,
        &online_spec(None, target, &variant),
        &dest,
    )
    .await
    .unwrap();
    assert_eq!(got.line.version, version);
    assert_eq!(got.line.variant.as_deref(), Some(variant.as_str()));
    assert!(
        got.origin.contains(&format!("v{version}")),
        "{}",
        got.origin
    );
    assert_eq!(std::fs::read(&dest).unwrap(), std::fs::read(&real).unwrap());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn online_falls_back_to_musl_when_the_gnu_asset_does_not_run() {
    let home = TestHome::new();
    let files = home.path().join("files");
    std::fs::create_dir_all(&files).unwrap();
    let real = PathBuf::from(env!("CARGO_BIN_EXE_aproxy"));
    let broken = files.join("broken");
    std::fs::write(&broken, b"not an executable").unwrap();
    let version = env!("CARGO_PKG_VERSION");
    let variant = own_variant();
    let manifest = manifest_file(
        &files,
        "m.json",
        version,
        vec![
            asset("x86_64-unknown-linux-gnu", &variant, "gnu-bin", &broken),
            asset("x86_64-unknown-linux-musl", &variant, "musl-bin", &real),
        ],
    );
    let api = serve_fake_github(vec![(
        format!("v{version}"),
        true,
        vec![
            (force::MANIFEST_ASSET.into(), manifest),
            ("gnu-bin".into(), broken),
            ("musl-bin".into(), real.clone()),
        ],
    )])
    .await;
    let dest = home
        .path()
        .join(format!("got{}", std::env::consts::EXE_SUFFIX));
    let got = force::acquire_online(
        &local_client(),
        &api,
        &online_spec(Some(version), "x86_64-unknown-linux-gnu", &variant),
        &dest,
    )
    .await
    .unwrap();
    assert!(got.origin.contains("musl-bin"), "{}", got.origin);
    assert_eq!(std::fs::read(&dest).unwrap(), std::fs::read(&real).unwrap());
}

/// 清单里找不到要的「目标三元组 + 变体 [+ 指令集档]」时，错误如实列出这个目标有什么——
/// 例如 musl 目标只发布 headless，要 desktop 时提示「这个目标只有 headless」；显式的
/// `--cpu` 不退回别的档；linux-gnu 产物跑不起来、musl 又没有同变体时也这样说明。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn online_reports_what_a_target_offers_when_the_combination_is_missing() {
    let home = TestHome::new();
    let files = home.path().join("files");
    std::fs::create_dir_all(&files).unwrap();
    let broken = files.join("broken");
    std::fs::write(&broken, b"not an executable").unwrap();
    let version = env!("CARGO_PKG_VERSION");
    let gnu = "x86_64-unknown-linux-gnu";
    let musl = "x86_64-unknown-linux-musl";
    let manifest = manifest_file(
        &files,
        "m.json",
        version,
        vec![
            asset(gnu, force::VARIANT_DESKTOP, "gnu-desktop", &broken),
            asset(gnu, force::VARIANT_HEADLESS, "gnu-headless", &broken),
            asset(musl, force::VARIANT_HEADLESS, "musl-headless", &broken),
        ],
    );
    let api = serve_fake_github(vec![(
        format!("v{version}"),
        true,
        vec![
            (force::MANIFEST_ASSET.into(), manifest),
            ("gnu-desktop".into(), broken.clone()),
            ("gnu-headless".into(), broken.clone()),
            ("musl-headless".into(), broken),
        ],
    )])
    .await;
    let dest = home
        .path()
        .join(format!("got{}", std::env::consts::EXE_SUFFIX));
    let client = local_client();
    let attempt = |spec: OnlineSpec| {
        let (client, api, dest) = (client.clone(), api.clone(), dest.clone());
        async move {
            force::acquire_online(&client, &api, &spec, &dest)
                .await
                .unwrap_err()
        }
    };

    let e = attempt(online_spec(Some(version), musl, force::VARIANT_DESKTOP)).await;
    assert!(
        e.contains(musl) && e.contains("这个目标只有 headless（baseline）"),
        "{e}"
    );
    assert!(!dest.exists(), "找不到资产时什么都不下载");

    let mut explicit_v3 = online_spec(Some(version), gnu, force::VARIANT_HEADLESS);
    explicit_v3.cpu = Some(force::CPU_X86_64_V3.into());
    let e = attempt(explicit_v3).await;
    assert!(
        e.contains("headless（x86-64-v3）")
            && e.contains("这个目标只有 desktop（baseline）；headless（baseline）"),
        "{e}"
    );

    // gnu 的 desktop 产物跑不起来，musl 又只有 headless
    let e = attempt(online_spec(Some(version), gnu, force::VARIANT_DESKTOP)).await;
    assert!(
        e.contains("改用 musl 产物也不行") && e.contains("这个目标只有 headless（baseline）"),
        "{e}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn online_rejects_what_does_not_match() {
    let home = TestHome::new();
    let files = home.path().join("files");
    std::fs::create_dir_all(&files).unwrap();
    let real = PathBuf::from(env!("CARGO_BIN_EXE_aproxy"));
    let version = env!("CARGO_PKG_VERSION");
    let target = env!("APROXY_BUILD_TARGET");
    let variant = own_variant();
    let other_variant = if variant == force::VARIANT_DESKTOP {
        force::VARIANT_HEADLESS
    } else {
        force::VARIANT_DESKTOP
    };
    // sha256 不符的清单（比对在执行之前，与版本无关）
    let mut wrong_sha = asset(target, &variant, "aproxy-bin", &real);
    wrong_sha.sha256 = "0".repeat(64);
    let bad_sha = manifest_file(&files, "bad-sha.json", "0.2.1", vec![wrong_sha]);
    // 写着另一个版本的清单
    let wrong_version = manifest_file(
        &files,
        "wrong-version.json",
        "0.2.1",
        vec![asset(target, &variant, "aproxy-bin", &real)],
    );
    // 把本构建标成另一个变体的清单：版本对得上，下载下来自报的变体对不上
    let mislabeled = manifest_file(
        &files,
        "mislabeled.json",
        version,
        vec![asset(target, other_variant, "aproxy-bin", &real)],
    );
    let api = serve_fake_github(vec![
        (
            format!("v{version}"),
            true,
            vec![
                (force::MANIFEST_ASSET.into(), mislabeled),
                ("aproxy-bin".into(), real.clone()),
            ],
        ),
        (
            "v0.2.1".into(),
            false,
            vec![
                (force::MANIFEST_ASSET.into(), bad_sha),
                ("aproxy-bin".into(), real.clone()),
            ],
        ),
        (
            "v0.2.2".into(),
            false,
            vec![("aproxy-bin".into(), real.clone())],
        ),
        (
            "v0.2.3".into(),
            false,
            vec![(force::MANIFEST_ASSET.into(), wrong_version)],
        ),
    ])
    .await;
    let dest = home
        .path()
        .join(format!("got{}", std::env::consts::EXE_SUFFIX));
    let client = local_client();
    let attempt = |version: &'static str, variant: String| {
        let (client, api, dest) = (client.clone(), api.clone(), dest.clone());
        async move {
            force::acquire_online(
                &client,
                &api,
                &online_spec(Some(version), target, &variant),
                &dest,
            )
            .await
            .unwrap_err()
        }
    };
    let e = attempt("0.2.1", variant.clone()).await;
    assert!(e.contains("sha256"), "{e}");
    assert!(!dest.exists(), "sha256 不符的文件应已删除");
    let e = attempt(version, other_variant.to_string()).await;
    assert!(e.contains("变体") && e.contains(other_variant), "{e}");
    let e = attempt("0.2.2", variant.clone()).await;
    assert!(
        e.contains(force::MANIFEST_ASSET) && e.contains("--from"),
        "{e}"
    );
    let e = attempt("0.2.3", variant.clone()).await;
    assert!(e.contains("0.2.1") && e.contains("不符"), "{e}");
    let e = attempt("0.1.5", variant.clone()).await;
    assert!(e.contains("低于 0.2.0"), "{e}");
}
