//! 进程台账 `<home>/ledger/`：永久兼容契约 C3（设计全文见
//! `.agents/plan/release-0.2.0.md` 的「W2b 强制安装：永久兼容契约」一节）。
//!
//! **永久兼容契约：本文件定义的目录名、文件名、条目的 JSON 字段（名字、类型、含义）、
//! `start` 启动标记的推导方式与 `role` 词表，无论如何都不得改动，只许追加可选字段。**
//!
//! 为什么不能改：从 0.2.0 起，任意版本 X 的 `aproxy install --force` 都要能结束同一个
//! home 下由任意版本 Y 启动的全部 aproxy 进程——X 可能比 Y 旧，也可能比 Y 新。强制安装
//! 不走 IPC、不读实例注册表与 install.state（那些都可以随版本演进），台账是它找到进程、
//! 确认「这个 pid 仍是写下条目的那个进程」的唯一依据。某个版本只要改动这里的任何一处
//! （换目录、改字段名、换启动标记的算法、删掉一个角色），别的版本的强制安装就会漏掉
//! 它启动的进程，或者认不出它写下的启动标记而拒绝结束——强制安装的保证随之失效，而且
//! 偏偏要等到用户已经陷入「普通安装失败」的困境、最需要它的时候才会暴露。万一将来非改
//! 不可，只能另立一份并行的新格式，旧格式照写不误。
//!
//! 谁来写：每个长期运行的 aproxy 进程——后台与前台实例、看护者、安装者（含交接后接手
//! 的新版本进程、`install --continue` 续作、强制安装自己）、桌面界面——在占用端口或拉起
//! 任何子进程之前登记一条 `<home>/ledger/<pid>.json`，正常退出时删除。写失败只告警、
//! 照常运行：台账只服务强制安装这一条兜底路径，不能反过来拖垮正常服务。
//!
//! 台账与实例注册记录（`run/<端口>.pid`）是两回事（G3），不要合并：注册记录服务
//! status/stop/IPC，可以随版本自由演进；台账只回答「这个 home 下有哪些 aproxy 进程、
//! 怎样确认 pid 仍是它」，所以冻结得越少越好。
//!
//! 谁来读、读者的义务：强制安装。读者跳过解析不了的文件；忽略不认识的字段；不认识的
//! `role` 照样结束，只是排在最后；条目对应的进程已死（pid 不在、或启动标记对不上——
//! pid 已被复用给别的进程）时删掉条目；结束某个进程之后也删掉它的条目。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

/// 台账目录名：`<home>/ledger/`。
///
/// 永久兼容契约 C3：无论如何都不得改动。每个版本的强制安装都在这个位置找进程。
pub const LEDGER_DIR: &str = "ledger";

/// 条目 `ledger` 字段的取值。写者恒写 1；读者要求这个字段存在（它标明「这是一条台账
/// 条目」），但不按它的值分支。
///
/// 永久兼容契约 C3：无论如何都不得改动。格式只许追加可选字段，所以永远是 1；真要换
/// 格式，只能另立并行的新格式（新目录或新文件），这个值不会因此变成 2。
pub const LEDGER_FORMAT: u32 = 1;

/// `role` 词表：安装者（普通安装、续作、交接后的接手者、强制安装）。
///
/// 永久兼容契约 C3：词表只增不减，已有的词无论如何都不得改名或删除。读者对不认识的
/// 角色照样结束、排在最后，所以以后增加角色不会让旧版本的强制安装漏掉它。
pub const ROLE_INSTALLER: &str = "installer";
/// `role` 词表：看护者（watchdog）。永久兼容契约 C3，同上。
pub const ROLE_WATCHDOG: &str = "watchdog";
/// `role` 词表：代理实例（后台与前台都是它）。永久兼容契约 C3，同上。
pub const ROLE_INSTANCE: &str = "instance";
/// `role` 词表：桌面界面。永久兼容契约 C3，同上。
pub const ROLE_DESKTOP: &str = "desktop";

