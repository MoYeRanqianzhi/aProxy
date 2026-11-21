//! 守护进程编排：后台启动、实例注册表、IPC 控制。
//!
//! 进程模型：`aproxy`（默认）= 后台启动——父进程做预检与就绪等待，
//! 实际服务由分离的子进程（`aproxy __serve`）承载。子进程以 CREATE_NO_WINDOW
//! 启动、不继承控制台，终端关闭/常规内存清理不会连带杀掉守护进程。
//!
//! IPC（铁律：控制通道绝不占用代理端口，代理端口完全用于透传——避免控制
//! 路径与客户端请求路径巧合重叠造成严重 bug）：每个实例一条 Windows 命名
//! 管道 `\\.\pipe\aproxy-<home_id>-<port>`（unix 为 `<run 目录>/<port>.sock`），
//! 承载 `ping`/`shutdown` 等控制。端点按 run 目录划分命名空间（见
//! [`endpoint_for_in`]）：一个 home 的命令只能控制本 home 的实例，测试 home
//! 里的 `aproxy stop <端口>` 碰不到用户正在用的同端口实例。端口冲突的两种
//! 情况由此区分：
//! - 管道 ping 通 → 该端口已有 aProxy 实例在运行；
//! - 管道不通但 TCP bind 失败 → 端口被其他程序占用。
//!
//! 实例注册表：`~/.aproxy/run/<port>.instance.json`（JSON），仅供 `status`/`stop` 枚举
//! 实例；存活一律以 IPC ping 为准，不信任注册文件与 pid 本身。

pub use aproxy_ipc::client::{
    EventStream, IpcError, ipc_ping_in, ipc_probe_in, ipc_read_in, ipc_request_in, ipc_subscribe_in,
};
#[cfg(test)]
use aproxy_ipc::client::{parse_reply, request_raw};
pub use aproxy_ipc::registry::{
    INSTANCE_FILE_SUFFIX, INSTANCE_SCHEMA, InstanceFile, InstanceSurvey, instance_file_path_in,
    list_instances_in, now_millis, parse_instance_file, port_of, read_instance_file_in,
    read_instance_record, registry_contains_pid_in, registry_find_pid_in, registry_instances_in,
    registry_pids_in, survey_instances_in,
};
pub use aproxy_ipc::wire::*;
use std::{io, path::PathBuf, sync::Arc, time::Duration};

/// 运行期记录的目录：`<home>/run/`。home 是唯一的根，寻址只有 `APROXY_HOME` 这一个变量。
pub fn run_dir() -> PathBuf {
    aproxy_ipc::home::home().join("run")
}

/// 守护进程日志目录：`<home>/logs/`
pub fn logs_dir() -> PathBuf {
    aproxy_ipc::home::home().join("logs")
}

/// spool 临时文件的根目录：`<home>/spool/`。每个实例进程在它下面有自己的一个子目录
/// （见 [`own_spool_dir`]）。
pub fn spool_root() -> PathBuf {
    aproxy_ipc::home::home().join("spool")
}

/// 一个进程的 spool 子目录名：`<pid>-<启动标记的短哈希>`（哈希是 8 位小写十六进制）。
///
/// 按进程而不是按端口分：目录按端口命名时，同一端口上第二次启动的进程会在 bind 失败之前
/// 清掉正在运行的那个实例的缓存文件。带上启动标记（台账用的那一个，见
/// [`aproxy_ipc::ledger::start_marker`]）是为了防 pid 复用——pid 相同而标记不同的目录
/// 属于早已不在的进程。读不到标记的平台按空串算哈希，名字仍然确定。
pub fn spool_dir_name(pid: u32, start: Option<&str>) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(start.unwrap_or("").as_bytes());
    format!(
        "{pid}-{:02x}{:02x}{:02x}{:02x}",
        digest[0], digest[1], digest[2], digest[3]
    )
}

/// 本进程的 spool 目录：`<home>/spool/<pid>-<启动标记的短哈希>/`。
pub fn own_spool_dir() -> PathBuf {
    let pid = std::process::id();
    spool_root().join(spool_dir_name(
        pid,
        aproxy_ipc::ledger::start_marker(pid).as_deref(),
    ))
}

/// 建出实例自己的 spool 目录（开始接收请求之前调用）。失败只告警：磁盘缓存不可用时
/// 请求体与响应体留在内存里。
pub fn prepare_spool_dir(dir: &std::path::Path) {
    if let Err(e) = crate::fsx::create_dir_all(dir) {
        tracing::warn!(error = %e, dir = %dir.display(), "spool 目录创建失败（磁盘缓存将不可用，回退纯内存）");
    }
}

/// 删掉一个 spool 目录里的临时文件（`*.spooltmp`），目录空了就连目录一起删，返回目录
/// 是否已删。只删这一种文件：意外出现的其他内容保留，目录也就留着（不扩大删除面）。
pub fn remove_spool_dir(dir: &std::path::Path) -> bool {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "spooltmp")
            && let Err(e) = std::fs::remove_file(&path)
        {
            tracing::warn!(error = %e, path = %path.display(), "spool 残留清理失败");
        }
    }
    std::fs::remove_dir(dir).is_ok()
}

/// 清掉别的进程留下的 spool 目录，返回删掉的目录。spool 里是请求与响应的全文，进程崩溃
/// 或被强杀之后不该一直留在盘上。只看名字是 `<pid>-<8 位十六进制>` 的子目录，删除的条件
/// 是下面两条之一：
/// - 这个 pid 现在的启动标记算出来的名字与目录名不同（pid 已被别的进程复用）；
/// - 读不到这个 pid 的启动标记，并且能确认这个 pid 上已经没有进程。
///
/// 读不到标记、又确认不了进程不在时（本平台没有定义标记，或没有权限）保留。别的名字
/// （包括旧版本按端口命名的目录）不碰。
pub fn collect_stale_spool_in(root: &std::path::Path) -> Vec<PathBuf> {
    let mut removed = Vec::new();
    for entry in std::fs::read_dir(root).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(pid) = spool_dir_owner(&name) else {
            continue;
        };
        let stale = match aproxy_ipc::ledger::start_marker(pid) {
            Some(marker) => spool_dir_name(pid, Some(&marker)) != name,
            None => crate::process::process_exited(pid) == Some(true),
        };
        let path = entry.path();
        if stale && path.is_dir() && remove_spool_dir(&path) {
            removed.push(path);
        }
    }
    removed
}

/// 目录名是 `<pid>-<8 位小写十六进制>` 时返回其中的 pid。
fn spool_dir_owner(name: &str) -> Option<u32> {
    let (pid, hash) = name.split_once('-')?;
    (hash.len() == 8
        && hash
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)))
    .then_some(())?;
    pid.parse().ok()
}

/// 实例的 IPC 端点：windows 为命名管道名，unix 为 UDS 路径。
/// 端口号在一个 run 目录内唯一区分实例（同端口=同实例）。
pub fn endpoint_for(port: &str) -> String {
    endpoint_for_in(&run_dir(), port)
}

/// 同 endpoint_for，但按**显式 run_dir** 派生。库层函数（install 的
/// flow/restart、看门狗）收了 run_dir 参数就必须全程用它——若内部回落到
/// 进程级 APROXY_HOME 派生，测试进程与多 home 场景下会找到别的 home 的端点。
///
/// 名字怎样取、为什么带上 run 目录的标识，见 [`aproxy_ipc::endpoint::instance`]。
pub use aproxy_ipc::endpoint::instance as endpoint_for_in;

/// 从生效的配置取摘要。
pub fn behavior_of(cfg: &crate::config::Config) -> Behavior {
    Behavior {
        delivery: if cfg.forward_only_enabled() {
            "forward"
        } else {
            "retry"
        }
        .to_string(),
        disk_cache: cfg.disk_cache_enabled(),
        keepalive: KeepaliveBehavior {
            trigger: cfg.keepalive_trigger().as_str().to_string(),
            interval_secs: cfg.keepalive_interval().as_secs(),
        },
        retry_rules: cfg.retry_rules.len() as u32,
        history: HistoryBehavior {
            body_budget_mb: cfg.history_body_budget_mb,
        },
    }
}

/// 探测端口上是否有 aProxy 实例（纯 IPC，不触碰任何 TCP 端口）。
/// Ok = 实例在运行；Err = 端点上没有可识别的 aProxy（无实例，或该端口被
/// 其他程序占用——由调用方结合 TCP bind 结果区分这两种情况）。
/// 只找本进程 run 目录里的实例（见 `endpoint_for_in`）。
pub async fn ipc_ping(port: &str) -> Result<InstanceStatus, IpcError> {
    ipc_ping_in(&run_dir(), port).await
}

/// 发送一次 IPC 请求并等待应答（3 秒超时），找本进程 run 目录里的实例。
pub async fn ipc_request(port: &str, op: IpcOp) -> Result<InstanceStatus, IpcError> {
    ipc_request_in(&run_dir(), port, op).await
}

/// 本实例已独占创建、尚未开始接受连接的控制端点。守护先创建它、再写注册表
/// 与恢复记录（见 server::serve_forever）：有实例记录就意味着端点在应答
/// （挂死除外）。创建失败时守护直接启动失败，不带着一个谁也控制不了的实例
/// 继续跑。
pub struct IpcEndpoint(imp::Listener);

/// 创建本实例的控制端点。端点已被占用（同一 home 同端口的另一个实例，或抢先
/// 占住名字的其他进程）时报错，不与对方分摊或抢夺。
pub async fn bind_ipc(port: &str) -> io::Result<IpcEndpoint> {
    imp::bind(&endpoint_for(port)).await.map(IpcEndpoint)
}

/// 控制服务应答所需的一切，所有连接共享一份。
pub struct ControlState {
    /// 本实例的注册记录（应答里的 `instance`）
    pub record: InstanceRecord,
    /// 本实例的行为摘要（应答里的 `behavior`）
    pub behavior: Behavior,
    /// 本实例所属的 run 目录（期望状态记录写在这里）
    pub run_dir: String,
    /// 代理层的实时观测源，应答时现读
    pub stats: Arc<IpcStats>,
    /// 请求历史：读类 op 与事件都从这里取
    pub history: Arc<crate::history::History>,
    /// 收到 shutdown 时置位，服务主循环据此优雅退出；已置位即「正在退出」
    pub on_shutdown: tokio::sync::watch::Sender<bool>,
    /// 「用户已经要它停下」：收到过不带 `keep_restore` 的 shutdown。期望状态记录的写与删
    /// 都在这把锁下进行，置位之后不再写——否则启动途中收到 stop 的实例，会在记录被删掉
    /// 之后又把它写回来，随后被 restore 或看护者拉起
    pub stop_requested: std::sync::Mutex<bool>,
}

impl ControlState {
    /// 本实例的端口（记录与端点都按它命名）
    fn port(&self) -> &str {
        port_of(&self.record.listen_addr)
    }

    /// 最近一次收到客户端请求的时刻（Unix 毫秒）；还没收到过请求时是启动时刻
    fn last_request_at(&self) -> u64 {
        self.stats
            .last_activity_ms
            .load(std::sync::atomic::Ordering::Relaxed)
            .max(self.record.started_at)
    }

    /// 写本实例的期望状态记录。已经收到过不带 `keep_restore` 的 shutdown 时不写，返回
    /// `Ok(false)`。
    pub fn write_restore(&self, spec: &crate::spec::InstanceSpec) -> io::Result<bool> {
        let stopped = self
            .stop_requested
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if *stopped {
            return Ok(false);
        }
        crate::spec::write_restore_in(std::path::Path::new(&self.run_dir), self.port(), spec)?;
        Ok(true)
    }

    /// 兑现不带 `keep_restore` 的 shutdown：记下「用户要它停下」，删掉期望状态记录。
    fn forget_restore(&self) {
        let mut stopped = self
            .stop_requested
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *stopped = true;
        crate::spec::remove_restore_in(std::path::Path::new(&self.run_dir), self.port());
    }
}

/// 在已创建的端点上提供控制服务，直到进程退出。
pub async fn serve_ipc(endpoint: IpcEndpoint, state: Arc<ControlState>) {
    imp::accept_loop(endpoint.0, state).await
}

/// 在给定端点名上创建并提供控制服务（测试用）。只有创建失败会返回。
#[cfg(test)]
async fn serve_endpoint(endpoint: String, state: Arc<ControlState>) -> io::Result<()> {
    let listener = imp::bind(&endpoint).await?;
    imp::accept_loop(listener, state).await;
    Ok(())
}

/// 实例的实时观测数据源：代理热路径写入，IPC 响应读取。
/// 原子计数全部 Relaxed——只用于展示与告警，无跨线程因果序需求。
#[derive(Debug, Default)]
pub struct IpcStats {
    /// 与请求记录使用同一组凭据做替换；没有请求历史的空计数器没有配置密钥。
    history: Option<Arc<crate::history::History>>,
    /// 池的生命期由实例持有；IPC 仅读取快照，不延长进程生命期。
    pub(crate) formats: Vec<std::sync::Weak<crate::transform::Pool>>,
    /// 实例阶段变动唤醒状态读者；熔断变化另有带快照的有界队列。
    pub state_changed: Arc<tokio::sync::Notify>,
    /// 控制端点已上线，但格式程序尚未全部协商，或启动记录尚未落盘。
    pub starting: std::sync::atomic::AtomicBool,
    /// 最近一次收到客户端请求的时刻（Unix 毫秒；即 AppState::last_activity_ms，
    /// 同一 Arc 由 serve_forever 组装进来）
    pub last_activity_ms: Arc<std::sync::atomic::AtomicU64>,
    /// 累计收到客户端请求数
    pub requests_total: std::sync::atomic::AtomicU64,
    /// 累计上游重试次数（首轮之后的所有尝试）
    pub retries_total: std::sync::atomic::AtomicU64,
    /// 经过至少一次重试、最终成功的请求数
    pub rescued_total: std::sync::atomic::AtomicU64,
    /// 每次请求内升级只计一次；与第几次实际发送无关。
    pub upgraded_total: std::sync::atomic::AtomicU64,
    /// 已收到、响应还没交付完的请求数（由 [`IpcStats::enter`] 的守卫维护）
    pub in_flight: std::sync::atomic::AtomicU64,
    /// 正处在第二次及以后的尝试里的请求数（由 [`IpcStats::begin_retrying`] 的守卫维护）
    pub retrying: std::sync::atomic::AtomicU64,
    /// 按码的失败计数、时间桶与最近一次失败。一把锁：每个请求、每次重试、每次失败各锁
    /// 一次，临界区里只有几次整数运算
    tally: std::sync::Mutex<Tally>,
    /// 二进制更换阶段的到期时刻（Unix 毫秒）：prepare_swap 时写下，晚于现在就算在这个
    /// 阶段里。0 = 没进入过；重启自然清除
    pub swap_until_ms: std::sync::atomic::AtomicU64,
}

/// [`IpcStats`] 里要在一把锁下一起改的部分。
#[derive(Debug, Default)]
struct Tally {
    by_code: std::collections::BTreeMap<String, u64>,
    buckets: Buckets,
    last_error: Option<LastError>,
}

/// 时间桶：三条等长的计数，下标 0 最旧，最后一个是序号为 `newest` 的桶。
#[derive(Debug)]
struct Buckets {
    /// 最后一个桶的序号：Unix 秒除以 [`BUCKET_WIDTH_SECS`]
    newest: u64,
    requests: [u64; BUCKET_COUNT],
    retries: [u64; BUCKET_COUNT],
    failures: [u64; BUCKET_COUNT],
}

impl Default for Buckets {
    fn default() -> Self {
        Buckets {
            newest: 0,
            requests: [0; BUCKET_COUNT],
            retries: [0; BUCKET_COUNT],
            failures: [0; BUCKET_COUNT],
        }
    }
}

impl Buckets {
    /// 把窗口推进到 `now_ms` 所在的桶：过去的桶往前挪，新露出来的桶清零。时钟回拨时
    /// 不动（记进当前最新的那个桶）。
    fn advance(&mut self, now_ms: u64) {
        let index = now_ms / 1000 / BUCKET_WIDTH_SECS;
        let shift = index.saturating_sub(self.newest);
        if shift == 0 {
            return;
        }
        self.newest = index;
        for counts in [&mut self.requests, &mut self.retries, &mut self.failures] {
            if shift >= BUCKET_COUNT as u64 {
                counts.fill(0);
            } else {
                let shift = shift as usize;
                counts.rotate_left(shift);
                counts[BUCKET_COUNT - shift..].fill(0);
            }
        }
    }

    fn view(&self) -> ActivityBuckets {
        ActivityBuckets {
            width_secs: BUCKET_WIDTH_SECS,
            requests: self.requests.to_vec(),
            retries: self.retries.to_vec(),
            failures: self.failures.to_vec(),
        }
    }
}

/// 存活期间让 [`IpcStats`] 的一个计数加一的守卫：丢弃时减回去。
#[derive(Debug)]
pub struct Counted {
    stats: Arc<IpcStats>,
    which: fn(&IpcStats) -> &std::sync::atomic::AtomicU64,
}

impl Drop for Counted {
    fn drop(&mut self) {
        (self.which)(&self.stats).fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
    }
}

impl IpcStats {
    /// 一份全新的观测值，活动时间戳用给定的那一个（代理层的 AppState 也持有它）。
    pub fn sharing(
        last_activity_ms: Arc<std::sync::atomic::AtomicU64>,
        history: Arc<crate::history::History>,
    ) -> Self {
        IpcStats {
            last_activity_ms,
            history: Some(history),
            ..Default::default()
        }
    }

    fn counted(self: &Arc<Self>, which: fn(&IpcStats) -> &std::sync::atomic::AtomicU64) -> Counted {
        which(self).fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Counted {
            stats: self.clone(),
            which,
        }
    }

    /// 一个请求进入处理：在途数加一，守卫丢弃时减回去。
    pub fn enter(self: &Arc<Self>) -> Counted {
        self.counted(|stats| &stats.in_flight)
    }

    /// 一个请求开始它的第二次尝试：重试中的请求数加一，守卫丢弃时减回去。
    pub fn begin_retrying(self: &Arc<Self>) -> Counted {
        self.counted(|stats| &stats.retrying)
    }

    fn tally(&self) -> std::sync::MutexGuard<'_, Tally> {
        // 锁被毒化只可能是持锁时 panic；计数照常用，不让一次 panic 废掉整个观测
        self.tally.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 记一个收到的请求，返回它是本实例的第几个请求。
    pub fn note_request(&self) -> u64 {
        self.note_request_at(now_millis())
    }

    fn note_request_at(&self, now_ms: u64) -> u64 {
        let mut tally = self.tally();
        tally.buckets.advance(now_ms);
        tally.buckets.requests[BUCKET_COUNT - 1] += 1;
        self.requests_total
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1
    }

    /// 记一次重试（首轮之后的一次尝试开始）。
    pub fn note_retry(&self) {
        self.note_retry_at(now_millis());
    }

