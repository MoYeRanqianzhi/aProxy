//! 两个独立运行的实例继承不同的语言。控制帧、错误体、清点文档与日志都来自真实写端。
mod common;
use common::{ChildGuard, TestHome};
use serde_json::{Value, json};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// 取协议帧原字节，而不是让客户端把拒绝转为 Display 再作比较。
async fn refusal(endpoint: &str) -> Vec<u8> {
    #[cfg(windows)]
    let mut stream = loop {
        match tokio::net::windows::named_pipe::ClientOptions::new().open(endpoint) {
            Ok(s) => break s,
            Err(e) if e.raw_os_error() == Some(231) => {
                tokio::time::sleep(Duration::from_millis(10)).await
            }
            Err(e) => panic!("{e}"),
        }
    };
    #[cfg(unix)]
    let mut stream = tokio::net::UnixStream::connect(endpoint).await.unwrap();
    stream
        .write_all(b"{\"v\":1,\"op\":\"locale_check\"}\n")
        .await
        .unwrap();
    let mut reply = Vec::new();
    BufReader::new(stream)
        .read_until(b'\n', &mut reply)
        .await
        .unwrap();
    reply
}

#[derive(Debug, PartialEq)]
struct Observed {
    status: String,
    http: String,
    ipc: Vec<u8>,
    log: String,
}

async fn observe(lang: &str, upstream: std::net::SocketAddr) -> Observed {
    let h = TestHome::new();
    let cfg = h.path().join("test.toml");
    std::fs::write(
        &cfg,
        format!("listen_addr = \"127.0.0.1:0\"\nbase_url = \"http://{upstream}\"\n"),
    )
    .unwrap();
    let child = ChildGuard(
        h.serve_cmd(&cfg)
            .env("APROXY_LANG", lang)
            .env("RUST_LOG", "info")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let run = h.path().join("run");
    let info = loop {
        if let Some(rec) = aproxy_ipc::registry::registry_instances_in(&run)
            .into_iter()
            .find(|r| r.pid == child.0.id())
            && aproxy::daemon::ipc_ping_in(&run, aproxy::daemon::port_of(&rec.listen_addr))
                .await
                .is_ok()
        {
            break rec;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    let port = aproxy::daemon::port_of(&info.listen_addr);
    let get_status = |reader| {
        let out = h
            .cmd()
            .env("APROXY_LANG", reader)
            .args(["status", "--json"])
            .output()
            .unwrap();
        assert!(out.status.success());
        out.stdout
    };
    let stdout = get_status("en");
    assert_eq!(stdout, get_status("zh-CN"));
    let mut doc: Value = serde_json::from_slice(&stdout).unwrap();
    assert_eq!(doc["instances"].as_array().unwrap().len(), 1);
    let one = &mut doc["instances"][0];
    assert_eq!(one["liveness"], "live");
    assert_eq!(one["port"], port.parse::<u16>().unwrap());
    one["port"] = 0.into();
    // 只替换已经与本次注册、实际路径和子进程核对过的标识及时间。所有句子及未知字段照字节比较。
    let record = &mut one["instance"];
    for (key, actual) in [
        ("pid", json!(child.0.id())),
        ("start", json!(info.start)),
        ("instance_id", json!(info.instance_id)),
        ("listen_addr", json!(info.listen_addr)),
        ("started_at", json!(info.started_at)),
        ("config_path", json!(info.config_path)),
        ("log_path", json!(info.log_path)),
    ] {
        assert_eq!(record[key], actual);
        record[key] = Value::Null;
    }
    assert_eq!(
        std::fs::canonicalize(info.config_path.as_ref().unwrap()).unwrap(),
        std::fs::canonicalize(&cfg).unwrap()
    );
    let log_path = Path::new(info.log_path.as_ref().unwrap());
    assert!(
        std::fs::canonicalize(log_path)
            .unwrap()
            .starts_with(std::fs::canonicalize(h.path()).unwrap())
    );
    let activity = &mut one["status"]["activity"];
    assert_eq!(activity["requests_total"], 0);
    assert!(activity["last_request_at"].is_u64());
    activity["last_request_at"] = Value::Null;
    let endpoint = aproxy_ipc::endpoint::instance(&run, port);
    let ipc = refusal(&endpoint).await;
    let frame: Value = serde_json::from_slice(&ipc).unwrap();
    assert_eq!(frame["error"]["code"], "unknown_op");
    assert_eq!(frame["error"]["message"], "unknown op: locale_check");
    let resp = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(format!("http://{}/locale-check", info.listen_addr))
        .header(
            "origin",
            reqwest::header::HeaderValue::from_bytes(b"\xff").unwrap(),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403);
    let raw = resp.text().await.unwrap();
    let body: Value = serde_json::from_str(&raw).unwrap();
    let error = &body["error"];
    let id = format!("{}-x1", info.instance_id);
    assert_eq!(error["aproxy"]["request_id"], id);
    assert_eq!(error["aproxy"]["details"]["origin"], "<non-ASCII>");
    assert!(error["message"].as_str().unwrap().contains("not allowed"));
    let log = loop {
        let text = std::fs::read_to_string(log_path).unwrap();
        if let Some(line) = text.lines().find(|l| l.contains("inbound_origin_rejected")) {
            assert!(
                line.contains("<non-ASCII>") && line.contains("was not forwarded"),
                "{line}"
            );
            assert!(line.contains(&id));
            break line.split_once(' ').unwrap().1.replace(&id, "<request-id>");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    Observed {
        status: serde_json::to_string(&doc).unwrap(),
        http: raw.replace(&id, "<request-id>"),
        ipc,
        log,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worker_output_does_not_follow_the_launchers_language() {
    // 不预选端口；两个实例共用的上游也完全留在环回，拒绝应发生在转发之前。
    let local = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = local.local_addr().unwrap();
    let run = |lang| tokio::time::timeout(Duration::from_secs(60), observe(lang, addr));
    assert_eq!(run("en").await.unwrap(), run("zh-CN").await.unwrap());
}
