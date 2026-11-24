//! 0.2 的公开契约，从外面看：对一个真的实例走控制通道、读它写在盘上的记录，逐项与这里
//! 的样本比对。
//!
//! 这些形状是「只增不改」的：以后的版本可以加字段、加 op、加参数、加码、加取值，已有的
//! 不删、不改名、不改类型、不改含义——别的版本的 aproxy（原地升级时新旧并存）、界面、
//! agent 与第三方客户端都按它们读写。所以这里的比对分两种：
//!
//! - **错误应答**逐字比对（`message` 除外，它是给日志看的英文，不是契约）；
//! - **快照与记录**用 [`assert_covers`]：样本里的每个键都必须在、类型必须相同，多出来的
//!   键不算错。加字段不必改样本；删掉或改掉样本里已有的东西会让测试失败，那就是破坏契约，
//!   **不要改样本来迁就代码**。
//!
//! 本文件用不到实例内部的东西，只看线上与盘上的字节。逐字钉住每个结构序列化结果的样本
//! 在各模块自己的单测里（它们要用模块私有的函数），清单见 [`PINNED_IN_MODULES`]，最后一个
//! 测试守着它们不被悄悄删掉或改名。
//!
//! 隔离：每个测试一个 `common::TestHome`，端口向系统要，结束时按端口 stop。

mod common;
use common::ReportExit as _;
use common::{TestHome, free_port};

use serde_json::{Value, json};
use std::path::Path;
use std::time::{Duration, Instant};

/// 各模块里逐字钉住形状的单测：(源文件, 测试函数名)。
const PINNED_IN_MODULES: &[(&str, &str)] = &[
    // 实例记录：盘上的 `run/<端口>.instance.json` 与快照里的 `instance`
    ("src/daemon.rs", "instance_record_shape_is_pinned"),
    // 每个 op 的请求行（含各个参数）
    (
        "aproxy-ipc/src/wire.rs",
        "request_lines_are_pinned_and_round_trip_through_the_server_parser",
    ),
    // 每种拒绝的应答行
    ("aproxy-ipc/src/wire.rs", "rejections_are_pinned"),
    // 读请求历史的 op：请求行与拒绝
    (
        "aproxy-ipc/src/wire.rs",
        "read_request_lines_are_pinned_and_round_trip_through_the_server_parser",
    ),
    ("src/daemon.rs", "read_rejections_are_pinned"),
    ("src/daemon.rs", "format_restart_frames_are_pinned"),
    ("src/daemon.rs", "format_control_frames_are_pinned"),
    // 请求记录
    ("src/history.rs", "record_shape_is_pinned"),
    // 成功应答与状态快照
    ("src/daemon.rs", "snapshot_shape_is_pinned"),
    // 客户端怎样读别的版本的应答
    ("src/daemon.rs", "client_reads_v1_errors_and_unknown_states"),
    // 拉起规格与期望状态记录
    ("src/spec.rs", "spec_serializes_to_the_documented_shape"),
    // 看护者记录 `run/watchdog.json`
    ("src/watchdog.rs", "record_shape_is_pinned"),
    // 只读清点的结果
    ("aproxy-ipc/src/enumerate.rs", "enumeration_shape_is_pinned"),
    // HTTP 错误体与 SSE 终态事件
    ("src/proxy/errors.rs", "payload_has_the_documented_shape"),
    // 控制端点的名字
    (
        "aproxy-ipc/src/endpoint.rs",
        "names_have_the_documented_shape",
    ),
    // 码表导出的 JSON
    ("aproxy-ipc/src/code.rs", "exported_json_shape"),
];

/// 断言 `actual` 覆盖 `sample`：
/// - 对象：样本里的每个键都在，值各自覆盖；多出来的键不管；
/// - 数组：样本给了元素时，实际的每个元素都要覆盖样本的第一个元素；样本是空数组时只要求
///   实际也是数组；
/// - 其余：JSON 类型相同（null、布尔、数、字符串），值不比。
fn assert_covers(actual: &Value, sample: &Value, at: &str) {
    match (actual, sample) {
        (Value::Object(actual), Value::Object(sample)) => {
            for (key, expected) in sample {
                let found = actual
                    .get(key)
                    .unwrap_or_else(|| panic!("{at}.{key} 不见了：已有的键不能删、不能改名"));
                assert_covers(found, expected, &format!("{at}.{key}"));
            }
        }
        (Value::Array(actual), Value::Array(sample)) => {
            if let Some(expected) = sample.first() {
                for (index, item) in actual.iter().enumerate() {
                    assert_covers(item, expected, &format!("{at}[{index}]"));
                }
            }
        }
        (actual, sample) => assert_eq!(
            std::mem::discriminant(actual),
            std::mem::discriminant(sample),
            "{at} 的类型变了：实际是 {actual}，样本是 {sample}"
        ),
    }
}

/// 快照的样本：`{"v":1,"ok":true,"result":…}` 里的 `result`。有配置文件、有日志文件的后台
/// 实例；没有失败过。
fn snapshot_sample() -> Value {
    json!({
        "instance": {
            "pid": 4321,
            "start": "w:133712345678901234",
            "instance_id": "19ac3f2e8b5d1a2b",
            "version": "0.2.0",
            "listen_addr": "127.0.0.1:12346",
            "config_path": "/home/u/.aproxy/config.toml",
            "base_url": "https://api.example.com",
            "started_at": 1_760_190_000_123_u64,
            "log_path": "/home/u/.aproxy/logs/19ac3f2e8b5d-1a2b.log",
            "foreground": false
        },
        "state": "serving",
        "behavior": {
            "delivery": "retry",
            "disk_cache": true,
            "keepalive": {"trigger": "any", "interval_secs": 15},
            "retry_rules": 1,
            "history": {"body_budget_mb": 64}
        },
        "activity": {
            "last_request_at": 1_760_190_000_123_u64,
            "requests_total": 0,
            "retries_total": 0,
            "rescued_total": 0,
            "in_flight": 0,
            "retrying": 0,
            "by_code": {},
            "buckets": {"width_secs": 120, "requests": [0], "retries": [0], "failures": [0]},
            "last_error": null
        },
        "ops": ["ping"],
        "features": ["shutdown.keep_restore"]
    })
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

/// 控制端点上的一条连接（两个平台的传输不同，这里只当它是能读能写的字节流）。
trait Duplex: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin> Duplex for T {}

/// 向控制端点原样写出 `request`（调用方自己带换行），读回一行应答。不经 aproxy 的客户端
/// 代码：别的程序连上来看到的就是这些字节。
async fn exchange(endpoint: &str, request: &str) -> String {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let (reader, mut writer) = tokio::io::split(connect(endpoint).await);
    writer.write_all(request.as_bytes()).await.unwrap();
    let mut line = String::new();
    BufReader::new(reader).read_line(&mut line).await.unwrap();
    line
}

/// 连上控制端点。
async fn connect(endpoint: &str) -> Box<dyn Duplex> {
    #[cfg(windows)]
    {
        use tokio::net::windows::named_pipe::ClientOptions;
        // 231 = ERROR_PIPE_BUSY：服务端正在两个连接之间重建监听实例，稍后再连
        let deadline = Instant::now() + Duration::from_secs(5);
        let client = loop {
            match ClientOptions::new().open(endpoint) {
                Ok(client) => break client,
                Err(e) if e.raw_os_error() == Some(231) && Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(e) => panic!("连不上控制端点 {endpoint}：{e}"),
            }
        };
        Box::new(client)
    }
    #[cfg(unix)]
    {
        let client = tokio::net::UnixStream::connect(endpoint)
            .await
            .unwrap_or_else(|e| panic!("连不上控制端点 {endpoint}：{e}"));
        Box::new(client)
    }
}

/// 一个在隔离 home 里启动好的实例，与向它发原始请求的办法。
struct Instance<'a> {
    home: &'a TestHome,
    port: u16,
    rt: tokio::runtime::Runtime,
}

