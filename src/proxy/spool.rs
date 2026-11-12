//! 响应缓冲（spool）：内存 / 磁盘双模缓冲、磁盘回放流、提交后解码用的溢写写端。

use super::*;

/// 尚未交付的 spool 临时文件路径：Drop 时删除文件。缓冲在收集途中被丢弃的
/// 一切路径——客户端断开让在途的 forward_once future 整体被 drop、TooLarge、
/// 读取中断、写盘失败——都经此清理，不留半截 `.spooltmp` 等到下次启动才回收；
/// 正常收尾经 `into_path` 把路径交给 `SpooledBody`（文件从此由它负责）。
///
/// 删除时写句柄可能仍在 tokio 阻塞池的在途写操作里持有：Rust 标准库在 Windows
/// 上以 FILE_SHARE_DELETE 打开文件，删除照样成功（最后一个句柄关闭时文件消失），
/// unix 上 unlink 本就不受打开句柄影响。
pub(super) struct SpoolPath(pub(super) PathBuf);

impl SpoolPath {
    /// 交出路径并解除删除责任（取走后自身成空路径，Drop 不再删）
    pub(super) fn into_path(mut self) -> PathBuf {
        std::mem::take(&mut self.0)
    }
}

impl Drop for SpoolPath {
    fn drop(&mut self) {
        if !self.0.as_os_str().is_empty() {
            let _ = std::fs::remove_file(&self.0);
        }
    }
}

/// 响应 spool 缓冲：内存累积，超 `RESIDENT_LIMIT` 溢写磁盘。
/// 完成后统一为 `SpooledBody` 消费（判定/回放）；中途丢弃时磁盘文件经
/// `SpoolPath` 的 Drop 删除。
pub(super) enum SpoolBuffer {
    Memory(Vec<u8>),
    Disk {
        // 字段顺序即 drop 顺序：先关写句柄，再由 SpoolPath 删文件
        file: tokio::fs::File,
        path: SpoolPath,
        len: u64,
    },
    /// 收集中途写盘失败：内部状态不可用，forward_once 以 SpoolFailed 终态返回
    /// （本地磁盘故障与上游无关，重试无意义）
    Poisoned,
}

impl SpoolBuffer {
    pub(super) fn len(&self) -> u64 {
        match self {
            SpoolBuffer::Memory(mem) => mem.len() as u64,
            SpoolBuffer::Disk { len, .. } => *len,
            SpoolBuffer::Poisoned => 0,
        }
    }

    /// 收集完成：内存 → SpooledBody::Memory（重试判定走整体扫描旧路径，
    /// disk_scan=None）；磁盘 → flush 后关写句柄产出 SpooledBody::Disk +
    /// （增量扫描结论, 头部快照）。消费 scanner。
    ///
    /// Err = 磁盘模式收尾 flush 失败（磁盘满/IO 故障），调用方按 SpoolFailed
    /// 处理；半截文件已在此删除。
    pub(super) async fn finish(
        self,
        scanner: StreamErrorScanner,
    ) -> Result<(SpooledBody, Option<(bool, Vec<u8>)>), String> {
        match self {
            SpoolBuffer::Memory(mem) => Ok((SpooledBody::Memory(Bytes::from(mem)), None)),
            SpoolBuffer::Disk {
                mut file,
                path,
                len,
            } => {
                // drop 前必须 flush：tokio File 的 poll_write 把写操作派发到阻塞池后
                // **立刻**报 Ok，真实写入结果只在下一次 write/flush 时浮现——前面
                // 各块的错误会被后一块的 write_all 捕获（走 Poisoned），但**最后
                // 一块**之后再没有写操作，不 flush 就 drop 会把它的错误连同未完成
                // 的写一起吞掉：回放读到比 len 短的文件，DiskSpoolStream 把提前 EOF
                // 当正常结束，客户端拿到被静默截断的「成功」响应。内存→磁盘切换块
                // 恰是最后一块时同理——切换后本变体就是 Disk，同样由这里兜住。
                // flush 只把写推进 OS（不等物理落盘），与 read_request_body 同款。
                if let Err(e) = file.flush().await {
                    drop(file); // 先关句柄，再由 SpoolPath 的 Drop 删半截文件
                    drop(path);
                    return Err(format!("spool 磁盘写入失败（收尾 flush）: {e}"));
                }
                drop(file); // 关写句柄，头部快照以只读重新打开
                let head = match tokio::fs::File::open(&path.0).await {
                    Ok(mut f) => {
                        let mut buf = vec![0u8; 1024];
                        match f.read(&mut buf).await {
                            Ok(n) => {
                                buf.truncate(n);
                                buf
                            }
                            Err(_) => Vec::new(),
                        }
                    }
                    Err(_) => Vec::new(),
                };
                let error = scanner.finish();
                Ok((
                    SpooledBody::Disk {
                        path: path.into_path(),
                        len,
                    },
                    Some((error, head)),
                ))
            }
            // 不可达：Poisoned 在 forward_once 收集循环中提前返回。防御性产出
            // 空 body 成功（与磁盘判定缺失同样按不重试处理，服务不因内部
            // 不变量被破坏而失能）
            SpoolBuffer::Poisoned => {
                Ok((SpooledBody::Memory(Bytes::new()), Some((false, Vec::new()))))
            }
        }
    }
}

