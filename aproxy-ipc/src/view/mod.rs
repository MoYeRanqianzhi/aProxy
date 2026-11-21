//! A read-only, run-scoped view of one explicit home. One Tokio task owns discovery
//! and all subscription futures; readers share its cache, never a socket per screen.
//! Enumeration supplies membership and fallback snapshots. A slot refusal means
//! polling, not unresponsiveness. Broken subscriptions are retried on the next scan.
//!
//! Subscribe *before* paging: the server has already registered the receiver when
//! it captures the opening in-flight set. Descending exclusive cursors cover older
//! records while the stream covers concurrent starts and finishes. Fold both by
//! sequence within a verified run; terminal records cannot be reopened by replay.
//! Publish the joined history atomically, retaining the old cache until resync
//! succeeds. A lag marker invalidates the cut: close it and take a fresh one.
//!
//! Body bytes and detail documents bypass the cache. Dropping their read future
//! closes its connection. Dropping the view aborts its sole task; `shutdown` also
//! waits until that task has released every stream and pending read.

mod dto;
use crate::{
    client::{self, IpcError, RunIdentity},
    enumerate::{self, Liveness},
    wire::{MAX_BODY_READ, MAX_LIST, Part, ReadOp},
};
pub use dto::*;
use futures_util::{
    FutureExt, StreamExt,
    future::{self, AbortHandle, Abortable, BoxFuture, Either},
    stream::{self, BoxStream, FuturesUnordered},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::broadcast;

/// A recent window of up to 16 MiB of 64-KiB wire records per run (before deserialization).
pub const PER_RUN_LIMIT: usize = 256;
/// Four full windows across the home. Active requests take precedence over either limit.
pub const HOME_LIMIT: usize = 1024;

/// Enumeration result, kept separate from subscription health.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Presence {
    /// A verified instance answered.
    Live,
    /// The recorded process exists but did not answer.
    Unresponsive,
    /// The endpoint could not be trusted or understood.
    Incompatible,
    /// Restoration is pending without a running process.
    Down,
    /// An unrecognized liveness value.
    Unknown,
}

/// State source, independent of whether a live instance has a subscription slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Feed {
    /// No verified live endpoint.
    Offline,
    /// Establishing the first consistent history cut.
    Joining,
    /// History and state are being updated by a subscription.
    Streaming,
    /// All subscription slots are busy; state comes from periodic enumeration.
    Polling,
    /// The connection broke; keep the last history while waiting to rejoin.
    Disconnected,
}

/// Bounded invalidations, not deltas: after receiver lag simply reread the cache.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "target", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Change {
    /// A port appeared, disappeared, changed run or changed state. Reread instances.
    Instance(u16),
    /// The named run's records changed or were evicted. Reread requests.
    Requests(RunKey),
}

/// Caller-facing failure categories. Transport diagnostics may contain paths or peer text and are not DTOs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ReadFailure {
    /// The target is no longer a live run in this home.
    RunChanged,
    /// The instance no longer retains this request or body.
    Gone,
    /// The endpoint or its response could not be verified or decoded.
    Unavailable,
}
impl From<IpcError> for ReadFailure {
    fn from(e: IpcError) -> Self {
        match e {
            IpcError::RunChanged => Self::RunChanged,
            IpcError::Remote { code, .. } if code == crate::wire::error_code::NOT_FOUND => {
                Self::Gone
            }
            _ => Self::Unavailable,
        }
    }
}

type Result<T> = std::result::Result<T, IpcError>;
type Events = BoxStream<'static, Result<Value>>;
type Records = BTreeMap<u64, Arc<RequestSummary>>;

