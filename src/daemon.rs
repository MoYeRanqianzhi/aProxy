//! 守护进程编排：后台启动、实例注册表、IPC 控制。
//!
//! 进程模型：`aproxy`（默认）= 后台启动——父进程做预检与就绪等待，
//! 实际服务由分离的子进程（`--daemon-child`）承载。子进程以 CREATE_NO_WINDOW
//! 启动、不继承控制台，终端关闭/常规内存清理不会连带杀掉守护进程。
//!
//! IPC（铁律：控制通道绝不占用代理端口，代理端口完全用于透传——避免控制
//! 路径与客户端请求路径巧合重叠造成严重 bug）：每个实例一条 Windows 命名
//! 管道 `\\.\pipe\aproxy-<port>`（unix 为 `~/.aproxy/run/<port>.sock`），
//! 承载 `ping`/`shutdown` 控制。端口冲突的两种情况由此区分：
//! - 管道 ping 通 → 该端口已有 aProxy 实例在运行；
//! - 管道不通但 TCP bind 失败 → 端口被其他程序占用。
//!
//! 实例注册表：`~/.aproxy/run/<port>.pid`（JSON），仅供 `status`/`stop` 枚举
//! 实例；存活一律以 IPC ping 为准，不信任注册文件与 pid 本身。

use serde::{Deserialize, Serialize};
use std::{io, path::PathBuf, sync::Arc, time::Duration};

/// 实例注册表目录：`~/.aproxy/run/`。
/// `APROXY_RUN_DIR` 环境变量可整体改指别处——集成测试用它与 tempdir 隔离
/// （守护/看护子进程经 spawn_detached 继承环境），高级用户亦可借此自定义
/// 运行数据位置。
pub fn run_dir() -> PathBuf {
    if let Ok(d) = std::env::var("APROXY_RUN_DIR") {
        return PathBuf::from(d);
    }
    crate::config::config_dir().join("run")
}

/// 守护进程日志目录：`~/.aproxy/logs/`
pub fn logs_dir() -> PathBuf {
    crate::config::config_dir().join("logs")
}

/// spool 临时文件目录基址：`~/.aproxy/spool/<端口>/`。每实例独立子目录，
/// 启动时清空自己的子目录即可回收崩溃残留，互不干扰。
pub fn spool_dir_for(port: &str) -> PathBuf {
    crate::config::config_dir().join("spool").join(port)
}

/// 清空实例的 spool 目录（启动时调用）：删除崩溃/强杀残留的 *.spooltmp。
/// 目录不存在视为首次运行（创建之）。仅在 bind 监听端口之前调用——此后
/// 该端口目录归本进程独占，运行中不清理（清理职责在临时文件的使用方）。
pub fn clean_spool_dir(port: &str) {
    let dir = spool_dir_for(port);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::warn!(error = %e, dir = %dir.display(), "spool 目录创建失败（磁盘缓存将不可用，回退纯内存）");
        return;
    }
    match std::fs::read_dir(&dir) {
        Ok(entries) => {
            for entry in entries.flatten() {
                // 只删本实例的 spool 临时文件；意外出现的其他内容保留（不扩大删除面）
                if entry
                    .path()
                    .extension()
                    .is_some_and(|ext| ext == "spooltmp")
                    && let Err(e) = std::fs::remove_file(entry.path())
                {
                    tracing::warn!(error = %e, path = %entry.path().display(), "spool 残留清理失败");
                }
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, dir = %dir.display(), "spool 目录读取失败");
        }
    }
}

/// 实例的 IPC 端点：windows 为命名管道名，unix 为 UDS 路径。
/// 端口号唯一区分实例（同端口=同实例；多实例的监听端口必然互不相同）。
pub fn endpoint_for(port: &str) -> String {
    #[cfg(windows)]
    {
        format!(r"\\.\pipe\aproxy-{port}")
    }
    #[cfg(unix)]
    {
        run_dir().join(format!("{port}.sock")).display().to_string()
    }
}

/// 从监听地址提取端口（IPC 端点名、日志/注册文件名共用）
pub fn port_of(listen_addr: &str) -> &str {
    listen_addr.rsplit(':').next().unwrap_or("unknown")
}

/// 运行实例的信息（注册表落盘内容，也是 IPC ping 的响应载荷）
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct InstanceInfo {
    pub pid: u32,
    pub version: String,
    pub listen_addr: String,
    pub config_path: String,
    pub base_url: String,
    /// 启动时刻（Unix 秒）
    pub started_at: u64,
    /// 最近一次收到客户端请求的时刻（Unix 秒）。0 = 实例未上报（旧版本
    /// 或注册时快照）——调用方按「未知，视为非闲置」处理。
    #[serde(default)]
    pub last_activity_secs: u64,
    // ---- 以下为 IPC v2 观测字段（全部 serde default：旧实例/旧注册表文件
    // ---- 无这些字段时读默认值，双向兼容）----
    /// 实例自身的协议版本（与 IpcResponse::proto 双保险，供消费侧单独判定）
    #[serde(default)]
    pub proto_version: u32,
    /// 实例累计转发的客户端请求数（含重试中未决的；IPC 探测时实时读取）
    #[serde(default)]
    pub requests_total: u64,
    /// 累计上游重试次数（含首轮后的全部重试尝试）
    #[serde(default)]
    pub retries_total: u64,
    /// 最近一次上游失败的简短摘要（已打码，可能为 None = 从未失败）
    #[serde(default)]
    pub last_error: Option<String>,
    /// last_error 的发生时刻（Unix 秒；0 = 无错误记录）
    #[serde(default)]
    pub last_error_at: u64,
}

