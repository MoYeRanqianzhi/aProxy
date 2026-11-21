//! Connect to an instance in an explicit run directory; a connection never uses TCP.
//! One-shot commands and event subscriptions share the platform peer check.

use crate::{endpoint::instance as endpoint_for_in, wire::*};
use std::{io, time::Duration};

/// 一次 IPC 请求的失败，按「能据此下什么结论」分类。
#[derive(Debug)]
pub enum IpcError {
    /// 端点不存在：确定没有实例在这里（不必重试；stop 据此确认已退出）
    Unreachable(String),
    /// 管道忙、超时、读写出错：可能有实例，只是此刻没给出应答（挂死也是这样）
    Transient(String),
    /// 有应答但无法采信：解析不了、协议版本不符、自报 pid 与连接对端不符
    Protocol(String),
    /// 实例明确拒绝了请求。`details` 是这个码带的数据（没有时为 null）
    Remote {
        code: String,
        message: String,
        details: serde_json::Value,
    },
}

impl std::fmt::Display for IpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreachable(e) | Self::Transient(e) | Self::Protocol(e) => f.write_str(e),
            Self::Remote { code, message, .. } => {
                write!(f, "实例拒绝了请求（{code}）：{message}")
            }
        }
    }
}

impl std::error::Error for IpcError {}

impl IpcError {
    /// 给连接类与协议类的失败补上是哪个端点；实例明确的拒绝原样返回。
    fn at(self, endpoint: &str) -> Self {
        match self {
            IpcError::Unreachable(e) => IpcError::Unreachable(format!("{endpoint}: {e}")),
            IpcError::Transient(e) => IpcError::Transient(format!("{endpoint}: {e}")),
            IpcError::Protocol(e) => IpcError::Protocol(format!("{endpoint}: {e}")),
            other => other,
        }
    }
}

/// 同 ipc_ping，但按显式 run_dir 寻址（库层调用者用这个）。
///
/// 端点不存在、应答无法采信、实例明确拒绝都立即返回；只有 `Transient`
/// 连续 3 次（间隔 200ms）才算失败。单次 `Transient` 可能是 Windows 命名管道
/// 瞬时 busy（serve 重建监听实例的零监听窗口）或 3 秒超时，调用方据 Err
/// 下结论（注册表普查据它转入进程身份核验），单次抖动不该左右结论。
pub async fn ipc_ping_in(
    run_dir: &std::path::Path,
    port: &str,
) -> Result<InstanceStatus, IpcError> {
    const ATTEMPTS: usize = 3;
    const RETRY_INTERVAL: Duration = Duration::from_millis(200);
    let mut attempt = 0;
    loop {
        match request_in(run_dir, port, IpcOp::Ping).await {
            Err(IpcError::Transient(_)) if attempt + 1 < ATTEMPTS => {
                attempt += 1;
                tokio::time::sleep(RETRY_INTERVAL).await;
            }
            other => return other,
        }
    }
}

/// 清点用的探测：比 [`ipc_ping_in`] 快得多地下结论——单次只等 1 秒，`Transient` 时再试
/// 一次。清点要同时问遍所有端口并立刻给出一张表，挂死的实例不该让整张表等上十秒；代价是
/// 忙到 1 秒内答不上来的实例会被报成不应答，下一次清点自然纠正。
pub async fn ipc_probe_in(
    run_dir: &std::path::Path,
    port: &str,
) -> Result<InstanceStatus, IpcError> {
    const TIMEOUT: Duration = Duration::from_secs(1);
    let endpoint = endpoint_for_in(run_dir, port);
    match request_raw_within(&endpoint, IpcOp::Ping, TIMEOUT).await {
        Err(IpcError::Transient(_)) => {
            tokio::time::sleep(Duration::from_millis(200)).await;
            request_raw_within(&endpoint, IpcOp::Ping, TIMEOUT).await
        }
        other => other,
    }
}

/// 同 ipc_request，按显式 run_dir 寻址。
pub async fn ipc_request_in(
    run_dir: &std::path::Path,
    port: &str,
    op: IpcOp,
) -> Result<InstanceStatus, IpcError> {
    request_in(run_dir, port, op).await
}

