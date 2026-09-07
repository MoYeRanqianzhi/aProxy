//! 压测专用 mock 上游：把任意 POST 转成限速流式 SSE 响应。
//!
//! 设计约束：自身内存恒定（请求体丢弃式读取、响应流按批生成不缓冲）——
//! 压测中唯一的内存研究对象是 aproxy 的 spool，mock 不得成为干扰源。
//! 负载画像对齐真实 LLM 上游：请求体 50 万 token 量级（~2MB 文本，含
//! 图片可达 50MB），响应为数万条流式 delta 消息，且生成有在途时长
//! （限速吐出），使「并发数」等价于「同时在途请求数」。
//!
//! query 参数（每请求可控，压测矩阵可变负载）：
//! - `msgs`：流式消息条数（默认 50000，对齐「数万条流式消息」画像）
//! - `pad`：每条消息填充字节数（默认 40，单条约 150 字节）
//! - `rate_mb_s`：响应吐出速率上限 MB/s（默认 5，0=不限速）

use axum::{
    Router,
    body::Body,
    extract::{Query, Request},
    http::StatusCode,
    response::Response,
};
use bytes::Bytes;
use futures_util::stream::unfold;
use http_body_util::BodyExt;
use std::{collections::HashMap, time::Duration};

const MSG_PREFIX: &str = "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"";
const MSG_SUFFIX: &str = "\"}\n\n";
const MSG_STOP: &[u8] = b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";
/// 每个产出 chunk 攒的条数（约 15KB：逐条 150 字节会产生过多系统调用，
/// 过大则限速粒度变粗——100 条是吞吐与平滑的折衷）
const BATCH_MSGS: usize = 100;

async fn handle(Query(params): Query<HashMap<String, String>>, req: Request) -> Response {
    // 丢弃式读请求体：mock 不缓冲、不解析，只消耗转发带宽
    let mut body = req.into_body();
    while let Some(frame) = body.frame().await {
        match frame {
            Ok(f) => drop(f.into_data()),
            Err(_) => break,
        }
    }

    // query 只识别本 mock 的调参键，其余（如 stream=true）容忍忽略——
    // 真实上游不会因 query 带了别的键而 400
    let num = |k: &str, d: u64| -> u64 { params.get(k).and_then(|v| v.parse().ok()).unwrap_or(d) };
    let msgs = num("msgs", 50_000) as usize;
    let pad = num("pad", 40) as usize;
    let rate = (num("rate_mb_s", 5) as f64) * 1024.0 * 1024.0;

    let msg_len = MSG_PREFIX.len() + pad + MSG_SUFFIX.len();
    let stream = unfold(0usize, move |sent| async move {
        if sent >= msgs {
            return None;
        }
        let n = BATCH_MSGS.min(msgs - sent);
        let last = sent + n >= msgs;
        let mut buf = Vec::with_capacity(n * msg_len + if last { MSG_STOP.len() } else { 0 });
        for _ in 0..n {
            buf.extend_from_slice(MSG_PREFIX.as_bytes());
            buf.resize(buf.len() + pad, b'x');
            buf.extend_from_slice(MSG_SUFFIX.as_bytes());
        }
        if last {
            buf.extend_from_slice(MSG_STOP);
        }
        if rate > 0.0 {
            tokio::time::sleep(Duration::from_secs_f64(buf.len() as f64 / rate)).await;
        }
        Some((Ok::<Bytes, std::io::Error>(Bytes::from(buf)), sent + n))
    });

    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .header("cache-control", "no-cache")
        .body(Body::from_stream(stream))
        .unwrap()
}

#[tokio::main]
async fn main() {
    let port: u16 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(28081);
    let app = Router::new().fallback(handle);
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .unwrap_or_else(|e| panic!("mock upstream bind {addr} 失败: {e}"));
    eprintln!("mock-upstream listening on {addr}");
    axum::serve(listener, app).await.unwrap();
}
