//! install 主流程端到端：--from 全流程（无实例快路径 / 有实例滚动重启 +
//! Windows 接力）、管辖检查、--adopt 收编、--abort 回滚窗口。
//!
//! 隔离：每个测试独立 tempdir 作为 APROXY_HOME，实例与安装进程都注入同一
//! 环境变量；端口从测试进程 pid 派生，绝不触碰生产实例（12345/12349）。
//!
//! 形态说明：旧/新二进制都是 CARGO_BIN_EXE 副本（同版本——测的是舞步与
//! 状态机推进，不是版本差异；verifying 的 version==target 对同版本成立）。

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// bind 试探选取可用端口（pid 派生可能撞上 Windows 动态端口排除区间——
/// Hyper-V/WinNAT 保留块；原子计数保证同进程并发测试不互撞）
fn free_port() -> u16 {
    use std::sync::atomic::{AtomicU16, Ordering};
    static SEQ: AtomicU16 = AtomicU16::new(0);
    let base = 27000u16 + (std::process::id() % 500) as u16 * 20;
    for i in 0..200u16 {
        let candidate = base + SEQ.fetch_add(1, Ordering::Relaxed) + i * 300;
        if std::net::TcpListener::bind(("127.0.0.1", candidate)).is_ok() {
            return candidate;
        }
    }
    panic!("200 次内未找到可用端口");
}

struct TestEnv {
    home: tempfile::TempDir,
    port: u16,
}

impl TestEnv {
    fn new(_port_offset: u32) -> Self {
        Self {
            home: tempfile::tempdir().unwrap(),
            port: free_port(),
        }
    }
    fn home(&self) -> std::path::PathBuf {
        self.home.path().to_path_buf()
    }
    /// 规范位置预放「旧版本」二进制（已安装形态）
    fn seed_bin(&self) -> std::path::PathBuf {
        let bin = aproxy::install::swap::bin_path_in(&self.home());
        std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
        std::fs::copy(env!("CARGO_BIN_EXE_aproxy"), &bin).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        bin
    }
    /// --from 源（bin 外的副本——模拟新版本下载产物）
    fn source_file(&self, tag: &str) -> std::path::PathBuf {
        let from = self.home().join(format!("download-{tag}"));
        std::fs::copy(env!("CARGO_BIN_EXE_aproxy"), &from).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&from, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        from
    }
    /// install 子进程（APROXY_HOME 注入）
    fn install_cmd(&self, args: &[&str]) -> Command {
        let exe = env!("CARGO_BIN_EXE_aproxy");
        let mut cmd = Command::new(exe);
        cmd.arg("install").args(args);
        cmd.env("APROXY_HOME", self.home());
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        cmd
    }
}

