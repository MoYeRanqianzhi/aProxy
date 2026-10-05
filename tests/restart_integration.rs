//! 实例生命周期管理端到端集成测试：restart / stop --force / restore 与
//! 看护者（watchdog）的进程身份判定。
//!
//! 覆盖：
//! - 改 config.toml 换端口后 restart——就绪判定必须按新实例 pid 在注册表
//!   定位（旧端口 ping 不到 ≠ 重启失败），成功后新端口 IPC 可达、旧端口无实例；
//! - restart 先预检后停止：坏配置/新端口被占时旧实例保持运行，restart all
//!   跳过失败者并以非零退出，新实例启动即死时立即展示 startup.log 新增内容；
//! - stop --force 清理恢复记录，强杀后不被看护者复活；
//! - 进程身份与二进制名称解耦：以官方 Release 资产名（aproxy-<target>）运行
//!   的实例照常被收养、崩溃重拉、可被 --force，并发 start 只留一个看护者；
//! - 重拉/恢复后端口变化（改配置端口未 restart、端口 0）：旧端口恢复记录
//!   被清理，stop 新端口后实例不再被复活。
//!
//! 隔离：APROXY_HOME 指向 tempdir（守护/看护子进程经 spawn 继承环境，注册表/
//! 日志/spool 全部落在 home 下的对应子目录，看护者只看得到本测试的 run/），
//! 端口从测试进程 pid 派生（独立偏移），绝不触碰生产端口。按 pid 终止进程前
//! 一律核对其镜像路径在本测试的临时目录或 cargo target 目录下。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// 从测试进程 pid 派生守护测试端口（与 proxy_integration.rs 同思路），并跳过
/// 当前不可绑定的候选：Windows 会把成片端口划进 Hyper-V/WinNAT 排除区间，落进
/// 去时守护「无法绑定」，与实现无关的假失败。步长 32 且本文件 offset 取
/// 20..=39（模 32 互不相同），保证「每个测试用不同端口」的约束在跳步后依然成立。
fn restart_test_port(offset: u32) -> u16 {
    let base = 25000 + (std::process::id() % 20000) * 2;
    for step in 0..64u32 {
        let candidate = base + offset + step * 32;
        if candidate > u16::MAX as u32 {
            break;
        }
        if std::net::TcpListener::bind(("127.0.0.1", candidate as u16)).is_ok() {
            return candidate as u16;
        }
    }
    (base + offset) as u16
}

/// 测试收尾守卫：Drop 时对测试派生端口执行 `aproxy stop`（断言失败路径也
/// 运行，杜绝守护泄漏在真实注册表）。两个端口都可能被拉起，分别守卫。
/// 本文件守护一律跑在隔离主目录：unix 的 UDS socket（home/run/ 下）与
/// 注册表都在其中，stop 须带同一 APROXY_HOME 才找得到守护（Windows 管道名
/// 全局唯一，不受影响）。
struct RestartGuard {
    exe: &'static str,
    port: u16,
    home_dir: std::path::PathBuf,
}

impl Drop for RestartGuard {
    fn drop(&mut self) {
        let _ = Command::new(self.exe)
            .args(["stop", &self.port.to_string()])
            .env("APROXY_HOME", &self.home_dir)
            .output();
    }
}

/// 对运行在隔离主目录里的守护做 IPC ping：端点属于 run 目录（unix 的 socket
/// 在 home/run/ 下，Windows 的管道名带 run 目录标识），测试进程（默认主目录）
/// 的 ipc_ping 会找错位置，须显式给目录。
fn ipc_ping_in_dir(
    rt: &tokio::runtime::Runtime,
    port: u16,
    home_dir: &std::path::Path,
) -> Result<aproxy::daemon::InstanceInfo, String> {
    rt.block_on(aproxy::daemon::ipc_ping_in(
        &home_dir.join("run"),
        &port.to_string(),
    ))
}

