//! 请求历史：最近的请求、它们的每一次上游尝试，以及请求体与响应体。只在内存里，重启即失。
//!
//! 三样东西：
//! - **记录**（[`Record`]）：一个请求一条，代理热路径经 [`Recorder`] 往里写。最多留
//!   [`MAX_RECORDS`] 条，先进先出，在途的不淘汰。每条记录有界：尝试只留最前
//!   [`TRIES_HEAD`] 次与最近 [`TRIES_TAIL`] 次，序列化后不超过 [`MAX_RECORD_BYTES`]。
//! - **体**（[`Body`]）：客户端的请求体、每次尝试发出的体、每次尝试收到的上游响应体，整份
//!   保留，只受一个总量上限约束（`history.body_budget_mb`）。超出时从最旧的请求开始丢体，
//!   记录本身留着。带 content-encoding 的体先解码再入库；入库时把 aproxy 自己知道的密钥
//!   换成 `<secret>`，所以内存里的历史本身不含这些明文。
//! - **事件**（[`Event`]）：记录变化时发给订阅者。一个请求的开始与结束立即发，中间的变化
//!   合并成每秒最多一个；没有订阅者时什么都不构造。
//!
//! 锁的次序固定为「记录表 → 单条记录 → 单个体」。热路径上一个请求只在开始时碰一次记录表，
//! 其余的写都只锁自己那条记录或自己那个体，请求之间互不争抢。

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use aproxy_ipc::redact::{self, Secrets};
use bytes::Bytes;
use serde_json::{Map, Value, json};

use crate::daemon::now_millis;

/// 最多保留多少条记录。在途的请求不占这个名额之外的位置，但也不会被淘汰
pub const MAX_RECORDS: usize = 1000;
/// 一条记录里保留最前面的几次尝试
pub const TRIES_HEAD: usize = 3;
/// 一条记录里保留最近的几次尝试
pub const TRIES_TAIL: usize = 20;
/// 一条记录序列化后的字节数上限
pub const MAX_RECORD_BYTES: usize = 64 * 1024;
/// 记录超出上限时，`path`、`user_agent`、`model` 各截到多少个字符
pub const TRUNCATED_TEXT_CHARS: usize = 256;
/// 读体时一块最多多少字节（原始字节，base64 之前）
pub const MAX_BODY_READ: u64 = 1024 * 1024;
/// 列记录时一次最多多少条
pub const MAX_LIST: usize = 200;
/// 列记录时不给 `limit` 就是多少条
pub const DEFAULT_LIST: usize = 50;

/// 同一个请求的两个 `request` 事件之间至少隔多久（开始与结束不受它限制）
const REQUEST_EVENT_INTERVAL_MS: u64 = 1000;
/// 合并后的事件多久补发一轮
const FLUSH_EVERY: Duration = Duration::from_millis(250);
/// 每个订阅者最多积压多少个事件；跟不上的订阅者丢掉最旧的，并被告知丢了几个
const EVENT_QUEUE: usize = 1024;

/// 取锁；持锁的线程 panic 过也照常用（这里的数据每一步都是自洽的）。
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// 请求此刻在做什么（记录的 `phase`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// 正在读客户端的请求体
    ReadingBody,
    /// 正在等 format 程序改写请求或响应
    Transforming,
    /// 一次上游尝试正在进行
    Attempting,
    /// 上一次尝试失败了，正在等下一次
    Backoff,
    /// 正把缓冲好的上游响应回放给客户端
    Replaying,
    /// 正把上游响应实时转给客户端（forward）
    Forwarding,
    /// 结束了
    Done,
}

impl Phase {
    pub fn as_str(self) -> &'static str {
        match self {
            Phase::ReadingBody => "reading_body",
            Phase::Transforming => "transforming",
            Phase::Attempting => "attempting",
            Phase::Backoff => "backoff",
            Phase::Replaying => "replaying",
            Phase::Forwarding => "forwarding",
            Phase::Done => "done",
        }
    }
}

/// 请求的结局（记录的 `outcome`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// 还没结束
    InFlight,
    /// 上游的成功响应交给了客户端
    Ok,
    /// 放弃之后透传了上游的非成功响应；不重试的请求的上游失败或流中途断开也记为它
    UpstreamError,
    /// aproxy 自己结束了这个请求（错误响应或终态事件）
    AproxyError,
    /// 客户端在拿到完整响应之前断开了
    ClientGone,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::InFlight => "in_flight",
            Outcome::Ok => "ok",
            Outcome::UpstreamError => "upstream_error",
            Outcome::AproxyError => "aproxy_error",
            Outcome::ClientGone => "client_gone",
        }
    }
}

/// 一个体为什么没有保留（体对象的 `cause`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cause {
    /// 预算是 0：不保留任何体
    BudgetDisabled,
    /// 这一个体就比整个预算大
    ExceedsBudget,
    /// 按 content-encoding 解码失败
    DecodeFailed,
}

impl Cause {
    pub fn as_str(self) -> &'static str {
        match self {
            Cause::BudgetDisabled => "budget_disabled",
            Cause::ExceedsBudget => "exceeds_budget",
            Cause::DecodeFailed => "decode_failed",
        }
    }
}

/// 一个体的保留状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BodyState {
    Retained,
    /// 留过，为了给更新的体腾地方被丢掉了
    Evicted,
    NotRetained(Cause),
}

/// 全部体共用的预算。
#[derive(Debug)]
struct Budget {
    /// 上限（字节）；0 = 不保留体
    limit: u64,
    /// 此刻全部体占着的字节数，含正在接收的
    used: AtomicU64,
}

/// 历史里的一个体：请求体、某次尝试发出的体，或某次尝试收到的响应体。
#[derive(Debug)]
pub struct Body {
    budget: Arc<Budget>,
    inner: Mutex<BodyInner>,
}

#[derive(Debug)]
struct BodyInner {
    /// 入库的字节，按到达的块存；块与转发路径共用同一份内存，不另外拷贝
    chunks: Vec<Bytes>,
    /// 读得到的字节数：解码、替换之后的。带编码的体在解码完成之前是 0
    bytes: u64,
    /// 记在预算上的字节数。带编码的体在解码完成之前，这里是攒着的原始字节
    held: u64,
    state: BodyState,
    /// 不会再有字节进来了
    complete: bool,
    /// 线上原来的 content-encoding；没有编码时为 None
    encoding: Option<Arc<str>>,
}

impl Body {
    fn new(budget: &Arc<Budget>, encoding: Option<&str>) -> Arc<Body> {
        let disabled = budget.limit == 0;
        Arc::new(Body {
            budget: budget.clone(),
            inner: Mutex::new(BodyInner {
                chunks: Vec::new(),
                bytes: 0,
                held: 0,
                state: if disabled {
                    BodyState::NotRetained(Cause::BudgetDisabled)
                } else {
                    BodyState::Retained
                },
                complete: disabled,
                encoding: encoding.map(Arc::from),
            }),
        })
    }

    /// 丢掉入库的字节、把占着的预算还回去，状态改成 `state`。
    fn release(&self, inner: &mut BodyInner, state: BodyState) {
        self.budget.used.fetch_sub(inner.held, Ordering::Relaxed);
        inner.held = 0;
        inner.chunks = Vec::new();
        inner.state = state;
    }

    /// 为了腾地方丢掉这个体。已经不在的不动。
    fn evict(&self) {
        let mut inner = lock(&self.inner);
        if inner.state == BodyState::Retained {
            self.release(&mut inner, BodyState::Evicted);
        }
    }

    /// 体对象：`{"bytes","state","cause"?,"encoding"?}`。`same` 为真时状态写成
    /// `same_as_previous`（这次发出的体与上一次是同一份）。
    fn view(&self, same: bool) -> Value {
        let inner = lock(&self.inner);
        let mut view = Map::new();
        let (state, cause) = match inner.state {
            BodyState::Retained if same => ("same_as_previous", None),
            BodyState::Retained => ("retained", None),
            BodyState::Evicted => ("evicted", None),
            BodyState::NotRetained(cause) => ("not_retained", Some(cause)),
        };
        let bytes = match inner.state {
            BodyState::NotRetained(_) => 0,
            _ => inner.bytes,
        };
        view.insert("bytes".into(), bytes.into());
        view.insert("state".into(), state.into());
        if let Some(cause) = cause {
            view.insert("cause".into(), cause.as_str().into());
        }
        if let Some(encoding) = &inner.encoding {
            view.insert("encoding".into(), (&**encoding).into());
        }
        Value::Object(view)
    }

    /// 读一块：从 `offset` 起最多 `limit` 字节。返回这一块的字节、此刻入库的总字节数，
    /// 以及体是否已经收完。
    fn read(&self, offset: u64, limit: u64) -> Result<(Vec<u8>, u64, bool), ReadError> {
        let inner = lock(&self.inner);
        match inner.state {
            BodyState::Retained => {}
            BodyState::Evicted => return Err(ReadError::gone("evicted", None)),
            BodyState::NotRetained(cause) => {
                return Err(ReadError::gone("not_retained", Some(cause)));
            }
        }
        let total = inner.bytes;
        if offset > total {
            return Err(ReadError::BadOffset { total });
        }
        let end = total.min(offset.saturating_add(limit));
        let mut out = Vec::with_capacity((end - offset) as usize);
        let mut at = 0u64;
        for chunk in &inner.chunks {
            let next = at + chunk.len() as u64;
            if next > offset && at < end {
                let from = offset.saturating_sub(at) as usize;
                let to = (end.min(next) - at) as usize;
                out.extend_from_slice(&chunk[from..to]);
            }
            at = next;
            if at >= end {
                break;
            }
        }
        Ok((out, total, inner.complete))
    }
}

impl Drop for Body {
    fn drop(&mut self) {
        let held = self
            .inner
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .held;
        self.budget.used.fetch_sub(held, Ordering::Relaxed);
    }
}

/// 读体失败的原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadError {
    /// 要的东西不在：`reason` 取 `unknown_request`、`no_such_part`、`evicted`、`not_retained`；
    /// `not_retained` 时 `cause` 说明为什么没留
    NotFound {
        reason: &'static str,
        cause: Option<&'static str>,
    },
    /// `offset` 超过了此刻入库的总字节数
    BadOffset { total: u64 },
}

