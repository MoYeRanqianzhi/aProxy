//! Home-scoped registry records and liveness probes. No function writes a record,
//! removes a stale entry or terminates a process. Discovery exposes only the restore configuration path,
//! never launch overrides or the captured environment.
use crate::{
    client::ipc_ping_in,
    wire::{InstanceRecord, InstanceStatus},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};
/// 现在的时刻（Unix 毫秒）。记录、快照与事件里的时刻都用这个单位。
pub fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 从监听地址提取端口（IPC 端点名、日志/注册文件名共用）
pub fn port_of(listen_addr: &str) -> &str {
    listen_addr.rsplit(':').next().unwrap_or("unknown")
}

/// 实例记录的 schema。读到更高的 schema 时不解读内容，那份记录算读不懂。
pub const INSTANCE_SCHEMA: u32 = 1;

/// 实例记录在盘上的形状：`schema` 在前，其余就是 [`InstanceRecord`]。
#[derive(Serialize, Deserialize)]
pub struct InstanceFile {
    pub schema: u32,
    #[serde(flatten)]
    pub record: InstanceRecord,
}

/// 解析一份实例记录的文本。读不懂（不是这个形状，或 schema 比本版本高）→ None。
pub fn parse_instance_file(text: &str) -> Option<InstanceRecord> {
    serde_json::from_str::<InstanceFile>(text)
        .ok()
        .filter(|file| file.schema <= INSTANCE_SCHEMA)
        .map(|file| file.record)
}

/// 读一份实例记录文件，读不懂时说明是哪一种读不懂（读不出、不是这个形状、schema 比本版本
/// 高）。只想知道「读不读得懂」的调用方用 [`parse_instance_file`]。
pub fn read_instance_record(path: &std::path::Path) -> Result<InstanceRecord, Unreadable> {
    read_record::<InstanceFile>(path, INSTANCE_SCHEMA).map(|file| file.record)
}

/// 注册表中登记的全部实例记录（只读 `*.instance.json`，不做 IPC 探活；读不懂的跳过），按 pid
/// 升序。看门狗选举的输入：存活与身份由调用方用进程级手段（pid + 记录里
/// 的启动标记比对现算的标记）判定——实例 IPC 不可达恰恰是需要
/// 看护的信号，不能作为「死」的依据参与选举。
pub fn registry_instances_in(run_dir: &std::path::Path) -> Vec<InstanceRecord> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(run_dir) else {
        return out;
    };
    for entry in entries.flatten() {
        if !entry
            .file_name()
            .to_string_lossy()
            .ends_with(INSTANCE_FILE_SUFFIX)
        {
            continue;
        }
        if let Ok(content) = record_text(&entry.path())
            && let Some(info) = parse_instance_file(&content)
        {
            out.push(info);
        }
    }
    out.sort_by_key(|i| i.pid);
    out
}

/// 注册表中登记的全部实例 PID（去重升序，只读文件不探活）。
pub fn registry_pids_in(run_dir: &std::path::Path) -> Vec<u32> {
    let mut out: Vec<u32> = registry_instances_in(run_dir)
        .iter()
        .map(|i| i.pid)
        .collect();
    out.dedup();
    out
}

/// 读取单个端口的注册表记录（只读，不探活；不存在/读不懂 → None）。
pub fn read_instance_file_in(run_dir: &std::path::Path, port: &str) -> Option<InstanceRecord> {
    let content = record_text(&instance_file_path_in(run_dir, port)).ok()?;
    parse_instance_file(&content)
}

/// 按端口读注册表记录，且记录的进程经「pid + 创建时间」核验仍在——不应答
/// 控制通道的实例（多半挂死）只能这样定位。
pub fn live_record_in(run_dir: &Path, port: &str) -> Option<InstanceRecord> {
    read_instance_file_in(run_dir, port)
        .filter(|r| record_identity(r.pid, &r.start) == RecordIdentity::Alive)
}

