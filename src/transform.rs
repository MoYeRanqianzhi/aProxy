//! 外部转换器（外部 format 程序）的进程池编排。
//!
//! aproxy 把请求/响应装进一行 JSON 信封（见 aproxy-envelope crate）交给外部
//! format 程序改写后收回——转换逻辑全部外置，本模块只做编排。两种模式统一进
//! 同一个池抽象：
//! - `spawn`（一次性）：每请求启动进程、一行进出、进程退出（permits=1、
//!   idle 恒空——用毕即弃）
//! - `persistent`（持续进程池）：worker 进程 `while` 循环逐行处理（一次一个
//!   请求、输入输出有序、无多路复用），按需扩容至 `pool_max`，空闲超时回收
//!
//! 失败语义由调用方（proxy.rs）决定：请求侧 502 不重试、响应侧透传原样。
//! 池内唯一的重试是「复用的空闲 worker 在产出前就死了 → 换新 worker 重做
//! 一次」（见 convert）：那是池拿到了死 worker 的状态问题，不是转换失败，
//! 不改变上述失败语义。
//!
//! **stdout 同步不变量**（串包防线）：信封协议没有请求序号，worker 的第 N 行
//! 输出只能靠「一请求恰一行、按序」对应第 N 个请求。因此 worker 只有在本次
//! 输出被确认为一行合法信封、且其后没有残留输出时才归还空闲表；任何让对应
//! 关系存疑的情形（非法行、多行、空闲期冒出的输出）一律剔除 worker——留在池
//! 里的话，下一个请求会读到上一个请求的输出（A 会话的 body/key 发往 B）。

use std::{
    collections::btree_map::{BTreeMap, Entry},
    path::Path,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering as AtomicOrdering},
    },
    task::{Context, Poll, Waker},
    time::{Duration, Instant},
};

use tokio::io::{AsyncBufRead, AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::{Mutex, Semaphore};

use aproxy_envelope::TransformEnvelope;

use crate::{
    config::{TransformConfig, TransformMode},
    proxy::{RESIDENT_LIMIT, RequestBody, SpooledBody, is_hop_header, unique_seq},
};

/// 转换失败分类。Display 文案供 502 响应体与日志直接内插。
///
/// 按成因分类，文案各自如实表述（不把瞬时故障说成「确定性」）：
/// - **format 的输出本身**（同一输入重来大概率同样失败）：`Rejected`
///   （format 自报 error 行）、`Protocol`（输出违反信封协议）
/// - **进程/管道层故障**（与输入无关，常为瞬时）：`Spawn`、`Io`、
///   `WorkerDied`、`TimedOut`
/// - **其他本地故障**：`Spool`（磁盘）、`Serialize`（aproxy 侧，不发生）、
///   `Closed`（防御性）
#[derive(Debug)]
pub(crate) enum TransformError {
    /// format 进程启动失败（命令不存在、二进制正被替换、句柄耗尽等）。
    Spawn(std::io::Error),
    /// 信封序列化失败（字段全是字符串/整数，现实中不发生）。
    Serialize(serde_json::Error),
    /// 读写 format 进程的 stdin/stdout 失败（管道断开等）。
    Io(std::io::Error),
    /// 读取落盘的请求/响应 body 临时文件失败（本地磁盘故障，与 format 无关）。
    Spool(std::io::Error),
    /// 单请求转换超时（worker 可能仍在消化旧输入，kill 后剔除，绝不能复用）。
    TimedOut,
    /// worker 进程意外退出且无输出（EOF）。
    WorkerDied,
    /// format 的输出违反信封协议（非 UTF-8、非合法信封 JSON、body 与 body_b64
    /// 冲突、persistent 下单请求输出多行等）。携带人类可读的细节。
    /// 与 `Rejected` 的区别：这不是 format 的业务判定，而是 format 程序本身
    /// 写错了（往 stdout 打日志/横幅、jq 忘加 -c 等），排障方向完全不同。
    Protocol(String),
    /// format 通过信封 error 行表达的单请求失败（进程未崩）。
    Rejected(String),
    /// 池已关闭（防御性：现实中信号量不会被 close）。
    Closed,
}

impl std::fmt::Display for TransformError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 各变体文案的**开头短语**被排障文档（aproxy-format skill 的
        // troubleshooting）按「错误含 xxx」检索，改动开头需同步文档
        match self {
            TransformError::Spawn(e) => {
                write!(f, "format 进程启动失败（检查 command 路径）: {e}")
            }
            TransformError::Serialize(e) => write!(f, "信封序列化失败: {e}"),
            TransformError::Io(e) => write!(
                f,
                "format 进程管道读写失败（进程/管道层故障，worker 已剔除）: {e}"
            ),
            TransformError::Spool(e) => write!(f, "body 临时文件读取失败（本地磁盘故障）: {e}"),
            TransformError::TimedOut => write!(
                f,
                "转换超时（format 进程已终止并剔除；可能是瞬时负载，也可能是 format 卡死）"
            ),
            TransformError::WorkerDied => write!(
                f,
                "format 进程意外退出且无输出（进程层故障，worker 已剔除）"
            ),
            TransformError::Protocol(detail) => {
                write!(f, "format 输出违反信封协议: {detail}")
            }
            TransformError::Rejected(msg) => write!(f, "format 报告转换失败: {msg}"),
            TransformError::Closed => write!(f, "转换器已关闭"),
        }
    }
}

/// 一次「写一行、读一行」往返的结局。worker 的所有权在往返内部处置完毕
/// （归还空闲表或剔除），调用方只需按结局决定是否换 worker 重试。
// Done 携带整个信封、比 DeadBeforeOutput 大得多：本枚举每次转换只按值返回
// 一次、随即被拆开，装箱省下的栈空间不值一次堆分配，保持扁平匹配
#[allow(clippy::large_enum_variant)]
enum Exchange {
    /// 往返完成：成功，或不应重试的失败（format 自报 error、协议错误、超时、
    /// 读到部分输出后才出错）。
    Done(Result<TransformEnvelope, TransformError>),
    /// worker 在产出**任何**输出之前就失败（写入失败 / 读到 EOF / 读出错且
    /// 无字节）：worker 已剔除。此时 format 尚未对本请求产出任何东西，换一个
    /// worker 重做不会重复或丢失输出——是否重试由调用方按 worker 来源决定。
    DeadBeforeOutput(TransformError),
}