impl Instance<'_> {
    /// 发一行请求，应答按 JSON 解析。应答必须是恰好一行。
    fn ask(&self, request: &str) -> Value {
        let endpoint =
            aproxy::daemon::endpoint_for_in(&self.home.path().join("run"), &self.port.to_string());
        let line = self.rt.block_on(exchange(&endpoint, request));
        assert!(line.ends_with('\n'), "应答应以换行结束：{line:?}");
        assert_eq!(line.matches('\n').count(), 1, "应答应是一行：{line:?}");
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("应答不是 JSON（{e}）：{line}"))
    }

    /// 这个实例的控制端点。
    fn endpoint(&self) -> String {
        aproxy::daemon::endpoint_for_in(&self.home.path().join("run"), &self.port.to_string())
    }

    /// 被拒的请求：应答除 `message` 外必须逐字等于 `expected`（`{"code","details"}`），码与
    /// `details` 的键都登记在码表里。
    fn rejected(&self, request: &str, expected: &Value) {
        let mut reply = self.ask(request);
        assert!(
            reply["error"]["message"]
                .as_str()
                .is_some_and(|m| m.is_ascii() && !m.is_empty()),
            "message 是一句英文：{reply}"
        );
        reply["error"]
            .as_object_mut()
            .unwrap()
            .shift_remove("message");
        assert_eq!(
            reply,
            json!({"v": 1, "ok": false, "error": expected}),
            "{request}"
        );
        let code = aproxy_ipc::code::lookup(expected["code"].as_str().unwrap())
            .unwrap_or_else(|| panic!("码表里没有 {}", expected["code"]));
        for key in expected["details"].as_object().unwrap().keys() {
            assert!(
                code.details.iter().any(|detail| detail.key == key),
                "{} 的 details 里有码表没登记的键 {key}",
                code.name
            );
        }
    }

    /// 成功应答里的快照；应答不成功就失败。
    fn snapshot(&self, request: &str) -> Value {
        let reply = self.ask(request);
        assert_eq!(reply["v"], 1, "{reply}");
        assert_eq!(reply["ok"], true, "{reply}");
        assert!(reply.get("error").is_none(), "成功应答不带 error：{reply}");
        reply["result"].clone()
    }

    /// 这个端口的控制端点是否已经没有人应答。
    fn gone(&self) -> bool {
        self.rt
            .block_on(aproxy::daemon::ipc_ping_in(
                &self.home.path().join("run"),
                &self.port.to_string(),
            ))
            .is_err()
    }
}

/// 在 `home` 里以 `args` 启动一个后台实例，等它就绪（`aproxy start` 返回即就绪）。
fn start<'a>(home: &'a TestHome, port: u16, args: &[&str]) -> Instance<'a> {
    let mut cmd = home.cmd();
    cmd.args(args);
    if port == 0 {
        cmd.arg("--json");
    } else {
        home.stop_on_drop(port);
    }
    let out = cmd
        .env("NO_COLOR", "1")
        .output()
        .report_exit()
        .expect("运行 aproxy 失败");
    assert!(
        out.status.success(),
        "start 失败：{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let port = if port == 0 {
        let bound = u16::try_from(common::output_document(&out)["port"].as_u64().unwrap()).unwrap();
        home.stop_on_drop(bound);
        bound
    } else {
        port
    };
    Instance {
        home,
        port,
        rt: runtime(),
    }
}

fn wait_for(timeout: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if cond() {
            return true;
        }
        if Instant::now() > deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// 读 `run/` 里的一份记录：必须是恰好一行 JSON。
fn record_on_disk(home: &Path, name: &str) -> Value {
    let path = home.join("run").join(name);
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("读不到 {}：{e}", path.display()));
    assert_eq!(
        text.trim_end_matches('\n').matches('\n').count(),
        0,
        "{name} 应是一行 JSON：{text:?}"
    );
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{name} 不是 JSON（{e}）：{text}"))
}

/// 请求记录（RequestRecord）的样本：`requests` 的元素、`request` 结果里的 `record`、
/// `request` 事件的 `data`。一个重试过一次、还在进行的请求。
fn record_sample() -> Value {
    json!({
        "request_id": "19ac3f2e8b5d1a2b-42",
        "request_seq": 42,
        "started_at": 1_760_190_000_123_u64,
        "method": "POST",
        "path": "/v1/messages?beta=***",
        "stream": true,
        "delivery": "retry",
        "upgraded": false,
        "outcome": "in_flight",
        "phase": "attempting",
        "attempts": 2,
        "tries_omitted": 0,
        "tries": [{
            "attempt": 1,
            "started_at": 1_760_190_000_200_u64,
            "response_bytes": 96,
            "reports": []
        }],
        "keepalive": true,
        "heartbeats": 3,
        "request_bytes": 20480,
        "response_bytes": 0,
        "reports": []
    })
}

/// 可以为 null 的键：样本里给不出一个固定的类型，只要求键在。
const NULLABLE_RECORD_KEYS: &[&str] = &[
    "ended_at",
    "model",
    "user_agent",
    "code",
    "client_status",
    "next_attempt_at",
];
const NULLABLE_TRY_KEYS: &[&str] = &["duration_ms", "upstream_status", "code", "decision"];

/// 断言 `record` 是一条请求记录：覆盖样本，可空的键一个不少。
fn assert_is_record(record: &Value, at: &str) {
    assert_covers(record, &record_sample(), at);
    for key in NULLABLE_RECORD_KEYS {
        assert!(record.get(key).is_some(), "{at}.{key} 不见了");
    }
    for (index, attempt) in record["tries"].as_array().unwrap().iter().enumerate() {
        for key in NULLABLE_TRY_KEYS {
            assert!(
                attempt.get(key).is_some(),
                "{at}.tries[{index}].{key} 不见了"
            );
        }
    }
}

