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

// 溢写之前攒下的那一段正好这么大；请求历史循环使用的大缓冲区按同一个大小准备
const _: () = assert!(RESIDENT_LIMIT == crate::history::LARGE_BUFFER);

/// 阻塞解码不能靠丢弃 JoinHandle 取消；每次读原体、写输出前看一次客户端是否仍在等待。
struct ConnectedIo<'a, T, F> {
    /// 压缩输入或可溢写的输出，两边都可能先于对方出现大量小块。
    io: T,
    /// 使用外层 future 所持的接收端的生命期，不跨线程共享正文。
    cancelled: &'a F,
}
impl<T: std::io::Read, F: Fn() -> bool> std::io::Read for ConnectedIo<'_, T, F> {
    /// 非 Interrupted 的错误让 copy 停止，不会在客户端已经断开后重新读。
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if (self.cancelled)() {
            return Err(std::io::ErrorKind::BrokenPipe.into());
        }
        self.io.read(buf)
    }
}
impl<T: std::io::Write, F: Fn() -> bool> std::io::Write for ConnectedIo<'_, T, F> {
    /// 解码输出也逐块检查：压缩输入可能已经全被预读。
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if (self.cancelled)() {
            return Err(std::io::ErrorKind::BrokenPipe.into());
        }
        self.io.write(buf)
    }
    /// 向底层交付已接受的数据，不重新启动读取。
    fn flush(&mut self) -> std::io::Result<()> {
        self.io.flush()
    }
}

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

/// 读取请求体错误。三种原因各有各的终态码，归因不能混：客户端没发完不是本机的故障，
/// 本机磁盘写不进去也不是客户端的问题。
pub(super) enum ReadBodyError {
    /// 超出上限（413）
    TooLarge,
    /// 客户端在上传途中断开或出错（400）
    ClientAborted(std::io::Error),
    /// 请求体写进磁盘缓存失败（507）
    Storage(std::io::Error),
}