/// 启动标记前缀：Windows。完整形如 `w:<GetProcessTimes 的创建时间 FILETIME，十进制>`，
/// FILETIME 是 1601-01-01 起的 100ns 计数，取 `(dwHighDateTime << 32) | dwLowDateTime`
/// 的无符号十进制，例如 `w:133730123456789012`。
///
/// 永久兼容契约 C3：前缀与推导方式无论如何都不得改动。条目的 `start` 由写下它的进程
/// 自己按这里的方式算出；任何版本的强制安装都按同样的方式现算一遍、逐字符比较，两边
/// 算法只要差一个字符，强制安装就认不出它、拒绝结束。前缀区分平台：同一个 home 被别的
/// 系统读到（例如 WSL 读 Windows 的目录）时，标记永远对不上，于是只会拒绝、不会误杀。
pub const START_WINDOWS: &str = "w:";
/// 启动标记前缀：Linux。完整形如 `l:<boot_id>:<starttime>`：`boot_id` 是
/// `/proc/sys/kernel/random/boot_id` 去掉首尾空白后的原文（小写、带连字符的 UUID），
/// `starttime` 是 `/proc/<pid>/stat` 第 22 个字段（开机以来的时钟滴答，十进制）。
/// 带上 boot_id，是因为 starttime 从开机算起：重启之后 pid 与滴答数恰好重复，也不会
/// 把新进程认成旧条目。
///
/// 永久兼容契约 C3：无论如何都不得改动（理由见 [`START_WINDOWS`]）。
pub const START_LINUX: &str = "l:";
/// 启动标记前缀：macOS。完整形如 `m:<pbi_start_tvsec>.<pbi_start_tvusec>`：
/// `proc_pidinfo(PROC_PIDTBSDINFO)` 返回的 `proc_bsdinfo` 里两个字段各自的十进制，不补零
/// （微秒为 5 时写 `.5`，不写 `.000005`）。它只是一个按值逐字比较的标记，不当小数解读。
///
/// 永久兼容契约 C3：无论如何都不得改动（理由见 [`START_WINDOWS`]）。
pub const START_MACOS: &str = "m:";

/// 台账目录：`<home>/ledger/`。
pub fn ledger_dir_in(home: &Path) -> PathBuf {
    home.join(LEDGER_DIR)
}

/// 一个进程的条目文件：`<home>/ledger/<pid>.json`（pid 十进制）。
///
/// 永久兼容契约 C3：文件名无论如何都不得改动。读者只认扩展名为 `json` 的文件，写到
/// 一半的临时文件（`<pid>.json.tmp`）不会被当成条目。
pub fn entry_path_in(home: &Path, pid: u32) -> PathBuf {
    ledger_dir_in(home).join(format!("{pid}.json"))
}

/// 一条台账条目，落盘是一行 JSON，字段按下面的顺序写出，例如：
///
/// ```text
/// {"ledger":1,"pid":4242,"start":"w:133730123456789012","role":"instance","version":"0.2.0","port":12345,"label":"C:\\Users\\me\\.aproxy\\config.toml","exe":"C:\\Users\\me\\.aproxy\\bin\\aproxy.exe"}
/// ```
///
/// 永久兼容契约 C3：已有字段的名字、类型、含义无论如何都不得改动，只许追加可选字段
/// （`#[serde(default)]`，读者缺了它也能读）。不加 `deny_unknown_fields`：以后的版本追加
/// 的字段，今天的读者必须能忽略。`role` 用字符串而不是枚举：不认识的角色也要读得出来。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerEntry {
    /// 恒为 [`LEDGER_FORMAT`]
    pub ledger: u32,
    /// 进程 id
    pub pid: u32,
    /// 进程启动标记（推导方式见 [`START_WINDOWS`]、[`START_LINUX`]、[`START_MACOS`]）：
    /// pid 被复用后标记必然不同，强制安装据此确认 pid 仍是写下条目的那个进程
    pub start: String,
    /// 角色，取值见 `ROLE_*`（词表只增不减）
    pub role: String,
    /// 写下条目的那个 aproxy 的版本（不带 v），只供展示与排查
    pub version: String,
    /// 实例在配置里的监听端口（可选；实例才有）。强制安装据此判断被结束的实例有没有
    /// 被 restore 拉回来，拉不回的按它列出来请用户手动启动
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// 给人看的说明（可选，自由文本）：实例写配置文件路径，安装者写它的模式
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// 写下条目时进程镜像的完整路径（可选），只供展示与排查
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exe: Option<String>,
}

