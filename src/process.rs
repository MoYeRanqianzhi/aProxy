//! 进程原语（平台层）：创建时间、存活与退出、镜像路径、按「pid + 创建时间」核验身份后终止，
//! 以及看护者等待进程死亡用的同步句柄。看护者、实例管理（stop --force、restart）、安装器与
//! 拉起逻辑共用。
//!
//! 创建时间的单位随平台：Windows 为 FILETIME（100ns），unix 为 /proc/<pid>/stat 的 starttime
//! （时钟滴答）。只用于相等比较，不跨平台换算。

use std::path::PathBuf;

/// 查询进程创建时间（FILETIME/滴答）；进程不存在返回 None。
/// 供 spawn 前登记与收养时建立身份基线。
pub fn process_start_time(pid: u32) -> Option<u64> {
    imp_process::process_start_time(pid)
}

/// 进程是否已退出（终止确认）。与 `process_start_time(pid).is_none()` 的
/// 本质区别：进程终止后只要还有任何句柄（Rust 的 Child、看门狗的 sync
/// handle、调试器）维持内核对象，OpenProcess 就依然成功、start_time 查询
/// 依旧返回原值——「停止后等进程终止」场景（install 滚动重启）用 start_time
/// 判死会永远等不到。本原语查退出码（Windows）/进程态（unix），句柄存在
/// 不影响结论。
///
/// 返回 None = 无法判定（进程对象已彻底回收后 OpenProcess 失败 → Some(true)；
/// 权限不足等 → None，调用方保守处理）。
pub fn process_exited(pid: u32) -> Option<bool> {
    imp_process::process_exited(pid)
}

/// 进程镜像的完整路径（install 管辖检查用：实例 exe 是否在 ~/.aproxy/bin/
/// 下）。查询失败（进程刚死/权限）→ None——调用方按「不可判 → 不拦」处理
/// （管辖检查只拦「确认在管辖外」，误拦的代价是安装不可用）。
/// 注意这是「装在哪」的管辖判断，**不是**进程身份判断——身份一律走
/// record_identity（pid + 创建时间），与二进制路径/名称无关。
pub fn process_image_path(pid: u32) -> Option<PathBuf> {
    imp_process::process_image_path(pid)
}

// ---------------------------------------------------------------------------
// 进程身份：pid + 进程创建时间戳（与二进制名称无关）
// ---------------------------------------------------------------------------
//
// 身份问的是「这个 pid 现在是不是写下这条记录的那个进程」，由归属关系决定，
// 与二进制叫什么无关（用户定调：名称判断是严重谬误——官方 Release 资产名
// 本身就是 aproxy-<target>(.exe)，改名部署是合法形态；冒名进程改名即可绕过
// 名称比对，它对安全零贡献）。项目里可用的锚点有三个：
// 1. spawn 链：看护者亲手 spawn 并等到就绪的 pid 直接可信；
// 2. IPC 端点归属：应答 `<端口>` 端点 ping、且自报 pid 与记录一致的进程，
//    就是该端口的实例——但挂死实例不应答，所以它不能作为判据；
// 3. 进程创建时间戳：守护注册时把自己的创建时间写进注册表（InstanceRecord.
//    process_start），看护者 claim 同理（created_at_process）。pid 被复用后
//    新进程的创建时间必然不同，比对它就能把「同一个进程」钉死，且不需要
//    对方应答（挂死实例同样可判）。

/// 一条记录（pid + 登记的创建时间）在当前系统上的身份核验结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordIdentity {
    /// 记录的进程仍在运行：实测创建时间与登记值一致（携带实测值）
    Alive(u64),
    /// 该 pid 当前有在运行的进程，但创建时间与登记值不符：pid 已被系统
    /// 回收后复用，记录的进程早已死亡
    Reused,
    /// 该 pid 当前没有在运行的进程（不存在、已退出未回收的 zombie、或只剩
    /// 被句柄维持的已退出进程对象）
    Gone,
}

