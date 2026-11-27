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
//! 隔离：每个测试一个 common::TestHome（APROXY_HOME 指向临时目录，守护/看护子进程经
//! spawn 继承环境，注册表/日志/spool 全部落在 home 下的对应子目录，看护者只看得到本测试
//! 的 run/；看门狗开关默认关，要测 start 拉起看护者的测试显式打开），测试结束时它按端口
//! stop 登记过的实例。端口向系统要（common::free_port），绝不触碰生产端口。按 pid 终止
//! 进程前一律核对其镜像路径在本测试的临时目录或 cargo target 目录下。

mod common;
use common::CopiedExecutable;
use common::ReportExit as _;
use common::{ChildGuard, TestHome, free_port};

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

/// 对运行在隔离主目录里的守护做 IPC ping：端点属于 run 目录（unix 的 socket
/// 在 home/run/ 下，Windows 的管道名带 run 目录标识），测试进程（默认主目录）
/// 的 ipc_ping 会找错位置，须显式给目录。
fn ipc_ping_in_dir(
    rt: &tokio::runtime::Runtime,
    port: u16,
    home_dir: &std::path::Path,
) -> Result<aproxy::daemon::InstanceStatus, aproxy::daemon::IpcError> {
    rt.block_on(aproxy::daemon::ipc_ping_in(
        &home_dir.join("run"),
        &port.to_string(),
    ))
}