/// worker stdout 的「此刻」状态（非阻塞探测，见 [`probe_stdout`]）。
enum StdoutProbe {
    /// 暂无可读数据：stdout 与请求的对应关系完好。
    Quiet,
    /// 已有未被请求的输出待读：再复用该 worker，下一个请求就会读到它（串包）。
    Stray,
    /// EOF 或读错误：进程已退出 / 管道已断。
    Closed,
}

/// 非阻塞探测 worker stdout：只 poll 一次、不等待。
///
/// 用 noop waker 单次 poll `poll_fill_buf`：BufReader 内部缓冲非空时直接
/// 返回缓冲（确定性地发现「同批到达的多余行」）；缓冲空时向底层管道要数据——
/// unix 上是非阻塞读，立刻知道管道里有没有；Windows 上 tokio 的子进程管道是
/// 「后台阻塞线程读」适配器，首次 poll 只是**发起**一次后台读并返回 Pending，
/// 读到的数据（或 EOF）由适配器暂存、下次 poll 交付，不会丢失。因此本函数在
/// 归还空闲表时调用一次（Windows 上顺带发起后台读）、从空闲表取出时再调用一次
/// （此时空闲期里冒出的输出或进程退出已可见），两个平台都能覆盖空闲期。
///
/// 残余窗口（诚实声明）：多余输出若恰好在「取出探测」与「本次读」之间的微秒
/// 级窗口才到达，仍会被当作本请求的输出——根治需要信封带请求序号由 format
/// 回显（协议变更），属后续版本议题。
fn probe_stdout(stdout: &mut BufReader<ChildStdout>) -> StdoutProbe {
    let mut cx = Context::from_waker(Waker::noop());
    match Pin::new(stdout).poll_fill_buf(&mut cx) {
        Poll::Pending => StdoutProbe::Quiet,
        Poll::Ready(Ok([])) => StdoutProbe::Closed,
        Poll::Ready(Ok(_)) => StdoutProbe::Stray,
        Poll::Ready(Err(_)) => StdoutProbe::Closed,
    }
}

/// 池内单个 worker：一个长驻子进程 + 它独占的 stdin/stdout。
///
/// **stdout 必须用同一个 BufReader 包裹**（存进 Worker 而非每次 convert 新建）：
/// BufReader 有内部缓冲，persistent worker 的响应交错到达时，新建的
/// BufReader 会把上个缓冲里残留的下一行丢掉——这是致命 bug 风险，结构上根除。
struct Worker {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<tokio::process::ChildStdout>,
    /// 池槽位号（0..pool_max）：轮换类 format 用它做每 worker 起始偏移，
    /// 避免池内各 worker 轮换起点重合。
    worker_id: u32,
    last_used: Instant,
}

struct PoolState {
    idle: Vec<Worker>,
    next_worker_id: u32,
}

/// 外部转换器进程池。spawn 与 persistent 两模式统一抽象（permits=1、idle
/// 恒空即退化为一次性 spawn），调用侧无分支。
pub(crate) struct TransformPool {
    cfg: Arc<TransformConfig>,
    /// Arc 化：空闲回收 reaper 任务需要 'static 的 state 引用（&self.state
    /// 借用逃不出 tokio::spawn），clone Arc 即可。
    state: Arc<Mutex<PoolState>>,
    /// 并发上限（含待 spawn 名额）：并发超 pool_max 排队——排队只加延迟不损
    /// 吞吐，与重试语义天然兼容。
    permits: Semaphore,
    /// 空闲回收 reaper 是否已启动（惰性：首次 convert 时在 tokio 上下文内
    /// 启动——AppState::new 可能不在 async 上下文，tokio::spawn 会 panic）。
    reaper_started: AtomicBool,
}

impl TransformPool {
    pub(crate) fn new(cfg: Arc<TransformConfig>) -> Self {
        let permits = match cfg.mode {
            TransformMode::Spawn => 1,
            TransformMode::Persistent => cfg.effective_pool_max() as usize,
        };
        Self {
            cfg,
            state: Arc::new(Mutex::new(PoolState {
                idle: Vec::new(),
                next_worker_id: 0,
            })),
            permits: Semaphore::new(permits),
            reaper_started: AtomicBool::new(false),
        }
    }

    /// 单请求转换：写信封行、读回信封行。worker 取自空闲表（persistent）或
    /// 现场启动（spawn / 无空闲）；用毕归还或按模式丢弃。
    ///
    /// 取自空闲表的 worker 若在产出任何输出前就失败（空闲期间崩溃/被杀/自行
    /// 退出、恰在取出探测之后才死），丢弃并**新 spawn 一个**重试一次：这是池
    /// 状态问题（拿到了死 worker），与本请求内容无关，format 也尚未对本请求
    /// 产出任何东西，重做安全。上限 1 次——新 spawn 的 worker 仍失败说明
    /// format 本身起不来（崩溃型 format），照旧直接返回，避免循环拉起。
    /// format 自报的 error 行、协议错误、超时一律不重试（同一输入重来大概率
    /// 同样失败，且超时重试会把等待时间翻倍）。
    pub(crate) async fn convert(
        &self,
        mut env: TransformEnvelope,
    ) -> Result<TransformEnvelope, TransformError> {
        self.ensure_reaper_started().await;
        let _permit = self
            .permits
            .acquire()
            .await
            .map_err(|_| TransformError::Closed)?;

        let (worker, reused) = match self.take_idle_worker().await {
            Some(w) => (w, true),
            None => (self.spawn_worker().await?, false),
        };
        match self.exchange(worker, &mut env).await {
            Exchange::Done(result) => result,
            Exchange::DeadBeforeOutput(e) if reused => {
                tracing::warn!(error = %e, "复用的空闲 worker 已失效（未产出任何输出），换新 worker 重试一次");
                let fresh = self.spawn_worker().await?;
                match self.exchange(fresh, &mut env).await {
                    Exchange::Done(result) => result,
                    Exchange::DeadBeforeOutput(e) => Err(e),
                }
            }
            Exchange::DeadBeforeOutput(e) => Err(e),
        }
    }