/// 一个端口上有没有本 home 的实例进程：应答控制通道的，或不应答但进程经核验仍在的。
pub async fn port_has_instance_in(run_dir: &Path, port: &str) -> bool {
    ipc_ping_in(run_dir, port).await.is_ok() || live_record_in(run_dir, port).is_some()
}

/// 实例记录的文件名后缀：文件是 `<端口>.instance.json`。
pub const INSTANCE_FILE_SUFFIX: &str = ".instance.json";

/// Locate an instance record in the supplied run directory.
pub fn instance_file_path_in(run_dir: &std::path::Path, listen_addr: &str) -> PathBuf {
    run_dir.join(format!("{}{INSTANCE_FILE_SUFFIX}", port_of(listen_addr)))
}

/// 注册表普查结果：应答的实例，与「进程仍在、却不应答控制通道」的实例。
pub struct InstanceSurvey {
    /// IPC ping 成功：ping 应答里的实时状态
    pub responsive: Vec<InstanceStatus>,
    /// ping 失败但记录的进程经「pid + 创建时间」核验仍在（多为挂死）：信息
    /// 取自注册表记录（启动时刻的快照）。aproxy 无法经 IPC 优雅停止它们，只能
    /// `stop --force`（终止前同样核验身份）
    pub unresponsive: Vec<InstanceRecord>,
}

/// Return only responsive records from the supplied registry.
pub async fn list_instances_in(dir: &std::path::Path) -> Vec<InstanceStatus> {
    survey_instances_in(dir).await.responsive
}

/// 逐条读注册表、IPC ping 验活。只读，不删任何文件。ping 失败时按「pid + 创建时间」
/// 核验记录的进程：仍在 → 归入 unresponsive；已退出、pid 已被复用、或记录没登记创建时间
/// （证明不了身份）→ 不列出，残留的记录留给清理（`collect_garbage_in`）。
/// **不应答 ≠ 已死**：挂死的实例进程还在，看门狗的挂死接管要靠这条记录核验身份，
/// `stop --force` 也要靠它定位。读不懂的记录跳过。
pub async fn survey_instances_in(dir: &std::path::Path) -> InstanceSurvey {
    let mut out = Vec::new();
    let mut unresponsive = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return InstanceSurvey {
            responsive: out,
            unresponsive,
        };
    };
    for entry in entries.flatten() {
        if !entry
            .file_name()
            .to_string_lossy()
            .ends_with(INSTANCE_FILE_SUFFIX)
        {
            continue;
        }
        let info = record_text(&entry.path())
            .ok()
            .and_then(|c| parse_instance_file(&c));
        // 读不懂的记录（损坏，或更新的版本写的）定位不了端口，跳过
        let Some(info) = info else {
            continue;
        };
        let port = port_of(&info.listen_addr).to_string();
        match ipc_ping_in(dir, &port).await {
            // ping 应答携带实例的实时状态（含最近活动时刻）——实例记录
            // 是启动时刻的快照，闲置判定/展示必须用实时值
            Ok(live) if live.instance.pid == info.pid => out.push(live),
            // 应答的不是记录里的进程。新实例先建端点、后写记录，记录可能正被
            // 重写——重读一次再比。仍对不上时，记录的进程若还在就按无响应
            // 列出（它不在自己的端点上应答）；否则不列出、也不删：此刻删除
            // 可能恰好删掉应答者刚写下的记录
            Ok(live) => {
                if read_instance_file_in(dir, &port).is_some_and(|f| f.pid == live.instance.pid) {
                    out.push(live);
                } else if record_identity(info.pid, &info.start) == RecordIdentity::Alive {
                    unresponsive.push(info);
                }
            }
            Err(_) => {
                if record_identity(info.pid, &info.start) == RecordIdentity::Alive {
                    unresponsive.push(info);
                }
            }
        }
    }
    out.sort_by(|a, b| a.instance.listen_addr.cmp(&b.instance.listen_addr));
    unresponsive.sort_by(|a, b| a.listen_addr.cmp(&b.listen_addr));
    InstanceSurvey {
        responsive: out,
        unresponsive,
    }
}

