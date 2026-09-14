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
// 2b. 压缩/二进制错误体触发重试，且日志预览不产生乱码
//
// 回归：上游（如 Cloudflare）曾返回 zstd 压缩的错误页，错误预览被
// from_utf8_lossy 渲染成整片乱码污染日志。现要求：二进制体仍重试（压缩体
// 不是可判定为成功的响应），预览输出为 hex 摘要。
// ---------------------------------------------------------------------------
#[tokio::test]
async fn retry_on_binary_error_body_and_clean_log_preview() {
    let counter = Arc::new(AtomicUsize::new(0));
    let c2 = counter.clone();

    let upstream = Router::new().route(
        "/v1/messages",
        any(move |_req: axum::extract::Request| {
            let c = c2.clone();
            async move {
                let n = c.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    // zstd magic 开头的伪压缩错误体（Content-Type 模糊）
                    let mut body = vec![0x28u8, 0xb5, 0x2f, 0xfd];
                    body.extend((0..100u8).map(|i| i.wrapping_mul(37)));
                    (StatusCode::BAD_GATEWAY, body).into_response()
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
    // 2 次调用 = 1 次压缩错误 + 1 次成功（压缩体按可重试状态码重试）
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
// 16. 超过 max_body_mb 的请求体 → 413（显式设小上限；默认 128 MB 太大，
//     真发 128MiB+1 的测试体既慢又会触发上游/客户端的其他上限）
// ---------------------------------------------------------------------------
#[tokio::test]
async fn oversized_request_returns_413() {
    let upstream = Router::new().route("/v1/x", any(|| async { "ok" }));
    let (upstream_url, _h1) = bind_random_router(upstream).await;
    let mut cfg = proxy_config_for(&upstream_url);
    cfg.max_body_mb = Some(1); // 1 MiB 上限
    let proxy_state = AppState::new(cfg);
    let (proxy_url, _h2) = bind_random_router(aproxy::proxy::router(proxy_state)).await;

    let big = vec![b'a'; 1024 * 1024 + 1];
    let client = local_client();
    let resp = client
        .post(format!("{proxy_url}/v1/x"))
        .body(big)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 413, "超过 max_body_mb 上限应返回 413");
}

// ---------------------------------------------------------------------------
// 16b. 大请求体走磁盘缓存（disk_cache 默认开）：> 1 MiB 驻留阈值的请求体
//      溢写临时文件，上游收到的字节必须与发送完全一致（重试重放路径同样
//      从文件流式读取），且请求结束后临时文件被清理
// ---------------------------------------------------------------------------
#[tokio::test]
async fn large_request_body_spools_to_disk_and_replays() {
    use tempfile::TempDir;

    let received = Arc::new(Mutex::new(Vec::<u8>::new()));
    let received_clone = received.clone();
    let upstream = Router::new().route(
        "/v1/upload",
        any(move |req: axum::extract::Request| {
            let received = received_clone.clone();
            async move {
                let bytes = axum::body::to_bytes(req.into_body(), 8 * 1024 * 1024)
                    .await
                    .unwrap();
                received.lock().unwrap().extend_from_slice(&bytes);
                (StatusCode::OK, "stored").into_response()
            }
        }),
    );
    let (upstream_url, _h1) = bind_random_router(upstream).await;

    // spool 目录注入 tempdir：断言临时文件生命周期（请求结束后目录应为空）
    let spool_dir = TempDir::new().unwrap();
    let mut cfg = proxy_config_for(&upstream_url);
    cfg.spool_dir_override = Some(spool_dir.path().to_path_buf());
    let proxy_state = AppState::new(cfg);
    let (proxy_url, _h2) = bind_random_router(aproxy::proxy::router(proxy_state)).await;

    // 2 MiB：超过 1 MiB 驻留阈值，必然溢写磁盘
    let big: Vec<u8> = (0..2 * 1024 * 1024).map(|i| (i % 251) as u8).collect();
    let client = local_client();
    let resp = client
        .post(format!("{proxy_url}/v1/upload"))
        .body(big.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        received.lock().unwrap().as_slice(),
        &big,
        "上游收到的字节必须与发送一致"
    );
    // 请求结束（RequestBody Drop）后临时文件应已清理
    let leftover: Vec<_> = std::fs::read_dir(spool_dir.path())
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    assert!(
        leftover.is_empty(),
        "请求结束后 spool 目录不应残留临时文件: {:?}",
        leftover.iter().map(|e| e.path()).collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// 16c. 大响应走磁盘缓存：> 1 MiB 的响应 spool 溢写，客户端收到的字节必须
//      与上游完全一致（回放从临时文件流式读出），结束后无残留
// ---------------------------------------------------------------------------
#[tokio::test]
async fn large_response_spools_to_disk_and_replays() {
    use tempfile::TempDir;

    // 3 MiB 伪随机（避免可压缩）SSE 载荷
    let payload: Vec<u8> = (0..3 * 1024 * 1024).map(|i| (i % 251) as u8).collect();
    let payload_clone = payload.clone();
    let upstream = Router::new().route(
        "/v1/large",
        any(move || {
            let payload = payload_clone.clone();
            async move { (StatusCode::OK, payload).into_response() }
        }),
    );
    let (upstream_url, _h1) = bind_random_router(upstream).await;

    let spool_dir = TempDir::new().unwrap();
    let mut cfg = proxy_config_for(&upstream_url);
    cfg.spool_dir_override = Some(spool_dir.path().to_path_buf());
    let proxy_state = AppState::new(cfg);
    let (proxy_url, _h2) = bind_random_router(aproxy::proxy::router(proxy_state)).await;

    let client = local_client();
    let resp = client
        .get(format!("{proxy_url}/v1/large"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.bytes().await.unwrap();
    assert_eq!(
        body.as_ref(),
        &payload[..],
        "磁盘回放字节必须与上游完全一致"
    );
    // 回放完成（流 EOF 删除）后 spool 目录应为空
    let leftover: Vec<_> = std::fs::read_dir(spool_dir.path())
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    assert!(
        leftover.is_empty(),
        "回放结束后 spool 目录不应残留临时文件: {:?}",
        leftover.iter().map(|e| e.path()).collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// 16d. 磁盘模式下大响应流尾错误仍触发重试（StreamErrorScanner 增量扫描：
//      流尾 error 事件必须命中——与 is_stream_error_body 的测试锚定对齐）
// ---------------------------------------------------------------------------
#[tokio::test]
async fn disk_spool_stream_trailing_error_retries() {
    use tempfile::TempDir;

    let counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = counter.clone();
    // 响应 > 1 MiB（必然溢写磁盘），错误事件在流尾
    let padding = "x".repeat(1024 * 1024 + 42);
    let sse =
        format!("data: {{\"type\":\"content\"}}\n\n{padding}\ndata: {{\"error\":\"boom\"}}\n");
    let sse_clone = sse.clone();
    let upstream = Router::new().route(
        "/v1/sse",
        any(move || {
            let c = counter_clone.clone();
            let sse = sse_clone.clone();
            async move {
                let n = c.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    (StatusCode::OK, sse).into_response()
                } else {
                    (StatusCode::OK, "fixed").into_response()
                }
            }
        }),
    );
    let (upstream_url, _h1) = bind_random_router(upstream).await;

    let spool_dir = TempDir::new().unwrap();
    let mut cfg = proxy_config_for(&upstream_url);
    cfg.spool_dir_override = Some(spool_dir.path().to_path_buf());
    let proxy_state = AppState::new(cfg);
    let (proxy_url, _h2) = bind_random_router(aproxy::proxy::router(proxy_state)).await;

    let client = local_client();
    let resp = client
        .get(format!("{proxy_url}/v1/sse"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "fixed");
    assert_eq!(counter.load(Ordering::SeqCst), 2, "流尾错误应恰好重试一次");
}

// ---------------------------------------------------------------------------
// 17. 8 KiB 块边界：恰好 2×8KiB 的 body 回放字节完全一致
// ---------------------------------------------------------------------------

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
// 22. 仅转发模式：门控分块 mock（下面 22a/22b 两条对照测试共用）
//
// 「首字节是否真·增量转发」需要一个能**停在半途等外部放行**的上游：吐第一块
// 之后挂起，测试确认收到首块才放行，再吐第二块。默认模式必须 spool 完整响应
// 才回放，故放行前一个字节都拿不到；仅转发模式边收边发，放行前就该拿到首块。
// 两条测试跑同一个 mock，互为对照——单独一条无法证明断言有鉴别力。
// ---------------------------------------------------------------------------

/// 门控分块 mock：任何匹配 `path` 的请求都在响应里先吐 `first`，随后**停等**
/// 调用方通过返回的 oneshot 放行，再吐 `second` 并正常结束。
///
/// 返回 `(router, release)`：`release.send(())` 即放行。
fn gated_chunk_router(
    path: &'static str,
    first: &'static [u8],
    second: &'static [u8],
) -> (Router, tokio::sync::oneshot::Sender<()>) {
    let (release, rx) = tokio::sync::oneshot::channel::<()>();
    // oneshot::Receiver 不是 Clone：门控状态放进 Arc<Mutex<Option<..>>>，
    // 首个请求 take() 出来 await（只放行一次）。
    let gate: Arc<tokio::sync::Mutex<Option<tokio::sync::oneshot::Receiver<()>>>> =
        Arc::new(tokio::sync::Mutex::new(Some(rx)));

    let router = Router::new().route(
        path,
        any(move |_req: axum::extract::Request| {
            let gate = gate.clone();
            async move {
                // unfold 的流是一次性的，故每次请求都在 handler 内新建
                let stream = futures_util::stream::unfold(0u8, move |step| {
                    let gate = gate.clone();
                    async move {
                        match step {
                            0 => Some((Ok::<Bytes, std::io::Error>(Bytes::from_static(first)), 1)),
                            1 => {
                                let rx = gate.lock().await.take();
                                if let Some(rx) = rx {
                                    let _ = rx.await;
                                }
                                Some((Ok(Bytes::from_static(second)), 2))
                            }
                            _ => None,
                        }
                    }
                });
                axum::response::Response::builder()
                    .status(StatusCode::OK)
                    .header("content-type", "text/event-stream")
                    .body(axum::body::Body::from_stream(stream))
                    .unwrap()
            }
        }),
    );
    (router, release)
}

// ---------------------------------------------------------------------------
// 22a. 仅转发模式：真·增量流——上游放行之前客户端就应拿到首块
// ---------------------------------------------------------------------------
#[tokio::test]
async fn forward_only_streams_incrementally() {
    use futures_util::StreamExt;

    let (upstream, release) =
        gated_chunk_router("/v1/messages", b"data: chunk-A\n\n", b"data: chunk-B\n\n");
    let (upstream_url, _h1) = bind_random_router(upstream).await;

    let mut cfg = proxy_config_for(&upstream_url);
    cfg.forward_only = Some(true);
    let proxy_state = AppState::new(cfg);
    let (proxy_url, _h2) = bind_random_router(aproxy::proxy::router(proxy_state)).await;

    let client = local_client();
    let resp = client
        .post(format!("{proxy_url}/v1/messages"))
        .body("ping")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "上游 200 应原样透传");

    let mut stream = resp.bytes_stream();
    // 关键断言：**放行之前**就必须收到首块。上游此刻还挂在第 1 步的 await 上，
    // 能拿到 chunk-A 只可能来自「边收边发」。
    let first = tokio::time::timeout(Duration::from_secs(3), stream.next()).await;
    let first = first.unwrap_or_else(|e| {
        panic!("仅转发模式应在上游放行前就转发首块，但 3 秒内未收到任何字节: {e}")
    });
    let first = first
        .expect("响应流不应在首块前结束")
        .expect("首块不应是错误");
    assert_eq!(
        first.as_ref(),
        b"data: chunk-A\n\n",
        "首块字节必须原样到达客户端"
    );

    // 放行后应拿到第二块并正常收尾
    release.send(()).unwrap();
    let mut rest = Vec::new();
    while let Some(chunk) = stream.next().await {
        rest.extend_from_slice(&chunk.expect("第二块不应是错误"));
    }
    assert_eq!(
        rest.as_slice(),
        b"data: chunk-B\n\n",
        "放行后应收到第二块并正常收尾"
    );
}

// ---------------------------------------------------------------------------
// 22b. 负向对照：同款 mock 走**默认（非仅转发）模式**——必须 spool 完整响应
//      才回放，3 秒内拿不到任何字节。没有这一条，22a 的断言无法证明有鉴别力
//      （可能只是「恰好为真」）。
// ---------------------------------------------------------------------------
#[tokio::test]
async fn default_mode_buffers_whole_response_before_first_byte() {
    use futures_util::StreamExt;

    let (upstream, release) =
        gated_chunk_router("/v1/messages", b"data: chunk-A\n\n", b"data: chunk-B\n\n");
    let (upstream_url, _h1) = bind_random_router(upstream).await;

    // 显式关掉仅转发（也是内置默认），确保走的是缓冲 + spool 回放路径
    let mut cfg = proxy_config_for(&upstream_url);
    cfg.forward_only = Some(false);
    let proxy_state = AppState::new(cfg);
    let (proxy_url, _h2) = bind_random_router(aproxy::proxy::router(proxy_state)).await;

    let client = local_client();
    // 默认模式下连响应头都要等 spool 完成才发（build_replay_response 在读完
    // 整个上游响应之后才调用），故 send() 本身就该超时——两者一并用超时包住
    let fut = client
        .post(format!("{proxy_url}/v1/messages"))
        .body("ping")
        .send();
    let got = tokio::time::timeout(Duration::from_secs(3), async move {
        let resp = fut.await.unwrap();
        resp.bytes_stream().next().await
    })
    .await;
    assert!(
        got.is_err(),
        "默认模式必须 spool 完整响应才回放，3 秒内不得有任何字节（含响应头）到达客户端；\
         实际拿到: {got:?}"
    );

    // 放行让上游收尾，避免遗留挂起的任务
    release.send(()).unwrap();
}

// ---------------------------------------------------------------------------
// 22c. 仅转发模式：上游 5xx + 错误 JSON 一律不重试，原样透传给客户端
//      （默认模式会对这种响应重试——见测试 1/2）
// ---------------------------------------------------------------------------
#[tokio::test]
async fn forward_only_does_not_retry_on_error_status() {
    const ERROR_BODY: &str =
        r#"{"type":"error","error":{"type":"overloaded_error","message":"busy"}}"#;

    let counter = Arc::new(AtomicUsize::new(0));
    let c2 = counter.clone();
    let upstream = Router::new().route(
        "/v1/messages",
        any(move || {
            let c = c2.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    [("content-type", "application/json")],
                    ERROR_BODY,
                )
                    .into_response()
            }
        }),
    );
    let (upstream_url, _h1) = bind_random_router(upstream).await;

    let mut cfg = proxy_config_for(&upstream_url);
    cfg.forward_only = Some(true);
    let proxy_state = AppState::new(cfg);
    let (proxy_url, _h2) = bind_random_router(aproxy::proxy::router(proxy_state)).await;

    let client = local_client();
    let resp = client
        .post(format!("{proxy_url}/v1/messages"))
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 500, "上游 500 应原样透传");
    let body = resp.bytes().await.unwrap();
    assert_eq!(
        body.as_ref(),
        ERROR_BODY.as_bytes(),
        "错误体必须原样到达客户端——既不被拦截判定，也不被重试替换"
    );
    assert_eq!(
        counter.load(Ordering::SeqCst),
        1,
        "仅转发模式不得重试：上游应被调用恰好 1 次"
    );
}

// ---------------------------------------------------------------------------
// 22d. 仅转发模式：api_key / override_headers 照常生效，路径与查询串透传
//      （本模式的用户场景本体：本地改写鉴权头 + 真流式）
// ---------------------------------------------------------------------------
#[tokio::test]
async fn forward_only_applies_header_overrides() {
    let upstream = Router::new().route(
        "/v1/echo",
        any(|req: axum::extract::Request| async move {
            let pick = |name: &str| {
                req.headers()
                    .get(name)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_string()
            };
            let uri = req.uri().to_string();
            axum::Json(serde_json::json!({
                "uri": uri,
                "auth": pick("authorization"),
                "ov": pick("x-override"),
            }))
            .into_response()
        }),
    );
    let (upstream_url, _h1) = bind_random_router(upstream).await;

    let mut cfg = proxy_config_for(&upstream_url);
    cfg.forward_only = Some(true);
    cfg.api_key = Some("sk-forward".to_string());
    cfg.override_headers
        .insert("x-override".to_string(), "forced".to_string());
    let proxy_state = AppState::new(cfg);
    let (proxy_url, _h2) = bind_random_router(aproxy::proxy::router(proxy_state)).await;

    let client = local_client();
    let resp = client
        .get(format!("{proxy_url}/v1/echo?a=1&b=two"))
        .header("x-override", "client-value")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["auth"], "Bearer sk-forward",
        "api_key 应转为 Bearer authorization 到达上游"
    );
    assert_eq!(
        body["ov"], "forced",
        "override_headers 应无条件覆盖客户端值"
    );
    assert_eq!(
        body["uri"], "/v1/echo?a=1&b=two",
        "路径与查询串必须照常透传（仅转发模式只在请求体与响应体上改变行为）"
    );
}

// ---------------------------------------------------------------------------
// 22e. 仅转发模式：上游响应流中断 → 直接截断（不注入上游未发出的字节）、
//      不重试、不挂起。裸 TCP 上游：axum 无法模拟「响应中途掐断」。
// ---------------------------------------------------------------------------
#[tokio::test]
async fn forward_only_truncates_on_upstream_abort() {
    use futures_util::StreamExt;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let conns = Arc::new(AtomicUsize::new(0));
    let c2 = conns.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            c2.fetch_add(1, Ordering::SeqCst);

            // 先把请求读干净再回。裸 TCP 上游不会自动消费请求体，若接收缓冲
            // 仍有未读字节，close 时会对端 RST，把刚写出的响应字节一起丢掉。
            let mut buf = [0u8; 4096];
            loop {
                match tokio::time::timeout(Duration::from_millis(150), sock.read(&mut buf)).await {
                    Ok(Ok(0)) | Ok(Err(_)) | Err(_) => break,
                    Ok(Ok(_)) => continue,
                }
            }

            // 写出一个**完整** chunk 帧后不发终止块直接断开：chunked framing 下
            // FIN 提前到达即「上游响应流中断」
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
        }
    });

    let mut cfg = proxy_config_for(&format!("http://{addr}"));
    cfg.forward_only = Some(true);
    let proxy_state = AppState::new(cfg);
    let (proxy_url, _h2) = bind_random_router(aproxy::proxy::router(proxy_state)).await;

    let client = local_client();
    let resp = client
        .post(format!("{proxy_url}/v1/messages"))
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "上游已发出响应头，状态码应原样透传");

    let mut stream = resp.bytes_stream();
    let mut got = Vec::new();
    let outcome = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(c) => got.extend_from_slice(&c),
                Err(_) => return "err",
            }
        }
        "eof"
    })
    .await;
    let outcome = outcome
        .unwrap_or_else(|e| panic!("上游中途断开后响应流必须立刻结束，不得在 5 秒内挂起: {e}"));
    assert_eq!(
        outcome, "err",
        "半截 chunked 帧应表现为响应流错误（截断），而不是静默 EOF"
    );
    assert_eq!(
        got.as_slice(),
        b"data: {\"partial\":true}\n\n",
        "上游已发出的字节必须原样转发；截断处不得注入任何上游未发出的字节"
    );
    assert_eq!(
        conns.load(Ordering::SeqCst),
        1,
        "仅转发模式不得重试：上游应被连接恰好 1 次"
    );
}