/// 等待端口就绪：TCP 可连 + IPC ping 确认是自家守护（端点名含端口，
/// 只有我们的 --daemon-child 子进程会创建它），最多 10 秒。
fn wait_daemon_ready(rt: &tokio::runtime::Runtime, port: u16, home_dir: &std::path::Path) -> bool {
    for _ in 0..100 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok()
            && ipc_ping_in_dir(rt, port, home_dir).is_ok()
        {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

/// 用隔离主目录启动守护子进程（直接 --daemon-child，与 start 的 spawn
/// 同一路径）；返回进程句柄。环境注入必须用 Command::env——直接 spawn 的
/// 子进程继承测试进程环境，无法逐子进程指定 APROXY_HOME。
fn spawn_daemon(
    exe: &str,
    cfg_file: &std::path::Path,
    home_dir: &std::path::Path,
) -> std::process::Child {
    Command::new(exe)
        .args([
            "--config",
            cfg_file.display().to_string().as_str(),
            "--daemon-child",
        ])
        .env("APROXY_HOME", home_dir.display().to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn 守护子进程失败")
}

// ---------------------------------------------------------------------------
// 1. 改端口后 restart：误报回归——就绪判定按新 pid 定位
// ---------------------------------------------------------------------------
#[test]
fn restart_after_port_change_reports_new_port() {
    let port_a = restart_test_port(20);
    let port_b = restart_test_port(21);
    let dir = tempfile::tempdir().unwrap();
    let home_dir = dir.path();
    let cfg_file = dir.path().join("restart.toml");
    std::fs::write(
        &cfg_file,
        format!("base_url = \"https://restart-test.example.com\"\nlisten_addr = \"127.0.0.1:{port_a}\"\n"),
    )
    .unwrap();
    let exe = env!("CARGO_BIN_EXE_aproxy");
    let guard_a = RestartGuard {
        exe,
        port: port_a,
        home_dir: home_dir.to_path_buf(),
    };
    let guard_b = RestartGuard {
        exe,
        port: port_b,
        home_dir: home_dir.to_path_buf(),
    };
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    // 启动实例（端口 A）并等就绪。守护是分离进程语义（后续 restart 会停掉
    // 它再拉起新进程），句柄立即泄漏（forget）——wait 会阻塞且无意义
    let child = spawn_daemon(exe, &cfg_file, home_dir);
    std::mem::forget(child);
    assert!(
        wait_daemon_ready(&rt, port_a, home_dir),
        "初始守护（端口 {port_a}）未就绪"
    );

    // 改配置换端口（restart 的真实用途：让新配置生效）
    std::fs::write(
        &cfg_file,
        format!("base_url = \"https://restart-test.example.com\"\nlisten_addr = \"127.0.0.1:{port_b}\"\n"),
    )
    .unwrap();

    // restart 旧端口 A：必须成功，且输出指向新端口 B。--config 显式给出
    // （避免 CLI 进程读取真实 settings.json 的 default_config）
    let out = Command::new(exe)
        .args([
            "restart",
            &port_a.to_string(),
            "--config",
            &cfg_file.display().to_string(),
        ])
        .env("APROXY_HOME", home_dir.display().to_string())
        .output()
        .unwrap();
    assert!(out.status.success(), "restart 应成功，实际 exit 非零");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains(&port_b.to_string()),
        "restart 输出应指向新端口 {port_b}，实际: {stdout}"
    );

    // 新端口 IPC 可达（新配置生效），旧端口无实例
    assert!(
        ipc_ping_in_dir(&rt, port_b, home_dir).is_ok(),
        "新端口 {port_b} 应有运行中的实例"
    );
    assert!(
        ipc_ping_in_dir(&rt, port_a, home_dir).is_err(),
        "旧端口 {port_a} 不应再有实例"
    );

    // 注册表/IPC 响应里是同一个配置文件路径（restart 用原参数拉起）。
    // 断言经 ipc_ping 的响应取信息——不用 list_instances（它附带孤儿日志
    // 清理，在 APROXY_HOME 重定向场景会误删真实 logs 目录的文件）。
    // 路径按原样字符串比对：restore 参数里的值就是初始 spawn 传入的原文。
    let live = ipc_ping_in_dir(&rt, port_b, home_dir).expect("端口 B 实例应可 ping");
    assert_eq!(
        live.config_path,
        cfg_file.display().to_string(),
        "重启后实例应指向同一配置文件"
    );

    drop(guard_a);
    drop(guard_b);
}

// ---------------------------------------------------------------------------
// 2. restart 未启动的端口：只重启不启动语义——提示未运行并退出 1
// ---------------------------------------------------------------------------
#[test]
fn restart_not_running_port_fails() {
    let port = restart_test_port(22);
    let dir = tempfile::tempdir().unwrap();
    let cfg_file = dir.path().join("dummy.toml");
    std::fs::write(&cfg_file, "base_url = \"https://x.example.com\"\n").unwrap();
    let exe = env!("CARGO_BIN_EXE_aproxy");

    let out = Command::new(exe)
        .args([
            "restart",
            &port.to_string(),
            "--config",
            &cfg_file.display().to_string(),
        ])
        .env("APROXY_HOME", dir.path().display().to_string())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "未启动的 target 应退出 1");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("没有运行中的 aProxy 实例"),
        "应提示未在运行，实际: {stdout}"
    );
}

// ---------------------------------------------------------------------------
// 以下测试共用的辅助
// ---------------------------------------------------------------------------

/// cargo 构建出的 aproxy 二进制
fn built_exe() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_aproxy"))
}

