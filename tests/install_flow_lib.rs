//! install 库层诊断：continue_install 从 restarting 残局续作（接管者视角，
//! 进程内直接跑——子进程黑盒链路里看不到的卡点在此暴露）。

use std::path::Path;
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

// ---------------------------------------------------------------------------
// 恢复矩阵崩溃注入（无实例快路径形态——覆盖纯文件舞的各中断点；有实例的
// 续作由上方测试覆盖）。
// ---------------------------------------------------------------------------

use aproxy::install::state::{InstallPhase, InstallSource, InstallState};

/// 残局构造：phase + 过期时间 + 死 pid（is_takeable 通过）+ from/staged 路径。
fn crash_state(phase: InstallPhase, from: &Path, staged: Option<&Path>) -> InstallState {
    let mut state = InstallState::new_marking(env!("CARGO_PKG_VERSION"), InstallSource::From);
    state.phase = phase;
    state.from_path = Some(from.display().to_string());
    state.staged_path = staged.map(|p| p.display().to_string());
    state.installer_pid = u32::MAX - 7;
    state.updated_at =
        aproxy::watchdog::now_secs().saturating_sub(aproxy::install::state::STALE_AFTER_SECS + 60);
    state
}

fn usable_from(home: &Path) -> std::path::PathBuf {
    let from = home.join("download.exe");
    std::fs::copy(env!("CARGO_BIN_EXE_aproxy"), &from).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&from, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    from
}

/// 恢复矩阵行「downloading 中」：staging 半截文件 → 删半截重下 → done。
#[tokio::test(flavor = "current_thread")]
async fn crash_during_downloading_redownloads() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let run_dir = home.join("run");
    std::fs::create_dir_all(&run_dir).unwrap();
    let from = usable_from(home);
    // 半截 staged（损坏的截断文件——备料会整体重做覆盖它）
    let staged = aproxy::install::staging::staging_dir_in(home, env!("CARGO_PKG_VERSION"))
        .join(aproxy::install::staging::binary_name());
    std::fs::create_dir_all(staged.parent().unwrap()).unwrap();
    std::fs::write(&staged, b"truncated").unwrap();
    let state = crash_state(InstallPhase::Downloading, &from, Some(&staged));
    aproxy::install::state::write_in(&run_dir, &mut state.clone()).unwrap();

    aproxy::install::flow::continue_install(home, &run_dir)
        .await
        .expect("downloading 中断续作应完成");
    assert!(
        !aproxy::install::state::state_path_in(&run_dir).exists(),
        "续作应走到 done 清状态"
    );
    assert!(bin_works(home), "重备料后规范位置应有可用二进制");
}

/// 恢复矩阵行「swapping 中（Windows bin 空窗：旧已改名、新未落位）」——
/// 计划标注的最要命失败态：bin 只有 .old（+残留 .new），staging 完整。
/// 续作从 staging 重新落位 → bin 恢复 → done。
#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn crash_during_swapping_recovers_from_staging() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let run_dir = home.join("run");
    std::fs::create_dir_all(&run_dir).unwrap();
    let from = usable_from(home);

    // 空窗现场：bin 缺失、.old 在场（可用旧版）、.new 半成品、staging 完整
    let bin = aproxy::install::swap::bin_path_in(home);
    let old = aproxy::install::swap::old_path_in(home);
    let new_tmp = aproxy::install::swap::new_tmp_path_in(home);
    std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
    std::fs::copy(env!("CARGO_BIN_EXE_aproxy"), &old).unwrap();
    std::fs::write(&new_tmp, b"half-written-new").unwrap();
    let staged = aproxy::install::staging::staging_dir_in(home, env!("CARGO_PKG_VERSION"))
        .join(aproxy::install::staging::binary_name());
    std::fs::create_dir_all(staged.parent().unwrap()).unwrap();
    std::fs::copy(env!("CARGO_BIN_EXE_aproxy"), &staged).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let state = crash_state(InstallPhase::Swapping, &from, Some(&staged));
    aproxy::install::state::write_in(&run_dir, &mut state.clone()).unwrap();

    aproxy::install::flow::continue_install(home, &run_dir)
        .await
        .expect("swapping 空窗续作应完成");
    // 终态：bin 恢复可用、.new 消费、.old 清理（无实例 → cleaning 可删）
    assert!(bin_works(home), "空窗后 bin 应从 staging 恢复");
    assert!(!new_tmp.exists(), "残留 .new 应被 swap 舞消费");
    assert!(!old.exists(), ".old 应在 cleaning 删除（无实例无镜像锁）");
    assert!(!aproxy::install::state::state_path_in(&run_dir).exists());
}

/// 恢复矩阵行「swapped」：bin=新、.old 残留 → 续作直接进尾部（滚动/终验/
/// 清理）→ done。
#[tokio::test(flavor = "current_thread")]
async fn crash_after_swap_finishes_tail() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let run_dir = home.join("run");
    std::fs::create_dir_all(&run_dir).unwrap();
    let from = usable_from(home);
    // swapped 现场：bin=新（可用）、.old 残留
    let bin = aproxy::install::swap::bin_path_in(home);
    let old = aproxy::install::swap::old_path_in(home);
    std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
    std::fs::copy(env!("CARGO_BIN_EXE_aproxy"), &bin).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    std::fs::write(&old, b"old-binary").unwrap();
    let mut state = crash_state(InstallPhase::Swapped, &from, None);
    state.old_path = Some(old.display().to_string());
    aproxy::install::state::write_in(&run_dir, &mut state).unwrap();

    aproxy::install::flow::continue_install(home, &run_dir)
        .await
        .expect("swapped 残留续作应完成");
    assert!(bin_works(home));
    assert!(!old.exists(), "尾部 cleaning 应删除 .old");
    assert!(!aproxy::install::state::state_path_in(&run_dir).exists());
}

/// 恢复矩阵行「任何阶段：状态文件损坏」→ 按无/失效处理，continue 静默退出
/// （不 crash 不留半成品）。
#[tokio::test(flavor = "current_thread")]
async fn corrupted_state_file_is_handled_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let run_dir = home.join("run");
    std::fs::create_dir_all(&run_dir).unwrap();
    std::fs::write(
        aproxy::install::state::state_path_in(&run_dir),
        "{corrupted",
    )
    .unwrap();
    // 静默退出：无残留、无 panic（库层返回 Ok——无有效状态即无事发生）
    aproxy::install::flow::continue_install(home, &run_dir)
        .await
        .expect("损坏状态文件应被静默处置");
}

/// 恢复矩阵行「done 残留（清文件前崩溃）」→ 清文件即完成。
#[tokio::test(flavor = "current_thread")]
async fn done_residue_is_cleared() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let run_dir = home.join("run");
    std::fs::create_dir_all(&run_dir).unwrap();
    let state = crash_state(InstallPhase::Done, &usable_from(home), None);
    aproxy::install::state::write_in(&run_dir, &mut state.clone()).unwrap();
    aproxy::install::flow::continue_install(home, &run_dir)
        .await
        .expect("done 残留应被清理");
    assert!(!aproxy::install::state::state_path_in(&run_dir).exists());
}

fn bin_works(home: &Path) -> bool {
    let bin = aproxy::install::swap::bin_path_in(home);
    bin.is_file()
        && aproxy::install::staging::probe_version(&bin)
            .map(|v| v == env!("CARGO_PKG_VERSION"))
            .unwrap_or(false)
}