/// 等待端口就绪：TCP 可连 + IPC 确认自家实例已进入 serving（端点名含端口，
/// 只有我们的 --daemon-child 子进程会创建它），最多 10 秒。
fn wait_daemon_ready(rt: &tokio::runtime::Runtime, port: u16, home_dir: &std::path::Path) -> bool {
    for _ in 0..100 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok()
            && ipc_ping_in_dir(rt, port, home_dir)
                .is_ok_and(|s| s.state == aproxy::daemon::InstanceState::Serving)
        {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

/// 在隔离主目录里直接启动守护子进程（--daemon-child，与 start 的 spawn 同一路径）；
/// 返回进程句柄。
fn spawn_daemon(home: &TestHome, cfg_file: &std::path::Path) -> std::process::Child {
    home.serve_cmd(cfg_file)
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
    let port_a = free_port();
    let port_b = free_port();
    let home = TestHome::new();
    let home_dir = home.path();
    let cfg_file = home_dir.join("restart.toml");
    std::fs::write(
        &cfg_file,
        format!("base_url = \"https://restart-test.example.com\"\nlisten_addr = \"127.0.0.1:{port_a}\"\n"),
    )
    .unwrap();
    home.stop_on_drop(port_a);
    home.stop_on_drop(port_b);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    // 启动实例（端口 A）并等就绪。守护是分离进程语义（后续 restart 会停掉
    // 它再拉起新进程），句柄立即泄漏（forget）——wait 会阻塞且无意义
    let child = spawn_daemon(&home, &cfg_file);
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
    let out = home
        .cmd()
        .args(["restart", &port_a.to_string()])
        .output()
        .report_exit()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "restart 应成功，实际 {:?}；stdout: {stdout}；stderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
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
        live.instance.config_path,
        Some(cfg_file.display().to_string()),
        "重启后实例应指向同一配置文件"
    );
}

// ---------------------------------------------------------------------------
// 2. restart 未启动的端口：只重启不启动语义——报 target_not_running 并退出 4
// ---------------------------------------------------------------------------
#[test]
fn restart_not_running_port_fails() {
    let port = free_port();
    let home = TestHome::new();
    let cfg_file = home.path().join("dummy.toml");
    std::fs::write(&cfg_file, "base_url = \"https://x.example.com\"\n").unwrap();

    let out = home
        .cmd()
        .args(["restart", &port.to_string(), "--json"])
        .output()
        .report_exit()
        .unwrap();
    assert_eq!(out.status.code(), Some(4), "未启动的 target 应退出 4");
    let failure = common::output_document(&out);
    assert_eq!(failure["error"]["code"], "target_not_running");
    assert_eq!(failure["error"]["details"]["target"], port.to_string());
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

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

/// 以 exe（构建产物或它的资产名副本）在隔离主目录里直接启动守护子进程
/// （--daemon-child，与 start 的 spawn 同一路径）
fn spawn_daemon_with(home: &TestHome, exe: &Path, cfg: &Path) -> ChildGuard {
    ChildGuard(
        home.serve_cmd_with(exe, cfg)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn_ready()
            .expect("spawn 守护子进程失败"),
    )
}

/// 启动看护者（扫描周期 1s，只看得到隔离主目录的 run/），stdout 落文件供断言它的处置
/// 日志。先打开 settings 里的开关：看护者每个周期都读它，关着就退出
fn spawn_watchdog(home: &TestHome, exe: &Path, log: &Path) -> ChildGuard {
    home.enable_watchdog();
    let out = std::fs::File::create(log).unwrap();
    ChildGuard(
        home.watchdog_cmd_with(exe)
            .env("APROXY_WATCHDOG_SCAN_SECS", "1")
            .env("NO_COLOR", "1")
            .stdout(Stdio::from(out))
            .stderr(Stdio::null())
            .spawn_ready()
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

/// 以 exe 在隔离主目录里运行一条 aproxy 命令（不带颜色，便于断言输出）
fn cli(home: &TestHome, exe: &Path, args: &[&str]) -> std::process::Command {
    let mut cmd = home.cmd_with(exe);
    cmd.args(args).env("NO_COLOR", "1");
    cmd
}

fn run_cli(home: &TestHome, exe: &Path, args: &[&str]) -> std::process::Output {
    cli(home, exe, args)
        .output_ready()
        .expect("运行 aproxy CLI 失败")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn has_restore(home: &Path, port: u16) -> bool {
    home.join("run")
        .join(format!("{port}.restore.json"))
        .is_file()
}

fn has_registry(home: &Path, port: u16) -> bool {
    home.join("run")
        .join(format!("{port}.instance.json"))
        .is_file()
}

/// run/ 下全部期望状态记录的端口（读不懂的记录也算，按文件名取端口）
fn restore_ports(home: &Path) -> Vec<String> {
    let listing = aproxy::spec::list_restore_in(&home.join("run"));
    let mut ports: Vec<String> = listing.entries.into_iter().map(|e| e.port).collect();
    ports.extend(listing.unreadable.into_iter().map(|u| {
        u.path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .trim_end_matches(".restore.json")
            .to_string()
    }));
    ports.sort();
    ports
}

/// 按 pid 终止本测试拉起的进程（被看护者重拉/由 start 分离出去的进程不是
/// 测试的子进程，只能按 pid 杀）。三道关：先测创建时间、再核对镜像路径在
/// 本测试的临时目录或 cargo target 目录下（绝不误伤生产实例或任何其他进程）、
/// 最后按「pid + 创建时间」核验后终止——路径核对与终止之间 pid 若被复用，
/// 创建时间对不上，终止被拒。
fn kill_own_pid(pid: u32, home: &Path) {
    let start = aproxy::process::process_start_time(pid).expect("进程创建时间可查");
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let image = canon(&aproxy::process::process_image_path(pid).expect("进程镜像可查"));
    let allowed = [canon(home), canon(built_exe().parent().unwrap())];
    assert!(
        allowed.iter().any(|d| image.starts_with(d)),
        "拒绝终止镜像不在测试目录下的进程 pid {pid}: {}",
        image.display()
    );
    aproxy::process::terminate_verified_process(pid, start).expect("终止测试进程失败");
}

// ---------------------------------------------------------------------------
// 3. restart 预检：坏配置不碰旧实例（daemon-ipc-01）
// ---------------------------------------------------------------------------
#[test]
fn restart_with_broken_config_keeps_old_instance() {
    let port = free_port();
    let test_home = TestHome::new();
    let home = test_home.path();
    let cfg = home.join("c.toml");
    write_cfg(&cfg, &format!("127.0.0.1:{port}"));
    let exe = built_exe();
    let rt = runtime();
    let _daemon = spawn_daemon_with(&test_home, &exe, &cfg);
    test_home.stop_on_drop(port);
    assert!(wait_daemon_ready(&rt, port, home), "守护未就绪");
    let orig = ipc_ping_in_dir(&rt, port, home).unwrap().instance.pid;

    write_broken_cfg(&cfg, port);
    let out = run_cli(&test_home, &exe, &["restart", &port.to_string()]);
    let stderr = text(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(3),
        "预检发现配置无效应退出 3: {stderr}"
    );
    assert!(
        stderr.contains("preflight failed") && stderr.contains("TOML"),
        "应报出预检失败与配置错误原因: {stderr}"
    );
    let live = ipc_ping_in_dir(&rt, port, home).expect("旧实例必须仍在运行");
    assert_eq!(live.instance.pid, orig, "预检失败不得触碰旧实例");
    assert!(
        has_restore(home, port),
        "旧实例的恢复记录必须保留（崩溃自愈依赖它）"
    );
}

// ---------------------------------------------------------------------------
// 4. restart all：逐实例预检，失败的跳过、其余照常重启，最后以部分失败（5）退出
// ---------------------------------------------------------------------------
#[test]
fn restart_all_skips_failed_instance_and_exits_nonzero() {
    let (port_a, port_b) = (free_port(), free_port());
    let test_home = TestHome::new();
    let home = test_home.path();
    let (cfg_a, cfg_b) = (home.join("a.toml"), home.join("b.toml"));
    write_cfg(&cfg_a, &format!("127.0.0.1:{port_a}"));
    write_cfg(&cfg_b, &format!("127.0.0.1:{port_b}"));
    let exe = built_exe();
    let rt = runtime();
    let _daemon_a = spawn_daemon_with(&test_home, &exe, &cfg_a);
    let _daemon_b = spawn_daemon_with(&test_home, &exe, &cfg_b);
    test_home.stop_on_drop(port_a);
    test_home.stop_on_drop(port_b);
    assert!(wait_daemon_ready(&rt, port_a, home) && wait_daemon_ready(&rt, port_b, home));
    assert!(
        has_restore(home, port_a) && has_restore(home, port_b),
        "两个后台实例都应在就绪前写好恢复记录"
    );
    let orig_a = ipc_ping_in_dir(&rt, port_a, home).unwrap().instance.pid;
    let orig_b = ipc_ping_in_dir(&rt, port_b, home).unwrap().instance.pid;

    write_broken_cfg(&cfg_b, port_b);
    let out = run_cli(&test_home, &exe, &["restart", "all"]);
    let stderr = text(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(5),
        "有成有败时应以 5 退出: {stderr}"
    );
    assert!(
        stderr.contains("Restart failed for 1 of the 2 instances attempted."),
        "应汇总失败实例: {stderr}"
    );
    let a = ipc_ping_in_dir(&rt, port_a, home).expect("A 应已重启并运行");
    assert_ne!(
        a.instance.pid, orig_a,
        "配置正常的 A 应照常重启（不被 B 的失败中止）"
    );
    let b = ipc_ping_in_dir(&rt, port_b, home).expect("B 旧实例必须仍在运行");
    assert_eq!(b.instance.pid, orig_b, "预检失败的 B 不得被停止");
}

// ---------------------------------------------------------------------------
// 5. restart 预检：换到已被占用的端口时不停旧实例
// ---------------------------------------------------------------------------
#[test]
fn restart_refuses_port_change_onto_occupied_port() {
    // 本测试自己占住目标端口（模拟被其他程序占用）
    let hold = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("占位端口绑定失败");
    let port_c = hold.local_addr().unwrap().port();
    let port_a = free_port();
    let test_home = TestHome::new();
    let home = test_home.path();
    let cfg = home.join("c.toml");
    write_cfg(&cfg, &format!("127.0.0.1:{port_a}"));
    let exe = built_exe();
    let rt = runtime();
    let _daemon = spawn_daemon_with(&test_home, &exe, &cfg);
    test_home.stop_on_drop(port_a);
    assert!(wait_daemon_ready(&rt, port_a, home), "守护未就绪");
    let orig = ipc_ping_in_dir(&rt, port_a, home).unwrap().instance.pid;

    write_cfg(&cfg, &format!("127.0.0.1:{port_c}"));
    let out = run_cli(&test_home, &exe, &["restart", &port_a.to_string()]);
    let stderr = text(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(1),
        "新端口不可绑定应退出 1: {stderr}"
    );
    assert!(
        stderr.contains("preflight failed") && stderr.contains(&port_c.to_string()),
        "应报出新端口不可用: {stderr}"
    );
    let live = ipc_ping_in_dir(&rt, port_a, home).expect("旧实例必须仍在运行");
    assert_eq!(live.instance.pid, orig, "预检失败不得触碰旧实例");
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

    let port = free_port();
    let test_home = TestHome::new();
    let home = test_home.path().to_path_buf();
    let cfg = home.join("c.toml");
    write_cfg(&cfg, &format!("127.0.0.1:{port}"));
    let exe = built_exe();
    let rt = runtime();
    let _daemon = spawn_daemon_with(&test_home, &exe, &cfg);
    test_home.stop_on_drop(port);
    assert!(wait_daemon_ready(&rt, port, &home), "守护未就绪");

    let mut conn = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    conn.write_all(
        format!(
            "POST /v1/messages HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: 4096\r\n\r\n{{\"model\":"
        )
        .as_bytes(),
    )
    .unwrap();
    assert!(
        wait_for(Duration::from_secs(20), || ipc_ping_in_dir(
            &rt, port, &home
        )
        .is_ok_and(|s| s.activity.in_flight == 1)),
        "旧实例应已接收尚未读完的请求"
    );

    let restart = {
        // Command 在本线程里建好再交给子线程：它带着 TestHome 注入的环境
        let mut cmd = cli(&test_home, &exe, &["restart", &port.to_string(), "--json"]);
        std::thread::spawn(move || cmd.output_ready().expect("运行 aproxy CLI 失败"))
    };
    // stopping 只在通过预检并送达 shutdown 后才可见。半截请求仍在旧实例中，
    // 改坏配置后才释放它，因而错误一定发生在接任进程，而不是发起者的预检里。
    assert!(
        wait_for(Duration::from_secs(20), || ipc_ping_in_dir(
            &rt, port, &home
        )
        .is_ok_and(|s| s.state == aproxy::daemon::InstanceState::Stopping)),
        "旧实例没有进入停止阶段"
    );
    write_broken_cfg(&cfg, port);
    drop(conn);
    let out = restart.join().unwrap();
    let stderr = text(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "新实例起不来应退出 1: {stderr}");
    assert!(
        stderr.contains("exited immediately after startup"),
        "应识别新实例提前退出: {stderr}"
    );
    assert!(
        stderr.contains("TOML"),
        "应展示 startup.log 本次新增的配置错误: {stderr}"
    );
    // 提前退出与等到就绪期限用的是不同的码；不把机器的调度速度当作判据。
    let result = common::output_document(&out);
    let error = &result["results"][0]["error"];
    assert_eq!(error["code"], "start_failed");
    assert!(
        error["details"]["startup_log"]
            .as_array()
            .unwrap()
            .iter()
            .any(|line| line.as_str().is_some_and(|s| s.contains("TOML")))
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
    let port = free_port();
    let port_s = port.to_string();
    let test_home = TestHome::new();
    let home = test_home.path();
    let bin = release_asset_copy(home);
    let cfg = home.join("c.toml");
    write_cfg(&cfg, &format!("127.0.0.1:{port}"));
    let rt = runtime();
    let wd_log = home.join("watchdog.log");
    let mut daemon = spawn_daemon_with(&test_home, &bin, &cfg);
    let _wd = spawn_watchdog(&test_home, &bin, &wd_log);
    test_home.stop_on_drop_with(&bin, port);
    assert!(wait_daemon_ready(&rt, port, home), "守护未就绪");
    let orig_pid = daemon.0.id();
    assert!(
        wait_for(Duration::from_secs(15), || log_has(
            &wd_log,
            &["Watchdog adopted instance", &port_s]
        )),
        "以资产名运行的实例应被收养；看护者日志:\n{}",
        read_text(&wd_log)
    );

    // 崩溃：强杀测试自己 spawn 的守护（句柄即身份）
    daemon.0.kill().unwrap();
    daemon.0.wait().report_exit().unwrap();
    assert!(
        wait_for(Duration::from_secs(20), || ipc_ping_in_dir(&rt, port, home)
            .is_ok_and(|i| i.instance.pid != orig_pid)),
        "看护者应重拉资产名运行的实例；看护者日志:\n{}",
        read_text(&wd_log)
    );
    let live = ipc_ping_in_dir(&rt, port, home).unwrap();
    let image = aproxy::process::process_image_path(live.instance.pid).expect("新实例镜像可查");
    assert!(
        image
            .file_name()
            .unwrap()
            .to_string_lossy()
            .eq_ignore_ascii_case(&bin.file_name().unwrap().to_string_lossy()),
        "重拉应沿用看护者自身镜像（资产名）: {}",
        image.display()
    );
    #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
    assert!(
        !live.instance.start.is_empty(),
        "新实例应登记启动标记（身份锚点）"
    );

    // 等看护者确认重拉就绪、重新收编新 pid 之后再强杀：新实例的 IPC 在它
    // 起来后几毫秒内就可 ping 通，而看护者按 200ms 间隔轮询注册表确认就绪——
    // 抢在这之前强杀，测到的就成了「重拉就绪等待期间实例被停」，不是本用例
    // 要验证的「被看护的实例被强杀」
    assert!(
        wait_for(Duration::from_secs(10), || log_has(
            &wd_log,
            &["Respawned instance is ready", &port_s]
        )),
        "看护者应确认重拉就绪；日志:\n{}",
        read_text(&wd_log)
    );

    // --force 对资产名运行的实例可用（不再因镜像名拒绝）
    let out = run_cli(&test_home, &bin, &["stop", &port_s, "--force", "--json"]);
    let stdout = text(&out.stdout);
    assert!(
        out.status.success() && common::output_document(&out)["results"][0]["outcome"] == "killed",
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
        log_has(&wd_log, &["Instance stopped", &port_s]),
        "看护者应按「已被停止」摘除看护；日志:\n{}",
        read_text(&wd_log)
    );
}

// ---------------------------------------------------------------------------
// 8. 看护者重拉到新端口（改了配置端口但没 restart）：旧端口记录被清理，
//    stop 新端口后不再复活（watchdog-01）
// ---------------------------------------------------------------------------
#[test]
fn watchdog_respawn_onto_changed_port_retires_old_record() {
    let (port_a, port_b) = (free_port(), free_port());
    let test_home = TestHome::new();
    let home = test_home.path();
    let cfg = home.join("c.toml");
    write_cfg(&cfg, &format!("127.0.0.1:{port_a}"));
    let exe = built_exe();
    let rt = runtime();
    let wd_log = home.join("watchdog.log");
    let mut daemon = spawn_daemon_with(&test_home, &exe, &cfg);
    let _wd = spawn_watchdog(&test_home, &exe, &wd_log);
    test_home.stop_on_drop(port_a);
    test_home.stop_on_drop(port_b);
    assert!(wait_daemon_ready(&rt, port_a, home), "守护未就绪");
    assert!(
        wait_for(Duration::from_secs(15), || log_has(
            &wd_log,
            &["Watchdog adopted instance", &port_a.to_string()]
        )),
        "实例应被收养；看护者日志:\n{}",
        read_text(&wd_log)
    );

    write_cfg(&cfg, &format!("127.0.0.1:{port_b}"));
    daemon.0.kill().unwrap();
    daemon.0.wait().report_exit().unwrap();
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
    let out = run_cli(&test_home, &exe, &["stop", &port_b.to_string()]);
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
// 9. 端口 0（由系统分配）：第一次拿到的端口钉在期望状态记录里，崩溃重拉回到同一个端口
//    （客户端配的就是它），run/ 里始终只有一份期望状态记录
// ---------------------------------------------------------------------------
#[test]
fn watchdog_respawn_with_port_zero_returns_to_the_same_port() {
    let test_home = TestHome::new();
    let home = test_home.path();
    let run = home.join("run");
    let cfg = home.join("c.toml");
    write_cfg(&cfg, "127.0.0.1:0");
    let exe = built_exe();
    let rt = runtime();
    let wd_log = home.join("watchdog.log");
    let mut daemon = spawn_daemon_with(&test_home, &exe, &cfg);
    let _wd = spawn_watchdog(&test_home, &exe, &wd_log);
    let orig_pid = daemon.0.id();
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
    test_home.stop_on_drop(port_1);
    assert!(wait_daemon_ready(&rt, port_1, home), "守护未就绪");
    assert!(
        wait_for(Duration::from_secs(15), || log_has(
            &wd_log,
            &["Watchdog adopted instance", &port_1.to_string()]
        )),
        "实例应被收养；看护者日志:\n{}",
        read_text(&wd_log)
    );

    // 直接 __serve 的交接文件不带启动说明，与旧拉起方相同。
    let direct = ipc_ping_in_dir(&rt, port_1, home).unwrap().instance;
    assert_eq!(direct.launch_reason, "unknown");
    assert!(direct.launch_note.is_empty());
    daemon.0.kill().unwrap();
    let exit = daemon.0.wait().report_exit().unwrap();
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
    test_home.stop_on_drop(port_2);
    let relaunched = ipc_ping_in_dir(&rt, port_2, home).unwrap().instance;
    assert_eq!(relaunched.launch_reason, "watchdog");
    let expected = if cfg!(windows) {
        format!("previous process exited with code {}", exit.code().unwrap())
    } else {
        "previous process exited".into()
    };
    assert_eq!(relaunched.launch_note, expected);
    assert_eq!(port_1, port_2, "端口 0 的实例重拉后应回到钉住的端口");
    assert_eq!(
        restore_ports(home),
        vec![port_1.to_string()],
        "run/ 里应始终只有一份期望状态记录"
    );

    let out = run_cli(&test_home, &exe, &["stop", &port_2.to_string()]);
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
    let (port_a, port_b) = (free_port(), free_port());
    let test_home = TestHome::new();
    let home = test_home.path();
    // 本测试不需要看护者：TestHome 默认已关看门狗，排除它参与重拉的干扰
    let cfg = home.join("c.toml");
    write_cfg(&cfg, &format!("127.0.0.1:{port_a}"));
    let exe = built_exe();
    let rt = runtime();
    let mut daemon = spawn_daemon_with(&test_home, &exe, &cfg);
    test_home.stop_on_drop(port_a);
    test_home.stop_on_drop(port_b);
    assert!(wait_daemon_ready(&rt, port_a, home), "守护未就绪");
    daemon.0.kill().unwrap();
    daemon.0.wait().report_exit().unwrap();
    assert!(has_restore(home, port_a), "崩溃应留下恢复记录");

    write_cfg(&cfg, &format!("127.0.0.1:{port_b}"));
    let out = run_cli(&test_home, &exe, &["restore"]);
    let stdout = text(&out.stdout);
    assert!(
        stdout.contains("Restored pid")
            && stdout.contains(&port_b.to_string())
            && stdout.contains("the record now belongs to the new port"),
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
    let out = run_cli(&test_home, &exe, &["stop", &port_b.to_string()]);
    assert!(out.status.success(), "stop 新端口应成功");
    let out = run_cli(&test_home, &exe, &["restore", "--json"]);
    assert!(
        out.status.success() && common::output_document(&out)["results"] == serde_json::json!([]),
        "stop 后不应再有可恢复的实例: {}",
        text(&out.stdout)
    );
}

/// 收尾守卫：按看护者记录终止本测试主目录里的在任看护者（镜像路径核对 +
/// pid/创建时间核验，绝不按名字杀）
struct WatchdogCleanup(PathBuf);

impl Drop for WatchdogCleanup {
    fn drop(&mut self) {
        if let aproxy::watchdog::RecordRead::Readable(record) =
            aproxy::watchdog::read_record_in(&self.0.join("run"))
            && record.owner_alive()
        {
            kill_own_pid(record.pid, &self.0);
        }
    }
}

// ---------------------------------------------------------------------------
// 11. 资产名二进制并发 start：选举 + 看护者记录收敛到恰好一个在任看护者，且它
//     收养了这些实例（崩溃即重拉）
// ---------------------------------------------------------------------------
#[test]
fn concurrent_start_with_release_asset_name_converges_to_one_watchdog() {
    let ports = [(); 3].map(|_| free_port());
    let test_home = TestHome::new();
    let home = test_home.path().to_path_buf();
    let run = home.join("run");
    // 本测试要 start 按看门狗开关拉起看护者：显式打开（TestHome 默认关）
    test_home.settings(r#"{"watchdog": {"enabled": true}}"#);
    let bin = release_asset_copy(&home);
    let rt = runtime();
    for port in ports {
        test_home.stop_on_drop_with(&bin, port);
    }
    // 收尾顺序：先终止看护者（本守卫比 test_home 后声明，先析构），再由 test_home 按端口
    // 停实例。stop 会删掉实例的恢复记录，看护者在不在都不会把它们拉回来；实例的自检要
    // 300 秒才补种看护者，测试收尾期间不会发生
    let _wd_cleanup = WatchdogCleanup(home.clone());

    let starts: Vec<_> = ports
        .iter()
        .map(|port| {
            let cfg = home.join(format!("c{port}.toml"));
            write_cfg(&cfg, &format!("127.0.0.1:{port}"));
            // Command 在本线程里建好再交给子线程：它带着 TestHome 注入的环境
            let mut cmd = cli(
                &test_home,
                &bin,
                &[
                    "start",
                    "--json",
                    "--config",
                    cfg.display().to_string().as_str(),
                ],
            );
            cmd.env("APROXY_WATCHDOG_SCAN_SECS", "1")
                .env("RUST_LOG", "info");
            std::thread::spawn(move || cmd.output_ready().unwrap())
        })
        .collect();
    // start CLI 拉起看护者时向 stderr 写 INFO「看护者已随实例启动拉起 … pid=N」：收集全部
    // 被拉起的看护者 pid，用来断言最终只剩一个
    let mut spawned = Vec::new();
    for h in starts {
        let out = h.join().unwrap();
        let stdout = text(&out.stdout);
        let stderr = text(&out.stderr);
        assert!(
            out.status.success() && common::output_document(&out)["outcome"] == "started",
            "start 应成功: {stdout}{stderr}"
        );
        for line in stderr
            .lines()
            .filter(|l| l.contains("Started the watchdog with the instance"))
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

    // 收敛：落败者发现看护者记录已被占后立即退出；只剩记录的属主在世
    let claim_holder = || match aproxy::watchdog::read_record_in(&run) {
        aproxy::watchdog::RecordRead::Readable(record) if record.owner_alive() => Some(record),
        _ => None,
    };
    assert!(
        wait_for(Duration::from_secs(10), || claim_holder().is_some()),
        "应有在任看护者"
    );
    let holder = claim_holder().expect("在任看护者应持续在任");
    let asset = std::fs::canonicalize(&bin).unwrap();
    // 只观察这批拉起的进程。PID 若已复用于别的测试的程序，不是本次选举的落败者；
    // 镜像暂时不可读则保守地继续等，不能单凭读失败认定进程已退出。
    let contenders = || {
        spawned
            .iter()
            .copied()
            .filter(|pid| {
                aproxy::process::process_exited(*pid) != Some(true)
                    && aproxy::process::process_image_path(*pid)
                        .and_then(|path| std::fs::canonicalize(path).ok())
                        .is_none_or(|path| path == asset)
            })
            .collect::<Vec<_>>()
    };
    assert!(
        wait_for(Duration::from_secs(15), || contenders() == [holder.pid]),
        "看护者选举未收敛：spawned={spawned:?}, live={:?}, holder={holder:?}",
        contenders()
    );
    let after = claim_holder().expect("选举收敛后在任记录应存在");
    assert!(after.same_owner(&holder));
    // 收敛后同一属主确实又完成了一次扫描，才验证收养和重拉；固定等待并不能证明运行过扫描。
    assert!(
        wait_for(Duration::from_secs(15), || claim_holder().is_some_and(
            |now| {
                assert!(
                    now.same_owner(&holder),
                    "看护者记录不得在看护者之间翻转：{now:?}"
                );
                now.last_scan_at > after.last_scan_at
            }
        )),
        "收敛后未观察到同一看护者的下一次扫描：{:?}",
        claim_holder()
    );

    // 唯一的看护者收养了资产名运行的实例：杀掉一个，应被重拉
    let victim = ipc_ping_in_dir(&rt, ports[0], &home).unwrap().instance.pid;
    kill_own_pid(victim, &home);
    assert!(
        wait_for(Duration::from_secs(20), || ipc_ping_in_dir(
            &rt, ports[0], &home
        )
        .is_ok_and(|i| i.instance.pid != victim)),
        "在任看护者应重拉资产名运行的实例"
    );
}

// ---------------------------------------------------------------------------
// 拉起规格与期望状态记录（按规格拉起、记录只有 stop 能删）
// ---------------------------------------------------------------------------

/// 读一个端口的期望状态记录里的拉起规格
fn restore_spec(home: &Path, port: u16) -> Option<aproxy::spec::InstanceSpec> {
    aproxy::spec::read_restore_in(&home.join("run"), &port.to_string())
        .expect("期望状态记录应读得懂")
        .map(|entry| entry.spec)
}

/// 一个只会回 200 的 HTTP 服务，数它收到了多少个请求。每个连接读完请求头再应答、应答后
/// 关闭（不读就关会让 Windows 把应答连同 RST 一起丢掉）。返回它的地址与计数器。
fn counting_http_server() -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = hits.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let counter = counter.clone();
            std::thread::spawn(move || {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let mut seen = Vec::new();
                let mut buf = [0u8; 1024];
                while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut buf) {
                        Ok(0) | Err(_) => return,
                        Ok(n) => seen.extend_from_slice(&buf[..n]),
                    }
                }
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let _ = stream.write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok",
                );
            });
        }
    });
    (addr, hits)
}

/// 经实例发一个 GET，返回整个应答的文本
fn http_get_via(port: u16, path: &str) -> String {
    use std::io::{Read, Write};
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nhost: 127.0.0.1:{port}\r\nconnection: close\r\n\r\n"
    )
    .unwrap();
    let mut out = String::new();
    let _ = stream.read_to_string(&mut out);
    out
}

/// 一个进程的完整命令行（给人看的那种：别的用户列进程时看到的就是它）
fn process_command_line(pid: u32) -> String {
    #[cfg(windows)]
    let out = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &format!("(Get-CimInstance Win32_Process -Filter 'ProcessId={pid}').CommandLine"),
        ])
        .output()
        .report_exit()
        .expect("运行 powershell 失败");
    #[cfg(unix)]
    let out = std::process::Command::new("ps")
        .args(["-ww", "-o", "args=", "-p", &pid.to_string()])
        .output()
        .report_exit()
        .expect("运行 ps 失败");
    text(&out.stdout).trim().to_string()
}

