//! `aproxy config show|get|set|unset`（设计稿 §9.3、§9.4）：值的写法、`--json` 的文档、
//! 密钥的占位与写回、退出码。就地编辑保留注释这类写回本身的性质在 proxy_integration 里。

mod common;
use common::ReportExit as _;
use common::TestHome;

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};

use serde_json::{Value, json};

const SECRET: &str = "sk-CONFIG-CLI-0123456789abcdef";

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// `aproxy config --config <file> <args…>`，stdin 为空
fn config(home: &TestHome, file: &Path, args: &[&str]) -> Output {
    home.cmd()
        .arg("config")
        .arg("--config")
        .arg(file)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .report_exit()
        .unwrap()
}

/// 同 [`config`]，要求成功，返回 stdout 上的那个 JSON 文档
fn doc(home: &TestHome, file: &Path, args: &[&str]) -> Value {
    let out = config(home, file, args);
    assert!(out.status.success(), "{args:?}: {}", text(&out.stderr));
    document(&out, args)
}

/// stdout 上恰好是一行 JSON，`schema` 为 1
fn document(out: &Output, args: &[&str]) -> Value {
    let stdout = text(&out.stdout);
    assert_eq!(stdout.lines().count(), 1, "{args:?}: {stdout}");
    let doc: Value =
        serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("{args:?}: {e}: {stdout}"));
    assert_eq!(doc["schema"], 1, "{args:?}: {doc}");
    doc
}

/// 写一份有效的配置，返回它的路径
fn write_config(home: &TestHome, name: &str, extra: &str) -> PathBuf {
    let path = home.path().join(name);
    std::fs::write(
        &path,
        format!("base_url = \"https://api.example.com\"\n{extra}"),
    )
    .unwrap();
    path
}

