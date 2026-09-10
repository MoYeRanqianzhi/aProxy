//! 看门狗（G2）核心原语：claim 选举、进程探活、共享内存心跳。
//!
//! 架构（见 .agents/plan/watchdog-v1.md）：
//! - **全局单看护进程**（`aproxy watchdog`，同二进制分离进程）看护全部实例，
//!   内存 ~2-3MB 与实例数无关；等待全部下沉内核（线程池等进程句柄 +
//!   周期扫描共享内存心跳表），自身零轮询线程。
//! - **选举规范**：排序定发起者（发现看护者缺席时，存活实例中 PID 最小者
//!   才有权 spawn）+ 原子 claim 定在任者（`CREATE_NEW` 语义，并发 spawn
//!   收敛到恰好一个在任看护者）。
//! - **在任判定四条件**（任何进程独立判定，结果必然一致）：
//!   claim 文件存在 && PID 探活 && 进程创建时间匹配（防 PID 复用冒名）
//!   && 心跳新鲜（兼测假死）。
//!
//! 模块只放纯逻辑与可注入测试的原语；`aproxy watchdog` 入口在
//! commands/watchdog.rs。

use crate::settings;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;

// ---------------------------------------------------------------------------
// claim 文件：看护者在任的真相源
// ---------------------------------------------------------------------------

/// 看护者身份凭据（run/watchdog.claim，JSON 一行）。
/// 进程创建时间是防 PID 复用的关键：PID 被无关进程（甚至另一个 aProxy 实例）
/// 复用后，创建时间必然不同——探活通过但时间不匹配 = 冒名，判无效。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct WatchdogClaim {
    pub pid: u32,
    /// 看护进程的创建时刻（100ns 单位的 Windows FILETIME；unix 为
    /// 进程启动的 Unix 秒）。跨平台语义统一为「进程启动时间戳」，
    /// 比较只在同平台进行。
    pub created_at_process: u64,
    /// 看护者最近一次心跳续写时刻（Unix 秒）。看护者每周期续写；
    /// 超过 3×心跳周期未更新 = 看护者假死。
    pub heartbeat_secs: u64,
}

/// claim 文件路径（run_dir 注入，测试绝不读写真实 run/）
pub fn claim_path_in(run_dir: &Path) -> PathBuf {
    run_dir.join("watchdog.claim")
}

/// 读 claim 文件（不存在/损坏 → None）
pub fn read_claim_in(run_dir: &Path) -> Option<WatchdogClaim> {
    let content = std::fs::read_to_string(claim_path_in(run_dir)).ok()?;
    serde_json::from_str(&content).ok()
}

/// 原子接管 claim：仅当成功创建新文件（或安全替换已判无效的旧文件）时返回
/// Some——这是「并发 spawn 收敛到恰好一个在任看护者」的落点。
///
/// Windows 无 create_new 可覆盖已存在文件的原子原语，策略分两步：
/// 1. 旧 claim 不存在 → `create_new` 原子创建（两个竞争者至多一个成功）；
/// 2. 旧 claim 存在 → 先验证无效（调用方负责），删除后 create_new。
///    删旧建新之间仍有理论竞态窗，由创建后二次校验补齐：写入者回头读自己的
///    claim 与进程身份比对，不符即让位退出。
pub fn acquire_claim_in(run_dir: &Path, claim: &WatchdogClaim) -> Option<WatchdogClaim> {
    std::fs::create_dir_all(run_dir).ok()?;
    let path = claim_path_in(run_dir);
    let json = serde_json::to_string(claim).ok()?;
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(mut f) => {
            use std::io::Write;
            f.write_all(json.as_bytes()).ok()?;
            f.write_all(b"\n").ok()?;
            Some(claim.clone())
        }
        // 已有 claim：调用方判定无效后先 remove_claim_in 再重试一次；
        // 仍失败 = 竞争者抢先，让位
        Err(_) => None,
    }
}

/// 删除 claim（看护者优雅退出/清理残留时调用）
pub fn remove_claim_in(run_dir: &Path) {
    let _ = std::fs::remove_file(claim_path_in(run_dir));
}

/// 在任判定：claim 存在且其身份在当前系统上验证通过且心跳新鲜。
/// `heartbeat_fresh_secs` = 3×心跳周期（新鲜阈值）。
/// `now_secs` 注入（测试可拨时钟）；进程身份验证不可注入（真实系统调用），
/// 测试用 `verify_claim_identity_in` 的注入版本。
pub fn claim_is_in_effect_in(run_dir: &Path, heartbeat_fresh_secs: u64, now_secs: u64) -> bool {
    let Some(claim) = read_claim_in(run_dir) else {
        return false;
    };
    // 心跳新鲜性：假死的看护者不再续写——即使进程还活着也判不在任
    now_secs.saturating_sub(claim.heartbeat_secs) <= heartbeat_fresh_secs
        && verify_claim_identity_in(run_dir, &claim)
}

/// claim 身份验证（真实系统调用版）：PID 探活 + 进程创建时间匹配。
pub fn verify_claim_identity_in(_run_dir: &Path, claim: &WatchdogClaim) -> bool {
    verify_claim_identity(claim.pid, claim.created_at_process)
}

// ---------------------------------------------------------------------------
// 进程探活与身份（平台层）
// ---------------------------------------------------------------------------

/// 进程是否存活且创建时间与 `created_at` 匹配。
/// Windows：`created_at` 为 FILETIME（100ns）；
/// unix：为 /proc/<pid>/stat 的 starttime（时钟滴答）。
/// 探活失败（进程不存在）与身份不符（PID 复用）统一返回 false——
/// 对「在任判定」两者等价：都不是我们的看护者。
pub fn verify_claim_identity(pid: u32, created_at: u64) -> bool {
    imp_process::process_alive_with_start(pid, created_at).unwrap_or(false)
}

/// 查询进程创建时间（FILETIME/滴答）；进程不存在返回 None。
/// 供 spawn 前登记与收养时建立身份基线。
pub fn process_start_time(pid: u32) -> Option<u64> {
    imp_process::process_start_time(pid)
}

/// 该 PID 是否为 aProxy 进程（镜像名验证）。
/// 一切「主动杀」动作（--force、看门狗挂死终止）前的防误杀关卡。
pub fn is_aproxy_process(pid: u32) -> bool {
    imp_process::is_aproxy_process(pid)
}

// ---------------------------------------------------------------------------
// 看护者拉起：排序定发起者
// ---------------------------------------------------------------------------

/// 选举：本进程是否有权发起 spawn 看护者。
/// 规范：枚举 run/ 注册表探活得存活实例集合，本进程 PID 是集合最小者
/// 才有权（N 个守护同时发现缺席时收敛到 1 个发起者）。无存活实例
/// （或本进程不在注册表）时视为有权——孤守护（注册表丢失）也要能自保。
pub fn this_process_may_spawn_watchdog_in(run_dir: &Path) -> bool {
    let my_pid = std::process::id();
    let mut min_alive: Option<u32> = None;
    for pid in crate::daemon::registry_pids_in(run_dir) {
        // 只把「真正存活的 aProxy 实例」计入集合（PID 复用防冒名同款逻辑）
        if imp_process::is_aproxy_process(pid) && min_alive.is_none_or(|m| pid < m) {
            min_alive = Some(pid);
        }
    }
    match min_alive {
        Some(m) => my_pid <= m,
        None => true,
    }
}

/// 看护者 spawn 参数：从当前进程命令行还原（与 restore 同思路），
/// 加上 --daemon-watchdog 标记。
pub fn watchdog_spawn_args() -> Vec<String> {
    let mut v: Vec<String> = std::env::args().skip(1).collect();
    v.retain(|a| a != "--daemon-child" && a != "--foreground");
    v.push("--daemon-watchdog".to_string());
    v
}

/// 看护者的心跳续写间隔（读取 settings，可注入覆盖供测试）。
pub fn heartbeat_period(settings: &settings::Settings) -> Duration {
    Duration::from_secs(settings.watchdog_heartbeat_secs.max(1))
}

/// 心跳新鲜阈值：3×周期（连续 3 个周期没续写 = 假死/死亡）。
pub fn heartbeat_fresh_secs(settings: &settings::Settings) -> u64 {
    settings.watchdog_heartbeat_secs.max(1) * 3
}

// ---------------------------------------------------------------------------
// 看护者主循环状态：全部步进函数可测（serve() 只是 tick 循环壳）
// ---------------------------------------------------------------------------

/// 看护者配置（测试注入友好——settings 只在 CLI 层读取一次转成本结构）
#[derive(Debug, Clone)]
pub struct WatchdogConfig {
    /// 实例注册表/run 目录（测试注入 tempdir，绝不触碰真实 run/）
    pub run_dir: PathBuf,
    /// 心跳扫描周期（秒）：每此间隔执行一次 adopt/health/claim 续写
    pub scan_secs: u64,
    /// 挂死容忍周期数：连续 N 轮心跳过期 + ping 失败才判挂死
    pub stale_after_cycles: u64,
    /// 同一实例连续重拉失败上限（crashloop 防护）
    pub max_restarts: u32,
    /// 全部实例清零后的闲置自灭等待（秒）；0 = 永不自灭
    pub idle_exit_secs: u64,
}