/// 启动者环境里的代理变量记在拉起规格里：restart 是在另一个环境（没有这个变量）里执行的，
/// 重拉出来的实例仍走同一个代理。只设大写的变量名——Windows 上变量名不分大小写，规格里
/// 记的就是大写的那一个。
#[test]
fn restart_keeps_the_launchers_proxy_environment() {
    let test_home = TestHome::new();
    let home = test_home.path();
    let exe = built_exe();
    let (proxy_addr, hits) = counting_http_server();
    let proxy_url = format!("http://{proxy_addr}");
    let port = free_port();
    let cfg = home.join("via-proxy.toml");
    // 上游是一个解析不了的名字：请求只有经代理才到得了那个计数服务
    std::fs::write(
        &cfg,
        format!("base_url = \"http://upstream.invalid\"\nlisten_addr = \"127.0.0.1:{port}\"\n"),
    )
    .unwrap();
    test_home.stop_on_drop(port);

    let out = cli(
        &test_home,
        &exe,
        &["start", "--config", cfg.to_str().unwrap()],
    )
    .env("HTTP_PROXY", &proxy_url)
    .output_ready()
    .unwrap();
    assert!(out.status.success(), "start: {}", text(&out.stderr));
    let spec = restore_spec(home, port).expect("后台实例应有期望状态记录");
    assert_eq!(
        spec.env.get("HTTP_PROXY"),
        Some(&Some(proxy_url.clone())),
        "规格应记下启动者的代理变量: {:?}",
        spec.env
    );
    assert!(http_get_via(port, "/first").starts_with("HTTP/1.1 200"));
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1);

    // restart 的环境里没有 HTTP_PROXY（TestHome 的命令一律删掉代理变量）
    let old_pid = restore_pid(home, port);
    let out = run_cli(&test_home, &exe, &["restart", &port.to_string()]);
    assert!(out.status.success(), "restart: {}", text(&out.stderr));
    assert_ne!(restore_pid(home, port), old_pid, "应是新进程");
    assert!(http_get_via(port, "/second").starts_with("HTTP/1.1 200"));
    assert_eq!(
        hits.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "重启后的实例应仍经启动者当时的代理出去"
    );
}