/// 值的写法：字符串类型的键收原文，以引号开头时按字面量解析；其余的键只收 TOML 值。
#[test]
fn text_keys_take_raw_text_and_other_keys_take_toml() {
    let home = TestHome::new();
    let file = write_config(&home, "values.toml", "");
    for (key, value) in [
        // 看着像数字、像布尔的原文，在字符串类型的键上就是字符串
        ("api_key", "12345"),
        ("extra_headers.x-flag", "true"),
        ("extra_headers.\"x.dotted\"", "a b"),
        ("keepalive.heartbeat", r#"": ping\n\n""#),
        ("max_request_mb", "64"),
        ("disk_cache", "false"),
        ("allowed_hosts", r#"["a.example", "b.example"]"#),
        ("retry.rules", "[{ status = [500], max_attempts = 3 }]"),
    ] {
        let out = config(&home, &file, &["set", key, value]);
        assert!(out.status.success(), "{key}: {}", text(&out.stderr));
    }
    let saved = std::fs::read_to_string(&file).unwrap();
    for line in [
        "api_key = \"12345\"",
        "x-flag = \"true\"",
        "\"x.dotted\" = \"a b\"",
        "heartbeat = \": ping\\n\\n\"",
        "max_request_mb = 64",
        "disk_cache = false",
        "allowed_hosts = [\"a.example\", \"b.example\"]",
        "[[retry.rules]]",
        "max_attempts = 3",
    ] {
        assert!(saved.contains(line), "应有 {line}:\n{saved}");
    }

    // 引号没配对的字符串字面量、不是 TOML 的值：用法错误，文件不动
    for (key, value) in [
        ("base_url", "\"https://unterminated"),
        ("max_request_mb", "many"),
        ("allowed_hosts", "a.example"),
    ] {
        let out = config(&home, &file, &["set", key, value]);
        assert_eq!(out.status.code(), Some(2), "{key}: {}", text(&out.stderr));
    }
    // 类型对不上的值过得了解析，过不了校验：退出码 3
    let out = config(&home, &file, &["set", "max_request_mb", "\"64\""]);
    assert_eq!(out.status.code(), Some(3), "{}", text(&out.stderr));
    assert!(
        text(&out.stderr).contains("[config_type]"),
        "{}",
        text(&out.stderr)
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), saved);
}

/// `get`：stdout 上只有值（TOML 写法），来源在 stderr；没设置的键什么都不打印，照样成功。
/// `--config` 可以写别名。
#[test]
fn get_prints_the_effective_value_and_its_source() {
    let home = TestHome::new();
    let file = write_config(&home, "get.toml", "[timeouts]\nconnect_secs = 7\n");
    let out = home
        .cmd()
        .args(["alias", "add", "work", file.to_str().unwrap()])
        .output()
        .report_exit()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    let get = |args: &[&str]| {
        home.cmd()
            .args(["config", "get"])
            .args(args)
            .args(["--config", "work"])
            .output()
            .report_exit()
            .unwrap()
    };

    let out = get(&["timeouts.connect_secs"]);
    assert_eq!(text(&out.stdout).trim_end(), "7");
    assert!(
        text(&out.stderr).contains("config.toml"),
        "{}",
        text(&out.stderr)
    );
    let out = get(&["listen_addr"]);
    assert_eq!(text(&out.stdout).trim_end(), "\"127.0.0.1:12345\"");
    assert!(
        text(&out.stderr).contains("built-in default"),
        "{}",
        text(&out.stderr)
    );
    // 一张表整体读：没写的键补上了默认值
    let out = get(&["timeouts"]);
    assert_eq!(
        text(&out.stdout).trim_end(),
        "{ connect_secs = 7, first_byte_secs = 300, read_idle_secs = 300 }"
    );
    // 认识、但没有值的键
    let out = get(&["api_key"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty(), "{}", text(&out.stdout));

    assert_eq!(
        document(&get(&["timeouts.connect_secs", "--json"]), &["get"]),
        json!({"schema": 1, "key": "timeouts.connect_secs", "value": 7, "source": "toml"})
    );
    assert_eq!(
        document(&get(&["keepalive.trigger", "--json"]), &["get"]),
        json!({"schema": 1, "key": "keepalive.trigger", "value": "any", "source": "default"})
    );
    assert_eq!(
        document(&get(&["api_key", "--json"]), &["get"]),
        json!({"schema": 1, "key": "api_key", "value": null})
    );

    // 不认识的键：退出码 3，--json 时 stdout 上是错误文档
    let out = get(&["timeouts.total_secs", "--json"]);
    assert_eq!(out.status.code(), Some(3));
    let error = document(&out, &["get"]);
    assert_eq!(error["error"]["code"], "config_unknown_key", "{error}");
    assert_eq!(error["error"]["details"]["key"], "timeouts.total_secs");
}

/// 密钥：`-` 从 stdin 读；读出来是占位；占位原样写回等于不改；对不上的占位被拒绝。
#[test]
fn secrets_are_read_as_placeholders_and_written_back_unchanged() {
    let home = TestHome::new();
    let file = write_config(&home, "secrets.toml", "");
    let mut child = home
        .cmd()
        .arg("config")
        .arg("--config")
        .arg(&file)
        .args(["set", "api_key", "-", "--json"])
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
    let out = child.wait_with_output().report_exit().unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    let placeholder = json!({"secret": true, "hint": "sk-C…cdef"});
    let written = document(&out, &["set"]);
    assert_eq!(written["value"], placeholder, "{written}");
    assert!(!text(&out.stdout).contains(SECRET) && !text(&out.stderr).contains(SECRET));
    let saved = std::fs::read_to_string(&file).unwrap();
    assert!(
        saved.contains(&format!("api_key = \"{SECRET}\"")),
        "{saved}"
    );

    // 每一种读法给的都是占位
    assert_eq!(
        doc(&home, &file, &["get", "api_key", "--json"])["value"],
        placeholder
    );
    assert_eq!(
        doc(&home, &file, &["show", "--json"])["values"]["api_key"],
        placeholder
    );
    let out = config(&home, &file, &["get", "api_key"]);
    assert_eq!(text(&out.stdout).trim_end(), "\"<secret sk-C…cdef>\"");

    // 占位写回：文件不变
    let again = doc(
        &home,
        &file,
        &[
            "set",
            "api_key",
            "{ secret = true, hint = \"sk-C…cdef\" }",
            "--json",
        ],
    );
    assert_eq!(again["changed"], false, "{again}");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), saved);

    // format 整体写回：env 里的占位按条目的名字与键名对上原值
    let out = config(
        &home,
        &file,
        &[
            "set",
            "format",
            r#"[{ name = "keys", stages = ["request"], command = "fmt", env = { UPSTREAM_KEY = "env-SECRET-0123456789" } }]"#,
        ],
    );
    assert!(out.status.success(), "{}", text(&out.stderr));
    let read = doc(&home, &file, &["get", "format", "--json"]);
    assert_eq!(
        read["value"][0]["env"]["UPSTREAM_KEY"],
        json!({"secret": true}),
        "{read}"
    );
    let out = config(
        &home,
        &file,
        &[
            "set",
            "format",
            r#"[{ name = "keys", stages = ["request", "response"], command = "fmt", env = { UPSTREAM_KEY = { secret = true } } }]"#,
        ],
    );
    assert!(out.status.success(), "{}", text(&out.stderr));
    let saved = std::fs::read_to_string(&file).unwrap();
    assert!(
        saved.contains("env-SECRET-0123456789") && saved.contains("\"response\""),
        "{saved}"
    );

    // 对不上的占位：原来没有值的键、改了名字的条目
    for (key, value, at) in [
        ("proxy_username", "{ secret = true }", "proxy_username"),
        (
            "format",
            r#"[{ name = "renamed", stages = ["request"], command = "fmt", env = { UPSTREAM_KEY = { secret = true } } }]"#,
            "format[0].env.UPSTREAM_KEY",
        ),
    ] {
        let out = config(&home, &file, &["set", key, value, "--json"]);
        assert_eq!(out.status.code(), Some(3), "{key}: {}", text(&out.stderr));
        let error = document(&out, &["set", key]);
        let diagnostic = &error["error"]["details"]["diagnostics"][0];
        assert_eq!(
            diagnostic["code"], "secret_placeholder_unmatched",
            "{error}"
        );
        assert_eq!(diagnostic["key"], at, "{error}");
    }
    assert_eq!(std::fs::read_to_string(&file).unwrap(), saved);
}

