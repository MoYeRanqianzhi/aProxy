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
    /// The endpoint now belongs to a different run than the reader selected.
    RunChanged,
    /// An acknowledgment or event did not fit in its bounded frame.
    LineTooLong,
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
            Self::LineTooLong => f.write_str("应答超过长度上限"),
            Self::RunChanged => f.write_str("instance run changed"),
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

/// Process and protocol identity of one run. A port or a PID alone can be reused.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunIdentity {
    /// PID obtained from the transport, not just the payload.
    pub pid: u32,
    /// Opaque run identifier in the verified snapshot.
    pub instance_id: String,
}

impl RunIdentity {
    fn check(&self, expected: Option<&Self>) -> Result<(), IpcError> {
        if expected.is_some_and(|run| run != self) {
            Err(IpcError::RunChanged)
        } else {
            Ok(())
        }
    }

    fn verify_record(&self, record: &serde_json::Value) -> Result<(), IpcError> {
        let (run, _) = record["request_id"]
            .as_str()
            .and_then(|id| id.rsplit_once('-'))
            .ok_or_else(|| IpcError::Protocol("invalid request identity".into()))?;
        if run == self.instance_id {
            Ok(())
        } else {
            Err(IpcError::RunChanged)
        }
    }
}

fn snapshot_run(snapshot: &serde_json::Value, peer: u32) -> Result<RunIdentity, IpcError> {
    let record: InstanceRecord = serde_json::from_value(snapshot["instance"].clone())
        .map_err(|e| IpcError::Protocol(format!("应答格式不符：{e}")))?;
    verify_pid(record.pid, peer)?;
    Ok(RunIdentity {
        pid: peer,
        instance_id: record.instance_id,
    })
}

fn verify_pid(claimed: u32, peer: u32) -> Result<(), IpcError> {
    if claimed != peer {
        Err(IpcError::Protocol(format!(
            "应答者自报 pid {claimed} 与连接对端进程 {peer} 不一致"
        )))
    } else {
        Ok(())
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
    verify_pid(status.instance.pid, peer).map_err(|e| e.at(endpoint))?;
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
    read_for_run_in(run_dir, port, op, None)
        .await
        .map(|(_, result)| result)
}

/// Read history and return the run that supplied it. If `expected` is set, a
/// replacement on the same port is an error, even if its PID was recycled.
pub async fn read_for_run_in(
    run_dir: &std::path::Path,
    port: &str,
    op: &ReadOp,
    expected: Option<&RunIdentity>,
) -> Result<(RunIdentity, serde_json::Value), IpcError> {
    let endpoint = endpoint_for_in(run_dir, port);
    let exchange = async {
        // History replies have no identity envelope. Keep their connection open
        // while asking the endpoint for a snapshot, then compare both peers before
        // sending the operation. This also works for body chunks and empty pages.
        let (stream, peer) = imp::connect(&endpoint).await?;
        if expected.is_some_and(|run| run.pid != peer) {
            return Err(IpcError::RunChanged);
        }
        let (ping, ping_peer) = imp::exchange(&endpoint, &IpcOp::Ping.request_line()).await?;
        if ping_peer != peer {
            return Err(IpcError::RunChanged);
        }
        let record = parse_reply(&ping)?.instance;
        verify_pid(record.pid, peer)?;
        let run = RunIdentity {
            pid: peer,
            instance_id: record.instance_id,
        };
        run.check(expected)?;
        let result = parse_result(&exchange_over(stream, &op.request_line()).await?)?;
        Ok((run, result))
    };
    match tokio::time::timeout(Duration::from_secs(3), exchange).await {
        Ok(result) => result.map_err(|e| e.at(&endpoint)),
        Err(_) => Err(IpcError::Transient(format!("{endpoint}: 请求超时"))),
    }
}

/// Consume at most `limit` bytes including the newline. `read_until` preserves
/// partial bytes in `buffer` if the consumer cancels and later resumes `next`.
async fn subscription_line<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
    buffer: &mut Vec<u8>,
    limit: usize,
) -> Result<bool, IpcError> {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt};
    reader
        .take((limit - buffer.len()) as u64)
        .read_until(b'\n', buffer)
        .await
        .map_err(|error| IpcError::Transient(error.to_string()))?;
    if buffer.last() == Some(&b'\n') {
        Ok(true)
    } else if buffer.len() == limit {
        Err(IpcError::LineTooLong)
    } else {
        Ok(false)
    }
}

/// 一条订阅连接：[`ipc_subscribe_in`] 的返回值，之后每调一次 [`EventStream::next`] 读一个
/// 事件。丢掉它就是退订。
pub struct EventStream {
    run: RunIdentity,
    buffer: Vec<u8>,
    lines: Option<tokio::io::BufReader<imp::Stream>>,
}

