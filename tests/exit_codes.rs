//! 各命令的退出码（设计稿 §9.5 的表）：每一格一个断言，连同 `--json` 时 stdout 上的文档——
//! 成功时是命令自己的文档，整条命令失败时是 `{"schema":1,"error":{"code","message","details"}}`。
//!
//! 退出码一公布就是契约：agent 与脚本靠它与 `--json` 判断结果，不读给人看的文字。

mod common;
use common::ReportExit as _;
use common::{ChildGuard, TestHome, free_port};

use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::time::Duration;

use serde_json::Value;

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// 在 `home` 里运行当前构建，stdin 为空
fn run(home: &TestHome, args: &[&str]) -> Output {
    home.cmd()
        .args(args)
        .stdin(Stdio::null())
        .output()
        .report_exit()
        .unwrap()
}

/// stdout 上恰好是一行 JSON，`schema` 为 1
fn document(out: &Output, args: &[&str]) -> Value {
    let stdout = text(&out.stdout);
    assert_eq!(
        stdout.lines().count(),
        1,
        "{args:?}: stdout={stdout} stderr={}",
        text(&out.stderr)
    );
    let doc: Value =
        serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("{args:?}: {e}: {stdout}"));
    assert_eq!(doc["schema"], 1, "{args:?}: {doc}");
    doc
}

/// 运行一条带 `--json` 的命令，核对退出码，返回文档
fn run_json(home: &TestHome, args: &[&str], exit: i32) -> Value {
    let out = run(home, args);
    assert_eq!(
        out.status.code(),
        Some(exit),
        "{args:?}: stdout={} stderr={}",
        text(&out.stdout),
        text(&out.stderr)
    );
    document(&out, args)
}

/// 整条命令失败：核对退出码与错误文档里的码，返回 `error.details`
fn failure(home: &TestHome, args: &[&str], exit: i32, code: &str) -> Value {
    let doc = run_json(home, args, exit);
    assert_eq!(doc["error"]["code"], code, "{args:?}: {doc}");
    assert!(
        doc["error"]["message"]
            .as_str()
            .is_some_and(|m| !m.is_empty()),
        "{args:?}: {doc}"
    );
    // 不带 --json 时退出码相同，stdout 上没有文档
    let plain: Vec<&str> = args.iter().copied().filter(|a| *a != "--json").collect();
    let out = run(home, &plain);
    assert_eq!(
        out.status.code(),
        Some(exit),
        "{plain:?}: {}",
        text(&out.stderr)
    );
    assert!(out.stdout.is_empty(), "{plain:?}: {}", text(&out.stdout));
    doc["error"]["details"].clone()
}

/// 写一份监听 `port` 的有效配置，返回它的路径
fn config(home: &TestHome, name: &str, port: u16) -> PathBuf {
    let path = home.path().join(format!("{name}.toml"));
    std::fs::write(
        &path,
        format!("base_url = \"https://{name}.example.com\"\nlisten_addr = \"127.0.0.1:{port}\"\n"),
    )
    .unwrap();
    path
}

// ---------------------------------------------------------------------------
// start
// ---------------------------------------------------------------------------

/// 0：启动了；再启动同一份配置，是「已在运行」，同样是 0。
#[test]
fn start_exits_0_when_started_or_already_running() {
    let home = TestHome::new();
    let port = free_port();
    let cfg = config(&home, "start-ok", port);
    let cfg = cfg.to_str().unwrap();

    let started = run_json(&home, &["start", cfg, "--json"], 0);
    home.stop_on_drop(port);
    assert_eq!(started["outcome"], "started", "{started}");
    assert_eq!(started["port"], port);
    assert!(
        started["pid"].as_u64().is_some_and(|pid| pid > 0),
        "{started}"
    );
    assert!(
        started["log_path"]
            .as_str()
            .is_some_and(|p| PathBuf::from(p).is_file()),
        "{started}"
    );
    assert_eq!(
        started["instance"]["instance"]["pid"], started["pid"],
        "{started}"
    );

    let again = run_json(&home, &["start", cfg, "--json"], 0);
    assert_eq!(again["outcome"], "already_running", "{again}");
    assert_eq!(again["pid"], started["pid"]);
    assert_eq!(again["port"], port);
}