/// 读取请求体：内存缓冲，超 `RESIDENT_LIMIT` 且 disk_cache 可用则溢写磁盘。
/// `limit` 超出返回 TooLarge；客户端中断返回 ClientAborted，写盘失败返回 Storage。
///
/// 溢写文件在交给 `RequestBody::Disk` 之前由 `SpoolPath` 持有：显式的出错分支
/// 之外，客户端在上传途中断开时 hyper 会直接 drop 整个 handler future（本函数
/// 停在某个 await 上、不会走到任何返回分支），守卫的 Drop 照样删掉半截文件，
/// 不留 `req-*.spooltmp` 等到下次启动才回收。
///
/// `history` 是请求历史里这个请求体的写者（不保留体时为 None）。留在内存里的请求体读完
/// 之后整份交过去，与重试重放共用同一份内存；溢写磁盘的边读边交。
///
/// 一并返回请求体顶层的 `model`（不是 JSON 对象、没有这个键或不是字符串时为 None）。溢写
/// 磁盘的请求体在溢写的那一刻、开头一段还在内存里时读：`model` 通常是头几个键之一，这样
/// 读到了就不必为它再打开一次文件——在会扫描新文件的系统上，多开一次文件是毫秒级的开销。
/// 开头一段里没有时才从文件读。
pub(super) async fn read_request_body(
    body: axum::body::Body,
    limit: usize,
    spool_dir: Option<&Path>,
    mut history: Option<crate::history::BodyWriter>,
    mut digest: Option<&mut sha2::Sha256>,
) -> Result<(RequestBody, Option<String>), ReadBodyError> {
    // 客户端声明了长度时按它预留（封顶到内存驻留阈值）：缓冲不必一次次翻倍搬家，读完时
    // 容量正好等于长度——这份内存接着要与请求历史共用，多出来的容量会白白算在历史头上。
    // 预留不超过 1 MiB，声明一个很大的长度却什么都不发的客户端占不到更多
    let declared = axum::body::HttpBody::size_hint(&body).upper();
    let reserve = declared.map_or(0, |len| len.min(RESIDENT_LIMIT as u64) as usize);
    let mut body = body.into_data_stream();
    let reserve = reserve.min(limit);
    // 进历史的请求体向历史要缓冲：大请求体溢写之前的这一段，历史那边是循环使用的
    let mut mem: Vec<u8> = match history.as_ref() {
        Some(history) => history.buffer(reserve),
        None => Vec::with_capacity(reserve),
    };
    // 落盘状态：写句柄 + 删除责任守卫 + 已写字节数。各出错分支直接 return，
    // 守卫随之 drop 删除文件
    let mut disk: Option<(tokio::fs::File, SpoolPath, u64)> = None;
    // 溢写磁盘的请求体：溢写时从内存里的开头一段读到的 `model`
    let mut head_model: Option<String> = None;
    loop {
        let chunk = match body.next().await {
            Some(Ok(c)) => c,
            Some(Err(e)) => return Err(ReadBodyError::ClientAborted(std::io::Error::other(e))),
            None => break,
        };
        // 上限判定：磁盘模式下基于累计总量；内存模式基于 Vec 长度
        let total = disk.as_ref().map_or(mem.len(), |(_, _, l)| *l as usize);
        if limit != usize::MAX && total + chunk.len() > limit {
            drain_rejected_body(&mut body).await;
            return Err(ReadBodyError::TooLarge);
        }
        // 散列的是客户端原始字节，不是后面的解码体、代理改写体或历史里保留的副本。
        if let Some(h) = digest.as_deref_mut() {
            sha2::Digest::update(h, &chunk);
        }
        match &mut disk {
            Some((file, _, len)) => {
                if file.write_all(&chunk).await.is_err() {
                    return Err(ReadBodyError::Storage(std::io::Error::other(
                        "writing the request body to the disk cache failed",
                    )));
                }
                *len += chunk.len() as u64;
                if let Some(history) = history.as_mut() {
                    history.feed_copy(&chunk);
                }
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
                            head_model = model_in_slice(&mem);
                            // 已经写进文件的这一段内存不再需要：整块交给请求历史，不拷贝。
                            // 先用这一块的开头把它没用上的容量填满（这些字节本来也要拷进
                            // 历史），交过去的缓冲就没有空着的容量算在历史头上
                            let mut spilled = std::mem::take(&mut mem);
                            if let Some(history) = history.as_mut() {
                                let head = chunk.len().min(spilled.capacity() - spilled.len());
                                spilled.extend_from_slice(&chunk[..head]);
                                history.feed_buffer(spilled);
                                history.feed_copy(&chunk[head..]);
                            }
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
                return Err(ReadBodyError::Storage(std::io::Error::other(
                    "flushing the request body to the disk cache failed",
                )));
            }
            drop(file);
            // 删除责任从守卫移交给 RequestBody（其 Drop 负责删文件）
            let body = RequestBody::Disk {
                path: path.into_path(),
                len,
            };
            let model = match (head_model, &body) {
                (None, RequestBody::Disk { path, .. }) => model_in_file(path).await,
                (model, _) => model,
            };
            (body, model)
        }
        None => {
            // 请求历史与重试重放共用这一份内存。进历史的要把没用上的容量还回去：没声明长度
            // 的请求体，缓冲是翻倍长出来的，不还的话历史实际占着的内存会比记在预算上的多出
            // 近一倍。声明了长度的容量本来就正好，这里什么都不做
            if history.is_some() {
                mem.shrink_to_fit();
            }
            let bytes = Bytes::from(mem);
            if let Some(history) = history.as_mut() {
                history.feed(&bytes);
            }
            let model = model_in_slice(&bytes);
            (RequestBody::Memory(bytes), model)
        }
    })
}

impl RequestBody {
    pub fn is_empty(&self) -> bool {
        match self {
            RequestBody::Memory(b) => b.is_empty(),
            RequestBody::Disk { len, .. } => *len == 0,
        }
    }