impl EventStream {
    /// Identity verified at the opening acknowledgment; reconnecting creates a different stream.
    pub fn run(&self) -> &RunIdentity {
        &self.run
    }

    /// 下一个事件：`{"v":1,"event":…,"seq":n,"at":毫秒,"data":…}`。实例关了连接时为
    /// `Ok(None)`。45 秒收不到任何一行按失联报 `Transient`（实例至少每 15 秒发一行）。
    ///
    /// 不认识的事件照样返回，由调用方忽略；`lagged` 之后调用方该重取快照与记录。
    pub async fn next(&mut self) -> Result<Option<serde_json::Value>, IpcError> {
        const SILENCE: Duration = Duration::from_secs(45);
        let Some(lines) = &mut self.lines else {
            return Ok(None);
        };
        let read = tokio::time::timeout(
            SILENCE,
            subscription_line(lines, &mut self.buffer, crate::wire::MAX_REPLY_LINE),
        )
        .await
        .map_err(|_| IpcError::Transient("订阅连接 45 秒没有收到任何事件".to_string()))
        .and_then(|r| r);
        match read {
            Ok(true) => {}
            end => {
                self.lines = None;
                self.buffer = Vec::new();
                return end.map(|_| None);
            }
        }
        let event = serde_json::from_slice::<serde_json::Value>(&self.buffer)
            .map_err(|e| IpcError::Protocol(format!("事件不是 JSON：{e}")));
        self.buffer.clear();
        let event = event?;
        let identity = match event["event"].as_str() {
            Some("state") => snapshot_run(&event["data"], self.run.pid)
                .and_then(|run| run.check(Some(&self.run))),
            Some("request") => self.run.verify_record(&event["data"]),
            _ => Ok(()),
        };
        if let Err(error) = identity {
            self.lines = None;
            return Err(error);
        }
        Ok(Some(event))
    }
}

/// 订阅实例的事件。返回订阅那一刻的 `{"snapshot":…,"requests":[在途的记录]}` 与事件流。
/// 实例的流式连接已满时是 `Remote`，码为 `busy`。
pub async fn ipc_subscribe_in(
    run_dir: &std::path::Path,
    port: &str,
) -> Result<(serde_json::Value, EventStream), IpcError> {
    subscribe_for_run_in(run_dir, port, None).await
}

