//! 跨版本强制安装（永久兼容契约 C1–C8），端到端：真实发布的旧版本二进制与当前构建互相
//! 强制安装、互相结束对方启动的进程。
//!
//! 契约说的是「从 0.2.0 起，每个发布的版本都要能强制安装别的每个版本，并结束它启动的
//! 进程」。`force_contract.rs` 钉的是格式与常量，`install_force.rs` 用当前构建自己走流程；
//! 两个不同版本的二进制真的碰在一起，只有这里测。对每个旧版本走四种组合（执行强制安装的
//! 版本 → 装上的版本，被结束的进程是谁启动的）：
//!
//! | 执行者 | 目标 | 运行中的进程 | 对应的用户场景 |
//! |---|---|---|---|
//! | 当前 | 当前 | 旧 | 新版本的安装器收拾旧版本留下的现场 |
//! | 旧 | 当前 | 旧 | 旧版本的 `install --force` 升到新版本 |
//! | 旧 | 旧 | 当前 | 旧版本的安装器把新版本换回去 |
//! | 当前 | 旧 | 当前 | 新版本的 `install --force --from` 降回旧版本 |
//!
//! 每一种都断言：退出码是 0 或 12（二进制一定换好了）、强制安装之前台账里活着的进程（实例
//! 与看护者）全部结束、`bin/` 里是目标二进制、实例的端口已经放开。实例回没回来不作要求：
//! 期望状态记录不在契约里，两个版本的记录格式不同时目标版本的 `restore` 读不懂它，那就是
//! 12、输出里按端口列出；回来了就是 0，这时核对它确实在台账里、版本是目标版本。
//!
//! 只在给了旧版本二进制时跑：测试标了 `#[ignore]`，CI 的 force-cross-version 任务下载各个
//! 发布版本的无界面资产、把路径放进 `APROXY_PREV_BINS`（用系统的路径分隔符隔开）后以
//! `--ignored` 运行。旧版本的二进制只在测试的临时 home 里运行，它结束进程只按这个 home 的
//! 台账。

mod common;
use common::CopiedExecutable;
use common::{TestHome, free_port};

use aproxy::install::force;
use aproxy::install::swap::bin_path_in;
use aproxy_ipc::ledger::{self, LedgerEntry};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

/// 当前构建
const THIS_BUILD: &str = env!("CARGO_BIN_EXE_aproxy");

