//! 外部转换器（request_transform / response_transform）的集成测试。
//!
//! 复用 examples/format-echo.rs 作为跨平台假 format（不依赖系统 python）；
//! mock 上游为 axum Router。核心断言面：spawn/persistent 两模式 × 请求/响应
//! 两方向、url 改写、多 key 轮换（persistent 连续 / spawn 重置）、失败语义
//! （请求 502 不重试 / 响应透传原样）、超时、b64 保真、大 body 磁盘路径、
//! forward_only 互斥拦截、保活通道透传、进程池加固（format 多写 stdout 时
//! 不串包 / 复用到死 worker 时换新重试）。

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
async fn transform_stages_share_request_id_and_state() {
    // 请求转换回信的 state 经 aProxy 转交给响应转换（两者是不同进程）；两阶段
    // 收到同一个 request_id 与各自的 stage。保活通道与非保活通道各走一次
    isolate_env_proxy();
    let app = Router::new().fallback(|req: Request| async move {
        let seen = req
            .headers()
            .get("x-stage-seen")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        (
            StatusCode::OK,
            [
                ("content-type", "text/event-stream".to_string()),
                ("x-upstream-saw", seen),
            ],
            "data: {}\n\n",
        )
            .into_response()
    });
    let (upstream, _jh) = bind_router(app).await;
    let mut cfg = proxy_config_for(&upstream);
    cfg.request_transform = Some(transform_config("stateful", TransformMode::Persistent));
    cfg.response_transform = Some(transform_config("stateful", TransformMode::Spawn));
    let (proxy, _pj) = start_proxy(cfg).await;

    let mut ids = Vec::new();
    for accept in ["application/json", "text/event-stream"] {
        let resp = local_client()
            .post(format!("{proxy}/v1/messages"))
            .header("accept", accept)
            .body("{}")
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let header = |name: &str| {
            resp.headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string()
        };
        let upstream_saw = header("x-upstream-saw");
        let id = upstream_saw
            .strip_prefix("request|")
            .and_then(|rest| rest.strip_suffix("|-"))
            .unwrap_or_else(|| panic!("请求阶段应收到 stage=request、尚无 state: {upstream_saw:?}"))
            .to_string();
        assert!(id.parse::<u64>().is_ok(), "request_id 应是请求序号: {id:?}");
        assert_eq!(
            header("x-stage-seen"),
            format!("response|{id}|from-request-{id}"),
            "响应阶段应收到同一 request_id 与请求阶段留下的 state（accept={accept}）"
        );
        ids.push(id);
    }
    assert_ne!(ids[0], ids[1], "不同请求的 request_id 不同");
}

/// 慢上游：等 `delay_ms` 后回一个 SSE 事件（保活通道在等待期间提交骨架、发心跳）
async fn slow_sse_upstream(delay_ms: u64) -> (String, tokio::task::JoinHandle<()>) {
    let app = Router::new().fallback(move || async move {
        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
        (
            StatusCode::OK,
            [("content-type", "text/event-stream")],
            "data: {\"ok\":true}\n\n",
        )
            .into_response()
    });
    bind_router(app).await
}

#[tokio::test]
async fn heartbeat_transform_generates_heartbeats_and_hands_state_to_response() {
    // 心跳转换器逐拍生成心跳字节；首拍带请求体、之后不带；它回信的 state 经
    // aProxy 交给响应转换（响应转换把收到的 state 追加成一行注释）。回放的上游
    // 事件完整出现在所有心跳之后
    isolate_env_proxy();
    let (upstream, _jh) = slow_sse_upstream(2500).await;
    let mut cfg = keepalive_1s_config_for(&upstream);
    cfg.heartbeat_transform = Some(transform_config("heartbeat", TransformMode::Persistent));
    cfg.response_transform = Some(transform_config("heartbeat", TransformMode::Spawn));
    let (proxy, _pj) = start_proxy(cfg).await;

    let body = local_client()
        .post(format!("{proxy}/v1/messages"))
        .header("accept", "text/event-stream")
        .body(r#"{"stream":true}"#)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        body.contains(": hb seq=1 attempt=1 body=true\n\n"),
        "首拍应由心跳转换器生成且带请求体: {body:?}"
    );
    assert!(
        body.contains(": hb seq=2 attempt=1 body=false\n\n"),
        "之后各拍不再带请求体: {body:?}"
    );
    let replay = body.find("data: {\"ok\":true}").expect("应回放上游事件");
    assert!(
        body.rfind(": hb seq=").unwrap() < replay,
        "心跳只出现在回放之前: {body:?}"
    );
    // 最后一拍是第几拍取决于时序，只断言 state 来自心跳回信
    assert!(
        body[replay..].contains("\n\n: state=hb-"),
        "响应转换应收到心跳回信留下的 state: {body:?}"
    );
}

