//! 0.1.0 → 当前构建的原地升级，端到端：真实的 v0.1.0 二进制起实例，再用它自己的
//! `aproxy install --from` 装上当前构建。验证的是兼容层（src/compat_0_1_0.rs）与升级
//! 规则（.agents/plan/ipc-v1.md 的 (c) 节）合在一起真的能用——0.1.0 的安装器广播、
//! 换二进制、（Windows 上）交接给新二进制续作、滚动重启、核验，全程都要与新守护对话。
//!
//! 只在 CI 跑：测试标了 `#[ignore]`，CI 的 upgrade-from-0.1.0 任务下载 v0.1.0 资产、
//! 设好 `APROXY_V010_BIN` 后以 `--ignored` 运行。开发机上往往跑着用户真正在用的
//! 0.1.0 实例，而 0.1.0 的 Windows 控制管道按端口全机共享，测试里的 0.1.0 二进制
//! 可能碰到它们。

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

fn exe_name() -> &'static str {
    if cfg!(windows) {
        "aproxy.exe"
    } else {
        "aproxy"
    }
}

fn copy_exe(from: &Path, to: &Path) {
    std::fs::create_dir_all(to.parent().unwrap()).unwrap();
    std::fs::copy(from, to).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(to, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// 测试结束（含断言失败）时用 bin 里的二进制停掉本 home 的全部实例。升级成功时
/// 那是当前构建，失败时可能还是 0.1.0——两者都认得 `stop all`。
struct StopAll<'a> {
    bin: &'a Path,
    home: &'a Path,
}

impl Drop for StopAll<'_> {
    fn drop(&mut self) {
        let _ = Command::new(self.bin)
            .args(["stop", "all"])
            .env("APROXY_HOME", self.home)
            .output();
    }
}

#[test]
#[ignore = "需要 APROXY_V010_BIN 指向 v0.1.0 的发布资产；CI 的 upgrade-from-0.1.0 任务下载后运行"]
fn upgrade_driven_by_0_1_0_rolls_instances_onto_this_build() {
    let old_bin = PathBuf::from(std::env::var("APROXY_V010_BIN").expect("APROXY_V010_BIN 未设置"));
    let home_dir = tempfile::tempdir().unwrap();
    let home = home_dir.path();
    let run_dir = home.join("run");
    let bin = home.join("bin").join(exe_name());
    copy_exe(&old_bin, &bin);
    let new_bin = home.join(format!("download-{}", exe_name()));
    copy_exe(Path::new(env!("CARGO_BIN_EXE_aproxy")), &new_bin);
    let run = |args: &[&str]| {
        Command::new(&bin)
            .args(args)
            .env("APROXY_HOME", home)
            .env("NO_PROXY", "127.0.0.1,localhost")
            .output()
            .unwrap()
    };
    let text = |out: &std::process::Output| {
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    };

    // 两个 0.1.0 实例；看门狗保持默认开启——0.1.0 的看门狗也得被正确退役
    let ports = [free_port(), free_port()];
    let _stop = StopAll { bin: &bin, home };
    for (i, port) in ports.iter().enumerate() {
        let cfg = home.join(format!("c{i}.toml"));
        std::fs::write(
            &cfg,
            format!("base_url = \"https://api.example.com\"\nlisten_addr = \"127.0.0.1:{port}\"\n"),
        )
        .unwrap();
        let out = run(&["start", "--config", cfg.to_str().unwrap()]);
        assert!(out.status.success(), "0.1.0 start: {}", text(&out));
    }

    // 用 0.1.0 自己的安装器装当前构建
    let out = run(&[
        "install",
        "--from",
        new_bin.to_str().unwrap(),
        "--no-skills",
    ]);
    assert!(out.status.success(), "0.1.0 install: {}", text(&out));

    // Windows 上 0.1.0 交接给新二进制续作后就返回：等 install.state 消失（done 清场）
    let state = run_dir.join("install.state");
    let deadline = Instant::now() + Duration::from_secs(180);
    while state.exists() {
        assert!(
            Instant::now() < deadline,
            "install.state 迟迟不消失: {}",
            std::fs::read_to_string(&state).unwrap_or_default()
        );
        std::thread::sleep(Duration::from_millis(500));
    }

    // bin 里已是当前构建（两边自报的版本号此刻可能相同，按内容比）
    assert!(
        std::fs::read(&bin).unwrap() == std::fs::read(&new_bin).unwrap(),
        "bin 应已换成当前构建"
    );
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    for port in ports {
        let status = rt
            .block_on(aproxy::daemon::ipc_ping_in(&run_dir, &port.to_string()))
            .unwrap_or_else(|e| panic!("端口 {port} 的实例不可达: {e}"));
        // 新守护的 v1 应答带 run_dir；0.1.0 守护的应答经兼容层解析后为空——
        // 这是「实例真的滚动到了当前构建」的依据
        assert!(!status.run_dir.is_empty(), "端口 {port} 仍是 0.1.0 守护");
        assert_eq!(status.state, aproxy::daemon::InstanceState::Serving);
        assert!(
            aproxy::daemon::restore_file_path_in(&run_dir, &format!("127.0.0.1:{port}")).is_file(),
            "端口 {port} 的恢复记录应保留"
        );
    }
    // 0.1.0 的看门狗已不在任（新看门狗由自检稍后补种，此刻可能还没有）
    if let Some(claim) = aproxy::watchdog::read_claim_in(&run_dir) {
        assert_eq!(
            claim.version.as_deref(),
            Some(env!("CARGO_PKG_VERSION")),
            "0.1.0 的看门狗仍持有 claim"
        );
    }
}
