//! 管道测试只使用本地示例进程；进程号检验生命期，worker_id 检验同时占用的槽位。
use super::super::{format_echo_path, headers_from_btreemap, headers_to_btreemap};
use super::*;
use aproxy_envelope as wire;
use std::collections::BTreeMap;
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
    let mut req = wire::Request::new("POST".into(), "http://localhost/test".into());
    req.body = wire::Body::Text(body.into());
    let frame = Stage::Request(wire::RequestMessage::new(
        wire::Context::new("r".into()),
        wire::Delivery::Retry,
        req,
        wire::Client::new("/test".into(), false),
    ));
    p.call(frame, p.entry.timeout_secs, fresh).await
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

    let headers = headers_from_btreemap(&map);
    assert!(headers.contains_key("content-type"));
    assert!(headers.contains_key("x-custom"));
    assert!(!headers.contains_key("content-length"), "CL 防御性剔除");
    assert_eq!(headers.len(), 2, "非法头名丢弃");
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
/// spawn 始终只有一个 0 号槽。
#[tokio::test]
async fn spawn_mode_pool_max_is_one() {
    let p = pool(Process::Spawn, "echo", |_| {});
    assert_eq!(p.entry.pool_max, 4);
    assert_eq!(p.turns.available_permits(), 1);
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
    let p = pool(Process::Persistent, "sleep", |e| {
        e.args.push("3000".into());
        e.timeout_secs = 1
    });
    assert!(matches!(call(&p, "s", false).await, Err(Fault::Timeout)));
    assert!(p.idle.lock().unwrap().is_empty());
}
/// 两个重叠的往返使用不同的槽位。
#[tokio::test]
async fn concurrent_requests_within_pool_max() {
    let p = pool(Process::Persistent, "sleep", |e| {
        e.pool_max = 2;
        e.args.push("150".into());
    });
    let (a, b) = tokio::join!(call(&p, "a", false), call(&p, "b", false));
    let ids = [worker(a.unwrap(), "a"), worker(b.unwrap(), "b")];
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
    tokio::time::sleep(Duration::from_millis(1500)).await;
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
    tokio::time::sleep(Duration::from_millis(800)).await;
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
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let text = std::fs::read_to_string(path).unwrap_or_default();
            let events: Vec<_> = text
                .lines()
                .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
                .filter(|v| v["event"] == event)
                .collect();
            if events.len() >= count {
                return events;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
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
            e.timeout_secs = 1;
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
            assert!(matches!(first.await.unwrap(), Err(Fault::Timeout)));
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
    let (a, b) = tokio::time::timeout(Duration::from_secs(4), async {
        tokio::join!(p.probe(), p.probe())
    })
    .await
    .unwrap();
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
    let p = pool(Process::Persistent, "slow-eof", |e| {
        e.pool_max = 2;
        e.worker_idle_secs = 86400;
        e.env
            .insert("ECHO_OBSERVE".into(), obs.display().to_string());
    });
    let (a, b) = tokio::join!(call(&p, "a", false), call(&p, "b", false));
    let a = worker(a.unwrap(), "a");
    let b = worker(b.unwrap(), "b");
    assert_ne!(a, b);
    tokio::time::timeout(Duration::from_millis(3400), p.shutdown())
        .await
        .unwrap();
    assert_eq!(wait_observed(&obs, "eof", 2).await.len(), 2);
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

/// 命令搜索忽略不可执行的影子，home/bin 胜过显式 PATH；不存在的 PATH 不补当前目录。
#[test]
fn command_lookup_uses_runnable_candidates_and_the_supplied_path() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let bin = home.join("bin");
    let other = home.join("path");
    std::fs::create_dir(&bin).unwrap();
    std::fs::create_dir(&other).unwrap();
    #[cfg(windows)]
    {
        std::fs::write(bin.join("tool"), b"shadow").unwrap();
        std::fs::write(other.join("tool.exe"), b"fixture").unwrap();
        assert_eq!(
            program_path("tool", home, Some(other.as_os_str())).unwrap(),
            other.join("tool.exe")
        );
        assert!(program_path("tool", home, None).is_err());
        std::fs::write(bin.join("tool.exe"), b"fixture").unwrap();
        assert_eq!(
            program_path("tool", home, Some(other.as_os_str())).unwrap(),
            bin.join("tool.exe")
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let earlier = home.join("first");
        std::fs::create_dir(&earlier).unwrap();
        for d in [&bin, &earlier, &other] {
            std::fs::write(d.join("tool"), b"fixture").unwrap();
        }
        std::fs::set_permissions(other.join("tool"), std::fs::Permissions::from_mode(0o700))
            .unwrap();
        let path = std::env::join_paths([earlier, other.clone()]).unwrap();
        assert_eq!(
            program_path("tool", home, Some(&path)).unwrap(),
            other.join("tool")
        );
        assert!(program_path("tool", home, None).is_err());
    }
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
        wire::Client::new("/".into(), false),
    ));
    p.exchange(&mut turn.worker, frame, 2, &turn.ticket)
        .await
        .unwrap();
    let pid = wait_observed(&obs, "hello", 1).await[0]["pid"]
        .as_u64()
        .unwrap();
    std::fs::write(dir.path().join(format!("release-{pid}")), b"").unwrap();
    assert!(
        !tokio::time::timeout(Duration::from_secs(2), turn.worker.stdout.fill_buf())
            .await
            .unwrap()
            .unwrap()
            .is_empty()
    );
    assert!(matches!(turn.worker.reusable(), Err(Fault::Protocol)));
    let p = pool(Process::Persistent, "echo", |_| {});
    let mut t = p.turn(false).await.unwrap();
    t.worker.stdin.take();
    // 空切片不会像非空数据留在 BufReader 里；探测函数自己观察到第一次 EOF 即可。
    tokio::time::timeout(Duration::from_secs(2), async {
        while t.worker.reusable().unwrap() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
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
    #[cfg(any(windows, target_os = "linux"))]
    let pid = wait_observed(&obs, "hello", 1).await[0]["pid"]
        .as_u64()
        .unwrap() as u32;
    tokio::time::timeout(Duration::from_secs(5), p.shutdown())
        .await
        .unwrap();
    assert!(
        p.ids.lock().unwrap().is_empty(),
        "shutdown returned before the child exited"
    );
    #[cfg(any(windows, target_os = "linux"))]
    assert_eq!(crate::process::process_exited(pid), Some(true));
    assert!(matches!(p.turn(false).await, Err(Fault::Died)));
    drop(held);
}