    fn note_retry_at(&self, now_ms: u64) {
        let mut tally = self.tally();
        tally.buckets.advance(now_ms);
        tally.buckets.retries[BUCKET_COUNT - 1] += 1;
        self.retries_total
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// 记一个经过重试之后成功的请求。
    pub fn note_rescued(&self) {
        self.rescued_total
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// 记一次失败的上游尝试：按码计数、进时间桶，并成为「最近一次失败」。`message` 与
    /// `details` 须已打码。
    pub fn note_failed_attempt(
        &self,
        code: &str,
        details: &serde_json::Map<String, serde_json::Value>,
        message: &str,
    ) {
        self.note_failure_at(now_millis(), code, details, message, true);
    }

    /// 记一个被 aproxy 自己结束的请求（终态码）：按码计数，并成为「最近一次失败」。不进
    /// 时间桶的 `failures`——那里数的是失败的上游尝试。`message` 与 `details` 须已打码。
    pub fn note_terminal(
        &self,
        code: &str,
        details: &serde_json::Map<String, serde_json::Value>,
        message: &str,
    ) {
        self.note_failure_at(now_millis(), code, details, message, false);
    }

    fn note_failure_at(
        &self,
        now_ms: u64,
        code: &str,
        details: &serde_json::Map<String, serde_json::Value>,
        message: &str,
        attempt: bool,
    ) {
        // 解码错误里的 encoding 等信息来自对端。先替换再截断，也不在计数锁里做字符串扫描。
        let (message, details) = match &self.history {
            Some(h) => (h.secrets().replace(message), h.redacted_details(details)),
            None => (
                message.to_owned(),
                serde_json::Value::Object(details.clone()),
            ),
        };
        let mut tally = self.tally();
        *tally.by_code.entry(code.to_string()).or_insert(0) += 1;
        if attempt {
            tally.buckets.advance(now_ms);
            tally.buckets.failures[BUCKET_COUNT - 1] += 1;
        }
        tally.last_error = Some(LastError {
            code: code.to_string(),
            details,
            // 截断到 200 字符：说明进 IPC 应答与 status 展示，防止上游返回的超长错误文本
            // 把应答行撑大
            message: message.chars().take(200).collect(),
            at: now_ms,
        });
    }

    /// 最近一次失败；从未失败为 None。
    pub fn last_error(&self) -> Option<LastError> {
        self.tally().last_error.clone()
    }

    /// 此刻的观测值（快照里的 `activity`）。`started_at` 是实例的启动时刻：还没收到过请求
    /// 时，「最近一次请求」取它。
    pub fn activity(&self, started_at: u64) -> Activity {
        self.activity_at(now_millis(), started_at)
    }

    fn activity_at(&self, now_ms: u64, started_at: u64) -> Activity {
        use std::sync::atomic::Ordering::Relaxed;
        let mut tally = self.tally();
        // 推进到现在：长时间没有请求的实例，桶也要跟着时间往前走
        tally.buckets.advance(now_ms);
        Activity {
            last_request_at: self.last_activity_ms.load(Relaxed).max(started_at),
            requests_total: self.requests_total.load(Relaxed),
            retries_total: self.retries_total.load(Relaxed),
            rescued_total: self.rescued_total.load(Relaxed),
            upgraded_total: self.upgraded_total.load(Relaxed),
            in_flight: self.in_flight.load(Relaxed),
            retrying: self.retrying.load(Relaxed),
            by_code: tally.by_code.clone(),
            buckets: tally.buckets.view(),
            last_error: tally.last_error.clone(),
        }
    }
}

/// 强制终止实例进程（`--force`）：跳过 IPC 优雅关闭直接终止。`info` 必须
/// 是刚经 IPC ping 从该端口拿到的实例信息（stop 的 target 解析一律经 ping，
/// 应答者自报的 pid 即端点归属证明）。
///
/// 身份判定与二进制名称无关（改名运行的官方资产 `aproxy-<target>` 同样可
/// 强杀）：「pid + 创建时间」核验后才终止（Windows 在同一进程句柄上先核验
/// 后终止，中间不存在 pid 复用窗口）。不符 = pid 已被复用给别的进程；未登记
/// 创建时间（0）= 证明不了身份——两者都拒绝，杀错进程不可逆。
///
/// 返回 Err 的信息已含原因，调用方直接展示。
pub fn force_terminate(info: &InstanceRecord) -> Result<(), String> {
    crate::process::terminate_marked_process(info.pid, &info.start)
}

/// 向 startup.log 追加一行（带 unix 时间戳前缀）。看门狗的 crashloop 放弃、
/// 假死接管等重大事件写这里——用户排查「实例为什么没被拉起」的第一个入口。
pub fn append_startup_log(line: &str) {
    let _ = crate::fsx::create_dir_all(logs_dir());
    let path = logs_dir().join("startup.log");
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        use std::io::Write;
        let _ = writeln!(f, "[unix+{ts}s] {line}");
    }
}

/// 等待实例退出：只有端点消失（`Unreachable`）才算已退出，超时返回 false。
/// 忙、超时、应答无法采信都不算——挂死的实例正是「端点还在、却不应答」，
/// 据此报「已停止」会掩盖一个仍占着端口的进程。
pub async fn wait_until_gone(port: &str, timeout: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if let Err(IpcError::Unreachable(_)) = ipc_ping(port).await {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// 分离拉起时对子进程环境的调整。默认值 = 原样继承环境与工作目录。
#[derive(Debug, Clone, Copy, Default)]
pub struct Launch<'a> {
    /// 要改的环境变量：`Some` 设成这个值，`None` 在子进程里删掉。其余变量原样继承
    pub env: &'a [(String, Option<String>)],
    /// 子进程的工作目录；None = 继承
    pub cwd: Option<&'a std::path::Path>,
}

/// 以分离进程运行 `exe`，环境与工作目录原样继承。见 [`spawn_detached_with`]。
pub fn spawn_detached(exe: &std::path::Path, args: &[String]) -> io::Result<u32> {
    spawn_detached_with(exe, args, &Launch::default())
}

/// 以分离进程运行 `exe`：不继承句柄、没有控制台，并脱离启动者——关掉启动它的终端、
/// 终端向前台进程组发的 Ctrl+C、启动者所在的作业对象被结束，都不该连带结束它。
/// 返回子进程 pid（拉起即返回，不等待）。
///
/// Windows 上不走 std::process::Command：std 的 CreateProcessW 调用固定
/// bInheritHandles=TRUE 且无稳定 API 可关闭（CommandExt::inherit_handles
/// 仍 unstable）。这意味着调用方（cargo test / agent 脚本 / CI）经
/// Command::output() 捕获输出运行 `aproxy start` 时，其 stdout/stderr 管道
/// 写端句柄会被常驻守护进程继承，EOF 永不到来，调用方永久挂死。因此这里
/// 手写 CreateProcessW，以 bInheritHandles=FALSE 启动，不继承任何句柄。
///
/// 脱离的手段：
/// - Windows：CREATE_NO_WINDOW（没有控制台，控制台的进程树清理碰不到它），外加
///   CREATE_BREAKAWAY_FROM_JOB 脱出启动者的作业对象。作业不允许脱出时带着这个标志的
///   CreateProcess 会直接失败，所以失败就去掉它再试一次，并在 startup.log 记一条告警：
///   子进程仍在启动者的作业里，作业被结束时它会跟着结束。
/// - unix：子进程里 `setsid()`，自成会话与进程组，终端的信号到不了它。不与
///   `process_group(0)` 同用：先成了进程组组长，`setsid()` 会以 EPERM 失败。
pub fn spawn_detached_with(
    exe: &std::path::Path,
    args: &[String],
    launch: &Launch<'_>,
) -> io::Result<u32> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Foundation::{CloseHandle, GetLastError};
        use windows_sys::Win32::System::Threading::{
            CREATE_BREAKAWAY_FROM_JOB, CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT,
            CreateProcessW, PROCESS_INFORMATION, STARTUPINFOW,
        };

        /// Windows 命令行引号规则：含空格/制表符/引号的参数包引号；
        /// 参数内的 `"` 转成 `""`，引号前的连续 `\` 翻倍（闭引号前同样翻倍）。
        fn quote_arg(arg: &str) -> String {
            if !arg.is_empty()
                && !arg
                    .chars()
                    .any(|c| matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0b' | '\x0c' | '"'))
            {
                return arg.to_string();
            }
            let mut out = String::with_capacity(arg.len() + 2);
            out.push('"');
            let mut backslashes = 0usize;
            for c in arg.chars() {
                match c {
                    '\\' => backslashes += 1,
                    '"' => {
                        out.push_str(&"\\".repeat(backslashes * 2 + 1));
                        out.push('"');
                        backslashes = 0;
                    }
                    _ => {
                        out.push_str(&"\\".repeat(backslashes));
                        out.push(c);
                        backslashes = 0;
                    }
                }
            }
            // 闭引号前的 `\` 同样要翻倍，否则会被当作转义引号
            out.push_str(&"\\".repeat(backslashes * 2));
            out.push('"');
            out
        }

        let mut cmdline = quote_arg(&exe.display().to_string());
        for arg in args {
            cmdline.push(' ');
            cmdline.push_str(&quote_arg(arg));
        }
        let exe_wide: Vec<u16> = exe.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut cmdline_wide: Vec<u16> = cmdline.encode_utf16().chain(Some(0)).collect();
        // 没有要改的变量时传 NULL（继承），省掉一次整块复制
        let env_block = (!launch.env.is_empty()).then(|| windows_env_block(launch.env));
        let env_ptr = env_block
            .as_ref()
            .map_or(std::ptr::null(), |block| block.as_ptr().cast());
        let cwd_wide: Option<Vec<u16>> = launch
            .cwd
            .map(|dir| dir.as_os_str().encode_wide().chain(Some(0)).collect());
        let cwd_ptr = cwd_wide
            .as_ref()
            .map_or(std::ptr::null(), |dir| dir.as_ptr());

        let mut si: STARTUPINFOW = unsafe { std::mem::zeroed() };
        si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
        let mut create = |flags: u32| -> Result<(), u32> {
            let ok = unsafe {
                CreateProcessW(
                    exe_wide.as_ptr(),
                    cmdline_wide.as_mut_ptr(),
                    std::ptr::null(), // lpProcessAttributes：默认安全描述符
                    std::ptr::null(), // lpThreadAttributes：默认安全描述符
                    0,                // bInheritHandles=FALSE：核心目的，杜绝句柄泄漏
                    flags,
                    env_ptr,
                    cwd_ptr,
                    &si,
                    &mut pi,
                )
            };
            if ok == 0 {
                Err(unsafe { GetLastError() })
            } else {
                Ok(())
            }
        };
        let base = CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT;
        if create(base | CREATE_BREAKAWAY_FROM_JOB).is_err() {
            // 作业不允许脱出（或别的原因）：去掉标志再试。这一次的错误才是要报的错误——
            // 可执行文件不在、没有权限这类原因两次都会失败
            create(base).map_err(|code| io::Error::from_raw_os_error(code as i32))?;
            append_startup_log(&format!(
                "warning: pid {} could not leave the launcher's job object and stays inside it; ending that job ends this process too",
                pi.dwProcessId
            ));
        }
        let pid = pi.dwProcessId;
        // 守护进程只关心 pid；句柄持有会导致进程退出通知与资源泄漏
        unsafe {
            CloseHandle(pi.hProcess);
            CloseHandle(pi.hThread);
        }
        Ok(pid)
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        use std::process::{Command, Stdio};
        let mut cmd = Command::new(exe);
        cmd.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        for (name, value) in launch.env {
            match value {
                Some(value) => cmd.env(name, value),
                None => cmd.env_remove(name),
            };
        }
        if let Some(dir) = launch.cwd {
            cmd.current_dir(dir);
        }
        // SAFETY: 闭包在 fork 之后、exec 之前的子进程里运行，只调用 setsid 这一个
        // async-signal-safe 的系统调用，不分配内存、不碰锁
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        Ok(cmd.spawn()?.id())
    }
}

/// 在当前进程的环境上套用 `changes`，写成 CreateProcessW 要的环境块：UTF-16 的
/// `名=值\0` 逐条相接，末尾再加一个 `\0`。变量名不分大小写：设置或删除一个变量时，
/// 先去掉任何大小写写法的同名变量。条目按名字的大写形式排序（系统对环境块的约定）。
#[cfg(windows)]
fn windows_env_block(changes: &[(String, Option<String>)]) -> Vec<u16> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStrExt;

    let mut vars: Vec<(OsString, OsString)> = std::env::vars_os().collect();
    for (name, value) in changes {
        vars.retain(|(existing, _)| !existing.to_string_lossy().eq_ignore_ascii_case(name));
        if let Some(value) = value {
            vars.push((name.into(), value.into()));
        }
    }
    vars.sort_by_cached_key(|(name, _)| name.to_string_lossy().to_uppercase());
    let mut block = Vec::new();
    for (name, value) in &vars {
        block.extend(name.encode_wide());
        block.push(u16::from(b'='));
        block.extend(value.encode_wide());
        block.push(0);
    }
    if vars.is_empty() {
        // 空环境块是两个 `\0`
        block.push(0);
    }
    block.push(0);
    block
}

// ---------------------------------------------------------------------------
// 实例注册表（~/.aproxy/run/<port>.instance.json）
// ---------------------------------------------------------------------------

/// 注册表文件路径：`<run_dir>/<port>.instance.json`
pub fn instance_file_path(listen_addr: &str) -> PathBuf {
    instance_file_path_in(&run_dir(), listen_addr)
}

/// 写入实例注册（bind 成功后调用，避免留下死记录）
pub fn write_instance_file(info: &InstanceRecord) -> io::Result<()> {
    write_instance_file_in(&run_dir(), info)
}

/// 同上，目录可指定（测试注入用）
///
/// 一行 JSON，`schema` 在前。原子写、只有本用户可读（crate::fsx::write_atomic_private）：
/// 直接写目标文件时，并发的读者可能读到写到一半的 JSON，把这个实例当成不存在。
pub fn write_instance_file_in(run_dir: &std::path::Path, info: &InstanceRecord) -> io::Result<()> {
    let path = instance_file_path_in(run_dir, &info.listen_addr);
    let file = InstanceFile {
        schema: INSTANCE_SCHEMA,
        record: info.clone(),
    };
    let json = serde_json::to_string(&file).expect("序列化实例信息失败");
    crate::fsx::write_atomic_private(&path, (json + "\n").as_bytes())
}

/// 删除实例注册（服务退出时调用）
pub fn remove_instance_file(listen_addr: &str) {
    let _ = std::fs::remove_file(instance_file_path(listen_addr));
}

/// 列出注册表中应答控制通道的实例（见 `survey_instances_in`）。
pub async fn list_instances() -> Vec<InstanceStatus> {
    survey_instances().await.responsive
}

/// 普查本 home 的注册表。只读：不删任何文件（清理见 [`collect_garbage_in`]）。
pub async fn survey_instances() -> InstanceSurvey {
    survey_instances_in(&run_dir()).await
}

// ---------------------------------------------------------------------------
// 清理：只在实例开始服务之后与看护者的周期里做，读路径（status、选目标、界面轮询）
// 不删任何文件——查看状态的命令不该有副作用，而且它们可能由比写者更旧的版本执行。
// ---------------------------------------------------------------------------

/// 清理只碰早于这个年龄（秒）的日志、交接文件与临时文件：更新的可能正被某个进程写着、
/// 或刚写下还没被引用。
const GC_MIN_AGE_SECS: u64 = 600;

/// 一轮清理做了什么。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GcReport {
    /// 删掉的文件
    pub removed: Vec<PathBuf>,
    /// 日志清理是否因为有读不懂的记录而暂停（引用集不完整，不敢删）
    pub logs_paused: bool,
}

/// 一轮清理。规则：
/// - **实例记录** `<端口>.instance.json`：读得懂、并且写下它的进程经「pid + 启动标记」核验已经不在，
///   才删。读不懂的（损坏，或更新的版本写的）绝不删。
/// - **期望状态记录**：不归清理管，只有 stop 能删。
/// - **心跳文件** `<端口>.heart`（unix）：这个端口没有活着的实例、且文件早于
///   [`GC_MIN_AGE_SECS`] 时删（活实例每 10 秒写它一次，旧成这样的只能是崩溃留下的）。
/// - **交接文件与原子写的临时文件**（`*.spec.json`、`*.tmp-<pid>-<纳秒>`）：正常情况下
///   写者自己立即删掉或改名；写者崩溃留下的，在它的 pid 已不在、且文件早于
///   [`GC_MIN_AGE_SECS`] 时删。交接文件里有明文密钥，不能一直留在盘上。只有失败时写的
///   `startup-<毫秒>-<pid>.result` 只含握手诊断，用同一保护期让各个旁观者都能读到。
/// - **实例日志**：只删同时满足三条的——名字是内置的随机名形状
///   （`<十六进制>-<四位十六进制>.log`）、早于 [`GC_MIN_AGE_SECS`]、不被任何实例记录或
///   期望状态记录引用。run 目录里有读不懂的记录时，引用集不完整，这一轮不删任何日志。
///   自定义的 `log_file` 与 startup.log 等固定名的日志永远不删。
pub fn collect_garbage_in(run_dir: &std::path::Path, logs_dir: &std::path::Path) -> GcReport {
    let mut report = GcReport::default();
    let mut referenced_logs: Vec<String> = Vec::new();
    let mut unreadable = false;
    // 有活实例的端口，与见到的心跳文件（等实例记录都看完再判断它们）
    let mut live_ports: Vec<String> = Vec::new();
    let mut hearts: Vec<(String, PathBuf)> = Vec::new();

    let restores = crate::spec::list_restore_in(run_dir);
    unreadable |= !restores.unreadable.is_empty();
    referenced_logs.extend(restores.entries.into_iter().map(|e| e.spec.log_path));

    for entry in std::fs::read_dir(run_dir).into_iter().flatten().flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(INSTANCE_FILE_SUFFIX) {
            let record = std::fs::read_to_string(&path)
                .ok()
                .and_then(|text| parse_instance_file(&text));
            match record {
                None => unreadable = true,
                Some(info) => {
                    // 没登记启动标记（空串，平台算不出）的记录核验不了身份：只在这个 pid 上
                    // 确实没有进程时才当它的写者已经不在，否则宁可留着
                    let alive = if info.start.is_empty() {
                        crate::process::process_exited(info.pid) != Some(true)
                    } else {
                        crate::process::record_identity(info.pid, &info.start)
                            == crate::process::RecordIdentity::Alive
                    };
                    if alive {
                        live_ports.push(port_of(&info.listen_addr).to_string());
                        referenced_logs.extend(info.log_path);
                    } else if std::fs::remove_file(&path).is_ok() {
                        report.removed.push(path);
                    }
                }
            }
        } else if let Some(port) = name.strip_suffix(".heart") {
            hearts.push((port.to_string(), path));
        } else if let Some(writer) = leftover_writer_pid(&name)
            && older_than(&path, GC_MIN_AGE_SECS)
            && (crate::process::process_exited(writer) != Some(false)
                || (name.ends_with(".result") && startup_writer_reused(&path, writer)))
            && std::fs::remove_file(&path).is_ok()
        {
            report.removed.push(path);
        }
    }

    for (port, path) in hearts {
        if !live_ports.contains(&port)
            && older_than(&path, GC_MIN_AGE_SECS)
            && std::fs::remove_file(&path).is_ok()
        {
            report.removed.push(path);
        }
    }

    if unreadable {
        report.logs_paused = true;
        return report;
    }
    // 引用集按 path_match_key 归一比较（大小写/分隔符规则与别名匹配共用）：记录里存的
    // 路径字符串与 read_dir 枚举出的表示在 Windows 上可能有分隔符差异，字面比较会误删
    let referenced: std::collections::HashSet<String> = referenced_logs
        .iter()
        .filter(|p| !p.is_empty())
        .map(|p| crate::settings::path_match_key(p))
        .collect();
    for entry in std::fs::read_dir(logs_dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if is_random_log_name(&entry.file_name().to_string_lossy())
            && older_than(&path, GC_MIN_AGE_SECS)
            && !referenced.contains(&crate::settings::path_match_key(
                &path.display().to_string(),
            ))
            && std::fs::remove_file(&path).is_ok()
        {
            report.removed.push(path);
        }
    }
    report
}