/// 把构建产物复制成官方 Release 资产的命名形态（aproxy-<target>[.exe]）——
/// 用户从 Releases 页面直接下载运行就是这个名字。进程身份判定若仍看二进制
/// 名称，这样运行的实例会完全不受看护、--force 被拒。
fn release_asset_copy(dir: &Path) -> PathBuf {
    let name = if cfg!(windows) {
        "aproxy-x86_64-pc-windows-msvc.exe"
    } else {
        "aproxy-x86_64-unknown-linux-gnu"
    };
    let bin = dir.join(name);
    std::fs::copy(built_exe(), &bin).expect("复制二进制失败");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    bin
}

/// RestartGuard 需要 &'static str 形态的可执行文件路径
fn leak_str(p: &Path) -> &'static str {
    Box::leak(p.display().to_string().into_boxed_str())
}

fn write_cfg(path: &Path, listen: &str) {
    std::fs::write(
        path,
        format!("base_url = \"https://lifecycle-test.example.com\"\nlisten_addr = \"{listen}\"\n"),
    )
    .unwrap();
}

/// 写一个 TOML 语法错误的配置（base_url 缺右引号）——「改完配置手滑」场景
fn write_broken_cfg(path: &Path, port: u16) {
    std::fs::write(
        path,
        format!(
            "base_url = \"https://lifecycle-test.example.com\nlisten_addr = \"127.0.0.1:{port}\"\n"
        ),
    )
    .unwrap();
}