// ---------------------------------------------------------------------------
// 22f. 仅转发模式：全程不 spool。> 1 MiB 请求体 + > 1 MiB 响应往返期间与结束
//      后各做一次目录快照，spool 目录必须始终为空（默认模式这两侧都会溢写磁盘）。
// ---------------------------------------------------------------------------
#[tokio::test]
async fn forward_only_never_spools() {
    use futures_util::StreamExt;
    use tempfile::TempDir;

    // 24 × 64 KiB = 1.5 MiB，必然越过 1 MiB 内存驻留阈值
    const CHUNKS: usize = 24;
    const CHUNK: usize = 64 * 1024;

    let upstream = Router::new().route(
        "/v1/upload",
        any(|req: axum::extract::Request| async move {
            // 请求体（1.5 MiB）必须整份读掉，否则上游侧背压会把发送卡住
            let _ = axum::body::to_bytes(req.into_body(), 16 * 1024 * 1024)
                .await
                .expect("上游应能读完整请求体");
            // 分块 + 小延迟，保证客户端读第一块时响应确实还在途（而非已被
            // 上游一次性塞进缓冲区），「流式读取途中」的快照才有意义
            let stream = futures_util::stream::unfold(0usize, |i| async move {
                if i >= CHUNKS {
                    return None;
                }
                tokio::time::sleep(Duration::from_millis(15)).await;
                Some((
                    Ok::<Bytes, std::io::Error>(Bytes::from(vec![(i % 251) as u8; CHUNK])),
                    i + 1,
                ))
            });
            axum::body::Body::from_stream(stream)
        }),
    );
    let (upstream_url, _h1) = bind_random_router(upstream).await;

    let spool_dir = TempDir::new().unwrap();
    let mut cfg = proxy_config_for(&upstream_url);
    cfg.forward_only = Some(true);
    cfg.spool_dir_override = Some(spool_dir.path().to_path_buf());
    let proxy_state = AppState::new(cfg);
    let (proxy_url, _h2) = bind_random_router(aproxy::proxy::router(proxy_state)).await;

    let assert_spool_empty = |tag: &str| {
        let entries: Vec<_> = std::fs::read_dir(spool_dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.path())
            .collect();
        assert!(
            entries.is_empty(),
            "{tag}：仅转发模式不得产生任何 spool 临时文件，实际: {entries:?}"
        );
    };

    let big: Vec<u8> = (0..3 * 512 * 1024).map(|i| (i % 251) as u8).collect(); // 1.5 MiB
    let client = local_client();
    let resp = client
        .post(format!("{proxy_url}/v1/upload"))
        .body(big)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let mut stream = resp.bytes_stream();
    let mut total = 0usize;
    let mut mid_checked = false;
    while let Some(chunk) = stream.next().await {
        total += chunk.expect("响应流不应出错").len();
        if !mid_checked {
            mid_checked = true;
            assert!(
                total < CHUNKS * CHUNK,
                "首块之后响应应仍在流式中（实际已收完 {total} 字节），否则「途中」快照无意义"
            );
            assert_spool_empty("流式读取途中");
        }
    }
    assert_eq!(total, CHUNKS * CHUNK, "响应字节总数应与上游发出的完全一致");
    assert_spool_empty("响应结束后");
}

