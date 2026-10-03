//! install 库层诊断：continue_install 从 restarting 残局续作（接管者视角，
//! 进程内直接跑——子进程黑盒链路里看不到的卡点在此暴露）。

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// live 测试（起真实守护 + 进程级 set_var APROXY_HOME）互斥：env 是进程级
/// 的，并行线程交错 set_var 会把对方守护的注册表指到别的 tempdir（实测
/// 90s 超时的根源）。async Mutex——guard 跨 await 持有整个测试期，串行执行
/// 所有 live 测试。
static LIVE_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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

/// Windows 接力交棒：continue_install 返回 `HandedOver` 时状态文件**留给
/// 接棒者**继续完成，命令返回 ≠ 完成——轮询等待其收敛（接棒者要做完整
/// 交换 + 滚动重启，窗口给足 90s）。此前测试在交棒返回后立即断言状态
/// 文件已删，windows-latest 上高概率走进交棒分支而确定性失败——
/// alpha.12 / alpha.15 两次发版被它卡住 Release 的 test 门禁（留档预警
/// 应验后才定位到真正根因：不是超时竞态，是 HandedOver 语义未被测试跟上）。
async fn wait_handed_over_done(run_dir: &std::path::Path, exit: &aproxy::install::flow::FlowExit) {
    if matches!(exit, aproxy::install::flow::FlowExit::HandedOver) {
        let deadline = std::time::Instant::now() + Duration::from_secs(90);
        while aproxy::install::state::state_path_in(run_dir).exists() {
            if std::time::Instant::now() > deadline {
                panic!("交棒后接棒者 90s 未完成（install.state 仍在）");
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn continue_from_restarting_reclaims_instance() {
    let _guard = LIVE_TEST_LOCK.lock().await;
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
    aproxy::install::broadcast::ack_one(&run_dir, &port.to_string())
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
        Ok(Ok(exit)) => {
            wait_handed_over_done(&run_dir, &exit).await;
            println!("continue_install 完成: {exit:?}，耗时 {:?}", t0.elapsed());
        }
    }
    // 终态：state 清理（Completed 直接删；HandedOver 经上方等待接棒者收敛）
    assert!(
        !aproxy::install::state::state_path_in(&run_dir).exists(),
        "done 后状态文件应删除"
    );
    // 实例已滚动：新 pid + swap_phase 清除（restart 真跑的证据）
    let ping = aproxy::daemon::ipc_ping(&port.to_string()).await;
    let info = ping.expect("滚动后实例应可 ping");
    assert_ne!(info.pid, old_pid, "实例应已滚动到新 pid");
    assert!(!info.swap_phase, "滚动后应退出更换阶段");
    // 收尾：滚动出的新实例是 detached 守护（不是 child），必须经 IPC 优雅
    // 停掉——只 kill 原 child（早已被滚动停止）会把它泄漏在测试机上
    let _ = aproxy::install::restart::stop_and_wait(
        &run_dir,
        &port.to_string(),
        Duration::from_secs(20),
    )
    .await;
    let _ = child.kill();
    let _ = child.wait();
}

/// 恢复矩阵行「swapping 中（Windows bin 空窗）」的**有实例版本**：实例已
/// ACK（swap_phase 置位）+ staging 完整在盘。续作重入 run_forward_from_staged
/// 时 phase(Swapping) 已高于 Broadcasting/Acked——相位守卫必须跳过逆向
/// advance，广播幂等重做后重入 Swapping 完成交换与滚动（无守卫版本会被
/// 状态机「非法迁移 Swapping → Broadcasting」拒绝，回归用）。
#[tokio::test(flavor = "current_thread")]
async fn continue_from_swapping_with_live_instance_redoes_swap() {
    let _guard = LIVE_TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
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

    let port = free_port();
    let cfg_file = home.join("cfg.toml");
    std::fs::write(
        &cfg_file,
        format!("base_url = \"https://libdiag.example.com\"\nlisten_addr = \"127.0.0.1:{port}\"\n"),
    )
    .unwrap();
    let mut child = Command::new(&bin)
        .args([
            "--config",
            &cfg_file.display().to_string(),
            "--daemon-child",
        ])
        .env("APROXY_HOME", home)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let old_pid = child.id();
    let mut ready = false;
    for _ in 0..100 {
        if aproxy::daemon::registry_pids_in(&run_dir).contains(&old_pid) {
            ready = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(ready, "实例未就绪");

    // swapping 现场语义：实例已 ACK（swap_phase 置位）+ staging 完整在盘
    aproxy::install::broadcast::ack_one(&run_dir, &port.to_string())
        .await
        .expect("PrepareSwap 广播应成功");
    let staged = aproxy::install::staging::staging_dir_in(home, env!("CARGO_PKG_VERSION"))
        .join(aproxy::install::staging::binary_name());
    std::fs::create_dir_all(staged.parent().unwrap()).unwrap();
    std::fs::copy(env!("CARGO_BIN_EXE_aproxy"), &staged).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let mut state = aproxy::install::state::InstallState::new_marking(
        env!("CARGO_PKG_VERSION"),
        aproxy::install::state::InstallSource::From,
    );
    state.phase = aproxy::install::state::InstallPhase::Swapping;
    state.instance_snapshot = vec![port.to_string()];
    state.from_path = Some(bin.display().to_string());
    state.staged_path = Some(staged.display().to_string());
    state.installer_pid = u32::MAX - 7;
    state.updated_at =
        aproxy::watchdog::now_secs().saturating_sub(aproxy::install::state::STALE_AFTER_SECS + 60);
    aproxy::install::state::write_in(&run_dir, &mut state).unwrap();

    let result = tokio::time::timeout(
        Duration::from_secs(90),
        aproxy::install::flow::continue_install(home, &run_dir),
    )
    .await;
    if result.is_err() {
        let cur = aproxy::install::state::load_in(&run_dir).map(|s| format!("{:?}", s.phase));
        panic!("swapping 有实例续作 90s 未返回（phase={cur:?}）");
    }
    let exit = result
        .expect("swapping 有实例续作应完成")
        .expect("swapping 有实例续作不应报错");
    wait_handed_over_done(&run_dir, &exit).await;
    assert!(
        !aproxy::install::state::state_path_in(&run_dir).exists(),
        "done 后状态文件应删除"
    );
    let info = aproxy::daemon::ipc_ping_in(&run_dir, &port.to_string())
        .await
        .expect("滚动后实例应可 ping");
    assert_ne!(info.pid, old_pid, "实例应已滚动到新 pid");
    assert!(!info.swap_phase, "滚动后应退出更换阶段");
    // 收尾：滚动出的新实例是 detached 守护（不是 child），必须经 IPC 优雅
    // 停掉——只 kill 原 child（早已被滚动停止）会把它泄漏在测试机上
    let _ = aproxy::install::restart::stop_and_wait(
        &run_dir,
        &port.to_string(),
        Duration::from_secs(20),
    )
    .await;
    let _ = child.kill();
    let _ = child.wait();
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

/// 恢复矩阵行「cleaning 残留（删除 .old 中崩溃）」→ 重入不回退重验
/// （Cleaning → Verifying 逆向迁移会被状态机拒绝——相位守卫修复的回归），
/// 重做清理即 done。
#[tokio::test(flavor = "current_thread")]
async fn crash_during_cleaning_resumes_without_backward_move() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let run_dir = home.join("run");
    std::fs::create_dir_all(&run_dir).unwrap();
    let from = usable_from(home);
    let bin = aproxy::install::swap::bin_path_in(home);
    let old = aproxy::install::swap::old_path_in(home);
    std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
    std::fs::copy(env!("CARGO_BIN_EXE_aproxy"), &bin).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    std::fs::write(&old, b"old-binary-residual").unwrap();
    let mut state = crash_state(InstallPhase::Cleaning, &from, None);
    state.old_path = Some(old.display().to_string());
    aproxy::install::state::write_in(&run_dir, &mut state).unwrap();

    aproxy::install::flow::continue_install(home, &run_dir)
        .await
        .expect("cleaning 残留续作应完成");
    assert!(bin_works(home));
    assert!(!old.exists(), ".old 应重做清理");
    assert!(!aproxy::install::state::state_path_in(&run_dir).exists());
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

// ---------------------------------------------------------------------------
// 滚动重启失败回滚（实例级失败注入）：新二进制拉不起实例时，实例必须被旧
// 二进制按原参数拉回、.restore 完好、滚动中止且自动续作不再重试。
// ---------------------------------------------------------------------------

/// 隔离 home + 进程级 APROXY_HOME（restart 原语 spawn 的隔代守护只继承进程
/// env）+ 关看护者（settings 总开关，实验期间不让守护补种看护者）。
fn live_home() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("APROXY_HOME", dir.path()) };
    std::fs::create_dir_all(dir.path().join("run")).unwrap();
    std::fs::write(dir.path().join("settings.json"), r#"{"watchdog": false}"#).unwrap();
    dir
}

fn make_exec(p: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    #[cfg(windows)]
    let _ = p;
}

/// 在 `exe` 上起一个守护实例（配置写入 home），等注册表就绪，返回
/// (端口, 配置路径, 子进程)。
fn start_instance(
    home: &Path,
    exe: &Path,
    name: &str,
) -> (u16, std::path::PathBuf, std::process::Child) {
    let port = free_port();
    let cfg = home.join(format!("{name}.toml"));
    std::fs::write(
        &cfg,
        format!("base_url = \"https://{name}.example.com\"\nlisten_addr = \"127.0.0.1:{port}\"\n"),
    )
    .unwrap();
    let child = Command::new(exe)
        .args(["--config", &cfg.display().to_string(), "--daemon-child"])
        .env("APROXY_HOME", home)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let run_dir = home.join("run");
    let pid = child.id();
    let mut ready = false;
    for _ in 0..100 {
        if aproxy::daemon::registry_pids_in(&run_dir).contains(&pid) {
            ready = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(ready, "实例未就绪");
    (port, cfg, child)
}

/// 模拟 swap 之后的现场：运行中的规范 bin 改名为旧二进制（Windows 允许
/// rename 运行中镜像；unix 同理），bin 换成 `new_bin` 的内容。返回旧路径。
fn simulate_swap(home: &Path, new_bin: impl FnOnce(&Path)) -> std::path::PathBuf {
    let bin = aproxy::install::swap::bin_path_in(home);
    let old = aproxy::install::swap::old_path_in(home);
    std::fs::rename(&bin, &old).unwrap();
    new_bin(&bin);
    make_exec(&bin);
    old
}

/// swapped 残局：续作直接进滚动尾部。target 取一个与在跑实例都不同的
/// 版本，保证「跳过已就绪」不会误跳。
fn swapped_state(run_dir: &Path, port: u16, old: &Path, bin: &Path) {
    let mut state = InstallState::new_marking("99.0.0", InstallSource::From);
    state.phase = InstallPhase::Swapped;
    state.instance_snapshot = vec![port.to_string()];
    state.old_path = Some(old.display().to_string());
    state.from_path = Some(bin.display().to_string());
    state.installer_pid = u32::MAX - 7;
    state.updated_at =
        aproxy::watchdog::now_secs().saturating_sub(aproxy::install::state::STALE_AFTER_SECS + 60);
    aproxy::install::state::write_in(run_dir, &mut state).unwrap();
}

fn restore_args_of(run_dir: &Path, port: u16) -> Option<Vec<String>> {
    aproxy::daemon::list_restore_entries_in(run_dir)
        .into_iter()
        .find(|e| e.port == port.to_string())
        .map(|e| e.args)
}

fn same_path(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| p.to_string_lossy().replace('\\', "/").to_lowercase();
    norm(a) == norm(b)
}

/// 「新二进制拉不起实例」的两种形态：
/// - 不可执行（坏文件——spawn 本身失败：被杀软拦截/下载损坏的形态）；
/// - 启动即退出（新版本拒绝现有配置的形态：bind 之前就退出，不写 .restore
///   也不写注册表）。Windows 上用系统自带的 whoami.exe 充当「能启动、但
///   不认这些参数而立即退出」的二进制；unix 用 `exit 1` 脚本。
#[derive(Clone, Copy)]
enum BrokenNew {
    NotExecutable,
    ExitsImmediately,
}

fn write_broken(kind: BrokenNew, bin: &Path) {
    match kind {
        BrokenNew::NotExecutable => std::fs::write(bin, b"this is not an executable").unwrap(),
        BrokenNew::ExitsImmediately => {
            #[cfg(windows)]
            std::fs::copy(r"C:\Windows\System32\whoami.exe", bin).unwrap();
            #[cfg(unix)]
            std::fs::write(bin, b"#!/bin/sh\nexit 1\n").unwrap();
        }
    }
}

async fn rollback_case(kind: BrokenNew) {
    let dir = live_home();
    let home = dir.path();
    let run_dir = home.join("run");
    let bin = aproxy::install::swap::bin_path_in(home);
    std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
    std::fs::copy(env!("CARGO_BIN_EXE_aproxy"), &bin).unwrap();
    make_exec(&bin);

    let (port, cfg, mut child) = start_instance(home, &bin, "rollback");
    let old_pid = child.id();
    let args_before = restore_args_of(&run_dir, port).expect("在跑实例应有 .restore");
    aproxy::install::broadcast::ack_one(&run_dir, &port.to_string())
        .await
        .expect("PrepareSwap 广播应成功");
    let old = simulate_swap(home, |b| write_broken(kind, b));
    swapped_state(&run_dir, port, &old, &bin);

    let err = tokio::time::timeout(
        Duration::from_secs(90),
        aproxy::install::flow::continue_install(home, &run_dir),
    )
    .await
    .expect("续作不应挂死")
    .expect_err("新二进制起不来，续作应失败");
    assert!(
        err.contains("旧二进制"),
        "错误应说明已用旧二进制拉回: {err}"
    );

    // 实例已被旧二进制按原参数拉回：新 pid、跑在旧二进制上、.restore 完好
    let info = aproxy::daemon::ipc_ping_in(&run_dir, &port.to_string())
        .await
        .expect("回滚后实例应在线");
    assert_ne!(info.pid, old_pid, "应是重新拉起的进程");
    let image = aproxy::watchdog::process_image_path(info.pid).expect("镜像路径可查");
    assert!(same_path(&image, &old), "实例应跑在旧二进制上: {image:?}");
    assert_eq!(
        restore_args_of(&run_dir, port).as_deref(),
        Some(args_before.as_slice()),
        ".restore 应完好（原参数）"
    );
    assert!(
        args_before.iter().any(|a| same_path(Path::new(a), &cfg)),
        "原参数应含配置路径: {args_before:?}"
    );

    // 状态：failed + halted + 原因；在途清单已清（实例已恢复）
    let st = aproxy::install::state::load_in(&run_dir).expect("failed 现场保留");
    assert_eq!(st.phase, InstallPhase::Failed);
    assert!(st.halted, "实例级失败应置 halted");
    assert!(st.last_error.is_some_and(|e| e.contains("旧二进制")));
    assert!(st.pending_restores.is_empty(), "实例已拉回，在途记录应清除");

    // 自动续作不再重试：原地拒绝且不碰实例
    let err2 = aproxy::install::flow::continue_install(home, &run_dir)
        .await
        .expect_err("halted 后续作应拒绝");
    assert!(err2.contains("不自动重试"), "{err2}");
    let again = aproxy::daemon::ipc_ping_in(&run_dir, &port.to_string())
        .await
        .expect("实例应仍在线");
    assert_eq!(again.pid, info.pid, "halted 续作不得重启实例");

    // 收尾：优雅停止本测试拉起的实例（隔离 home 内，按端口经 IPC）
    let _ = aproxy::install::restart::stop_and_wait(
        &run_dir,
        &port.to_string(),
        Duration::from_secs(20),
    )
    .await;
    let _ = child.wait();
}

#[tokio::test(flavor = "current_thread")]
async fn rolling_restart_rolls_back_when_new_binary_cannot_spawn() {
    let _guard = LIVE_TEST_LOCK.lock().await;
    rollback_case(BrokenNew::NotExecutable).await;
}

#[tokio::test(flavor = "current_thread")]
async fn rolling_restart_rolls_back_when_new_binary_exits_immediately() {
    let _guard = LIVE_TEST_LOCK.lock().await;
    rollback_case(BrokenNew::ExitsImmediately).await;
}

/// 审查原始复现（install-01）：实例运行中把配置改坏（新旧二进制都起不来）
/// → 滚动重启时新版本起不来、旧二进制拉回也失败。修复前：.restore 已被
/// 守护优雅退出删除、实例永久丢失。修复后：.restore 以原参数保住、在途
/// 记录留存、halted 中止，排除原因后 `aproxy restore` 可恢复。
#[tokio::test(flavor = "current_thread")]
async fn rolling_restart_keeps_restore_when_rollback_also_fails() {
    let _guard = LIVE_TEST_LOCK.lock().await;
    let dir = live_home();
    let home = dir.path();
    let run_dir = home.join("run");
    let bin = aproxy::install::swap::bin_path_in(home);
    std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
    std::fs::copy(env!("CARGO_BIN_EXE_aproxy"), &bin).unwrap();
    make_exec(&bin);

    let (port, cfg, mut child) = start_instance(home, &bin, "down");
    let args_before = restore_args_of(&run_dir, port).unwrap();
    aproxy::install::broadcast::ack_one(&run_dir, &port.to_string())
        .await
        .unwrap();
    // 运行中改坏配置：在跑的实例不受影响，任何版本按它重启都会在 bind 前退出
    std::fs::write(
        &cfg,
        format!("base_url = \"ftp://bad.example\"\nlisten_addr = \"127.0.0.1:{port}\"\n"),
    )
    .unwrap();
    let old = simulate_swap(home, |b| {
        std::fs::copy(env!("CARGO_BIN_EXE_aproxy"), b).unwrap();
    });
    swapped_state(&run_dir, port, &old, &bin);

    let err = tokio::time::timeout(
        Duration::from_secs(90),
        aproxy::install::flow::continue_install(home, &run_dir),
    )
    .await
    .expect("续作不应挂死")
    .expect_err("新旧二进制都起不来，续作应失败");
    assert!(err.contains("aproxy restore"), "错误应指引 restore: {err}");
    let _ = child.wait();

    assert!(
        aproxy::daemon::ipc_ping_in(&run_dir, &port.to_string())
            .await
            .is_err(),
        "实例确实下线（新旧都起不来）"
    );
    assert_eq!(
        restore_args_of(&run_dir, port).as_deref(),
        Some(args_before.as_slice()),
        ".restore 必须以原参数保住（修复前此处为空）"
    );
    let st = aproxy::install::state::load_in(&run_dir).unwrap();
    assert_eq!(st.phase, InstallPhase::Failed);
    assert!(st.halted);
    assert_eq!(
        st.pending_restores
            .iter()
            .map(|p| p.port.as_str())
            .collect::<Vec<_>>(),
        vec![port.to_string().as_str()],
        "下线实例的在途记录应留存"
    );
}

/// 「已停未恢复」的在途实例：安装进程在旧实例已停、新实例未就绪的窗口
/// 崩溃——实例既不在跑也没了 .restore，只剩 install.state 的在途记录。
/// 续作从 swapping 重入时重算快照必须把它并回去并用在途参数拉起（修复前
/// 快照只看 IPC 枚举，它被永久排除，续作照常 done 而实例从此下线）。
#[tokio::test(flavor = "current_thread")]
async fn continue_restores_instance_stopped_mid_restart() {
    let _guard = LIVE_TEST_LOCK.lock().await;
    let dir = live_home();
    let home = dir.path();
    let run_dir = home.join("run");
    let bin = aproxy::install::swap::bin_path_in(home);
    std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
    std::fs::copy(env!("CARGO_BIN_EXE_aproxy"), &bin).unwrap();
    make_exec(&bin);

    // 实例跑起来再优雅停掉：.restore 随之删除，复刻窗口内的现场
    let (port, _cfg, mut child) = start_instance(home, &bin, "pending");
    let args = restore_args_of(&run_dir, port).unwrap();
    aproxy::install::restart::stop_and_wait(&run_dir, &port.to_string(), Duration::from_secs(20))
        .await
        .unwrap();
    let _ = child.wait();
    assert!(
        restore_args_of(&run_dir, port).is_none(),
        "优雅退出应删 .restore"
    );

    let staged = aproxy::install::staging::staging_dir_in(home, env!("CARGO_PKG_VERSION"))
        .join(aproxy::install::staging::binary_name());
    std::fs::create_dir_all(staged.parent().unwrap()).unwrap();
    std::fs::copy(env!("CARGO_BIN_EXE_aproxy"), &staged).unwrap();
    make_exec(&staged);
    let mut state = crash_state(InstallPhase::Swapping, &bin, Some(&staged));
    state.pending_restores = vec![aproxy::install::state::PendingRestore {
        port: port.to_string(),
        args,
        log_path: String::new(),
    }];
    aproxy::install::state::write_in(&run_dir, &mut state).unwrap();

    let exit = tokio::time::timeout(
        Duration::from_secs(120),
        aproxy::install::flow::continue_install(home, &run_dir),
    )
    .await
    .expect("续作不应挂死")
    .expect("续作应完成");
    wait_handed_over_done(&run_dir, &exit).await;
    assert!(!aproxy::install::state::state_path_in(&run_dir).exists());

    let info = aproxy::daemon::ipc_ping_in(&run_dir, &port.to_string())
        .await
        .expect("在途实例应被续作拉起");
    assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
    assert!(
        restore_args_of(&run_dir, port).is_some(),
        "拉起后 .restore 应在"
    );

    let _ = aproxy::install::restart::stop_and_wait(
        &run_dir,
        &port.to_string(),
        Duration::from_secs(20),
    )
    .await;
}

fn bin_works(home: &Path) -> bool {
    let bin = aproxy::install::swap::bin_path_in(home);
    bin.is_file()
        && aproxy::install::staging::probe_version(&bin)
            .map(|v| v == env!("CARGO_PKG_VERSION"))
            .unwrap_or(false)
}
