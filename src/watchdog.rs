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
    imp::process_alive_with_start(pid, created_at).unwrap_or(false)
}

/// 查询进程创建时间（FILETIME/滴答）；进程不存在返回 None。
/// 供 spawn 前登记与收养时建立身份基线。
pub fn process_start_time(pid: u32) -> Option<u64> {
    imp::process_start_time(pid)
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
        if imp::is_aproxy_process(pid) && min_alive.is_none_or(|m| pid < m) {
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
// 平台实现
// ---------------------------------------------------------------------------
#[cfg(windows)]
mod imp {
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
mod imp {
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
        let mut s = settings::Settings::default();
        s.watchdog_heartbeat_secs = 0;
        assert_eq!(heartbeat_period(&s), Duration::from_secs(1));
        assert_eq!(heartbeat_fresh_secs(&s), 3);
    }
}
