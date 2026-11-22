//! `aproxy::spawn::spawn_from_spec`（start、restart、restore、安装的滚动重启、看护者重拉
//! 共用的「按规格拉起并等就绪」）的端到端：新实例启动即死时立即报告，附上 startup.log 这次
//! 新增的内容，不空等到时限；正常的实例（含监听端口 0、由系统分配）等到就绪，返回它的
//! ping 应答；交接文件用完即删，密钥不出现在子进程的命令行上。
//!
//! 本文件只有一个测试：就绪判定里的 IPC 寻址用测试进程的 APROXY_HOME，它只能设在进程上，
//! 多个测试并行时会互相改掉对方的 home。

mod common;
use common::TestHome;

use std::time::Instant;

use aproxy::config::Overrides;
use aproxy::spawn::{READY_TIMEOUT, SpawnError, spawn_from_spec};
use aproxy::spec;

#[tokio::test(flavor = "current_thread")]
async fn spawn_from_spec_reports_an_early_exit_at_once_and_waits_for_a_healthy_instance() {
    let home = TestHome::new();
    // edition 2024 下 set_var 为 unsafe：本测试程序只有这一个测试，没有并发读环境的线程
    unsafe { std::env::set_var("APROXY_HOME", home.path()) };
    let run_dir = home.path().join("run");
    let exe = std::path::Path::new(common::APROXY);

    // 1. 配置文件不存在：守护子进程把原因写进 startup.log 后立即退出（正常的 `aproxy start`
    //    会在拉起之前就拦下这个错误，这里直接调用库函数绕过父进程的检查）
    let missing = home.path().join("missing-config.toml");
    let log_path = home.path().join("logs").join("spawn-test.log");
    let spec = spec::capture(Some(&missing), Overrides::default(), &log_path);
    let started = Instant::now();
    let err = spawn_from_spec(home.path(), exe, &spec, READY_TIMEOUT)
        .await
        .expect_err("配置文件不存在，实例应起不来");
    let elapsed = started.elapsed();
    assert!(
        err.excerpt(aproxy_ipc::i18n::Lang::Zh)
            .contains("配置文件不存在:")
    );
    assert!(
        err.excerpt(aproxy_ipc::i18n::Lang::En)
            .contains("Configuration file not found:")
    );
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
    // 覆盖里带一个密钥：它只该出现在交接文件与期望状态记录里
    let overrides = Overrides {
        api_key: Some("sk-SPAWNTESTSECRET-0123456789".to_string()),
        ..Overrides::default()
    };
    let spec = spec::capture(Some(&cfg), overrides, &log_path);
    let live = spawn_from_spec(home.path(), exe, &spec, READY_TIMEOUT)
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

    // 交接文件在就绪（或失败）之后已经删掉；期望状态记录按实际端口命名，钉着这个端口
    // （配置里的端口是 0），带着规格里的覆盖
    let leftovers: Vec<_> = std::fs::read_dir(&run_dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".spec.json"))
        .collect();
    assert!(leftovers.is_empty(), "交接文件应已删除: {leftovers:?}");
    let entry = aproxy::spec::read_restore_in(&run_dir, &port.to_string())
        .unwrap()
        .expect("后台实例应写下期望状态记录");
    assert_eq!(entry.spec.pinned_port, Some(port));
    assert_eq!(entry.spec.overrides, spec.overrides);
    assert_eq!(
        Some(&entry.spec.log_path),
        live.instance.log_path.as_ref(),
        "实例记录里的日志路径就是规格里的那个"
    );

    // 密钥不在实例的命令行上（Linux 上直接读 /proc；别的平台由规格的传递方式保证）
    #[cfg(target_os = "linux")]
    {
        let cmdline = std::fs::read(format!("/proc/{}/cmdline", live.instance.pid)).unwrap();
        let cmdline = String::from_utf8_lossy(&cmdline);
        assert!(cmdline.contains("__serve"), "{cmdline}");
        assert!(!cmdline.contains("SPAWNTESTSECRET"), "{cmdline}");
    }
    // 收尾交给 TestHome：按端口 stop，再按台账兜底
}