/// 收集一段响应体到 SpoolBuffer（内存累积 + 溢写磁盘）。
/// 磁盘不可用（目录创建失败/写失败）时回退纯内存继续收集——磁盘满不应使
/// 代理失能，最多退化为内存模式；唯有「已落盘后中途写失败」才 Poisoned。
pub(super) async fn spool_chunk(buf: &mut SpoolBuffer, chunk: &[u8], spool_dir: Option<&Path>) {
    match buf {
        SpoolBuffer::Memory(mem) => {
            if mem.len() + chunk.len() <= RESIDENT_LIMIT {
                mem.extend_from_slice(chunk);
                return;
            }
            // 超阈值：已有内存迁移到磁盘；磁盘不可用则留在内存（退化）
            let Some(dir) = spool_dir else {
                mem.extend_from_slice(chunk);
                return;
            };
            // 文件一创建就交给 SpoolPath 守卫：下面两次 write_all（含 1 MiB 的
            // 内存前缀）期间本 future 随时可能被 drop（客户端断开），守卫保证
            // 迁移中途的半截文件同样被删
            let path = SpoolPath(dir.join(format!(
                "spool-{}-{}.spooltmp",
                std::process::id(),
                unique_seq()
            )));
            match tokio::fs::File::create(&path.0).await {
                Ok(mut file) => {
                    if file.write_all(mem).await.is_ok() && file.write_all(chunk).await.is_ok() {
                        let len = (mem.len() + chunk.len()) as u64;
                        *buf = SpoolBuffer::Disk { file, path, len };
                        return;
                    }
                    // 写失败：文件句柄已创建但内容不完整，删除（关句柄后守卫
                    // 随作用域结束删文件）；保持 Memory 退化
                    //（create 成功 write 失败极罕见，但半截文件绝不能留下）
                    drop(file);
                    mem.extend_from_slice(chunk);
                }
                Err(_) => mem.extend_from_slice(chunk),
            }
        }
        SpoolBuffer::Disk { file, len, .. } => {
            // 已落盘后写失败：进程内 spool 无法回退（数据已在盘上），只能
            // Poisoned。被替换掉的 Disk 变体随之 drop：先关句柄、再删半截文件
            if file.write_all(chunk).await.is_err() {
                *buf = SpoolBuffer::Poisoned;
                return;
            }
            *len += chunk.len() as u64;
        }
        SpoolBuffer::Poisoned => {}
    }
}

/// 临时文件名唯一序号：进程内单调即可（同进程不重名；跨进程由 pid 区分）
pub(crate) fn unique_seq() -> u64 {
    use std::sync::atomic::AtomicU64;
    static SEQ: AtomicU64 = AtomicU64::new(0);
    SEQ.fetch_add(1, AtomicOrdering::Relaxed)
}

/// 完整收集后的响应体：消费方式统一（判定 + 回放），磁盘模式自带 Drop 清理
pub enum SpooledBody {
    Memory(Bytes),
    Disk { path: PathBuf, len: u64 },
}

impl Drop for SpooledBody {
    fn drop(&mut self) {
        if let SpooledBody::Disk { path, .. } = self {
            // 消费式回放接管文件后 path 已被置空，此处只删「未消费即丢弃」的
            // （重试 continue / TooLarge 判定后 / 客户端断开）
            if !path.as_os_str().is_empty()
                && let Err(e) = std::fs::remove_file(&*path)
            {
                tracing::warn!(error = %e, path = %path.display(), "spool 临时文件删除失败");
            }
            *path = PathBuf::new();
        }
    }
}

