//! format 协议的进程与管道。一份配置共享一个池，序号、读缓冲和槽位都跟随进程，
//! 不跟随请求；在写入和读完一帧之间取消任务，等价于丢掉这个会话。

use crate::config::file::{FormatEntry, Process};
use aproxy_envelope::{BodyNeed, Hello, HelloReply, Message, Reply, Stage};
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
fn program_path(command: &str, home: &Path) -> Result<PathBuf, Fault> {
    let expanded = crate::config::expand_tilde(command);
    let p = Path::new(&expanded);
    if p.file_name().is_none_or(|name| name != p.as_os_str()) {
        return Ok(home.join(p));
    }
    let dirs = std::env::var_os("PATH").unwrap_or_default();
    for dir in std::iter::once(home.join("bin")).chain(std::env::split_paths(&dirs)) {
        let file = dir.join(p);
        if file.is_file() {
            return Ok(file);
        }
        #[cfg(windows)]
        if file.extension().is_none() && file.with_extension("exe").is_file() {
            return Ok(file.with_extension("exe"));
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
impl Worker {
    /// 正常结束先发 EOF，给遵守协议的程序留出清理机会；不读 EOF 的进程仍能回收。
    async fn finish(mut self) {
        self.stdin.take();
        if tokio::time::timeout(Duration::from_secs(1), self.child.as_mut().unwrap().wait())
            .await
            .is_ok_and(|wait| wait.is_ok())
        {
            self.child.take();
        }
    }
    /// 排队与序列化不算在时限内，堵在写管道则算；调用被取消时不复用 worker。
    async fn frame(&mut self, stage: Stage, timeout: u64) -> Result<Vec<u8>, Fault> {
        let mut line = serde_json::to_vec(&Message::new(self.seq, stage))
            .expect("protocol messages serialize");
        line.push(b'\n');
        let io = async {
            self.stdin
                .as_mut()
                .unwrap()
                .write_all(&line)
                .await
                .map_err(|_| Fault::Io)?;
            let mut reply = Vec::new();
            self.stdout
                .read_until(b'\n', &mut reply)
                .await
                .map_err(|_| Fault::Io)?;
            match reply.last() {
                None => Err(Fault::Died),
                Some(b'\n') if self.stdout.buffer().is_empty() => Ok(reply),
                _ => Err(Fault::Protocol),
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
            if c.epoch == self.epoch {
                c.probing = false;
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
    /// 同时进池的请求数；空闲进程可由持票者直接取走。
    turns: Semaphore,
    /// 已启动但仍未退出的进程数上限。
    capacity: Arc<Semaphore>,
    /// 活进程的编号，空洞总是优先复用。
    ids: Arc<Mutex<BTreeSet<u32>>>,
    /// 仅在收发全部完成后归还，这把同步锁不跨 await。
    idle: Mutex<Vec<Worker>>,
    /// 回收任务只持弱引用，实例退出不被它阻挡。
    reaper: OnceLock<()>,
    /// 熔断状态的写锁只覆盖进程消息的接纳与完成，不跨管道操作。
    circuit: Mutex<Circuit>,
    /// IPC 订阅者在阶段变动时重读快照。
    notify: Arc<tokio::sync::Notify>,
    /// 关闭时唤醒等候池位的消息；旧队列不得成为下一轮冷却后的试探。
    changed: tokio::sync::watch::Sender<u64>,
    /// 最近一次成功握手的身份与能力，握手失败时不杜撰版本。
    pub(crate) negotiated: Mutex<Option<HelloReply>>,
}
impl Pool {
    /// 这里只分配管理结构；进程由异步启动探测或阶段入口创建。
    pub(crate) fn new(
        entry: FormatEntry,
        home: PathBuf,
        secrets: Secrets,
        notify: Arc<tokio::sync::Notify>,
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
            turns: Semaphore::new(n),
            capacity: Arc::new(Semaphore::new(n)),
            ids: Default::default(),
            idle: Default::default(),
            reaper: OnceLock::new(),
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
        if c.epoch != a.epoch {
            return;
        }
        if let Some(f) = fault {
            let now = crate::daemon::now_millis();
            c.failures = c.failures.saturating_add(1);
            c.last = Some((f.clone(), now));
            if a.probe {
                self.open(&mut c, now);
            }
        } else {
            c.failures = 0;
            if a.probe {
                c.window = None;
                c.probing = false;
                self.notify.notify_waiters();
                tracing::info!(format=%self.secrets.replace(&self.entry.name),"format circuit closed");
            }
        }
    }
    /// 建会话、分配最小空闲编号。每次都重新查找命令，供冷却后的程序恢复。
    async fn spawn(&self) -> Result<Worker, Fault> {
        let permit = self
            .capacity
            .clone()
            .acquire_owned()
            .await
            .expect("pool is never closed");
        let exe = program_path(&self.entry.command, &self.home)?;
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
        let mut child = Command::new(exe)
            .args(&self.entry.args)
            .envs(&self.entry.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| Fault::Spawn)?;
        let stderr = child.stderr.take().unwrap();
        let secrets = self.secrets.clone();
        let name = secrets.replace(&self.entry.name);
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr);
            let mut line = Vec::new();
            while let Ok(n) = reader.read_until(b'\n', &mut line).await {
                if n == 0 {
                    break;
                }
                let safe = secrets.replace_bytes(&line);
                tracing::warn!(
                    "format[{}] {}",
                    name,
                    String::from_utf8_lossy(&safe).trim_end_matches(['\r', '\n'])
                );
                line.clear();
            }
        });
        let mut w = Worker {
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
        let line = w.frame(Stage::Hello(hello), 10).await?;
        let reply: HelloReply = serde_json::from_slice(&line).map_err(|_| Fault::Invalid)?;
        if reply.v != 1 || reply.protocol != 1 || reply.seq != 0 {
            return Err(Fault::Protocol);
        }
        if matches!(reply.request_body.get("notify"), Some(BodyNeed::Full)) {
            return Err(Fault::Invalid);
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
        let w = self.spawn().await?;
        self.put(w).await;
        Ok(())
    }
    /// 没有流占用时，回收器每个空闲期唤醒一次，取出时也检查截止时刻。
    async fn put(self: &Arc<Self>, mut w: Worker) {
        if self.entry.process == Process::Spawn {
            w.finish().await;
            return;
        }
        // Windows 的异步管道首个 poll 可能只启动后台读取。归还时先 poll 一次，
        // 下次借出再查，跨越这两次检查的迟到帧则由 seq 拦住。
        if futures_util::FutureExt::now_or_never(w.stdout.fill_buf()).is_some() {
            return;
        }
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
                        idle.retain(|w| w.idle_since.elapsed() < timeout);
                        idle.iter()
                            .map(|w| timeout.saturating_sub(w.idle_since.elapsed()))
                            .min()
                            .unwrap_or(timeout)
                    };
                    drop(p);
                    tokio::time::sleep(delay).await;
                }
            });
        });
    }
    /// 空闲进程若已退出，或空闲过久，直接回收；下一次消息使用另一条会话。
    fn take(&self) -> Option<Worker> {
        let mut idle = self.idle.lock().unwrap();
        while let Some(mut w) = idle.pop() {
            if (self.entry.worker_idle_secs == 0
                || w.idle_since.elapsed() < Duration::from_secs(self.entry.worker_idle_secs))
                && matches!(w.child.as_mut().unwrap().try_wait(), Ok(None))
                && futures_util::FutureExt::now_or_never(w.stdout.fill_buf()).is_none()
            {
                return Some(w);
            }
        }
        None
    }
    /// 做一次有序往返。序号先于业务字段检查；字段语义也要在归还 worker 之前校验。
    pub(crate) async fn call(
        self: &Arc<Self>,
        stage: Stage,
        timeout: u64,
        fresh: bool,
    ) -> Result<Reply, Fault> {
        let mut changed = self.changed.subscribe();
        let ticket = self.admit()?;
        let _turn = tokio::select! {biased;
            _=changed.changed()=>return Err(self.circuit.lock().unwrap().refusal()),
            turn=self.turns.acquire()=>turn.expect("pool is never closed"),
        };
        let result = self
            .exchange(stage, timeout, fresh || ticket.probe, &ticket)
            .await;
        if !matches!(&result, Err(Fault::Open { .. })) {
            self.complete(&ticket, result.as_ref().err());
        }
        result
    }
    /// 得到派发位后取得会话；若排队或建会话期间已经熔断，不把阶段帧写进管道。
    async fn exchange(
        self: &Arc<Self>,
        stage: Stage,
        timeout: u64,
        fresh: bool,
        a: &Admission<'_>,
    ) -> Result<Reply, Fault> {
        if self.circuit.lock().unwrap().epoch != a.epoch {
            return Err(self.circuit.lock().unwrap().refusal());
        }
        let old = self.take();
        let mut w = if fresh {
            drop(old);
            self.spawn().await?
        } else if let Some(w) = old {
            w
        } else {
            self.spawn().await?
        };
        if self.circuit.lock().unwrap().epoch != a.epoch {
            return Err(self.circuit.lock().unwrap().refusal());
        }
        let line = w.frame(stage, timeout).await?;
        #[derive(serde::Deserialize)]
        struct Sequence {
            v: u32,
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
                .any(|s| http::StatusCode::from_u16(s).is_err())
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
        w.seq += 1;
        if self.circuit.lock().unwrap().epoch == a.epoch {
            self.put(w).await;
        }
        Ok(r)
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