/// 诊断已过保护期但 pid 仍在时，只有可比较的写者标记才证明旧进程已退出。
fn startup_writer_reused(path: &std::path::Path, pid: u32) -> bool {
    /// 只取身份字段，诊断结构的扩展不影响清理。
    #[derive(serde::Deserialize)]
    struct Writer {
        /// 与实例登记同源的启动标记；旧结果没有这个字段，保守地等 pid 退出。
        writer_start: String,
    }
    let Some(writer) = std::fs::File::open(path)
        .ok()
        .and_then(|f| serde_json::from_reader::<_, Writer>(std::io::Read::take(f, 65536)).ok())
    else {
        return false;
    };
    !writer.writer_start.is_empty()
        && aproxy_ipc::ledger::start_marker(pid)
            .is_some_and(|current| current != writer.writer_start)
}

/// run 目录里一个文件若是交接文件或原子写的临时文件，返回名字里写者的 pid。
/// 交接文件是 `<纳秒>-<pid>.spec.json`，临时文件是 `<目标名>.tmp-<pid>-<纳秒>`。
fn leftover_writer_pid(name: &str) -> Option<u32> {
    if let Some(stem) = name.strip_suffix(".spec.json") {
        return stem.rsplit_once('-')?.1.parse().ok();
    }
    // 旁观启动者可能还没来得及读错误；同其他交接文件一样，只有超过保护期且写者已退出才删。
    if let Some(stem) = name
        .strip_prefix("startup-")
        .and_then(|s| s.strip_suffix(".result"))
    {
        let (stamp, pid) = stem.split_once('-')?;
        if !stamp.bytes().chain(pid.bytes()).all(|c| c.is_ascii_digit()) {
            return None;
        }
        stamp.parse::<u64>().ok()?;
        return pid.parse().ok();
    }
    let (_, tail) = name.rsplit_once(".tmp-")?;
    let (pid, nanos) = tail.split_once('-')?;
    nanos.chars().all(|c| c.is_ascii_digit()).then_some(())?;
    pid.parse().ok()
}

/// 名字是不是内置的随机日志名：`<1 到 16 位小写十六进制>-<4 位小写十六进制>.log`。
fn is_random_log_name(name: &str) -> bool {
    let is_hex = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
    };
    name.strip_suffix(".log")
        .and_then(|stem| stem.split_once('-'))
        .is_some_and(|(nanos, pid)| {
            nanos.len() <= 16 && is_hex(nanos) && pid.len() == 4 && is_hex(pid)
        })
}

/// 文件的修改时间是否早于 `secs` 秒之前。读不到修改时间的按「不够旧」处理。
fn older_than(path: &std::path::Path, secs: u64) -> bool {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|age| age.as_secs() >= secs)
}

/// 按一份期望状态记录拉起的实例落在了另一个端口（配置里的端口改了）时，退役旧端口的
/// 残留记录：删除旧端口的期望状态记录与实例记录。这是期望状态记录「只有 stop 能删」
/// 之外唯一的例外：新端口的记录接替了它。
///
/// 为什么必须删：新实例已按实际端口写了自己的记录；旧端口的期望状态记录若残留，用户
/// `stop <新端口>` 后看护者（或下一次 `aproxy restore`）会凭它把实例再拉起来，stop 被撤销
/// 且可无限重复（watchdog-01 实测）。
/// 旧端口的实例记录是已死旧进程的记录，留着只会让看护者的闲置自灭永远等不到
/// 「注册表空」。
///
/// 保护：旧端口若已被另一个仍在运行的实例接手（其注册表记录核验为同一
/// 进程），记录归它，一律不动。返回是否执行了退役。
pub fn retire_moved_port_records_in(
    run_dir: &std::path::Path,
    old_port: &str,
    new_port: &str,
) -> bool {
    if old_port == new_port {
        return false;
    }
    if let Some(rec) = read_instance_file_in(run_dir, old_port)
        && crate::process::record_identity(rec.pid, &rec.start)
            == crate::process::RecordIdentity::Alive
    {
        return false;
    }
    crate::spec::remove_restore_in(run_dir, old_port);
    let _ = std::fs::remove_file(instance_file_path_in(run_dir, old_port));
    true
}

/// 本进程删过控制 socket（退出流程）：unix 的接受循环据此不再把它重建出来。
#[cfg(unix)]
static SOCKET_RETIRED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 删除实例的 IPC 端点文件（优雅退出/看护摘除时调用）。
/// Windows 命名管道由内核回收（no-op）；unix 的 UDS socket 是真实文件，
/// bind 前的 remove_file 已自愈残留，此处显式清理让 run/ 目录不留死端点。
pub fn remove_socket_file(port: &str) {
    #[cfg(unix)]
    {
        SOCKET_RETIRED.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = std::fs::remove_file(endpoint_for(port));
    }
    #[cfg(windows)]
    {
        let _ = port;
    }
}

/// 实例退出时的运行期文件清理：实例记录、心跳与控制端点文件（后两者在 Windows 上由
/// 内核回收，调用是 no-op）。被强制终止的实例来不及自清，由终止它的一方代为执行。
///
/// 期望状态记录不在这里删：它表达「这个实例应当在运行」，只有 stop 能删（见
/// [`crate::spec`]）。实例因信号、崩溃或系统关机退出时记录留着，restore 与看护者据此把它
/// 拉回来。
pub fn remove_registry_files(listen_addr: &str) {
    remove_instance_file(listen_addr);
    crate::watchdog::remove_heartbeat_file(&run_dir(), port_of(listen_addr));
    remove_socket_file(port_of(listen_addr));
}

// ---------------------------------------------------------------------------
// 通用协议层：请求-响应各一行 JSON。两个平台的传输实现共用这套编解码。
// ---------------------------------------------------------------------------

/// 处理一条 IPC 连接：读请求行 → 执行 → 回应答行。
async fn handle_conn<S>(stream: S, state: Arc<ControlState>) -> io::Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
    let (reader, mut writer) = tokio::io::split(stream);
    let mut line = String::new();
    // 请求行长度上限：控制端点是系统边界输入（同用户本地进程均可打开写入），
    // read_line 会无界累积直到遇到 \n，恶意/异常客户端可借此把守护进程内存
    // 吃到 OOM。超过上限即按无效请求回绝后断开。
    const MAX_REQUEST_LINE: u64 = 64 * 1024;
    // 读请求的时限：连上却迟迟不发完一行的客户端会一直占着这个处理任务
    // （Windows 上还占着一个管道实例，实例数有上限，占满后新连接全部失败）。
    // 正常客户端连上即发，几毫秒内完成
    const REQUEST_READ_TIMEOUT: Duration = Duration::from_secs(5);
    let mut limited = BufReader::new(reader).take(MAX_REQUEST_LINE);
    match tokio::time::timeout(REQUEST_READ_TIMEOUT, limited.read_line(&mut line)).await {
        Ok(read) => {
            read?;
        }
        Err(_) => return Ok(()),
    }
    let op = match parse_request(&line) {
        Ok(Parsed::Control(op)) => op,
        Ok(Parsed::Read(ReadOp::Subscribe)) => {
            // 请求行之后客户端不再发东西；读端留着，只为看出它关了连接
            return serve_events(limited.into_inner(), writer, state).await;
        }
        Ok(Parsed::Read(op)) => {
            let reply = match read_reply(&op, &state.history) {
                Ok(result) => ok_reply(&result),
                Err(rejection) => rejection.reply(),
            };
            return write_line(&mut writer, &reply).await;
        }
        Err(rejection) => return write_line(&mut writer, &rejection.reply()).await,
    };
    let stopping = *state.on_shutdown.borrow();
    match op {
        IpcOp::Ping => {}
        IpcOp::PrepareSwap { ttl_secs } => {
            if stopping {
                let reply = Rejection::InvalidState("stopping").reply();
                return write_line(&mut writer, &reply).await;
            }
            if state
                .stats
                .starting
                .load(std::sync::atomic::Ordering::Acquire)
            {
                return write_line(&mut writer, &Rejection::InvalidState("starting").reply()).await;
            }
            // 先置位再组装应答：应答里立即是 swap_prepared，安装器拿这一个应答
            // 就完成 ACK 判定，无需再补一次 ping
            let ttl_ms = ttl_secs
                .unwrap_or(DEFAULT_SWAP_TTL_SECS)
                .saturating_mul(1000);
            state.stats.swap_until_ms.store(
                now_millis().saturating_add(ttl_ms),
                std::sync::atomic::Ordering::Relaxed,
            );
            state.stats.state_changed.notify_waiters();
        }
        IpcOp::Shutdown {
            keep_restore,
            if_idle_secs,
        } => {
            // 条件只在还没开始退出时判断：已经在退出的实例，重复的 shutdown 结果相同
            if let Some(secs) = if_idle_secs
                && !stopping
            {
                let idle_ms = now_millis().saturating_sub(state.last_request_at());
                if idle_ms < secs.saturating_mul(1000) {
                    let reply = Rejection::ConditionNotMet { idle_ms }.reply();
                    return write_line(&mut writer, &reply).await;
                }
            }
            // 记录先删、应答后发：调用方拿到应答时，「它不会再被拉回来」已经成立
            if !keep_restore {
                state.forget_restore();
            }
        }
    }
    let mut status = status_snapshot(&state);
    if matches!(op, IpcOp::Shutdown { .. }) {
        status.state = InstanceState::Stopping;
    }
    let reply = serde_json::to_string(&WireResponse {
        v: IPC_VERSION,
        ok: true,
        result: Some(status),
        error: None,
    })
    .expect("序列化 IPC 应答失败");
    write_line(&mut writer, &reply).await?;
    // 应答先发出去再触发停止：客户端立刻拿到确认，服务随后优雅退出
    if matches!(op, IpcOp::Shutdown { .. }) {
        let _ = state.on_shutdown.send(true);
    }
    Ok(())
}

