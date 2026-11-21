use super::*;
use serde_json::json;
use tokio::sync::{mpsc, oneshot};

type Tx<T> = mpsc::UnboundedSender<T>;
fn channel<T: Send + 'static>() -> (Tx<T>, BoxStream<'static, T>) {
    let (send, recv) = mpsc::unbounded_channel();
    (
        send,
        stream::unfold(recv, |mut r| async move { r.recv().await.map(|v| (v, r)) }).boxed(),
    )
}
fn state(n: u64) -> Value {
    json!({"state":"serving","activity":{"last_request_at":0,"requests_total":n,"retries_total":0,"buckets":{"width_secs":120,"requests":[],"retries":[],"failures":[]}}})
}
fn instance(port: u16, id: &str, n: u64) -> InstanceSummary {
    InstanceSummary {
        port,
        run: Some(RunKey {
            port,
            instance_id: id.into(),
        }),
        pid: Some(42),
        started_at: Some(1),
        version: None,
        liveness: Presence::Live,
        restore_pending: false,
        restore_reason: None,
        reads: ReadCapabilities {
            history_feed: true,
            detail: true,
            body: true,
        },
        history_unavailable: None,
        feed: Feed::Joining,
        state: Some(decode(state(n)).unwrap()),
    }
}
fn row(id: &str, n: u64, done: bool) -> Value {
    json!({"request_id":format!("{id}-{n}"),"request_seq":n,"started_at":n,"ended_at":done.then_some(n),"method":"GET","path":"/","stream":false,"delivery":"retry","upgraded":false,"outcome":if done{"ok"}else{"in_flight"},"phase":if done{"done"}else{"sending"},"attempts":1,"tries_omitted":0,"tries":[],"keepalive":false,"heartbeats":0,"request_bytes":0,"response_bytes":0})
}
fn page(rows: Vec<Value>, more: bool) -> Value {
    json!({"requests":rows,"more":more})
}

enum Call {
    Open(RunIdentity, oneshot::Sender<Result<(Value, Events)>>),
    Read(RunIdentity, ReadOp, oneshot::Sender<Result<Value>>),
}
struct Fake {
    home: Mutex<Vec<InstanceSummary>>,
    fail_scan: std::sync::atomic::AtomicBool,
    calls: Tx<Call>,
}
impl Source for Fake {
    fn scan(&self) -> BoxFuture<'_, Option<Vec<InstanceSummary>>> {
        future::ready(
            (!self
                .fail_scan
                .swap(false, std::sync::atomic::Ordering::SeqCst))
            .then(|| self.home.lock().unwrap().clone()),
        )
        .boxed()
    }
    fn subscribe<'a>(
        &'a self,
        _: u16,
        run: &'a RunIdentity,
    ) -> BoxFuture<'a, Result<(Value, Events)>> {
        async move {
            let (tx, rx) = oneshot::channel();
            assert!(self.calls.send(Call::Open(run.clone(), tx)).is_ok());
            rx.await.unwrap()
        }
        .boxed()
    }
    fn read<'a>(
        &'a self,
        _: u16,
        run: &'a RunIdentity,
        op: ReadOp,
    ) -> BoxFuture<'a, Result<Value>> {
        async move {
            let (tx, rx) = oneshot::channel();
            assert!(self.calls.send(Call::Read(run.clone(), op, tx)).is_ok());
            rx.await.unwrap()
        }
        .boxed()
    }
}
struct Script {
    source: Arc<Fake>,
    ticks: Tx<()>,
    calls: BoxStream<'static, Call>,
}
impl Script {
    fn start() -> (HomeView, Self) {
        let (ticks, t) = channel();
        let (c, calls) = channel();
        let source = Arc::new(Fake {
            home: Mutex::new(vec![instance(80, "a", 0)]),
            calls: c,
            fail_scan: Default::default(),
        });
        (
            HomeView::start(source.clone(), t),
            Self {
                source,
                ticks,
                calls,
            },
        )
    }
    fn tick(&self) {
        self.ticks.send(()).unwrap();
    }
    async fn open(&mut self, id: &str) -> oneshot::Sender<Result<(Value, Events)>> {
        let Some(Call::Open(run, tx)) = self.calls.next().await else {
            panic!("expected subscribe")
        };
        assert_eq!(run.instance_id, id);
        tx
    }
    async fn ack(&mut self, id: &str, rows: Vec<Value>) -> Tx<Result<Value>> {
        let tx = self.open(id).await;
        let (send, stream) = channel();
        assert!(
            tx.send(Ok((json!({"snapshot":state(2),"requests":rows}), stream)))
                .is_ok()
        );
        send
    }
    async fn page(&mut self, id: &str, before: Option<&str>) -> oneshot::Sender<Result<Value>> {
        let Some(Call::Read(run, ReadOp::Requests { before: b, .. }, tx)) = self.calls.next().await
        else {
            panic!("expected page")
        };
        assert_eq!(run.instance_id, id);
        assert_eq!(b.as_deref(), before);
        tx
    }
}
async fn until(view: &HomeView, test: impl Fn(&HomeView) -> bool) {
    let mut changes = view.changes();
    while !test(view) {
        changes.recv().await.unwrap();
    }
}
fn rt(f: impl Future<Output = ()>) {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap()
        .block_on(async {
            tokio::time::timeout(Duration::from_secs(20), f)
                .await
                .expect("script deadlock")
        })
}
fn feed(v: &HomeView) -> Feed {
    v.instances()[0].feed
}

