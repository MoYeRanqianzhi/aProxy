//! 请求历史，经真实的代理路径：请求从客户端进来、发往模拟上游、响应交回客户端之后，历史里
//! 记下的记录、头、体与结局。
//!
//! 每个测试在进程内起两个服务：模拟上游，以及指向它的代理路由。代理的 `AppState` 留在手里，
//! 断言直接读它的 `history`——读出来的 JSON 就是控制通道 `requests`、`request`、`body` 三个
//! op 的 `result`（那一层的线上形状由 `src/daemon.rs` 的单测与 `tests/contract_v0_2.rs` 钉住）。
//! 不拉起 aproxy 进程，也不碰任何 home：落盘缓存的目录是测试自己的临时目录。

use axum::{Router, http::StatusCode, response::IntoResponse, routing::any};
use bytes::Bytes;
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use aproxy::{config::Config, history::Part, proxy::AppState};

// 本机环境常带 HTTP_PROXY 之类的变量，会把发往 127.0.0.1 的测试流量送去外部代理。测试
// 客户端一律 no_proxy()；代理内部的上游客户端读环境变量构建，靠 NO_PROXY 排除环回地址，
// 必须在任何 AppState::new 之前设好。
static ENV_GUARD: std::sync::Once = std::sync::Once::new();

fn isolate_env_proxy() {
    ENV_GUARD.call_once(|| {
        // SAFETY: 本测试程序里只有这一处写环境变量，且早于各测试的主体
        unsafe {
            std::env::set_var("NO_PROXY", "127.0.0.1,localhost");
            std::env::set_var("no_proxy", "127.0.0.1,localhost");
        }
    });
}

fn client() -> reqwest::Client {
    isolate_env_proxy();
    reqwest::Client::builder().no_proxy().build().unwrap()
}

/// 把 `router` 绑到一个系统分配的环回端口上，返回它的地址。
async fn serve(router: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    url
}

/// 指向 `upstream` 的代理配置。落盘缓存放进 `spool`（测试自己的临时目录）。
fn config(upstream: &str, spool: &std::path::Path) -> Config {
    isolate_env_proxy();
    Config {
        base_url: upstream.to_string(),
        listen_addr: "127.0.0.1:0".to_string(),
        spool_dir_override: Some(spool.to_path_buf()),
        ..Default::default()
    }
    .normalized()
}

/// 起一个代理，返回它的地址与状态。
async fn proxy(config: Config) -> (String, AppState) {
    let state = AppState::new(config);
    let url = serve(aproxy::proxy::router(state.clone())).await;
    (url, state)
}

/// 历史里最新的 `limit` 条记录（`requests` 的 `result`）。
fn list(state: &AppState, limit: usize) -> Value {
    serde_json::from_str(&state.history.list_json(None, limit).unwrap()).unwrap()
}

/// 等到最新的那条记录结束，返回它。响应体交给客户端之后记录才收尾，比客户端读完响应晚
/// 一点点，所以要等。
async fn finished(state: &AppState) -> Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let record = list(state, 1)["requests"][0].clone();
        if record["phase"] == "done" {
            return record;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "记录迟迟没有结束：{record}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// 一个请求的明细（`request` 的 `result`）。
fn detail(state: &AppState, record: &Value) -> Value {
    let id = record["request_id"].as_str().unwrap();
    serde_json::from_str(&state.history.detail_json(id).unwrap()).unwrap()
}

/// 把一个体分块读完（`body` 的 `result` 逐块拼起来）。体还在解码时等它完成。
async fn body(state: &AppState, record: &Value, part: Part, attempt: u32) -> Vec<u8> {
    use base64::Engine as _;
    let id = record["request_id"].as_str().unwrap();
    let mut out = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let chunk: Value = serde_json::from_str(
            &state
                .history
                .read_body_json(id, part, attempt, out.len() as u64, 64 * 1024)
                .unwrap_or_else(|e| panic!("读体失败：{e:?}")),
        )
        .unwrap();
        out.extend(
            base64::engine::general_purpose::STANDARD
                .decode(chunk["data_b64"].as_str().unwrap())
                .unwrap(),
        );
        if chunk["more"] == true {
            continue;
        }
        if chunk["complete"] == true {
            return out;
        }
        assert!(tokio::time::Instant::now() < deadline, "体迟迟没有收完");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// 头表里名为 `name` 的那一项的值。
fn header<'a>(headers: &'a Value, name: &str) -> &'a Value {
    headers
        .as_array()
        .unwrap()
        .iter()
        .find(|h| h["name"] == name)
        .map(|h| &h["value"])
        .unwrap_or(&Value::Null)
}

