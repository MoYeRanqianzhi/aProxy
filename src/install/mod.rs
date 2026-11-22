//! install/upgrade：二进制安装与升级。
//!
//! 设计全文见 `.agents/plan/install-v1.md`（四条铁律：先标记、先下载后
//! rename、ACK 齐了才交换、逐个重启最后删除）。本模块当前承载状态文件
//! 与状态机；下载链条、交换原语、IPC 广播等随计划分步落位。

/// 普通安装的跨进程诊断；强制安装按自己的契约报告。
#[path = "../i18n/install_reason.rs"]
pub mod reason;
use aproxy_ipc::i18n::Lang;
use reason::Cause;
pub mod announce;
pub mod broadcast;
pub mod download;
/// 安装文件操作的原因：普通流程记录参数，强制流程只取原有措辞。
#[path = "../i18n/install_files.rs"]
pub mod files;
pub mod flow;
/// 普通交接、锁竞争与滚动安装的私有诊断参数。
#[path = "../i18n/install_flow.rs"]
pub mod flow_issue;
use files::FileIssue;
use flow_issue::FlowIssue;
pub mod force;
pub mod restart;
pub mod skills;
pub mod staging;
pub mod state;
pub mod swap;

use std::path::{Path, PathBuf};

/// 普通安装在操作边界确定失败码，交棒与落盘只搬运它，不从翻译后的文字猜原因。
#[derive(Debug)]
pub struct InstallFailure {
    /// 码表里的名字；接手者可能是更高版本，所以保留它写下的原名。
    pub code: String,
    /// 人能读懂的原因；也是较早的安装者唯一认得的 last_error。
    pub message: String,
    /// 与这个码同一项的详情；没有字段时仍是空对象。
    pub details: Box<serde_json::Value>,
    /// 用于本进程或交接状态的原因；不作为公开 error 的字段。
    pub cause: Option<Box<Cause>>,
}

impl InstallFailure {
    /// 在知道原因的那一层选码；没有结构化详情的失败到这里就构造完了。
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            details: Box::new(serde_json::json!({})),
            cause: None,
        }
    }

    /// 同一原因同时生成持久的英文和可以发给接手者的排版参数。
    pub fn because(code: &str, cause: Cause) -> Self {
        let message = cause.render(Lang::En);
        Self {
            cause: Some(Box::new(cause)),
            ..Self::new(code, message)
        }
    }
    /// 旧二进制的说明和不识别的私有排版参数一律回退到该记录的原文。
    pub fn render(&self, lang: Lang) -> String {
        self.cause
            .as_ref()
            .map(|r| r.render(lang))
            .unwrap_or_else(|| self.message.clone())
    }
    /// 附加操作阶段时仍能使用底层原因的结构化参数。
    pub fn into_cause(self) -> Cause {
        self.cause
            .map(|c| *c)
            .unwrap_or(Cause::Evidence(self.message))
    }
    /// 上层补充操作上下文时保留底层的码与详情。
    pub fn with_message(self, message: String) -> Self {
        Self {
            message,
            cause: None,
            ..self
        }
    }

    /// 给操作阶段加一层说明，保持原有的码、详情及内层的跨进程排版参数。
    pub(crate) fn context<T: Into<Cause>>(mut self, wrap: impl FnOnce(Cause) -> T) -> Self {
        let inner = self
            .cause
            .take()
            .map(|v| *v)
            .unwrap_or_else(|| Cause::Evidence(std::mem::take(&mut self.message)));
        let cause = wrap(inner).into();
        self.message = cause.render(Lang::En);
        self.cause = Some(Box::new(cause));
        self
    }
    /// 强制安装的共享原语必须交付原有的字节，不把普通流程的语言策略带进冻结路径。
    fn for_force(self) -> Self {
        let message = self.render(Lang::Zh);
        self.with_message(message)
    }

    /// 交棒后使用接手版本留下的失败记录；旧版本没写码时仍然按普通失败退出。
    pub fn recorded(state: &state::InstallState, message: String) -> Self {
        let mut cause = state
            .error_render
            .clone()
            .and_then(|v| serde_json::from_value::<Box<Cause>>(v).ok());
        // 私有原因必须和同一位置的公开诊断一致；不能让错序、缺项的排版数据改写证据。
        if let Some(Cause::Precheck { issues, .. }) = cause.as_deref_mut() {
            let diagnostics = state
                .error_details
                .as_ref()
                .and_then(|v| v["diagnostics"].as_array());
            *issues = diagnostics
                .into_iter()
                .flatten()
                .enumerate()
                .map(|(i, d)| {
                    let text = d["message"].as_str().unwrap_or_default();
                    issues
                        .get(i)
                        .filter(|r| r.render(Lang::En) == text)
                        .cloned()
                        .unwrap_or_else(|| aproxy_ipc::i18n::diagnostic::Reason::Verbatim {
                            text: text.to_owned(),
                        })
                })
                .collect();
        }
        Self {
            cause: cause.filter(|c| c.render(Lang::En) == message),
            code: state
                .error_code
                .clone()
                .unwrap_or_else(|| "install_failed".into()),
            message,
            details: Box::new(
                state
                    .error_details
                    .clone()
                    .filter(|v| v.is_object())
                    .unwrap_or_else(|| serde_json::json!({})),
            ),
        }
    }

    /// 安装码都带固定退出码；若接手者比自己新，读不懂的码仍以失败结束。
    pub fn exit_code(&self) -> i32 {
        use aproxy_ipc::code::{self, ExitCode, Family};
        match code::lookup(&self.code)
            .filter(|c| c.families.contains(&Family::Install))
            .and_then(|c| c.exit)
        {
            Some(ExitCode::Fixed(n)) => i32::from(n),
            _ => i32::from(code::EXIT_FAILED),
        }
    }
}

