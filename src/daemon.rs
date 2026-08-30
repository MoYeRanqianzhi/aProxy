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
use std::{io, path::PathBuf, time::Duration};

/// 实例注册表目录：`~/.aproxy/run/`
pub fn run_dir() -> PathBuf {
    crate::config::config_dir().join("run")
}

/// 守护进程日志目录：`~/.aproxy/logs/`
pub fn logs_dir() -> PathBuf {
    crate::config::config_dir().join("logs")
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
}

/// IPC 请求。framing：一行 JSON + `\n`。
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum IpcRequest {
    /// 实例识别（status / start 预检）
    Ping,
    /// 优雅停止
    Shutdown,
}

/// IPC 响应：一行 JSON + `\n`。
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct IpcResponse {
    pub ok: bool,
    /// 实例信息（ping/shutdown 成功时都携带，便于展示）
    #[serde(default)]
    pub info: Option<InstanceInfo>,
}

/// 探测端口上是否有 aProxy 实例（纯 IPC，不触碰任何 TCP 端口）。
/// Ok(info) = 实例在运行；Err = 端点上没有可识别的 aProxy（无实例，
/// 或该端口被其他程序占用——由调用方结合 TCP bind 结果区分这两种情况）。
pub async fn ipc_ping(port: &str) -> Result<InstanceInfo, String> {
    match ipc_request(port, &IpcRequest::Ping).await {
        Ok(resp) if resp.ok => resp.info.ok_or_else(|| "实例响应缺少信息".to_string()),
        Ok(_) => Err("实例返回失败".to_string()),
        Err(e) => Err(e),
    }
}

/// 发送 IPC 请求并等待响应（3 秒超时）。
pub async fn ipc_request(port: &str, req: &IpcRequest) -> Result<IpcResponse, String> {
    let endpoint = endpoint_for(port);
    let req_line = serde_json::to_string(req).expect("序列化 IPC 请求失败");
    let fut = imp::exchange(&endpoint, &req_line);
    match tokio::time::timeout(Duration::from_secs(3), fut).await {
        Ok(Ok(line)) => serde_json::from_str(&line).map_err(|e| format!("{endpoint}: 响应解析失败 {e}")),
        Ok(Err(e)) => Err(format!("{endpoint}: {e}")),
        Err(_) => Err(format!("{endpoint}: 请求超时")),
    }
}

/// 启动实例的 IPC 控制服务（每实例一条独立端点，随进程退出而终止）。
/// 收到 shutdown 时置位 `on_shutdown`（watch bool），由服务主循环执行优雅退出。
/// `port` 收 owned 值：调用方以 tokio::spawn 运行本 future，参数不能借用。
pub async fn serve_ipc(
    port: String,
    on_shutdown: tokio::sync::watch::Sender<bool>,
    info: InstanceInfo,
) -> io::Result<()> {
    imp::serve(endpoint_for(&port), on_shutdown, info).await
}

