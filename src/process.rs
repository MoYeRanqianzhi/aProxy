//! 进程原语（平台层）：创建时间、存活与退出、镜像路径、核验身份后终止，以及看护者等待进程
//! 死亡用的同步句柄。看护者、实例管理（stop --force、restart）、安装器与拉起逻辑共用。
//!
//! 身份有两种锚点，都只用于相等比较：
//! - **启动标记**（字符串，见 [`aproxy_ipc::ledger::start_marker`]）：实例记录、看护者记录
//!   与台账里登记的都是它，[`record_identity`] 与 [`terminate_marked_process`] 按它核验；
//! - **创建时间**（数字；Windows 为 FILETIME，Linux 为开机滴答，macOS 为 Unix 纪元以来的微秒）：只给
//!   「刚拉起、还没写下任何记录的子进程」与安装状态用。

pub use aproxy_ipc::registry::{RecordIdentity, process_exited, record_identity};
use std::path::PathBuf;

#[path = "i18n/process.rs"]
mod text;
use aproxy_ipc::i18n::Lang;

/// 拒绝或未能按标记终止进程。台账提供的原始系统证据保留在原因中。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminationError {
    /// 缺少身份锚点，不能安全终止。
    Unmarked(u32),
    /// 记录的进程已退出或 PID 已复用。
    Gone(u32),
    /// 经过核验，属主不是当前用户。
    Other(u32, String),
    /// 系统未能结束进程。
    Failed(u32, String),
}
impl TerminationError {
    /// 按接收方语言说明同一项身份核验或系统错误。
    pub fn render(&self, lang: Lang) -> String {
        match self {
            Self::Unmarked(pid) => text::unmarked(lang, *pid),
            Self::Gone(pid) => text::gone(lang, *pid),
            Self::Other(pid, owner) => text::other(lang, *pid, owner),
            Self::Failed(pid, why) => text::failed(lang, *pid, why),
        }
    }
}

/// 查询进程创建时间（单位见模块说明）；进程不存在或无权限查询时返回 None。
/// 供 spawn 前登记与收养时建立身份基线。
pub fn process_start_time(pid: u32) -> Option<u64> {
    imp_process::process_start_time(pid)
}

/// 进程镜像的完整路径（install 管辖检查用：实例 exe 是否在 ~/.aproxy/bin/
/// 下）。查询失败（进程刚死/权限）→ None——调用方按「不可判 → 不拦」处理
/// （管辖检查只拦「确认在管辖外」，误拦的代价是安装不可用）。
/// 注意这是「装在哪」的管辖判断，**不是**进程身份判断——身份一律走
/// record_identity（pid + 创建时间），与二进制路径/名称无关。
pub fn process_image_path(pid: u32) -> Option<PathBuf> {
    imp_process::process_image_path(pid)
}

/// 核验后终止：仅当 pid 当前对应进程的启动标记 == `expected_start` 时强制结束它，并等它
/// 退出（最多 5 秒）。用于 `stop --force` 等「按记录主动杀」的动作。核验与结束走台账的
/// [`aproxy_ipc::ledger::terminate`]（Windows 在同一个句柄上先核验后结束，Linux 经 pidfd），
/// 属主不是本用户的进程不结束。
///
/// 空的 `expected_start` 不是可比对的锚点，直接拒绝。不符、已退出、结束失败均返回 Err
/// （信息已含原因）——宁可漏杀不误杀。阻塞调用。
pub fn terminate_marked_process(pid: u32, expected_start: &str) -> Result<(), TerminationError> {
    use aproxy_ipc::ledger;
    if expected_start.is_empty() {
        return Err(TerminationError::Unmarked(pid));
    }
    let entry = ledger::LedgerEntry {
        ledger: ledger::LEDGER_FORMAT,
        pid,
        start: expected_start.to_string(),
        role: ledger::ROLE_INSTANCE.to_string(),
        version: String::new(),
        port: None,
        label: None,
        exe: None,
        config: None,
    };
    match ledger::terminate(&entry, std::time::Duration::from_secs(5)) {
        ledger::Termination::Terminated => Ok(()),
        ledger::Termination::AlreadyGone => Err(TerminationError::Gone(pid)),
        ledger::Termination::NotOwned(owner) => Err(TerminationError::Other(pid, owner)),
        ledger::Termination::Failed(why) => Err(TerminationError::Failed(pid, why)),
    }
}