#[test]
fn join_reconcile_and_restart_are_atomic() {
    rt(async {
        let (v, mut script) = Script::start();
        let key = instance(80, "a", 0).run.unwrap();
        let _two_readers = (v.changes(), v.changes());
        script.tick();
        let stream = script.ack("a", vec![row("a", 2, false)]).await;
        let finish = row("a", 2, true);
        let start = row("a", 3, false);
        script
            .page("a", None)
            .await
            .send(Ok(page(vec![finish.clone()], true)))
            .unwrap();
        // The page already knows the finish. Both its earlier opening event and a
        // start that missed the first page are waiting on this same subscription.
        stream
            .send(Ok(json!({"event":"request","data":row("a",2,false)})))
            .unwrap();
        stream
            .send(Ok(json!({"event":"request","data":start})))
            .unwrap();
        script
            .page("a", Some("a-2"))
            .await
            .send(Ok(page(vec![row("a", 1, true)], false)))
            .unwrap();
        until(&v, |v| v.requests(&key).len() == 3).await;
        let rows = v.requests(&key);
        assert_eq!(
            rows.iter().map(|r| r.request_seq).collect::<Vec<_>>(),
            [3, 2, 1]
        );
        assert!(rows[1].ended_at.is_some());
        stream.send(Ok(json!({"event":"lagged"}))).unwrap();
        until(&v, |v| feed(v) == Feed::Disconnected).await;
        stream.closed().await;
        script.tick();
        let stream = script.ack("a", vec![]).await;
        let p = script.page("a", None).await;
        assert_eq!(v.requests(&key), rows);
        p.send(Ok(page(vec![row("a", 3, true), finish], false)))
            .unwrap();
        until(&v, |v| feed(v) == Feed::Streaming).await;
        assert_eq!(v.requests(&key).len(), 3);
        assert!(v.requests(&key)[0].ended_at.is_some());
        drop(stream);
        until(&v, |v| feed(v) == Feed::Disconnected).await;
        script.tick();
        let old = script.ack("a", vec![]).await;
        let inflight = script.page("a", None).await;
        *script.source.home.lock().unwrap() = vec![instance(80, "b", 1)];
        script.tick();
        let fresh = script.ack("b", vec![]).await;
        old.closed().await;
        assert!(
            inflight
                .send(Ok(page(vec![row("a", 9, true)], false)))
                .is_err()
        );
        assert!(v.requests(&key).is_empty());
        script
            .page("b", None)
            .await
            .send(Ok(page(vec![row("b", 1, false)], false)))
            .unwrap();
        until(&v, |v| feed(v) == Feed::Streaming).await;
        assert_eq!(
            v.requests(&v.instances()[0].run.clone().unwrap())[0].request_id,
            "b-1"
        );
        script.source.home.lock().unwrap().clear();
        script.tick();
        until(&v, |v| v.instances().is_empty()).await;
        fresh.closed().await;
        v.shutdown().await;
    })
}
#[test]
fn busy_polls_verification_fails_closed_and_reads_cancel() {
    rt(async {
        let (v, mut s) = Script::start();
        s.tick();
        assert!(
            s.open("a")
                .await
                .send(Err(IpcError::Remote {
                    code: "busy".into(),
                    message: "".into(),
                    details: Value::Null
                }))
                .is_ok()
        );
        until(&v, |v| feed(v) == Feed::Polling).await;
        s.source.home.lock().unwrap()[0] = instance(80, "a", 17);
        s.tick();
        let open = s.open("a").await;
        assert_eq!(
            v.instances()[0]
                .state
                .as_ref()
                .unwrap()
                .activity
                .requests_total,
            17
        );
        assert!(open.send(Err(IpcError::RunChanged)).is_ok());
        until(&v, |v| feed(v) == Feed::Disconnected).await;
        s.tick();
        let events = s.ack("a", vec![]).await;
        s.page("a", None)
            .await
            .send(Ok(page(vec![], false)))
            .unwrap();
        until(&v, |v| feed(v) == Feed::Streaming).await;
        events.send(Err(IpcError::LineTooLong)).unwrap();
        until(&v, |v| feed(v) == Feed::Disconnected).await;
        events.closed().await;
        s.tick();
        let events = s.ack("a", vec![]).await;
        s.page("a", None)
            .await
            .send(Ok(page(vec![row("a", 1, true)], false)))
            .unwrap();
        until(&v, |v| feed(v) == Feed::Streaming).await;
        let key = v.instances()[0].run.take().unwrap();
        let mut read = Box::pin(v.body_chunk(&key, "a-1", Part::Request, None, 1, 7));
        assert!(future::poll_immediate(&mut read).await.is_none());
        let Some(Call::Read(run, ReadOp::Body { offset, limit, .. }, reply)) = s.calls.next().await
        else {
            panic!("unexpected prefetch")
        };
        assert_eq!(
            (run.instance_id.as_str(), offset, limit),
            ("a", Some(1), Some(7))
        );
        drop(read);
        assert!(reply.send(Ok(Value::Null)).is_err());
        drop(v);
        events.closed().await;
    })
}
#[test]
fn merging_terminal_replays_is_idempotent() {
    // Exhaust every placement of a finish among the cache, page and queued events.
    for mask in 0..8 {
        let r = |i: u8| {
            Arc::new(decode::<RequestSummary>(row("a", 1, mask & (1u8 << i) != 0)).unwrap())
        };
        let rows = merge([(1, r(0))].into(), [r(1)], [r(2)]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[&1].ended_at.is_some(), mask != 0);
    }
}
#[test]
fn both_eviction_limits_keep_in_flight_records() {
    let shared = Shared {
        join_slot: tokio::sync::Semaphore::new(1),
        data: Mutex::default(),
        changes: broadcast::channel(1).0,
    };
    for p in 1..=5 {
        shared.data.lock().unwrap().ports.insert(
            p,
            Entry {
                info: instance(p, "a", 0),
                records: Records::new(),
            },
        );
        let rows = (0..=PER_RUN_LIMIT as u64)
            .map(|n| {
                let mut r: RequestSummary = decode(row("a", n, n != 0)).unwrap();
                r.ended_at = r.ended_at.map(|t| t + u64::from(p) * 1000);
                (n, Arc::new(r))
            })
            .collect();
        shared.commit(p, rows, true, None);
    }
    let d = shared.data.lock().unwrap();
    assert_eq!(
        d.ports.values().map(|e| e.records.len()).sum::<usize>(),
        HOME_LIMIT
    );
    assert!(
        d.ports
            .values()
            .all(|e| e.records.contains_key(&0) && e.records.len() <= PER_RUN_LIMIT)
    );
    assert_eq!(d.ports[&1].records.len(), 1);
    assert!(!d.ports[&5].records.contains_key(&1));
    let mut active: Records = (0..4)
        .map(|n| (n, Arc::new(decode(row("a", n, false)).unwrap())))
        .collect();
    trim(&mut active, 2);
    assert_eq!(active.len(), 4);
}

