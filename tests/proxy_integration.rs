//! 端到端集成测试：通过真实的 mock 上游 + 代理联调验证重试与流式暂存回放。
//!
//! 每个测试启动两个本地服务：
//! - mock 上游：按需返回 500 / 错误 JSON / SSE 成功载荷
//! - aProxy：指向该上游，监听随机端口
//!
//! 客户端直接请求 aProxy，断言重试与字节保真行为。

use axum::{
    Router,
    http::{HeaderMap, HeaderValue, StatusCode},
    routing::{any, get, post},
};
use bytes::Bytes;
use std::{
    io::Read,
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use aproxy::{config::Config, proxy::AppState};

// 本机环境常带 HTTP_PROXY/HTTPS_PROXY/ALL_PROXY（指向外部代理），会把指向
// 127.0.0.1 mock 的测试流量也送出去，造成随机假失败（实测约半数跑挂）。
// 两层隔离：测试客户端一律 no_proxy()；aproxy 内部 upstream 客户端读环境变量
// 构建（生产语义），靠 NO_PROXY 排除环回目标——须在任何 AppState::new 之前设置。
static ENV_GUARD: std::sync::Once = std::sync::Once::new();

fn isolate_env_proxy() {
    ENV_GUARD.call_once(|| {
        // SAFETY: 测试进程内仅此一处写环境变量；首次 proxy_config_for 调用早于
        // 绝大多数 reqwest 客户端构建。多线程并发读写 env 理论上 UB，但此处
        // 一次性写入且早于测试主体，实践中安全。
        unsafe {
            std::env::set_var("NO_PROXY", "127.0.0.1,localhost");
            std::env::set_var("no_proxy", "127.0.0.1,localhost");
        }
    });
}

fn local_client() -> reqwest::Client {
    isolate_env_proxy();
    reqwest::Client::builder().no_proxy().build().unwrap()
}

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
    isolate_env_proxy();
    Config {
        base_url: upstream.to_string(),
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

    let client = local_client();
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
                        axum::Json(
                            serde_json::json!({"type":"error","error":{"type":"overloaded"}}),
                        ),
                    )
                        .into_response()
                } else {
                    (
                        StatusCode::OK,
                        axum::Json(serde_json::json!({"content":"hello"})),
                    )
                        .into_response()
                }
            }
        }),
    );

    let (upstream_url, _h1) = bind_random_router(upstream).await;
    let proxy_state = AppState::new(proxy_config_for(&upstream_url));
    let (proxy_url, _h2) = bind_random_router(aproxy::proxy::router(proxy_state)).await;

    let client = local_client();
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

    let client = local_client();
    let resp = client
        .get(format!("{}/v1/stream", proxy_url))
        .header("Accept", "text/event-stream")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    assert!(
        resp.headers()
            .get(reqwest::header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap()
            .contains("text/event-stream")
    );

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
    let good_sse =
        "data: {\"type\":\"content_block_delta\",\"text\":\"hello\"}\n\ndata: [DONE]\n\n";

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

    let client = local_client();
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
            (
                StatusCode::OK,
                headers,
                axum::Json(serde_json::json!({"pong":1})),
            )
                .into_response()
        }),
    );
    let (upstream_url, _h1) = bind_random_router(upstream).await;
    let proxy_state = AppState::new(proxy_config_for(&upstream_url));
    let (proxy_url, _h2) = bind_random_router(aproxy::proxy::router(proxy_state)).await;

    let client = local_client();
    let resp = client
        .get(format!("{}/v1/ping", proxy_url))
        .send()
        .await
        .unwrap();
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
        (
            StatusCode::OK,
            axum::Json(serde_json::json!({"upstream": "health"})),
        )
            .into_response()
    }));
    let (upstream_url, _h1) = bind_random_router(upstream).await;
    let proxy_state = AppState::new(proxy_config_for(&upstream_url));
    let (proxy_url, _h2) = bind_random_router(aproxy::proxy::router(proxy_state)).await;

    let client = local_client();
    let resp = client
        .get(format!("{}/health", proxy_url))
        .send()
        .await
        .unwrap();
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
                    (
                        StatusCode::UNAUTHORIZED,
                        axum::Json(serde_json::json!({"error":"unauthorized"})),
                    )
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

    let client = local_client();
    let resp = client
        .get(format!("{}/v1/auth", proxy_url))
        .send()
        .await
        .unwrap();
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
                    (
                        StatusCode::OK,
                        headers,
                        "data: {\"ok\":true}\n\ndata: [DONE]\n\n",
                    )
                        .into_response()
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

    let client = local_client();
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
    // 测试名称所声称的两个前提必须被验证：心跳确实发过、重试确实发生
    assert!(
        body.contains(": keepalive"),
        "重试期间应收到 : keepalive 心跳注释"
    );
    assert_eq!(
        counter.load(Ordering::SeqCst),
        2,
        "应恰好两次上游尝试（首次 500 + 重试成功）"
    );
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
    cfg.extra_headers
        .insert("x-extra".to_string(), "from-config".to_string());
    cfg.override_headers
        .insert("x-override".to_string(), "forced".to_string());
    let proxy_state = AppState::new(cfg);
    let (proxy_url, _h2) = bind_random_router(aproxy::proxy::router(proxy_state)).await;

    let client = local_client();
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
            let Ok((mut stream, _)) = proxy_listener.accept().await else {
                break;
            };
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

                let Ok(url) = url::Url::parse(&absolute_url) else {
                    return;
                };
                let path = url.path().to_string();
                let query = url.query().map(|q| format!("?{q}")).unwrap_or_default();
                let req_head = format!(
                    "GET {path}{query} HTTP/1.1\r\nHost: {uh}\r\nConnection: close\r\n\r\n"
                );

                let Ok(mut up) = TcpStream::connect(&uh).await else {
                    return;
                };
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
    let client = local_client();
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
    let client = local_client();
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
            let Ok((mut stream, _)) = proxy_listener.accept().await else {
                break;
            };
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
                    if let Some(idx) = header.find(':')
                        && header[..idx]
                            .trim()
                            .eq_ignore_ascii_case("proxy-authorization")
                    {
                        *ca.lock().unwrap() = Some(header[idx + 1..].trim().to_string());
                    }
                }
                drop(reader);

                let Ok(url) = url::Url::parse(&absolute_url) else {
                    return;
                };
                let path = url.path().to_string();
                let query = url.query().map(|q| format!("?{q}")).unwrap_or_default();
                let req_head = format!(
                    "GET {path}{query} HTTP/1.1\r\nHost: {uh}\r\nConnection: close\r\n\r\n"
                );

                let Ok(mut up) = TcpStream::connect(&uh).await else {
                    return;
                };
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
    let client = local_client();
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