/// 核验后终止（按数字创建时间）：仅当 pid 当前对应进程的创建时间 == `expected_start` 时
/// 终止（SIGKILL / TerminateProcess，不做优雅关闭）。只给「本进程刚拉起、还没写下任何记录
/// 的子进程」用——它的身份锚点是拉起时读到的创建时间；按记录终止走
/// [`terminate_marked_process`]。
///
/// - Windows：在**同一个进程句柄**上先 GetProcessTimes 核验、后
///   TerminateProcess。句柄打开后即钉住该进程对象（其 pid 在句柄关闭前不会
///   被复用），核验与终止之间不存在 pid 复用窗口。
/// - unix：读内核的创建时间核验后 kill(pid, SIGKILL)。核验与
///   kill 之间仍有理论窗口（无 pidfd 时的平台固有边界），需要进程恰在这
///   微秒级窗口内退出、且 pid 恰被回收再分配才会误中。
///
/// `expected_start == 0` 不是可比对的锚点，直接拒绝。不符/已退出/终止失败
/// 均返回 Err（信息已含原因）——宁可漏杀不误杀。
pub fn terminate_verified_process(pid: u32, expected_start: u64) -> Result<(), String> {
    if expected_start == 0 {
        return Err(format!(
            "pid {pid} has no recorded creation time; refusing termination because its identity cannot be verified"
        ));
    }
    imp::terminate_verified(pid, expected_start)
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
                return Err(format!(
                    "cannot open pid {pid} (the process may have exited)"
                ));
            }
            let mut code: u32 = 0;
            let result = if super::imp_process::handle_start_time(h) != Some(expected_start) {
                Err(format!(
                    "the creation time of pid {pid} differs from the record; refusing to terminate a reused pid"
                ))
            } else if GetExitCodeProcess(h, &mut code) != 0 && code != 259 {
                Err(format!("pid {pid} has exited"))
            } else if TerminateProcess(h, 1) == 0 {
                Err(super::text::failed(
                    super::Lang::En,
                    pid,
                    &std::io::Error::last_os_error().to_string(),
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
    /// Linux 从 procfs、macOS 从 libproc 读取进程态，无需自己是它的父进程。
    pub fn wait_blocking(handle: isize) {
        let pid = handle as i32;
        loop {
            if !process_alive_for_wait(pid) {
                return;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    }

    /// 无法读取进程态时继续等，权限不足不是触发重拉的依据。
    #[cfg(target_os = "macos")]
    fn process_alive_for_wait(pid: i32) -> bool {
        super::process_exited(pid as u32) != Some(true)
    }

    /// procfs 可读时排除僵尸，否则用信号 0 查询存在性（EPERM 也说明进程存在）。
    #[cfg(not(target_os = "macos"))]
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
        std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }

    /// SIGKILL 处决（挂死判定后调用）：unix 直接以句柄中保存的 pid 杀进程。
    /// 上层靠死亡 watcher 的轮询确认死亡。
    pub fn terminate_handle(handle: isize) {
        unsafe {
            kill(handle as i32, 9);
        }
    }
    pub fn close_handle(_handle: isize) {}
    /// 核验后终止（语义见 super::terminate_verified_process）：内核创建时间
    /// 比对通过、且进程未退出（非 zombie）才 SIGKILL。unix 无可
    /// 钉住进程的句柄对象，核验与 kill 之间的微秒级窗口是平台固有边界。
    /// 读不到创建时间时，一律按「已退出/不可核验」拒绝。
    pub fn terminate_verified(pid: u32, expected_start: u64) -> Result<(), String> {
        match super::imp_process::process_start_time(pid) {
            None => {
                return Err(format!(
                    "pid {pid} has exited or its creation time cannot be read"
                ));
            }
            Some(actual) if actual != expected_start => {
                return Err(format!(
                    "the creation time of pid {pid} differs from the record; refusing to terminate a reused pid"
                ));
            }
            Some(_) => {}
        }
        if super::process_exited(pid) == Some(true) {
            return Err(format!("pid {pid} has exited"));
        }
        let r = unsafe { kill(pid as i32, 9) };
        if r == 0 {
            Ok(())
        } else {
            Err(super::text::failed(
                super::Lang::En,
                pid,
                &std::io::Error::last_os_error().to_string(),
            ))
        }
    }
}

#[cfg(windows)]
mod imp_process {
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

#[cfg(all(unix, not(target_os = "macos")))]
mod imp_process {
    /// /proc/<pid>/stat 的 starttime（字段 22，时钟滴答）。
    pub fn process_start_time(pid: u32) -> Option<u64> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        // comm 可含空格与括号，取最后一个 ')' 之后切字段
        let after = stat.rsplit(')').next()?;
        let fields: Vec<&str> = after.split_whitespace().collect();
        fields.get(19)?.parse().ok()
    }

    /// 进程镜像完整路径：/proc/<pid>/exe readlink。进程已死（ENOENT，含
    /// zombie）→ None；其他失败（跨用户权限等）→ None（调用方按「不可判
    /// → 不拦」处理）。路径可能带「 (deleted)」后缀（swap 升级场景），
    /// 调用方按需剥除。只服务 install 的管辖检查，不参与进程身份判定。
    pub fn process_image_path(pid: u32) -> Option<std::path::PathBuf> {
        std::fs::read_link(format!("/proc/{pid}/exe")).ok()
    }
}

/// macOS 的进程快照只用于清点；读取某个 pid 的镜像时，进程可能已经退出。
#[cfg(target_os = "macos")]
pub(crate) use imp_process::process_ids;

#[cfg(target_os = "macos")]
mod imp_process {
    pub use aproxy_ipc::registry::process_start_time;

    /// proc_listallpids 的缓冲区大小用字节，返回值却是 pid 个数。快照刚好填满时
    /// 可能有进程没列出来，扩大缓冲区再取；枚举失败不据此推断任何进程已退出。
    pub(crate) fn process_ids() -> Vec<u32> {
        let mut pids = vec![0u32; 256];
        loop {
            let Ok(bytes) = i32::try_from(std::mem::size_of_val(&pids[..])) else {
                return Vec::new();
            };
            let count = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), bytes) };
            if count <= 0 {
                return Vec::new();
            }
            if (count as usize) < pids.len() {
                pids.truncate(count as usize);
                pids.retain(|&pid| pid > 0);
                return pids;
            }
            pids.resize(pids.len() * 2, 0);
        }
    }

    /// proc_pidpath 返回路径字节数（不含 NUL）；进程刚退出或无权限读取时返回 0。
    /// 保留原始字节，路径只用来判断安装位置，不用来认定进程身份。
    pub fn process_image_path(pid: u32) -> Option<std::path::PathBuf> {
        use std::os::unix::ffi::OsStringExt;
        let pid = i32::try_from(pid).ok().filter(|&pid| pid > 0)?;
        let mut path = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
        let len = unsafe { libc::proc_pidpath(pid, path.as_mut_ptr().cast(), path.len() as u32) };
        if len <= 0 {
            return None;
        }
        path.truncate(len as usize);
        Some(std::ffi::OsString::from_vec(path).into())
    }
}