impl LedgerEntry {
    /// 本进程的条目：pid 与启动标记现取，版本取本构建的版本。读不到启动标记（不支持的
    /// 平台）→ None。
    pub fn for_current_process(role: &str, port: Option<u16>, label: Option<&str>) -> Option<Self> {
        let pid = std::process::id();
        Some(Self {
            ledger: LEDGER_FORMAT,
            pid,
            start: start_marker(pid)?,
            role: role.to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            port,
            label: label.map(str::to_string),
            exe: std::env::current_exe()
                .ok()
                .map(|p| p.display().to_string()),
        })
    }

    /// 落盘用的一行 JSON（不含换行）。
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("台账条目只含字符串与整数，序列化不会失败")
    }

    /// 解析一条落盘的条目：缺必填字段、类型不对、不是 JSON 都算解析不了（None），读者
    /// 跳过这样的文件；多出来的字段一律忽略。
    pub fn parse(text: &str) -> Option<Self> {
        serde_json::from_str(text.trim()).ok()
    }

    /// 强制安装结束进程的先后：安装者最先（它们可能正在改 bin/，也可能在交接里拉起新的
    /// 安装者），然后看护者（先于实例结束，免得它把刚结束的实例当成崩溃重新拉起来），
    /// 再是实例、桌面界面，不认识的角色排在最后。
    ///
    /// 永久兼容契约 C3 的读者义务：不认识的角色照样结束、排在最后。
    pub fn stop_order(&self) -> u8 {
        match self.role.as_str() {
            ROLE_INSTALLER => 0,
            ROLE_WATCHDOG => 1,
            ROLE_INSTANCE => 2,
            ROLE_DESKTOP => 3,
            _ => 4,
        }
    }
}

// ---------------------------------------------------------------------------
// 写者：登记、退出时删除
// ---------------------------------------------------------------------------

/// 本进程登记的条目文件（进程内至多一条）。显式 `std::process::exit` 的路径不跑析构，
/// [`exit`] 据此在退出前删掉它。
static OWN_ENTRY: Mutex<Option<PathBuf>> = Mutex::new(None);

fn own_entry() -> std::sync::MutexGuard<'static, Option<PathBuf>> {
    // 持锁时只做赋值与 take，不会在持锁期间 panic；即便中毒，里面的路径照样可信
    OWN_ENTRY.lock().unwrap_or_else(|e| e.into_inner())
}

/// 登记的守卫：析构（正常返回、panic 展开）时删掉本进程的条目。经 `std::process::exit`
/// 退出的路径不跑析构，要改用 [`exit`]。
#[must_use = "守卫一析构就会删掉台账条目，要让它活到进程结束"]
pub struct Registration {
    _private: (),
}

impl Drop for Registration {
    fn drop(&mut self) {
        remove_own();
    }
}

/// 把本进程登记进 `home` 的台账：必须在占用端口或拉起任何子进程之前调用。写失败
/// （目录不可写、平台读不到启动标记）只告警，照常返回守卫——台账只服务强制安装这条
/// 兜底路径，不能挡住正常服务。
pub fn register(home: &Path, role: &str, port: Option<u16>, label: Option<&str>) -> Registration {
    match LedgerEntry::for_current_process(role, port, label) {
        Some(entry) => match write_entry_in(home, &entry) {
            Ok(path) => *own_entry() = Some(path),
            Err(e) => tracing::warn!(
                error = %e,
                role,
                "failed to write the process ledger entry; continuing without it (install --force will not see this process)"
            ),
        },
        None => tracing::warn!(
            role,
            "cannot derive this process's start marker on this platform; process ledger entry not written"
        ),
    }
    Registration { _private: () }
}

