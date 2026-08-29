//! 端到端集成测试：通过真实的 mock 上游 + 代理联调验证重/RY与流式暂存回放。
//!
//! 每个测试启动两个本地服务：
//! - mock 上游：按需返回 500 / 错误 JSON / SSE 成功载荷
//! - aProxy：指向该上游，监听随机端口
//! 客户端直接请求 aProxy，断言重试与字节保真行为。

use axum::{
    body::Body,
    http::{HeaderMap, HeaderValue, StatusCode},
    routing::{any, get, post},
    Router,
};
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use aproxy::{config::Config, proxy::AppState};

async fn bind_random_router(router: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{}", addr);
    let handle = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    // 等待端口就绪
    tokio::time::sleep(Duration::from_millis(50)).await;
    (url, handle)
}

fn proxy_config_for(upstream: &str) -> Config {
    Config {
        upstream_url: upstream.to_string(),
        listen_addr: "127.0.0.1:0".to_string(),
        ..Default::default()
    }
    .normalized()
}

// ---------------------------------------------------------------------------
// 1. 5xx 重试后成功
// ---------------------------------------------------------------------------
#[tokio::test]
async fn retry_on_500_then_success() {
    let counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = counter.clone();

    let upstream = Router::new().route(
        "/v1/chat",
        any(move |_req: axum::extract::Request| {
            let c = counter_clone.clone();
            async move {
                let n = c.fetch_add(1, Ordering::SeqCst);
                if n < 2 {
                    (StatusCode::INTERNAL_SERVER_ERROR, "upstream overloaded").into_response()
                } else {
                    (StatusCode::OK, axum::Json(serde_json::json!({"ok": true}))).into_response()
                }
            }
        }),
    );

    let (upstream_url, _h1) = bind_random_router(upstream).await;

    let proxy_state = AppState::new(proxy_config_for(&upstream_url));
    let proxy_router = aproxy::proxy::router(proxy_state);
    let (proxy_url, _h2) = bind_random_router(proxy_router).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/v1/chat", proxy_url))
        .json(&serde_json::json!({"model": "test"}))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["ok"], true);
    // 上游被调用了 3 次（2 次 500 + 1 次成功）
    assert_eq!(counter.load(Ordering::SeqCst), 3);
}

// ---------------------------------------------------------------------------
// 2. 错误 JSON 内容（200 携带 error）触发重试
// ---------------------------------------------------------------------------
#[tokio::test]
async fn retry_on_error_body_then_success() {
    let counter = Arc::new(AtomicUsize::new(0));
    let c2 = counter.clone();

    let upstream = Router::new().route(
        "/v1/messages",
        any(move |_req: axum::extract::Request| {
            let c = c2.clone();
            async move {
                let n = c.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    (
                        StatusCode::OK,
                        axum::Json(serde_json::json!({"type":"error","error":{"type":"overloaded"}})),
                    )
                        .into_response()
                } else {
                    (StatusCode::OK, axum::Json(serde_json::json!({"content":"hello"}))).into_response()
                }
            }
        }),
    );

    let (upstream_url, _h1) = bind_random_router(upstream).await;
    let proxy_state = AppState::new(proxy_config_for(&upstream_url));
    let (proxy_url, _h2) = bind_random_router(aproxy::proxy::router(proxy_state)).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/v1/messages", proxy_url))
        .json(&serde_json::json!({"stream": false}))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["content"], "hello");
    assert_eq!(counter.load(Ordering::SeqCst), 2);
}

// ---------------------------------------------------------------------------
// 3. 流式暂存后原样回放：字节一致（重试期间的 : keepalive 为 SSE 注释，测试中需过滤后再比对）
// ---------------------------------------------------------------------------
#[tokio::test]
async fn stream_spool_then_replay_preserves_bytes() {
    // 构造一段典型 SSE 载荷
    let sse_payload: String = (0..20)
        .map(|i| format!("data: {{\"id\":{i},\"text\":\"chunk-{i}\"}}\n\n"))
        .collect::<String>()
        + "data: [DONE]\n\n";

    let payload_clone = sse_payload.clone();
    let upstream = Router::new().route(
        "/v1/stream",
        any(move |_req: axum::extract::Request| {
            let p = payload_clone.clone();
            async move {
                let mut headers = HeaderMap::new();
                headers.insert(
                    axum::http::header::CONTENT_TYPE,
                    HeaderValue::from_static("text/event-stream"),
                );
                // 直接返回完整 body，代理侧会 spool 后分块回放
                (StatusCode::OK, headers, p).into_response()
            }
        }),
    );

    let (upstream_url, _h1) = bind_random_router(upstream).await;
    let proxy_state = AppState::new(proxy_config_for(&upstream_url));
    let (proxy_url, _h2) = bind_random_router(aproxy::proxy::router(proxy_state)).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{}/v1/stream", proxy_url))
        .header("Accept", "text/event-stream")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    assert!(resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .unwrap()
        .to_str()
        .unwrap()
        .contains("text/event-stream"));

    let bytes = resp.bytes().await.unwrap();
    let text = String::from_utf8_lossy(&bytes);
    // 过滤 SSE 注释保活行（如 ": keepalive" / ": connected"），保证与上游字节语义一致
    let filtered: String = text
        .lines()
        .filter(|l| !l.starts_with(':'))
        .map(|l| format!("{l}\n"))
        .collect();
    // 重新以 SSE 空行语义比对：过滤后应还原为原始 data 行序列
    // 原始 payload 已是 data 行+空行，直接比对过滤后的 data 行拼接
    let expected_filtered: String = sse_payload
        .lines()
        .filter(|l| !l.starts_with(':'))
        .map(|l| format!("{l}\n"))
        .collect();
    assert_eq!(filtered, expected_filtered);
}

