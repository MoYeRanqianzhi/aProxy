//! 用临时 home 里的真实后台实例核对三处读数：订阅视图、请求列表、文本日志。
mod common;
use aproxy_ipc::{
    client::ipc_read_in,
    view::{Feed, HomeView},
    wire::{Part, ReadOp},
};
use axum::{Router, routing::post};
use serde_json::{Value, json};
use std::time::Duration;

#[tokio::test]
async fn view_matches_instance_history_and_plain_logs() {
    let home = common::TestHome::new();
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let upstream = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route(
                "/test-only-known-key/v1/messages",
                post(|| async { "local reply" }),
            ),
        )
        .await
        .unwrap()
    });
    const KEY: &str = "test-only-known-key";
    let cfg = home.path().join("view.toml");
    std::fs::write(
        &cfg,
        format!(
            "listen_addr = \"127.0.0.1:0\"\nbase_url = \"http://{upstream}/{KEY}\"\napi_key = \"{KEY}\"\n"
        ),
    )
    .unwrap();
    let start = home
        .cmd()
        .env("RUST_LOG", "info")
        .args(["start", "--json", "--config"])
        .arg(cfg)
        .output()
        .unwrap();
    assert!(start.status.success(), "{:?}", start);
    let doc: Value = serde_json::from_slice(&start.stdout).unwrap();
    let port = u16::try_from(doc["port"].as_u64().unwrap()).unwrap();
    home.stop_on_drop(port);
    let status = aproxy_ipc::client::ipc_ping_in(&home.path().join("run"), &port.to_string())
        .await
        .unwrap();
    assert_eq!(
        status.instance.base_url,
        format!("http://{upstream}/<secret>")
    );
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let url = format!("http://127.0.0.1:{port}/v1/messages");
    let send = || async {
        let r = http
            .post(&url)
            .header("user-agent", KEY)
            .header("x-test-only-known-key", "ordinary")
            .json(&json!({"model":KEY}))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
        assert_eq!(r.text().await.unwrap(), "local reply");
    };
    // 第一条在打开视图前就存在，另外两条从流来；不能只用订阅或只用启动时那一页。
    send().await;
    let view = HomeView::open(home.path().into(), Duration::from_secs(1));
    let mut changes = view.changes();
    let key = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if let Some(i) = view
                .instances()
                .into_iter()
                .find(|i| i.feed == Feed::Streaming)
            {
                break i.run.unwrap();
            }
            changes.recv().await.unwrap();
        }
    })
    .await
    .unwrap();
    send().await;
    send().await;
    let rows = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let rows = view.requests(&key);
            if rows.len() == 3 && rows.iter().all(|r| r.ended_at.is_some()) {
                break rows;
            }
            changes.recv().await.unwrap();
        }
    })
    .await
    .unwrap();
    assert_eq!(
        rows.iter().map(|r| r.request_seq).collect::<Vec<_>>(),
        [3, 2, 1]
    );
    let page = ipc_read_in(
        &home.path().join("run"),
        &port.to_string(),
        &ReadOp::Requests {
            before: None,
            limit: None,
        },
    )
    .await
    .unwrap();
    let raw = page["requests"].as_array().unwrap();
    assert_eq!(rows.len(), raw.len());
    let logs = home
        .cmd()
        .args(["logs", &port.to_string()])
        .output()
        .unwrap();
    assert!(logs.status.success(), "{logs:?}");
    let log = String::from_utf8(logs.stdout).unwrap();
    for (r, raw) in rows.iter().zip(raw) {
        let doc = serde_json::to_value(r).unwrap();
        for field in [
            "request_id",
            "request_seq",
            "started_at",
            "ended_at",
            "phase",
            "outcome",
            "code",
            "client_status",
            "attempts",
            "response_bytes",
        ] {
            assert_eq!(doc[field], raw[field], "{field}");
        }
        assert!(log.contains(&r.request_id), "{log}");
        assert_eq!(r.user_agent.as_deref(), Some("<secret>"));
        assert_eq!(r.model.as_deref(), Some("<secret>"));
    }
    let first = &rows[2].request_id;
    let detail = view.detail(&key, first).await.unwrap();
    assert!(
        detail
            .request
            .headers
            .iter()
            .any(|h| h.name == "x-<secret>")
    );
    assert!(!serde_json::to_string(&detail).unwrap().contains(KEY));
    let chunk = view
        .body_chunk(&key, first, Part::Response, Some(1), 1, 3)
        .await
        .unwrap();
    assert_eq!(
        (chunk.offset, chunk.data_b64.as_str(), chunk.more),
        (1, "b2Nh", true)
    );
    view.shutdown().await;
    server.abort();
}