/// 一次就成功的请求：记录的各项、请求头与发出的头（密钥打码）、三个体都读得回来，体里的
/// 已知密钥已经换掉。
#[tokio::test]
async fn a_successful_request_is_recorded_with_its_bodies() {
    const KEY: &str = "sk-live-0123456789abcdefghij";
    let upstream = Router::new().route(
        "/v1/messages",
        any(|body: Bytes| async move {
            // 把请求体原样回给客户端：响应体里因此也带着那个密钥
            (
                StatusCode::OK,
                [("content-type", "application/json"), ("x-upstream", "yes")],
                body,
            )
        }),
    );
    let upstream_url = serve(upstream).await;
    let spool = tempfile::tempdir().unwrap();
    let mut cfg = config(&upstream_url, spool.path());
    cfg.api_key = Some(KEY.to_string());
    let (proxy_url, state) = proxy(cfg).await;

    let sent = format!(r#"{{"model":"claude-test","stream":false,"note":"my key is {KEY}"}}"#);
    let response = client()
        .post(format!("{proxy_url}/v1/messages?beta=tools"))
        .header("user-agent", "history-test/1.0")
        .header("x-api-key", "client-side-placeholder-key")
        .header("content-type", "application/json")
        .body(sent.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.text().await.unwrap(),
        sent,
        "转发的字节原样，不打码"
    );

    let record = finished(&state).await;
    assert_eq!(record["request_seq"], 1);
    assert_eq!(
        record["request_id"],
        format!("{}-1", state.instance_id).as_str()
    );
    assert_eq!(record["method"], "POST");
    assert_eq!(record["path"], "/v1/messages?beta=***");
    assert_eq!(record["model"], "claude-test");
    assert_eq!(record["stream"], false);
    assert_eq!(record["user_agent"], "history-test/1.0");
    assert_eq!(record["delivery"], "retry");
    assert_eq!(record["outcome"], "ok");
    assert_eq!(record["code"], Value::Null);
    assert_eq!(record["client_status"], 200);
    assert_eq!(record["attempts"], 1);
    assert_eq!(record["keepalive"], false);
    assert_eq!(record["request_bytes"], sent.len());
    assert_eq!(record["response_bytes"], sent.len());
    assert!(record["ended_at"].as_u64() >= record["started_at"].as_u64());
    let first = &record["tries"][0];
    assert_eq!(first["attempt"], 1);
    assert_eq!(first["upstream_status"], 200);
    assert_eq!(first["code"], Value::Null);
    assert_eq!(first["decision"], Value::Null);
    assert_eq!(first["response_bytes"], sent.len());
    assert!(first["duration_ms"].as_u64().is_some());

    let detail = detail(&state, &record);
    assert_eq!(detail["record"], record);
    // 客户端发来的头：原样的名字，密钥头给占位对象
    let request_headers = &detail["request"]["headers"];
    assert_eq!(
        header(request_headers, "x-api-key"),
        &json!({"secret": true, "hint": "clie…-key"})
    );
    assert_eq!(header(request_headers, "content-type"), "application/json");
    // 发往上游的头：api_key 覆盖之后的，同样是占位
    let sent_head = &detail["tries"][0]["sent"];
    assert_eq!(sent_head["method"], "POST");
    assert_eq!(
        sent_head["url"],
        format!("{upstream_url}/v1/messages?beta=***").as_str()
    );
    assert_eq!(
        header(&sent_head["headers"], "x-api-key"),
        &json!({"secret": true, "hint": "sk-l…ghij"})
    );
    assert_eq!(
        header(&sent_head["headers"], "authorization"),
        &json!({"secret": true, "hint": "Bear…ghij"})
    );
    let response_head = &detail["tries"][0]["response"];
    assert_eq!(response_head["upstream_status"], 200);
    assert_eq!(header(&response_head["headers"], "x-upstream"), "yes");
    assert_eq!(detail["tries"][0]["error"], Value::Null);
    assert!(!detail.to_string().contains(KEY), "明细里没有密钥原文");

    // 体：入库的是换掉密钥之后的；发出的体与请求体是同一份
    let redacted = sent.replace(KEY, "<secret>");
    assert_eq!(
        detail["request"]["body"],
        json!({"bytes": redacted.len(), "state": "retained"})
    );
    assert_eq!(
        sent_head["body"],
        json!({"bytes": redacted.len(), "state": "same_as_previous"})
    );
    assert_eq!(
        response_head["body"],
        json!({"bytes": redacted.len(), "state": "retained"})
    );
    for (part, attempt) in [(Part::Request, 0), (Part::Sent, 1), (Part::Response, 1)] {
        assert_eq!(
            String::from_utf8(body(&state, &record, part, attempt).await).unwrap(),
            redacted,
            "{part:?}"
        );
    }
}

/// 重试之后成功：每次尝试各有一条，失败的带码与「再试」的决定，各自的响应体都留着。
#[tokio::test]
async fn retried_attempts_are_recorded_one_by_one() {
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let upstream = Router::new().route(
        "/v1/chat",
        any(move || {
            let counter = counter.clone();
            async move {
                match counter.fetch_add(1, Ordering::SeqCst) {
                    0 => (StatusCode::SERVICE_UNAVAILABLE, "first failure").into_response(),
                    1 => (StatusCode::TOO_MANY_REQUESTS, "second failure").into_response(),
                    _ => (StatusCode::OK, "third time lucky").into_response(),
                }
            }
        }),
    );
    let upstream_url = serve(upstream).await;
    let spool = tempfile::tempdir().unwrap();
    let mut cfg = config(&upstream_url, spool.path());
    // 一条只管 429 的规则：第二次失败的决定该记成它
    cfg.retry_rules = vec![aproxy::config::RetryRule {
        path: None,
        status: Some(vec![429]),
        max_attempts: 0,
    }];
    let (proxy_url, state) = proxy(cfg).await;

    let response = client()
        .post(format!("{proxy_url}/v1/chat"))
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(response.text().await.unwrap(), "third time lucky");

    let record = finished(&state).await;
    assert_eq!(record["outcome"], "ok");
    assert_eq!(record["attempts"], 3);
    assert_eq!(record["tries_omitted"], 0);
    let tries = record["tries"].as_array().unwrap();
    assert_eq!(tries.len(), 3);
    assert_eq!(
        (&tries[0]["upstream_status"], &tries[0]["code"]),
        (&json!(503), &json!("upstream_status"))
    );
    assert_eq!(
        tries[0]["decision"],
        json!({"by": "default", "decision": "retry", "delay_ms": 0})
    );
    assert_eq!(tries[0]["response_bytes"], "first failure".len());
    assert_eq!(
        tries[1]["decision"],
        json!({"by": "rule:1", "decision": "retry", "delay_ms": 0})
    );
    assert_eq!(tries[2]["code"], Value::Null);
    assert_eq!(tries[2]["decision"], Value::Null);
    assert_eq!(record["response_bytes"], "third time lucky".len());

    assert_eq!(
        body(&state, &record, Part::Response, 1).await,
        b"first failure"
    );
    assert_eq!(
        body(&state, &record, Part::Response, 2).await,
        b"second failure"
    );
    assert_eq!(
        body(&state, &record, Part::Response, 3).await,
        b"third time lucky"
    );
    // 三次发的是同一个体：只存一份
    let detail = detail(&state, &record);
    for index in 0..3 {
        assert_eq!(
            detail["tries"][index]["sent"]["body"]["state"],
            "same_as_previous"
        );
    }
    assert_eq!(state.history.body_bytes(), 2 + 13 + 14 + 16);
}

/// 重试规则给的次数用完：客户端拿到上游的失败响应原样，记录的结局是「上游错误」，码是
/// `retry_exhausted`，最后一次尝试的决定是「停」。
#[tokio::test]
async fn an_exhausted_request_ends_as_an_upstream_error() {
    let upstream = Router::new().route(
        "/v1/compact",
        any(|| async { (StatusCode::NOT_FOUND, "no such route") }),
    );
    let upstream_url = serve(upstream).await;
    let spool = tempfile::tempdir().unwrap();
    let mut cfg = config(&upstream_url, spool.path());
    cfg.retry_rules = vec![aproxy::config::RetryRule {
        path: Some("/v1/compact".to_string()),
        status: None,
        max_attempts: 2,
    }];
    let (proxy_url, state) = proxy(cfg).await;

    let response = client()
        .post(format!("{proxy_url}/v1/compact"))
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 404);
    assert_eq!(response.text().await.unwrap(), "no such route");

    let record = finished(&state).await;
    assert_eq!(record["outcome"], "upstream_error");
    assert_eq!(record["code"], "retry_exhausted");
    assert_eq!(record["client_status"], 404);
    assert_eq!(record["attempts"], 2);
    assert_eq!(record["response_bytes"], "no such route".len());
    assert_eq!(
        record["tries"][0]["decision"],
        json!({"by": "rule:1", "decision": "retry", "delay_ms": 0})
    );
    assert_eq!(
        record["tries"][1]["decision"],
        json!({"by": "rule:1", "decision": "stop"})
    );
}

