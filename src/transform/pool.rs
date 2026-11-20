//! format 协议的进程与管道。一份配置共享一个池，序号、读缓冲和槽位都跟随进程，
//! 不跟随请求；在写入和读完一帧之间取消任务，等价于丢掉这个会话。

use crate::config::file::{FormatEntry, Process};
use aproxy_envelope::{Hello, HelloReply, Message, Reply, Stage};
use aproxy_ipc::redact::Secrets;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{ChildStdin, ChildStdout, Command},
    sync::{OwnedSemaphorePermit, Semaphore},
    time::Instant,
};

mod notify;
/// 能逐请求改变的键在握手时逐项公布；未协商的键不会成为隐式配置入口。
const OVERRIDES: [&str; 8] = [
    "timeouts.connect_secs",
    "timeouts.first_byte_secs",
    "timeouts.read_idle_secs",
    "keepalive.interval_secs",
    "keepalive.trigger",
    "keepalive.heartbeat",
    "retry.max_backoff_secs",
    "retry.max_attempts",
];

/// 协商帧不带业务正文，普通控制帧只需容纳状态与报告。上限包括行末换行。
const HELLO_BYTES: usize = 64 * 1024;
/// 正文帧在两倍正文预算之外留给头、补丁、状态与 JSON 编码的空间。
const FRAME_OVERHEAD: usize = 1024 * 1024;
/// stderr 只保留一行的前缀；stdout 是协议，不能截断后继续解码。
const STDERR_BYTES: usize = 8 * 1024;
/// 同一程序的所有进程共享日志额度，不因池变大而放大日志流量。
const STDERR_LINES: u32 = 50;

/// 在每次从读缓冲取出字节时就检查剩余空间；没有换行的 EOF 是进程退出，不是帧。
async fn read_reply<R: tokio::io::AsyncBufRead + Unpin>(
    r: &mut R,
    limit: usize,
) -> Result<Vec<u8>, Fault> {
    let mut line = Vec::new();
    loop {
        let chunk = r.fill_buf().await.map_err(|_| Fault::Io)?;
        if chunk.is_empty() {
            return Err(Fault::Died);
        }
        let remaining = limit - line.len();
        let visible = &chunk[..chunk.len().min(remaining)];
        if let Some(i) = visible.iter().position(|&b| b == b'\n') {
            line.extend_from_slice(&visible[..=i]);
            r.consume(i + 1);
            return Ok(line);
        }
        if visible.len() == remaining {
            return Err(Fault::Invalid);
        }
        let n = visible.len();
        line.extend_from_slice(visible);
        r.consume(n);
    }
}

/// 每秒的日志额度与被抑制行数，同一程序的所有 worker 共用一个窗口。
struct LogWindow {
    /// 这一秒结束时，即使没有新输出也要汇总丢弃数。
    end: Instant,
    /// 已经写出的行数。
    shown: u32,
    /// 只计数，不保存限流后丢弃的内容。
    dropped: u64,
}
impl LogWindow {
    /// 开启首个固定时长窗口，随后仅在有输出时续开。
    fn new() -> Self {
        Self {
            end: Instant::now() + Duration::from_secs(1),
            shown: 0,
            dropped: 0,
        }
    }
    /// 窗口交界处可由定时器或下一行触发汇总，但只清零一次。
    fn report(&mut self, name: &str) {
        if self.dropped > 0 {
            tracing::warn!("format[{}] dropped {} stderr lines", name, self.dropped);
            self.dropped = 0;
        }
    }
    /// 只在首次丢弃时安排定时汇总，进程即使退出或不再输出仍能看见计数。
    fn allow(window: &Arc<Mutex<Self>>, name: &Arc<str>) -> bool {
        let mut w = window.lock().unwrap();
        if w.shown == 0 || Instant::now() >= w.end {
            w.report(name);
            *w = Self::new();
        }
        if w.shown < STDERR_LINES {
            w.shown += 1;
            return true;
        }
        w.dropped = w.dropped.saturating_add(1);
        if w.dropped == 1 {
            let window = window.clone();
            let name = name.clone();
            let end = w.end;
            tokio::spawn(async move {
                tokio::time::sleep_until(end).await;
                let mut w = window.lock().unwrap();
                if w.end == end {
                    w.report(&name);
                }
            });
        }
        false
    }
}

/// 边读边丢弃超长行的余部，保留有界前缀；截点若切进密钥，宁可多遮而不泄露前半段。
async fn log_stderr<R: tokio::io::AsyncRead + Unpin>(
    pipe: R,
    secrets: Secrets,
    name: Arc<str>,
    window: Arc<Mutex<LogWindow>>,
) {
    let mut r = BufReader::new(pipe);
    let mut line = Vec::new();
    let mut truncated = false;
    while let Ok(chunk) = r.fill_buf().await {
        let eof = chunk.is_empty();
        if eof && line.is_empty() && !truncated {
            break;
        }
        let newline = chunk.iter().position(|&b| b == b'\n');
        let n = newline.unwrap_or(chunk.len());
        let take = n.min(STDERR_BYTES - line.len());
        line.extend_from_slice(&chunk[..take]);
        truncated |= take < n;
        r.consume(n + usize::from(newline.is_some()));
        if eof || newline.is_some() {
            if LogWindow::allow(&window, &name) {
                let mut safe = if truncated {
                    secrets.replace_cut_prefix(&line)
                } else {
                    secrets.replace_bytes(&line)
                };
                // 输入截点可能刚好落在一个 UTF-8 字符中间；不要把这个人为的半字符记为乱码。
                if truncated && let Some(chunk) = safe.utf8_chunks().last() {
                    let suffix = chunk.invalid();
                    if std::str::from_utf8(suffix).is_err_and(|e| e.error_len().is_none()) {
                        safe.truncate(safe.len() - suffix.len());
                    }
                }
                let text = String::from_utf8_lossy(&safe);
                truncated |= text.len() > STDERR_BYTES;
                let mut end = text.len().min(STDERR_BYTES);
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                tracing::warn!(
                    "format[{}] {}{}",
                    name,
                    text[..end].trim_end_matches('\r'),
                    if truncated { "…(truncated)" } else { "" }
                );
            }
            line.clear();
            truncated = false;
        }
        if eof {
            break;
        }
    }
}