#[test]
fn old_live_run_exposes_state_but_not_history_or_diagnostics() {
    rt(async {
        let (v, s) = Script::start();
        {
            let mut home = s.source.home.lock().unwrap();
            home[0].history_unavailable = Some(HistoryUnavailable::SourceRedactionMissing);
            home[0].state.as_mut().unwrap().activity.last_error =
                Some(decode(json!({"message":"private", "at":5})).unwrap());
        }
        s.tick();
        until(&v, |v| !v.instances().is_empty()).await;
        let i = v.instances().remove(0);
        let k = i.run.unwrap();
        assert_eq!(i.feed, Feed::Polling);
        assert!(i.state.unwrap().activity.last_error.is_none());
        assert!(v.requests(&k).is_empty());
        assert_eq!(v.detail(&k, "a-1").await, Err(ReadFailure::Unavailable));
        assert_eq!(
            v.body_chunk(&k, "a-1", Part::Request, None, 0, 4).await,
            Err(ReadFailure::Unavailable)
        );
        v.shutdown().await;
    })
}

#[test]
fn active_and_terminal_revisions_are_independent_of_arrival_order() {
    for terminal in [false, true] {
        let observation = |revision| {
            let mut value = row("a", 1, terminal);
            value["revision"] = json!(revision);
            value["attempts"] = json!(revision);
            Arc::new(decode::<RequestSummary>(value).unwrap())
        };
        for revisions in [[2, 1, 3], [3, 1, 2], [1, 3, 2]] {
            let [a, b, c] = revisions.map(observation);
            assert_eq!(merge([(1, a)].into(), [b], [c])[&1].attempts, 3);
        }
    }
}