/// 一个只会回固定响应的上游：每条连接读完一个请求、回一个 200，然后关闭。返回它的地址。
/// 线程随测试程序结束。
fn fixed_upstream(body: &'static str) -> String {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            // 读到请求头结束，再按 content-length 读完请求体
            let mut seen = Vec::new();
            let mut buf = [0u8; 4096];
            let head_end = loop {
                if let Some(at) = seen.windows(4).position(|w| w == b"\r\n\r\n") {
                    break at + 4;
                }
                match stream.read(&mut buf) {
                    Ok(n) if n > 0 => seen.extend_from_slice(&buf[..n]),
                    _ => break seen.len(),
                }
            };
            let head = String::from_utf8_lossy(&seen[..head_end]).to_ascii_lowercase();
            let length: usize = head
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
                .and_then(|value| value.trim().parse().ok())
                .unwrap_or(0);
            while seen.len() < head_end + length {
                match stream.read(&mut buf) {
                    Ok(n) if n > 0 => seen.extend_from_slice(&buf[..n]),
                    _ => break,
                }
            }
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    url
}

/// 经代理发一个 POST，读完整个响应，返回响应的全文（状态行、头与体）。
fn post_through(port: u16, path: &str, body: &str) -> String {
    use std::io::{Read, Write};
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    write!(
        stream,
        "POST {path} HTTP/1.1\r\nhost: 127.0.0.1:{port}\r\ncontent-type: application/json\r\nuser-agent: contract-test/1.0\r\nx-api-key: client-side-0123456789abcdef\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

/// 请求历史与事件，走真实例的控制端点：`requests`、`request`、`body` 的结果，它们的拒绝，
/// `subscribe` 的第一行与之后的事件。经控制通道出去的字节里没有密钥原文。
#[test]
fn request_history_is_read_over_the_control_channel() {
    use base64::Engine as _;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    const KEY: &str = "sk-contract-0123456789abcdefghij";
    const UPSTREAM_BODY: &str = r#"{"id":"msg_1","content":"hello"}"#;

    let home = TestHome::new();
    let port = free_port();
    let upstream = fixed_upstream(UPSTREAM_BODY);
    let cfg = home.path().join("history.toml");
    std::fs::write(
        &cfg,
        format!(
            "base_url = \"{upstream}\"\nlisten_addr = \"127.0.0.1:{port}\"\napi_key = \"{KEY}\"\n"
        ),
    )
    .unwrap();
    let instance = start(&home, port, &["start", "--config", cfg.to_str().unwrap()]);
    let snapshot = instance.snapshot("{\"v\":1,\"op\":\"ping\"}\n");
    assert_eq!(snapshot["behavior"]["history"]["body_budget_mb"], 64);

    // 还没有请求
    let empty = instance.ask("{\"v\":1,\"op\":\"requests\"}\n");
    assert_eq!(
        empty,
        json!({"v": 1, "ok": true, "result": {"requests": [], "more": false}})
    );

    // ---- subscribe：第一行是一元应答，带快照与在途的记录 ----
    let (subscriber, mut subscriber_writer) =
        tokio::io::split(instance.rt.block_on(connect(&instance.endpoint())));
    let mut events = BufReader::new(subscriber);
    let next_line = |events: &mut BufReader<_>| -> Value {
        let mut line = String::new();
        instance
            .rt
            .block_on(async {
                tokio::time::timeout(Duration::from_secs(20), events.read_line(&mut line)).await
            })
            .expect("20 秒内应有一行")
            .unwrap();
        assert!(line.ends_with('\n'), "连接提前关闭：{line:?}");
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("不是 JSON（{e}）：{line}"))
    };
    instance
        .rt
        .block_on(subscriber_writer.write_all(b"{\"v\":1,\"op\":\"subscribe\"}\n"))
        .unwrap();
    let first = next_line(&mut events);
    assert_eq!((&first["v"], &first["ok"]), (&json!(1), &json!(true)));
    assert_covers(&first["result"]["snapshot"], &snapshot_sample(), "snapshot");
    assert_eq!(first["result"]["requests"], json!([]));

    // ---- 一个请求经过代理 ----
    let sent = format!(r#"{{"model":"claude-contract","stream":false,"note":"{KEY}"}}"#);
    let response = post_through(port, "/v1/messages?beta=tools", &sent);
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.ends_with(UPSTREAM_BODY), "{response}");

    // 订阅者先后收到开始与结束；之间与之后可能夹着 state 事件（计数变了）
    let mut request_events = Vec::new();
    let mut last_seq = 0;
    while request_events.len() < 2 {
        let event = next_line(&mut events);
        assert_covers(
            &event,
            &json!({"v": 1, "event": "request", "seq": 42, "at": 1_760_190_000_123_u64}),
            "event",
        );
        let seq = event["seq"].as_u64().unwrap();
        assert_eq!(seq, last_seq + 1, "seq 在一条连接上从 1 起递增");
        last_seq = seq;
        match event["event"].as_str().unwrap() {
            "request" => request_events.push(event["data"].clone()),
            "state" => assert_covers(&event["data"], &snapshot_sample(), "state.data"),
            "heartbeat" => assert_eq!(event["data"], json!({})),
            other => panic!("意料之外的事件 {other}：{event}"),
        }
    }
    for (index, record) in request_events.iter().enumerate() {
        assert_is_record(record, &format!("events[{index}]"));
    }
    assert_eq!(request_events[0]["outcome"], "in_flight");
    assert_eq!(request_events[1]["outcome"], "ok");
    assert_eq!(request_events[1]["phase"], "done");

    // ---- requests ----
    let listed = instance.ask("{\"v\":1,\"op\":\"requests\",\"args\":{\"limit\":10}}\n");
    assert_eq!(listed["ok"], true, "{listed}");
    assert_eq!(listed["result"]["more"], false);
    let records = listed["result"]["requests"].as_array().unwrap();
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_is_record(record, "requests[0]");
    assert_eq!(record, &request_events[1], "事件里的就是整条记录");
    let id = record["request_id"].as_str().unwrap();
    // 请求标识是「实例标识-序号」；实例标识是 16 位十六进制，与快照里的是同一个
    let instance_id = snapshot["instance"]["instance_id"].as_str().unwrap();
    assert!(
        instance_id.len() == 16 && instance_id.bytes().all(|b| b.is_ascii_hexdigit()),
        "{instance_id}"
    );
    assert_eq!(
        id,
        format!(
            "{}-1",
            snapshot["instance"]["instance_id"].as_str().unwrap()
        )
    );
    assert_eq!(record["path"], "/v1/messages?beta=***");
    assert_eq!(record["model"], "claude-contract");
    assert_eq!(record["user_agent"], "contract-test/1.0");
    assert_eq!(record["client_status"], 200);
    assert_eq!(record["request_bytes"], sent.len());
    assert_eq!(record["response_bytes"], UPSTREAM_BODY.len());

    // ---- request ----
    let detail = instance.ask(&format!(
        "{{\"v\":1,\"op\":\"request\",\"args\":{{\"request_id\":\"{id}\"}}}}\n"
    ));
    assert_eq!(detail["ok"], true, "{detail}");
    let detail = &detail["result"];
    let redacted_len = sent.len() - KEY.len() + "<secret>".len();
    assert_covers(
        detail,
        &json!({
            "request": {
                "headers": [{"name": "x-api-key"}],
                "body": {"bytes": 20480, "state": "retained"}
            },
            "tries": [{
                "attempt": 1,
                "sent": {
                    "method": "POST",
                    "url": "https://relay.example.com/v1/chat/completions",
                    "headers": [{"name": "authorization"}],
                    "body": {"bytes": 18200, "state": "retained"}
                },
                "response": {
                    "upstream_status": 529,
                    "headers": [{"name": "content-type"}],
                    "body": {"bytes": 96, "state": "retained"}
                }
            }]
        }),
        "detail",
    );
    assert_eq!(&detail["record"], record);
    assert_eq!(
        detail["request"]["body"],
        json!({"bytes": redacted_len, "state": "retained"})
    );
    assert_eq!(
        detail["tries"][0]["sent"]["body"],
        json!({"bytes": redacted_len, "state": "same_as_previous"})
    );
    assert_eq!(detail["tries"][0]["error"], Value::Null);
    let header_value = |headers: &Value, name: &str| {
        headers
            .as_array()
            .unwrap()
            .iter()
            .find(|h| h["name"] == name)
            .map(|h| h["value"].clone())
    };
    // 密钥头的值是占位对象：`{"secret":true}`，够长时带 `hint`
    assert_eq!(
        header_value(&detail["request"]["headers"], "x-api-key"),
        Some(json!({"secret": true, "hint": "clie…cdef"}))
    );
    assert_eq!(
        header_value(&detail["tries"][0]["sent"]["headers"], "x-api-key"),
        Some(json!({"secret": true, "hint": "sk-c…ghij"}))
    );
    assert_eq!(
        header_value(&detail["request"]["headers"], "content-type"),
        Some(json!("application/json"))
    );

    // ---- body：分两块读回请求体，再读响应体 ----
    let read = |args: &str| -> Value {
        let reply = instance.ask(&format!(
            "{{\"v\":1,\"op\":\"body\",\"args\":{{\"request_id\":\"{id}\",{args}}}}}\n"
        ));
        assert_eq!(reply["ok"], true, "{reply}");
        reply["result"].clone()
    };
    let decode = |chunk: &Value| {
        base64::engine::general_purpose::STANDARD
            .decode(chunk["data_b64"].as_str().unwrap())
            .unwrap()
    };
    let head = read("\"part\":\"request\",\"limit\":16");
    assert_eq!(
        (
            &head["total_bytes"],
            &head["offset"],
            &head["more"],
            &head["complete"]
        ),
        (&json!(redacted_len), &json!(0), &json!(true), &json!(true))
    );
    let tail = read("\"part\":\"request\",\"offset\":16");
    assert_eq!(tail["offset"], 16);
    assert_eq!(tail["more"], false);
    let mut stored = decode(&head);
    assert_eq!(stored.len(), 16);
    stored.extend(decode(&tail));
    assert_eq!(
        String::from_utf8(stored).unwrap(),
        sent.replace(KEY, "<secret>")
    );
    let at_end = read(&format!("\"part\":\"request\",\"offset\":{redacted_len}"));
    assert_eq!(
        at_end,
        json!({"total_bytes": redacted_len, "offset": redacted_len, "data_b64": "", "more": false, "complete": true})
    );
    let upstream_reply = read("\"part\":\"response\",\"attempt\":1");
    assert_eq!(decode(&upstream_reply), UPSTREAM_BODY.as_bytes());
    assert_eq!(
        decode(&read("\"part\":\"sent\",\"attempt\":1")).len(),
        redacted_len
    );

    // ---- 拒绝：除 message 外逐字 ----
    let body_request = |args: &str| {
        format!("{{\"v\":1,\"op\":\"body\",\"args\":{{\"request_id\":\"{id}\",{args}}}}}\n")
    };
    let rejections = [
        (
            "{\"v\":1,\"op\":\"request\",\"args\":{\"request_id\":\"0000000000000000-1\"}}\n"
                .to_string(),
            json!({"code": "not_found", "details": {"request_id": "0000000000000000-1", "reason": "unknown_request"}}),
        ),
        (
            body_request("\"part\":\"response\",\"attempt\":7"),
            json!({"code": "not_found", "details": {"request_id": id, "reason": "no_such_part"}}),
        ),
        (
            body_request(&format!(
                "\"part\":\"request\",\"offset\":{}",
                redacted_len + 1
            )),
            json!({"code": "bad_request", "details": {"reason": "invalid_arg", "arg": "offset"}}),
        ),
        (
            body_request("\"part\":\"request\",\"limit\":0"),
            json!({"code": "bad_request", "details": {"reason": "invalid_arg", "arg": "limit"}}),
        ),
        (
            body_request("\"part\":\"request\",\"limit\":1048577"),
            json!({"code": "bad_request", "details": {"reason": "invalid_arg", "arg": "limit"}}),
        ),
        (
            body_request("\"part\":\"sent\""),
            json!({"code": "bad_request", "details": {"reason": "missing_arg", "arg": "attempt"}}),
        ),
        (
            "{\"v\":1,\"op\":\"request\"}\n".to_string(),
            json!({"code": "bad_request", "details": {"reason": "missing_arg", "arg": "request_id"}}),
        ),
        (
            "{\"v\":1,\"op\":\"requests\",\"args\":{\"limit\":201}}\n".to_string(),
            json!({"code": "bad_request", "details": {"reason": "invalid_arg", "arg": "limit"}}),
        ),
        (
            "{\"v\":1,\"op\":\"subscribe\",\"args\":{\"filter\":\"x\"}}\n".to_string(),
            json!({"code": "unsupported_arg", "details": {"args": ["filter"]}}),
        ),
    ];
    for (request, expected) in &rejections {
        instance.rejected(request, expected);
    }

    // ---- 流式连接最多 8 条，第 9 条回 busy ----
    let mut held = Vec::new();
    for _ in 0..7 {
        let (extra, mut extra_writer) =
            tokio::io::split(instance.rt.block_on(connect(&instance.endpoint())));
        instance
            .rt
            .block_on(extra_writer.write_all(b"{\"v\":1,\"op\":\"subscribe\"}\n"))
            .unwrap();
        let mut extra = BufReader::new(extra);
        assert_eq!(next_line(&mut extra)["ok"], true);
        // 读写两半都留着：丢掉任何一半，服务端都可能当成客户端走了
        held.push((extra, extra_writer));
    }
    instance.rejected(
        "{\"v\":1,\"op\":\"subscribe\"}\n",
        &json!({"code": "busy", "details": {"limit": 8}}),
    );
    drop(held);

    // ---- 密钥不出线：到此为止经控制通道读到的每一行里都没有密钥原文 ----
    for value in [&listed, detail, &head, &tail, &first] {
        let wire = value.to_string();
        assert!(
            !wire.contains(KEY) && !wire.contains("client-side-0123456789abcdef"),
            "{wire}"
        );
    }

    // ---- 实例退出前告诉订阅者：closing，然后关连接 ----
    let stopping = instance.snapshot("{\"v\":1,\"op\":\"shutdown\"}\n");
    assert_eq!(stopping["state"], "stopping");
    let mut saw_stopping = false;
    loop {
        let event = next_line(&mut events);
        match event["event"].as_str().unwrap() {
            "closing" => {
                assert_eq!(event["data"], json!({"reason": "shutdown"}));
                break;
            }
            "state" => saw_stopping |= event["data"]["state"] == "stopping",
            "heartbeat" | "request" => {}
            other => panic!("意料之外的事件 {other}：{event}"),
        }
    }
    assert!(
        saw_stopping,
        "状态变成 stopping 时订阅者应收到一个 state 事件"
    );
    let mut rest = String::new();
    let closed = instance.rt.block_on(async {
        tokio::time::timeout(Duration::from_secs(10), events.read_line(&mut rest)).await
    });
    assert!(
        matches!(closed, Ok(Ok(0)) | Ok(Err(_))),
        "closing 之后服务端应关闭连接：{closed:?} {rest:?}"
    );
    assert!(
        wait_for(Duration::from_secs(15), || instance.gone()),
        "收到 shutdown 的实例应退出"
    );
}

/// 控制协议 v1：每个 op 的请求与成功应答、每种拒绝，走真实例的控制端点。
#[test]
fn control_channel_speaks_v1_as_sampled() {
    let home = TestHome::new();
    let port = free_port();
    let cfg = home.path().join("contract.toml");
    // 上游地址带 userinfo：快照与记录里只许出现打过码的
    std::fs::write(
        &cfg,
        format!(
            "base_url = \"https://alice:WIREPASS123@contract-test.example.com/v1\"\nlisten_addr = \"127.0.0.1:{port}\"\n"
        ),
    )
    .unwrap();
    let instance = start(&home, port, &["start", "--config", cfg.to_str().unwrap()]);

    // ---- ping ----
    let snapshot = instance.snapshot("{\"v\":1,\"op\":\"ping\"}\n");
    assert_covers(&snapshot, &snapshot_sample(), "result");
    assert_eq!(snapshot["state"], "serving");
    assert_eq!(
        snapshot["instance"]["listen_addr"],
        format!("127.0.0.1:{port}")
    );
    assert_eq!(snapshot["instance"]["foreground"], false);
    assert_eq!(snapshot["activity"]["requests_total"], 0);
    assert_eq!(snapshot["activity"]["buckets"]["width_secs"], 120);
    for series in ["requests", "retries", "failures"] {
        assert_eq!(
            snapshot["activity"]["buckets"][series]
                .as_array()
                .unwrap()
                .len(),
            30,
            "{series}"
        );
    }
    // 调用方靠这两张表决定用不用某个 op、带不带某个参数：已有的名字不能少
    for op in [
        "ping",
        "shutdown",
        "prepare_swap",
        "requests",
        "request",
        "body",
        "subscribe",
    ] {
        assert!(
            snapshot["ops"].as_array().unwrap().contains(&json!(op)),
            "{op}"
        );
    }
    for feature in [
        "shutdown.keep_restore",
        "shutdown.if_idle_secs",
        "prepare_swap.ttl_secs",
    ] {
        assert!(
            snapshot["features"]
                .as_array()
                .unwrap()
                .contains(&json!(feature)),
            "{feature}"
        );
    }
    // 密钥不出线：userinfo 已打码
    let wire = snapshot.to_string();
    assert!(
        !wire.contains("WIREPASS123") && !wire.contains("alice"),
        "{wire}"
    );
    assert!(
        snapshot["instance"]["base_url"]
            .as_str()
            .unwrap()
            .contains("contract-test.example.com"),
        "{wire}"
    );

    // 盘上的实例记录 = `schema` 加上快照里的 `instance`
    let mut on_disk = record_on_disk(home.path(), &format!("{port}.instance.json"));
    assert_eq!(on_disk["schema"], 1);
    on_disk.as_object_mut().unwrap().shift_remove("schema");
    assert_eq!(on_disk, snapshot["instance"]);

    // 请求里多出来的顶层字段被忽略（以后的客户端可能带上）；`args` 给空对象等于没给
    let tolerant =
        instance.snapshot("{\"v\":1,\"op\":\"ping\",\"args\":{},\"trace\":{\"id\":7}}\n");
    assert_eq!(tolerant["instance"], snapshot["instance"]);

    // ---- 拒绝：除 message 外逐字 ----
    let rejections = [
        (
            "{\"v\":1,\"op\":\"what\"}\n",
            json!({"code": "unknown_op", "details": {"op": "what"}}),
        ),
        (
            "{\"v\":2,\"op\":\"ping\"}\n",
            json!({"code": "unsupported_version", "details": {"supported": [1]}}),
        ),
        (
            "{\"v\":1,\"op\":\"shutdown\",\"args\":{\"drain_secs\":5,\"keep_restore\":true,\"later\":1}}\n",
            json!({"code": "unsupported_arg", "details": {"args": ["drain_secs", "later"]}}),
        ),
        (
            "{\"v\":1,\"op\":\"ping\",\"args\":{\"x\":1}}\n",
            json!({"code": "unsupported_arg", "details": {"args": ["x"]}}),
        ),
        (
            "{\"v\":1,\"op\":\"shutdown\",\"args\":{\"keep_restore\":\"yes\"}}\n",
            json!({"code": "bad_request", "details": {"reason": "invalid_arg", "arg": "keep_restore"}}),
        ),
        (
            "{\"v\":1,\"op\":\"prepare_swap\",\"args\":{\"ttl_secs\":0}}\n",
            json!({"code": "bad_request", "details": {"reason": "invalid_arg", "arg": "ttl_secs"}}),
        ),
        (
            "not json\n",
            json!({"code": "bad_request", "details": {"reason": "not_json"}}),
        ),
        (
            // 0.1.0 的请求没有 v
            "{\"op\":\"ping\"}\n",
            json!({"code": "bad_request", "details": {"reason": "missing_v"}}),
        ),
        (
            "{\"v\":1}\n",
            json!({"code": "bad_request", "details": {"reason": "missing_op"}}),
        ),
        (
            "{\"v\":1,\"op\":\"ping\",\"args\":3}\n",
            json!({"code": "bad_request", "details": {"reason": "args_not_object"}}),
        ),
    ];
    for (request, expected) in &rejections {
        instance.rejected(request, expected);
    }
    // 被拒的请求没有任何效果：实例照常服务，期望状态记录还在
    assert_eq!(
        instance.snapshot("{\"v\":1,\"op\":\"ping\"}\n")["state"],
        "serving"
    );
    let restore = home.path().join("run").join(format!("{port}.restore.json"));
    assert!(restore.exists());

    // ---- 带条件的 shutdown：不够闲就不停，并说出实际闲置了多久 ----
    let mut reply =
        instance.ask("{\"v\":1,\"op\":\"shutdown\",\"args\":{\"if_idle_secs\":3600}}\n");
    assert!(reply["error"]["details"]["idle_ms"].is_u64(), "{reply}");
    let error = reply["error"].as_object_mut().unwrap();
    error.shift_remove("message");
    error.insert("details".into(), json!({"idle_ms": 0}));
    assert_eq!(
        reply,
        json!({"v": 1, "ok": false, "error": {"code": "condition_not_met", "details": {"idle_ms": 0}}})
    );
    assert!(restore.exists(), "没停成的 shutdown 不删期望状态记录");

    // ---- prepare_swap：回同一份快照，状态变成 swap_prepared，之后的 ping 也看得到 ----
    let swapped =
        instance.snapshot("{\"v\":1,\"op\":\"prepare_swap\",\"args\":{\"ttl_secs\":30}}\n");
    assert_covers(&swapped, &snapshot_sample(), "result");
    assert_eq!(swapped["state"], "swap_prepared");
    assert_eq!(
        instance.snapshot("{\"v\":1,\"op\":\"ping\"}\n")["state"],
        "swap_prepared"
    );

    // ---- shutdown 带 keep_restore：实例退出，期望状态记录留着 ----
    let stopping =
        instance.snapshot("{\"v\":1,\"op\":\"shutdown\",\"args\":{\"keep_restore\":true}}\n");
    assert_covers(&stopping, &snapshot_sample(), "result");
    assert_eq!(stopping["state"], "stopping");
    assert!(
        wait_for(Duration::from_secs(15), || instance.gone()),
        "收到 shutdown 的实例应退出"
    );
    assert!(
        restore.exists(),
        "keep_restore 的 shutdown 不删期望状态记录"
    );
    assert!(
        wait_for(Duration::from_secs(5), || {
            !home
                .path()
                .join("run")
                .join(format!("{port}.instance.json"))
                .exists()
        }),
        "正常退出的实例删掉自己的实例记录"
    );

    // 清点里它成了「没在运行、等着恢复」的一条
    let listing = instance
        .rt
        .block_on(aproxy::ops::enumerate::enumerate_in(home.path()));
    let listing = serde_json::to_value(&listing).unwrap();
    assert_eq!(
        listing["instances"],
        json!([{"port": port, "liveness": "down", "instance": null, "restore_pending": true}])
    );
}

/// 没有配置文件的实例：快照与实例记录的 `config_path` 是 null。不带参数的 shutdown 让
/// 实例退出并删掉期望状态记录。
#[test]
fn instance_without_a_config_file_reports_null_config_path() {
    let home = TestHome::new();
    home.settings(
        r#"{"watchdog": {"enabled": false},
            "defaults": {"base_url": "https://defaults-only.example.com"}}"#,
    );
    let port = free_port();
    let instance = start(
        &home,
        port,
        &["start", "--listen-addr", &format!("127.0.0.1:{port}")],
    );

    let snapshot = instance.snapshot("{\"v\":1,\"op\":\"ping\"}\n");
    assert!(snapshot["instance"]["config_path"].is_null(), "{snapshot}");
    assert!(snapshot["instance"]["log_path"].is_string(), "{snapshot}");
    // 除了那个 null，其余与有配置文件的实例同形
    let mut sample = snapshot_sample();
    sample["instance"]["config_path"] = Value::Null;
    assert_covers(&snapshot, &sample, "result");
    assert!(record_on_disk(home.path(), &format!("{port}.instance.json"))["config_path"].is_null());

    let restore = home.path().join("run").join(format!("{port}.restore.json"));
    assert!(restore.exists());
    let stopping = instance.snapshot("{\"v\":1,\"op\":\"shutdown\"}\n");
    assert_eq!(stopping["state"], "stopping");
    assert!(
        wait_for(Duration::from_secs(15), || instance.gone()),
        "收到 shutdown 的实例应退出"
    );
    assert!(
        !restore.exists(),
        "不带 keep_restore 的 shutdown 是「用户要它停下」：记录删掉"
    );
}