// 测试辅助函数给 watchdog、daemon 的测试共用（被测对象同样是名字与 aProxy 无关的子进程）
#[cfg(all(test, any(windows, target_os = "linux", target_os = "macos")))]
pub(crate) mod tests {
    use super::*;
    use std::time::Duration;

    // ---------------- 进程身份（pid + 创建时间，与二进制名称无关） ----------------
    //
    // 下列测试用一个**名字与 aProxy 毫无关系**的子进程（Windows 的 PING.EXE /
    // unix 的 sleep）当被测对象：旧的名称判定会一律拒绝它，新判定只看
    // 「pid + 创建时间」——测试通过即证明身份判定已与名称解耦。子进程均由
    // 测试自己 spawn 并持有句柄，结束前自行回收。

    /// 拉起一个长时间存活、无副作用、名字与 aProxy 无关的子进程
    pub(crate) fn spawn_unrelated_child() -> std::process::Child {
        #[cfg(windows)]
        let mut cmd = {
            let mut c = std::process::Command::new("ping");
            c.args(["-n", "60", "127.0.0.1"]);
            c
        };
        #[cfg(unix)]
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

    /// 数字锚点和台账字符串必须标识同一次启动；小数点后的微秒不补零。
    /// 枚举只是安装后的报告依据，不代替 pid 和创建时间的身份核验。
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_process_queries_agree_with_ledger_and_executable() {
        let pid = std::process::id();
        let micros = process_start_time(pid).unwrap();
        assert_eq!(
            aproxy_ipc::ledger::start_marker(pid).unwrap(),
            format!("m:{}.{}", micros / 1_000_000, micros % 1_000_000)
        );
        assert_eq!(
            process_image_path(pid).unwrap().canonicalize().unwrap(),
            std::env::current_exe().unwrap().canonicalize().unwrap()
        );
        assert!(process_ids().contains(&pid));
        assert_eq!(process_exited(0), None);
        assert_eq!(process_exited(u32::MAX), Some(true));
        assert_eq!(process_start_time(u32::MAX), None);
        assert_eq!(process_image_path(u32::MAX), None);
    }

    #[test]
    fn record_identity_classifies_alive_reused_legacy_gone() {
        let mut child = spawn_unrelated_child();
        let pid = child.id();
        let start = aproxy_ipc::ledger::start_marker(pid).expect("子进程的启动标记可查");
        assert_eq!(record_identity(pid, &start), RecordIdentity::Alive);
        // 登记值不符 = pid 已被复用给别的进程
        assert_eq!(
            record_identity(pid, &format!("{start}0")),
            RecordIdentity::Reused
        );
        // 空的登记值证明不了身份：即便 pid 在世也不算 Alive
        assert_eq!(record_identity(pid, ""), RecordIdentity::Gone);
        // 已退出但未回收（unix zombie / Windows 被 Child 句柄维持的对象）：
        // 不论系统还能否读到标记，都必须判 Gone——死进程不得计入存活集合
        child.kill().unwrap();
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(process_exited(pid), Some(true));
        assert_eq!(record_identity(pid, &start), RecordIdentity::Gone);
        child.wait().unwrap();
        // 不存在的 pid
        assert_eq!(record_identity(u32::MAX - 7, "w:1"), RecordIdentity::Gone);
    }

    #[test]
    fn terminate_marked_refuses_mismatch_and_kills_match() {
        let mut child = spawn_unrelated_child();
        let pid = child.id();
        let start = aproxy_ipc::ledger::start_marker(pid).expect("子进程的启动标记可查");
        // 标记不符（pid 被复用的形态）与没有登记标记：都拒绝，子进程必须仍在
        assert!(terminate_marked_process(pid, &format!("{start}0")).is_err());
        let err = terminate_marked_process(pid, "").unwrap_err();
        assert!(matches!(err, TerminationError::Unmarked(p) if p == pid));
        std::thread::sleep(Duration::from_millis(200));
        assert!(
            child.try_wait().unwrap().is_none(),
            "拒绝后子进程必须仍在运行"
        );
        // 标记一致：结束，并且已经等到它退出
        terminate_marked_process(pid, &start).expect("身份一致应终止成功");
        assert!(wait_child_exit(&mut child), "终止后子进程应退出");
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