/// 按 run 目录与端口寻址：端点带 run 目录的命名空间（见 `endpoint_for_in`）。
async fn request_in(
    run_dir: &std::path::Path,
    port: &str,
    op: IpcOp,
) -> Result<InstanceStatus, IpcError> {
    request_raw(&endpoint_for_in(run_dir, port), op).await
}

/// 按显式端点发送 IPC 请求并等待应答（3 秒超时）。按实例寻址请用
/// `ipc_request_in`：它负责端点命名空间。
pub async fn request_raw(endpoint: &str, op: IpcOp) -> Result<InstanceStatus, IpcError> {
    request_raw_within(endpoint, op, Duration::from_secs(3)).await
}

/// 同 [`request_raw`]，等待应答的时限由调用方给。
async fn request_raw_within(
    endpoint: &str,
    op: IpcOp,
    timeout: Duration,
) -> Result<InstanceStatus, IpcError> {
    let request = op.request_line();
    let fut = imp::exchange(endpoint, &request);
    let (line, peer) = match tokio::time::timeout(timeout, fut).await {
        Ok(Ok(reply)) => reply,
        Ok(Err(IpcError::Unreachable(e))) => {
            return Err(IpcError::Unreachable(format!("{endpoint}: {e}")));
        }
        Ok(Err(IpcError::Transient(e))) => {
            return Err(IpcError::Transient(format!("{endpoint}: {e}")));
        }
        Ok(Err(e)) => return Err(e),
        Err(_) => return Err(IpcError::Transient(format!("{endpoint}: 请求超时"))),
    };
    let status = parse_reply(&line).map_err(|e| match e {
        IpcError::Protocol(m) => IpcError::Protocol(format!("{endpoint}: {m}")),
        other => other,
    })?;
    // 应答者自报的 pid 必须就是连接对端的进程：普查与 stop 都按这个 pid 认
    // 实例，而端点名可以预测，任何本机进程都可能抢先占住它、冒充实例应答
    if let Some(peer) = peer
        && status.instance.pid != peer
    {
        return Err(IpcError::Protocol(format!(
            "{endpoint}: 应答者自报 pid {} 与连接对端进程 {peer} 不一致",
            status.instance.pid
        )));
    }
    Ok(status)
}

/// 读请求历史的一元 op（`requests`、`request`、`body`）：发一次请求，返回应答里的 `result`。
/// 3 秒超时。`subscribe` 用 [`ipc_subscribe_in`]。
///
/// 调用之前先看快照的 `ops` 里有没有这个 op：没有它的实例回 `unknown_op`。
pub async fn ipc_read_in(
    run_dir: &std::path::Path,
    port: &str,
    op: &ReadOp,
) -> Result<serde_json::Value, IpcError> {
    let endpoint = endpoint_for_in(run_dir, port);
    let request = op.request_line();
    let exchange = async {
        let (stream, _) = imp::connect(&endpoint).await?;
        exchange_over(stream, &request).await
    };
    let line = match tokio::time::timeout(Duration::from_secs(3), exchange).await {
        Ok(Ok(line)) => line,
        Ok(Err(e)) => return Err(e.at(&endpoint)),
        Err(_) => return Err(IpcError::Transient(format!("{endpoint}: 请求超时"))),
    };
    parse_result(&line).map_err(|e| e.at(&endpoint))
}

/// 一条订阅连接：[`ipc_subscribe_in`] 的返回值，之后每调一次 [`EventStream::next`] 读一个
/// 事件。丢掉它就是退订。
pub struct EventStream {
    lines: tokio::io::BufReader<imp::Stream>,
}

impl EventStream {
    /// 下一个事件：`{"v":1,"event":…,"seq":n,"at":毫秒,"data":…}`。实例关了连接时为
    /// `Ok(None)`。45 秒收不到任何一行按失联报 `Transient`（实例至少每 15 秒发一行）。
    ///
    /// 不认识的事件照样返回，由调用方忽略；`lagged` 之后调用方该重取快照与记录。
    pub async fn next(&mut self) -> Result<Option<serde_json::Value>, IpcError> {
        use tokio::io::AsyncBufReadExt;
        const SILENCE: Duration = Duration::from_secs(45);
        let mut line = String::new();
        let read = tokio::time::timeout(SILENCE, self.lines.read_line(&mut line))
            .await
            .map_err(|_| IpcError::Transient("订阅连接 45 秒没有收到任何事件".to_string()))?
            .map_err(|e| IpcError::Transient(e.to_string()))?;
        if read == 0 || !line.ends_with('\n') {
            return Ok(None);
        }
        serde_json::from_str(&line)
            .map(Some)
            .map_err(|e| IpcError::Protocol(format!("事件不是 JSON：{e}")))
    }
}