impl ReadError {
    fn gone(reason: &'static str, cause: Option<Cause>) -> Self {
        ReadError::NotFound {
            reason,
            cause: cause.map(Cause::as_str),
        }
    }
}

/// 要读的是哪个体。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// 客户端发来的请求体
    Request,
    /// 某次尝试发往上游的体
    Sent,
    /// 某次尝试从上游收到的响应体
    Response,
}

/// 往一个体里写：转发路径每经过一块就交给它一份。
///
/// 写者不持有记录的锁，也不阻塞转发：一块进来只做一次子串查找（找已知密钥）、一次引用
/// 计数加一、一次短暂的加锁。结束（[`BodyWriter::finish`] 或被丢弃）时把留着的尾巴入库；
/// 带 content-encoding 的体这时才解码。
pub struct BodyWriter {
    history: Arc<History>,
    body: Arc<Body>,
    /// 留着没入库的尾巴：比最长的已知密钥短 1 字节，等下一块到了一起看，被块边界切开的
    /// 密钥才换得掉。它里面从来没有一个完整的密钥
    carry: Vec<u8>,
    /// 带 content-encoding：先攒原始字节，结束时解码、替换，再入库
    encoded: bool,
    /// 这个体已经不保留了（被淘汰、超出预算）：之后进来的块直接丢掉
    dead: bool,
    finished: bool,
}

impl BodyWriter {
    /// 交一块。
    pub fn feed(&mut self, chunk: &Bytes) {
        if self.dead || chunk.is_empty() {
            return;
        }
        if self.encoded {
            self.store([Some(chunk.clone()), None], false);
            return;
        }
        let pieces = self.redact(chunk);
        self.store(pieces, true);
    }

    /// 这个体收完了。
    pub fn finish(mut self) {
        self.end();
    }

    /// 这个体此刻是否还在保留。不保留了的体，调用方不必再为它准备数据（例如读文件）。
    pub fn is_live(&self) -> bool {
        !self.dead
    }

    /// 把 `chunk` 里的已知密钥换掉，返回这一步可以入库的片段（最多两片），并留下新的尾巴。
    ///
    /// 绝大多数块里没有密钥：那时入库的就是原块的一个切片，不拷贝；只有接缝上的那几十个
    /// 字节（上一块的尾巴）单独成一小片。
    fn redact(&mut self, chunk: &Bytes) -> [Option<Bytes>; 2] {
        let secrets = &self.history.secrets;
        if secrets.is_empty() {
            return [Some(chunk.clone()), None];
        }
        let hold = secrets.max_len() - 1;
        // 密钥要么整个在本块里，要么跨着接缝：从尾巴里起头的，最远只够得着本块的前 hold 字节
        let clean = secrets.find(chunk).is_none() && {
            let tail = self.carry.len();
            self.carry
                .extend_from_slice(&chunk[..chunk.len().min(hold)]);
            let seam_clean = tail == 0 || secrets.find(&self.carry).is_none();
            self.carry.truncate(tail);
            seam_clean
        };
        if clean {
            if chunk.len() >= hold {
                let tail =
                    (!self.carry.is_empty()).then(|| Bytes::from(std::mem::take(&mut self.carry)));
                let keep = chunk.len() - hold;
                self.carry.extend_from_slice(&chunk[keep..]);
                return [tail, Some(chunk.slice(..keep))];
            }
            // 比尾巴还短的小块：并进尾巴，超出 hold 的前段入库
            self.carry.extend_from_slice(chunk);
            if self.carry.len() > hold {
                let rest = self.carry.split_off(self.carry.len() - hold);
                let ready = std::mem::replace(&mut self.carry, rest);
                return [Some(Bytes::from(ready)), None];
            }
            return [None, None];
        }
        // 有密钥：尾巴与本块拼起来逐处替换。这条路要拷贝，但只在真的出现密钥时才走
        let mut joined = std::mem::take(&mut self.carry);
        joined.extend_from_slice(chunk);
        let mut out = Vec::with_capacity(joined.len());
        let mut at = 0;
        while let Some((start, len)) = secrets.find(&joined[at..]) {
            out.extend_from_slice(&joined[at..at + start]);
            out.extend_from_slice(redact::SECRET_TEXT.as_bytes());
            at += start + len;
        }
        let rest = &joined[at..];
        let keep = rest.len().saturating_sub(hold);
        out.extend_from_slice(&rest[..keep]);
        self.carry = rest[keep..].to_vec();
        [Some(Bytes::from(out)), None]
    }

    /// 把片段入库并记到预算上。`visible`：这些字节是否算进读得到的字节数（攒着等解码的
    /// 原始字节不算）。
    fn store(&mut self, pieces: [Option<Bytes>; 2], visible: bool) {
        let added: u64 = pieces.iter().flatten().map(|p| p.len() as u64).sum();
        if added == 0 {
            return;
        }
        let budget = &self.body.budget;
        let used = {
            let mut inner = lock(&self.body.inner);
            if inner.state != BodyState::Retained {
                self.dead = true;
                return;
            }
            if inner.held + added > budget.limit {
                // 这一个体就比整个预算大：已经入库的部分也丢掉
                self.body
                    .release(&mut inner, BodyState::NotRetained(Cause::ExceedsBudget));
                inner.bytes = 0;
                self.dead = true;
                return;
            }
            inner
                .chunks
                .extend(pieces.into_iter().flatten().filter(|p| !p.is_empty()));
            inner.held += added;
            if visible {
                inner.bytes += added;
            }
            // 在体的锁里记账：淘汰也要先拿这把锁，两边看到的 held 与 used 始终对得上
            budget.used.fetch_add(added, Ordering::Relaxed) + added
        };
        if used > budget.limit {
            self.history.make_room(&self.body);
        }
    }

    fn end(&mut self) {
        if self.finished {
            return;
        }
        self.finished = true;
        if self.dead {
            lock(&self.body.inner).complete = true;
            return;
        }
        if self.encoded {
            self.decode();
            return;
        }
        let tail = std::mem::take(&mut self.carry);
        self.store([Some(Bytes::from(tail)), None], true);
        lock(&self.body.inner).complete = true;
    }

    /// 把攒着的原始字节解码、替换密钥，再入库。解码放到阻塞线程池里：大的体要算一阵子，
    /// 不能占着运行时的工作线程，更不能让转发等它。
    fn decode(&mut self) {
        let (raw, encoding) = {
            let mut inner = lock(&self.body.inner);
            if inner.state != BodyState::Retained {
                inner.complete = true;
                return;
            }
            // 原始字节拿出来解码；它们占的预算（held）先留着，解码完再换成解码后的大小
            (
                std::mem::take(&mut inner.chunks),
                inner.encoding.clone().unwrap_or_else(|| Arc::from("")),
            )
        };
        let history = self.history.clone();
        let body = self.body.clone();
        let job = move || history.store_decoded(&body, &raw, &encoding);
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => {
                runtime.spawn_blocking(job);
            }
            Err(_) => job(),
        }
    }
}

impl Drop for BodyWriter {
    /// 没走到 `finish` 的写者（上游中途断开、客户端走了）：已经收到的部分照样留下。
    fn drop(&mut self) {
        self.end();
    }
}

/// 把若干块首尾相接当成一个字节流来读（解码器的输入）。
struct ChunkReader<'a> {
    chunks: &'a [Bytes],
    index: usize,
    at: usize,
}

impl std::io::Read for ChunkReader<'_> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        while let Some(chunk) = self.chunks.get(self.index) {
            let rest = &chunk[self.at..];
            if rest.is_empty() {
                self.index += 1;
                self.at = 0;
                continue;
            }
            let n = rest.len().min(out.len());
            out[..n].copy_from_slice(&rest[..n]);
            self.at += n;
            return Ok(n);
        }
        Ok(0)
    }
}

/// 一个头：名字与值。密钥头（名字像鉴权的）的值是占位对象，其余是字符串。
#[derive(Debug, Clone, PartialEq)]
pub struct Header {
    name: String,
    value: Value,
}

impl Header {
    fn view(&self) -> Value {
        json!({ "name": self.name, "value": self.value })
    }
}

fn headers_view(headers: &[Header]) -> Value {
    Value::Array(headers.iter().map(Header::view).collect())
}

/// 一次尝试发往上游的请求行与头（体另存）。发出的请求不变时，各次尝试共用同一份。
#[derive(Debug)]
pub struct SentHead {
    method: String,
    /// 已打码
    url: String,
    headers: Vec<Header>,
}

/// 一次尝试之后的决定（记录里的 `tries[].decision`）。
#[derive(Debug, Clone)]
struct Decision {
    /// 谁决定的：`rule:<序号>` 或 `default`
    by: String,
    /// `retry` 或 `stop`
    decision: &'static str,
    /// 下一次尝试之前实际等了多久；还没开始等时为 None
    delay_ms: Option<u64>,
}

/// 一次尝试收到的上游响应（体另存）。
#[derive(Debug)]
struct TryResponse {
    status: u16,
    headers: Vec<Header>,
    body: Arc<Body>,
}

/// 一次上游尝试。
#[derive(Debug)]
struct Try {
    attempt: u32,
    started_at: u64,
    started: Instant,
    duration_ms: Option<u64>,
    upstream_status: Option<u16>,
    /// 尝试失败码；成功或还没结束时为 None
    code: Option<&'static str>,
    /// 这次尝试从上游收到的响应体字节数（线上的）
    response_bytes: u64,
    decision: Option<Decision>,
    sent: Option<Arc<SentHead>>,
    /// 这次发出的体。与上一次相同时指向同一个体（`sent_same` 为真），只存一份
    sent_body: Option<Arc<Body>>,
    sent_same: bool,
    response: Option<TryResponse>,
    /// 没拿到完整的上游响应时的 `{code, details, message}`
    error: Option<Value>,
}

/// 一条请求记录。
#[derive(Debug)]
pub struct Record {
    seq: u64,
    id: Arc<str>,
    /// 已经结束。淘汰记录时只看它，不必去拿记录的锁
    done: AtomicBool,
    inner: Mutex<RecordInner>,
}