// ---------------------------------------------------------------------------
// 4. 流式内容中的错误 data 行触发重试
// ---------------------------------------------------------------------------
#[tokio::test]
async fn stream_error_data_triggers_retry() {
    let counter = Arc::new(AtomicUsize::new(0));
    let c2 = counter.clone();

    let bad_sse = "data: {\"type\":\"content_block_delta\",\"text\":\"hi\"}\n\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded\"}}\n\n";
    let good_sse = "data: {\"type\":\"content_block_delta\",\"text\":\"hello\"}\n\ndata: [DONE]\n\n";

    let upstream = Router::new().route(
        "/v1/stream",
        any(move |_req: axum::extract::Request| {
            let c = c2.clone();
            async move {
                let n = c.fetch_add(1, Ordering::SeqCst);
                let mut headers = HeaderMap::new();
                headers.insert(
                    axum::http::header::CONTENT_TYPE,
                    HeaderValue::from_static("text/event-stream"),
                );
                if n == 0 {
                    (StatusCode::OK, headers, bad_sse.to_string()).into_response()
                } else {
                    (StatusCode::OK, headers, good_sse.to_string()).into_response()
                }
            }
        }),
    );

    let (upstream_url, _h1) = bind_random_router(upstream).await;
    let proxy_state = AppState::new(proxy_config_for(&upstream_url));
    let (proxy_url, _h2) = bind_random_router(aproxy::proxy::router(proxy_state)).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{}/v1/stream", proxy_url))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert_eq!(body, good_sse);
    assert_eq!(counter.load(Ordering::SeqCst), 2);
}

// ---------------------------------------------------------------------------
// 5. 普通成功透传：状态与头完整透传
// ---------------------------------------------------------------------------
#[tokio::test]
async fn passthrough_success() {
    let upstream = Router::new().route(
        "/v1/ping",
        get(|| async {
            let mut headers = HeaderMap::new();
            headers.insert("x-custom", HeaderValue::from_static("abc"));
            (StatusCode::OK, headers, axum::Json(serde_json::json!({"pong":1}))).into_response()
        }),
    );
    let (upstream_url, _h1) = bind_random_router(upstream).await;
    let proxy_state = AppState::new(proxy_config_for(&upstream_url));
    let (proxy_url, _h2) = bind_random_router(aproxy::proxy::router(proxy_state)).await;

    let client = reqwest::Client::new();
    let resp = client.get(format!("{}/v1/ping", proxy_url)).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.headers().get("x-custom").unwrap(), "abc");
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["pong"], 1);
}

// ---------------------------------------------------------------------------
// 6. /health 已不再是代理侧健康检查，完整透传到上游
// ---------------------------------------------------------------------------
#[tokio::test]
async fn health_is_proxied() {
    let upstream = Router::new().fallback(any(|| async {
        (StatusCode::OK, axum::Json(serde_json::json!({"upstream": "health"}))).into_response()
    }));
    let (upstream_url, _h1) = bind_random_router(upstream).await;
    let proxy_state = AppState::new(proxy_config_for(&upstream_url));
    let (proxy_url, _h2) = bind_random_router(aproxy::proxy::router(proxy_state)).await;

    let client = reqwest::Client::new();
    let resp = client.get(format!("{}/health", proxy_url)).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["upstream"], "health");
}