// ---------------------------------------------------------------------------
// 15. 客户端断开 → 后台任务立即中止：上游计数停滞（计费保护）
//
// keepalive 通道下上游持续 429，客户端读到首个心跳后主动断开。
// 断开后后台任务必须退出：不再发起新的上游请求，in-flight 的请求也被 select 竞速丢弃。
//
// 观测窗口必须覆盖完整退避周期（attempt4 起 5s、10s…）：断开通常发生在
// 5s 退避间隙内，过短的窗口即使删除断开保护计数也自然停滞，测试失去区分度。
// 用请求时间戳断言「断开时刻（+余量）之后没有任何新请求」。
// ---------------------------------------------------------------------------
#[tokio::test]
async fn client_disconnect_stops_upstream_requests() {
    let counter = Arc::new(AtomicUsize::new(0));
    let stamps: Arc<Mutex<Vec<std::time::Instant>>> = Arc::new(Mutex::new(Vec::new()));
    let c2 = counter.clone();
    let s2 = stamps.clone();
    let upstream = Router::new().route(
        "/v1/slow",
        any(move |_req: axum::extract::Request| {
            let c = c2.clone();
            let s = s2.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                s.lock().unwrap().push(std::time::Instant::now());
                // 永远 429：迫使代理进入无限重试
                (StatusCode::TOO_MANY_REQUESTS, "rate limited").into_response()
            }
        }),
    );
    let (upstream_url, _h1) = bind_random_router(upstream).await;
    let mut cfg = proxy_config_for(&upstream_url);
    cfg.keepalive_interval_secs = 1;
    let proxy_state = AppState::new(cfg);
    let (proxy_url, _h2) = bind_random_router(aproxy::proxy::router(proxy_state)).await;

    let client = local_client();
    let resp = client
        .get(format!("{}/v1/slow", proxy_url))
        .header("Accept", "text/event-stream")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "keepalive 骨架应立即返回 200");

    // 读到首个 ": keepalive" 心跳（证明后台任务在跑）后立即断开
    use futures_util::StreamExt;
    let mut stream = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = stream.next().await {
        buf.extend_from_slice(&chunk.unwrap());
        if String::from_utf8_lossy(&buf).contains(": keepalive") {
            break;
        }
    }
    assert!(!buf.is_empty(), "断开前应已收到心跳字节");
    drop(stream); // bytes_stream 借用 resp，先 drop 流再随作用域 drop resp → 连接断开

    // 断开后的首次重试窗口：select 竞速应拦下 in-flight 与后续尝试
    let c_at_disconnect = counter.load(Ordering::SeqCst);
    let disconnected_at = std::time::Instant::now();
    // 覆盖断开后的两个退避周期（5s + 10s）：若保护失效，5s 退避结束后的
    // attempt4 必然产生新请求并落入窗口
    tokio::time::sleep(Duration::from_secs(17)).await;
    // 1.5s 为调度余量；保护失效场景的新请求出现在断开 +5s 处，远超余量
    let leaks = stamps
        .lock()
        .unwrap()
        .iter()
        .filter(|&&t| t >= disconnected_at + Duration::from_millis(1500))
        .count();
    assert_eq!(
        leaks, 0,
        "断开后不得发起新的上游请求（计费保护），断开时计数={c_at_disconnect}"
    );
}