#[derive(Debug)]
struct RecordInner {
    started_at: u64,
    ended_at: Option<u64>,
    method: String,
    /// `路径?查询串`，查询串的值已打码
    path: String,
    model: Option<String>,
    stream: bool,
    user_agent: Option<String>,
    delivery: &'static str,
    outcome: Outcome,
    /// 结局已定、响应还没交付完时先记在这里；交付完（或客户端断开）时成为 `outcome`
    settled: Option<(Outcome, Option<&'static str>)>,
    code: Option<&'static str>,
    client_status: Option<u16>,
    phase: Phase,
    next_attempt_at: Option<u64>,
    attempts: u32,
    tries_omitted: u32,
    tries: VecDeque<Try>,
    keepalive: bool,
    heartbeats: u64,
    request_bytes: u64,
    response_bytes: u64,
    request_headers: Vec<Header>,
    request_body: Option<Arc<Body>>,
    /// 最近一次为它发事件的时刻
    last_event_at: u64,
    /// 有变化还没发事件，已经排进补发的队列
    queued: bool,
}

impl RecordInner {
    fn try_mut(&mut self, attempt: u32) -> Option<&mut Try> {
        self.tries.iter_mut().rev().find(|t| t.attempt == attempt)
    }

    /// 这条记录名下的全部体（同一个体被几次尝试共用时会出现几次）。
    fn bodies(&self) -> impl Iterator<Item = &Arc<Body>> {
        self.request_body.iter().chain(
            self.tries
                .iter()
                .flat_map(|t| t.sent_body.iter().chain(t.response.iter().map(|r| &r.body))),
        )
    }
}

/// 发给订阅者的事件。
#[derive(Debug, Clone)]
pub enum Event {
    /// 一条记录变了：整条记录的 JSON，订阅者按 `request_id` 整条替换
    Request(Arc<str>),
    /// 实例要退出了。`reason` 取 `shutdown` 或 `swap`
    Closing(&'static str),
}

/// 一个请求开始时就知道的东西。
pub struct Start<'a> {
    pub method: &'a str,
    /// 客户端请求的 `路径?查询串`，原样；入库时打码
    pub path_and_query: &'a str,
    pub headers: &'a http::HeaderMap,
    /// 投递方式：`retry` 或 `forward`
    pub delivery: &'static str,
    pub phase: Phase,
}

/// 一次尝试的失败（记录里只借用，不依赖代理层的类型）。
pub struct Failure<'a> {
    pub code: &'static str,
    pub details: &'a Map<String, Value>,
    pub message: &'a str,
    /// 上游给了完整的响应、只是内容算失败（状态码、错误体）。这时不另记 `error`
    pub has_response: bool,
}

/// 实例的请求历史。
pub struct History {
    instance_id: Arc<str>,
    budget: Arc<Budget>,
    secrets: Secrets,
    records: Mutex<BTreeMap<u64, Arc<Record>>>,
    /// 序号比它小的记录都没有还留着的体：腾地方时从它开始找，不必每次从头扫过早已空了的
    /// 记录。给旧记录挂上新的体时把它往回拨
    evict_floor: AtomicU64,
    events: tokio::sync::broadcast::Sender<Event>,
    /// 有变化、还没发事件的记录的序号
    dirty: Mutex<Vec<u64>>,
    /// 补发任务是否在跑
    flushing: AtomicBool,
}

impl std::fmt::Debug for History {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("History")
            .field("instance_id", &self.instance_id)
            .field("budget", &self.budget)
            .finish_non_exhaustive()
    }
}

impl History {
    /// `budget_mb`：全部体加起来最多占多少 MiB，0 = 不保留体。`secrets`：入库时要换掉的
    /// 已知密钥。`instance_id`：请求标识的前半段，用来从标识里认出序号。
    pub fn new(budget_mb: u64, secrets: Secrets, instance_id: Arc<str>) -> Arc<History> {
        let (events, _) = tokio::sync::broadcast::channel(EVENT_QUEUE);
        Arc::new(History {
            instance_id,
            budget: Arc::new(Budget {
                limit: budget_mb.saturating_mul(1024 * 1024),
                used: AtomicU64::new(0),
            }),
            secrets,
            records: Mutex::new(BTreeMap::new()),
            evict_floor: AtomicU64::new(0),
            events,
            dirty: Mutex::new(Vec::new()),
            flushing: AtomicBool::new(false),
        })
    }

    /// 此刻全部体占着的字节数。
    pub fn body_bytes(&self) -> u64 {
        self.budget.used.load(Ordering::Relaxed)
    }

    /// 入库时要换掉的已知密钥。
    pub fn secrets(&self) -> &Secrets {
        &self.secrets
    }