/// 子进程守卫：Drop 时强杀并回收（测试自己 spawn 的进程，句柄即身份，不会
/// 误伤他人；已退出的进程 kill 失败无害，wait 负责回收 unix zombie）
struct ChildGuard(std::process::Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

/// 以隔离主目录直接启动守护子进程（--daemon-child，与 start 的 spawn 同一路径）
fn spawn_daemon_with(exe: &Path, cfg: &Path, home: &Path) -> ChildGuard {
    ChildGuard(
        Command::new(exe)
            .args([
                "--config",
                cfg.display().to_string().as_str(),
                "--daemon-child",
            ])
            .env("APROXY_HOME", home)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn 守护子进程失败"),
    )
}

/// 启动看护者（--daemon-watchdog，扫描周期 1s，只看得到隔离主目录的 run/），
/// stdout 落文件供断言它的处置日志
fn spawn_watchdog(exe: &Path, home: &Path, log: &Path) -> ChildGuard {
    let out = std::fs::File::create(log).unwrap();
    ChildGuard(
        Command::new(exe)
            .arg("--daemon-watchdog")
            .env("APROXY_HOME", home)
            .env("APROXY_WATCHDOG_SCAN_SECS", "1")
            .env("NO_COLOR", "1")
            .stdout(Stdio::from(out))
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn 看护者失败"),
    )
}

fn wait_for(timeout: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if cond() {
            return true;
        }
        if Instant::now() > deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn read_text(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

/// 日志里是否有一行同时包含全部片段（字段格式化可能夹 ANSI 码，按片段匹配）
fn log_has(log: &Path, parts: &[&str]) -> bool {
    read_text(log)
        .lines()
        .any(|l| parts.iter().all(|p| l.contains(p)))
}

fn run_cli(exe: &Path, home: &Path, args: &[&str]) -> std::process::Output {
    Command::new(exe)
        .args(args)
        .env("APROXY_HOME", home)
        .env("NO_COLOR", "1")
        .output()
        .expect("运行 aproxy CLI 失败")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn has_restore(home: &Path, port: u16) -> bool {
    home.join("run").join(format!("{port}.restore")).is_file()
}

fn has_registry(home: &Path, port: u16) -> bool {
    home.join("run").join(format!("{port}.pid")).is_file()
}

/// run/ 下全部 .restore 记录的端口
fn restore_ports(home: &Path) -> Vec<String> {
    std::fs::read_dir(home.join("run"))
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("restore"))
                .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
                .collect()
        })
        .unwrap_or_default()
}

/// 按 pid 终止本测试拉起的进程（被看护者重拉/由 start 分离出去的进程不是
/// 测试的子进程，只能按 pid 杀）。三道关：先测创建时间、再核对镜像路径在
/// 本测试的临时目录或 cargo target 目录下（绝不误伤生产实例或任何其他进程）、
/// 最后按「pid + 创建时间」核验后终止——路径核对与终止之间 pid 若被复用，
/// 创建时间对不上，终止被拒。
fn kill_own_pid(pid: u32, home: &Path) {
    let start = aproxy::watchdog::process_start_time(pid).expect("进程创建时间可查");
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let image = canon(&aproxy::watchdog::process_image_path(pid).expect("进程镜像可查"));
    let allowed = [canon(home), canon(built_exe().parent().unwrap())];
    assert!(
        allowed.iter().any(|d| image.starts_with(d)),
        "拒绝终止镜像不在测试目录下的进程 pid {pid}: {}",
        image.display()
    );
    aproxy::watchdog::terminate_verified_process(pid, start).expect("终止测试进程失败");
}

// ---------------------------------------------------------------------------
// 3. restart 预检：坏配置不碰旧实例（daemon-ipc-01）
// ---------------------------------------------------------------------------
#[test]
fn restart_with_broken_config_keeps_old_instance() {
    let port = restart_test_port(23);
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let cfg = home.join("c.toml");
    write_cfg(&cfg, &format!("127.0.0.1:{port}"));
    let exe = built_exe();
    let rt = runtime();
    let _daemon = spawn_daemon_with(&exe, &cfg, home);
    let _guard = RestartGuard {
        exe: env!("CARGO_BIN_EXE_aproxy"),
        port,
        home_dir: home.to_path_buf(),
    };
    assert!(wait_daemon_ready(&rt, port, home), "守护未就绪");
    let orig = ipc_ping_in_dir(&rt, port, home).unwrap().pid;

    write_broken_cfg(&cfg, port);
    let out = run_cli(&exe, home, &["restart", &port.to_string()]);
    let stderr = text(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "预检失败应退出 1: {stderr}");
    assert!(
        stderr.contains("预检未通过") && stderr.contains("TOML"),
        "应报出预检失败与配置错误原因: {stderr}"
    );
    let live = ipc_ping_in_dir(&rt, port, home).expect("旧实例必须仍在运行");
    assert_eq!(live.pid, orig, "预检失败不得触碰旧实例");
    assert!(
        has_restore(home, port),
        "旧实例的恢复记录必须保留（崩溃自愈依赖它）"
    );
}

// ---------------------------------------------------------------------------
// 4. restart all：逐实例预检，失败的跳过、其余照常重启，最后非零退出
// ---------------------------------------------------------------------------
#[test]
fn restart_all_skips_failed_instance_and_exits_nonzero() {
    let (port_a, port_b) = (restart_test_port(24), restart_test_port(25));
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let (cfg_a, cfg_b) = (home.join("a.toml"), home.join("b.toml"));
    write_cfg(&cfg_a, &format!("127.0.0.1:{port_a}"));
    write_cfg(&cfg_b, &format!("127.0.0.1:{port_b}"));
    let exe = built_exe();
    let rt = runtime();
    let _daemon_a = spawn_daemon_with(&exe, &cfg_a, home);
    let _daemon_b = spawn_daemon_with(&exe, &cfg_b, home);
    let _guards = [port_a, port_b].map(|port| RestartGuard {
        exe: env!("CARGO_BIN_EXE_aproxy"),
        port,
        home_dir: home.to_path_buf(),
    });
    assert!(wait_daemon_ready(&rt, port_a, home) && wait_daemon_ready(&rt, port_b, home));
    let orig_a = ipc_ping_in_dir(&rt, port_a, home).unwrap().pid;
    let orig_b = ipc_ping_in_dir(&rt, port_b, home).unwrap().pid;

    write_broken_cfg(&cfg_b, port_b);
    let out = run_cli(&exe, home, &["restart", "all"]);
    let stderr = text(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "有失败者时应非零退出: {stderr}");
    assert!(
        stderr.contains("2 个实例中有 1 个重启失败"),
        "应汇总失败实例: {stderr}"
    );
    let a = ipc_ping_in_dir(&rt, port_a, home).expect("A 应已重启并运行");
    assert_ne!(
        a.pid, orig_a,
        "配置正常的 A 应照常重启（不被 B 的失败中止）"
    );
    let b = ipc_ping_in_dir(&rt, port_b, home).expect("B 旧实例必须仍在运行");
    assert_eq!(b.pid, orig_b, "预检失败的 B 不得被停止");
}

// ---------------------------------------------------------------------------
// 5. restart 预检：换到已被占用的端口时不停旧实例
// ---------------------------------------------------------------------------
#[test]
fn restart_refuses_port_change_onto_occupied_port() {
    let (port_a, port_c) = (restart_test_port(26), restart_test_port(27));
    // 本测试自己占住目标端口（模拟被其他程序占用）
    let _hold = std::net::TcpListener::bind(("127.0.0.1", port_c)).expect("占位端口绑定失败");
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let cfg = home.join("c.toml");
    write_cfg(&cfg, &format!("127.0.0.1:{port_a}"));
    let exe = built_exe();
    let rt = runtime();
    let _daemon = spawn_daemon_with(&exe, &cfg, home);
    let _guard = RestartGuard {
        exe: env!("CARGO_BIN_EXE_aproxy"),
        port: port_a,
        home_dir: home.to_path_buf(),
    };
    assert!(wait_daemon_ready(&rt, port_a, home), "守护未就绪");
    let orig = ipc_ping_in_dir(&rt, port_a, home).unwrap().pid;

    write_cfg(&cfg, &format!("127.0.0.1:{port_c}"));
    let out = run_cli(&exe, home, &["restart", &port_a.to_string()]);
    let stderr = text(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(1),
        "新端口不可绑定应退出 1: {stderr}"
    );
    assert!(
        stderr.contains("预检未通过") && stderr.contains(&port_c.to_string()),
        "应报出新端口不可用: {stderr}"
    );
    let live = ipc_ping_in_dir(&rt, port_a, home).expect("旧实例必须仍在运行");
    assert_eq!(live.pid, orig, "预检失败不得触碰旧实例");
}

// ---------------------------------------------------------------------------
// 6. 新实例启动即死：立即报告并展示 startup.log 本次新增内容
//
// 预检之后、新实例读配置之前改坏配置，才能越过预检让新实例秒死。时间窗靠
// 一个只发了半截请求体的在途连接制造：旧实例的优雅停止会等它，测试在此期间
// 改坏配置，再主动断开连接放行停止。
// ---------------------------------------------------------------------------
#[test]
fn restart_early_exit_shows_startup_log_excerpt() {
    use std::io::Write;

    let port = restart_test_port(28);
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().to_path_buf();
    let cfg = home.join("c.toml");
    write_cfg(&cfg, &format!("127.0.0.1:{port}"));
    let exe = built_exe();
    let rt = runtime();
    let _daemon = spawn_daemon_with(&exe, &cfg, &home);
    let _guard = RestartGuard {
        exe: env!("CARGO_BIN_EXE_aproxy"),
        port,
        home_dir: home.clone(),
    };
    assert!(wait_daemon_ready(&rt, port, &home), "守护未就绪");

    let mut conn = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    conn.write_all(
        format!(
            "POST /v1/messages HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: 4096\r\n\r\n{{\"model\":"
        )
        .as_bytes(),
    )
    .unwrap();
    std::thread::sleep(Duration::from_millis(300));

    let restart = {
        let (exe, home) = (exe.clone(), home.clone());
        std::thread::spawn(move || run_cli(&exe, &home, &["restart", &port.to_string()]))
    };
    // 预检是毫秒级的；此刻旧实例正在等在途连接，新实例尚未 spawn
    std::thread::sleep(Duration::from_millis(1500));
    write_broken_cfg(&cfg, port);
    std::thread::sleep(Duration::from_millis(300));
    let released = Instant::now();
    drop(conn);
    let out = restart.join().unwrap();
    let elapsed = released.elapsed();
    let stderr = text(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "新实例起不来应退出 1: {stderr}");
    assert!(
        stderr.contains("启动后立即退出"),
        "应识别新实例提前退出: {stderr}"
    );
    assert!(
        stderr.contains("TOML"),
        "应展示 startup.log 本次新增的配置错误: {stderr}"
    );
    assert!(
        elapsed < Duration::from_secs(7),
        "提前退出应立即报告，不空等满 8 秒（放行停止后耗时 {elapsed:?}）"
    );
}

// ---------------------------------------------------------------------------
// 7. 进程身份与名称解耦（watchdog-04）+ stop --force 不被复活（daemon-ipc-02）
//
// 以官方 Release 资产名运行：实例被收养 → 崩溃后被重拉（重拉出的仍是资产名
// 镜像）→ --force 可用 → 强杀后恢复记录被清理、不被看护者复活。
// ---------------------------------------------------------------------------
#[test]
fn release_asset_named_instance_is_adopted_respawned_and_force_stoppable() {
    let port = restart_test_port(29);
    let port_s = port.to_string();
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let bin = release_asset_copy(home);
    let cfg = home.join("c.toml");
    write_cfg(&cfg, &format!("127.0.0.1:{port}"));
    let rt = runtime();
    let wd_log = home.join("watchdog.log");
    let mut daemon = spawn_daemon_with(&bin, &cfg, home);
    let _wd = spawn_watchdog(&bin, home, &wd_log);
    let _guard = RestartGuard {
        exe: leak_str(&bin),
        port,
        home_dir: home.to_path_buf(),
    };
    assert!(wait_daemon_ready(&rt, port, home), "守护未就绪");
    let orig_pid = daemon.0.id();
    assert!(
        wait_for(Duration::from_secs(15), || log_has(
            &wd_log,
            &["看护者收养实例", &port_s]
        )),
        "以资产名运行的实例应被收养；看护者日志:\n{}",
        read_text(&wd_log)
    );

    // 崩溃：强杀测试自己 spawn 的守护（句柄即身份）
    daemon.0.kill().unwrap();
    daemon.0.wait().unwrap();
    assert!(
        wait_for(Duration::from_secs(20), || ipc_ping_in_dir(&rt, port, home)
            .is_ok_and(|i| i.pid != orig_pid)),
        "看护者应重拉资产名运行的实例；看护者日志:\n{}",
        read_text(&wd_log)
    );
    let live = ipc_ping_in_dir(&rt, port, home).unwrap();
    let image = aproxy::watchdog::process_image_path(live.pid).expect("新实例镜像可查");
    assert!(
        image
            .file_name()
            .unwrap()
            .to_string_lossy()
            .eq_ignore_ascii_case(&bin.file_name().unwrap().to_string_lossy()),
        "重拉应沿用看护者自身镜像（资产名）: {}",
        image.display()
    );
    #[cfg(any(windows, target_os = "linux"))]
    assert_ne!(
        live.process_start, 0,
        "新实例应登记进程创建时间（身份锚点）"
    );

    // 等看护者确认重拉就绪、重新收编新 pid 之后再强杀：新实例的 IPC 在它
    // 起来后几毫秒内就可 ping 通，而看护者按 200ms 间隔轮询注册表确认就绪——
    // 抢在这之前强杀，测到的就成了「重拉就绪等待期间实例被停」，不是本用例
    // 要验证的「被看护的实例被强杀」
    assert!(
        wait_for(Duration::from_secs(10), || log_has(
            &wd_log,
            &["实例已重拉并就绪", &port_s]
        )),
        "看护者应确认重拉就绪；日志:\n{}",
        read_text(&wd_log)
    );

    // --force 对资产名运行的实例可用（不再因镜像名拒绝）
    let out = run_cli(&bin, home, &["stop", &port_s, "--force"]);
    let stdout = text(&out.stdout);
    assert!(
        out.status.success() && stdout.contains("已强制终止"),
        "stop --force 应成功: {stdout}{}",
        text(&out.stderr)
    );
    assert!(
        !has_restore(home, port) && !has_registry(home, port),
        "强杀后恢复记录与注册表应被清理"
    );
    // 扫描周期 1s：给看护者足够多个 tick，确认它没有把实例拉回来
    std::thread::sleep(Duration::from_secs(4));
    assert!(
        ipc_ping_in_dir(&rt, port, home).is_err(),
        "stop --force 后实例不得被看护者复活；看护者日志:\n{}",
        read_text(&wd_log)
    );
    assert!(
        log_has(&wd_log, &["已优雅退出", &port_s]),
        "看护者应按优雅退出摘除看护；日志:\n{}",
        read_text(&wd_log)
    );
}

// ---------------------------------------------------------------------------
// 8. 看护者重拉到新端口（改了配置端口但没 restart）：旧端口记录被清理，
//    stop 新端口后不再复活（watchdog-01）
// ---------------------------------------------------------------------------
#[test]
fn watchdog_respawn_onto_changed_port_retires_old_record() {
    let (port_a, port_b) = (restart_test_port(30), restart_test_port(31));
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let cfg = home.join("c.toml");
    write_cfg(&cfg, &format!("127.0.0.1:{port_a}"));
    let exe = built_exe();
    let rt = runtime();
    let wd_log = home.join("watchdog.log");
    let mut daemon = spawn_daemon_with(&exe, &cfg, home);
    let _wd = spawn_watchdog(&exe, home, &wd_log);
    let _guards = [port_a, port_b].map(|port| RestartGuard {
        exe: env!("CARGO_BIN_EXE_aproxy"),
        port,
        home_dir: home.to_path_buf(),
    });
    assert!(wait_daemon_ready(&rt, port_a, home), "守护未就绪");
    assert!(
        wait_for(Duration::from_secs(15), || log_has(
            &wd_log,
            &["看护者收养实例", &port_a.to_string()]
        )),
        "实例应被收养；看护者日志:\n{}",
        read_text(&wd_log)
    );

    write_cfg(&cfg, &format!("127.0.0.1:{port_b}"));
    daemon.0.kill().unwrap();
    daemon.0.wait().unwrap();
    assert!(
        wait_for(Duration::from_secs(20), || ipc_ping_in_dir(
            &rt, port_b, home
        )
        .is_ok()),
        "看护者应把实例重拉到新配置端口；看护者日志:\n{}",
        read_text(&wd_log)
    );
    assert!(
        wait_for(Duration::from_secs(5), || !has_restore(home, port_a)),
        "旧端口的恢复记录应被清理"
    );
    assert!(has_restore(home, port_b), "新端口应有自己的恢复记录");
    assert!(!has_registry(home, port_a), "旧端口的死注册记录应被清理");

    // 修复前：看护者凭旧端口残留的 .restore 把 stop 掉的实例再拉起来
    let out = run_cli(&exe, home, &["stop", &port_b.to_string()]);
    assert!(out.status.success(), "stop 新端口应成功");
    std::thread::sleep(Duration::from_secs(4));
    assert!(
        ipc_ping_in_dir(&rt, port_b, home).is_err() && ipc_ping_in_dir(&rt, port_a, home).is_err(),
        "stop 新端口后实例不得被复活；看护者日志:\n{}",
        read_text(&wd_log)
    );
    assert!(
        restore_ports(home).is_empty(),
        "不应残留任何恢复记录: {:?}",
        restore_ports(home)
    );
}

// ---------------------------------------------------------------------------
// 9. 端口 0（每次由系统分配）：崩溃重拉必然换端口，旧端口记录同样被清理
// ---------------------------------------------------------------------------
#[test]
fn watchdog_respawn_with_port_zero_retires_old_record() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let run = home.join("run");
    let cfg = home.join("c.toml");
    write_cfg(&cfg, "127.0.0.1:0");
    let exe = built_exe();
    let rt = runtime();
    let wd_log = home.join("watchdog.log");
    let mut daemon = spawn_daemon_with(&exe, &cfg, home);
    let _wd = spawn_watchdog(&exe, home, &wd_log);
    let orig_pid = daemon.0.id();
    let mut guards = Vec::new();
    let mut port_1 = 0u16;
    assert!(
        wait_for(Duration::from_secs(10), || {
            match aproxy::daemon::registry_find_pid_in(&run, orig_pid) {
                Some(info) => {
                    port_1 = aproxy::daemon::port_of(&info.listen_addr).parse().unwrap();
                    true
                }
                None => false,
            }
        }),
        "守护未登记"
    );
    guards.push(RestartGuard {
        exe: env!("CARGO_BIN_EXE_aproxy"),
        port: port_1,
        home_dir: home.to_path_buf(),
    });
    assert!(wait_daemon_ready(&rt, port_1, home), "守护未就绪");
    assert!(
        wait_for(Duration::from_secs(15), || log_has(
            &wd_log,
            &["看护者收养实例", &port_1.to_string()]
        )),
        "实例应被收养；看护者日志:\n{}",
        read_text(&wd_log)
    );

    daemon.0.kill().unwrap();
    daemon.0.wait().unwrap();
    let mut port_2 = 0u16;
    assert!(
        wait_for(Duration::from_secs(20), || {
            aproxy::daemon::registry_instances_in(&run)
                .into_iter()
                .find(|i| i.pid != orig_pid)
                .is_some_and(|i| {
                    port_2 = aproxy::daemon::port_of(&i.listen_addr).parse().unwrap();
                    ipc_ping_in_dir(&rt, port_2, home).is_ok()
                })
        }),
        "看护者应重拉实例；看护者日志:\n{}",
        read_text(&wd_log)
    );
    guards.push(RestartGuard {
        exe: env!("CARGO_BIN_EXE_aproxy"),
        port: port_2,
        home_dir: home.to_path_buf(),
    });
    assert_ne!(port_1, port_2, "端口 0 重拉应由系统分配到新端口");
    assert!(
        wait_for(Duration::from_secs(5), || !has_restore(home, port_1)),
        "旧端口的恢复记录应被清理"
    );
    assert!(has_restore(home, port_2), "新端口应有自己的恢复记录");

    let out = run_cli(&exe, home, &["stop", &port_2.to_string()]);
    assert!(out.status.success(), "stop 新端口应成功");
    std::thread::sleep(Duration::from_secs(4));
    assert!(
        aproxy::daemon::registry_instances_in(&run).is_empty() && restore_ports(home).is_empty(),
        "stop 后不得被复活、不得残留记录；看护者日志:\n{}",
        read_text(&wd_log)
    );
}