/// 只读检索注册表：是否存在 pid 匹配的实例记录。
/// respawn 的就绪判定专用：就绪只需「新 pid 的注册记录已出现」，读文件即可，无需 IPC。
pub fn registry_contains_pid_in(dir: &std::path::Path, pid: u32) -> bool {
    registry_find_pid_in(dir, pid).is_some()
}

/// 只读检索注册表：取 pid 匹配的实例记录（无副作用，理由同上）。
/// 重拉/恢复/重启按 spawn 返回的 pid 定位新实例，从记录里读它**实际**监听
/// 的地址——配置可能已改端口（或 listen 端口为 0 由系统分配），新实例未必
/// 落在原端口。
pub fn registry_find_pid_in(dir: &std::path::Path, pid: u32) -> Option<InstanceRecord> {
    let entries = std::fs::read_dir(dir).ok()?;
    entries.flatten().find_map(|entry| {
        if !entry
            .file_name()
            .to_string_lossy()
            .ends_with(INSTANCE_FILE_SUFFIX)
        {
            return None;
        }
        record_text(&entry.path())
            .ok()
            .and_then(|c| parse_instance_file(&c))
            .filter(|info| info.pid == pid)
    })
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
// 3. 启动标记：实例注册时把自己的启动标记写进实例记录（InstanceRecord.start），看护者
//    记录同理。pid 被复用后新进程的标记必然不同，比对它就能把「同一个进程」钉死，且
//    不需要对方应答（挂死实例同样可判）。

/// 一条记录（pid + 登记的启动标记）在当前系统上的身份核验结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordIdentity {
    /// 记录的进程仍在运行：现算的启动标记与登记值一致
    Alive,
    /// 该 pid 当前有在运行的进程，但启动标记与登记值不符：pid 已被系统
    /// 回收后复用，记录的进程早已死亡
    Reused,
    /// 该 pid 当前没有在运行的进程（不存在、已退出未回收的 zombie、或只剩
    /// 被句柄维持的已退出进程对象）
    Gone,
}

/// 核验记录身份：`recorded_start` 为记录登记的启动标记。
/// 「已退出但进程对象仍在」（unix zombie、Windows 被句柄维持的已退出对象）
/// 一律算 Gone——它们的标记仍算得出且与登记一致，但进程已死，不能计入
/// 选举的存活集合，也不该被收养或处决。
///
/// 空的登记值不是可比对的锚点（平台算不出标记时登记的就是空串）：这样的
/// 记录证明不了任何进程是它，同样算 Gone——不计入选举、不收养、不处决。
pub fn record_identity(pid: u32, recorded_start: &str) -> RecordIdentity {
    if recorded_start.is_empty() {
        return RecordIdentity::Gone;
    }
    let Some(actual) = crate::ledger::start_marker(pid) else {
        return RecordIdentity::Gone;
    };
    if process_exited(pid) == Some(true) {
        return RecordIdentity::Gone;
    }
    if actual == recorded_start {
        RecordIdentity::Alive
    } else {
        RecordIdentity::Reused
    }
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
    process_platform::process_exited(pid)
}