impl WatchdogConfig {
    /// 从 settings 组装（生产路径）。
    /// `APROXY_WATCHDOG_SCAN_SECS` 可覆盖扫描周期——集成测试以秒级周期驱动
    /// 真实看护进程；也可作为高级用户临时调频入口（正式调优走 settings）。
    pub fn from_settings() -> Self {
        let s = settings::load();
        let scan_secs = std::env::var("APROXY_WATCHDOG_SCAN_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|v| *v > 0)
            .unwrap_or_else(|| s.watchdog_heartbeat_secs.max(1));
        Self {
            run_dir: crate::daemon::run_dir(),
            scan_secs,
            stale_after_cycles: s.watchdog_stale_after_cycles.max(1),
            max_restarts: s.watchdog_max_restarts,
            idle_exit_secs: s.watchdog_idle_exit_secs,
        }
    }
}

/// 被看护实例的跟踪状态
struct Watched {
    port: String,
    pid: u32,
    /// 进程句柄（SYNCHRONIZE|TERMINATE）：挂死杀进程与死亡等待都用它——
    /// 句柄绑定原进程对象，PID 复用不影响其语义。unix 无句柄对象恒为 0
    /// （死亡检测退化为轮询，未实测分支）。0 = 暂无句柄（open 失败，
    /// adopt_scan 会补挂），绝不能复用已 close 的旧值——close 后的句柄号
    /// 可能被内核分配给无关对象，复用即误杀。
    handle: isize,
    /// 本实例连续重拉失败次数（crashloop 防护计数，重拉成功清零）
    consecutive_failures: u32,
}

/// 待重试的崩溃实例（重拉失败后的退避队列条目）。
/// 实例从 watched 摘除后进队列——失败期间不存在可等待的进程句柄，
/// 死亡事件不会再来，「下轮重试」只能由时间驱动。
#[derive(Debug)]
struct PendingRetry {
    port: String,
    /// 连续重拉失败次数（crashloop 计数挂在队列条目上，跨收养周期累计）
    failures: u32,
    /// 下次重试时刻（退避序列 1s→2s→4s…封顶 300s）
    next_retry: std::time::Instant,
}

/// 一个步进周期的完整看护状态
pub struct WatchdogState {
    pub cfg: WatchdogConfig,
    /// 被收养的实例（端口 → 状态）
    watched: Vec<Watched>,
    /// 重拉失败的退避队列：实例崩溃 → 立即重拉失败 → 摘除进队列，到期由
    /// tick 重试。等待发生在 tick 之间（主循环按 next_pending_deadline 竞速
    /// 提前唤醒），绝不阻塞 tick 本身——否则一次 300s 的退避 sleep 就能让
    /// claim 心跳停摆超过 90s 新鲜窗口，现任被「假死夺权」误杀（M2 实审）。
    pending: Vec<PendingRetry>,
    /// 死亡事件接收端（等待任务 → 主循环）
    deaths: tokio::sync::mpsc::UnboundedReceiver<(String, u32)>,
    #[allow(dead_code)] // 发送端由等待任务持有（字段仅生命周期管理需要）
    death_tx: tokio::sync::mpsc::UnboundedSender<(String, u32)>,
    /// 全部实例清零的起始时刻（None = 非空）；闲置自灭计时
    empty_since: Option<std::time::Instant>,
}

impl WatchdogState {
    pub fn new(cfg: WatchdogConfig) -> Self {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        Self {
            cfg,
            watched: Vec::new(),
            pending: Vec::new(),
            deaths: rx,
            death_tx: tx,
            empty_since: None,
        }
    }

    /// 当前在册（收养中）实例端口
    pub fn watched_ports(&self) -> Vec<String> {
        self.watched.iter().map(|w| w.port.clone()).collect()
    }

    /// 收养扫描：注册表 diff——新出现的 aProxy 实例开句柄纳入看护；
    /// 死亡事件里已摘除的不在此重复纳入（除非注册表又有它的记录 = 手工重启）。
    /// 返回本次新收养的端口（日志/测试断言用）。
    pub async fn adopt_scan(&mut self) -> Vec<String> {
        let mut adopted = Vec::new();
        for entry in crate::daemon::list_restore_entries_in(&self.cfg.run_dir) {
            // 已在看护的端口：只处理句柄缺失的条目（respawn 成功但 open 句柄
            // 失败的兜底——原注释「下轮补挂」曾因端口去重永远走不到，H1）。
            // 句柄有效则跳过（死亡 watcher 已挂，重复挂会双发死亡事件）。
            if let Some(w) = self.watched.iter().find(|w| w.port == entry.port) {
                if w.handle == 0
                    && let Some(h) = imp::open_sync_handle(w.pid)
                {
                    // 借用拆分：iter 的借用已结束，直接改字段 + spawn
                    let (port, pid) = (w.port.clone(), w.pid);
                    let idx = self.watched.iter().position(|w| w.port == port).unwrap();
                    self.watched[idx].handle = h;
                    self.spawn_death_watcher(port, pid, h);
                }
                continue;
            }
            // 实例身份建立：注册 PID 必须是活着的 aProxy 进程（PID 复用防冒名）。
            // 不通过（刚死/被复用）→ 跳过，下轮再看（restore 记录仍在，等
            // respawn 路径或真实实例出现）
            let Some(info) = read_registry_info(&self.cfg.run_dir, &entry.port) else {
                continue;
            };
            if !imp_process::is_aproxy_process(info.pid) {
                continue;
            }
            let Some(handle) = imp::open_sync_handle(info.pid) else {
                continue;
            };
            tracing::info!(port = %entry.port, pid = info.pid, "看护者收养实例");
            self.watched.push(Watched {
                port: entry.port.clone(),
                pid: info.pid,
                handle,
                consecutive_failures: 0,
            });
            self.spawn_death_watcher(entry.port.clone(), info.pid, handle);
            adopted.push(entry.port);
        }
        adopted
    }

    /// 为实例挂等待任务：进程死亡（句柄 signaled）即发死亡事件。
    /// 阻塞在内核 WaitForSingleObject——无轮询，无 CPU 占用。
    fn spawn_death_watcher(&self, port: String, pid: u32, handle: isize) {
        let tx = self.death_tx.clone();
        tokio::task::spawn_blocking(move || {
            imp::wait_blocking(handle);
            let _ = tx.send((port, pid));
        });
    }

    /// 健康扫描：心跳过期者走 IPC ping 二意见，都失败判挂死 → 杀 → 死亡事件
    /// 走统一 respawn 路径。杀的边界按平台：Windows 句柄绑定原进程，无 PID
    /// 复用风险；unix 是裸 SIGKILL(pid)，处决前以 is_aproxy_process 做防误杀
    /// 关卡（见下）——句柄语义差异由该关卡收敛到等效安全性。
    /// 返回被判挂死的端口（测试断言用）。
    pub async fn health_scan(&mut self) -> Vec<String> {
        let stale_ms = self.cfg.scan_secs * 1000 * (self.cfg.stale_after_cycles.max(1) + 1);
        let now_ms = now_millis();
        let mut hung = Vec::new();
        for w in &self.watched {
            match read_heartbeat(&w.port) {
                // 无心跳数据（旧版本守护/创建失败）：退化为纯死亡检测
                None => continue,
                Some(ts) if now_ms.saturating_sub(ts) <= stale_ms => continue,
                Some(_) => {}
            }
            // 心跳过期：IPC ping 二意见（3s 超时 ×1——已在 3× 周期容忍之后，
            // ping 内部还有 3 次重试判死语义）
            if crate::daemon::ipc_ping(&w.port).await.is_ok() {
                continue; // runtime 活着（可能调度延迟），下轮再看
            }
            // 防误杀关卡：「主动杀」前的既定验证。pid 若被复用给无关进程，
            // 心跳/IPC 失效的表现与「实例挂死」不可区分，但那个进程是无辜的。
            // unix 的裸 SIGKILL 尤其依赖此关（Windows 侧为冗余的第二道验证）。
            // 候选已死（zombie/reaped）时关卡判 false → 跳过处决，死亡事件由
            // watcher 兜底
            if !imp_process::is_aproxy_process(w.pid) {
                tracing::warn!(port = %w.port, pid = w.pid, "挂死候选的 pid 已非 aProxy 进程（疑似复用），跳过处决");
                continue;
            }
            tracing::error!(port = %w.port, pid = w.pid, "实例心跳过期且 IPC 无响应，判定挂死，终止进程");
            hung.push(w.port.clone());
            imp::terminate_handle(w.handle);
        }
        hung
    }