    /// 从空闲表取一个可用 worker：取出时先剔除已退出的、以及 stdout 冒出了
    /// 未被请求的输出的（见 [`probe_stdout`]）。全部不可用返回 None（调用方
    /// 现场 spawn）。
    ///
    /// 取出时的存活检查只是低成本前置过滤（省一次往死管道写入的失败往返），
    /// 有 TOCTOU 竞态——检查后才死的 worker 由 convert 的「产出前失败重试
    /// 一次」兜底，两者缺一不可。
    async fn take_idle_worker(&self) -> Option<Worker> {
        loop {
            // **先释放锁再做后续**：pop 的 MutexGuard 是本语句的临时值，语句
            // 结束即释放——若写成 `match self.state.lock().await.idle.pop()`，
            // guard 会活到整个 match 结束，调用方随后 spawn_worker() 再拿同一
            // 把锁就是自死锁（tokio Mutex 不可重入）
            let mut worker = self.state.lock().await.idle.pop()?;
            if !matches!(worker.child.try_wait(), Ok(None)) {
                tracing::info!(worker_id = worker.worker_id, "空闲 worker 已退出，剔除");
                discard_worker(worker);
                continue;
            }
            match probe_stdout(&mut worker.stdout) {
                StdoutProbe::Quiet => return Some(worker),
                StdoutProbe::Closed => {
                    tracing::info!(
                        worker_id = worker.worker_id,
                        "空闲 worker 的 stdout 已关闭，剔除"
                    );
                }
                StdoutProbe::Stray => {
                    // 不记录内容本身：多余输出可能是 format 打出的信封（含鉴权头）
                    tracing::warn!(
                        worker_id = worker.worker_id,
                        "空闲 worker 的 stdout 冒出了未被请求的输出（format 违反「一请求一行」：\
                         stdout 只能写信封行，日志请写 stderr），剔除以免下一个请求读到错位输出"
                    );
                }
            }
            discard_worker(worker);
        }
    }

    /// 在给定 worker 上做一次往返，并按结局处置 worker：确认 stdout 同步完好
    /// 才归还空闲表，其余一律剔除（见模块文档「stdout 同步不变量」）。
    async fn exchange(&self, mut worker: Worker, env: &mut TransformEnvelope) -> Exchange {
        env.worker_id = worker.worker_id;
        let line = match env.to_line() {
            Ok(l) => l,
            Err(e) => {
                discard_worker(worker);
                return Exchange::Done(Err(TransformError::Serialize(e)));
            }
        };

        /// 往返进行到哪一步失败——决定「是否已产出输出」。
        enum Phase {
            Line(Vec<u8>),
            Eof,
            WriteFailed(std::io::Error),
            ReadFailed { err: std::io::Error, partial: bool },
        }
        let timeout_secs = self.cfg.effective_timeout_secs();
        // 写 + 读一次往返；timeout 包裹整个往返——超时的 worker 不可信（可能
        // 仍在消化旧输入），必须 kill 剔除。按字节读到 \n（而非 read_line）：
        // 非 UTF-8 输出要归为协议错误，read_line 会把它混成 IO 错误
        let io = async {
            let write = async {
                worker.stdin.write_all(line.as_bytes()).await?;
                worker.stdin.write_all(b"\n").await?;
                worker.stdin.flush().await
            };
            if let Err(e) = write.await {
                return Phase::WriteFailed(e);
            }
            let mut buf = Vec::new();
            match worker.stdout.read_until(b'\n', &mut buf).await {
                Ok(0) => Phase::Eof,
                Ok(_) => Phase::Line(buf),
                Err(err) => Phase::ReadFailed {
                    err,
                    partial: !buf.is_empty(),
                },
            }
        };
        let phase = if timeout_secs == 0 {
            io.await
        } else {
            match tokio::time::timeout(Duration::from_secs(timeout_secs), io).await {
                Ok(p) => p,
                Err(_) => {
                    discard_worker(worker);
                    return Exchange::Done(Err(TransformError::TimedOut));
                }
            }
        };

        let bytes = match phase {
            Phase::Line(bytes) => bytes,
            Phase::Eof => {
                // EOF：进程退出且无输出（协议义务是遇 EOF 退出，读到 EOF 无输出
                // 说明进程刚死）
                discard_worker(worker);
                return Exchange::DeadBeforeOutput(TransformError::WorkerDied);
            }
            Phase::WriteFailed(e) => {
                discard_worker(worker);
                return Exchange::DeadBeforeOutput(TransformError::Io(e));
            }
            Phase::ReadFailed {
                err,
                partial: false,
            } => {
                discard_worker(worker);
                return Exchange::DeadBeforeOutput(TransformError::Io(err));
            }
            Phase::ReadFailed { err, partial: true } => {
                // 已读到半行：format 对本请求产出过东西，不属于「产出前失败」
                discard_worker(worker);
                return Exchange::Done(Err(TransformError::Io(err)));
            }
        };

        // 信封行以 \n 结尾；trim 后解析（空行是协议错误）。**解析成功之前绝不
        // 归还**：解析失败说明这一行不是本请求的回复（横幅、日志、多行 JSON
        // 的碎片），对应关系已断，worker 必须剔除
        let parsed = std::str::from_utf8(&bytes)
            .map_err(|e| format!("输出不是 UTF-8（{e}）"))
            .and_then(|text| {
                TransformEnvelope::from_line(text.trim_end()).map_err(|e| e.to_string())
            });
        let out = match parsed {
            Ok(out) => out,
            Err(detail) => {
                discard_worker(worker);
                return Exchange::Done(Err(TransformError::Protocol(format!(
                    "{detail}；stdout 只能写信封行（横幅/日志请写 stderr，JSON 须紧凑单行），\
                     该 worker 已剔除"
                ))));
            }
        };

        // spawn 模式（一次性）或进程已退（写完就退）：用毕即弃，多余输出随进程
        // 一并丢弃，不存在错位风险，不必再查
        let alive = matches!(worker.child.try_wait(), Ok(None));
        if !alive || self.cfg.mode != TransformMode::Persistent {
            discard_worker(worker);
            return Exchange::Done(Ok(out));
        }
        match probe_stdout(&mut worker.stdout) {
            StdoutProbe::Quiet => {
                worker.last_used = Instant::now();
                self.state.lock().await.idle.push(worker);
                Exchange::Done(Ok(out))
            }
            StdoutProbe::Closed => {
                // 写完就退：本行合法照收，进程收尾丢弃
                discard_worker(worker);
                Exchange::Done(Ok(out))
            }
            StdoutProbe::Stray => {
                // 一个请求回了不止一行：哪一行才是本请求的回复已无从判定
                // （多出的可能在前也可能在后），本次结果不可信，按协议错误失败
                discard_worker(worker);
                Exchange::Done(Err(TransformError::Protocol(
                    "对单个请求输出了不止一行（每个请求必须恰好回一行信封；日志请写 stderr，\
                     JSON 须紧凑单行），该 worker 已剔除"
                        .to_string(),
                )))
            }
        }
    }