trait Source: Send + Sync + 'static {
    fn scan(&self) -> BoxFuture<'_, Vec<InstanceSummary>>;
    fn subscribe<'a>(
        &'a self,
        port: u16,
        run: &'a RunIdentity,
    ) -> BoxFuture<'a, Result<(Value, Events)>>;
    fn read<'a>(
        &'a self,
        port: u16,
        run: &'a RunIdentity,
        op: ReadOp,
    ) -> BoxFuture<'a, Result<Value>>;
}
struct Local {
    home: PathBuf,
    dir: PathBuf,
}
impl Source for Local {
    fn scan(&self) -> BoxFuture<'_, Vec<InstanceSummary>> {
        async {
            enumerate::enumerate_in(&self.home)
                .await
                .instances
                .into_iter()
                .map(|i| {
                    let liveness = match i.liveness {
                        Liveness::Live => Presence::Live,
                        Liveness::Unresponsive => Presence::Unresponsive,
                        Liveness::Down => Presence::Down,
                        Liveness::Incompatible => Presence::Incompatible,
                        Liveness::Unknown => Presence::Unknown,
                    };
                    InstanceSummary {
                        port: i.port,
                        run: i.instance.as_ref().map(|r| RunKey {
                            port: i.port,
                            instance_id: r.instance_id.clone(),
                        }),
                        pid: i.instance.as_ref().map(|r| r.pid),
                        started_at: i.instance.as_ref().map(|r| r.started_at),
                        version: i.instance.map(|r| r.version),
                        liveness,
                        restore_pending: i.restore_pending,
                        restore_reason: i.restore_reason,
                        history_unavailable: i
                            .status
                            .as_ref()
                            .filter(|s| {
                                !s.features
                                    .iter()
                                    .any(|f| f == crate::wire::FEATURE_REDACTED_CLIENT_FIELDS)
                            })
                            .map(|_| HistoryUnavailable::SourceRedactionMissing),
                        feed: if liveness == Presence::Live {
                            Feed::Joining
                        } else {
                            Feed::Offline
                        },
                        state: i.status.map(|s| StateSnapshot {
                            state: match s.state {
                                crate::wire::InstanceState::Starting => "starting",
                                crate::wire::InstanceState::Serving => "serving",
                                crate::wire::InstanceState::SwapPrepared => "swap_prepared",
                                crate::wire::InstanceState::Stopping => "stopping",
                                crate::wire::InstanceState::Unknown => "unknown",
                            }
                            .into(),
                            activity: s.activity.into(),
                        }),
                    }
                })
                .collect()
        }
        .boxed()
    }
    fn subscribe<'a>(
        &'a self,
        port: u16,
        run: &'a RunIdentity,
    ) -> BoxFuture<'a, Result<(Value, Events)>> {
        async move {
            let (ack, stream) =
                client::subscribe_for_run_in(&self.dir, &port.to_string(), Some(run)).await?;
            Ok((
                ack,
                stream::try_unfold(stream, |mut s| async move {
                    Ok(s.next().await?.map(|event| (event, s)))
                })
                .boxed(),
            ))
        }
        .boxed()
    }
    fn read<'a>(
        &'a self,
        port: u16,
        run: &'a RunIdentity,
        op: ReadOp,
    ) -> BoxFuture<'a, Result<Value>> {
        async move {
            Ok(
                client::read_for_run_in(&self.dir, &port.to_string(), &op, Some(run))
                    .await?
                    .1,
            )
        }
        .boxed()
    }
}
impl From<crate::wire::Activity> for RunActivity {
    fn from(a: crate::wire::Activity) -> Self {
        Self {
            last_request_at: a.last_request_at,
            requests_total: a.requests_total,
            retries_total: a.retries_total,
            rescued_total: a.rescued_total,
            upgraded_total: a.upgraded_total,
            in_flight: a.in_flight,
            retrying: a.retrying,
            by_code: a.by_code,
            last_error: a.last_error.map(|e| LastFailure {
                code: e.code,
                details: e.details,
                message: e.message,
                at: e.at,
            }),
            buckets: Buckets {
                width_secs: a.buckets.width_secs,
                requests: a.buckets.requests,
                retries: a.buckets.retries,
                failures: a.buckets.failures,
            },
        }
    }
}