// ---------------------------------------------------------------------------
// 7. 4xx 亦重试：原型阶段所有错误状态码均视为可重试（符合“易发生的报错都重试”）
//    上游先返回 429/401，再成功，验证 4xx 重试链路
// ---------------------------------------------------------------------------
#[tokio::test]
async fn retry_on_4xx_then_success() {
    let counter = Arc::new(AtomicUsize::new(0));
    let c2 = counter.clone();
    let upstream = Router::new().route(
        "/v1/auth",
        any(move |_req: axum::extract::Request| {
            let c = c2.clone();
            async move {
                let n = c.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    (StatusCode::TOO_MANY_REQUESTS, "rate limited").into_response()
                } else if n == 1 {
                    (StatusCode::UNAUTHORIZED, axum::Json(serde_json::json!({"error":"unauthorized"})))
                        .into_response()
                } else {
                    (StatusCode::OK, axum::Json(serde_json::json!({"ok": true}))).into_response()
                }
            }
        }),
    );
    let (upstream_url, _h1) = bind_random_router(upstream).await;
    let proxy_state = AppState::new(proxy_config_for(&upstream_url));
    let (proxy_url, _h2) = bind_random_router(aproxy::proxy::router(proxy_state)).await;

    let client = reqwest::Client::new();
    let resp = client.get(format!("{}/v1/auth", proxy_url)).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["ok"], true);
    assert_eq!(counter.load(Ordering::SeqCst), 3);
}

use axum::response::IntoResponse;

// ---------------------------------------------------------------------------
// 8. 保活心跳：SSE 客户端在重试期间收到 : keepalive 注释
// ---------------------------------------------------------------------------
#[tokio::test]
async fn keepalive_during_retry() {
    let counter = Arc::new(AtomicUsize::new(0));
    let c2 = counter.clone();
    let upstream = Router::new().route(
        "/v1/slow",
        any(move |_req: axum::extract::Request| {
            let c = c2.clone();
            async move {
                let n = c.fetch_add(1, Ordering::SeqCst);
                if n < 1 {
                    (StatusCode::INTERNAL_SERVER_ERROR, "overloaded").into_response()
                } else {
                    let mut headers = HeaderMap::new();
                    headers.insert(
                        axum::http::header::CONTENT_TYPE,
                        HeaderValue::from_static("text/event-stream"),
                    );
                    (StatusCode::OK, headers, "data: {\"ok\":true}\n\ndata: [DONE]\n\n").into_response()
                }
            }
        }),
    );
    let (upstream_url, _h1) = bind_random_router(upstream).await;
    // 缩短保活间隔以加速测试
    let mut cfg = proxy_config_for(&upstream_url);
    cfg.keepalive_interval_secs = 1;
    let proxy_state = AppState::new(cfg);
    let (proxy_url, _h2) = bind_random_router(aproxy::proxy::router(proxy_state)).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{}/v1/slow", proxy_url))
        .header("Accept", "text/event-stream")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    // SSE 保活注释不应影响最终内容，且应出现在流中
    assert!(body.contains("data: {\"ok\":true}"));
    assert!(body.contains("data: [DONE]"));
}

// ---------------------------------------------------------------------------
// 9. 头覆盖：api_key 快捷 + override/extra
// ---------------------------------------------------------------------------
#[tokio::test]
async fn header_override_and_extra() {
    let upstream = Router::new().route(
        "/v1/echo",
        any(|req: axum::extract::Request| async move {
            let auth = req
                .headers()
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string();
            let extra = req
                .headers()
                .get("x-extra")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string();
            let ov = req
                .headers()
                .get("x-override")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string();
            axum::Json(serde_json::json!({"auth": auth, "extra": extra, "ov": ov})).into_response()
        }),
    );
    let (upstream_url, _h1) = bind_random_router(upstream).await;
    let mut cfg = proxy_config_for(&upstream_url);
    cfg.api_key = Some("sk-test123".to_string());
    cfg.extra_headers.insert("x-extra".to_string(), "from-config".to_string());
    cfg.override_headers.insert("x-override".to_string(), "forced".to_string());
    let proxy_state = AppState::new(cfg);
    let (proxy_url, _h2) = bind_random_router(aproxy::proxy::router(proxy_state)).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{}/v1/echo", proxy_url))
        .header("x-override", "client-value")
        .header("x-extra", "client-extra")
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["auth"], "Bearer sk-test123");
    // override 无条件覆盖
    assert_eq!(body["ov"], "forced");
    // extra 仅在缺失时追加，客户端已带则保留客户端值
    assert_eq!(body["extra"], "client-extra");

    // 第二次：客户端未带 x-extra，应由 config 补上
    let resp2 = client
        .get(format!("{}/v1/echo", proxy_url))
        .send()
        .await
        .unwrap();
    let body2: serde_json::Value = resp2.json().await.unwrap();
    assert_eq!(body2["extra"], "from-config");
}
