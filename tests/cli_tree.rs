//! 命令树的形状（设计稿 §9.1）：哪些写法被接受、不带子命令时做什么、几个旗标各自的规矩。
//! 只看命令行这一层——解析、退出码、输出落在哪一路；各命令做的事在各自的集成测试里。

mod common;
use common::{TestHome, free_port};

use std::io::Write as _;
use std::path::PathBuf;
use std::process::{Output, Stdio};

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// 在 `home` 里运行当前构建，stdin 为空
fn run(home: &TestHome, args: &[&str]) -> Output {
    home.cmd().args(args).stdin(Stdio::null()).output().unwrap()
}

/// 写一份指向不存在的上游、监听 `port` 的配置，返回它的路径
fn config(home: &TestHome, name: &str, port: u16) -> PathBuf {
    let path = home.path().join(format!("{name}.toml"));
    std::fs::write(
        &path,
        format!("base_url = \"https://{name}.example.com\"\nlisten_addr = \"127.0.0.1:{port}\"\n"),
    )
    .unwrap();
    path
}

/// 启动一个后台实例并登记析构时停掉它
fn start(home: &TestHome, port: u16, args: &[&str]) {
    let out = run(home, args);
    assert!(
        out.status.success(),
        "{args:?}: {}{}",
        text(&out.stdout),
        text(&out.stderr)
    );
    home.stop_on_drop(port);
}

/// 不带任何参数：帮助写到 stderr，退出码 2，什么都不启动。
#[test]
fn bare_invocation_prints_help_and_exits_2() {
    let home = TestHome::new();
    let out = run(&home, &[]);
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty(), "{}", text(&out.stdout));
    let help = text(&out.stderr);
    assert!(help.contains("Usage: aproxy"), "{help}");
    for command in ["start", "stop", "status", "install", "config"] {
        assert!(help.contains(command), "帮助里应列出 {command}: {help}");
    }
    assert!(
        !home.path().join("run").exists(),
        "不带子命令的调用不应启动任何东西"
    );
}

/// 启动的覆盖参数只属于 `start`：写在别的位置是用法错误。
#[test]
fn start_overrides_belong_to_start() {
    let home = TestHome::new();
    for args in [
        &["--listen-addr", "127.0.0.1:1", "start"][..],
        &["--config", "x.toml", "start"],
        &["--foreground"],
        &["status", "--base-url", "https://x.example.com"],
        // 0.1.x 的旗标名不再接受
        &["start", "--baseurl", "https://x.example.com"],
        &["start", "--listen", "127.0.0.1:1"],
        // 配置文件给两次
        &["start", "x.toml", "--config", "y.toml"],
    ] {
        let out = run(&home, args);
        assert_eq!(
            out.status.code(),
            Some(2),
            "{args:?}: {}",
            text(&out.stderr)
        );
    }
}

/// 配置文件可以直接写在 `start` 后面，也可以写成 `--config <文件>`；两种写法启动的是同一份
/// 配置。后一种是强制安装让用户执行的写法（永久）。
#[test]
fn start_takes_the_config_as_a_path_or_with_the_config_flag() {
    let home = TestHome::new();
    let (port_a, port_b) = (free_port(), free_port());
    let cfg_a = config(&home, "positional", port_a);
    let cfg_b = config(&home, "flagged", port_b);
    start(&home, port_a, &["start", cfg_a.to_str().unwrap()]);
    start(
        &home,
        port_b,
        &["start", "--config", cfg_b.to_str().unwrap()],
    );
    let status = text(&run(&home, &["status"]).stdout);
    assert!(
        status.contains(&port_a.to_string()) && status.contains(&port_b.to_string()),
        "{status}"
    );
}

/// `--api-key -`：密钥从 stdin 读一行，记进拉起规格的是读到的密钥，而不是那个 `-`。
#[test]
fn start_reads_the_api_key_from_stdin() {
    const SECRET: &str = "sk-FROM-STDIN-0123456789abcdef";
    let home = TestHome::new();
    let port = free_port();
    let cfg = config(&home, "stdin-key", port);
    let mut child = home
        .cmd()
        .args(["start", cfg.to_str().unwrap(), "--api-key", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(format!("{SECRET}\n").as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{}{}",
        text(&out.stdout),
        text(&out.stderr)
    );
    home.stop_on_drop(port);
    assert!(
        !text(&out.stdout).contains(SECRET) && !text(&out.stderr).contains(SECRET),
        "密钥不应出现在输出里"
    );
    let record =
        std::fs::read_to_string(home.path().join("run").join(format!("{port}.restore.json")))
            .unwrap();
    let record: serde_json::Value = serde_json::from_str(&record).unwrap();
    assert_eq!(record["spec"]["overrides"]["api_key"], SECRET, "{record}");
}

/// `--idle-threshold-secs` 只对目标 idle 有意义，配在别的目标上是用法错误。
#[test]
fn idle_threshold_requires_the_idle_target() {
    let home = TestHome::new();
    for args in [
        &["stop", "54321", "--idle-threshold-secs", "60"][..],
        &["stop", "--idle-threshold-secs", "60"],
        &["restart", "all", "--idle-threshold-secs", "60"],
        // 0.1.x 的写法：阈值作为第二个位置参数
        &["stop", "idle", "600"],
    ] {
        let out = run(&home, args);
        assert_eq!(
            out.status.code(),
            Some(2),
            "{args:?}: {}",
            text(&out.stderr)
        );
    }
    // 没有实例时 idle 无事可做，是成功
    let out = run(&home, &["stop", "idle", "--idle-threshold-secs", "60"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
}

/// `logs` 默认打印最近的若干行就退出，实例照常运行；`-n` 定行数；说明文字不进 stdout。
#[test]
fn logs_prints_recent_lines_and_exits() {
    let home = TestHome::new();
    let port = free_port();
    let cfg = config(&home, "logs-tail", port);
    start(&home, port, &["start", cfg.to_str().unwrap()]);
    let port_text = port.to_string();

    let out = run(&home, &["logs", &port_text]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let all = text(&out.stdout);
    assert!(all.contains("启动 aProxy"), "{all}");
    assert!(!all.contains("日志文件"), "说明文字应在 stderr: {all}");
    assert!(
        text(&out.stderr).contains("日志文件"),
        "{}",
        text(&out.stderr)
    );

    let out = run(&home, &["logs", &port_text, "-n", "1"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        text(&out.stdout).lines().count(),
        1,
        "{}",
        text(&out.stdout)
    );
    let out = run(&home, &["logs", "--lines", "0", &port_text]);
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty(), "{}", text(&out.stdout));

    // 实例没有因为 logs 退出而受影响
    let status = text(&run(&home, &["status"]).stdout);
    assert!(status.contains(&port_text), "{status}");
}

/// `upgrade` 是 `install` 的别名：两个名字接受同样的参数，帮助里看得见。
#[test]
fn upgrade_is_an_alias_of_install() {
    let home = TestHome::new();
    let help = text(&run(&home, &["--help"]).stdout);
    assert!(help.contains("[alias: upgrade]"), "{help}");
    let install = text(&run(&home, &["install", "--help"]).stdout);
    let upgrade = text(&run(&home, &["upgrade", "--help"]).stdout);
    assert!(install.contains("--from"), "{install}");
    assert_eq!(
        install.replace("aproxy install", "aproxy upgrade"),
        upgrade.replace("aproxy install", "aproxy upgrade")
    );
}
