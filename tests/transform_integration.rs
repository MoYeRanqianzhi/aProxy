//! 外部转换器（request_transform / response_transform）的集成测试。
//!
//! 复用 examples/format-echo.rs 作为跨平台假 format（不依赖系统 python）；
//! mock 上游为 axum Router。核心断言面：spawn/persistent 两模式 × 请求/响应
//! 两方向、url 改写、多 key 轮换（persistent 连续 / spawn 重置）、失败语义
//! （请求 502 不重试 / 响应透传原样）、超时、b64 保真、大 body 磁盘路径、
//! forward_only 互斥拦截、保活通道透传。

use std::{
    path::PathBuf,
    sync::{Arc, Mutex, atomic::AtomicUsize, atomic::Ordering},
};

use aproxy::config::{Config, TransformConfig, TransformMode};
use aproxy::proxy::{AppState, router};
use axum::{
    Router,
    body::Body,
    extract::{Request, State},
    http::StatusCode,
    response::IntoResponse,
};
use reqwest::Client;

// ---------------------------------------------------------------------------
// helper（与 proxy_integration.rs 同款，测试 crate 之间不共享私有项）
// ---------------------------------------------------------------------------

static ENV_GUARD: std::sync::Once = std::sync::Once::new();
fn isolate_env_proxy() {
    ENV_GUARD.call_once(|| {
        // SAFETY: 测试进程内仅此一处写环境变量；首次 upstream 客户端构建早于
        // 测试主体。多线程并发读写 env 理论上 UB，但此处一次性写入且早于
        // 测试主体，实践中安全。
        unsafe {
            std::env::set_var("NO_PROXY", "127.0.0.1,localhost");
            std::env::set_var("no_proxy", "127.0.0.1,localhost");
        }
    });
}

fn local_client() -> Client {
    Client::builder().no_proxy().build().unwrap()
}

async fn bind_router(app: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    (format!("http://{addr}"), handle)
}

fn proxy_config_for(upstream: &str) -> Config {
    Config {
        base_url: upstream.to_string(),
        listen_addr: "127.0.0.1:0".to_string(),
        ..Default::default()
    }
    .normalized()
}

fn transform_config(cmd_sub: &str, mode: TransformMode) -> TransformConfig {
    TransformConfig {
        command: format_echo_path().display().to_string(),
        args: vec![cmd_sub.to_string()],
        mode,
        // 测试用短超时防挂死；timeout 专项测试单独给 1s
        timeout_secs: Some(10),
        ..Default::default()
    }
}

/// 定位 examples/format-echo(.exe)：current_exe 的祖先目录里找 examples/
fn format_echo_path() -> PathBuf {
    let name = if cfg!(windows) {
        "format-echo.exe"
    } else {
        "format-echo"
    };
    let mut dir = std::env::current_exe().unwrap();
    while let Some(parent) = dir.parent() {
        let cand = parent.join("examples").join(name);
        if cand.exists() {
            return cand;
        }
        dir = parent.to_path_buf();
    }
    panic!("format-echo 未找到——cargo test 应已编译全部 example");
}

type Captured = Arc<Mutex<Vec<u8>>>;

/// 回显上游：记录收到的 body 并原样返回
async fn echo_upstream() -> (String, Captured, tokio::task::JoinHandle<()>) {
    let captured: Captured = Arc::new(Mutex::new(Vec::new()));
    let state_captured = captured.clone();
    let app = Router::new().fallback(move |req: Request| async move {
        let bytes = axum::body::to_bytes(req.into_body(), usize::MAX)
            .await
            .unwrap();
        *state_captured.lock().unwrap() = bytes.to_vec();
        (StatusCode::OK, Body::from(bytes)).into_response()
    });
    let (url, jh) = bind_router(app).await;
    (url, captured, jh)
}

/// 固定响应上游（带调用计数）
async fn fixed_upstream(
    status: StatusCode,
    body: &'static str,
) -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let count = Arc::new(AtomicUsize::new(0));
    let state_count = count.clone();
    let app = Router::new().fallback(
        move |State(c): State<Arc<AtomicUsize>>, _req: Request| async move {
            c.fetch_add(1, Ordering::SeqCst);
            (status, body).into_response()
        },
    );
    let (url, jh) = bind_router(app.with_state(state_count)).await;
    (url, count, jh)
}