/// Pure fold for opening records, pages and event batches. Terminal data wins over
/// an earlier queued event. A page uses exactly the same key space as the stream.
fn merge(
    mut cached: Records,
    page: impl IntoIterator<Item = Arc<RequestSummary>>,
    events: impl IntoIterator<Item = Arc<RequestSummary>>,
) -> Records {
    for r in page.into_iter().chain(events) {
        if !cached
            .get(&r.request_seq)
            .is_some_and(|old| old.ended_at.is_some() && r.ended_at.is_none())
        {
            cached.insert(r.request_seq, r);
        }
    }
    cached
}
fn oldest(records: &Records) -> Option<(u64, u64)> {
    records
        .values()
        .filter_map(|r| Some((r.ended_at?, r.request_seq)))
        .min()
}
fn trim(records: &mut Records, limit: usize) {
    while records.len() > limit {
        let Some((_, seq)) = oldest(records) else {
            break;
        };
        records.remove(&seq);
    }
}
fn invalid() -> IpcError {
    IpcError::Protocol("invalid view data".into())
}
fn decode<T: DeserializeOwned>(value: Value) -> Result<T> {
    serde_json::from_value(value).map_err(|_| invalid())
}
fn check_record(run: &RunIdentity, r: &RequestSummary) -> Result<()> {
    if r.request_id != format!("{}-{}", run.instance_id, r.request_seq) {
        Err(IpcError::RunChanged)
    } else {
        Ok(())
    }
}
#[derive(Deserialize)]
struct Opening {
    snapshot: StateSnapshot,
    requests: Vec<RequestSummary>,
}
#[derive(Deserialize)]
struct Page {
    more: bool,
    requests: Vec<RequestSummary>,
}
fn records(run: &RunIdentity, rows: Vec<RequestSummary>) -> Result<Vec<Arc<RequestSummary>>> {
    rows.into_iter()
        .map(|r| {
            check_record(run, &r)?;
            Ok(Arc::new(r))
        })
        .collect()
}

struct Entry {
    info: InstanceSummary,
    records: Records,
}
#[derive(Default)]
struct Store {
    ports: BTreeMap<u16, Entry>,
}
struct Shared {
    data: Mutex<Store>,
    changes: broadcast::Sender<Change>,
}
impl Shared {
    fn notify(&self, c: Change) {
        let _ = self.changes.send(c);
    }
    // All mutation happens on the one driver. Its per-run futures cannot publish after
    // discovery drops them. Reader threads see either side of a complete transaction.
    fn commit(&self, port: u16, rows: Records, full: bool, state: Option<StateSnapshot>) {
        let mut store = self.data.lock().unwrap();
        let entry = store.ports.get_mut(&port).unwrap();
        let key = entry.info.run.clone().unwrap();
        let mut old = std::mem::take(&mut entry.records);
        if full {
            old.retain(|n, r| r.ended_at.is_some() || rows.contains_key(n));
        }
        entry.records = merge(old, rows.into_values(), []);
        trim(&mut entry.records, PER_RUN_LIMIT);
        if let Some(s) = state {
            entry.info.feed = Feed::Streaming;
            entry.info.state = Some(s);
            self.notify(Change::Instance(port));
        }
        let mut dirty = BTreeSet::from([port]);
        let mut total: usize = store.ports.values().map(|e| e.records.len()).sum();
        while total > HOME_LIMIT {
            let Some((_, seq, p)) = store
                .ports
                .iter()
                .filter_map(|(p, e)| oldest(&e.records).map(|(t, n)| (t, n, *p)))
                .min()
            else {
                break;
            };
            store.ports.get_mut(&p).unwrap().records.remove(&seq);
            dirty.insert(p);
            total -= 1;
        }
        for p in dirty {
            self.notify(Change::Requests(if p == port {
                key.clone()
            } else {
                store.ports[&p].info.run.clone().unwrap()
            }));
        }
    }
}

async fn follow(source: &dyn Source, shared: &Shared, port: u16, run: &RunIdentity) -> Result<()> {
    let (ack, mut events) = source.subscribe(port, run).await?;
    let Opening { snapshot, requests } = decode(ack)?;
    let mut cut = merge(Records::new(), records(run, requests)?, []);
    let mut cursor = None;
    loop {
        let page = source
            .read(
                port,
                run,
                ReadOp::Requests {
                    before: cursor.as_ref().map(|(_, id): &(u64, String)| id.clone()),
                    limit: Some(MAX_LIST as u64),
                },
            )
            .await?;
        let Page { more, requests } = decode(page)?;
        let rows = records(run, requests)?;
        let mut below = cursor.as_ref().map(|(n, _)| *n);
        for r in &rows {
            if below.is_some_and(|n| n <= r.request_seq) {
                return Err(invalid());
            }
            below = Some(r.request_seq);
        }
        cursor = rows.last().map(|r| (r.request_seq, r.request_id.clone()));
        cut = merge(cut, rows, []);
        trim(&mut cut, PER_RUN_LIMIT);
        if !more {
            break;
        }
        if cursor.is_none() {
            return Err(invalid());
        }
    }
    shared.commit(port, cut, true, Some(snapshot));
    while let Some(event) = events.next().await {
        let mut event = event?;
        match event["event"].as_str() {
            Some("request") => {
                let r: RequestSummary = decode(event["data"].take())?;
                check_record(run, &r)?;
                shared.commit(port, [(r.request_seq, Arc::new(r))].into(), false, None);
            }
            Some("state") => {
                let state = decode(event["data"].take())?;
                shared
                    .data
                    .lock()
                    .unwrap()
                    .ports
                    .get_mut(&port)
                    .unwrap()
                    .info
                    .state = Some(state);
                shared.notify(Change::Instance(port));
            }
            Some("lagged" | "closing") => break,
            Some(_) => {}
            None => return Err(invalid()),
        }
    }
    Ok(())
}