impl SpooledBody {
    pub fn len(&self) -> u64 {
        match self {
            SpooledBody::Memory(b) => b.len() as u64,
            SpooledBody::Disk { len, .. } => *len,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 内存模式的字节视图（整体判定用）。仅内存模式调用——磁盘模式的判定
    /// 走增量扫描结论（forward_once 的 disk_scan 字段）。
    pub fn memory_bytes(&self) -> &[u8] {
        match self {
            SpooledBody::Memory(b) => b,
            SpooledBody::Disk { .. } => &[],
        }
    }

    /// 摘出内存字节（Drop 语义下的安全消费：经 &mut swap，不走字段 move）。
    /// 磁盘模式返回空（调用方保证不会走到）。
    pub(super) fn take_memory(&mut self) -> Bytes {
        match self {
            SpooledBody::Memory(b) => std::mem::take(b),
            SpooledBody::Disk { .. } => Bytes::new(),
        }
    }

    /// 回放流：内存按 8 KiB 零拷贝切块；磁盘流式读 64 KiB 块，EOF/Drop 时
    /// 删除临时文件。读取失败产出 Err（本地磁盘故障，回放中断）。
    pub async fn into_stream(
        &mut self,
    ) -> futures_util::stream::BoxStream<'static, Result<Bytes, std::io::Error>> {
        match self {
            SpooledBody::Memory(b) => {
                const CHUNK_SIZE: usize = 8 * 1024;
                let b = std::mem::take(b);
                futures_util::stream::iter(
                    b.chunks(CHUNK_SIZE)
                        .map(|c| Ok(b.slice_ref(c)))
                        .collect::<Vec<_>>(),
                )
                .boxed()
            }
            SpooledBody::Disk { path, len } => {
                // mem::take 剥离 Drop 责任：文件由 DiskSpoolStream 负责
                //（EOF/Drop 删除），此处 SpooledBody 已成空壳
                let path = std::mem::take(path);
                let len = *len;
                match tokio::fs::File::open(&path).await {
                    Ok(file) => DiskSpoolStream {
                        inner: Some((file, len)),
                        path,
                    }
                    .boxed(),
                    Err(e) => {
                        // 文件丢失（外部清理等）：回放为空 body，日志留痕
                        tracing::warn!(error = %e, path = %path.display(), "spool 临时文件读取失败，回放为空");
                        futures_util::stream::empty().boxed()
                    }
                }
            }
        }
    }
}

/// 磁盘 spool 的回放流：从临时文件流式读出（64 KiB 块）；EOF、提前丢弃
/// （Drop）或读取错误时删除文件。Windows 语义要求先关句柄再删，故 finish
/// 先 take 掉 File。
pub(super) struct DiskSpoolStream {
    /// Some((file, 剩余字节))；None = 已结束
    pub(super) inner: Option<(tokio::fs::File, u64)>,
    pub(super) path: PathBuf,
}

impl DiskSpoolStream {
    pub(super) fn finish(&mut self) {
        self.inner = None; // 关句柄
        let _ = std::fs::remove_file(&self.path);
    }
}

impl Drop for DiskSpoolStream {
    fn drop(&mut self) {
        // EOF 后重复调用幂等（文件已删，remove 的 NotFound 忽略）
        self.finish();
    }
}

impl Stream for DiskSpoolStream {
    type Item = Result<Bytes, std::io::Error>;
    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        let Some((file, remaining)) = this.inner.as_mut() else {
            return Poll::Ready(None);
        };
        if *remaining == 0 {
            this.finish();
            return Poll::Ready(None);
        }
        let want = (*remaining).min(64 * 1024) as usize;
        let mut buf = vec![0u8; want];
        let mut rb = tokio::io::ReadBuf::new(&mut buf);
        match ready!(std::pin::Pin::new(file).poll_read(cx, &mut rb)) {
            Err(e) => {
                this.finish();
                Poll::Ready(Some(Err(e)))
            }
            Ok(()) => {
                let filled = rb.filled();
                if filled.is_empty() {
                    // 提前 EOF（文件比记录的 len 短）：按结束处理
                    this.finish();
                    return Poll::Ready(None);
                }
                *remaining -= filled.len() as u64;
                Poll::Ready(Some(Ok(Bytes::copy_from_slice(filled))))
            }
        }
    }
}

/// 同步溢写写入器（在阻塞线程里写解码结果）：先写内存，超 `RESIDENT_LIMIT` 且
/// 有 spool 目录时整体迁到磁盘临时文件——与响应 spool 同一阈值与命名；中途
/// 失败或被放弃时由 `SpoolPath` 删除半截文件。
pub(super) enum SpillWriter {
    Memory {
        buf: Vec<u8>,
        dir: Option<PathBuf>,
    },
    Disk {
        // 字段顺序即 drop 顺序：先关句柄，再由 SpoolPath 删文件
        file: std::fs::File,
        path: SpoolPath,
        len: u64,
    },
}

impl std::io::Write for SpillWriter {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Memory { buf, dir } => match dir {
                Some(dir) if buf.len() + data.len() > RESIDENT_LIMIT => {
                    let path = SpoolPath(dir.join(format!(
                        "spool-{}-{}.spooltmp",
                        std::process::id(),
                        unique_seq()
                    )));
                    let mut file = std::fs::File::create(&path.0)?;
                    file.write_all(buf)?;
                    file.write_all(data)?;
                    let len = (buf.len() + data.len()) as u64;
                    *self = Self::Disk { file, path, len };
                }
                _ => buf.extend_from_slice(data),
            },
            Self::Disk { file, len, .. } => {
                file.write_all(data)?;
                *len += data.len() as u64;
            }
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Memory { .. } => Ok(()),
            Self::Disk { file, .. } => file.flush(),
        }
    }
}

impl SpillWriter {
    pub(super) fn into_body(self) -> std::io::Result<SpooledBody> {
        use std::io::Write as _;
        Ok(match self {
            Self::Memory { buf, .. } => SpooledBody::Memory(Bytes::from(buf)),
            Self::Disk {
                mut file,
                path,
                len,
            } => {
                file.flush()?;
                drop(file);
                SpooledBody::Disk {
                    path: path.into_path(),
                    len,
                }
            }
        })
    }
}