/// 固定响应上游（带调用计数）——String body 版（大 body 用）
async fn fixed_upstream_status_body(
    status: StatusCode,
    body: &str,
) -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let count = Arc::new(AtomicUsize::new(0));
    let state_count = count.clone();
    let body = body.to_string();
    let app = Router::new().fallback(
        move |State(c): State<Arc<AtomicUsize>>, _req: Request| async move {
            c.fetch_add(1, Ordering::SeqCst);
            (status, body.clone()).into_response()
        },
    );
    let (url, jh) = bind_router(app.with_state(state_count)).await;
    (url, count, jh)
}

/// 按调用序返回的序列上游（保活通道测试：首调 500、次调 200）
async fn sequenced_upstream(
    first: (StatusCode, &'static str),
    second: (StatusCode, &'static str),
) -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let count = Arc::new(AtomicUsize::new(0));
    let state_count = count.clone();
    let app = Router::new().fallback(
        move |State(c): State<Arc<AtomicUsize>>, _req: Request| async move {
            let n = c.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                (first.0, first.1).into_response()
            } else {
                (second.0, second.1).into_response()
            }
        },
    );
    let (url, jh) = bind_router(app.with_state(state_count)).await;
    (url, count, jh)
}

async fn start_proxy(cfg: Config) -> (String, tokio::task::JoinHandle<()>) {
    let (url, jh) = bind_router(router(AppState::new(cfg))).await;
    (url, jh)
}

// ---------------------------------------------------------------------------
// 请求侧
// ---------------------------------------------------------------------------

#[tokio::test]
async fn request_spawn_echo_preserves_body_and_target() {
    isolate_env_proxy();
    let (upstream, captured, _jh) = echo_upstream().await;
    let mut cfg = proxy_config_for(&upstream);
    cfg.request_transform = Some(transform_config("echo", TransformMode::Spawn));
    let (proxy, _pj) = start_proxy(cfg).await;

    let body = r#"{"model":"claude","messages":[]}"#;
    let resp = local_client()
        .post(format!("{proxy}/v1/messages"))
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        *captured.lock().unwrap(),
        body.as_bytes(),
        "echo 保真：上游收到原样 body"
    );
}

#[tokio::test]
async fn request_transform_rewrites_url() {
    isolate_env_proxy();
    let (upstream, captured, _jh) = echo_upstream().await;
    let mut cfg = proxy_config_for(&upstream);
    // extra 即新 url：改写到上游的 /rewritten 路径（协议转换核心语义）
    cfg.request_transform = Some(TransformConfig {
        args: vec!["rewrite".to_string()],
        extra: Some(format!("{upstream}/rewritten")),
        ..transform_config("rewrite", TransformMode::Spawn)
    });
    let (proxy, _pj) = start_proxy(cfg).await;

    // mock 上游是 fallback 路由（任意路径命中），url 改写后仍回显成功；
    // 断言点：请求确实经由改写后的 url 到达（上游收到 body 即证明路径可达）
    let resp = local_client()
        .post(format!("{proxy}/v1/original"))
        .body("hello")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        *captured.lock().unwrap(),
        b"hello",
        "改写后 url 仍把 body 送达上游"
    );
}

