//! 按拉起规格（[`crate::spec::InstanceSpec`]）拉起后台实例并等它就绪。start、restart、
//! restore、安装的滚动重启、看护者重拉都经这一处，就绪的判定与「启动即死」的报告因此只有
//! 一份。
//!
//! 规格经交接文件交给子进程（`aproxy __serve --home <dir> --spec <file>`），密钥不进命令行；
//! 子进程的环境是拉起方的环境套上规格里记下的代理变量与 `PATH`，工作目录固定为 home，
//! 并脱离启动者（见 [`crate::daemon::spawn_detached_with`]）。
//!
//! 就绪的判定：按 spawn 返回的新 pid 在注册表里找到它的记录（守护 bind 成功之后才写注册表），
//! 再对记录里的实际端口做 IPC ping，应答的必须就是这个 pid。不按端口轮询：配置可能已被用户
//! 改到别的端口、或监听端口 0 由系统分配，ping 原端口要么永远不通，要么 ping 到别的进程
//! （restart 换端口误报「未就绪」的实测教训）；pid 是唯一可靠的锚点。
//!
//! 子进程没有控制台，配置错误、端口绑定失败这类原因会写进 startup.log（全部实例共用的追加
//! 文件）。拉起之前记下它的长度，失败时只取这次新增的内容；本次交接槽另可带有诊断参数。

/// 私有启动诊断；公开错误文档仍只带原有的码、字段与英文摘录。
#[path = "i18n/startup.rs"]
pub mod startup;
use aproxy_ipc::i18n::Lang;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::daemon::{self, InstanceStatus};
use crate::spec::{self, InstanceSpec};

/// 等新实例就绪的时限。冷启动受杀毒软件对新进程的扫描等影响，可能要好几秒；实例还要在
/// 就绪之前等它的 format 程序启动。子进程退出会立即判失败，所以这个时限只在「进程还在、
/// 迟迟不就绪」时才会用满
pub const READY_TIMEOUT: Duration = Duration::from_secs(30);

/// 拉起失败的原因
#[derive(Debug)]
pub enum SpawnError {
    /// spawn 本身失败（可执行文件不在、没有权限等）
    Spawn(std::io::Error),
    /// 新进程在就绪之前就退出了：配置错误、端口被占这类确定性错误在毫秒级就会退出，
    /// 不必空等到时限
    Exited {
        pid: u32,
        startup_log: StartupLog,
        /// 子进程离开前写回交接文件的格式诊断，与别的实例的日志无关。
        format: Option<Box<crate::transform::StartupError>>,
        /// 仅从本次拉起的规格槽读取，不从共享日志或旁观者的文件推测。
        report: Option<Box<startup::LaunchReport>>,
    },
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
        f.write_str(&self.render(Lang::En))
    }
}

impl SpawnError {
    /// 单独排版故障摘要，使清点/日志和终端可以共用原因而不共用语言。
    pub fn render(&self, lang: Lang) -> String {
        match self {
            Self::Spawn(e) => startup::spawn(lang, e),
            Self::Exited { pid, .. } => startup::exited(lang, *pid),
            Self::TimedOut { pid, alive, .. } => startup::timeout(
                lang,
                *pid,
                if *alive {
                    startup::alive(lang)
                } else {
                    startup::unknown(lang)
                },
            ),
        }
    }

    /// 公开证据始终是原始日志；只在能关联到本次交接槽时才在中文终端排版自己的记录。
    pub fn excerpt(&self, lang: Lang) -> String {
        let Some(log) = self.startup_log() else {
            return String::new();
        };
        let lines = match self {
            Self::Exited {
                report: Some(report),
                ..
            } if lang == Lang::Zh => report.localized_lines(&log.lines, lang),
            _ => None,
        };
        log.excerpt_in(lang, lines.as_deref().unwrap_or(&log.lines))
    }

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
    fn since(home: &Path, mark: u64) -> Self {
        use std::io::{Read, Seek, SeekFrom};
        let path = startup_log_path(home);
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
        self.excerpt_in(Lang::En, &self.lines)
    }

    fn excerpt_in(&self, lang: Lang, lines: &[String]) -> String {
        if lines.is_empty() {
            return startup::look(
                lang,
                self.path.display(),
                self.path
                    .parent()
                    .unwrap_or_else(|| Path::new(""))
                    .display(),
            );
        }
        let mut out = startup::excerpt(lang, self.path.display());
        for line in lines {
            out.push_str("\n  ");
            out.push_str(line);
        }
        out
    }
}

fn startup_log_path(home: &Path) -> PathBuf {
    home.join("logs").join("startup.log")
}

fn startup_log_len(home: &Path) -> u64 {
    std::fs::metadata(startup_log_path(home))
        .map(|m| m.len())
        .unwrap_or(0)
}

/// 用 `exe` 在 `home` 里按 `spec` 拉起一个后台实例，等它就绪，返回它对 IPC ping 的应答
/// （实际监听地址、日志路径等）。
///
/// 交接文件在子进程就绪或失败之后立即删掉：它含明文密钥，留在盘上的时间越短越好；实例
/// 自己把规格写进了期望状态记录，不再需要它。
pub async fn spawn_from_spec(
    home: &Path,
    exe: &Path,
    spec: &InstanceSpec,
    timeout: Duration,
) -> Result<InstanceStatus, SpawnError> {
    let run_dir = home.join("run");
    let mark = startup_log_len(home);
    let spec_path = spec::write_spec_file(&run_dir, spec).map_err(SpawnError::Spawn)?;
    let result = launch_and_wait(home, exe, spec, &spec_path, mark, timeout).await;
    let _ = std::fs::remove_file(&spec_path);
    result
}