/// 不把外部程序输出的原文拼进错误：畸形 JSON 或错位帧也可能含凭据。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) enum Fault {
    /// 操作系统无法拉起程序，冷却后可以重试。
    Spawn,
    /// 管道写入或读取失败。
    Io,
    /// 对方提前关闭 stdout。
    Died,
    /// 从开始写出起，往返没有按时完成。
    Timeout,
    /// 版本、帧边界或序号不相符。
    Protocol,
    /// 收到匹配的帧，但它的内容不合法。
    Invalid,
    /// 握手成功但程序未声明这些配置阶段。
    Stages(Vec<String>),
    /// 熔断器拒绝派发，不算一次进程级失败；保留上次失败的分类与试探时刻。
    Open { kind: String, probe_at: u64 },
}
impl Fault {
    /// 错误分类在快照与请求历史里共用同一个词表。
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::Spawn => "spawn",
            Self::Io => "io",
            Self::Died => "died",
            Self::Timeout => "timeout",
            Self::Protocol | Self::Stages(_) => "protocol",
            Self::Invalid => "invalid_output",
            Self::Open { .. } => "open",
        }
    }
    /// 只有程序不可用才允许可选条目带着打开的熔断器启动。
    pub(crate) fn unavailable(&self) -> bool {
        matches!(self, Self::Spawn | Self::Io | Self::Died | Self::Timeout)
    }
}
impl std::fmt::Display for Fault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Open { .. } => "circuit open",
            Self::Spawn => "could not start the program",
            Self::Io => "worker pipe I/O failed",
            Self::Died => "worker exited before replying",
            Self::Timeout => "worker reply timed out",
            Self::Protocol => "invalid protocol version, framing or sequence number",
            Self::Invalid => "invalid worker reply",
            Self::Stages(_) => "worker does not support the configured stages",
        })
    }
}

/// 程序名只在 home/bin 与 PATH 中查，不让后台进程的当前目录意外参与解析。
fn program_path(
    command: &str,
    home: &Path,
    path: Option<&std::ffi::OsStr>,
) -> Result<PathBuf, Fault> {
    let expanded = crate::config::expand_tilde(command);
    let p = Path::new(&expanded);
    if p.file_name().is_none_or(|name| name != p.as_os_str()) {
        return Ok(home.join(p));
    }
    #[cfg(windows)]
    let exe = p.extension().is_none().then(|| p.with_extension("exe"));
    #[cfg(windows)]
    let p = exe.as_deref().unwrap_or(p);
    for dir in
        std::iter::once(home.join("bin")).chain(path.into_iter().flat_map(std::env::split_paths))
    {
        let file = dir.join(p);
        if let Ok(meta) = file.metadata()
            && meta.is_file()
        {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if meta.permissions().mode() & 0o111 == 0 {
                    continue;
                }
            }
            return Ok(file);
        }
    }
    Err(Fault::Spawn)
}

/// 直到操作系统确认进程退出才释放编号与进程名额；取消往返也不能造成两个活进程同号。
struct Slot {
    /// 发给该进程的永久 worker_id。
    id: u32,
    /// 这个程序仍在使用的所有编号。
    used: Arc<Mutex<BTreeMap<u32, Option<u32>>>>,
    /// 没有普通会话可归还时，最后一个退出的进程也要唤醒等空闲进程的观察者。
    parked: Option<Arc<tokio::sync::Notify>>,
    /// 进程数上限与并发请求数分别计数，前者包括正在退出的进程。
    capacity: Arc<Mutex<Option<OwnedSemaphorePermit>>>,
}
impl Drop for Slot {
    fn drop(&mut self) {
        self.used.lock().unwrap().remove(&self.id);
        self.capacity.lock().unwrap().take();
        if let Some(wake) = &self.parked {
            wake.notify_waiters();
        }
    }
}

/// stdin、stdout 与序号作为一组移动；读缓冲在同一进程的各帧之间保持不变。
struct Worker {
    /// 池进程占一个名额，独占流不占；进程所有者在 wait 完成后收回最后一个名额。
    capacity: Arc<Mutex<Option<OwnedSemaphorePermit>>>,
    /// 管道使用者与进程回收分开；即使管道被未完成的 Turn 借走，池关闭时仍能等到 wait。
    exited: tokio::task::JoinHandle<()>,
    /// 强制丢弃管道时立即停止日志读取，不让继承 stderr 的后代拖住它。
    stderr_abort: tokio::task::AbortHandle,
    /// 丢弃发送端会立即强制终止进程；正常退役先关 stdin 给程序宽限。
    kill: Option<tokio::sync::oneshot::Sender<()>>,
    /// 先关闭 stdin 通知进程退出，再收走进程名额。
    stdin: Option<ChildStdin>,
    /// 已预读的下一行不能交给另一个进程或请求。
    stdout: BufReader<ChildStdout>,
    /// 这一条管道实际协商到的能力，不借用另一个 worker 的握手结果。
    hello: Option<Arc<HelloReply>>,
    /// 握手用 0，其余消息沿用这个进程各自的序列。
    seq: u64,
    /// 异步 writer 是否已接受本帧的首字节；Windows 无法观察底层写入。等池位不计心跳故障。
    written: bool,
    /// 最后一次归还到池里的时刻。
    idle_since: Instant,
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stderr_abort.abort();
        self.kill.take();
        self.stdin.take();
    }
}
/// 正常退役的 EOF 宽限时间；并发等待而不是按进程逐个累加。
const RETIRE_GRACE: Duration = Duration::from_secs(2);
impl Worker {
    /// 无数据可读才复用；正常 EOF 使本次回信有效但不留进程，未请求的额外字节则使往返失败。
    fn reusable(&mut self) -> Result<bool, Fault> {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            if !self.stdout.buffer().is_empty() {
                return Err(Fault::Protocol);
            }
            // Unix 子管道本身是非阻塞的；进程已写出字节时，反应器的就绪缓存还可能没收过这次通知。
            let mut byte = 0u8;
            loop {
                // SAFETY: 管道由本 worker 独占；指针指向可写的一个字节，不保留其地址。
                let n = unsafe {
                    libc::read(
                        self.stdout.get_ref().as_raw_fd(),
                        (&mut byte as *mut u8).cast(),
                        1,
                    )
                };
                match n {
                    1 => return Err(Fault::Protocol),
                    0 => return Ok(false),
                    _ => match std::io::Error::last_os_error().kind() {
                        std::io::ErrorKind::Interrupted => continue,
                        std::io::ErrorKind::WouldBlock => return Ok(true),
                        _ => return Err(Fault::Io),
                    },
                }
            }
        }
        #[cfg(not(unix))]
        match futures_util::FutureExt::now_or_never(self.stdout.fill_buf()) {
            None => Ok(true),
            Some(Ok([])) => Ok(false),
            Some(Ok(_)) => Err(Fault::Protocol),
            Some(Err(_)) => Err(Fault::Io),
        }
    }
    /// 正常结束先发 EOF，给遵守协议的程序留出清理机会；不读 EOF 的进程仍能回收。
    async fn finish(mut self) {
        let _ = self.retire_pipe(false).await;
    }
    /// 一次性进程的最后回复不能带多余 stdout；给它 EOF 后在宽限期内读完管道。
    async fn final_stdout(&mut self, end: Instant) -> Result<(), Fault> {
        self.stdin.take();
        match tokio::time::timeout_at(end, self.stdout.fill_buf()).await {
            Ok(Ok([])) | Err(_) => Ok(()),
            Ok(Ok(_)) => Err(Fault::Protocol),
            Ok(Err(_)) => Err(Fault::Io),
        }
    }
    /// 超过从关 stdin 起算的宽限仍未退出才强退；系统 wait 之前不能交还进程名额。
    async fn reap_until(&mut self, end: Instant) {
        if tokio::time::timeout_at(end, &mut self.exited)
            .await
            .is_err()
        {
            self.kill.take();
            let _ = (&mut self.exited).await;
        }
    }
    /// 握手和普通消息在同一个宽限内检查 stdout 并回收；空闲退役不用核对最后的帧。
    async fn retire_pipe(&mut self, check_stdout: bool) -> Result<(), Fault> {
        let end = Instant::now() + RETIRE_GRACE;
        self.stdin.take();
        let checked = if check_stdout {
            self.final_stdout(end).await
        } else {
            Ok(())
        };
        if checked.is_err() {
            self.kill.take();
        }
        self.reap_until(end).await;
        checked
    }
    /// 排队与序列化不算在时限内，堵在写管道则算；调用被取消时不复用 worker。
    async fn frame(
        &mut self,
        stage: Stage,
        timeout: u64,
        admission: Option<&Admission<'_>>,
        limit: usize,
    ) -> Result<Vec<u8>, Fault> {
        let mut line = serde_json::to_vec(&Message::new(self.seq, stage))
            .expect("protocol messages serialize");
        line.push(b'\n');
        let io = async {
            let stdin = self.stdin.as_mut().unwrap();
            let written = if let Some(a) = admission {
                use tokio::io::AsyncWrite;
                let first = std::future::poll_fn(|cx| {
                    let c = a.pool.circuit.lock().unwrap();
                    if c.epoch != a.epoch {
                        return std::task::Poll::Ready(Err(a.refusal()));
                    }
                    std::pin::Pin::new(&mut *stdin)
                        .poll_write(cx, &line)
                        .map_err(|_| Fault::Io)
                });
                tokio::select! {biased; f=a.revoked()=>return Err(f), result=first=>result?}
            } else {
                0
            };
            if admission.is_some() && written == 0 {
                return Err(Fault::Io);
            }
            self.written = written != 0;
            stdin
                .write_all(&line[written..])
                .await
                .map_err(|_| Fault::Io)?;
            let reply = read_reply(&mut self.stdout, limit).await?;
            if self.stdout.buffer().is_empty() {
                Ok(reply)
            } else {
                Err(Fault::Protocol)
            }
        };
        if timeout == 0 {
            io.await
        } else {
            tokio::time::timeout(Duration::from_secs(timeout), io)
                .await
                .map_err(|_| Fault::Timeout)?
        }
    }
}

