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
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
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
/// 记录超出上限时，`method`、`path`、`user_agent`、`model` 各截到多少个字符
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
/// 同时最多多少个订阅者
pub const MAX_SUBSCRIBERS: usize = 8;

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
    /// 主动返回 format 选择的客户端协议响应。
    FormatResponse,
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
            Outcome::FormatResponse => "format_response",
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

    /// 把入库的字节摘下来、把占着的预算还回去，状态改成 `state`。摘下来的块交还给调用方：
    /// 释放它们的内存可能要找系统（大块）或者进空闲表，调用方手里要是还握着别人在等的锁，
    /// 应当先放了锁再丢它们。
    #[must_use]
    fn release(&self, inner: &mut BodyInner, state: BodyState) -> Vec<Bytes> {
        self.budget.used.fetch_sub(inner.held, Ordering::Relaxed);
        inner.held = 0;
        inner.state = state;
        std::mem::take(&mut inner.chunks)
    }

    /// 为了腾地方丢掉这个体，返回摘下来的块（见 [`Body::release`]）。已经不在的不动。
    #[must_use]
    fn evict(&self) -> Vec<Bytes> {
        let mut inner = lock(&self.inner);
        if inner.state == BodyState::Retained {
            self.release(&mut inner, BodyState::Evicted)
        } else {
            Vec::new()
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
    /// [`BodyWriter::feed_copy`] 正在填的一块（[`COPY_BLOCK`] 字节，取自空闲表）、已经填了
    /// 多少，以及它里面最早的字节是什么时候进来的
    block: Option<Box<[u8]>>,
    filled: usize,
    block_since: Instant,
    /// 这个体已经不保留了（被淘汰、超出预算）：之后进来的块直接丢掉
    dead: bool,
    finished: bool,
}

/// [`BodyWriter::feed_copy`] 攒到多大就入库
const COPY_BLOCK: usize = 64 * 1024;
/// [`BodyWriter::feed_copy`] 攒着的字节最多留多久再入库：正在接收的体，读的人最多晚这么久
/// 看到新到的字节
const COPY_BLOCK_AGE: Duration = Duration::from_millis(250);
/// 空闲表里最多留多少块（见 [`BlockPool`]）
const MAX_FREE_BLOCKS: usize = 128;
/// 循环使用的大缓冲区的容量（见 [`BufferPool`]）：正好是请求体在内存里驻留的上限，溢写
/// 磁盘之前攒下的那一段用的就是这么大的缓冲
pub const LARGE_BUFFER: usize = 1024 * 1024;
/// 空闲表里最多留多少个大缓冲区（见 [`BufferPool`]）
const MAX_FREE_BUFFERS: usize = 8;

/// [`COPY_BLOCK`] 字节的块的空闲表：被丢掉的块回到这里，下一个体接着用。
///
/// 高吞吐下历史每秒要换掉几百 MiB 的体。每一块都向系统要新内存、用完还回去的话，每次都要
/// 重新建立页映射，这笔开销比拷贝本身还大，会实实在在拖慢转发；块循环使用，稳定之后就不
/// 再向系统要内存。空闲表有上限（预算的八分之一，最多 [`MAX_FREE_BLOCKS`] 块），所以历史
/// 占的内存是「预算，加上至多这么多空闲块，再加上 [`BufferPool`] 里空闲的缓冲区」。
#[derive(Debug)]
struct BlockPool {
    free: Mutex<Vec<Box<[u8]>>>,
    /// 空闲表最多留几块
    keep: usize,
}

impl BlockPool {
    fn take(&self) -> Box<[u8]> {
        lock(&self.free)
            .pop()
            .unwrap_or_else(|| vec![0u8; COPY_BLOCK].into_boxed_slice())
    }

    fn give(&self, block: Box<[u8]>) {
        let mut free = lock(&self.free);
        if free.len() < self.keep {
            free.push(block);
        }
    }
}

/// [`LARGE_BUFFER`] 字节的缓冲区的空闲表：大请求体溢写磁盘之前在内存里攒的那一段，写进
/// 文件之后整块进历史，被丢掉时回到这里，下一个大请求体接着用。
///
/// 这么大的一块内存，分配器是直接向系统要、用完直接还的：要来时每一页都要缺页一次，还回去
/// 要拆页表、通知各个核，一来一回是几百微秒的 CPU。循环使用就没有这笔开销。空闲表有上限
/// （预算的八分之一，最多 [`MAX_FREE_BUFFERS`] 个）。
#[derive(Debug)]
struct BufferPool {
    free: Mutex<Vec<Vec<u8>>>,
    /// 空闲表最多留几个
    keep: usize,
}

impl BufferPool {
    /// 一个空的、容量为 [`LARGE_BUFFER`] 的缓冲区。
    fn take(&self) -> Vec<u8> {
        lock(&self.free)
            .pop()
            .unwrap_or_else(|| Vec::with_capacity(LARGE_BUFFER))
    }

    fn give(&self, mut buffer: Vec<u8>) {
        buffer.clear();
        let mut free = lock(&self.free);
        if free.len() < self.keep {
            free.push(buffer);
        }
    }
}

/// 一个入了库的大缓冲区：没有人再用它时回到空闲表。
struct PooledBuffer {
    data: Option<Vec<u8>>,
    pool: Arc<BufferPool>,
}

impl AsRef<[u8]> for PooledBuffer {
    fn as_ref(&self) -> &[u8] {
        self.data.as_deref().unwrap_or_default()
    }
}

impl Drop for PooledBuffer {
    fn drop(&mut self) {
        if let Some(data) = self.data.take() {
            self.pool.give(data);
        }
    }
}

/// 一块填满了、入了库的块：没有人再用它时回到空闲表。
struct PooledBlock {
    data: Option<Box<[u8]>>,
    pool: Arc<BlockPool>,
}

impl AsRef<[u8]> for PooledBlock {
    fn as_ref(&self) -> &[u8] {
        self.data.as_deref().unwrap_or_default()
    }
}

impl Drop for PooledBlock {
    fn drop(&mut self) {
        if let Some(data) = self.data.take() {
            self.pool.give(data);
        }
    }
}

impl BodyWriter {
    /// 交一块自己拥有、大小正好的字节（读完的请求体、format 给出的体）：入库的就是这一块，
    /// 不拷贝。
    ///
    /// 网络库交出来的块不要走这里：它们是读缓冲区的切片，留住几百字节的一小片会把整个
    /// 缓冲区（几 KiB 到几百 KiB）一起留住，实际占的内存就远多于记在预算上的。那样的块用
    /// [`BodyWriter::feed_copy`]。
    pub fn feed(&mut self, chunk: &Bytes) {
        self.flush_block();
        self.accept(chunk);
    }

    /// 一个空的缓冲区，容量为 `capacity`：调用方往里攒字节，攒好了经
    /// [`BodyWriter::feed_buffer`] 整个交回来。要的正好是 [`LARGE_BUFFER`] 时从空闲表里取
    /// （见 [`BufferPool`]），别的大小现分配。
    pub fn buffer(&self, capacity: usize) -> Vec<u8> {
        if capacity == LARGE_BUFFER {
            self.history.buffers.take()
        } else {
            Vec::with_capacity(capacity)
        }
    }

    /// 交一个自己拥有的缓冲区：入库的就是它，不拷贝。容量是 [`LARGE_BUFFER`] 的（不管是不是
    /// 从 [`BodyWriter::buffer`] 拿的）在没有人再用它时进空闲表。
    ///
    /// 记在预算上的是它里面的字节数，所以交过来之前要么填满、要么把多余的容量还回去。
    pub fn feed_buffer(&mut self, buffer: Vec<u8>) {
        let chunk = if buffer.capacity() == LARGE_BUFFER {
            Bytes::from_owner(PooledBuffer {
                data: Some(buffer),
                pool: self.history.buffers.clone(),
            })
        } else {
            Bytes::from(buffer)
        };
        self.feed(&chunk);
    }

    /// 交一块借来的字节（网络库的读缓冲区切片）：拷进自己的块里，填满 [`COPY_BLOCK`] 或
    /// 留了 [`COPY_BLOCK_AGE`] 再入库。入库的块大小正好，记在预算上的就是实际占的。
    pub fn feed_copy(&mut self, mut data: &[u8]) {
        while !self.dead && !data.is_empty() {
            let pool = &self.history.pool;
            let block = self.block.get_or_insert_with(|| pool.take());
            if self.filled == 0 {
                self.block_since = Instant::now();
            }
            let n = data.len().min(COPY_BLOCK - self.filled);
            block[self.filled..self.filled + n].copy_from_slice(&data[..n]);
            self.filled += n;
            data = &data[n..];
            if self.filled == COPY_BLOCK {
                // 填满了：整块入库，没有人再用它时回到空闲表
                self.filled = 0;
                let full = Bytes::from_owner(PooledBlock {
                    data: self.block.take(),
                    pool: pool.clone(),
                });
                self.accept(&full);
            }
        }
        if self.filled > 0 && self.block_since.elapsed() >= COPY_BLOCK_AGE {
            self.flush_block();
        }
    }

    /// 把 [`BodyWriter::feed_copy`] 攒着的、还没填满一块的字节入库：按实际大小另存一份，
    /// 正在填的那一块留着接着用。
    fn flush_block(&mut self) {
        if self.filled == 0 {
            return;
        }
        let filled = std::mem::take(&mut self.filled);
        let part = match &self.block {
            Some(block) => Bytes::copy_from_slice(&block[..filled]),
            None => return,
        };
        self.accept(&part);
    }

    fn accept(&mut self, chunk: &Bytes) {
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
                // 握着的只有这个体自己的锁，没有别人在等：就地释放
                drop(
                    self.body
                        .release(&mut inner, BodyState::NotRetained(Cause::ExceedsBudget)),
                );
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
        self.flush_block();
        if let Some(block) = self.block.take() {
            self.history.pool.give(block);
        }
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

/// 一组头，入库时就写成 JSON 数组的文本：`[{"name":…,"value":…},…]`。密钥头（名字像鉴权
/// 的）的值是占位对象，其余是字符串。
///
/// 存成一段文本而不是逐个头的结构：每个请求、每次尝试都要存一组头，这样一组只分配一次。
type HeaderList = Box<str>;

/// 一次尝试发往上游的请求行与头（体另存）。发出的请求不变时，各次尝试共用同一份。
#[derive(Debug)]
pub struct SentHead {
    method: String,
    /// 已打码
    url: String,
    headers: HeaderList,
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
    /// 等待期间的单调起点；正常结束或客户端取消时结算实际时长。
    backoff_started: Option<Instant>,
    /// 只有程序明确表态才有这一项，不把默认重跑冒充投票。
    reroute: Option<bool>,
}

/// 一次尝试收到的上游响应（体另存）。
#[derive(Debug)]
struct TryResponse {
    status: u16,
    headers: HeaderList,
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
    /// 与这一轮一起保留、一起淘汰的 retry 与 decide 报告。
    reports: Vec<Value>,
    sent: Option<Arc<SentHead>>,
    /// 这次发出的体。与上一次相同时指向同一个体（`sent_same` 为真），只存一份
    sent_body: Option<Arc<Body>>,
    sent_same: bool,
    response: Option<TryResponse>,
    /// 没拿到完整的上游响应时的 `{code, details, message}`
    error: Option<Value>,
}

/// 顶层报告在同一阶段内按配置位置替换；位置是内部身份，不在 IPC 上公开。
#[derive(Debug, serde::Serialize)]
#[serde(transparent)]
struct LayerReport {
    /// 名字经脱敏后可能碰撞，位置不会。
    #[serde(skip)]
    layer: usize,
    /// 唯一公开的数据，已经过脱敏和长度限制。
    value: Value,
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
    /// 最近一次确实收到的上游响应头，尝试被裁掉也不丢失。
    last_upstream: Option<u16>,
    /// 流被截断后外层可以合法地给出结尾字节，但这不算上游正常结束。
    stream_aborted: bool,
    /// 已经失败的 forward 尝试被 decide 救回并改走重试，不含初始 request 投票。
    upgraded: bool,
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
    /// request 与 response 报告各按程序保留，重试同一层不能使顶层记录无限增长。
    reports: Vec<LayerReport>,
    keepalive: bool,
    heartbeats: u64,
    request_bytes: u64,
    response_bytes: u64,
    request_headers: HeaderList,
    request_body: Option<Arc<Body>>,
    /// 最近一次为它发事件的时刻
    last_event_at: u64,
    /// 有变化还没发事件，已经排进补发的队列
    queued: bool,
}

impl RecordInner {
    /// 等待中保留预计的下次尝试时刻；请求提前结束也只记真正等过的时间。
    fn finish_backoff(&mut self) {
        if let Some(d) = self.tries.back_mut().and_then(|t| t.decision.as_mut())
            && let Some(start) = d.backoff_started.take()
        {
            d.delay_ms = Some(start.elapsed().as_millis() as u64);
        }
    }
    /// 格式程序可能在发送之前就报告本轮工作；先建一个无发出头、无上游响应的尝试，
    /// 随后真正发送时补写头和体，不重复计数，也不会在断开时丢掉早到的报告。
    fn enter_try(&mut self, attempt: u32) -> &mut Try {
        if self.tries.back().is_none_or(|t| t.attempt != attempt) {
            self.attempts = attempt;
            self.tries.push_back(Try {
                attempt,
                started_at: now_millis(),
                started: Instant::now(),
                duration_ms: None,
                upstream_status: None,
                code: None,
                response_bytes: 0,
                decision: None,
                reports: Vec::new(),
                sent: None,
                sent_body: None,
                sent_same: false,
                response: None,
                error: None,
            });
            // 只留最前几次与最近几次；中间省掉的尝试连同它们的体一起丢
            if self.tries.len() > TRIES_HEAD + TRIES_TAIL {
                drop(self.tries.remove(TRIES_HEAD));
                self.tries_omitted += 1;
            }
        }
        self.tries.back_mut().expect("attempt just entered")
    }

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
    /// 一条记录变了：`record` 是整条记录的 JSON，订阅者按 `request_id` 整条替换；`at` 是
    /// 记录成为这个样子的时刻（Unix 毫秒）。限频按这个时刻算，所以同一条记录相邻两个中间
    /// 事件的 `at` 至少相隔一秒——事件什么时候真的写到订阅者的连接上，前后还差着排队的时间
    Request { at: u64, record: Arc<str> },
    /// 进程池状态变动时的完整脱敏条目，与记录共用有界队列和 lagged 语义。
    Format {
        at: u64,
        index: usize,
        revision: u64,
        snapshot: Arc<Value>,
    },
    /// 实例要退出了。`reason` 取 `shutdown` 或 `swap`
    Closing(&'static str),
}

/// 一个订阅：事件的接收端，以及订阅那一刻在途的记录。丢掉它就是退订。
pub struct Subscription {
    history: Arc<History>,
    /// 事件。跟不上时收到的是「丢了几个」（`RecvError::Lagged`），之后接着收新的
    pub events: tokio::sync::broadcast::Receiver<Event>,
    /// 订阅那一刻在途的记录：一个 JSON 数组的文本，元素是 RequestRecord
    pub in_flight: String,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        self.history.subscribers.fetch_sub(1, Ordering::AcqRel);
    }
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
    /// 拷贝入库用的块的空闲表
    pool: Arc<BlockPool>,
    /// 大缓冲区的空闲表
    buffers: Arc<BufferPool>,
    secrets: Secrets,
    records: Mutex<BTreeMap<u64, Arc<Record>>>,
    /// 序号比它小的记录都没有还留着的体：腾地方时从它开始找，不必每次从头扫过早已空了的
    /// 记录。给旧记录挂上新的体时把它往回拨
    evict_floor: AtomicU64,
    events: tokio::sync::broadcast::Sender<Event>,
    /// 此刻有多少个订阅者。热路径每次改记录都要问一句「有人在听吗」，所以自己数：问广播
    /// 通道要拿它的锁，所有请求会挤在那一把锁上
    subscribers: AtomicUsize,
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
        let limit = budget_mb.saturating_mul(1024 * 1024);
        Arc::new(History {
            instance_id,
            budget: Arc::new(Budget {
                limit,
                used: AtomicU64::new(0),
            }),
            pool: Arc::new(BlockPool {
                free: Mutex::new(Vec::new()),
                keep: usize::try_from(limit / COPY_BLOCK as u64 / 8)
                    .unwrap_or(MAX_FREE_BLOCKS)
                    .min(MAX_FREE_BLOCKS),
            }),
            buffers: Arc::new(BufferPool {
                free: Mutex::new(Vec::new()),
                keep: usize::try_from(limit / LARGE_BUFFER as u64 / 8)
                    .unwrap_or(MAX_FREE_BUFFERS)
                    .min(MAX_FREE_BUFFERS),
            }),
            secrets,
            records: Mutex::new(BTreeMap::new()),
            evict_floor: AtomicU64::new(0),
            events,
            subscribers: AtomicUsize::new(0),
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

    /// URL 中的用户信息和查询值按位置遮住；路径中若含实例已知的凭据，也不能写进记录或日志。
    pub(crate) fn redacted_url(&self, url: &str) -> String {
        let masked = redact::url(url);
        if self.secrets.is_empty() {
            masked
        } else {
            self.secrets.replace(&masked)
        }
    }

    /// 把一组头变成入库的形状：密钥头的值换成占位对象，其余的值里出现的已知密钥换掉。
    fn headers<'a>(&self, headers: impl Iterator<Item = (&'a str, &'a [u8])>) -> HeaderList {
        // serde_json 往字节缓冲里写的都是合法的 UTF-8；往 Vec 里写不会失败
        fn push(out: &mut Vec<u8>, value: &impl serde::Serialize) {
            let _ = serde_json::to_writer(out, value);
        }
        let mut out = Vec::with_capacity(1024);
        out.push(b'[');
        for (name, value) in headers {
            if out.len() > 1 {
                out.push(b',');
            }
            out.extend_from_slice(br#"{"name":"#);
            push(&mut out, &name);
            out.extend_from_slice(br#","value":"#);
            let text = String::from_utf8_lossy(value);
            if redact::is_secret_header(name) {
                push(&mut out, &redact::placeholder(&text));
            } else if self.secrets.find(value).is_some() {
                push(&mut out, &self.secrets.replace(&text));
            } else {
                push(&mut out, &text);
            }
            out.push(b'}');
        }
        out.push(b']');
        String::from_utf8(out).unwrap_or_default().into_boxed_str()
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
            path: self.redacted_url(start.path_and_query),
            model: None,
            stream: false,
            user_agent,
            delivery: start.delivery,
            last_upstream: None,
            stream_aborted: false,
            upgraded: false,
            outcome: Outcome::InFlight,
            settled: None,
            code: None,
            client_status: None,
            phase: start.phase,
            next_attempt_at: None,
            attempts: 0,
            tries_omitted: 0,
            tries: VecDeque::new(),
            reports: Vec::new(),
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
        if self.subscribers.load(Ordering::Relaxed) > 0 {
            let line = record_json(&record, &lock(&record.inner));
            let _ = self.events.send(Event::Request {
                at: now,
                record: line.into(),
            });
        }
        Recorder(Some(Arc::new(Live {
            history: self.clone(),
            record,
        })))
    }

    /// 记录变了。`urgent`（开始、结束）立即发事件；其余的变化，距上一个事件不到一秒就先
    /// 记下，由补发任务合并成一个。没有订阅者时什么都不做——连记录的 JSON 都不构造。
    fn changed(&self, record: &Record, inner: &mut RecordInner, urgent: bool) {
        if self.subscribers.load(Ordering::Relaxed) == 0 {
            inner.queued = false;
            return;
        }
        let now = now_millis();
        if urgent || now.saturating_sub(inner.last_event_at) >= REQUEST_EVENT_INTERVAL_MS {
            inner.last_event_at = now;
            inner.queued = false;
            let _ = self.events.send(Event::Request {
                at: now,
                record: record_json(record, inner).into(),
            });
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
                let _ = self.events.send(Event::Request {
                    at: now,
                    record: record_json(&record, &inner).into(),
                });
            } else {
                later.push(seq);
            }
        }
        if !later.is_empty() {
            lock(&self.dirty).extend(later);
        }
    }

    /// 订阅事件。已经有 [`MAX_SUBSCRIBERS`] 个订阅者时为 None。先订阅、后取在途的记录：
    /// 两者之间发生的变化订阅者会收到两次，不会漏。
    ///
    /// 要在 tokio 运行时里调用：第一个订阅者到来时起一个补发任务，最后一个走了它自己结束。
    pub fn subscribe(self: &Arc<Self>) -> Option<Subscription> {
        // 占一个名额：满了就不给
        let mut count = self.subscribers.load(Ordering::Acquire);
        loop {
            if count >= MAX_SUBSCRIBERS {
                return None;
            }
            match self.subscribers.compare_exchange_weak(
                count,
                count + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(now) => count = now,
            }
        }
        let events = self.events.subscribe();
        if !self.flushing.swap(true, Ordering::AcqRel) {
            let history = self.clone();
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(FLUSH_EVERY).await;
                    if history.subscribers.load(Ordering::Acquire) == 0 {
                        history.flushing.store(false, Ordering::Release);
                        // 刚要结束时又来了订阅者、而它看到任务还在跑就没有另起：接着跑
                        if history.subscribers.load(Ordering::Acquire) == 0
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
        Some(Subscription {
            history: self.clone(),
            events,
            in_flight: format!("[{}]", lines.join(",")),
        })
    }

    /// 此刻有多少个订阅者。
    pub fn subscribers(&self) -> usize {
        self.subscribers.load(Ordering::Acquire)
    }

    /// 没有实时读者就不序列化。新读者从订阅后的初始快照和它的修订号接着读。
    pub(crate) fn format_transition(
        &self,
        index: usize,
        revision: u64,
        snapshot: impl FnOnce() -> Value,
    ) {
        if self.events.receiver_count() != 0 {
            let _ = self.events.send(Event::Format {
                at: crate::daemon::now_millis(),
                index,
                revision,
                snapshot: Arc::new(snapshot()),
            });
        }
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
        // 头入库时已经是 JSON 文本，记录也直接写成文本：整份明细按文本拼，不搭 JSON 树
        let body = |body: Option<&Arc<Body>>, same: bool| {
            body.map_or_else(|| "null".to_string(), |b| b.view(same).to_string())
        };
        let tries: Vec<String> = inner
            .tries
            .iter()
            .map(|t| {
                let sent = t.sent.as_ref().map_or_else(
                    || "null".to_string(),
                    |head| {
                        format!(
                            r#"{{"method":{},"url":{},"headers":{},"body":{}}}"#,
                            Value::from(head.method.as_str()),
                            Value::from(head.url.as_str()),
                            head.headers,
                            body(t.sent_body.as_ref(), t.sent_same),
                        )
                    },
                );
                let response = t.response.as_ref().map_or_else(
                    || "null".to_string(),
                    |r| {
                        format!(
                            r#"{{"upstream_status":{},"headers":{},"body":{}}}"#,
                            r.status,
                            r.headers,
                            body(Some(&r.body), false),
                        )
                    },
                );
                format!(
                    r#"{{"attempt":{},"sent":{sent},"response":{response},"error":{}}}"#,
                    t.attempt,
                    t.error.as_ref().unwrap_or(&Value::Null),
                )
            })
            .collect();
        Some(format!(
            r#"{{"record":{},"request":{{"headers":{},"body":{}}},"tries":[{}]}}"#,
            record_json(&record, &inner),
            inner.request_headers,
            body(inner.request_body.as_ref(), false),
            tries.join(","),
        ))
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
        // 丢掉的块攒在这里，放了记录表的锁再释放：每个新请求都要拿这把锁，不能让它们等着
        // 这边把几 MiB 内存还完
        let mut freed: Vec<Vec<Bytes>> = Vec::new();
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
                        let chunks = body.evict();
                        if !chunks.is_empty() {
                            freed.push(chunks);
                        }
                    }
                }
                // 丢空了的记录连成一段时，下次从这一段之后找起
                contiguous &= !kept;
                if contiguous {
                    new_floor = seq + 1;
                }
            }
        }
        drop(freed);
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
            block: None,
            filled: 0,
            block_since: Instant::now(),
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
                    drop(body.release(&mut inner, BodyState::NotRetained(Cause::ExceedsBudget)));
                    return;
                }
                Err(cause) => {
                    drop(body.release(&mut inner, BodyState::NotRetained(cause)));
                    return;
                }
            }
        };
        if used > limit {
            self.make_room(body);
        }
    }
}