/// 核验记录身份：`recorded_start` 为记录登记的进程创建时间。
/// 「已退出但进程对象仍在」（unix zombie、Windows 被句柄维持的已退出对象）
/// 一律算 Gone——它们的创建时间仍查得到且与登记一致，但进程已死，不能计入
/// 选举的存活集合，也不该被收养或处决。
///
/// 登记值 0 不是可比对的锚点（平台读不到创建时间时守护登记的就是 0）：这样的
/// 记录证明不了任何进程是它，同样算 Gone——不计入选举、不收养、不处决。
pub fn record_identity(pid: u32, recorded_start: u64) -> RecordIdentity {
    if recorded_start == 0 {
        return RecordIdentity::Gone;
    }
    let Some(actual) = process_start_time(pid) else {
        return RecordIdentity::Gone;
    };
    if process_exited(pid) == Some(true) {
        return RecordIdentity::Gone;
    }
    if actual == recorded_start {
        RecordIdentity::Alive(actual)
    } else {
        RecordIdentity::Reused
    }
}

/// 核验后终止：仅当 pid 当前对应进程的创建时间 == `expected_start` 时终止
/// （SIGKILL / TerminateProcess，不做优雅关闭）。用于 `stop --force`、接管
/// 假死前任看护者、install 换血停旧看护者——一切「按记录主动杀」的动作。
///
/// - Windows：在**同一个进程句柄**上先 GetProcessTimes 核验、后
///   TerminateProcess。句柄打开后即钉住该进程对象（其 pid 在句柄关闭前不会
///   被复用），核验与终止之间不存在 pid 复用窗口。
/// - unix：读 /proc/<pid>/stat 的 starttime 核验后 kill(pid, SIGKILL)。核验与
///   kill 之间仍有理论窗口（无 pidfd 时的平台固有边界），需要进程恰在这
///   微秒级窗口内退出、且 pid 恰被回收再分配才会误中。
///
/// `expected_start == 0` 不是可比对的锚点，直接拒绝。不符/已退出/终止失败
/// 均返回 Err（信息已含原因）——宁可漏杀不误杀。
pub fn terminate_verified_process(pid: u32, expected_start: u64) -> Result<(), String> {
    if expected_start == 0 {
        return Err(format!(
            "pid {pid} 没有登记进程创建时间，无法核验身份，拒绝终止以防误杀"
        ));
    }
    imp::terminate_verified(pid, expected_start)
}

/// 进程是否存活且创建时间与 `created_at` 匹配；None = 无法判定（权限不足等）。
pub(crate) fn alive_with_start(pid: u32, created_at: u64) -> Option<bool> {
    imp_process::process_alive_with_start(pid, created_at)
}