/// `APROXY_PREV_BINS` 里的旧版本二进制
fn earlier_releases() -> Vec<PathBuf> {
    let list = std::env::var_os("APROXY_PREV_BINS").expect("APROXY_PREV_BINS 未设置");
    let bins: Vec<PathBuf> = std::env::split_paths(&list)
        .filter(|p| !p.as_os_str().is_empty())
        .collect();
    assert!(!bins.is_empty(), "APROXY_PREV_BINS 里没有路径");
    for bin in &bins {
        assert!(bin.is_file(), "不是文件: {}", bin.display());
    }
    bins
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

/// `exe --version` 首行里的版本号（永久格式 `aproxy v<版本> (…)`，契约 C5）。`--version` 不读
/// home，但照样经测试的 home 运行：这个文件里的 aproxy 进程没有一个是带着用户环境跑的
fn version_of(home: &TestHome, exe: &Path) -> String {
    let out = home
        .cmd_with(exe)
        .arg("--version")
        .stdin(Stdio::null())
        .output_ready()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    force::VersionLine::parse(&text)
        .unwrap_or_else(|| panic!("{} 的 --version 不是永久格式: {text}", exe.display()))
        .version
}

/// 台账里此刻活着的条目
fn alive(home: &Path) -> Vec<LedgerEntry> {
    ledger::read_all_in(home)
        .into_iter()
        .map(|found| found.entry)
        .filter(ledger::is_alive)
        .collect()
}

/// 等台账里出现一条满足 `matches` 且仍在运行的条目
fn wait_for(
    home: &Path,
    timeout: Duration,
    matches: impl Fn(&LedgerEntry) -> bool,
) -> Option<LedgerEntry> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(found) = alive(home).into_iter().find(|e| matches(e)) {
            return Some(found);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// 一种组合：`running` 装在 `bin/` 里并启动一个实例（带看护者），然后由 `installer` 执行
/// `install --force --from <target 的副本> --yes`。
fn force_install_across(installer: &Path, target: &Path, running: &Path) {
    let home = TestHome::new();
    // 开着看护者：它也是要被结束的长期进程。这个形状旧版本读不懂时按它自己的默认运行，
    // 默认同样是开
    home.settings(r#"{"watchdog": {"enabled": true}}"#);
    let version_of = |exe: &Path| version_of(&home, exe);
    // 当前构建的版本号可能与某个发布版本相同（开发期间还没改号），所以单独标出来
    let name = |exe: &Path| {
        if exe == Path::new(THIS_BUILD) {
            format!("当前构建（{}）", version_of(exe))
        } else {
            format!("发布的 {}", version_of(exe))
        }
    };
    let what = format!(
        "执行者：{}；目标：{}；运行中的：{}",
        name(installer),
        name(target),
        name(running)
    );
    let bin = bin_path_in(home.path());
    copy_exe(running, &bin);
    let port = free_port();
    let cfg = home.path().join("main.toml");
    std::fs::write(
        &cfg,
        format!("base_url = \"https://main.example.com\"\nlisten_addr = \"127.0.0.1:{port}\"\n"),
    )
    .unwrap();
    let run = |exe: &Path, args: &[&str]| {
        let out = home
            .cmd_with(exe)
            .args(args)
            .stdin(Stdio::null())
            .output_ready()
            .unwrap();
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        (out.status.code(), text)
    };

    let (code, output) = run(&bin, &["start", "--config", cfg.to_str().unwrap()]);
    assert_eq!(code, Some(0), "{what}：start 失败: {output}");
    // 万一强制安装没能结束它：析构时用启动它的那个版本来停（别的版本可能联系不上它）
    home.stop_on_drop_with(running, port);
    let instance = wait_for(home.path(), Duration::from_secs(10), |e| {
        e.role == ledger::ROLE_INSTANCE && e.port == Some(port)
    })
    .unwrap_or_else(|| panic!("{what}：实例没有登记台账"));
    assert_eq!(instance.version, version_of(running), "{what}");
    wait_for(home.path(), Duration::from_secs(20), |e| {
        e.role == ledger::ROLE_WATCHDOG
    })
    .unwrap_or_else(|| panic!("{what}：看护者没有登记台账"));
    let before = alive(home.path());

    // `--from` 的源放在 home 之外的写法不必要：强制安装先把它拷进自己的工作目录
    let source = home
        .path()
        .join(format!("aproxy-target{}", std::env::consts::EXE_SUFFIX));
    copy_exe(target, &source);
    let (code, output) = run(
        installer,
        &[
            "install",
            "--force",
            "--from",
            source.to_str().unwrap(),
            "--yes",
        ],
    );

    assert!(
        code == Some(force::EXIT_DONE) || code == Some(force::EXIT_NOT_RESTORED),
        "{what}：退出码 {code:?}\n{output}"
    );
    // 留在测试输出里（`--nocapture`）：哪一种组合实例回来了、哪一种要手动启动
    eprintln!("{what}：退出码 {code:?}");
    for entry in &before {
        assert!(
            !ledger::is_alive(entry),
            "{what}：{} pid {} 还活着\n{output}",
            entry.role,
            entry.pid
        );
    }
    assert!(
        std::fs::read(&bin).unwrap() == std::fs::read(target).unwrap(),
        "{what}：bin 里不是目标二进制\n{output}"
    );
    let back = alive(home.path())
        .into_iter()
        .find(|e| e.role == ledger::ROLE_INSTANCE && e.port == Some(port));
    if code == Some(force::EXIT_DONE) {
        let back = back.unwrap_or_else(|| panic!("{what}：退出码 0 而实例不在台账里\n{output}"));
        assert_ne!(back.pid, instance.pid, "{what}");
        assert_eq!(back.version, version_of(target), "{what}");
    } else {
        assert!(back.is_none(), "{what}：退出码 12 而实例在台账里\n{output}");
        assert!(
            output.contains(&port.to_string()),
            "{what}：没回来的实例应按端口列出\n{output}"
        );
        // 端口已经放开：手动启动不会撞上残留的进程
        std::net::TcpListener::bind(("127.0.0.1", port))
            .unwrap_or_else(|e| panic!("{what}：端口 {port} 还被占着: {e}\n{output}"));
    }
}

#[test]
#[ignore = "需要 APROXY_PREV_BINS 指向旧版本的发布资产；CI 的 force-cross-version 任务下载后运行"]
fn this_build_cleans_up_after_an_earlier_release() {
    for old in earlier_releases() {
        force_install_across(Path::new(THIS_BUILD), Path::new(THIS_BUILD), &old);
    }
}

#[test]
#[ignore = "需要 APROXY_PREV_BINS 指向旧版本的发布资产；CI 的 force-cross-version 任务下载后运行"]
fn an_earlier_release_force_installs_this_build() {
    for old in earlier_releases() {
        force_install_across(&old, Path::new(THIS_BUILD), &old);
    }
}

#[test]
#[ignore = "需要 APROXY_PREV_BINS 指向旧版本的发布资产；CI 的 force-cross-version 任务下载后运行"]
fn an_earlier_release_replaces_this_build() {
    for old in earlier_releases() {
        force_install_across(&old, &old, Path::new(THIS_BUILD));
    }
}

#[test]
#[ignore = "需要 APROXY_PREV_BINS 指向旧版本的发布资产；CI 的 force-cross-version 任务下载后运行"]
fn this_build_force_installs_an_earlier_release() {
    for old in earlier_releases() {
        force_install_across(Path::new(THIS_BUILD), &old, Path::new(THIS_BUILD));
    }
}