/// 等待实例退出（IPC ping 失败即视为已退出），超时返回 false。
pub async fn wait_until_gone(port: &str, timeout: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if ipc_ping(port).await.is_err() {
            return true;
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
/// 返回子进程 pid（spawn 即返回，不等待——守护不随父进程生命周期）。
pub fn spawn_detached(exe: &std::path::Path, args: &[String]) -> io::Result<u32> {
    use std::process::{Command, Stdio};
    let mut cmd = Command::new(exe);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    Ok(cmd.spawn()?.id())
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
pub fn write_instance_file_in(run_dir: &std::path::Path, info: &InstanceInfo) -> io::Result<()> {
    std::fs::create_dir_all(run_dir)?;
    let path = instance_file_path_in(run_dir, &info.listen_addr);
    let json = serde_json::to_string_pretty(info).expect("序列化实例信息失败");
    std::fs::write(path, json)
}

/// 删除实例注册（服务退出时调用）
pub fn remove_instance_file(listen_addr: &str) {
    let _ = std::fs::remove_file(instance_file_path(listen_addr));
}

/// 列出注册表中的实例并逐个 IPC ping 验活；已死亡/损坏的记录直接清理残留文件。
pub async fn list_instances() -> Vec<InstanceInfo> {
    list_instances_in(&run_dir()).await
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
        if ipc_ping(&port).await.is_ok() {
            out.push(info);
        } else {
            // 实例不在了：注册已失效，清理
            let _ = std::fs::remove_file(&path);
        }
    }
    out.sort_by(|a, b| a.listen_addr.cmp(&b.listen_addr));
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
async fn handle_conn<S>(
    stream: S,
    on_shutdown: tokio::sync::watch::Sender<bool>,
    info: InstanceInfo,
) -> io::Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::{AsyncBufReadExt, BufReader};
    let (reader, mut writer) = tokio::io::split(stream);
    let mut line = String::new();
    BufReader::new(reader).read_line(&mut line).await?;
    let resp: IpcResponse = match serde_json::from_str(&line) {
        Ok(IpcRequest::Ping) => IpcResponse { ok: true, info: Some(info) },
        Ok(IpcRequest::Shutdown) => {
            // 响应先发出去再触发停止：客户端立刻拿到确认，服务随后优雅退出
            let resp = IpcResponse { ok: true, info: Some(info) };
            write_line(&mut writer, &serde_json::to_string(&resp).expect("序列化 IPC 响应失败")).await?;
            let _ = on_shutdown.send(true);
            return Ok(());
        }
        Err(_) => IpcResponse { ok: false, info: None },
    };
    write_line(&mut writer, &serde_json::to_string(&resp).expect("序列化 IPC 响应失败")).await
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
    use super::{handle_conn, exchange_over, InstanceInfo};
    use std::io;
    use tokio::net::windows::named_pipe::{ClientOptions, ServerOptions};
    use tokio::sync::watch::Sender;

    /// 单次请求-响应交换：连接端点、发请求行、收响应行。
    /// 端点不存在（无实例）时返回可读错误。
    pub async fn exchange(endpoint: &str, req_line: &str) -> Result<String, String> {
        let client = ClientOptions::new()
            .open(endpoint)
            .map_err(|e| format!("无法连接（{e}）"))?;
        exchange_over(client, req_line).await
    }

    /// 接受循环：为每个连接 spawn 处理任务；始终重建监听实例以接受后续连接。
    pub async fn serve(endpoint: String, on_shutdown: Sender<bool>, info: InstanceInfo) -> io::Result<()> {
        let mut server = ServerOptions::new()
            .first_pipe_instance(true)
            .create(&endpoint)?;
        loop {
            server.connect().await?;
            let client = server;
            server = ServerOptions::new().create(&endpoint)?;
            let shutdown = on_shutdown.clone();
            let info = info.clone();
            tokio::spawn(async move {
                let _ = handle_conn(client, shutdown, info).await;
            });
        }
    }
}

#[cfg(unix)]
mod imp {
    use super::{handle_conn, exchange_over, InstanceInfo};
    use std::{io, path::Path};
    use tokio::net::UnixListener;
    use tokio::sync::watch::Sender;

    pub async fn exchange(endpoint: &str, req_line: &str) -> Result<String, String> {
        let client = tokio::net::UnixStream::connect(endpoint)
            .await
            .map_err(|e| format!("无法连接（{e}）"))?;
        exchange_over(client, req_line).await
    }

    pub async fn serve(endpoint: String, on_shutdown: Sender<bool>, info: InstanceInfo) -> io::Result<()> {
        let path = Path::new(&endpoint);
        // 残留 socket 文件会令 bind 失败，先清理
        let _ = std::fs::remove_file(path);
        let listener = UnixListener::bind(path)?;
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                continue;
            };
            let shutdown = on_shutdown.clone();
            let info = info.clone();
            tokio::spawn(async move {
                let _ = handle_conn(stream, shutdown, info).await;
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_info(port: &str) -> InstanceInfo {
        InstanceInfo {
            pid: 42,
            version: "0.0.0-test".into(),
            listen_addr: format!("127.0.0.1:{port}"),
            config_path: "C:/tmp/config.toml".into(),
            base_url: "https://api.example.com".into(),
            started_at: 1_700_000_000,
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
        let server = tokio::spawn(imp::serve(endpoint.clone(), tx, info.clone()));
        // 等待服务端监听实例建好
        tokio::time::sleep(Duration::from_millis(100)).await;

        // ping：响应携带实例信息
        let line = imp::exchange(&endpoint, r#"{"op":"ping"}"#).await.unwrap();
        let resp: IpcResponse = serde_json::from_str(&line).unwrap();
        assert!(resp.ok);
        assert_eq!(resp.info.unwrap().pid, 42);

        // shutdown：响应确认后置位停止信号
        let line = imp::exchange(&endpoint, r#"{"op":"shutdown"}"#).await.unwrap();
        let resp: IpcResponse = serde_json::from_str(&line).unwrap();
        assert!(resp.ok);
        rx.changed().await.unwrap();
        assert!(*rx.borrow());

        // 未知请求：ok=false 而非崩溃
        let line = imp::exchange(&endpoint, r#"{"op":"what"}"#).await.unwrap();
        let resp: IpcResponse = serde_json::from_str(&line).unwrap();
        assert!(!resp.ok);

        server.abort();
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
}