    /// 启动 worker：按配置的 command/args spawn，stdin/stdout 接管、stderr
    /// 弃之（format 的排障输出进守护日志无意义，崩溃原因按退出码上报）。
    async fn spawn_worker(&self) -> Result<Worker, TransformError> {
        let mut cmd = tokio::process::Command::new(&self.cfg.command);
        cmd.args(&self.cfg.args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            // 兜底：任何提前 return / panic 路径子进程被 kill（Worker 的
            // stdin/stdout 被 drop 时管道关闭，persistent worker 按协议义务
            // 自行 exit；kill_on_drop 兜底不吃 EOF 的挂死进程）
            .kill_on_drop(true);
        let mut child = cmd.spawn().map_err(TransformError::Spawn)?;
        let stdin = child.stdin.take().ok_or(TransformError::WorkerDied)?;
        let stdout = child.stdout.take().ok_or(TransformError::WorkerDied)?;
        let worker_id = {
            let mut st = self.state.lock().await;
            // 自增取模分配槽位，基数与 permits 同源：spawn 模式恒 0（协议契约
            // 「一次性模式 worker_id 恒 0」——信封文档与 skill 都按此声明，
            // 轮换类 format 依赖它区分模式语义）；persistent 池内各 worker
            // 拿到稳定不同的槽位号（0..pool_max）
            let modulus = match self.cfg.mode {
                TransformMode::Spawn => 1,
                TransformMode::Persistent => self.cfg.effective_pool_max(),
            };
            let id = st.next_worker_id;
            st.next_worker_id = (st.next_worker_id + 1) % modulus;
            id
        };
        Ok(Worker {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            worker_id,
            last_used: Instant::now(),
        })
    }

    /// 惰性启动空闲回收 reaper（仅 persistent + idle_timeout_secs > 0）。
    async fn ensure_reaper_started(&self) {
        if self.cfg.mode != TransformMode::Persistent {
            return;
        }
        if self.reaper_started.swap(true, AtomicOrdering::AcqRel) {
            return;
        }
        let idle_secs = self.cfg.effective_idle_timeout_secs();
        if idle_secs == 0 {
            return; // 0 = 永不回收
        }
        // 单 reaper 任务扫描（非每 worker sleep 竞速）：无 per-worker 任务爆炸
        // 与取消簿记；池上限个位数量级，扫描 O(空闲数) 无性能问题
        let period = Duration::from_secs((idle_secs / 2).clamp(1, 5));
        let state = Arc::clone(&self.state);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(period).await;
                let now = Instant::now();
                let expired: Vec<Worker> = state
                    .lock()
                    .await
                    .idle
                    .extract_if(.., |w| {
                        now.duration_since(w.last_used) >= Duration::from_secs(idle_secs)
                    })
                    .collect();
                for w in expired {
                    tracing::info!(worker_id = w.worker_id, "format worker 空闲超时，回收");
                    discard_worker(w);
                }
            }
        });
    }
}

/// 丢弃 worker：kill + wait 收尾。kill_on_drop 兜底，wait 在 spawn 的任务里
/// 完成（unix 防 zombie；kill 后进程很快退出，任务即刻结束）。
fn discard_worker(mut worker: Worker) {
    let _ = worker.child.start_kill();
    tokio::spawn(async move {
        let _ = worker.child.wait().await;
    });
}

/// 头表 → 信封：键小写（HeaderMap 迭代即小写）、跳过 hop-by-hop 头（含
/// content-length——由 aproxy 按实际字节回填，format 不该看到也不能改）。
/// **多值头仅保留首值**（set-cookie 的 Expires 含逗号，逗号合并不合法；拒绝
/// 转换代价失衡——LLM API 请求侧无多值头，响应侧 set-cookie 在代理场景无
/// 会话意义）；非可见 ASCII 的头值丢弃 + warn。
fn headers_to_btreemap(headers: &axum::http::HeaderMap) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for (name, value) in headers.iter() {
        let key = name.as_str().to_string();
        if is_hop_header(&key) {
            continue;
        }
        let Ok(v) = value.to_str() else {
            tracing::warn!(header = %key, "头值非可见 ASCII，转换器信封中丢弃");
            continue;
        };
        match map.entry(key) {
            Entry::Occupied(_) => {
                tracing::warn!(header = %name, "多值头仅保留首值，后续值不进信封");
            }
            Entry::Vacant(e) => {
                e.insert(v.to_string());
            }
        }
    }
    map
}

/// 信封头表 → HeaderMap（整表替换）：非法头名/头值丢弃 + warn；hop-by-hop
/// 与 content-length 防御性再过滤（format 不该回传它们，回了也不生效）。
fn headers_from_btreemap(map: &BTreeMap<String, String>) -> axum::http::HeaderMap {
    let mut headers = axum::http::HeaderMap::new();
    for (k, v) in map {
        if is_hop_header(k) || k.eq_ignore_ascii_case("content-length") {
            tracing::warn!(header = %k, "format 输出的头属自动管理范畴，忽略");
            continue;
        }
        let Ok(name) = axum::http::HeaderName::from_bytes(k.as_bytes()) else {
            tracing::warn!(header = %k, "format 输出的头名非法，丢弃");
            continue;
        };
        let Ok(val) = axum::http::HeaderValue::from_str(v) else {
            tracing::warn!(header = %k, "format 输出的头值非法，丢弃");
            continue;
        };
        headers.insert(name, val);
    }
    headers
}