    /// 处理死亡事件：.restore 缺失 = 优雅退出 → 摘除看护；.restore 在 = 崩溃
    /// → 立即重拉（注册表 .pid 是否在不是判据——其他实例 respawn 的验活清理
    /// 会顺带删掉本实例的死注册记录，混合态若据此判「优雅退出」会让崩溃实例
    /// 静默失去自动恢复，P6 死亡风暴实测）。优雅退出会删 .restore（守护退出
    /// 路径先 .restore 后 .pid，两 unlink 间死亡事件最多多拉一次，方向安全）。
    /// 重拉失败进退避队列（不 sleep——阻塞主循环会让 claim 心跳停摆、被竞争者
    /// 按「假死」夺权，M2），由 tick 的时间驱动重试。
    /// 返回处置结果（测试断言用）。
    pub async fn handle_death(&mut self, port: &str) -> DeathOutcome {
        let Some(idx) = self.watched.iter().position(|w| w.port == port) else {
            return DeathOutcome::Unknown;
        };
        let w = self.watched.swap_remove(idx);
        imp::close_handle(w.handle);
        // 看护关系终止即清理实例的 IPC 端点与心跳残留：优雅退出路径守护会
        // 自清，崩溃/强杀路径靠这里兜底（Windows 管道/节对象由内核回收，
        // 两个清理在 Windows 上均为 no-op；unix 的 .sock 靠 respawn 前
        // remove_file 自愈，但清掉更干净，/dev/shm 心跳文件则无人自愈）
        remove_heartbeat_file(port);
        crate::daemon::remove_socket_file(port);

        // 安装态差异化（宣告有效时）：死亡事件可能是 install 滚动重启的
        // 预期内 stop（stop→新 exe spawn→IPC 就绪通常 2-3s、上限 10s）——
        // 复查 5×3s 等实例以新 pid 回归，回归即重新收养（新 pid 挂死亡
        // watcher，不动作）；复查耗尽仍死 → 落回常规路径照常重拉（短窗口
        // 不拖长——真崩溃不悬置）。与 install 的竞争收敛：看护者重拉成功
        // = install 的就绪判定（IPC ping）直接通过，两者收敛到同一目标态
        // （新版本实例就绪），worst case 双 spawn 端口冲突后者退出，无死锁。
        if self.install_announcement_active()
            && let Some(new_pid) = self.wait_for_install_restart(port, w.pid).await
        {
            self.admit_respawned(port, new_pid);
            return DeathOutcome::Reclaimed(new_pid);
        }

        let restore_path = crate::daemon::restore_file_path_in(&self.cfg.run_dir, port);
        if !restore_path.is_file() {
            tracing::info!(port = %port, "实例已优雅退出（无恢复记录），摘除看护");
            self.after_watch_removal();
            return DeathOutcome::GracefulExit;
        }

        // 首次崩溃（failures=0）退避为 0 → 立即重拉；失败后进队列按指数退避
        let failures = w.consecutive_failures;
        if failures >= self.cfg.max_restarts {
            return self.give_up(port, failures);
        }
        let delay = backoff_delay_secs(failures);
        if delay > 0 {
            tracing::warn!(port = %port, delay_secs = delay, "实例崩溃，进入退避队列");
            self.pending.push(PendingRetry {
                port: port.to_string(),
                failures,
                next_retry: std::time::Instant::now() + Duration::from_secs(delay),
            });
            self.after_watch_removal();
            return DeathOutcome::RespawnFailed;
        }

        tracing::warn!(port = %port, "实例崩溃，立即重拉");
        match respawn_instance(&self.cfg.run_dir, port).await {
            Ok(pid) => {
                self.admit_respawned(port, pid);
                DeathOutcome::Respawned(pid)
            }
            Err(e) => self.queue_retry(port, failures, e),
        }
    }

    /// 处理退避队列：到期的条目逐个重拉。成功 → 重新收养；失败 → 计数+1
    /// 重新排期，达上限放弃。
    pub async fn process_pending_retries(&mut self) {
        // 收集到期条目后逐个处理（处理中可能再 push，用索引推进避免乱序）
        let now = std::time::Instant::now();
        let due: Vec<usize> = self
            .pending
            .iter()
            .enumerate()
            .filter(|(_, p)| p.next_retry <= now)
            .map(|(i, _)| i)
            .collect();
        // 从大到小摘除，保证索引在多次 remove 中保持有效
        for i in due.into_iter().rev() {
            let p = self.pending.remove(i);
            tracing::info!(port = %p.port, attempt = p.failures + 1, "退避到期，重试重拉");
            match respawn_instance(&self.cfg.run_dir, &p.port).await {
                Ok(pid) => self.admit_respawned(&p.port, pid),
                Err(e) => {
                    self.queue_retry(&p.port, p.failures, e);
                }
            }
        }
    }

    /// 退避队列中最早的重试时刻（None = 队列空）。主循环据此提前唤醒 tick，
    /// 让重试时刻精确到秒而非受扫描周期（默认 30s）拖累。
    pub fn next_pending_deadline(&self) -> Option<std::time::Instant> {
        self.pending.iter().map(|p| p.next_retry).min()
    }

    /// 重拉成功后的重新收养：开句柄挂死亡 watcher；开不出则 handle=0 留给
    /// adopt_scan 补挂（见 adopt_scan 开头分支）。
    fn admit_respawned(&mut self, port: &str, pid: u32) {
        tracing::info!(port = %port, new_pid = pid, "实例已重拉并就绪");
        let handle = imp::open_sync_handle(pid).unwrap_or(0);
        self.watched.push(Watched {
            port: port.to_string(),
            pid,
            handle,
            consecutive_failures: 0,
        });
        if handle != 0 {
            self.spawn_death_watcher(port.to_string(), pid, handle);
        }
        self.empty_since = None;
    }

    /// 重拉失败：计数+1，达上限放弃，否则入队按指数退避。
    /// 崩溃实例从 watched 消失（死亡 watcher 已消费，句柄已关——留着的都是
    /// 悬空条目，曾导致一次失败后永久失去看护，H1）。
    fn queue_retry(&mut self, port: &str, failures: u32, err: String) -> DeathOutcome {
        let failures = failures + 1;
        tracing::error!(port = %port, error = %err, attempt = failures, "重拉失败");
        if failures >= self.cfg.max_restarts {
            return self.give_up(port, failures);
        }
        let delay = backoff_delay_secs(failures);
        self.pending.push(PendingRetry {
            port: port.to_string(),
            failures,
            next_retry: std::time::Instant::now() + Duration::from_secs(delay),
        });
        self.after_watch_removal();
        DeathOutcome::RespawnFailed
    }

    /// crashloop 放弃：写 startup.log 大声报出，保留 .restore 人工兜底。
    fn give_up(&mut self, port: &str, failures: u32) -> DeathOutcome {
        tracing::error!(
            port = %port,
            attempts = failures,
            "实例连续重拉失败达上限，放弃自动重拉（.restore 已保留，可 aproxy restore 手工恢复）"
        );
        crate::daemon::append_startup_log(&format!(
            "[watchdog] 端口 {port} 的实例连续重拉 {failures} 次失败，已放弃自动重拉；可执行 aproxy restore 手工恢复"
        ));
        self.after_watch_removal();
        DeathOutcome::GaveUp
    }

    /// 一个完整步进周期：claim 续写 + 收养 + 健康 + 挂死事件处理 + 退避重试
    /// + 闲置自灭判定 + install 保活
    pub async fn tick(&mut self) {
        // 消化本周期内累积的死亡事件
        let mut deaths = Vec::new();
        while let Ok((port, _pid)) = self.deaths.try_recv() {
            deaths.push(port);
        }
        for port in deaths {
            self.handle_death(&port).await;
        }
        self.process_pending_retries().await;
        self.adopt_scan().await;
        self.health_scan().await;
        self.refresh_claim();
        self.maybe_idle_exit().await;
        self.check_install_keepalive();
    }

    /// claim 续写：心跳时间戳刷新（其他进程据此刻定本看护者是否在任/假死）。
    /// 覆写前先校验归属——claim 已易主（被接管者夺权）还继续双写，两个看护者
    /// 会同时 respawn 同一实例（P7 实测 claim 内容在两 pid 间翻转的根源）。
    /// 正常路径接管者已把前任杀掉（terminate_verified），这里只是杀失败/
    /// 竞态窗口的确定性收尾：让位退出，claim 文件留给新任（勿动，动了会被
    /// 新任按「心跳停滞」再次夺权）。
    fn refresh_claim(&mut self) {
        if let Some(existing) = read_claim_in(&self.cfg.run_dir)
            && existing.pid != std::process::id()
        {
            tracing::warn!(
                claim_pid = existing.pid,
                "claim 已被其他看护者接管，本进程让位退出"
            );
            std::process::exit(0);
        }
        let claim = WatchdogClaim {
            pid: std::process::id(),
            created_at_process: process_start_time(std::process::id()).unwrap_or(0),
            heartbeat_secs: now_secs(),
        };
        let path = claim_path_in(&self.cfg.run_dir);
        if let Ok(json) = serde_json::to_string(&claim)
            && let Err(e) = std::fs::write(&path, json + "\n")
        {
            tracing::warn!(error = %e, "claim 心跳续写失败");
        }
    }