/// 1：端口被别的程序占着。
#[test]
fn start_exits_1_when_the_port_is_taken() {
    let home = TestHome::new();
    let holder = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = holder.local_addr().unwrap().port();
    let cfg = config(&home, "start-taken", port);
    let details = failure(
        &home,
        &["start", cfg.to_str().unwrap(), "--json"],
        1,
        "port_in_use",
    );
    assert_eq!(details["port"], port, "{details}");
    assert_eq!(details["owner"], "unknown", "{details}");
}

/// 2：用法错误——`--json` 与 `--foreground` 不能同用。
#[test]
fn start_exits_2_on_usage_errors() {
    let home = TestHome::new();
    let out = run(&home, &["start", "--json", "--foreground"]);
    assert_eq!(out.status.code(), Some(2), "{}", text(&out.stderr));
    // 参数解析器报的用法错误不受 --json 影响：stdout 上没有文档
    assert!(out.stdout.is_empty(), "{}", text(&out.stdout));
}

/// 3：配置无效，诊断在错误文档里。
#[test]
fn start_exits_3_when_the_config_is_invalid() {
    let home = TestHome::new();
    let cfg = home.path().join("start-invalid.toml");
    std::fs::write(&cfg, "base_url = \"https://x.example.com\"\nnope = 1\n").unwrap();
    let details = failure(
        &home,
        &["start", cfg.to_str().unwrap(), "--json"],
        3,
        "config_invalid",
    );
    assert_eq!(
        details["diagnostics"][0]["code"], "config_unknown_key",
        "{details}"
    );
    assert_eq!(details["diagnostics"][0]["key"], "nope");

    // 没有 base_url 也是配置无效
    let empty = home.path().join("start-empty.toml");
    std::fs::write(&empty, "").unwrap();
    let details = failure(
        &home,
        &["start", empty.to_str().unwrap(), "--json"],
        3,
        "config_invalid",
    );
    assert_eq!(
        details["diagnostics"][0]["code"], "config_missing_key",
        "{details}"
    );
}

/// 4：别名不存在、配置文件不存在、默认配置文件的指向失效。
#[test]
fn start_exits_4_when_the_alias_or_file_does_not_exist() {
    let home = TestHome::new();
    let details = failure(
        &home,
        &["start", "no-such-alias", "--json"],
        4,
        "target_not_found",
    );
    assert_eq!(details["target"], "no-such-alias", "{details}");

    let missing = home.path().join("missing.toml");
    let missing = missing.to_str().unwrap();
    for args in [
        &["start", missing, "--json"][..],
        &["start", "--config", missing, "--json"],
    ] {
        failure(&home, args, 4, "target_not_found");
    }

    std::fs::write(
        home.path().join("settings.json"),
        serde_json::json!({"schema": 1, "default_config": missing}).to_string(),
    )
    .unwrap();
    let details = failure(&home, &["start", "--json"], 4, "target_not_found");
    assert!(
        details["target"]
            .as_str()
            .is_some_and(|t| t.ends_with("missing.toml")),
        "{details}"
    );
}

// ---------------------------------------------------------------------------
// 以下各行共用的辅助
// ---------------------------------------------------------------------------

/// 不带 `--json` 的命令（logs）失败：核对退出码，stdout 上什么都没有，返回 stderr
fn plain_failure(home: &TestHome, args: &[&str], exit: i32) -> String {
    let out = run(home, args);
    assert_eq!(
        out.status.code(),
        Some(exit),
        "{args:?}: stdout={} stderr={}",
        text(&out.stdout),
        text(&out.stderr)
    );
    assert!(out.stdout.is_empty(), "{args:?}: {}", text(&out.stdout));
    text(&out.stderr)
}

/// 在 `home` 里启动一个实例（测试结束时停掉），返回它的端口、pid 与配置文件
fn start_instance(home: &TestHome, name: &str) -> (u16, u64, PathBuf) {
    let port = free_port();
    let cfg = config(home, name, port);
    home.stop_on_drop(port);
    let doc = run_json(home, &["start", cfg.to_str().unwrap(), "--json"], 0);
    (port, doc["pid"].as_u64().unwrap(), cfg)
}