/// 盘上的记录与只读清点：实例记录、期望状态记录、看护者记录，以及清点结果的 JSON。
#[test]
fn records_on_disk_and_the_enumeration_match_the_samples() {
    let home = TestHome::new();
    home.enable_watchdog();
    let port = free_port();
    let cfg = home.path().join("records.toml");
    std::fs::write(
        &cfg,
        format!(
            "base_url = \"https://contract-test.example.com\"\nlisten_addr = \"127.0.0.1:{port}\"\n"
        ),
    )
    .unwrap();
    let instance = start(&home, port, &["start", "--config", cfg.to_str().unwrap()]);

    // 实例记录
    let record = record_on_disk(home.path(), &format!("{port}.instance.json"));
    let mut sample = snapshot_sample()["instance"].clone();
    sample
        .as_object_mut()
        .unwrap()
        .insert("schema".into(), json!(1));
    assert_covers(&record, &sample, "instance.json");
    assert_eq!(record["schema"], 1);

    // 期望状态记录：拉起规格外面包一层
    let restore = record_on_disk(home.path(), &format!("{port}.restore.json"));
    assert_covers(
        &restore,
        &json!({
            "schema": 1,
            "spec": {
                "schema": 1,
                "config_path": "/home/u/.aproxy/config.toml",
                "overrides": {},
                "pinned_port": null,
                "env": {},
                "log_path": "/home/u/.aproxy/logs/19ac3f2e8b5d-1a2b.log"
            },
            "written_at": 1_760_190_000_123_u64
        }),
        "restore.json",
    );
    assert_eq!(restore["schema"], 1);
    assert_eq!(
        restore["spec"]["log_path"], record["log_path"],
        "实例写日志的地方就是规格里记的那个"
    );
    // 环境快照：每个记下的变量要么是字符串，要么是 null（启动者没设）
    let env = restore["spec"]["env"].as_object().unwrap();
    assert!(
        env.contains_key("PATH") && env.contains_key("HTTPS_PROXY"),
        "{env:?}"
    );
    assert!(
        env.values()
            .all(|value| value.is_string() || value.is_null())
    );

    // 看护者记录：start 拉起了看护者，它接管之后写下记录
    assert!(
        wait_for(Duration::from_secs(20), || {
            home.path().join("run").join("watchdog.json").exists()
        }),
        "开着看护者开关时 start 应拉起看护者"
    );
    let watchdog = record_on_disk(home.path(), "watchdog.json");
    assert_covers(
        &watchdog,
        &json!({
            "schema": 1,
            "pid": 5555,
            "start": "w:133712345000000000",
            "version": "0.2.0",
            "heartbeat_secs": 30,
            "created_at": 1_760_190_000_000_u64,
            "last_scan_at": 1_760_190_123_456_u64,
            "restarts": []
        }),
        "watchdog.json",
    );
    assert_eq!(watchdog["schema"], 1);

    // 只读清点
    let listing = instance
        .rt
        .block_on(aproxy::ops::enumerate::enumerate_in(home.path()));
    let listing = serde_json::to_value(&listing).unwrap();
    let mut live = snapshot_sample();
    let live = live.as_object_mut().unwrap();
    let instance_sample = live.shift_remove("instance").unwrap();
    assert_covers(
        &listing,
        &json!({
            "instances": [{
                "port": 12346,
                "liveness": "live",
                "instance": instance_sample,
                "status": Value::Object(live.clone()),
                "restore_pending": false
            }],
            "unreadable": [],
            "watchdog": {
                "pid": 5555,
                "start": "w:133712345000000000",
                "version": "0.2.0",
                "state": "running",
                "last_scan_at": 1_760_190_123_456_u64,
                "restarts": []
            },
            "processes": []
        }),
        "enumeration",
    );
    assert_eq!(
        listing["instances"].as_array().unwrap().len(),
        1,
        "{listing}"
    );
    assert_eq!(listing["instances"][0]["port"], port);
    assert_eq!(listing["instances"][0]["liveness"], "live");
    assert_eq!(listing["instances"][0]["instance"]["pid"], record["pid"]);
    assert_eq!(listing["watchdog"]["state"], "running");
    assert_eq!(listing["watchdog"]["pid"], watchdog["pid"]);
    assert_eq!(listing["unreadable"], json!([]));
    assert!(listing.get("gc_paused").is_none(), "{listing}");
    // 实例与看护者都已各自列出，「其余进程」里没有它们
    assert_eq!(listing["processes"], json!([]), "{listing}");
}