/// 转换后的字节 → RequestBody：超内存驻留阈值且有 spool 目录则落盘成 Disk
/// 形态（复用 spool 临时文件命名与 Drop 自清理），写失败降级内存。
async fn spool_request_body(bytes: &[u8], spool_dir: Option<&Path>) -> RequestBody {
    if bytes.len() > RESIDENT_LIMIT
        && let Some(dir) = spool_dir
    {
        let path = dir.join(format!(
            "req-{}-{}.spooltmp",
            std::process::id(),
            unique_seq()
        ));
        if let Ok(mut file) = tokio::fs::File::create(&path).await {
            if file.write_all(bytes).await.is_ok() && file.flush().await.is_ok() {
                return RequestBody::Disk {
                    path,
                    len: bytes.len() as u64,
                };
            }
            // 半截文件必须删除
            drop(file);
            let _ = std::fs::remove_file(&path);
        }
    }
    RequestBody::Memory(bytes::Bytes::copy_from_slice(bytes))
}

/// 转换后的字节 → SpooledBody（同 spool_request_body 的落盘语义）。
async fn spool_response_body(bytes: &[u8], spool_dir: Option<&Path>) -> SpooledBody {
    if bytes.len() > RESIDENT_LIMIT
        && let Some(dir) = spool_dir
    {
        let path = dir.join(format!(
            "res-{}-{}.spooltmp",
            std::process::id(),
            unique_seq()
        ));
        if let Ok(mut file) = tokio::fs::File::create(&path).await {
            if file.write_all(bytes).await.is_ok() && file.flush().await.is_ok() {
                return SpooledBody::Disk {
                    path,
                    len: bytes.len() as u64,
                };
            }
            drop(file);
            let _ = std::fs::remove_file(&path);
        }
    }
    SpooledBody::Memory(bytes::Bytes::copy_from_slice(bytes))
}

/// 请求侧转换产物：method/url/headers/body 全部可被 format 改写。
pub(crate) struct TransformedRequest {
    pub method: axum::http::Method,
    pub url: String,
    pub headers: axum::http::HeaderMap,
    pub body: RequestBody,
    /// format 回信里的跨阶段状态；None = 回信未带，调用方保留原值
    pub state: Option<String>,
}

/// 一个客户端请求交给各转换阶段的标识与跨阶段状态（见信封的 `request_id` /
/// `state`）。随请求在代理通道里传递：请求转换回信的 state 替换这里的值，
/// 响应转换收到的就是它。
#[derive(Debug, Clone, Default)]
pub(crate) struct ExchangeCtx {
    pub request_id: String,
    pub state: Option<String>,
}

impl ExchangeCtx {
    /// 填信封的阶段字段
    fn stamp(&self, env: &mut TransformEnvelope, stage: &str) {
        env.stage = Some(stage.to_string());
        env.request_id = Some(self.request_id.clone());
        env.state = self.state.clone();
    }
}

/// 请求侧转换：body 缓冲完成后交给 format 改写。转换**一次**，产物被重试
/// 循环的每一轮 forward_once 自动重放（调用侧零分支）。
///
/// `body` 所有权接管（Disk 形态读毕由 Drop 自删临时文件）；输出超大时重新
/// 落盘成新 Disk 形态（内存与负载解耦的语义保持）。
pub(crate) async fn transform_request(
    pool: &Arc<TransformPool>,
    ctx: &ExchangeCtx,
    method: &axum::http::Method,
    url: &str,
    headers: &axum::http::HeaderMap,
    body: RequestBody,
    spool_dir: Option<&Path>,
) -> Result<TransformedRequest, TransformError> {
    let body_bytes = match &body {
        RequestBody::Memory(b) => b.to_vec(),
        RequestBody::Disk { path, .. } => {
            tokio::fs::read(path).await.map_err(TransformError::Spool)?
        }
    };
    drop(body); // Disk 文件由 Drop 删除（已被读出）

    let mut env = TransformEnvelope::from_body_bytes(&body_bytes);
    env.method = Some(method.as_str().to_string());
    env.url = Some(url.to_string());
    env.headers = headers_to_btreemap(headers);
    env.extra = pool.cfg.effective_extra().to_string();
    ctx.stamp(&mut env, "request");

    let out = pool.convert(env).await?;
    if let Some(err) = out.error {
        return Err(TransformError::Rejected(err));
    }
    // body_b64 解码失败是 format 输出写错（协议错误），不是 format 的业务判定。
    // worker 已在 convert 内归还：该行本身是一行合法信封，stdout 帧同步未受
    // 影响，无需剔除
    let resp_bytes = out
        .body_bytes()
        .map_err(|e| TransformError::Protocol(e.to_string()))?;
    let new_body = spool_request_body(&resp_bytes, spool_dir).await;
    let new_method = match out
        .method
        .as_deref()
        .map(|m| m.parse::<axum::http::Method>())
    {
        Some(Ok(m)) => m,
        // 与本函数族其他「format 输出不合法」路径一致（非法头名/头值逐条
        // warn）：输出 method 非法 HTTP token 时回退原方法并留痕——静默回退
        // 会让 format 本意的 DELETE 变成 POST，排障必须能看到这次改写被丢。
        // 字段缺省（None）= 沿用原方法（信封契约），不告警
        Some(Err(_)) => {
            tracing::warn!(
                method = %out.method.as_deref().unwrap_or(""),
                "format 输出的 method 非法，沿用原方法"
            );
            method.clone()
        }
        None => method.clone(),
    };
    let new_url = out.url.unwrap_or_else(|| url.to_string());
    let new_headers = headers_from_btreemap(&out.headers);
    Ok(TransformedRequest {
        method: new_method,
        url: new_url,
        headers: new_headers,
        body: new_body,
        state: out.state,
    })
}

/// 响应侧转换产物。失败时 `error` 为 Some 且 headers/body 已还原为**原始值**
/// （调用方透传上游原始响应——响应在手，可用性优先）。
pub(crate) struct TransformedResponse {
    pub headers: axum::http::HeaderMap,
    pub body: SpooledBody,
    pub error: Option<TransformError>,
}