    /// 闲置自灭：全部实例清零（收养表空、退避队列空且注册表空）持续
    /// idle_exit_secs 后删除 claim 退出，系统回到零常驻。pending 非空 = 还有
    /// 待重试的崩溃实例（.restore 在，期望恢复）——不算空闲，否则看护者会在
    /// 长退避中自灭、崩溃实例失去重试（历史上 respawn 失败路径曾造成此态）。
    async fn maybe_idle_exit(&mut self) {
        if self.cfg.idle_exit_secs == 0 {
            return;
        }
        let registry_empty = crate::daemon::registry_pids_in(&self.cfg.run_dir).is_empty();
        if self.watched.is_empty() && self.pending.is_empty() && registry_empty {
            let since = *self.empty_since.get_or_insert_with(std::time::Instant::now);
            if since.elapsed().as_secs() >= self.cfg.idle_exit_secs {
                tracing::info!("无任何实例需要看护，看护者退出（闲置自灭）");
                remove_claim_in(&self.cfg.run_dir);
                std::process::exit(0);
            }
        } else {
            self.empty_since = None;
        }
    }

    fn after_watch_removal(&mut self) {
        if self.watched.is_empty() {
            self.empty_since = Some(std::time::Instant::now());
        }
    }

    /// 安装态宣告是否有效（看护者只认易失介质的显式宣告，绝不解读
    /// install.state 残留——分工原则）。无效/无宣告 → false，常态零改变。
    fn install_announcement_active(&self) -> bool {
        crate::install::announce::read()
            .is_some_and(|a| crate::install::announce::is_active(&a, now_millis()))
    }

    /// 安装态死亡复查：等 install 以新 exe 重新拉起实例（注册表记录重新
    /// 出现——优雅退出会删文件，回归 = 记录带着新 pid 重现）且新 pid 通过
    /// 身份判定（防 PID 复用，与 adopt 同关）。5×3s 覆盖 stop+spawn+ready
    /// 通常 2-3s、上限 10s 的窗口。
    async fn wait_for_install_restart(&self, port: &str, old_pid: u32) -> Option<u32> {
        for _ in 0..5 {
            tokio::time::sleep(Duration::from_secs(3)).await;
            if let Some(info) = read_registry_info(&self.cfg.run_dir, port)
                && info.pid != old_pid
                && imp_process::is_aproxy_process(info.pid)
            {
                tracing::info!(port = %port, new_pid = info.pid, "安装态复查：实例已以新 pid 回归，重新收养");
                return Some(info.pid);
            }
        }
        None
    }