/// 订阅实例的事件。返回订阅那一刻的 `{"snapshot":…,"requests":[在途的记录]}` 与事件流。
/// 实例的流式连接已满时是 `Remote`，码为 `busy`。
pub async fn ipc_subscribe_in(
    run_dir: &std::path::Path,
    port: &str,
) -> Result<(serde_json::Value, EventStream), IpcError> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    let endpoint = endpoint_for_in(run_dir, port);
    let open = async {
        let (mut stream, _) = imp::connect(&endpoint).await?;
        let failed = |e: io::Error| IpcError::Transient(e.to_string());
        let request = ReadOp::Subscribe.request_line();
        stream.write_all(request.as_bytes()).await.map_err(failed)?;
        stream.write_all(b"\n").await.map_err(failed)?;
        let mut lines = tokio::io::BufReader::new(stream);
        let mut line = String::new();
        lines.read_line(&mut line).await.map_err(failed)?;
        if !line.ends_with('\n') {
            return Err(IpcError::Transient("连接在应答完整之前关闭".to_string()));
        }
        Ok((parse_result(&line)?, EventStream { lines }))
    };
    match tokio::time::timeout(Duration::from_secs(3), open).await {
        Ok(Ok(opened)) => Ok(opened),
        Ok(Err(e)) => Err(e.at(&endpoint)),
        Err(_) => Err(IpcError::Transient(format!("{endpoint}: 请求超时"))),
    }
}

/// 解析一行 v1 应答，取出 `result`；失败应答变成 `Remote`。
fn parse_result(line: &str) -> Result<serde_json::Value, IpcError> {
    let mut value: serde_json::Value = serde_json::from_str(line)
        .map_err(|e| IpcError::Protocol(format!("应答不是 JSON：{e}")))?;
    let version = value.get("v").and_then(serde_json::Value::as_u64);
    let error = value.get("error");
    let field = |key: &str| error.and_then(|error| error.get(key));
    if let Some(code) = field("code").and_then(serde_json::Value::as_str)
        // 别的版本的应答里只认「不支持这个版本」（见 parse_reply）
        && (version == Some(u64::from(IPC_VERSION)) || code == error_code::UNSUPPORTED_VERSION)
    {
        return Err(IpcError::Remote {
            code: code.to_string(),
            message: field("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
            details: field("details").cloned().unwrap_or(serde_json::Value::Null),
        });
    }
    if version != Some(u64::from(IPC_VERSION)) {
        return Err(IpcError::Protocol(match version {
            Some(v) => format!(
                "实例使用控制协议 v{v}，本程序只懂 v{IPC_VERSION}：请用与实例同版本的 aproxy 操作它"
            ),
            None => "应答没有协议版本 v".to_string(),
        }));
    }
    let ok = value.get("ok").and_then(serde_json::Value::as_bool) == Some(true);
    match value.get_mut("result") {
        Some(result) if ok => Ok(result.take()),
        _ => Err(IpcError::Protocol("应答缺少 result 或 error".to_string())),
    }
}

/// 解析一行 v1 应答。
pub fn parse_reply(line: &str) -> Result<InstanceStatus, IpcError> {
    let value: serde_json::Value = serde_json::from_str(line)
        .map_err(|e| IpcError::Protocol(format!("应答不是 JSON：{e}")))?;
    // 先只看 `v`：别的版本的应答可能改了其余字段的形状，按 v1 的结构解析只会报出一句
    // 「格式不符」，说不出真正的原因
    let version = value.get("v").and_then(serde_json::Value::as_u64);
    if version != Some(u64::from(IPC_VERSION)) {
        // 别的版本的应答里只认一种：实例说它不支持请求的版本。这个错误的形状各版本都
        // 一样（它本来就是说给不同版本的对端听的），`details.supported` 列出实例支持哪些
        if let Some(error) = value.get("error")
            && error.get("code").and_then(serde_json::Value::as_str)
                == Some(error_code::UNSUPPORTED_VERSION)
        {
            return Err(IpcError::Remote {
                code: error_code::UNSUPPORTED_VERSION.to_string(),
                message: error
                    .get("message")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                details: error
                    .get("details")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            });
        }
        return Err(IpcError::Protocol(match version {
            Some(v) => format!(
                "实例使用控制协议 v{v}，本程序只懂 v{IPC_VERSION}：请用与实例同版本的 aproxy 操作它"
            ),
            None => "应答没有协议版本 v".to_string(),
        }));
    }
    let reply: WireResponse = serde_json::from_value(value)
        .map_err(|e| IpcError::Protocol(format!("应答格式不符：{e}")))?;
    match (reply.ok, reply.result, reply.error) {
        (true, Some(status), _) => Ok(status),
        (false, _, Some(error)) => Err(IpcError::Remote {
            code: error.code,
            message: error.message,
            details: error.details,
        }),
        _ => Err(IpcError::Protocol("应答缺少 result 或 error".to_string())),
    }
}
/// 在已建立的连接上完成一次「发请求行、收响应行」。
async fn exchange_over<S>(stream: S, req_line: &str) -> Result<String, IpcError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    // 应答行长度上限：端点名可以预测，应答者未必是 aProxy，不能让它把客户端
    // 内存撑爆。快照只有几百字节到几千字节；上限留给以后一行带一批请求记录的 op
    const MAX_RESPONSE_LINE: u64 = 16 * 1024 * 1024;
    let failed = |e: io::Error| IpcError::Transient(e.to_string());
    let (reader, mut writer) = tokio::io::split(stream);
    writer
        .write_all(req_line.as_bytes())
        .await
        .map_err(failed)?;
    writer.write_all(b"\n").await.map_err(failed)?;
    let mut line = String::new();
    BufReader::new(reader)
        .take(MAX_RESPONSE_LINE)
        .read_line(&mut line)
        .await
        .map_err(failed)?;
    if !line.ends_with('\n') {
        return Err(if line.len() as u64 >= MAX_RESPONSE_LINE {
            IpcError::Protocol("应答超过长度上限".to_string())
        } else {
            IpcError::Transient("连接在应答完整之前关闭".to_string())
        });
    }
    Ok(line)
}

