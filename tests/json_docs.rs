//! 按设计稿 §9.4 把各命令的 `--json` 输出作为整份文档钉住：agent 和脚本会解析它们，
//! 文档形状只要变了就是破坏性变更。

mod common;
use common::ReportExit as _;
use common::{TestHome, free_port};

use std::fs;
use std::process::{Command, Output, Stdio};
use std::time::Duration;

use aproxy::settings::path_match_key;
use serde_json::{Value, json};

/// 在隔离的 home 里运行命令，核对退出码为 0，并保留两个输出流供分别检查。
fn run(home: &TestHome, args: &[&str]) -> Output {
    let out = home
        .cmd()
        .args(args)
        .stdin(Stdio::null())
        .output()
        .report_exit()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

/// stdout 只能有一行，以版本字段起头；提示文字不能混进文档。
fn document(out: &Output) -> Value {
    let stdout = std::str::from_utf8(&out.stdout).unwrap();
    assert_eq!(stdout.lines().count(), 1, "{stdout}");
    assert!(stdout.starts_with(r#"{"schema":1,"#), "{stdout}");
    serde_json::from_str(stdout).unwrap()
}

/// 失败也只能给一份文档；分别保留解析结果和 stderr，便于核对退出码对应的诊断与修法。
fn failed_document(home: &TestHome, args: &[&str], code: i32) -> (Value, String) {
    let out = home
        .cmd()
        .args(args)
        .stdin(Stdio::null())
        .output()
        .report_exit()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(code),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    (document(&out), String::from_utf8(out.stderr).unwrap())
}

/// 按集合核对字段：多一个或少一个都不行，但不约束对象内部的排列顺序。
fn assert_keys(value: &Value, keys: &[&str]) {
    let obj = value.as_object().unwrap();
    assert_eq!(obj.len(), keys.len(), "{value}");
    for key in keys {
        assert!(obj.contains_key(*key), "缺少 {key}: {value}");
    }
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

/// 用户提供的说明只属于一次启动。两个实例共享同一条批量命令，各自用自己的配置脱敏。
#[test]
fn launch_metadata_follows_each_launch_not_the_desired_state() {
    let home = TestHome::new();
    let run_dir = home.path().join("run");
    let key = "test-launch-private-key";
    let note = format!("handoff by {key}");
    let mut ports = Vec::new();
    for name in ["one", "two"] {
        let cfg = home.path().join(format!("{name}.toml"));
        fs::write(
            &cfg,
            format!("listen_addr='127.0.0.1:0'\nbase_url='http://127.0.0.1:9'\napi_key='{key}'\n"),
        )
        .unwrap();
        let started = document(&run(
            &home,
            &[
                "start",
                cfg.to_str().unwrap(),
                "--reason",
                "plugin",
                "--note",
                &note,
                "--json",
            ],
        ));
        let port = home.track_started_port(&started);
        ports.push(port);
        let record: Value = serde_json::from_slice(
            &fs::read(run_dir.join(format!("{port}.instance.json"))).unwrap(),
        )
        .unwrap();
        assert_eq!(record["launch_reason"], "plugin");
        assert_eq!(record["launch_note"], "handoff by <secret>");
        let log = fs::read_to_string(record["log_path"].as_str().unwrap()).unwrap();
        assert!(
            log.contains("launch_reason=plugin")
                && log.contains("handoff by <secret>")
                && !log.contains(key),
            "{log}"
        );
        let restore = fs::read_to_string(run_dir.join(format!("{port}.restore.json"))).unwrap();
        for forbidden in [
            "plugin",
            "handoff by",
            "launch_reason",
            "launch_note",
            "\"launch\"",
        ] {
            assert!(!restore.contains(forbidden));
        }
    }
    let check = |reason: &str, note: &str| {
        let doc = document(&run(&home, &["status", "--json"]));
        let rows = doc["instances"].as_array().unwrap();
        assert_eq!(rows.len(), ports.len());
        for row in rows {
            assert_eq!(row["instance"]["launch_reason"], reason);
            assert_eq!(row["instance"]["launch_note"], note);
        }
    };
    let crash_all = || {
        for port in &ports {
            let record: Value = serde_json::from_slice(
                &fs::read(run_dir.join(format!("{port}.instance.json"))).unwrap(),
            )
            .unwrap();
            crash(record["pid"].as_u64().unwrap(), *port);
        }
    };
    check("plugin", "handoff by <secret>");
    for (lang, line) in [
        (
            "en",
            "Last start: started by a plugin (handoff by <secret>)",
        ),
        ("zh-CN", "最近一次启动：插件启动（handoff by <secret>）"),
    ] {
        let out = home
            .cmd()
            .env("APROXY_LANG", lang)
            .arg("status")
            .output()
            .report_exit()
            .unwrap();
        assert!(out.status.success());
        assert_eq!(
            String::from_utf8(out.stdout).unwrap().matches(line).count(),
            2
        );
    }
    run(&home, &["restart", "all"]);
    check("restart", "");
    let max_note = "é".repeat(256);
    run(
        &home,
        &["restart", "all", "--reason", "plugin", "--note", &max_note],
    );
    check("plugin", &max_note);
    crash_all();
    run(
        &home,
        &[
            "restore",
            "--reason",
            "restart",
            "--note",
            "after maintenance",
        ],
    );
    check("restart", "after maintenance");
    crash_all();
    run(&home, &["restore"]);
    check("restore", "");
    crash_all();
    run(
        &home,
        &["restore-at-login", "--home", home.path().to_str().unwrap()],
    );
    check("login", "");
}

/// 空 home 也输出完整文档；活动筛选只影响运行中的实例，等待恢复的实例仍可供人和程序查看。
#[test]
fn status_json_document() {
    let home = TestHome::new();
    let args = ["status", "--json"];
    assert_eq!(
        document(&run(&home, &args)),
        json!({
            "schema": 1,
            "instances": [],
            "unreadable": [],
            "watchdog": null,
            "processes": []
        })
    );

    let path = home.path().join("a.toml");
    fs::write(
        &path,
        "base_url = \"https://a.example.com\"\nlisten_addr = \"127.0.0.1:0\"\n",
    )
    .unwrap();
    let start = document(&run(&home, &["start", path.to_str().unwrap(), "--json"]));
    let port = home.track_started_port(&start);
    let pid = start["pid"].as_u64().unwrap();

    let doc = document(&run(&home, &args));
    let instances = doc["instances"].as_array().unwrap();
    assert_eq!(instances.len(), 1);
    let live = &instances[0];
    assert_eq!(live["port"], port);
    assert_eq!(live["liveness"], "live");
    assert_eq!(live["instance"]["pid"], pid);
    assert_eq!(live["instance"]["launch_reason"], "start");
    assert_eq!(live["instance"]["launch_note"], "");
    assert_eq!(live["status"]["state"], "serving");
    assert_eq!(live["restore_pending"], false);
    assert!(live.get("reason").is_none(), "{live}");

    let busy = document(&run(&home, &["status", "--json", "--busy"]));
    assert!(
        busy["instances"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["port"] == port),
        "{busy}"
    );
    let idle = document(&run(&home, &["status", "--json", "--idle"]));
    assert_eq!(idle["instances"], json!([]));

    let plain = run(&home, &["status"]);
    let stdout = String::from_utf8_lossy(&plain.stdout);
    assert!(stdout.contains("Running aProxy instances (1):"), "{stdout}");
    assert!(stdout.contains(&format!("Port {port}")), "{stdout}");

    crash(pid, port);
    fs::remove_file(&path).unwrap();

    let doc = document(&run(&home, &args));
    let down = doc["instances"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["port"] == port)
        .unwrap();
    assert_eq!(down["liveness"], "down");
    assert_eq!(down.get("instance"), Some(&Value::Null));
    assert_eq!(down["restore_pending"], true);
    assert_eq!(down["restore_reason"], "config_missing");

    let plain = run(&home, &["status"]);
    let stdout = String::from_utf8_lossy(&plain.stdout);
    assert!(
        stdout.contains("Instances awaiting restore (1)"),
        "{stdout}"
    );
    assert!(
        stdout.contains(&format!("Port {port} (configuration file not found)")),
        "{stdout}"
    );
    assert!(
        stdout.contains(&format!(
            "Run aproxy stop {port} if you no longer need this instance."
        )),
        "{stdout}"
    );
    assert!(!stdout.contains("No running aProxy instances"), "{stdout}");
}

/// 读不懂的记录带相对路径和原因列出，暂停日志清理的状态也必须对人和程序可见。
#[test]
fn status_lists_unreadable_records() {
    let home = TestHome::new();
    let port = free_port();
    let file = format!("{port}.instance.json");
    let run_dir = home.path().join("run");
    fs::create_dir(&run_dir).unwrap();
    fs::write(run_dir.join(&file), "not json").unwrap();
    let file = format!("run/{file}");

    let doc = document(&run(&home, &["status", "--json"]));
    let unreadable = doc["unreadable"].as_array().unwrap();
    assert_eq!(unreadable.len(), 1);
    assert_eq!(unreadable[0]["file"], file);
    assert_eq!(unreadable[0]["reason"]["code"], "record_unreadable");
    assert_eq!(doc["gc_paused"], json!({"reason": "unreadable_records"}));

    let plain = run(&home, &["status"]);
    let stdout = String::from_utf8_lossy(&plain.stdout);
    assert!(stdout.contains("Unreadable records (1)"), "{stdout}");
    assert!(
        stdout.contains(&format!("{file}  [record_unreadable]")),
        "{stdout}"
    );
    assert!(stdout.contains("Log cleanup is paused"), "{stdout}");
}

/// 别名按名称排序；目标消失只改变 exists，不会抹掉已登记的路径。
#[test]
fn alias_list_json_document() {
    let home = TestHome::new();
    let args = ["alias", "list", "--json"];
    let empty = run(&home, &args);
    assert_eq!(document(&empty), json!({"schema": 1, "aliases": []}));
    assert!(String::from_utf8_lossy(&empty.stderr).contains("No aliases"));

    let work = home.path().join("work.toml");
    let gone = home.path().join("gone.toml");
    for (name, path) in [("work", &work), ("gone", &gone)] {
        fs::write(path, "base_url = \"https://x.example.com\"\n").unwrap();
        run(&home, &["alias", "add", name, path.to_str().unwrap()]);
    }
    fs::remove_file(&gone).unwrap();

    let doc = document(&run(&home, &args));
    assert_keys(&doc, &["schema", "aliases"]);
    let aliases = doc["aliases"].as_array().unwrap();
    assert_eq!(aliases.len(), 2);
    for (item, (name, path, exists)) in aliases
        .iter()
        .zip([("gone", &gone, false), ("work", &work, true)])
    {
        assert_keys(item, &["name", "path", "exists"]);
        assert_eq!(item["name"], name);
        assert_eq!(item["exists"], exists);
        assert_eq!(
            path_match_key(item["path"].as_str().unwrap()),
            path_match_key(path.to_str().unwrap())
        );
    }

    let plain = run(&home, &["alias", "list"]);
    let stdout = String::from_utf8_lossy(&plain.stdout);
    assert!(
        stdout.contains("work") && !stdout.starts_with('{'),
        "{stdout}"
    );
}

/// 配置按路径排序；无效文件仍带诊断列出，但不能命中端口，有效文件只展示打码后的上游。
#[test]
fn find_json_document() {
    let home = TestHome::new();
    let args = ["find", "--json"];
    let empty = run(&home, &args);
    assert_eq!(document(&empty), json!({"schema": 1, "configs": []}));
    assert!(String::from_utf8_lossy(&empty.stderr).contains("No configuration files found"));

    let dir = home.path().join("configs");
    fs::create_dir(&dir).unwrap();
    let good = dir.join("good.toml");
    let bad = dir.join("bad.toml");
    let port = free_port();
    let listen_addr = format!("127.0.0.1:{port}");
    let base_url = "https://user:pw@good.example.com/v1";
    fs::write(
        &good,
        format!("base_url = \"{base_url}\"\nlisten_addr = \"{listen_addr}\"\n"),
    )
    .unwrap();
    fs::write(&bad, "base_url = \"https://bad.example.com\"\nnope = 1\n").unwrap();
    run(&home, &["alias", "add", "good", good.to_str().unwrap()]);

    let doc = document(&run(&home, &args));
    assert_keys(&doc, &["schema", "configs"]);
    let configs = doc["configs"].as_array().unwrap();
    assert_eq!(configs.len(), 2);
    for (item, path) in configs.iter().zip([&bad, &good]) {
        assert_keys(
            item,
            &["path", "aliases", "listen_addr", "base_url", "errors"],
        );
        assert_eq!(
            path_match_key(item["path"].as_str().unwrap()),
            path_match_key(path.to_str().unwrap())
        );
    }
    let bad = &configs[0];
    assert_eq!(bad["aliases"], json!([]));
    assert!(bad["listen_addr"].is_null());
    assert!(bad["base_url"].is_null());
    let error = bad["errors"].as_array().unwrap().first().unwrap();
    assert_eq!(error["code"], "config_unknown_key");
    assert_eq!(error["key"], "nope");

    let good = &configs[1];
    assert_eq!(good["aliases"], json!(["good"]));
    assert_eq!(good["listen_addr"], listen_addr);
    assert_eq!(good["base_url"], aproxy_ipc::redact::url(base_url));
    assert!(!good["base_url"].as_str().unwrap().contains("pw@"));
    assert_eq!(good["errors"], json!([]));

    let filtered = document(&run(
        &home,
        &["find", "--json", "--port", &port.to_string()],
    ));
    assert_eq!(filtered, json!({"schema": 1, "configs": [good]}));
    let plain = run(&home, &["find"]);
    let stdout = String::from_utf8_lossy(&plain.stdout);
    assert!(
        stdout.contains("good.toml") && !stdout.starts_with('{'),
        "{stdout}"
    );
}

/// 生效设置的文档不泄露密钥；能读但无效的对象仍列出诊断，让人可以按 stderr 的提示修复。
#[test]
fn settings_show_json_document() {
    let home = TestHome::new();
    let args = ["settings", "show", "--json"];
    let doc = document(&run(&home, &args));
    assert_keys(&doc, &["schema", "file", "values", "diagnostics"]);
    assert_eq!(
        path_match_key(doc["file"].as_str().unwrap()),
        path_match_key(home.path().join("settings.json").to_str().unwrap())
    );
    assert_eq!(doc["values"]["watchdog"]["enabled"], false);
    assert_eq!(doc["diagnostics"], json!([]));

    run(
        &home,
        &[
            "settings",
            "set",
            "defaults.base_url",
            "https://api.example.com",
        ],
    );
    run(
        &home,
        &[
            "settings",
            "set",
            "defaults.api_key",
            "sk-abcdefghijklmnopqrstuvwxyz0123",
        ],
    );
    let out = run(&home, &args);
    assert_eq!(
        document(&out)["values"]["defaults"]["api_key"],
        json!({"secret": true, "hint": "sk-a…0123"})
    );
    assert!(!String::from_utf8_lossy(&out.stdout).contains("abcdefghijklmnopqrstuvwxyz"));

    home.settings(r#"{"schema":1,"nope":1}"#);
    let (doc, stderr) = failed_document(&home, &args, 3);
    assert!(
        doc["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| { d["code"] == "config_unknown_key" && d["key"] == "nope" })
    );
    assert!(stderr.contains("aproxy settings unset 'nope'"), "{stderr}");
}

/// 未设置的值是 null，不等于路径写错；只有后者是带原因的用法错误。
#[test]
fn settings_get_json_document() {
    let home = TestHome::new();
    for (key, value) in [
        ("watchdog.enabled", json!(false)),
        ("defaults.api_key", Value::Null),
    ] {
        let doc = document(&run(&home, &["settings", "get", key, "--json"]));
        assert_eq!(doc, json!({"schema": 1, "key": key, "value": value}));
    }
    let (doc, _) = failed_document(&home, &["settings", "get", "a..b", "--json"], 2);
    assert_eq!(doc["error"]["code"], "usage");
    assert_eq!(doc["error"]["details"]["reason"], "bad_key_path");
}

/// 写回后报告生效的值；只有改动全局默认才列出实例，文本模式把重启命令留在 stderr。
#[test]
fn settings_set_and_unset_json_documents() {
    let home = TestHome::new();
    let builtin = document(&run(
        &home,
        &["settings", "get", "logs.rotate_mb", "--json"],
    ));
    let args = ["settings", "set", "logs.rotate_mb", "16", "--json"];
    let doc = document(&run(&home, &args));
    assert_keys(
        &doc,
        &[
            "schema",
            "file",
            "key",
            "value",
            "changed",
            "instances_to_restart",
        ],
    );
    assert_eq!(doc["key"], "logs.rotate_mb");
    assert_eq!(doc["value"], 16);
    assert_eq!(doc["changed"], true);
    assert_eq!(doc["instances_to_restart"], json!([]));
    assert_eq!(document(&run(&home, &args))["changed"], false);
    let doc = document(&run(
        &home,
        &["settings", "unset", "logs.rotate_mb", "--json"],
    ));
    assert_eq!(doc["changed"], true);
    assert_eq!(doc["value"], builtin["value"]);

    let path = home.path().join("a.toml");
    fs::write(
        &path,
        "base_url = \"https://a.example.com\"\nlisten_addr = \"127.0.0.1:0\"\n",
    )
    .unwrap();
    let start = document(&run(&home, &["start", path.to_str().unwrap(), "--json"]));
    let port = home.track_started_port(&start);
    for (key, value, ports) in [
        ("defaults.max_request_mb", "64", json!([port])),
        ("watchdog.heartbeat_secs", "60", json!([])),
    ] {
        let doc = document(&run(&home, &["settings", "set", key, value, "--json"]));
        assert_eq!(doc["instances_to_restart"], ports);
    }
    let out = run(&home, &["settings", "set", "defaults.max_request_mb", "32"]);
    assert!(!out.stdout.starts_with(b"{"));
    assert!(String::from_utf8_lossy(&out.stderr).contains(&format!("aproxy restart {port}")));

    let (doc, _) = failed_document(&home, &["settings", "set", "nope", "1", "--json"], 3);
    assert_eq!(doc["error"]["code"], "config_invalid");
    assert_eq!(
        doc["error"]["details"]["diagnostics"][0]["code"],
        "config_unknown_key"
    );
    let (doc, _) = failed_document(
        &home,
        &["settings", "set", "watchdog.enabled", "notjson", "--json"],
        2,
    );
    assert_eq!(doc["error"]["code"], "usage");
    assert_eq!(doc["error"]["details"]["reason"], "bad_value");
}

/// 体检始终列出检查过的文件；冲突告诉每一方另一方是谁，只有错误改变退出码。
#[test]
fn doctor_json_document() {
    let home = TestHome::new();
    let args = ["doctor", "--json"];
    let doc = document(&run(&home, &args));
    assert_keys(&doc, &["schema", "files", "summary"]);
    assert_eq!(doc["summary"], json!({"errors":0,"warnings":0}));
    let files = doc["files"].as_array().unwrap();
    assert_eq!(files.len(), 2);
    assert_eq!(files[0]["kind"], "settings");
    assert_eq!(files[1]["kind"], "home");
    for file in files {
        assert_keys(file, &["file", "kind", "diagnostics"]);
        assert_eq!(file["diagnostics"], json!([]));
    }

    let port = free_port();
    let configs = home.path().join("configs");
    fs::create_dir_all(&configs).unwrap();
    let a = configs.join("a.toml");
    let b = configs.join("b.toml");
    let bad = configs.join("bad.toml");
    let gone = home.path().join("gone.toml");
    for path in [&a, &b] {
        fs::write(
            path,
            format!("base_url = 'https://ok.example.com'\nlisten_addr = '127.0.0.1:{port}'\n"),
        )
        .unwrap();
    }
    fs::write(&bad, "base_url = \"https://bad.example.com\"\nnope = 1\n").unwrap();
    home.settings(
        &json!({
            "schema": 1, "watchdog": {"enabled": false},
            "defaults": {"extra_headers": {"x-team": "blue"}}, "aliases": {"gone": gone}
        })
        .to_string(),
    );

    let (doc, _) = failed_document(&home, &args, 3);
    assert_eq!(doc["summary"]["errors"], 1, "{doc}");
    let files = doc["files"].as_array().unwrap();
    let diag = |path: &std::path::Path, code: &str| {
        let key = path_match_key(path.to_str().unwrap());
        let file = files
            .iter()
            .find(|f| path_match_key(f["file"].as_str().unwrap()) == key)
            .unwrap();
        file["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["code"] == code)
            .unwrap()
    };
    assert_eq!(diag(&bad, "config_unknown_key")["key"], "nope");
    for (path, other) in [(&a, &b), (&b, &a)] {
        let d = diag(path, "port_shared");
        assert_eq!(d["details"]["port"], port);
        let others = d["details"]["files"].as_array().unwrap();
        assert_eq!(others.len(), 1);
        assert_eq!(
            path_match_key(others[0].as_str().unwrap()),
            path_match_key(other.to_str().unwrap())
        );
    }
    assert_eq!(
        diag(&gone, "config_file_unreadable")["details"]["reason"],
        "not_found"
    );
    let header = diag(&home.path().join("settings.json"), "default_header_shared");
    assert_eq!(header["severity"], "info");
    assert_eq!(header["key"], "defaults.extra_headers.x-team");
    let warnings = files
        .iter()
        .flat_map(|f| f["diagnostics"].as_array().unwrap())
        .filter(|d| d["severity"] == "warning")
        .count();
    assert_eq!(doc["summary"]["warnings"], warnings);
    let plain = home.cmd().arg("doctor").output().report_exit().unwrap();
    assert_eq!(plain.status.code(), Some(3));
    let stdout = String::from_utf8(plain.stdout).unwrap();
    for fragment in [
        "[ERROR] ",
        "Remove this key: aproxy config --config '",
        "Results: 1 error,",
        "Instances cannot start with configuration errors",
    ] {
        assert!(stdout.contains(fragment), "{stdout}");
    }
}

/// 无法读取的记录保留在原处并暂停日志清理，但不是配置错误。
#[test]
fn doctor_reports_unreadable_records() {
    let home = TestHome::new();
    let file = format!("run/{}.instance.json", free_port());
    fs::create_dir(home.path().join("run")).unwrap();
    let path = home.path().join(&file);
    fs::write(&path, "not json").unwrap();
    let doc = document(&run(&home, &["doctor", "--json"]));
    let record = doc["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["kind"] == "record" && f["file"] == file)
        .unwrap();
    let diagnostics = record["diagnostics"].as_array().unwrap();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0]["code"], "record_unreadable");
    assert_eq!(diagnostics[0]["severity"], "warning");
    assert_eq!(diagnostics[0]["file"], file);
    assert_eq!(doc["gc_paused"], json!({"reason":"unreadable_records"}));
    assert!(
        String::from_utf8(run(&home, &["doctor"]).stdout)
            .unwrap()
            .contains("Log cleanup is paused")
    );
    assert_eq!(fs::read_to_string(path).unwrap(), "not json");
}

/// 同一进程在 stdout 写英文错误文档，同时在 stderr 向调用者说明同一条错误。
#[test]
fn machine_errors_do_not_inherit_the_terminal_language() {
    let home = TestHome::new();
    for (args, en, zh) in [
        (
            vec!["config", "get", "bogus", "--json"],
            "Unknown key: bogus",
            "不认识的键: bogus",
        ),
        (
            vec!["settings", "set", "watchdog.enabled", "not-json", "--json"],
            "Write the value of watchdog.enabled as JSON",
            "watchdog.enabled 的值要写成 JSON",
        ),
    ] {
        let call = |lang| {
            home.cmd()
                .env("APROXY_LANG", lang)
                .args(&args)
                .output()
                .report_exit()
                .unwrap()
        };
        let english = call("en");
        let chinese = call("zh-CN");
        assert_eq!(english.status.code(), chinese.status.code());
        assert!(!english.status.success());
        assert_eq!(english.stdout, chinese.stdout);
        let doc: serde_json::Value = serde_json::from_slice(&chinese.stdout).unwrap();
        assert!(
            doc["error"]["message"].as_str().unwrap().starts_with(en),
            "{doc}"
        );
        assert!(String::from_utf8_lossy(&english.stderr).contains(en));
        assert!(String::from_utf8_lossy(&chinese.stderr).contains(zh));
    }
}

/// 只切换子进程的环境；别名、发现和体检的普通输出均不能继承测试机的语言。
#[test]
fn local_reports_follow_the_reader() {
    let home = TestHome::new();
    for (args, en, zh) in [
        (vec!["alias", "list"], "No aliases", "暂无别名"),
        (
            vec!["find"],
            "No configuration files found",
            "没有找到配置文件",
        ),
        (
            vec!["doctor"],
            "aProxy configuration check",
            "aProxy 配置体检",
        ),
    ] {
        for (lang, expected) in [("en", en), ("zh-CN", zh)] {
            let out = home
                .cmd()
                .env("APROXY_LANG", lang)
                .args(&args)
                .output()
                .report_exit()
                .unwrap();
            let stdout = String::from_utf8(out.stdout).unwrap();
            assert!(
                out.status.success() && stdout.contains(expected),
                "{args:?}: {stdout}"
            );
        }
    }
}

/// 同一个类型错误经过设置编辑、配置解析与体检；有结构的原因只供终端排版，
/// 对外仍是同一条英文句子，既不多出私有字段，也不受终端语言影响。
#[test]
fn nested_diagnostics_localize_without_changing_public_documents() {
    let home = TestHome::new();
    fs::write(home.path().join("config.toml"), "base_url = true\n").unwrap();
    let call = |lang, args: &[&str]| {
        home.cmd()
            .env("APROXY_LANG", lang)
            .args(args)
            .output()
            .report_exit()
            .unwrap()
    };
    for (args, plain, key, en, zh) in [
        (
            vec!["settings", "set", "watchdog.enabled", "42", "--json"],
            vec!["settings", "set", "watchdog.enabled", "42"],
            "watchdog.enabled",
            "watchdog.enabled must be true or false",
            "watchdog.enabled 必须为 true 或 false",
        ),
        (
            vec!["doctor", "--json"],
            vec!["doctor"],
            "base_url",
            "base_url must be a string",
            "base_url 必须为字符串",
        ),
    ] {
        let a = call("en", &args);
        let b = call("zh-CN", &args);
        assert_eq!(a.status.code(), Some(3));
        assert_eq!(b.status.code(), Some(3));
        assert_eq!(a.stdout, b.stdout);
        let doc = document(&b);
        let items = match args[0] {
            "settings" => doc["error"]["details"]["diagnostics"].as_array().unwrap(),
            _ => doc["files"]
                .as_array()
                .unwrap()
                .iter()
                .find(|f| f["kind"] == "config")
                .unwrap()["diagnostics"]
                .as_array()
                .unwrap(),
        };
        let d = items
            .iter()
            .find(|d| d["code"] == "config_type" && d["key"] == key)
            .unwrap();
        assert_eq!(d["message"], en);
        assert!(d.get("render").is_none());
        for (lang, want, other) in [("en", en, zh), ("zh-CN", zh, en)] {
            let out = call(lang, &plain);
            assert_eq!(out.status.code(), Some(3));
            let human = String::from_utf8(if args[0] == "doctor" {
                out.stdout
            } else {
                out.stderr
            })
            .unwrap();
            assert!(human.contains(want) && !human.contains(other), "{human}");
        }
    }
}

/// 日志可以由旧版本或外部程序留下：只翻译阅读器的说明，不重写记录的任何字节。
#[test]
fn log_replay_is_independent_of_notice_language() {
    let home = TestHome::new();
    let log = home.path().join("previous.log");
    let content = "older log record\n旧日志原文\n";
    fs::write(&log, content).unwrap();
    let port = free_port().to_string();
    let run = home.path().join("run");
    fs::create_dir_all(&run).unwrap();
    let spec = aproxy::spec::InstanceSpec {
        schema: aproxy::spec::SCHEMA,
        config_path: None,
        overrides: Default::default(),
        pinned_port: None,
        env: Default::default(),
        log_path: log.display().to_string(),
    };
    aproxy::spec::write_restore_in(&run, &port, &spec).unwrap();
    for (lang, notice, pending) in [
        ("en", "Log file:", "Instances awaiting restore (1)"),
        ("zh-CN", "日志文件:", "等待恢复的实例 (1)"),
    ] {
        let out = home
            .cmd()
            .env("APROXY_LANG", lang)
            .arg("logs")
            .output()
            .report_exit()
            .unwrap();
        assert!(out.status.success());
        assert_eq!(out.stdout, content.as_bytes());
        assert!(String::from_utf8(out.stderr).unwrap().contains(notice));
        let status = home
            .cmd()
            .env("APROXY_LANG", lang)
            .arg("status")
            .output()
            .report_exit()
            .unwrap();
        assert!(status.status.success());
        assert!(String::from_utf8(status.stdout).unwrap().contains(pending));
    }
}

/// 同一空目录在终端按读者语言解释启动失败和恢复结果，机器文档不取终端说明。
#[test]
fn start_and_restore_choose_language_only_for_the_terminal() {
    let home = TestHome::new();
    let run = |language, args: &[&str]| {
        home.cmd()
            .env("APROXY_LANG", language)
            .args(args)
            .output()
            .report_exit()
            .unwrap()
    };
    let target = "absent-alias";
    for (lang, missing, none) in [
        (
            "en",
            "Unknown alias or configuration file",
            "No instances to restore.",
        ),
        ("zh-CN", "未知的别名或配置文件", "没有需要恢复的实例。"),
    ] {
        let f = run(lang, &["start", target]);
        assert_eq!(f.status.code(), Some(4));
        assert!(f.stdout.is_empty() && String::from_utf8(f.stderr).unwrap().contains(missing));
        let r = run(lang, &["restore"]);
        assert!(r.status.success() && String::from_utf8(r.stdout).unwrap().contains(none));
    }
    let args = ["start", target, "--json"];
    let en = run("en", &args);
    let zh = run("zh-CN", &args);
    assert_eq!(en.status.code(), Some(4));
    assert_eq!(en.status.code(), zh.status.code());
    assert_eq!(en.stdout, zh.stdout);
    assert_eq!(document(&zh)["error"]["details"]["target"], target);
}

/// 没有目标时 stop 与 restart 的语义不同；失败的机器文档不受终端语言影响。
#[test]
fn stop_and_restart_render_their_own_terminal_notices() {
    let home = TestHome::new();
    let run = |lang, args: &[&str]| {
        home.cmd()
            .env("APROXY_LANG", lang)
            .args(args)
            .output()
            .report_exit()
            .unwrap()
    };
    for (lang, none, restart) in [
        (
            "en",
            "No running aProxy instances.",
            "Restart only restarts running instances.",
        ),
        (
            "zh-CN",
            "没有运行中的 aProxy 实例。",
            "restart 只重启运行中的实例",
        ),
    ] {
        let stop = run(lang, &["stop"]);
        assert!(stop.status.success() && String::from_utf8(stop.stdout).unwrap().contains(none));
        let fail = run(lang, &["restart"]);
        assert_eq!(fail.status.code(), Some(4));
        let err = String::from_utf8(fail.stderr).unwrap();
        assert!(err.contains(none) && err.contains(restart), "{err}");
    }
    let en = run("en", &["restart", "--json"]);
    let zh = run("zh-CN", &["restart", "--json"]);
    assert_eq!(en.stdout, zh.stdout);
    assert_eq!(document(&zh)["error"]["code"], "target_not_running");
}

/// format-only 命令与实例重启共享汇总外壳，但逐条目计数；改坏一个角色文件不影响其余条目。
#[test]
fn restart_formats_json_document() {
    let home = TestHome::new();
    let cfg = home.path().join("formats.toml");
    let role = home.path().join("version");
    fs::write(&role, "2").unwrap();
    fs::write(&cfg,format!("base_url='http://127.0.0.1'\nlisten_addr='127.0.0.1:0'\n[[format]]\nname='a'\ncommand={0}\nstages=['request']\n[[format]]\nname='b'\ncommand={0}\nargs=['role-version',{1}]\nstages=['request']",json!(common::format_echo_path()),json!(role))).unwrap();
    let start = document(&run(&home, &["start", cfg.to_str().unwrap(), "--json"]));
    let port = home.track_started_port(&start);
    let addr = port.to_string();
    let args = ["restart", &addr, "--format", "b", "--json"];
    let one = run(&home, &args);
    let item = json!({"target":addr,"port":port,"name":"b","outcome":"restarted","program":"format-echo","version":"2","configReloaded":false});
    assert_eq!(
        document(&one),
        json!({"schema":1,"results":[item],"summary":{"ok":1,"failed":0}})
    );
    let text = String::from_utf8_lossy(&one.stderr);
    assert!(text.contains(&format!("Port {port}: restarted format b (format-echo 2).")));
    assert!(text.contains("config.toml was not reloaded"));
    fs::remove_file(role).unwrap();
    let (doc, text) = failed_document(&home, &["restart", &addr, "--format", "all", "--json"], 5);
    assert_eq!(doc["summary"], json!({"ok":1,"failed":1}));
    assert_eq!(
        doc["results"][1],
        json!({"target":addr,"port":port,"name":"b","outcome":"handshake-failed","reason":"invalid_output","configReloaded":false})
    );
    assert!(text.contains(&format!(
        "Port {port}: format b failed its handshake: invalid_output; nothing was replaced."
    )));
    let (doc, text) = failed_document(
        &home,
        &["restart", &addr, "--format", "missing", "--json"],
        1,
    );
    assert_eq!(
        doc["results"][0]["error"]["details"]["reason"],
        "unknown_format"
    );
    assert!(text.contains("no configured format named missing"));
    for flags in [
        &["--format", "a"][..],
        &["--force"],
        &["--idle-threshold-secs", "1"],
    ] {
        let mut args = vec!["restart", "idle", "--format", "all", "--json"];
        args.extend(flags);
        let (doc, _) = failed_document(&home, &args, 2);
        assert_eq!(doc["schema"], 1);
        assert_eq!(doc["error"]["code"], "usage");
        assert_eq!(doc["error"]["details"]["reason"], "format_selection");
    }
}

/// 别名、显式默认与目录扫描可以撞上同一目标；备份、报告和重跑都以实际文件为单位。
#[test]
fn migrate_preview_apply_and_repeat_in_a_mixed_home() {
    let h = TestHome::new();
    let home = h.path();
    let unscanned = home.join("aliases-only");
    let configs = home.join("extra-dir");
    fs::create_dir(&unscanned).unwrap();
    fs::create_dir(&configs).unwrap();
    let alias = unscanned.join("legacy.cfg");
    let extra = configs.join("other.toml");
    let current = home.join("current.toml");
    let manual = home.join("manual.toml");
    let sp = home.join("settings.json");
    let settings=json!({"aliases":{"demo":alias},"default_config":alias,"config_dirs":[configs],"forward_only":false,"watchdog":false,"download_proxy":"http://username:private-value@proxy.example","download_chain":[{"url":"https://mirror.example","ignored":[{"extra_headers":{"Authorization":"nested-secret"}}]}]}).to_string();
    h.settings(&settings);
    let orig = "base_url = 'http://127.0.0.1'\nlisten_addr = '127.0.0.1:0'\n# keep the cap\nmax_body_mb = 17 # important\nread_timeout_secs = 44\n";
    let other = "base_url='http://127.0.0.1'\nbounded_retry_paths=['/count']\n";
    let unchanged = "base_url = 'http://127.0.0.1' # leave alone\n";
    let transform = "max_body_mb=3\n[request_transform]\ncommand='tool'\n";
    for (p, s) in [
        (&alias, orig),
        (&extra, other),
        (&current, unchanged),
        (&manual, transform),
    ] {
        fs::write(p, s).unwrap();
    }
    let all = [
        (&sp, settings.as_str()),
        (&alias, orig),
        (&extra, other),
        (&current, unchanged),
        (&manual, transform),
    ];
    let backup = |p: &std::path::Path| {
        let mut n = p.as_os_str().to_owned();
        n.push(".0.1.bak");
        std::path::PathBuf::from(n)
    };
    let check = |dry| {
        let args = if dry {
            vec!["migrate", "--json", "--dry-run"]
        } else {
            vec!["migrate", "--json"]
        };
        let (doc, stderr) = failed_document(&h, &args, 5);
        assert!(stderr.is_empty(), "{stderr}");
        assert!(!doc.to_string().contains("private-value"));
        assert!(!doc.to_string().contains("nested-secret"));
        assert_keys(&doc, &["schema", "files", "summary"]);
        assert_eq!(doc["summary"]["dry_run"], dry);
        assert_eq!(doc["summary"]["migrated"], 3);
        assert_eq!(doc["summary"]["current"], 1);
        assert_eq!(doc["summary"]["needs_review"], 1);
        let files = doc["files"].as_array().unwrap();
        assert_eq!(files.len(), all.len());
        for (p, _) in all {
            let actual = fs::canonicalize(p).unwrap();
            let row = files
                .iter()
                .find(|r| fs::canonicalize(r["path"].as_str().unwrap()).unwrap() == actual)
                .unwrap();
            assert_keys(row, &["path", "outcome", "changes", "issues"]);
            if *p == manual {
                assert_eq!(row["outcome"], "needs-review");
                assert_eq!(row["issues"][0]["key"], "request_transform");
                assert_eq!(row["issues"][0]["line"], 2);
            } else if *p == current {
                assert_eq!(row["outcome"], "current");
            } else {
                assert_eq!(row["outcome"], "migrated");
                assert!(!row["changes"].as_array().unwrap().is_empty());
            }
        }
        doc["files"].clone()
    };
    let text = h.cmd().args(["migrate", "--dry-run"]).output().unwrap();
    assert_eq!(text.status.code(), Some(5));
    for stream in [&text.stdout, &text.stderr] {
        assert!(!String::from_utf8_lossy(stream).contains("nested-secret"));
    }
    let preview = check(true);
    assert!(!home.join("run").exists());
    for (p, s) in all {
        assert_eq!(fs::read(p).unwrap(), s.as_bytes());
        assert!(!backup(p).exists());
    }
    assert_eq!(check(false), preview);
    for (p, s) in all {
        if *p == current || *p == manual {
            assert_eq!(fs::read(p).unwrap(), s.as_bytes());
            assert!(!backup(p).exists());
        } else {
            assert_eq!(fs::read(backup(p)).unwrap(), s.as_bytes());
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                assert_eq!(
                    fs::metadata(backup(p)).unwrap().permissions().mode() & 0o777,
                    0o600
                );
            }
            #[cfg(windows)]
            {
                assert!(
                    aproxy::fsx::dacl_sddl(&backup(p))
                        .unwrap()
                        .starts_with("D:P")
                );
            }
        }
    }
    let toml = fs::read_to_string(&alias).unwrap();
    assert!(toml.contains("# keep the cap") && toml.contains("# important"));
    assert!(!toml.contains("read_timeout_secs"));
    assert!(
        fs::read_to_string(&extra)
            .unwrap()
            .contains("[[retry.rules]]")
    );
    let files = all.map(|(p, _)| fs::read(p).unwrap());
    let started = document(&run(&h, &["start", "demo", "--json"]));
    let port = h.track_started_port(&started);
    run(&h, &["stop", &port.to_string()]);
    let (second, _) = failed_document(&h, &["migrate", "--json"], 3);
    assert_eq!(second["summary"]["current"], 4);
    assert_eq!(second["summary"]["migrated"], 0);
    for ((p, s), bytes) in all.into_iter().zip(files) {
        assert_eq!(fs::read(p).unwrap(), bytes);
        if *p != current && *p != manual {
            assert_eq!(fs::read(backup(p)).unwrap(), s.as_bytes());
        }
    }
}

/// 已有不同备份时预演和实际运行都应拒绝；相同备份可用来恢复中断的写入。
#[test]
fn migrate_never_overwrites_a_different_backup() {
    let h = TestHome::new();
    let backup = h.path().join("settings.json.0.1.bak");
    let old = r#"{"watchdog":false}"#;
    h.settings(old);
    fs::write(&backup, "original").unwrap();
    for args in [
        &["migrate", "--dry-run", "--json"][..],
        &["migrate", "--json"],
    ] {
        let (doc, _) = failed_document(&h, args, 3);
        assert!(
            doc["files"][0]["issues"][0]["message"]
                .as_str()
                .unwrap()
                .contains(".0.1.bak")
        );
        assert_eq!(fs::read(&backup).unwrap(), b"original");
        assert_eq!(
            fs::read(h.path().join("settings.json")).unwrap(),
            old.as_bytes()
        );
    }
    fs::write(&backup, old).unwrap();
    #[cfg(windows)]
    {
        // 本机临时目录可能本来就私有；显式开放给 Everyone，避免测试在旧代码上也通过。
        assert!(
            h.cmd_with(
                std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap())
                    .join("System32")
                    .join("icacls.exe")
            )
            .arg(&backup)
            .args(["/grant", "*S-1-1-0:(R)"])
            .status()
            .unwrap()
            .success()
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&backup, fs::Permissions::from_mode(0o644)).unwrap();
    }
    assert_ne!(
        aproxy::fsx::access_of(&backup).unwrap(),
        aproxy::fsx::Access::Private
    );
    run(&h, &["migrate", "--dry-run", "--json"]);
    assert_ne!(
        aproxy::fsx::access_of(&backup).unwrap(),
        aproxy::fsx::Access::Private
    );
    assert_eq!(
        document(&run(&h, &["migrate", "--json"]))["summary"]["migrated"],
        1
    );
    assert_eq!(
        aproxy::fsx::access_of(&backup).unwrap(),
        aproxy::fsx::Access::Private
    );
    let missing = TestHome::unborn();
    let doc = document(&run(&missing, &["migrate", "--dry-run", "--json"]));
    assert_eq!(doc["files"], json!([]));
    assert!(!missing.path().exists());
    // 命令行点名了一个不存在的文件，不是 settings 里需要人工清理的登记项。
    let missing = h.path().join("gone.cfg");
    let (doc, _) = failed_document(&h, &["migrate", "--json", missing.to_str().unwrap()], 1);
    assert_eq!(doc["summary"]["failed"], 1);
}

/// 不登记目录或别名，体检能发现的默认 configs/ 里的旧文件也应被无参数迁移处理。
#[test]
fn migrate_inventory_includes_the_builtin_configs_directory() {
    let h = TestHome::new();
    let dir = h.path().join("configs");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("client.toml");
    let backup = dir.join("client.toml.0.1.bak");
    let old = "base_url='http://localhost'\nmax_body_mb=7\n";
    fs::write(&path, old).unwrap();
    for args in [
        &["migrate", "--dry-run", "--json"][..],
        &["migrate", "--json"],
    ] {
        let doc = document(&run(&h, args));
        assert_eq!(doc["summary"]["migrated"], 1);
        if args.contains(&"--dry-run") {
            assert_eq!(fs::read_to_string(&path).unwrap(), old);
            assert!(!backup.exists());
        } else {
            assert_eq!(fs::read_to_string(&backup).unwrap(), old);
            assert!(
                fs::read_to_string(&path)
                    .unwrap()
                    .contains("max_request_mb")
            );
        }
    }
}

/// 丢失的登记项需要使用者取舍，但不应妨碍本次迁移其他文件。仅真正的读取错误优先退出 1。
#[test]
fn migrate_inventory_reports_stale_references_without_blocking_other_files() {
    let h = TestHome::new();
    let home = h.path();
    let alias = home.join("missing-alias.cfg");
    let default = home.join("missing-default.cfg");
    let dir = home.join("missing-dir");
    let settings =
        json!({"aliases":{"old":alias},"default_config":default,"config_dirs":[dir]}).to_string();
    h.settings(&settings);
    let good = home.join("good.toml");
    let old = "base_url='http://localhost'\nmax_body_mb=9\n";
    fs::write(&good, old).unwrap();
    let target_msg = "This alias or default_config points to a file that does not exist; remove the alias or restore the file.";
    let dir_msg =
        "This config_dirs entry does not exist; remove it from settings.json or create it.";
    for (args, code, migrated) in [
        (&["migrate", "--dry-run", "--json"][..], 5, 1),
        (&["migrate", "--json"], 5, 1),
        (&["migrate", "--json"], 3, 0),
    ] {
        let (doc, _) = failed_document(&h, args, code);
        assert_eq!(doc["summary"]["migrated"], migrated);
        assert_eq!(doc["summary"]["needs_review"], 3);
        assert_eq!(doc["summary"]["failed"], 0);
        for (missing, msg) in [
            (&alias, target_msg),
            (&default, target_msg),
            (&dir, dir_msg),
        ] {
            let row = doc["files"]
                .as_array()
                .unwrap()
                .iter()
                .find(|f| {
                    std::path::Path::new(f["path"].as_str().unwrap()).file_name()
                        == missing.file_name()
                })
                .unwrap();
            assert_eq!(row["outcome"], "needs-review");
            assert_eq!(row["issues"][0]["message"], msg);
            assert_eq!(
                row["issues"][0]["section"],
                "compatibility/migrate.md#converting-the-files-with-aproxy-migrate"
            );
            assert!(!missing.exists());
        }
        assert_eq!(
            fs::read_to_string(home.join("settings.json")).unwrap(),
            settings
        );
        if args.contains(&"--dry-run") {
            assert_eq!(fs::read_to_string(&good).unwrap(), old);
        } else {
            assert!(
                fs::read_to_string(&good)
                    .unwrap()
                    .contains("max_request_mb")
            );
            assert_eq!(
                fs::read_to_string(home.join("good.toml.0.1.bak")).unwrap(),
                old
            );
        }
    }
    fs::write(&alias, [0xff]).unwrap();
    let (doc, _) = failed_document(&h, &["migrate", "--json"], 1);
    assert_eq!(doc["summary"]["failed"], 1);
    assert_eq!(doc["summary"]["needs_review"], 2);
}

/// 同一份严格解析既给体检定专用码，也决定何时能建议迁移而不是手工修复。
#[test]
fn migrate_suggestions_are_specific_to_legacy_files() {
    let h = TestHome::new();
    let c = h.path().join("sample.toml");
    for (settings, toml, legacy) in [
        (r#"{"watchdog":false}"#, "base_url='http://localhost'", true),
        (
            r#"{"watchdog":"wrong"}"#,
            "base_url='http://localhost'",
            false,
        ),
        ("{}", "max_body_mb=1", true),
        ("{}", "max_body_mb=1\nunrelated=2", false),
        ("{}", "'format[0].mode'='spawn'", false),
    ] {
        h.settings(settings);
        fs::write(&c, toml).unwrap();
        let (_, stderr) = failed_document(&h, &["start", c.to_str().unwrap(), "--json"], 3);
        assert_eq!(
            stderr
                .trim_end()
                .lines()
                .last()
                .unwrap()
                .contains("aproxy migrate --dry-run"),
            legacy,
            "{stderr}"
        );
        let (doc, _) = failed_document(&h, &["doctor", "--json"], 3);
        assert_eq!(doc.to_string().contains("config_legacy_shape"), legacy);
        assert_eq!(doc.to_string().contains("aproxy migrate --dry-run"), legacy);
        assert_eq!(fs::read(&c).unwrap(), toml.as_bytes());
        assert_eq!(
            fs::read(h.path().join("settings.json")).unwrap(),
            settings.as_bytes()
        );
    }
}

/// 有路径时只处理指定配置；没有路径时，损坏的设置仍能提供部分清单，但不能用于启动。
#[test]
fn migrate_limits_explicit_targets_and_discovers_readable_directory_entries() {
    let h = TestHome::new();
    let outside = TestHome::new();
    let config = outside.path().join("one file.toml");
    let other = h.path().join("ignored.toml");
    let old = "base_url='http://127.0.0.1'\nmax_body_mb=3\n";
    fs::write(&config, old).unwrap();
    fs::write(&other, old).unwrap();
    let sp = h.path().join("settings.json");
    h.settings("{}");
    let path = config.to_str().unwrap();
    for args in [
        vec!["start", path, "--json"],
        vec!["config", "--config", path, "show", "--json"],
    ] {
        let (doc, stderr) = failed_document(&h, &args, 3);
        let hint = aproxy::migrate::path_hint(&config).render(aproxy_ipc::i18n::Lang::En);
        assert!(hint.contains(path));
        assert!(stderr.trim_end().ends_with(&hint), "{stderr}");
        let ds = doc
            .pointer("/error/details/diagnostics")
            .or_else(|| doc.get("diagnostics"))
            .unwrap()
            .as_array()
            .unwrap();
        assert!(
            ds.iter()
                .any(|d| d["code"] == "config_legacy_shape" && d["message"] == hint)
        );
    }
    let second = outside.path().join(".").join("one file.toml");
    let args = [path, second.to_str().unwrap(), path];
    for dry in [true, false] {
        let mut cmd = vec!["migrate", "--json"];
        if dry {
            cmd.push("--dry-run");
        }
        cmd.extend(args);
        let doc = document(&run(&h, &cmd));
        assert_eq!(doc["files"].as_array().unwrap().len(), 1);
        assert_eq!(doc["files"][0]["outcome"], "migrated");
        assert_eq!(fs::read(&sp).unwrap(), b"{}");
        assert_eq!(fs::read_to_string(&other).unwrap(), old);
        assert_eq!(fs::read_to_string(&config).unwrap() == old, dry);
    }
    h.settings(&json!({"config_dirs":[outside.path(),7]}).to_string());
    let (doc, _) = failed_document(&h, &["migrate", "--dry-run", "--json"], 3);
    assert!(
        doc["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["outcome"] == "current"
                && fs::canonicalize(f["path"].as_str().unwrap()).unwrap()
                    == fs::canonicalize(&config).unwrap())
    );
    let reserved = outside.path().join("other.0.1.BaK");
    fs::write(&reserved, old).unwrap();
    let (doc, _) = failed_document(&h, &["migrate", "--json", reserved.to_str().unwrap()], 3);
    assert!(
        doc["files"][0]["issues"][0]["message"]
            .as_str()
            .unwrap()
            .contains("migration backup")
    );
    assert_eq!(fs::read_to_string(reserved).unwrap(), old);
}

#[cfg(unix)]
#[test]
fn migrate_keeps_linked_files_together() {
    use std::os::unix::fs::MetadataExt;
    let h = TestHome::new();
    let path = h.path().join("a.toml");
    let link = h.path().join("alias");
    let old = "base_url='http://localhost'\nforward_only=true";
    fs::write(&path, old).unwrap();
    fs::hard_link(&path, &link).unwrap();
    let actual = fs::canonicalize(&path).unwrap();
    for flags in [
        vec!["migrate", "--dry-run", "--json"],
        vec!["migrate", "--json"],
    ] {
        let (doc, _) = failed_document(&h, &flags, 3);
        let row = doc["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| fs::canonicalize(f["path"].as_str().unwrap()).unwrap() == actual)
            .expect("missing a.toml in migration report");
        assert_eq!(row["outcome"], "needs-review");
        assert_eq!(
            row["issues"][0]["message"],
            "This file has other hard links; migrating it would separate them."
        );
        assert_eq!(
            fs::metadata(&path).unwrap().ino(),
            fs::metadata(&link).unwrap().ino()
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), old);
        assert!(!h.path().join("a.toml.0.1.bak").exists());
    }
}