    /// install 保活（仅安装态）：宣告节在但心跳过期（install 挂死——
    /// Windows 节随进程死消失，此态即挂死；unix 崩溃残留文件同判定路径）
    /// → 拉起 `install --continue` 续作。常态无宣告 = 零成本跳过。重复拉起
    /// 由 --continue 的接管判定收敛（未 stale 即退出；接管后宣告刷新），
    /// 最坏情况是每周期一个短命进程直到 updated_at 过 stale 阈值。
    fn check_install_keepalive(&self) {
        let Some(ann) = crate::install::announce::read() else {
            return;
        };
        if crate::install::announce::is_active(&ann, now_millis()) {
            return;
        }
        tracing::warn!(
            installer_pid = ann.installer_pid,
            "install 宣告心跳过期（挂死/残留），拉起 install --continue 续作"
        );
        if let Ok(exe) = std::env::current_exe() {
            let _ = crate::daemon::spawn_detached(
                &exe,
                &["install".to_string(), "--continue".to_string()],
            );
        }
    }
}

/// 死亡事件处置结果（handle_death 返回，测试断言用）
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DeathOutcome {
    /// 崩溃并成功重拉（携带新 PID）
    Respawned(u32),
    /// 安装态复查：实例以新 pid 回归（install 滚动重启），重新收养
    Reclaimed(u32),
    /// 重拉失败（退避计数 +1，未达上限）
    RespawnFailed,
    /// 连续失败达上限，放弃自动重拉
    GaveUp,
    /// 优雅退出（无 .restore），正常摘除
    GracefulExit,
    /// 端口不在看护表中（重复事件等）
    Unknown,
}

/// 重拉退避：`consecutive_failures` 为已连续失败次数。首次崩溃（0）立即重拉；
/// 失败 1 次后等 1s、2 次后 2s、3 次后 4s……封顶 300s。
pub fn backoff_delay_secs(consecutive_failures: u32) -> u64 {
    if consecutive_failures == 0 {
        return 0;
    }
    // 移位上限 30 防 u32::MAX 溢出（1<<30 已远超 300s 封顶）
    (1u64 << (consecutive_failures - 1).min(30)).min(300)
}

/// 读注册表文件获取实例信息（收养时的身份基线）
fn read_registry_info(run_dir: &Path, port: &str) -> Option<crate::daemon::InstanceInfo> {
    let path = crate::daemon::instance_file_path_in(run_dir, port);
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

/// 当前 Unix 秒
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 按 .restore 记录重拉实例并等待 IPC 就绪（8s，与 restore/start 一致）。
/// 成功返回新 PID；注册表缺失记录的实例恢复（实例文件由守护 bind 后自写）。
async fn respawn_instance(run_dir: &Path, port: &str) -> Result<u32, String> {
    let entries = crate::daemon::list_restore_entries_in(run_dir);
    let entry = entries
        .iter()
        .find(|e| e.port == port)
        .ok_or_else(|| "恢复记录不存在".to_string())?;
    // 配置文件已被删的记录无法忠实恢复（与 aproxy restore 同语义）
    if let Some(cfg) = entry
        .args
        .iter()
        .position(|a| a == "--config")
        .and_then(|i| entry.args.get(i + 1))
        && !std::path::Path::new(cfg).exists()
    {
        crate::daemon::remove_restore_file_in(run_dir, port);
        return Err(format!("配置文件已不存在: {cfg}"));
    }
    let mut args = entry.args.clone();
    args.push("--daemon-child".to_string());
    let exe = std::env::current_exe().map_err(|e| format!("无法定位自身可执行文件: {e}"))?;
    let pid = crate::daemon::spawn_detached(&exe, &args).map_err(|e| format!("spawn 失败: {e}"))?;
    // 就绪判定按新 pid 在注册表定位（8s，与 start/restore 一致）：重拉用
    // .restore 原参数，但配置可能已被用户改动（换端口）——新守护监听新端口
    // 时 ping 旧端口永远不通（与 restart 同款实测坑）。pid 是 spawn 返回值，
    // 唯一可靠锚点；新守护 bind 成功才写注册表，出现即就绪。
    // 用只读检索而非 list_instances_in：后者的验活清理副作用会在本循环的
    // 200ms 轮询里顺带删掉同注册表其他死实例的记录，其死亡事件随后被混合态
    // 误判为优雅退出、静默失去自动恢复（P6 死亡风暴实测）。
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    loop {
        if crate::daemon::registry_contains_pid_in(run_dir, pid) {
            return Ok(pid);
        }
        if std::time::Instant::now() > deadline {
            return Err("重拉后 IPC 未在预期时间内就绪".to_string());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// 看护者主循环（CLI 入口调用）：接管 claim → tick 循环。
/// claim 被竞争者持有且有效时安静退出（唯一性落点）。
pub async fn serve(cfg: WatchdogConfig) {
    // claim 接管：已有有效 claim = 另一个看护者在任 → 让位退出
    let my_claim = WatchdogClaim {
        pid: std::process::id(),
        created_at_process: process_start_time(std::process::id()).unwrap_or(0),
        heartbeat_secs: now_secs(),
    };
    if acquire_claim_in(&cfg.run_dir, &my_claim).is_none() {
        // 已有 claim：验证其有效性；无效则清理重试（前任死亡/假死/残留）
        if claim_is_in_effect_in(
            &cfg.run_dir,
            heartbeat_fresh_secs(&settings::load()),
            now_secs(),
        ) {
            tracing::info!("已有在任的看护者，本进程退出（选举唯一性）");
            return;
        }
        // 无效 claim：先杀掉「活着但假死」的前任（身份可验证才杀——PID 复用
        // 防冒名的最后一道关），再原子接管
        if let Some(old) = read_claim_in(&cfg.run_dir) {
            let old_alive = imp_process::is_aproxy_process(old.pid)
                && verify_claim_identity(old.pid, old.created_at_process);
            if old_alive {
                tracing::error!(
                    pid = old.pid,
                    "前任看护者仍在但心跳过期（假死），终止后接管"
                );
                imp::terminate_verified(old.pid);
            }
            remove_claim_in(&cfg.run_dir);
        }
        if acquire_claim_in(&cfg.run_dir, &my_claim).is_none() {
            tracing::info!("claim 被竞争者抢先，本进程退出（选举唯一性）");
            return;
        }
    }
    tracing::info!(pid = std::process::id(), "看护者就绪（watchdog）");

    // 启动全量检查：本地状态文件残留 → 拉起对应处理者（恢复机制的主责入口，
    // 用户定调）。检查对象是各类本地状态文件（当前只有 install.state，后续
    // 扩展更多）；职责仅为「发现残留 → spawn」，处于哪一步、续跑还是清理
    // 全部由处理者自己判断——看护者绝不解读状态文件语义（分工原则）。
    // 启动时一次 + 运行中每日一次（防常态浪费）。
    sweep_local_states(&cfg.run_dir);

    let mut state = WatchdogState::new(cfg);
    state.adopt_scan().await;
    let scan = Duration::from_secs(state.cfg.scan_secs.max(1));
    let mut last_sweep = std::time::Instant::now();
    loop {
        // 双源竞速唤醒：常规扫描周期，或退避队列的最早到期时刻——重试时刻
        // 精确到秒而非被扫描周期拖累。主循环单步耗时预算：handle_death/process
        // 的 respawn 至多 8s + health_scan 的 ipc_ping 至多 ~9.6s，远小于 claim
        // 的 90s 新鲜窗口——把退避 sleep 放进 tick 会击穿这个预算（M2），故
        // 等待一律发生在 tick 之间。
        let next_retry = state.next_pending_deadline();
        tokio::select! {
            _ = tokio::time::sleep(scan) => {},
            _ = async {
                match next_retry {
                    Some(d) => tokio::time::sleep_until(tokio::time::Instant::from_std(d)).await,
                    None => std::future::pending::<()>().await,
                }
            } => {},
        }
        state.tick().await;
        // 每日一次的残留状态文件复查（安装中断后看护者长期存活的场景）
        if last_sweep.elapsed() >= Duration::from_secs(24 * 3600) {
            last_sweep = std::time::Instant::now();
            sweep_local_states(&state.cfg.run_dir);
        }
    }
}

/// 本地状态文件残留全量检查（当前只有 install.state）：存在即拉起
/// `install --continue`（detached，短命进程——已正常结束的场景由续作进程
/// 自行清文件退出，幂等收敛）。
fn sweep_local_states(run_dir: &Path) {
    if crate::install::state::load_in(run_dir).is_some() {
        tracing::info!("发现 install 状态文件残留，拉起 install --continue 续作");
        if let Ok(exe) = std::env::current_exe() {
            let _ = crate::daemon::spawn_detached(
                &exe,
                &["install".to_string(), "--continue".to_string()],
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 共享内存心跳：守护每周期写毫秒时间戳，看护者扫描判定挂死
// ---------------------------------------------------------------------------
//
// 设计要点：
// - 节名 `aproxy-heart-<端口>`（与 IPC 管道命名同款规则，端口唯一区分实例）。
// - 8 字节 = 毫秒级 Unix 时间戳（Windows FILETIME 换算），原子 u64 读写。
// - **守护侧由独立 ticker 任务每 10s 写一次，不经请求热路径**：挂死的定义是
//   「tokio runtime 无法调度」，ticker 停摆与 runtime 死锁等价；零热路径成本
//   是它优于请求内 store 的地方（计划偏差：热路径 store 改 ticker，W7 记录）。
// - 心跳新鲜阈值（3×看护扫描周期）远大于 10s 写入间隔，时钟毛刺无影响。

/// 守护侧心跳写入间隔（秒）。远小于看护扫描周期（默认 30s），
/// 保证任何一次扫描都能读到新鲜值。
pub const HEARTBEAT_WRITE_INTERVAL_SECS: u64 = 10;

/// 共享内存节的命名（与 IPC 端点同款端口规则）
pub fn heartbeat_section_name(port: &str) -> String {
    #[cfg(windows)]
    {
        format!(r"Local\aproxy-heart-{port}")
    }
    #[cfg(unix)]
    {
        format!("aproxy-heart-{port}")
    }
}

/// 守护侧心跳句柄：持有映射视图，Drop 时 UnmapViewOfFile（句柄随进程退出
/// 由系统回收，测试中显式 Drop 防泄漏）。
pub struct HeartbeatWriter {
    /// 节句柄：映射期间必须保持打开——关闭节句柄后 OpenFileMappingW 将找不到
    /// 该节（内核对象仅剩视图弱引用，名字空间注册随之消失）
    #[cfg(windows)]
    mapping: isize,
    #[cfg(windows)]
    view: *mut std::ffi::c_void,
    /// unix 实现按路径写 /dev/shm 文件，beat() 需要知道写入目标
    #[cfg(unix)]
    port: String,
    #[cfg(unix)]
    _shm_fd: std::fs::File,
}

// 视图指针跨线程使用（写操作是原子 store）；Windows 句柄本身线程无关
#[cfg(windows)]
unsafe impl Send for HeartbeatWriter {}
#[cfg(windows)]
unsafe impl Sync for HeartbeatWriter {}

impl HeartbeatWriter {
    /// 创建/打开本实例的心跳节并写入初始时间戳。实例 bind 成功后调用；
    /// 失败只降级（看护者对该实例退化为纯进程死亡检测），绝不能阻断启动。
    pub fn create(port: &str) -> Option<Self> {
        imp_heart::create_heartbeat(port)
    }

    /// 写入当前毫秒时间戳（原子 store，无锁）
    pub fn beat(&self) {
        imp_heart::heartbeat_store(self, now_millis());
    }
}

impl Drop for HeartbeatWriter {
    fn drop(&mut self) {
        imp_heart::unmap_heartbeat(self);
    }
}

/// 看护侧读取：实例最近一次心跳的毫秒时间戳；节不存在 = 实例无心跳
/// （旧版本守护或创建失败）→ None，看护者按「无心跳数据」处理（只做
/// 进程死亡检测，不做挂死判定）。
pub fn read_heartbeat(port: &str) -> Option<u64> {
    imp_heart::heartbeat_load(port)
}

/// 清理实例的心跳文件（守护优雅退出/看护摘除时调用）。
/// Windows 节对象由内核回收（no-op）；unix 删除 /dev/shm 下的文件，
/// 消除崩溃/强杀后的 tmpfs 残留（实测一轮测试曾留 13 个）。
pub fn remove_heartbeat_file(port: &str) {
    imp_heart::remove_heartbeat_file(port)
}

/// 当前 Unix 毫秒（系统时钟早于 epoch 回退 0——仅用于新鲜度比较，
/// 看护侧同时校验非 0）
pub fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// 平台实现
// ---------------------------------------------------------------------------

#[cfg(windows)]
mod imp {
    /// 打开进程的 SYNCHRONIZE|TERMINATE 句柄（死亡等待/挂死终止共用）
    pub fn open_sync_handle(pid: u32) -> Option<isize> {
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
        };
        unsafe {
            let h = OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_TERMINATE, 0, pid);
            if h == 0 { None } else { Some(h) }
        }
    }

    /// 阻塞等待进程死亡（内核对象 signaled）。仅在 spawn_blocking 中调用。
    pub fn wait_blocking(handle: isize) {
        use windows_sys::Win32::System::Threading::WaitForSingleObject;
        unsafe {
            // INFINITE：进程死亡是必然事件（迟早 signaled），无需超时
            let _ = WaitForSingleObject(handle, 0xFFFF_FFFF);
        }
    }

    /// 终止挂死实例（句柄绑定原进程对象，无 PID 复用风险）
    pub fn terminate_handle(handle: isize) {
        use windows_sys::Win32::System::Threading::TerminateProcess;
        unsafe {
            let _ = TerminateProcess(handle, 1);
        }
    }

    /// 关闭句柄（摘除看护/重拉换句柄时）
    pub fn close_handle(handle: isize) {
        if handle != 0 {
            unsafe {
                let _ = windows_sys::Win32::Foundation::CloseHandle(handle);
            }
        }
    }

    /// 杀掉经身份验证的假死前任看护者（claim 记录的 PID + 创建时间已验证，
    /// PID 复用冒名在此被拒绝）。验证失败静默返回——宁可漏杀不误杀。
    pub fn terminate_verified(pid: u32) {
        if !super::imp_process::is_aproxy_process(pid) {
            return;
        }
        if let Some(h) = open_sync_handle(pid) {
            terminate_handle(h);
            close_handle(h);
        }
    }
}

#[cfg(unix)]
mod imp {
    use std::time::Duration;

    unsafe extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }

    /// unix 无可等待的进程句柄对象：句柄值直接存 pid，死亡等待退化为轮询。
    /// 存 pid 而非 0 的意义：0 会被 adopt_scan 视作「句柄缺失」反复补挂。
    pub fn open_sync_handle(pid: u32) -> Option<isize> {
        Some(pid as isize)
    }
    /// 轮询进程死亡。仅在 spawn_blocking 中调用——扫描周期由调用方健康检查
    /// 兜底，此处 1s 粒度足够（Windows 侧为内核事件驱动，unix 无等价原语，
    /// 这是设计内的退化）。
    ///
    /// 不能只用 kill(pid,0)：它对 zombie（已死未收割）也返回成功——守护的
    /// 父进程（start/测试/看护者自己）不 wait 之前死亡事件永远不触发。
    /// 须读 /proc 的进程态排除 Z。无 /proc 的平台（macOS）回退 kill(pid,0)。
    pub fn wait_blocking(handle: isize) {
        let pid = handle as i32;
        loop {
            if !process_alive_for_wait(pid) {
                return;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    }

    /// 死亡等待的存在性探测：/proc 可用（Linux）时 zombie 视为死亡；
    /// 无 /proc（macOS）回退 kill(pid,0)（EPERM 视为存活，其余错误为死亡）。
    /// 已知退化：回退分支对 zombie 失效——kill(pid,0) 对已死未收割的进程
    /// 也返回成功，死亡事件会延迟到 health_scan 兜底。macOS 属未实测平台，
    /// 接受此退化；若未来需要，可改 waitid(WNOHANG) 或进程状态查询。
    fn process_alive_for_wait(pid: i32) -> bool {
        if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            // stat 形如 "pid (comm) S ..."，comm 可含空格/括号，取 ')' 之后首字段
            return stat
                .rsplit_once(')')
                .and_then(|(_, rest)| rest.split_whitespace().next())
                .map(|state| state != "Z")
                .unwrap_or(false);
        }
        let r = unsafe { kill(pid, 0) };
        if r == 0 {
            return true;
        }
        std::io::Error::last_os_error().raw_os_error() == Some(1)
    }

    /// SIGKILL 处决（挂死判定后调用）：unix 直接以句柄中保存的 pid 杀进程。
    /// 上层靠死亡 watcher 的轮询确认死亡。
    pub fn terminate_handle(handle: isize) {
        unsafe {
            kill(handle as i32, 9);
        }
    }
    pub fn close_handle(_handle: isize) {}
    /// 杀掉经身份验证的假死前任看护者：身份由调用方把关（is_aproxy_process
    /// 的 /proc exe 比对 + verify_claim_identity 的 starttime 比对），此处
    /// 只负责 SIGKILL。unix 上无句柄对象，直接按 pid 杀——claim 记录到被
    /// 杀之间的复用窗口由上述双重验证封住。
    pub fn terminate_verified(pid: u32) {
        unsafe {
            kill(pid as i32, 9);
        }
    }
}

#[cfg(windows)]
mod imp_heart {
    /// 创建命名节并映射视图（守护侧）
    pub fn create_heartbeat(port: &str) -> Option<super::HeartbeatWriter> {
        use windows_sys::Win32::System::Memory::{
            CreateFileMappingW, FILE_MAP_WRITE, MapViewOfFile, PAGE_READWRITE,
        };
        let name = super::heartbeat_section_name(port);
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            // 8 字节节：一个 u64 毫秒时间戳。INVALID_HANDLE_VALUE = 由页面
            // 文件支撑的共享节（不落盘、随最后一个句柄关闭消失）
            let mapping = CreateFileMappingW(
                windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE,
                std::ptr::null(),
                PAGE_READWRITE,
                0,
                8,
                wide.as_ptr(),
            );
            if mapping == 0 {
                return None;
            }
            let view = MapViewOfFile(mapping, FILE_MAP_WRITE, 0, 0, 8);
            if view.Value.is_null() {
                let _ = windows_sys::Win32::Foundation::CloseHandle(mapping);
                return None;
            }
            let writer = super::HeartbeatWriter {
                mapping,
                view: view.Value,
            };
            writer.beat();
            Some(writer)
        }
    }

    /// 原子写毫秒时间戳到视图首 8 字节（writer 持有写视图）
    pub fn heartbeat_store(writer: &super::HeartbeatWriter, millis: u64) {
        let ptr = writer.view as *mut std::sync::atomic::AtomicU64;
        unsafe {
            (*ptr).store(millis, std::sync::atomic::Ordering::Release);
        }
    }

    /// 读侧：打开命名节映射只读视图并读首 8 字节
    pub fn heartbeat_load(port: &str) -> Option<u64> {
        use windows_sys::Win32::System::Memory::{FILE_MAP_READ, MapViewOfFile, OpenFileMappingW};
        let name = super::heartbeat_section_name(port);
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            let mapping = OpenFileMappingW(FILE_MAP_READ, 0, wide.as_ptr());
            if mapping == 0 {
                return None;
            }
            let view = MapViewOfFile(mapping, FILE_MAP_READ, 0, 0, 8);
            let _ = windows_sys::Win32::Foundation::CloseHandle(mapping);
            if view.Value.is_null() {
                return None;
            }
            let ptr = view.Value as *const std::sync::atomic::AtomicU64;
            let val = (*ptr).load(std::sync::atomic::Ordering::Acquire);
            let _ = windows_sys::Win32::System::Memory::UnmapViewOfFile(view);
            Some(val)
        }
    }

    pub fn unmap_heartbeat(writer: &super::HeartbeatWriter) {
        // MEMORY_MAPPED_VIEW_ADDRESS 结构体重组（0.52 的 MapViewOfFile 返回形状）
        let view =
            windows_sys::Win32::System::Memory::MEMORY_MAPPED_VIEW_ADDRESS { Value: writer.view };
        unsafe {
            let _ = windows_sys::Win32::System::Memory::UnmapViewOfFile(view);
            let _ = windows_sys::Win32::Foundation::CloseHandle(writer.mapping);
        }
    }

    /// 节对象由内核在最后一个句柄关闭时回收，无文件系统残留可清
    pub fn remove_heartbeat_file(_port: &str) {}
}

#[cfg(unix)]
mod imp_heart {
    pub fn create_heartbeat(port: &str) -> Option<super::HeartbeatWriter> {
        // posix shm：/dev/shm/aproxy-heart-<port>；写透文件实现同一语义
        // （unix 未实测，与 UDS IPC 同批处理）
        let path = format!("/dev/shm/{}", super::heartbeat_section_name(port));
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path)
            .ok()?;
        use std::io::Write;
        f.write_all(&super::now_millis().to_le_bytes()).ok()?;
        Some(super::HeartbeatWriter {
            port: port.to_string(),
            _shm_fd: f,
        })
    }

    pub fn heartbeat_store(writer: &super::HeartbeatWriter, millis: u64) {
        let path = format!("/dev/shm/{}", super::heartbeat_section_name(&writer.port));
        if let Ok(mut f) = std::fs::OpenOptions::new().write(true).open(path) {
            use std::io::{Seek, SeekFrom, Write};
            let _ = f.seek(SeekFrom::Start(0));
            let _ = f.write_all(&millis.to_le_bytes());
        }
    }

    pub fn heartbeat_load(port: &str) -> Option<u64> {
        let path = format!("/dev/shm/{}", super::heartbeat_section_name(port));
        let data = std::fs::read(path).ok()?;
        if data.len() < 8 {
            return None;
        }
        let mut b = [0u8; 8];
        b.copy_from_slice(&data[..8]);
        Some(u64::from_le_bytes(b))
    }

    pub fn unmap_heartbeat(_writer: &super::HeartbeatWriter) {}

    /// 删除心跳文件。Windows 的节对象随进程退出由内核回收，unix 是 /dev/shm
    /// 下的真实文件（tmpfs 内存计费），守护优雅退出/看护摘除时清掉，崩溃残留
    /// 由下次同端口 create 的 truncate 覆盖 + 人工/治理路径兜底
    pub fn remove_heartbeat_file(port: &str) {
        let path = format!("/dev/shm/{}", super::heartbeat_section_name(port));
        // 写侧仍持有打开句柄时 unlink 合法（句柄继续可写直至关闭），
        // 残留仅出现在「创建后未到优雅退出就崩溃」的场景
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(windows)]
mod imp_process {
    /// 进程存活且创建时间匹配（FILETIME 100ns）。
    pub fn process_alive_with_start(pid: u32, created_at: u64) -> Option<bool> {
        let actual = process_start_time(pid)?;
        Some(actual == created_at)
    }

    /// OpenProcess + GetProcessTimes：进程不存在（ERROR_INVALID_PARAMETER/
    /// ACCESS_DENIED 之外的失败一并视为不可知 → None）。
    pub fn process_start_time(pid: u32) -> Option<u64> {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle == 0 {
                return None;
            }
            let mut creation: i64 = 0;
            let mut exit: i64 = 0;
            let mut kernel: i64 = 0;
            let mut user: i64 = 0;
            let ok = GetProcessTimes(
                handle,
                &mut creation as *mut i64 as *mut _,
                &mut exit as *mut i64 as *mut _,
                &mut kernel as *mut i64 as *mut _,
                &mut user as *mut i64 as *mut _,
            );
            let _ = CloseHandle(handle);
            if ok == 0 {
                return None;
            }
            Some(creation as u64)
        }
    }

    /// 是否 aProxy 进程：镜像名比对（看门狗对 PID 复用的第二道防线——
    /// 纯死亡检测不需要它；任何「主动杀」动作前必须过这道验证）。
    pub fn is_aproxy_process(pid: u32) -> bool {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle == 0 {
                return false;
            }
            let mut buf = [0u16; 512];
            let mut len = buf.len() as u32;
            let ok = windows_sys::Win32::System::Threading::QueryFullProcessImageNameW(
                handle,
                0,
                buf.as_mut_ptr(),
                &mut len,
            );
            let _ = CloseHandle(handle);
            if ok == 0 {
                return false;
            }
            let name = String::from_utf16_lossy(&buf[..len as usize]);
            let base = name.rsplit(['\\', '/']).next().unwrap_or("");
            base.eq_ignore_ascii_case("aproxy.exe")
        }
    }
}

#[cfg(unix)]
mod imp_process {
    pub fn process_alive_with_start(pid: u32, created_at: u64) -> Option<bool> {
        let actual = process_start_time(pid)?;
        Some(actual == created_at)
    }

    /// /proc/<pid>/stat 的 starttime（字段 22，时钟滴答）。
    pub fn process_start_time(pid: u32) -> Option<u64> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        // comm 可含空格与括号，取最后一个 ')' 之后切字段
        let after = stat.rsplit(')').next()?;
        let fields: Vec<&str> = after.split_whitespace().collect();
        fields.get(19)?.parse().ok()
    }

    /// 是否 aProxy 进程：/proc/<pid>/exe 指向实际二进制，比对 basename。
    /// 判定分三档：
    /// - readlink 成功：比对 basename（Linux 二进制无 .exe 后缀；二进制被
    ///   原地替换后内核附加「 (deleted)」后缀，剥除后再比——swap 升级场景下
    ///   正在运行的进程不该因此被判定为异己）
    /// - ENOENT：进程已死（含 zombie，exe 随地址空间一并消失）→ false。
    ///   选举不再被注册表死条目卡住、收养不再收编死条目（与 Windows 的
    ///   OpenProcess 失败即拒收对齐）
    /// - 其他失败（跨用户权限等）：不可知 → 保守放行（fail-open）。误放行
    ///   的最坏结果是多一次 spawn 尝试 / 一次有身份验证前置的杀；误拒绝则
    ///   会让存活实例失去看护——两个方向上前者代价小得多
    pub fn is_aproxy_process(pid: u32) -> bool {
        match std::fs::read_link(format!("/proc/{pid}/exe")) {
            Ok(target) => {
                let name = target.file_name().and_then(|n| n.to_str()).unwrap_or("");
                let name = name.strip_suffix(" (deleted)").unwrap_or(name);
                name == "aproxy"
            }
            Err(e) => e.kind() != std::io::ErrorKind::NotFound,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_cfg(dir: &Path) -> WatchdogConfig {
        WatchdogConfig {
            run_dir: dir.to_path_buf(),
            scan_secs: 1,
            stale_after_cycles: 1,
            max_restarts: 2,
            idle_exit_secs: 0, // 测试不自灭（退出会杀测试进程！）
        }
    }

    #[test]
    fn claim_roundtrip_and_missing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_claim_in(dir.path()).is_none(), "无 claim 文件读 None");
        let claim = WatchdogClaim {
            pid: 42,
            created_at_process: 133_000_000_000,
            heartbeat_secs: 1_700_000_000,
        };
        acquire_claim_in(dir.path(), &claim).expect("空目录首次接管应成功");
        let read = read_claim_in(dir.path()).unwrap();
        assert_eq!(read, claim);
        // 已存在：再次接管失败（原子性——并发 spawn 只有一个赢）
        let rival = WatchdogClaim { pid: 99, ..claim };
        assert!(
            acquire_claim_in(dir.path(), &rival).is_none(),
            "已有在任 claim 时接管必须失败"
        );
        // 删除后可重新接管
        remove_claim_in(dir.path());
        assert!(acquire_claim_in(dir.path(), &rival).is_some());
    }

    #[test]
    fn claim_corrupted_file_reads_none() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(claim_path_in(dir.path()), "{corrupted").unwrap();
        assert!(read_claim_in(dir.path()).is_none());
        // 损坏 claim 的接管：OpenOptions create_new 失败 → None（调用方先
        // remove 再重试）；此处验证不会 panic
        let claim = WatchdogClaim {
            pid: 1,
            created_at_process: 1,
            heartbeat_secs: 1,
        };
        assert!(acquire_claim_in(dir.path(), &claim).is_none());
    }

    #[test]
    fn claim_identity_rejects_mismatch_and_dead() {
        // 真实进程身份验证：本进程的创建时间可查且与自身匹配
        let my_pid = std::process::id();
        let my_start = process_start_time(my_pid).expect("本进程创建时间必须可查");
        assert!(
            verify_claim_identity(my_pid, my_start),
            "本进程身份验证必须通过"
        );
        // 篡改创建时间 → 冒名拒绝（PID 复用场景的模拟）
        assert!(
            !verify_claim_identity(my_pid, my_start.wrapping_add(1)),
            "创建时间不匹配必须拒绝"
        );
        // 不存在的 PID（测试区高位端口同思路：选一个几乎不可能的 pid）
        assert!(
            process_start_time(u32::MAX - 7).is_none(),
            "不存在的进程应返回 None"
        );
    }

    #[test]
    fn heartbeat_freshness_math() {
        // 3×周期新鲜阈值：周期 30s → 90s 内算新鲜
        assert_eq!(heartbeat_fresh_secs(&settings::Settings::default()), 90);
        // 周期 0 防御为 1（doctor 拦 0，这里双保险）
        let s = settings::Settings {
            watchdog_heartbeat_secs: 0,
            ..Default::default()
        };
        assert_eq!(heartbeat_period(&s), Duration::from_secs(1));
        assert_eq!(heartbeat_fresh_secs(&s), 3);
    }

    // unix：身份判定对「已死进程」的两种形态都判 false——
    // 大号未用 pid（/proc 条目不存在）与真实 zombie（已 kill 未收割）
    #[cfg(unix)]
    #[test]
    fn is_aproxy_process_rejects_dead_pids() {
        // Linux pid_max 上限 4194304，此 pid 不可能存在 → readlink ENOENT → false
        assert!(!is_aproxy_process(u32::MAX - 1));

        // 真实 zombie：子进程被 SIGKILL 后、父进程收割前，/proc/<pid> 仍在但
        // exe 语义随地址空间消失（readlink ENOENT）→ false
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn sleep 失败（unix 测试环境必备）");
        child.kill().expect("SIGKILL 失败");
        std::thread::sleep(std::time::Duration::from_millis(150));
        assert!(
            !is_aproxy_process(child.id()),
            "zombie 进程不应通过身份判定"
        );
        child.wait().unwrap(); // 收割，避免测试自身留 zombie
    }

    #[test]
    fn heartbeat_section_name_uses_port() {
        // 节名含端口：实例唯一区分（同 IPC 管道命名规则）
        let n = heartbeat_section_name("12345");
        assert!(n.contains("12345"), "{n}");
        assert!(n.contains("aproxy-heart"), "{n}");
    }

    #[test]
    fn heartbeat_write_and_read_same_process() {
        // 同进程内写读往返：节创建 → beat → 读取值非 0 且随时间推进增长。
        // （跨进程读由集成测试覆盖——看护进程场景）
        let port = format!("598{:02}", std::process::id() % 100);
        let writer = HeartbeatWriter::create(&port).expect("心跳节创建");
        let first = read_heartbeat(&port).expect("写入后立即可读");
        assert!(first > 0, "初始时间戳应为正毫秒值");
        std::thread::sleep(std::time::Duration::from_millis(15));
        writer.beat();
        let second = read_heartbeat(&port).expect("二次读取");
        assert!(second >= first, "时间戳应单调不减");
        // 另一个端口没有节：读 None（看护者按「无心跳数据」处理）
        assert!(read_heartbeat(&format!("{port}-absent")).is_none());
    }

    // ---------------- 看护主循环状态机（纯逻辑，tempdir 注入） ----------------

    /// 构造一个伪实例环境：注册表 + 恢复记录齐全（进程身份不真存在——
    /// adopt_scan 的 is_aproxy_process 探活会拒绝它，测试直接操纵 watched）
    fn write_crashed_instance(dir: &Path, port: &str) {
        let info = crate::daemon::InstanceInfo {
            pid: u32::MAX - 777, // 不会存活也不易复用的 PID
            version: "0.0.0-test".into(),
            listen_addr: format!("127.0.0.1:{port}"),
            config_path: "C:/tmp/no-such-config.toml".into(),
            base_url: "https://x".into(),
            started_at: now_secs(),
            last_activity_secs: 0,
            proto_version: 2,
            requests_total: 0,
            retries_total: 0,
            last_error: None,
            last_error_at: 0,
            swap_phase: false,
        };
        crate::daemon::write_instance_file_in(dir, &info).unwrap();
        // restore 记录（崩溃信号）
        let args: Vec<String> = vec![
            "--config".to_string(),
            "C:/tmp/no-such-config.toml".to_string(),
        ];
        crate::daemon::write_restore_file_in(dir, port, &args).unwrap();
    }

    #[tokio::test]
    async fn watchdog_state_death_outcomes() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = test_cfg(dir.path());
        let mut st = WatchdogState::new(cfg);
        assert!(st.watched_ports().is_empty());

        // 优雅退出：无 .restore → GracefulExit（不在看护表也返回 Unknown 先验证）
        assert_eq!(st.handle_death("59901").await, DeathOutcome::Unknown);

        // 把伪实例直接塞进看护表（绕过 adopt 的真实进程探活）
        st.watched.push(Watched {
            port: "59901".into(),
            pid: u32::MAX - 777,
            handle: 0,
            consecutive_failures: 0,
        });
        // 有注册表无 .restore → 优雅退出语义
        let info = crate::daemon::InstanceInfo {
            pid: u32::MAX - 777,
            version: "t".into(),
            listen_addr: "127.0.0.1:59901".into(),
            config_path: "C:/tmp/c.toml".into(),
            base_url: "https://x".into(),
            started_at: now_secs(),
            last_activity_secs: 0,
            proto_version: 2,
            requests_total: 0,
            retries_total: 0,
            last_error: None,
            last_error_at: 0,
            swap_phase: false,
        };
        crate::daemon::write_instance_file_in(dir.path(), &info).unwrap();
        assert_eq!(st.handle_death("59901").await, DeathOutcome::GracefulExit);
        assert!(st.watched_ports().is_empty(), "优雅退出后应摘除看护");
    }