impl std::fmt::Display for InstallFailure {
    /// 日志与已有的文字界面显示原因，不把机器码混进 last_error。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl From<String> for InstallFailure {
    /// 没有专用码的本地操作保留原来的完整原因。
    fn from(message: String) -> Self {
        Self::new("install_failed", message)
    }
}

impl From<InstallFailure> for String {
    /// 只报告文字的安装接口不解释普通安装码，仍保留完整的人类可读原因。
    fn from(error: InstallFailure) -> Self {
        error.message
    }
}

/// 二进制安装的一切状态都只放在这里：`<home>/install/`。状态文件（state.rs）、备料与
/// 下载暂存（staging.rs）、一键安装脚本的下载暂存、unix 的安装宣告（announce.rs）都在
/// 它下面，此外不写任何地方，没有例外。skill 文档的下载暂存（`<home>/skills/` 下的
/// `.dl`、`.staging`）不属于二进制安装：它们要与目标目录同卷才能原子替换；强制安装不碰
/// skill，所以不受影响。
///
/// 收在一处是永久兼容契约 C4 的路径部分：强制安装（不走交接、不依赖任何版本读懂
/// 别的版本的状态）可以随时整目录清空它，而不必认识里面的内容。
///
/// 0.1.0 把状态写在 `run/install.state`、备料放在 `<home>/staging/`，新位置读不到
/// 它们是有意的：0.1.0 留下的安装现场与 0.2.0 起的安装器互不相认，自然隔离。
pub fn install_dir_in(home: &Path) -> PathBuf {
    home.join("install")
}

