//! `aproxy restart` 端到端集成测试。
//!
//! 覆盖用户实测踩坑路径：改 config.toml 换端口后 restart——就绪判定必须按
//! 新实例 pid 在注册表定位（旧端口 ping 不到 ≠ 重启失败），成功后新端口
//! IPC 可达、旧端口无实例。
//!
//! 隔离：APROXY_RUN_DIR 指向 tempdir（守护子进程经 spawn 继承环境），
//! 端口从测试进程 pid 派生（独立偏移，避开本文件其他测试与既有守护测试），
//! 绝不触碰生产端口（12345/12349 等）。

use std::process::{Command, Stdio};
use std::time::Duration;

/// 从测试进程 pid 派生守护测试端口（与 proxy_integration.rs 同规则；
/// restart 用偏移 20/21，避开该文件的 0..=2 与看门狗的 9）。
fn restart_test_port(offset: u32) -> u16 {
    (25000 + (std::process::id() % 20000) * 2 + offset) as u16
}

/// 测试收尾守卫：Drop 时对测试派生端口执行 `aproxy stop`（断言失败路径也
/// 运行，杜绝守护泄漏在真实注册表）。两个端口都可能被拉起，分别守卫。
struct RestartGuard {
    exe: &'static str,
    port: u16,
}

impl Drop for RestartGuard {
    fn drop(&mut self) {
        let _ = Command::new(self.exe)
            .args(["stop", &self.port.to_string()])
            .output();
    }
}

/// 等待端口就绪：TCP 可连 + IPC ping 确认是自家守护（管道名含端口，
/// 只有我们的 --daemon-child 子进程会创建它），最多 10 秒。
fn wait_daemon_ready(port: u16) -> bool {
    let port_str = port.to_string();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("创建测试 tokio runtime 失败");
    for _ in 0..100 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok()
            && rt.block_on(aproxy::daemon::ipc_ping(&port_str)).is_ok()
        {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

/// 用隔离 run 目录启动守护子进程（直接 --daemon-child，与 start 的 spawn
/// 同一路径）；返回进程句柄。环境注入必须用 Command::env——直接 spawn 的
/// 子进程继承测试进程环境，无法逐子进程指定 APROXY_RUN_DIR。
fn spawn_daemon(
    exe: &str,
    cfg_file: &std::path::Path,
    run_dir: &std::path::Path,
) -> std::process::Child {
    Command::new(exe)
        .args([
            "--config",
            cfg_file.display().to_string().as_str(),
            "--daemon-child",
        ])
        .env("APROXY_RUN_DIR", run_dir.display().to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn 守护子进程失败")
}

// ---------------------------------------------------------------------------
// 1. 改端口后 restart：误报回归——就绪判定按新 pid 定位
// ---------------------------------------------------------------------------
#[test]
#[cfg(windows)]
fn restart_after_port_change_reports_new_port() {
    let port_a = restart_test_port(20);
    let port_b = restart_test_port(21);
    let dir = tempfile::tempdir().unwrap();
    let run_dir = dir.path().join("run");
    let cfg_file = dir.path().join("restart.toml");
    std::fs::write(
        &cfg_file,
        format!("base_url = \"https://restart-test.example.com\"\nlisten_addr = \"127.0.0.1:{port_a}\"\n"),
    )
    .unwrap();
    let exe = env!("CARGO_BIN_EXE_aproxy");
    let guard_a = RestartGuard { exe, port: port_a };
    let guard_b = RestartGuard { exe, port: port_b };
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    // 启动实例（端口 A）并等就绪。守护是分离进程语义（后续 restart 会停掉
    // 它再拉起新进程），句柄立即泄漏（forget）——wait 会阻塞且无意义
    let child = spawn_daemon(exe, &cfg_file, &run_dir);
    std::mem::forget(child);
    assert!(wait_daemon_ready(port_a), "初始守护（端口 {port_a}）未就绪");

    // 改配置换端口（restart 的真实用途：让新配置生效）
    std::fs::write(
        &cfg_file,
        format!("base_url = \"https://restart-test.example.com\"\nlisten_addr = \"127.0.0.1:{port_b}\"\n"),
    )
    .unwrap();

    // restart 旧端口 A：必须成功，且输出指向新端口 B。--config 显式给出
    // （避免 CLI 进程读取真实 ~/.aproxy/settings.json 的 default_config）
    let out = Command::new(exe)
        .args([
            "restart",
            &port_a.to_string(),
            "--config",
            &cfg_file.display().to_string(),
        ])
        .env("APROXY_RUN_DIR", run_dir.display().to_string())
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
        rt.block_on(aproxy::daemon::ipc_ping(&port_b.to_string()))
            .is_ok(),
        "新端口 {port_b} 应有运行中的实例"
    );
    assert!(
        rt.block_on(aproxy::daemon::ipc_ping(&port_a.to_string()))
            .is_err(),
        "旧端口 {port_a} 不应再有实例"
    );

    // 注册表/IPC 响应里是同一个配置文件路径（restart 用原参数拉起）。
    // 断言经 ipc_ping 的响应取信息——不用 list_instances（它附带孤儿日志
    // 清理，在 APROXY_RUN_DIR 重定向场景会误删真实 logs 目录的文件）。
    // 路径按原样字符串比对：restore 参数里的值就是初始 spawn 传入的原文。
    let live = rt
        .block_on(aproxy::daemon::ipc_ping(&port_b.to_string()))
        .expect("端口 B 实例应可 ping");
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
#[cfg(windows)]
fn restart_not_running_port_fails() {
    let port = restart_test_port(22);
    let dir = tempfile::tempdir().unwrap();
    let run_dir = dir.path().join("run");
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
        .env("APROXY_RUN_DIR", run_dir.display().to_string())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "未启动的 target 应退出 1");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("没有运行中的 aProxy 实例"),
        "应提示未在运行，实际: {stdout}"
    );
}