/// 响应侧转换：重试判定为「成功」后、回放前交给 format 改写。
///
/// 压缩体先经 `decode::for_inspection` 解码（判定同款；超 8MiB 上限回退原始
/// 字节——>8MiB 的压缩 LLM 响应现实中不存在，见 decode.rs 的常数注释）。
/// 输出头强制剔除 content-length/content-encoding——字节已变换，旧声明必失真；
/// content-length 由回放路径按新长度回填或由 hyper 按实际字节自行分帧。
pub(crate) async fn transform_response(
    pool: &Arc<TransformPool>,
    ctx: &ExchangeCtx,
    upstream_url: &str,
    content_encoding: Option<&str>,
    headers: axum::http::HeaderMap,
    body: SpooledBody,
    spool_dir: Option<&Path>,
) -> TransformedResponse {
    let (raw, body) = match &body {
        SpooledBody::Memory(b) => (b.to_vec(), body),
        SpooledBody::Disk { path, .. } => match tokio::fs::read(path).await {
            Ok(b) => (b, body),
            Err(e) => {
                // 磁盘文件丢失：无法转换也无法透传 body，原样归还空错误由
                // 调用方按其回放路径处理（into_stream 已有该路径的空 body 回退）
                tracing::warn!(error = %e, "响应 spool 文件读取失败，跳过响应转换");
                return TransformedResponse {
                    headers,
                    body,
                    error: Some(TransformError::Spool(e)),
                };
            }
        },
    };
    let decoded = crate::decode::for_inspection(content_encoding, &raw);
    let feed = decoded.as_deref().unwrap_or(&raw);

    let mut env = TransformEnvelope::from_body_bytes(feed);
    // 响应侧信封 url = 请求侧最终上游地址：多渠道聚合可按它反查渠道表。响应
    // 转换器与请求转换器是不同进程、无共享内存（设计铁律）；需要请求侧的更多
    // 信息时由请求转换器写进 state，经 ctx 转交到这里
    env.url = Some(upstream_url.to_string());
    env.headers = headers_to_btreemap(&headers);
    env.extra = pool.cfg.effective_extra().to_string();
    ctx.stamp(&mut env, "response");

    let out = match pool.convert(env).await {
        Ok(o) => o,
        Err(e) => {
            return TransformedResponse {
                headers,
                body,
                error: Some(e),
            };
        }
    };
    if let Some(err) = out.error {
        return TransformedResponse {
            headers,
            body,
            error: Some(TransformError::Rejected(err)),
        };
    }
    let resp_bytes = match out.body_bytes() {
        Ok(b) => b,
        Err(e) => {
            return TransformedResponse {
                headers,
                body,
                // 同请求侧：body_b64 解码失败归协议错误
                error: Some(TransformError::Protocol(e.to_string())),
            };
        }
    };
    let mut new_headers = headers_from_btreemap(&out.headers);
    new_headers.remove(axum::http::header::CONTENT_LENGTH);
    new_headers.remove(axum::http::header::CONTENT_ENCODING);
    let new_body = spool_response_body(&resp_bytes, spool_dir).await;
    TransformedResponse {
        headers: new_headers,
        body: new_body,
        error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool_for(command: &str, args: &[&str], mode: TransformMode) -> TransformPool {
        TransformPool::new(Arc::new(TransformConfig {
            command: command.to_string(),
            args: args.iter().map(|s| s.to_string()).collect(),
            mode,
            ..Default::default()
        }))
    }

    #[tokio::test]
    async fn spawn_mode_single_request_roundtrip() {
        // 用测试用 echo 假 format：写一行回一行后退出（spawn 模式常态）
        // 这里直接用本测试二进制的父进程 mock 太复杂，用内置 PowerShell/
        // bash 不跨平台——单测只验证信封序列化与错误路径，进程行为由集成
        // 测试（examples/format-echo.rs）覆盖
        let pool = pool_for("definitely-not-exist-abc123", &[], TransformMode::Spawn);
        let env = TransformEnvelope {
            method: Some("POST".to_string()),
            headers: BTreeMap::new(),
            body: Some("{}".to_string()),
            ..Default::default()
        };
        let err = pool.convert(env).await.unwrap_err();
        assert!(matches!(err, TransformError::Spawn(_)), "{err:?}");
    }

    #[tokio::test]
    async fn convert_smoke_real_format_echo_roundtrip() {
        // 真实 spawn format-echo 的最小往返（不经代理栈，二分定位用）
        let name = if cfg!(windows) {
            "format-echo.exe"
        } else {
            "format-echo"
        };
        let mut dir = std::env::current_exe().unwrap();
        let mut echo = None;
        while let Some(parent) = dir.parent() {
            let cand = parent.join("examples").join(name);
            if cand.exists() {
                echo = Some(cand);
                break;
            }
            dir = parent.to_path_buf();
        }
        let echo = echo.expect("format-echo 未编译");
        let pool = TransformPool::new(Arc::new(TransformConfig {
            command: echo.display().to_string(),
            args: vec!["echo".to_string()],
            mode: TransformMode::Spawn,
            timeout_secs: Some(5),
            ..Default::default()
        }));
        let env = TransformEnvelope {
            method: Some("POST".to_string()),
            headers: BTreeMap::new(),
            body: Some("{\"a\":1}".to_string()),
            ..Default::default()
        };
        let out = tokio::time::timeout(Duration::from_secs(10), pool.convert(env))
            .await
            .expect("convert 整体超时（10s）")
            .expect("convert 失败");
        assert_eq!(
            out.body.as_deref(),
            Some("{\"a\":1}"),
            "echo 应原样回显 body"
        );
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

    #[test]
    fn transform_error_display_carries_reason() {
        let e = TransformError::Rejected("model 未命中".to_string());
        assert!(e.to_string().contains("model 未命中"));
        assert!(TransformError::TimedOut.to_string().contains("超时"));
        let p = TransformError::Protocol("missing field `headers`".to_string());
        assert!(p.to_string().contains("违反信封协议"), "{p}");
        assert!(p.to_string().contains("missing field"), "{p}");
    }

    #[tokio::test]
    async fn spawn_mode_pool_max_is_one() {
        let pool = pool_for("x", &[], TransformMode::Spawn);
        assert_eq!(pool.cfg.effective_pool_max(), 4); // 未配置回退默认
        assert_eq!(pool.permits.available_permits(), 1); // spawn 模式恒 1
    }
    /// 定位 examples/format-echo(.exe)（与集成测试同款定位逻辑）。
    fn format_echo_path() -> std::path::PathBuf {
        let name = if cfg!(windows) {
            "format-echo.exe"
        } else {
            "format-echo"
        };
        let mut dir = std::env::current_exe().unwrap();
        while let Some(parent) = dir.parent() {
            let cand = parent.join("examples").join(name);
            if cand.exists() {
                return cand;
            }
            dir = parent.to_path_buf();
        }
        panic!("format-echo 未编译");
    }

    fn echo_pool(
        mode: TransformMode,
        sub: &str,
        extra_cfg: impl FnOnce(&mut TransformConfig),
    ) -> TransformPool {
        let mut cfg = TransformConfig {
            command: format_echo_path().display().to_string(),
            args: vec![sub.to_string()],
            mode,
            timeout_secs: Some(5),
            ..Default::default()
        };
        extra_cfg(&mut cfg);
        TransformPool::new(Arc::new(cfg))
    }

    fn envelope_with(body: &str) -> TransformEnvelope {
        TransformEnvelope {
            method: Some("POST".to_string()),
            headers: BTreeMap::new(),
            body: Some(body.to_string()),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn persistent_mode_reuses_worker_across_requests() {
        // echo 原样回显信封（含 aproxy 填的 worker_id）：同 worker_id 复现 =
        // 复用同一 worker；池槽位稳定（不随请求增长 spawn）
        let pool = echo_pool(TransformMode::Persistent, "echo", |_| {});
        let first = pool.convert(envelope_with("r1")).await.unwrap();
        let second = pool.convert(envelope_with("r2")).await.unwrap();
        assert_eq!(
            first.worker_id, second.worker_id,
            "persistent 应复用同 worker"
        );
        assert_eq!(first.body.as_deref(), Some("r1"));
        assert_eq!(second.body.as_deref(), Some("r2"));
        assert_eq!(
            pool.state.lock().await.idle.len(),
            1,
            "空闲表恒持一个 worker"
        );
    }

    #[tokio::test]
    async fn idle_worker_is_reaped_after_timeout() {
        // idle_timeout_secs=1：reaper 周期 min(1/2,5).clamp(1,5)=1s——首个 worker
        // 回收后下次请求 spawn 新 worker（worker_id 递进）
        let pool = echo_pool(TransformMode::Persistent, "echo", |c| {
            c.idle_timeout_secs = Some(1);
        });
        let first = pool.convert(envelope_with("a")).await.unwrap();
        tokio::time::sleep(Duration::from_millis(2600)).await;
        let idle_before = pool.state.lock().await.idle.len();
        assert_eq!(idle_before, 0, "空闲 worker 应被 reaper 回收");
        let second = pool.convert(envelope_with("b")).await.unwrap();
        assert_ne!(
            first.worker_id, second.worker_id,
            "回收后应 spawn 新 worker（pool_max 默认 4，槽位递进）"
        );
    }

    #[tokio::test]
    async fn crashed_worker_is_evicted_and_next_request_respawns() {
        // exit1 读行后即退：每次 convert 都是「新 spawn → EOF」——两次错误互不
        // 干扰且池不残留坏 worker（崩溃剔除路径）
        let pool = echo_pool(TransformMode::Persistent, "exit1", |_| {});
        let e1 = pool.convert(envelope_with("x")).await.unwrap_err();
        assert!(matches!(e1, TransformError::WorkerDied), "{e1:?}");
        let e2 = pool.convert(envelope_with("y")).await.unwrap_err();
        assert!(matches!(e2, TransformError::WorkerDied));
        assert_eq!(
            pool.state.lock().await.idle.len(),
            0,
            "崩溃 worker 不进空闲表"
        );
    }

    #[tokio::test]
    async fn per_request_timeout_evicts_stuck_worker() {
        // sleep helper 睡 3s > timeout 1s：Err(TimedOut) 且 worker 被剔除
        let pool = echo_pool(TransformMode::Persistent, "sleep", |c| {
            c.args = vec!["sleep".to_string(), "3000".to_string()];
            c.timeout_secs = Some(1);
        });
        let err = pool.convert(envelope_with("s")).await.unwrap_err();
        assert!(matches!(err, TransformError::TimedOut), "{err:?}");
        assert_eq!(
            pool.state.lock().await.idle.len(),
            0,
            "超时 worker 不得归还"
        );
    }

    #[tokio::test]
    async fn concurrent_requests_within_pool_max() {
        // pool_max=2：两并发各得一个 worker（spawn 模式恒 0，persistent 下
        // 槽位分配 0/1 交错），互不阻塞、各自成功
        let pool = Arc::new(echo_pool(TransformMode::Persistent, "echo", |c| {
            c.pool_max = Some(2);
        }));
        let p1 = Arc::new(&pool);
        let p2 = Arc::clone(&p1);
        let (r1, r2) = tokio::join!(async { p1.convert(envelope_with("c1")).await }, async {
            p2.convert(envelope_with("c2")).await
        },);
        let a = r1.unwrap();
        let b = r2.unwrap();
        assert_ne!(a.worker_id, b.worker_id, "两并发应各占一个槽位");
        let ids = [a.worker_id, b.worker_id];
        assert!(
            ids.contains(&0) && ids.contains(&1),
            "槽位应为 0/1: {ids:?}"
        );
    }

    #[tokio::test]
    async fn spawn_mode_borrowed_worker_never_reused() {
        // spawn 模式同 worker_id 恒 0，但进程是每次新 spawn（用毕即弃）——
        // 断言两次都成功即可（复用与否由 permits=1 + discard 路径保证）
        let pool = echo_pool(TransformMode::Spawn, "echo", |_| {});
        let a = pool.convert(envelope_with("1")).await.unwrap();
        let b = pool.convert(envelope_with("2")).await.unwrap();
        assert_eq!(a.worker_id, 0);
        assert_eq!(b.worker_id, 0);
        assert_eq!(
            pool.state.lock().await.idle.len(),
            0,
            "spawn 模式 idle 恒空"
        );
    }
    #[tokio::test]
    async fn idle_zero_means_never_reaped() {
        // idle_timeout_secs=0（永不回收）：等待超过 reaper 典型周期后空闲表
        // 仍保有 worker——0 的「永不回收」语义不得被误当成「立即回收」
        let pool = echo_pool(TransformMode::Persistent, "echo", |c| {
            c.idle_timeout_secs = Some(0);
        });
        pool.convert(envelope_with("keep")).await.unwrap();
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert_eq!(
            pool.state.lock().await.idle.len(),
            1,
            "idle=0 永不回收：worker 应仍在空闲表"
        );
    }

    #[tokio::test]
    async fn banner_line_is_protocol_error_and_evicts_worker() {
        // 横幅先于回复：读到的首行不是信封 → Protocol（不是 Rejected——这不是
        // format 的业务判定）且 worker 必须剔除：留在池里，下一个请求会读到
        // 本请求的回复
        let pool = echo_pool(TransformMode::Persistent, "banner", |_| {});
        for body in ["b1", "b2"] {
            let err = pool.convert(envelope_with(body)).await.unwrap_err();
            assert!(matches!(err, TransformError::Protocol(_)), "{err:?}");
            assert_eq!(
                pool.state.lock().await.idle.len(),
                0,
                "协议错误的 worker 不得归还"
            );
        }
    }

    #[tokio::test]
    async fn multiline_reply_in_one_batch_is_protocol_error() {
        // 两行同批到达：读完首行时缓冲里还有残行——一个请求回了多行，哪行是
        // 本请求的回复已无从判定，按协议错误失败并剔除（绝不能归还后让下一个
        // 请求读到残行）
        let pool = echo_pool(TransformMode::Persistent, "multiline", |_| {});
        for body in ["m1", "m2", "m3"] {
            let err = pool.convert(envelope_with(body)).await.unwrap_err();
            assert!(matches!(err, TransformError::Protocol(_)), "{err:?}");
            assert_eq!(pool.state.lock().await.idle.len(), 0);
        }
    }

    #[tokio::test]
    async fn stray_line_arriving_while_idle_is_evicted_before_reuse() {
        // 多余行晚于回复到达（worker 已在空闲表）：取用前探测到并剔除，换新
        // worker 处理本请求——本请求拿到的必须是自己的回显
        let pool = echo_pool(TransformMode::Persistent, "late-stray", |c| {
            c.args = vec!["late-stray".to_string(), "50".to_string()];
            c.pool_max = Some(1);
        });
        let first = pool.convert(envelope_with("s1")).await.unwrap();
        assert_eq!(first.body.as_deref(), Some("s1"));
        tokio::time::sleep(Duration::from_millis(400)).await;
        let second = pool.convert(envelope_with("s2")).await.unwrap();
        assert_eq!(
            second.body.as_deref(),
            Some("s2"),
            "读到了上一个请求的多余行（串包）"
        );
    }

    #[tokio::test]
    async fn error_line_keeps_worker_in_pool() {
        // format 自报 error 行是一行合法信封：stdout 同步完好，worker 照常
        // 归还（不能因「本请求失败」误伤健康 worker，否则轮换计数会被重置）
        let pool = echo_pool(TransformMode::Persistent, "error", |_| {});
        let out = pool.convert(envelope_with("e")).await.unwrap();
        assert!(out.error.is_some());
        assert_eq!(pool.state.lock().await.idle.len(), 1);
    }

    #[tokio::test]
    async fn idle_worker_that_exited_is_skipped_on_take() {
        // die-idle：空闲 100ms 自行退出。取用时剔除死 worker、现场 spawn，
        // 请求照常成功
        let pool = echo_pool(TransformMode::Persistent, "die-idle", |c| {
            c.args = vec!["die-idle".to_string(), "100".to_string()];
        });
        pool.convert(envelope_with("d1")).await.unwrap();
        tokio::time::sleep(Duration::from_millis(800)).await;
        let out = pool.convert(envelope_with("d2")).await.unwrap();
        assert_eq!(out.body.as_deref(), Some("d2"));
    }

    #[tokio::test]
    async fn reused_worker_dead_before_output_retries_on_fresh_worker() {
        // oneshot：复用的 worker 写入成功后无输出退出（读到 EOF）——取出时它
        // 还活着，存活检查拦不住，只能靠「产出前失败 → 新 worker 重试一次」
        let pool = echo_pool(TransformMode::Persistent, "oneshot", |_| {});
        pool.convert(envelope_with("o1")).await.unwrap();
        let out = pool.convert(envelope_with("o2")).await.unwrap();
        assert_eq!(out.body.as_deref(), Some("o2"));
    }

    #[tokio::test]
    async fn retry_is_bounded_when_fresh_worker_also_dies() {
        // marker 已存在时 oneshot 首行即无输出退出：重试用的新 worker 同样
        // 失败 → 直接返回 WorkerDied，不得再拉起第三个（崩溃型 format 循环）。
        // marker 行数 = 启动次数；外层 timeout 兜住「无限重试」形态的回归
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("spawned");
        let pool = echo_pool(TransformMode::Persistent, "oneshot", |c| {
            c.args = vec!["oneshot".to_string(), marker.display().to_string()];
        });
        pool.convert(envelope_with("first")).await.unwrap();
        let err = tokio::time::timeout(Duration::from_secs(15), pool.convert(envelope_with("x")))
            .await
            .expect("重试未收敛（疑似无限重试）")
            .unwrap_err();
        assert!(matches!(err, TransformError::WorkerDied), "{err:?}");
        assert_eq!(pool.state.lock().await.idle.len(), 0);
        let spawns = std::fs::read_to_string(&marker).unwrap().lines().count();
        assert_eq!(spawns, 2, "应恰好：首个 worker + 重试用的 1 个新 worker");
    }

    #[tokio::test]
    async fn fresh_worker_failure_is_not_retried() {
        // 新 spawn 的 worker 产出前失败不重试：marker 预先存在 → 首个 worker
        // 就起不来，直接 WorkerDied（只有「取自空闲表」的 worker 才有重试资格）
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("spawned");
        std::fs::write(&marker, b"").unwrap();
        let pool = echo_pool(TransformMode::Persistent, "oneshot", |c| {
            c.args = vec!["oneshot".to_string(), marker.display().to_string()];
        });
        let err = pool.convert(envelope_with("y")).await.unwrap_err();
        assert!(matches!(err, TransformError::WorkerDied), "{err:?}");
        let spawns = std::fs::read_to_string(&marker).unwrap().lines().count();
        assert_eq!(spawns, 1, "新 spawn 的 worker 失败不得再拉起第二个");
    }

    #[tokio::test]
    async fn pool_max_one_serializes_on_single_worker() {
        // pool_max=1：两个请求串行复用同一 worker，槽位恒 0，无并发扩容
        let pool = echo_pool(TransformMode::Persistent, "echo", |c| {
            c.pool_max = Some(1);
        });
        let a = pool.convert(envelope_with("1")).await.unwrap();
        let b = pool.convert(envelope_with("2")).await.unwrap();
        assert_eq!(a.worker_id, 0);
        assert_eq!(b.worker_id, 0);
        assert_eq!(pool.state.lock().await.idle.len(), 1);
    }
}