/// The sole owner of background IO for a home. Create on a Tokio runtime with IO and time enabled.
pub struct HomeView {
    source: Arc<dyn Source>,
    shared: Arc<Shared>,
    driver: tokio::task::JoinHandle<()>,
}
impl HomeView {
    /// Start discovery immediately, then rescan with the supplied nonzero interval.
    pub fn open(home: PathBuf, interval: Duration) -> Self {
        let mut timer = tokio::time::interval(interval);
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        Self::start(
            Arc::new(Local {
                dir: home.join("run"),
                home,
            }),
            stream::unfold(timer, |mut timer| async move {
                timer.tick().await;
                Some(((), timer))
            })
            .boxed(),
        )
    }
    fn start(source: Arc<dyn Source>, ticks: BoxStream<'static, ()>) -> Self {
        let shared = Arc::new(Shared {
            data: Mutex::default(),
            changes: broadcast::channel(128).0,
        });
        let driver = tokio::spawn(drive(source.clone(), shared.clone(), ticks));
        Self {
            source,
            shared,
            driver,
        }
    }
    /// Subscribe before taking an initial snapshot. Lagged receivers can reread without reconnecting.
    pub fn changes(&self) -> broadcast::Receiver<Change> {
        self.shared.changes.subscribe()
    }
    /// Port-ordered current membership, including unresponsive and restore-pending entries.
    pub fn instances(&self) -> Vec<InstanceSummary> {
        self.shared
            .data
            .lock()
            .unwrap()
            .ports
            .values()
            .map(|e| e.info.clone())
            .collect()
    }
    /// Recent summaries newest first; a retired run never aliases its successor.
    pub fn requests(&self, key: &RunKey) -> Vec<RequestSummary> {
        self.shared
            .data
            .lock()
            .unwrap()
            .ports
            .get(&key.port)
            .filter(|e| e.info.run.as_ref() == Some(key))
            .into_iter()
            .flat_map(|e| e.records.values().rev().map(|r| (**r).clone()))
            .collect()
    }
    fn identity(&self, key: &RunKey) -> std::result::Result<RunIdentity, ReadFailure> {
        self.shared
            .data
            .lock()
            .unwrap()
            .ports
            .get(&key.port)
            .filter(|e| e.info.run.as_ref() == Some(key) && e.info.liveness == Presence::Live)
            .ok_or(ReadFailure::RunChanged)
            .and_then(|e| {
                if e.info.history_unavailable.is_some() {
                    return Err(ReadFailure::Unavailable);
                }
                Ok(RunIdentity {
                    pid: e.info.pid.ok_or(ReadFailure::RunChanged)?,
                    instance_id: key.instance_id.clone(),
                })
            })
    }
    /// Fetch metadata without retaining it or reading any body.
    pub async fn detail(
        &self,
        key: &RunKey,
        id: &str,
    ) -> std::result::Result<RequestDetail, ReadFailure> {
        let run = self.identity(key)?;
        let detail: RequestDetail = decode(
            self.source
                .read(
                    key.port,
                    &run,
                    ReadOp::Request {
                        request_id: id.into(),
                    },
                )
                .await?,
        )?;
        check_record(&run, &detail.record)?;
        if detail.record.request_id != id {
            return Err(ReadFailure::Unavailable);
        }
        Ok(detail)
    }
    /// Fetch at most one protocol-sized chunk. Drop this future to cancel, including during peer verification.
    pub async fn body_chunk(
        &self,
        key: &RunKey,
        id: &str,
        part: Part,
        attempt: Option<u32>,
        offset: u64,
        limit: u64,
    ) -> std::result::Result<BodyChunk, ReadFailure> {
        let run = self.identity(key)?;
        Ok(decode(
            self.source
                .read(
                    key.port,
                    &run,
                    ReadOp::Body {
                        request_id: id.into(),
                        part,
                        attempt,
                        offset: Some(offset),
                        limit: Some(limit.min(MAX_BODY_READ)),
                    },
                )
                .await?,
        )?)
    }
    /// Stop and wait for all IO futures to be dropped before returning.
    pub async fn shutdown(mut self) {
        self.driver.abort();
        let _ = (&mut self.driver).await;
    }
}
impl Drop for HomeView {
    fn drop(&mut self) {
        self.driver.abort();
    }
}