/// 成功应答的一行（不含换行）。`result` 是已经写成 JSON 文本的结果。
fn ok_reply(result: &str) -> String {
    format!(r#"{{"v":{IPC_VERSION},"ok":true,"result":{result}}}"#)
}

/// 执行一个一元的读 op，返回 `result` 的 JSON 文本。
fn read_reply(op: &ReadOp, history: &crate::history::History) -> Result<String, Rejection> {
    use crate::history::ReadError;
    let unknown = |request_id: &str| Rejection::NotFound {
        request_id: request_id.to_string(),
        reason: "unknown_request",
        cause: None,
    };
    match op {
        ReadOp::Requests { before, limit } => {
            let limit = limit.map_or(crate::history::DEFAULT_LIST, |n| n as usize);
            history
                .list_json(before.as_deref(), limit)
                .ok_or_else(|| unknown(before.as_deref().unwrap_or_default()))
        }
        ReadOp::Request { request_id } => history
            .detail_json(request_id)
            .ok_or_else(|| unknown(request_id)),
        ReadOp::Body {
            request_id,
            part,
            attempt,
            offset,
            limit,
        } => history
            .read_body_json(
                request_id,
                *part,
                attempt.unwrap_or(0),
                offset.unwrap_or(0),
                limit.unwrap_or(crate::history::MAX_BODY_READ),
            )
            .map_err(|e| match e {
                ReadError::NotFound { reason, cause } => Rejection::NotFound {
                    request_id: request_id.clone(),
                    reason,
                    cause,
                },
                // 起点在体的末尾之后
                ReadError::BadOffset { .. } => Rejection::BadRequest {
                    reason: "invalid_arg",
                    arg: Some("offset"),
                },
            }),
        // 流式的 op 不走这里（见 handle_conn）
        ReadOp::Subscribe => Err(Rejection::bad("invalid_arg")),
    }
}

/// 流式连接上两行之间最长的间隔：没有别的事件可发时，到点发一个 `heartbeat`。客户端 45 秒
/// 收不到任何一行就判失联。
const EVENT_IDLE: Duration = Duration::from_secs(15);
/// 往流式连接里写一行的时限：不读的客户端不能一直占着一条流式连接的名额
const EVENT_WRITE_TIMEOUT: Duration = Duration::from_secs(10);
/// 一次写出去的事件攒到这么多字节就不再往里加：一批写不完的留给下一轮
const EVENT_BATCH_BYTES: usize = 256 * 1024;

/// 信封与换行也占行长预算；只有省略了记录才写标记。
fn opening_reply(
    sub: &crate::history::Subscription,
    snapshot: &str,
    limit: usize,
) -> io::Result<String> {
    const MARK: &str = r#","requests_truncated":true"#;
    let prefix = format!(r#"{{"snapshot":{snapshot},"requests":["#);
    let fixed = ok_reply("").len() + prefix.len() + 3; // ]} 与换行
    let too_long = || io::Error::new(io::ErrorKind::InvalidData, "snapshot exceeds reply limit");
    let budget = limit.checked_sub(fixed).ok_or_else(too_long)?;
    let (rows, truncated) = sub.opening_requests(budget, MARK.len());
    if truncated && budget < MARK.len() {
        return Err(too_long());
    }
    let mark = if truncated { MARK } else { "" };
    Ok(ok_reply(&format!("{prefix}{rows}]{mark}}}")))
}

/// `subscribe`：先回一行应答（快照与在途的记录），之后每行一个事件，直到任一方关闭连接、
/// 写不出去或实例退出。
///
/// 事件有五种：`request`（一条记录变了，整条替换）、`state`（整份快照：状态变化立即发，
/// 计数的变化合并成每秒最多一个）、`heartbeat`（[`EVENT_IDLE`] 内没有别的事件）、
/// `lagged`（订阅者跟不上，丢了 `dropped` 个事件，它该重取快照与记录）、`closing`（实例
/// 要退出了）。每条连接上事件的 `seq` 从 1 起递增。
async fn serve_events<R, W>(
    mut reader: R,
    mut writer: W,
    state: Arc<ControlState>,
) -> io::Result<()>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    use crate::history::Event;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::broadcast::error::{RecvError, TryRecvError};

    let Some(mut subscription) = state.history.subscribe() else {
        let reply = Rejection::Busy {
            limit: crate::history::MAX_SUBSCRIBERS,
        }
        .reply();
        return write_line(&mut writer, &reply).await;
    };
    let mut view = SubscriberStatus::new(&state);
    // 最近一次发给这个订阅者的快照：之后只在它变了的时候再发
    let mut sent_snapshot = view.json();
    let first = opening_reply(
        &subscription,
        &sent_snapshot,
        aproxy_ipc::wire::MAX_REPLY_LINE,
    )?;
    write_line(&mut writer, &first).await?;

    let mut seq = 0u64;
    let mut last_line = tokio::time::Instant::now();
    let mut stopping = state.on_shutdown.subscribe();
    let mut every_second = tokio::time::interval(Duration::from_secs(1));
    every_second.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut scratch = [0u8; 256];
    // 这一轮要写出去的事件行
    let mut batch = String::new();
    loop {
        batch.clear();
        // 唤醒时收到的第一项和批内后续项走同一条路径，尤其不能漏算在批内发现的溢出。
        let mut first = tokio::select! {
            received = subscription.events.recv() => Some(match received {
                Ok(event) => Ok(event),
                Err(RecvError::Lagged(n)) => Err(TryRecvError::Lagged(n)),
                Err(RecvError::Closed) => return Ok(()),
            }),
            _ = state.stats.state_changed.notified() => None,
            changed = stopping.changed() => match changed { Ok(()) => None, Err(_) => return Ok(()) },
            _ = every_second.tick() => {
                if last_line.elapsed() >= EVENT_IDLE { push_event(&mut batch, &mut seq, "heartbeat", None, "{}"); }
                None
            },
            // 客户端关了连接。它在请求行之后发来的别的东西一概不看。
            read = reader.read(&mut scratch) => match read { Ok(0) | Err(_) => return Ok(()), Ok(_) => continue },
        };
        if first.is_none() && batch.is_empty() {
            push_state_if_changed(&mut batch, &mut seq, &state, &mut view, &mut sent_snapshot);
        }
        // 队列里已经排着的事件一并带上，一次写出去。实例忙的时候，每写一次都要等运行时再排到
        // 这个任务；一次只写一个事件的话，每秒几百个事件就跟不上，订阅者会不停地收到 lagged。
        let mut last = false;
        while !last && batch.len() < EVENT_BATCH_BYTES {
            match first
                .take()
                .unwrap_or_else(|| subscription.events.try_recv())
            {
                Ok(Event::Request { at, record }) => {
                    push_event(&mut batch, &mut seq, "request", Some(at), &record);
                }
                Ok(Event::Format {
                    at,
                    index,
                    revision,
                    snapshot,
                }) => {
                    if view.apply(index, revision, &snapshot) {
                        sent_snapshot = view.json();
                        push_event(&mut batch, &mut seq, "state", Some(at), &sent_snapshot);
                    }
                }
                Ok(Event::Closing(reason)) => {
                    push_state_if_changed(
                        &mut batch,
                        &mut seq,
                        &state,
                        &mut view,
                        &mut sent_snapshot,
                    );
                    let data = format!(r#"{{"reason":"{reason}"}}"#);
                    push_event(&mut batch, &mut seq, "closing", None, &data);
                    last = true;
                }
                Err(TryRecvError::Lagged(lost)) => {
                    view.refresh(&state, true);
                    let (dropped, closing) =
                        recover_subscription(&mut subscription, &mut view, &state, lost);
                    push_event(
                        &mut batch,
                        &mut seq,
                        "lagged",
                        None,
                        &format!(r#"{{"dropped":{dropped}}}"#),
                    );
                    sent_snapshot = view.json();
                    push_event(&mut batch, &mut seq, "state", None, &sent_snapshot);
                    if let Some(reason) = closing {
                        push_event(
                            &mut batch,
                            &mut seq,
                            "closing",
                            None,
                            &format!(r#"{{"reason":"{reason}"}}"#),
                        );
                        last = true;
                    }
                }
                // 队列关了的那一种留给下一轮的 recv 去结束
                Err(TryRecvError::Empty | TryRecvError::Closed) => break,
            }
        }
        if batch.is_empty() {
            continue;
        }
        let write = async {
            writer.write_all(batch.as_bytes()).await?;
            writer.flush().await
        };
        match tokio::time::timeout(EVENT_WRITE_TIMEOUT, write).await {
            Ok(written) => written?,
            Err(_) => return Ok(()),
        }
        last_line = tokio::time::Instant::now();
        if last {
            return Ok(());
        }
    }
}

/// 处理一次广播溢出；返回未送达的总数以及应在快照之后送出的关闭原因。
fn recover_subscription(
    subscription: &mut crate::history::Subscription,
    view: &mut SubscriberStatus,
    state: &ControlState,
    lost: u64,
) -> (u64, Option<&'static str>) {
    let result = subscription.resync(lost, |index, rev, data| {
        view.apply(index, rev, data);
    });
    // 关闭可能就发生在取得初始快照时；closing 之前也要送上此刻的实例状态。
    view.refresh(state, false);
    result
}

/// 订阅者看过的每个池的修订号。定时刷新可带上同一代的失败次数，但不能抢发队列里
/// 尚未交付的状态；否则短暂的 half_open 会被它之后的 closed 越过。
struct SubscriberStatus {
    /// 配置位置、进程池和订阅者已收到的状态修订号，位置不受脱敏后名字重复的影响。
    pools: Vec<(usize, std::sync::Weak<crate::transform::Pool>, u64)>,
    /// 最近一次取样的活动与实例状态，以及到当前事件为止的各池状态。
    snapshot: InstanceStatus,
}
impl SubscriberStatus {
    /// 先订阅再取基线；比基线早的队列项在 apply 中忽略。
    fn new(state: &ControlState) -> Self {
        let (pools, formats) = state
            .stats
            .formats
            .iter()
            .enumerate()
            .filter_map(|(index, pool)| {
                let pool = pool.upgrade()?;
                let (revision, format) = pool.snapshot();
                Some(((index, Arc::downgrade(&pool), revision), format))
            })
            .unzip();
        Self {
            pools,
            snapshot: status_with_formats(state, formats),
        }
    }
    /// 队列提供当前变化自己的快照，不再向活池查询此刻可能已经不同的状态。
    fn apply(&mut self, index: usize, revision: u64, value: &serde_json::Value) -> bool {
        for ((id, _, seen), slot) in self.pools.iter_mut().zip(&mut self.snapshot.formats) {
            if *id == index && revision > *seen {
                *seen = revision;
                slot.clone_from(value);
                return true;
            }
        }
        false
    }
    /// 队列溢出时按 lagged 契约重新建立基线；日常采样不能跳过事件。
    fn refresh(&mut self, state: &ControlState, resync: bool) {
        let mut formats = std::mem::take(&mut self.snapshot.formats);
        for ((_, pool, seen), slot) in self.pools.iter_mut().zip(&mut formats) {
            let Some(pool) = pool.upgrade() else {
                continue;
            };
            let (revision, current) = pool.snapshot();
            if revision == *seen || resync {
                *seen = revision;
                *slot = current;
            }
        }
        self.snapshot = status_with_formats(state, formats);
    }
    /// 初始应答和每一条 state 都使用 ping 的既有形状。
    fn json(&self) -> String {
        serde_json::to_string(&self.snapshot).expect("序列化状态快照失败")
    }
}

/// 定时采样、状态唤醒和 `closing` 之前：快照与上一次发送的不一样时补一个 `state` 事件。
///
/// 实例是先改状态（`stopping`、更换阶段）、再宣布关闭的，但两件事可能在这个任务被排到之前
/// 都已经发生，那时先处理哪一个没有定数。在这里补发，订阅者就总是先看到状态的变化、再看到
/// `closing`，不会带着一份过时的状态被关掉连接。
fn push_state_if_changed(
    batch: &mut String,
    seq: &mut u64,
    state: &ControlState,
    view: &mut SubscriberStatus,
    sent_snapshot: &mut String,
) {
    view.refresh(state, false);
    let now = view.json();
    if now != *sent_snapshot {
        push_event(batch, seq, "state", None, &now);
        *sent_snapshot = now;
    }
}

/// 往 `batch` 里加一行事件：序号加一。`at` 是事件发生的时刻（`request` 事件自带，是记录
/// 成为这个样子的时刻），没有的取此刻。`data` 是事件数据的 JSON 文本。
///
/// 所以 `at` 不保证逐行不减：一个排了几毫秒队的 `request` 事件可以比它前面那一行的 `state`
/// 略早。次序以 `seq` 为准。
fn push_event(batch: &mut String, seq: &mut u64, event: &str, at: Option<u64>, data: &str) {
    use std::fmt::Write as _;
    *seq += 1;
    let _ = writeln!(
        batch,
        r#"{{"v":{IPC_VERSION},"event":"{event}","seq":{seq},"at":{},"data":{data}}}"#,
        at.unwrap_or_else(now_millis)
    );
}

/// 组装此刻的状态快照（计数器、最近错误在应答瞬间现读）。
fn status_snapshot(state: &ControlState) -> InstanceStatus {
    let formats = state
        .stats
        .formats
        .iter()
        .filter_map(|p| p.upgrade().map(|p| p.snapshot().1))
        .collect();
    status_with_formats(state, formats)
}

/// 非 format 状态保持原有采样频率，熔断事件只替换它自己那一项。
fn status_with_formats(state: &ControlState, formats: Vec<serde_json::Value>) -> InstanceStatus {
    use std::sync::atomic::Ordering::Relaxed;
    let stats = &state.stats;
    let phase = if *state.on_shutdown.borrow() {
        InstanceState::Stopping
    } else if stats.starting.load(std::sync::atomic::Ordering::Acquire) {
        InstanceState::Starting
    } else if stats.swap_until_ms.load(Relaxed) > now_millis() {
        InstanceState::SwapPrepared
    } else {
        InstanceState::Serving
    };
    InstanceStatus {
        instance: state.record.clone(),
        state: phase,
        behavior: state.behavior.clone(),
        activity: stats.activity(state.record.started_at),
        formats,
        ops: IpcOp::NAMES.iter().map(|name| name.to_string()).collect(),
        features: IpcOp::FEATURES
            .iter()
            .map(|name| name.to_string())
            .collect(),
    }
}

async fn write_line<W>(writer: &mut W, line: &str) -> io::Result<()>
where
    W: tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::AsyncWriteExt;
    writer.write_all(line.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await
}

// ---------------------------------------------------------------------------
// 平台实现：windows 命名管道 / unix domain socket，上层不感知差异。
// ---------------------------------------------------------------------------
#[cfg(windows)]
pub(crate) use imp::PipeSecurity;
#[cfg(windows)]
mod imp {
    use super::{ControlState, handle_conn};
    use std::{io, sync::Arc, time::Duration};
    use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};

    /// 控制管道的安全描述符：受保护的显式 DACL，只有两项——当前用户与 SYSTEM，各自完全
    /// 控制。每个管道实例创建时都带它。
    ///
    /// 不带它时管道用默认的安全描述符，其中有给 Everyone 的读权限；控制通道能停实例，
    /// 以后还能读请求内容，只该让启动实例的用户（与本来就无所不能的 SYSTEM）连上。同一
    /// 用户提权后的进程用的还是同一个用户 SID，照样连得上。
    pub struct PipeSecurity(crate::fsx::Descriptor);

    impl PipeSecurity {
        /// 为当前用户建描述符。
        pub fn for_current_user() -> io::Result<Self> {
            // D:P = 受保护的 DACL（不继承）；GA = 完全控制；SY = SYSTEM
            crate::fsx::Descriptor::from_sddl(&format!(
                "D:P(A;;GA;;;{})(A;;GA;;;SY)",
                crate::fsx::current_user_sid()?
            ))
            .map(Self)
        }

        /// 按 `options` 建一个带这份描述符的管道实例。
        pub(crate) fn create(
            &self,
            options: &ServerOptions,
            endpoint: &str,
        ) -> io::Result<NamedPipeServer> {
            let mut attributes = self.0.attributes();
            // SAFETY: attributes 是一个有效的 SECURITY_ATTRIBUTES，它指向的描述符在 self
            // 存活期间有效；CreateNamedPipe 在调用期间拷贝描述符，不保留指针
            unsafe {
                options.create_with_security_attributes_raw(
                    endpoint,
                    (&mut attributes as *mut windows_sys::Win32::Security::SECURITY_ATTRIBUTES)
                        .cast(),
                )
            }
        }
    }

    /// 已独占创建的端点：第一个管道实例在手，名字从此归本进程。
    pub struct Listener {
        endpoint: String,
        first: NamedPipeServer,
        security: PipeSecurity,
    }

    /// `first_pipe_instance`：名字已被别的进程占着就失败，而不是加入它、和它
    /// 分摊连接——那样一半的 stop/ping 会落到别人手里。管道拒绝远程客户端（tokio 的
    /// 默认值），只让当前用户与 SYSTEM 连接（见 [`PipeSecurity`]）；建不出安全描述符时
    /// 宁可没有控制通道，也不建一个谁都能连的。
    pub async fn bind(endpoint: &str) -> io::Result<Listener> {
        let security = PipeSecurity::for_current_user()?;
        let first = security.create(ServerOptions::new().first_pipe_instance(true), endpoint)?;
        Ok(Listener {
            endpoint: endpoint.to_string(),
            first,
            security,
        })
    }

    /// 接受循环：为每个连接 spawn 处理任务，并始终备好下一个监听实例。不会
    /// 返回——单个连接出错只丢掉那一个实例，控制通道本身要一直在。
    pub async fn accept_loop(listener: Listener, state: Arc<ControlState>) {
        let Listener {
            endpoint,
            first: mut server,
            security,
        } = listener;
        loop {
            let connected = server.connect().await;
            // 先建好下一个实例，再交出（或丢掉）当前这个：管道名只在至少还有
            // 一个实例时存在，名字一消失，客户端就会把「找不到」当成实例已退出
            let next = loop {
                match security.create(&ServerOptions::new(), &endpoint) {
                    Ok(next) => break next,
                    Err(e) => {
                        tracing::warn!(error = %e, "控制管道新建监听实例失败，1 秒后重试");
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }
            };
            let current = std::mem::replace(&mut server, next);
            match connected {
                Ok(()) => {
                    let state = state.clone();
                    tokio::spawn(async move {
                        let _ = handle_conn(current, state).await;
                    });
                }
                // 客户端在服务端接上之前就断开等：只影响这一个连接
                Err(e) => tracing::debug!(error = %e, "控制管道连接未建立，换一个监听实例继续"),
            }
        }
    }
}

#[cfg(unix)]
mod imp {
    use super::{ControlState, handle_conn};
    use std::{io, path::Path, sync::Arc, time::Duration};
    use tokio::net::UnixListener;

    /// 服务端该不该接受这个客户端：同一个用户，或 root（它本来就无所不能，放行它
    /// `sudo aproxy stop` 才能用）。
    pub(super) fn accepted_client_uid(me: u32, client: u32) -> bool {
        client == me || client == 0
    }

    /// 在 `path` 上建监听 socket，并把它的权限收到 0600：只有属主连得上。bind 按 umask
    /// 建文件，随后才改权限，中间有一小段窗口；这段窗口里连进来的别的用户仍然过不了接受
    /// 循环里的对端 uid 核对。权限改不成时不留下这个 socket。
    fn bind_private(path: &Path) -> io::Result<UnixListener> {
        use std::os::unix::fs::PermissionsExt;
        let listener = UnixListener::bind(path)?;
        if let Err(e) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) {
            drop(listener);
            let _ = std::fs::remove_file(path);
            return Err(e);
        }
        Ok(listener)
    }

    pub struct Listener {
        listener: UnixListener,
        path: std::path::PathBuf,
    }

    /// 独占创建 socket。路径上已有文件时先探测：连得上（或积压队列满）说明有
    /// 进程正在用它（同一 home 同端口的另一个实例），拒绝——删掉它的 socket 等于
    /// 把它的控制通道抢走，它从此 stop/status 不可达；其余连接失败都说明没有
    /// 进程在听（崩溃残留），删掉重建。
    pub async fn bind(endpoint: &str) -> io::Result<Listener> {
        let path = Path::new(endpoint);
        // 端点先于注册表创建，首次启动时 run 目录可能还不存在
        if let Some(dir) = path.parent() {
            crate::fsx::create_dir_all(dir)?;
        }
        match tokio::net::UnixStream::connect(path).await {
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) if e.kind() != io::ErrorKind::WouldBlock => {
                let _ = std::fs::remove_file(path);
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    format!("已有进程在 {endpoint} 上提供控制通道"),
                ));
            }
        }
        bind_private(path).map(|listener| Listener {
            listener,
            path: path.to_path_buf(),
        })
    }

    /// 接受循环，兼管 socket 文件。文件被别的进程删掉时，监听还在、名字没了，
    /// 客户端从此找不到本实例。0.1.0 的看护者就会这么做：它处理实例的死亡事件
    /// 时删 `run/<端口>.sock`，迁移到新版本时它若还没退役，那条路径上可能已是
    /// 新实例的 socket。所以每秒看一眼，名字没了就在原路径重建；本进程自己在
    /// 退出（已收到 shutdown，或退出流程删了 socket）时不重建。
    pub async fn accept_loop(listener: Listener, state: Arc<ControlState>) {
        let Listener { mut listener, path } = listener;
        let mut check = tokio::time::interval(Duration::from_secs(1));
        check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                accepted = listener.accept() => {
                    let stream = match accepted {
                        Ok((stream, _)) => stream,
                        Err(_) => {
                            // 持续性 accept 错误（如 fd 耗尽的 EMFILE）不会自行恢复，
                            // 立即重试会形成占满 CPU 的紧死循环；睡一拍给错误源恢复机会。
                            tokio::time::sleep(Duration::from_millis(100)).await;
                            continue;
                        }
                    };
                    // 只接受本用户与 root 的连接；对端是谁都读不出来的连接同样不接受。
                    // 不读请求、不回应答，直接断开
                    // SAFETY: geteuid 没有前置条件，也不会失败
                    let me = unsafe { libc::geteuid() };
                    match stream.peer_cred().map(|cred| cred.uid()) {
                        Ok(client) if accepted_client_uid(me, client) => {}
                        Ok(client) => {
                            tracing::warn!(uid = client, "控制 socket 拒绝了另一个用户的连接");
                            continue;
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "控制 socket 读不出对端的身份，连接已拒绝");
                            continue;
                        }
                    }
                    let state = state.clone();
                    tokio::spawn(async move {
                        let _ = handle_conn(stream, state).await;
                    });
                }
                _ = check.tick() => {
                    let retiring = *state.on_shutdown.borrow()
                        || super::SOCKET_RETIRED.load(std::sync::atomic::Ordering::Relaxed);
                    let missing = matches!(
                        std::fs::symlink_metadata(&path),
                        Err(e) if e.kind() == io::ErrorKind::NotFound
                    );
                    if missing && !retiring {
                        match bind_private(&path) {
                            Ok(fresh) => {
                                listener = fresh;
                                tracing::warn!(path = %path.display(), "控制 socket 文件被删除，已在原路径重建");
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, "控制 socket 文件被删除，重建失败，1 秒后再试");
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    /// 用测试进程自己持有的双工流向服务端发原始帧（包括不合法的帧）。
    async fn exchange_over(mut stream: tokio::io::DuplexStream, line: &str) -> io::Result<String> {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
        stream.write_all(line.as_bytes()).await?;
        stream.write_all(b"\n").await?;
        let mut reply = String::new();
        tokio::io::BufReader::new(stream)
            .read_line(&mut reply)
            .await?;
        Ok(reply)
    }

    fn sample_info(port: &str) -> InstanceRecord {
        InstanceRecord {
            pid: 42,
            start: String::new(),
            instance_id: "19ac3f2e8b5d1a2b".into(),
            version: "0.0.0-test".into(),
            listen_addr: format!("127.0.0.1:{port}"),
            config_path: Some("C:/tmp/config.toml".into()),
            base_url: "https://api.example.com".into(),
            started_at: 1_700_000_000_000,
            log_path: None,
            foreground: false,
        }
    }

    fn sample_behavior() -> Behavior {
        Behavior {
            delivery: "retry".into(),
            disk_cache: true,
            keepalive: KeepaliveBehavior {
                trigger: "any".into(),
                interval_secs: 15,
            },
            retry_rules: 1,
            history: HistoryBehavior { body_budget_mb: 64 },
        }
    }

    /// 一份空的请求历史：1 MiB 的体预算，认得一个密钥
    fn sample_history() -> Arc<crate::history::History> {
        crate::history::History::new(
            1,
            aproxy_ipc::redact::Secrets::new(["sk-live-0123456789abcdef"]),
            Arc::from("19ac3f2e8b5d1a2b"),
        )
    }

    /// 测试用的控制服务状态；返回的 Receiver 用来观察 shutdown 是否被触发
    fn control(record: InstanceRecord) -> (Arc<ControlState>, tokio::sync::watch::Receiver<bool>) {
        let (on_shutdown, rx) = tokio::sync::watch::channel(false);
        let state = ControlState {
            record,
            behavior: sample_behavior(),
            run_dir: "/test/run".into(),
            stats: Arc::new(IpcStats::default()),
            history: sample_history(),
            on_shutdown,
            stop_requested: std::sync::Mutex::new(false),
        };
        (Arc::new(state), rx)
    }

    /// 一份只为占位的拉起规格：日志指向 `log_path`
    fn sample_spec(log_path: &str) -> crate::spec::InstanceSpec {
        crate::spec::InstanceSpec {
            schema: crate::spec::SCHEMA,
            config_path: None,
            overrides: Default::default(),
            pinned_port: None,
            env: Default::default(),
            log_path: log_path.to_string(),
        }
    }

    #[test]
    fn port_of_extracts_trailing_port() {
        assert_eq!(port_of("127.0.0.1:12345"), "12345");
        assert_eq!(port_of("[::1]:8080"), "8080");
        assert_eq!(port_of("no-port"), "no-port");
    }

    // ---------------- 进程身份（pid + 创建时间，与二进制名称无关） ----------------
    //
    // 被测对象是名字与 aProxy 无关的子进程（Windows PING.EXE / Linux sleep），
    // 由测试自己 spawn 并回收：旧的镜像名关卡会一律拒绝它，新判定只看 pid +
    // 创建时间。macOS 读不到创建时间（不在本轮支持范围），只在 Windows/Linux
    // 编译运行。端口用 597xx 段：没有其他测试在用，避免与「端点必须无人应答」
    // 的测试（59xxx）互相干扰。

    #[cfg(any(windows, target_os = "linux"))]
    use crate::process::tests::{spawn_unrelated_child, wait_child_exit};

    /// stop --force 的身份策略：登记时间不符（pid 复用）拒绝；无时间戳的旧
    /// 记录且端点无人应答（归属无从证明）拒绝；身份一致即终止——被杀的进程
    /// 名字与 aProxy 无关，证明判定不看名称
    #[cfg(any(windows, target_os = "linux"))]
    #[tokio::test]
    async fn force_terminate_checks_identity_not_name() {
        let mut child = spawn_unrelated_child();
        let pid = child.id();
        let start = aproxy_ipc::ledger::start_marker(pid).expect("子进程的启动标记可查");
        let mut info = sample_info(&format!("597{:02}", std::process::id() % 50));
        info.pid = pid;

        info.start = format!("{start}0");
        let err = force_terminate(&info).unwrap_err();
        assert!(
            err.contains("已不是记录里的那个进程"),
            "pid 复用应以身份不符拒绝: {err}"
        );

        info.start = String::new();
        let err = force_terminate(&info).unwrap_err();
        assert!(err.contains("核验"), "未登记启动标记应拒绝: {err}");
        std::thread::sleep(Duration::from_millis(200));
        assert!(
            child.try_wait().unwrap().is_none(),
            "拒绝后子进程必须仍在运行"
        );

        info.start = start;
        force_terminate(&info).expect("身份一致应终止成功");
        assert!(wait_child_exit(&mut child), "终止后子进程应退出");
    }

    #[test]
    fn registry_find_pid_returns_record_with_actual_addr() {
        let dir = tempfile::tempdir().unwrap();
        let mut info = sample_info("59731");
        info.pid = 5151;
        write_instance_file_in(dir.path(), &info).unwrap();
        let found = registry_find_pid_in(dir.path(), 5151).expect("应按 pid 找到记录");
        assert_eq!(found.listen_addr, "127.0.0.1:59731");
        assert!(registry_find_pid_in(dir.path(), 5152).is_none());
        assert!(read_instance_file_in(dir.path(), "59731").is_some());
        assert!(read_instance_file_in(dir.path(), "59732").is_none());
    }

    /// 重拉/恢复落到新端口后退役旧端口记录：同端口不动；旧端口记录属于已死
    /// 进程 → 期望状态记录与实例记录一并删除；旧端口已被另一个在世实例接手（身份
    /// 核验通过，或旧版本记录 pid 在世无从核验）→ 记录归它，不动
    #[cfg(any(windows, target_os = "linux"))]
    #[test]
    fn retire_moved_port_records_only_touches_dead_owner() {
        let dir = tempfile::tempdir().unwrap();
        let write = |port: &str, pid: u32, start: &str| {
            let mut info = sample_info(port);
            info.pid = pid;
            info.start = start.to_string();
            write_instance_file_in(dir.path(), &info).unwrap();
            crate::spec::write_restore_in(dir.path(), port, &sample_spec("")).unwrap();
        };
        let has = |port: &str| {
            (
                crate::spec::restore_path_in(dir.path(), port).is_file(),
                instance_file_path_in(dir.path(), port).is_file(),
            )
        };

        write("59741", u32::MAX - 777, "w:123");
        assert!(!retire_moved_port_records_in(dir.path(), "59741", "59741"));
        assert_eq!(has("59741"), (true, true), "同端口不得退役");
        assert!(retire_moved_port_records_in(dir.path(), "59741", "59742"));
        assert_eq!(
            has("59741"),
            (false, false),
            "已死进程的旧端口记录应整体退役"
        );

        let mut child = spawn_unrelated_child();
        let pid = child.id();
        let start = aproxy_ipc::ledger::start_marker(pid).expect("子进程的启动标记可查");
        write("59743", pid, &start);
        assert!(!retire_moved_port_records_in(dir.path(), "59743", "59744"));
        assert_eq!(has("59743"), (true, true), "在世实例接手的端口不得退役");
        // 未登记启动标记：证明不了是哪个进程，按无主记录退役
        write("59745", pid, "");
        assert!(retire_moved_port_records_in(dir.path(), "59745", "59746"));
        assert_eq!(has("59745"), (false, false));
        // 登记的标记不符 = pid 已复用，记录的进程已死 → 退役
        write("59747", pid, &format!("{start}0"));
        assert!(retire_moved_port_records_in(dir.path(), "59747", "59748"));
        assert_eq!(has("59747"), (false, false));

        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn registry_contains_pid_matches_by_parsed_record() {
        let dir = tempfile::tempdir().unwrap();
        let mut info = sample_info("59901");
        info.pid = 4242;
        write_instance_file_in(dir.path(), &info).unwrap();

        assert!(registry_contains_pid_in(dir.path(), 4242));
        assert!(!registry_contains_pid_in(dir.path(), 4243));
        // 不是实例记录后缀的文件不参与检索
        std::fs::write(dir.path().join("59901.restore"), "args").unwrap();
        assert!(registry_contains_pid_in(dir.path(), 4242));
    }

    #[test]
    fn registry_contains_pid_skips_corrupt_and_missing_dir() {
        let dir = tempfile::tempdir().unwrap();
        // 损坏记录（不可解析）跳过，不 panic 不误报
        std::fs::write(dir.path().join("59902.instance.json"), "not-json").unwrap();
        assert!(!registry_contains_pid_in(dir.path(), 1));
        // 目录不存在 → false（respawn 轮询的前置态）
        assert!(!registry_contains_pid_in(
            dir.path().join("no-such").as_path(),
            1
        ));
    }

    /// 期望状态记录的写与删在同一把锁下：收到过不带 `keep_restore` 的 shutdown 之后不再写。
    #[tokio::test]
    async fn stop_deletes_the_restore_record_and_blocks_later_writes() {
        let dir = tempfile::tempdir().unwrap();
        let record = sample_info("59761");
        let make = || {
            let (on_shutdown, _rx) = tokio::sync::watch::channel(false);
            ControlState {
                record: record.clone(),
                behavior: sample_behavior(),
                run_dir: dir.path().display().to_string(),
                stats: Arc::new(IpcStats::default()),
                history: sample_history(),
                on_shutdown,
                stop_requested: std::sync::Mutex::new(false),
            }
        };
        let path = crate::spec::restore_path_in(dir.path(), "59761");
        let spec = sample_spec("");

        // 启动时写入
        let state = make();
        assert!(state.write_restore(&spec).unwrap());
        assert!(path.is_file());
        // stop：记录删掉，之后的写入被拒（启动途中收到 stop 的实例不会把记录写回来）
        state.forget_restore();
        assert!(!path.exists());
        assert!(!state.write_restore(&spec).unwrap());
        assert!(!path.exists());

        // 带 keep_restore 的 shutdown 不走 forget_restore：经真实的请求处理验证
        let state = Arc::new(make());
        assert!(state.write_restore(&spec).unwrap());
        let (client, server) = tokio::io::duplex(4096);
        let handler = tokio::spawn(handle_conn(server, state.clone()));
        let line = IpcOp::STOP_KEEPING_RESTORE.request_line();
        let reply = exchange_over(client, &line).await.unwrap();
        handler.await.unwrap().unwrap();
        assert_eq!(parse_reply(&reply).unwrap().state, InstanceState::Stopping);
        assert!(path.is_file(), "keep_restore 的 shutdown 不得删记录");
        // 同一个实例随后收到 stop：记录删掉
        let (client, server) = tokio::io::duplex(4096);
        let handler = tokio::spawn(handle_conn(server, state.clone()));
        exchange_over(client, &IpcOp::STOP.request_line())
            .await
            .unwrap();
        handler.await.unwrap().unwrap();
        assert!(!path.exists(), "stop 应删掉记录");
    }

    /// 往测试用的请求历史里放一个结束了的请求：一个请求体、一次带响应的尝试。返回它的标识。
    fn recorded_request(history: &Arc<crate::history::History>, seq: u64) -> String {
        let id = format!("19ac3f2e8b5d1a2b-{seq}");
        let mut headers = http::HeaderMap::new();
        headers.insert("x-api-key", "sk-live-0123456789abcdef".parse().unwrap());
        let recorder = history.begin(
            seq,
            Arc::from(id.as_str()),
            crate::history::Start {
                method: "POST",
                path_and_query: "/v1/messages",
                headers: &headers,
                delivery: "retry",
                phase: crate::history::Phase::ReadingBody,
            },
        );
        let mut body = recorder.request_body(None).unwrap();
        body.feed(&bytes::Bytes::from_static(
            br#"{"model":"m","key":"sk-live-0123456789abcdef"}"#,
        ));
        body.finish();
        recorder.try_started(1, None, true, None);
        let mut response = recorder
            .response_head(
                1,
                200,
                [("content-type", &b"application/json"[..])].into_iter(),
                None,
            )
            .unwrap();
        response.feed(&bytes::Bytes::from_static(br#"{"ok":true}"#));
        response.finish();
        recorder.try_ended(1, 11, None);
        recorder.client_status(200);
        recorder.end(true);
        id
    }

    /// 读类 op 的拒绝：未知参数、缺必填、类型或取值不对、取不到。应答行逐字钉住。
    #[test]
    fn read_rejections_are_pinned() {
        let (state, _rx) = control(sample_info("12345"));
        let id = recorded_request(&state.history, 42);
        let reply = |line: &str| match parse_request(&format!("{line}\n")) {
            Err(rejection) => rejection.reply(),
            Ok(Parsed::Read(op)) => read_reply(&op, &state.history).unwrap_err().reply(),
            Ok(other) => panic!("{line}: {other:?}"),
        };
        let bad = |arg: &str| {
            format!(
                r#"{{"v":1,"ok":false,"error":{{"code":"bad_request","message":"arg {arg} has the wrong type or value","details":{{"reason":"invalid_arg","arg":"{arg}"}}}}}}"#
            )
        };
        let missing = |arg: &str| {
            format!(
                r#"{{"v":1,"ok":false,"error":{{"code":"bad_request","message":"arg {arg} is required","details":{{"reason":"missing_arg","arg":"{arg}"}}}}}}"#
            )
        };
        let not_found = |request_id: &str, reason: &str| {
            format!(
                r#"{{"v":1,"ok":false,"error":{{"code":"not_found","message":"nothing to return for request {request_id}: {reason}","details":{{"request_id":"{request_id}","reason":"{reason}"}}}}}}"#
            )
        };
        let body =
            |args: &str| format!(r#"{{"v":1,"op":"body","args":{{"request_id":"{id}",{args}}}}}"#);
        let cases: Vec<(String, String)> = vec![
            (
                r#"{"v":1,"op":"subscribe","args":{"filter":"x"}}"#.into(),
                r#"{"v":1,"ok":false,"error":{"code":"unsupported_arg","message":"unknown arg: filter","details":{"args":["filter"]}}}"#.into(),
            ),
            (
                r#"{"v":1,"op":"requests","args":{"since":1}}"#.into(),
                r#"{"v":1,"ok":false,"error":{"code":"unsupported_arg","message":"unknown arg: since","details":{"args":["since"]}}}"#.into(),
            ),
            (r#"{"v":1,"op":"requests","args":{"limit":0}}"#.into(), bad("limit")),
            (r#"{"v":1,"op":"requests","args":{"limit":201}}"#.into(), bad("limit")),
            (r#"{"v":1,"op":"requests","args":{"before":7}}"#.into(), bad("before")),
            (r#"{"v":1,"op":"request"}"#.into(), missing("request_id")),
            (r#"{"v":1,"op":"request","args":{"request_id":7}}"#.into(), bad("request_id")),
            (r#"{"v":1,"op":"body","args":{"request_id":"x"}}"#.into(), missing("part")),
            (body(r#""part":"headers""#), bad("part")),
            (body(r#""part":"sent""#), missing("attempt")),
            (body(r#""part":"response","attempt":0"#), bad("attempt")),
            (body(r#""part":"request","limit":0"#), bad("limit")),
            (body(r#""part":"request","limit":1048577"#), bad("limit")),
            (body(r#""part":"request","offset":-1"#), bad("offset")),
            // 起点在体的末尾之后
            (body(r#""part":"request","offset":1000"#), bad("offset")),
            (body(r#""part":"response","attempt":2"#), not_found(&id, "no_such_part")),
            (
                r#"{"v":1,"op":"request","args":{"request_id":"19ac3f2e8b5d1a2b-7"}}"#.into(),
                not_found("19ac3f2e8b5d1a2b-7", "unknown_request"),
            ),
            (
                r#"{"v":1,"op":"body","args":{"request_id":"nope","part":"request"}}"#.into(),
                not_found("nope", "unknown_request"),
            ),
            (
                r#"{"v":1,"op":"requests","args":{"before":"nope"}}"#.into(),
                not_found("nope", "unknown_request"),
            ),
        ];
        for (line, expected) in &cases {
            assert_eq!(&reply(line), expected, "{line}");
        }
        assert_eq!(
            Rejection::NotFound {
                request_id: id.clone(),
                reason: "not_retained",
                cause: Some("exceeds_budget"),
            }
            .reply(),
            format!(
                r#"{{"v":1,"ok":false,"error":{{"code":"not_found","message":"nothing to return for request {id}: not_retained","details":{{"request_id":"{id}","reason":"not_retained","cause":"exceeds_budget"}}}}}}"#
            )
        );
        assert_eq!(
            Rejection::Busy { limit: 8 }.reply(),
            r#"{"v":1,"ok":false,"error":{"code":"busy","message":"all 8 streaming connections are in use","details":{"limit":8}}}"#
        );
        // 码与 details 的键都在码表里
        let mut replies: Vec<String> = cases.iter().map(|(line, _)| reply(line)).collect();
        replies.push(Rejection::Busy { limit: 8 }.reply());
        replies.push(
            Rejection::NotFound {
                request_id: id,
                reason: "not_retained",
                cause: Some("decode_failed"),
            }
            .reply(),
        );
        for text in replies {
            let value: serde_json::Value = serde_json::from_str(&text).unwrap();
            let code = aproxy_ipc::code::lookup(value["error"]["code"].as_str().unwrap())
                .expect("码表里应有这个码");
            for key in value["error"]["details"].as_object().unwrap().keys() {
                assert!(
                    code.details.iter().any(|d| d.key == key),
                    "{} 的 details 里有码表没登记的键 {key}",
                    code.name
                );
            }
        }
    }

    /// `requests`、`request`、`body` 经真实的端点走一遍：结果的形状，以及体里的密钥已经
    /// 在入库时换掉。
    #[tokio::test]
    async fn subscription_rejects_a_claimed_pid_other_than_the_pipe_peer() {
        let dir = tempfile::tempdir().unwrap();
        let endpoint = endpoint_for_in(dir.path(), "wrong-peer");
        let listener = imp::bind(&endpoint).await.unwrap();
        let (state, _rx) = control(sample_info("0"));
        let server = tokio::spawn(imp::accept_loop(listener, state));
        let result = ipc_subscribe_in(dir.path(), "wrong-peer").await;
        server.abort();
        assert!(matches!(result, Err(IpcError::Protocol(_))));
    }

    #[tokio::test]
    async fn read_ops_round_trip_over_a_real_endpoint() {
        use crate::history::Part;
        use base64::Engine as _;
        let dir = tempfile::tempdir().unwrap();
        let endpoint = endpoint_for_in(dir.path(), "readops");
        let mut record = sample_info("0");
        record.pid = std::process::id();
        let (state, _rx) = control(record);
        let first = recorded_request(&state.history, 41);
        let second = recorded_request(&state.history, 42);
        let server = tokio::spawn(serve_endpoint(endpoint.clone(), state.clone()));
        tokio::time::sleep(Duration::from_millis(100)).await;
        let read = |op: ReadOp| {
            let run = dir.path();
            async move { ipc_read_in(run, "readops", &op).await }
        };

        let list = read(ReadOp::Requests {
            before: None,
            limit: Some(1),
        })
        .await
        .unwrap();
        assert_eq!(list["more"], true);
        assert_eq!(list["requests"].as_array().unwrap().len(), 1);
        assert_eq!(list["requests"][0]["request_id"], second.as_str());
        assert_eq!(list["requests"][0]["outcome"], "ok");
        let older = read(ReadOp::Requests {
            before: Some(second.clone()),
            limit: None,
        })
        .await
        .unwrap();
        assert_eq!(older["more"], false);
        assert_eq!(older["requests"][0]["request_id"], first.as_str());

        let detail = read(ReadOp::Request {
            request_id: second.clone(),
        })
        .await
        .unwrap();
        assert_eq!(detail["record"], list["requests"][0]);
        assert_eq!(
            detail["request"],
            serde_json::json!({
                "headers": [{"name": "x-api-key", "value": {"secret": true, "hint": "sk-l…cdef"}}],
                "body": {"bytes": 30, "state": "retained"},
            })
        );
        assert_eq!(
            detail["tries"],
            serde_json::json!([{
                "attempt": 1,
                "sent": null,
                "response": {
                    "upstream_status": 200,
                    "headers": [{"name": "content-type", "value": "application/json"}],
                    "body": {"bytes": 11, "state": "retained"},
                },
                "error": null,
            }])
        );

        let chunk = read(ReadOp::Body {
            request_id: second.clone(),
            part: Part::Request,
            attempt: None,
            offset: Some(4),
            limit: Some(10),
        })
        .await
        .unwrap();
        let data = base64::engine::general_purpose::STANDARD
            .decode(chunk["data_b64"].as_str().unwrap())
            .unwrap();
        // 入库的是换掉密钥之后的：{"model":"m","key":"<secret>"}
        assert_eq!(String::from_utf8(data).unwrap(), r#"del":"m",""#);
        assert_eq!(
            (
                &chunk["total_bytes"],
                &chunk["offset"],
                &chunk["more"],
                &chunk["complete"]
            ),
            (
                &serde_json::json!(30),
                &serde_json::json!(4),
                &serde_json::json!(true),
                &serde_json::json!(true)
            )
        );
        let response = read(ReadOp::Body {
            request_id: second.clone(),
            part: Part::Response,
            attempt: Some(1),
            offset: None,
            limit: None,
        })
        .await
        .unwrap();
        assert_eq!(response["data_b64"], "eyJvayI6dHJ1ZX0=");
        assert_eq!(response["more"], false);

        let err = read(ReadOp::Request {
            request_id: "19ac3f2e8b5d1a2b-7".into(),
        })
        .await
        .unwrap_err();
        assert!(
            matches!(&err, IpcError::Remote { code, details, .. }
                if code == "not_found" && details["reason"] == "unknown_request"),
            "{err:?}"
        );
        server.abort();
    }

    /// 读一行事件，按 JSON 解析。
    async fn next_event<R>(lines: &mut tokio::io::BufReader<R>) -> serde_json::Value
    where
        R: tokio::io::AsyncRead + Unpin,
    {
        use tokio::io::AsyncBufReadExt;
        let mut line = String::new();
        tokio::time::timeout(Duration::from_secs(5), lines.read_line(&mut line))
            .await
            .expect("5 秒内应有一行")
            .unwrap();
        assert!(line.ends_with('\n'), "连接提前关闭：{line:?}");
        serde_json::from_str(&line).unwrap()
    }

    /// 单线程运行时先填满队列，再让订阅任务运行；快照相同也须重新发，不能重放旧事件。
    #[tokio::test]
    async fn lagged_subscription_counts_all_skipped_events_then_sends_current_state() {
        let (state, _rx) = control(sample_info("0"));
        let (client, server) = tokio::io::duplex(64);
        let (r, w) = tokio::io::split(server);
        let handler = tokio::spawn(serve_events(r, w, state.clone()));
        let mut lines = tokio::io::BufReader::new(client);
        let first = next_event(&mut lines).await;
        assert_eq!(first["ok"], true);
        for revision in 1..=2048 {
            state
                .history
                .format_transition(0, revision, || serde_json::json!({}));
        }
        state.history.announce_closing("shutdown");
        let lag = next_event(&mut lines).await;
        assert_eq!(lag["event"], "lagged");
        assert_eq!(lag["data"]["dropped"], 2048);
        let snapshot = next_event(&mut lines).await;
        assert_eq!(snapshot["event"], "state");
        assert_eq!(
            snapshot["data"]["state"],
            first["result"]["snapshot"]["state"]
        );
        assert_eq!(next_event(&mut lines).await["event"], "closing");
        handler.await.unwrap().unwrap();
    }

    /// 在取过旧队尾之后再写满两轮并关机；保留的后半轮也不能漏计，closing 前要看到 stopping。
    #[tokio::test]
    async fn second_overflow_accounts_for_retained_events_and_closing_state() {
        let (state, _keep) = control(sample_info("0"));
        let mut sub = state.history.subscribe().unwrap();
        let mut view = SubscriberStatus::new(&state);
        for rev in 1..=2048 {
            state
                .history
                .format_transition(0, rev, || serde_json::json!({}));
        }
        let Err(tokio::sync::broadcast::error::TryRecvError::Lagged(n)) = sub.events.try_recv()
        else {
            panic!("expected first overflow")
        };
        view.refresh(&state, true);
        for rev in 2049..=4096 {
            state
                .history
                .format_transition(0, rev, || serde_json::json!({}));
        }
        state.on_shutdown.send(true).unwrap();
        state.history.announce_closing("shutdown");
        let (lost, closing) = recover_subscription(&mut sub, &mut view, &state, n);
        assert_eq!(lost, 4096);
        assert_eq!(closing, Some("shutdown"));
        assert_eq!(view.snapshot.state, InstanceState::Stopping);
        assert!(sub.events.is_empty());
    }

    /// `subscribe`：先回快照与在途的记录，之后是事件——记录的开始与结束、状态变化、实例
    /// 退出。每条连接上 `seq` 从 1 起递增。
    #[tokio::test]
    async fn opening_fits_exact_limit_and_marks_only_an_omitted_suffix() {
        let (state, _keep) = control(sample_info("0"));
        let begin = |seq| {
            state.history.begin(
                seq,
                format!("run-{seq}").into(),
                crate::history::Start {
                    method: "GET",
                    path_and_query: "/",
                    headers: &http::HeaderMap::new(),
                    delivery: "retry",
                    phase: crate::history::Phase::ReadingBody,
                },
            )
        };
        let _first = begin(41);
        let sub = state.history.subscribe().unwrap();
        let one = opening_reply(&sub, "{}", usize::MAX).unwrap();
        assert_eq!(opening_reply(&sub, "{}", one.len() + 1).unwrap(), one);
        let _second = begin(42);
        let limit = one.len() + 1 + r#","requests_truncated":true"#.len();
        let cut = opening_reply(&sub, "{}", limit).unwrap();
        assert_eq!(cut.len() + 1, limit);
        let doc: serde_json::Value = serde_json::from_str(&cut).unwrap();
        assert_eq!(doc["result"]["requests_truncated"], true);
        assert_eq!(doc["result"]["requests"][0]["request_seq"], 41);
    }

    #[tokio::test]
    async fn subscribe_streams_snapshot_then_events() {
        use tokio::io::AsyncWriteExt;
        let (state, _rx) = control(sample_info("12345"));
        // 订阅之前就在途的请求
        let headers = http::HeaderMap::new();
        let begin = |seq: u64| {
            state.history.begin(
                seq,
                Arc::from(format!("19ac3f2e8b5d1a2b-{seq}")),
                crate::history::Start {
                    method: "POST",
                    path_and_query: "/v1/messages",
                    headers: &headers,
                    delivery: "retry",
                    phase: crate::history::Phase::ReadingBody,
                },
            )
        };
        let early = begin(1);

        let (mut client, server) = tokio::io::duplex(1 << 20);
        let handler = tokio::spawn(handle_conn(server, state.clone()));
        client
            .write_all(b"{\"v\":1,\"op\":\"subscribe\"}\n")
            .await
            .unwrap();
        let (read_half, _write_half) = tokio::io::split(client);
        let mut lines = tokio::io::BufReader::new(read_half);

        let first = next_event(&mut lines).await;
        assert_eq!(first["ok"], true);
        assert_eq!(first["result"]["snapshot"]["state"], "serving");
        assert_eq!(
            first["result"]["snapshot"]["instance"]["listen_addr"],
            "127.0.0.1:12345"
        );
        assert_eq!(first["result"]["requests"].as_array().unwrap().len(), 1);
        assert_eq!(first["result"]["requests"][0]["request_seq"], 1);
        assert_eq!(state.history.subscribers(), 1);

        // 开始与结束各一个 request 事件，data 是整条记录
        let recorder = begin(2);
        let started = next_event(&mut lines).await;
        assert_eq!(
            (&started["v"], &started["event"], &started["seq"]),
            (
                &serde_json::json!(1),
                &serde_json::json!("request"),
                &serde_json::json!(1)
            )
        );
        assert!(started["at"].as_u64().is_some_and(|at| at > 0));
        assert_eq!(started["data"]["request_seq"], 2);
        assert_eq!(started["data"]["outcome"], "in_flight");
        recorder.end(true);
        let ended = next_event(&mut lines).await;
        assert_eq!(ended["seq"], 2);
        assert_eq!(ended["data"]["outcome"], "ok");
        assert_eq!(ended["data"]["phase"], "done");
        early.end(false);
        let gone = next_event(&mut lines).await;
        assert_eq!(gone["data"]["outcome"], "client_gone");

        // 状态变化：另一条连接上的 prepare_swap 让订阅者立即收到一份新的快照
        let (other, other_server) = tokio::io::duplex(1 << 16);
        let other_handler = tokio::spawn(handle_conn(other_server, state.clone()));
        exchange_over(other, &IpcOp::PREPARE_SWAP.request_line())
            .await
            .unwrap();
        other_handler.await.unwrap().unwrap();
        let swapped = next_event(&mut lines).await;
        assert_eq!(swapped["event"], "state");
        assert_eq!(swapped["seq"], 4);
        assert_eq!(swapped["data"]["state"], "swap_prepared");
        assert_eq!(swapped["data"]["ops"], first["result"]["snapshot"]["ops"]);

        // 实例退出：closing 之后服务端关连接，名额还回去
        state.history.announce_closing("swap");
        let closing = next_event(&mut lines).await;
        assert_eq!(closing["event"], "closing");
        assert_eq!(closing["data"], serde_json::json!({"reason": "swap"}));
        handler.await.unwrap().unwrap();
        assert_eq!(state.history.subscribers(), 0);
    }

    /// 实例退出时状态先变、再宣布关闭；两件事在订阅的任务被排到之前都已发生时，订阅者仍然
    /// 先收到状态的变化，再收到 `closing`。先处理哪一个是随机的，所以多试几十次。
    #[tokio::test]
    async fn closing_always_follows_the_state_change() {
        use tokio::io::AsyncWriteExt;
        for round in 0..50 {
            let (state, _rx) = control(sample_info("12345"));
            let (mut client, server) = tokio::io::duplex(1 << 16);
            let handler = tokio::spawn(handle_conn(server, state.clone()));
            client
                .write_all(
                    b"{\"v\":1,\"op\":\"subscribe\"}
",
                )
                .await
                .unwrap();
            let (read_half, _write_half) = tokio::io::split(client);
            let mut lines = tokio::io::BufReader::new(read_half);
            assert_eq!(next_event(&mut lines).await["ok"], true);

            // 中间不让出执行权：订阅的任务下一次被排到时，两件事都已经发生
            state.on_shutdown.send(true).unwrap();
            state.history.announce_closing("shutdown");

            let changed = next_event(&mut lines).await;
            assert_eq!(changed["event"], "state", "第 {round} 轮：{changed}");
            assert_eq!(changed["data"]["state"], "stopping");
            let closing = next_event(&mut lines).await;
            assert_eq!(closing["event"], "closing", "第 {round} 轮：{closing}");
            assert_eq!(closing["seq"], 2);
            handler.await.unwrap().unwrap();
        }
    }

    /// 一次试探可以在订阅任务再次获得调度前开始又取消；两次状态变化都要出现在 IPC 上。
    #[tokio::test]
    async fn subscription_preserves_breaker_transitions_between_reads() {
        use tokio::io::AsyncWriteExt;
        let dir = tempfile::tempdir().unwrap();
        let (mut control, _rx) = control(sample_info("0"));
        let state = Arc::get_mut(&mut control).unwrap();
        let (cfg, diagnostics) = crate::config::file::parse(
            "[[format]]\ncommand='example'\nprocess='persistent'\nstages=['request']\npool_max=1\nbreaker_cooldown_secs=1",
            &dir.path().join("config.toml"),
        );
        assert!(diagnostics.is_empty());
        let mut entry = cfg.format.unwrap().pop().unwrap();
        entry.command = crate::transform::format_echo_path().display().to_string();
        let pool = crate::transform::Pool::new(
            entry,
            dir.path().into(),
            Default::default(),
            Some((0, Arc::downgrade(&state.history))),
            [1024; 2],
        );
        Arc::get_mut(&mut state.stats).unwrap().formats = vec![Arc::downgrade(&pool)];
        let held = pool.turn(false).await.unwrap();
        let (mut client, server) = tokio::io::duplex(1 << 16);
        let handler = tokio::spawn(handle_conn(server, control));
        client
            .write_all(b"{\"v\":1,\"op\":\"subscribe\"}\n")
            .await
            .unwrap();
        let mut lines = tokio::io::BufReader::new(client);
        let first = next_event(&mut lines).await;
        assert_eq!(
            first["result"]["snapshot"]["formats"][0]["breaker"]["state"],
            "closed"
        );
        pool.open_at_startup(crate::transform::Fault::Spawn);
        assert_eq!(
            next_event(&mut lines).await["data"]["formats"][0]["breaker"]["state"],
            "open"
        );
        tokio::time::sleep(Duration::from_millis(1050)).await;
        let mut probe = Box::pin(pool.turn(false));
        assert!(futures_util::poll!(&mut probe).is_pending());
        drop(probe);
        for expected in ["half_open", "open"] {
            let event = next_event(&mut lines).await;
            assert_eq!(event["event"], "state");
            assert_eq!(event["data"]["formats"][0]["breaker"]["state"], expected);
        }
        drop(lines);
        handler.await.unwrap().unwrap();
        drop(held);
        pool.shutdown().await;
    }

    /// 流式连接最多 8 条，第 9 条回 `busy`；客户端关了连接，名额就还回来。
    #[tokio::test]
    async fn subscribers_are_capped_and_a_closed_client_frees_its_slot() {
        use tokio::io::AsyncWriteExt;
        let (state, _rx) = control(sample_info("12345"));
        let mut clients = Vec::new();
        for _ in 0..crate::history::MAX_SUBSCRIBERS {
            let (mut client, server) = tokio::io::duplex(1 << 16);
            let handler = tokio::spawn(handle_conn(server, state.clone()));
            client
                .write_all(b"{\"v\":1,\"op\":\"subscribe\"}\n")
                .await
                .unwrap();
            let (read_half, write_half) = tokio::io::split(client);
            let mut lines = tokio::io::BufReader::new(read_half);
            assert_eq!(next_event(&mut lines).await["ok"], true);
            clients.push((lines, write_half, handler));
        }
        assert_eq!(state.history.subscribers(), crate::history::MAX_SUBSCRIBERS);

        let (client, server) = tokio::io::duplex(1 << 16);
        let handler = tokio::spawn(handle_conn(server, state.clone()));
        let reply = exchange_over(client, r#"{"v":1,"op":"subscribe"}"#)
            .await
            .unwrap();
        handler.await.unwrap().unwrap();
        assert_eq!(
            reply.trim_end(),
            r#"{"v":1,"ok":false,"error":{"code":"busy","message":"all 8 streaming connections are in use","details":{"limit":8}}}"#
        );

        // 一个客户端走了：它那条连接的处理任务结束，名额还回来
        let (lines, write_half, handler) = clients.pop().unwrap();
        drop((lines, write_half));
        tokio::time::timeout(Duration::from_secs(5), handler)
            .await
            .expect("客户端关闭后处理任务应结束")
            .unwrap()
            .unwrap();
        assert_eq!(
            state.history.subscribers(),
            crate::history::MAX_SUBSCRIBERS - 1
        );
    }

    /// 成功应答的形状钉在这里：快照的字段名、顺序与类型。
    #[test]
    fn snapshot_shape_is_pinned() {
        let (state, _rx) = control(sample_info("12345"));
        state.stats.requests_total.store(120, Ordering::Relaxed);
        state.stats.retries_total.store(7, Ordering::Relaxed);
        let reply = serde_json::to_string(&WireResponse {
            v: IPC_VERSION,
            ok: true,
            result: Some(status_snapshot(&state)),
            error: None,
        })
        .unwrap();
        assert_eq!(
            reply.replace(&["0"; BUCKET_COUNT].join(","), "ZEROS"),
            concat!(
                r#"{"v":1,"ok":true,"result":{"#,
                r#""instance":{"pid":42,"start":"","instance_id":"19ac3f2e8b5d1a2b","version":"0.0.0-test","listen_addr":"127.0.0.1:12345","config_path":"C:/tmp/config.toml","base_url":"https://api.example.com","started_at":1700000000000,"log_path":null,"foreground":false},"#,
                r#""state":"serving","#,
                r#""behavior":{"delivery":"retry","disk_cache":true,"keepalive":{"trigger":"any","interval_secs":15},"retry_rules":1,"history":{"body_budget_mb":64}},"#,
                r#""activity":{"last_request_at":1700000000000,"requests_total":120,"retries_total":7,"rescued_total":0,"upgraded_total":0,"in_flight":0,"retrying":0,"by_code":{},"#,
                r#""buckets":{"width_secs":120,"requests":[ZEROS],"retries":[ZEROS],"failures":[ZEROS]},"last_error":null},"#,
                r#""formats":[],"#,
                r#""ops":["ping","shutdown","prepare_swap","requests","request","body","subscribe"],"#,
                r#""features":["shutdown.if_idle_secs","shutdown.keep_restore","prepare_swap.ttl_secs","redacted_client_fields"]}}"#
            )
        );
        // 客户端读回同一份
        let status = parse_reply(&reply).unwrap();
        assert_eq!(status.behavior, sample_behavior());
        assert_eq!(status.state, InstanceState::Serving);
    }

    #[test]
    fn client_reads_v1_errors_and_unknown_states() {
        let err = parse_reply(
            r#"{"v":1,"ok":false,"error":{"code":"unsupported_arg","message":"x","details":{"args":["a"]},"later":1}}"#,
        )
        .unwrap_err();
        assert!(
            matches!(&err, IpcError::Remote { code, details, .. }
                if code == "unsupported_arg" && details["args"][0] == "a"),
            "{err}"
        );
        // 没有 details 的错误也读得出
        let err = parse_reply(r#"{"v":1,"ok":false,"error":{"code":"unknown_op","message":"x"}}"#)
            .unwrap_err();
        assert!(
            matches!(&err, IpcError::Remote { code, details, .. }
                if code == "unknown_op" && details.is_null()),
            "{err}"
        );
        assert!(matches!(
            parse_reply(r#"{"v":2,"ok":true}"#),
            Err(IpcError::Protocol(_))
        ));
        // 别的版本的应答里只认一种：实例说它不支持请求的版本，并列出它支持哪些。别的
        // 版本的其余应答（哪怕也是个错误）形状不保证，一律不采信
        let err = parse_reply(
            r#"{"v":2,"ok":false,"error":{"code":"unsupported_version","message":"x","details":{"supported":[2]},"shape":"new"}}"#,
        )
        .unwrap_err();
        assert!(
            matches!(&err, IpcError::Remote { code, details, .. }
                if code == "unsupported_version" && details["supported"][0] == 2),
            "{err}"
        );
        assert!(matches!(
            parse_reply(r#"{"v":2,"ok":false,"error":{"code":"unknown_op","message":"x"}}"#),
            Err(IpcError::Protocol(_))
        ));
        // 更新版本的实例：不认识的状态读作 Unknown，不认识的字段（顶层、快照里、instance 里、
        // behavior 里）一律忽略，没见过的枚举值照样读得出
        let mut instance = serde_json::to_value(sample_info("1")).unwrap();
        instance["added_later"] = serde_json::json!({"x": 1});
        let newer = serde_json::json!({"v": 1, "ok": true, "extra": 1, "result": {
            "instance": instance, "state": "draining",
            "behavior": {"delivery": "adaptive", "disk_cache": false,
                         "keepalive": {"trigger": "path", "interval_secs": 0, "later": 1},
                         "retry_rules": 0, "history": {"body_budget_mb": 64}},
            "formats": [{"name": "adaptive"}],
            "activity": {"last_request_at": 1, "requests_total": 0, "retries_total": 0,
                         "last_error": null, "in_flight": 2},
            "ops": ["ping", "subscribe"], "features": ["subscribe.filter"],
            "future_field": true}});
        let status = parse_reply(&newer.to_string()).unwrap();
        assert_eq!(status.state, InstanceState::Unknown);
        assert_eq!(status.behavior.delivery, "adaptive");
        assert_eq!(status.ops, ["ping", "subscribe"]);
        // `starting` 是认得的状态
        let mut starting = newer.clone();
        starting["result"]["state"] = "starting".into();
        assert_eq!(
            parse_reply(&starting.to_string()).unwrap().state,
            InstanceState::Starting
        );
        // 没有 v 的应答（0.1.0 守护的形状）无法采信
        assert!(matches!(
            parse_reply(r#"{"ok":true,"info":{"pid":7},"proto":2}"#),
            Err(IpcError::Protocol(_))
        ));
    }

    /// 实例记录的形状是跨版本读的：文件名、字段名、顺序与类型钉在这里。快照里的 `instance`
    /// 是同一个对象去掉 `schema`。
    #[test]
    fn instance_record_shape_is_pinned() {
        let record = InstanceRecord {
            pid: 4321,
            start: "w:133712345678901234".into(),
            instance_id: "19ac3f2e8b5d1a2b".into(),
            version: "0.2.0".into(),
            listen_addr: "127.0.0.1:12345".into(),
            config_path: Some("/home/u/.aproxy/config.toml".into()),
            base_url: "https://api.example.com".into(),
            started_at: 1_760_190_000_123,
            log_path: Some("/home/u/.aproxy/logs/19ac3f2e8b5d-1a2b.log".into()),
            foreground: false,
        };
        let in_snapshot = r#"{"pid":4321,"start":"w:133712345678901234","instance_id":"19ac3f2e8b5d1a2b","version":"0.2.0","listen_addr":"127.0.0.1:12345","config_path":"/home/u/.aproxy/config.toml","base_url":"https://api.example.com","started_at":1760190000123,"log_path":"/home/u/.aproxy/logs/19ac3f2e8b5d-1a2b.log","foreground":false}"#;
        assert_eq!(serde_json::to_string(&record).unwrap(), in_snapshot);

        let dir = tempfile::tempdir().unwrap();
        write_instance_file_in(dir.path(), &record).unwrap();
        let on_disk = std::fs::read_to_string(dir.path().join("12345.instance.json")).unwrap();
        assert_eq!(
            on_disk,
            format!(
                "{{\"schema\":1,{}
",
                &in_snapshot[1..]
            ),
            "盘上的形状是 schema 在前、其余与快照里的 instance 相同"
        );
        assert_eq!(parse_instance_file(&on_disk), Some(record));

        // 前台实例、没有配置文件：两个路径都是 null
        let foreground = parse_instance_file(
            r#"{"schema":1,"pid":1,"start":"","instance_id":"a","version":"0.2.0","listen_addr":"127.0.0.1:1","config_path":null,"base_url":"https://x","started_at":1,"log_path":null,"foreground":true,"later":1}"#,
        )
        .expect("以后追加的字段读得过去");
        assert!(foreground.foreground);
        assert_eq!((foreground.config_path, foreground.log_path), (None, None));
        // schema 更高的记录不解读；旧版本的形状读不懂
        assert_eq!(
            parse_instance_file(&on_disk.replace("\"schema\":1", "\"schema\":2")),
            None
        );
        assert_eq!(
            parse_instance_file(
                r#"{"pid":7,"process_start":1,"version":"0.1.0","listen_addr":"127.0.0.1:1","config_path":"c","base_url":"b","started_at":1,"log_path":""}"#
            ),
            None
        );
    }

    #[test]
    fn instance_file_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let info = sample_info("45678");
        write_instance_file_in(dir.path(), &info).unwrap();
        let path = instance_file_path_in(dir.path(), "127.0.0.1:45678");
        assert_eq!(path.file_name().unwrap(), "45678.instance.json");
        let loaded = parse_instance_file(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(loaded.pid, 42);
        assert_eq!(loaded.listen_addr, "127.0.0.1:45678");
    }

    #[tokio::test]
    async fn control_ops_round_trip_over_a_real_endpoint() {
        let dir = tempfile::tempdir().unwrap();
        let endpoint = test_endpoint(dir.path(), "roundtrip");
        let mut record = sample_info("0");
        record.pid = std::process::id();
        let (state, mut rx) = control(record);
        state.stats.requests_total.store(7, Ordering::Relaxed);
        state.stats.retries_total.store(2, Ordering::Relaxed);
        state.stats.note_failed_attempt(
            "upstream_status",
            serde_json::json!({ "status": 502 }).as_object().unwrap(),
            "the upstream returned 502",
        );
        let server = tokio::spawn(serve_endpoint(endpoint.clone(), state.clone()));
        tokio::time::sleep(Duration::from_millis(100)).await;

        // ping：状态快照带实时观测值与能力清单
        let status = request_raw(&endpoint, IpcOp::Ping).await.unwrap();
        assert_eq!(status.instance.pid, std::process::id());
        assert_eq!(status.state, InstanceState::Serving);
        assert_eq!(status.behavior, sample_behavior());
        assert_eq!(status.features.len(), 3);
        assert_eq!(
            status.ops,
            [
                "ping",
                "shutdown",
                "prepare_swap",
                "requests",
                "request",
                "body",
                "subscribe"
            ]
        );
        assert_eq!(status.activity.requests_total, 7);
        assert_eq!(status.activity.retries_total, 2);
        let last = status.activity.last_error.expect("应带最近错误");
        assert!(last.message.contains("502") && last.at > 0);
        assert_eq!(last.code, "upstream_status");
        assert_eq!(last.details["status"], 502);
        assert_eq!(status.activity.by_code["upstream_status"], 1);
        assert_eq!(status.activity.buckets.failures.len(), BUCKET_COUNT);
        assert_eq!(status.activity.buckets.failures.last(), Some(&1));
        // 还没收到过请求：最近活动取启动时刻，不是 0
        assert_eq!(status.activity.last_request_at, status.instance.started_at);

        // 实例不认识的参数：整个请求被拒，什么都没做
        let (client, conn) = tokio::io::duplex(4096);
        let handler = tokio::spawn(handle_conn(conn, state.clone()));
        let reply = exchange_over(client, r#"{"v":1,"op":"shutdown","args":{"drain_secs":5}}"#)
            .await
            .unwrap();
        handler.await.unwrap().unwrap();
        assert!(
            matches!(parse_reply(&reply), Err(IpcError::Remote { code, .. }) if code == "unsupported_arg")
        );
        assert!(!*rx.borrow(), "被拒的 shutdown 不得触发停止");

        // 带条件的 shutdown：实例刚收到过请求，不够闲 → 拒绝，照常服务
        state
            .stats
            .last_activity_ms
            .store(now_millis(), Ordering::Relaxed);
        let err = request_raw(
            &endpoint,
            IpcOp::Shutdown {
                keep_restore: false,
                if_idle_secs: Some(3600),
            },
        )
        .await
        .unwrap_err();
        assert!(
            matches!(&err, IpcError::Remote { code, details, .. }
                if code == "condition_not_met" && details["idle_ms"].as_u64().is_some_and(|ms| ms < 60_000)),
            "{err}"
        );
        assert!(!*rx.borrow(), "条件不成立时不得触发停止");
        assert_eq!(
            request_raw(&endpoint, IpcOp::Ping).await.unwrap().state,
            InstanceState::Serving
        );

        // prepare_swap：应答里立即是 swap_prepared（安装器的 ACK 判定），之后也是
        let swap = request_raw(&endpoint, IpcOp::PREPARE_SWAP).await.unwrap();
        assert_eq!(swap.state, InstanceState::SwapPrepared);
        let again = request_raw(&endpoint, IpcOp::Ping).await.unwrap();
        assert_eq!(again.state, InstanceState::SwapPrepared);
        // 到期之后自己回到 serving：安装者中途没了，实例不会永远停在更换阶段
        state
            .stats
            .swap_until_ms
            .store(now_millis() - 1, Ordering::Relaxed);
        assert_eq!(
            request_raw(&endpoint, IpcOp::Ping).await.unwrap().state,
            InstanceState::Serving
        );
        // 带时长的 prepare_swap 重新进入，按给的时长计
        let before = now_millis();
        let swap = request_raw(
            &endpoint,
            IpcOp::PrepareSwap {
                ttl_secs: Some(120),
            },
        )
        .await
        .unwrap();
        assert_eq!(swap.state, InstanceState::SwapPrepared);
        let until = state.stats.swap_until_ms.load(Ordering::Relaxed);
        assert!(
            (before + 120_000..=now_millis() + 120_000).contains(&until),
            "{until}"
        );

        // 带条件的 shutdown：条件成立（闲置 0 秒总是成立）就停。应答先到（状态 stopping），
        // 随后置位停止信号
        let stopping = request_raw(
            &endpoint,
            IpcOp::Shutdown {
                keep_restore: false,
                if_idle_secs: Some(0),
            },
        )
        .await
        .unwrap();
        assert_eq!(stopping.state, InstanceState::Stopping);
        rx.changed().await.unwrap();
        assert!(*rx.borrow());
        // 已经在退出：重复的 shutdown 结果相同，条件不再判断
        state
            .stats
            .last_activity_ms
            .store(now_millis(), Ordering::Relaxed);
        let again = request_raw(
            &endpoint,
            IpcOp::Shutdown {
                keep_restore: false,
                if_idle_secs: Some(3600),
            },
        )
        .await
        .unwrap();
        assert_eq!(again.state, InstanceState::Stopping);
        // 退出中不再进入更换阶段
        let err = request_raw(&endpoint, IpcOp::PREPARE_SWAP)
            .await
            .unwrap_err();
        assert!(
            matches!(&err, IpcError::Remote { code, details, .. }
                if code == "invalid_state" && details["state"] == "stopping"),
            "{err}"
        );

        server.abort();
    }

    #[test]
    fn last_error_is_overwritten_and_its_message_truncated() {
        let stats = IpcStats::default();
        assert!(stats.last_error().is_none());
        let none = serde_json::Map::new();
        stats.note_failed_attempt("upstream_status", &none, "first");
        stats.note_terminal("retry_exhausted", &none, "second");
        let last = stats.last_error().unwrap();
        assert_eq!(
            (last.code.as_str(), last.message.as_str()),
            ("retry_exhausted", "second")
        );
        // 超长的说明截断到 200 字符（按 char，多字节不撕裂）
        stats.note_failed_attempt("upstream_connect_failed", &none, &"长".repeat(500));
        assert_eq!(stats.last_error().unwrap().message.chars().count(), 200);
    }

    #[test]
    fn snapshot_redacts_peer_text_before_truncation() {
        let history = crate::history::History::new(
            0,
            aproxy_ipc::redact::Secrets::new(["test-key-012345"]),
            "run".into(),
        );
        let stats = IpcStats::sharing(Arc::default(), history);
        let details = serde_json::json!({"encoding":"test-key-012345"});
        stats.note_terminal(
            "decode_failed",
            details.as_object().unwrap(),
            &format!("{}test-key-012345", "x".repeat(190)),
        );
        let error = stats.activity(0).last_error.unwrap();
        assert_eq!(error.details["encoding"], "<secret>");
        assert!(error.message.ends_with("<secret>"));
    }

    /// 计数与时间桶：请求、重试、失败的尝试各进各的桶；终态码只进按码计数；窗口随时间
    /// 前移，移出窗口的桶清零。
    #[test]
    fn activity_counts_by_code_and_by_time_bucket() {
        let stats = Arc::new(IpcStats::default());
        let none = serde_json::Map::new();
        let width_ms = BUCKET_WIDTH_SECS * 1000;
        let t0 = 1_760_190_000_000 / width_ms * width_ms;

        assert_eq!(stats.note_request_at(t0), 1);
        assert_eq!(stats.note_request_at(t0 + 1), 2);
        stats.note_retry_at(t0 + 2);
        stats.note_failure_at(t0 + 3, "upstream_status", &none, "x", true);
        stats.note_failure_at(t0 + 4, "retry_exhausted", &none, "y", false);
        // 下一个桶
        assert_eq!(stats.note_request_at(t0 + width_ms), 3);
        stats.note_failure_at(t0 + width_ms, "upstream_status", &none, "z", true);
        stats.note_rescued();

        let activity = stats.activity_at(t0 + width_ms + 5, t0);
        let tail = |counts: &[u64]| counts[BUCKET_COUNT - 2..].to_vec();
        assert_eq!(activity.buckets.width_secs, BUCKET_WIDTH_SECS);
        for counts in [
            &activity.buckets.requests,
            &activity.buckets.retries,
            &activity.buckets.failures,
        ] {
            assert_eq!(counts.len(), BUCKET_COUNT);
        }
        assert_eq!(tail(&activity.buckets.requests), [2, 1]);
        assert_eq!(tail(&activity.buckets.retries), [1, 0]);
        assert_eq!(
            tail(&activity.buckets.failures),
            [1, 1],
            "终态码不进 failures"
        );
        assert_eq!(activity.by_code["upstream_status"], 2);
        assert_eq!(activity.by_code["retry_exhausted"], 1);
        assert_eq!(
            (
                activity.requests_total,
                activity.retries_total,
                activity.rescued_total
            ),
            (3, 1, 1)
        );
        assert_eq!(activity.last_error.unwrap().code, "upstream_status");

        // 没有新事件，只是时间过去：读快照时桶照样前移
        let later = stats.activity_at(t0 + 3 * width_ms, t0);
        assert_eq!(later.buckets.requests[BUCKET_COUNT - 4..], [2, 1, 0, 0]);
        // 整个窗口都过去了：全部清零，累计值不变
        let much_later = stats.activity_at(t0 + (BUCKET_COUNT as u64 + 5) * width_ms, t0);
        assert!(much_later.buckets.requests.iter().all(|n| *n == 0));
        assert_eq!(much_later.requests_total, 3);
        // 时钟回拨：记进当前最新的桶，不动窗口
        stats.note_request_at(t0);
        let after_rewind = stats.activity_at(t0, t0);
        assert_eq!(after_rewind.buckets.requests.last(), Some(&1));

        // 在途与重试中由守卫维护
        let a = stats.enter();
        let b = stats.enter();
        let retrying = stats.begin_retrying();
        let now = stats.activity_at(t0, t0);
        assert_eq!((now.in_flight, now.retrying), (2, 1));
        drop((a, retrying));
        let now = stats.activity_at(t0, t0);
        assert_eq!((now.in_flight, now.retrying), (1, 0));
        drop(b);
        assert_eq!(stats.activity_at(t0, t0).in_flight, 0);
    }

    #[tokio::test]
    async fn ipc_ping_fails_when_no_instance() {
        // 不存在的端点：ping 必须报错（= 端口上没有 aProxy）
        let run = tempfile::tempdir().unwrap();
        let port = format!("599{:02}", std::process::id() % 100);
        assert!(ipc_ping_in(run.path(), &port).await.is_err());
    }

    /// 测试专用端点：Windows 管道名带测试名与进程号（不会与真实实例重名），
    /// unix 的 socket 放在测试目录里
    fn test_endpoint(dir: &std::path::Path, name: &str) -> String {
        #[cfg(windows)]
        {
            let _ = dir;
            format!(r"\\.\pipe\aproxy-test-{name}-{}", std::process::id())
        }
        #[cfg(unix)]
        {
            dir.join(format!("{name}.sock")).display().to_string()
        }
    }

    #[tokio::test]
    async fn client_rejects_a_responder_whose_pid_is_not_the_peer() {
        // 冒充者：在端点上应答，自报的 pid（42）却不是它自己。客户端必须拒收——
        // 普查与 stop 都按应答里的 pid 认实例
        let dir = tempfile::tempdir().unwrap();
        let endpoint = test_endpoint(dir.path(), "peer");
        let server = tokio::spawn(serve_endpoint(
            endpoint.clone(),
            control(sample_info("0")).0,
        ));
        tokio::time::sleep(Duration::from_millis(100)).await;
        let err = request_raw(&endpoint, IpcOp::Ping).await.unwrap_err();
        assert!(err.to_string().contains("不一致"), "{err}");
        server.abort();
    }

    #[tokio::test]
    async fn binding_a_live_endpoint_fails_and_leaves_its_owner_answering() {
        // 同一 home 同端口的第二个实例：不得与第一个分摊连接（Windows），也不得
        // 删掉它的 socket 抢过来（unix），只能创建失败；第一个照常应答
        let dir = tempfile::tempdir().unwrap();
        let endpoint = test_endpoint(dir.path(), "dup");
        let mut info = sample_info("0");
        info.pid = std::process::id();
        let server = tokio::spawn(serve_endpoint(endpoint.clone(), control(info).0));
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(imp::bind(&endpoint).await.is_err());
        assert!(request_raw(&endpoint, IpcOp::Ping).await.is_ok());
        server.abort();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn binding_over_a_stale_socket_file_succeeds() {
        // 崩溃残留：socket 文件还在，但没有进程在听——清掉重建
        let dir = tempfile::tempdir().unwrap();
        let endpoint = test_endpoint(dir.path(), "stale");
        drop(std::os::unix::net::UnixListener::bind(&endpoint).unwrap());
        assert!(std::path::Path::new(&endpoint).exists());
        // 客户端眼里，崩溃实例留下的 socket 就是「这里没有实例」：stop 据此确认
        // 已退出，普查据此转入进程身份核验
        let probe = request_raw(&endpoint, IpcOp::Ping).await;
        assert!(
            matches!(probe, Err(IpcError::Unreachable(_))),
            "{:?}",
            probe.err()
        );
        let bound = imp::bind(&endpoint).await;
        assert!(bound.is_ok(), "{:?}", bound.err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn deleted_socket_file_is_recreated_while_serving() {
        // 别的进程删了本实例的 socket 文件（还没退役的 0.1.0 看护者会这么做）：
        // 一两秒内在原路径重建，客户端照常连得上
        let dir = tempfile::tempdir().unwrap();
        let endpoint = test_endpoint(dir.path(), "heal");
        let mut record = sample_info("0");
        record.pid = std::process::id();
        let server = tokio::spawn(serve_endpoint(endpoint.clone(), control(record).0));
        tokio::time::sleep(Duration::from_millis(100)).await;
        std::fs::remove_file(&endpoint).unwrap();
        let mut healed = false;
        for _ in 0..30 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            if request_raw(&endpoint, IpcOp::Ping).await.is_ok() {
                healed = true;
                break;
            }
        }
        assert!(healed, "被删的控制 socket 应在原路径重建");
        server.abort();
    }

    /// 控制 socket 只有属主连得上（0600），被删后重建出来的也一样。
    #[cfg(unix)]
    #[tokio::test]
    async fn control_socket_is_private_to_its_owner() {
        use std::os::unix::fs::PermissionsExt;
        let mode = |path: &str| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        let dir = tempfile::tempdir().unwrap();
        let endpoint = test_endpoint(dir.path(), "private");
        let mut record = sample_info("0");
        record.pid = std::process::id();
        let server = tokio::spawn(serve_endpoint(endpoint.clone(), control(record).0));
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(mode(&endpoint), 0o600);

        std::fs::remove_file(&endpoint).unwrap();
        let mut recreated = false;
        for _ in 0..30 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            if std::path::Path::new(&endpoint).exists() {
                recreated = true;
                break;
            }
        }
        assert!(recreated, "被删的控制 socket 应在原路径重建");
        assert_eq!(
            mode(&endpoint),
            0o600,
            "重建出来的 socket 同样只有属主连得上"
        );
        server.abort();
    }

    /// 两端各自信谁：普通用户只信自己；root 作客户端时信 home 的属主与 root；服务端只接受
    /// 本用户与 root。
    #[cfg(unix)]
    #[test]
    fn peers_are_trusted_by_uid() {
        use imp::accepted_client_uid;
        // 服务端
        assert!(accepted_client_uid(1000, 1000));
        assert!(accepted_client_uid(1000, 0));
        assert!(!accepted_client_uid(1000, 1001));
    }

    /// 控制管道的 DACL 只有两项：当前用户与 SYSTEM，各自完全控制，不继承。连接之后新建的
    /// 下一个管道实例同样如此。
    #[cfg(windows)]
    #[tokio::test]
    async fn control_pipe_admits_only_its_owner_and_system() {
        use std::os::windows::io::AsRawHandle;
        use tokio::net::windows::named_pipe::ClientOptions;
        use windows_sys::Win32::Foundation::LocalFree;
        use windows_sys::Win32::Security::Authorization::{
            ConvertSecurityDescriptorToStringSecurityDescriptorW, GetSecurityInfo, SDDL_REVISION_1,
            SE_KERNEL_OBJECT,
        };
        use windows_sys::Win32::Security::{DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR};

        /// 读回一个已打开的管道句柄的 DACL，写成 SDDL。
        fn dacl_of(client: &tokio::net::windows::named_pipe::NamedPipeClient) -> String {
            unsafe {
                let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
                let code = GetSecurityInfo(
                    client.as_raw_handle() as isize,
                    SE_KERNEL_OBJECT,
                    DACL_SECURITY_INFORMATION,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    &mut descriptor,
                );
                assert_eq!(code, 0, "GetSecurityInfo 失败");
                let mut text: *mut u16 = std::ptr::null_mut();
                let ok = ConvertSecurityDescriptorToStringSecurityDescriptorW(
                    descriptor,
                    SDDL_REVISION_1,
                    DACL_SECURITY_INFORMATION,
                    &mut text,
                    std::ptr::null_mut(),
                );
                assert_ne!(ok, 0, "描述符转 SDDL 失败");
                let mut len = 0;
                while *text.add(len) != 0 {
                    len += 1;
                }
                let sddl = String::from_utf16_lossy(std::slice::from_raw_parts(text, len));
                LocalFree(text as _);
                LocalFree(descriptor as _);
                sddl
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let endpoint = test_endpoint(dir.path(), "dacl");
        let mut record = sample_info("0");
        record.pid = std::process::id();
        let server = tokio::spawn(serve_endpoint(endpoint.clone(), control(record).0));
        tokio::time::sleep(Duration::from_millis(100)).await;

        // 第一个连接落在第一个管道实例上；它连上之后服务端建出第二个实例，再连一次就是它
        for round in ["第一个管道实例", "连接之后新建的管道实例"] {
            let client = ClientOptions::new().open(&endpoint).unwrap();
            let sddl = dacl_of(&client);
            assert!(sddl.starts_with("D:P"), "{round}：不继承，{sddl}");
            let mut aces: Vec<&str> = sddl[sddl.find('(').unwrap_or(sddl.len())..]
                .split(['(', ')'])
                .filter(|ace| !ace.is_empty())
                .collect();
            aces.sort();
            assert_eq!(aces.len(), 2, "{round}：只有两项，{sddl}");
            // 「完全控制」读回来是 FA（管道对象上 GENERIC_ALL 映射成的具体权限）
            assert!(aces.contains(&"A;;FA;;;SY"), "{round}：{sddl}");
            let own = aces.iter().find(|ace| **ace != "A;;FA;;;SY").unwrap();
            assert!(own.starts_with("A;;FA;;;"), "{round}：{sddl}");
            // 当前用户恰好是 SDDL 有两个字母别名的账户时读回来的是别名，否则是 SID 原文
            let user = crate::fsx::current_user_sid().unwrap();
            let who = own.rsplit(';').next().unwrap();
            assert!(
                who == user || who.len() == 2,
                "{round}：{sddl}（当前用户 {user}）"
            );
            drop(client);
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        // 带着这份描述符，本用户照常能用控制通道
        assert!(request_raw(&endpoint, IpcOp::Ping).await.is_ok());
        server.abort();
    }

    #[tokio::test]
    async fn survey_does_not_credit_a_record_with_another_processs_answer() {
        // 记录说端口上是进程 X，端点上应答的却是另一个进程：不能把应答者当成
        // 这条记录的实例列出，也不能删记录（它可能正被应答者重写）
        let dir = tempfile::tempdir().unwrap();
        let port = format!("survey-{}", std::process::id());
        let mut live = sample_info(&port);
        live.pid = std::process::id();
        let server = tokio::spawn(serve_endpoint(
            endpoint_for_in(dir.path(), &port),
            control(live.clone()).0,
        ));
        tokio::time::sleep(Duration::from_millis(100)).await;

        let mut stale = live.clone();
        stale.pid = u32::MAX - 778;
        stale.start = "w:123".into();
        write_instance_file_in(dir.path(), &stale).unwrap();
        let survey = survey_instances_in(dir.path()).await;
        assert!(survey.responsive.is_empty(), "{:?}", survey.responsive);
        assert!(survey.unresponsive.is_empty(), "{:?}", survey.unresponsive);
        assert!(read_instance_file_in(dir.path(), &port).is_some());

        // 记录与应答者一致：照常列出
        write_instance_file_in(dir.path(), &live).unwrap();
        let survey = survey_instances_in(dir.path()).await;
        assert_eq!(survey.responsive.len(), 1);
        assert_eq!(survey.responsive[0].instance.pid, live.pid);
        server.abort();
    }

    #[tokio::test]
    async fn listing_skips_dead_records_without_deleting_anything() {
        // 一条指向不存在实例的注册记录、一条读不懂的记录：列举不返回它们，也不删它们
        let dir = tempfile::tempdir().unwrap();
        let dead = sample_info("59801");
        write_instance_file_in(dir.path(), &dead).unwrap();
        let path = instance_file_path_in(dir.path(), "127.0.0.1:59801");
        let garbled = dir.path().join("59809.instance.json");
        std::fs::write(&garbled, "{not json").unwrap();
        let listed = list_instances_in(dir.path()).await;
        assert!(listed.is_empty(), "{listed:?}");
        assert!(path.exists() && garbled.exists(), "读路径不删任何文件");
    }

    #[tokio::test]
    async fn survey_keeps_records_of_live_but_unresponsive_instances() {
        // 进程还在、端点却无人应答（挂死的形态）：用本测试进程自身冒充——
        // pid 与创建时间都核验得上，但 59802 上没有 IPC 端点
        let dir = tempfile::tempdir().unwrap();
        let pid = std::process::id();
        let start = aproxy_ipc::ledger::start_marker(pid).expect("读不到自身的启动标记");
        let mut hung = sample_info("59802");
        hung.pid = pid;
        hung.start = start.clone();
        write_instance_file_in(dir.path(), &hung).unwrap();
        let path = instance_file_path_in(dir.path(), "127.0.0.1:59802");

        let survey = survey_instances_in(dir.path()).await;
        assert!(survey.responsive.is_empty());
        assert_eq!(survey.unresponsive.len(), 1, "挂死实例应归入 unresponsive");
        assert_eq!(survey.unresponsive[0].pid, pid);
        assert!(path.exists(), "进程仍在的记录不能因 ping 失败被删");

        // 同一 pid、创建时间对不上 = pid 已被复用，记录的进程早已死亡 → 不列出；记录留给清理
        hung.start = format!("{start}0");
        write_instance_file_in(dir.path(), &hung).unwrap();
        let survey = survey_instances_in(dir.path()).await;
        assert!(survey.unresponsive.is_empty());
        assert!(path.exists(), "读路径不删任何文件");
    }

    /// 把一个文件的修改时间拨到 `secs` 秒之前
    fn age(path: &std::path::Path, secs: u64) {
        let file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        file.set_modified(std::time::SystemTime::now() - Duration::from_secs(secs))
            .unwrap();
    }

    /// 错误属于原来的握手；旁观者不必等待复用 pid 的进程退出，也不能回收这个进程。
    #[tokio::test]
    async fn observed_startup_failure_survives_a_reused_pid() {
        let home = tempfile::tempdir().unwrap();
        let run = home.path().join("run");
        std::fs::create_dir(&run).unwrap();
        let mut instance = sample_info("0");
        instance.pid = std::process::id();
        instance.start = "another-run".into();
        crate::spawn::publish_startup_failure(
            &run,
            &instance,
            &crate::transform::StartupError {
                name: "fixture".into(),
                fault: crate::transform::Fault::Protocol,
            },
        )
        .unwrap();
        let error = tokio::time::timeout(
            Duration::from_secs(1),
            crate::spawn::wait_existing(home.path(), &instance),
        )
        .await
        .expect("ignored published result")
        .unwrap_err();
        assert!(
            matches!(error, crate::spawn::SpawnError::Exited { format: Some(e), .. } if e.exit_code() == 3 && e.name == "fixture")
        );
    }

    /// 保护期内谁都能读；超过保护期则按创建标记区分写者与占用同一 pid 的其他进程。
    #[cfg(any(windows, target_os = "linux"))]
    #[test]
    fn expired_startup_result_distinguishes_reused_and_live_writers() {
        let home = tempfile::tempdir().unwrap();
        let run = home.path().join("run");
        let logs = home.path().join("logs");
        std::fs::create_dir(&run).unwrap();
        let mut instance = sample_info("0");
        instance.pid = std::process::id();
        let file = run.join(format!(
            "startup-{}-{}.result",
            instance.started_at, instance.pid
        ));
        for (start, keep) in [
            (
                aproxy_ipc::ledger::start_marker(instance.pid).unwrap(),
                true,
            ),
            ("another-run".into(), false),
        ] {
            instance.start = start;
            crate::spawn::publish_startup_failure(
                &run,
                &instance,
                &crate::transform::StartupError {
                    name: "fixture".into(),
                    fault: crate::transform::Fault::Spawn,
                },
            )
            .unwrap();
            collect_garbage_in(&run, &logs);
            assert!(file.exists());
            age(&file, GC_MIN_AGE_SECS + 60);
            collect_garbage_in(&run, &logs);
            assert_eq!(file.exists(), keep);
        }
    }

    #[test]
    fn garbage_collection_follows_the_cleanup_rules() {
        let home = tempfile::tempdir().unwrap();
        let run = home.path().join("run");
        let logs = home.path().join("logs");
        std::fs::create_dir_all(&run).unwrap();
        std::fs::create_dir_all(&logs).unwrap();
        let old = GC_MIN_AGE_SECS + 60;
        let touch = |path: &std::path::Path, secs: u64| {
            std::fs::write(path, "x").unwrap();
            age(path, secs);
        };

        // 实例记录：活着的（本测试进程）留下，写者已不在的删掉
        let me = std::process::id();
        let mut live = sample_info("59811");
        live.pid = me;
        live.start = aproxy_ipc::ledger::start_marker(me).unwrap();
        let live_log = logs.join("1f3a2b-00ab.log");
        live.log_path = Some(live_log.display().to_string().replace('\\', "/"));
        write_instance_file_in(&run, &live).unwrap();
        let mut dead = sample_info("59812");
        dead.pid = u32::MAX - 779;
        dead.start = "w:123".into();
        let dead_log = logs.join("2a2a2a-00cd.log");
        dead.log_path = Some(dead_log.display().to_string());
        write_instance_file_in(&run, &dead).unwrap();
        // 期望状态记录：不归清理管；它引用的日志留着
        let crash_log = logs.join("5c6d7e-11cd.log");
        crate::spec::write_restore_in(
            &run,
            "59813",
            &sample_spec(&crash_log.display().to_string()),
        )
        .unwrap();
        // 交接文件与临时文件：写者不在且够旧才删
        let dead_writer = u32::MAX - 780;
        let stale_spec = run.join(format!("1700000000000000000-{dead_writer}.spec.json"));
        let fresh_spec = run.join(format!("1700000000000000001-{dead_writer}.spec.json"));
        let own_spec = run.join(format!("1700000000000000002-{me}.spec.json"));
        let stale_tmp = run.join(format!(
            "59811.instance.json.tmp-{dead_writer}-1700000000000000003"
        ));
        // 心跳文件：端口上没有活实例且够旧才删
        let dead_heart = run.join("59812.heart");
        let live_heart = run.join("59811.heart");
        touch(&dead_heart, old);
        touch(&live_heart, old);
        touch(&stale_spec, old);
        touch(&fresh_spec, 0);
        touch(&own_spec, old);
        touch(&stale_tmp, old);
        // 日志
        let orphan = logs.join("9900aa-22ef.log");
        let young_orphan = logs.join("9900ab-22ef.log");
        let custom = logs.join("my-instance.log");
        let fixed = logs.join("startup.log");
        for path in [&live_log, &dead_log, &crash_log, &orphan, &custom, &fixed] {
            touch(path, old);
        }
        touch(&young_orphan, 0);
        std::fs::write(logs.join("not-a-log.txt"), "x").unwrap();

        // 有读不懂的记录时：实例记录与残留文件照常清，日志一个都不删
        let unreadable = run.join("7.restore.json");
        std::fs::write(&unreadable, r#"{"schema":9}"#).unwrap();
        let report = collect_garbage_in(&run, &logs);
        assert!(report.logs_paused);
        assert!(
            orphan.exists() && dead_log.exists(),
            "引用集不完整时不得删日志"
        );
        assert!(unreadable.exists(), "读不懂的记录绝不删");
        assert!(
            !instance_file_path_in(&run, "59812").exists(),
            "写者已不在的实例记录应删"
        );
        assert!(
            instance_file_path_in(&run, "59811").exists(),
            "活实例的记录不删"
        );
        assert!(
            !stale_spec.exists() && !stale_tmp.exists(),
            "写者不在的旧残留应删"
        );
        assert!(fresh_spec.exists(), "不够旧的不删");
        assert!(own_spec.exists(), "写者还在的不删");
        assert!(!dead_heart.exists(), "没有活实例的端口留下的旧心跳文件应删");
        assert!(live_heart.exists(), "活实例的心跳文件不删");
        assert!(crate::spec::restore_path_in(&run, "59813").exists());

        std::fs::remove_file(&unreadable).unwrap();
        let report = collect_garbage_in(&run, &logs);
        assert!(!report.logs_paused);
        let mut removed = report.removed.clone();
        removed.sort();
        // 死实例的日志在它的记录被清掉之后不再被引用
        let mut expected = vec![dead_log.clone(), orphan.clone()];
        expected.sort();
        assert_eq!(removed, expected);
        assert!(
            live_log.exists(),
            "活实例引用的日志不删（路径分隔符不同也认得出）"
        );
        assert!(crash_log.exists(), "期望状态记录引用的日志不删");
        assert!(young_orphan.exists(), "不够旧的不删");
        assert!(
            custom.exists() && fixed.exists(),
            "不是随机名形状的日志永远不删"
        );
        assert!(logs.join("not-a-log.txt").exists());
    }

    #[test]
    fn stale_spool_dirs_are_removed_and_live_ones_kept() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("spool");
        let make = |name: &str, files: &[&str]| {
            let dir = root.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            for file in files {
                std::fs::write(dir.join(file), "x").unwrap();
            }
            dir
        };
        let me = std::process::id();
        let marker = aproxy_ipc::ledger::start_marker(me);
        let own_name = spool_dir_name(me, marker.as_deref());
        // 活着的进程自己的目录
        let own = make(&own_name, &["req-1-1.spooltmp"]);
        // pid 上已经没有进程
        let dead = make(
            &format!("{}-0a0b0c0d", u32::MAX - 781),
            &["spool-1-1.spooltmp"],
        );
        // 里面有不认识的文件：临时文件删掉，目录与那个文件留着
        let mixed = make(
            &format!("{}-0a0b0c0d", u32::MAX - 782),
            &["res-1-1.spooltmp", "notes.txt"],
        );
        // 名字不是这个形状的不碰（旧版本按端口命名的目录也在其中）
        let by_port = make("59820", &["spool-9-9.spooltmp"]);
        let odd = make(&format!("{me}-XYZ"), &["a.spooltmp"]);

        let mut expected = vec![dead.clone()];
        // pid 还在而启动标记对不上：只有读得到标记的平台才判得出
        let reused = marker.as_ref().map(|_| {
            let wrong = if own_name.ends_with("00000000") {
                "11111111"
            } else {
                "00000000"
            };
            make(&format!("{me}-{wrong}"), &["req-2-2.spooltmp"])
        });
        expected.extend(reused.clone());
        expected.sort();

        let mut removed = collect_stale_spool_in(&root);
        removed.sort();
        assert_eq!(removed, expected);
        assert!(own.join("req-1-1.spooltmp").exists(), "活进程的目录不碰");
        assert!(!dead.exists());
        assert!(mixed.join("notes.txt").exists() && !mixed.join("res-1-1.spooltmp").exists());
        assert!(by_port.join("spool-9-9.spooltmp").exists());
        assert!(odd.join("a.spooltmp").exists());
        // 根目录不存在时什么也不做
        assert!(collect_stale_spool_in(&home.path().join("absent")).is_empty());
    }

    #[test]
    fn spool_dir_name_is_stable_and_tied_to_the_start_marker() {
        let a = spool_dir_name(4242, Some("133801234567890000"));
        assert_eq!(a, spool_dir_name(4242, Some("133801234567890000")));
        assert_ne!(a, spool_dir_name(4242, Some("133801234567890001")));
        assert_eq!(spool_dir_owner(&a), Some(4242));
        assert_eq!(spool_dir_owner(&spool_dir_name(7, None)), Some(7));
        for name in ["59820", "7-abc", "7-ABCDEF01", "x-0a0b0c0d", "7-0a0b0c0d0"] {
            assert_eq!(spool_dir_owner(name), None, "{name}");
        }
    }

    #[test]
    fn leftover_and_log_names_are_recognized_strictly() {
        assert_eq!(leftover_writer_pid("1700-4242.spec.json"), Some(4242));
        assert_eq!(leftover_writer_pid("startup-82000-42.result"), Some(42));
        assert_eq!(leftover_writer_pid("startup-not-a-time-42.result"), None);
        assert_eq!(
            leftover_writer_pid("12345.instance.json.tmp-4242-1700"),
            Some(4242)
        );
        assert_eq!(leftover_writer_pid("settings.json.tmp-7-9"), Some(7));
        for name in [
            "12345.instance.json",
            "12345.restore.json",
            "x.tmp-abc-1",
            "x.tmp-1-abc",
            "watchdog.json",
        ] {
            assert_eq!(leftover_writer_pid(name), None, "{name}");
        }
        for name in ["19ac3f2e8b5d-1a2b.log", "0-0000.log"] {
            assert!(is_random_log_name(name), "{name}");
        }
        for name in [
            "startup.log",
            "watchdog.log",
            "my-instance.log",
            "19AC-1A2B.log",
            "19ac-1a2.log",
            "19ac3f2e8b5d19ac3-1a2b.log",
            "19ac-1a2b.txt",
        ] {
            assert!(!is_random_log_name(name), "{name}");
        }
    }
}