/// [`PINNED_IN_MODULES`] 里的每个单测都还在原处。它们逐字钉住的样本是契约的一部分：
/// 要挪动或改名，连同这张清单一起改；不能删。
#[test]
fn the_samples_pinned_in_the_modules_are_still_there() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for (file, test) in PINNED_IN_MODULES {
        let source = std::fs::read_to_string(root.join(file))
            .unwrap_or_else(|e| panic!("读不到 {file}：{e}"));
        assert!(
            source.contains(&format!("fn {test}()")),
            "{file} 里应有钉住契约样本的单测 {test}"
        );
    }
}

/// 通过运行中的端点验证命名选择、握手结果与失败时的实例身份；不经 CLI 解码器。
#[test]
fn format_restart_operation_contract() {
    let home = TestHome::new();
    let cfg = home.path().join("format.toml");
    let role = home.path().join("role");
    std::fs::write(&role, "1").unwrap();
    std::fs::write(&cfg,format!("base_url='{}'\nlisten_addr='127.0.0.1:0'\n[[format]]\nname='role'\ncommand={}\nargs=['role-version',{}]\nstages=['request']\nprocess='persistent'\nrequired=true",fixed_upstream("still serving"),json!(common::format_echo_path()),json!(role))).unwrap();
    let instance = start(&home, 0, &["start", cfg.to_str().unwrap()]);
    let ping = "{\"v\":1,\"op\":\"ping\"}\n";
    let before = instance.snapshot(ping);
    assert!(
        before["ops"]
            .as_array()
            .unwrap()
            .contains(&json!("restart_formats"))
    );
    let named = "{\"v\":1,\"op\":\"restart_formats\",\"args\":{\"names\":[\"role\"]}}\n";
    instance.rejected(
        "{\"v\":1,\"op\":\"restart_formats\",\"args\":{\"names\":[\"role\",\"missing\"]}}\n",
        &json!({"code":"unknown_format","details":{"name":"missing"}}),
    );
    assert!(instance.snapshot(ping)["formats"][0]["restarted_at"].is_null());
    std::fs::write(&role, "2").unwrap();
    std::fs::write(&cfg, "invalid config!").unwrap();
    assert_eq!(
        instance.snapshot(named),
        json!({"configReloaded":false,"entries":[{"name":"role","program":"format-echo","version":"2","outcome":"restarted"}]})
    );
    let serving = instance.snapshot(ping)["formats"].clone();
    std::fs::remove_file(role).unwrap();
    assert_eq!(
        instance.snapshot(named),
        json!({"configReloaded":false,"entries":[{"name":"role","outcome":"handshake-failed","reason":"invalid_output"}]})
    );
    let after = instance.snapshot(ping);
    assert_eq!(after["instance"], before["instance"]);
    assert_eq!(after["state"], "serving");
    assert_eq!(after["formats"], serving);
    assert!(post_through(instance.port, "/test", "request").ends_with("still serving"));
}