mod process_platform {
    #[cfg(windows)]
    /// 进程是否已退出：用零超时等待进程句柄，不把合法退出码 259 当成存活。即使进程
    /// 对象被其他句柄（Rust Child 等）维持，是否已发信号也已确定——终止确认的
    /// 唯一可靠判据（start_time 查询在对象被引用期间会误报存活）。
    /// 只有无此 pid 才算退出；权限等其它错误不是死亡证据。
    pub fn process_exited(pid: u32) -> Option<bool> {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
        };
        unsafe {
            let handle = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
            if handle == 0 {
                let error = std::io::Error::last_os_error();
                return (error.raw_os_error()
                    == Some(windows_sys::Win32::Foundation::ERROR_INVALID_PARAMETER as i32))
                .then_some(true);
            }
            let wait = WaitForSingleObject(handle, 0);
            let _ = CloseHandle(handle);
            match wait {
                windows_sys::Win32::Foundation::WAIT_OBJECT_0 => Some(true),
                windows_sys::Win32::Foundation::WAIT_TIMEOUT => Some(false),
                _ => None,
            }
        }
    }
    /// 进程是否已退出：/proc/<pid>/stat 的进程态——'Z'（zombie，已退出未
    /// 收割）与文件不存在（已收割）都算已退出。macOS 无 /proc 回退
    /// kill(pid,0)：ESRCH = 已退出；EPERM = 活着（权限拒绝但存在）。
    #[cfg(all(unix, not(target_os = "macos")))]
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

    /// macOS 没有 procfs；信号 0 只查询正 pid 的存在性。这里只认 ESRCH 为已退出，
    /// EPERM 说明进程仍在；未收割的僵尸要等父进程收割之后才能观察到退出。
    #[cfg(target_os = "macos")]
    pub fn process_exited(pid: u32) -> Option<bool> {
        let pid = i32::try_from(pid).ok().filter(|p| *p > 0)?;
        if unsafe { libc::kill(pid, 0) } == 0 {
            return Some(false);
        }
        match std::io::Error::last_os_error().raw_os_error() {
            Some(libc::ESRCH) => Some(true),
            Some(libc::EPERM) => Some(false),
            _ => None,
        }
    }
}

/// 本版本写出、也是本版本能读的最高记录格式版本。
pub const SCHEMA: u32 = 1;

/// 期望状态记录的文件名后缀：`<端口>.restore.json`。
pub const RESTORE_SUFFIX: &str = ".restore.json";
/// 一条记录读不懂的原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unreadable {
    /// 读不出文件
    Io(String),
    /// 不是这种记录应有的 JSON
    Parse(String),
    /// `schema` 高于本版本认识的：写下它的是更新的版本
    SchemaTooNew(u64),
}

impl std::fmt::Display for Unreadable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "cannot read: {e}"),
            Self::Parse(e) => write!(f, "not a valid record: {e}"),
            Self::SchemaTooNew(schema) => {
                write!(f, "written by a newer aproxy (schema {schema})")
            }
        }
    }
}

/// 解析一条带 `schema` 的记录；`newest` 是本版本读得懂的最高 schema（每种记录各有自己的）。
/// 先只取 `schema`：更高的版本可能改了别的字段的形状，直接按本版本的结构解析会把「太新」
/// 误报成「损坏」。
pub fn parse_record<T: serde::de::DeserializeOwned>(
    text: &str,
    newest: u32,
) -> Result<T, Unreadable> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| Unreadable::Parse(e.to_string()))?;
    match value.get("schema").and_then(serde_json::Value::as_u64) {
        Some(schema) if schema > u64::from(newest) => return Err(Unreadable::SchemaTooNew(schema)),
        Some(_) => {}
        None => return Err(Unreadable::Parse("missing schema".to_string())),
    }
    serde_json::from_value(value).map_err(|e| Unreadable::Parse(e.to_string()))
}

// Validate the opened handle, not just the directory entry: a FIFO swapped
// in between lookup and open must not block the discovery worker either.
fn record_text(path: &Path) -> std::io::Result<String> {
    use std::io::Read;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    if !path.symlink_metadata()?.is_file() {
        return Err(std::io::Error::other("not a regular record file"));
    }
    let mut file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::other("not a regular record file"));
    }
    let mut text = String::new();
    file.read_to_string(&mut text)?;
    Ok(text)
}

#[cfg(unix)]
#[test]
fn non_regular_record_is_an_io_error_not_bad_json() {
    assert!(matches!(
        read_record::<InstanceFile>(Path::new("/dev/null"), 1),
        Err(Unreadable::Io(_))
    ));
}