/// Subscribe only to the selected run (or discover it from the acknowledgment).
/// A rejected opening never exposes the snapshot or any buffered events.
pub async fn subscribe_for_run_in(
    run_dir: &std::path::Path,
    port: &str,
    expected: Option<&RunIdentity>,
) -> Result<(serde_json::Value, EventStream), IpcError> {
    use tokio::io::AsyncWriteExt;
    let endpoint = endpoint_for_in(run_dir, port);
    let open = async {
        let (mut stream, peer) = imp::connect(&endpoint).await?;
        if expected.is_some_and(|run| run.pid != peer) {
            return Err(IpcError::RunChanged);
        }
        let failed = |e: io::Error| IpcError::Transient(e.to_string());
        let request = ReadOp::Subscribe.request_line();
        stream.write_all(request.as_bytes()).await.map_err(failed)?;
        stream.write_all(b"\n").await.map_err(failed)?;
        let mut lines = tokio::io::BufReader::new(stream);
        let mut line = Vec::new();
        if !subscription_line(&mut lines, &mut line, crate::wire::MAX_REPLY_LINE).await? {
            return Err(IpcError::Transient("连接在应答完整之前关闭".to_string()));
        }
        let response = parse_result(
            std::str::from_utf8(&line).map_err(|e| IpcError::Protocol(e.to_string()))?,
        );
        // An error frame has no run ID. Keep this connection open while checking
        // the peer of a fresh ping before assigning a refusal to a selected run.
        if let (Some(expected), Err(IpcError::Remote { .. })) = (expected, &response) {
            let status = request_raw(&endpoint, IpcOp::Ping).await?;
            RunIdentity {
                pid: status.instance.pid,
                instance_id: status.instance.instance_id,
            }
            .check(Some(expected))?;
        }
        let ack = response?;
        let run = snapshot_run(&ack["snapshot"], peer)?;
        run.check(expected)?;
        let records = ack["requests"]
            .as_array()
            .ok_or_else(|| IpcError::Protocol("invalid subscription acknowledgment".into()))?;
        for record in records {
            run.verify_record(record)?;
        }
        Ok((
            ack,
            EventStream {
                run,
                lines: Some(lines),
                buffer: Vec::new(),
            },
        ))
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
    let failed = |e: io::Error| IpcError::Transient(e.to_string());
    let (reader, mut writer) = tokio::io::split(stream);
    writer
        .write_all(req_line.as_bytes())
        .await
        .map_err(failed)?;
    writer.write_all(b"\n").await.map_err(failed)?;
    let mut line = String::new();
    BufReader::new(reader)
        .take(crate::wire::MAX_REPLY_LINE as u64)
        .read_line(&mut line)
        .await
        .map_err(failed)?;
    if !line.ends_with('\n') {
        return Err(if line.len() as u64 >= crate::wire::MAX_REPLY_LINE as u64 {
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
    /// pid（取不到就拒绝），供调用方核对应答者自报的 pid。
    ///
    /// ERROR_PIPE_BUSY(231) 重试：serve 循环在 connect() 完成、重建下一个
    /// 监听实例之间存在零监听窗口，管道名存在但无空闲实例，此时 CreateFile
    /// 返回 busy 而非「端点不存在」。tokio 文档明确要求客户端对该错误
    /// sleep 后重试；封顶 2 秒（上层 ipc_request 的 3 秒超时之内）。
    pub async fn exchange(endpoint: &str, req_line: &str) -> Result<(String, u32), IpcError> {
        let (client, peer) = connect(endpoint).await?;
        Ok((exchange_over(client, req_line).await?, peer))
    }

    /// 客户端一侧的连接
    pub type Stream = NamedPipeClient;

    /// 连上端点。一并返回管道服务端的进程 pid（取不到就拒绝）。重试的规矩见 [`exchange`]。
    pub async fn connect(endpoint: &str) -> Result<(Stream, u32), IpcError> {
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
        let peer = server_pid(&client)
            .ok_or_else(|| IpcError::Protocol("cannot verify peer pid".into()))?;
        verify_server_owner(peer)?;
        Ok((client, peer))
    }

    // A kernel-reported PID identifies the server, not its user. Failure to
    // inspect either token is a verification failure, before the first write.
    fn verify_server_owner(pid: u32) -> Result<(), IpcError> {
        use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
        use windows_sys::Win32::{
            Security::{EqualSid, GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser},
            System::Threading::{
                GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
            },
        };
        let failure = || IpcError::Protocol("cannot verify pipe server owner".into());
        fn user(process: isize) -> Option<Vec<usize>> {
            let mut token = 0;
            // TOKEN_USER points into the same variable-length, pointer-aligned buffer.
            unsafe {
                if OpenProcessToken(process, TOKEN_QUERY, &mut token) == 0 {
                    return None;
                }
                let handle = OwnedHandle::from_raw_handle(token as _);
                let token = handle.as_raw_handle() as isize;
                let mut len = 0;
                GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut len);
                if len < size_of::<TOKEN_USER>() as u32 {
                    return None;
                }
                let mut buf = vec![0usize; (len as usize).div_ceil(size_of::<usize>())];
                (GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast(), len, &mut len) != 0)
                    .then_some(buf)
            }
        }
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle == 0 {
                return Err(failure());
            }
            let process = OwnedHandle::from_raw_handle(handle as _);
            let theirs = user(process.as_raw_handle() as isize).ok_or_else(failure)?;
            let ours = user(GetCurrentProcess()).ok_or_else(failure)?;
            let sid = |buf: &[usize]| (*(buf.as_ptr().cast::<TOKEN_USER>())).User.Sid;
            if EqualSid(sid(&ours), sid(&theirs)) == 0 {
                return Err(failure());
            }
        }
        Ok(())
    }

    #[test]
    fn pipe_server_requires_a_verifiable_current_user() {
        verify_server_owner(std::process::id()).unwrap();
        assert!(matches!(verify_server_owner(0), Err(IpcError::Protocol(_))));
        assert!(matches!(verify_server_owner(4), Err(IpcError::Protocol(_))));
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
    pub async fn exchange(endpoint: &str, req_line: &str) -> Result<(String, u32), IpcError> {
        let (client, peer) = connect(endpoint).await?;
        Ok((exchange_over(client, req_line).await?, peer))
    }

    /// 客户端一侧的连接
    pub type Stream = tokio::net::UnixStream;

    /// 连上端点，核对在那里听的进程该不该信。一并返回对端进程 pid（取不到就拒绝）。
    pub async fn connect(endpoint: &str) -> Result<(Stream, u32), IpcError> {
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
        let cred = client
            .peer_cred()
            .map_err(|error| IpcError::Protocol(error.to_string()))?;
        // 在这个路径上听的进程必须是该信的用户，否则不发请求、也不读它的应答：socket
        // 的路径可以预测，目录权限过宽时别的用户能抢先在那里放一个自己的 socket
        {
            let server = cred.uid();
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
            .pid()
            .and_then(|pid| u32::try_from(pid).ok())
            .filter(|&pid| pid != 0)
            .ok_or_else(|| IpcError::Protocol("cannot verify peer pid".into()))?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn verified_run_checks_peer_run_and_request_identity_separately() {
        let snapshot = json!({"instance": {"pid":17,"start":"s","instance_id":"first","version":"v","listen_addr":"local","base_url":"","started_at":1,"foreground":true}});
        assert!(matches!(
            snapshot_run(&snapshot, 18),
            Err(IpcError::Protocol(_))
        ));
        let first = snapshot_run(&snapshot, 17).unwrap();
        let next = RunIdentity {
            pid: 17,
            instance_id: "second".into(),
        };
        assert!(matches!(
            first.check(Some(&next)),
            Err(IpcError::RunChanged)
        ));
        first
            .verify_record(&json!({"request_id":"first-1"}))
            .unwrap();
        assert!(matches!(
            first.verify_record(&json!({"request_id":"second-1"})),
            Err(IpcError::RunChanged)
        ));
    }
    #[test]
    fn rejected_openings_and_events_close_the_real_connection() {
        use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            for case in ["missing", "replacement", "request", "state", "ack-size", "event-size", "exact"] {
                let dir=tempfile::tempdir().unwrap(); let endpoint=endpoint_for_in(dir.path(), "frames");
                #[cfg(unix)] let listener=tokio::net::UnixListener::bind(&endpoint).unwrap();
                #[cfg(windows)] let listener=tokio::net::windows::named_pipe::ServerOptions::new().first_pipe_instance(true).create(&endpoint).unwrap();
                let pid=std::process::id();
                let server=tokio::spawn(async move {
                    #[cfg(windows)] let mut pipe={listener.connect().await.unwrap();listener};
                    #[cfg(unix)] let mut pipe=listener.accept().await.unwrap().0;
                    let mut request=String::new(); BufReader::new(&mut pipe).read_line(&mut request).await.unwrap();
                    assert_eq!(serde_json::from_str::<serde_json::Value>(&request).unwrap()["op"],"subscribe");
                    let mut instance=json!({"pid":pid,"instance_id":"first","start":"s","version":"v","listen_addr":"local","base_url":"","started_at":1,"foreground":true});
                    if case=="missing" {instance.as_object_mut().unwrap().remove("instance_id");}
                    if case=="replacement" {instance["instance_id"]=json!("second");}
                    let mut ack=json!({"v":1,"ok":true,"result":{"snapshot":{"instance":instance},"requests":[]}}).to_string().into_bytes();
                    if case=="ack-size" {ack.resize(crate::wire::MAX_REPLY_LINE,b' ');} else if case=="exact" {ack.resize(crate::wire::MAX_REPLY_LINE-1,b' ');} ack.push(b'\n');
                    let mut frames=ack;
                    instance["instance_id"]=json!("second");
                    let event=match case {"request"=>Some(json!({"event":"request","data":{"request_id":"second-1"}})),"state"=>Some(json!({"event":"state","data":{"instance":instance}})),"event-size"=>Some(json!({"event":"heartbeat"})), _=>None};
                    if let Some(event)=event {let mut line=event.to_string().into_bytes();if case=="event-size" {line.resize(crate::wire::MAX_REPLY_LINE,b' ');} frames.extend(line);frames.push(b'\n');}
                    let _=pipe.write_all(&frames).await;
                    let mut byte=[0]; assert!(matches!(pipe.read(&mut byte).await,Ok(0)|Err(_)));
                });
                let run=RunIdentity {pid,instance_id:"first".into()};
                let opened=subscribe_for_run_in(dir.path(),"frames",Some(&run)).await;
                match case {
                    "missing"=>assert!(matches!(opened,Err(IpcError::Protocol(_)))),
                    "replacement"=>assert!(matches!(opened,Err(IpcError::RunChanged))),
                    "ack-size"=>assert!(matches!(opened,Err(IpcError::LineTooLong))),
                    "exact"=>drop(opened.unwrap()),
                    _=>{let (_,mut stream)=opened.unwrap(); let result=stream.next().await;
                        if case=="event-size" {assert!(matches!(result,Err(IpcError::LineTooLong)));} else {assert!(matches!(result,Err(IpcError::RunChanged)));}
                        assert!(stream.next().await.unwrap().is_none());
                    }
                }
                server.await.unwrap();
            }
        })
    }

    #[test]
    fn overlong_subscription_line_stops_at_the_limit() {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(async {
                let mut input = &b"12345678\nnext\n"[..];
                let mut buffer = Vec::new();
                assert!(matches!(
                    subscription_line(&mut input, &mut buffer, 8).await,
                    Err(IpcError::LineTooLong)
                ));
                assert_eq!(input, b"\nnext\n");
                buffer.clear();
                assert!(subscription_line(&mut input, &mut buffer, 1).await.unwrap());
                buffer.clear();
                assert!(subscription_line(&mut input, &mut buffer, 5).await.unwrap());
                assert_eq!(buffer, b"next\n");
            });
    }
}