// ---------------------------------------------------------------------------
// 16. 超过 10 MiB 的请求体 → 413
// ---------------------------------------------------------------------------
#[tokio::test]
async fn oversized_request_returns_413() {
    let upstream = Router::new().route("/v1/x", any(|| async { "ok" }));
    let (upstream_url, _h1) = bind_random_router(upstream).await;
    let proxy_state = AppState::new(proxy_config_for(&upstream_url));
    let (proxy_url, _h2) = bind_random_router(aproxy::proxy::router(proxy_state)).await;

    let big = vec![b'a'; 10 * 1024 * 1024 + 1];
    let client = local_client();
    let resp = client
        .post(format!("{proxy_url}/v1/x"))
        .body(big)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 413, "超过 10 MiB 上限应返回 413");
}

// ---------------------------------------------------------------------------
// 17. 8 KiB 块边界：恰好 2×8KiB 的 body 回放字节完全一致
// ---------------------------------------------------------------------------
#[tokio::test]
async fn chunk_boundary_body_replayed_byte_exact() {
    // 16384 = 8 KiB × 2：恰好跨回放分块边界，验证多块循环与字节保真
    let payload = vec![b'x'; 16 * 1024];
    let upstream = Router::new().route(
        "/v1/big",
        any(move || {
            let payload = payload.clone();
            async move { (StatusCode::OK, payload).into_response() }
        }),
    );
    let (upstream_url, _h1) = bind_random_router(upstream).await;
    let proxy_state = AppState::new(proxy_config_for(&upstream_url));
    let (proxy_url, _h2) = bind_random_router(aproxy::proxy::router(proxy_state)).await;

    let client = local_client();
    let resp = client
        .get(format!("{proxy_url}/v1/big"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.bytes().await.unwrap();
    assert_eq!(body.len(), 16 * 1024, "回放 body 长度必须与上游一致");
    assert!(
        body.iter().all(|&b| b == b'x'),
        "回放 body 内容必须与上游一致"
    );
}

// ---------------------------------------------------------------------------
// 18. api_key 同时覆盖 Authorization 与 x-api-key
// ---------------------------------------------------------------------------
#[tokio::test]
async fn api_key_overrides_both_auth_headers() {
    let captured: Arc<Mutex<Option<(String, String)>>> = Arc::new(Mutex::new(None));
    let cap = captured.clone();
    let upstream = Router::new().route(
        "/v1/echo",
        any(move |req: axum::extract::Request| {
            let cap = cap.clone();
            async move {
                let auth = req
                    .headers()
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_string();
                let xkey = req
                    .headers()
                    .get("x-api-key")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_string();
                *cap.lock().unwrap() = Some((auth, xkey));
                "ok".into_response()
            }
        }),
    );
    let (upstream_url, _h1) = bind_random_router(upstream).await;

    let mut cfg = proxy_config_for(&upstream_url);
    cfg.api_key = Some("sk-proxy-key".to_string());
    let proxy_state = AppState::new(cfg.normalized());
    let (proxy_url, _h2) = bind_random_router(aproxy::proxy::router(proxy_state)).await;

    let client = local_client();
    client
        .post(format!("{proxy_url}/v1/echo"))
        .header("x-api-key", "sk-client-original")
        .send()
        .await
        .unwrap();

    let (auth, xkey) = captured.lock().unwrap().clone().expect("上游应收到请求");
    assert_eq!(auth, "Bearer sk-proxy-key", "authorization 应为配置 key");
    assert_eq!(
        xkey, "sk-proxy-key",
        "x-api-key 应被配置 key 覆盖，不得泄漏客户端原值"
    );
}

// ---------------------------------------------------------------------------
// 19. 多值 Set-Cookie 头透传（append 而非坍缩）
// ---------------------------------------------------------------------------
#[tokio::test]
async fn multi_value_set_cookie_headers_pass_through() {
    let upstream = Router::new().route(
        "/v1/cookies",
        any(|| async {
            let mut headers = HeaderMap::new();
            headers.append(
                axum::http::header::SET_COOKIE,
                HeaderValue::from_static("a=1"),
            );
            headers.append(
                axum::http::header::SET_COOKIE,
                HeaderValue::from_static("b=2"),
            );
            (StatusCode::OK, headers, "ok").into_response()
        }),
    );
    let (upstream_url, _h1) = bind_random_router(upstream).await;
    let proxy_state = AppState::new(proxy_config_for(&upstream_url));
    let (proxy_url, _h2) = bind_random_router(aproxy::proxy::router(proxy_state)).await;

    let client = local_client();
    let resp = client
        .get(format!("{proxy_url}/v1/cookies"))
        .send()
        .await
        .unwrap();
    let cookies: Vec<_> = resp
        .headers()
        .get_all("set-cookie")
        .iter()
        .map(|v| v.to_str().unwrap().to_string())
        .collect();
    assert_eq!(cookies.len(), 2, "两个 Set-Cookie 都应透传，不得坍缩");
    assert!(cookies.contains(&"a=1".to_string()) && cookies.contains(&"b=2".to_string()));
}

// ---------------------------------------------------------------------------
// 20. 流式中断可重试：上游发出部分 SSE 后断开连接，代理 spool 失败必须重试
//     （原始 TCP 上游，axum 无法模拟「响应中途掐断」）
// ---------------------------------------------------------------------------
#[tokio::test]
async fn interrupted_stream_is_retried() {
    use tokio::io::AsyncWriteExt;

    let counter = Arc::new(AtomicUsize::new(0));
    let c2 = counter.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let n = c2.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                // 首次：chunked 响应写出部分数据后不发终止块直接断开
                // （connection-close framing 下 FIN 即合法结束，必须用 chunked 半截才构成"中断"）
                let partial = b"data: {\"partial\":true}\n\n";
                let mut head = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n{:x}\r\n",
                    partial.len()
                )
                .into_bytes();
                head.extend_from_slice(partial);
                head.extend_from_slice(b"\r\n");
                let _ = sock.write_all(&head).await;
                let _ = sock.flush().await;
                drop(sock);
            } else {
                // 第二次：完整成功响应（chunked + 终止块）
                let body = b"data: {\"ok\":true}\n\ndata: [DONE]\n\n";
                let mut resp = String::from(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n",
                );
                resp.push_str(&format!("{:x}\r\n", body.len()));
                // 将 body 转义为单帧写入
                let mut frame = resp.into_bytes();
                frame.extend_from_slice(body);
                frame.extend_from_slice(b"\r\n0\r\n\r\n");
                let _ = sock.write_all(&frame).await;
                let _ = sock.flush().await;
                let _ = sock.shutdown().await;
            }
        }
    });

    let proxy_state = AppState::new(proxy_config_for(&format!("http://{addr}")));
    let (proxy_url, _h2) = bind_random_router(aproxy::proxy::router(proxy_state)).await;

    let client = local_client();
    let resp = client
        .post(format!("{proxy_url}/v1/messages"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("data: {\"ok\":true}"),
        "中断后应回放第二次成功的完整 body，得到: {body}"
    );
    assert!(
        !body.contains("\"partial\""),
        "首次中断的部分流不得泄漏给客户端"
    );
    assert_eq!(
        counter.load(Ordering::SeqCst),
        2,
        "spool 中断后应恰好重试一次"
    );
}