/// 连不上上游的尝试没有响应：明细里 `response` 为 null，`error` 给出码、数据与说明。
/// 客户端等不下去断开之后，记录的结局是「客户端先走了」，尝试的条数有界。
#[tokio::test]
async fn failed_connections_and_a_departed_client_are_recorded() {
    // 一个没有人在听的端口：绑定之后立刻放开
    let dead = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", listener.local_addr().unwrap())
    };
    let spool = tempfile::tempdir().unwrap();
    let mut cfg = config(&dead, spool.path());
    cfg.max_retry_backoff_secs = 0;
    let (proxy_url, state) = proxy(cfg).await;

    let request = client()
        .post(format!("{proxy_url}/v1/messages"))
        .body(r#"{"model":"m"}"#)
        .send();
    // 等它至少失败过两次，然后客户端放弃。不按固定的时长等：连接被拒在 Windows 上要约
    // 2 秒才报出来，在别的系统上是立刻
    let retried_twice = async {
        loop {
            tokio::time::sleep(Duration::from_millis(20)).await;
            if list(&state, 1)["requests"][0]["attempts"]
                .as_u64()
                .is_some_and(|attempts| attempts >= 3)
            {
                return;
            }
        }
    };
    tokio::select! {
        _ = request => panic!("上游连不上时请求应一直在重试，不该有响应"),
        _ = retried_twice => {}
        _ = tokio::time::sleep(Duration::from_secs(30)) => panic!("30 秒内没有重试到第三次"),
    }

    let record = finished(&state).await;
    assert_eq!(record["outcome"], "client_gone");
    assert_eq!(record["code"], Value::Null);
    assert_eq!(record["client_status"], Value::Null);
    let attempts = record["attempts"].as_u64().unwrap();
    assert!(attempts >= 2, "{record}");
    let tries = record["tries"].as_array().unwrap();
    assert!(tries.len() <= 23, "尝试只留前 3 次与最近 20 次");
    assert_eq!(
        tries.len() as u64 + record["tries_omitted"].as_u64().unwrap(),
        attempts
    );
    assert_eq!(tries[0]["code"], "upstream_connect_failed");
    assert_eq!(tries[0]["upstream_status"], Value::Null);
    assert_eq!(tries[0]["decision"]["by"], "default");
    assert!(record.to_string().len() <= 64 * 1024);

    let detail = detail(&state, &record);
    assert_eq!(detail["tries"][0]["response"], Value::Null);
    let error = &detail["tries"][0]["error"];
    assert_eq!(error["code"], "upstream_connect_failed");
    assert!(error["details"]["reason"].is_string());
    assert!(error["message"].as_str().unwrap().contains("connect"));
    // 发出的体每次都一样：只存了客户端的那一份
    assert_eq!(state.history.body_bytes(), r#"{"model":"m"}"#.len() as u64);
}

/// aproxy 自己结束的请求：结局是 `aproxy_error`，码是终态码。
#[tokio::test]
async fn a_request_ended_by_aproxy_is_recorded_with_its_code() {
    let spool = tempfile::tempdir().unwrap();
    let mut cfg = config("http://127.0.0.1:9", spool.path());
    cfg.max_body_mb = Some(1);
    let (proxy_url, state) = proxy(cfg).await;

    let response = client()
        .post(format!("{proxy_url}/v1/messages"))
        .body(vec![b'x'; 2 * 1024 * 1024])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 413);
    let _ = response.bytes().await;

    let record = finished(&state).await;
    assert_eq!(record["outcome"], "aproxy_error");
    assert_eq!(record["code"], "request_too_large");
    assert_eq!(record["client_status"], 413);
    assert_eq!(record["attempts"], 0);
    assert_eq!(record["tries"], json!([]));
    // 被入站校验拒掉的请求不进历史
    let rejected = client()
        .get(format!("{proxy_url}/v1/models"))
        .header("origin", "https://evil.example")
        .send()
        .await
        .unwrap();
    assert_eq!(rejected.status(), 403);
    assert_eq!(list(&state, 10)["requests"].as_array().unwrap().len(), 1);
}