// 看护者等待实例死亡用的同步句柄（Windows 为进程句柄；unix 上句柄值就是 pid，等待退化为每秒探活）
pub(crate) use imp::{close_handle, open_sync_handle, terminate_handle, wait_blocking};

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

    /// 核验后终止（语义见 super::terminate_verified_process）：同一句柄上
    /// 先 GetProcessTimes 比对创建时间、再 TerminateProcess——句柄钉住进程
    /// 对象，两步之间 pid 不可能被复用。已退出的进程（退出码非 STILL_ACTIVE）
    /// 按「已退出」报错，不当作终止成功。
    pub fn terminate_verified(pid: u32, expected_start: u64) -> Result<(), String> {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
            TerminateProcess,
        };
        unsafe {
            let h = OpenProcess(
                PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION,
                0,
                pid,
            );
            if h == 0 {
                return Err(format!("无法打开 pid {pid}（进程可能已退出）"));
            }
            let mut code: u32 = 0;
            let result = if super::imp_process::handle_start_time(h) != Some(expected_start) {
                Err(format!(
                    "pid {pid} 的进程创建时间与登记不符（pid 已被复用），拒绝终止以防误杀"
                ))
            } else if GetExitCodeProcess(h, &mut code) != 0 && code != 259 {
                Err(format!("pid {pid} 已退出"))
            } else if TerminateProcess(h, 1) == 0 {
                Err(format!(
                    "终止 pid {pid} 失败（{}）",
                    std::io::Error::last_os_error()
                ))
            } else {
                Ok(())
            };
            let _ = CloseHandle(h);
            result
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
    /// 核验后终止（语义见 super::terminate_verified_process）：/proc 的
    /// starttime 比对通过、且进程未退出（非 zombie）才 SIGKILL。unix 无可
    /// 钉住进程的句柄对象，核验与 kill 之间的微秒级窗口是平台固有边界。
    /// 无 /proc 的平台读不到创建时间，一律按「已退出/不可核验」拒绝。
    pub fn terminate_verified(pid: u32, expected_start: u64) -> Result<(), String> {
        match super::imp_process::process_start_time(pid) {
            None => return Err(format!("pid {pid} 已退出（或无法读取进程创建时间）")),
            Some(actual) if actual != expected_start => {
                return Err(format!(
                    "pid {pid} 的进程创建时间与登记不符（pid 已被复用），拒绝终止以防误杀"
                ));
            }
            Some(_) => {}
        }
        if super::imp_process::process_exited(pid) == Some(true) {
            return Err(format!("pid {pid} 已退出"));
        }
        let r = unsafe { kill(pid as i32, 9) };
        if r == 0 {
            Ok(())
        } else {
            Err(format!(
                "终止 pid {pid} 失败（{}）",
                std::io::Error::last_os_error()
            ))
        }
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
            OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle == 0 {
                return None;
            }
            let start = handle_start_time(handle);
            let _ = CloseHandle(handle);
            start
        }
    }

    /// 已打开进程句柄（需 PROCESS_QUERY_LIMITED_INFORMATION）的创建时间
    /// （FILETIME 100ns）。核验后终止要在同一句柄上取时间，故单独成函数。
    pub fn handle_start_time(handle: isize) -> Option<u64> {
        use windows_sys::Win32::System::Threading::GetProcessTimes;
        let mut creation: i64 = 0;
        let mut exit: i64 = 0;
        let mut kernel: i64 = 0;
        let mut user: i64 = 0;
        let ok = unsafe {
            GetProcessTimes(
                handle,
                &mut creation as *mut i64 as *mut _,
                &mut exit as *mut i64 as *mut _,
                &mut kernel as *mut i64 as *mut _,
                &mut user as *mut i64 as *mut _,
            )
        };
        if ok == 0 {
            return None;
        }
        Some(creation as u64)
    }

    /// 进程是否已退出：OpenProcess + GetExitCodeProcess。退出后即使进程
    /// 对象被其他句柄（Rust Child 等）维持，退出码也已确定——终止确认的
    /// 唯一可靠判据（start_time 查询在对象被引用期间会误报存活）。
    /// OpenProcess 失败（对象已彻底回收）= 已退出 → Some(true)。
    pub fn process_exited(pid: u32) -> Option<bool> {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle == 0 {
                return Some(true);
            }
            let mut code: u32 = 0;
            let ok = GetExitCodeProcess(handle, &mut code);
            let _ = CloseHandle(handle);
            if ok == 0 {
                return None;
            }
            // STILL_ACTIVE(259) = 未退出；注意退出码恰为 259 的进程无法区分
            // （Windows 文档明示的固有局限）——本仓库进程不用 259 作退出码
            Some(code != 259)
        }
    }

    /// 进程镜像完整路径（QueryFullProcessImageNameW）。
    pub fn process_image_path(pid: u32) -> Option<std::path::PathBuf> {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle == 0 {
                return None;
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
                return None;
            }
            Some(std::path::PathBuf::from(String::from_utf16_lossy(
                &buf[..len as usize],
            )))
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

    /// 进程是否已退出：/proc/<pid>/stat 的进程态——'Z'（zombie，已退出未
    /// 收割）与文件不存在（已收割）都算已退出。macOS 无 /proc 回退
    /// kill(pid,0)：ESRCH = 已退出；EPERM = 活着（权限拒绝但存在）。
    pub fn process_exited(pid: u32) -> Option<bool> {
        match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            Ok(stat) => {
                // comm 可含空格与括号，取最后一个 ')' 之后的首字段为 state
                let after = stat.rsplit(')').next()?;
                let state = after.split_whitespace().next()?;
                Some(state == "Z")
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(true),
            Err(_) => None,
        }
    }

    /// 进程镜像完整路径：/proc/<pid>/exe readlink。进程已死（ENOENT，含
    /// zombie）→ None；其他失败（跨用户权限等）→ None（调用方按「不可判
    /// → 不拦」处理）。路径可能带「 (deleted)」后缀（swap 升级场景），
    /// 调用方按需剥除。只服务 install 的管辖检查，不参与进程身份判定。
    pub fn process_image_path(pid: u32) -> Option<std::path::PathBuf> {
        std::fs::read_link(format!("/proc/{pid}/exe")).ok()
    }
}

