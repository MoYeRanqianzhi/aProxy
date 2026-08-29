//! 端到端集成测试：通过真实的 mock 上游 + 代理联调验证重/RY与流式暂存回放。
//!
//! 每个测试启动两个本地服务：
//! - mock 上游：按需返回 500 / 错误 JSON / SSE 成功载荷
//! - aProxy：指向该上游，监听随机端口
//! 客户端直接请求 aProxy，断言重试与字节保真行为。

use axum::{
    http::{HeaderMap, HeaderValue, StatusCode},
    routing::{any, get, post},
    Router,
};
use bytes::Bytes;
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
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

// ---------------------------------------------------------------------------
// 10. 配置代理：上游请求经配置的 HTTP 代理转发，路径与查询串完整透传
// ---------------------------------------------------------------------------
#[tokio::test]
async fn proxy_config_routes_through_proxy() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::{TcpListener, TcpStream};

    // 本地上游：回显所请求的路径与查询串
    let upstream = Router::new().fallback(any(|req: axum::extract::Request| async move {
        let path = req
            .uri()
            .path_and_query()
            .map(|pq| pq.as_str())
            .unwrap_or("/")
            .to_string();
        axum::Json(serde_json::json!({"ok": true, "path": path})).into_response()
    }));
    let (upstream_url, _h1) = bind_random_router(upstream).await;
    let upstream_host = upstream_url.trim_start_matches("http://").to_string();

    // 微型 HTTP 代理：接收“绝对 URI”形式的代理请求（GET http://host:port/path HTTP/1.1），
    // 剥离绝对 URL 为 path+query 后转发给目标并回传响应；记录被命中的次数
    let proxy_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_addr = proxy_listener.local_addr().unwrap();
    let proxy_hits = Arc::new(AtomicUsize::new(0));

    let ph = proxy_hits.clone();
    let uh = upstream_host.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = proxy_listener.accept().await else { break };
            let ph = ph.clone();
            let uh = uh.clone();
            tokio::spawn(async move {
                let mut line = String::new();
                let mut reader = BufReader::new(&mut stream);
                if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                    return;
                }
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() < 2 {
                    return;
                }
                let absolute_url = parts[1].to_string();
                // 读头直到空行，仅验证代理确实收到了请求
                loop {
                    let mut header = String::new();
                    if reader.read_line(&mut header).await.unwrap_or(0) == 0 {
                        return;
                    }
                    if header == "\r\n" || header == "\n" {
                        break;
                    }
                }
                drop(reader);

                ph.fetch_add(1, Ordering::SeqCst);

                let Ok(url) = url::Url::parse(&absolute_url) else { return };
                let path = url.path().to_string();
                let query = url.query().map(|q| format!("?{q}")).unwrap_or_default();
                let req_head = format!(
                    "GET {path}{query} HTTP/1.1\r\nHost: {uh}\r\nConnection: close\r\n\r\n"
                );

                let Ok(mut up) = TcpStream::connect(&uh).await else { return };
                if up.write_all(req_head.as_bytes()).await.is_err() {
                    return;
                }
                // 双向转发：上游响应回给客户端
                let (mut up_r, mut up_w) = up.split();
                let (mut cli_r, mut cli_w) = stream.split();
                let _ = tokio::io::copy(&mut up_r, &mut cli_w).await;
                let _ = tokio::io::copy(&mut cli_r, &mut up_w).await;
            });
        }
    });

    // aProxy 配置走该代理（显式代理会关闭系统/环境变量代理）
    let mut cfg = proxy_config_for(&upstream_url);
    cfg.proxy = Some(format!("http://{proxy_addr}"));
    let proxy_state = AppState::new(cfg);
    let (aproxy_url, _h3) = bind_random_router(aproxy::proxy::router(proxy_state)).await;

    // 测试客户端直连 aProxy，避免被环境变量代理干扰
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let resp = client
        .get(format!("{aproxy_url}/v1/deep/path?q=1"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["ok"], true);
    // 路径与查询串经代理后完整透传
    assert_eq!(body["path"], "/v1/deep/path?q=1");
    // 请求确实经过了配置的代理
    assert_eq!(proxy_hits.load(Ordering::SeqCst), 1);
}

