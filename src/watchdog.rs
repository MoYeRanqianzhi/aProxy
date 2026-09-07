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
    port: String,
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
        imp::create_heartbeat(port)
    }

    /// 写入当前毫秒时间戳（原子 store，无锁）
    pub fn beat(&self) {
        imp::heartbeat_store(self, now_millis());
    }
}

impl Drop for HeartbeatWriter {
    fn drop(&mut self) {
        imp::unmap_heartbeat(self);
    }
}

/// 看护侧读取：实例最近一次心跳的毫秒时间戳；节不存在 = 实例无心跳
/// （旧版本守护或创建失败）→ None，看护者按「无心跳数据」处理（只做
/// 进程死亡检测，不做挂死判定）。
pub fn read_heartbeat(port: &str) -> Option<u64> {
    imp::heartbeat_load(port)
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
                port: port.to_string(),
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
mod imp {
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
}
