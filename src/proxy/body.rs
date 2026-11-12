//! 请求体：读取（上限、内存 / 磁盘双模）、超限时排空、413 响应。

use super::*;

// ---------------------------------------------------------------------------
// 双模缓冲：内存（Bytes）与磁盘（临时文件）统一抽象
//
// 收集阶段超内存阈值即溢写磁盘；消费阶段（重试重放/回放给客户端）统一按
// Bytes 块产出。磁盘模式的内存在途占用为流式缓冲级（常数），与负载大小解耦。
// ---------------------------------------------------------------------------

/// 内存驻留阈值：缓冲超过即溢写磁盘（disk_cache 开启时）。
/// 1 MiB：覆盖真实 agent 流量（SSE/JSON 错误体 KB 级）使其零磁盘开销，
/// 大请求体/大响应才进入磁盘路径。
pub(crate) const RESIDENT_LIMIT: usize = 1024 * 1024;

/// 请求体缓冲：支持无限重试的字节重放源。
///
/// - Memory：单份 Bytes，重试间零拷贝共享（小请求体常态路径）
/// - Disk：溢写临时文件，每次重试重新流式读取（在途内存恒定）；
///   Drop 时删除临时文件（重试循环 continue / 客户端断开 / 请求完成全覆盖）
pub enum RequestBody {
    Memory(Bytes),
    Disk {
        path: PathBuf,
        /// 文件总字节数（回填 content-length 用）
        len: u64,
    },
}

/// 请求体超限后「读完再拒」的时长上限：总时长，与两次读到数据之间的空闲时长。
///
/// 为什么要读完：一发现超限就回 413、关掉一个还有未读请求数据的连接，对端 TCP
/// 会收到复位（Windows 上必然，Linux 上取决于时机），客户端接收缓冲里已经到达的
/// 413 随之被丢弃——它看到的是「连接被中止」，看不到该去调 max_body_mb，还可能
/// 原样重发。所以先把剩余的请求体读掉丢弃，客户端发完再回 413（HTTP 服务器常见
/// 的 lingering close）。读多久有上限：到了照常回 413，不为一个注定被拒的请求
/// 无限期占着连接。
pub(super) const REJECTED_BODY_DRAIN_TOTAL: Duration = Duration::from_secs(10);

pub(super) const REJECTED_BODY_DRAIN_IDLE: Duration = Duration::from_secs(2);

/// 把被拒请求体的剩余部分读掉丢弃，直到读完、出错或到了上限
/// （见 [`REJECTED_BODY_DRAIN_TOTAL`]）。只读不存，内存占用与剩余大小无关。
pub(super) async fn drain_rejected_body<S, E>(stream: &mut S)
where
    S: Stream<Item = Result<Bytes, E>> + Unpin,
{
    let deadline = tokio::time::Instant::now() + REJECTED_BODY_DRAIN_TOTAL;
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            return;
        }
        match tokio::time::timeout(left.min(REJECTED_BODY_DRAIN_IDLE), stream.next()).await {
            Ok(Some(Ok(_))) => {}
            _ => return,
        }
    }
}

/// 读取请求体错误：TooLarge=超出上限（413）；Io=连接中断/磁盘写入失败
pub(super) enum ReadBodyError {
    TooLarge,
    Io(std::io::Error),
}