    /// 把一组头变成入库的形状：密钥头的值换成占位对象，其余的值里出现的已知密钥换掉。
    fn headers<'a>(&self, headers: impl Iterator<Item = (&'a str, &'a [u8])>) -> Vec<Header> {
        headers
            .map(|(name, value)| {
                let text = String::from_utf8_lossy(value);
                let value = if redact::is_secret_header(name) {
                    redact::placeholder(&text)
                } else if self.secrets.is_empty() {
                    Value::String(text.into_owned())
                } else {
                    Value::String(self.secrets.replace(&text))
                };
                Header {
                    name: name.to_string(),
                    value,
                }
            })
            .collect()
    }

    /// 一个请求开始：建一条记录，返回往里写的句柄。`seq` 是实例内的请求序号，`id` 是
    /// 请求标识（`<instance_id>-<seq>`）。
    pub fn begin(self: &Arc<Self>, seq: u64, id: Arc<str>, start: Start<'_>) -> Recorder {
        let now = now_millis();
        let user_agent = start
            .headers
            .get(http::header::USER_AGENT)
            .map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned());
        let inner = RecordInner {
            started_at: now,
            ended_at: None,
            method: start.method.to_string(),
            path: redact::url(start.path_and_query),
            model: None,
            stream: false,
            user_agent,
            delivery: start.delivery,
            outcome: Outcome::InFlight,
            settled: None,
            code: None,
            client_status: None,
            phase: start.phase,
            next_attempt_at: None,
            attempts: 0,
            tries_omitted: 0,
            tries: VecDeque::new(),
            keepalive: false,
            heartbeats: 0,
            request_bytes: 0,
            response_bytes: 0,
            request_headers: self.headers(
                start
                    .headers
                    .iter()
                    .map(|(name, value)| (name.as_str(), value.as_bytes())),
            ),
            request_body: None,
            last_event_at: now,
            queued: false,
        };
        let record = Arc::new(Record {
            seq,
            id,
            done: AtomicBool::new(false),
            inner: Mutex::new(inner),
        });
        {
            let mut records = lock(&self.records);
            records.insert(seq, record.clone());
            // 先进先出，只淘汰已经结束的：在途的留着，它们的数量受并发数限制
            while records.len() > MAX_RECORDS {
                let oldest_done = records
                    .iter()
                    .find(|(_, r)| r.done.load(Ordering::Relaxed))
                    .map(|(seq, _)| *seq);
                match oldest_done {
                    Some(seq) => records.remove(&seq),
                    None => break,
                };
            }
        }
        if self.events.receiver_count() > 0 {
            let line = record_json(&record, &lock(&record.inner));
            let _ = self.events.send(Event::Request(line.into()));
        }
        Recorder(Some(Arc::new(Live {
            history: self.clone(),
            record,
        })))
    }

    /// 记录变了。`urgent`（开始、结束）立即发事件；其余的变化，距上一个事件不到一秒就先
    /// 记下，由补发任务合并成一个。没有订阅者时什么都不做——连记录的 JSON 都不构造。
    fn changed(&self, record: &Record, inner: &mut RecordInner, urgent: bool) {
        if self.events.receiver_count() == 0 {
            inner.queued = false;
            return;
        }
        let now = now_millis();
        if urgent || now.saturating_sub(inner.last_event_at) >= REQUEST_EVENT_INTERVAL_MS {
            inner.last_event_at = now;
            inner.queued = false;
            let _ = self
                .events
                .send(Event::Request(record_json(record, inner).into()));
        } else if !inner.queued {
            inner.queued = true;
            lock(&self.dirty).push(record.seq);
        }
    }

    /// 补发一轮：排着队的记录里，距上一个事件已满一秒的发出去，没满的留到下一轮。
    fn flush(&self) {
        let waiting = std::mem::take(&mut *lock(&self.dirty));
        if waiting.is_empty() {
            return;
        }
        let now = now_millis();
        let mut later = Vec::new();
        for seq in waiting {
            let Some(record) = lock(&self.records).get(&seq).cloned() else {
                continue;
            };
            let mut inner = lock(&record.inner);
            if !inner.queued {
                continue;
            }
            if now.saturating_sub(inner.last_event_at) >= REQUEST_EVENT_INTERVAL_MS {
                inner.last_event_at = now;
                inner.queued = false;
                let _ = self
                    .events
                    .send(Event::Request(record_json(&record, &inner).into()));
            } else {
                later.push(seq);
            }
        }
        if !later.is_empty() {
            lock(&self.dirty).extend(later);
        }
    }

    /// 订阅事件。返回事件的接收端，以及此刻在途的记录（JSON 数组）。先订阅、后取在途的
    /// 记录：两者之间发生的变化订阅者会收到两次，不会漏。
    ///
    /// 要在 tokio 运行时里调用：第一个订阅者到来时起一个补发任务，最后一个走了它自己结束。
    pub fn subscribe(self: &Arc<Self>) -> (tokio::sync::broadcast::Receiver<Event>, String) {
        let receiver = self.events.subscribe();
        if !self.flushing.swap(true, Ordering::AcqRel) {
            let history = self.clone();
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(FLUSH_EVERY).await;
                    if history.events.receiver_count() == 0 {
                        history.flushing.store(false, Ordering::Release);
                        // 刚要结束时又来了订阅者、而它看到任务还在跑就没有另起：接着跑
                        if history.events.receiver_count() == 0
                            || history.flushing.swap(true, Ordering::AcqRel)
                        {
                            return;
                        }
                    }
                    history.flush();
                }
            });
        }
        let records: Vec<Arc<Record>> = lock(&self.records)
            .values()
            .filter(|r| !r.done.load(Ordering::Relaxed))
            .cloned()
            .collect();
        let lines: Vec<String> = records
            .iter()
            .map(|record| record_json(record, &lock(&record.inner)))
            .collect();
        (receiver, format!("[{}]", lines.join(",")))
    }

    /// 告诉订阅者实例要退出了。
    pub fn announce_closing(&self, reason: &'static str) {
        let _ = self.events.send(Event::Closing(reason));
    }

    /// 从请求标识里认出序号。不是本实例的标识时为 None。
    fn seq_of(&self, request_id: &str) -> Option<u64> {
        request_id
            .strip_prefix(&*self.instance_id)?
            .strip_prefix('-')?
            .parse()
            .ok()
    }

    fn find(&self, request_id: &str) -> Option<Arc<Record>> {
        let seq = self.seq_of(request_id)?;
        lock(&self.records).get(&seq).cloned()
    }

    /// 列记录，新的在前：`{"requests":[RequestRecord…],"more":bool}`。`before` 给了时只列
    /// 比那个请求更早的；它不是本实例的请求标识时返回 None。
    pub fn list_json(&self, before: Option<&str>, limit: usize) -> Option<String> {
        let below = match before {
            Some(id) => self.seq_of(id)?,
            None => u64::MAX,
        };
        let (page, more) = {
            let records = lock(&self.records);
            let mut older = records.range(..below).rev();
            let page: Vec<Arc<Record>> =
                older.by_ref().take(limit).map(|(_, r)| r.clone()).collect();
            (page, older.next().is_some())
        };
        let lines: Vec<String> = page
            .iter()
            .map(|record| record_json(record, &lock(&record.inner)))
            .collect();
        Some(format!(
            r#"{{"requests":[{}],"more":{more}}}"#,
            lines.join(",")
        ))
    }

    /// 一个请求的明细（RequestDetail）：记录，加上请求头与每次尝试发出、收到的头；体只给
    /// 大小与状态。没有这个请求时为 None。
    pub fn detail_json(&self, request_id: &str) -> Option<String> {
        let record = self.find(request_id)?;
        let inner = lock(&record.inner);
        let (summary, _) = capped_record(&record, &inner);
        let tries: Vec<Value> = inner
            .tries
            .iter()
            .map(|t| {
                let sent = t.sent.as_ref().map(|head| {
                    json!({
                        "method": head.method,
                        "url": head.url,
                        "headers": headers_view(&head.headers),
                        "body": t.sent_body.as_ref().map(|b| b.view(t.sent_same)),
                    })
                });
                let response = t.response.as_ref().map(|r| {
                    json!({
                        "upstream_status": r.status,
                        "headers": headers_view(&r.headers),
                        "body": r.body.view(false),
                    })
                });
                json!({
                    "attempt": t.attempt,
                    "sent": sent,
                    "response": response,
                    "error": t.error,
                })
            })
            .collect();
        let detail = json!({
            "record": summary,
            "request": {
                "headers": headers_view(&inner.request_headers),
                "body": inner.request_body.as_ref().map(|b| b.view(false)),
            },
            "tries": tries,
        });
        Some(detail.to_string())
    }

    /// 读一个体的一块：`{"total_bytes","offset","data_b64","more","complete"}`。
    /// `attempt` 只在 `part` 是 `Sent`、`Response` 时用。
    pub fn read_body_json(
        &self,
        request_id: &str,
        part: Part,
        attempt: u32,
        offset: u64,
        limit: u64,
    ) -> Result<String, ReadError> {
        use base64::Engine as _;
        let record = self
            .find(request_id)
            .ok_or(ReadError::gone("unknown_request", None))?;
        let body = {
            let inner = lock(&record.inner);
            let found = match part {
                Part::Request => inner.request_body.clone(),
                Part::Sent | Part::Response => inner
                    .tries
                    .iter()
                    .find(|t| t.attempt == attempt)
                    .and_then(|t| match part {
                        Part::Sent => t.sent_body.clone(),
                        _ => t.response.as_ref().map(|r| r.body.clone()),
                    }),
            };
            found.ok_or(ReadError::gone("no_such_part", None))?
        };
        let (bytes, total, complete) = body.read(offset, limit)?;
        let more = offset + (bytes.len() as u64) < total;
        let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
        Ok(format!(
            r#"{{"total_bytes":{total},"offset":{offset},"data_b64":"{data}","more":{more},"complete":{complete}}}"#
        ))
    }

    /// 预算超了：从最旧的请求开始丢体，直到不超。`keep` 是正在写、引发这次腾地方的那个体，
    /// 不丢它（它自己比整个预算大的情形由写者处理）。
    fn make_room(&self, keep: &Arc<Body>) {
        let limit = self.budget.limit;
        let floor = self.evict_floor.load(Ordering::Acquire);
        let mut new_floor = floor;
        let mut contiguous = true;
        {
            let records = lock(&self.records);
            for (seq, record) in records.range(floor..) {
                if self.budget.used.load(Ordering::Relaxed) <= limit {
                    break;
                }
                let inner = lock(&record.inner);
                let mut kept = false;
                for body in inner.bodies() {
                    if Arc::ptr_eq(body, keep) {
                        kept = true;
                    } else {
                        body.evict();
                    }
                }
                // 丢空了的记录连成一段时，下次从这一段之后找起
                contiguous &= !kept;
                if contiguous {
                    new_floor = seq + 1;
                }
            }
        }
        if new_floor > floor {
            // 期间有人给更旧的记录挂了新的体、把起点往回拨了的话，以它为准
            let _ = self.evict_floor.compare_exchange(
                floor,
                new_floor,
                Ordering::AcqRel,
                Ordering::Acquire,
            );
        }
    }

    /// 新建一个体，挂到序号为 `seq` 的记录名下之前调用。
    fn new_body(&self, seq: u64, encoding: Option<&str>) -> Arc<Body> {
        self.evict_floor.fetch_min(seq, Ordering::AcqRel);
        Body::new(&self.budget, encoding)
    }

    fn writer(self: &Arc<Self>, body: &Arc<Body>) -> Option<BodyWriter> {
        if self.budget.limit == 0 {
            return None;
        }
        let encoded = lock(&body.inner).encoding.is_some();
        Some(BodyWriter {
            history: self.clone(),
            body: body.clone(),
            carry: Vec::new(),
            encoded,
            dead: false,
            finished: false,
        })
    }

    /// 解码攒着的原始字节（`raw`），替换密钥，入库。在阻塞线程池里跑。
    fn store_decoded(&self, body: &Arc<Body>, raw: &[Bytes], encoding: &str) {
        let limit = self.budget.limit;
        let mut decoded = Vec::new();
        let reader = ChunkReader {
            chunks: raw,
            index: 0,
            at: 0,
        };
        // 解到超出整个预算就停：挡住压缩炸弹
        let result = crate::decode::decode_strict(encoding, reader, &mut decoded, limit);
        let outcome = match result {
            Ok(_) if self.secrets.is_empty() => Ok(decoded),
            Ok(_) => Ok(self.secrets.replace_bytes(&decoded)),
            Err(_) if decoded.len() as u64 > limit => Err(Cause::ExceedsBudget),
            Err(_) => Err(Cause::DecodeFailed),
        };
        let used = {
            let mut inner = lock(&body.inner);
            inner.complete = true;
            if inner.state != BodyState::Retained {
                // 解码期间被淘汰了：占着的预算已经还回去
                return;
            }
            match outcome {
                Ok(bytes) if bytes.len() as u64 <= limit => {
                    let now = bytes.len() as u64;
                    let before = std::mem::replace(&mut inner.held, now);
                    inner.bytes = now;
                    inner.chunks = vec![Bytes::from(bytes)];
                    self.budget.used.fetch_sub(before, Ordering::Relaxed);
                    self.budget.used.fetch_add(now, Ordering::Relaxed) + now
                }
                Ok(_) => {
                    body.release(&mut inner, BodyState::NotRetained(Cause::ExceedsBudget));
                    return;
                }
                Err(cause) => {
                    body.release(&mut inner, BodyState::NotRetained(cause));
                    return;
                }
            }
        };
        if used > limit {
            self.make_room(body);
        }
    }
}

/// 一条记录（RequestRecord）的 JSON 值与它的序列化结果，已按 [`MAX_RECORD_BYTES`] 收过。
///
/// 超出上限时依次：从最旧的尝试起清掉 `tries[].reports`；把 `path`、`user_agent`、`model`
/// 截到 [`TRUNCATED_TEXT_CHARS`] 个字符；清掉顶层的 `reports`。动过的记录带
/// `"truncated": true`。大小不受控的只有客户端给的那三个串与 format 给的报告，三步之后
/// 剩下的字段都有上限。
fn capped_record(record: &Record, inner: &RecordInner) -> (Value, String) {
    let tries: Vec<Value> = inner
        .tries
        .iter()
        .map(|t| {
            let decision = t.decision.as_ref().map(|d| {
                let mut decision = Map::new();
                decision.insert("by".into(), d.by.as_str().into());
                decision.insert("decision".into(), d.decision.into());
                if let Some(delay) = d.delay_ms {
                    decision.insert("delay_ms".into(), delay.into());
                }
                Value::Object(decision)
            });
            json!({
                "attempt": t.attempt,
                "started_at": t.started_at,
                "duration_ms": t.duration_ms,
                "upstream_status": t.upstream_status,
                "code": t.code,
                "response_bytes": t.response_bytes,
                "decision": decision,
                "reports": [],
            })
        })
        .collect();
    let mut value = json!({
        "request_id": &*record.id,
        "request_seq": record.seq,
        "started_at": inner.started_at,
        "ended_at": inner.ended_at,
        "method": inner.method,
        "path": inner.path,
        "model": inner.model,
        "stream": inner.stream,
        "user_agent": inner.user_agent,
        "delivery": inner.delivery,
        "upgraded": false,
        "outcome": inner.outcome.as_str(),
        "code": inner.code,
        "client_status": inner.client_status,
        "phase": inner.phase.as_str(),
        "next_attempt_at": inner.next_attempt_at,
        "attempts": inner.attempts,
        "tries_omitted": inner.tries_omitted,
        "tries": tries,
        "keepalive": inner.keepalive,
        "heartbeats": inner.heartbeats,
        "request_bytes": inner.request_bytes,
        "response_bytes": inner.response_bytes,
        "reports": [],
    });
    let mut text = value.to_string();
    if text.len() <= MAX_RECORD_BYTES {
        return (value, text);
    }
    value["truncated"] = Value::Bool(true);
    let tries = value["tries"].as_array().map_or(0, Vec::len);
    for index in 0..tries {
        let reports = &mut value["tries"][index]["reports"];
        if reports.as_array().is_some_and(|r| !r.is_empty()) {
            *reports = json!([]);
            text = value.to_string();
            if text.len() <= MAX_RECORD_BYTES {
                return (value, text);
            }
        }
    }
    for key in ["path", "user_agent", "model"] {
        if let Some(short) = value[key]
            .as_str()
            .filter(|s| s.chars().count() > TRUNCATED_TEXT_CHARS)
            .map(|s| s.chars().take(TRUNCATED_TEXT_CHARS).collect::<String>())
        {
            value[key] = Value::String(short);
        }
    }
    text = value.to_string();
    if text.len() <= MAX_RECORD_BYTES {
        return (value, text);
    }
    value["reports"] = json!([]);
    text = value.to_string();
    (value, text)
}