#[tokio::test]
async fn heartbeat_transform_failure_falls_back_to_fixed_heartbeat() {
    // 心跳转换器每拍都回 error：照常发固定心跳，请求本身不受影响
    isolate_env_proxy();
    let (upstream, _jh) = slow_sse_upstream(2500).await;
    let mut cfg = keepalive_1s_config_for(&upstream);
    cfg.heartbeat_transform = Some(transform_config("error", TransformMode::Persistent));
    let (proxy, _pj) = start_proxy(cfg).await;

    let resp = local_client()
        .post(format!("{proxy}/v1/messages"))
        .header("accept", "text/event-stream")
        .body(r#"{"stream":true}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.text().await.unwrap();
    assert!(body.contains(": keepalive\n\n"), "应退回固定心跳: {body:?}");
    assert!(
        body.ends_with("data: {\"ok\":true}\n\n"),
        "上游事件照常回放: {body:?}"
    );
}

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

/// 保活间隔 1s 的实例配置（转换慢于一个间隔时，骨架头与心跳应先于转换完成到达）
fn keepalive_1s_config_for(upstream: &str) -> Config {
    Config {
        keepalive_interval_secs: 1,
        ..proxy_config_for(upstream)
    }
}

/// 发一个走保活通道的请求，返回（拿到响应头的耗时、状态、完整 body）
async fn timed_sse_post(proxy: &str) -> (std::time::Duration, StatusCode, String) {
    let started = std::time::Instant::now();
    let resp = local_client()
        .post(format!("{proxy}/v1/x"))
        .header("accept", "text/event-stream")
        .body("q")
        .send()
        .await
        .unwrap();
    let head_after = started.elapsed();
    let status = resp.status();
    (head_after, status, resp.text().await.unwrap())
}

/// 记录收到的 `x-retry-seen` 头的上游（every_attempt 故障转移的备用上游）
async fn backup_upstream(
    reply: &'static str,
) -> (String, Arc<Mutex<Vec<String>>>, tokio::task::JoinHandle<()>) {
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let state_seen = seen.clone();
    let app = Router::new().fallback(move |req: Request| async move {
        let header = req
            .headers()
            .get("x-retry-seen")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("(none)")
            .to_string();
        state_seen.lock().unwrap().push(header);
        (StatusCode::OK, reply).into_response()
    });
    let (url, jh) = bind_router(app).await;
    (url, seen, jh)
}

fn failover_config(sub: &str, primary: &str, backup: &str, keepalive: bool) -> Config {
    let mut cfg = if keepalive {
        keepalive_1s_config_for(primary)
    } else {
        proxy_config_for(primary)
    };
    cfg.request_transform = Some(TransformConfig {
        extra: Some(format!("{backup}/v1/x")),
        every_attempt: true,
        ..transform_config(sub, TransformMode::Spawn)
    });
    cfg
}

#[tokio::test]
async fn every_attempt_retransforms_the_original_request_before_each_retry() {
    // 主上游一直 500；every_attempt 让重试前的那次转换把请求改发备用上游。备用
    // 上游看到的重试上下文：第 2 次尝试、上次 500、转换器收到的是原始请求
    //（不是首次转换后的产物）。保活与非保活两条重试循环各走一遍
    isolate_env_proxy();
    for keepalive in [false, true] {
        let (primary, primary_hits, _h1) =
            fixed_upstream(StatusCode::INTERNAL_SERVER_ERROR, "down").await;
        let (backup, seen, _h2) = backup_upstream("ok-from-backup").await;
        let (proxy, _h3) =
            start_proxy(failover_config("failover", &primary, &backup, keepalive)).await;
        let mut req = local_client().post(format!("{proxy}/v1/x")).body("q");
        if keepalive {
            req = req.header("accept", "text/event-stream");
        }
        // 限时：转换没把请求改发备用上游时，主上游一直 500，请求会无限重试——
        // 要的是失败，不是挂住
        let (status, text) = tokio::time::timeout(std::time::Duration::from_secs(30), async {
            let resp = req.send().await.unwrap();
            (resp.status(), resp.text().await.unwrap())
        })
        .await
        .expect("30 秒内应由备用上游应答");
        assert_eq!(status, StatusCode::OK, "keepalive={keepalive}");
        assert!(
            text.contains("ok-from-backup"),
            "keepalive={keepalive}: {text}"
        );
        assert_eq!(
            primary_hits.load(Ordering::SeqCst),
            1,
            "只有第一次尝试打到主上游（keepalive={keepalive}）"
        );
        assert_eq!(
            *seen.lock().unwrap(),
            vec!["attempt=2 status=500 original=true".to_string()],
            "keepalive={keepalive}"
        );
    }
}

#[tokio::test]
async fn every_attempt_transform_failure_keeps_retrying_the_previous_request() {
    // 重试前的转换失败不能让请求终止（那会违背无限重试）：沿用上一次发出的请求，
    // 主上游第二次就好了，客户端照样拿到 200
    isolate_env_proxy();
    let (primary, hits, _h1) = sequenced_upstream(
        (StatusCode::INTERNAL_SERVER_ERROR, "down"),
        (StatusCode::OK, "recovered"),
    )
    .await;
    let (proxy, _h2) = start_proxy(failover_config(
        "failover-reject",
        &primary,
        "http://127.0.0.1:1",
        false,
    ))
    .await;
    let resp = local_client()
        .post(format!("{proxy}/v1/x"))
        .body("q")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.text().await.unwrap(), "recovered");
    assert_eq!(hits.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn keepalive_covers_slow_request_transform() {
    // 请求转换耗时 2.5s、保活间隔 1s：请求转换在保活通道内经心跳节拍驱动，
    // 骨架头约 1s 到达、早于转换完成，之后照常心跳——而不是让客户端在拿到任何
    // 字节之前干等转换（timeout_secs = 0 时这段等待没有上界）
    isolate_env_proxy();
    let (upstream, count, _jh) = fixed_upstream(StatusCode::OK, "ok-payload").await;
    let mut cfg = keepalive_1s_config_for(&upstream);
    cfg.request_transform = Some(TransformConfig {
        args: vec!["sleep".to_string(), "2500".to_string()],
        ..transform_config("sleep", TransformMode::Spawn)
    });
    let (proxy, _pj) = start_proxy(cfg).await;

    let (head_after, status, text) = timed_sse_post(&proxy).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        head_after < std::time::Duration::from_millis(2200),
        "骨架头应在约一个保活间隔后到达、早于请求转换完成（2.5s）: {head_after:?}"
    );
    assert!(text.contains(": keepalive"), "转换期间应有心跳: {text}");
    assert!(
        text.contains("ok-payload"),
        "转换完成后应照常转发并回放: {text}"
    );
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn keepalive_request_transform_failure_after_commit_ends_with_error_event() {
    // 请求转换在骨架头提交之后才失败（超时 2s > 保活间隔 1s）：状态行已发出，
    // 不能再回 502，以终态 SSE error 事件收场；请求未发往上游
    isolate_env_proxy();
    let (upstream, count, _jh) = fixed_upstream(StatusCode::OK, "never").await;
    let mut cfg = keepalive_1s_config_for(&upstream);
    cfg.request_transform = Some(TransformConfig {
        args: vec!["sleep".to_string(), "5000".to_string()],
        timeout_secs: Some(2),
        ..transform_config("sleep", TransformMode::Spawn)
    });
    let (proxy, _pj) = start_proxy(cfg).await;

    let (_, status, text) = timed_sse_post(&proxy).await;
    assert_eq!(status, StatusCode::OK, "骨架头已提交");
    assert!(
        text.contains("event: error") && text.contains("proxy_transform_failed"),
        "已提交后请求转换失败应以终态 error 事件收场: {text}"
    );
    assert!(
        text.contains("请求转换失败"),
        "error 事件应带失败原因: {text}"
    );
    assert_eq!(count.load(Ordering::SeqCst), 0, "请求转换失败不得发往上游");
}

#[tokio::test]
async fn keepalive_covers_slow_response_transform() {
    // 上游很快回 SSE 成功、响应转换耗时 2.5s：转换经心跳节拍驱动，骨架头约 1s
    // 到达、之后心跳，转换完成后回放转换后的 body
    isolate_env_proxy();
    let app = Router::new().fallback(|| async {
        (
            [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
            "data: ok\n\n",
        )
            .into_response()
    });
    let (upstream, _jh) = bind_router(app).await;
    let mut cfg = keepalive_1s_config_for(&upstream);
    cfg.response_transform = Some(TransformConfig {
        args: vec!["sleep".to_string(), "2500".to_string()],
        ..transform_config("sleep", TransformMode::Spawn)
    });
    let (proxy, _pj) = start_proxy(cfg).await;

    let (head_after, status, text) = timed_sse_post(&proxy).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        head_after < std::time::Duration::from_millis(2200),
        "骨架头应在约一个保活间隔后到达、早于响应转换完成（2.5s）: {head_after:?}"
    );
    assert!(text.contains(": keepalive"), "转换期间应有心跳: {text}");
    assert!(text.contains("data: ok"), "应回放转换后的 body: {text}");
}

// ---------------------------------------------------------------------------
// 进程池加固：stdout 错位不得串包、复用的死 worker 不得让请求失败
// ---------------------------------------------------------------------------

/// persistent + pool_max=1：所有请求必经同一个 worker 槽位——stdout 一旦
/// 错位，下一个请求必然踩中，串包若存在一定暴露（多槽位会让错位概率性漏测）。
fn single_worker_persistent(sub_args: &[&str]) -> TransformConfig {
    TransformConfig {
        args: sub_args.iter().map(|s| s.to_string()).collect(),
        pool_max: Some(1),
        ..transform_config(sub_args[0], TransformMode::Persistent)
    }
}

async fn post_body(client: &Client, proxy: &str, body: &str) -> (StatusCode, String) {
    let resp = client
        .post(format!("{proxy}/v1/x"))
        .body(body.to_string())
        .send()
        .await
        .unwrap();
    let status = resp.status();
    (status, resp.text().await.unwrap())
}

/// 串包判据：200 时回显的必须是**本请求自己的** body（recording_upstream 原样
/// 回显上游收到的 body，所以回显 = 上游实际收到的内容）——绝不允许「200 但
/// 内容是别的请求的」。失败形态由各测试另行断言。
fn assert_not_crosswired(status: StatusCode, text: &str, sent: &str) {
    if status == StatusCode::OK {
        assert_eq!(
            text, sent,
            "串包：本请求发的是 {sent}，上游收到的却是 {text}（读到了上一个请求的输出）"
        );
    }
}

#[tokio::test]
async fn request_persistent_banner_fails_loudly_and_never_crosswires() {
    isolate_env_proxy();
    // 启动横幅让 stdout 永远比请求多一行：每个新 worker 读到的第一行都是
    // 横幅 → 协议错误 + 剔除。剔除才是关键——留在池里的话，下一个请求会读到
    // 上一个请求的回显（A 的 body 被当成 B 的发往上游）
    let (upstream, log, _jh) = recording_upstream().await;
    let mut cfg = proxy_config_for(&upstream);
    cfg.request_transform = Some(single_worker_persistent(&["banner"]));
    let (proxy, _pj) = start_proxy(cfg).await;
    let client = local_client();

    // 先跑完全部请求、逐个做串包判定，错误归类放到最后断言：串包是首要
    // 缺陷，归类文案是次要的——断言顺序让串包优先暴露
    let mut texts = Vec::new();
    for sent in ["AAA", "BBB", "CCC"] {
        let (status, text) = post_body(&client, &proxy, sent).await;
        assert_not_crosswired(status, &text, sent);
        assert_eq!(
            status,
            StatusCode::BAD_GATEWAY,
            "横幅必定先于回显被读到，每个请求都应失败: {text}"
        );
        texts.push(text);
    }
    assert!(
        log.lock().unwrap().is_empty(),
        "协议错误的请求一个都不得发往上游"
    );
    for text in texts {
        assert!(
            text.contains("违反信封协议"),
            "应归类为协议错误（而非 format 自报的失败）: {text}"
        );
    }
}

#[tokio::test]
async fn request_persistent_multiline_never_crosswires() {
    isolate_env_proxy();
    // 每请求回两行**合法**信封：错位后解析照样成功——这是静默串包的形态
    // （修复前：第二个请求把第一个请求的 body 发往上游，客户端拿到 200）
    let (upstream, log, _jh) = recording_upstream().await;
    let mut cfg = proxy_config_for(&upstream);
    cfg.request_transform = Some(single_worker_persistent(&["multiline"]));
    let (proxy, _pj) = start_proxy(cfg).await;
    let client = local_client();

    for sent in ["AAA", "BBB", "CCC", "DDD"] {
        let (status, text) = post_body(&client, &proxy, sent).await;
        assert_not_crosswired(status, &text, sent);
        // 两行同批到达时读完首行即见残行 → 协议错误；残行若分批晚到，则在
        // 下次取用该 worker 前被探测剔除、本请求成功——两种结局都不串包
        assert!(
            status == StatusCode::OK || text.contains("违反信封协议"),
            "失败只应是协议错误: {status} {text}"
        );
    }
    // 上游视角复核：收到的每个 body 都只出现一次（错位重放会出现重复）
    let entries = log.lock().unwrap();
    let mut bodies: Vec<&[u8]> = entries.iter().map(|e| e.body.as_slice()).collect();
    bodies.sort_unstable();
    bodies.dedup();
    assert_eq!(
        bodies.len(),
        entries.len(),
        "上游收到了重复 body（错位重放）"
    );
}

#[tokio::test]
async fn request_persistent_late_stray_line_never_crosswires() {
    isolate_env_proxy();
    // 多余的一行（合法信封）晚于回复 100ms 才到：读回复时它还不在，worker
    // 已归还空闲表——必须在下次取用前探测到并剔除，否则下一个请求静默串包
    let (upstream, log, _jh) = recording_upstream().await;
    let mut cfg = proxy_config_for(&upstream);
    cfg.request_transform = Some(single_worker_persistent(&["late-stray", "100"]));
    let (proxy, _pj) = start_proxy(cfg).await;
    let client = local_client();

    for sent in ["AAA", "BBB", "CCC"] {
        let (status, text) = post_body(&client, &proxy, sent).await;
        assert_not_crosswired(status, &text, sent);
        assert_eq!(status, StatusCode::OK, "回复本身合法，请求应成功: {text}");
        // 等多余行落进管道：下一个请求取 worker 时它已可见
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    }
    assert_eq!(log.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn response_persistent_multiline_never_crosswires() {
    isolate_env_proxy();
    // 响应侧串包 = A 会话的模型回复回放给 B。echo 上游按请求回显各自 body，
    // 响应转换无论成功（multiline 首行是原样回显）还是失败（透传原始响应），
    // 客户端拿到的都必须是自己的内容
    let (upstream, _captured, _jh) = echo_upstream().await;
    let mut cfg = proxy_config_for(&upstream);
    cfg.response_transform = Some(single_worker_persistent(&["multiline"]));
    let (proxy, _pj) = start_proxy(cfg).await;
    let client = local_client();

    for sent in ["r-one", "r-two", "r-three", "r-four"] {
        let (status, text) = post_body(&client, &proxy, sent).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            text, sent,
            "响应串包：本请求应收到自己的回显，却收到了别的请求的响应"
        );
    }
}

#[tokio::test]
async fn request_persistent_idle_worker_death_respawns() {
    isolate_env_proxy();
    // worker 处理完首个请求后空闲 200ms 自行退出：空闲表里留下死 worker。
    // 下一个请求必须换新 worker 成功，而不是写进死管道后 502
    let (upstream, log, _jh) = recording_upstream().await;
    let mut cfg = proxy_config_for(&upstream);
    cfg.request_transform = Some(single_worker_persistent(&["die-idle", "200"]));
    let (proxy, _pj) = start_proxy(cfg).await;
    let client = local_client();

    let (s1, t1) = post_body(&client, &proxy, "AAA").await;
    assert_eq!((s1, t1.as_str()), (StatusCode::OK, "AAA"));
    // 远超空闲阈值：确保 worker 已退出（进程退出在 Windows 上也有收尾延迟）
    tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
    let (s2, t2) = post_body(&client, &proxy, "BBB").await;
    assert_eq!(
        (s2, t2.as_str()),
        (StatusCode::OK, "BBB"),
        "空闲期死掉的 worker 不得让下一个请求失败"
    );
    assert_eq!(log.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn request_persistent_reused_worker_dying_on_write_is_retried_once() {
    isolate_env_proxy();
    // oneshot：worker 只服务首个请求，之后收到输入即无输出退出——复用它的
    // 请求「写入成功、读到 EOF」，尚未产生任何输出，换新 worker 重试一次即成功
    let (upstream, log, _jh) = recording_upstream().await;
    let mut cfg = proxy_config_for(&upstream);
    cfg.request_transform = Some(single_worker_persistent(&["oneshot"]));
    let (proxy, _pj) = start_proxy(cfg).await;
    let client = local_client();

    for sent in ["AAA", "BBB", "CCC"] {
        let (status, text) = post_body(&client, &proxy, sent).await;
        assert_eq!(
            (status, text.as_str()),
            (StatusCode::OK, sent),
            "复用的 worker 死于产出前：应换新 worker 重试成功"
        );
    }
    assert_eq!(log.lock().unwrap().len(), 3);
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
        .env("APROXY_HOME", dir.path())
        .env("NO_PROXY", "127.0.0.1,localhost")
        .output()
        .unwrap();
    assert!(!out.status.success(), "互斥配置应启动失败");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("互斥"), "错误应说明互斥语义: {stderr}");
}
