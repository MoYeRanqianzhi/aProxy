//! install 库层诊断：continue_install 从 restarting 残局续作（接管者视角，
//! 进程内直接跑——子进程黑盒链路里看不到的卡点在此暴露）。

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// bind 试探选取可用端口（排除区间/占用者自动跳过；原子计数保证同进程
/// 并发测试不互撞）
fn free_port() -> u16 {
    use std::sync::atomic::{AtomicU16, Ordering};
    static SEQ: AtomicU16 = AtomicU16::new(0);
    let base = 28000u16 + (std::process::id() % 500) as u16 * 20;
    for i in 0..200u16 {
        let candidate = base + SEQ.fetch_add(1, Ordering::Relaxed) + i * 300;
        if std::net::TcpListener::bind(("127.0.0.1", candidate)).is_ok() {
            return candidate;
        }
    }
    panic!("200 次内未找到可用端口");
}

#[tokio::test(flavor = "current_thread")]
async fn continue_from_restarting_reclaims_instance() {
    // 测试进程内可见内部日志（tracing 默认无订阅者，输出被丢弃）
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_test_writer()
        .try_init();
    let dir = tempfile::tempdir().unwrap();
    // 进程级注入 APROXY_HOME：本文件独占测试进程（cargo test 每 target 一
    // 进程），可安全 set_var。必须设在进程 env 上——spawn_detached 只继承
    // 父进程环境，仅给直接子进程 cmd.env 注入管不到隔代 spawn（实测教训：
    // 新守护把注册表写进真实 ~/.aproxy/run，测试轮询 tempdir 永远落空）。
    // edition 2024 下 set_var 为 unsafe（本测试单线程且无并发读 env，安全）。
    unsafe { std::env::set_var("APROXY_HOME", dir.path()) };
    let home = dir.path();
    let run_dir = home.join("run");
    std::fs::create_dir_all(&run_dir).unwrap();

    let bin = aproxy::install::swap::bin_path_in(home);
    std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
    std::fs::copy(env!("CARGO_BIN_EXE_aproxy"), &bin).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    // 端口：bind 试探选取——pid 派生可能撞上 Windows 动态端口排除区间
    // （Hyper-V/WinNAT 保留块，bind 报「无权限或被系统保留」）
    let port = free_port();
    let cfg_file = home.join("cfg.toml");
    std::fs::write(
        &cfg_file,
        format!("base_url = \"https://libdiag.example.com\"\nlisten_addr = \"127.0.0.1:{port}\"\n"),
    )
    .unwrap();
    let dbg_err = std::fs::File::create(home.join("daemon-stderr.log")).unwrap();
    let mut child = Command::new(&bin)
        .args([
            "--config",
            &cfg_file.display().to_string(),
            "--daemon-child",
        ])
        .env("APROXY_HOME", home)
        .stdout(Stdio::null())
        .stderr(dbg_err)
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(500));
    println!(
        "--- 守护早期存活: {:?} stderr: {}",
        child.try_wait(),
        std::fs::read_to_string(home.join("daemon-stderr.log")).unwrap_or_default()
    );
    let old_pid = child.id();
    // 就绪：注册表出现
    let mut ready = false;
    for _ in 0..100 {
        if aproxy::daemon::registry_pids_in(&run_dir).contains(&old_pid) {
            ready = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(ready, "实例未就绪");
    // 失败诊断：dump 守护日志与启动错误
    let logs = home.join("logs");
    if let Ok(rd) = std::fs::read_dir(&logs) {
        for f in rd.flatten() {
            println!(
                "--- log {}: {}",
                f.file_name().to_string_lossy(),
                std::fs::read_to_string(f.path()).unwrap_or_default()
            );
        }
    }

    // 真实广播：实例置入 swap_phase（swapping 前的 ACK 形态）——续作 restart
    // 不得跳过它（跳过条件要求 !swap_phase）
    aproxy::install::broadcast::ack_one(&port.to_string())
        .await
        .expect("PrepareSwap 广播应成功");

    // 伪造 restarting 残局（原安装者已死 + updated_at 过期 = is_takeable 通过）
    let mut state = aproxy::install::state::InstallState::new_marking(
        env!("CARGO_PKG_VERSION"),
        aproxy::install::state::InstallSource::From,
    );
    state.phase = aproxy::install::state::InstallPhase::Restarting;
    state.instance_snapshot = vec![port.to_string()];
    state.from_path = Some(bin.display().to_string());
    state.staged_path = Some(bin.display().to_string());
    state.installer_pid = u32::MAX - 7; // 不存在的 pid（原安装者已死）
    state.updated_at =
        aproxy::watchdog::now_secs().saturating_sub(aproxy::install::state::STALE_AFTER_SECS + 60);
    aproxy::install::state::write_in(&run_dir, &mut state).unwrap();

    let t0 = Instant::now();
    let result = tokio::time::timeout(
        Duration::from_secs(60),
        aproxy::install::flow::continue_install(home, &run_dir),
    )
    .await;
    match result {
        Err(_) => panic!(
            "continue_install 60s 未返回——卡死复现（phase={:?}）",
            aproxy::install::state::load_in(&run_dir).map(|s| s.phase)
        ),
        Ok(Err(e)) => {
            // 失败诊断：dump 守护日志与启动错误
            let logs = home.join("logs");
            if let Ok(rd) = std::fs::read_dir(&logs) {
                for f in rd.flatten() {
                    println!(
                        "--- {}: {}",
                        f.file_name().to_string_lossy(),
                        std::fs::read_to_string(f.path()).unwrap_or_default()
                    );
                }
            }
            println!(
                "--- run/: {:?}",
                std::fs::read_dir(&run_dir).map(|rd| rd
                    .flatten()
                    .map(|x| x.file_name().to_string_lossy().to_string())
                    .collect::<Vec<_>>())
            );
            let startup = home.join("logs").join("startup.log");
            println!(
                "--- startup.log: {}",
                std::fs::read_to_string(&startup).unwrap_or_else(|_| "(无)".into())
            );
            panic!(
                "continue_install 失败: {e}（phase={:?}）",
                aproxy::install::state::load_in(&run_dir).map(|s| s.phase)
            );
        }
        Ok(Ok(exit)) => println!("continue_install 完成: {exit:?}，耗时 {:?}", t0.elapsed()),
    }
    // 终态：state 清理
    assert!(
        !aproxy::install::state::state_path_in(&run_dir).exists(),
        "done 后状态文件应删除"
    );
    // 实例已滚动：新 pid + swap_phase 清除（restart 真跑的证据）
    let ping = aproxy::daemon::ipc_ping(&port.to_string()).await;
    let info = ping.expect("滚动后实例应可 ping");
    assert_ne!(info.pid, old_pid, "实例应已滚动到新 pid");
    assert!(!info.swap_phase, "滚动后应退出更换阶段");
    let _ = child.kill();
}