/// 一条记录的 JSON 文本（RequestRecord）。
fn record_json(record: &Record, inner: &RecordInner) -> String {
    capped_record(record, inner).1
}

struct Live {
    history: Arc<History>,
    record: Arc<Record>,
}

/// 往一条记录里写的句柄，代理热路径上随请求传递。克隆很便宜（一次引用计数）。
///
/// 没有记录的请求（入站校验拒掉的）拿到的是空句柄，所有方法什么都不做。
#[derive(Clone, Default)]
pub struct Recorder(Option<Arc<Live>>);

impl std::fmt::Debug for Recorder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            Some(live) => write!(f, "Recorder({})", live.record.id),
            None => f.write_str("Recorder(-)"),
        }
    }
}

impl Recorder {
    /// 空句柄：这个请求不进历史。
    pub fn none() -> Self {
        Recorder(None)
    }

    /// 改一下记录，然后按规矩发事件。已经结束的记录不再改。
    fn update(&self, change: impl FnOnce(&mut RecordInner, &Live)) {
        let Some(live) = &self.0 else {
            return;
        };
        let mut inner = lock(&live.record.inner);
        if inner.phase == Phase::Done {
            return;
        }
        change(&mut inner, live);
        live.history.changed(&live.record, &mut inner, false);
    }

    /// 请求进入了哪个阶段。
    pub fn phase(&self, phase: Phase) {
        self.update(|inner, _| {
            inner.phase = phase;
            inner.next_attempt_at = None;
        });
    }

    /// 开始收客户端的请求体。`encoding` 是请求头里的 content-encoding。返回往这个体里写的
    /// 写者；不保留体（预算为 0）时为 None。
    pub fn request_body(&self, encoding: Option<&str>) -> Option<BodyWriter> {
        let live = self.0.as_ref()?;
        let body = live.history.new_body(live.record.seq, encoding);
        lock(&live.record.inner).request_body = Some(body.clone());
        live.history.writer(&body)
    }

    /// 客户端发来的请求体有多少字节（线上的）。
    pub fn request_bytes(&self, bytes: u64) {
        self.update(|inner, _| inner.request_bytes = bytes);
    }

    /// 从请求里读出来的两样：顶层的 `model`，以及客户端是不是在要一个流。
    pub fn request_shape(&self, model: Option<String>, stream: bool) {
        self.update(|inner, _| {
            inner.model = model;
            inner.stream = stream;
        });
    }

    /// 这个请求走保活通道。
    pub fn keepalive(&self) {
        self.update(|inner, _| inner.keepalive = true);
    }

