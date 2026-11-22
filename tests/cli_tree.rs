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
    assert!(all.contains("Starting aProxy"), "{all}");
    assert!(!all.contains("Log file:"), "说明文字应在 stderr: {all}");
    assert!(
        text(&out.stderr).contains("Log file:"),
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

/// 从真实帮助的 Commands 段发现下一层，既能覆盖以后增加的命令，也防止空遍历误过。
#[test]
fn help_tree_prints_help_for_every_command() {
    // 不含根与 help：顶层 12 个，alias 下 3 个，config、settings 各 4 个。
    const MIN_COMMAND_PATHS: usize = 23;
    let home = TestHome::new();
    let mut pending = vec![Vec::<String>::new()];
    let mut visited = 0;
    while let Some(path) = pending.pop() {
        let mut args: Vec<_> = path.iter().map(String::as_str).collect();
        args.push("--help");
        let out = run(&home, &args);
        assert_eq!(
            out.status.code(),
            Some(0),
            "{args:?}: {}",
            text(&out.stderr)
        );
        assert!(out.stderr.is_empty(), "{args:?}: {}", text(&out.stderr));
        let help = text(&out.stdout);
        assert!(!help.trim().is_empty(), "{args:?}");
        visited += usize::from(!path.is_empty());
        for line in help
            .lines()
            .skip_while(|line| *line != "Commands:")
            .skip(1)
            .take_while(|line| !line.is_empty())
        {
            let Some(entry) = line.strip_prefix("  ") else {
                continue;
            };
            // 续行是说明，不把它的第一个词当子命令。
            if entry.starts_with(char::is_whitespace) {
                continue;
            }
            if let Some(name) = entry.split_whitespace().next()
                && name != "help"
            {
                let mut child = path.clone();
                child.push(name.to_owned());
                pending.push(child);
            }
        }
    }
    assert!(
        visited >= MIN_COMMAND_PATHS,
        "{visited} < {MIN_COMMAND_PATHS}"
    );
}

/// 在原来的每个入口核对实际可见选项的排列（含隐藏命令）；命令名、选项名都不是翻译键。
#[test]
fn localized_help_preserves_baseline_option_order() {
    const BASELINE: &str = r#".: -V --version -h --help
__serve: --home --spec -h --help
__watchdog: --home -h --help
alias: -h --help
alias add: -h --help
alias help:
alias list: --json -h --help
alias remove: -h --help
config: --config -h --help
config get: --config --json -h --help
config help:
config set: --config --json -h --help
config show: --config --effective --json -h --help
config unset: --config --json -h --help
desktop: -h --help
doctor: --json -h --help
find: --aliased --unaliased --port --json -h --help
help:
install: --from --adopt --abort --force --yes --variant --cpu --allow-downgrade --pre --download-proxy --no-skills --check --json -h --help
logs: -n --lines -f --follow -h --help
restart: --idle-threshold-secs --force --json -h --help
restore: --json -h --help
restore-at-login: --home -h --help
settings: -h --help
settings get: --json -h --help
settings help:
settings set: --json -h --help
settings show: --json -h --help
settings unset: --json -h --help
skill: -h --help
skill __verify-package: --packaged-crate -h --help
skill help:
skill install: --download-proxy --json -h --help
skill list: --json -h --help
skill show: --files -h --help
start: --config --base-url --listen-addr --proxy --api-key --log-file --foreground --json -h --help
status: --idle --busy --json -h --help
stop: --idle-threshold-secs --force --json -h --help"#;
    let home = TestHome::new();
    home.settings(r#"{"language":"zh-CN"}"#);
    for row in BASELINE.lines() {
        let (path, expected) = row.split_once(':').unwrap();
        let path: Vec<_> = path.split_whitespace().filter(|p| *p != ".").collect();
        let mut args = path.clone();
        if args.last() == Some(&"help") {
            args.insert(0, "help");
        } else {
            args.push("--help");
        }
        for chinese in [false, true] {
            let mut cmd = home.cmd();
            if chinese {
                cmd.env_remove("APROXY_LANG");
            }
            let out = cmd.args(&args).output().unwrap();
            let help = text(&out.stdout);
            assert!(
                out.status.success() && out.stderr.is_empty(),
                "{args:?}: {}",
                text(&out.stderr)
            );
            assert!(
                help.lines()
                    .any(|s| s.starts_with(if chinese { "用法:" } else { "Usage:" })),
                "{args:?}: {help}"
            );
            assert!(
                help.trim_end().ends_with(
                    "Agents: run 'aproxy skill list' and 'aproxy skill show <name>' for full docs."
                ),
                "{args:?}: {help}"
            );
            let options: Vec<_> = help
                .lines()
                .filter(|line| {
                    let indent = line.len() - line.trim_start().len();
                    (2..=6).contains(&indent) && line.trim_start().starts_with('-')
                })
                .flat_map(|line| {
                    let mut fields = line.split_whitespace();
                    let first = fields.next().unwrap();
                    [
                        Some(first.trim_end_matches(',')),
                        if first.ends_with(',') {
                            fields.next()
                        } else {
                            None
                        },
                    ]
                    .into_iter()
                    .flatten()
                })
                .collect();
            assert_eq!(options.join(" "), expected.trim(), "{args:?}: {help}");
        }
    }
}

/// 真的触发一条 INFO，分别检查它所在的输出流和默认级别，而不是仅凭空日志推断两者。
#[test]
fn cli_logs_stay_off_stdout() {
    const INFO: &str =
        "A global default credential was not inherited because it belongs to a different target";
    let home = TestHome::new();
    let port = free_port();
    let cfg = config(&home, "logging", port);
    let cfg = cfg.to_str().unwrap();
    home.stop_on_drop(port);
    // 默认凭据属于另一个上游，启动时必记 INFO，无须为了产生事件而启用看护者。
    home.settings(r#"{"watchdog":{"enabled":false},"defaults":{"base_url":"https://other.example.com","api_key":"test-only"}}"#);
    let with_logs = |args: &[&str]| {
        home.cmd()
            .env("RUST_LOG", "info")
            .args(args)
            .stdin(Stdio::null())
            .output()
            .unwrap()
    };
    let start = ["start", cfg, "--json"];
    let port_arg = port.to_string();
    for args in [
        start.as_slice(),
        &["status", "--json"],
        &["alias", "list", "--json"],
        &["find", "--json"],
        &["settings", "show", "--json"],
        &["doctor", "--json"],
        &["stop", &port_arg, "--json"],
    ] {
        let out = with_logs(args);
        let stdout = text(&out.stdout);
        let stderr = text(&out.stderr);
        assert_eq!(
            stdout.lines().count(),
            1,
            "{args:?}: status={}, stdout={stdout:?}, stderr={stderr:?}",
            out.status
        );
        let doc: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(doc["schema"], 1, "{args:?}: {doc}");
        assert!(out.status.success(), "{args:?}: {doc} {stderr}");
        if args == start {
            assert!(stderr.contains(INFO), "{stderr}");
            assert!(!stderr.contains('\x1b'), "{stderr:?}");
        }
    }
    for args in [&["status"][..], &["settings", "show"]] {
        let out = with_logs(args);
        assert!(out.status.success(), "{args:?}: {}", text(&out.stderr));
        let stdout = text(&out.stdout);
        for level in [" INFO ", " WARN ", " DEBUG "] {
            assert!(!stdout.contains(level), "{stdout}");
        }
    }
    // 同一份配置停掉后再启动，扣下的凭据不变；不设置 RUST_LOG 时不显示那条 INFO。
    let quiet = home
        .cmd()
        .env_remove("RUST_LOG")
        .args(start)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        quiet.status.success(),
        "{}{}",
        text(&quiet.stdout),
        text(&quiet.stderr)
    );
    assert!(!text(&quiet.stderr).contains(INFO) && !text(&quiet.stdout).contains(INFO));
}

/// 探测应容忍失效的设置，即使日志消费者已关掉 stderr，仍要在另一条管道上答复 JSON。
#[test]
fn status_json_survives_a_closed_diagnostic_pipe() {
    let home = TestHome::new();
    home.settings("{");
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    let out = home
        .cmd()
        .args(["status", "--json"])
        .env("RUST_LOG", "info")
        .stderr(writer)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "status={}, stdout={:?}",
        out.status,
        text(&out.stdout)
    );
    let doc: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["schema"], 1);
    assert_eq!(doc["instances"], serde_json::json!([]));
}