#[cfg(unix)]
#[test]
fn simple_registry_readers_also_reject_symlinks() {
    let dir = tempfile::tempdir().unwrap();
    let ordinary = dir.path().join("payload");
    let record = serde_json::json!({"schema":1,"pid":71,"start":"s","instance_id":"r","version":"v","listen_addr":"127.0.0.1:80","base_url":"","started_at":1,"foreground":true});
    std::fs::write(&ordinary, record.to_string()).unwrap();
    std::os::unix::fs::symlink(&ordinary, instance_file_path_in(dir.path(), "80")).unwrap();
    assert!(read_instance_file_in(dir.path(), "80").is_none());
    assert!(registry_instances_in(dir.path()).is_empty());
    assert!(registry_find_pid_in(dir.path(), 71).is_none());
}

/// 读一条带 `schema` 的记录文件，见 [`parse_record`]。
pub fn read_record<T: serde::de::DeserializeOwned>(
    path: &Path,
    newest: u32,
) -> Result<T, Unreadable> {
    let text = record_text(path).map_err(|e| Unreadable::Io(e.to_string()))?;
    parse_record(&text, newest)
}

/// Read-only evidence of a pending restore. Private launch settings are not
/// returned, even when a file contains environment or command-line credentials.
/// ```compile_fail
/// use aproxy_ipc::registry::{RestoreRecord, InstanceSpec, Overrides};
/// ```
#[derive(Debug, PartialEq, Eq)]
pub struct RestorePresence {
    /// Configuration to check for existence, if the launch had one.
    pub config_path: Option<PathBuf>,
}

/// Read the canonical restore record, validating known fields but projecting only
/// discovery data. Unknown additive fields have no bearing on restoration.
pub fn read_restore_presence_in(
    dir: &Path,
    port: u16,
) -> Result<Option<RestorePresence>, Unreadable> {
    #[derive(Deserialize)]
    struct Header {
        spec: Spec,
        #[serde(rename = "written_at")]
        _written: u64,
    }
    #[derive(Deserialize)]
    struct Spec {
        #[serde(rename = "schema")]
        _schema: u32,
        config_path: Option<String>,
        #[serde(rename = "log_path")]
        _log: String,
        #[serde(rename = "pinned_port")]
        _port: Option<u16>,
        #[serde(default, rename = "env")]
        _env: BTreeMap<String, Option<String>>,
        #[serde(default)]
        overrides: serde_json::Map<String, serde_json::Value>,
    }
    let Some(header): Option<Header> =
        read_optional_record(&restore_path_in(dir, &port.to_string()), SCHEMA)?
    else {
        return Ok(None);
    };
    for name in ["base_url", "listen_addr", "proxy", "api_key", "log_file"] {
        if header
            .spec
            .overrides
            .get(name)
            .is_some_and(|v| !v.is_null() && !v.is_string())
        {
            return Err(Unreadable::Parse("invalid restore override".into()));
        }
    }
    Ok(Some(RestorePresence {
        config_path: header.spec.config_path.map(PathBuf::from),
    }))
}

/// 期望状态记录的路径：`<run_dir>/<端口>.restore.json`。
pub fn restore_path_in(run_dir: &Path, port: &str) -> PathBuf {
    run_dir.join(format!("{port}{RESTORE_SUFFIX}"))
}

/// 看护者记录的 schema。读到更高的 schema 时不解读内容（见 `unreadable`）。
pub const RECORD_SCHEMA: u32 = 1;
/// 记录里保留最近多少次重拉。
pub const RESTARTS_KEPT: usize = 20;
/// 重拉的起因：进程自己没了（崩溃、被信号结束、被强杀）。
pub const KIND_CRASH: &str = "crash";
/// 重拉的起因：进程还在、却不再调度（心跳过期且控制端点不应答），被看护者结束。
pub const KIND_HANG: &str = "hang";
/// 「最近一次扫描」早于这么久（秒）之前，才可能算看护者假死，不管扫描周期设得多短。
/// 看护者的一个步进周期里可能要等重拉出来的实例就绪（每个最多
/// 等到启动就绪时限），窗口比这短的话，正忙着重拉的看护者会被当成假死、
/// 在重拉途中被接管者结束。
pub const FRESH_FLOOR_SECS: u64 = 90;