#[test]
fn loss_during_paging_preserves_previous_cut() {
    rt(async {
        let (v, mut s) = Script::start();
        s.tick();
        let events = s.ack("a", vec![]).await;
        s.page("a", None)
            .await
            .send(Ok(page(vec![row("a", 1, true)], false)))
            .unwrap();
        until(&v, |v| feed(v) == Feed::Streaming).await;
        let key = v.instances().remove(0).run.unwrap();
        let good = v.requests(&key);
        drop(events);
        until(&v, |v| feed(v) == Feed::Disconnected).await;
        for lag in [true, false] {
            s.tick();
            let events = s.ack("a", vec![]).await;
            s.page("a", None)
                .await
                .send(Ok(page(vec![row("a", 3, true)], true)))
                .unwrap();
            let last = s.page("a", Some("a-3")).await;
            if lag {
                events.send(Ok(json!({"event":"lagged"}))).unwrap();
            }
            drop(events);
            let mut changes = v.changes();
            last.send(Ok(page(vec![], false))).unwrap();
            assert!(matches!(
                changes.recv().await.unwrap(),
                Change::Instance(80)
            ));
            assert_eq!(feed(&v), Feed::Disconnected);
            assert_eq!(v.requests(&key), good);
        }
        v.shutdown().await;
    })
}

#[test]
fn unadvertised_reads_remain_polling_without_opening_connections() {
    rt(async {
        let (v, mut s) = Script::start();
        s.source.home.lock().unwrap()[0].reads = ReadCapabilities::default();
        s.tick();
        until(&v, |v| !v.instances().is_empty()).await;
        let k = v.instances()[0].run.clone().unwrap();
        assert_eq!(feed(&v), Feed::Polling);
        assert_eq!(v.detail(&k, "a-1").await, Err(ReadFailure::Unavailable));
        assert_eq!(
            v.body_chunk(&k, "a-1", Part::Request, None, 0, 1).await,
            Err(ReadFailure::Unavailable)
        );
        s.source.home.lock().unwrap()[0].reads.history_feed = true;
        s.tick();
        let _newly_supported = s.open("a").await;
        assert!(s.calls.next().now_or_never().is_none());
        v.shutdown().await;
    })
}

#[test]
fn joins_wait_before_opening_but_completed_streams_share_the_home() {
    rt(async {
        let (v, mut s) = Script::start();
        *s.source.home.lock().unwrap() = vec![instance(80, "a", 0), instance(81, "b", 0)];
        s.tick();
        let a = s.ack("a", vec![]).await;
        let first = s.page("a", None).await;
        assert!(s.calls.next().now_or_never().is_none());
        first.send(Ok(page(vec![], false))).unwrap();
        let b = s.ack("b", vec![]).await;
        assert!(!a.is_closed());
        v.shutdown().await;
        assert!(b.is_closed());
    })
}

#[test]
fn inconclusive_scan_does_not_retire_a_streaming_run() {
    rt(async {
        let (v, mut s) = Script::start();
        s.tick();
        let events = s.ack("a", vec![]).await;
        s.page("a", None)
            .await
            .send(Ok(page(vec![row("a", 1, false)], false)))
            .unwrap();
        until(&v, |v| feed(v) == Feed::Streaming).await;
        s.source
            .fail_scan
            .store(true, std::sync::atomic::Ordering::SeqCst);
        s.tick();
        while s.source.fail_scan.load(std::sync::atomic::Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
        let k = v.instances().remove(0).run.unwrap();
        assert_eq!(v.requests(&k).len(), 1);
        assert!(!events.is_closed());
    })
}

#[test]
fn unknown_lifecycle_means_the_same_on_every_snapshot_path() {
    let mut state = state(1);
    state["state"] = json!("draining");
    let expected = lifecycle(decode(state["state"].clone()).unwrap());
    assert_eq!(expected, "unknown");
    assert_eq!(decode::<StateSnapshot>(state).unwrap().state, expected);
}

#[test]
fn discovery_interval_has_a_floor_even_for_zero() {
    rt(async {
        for (input, effective) in [(0, 2), (1, 2), (2, 2), (7, 7)] {
            assert_eq!(
                discovery_timer(Duration::from_secs(input)).period(),
                Duration::from_secs(effective)
            );
        }
    })
}
