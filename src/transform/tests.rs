//! 管道测试只使用本地示例进程；进程号检验生命期，worker_id 检验同时占用的槽位。
use super::super::{format_echo_path, headers_from_btreemap, headers_to_btreemap};
use super::*;
use aproxy_envelope as wire;
use std::{collections::BTreeMap, path::Path};
/// 独立的挂死保护，不用完成耗时判定行为；调用处核对同时持有的租约、进程帧或实际 EOF。
async fn until_observed<T>(work: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(30), work)
        .await
        .expect("worker observation never arrived")
}
/// 构造与正式配置相同的条目；绝对路径保证示例与运行测试的构建一致。
fn pool(mode: Process, sub: &str, edit: impl FnOnce(&mut FormatEntry)) -> Arc<Pool> {
    let (cfg, diags) = crate::config::file::parse(
        "[[format]]\ncommand='f'\nstages=['request']",
        Path::new("unit.toml"),
    );
    assert!(diags.is_empty());
    let mut e = cfg.format.unwrap().pop().unwrap();
    e.command = format_echo_path().display().to_string();
    e.args = vec![sub.into()];
    e.process = mode;
    e.timeout_secs = 5;
    edit(&mut e);
    Pool::new(
        e,
        PathBuf::new(),
        Secrets::default(),
        Default::default(),
        [1024 * 1024; 2],
    )
}
/// 每次发送不同的正文；从错误的进程或旧序号取到的回信不能冒充本次回显。
async fn call(p: &Arc<Pool>, body: &str, fresh: bool) -> Result<Reply, Fault> {
    p.call(request_frame(body), p.entry.timeout_secs, fresh)
        .await
}
/// 构造小业务帧，供已借出进程和按需借用两种测试共用。
fn request_frame(body: &str) -> Stage {
    let mut req = wire::Request::new("POST".into(), "http://localhost/test".into());
    req.body = wire::Body::Text(body.into());
    Stage::Request(wire::RequestMessage::new(
        wire::Context::new("r".into()),
        wire::Delivery::Retry,
        req,
        wire::Client::new("/test".into(), Some(false)),
    ))
}
/// 假程序的 state 携带槽号与 pid；顺带核对正文确实经过了管道。
fn worker(r: Reply, body: &str) -> String {
    assert_eq!(r.request.unwrap().body.as_bytes(), Some(body.as_bytes()));
    r.state.unwrap()
}
#[test]
fn headers_to_btreemap_skips_hop_and_keeps_first_value() {
    use axum::http::{HeaderMap, HeaderValue};
    let mut headers = HeaderMap::new();
    headers.insert("content-type", HeaderValue::from_static("application/json"));
    headers.insert("content-length", HeaderValue::from_static("5"));
    headers.insert("connection", HeaderValue::from_static("close"));
    // 多值头：append 两个同名值，仅首值进信封
    headers.append("x-demo", HeaderValue::from_static("first"));
    headers.append("x-demo", HeaderValue::from_static("second"));

    let map = headers_to_btreemap(&headers);
    assert_eq!(map.get("content-type").unwrap(), "application/json");
    assert!(!map.contains_key("content-length"), "hop 头不进信封");
    assert!(!map.contains_key("connection"), "hop 头不进信封");
    assert_eq!(map.get("x-demo").unwrap(), "first", "多值头仅保留首值");
}

#[test]
fn headers_from_btreemap_filters_managed_and_invalid() {
    let mut map = BTreeMap::new();
    map.insert("content-type".to_string(), "application/json".to_string());
    map.insert("content-length".to_string(), "99".to_string());
    map.insert("x-custom".to_string(), "1".to_string());
    map.insert("[invalid header]".to_string(), "x".to_string());

    let headers = headers_from_btreemap(&map, 8).unwrap();
    assert!(headers.contains_key("content-type"));
    assert!(headers.contains_key("x-custom"));
    assert!(!headers.contains_key("content-length"), "CL 防御性剔除");
    assert_eq!(headers.len(), 2, "非法头名丢弃");
}

/// 程序提供的头表最终还要添加内容长度、错误码等代理头，不可耗尽容量后才到交付路径。
#[test]
fn accepted_format_headers_leave_space_for_delivery_headers() {
    let map = (0..6)
        .map(|i| (format!("x-field-{i}"), "v".into()))
        .collect();
    let headers = headers_from_btreemap(&map, 8).unwrap();
    assert!(headers.capacity() - headers.len() >= 8);
    // 用同一个小表模拟无法满足的容量预留；不需要把整个请求解析器逼到极限。
    assert!(matches!(
        headers_from_btreemap(&map, usize::MAX),
        Err(Fault::Invalid)
    ));
}