// ---------------------------------------------------------------------------
// 22g. 仅转发模式：8 MiB 请求体/响应体字节保真，且远低于默认 128 MiB 上限的
//      大流量不得误触 413（流式计数上限只在真正越界时才触发）
// ---------------------------------------------------------------------------
#[tokio::test]
async fn forward_only_large_body_streams_through() {
    const SIZE: usize = 8 * 1024 * 1024; // 8 MiB

    let received = Arc::new(Mutex::new(Vec::<u8>::new()));
    let received_clone = received.clone();
    let resp_payload: Vec<u8> = (0..SIZE).map(|i| (i % 253) as u8).collect();
    let resp_clone = resp_payload.clone();
    let upstream = Router::new().route(
        "/v1/upload",
        any(move |req: axum::extract::Request| {
            let received = received_clone.clone();
            let resp_payload = resp_clone.clone();
            async move {
                let bytes = axum::body::to_bytes(req.into_body(), 32 * 1024 * 1024)
                    .await
                    .expect("上游应能读完整 8 MiB 请求体");
                received.lock().unwrap().extend_from_slice(&bytes);
                axum::body::Body::from(resp_payload)
            }
        }),
    );
    let (upstream_url, _h1) = bind_random_router(upstream).await;

    let mut cfg = proxy_config_for(&upstream_url);
    cfg.forward_only = Some(true);
    let proxy_state = AppState::new(cfg);
    let (proxy_url, _h2) = bind_random_router(aproxy::proxy::router(proxy_state)).await;

    let big: Vec<u8> = (0..SIZE).map(|i| (i % 251) as u8).collect();
    let client = local_client();
    let resp = client
        .post(format!("{proxy_url}/v1/upload"))
        .body(big.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        200,
        "8 MiB 远低于默认 128 MiB 上限，不得误触 413"
    );
    let got = resp.bytes().await.unwrap();
    assert_eq!(got.len(), SIZE, "响应字节数应保真（期望 {SIZE}）");
    assert_eq!(
        got.as_ref(),
        resp_payload.as_slice(),
        "响应字节必须与上游发出的完全一致"
    );
    let upstream_saw = received.lock().unwrap().clone();
    assert_eq!(
        upstream_saw.len(),
        SIZE,
        "上游收到的请求体字节数应保真（期望 {SIZE}）"
    );
    assert_eq!(
        upstream_saw.as_slice(),
        big.as_slice(),
        "上游收到的请求体必须与客户端发送的完全一致"
    );
}

// ---------------------------------------------------------------------------
// 22h. 仅转发模式：max_body_mb 仍强制——流式途中计数越界即 413
// ---------------------------------------------------------------------------
#[tokio::test]
async fn forward_only_enforces_max_body_mb() {
    let upstream = Router::new().route(
        "/v1/upload",
        any(|req: axum::extract::Request| async move {
            // 上游必须**读完整个请求体**才回响应：这样请求体适配器一旦产出
            // Err（超限），上游永不给出响应，reqwest::send() 必然以错误收场，
            // 413 的判定就是确定性的，而非与「上游抢答」赛跑
            match axum::body::to_bytes(req.into_body(), 64 * 1024 * 1024).await {
                Ok(_) => (StatusCode::OK, "stored").into_response(),
                Err(_) => (StatusCode::BAD_REQUEST, "incomplete body").into_response(),
            }
        }),
    );
    let (upstream_url, _h1) = bind_random_router(upstream).await;

    let mut cfg = proxy_config_for(&upstream_url);
    cfg.forward_only = Some(true);
    cfg.max_body_mb = Some(1); // 1 MiB 上限
    let proxy_state = AppState::new(cfg);
    let (proxy_url, _h2) = bind_random_router(aproxy::proxy::router(proxy_state)).await;

    let big = vec![b'a'; 2 * 1024 * 1024];
    let client = local_client();
    let resp = client
        .post(format!("{proxy_url}/v1/upload"))
        .body(big)
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        413,
        "仅转发模式流式计数越过 max_body_mb 后应返回 413"
    );
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("请求体超出上限"),
        "413 文案应与缓冲路径共用（期望含「请求体超出上限」），实际: {body}"
    );
}

