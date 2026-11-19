//! 按设计稿 §9.4 把各命令的 `--json` 输出作为整份文档钉住：agent 和脚本会解析它们，
//! 文档形状只要变了就是破坏性变更。

mod common;
use common::{TestHome, free_port};

use std::fs;
use std::process::{Output, Stdio};

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

/// 按集合核对字段：多一个或少一个都不行，但不约束对象内部的排列顺序。
fn assert_keys(value: &Value, keys: &[&str]) {
    let obj = value.as_object().unwrap();
    assert_eq!(obj.len(), keys.len(), "{value}");
    for key in keys {
        assert!(obj.contains_key(*key), "缺少 {key}: {value}");
    }
}

/// 别名按名称排序；目标消失只改变 exists，不会抹掉已登记的路径。
#[test]
fn alias_list_json_document() {
    let home = TestHome::new();
    let args = ["alias", "list", "--json"];
    let empty = run(&home, &args);
    assert_eq!(document(&empty), json!({"schema": 1, "aliases": []}));
    assert!(String::from_utf8_lossy(&empty.stderr).contains("暂无别名"));

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
    assert!(String::from_utf8_lossy(&empty.stderr).contains("没有找到配置文件"));

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
