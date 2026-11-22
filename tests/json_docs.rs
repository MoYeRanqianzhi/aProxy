//! 按设计稿 §9.4 把各命令的 `--json` 输出作为整份文档钉住：agent 和脚本会解析它们，
//! 文档形状只要变了就是破坏性变更。

mod common;
use common::{TestHome, free_port};

use std::fs;
use std::process::{Command, Output, Stdio};
use std::time::Duration;

use aproxy::settings::path_match_key;
use serde_json::{Value, json};

/// 在隔离的 home 里运行命令，核对退出码为 0，并保留两个输出流供分别检查。
fn run(home: &TestHome, args: &[&str]) -> Output {
    let out = home.cmd().args(args).stdin(Stdio::null()).output().unwrap();
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
    let out = home.cmd().args(args).stdin(Stdio::null()).output().unwrap();
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
        .status();
    #[cfg(unix)]
    let status = Command::new("kill").args(["-9", &pid.to_string()]).status();
    assert!(status.unwrap().success(), "结束 pid {pid} 失败");
    for _ in 0..100 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("pid {pid} 被结束之后，端口 {port} 仍在接受连接");
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

    let port = free_port();
    let path = home.path().join("a.toml");
    fs::write(
        &path,
        format!("base_url = \"https://a.example.com\"\nlisten_addr = \"127.0.0.1:{port}\"\n"),
    )
    .unwrap();
    home.stop_on_drop(port);
    let start = document(&run(&home, &["start", path.to_str().unwrap(), "--json"]));
    let pid = start["pid"].as_u64().unwrap();

    let doc = document(&run(&home, &args));
    let instances = doc["instances"].as_array().unwrap();
    assert_eq!(instances.len(), 1);
    let live = &instances[0];
    assert_eq!(live["port"], port);
    assert_eq!(live["liveness"], "live");
    assert_eq!(live["instance"]["pid"], pid);
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
    assert!(stdout.contains("运行中的 aProxy 实例 (1):"), "{stdout}");
    assert!(stdout.contains(&format!("端口 {port}")), "{stdout}");

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
    assert!(stdout.contains("等待恢复的实例 (1)"), "{stdout}");
    assert!(
        stdout.contains(&format!("端口 {port}（配置文件找不到）")),
        "{stdout}"
    );
    assert!(
        stdout.contains(&format!("不再需要它时执行 aproxy stop {port}")),
        "{stdout}"
    );
    assert!(!stdout.contains("没有运行中的 aProxy 实例"), "{stdout}");
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
    assert!(stdout.contains("读不懂的记录 (1)"), "{stdout}");
    assert!(
        stdout.contains(&format!("{file}  [record_unreadable]")),
        "{stdout}"
    );
    assert!(stdout.contains("日志清理已暂停"), "{stdout}");
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

    let port = free_port();
    let path = home.path().join("a.toml");
    fs::write(
        &path,
        format!("base_url = \"https://a.example.com\"\nlisten_addr = \"127.0.0.1:{port}\"\n"),
    )
    .unwrap();
    home.stop_on_drop(port);
    run(&home, &["start", path.to_str().unwrap(), "--json"]);
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
    let plain = home.cmd().arg("doctor").output().unwrap();
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
                .unwrap();
            let stdout = String::from_utf8(out.stdout).unwrap();
            assert!(
                out.status.success() && stdout.contains(expected),
                "{args:?}: {stdout}"
            );
        }
    }
}