/// 读取请求体：内存缓冲，超 `RESIDENT_LIMIT` 且 disk_cache 可用则溢写磁盘。
/// `limit` 超出返回 TooLarge；客户端中断或写盘失败返回 Io。
///
/// 溢写文件在交给 `RequestBody::Disk` 之前由 `SpoolPath` 持有：显式的出错分支
/// 之外，客户端在上传途中断开时 hyper 会直接 drop 整个 handler future（本函数
/// 停在某个 await 上、不会走到任何返回分支），守卫的 Drop 照样删掉半截文件，
/// 不留 `req-*.spooltmp` 等到下次启动才回收。
pub(super) async fn read_request_body(
    body: axum::body::Body,
    limit: usize,
    spool_dir: Option<&Path>,
) -> Result<RequestBody, ReadBodyError> {
    let mut body = body.into_data_stream();
    let mut mem: Vec<u8> = Vec::new();
    // 落盘状态：写句柄 + 删除责任守卫 + 已写字节数。各出错分支直接 return，
    // 守卫随之 drop 删除文件
    let mut disk: Option<(tokio::fs::File, SpoolPath, u64)> = None;
    loop {
        let chunk = match body.next().await {
            Some(Ok(c)) => c,
            Some(Err(e)) => return Err(ReadBodyError::Io(std::io::Error::other(e))),
            None => break,
        };
        // 上限判定：磁盘模式下基于累计总量；内存模式基于 Vec 长度
        let total = disk.as_ref().map_or(mem.len(), |(_, _, l)| *l as usize);
        if limit != usize::MAX && total + chunk.len() > limit {
            drain_rejected_body(&mut body).await;
            return Err(ReadBodyError::TooLarge);
        }
        match &mut disk {
            Some((file, _, len)) => {
                if file.write_all(&chunk).await.is_err() {
                    return Err(ReadBodyError::Io(std::io::Error::other(
                        "请求体磁盘缓存写入失败",
                    )));
                }
                *len += chunk.len() as u64;
            }
            None => {
                if mem.len() + chunk.len() > RESIDENT_LIMIT {
                    // 溢写决策：有目录才落盘；无目录（disk_cache 关闭/目录不可用）
                    // 继续内存缓冲（limit 语义不受影响）
                    if let Some(dir) = spool_dir {
                        let path = SpoolPath(dir.join(format!(
                            "req-{}-{}.spooltmp",
                            std::process::id(),
                            unique_seq()
                        )));
                        // 目录不可用（create 失败）：内存退化。写入失败时 file 与
                        // 守卫一起在本块末尾 drop——半截文件随之删除
                        if let Ok(mut file) = tokio::fs::File::create(&path.0).await
                            && file.write_all(&mem).await.is_ok()
                            && file.write_all(&chunk).await.is_ok()
                        {
                            disk = Some((file, path, (mem.len() + chunk.len()) as u64));
                            mem = Vec::new();
                            continue;
                        }
                    }
                }
                mem.extend_from_slice(&chunk);
            }
        }
    }
    Ok(match disk {
        Some((mut file, path, len)) => {
            // 写完整后 flush 确保后续重试读到全量数据（flush 只推缓冲到 OS，
            // 不等待物理落盘——page cache 一致性由 OS 保证）
            if file.flush().await.is_err() {
                return Err(ReadBodyError::Io(std::io::Error::other(
                    "请求体磁盘缓存写入失败",
                )));
            }
            drop(file);
            // 删除责任从守卫移交给 RequestBody（其 Drop 负责删文件）
            RequestBody::Disk {
                path: path.into_path(),
                len,
            }
        }
        None => RequestBody::Memory(Bytes::from(mem)),
    })
}

impl RequestBody {
    pub fn is_empty(&self) -> bool {
        match self {
            RequestBody::Memory(b) => b.is_empty(),
            RequestBody::Disk { len, .. } => *len == 0,
        }
    }

    /// 把请求体应用为 reqwest body：内存零拷贝；磁盘流式 + content-length。
    pub async fn apply_to(
        &self,
        mut builder: reqwest::RequestBuilder,
    ) -> Result<reqwest::RequestBuilder, std::io::Error> {
        match self {
            RequestBody::Memory(b) => {
                if !b.is_empty() {
                    builder = builder.body(b.clone());
                }
                Ok(builder)
            }
            RequestBody::Disk { path, len } => {
                let file = tokio::fs::File::open(path).await?;
                let stream = ReaderStream::with_capacity(file, 64 * 1024);
                Ok(builder
                    .header(http::header::CONTENT_LENGTH, len.to_string())
                    .body(reqwest::Body::wrap_stream(stream)))
            }
        }
    }
}

/// Drop 时删除临时文件（重试循环 continue / 客户端断开 / 请求完成全覆盖）
impl Drop for RequestBody {
    fn drop(&mut self) {
        if let RequestBody::Disk { path, .. } = self
            && !path.as_os_str().is_empty()
        {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// 请求体上限的人类可读文案（0 = 不设限 → 「不设限」，否则「N MiB」）。
/// 缓冲路径与仅转发路径的 413 共用，保证两条路径对用户说同一句话。
pub(super) fn limit_text(body_limit: usize) -> String {
    if body_limit == usize::MAX {
        "不设限".to_string()
    } else {
        format!("{} MiB", body_limit / 1024 / 1024)
    }
}

/// 请求体超限的 413 响应：判定、日志与文案收敛在一处，两条路径共用。
pub(super) fn body_too_large_response(body_limit: usize) -> Response {
    let limit_text = limit_text(body_limit);
    tracing::warn!(limit = %limit_text, "请求体超出上限");
    (
        StatusCode::PAYLOAD_TOO_LARGE,
        format!("请求体超出上限（{limit_text}），可在 settings.json 的 max_body_mb 或 config.toml 的 max_body_mb 调整"),
    )
        .into_response()
}