/// 原子写入一条条目：先写同目录的 `<pid>.json.tmp`，再 rename 成 `<pid>.json`。直接写
/// 目标文件时，并发的读者可能读到写到一半的 JSON 而跳过它——强制安装恰好在这一刻扫描，
/// 就会漏掉这个进程。同一个 pid 同一时刻只可能属于一个进程，临时文件名不会互踩；死去的
/// 同 pid 进程留下的旧条目被直接覆盖（Windows 上 std::fs::rename 也会替换已存在的目标）。
pub fn write_entry_in(home: &Path, entry: &LedgerEntry) -> std::io::Result<PathBuf> {
    let dir = ledger_dir_in(home);
    std::fs::create_dir_all(&dir)?;
    let path = entry_path_in(home, entry.pid);
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, entry.to_json() + "\n")?;
    match std::fs::rename(&tmp, &path) {
        Ok(()) => Ok(path),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// 删掉本进程登记的条目（没登记过就什么都不做）。
pub fn remove_own() {
    if let Some(path) = own_entry().take() {
        let _ = std::fs::remove_file(path);
    }
}

/// 删掉本进程的条目后退出进程。`std::process::exit` 不跑析构，[`Registration`] 管不到
/// 这条路，所以登记过的进程一律经这里退出。
pub fn exit(code: i32) -> ! {
    remove_own();
    std::process::exit(code)
}

// ---------------------------------------------------------------------------
// 读者：枚举、判活、核验后结束
// ---------------------------------------------------------------------------

/// 读到的一条条目与它的文件路径（删除条目用路径，不按 pid 重拼：读者只信文件内容）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub path: PathBuf,
    pub entry: LedgerEntry,
}

/// 枚举 `home` 的台账：只看扩展名为 `json` 的文件，解析不了的跳过（不删——它可能是
/// 别的版本按以后的格式写的，留给写它的进程自己收拾）。目录不存在 = 没有条目。
pub fn read_all_in(home: &Path) -> Vec<Found> {
    let Ok(dir) = std::fs::read_dir(ledger_dir_in(home)) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in dir.flatten() {
        let path = item.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(entry) = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| LedgerEntry::parse(&text))
        else {
            continue;
        };
        out.push(Found { path, entry });
    }
    out.sort_by_key(|f| (f.entry.stop_order(), f.entry.pid));
    out
}

/// `pid` 当前那个进程的启动标记，按冻结的推导方式（见 `START_*`）现算。进程不存在、
/// 读不到、或本平台没有定义标记 → None。
pub fn start_marker(pid: u32) -> Option<String> {
    imp::start_marker(pid)
}

/// 条目对应的进程是否仍在运行：pid 当前的启动标记与条目一致，并且没有退出（Windows
/// 上被别的句柄维持着的已退出进程对象、unix 上还没被收割的 zombie，都算已经不在）。
/// 查不到（进程不在、pid 已被复用、权限不足）一律算不在。
pub fn is_alive(entry: &LedgerEntry) -> bool {
    imp::is_alive(entry.pid, &entry.start)
}

/// 核验后结束一个条目对应的进程的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Termination {
    /// 核验通过、已强制结束，并且在等待时限内确认它已经退出
    Terminated,
    /// 进程已经不在（pid 不存在、启动标记对不上——pid 已被复用、或已经退出）：
    /// 条目作废，什么都没结束
    AlreadyGone,
    /// 确认是条目里的进程，却结束不了（权限不足），或结束后在时限内没有退出
    Failed(String),
}

/// 核验后强制结束条目对应的进程，并等它真正退出（最多 `wait`）。只认 pid + 启动标记，
/// 与进程叫什么、从哪里运行无关；核验不过就什么都不做（宁可漏杀，不可误杀）。
///
/// - Windows：打开进程句柄，在**同一个句柄**上用 GetProcessTimes 现算启动标记、比对，
///   通过才 TerminateProcess，然后在这个句柄上等它退出。句柄钉住了进程对象，句柄关闭
///   之前 pid 不会被复用，核验与结束之间没有窗口。
/// - Linux：先 pidfd_open 拿到钉住进程的 pidfd，再读 `/proc/<pid>/stat` 与 boot_id 比对，
///   通过后经 pidfd_send_signal 发 SIGKILL。比对时读到的若已是复用这个 pid 的新进程，
///   标记必然对不上；信号只会送到 pidfd 指向的那个进程，没有窗口。内核不支持 pidfd
///   （5.3 之前，或容器的 seccomp 拒绝了这个系统调用）时退回「核验后 kill(pid)」，见
///   `imp` 里的说明。
/// - macOS：没有 pidfd，核验后 kill(pid)，理论窗口同 Linux 的退路。
///
/// 阻塞调用（内部按毫秒级间隔等待）。
pub fn terminate(entry: &LedgerEntry, wait: Duration) -> Termination {
    imp::terminate(entry.pid, &entry.start, wait)
}