/// 冷却按单调时间执行，IPC 时间戳只用于说明；拒绝信息在一次打开后固定不变。
struct Cooldown {
    /// 本机经过的时间不受系统时钟校正影响。
    start: Instant,
    /// 快照里的 Unix 毫秒。
    opened_at: u64,
    /// 真正触发本次打开的故障分类，不随在途消息的结果而变。
    kind: &'static str,
    /// 快照显示的下次试探时间，不用来计算经过的时长。
    probe_at: u64,
}
impl Cooldown {
    /// 取出给请求的拒绝原因，与被本次打开撤销的旧票所见一致。
    fn refusal(&self) -> Fault {
        Fault::Open {
            kind: self.kind.into(),
            probe_at: self.probe_at,
        }
    }
}

/// 每次打开都推进代数，已写出的旧请求只能结束自己的往返，不能让熔断器越过冷却。
#[derive(Default)]
struct Circuit {
    /// 每次状态变化单独编号；订阅的初始快照可能已包含队列中的前几项。
    revision: u64,
    /// 当前冷却代；活跃消息在派发时记下它。
    epoch: u64,
    /// 进程级连续失败次数，合法 fail 回信也清零。
    failures: u32,
    /// 当前熔断窗口，合法试探完成后才清空。
    window: Option<Cooldown>,
    /// 打开时仅通知旧代的持票者；关闭不会唤醒旧队列。
    changed: tokio::sync::watch::Sender<Option<Fault>>,
    /// 冷却结束后，同一时刻只有一个数据消息在试探。
    probing: bool,
    /// 最近一次失败分类与时刻，不记录任意进程输出。
    last: Option<(Fault, u64)>,
}
impl Circuit {
    /// 两种熔断器在快照使用相同的计数、冷却和故障词表。
    fn view(&self) -> serde_json::Value {
        use serde_json::json;
        let mut breaker = json!({"state":if self.probing{"half_open"}else if self.window.is_some(){"open"}else{"closed"},"failures":self.failures});
        if let Some(window) = &self.window {
            breaker["opened_at"] = json!(window.opened_at);
            breaker["probe_at"] = json!(window.probe_at);
        }
        if let Some((f, at)) = &self.last {
            breaker["last_failure"] =
                json!({"code":"format_error","details":{"kind":f.kind()},"at":at});
        }
        breaker
    }
}
/// 客户端取消时释放试探资格，不计故障，也不让半开的状态永远卡住。
struct Admission<'a> {
    /// 这次派发所属的池。
    pool: &'a Pool,
    /// 只改变自己所属代的状态。
    epoch: u64,
    /// 即使后面又开闭过几次，这里收到的仍是令本票作废的那次打开。
    changed: tokio::sync::watch::Receiver<Option<Fault>>,
    /// 普通消息不用归还试探资格。
    probe: bool,
}
impl Admission<'_> {
    /// 只在锁内已见到代号不符时读取；打开先写原因再释放锁。
    fn refusal(&self) -> Fault {
        self.changed.borrow().clone().expect("revoked generation")
    }
    /// 排队或等待首次写入时及时退出，不让过期票变成冷却后的试探。
    async fn revoked(&self) -> Fault {
        self.changed
            .clone()
            .wait_for(Option::is_some)
            .await
            .expect("admission holds pool")
            .clone()
            .expect("revoked generation")
    }
}
impl Drop for Admission<'_> {
    fn drop(&mut self) {
        if self.probe {
            let mut c = self.pool.circuit.lock().unwrap();
            if c.epoch == self.epoch && c.probing {
                c.probing = false;
                self.pool.transition(&mut c);
            }
        }
    }
}
/// 每个配置条目独享一个进程池，空闲池之外的正在退出的进程也占进程名额。
pub(crate) struct Pool {
    /// 配置顺序由外层持有，条目本身在一次运行中不变。
    pub(crate) entry: FormatEntry,
    /// home 固定，不依赖每次调用时的进程工作目录。
    home: PathBuf,
    /// stderr 在写日志之前走与请求历史相同的替换。
    secrets: Secrets,
    /// 请求、响应的单帧上限；usize::MAX 对应用户明确选择的不设限。
    frame_limits: [usize; 2],
    /// 每个程序一个日志窗口，跨并发、换进程与重新握手。
    stderr_window: Arc<Mutex<LogWindow>>,
    /// 同时进池的请求数；空闲进程可由持票者直接取走。
    turns: Semaphore,
    /// 归还的流可能拿走空闲名额，唤醒同时等待名额的普通请求重新检查池。
    parked: Arc<tokio::sync::Notify>,
    /// 已启动但仍未退出的普通进程数上限，不包括独占流。
    capacity: Arc<Semaphore>,
    /// 开始排空后不再留空闲会话，仍在途的请求可完成后续阶段。
    draining: std::sync::atomic::AtomicBool,
    /// 关闭信号也送给已借出和正在握手的进程，与持有管道的任务是否继续运行无关。
    stopping: tokio::sync::watch::Sender<bool>,
    /// 协议编号映射到未确认退出的 OS PID；拉起前与退出后的日志收尾期保留编号，但没有 PID。
    ids: Arc<Mutex<BTreeMap<u32, Option<u32>>>>,
    /// 仅在收发全部完成后归还，这把同步锁不跨 await。
    idle: Mutex<Vec<Worker>>,
    /// 回收任务只持弱引用，实例退出不被它阻挡。
    reaper: OnceLock<tokio::task::JoinHandle<()>>,
    /// 同一池只探测一次；同时询问的调用方共享第一次握手的结果。
    startup: tokio::sync::OnceCell<Result<(), Fault>>,
    /// 因空闲正常退役的进程在后台限时等待 EOF；实例关闭时一起等完。
    retiring: Mutex<tokio::task::JoinSet<()>>,
    /// 首次 poll_write 与打开熔断器在同一锁内排序；从不持锁跨越 await。
    circuit: Mutex<Circuit>,
    /// 只有声明 notify 的程序才分配观察队列与独立的故障计数。
    notices: Option<Mutex<notify::Lane>>,
    /// 上报状态的配置序号和有界事件队列；不用脱敏后的名字作为身份。
    events: Option<(usize, std::sync::Weak<crate::history::History>)>,
    /// 最近一次成功握手的身份与能力，握手失败时不杜撰版本。
    pub(crate) negotiated: Mutex<Option<Arc<HelloReply>>>,
}
impl Drop for Pool {
    fn drop(&mut self) {
        if let Some(t) = self.reaper.take() {
            t.abort();
        }
    }
}
impl Pool {
    /// 这里只分配管理结构；进程由异步启动探测或阶段入口创建。
    pub(crate) fn new(
        entry: FormatEntry,
        home: PathBuf,
        secrets: Secrets,
        events: Option<(usize, std::sync::Weak<crate::history::History>)>,
        body_limits: [usize; 2],
    ) -> Arc<Self> {
        let n = if entry.process == Process::Spawn {
            1
        } else {
            entry.pool_max as usize
        };
        Arc::new(Self {
            notices: entry
                .has_stage(crate::config::file::Stage::Notify)
                .then(Default::default),
            entry,
            home,
            secrets,
            frame_limits: body_limits.map(|n| n.saturating_mul(2).saturating_add(FRAME_OVERHEAD)),
            stderr_window: Arc::new(Mutex::new(LogWindow::new())),
            turns: Semaphore::new(n),
            parked: Default::default(),
            capacity: Arc::new(Semaphore::new(n)),
            draining: Default::default(),
            stopping: tokio::sync::watch::channel(false).0,
            ids: Default::default(),
            idle: Default::default(),
            reaper: OnceLock::new(),
            startup: Default::default(),
            retiring: Default::default(),
            events,
            negotiated: Mutex::new(None),
            circuit: Default::default(),
        })
    }
    /// 快照不曝光命令行、环境、回信中的任意文本；版本与配置名也替换已知凭据。
    pub(crate) fn snapshot(&self) -> (u64, serde_json::Value) {
        let c = self.circuit.lock().unwrap();
        (c.revision, self.circuit_snapshot(&c))
    }
    /// 在修改者仍持有熔断锁时取值；发到队列不等待 IPC 写出，也不锁其他进程池。
    fn transition(&self, c: &mut Circuit) {
        c.revision += 1;
        if let Some((index, history)) = &self.events
            && let Some(history) = history.upgrade()
        {
            history.format_transition(*index, c.revision, || self.circuit_snapshot(c));
        }
    }
    /// 读者和状态事件共用同一份字段选择和脱敏逻辑。
    fn circuit_snapshot(&self, c: &Circuit) -> serde_json::Value {
        use serde_json::json;
        let breaker = c.view();
        let mut snap = json!({"name":self.secrets.replace(&self.entry.name),"process":if self.entry.process==Process::Spawn{"spawn"}else{"persistent"},"required":self.entry.required,"stages":self.entry.stages.iter().map(|s|s.as_str()).collect::<Vec<_>>(),"breaker":breaker});
        if let Some(n) = &self.notices {
            let lane = n.lock().unwrap();
            snap["notify_breaker"] = lane.breaker.view();
            snap["notify_dropped_total"] = json!(lane.dropped);
        }
        if let Some(h) = self.negotiated.lock().unwrap().as_ref() {
            snap["protocol"] = json!(h.protocol);
            snap["version"] = json!(self.secrets.replace(&h.version));
        }
        snap
    }
    /// 记录冷却并清空空闲进程；与试探、已排队消息的判定共用同一把锁。
    fn open(&self, c: &mut Circuit, now: u64) {
        c.epoch = c.epoch.wrapping_add(1);
        let window = Cooldown {
            start: Instant::now(),
            opened_at: now,
            kind: c.last.as_ref().expect("opening fault").0.kind(),
            probe_at: now.saturating_add(self.entry.breaker_cooldown_secs.saturating_mul(1000)),
        };
        std::mem::take(&mut c.changed).send_replace(Some(window.refusal()));
        c.window = Some(window);
        c.probing = false;
        self.idle.lock().unwrap().clear();
        if let Some(n) = &self.notices {
            let mut lane = n.lock().unwrap();
            lane.breaker.probing = false;
            lane.discard_all(self);
        }
        self.transition(c);
        tracing::warn!(format=%self.secrets.replace(&self.entry.name),failures=c.failures,cooldown_secs=self.entry.breaker_cooldown_secs,kind=c.last.as_ref().map(|(f,_)|f.kind()),"format circuit opened");
    }
    /// 可选程序启动时暂不可用，与静态不匹配不同；冷却后允许一次真正的数据往返。
    pub(crate) fn open_at_startup(&self, f: Fault) {
        if self.entry.stages.as_slice() == [crate::config::file::Stage::Notify] {
            self.notify_failure_at_start(f);
            return;
        }
        let mut c = self.circuit.lock().unwrap();
        let now = crate::daemon::now_millis();
        c.failures = 1;
        c.last = Some((f, now));
        if self.entry.breaker_failures != 0 {
            self.open(&mut c, now);
        } else {
            tracing::warn!(format=%self.secrets.replace(&self.entry.name),kind=c.last.as_ref().expect("startup fault").0.kind(),"optional format unavailable at startup");
        }
    }
    /// 准备请求表示时只查看，不抢独占试探资格。已取到自己进程的一层直接使用实际握手。
    pub(crate) fn available_for_message(&self) -> bool {
        let c = self.circuit.lock().unwrap();
        !c.probing
            && c.window.as_ref().is_none_or(|w| {
                w.start.elapsed() >= Duration::from_secs(self.entry.breaker_cooldown_secs)
            })
    }
    /// 打开的程序不排队。冷却后的首条消息抢到独占试探，即使用 persistent 也重新握手。
    fn admit(&self) -> Result<Admission<'_>, Fault> {
        let mut c = self.circuit.lock().unwrap();
        let probe = if let Some(window) = &c.window {
            if c.probing
                || window.start.elapsed() < Duration::from_secs(self.entry.breaker_cooldown_secs)
            {
                return Err(window.refusal());
            }
            c.probing = true;
            self.transition(&mut c);
            tracing::info!(format=%self.secrets.replace(&self.entry.name),"format circuit half-open");
            true
        } else {
            false
        };
        Ok(Admission {
            pool: self,
            epoch: c.epoch,
            changed: c.changed.subscribe(),
            probe,
        })
    }
    /// 每个已写出的消息按完成顺序登记；程序的显式拒绝是合法回信，不是进程故障。
    fn complete(&self, a: &Admission<'_>, fault: Option<&Fault>) {
        self.complete_at(a.epoch, a.probe, fault);
    }
    /// 流开始后的事件仍更新故障计数，但只有被选中的 open 才能完成半开试探。
    fn complete_at(&self, epoch: u64, probe: bool, fault: Option<&Fault>) {
        let mut c = self.circuit.lock().unwrap();
        let current_probe = probe && c.epoch == epoch;
        if let Some(f) = fault {
            let now = crate::daemon::now_millis();
            c.failures = c.failures.saturating_add(1);
            c.last = Some((f.clone(), now));
            if current_probe
                || (c.window.is_none()
                    && self.entry.breaker_failures != 0
                    && c.failures >= self.entry.breaker_failures)
            {
                self.open(&mut c, now);
            }
        } else {
            c.failures = 0;
            if current_probe {
                c.window = None;
                c.probing = false;
                self.transition(&mut c);
                tracing::info!(format=%self.secrets.replace(&self.entry.name),"format circuit closed");
            }
        }
    }
    /// 建会话、分配最小空闲编号。每次都重新查找命令，供冷却后的程序恢复。
    async fn spawn(&self, admission: Option<&Admission<'_>>) -> Result<Worker, Fault> {
        let permit = self
            .capacity
            .clone()
            .acquire_owned()
            .await
            .expect("pool is never closed");
        self.spawn_with(admission, Some(permit)).await
    }
    /// 有普通名额或是独占流时才创建进程；编号和关闭通知覆盖两种会话。
    async fn spawn_with(
        &self,
        admission: Option<&Admission<'_>>,
        permit: Option<OwnedSemaphorePermit>,
    ) -> Result<Worker, Fault> {
        let mut stopping = self.stopping.subscribe();
        if *stopping.borrow() {
            return Err(Fault::Died);
        }
        let env_path = self.entry.env.iter().rev().find(|(key, _)| {
            if cfg!(windows) {
                key.eq_ignore_ascii_case("PATH")
            } else {
                key.as_str() == "PATH"
            }
        });
        let inherited = std::env::var_os("PATH");
        let exe = program_path(
            &self.entry.command,
            &self.home,
            env_path
                .map(|(_, v)| std::ffi::OsStr::new(v))
                .or(inherited.as_deref()),
        )?;
        let id = {
            let mut ids = self.ids.lock().unwrap();
            let id = (0..)
                .find(|id| !ids.contains_key(id))
                .expect("worker count fits u32");
            ids.insert(id, None);
            id
        };
        let capacity = Arc::new(Mutex::new(permit));
        let slot = Slot {
            id,
            used: self.ids.clone(),
            parked: self.notices.as_ref().map(|_| self.parked.clone()),
            capacity: capacity.clone(),
        };
        let mut child = {
            let c = self.circuit.lock().unwrap();
            if let Some(a) = admission
                && a.epoch != c.epoch
            {
                return Err(a.refusal());
            }
            Command::new(exe)
                .args(&self.entry.args)
                .envs(&self.entry.env)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .map_err(|_| Fault::Spawn)?
        };
        *slot.used.lock().unwrap().get_mut(&id).unwrap() = Some(child.id().expect("fresh child"));
        let stderr = tokio::spawn(log_stderr(
            child.stderr.take().unwrap(),
            self.secrets.clone(),
            self.secrets.replace(&self.entry.name).into(),
            self.stderr_window.clone(),
        ));
        let stdin = child.stdin.take();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let stderr_abort = stderr.abort_handle();
        let (kill, force) = tokio::sync::oneshot::channel();
        // 只有此任务持有 OS 句柄和槽位。交还全部槽位因此也证明所有子进程都已 wait，
        // 不需要按 pid 寻找进程，更不依赖即将强退的服务端执行各请求的析构函数。
        let exited = tokio::spawn(async move {
            let normal = tokio::select! {
                result = child.wait() => result.is_ok(),
                _ = force => false,
                _ = stopping.wait_for(|stop| *stop) => false,
            };
            if !normal {
                let _ = child.start_kill();
                // wait 出错不能当作已退出；始终只用原句柄等待，截止点由整个实例共用。
                while child.wait().await.is_err() {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
            *slot.used.lock().unwrap().get_mut(&id).unwrap() = None;
            let mut stderr = stderr;
            if normal {
                let _ = tokio::time::timeout(Duration::from_millis(100), &mut stderr).await;
            }
            stderr.abort();
            drop(slot);
            drop(stopping);
        });
        let mut w = Worker {
            capacity,
            exited,
            stderr_abort,
            kill: Some(kill),
            stdin,
            stdout,
            hello: None,
            seq: 0,
            written: false,
            idle_since: Instant::now(),
        };
        self.handshake(&mut w, id, admission).await?;
        Ok(w)
    }
    /// 拉起和握手之间别的进程仍能令本票作废；hello 与业务帧共用首次写入的原子检查。
    async fn handshake(
        &self,
        w: &mut Worker,
        id: u32,
        admission: Option<&Admission<'_>>,
    ) -> Result<(), Fault> {
        let mut hello = Hello::new(
            env!("CARGO_PKG_VERSION").into(),
            self.entry.name.clone(),
            match self.entry.process {
                Process::Persistent => aproxy_envelope::Process::Persistent,
                Process::Spawn => aproxy_envelope::Process::Spawn,
            },
            self.entry
                .stages
                .iter()
                .map(|s| s.as_str().into())
                .collect(),
            id,
        );
        hello.features.push("decide.success".into());
        hello.overrides = OVERRIDES.iter().map(|k| (*k).into()).collect();
        let line = w
            .frame(Stage::Hello(hello), 10, admission, HELLO_BYTES)
            .await?;
        let object: serde_json::Map<String, serde_json::Value> =
            serde_json::from_slice(&line).map_err(|_| Fault::Invalid)?;
        if object.get("seq").and_then(|v| v.as_u64()) != Some(0) {
            return Err(Fault::Protocol);
        }
        let reply = HelloReply::try_from(object).map_err(|_| Fault::Invalid)?;
        if reply.v != 1 || reply.protocol != 1 || reply.seq != 0 {
            return Err(Fault::Protocol);
        }
        let missing: Vec<_> = self
            .entry
            .stages
            .iter()
            .filter(|s| !reply.stages.iter().any(|n| n == s.as_str()))
            .map(|s| s.as_str().into())
            .collect();
        if !missing.is_empty() {
            return Err(Fault::Stages(missing));
        }
        // 握手成功只建立会话；只有合法的业务阶段回信才清零连续失败数。
        let hello = Arc::new(reply);
        w.hello = Some(hello.clone());
        *self.negotiated.lock().unwrap() = Some(hello);
        w.seq = 1;
        Ok(())
    }
    /// 探测 persistent 的首个进程留池；spawn 进程在握手后单独关闭。
    pub(crate) async fn probe(self: &Arc<Self>) -> Result<(), Fault> {
        self.startup
            .get_or_init(|| async {
                let mut w = self.spawn(None).await?;
                if self.entry.process == Process::Spawn {
                    w.retire_pipe(true).await?;
                } else if w.reusable()? {
                    self.park(w);
                } else {
                    w.finish().await;
                }
                Ok(())
            })
            .await
            .clone()
    }
    /// 已完成的退役任务及时摘除；在清理宽限期内，进程仍占容量但不占数据通路的锁。
    fn retire(&self, w: Worker) {
        let mut tasks = self.retiring.lock().unwrap();
        while tasks.try_join_next().is_some() {}
        tasks.spawn(w.finish());
    }
    /// 停机开始就给空闲会话 EOF，不消耗在途请求的排空期限；随后归还的进程也直接退役。
    pub(crate) fn begin_drain(&self) {
        let mut idle = self.idle.lock().unwrap();
        self.draining
            .store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(t) = self.reaper.get() {
            t.abort();
        }
        for w in idle.drain(..) {
            self.retire(w);
        }
    }
    /// 截止点到了不再接收工作；直接唤醒持有 OS 句柄的任务，排空已经给过空闲进程宽限。
    pub(crate) fn terminate_remaining(&self) {
        self.turns.close();
        self.stopping.send_replace(true);
    }
    /// 截止后只报告仍未 wait 成功的 OS PID；协议编号与 PID 不同，日志也不用于事后杀进程。
    pub(crate) async fn confirm_exits_until(&self, deadline: Instant) {
        if tokio::time::timeout_at(deadline, self.stopping.closed())
            .await
            .is_ok()
        {
            return;
        }
        for pid in self.ids.lock().unwrap().values().flatten() {
            tracing::warn!(format = %self.secrets.replace(&self.entry.name), pid, "format worker exit unconfirmed at shutdown deadline");
        }
    }
    /// 无在途请求时优先等空闲进程清理；已借出的会话不再等待回答。
    pub(crate) async fn shutdown(&self) {
        self.turns.close();
        self.begin_drain();
        let mut retired = std::mem::take(&mut *self.retiring.lock().unwrap());
        // 健康的空闲进程先有 EOF 宽限；所有者无法确认 OS 退出时，不能让握手失败或 stop 卡在这里。
        let _ = tokio::time::timeout(RETIRE_GRACE, async {
            while retired.join_next().await.is_some() {}
        })
        .await;
        self.terminate_remaining();
        self.confirm_exits_until(Instant::now() + Duration::from_secs(1))
            .await;
    }
    /// 只有刚检查过输出的健康会话可以归还；第二次检查放在取用一刻，截住迟到的杂字节。
    fn park(self: &Arc<Self>, mut w: Worker) {
        w.idle_since = Instant::now();
        {
            let mut idle = self.idle.lock().unwrap();
            if *self.stopping.borrow() {
                return;
            }
            if self.draining.load(std::sync::atomic::Ordering::Relaxed) {
                self.retire(w);
                return;
            }
            idle.push(w);
            self.parked.notify_waiters();
        }
        let timeout = Duration::from_secs(self.entry.worker_idle_secs);
        if timeout.is_zero() {
            return;
        }
        self.reaper.get_or_init(|| {
            let pool = Arc::downgrade(self);
            tokio::spawn(async move {
                loop {
                    let Some(p) = pool.upgrade() else { break };
                    let delay = {
                        let mut idle = p.idle.lock().unwrap();
                        if p.draining.load(std::sync::atomic::Ordering::Relaxed) {
                            break;
                        }
                        let mut i = 0;
                        while i < idle.len() {
                            if idle[i].idle_since.elapsed() >= timeout {
                                p.retire(idle.swap_remove(i));
                            } else {
                                i += 1;
                            }
                        }
                        idle.iter()
                            .map(|w| timeout.saturating_sub(w.idle_since.elapsed()))
                            .min()
                            .unwrap_or(timeout)
                    };
                    drop(p);
                    tokio::time::sleep(delay).await;
                }
            })
        });
    }
    /// 空闲进程若已退出，或空闲过久，直接回收；下一次消息使用另一条会话。
    fn take(&self) -> Option<Worker> {
        let mut idle = self.idle.lock().unwrap();
        while let Some(mut w) = idle.pop() {
            if w.exited.is_finished() {
                continue;
            }
            if self.entry.worker_idle_secs != 0
                && w.idle_since.elapsed() >= Duration::from_secs(self.entry.worker_idle_secs)
            {
                self.retire(w);
            } else {
                match w.reusable() {
                    Ok(true) => return Some(w),
                    Ok(false) => self.retire(w),
                    Err(_) => (),
                }
            }
        }
        None
    }
    /// 先拿到已握手的独占进程，再让上一层读取磁盘与构造业务帧；等待者只持有轻量上下文。
    pub(crate) async fn turn(self: &Arc<Self>, fresh: bool) -> Result<Turn<'_>, Fault> {
        let waiting = self.waiting_data();
        let ticket = self.admit()?;
        let _turn = tokio::select! {biased;
            f=ticket.revoked()=>return Err(f),
            turn=self.turns.acquire()=>turn.map_err(|_| Fault::Died)?,
        };
        let borrow = async {
            loop {
                let parked = self.parked.notified();
                let old = self.take();
                if fresh || ticket.probe {
                    drop(old);
                } else if let Some(w) = old {
                    return Ok(w);
                }
                tokio::select! {biased;
                    _=parked=>continue,
                    permit=self.capacity.clone().acquire_owned()=> return self.spawn_with(Some(&ticket), Some(permit.expect("pool is never closed"))).await,
                }
            }
        };
        let worker = tokio::select! {biased;
            f=ticket.revoked()=>return Err(f),
            result=borrow=>result,
        };
        drop(waiting);
        match worker {
            Ok(worker) => Ok(Turn {
                pool: self,
                ticket,
                worker,
                _turn,
            }),
            Err(f) => {
                if !matches!(f, Fault::Open { .. }) {
                    self.complete(&ticket, Some(&f));
                }
                Err(f)
            }
        }
    }
    /// open 才查熔断门。借走首个已握手的进程即释放其普通名额；没有空闲进程时直接为流建会话。
    pub(crate) async fn open_stream(
        self: &Arc<Self>,
        stage: Stage,
        fresh: bool,
    ) -> Result<(StreamWorker, PreparedReply), Fault> {
        let waiting = self.waiting_data();
        let ticket = self.admit()?;
        let obtain = async {
            if let Some(w) = self.take()
                && !fresh
                && !ticket.probe
            {
                w.capacity.lock().unwrap().take();
                return Ok(w);
            }
            self.spawn_with(Some(&ticket), None).await
        };
        let result = async {
            let mut w = tokio::select! {biased; f=ticket.revoked()=>return Err(f), r=obtain=>r?};
            drop(waiting);
            let reply = self
                .exchange(&mut w, stage, self.entry.timeout_secs, Some(&ticket))
                .await?;
            w.reusable()?;
            Ok((
                StreamWorker {
                    pool: self.clone(),
                    worker: w,
                    epoch: ticket.epoch,
                },
                reply,
            ))
        }
        .await;
        if !matches!(result, Err(Fault::Open { .. })) {
            self.complete(&ticket, result.as_ref().err());
        }
        result
    }
    /// 管道单元测试只传很小的合成帧，生产路径先取得 turn 再构造可能很大的消息。
    #[cfg(test)]
    async fn call(
        self: &Arc<Self>,
        stage: Stage,
        timeout: u64,
        fresh: bool,
    ) -> Result<Reply, Fault> {
        Ok(self.turn(fresh).await?.send(stage, timeout).await?.0)
    }
    /// 帧的首次写入与熔断按同一把锁排序；已经发出的消息读完并计数，但不会提前结束冷却。
    async fn exchange(
        &self,
        w: &mut Worker,
        stage: Stage,
        timeout: u64,
        a: Option<&Admission<'_>>,
    ) -> Result<PreparedReply, Fault> {
        let limit = match &stage {
            Stage::Request(_) | Stage::Retry(_) => self.frame_limits[0],
            Stage::Response(_) | Stage::Stream(_) => self.frame_limits[1],
            _ => FRAME_OVERHEAD,
        };
        let may_choose_delivery = matches!(&stage, Stage::Request(_));
        let request_stage = matches!(&stage, Stage::Request(_) | Stage::Retry(_));
        let line = w.frame(stage, timeout, a, limit).await?;
        /// 先核对回信的版本和序列再解释补丁，以免把其他请求的回信应用到本次。
        #[derive(serde::Deserialize)]
        struct Sequence {
            /// 固定的协议版本。
            v: u32,
            /// 本进程本次派发的序号。
            seq: u64,
        }
        let stamp: Sequence = serde_json::from_slice(&line).map_err(|_| Fault::Protocol)?;
        if stamp.v != 1 || stamp.seq != w.seq {
            return Err(Fault::Protocol);
        }
        let r: Reply = serde_json::from_slice(&line).map_err(|_| Fault::Invalid)?;
        if r.request
            .as_ref()
            .and_then(|p| p.method.as_ref())
            .is_some_and(|m| http::Method::from_bytes(m.as_bytes()).is_err())
            || r.response
                .as_ref()
                .and_then(|p| p.status)
                .into_iter()
                .chain(
                    r.fail
                        .as_ref()
                        .and_then(|f| f.response.as_ref())
                        .map(|p| p.status),
                )
                .any(|s| !(100..=599).contains(&s))
            || r.state.as_ref().is_some_and(|s| s.len() > 64 * 1024)
            || r.request
                .as_ref()
                .and_then(|p| p.url.as_ref())
                .is_some_and(|u| {
                    reqwest::Url::parse(u).map_or(true, |u| {
                        !matches!(u.scheme(), "http" | "https") || u.host().is_none()
                    })
                })
        {
            return Err(Fault::Invalid);
        }
        if !may_choose_delivery && r.delivery.is_some() {
            tracing::warn!(format=%self.secrets.replace(&self.entry.name),"format delivery outside request ignored");
        }
        if r.fail.as_ref().is_some_and(|f| f.discarded_code) {
            tracing::warn!(format=%self.secrets.replace(&self.entry.name),"invalid format fail code ignored");
        }
        match &r.report {
            aproxy_envelope::ReportField::Invalid => {
                tracing::warn!(format=%self.secrets.replace(&self.entry.name),"invalid format report ignored")
            }
            aproxy_envelope::ReportField::Present(report) if report.discarded_actions != 0 => {
                tracing::warn!(format=%self.secrets.replace(&self.entry.name),count=report.discarded_actions,"invalid format report actions ignored")
            }
            _ => (),
        }
        // 只解析这一阶段实际采用的补丁；优先级和数据面相同，fail 覆盖同帧的其他补丁。
        let table = if let Some(fail) = &r.fail {
            fail.response.as_ref().map(|r| &r.headers)
        } else if request_stage {
            r.request.as_ref().and_then(|p| p.headers.as_ref())
        } else {
            r.response.as_ref().and_then(|p| p.headers.as_ref())
        };
        let headers = table
            .map(|h| super::headers_from_btreemap(h, 8))
            .transpose()?;
        w.seq += 1;
        Ok((r, headers))
    }
}