    #[tokio::test]
    async fn watchdog_state_crashloop_gives_up() {
        let dir = tempfile::tempdir().unwrap();
        let port = "59902";
        write_crashed_instance(dir.path(), port);
        let cfg = test_cfg(dir.path());
        let mut st = WatchdogState::new(cfg);
        st.watched.push(Watched {
            port: port.into(),
            pid: u32::MAX - 777,
            handle: 0,
            consecutive_failures: st.cfg.max_restarts, // 已达上限
        });
        // 崩溃（restore+registry 都在）但失败计数达上限 → GaveUp，记录保留
        assert_eq!(st.handle_death(port).await, DeathOutcome::GaveUp);
        assert!(st.watched_ports().is_empty(), "放弃后应摘除看护");
        assert!(
            crate::daemon::restore_file_path_in(dir.path(), port).is_file(),
            ".restore 必须保留（人工 restore 兜底）"
        );
    }

    #[tokio::test]
    async fn watchdog_state_respawn_failure_goes_to_pending_queue() {
        // H1 回归：respawn 失败（配置文件已不存在 → respawn_instance Err）后
        // 实例必须从 watched 摘除（不留悬空句柄条目）、进退避队列、计数跨周期累计
        let dir = tempfile::tempdir().unwrap();
        let port = "59903";
        write_crashed_instance(dir.path(), port); // config 指向 C:/tmp/no-such-config.toml
        // max_restarts=5：给累计留空间（test_cfg 的 2 会让第二次重试直接 GaveUp，
        // 测不到「未达上限继续入队」）
        let cfg = WatchdogConfig {
            max_restarts: 5,
            ..test_cfg(dir.path())
        };
        let mut st = WatchdogState::new(cfg);
        st.watched.push(Watched {
            port: port.into(),
            pid: u32::MAX - 777,
            handle: 0,
            consecutive_failures: 0,
        });
        // 首次崩溃：立即重拉 → 失败（配置不存在）→ 入队，failures=1
        assert_eq!(st.handle_death(port).await, DeathOutcome::RespawnFailed);
        assert!(
            !st.watched_ports().contains(&port.to_string()),
            "失败实例必须摘出 watched（H1：留条目会让 adopt_scan 永远跳过）"
        );
        assert_eq!(st.pending.len(), 1, "失败后应进退避队列");
        assert_eq!(st.pending[0].failures, 1);
        assert!(
            st.pending[0].next_retry > std::time::Instant::now(),
            "退避 1s：next_retry 应在未来"
        );
        // 配置不存在的失败路径：respawn_instance 已按既有语义删除 .restore
        // （与 aproxy restore 清理无法恢复的记录一致），此处验证队列语义
        assert!(
            !crate::daemon::restore_file_path_in(dir.path(), port).is_file(),
            "配置已删场景下 .restore 应被 respawn 清理（既有语义）"
        );

        // 拨快到期重试：再次失败（恢复记录已不在）→ failures=2 继续入队
        st.pending[0].next_retry = std::time::Instant::now() - Duration::from_secs(1);
        st.process_pending_retries().await;
        assert_eq!(st.pending.len(), 1, "未达上限应继续入队");
        assert_eq!(st.pending[0].failures, 2, "失败计数跨重试周期累计");
    }