/// `show --json`：文件里的值；`--effective` 时多出每个键的来源与没有继承的凭据。文件有
/// 错误时文档照样给出，诊断在里面，退出码 3。
#[test]
fn show_json_documents() {
    let home = TestHome::new();
    std::fs::write(
        home.path().join("settings.json"),
        r#"{"schema":1,"defaults":{"base_url":"https://default.example.com","api_key":"sk-DEFAULT-KEY-0123456789abcdef","timeouts":{"connect_secs":11}}}"#,
    )
    .unwrap();
    let file = write_config(&home, "show.toml", "max_request_mb = 8\n");

    let plain = doc(&home, &file, &["show", "--json"]);
    assert_eq!(
        plain["values"],
        json!({"base_url": "https://api.example.com", "max_request_mb": 8})
    );
    assert_eq!(plain["file"], file.display().to_string());
    // 诊断说的是这份配置能不能用来启动，与显示哪一层的值无关：全局默认里的 key 没有继承
    // 的告警两种显示都带着
    assert_eq!(
        plain["diagnostics"][0]["code"], "default_credential_withheld",
        "{plain}"
    );
    assert_eq!(plain["diagnostics"].as_array().unwrap().len(), 1, "{plain}");
    assert!(
        plain.get("sources").is_none() && plain.get("withheld").is_none(),
        "{plain}"
    );

    let effective = doc(&home, &file, &["show", "--effective", "--json"]);
    assert_eq!(effective["values"]["max_request_mb"], 8);
    assert_eq!(effective["values"]["timeouts"]["connect_secs"], 11);
    assert_eq!(effective["values"]["listen_addr"], "127.0.0.1:12345");
    assert_eq!(effective["sources"]["max_request_mb"], "toml");
    assert_eq!(effective["sources"]["timeouts.connect_secs"], "settings");
    assert_eq!(effective["sources"]["listen_addr"], "default");
    // 全局默认里的 key 属于另一个上游：没有继承，列在 withheld 里
    assert!(effective["values"].get("api_key").is_none(), "{effective}");
    assert_eq!(effective["withheld"][0]["key"], "api_key", "{effective}");
    assert_eq!(effective["withheld"][0]["reason"], "base_url_differs");
    assert!(
        !effective.to_string().contains("sk-DEFAULT-KEY"),
        "{effective}"
    );

    // 有错误的文件
    std::fs::write(&file, "base_url = \"https://api.example.com\"\nnope = 1\n").unwrap();
    for args in [&["show", "--json"][..], &["show", "--effective", "--json"]] {
        let out = config(&home, &file, args);
        assert_eq!(out.status.code(), Some(3), "{args:?}");
        let shown = document(&out, args);
        assert_eq!(
            shown["diagnostics"][0]["code"], "config_unknown_key",
            "{shown}"
        );
        assert_eq!(shown["diagnostics"][0]["key"], "nope");
    }
    // 语法错误：读不出值，get 报 config_invalid
    std::fs::write(&file, "base_url = \n").unwrap();
    let out = config(&home, &file, &["get", "base_url", "--json"]);
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(document(&out, &["get"])["error"]["code"], "config_invalid");
    let out = config(&home, &file, &["set", "max_request_mb", "8", "--json"]);
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(document(&out, &["set"])["error"]["code"], "config_invalid");
}