/// 一次重拉。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct RestartEvent {
    /// 实例的端口
    pub port: u16,
    /// 重拉结束的时刻（Unix 毫秒）
    pub at: u64,
    /// 起因，[`KIND_CRASH`] 或 [`KIND_HANG`]（词表只增）
    pub kind: String,
    /// 实例有没有起来
    pub ok: bool,
}

/// 看护者记录（`run/watchdog.json`，一行 JSON）：谁在任、多久扫一次、最近一次扫描在什么
/// 时候、最近重拉过谁。在任的看护者每个步进周期整份重写它；别的进程据此判断有没有看护者
/// 在任（见 `watchdog_in_effect_in`），status 与界面据此展示。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct WatchdogRecord {
    /// 恒为 [`RECORD_SCHEMA`]
    pub schema: u32,
    pub pid: u32,
    /// 看护者进程的启动标记（台账用的同一个，见 [`crate::ledger::start_marker`]）：pid
    /// 被别的进程复用之后标记必然不同。读不到标记的平台记空串，这时只能按 pid 判断。
    pub start: String,
    /// 看护者自己的版本（不带 v）
    pub version: String,
    /// 扫描周期（秒）。读者按它算新鲜窗口，不按自己的 settings：看护者启动之后周期不变，
    /// 而 settings 可能已经被改过
    pub heartbeat_secs: u64,
    /// 看护者开始看护的时刻（Unix 毫秒）
    pub created_at: u64,
    /// 最近一次步进周期结束的时刻（Unix 毫秒）
    pub last_scan_at: u64,
    /// 最近 [`RESTARTS_KEPT`] 次重拉，旧的在前
    #[serde(default)]
    pub restarts: Vec<RestartEvent>,
}

impl WatchdogRecord {
    /// 本进程的记录：刚开始看护，还没重拉过谁。
    pub fn for_current_process(heartbeat_secs: u64) -> Self {
        let pid = std::process::id();
        let now = now_millis();
        Self {
            schema: RECORD_SCHEMA,
            pid,
            start: crate::ledger::start_marker(pid).unwrap_or_default(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            heartbeat_secs,
            created_at: now,
            last_scan_at: now,
            restarts: Vec::new(),
        }
    }

    /// 两份记录是不是同一个看护者进程写的。
    pub fn same_owner(&self, other: &Self) -> bool {
        self.pid == other.pid && self.start == other.start
    }

    /// 写下这份记录的进程是否还在：pid 现在的启动标记与记录一致，并且没有退出。记录里
    /// 没有标记（平台读不到）时只看这个 pid 上的进程有没有退出。
    pub fn owner_alive(&self) -> bool {
        if process_exited(self.pid) == Some(true) {
            return false;
        }
        self.start.is_empty()
            || crate::ledger::start_marker(self.pid).as_deref() == Some(self.start.as_str())
    }

    /// 最近一次扫描是否还算新鲜：距今不超过三个扫描周期（且不短于 [`FRESH_FLOOR_SECS`]）。
    pub fn is_fresh(&self, now_ms: u64) -> bool {
        let window_secs = self.heartbeat_secs.saturating_mul(3).max(FRESH_FLOOR_SECS);
        now_ms.saturating_sub(self.last_scan_at) <= window_secs.saturating_mul(1000)
    }
}

/// 看护者记录的路径（run_dir 注入，测试绝不读写真实 run/）
pub fn record_path_in(run_dir: &Path) -> PathBuf {
    run_dir.join("watchdog.json")
}

/// Read an optional schema-bearing document. Only disappearance means absence;
/// a corrupt or newer document still represents an obligation in this home.
pub fn read_optional_record<T: serde::de::DeserializeOwned>(
    path: &Path,
    schema: u32,
) -> Result<Option<T>, Unreadable> {
    // 只有路径不存在才表示没有恢复义务；权限等错误不能把尚在的记录变成空。
    let text = match record_text(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(Unreadable::Io(e.to_string())),
    };
    parse_record(&text, schema).map(Some)
}