/// 记录里的一次尝试，序列化用。
#[derive(serde::Serialize)]
struct TryView<'a> {
    attempt: u32,
    started_at: u64,
    duration_ms: Option<u64>,
    upstream_status: Option<u16>,
    code: Option<&'a str>,
    response_bytes: u64,
    decision: Option<DecisionView<'a>>,
    /// 这次尝试的 retry、decide 阶段的报告。
    reports: &'a [Value],
}

#[derive(serde::Serialize)]
struct DecisionView<'a> {
    by: &'a str,
    decision: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    delay_ms: Option<u64>,
    /// 缺省和投了 false 有不同含义。
    #[serde(skip_serializing_if = "Option::is_none")]
    reroute: Option<bool>,
}

/// 一条记录（RequestRecord）的线上形状，序列化用。字段的次序就是线上的次序。
#[derive(serde::Serialize)]
struct RecordView<'a> {
    request_id: &'a str,
    request_seq: u64,
    started_at: u64,
    ended_at: Option<u64>,
    method: &'a str,
    path: &'a str,
    model: Option<&'a str>,
    stream: bool,
    user_agent: Option<&'a str>,
    delivery: &'a str,
    /// 中途从 forward 转成了 retry。本版本的投递方式由配置定死，恒为 false
    upgraded: bool,
    outcome: &'a str,
    code: Option<&'a str>,
    client_status: Option<u16>,
    phase: &'a str,
    next_attempt_at: Option<u64>,
    attempts: u32,
    tries_omitted: u32,
    tries: Vec<TryView<'a>>,
    keepalive: bool,
    heartbeats: u64,
    request_bytes: u64,
    response_bytes: u64,
    /// request、response、stream 阶段的报告。
    reports: &'a [LayerReport],
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    truncated: bool,
}

