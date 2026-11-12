//! 拉起守护实例并等它就绪。start、restart、restore、安装的滚动重启、看护者重拉都经这一处，
//! 就绪的判定与「启动即死」的报告因此只有一份。
//!
//! 就绪的判定：按 spawn 返回的新 pid 在注册表里找到它的记录（守护 bind 成功之后才写注册表），
//! 再对记录里的实际端口做 IPC ping，应答的必须就是这个 pid。不按端口轮询：配置可能已被用户
//! 改到别的端口、或监听端口 0 由系统分配，ping 原端口要么永远不通，要么 ping 到别的进程
//! （restart 换端口误报「未就绪」的实测教训）；pid 是唯一可靠的锚点。
//!
//! 子进程没有控制台，配置错误、端口绑定失败这类原因只会写进 startup.log（全部实例共用的追加
//! 文件）。拉起之前记下它的长度，失败时只取这次新增的内容，免得把陈年旧账当成这次的原因。

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::daemon::{self, InstanceStatus};

/// 等新实例就绪的时限：冷启动受杀毒软件对新进程的扫描等影响，可能要好几秒
pub const READY_TIMEOUT: Duration = Duration::from_secs(8);

/// 拉起失败的原因
#[derive(Debug)]
pub enum SpawnError {
    /// spawn 本身失败（可执行文件不在、没有权限等）
    Spawn(std::io::Error),
    /// 新进程在就绪之前就退出了：配置错误、端口被占这类确定性错误在毫秒级就会退出，
    /// 不必空等到时限
    Exited { pid: u32, startup_log: StartupLog },
    /// 时限内没有就绪。`start` 是 spawn 之后立刻读到的进程创建时间（身份锚点）：回收卡住的
    /// 子进程时按「pid + 创建时间」核验再终止，等待期间 pid 若已被系统复用给别的进程，届时
    /// 现读的创建时间必然对不上，绝不误杀；None = 没读到（进程瞬间退出或平台不支持），此时
    /// 不要终止它。`alive` = 时限到时它确认仍在运行
    TimedOut {
        pid: u32,
        start: Option<u64>,
        alive: bool,
        startup_log: StartupLog,
    },
}

impl std::fmt::Display for SpawnError {
    /// 一句不带 startup.log 摘录的说明（摘录见 [`StartupLog::excerpt`]，由调用方决定要不要附上）
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn(e) => write!(f, "spawn 失败: {e}"),
            Self::Exited { pid, .. } => write!(f, "新实例（pid {pid}）启动后立即退出"),
            Self::TimedOut { pid, alive, .. } => write!(
                f,
                "新实例（pid {pid}）未在预期时间内就绪（{}）",
                if *alive {
                    "进程存活"
                } else {
                    "进程状态不可判"
                }
            ),
        }
    }
}

impl SpawnError {
    /// 这次拉起期间 startup.log 新增的内容（spawn 本身失败时没有）
    pub fn startup_log(&self) -> Option<&StartupLog> {
        match self {
            Self::Spawn(_) => None,
            Self::Exited { startup_log, .. } | Self::TimedOut { startup_log, .. } => {
                Some(startup_log)
            }
        }
    }
}

/// startup.log 在一次拉起期间新增的内容（最多末尾 [`StartupLog::MAX_LINES`] 行，去掉空行）
#[derive(Debug, Clone)]
pub struct StartupLog {
    pub path: PathBuf,
    pub lines: Vec<String>,
}

impl StartupLog {
    pub const MAX_LINES: usize = 20;

    /// `mark` 之后新增的内容。文件比 mark 还短（被截断或重建）时从头读。
    fn since(mark: u64) -> Self {
        use std::io::{Read, Seek, SeekFrom};
        let path = startup_log_path();
        let mut fresh = String::new();
        if let Ok(mut f) = std::fs::File::open(&path) {
            let len = f.metadata().map(|m| m.len()).unwrap_or(0);
            let from = if len >= mark { mark } else { 0 };
            if f.seek(SeekFrom::Start(from)).is_ok() {
                let mut buf = Vec::new();
                let _ = f.read_to_end(&mut buf);
                fresh = String::from_utf8_lossy(&buf).into_owned();
            }
        }
        let lines: Vec<String> = fresh
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(str::to_string)
            .collect();
        let lines = lines[lines.len().saturating_sub(Self::MAX_LINES)..].to_vec();
        Self { path, lines }
    }

    /// 可直接拼在错误说明后面的片段：有新增内容时逐行列出；没有时指路（运行日志按启动随机
    /// 命名，父进程无法预知文件名，只能指向目录）
    pub fn excerpt(&self) -> String {
        if self.lines.is_empty() {
            return format!(
                "，原因通常记录在:\n  {}\n  {}",
                self.path.display(),
                daemon::logs_dir().display()
            );
        }
        let mut out = format!("。{} 本次新增内容:", self.path.display());
        for line in &self.lines {
            out.push_str("\n  ");
            out.push_str(line);
        }
        out
    }
}

fn startup_log_path() -> PathBuf {
    daemon::logs_dir().join("startup.log")
}

fn startup_log_len() -> u64 {
    std::fs::metadata(startup_log_path())
        .map(|m| m.len())
        .unwrap_or(0)
}

/// 以 `exe` 按 `args`（再追加 `--daemon-child`）拉起一个守护实例，等它就绪，返回它对 IPC
/// ping 的应答（实际监听地址、日志路径等）。`run_dir` 是新实例将登记的注册表目录（调用方所在
/// home 的 run/）。
pub async fn spawn_instance(
    run_dir: &Path,
    exe: &Path,
    args: &[String],
    timeout: Duration,
) -> Result<InstanceStatus, SpawnError> {
    let mark = startup_log_len();
    let mut full = args.to_vec();
    full.push("--daemon-child".to_string());
    let pid = daemon::spawn_detached(exe, &full).map_err(SpawnError::Spawn)?;
    let start = crate::watchdog::process_start_time(pid);
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Some(rec) = daemon::registry_find_pid_in(run_dir, pid)
            && let Ok(live) = daemon::ipc_ping_in(run_dir, daemon::port_of(&rec.listen_addr)).await
            && live.instance.pid == pid
        {
            return Ok(live);
        }
        if crate::watchdog::process_exited(pid) == Some(true) {
            return Err(SpawnError::Exited {
                pid,
                startup_log: StartupLog::since(mark),
            });
        }
        if std::time::Instant::now() > deadline {
            return Err(SpawnError::TimedOut {
                pid,
                start,
                alive: crate::watchdog::process_exited(pid) == Some(false),
                startup_log: StartupLog::since(mark),
            });
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