/// 端口上应答控制通道的实例的 pid
fn restore_pid(home: &Path, port: u16) -> u32 {
    ipc_ping_in_dir(&runtime(), port, home)
        .expect("实例应在线")
        .instance
        .pid
}

/// 密钥只经拉起规格的交接文件交给实例：实例与看护者的命令行上都没有它。
#[test]
fn secrets_never_reach_the_instance_or_watchdog_command_line() {
    let test_home = TestHome::new();
    test_home.settings(r#"{"watchdog": {"enabled": true}}"#);
    let home = test_home.path();
    let exe = built_exe();
    let port = free_port();
    let cfg = home.join("secret.toml");
    write_cfg(&cfg, &format!("127.0.0.1:{port}"));
    test_home.stop_on_drop(port);

    const SECRET: &str = "sk-CMDLINESECRET-0123456789abcdef";
    let out = cli(
        &test_home,
        &exe,
        &[
            "start",
            "--api-key",
            SECRET,
            "--config",
            cfg.to_str().unwrap(),
        ],
    )
    .env("APROXY_WATCHDOG_SCAN_SECS", "1")
    .output_ready()
    .unwrap();
    assert!(out.status.success(), "start: {}", text(&out.stderr));

    // 规格里有它（期望状态记录只有本用户能读），命令行上没有
    let spec = restore_spec(home, port).unwrap();
    assert_eq!(spec.overrides.api_key.as_deref(), Some(SECRET));
    let instance_pid = restore_pid(home, port);
    let cmdline = process_command_line(instance_pid);
    assert!(cmdline.contains("__serve"), "实例的命令行: {cmdline}");
    assert!(
        !cmdline.contains("CMDLINESECRET"),
        "实例的命令行: {cmdline}"
    );

    // 看护者：从台账里找到它
    let mut watchdog_pid = None;
    assert!(
        wait_for(Duration::from_secs(10), || {
            watchdog_pid = aproxy_ipc::ledger::read_all_in(home)
                .into_iter()
                .find(|found| found.entry.role == aproxy_ipc::ledger::ROLE_WATCHDOG)
                .map(|found| found.entry.pid);
            watchdog_pid.is_some()
        }),
        "start 应拉起看护者"
    );
    let cmdline = process_command_line(watchdog_pid.unwrap());
    assert!(cmdline.contains("__watchdog"), "看护者的命令行: {cmdline}");
    assert!(
        !cmdline.contains("CMDLINESECRET"),
        "看护者的命令行: {cmdline}"
    );

    // 看护者重拉出来的实例同样没有：崩溃一次再看
    kill_own_pid(instance_pid, home);
    assert!(
        wait_for(Duration::from_secs(20), || {
            ipc_ping_in_dir(&runtime(), port, home).is_ok_and(|i| i.instance.pid != instance_pid)
        }),
        "看护者应重拉实例"
    );
    let cmdline = process_command_line(restore_pid(home, port));
    assert!(cmdline.contains("__serve"), "重拉实例的命令行: {cmdline}");
    assert!(
        !cmdline.contains("CMDLINESECRET"),
        "重拉实例的命令行: {cmdline}"
    );
}

/// 配置文件暂时找不到（网络盘还没挂上）时，restore 不删记录；文件回来之后再执行就恢复。
#[test]
fn restore_keeps_the_record_while_the_config_is_missing() {
    let test_home = TestHome::new();
    let home = test_home.path();
    let exe = built_exe();
    let rt = runtime();
    let port = free_port();
    let cfg = home.join("late-mount.toml");
    write_cfg(&cfg, &format!("127.0.0.1:{port}"));
    test_home.stop_on_drop(port);
    let mut daemon = spawn_daemon_with(&test_home, &exe, &cfg);
    assert!(wait_daemon_ready(&rt, port, home), "实例未就绪");
    assert!(wait_for(Duration::from_secs(5), || has_restore(home, port)));
    daemon.0.kill().unwrap();
    daemon.0.wait().report_exit().unwrap();

    let parked = home.join("late-mount.toml.away");
    std::fs::rename(&cfg, &parked).unwrap();
    let out = run_cli(&test_home, &exe, &["restore"]);
    let stderr = text(&out.stderr);
    assert!(
        stderr.contains("not found") && stderr.contains("record is retained"),
        "应说明配置文件找不到、记录保留: {stderr}"
    );
    assert!(has_restore(home, port), "配置文件暂时不在时不得删记录");
    assert!(ipc_ping_in_dir(&rt, port, home).is_err());

    std::fs::rename(&parked, &cfg).unwrap();
    let out = run_cli(&test_home, &exe, &["restore", "--json"]);
    assert!(
        out.status.success()
            && common::output_document(&out)["results"][0]["outcome"] == "restored",
        "文件回来之后应能恢复: {}{}",
        text(&out.stdout),
        text(&out.stderr)
    );
    assert!(ipc_ping_in_dir(&rt, port, home).is_ok(), "实例应已恢复");
}

/// 没有配置文件、只靠覆盖旗标与 settings.json 全局默认启动的实例：规格里的配置路径为
/// null，restart 与 restore 都不因为「没有配置文件」而拒绝它。
#[test]
fn instance_without_a_config_file_restarts_and_restores() {
    let test_home = TestHome::new();
    test_home.settings(
        r#"{"watchdog": {"enabled": false},
            "defaults": {"base_url": "https://defaults-only.example.com"}}"#,
    );
    let home = test_home.path();
    let exe = built_exe();
    let rt = runtime();
    let port = free_port();
    let listen = format!("127.0.0.1:{port}");
    test_home.stop_on_drop(port);
    assert!(!home.join("config.toml").exists());

    let out = run_cli(&test_home, &exe, &["start", "--listen-addr", &listen]);
    assert!(out.status.success(), "start: {}", text(&out.stderr));
    assert!(
        text(&out.stdout).contains("no configuration file"),
        "{}",
        text(&out.stdout)
    );
    let spec = restore_spec(home, port).expect("应有期望状态记录");
    assert_eq!(spec.config_path, None);
    assert_eq!(spec.overrides.listen_addr.as_deref(), Some(listen.as_str()));
    assert!(
        !home.join("config.toml").exists(),
        "启动不得凭空创建配置文件"
    );

    // restart
    let first = restore_pid(home, port);
    let out = run_cli(&test_home, &exe, &["restart", &port.to_string()]);
    assert!(out.status.success(), "restart: {}", text(&out.stderr));
    let second = restore_pid(home, port);
    assert_ne!(first, second);

    // 崩溃后 restore
    kill_own_pid(second, home);
    assert!(wait_for(Duration::from_secs(5), || {
        ipc_ping_in_dir(&rt, port, home).is_err()
    }));
    let out = run_cli(&test_home, &exe, &["restore", "--json"]);
    assert!(
        out.status.success()
            && common::output_document(&out)["results"][0]["outcome"] == "restored",
        "restore: {}{}",
        text(&out.stdout),
        text(&out.stderr)
    );
    let live = ipc_ping_in_dir(&rt, port, home).expect("实例应已恢复");
    assert_eq!(live.instance.base_url, "https://defaults-only.example.com");
}