/// 模拟崩溃：按 pid 强行结束本测试自己启动的实例，等它的端口不再接受连接。期望状态记录
/// 留在原处，正是崩溃之后的样子。
fn crash(pid: u64, port: u16) {
    #[cfg(windows)]
    let status = Command::new("taskkill")
        .args(["/F", "/PID", &pid.to_string()])
        .stdout(Stdio::null())
        .status()
        .report_exit();
    #[cfg(unix)]
    let status = Command::new("kill")
        .args(["-9", &pid.to_string()])
        .status()
        .report_exit();
    assert!(status.unwrap().success(), "结束 pid {pid} 失败");
    for _ in 0..100 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("pid {pid} 被结束之后，端口 {port} 仍在接受连接");
}

/// 冒充一个挂死的实例：注册表里有它的记录（pid 与启动标记核验得上），端口上却没有控制
/// 端点。返回哑进程（析构时结束）与记录里的端口。
fn hung_instance(home: &TestHome) -> (ChildGuard, u16) {
    let port = free_port();
    #[cfg(windows)]
    let dummy = Command::new("ping")
        .args(["-n", "120", "127.0.0.1"])
        .stdout(Stdio::null())
        .spawn();
    #[cfg(unix)]
    let dummy = Command::new("sleep").arg("120").spawn();
    let dummy = ChildGuard(dummy.expect("拉起哑进程失败"));
    let pid = dummy.0.id();
    let start = aproxy_ipc::ledger::start_marker(pid).expect("读不到哑进程的启动标记");
    let info = aproxy::daemon::InstanceRecord {
        launch_reason: "unknown".into(),
        launch_note: String::new(),
        pid,
        start,
        instance_id: "19ac3f2e8b5d1a2b".into(),
        version: "0.0.0-test".into(),
        listen_addr: format!("127.0.0.1:{port}"),
        config_path: Some(home.path().join("hung.toml").display().to_string()),
        base_url: "https://api.example.com".into(),
        started_at: 1_700_000_000_000,
        log_path: None,
        foreground: false,
    };
    aproxy::daemon::write_instance_file_in(&home.path().join("run"), &info).unwrap();
    (dummy, port)
}

/// 逐实例结果文档：核对 `summary`，返回 `results`
fn results(doc: &Value, ok: u64, failed: u64) -> Vec<Value> {
    assert_eq!(doc["summary"]["ok"], ok, "{doc}");
    assert_eq!(doc["summary"]["failed"], failed, "{doc}");
    doc["results"].as_array().cloned().unwrap_or_default()
}

/// 结果里端口为 `port` 的那一项的结局
fn outcome_of(items: &[Value], port: u16) -> Option<Value> {
    items
        .iter()
        .find(|item| item["port"] == port)
        .map(|item| item["outcome"].clone())
}

// ---------------------------------------------------------------------------
// stop
// ---------------------------------------------------------------------------

/// 0：停了；已经停了（再停一次）；idle 目标下没有闲置的实例，什么都不停。
#[test]
fn stop_exits_0_when_stopped_already_stopped_or_nothing_is_idle() {
    let home = TestHome::new();
    let (port, _, _) = start_instance(&home, "stop-ok");
    let (busy, _, _) = start_instance(&home, "stop-busy");
    let p = port.to_string();

    let none_idle = run_json(
        &home,
        &["stop", "idle", "--idle-threshold-secs", "3600", "--json"],
        0,
    );
    assert!(results(&none_idle, 0, 0).is_empty(), "{none_idle}");

    let doc = run_json(&home, &["stop", &p, "--json"], 0);
    let items = results(&doc, 1, 0);
    assert_eq!(items[0]["target"], p.as_str(), "{doc}");
    assert_eq!(items[0]["port"], port, "{doc}");
    assert_eq!(items[0]["outcome"], "stopped", "{doc}");

    let again = run_json(&home, &["stop", &p, "--json"], 0);
    assert_eq!(results(&again, 1, 0)[0]["outcome"], "already_stopped");

    // idle 目标没有停掉刚启动的那个实例
    let doc = run_json(&home, &["stop", &busy.to_string(), "--json"], 0);
    assert_eq!(results(&doc, 1, 0)[0]["outcome"], "stopped", "{doc}");
}

