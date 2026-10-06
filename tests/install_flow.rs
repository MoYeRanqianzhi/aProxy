//! install 主流程端到端：--from 全流程（无实例快路径 / 有实例滚动重启，都经
//! 早交接由 staging 里的目标二进制驱动）、交棒失败与失败转告、管辖检查、
//! --adopt 收编、--abort 回滚窗口。
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
//    状态文件删除（broadcasting/restarting 全部跳过）
// ---------------------------------------------------------------------------
#[test]
fn install_from_without_instances_fast_path() {
    let env = TestEnv::new(1);
    env.seed_bin();
    let from = env.source_file("fast");

    // --no-skills：skill 支线会去 GitHub/npm 下载，测试不依赖外网
    let out = env
        .install_cmd(&["--from", &from.display().to_string(), "--no-skills"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "install 应成功: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    // 安装者在接手者进入 cleaning 时就以 0 退出（Windows 上 .old 是它自己的
    // 镜像，它不退出就删不掉），状态文件稍后由接手者清掉
    let state = aproxy::install::state::state_path_in(&env.home().join("run"));
    let deadline = Instant::now() + Duration::from_secs(30);
    while state.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(200));
    }
    // 终态：bin 可运行、.old 已删、staging 清理（Windows 上接手者交换后把剩余
    // 阶段交给了 bin 里的二进制，staging 里的副本已不在运行）、状态文件删除
    //（done 的完成语义）
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
// 2. 有实例滚动重启：实例跑在规范 bin → install --from → 交给 staging 里的
//    目标二进制 → 广播 ACK → swap →（Windows 再交给 bin 里的二进制）→ 滚动
//    重启实例 → 终验 → 清理 → done
// ---------------------------------------------------------------------------
#[test]
fn install_from_with_instance_rolling_restart() {
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
    // 安装含两次交棒（各等接手最多 30s）+ 滚动重启 + 终验，给 120s
    let done = wait_install_done(&env, Duration::from_secs(120));
    let mut output = String::new();
    let _ = installer.stdout.take().unwrap().read_to_string(&mut output);
    let _ = installer.stderr.take().unwrap().read_to_string(&mut output);
    let status = installer.wait().unwrap();
    assert!(
        done,
        "安装未在预期时间内完成（状态文件残留）; 输出: {output}"
    );
    // 交棒后 CLI 等到接手者终验通过才退出：退出码 0 + 完成提示即真实结局
    assert!(
        status.success() && output.contains("新版本进程已接手安装") && output.contains("安装完成"),
        "CLI 应交棒、在接手者完成后以 0 退出并报告完成; 输出: {output}"
    );

    // 终态断言：bin 可运行、.old 已删（尾部由 bin 里的二进制执行，非自镜像）、
    // 实例已滚动到新 pid（旧实例被优雅停止后用新二进制拉起）
    assert!(bin_works(&env.home()));
    assert!(!aproxy::install::swap::old_path_in(&env.home()).exists());
    let live = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            aproxy::daemon::ipc_ping_in(&env.home().join("run"), &env.port.to_string()).await
        });
    let info = live.expect("滚动后实例应可 ping");
    assert_ne!(info.instance.pid, old_pid, "实例应已滚动到新 pid");
    assert_eq!(info.instance.version, env!("CARGO_PKG_VERSION"));
    assert_eq!(
        info.state,
        aproxy::daemon::InstanceState::Serving,
        "滚动完成后实例应退出更换阶段"
    );

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
    // 下载链指向一个必然连不上的地址：在线安装若在管辖检查之前就开始下载，
    // 报的会是下载失败而不是管辖错误
    std::fs::write(
        env.home().join("settings.json"),
        r#"{"watchdog": false, "download_chain": [{"url": "http://127.0.0.1:1/{asset}"}]}"#,
    )
    .unwrap();
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
    // 在线路径同样拒绝，且在任何下载之前（显式版本号跳过 latest 查询）
    let out = env.install_cmd(&["9.9.9", "--no-skills"]).output().unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!out.status.success(), "在线安装应被拒绝: {text}");
    assert!(text.contains("--adopt"), "在线安装应报管辖错误: {text}");
    assert!(!text.contains("开始下载"), "管辖检查应先于下载: {text}");

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
    // CLI 交棒后会等接手者到达终点（终验通过即 cleaning）才以 0 退出——
    // 退出码即结局；此后只剩接手者删 .old/staging 与状态文件
    let out = env
        .install_cmd(&["--adopt", "--no-skills"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "收编应成功: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let state = aproxy::install::state::state_path_in(&run_dir);
    let deadline = Instant::now() + Duration::from_secs(30);
    while state.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(!state.exists(), "收编未完成（状态文件残留）");

    // 终态：标准位置出现二进制、实例滚动到 bin 二进制
    assert!(bin_works(&env.home()));
    let live = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            aproxy::daemon::ipc_ping_in(&env.home().join("run"), &env.port.to_string()).await
        })
        .expect("收编后实例应可 ping");
    assert_ne!(live.instance.pid, old_pid);
    // 新实例的镜像在 bin 下（收编完成的事实）
    let image = aproxy::watchdog::process_image_path(live.instance.pid).expect("新实例镜像可查");
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
// 6b. 滚动重启实例级失败（审查 install-01 原始复现）：实例运行中配置被改坏
//     → install 滚动到它时新旧二进制都起不来。修复前：.restore 被守护优雅
//     退出删除、实例永久丢失；Windows 接力路径打印「交换完成」并以 0 退出。
//     修复后：CLI（等接手者跑到终点）以非零退出并给出原因与指引，
//     .restore 以原参数保住，状态 halted（自动续作不再重试）。
// ---------------------------------------------------------------------------
#[test]
fn install_instance_failure_exits_nonzero_and_keeps_restore() {
    let env = TestEnv::new(8);
    std::fs::write(env.home().join("settings.json"), r#"{"watchdog": false}"#).unwrap();
    let bin = env.seed_bin();
    let cfg_file = env.home().join("broken.toml");
    std::fs::write(
        &cfg_file,
        format!(
            "base_url = \"https://broken.example.com\"\nlisten_addr = \"127.0.0.1:{}\"\n",
            env.port
        ),
    )
    .unwrap();
    let mut child = Command::new(&bin)
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
    let run_dir = env.home().join("run");
    let mut ready = false;
    for _ in 0..100 {
        if aproxy::daemon::registry_pids_in(&run_dir).contains(&child.id()) {
            ready = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(ready, "实例未就绪");
    let restore_before = std::fs::read_to_string(aproxy::daemon::restore_file_path_in(
        &run_dir,
        &env.port.to_string(),
    ))
    .unwrap();

    // 运行中改坏配置（在跑的实例不受影响；任何版本按它重启都在 bind 前退出）
    std::fs::write(
        &cfg_file,
        format!(
            "base_url = \"ftp://bad.example\"\nlisten_addr = \"127.0.0.1:{}\"\n",
            env.port
        ),
    )
    .unwrap();

    let from = env.source_file("broken");
    let started = Instant::now();
    let out = env
        .install_cmd(&["--from", &from.display().to_string(), "--no-skills"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(1),
        "实例级失败必须非零退出（Windows 接力路径修复前为 0）; stdout: {stdout} stderr: {stderr}"
    );
    assert!(
        stderr.contains("滚动已中止") && stderr.contains("aproxy restore"),
        "应给出中止说明与 restore 指引: {stderr}"
    );
    // 失败发生在接手的目标版本进程里，经 CLI 轮询 install.state 转告
    assert!(
        stdout.contains("新版本进程已接手安装"),
        "应交给目标版本驱动: {stdout}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(120),
        "失败应在有限时间内报告"
    );
    let _ = child.wait();

    // .restore 以原参数保住（修复前被删）
    let restore_after = std::fs::read_to_string(aproxy::daemon::restore_file_path_in(
        &run_dir,
        &env.port.to_string(),
    ))
    .expect(".restore 必须保住");
    let args_of = |s: &str| serde_json::from_str::<serde_json::Value>(s).unwrap()["args"].clone();
    assert_eq!(args_of(&restore_after), args_of(&restore_before));
    let st = aproxy::install::state::load_in(&run_dir).expect("failed 现场保留");
    assert_eq!(st.phase, aproxy::install::state::InstallPhase::Failed);
    assert!(st.halted, "实例级失败应置 halted");
}

// ---------------------------------------------------------------------------
// 6c. unix 端到端回滚：--from 一个「能自报版本、但拉不起实例」的新二进制
//     → 交换前保留的 bin/aproxy.old 把实例按原参数拉回，CLI 非零退出。
//     假二进制自报的版本比当前低，是降级：不交棒，滚动在 CLI 进程内执行
//     （交棒路径上的同一失败见 6d；Windows 的同一逻辑由 install_flow_lib 的
//     回滚用例覆盖——Windows 上造不出「能答 --version 却拒绝启动」的假 exe）
// ---------------------------------------------------------------------------
#[cfg(unix)]
#[test]
fn unix_install_rolls_back_instance_to_preserved_old_binary() {
    use std::os::unix::fs::PermissionsExt;
    let env = TestEnv::new(9);
    std::fs::write(env.home().join("settings.json"), r#"{"watchdog": false}"#).unwrap();
    let bin = env.seed_bin();
    let cfg_file = env.home().join("unixrb.toml");
    std::fs::write(
        &cfg_file,
        format!(
            "base_url = \"https://unixrb.example.com\"\nlisten_addr = \"127.0.0.1:{}\"\n",
            env.port
        ),
    )
    .unwrap();
    let mut child = Command::new(&bin)
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

    let fake = env.home().join("fake-new");
    std::fs::write(
        &fake,
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo \"aproxy 0.0.0-fake\"; exit 0; fi\nexit 1\n",
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();

    let out = env
        .install_cmd(&["--from", &fake.display().to_string(), "--no-skills"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "应非零退出: {stderr}");
    assert!(
        stderr.contains("旧二进制"),
        "应说明已用旧二进制拉回: {stderr}"
    );
    let _ = child.wait();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let info = rt
        .block_on(aproxy::daemon::ipc_ping_in(&run_dir, &env.port.to_string()))
        .expect("实例应被旧二进制拉回");
    assert_ne!(info.instance.pid, old_pid);
    assert_eq!(
        info.instance.version,
        env!("CARGO_PKG_VERSION"),
        "拉回的是旧版本"
    );
    let image = aproxy::watchdog::process_image_path(info.instance.pid).unwrap();
    assert_eq!(image, aproxy::install::swap::old_path_in(&env.home()));
    assert!(
        aproxy::daemon::restore_file_path_in(&run_dir, &env.port.to_string()).is_file(),
        ".restore 应在"
    );

    let _ = rt.block_on(aproxy::install::restart::stop_and_wait(
        &run_dir,
        &env.port.to_string(),
        Duration::from_secs(20),
    ));
}

/// unix 上起一个跑在规范 bin 上的实例，等它就绪
#[cfg(unix)]
fn start_bin_instance(env: &TestEnv, name: &str) -> std::process::Child {
    let cfg_file = env.home().join(format!("{name}.toml"));
    std::fs::write(
        &cfg_file,
        format!(
            "base_url = \"https://{name}.example.com\"\nlisten_addr = \"127.0.0.1:{}\"\n",
            env.port
        ),
    )
    .unwrap();
    let mut child = Command::new(aproxy::install::swap::bin_path_in(&env.home()))
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
    let run_dir = env.home().join("run");
    for _ in 0..100 {
        if aproxy::daemon::registry_pids_in(&run_dir).contains(&child.id()) {
            return child;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = child.kill();
    let _ = child.wait();
    panic!("实例未就绪");
}

/// unix 上把 `body` 写成可执行脚本
#[cfg(unix)]
fn write_script(path: &std::path::Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

// ---------------------------------------------------------------------------
// 6d. 交棒失败：目标二进制自报同一版本（会交给它），但不接手安装就退出 →
//     安装以失败收场，失败说清楚且可续作（不是 halted），实例与 bin 都没动。
// ---------------------------------------------------------------------------
#[cfg(unix)]
#[test]
fn handover_to_a_target_that_never_takes_over_fails_without_touching_anything() {
    let env = TestEnv::new(10);
    std::fs::write(env.home().join("settings.json"), r#"{"watchdog": false}"#).unwrap();
    env.seed_bin();
    let mut child = start_bin_instance(&env, "refuse");
    let pid = child.id();

    let fake = env.home().join("fake-refuse");
    write_script(
        &fake,
        &format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo \"aproxy {}\"; exit 0; fi\nexit 1\n",
            env!("CARGO_PKG_VERSION")
        ),
    );
    let started = Instant::now();
    let out = env
        .install_cmd(&["--from", &fake.display().to_string(), "--no-skills"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "应非零退出: {stderr}");
    assert!(stderr.contains("没有接手"), "应说明目标没有接手: {stderr}");
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "接手者退出应被立即发现，而不是等满超时"
    );

    let run_dir = env.home().join("run");
    let st = aproxy::install::state::load_in(&run_dir).expect("failed 现场保留");
    assert_eq!(st.phase, aproxy::install::state::InstallPhase::Failed);
    assert!(!st.halted, "交棒失败可续作，不是实例级失败");
    assert_eq!(
        std::fs::read(aproxy::install::swap::bin_path_in(&env.home())).unwrap(),
        std::fs::read(env!("CARGO_BIN_EXE_aproxy")).unwrap(),
        "bin 不应被换"
    );
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let info = rt
        .block_on(aproxy::daemon::ipc_ping_in(&run_dir, &env.port.to_string()))
        .expect("实例应仍在线");
    assert_eq!(info.instance.pid, pid, "实例不应被重启");
    assert_eq!(info.state, aproxy::daemon::InstanceState::Serving);

    let _ = rt.block_on(aproxy::install::restart::stop_and_wait(
        &run_dir,
        &env.port.to_string(),
        Duration::from_secs(20),
    ));
    let _ = child.wait();
}

// ---------------------------------------------------------------------------
// 6e. 早交接的证据与失败转告：目标二进制是一层包装（记下自己被怎样调用，
//     install 交给真二进制，其余一律失败）。安装者应以 `install --continue
//     --handover-from <pid>` 拉起它；接手者滚动时新二进制拉不起实例 → 用
//     旧二进制拉回、halted；安装者把接手者记下的原因转告用户并非零退出。
// ---------------------------------------------------------------------------
#[cfg(unix)]
#[test]
fn staged_target_drives_the_install_and_its_failure_reaches_the_installer() {
    let env = TestEnv::new(11);
    std::fs::write(env.home().join("settings.json"), r#"{"watchdog": false}"#).unwrap();
    env.seed_bin();
    let mut child = start_bin_instance(&env, "wrapped");

    let calls = env.home().join("wrapper-calls.log");
    let wrapper = env.home().join("wrapper");
    write_script(
        &wrapper,
        &format!(
            "#!/bin/sh\necho \"$*\" >> '{calls}'\n\
             if [ \"$1\" = \"--version\" ]; then echo \"aproxy {version}\"; exit 0; fi\n\
             if [ \"$1\" = \"install\" ]; then exec '{real}' \"$@\"; fi\n\
             exit 1\n",
            calls = calls.display(),
            version = env!("CARGO_PKG_VERSION"),
            real = env!("CARGO_BIN_EXE_aproxy"),
        ),
    );
    let out = env
        .install_cmd(&["--from", &wrapper.display().to_string(), "--no-skills"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(1),
        "stdout: {stdout} stderr: {stderr}"
    );
    assert!(stdout.contains("新版本进程已接手安装"), "{stdout}");
    // 「实例 <端口> 滚动重启失败」只出现在接手者记下的 last_error 里（中止指引
    // 是安装者自己的文案），据此确认原因是从接手者转告过来的
    assert!(
        stderr.contains(&format!("实例 {} 滚动重启失败", env.port))
            && stderr.contains("滚动已中止"),
        "应转告接手者的失败原因与中止指引: {stderr}"
    );
    let log = std::fs::read_to_string(&calls).unwrap();
    assert!(
        log.lines()
            .any(|l| l.starts_with("install --continue --handover-from ")),
        "安装者应指名交给目标二进制: {log}"
    );

    let run_dir = env.home().join("run");
    let st = aproxy::install::state::load_in(&run_dir).expect("failed 现场保留");
    assert_eq!(st.phase, aproxy::install::state::InstallPhase::Failed);
    assert!(st.halted);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let info = rt
        .block_on(aproxy::daemon::ipc_ping_in(&run_dir, &env.port.to_string()))
        .expect("实例应被旧二进制拉回");
    let image = aproxy::watchdog::process_image_path(info.instance.pid).unwrap();
    assert_eq!(image, aproxy::install::swap::old_path_in(&env.home()));

    let _ = rt.block_on(aproxy::install::restart::stop_and_wait(
        &run_dir,
        &env.port.to_string(),
        Duration::from_secs(20),
    ));
    let _ = child.wait();
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