/// 保活通道：客户端要流的请求记 `stream` 与 `keepalive`，重试期间发出的心跳计数，写给
/// 客户端的字节数不含心跳。
#[tokio::test]
async fn a_keepalive_request_counts_heartbeats_but_not_their_bytes() {
    const EVENT: &str = "data: {\"type\":\"message_stop\"}\n\n";
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let upstream = Router::new().route(
        "/v1/messages",
        any(move || {
            let counter = counter.clone();
            async move {
                if counter.fetch_add(1, Ordering::SeqCst) == 0 {
                    // 第一次拖过一个保活间隔再失败：骨架头与心跳已经发给客户端
                    tokio::time::sleep(Duration::from_millis(2500)).await;
                    (StatusCode::BAD_GATEWAY, "bad gateway").into_response()
                } else {
                    (
                        StatusCode::OK,
                        [("content-type", "text/event-stream")],
                        EVENT,
                    )
                        .into_response()
                }
            }
        }),
    );
    let upstream_url = serve(upstream).await;
    let spool = tempfile::tempdir().unwrap();
    let mut cfg = config(&upstream_url, spool.path());
    cfg.keepalive_interval_secs = 1;
    let (proxy_url, state) = proxy(cfg).await;

    let response = client()
        .post(format!("{proxy_url}/v1/messages"))
        .body(r#"{"model":"claude-stream","stream":true}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let text = response.text().await.unwrap();
    assert!(text.ends_with(EVENT), "{text:?}");
    let heartbeats = text.matches(": keepalive").count();
    assert!(heartbeats >= 2, "{text:?}");

    let record = finished(&state).await;
    assert_eq!(record["outcome"], "ok");
    assert_eq!(record["stream"], true);
    assert_eq!(record["keepalive"], true);
    assert_eq!(record["model"], "claude-stream");
    assert_eq!(record["heartbeats"], heartbeats);
    assert_eq!(record["response_bytes"], EVENT.len());
    assert_eq!(record["attempts"], 2);
    assert_eq!(record["client_status"], 200);
}

/// 上游回的压缩体：转发给客户端的是原样的压缩字节，历史里存的是解码之后的，并记下原来的
/// 编码。
#[tokio::test]
async fn compressed_responses_are_stored_decoded() {
    use std::io::Write as _;
    let plain = format!(r#"{{"text":"{}"}}"#, "compressible ".repeat(500));
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(plain.as_bytes()).unwrap();
    let packed = Bytes::from(encoder.finish().unwrap());
    let served = packed.clone();
    let upstream = Router::new().route(
        "/v1/messages",
        any(move || {
            let served = served.clone();
            async move {
                (
                    StatusCode::OK,
                    [
                        ("content-type", "application/json"),
                        ("content-encoding", "gzip"),
                    ],
                    served,
                )
            }
        }),
    );
    let upstream_url = serve(upstream).await;
    let spool = tempfile::tempdir().unwrap();
    let (proxy_url, state) = proxy(config(&upstream_url, spool.path())).await;

    let response = client()
        .post(format!("{proxy_url}/v1/messages"))
        .header("accept-encoding", "gzip")
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(response.headers()["content-encoding"], "gzip");
    assert_eq!(
        response.bytes().await.unwrap(),
        packed,
        "客户端拿到上游原样的字节"
    );

    let record = finished(&state).await;
    assert_eq!(record["response_bytes"], packed.len(), "按线上的字节数");
    assert_eq!(record["tries"][0]["response_bytes"], packed.len());
    assert_eq!(
        String::from_utf8(body(&state, &record, Part::Response, 1).await).unwrap(),
        plain
    );
    let detail = detail(&state, &record);
    assert_eq!(
        detail["tries"][0]["response"]["body"],
        json!({"bytes": plain.len(), "state": "retained", "encoding": "gzip"})
    );
}

/// 溢写磁盘的大请求体（超过 1 MiB）照样整份进历史，逐字节相同。
#[tokio::test]
async fn a_request_body_spilled_to_disk_is_stored_whole() {
    // 模拟上游收得下 3 MiB 的请求体：axum 的提取器默认只收 2 MiB，超了回 413——那会被代理
    // 当成上游失败一直重试下去
    let upstream = Router::new()
        .route("/v1/messages", any(|_body: Bytes| async { "ok" }))
        .layer(axum::extract::DefaultBodyLimit::disable());
    let upstream_url = serve(upstream).await;
    let spool = tempfile::tempdir().unwrap();
    let (proxy_url, state) = proxy(config(&upstream_url, spool.path())).await;

    let mut sent = br#"{"model":"big-one","pad":""#.to_vec();
    sent.extend((0..3 * 1024 * 1024).map(|i| b'a' + (i % 23) as u8));
    sent.extend_from_slice(br#""}"#);
    let response = client()
        .post(format!("{proxy_url}/v1/messages"))
        .body(sent.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(response.text().await.unwrap(), "ok");

    let record = finished(&state).await;
    assert_eq!(record["model"], "big-one");
    assert_eq!(record["request_bytes"], sent.len());
    let stored = body(&state, &record, Part::Request, 0).await;
    assert_eq!(stored.len(), sent.len());
    assert!(stored == sent, "入库的请求体与客户端发来的逐字节相同");
    assert!(
        body(&state, &record, Part::Sent, 1).await == sent,
        "发出的体就是那一份"
    );
    assert_eq!(state.history.body_bytes(), sent.len() as u64 + 2);

    // `model` 排在溢写之前攒下的那一段之后：开头一段里读不到，要从落盘的文件里读出来
    let mut late = br#"{"pad":""#.to_vec();
    late.extend((0..3 * 1024 * 1024).map(|i| b'a' + (i % 23) as u8));
    late.extend_from_slice(br#"","model":"late-one"}"#);
    let response = client()
        .post(format!("{proxy_url}/v1/messages"))
        .body(late.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(response.text().await.unwrap(), "ok");
    let record = finished(&state).await;
    assert_eq!(record["request_bytes"], late.len());
    assert_eq!(record["model"], "late-one");
    assert!(body(&state, &record, Part::Request, 0).await == late);
}

/// 预算为 0：记录照记，体一个都不留；转发不受影响。
#[tokio::test]
async fn a_zero_budget_records_requests_without_bodies() {
    let upstream = Router::new().route("/v1/messages", any(|| async { "hello" }));
    let upstream_url = serve(upstream).await;
    let spool = tempfile::tempdir().unwrap();
    let mut cfg = config(&upstream_url, spool.path());
    cfg.history_body_budget_mb = 0;
    let (proxy_url, state) = proxy(cfg).await;

    let response = client()
        .post(format!("{proxy_url}/v1/messages"))
        .body(r#"{"model":"m"}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(response.text().await.unwrap(), "hello");

    let record = finished(&state).await;
    assert_eq!(record["outcome"], "ok");
    assert_eq!(record["model"], "m");
    assert_eq!(record["response_bytes"], 5);
    let detail = detail(&state, &record);
    let disabled = json!({"bytes": 0, "state": "not_retained", "cause": "budget_disabled"});
    assert_eq!(detail["request"]["body"], disabled);
    assert_eq!(detail["tries"][0]["response"]["body"], disabled);
    assert_eq!(state.history.body_bytes(), 0);
}

/// forward 请求：边转发边复制。成功的记 `ok`；上游回错误状态码时原样转给客户端，记录的
/// 结局是「上游错误」，码是 `upstream_failed`。
#[tokio::test]
async fn forwarded_requests_are_recorded_as_they_stream() {
    let upstream = Router::new()
        .route(
            "/v1/messages",
            any(|body: Bytes| async move {
                let chunks = vec![
                    Ok::<_, std::io::Error>(Bytes::from_static(b"data: one\n\n")),
                    Ok(Bytes::from(format!("data: got {} bytes\n\n", body.len()))),
                ];
                axum::response::Response::builder()
                    .header("content-type", "text/event-stream")
                    .body(axum::body::Body::from_stream(futures_util::stream::iter(
                        chunks,
                    )))
                    .unwrap()
            }),
        )
        .route(
            "/v1/broken",
            any(|| async { (StatusCode::SERVICE_UNAVAILABLE, "overloaded") }),
        );
    let upstream_url = serve(upstream).await;
    let spool = tempfile::tempdir().unwrap();
    let mut cfg = config(&upstream_url, spool.path());
    cfg.forward_only = Some(true);
    let (proxy_url, state) = proxy(cfg).await;

    let sent = r#"{"model":"forwarded","stream":true,"messages":[]}"#;
    let response = client()
        .post(format!("{proxy_url}/v1/messages"))
        .body(sent)
        .send()
        .await
        .unwrap();
    let text = response.text().await.unwrap();
    assert_eq!(
        text,
        format!("data: one\n\ndata: got {} bytes\n\n", sent.len())
    );

    let record = finished(&state).await;
    assert_eq!(record["delivery"], "forward");
    assert_eq!(record["outcome"], "ok");
    assert_eq!(record["model"], "forwarded");
    assert_eq!(record["stream"], true);
    assert_eq!(record["attempts"], 1);
    assert_eq!(record["request_bytes"], sent.len());
    assert_eq!(record["response_bytes"], text.len());
    assert_eq!(record["tries"][0]["upstream_status"], 200);
    assert_eq!(record["tries"][0]["response_bytes"], text.len());
    assert_eq!(
        body(&state, &record, Part::Request, 0).await,
        sent.as_bytes()
    );
    assert_eq!(body(&state, &record, Part::Sent, 1).await, sent.as_bytes());
    assert_eq!(
        body(&state, &record, Part::Response, 1).await,
        text.as_bytes()
    );

    let response = client()
        .post(format!("{proxy_url}/v1/broken"))
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 503);
    assert_eq!(response.text().await.unwrap(), "overloaded");
    let record = finished(&state).await;
    assert_eq!(record["request_seq"], 2);
    assert_eq!(record["outcome"], "upstream_error");
    assert_eq!(record["code"], "upstream_failed");
    assert_eq!(record["client_status"], 503);
    assert_eq!(record["tries"][0]["code"], "upstream_status");
    assert_eq!(
        body(&state, &record, Part::Response, 1).await,
        b"overloaded"
    );
}

/// 订阅者对每个请求先后收到「开始」与「结束」两个事件；长重试期间每条记录每秒最多一个。
#[tokio::test]
async fn subscribers_see_each_request_start_and_end() {
    use aproxy::history::Event;
    let upstream = Router::new().route(
        "/v1/messages",
        any(|| async { (StatusCode::INTERNAL_SERVER_ERROR, "always failing") }),
    );
    let upstream_url = serve(upstream).await;
    let spool = tempfile::tempdir().unwrap();
    let mut cfg = config(&upstream_url, spool.path());
    cfg.max_retry_backoff_secs = 0;
    let (proxy_url, state) = proxy(cfg).await;
    let mut subscription = state.history.subscribe().expect("第一个订阅者");
    assert_eq!(
        subscription.opening_requests(1024, 0),
        (String::new(), false)
    );

    // 一个一直失败、一直重试的请求，跑 2.6 秒后客户端放弃
    let started = std::time::Instant::now();
    let request = client()
        .post(format!("{proxy_url}/v1/messages"))
        .body("{}")
        .send();
    assert!(
        tokio::time::timeout(Duration::from_millis(2600), request)
            .await
            .is_err()
    );
    let record = finished(&state).await;
    let attempts = record["attempts"].as_u64().unwrap();
    assert!(attempts > 20, "这段时间里应当重试了很多次：{attempts}");

    let mut events = Vec::new();
    while let Ok(event) = subscription.events.try_recv() {
        match event {
            Event::Request { at, record } => {
                events.push((at, serde_json::from_str::<Value>(&record).unwrap()));
            }
            other => panic!("{other:?}"),
        }
    }
    let elapsed = started.elapsed().as_secs_f64();
    // 开始、结束各一个，中间每秒最多一个
    assert!(
        events.len() as f64 <= 2.0 + elapsed.ceil(),
        "{} 个事件 / {elapsed:.1} 秒 / {attempts} 次尝试",
        events.len()
    );
    assert!(events.len() >= 3, "{}", events.len());
    assert_eq!(events[0].1["phase"], "reading_body");
    assert_eq!(events[0].1["outcome"], "in_flight");
    let (_, last) = events.last().unwrap();
    assert_eq!(last["outcome"], "client_gone");
    assert_eq!(last, &record);
    for (_, event) in &events {
        assert_eq!(event["request_id"], record["request_id"]);
        assert!(event.to_string().len() <= 64 * 1024);
    }
    // 中间的事件（去掉开始与结束）按它们自带的时刻算，相邻两个至少隔一秒
    let middle = &events[1..events.len() - 1];
    for pair in middle.windows(2) {
        assert!(
            pair[1].0 >= pair[0].0 + 1000,
            "相邻两个中间事件只隔 {} ms",
            pair[1].0 - pair[0].0
        );
    }
}