/// 1：停不掉——实例的进程还在，却不应答控制通道。
#[test]
fn stop_exits_1_when_the_instance_cannot_be_stopped() {
    let home = TestHome::new();
    let (mut dummy, port) = hung_instance(&home);
    let p = port.to_string();
    let doc = run_json(&home, &["stop", &p, "--json"], 1);
    let items = results(&doc, 0, 1);
    assert_eq!(items[0]["outcome"], "failed", "{doc}");
    assert_eq!(items[0]["error"]["code"], "instance_unresponsive", "{doc}");
    assert_eq!(items[0]["error"]["details"]["port"], port, "{doc}");
    assert_eq!(items[0]["error"]["details"]["pid"], dummy.0.id(), "{doc}");
    plain_failure(&home, &["stop", &p], 1);
    assert!(
        dummy.0.try_wait().report_exit().unwrap().is_none(),
        "不带 --force 不得结束进程"
    );
}

/// 2：多个实例在运行而没指定目标（stop、restart、logs 相同）；`--idle-threshold-secs`
/// 配在 idle 以外的目标上是用法错误。哪一条都不动实例。
#[test]
fn stop_restart_and_logs_exit_2_without_a_target_among_several_instances() {
    let home = TestHome::new();
    let (a, _, _) = start_instance(&home, "ambiguous-a");
    let (b, _, _) = start_instance(&home, "ambiguous-b");
    for cmd in ["stop", "restart"] {
        let details = failure(&home, &[cmd, "--json"], 2, "target_ambiguous");
        let ports = details["ports"].as_array().unwrap();
        assert!(
            ports.contains(&a.into()) && ports.contains(&b.into()),
            "{details}"
        );
        let details = failure(
            &home,
            &[cmd, &a.to_string(), "--idle-threshold-secs", "5", "--json"],
            2,
            "usage",
        );
        assert_eq!(details["reason"], "idle_threshold_without_idle");
    }
    let stderr = plain_failure(&home, &["logs"], 2);
    assert!(stderr.contains(&format!("aproxy logs {a}")), "{stderr}");
    plain_failure(&home, &["logs", "all"], 2);
    let doc = run_json(&home, &["stop", "all", "--json"], 0);
    let items = results(&doc, 2, 0);
    assert_eq!(outcome_of(&items, a), Some("stopped".into()), "{doc}");
    assert_eq!(outcome_of(&items, b), Some("stopped".into()), "{doc}");
}

/// 4：别名不存在。
#[test]
fn stop_exits_4_when_the_alias_does_not_exist() {
    let home = TestHome::new();
    let details = failure(
        &home,
        &["stop", "no-such-alias", "--json"],
        4,
        "target_not_found",
    );
    assert_eq!(details["target"], "no-such-alias", "{details}");
}

/// 5：部分失败——`stop all` 停掉了能停的，挂死的那个停不掉。
#[test]
fn stop_exits_5_when_some_instances_cannot_be_stopped() {
    let home = TestHome::new();
    let (port, _, _) = start_instance(&home, "partial-stop");
    let (_dummy, hung) = hung_instance(&home);
    let doc = run_json(&home, &["stop", "all", "--json"], 5);
    let items = results(&doc, 1, 1);
    assert_eq!(outcome_of(&items, port), Some("stopped".into()), "{doc}");
    assert_eq!(outcome_of(&items, hung), Some("failed".into()), "{doc}");
}

// ---------------------------------------------------------------------------
// restart
// ---------------------------------------------------------------------------