/// 当前 IPC 协议版本。协议变更（增字段/增 op）不递增——serde default/忽略
/// 未知字段保证字段级双向兼容；只有破坏性变更（语义不兼容）才递增此号，
/// 消费方按版本降级。
pub const IPC_PROTO_VERSION: u32 = 2;

/// v1（alpha.5 及更早）的协议版本号：旧实例的响应不带 proto 字段，
/// 读出默认值 1（serde default 的目标）。
pub const IPC_PROTO_V1: u32 = 1;

/// IPC 请求。framing：一行 JSON + `\n`。
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum IpcRequest {
    /// 实例识别（status / start 预检）
    Ping,
    /// 优雅停止
    Shutdown,
    /// 观测数据查询（IPC v2）：携带实时计数器与最近错误。旧实例收到此 op
    /// 反序列化失败（未知 tag）——客户端据此探测对端能力并降级为仅 Ping。
    Stats,
}

/// IPC 响应：一行 JSON + `\n`。
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct IpcResponse {
    pub ok: bool,
    /// 实例信息（ping/shutdown 成功时都携带，便于展示）
    #[serde(default)]
    pub info: Option<InstanceInfo>,
    /// 响应方的协议版本。v1 实例（alpha.5 及更早）不写此字段——serde 读为
    /// 1；客户端据此判定对端能力（proto=1 无 Stats/观测字段）。
    #[serde(default = "ipc_proto_v1_default")]
    pub proto: u32,
}

fn ipc_proto_v1_default() -> u32 {
    IPC_PROTO_V1
}

/// 探测端口上是否有 aProxy 实例（纯 IPC，不触碰任何 TCP 端口）。
/// Ok(info) = 实例在运行；Err = 端点上没有可识别的 aProxy（无实例，
/// 或该端口被其他程序占用——由调用方结合 TCP bind 结果区分这两种情况）。
///
/// 判死门槛：连续 3 次（间隔 200ms）都拿不到有效响应才算 Err。单次 ping
/// 可能因 Windows 命名管道瞬时 busy（serve 重建监听实例的零监听窗口）或
/// 3 秒超时等瞬态原因失败；调用方（list_instances 的注册清理、stop 的
/// 已停止判定）会把 Err 当作「实例已死」处理，误判会删掉活实例的注册记录。
pub async fn ipc_ping(port: &str) -> Result<InstanceInfo, String> {
    const DEAD_AFTER: usize = 3;
    const RETRY_INTERVAL: Duration = Duration::from_millis(200);
    let mut last_err = String::new();
    for attempt in 0..DEAD_AFTER {
        if attempt > 0 {
            tokio::time::sleep(RETRY_INTERVAL).await;
        }
        match ipc_request(port, &IpcRequest::Ping).await {
            Ok(resp) if resp.ok => return resp.info.ok_or_else(|| "实例响应缺少信息".to_string()),
            Ok(_) => last_err = "实例返回失败".to_string(),
            Err(e) => last_err = e,
        }
    }
    Err(last_err)
}

/// 发送 IPC 请求并等待响应（3 秒超时）。
pub async fn ipc_request(port: &str, req: &IpcRequest) -> Result<IpcResponse, String> {
    ipc_request_to(&endpoint_for(port), req).await
}

/// 按显式端点发送 IPC 请求并等待响应（3 秒超时）。
/// 供绕过 `endpoint_for` 解析的场景使用：unix 的 UDS 路径在 run_dir 里，
/// 守护以隔离 `APROXY_RUN_DIR` 运行时，同进程的库调用方（测试）须按守护
/// 实际的 socket 路径寻址；Windows 管道名全局唯一，不受 run_dir 影响。
pub async fn ipc_request_to(endpoint: &str, req: &IpcRequest) -> Result<IpcResponse, String> {
    let req_line = serde_json::to_string(req).expect("序列化 IPC 请求失败");
    let fut = imp::exchange(endpoint, &req_line);
    match tokio::time::timeout(Duration::from_secs(3), fut).await {
        Ok(Ok(line)) => {
            serde_json::from_str(&line).map_err(|e| format!("{endpoint}: 响应解析失败 {e}"))
        }
        Ok(Err(e)) => Err(format!("{endpoint}: {e}")),
        Err(_) => Err(format!("{endpoint}: 请求超时")),
    }
}

/// 启动实例的 IPC 控制服务（每实例一条独立端点，随进程退出而终止）。
/// 收到 shutdown 时置位 `on_shutdown`（watch bool），由服务主循环执行优雅退出。
/// `port` 收 owned 值：调用方以 tokio::spawn 运行本 future，参数不能借用。
/// `last_activity_secs`：代理层的活动时间戳共享原子，ping 实时读取。
pub async fn serve_ipc(
    port: String,
    on_shutdown: tokio::sync::watch::Sender<bool>,
    info: InstanceInfo,
    stats: Arc<IpcStats>,
) -> io::Result<()> {
    imp::serve(endpoint_for(&port), on_shutdown, info, stats).await
}

/// 实例的实时观测数据源：代理热路径写入，IPC 响应读取。
/// 全部 Relaxed——计数器只用于展示与告警，无跨线程因果序需求。
#[derive(Debug, Default)]
pub struct IpcStats {
    /// 最近一次收到客户端请求的时刻（即 AppState::last_activity_secs，
    /// 同一 Arc 由 serve_forever 组装进来）
    pub last_activity_secs: Arc<std::sync::atomic::AtomicU64>,
    /// 累计收到客户端请求数
    pub requests_total: std::sync::atomic::AtomicU64,
    /// 累计上游重试次数（首轮之后的所有尝试）
    pub retries_total: std::sync::atomic::AtomicU64,
    /// 最近一次上游失败的简短摘要（打码后的短串；None = 从未失败）
    pub last_error: std::sync::Mutex<Option<(String, u64)>>,
}