/// 文件 `file` 是否在目录 `dir` 里或其下。安装的管辖检查（实例的二进制是否在
/// `<home>/bin/` 下）与强制安装第 7 步的报告（哪些进程从 bin 运行）都用它判断。
///
/// 比较的是解析后的真实路径：两边都先 canonicalize，再按 settings::path_match_key 归一
/// （去 `\\?\` 前缀；Windows 上统一分隔符并转小写）。同一个目录可以有好几种写法——
/// Windows 的 8.3 短名（`C:\Users\RUNNER~1\…` 与 `C:\Users\runneradmin\…`）、符号链接与
/// junction、`\\?\` 前缀。进程的镜像路径（QueryFullProcessImageNameW）保留的是启动时
/// 用的写法，经短名启动的进程报出来就带着短名（本机实测）；`<home>` 可能来自展开成长名
/// 的用户目录，也可能来自写着短名的 APROXY_HOME。不解析，两边就会对不上：管辖检查把
/// bin 里的实例当成管辖外而拒绝安装，第 7 步漏报。
///
/// 解析的是 `file` 所在的目录而不是文件本身：镜像文件可能已被改名或删除（交换时让位、
/// unix 上被 rename 覆盖），它所在的目录还在。解析不了的（已不存在、没有权限）退回原样。
pub fn file_is_within(file: &Path, dir: &Path) -> bool {
    let Some(parent) = file.parent() else {
        return false;
    };
    let key = |p: &Path| {
        let real = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        let mut key = crate::settings::path_match_key(&real.display().to_string());
        // 以分隔符收尾再比前缀：`…\bin2` 不能算在 `…\bin` 下
        if !key.ends_with(std::path::MAIN_SEPARATOR) {
            key.push(std::path::MAIN_SEPARATOR);
        }
        key
    };
    key(parent).starts_with(&key(dir))
}

/// 安装锁的文件名：`<home>/install/lock`。
///
/// 永久兼容契约 C4：位置、文件名与「操作系统级排他锁」的语义无论如何都不得改动。
/// 任何版本修改 `<home>/bin/` 期间（换二进制、删让位的旧二进制、写入口兜底脚本）都
/// 持有它；强制安装先按台账结束全部安装者、再拿这把锁，然后才动 bin/。某个版本换了
/// 锁的位置或方式，它与别的版本就会同时改 bin/，互相踩坏对方的交换。
pub const LOCK_FILE: &str = "lock";

/// 安装锁文件：`<home>/install/lock`（见 [`LOCK_FILE`]）。
pub fn lock_path_in(home: &Path) -> PathBuf {
    install_dir_in(home).join(LOCK_FILE)
}

/// 持有中的安装锁，析构即释放。锁是文件句柄上的操作系统锁（见 [`lock_install_in`]），
/// 持有者进程死亡时由系统自动释放，所以崩溃不会留下打不开的死锁。
#[must_use = "keep the installation lock alive until changes to bin/ are complete"]
pub struct InstallLock {
    file: std::fs::File,
}

/// 析构时先显式解锁，再随 File 关闭句柄。只关句柄系统也会解锁，但：Windows 上
/// LockFileEx 的文档说那种解锁何时发生「取决于可用的系统资源」，建议显式解锁；unix 上
/// flock 跟着打开的文件走，本进程恰好 fork 出、还没 exec 的子进程手里那份副本会让锁
/// 多留一会儿。显式解锁之后，等着拿锁的下一个安装者不必多等。
impl Drop for InstallLock {
    #[cfg(unix)]
    fn drop(&mut self) {
        use std::os::unix::io::AsRawFd;
        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
    }

    #[cfg(windows)]
    fn drop(&mut self) {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::UnlockFileEx;
        // OVERLAPPED 全零即从第 0 字节起，范围与加锁时相同
        let mut overlapped: windows_sys::Win32::System::IO::OVERLAPPED =
            unsafe { std::mem::zeroed() };
        unsafe {
            UnlockFileEx(
                self.file.as_raw_handle() as isize,
                0,
                u32::MAX,
                u32::MAX,
                &mut overlapped,
            );
        }
    }
}