/// 0：重启了（新实例换了 pid）；idle 目标下没有闲置的实例，跳过。
#[test]
fn restart_exits_0_when_restarted_or_nothing_is_idle() {
    let home = TestHome::new();
    let (port, pid, cfg) = start_instance(&home, "restart-ok");
    let none_idle = run_json(
        &home,
        &["restart", "idle", "--idle-threshold-secs", "3600", "--json"],
        0,
    );
    assert!(results(&none_idle, 0, 0).is_empty(), "{none_idle}");
    let doc = run_json(&home, &["restart", &port.to_string(), "--json"], 0);
    let items = results(&doc, 1, 0);
    assert_eq!(items[0]["outcome"], "restarted", "{doc}");
    assert_eq!(items[0]["port"], port, "{doc}");
    let now = run_json(&home, &["start", cfg.to_str().unwrap(), "--json"], 0);
    assert_eq!(now["outcome"], "already_running", "{now}");
    assert_ne!(now["pid"], pid, "{now}");
}

/// 1：前台实例（没有可用来重新拉起它的记录），连同 logs 的 1：前台实例没有日志文件。
#[test]
fn restart_and_logs_exit_1_for_a_foreground_instance() {
    let home = TestHome::new();
    let port = free_port();
    let cfg = config(&home, "foreground", port);
    home.stop_on_drop(port);
    let _fg = ChildGuard(
        home.cmd()
            .args(["start", cfg.to_str().unwrap(), "--foreground"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let p = port.to_string();
    // 就绪之前 logs 报「没在运行」（4）；应答之后是「没有日志文件」（1）
    let mut code = None;
    for _ in 0..150 {
        code = run(&home, &["logs", &p]).status.code();
        if code == Some(1) {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(code, Some(1), "前台实例没有就绪，或 logs 没报 no_log_file");
    let stderr = plain_failure(&home, &["logs", &p], 1);
    assert!(stderr.contains("--foreground"), "{stderr}");

    let doc = run_json(&home, &["restart", &p, "--json"], 1);
    let items = results(&doc, 0, 1);
    assert_eq!(items[0]["error"]["code"], "restart_unsupported", "{doc}");
    assert_eq!(items[0]["error"]["details"]["port"], port, "{doc}");
}

/// 3：预检发现配置无效，旧实例不动。
#[test]
fn restart_exits_3_when_the_precheck_finds_the_config_invalid() {
    let home = TestHome::new();
    let (port, _, cfg) = start_instance(&home, "restart-invalid");
    std::fs::write(
        &cfg,
        format!(
            "base_url = \"https://x.example.com\"\nlisten_addr = \"127.0.0.1:{port}\"\nnope = 1\n"
        ),
    )
    .unwrap();
    let doc = run_json(&home, &["restart", &port.to_string(), "--json"], 3);
    let items = results(&doc, 0, 1);
    assert_eq!(items[0]["error"]["code"], "config_invalid", "{doc}");
    assert_eq!(
        items[0]["error"]["details"]["diagnostics"][0]["code"], "config_unknown_key",
        "{doc}"
    );
    // 旧实例还在：stop 停的是它
    let stop = run_json(&home, &["stop", &port.to_string(), "--json"], 0);
    assert_eq!(results(&stop, 1, 0)[0]["outcome"], "stopped", "{stop}");
}

/// 4：目标没在运行（restart 不启动它），或不存在。
#[test]
fn restart_exits_4_when_the_target_is_not_running_or_does_not_exist() {
    let home = TestHome::new();
    let details = failure(&home, &["restart", "--json"], 4, "target_not_running");
    assert_eq!(details["target"], Value::Null, "{details}");
    let p = free_port().to_string();
    let details = failure(&home, &["restart", &p, "--json"], 4, "target_not_running");
    assert_eq!(details["target"], p.as_str(), "{details}");
    let cfg = config(&home, "restart-stopped", free_port());
    failure(
        &home,
        &["restart", cfg.to_str().unwrap(), "--json"],
        4,
        "target_not_running",
    );
    failure(
        &home,
        &["restart", "no-such-alias", "--json"],
        4,
        "target_not_found",
    );
}

/// 5：部分失败——`restart all` 重启了能重启的，挂死的那个处理不了。
#[test]
fn restart_exits_5_when_some_instances_fail() {
    let home = TestHome::new();
    let (port, _, _) = start_instance(&home, "partial-restart");
    let (_dummy, hung) = hung_instance(&home);
    let doc = run_json(&home, &["restart", "all", "--json"], 5);
    let items = results(&doc, 1, 1);
    assert_eq!(outcome_of(&items, port), Some("restarted".into()), "{doc}");
    assert_eq!(outcome_of(&items, hung), Some("failed".into()), "{doc}");
}

// ---------------------------------------------------------------------------
// logs（1 与 2 在上面的前台实例、多实例两个测试里，0 在 restore 的测试里）
// ---------------------------------------------------------------------------

/// 4：没在运行，也没有记录可读；别名不存在。
#[test]
fn logs_exits_4_when_there_is_nothing_to_read() {
    let home = TestHome::new();
    plain_failure(&home, &["logs"], 4);
    let stderr = plain_failure(&home, &["logs", &free_port().to_string()], 4);
    assert!(stderr.contains("No running aProxy instance"), "{stderr}");
    plain_failure(&home, &["logs", "no-such-alias"], 4);
}

// ---------------------------------------------------------------------------
// restore（连同 logs 的 0：实例在运行时读它的日志，崩溃之后读记录里的日志）
// ---------------------------------------------------------------------------

/// 0：没有记录时无事可做；崩溃的实例恢复了；再执行一次是「已在运行」。
#[test]
fn restore_exits_0_when_everything_is_restored_or_there_is_nothing_to_do() {
    let home = TestHome::new();
    let empty = run_json(&home, &["restore", "--json"], 0);
    assert!(results(&empty, 0, 0).is_empty(), "{empty}");

    let (port, pid, _) = start_instance(&home, "restore-ok");
    let p = port.to_string();
    let out = run(&home, &["logs", &p, "-n", "5"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));

    crash(pid, port);
    // 实例没在运行，期望状态记录还在：logs 读记录里的日志（指定端口与省略目标都是）
    for args in [&["logs", &p, "-n", "5"][..], &["logs", "-n", "5"]] {
        let out = run(&home, args);
        assert_eq!(
            out.status.code(),
            Some(0),
            "{args:?}: {}",
            text(&out.stderr)
        );
        assert!(
            text(&out.stderr).contains("is not running"),
            "{args:?}: {}",
            text(&out.stderr)
        );
    }

    let doc = run_json(&home, &["restore", "--json"], 0);
    let items = results(&doc, 1, 0);
    assert_eq!(items[0]["outcome"], "restored", "{doc}");
    assert_eq!(items[0]["port"], port, "{doc}");
    assert_eq!(items[0]["target"], Value::Null, "{doc}");

    let again = run_json(&home, &["restore", "--json"], 0);
    assert_eq!(results(&again, 1, 0)[0]["outcome"], "already_running");
}

/// 1：全部失败——记录在，配置却改坏了，实例拉不起来。
#[test]
fn restore_exits_1_when_every_record_fails() {
    let home = TestHome::new();
    let (port, pid, cfg) = start_instance(&home, "restore-fail");
    crash(pid, port);
    std::fs::write(
        &cfg,
        format!(
            "base_url = \"https://x.example.com\"\nlisten_addr = \"127.0.0.1:{port}\"\nnope = 1\n"
        ),
    )
    .unwrap();
    let doc = run_json(&home, &["restore", "--json"], 1);
    let items = results(&doc, 0, 1);
    assert_eq!(items[0]["outcome"], "failed", "{doc}");
    assert_eq!(items[0]["error"]["code"], "start_failed", "{doc}");
    assert!(
        items[0]["error"]["details"]["startup_log"].is_array(),
        "{doc}"
    );
}

/// 5：有配置文件暂时不在的记录——没有恢复，记录留着等文件回来；stop 删掉它之后无事可做。
#[test]
fn restore_exits_5_when_a_config_is_missing() {
    let home = TestHome::new();
    let (port, pid, cfg) = start_instance(&home, "restore-missing");
    crash(pid, port);
    std::fs::remove_file(&cfg).unwrap();
    let doc = run_json(&home, &["restore", "--json"], 5);
    let items = results(&doc, 0, 0);
    assert_eq!(items[0]["outcome"], "config_missing", "{doc}");
    assert_eq!(items[0]["port"], port, "{doc}");
    // 记录还在：再执行一次结局相同
    run_json(&home, &["restore", "--json"], 5);

    let stop = run_json(&home, &["stop", &port.to_string(), "--json"], 0);
    assert_eq!(
        results(&stop, 1, 0)[0]["outcome"],
        "already_stopped",
        "{stop}"
    );
    let empty = run_json(&home, &["restore", "--json"], 0);
    assert!(results(&empty, 0, 0).is_empty(), "{empty}");
}

// ---------------------------------------------------------------------------
// config、settings
// ---------------------------------------------------------------------------

/// config：0 完成；1 写冲突（写锁一直被别人拿着）；2 用法；3 未知键、结果无效；
/// 4 `config get` 时文件不存在。
#[test]
fn config_exit_codes() {
    let home = TestHome::new();
    let cfg = config(&home, "config-codes", free_port());
    let c = cfg.to_str().unwrap();
    let set = |key: &str, value: &str| {
        ["config", "set", key, value, "--config", c, "--json"].map(str::to_string)
    };
    let args = set("base_url", "https://y.example.com");
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let doc = run_json(&home, &args, 0);
    assert_eq!(doc["value"], "https://y.example.com", "{doc}");

    for (key, value, exit, code) in [
        ("a..b", "1", 2, "usage"),
        ("nope", "1", 3, "config_unknown_key"),
        ("listen_addr", "not-an-address", 3, "config_invalid"),
    ] {
        let args = set(key, value);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        failure(&home, &args, exit, code);
    }
    let missing = home.path().join("missing.toml");
    failure(
        &home,
        &[
            "config",
            "get",
            "base_url",
            "--config",
            missing.to_str().unwrap(),
            "--json",
        ],
        4,
        "target_not_found",
    );

    // 写锁被测试自己拿着：等满时限仍拿不到，没有保存
    let lock_path = home.path().join("run").join("config-write.lock");
    std::fs::create_dir_all(lock_path.parent().unwrap()).unwrap();
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .unwrap();
    lock.try_lock().unwrap();
    let args = set("base_url", "https://z.example.com");
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let doc = run_json(&home, &args, 1);
    assert_eq!(doc["error"]["code"], "write_conflict", "{doc}");
    drop(lock);
    assert!(
        std::fs::read_to_string(&cfg)
            .unwrap()
            .contains("y.example.com"),
        "写冲突时不得改动文件"
    );
}

/// settings：0 完成；2 用法；3 未知键、结果无效。
#[test]
fn settings_exit_codes() {
    let home = TestHome::new();
    for (args, exit) in [
        (
            &["settings", "set", "instances.idle_threshold_secs", "900"][..],
            0,
        ),
        (&["settings", "set", "a..b", "1"], 2),
        (&["settings", "set", "nope", "1"], 3),
        (
            &["settings", "set", "instances.idle_threshold_secs", "\"x\""],
            3,
        ),
    ] {
        let out = run(&home, args);
        assert_eq!(
            out.status.code(),
            Some(exit),
            "{args:?}: {}",
            text(&out.stderr)
        );
    }
}

/// 体检给完整报告而不是失败文档；只有错误使文字和 JSON 两种模式都返回 3。
#[test]
fn doctor_exit_codes() {
    let home = TestHome::new();
    for exit in [0, 3] {
        if exit == 3 {
            home.settings(r#"{"schema":1,"nope":1}"#);
        }
        let doc = run_json(&home, &["doctor", "--json"], exit);
        assert_eq!(doc["summary"]["errors"], usize::from(exit == 3));
        let plain = run(&home, &["doctor"]);
        assert_eq!(plain.status.code(), Some(exit));
        if exit == 3 {
            assert!(
                text(&plain.stdout).contains("    Remove this key: aproxy settings unset 'nope'")
            );
        }
    }
}

#[cfg(windows)]
#[test]
fn windows_abnormal_exit_is_not_a_cli_exit_code() {
    use std::os::windows::process::ExitStatusExt;
    use std::process::ExitStatus;
    assert_eq!(
        common::status_text(ExitStatus::from_raw(0xC0000374)),
        "abnormal termination: 0xC0000374"
    );
    assert_eq!(
        common::status_text(ExitStatus::from_raw(2)),
        "exit status: 0x00000002"
    );
}