// ---------------------------------------------------------------------------
// 22i. 仅转发模式：上游不可达 → 502 + 不重试 + 失败进入观测
//
// 规格原文是「上游请求失败 → 502 + note_upstream_failure + 不重试」，此前零覆盖。
//
// 鉴别力（实现退化成什么样会红）：
// - 若 forward_only 分支被移出 proxy_handler（落回常规缓冲路径），常规模式对
//   不可达上游是**无限重试**，下面的 10 秒超时直接红；
// - 若去掉 note_upstream_failure 调用，last_error 保持 None → 红；
// - 若 retries_total 被计入（把首轮当重试、或误入重试循环），计数断言红；
// - 状态码改成 500/504 或去掉原因文案 → 状态码/正文断言红。
//
// 观测经**进程内同一个 Arc**（AppState::stats）读取：这正是热路径写入的那一份。
// 「serve_forever 是否把它共享进 IPC」属于接线问题，由进程级测试 31/32 覆盖，
// 此处不重复（那条才是接线缺陷的盲区）。
// ---------------------------------------------------------------------------
#[tokio::test]
async fn forward_only_unreachable_upstream_returns_502() {
    // 取一个刚释放的端口：bind 到 0 拿到内核分配的端口后立刻 drop，此后对该
    // 端口的 connect 必然 ECONNREFUSED（环回上无监听者），且失败是即时的——
    // 不依赖 connect_timeout 收敛
    let dead_addr = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        drop(l);
        addr
    };

    let mut cfg = proxy_config_for(&format!("http://{dead_addr}"));
    cfg.forward_only = Some(true);
    let state = AppState::new(cfg);
    // 路由拿走一份 AppState，这里留一份读观测：两者共享同一组 Arc
    let probe = state.clone();
    let (proxy_url, _h2) = bind_random_router(aproxy::proxy::router(state)).await;

    let client = local_client();
    let resp = tokio::time::timeout(
        Duration::from_secs(10),
        client
            .post(format!("{proxy_url}/v1/messages"))
            .body("{}")
            .send(),
    )
    .await
    .expect("仅转发模式的上游失败必须立刻终结；10 秒内没有响应说明落进了常规模式的重试循环")
    .unwrap();

    assert_eq!(resp.status(), 502, "上游请求失败应回 502");
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("上游请求失败"),
        "502 正文应带上失败原因，实际: {body}"
    );
    assert!(
        body.contains("仅转发模式不重试"),
        "502 正文应指明本模式不重试（否则排障会指望它重试），实际: {body}"
    );

    assert_eq!(
        probe.stats.requests_total.load(Ordering::Relaxed),
        1,
        "应恰好计 1 次客户端请求"
    );
    assert_eq!(
        probe.stats.retries_total.load(Ordering::Relaxed),
        0,
        "仅转发模式不得重试：重试计数必须为 0（常规模式对不可达上游会无限重试）"
    );
    let last_error = probe.stats.last_error.lock().unwrap().clone();
    let msg = last_error.map(|(m, _)| m).unwrap_or_default();
    assert!(
        msg.contains("上游请求失败"),
        "失败必须记入观测（status 的「最近错误」），实际: {msg:?}"
    );
}

// ---------------------------------------------------------------------------
// 22i-2. 仅转发模式：网络失败在上游侧只有**一次**尝试（监听器直接计数）
//
// 22i 指向的端口无人监听，「尝试了几次」在那一侧不可观测——那条测试能证明
// 「没有落进无限重试循环」（10 秒超时是硬约束），但不能证明「恰好一次」。
// 这里换成**接受连接后立刻关闭、不回任何字节**的计数监听器：每次尝试必然
// 建立一条连接，于是连接数就是尝试数。
//
// 鉴别力：若 forward_only_proxy 被接回重试循环（哪怕只重试一次），计数变 2 → 红；
// 若错误分支被改成「失败当成功回放」，502 断言红。
// ---------------------------------------------------------------------------
#[tokio::test]
async fn forward_only_makes_exactly_one_upstream_attempt_on_network_failure() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let counter = attempts.clone();
    // 接受即关闭：不给状态行、不给响应体，对代理就是 send() 失败
    let acceptor = tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((sock, _)) => {
                    counter.fetch_add(1, Ordering::SeqCst);
                    drop(sock);
                }
                // 监听器出错即收工，避免测试结束后任务空转
                Err(_) => return,
            }
        }
    });

    let mut cfg = proxy_config_for(&format!("http://{addr}"));
    cfg.forward_only = Some(true);
    let state = AppState::new(cfg);
    let (proxy_url, _h2) = bind_random_router(aproxy::proxy::router(state)).await;

    let client = local_client();
    let resp = tokio::time::timeout(
        Duration::from_secs(10),
        client.get(format!("{proxy_url}/v1/attempts")).send(),
    )
    .await
    .expect("上游连接被立即关闭时必须立刻终结；10 秒内没有响应说明进了重试循环")
    .unwrap();
    assert_eq!(resp.status(), 502, "上游网络失败应回 502");
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("上游请求失败"),
        "502 正文应带上失败原因，实际: {body}"
    );
    assert_eq!(
        attempts.load(Ordering::SeqCst),
        1,
        "网络失败在上游侧必须只尝试一次（仅转发模式不重试）"
    );

    acceptor.abort();
}

// ---------------------------------------------------------------------------
// 22j. 仅转发模式：上游定长响应经代理后**仍带 content-length**（不退化为 chunked）
//
// 规格「保留 content-length」此前只有 helper 单测（is_hop_response_header 的
// 直接调用），没有端到端断言——调用点写错（比如传错 upstream_has_te）照样能过。
//
// 鉴别力：断言落在**响应头**上。若调用点退回 is_hop_header（无条件剥 CL）或
// upstream_has_te 传成 true，CL 被剥掉，下游只能按 chunked 分帧（from_stream
// 的 body 无精确长度），客户端拿到的响应头里就没有 content-length → 红。
// 只断言 body 字节的写法抓不到这个退化。
// ---------------------------------------------------------------------------
#[tokio::test]
async fn forward_only_preserves_content_length_on_fixed_length_response() {
    /// 定长响应体：axum 对 &'static str 自动写入 content-length
    const BODY: &str = r#"{"ok":true,"model":"test","n":1234567890}"#;

    let upstream = Router::new().route("/v1/fixed", any(|| async { (StatusCode::OK, BODY) }));
    let (upstream_url, _h1) = bind_random_router(upstream).await;

    let mut cfg = proxy_config_for(&upstream_url);
    cfg.forward_only = Some(true);
    let proxy_state = AppState::new(cfg);
    let (proxy_url, _h2) = bind_random_router(aproxy::proxy::router(proxy_state)).await;

    let client = local_client();
    let resp = client
        .get(format!("{proxy_url}/v1/fixed"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let cl = resp
        .headers()
        .get(axum::http::header::CONTENT_LENGTH)
        .map(|v| v.to_str().unwrap().to_string());
    assert_eq!(
        cl.as_deref(),
        Some(BODY.len().to_string().as_str()),
        "上游定长响应的 content-length 必须原样保留（字节未经变换，长度仍精确）；\
         实际响应头: {:?}",
        resp.headers()
    );
    assert!(
        resp.headers()
            .get(axum::http::header::TRANSFER_ENCODING)
            .is_none(),
        "保留 content-length 后不应退化为 chunked 分帧，实际响应头: {:?}",
        resp.headers()
    );
    assert_eq!(resp.text().await.unwrap(), BODY, "响应体应原样透传");
}

// ---------------------------------------------------------------------------
// 22k. 仅转发模式：响应流式期间刷新活动时间戳
//
// 回归锚点（刚修的行为）：本模式下小时级长流是常态，若响应流路径不刷新
// last_activity_secs，一个正在传输的实例会被 stop idle / status --idle 判为
// 闲置并强退——恰在传输中途被掐断，正砸在本模式的存在理由上。
//
// 构造成确定性的（不依赖真实时钟竞态）：门控上游在两次 chunk 之间挂起，客户端
// 收到首块后把活动时间戳**显式拨回远古**（模拟上游长时间无输出、idle 已越线），
// 此时上游仍被门控，唯一可能刷新它的就是「放行后的那个 chunk 被转发」。
//
// 鉴别力：若响应流 map 里的 store 被删（或只在请求入口刷新一次），时间戳会
// 停在 1 → 红。
// ---------------------------------------------------------------------------
#[tokio::test]
async fn forward_only_refreshes_activity_while_response_streams() {
    use futures_util::StreamExt;

    let (upstream, release) =
        gated_chunk_router("/v1/messages", b"data: chunk-A\n\n", b"data: chunk-B\n\n");
    let (upstream_url, _h1) = bind_random_router(upstream).await;

    let mut cfg = proxy_config_for(&upstream_url);
    cfg.forward_only = Some(true);
    let state = AppState::new(cfg);
    let probe = state.clone();
    let (proxy_url, _h2) = bind_random_router(aproxy::proxy::router(state)).await;

    let client = local_client();
    let resp = client
        .post(format!("{proxy_url}/v1/messages"))
        .body("ping")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let mut stream = resp.bytes_stream();
    let first = stream.next().await.unwrap().unwrap();
    assert_eq!(first.as_ref(), b"data: chunk-A\n\n");

    // 拨回远古：此后到 chunk-B 到达之前，上游被门控挂住，不可能有任何刷新
    // （请求入口那次 store 早已发生，重试轮也不存在——本模式无重试）
    probe.last_activity_secs.store(1, Ordering::Relaxed);

    release.send(()).unwrap();
    let second = stream.next().await.unwrap().unwrap();
    assert_eq!(second.as_ref(), b"data: chunk-B\n\n");

    let activity = probe.last_activity_secs.load(Ordering::Relaxed);
    assert!(
        activity > 1,
        "转发 chunk 时必须刷新活动时间戳，否则长流实例会被 stop idle 误杀；实际仍为 {activity}"
    );
}

// ---------------------------------------------------------------------------
// 22l. 仅转发模式：客户端上传途中断开 → 400，且**不**记入「上游请求失败」
//
// 回归锚点（刚修的行为）：reqwest 的 send() 失败有两种来源——上游真的失败，
// 或请求体适配器产出 Err 令其主动中止（超限 / 客户端上传中断）。此前两类混为
// 一谈，客户端自己断开会把排障矛头指向根本没收到完整请求体的上游。
//
// 构造：裸 TCP 客户端声明一个远大于实发字节数的 Content-Length，发一部分就
// 半关写端（FIN 但保留读端），于是代理侧的请求体流以 Err 收场；上游则必须
// **读完整个请求体**才回响应（否则它抢答，send() 会先拿到 200）。
//
// 鉴别力：若 client_gone 标记缺失（或判定顺序错、标记未置位），send() 的错误
// 会落进通用分支，记下「上游请求失败: …」并回 502 → last_error 断言与状态码
// 断言都会红。
// ---------------------------------------------------------------------------
#[tokio::test]
async fn forward_only_client_upload_abort_is_not_an_upstream_failure() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    // 上游读完整个请求体才回响应：读失败（客户端中断导致 reqwest 中止）时回 400，
    // 绝不会先于 reqwest 的错误抢答一个成功响应
    let upstream = Router::new().route(
        "/v1/upload",
        any(|req: axum::extract::Request| async move {
            match axum::body::to_bytes(req.into_body(), 64 * 1024 * 1024).await {
                Ok(b) => (StatusCode::OK, format!("stored {}", b.len())).into_response(),
                Err(_) => (StatusCode::BAD_REQUEST, "incomplete body").into_response(),
            }
        }),
    );
    let (upstream_url, _h1) = bind_random_router(upstream).await;

    let mut cfg = proxy_config_for(&upstream_url);
    cfg.forward_only = Some(true);
    let state = AppState::new(cfg);
    let probe = state.clone();
    let (proxy_url, _h2) = bind_random_router(aproxy::proxy::router(state)).await;

    // 裸 TCP：声明 64 KiB 却只发 1 KiB 就半关写端
    let addr = proxy_url.trim_start_matches("http://").to_string();
    let mut sock = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let head = format!(
        "POST /v1/upload HTTP/1.1\r\nhost: {addr}\r\ncontent-length: {}\r\n\r\n",
        64 * 1024
    );
    sock.write_all(head.as_bytes()).await.unwrap();
    sock.write_all(&vec![b'x'; 1024]).await.unwrap();
    sock.flush().await.unwrap();
    // 半关：发 FIN 但保留读端，好让代理侧的 400 能回到我们手里
    sock.shutdown().await.unwrap();

    // 读回响应（半关后仍可读）。代理可能只回状态行与正文，不保证本机 socket
    // 何时收到 EOF，故读到「读满一段」或超时即止
    let mut raw = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), sock.read_to_end(&mut raw)).await;
    let text = String::from_utf8_lossy(&raw).to_string();
    assert!(
        text.starts_with("HTTP/1.1 400"),
        "客户端上传途中断开应与缓冲路径同一归因（400），实际响应: {text:?}"
    );
    assert!(
        text.contains("客户端在上传途中断开"),
        "400 正文应指明是客户端断开，而非上游失败，实际: {text:?}"
    );

    // 归因锚点：客户端自己断开**不得**污染「最近错误」——否则 status 会把矛头
    // 指向根本没收到完整请求体的上游
    let last_error = probe.stats.last_error.lock().unwrap().clone();
    assert!(
        last_error.is_none(),
        "客户端主动断开不是上游失败，不得记入 last_error，实际: {last_error:?}"
    );
    assert_eq!(
        probe.stats.requests_total.load(Ordering::Relaxed),
        1,
        "应恰好计 1 次客户端请求"
    );
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

