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

/// 记录型上游：完整捕获每次请求的 method/path/headers/body（末次快照 +
/// 调用序列）——url 改写、method 改写、头表替换等改写面的可观测断言面。
#[derive(Clone, Default)]
struct CapturedRequest {
    method: String,
    path: String,
    authorization: Option<String>,
    body: Vec<u8>,
}

type RequestLog = Arc<Mutex<Vec<CapturedRequest>>>;

/// 全量记录上游：method/path/headers/body 逐请求入列并原样回显 body
async fn recording_upstream() -> (String, RequestLog, tokio::task::JoinHandle<()>) {
    let log: RequestLog = Arc::new(Mutex::new(Vec::new()));
    let state_log = log.clone();
    let app = Router::new().fallback(move |req: Request| async move {
        let method = req.method().to_string();
        let path = req
            .uri()
            .path_and_query()
            .map(|pq| pq.as_str().to_string())
            .unwrap_or_default();
        let authorization = req
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok().map(String::from));
        let bytes = axum::body::to_bytes(req.into_body(), usize::MAX)
            .await
            .unwrap();
        state_log.lock().unwrap().push(CapturedRequest {
            method,
            path,
            authorization,
            body: bytes.to_vec(),
        });
        (StatusCode::OK, Body::from(bytes)).into_response()
    });
    let (url, jh) = bind_router(app).await;
    (url, log, jh)
}

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

/// 固定字节上游：返回任意（含非 UTF-8）body（带调用计数）
async fn bytes_upstream(
    status: StatusCode,
    body: Vec<u8>,
) -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let count = Arc::new(AtomicUsize::new(0));
    let state_count = count.clone();
    let app = Router::new().fallback(
        move |State(c): State<Arc<AtomicUsize>>, _req: Request| async move {
            c.fetch_add(1, Ordering::SeqCst);
            (status, Body::from(body.clone())).into_response()
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
    // 全量记录上游：断言上游**实际收到的路径**是改写后的——只断言「200」
    // 没有鉴别力（fallback 路由任意路径都 200，改写被整个移除测试照样绿）
    let (upstream, log, _jh) = recording_upstream().await;
    let mut cfg = proxy_config_for(&upstream);
    // extra 即新 url：改写到上游的 /rewritten 路径（协议转换核心语义）
    cfg.request_transform = Some(TransformConfig {
        args: vec!["rewrite".to_string()],
        extra: Some(format!("{upstream}/rewritten?marker=1")),
        ..transform_config("rewrite", TransformMode::Spawn)
    });
    let (proxy, _pj) = start_proxy(cfg).await;

    let resp = local_client()
        .post(format!("{proxy}/v1/original"))
        .body("hello")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let entries = log.lock().unwrap();
    assert_eq!(entries.len(), 1, "恰好一次上游请求");
    assert_eq!(
        entries[0].path, "/rewritten?marker=1",
        "上游收到的必须是改写后的路径+查询串，而非客户端原始 /v1/original"
    );
    assert_eq!(entries[0].body, b"hello", "body 照常送达");
}

#[tokio::test]
async fn request_transform_scrub_deletes_header_and_rewrites_method() {
    isolate_env_proxy();
    // 头表替换的「删头语义」与 method 改写的可观测验证
    let (upstream, log, _jh) = recording_upstream().await;
    let mut cfg = proxy_config_for(&upstream);
    cfg.request_transform = Some(transform_config("scrub", TransformMode::Spawn));
    let (proxy, _pj) = start_proxy(cfg).await;

    let resp = local_client()
        .post(format!("{proxy}/v1/x"))
        .header("authorization", "Bearer client-secret")
        .body("q")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let entries = log.lock().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].method, "PUT", "format 改写的 method 应到达上游");
    assert_eq!(
        entries[0].authorization, None,
        "format 删掉的 authorization 不得到达上游"
    );
}

#[tokio::test]
async fn request_persistent_rotate_rotates_keys_continuously() {
    isolate_env_proxy();
    // 全量记录上游：轮换序列（k1→k2）必须可在**上游实际收到的
    // authorization 头**上观测——旧版此测试第二次请求零断言（假覆盖）
    let (upstream, log, _jh) = recording_upstream().await;
    let mut cfg = proxy_config_for(&upstream);
    cfg.request_transform = Some(TransformConfig {
        args: vec!["rotate".to_string()],
        extra: Some(r#"{"keys":["k1","k2"]}"#.to_string()),
        ..transform_config("rotate", TransformMode::Persistent)
    });
    let (proxy, _pj) = start_proxy(cfg).await;
    let client = local_client();

    // persistent 池内同 worker 复用：轮换计数在进程内存连续 → k1, k2
    let r1 = client
        .post(format!("{proxy}/v1/x"))
        .body("r1")
        .send()
        .await
        .unwrap();
    assert_eq!(r1.status(), StatusCode::OK);
    let r2 = client
        .post(format!("{proxy}/v1/x"))
        .body("r2")
        .send()
        .await
        .unwrap();
    assert_eq!(r2.status(), StatusCode::OK);
    let entries = log.lock().unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(
        entries[0].authorization.as_deref(),
        Some("Bearer k1"),
        "第一请求拿首 key"
    );
    assert_eq!(
        entries[1].authorization.as_deref(),
        Some("Bearer k2"),
        "第二请求轮换到次 key（persistent 进程内计数连续的实锤）"
    );
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
    // 2 MiB：超内存驻留阈值（1 MiB），走「Disk 读出 → 转换 → 新 Disk 落盘」
    // 链路。spool_dir 注入 tempdir——不注入会按端口写真实 ~/.aproxy/spool/
    // （测试卫生：每次运行约 6MiB 临时文件进生产目录）
    let spool = tempfile::tempdir().unwrap();
    cfg.spool_dir_override = Some(spool.path().to_path_buf());
    let (proxy, _pj) = start_proxy(cfg).await;
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
    // spool 临时文件的清理由 RequestBody/SpooledBody 的 Drop 语义保证
    // （响应返回与 Drop 间存在时序窗口，不做即时目录空断言）
    drop(spool);
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
async fn response_transform_b64_body_preserved() {
    isolate_env_proxy();
    // 非 UTF-8 上游响应：aproxy 侧以 body_b64 喂 format（echo 原样回显）、
    // 读回后逐字节保真回放——响应侧的 b64 往返此前零覆盖
    let payload: Vec<u8> = vec![0x1f, 0x8b, 0x00, 0xff, 0xfe, 0x80];
    let (upstream, _count, _jh) = bytes_upstream(StatusCode::OK, payload.clone()).await;
    let mut cfg = proxy_config_for(&upstream);
    cfg.response_transform = Some(transform_config("echo", TransformMode::Spawn));
    let (proxy, _pj) = start_proxy(cfg).await;

    let resp = local_client()
        .get(format!("{proxy}/v1/x"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.bytes().await.unwrap();
    assert_eq!(
        bytes.as_ref(),
        payload.as_slice(),
        "非 UTF-8 响应逐字节保真"
    );
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