// ---------------------------------------------------------------------------
// 21. parity：首轮成功时 keepalive 开与关的回放完全一致（保真快速路径锁定）
// ---------------------------------------------------------------------------
#[tokio::test]
async fn keepalive_first_attempt_success_matches_non_keepalive() {
    let upstream = Router::new().route(
        "/v1/sse",
        any(|| async {
            let mut headers = HeaderMap::new();
            headers.insert(
                axum::http::header::CONTENT_TYPE,
                HeaderValue::from_static("text/event-stream"),
            );
            (
                StatusCode::OK,
                headers,
                "data: {\"ok\":true}\n\ndata: [DONE]\n\n",
            )
                .into_response()
        }),
    );
    let (upstream_url, _h1) = bind_random_router(upstream).await;

    // keepalive 开（默认间隔）与关（0 = 禁用）两个实例
    let on_state = AppState::new(proxy_config_for(&upstream_url));
    let (on_url, _p1) = bind_random_router(aproxy::proxy::router(on_state)).await;
    let mut cfg_off = proxy_config_for(&upstream_url);
    cfg_off.keepalive_interval_secs = 0;
    let off_state = AppState::new(cfg_off);
    let (off_url, _p2) = bind_random_router(aproxy::proxy::router(off_state)).await;

    let client = local_client();
    let r1 = client
        .post(format!("{on_url}/v1/sse"))
        .header("Accept", "text/event-stream")
        .send()
        .await
        .unwrap();
    let r2 = client
        .post(format!("{off_url}/v1/sse"))
        .header("Accept", "text/event-stream")
        .send()
        .await
        .unwrap();

    assert_eq!(r1.status(), r2.status(), "status 必须一致");
    let ct1 = r1
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let ct2 = r2
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let b1 = r1.bytes().await.unwrap();
    let b2 = r2.bytes().await.unwrap();
    assert_eq!(b1, b2, "keepalive 开与关的回放 body 必须字节一致");
    assert_eq!(ct1, ct2, "content-type 必须一致");
    assert!(ct1.contains("text/event-stream"), "上游 SSE 头应原样透传");
}

// ---------------------------------------------------------------------------
// 21. CLI 进程级：--config 显式配置文件（多开不同配置的进程）
//
// 每个进程一份配置：启动时加载指定文件，config 子命令读写同一文件。
// ---------------------------------------------------------------------------
use std::process::Command;

#[test]
fn cli_config_flag_rejects_missing_file_on_start() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("nope.toml");
    let out = Command::new(env!("CARGO_BIN_EXE_aproxy"))
        .arg("--config")
        .arg(&missing)
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "显式指定的配置文件不存在时启动必须失败（而非静默回退默认配置）"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("不存在"),
        "stderr 应提示文件不存在，实际: {stderr}"
    );
}