/// 从测试进程 pid 派生第 offset 个互不相同的守护测试端口（25000..=65535 区间，
/// 避开默认端口 12345 与常见手工实例端口；每个测试用不同 offset，互不冲突）。
///
/// 派生出起点后**逐个试探可绑定性**，而不是盲取：Windows 上 Hyper-V/WinNAT 会
/// 保留成片的排除区间（`netsh interface ipv4 show excludedportrange protocol=tcp`
/// 实测本机有 50000-51059、63912-65081 等），落进去时守护以「无法绑定」直接启动
/// 失败。危害不止一个端口：同一进程内所有守护测试共用同一 base，一挂就是一整片，
/// 实测约一成多的整套运行会因此变红（与实现无关的假失败，最坏时被当成回归）。
///
/// 步长取 16 而非 1：offset 实际只用 0..=12，故不同 offset 落在不同的模 16 余数
/// 类里，「每个测试用不同端口」的既有约束得以保持——顺带还能跳过正被其他测试的
/// 残留守护占用的端口。
fn daemon_test_port(offset: u32) -> u16 {
    let base = 25000 + (std::process::id() % 20000) * 2;
    // 96 步 × 16 = 1536 宽的窗口：本机最宽的连续排除块（50000-51059，1060 宽）
    // 装得下。窗口贴着上界时循环会提前 break，那里 65082..65535 是空的，够用。
    for step in 0..96u32 {
        let candidate = base + offset + step * 16;
        if candidate > u16::MAX as u32 {
            break;
        }
        if std::net::TcpListener::bind(("127.0.0.1", candidate as u16)).is_ok() {
            return candidate as u16;
        }
    }
    // 全部候选都不可绑定（实测不会发生）：退回起点，让测试以「无法绑定」明确
    // 失败，而不是悄悄换到别的端口导致断言指向错的地方
    (base + offset) as u16
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
    /// 守护的隔离主目录（APROXY_HOME）：注册表在 home/run/ 下，unix 的
    /// UDS socket 也在其中，stop 须看到同一 APROXY_HOME 才找得到守护；
    /// None = 默认主目录 ~/.aproxy
    home_dir: Option<std::path::PathBuf>,
}

impl Drop for DaemonGuard {
    fn drop(&mut self) {
        let mut cmd = Command::new(self.exe);
        cmd.args(["stop", &self.port.to_string()]);
        if let Some(dir) = &self.home_dir {
            cmd.env("APROXY_HOME", dir);
        }
        let _ = cmd.output();
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

// ps -p 对 zombie（defunct）进程也返回成功——守护已退出但尚未被收割时
// 会被误判为存活；须读取进程态排除 Z 状态。进程不存在时 ps 以非零退出
// 且 stdout 为空，须先校验退出码，否则会被误判为存活。
#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "stat="])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|stat| !stat.trim_start().starts_with('Z'))
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
    let _guard = DaemonGuard {
        exe,
        port,
        home_dir: None,
    };

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

    // 身份判定：正名运行的守护必须通过 is_aproxy_process（看门狗收养/选举/
    // 处决关卡的全部前置）。镜像名精确比对的成功路径（Windows aproxy.exe /
    // unix aproxy）
    assert!(
        aproxy::watchdog::is_aproxy_process(pid),
        "正名运行的守护 (pid {pid}) 应通过进程身份判定"
    );

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
    let _guard = DaemonGuard {
        exe,
        port,
        home_dir: None,
    };

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
    let _guard = DaemonGuard {
        exe,
        port,
        home_dir: None,
    };

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
    let _guard = DaemonGuard {
        exe,
        port,
        home_dir: None,
    };

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
    let _guard_a = DaemonGuard {
        exe,
        port: port_a,
        home_dir: None,
    };
    let _guard_b = DaemonGuard {
        exe,
        port: port_b,
        home_dir: None,
    };
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
    let _guard = DaemonGuard {
        exe,
        port,
        home_dir: None,
    };
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
    kill_pid(pid);
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

    // 幂等：再次 restore 时已在运行 → 跳过而非报错/重复启动。
    // restore 作用于全局真实 run 目录（开发机上可能存在用户实例的记录被
    // 跳过或恢复），只断言本测试端口的跳过行为，不断言其他端口。
    let out = Command::new(exe).arg("restore").output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains(&format!("端口 {port} 已在运行，跳过")),
        "幂等跳过本端口: {stdout}"
    );

    // 优雅停止：恢复记录被删除，此后 restore 不再恢复本端口
    let out = Command::new(exe)
        .args(["stop", &port.to_string()])
        .output()
        .unwrap();
    assert!(out.status.success(), "stop 恢复实例应成功: {stdout}");
    assert!(!restore_path.exists(), "优雅停止后恢复记录应被删除");
    let out = Command::new(exe).arg("restore").output().unwrap();
    assert!(out.status.success(), "restore 应静默成功");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("已恢复") || !stdout.contains(&port.to_string()),
        "本端口已无记录，不应再被恢复: {stdout}"
    );
    // 若全局为空则明确输出「没有需要恢复的实例」；非空（用户实例在册）则
    // 输出的是它们的跳过/恢复行——两者都算通过
    if stdout.contains("没有需要恢复的实例") {
        // 全局为空的经典路径，已验证
    }
}