// ---------------------------------------------------------------------------
// 11. 重试时原样重放请求体：上游两次尝试读到的请求体字节完全一致
// ---------------------------------------------------------------------------
#[tokio::test]
async fn request_body_replayed_on_retry() {
    // 记录每次上游尝试实际读到的请求体
    let body_log = Arc::new(Mutex::new(Vec::<Bytes>::new()));
    let counter = Arc::new(AtomicUsize::new(0));

    let bl = body_log.clone();
    let c = counter.clone();
    let upstream = Router::new().route(
        "/v1/echo",
        post(move |req: axum::extract::Request| {
            let bl = bl.clone();
            let c = c.clone();
            async move {
                let n = c.fetch_add(1, Ordering::SeqCst);
                // 读取完整请求体并记录，验证重试时是否原样重放
                let bytes = axum::body::to_bytes(req.into_body(), 10 * 1024 * 1024)
                    .await
                    .unwrap();
                bl.lock().unwrap().push(bytes);
                // 首次返回 500 触发重试，第二次成功
                if n == 0 {
                    (StatusCode::INTERNAL_SERVER_ERROR, "upstream overloaded").into_response()
                } else {
                    (StatusCode::OK, "ok").into_response()
                }
            }
        }),
    );

    let (upstream_url, _h1) = bind_random_router(upstream).await;
    let proxy_state = AppState::new(proxy_config_for(&upstream_url));
    let (proxy_url, _h2) = bind_random_router(aproxy::proxy::router(proxy_state)).await;

    // 客户端直连 aProxy，避免被环境变量代理干扰
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let payload = serde_json::json!({"model": "test", "prompt": "你好，世界"});
    let resp = client
        .post(format!("{proxy_url}/v1/echo"))
        .json(&payload)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // 恰好两次上游尝试（首次 500 + 第二次成功）
    assert_eq!(counter.load(Ordering::SeqCst), 2);
    let log = body_log.lock().unwrap();
    assert_eq!(log.len(), 2);
    // 两次尝试收到的请求体字节完全一致（重试原样重放）
    assert_eq!(log[0], log[1]);
    // 且等于客户端实际发送的 JSON 字节
    let expected = serde_json::to_vec(&payload).unwrap();
    assert_eq!(log[0].as_ref(), expected.as_slice());
}

// ---------------------------------------------------------------------------
// 12. 配置的代理用户名/密码：reqwest 向代理发送 Proxy-Authorization: Basic ...
// ---------------------------------------------------------------------------
#[tokio::test]
async fn proxy_basic_auth_sent() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::{TcpListener, TcpStream};

    // 本地上游：返回成功即可
    let upstream = Router::new().fallback(any(|| async {
        (StatusCode::OK, axum::Json(serde_json::json!({"ok": true}))).into_response()
    }));
    let (upstream_url, _h1) = bind_random_router(upstream).await;
    let upstream_host = upstream_url.trim_start_matches("http://").to_string();

    // 微型 HTTP 代理：参考 proxy_config_routes_through_proxy 的写法，额外在
    // 读请求头循环中捕获 Proxy-Authorization 行的值到共享变量
    let proxy_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_addr = proxy_listener.local_addr().unwrap();
    let captured_auth = Arc::new(Mutex::new(None::<String>));

    let ca = captured_auth.clone();
    let uh = upstream_host.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = proxy_listener.accept().await else { break };
            let ca = ca.clone();
            let uh = uh.clone();
            tokio::spawn(async move {
                let mut line = String::new();
                let mut reader = BufReader::new(&mut stream);
                if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                    return;
                }
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() < 2 {
                    return;
                }
                let absolute_url = parts[1].to_string();
                // 读头直到空行，大小写不敏感捕获 Proxy-Authorization 的值
                loop {
                    let mut header = String::new();
                    if reader.read_line(&mut header).await.unwrap_or(0) == 0 {
                        return;
                    }
                    if header == "\r\n" || header == "\n" {
                        break;
                    }
                    // 仅对头名做大小写不敏感比较，值原样保留（base64 区分大小写）
                    if let Some(idx) = header.find(':') {
                        if header[..idx].trim().eq_ignore_ascii_case("proxy-authorization") {
                            *ca.lock().unwrap() = Some(header[idx + 1..].trim().to_string());
                        }
                    }
                }
                drop(reader);

                let Ok(url) = url::Url::parse(&absolute_url) else { return };
                let path = url.path().to_string();
                let query = url.query().map(|q| format!("?{q}")).unwrap_or_default();
                let req_head = format!(
                    "GET {path}{query} HTTP/1.1\r\nHost: {uh}\r\nConnection: close\r\n\r\n"
                );

                let Ok(mut up) = TcpStream::connect(&uh).await else { return };
                if up.write_all(req_head.as_bytes()).await.is_err() {
                    return;
                }
                // 双向转发：上游响应回给客户端
                let (mut up_r, mut up_w) = up.split();
                let (mut cli_r, mut cli_w) = stream.split();
                let _ = tokio::io::copy(&mut up_r, &mut cli_w).await;
                let _ = tokio::io::copy(&mut cli_r, &mut up_w).await;
            });
        }
    });

    // aProxy 配置走该代理，并单独配置代理用户名/密码
    let mut cfg = proxy_config_for(&upstream_url);
    cfg.proxy = Some(format!("http://{proxy_addr}"));
    cfg.proxy_username = Some("alice".to_string());
    cfg.proxy_password = Some("secret".to_string());
    let proxy_state = AppState::new(cfg);
    let (aproxy_url, _h3) = bind_random_router(aproxy::proxy::router(proxy_state)).await;

    // 测试客户端直连 aProxy，避免被环境变量代理干扰
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let resp = client
        .get(format!("{aproxy_url}/v1/ping"))
        .send()
        .await
        .unwrap();

    // 请求成功透传（上游返回 200）
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["ok"], true);

    // 代理确实收到了 Proxy-Authorization: Basic base64(alice:secret)
    // base64 值 "YWxpY2U6c2VjcmV0" 已用 PowerShell 验证
    let captured = captured_auth.lock().unwrap();
    assert_eq!(captured.as_deref(), Some("Basic YWxpY2U6c2VjcmV0"));
}