/// 从 open 至 close/abort 都留在一个进程；不会把另一条流的序号或状态带进来。
pub(crate) struct StreamWorker {
    /// 进程回收与按程序计数的归属。
    pool: Arc<Pool>,
    /// 只在完整收尾后允许归还的管道。
    worker: Worker,
    /// 开始流时所处的代，进行中的流不受后续熔断的开关影响。
    epoch: u64,
}
impl StreamWorker {
    /// 已接纳的流不经过普通队列，也不因别的流触发熔断而拒绝收尾。
    pub(crate) async fn event(&mut self, stage: Stage) -> Result<PreparedReply, Fault> {
        let result = self
            .pool
            .exchange(&mut self.worker, stage, self.pool.entry.timeout_secs, None)
            .await
            .and_then(|r| {
                self.worker.reusable()?;
                Ok(r)
            });
        self.pool
            .complete_at(self.epoch, false, result.as_ref().err());
        result
    }
    /// 健康会话按空闲名额数归还，不按编号截断；池满则关 stdin 正常退役。
    pub(crate) fn finish(self) {
        let Self {
            pool, mut worker, ..
        } = self;
        if matches!(worker.reusable(), Ok(true))
            && let Ok(permit) = pool.capacity.clone().try_acquire_owned()
        {
            *worker.capacity.lock().unwrap() = Some(permit);
            pool.park(worker);
        } else {
            pool.retire(worker);
        }
    }
}