/// stop 的含义是「它不该在运行」：目标已经崩溃、只剩期望状态记录时，stop 删掉记录，
/// 之后 restore 没有可拉的东西。按端口、按配置文件、`all` 三种写法都如此。
#[test]
fn stop_of_a_crashed_instance_deletes_its_record() {
    let test_home = TestHome::new();
    let home = test_home.path();
    let exe = built_exe();
    let rt = runtime();
    let port = free_port();
    let cfg = home.join("crashed.toml");
    write_cfg(&cfg, &format!("127.0.0.1:{port}"));
    test_home.stop_on_drop(port);
    let port_s = port.to_string();

    for target in [port_s.as_str(), cfg.to_str().unwrap(), "all"] {
        let mut daemon = spawn_daemon_with(&test_home, &exe, &cfg);
        assert!(wait_daemon_ready(&rt, port, home), "实例未就绪");
        assert!(wait_for(Duration::from_secs(5), || has_restore(home, port)));
        daemon.0.kill().unwrap();
        daemon.0.wait().report_exit().unwrap();
        assert!(has_restore(home, port), "崩溃应留下期望状态记录");

        let out = run_cli(&test_home, &exe, &["stop", target]);
        assert!(
            out.status.success(),
            "stop {target}: {}{}",
            text(&out.stdout),
            text(&out.stderr)
        );
        assert!(
            text(&out.stdout).contains("desired-state record"),
            "stop {target} 应说明删了记录: {}",
            text(&out.stdout)
        );
        assert!(!has_restore(home, port), "stop {target} 应删掉记录");
        let out = run_cli(&test_home, &exe, &["restore", "--json"]);
        assert!(
            out.status.success()
                && common::output_document(&out)["results"] == serde_json::json!([]),
            "stop {target} 之后 restore 不该再拉它: {}",
            text(&out.stdout)
        );
        assert!(ipc_ping_in_dir(&rt, port, home).is_err());
    }
}

