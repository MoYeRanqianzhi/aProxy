//! 0.1.0 的安装器碰到当前构建时干净地拒绝，端到端：真实的 v0.1.0 二进制起一个实例，
//! 再用它自己的 `aproxy install --from` 装当前构建。
//!
//! 0.2.0 起不支持从 0.1.0 原地升级，0.1.0 的安装器必须在改动任何东西之前失败。它在交换
//! 之前只执行一次新二进制：`--version`，取输出里第一个以数字开头的词当版本。当前构建输出
//! 永久格式 `aproxy v<版本> (…)`（见 aproxy::VERSION_LINE），没有以数字开头的词，于是
//! 0.1.0 报「--version 输出无版本号」退出。这里断言：非零退出、失败来自版本探测、bin 里
//! 仍是 0.1.0、没有安装状态与备料、0.1.0 的实例原样在服务（用 0.1.0 自己的 status 确认
//! ——0.2.0 起的 CLI 联系不上 0.1.0 的实例）。0.1.0 在线安装的各条渠道下载之后走的是同一个
//! 探测，这里用 `--from` 覆盖它。
//!
//! 只在 CI 跑：测试标了 `#[ignore]`，CI 的 upgrade-from-0-1-0 任务下载 v0.1.0 资产、
//! 设好 `APROXY_V010_BIN` 后以 `--ignored` 运行。开发机上往往跑着用户真正在用的
//! 0.1.0 实例，而 0.1.0 的 Windows 控制管道按端口全机共享，测试里的 0.1.0 二进制
//! 可能碰到它们。

use std::path::{Path, PathBuf};
use std::process::Command;

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

/// 测试结束（含断言失败）时用 bin 里的 0.1.0 停掉本 home 的全部实例
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
fn installer_of_0_1_0_refuses_this_build_before_touching_anything() {
    let old_bin = PathBuf::from(std::env::var("APROXY_V010_BIN").expect("APROXY_V010_BIN 未设置"));
    let home_dir = tempfile::tempdir().unwrap();
    let home = home_dir.path();
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

    // 一个 0.1.0 实例；看门狗保持默认开启，与用户的现场一致
    let port = free_port();
    let _stop = StopAll { bin: &bin, home };
    let cfg = home.join("c.toml");
    std::fs::write(
        &cfg,
        format!("base_url = \"https://api.example.com\"\nlisten_addr = \"127.0.0.1:{port}\"\n"),
    )
    .unwrap();
    let out = run(&["start", "--config", cfg.to_str().unwrap()]);
    assert!(out.status.success(), "0.1.0 start: {}", text(&out));
    // 0.1.0 的 status 每个实例一行 `  端口 <端口>  pid <pid>  v<版本>  …`：取 pid 与版本
    let instance = |status: &str| -> Option<(String, String)> {
        let line = status
            .lines()
            .find(|line| line.contains(&format!("端口 {port} ")))?;
        let words: Vec<&str> = line.split_whitespace().collect();
        let pid = words.iter().skip_while(|w| **w != "pid").nth(1)?;
        let version = words.iter().find(|w| w.starts_with('v'))?;
        Some((pid.to_string(), version.to_string()))
    };
    let before = text(&run(&["status"]));
    let (pid, version) =
        instance(&before).unwrap_or_else(|| panic!("0.1.0 status 里找不到端口 {port}: {before}"));
    assert_eq!(version, "v0.1.0", "{before}");
    let bin_before = std::fs::read(&bin).unwrap();

    // 用 0.1.0 自己的安装器装当前构建：必须在交换之前失败
    let out = run(&[
        "install",
        "--from",
        new_bin.to_str().unwrap(),
        "--no-skills",
    ]);
    let output = text(&out);
    assert!(!out.status.success(), "0.1.0 install 应失败: {output}");
    assert!(
        output.contains("--version 输出无版本号"),
        "失败应来自 0.1.0 的版本探测: {output}"
    );
    assert_eq!(
        std::fs::read(&bin).unwrap(),
        bin_before,
        "bin 里应仍是 0.1.0"
    );
    assert!(
        !home.join("run").join("install.state").exists(),
        "0.1.0 不应建立安装状态"
    );
    assert!(!home.join("staging").exists(), "0.1.0 不应开始备料");

    // 0.1.0 的实例原样在服务：同一个进程、仍是 0.1.0
    let after = text(&run(&["status"]));
    assert_eq!(
        instance(&after),
        Some((pid, version)),
        "0.1.0 的实例应原样在服务: {after}"
    );
}
