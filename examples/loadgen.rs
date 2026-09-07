//! 压测客户端：向 aproxy 打 N 路并发流式请求，全部接收至结束（流式丢弃），
//! 输出吞吐/延迟/错误统计。
//!
//! 设计要点：请求体一份（Arc<Vec<u8>>，reqwest body 用 Bytes 零拷贝共享），
//! N 个并发共享同一份——压测中内存研究对象只有 aproxy，客户端自身内存
//! 恒定。逐请求计时并统计 P50/P99 与总吞吐。

use bytes::Bytes;
use std::{sync::Arc, time::{Duration, Instant}};

#[derive(Clone)]
struct Args {
    target: String,
    concurrency: usize,
    requests: usize,
    timeout: Duration,
    query: String,
}

fn parse_args() -> Args {
    let mut a = Args {
        target: "http://127.0.0.1:25990".into(),
        concurrency: 10,
        requests: 10,
        timeout: Duration::from_secs(600),
        query: String::new(),
    };
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        let mut val = || it.next().expect("缺少参数值");
        match k.as_str() {
            "--target" => a.target = val(),
            "-c" => a.concurrency = val().parse().expect("-c 需数字"),
            "-n" => a.requests = val().parse().expect("-n 需数字"),
            "--timeout-secs" => a.timeout = Duration::from_secs(val().parse().expect("需数字")),
            "--query" => a.query = val(),
            other => panic!("未知参数: {other}"),
        }
    }
    a
}

/// 请求体：对齐「50 万 token 会话」画像。纯文本填充约 2 MiB
///（Anthropic 每 token ≈ 4 字符），每请求再带 50KiB（模拟若干图片的
/// base64 碎片）让体量接近真实混合负载。
fn build_body() -> Vec<u8> {
    let mut b = Vec::with_capacity(2 * 1024 * 1024 + 64 * 1024);
    b.extend_from_slice(b"{\"model\":\"claude-sonnet-5\",\"messages\":[{\"role\":\"user\",\"content\":[");
    b.extend_from_slice(b"{\"type\":\"text\",\"text\":\"");
    b.resize(b.len() + 2 * 1024 * 1024, b'a');
    b.extend_from_slice(b"\"},{\"type\":\"image\",\"source\":{\"data\":\"");
    b.resize(b.len() + 64 * 1024, b'Q');
    b.extend_from_slice(b"\"}}]}]}");
    b
}

#[tokio::main]
async fn main() {
    let args = Arc::new(parse_args());
    let body = Arc::new(Bytes::from(build_body()));
    eprintln!(
        "loadgen: target={} c={} n={} body={} KiB query='{}'",
        args.target,
        args.concurrency,
        args.requests,
        body.len() / 1024,
        args.query
    );

    let client = reqwest::Client::builder()
        .timeout(args.timeout)
        .build()
        .expect("构建 client");

    // 每槽串行领请求，直到领完
    let url = format!(
        "{}/v1/messages?stream=true{}",
        args.target.trim_end_matches('/'),
        args.query
    );
    let mut handles = Vec::with_capacity(args.concurrency);
    for slot in 0..args.concurrency {
        let client = client.clone();
        let url = url.clone();
        let body = body.clone();
        let args = args.clone();
        handles.push(tokio::spawn(async move {
            let mut latencies = Vec::new();
            let mut bytes_rx = 0u64;
            let mut errors = Vec::new();
            let mut i = slot;
            while i < args.requests {
                let t0 = Instant::now();
                let req = client
                    .post(&url)
                    .header("content-type", "application/json")
                    .header("accept", "text/event-stream")
                    .body(reqwest::Body::from(Bytes::clone(&body)));
                match req.send().await {
                    Ok(resp) => {
                        if !resp.status().is_success() {
                            errors.push(format!("{} HTTP {}", t0.elapsed().as_millis(), resp.status()));
                        } else {
                            // 流式接收至结束：内存恒定（逐 chunk 丢弃），计入总接收量
                            let mut resp = resp;
                            while let Ok(Some(chunk)) = resp.chunk().await {
                                bytes_rx += chunk.len() as u64;
                            }
                            latencies.push(t0.elapsed());
                        }
                    }
                    Err(e) => errors.push(format!("{} net {}", t0.elapsed().as_millis(), e)),
                }
                i += args.concurrency;
            }
            (latencies, bytes_rx, errors)
        }));
    }

    let t_start = Instant::now();
    let mut all_lat = Vec::new();
    let mut total_rx = 0u64;
    let mut all_err = Vec::new();
    for h in handles {
        let (lat, rx, err) = h.await.expect("worker panic");
        all_lat.extend(lat);
        total_rx += rx;
        all_err.extend(err);
    }
    let elapsed = t_start.elapsed();

    all_lat.sort();
    let pick = |p: f64| -> f64 {
        if all_lat.is_empty() {
            return 0.0;
        }
        all_lat[((all_lat.len() as f64 - 1.0) * p).round() as usize].as_secs_f64()
    };
    let stats = serde_json::json!({
        "concurrency": args.concurrency,
        "requests": args.requests,
        "elapsed_s": elapsed.as_secs_f64(),
        "rps": all_lat.len() as f64 / elapsed.as_secs_f64(),
        "ok": all_lat.len(),
        "failed": all_err.len(),
        "bytes_rx_mb": total_rx as f64 / 1024.0 / 1024.0,
        "throughput_mb_s": total_rx as f64 / 1024.0 / 1024.0 / elapsed.as_secs_f64(),
        "latency_s": { "p50": pick(0.50), "p99": pick(0.99), "max": pick(1.0) },
    });
    println!("{stats}");
    if !all_err.is_empty() {
        eprintln!("错误样例（前 5）:");
        for e in all_err.iter().take(5) {
            eprintln!("  {e}");
        }
    }
}