#[cfg(windows)]
mod imp {
    use super::*;
    use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};
    /// 端点不存在（无实例）时返回可读错误。成功时一并返回管道服务端的进程
    /// pid（取不到为 None），供调用方核对应答者自报的 pid。
    ///
    /// ERROR_PIPE_BUSY(231) 重试：serve 循环在 connect() 完成、重建下一个
    /// 监听实例之间存在零监听窗口，管道名存在但无空闲实例，此时 CreateFile
    /// 返回 busy 而非「端点不存在」。tokio 文档明确要求客户端对该错误
    /// sleep 后重试；封顶 2 秒（上层 ipc_request 的 3 秒超时之内）。
    pub async fn exchange(
        endpoint: &str,
        req_line: &str,
    ) -> Result<(String, Option<u32>), IpcError> {
        let (client, peer) = connect(endpoint).await?;
        Ok((exchange_over(client, req_line).await?, peer))
    }

    /// 客户端一侧的连接
    pub type Stream = NamedPipeClient;

    /// 连上端点。一并返回管道服务端的进程 pid（取不到为 None）。重试的规矩见 [`exchange`]。
    pub async fn connect(endpoint: &str) -> Result<(Stream, Option<u32>), IpcError> {
        const ERROR_PIPE_BUSY: i32 = 231;
        const BUSY_RETRY_CAP: Duration = Duration::from_secs(2);
        const BUSY_RETRY_INTERVAL: Duration = Duration::from_millis(50);
        let deadline = tokio::time::Instant::now() + BUSY_RETRY_CAP;
        let client = loop {
            match ClientOptions::new().open(endpoint) {
                Ok(client) => break client,
                Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                    if tokio::time::Instant::now() >= deadline {
                        return Err(IpcError::Transient(format!("无法连接（{e}）")));
                    }
                    tokio::time::sleep(BUSY_RETRY_INTERVAL).await;
                }
                // 管道名不存在：确定没有实例在这个端点上听
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    return Err(IpcError::Unreachable(format!("无法连接（{e}）")));
                }
                Err(e) => return Err(IpcError::Transient(format!("无法连接（{e}）"))),
            }
        };
        let peer = server_pid(&client);
        Ok((client, peer))
    }

    fn server_pid(client: &NamedPipeClient) -> Option<u32> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::Pipes::GetNamedPipeServerProcessId;
        let mut pid = 0u32;
        // SAFETY: 句柄来自仍存活的 client，pid 指向本栈上的 u32
        let ok = unsafe { GetNamedPipeServerProcessId(client.as_raw_handle() as isize, &mut pid) };
        (ok != 0 && pid != 0).then_some(pid)
    }
}