/// 前台实例不写期望状态记录（关掉终端就是用户要它停下），也就没有可用来重启它的规格：
/// restart 明确拒绝，旧实例不受影响。
#[test]
fn foreground_instance_has_no_record_and_restart_refuses_it() {
    let test_home = TestHome::new();
    let home = test_home.path();
    let exe = built_exe();
    let rt = runtime();
    let port = free_port();
    let cfg = home.join("fg.toml");
    write_cfg(&cfg, &format!("127.0.0.1:{port}"));
    test_home.stop_on_drop(port);
    let fg = ChildGuard(
        cli(
            &test_home,
            &exe,
            &["start", "--config", cfg.to_str().unwrap(), "--foreground"],
        )
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap(),
    );
    assert!(wait_daemon_ready(&rt, port, home), "前台实例未就绪");
    assert!(!has_restore(home, port), "前台实例不写期望状态记录");

    let out = run_cli(&test_home, &exe, &["restart", &port.to_string()]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        text(&out.stderr).contains("foreground"),
        "应说明前台实例不能这样重启: {}",
        text(&out.stderr)
    );
    assert_eq!(restore_pid(home, port), fg.0.id(), "旧实例应原样在跑");
}

/// 端口 0 的实例第一次拿到的端口钉在规格里；配置后来改成了固定端口，restart 以配置为准，
/// 钉住的端口作废，旧端口的记录由新端口的记录接替。
#[test]
fn pinned_port_gives_way_once_the_config_names_a_port() {
    let test_home = TestHome::new();
    let home = test_home.path();
    let run = home.join("run");
    let exe = built_exe();
    let rt = runtime();
    let cfg = home.join("pinned.toml");
    write_cfg(&cfg, "127.0.0.1:0");
    let daemon = spawn_daemon_with(&test_home, &exe, &cfg);
    let pid = daemon.0.id();
    let mut first = 0u16;
    assert!(
        wait_for(Duration::from_secs(10), || {
            aproxy::daemon::registry_find_pid_in(&run, pid).is_some_and(|info| {
                first = aproxy::daemon::port_of(&info.listen_addr).parse().unwrap();
                true
            })
        }),
        "实例未登记"
    );
    test_home.stop_on_drop(first);
    assert!(wait_daemon_ready(&rt, first, home), "实例未就绪");
    assert!(wait_for(Duration::from_secs(5), || has_restore(
        home, first
    )));
    assert_eq!(restore_spec(home, first).unwrap().pinned_port, Some(first));

    // 配置不变时 restart：回到钉住的端口
    let out = run_cli(&test_home, &exe, &["restart", &first.to_string()]);
    assert!(out.status.success(), "restart: {}", text(&out.stderr));
    assert!(
        ipc_ping_in_dir(&rt, first, home).is_ok(),
        "应回到钉住的端口"
    );
    assert_eq!(restore_ports(home), vec![first.to_string()]);

    // 配置改成固定端口
    let fixed = free_port();
    test_home.stop_on_drop(fixed);
    write_cfg(&cfg, &format!("127.0.0.1:{fixed}"));
    let out = run_cli(&test_home, &exe, &["restart", &first.to_string()]);
    assert!(out.status.success(), "restart: {}", text(&out.stderr));
    assert!(
        ipc_ping_in_dir(&rt, fixed, home).is_ok(),
        "应监听配置里的端口"
    );
    assert!(ipc_ping_in_dir(&rt, first, home).is_err());
    assert_eq!(
        restore_ports(home),
        vec![fixed.to_string()],
        "旧端口的记录应由新端口的记录接替"
    );
    assert_eq!(restore_spec(home, fixed).unwrap().pinned_port, None);
}

/// 实例脱离启动者：对启动它的那个进程组发 SIGINT（终端里按 Ctrl+C 就是这样），实例仍在。
#[cfg(unix)]
#[test]
fn instance_survives_sigint_to_the_launchers_process_group() {
    use std::os::unix::process::CommandExt;
    let test_home = TestHome::new();
    let home = test_home.path();
    let exe = built_exe();
    let rt = runtime();
    let port = free_port();
    let cfg = home.join("detached.toml");
    write_cfg(&cfg, &format!("127.0.0.1:{port}"));
    test_home.stop_on_drop(port);

    // start 自成一个进程组：不脱离的话，它拉起的实例会留在这个组里
    let mut start = cli(
        &test_home,
        &exe,
        &["start", "--config", cfg.to_str().unwrap()],
    );
    start
        .process_group(0)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = start.spawn().unwrap();
    let pgid = child.id();
    assert!(
        child.wait().report_exit().unwrap().success(),
        "start 应成功"
    );
    assert!(wait_daemon_ready(&rt, port, home), "实例未就绪");
    let pid = restore_pid(home, port);

    // 组里已经没有进程时 kill 会报错，这正是期望的结果，不看它的退出码
    let _ = std::process::Command::new("kill")
        .args(["-INT", "--", &format!("-{pgid}")])
        .status()
        .report_exit();
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(
        ipc_ping_in_dir(&rt, port, home)
            .map(|i| i.instance.pid)
            .ok(),
        Some(pid),
        "发给启动者进程组的 SIGINT 不该停掉实例"
    );
}