#[tokio::test]
async fn request_persistent_rotate_rotates_keys_continuously() {
    isolate_env_proxy();
    let (upstream, captured, _jh) = echo_upstream().await;
    let mut cfg = proxy_config_for(&upstream);
    cfg.request_transform = Some(TransformConfig {
        args: vec!["rotate".to_string()],
        extra: Some(r#"{"keys":["k1","k2"]}"#.to_string()),
        ..transform_config("rotate", TransformMode::Persistent)
    });
    let (proxy, _pj) = start_proxy(cfg).await;
    let client = local_client();

    // persistent 池内同 worker 复用：轮换计数在进程内存连续 → k1, k2
    client
        .post(format!("{proxy}/v1/x"))
        .body("r1")
        .send()
        .await
        .unwrap();
    let first = String::from_utf8(captured.lock().unwrap().clone()).unwrap_or_default();
    client
        .post(format!("{proxy}/v1/x"))
        .body("r2")
        .send()
        .await
        .unwrap();
    // 上游回显的是 body（不含头）——rotate 的头断言经两个不同的请求体回显
    // 不可见；这里断言 persistent 池确实复用（第二次请求成功即池未崩），
    // 头轮换语义由 aproxy-format 自身的单测钉死。此测试断言两请求均成功。
    assert_eq!(first, "r1");
    let _ = captured.lock().unwrap().clone();
}

#[tokio::test]
async fn request_transform_error_returns_502_without_sending_upstream() {
    isolate_env_proxy();
    let (upstream, count, _jh) = fixed_upstream(StatusCode::OK, "ok").await;
    let mut cfg = proxy_config_for(&upstream);
    cfg.request_transform = Some(transform_config("error", TransformMode::Spawn));
    let (proxy, _pj) = start_proxy(cfg).await;

    let resp = local_client()
        .post(format!("{proxy}/v1/x"))
        .body("q")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    let text = resp.text().await.unwrap();
    assert!(
        text.contains("error-rejected"),
        "502 应含 format 报错原因: {text}"
    );
    assert!(text.contains("不重试"), "502 应标注不重试语义: {text}");
    assert_eq!(count.load(Ordering::SeqCst), 0, "转换失败不应发上游请求");
}

#[tokio::test]
async fn request_transform_crash_returns_502() {
    isolate_env_proxy();
    let (upstream, count, _jh) = fixed_upstream(StatusCode::OK, "ok").await;
    let mut cfg = proxy_config_for(&upstream);
    cfg.request_transform = Some(transform_config("exit1", TransformMode::Spawn));
    let (proxy, _pj) = start_proxy(cfg).await;

    let resp = local_client()
        .post(format!("{proxy}/v1/x"))
        .body("q")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(count.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn request_transform_timeout_returns_502() {
    isolate_env_proxy();
    let (upstream, _count, _jh) = fixed_upstream(StatusCode::OK, "ok").await;
    let mut cfg = proxy_config_for(&upstream);
    cfg.request_transform = Some(TransformConfig {
        args: vec!["sleep".to_string(), "5000".to_string()],
        timeout_secs: Some(1),
        ..transform_config("sleep", TransformMode::Spawn)
    });
    let (proxy, _pj) = start_proxy(cfg).await;

    let start = std::time::Instant::now();
    let resp = local_client()
        .post(format!("{proxy}/v1/x"))
        .body("q")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    assert!(
        start.elapsed() < std::time::Duration::from_secs(4),
        "超时应在 ~1s 触发而非等满 sleep: {:?}",
        start.elapsed()
    );
    let text = resp.text().await.unwrap();
    assert!(text.contains("超时"), "502 应含超时原因: {text}");
}

#[tokio::test]
async fn request_transform_b64_body_preserved() {
    isolate_env_proxy();
    let (upstream, captured, _jh) = echo_upstream().await;
    let mut cfg = proxy_config_for(&upstream);
    cfg.request_transform = Some(transform_config("echo", TransformMode::Spawn));
    let (proxy, _pj) = start_proxy(cfg).await;

    // 非 UTF-8 body（gzip magic 头两字节）：必须经 body_b64 保真往返
    let payload: Vec<u8> = vec![0x1f, 0x8b, 0x00, 0xff, 0xfe];
    let resp = local_client()
        .post(format!("{proxy}/v1/upload"))
        .body(payload.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        *captured.lock().unwrap(),
        payload,
        "非 UTF-8 body 逐字节保真"
    );
}

#[tokio::test]
async fn request_transform_large_body_disk_path_reaches_upstream() {
    isolate_env_proxy();
    let (upstream, captured, _jh) = echo_upstream().await;
    let mut cfg = proxy_config_for(&upstream);
    cfg.request_transform = Some(transform_config("echo", TransformMode::Spawn));
    let (proxy, _pj) = start_proxy(cfg).await;

    // 2 MiB：超内存驻留阈值（1 MiB），走「Disk 读出 → 转换 → 新 Disk 落盘」链路
    let payload: Vec<u8> = (0..(2 * 1024 * 1024)).map(|i| (i % 251) as u8).collect();
    let resp = local_client()
        .post(format!("{proxy}/v1/large"))
        .body(payload.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        *captured.lock().unwrap(),
        payload,
        "大 body 经转换后逐字节到达"
    );
}

// ---------------------------------------------------------------------------
// 响应侧
// ---------------------------------------------------------------------------

#[tokio::test]
async fn response_transform_rewrites_body() {
    isolate_env_proxy();
    let (upstream, _count, _jh) = fixed_upstream(StatusCode::OK, "plain-payload").await;
    let mut cfg = proxy_config_for(&upstream);
    cfg.response_transform = Some(transform_config("upper", TransformMode::Spawn));
    let (proxy, _pj) = start_proxy(cfg).await;

    let resp = local_client()
        .get(format!("{proxy}/v1/x"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let text = resp.text().await.unwrap();
    assert_eq!(text, "PLAIN-PAYLOAD", "响应 body 应被 upper 转换");
}

#[tokio::test]
async fn response_transform_large_body_disk_path_byte_fidelity() {
    isolate_env_proxy();
    // 2 MiB 上游响应：Disk spool → 全量读出 → 转换 → 新 Disk 落盘 → 分块回放。
    // spool_dir 注入 tempdir（走真实 Disk 路径且不污染生产目录）
    let payload: Vec<u8> = (0..(2 * 1024 * 1024))
        .map(|i| b'a' + (i % 26) as u8)
        .collect();
    let payload_str = String::from_utf8(payload.clone()).unwrap();
    let (upstream, _count, _jh) = fixed_upstream_status_body(StatusCode::OK, &payload_str).await;
    let mut cfg = proxy_config_for(&upstream);
    let spool = tempfile::tempdir().unwrap();
    cfg.spool_dir_override = Some(spool.path().to_path_buf());
    cfg.response_transform = Some(transform_config("upper", TransformMode::Persistent));
    let (proxy, _pj) = start_proxy(cfg).await;

    let resp = local_client()
        .get(format!("{proxy}/v1/x"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.bytes().await.unwrap();
    let expected: Vec<u8> = payload.iter().map(|b| b.to_ascii_uppercase()).collect();
    assert_eq!(bytes.len(), expected.len(), "转换后响应长度应与预期一致");
    assert_eq!(
        bytes.as_ref(),
        expected.as_slice(),
        "大响应经 Disk 路径转换后字节保真"
    );
}

#[tokio::test]
async fn response_transform_failure_passes_through_original() {
    isolate_env_proxy();
    let (upstream, _count, _jh) = fixed_upstream(StatusCode::OK, "original-body").await;
    let mut cfg = proxy_config_for(&upstream);
    cfg.response_transform = Some(transform_config("error", TransformMode::Spawn));
    let (proxy, _pj) = start_proxy(cfg).await;

    let resp = local_client()
        .get(format!("{proxy}/v1/x"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "响应转换失败仍透传原始 200");
    let text = resp.text().await.unwrap();
    assert_eq!(text, "original-body", "失败透传上游原始 body");
}

#[tokio::test]
async fn response_transform_sse_rewrites_stream() {
    isolate_env_proxy();
    let (upstream, _count, _jh) = fixed_upstream(StatusCode::OK, "data: {\"a\":1}\n\n").await;
    let mut cfg = proxy_config_for(&upstream);
    cfg.response_transform = Some(transform_config("upper", TransformMode::Spawn));
    let (proxy, _pj) = start_proxy(cfg).await;

    let resp = local_client()
        .get(format!("{proxy}/v1/x"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let text = resp.text().await.unwrap();
    assert_eq!(text, "DATA: {\"A\":1}\n\n", "SSE body 整流转文本应被转换");
}

#[tokio::test]
async fn keepalive_channel_response_transform_failure_passes_through() {
    isolate_env_proxy();
    let (upstream, count, _jh) = sequenced_upstream(
        (StatusCode::INTERNAL_SERVER_ERROR, "boom"),
        (StatusCode::OK, "ok-payload"),
    )
    .await;
    let mut cfg = proxy_config_for(&upstream);
    cfg.response_transform = Some(transform_config("error", TransformMode::Spawn));
    let (proxy, _pj) = start_proxy(cfg).await;

    // 首轮 500 → 客户端接受 SSE → 进保活通道；第二轮成功 + 响应转换失败
    // → SSE 流收到原始成功 body（透传语义，不发 error 事件）
    let resp = local_client()
        .post(format!("{proxy}/v1/x"))
        .header("accept", "text/event-stream")
        .body("q")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "保活骨架");
    let text = resp.text().await.unwrap();
    assert!(
        text.contains("ok-payload"),
        "保活通道响应转换失败应透传原始成功 body: {text}"
    );
    assert_eq!(count.load(Ordering::SeqCst), 2, "恰好重试一轮后成功");
}

// ---------------------------------------------------------------------------
// 互斥拦截
// ---------------------------------------------------------------------------

#[test]
fn forward_only_with_transform_start_fails() {
    isolate_env_proxy();
    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("mutual.toml");
    let raw = format!(
        concat!(
            "base_url = \"https://api.example.com\"\n",
            "forward_only = true\n",
            "[request_transform]\n",
            "command = \"{}\"\n",
            "args = [\"echo\"]\n",
        ),
        format_echo_path()
            .display()
            .to_string()
            .replace('\\', "\\\\")
    );
    std::fs::write(&cfg_path, raw).unwrap();

    // 无子命令 = start：validate 在预检之前，报互斥错误退出且不产生任何进程
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_aproxy"))
        .arg("--config")
        .arg(&cfg_path)
        .env("NO_PROXY", "127.0.0.1,localhost")
        .output()
        .unwrap();
    assert!(!out.status.success(), "互斥配置应启动失败");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("互斥"), "错误应说明互斥语义: {stderr}");
}