// ---------------------------------------------------------------------------
// 10. aproxy restore 落到新端口（改了配置端口）：报告实际端口并清理旧记录
// ---------------------------------------------------------------------------
#[test]
fn restore_onto_changed_port_retires_old_record() {
    let (port_a, port_b) = (restart_test_port(32), restart_test_port(33));
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    // 本测试不需要看护者：显式关掉，排除它参与重拉的干扰
    std::fs::write(home.join("settings.json"), r#"{"watchdog": false}"#).unwrap();
    let cfg = home.join("c.toml");
    write_cfg(&cfg, &format!("127.0.0.1:{port_a}"));
    let exe = built_exe();
    let rt = runtime();
    let mut daemon = spawn_daemon_with(&exe, &cfg, home);
    let _guards = [port_a, port_b].map(|port| RestartGuard {
        exe: env!("CARGO_BIN_EXE_aproxy"),
        port,
        home_dir: home.to_path_buf(),
    });
    assert!(wait_daemon_ready(&rt, port_a, home), "守护未就绪");
    daemon.0.kill().unwrap();
    daemon.0.wait().unwrap();
    assert!(has_restore(home, port_a), "崩溃应留下恢复记录");

    write_cfg(&cfg, &format!("127.0.0.1:{port_b}"));
    let out = run_cli(&exe, home, &["restore"]);
    let stdout = text(&out.stdout);
    assert!(
        stdout.contains("已恢复")
            && stdout.contains(&port_b.to_string())
            && stdout.contains("原端口的恢复记录已清理"),
        "restore 应报告实际端口并清理旧记录: {stdout}{}",
        text(&out.stderr)
    );
    assert!(!has_restore(home, port_a), "旧端口的恢复记录应被清理");
    assert!(has_restore(home, port_b));
    assert!(
        ipc_ping_in_dir(&rt, port_b, home).is_ok(),
        "实例应在新端口运行"
    );

    // 修复前：stop 新端口后，下一次 restore 会凭旧端口记录把它复活
    let out = run_cli(&exe, home, &["stop", &port_b.to_string()]);
    assert!(out.status.success(), "stop 新端口应成功");
    let out = run_cli(&exe, home, &["restore"]);
    assert!(
        text(&out.stdout).contains("没有需要恢复的实例"),
        "stop 后不应再有可恢复的实例: {}",
        text(&out.stdout)
    );
}

/// 收尾守卫：按 claim 终止本测试主目录里的在任看护者（镜像路径核对 +
/// pid/创建时间核验，绝不按名字杀）
struct WatchdogCleanup(PathBuf);

impl Drop for WatchdogCleanup {
    fn drop(&mut self) {
        if let Some(c) = aproxy::watchdog::read_claim_in(&self.0.join("run"))
            && aproxy::watchdog::verify_claim_identity(c.pid, c.created_at_process)
        {
            kill_own_pid(c.pid, &self.0);
        }
    }
}

// ---------------------------------------------------------------------------
// 11. 资产名二进制并发 start：选举 + claim 收敛到恰好一个在任看护者，且它
//     收养了这些实例（崩溃即重拉）
// ---------------------------------------------------------------------------
#[test]
fn concurrent_start_with_release_asset_name_converges_to_one_watchdog() {
    let ports = [34, 35, 36].map(restart_test_port);
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().to_path_buf();
    let run = home.join("run");
    let bin = release_asset_copy(&home);
    let rt = runtime();
    // Drop 逆序：先停实例（RestartGuard），再终止看护者
    let _wd_cleanup = WatchdogCleanup(home.clone());
    let _guards = ports.map(|port| RestartGuard {
        exe: leak_str(&bin),
        port,
        home_dir: home.clone(),
    });

    let starts: Vec<_> = ports
        .iter()
        .map(|port| {
            let cfg = home.join(format!("c{port}.toml"));
            write_cfg(&cfg, &format!("127.0.0.1:{port}"));
            let (bin, home) = (bin.clone(), home.clone());
            std::thread::spawn(move || {
                Command::new(&bin)
                    .args(["start", "--config", cfg.display().to_string().as_str()])
                    .env("APROXY_HOME", &home)
                    .env("APROXY_WATCHDOG_SCAN_SECS", "1")
                    .env("NO_COLOR", "1")
                    .output()
                    .unwrap()
            })
        })
        .collect();
    // start CLI 拉起看护者时打印「看护者已随实例启动拉起 … pid=N」：收集全部
    // 被拉起的看护者 pid，用来断言最终只剩一个
    let mut spawned = Vec::new();
    for h in starts {
        let out = h.join().unwrap();
        let stdout = text(&out.stdout);
        assert!(
            out.status.success() && stdout.contains("已在后台启动"),
            "start 应成功: {stdout}{}",
            text(&out.stderr)
        );
        for line in stdout
            .lines()
            .filter(|l| l.contains("看护者已随实例启动拉起"))
        {
            let digits: String = line
                .rsplit("pid=")
                .next()
                .unwrap_or("")
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            spawned.push(digits.parse::<u32>().expect("看护者 pid 可解析"));
        }
    }
    assert!(
        !spawned.is_empty(),
        "并发 start 应至少拉起一个看护者（CLI 不在注册表，必有发起权）"
    );

    // 收敛：落败者发现 claim 已被占后立即退出；只剩 claim 持有者在世
    let claim_holder = || {
        aproxy::watchdog::read_claim_in(&run)
            .filter(|c| aproxy::watchdog::verify_claim_identity(c.pid, c.created_at_process))
    };
    assert!(
        wait_for(Duration::from_secs(10), || claim_holder().is_some()),
        "应有在任看护者"
    );
    std::thread::sleep(Duration::from_secs(3));
    let holder = claim_holder().expect("在任看护者应持续在任").pid;
    let alive: Vec<u32> = spawned
        .iter()
        .copied()
        .filter(|pid| aproxy::watchdog::process_exited(*pid) == Some(false))
        .collect();
    assert_eq!(
        alive,
        vec![holder],
        "被拉起的看护者 {spawned:?} 中应恰好剩下 claim 持有者"
    );
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(
        claim_holder().map(|c| c.pid),
        Some(holder),
        "claim 不得在看护者之间翻转"
    );

    // 唯一的看护者收养了资产名运行的实例：杀掉一个，应被重拉
    let victim = ipc_ping_in_dir(&rt, ports[0], &home).unwrap().pid;
    kill_own_pid(victim, &home);
    assert!(
        wait_for(Duration::from_secs(20), || ipc_ping_in_dir(
            &rt, ports[0], &home
        )
        .is_ok_and(|i| i.pid != victim)),
        "在任看护者应重拉资产名运行的实例"
    );
}