async fn drive(source: Arc<dyn Source>, shared: Arc<Shared>, ticks: BoxStream<'static, ()>) {
    // The scan stream retains an in-progress enumeration across worker completions.
    let mut scans = ticks.then(|()| source.scan()).boxed();
    let mut workers = FuturesUnordered::<BoxFuture<'static, (u16, u64, Feed)>>::new();
    let mut handles = BTreeMap::<u16, (u64, AbortHandle)>::new();
    let mut serial = 0;
    loop {
        let scan = if workers.is_empty() {
            scans.next().await
        } else {
            match future::select(scans.next(), workers.next()).await {
                Either::Left((s, _)) => s,
                Either::Right((Some((port, n, feed)), _)) => {
                    if handles.get(&port).is_some_and(|(id, _)| *id == n) {
                        handles.remove(&port);
                        shared
                            .data
                            .lock()
                            .unwrap()
                            .ports
                            .get_mut(&port)
                            .unwrap()
                            .info
                            .feed = feed;
                        shared.notify(Change::Instance(port));
                    }
                    continue;
                }
                Either::Right((None, _)) => unreachable!(),
            }
        };
        let Some(scan) = scan else { break };
        let mut store = shared.data.lock().unwrap();
        let mut previous = std::mem::take(&mut store.ports);
        for mut info in scan {
            let p = info.port;
            let old = previous.remove(&p);
            let same = old
                .as_ref()
                .is_some_and(|e| e.info.run == info.run && e.info.pid == info.pid);
            let safe = info.history_unavailable.is_none();
            if !safe {
                info.feed = Feed::Polling;
                if let Some(state) = &mut info.state {
                    state.activity.last_error = None;
                }
            }
            if (!same || !safe || info.liveness != Presence::Live)
                && let Some((_, h)) = handles.remove(&p)
            {
                h.abort();
            }
            if let Some(e) = &old {
                if same
                    && safe
                    && e.info.history_unavailable.is_none()
                    && info.liveness == Presence::Live
                    && e.info.liveness == Presence::Live
                {
                    info.feed = e.info.feed;
                    // A scan started before the last state event; don't regress a live stream.
                    if info.feed == Feed::Streaming {
                        info.state = e.info.state.clone();
                    }
                } else if !same && let Some(key) = &e.info.run {
                    shared.notify(Change::Requests(key.clone()));
                }
            }
            if safe
                && !handles.contains_key(&p)
                && info.liveness == Presence::Live
                && let (Some(key), Some(pid)) = (&info.run, info.pid)
            {
                let run = RunIdentity {
                    pid,
                    instance_id: key.instance_id.clone(),
                };
                let s = source.clone();
                let cache = shared.clone();
                let (handle, registration) = AbortHandle::new_pair();
                serial += 1;
                let n = serial;
                handles.insert(p, (n, handle));
                workers.push(
                    async move {
                        let result =
                            Abortable::new(follow(&*s, &cache, p, &run), registration).await;
                        let feed = match result {
                            Ok(Err(IpcError::Remote { code, .. })) if code == "busy" => {
                                Feed::Polling
                            }
                            _ => Feed::Disconnected,
                        };
                        (p, n, feed)
                    }
                    .boxed(),
                );
            }
            if old.as_ref().is_none_or(|e| e.info != info) {
                shared.notify(Change::Instance(p));
            }
            store.ports.insert(
                p,
                Entry {
                    info,
                    records: old
                        .filter(|_| same && safe)
                        .map(|e| e.records)
                        .unwrap_or_default(),
                },
            );
        }
        for (p, e) in previous {
            if let Some((_, h)) = handles.remove(&p) {
                h.abort();
            }
            shared.notify(Change::Instance(p));
            if let Some(key) = e.info.run {
                shared.notify(Change::Requests(key));
            }
        }
    }
}

#[cfg(test)]
mod tests;