/// 不存在的程序返回 spawn 分类。
#[tokio::test]
async fn spawn_mode_single_request_roundtrip() {
    let p = pool(Process::Spawn, "echo", |e| {
        e.command = "nonexistent-format-unit-431".into()
    });
    assert!(matches!(call(&p, "r", false).await, Err(Fault::Spawn)));
}
/// 一来一回的文本与版本必须完整。
#[tokio::test]
async fn convert_smoke_real_format_echo_roundtrip() {
    let p = pool(Process::Spawn, "echo", |_| {});
    assert!(worker(call(&p, "{}", false).await.unwrap(), "{}").starts_with("0/"));
}
/// 分类要能解释错误，又不把未受信的输出复制到诊断。
#[test]
fn transform_error_display_carries_reason() {
    assert!(Fault::Timeout.to_string().contains("timed out"));
    assert!(Fault::Protocol.to_string().contains("sequence"));
    assert!(
        Fault::Stages(vec!["request".into()])
            .to_string()
            .contains("stages")
    );
}
/// 第一个一次性进程已读到请求但不能回信，第二个仍可完成；线上编号相同，实际 PID 不同。
#[tokio::test]
async fn spawn_messages_overlap_up_to_pool_max() {
    let dir = tempfile::tempdir().unwrap();
    let gate = dir.path().join("release");
    let obs = dir.path().join("seen");
    let p = pool(Process::Spawn, "gate-request", |e| {
        e.pool_max = 2;
        e.timeout_secs = 0;
        e.args.push(gate.display().to_string());
        e.env
            .insert("ECHO_OBSERVE".into(), obs.display().to_string());
    });
    let mut first = std::pin::pin!(call(&p, "blocked", false));
    tokio::select! {_=&mut first=>panic!("completed before release"),_=wait_observed(&obs,"request",1)=>{}}
    assert_eq!(
        p.turns.available_permits(),
        1,
        "second message cannot enter"
    );
    let second = worker(call(&p, "other", false).await.unwrap(), "other");
    std::fs::write(gate, b"ok").unwrap();
    let first = worker(first.await.unwrap(), "blocked");
    assert!(first.starts_with("0/") && second.starts_with("0/"));
    assert_ne!(first, second);
}
/// persistent 复用的是原进程，而不是碰巧编号相同的另一个进程。
#[tokio::test]
async fn persistent_mode_reuses_worker_across_requests() {
    let p = pool(Process::Persistent, "echo", |_| {});
    let a = worker(call(&p, "r1", false).await.unwrap(), "r1");
    assert_eq!(worker(call(&p, "r2", false).await.unwrap(), "r2"), a);
    assert_eq!(p.idle.lock().unwrap().len(), 1);
}
/// 空闲回收后，进程不同但取最小空闲号，仍是 0。
#[tokio::test]
async fn idle_worker_is_reaped_after_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let obs = dir.path().join("events");
    let p = pool(Process::Persistent, "echo", |e| {
        // 程序看见 EOF 不表示它已经退出；单进程池让下一次取号等到旧槽真正释放。
        e.pool_max = 1;
        e.worker_idle_secs = 1;
        e.env
            .insert("ECHO_OBSERVE".into(), obs.display().to_string());
    });
    let a = worker(call(&p, "a", false).await.unwrap(), "a");
    wait_observed(&obs, "eof", 1).await;
    assert!(p.idle.lock().unwrap().is_empty());
    let b = worker(call(&p, "b", false).await.unwrap(), "b");
    assert_ne!(a, b);
    assert!(a.starts_with("0/") && b.starts_with("0/"));
}
/// 两次崩溃是两个真正的往返失败，不能回收死进程。
#[tokio::test]
async fn crashed_worker_is_evicted_and_next_request_respawns() {
    let p = pool(Process::Persistent, "exit1", |_| {});
    for b in ["a", "b"] {
        assert!(matches!(call(&p, b, false).await, Err(Fault::Died)));
        assert!(p.idle.lock().unwrap().is_empty());
    }
}
/// 超时后即使回复稍后到达，也不会被下个请求拿走。
#[tokio::test]
async fn per_request_timeout_evicts_stuck_worker() {
    let temp = tempfile::tempdir().unwrap();
    let seen = temp.path().join("events");
    let p = pool(Process::Persistent, "no-stage-reply", |e| {
        e.timeout_secs = 3600;
        e.env
            .insert("ECHO_OBSERVE".into(), seen.display().to_string());
    });
    let pending = p.clone();
    let task = tokio::spawn(async move { call(&pending, "s", false).await });
    wait_observed(&seen, "request", 1).await;
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(3601)).await;
    assert!(matches!(task.await.unwrap(), Err(Fault::Timeout)));
    assert!(p.idle.lock().unwrap().is_empty());
}
/// 两个重叠的往返使用不同的槽位。
#[tokio::test]
async fn concurrent_requests_within_pool_max() {
    let dir = tempfile::tempdir().unwrap();
    let gate = dir.path().join("release");
    let obs = dir.path().join("seen");
    let p = pool(Process::Persistent, "gate-request", |e| {
        e.pool_max = 2;
        e.timeout_secs = 0;
        e.args.push(gate.display().to_string());
        e.env
            .insert("ECHO_OBSERVE".into(), obs.display().to_string());
    });
    let mut a = std::pin::pin!(call(&p, "blocked", false));
    tokio::select! {_=&mut a=>panic!("completed before release"),_=wait_observed(&obs,"request",1)=>{}}
    let b = call(&p, "b", false).await.unwrap();
    std::fs::write(gate, b"ok").unwrap();
    let ids = [worker(a.await.unwrap(), "blocked"), worker(b, "b")];
    for n in ["0/", "1/"] {
        assert!(ids.iter().any(|s| s.starts_with(n)));
    }
}
/// 两次 spawn 的槽都叫 0，进程本身不能复用。
#[tokio::test]
async fn spawn_mode_borrowed_worker_never_reused() {
    let p = pool(Process::Spawn, "echo", |_| {});
    let a = worker(call(&p, "a", false).await.unwrap(), "a");
    let b = worker(call(&p, "b", false).await.unwrap(), "b");
    assert_ne!(a, b);
    assert!(a.starts_with("0/") && b.starts_with("0/"));
    assert!(p.idle.lock().unwrap().is_empty());
}
/// idle=0 不启动回收计时。
#[tokio::test]
async fn idle_zero_means_never_reaped() {
    let p = pool(Process::Persistent, "echo", |e| e.worker_idle_secs = 0);
    call(&p, "keep", false).await.unwrap();
    assert!(p.reaper.get().is_none());
    assert_eq!(p.idle.lock().unwrap().len(), 1);
}
/// 横幅破坏握手，无论如何分类都不是一次业务拒绝。
#[tokio::test]
async fn banner_line_is_protocol_error_and_evicts_worker() {
    let p = pool(Process::Persistent, "banner", |_| {});
    for b in ["a", "b"] {
        assert!(matches!(
            call(&p, b, false).await,
            Err(Fault::Protocol | Fault::Invalid)
        ));
        assert!(p.idle.lock().unwrap().is_empty());
    }
}
/// 同批进来的两行是协议错；不让合法 JSON 掩盖帧错位。
#[tokio::test]
async fn multiline_reply_in_one_batch_is_protocol_error() {
    let p = pool(Process::Persistent, "multiline", |_| {});
    for b in ["a", "b", "c"] {
        assert!(matches!(call(&p, b, false).await, Err(Fault::Protocol)));
        assert!(p.idle.lock().unwrap().is_empty());
    }
}
/// 归还后才多写的一帧由取用前的检查剔除。
#[tokio::test]
async fn stray_line_arriving_while_idle_is_evicted_before_reuse() {
    let dir = tempfile::tempdir().unwrap();
    let obs = dir.path().join("observed");
    let p = pool(Process::Persistent, "late-stray", |e| {
        e.args.push(dir.path().display().to_string());
        e.env
            .insert("ECHO_OBSERVE".into(), obs.display().to_string());
        e.pool_max = 1;
    });
    let a = worker(call(&p, "s1", false).await.unwrap(), "s1");
    let pid = a.split_once('/').unwrap().1;
    std::fs::write(dir.path().join(format!("release-{pid}")), b"").unwrap();
    // Unix 的单线程反应器故意不让出：已刷到管道的帧不一定已经触发 Tokio 的就绪通知。
    #[cfg(unix)]
    {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !std::fs::read_to_string(&obs)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .any(|v| v["event"] == "stray-written")
        {
            assert!(std::time::Instant::now() < deadline, "no post-flush marker");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    #[cfg(not(unix))]
    wait_observed(&obs, "stray-written", 1).await;
    assert_ne!(a, worker(call(&p, "s2", false).await.unwrap(), "s2"));
}
/// 合法的 fail 不破坏会话。
#[tokio::test]
async fn error_line_keeps_worker_in_pool() {
    let p = pool(Process::Persistent, "error", |_| {});
    assert!(call(&p, "e", false).await.unwrap().fail.is_some());
    assert_eq!(p.idle.lock().unwrap().len(), 1);
}
/// 即使还没超过本地的空闲时限，也要先检查进程是否仍然活着。
#[tokio::test]
async fn idle_worker_that_exited_is_skipped_on_take() {
    let p = pool(Process::Persistent, "die-idle", |e| {
        e.args.push("100".into())
    });
    let a = worker(call(&p, "d1", false).await.unwrap(), "d1");
    tokio::time::timeout(Duration::from_secs(5), async {
        while !p.ids.lock().unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("idle worker was not reaped");
    let b = worker(call(&p, "d2", false).await.unwrap(), "d2");
    assert_ne!(a, b);
    assert!(a.starts_with("0/") && b.starts_with("0/"), "{a}, {b}");
}
/// 复用时遇到 EOF 先交回失败；上层明确派发到新 worker 才会重做。
#[tokio::test]
async fn reused_worker_dead_before_output_retries_on_fresh_worker() {
    let p = pool(Process::Persistent, "oneshot", |_| {});
    let a = worker(call(&p, "o1", false).await.unwrap(), "o1");
    assert!(matches!(call(&p, "o2", false).await, Err(Fault::Died)));
    assert_ne!(a, worker(call(&p, "o2", true).await.unwrap(), "o2"));
}
/// 新会话也失败时，不在管道内部偷偷重试；每个新进程都能从标记文件数出。
#[tokio::test]
async fn retry_is_bounded_when_fresh_worker_also_dies() {
    let d = tempfile::tempdir().unwrap();
    let marker = d.path().join("spawns");
    let p = pool(Process::Persistent, "oneshot", |e| {
        e.args.push(marker.display().to_string())
    });
    call(&p, "first", false).await.unwrap();
    for fresh in [false, true] {
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(15), call(&p, "x", fresh))
                .await
                .unwrap(),
            Err(Fault::Died)
        ));
    }
    assert_eq!(std::fs::read_to_string(marker).unwrap().lines().count(), 2);
    assert!(p.idle.lock().unwrap().is_empty());
}
/// 阶段入口的一次新进程启动失败只产生一次启动记录。
#[tokio::test]
async fn fresh_worker_failure_is_not_retried() {
    let d = tempfile::tempdir().unwrap();
    let marker = d.path().join("spawns");
    std::fs::write(&marker, b"").unwrap();
    let p = pool(Process::Persistent, "oneshot", |e| {
        e.args.push(marker.display().to_string())
    });
    assert!(matches!(call(&p, "x", false).await, Err(Fault::Died)));
    assert_eq!(std::fs::read_to_string(marker).unwrap().lines().count(), 1);
}
/// 池限额为一时，并发往返必须串行到同一进程，不能靠池满后另起进程规避等待。
#[tokio::test]
async fn pool_max_one_serializes_on_single_worker() {
    let p = pool(Process::Persistent, "sleep", |e| e.pool_max = 1);
    let (a, b) = tokio::join!(call(&p, "1", false), call(&p, "2", false));
    assert_eq!(worker(a.unwrap(), "1"), worker(b.unwrap(), "2"));
    assert_eq!(p.idle.lock().unwrap().len(), 1);
}

/// 无终点的内存数据源也只读到上限；最后一个字节恰为换行时仍是一条合法的完整帧。
#[tokio::test]
async fn reply_limit_is_enforced_before_newline_or_eof() {
    let mut endless = BufReader::with_capacity(7, tokio::io::repeat(b'x'));
    assert!(matches!(
        read_reply(&mut endless, 31).await,
        Err(Fault::Invalid)
    ));
    for (input, ok) in [(b"abc\n".as_slice(), true), (b"abcd\n", false)] {
        let mut r = BufReader::with_capacity(2, input);
        assert_eq!(read_reply(&mut r, 4).await.is_ok(), ok);
    }
    let mut partial = b"{\"v\":".as_slice();
    assert!(matches!(
        read_reply(&mut partial, 100).await,
        Err(Fault::Died)
    ));
}

/// 模拟 stderr 被其他进程继承：原进程被强制丢弃后，永不就绪的读取任务仍会随 worker 取消。
#[tokio::test]
async fn evicted_worker_aborts_even_a_stderr_reader_that_never_reaches_eof() {
    let p = pool(Process::Persistent, "echo", |_| {});
    let mut t = p.turn(false).await.unwrap();
    let replacement = tokio::spawn(std::future::pending::<()>());
    let handle = replacement.abort_handle();
    std::mem::replace(&mut t.worker.stderr_abort, handle.clone()).abort();
    drop(t);
    tokio::time::timeout(Duration::from_secs(2), async {
        while !handle.is_finished() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

/// 读取由进程自己确认的事件；未写完整的一行留给下次轮询，不用睡眠猜测进程进度。
async fn wait_observed(path: &Path, event: &str, count: usize) -> Vec<serde_json::Value> {
    let path = path.to_owned();
    let event = event.to_owned();
    // 文件由独立进程写入，使用真实的挂死保护。等待文件不会推进被测试的 Tokio 时钟。
    tokio::task::spawn_blocking(move || {
        let end = std::time::Instant::now() + Duration::from_secs(15);
        loop {
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            let events: Vec<_> = text
                .lines()
                .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
                .filter(|v| v["event"] == event)
                .collect();
            if events.len() >= count {
                return events;
            }
            assert!(std::time::Instant::now() < end, "no {event} in {path:?}");
            std::thread::sleep(Duration::from_millis(5));
        }
    })
    .await
    .unwrap()
}

/// 超时和在途取消都淘汰读过 A 的进程；排队时取消不消耗一个唤醒，也不占试探名额。
#[tokio::test]
async fn cancellation_or_timeout_drops_the_exchange_and_queued_cancellation_releases_its_turn() {
    for cancel in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let obs = dir.path().join("observations");
        let p = pool(Process::Persistent, "gate-request", |e| {
            e.pool_max = 1;
            e.timeout_secs = 3600;
            e.args
                .push(dir.path().join("release").display().to_string());
            e.env
                .insert("ECHO_OBSERVE".into(), obs.display().to_string());
        });
        let p1 = p.clone();
        let first = tokio::spawn(async move { call(&p1, "blocked", false).await });
        let a = wait_observed(&obs, "request", 1).await;
        {
            let mut removed = std::pin::pin!(call(&p, "cancelled-in-queue", false));
            assert!(futures_util::poll!(&mut removed).is_pending());
        }
        if cancel {
            first.abort();
            assert!(first.await.unwrap_err().is_cancelled());
        } else {
            tokio::time::pause();
            tokio::time::advance(Duration::from_secs(3601)).await;
            assert!(matches!(first.await.unwrap(), Err(Fault::Timeout)));
            tokio::time::resume();
        }
        let b = worker(call(&p, "second", false).await.unwrap(), "second");
        assert_ne!(b.split_once('/').unwrap().1, a[0]["pid"].to_string());
        let second = wait_observed(&obs, "request", 2).await;
        assert_eq!(second.len(), 2);
        assert_eq!(second[1]["seq"], 1);
    }
}

/// 实际进程收到的 seq 随进程重置；同一个池中的第二次探测不能再抢自己的名额。
#[tokio::test]
async fn one_handshake_per_pool_and_sequences_per_live_worker() {
    let dir = tempfile::tempdir().unwrap();
    let obs = dir.path().join("observed");
    let p = pool(Process::Persistent, "echo", |e| {
        e.pool_max = 1;
        e.env
            .insert("ECHO_OBSERVE".into(), obs.display().to_string());
    });
    let (a, b) = until_observed(async { tokio::join!(p.probe(), p.probe()) }).await;
    a.unwrap();
    b.unwrap();
    for (body, fresh) in [("a", false), ("b", false), ("c", true)] {
        worker(call(&p, body, fresh).await.unwrap(), body);
    }
    let events: Vec<serde_json::Value> = std::fs::read_to_string(&obs)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .filter(|v: &serde_json::Value| v["event"] != "eof")
        .collect();
    assert_eq!(
        events
            .iter()
            .map(|e| e["seq"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        [0, 1, 2, 0, 1]
    );
    assert_eq!(events[0]["pid"], events[2]["pid"]);
    assert_ne!(events[0]["pid"], events[3]["pid"]);
    assert_eq!(events[3]["pid"], events[4]["pid"]);
    let bad = pool(Process::Persistent, "bad-seq", |_| {});
    assert!(matches!(call(&bad, "q", false).await, Err(Fault::Protocol)));
    assert!(bad.idle.lock().unwrap().is_empty());
}

/// 一个慢退出进程不能让其他进程多等一个宽限期；闲置回收器也不跟着长间隔继续睡。
#[tokio::test]
async fn normal_retirement_sends_parallel_eof_and_drop_ends_the_reaper() {
    let dir = tempfile::tempdir().unwrap();
    let obs = dir.path().join("observed");
    let gate = dir.path().join("release");
    let p = pool(Process::Persistent, "gated-eof", |e| {
        e.pool_max = 2;
        e.worker_idle_secs = 86400;
        e.args.push(gate.display().to_string());
        e.env
            .insert("ECHO_OBSERVE".into(), obs.display().to_string());
    });
    let a = p.turn(false).await.unwrap();
    let b = p.turn(false).await.unwrap();
    let (a, b) = tokio::join!(a.send(request_frame("a"), 0), b.send(request_frame("b"), 0));
    assert_ne!(worker(a.unwrap().0, "a"), worker(b.unwrap().0, "b"));
    tokio::time::pause();
    let mut shutdown = std::pin::pin!(p.shutdown());
    tokio::select! {_=&mut shutdown=>panic!("returned before both gated workers received EOF"), eof=wait_observed(&obs,"eof",2)=>assert_eq!(eof.len(),2)}
    std::fs::write(gate, b"finish").unwrap();
    tokio::time::resume();
    shutdown.await;
    let p = pool(Process::Persistent, "echo", |e| e.worker_idle_secs = 86400);
    p.probe().await.unwrap();
    let h = p.reaper.get().unwrap().abort_handle();
    drop(p);
    tokio::time::timeout(Duration::from_secs(1), async {
        while !h.is_finished() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

/// 仅条目自己的 PATH 包含这个工具，验证实际创建进程时也用该环境来解析裸名。
#[tokio::test]
async fn probe_resolves_a_bare_name_from_the_entry_path() {
    let temp = tempfile::tempdir().unwrap();
    let name = "aproxy-format-private-path-fixture";
    std::fs::copy(
        format_echo_path(),
        temp.path()
            .join(format!("{name}{}", std::env::consts::EXE_SUFFIX)),
    )
    .unwrap();
    let p = pool(Process::Spawn, "echo", |e| {
        e.command = name.into();
        e.env
            .insert("PATH".into(), temp.path().display().to_string());
    });
    p.probe().await.unwrap();
}

/// 即使一份合法应答已读完，归还前可读的额外字节仍是协议错误，EOF 则只是不可复用。
#[tokio::test]
async fn return_check_distinguishes_stray_bytes_from_clean_eof() {
    let dir = tempfile::tempdir().unwrap();
    let obs = dir.path().join("events");
    let p = pool(Process::Persistent, "late-stray", |e| {
        e.env
            .insert("ECHO_OBSERVE".into(), obs.display().to_string());
        e.args.push(dir.path().display().to_string());
    });
    let mut turn = p.turn(false).await.unwrap();
    let frame = Stage::Request(wire::RequestMessage::new(
        wire::Context::new("q".into()),
        wire::Delivery::Retry,
        wire::Request::new("POST".into(), "http://localhost/".into()),
        wire::Client::new("/".into(), Some(false)),
    ));
    p.exchange(&mut turn.worker, frame, 0, Some(&turn.ticket))
        .await
        .unwrap();
    let pid = wait_observed(&obs, "hello", 1).await[0]["pid"]
        .as_u64()
        .unwrap();
    std::fs::write(dir.path().join(format!("release-{pid}")), b"").unwrap();
    assert!(
        !until_observed(turn.worker.stdout.fill_buf())
            .await
            .unwrap()
            .is_empty()
    );
    assert!(matches!(turn.worker.reusable(), Err(Fault::Protocol)));
    let p = pool(Process::Persistent, "echo", |_| {});
    let mut t = p.turn(false).await.unwrap();
    t.worker.stdin.take();
    // 空切片不会像非空数据留在 BufReader 里；探测函数自己观察到第一次 EOF 即可。
    until_observed(async {
        while t.worker.reusable().unwrap() {
            tokio::task::yield_now().await;
        }
    })
    .await;
}

/// 持有 Turn 的任务可以仍在准备本地数据；停止池不能靠该任务主动归还进程。
#[tokio::test]
async fn shutdown_reaps_a_checked_out_worker_before_returning() {
    let temp = tempfile::tempdir().unwrap();
    let obs = temp.path().join("events");
    let p = pool(Process::Persistent, "echo", |e| {
        e.pool_max = 1;
        e.env
            .insert("ECHO_OBSERVE".into(), obs.display().to_string());
    });
    let held = p.turn(false).await.unwrap();
    assert_eq!(p.ids.lock().unwrap().len(), 1);
    let pid = wait_observed(&obs, "hello", 1).await[0]["pid"]
        .as_u64()
        .unwrap() as u32;
    assert_eq!(p.ids.lock().unwrap().get(&0), Some(&Some(pid)));
    p.shutdown().await;
    assert!(
        p.ids.lock().unwrap().is_empty(),
        "shutdown returned before the child exited"
    );
    #[cfg(any(windows, target_os = "linux"))]
    assert_eq!(crate::process::process_exited(pid), Some(true));
    assert!(matches!(p.turn(false).await, Err(Fault::Died)));
    drop(held);
}

/// 调整系统时钟不会缩短冷却；观测用的 Unix 时间可与真正的耗时完全不同。
#[tokio::test]
async fn breaker_generation_cooldown_uses_elapsed_time() {
    let p = pool(Process::Persistent, "echo", |e| {
        e.breaker_cooldown_secs = 90
    });
    {
        let mut c = p.circuit.lock().unwrap();
        c.last = Some((Fault::Died, 0));
        p.open(&mut c, 0);
    }
    assert!(matches!(
        p.admit(),
        Err(Fault::Open {
            probe_at: 90000,
            ..
        })
    ));
}
/// 熔断后的在途故障更新快照，却不能改写旧请求被拒时的故障和时间，包括恢复以后。
#[tokio::test]
async fn breaker_generation_retains_the_fault_after_recovery() {
    let p = pool(Process::Persistent, "echo", |e| {
        e.breaker_failures = 1;
        e.breaker_cooldown_secs = 0
    });
    let old = p.turn(false).await.unwrap();
    let a = p.admit().unwrap();
    let b = p.admit().unwrap();
    p.complete(&a, Some(&Fault::Protocol));
    let at = p.snapshot().1["breaker"]["probe_at"].as_u64().unwrap();
    p.complete(&b, Some(&Fault::Timeout));
    let probe = p.admit().unwrap();
    let refused = |r: Fault| {
        assert!(
            matches!(r,Fault::Open{ref kind,probe_at} if kind=="protocol" && probe_at==at),
            "{r:?}"
        )
    };
    refused(p.admit().err().unwrap());
    assert_eq!(
        p.snapshot().1["breaker"]["last_failure"]["details"]["kind"],
        "timeout"
    );
    p.complete(&probe, None);
    refused(old.send(request_frame("not written"), 1).await.unwrap_err());
    assert_eq!(p.snapshot().1["breaker"]["state"], "closed");
}

/// 已经获得 OS 句柄并非获准写 hello；在两步之间打开就不能再写出一行。
#[tokio::test]
async fn revoked_ticket_prevents_the_first_runtime_hello_byte() {
    let dir = tempfile::tempdir().unwrap();
    let frames = dir.path().join("frames");
    let p = pool(Process::Persistent, "echo", |e| {
        e.breaker_failures = 1;
        e.env
            .insert("ECHO_RECORD".into(), frames.display().to_string());
    });
    let mut w = p.spawn(None).await.unwrap();
    let a = p.admit().unwrap();
    p.complete(&a, Some(&Fault::Protocol));
    assert!(matches!(
        p.handshake(&mut w, 0, Some(&a)).await,
        Err(Fault::Open { .. })
    ));
    w.finish().await;
    assert_eq!(std::fs::read_to_string(frames).unwrap().lines().count(), 1);
}

/// 真正写出探针后分别取消、超时和收到坏序号；只有后两者另起冷却，三者均不遗留半开资格。
#[tokio::test]
async fn half_open_probe_cancellation_timeout_and_invalid_reply_have_distinct_outcomes() {
    for (mode, cancel) in [
        ("gate-request", true),
        ("gate-request", false),
        ("bad-seq", false),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let obs = dir.path().join("observations");
        let p = pool(Process::Persistent, mode, |e| {
            e.timeout_secs = 3600;
            e.breaker_cooldown_secs = 86400;
            e.args.push(dir.path().join("gate").display().to_string());
            e.env
                .insert("ECHO_OBSERVE".into(), obs.display().to_string());
        });
        p.open_at_startup(Fault::Spawn);
        let opened = p.snapshot().1["breaker"]["probe_at"].clone();
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(86401)).await;
        tokio::time::resume();
        let test_pool = p.clone();
        let send = tokio::spawn(async move { call(&test_pool, "blocked", false).await });
        wait_observed(&obs, "request", 1).await;
        if cancel {
            send.abort();
            assert!(send.await.unwrap_err().is_cancelled());
        } else {
            if mode != "bad-seq" {
                tokio::time::pause();
                tokio::time::advance(Duration::from_secs(3601)).await;
            }
            let f = send.await.unwrap().unwrap_err();
            if mode != "bad-seq" {
                tokio::time::resume();
            }
            assert_eq!(
                f.kind(),
                if mode == "bad-seq" {
                    "protocol"
                } else {
                    "timeout"
                }
            );
        }
        let b = &p.snapshot().1["breaker"];
        assert_eq!(b["state"], "open");
        assert_eq!(b["failures"], if cancel { 1 } else { 2 });
        if cancel {
            assert_eq!(b["probe_at"], opened);
        } else {
            assert_eq!(
                b["probe_at"].as_u64().unwrap() - b["opened_at"].as_u64().unwrap(),
                86_400_000
            );
            assert!(matches!(p.admit(), Err(Fault::Open { .. })));
            tokio::time::pause();
            tokio::time::advance(Duration::from_secs(86401)).await;
            tokio::time::resume();
        }
        assert!(p.admit().unwrap().probe);
        assert_eq!(wait_observed(&obs, "hello", 1).await.len(), 1);
        assert_eq!(wait_observed(&obs, "request", 1).await.len(), 1);
    }
}

/// 正在 poll 的已写往返不会被 abort 同步打断；响应先关闸时，即便随后执行超时分支也不能记账。
#[tokio::test]
async fn cancelled_written_heartbeat_cannot_charge_a_later_deadline() {
    let dir = tempfile::tempdir().unwrap();
    let obs = dir.path().join("observed");
    let p = pool(Process::Persistent, "no-stage-reply", |e| {
        e.env
            .insert("ECHO_OBSERVE".into(), obs.display().to_string());
    });
    let turn = p.turn(false).await.unwrap();
    let deadline = Instant::now() + Duration::from_secs(3600);
    let cancelled = Mutex::new(false);
    let send = turn.send_until(request_frame("held"), 0, Some((deadline, &cancelled)));
    tokio::pin!(send);
    tokio::select! { result = &mut send => panic!("early result: {result:?}"), _ = wait_observed(&obs,"request",1) => {} }
    *cancelled.lock().unwrap() = true;
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(3601)).await;
    assert!(matches!(send.await, Err(Fault::Timeout)));
    assert_eq!(p.snapshot().1["breaker"]["failures"], 0);
}

/// 已经写出的心跳在关闸后收到合法回信：不能让下一阶段借走这条已放弃的会话。
#[tokio::test]
async fn response_ready_cancellation_discards_even_a_valid_heartbeat_reply() {
    let dir = tempfile::tempdir().unwrap();
    let obs = dir.path().join("seen");
    let gate = dir.path().join("release");
    let p = pool(Process::Persistent, "gate-request", |e| {
        e.pool_max = 1;
        e.args.push(gate.display().to_string());
        e.env
            .insert("ECHO_OBSERVE".into(), obs.display().to_string());
    });
    let turn = p.turn(false).await.unwrap();
    let cancelled = Mutex::new(false);
    let send = turn.send_until(
        request_frame("blocked"),
        0,
        Some((Instant::now() + Duration::from_secs(3600), &cancelled)),
    );
    tokio::pin!(send);
    let sent = tokio::select! {r=&mut send=>panic!("reply before release: {r:?}"),s=wait_observed(&obs,"request",1)=>s};
    *cancelled.lock().unwrap() = true;
    std::fs::write(gate, b"ok").unwrap();
    send.await.unwrap();
    assert_eq!(p.snapshot().1["breaker"]["failures"], 0);
    let next = worker(call(&p, "next", false).await.unwrap(), "next");
    assert_ne!(next.split_once('/').unwrap().1, sent[0]["pid"].to_string());
}

/// 在首字节之前已经错过截止点：不用这张票，也不处罚这次握手得到的健康会话。
#[tokio::test]
async fn expired_unwritten_heartbeat_does_not_charge_the_breaker() {
    let dir = tempfile::tempdir().unwrap();
    let obs = dir.path().join("frames");
    let p = pool(Process::Persistent, "echo", |e| {
        e.env
            .insert("ECHO_RECORD".into(), obs.display().to_string());
    });
    let turn = p.turn(false).await.unwrap();
    assert!(matches!(
        turn.send_until(
            request_frame("expired"),
            0,
            Some((Instant::now(), &Mutex::new(false)))
        )
        .await,
        Err(Fault::Timeout)
    ));
    assert_eq!(p.snapshot().1["breaker"]["failures"], 0);
    call(&p, "timely", false).await.unwrap();
    let log = std::fs::read_to_string(obs).unwrap();
    assert_eq!(log.lines().count(), 2);
    assert!(!log.contains("expired"));
}

/// spawn 进程在 EOF 后的额外 stdout 仍属于当前往返；握手或业务都不能将它当作合法单帧。
#[tokio::test]
async fn spawn_rejects_extra_stdout_at_hello_and_after_stage() {
    for startup in [true, false] {
        let p = pool(
            Process::Spawn,
            if startup {
                "stray-on-eof"
            } else {
                "stage-stray-on-eof"
            },
            |_| {},
        );
        let result = if startup {
            p.probe().await
        } else {
            p.probe().await.unwrap();
            call(&p, "ok", false).await.map(drop)
        };
        assert!(matches!(result, Err(Fault::Protocol)), "{result:?}");
    }
}

/// 用真实管道上的 open/close 校验独占期间和归还之后的进程身份。
fn stream_frame(id: &str, open: bool) -> Stage {
    let event = if open {
        wire::StreamEvent::Open(wire::StreamOpen::new(
            wire::Context::new(id.into()),
            wire::RequestMeta::new("GET".into(), "http://localhost/".into()),
            wire::Response::new(200),
        ))
    } else {
        wire::StreamEvent::Close
    };
    Stage::Stream(wire::StreamMessage::new(format!("{id}/1"), event))
}
/// 普通池只有一个名额时，长流不妨碍另一条普通 request 得到不同的进程。
#[tokio::test]
async fn stream_workers_do_not_count_toward_pool_max() {
    let p = pool(Process::Persistent, "echo", |e| e.pool_max = 1);
    p.probe().await.unwrap();
    let (mut stream, (hello, _)) = p
        .open_stream(stream_frame("a", true), false, None)
        .await
        .unwrap();
    let other = until_observed(call(&p, "other", false)).await.unwrap();
    assert_ne!(other.state, hello.state);
    let end = stream.event(stream_frame("a", false)).await.unwrap().0;
    assert_eq!(end.state, hello.state);
    stream.finish();
    p.shutdown().await;
}
/// 两条尚未关闭的流各有一个不冲突的编号；第二个可以超出 pool_max 的编号范围。
#[tokio::test]
async fn stream_worker_ids_stay_unique_beyond_pool_max() {
    let p = pool(Process::Persistent, "echo", |e| e.pool_max = 1);
    let (mut a, (first, _)) = p
        .open_stream(stream_frame("a", true), false, None)
        .await
        .unwrap();
    let (mut b, (second, _)) = until_observed(p.open_stream(stream_frame("b", true), false, None))
        .await
        .unwrap();
    let id = |r: &Reply| {
        r.state
            .as_ref()
            .unwrap()
            .split('/')
            .next()
            .unwrap()
            .parse::<u32>()
            .unwrap()
    };
    assert_eq!((id(&first), id(&second)), (0, 1));
    assert_eq!(
        b.event(stream_frame("b", false)).await.unwrap().0.state,
        second.state
    );
    b.finish();
    assert_eq!(
        a.event(stream_frame("a", false)).await.unwrap().0.state,
        first.state
    );
    a.finish();
    p.shutdown().await;
}
/// 先关掉编号较大的流，再关小编号；池按会话个数而不是按编号保留先归还者。
#[tokio::test]
async fn stream_worker_returns_to_the_pool_or_exits_after_close() {
    let p = pool(Process::Persistent, "echo", |e| e.pool_max = 1);
    let (mut a, _) = p
        .open_stream(stream_frame("a", true), false, None)
        .await
        .unwrap();
    let (mut b, (second, _)) = p
        .open_stream(stream_frame("b", true), false, None)
        .await
        .unwrap();
    b.event(stream_frame("b", false)).await.unwrap();
    b.finish();
    a.event(stream_frame("a", false)).await.unwrap();
    a.finish();
    assert_eq!(call(&p, "reuse", false).await.unwrap().state, second.state);
    tokio::time::timeout(Duration::from_secs(4), async {
        while p.ids.lock().unwrap().len() != 1 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(p.ids.lock().unwrap().contains_key(&1));
    p.shutdown().await;
}

/// 模拟握手所有者收到终止信号却尚不能确认退出；不能用这种未确认状态挡住启动注册表清理。
#[tokio::test(start_paused = true)]
async fn startup_retirement_returns_with_an_unconfirmed_owner() {
    let p = pool(Process::Persistent, "echo", |_| {});
    let mut unconfirmed = p.stopping.subscribe();
    let mut state = crate::proxy::AppState::new(crate::config::Config {
        disk_cache: Some(false),
        ..Default::default()
    });
    state.formats = vec![p].into();
    let wait = tokio::spawn(async move { state.retire_formats().await });
    unconfirmed.wait_for(|stop| *stop).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), wait)
        .await
        .expect("one shared second for exit confirmation")
        .unwrap();
}

/// 在真进程读到 open 后才取消；暂停代理的时钟，测试驱动的调度延迟不吃掉两秒协议宽限。
#[tokio::test]
async fn cancelled_written_open_drains_then_aborts_on_the_same_worker() {
    let dir = tempfile::tempdir().unwrap();
    let frames = dir.path().join("frames");
    let gate = dir.path().join("release");
    let p = pool(Process::Persistent, "gate-open", |e| {
        e.stages = vec![crate::config::file::Stage::Stream];
        e.args.push(gate.display().to_string());
        e.env
            .insert("ECHO_RECORD".into(), frames.display().to_string());
    });
    let open = wire::StreamOpen::new(
        wire::Context::new("r".into()),
        wire::RequestMeta::new("GET".into(), "http://localhost/".into()),
        wire::Response::new(200),
    );
    let mut operation = Box::pin(p.open_stream(
        Stage::Stream(wire::StreamMessage::new(
            "r/1".into(),
            wire::StreamEvent::Open(open),
        )),
        false,
        None,
    ));
    tokio::select! {_=&mut operation=>panic!("completed before the fixture released open"),_=wait_observed(&frames,"open",1)=>{}}
    tokio::time::pause();
    drop(operation);
    std::fs::write(gate, b"reply").unwrap();
    // 外部进程使用真实文件确认；blocking 任务在等待时禁止运行时自动推进虚拟时钟。
    let abort = tokio::task::spawn_blocking(move || {
        let end = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(v) = std::fs::read_to_string(&frames)
                .unwrap_or_default()
                .lines()
                .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
                .find(|m| m["event"] == "abort")
            {
                break v;
            }
            assert!(std::time::Instant::now() < end, "no abort after open");
            std::thread::sleep(Duration::from_millis(5));
        }
    })
    .await
    .unwrap();
    assert_eq!(abort["seq"], 2);
    assert_eq!(abort["stream_id"], "r/1");
    assert_eq!(abort["reason"]["code"], "client_gone");
    assert_eq!(p.snapshot().1["breaker"]["failures"], 0);
}