// 测试辅助函数给 watchdog、daemon 的测试共用（被测对象同样是名字与 aProxy 无关的子进程）
#[cfg(all(test, any(windows, target_os = "linux")))]
pub(crate) mod tests {
    use super::*;
    use std::time::Duration;

    // ---------------- 进程身份（pid + 创建时间，与二进制名称无关） ----------------
    //
    // 下列测试用一个**名字与 aProxy 毫无关系**的子进程（Windows 的 PING.EXE /
    // unix 的 sleep）当被测对象：旧的名称判定会一律拒绝它，新判定只看
    // 「pid + 创建时间」——测试通过即证明身份判定已与名称解耦。子进程均由
    // 测试自己 spawn 并持有句柄，结束前自行回收。macOS 无 /proc 读不到创建
    // 时间（不在本轮支持范围），相关测试只在 Windows/Linux 编译运行。

    /// 拉起一个长时间存活、无副作用、名字与 aProxy 无关的子进程
    pub(crate) fn spawn_unrelated_child() -> std::process::Child {
        #[cfg(windows)]
        let mut cmd = {
            let mut c = std::process::Command::new("ping");
            c.args(["-n", "60", "127.0.0.1"]);
            c
        };
        #[cfg(target_os = "linux")]
        let mut cmd = {
            let mut c = std::process::Command::new("sleep");
            c.arg("60");
            c
        };
        cmd.stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn 测试子进程失败")
    }

    /// 等子进程退出（最多 5s），返回是否已退出。try_wait 在 unix 上会收割。
    pub(crate) fn wait_child_exit(child: &mut std::process::Child) -> bool {
        for _ in 0..50 {
            if child.try_wait().unwrap().is_some() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        false
    }

    #[test]
    fn record_identity_classifies_alive_reused_legacy_gone() {
        let mut child = spawn_unrelated_child();
        let pid = child.id();
        let start = process_start_time(pid).expect("子进程创建时间可查");
        assert_eq!(record_identity(pid, start), RecordIdentity::Alive(start));
        // 登记值不符 = pid 已被复用给别的进程
        assert_eq!(
            record_identity(pid, start.wrapping_add(1)),
            RecordIdentity::Reused
        );
        // 旧版本记录（未登记创建时间）：在世但无从核验
        // 登记值 0 证明不了身份：即便 pid 在世也不算 Alive
        assert_eq!(record_identity(pid, 0), RecordIdentity::Gone);
        // 已退出但未回收（unix zombie / Windows 被 Child 句柄维持的对象）：
        // 创建时间仍查得到且一致，但必须判 Gone——死进程不得计入存活集合
        child.kill().unwrap();
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(record_identity(pid, start), RecordIdentity::Gone);
        child.wait().unwrap();
        // 不存在的 pid
        assert_eq!(record_identity(u32::MAX - 7, 1), RecordIdentity::Gone);
    }

    #[test]
    fn terminate_verified_refuses_mismatch_and_kills_match() {
        let mut child = spawn_unrelated_child();
        let pid = child.id();
        let start = process_start_time(pid).expect("子进程创建时间可查");
        // pid 复用模拟：登记时间不符 → 拒绝，进程毫发无损
        assert!(terminate_verified_process(pid, start.wrapping_add(1)).is_err());
        // 无锚点（0）→ 拒绝
        assert!(terminate_verified_process(pid, 0).is_err());
        std::thread::sleep(Duration::from_millis(200));
        assert!(
            child.try_wait().unwrap().is_none(),
            "拒绝后子进程必须仍在运行"
        );
        // 时间一致 → 终止（与进程叫什么无关）
        terminate_verified_process(pid, start).expect("身份一致应终止成功");
        std::thread::sleep(Duration::from_millis(300));
        // 已退出（尚未回收）→ 再次终止报「已退出」而非误报成功
        assert!(terminate_verified_process(pid, start).is_err());
        assert!(wait_child_exit(&mut child), "终止后子进程应退出");
    }
}