/// 轮询安装完成标志：状态文件**先出现（installer 抢锁）后消失（done 清场）**。
/// 两段缺一不可——直接等「消失」有启动竞态：installer 尚未 create_new 时
/// state 也不存在，会被误判为已完成（实测三轮 1.6s 假通过的根源）。
fn wait_install_done(env: &TestEnv, timeout: Duration) -> bool {
    let state = aproxy::install::state::state_path_in(&env.home().join("run"));
    let deadline = Instant::now() + timeout;
    // 段 1：等抢锁（state 出现）
    while !state.exists() {
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    // 段 2：等清场（state 消失 = done）
    while state.exists() {
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    true
}

fn bin_works(home: &std::path::Path) -> bool {
    let bin = aproxy::install::swap::bin_path_in(home);
    bin.is_file()
        && aproxy::install::staging::probe_version(&bin)
            .map(|v| v == env!("CARGO_PKG_VERSION"))
            .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// 1. 无实例快路径：bin 预放旧二进制 → install --from → 落位 + .old 清理 +
//    状态文件删除（broadcasting/relaying/restarting 全部跳过）
// ---------------------------------------------------------------------------
#[test]
fn install_from_without_instances_fast_path() {
    let env = TestEnv::new(1);
    env.seed_bin();
    let from = env.source_file("fast");

    let out = env
        .install_cmd(&["--from", &from.display().to_string()])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "install 应成功: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    // 终态：bin 可运行、.old 已删（安装进程不在 bin 内，无镜像锁）、
    // staging 清理、状态文件删除（done 的完成语义）
    assert!(bin_works(&env.home()));
    assert!(
        !aproxy::install::swap::old_path_in(&env.home()).exists(),
        "无实例快路径下 .old 应在 cleaning 删除（无镜像锁）"
    );
    assert!(!aproxy::install::state::state_path_in(&env.home().join("run")).exists());
    assert!(
        !aproxy::install::staging::staging_dir_in(&env.home(), env!("CARGO_PKG_VERSION")).exists(),
        "staging 应在 done 清理"
    );
    // 幂等：二进制可再执行（fallback 脚本也不干扰正常解析）
    //（bin_works 已覆盖 --version 试跑）
}

// ---------------------------------------------------------------------------
// 2. 有实例滚动重启：实例跑在规范 bin → install --from → 广播 ACK → swap →
//    Windows 接力 → 接管者滚动重启实例 → 终验 → 清理 → done
// ---------------------------------------------------------------------------
#[test]
#[cfg(windows)]
fn install_from_with_instance_rolling_restart_and_relay() {
    use std::io::Read;

    let env = TestEnv::new(2);
    let bin = env.seed_bin();
    let cfg_file = env.home().join("inst.toml");
    std::fs::write(
        &cfg_file,
        format!(
            "base_url = \"https://inst.example.com\"\nlisten_addr = \"127.0.0.1:{}\"\n",
            env.port
        ),
    )
    .unwrap();

    // 实例跑在规范 bin 二进制上（管辖内的真实升级形态）
    let child = Command::new(&bin)
        .args([
            "--config",
            &cfg_file.display().to_string(),
            "--daemon-child",
        ])
        .env("APROXY_HOME", env.home())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let old_pid = child.id();
    std::mem::forget(child);
    // 就绪：注册表出现该实例（bind 成功才写）
    let run_dir = env.home().join("run");
    let mut ready = false;
    for _ in 0..100 {
        if aproxy::daemon::registry_pids_in(&run_dir).contains(&old_pid) {
            ready = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(ready, "实例未就绪");

    let from = env.source_file("rolling");
    let mut installer = env
        .install_cmd(&["--from", &from.display().to_string(), "--no-skills"])
        .spawn()
        .unwrap();
    // 安装含 relay（等接管最多 30s）+ 滚动重启 + 终验，给 120s
    let done = wait_install_done(&env, Duration::from_secs(120));
    let mut output = String::new();
    let _ = installer.stdout.take().unwrap().read_to_string(&mut output);
    let _ = installer.stderr.take().unwrap().read_to_string(&mut output);
    let _ = installer.wait();
    assert!(
        done,
        "安装未在预期时间内完成（状态文件残留）; 输出: {output}"
    );

    // 终态断言：bin 可运行、.old 已删（接管进程跑在新 bin 上，非自镜像）、
    // 实例已滚动到新 pid（旧实例被优雅停止后用新二进制拉起）
    assert!(bin_works(&env.home()));
    assert!(!aproxy::install::swap::old_path_in(&env.home()).exists());
    let live = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async { aproxy::daemon::ipc_ping(&env.port.to_string()).await });
    let info = live.expect("滚动后实例应可 ping");
    assert_ne!(info.pid, old_pid, "实例应已滚动到新 pid");
    assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
    assert!(!info.swap_phase, "滚动完成后实例应退出更换阶段");

    // 清理实例（测试收尾——同 home 找得到注册表）
    let _ = Command::new(env!("CARGO_BIN_EXE_aproxy"))
        .args(["stop", &env.port.to_string()])
        .env("APROXY_HOME", env.home())
        .output();
}

// ---------------------------------------------------------------------------
// 3. 管辖检查：实例二进制在 bin 外 → 拒绝安装并指路 --adopt
// ---------------------------------------------------------------------------
#[tokio::test]
async fn jurisdiction_check_rejects_foreign_binary() {
    let env = TestEnv::new(3);
    // 不 seed_bin：实例将跑在 from 源（bin 外）
    let cfg_file = env.home().join("foreign.toml");
    std::fs::write(
        &cfg_file,
        format!(
            "base_url = \"https://foreign.example.com\"\nlisten_addr = \"127.0.0.1:{}\"\n",
            env.port
        ),
    )
    .unwrap();
    let foreign_bin = env.source_file("foreign");
    let child = Command::new(&foreign_bin)
        .args([
            "--config",
            &cfg_file.display().to_string(),
            "--daemon-child",
        ])
        .env("APROXY_HOME", env.home())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    std::mem::forget(child);
    let run_dir = env.home().join("run");
    let mut ready = false;
    for _ in 0..100 {
        if !aproxy::daemon::registry_pids_in(&run_dir).is_empty() {
            ready = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(ready, "外域实例未就绪");

    // 管辖检查（库层直接断言）：bin 外实例 → 拒绝
    let err = aproxy::install::flow::check_jurisdiction(&run_dir, &env.home(), false)
        .await
        .unwrap_err();
    assert!(err.contains("--adopt"), "应指路 --adopt: {err}");
    // adopt 豁免
    assert!(
        aproxy::install::flow::check_jurisdiction(&run_dir, &env.home(), true)
            .await
            .is_ok()
    );

    let _ = Command::new(env!("CARGO_BIN_EXE_aproxy"))
        .args(["stop", &env.port.to_string()])
        .env("APROXY_HOME", env.home())
        .output();
}

// ---------------------------------------------------------------------------
// 4. --adopt：bin 外安装的实例收编到标准位置（管辖检查豁免 + 完整流水线）
// ---------------------------------------------------------------------------
#[test]
#[cfg(windows)]
fn adopt_migrates_foreign_instance() {
    let env = TestEnv::new(4);
    // 实例跑在 bin 外的「npm 安装形态」
    let cfg_file = env.home().join("adopted.toml");
    std::fs::write(
        &cfg_file,
        format!(
            "base_url = \"https://adopt.example.com\"\nlisten_addr = \"127.0.0.1:{}\"\n",
            env.port
        ),
    )
    .unwrap();
    let foreign_bin = env.source_file("adopt");
    let child = Command::new(&foreign_bin)
        .args([
            "--config",
            &cfg_file.display().to_string(),
            "--daemon-child",
        ])
        .env("APROXY_HOME", env.home())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    std::mem::forget(child);
    let run_dir = env.home().join("run");
    let old_pid = {
        let mut pid = 0;
        for _ in 0..100 {
            let pids = aproxy::daemon::registry_pids_in(&run_dir);
            if let Some(p) = pids.first() {
                pid = *p;
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        pid
    };
    assert!(old_pid != 0, "外域实例未就绪");

    // --adopt：当前进程（CARGO_BIN_EXE）作为源收编
    let mut installer = env
        .install_cmd(&["--adopt", "--no-skills"])
        .spawn()
        .unwrap();
    let _ = installer.wait();
    let done = wait_install_done(&env, Duration::from_secs(90));
    assert!(done, "收编未完成（状态文件残留）");

    // 终态：标准位置出现二进制、实例滚动到 bin 二进制
    assert!(bin_works(&env.home()));
    let live = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async { aproxy::daemon::ipc_ping(&env.port.to_string()).await })
        .expect("收编后实例应可 ping");
    assert_ne!(live.pid, old_pid);
    // 新实例的镜像在 bin 下（收编完成的事实）
    let image = aproxy::watchdog::process_image_path(live.pid).expect("新实例镜像可查");
    assert!(
        image.starts_with(env.home().join("bin")),
        "收编后实例应跑在标准位置: {image:?}"
    );

    let _ = Command::new(env!("CARGO_BIN_EXE_aproxy"))
        .args(["stop", &env.port.to_string()])
        .env("APROXY_HOME", env.home())
        .output();
}

// ---------------------------------------------------------------------------
// 5. --abort：swapping 前干净回滚（清 staging + 状态文件）
// ---------------------------------------------------------------------------
#[test]
fn abort_rolls_back_cleanly_before_swap() {
    let env = TestEnv::new(5);
    let run_dir = env.home().join("run");
    std::fs::create_dir_all(&run_dir).unwrap();
    // 伪造 downloaded 现场：状态文件 + staging 完整副本（二进制未动的形态）
    let mut state = aproxy::install::state::InstallState::new_marking(
        env!("CARGO_PKG_VERSION"),
        aproxy::install::state::InstallSource::From,
    );
    let staged = aproxy::install::staging::staging_dir_in(&env.home(), env!("CARGO_PKG_VERSION"))
        .join(aproxy::install::staging::binary_name());
    std::fs::create_dir_all(staged.parent().unwrap()).unwrap();
    std::fs::write(&staged, b"staged-content").unwrap();
    state.phase = aproxy::install::state::InstallPhase::Downloaded;
    state.staged_path = Some(staged.display().to_string());
    aproxy::install::state::write_in(&run_dir, &mut state).unwrap();

    let out = env.install_cmd(&["--abort"]).output().unwrap();
    assert!(out.status.success(), "abort 应成功");
    assert!(
        !aproxy::install::state::state_path_in(&run_dir).exists(),
        "状态文件应删除"
    );
    assert!(!staged.exists(), "staged 副本应清理");
}

// ---------------------------------------------------------------------------
// 6. --abort：swapping 后只进不退 → 拒绝
// ---------------------------------------------------------------------------
#[test]
fn abort_rejected_after_swap_phase() {
    let env = TestEnv::new(6);
    let run_dir = env.home().join("run");
    std::fs::create_dir_all(&run_dir).unwrap();
    let mut state = aproxy::install::state::InstallState::new_marking(
        env!("CARGO_PKG_VERSION"),
        aproxy::install::state::InstallSource::From,
    );
    state.phase = aproxy::install::state::InstallPhase::Swapped;
    aproxy::install::state::write_in(&run_dir, &mut state).unwrap();

    let out = env.install_cmd(&["--abort"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1), "swapping 后 abort 应拒绝");
    assert!(
        aproxy::install::state::state_path_in(&run_dir).exists(),
        "现场保留（续作或人工兜底）"
    );
}

// ---------------------------------------------------------------------------
// 7. 并发安装锁：已有新鲜安装 → 第二个 install 拒绝
// ---------------------------------------------------------------------------
#[test]
fn concurrent_install_rejected_by_lock() {
    let env = TestEnv::new(7);
    env.seed_bin();
    let run_dir = env.home().join("run");
    std::fs::create_dir_all(&run_dir).unwrap();
    // 新鲜锁（installer_pid = 本进程 = 活着的 pid）
    let state = aproxy::install::state::InstallState::new_marking(
        env!("CARGO_PKG_VERSION"),
        aproxy::install::state::InstallSource::From,
    );
    aproxy::install::state::create_new_in(&run_dir, state).unwrap();

    let from = env.source_file("lock");
    let out = env
        .install_cmd(&["--from", &from.display().to_string()])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "并发安装应被锁拒绝");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("已有安装进行中"), "{stderr}");
}