    /// 把要发往上游的请求行与头变成入库的形状。发出的请求不变时，各次尝试共用返回的这
    /// 一份。不记录的请求返回 None，什么都不算。
    pub fn sent_head<'a>(
        &self,
        method: &str,
        url: &str,
        headers: impl Iterator<Item = (&'a str, &'a [u8])>,
    ) -> Option<Arc<SentHead>> {
        let live = self.0.as_ref()?;
        Some(Arc::new(SentHead {
            method: method.to_string(),
            url: redact::url(url),
            headers: live.history.headers(headers),
        }))
    }

    /// 第 `attempt` 次尝试开始。`same_body`：这次发出的体与上一次是同一份（第 1 次与客户端
    /// 的原始请求体比）。不是同一份时返回往新的体里写的写者，`encoding` 是它的
    /// content-encoding。
    pub fn try_started(
        &self,
        attempt: u32,
        head: Option<&Arc<SentHead>>,
        same_body: bool,
        encoding: Option<&str>,
    ) -> Option<BodyWriter> {
        let mut writer = None;
        self.update(|inner, live| {
            let sent_body = if same_body {
                inner
                    .tries
                    .back()
                    .and_then(|t| t.sent_body.clone())
                    .or_else(|| inner.request_body.clone())
            } else {
                let body = live.history.new_body(live.record.seq, encoding);
                writer = live.history.writer(&body);
                Some(body)
            };
            inner.phase = Phase::Attempting;
            inner.next_attempt_at = None;
            inner.attempts = attempt;
            inner.tries.push_back(Try {
                attempt,
                started_at: now_millis(),
                started: Instant::now(),
                duration_ms: None,
                upstream_status: None,
                code: None,
                response_bytes: 0,
                decision: None,
                sent: head.cloned(),
                sent_body,
                sent_same: same_body,
                response: None,
                error: None,
            });
            // 只留最前几次与最近几次；中间省掉的尝试连同它们的体一起丢
            if inner.tries.len() > TRIES_HEAD + TRIES_TAIL {
                drop(inner.tries.remove(TRIES_HEAD));
                inner.tries_omitted += 1;
            }
        });
        writer
    }

    /// 第 `attempt` 次尝试拿到了上游的响应头。返回往响应体里写的写者；不保留体时为 None。
    pub fn response_head<'a>(
        &self,
        attempt: u32,
        status: u16,
        headers: impl Iterator<Item = (&'a str, &'a [u8])>,
        encoding: Option<&str>,
    ) -> Option<BodyWriter> {
        let live = self.0.as_ref()?;
        let body = live.history.new_body(live.record.seq, encoding);
        let headers = live.history.headers(headers);
        self.update(|inner, _| {
            if let Some(t) = inner.try_mut(attempt) {
                t.upstream_status = Some(status);
                t.response = Some(TryResponse {
                    status,
                    headers,
                    body: body.clone(),
                });
            }
        });
        live.history.writer(&body)
    }

    /// 第 `attempt` 次尝试结束了。`response_bytes` 是这次从上游收到的响应体字节数；
    /// `failure` 是它没成功的原因（成功为 None）。
    pub fn try_ended(&self, attempt: u32, response_bytes: u64, failure: Option<Failure<'_>>) {
        self.update(|inner, live| {
            let Some(t) = inner.try_mut(attempt) else {
                return;
            };
            t.duration_ms = Some(t.started.elapsed().as_millis() as u64);
            t.response_bytes = response_bytes;
            if let Some(failure) = failure {
                t.code = Some(failure.code);
                if !failure.has_response {
                    t.error = Some(json!({
                        "code": failure.code,
                        "details": failure.details,
                        "message": live.history.secrets.replace(failure.message),
                    }));
                }
            }
        });
    }

    /// 第 `attempt` 次尝试失败之后的决定：`by` 是谁决定的，`retry` 为真是再试、为假是停。
    pub fn decided(&self, attempt: u32, by: String, retry: bool) {
        self.update(|inner, _| {
            if let Some(t) = inner.try_mut(attempt) {
                t.decision = Some(Decision {
                    by,
                    decision: if retry { "retry" } else { "stop" },
                    delay_ms: None,
                });
            }
        });
    }

    /// 下一次尝试之前要等 `delay`。
    pub fn backoff(&self, delay: Duration) {
        self.update(|inner, _| {
            let delay_ms = delay.as_millis() as u64;
            if let Some(decision) = inner.tries.back_mut().and_then(|t| t.decision.as_mut()) {
                decision.delay_ms = Some(delay_ms);
            }
            if delay_ms > 0 {
                inner.phase = Phase::Backoff;
                inner.next_attempt_at = Some(now_millis() + delay_ms);
            }
        });
    }

    /// 给客户端发了一个心跳。
    pub fn heartbeat(&self) {
        self.update(|inner, _| inner.heartbeats += 1);
    }

    /// 又有 `bytes` 字节的响应体交给了客户端（不含心跳、aproxy 自产的错误体与终态事件）。
    pub fn response_bytes(&self, bytes: u64) {
        if bytes > 0 {
            self.update(|inner, _| inner.response_bytes += bytes);
        }
    }

    /// 给客户端的响应的状态码。
    pub fn client_status(&self, status: u16) {
        self.update(|inner, _| inner.client_status = Some(status));
    }

    /// 结局已定（响应可能还在交付）：到请求结束时它成为记录的 `outcome` 与 `code`。
    pub fn settle(&self, outcome: Outcome, code: Option<&'static str>) {
        self.update(|inner, _| inner.settled = Some((outcome, code)));
    }

    /// 请求结束。`delivered`：响应完整交给了客户端。结局没有另外定过的，交付完就是 `ok`，
    /// 没交付完就是客户端先走了。只有第一次调用生效。
    pub fn end(&self, delivered: bool) {
        let Some(live) = &self.0 else {
            return;
        };
        let mut inner = lock(&live.record.inner);
        if inner.phase == Phase::Done {
            return;
        }
        let (outcome, code) = inner.settled.take().unwrap_or(if delivered {
            (Outcome::Ok, None)
        } else {
            (Outcome::ClientGone, None)
        });
        inner.outcome = outcome;
        inner.code = code;
        inner.ended_at = Some(now_millis());
        inner.phase = Phase::Done;
        inner.next_attempt_at = None;
        live.record.done.store(true, Ordering::Relaxed);
        live.history.changed(&live.record, &mut inner, true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    const MIB: u64 = 1024 * 1024;

    fn history(budget_mb: u64, secrets: &[&str]) -> Arc<History> {
        History::new(
            budget_mb,
            Secrets::new(secrets),
            Arc::from("0123456789abcdef"),
        )
    }

    fn begin(history: &Arc<History>, seq: u64) -> Recorder {
        begin_with(history, seq, "/v1/messages?beta=1", &http::HeaderMap::new())
    }

    fn begin_with(
        history: &Arc<History>,
        seq: u64,
        path: &str,
        headers: &http::HeaderMap,
    ) -> Recorder {
        history.begin(
            seq,
            Arc::from(format!("0123456789abcdef-{seq}")),
            Start {
                method: "POST",
                path_and_query: path,
                headers,
                delivery: "retry",
                phase: Phase::ReadingBody,
            },
        )
    }

    fn id(seq: u64) -> String {
        format!("0123456789abcdef-{seq}")
    }

    /// 给一个请求存一个请求体。
    fn store_request(recorder: &Recorder, bytes: &[u8]) {
        let mut writer = recorder.request_body(None).expect("预算不为 0");
        writer.feed(&Bytes::copy_from_slice(bytes));
        writer.finish();
    }

    fn body_state(history: &History, seq: u64) -> Value {
        let detail: Value = serde_json::from_str(&history.detail_json(&id(seq)).unwrap()).unwrap();
        detail["request"]["body"].clone()
    }

    /// 把一个体分块读完。
    fn read_all(history: &History, seq: u64, part: Part, attempt: u32, step: u64) -> Vec<u8> {
        use base64::Engine as _;
        let mut out = Vec::new();
        loop {
            let chunk: Value = serde_json::from_str(
                &history
                    .read_body_json(&id(seq), part, attempt, out.len() as u64, step)
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(chunk["offset"], out.len() as u64);
            let data = base64::engine::general_purpose::STANDARD
                .decode(chunk["data_b64"].as_str().unwrap())
                .unwrap();
            assert!(data.len() as u64 <= step);
            out.extend_from_slice(&data);
            assert_eq!(
                chunk["more"],
                (out.len() as u64) < chunk["total_bytes"].as_u64().unwrap()
            );
            if chunk["more"] == false {
                return out;
            }
        }
    }

    /// RequestRecord 的线上形状：键与缺省值逐个钉住。
    #[test]
    fn record_shape_is_pinned() {
        let history = history(1, &[]);
        let mut headers = http::HeaderMap::new();
        headers.insert("user-agent", "claude-cli/2.1.0".parse().unwrap());
        let recorder = begin_with(&history, 42, "/v1/messages?beta=tools", &headers);
        let list: Value = serde_json::from_str(&history.list_json(None, 50).unwrap()).unwrap();
        assert_eq!(list["more"], false);
        let mut record = list["requests"][0].clone();
        assert!(record["started_at"].as_u64().is_some_and(|at| at > 0));
        record["started_at"] = json!(0);
        assert_eq!(
            record,
            json!({
                "request_id": "0123456789abcdef-42", "request_seq": 42, "started_at": 0,
                "ended_at": null, "method": "POST", "path": "/v1/messages?beta=***",
                "model": null, "stream": false, "user_agent": "claude-cli/2.1.0",
                "delivery": "retry", "upgraded": false, "outcome": "in_flight", "code": null,
                "client_status": null, "phase": "reading_body", "next_attempt_at": null,
                "attempts": 0, "tries_omitted": 0, "tries": [], "keepalive": false,
                "heartbeats": 0, "request_bytes": 0, "response_bytes": 0, "reports": [],
            })
        );

        recorder.request_shape(Some("claude-sonnet-4-5".into()), true);
        recorder.request_bytes(20480);
        recorder.keepalive();
        recorder.try_started(1, None, true, None);
        recorder.try_ended(
            1,
            96,
            Some(Failure {
                code: "upstream_status",
                details: &Map::new(),
                message: "the upstream returned 529",
                has_response: true,
            }),
        );
        recorder.decided(1, "rule:2".into(), true);
        recorder.backoff(Duration::from_secs(5));
        recorder.heartbeat();
        let list: Value = serde_json::from_str(&history.list_json(None, 50).unwrap()).unwrap();
        let record = &list["requests"][0];
        assert_eq!(record["model"], "claude-sonnet-4-5");
        assert_eq!(record["stream"], true);
        assert_eq!(record["keepalive"], true);
        assert_eq!(record["heartbeats"], 1);
        assert_eq!(record["request_bytes"], 20480);
        assert_eq!(record["phase"], "backoff");
        assert!(record["next_attempt_at"].as_u64().is_some());
        assert_eq!(record["attempts"], 1);
        let first = &record["tries"][0];
        assert_eq!(first["attempt"], 1);
        assert_eq!(first["code"], "upstream_status");
        assert_eq!(first["response_bytes"], 96);
        assert!(first["duration_ms"].as_u64().is_some());
        assert_eq!(
            first["decision"],
            json!({"by": "rule:2", "decision": "retry", "delay_ms": 5000})
        );
        assert_eq!(first["reports"], json!([]));

        // 结局先定、交付完才结束
        recorder.client_status(404);
        recorder.settle(Outcome::UpstreamError, Some("retry_exhausted"));
        recorder.end(true);
        // 结束之后的写不再生效
        recorder.heartbeat();
        recorder.end(false);
        let list: Value = serde_json::from_str(&history.list_json(None, 50).unwrap()).unwrap();
        let record = &list["requests"][0];
        assert_eq!(record["outcome"], "upstream_error");
        assert_eq!(record["code"], "retry_exhausted");
        assert_eq!(record["client_status"], 404);
        assert_eq!(record["phase"], "done");
        assert_eq!(record["next_attempt_at"], Value::Null);
        assert_eq!(record["heartbeats"], 1);
        assert!(record["ended_at"].as_u64().is_some());
    }

    /// 结局没有另外定过的请求：交付完是 `ok`，没交付完是客户端先走了。
    #[test]
    fn outcome_defaults_follow_delivery() {
        let history = history(1, &[]);
        begin(&history, 1).end(true);
        begin(&history, 2).end(false);
        let list: Value = serde_json::from_str(&history.list_json(None, 50).unwrap()).unwrap();
        assert_eq!(list["requests"][0]["outcome"], "client_gone");
        assert_eq!(list["requests"][1]["outcome"], "ok");
        // 空句柄什么都不做
        let none = Recorder::none();
        none.phase(Phase::Attempting);
        assert!(none.request_body(None).is_none());
        none.end(true);
    }

    /// 长重试：尝试只留最前 3 次与最近 20 次，记录不随尝试次数变大。
    #[test]
    fn tries_are_bounded_to_head_and_tail() {
        let history = history(1, &[]);
        let recorder = begin(&history, 1);
        let details: Map<String, Value> =
            serde_json::from_value(json!({"reason": "connection refused"})).unwrap();
        for attempt in 1..=5000u32 {
            recorder.try_started(attempt, None, true, None);
            recorder.try_ended(
                attempt,
                0,
                Some(Failure {
                    code: "upstream_connect_failed",
                    details: &details,
                    message: "could not connect to the upstream: connection refused",
                    has_response: false,
                }),
            );
            recorder.decided(attempt, "default".into(), true);
        }
        let text = history.list_json(None, 1).unwrap();
        assert!(text.len() < MAX_RECORD_BYTES, "{}", text.len());
        let list: Value = serde_json::from_str(&text).unwrap();
        let record = &list["requests"][0];
        assert_eq!(record["attempts"], 5000);
        assert_eq!(record["tries_omitted"], 5000 - 23);
        let kept: Vec<u64> = record["tries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["attempt"].as_u64().unwrap())
            .collect();
        let expected: Vec<u64> = (1..=3).chain(4981..=5000).collect();
        assert_eq!(kept, expected);
        assert!(record.get("truncated").is_none());

        // 明细里的失败原因跟着尝试一起留、一起丢
        let detail: Value = serde_json::from_str(&history.detail_json(&id(1)).unwrap()).unwrap();
        assert_eq!(detail["tries"].as_array().unwrap().len(), 23);
        assert_eq!(
            detail["tries"][22]["error"],
            json!({
                "code": "upstream_connect_failed",
                "details": {"reason": "connection refused"},
                "message": "could not connect to the upstream: connection refused",
            })
        );
        assert_eq!(detail["tries"][22]["response"], Value::Null);
    }

    /// 客户端给的串再长，一条记录序列化后也不超过 64 KiB，并带上 `truncated`。
    #[test]
    fn oversized_records_are_truncated_to_the_limit() {
        let history = history(1, &[]);
        let mut headers = http::HeaderMap::new();
        headers.insert(
            "user-agent",
            http::HeaderValue::from_bytes("界".repeat(30_000).as_bytes()).unwrap(),
        );
        let path = format!("/{}", "p".repeat(100_000));
        let recorder = begin_with(&history, 7, &path, &headers);
        recorder.request_shape(Some("m".repeat(50_000)), false);
        for attempt in 1..=40 {
            recorder.try_started(attempt, None, true, None);
        }
        let text = history.list_json(None, 1).unwrap();
        let list: Value = serde_json::from_str(&text).unwrap();
        let record = &list["requests"][0];
        assert!(record.to_string().len() <= MAX_RECORD_BYTES);
        assert_eq!(record["truncated"], true);
        for key in ["path", "user_agent", "model"] {
            assert_eq!(
                record[key].as_str().unwrap().chars().count(),
                TRUNCATED_TEXT_CHARS,
                "{key}"
            );
        }
        // 没超的记录不带这个键
        begin(&history, 8);
        let list: Value = serde_json::from_str(&history.list_json(None, 1).unwrap()).unwrap();
        assert!(list["requests"][0].get("truncated").is_none());
    }

    /// 记录最多 1000 条，先进先出；在途的不淘汰。
    #[test]
    fn records_are_fifo_and_in_flight_ones_stay() {
        let history = history(1, &[]);
        let stuck = begin(&history, 1);
        for seq in 2..=(MAX_RECORDS as u64 + 50) {
            begin(&history, seq).end(true);
        }
        let records = lock(&history.records);
        assert_eq!(records.len(), MAX_RECORDS);
        assert!(records.contains_key(&1), "在途的最旧一条还在");
        assert!(!records.contains_key(&2));
        assert!(!records.contains_key(&51));
        assert!(records.contains_key(&52));
        drop(records);
        assert!(history.detail_json(&id(2)).is_none());
        stuck.end(true);
    }

    /// 列记录：新的在前，`before` 往更早翻，`more` 说明后面还有没有。
    #[test]
    fn listing_pages_backwards() {
        let history = history(1, &[]);
        for seq in 1..=5 {
            begin(&history, seq).end(true);
        }
        let seqs = |text: String| -> (Vec<u64>, bool) {
            let list: Value = serde_json::from_str(&text).unwrap();
            (
                list["requests"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|r| r["request_seq"].as_u64().unwrap())
                    .collect(),
                list["more"].as_bool().unwrap(),
            )
        };
        assert_eq!(
            seqs(history.list_json(None, 2).unwrap()),
            (vec![5, 4], true)
        );
        assert_eq!(
            seqs(history.list_json(Some(&id(4)), 2).unwrap()),
            (vec![3, 2], true)
        );
        assert_eq!(
            seqs(history.list_json(Some(&id(2)), 2).unwrap()),
            (vec![1], false)
        );
        assert_eq!(
            seqs(history.list_json(Some(&id(1)), 2).unwrap()),
            (vec![], false)
        );
        // 不是本实例的标识
        assert!(history.list_json(Some("ffffffffffffffff-3"), 2).is_none());
        assert!(history.list_json(Some("0123456789abcdef-x3"), 2).is_none());
    }

    /// 10 MiB 的体分块读回来逐字节相同；`offset` 等于总长时回空块，超过时报错。
    #[test]
    fn a_large_body_round_trips_in_chunks() {
        let history = history(64, &["sk-live-0123456789abcdef"]);
        let recorder = begin(&history, 1);
        let original: Vec<u8> = (0..10 * MIB).map(|i| (i * 31 % 251) as u8).collect();
        let mut writer = recorder.request_body(None).unwrap();
        // 大小不一的块，包括比尾巴还短的
        let mut at = 0;
        for size in [1, 7, 4096, 65_536, 3, 1_000_000].iter().cycle() {
            if at >= original.len() {
                break;
            }
            let end = original.len().min(at + size);
            writer.feed(&Bytes::copy_from_slice(&original[at..end]));
            at = end;
        }
        // 还在接收：读得到的比已收到的少一段尾巴，`complete` 为假
        let chunk: Value = serde_json::from_str(
            &history
                .read_body_json(&id(1), Part::Request, 0, 0, 16)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(chunk["complete"], false);
        let total = chunk["total_bytes"].as_u64().unwrap();
        assert!(total < original.len() as u64 && total > original.len() as u64 - 64);
        writer.finish();

        assert_eq!(history.body_bytes(), original.len() as u64);
        assert_eq!(
            read_all(&history, 1, Part::Request, 0, MAX_BODY_READ),
            original
        );
        assert_eq!(read_all(&history, 1, Part::Request, 0, 99_991), original);
        assert_eq!(
            body_state(&history, 1),
            json!({"bytes": 10 * MIB, "state": "retained"})
        );

        let end: Value = serde_json::from_str(
            &history
                .read_body_json(&id(1), Part::Request, 0, 10 * MIB, 1024)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            end,
            json!({"total_bytes": 10 * MIB, "offset": 10 * MIB, "data_b64": "", "more": false, "complete": true})
        );
        assert_eq!(
            history.read_body_json(&id(1), Part::Request, 0, 10 * MIB + 1, 1024),
            Err(ReadError::BadOffset { total: 10 * MIB })
        );
        // 记录没了，体占的预算跟着还回去
        drop(recorder);
        lock(&history.records).clear();
        assert_eq!(history.body_bytes(), 0);
    }

    /// 预算超了从最旧的请求开始丢体，记录留着；正在接收的也计入。
    #[test]
    fn bodies_are_evicted_oldest_first() {
        let history = history(2, &[]);
        let block = vec![b'a'; (MIB * 3 / 4) as usize];
        for seq in 1..=2 {
            let recorder = begin(&history, seq);
            store_request(&recorder, &block);
            recorder.end(true);
        }
        assert_eq!(history.body_bytes(), MIB * 3 / 2);
        assert_eq!(body_state(&history, 1)["state"], "retained");

        // 第三个体收到一半就超了：最旧的被丢，自己继续收
        let third = begin(&history, 3);
        let mut writer = third.request_body(None).unwrap();
        writer.feed(&Bytes::from(vec![b'c'; (MIB / 4) as usize]));
        assert_eq!(body_state(&history, 1)["state"], "retained");
        writer.feed(&Bytes::from(vec![b'c'; (MIB / 2) as usize]));
        assert_eq!(
            body_state(&history, 1),
            json!({"bytes": MIB * 3 / 4, "state": "evicted"})
        );
        assert_eq!(body_state(&history, 2)["state"], "retained");
        writer.finish();
        assert_eq!(history.body_bytes(), MIB * 3 / 2);
        assert_eq!(
            history.read_body_json(&id(1), Part::Request, 0, 0, 16),
            Err(ReadError::NotFound {
                reason: "evicted",
                cause: None
            })
        );
        assert!(history.detail_json(&id(1)).is_some(), "记录本身留着");
        assert_eq!(
            read_all(&history, 3, Part::Request, 0, MAX_BODY_READ).len(),
            (MIB * 3 / 4) as usize
        );

        // 读到一半被淘汰：下一块回 not_found
        let fourth = begin(&history, 4);
        store_request(&fourth, &vec![b'd'; (MIB * 3 / 2) as usize]);
        assert_eq!(body_state(&history, 2)["state"], "evicted");
        assert_eq!(body_state(&history, 3)["state"], "evicted");
        assert_eq!(history.body_bytes(), MIB * 3 / 2);
    }

    /// 一个体就比整个预算大：不保留，已经收下的部分也丢掉，别的体不受牵连。
    #[test]
    fn a_body_larger_than_the_budget_is_not_retained() {
        let history = history(1, &[]);
        let small = begin(&history, 1);
        store_request(&small, b"small body");
        let big = begin(&history, 2);
        let mut writer = big.request_body(None).unwrap();
        for _ in 0..5 {
            writer.feed(&Bytes::from(vec![b'x'; (MIB / 4) as usize]));
        }
        assert!(!writer.is_live());
        writer.finish();
        assert_eq!(
            body_state(&history, 2),
            json!({"bytes": 0, "state": "not_retained", "cause": "exceeds_budget"})
        );
        assert_eq!(
            history.read_body_json(&id(2), Part::Request, 0, 0, 16),
            Err(ReadError::NotFound {
                reason: "not_retained",
                cause: Some("exceeds_budget")
            })
        );
        // 腾地方时最旧的小体被丢过；预算里不再有那个大体
        assert_eq!(history.body_bytes(), 0);
    }

    /// 预算为 0：不保留任何体，连写者都没有。
    #[test]
    fn a_zero_budget_keeps_no_bodies() {
        let history = history(0, &[]);
        let recorder = begin(&history, 1);
        assert!(recorder.request_body(None).is_none());
        assert!(recorder.try_started(1, None, false, None).is_none());
        assert!(
            recorder
                .response_head(1, 200, std::iter::empty(), None)
                .is_none()
        );
        let detail: Value = serde_json::from_str(&history.detail_json(&id(1)).unwrap()).unwrap();
        let disabled = json!({"bytes": 0, "state": "not_retained", "cause": "budget_disabled"});
        assert_eq!(detail["request"]["body"], disabled);
        assert_eq!(detail["tries"][0]["response"]["body"], disabled);
        assert_eq!(
            history.read_body_json(&id(1), Part::Response, 1, 0, 16),
            Err(ReadError::NotFound {
                reason: "not_retained",
                cause: Some("budget_disabled")
            })
        );
        assert_eq!(history.body_bytes(), 0);
    }

    /// 已知密钥在入库时换掉，不管块边界把它切在哪里；结果与整段一次替换相同。
    #[test]
    fn known_secrets_are_replaced_across_chunk_boundaries() {
        let secrets = ["sk-live-0123456789abcdef", "hunter2-hunter2"];
        let text = format!(
            "{{\"key\":\"{}\",\"pad\":\"{}\",\"password\":\"{}\",\"tail\":\"{}\"}}",
            secrets[0],
            "x".repeat(70),
            secrets[1],
            &secrets[0][..10],
        );
        let expected = Secrets::new(secrets).replace_bytes(text.as_bytes());
        assert!(!String::from_utf8_lossy(&expected).contains("hunter2"));
        let whole = Bytes::from(text.clone().into_bytes());
        // 每一种两段切法，以及几种固定块长
        let mut seq = 0;
        let mut check = |cuts: Vec<usize>| {
            seq += 1;
            let history = history(1, &secrets);
            let recorder = begin(&history, seq);
            let mut writer = recorder.request_body(None).unwrap();
            let mut from = 0;
            for cut in cuts.iter().copied().chain([whole.len()]) {
                writer.feed(&whole.slice(from..cut));
                from = cut;
            }
            writer.finish();
            let stored = read_all(&history, seq, Part::Request, 0, 37);
            assert_eq!(
                String::from_utf8_lossy(&stored),
                String::from_utf8_lossy(&expected),
                "切点 {cuts:?}"
            );
            assert_eq!(history.body_bytes(), expected.len() as u64);
        };
        for cut in 0..=whole.len() {
            check(vec![cut]);
        }
        for size in [1, 2, 5, 23, 24, 25, 64] {
            check((size..whole.len()).step_by(size).collect());
        }
    }

    /// 没有密钥要换时，入库的块与转发路径上的是同一份内存。
    #[test]
    fn clean_chunks_are_stored_without_copying() {
        for secrets in [&[][..], &["sk-live-0123456789abcdef"][..]] {
            let history = history(1, secrets);
            let recorder = begin(&history, 1);
            let chunk = Bytes::from(vec![b'z'; 4096]);
            let mut writer = recorder.request_body(None).unwrap();
            writer.feed(&chunk);
            writer.finish();
            let record = history.find(&id(1)).unwrap();
            let inner = lock(&record.inner);
            let body = lock(&inner.request_body.as_ref().unwrap().inner);
            assert_eq!(
                body.chunks[0].as_ptr(),
                chunk.as_ptr(),
                "密钥数 {}",
                secrets.len()
            );
            assert_eq!(body.bytes, 4096);
        }
    }

    fn gzip(data: &[u8]) -> Vec<u8> {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(data).unwrap();
        encoder.finish().unwrap()
    }

    /// 带 content-encoding 的体先解码再入库，密钥在解码之后替换；`bytes` 是解码后的大小。
    #[test]
    fn encoded_bodies_are_decoded_before_they_are_stored() {
        let history = history(1, &["sk-live-0123456789abcdef"]);
        let recorder = begin(&history, 1);
        recorder.try_started(1, None, true, None);
        let plain = br#"{"error":"bad key sk-live-0123456789abcdef"}"#;
        let packed = gzip(plain);
        let mut writer = recorder
            .response_head(1, 401, std::iter::empty(), Some("gzip"))
            .unwrap();
        let (a, b) = packed.split_at(packed.len() / 2);
        writer.feed(&Bytes::copy_from_slice(a));
        // 还没收完：读得到 0 字节，原始字节已经记在预算上
        let partial: Value = serde_json::from_str(
            &history
                .read_body_json(&id(1), Part::Response, 1, 0, 1024)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(partial["total_bytes"], 0);
        assert_eq!(partial["complete"], false);
        assert_eq!(history.body_bytes(), a.len() as u64);
        writer.feed(&Bytes::copy_from_slice(b));
        writer.finish();

        let stored = read_all(&history, 1, Part::Response, 1, 1024);
        assert_eq!(
            String::from_utf8_lossy(&stored),
            r#"{"error":"bad key <secret>"}"#
        );
        assert_eq!(history.body_bytes(), stored.len() as u64);
        let detail: Value = serde_json::from_str(&history.detail_json(&id(1)).unwrap()).unwrap();
        assert_eq!(
            detail["tries"][0]["response"]["body"],
            json!({"bytes": stored.len(), "state": "retained", "encoding": "gzip"})
        );
    }

    /// 解码失败的体不保留；解到超出整个预算的（压缩炸弹）也不保留。
    #[test]
    fn undecodable_and_exploding_bodies_are_not_retained() {
        let history = history(1, &[]);
        let recorder = begin(&history, 1);
        recorder.try_started(1, None, true, None);
        let mut writer = recorder
            .response_head(1, 200, std::iter::empty(), Some("gzip"))
            .unwrap();
        writer.feed(&Bytes::from_static(b"this is not gzip at all"));
        writer.finish();
        recorder.try_started(2, None, true, None);
        let bomb = gzip(&vec![0u8; 8 * MIB as usize]);
        assert!((bomb.len() as u64) < MIB / 16);
        let mut writer = recorder
            .response_head(2, 200, std::iter::empty(), Some("gzip"))
            .unwrap();
        writer.feed(&Bytes::from(bomb));
        writer.finish();

        let detail: Value = serde_json::from_str(&history.detail_json(&id(1)).unwrap()).unwrap();
        assert_eq!(
            detail["tries"][0]["response"]["body"],
            json!({"bytes": 0, "state": "not_retained", "cause": "decode_failed", "encoding": "gzip"})
        );
        assert_eq!(
            detail["tries"][1]["response"]["body"],
            json!({"bytes": 0, "state": "not_retained", "cause": "exceeds_budget", "encoding": "gzip"})
        );
        assert_eq!(
            history.read_body_json(&id(1), Part::Response, 1, 0, 16),
            Err(ReadError::NotFound {
                reason: "not_retained",
                cause: Some("decode_failed")
            })
        );
        assert_eq!(history.body_bytes(), 0);
    }

    /// 发出的体与上一次相同时只存一份；被省掉的尝试的体读不到，也不再占预算。
    #[test]
    fn sent_bodies_are_shared_and_dropped_with_their_tries() {
        let history = history(4, &[]);
        let recorder = begin(&history, 1);
        store_request(&recorder, b"original request");
        // 第 1 次：与客户端的请求体相同。第 2 次：换了一个体。之后都与上一次相同
        recorder.try_started(1, None, true, None);
        let mut writer = recorder.try_started(2, None, false, None).unwrap();
        writer.feed(&Bytes::from_static(b"rewritten request"));
        writer.finish();
        recorder.try_started(3, None, true, None);
        let used = history.body_bytes();
        assert_eq!(used, 16 + 17);

        let detail: Value = serde_json::from_str(&history.detail_json(&id(1)).unwrap()).unwrap();
        assert_eq!(
            detail["tries"][0]["sent"],
            Value::Null,
            "没给请求行时为 null"
        );
        assert_eq!(
            read_all(&history, 1, Part::Sent, 1, 64),
            b"original request"
        );
        assert_eq!(
            read_all(&history, 1, Part::Sent, 2, 64),
            b"rewritten request"
        );
        assert_eq!(
            read_all(&history, 1, Part::Sent, 3, 64),
            b"rewritten request"
        );
        let missing = Err(ReadError::NotFound {
            reason: "no_such_part",
            cause: None,
        });
        assert_eq!(
            history.read_body_json(&id(1), Part::Response, 1, 0, 16),
            missing
        );
        assert_eq!(
            history.read_body_json(&id(1), Part::Sent, 9, 0, 16),
            missing
        );
        assert_eq!(
            history.read_body_json(&id(9), Part::Request, 0, 0, 16),
            Err(ReadError::NotFound {
                reason: "unknown_request",
                cause: None
            })
        );

        // 每次都换体的长重试：只有留着的那 23 次的体占预算
        for attempt in 4..=100 {
            let mut writer = recorder.try_started(attempt, None, false, None).unwrap();
            writer.feed(&Bytes::from(vec![b'r'; 1000]));
            writer.finish();
        }
        // 留着的：第 1 次（原始体）、第 2、3 次（共用一份）、最近 20 次各一份
        assert_eq!(history.body_bytes(), 16 + 17 + 20 * 1000);
        assert_eq!(
            history.read_body_json(&id(1), Part::Sent, 50, 0, 16),
            missing
        );
    }

    /// 请求行、头与 URL 入库时就打码：密钥头给占位对象，URL 的查询值与别处出现的已知密钥
    /// 都换掉。
    #[test]
    fn heads_are_redacted_when_they_are_stored() {
        let history = history(1, &["sk-live-0123456789abcdef"]);
        let mut headers = http::HeaderMap::new();
        headers.insert(
            "x-api-key",
            "sk-live-0123456789abcdef-and-more".parse().unwrap(),
        );
        headers.insert("x-note", "key is sk-live-0123456789abcdef".parse().unwrap());
        headers.insert("accept", "application/json".parse().unwrap());
        let recorder = begin_with(&history, 1, "/v1/models?key=abc", &headers);
        let head = recorder.sent_head(
            "POST",
            "https://user:pass@relay.example.com/v1/chat?key=abc",
            [("authorization", &b"Bearer short"[..])].into_iter(),
        );
        recorder.try_started(1, head.as_ref(), true, None);
        recorder.response_head(
            1,
            200,
            [
                ("set-cookie", &b"session=1"[..]),
                ("content-type", &b"text/plain"[..]),
            ]
            .into_iter(),
            None,
        );
        let text = history.detail_json(&id(1)).unwrap();
        assert!(!text.contains("sk-live-0123456789abcdef"), "{text}");
        assert!(!text.contains("pass@"), "{text}");
        assert!(!text.contains("key=abc"), "{text}");
        let detail: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            detail["request"]["headers"],
            json!([
                {"name": "x-api-key", "value": {"secret": true, "hint": "sk-l…more"}},
                {"name": "x-note", "value": "key is <secret>"},
                {"name": "accept", "value": "application/json"},
            ])
        );
        let sent = &detail["tries"][0]["sent"];
        assert_eq!(sent["method"], "POST");
        assert_eq!(
            sent["headers"],
            json!([{"name": "authorization", "value": {"secret": true}}])
        );
        assert_eq!(
            detail["tries"][0]["response"]["headers"],
            json!([
                {"name": "set-cookie", "value": {"secret": true}},
                {"name": "content-type", "value": "text/plain"},
            ])
        );
        assert_eq!(detail["tries"][0]["response"]["upstream_status"], 200);
        assert_eq!(detail["record"]["path"], "/v1/models?key=***");
    }

    /// 事件：没有订阅者时不发；开始与结束立即发；中间的变化每条每秒最多一个。
    #[tokio::test]
    async fn request_events_are_coalesced() {
        let history = history(1, &[]);
        // 没有订阅者：什么都不排队
        let quiet = begin(&history, 1);
        quiet.phase(Phase::Attempting);
        assert!(lock(&history.dirty).is_empty());

        let (mut events, in_flight) = history.subscribe();
        let in_flight: Value = serde_json::from_str(&in_flight).unwrap();
        assert_eq!(in_flight.as_array().unwrap().len(), 1);
        assert_eq!(in_flight[0]["request_seq"], 1);

        let recorder = begin(&history, 2);
        let seq_and_phase = |event: Event| match event {
            Event::Request(line) => {
                let record: Value = serde_json::from_str(&line).unwrap();
                (
                    record["request_seq"].as_u64().unwrap(),
                    record["phase"].as_str().unwrap().to_string(),
                    record["attempts"].as_u64().unwrap(),
                )
            }
            other => panic!("{other:?}"),
        };
        assert_eq!(
            seq_and_phase(events.try_recv().unwrap()),
            (2, "reading_body".into(), 0)
        );
        // 一秒之内的一串变化：一个事件都不发，只排一次队
        for attempt in 1..=50 {
            recorder.try_started(attempt, None, true, None);
        }
        assert!(events.try_recv().is_err());
        assert_eq!(*lock(&history.dirty), vec![2]);
        // 满一秒之后补发一个，带着最新的状态
        let event = tokio::time::timeout(Duration::from_secs(3), events.recv())
            .await
            .expect("补发任务没有发出合并后的事件")
            .unwrap();
        assert_eq!(seq_and_phase(event), (2, "attempting".into(), 50));
        assert!(events.try_recv().is_err());
        // 结束立即发，不等
        recorder.try_started(51, None, true, None);
        recorder.end(true);
        assert_eq!(
            seq_and_phase(events.try_recv().unwrap()),
            (2, "done".into(), 51)
        );
        tokio::time::sleep(FLUSH_EVERY * 2).await;
        assert!(events.try_recv().is_err(), "结束之后不再补发");

        history.announce_closing("shutdown");
        assert!(matches!(
            events.try_recv().unwrap(),
            Event::Closing("shutdown")
        ));
        // 最后一个订阅者走了，补发任务自己结束
        drop(events);
        tokio::time::sleep(FLUSH_EVERY * 3).await;
        assert!(!history.flushing.load(Ordering::Acquire));
    }
}