/// 模拟只公布 ping 的旧实例。第二条连接已经绑定对端，但不能收到任何更改状态的帧。
#[tokio::test]
async fn older_instance_never_receives_format_restart() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let home = TestHome::new();
    let port = free_port();
    let dir = home.path().join("run");
    std::fs::create_dir_all(&dir).unwrap();
    let endpoint = aproxy::daemon::endpoint_for_in(&dir, &port.to_string());
    #[cfg(windows)]
    let mut listener = tokio::net::windows::named_pipe::ServerOptions::new()
        .first_pipe_instance(true)
        .create(&endpoint)
        .unwrap();
    #[cfg(unix)]
    let listener = tokio::net::UnixListener::bind(&endpoint).unwrap();
    let mut snapshot = snapshot_sample();
    snapshot["instance"]["pid"] = json!(std::process::id());
    snapshot["instance"]["listen_addr"] = json!(format!("127.0.0.1:{port}"));
    let reply = json!({"v":1,"ok":true,"result":snapshot}).to_string() + "\n";
    let server = tokio::spawn(async move {
        let mut tasks = tokio::task::JoinSet::new();
        for _ in 0..3 {
            #[cfg(windows)]
            let pipe = {
                listener.connect().await.unwrap();
                std::mem::replace(
                    &mut listener,
                    tokio::net::windows::named_pipe::ServerOptions::new()
                        .create(&endpoint)
                        .unwrap(),
                )
            };
            #[cfg(unix)]
            let pipe = listener.accept().await.unwrap().0;
            let reply = reply.clone();
            tasks.spawn(async move {
                let (r, mut w) = tokio::io::split(pipe);
                let mut line = String::new();
                BufReader::new(r).read_line(&mut line).await.unwrap();
                if !line.is_empty() {
                    assert_eq!(line, "{\"v\":1,\"op\":\"ping\"}\n");
                    w.write_all(reply.as_bytes()).await.unwrap();
                }
                line.is_empty()
            });
        }
        let mut empty = 0;
        while let Some(r) = tasks.join_next().await {
            empty += usize::from(r.unwrap());
        }
        assert_eq!(empty, 1);
    });
    let out = tokio::task::spawn_blocking(move || {
        home.cmd()
            .args(["restart", &port.to_string(), "--format", "all", "--json"])
            .output()
            .report_exit()
            .unwrap()
    })
    .await
    .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let doc = common::output_document(&out);
    assert_eq!(
        doc["results"][0]["error"]["details"]["reason"],
        "unknown_op"
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("does not support restarting formats"));
    server.await.unwrap();
}