/// `text` 的前 [`TRUNCATED_TEXT_CHARS`] 个字符（按字符数，不会切在多字节字符中间）。
fn clipped(text: &str) -> &str {
    match text.char_indices().nth(TRUNCATED_TEXT_CHARS) {
        Some((end, _)) => &text[..end],
        None => text,
    }
}

/// 一条记录的 JSON 文本（RequestRecord），不超过 [`MAX_RECORD_BYTES`]。
///
/// 超出时先从最旧的尝试起去报告，再截短客户端的四个字符串，最后才丢顶层报告。
/// 只改变序列化视图，不让某次读取改写还在进行中的请求。
///
/// 直接从记录写成文本，不先搭一棵 JSON 树：有订阅者时每个请求至少要写两次。
fn record_json(record: &Record, inner: &RecordInner) -> String {
    let view = |truncated: bool, clip_text: bool, skip: usize, top: bool| {
        fn clip(text: &str, truncated: bool) -> &str {
            if truncated { clipped(text) } else { text }
        }
        RecordView {
            request_id: &record.id,
            request_seq: record.seq,
            started_at: inner.started_at,
            ended_at: inner.ended_at,
            method: clip(&inner.method, clip_text),
            path: clip(&inner.path, clip_text),
            model: inner.model.as_deref().map(|text| clip(text, clip_text)),
            stream: inner.stream,
            user_agent: inner
                .user_agent
                .as_deref()
                .map(|text| clip(text, clip_text)),
            delivery: inner.delivery,
            upgraded: inner.upgraded,
            outcome: inner.outcome.as_str(),
            code: inner.code,
            client_status: inner.client_status,
            phase: inner.phase.as_str(),
            next_attempt_at: inner.next_attempt_at,
            attempts: inner.attempts,
            tries_omitted: inner.tries_omitted,
            tries: inner
                .tries
                .iter()
                .enumerate()
                .map(|(i, t)| TryView {
                    attempt: t.attempt,
                    started_at: t.started_at,
                    duration_ms: t.duration_ms,
                    upstream_status: t.upstream_status,
                    code: t.code,
                    response_bytes: t.response_bytes,
                    decision: t.decision.as_ref().map(|d| DecisionView {
                        by: &d.by,
                        decision: d.decision,
                        delay_ms: d.delay_ms,
                        reroute: d.reroute,
                    }),
                    reports: if i < skip { &[] } else { &t.reports },
                })
                .collect(),
            keepalive: inner.keepalive,
            heartbeats: inner.heartbeats,
            request_bytes: inner.request_bytes,
            response_bytes: inner.response_bytes,
            reports: if top { &inner.reports } else { &[] },
            truncated,
        }
    };
    // 这些结构里没有会让序列化失败的东西（没有非字符串的键，没有自定义的 Serialize）
    let text = serde_json::to_string(&view(false, false, 0, true)).unwrap_or_default();
    if text.len() <= MAX_RECORD_BYTES {
        return text;
    }
    for (i, t) in inner.tries.iter().enumerate() {
        if !t.reports.is_empty() {
            let text = serde_json::to_string(&view(true, false, i + 1, true)).unwrap();
            if text.len() <= MAX_RECORD_BYTES {
                return text;
            }
        }
    }
    let clipped = serde_json::to_string(&view(true, true, usize::MAX, true)).unwrap();
    if clipped.len() <= MAX_RECORD_BYTES {
        return clipped;
    }
    serde_json::to_string(&view(true, true, usize::MAX, false)).unwrap()
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

    /// request 初选与失败后的升级都改变当前方式，只有后者置位升级标记。
    pub(crate) fn set_delivery(&self, forward: bool, upgraded: bool) {
        self.update(|inner, _| {
            inner.delivery = if forward { "forward" } else { "retry" };
            inner.upgraded |= upgraded;
        });
    }

    /// 这个请求走保活通道。
    pub fn keepalive(&self) {
        self.set_keepalive(true);
    }

    /// 只有响应尚未提交时才能重新选择保活；历史反映这次请求实际选用的出口。
    pub(crate) fn set_keepalive(&self, enabled: bool) {
        self.update(|inner, _| inner.keepalive = enabled);
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
            url: live.history.redacted_url(url),
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
                    .iter()
                    .rev()
                    .find(|t| t.attempt != attempt)
                    .and_then(|t| t.sent_body.clone())
                    .or_else(|| inner.request_body.clone())
            } else {
                let body = live.history.new_body(live.record.seq, encoding);
                writer = live.history.writer(&body);
                Some(body)
            };
            inner.phase = Phase::Attempting;
            inner.next_attempt_at = None;
            let t = inner.enter_try(attempt);
            t.sent = head.cloned();
            t.sent_body = sent_body;
            t.sent_same = same_body;
        });
        writer
    }

    /// 来自程序的报告先打码再限长。同一层在首轮 stage 失败而续跑时，保留它最后一份报告。
    pub(crate) fn format_report(
        &self,
        (layer, name): (usize, &str),
        stage: &str,
        attempt: Option<u32>,
        mut report: aproxy_envelope::Report,
    ) {
        let Some(live) = &self.0 else { return };
        let secrets = &live.history.secrets;
        let name = secrets.replace(name);
        report.summary = secrets.replace(&report.summary);
        if let Some((end, _)) = report.summary.char_indices().nth(512) {
            report.summary.truncate(end);
            tracing::warn!(format=%name,stage,"format report summary truncated to 512 characters");
        }
        if report.actions.len() > 32 {
            tracing::warn!(format=%name,stage,"format report truncated to 32 actions");
            report.actions.truncate(32);
        }
        let mut value = json!(report);
        value["format"] = json!(name);
        value["stage"] = json!(stage);
        for action in value["actions"].as_array_mut().unwrap() {
            for field in action.as_object_mut().unwrap().values_mut() {
                if let Value::String(s) = field {
                    // 恰好达到接收上限时，末尾可能是被截断的凭据前缀；宁可多遮住几个字符。
                    *s = if s.chars().count() >= 256 {
                        let end = s.char_indices().nth(256).map_or(s.len(), |(i, _)| i);
                        String::from_utf8_lossy(&secrets.replace_cut_prefix(&s.as_bytes()[..end]))
                            .chars()
                            .take(256)
                            .collect()
                    } else {
                        secrets.replace(s).chars().take(256).collect()
                    };
                }
            }
        }
        self.update(|inner, _| {
            if let Some(n) = attempt {
                inner.enter_try(n).reports.push(value);
            } else if let Some(v) = inner
                .reports
                .iter_mut()
                .find(|r| r.layer == layer && r.value["stage"] == stage)
            {
                v.value = value;
            } else {
                inner.reports.push(LayerReport { layer, value });
            }
        });
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
            inner.last_upstream = Some(status);
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
            t.duration_ms
                .get_or_insert_with(|| t.started.elapsed().as_millis() as u64);
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
        self.decided_with_reroute(attempt, by, retry, None);
    }
    /// 决定与重跑请求链各取第一票，哪怕它们来自不同的程序也属于同一次尝试。
    pub(crate) fn decided_with_reroute(
        &self,
        attempt: u32,
        by: String,
        retry: bool,
        reroute: Option<bool>,
    ) {
        self.update(|inner, _| {
            if let Some(t) = inner.try_mut(attempt) {
                t.decision = Some(Decision {
                    by,
                    decision: if retry { "retry" } else { "stop" },
                    delay_ms: None,
                    backoff_started: None,
                    reroute,
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
                decision.backoff_started = (delay_ms > 0).then(Instant::now);
            }
            if delay_ms > 0 {
                inner.phase = Phase::Backoff;
                inner.next_attempt_at = Some(now_millis() + delay_ms);
            }
        });
    }

    /// 实际等待已结束，之后的请求改写、排队和上游调用都不属于 delay_ms。
    pub(crate) fn finish_backoff(&self) {
        self.update(|inner, _| inner.finish_backoff());
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

    /// 只记交付流的中断，不把重试时已丢弃的半截实体算进最终完成通知。
    pub(crate) fn stream_aborted(&self) {
        self.update(|inner, _| inner.stream_aborted = true);
    }

    /// 交付结束后取同一份已经定案的结果。通知不接触请求头、体或历史预算。
    pub(crate) fn completion(
        &self,
        delivered: bool,
        streamed: bool,
    ) -> Option<aproxy_envelope::Completion> {
        use aproxy_envelope::{Completion, Context, Delivery};
        let live = self.0.as_ref()?;
        let inner = lock(&live.record.inner);
        if matches!(
            inner.code,
            Some("request_too_large" | "client_body_aborted")
        ) {
            return None;
        }
        let end = inner.ended_at?;
        let mut m = Completion::new(
            Context::new(live.record.id.to_string()),
            if inner.delivery == "forward" {
                Delivery::Forward
            } else {
                Delivery::Retry
            },
            inner.attempts,
            end.saturating_sub(inner.started_at),
            inner.outcome.as_str().into(),
        );
        m.upgraded = inner.upgraded;
        m.upstream_status = inner.last_upstream;
        m.stream_completed =
            (streamed || inner.stream_aborted).then_some(delivered && !inner.stream_aborted);
        m.code = inner.code.map(str::to_owned);
        Some(m)
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
        inner.finish_backoff();
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

    /// 扩展方法和头值没有单字段长度限制，分别取 HTTP/1 读缓冲的上界；路径取 URI
    /// 解析器的上界，模型取默认请求体能容纳的最大 JSON 字符串。它们共享报头预算，故
    /// 在记录层组合各自的极值，比从单条连接构造边界更严格，不会改变真实 HTTP 请求。
    #[test]
    fn a_record_with_an_oversized_method_fits_the_limit() {
        const HEAD_LIMIT: usize = 8192 + 4096 * 100;
        const URI_LIMIT: usize = u16::MAX as usize - 1;
        let history = history(0, &[]);
        let method = http::Method::from_bytes(&vec![b'X'; HEAD_LIMIT]).unwrap();
        let uri = format!("/{}", "p".repeat(URI_LIMIT - 1))
            .parse::<http::Uri>()
            .unwrap();
        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::USER_AGENT,
            http::HeaderValue::from_bytes(&vec![b'a'; HEAD_LIMIT]).unwrap(),
        );
        let recorder = history.begin(
            1,
            id(1).into(),
            Start {
                method: method.as_str(),
                path_and_query: uri.path(),
                headers: &headers,
                delivery: "retry",
                phase: Phase::ReadingBody,
            },
        );
        let max_model = crate::config::body_limit_bytes(None) - br#"{"model":""}"#.len();
        recorder.request_shape(Some("m".repeat(max_model)), false);
        let list: Value = serde_json::from_str(&history.list_json(None, 1).unwrap()).unwrap();
        let record = &list["requests"][0];
        assert!(serde_json::to_vec(record).unwrap().len() <= MAX_RECORD_BYTES);
        assert_eq!(record["truncated"], true);
        assert_eq!(record["reports"], json!([]));
        assert_eq!(record["tries"], json!([]));
        for key in ["method", "path", "user_agent", "model"] {
            assert_eq!(
                record[key].as_str().unwrap().chars().count(),
                TRUNCATED_TEXT_CHARS,
                "{key}"
            );
        }
        assert_eq!(method.as_str().len(), HEAD_LIMIT);
    }

    /// 请求在后续层断开时，上一层的 retry 报告和尝试仍然可见；不能杜撰发出的请求。
    #[test]
    fn early_format_reports_survive_disconnect() {
        let h = history(1, &[]);
        let rec = begin(&h, 1);
        rec.format_report(
            (0, "inner"),
            "retry",
            Some(1),
            aproxy_envelope::Report::new("before disconnect".into()),
        );
        rec.end(false);
        let d: Value = serde_json::from_str(&h.detail_json(&id(1)).unwrap()).unwrap();
        assert_eq!(d["record"]["outcome"], "client_gone");
        assert_eq!(d["record"]["attempts"], 1);
        assert_eq!(
            d["record"]["tries"][0]["reports"][0]["summary"],
            "before disconnect"
        );
        assert_eq!(d["tries"][0]["sent"], Value::Null);
        assert_eq!(d["tries"][0]["response"], Value::Null);
    }

    /// 脱敏后的名称可以完全相同，替换报告只能覆盖同一个配置项。
    #[test]
    fn early_format_reports_keep_distinct_redacted_layers() {
        let h = history(0, &["abcdefgh"]);
        let rec = begin(&h, 1);
        for (layer, name, summary) in [
            (0, "layer-abcdefgh", "first"),
            (1, "layer-<secret>", "second"),
            (0, "layer-abcdefgh", "latest"),
        ] {
            rec.format_report(
                (layer, name),
                "request",
                None,
                aproxy_envelope::Report::new(summary.into()),
            );
        }
        let text = h.list_json(None, 1).unwrap();
        assert!(!text.contains("abcdefgh"));
        let doc: Value = serde_json::from_str(&text).unwrap();
        let reports = doc["requests"][0]["reports"].as_array().unwrap();
        assert_eq!(reports.len(), 2);
        assert_eq!(reports[0]["format"], reports[1]["format"]);
        assert_eq!(reports[0]["summary"], "latest");
        assert_eq!(reports[1]["summary"], "second");
    }

    /// 每一步能让记录放得下就停止，读记录也不能破坏后续读取看到的原始字段。
    #[test]
    fn record_trims_oldest_try_reports_then_client_text_then_top_reports() {
        let h = history(0, &[]);
        let path = format!("/{}", "p".repeat(500));
        let rec = begin_with(&h, 1, &path, &http::HeaderMap::new());
        let latest = || {
            let list: Value = serde_json::from_str(&h.list_json(None, 1).unwrap()).unwrap();
            let record = list["requests"][0].clone();
            assert!(record.to_string().len() <= MAX_RECORD_BYTES);
            record
        };
        let report = || aproxy_envelope::Report::new("done".into());
        rec.format_report((0, "outer"), "request", None, report());
        for n in 1..=2 {
            rec.try_started(n, None, true, None);
            rec.format_report((1, &"n".repeat(40_000)), "retry", Some(n), report());
        }
        let r = latest();
        assert_eq!(r["truncated"], true);
        assert_eq!(r["path"], path);
        assert_eq!(r["tries"][0]["reports"], json!([]));
        assert_eq!(r["tries"][1]["reports"].as_array().unwrap().len(), 1);
        rec.request_shape(Some("m".repeat(MAX_RECORD_BYTES)), false);
        let r = latest();
        assert_eq!(r["tries"][1]["reports"], json!([]));
        assert_eq!(r["model"].as_str().unwrap().len(), TRUNCATED_TEXT_CHARS);
        assert_eq!(r["reports"][0]["format"], "outer");
        rec.format_report(
            (1, &"n".repeat(MAX_RECORD_BYTES)),
            "response",
            None,
            report(),
        );
        let r = latest();
        assert_eq!(r["reports"], json!([]));
        assert_eq!(r["truncated"], true);
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

    /// 以 64 MiB 的预算推入 200 MiB 的体：任何时刻保留的合计都不超过预算，留下的是最新的
    /// 那一段，更旧的全部被淘汰。
    #[test]
    fn the_budget_holds_under_sustained_load() {
        let history = history(64, &[]);
        // 一百个请求共用同一块 2 MiB 的内存：记账按字节数算，测试本身不必真占 200 MiB
        let block = Bytes::from(vec![b'x'; (2 * MIB) as usize]);
        for seq in 1..=100 {
            let recorder = begin(&history, seq);
            let mut writer = recorder.request_body(None).unwrap();
            writer.feed(&block);
            writer.finish();
            recorder.end(true);
            assert!(history.body_bytes() <= 64 * MIB, "第 {seq} 个之后超了预算");
        }
        assert_eq!(history.body_bytes(), 64 * MIB);
        let states: Vec<String> = (1..=100)
            .map(|seq| {
                body_state(&history, seq)["state"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect();
        let kept = states.iter().filter(|s| *s == "retained").count();
        assert_eq!(kept, 32);
        assert!(states[..100 - kept].iter().all(|s| s == "evicted"));
        assert!(states[100 - kept..].iter().all(|s| s == "retained"));
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

    /// 借来的小块拷进自己的块里再入库：入库的块不指向原来的缓冲区，攒够了才成块。
    #[test]
    fn borrowed_chunks_are_copied_into_owned_blocks() {
        let history = history(1, &["sk-live-0123456789abcdef"]);
        let recorder = begin(&history, 1);
        let mut writer = recorder.request_body(None).unwrap();
        let buffer = Bytes::from(vec![b'e'; 8192]);
        let mut expected = Vec::new();
        for round in 0..40 {
            // 夹一个被切成两半的密钥
            let piece: &[u8] = match round {
                7 => b"sk-live-0123",
                8 => b"456789abcdef",
                _ => &buffer[..3000],
            };
            writer.feed_copy(piece);
            expected.extend_from_slice(piece);
        }
        let expected = Secrets::new(["sk-live-0123456789abcdef"]).replace_bytes(&expected);
        {
            let record = history.find(&id(1)).unwrap();
            let inner = lock(&record.inner);
            let body = lock(&inner.request_body.as_ref().unwrap().inner);
            // 40 次、共约 114 KiB：攒满 64 KiB 入库过一次，其余还攒着
            assert!(body.bytes >= COPY_BLOCK as u64 - 64 && body.bytes < 2 * COPY_BLOCK as u64);
            let range = buffer.as_ptr() as usize..buffer.as_ptr() as usize + buffer.len();
            for chunk in &body.chunks {
                assert!(!range.contains(&(chunk.as_ptr() as usize)));
            }
        }
        writer.finish();
        assert_eq!(
            read_all(&history, 1, Part::Request, 0, MAX_BODY_READ),
            expected
        );
        assert_eq!(history.body_bytes(), expected.len() as u64);
    }

    /// 填满的块在体被丢掉之后回到空闲表，下一个体用的就是同一块内存；空闲表有上限。
    #[test]
    fn full_blocks_are_recycled() {
        let history = history(2, &[]);
        assert_eq!(history.pool.keep, 4, "2 MiB 预算：空闲表最多留八分之一");
        let data = vec![b'p'; 6 * COPY_BLOCK + 100];
        let first = begin(&history, 1);
        let mut writer = first.request_body(None).unwrap();
        writer.feed_copy(&data);
        writer.finish();
        assert_eq!(history.body_bytes(), data.len() as u64);
        let blocks: Vec<usize> = {
            let record = history.find(&id(1)).unwrap();
            let inner = lock(&record.inner);
            let body = lock(&inner.request_body.as_ref().unwrap().inner);
            // 六个整块，加一个按实际大小另存的尾巴
            assert_eq!(body.chunks.len(), 7);
            assert_eq!(body.chunks[6].len(), 100);
            body.chunks[..6]
                .iter()
                .map(|c| {
                    assert_eq!(c.len(), COPY_BLOCK);
                    c.as_ptr() as usize
                })
                .collect()
        };
        assert_eq!(lock(&history.pool.free).len(), 1, "写者用完的那一块");

        // 记录没了：六个整块还回来，空闲表只留四块
        drop(first);
        lock(&history.records).clear();
        assert_eq!(history.body_bytes(), 0);
        let free: Vec<usize> = lock(&history.pool.free)
            .iter()
            .map(|b| b.as_ptr() as usize)
            .collect();
        assert_eq!(free.len(), 4);

        // 下一个体用的是空闲表里的块
        let second = begin(&history, 2);
        let mut writer = second.request_body(None).unwrap();
        writer.feed_copy(&data[..2 * COPY_BLOCK]);
        writer.finish();
        {
            let record = history.find(&id(2)).unwrap();
            let inner = lock(&record.inner);
            let body = lock(&inner.request_body.as_ref().unwrap().inner);
            assert_eq!(body.chunks.len(), 2);
            for chunk in &body.chunks {
                let at = chunk.as_ptr() as usize;
                assert!(free.contains(&at), "用的是空闲表里的块");
                assert!(blocks.contains(&at), "也就是上一个体用过的块");
            }
        }
        assert_eq!(
            read_all(&history, 2, Part::Request, 0, MAX_BODY_READ),
            &data[..2 * COPY_BLOCK]
        );
    }

    /// 大缓冲区入库不拷贝；体被丢掉之后它回到空闲表，下一个要的人拿到的是同一块内存。
    /// 别的大小的缓冲区不进空闲表。
    #[test]
    fn large_buffers_are_recycled() {
        let history = history(16, &[]);
        assert_eq!(history.buffers.keep, 2, "16 MiB 预算：空闲表最多留八分之一");
        let first = begin(&history, 1);
        let mut writer = first.request_body(None).unwrap();
        let mut buffer = writer.buffer(LARGE_BUFFER);
        assert_eq!((buffer.len(), buffer.capacity()), (0, LARGE_BUFFER));
        let at = buffer.as_ptr() as usize;
        buffer.resize(LARGE_BUFFER, b'h');
        writer.feed_buffer(buffer);
        // 不是这个大小的：现分配，入库之后也不回空闲表
        let mut small = writer.buffer(1000);
        assert_eq!(small.capacity(), 1000);
        small.resize(1000, b's');
        writer.feed_buffer(small);
        writer.finish();
        assert_eq!(history.body_bytes(), LARGE_BUFFER as u64 + 1000);
        {
            let record = history.find(&id(1)).unwrap();
            let inner = lock(&record.inner);
            let body = lock(&inner.request_body.as_ref().unwrap().inner);
            assert_eq!(
                body.chunks[0].as_ptr() as usize,
                at,
                "入库的就是交来的那一块"
            );
        }
        let all = read_all(&history, 1, Part::Request, 0, MAX_BODY_READ);
        assert_eq!(all.len(), LARGE_BUFFER + 1000);
        assert!(all[..LARGE_BUFFER].iter().all(|b| *b == b'h'));
        assert!(all[LARGE_BUFFER..].iter().all(|b| *b == b's'));
        assert!(lock(&history.buffers.free).is_empty());

        drop(first);
        lock(&history.records).clear();
        assert_eq!(history.body_bytes(), 0);
        assert_eq!(lock(&history.buffers.free).len(), 1);

        let second = begin(&history, 2);
        let writer = second.request_body(None).unwrap();
        let again = writer.buffer(LARGE_BUFFER);
        assert_eq!(again.as_ptr() as usize, at);
        assert_eq!((again.len(), again.capacity()), (0, LARGE_BUFFER));
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

    /// br 编码的体同样解码后保留，其中的密钥被替换，`encoding` 照记。
    #[test]
    fn brotli_bodies_are_decoded_too() {
        let history = history(1, &["sk-live-0123456789abcdef"]);
        let recorder = begin(&history, 1);
        recorder.try_started(1, None, true, None);
        let plain = br#"{"error":"bad key sk-live-0123456789abcdef"}"#;
        let mut encoder = brotli::CompressorWriter::new(Vec::new(), 4096, 5, 22);
        encoder.write_all(plain).unwrap();
        let packed = encoder.into_inner();
        let mut writer = recorder
            .response_head(1, 401, std::iter::empty(), Some("br"))
            .unwrap();
        writer.feed_copy(&packed);
        writer.finish();

        let stored = read_all(&history, 1, Part::Response, 1, 1024);
        assert_eq!(
            String::from_utf8_lossy(&stored),
            r#"{"error":"bad key <secret>"}"#
        );
        let detail: Value = serde_json::from_str(&history.detail_json(&id(1)).unwrap()).unwrap();
        assert_eq!(
            detail["tries"][0]["response"]["body"],
            json!({"bytes": stored.len(), "state": "retained", "encoding": "br"})
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

        let mut subscription = history.subscribe().unwrap();
        let events = &mut subscription.events;
        let in_flight: Value = serde_json::from_str(&subscription.in_flight).unwrap();
        assert_eq!(in_flight.as_array().unwrap().len(), 1);
        assert_eq!(in_flight[0]["request_seq"], 1);

        let recorder = begin(&history, 2);
        let seq_and_phase = |event: Event| match event {
            Event::Request { record: line, .. } => {
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
        // 订阅者有上限
        let more: Vec<Subscription> = (1..MAX_SUBSCRIBERS)
            .map(|_| history.subscribe().unwrap())
            .collect();
        assert_eq!(history.subscribers(), MAX_SUBSCRIBERS);
        assert!(history.subscribe().is_none());
        drop(more);
        assert!(history.subscribe().is_some(), "退订之后名额还回来");
        // 最后一个订阅者走了，补发任务自己结束
        drop(subscription);
        assert_eq!(history.subscribers(), 0);
        tokio::time::sleep(FLUSH_EVERY * 3).await;
        assert!(!history.flushing.load(Ordering::Acquire));
    }
}
