//! format 协议的进程与管道。一份配置共享一个池，序号、读缓冲和槽位都跟随进程，
//! 不跟随请求；在写入和读完一帧之间取消任务，等价于丢掉这个会话。

use crate::config::file::{FormatEntry, Process};
use aproxy_envelope::{Hello, HelloReply, Message, Reply, Stage};
use aproxy_ipc::redact::Secrets;
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::{OwnedSemaphorePermit, Semaphore},
    time::Instant,
};

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
    loop {
        let Ok(chunk) = r.fill_buf().await else { break };
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
                truncated |= safe.len() > STDERR_BYTES;
                safe.truncate(STDERR_BYTES);
                tracing::warn!(
                    "format[{}] {}{}",
                    name,
                    String::from_utf8_lossy(&safe).trim_end_matches('\r'),
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
    let name = if p.extension().is_none() {
        p.with_extension("exe")
    } else {
        p.to_owned()
    };
    #[cfg(not(windows))]
    let name = p;
    for dir in
        std::iter::once(home.join("bin")).chain(path.into_iter().flat_map(std::env::split_paths))
    {
        let file = dir.join(&name);
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
    used: Arc<Mutex<BTreeSet<u32>>>,
    /// 进程数上限与并发请求数分别计数，前者包括正在退出的进程。
    _permit: OwnedSemaphorePermit,
}
impl Drop for Slot {
    fn drop(&mut self) {
        self.used.lock().unwrap().remove(&self.id);
    }
}

/// stdin、stdout 与序号作为一组移动；读缓冲在同一进程的各帧之间保持不变。
struct Worker {
    /// Drop 时可以取出子进程，异步等待真正退出。
    child: Option<Child>,
    /// 强制回收时终止读取，避免继承管道的后代拖住日志任务。
    stderr: tokio::task::JoinHandle<()>,
    /// 先关闭 stdin 通知进程退出，再收走进程名额。
    stdin: Option<ChildStdin>,
    /// 已预读的下一行不能交给另一个进程或请求。
    stdout: BufReader<ChildStdout>,
    /// 直到 wait 完成才交还进程池。
    slot: Option<Slot>,
    /// 握手用 0，其余消息沿用这个进程各自的序列。
    seq: u64,
    /// 最后一次归还到池里的时刻。
    idle_since: Instant,
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stderr.abort();
        self.stdin.take();
        if let Some(mut child) = self.child.take() {
            let _ = child.start_kill();
            let slot = self.slot.take();
            if let Ok(rt) = tokio::runtime::Handle::try_current() {
                rt.spawn(async move {
                    let _ = child.wait().await;
                    drop(slot);
                });
            }
        }
    }
}
/// 正常退役的 EOF 宽限时间；并发等待而不是按进程逐个累加。
const RETIRE_GRACE: Duration = Duration::from_secs(2);
impl Worker {
    /// 无数据可读才复用；正常 EOF 使本次回信有效但不留进程，未请求的额外字节则使往返失败。
    fn reusable(&mut self) -> Result<bool, Fault> {
        match futures_util::FutureExt::now_or_never(self.stdout.fill_buf()) {
            None => Ok(true),
            Some(Ok([])) => Ok(false),
            Some(Ok(_)) => Err(Fault::Protocol),
            Some(Err(_)) => Err(Fault::Io),
        }
    }
    /// 正常结束先发 EOF，给遵守协议的程序留出清理机会；不读 EOF 的进程仍能回收。
    async fn finish(mut self) {
        self.stdin.take();
        let end = Instant::now() + RETIRE_GRACE;
        if tokio::time::timeout_at(end, self.child.as_mut().unwrap().wait())
            .await
            .is_ok_and(|wait| wait.is_ok())
        {
            self.child.take();
            self.slot.take();
            // 正常退出允许排空最后几行，但继承 stderr 的其他进程不应阻挡回收。
            let _ = tokio::time::timeout_at(
                end.min(Instant::now() + Duration::from_millis(100)),
                &mut self.stderr,
            )
            .await;
        }
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
                let mut changed = a.pool.changed.subscribe();
                let first = std::future::poll_fn(|cx| {
                    let c = a.pool.circuit.lock().unwrap();
                    if c.epoch != a.epoch {
                        return std::task::Poll::Ready(Err(c.refusal()));
                    }
                    std::pin::Pin::new(&mut *stdin)
                        .poll_write(cx, &line)
                        .map_err(|_| Fault::Io)
                });
                tokio::select! {biased; _=changed.changed()=>return Err(a.pool.circuit.lock().unwrap().refusal()), result=first=>result?}
            } else {
                0
            };
            if admission.is_some() && written == 0 {
                return Err(Fault::Io);
            }
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

/// 每次打开都推进代数，已写出的旧请求只能结束自己的往返，不能让熔断器越过冷却。
#[derive(Default)]
struct Circuit {
    /// 当前冷却代；活跃消息在派发时记下它。
    epoch: u64,
    /// 进程级连续失败次数，合法 fail 回信也清零。
    failures: u32,
    /// 这次熔断的开始与最早试探时刻，单位都是 Unix 毫秒。
    window: Option<(u64, u64)>,
    /// 冷却结束后，同一时刻只有一个数据消息在试探。
    probing: bool,
    /// 最近一次失败分类与时刻，不记录任意进程输出。
    last: Option<(Fault, u64)>,
}
impl Circuit {
    /// 为快速失败保留快照所用的同一个分类与试探时间。
    fn refusal(&self) -> Fault {
        Fault::Open {
            kind: self.last.as_ref().map_or("io", |(f, _)| f.kind()).into(),
            probe_at: self.window.map_or(0, |(_, t)| t),
        }
    }
}
/// 客户端取消时释放试探资格，不计故障，也不让半开的状态永远卡住。
struct Admission<'a> {
    /// 这次派发所属的池。
    pool: &'a Pool,
    /// 只改变自己所属代的状态。
    epoch: u64,
    /// 普通消息不用归还试探资格。
    probe: bool,
}
impl Drop for Admission<'_> {
    fn drop(&mut self) {
        if self.probe {
            let mut c = self.pool.circuit.lock().unwrap();
            if c.epoch == self.epoch && c.probing {
                c.probing = false;
                self.pool.notify.notify_waiters();
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
    /// 已启动但仍未退出的进程数上限。
    capacity: Arc<Semaphore>,
    /// 活进程的编号，空洞总是优先复用。
    ids: Arc<Mutex<BTreeSet<u32>>>,
    /// 仅在收发全部完成后归还，这把同步锁不跨 await。
    idle: Mutex<Vec<Worker>>,
    /// 回收任务只持弱引用，实例退出不被它阻挡。
    reaper: OnceLock<tokio::task::JoinHandle<()>>,
    /// 同一池只探测一次；同时询问的调用方共享第一次握手的结果。
    startup: tokio::sync::OnceCell<Result<(), Fault>>,
    /// 因空闲正常退役的进程在后台限时等待 EOF；实例关闭时一起等完。
    retiring: Mutex<tokio::task::JoinSet<()>>,
    /// 熔断状态的写锁只覆盖进程消息的接纳与完成，不跨管道操作。
    circuit: Mutex<Circuit>,
    /// IPC 订阅者在阶段变动时重读快照。
    notify: Arc<tokio::sync::Notify>,
    /// 关闭时唤醒等候池位的消息；旧队列不得成为下一轮冷却后的试探。
    changed: tokio::sync::watch::Sender<u64>,
    /// 最近一次成功握手的身份与能力，握手失败时不杜撰版本。
    pub(crate) negotiated: Mutex<Option<HelloReply>>,
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
        notify: Arc<tokio::sync::Notify>,
        body_limits: [usize; 2],
    ) -> Arc<Self> {
        let n = if entry.process == Process::Spawn {
            1
        } else {
            entry.pool_max as usize
        };
        Arc::new(Self {
            entry,
            home,
            secrets,
            frame_limits: body_limits.map(|n| n.saturating_mul(2).saturating_add(FRAME_OVERHEAD)),
            stderr_window: Arc::new(Mutex::new(LogWindow::new())),
            turns: Semaphore::new(n),
            capacity: Arc::new(Semaphore::new(n)),
            ids: Default::default(),
            idle: Default::default(),
            reaper: OnceLock::new(),
            startup: Default::default(),
            retiring: Default::default(),
            notify,
            negotiated: Mutex::new(None),
            circuit: Default::default(),
            changed: tokio::sync::watch::channel(0).0,
        })
    }
    /// 快照不曝光命令行、环境、回信中的任意文本；版本与配置名也替换已知凭据。
    pub(crate) fn snapshot(&self) -> serde_json::Value {
        use serde_json::json;
        let c = self.circuit.lock().unwrap();
        let mut breaker = json!({"state":if c.probing{"half_open"}else if c.window.is_some(){"open"}else{"closed"},"failures":c.failures});
        if let Some((opened, probe)) = c.window {
            breaker["opened_at"] = json!(opened);
            breaker["probe_at"] = json!(probe);
        }
        if let Some((f, at)) = &c.last {
            breaker["last_failure"] =
                json!({"code":"format_error","details":{"kind":f.kind()},"at":at});
        }
        let mut snap = json!({"name":self.secrets.replace(&self.entry.name),"process":if self.entry.process==Process::Spawn{"spawn"}else{"persistent"},"required":self.entry.required,"stages":self.entry.stages.iter().map(|s|s.as_str()).collect::<Vec<_>>(),"breaker":breaker});
        if let Some(h) = self.negotiated.lock().unwrap().as_ref() {
            snap["protocol"] = json!(h.protocol);
            snap["version"] = json!(self.secrets.replace(&h.version));
        }
        snap
    }
    /// 记录冷却并清空空闲进程；与试探、已排队消息的判定共用同一把锁。
    fn open(&self, c: &mut Circuit, now: u64) {
        c.epoch = c.epoch.wrapping_add(1);
        c.window = Some((
            now,
            now.saturating_add(self.entry.breaker_cooldown_secs.saturating_mul(1000)),
        ));
        c.probing = false;
        self.idle.lock().unwrap().clear();
        self.changed.send_replace(c.epoch);
        self.notify.notify_waiters();
        tracing::warn!(format=%self.secrets.replace(&self.entry.name),failures=c.failures,cooldown_secs=self.entry.breaker_cooldown_secs,kind=c.last.as_ref().map(|(f,_)|f.kind()),"format circuit opened");
    }
    /// 可选程序启动时暂不可用，与静态不匹配不同；冷却后允许一次真正的数据往返。
    pub(crate) fn open_at_startup(&self, f: Fault) {
        let mut c = self.circuit.lock().unwrap();
        let now = crate::daemon::now_millis();
        c.failures = 1;
        c.last = Some((f, now));
        self.open(&mut c, now);
    }
    /// 打开的程序不排队。冷却后的首条消息抢到独占试探，即使用 persistent 也重新握手。
    fn admit(&self) -> Result<Admission<'_>, Fault> {
        let mut c = self.circuit.lock().unwrap();
        let probe = if let Some((_, at)) = c.window {
            if c.probing || crate::daemon::now_millis() < at {
                return Err(c.refusal());
            }
            c.probing = true;
            self.notify.notify_waiters();
            tracing::info!(format=%self.secrets.replace(&self.entry.name),"format circuit half-open");
            true
        } else {
            false
        };
        Ok(Admission {
            pool: self,
            epoch: c.epoch,
            probe,
        })
    }
    /// 每个已写出的消息按完成顺序登记；程序的显式拒绝是合法回信，不是进程故障。
    fn complete(&self, a: &Admission<'_>, fault: Option<&Fault>) {
        let mut c = self.circuit.lock().unwrap();
        let current_probe = a.probe && c.epoch == a.epoch;
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
                self.notify.notify_waiters();
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
                .find(|id| !ids.contains(id))
                .expect("worker count fits u32");
            ids.insert(id);
            id
        };
        let slot = Slot {
            id,
            used: self.ids.clone(),
            _permit: permit,
        };
        let mut child = {
            let c = self.circuit.lock().unwrap();
            if admission.is_some_and(|a| a.epoch != c.epoch) {
                return Err(c.refusal());
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
        let stderr = tokio::spawn(log_stderr(
            child.stderr.take().unwrap(),
            self.secrets.clone(),
            self.secrets.replace(&self.entry.name).into(),
            self.stderr_window.clone(),
        ));
        let mut w = Worker {
            stderr,
            stdin: child.stdin.take(),
            stdout: BufReader::new(child.stdout.take().unwrap()),
            child: Some(child),
            seq: 0,
            slot: Some(slot),
            idle_since: Instant::now(),
        };
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
        hello.overrides = OVERRIDES.iter().map(|k| (*k).into()).collect();
        let line = w.frame(Stage::Hello(hello), 10, None, HELLO_BYTES).await?;
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
        *self.negotiated.lock().unwrap() = Some(reply);
        w.seq = 1;
        Ok(w)
    }
    /// 探测 persistent 的首个进程留池；spawn 进程在握手后单独关闭。
    pub(crate) async fn probe(self: &Arc<Self>) -> Result<(), Fault> {
        self.startup
            .get_or_init(|| async {
                let mut w = self.spawn(None).await?;
                if self.entry.process == Process::Spawn || !w.reusable()? {
                    w.finish().await;
                } else {
                    self.park(w);
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
    /// 停止回收计时器，发出所有空闲进程的 EOF，与已经退役的进程并行等待。
    pub(crate) async fn shutdown(&self) {
        if let Some(t) = self.reaper.get() {
            t.abort();
        }
        let workers = std::mem::take(&mut *self.idle.lock().unwrap());
        let mut retired = std::mem::take(&mut *self.retiring.lock().unwrap());
        for w in workers {
            retired.spawn(w.finish());
        }
        while retired.join_next().await.is_some() {}
    }
    /// 只有刚检查过输出的健康会话可以归还；第二次检查放在取用一刻，截住迟到的杂字节。
    fn park(self: &Arc<Self>, mut w: Worker) {
        w.idle_since = Instant::now();
        self.idle.lock().unwrap().push(w);
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
            match w.child.as_mut().unwrap().try_wait() {
                Ok(Some(_)) => {
                    w.child.take();
                    w.slot.take();
                    continue;
                }
                Err(_) => continue,
                Ok(None) => {}
            }
            if self.entry.worker_idle_secs != 0
                && w.idle_since.elapsed() >= Duration::from_secs(self.entry.worker_idle_secs)
            {
                self.retire(w);
            } else if matches!(w.reusable(), Ok(true)) {
                return Some(w);
            }
        }
        None
    }
    /// 先拿到已握手的独占进程，再让上一层读取磁盘与构造业务帧；等待者只持有轻量上下文。
    pub(crate) async fn turn(self: &Arc<Self>, fresh: bool) -> Result<Turn<'_>, Fault> {
        let mut changed = self.changed.subscribe();
        let ticket = self.admit()?;
        let _turn = tokio::select! {biased;
            _=changed.changed()=>return Err(self.circuit.lock().unwrap().refusal()),
            turn=self.turns.acquire()=>turn.expect("pool is never closed"),
        };
        let borrow = async {
            let old = self.take();
            if fresh || ticket.probe {
                drop(old);
            } else if let Some(w) = old {
                return Ok(w);
            }
            self.spawn(Some(&ticket)).await
        };
        let worker = tokio::select! {biased;
            _=changed.changed()=>return Err(self.circuit.lock().unwrap().refusal()),
            result=borrow=>result,
        };
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
    /// 管道单元测试只传很小的合成帧，生产路径先取得 turn 再构造可能很大的消息。
    #[cfg(test)]
    async fn call(
        self: &Arc<Self>,
        stage: Stage,
        timeout: u64,
        fresh: bool,
    ) -> Result<Reply, Fault> {
        self.turn(fresh).await?.send(stage, timeout).await
    }
    /// 帧的首次写入与熔断按同一把锁排序；已经发出的消息读完并计数，但不会提前结束冷却。
    async fn exchange(
        &self,
        w: &mut Worker,
        stage: Stage,
        timeout: u64,
        a: &Admission<'_>,
    ) -> Result<Reply, Fault> {
        let limit = match &stage {
            Stage::Request(_) | Stage::Retry(_) => self.frame_limits[0],
            Stage::Response(_) | Stage::Stream(_) => self.frame_limits[1],
            _ => FRAME_OVERHEAD,
        };
        let line = w.frame(stage, timeout, Some(a), limit).await?;
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
        match &r.report {
            aproxy_envelope::ReportField::Invalid => {
                tracing::warn!(format=%self.secrets.replace(&self.entry.name),"invalid format report ignored")
            }
            aproxy_envelope::ReportField::Present(report) if report.discarded_actions != 0 => {
                tracing::warn!(format=%self.secrets.replace(&self.entry.name),count=report.discarded_actions,"invalid format report actions ignored")
            }
            _ => (),
        }
        w.seq += 1;
        Ok(r)
    }
}

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
    /// 只从写出开始计时，已取得名额后的本地正文读取也不占管道时限。
    pub(crate) async fn send(mut self, stage: Stage, timeout: u64) -> Result<Reply, Fault> {
        let pool = self.pool;
        let result = pool
            .exchange(&mut self.worker, stage, timeout, &self.ticket)
            .await;
        if matches!(result, Err(Fault::Open { .. })) {
            return result;
        }
        let result = result.and_then(|r| {
            Ok((
                r,
                pool.entry.process == Process::Persistent && self.worker.reusable()?,
            ))
        });
        pool.complete(&self.ticket, result.as_ref().err());
        let (reply, reuse) = result?;
        if !reuse {
            self.worker.finish().await;
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