#[cfg(unix)]
mod imp {
    use super::*;
    use std::path::Path;
    /// 成功时一并返回对端进程 pid（SO_PEERCRED 一类机制；取不到为 None），
    /// 供调用方核对应答者自报的 pid。
    pub async fn exchange(
        endpoint: &str,
        req_line: &str,
    ) -> Result<(String, Option<u32>), IpcError> {
        let (client, peer) = connect(endpoint).await?;
        Ok((exchange_over(client, req_line).await?, peer))
    }

    /// 客户端一侧的连接
    pub type Stream = tokio::net::UnixStream;

    /// 连上端点，核对在那里听的进程该不该信。一并返回对端进程 pid（取不到为 None）。
    pub async fn connect(endpoint: &str) -> Result<(Stream, Option<u32>), IpcError> {
        let client = tokio::net::UnixStream::connect(endpoint)
            .await
            .map_err(|e| {
                // socket 文件不存在，或文件还在但没有进程在听（崩溃残留）：
                // 确定没有实例在这个端点上
                let msg = format!("无法连接（{e}）");
                match e.kind() {
                    io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused => {
                        IpcError::Unreachable(msg)
                    }
                    _ => IpcError::Transient(msg),
                }
            })?;
        let cred = client.peer_cred().ok();
        // 在这个路径上听的进程必须是该信的用户，否则不发请求、也不读它的应答：socket
        // 的路径可以预测，目录权限过宽时别的用户能抢先在那里放一个自己的 socket
        if let Some(server) = cred.as_ref().map(|cred| cred.uid()) {
            // SAFETY: geteuid 没有前置条件，也不会失败
            let me = unsafe { libc::geteuid() };
            let dir_owner = Path::new(endpoint)
                .parent()
                .and_then(|dir| std::fs::metadata(dir).ok())
                .map(|meta| std::os::unix::fs::MetadataExt::uid(&meta));
            if !trusted_server_uid(me, server, dir_owner) {
                return Err(IpcError::Protocol(format!(
                    "控制端点上的进程属于另一个用户（uid {server}），不予采信"
                )));
            }
        }
        let peer = cred
            .and_then(|cred| cred.pid())
            .and_then(|pid| u32::try_from(pid).ok())
            .filter(|&pid| pid != 0);
        Ok((client, peer))
    }

    /// 客户端该不该信在控制端点上听的进程。`me` 是客户端的有效 uid，`server` 是对端的，
    /// `dir_owner` 是 socket 所在目录（run 目录，归 home 的属主）的属主。
    /// - 普通用户：只信自己的进程；
    /// - root（`sudo aproxy stop`）：信 home 属主的进程，也信 root 自己的。
    pub(super) fn trusted_server_uid(me: u32, server: u32, dir_owner: Option<u32>) -> bool {
        if me != 0 {
            return server == me;
        }
        server == 0 || dir_owner == Some(server)
    }

    #[cfg(test)]
    #[test]
    fn peers_are_trusted_by_uid() {
        // 普通用户作客户端
        assert!(trusted_server_uid(1000, 1000, Some(1000)));
        assert!(!trusted_server_uid(1000, 1001, Some(1001)));
        assert!(
            !trusted_server_uid(1000, 0, Some(1000)),
            "root 的实例普通用户不认"
        );
        // root 作客户端
        assert!(trusted_server_uid(0, 1000, Some(1000)), "home 属主的实例");
        assert!(trusted_server_uid(0, 0, Some(1000)), "root 自己的实例");
        assert!(
            !trusted_server_uid(0, 1001, Some(1000)),
            "别人放在这个目录里的"
        );
        assert!(!trusted_server_uid(0, 1001, None), "目录属主读不出来时不认");
    }
}