/// 拿安装锁，最多等 `wait`：打开锁文件、加锁，任何一步不成都每 100 毫秒从头再试一次，
/// 到时仍不成 → Err（谁也不许无限等——强制安装要按退出码规则结束，普通安装落 failed
/// 现场等续作）。
///
/// 永久兼容契约 C4：锁原语无论如何都不得改动——
/// - unix：`flock(fd, LOCK_EX|LOCK_NB)`；
/// - Windows：`LockFileEx(LOCKFILE_EXCLUSIVE_LOCK|LOCKFILE_FAIL_IMMEDIATELY)`，从第 0 字节
///   起、长度 u64::MAX；CreateFile 的共享模式为 FILE_SHARE_READ|FILE_SHARE_WRITE，不带
///   FILE_SHARE_DELETE。
///
/// 两个版本只有用同一种锁、锁同一段字节，才互相看得见。所以直接调用系统接口，不用 std
/// 的 `File::try_lock`：std 文档明说它「目前」对应 flock 与 LockFileEx、以后可能改变，
/// 也没写明 Windows 上锁哪一段。某天它在 unix 上换成 fcntl/OFD 锁（在 Linux 上与 flock
/// 互不排斥），或在 Windows 上换了锁的范围，新旧版本就会同时修改 bin/，踩坏对方的交换。
///
/// 打开也在重试里：Windows 上别的进程（杀毒软件、另一个版本）以不兼容的共享模式开着
/// 这个文件时，打开会暂时失败，那也是「等一会儿」而不是「立即放弃」。
///
/// 同一个进程只能持有一份：锁绑在打开的文件上，同一进程再开一次、再锁一次，会被自己
/// 先前那份挡住。
///
/// Windows 上不给 FILE_SHARE_DELETE：持锁期间别人删不掉这个文件。否则删掉之后有人新建
/// 一个同名文件再锁上，就会有两个持有者。unix 没有对应的办法，靠所有版本都不删它
/// （强制安装清空 `<home>/install/` 时也跳过它）。
pub fn lock_install_in(home: &Path, wait: std::time::Duration) -> Result<InstallLock, String> {
    lock_for_install(home, wait).map_err(|e| e.render(Lang::Zh))
}

