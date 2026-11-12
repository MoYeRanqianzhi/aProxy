//! `aproxy::spawn::spawn_instance`（start、restart、restore、安装的滚动重启、看护者重拉共用的
//! 「拉起并等就绪」）的端到端：新实例启动即死时立即报告，附上 startup.log 这次新增的内容，
//! 不空等到时限；正常的实例（含监听端口 0、由系统分配）等到就绪，返回它的 ping 应答。
//!
//! 本文件只有一个测试：库函数分离拉起的守护只继承测试进程的环境，APROXY_HOME 只能设在
//! 进程上，多个测试并行时会互相改掉对方的 home。

mod common;
use common::TestHome;

use std::time::Instant;

use aproxy::spawn::{READY_TIMEOUT, SpawnError, spawn_instance};

#[tokio::test(flavor = "current_thread")]
async fn spawn_instance_reports_an_early_exit_at_once_and_waits_for_a_healthy_instance() {
    let home = TestHome::new();
    // edition 2024 下 set_var 为 unsafe：本测试程序只有这一个测试，没有并发读环境的线程
    unsafe { std::env::set_var("APROXY_HOME", home.path()) };
    let run_dir = home.path().join("run");
    let exe = std::path::Path::new(common::APROXY);

    // 1. 配置文件不存在：守护子进程把原因写进 startup.log 后立即退出（正常的 `aproxy start`
    //    会在拉起之前就拦下这个错误，这里直接调用库函数绕过父进程的检查）
    let missing = home.path().join("missing-config.toml");
    let args = vec!["--config".to_string(), missing.display().to_string()];
    let started = Instant::now();
    let err = spawn_instance(&run_dir, exe, &args, READY_TIMEOUT)
        .await
        .expect_err("配置文件不存在，实例应起不来");
    let elapsed = started.elapsed();
    let SpawnError::Exited { startup_log, .. } = &err else {
        panic!("应识别为启动即退出，实际: {err:?}");
    };
    assert!(
        startup_log
            .lines
            .iter()
            .any(|line| line.contains("missing-config.toml")),
        "应附上 startup.log 这次新增的内容: {startup_log:?}"
    );
    // 识别提前退出靠轮询进程状态（100 毫秒一次）；上限留给进程冷启动与杀毒软件对新进程的
    // 扫描，仍远小于就绪时限
    assert!(
        elapsed < READY_TIMEOUT / 2,
        "启动即死应立即报告，不空等到时限（用了 {elapsed:?}）"
    );

    // 2. 监听端口 0：由系统分配，等到就绪并报出实际端口
    let cfg = home.path().join("port-zero.toml");
    std::fs::write(
        &cfg,
        "base_url = \"https://spawn-test.example.com\"\nlisten_addr = \"127.0.0.1:0\"\n",
    )
    .unwrap();
    let args = vec!["--config".to_string(), cfg.display().to_string()];
    let live = spawn_instance(&run_dir, exe, &args, READY_TIMEOUT)
        .await
        .expect("正常的配置应能就绪");
    let port: u16 = aproxy::daemon::port_of(&live.instance.listen_addr)
        .parse()
        .unwrap();
    home.stop_on_drop(port);
    assert_ne!(port, 0, "应报出系统分配的实际端口");
    let again = aproxy::daemon::ipc_ping_in(&run_dir, &port.to_string())
        .await
        .expect("就绪之后应能 ping 通");
    assert_eq!(again.instance.pid, live.instance.pid);
    // 收尾交给 TestHome：按端口 stop，再按台账兜底
}
