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
    /// （死亡检测退化为轮询，未实测分支）。
    handle: isize,
    /// 本实例连续重拉失败次数（crashloop 防护计数，重拉成功清零）
    consecutive_failures: u32,
}

/// 一个步进周期的完整看护状态
pub struct WatchdogState {
    pub cfg: WatchdogConfig,
    /// 被收养的实例（端口 → 状态）
    watched: Vec<Watched>,
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
            if self.watched.iter().any(|w| w.port == entry.port) {
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

    /// 健康扫描：心跳过期者走 IPC ping 二意见，都失败判挂死 → 杀（本方句柄
    /// 绑定原进程，无 PID 复用风险）→ 死亡事件走统一 respawn 路径。
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
            tracing::error!(port = %w.port, pid = w.pid, "实例心跳过期且 IPC 无响应，判定挂死，终止进程");
            hung.push(w.port.clone());
            imp::terminate_handle(w.handle);
        }
        hung
    }

    /// 处理死亡事件：.restore + 注册表都在 = 崩溃 → 重拉；否则优雅退出/清理，
    /// 摘除看护。指数退避 + crashloop 上限。返回处置结果（测试断言用）。
    pub async fn handle_death(&mut self, port: &str) -> DeathOutcome {
        let Some(idx) = self.watched.iter().position(|w| w.port == port) else {
            return DeathOutcome::Unknown;
        };
        let mut w = self.watched.swap_remove(idx);
        imp::close_handle(w.handle);

        let restore_path = crate::daemon::restore_file_path_in(&self.cfg.run_dir, port);
        let registry_path = crate::daemon::instance_file_path_in(&self.cfg.run_dir, port);
        if !restore_path.is_file() || !registry_path.is_file() {
            tracing::info!(port = %port, "实例已优雅退出（无恢复记录），摘除看护");
            self.after_watch_removal();
            return DeathOutcome::GracefulExit;
        }

        // crashloop 上限：连续失败达上限 → 放弃（保留 .restore 人工兜底）
        if w.consecutive_failures >= self.cfg.max_restarts {
            tracing::error!(
                port = %port,
                attempts = w.consecutive_failures,
                "实例连续重拉失败达上限，放弃自动重拉（.restore 已保留，可 aproxy restore 手工恢复）"
            );
            crate::daemon::append_startup_log(&format!(
                "[watchdog] 端口 {port} 的实例连续重拉 {} 次失败，已放弃自动重拉；可执行 aproxy restore 手工恢复",
                w.consecutive_failures
            ));
            self.after_watch_removal();
            return DeathOutcome::GaveUp;
        }

        // 退避：1s→2s→4s→…封顶 300s（按已连续失败次数）
        let delay = backoff_delay_secs(w.consecutive_failures);
        if delay > 0 {
            tracing::warn!(port = %port, delay_secs = delay, "实例崩溃，退避后重拉");
            tokio::time::sleep(Duration::from_secs(delay)).await;
        } else {
            tracing::warn!(port = %port, "实例崩溃，立即重拉");
        }

        match respawn_instance(&self.cfg.run_dir, port).await {
            Ok(pid) => {
                tracing::info!(port = %port, new_pid = pid, "实例已重拉并就绪");
                w.pid = pid;
                w.consecutive_failures = 0;
                if let Some(h) = imp::open_sync_handle(pid) {
                    w.handle = h;
                    self.spawn_death_watcher(port.to_string(), pid, h);
                } else {
                    // 句柄开不出来：轮询兜底（adopt_scan 下轮会补挂正确句柄）
                    w.handle = 0;
                }
                self.watched.push(w);
                self.empty_since = None;
                DeathOutcome::Respawned(pid)
            }
            Err(e) => {
                w.consecutive_failures += 1;
                tracing::error!(port = %port, error = %e, attempt = w.consecutive_failures, "重拉失败");
                // 保留 .restore（人工兜底），下轮 adopt_scan 视注册表情况重试；
                // 连续失败计数挂在端口上（不重新收养即丢失计数 → 记回 watched，
                // 下一轮死亡/adopt 后继续累计）
                self.watched.push(w);
                self.after_watch_removal();
                DeathOutcome::RespawnFailed
            }
        }
    }

    /// 一个完整步进周期：claim 续写 + 收养 + 健康 + 挂死事件处理 + 闲置自灭判定
    pub async fn tick(&mut self) {
        // 消化本周期内累积的死亡事件
        let mut deaths = Vec::new();
        while let Ok((port, _pid)) = self.deaths.try_recv() {
            deaths.push(port);
        }
        for port in deaths {
            self.handle_death(&port).await;
        }
        self.adopt_scan().await;
        self.health_scan().await;
        self.refresh_claim();
        self.maybe_idle_exit().await;
    }

    /// claim 续写：心跳时间戳刷新（其他进程据此刻定本看护者是否在任/假死）
    fn refresh_claim(&mut self) {
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

    /// 闲置自灭：全部实例清零（收养表空且注册表空）持续 idle_exit_secs 后
    /// 删除 claim 退出，系统回到零常驻。
    async fn maybe_idle_exit(&mut self) {
        if self.cfg.idle_exit_secs == 0 {
            return;
        }
        let registry_empty = crate::daemon::registry_pids_in(&self.cfg.run_dir).is_empty();
        if self.watched.is_empty() && registry_empty {
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
}

/// 死亡事件处置结果（handle_death 返回，测试断言用）
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DeathOutcome {
    /// 崩溃并成功重拉（携带新 PID）
    Respawned(u32),
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
    // 就绪判定：IPC ping（8s，与 start/restore 一致）
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    loop {
        if crate::daemon::ipc_ping(port).await.is_ok() {
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

    let mut state = WatchdogState::new(cfg);
    state.adopt_scan().await;
    let scan = Duration::from_secs(state.cfg.scan_secs.max(1));
    loop {
        tokio::time::sleep(scan).await;
        state.tick().await;
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
    pub fn open_sync_handle(_pid: u32) -> Option<isize> {
        // unix 无进程句柄对象；死亡检测退化为轮询（未实测分支，同 UDS 批处理）
        None
    }
    pub fn wait_blocking(_handle: isize) {
        // unix 无句柄等待原语接入（未实测分支）：恒久挂起占位，
        // 死亡检测退化为 adopt/health 轮询
        loop {
            std::thread::sleep(Duration::from_secs(3600));
        }
    }
    pub fn terminate_handle(_handle: isize) {}
    pub fn close_handle(_handle: isize) {}
    pub fn terminate_verified(_pid: u32) {}
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

    pub fn is_aproxy_process(_pid: u32) -> bool {
        // unix 分支未实测（TODO）；镜像名校验缺位时保守放行——选举只影响
        // 谁发起 spawn，误判最坏结果是多个 spawn 尝试（claim 原子性兜底）
        true
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

    #[test]
    fn heartbeat_section_name_uses_port() {
        // 节名含端口：实例唯一区分（同 IPC 管道命名规则）
        let n = heartbeat_section_name("12345");
        assert!(n.contains("12345"), "{n}");
        assert!(n.contains("aproxy-heart"), "{n}");
    }

    #[cfg(windows)]
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