/// 两个条目共享可执行文件但只控制所选条目；大参数可占满自身的额度，回信须逐个字符串替换已知密钥。
#[test]
fn format_control_operation_contract() {
    let home = TestHome::new();
    let cfg = home.path().join("control.toml");
    let key = "test-key-control-abcdefg";
    std::fs::write(&cfg,format!("base_url='http://127.0.0.1'\napi_key='{key}'\nlisten_addr='127.0.0.1:0'\n[[format]]\nname='a'\ncommand={0}\nstages=['request']\n[[format]]\nname='b'\ncommand={0}\nargs=['control-role']\nenv={{ECHO_SECRET='{key}'}}\nstages=['request']",json!(common::format_echo_path()))).unwrap();
    let instance = start(&home, 0, &["start", cfg.to_str().unwrap()]);
    let call = |name: &str, method: &str, params: Value| {
        json!({"v":1,"op":"format_control","args":{"name":name,"method":method,"params":params}})
            .to_string()
            + "\n"
    };
    let snapshot = instance.snapshot("{\"v\":1,\"op\":\"ping\"}\n");
    assert!(
        snapshot["ops"]
            .as_array()
            .unwrap()
            .contains(&json!("format_control"))
    );
    assert_eq!(snapshot["formats"][0]["control"], false);
    assert_eq!(snapshot["formats"][1]["control"], true);
    assert_eq!(snapshot["formats"][1]["program"], "format-echo");
    instance.rejected(
        &call("a", "ping", Value::Null),
        &json!({"code":"format_control_error","details":{"reason":"no-control"}}),
    );
    instance.rejected(
        &call("missing", "ping", Value::Null),
        &json!({"code":"unknown_format","details":{"name":"missing"}}),
    );
    assert_eq!(
        instance.snapshot(&call("b", "received", json!(key))),
        json!({"result":true})
    );
    let r = instance.snapshot(&call("b", "echo", json!({key:[key,null]})));
    assert!(!r.to_string().contains(key));
    let masked = aproxy_ipc::redact::Secrets::new([key]).replace(key);
    assert_eq!(r, json!({"result":{masked.clone():[masked,null]}}));
    assert_eq!(
        instance.snapshot(&call("b", "fail", Value::Null)),
        json!({"fail":{"code":"refused","reason":"author declined"}})
    );
    let big = json!("x".repeat(65534));
    assert_eq!(
        instance.snapshot(&call("b", "echo", big.clone())),
        json!({"result":big})
    );
    instance.rejected(
        &call("b", "echo", json!("x".repeat(65535))),
        &json!({"code":"format_control_error","details":{"reason":"too-large"}}),
    );
}