async fn launch_and_wait(
    home: &Path,
    exe: &Path,
    spec: &InstanceSpec,
    spec_path: &Path,
    mark: u64,
    timeout: Duration,
) -> Result<InstanceStatus, SpawnError> {
    let args = [
        "__serve".to_string(),
        "--home".to_string(),
        home.display().to_string(),
        "--spec".to_string(),
        spec_path.display().to_string(),
    ];
    // 子进程的 home 由 `--home` 定，不由继承来的 APROXY_HOME 定：拉起方操作的是哪个 home，
    // 实例就属于哪个 home。环境里的 APROXY_HOME 一并改掉，实例再拉起的子进程（看护者、
    // format 程序）看到的也是它
    // 规格里 Some 的变量设成该值，None 的变量在子进程里删掉。
    let mut env = spec
        .env
        .iter()
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect::<Vec<_>>();
    env.push(("APROXY_HOME".to_string(), Some(home.display().to_string())));
    let launch = daemon::Launch {
        env: &env,
        cwd: Some(home),
    };
    let pid = daemon::spawn_detached_with(exe, &args, &launch).map_err(SpawnError::Spawn)?;
    wait_ready(home, pid, spec_path, None, mark, timeout).await
}

/// 旁观者只能看到本次运行的已脱敏错误，不能接触原始启动规格。文件名只含两个数字；
/// 即使端口被另一进程重新绑定，仍能分清两次握手。多个旁观者可独立读取，过期待 GC。
fn observed_result(run_dir: &Path, instance: &daemon::InstanceRecord) -> PathBuf {
    run_dir.join(format!(
        "startup-{}-{}.result",
        instance.started_at, instance.pid
    ))
}

/// 启动探测失败才落这份小结果；成功启动和无格式程序的实例不写任何额外文件。
pub fn publish_startup_failure(
    run_dir: &Path,
    instance: &daemon::InstanceRecord,
    error: &crate::transform::StartupError,
) -> std::io::Result<()> {
    let mut result = serde_json::to_value(error)?;
    result["writer_start"] = serde_json::json!(instance.start);
    crate::fsx::write_atomic_private(
        &observed_result(run_dir, instance),
        &serde_json::to_vec(&result)?,
    )
}

/// 另一个启动者已占用端口，只观察这个实例的就绪或退出，不读取它的私有规格、不回收其进程。
pub(crate) async fn wait_existing(
    home: &Path,
    instance: &daemon::InstanceRecord,
) -> Result<InstanceStatus, SpawnError> {
    wait_ready(
        home,
        instance.pid,
        &observed_result(&home.join("run"), instance),
        Some(&instance.instance_id),
        startup_log_len(home),
        READY_TIMEOUT,
    )
    .await
}
/// 同一个就绪循环供拉起者与旁观者共用；各自只读取自己的诊断来源。
async fn wait_ready(
    home: &Path,
    pid: u32,
    result_path: &Path,
    expected_id: Option<&str>,
    mark: u64,
    timeout: Duration,
) -> Result<InstanceStatus, SpawnError> {
    let run_dir = home.join("run");
    let start = crate::process::process_start_time(pid);
    if expected_id.is_some() {
        tracing::debug!(pid, "waiting for existing instance readiness");
    }
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Some(rec) = daemon::registry_find_pid_in(&run_dir, pid)
            && let Ok(live) = daemon::ipc_ping_in(&run_dir, daemon::port_of(&rec.listen_addr)).await
            && live.instance.pid == pid
            && expected_id.is_none_or(|id| live.instance.instance_id == id)
            && live.state == daemon::InstanceState::Serving
        {
            return Ok(live);
        }
        let exited = crate::process::process_exited(pid) == Some(true);
        // 旁观者只看本次运行原子发布的终态；拉起者的交接文件在退出前还可能含有原始规格。
        #[derive(serde::Deserialize)]
        #[serde(untagged)]
        enum ResultFile {
            Format(Box<crate::transform::StartupError>),
            Launch(Box<startup::LaunchReport>),
        }
        let result = if exited || expected_id.is_some() {
            std::fs::File::open(result_path)
                .ok()
                .and_then(|f| serde_json::from_reader(f).ok())
        } else {
            None
        };
        // 旁观者只读已登记实例公开给它的握手故障；绑定之前的原因只属于持有交接槽的启动者。
        let (format, report) = match result {
            Some(ResultFile::Format(e)) => (Some(e), None),
            Some(ResultFile::Launch(e)) if exited && expected_id.is_none() => (None, Some(e)),
            _ => (None, None),
        };
        if exited || format.is_some() {
            return Err(SpawnError::Exited {
                pid,
                format,
                report,
                startup_log: StartupLog::since(home, mark),
            });
        }
        if std::time::Instant::now() > deadline {
            return Err(SpawnError::TimedOut {
                pid,
                start,
                alive: crate::process::process_exited(pid) == Some(false),
                startup_log: StartupLog::since(home, mark),
            });
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
