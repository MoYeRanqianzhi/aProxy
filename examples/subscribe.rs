//! 压测与排查用的事件订阅者：连上一个实例的控制通道，订阅事件，一直读到实例关闭连接或
//! 到了给定的时长，然后输出统计。
//!
//! 用途：
//! - 压测时充当「一个订阅者」，看有人订阅时代理的吞吐与延迟；
//! - 核对事件的限频：同一个请求在任何一秒的窗口里最多有几个 `request` 事件
//!   （开始与结束各一个不受限，所以一个很快结束的请求可以有 2 个；中间的变化每秒最多 1 个）。
//!   限频是在实例产生事件的那一刻算的，这里看到的时刻是事件写上连接的时刻，前后差着几毫秒
//!   的排队时间，所以两个相隔整一秒产生的事件在这里可能落进同一个一秒的窗口；
//!   `min_gap_ms_between_intermediate_events` 给出中间事件之间实际的最小间隔，它应当贴近
//!   1000。
//!
//! 用法：`subscribe --home <aproxy 的 home> --port <端口> [--secs <秒数>]`。home 一定要给：
//! 它决定去哪个 run 目录找实例，不默认落到用户真实的 home 上。

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

#[tokio::main]
async fn main() {
    let (mut home, mut port, mut secs) = (None, None, None);
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        let value = args.next().expect("缺少参数值");
        match flag.as_str() {
            "--home" => home = Some(value),
            "--port" => port = Some(value),
            "--secs" => secs = Some(value.parse::<u64>().expect("--secs 需数字")),
            other => panic!("未知参数: {other}"),
        }
    }
    let run_dir = std::path::PathBuf::from(home.expect("需要 --home")).join("run");
    let port = port.expect("需要 --port");

    let (first, mut events) = aproxy::daemon::ipc_subscribe_in(&run_dir, &port)
        .await
        .expect("订阅失败");
    eprintln!(
        "subscribe: 已订阅，实例状态 {}，在途 {} 个请求",
        first["snapshot"]["state"],
        first["requests"].as_array().map_or(0, Vec::len)
    );

    let deadline = secs.map(|secs| Instant::now() + Duration::from_secs(secs));
    // 各种事件的个数
    let mut counts: HashMap<String, u64> = HashMap::new();
    // 每个请求的 request 事件的时刻（毫秒，事件自带的 `at`）
    let mut per_request: HashMap<String, Vec<u64>> = HashMap::new();
    let (mut bytes, mut largest, mut lagged, mut last_seq) = (0u64, 0usize, 0u64, 0u64);
    let mut seq_gaps = 0u64;
    loop {
        let next = match deadline {
            Some(deadline) => {
                let left = deadline.saturating_duration_since(Instant::now());
                match tokio::time::timeout(left, events.next()).await {
                    Ok(next) => next,
                    Err(_) => break,
                }
            }
            None => events.next().await,
        };
        let event = match next {
            Ok(Some(event)) => event,
            Ok(None) => break,
            Err(e) => {
                eprintln!("subscribe: 连接中断：{e}");
                break;
            }
        };
        let size = event.to_string().len();
        bytes += size as u64;
        let seq = event["seq"].as_u64().unwrap_or(0);
        if seq != last_seq + 1 {
            seq_gaps += 1;
        }
        last_seq = seq;
        let name = event["event"].as_str().unwrap_or("?").to_string();
        match name.as_str() {
            "request" => {
                largest = largest.max(event["data"].to_string().len());
                if let Some(id) = event["data"]["request_id"].as_str() {
                    per_request
                        .entry(id.to_string())
                        .or_default()
                        .push(event["at"].as_u64().unwrap_or(0));
                }
            }
            "lagged" => lagged += event["data"]["dropped"].as_u64().unwrap_or(0),
            _ => {}
        }
        *counts.entry(name).or_default() += 1;
    }

    // 同一个请求在任何一秒的窗口里最多有几个事件；以及去掉首尾（开始、结束）之后的最多几个
    let most_in_a_second = |times: &[u64]| {
        (0..times.len())
            .map(|from| {
                times[from..]
                    .iter()
                    .take_while(|at| **at < times[from] + 1000)
                    .count()
            })
            .max()
            .unwrap_or(0)
    };
    let mut worst = 0;
    let mut worst_middle = 0;
    // 同一个请求相邻两个中间事件之间的最小间隔（毫秒）；没有哪个请求有两个中间事件时为 null
    let mut min_gap: Option<u64> = None;
    for times in per_request.values() {
        worst = worst.max(most_in_a_second(times));
        if times.len() > 2 {
            let middle = &times[1..times.len() - 1];
            worst_middle = worst_middle.max(most_in_a_second(middle));
            for pair in middle.windows(2) {
                let gap = pair[1].saturating_sub(pair[0]);
                min_gap = Some(min_gap.map_or(gap, |least| least.min(gap)));
            }
        }
    }
    println!(
        "{}",
        serde_json::json!({
            "events": counts,
            "requests_seen": per_request.len(),
            "bytes": bytes,
            "largest_record_bytes": largest,
            "lagged_dropped": lagged,
            "seq_gaps": seq_gaps,
            "max_request_events_per_request_in_1s": worst,
            "max_intermediate_events_per_request_in_1s": worst_middle,
            "min_gap_ms_between_intermediate_events": min_gap,
        })
    );
}