/// 用法错误必须在解析层结束：连安装目录与 bin 都不能出现。
#[test]
fn install_check_and_json_reject_conflicting_modes() {
    let h = TestHome::new();
    for args in [
        &["install", "--force", "--json"][..],
        &["install", "--force", "--check"],
        &["upgrade", "--force", "--json"],
        &["install", "--check", "0.2.0"],
        &["install", "--check", "--from", "x"],
        &["install", "--abort", "--json"],
        &["install", "--continue", "--json"],
        &["install", "--skills-only", "--json"],
    ] {
        assert_eq!(run(&h, args).status.code(), Some(2), "{args:?}");
        assert!(!h.path().join("install").exists() && !h.path().join("bin").exists());
    }
}

/// 逐个已解析的 JSON 入口走 C1、设置预检及命令本身；诊断管道断开不能吞掉 stdout 文档。
#[test]
fn json_commands_finish_with_one_document_even_without_stderr() {
    let home = TestHome::new();
    home.settings("{");
    for (args, malformed) in [
        (&["start"][..], "config_invalid"),
        (&["status"], "instances"),
        (&["stop", "missing-alias"], "target_not_found"),
        (&["restart"], "config_invalid"),
        (&["restore"], "config_invalid"),
        (&["doctor"], "files"),
        (&["find"], "config_invalid"),
        (&["alias", "list"], "config_invalid"),
        (&["config", "show"], "config_invalid"),
        (&["config", "get", "base_url"], "config_invalid"),
        (&["config", "set", "base_url", "x"], "config_invalid"),
        (&["config", "unset", "base_url"], "config_invalid"),
        (&["settings", "show"], "config_invalid"),
        (&["settings", "get", "default_config"], "config_invalid"),
        (
            &["settings", "set", "default_config", "x"],
            "config_invalid",
        ),
        (&["settings", "unset", "default_config"], "config_invalid"),
        (
            &["install", "--check", "--download-proxy", "http://["],
            "install_download_failed",
        ),
        (
            &["install", "--from", "./missing-test-binary"],
            "install_probe_failed",
        ),
    ] {
        for relative in [true, false] {
            for broken in [false, true] {
                let mut command = if relative {
                    home.cmd_with_relative_home()
                } else {
                    home.cmd()
                };
                command
                    .current_dir(home.path())
                    .args(args)
                    .arg("--json")
                    .stdin(Stdio::null());
                if broken {
                    let (reader, writer) = std::io::pipe().unwrap();
                    drop(reader);
                    command.stderr(writer);
                }
                let out = command.output().unwrap();
                let stdout = text(&out.stdout);
                assert_eq!(
                    stdout.lines().count(),
                    1,
                    "{args:?} relative={relative} closed={broken} {}: {stdout:?} {}",
                    out.status,
                    text(&out.stderr)
                );
                let doc: serde_json::Value = serde_json::from_str(&stdout).unwrap();
                assert_eq!(doc["schema"], 1);
                if args.starts_with(&["install", "--from"]) {
                    assert_eq!(doc["event"], "result");
                    assert_eq!(doc["outcome"], "failed");
                } else {
                    assert!(doc.get("event").is_none());
                }
                let expected = if relative { "home_invalid" } else { malformed };
                if ["instances", "files"].contains(&expected) {
                    assert!(doc[expected].is_array(), "{args:?}: {doc}");
                } else {
                    assert!(!out.status.success());
                    assert_eq!(doc["error"]["code"], expected, "{args:?}: {doc}");
                    if !broken && (relative || args == ["restore"]) {
                        let message = doc["error"]["message"].as_str().unwrap();
                        assert!(
                            text(&out.stderr)
                                .lines()
                                .any(|line| line.strip_prefix("[ERROR] ") == Some(message)),
                            "{args:?}: {doc}"
                        );
                    }
                    assert!(
                        doc["error"]["message"].is_string() && doc["error"]["details"].is_object()
                    );
                }
            }
        }
    }
}