    /// 在阻塞线程里严格解码原体；超出驻留阈值就溢写。失败保留原字节，不交出半份解码结果。
    pub(super) fn decode_for_format(
        self,
        encoding: &str,
        dir: Option<PathBuf>,
        limit: u64,
        cancelled: impl Fn() -> bool,
    ) -> (Self, bool) {
        if !format_encoding_depth_allowed(encoding) {
            return (self, false);
        }
        let mut out = SpillWriter::Memory {
            buf: Vec::new(),
            dir,
        };
        let mut decode = |input: &mut dyn std::io::Read| {
            crate::decode::decode_strict(
                encoding,
                ConnectedIo {
                    io: input,
                    cancelled: &cancelled,
                },
                &mut ConnectedIo {
                    io: &mut out,
                    cancelled: &cancelled,
                },
                limit,
            )
            .is_ok()
        };
        let decoded = match &self {
            Self::Memory(bytes) => decode(&mut bytes.as_ref()),
            Self::Disk { path, .. } => {
                std::fs::File::open(path).is_ok_and(|mut file| decode(&mut file))
            }
        };
        if decoded && let Ok(mut body) = out.into_body() {
            let body = match &mut body {
                SpooledBody::Memory(bytes) => Self::Memory(std::mem::take(bytes)),
                SpooledBody::Disk { path, len } => Self::Disk {
                    path: std::mem::take(path),
                    len: *len,
                },
            };
            (body, true)
        } else {
            (self, false)
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

/// 尚未读取的上传和决定客户端视角的原始头。请求链改写路由或头也不能改变这份输入。
pub(super) struct ClientUpload {
    /// 网络流只消费一次；需要重放时改用内存或磁盘体。
    pub(super) body: Body,
    /// 只按实例的初始触发器判定 client.stream，不接受程序反向改写。
    pub(super) trigger: crate::config::KeepaliveTrigger,
    /// 入站 Accept 的 SSE 标记，不看目标服务器的头覆盖。
    pub(super) accept: bool,
    /// 客户端发来的编码链，供请求历史按原始字节解码。
    pub(super) encoding: Option<HeaderValue>,
}
impl ClientUpload {
    /// 在重放、正文或摘要真正需要时接管网络流；先按原始字节记历史与摘要，再供各层改写。
    pub(super) async fn buffer(
        self,
        state: &AppState,
        req: &mut OutboundRequest,
        rec: &crate::history::Recorder,
    ) -> Result<bool, Terminal> {
        use crate::config::file::Stage;
        let formatted = req.ctx.is_some();
        let history = rec.request_body(request_history_encoding(self.encoding.as_ref(), formatted));
        let limit = state.config.body_limit_bytes();
        let mut digest = (formatted
            && ([Stage::Request, Stage::Retry, Stage::Decide]
                .into_iter()
                .any(|stage| state.has_format(stage))
                || state.formats.iter().any(|p| p.wants_notify_digest())))
        .then(sha2::Sha256::default);
        let (body, model) = read_request_body(self.body, limit, state.spool_dir.as_deref(), history, digest.as_mut()).await.map_err(|e| match e {
            ReadBodyError::TooLarge => body_too_large(limit),
            ReadBodyError::ClientAborted(e) => {
                tracing::error!(code = %"client_body_aborted", error = %e, "读取请求体失败：客户端在上传途中断开");
                Terminal::client_body_aborted()
            }
            ReadBodyError::Storage(e) => {
                let t = Terminal::local_storage_failed(StorageOp::WriteRequest, &e.to_string());
                t.record_in(&state.stats);
                tracing::error!(code = %t.code(), error = %e, "请求体写入磁盘缓存失败");
                t
            }
        })?;
        let (stream, flag) = keepalive_triggered(self.trigger, self.accept, &body).await;
        rec.request_bytes(match &body {
            RequestBody::Memory(b) => b.len() as u64,
            RequestBody::Disk { len, .. } => *len,
        });
        rec.request_shape(model, stream);
        req.body = body;
        req.body_stream = flag;
        if let Some(ctx) = &mut req.ctx {
            ctx.client.stream = Some(stream);
            if let Some(out) = &ctx.live_stream {
                let _ = out.set(stream);
            }
            if let Some(hash) = digest {
                ExchangeCtx::finish_digest(&ctx.digest, hash);
            }
        }
        Ok(stream)
    }
}

/// 请求体超限的 413 终态：判定、日志与响应收敛在一处，缓冲路径与仅转发路径共用。
pub(super) fn body_too_large(body_limit: usize) -> Terminal {
    tracing::warn!(
        code = %"request_too_large",
        limit_bytes = body_limit,
        "请求体超出上限"
    );
    Terminal::request_too_large(body_limit)
}