/// unix 上等进程退出：轮询到标记不再一致（进程已被收割、pid 被复用）或成了 zombie。
/// 被结束的进程多半不是本进程的子进程，它们的父进程（或 init）会收割；是本进程的子
/// 进程时（测试里常见），zombie 也算已经退出，不必等收割。
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn wait_until_gone(pid: u32, start: &str, wait: Duration) -> Termination {
    let deadline = std::time::Instant::now() + wait;
    loop {
        if !imp::is_alive(pid, start) {
            return Termination::Terminated;
        }
        if std::time::Instant::now() >= deadline {
            return Termination::Failed(format!(
                "pid {pid} 收到 SIGKILL 后 {} 秒内仍未退出",
                wait.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(windows)]
mod imp {
    use super::Termination;
    use std::time::Duration;
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_INVALID_PARAMETER, FILETIME, GetLastError, WAIT_OBJECT_0,
    };
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, TerminateProcess, WaitForSingleObject,
    };

    /// GetExitCodeProcess 对仍在运行的进程报告的值。退出码恰为 259 的进程无从区分
    /// （Windows 文档写明的固有局限），aproxy 的进程不用这个退出码。
    const STILL_ACTIVE: u32 = 259;

    /// 打开的进程句柄，析构时关闭。
    struct Process(isize);

    impl Drop for Process {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    /// 以 `access` 打开 `pid`；失败返回 GetLastError 的错误码（87 = 没有这个 pid 的进程，
    /// 5 = 权限不足）。
    fn open(pid: u32, access: u32) -> Result<Process, u32> {
        let handle = unsafe { OpenProcess(access, 0, pid) };
        if handle == 0 {
            Err(unsafe { GetLastError() })
        } else {
            Ok(Process(handle))
        }
    }

    impl Process {
        /// 永久兼容契约 C3 的 Windows 启动标记：创建时间 FILETIME 的 64 位值，十进制。
        fn marker(&self) -> Option<String> {
            let zero = FILETIME {
                dwLowDateTime: 0,
                dwHighDateTime: 0,
            };
            let (mut creation, mut exit, mut kernel, mut user) = (zero, zero, zero, zero);
            let ok = unsafe {
                GetProcessTimes(self.0, &mut creation, &mut exit, &mut kernel, &mut user)
            };
            if ok == 0 {
                return None;
            }
            let ticks =
                (u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime);
            Some(format!("{}{ticks}", super::START_WINDOWS))
        }

        /// 已经退出：退出码已确定。句柄（本进程的、别的进程的）维持着进程对象时，
        /// 创建时间照样查得到，只能靠退出码判断。
        fn exited(&self) -> bool {
            let mut code = 0u32;
            unsafe { GetExitCodeProcess(self.0, &mut code) != 0 && code != STILL_ACTIVE }
        }
    }

    pub fn start_marker(pid: u32) -> Option<String> {
        open(pid, PROCESS_QUERY_LIMITED_INFORMATION).ok()?.marker()
    }

    pub fn is_alive(pid: u32, start: &str) -> bool {
        match open(pid, PROCESS_QUERY_LIMITED_INFORMATION) {
            Ok(process) => process.marker().as_deref() == Some(start) && !process.exited(),
            Err(_) => false,
        }
    }

    pub fn terminate(pid: u32, start: &str, wait: Duration) -> Termination {
        let process = match open(
            pid,
            PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
        ) {
            Ok(process) => process,
            Err(ERROR_INVALID_PARAMETER) => return Termination::AlreadyGone,
            Err(code) => {
                // 多半是权限不足（例如进程以管理员身份运行，而强制安装没有）。用最小权限
                // 再看一眼它是不是条目里的进程：是就如实报告结束不了；不是（pid 已被复用、
                // 或已经退出）就作废条目
                return if is_alive(pid, start) {
                    Termination::Failed(format!(
                        "无法以结束权限打开 pid {pid}（Windows 错误码 {code}，多半是权限不足：该进程以更高权限运行时，强制安装也要以同样的权限运行）"
                    ))
                } else {
                    Termination::AlreadyGone
                };
            }
        };
        if process.marker().as_deref() != Some(start) || process.exited() {
            return Termination::AlreadyGone;
        }
        if unsafe { TerminateProcess(process.0, 1) } == 0 {
            let err = std::io::Error::last_os_error();
            // 核验与结束之间它自己退出了：TerminateProcess 对已退出的进程报拒绝访问
            return if process.exited() {
                Termination::AlreadyGone
            } else {
                Termination::Failed(format!("结束 pid {pid} 失败：{err}"))
            };
        }
        // 等它真正退出（镜像文件随之释放）再让调用方去动二进制。TerminateProcess 是异步的，
        // 正常几毫秒内完成；卡在驱动里的 I/O 可能拖住它
        let millis = u32::try_from(wait.as_millis())
            .unwrap_or(u32::MAX - 1)
            .min(u32::MAX - 1);
        if unsafe { WaitForSingleObject(process.0, millis) } == WAIT_OBJECT_0 {
            Termination::Terminated
        } else {
            Termination::Failed(format!(
                "pid {pid} 已被结束，但 {} 秒内没有退出",
                wait.as_secs()
            ))
        }
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::Termination;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::time::Duration;

    /// `/proc/<pid>/stat` 里的进程状态（第 3 个字段）与 starttime（第 22 个字段）。comm
    /// （第 2 个字段）可以含空格与括号，所以从最后一个 `)` 之后切分：之后的第一个字段是
    /// 第 3 个，starttime 是其后的第 19 个（下标 19）。
    fn stat(pid: u32) -> Option<(char, u64)> {
        let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let (_, after_comm) = text.rsplit_once(')')?;
        let fields: Vec<&str> = after_comm.split_whitespace().collect();
        let state = fields.first()?.chars().next()?;
        let starttime = fields.get(19)?.parse().ok()?;
        Some((state, starttime))
    }

    fn boot_id() -> Option<String> {
        let text = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?;
        let id = text.trim();
        (!id.is_empty()).then(|| id.to_string())
    }

    /// 永久兼容契约 C3 的 Linux 启动标记 `l:<boot_id>:<starttime>`，连同进程状态一起返回。
    fn marker_and_state(pid: u32) -> Option<(String, char)> {
        let (state, starttime) = stat(pid)?;
        Some((
            format!("{}{}:{starttime}", super::START_LINUX, boot_id()?),
            state,
        ))
    }

    pub fn start_marker(pid: u32) -> Option<String> {
        marker_and_state(pid).map(|(marker, _)| marker)
    }

    /// 标记一致，且不是 zombie（Z，已退出未收割）或正在消亡（X）。
    pub fn is_alive(pid: u32, start: &str) -> bool {
        matches!(marker_and_state(pid), Some((marker, state)) if marker == start && !matches!(state, 'Z' | 'X'))
    }

    pub fn terminate(pid: u32, start: &str, wait: Duration) -> Termination {
        // pid 0 与负数在 kill 里表示进程组，绝不能送进去
        let raw = match libc::pid_t::try_from(pid) {
            Ok(raw) if raw > 0 => raw,
            _ => return Termination::AlreadyGone,
        };
        // 1. 先拿 pidfd，钉住此刻占着这个 pid 的进程。之后的核验若通过，信号只会送到
        //    这个进程；核验时读到的若已是复用 pid 的新进程，标记必然对不上，不会发信号
        let opened = unsafe { libc::syscall(libc::SYS_pidfd_open, raw, 0) };
        let pidfd = if opened >= 0 {
            // pidfd_open 返回的是本进程新开的描述符，交给 OwnedFd 负责关闭
            Some(unsafe { OwnedFd::from_raw_fd(opened as libc::c_int) })
        } else {
            match std::io::Error::last_os_error().raw_os_error() {
                Some(libc::ESRCH) => return Termination::AlreadyGone,
                // ENOSYS（内核早于 5.3）、EPERM（容器的 seccomp 规则没放行这个系统调用）
                // 等：退回「核验后 kill(pid)」。核验与 kill 之间有一个理论窗口：进程恰在
                // 这几微秒里退出、被收割，pid 又恰好被分配给新进程，SIGKILL 才会落到别人
                // 身上。Linux 按递增顺序分配 pid、绕回前要用完 pid_max 个号，这个窗口在
                // 实践中碰不到，但它确实存在，所以只作退路
                _ => None,
            }
        };
        // 2. 核验：启动标记一致、且还没退出
        if !is_alive(pid, start) {
            return Termination::AlreadyGone;
        }
        // 3. 发 SIGKILL：有 pidfd 走 pidfd_send_signal，否则 kill
        let sent = match &pidfd {
            Some(fd) => unsafe {
                libc::syscall(
                    libc::SYS_pidfd_send_signal,
                    fd.as_raw_fd(),
                    libc::SIGKILL,
                    std::ptr::null::<libc::siginfo_t>(),
                    0,
                )
            },
            None => libc::c_long::from(unsafe { libc::kill(raw, libc::SIGKILL) }),
        };
        if sent != 0 {
            let err = std::io::Error::last_os_error();
            return if err.raw_os_error() == Some(libc::ESRCH) || !is_alive(pid, start) {
                Termination::AlreadyGone
            } else {
                Termination::Failed(format!("向 pid {pid} 发送 SIGKILL 失败：{err}"))
            };
        }
        // 4. 等它真正退出
        super::wait_until_gone(pid, start, wait)
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::Termination;
    use std::time::Duration;

    /// proc_pidinfo(PROC_PIDTBSDINFO)：进程不在（或读不到）→ None。
    fn bsdinfo(pid: u32) -> Option<libc::proc_bsdinfo> {
        let pid = libc::c_int::try_from(pid).ok()?;
        // proc_bsdinfo 是纯数据结构体（整数与 char 数组），全零是合法的初值
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = libc::c_int::try_from(std::mem::size_of::<libc::proc_bsdinfo>()).ok()?;
        let written = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                0,
                (&mut info as *mut libc::proc_bsdinfo).cast(),
                size,
            )
        };
        (written == size).then_some(info)
    }

    /// 永久兼容契约 C3 的 macOS 启动标记 `m:<pbi_start_tvsec>.<pbi_start_tvusec>`。
    fn marker(info: &libc::proc_bsdinfo) -> String {
        format!(
            "{}{}.{}",
            super::START_MACOS,
            info.pbi_start_tvsec,
            info.pbi_start_tvusec
        )
    }

    pub fn start_marker(pid: u32) -> Option<String> {
        bsdinfo(pid).map(|info| marker(&info))
    }

    /// 标记一致，且不是 zombie（SZOMB，已退出未收割）。
    pub fn is_alive(pid: u32, start: &str) -> bool {
        matches!(bsdinfo(pid), Some(info) if info.pbi_status != libc::SZOMB && marker(&info) == start)
    }

    pub fn terminate(pid: u32, start: &str, wait: Duration) -> Termination {
        let raw = match libc::pid_t::try_from(pid) {
            Ok(raw) if raw > 0 => raw,
            _ => return Termination::AlreadyGone,
        };
        if !is_alive(pid, start) {
            return Termination::AlreadyGone;
        }
        // macOS 没有 pidfd：核验与 kill 之间的理论窗口同 Linux 的退路（见那里的说明）
        if unsafe { libc::kill(raw, libc::SIGKILL) } != 0 {
            let err = std::io::Error::last_os_error();
            return if err.raw_os_error() == Some(libc::ESRCH) || !is_alive(pid, start) {
                Termination::AlreadyGone
            } else {
                Termination::Failed(format!("向 pid {pid} 发送 SIGKILL 失败：{err}"))
            };
        }
        super::wait_until_gone(pid, start, wait)
    }
}

/// 其余 unix（FreeBSD 等）：契约没有为它们定义启动标记，台账写不出条目，也不按台账
/// 结束进程。真要支持时，只能为它追加新的前缀，不得改动已有的三种。
#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
mod imp {
    use super::Termination;
    use std::time::Duration;

    pub fn start_marker(_pid: u32) -> Option<String> {
        None
    }

    pub fn is_alive(_pid: u32, _start: &str) -> bool {
        false
    }

    pub fn terminate(pid: u32, _start: &str, _wait: Duration) -> Termination {
        Termination::Failed(format!("本平台不支持按台账结束进程（pid {pid}）"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_serializes_in_the_frozen_field_order() {
        let entry = LedgerEntry {
            ledger: 1,
            pid: 7,
            start: "w:1".into(),
            role: ROLE_WATCHDOG.into(),
            version: "0.2.0".into(),
            port: None,
            label: None,
            exe: None,
        };
        // 可选字段缺省时不写出
        assert_eq!(
            entry.to_json(),
            r#"{"ledger":1,"pid":7,"start":"w:1","role":"watchdog","version":"0.2.0"}"#
        );
        assert_eq!(LedgerEntry::parse(&entry.to_json()).as_ref(), Some(&entry));
    }

    #[test]
    fn stop_order_puts_unknown_roles_last() {
        let with_role = |role: &str| LedgerEntry {
            ledger: 1,
            pid: 1,
            start: "w:1".into(),
            role: role.into(),
            version: "0.2.0".into(),
            port: None,
            label: None,
            exe: None,
        };
        let order: Vec<u8> = [
            ROLE_INSTALLER,
            ROLE_WATCHDOG,
            ROLE_INSTANCE,
            ROLE_DESKTOP,
            "something-new",
        ]
        .into_iter()
        .map(|r| with_role(r).stop_order())
        .collect();
        assert_eq!(order, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn register_writes_and_exit_path_removes_own_entry() {
        let home = tempfile::tempdir().unwrap();
        let path = entry_path_in(home.path(), std::process::id());
        {
            let _guard = register(home.path(), ROLE_INSTALLER, None, Some("test"));
            // 不支持的平台读不到启动标记，不写条目
            if start_marker(std::process::id()).is_none() {
                assert!(!path.exists());
                return;
            }
            let found = read_all_in(home.path());
            assert_eq!(found.len(), 1);
            assert_eq!(found[0].path, path);
            assert_eq!(found[0].entry.role, ROLE_INSTALLER);
            assert_eq!(found[0].entry.label.as_deref(), Some("test"));
            assert!(is_alive(&found[0].entry), "本进程的条目应判为在运行");
        }
        assert!(!path.exists(), "守卫析构应删掉条目");
    }

    #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
    #[test]
    fn own_start_marker_is_stable_and_platform_shaped() {
        let pid = std::process::id();
        let marker = start_marker(pid).expect("本进程的启动标记可查");
        assert_eq!(start_marker(pid).as_deref(), Some(marker.as_str()));
        let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
        #[cfg(windows)]
        assert!(
            marker.strip_prefix(START_WINDOWS).is_some_and(digits),
            "{marker}"
        );
        #[cfg(target_os = "linux")]
        {
            let rest = marker.strip_prefix(START_LINUX).expect(&marker);
            let (boot_id, starttime) = rest.rsplit_once(':').expect(&marker);
            assert_eq!(boot_id.len(), 36, "{marker}");
            assert!(digits(starttime), "{marker}");
        }
        #[cfg(target_os = "macos")]
        {
            let rest = marker.strip_prefix(START_MACOS).expect(&marker);
            let (sec, usec) = rest.split_once('.').expect(&marker);
            assert!(digits(sec) && digits(usec), "{marker}");
        }
        // 不存在的 pid 没有标记
        assert_eq!(start_marker(u32::MAX - 7), None);
    }
}