/// settings.json 里把看护者关掉：在任的看护者在下一个扫描周期退出，期望状态记录不动。
#[test]
fn watchdog_exits_once_it_is_disabled_in_settings() {
    let test_home = TestHome::new();
    let home = test_home.path();
    let exe = built_exe();
    let rt = runtime();
    let port = free_port();
    let cfg = home.join("wd-off.toml");
    write_cfg(&cfg, &format!("127.0.0.1:{port}"));
    test_home.stop_on_drop(port);
    let _daemon = spawn_daemon_with(&test_home, &exe, &cfg);
    assert!(wait_daemon_ready(&rt, port, home), "实例未就绪");
    let wd_log = home.join("watchdog.log");
    let mut wd = spawn_watchdog(&test_home, &exe, &wd_log);
    assert!(
        wait_for(Duration::from_secs(15), || log_has(
            &wd_log,
            &["Watchdog adopted instance", &port.to_string()]
        )),
        "实例应被收养；看护者日志:
{}",
        read_text(&wd_log)
    );

    test_home.settings(r#"{"watchdog": {"enabled": false}}"#);
    assert!(
        wait_for(Duration::from_secs(10), || wd
            .0
            .try_wait()
            .report_exit()
            .unwrap()
            .is_some()),
        "开关关掉后看护者应退出；看护者日志:
{}",
        read_text(&wd_log)
    );
    assert!(has_restore(home, port), "期望状态记录不归看护者删");
    assert!(
        !home.join("run").join("watchdog.json").exists(),
        "退出的看护者应删掉自己的记录"
    );
    assert!(ipc_ping_in_dir(&rt, port, home).is_ok(), "实例不受影响");
}

/// spool 按进程分目录：实例只用 `spool/<pid>-<启动标记短哈希>/`。同一端口上第二次启动的
/// 进程（会因端口被占而失败）不碰正在运行的实例的缓存文件；已退出的进程留下的目录在实例
/// 开始服务之后被清掉；实例正常停止时收掉自己的目录。
#[test]
fn spool_is_per_process_and_survives_a_second_start_on_the_same_port() {
    let test_home = TestHome::new();
    let home = test_home.path();
    let exe = built_exe();
    let rt = runtime();
    let port = free_port();
    let cfg = home.join("spool.toml");
    write_cfg(&cfg, &format!("127.0.0.1:{port}"));
    test_home.stop_on_drop(port);

    // 一个早已不在的进程留下的目录
    let spool = home.join("spool");
    let stale = spool.join(format!("{}-0a0b0c0d", u32::MAX - 783));
    std::fs::create_dir_all(&stale).unwrap();
    std::fs::write(stale.join("req-1-1.spooltmp"), "x").unwrap();

    let out = run_cli(
        &test_home,
        &exe,
        &["start", "--config", cfg.to_str().unwrap()],
    );
    assert!(out.status.success(), "start: {}", text(&out.stderr));
    let pid = ipc_ping_in_dir(&rt, port, home).unwrap().instance.pid;
    let own = spool.join(aproxy::daemon::spool_dir_name(
        pid,
        aproxy_ipc::ledger::start_marker(pid).as_deref(),
    ));
    assert!(
        own.is_dir(),
        "实例应建出自己的 spool 目录: {}",
        own.display()
    );
    assert!(
        wait_for(Duration::from_secs(10), || !stale.exists()),
        "已退出的进程留下的 spool 目录应在实例开始服务之后被清掉"
    );

    // 模拟一个在途请求的缓存文件，再在同一端口上启动第二次
    let in_flight = own.join("spool-7-7.spooltmp");
    std::fs::write(&in_flight, "in flight").unwrap();
    let out = run_cli(
        &test_home,
        &exe,
        &["start", "--config", cfg.to_str().unwrap()],
    );
    assert!(
        ipc_ping_in_dir(&rt, port, home).is_ok_and(|s| s.instance.pid == pid),
        "原实例应仍在: {}{}",
        text(&out.stdout),
        text(&out.stderr)
    );
    assert!(in_flight.exists(), "第二次启动不得清掉运行中实例的缓存文件");

    let out = run_cli(&test_home, &exe, &["stop", &port.to_string()]);
    assert!(out.status.success(), "stop: {}", text(&out.stderr));
    assert!(
        wait_for(Duration::from_secs(10), || !own.exists()),
        "实例正常停止后应收掉自己的 spool 目录"
    );
}

/// 实例记录被删掉之后（有人清了 run 目录），实例在下一次核对时把它补写回来，内容与原来
/// 一致；没有记录的实例 status 看不见、看护者也不收养。
#[test]
fn instance_rewrites_its_record_after_it_is_deleted() {
    let test_home = TestHome::new();
    let home = test_home.path();
    let exe = built_exe();
    let rt = runtime();
    let port = free_port();
    let cfg = home.join("rewrite.toml");
    write_cfg(&cfg, &format!("127.0.0.1:{port}"));
    test_home.stop_on_drop(port);

    let out = run_cli(
        &test_home,
        &exe,
        &["start", "--config", cfg.to_str().unwrap()],
    );
    assert!(out.status.success(), "start: {}", text(&out.stderr));
    let run_dir = home.join("run");
    let before = aproxy::daemon::read_instance_file_in(&run_dir, &port.to_string())
        .expect("实例应已写下记录");
    assert!(!before.foreground && !before.instance_id.is_empty());
    assert_eq!(
        ipc_ping_in_dir(&rt, port, home).unwrap().instance,
        before,
        "快照里的 instance 就是实例记录"
    );

    std::fs::remove_file(aproxy::daemon::instance_file_path_in(
        &run_dir,
        &port.to_string(),
    ))
    .unwrap();
    assert!(
        wait_for(Duration::from_secs(45), || has_registry(home, port)),
        "实例应在下一次核对时补写记录"
    );
    assert_eq!(
        aproxy::daemon::read_instance_file_in(&run_dir, &port.to_string()),
        Some(before)
    );
}

/// `stop idle N`：闲置不到 N 秒的实例不停；闲置够久的停掉、连同期望状态记录一起删。
/// 判断由实例自己做（shutdown 的 `if_idle_secs`），应答里说得出它实际闲置了多久。
#[test]
fn stop_idle_only_stops_instances_that_are_idle_long_enough() {
    let test_home = TestHome::new();
    let home = test_home.path();
    let exe = built_exe();
    let rt = runtime();
    let port = free_port();
    let cfg = home.join("idle.toml");
    write_cfg(&cfg, &format!("127.0.0.1:{port}"));
    test_home.stop_on_drop(port);

    let out = run_cli(
        &test_home,
        &exe,
        &["start", "--config", cfg.to_str().unwrap()],
    );
    assert!(out.status.success(), "start: {}", text(&out.stderr));
    let live = ipc_ping_in_dir(&rt, port, home).unwrap();
    assert!(
        live.features.iter().any(|f| f == "shutdown.if_idle_secs"),
        "{:?}",
        live.features
    );

    // 刚启动的实例闲置不到一小时：不在选中之列，照常运行
    let out = run_cli(
        &test_home,
        &exe,
        &["stop", "idle", "--idle-threshold-secs", "3600"],
    );
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(
        ipc_ping_in_dir(&rt, port, home).is_ok(),
        "不够闲的实例不得被停"
    );
    assert!(has_restore(home, port));

    // 条件由实例判断：直接发带条件的 shutdown，它拒绝并报出实际闲置时长
    let err = rt
        .block_on(aproxy::daemon::ipc_request_in(
            &home.join("run"),
            &port.to_string(),
            aproxy::daemon::IpcOp::Shutdown {
                keep_restore: false,
                if_idle_secs: Some(3600),
            },
        ))
        .unwrap_err();
    assert!(
        matches!(&err, aproxy::daemon::IpcError::Remote { code, details, .. }
            if code == "condition_not_met" && details["idle_ms"].is_u64()),
        "{err}"
    );
    assert!(
        has_restore(home, port),
        "被拒的 shutdown 不得删期望状态记录"
    );

    // 阈值 0：任何实例都算闲置，停掉并删记录
    let out = run_cli(
        &test_home,
        &exe,
        &["stop", "idle", "--idle-threshold-secs", "0", "--json"],
    );
    assert!(
        out.status.success() && common::output_document(&out)["results"][0]["outcome"] == "stopped",
        "{}{}",
        text(&out.stdout),
        text(&out.stderr)
    );
    assert!(ipc_ping_in_dir(&rt, port, home).is_err());
    assert!(!has_restore(home, port), "stop idle 是 stop：记录一并删掉");
}

/// 在 `endpoint` 上冒充一个只懂别的协议版本的实例：读一行请求，回一行 `reply`，循环。
/// 在调用方的运行时里跑，随运行时结束。
async fn answer_every_request_with(
    endpoint: String,
    reply: &'static str,
    ready: tokio::sync::oneshot::Sender<()>,
) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    async fn serve<S>(stream: S, reply: &str)
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    {
        let (reader, mut writer) = tokio::io::split(stream);
        let mut line = String::new();
        let _ = BufReader::new(reader).read_line(&mut line).await;
        let _ = writer.write_all(reply.as_bytes()).await;
        let _ = writer.write_all(b"\n").await;
        let _ = writer.flush().await;
    }
    #[cfg(windows)]
    {
        use tokio::net::windows::named_pipe::ServerOptions;
        let mut server = ServerOptions::new()
            .first_pipe_instance(true)
            .create(&endpoint)
            .expect("建冒充端点失败");
        let _ = ready.send(());
        loop {
            server.connect().await.expect("等待连接失败");
            let connected = std::mem::replace(
                &mut server,
                ServerOptions::new()
                    .create(&endpoint)
                    .expect("重建端点失败"),
            );
            serve(connected, reply).await;
        }
    }
    #[cfg(unix)]
    {
        let listener = tokio::net::UnixListener::bind(&endpoint).expect("建冒充端点失败");
        let _ = ready.send(());
        loop {
            let (stream, _) = listener.accept().await.expect("等待连接失败");
            serve(stream, reply).await;
        }
    }
}