/// 回信与通过容量校验的 HTTP 头表不可分开；交付终态不再重新解析头表。
type PreparedReply = (Reply, Option<http::HeaderMap>);

/// 已获接纳的一次往返；取消或未发出就丢弃时，进程与试探资格一起回收。
pub(crate) struct Turn<'a> {
    /// 归还 worker 和报告故障的原池。
    pool: &'a Arc<Pool>,
    /// 首字节仍须通过同代熔断器检查。
    ticket: Admission<'a>,
    /// 本次独占的会话。
    worker: Worker,
    /// 从接纳到归还始终占一个并发名额。
    _turn: tokio::sync::SemaphorePermit<'a>,
}
impl Turn<'_> {
    /// 借出的进程已经握手，按这个进程的实际能力决定要不要发送消息。
    pub(crate) fn hello(&self) -> &HelloReply {
        self.worker
            .hello
            .as_deref()
            .expect("worker has completed hello")
    }
    /// 本地没有消息可发时不惩罚健康进程，已经打开的旧代仍不得归还。
    pub(crate) async fn unused(self) {
        if self.pool.entry.process == Process::Spawn {
            self.worker.finish().await;
        } else {
            let c = self.pool.circuit.lock().unwrap();
            if c.epoch == self.ticket.epoch && !self.ticket.probe && c.window.is_none() {
                self.pool.park(self.worker);
            }
        }
    }
    /// 只从写出开始计时，已取得名额后的本地正文读取也不占管道时限。
    pub(crate) async fn send(self, stage: Stage, timeout: u64) -> Result<PreparedReply, Fault> {
        self.send_until(stage, timeout, None).await
    }
    /// 心跳还受下一拍的硬截止点约束；只处罚已写出却未及时得到回信的会话。普通取消直接丢票。
    pub(crate) async fn send_until(
        mut self,
        stage: Stage,
        timeout: u64,
        deadline: Option<(Instant, &Mutex<bool>)>,
    ) -> Result<PreparedReply, Fault> {
        let pool = self.pool;
        if deadline.is_some_and(|(d, _)| Instant::now() >= d) {
            self.unused().await;
            return Err(Fault::Timeout);
        }
        self.worker.written = false;
        let mut retiring = None;
        let exchange = async {
            let reply = pool
                .exchange(&mut self.worker, stage, timeout, Some(&self.ticket))
                .await?;
            if pool.entry.process == Process::Spawn {
                if deadline.is_some() {
                    let end = Instant::now() + RETIRE_GRACE;
                    self.worker.final_stdout(end).await?;
                    retiring = Some(end);
                } else {
                    self.worker.retire_pipe(true).await?;
                }
            }
            Ok(reply)
        };
        let result = if let Some((d, _)) = deadline {
            tokio::select! {biased; _=tokio::time::sleep_until(d)=>Err(Fault::Timeout),r=exchange=> {
                if Instant::now() < d { r } else { Err(Fault::Timeout) }
            }}
        } else {
            exchange.await
        };
        if matches!(result, Err(Fault::Open { .. })) {
            return result;
        }
        let result = result.and_then(|r| {
            Ok((
                r,
                pool.entry.process != Process::Spawn && self.worker.reusable()?,
            ))
        });
        {
            // abort 不会打断另一线程正在执行的 poll；结算和响应就绪必须在同一把闸锁下排序。
            let cancelled = deadline.map(|(_, gate)| gate.lock().unwrap());
            if !cancelled.as_deref().copied().unwrap_or(false)
                && !(deadline.is_some()
                    && !self.worker.written
                    && matches!(result, Err(Fault::Timeout)))
            {
                pool.complete(&self.ticket, result.as_ref().err());
            }
        }
        let (reply, reuse) = result?;
        if let Some(end) = retiring {
            // stdout 已经验完；只转交回收，槽位仍随真正的进程所有者走。
            let mut tasks = pool.retiring.lock().unwrap();
            while tasks.try_join_next().is_some() {}
            tasks.spawn(async move { self.worker.reap_until(end).await });
        } else if !reuse && pool.entry.process != Process::Spawn {
            if deadline.is_some() {
                pool.retire(self.worker);
            } else {
                self.worker.finish().await;
            }
        } else if !reuse {
            // 普通 spawn 的退出已在管道检查之后等待完成。
            return Ok(reply);
        } else {
            let c = pool.circuit.lock().unwrap();
            if c.epoch == self.ticket.epoch && c.window.is_none() {
                pool.park(self.worker);
            }
        }
        Ok(reply)
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