#[test]
fn cli_config_flag_scopes_config_subcommand() {
    let dir = tempfile::tempdir().unwrap();
    let cfg_file = dir.path().join("instance.toml");

    // 多开工作流：为新实例创建独立配置，写入 --config 指定的文件
    let out = Command::new(env!("CARGO_BIN_EXE_aproxy"))
        .arg("--config")
        .arg(&cfg_file)
        .args([
            "config",
            "--baseurl",
            "https://multi-instance.example.com",
            "--keepalive-secs",
            "30",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "config 子命令应成功，stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let saved = std::fs::read_to_string(&cfg_file).unwrap();
    assert!(
        saved.contains("multi-instance.example.com"),
        "配置应写入 --config 指定的文件，实际: {saved}"
    );
    assert!(
        saved.contains("keepalive_interval_secs = 30"),
        "其他字段应一并保存，实际: {saved}"
    );

    // --show 读取的也是同一份文件
    let out = Command::new(env!("CARGO_BIN_EXE_aproxy"))
        .arg("--config")
        .arg(&cfg_file)
        .args(["config", "--show"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("multi-instance.example.com"),
        "--show 应展示 --config 文件的内容，实际: {stdout}"
    );
}

// ---------------------------------------------------------------------------
// 22-24 守护进程测试公共设施
//
// 端口派生：守护测试曾硬编码 59001/59002 并对这些端口无条件执行真实
// `aproxy stop`——stop 按端口 IPC 定位、不区分实例身份，开发者若恰好把日常
// 实例跑在这两个端口，cargo test 会静默关掉它（生产代理中断且无任何提示）。
// 改为从测试进程 pid 派生专属高位端口：与用户实例、跨 worktree 并行测试撞
// 端口的概率都降到可忽略；预清理 stop 也只针对派生端口，永远不会触碰
// 12345 等用户可能使用的端口。
// ---------------------------------------------------------------------------

/// 从测试进程 pid 派生第 offset 个互不相同的守护测试端口（25000..=65000 区间，
/// 避开默认端口 12345 与常见手工实例端口；每个测试用不同 offset，互不冲突）。
fn daemon_test_port(offset: u32) -> u16 {
    (25000 + (std::process::id() % 20000) * 2 + offset) as u16
}

/// 既有守护测试的三个端口（offset 0..=2）。
fn daemon_test_ports() -> (u16, u16, u16) {
    (
        daemon_test_port(0),
        daemon_test_port(1),
        daemon_test_port(2),
    )
}

/// 守护清理守卫：Drop 时对测试派生端口执行 `aproxy stop`。
/// 守护以分离进程运行（不随测试进程退出），此前 stop 只在全部断言通过后才
/// 执行——任何一处断言失败都会把携带假上游的守护泄漏在真实 ~/.aproxy
/// 注册表/日志里，直到手工清理。Drop 在断言失败的 unwind 路径同样运行，
/// 杜绝泄漏。
struct DaemonGuard {
    exe: &'static str,
    port: u16,
}

impl Drop for DaemonGuard {
    fn drop(&mut self) {
        let _ = Command::new(self.exe)
            .args(["stop", &self.port.to_string()])
            .output();
    }
}

/// 等待守护就绪：TCP 可连只能证明「端口上有监听者」——可能是恰好占用端口的
/// 其他程序，或并行测试的另一实例，此前的 ready 判定会让这类占用者造成
/// 误导性假失败。须再经 IPC ping 确认是自家守护（管道名含端口，只有我们的
/// --daemon-child 子进程会创建它）才算就绪。
fn wait_daemon_ready(port: u16) -> bool {
    let port_str = port.to_string();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("创建测试 tokio runtime 失败");
    for _ in 0..100 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok()
            && rt.block_on(aproxy::daemon::ipc_ping(&port_str)).is_ok()
        {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

/// 进程存活探测：Windows 用 tasklist 按 PID 过滤；无匹配时输出为纯文字
/// 提示（不含数字），以「输出中出现该 pid」作为存活判据。
#[cfg(windows)]
fn process_alive(pid: u32) -> bool {
    let out = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}")])
        .output()
        .expect("tasklist 执行失败");
    String::from_utf8_lossy(&out.stdout).contains(&pid.to_string())
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    Command::new("ps")
        .arg("-p")
        .arg(pid.to_string())
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// 22. 守护进程生命周期：守护子进程承载服务 → status 列出 → stop 优雅停止
//
// 控制通道走 IPC（命名管道），代理端口完全用于透传，此处一并验证互不干扰。
// 注意：直接以 --daemon-child 拉起守护（与 `aproxy start` 的 spawn_detached
// 同一路径），不在测试进程树里再嵌套一层 start 父进程（该路径由测试 24 覆盖）。
// ---------------------------------------------------------------------------
#[test]
fn daemon_lifecycle_start_status_stop() {
    let (port, _port_b, _port_c) = daemon_test_ports();
    let dir = tempfile::tempdir().unwrap();
    let cfg_file = dir.path().join("daemon.toml");
    std::fs::write(
        &cfg_file,
        format!(
            "base_url = \"https://daemon-test.example.com\"\nlisten_addr = \"127.0.0.1:{port}\"\n"
        ),
    )
    .unwrap();
    let exe = env!("CARGO_BIN_EXE_aproxy");

    // 预清理：仅针对派生端口，清掉同端口残留（不影响任何其他端口上的实例）
    let _ = Command::new(exe).args(["stop", &port.to_string()]).output();
    let _guard = DaemonGuard { exe, port };

    let pid = aproxy::daemon::spawn_detached(
        std::path::Path::new(exe),
        &[
            "--config".to_string(),
            cfg_file.display().to_string(),
            "--daemon-child".to_string(),
        ],
    )
    .expect("spawn 守护子进程失败");

    // 就绪 = TCP 可连且 IPC ping 确认是自家守护（最多 10 秒）
    assert!(wait_daemon_ready(port), "守护子进程未就绪 (pid {pid})");

    // status 列出该实例（信息来自实例注册表，存活以 IPC 探测为准）
    let out = Command::new(exe).arg("status").output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains(&port.to_string()),
        "status 应列出实例: {stdout}"
    );
    assert!(
        stdout.contains("daemon-test.example.com"),
        "status 应展示上游: {stdout}"
    );

    // stop 指定端口：经 IPC 优雅停止并确认退出
    let out = Command::new(exe)
        .args(["stop", &port.to_string()])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("已停止"), "实际: {stdout}");

    // status 不再列出本实例（其他端口上可能还有并行测试的实例，不全局断言为空）
    let out = Command::new(exe).arg("status").output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains(&port.to_string()),
        "stop 后 status 不应再列出 {port}: {stdout}"
    );
}

// ---------------------------------------------------------------------------
// 23. 同端口重复守护：第二个守护 bind 失败即退出，原实例不受影响
// ---------------------------------------------------------------------------
#[test]
fn daemon_second_instance_on_same_port_exits() {
    let (_port_a, port, _port_c) = daemon_test_ports();
    let dir = tempfile::tempdir().unwrap();
    let cfg_file = dir.path().join("twice.toml");
    std::fs::write(
        &cfg_file,
        format!(
            "base_url = \"https://twice-test.example.com\"\nlisten_addr = \"127.0.0.1:{port}\"\n"
        ),
    )
    .unwrap();
    let exe = env!("CARGO_BIN_EXE_aproxy");
    let _ = Command::new(exe).args(["stop", &port.to_string()]).output();
    let _guard = DaemonGuard { exe, port };

    let args = |file: &std::path::Path| {
        vec![
            "--config".to_string(),
            file.display().to_string(),
            "--daemon-child".to_string(),
        ]
    };

    // 第一个守护：正常承载服务
    let pid1 = aproxy::daemon::spawn_detached(std::path::Path::new(exe), &args(&cfg_file))
        .expect("spawn 第一个守护失败");
    assert!(wait_daemon_ready(port), "第一个守护未就绪 (pid {pid1})");

    // 第二个守护：同端口 bind 失败 → 快速退出（不挂、不影响原实例）
    let pid2 = aproxy::daemon::spawn_detached(std::path::Path::new(exe), &args(&cfg_file))
        .expect("spawn 第二个守护失败");

    // 必须真正验证「第二个守护退出」：轮询 pid2 进程消失（最多 15 秒），
    // 不能只靠固定 sleep——回归成「第二实例滞留」时测试必须失败
    let mut exited = false;
    for _ in 0..150 {
        if !process_alive(pid2) {
            exited = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(exited, "第二个守护 (pid {pid2}) 应在 bind 失败后退出");

    // 原实例仍在服务（IPC ping 可达 + status 仍列出）
    assert!(wait_daemon_ready(port), "原实例应仍在运行 (pid {pid1})");
    let out = Command::new(exe).arg("status").output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains(&port.to_string()),
        "原实例应仍在运行: {stdout}"
    );
}

// ---------------------------------------------------------------------------
// 24. 回归：start 父进程经 Command::output() 运行必须正常返回
//
// spawn_detached 曾用 std::process::Command 启动守护子进程，Windows 上其
// CreateProcessW 固定 bInheritHandles=TRUE，调用方 Command::output() 的
// stdout/stderr 管道写端句柄会被常驻守护继承，EOF 永不到来——以捕获输出的
// 方式运行 `aproxy start`（agent 脚本/CI 的典型调用形态）会永久挂死。
// 修复后（手写 CreateProcessW，bInheritHandles=FALSE）此调用必须正常返回：
// 本测试若挂死即说明修复回归。
// ---------------------------------------------------------------------------
#[test]
fn start_parent_command_output_returns() {
    let (_port_a, _port_b, port) = daemon_test_ports();
    let dir = tempfile::tempdir().unwrap();
    let cfg_file = dir.path().join("start.toml");
    std::fs::write(
        &cfg_file,
        format!(
            "base_url = \"https://start-parent-test.example.com\"\nlisten_addr = \"127.0.0.1:{port}\"\n"
        ),
    )
    .unwrap();
    let exe = env!("CARGO_BIN_EXE_aproxy");

    let _ = Command::new(exe).args(["stop", &port.to_string()]).output();
    let _guard = DaemonGuard { exe, port };

    // 无子命令 = 后台启动：真实 start 父进程做预检、spawn 分离守护、
    // 等待 IPC 就绪后打印结果并退出——捕获输出的调用必须能等到这个退出
    let out = Command::new(exe)
        .arg("--config")
        .arg(&cfg_file)
        .output()
        .expect("start 父进程执行失败");
    assert!(
        out.status.success(),
        "start 应成功，stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("已在后台启动"),
        "start 父进程应报告后台启动，实际: {stdout}"
    );
}

// ---------------------------------------------------------------------------
// 25. `aproxy logs [PORT]`：连接实例实时输出守护日志，实例停止后自动退出
// ---------------------------------------------------------------------------
#[test]
fn logs_follows_daemon_and_exits_on_stop() {
    // 独立派生端口：既有测试占用 daemon_test_ports() 的前三个，这里从偏移 3 起
    let port = daemon_test_port(3);
    let dir = tempfile::tempdir().unwrap();
    let cfg_file = dir.path().join("logs.toml");
    std::fs::write(
        &cfg_file,
        format!(
            "base_url = \"https://logs-test.example.com\"\nlisten_addr = \"127.0.0.1:{port}\"\n"
        ),
    )
    .unwrap();
    let exe = env!("CARGO_BIN_EXE_aproxy");
    let _ = Command::new(exe).args(["stop", &port.to_string()]).output();
    let _guard = DaemonGuard { exe, port };

    let pid = aproxy::daemon::spawn_detached(
        std::path::Path::new(exe),
        &[
            "--config".to_string(),
            cfg_file.display().to_string(),
            "--daemon-child".to_string(),
        ],
    )
    .expect("spawn 守护子进程失败");
    assert!(wait_daemon_ready(port), "守护子进程未就绪 (pid {pid})");

    // 连接 logs（stdout 管道捕获；logs 进程为单层 spawn，无句柄继承问题）
    let mut child = Command::new(exe)
        .args(["logs", &port.to_string()])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn aproxy logs 失败");
    let collected = Arc::new(Mutex::new(String::new()));
    let reader = {
        let collected = collected.clone();
        let mut pipe = child.stdout.take().expect("logs stdout 管道");
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match pipe.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => collected
                        .lock()
                        .unwrap()
                        .push_str(&String::from_utf8_lossy(&buf[..n])),
                }
            }
        })
    };

    // 首屏应包含守护的启动日志行（tracing 写入日志文件，logs 回读输出）
    let mut saw_startup = false;
    for _ in 0..100 {
        if collected.lock().unwrap().contains("启动 aProxy") {
            saw_startup = true;
            break;
        }
        if child.try_wait().ok().flatten().is_some() {
            break; // logs 进程提前退出：断言时以已收集内容为准
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        saw_startup,
        "logs 首屏应输出启动日志行，实际: {}",
        collected.lock().unwrap()
    );

    // stop 实例 → logs 感知实例死亡后自动退出（IPC 探活），不挂死
    let _ = Command::new(exe).args(["stop", &port.to_string()]).output();
    let mut exited = false;
    for _ in 0..150 {
        if child.try_wait().ok().flatten().is_some() {
            exited = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    if !exited {
        // 兜底清理（断言仍会失败，但不能泄漏挂死的 logs 进程）
        let _ = child.kill();
        let _ = child.wait();
    }
    assert!(exited, "实例停止后 aproxy logs 应自动退出");
    let _ = reader.join();
    let out = collected.lock().unwrap();
    assert!(
        out.contains("实例已停止，日志跟踪结束"),
        "logs 退出前应说明原因，实际: {out}"
    );
}

// ---------------------------------------------------------------------------
// 26. `aproxy logs` 多实例时必须指定端口；不支持 all
// ---------------------------------------------------------------------------
#[test]
fn logs_requires_port_when_multiple_instances() {
    let port_a = daemon_test_port(4);
    let port_b = daemon_test_port(5);
    let exe = env!("CARGO_BIN_EXE_aproxy");

    let spawn_daemon = |port: u16, name: &str| {
        let dir = tempfile::tempdir().unwrap();
        let cfg_file = dir.path().join(name);
        std::fs::write(
            &cfg_file,
            format!(
                "base_url = \"https://{name}.example.com\"\nlisten_addr = \"127.0.0.1:{port}\"\n"
            ),
        )
        .unwrap();
        let _ = Command::new(exe).args(["stop", &port.to_string()]).output();
        let pid = aproxy::daemon::spawn_detached(
            std::path::Path::new(exe),
            &[
                "--config".to_string(),
                cfg_file.display().to_string(),
                "--daemon-child".to_string(),
            ],
        )
        .expect("spawn 守护子进程失败");
        (pid, dir)
    };
    // dir 须存活到测试结束（注册表里 config_path 引用它，无需实际存在，但保持干净）
    let (pid_a, _dir_a) = spawn_daemon(port_a, "logs-multi-a.toml");
    let (pid_b, _dir_b) = spawn_daemon(port_b, "logs-multi-b.toml");
    let _guard_a = DaemonGuard { exe, port: port_a };
    let _guard_b = DaemonGuard { exe, port: port_b };
    assert!(wait_daemon_ready(port_a), "守护 a 未就绪 (pid {pid_a})");
    assert!(wait_daemon_ready(port_b), "守护 b 未就绪 (pid {pid_b})");

    // 无参：报错列出两个端口
    let out = Command::new(exe).arg("logs").output().unwrap();
    assert!(!out.status.success(), "多实例时无参 logs 应失败");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(text.contains("必须指定端口号"), "实际: {text}");
    assert!(text.contains(&port_a.to_string()), "应列出端口 a: {text}");
    assert!(text.contains(&port_b.to_string()), "应列出端口 b: {text}");

    // all：明确拒绝（一次只能连接一个）
    let out = Command::new(exe).args(["logs", "all"]).output().unwrap();
    assert!(!out.status.success(), "logs all 应失败");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(text.contains("不支持 all"), "实际: {text}");
}

// ---------------------------------------------------------------------------
// 27. `aproxy logs <空端口>`：实例不存在时报错退出
// ---------------------------------------------------------------------------
#[test]
fn logs_reports_missing_instance() {
    let port = daemon_test_port(6); // 从未在该端口启动守护
    let exe = env!("CARGO_BIN_EXE_aproxy");
    let out = Command::new(exe)
        .args(["logs", &port.to_string()])
        .output()
        .unwrap();
    assert!(!out.status.success(), "无实例时 logs 应失败");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("没有运行中的 aProxy 实例"),
        "实际: {stdout}"
    );
}

// ---------------------------------------------------------------------------
// 28. `aproxy restore`：崩溃实例一键复活、幂等跳过、优雅停止后不再恢复
//
// restore 记录（run/<端口>.restore）语义：守护 bind 成功写入、优雅退出删除、
// 崩溃/系统重启保留。此处以 taskkill /F 模拟崩溃（目标仅为本测试拉起的守护）。
// ---------------------------------------------------------------------------
#[test]
fn restore_recovers_crashed_daemon_and_is_idempotent() {
    let port = daemon_test_port(7);
    let dir = tempfile::tempdir().unwrap();
    let cfg_file = dir.path().join("restore.toml");
    std::fs::write(
        &cfg_file,
        format!(
            "base_url = \"https://restore-test.example.com\"\nlisten_addr = \"127.0.0.1:{port}\"\n"
        ),
    )
    .unwrap();
    let exe = env!("CARGO_BIN_EXE_aproxy");
    let _ = Command::new(exe).args(["stop", &port.to_string()]).output();
    let _guard = DaemonGuard { exe, port };
    let restore_path = aproxy::daemon::restore_file_path(&format!("127.0.0.1:{port}"));

    // 正常后台启动：bind 成功即写恢复记录
    let pid = aproxy::daemon::spawn_detached(
        std::path::Path::new(exe),
        &[
            "--config".to_string(),
            cfg_file.display().to_string(),
            "--daemon-child".to_string(),
        ],
    )
    .expect("spawn 守护子进程失败");
    assert!(wait_daemon_ready(port), "守护未就绪 (pid {pid})");
    assert!(
        restore_path.exists(),
        "守护启动后应写入恢复记录: {}",
        restore_path.display()
    );

    // 模拟崩溃：强杀守护进程（不经过 IPC 优雅退出），恢复记录应残留
    let killed = Command::new("taskkill")
        .args(["/F", "/PID", &pid.to_string()])
        .output()
        .expect("taskkill 执行失败");
    assert!(killed.status.success(), "taskkill 应成功杀死测试守护");
    let mut gone = false;
    for _ in 0..50 {
        if !process_alive(pid) {
            gone = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(gone, "测试守护 (pid {pid}) 应已被强杀");
    assert!(restore_path.exists(), "崩溃后恢复记录应保留");

    // restore：一键拉起崩溃实例
    let out = Command::new(exe).arg("restore").output().unwrap();
    assert!(out.status.success(), "restore 应成功");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("已恢复") && stdout.contains(&port.to_string()),
        "restore 应恢复实例: {stdout}"
    );
    assert!(wait_daemon_ready(port), "恢复后的实例应就绪");

    // 幂等：再次 restore 时已在运行 → 跳过而非报错/重复启动
    let out = Command::new(exe).arg("restore").output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("已在运行，跳过"), "幂等跳过: {stdout}");

    // 优雅停止：恢复记录被删除，此后 restore 无事可做（静默成功，开机自启友好）
    let out = Command::new(exe)
        .args(["stop", &port.to_string()])
        .output()
        .unwrap();
    assert!(out.status.success(), "stop 恢复实例应成功: {stdout}");
    assert!(!restore_path.exists(), "优雅停止后恢复记录应被删除");
    let out = Command::new(exe).arg("restore").output().unwrap();
    assert!(out.status.success(), "无记录时 restore 应静默成功");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("没有需要恢复的实例"), "实际: {stdout}");
}