// ---------------------------------------------------------------------------
// 29. 配置别名：alias add → start <别名> → stop <别名> 端到端
//
// 隔离：别名表存于 settings.json（进程级全局配置），APROXY_HOME 注入
// tempdir 后写入隔离目录——不再触碰真实 ~/.aproxy/settings.json
// （历史版本无重定向注入点，曾用真实 settings.json + pid 派生唯一别名妥协）。
// ---------------------------------------------------------------------------
#[test]
fn alias_start_and_stop_roundtrip() {
    let port = daemon_test_port(8);
    let exe = env!("CARGO_BIN_EXE_aproxy");
    let alias = format!("alias-test-{}", std::process::id());
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    // 关掉看护者：`start` 会顺带拉起全局看护进程，而它带着 target/debug/aproxy.exe
    // 的镜像一直存活到 watchdog_idle_exit_secs（默认 300s）——测试早已结束，它却
    // 继续锁着构建产物，让随后的 cargo 重新链接失败（实测踩过：`failed to remove
    // file … 拒绝访问`，并让并行跑的 restart_integration 超时假失败）。
    // 本测试不涉及看护者行为，关掉不影响断言。
    std::fs::write(home.join("settings.json"), r#"{"watchdog": false}"#).unwrap();
    let cfg_file = dir.path().join("aliased.toml");
    std::fs::write(
        &cfg_file,
        format!(
            "base_url = \"https://alias-test.example.com\"\nlisten_addr = \"127.0.0.1:{port}\"\n"
        ),
    )
    .unwrap();
    let _guard = DaemonGuard {
        exe,
        port,
        home_dir: Some(home.to_path_buf()),
    };

    // add 别名（指向临时配置）
    let out = Command::new(exe)
        .args(["alias", "add", &alias])
        .arg(&cfg_file)
        .env("APROXY_HOME", home)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "alias add 应成功: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // list 包含该别名
    let out = Command::new(exe)
        .args(["alias", "list"])
        .env("APROXY_HOME", home)
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains(&alias),
        "alias list 应包含 {alias}: {stdout}"
    );

    // start <别名>：后台启动别名指向的配置
    let out = Command::new(exe)
        .args(["start", &alias])
        .env("APROXY_HOME", home)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "start 别名应成功: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("已在后台启动"), "实际: {stdout}");
    // 就绪：隔离 home 的 IPC 寻址（unix UDS 在 home/run/ 下）
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let mut ready = false;
    for _ in 0..100 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok()
            && ipc_ping_in_dir(&rt, port, home).is_ok()
        {
            ready = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(ready, "别名启动的守护应就绪 (端口 {port})");

    // 别名启动的实例 config_path 应指向别名配置（status 可见）
    let out = Command::new(exe)
        .arg("status")
        .env("APROXY_HOME", home)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("alias-test.example.com"),
        "status 应展示别名配置的上游: {stdout}"
    );

    // stop <别名>：按 config_path 匹配并停止
    let out = Command::new(exe)
        .args(["stop", &alias])
        .env("APROXY_HOME", home)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stop 别名应成功: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("已停止"), "实际: {stdout}");

    // 已停止后再 stop 别名：明确报「未在运行」
    let out = Command::new(exe)
        .args(["stop", &alias])
        .env("APROXY_HOME", home)
        .output()
        .unwrap();
    assert!(!out.status.success(), "别名配置未运行时 stop 应失败");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(text.contains("未在运行"), "实际: {text}");

    // 清理别名（无论如何执行，不留测试残留——隔离 home 内本就随 tempdir 删除）
    let _ = Command::new(exe)
        .args(["alias", "remove", &alias])
        .env("APROXY_HOME", home)
        .output();
}

// ---------------------------------------------------------------------------
// 30. 别名错误路径：start/stop 未知别名明确报错；保留字校验
// ---------------------------------------------------------------------------
#[test]
fn alias_errors_on_unknown_names() {
    let exe = env!("CARGO_BIN_EXE_aproxy");
    let unknown = format!("no-such-alias-{}", std::process::id());
    // 隔离：别名读写不触碰真实 settings.json
    let dir = tempfile::tempdir().unwrap();
    let home_arg = ("APROXY_HOME", dir.path().display().to_string());

    // start 未知别名：报错并列出管理方式
    let out = Command::new(exe)
        .args(["start", &unknown])
        .env(home_arg.0, &home_arg.1)
        .output()
        .unwrap();
    assert!(!out.status.success(), "start 未知别名应失败");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(text.contains("未知的别名或配置文件"), "实际: {text}");

    // stop 未知别名：报错
    let out = Command::new(exe)
        .args(["stop", &unknown])
        .env(home_arg.0, &home_arg.1)
        .output()
        .unwrap();
    assert!(!out.status.success(), "stop 未知别名应失败");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(text.contains("未知的别名或端口号"), "实际: {text}");

    // add 保留字：all / 纯数字被拒绝
    for bad in ["all", "12345"] {
        let out = Command::new(exe)
            .args(["alias", "add", bad])
            .arg("x.toml")
            .env(home_arg.0, &home_arg.1)
            .output()
            .unwrap();
        assert!(!out.status.success(), "别名 {bad} 应被拒绝");
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(text.contains("别名无效"), "实际: {text}");
    }

    // add 不存在的配置路径：报错
    let out = Command::new(exe)
        .args(["alias", "add", &format!("bad-{}", std::process::id())])
        .arg("Z:/no/such/file.toml")
        .env(home_arg.0, &home_arg.1)
        .output()
        .unwrap();
    assert!(!out.status.success(), "add 不存在的路径应失败");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(text.contains("配置文件不存在"), "实际: {text}");

    // remove 不存在的别名：报错
    let out = Command::new(exe)
        .args(["alias", "remove", &unknown])
        .env(home_arg.0, &home_arg.1)
        .output()
        .unwrap();
    assert!(!out.status.success(), "remove 不存在的别名应失败");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(text.contains("不存在"), "实际: {text}");
}