/// 普通安装要区分另一进程持锁与系统调用失败；两种安装仍竞争同一个系统锁。
fn lock_for_install(home: &Path, wait: std::time::Duration) -> Result<InstallLock, InstallFailure> {
    let dir = install_dir_in(home);
    let path = lock_path_in(home);
    let deadline = std::time::Instant::now() + wait;
    loop {
        let failure = match crate::fsx::create_dir_all(&dir).and_then(|()| open_lock_file(&path)) {
            Err(e) => FlowIssue::LockOpen {
                path: path.display().to_string(),
                e: e.to_string(),
            }
            .into(),
            Ok(file) => match try_lock_exclusive(&file) {
                Ok(true) => return Ok(InstallLock { file }),
                Ok(false) => InstallFailure::because(
                    "install_in_progress",
                    FlowIssue::Locked {
                        path: path.display().to_string(),
                        secs: wait.as_secs(),
                    }
                    .into(),
                ),
                Err(e) => FlowIssue::LockFailed {
                    path: path.display().to_string(),
                    e: e.to_string(),
                }
                .into(),
            },
        };
        if std::time::Instant::now() >= deadline {
            return Err(failure);
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// 在同一份短事务的锁上等候。锁竞争与打开错误分开，不能把未拿到的锁当作成功。
pub(crate) fn wait_lock_path(
    path: &Path,
    wait: std::time::Duration,
) -> std::io::Result<InstallLock> {
    let until = std::time::Instant::now() + wait;
    loop {
        if let Some(lock) = try_lock_path(path)? {
            return Ok(lock);
        }
        if std::time::Instant::now() >= until {
            return Err(std::io::ErrorKind::WouldBlock.into());
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// 对任意路径的锁文件用同一套锁原语不等待地试一次：Ok(Some) 拿到了，Ok(None) 别人持有。
/// 父目录不存在时先建好。给 home 里别的需要「同一时刻只有一个写者」的地方用；它们与安装
/// 锁共用原语，但锁的是各自的文件，互不相干。
pub(crate) fn try_lock_path(path: &Path) -> std::io::Result<Option<InstallLock>> {
    if let Some(parent) = path.parent() {
        crate::fsx::create_dir_all(parent)?;
    }
    let file = open_lock_file(path)?;
    Ok(try_lock_exclusive(&file)?.then_some(InstallLock { file }))
}

/// 打开（必要时新建）锁文件，读写；Windows 上共享模式见 [`lock_install_in`]。
fn open_lock_file(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};
        options.share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE);
    }
    options.open(path)
}

/// 不等待地加排他锁（永久兼容契约 C4 的 unix 原语）：Ok(true) 拿到了，Ok(false) 别人
/// 持有（或被信号打断，下一轮再试），Err 是加锁本身出错。
#[cfg(unix)]
fn try_lock_exclusive(file: &std::fs::File) -> std::io::Result<bool> {
    use std::os::unix::io::AsRawFd;
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(true);
    }
    let err = std::io::Error::last_os_error();
    match err.raw_os_error() {
        Some(libc::EWOULDBLOCK) | Some(libc::EINTR) => Ok(false),
        _ => Err(err),
    }
}

/// 不等待地加排他锁（永久兼容契约 C4 的 Windows 原语）：Ok(true) 拿到了，Ok(false) 别人
/// 持有，Err 是加锁本身出错。
#[cfg(windows)]
fn try_lock_exclusive(file: &std::fs::File) -> std::io::Result<bool> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::ERROR_LOCK_VIOLATION;
    use windows_sys::Win32::Storage::FileSystem::{
        LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY, LockFileEx,
    };
    // OVERLAPPED 全零：Offset 与 OffsetHigh 为 0，即从第 0 字节锁起；长度 u64::MAX 拆成
    // 低 32 位与高 32 位两个参数
    let mut overlapped: windows_sys::Win32::System::IO::OVERLAPPED = unsafe { std::mem::zeroed() };
    let locked = unsafe {
        LockFileEx(
            file.as_raw_handle() as isize,
            LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
            0,
            u32::MAX,
            u32::MAX,
            &mut overlapped,
        )
    };
    if locked != 0 {
        return Ok(true);
    }
    let err = std::io::Error::last_os_error();
    if err.raw_os_error() == Some(ERROR_LOCK_VIOLATION as i32) {
        Ok(false)
    } else {
        Err(err)
    }
}

/// 可安装版本的下限。0.1.x 与 0.2.0 起的版本互不兼容（控制协议、安装状态的位置、
/// `--version` 的格式都变了）：0.2.0 起的安装器驱动不了 0.1.x 的二进制，也联系不上
/// 0.1.x 的实例，装回 0.1.x 只会在交换二进制之后半途坏掉。所以在线安装与 `--from`
/// 都拒绝低于它的目标，`--allow-downgrade` 也越不过它。
///
/// 按「主.次.修订」比较：0.2.0 的预发布（alpha/beta/rc，以及开发构建自报的
/// 0.2.0-alpha.0）属于 0.2.0 这条线。按 semver 它们低于 0.2.0，直接比较 Version
/// 会把它们一起拒掉。
pub const INSTALL_FLOOR: semver::Version = semver::Version::new(0, 2, 0);

/// 目标版本不低于 [`INSTALL_FLOOR`]；低于时的错误说明为什么、怎样回到 0.1.x。
pub fn check_floor(target: &semver::Version) -> Result<(), String> {
    install_floor(target).map_err(|e| e.render(Lang::Zh))
}

/// 版本门槛不受语言影响；普通安装把版本参数写入自己的私有错误记录。
pub fn install_floor(target: &semver::Version) -> Result<(), FileIssue> {
    let floor = &INSTALL_FLOOR;
    if (target.major, target.minor, target.patch) >= (floor.major, floor.minor, floor.patch) {
        return Ok(());
    }
    Err(FileIssue::Floor {
        target: target.to_string(),
        floor: floor.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floor_rejects_0_1_x_and_admits_the_whole_0_2_0_line() {
        let v = |s: &str| semver::Version::parse(s).unwrap();
        for below in ["0.1.0", "0.1.99", "0.1.0-alpha.17", "0.0.1"] {
            let err = check_floor(&v(below)).unwrap_err();
            assert!(
                err.contains("0.1.x") && err.contains("--allow-downgrade"),
                "{err}"
            );
        }
        // 0.2.0 的预发布（含开发构建的 alpha.0 与最小的 0.2.0-0）都在线内
        for ok in [
            "0.2.0-0",
            "0.2.0-alpha.0",
            "0.2.0-rc.1",
            "0.2.0",
            "0.3.0",
            "1.0.0",
        ] {
            assert!(check_floor(&v(ok)).is_ok(), "{ok}");
        }
        assert!(check_floor(&v(env!("CARGO_PKG_VERSION"))).is_ok());
    }

    #[test]
    fn file_is_within_compares_directories_not_spellings() {
        let home = tempfile::tempdir().unwrap();
        let bin = home.path().join("bin");
        let bin2 = home.path().join("bin2");
        std::fs::create_dir_all(bin.join("nested")).unwrap();
        std::fs::create_dir_all(&bin2).unwrap();
        // 文件本身不必存在：比较的是它所在的目录
        assert!(file_is_within(&bin.join("aproxy.exe"), &bin));
        assert!(file_is_within(&bin.join("nested").join("x"), &bin));
        assert!(
            !file_is_within(&bin2.join("aproxy.exe"), &bin),
            "bin2 不在 bin 下"
        );
        assert!(!file_is_within(&home.path().join("aproxy.exe"), &bin));
        // `..` 绕一圈回来仍是同一个目录
        let detour = bin.join("..").join("bin").join("aproxy.exe");
        assert!(file_is_within(&detour, &bin), "{}", detour.display());
        // 目录不存在时按原样比较
        let gone = home.path().join("gone");
        assert!(file_is_within(&gone.join("x"), &gone));
    }

    /// Windows 的 8.3 短名：同一个目录的短名写法与长名写法必须判成同一个目录（CI 的临时
    /// 目录就是 `C:\Users\RUNNER~1\…`，经短名启动的进程镜像路径也带着短名）。
    #[cfg(windows)]
    #[test]
    fn file_is_within_treats_8dot3_short_names_as_the_same_directory() {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        let short = |p: &Path| -> PathBuf {
            let wide: Vec<u16> = p.as_os_str().encode_wide().chain(Some(0)).collect();
            let mut buf = vec![0u16; 1024];
            let len = unsafe {
                windows_sys::Win32::Storage::FileSystem::GetShortPathNameW(
                    wide.as_ptr(),
                    buf.as_mut_ptr(),
                    buf.len() as u32,
                )
            } as usize;
            assert!(
                len > 0 && len < buf.len(),
                "GetShortPathNameW 失败: {}",
                p.display()
            );
            PathBuf::from(std::ffi::OsString::from_wide(&buf[..len]))
        };
        let home = tempfile::tempdir().unwrap();
        // 名字长于 8 个字符、带连字符：卷上开着 8.3 短名生成时，它会得到 `APROXY~1` 式的短名
        let long_bin = home.path().join("aproxy-managed-bin");
        let long_other = home.path().join("aproxy-managed-bin-other");
        std::fs::create_dir_all(&long_bin).unwrap();
        std::fs::create_dir_all(&long_other).unwrap();
        let short_bin = short(&long_bin);
        let short_other = short(&long_other);
        if short_bin
            .as_os_str()
            .eq_ignore_ascii_case(long_bin.as_os_str())
        {
            eprintln!(
                "跳过：{} 所在的卷没有生成 8.3 短名（GetShortPathNameW 返回的就是长名），造不出短名写法",
                long_bin.display()
            );
            return;
        }
        let ctx = format!(
            "长名 {}，短名 {}；另一个目录 长名 {}，短名 {}",
            long_bin.display(),
            short_bin.display(),
            long_other.display(),
            short_other.display()
        );
        for file_dir in [&long_bin, &short_bin] {
            for dir in [&long_bin, &short_bin] {
                assert!(
                    file_is_within(&file_dir.join("aproxy.exe"), dir),
                    "{} 应在 {} 里（{ctx}）",
                    file_dir.display(),
                    dir.display()
                );
            }
            for other in [&long_other, &short_other] {
                assert!(
                    !file_is_within(&other.join("aproxy.exe"), file_dir),
                    "{} 不应在 {} 里（{ctx}）",
                    other.display(),
                    file_dir.display()
                );
            }
        }
    }
}