/// 展示名和配置名都可选择，但两者发生碰撞时整个操作失败，不能把控制帧发给另一行。
#[test]
fn format_selectors_resolve_exactly_one_entry() {
    let home = TestHome::new();
    let cfg = home.path().join("select.toml");
    let mut config =
        "base_url='http://127.0.0.1'\nlisten_addr='127.0.0.1:0'\napi_key='profile-east'\n"
            .to_owned();
    for name in ["profile-east", "<secret>", "only-profile-east"] {
        config.push_str(&format!(
            "[[format]]\nname='{name}'\ncommand={}\nargs=['control-role']\nstages=['request']\n",
            json!(common::format_echo_path())
        ));
    }
    std::fs::write(&cfg, config).unwrap();
    let instance = start(&home, 0, &["start", cfg.to_str().unwrap()]);
    let ping = "{\"v\":1,\"op\":\"ping\"}\n";
    let before = instance.snapshot(ping)["formats"].clone();
    assert_eq!(before[0]["name"], "<secret>");
    assert_eq!(before[1]["name"], "<secret>");
    let call = |name: &str| {
        json!({"v":1,"op":"format_control","args":{"name":name,"method":"pid","params":null}})
            .to_string()
            + "\n"
    };
    let restart = |names: Value| {
        json!({"v":1,"op":"restart_formats","args":{"names":names}}).to_string() + "\n"
    };
    let rejection = json!({"code":"unknown_format","details":{"name":"<secret>"}});
    instance.rejected(&call("<secret>"), &rejection);
    instance.rejected(&restart(json!(["only-<secret>", "<secret>"])), &rejection);
    assert_eq!(instance.snapshot(ping)["formats"], before);
    assert!(instance.snapshot(&call("profile-east"))["result"].is_u64());
    assert_eq!(
        instance.snapshot(&call("only-<secret>")),
        instance.snapshot(&call("only-profile-east"))
    );
    let report = instance.snapshot(&restart(json!(["only-<secret>", "only-profile-east"])));
    assert_eq!(
        report["entries"],
        json!([{"name":"only-<secret>","outcome":"restarted","program":"format-echo","version":"1.0"}])
    );
}

/// 请求阻塞于 A 时切换 B；B 的下一条请求消息和完整上传摘要都交给成功握手的新一代。
#[test]
fn request_crosses_format_cutover_with_complete_digest() {
    let h = TestHome::new();
    let cfg = h.path().join("chain.toml");
    let gate = h.path().join("release");
    let digest = h.path().join("digest.enabled");
    let a = h.path().join("a.events");
    let b = h.path().join("b.events");
    let frames = h.path().join("b.frames");
    std::fs::write(&cfg,format!("base_url='{}'\nlisten_addr='127.0.0.1:0'\n[[format]]\nname='a'\ncommand={exe}\nargs=['gate-request',{}]\nstages=['request']\nprocess='persistent'\nenv={{ECHO_OBSERVE={}}}\n[[format]]\nname='b'\ncommand={exe}\nargs=['digest-on-restart',{}]\nstages=['request','notify']\nrequired=true\nprocess='persistent'\nenv={{ECHO_OBSERVE={},ECHO_RECORD={}}}", fixed_upstream("done"),json!(gate),json!(a),json!(digest),json!(b),json!(frames),exe=json!(common::format_echo_path()))).unwrap();
    let instance = start(&h, 0, &["start", cfg.to_str().unwrap()]);
    let port = instance.port;
    let request = std::thread::spawn(move || post_through(port, "/test", "blocked"));
    let read = |p: &Path| {
        std::fs::read_to_string(p)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .collect::<Vec<_>>()
    };
    assert!(wait_for(Duration::from_secs(20), || read(&a)
        .iter()
        .any(|e| e["event"] == "request")));
    std::fs::write(digest, b"").unwrap();
    let outcome =
        instance.snapshot("{\"v\":1,\"op\":\"restart_formats\",\"args\":{\"names\":[\"b\"]}}\n");
    assert_eq!(outcome["entries"][0]["version"], "digest");
    std::fs::write(gate, b"").unwrap();
    let response = request.join().unwrap();
    assert!(
        response.starts_with("HTTP/1.1 200") && response.ends_with("done"),
        "{response}"
    );
    assert!(wait_for(Duration::from_secs(20), || read(&frames)
        .iter()
        .any(|f| f["stage"] == "notify")));
    let activity = instance.snapshot("{\"v\":1,\"op\":\"ping\"}\n")["activity"].clone();
    assert_eq!(activity["requests_total"], 1);
    assert_eq!(activity["retries_total"], 0);
    let events = read(&b);
    let pids: Vec<_> = events
        .iter()
        .filter(|e| e["event"] == "hello")
        .map(|e| &e["pid"])
        .collect();
    assert_eq!(pids.len(), 2);
    assert_ne!(pids[0], pids[1]);
    assert_eq!(
        events.iter().find(|e| e["event"] == "request").unwrap()["pid"],
        *pids[1]
    );
    let notify = read(&frames)
        .into_iter()
        .find(|f| f["stage"] == "notify")
        .unwrap();
    assert_eq!(
        notify["digest"],
        "6973dddd3ef9cb6a2932702f31777faad9c9bf3124d147a84f31aadb6d139546"
    );
}