// ---------------------------------------------------------------------------
// 17. 看门狗（G2）：全局单看护进程的重拉/放行/补种端到端
//
// 隔离：APROXY_HOME 指向 tempdir（守护/看护子进程经 spawn_detached 继承
// 环境），APROXY_WATCHDOG_SCAN_SECS=1 让看护者秒级扫描。测试端口照旧从测试
// 进程 pid 派生，绝不触碰生产实例；结束清理 claim 与残留守护。
// ---------------------------------------------------------------------------
#[test]
fn watchdog_respawns_killed_daemon() {
    // 独立端口（偏移 9）：既有守护测试并行占用 daemon_test_ports() 的前三个，
    // 本测试窗口长（看护扫描+重拉），同端口会与之互踩（实测 flaky）
    let port = daemon_test_port(9);
    let dir = tempfile::tempdir().unwrap();
    let cfg_file = dir.path().join("wd.toml");
    std::fs::write(
        &cfg_file,
        format!("base_url = \"https://wd-test.example.com\"\nlisten_addr = \"127.0.0.1:{port}\"\n"),
    )
    .unwrap();
    let exe = env!("CARGO_BIN_EXE_aproxy");
    let _guard = DaemonGuard {
        exe,
        port,
        home_dir: Some(dir.path().to_path_buf()),
    };
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    // 带隔离环境启动守护：必须用 Command::env（spawn_detached 继承的是测试
    // 进程环境，无法逐子进程注入）——守护与看护者都要看到同一个 tempdir 注册表
    let envs = [
        ("APROXY_HOME", dir.path().display().to_string()),
        ("APROXY_WATCHDOG_SCAN_SECS", "1".to_string()),
    ];
    let daemon_child = {
        let mut cmd = Command::new(exe);
        cmd.args([
            "--config",
            cfg_file.display().to_string().as_str(),
            "--daemon-child",
        ]);
        for (k, v) in &envs {
            cmd.env(k, v);
        }
        cmd.spawn().expect("spawn 守护失败")
    };
    let orig_pid = daemon_child.id();
    // 隔离主目录版的就绪等待：TCP 可连 + 隔离 socket 的 IPC ping 可达
    let mut ready = false;
    for _ in 0..100 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok()
            && ipc_ping_in_dir(&rt, port, dir.path()).is_ok()
        {
            ready = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(ready, "守护未就绪");

    // 启动看护进程（同一隔离 run 目录）
    let mut wd_cmd = Command::new(exe);
    wd_cmd.arg("--daemon-watchdog");
    for (k, v) in envs {
        wd_cmd.env(k, v);
    }
    let mut wd_child = wd_cmd.spawn().expect("spawn 看护者失败");
    // 等看护者收养（扫描周期 1s，给 3s）
    std::thread::sleep(Duration::from_secs(3));

    // 强杀守护（模拟崩溃——.restore 残留 = 异常死亡信号）
    kill_pid(orig_pid);
    let new_ready = {
        let mut ok = false;
        for _ in 0..150 {
            // 看护者 scan 1s + 退避 0（首次）+ spawn + 就绪，30s 足够
            if ipc_ping_in_dir(&rt, port, dir.path()).is_ok() {
                ok = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        ok
    };
    assert!(new_ready, "看护者未在预期时间内重拉守护");

    // 确认是新进程（旧 pid 已死，新 pid 就绪）
    let live = ipc_ping_in_dir(&rt, port, dir.path()).unwrap();
    assert_ne!(live.pid, orig_pid, "应是被重拉的新进程");
    // 看护者仍在运行（收养新实例继续看护）
    assert!(wd_child.try_wait().unwrap().is_none(), "看护者不应退出");

    // 清理：优雅 stop（.restore 删除）→ 看护者不得复活它；随后闲置自灭前
    // 先杀看护者防泄漏。stop 须带同一 APROXY_HOME（unix 的 UDS socket 在其中）
    let _ = Command::new(exe)
        .args(["stop", &port.to_string()])
        .env("APROXY_HOME", dir.path())
        .output();
    std::thread::sleep(Duration::from_secs(3));
    assert!(
        ipc_ping_in_dir(&rt, port, dir.path()).is_err(),
        "优雅停止后看护者不得复活实例"
    );
    let _ = wd_child.kill();
    let _ = std::fs::remove_file(dir.path().join("run").join("watchdog.claim"));
}

// ---------------------------------------------------------------------------
// 身份判定对「二进制被原地替换」的容忍（unix swap 升级场景）：运行中的守护
// 的 exe 链接会被内核附加「 (deleted)」后缀，is_aproxy_process 剥除后比对，
// 升级动作不得让在运行实例被判为异己（否则收养/选举/处决关卡全体失效）
// ---------------------------------------------------------------------------
#[test]
#[cfg(unix)]
fn identity_check_tolerates_swapped_binary() {
    use std::os::unix::fs::PermissionsExt;

    let port = daemon_test_port(10);
    let dir = tempfile::tempdir().unwrap();
    // 守护用副本二进制运行（不碰真实 target 二进制——并行测试共用它）。
    // 副本必须正名 `aproxy`：身份判定按 basename 精确比对
    let bin = dir.path().join("aproxy");
    std::fs::copy(env!("CARGO_BIN_EXE_aproxy"), &bin).unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let cfg_file = dir.path().join("swap.toml");
    std::fs::write(
        &cfg_file,
        format!(
            "base_url = \"https://swap-test.example.com\"\nlisten_addr = \"127.0.0.1:{port}\"\n"
        ),
    )
    .unwrap();
    let home_dir = dir.path();
    let leaked_bin: &'static str = Box::leak(bin.display().to_string().into_boxed_str());
    let guard = DaemonGuard {
        exe: leaked_bin,
        port,
        home_dir: Some(home_dir.to_path_buf()),
    };
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    let daemon_child = Command::new(&bin)
        .args([
            "--config",
            cfg_file.display().to_string().as_str(),
            "--daemon-child",
        ])
        .env("APROXY_HOME", home_dir.display().to_string())
        .spawn()
        .expect("spawn 副本守护失败");
    let pid = daemon_child.id();
    let mut ready = false;
    for _ in 0..100 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok()
            && ipc_ping_in_dir(&rt, port, home_dir).is_ok()
        {
            ready = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(ready, "副本守护未就绪");
    assert!(
        aproxy::watchdog::is_aproxy_process(pid),
        "替换前正名守护应通过身份判定"
    );

    // 原地替换（swap 升级的真实形态）：新文件 rename 原子覆盖原路径 →
    // 旧 inode 失名，内核把运行中进程的 exe 链接标为「... (deleted)」。
    // 注意不是把运行中二进制 rename 走开——那种场景 exe 跟随新路径名、
    // 无 (deleted) 后缀，basename 随之改变（精确比对判异己，属 F2 边界）
    let new_bin = dir.path().join("aproxy.new");
    std::fs::copy(env!("CARGO_BIN_EXE_aproxy"), &new_bin).unwrap();
    std::fs::rename(&new_bin, &bin).unwrap();
    assert!(
        aproxy::watchdog::is_aproxy_process(pid),
        "二进制被覆盖替换后，运行中守护仍应通过身份判定（(deleted) 后缀剥离）"
    );

    // 守护仍在正常服务（IPC 可达）——身份判定未误杀正常实例
    assert!(
        ipc_ping_in_dir(&rt, port, home_dir).is_ok(),
        "替换后守护应继续可 ping"
    );
    drop(guard);
}

#[test]
fn watchdog_lease_prevents_duplicate_watchdogs() {
    // 并发 spawn 两个看护者（同一隔离主目录）：claim 原子接管保证只有一个
    // 在任——后启动者应自行退出（选举唯一性）
    let dir = tempfile::tempdir().unwrap();
    let exe = env!("CARGO_BIN_EXE_aproxy");
    let envs = [
        ("APROXY_HOME", dir.path().display().to_string()),
        ("APROXY_WATCHDOG_SCAN_SECS", "1".to_string()),
    ];
    let mut c1 = {
        let mut cmd = Command::new(exe);
        cmd.arg("--daemon-watchdog");
        for (k, v) in &envs {
            cmd.env(k, v);
        }
        cmd.spawn().unwrap()
    };
    // 等 c1 接管 claim
    std::thread::sleep(Duration::from_secs(2));
    let mut c2 = {
        let mut cmd = Command::new(exe);
        cmd.arg("--daemon-watchdog");
        for (k, v) in &envs {
            cmd.env(k, v);
        }
        cmd.spawn().unwrap()
    };
    std::thread::sleep(Duration::from_secs(3));
    assert!(c1.try_wait().unwrap().is_none(), "先任看护者应持续在任");
    assert!(
        c2.try_wait().unwrap().is_some(),
        "后任看护者应因 claim 被占而退出"
    );
    let _ = c1.kill();
    let _ = std::fs::remove_file(dir.path().join("run").join("watchdog.claim"));
}

/// 对运行在隔离主目录里的守护做 IPC ping：unix 的 UDS socket 路径在
/// home/run/ 下（endpoint_for 解析依赖进程环境，库调用方须显式给目录）；
/// Windows 管道名全局唯一，主目录只影响注册表文件，端点忽略该参数。
fn ipc_ping_in_dir(
    rt: &tokio::runtime::Runtime,
    port: u16,
    #[cfg(unix)] home_dir: &std::path::Path,
    #[cfg(windows)] _home_dir: &std::path::Path,
) -> Result<aproxy::daemon::InstanceInfo, String> {
    #[cfg(windows)]
    let endpoint = aproxy::daemon::endpoint_for(&port.to_string());
    #[cfg(unix)]
    let endpoint = home_dir
        .join("run")
        .join(format!("{}.sock", port))
        .display()
        .to_string();
    rt.block_on(async {
        let mut last = String::new();
        for attempt in 0..3 {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            match aproxy::daemon::ipc_request_to(&endpoint, &aproxy::daemon::IpcRequest::Ping).await
            {
                Ok(resp) if resp.ok => {
                    return resp.info.ok_or_else(|| "实例响应缺少信息".to_string());
                }
                Ok(_) => last = "实例返回失败".to_string(),
                Err(e) => last = e,
            }
        }
        Err(last)
    })
}

/// 强杀进程（模拟崩溃）：Windows taskkill /F；unix SIGKILL
#[cfg(windows)]
fn kill_pid(pid: u32) {
    let _ = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/F"])
        .output();
}

#[cfg(unix)]
fn kill_pid(pid: u32) {
    let _ = Command::new("kill").args(["-9", &pid.to_string()]).output();
}

// ---------------------------------------------------------------------------
// 31. IPC 观测计数：status 的「请求 / 重试 / 最近错误」必须反映真实流量
//
// 回归锚点（2026-09-14 端到端实测发现）：serve_forever 曾在此**另建一份**
// IpcStats、只把活动时间戳共享进来，而请求热路径累加的是 AppState 里另一份——
// 于是「请求 / 重试 / 最近错误」三项对任何实例恒为 0。活动时间戳恰好正常
// （它确实是共享的），反而掩盖了缺陷，直到端到端实测才暴露。
//
// 断言刻意走 **IPC ping 的真实响应**而非进程内的同一个 Arc——后者无论如何
// 都自洽，正是这类接线缺陷的盲区。
//
// 隔离：APROXY_HOME 指向 tempdir，并在其中**关掉 watchdog**——看护进程带着
// target/debug/aproxy.exe 的镜像存活到 watchdog_idle_exit_secs，会锁住后续
// cargo 构建（实测踩过：`failed to remove file … 拒绝访问`）。
// ---------------------------------------------------------------------------
#[test]
fn ipc_stats_reflect_real_traffic() {
    isolate_env_proxy(); // 必须在 spawn 前：守护子进程继承 NO_PROXY
    let port = daemon_test_port(11);
    let exe = env!("CARGO_BIN_EXE_aproxy");
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().to_path_buf();
    std::fs::write(home.join("settings.json"), r#"{"watchdog": false}"#).unwrap();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    // mock 上游：**只对第一个请求**回 500，其后恒 200。刻意的——若上游恒 200
    // 走首轮成功快速路径，`retries_total == 0` 这条断言无论实现好坏都成立
    // （恒真、无法证伪）：0 既可能是「没重试」，也可能是「重试计数根本没接线」。
    // 让第一个请求必然产生一次重试，该断言才有鉴别力。
    let seen = Arc::new(AtomicUsize::new(0));
    let seen_clone = seen.clone();
    let (upstream, _handle) =
        rt.block_on(bind_random_router(Router::new().fallback(any(move || {
            let seen = seen_clone.clone();
            async move {
                let n = seen.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "{\"type\":\"error\",\"error\":{\"type\":\"overloaded\"}}",
                    )
                } else {
                    (StatusCode::OK, "{\"ok\":true}")
                }
            }
        }))));

    let cfg_file = home.join("observe.toml");
    std::fs::write(
        &cfg_file,
        format!("base_url = \"{upstream}\"\nlisten_addr = \"127.0.0.1:{port}\"\n"),
    )
    .unwrap();
    let _guard = DaemonGuard {
        exe,
        port,
        home_dir: Some(home.clone()),
    };

    let out = Command::new(exe)
        .arg("start")
        .arg("--config")
        .arg(&cfg_file)
        .env("APROXY_HOME", &home)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "start 应成功，stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // 就绪判定按**隔离 home 里的端点**寻址：unix 的 UDS 在 home/run/ 下，
    // 用默认主目录的 ipc_ping 会找错位置
    let run_dir = home.join("run");
    let port_str = port.to_string();
    let mut ready = false;
    for _ in 0..100 {
        if rt
            .block_on(aproxy::daemon::ipc_ping_in(&run_dir, &port_str))
            .is_ok()
        {
            ready = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(ready, "守护未在 10 秒内就绪");

    let client = local_client();
    let url = format!("http://127.0.0.1:{port}/v1/observe");
    for i in 0..3 {
        let resp = rt.block_on(client.get(&url).send()).unwrap();
        assert_eq!(
            resp.status(),
            200,
            "第 {i} 个请求应 200（首个经一次重试后成功）"
        );
        rt.block_on(resp.bytes()).unwrap();
    }

    let info = rt
        .block_on(aproxy::daemon::ipc_ping_in(&run_dir, &port_str))
        .expect("实例应可 ping");
    assert_eq!(
        info.requests_total, 3,
        "IPC 应报告 3 次请求（回归：曾因统计源不共享而恒为 0）"
    );
    // 鉴别力：3 个请求里第 1 个上游先回 500，必然重试一次后成功。若重试计数
    // 未接线（恒 0）或漏计，这里就是红的——恒 200 的场景抓不到这一点
    assert!(
        info.retries_total >= 1,
        "首个请求上游回 500，必然产生至少一次重试；IPC 报告的重试数为 {}",
        info.retries_total
    );
    // 重试本身是成功收尾的，但「最近错误」保留覆盖式的最后一次失败记录——
    // 这一半此前完全没断言（接线曾恒为 None）
    let last_error = info.last_error.clone().unwrap_or_default();
    assert!(
        last_error.contains("500"),
        "重试前的 500 应被记为最近错误（覆盖式保留），实际: {last_error:?}"
    );
    assert!(
        info.last_error_at > 0,
        "最近错误应带发生时刻，实际: {}",
        info.last_error_at
    );
}

// ---------------------------------------------------------------------------
// 32. IPC 请求计数：**仅转发模式**实例的分支点必须在计数之后
//
// proxy_handler 里 forward_only 分支位于 requests_total.fetch_add **之后**，
// 本模式的请求计数与常规模式完全一致（也就是说 status 的请求数对本模式不是
// 恒 0）。分支点一旦被挪到 requests_total.fetch_add **之前**（例如把仅转发
// 判定提到覆写/计数之前），这里就是红的。
//
// 与 31 同款：断言走 **IPC ping 的真实响应**而非进程内 Arc，隔离 home + 关
// watchdog 的理由见 31 的说明。
// ---------------------------------------------------------------------------
#[test]
fn ipc_stats_reflect_forward_only_traffic() {
    const REQUESTS: u64 = 4;

    isolate_env_proxy();
    let port = daemon_test_port(12);
    let exe = env!("CARGO_BIN_EXE_aproxy");
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().to_path_buf();
    std::fs::write(home.join("settings.json"), r#"{"watchdog": false}"#).unwrap();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    // 句柄留着：下半场要主动掐掉 mock 上游，把「上游请求失败」这条路径也拉到
    // 进程级 IPC 断言里。
    // 上游每个响应都带 connection: close：否则 reqwest 会把连接放回池子，掐掉
    // 监听后第 5 个请求仍可能复用那条（已被 accept、仍由 axum 连接任务持有）的
    // 长连接成功拿到 200，测试会随机红。close 让每次请求都新建连接，掐掉监听即
    // 必然不可达。（该头是 hop-by-hop，代理不会透传给客户端。）
    let (upstream, upstream_handle) =
        rt.block_on(bind_random_router(Router::new().fallback(any(|| async {
            (StatusCode::OK, [("connection", "close")], "{\"ok\":true}")
        }))));

    let cfg_file = home.join("forward-observe.toml");
    std::fs::write(
        &cfg_file,
        format!(
            "base_url = \"{upstream}\"\nlisten_addr = \"127.0.0.1:{port}\"\nforward_only = true\n"
        ),
    )
    .unwrap();
    let _guard = DaemonGuard {
        exe,
        port,
        home_dir: Some(home.clone()),
    };

    let out = Command::new(exe)
        .arg("start")
        .arg("--config")
        .arg(&cfg_file)
        .env("APROXY_HOME", &home)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "start 应成功，stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let run_dir = home.join("run");
    let port_str = port.to_string();
    let mut ready = false;
    for _ in 0..100 {
        if rt
            .block_on(aproxy::daemon::ipc_ping_in(&run_dir, &port_str))
            .is_ok()
        {
            ready = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(ready, "守护未在 10 秒内就绪");

    let client = local_client();
    let url = format!("http://127.0.0.1:{port}/v1/observe");
    for i in 0..REQUESTS {
        let resp = rt.block_on(client.get(&url).send()).unwrap();
        assert_eq!(resp.status(), 200, "第 {i} 个请求应 200");
        rt.block_on(resp.bytes()).unwrap();
    }

    let info = rt
        .block_on(aproxy::daemon::ipc_ping_in(&run_dir, &port_str))
        .expect("实例应可 ping");
    assert_eq!(
        info.requests_total, REQUESTS,
        "仅转发模式的请求计数必须与常规模式一致（回归：分支点若移到计数之前，这里恒为 0）"
    );
    // 这里刻意**不**断言 retries_total == 0：上游恒 200，连常规模式都不会重试，
    // 该断言无论实现好坏都成立（恒真，证明不了「仅转发不重试」的任何事）。
    // 重试计数的**接线**由 31 覆盖（那里首个请求上游必回 500）；无论落到哪个
    // 分支都不重试这一行为由 22c（上游 500）与 22i（上游不可达）覆盖。

    // ---- 下半场：掐掉上游，验证「上游请求失败」的**进程级**观测链路 ----
    //
    // 22i 已在进程内 Arc 上断言过 note_upstream_failure 被调用；这里补的是它与
    // IPC 的**接线**——热路径写进 AppState.stats 的最近错误，能否经 ipc_ping
    // 真的读到（历史上 234d520 修的正是「观测统计源与热路径不同一」这一类断链）。
    //
    // 鉴别力：若 note_upstream_failure 被删、或 stats 与 IPC 又各持一份，
    // last_error 将是 None → 红。
    upstream_handle.abort();
    // abort 只是打标记：current_thread 运行时里被中止的 future 要等调度器再转
    // 一圈才真正 drop，监听套接字那一刻才关闭。await 这个句柄把「已取消」逼出来，
    // 否则下面的探测永远看到监听仍在
    let _ = rt.block_on(upstream_handle);
    // 再等端口真的拒绝连接才发请求：轮询把「套接字关闭 → 生效」这段抹成确定性
    let upstream_addr = upstream.trim_start_matches("http://").to_string();
    let mut refused = false;
    for _ in 0..100 {
        if std::net::TcpStream::connect(&upstream_addr).is_err() {
            refused = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(refused, "mock 上游未能关闭，无法构造「上游不可达」场景");

    let resp = rt
        .block_on(client.get(&url).send())
        .expect("上游不可达时必须立刻终结（本模式不重试）");
    assert_eq!(resp.status(), 502, "上游请求失败应回 502");
    let body = rt.block_on(resp.text()).unwrap();
    assert!(
        body.contains("上游请求失败"),
        "502 正文应带上失败原因，实际: {body}"
    );

    let info = rt
        .block_on(aproxy::daemon::ipc_ping_in(&run_dir, &port_str))
        .expect("实例应可 ping");
    assert_eq!(
        info.requests_total,
        REQUESTS + 1,
        "失败的请求同样走 forward_only 分支，必须照常计数"
    );
    let last_error = info.last_error.clone().unwrap_or_default();
    assert!(
        last_error.contains("上游请求失败"),
        "仅转发模式的上游失败必须经 IPC 可见（status 的「最近错误」），实际: {last_error:?}"
    );
    assert!(
        info.last_error_at > 0,
        "最近错误应带发生时刻，实际: {}",
        info.last_error_at
    );
}