/// 配置与设置的文件读取失败、三处 stdin 读取失败都发生在预检之后，不能退化为空 stdout。
#[test]
fn json_command_io_errors_keep_a_result_after_preflight() {
    let home = TestHome::new();
    let directory = home.path().to_str().unwrap();
    for args in [
        &["config", "show", "--config", directory][..],
        &["start", "--api-key", "-"],
        &["config", "set", "base_url", "-"],
        &["settings", "set", "default_config", "-"],
    ] {
        let (reader, mut writer) = std::io::pipe().unwrap();
        writer.write_all(b"\xff\n").unwrap();
        drop(writer);
        let (closed, diagnostic) = std::io::pipe().unwrap();
        drop(closed);
        let out = home
            .cmd()
            .args(args)
            .arg("--json")
            .stdin(reader)
            .stderr(diagnostic)
            .output()
            .unwrap();
        assert_eq!(
            out.status.code(),
            Some(1),
            "{args:?}: {}",
            text(&out.stdout)
        );
        let doc: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(doc["schema"], 1);
        assert_eq!(doc["error"]["code"], "command_failed", "{args:?}: {doc}");
    }
    // 目录在各平台都不能当设置文件读，不依赖用户权限或 chmod。
    let file = home.path().join("settings.json");
    std::fs::rename(&file, home.path().join("initial-settings.json")).unwrap();
    std::fs::create_dir(file).unwrap();
    let out = run(&home, &["settings", "show", "--json"]);
    assert_eq!(out.status.code(), Some(1));
    let doc: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["error"]["code"], "command_failed");
}