impl IpcStats {
    /// 记录一次上游失败（覆盖式：只保留最近一次）。`error` 须已打码。
    pub fn record_error(&self, error: &str) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        // 截断到 200 字符：错误摘要进 IPC 响应与 status 展示，防止上游返回
        // 超长错误文本把响应行撑爆
        let mut brief: String = error.chars().take(200).collect();
        if brief.is_empty() {
            brief = "未知错误".to_string();
        }
        if let Ok(mut slot) = self.last_error.lock() {
            *slot = Some((brief, now));
        }
    }
}

/// 注册表中登记的全部实例 PID（只读 *.pid 文件，不做 IPC 探活）。
/// 看门狗选举的输入：探活由调用方用自己的进程级手段完成（实例 IPC 不可达
/// 恰恰是需要看护的信号，不能作为「死」的依据参与选举）。
pub fn registry_pids_in(run_dir: &std::path::Path) -> Vec<u32> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(run_dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("pid") {
            continue;
        }
        if let Ok(content) = std::fs::read_to_string(&path)
            && let Ok(info) = serde_json::from_str::<InstanceInfo>(&content)
        {
            out.push(info.pid);
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// 强制终止实例进程（`--force`）：跳过 IPC 优雅关闭直接 TerminateProcess。
/// 终止前验证进程镜像名——PID 复用下杀错进程不可逆，宁可拒绝执行。
/// 返回 Err 的信息已含原因，调用方直接展示。
pub fn force_terminate(pid: u32) -> Result<(), String> {
    imp::terminate_process(pid)
}

/// 向 startup.log 追加一行（带 unix 时间戳前缀）。看门狗的 crashloop 放弃、
/// 假死接管等重大事件写这里——用户排查「实例为什么没被拉起」的第一个入口。
pub fn append_startup_log(line: &str) {
    let _ = std::fs::create_dir_all(logs_dir());
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

/// 等待实例退出（连续 2 轮探测都失败才视为已退出），超时返回 false。
/// 单轮失败可能是 IPC 通道瞬态问题（管道 busy / 超时），据此上报「已停止」
/// 会掩盖仍存活的实例。
pub async fn wait_until_gone(port: &str, timeout: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut consecutive_failures = 0usize;
    loop {
        if ipc_ping(port).await.is_err() {
            consecutive_failures += 1;
            if consecutive_failures >= 2 {
                return true;
            }
        } else {
            consecutive_failures = 0;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// 以分离进程重新执行自身承载服务。
///
/// CREATE_NO_WINDOW：不继承父控制台、无窗口——关闭终端、控制台进程树清理
/// 都不会连带杀掉守护进程（这正是前台进程被「常规清理」关掉的根因）。
///
/// Windows 上不走 std::process::Command：std 的 CreateProcessW 调用固定
/// bInheritHandles=TRUE 且无稳定 API 可关闭（CommandExt::inherit_handles
/// 仍 unstable）。这意味着调用方（cargo test / agent 脚本 / CI）经
/// Command::output() 捕获输出运行 `aproxy start` 时，其 stdout/stderr 管道
/// 写端句柄会被常驻守护进程继承，EOF 永不到来，调用方永久挂死。因此这里
/// 手写 CreateProcessW，以 bInheritHandles=FALSE 启动，不继承任何句柄。
/// 刻意不加 CREATE_BREAKAWAY_FROM_JOB：作业对象不允许 breakaway 时该标志
/// 会让 CreateProcess 直接失败，把「父环境关闭可能连带杀掉守护」这一罕见
/// 场景恶化成「根本启动不了」，得不偿失。
/// 返回子进程 pid（spawn 即返回，不等待——守护不随父进程生命周期）。
pub fn spawn_detached(exe: &std::path::Path, args: &[String]) -> io::Result<u32> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Foundation::{CloseHandle, GetLastError};
        use windows_sys::Win32::System::Threading::{
            CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT, CreateProcessW, PROCESS_INFORMATION,
            STARTUPINFOW,
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

        let mut si: STARTUPINFOW = unsafe { std::mem::zeroed() };
        si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
        let ok = unsafe {
            CreateProcessW(
                exe_wide.as_ptr(),
                cmdline_wide.as_mut_ptr(),
                std::ptr::null(), // lpProcessAttributes：默认安全描述符
                std::ptr::null(), // lpThreadAttributes：默认安全描述符
                0,                // bInheritHandles=FALSE：核心目的，杜绝句柄泄漏
                CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT,
                std::ptr::null(), // lpEnvironment=NULL：继承父进程环境
                std::ptr::null(), // lpCurrentDirectory=NULL：维持继承 cwd 的现状
                &si,
                &mut pi,
            )
        };
        if ok == 0 {
            return Err(io::Error::from_raw_os_error(
                unsafe { GetLastError() } as i32
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
        use std::process::{Command, Stdio};
        let mut cmd = Command::new(exe);
        cmd.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        Ok(cmd.spawn()?.id())
    }
}

// ---------------------------------------------------------------------------
// 实例注册表（~/.aproxy/run/<port>.pid）
// ---------------------------------------------------------------------------

/// 注册表文件路径：`<run_dir>/<port>.pid`
pub fn instance_file_path(listen_addr: &str) -> PathBuf {
    instance_file_path_in(&run_dir(), listen_addr)
}

/// 同上，目录可指定（测试注入用）
pub fn instance_file_path_in(run_dir: &std::path::Path, listen_addr: &str) -> PathBuf {
    run_dir.join(format!("{}.pid", port_of(listen_addr)))
}

/// 写入实例注册（bind 成功后调用，避免留下死记录）
pub fn write_instance_file(info: &InstanceInfo) -> io::Result<()> {
    write_instance_file_in(&run_dir(), info)
}

/// 同上，目录可指定（测试注入用）
///
/// 原子写：先写同目录临时文件再 rename 覆盖目标。直接写目标文件时，
/// 并发的 list_instances 可能读到写到一半的 JSON，把记录当损坏清理掉。
/// rename 在同卷内是原子操作（Windows 上经 MoveFileEx 的替换语义覆盖
/// 已存在文件）；临时文件名按端口隔离，实例间不会互踩。
pub fn write_instance_file_in(run_dir: &std::path::Path, info: &InstanceInfo) -> io::Result<()> {
    std::fs::create_dir_all(run_dir)?;
    let path = instance_file_path_in(run_dir, &info.listen_addr);
    let json = serde_json::to_string_pretty(info).expect("序列化实例信息失败");
    let tmp = path.with_extension("pid.tmp");
    std::fs::write(&tmp, json)?;
    match std::fs::rename(&tmp, &path) {
        Ok(()) => Ok(()),
        Err(e) => {
            // rename 失败时清掉残留临时文件，避免堆积成永久垃圾
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// 删除实例注册（服务退出时调用）
pub fn remove_instance_file(listen_addr: &str) {
    let _ = std::fs::remove_file(instance_file_path(listen_addr));
}

/// 列出注册表中的实例并逐个 IPC ping 验活；已死亡/损坏的记录直接清理残留文件。
/// 顺带清理孤儿日志（status 是唯一可靠的清理时机）。
pub async fn list_instances() -> Vec<InstanceInfo> {
    let live = list_instances_in(&run_dir()).await;
    cleanup_orphan_logs_in(&logs_dir(), &live, &list_restore_entries());
    live
}

/// 清理孤儿日志：日志目录中 `<端口>.log` 的端口既无存活实例、也无 .restore
/// 恢复记录时删除。startup.log 不按端口归属，跳过。
/// .restore 在此**保留不删**：它的语义是「期望恢复」——崩溃实例恰恰要靠它
/// 存活到 `aproxy restore` 执行；先跑一次 status 就把记录清掉会让自愈失效。
/// 崩溃待恢复实例的日志同理保留（排障线索，复活后同端口继续追加）。
/// 目录与待恢复清单参数化（测试注入用）。
fn cleanup_orphan_logs_in(
    logs_dir: &std::path::Path,
    live: &[InstanceInfo],
    pending_restore: &[RestoreEntry],
) {
    let live_ports: std::collections::HashSet<String> = live
        .iter()
        .map(|i| port_of(&i.listen_addr).to_string())
        .collect();
    let pending_ports: std::collections::HashSet<String> =
        pending_restore.iter().map(|e| e.port.clone()).collect();

    let Ok(entries) = std::fs::read_dir(logs_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if path.extension().and_then(|e| e.to_str()) != Some("log") || stem == "startup" {
            continue;
        }
        // 只清理端口命名的日志（防御：目录里若有非端口命名文件一律不动）
        if stem.parse::<u16>().is_err() {
            continue;
        }
        if !live_ports.contains(stem) && !pending_ports.contains(stem) {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// 同上，目录可指定（测试注入用）
pub async fn list_instances_in(dir: &std::path::Path) -> Vec<InstanceInfo> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("pid") {
            continue;
        }
        let info = std::fs::read_to_string(&path)
            .ok()
            .and_then(|c| serde_json::from_str::<InstanceInfo>(&c).ok());
        let Some(info) = info else {
            // 损坏记录无法定位端口发 IPC，直接清理
            let _ = std::fs::remove_file(&path);
            continue;
        };
        let port = port_of(&info.listen_addr).to_string();
        match ipc_ping(&port).await {
            // ping 响应携带实例的实时信息（含 last_activity_secs）——注册表
            // .pid 是启动时刻的快照，闲置判定/展示必须用实时值，否则活动
            // 时间永远停留在启动时刻、闲置=运行时长（实测踩坑）
            Ok(live_info) => out.push(live_info),
            Err(_) => {
                // 实例不在了：注册已失效，清理
                let _ = std::fs::remove_file(&path);
            }
        }
    }
    out.sort_by(|a, b| a.listen_addr.cmp(&b.listen_addr));
    out
}

/// 只读检索注册表：是否存在 pid 匹配的实例记录。
/// respawn 的就绪判定专用——不复用 `list_instances_in`（它对 ping 失败的
/// 条目有删除副作用：A 实例重拉的就绪轮询会顺带清掉同注册表里 B/C 死实例
/// 的记录，其死亡事件随后被混合态误判为优雅退出而失去自动恢复）。就绪只需
/// 「新 pid 的注册记录已出现」，读文件即可，无需 IPC。
pub fn registry_contains_pid_in(dir: &std::path::Path, pid: u32) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("pid") {
            return false;
        }
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|c| serde_json::from_str::<InstanceInfo>(&c).ok())
            .is_some_and(|info| info.pid == pid)
    })
}

// ---------------------------------------------------------------------------
// 自愈恢复记录（~/.aproxy/run/<port>.restore）
//
// 与 .pid 注册表互补：.pid 只在实例存活期间存在，而崩溃/断电/系统重启时守护
// 进程来不及清理任何文件——restore 记录承载「期望在运行」的语义：守护 bind
// 成功时写入当时的启动参数，优雅退出（含 `aproxy stop`）时删除，非正常死亡
// 则保留，`aproxy restore` 据此一键拉起。注意：参数可能含 --api-key 等明文，
// 与 config.toml 同级存放（用户主目录内），不额外加密。
// ---------------------------------------------------------------------------

/// 恢复记录文件路径：`<run_dir>/<port>.restore`
pub fn restore_file_path(listen_addr: &str) -> PathBuf {
    restore_file_path_in(&run_dir(), listen_addr)
}

/// 同上，目录可指定（测试注入用）
pub fn restore_file_path_in(run_dir: &std::path::Path, listen_addr: &str) -> PathBuf {
    run_dir.join(format!("{}.restore", port_of(listen_addr)))
}

/// 写入恢复记录：`args` 为实例的启动参数（不含 --daemon-child，恢复时统一追加）。
/// bind 成功后调用；同端口重复启动时覆盖旧记录。
pub fn write_restore_file(listen_addr: &str, args: &[String]) -> io::Result<()> {
    write_restore_file_in(&run_dir(), listen_addr, args)
}

/// 同上，目录可指定（测试注入用）。原子写语义同 write_instance_file_in。
pub fn write_restore_file_in(
    run_dir: &std::path::Path,
    listen_addr: &str,
    args: &[String],
) -> io::Result<()> {
    std::fs::create_dir_all(run_dir)?;
    let path = restore_file_path_in(run_dir, listen_addr);
    let json = serde_json::to_string(args).expect("序列化恢复记录失败");
    let tmp = path.with_extension("restore.tmp");
    std::fs::write(&tmp, json)?;
    match std::fs::rename(&tmp, &path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// 删除恢复记录（实例优雅退出时调用；无记录时静默）
pub fn remove_restore_file(listen_addr: &str) {
    remove_restore_file_in(&run_dir(), listen_addr)
}

/// 同上，目录可指定（测试注入用）
pub fn remove_restore_file_in(run_dir: &std::path::Path, listen_addr: &str) {
    let _ = std::fs::remove_file(restore_file_path_in(run_dir, listen_addr));
}

/// 一条恢复记录：端口 + 启动参数
#[derive(Debug, Clone, PartialEq)]
pub struct RestoreEntry {
    pub port: String,
    pub args: Vec<String>,
}

/// 列出全部恢复记录；损坏记录直接清理。不做存活校验——记录的本义就是
/// 「实例可能已死」。
pub fn list_restore_entries() -> Vec<RestoreEntry> {
    list_restore_entries_in(&run_dir())
}

/// 同上，目录可指定（测试注入用）
pub fn list_restore_entries_in(dir: &std::path::Path) -> Vec<RestoreEntry> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("restore") {
            continue;
        }
        let Some(port) = path.file_stem().and_then(|s| s.to_str()).map(String::from) else {
            continue;
        };
        let args = std::fs::read_to_string(&path)
            .ok()
            .and_then(|c| serde_json::from_str::<Vec<String>>(&c).ok());
        match args {
            Some(args) => out.push(RestoreEntry { port, args }),
            None => {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
    out.sort_by(|a, b| a.port.cmp(&b.port));
    out
}

// ---------------------------------------------------------------------------
// 通用协议层：请求-响应各一行 JSON。两个平台的传输实现共用这套编解码。
// ---------------------------------------------------------------------------

/// 在已建立的连接上完成一次「发请求行、收响应行」。
async fn exchange_over<S>(stream: S, req_line: &str) -> Result<String, String>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let (reader, mut writer) = tokio::io::split(stream);
    writer
        .write_all(req_line.as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    writer.write_all(b"\n").await.map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new(reader)
        .read_line(&mut line)
        .await
        .map_err(|e| e.to_string())?;
    Ok(line)
}

/// 处理一条 IPC 连接：解析请求行 → 执行 → 回响应行。
/// `stats`：代理层共享的实时观测源（活动时间戳/计数器/最近错误），
/// ping/shutdown 响应实时读取——stop idle/status 筛选靠活动时间戳判定闲置。
async fn handle_conn<S>(
    stream: S,
    on_shutdown: tokio::sync::watch::Sender<bool>,
    info: InstanceInfo,
    stats: Arc<IpcStats>,
) -> io::Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
    let (reader, mut writer) = tokio::io::split(stream);
    let mut line = String::new();
    // 请求行长度上限：命名管道是系统边界输入（同用户本地进程均可打开写入），
    // read_line 会无界累积直到遇到 \n，恶意/异常客户端可借此把守护进程内存
    // 吃到 OOM。超过上限即按无效请求回 ok:false 后断开。
    const MAX_REQUEST_LINE: u64 = 64 * 1024;
    let mut limited = BufReader::new(reader).take(MAX_REQUEST_LINE);
    limited.read_line(&mut line).await?;
    if !line.ends_with('\n') {
        // 行未正常终止：超过长度上限被截断，或对端在发完整请求前就断开——
        // 两种情况都不再继续累积，直接以无效请求收尾。
        let resp = IpcResponse {
            ok: false,
            info: None,
            proto: IPC_PROTO_VERSION,
        };
        let resp_line = serde_json::to_string(&resp).expect("序列化 IPC 响应失败");
        let _ = write_line(&mut writer, &resp_line).await;
        return Ok(());
    }
    // 组装带实时观测值的实例信息（计数器/最近错误在探测瞬间读取）
    let mut info = info;
    info.proto_version = IPC_PROTO_VERSION;
    info.requests_total = stats
        .requests_total
        .load(std::sync::atomic::Ordering::Relaxed);
    info.retries_total = stats
        .retries_total
        .load(std::sync::atomic::Ordering::Relaxed);
    info.last_activity_secs = stats
        .last_activity_secs
        .load(std::sync::atomic::Ordering::Relaxed);
    if let Ok(slot) = stats.last_error.lock()
        && let Some((msg, at)) = slot.as_ref()
    {
        info.last_error = Some(msg.clone());
        info.last_error_at = *at;
    }
    let resp: IpcResponse = match serde_json::from_str(&line) {
        Ok(IpcRequest::Ping | IpcRequest::Stats) => IpcResponse {
            ok: true,
            info: Some(info),
            proto: IPC_PROTO_VERSION,
        },
        Ok(IpcRequest::Shutdown) => {
            // 响应先发出去再触发停止：客户端立刻拿到确认，服务随后优雅退出
            let resp = IpcResponse {
                ok: true,
                info: Some(info),
                proto: IPC_PROTO_VERSION,
            };
            write_line(
                &mut writer,
                &serde_json::to_string(&resp).expect("序列化 IPC 响应失败"),
            )
            .await?;
            let _ = on_shutdown.send(true);
            return Ok(());
        }
        Err(_) => IpcResponse {
            ok: false,
            info: None,
            proto: IPC_PROTO_VERSION,
        },
    };
    write_line(
        &mut writer,
        &serde_json::to_string(&resp).expect("序列化 IPC 响应失败"),
    )
    .await
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
mod imp {
    use super::{InstanceInfo, IpcStats, exchange_over, handle_conn};
    use std::{io, sync::Arc, time::Duration};
    use tokio::net::windows::named_pipe::{ClientOptions, ServerOptions};
    use tokio::sync::watch::Sender;

    /// 强制终止（--force）：镜像名验证 + TerminateProcess。
    /// 复用 watchdog::imp_process 的验证原语——同一套防 PID 复用逻辑。
    pub fn terminate_process(pid: u32) -> Result<(), String> {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_TERMINATE, TerminateProcess,
        };
        if !crate::watchdog::is_aproxy_process(pid) {
            return Err(format!(
                "pid {pid} 不是 aProxy 进程（镜像名不符），拒绝强制终止以防误杀"
            ));
        }
        unsafe {
            let handle = OpenProcess(PROCESS_TERMINATE, 0, pid);
            if handle == 0 {
                return Err(format!("无法打开 pid {pid}（可能已退出）"));
            }
            let ok = TerminateProcess(handle, 1);
            let _ = CloseHandle(handle);
            if ok == 0 {
                return Err(format!(
                    "终止 pid {pid} 失败（{}）",
                    io::Error::last_os_error()
                ));
            }
        }
        Ok(())
    }

    /// 端点不存在（无实例）时返回可读错误。
    ///
    /// ERROR_PIPE_BUSY(231) 重试：serve 循环在 connect() 完成、重建下一个
    /// 监听实例之间存在零监听窗口，管道名存在但无空闲实例，此时 CreateFile
    /// 返回 busy 而非「端点不存在」。tokio 文档明确要求客户端对该错误
    /// sleep 后重试；封顶 2 秒（上层 ipc_request 的 3 秒超时之内）。
    pub async fn exchange(endpoint: &str, req_line: &str) -> Result<String, String> {
        const ERROR_PIPE_BUSY: i32 = 231;
        const BUSY_RETRY_CAP: Duration = Duration::from_secs(2);
        const BUSY_RETRY_INTERVAL: Duration = Duration::from_millis(50);
        let deadline = tokio::time::Instant::now() + BUSY_RETRY_CAP;
        let client = loop {
            match ClientOptions::new().open(endpoint) {
                Ok(client) => break client,
                Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                    if tokio::time::Instant::now() >= deadline {
                        return Err(format!("无法连接（{e}）"));
                    }
                    tokio::time::sleep(BUSY_RETRY_INTERVAL).await;
                }
                Err(e) => return Err(format!("无法连接（{e}）")),
            }
        };
        exchange_over(client, req_line).await
    }

    /// 接受循环：为每个连接 spawn 处理任务；始终重建监听实例以接受后续连接。
    pub async fn serve(
        endpoint: String,
        on_shutdown: Sender<bool>,
        info: InstanceInfo,
        stats: Arc<IpcStats>,
    ) -> io::Result<()> {
        let mut server = ServerOptions::new()
            .first_pipe_instance(true)
            .create(&endpoint)?;
        loop {
            server.connect().await?;
            let client = server;
            server = ServerOptions::new().create(&endpoint)?;
            let shutdown = on_shutdown.clone();
            let info = info.clone();
            let stats = stats.clone();
            tokio::spawn(async move {
                let _ = handle_conn(client, shutdown, info, stats).await;
            });
        }
    }
}

#[cfg(unix)]
mod imp {
    use super::{InstanceInfo, IpcStats, exchange_over, handle_conn};
    use std::{io, path::Path, sync::Arc, time::Duration};
    use tokio::net::UnixListener;
    use tokio::sync::watch::Sender;

    /// 强制终止（--force）：SIGKILL（unix 无镜像名 API，靠 claim/探活上层验证）
    pub fn terminate_process(pid: u32) -> Result<(), String> {
        // 裸 extern 声明直接指向 libc 的 kill(2) 符号（unix 分支不引 libc crate，
        // Windows 下整个 imp 模块被 cfg 排除，符号只在 unix 链接）
        let r = unsafe { kill(pid as i32, 9) };
        if r == 0 {
            Ok(())
        } else {
            Err(format!(
                "终止 pid {pid} 失败（{}）",
                io::Error::last_os_error()
            ))
        }
    }

    unsafe extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }

    pub async fn exchange(endpoint: &str, req_line: &str) -> Result<String, String> {
        let client = tokio::net::UnixStream::connect(endpoint)
            .await
            .map_err(|e| format!("无法连接（{e}）"))?;
        exchange_over(client, req_line).await
    }

    pub async fn serve(
        endpoint: String,
        on_shutdown: Sender<bool>,
        info: InstanceInfo,
        stats: Arc<IpcStats>,
    ) -> io::Result<()> {
        let path = Path::new(&endpoint);
        // 残留 socket 文件会令 bind 失败，先清理
        let _ = std::fs::remove_file(path);
        let listener = UnixListener::bind(path)?;
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(conn) => conn,
                Err(_) => {
                    // 持续性 accept 错误（如 fd 耗尽的 EMFILE）不会自行恢复，
                    // 立即重试会形成占满 CPU 的紧死循环；睡一拍给错误源恢复机会。
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
            };
            let shutdown = on_shutdown.clone();
            let info = info.clone();
            let stats = stats.clone();
            tokio::spawn(async move {
                let _ = handle_conn(stream, shutdown, info, stats).await;
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // 唯一消费者是下方 cfg(windows) 的 IPC roundtrip 测试（unix 分支的
    // UDS roundtrip 测试尚未编写）
    #[cfg(windows)]
    use std::sync::atomic::Ordering;

    fn sample_info(port: &str) -> InstanceInfo {
        InstanceInfo {
            pid: 42,
            version: "0.0.0-test".into(),
            listen_addr: format!("127.0.0.1:{port}"),
            config_path: "C:/tmp/config.toml".into(),
            base_url: "https://api.example.com".into(),
            started_at: 1_700_000_000,
            last_activity_secs: 0,
            proto_version: IPC_PROTO_VERSION,
            requests_total: 0,
            retries_total: 0,
            last_error: None,
            last_error_at: 0,
        }
    }

    #[test]
    fn port_of_extracts_trailing_port() {
        assert_eq!(port_of("127.0.0.1:12345"), "12345");
        assert_eq!(port_of("[::1]:8080"), "8080");
        assert_eq!(port_of("no-port"), "no-port");
    }

    #[test]
    fn ipc_request_serde_roundtrip() {
        let ping = serde_json::to_string(&IpcRequest::Ping).unwrap();
        assert_eq!(ping, r#"{"op":"ping"}"#);
        let shutdown = serde_json::to_string(&IpcRequest::Shutdown).unwrap();
        assert_eq!(shutdown, r#"{"op":"shutdown"}"#);
        let back: IpcRequest = serde_json::from_str(&ping).unwrap();
        assert!(matches!(back, IpcRequest::Ping));
    }

    #[test]
    fn instance_file_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let info = sample_info("45678");
        write_instance_file_in(dir.path(), &info).unwrap();
        let path = instance_file_path_in(dir.path(), "127.0.0.1:45678");
        assert_eq!(path.file_name().unwrap(), "45678.pid");
        let loaded: InstanceInfo =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(loaded.pid, 42);
        assert_eq!(loaded.listen_addr, "127.0.0.1:45678");
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn ipc_ping_and_shutdown_roundtrip() {
        // 测试专属端点名（不与生产实例的 aproxy-<port> 冲突）
        let endpoint = format!(r"\\.\pipe\aproxy-test-{}", std::process::id());
        let (tx, mut rx) = tokio::sync::watch::channel(false);
        let info = sample_info("0");
        let stats = Arc::new(IpcStats::default());
        stats.requests_total.store(7, Ordering::Relaxed);
        stats.retries_total.store(2, Ordering::Relaxed);
        stats.record_error("上游返回 502 Bad Gateway（错误内容，重试）");
        let server = tokio::spawn(imp::serve(endpoint.clone(), tx, info.clone(), stats));
        // 等待服务端监听实例建好
        tokio::time::sleep(Duration::from_millis(100)).await;

        // ping：响应携带实例信息 + 实时观测值 + v2 协议号
        let line = imp::exchange(&endpoint, r#"{"op":"ping"}"#).await.unwrap();
        let resp: IpcResponse = serde_json::from_str(&line).unwrap();
        assert!(resp.ok);
        assert_eq!(resp.proto, IPC_PROTO_VERSION, "新实例应报 v2 协议");
        let info = resp.info.unwrap();
        assert_eq!(info.pid, 42);
        assert_eq!(info.proto_version, IPC_PROTO_VERSION);
        assert_eq!(info.requests_total, 7);
        assert_eq!(info.retries_total, 2);
        assert!(
            info.last_error.as_deref().unwrap_or("").contains("502"),
            "ping 应携带最近错误: {:?}",
            info.last_error
        );
        assert!(info.last_error_at > 0);

        // shutdown：响应确认后置位停止信号
        let line = imp::exchange(&endpoint, r#"{"op":"shutdown"}"#)
            .await
            .unwrap();
        let resp: IpcResponse = serde_json::from_str(&line).unwrap();
        assert!(resp.ok);
        rx.changed().await.unwrap();
        assert!(*rx.borrow());

        // 未知请求：ok=false 而非崩溃（新 CLI 对旧实例发未知 op 的降级依据：
        // 旧实例同样回 ok:false，客户端以此感知「op 不被支持」）
        let line = imp::exchange(&endpoint, r#"{"op":"what"}"#).await.unwrap();
        let resp: IpcResponse = serde_json::from_str(&line).unwrap();
        assert!(!resp.ok);

        server.abort();
    }

    #[test]
    fn ipc_v1_compat_old_response_and_registry() {
        // 旧实例（alpha.5）响应缺 proto 字段 → 读为 v1；新客户端据此降级
        let v1_line = r#"{"ok":true,"info":{"pid":42,"version":"0.1.0-alpha.5","listen_addr":"127.0.0.1:12345","config_path":"C:/tmp/c.toml","base_url":"https://x","started_at":123,"last_activity_secs":0}}"#;
        let resp: IpcResponse = serde_json::from_str(v1_line).unwrap();
        assert_eq!(resp.proto, IPC_PROTO_V1);
        let info = resp.info.unwrap();
        assert_eq!(info.proto_version, 0, "v1 实例无协议字段，读为 0");
        assert_eq!(info.requests_total, 0);
        assert_eq!(info.retries_total, 0);
        assert!(info.last_error.is_none());
        // 旧注册表文件（同样缺 v2 字段）照常读取
        let old_registry = r#"{"pid":7,"version":"0.1.0-alpha.4","listen_addr":"127.0.0.1:59811","config_path":"C:/tmp/c.toml","base_url":"https://x","started_at":9,"last_activity_secs":5}"#;
        let info: InstanceInfo = serde_json::from_str(old_registry).unwrap();
        assert_eq!(info.pid, 7);
        assert_eq!(info.last_activity_secs, 5);
        assert_eq!(info.requests_total, 0);
    }

    #[test]
    fn ipc_stats_record_error_overwrites_and_truncates() {
        let stats = IpcStats::default();
        assert!(stats.last_error.lock().unwrap().is_none());
        stats.record_error("第一次错误");
        stats.record_error("第二次错误");
        {
            let slot = stats.last_error.lock().unwrap();
            let (msg, _) = slot.as_ref().unwrap();
            assert!(msg.contains("第二次"), "应覆盖为最近一次: {msg}");
        }
        // 超长错误截断到 200 字符（按 char，多字节不撕裂）
        let long = "长".repeat(500);
        stats.record_error(&long);
        let slot = stats.last_error.lock().unwrap();
        let (msg, _) = slot.as_ref().unwrap();
        assert_eq!(msg.chars().count(), 200);
    }

    #[tokio::test]
    async fn ipc_ping_fails_when_no_instance() {
        // 不存在的端点：ping 必须报错（= 端口上没有 aProxy）
        let port = format!("599{:02}", std::process::id() % 100);
        assert!(ipc_ping(&port).await.is_err());
    }

    #[tokio::test]
    async fn list_instances_cleans_dead_records() {
        // 写一条指向不存在实例的注册记录 → list 应清理它且不返回
        let dir = tempfile::tempdir().unwrap();
        let dead = sample_info("59801");
        write_instance_file_in(dir.path(), &dead).unwrap();
        let path = instance_file_path_in(dir.path(), "127.0.0.1:59801");
        let listed = list_instances_in(dir.path()).await;
        assert!(listed.iter().all(|i| i.listen_addr != "127.0.0.1:59801"));
        assert!(!path.exists(), "死亡实例的注册记录应被清理");
    }

    #[test]
    fn restore_file_roundtrip_and_cleanup() {
        let dir = tempfile::tempdir().unwrap();
        let args = vec![
            "--config".to_string(),
            "C:/tmp/cfg.toml".to_string(),
            "--api-key".to_string(),
            "sk-test".to_string(),
        ];
        write_restore_file_in(dir.path(), "127.0.0.1:59805", &args).unwrap();
        let path = restore_file_path_in(dir.path(), "127.0.0.1:59805");
        assert_eq!(path.file_name().unwrap(), "59805.restore");

        // 覆盖写：同端口再次启动应以最新参数为准
        let args2 = vec!["--config".to_string(), "C:/tmp/new.toml".to_string()];
        write_restore_file_in(dir.path(), "127.0.0.1:59805", &args2).unwrap();

        let entries = list_restore_entries_in(dir.path());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].port, "59805");
        assert_eq!(entries[0].args, args2);

        remove_restore_file_in(dir.path(), "127.0.0.1:59805");
        assert!(!path.exists());
        assert!(list_restore_entries_in(dir.path()).is_empty());
    }

    #[test]
    fn restore_list_cleans_corrupted_record() {
        let dir = tempfile::tempdir().unwrap();
        let path = restore_file_path_in(dir.path(), "127.0.0.1:59806");
        std::fs::write(&path, "{not-json").unwrap();
        assert!(list_restore_entries_in(dir.path()).is_empty());
        assert!(!path.exists(), "损坏的恢复记录应被清理");
    }

    #[test]
    fn restore_entry_without_config_is_listed() {
        // 无 --config 参数的实例（纯默认配置）也应能列出与恢复
        let dir = tempfile::tempdir().unwrap();
        write_restore_file_in(dir.path(), "127.0.0.1:59807", &[]).unwrap();
        let entries = list_restore_entries_in(dir.path());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].port, "59807");
        assert!(entries[0].args.is_empty());
    }

    #[test]
    fn orphan_log_cleanup_keeps_live_pending_and_startup() {
        let logs = tempfile::tempdir().unwrap();
        // 三个日志：存活实例 59811 / 待恢复（崩溃）59812 / 彻底死亡 59813；
        // 另有 startup.log（永不按端口清理）与非数字名（防御性跳过）
        for name in [
            "59811.log",
            "59812.log",
            "59813.log",
            "startup.log",
            "weird.log",
        ] {
            std::fs::write(logs.path().join(name), "x").unwrap();
        }
        let live = vec![sample_info("59811")];
        let pending = vec![RestoreEntry {
            port: "59812".into(),
            args: vec![],
        }];
        cleanup_orphan_logs_in(logs.path(), &live, &pending);
        for kept in ["59811.log", "59812.log", "startup.log", "weird.log"] {
            assert!(logs.path().join(kept).exists(), "{kept} 不应被孤儿清理删除");
        }
        assert!(
            !logs.path().join("59813.log").exists(),
            "彻底死亡实例的孤儿日志应被删除"
        );
    }
}