/// 启动前的 IPC 拒绝与不可信应答都不能触发强制终止、删除期望记录或读取旧日志。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn incompatible_ping_refuses_targeted_commands_and_preserves_restore() {
    let home = TestHome::new();
    let run = home.path().join("run");
    let cfg = home.path().join("target.toml");
    let log = home.path().join("old.log");
    std::fs::write(&log, "old log").unwrap();
    let port = free_port();
    let p = port.to_string();
    write_cfg(&cfg, &format!("127.0.0.1:{port}"));
    let spec = aproxy::spec::capture(Some(&cfg), Default::default(), &log);
    aproxy::spec::write_restore_in(&run, &p, &spec).unwrap();
    let path = aproxy::spec::restore_path_in(&run, &p);
    let before = std::fs::read(&path).unwrap();
    let record = aproxy::daemon::InstanceRecord {
        pid: u32::MAX,
        start: String::new(),
        instance_id: "disk".into(),
        version: "0.2.0".into(),
        listen_addr: format!("127.0.0.1:{port}"),
        config_path: Some(cfg.display().to_string()),
        base_url: String::new(),
        started_at: 1,
        log_path: Some(log.display().to_string()),
        foreground: false,
        launch_reason: String::new(),
        launch_note: String::new(),
    };
    aproxy::daemon::write_instance_file_in(&run, &record).unwrap();
    for (reply, reason) in [
        (r#"{"v":2,"ok":true}"#, "the reply could not be trusted"),
        (
            r#"{"v":1,"ok":false,"error":{"code":"unknown_op","message":"ping"}}"#,
            "ping rejected: unknown_op",
        ),
        (
            r#"{"v":2,"ok":false,"error":{"code":"unsupported_version","message":"v2","details":{"supported":[2]}}}"#,
            "ping rejected: unsupported_version",
        ),
    ] {
        let (ready, started) = tokio::sync::oneshot::channel();
        let endpoint = aproxy::daemon::endpoint_for_in(&run, &p);
        let server = tokio::spawn(answer_every_request_with(endpoint.clone(), reply, ready));
        started.await.unwrap();
        for target in [p.as_str(), cfg.to_str().unwrap()] {
            for mut args in [
                vec!["stop", target],
                vec!["stop", target, "--force"],
                vec!["restart", target],
                vec!["restart", target, "--format", "all"],
            ] {
                args.push("--json");
                let out = run_cli(&home, &built_exe(), &args);
                assert_eq!(out.status.code(), Some(1), "{args:?}: {out:?}");
                let doc: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
                let error = &doc["results"][0]["error"];
                assert_eq!(error["code"], "instance_incompatible", "{doc}");
                assert_eq!(
                    error["details"],
                    serde_json::json!({"port":port,"reason":reason})
                );
            }
            let out = run_cli(&home, &built_exe(), &["logs", target]);
            assert_eq!(out.status.code(), Some(1), "{out:?}");
            assert!(out.stdout.is_empty());
            assert!(text(&out.stderr).contains("its reply could not be accepted"));
        }
        assert_eq!(std::fs::read(&path).unwrap(), before);
        server.abort();
        assert!(server.await.unwrap_err().is_cancelled());
        #[cfg(unix)]
        std::fs::remove_file(endpoint).unwrap();
    }
}

/// 只读清点把四种实例分清：应答的（live）、进程在却不应答的（unresponsive）、应答了但
/// 协议版本不同的（incompatible）、只剩期望状态记录的（down）。清点前后 `run/` 里的
/// 文件一个不少。
#[test]
fn enumeration_tells_the_four_kinds_of_instance_apart() {
    use aproxy::ops::enumerate::{Liveness, enumerate_in};
    let test_home = TestHome::new();
    let home = test_home.path();
    let exe = built_exe();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let run_dir = home.join("run");

    // live：一个真实例
    let live_port = free_port();
    let cfg = home.join("live.toml");
    write_cfg(&cfg, &format!("127.0.0.1:{live_port}"));
    test_home.stop_on_drop(live_port);
    let out = run_cli(
        &test_home,
        &exe,
        &["start", "--config", cfg.to_str().unwrap()],
    );
    assert!(out.status.success(), "start: {}", text(&out.stderr));
    let live_record =
        aproxy::daemon::read_instance_file_in(&run_dir, &live_port.to_string()).unwrap();

    // unresponsive：记录里的进程在（拿本测试进程顶替），它的控制通道上没有人听
    let hung_port = free_port();
    let own = std::process::id();
    let mut hung_record = live_record.clone();
    hung_record.pid = own;
    hung_record.start = aproxy_ipc::ledger::start_marker(own).expect("本进程的启动标记可查");
    hung_record.listen_addr = format!("127.0.0.1:{hung_port}");
    aproxy::daemon::write_instance_file_in(&run_dir, &hung_record).unwrap();

    // incompatible：端点上应答的是只懂 v2 的实例。它留下的记录本版本读不懂
    let alien_port = free_port();
    std::fs::write(
        run_dir.join(format!("{alien_port}.instance.json")),
        r#"{"schema":2,"shape":"new"}"#,
    )
    .unwrap();
    let (ready, started) = tokio::sync::oneshot::channel();
    rt.spawn(answer_every_request_with(
        aproxy::daemon::endpoint_for_in(&run_dir, &alien_port.to_string()),
        r#"{"v":2,"ok":false,"error":{"code":"unsupported_version","message":"this instance speaks v2","details":{"supported":[2]}}}"#,
        ready,
    ));
    // 冒充端点绑定之后才发就绪信号。Unix 上端点是 run 目录里的 socket 文件，要等它建好
    // 再记下「清点前」的文件列表，否则这个文件会被当成清点建出来的
    rt.block_on(started).unwrap();

    // down：只剩一份期望状态记录，配置文件已经不在
    let down_port = free_port();
    let mut spec = restore_spec(home, live_port).expect("真实例的期望状态记录");
    spec.config_path = Some(home.join("gone.toml").display().to_string());
    aproxy::spec::write_restore_in(&run_dir, &down_port.to_string(), &spec).unwrap();

    let listing = |dir: &Path| {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    };
    let before = listing(&run_dir);
    let found = rt.block_on(enumerate_in(home));
    assert_eq!(listing(&run_dir), before, "清点不删、不建任何文件");

    let of = |port: u16| {
        found
            .instances
            .iter()
            .find(|item| item.port == port)
            .unwrap_or_else(|| panic!("端口 {port} 应被列出：{found:?}"))
    };
    assert_eq!(found.instances.len(), 4, "{found:?}");

    let live = of(live_port);
    assert_eq!(live.liveness, Liveness::Live);
    assert_eq!(live.instance.as_ref(), Some(&live_record));
    let status = live.status.as_ref().expect("应答的实例带实时状态");
    assert_eq!(status.state, aproxy::daemon::InstanceState::Serving);
    assert!(status.ops.iter().any(|op| op == "ping"));
    assert!(!live.restore_pending, "在运行的实例没有什么等着恢复");

    let hung = of(hung_port);
    assert_eq!(hung.liveness, Liveness::Unresponsive);
    assert_eq!(hung.instance.as_ref(), Some(&hung_record));
    assert!(hung.status.is_none() && hung.reason.is_none());

    let alien = of(alien_port);
    assert_eq!(alien.liveness, Liveness::Incompatible);
    assert!(alien.instance.is_none() && alien.status.is_none());
    let reason = alien.reason.as_ref().expect("说明为什么不兼容");
    assert_eq!(reason.code, "unsupported_version");
    assert_eq!(reason.details["supported"][0], 2);

    let down = of(down_port);
    assert_eq!(down.liveness, Liveness::Down);
    assert!(down.instance.is_none() && down.restore_pending);
    assert_eq!(down.restore_reason.as_deref(), Some("config_missing"));

    // 读不懂的那份实例记录照实报告，日志清理因此暂停
    assert_eq!(found.unreadable.len(), 1, "{:?}", found.unreadable);
    assert_eq!(
        found.unreadable[0].file,
        format!("run/{alien_port}.instance.json")
    );
    assert_eq!(found.unreadable[0].reason.code, "record_schema_too_new");
    assert_eq!(
        found
            .gc_paused
            .as_ref()
            .map(|paused| paused.reason.as_str()),
        Some("unreadable_records")
    );
    // 真实例在台账里，但它已经列为实例，不在「其余进程」里重复出现
    assert!(
        found
            .processes
            .iter()
            .all(|process| process.pid != live_record.pid),
        "{:?}",
        found.processes
    );

    // 文档里的 JSON 形状：四种取值都是小写的词
    let json = serde_json::to_value(&found).unwrap();
    let mut kinds: Vec<&str> = json["instances"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["liveness"].as_str().unwrap())
        .collect();
    kinds.sort_unstable();
    assert_eq!(kinds, ["down", "incompatible", "live", "unresponsive"]);

    // 收尾：顶替用的记录指向本测试进程，不能留给 TestHome 的清理去「停」它
    std::fs::remove_file(aproxy::daemon::instance_file_path_in(
        &run_dir,
        &hung_port.to_string(),
    ))
    .unwrap();
}