    #[tokio::test]
    async fn watchdog_state_pending_gives_up_at_limit() {
        // 退避队列中累计到 max_restarts → GaveUp 且队列清空、.restore 保留
        let dir = tempfile::tempdir().unwrap();
        let port = "59904";
        write_crashed_instance(dir.path(), port);
        let cfg = test_cfg(dir.path()); // max_restarts = 2
        let mut st = WatchdogState::new(cfg);
        st.pending.push(PendingRetry {
            port: port.to_string(),
            failures: 1, // 再失败一次即达上限
            next_retry: std::time::Instant::now() - Duration::from_secs(1),
        });
        st.process_pending_retries().await;
        assert!(st.pending.is_empty(), "放弃后队列应清空");
        // 本用例配置文件也不存在：.restore 已被 respawn_instance 清理（既有
        // 语义——无法忠实恢复的记录不留）。真实 crashloop（端口被占等）时
        // .restore 保留，由集成测试 watchdog_respawns_killed_daemon 覆盖存活面。
    }

    #[tokio::test]
    async fn watchdog_state_handle_death_does_not_block() {
        // M2 回归：handle_death 不得内嵌退避 sleep——首次崩溃路径（respawn
        // 失败进队列）应立即返回
        let dir = tempfile::tempdir().unwrap();
        let port = "59905";
        write_crashed_instance(dir.path(), port);
        let cfg = test_cfg(dir.path());
        let mut st = WatchdogState::new(cfg);
        st.watched.push(Watched {
            port: port.into(),
            pid: u32::MAX - 777,
            handle: 0,
            consecutive_failures: 0,
        });
        let start = std::time::Instant::now();
        let _ = st.handle_death(port).await;
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "handle_death 必须立即返回（退避等待移到 tick 之间，M2）"
        );
    }

    #[tokio::test]
    async fn watchdog_state_pending_blocks_idle_exit() {
        // 退避队列非空 = 还有待恢复的崩溃实例，不得自灭（idle_exit_secs 拨小验证）
        let dir = tempfile::tempdir().unwrap();
        let port = "59906";
        write_crashed_instance(dir.path(), port);
        let cfg = WatchdogConfig {
            idle_exit_secs: 1, // 1s 即自灭（测试专用；0 = 禁用不能测出「被 pending 拦住」）
            ..test_cfg(dir.path())
        };
        let mut st = WatchdogState::new(cfg);
        st.pending.push(PendingRetry {
            port: port.to_string(),
            failures: 1,
            next_retry: std::time::Instant::now() + Duration::from_secs(3600), // 远期
        });
        // 队列非空：即使空置超过 idle_exit_secs 也不自灭（此处只验证判定路径
        // 不触发退出——退出是 std::process::exit，触发即测试进程死亡 = 失败）
        st.maybe_idle_exit().await;
        tokio::time::sleep(Duration::from_millis(1100)).await;
        st.maybe_idle_exit().await;
        assert!(
            !st.pending.is_empty(),
            "pending 应保持（测试进程仍活着即通过）"
        );
    }

    #[test]
    fn watchdog_next_pending_deadline() {
        let dir = tempfile::tempdir().unwrap();
        let st = WatchdogState::new(test_cfg(dir.path()));
        assert_eq!(st.next_pending_deadline(), None, "空队列无期限");
    }

    #[test]
    fn watchdog_backoff_progression() {
        // 1→2→4→8 指数增长，封顶 300
        assert_eq!(backoff_delay_secs(0), 0, "首次崩溃立即重拉");
        assert_eq!(backoff_delay_secs(1), 1);
        assert_eq!(backoff_delay_secs(2), 2);
        assert_eq!(backoff_delay_secs(3), 4);
        assert_eq!(backoff_delay_secs(4), 8);
        assert_eq!(backoff_delay_secs(20), 300, "封顶 300s");
        assert_eq!(backoff_delay_secs(u32::MAX), 300, "不溢出");
    }

    #[test]
    fn watchdog_config_from_settings_defaults() {
        // 生产路径组装：字段对齐 settings 语义
        let s = settings::Settings::default();
        assert_eq!(s.watchdog_heartbeat_secs.max(1), 30);
        assert_eq!(s.watchdog_max_restarts, 5);
        assert_eq!(s.watchdog_idle_exit_secs, 300);
    }
}