/// `set` 与 `unset` 报告哪些运行中的实例要重启才用上这次改动：用着这份配置文件的那些。
#[test]
fn writes_report_the_instances_to_restart() {
    let home = TestHome::new();
    let listen = "listen_addr = '127.0.0.1:0'\n";
    let file = write_config(&home, "running.toml", listen);
    let other = write_config(&home, "other.toml", listen);
    let [port, other_port] = [&file, &other].map(|cfg| {
        let out = home
            .cmd()
            .args(["start", "--json", cfg.to_str().unwrap()])
            .stdin(Stdio::null())
            .output()
            .report_exit()
            .unwrap();
        assert!(
            out.status.success(),
            "{}{}",
            text(&out.stdout),
            text(&out.stderr)
        );
        home.track_started_port(&common::output_document(&out))
    });

    let set = doc(
        &home,
        &file,
        &["set", "timeouts.connect_secs", "9", "--json"],
    );
    assert_eq!(
        set,
        json!({
            "schema": 1,
            "file": file.display().to_string(),
            "key": "timeouts.connect_secs",
            "value": 9,
            "changed": true,
            "instances_to_restart": [port],
        })
    );
    // 同样的值再设一次：文件没变，没有实例需要重启
    let again = doc(
        &home,
        &file,
        &["set", "timeouts.connect_secs", "9", "--json"],
    );
    assert_eq!(again["changed"], false);
    assert_eq!(again["instances_to_restart"], json!([]));
    let unset = doc(&home, &file, &["unset", "timeouts.connect_secs", "--json"]);
    assert_eq!(unset["value"], Value::Null);
    assert_eq!(unset["instances_to_restart"], json!([port]));

    // 给人看的输出：重启的命令写在 stderr
    let out = config(&home, &file, &["set", "max_request_mb", "32"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(
        text(&out.stdout).contains("Saved to"),
        "{}",
        text(&out.stdout)
    );
    assert!(
        text(&out.stderr).contains(&format!("aproxy restart {port}"))
            && !text(&out.stderr).contains(&other_port.to_string()),
        "{}",
        text(&out.stderr)
    );
}

/// 默认配置文件的指向归 settings 管：`settings set default_config <路径>` 把路径按当前目录
/// 绝对化，之后不带 `--config` 的 config 命令读写的就是它。
#[test]
fn the_default_config_is_chosen_through_settings() {
    let home = TestHome::new();
    write_config(&home, "main.toml", "max_request_mb = 16\n");
    let out = home
        .cmd()
        .current_dir(home.path())
        .args(["settings", "set", "default_config", "main.toml"])
        .output()
        .report_exit()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    let settings: Value =
        serde_json::from_str(&std::fs::read_to_string(home.path().join("settings.json")).unwrap())
            .unwrap();
    let stored = PathBuf::from(settings["default_config"].as_str().unwrap());
    assert!(
        stored.is_absolute() && stored.ends_with("main.toml"),
        "{settings}"
    );

    let out = home
        .cmd()
        .args(["config", "get", "max_request_mb"])
        .output()
        .report_exit()
        .unwrap();
    assert_eq!(text(&out.stdout).trim_end(), "16", "{}", text(&out.stderr));
    let out = home
        .cmd()
        .args(["settings", "unset", "default_config"])
        .output()
        .report_exit()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    // 回到 <home>/config.toml：它还不存在，get 以 4 退出
    let out = home
        .cmd()
        .args(["config", "get", "max_request_mb"])
        .output()
        .report_exit()
        .unwrap();
    assert_eq!(out.status.code(), Some(4), "{}", text(&out.stderr));
}
