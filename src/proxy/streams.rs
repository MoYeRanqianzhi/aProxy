//! 响应方向按配置逆序组合逐块会话与整包层。管道只预取一块；读者离开时释放上游，中断与正常 EOF 分别传向外层。
use super::*;
use crate::{
    config::file::Stage,
    transform::{self, Fault, StreamWorker},
};
use aproxy_envelope as wire;
use futures_util::stream::BoxStream;
use tokio::sync::mpsc;

/// 中断在层间不是正常 EOF：外层需要 abort 而不是 close，只有最后的客户端读者才拿收尾字节。
pub(super) struct End {
    /// 传给外层 abort 的稳定原因，不把任意管道文本写进诊断。
    reason: Box<wire::Failure>,
    /// 已经转换到当前协议的中断结尾。
    tail: Option<Bytes>,
}
impl End {
    /// 上游中断还没有客户端协议的收尾，由外层决定。
    fn upstream() -> Self {
        Self {
            reason: Box::new(wire::Failure::new("upstream_body_interrupted".into())),
            tail: None,
        }
    }
    /// 转发期间的读空闲、断流等沿用尝试的失败码；程序不会把读超时误诊为传输中断。
    pub(super) fn attempt(f: AttemptFailure) -> Self {
        let mut reason = wire::Failure::new(f.code.into());
        reason.details = Some(f.details);
        Self {
            reason: Box::new(reason),
            tail: None,
        }
    }
}
/// 每项是一块完整的转换输出或一个中断；容量受 HTTP 与子进程两侧的背压限制。
type Flow = BoxStream<'static, Result<Bytes, End>>;
/// 同一条链内部保留 abort 原因；进入 HTTP 出口时才展开末层收尾。
pub(super) enum Content {
    /// 尚不需要按块回放的体。
    Saved(SpooledBody),
    /// 不等 EOF 即交付的体。
    Flow(Flow),
}
impl Content {
    /// 从内存或磁盘边回放边读；取消读者也会收走文件。
    async fn flow(self) -> Flow {
        match self {
            Self::Flow(f) => f,
            Self::Saved(mut b) => b
                .into_stream()
                .await
                .map(|r| r.map_err(|_| End::upstream()))
                .boxed(),
        }
    }
    /// 只有声明 response 的层才需要读到尾；沿用响应上限和磁盘溢写。
    async fn gather(self, state: &AppState) -> Result<(SpooledBody, Option<End>), Terminal> {
        let mut f = match self {
            Self::Saved(b) => return Ok((b, None)),
            Self::Flow(f) => f,
        };
        let limit = spool_limit_bytes(&state.config);
        let mut buf = SpoolBuffer::Memory(Vec::new());
        let mut interrupted = None;
        while let Some(item) = f.next().await {
            let bytes = match item {
                Ok(b) => b,
                Err(end) => {
                    interrupted = Some(end);
                    break;
                }
            };
            if buf.len().saturating_add(bytes.len() as u64) > limit as u64 {
                return Err(Terminal::response_too_large(limit));
            }
            spool_chunk(&mut buf, &bytes, state.spool_dir.as_deref()).await;
            if matches!(buf, SpoolBuffer::Poisoned) {
                return Err(Terminal::local_storage_failed(
                    StorageOp::WriteResponse,
                    "could not write response spool",
                ));
            }
        }
        buf.finish(StreamErrorScanner::new())
            .await
            .map(|(b, _)| (b, interrupted))
            .map_err(|e| Terminal::local_storage_failed(StorageOp::WriteResponse, &e))
    }
}
/// 继续处理当前块，只把末尾确实可能成为有效 UTF-8 的一至三个字节留给下一块；其余二进制走 data_b64。
fn take_fragment(bytes: Bytes, carry: &mut Vec<u8>) -> Bytes {
    let bytes = if carry.is_empty() {
        bytes
    } else {
        carry.extend_from_slice(&bytes);
        Bytes::from(std::mem::take(carry))
    };
    for n in (1..=3.min(bytes.len())).rev() {
        let start = bytes.len() - n;
        if std::str::from_utf8(&bytes[start..])
            .is_err_and(|e| e.valid_up_to() == 0 && e.error_len().is_none())
        {
            carry.extend_from_slice(&bytes[start..]);
            return bytes.slice(..start);
        }
    }
    bytes
}
/// 用一条有界通道连接每一对层。下游不再 poll 时，生产者也不能继续消耗上游。
fn receive<T: Send + 'static>(rx: mpsc::Receiver<T>) -> BoxStream<'static, T> {
    futures_util::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|v| (v, rx)) })
        .boxed()
}
/// 严格解码器的逐块写端；通道满时只阻塞这条流的专属解码线程。
struct DecodeOutput(mpsc::Sender<Result<Bytes, End>>);
impl std::io::Write for DecodeOutput {
    /// 复制解码器稍后要复用的缓冲区；客户端走了就立刻停止解码。
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0
            .blocking_send(Ok(Bytes::copy_from_slice(b)))
            .map_err(std::io::Error::other)?;
        Ok(b.len())
    }
    /// 每一块已经交给通道，无须额外刷新。
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
/// 解码器与结尾校验都会读到 EOF。Read 允许重复读取 EOF，底层 unfold 却不能再次 poll。
fn decoder_input(input: Flow, out: mpsc::Sender<Result<Bytes, End>>) -> Flow {
    futures_util::stream::unfold((input, out), |(mut f, tx)| async move {
        let item = tokio::select! {biased; _=tx.closed()=>None,v=f.next()=>v};
        item.map(|v| (v, (f, tx)))
    })
    .fuse()
    .boxed()
}
/// 用流读端桥接既有的严格 gzip/br/zstd/deflate 解码器。等上游的 read 也能被下游断开唤醒。
fn decode(input: Flow, encoding: String, limit: usize) -> Flow {
    let (tx, rx) = mpsc::channel(1);
    // 这些同步 codec 可能等待一条长连接数小时，不能占用 tokio::fs 和短计算共用的线程。
    // 进程内至多 64 条解码线程，其余在异步信号量排队，不读数据，也能被客户端取消。
    static SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(64);
    tokio::spawn(async move {
        let slot = tokio::select! {biased; _=tx.closed()=>return, p=SLOTS.acquire()=>p.expect("static semaphore")};
        let handle = tokio::runtime::Handle::current();
        let out = tx.clone();
        if std::thread::Builder::new()
            .name("format-stream-decoder".into())
            .spawn(move || {
                // SyncIoBridge 只在 read 期间进入运行时。流在回压写失败后才析构时，
                // 最后一张记账票仍要在同一运行时调度异步完成通知。
                let _runtime = handle.enter();
                let _slot = slot;
                let mut failure = None;
                let mut output = DecodeOutput(out.clone());
                let result = {
                    let stream = decoder_input(input, out)
                        .map(|v| {
                            v.map_err(|end| {
                                failure = Some(end);
                                std::io::Error::other("upstream body interrupted")
                            })
                        })
                        .boxed();
                    let mut reader = tokio_util::io::SyncIoBridge::new_with_handle(
                        tokio_util::io::StreamReader::new(stream),
                        handle.clone(),
                    );
                    // 编码的结束标记不是 HTTP EOF。读到真正的消息结尾才发 close，保留延迟到来的运输层中断。
                    crate::decode::decode_strict(&encoding, &mut reader, &mut output, limit as u64)
                        .is_ok()
                        && std::io::copy(&mut reader, &mut std::io::sink()).is_ok()
                };
                if !result {
                    let _ = output
                        .0
                        .blocking_send(Err(failure.unwrap_or_else(End::upstream)));
                }
            })
            .is_err()
        {
            let _ = tx.send(Err(End::upstream())).await;
        }
    });
    receive(rx)
}
/// 某个程序在这一条流上的状态、编号与工作进程；不同请求不能共用这份状态。
struct Layer {
    /// 已完成 open 的专属进程。
    worker: StreamWorker,
    /// 整条请求共享的状态表和历史记录。
    ctx: ExchangeCtx,
    /// 配置名而非进程自报的名称。
    name: String,
    /// 报告用配置序号区分打码后重名的条目。
    index: usize,
    /// 四种事件使用同一个 <request_id>/<attempt>。
    id: String,
    /// 层中途损坏时是否必须截断输入。
    required: bool,
    /// open 预先约定的客户端协议结尾。
    tail: Bytes,
}
impl Layer {
    /// 应用状态与报告，即使这帧没有产出任何字节；报告沿用整条请求的一份记录。
    fn absorb(&self, r: &mut wire::Reply) {
        self.ctx.absorb(&self.name, r.state.take());
        if let wire::ReportField::Present(report) = std::mem::take(&mut r.report) {
            self.ctx
                .rec
                .format_report((self.index, &self.name), "stream", None, report);
        }
    }
    /// 有效的 fail 是程序拒绝，不是协议或进程故障；外层据此区分两种中断。
    async fn send(&mut self, event: wire::StreamEvent) -> Result<Bytes, wire::Failure> {
        let (mut r, _) = self
            .worker
            .event(wire::Stage::Stream(wire::StreamMessage::new(
                self.id.clone(),
                event,
            )))
            .await
            .map_err(|f| {
                let mut reason = wire::Failure::new("format_error".into());
                reason.details = Some(serde_json::Map::from_iter([(
                    "kind".into(),
                    f.kind().into(),
                )]));
                reason
            })?;
        self.absorb(&mut r);
        if r.fail.is_some() {
            return Err(wire::Failure::new("format_fail".into()));
        }
        Ok(Bytes::copy_from_slice(
            r.data.as_bytes().unwrap_or_default(),
        ))
    }
    /// 客户端已走，不再应用回信或报告；完整收回这一帧才能复用进程，卡住的进程随会话丢弃。
    async fn abort_client(mut self) {
        let event = wire::StreamEvent::Abort {
            reason: wire::Failure::new("client_gone".into()),
        };
        let frame = wire::Stage::Stream(wire::StreamMessage::new(self.id.clone(), event));
        if matches!(
            tokio::time::timeout(CANCEL_GRACE, self.worker.event(frame)).await,
            Ok(Ok(_))
        ) {
            self.worker.finish();
        }
    }
    /// 逐帧往返。外层丢弃本层的接收端时立即释放上游；只有真正的客户端取消才要发 client_gone。
    async fn pump(
        mut self,
        input: Flow,
        initial: Bytes,
        tx: mpsc::Sender<Result<Bytes, End>>,
        gone: tokio::sync::watch::Receiver<()>,
    ) {
        let mut input = Some(input);
        let mut carry = Vec::new();
        if !initial.is_empty() && tx.send(Ok(initial)).await.is_err() {
            drop(input);
            if gone.has_changed().is_err() {
                self.abort_client().await;
            }
            return;
        }
        let end = loop {
            let item = tokio::select! {biased;
                _=tx.closed()=>{drop(input); if gone.has_changed().is_err() { self.abort_client().await; } return},
                item=input.as_mut().unwrap().next()=>item
            };
            let eof = item.is_none();
            let bytes = match item {
                Some(Ok(b)) => take_fragment(b, &mut carry),
                Some(Err(end)) => break Some(end),
                None => Bytes::from(std::mem::take(&mut carry)),
            };
            if !bytes.is_empty() {
                let event = wire::StreamEvent::Data(wire::Fragment::from_bytes(bytes.to_vec()));
                let reply = {
                    let mut frame = std::pin::pin!(self.send(event));
                    tokio::select! {biased; _=tx.closed()=>{
                        // 先停读上游，再限时收回原序号的帧。显式 fail 同样是完整回信，管道没有失步。
                        drop(input.take());
                        if gone.has_changed().is_ok() { return }
                        match tokio::time::timeout(CANCEL_GRACE, frame).await {
                            Ok(Ok(_)) => None,
                            Ok(Err(reason)) if reason.code == "format_fail" => None,
                            _ => return,
                        }
                    }, result=&mut frame=>Some(result)}
                };
                match reply {
                    None => {
                        self.abort_client().await;
                        return;
                    }
                    Some(Ok(b)) if !b.is_empty() => {
                        if tx.send(Ok(b)).await.is_err() {
                            drop(input);
                            if gone.has_changed().is_err() {
                                self.abort_client().await;
                            }
                            return;
                        }
                    }
                    Some(Ok(_)) => (),
                    Some(Err(reason)) => {
                        if tx.is_closed() && gone.has_changed().is_err() {
                            drop(input);
                            if reason.code == "format_fail" {
                                self.abort_client().await;
                            }
                            return;
                        }
                        if self.required {
                            drop(input);
                            let _ = tx
                                .send(Err(End {
                                    reason: Box::new(reason),
                                    tail: Some(self.tail),
                                }))
                                .await;
                            return;
                        }
                        drop(self);
                        if tx.send(Ok(bytes)).await.is_err() {
                            return;
                        }
                        if !carry.is_empty() && tx.send(Ok(Bytes::from(carry))).await.is_err() {
                            return;
                        }
                        let mut f = input.unwrap();
                        loop {
                            tokio::select! {biased; _=tx.closed()=>return,item=f.next()=>match item {Some(v)=>if tx.send(v).await.is_err(){return},None=>return}}
                        }
                    }
                }
            }
            if eof {
                break None;
            }
        };
        drop(input);
        let event = match &end {
            Some(e) => wire::StreamEvent::Abort {
                reason: e.reason.as_ref().clone(),
            },
            None => wire::StreamEvent::Close,
        };
        let result = {
            let mut frame = std::pin::pin!(self.send(event));
            tokio::select! {biased; r=&mut frame=>r, _=tx.closed()=>match tokio::time::timeout(CANCEL_GRACE,frame).await {Ok(r)=>r,Err(_)=>return}}
        };
        match result {
            Ok(b) => {
                self.worker.finish();
                if let Some(mut end) = end {
                    end.tail = Some(b);
                    let _ = tx.send(Err(end)).await;
                } else if !b.is_empty() {
                    let _ = tx.send(Ok(b)).await;
                }
            }
            Err(reason) => {
                if self.required {
                    let _ = tx
                        .send(Err(End {
                            reason: Box::new(reason),
                            tail: Some(self.tail),
                        }))
                        .await;
                } else if let Some(end) = end {
                    let _ = tx.send(Err(end)).await;
                }
            }
        }
    }
}
/// 客户端已经离开时只限时收尾，不让配置为无限等待的进程脱离请求长期占资源。
const CANCEL_GRACE: Duration = Duration::from_secs(2);
/// 在每个层界处理当前表示的编码。只在真正遇到整体层时攒齐；流式层的元数据在 open 之后即可提交。
#[allow(clippy::too_many_arguments)]
pub(super) async fn chain(
    state: &AppState,
    ctx: &ExchangeCtx,
    meta: &wire::RequestMeta,
    attempt: u32,
    accounting: &std::sync::Weak<UpstreamJoin>,
    committed: Option<Arc<std::sync::atomic::AtomicBool>>,
    mut status: StatusCode,
    mut headers: HeaderMap,
    mut content: Content,
) -> Result<Output, Terminal> {
    let (client, gone) = tokio::sync::watch::channel(());
    let mut converted_stream = false;
    for (index, p) in state.formats.iter().enumerate().rev() {
        if p.entry.has_stage(Stage::Response) && p.available_for_message() {
            let (body, interrupted) = content.gather(state).await?;
            if let Some(end) = interrupted {
                // 整包层不处理半截响应；已收前缀和真实中断继续向外，逐块层仍须发 abort。
                let prefix = Content::Saved(body).flow().await;
                content = Content::Flow(
                    prefix
                        .chain(futures_util::stream::once(async move { Err(end) }))
                        .boxed(),
                );
                continue;
            }
            let (code, h, b) =
                transform_response_layer(state, ctx, meta, index, status, headers, body).await;
            status = code;
            headers = h;
            converted_stream = false;
            content = Content::Saved(b);
            continue;
        }
        if !p.entry.has_stage(Stage::Stream)
            || !delivery::delivers_body(ctx.client_head || meta.method == "HEAD", status)
        {
            continue;
        }
        let encoding = request_encoding(&headers, true)
            .map(|v| v.to_str().unwrap_or("unsupported").to_string());
        if encoding.as_ref().is_some_and(|e| {
            !super::headers::format_encoding_depth_allowed(e)
                || e.split(',').any(|s| {
                    !matches!(
                        s.trim().to_ascii_lowercase().as_str(),
                        "" | "identity" | "br" | "gzip" | "x-gzip" | "deflate" | "zstd"
                    )
                })
        }) {
            continue;
        }
        let name = &p.entry.name;
        let id = format!("{}/{attempt}", ctx.request_id);
        let mut head = wire::Response::new(status.as_u16());
        head.headers = transform::headers_to_btreemap(&headers);
        head.headers.remove("content-encoding");
        let mut start = wire::StreamOpen::new(ctx.context(name), meta.clone(), head);
        start.client = Some(ctx.wire_client());
        let open = wire::Stage::Stream(wire::StreamMessage::new(
            id.clone(),
            wire::StreamEvent::Open(start),
        ));
        let first = p
            .open_stream(open.clone(), false, ctx.live_stream.clone())
            .await;
        let result = match first {
            Err(f) if !matches!(f, Fault::Open { .. }) => {
                p.open_stream(open, true, ctx.live_stream.clone()).await
            }
            other => other,
        };
        let (worker, (mut reply, prepared)) = match result {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(format=%state.history.secrets().replace(name),error=%e,"stream format layer skipped");
                continue;
            }
        };
        let layer = Layer {
            worker,
            ctx: ctx.clone(),
            name: name.clone(),
            index,
            id,
            required: p.entry.required,
            tail: reply.abort_tail.take().unwrap_or_default().into(),
        };
        layer.absorb(&mut reply);
        // 正常 open 补丁与拒绝替换共享同一提交点；等工作进程回信的时间也算在内。
        let fixed = committed
            .as_ref()
            .is_some_and(|sent| sent.load(std::sync::atomic::Ordering::Acquire));
        if let Some(f) = reply.fail {
            layer.worker.finish();
            if let Some(replacement) = f.response {
                let replacement_headers = prepared.unwrap_or_default();
                if fixed {
                    // 交付的头不可改，替换实体却可能自带编码；只传递解码所需的表示信息。
                    headers.remove(http::header::CONTENT_ENCODING);
                    headers.remove(http::header::CONTENT_LENGTH);
                    if let Some(enc) = request_encoding(&replacement_headers, true) {
                        headers.insert(http::header::CONTENT_ENCODING, enc.into_owned());
                    }
                } else {
                    status = StatusCode::from_u16(replacement.status).expect("validated status");
                    headers = replacement_headers;
                }
                converted_stream = false;
                content = Content::Saved(
                    transform::spool_response_body(
                        replacement.body.as_bytes().unwrap_or_default(),
                        state.spool_dir.as_deref(),
                    )
                    .await,
                );
            }
            continue;
        }
        if !fixed {
            if let Some(code) = reply.response.and_then(|p| p.status) {
                status = StatusCode::from_u16(code).expect("validated status");
            }
            if let Some(h) = prepared {
                headers = h;
            }
        }
        headers.remove(http::header::CONTENT_ENCODING);
        headers.remove(http::header::CONTENT_LENGTH);
        if !delivery::delivers_body(ctx.client_head, status) {
            // HTTP 不会轮询 204/304 的响应体；显式收尾，不经依赖消费正文的泵。
            drop(content);
            content = Content::Saved(SpooledBody::Memory(Bytes::new()));
            let mut layer = layer;
            // HTTP 没有正文可等，立即交付头。只把历史和 notify 的结算留到 close 处理完毕。
            let finalization = accounting.upgrade();
            tokio::spawn(async move {
                if layer.send(wire::StreamEvent::Close).await.is_ok() {
                    layer.worker.finish();
                }
                drop(finalization);
            });
            break;
        }
        let mut f = content.flow().await;
        if let Some(encoding) = encoding
            && encoding
                .split(',')
                .any(|s| !s.trim().is_empty() && !s.trim().eq_ignore_ascii_case("identity"))
        {
            f = decode(f, encoding, spool_limit_bytes(&state.config));
        }
        let (tx, rx) = mpsc::channel(1);
        let data = Bytes::copy_from_slice(reply.data.as_bytes().unwrap_or_default());
        converted_stream = true;
        tokio::spawn(layer.pump(f, data, tx, gone.clone()));
        content = Content::Flow(receive(rx));
    }
    Ok(Output {
        converted_stream,
        status,
        headers,
        content,
        client,
        rec: ctx.rec.clone(),
    })
}

/// 先保留可重放表示，等响应头归属确定再选保真直回或已提交的明文回放。
pub(super) struct Output {
    /// 最外层实际以流交付，即使程序选择的媒体类型不是 SSE。
    converted_stream: bool,
    /// 头尚未提交时放行末层的状态。
    status: StatusCode,
    /// 当前正文的表示头，决定是否还需要解码。
    headers: HeaderMap,
    /// 没有流层接手时保留已有的 spool，不重复缓冲。
    content: Content,
    /// 客户端丢弃响应后，唤醒这条链仍在等待的层。
    client: tokio::sync::watch::Sender<()>,
    /// 只数真正交付的字节，不把编码输入也算一遍。
    rec: crate::history::Recorder,
}
impl Output {
    /// 已经提交的 SSE 无法补发编码头。先完整校验压缩成员，校验失败不向客户端泄出半截明文。
    pub(super) async fn response(
        self,
        state: &AppState,
        committed: bool,
    ) -> Result<Response, Terminal> {
        let Self {
            converted_stream,
            status,
            headers,
            mut content,
            client,
            rec,
        } = self;
        if committed && let Some(encoding) = encoding_to_undo(&headers) {
            let decode_failure =
                || Terminal::decode_failed(&state.history.secrets().replace(&encoding));
            if !format_encoding_depth_allowed(&encoding) {
                return Err(decode_failure());
            }
            let (raw, end) = content.gather(state).await?;
            if end.is_some() {
                return Err(decode_failure());
            }
            content = Content::Saved(
                decode_for_committed_replay(
                    raw,
                    encoding.clone(),
                    state.spool_dir.clone(),
                    spool_limit_bytes(&state.config),
                )
                .await
                .map_err(|_| decode_failure())?,
            );
        }
        let f = content.flow().await;
        let out = futures_util::stream::unfold((client, f), move |mut output| {
            let rec = rec.clone();
            async move {
                let item = output.1.next().await?;
                let b = match item {
                    Ok(b) => b,
                    Err(end) => {
                        rec.stream_aborted();
                        match end.tail {
                            Some(b) => b,
                            None => {
                                return Some((Err(std::io::Error::other(end.reason.code)), output));
                            }
                        }
                    }
                };
                rec.response_bytes(b.len() as u64);
                Some((Ok::<_, std::io::Error>(b), output))
            }
        });
        let mut response = Response::new(Body::from_stream(out));
        if converted_stream {
            response.extensions_mut().insert(ConvertedStream);
        }
        *response.status_mut() = status;
        *response.headers_mut() = headers;
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// 在 data 已写出后让消费端离开；成功和显式拒绝都是可收回的完整帧，应在原会话上 abort。
    #[tokio::test]
    async fn abandoned_data_drains_success_and_valid_refusal_before_abort() {
        for mode in ["stream-gate", "gate-data-fail"] {
            let dir = tempfile::tempdir().unwrap();
            let frames = dir.path().join("frames");
            let gate = dir.path().join("go");
            let (cfg, diagnostics) = crate::config::file::parse(
                "[[format]]\ncommand='fixture'\nstages=['stream']\nprocess='persistent'",
                Path::new("unit.toml"),
            );
            assert!(diagnostics.is_empty());
            let mut entry = cfg.format.unwrap().pop().unwrap();
            entry.command = crate::transform::format_echo_path().display().to_string();
            entry.args = vec![mode.into(), gate.display().to_string()];
            entry
                .env
                .insert("ECHO_RECORD".into(), frames.display().to_string());
            let pool = crate::transform::Pool::new(
                entry,
                dir.path().into(),
                Default::default(),
                None,
                [4096; 2],
            );
            let ctx = ExchangeCtx::new(
                "r".into(),
                "/",
                false,
                false,
                crate::history::Recorder::none(),
            );
            let open = wire::StreamOpen::new(
                ctx.context("fixture"),
                wire::RequestMeta::new("GET".into(), "http://localhost/".into()),
                wire::Response::new(200),
            );
            let (worker, _) = pool
                .open_stream(
                    wire::Stage::Stream(wire::StreamMessage::new(
                        "r/1".into(),
                        wire::StreamEvent::Open(open),
                    )),
                    false,
                    None,
                )
                .await
                .unwrap();
            let layer = Layer {
                worker,
                ctx,
                name: "fixture".into(),
                index: 0,
                id: "r/1".into(),
                required: true,
                tail: Bytes::new(),
            };
            let (tx, rx) = mpsc::channel(1);
            let (client, gone) = tokio::sync::watch::channel(());
            let input = futures_util::stream::iter([Ok(Bytes::from_static(b"upstream"))]).boxed();
            let mut pump = std::pin::pin!(layer.pump(input, Bytes::new(), tx, gone));
            let seen = |event: &str| {
                std::fs::read_to_string(&frames)
                    .unwrap_or_default()
                    .lines()
                    .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
                    .find(|v| v["event"] == event)
            };
            tokio::select! { _=&mut pump=>panic!("ended before release"), _=tokio::time::timeout(Duration::from_secs(10), async {while seen("data").is_none(){tokio::time::sleep(Duration::from_millis(5)).await;}})=>assert!(seen("data").is_some()) }
            drop(client);
            drop(rx);
            tokio::time::pause();
            assert!(futures_util::poll!(&mut pump).is_pending());
            std::fs::write(gate, b"reply").unwrap();
            // 真实管道仍在工作；同时运行的阻塞任务抑制虚拟时间自动跳到截止点。
            let (done, wait) = std::sync::mpsc::channel();
            let hold =
                tokio::task::spawn_blocking(move || wait.recv_timeout(Duration::from_secs(10)));
            pump.await;
            done.send(()).unwrap();
            hold.await.unwrap().unwrap();
            tokio::time::resume();
            let abort = seen("abort").unwrap_or_else(|| panic!("no abort for {mode}"));
            assert_eq!(abort["seq"], 3);
            assert_eq!(abort["stream_id"], "r/1");
            assert_eq!(abort["reason"]["code"], "client_gone");
            pool.shutdown().await;
        }
    }
    /// 回压中取消解码时，持有 HTTP 流的最后一张票会在专属线程析构；完成回调仍须能派发通知。
    #[tokio::test]
    async fn decoder_drops_backpressured_input_with_a_runtime_context() {
        use std::io::Write as _;
        /// 代替真实响应记账票，在原始输入被释放而不是读到 EOF 时观测运行时。
        struct Ticket(Option<tokio::sync::oneshot::Sender<bool>>);
        impl Drop for Ticket {
            fn drop(&mut self) {
                let _ = self
                    .0
                    .take()
                    .unwrap()
                    .send(tokio::runtime::Handle::try_current().is_ok());
            }
        }
        let (tx, rx) = tokio::sync::oneshot::channel();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gz.write_all(&vec![b'x'; 512 * 1024]).unwrap();
        let first = Bytes::from(gz.finish().unwrap());
        let input =
            futures_util::stream::unfold((Some(first), Ticket(Some(tx))), |(b, t)| async move {
                match b {
                    Some(bytes) => Some((Ok(bytes), (None, t))),
                    None => std::future::pending().await,
                }
            });
        let mut output = decode(input.boxed(), "gzip".into(), 1024 * 1024);
        assert!(matches!(output.next().await, Some(Ok(_))));
        drop(output);
        assert!(
            tokio::time::timeout(Duration::from_secs(8), rx)
                .await
                .unwrap()
                .unwrap(),
            "input destructor lost its runtime"
        );
    }
    /// 内层 zlib 已产出全部明文也不能让损坏的外层 gzip 校验尾变成正常结束。
    #[tokio::test]
    async fn inner_deflate_eof_does_not_hide_a_damaged_outer_gzip() {
        use std::io::Write as _;
        let mut inner = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        inner.write_all(b"complete inner member").unwrap();
        let mut outer = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        outer.write_all(&inner.finish().unwrap()).unwrap();
        let mut bytes = outer.finish().unwrap();
        let crc = bytes.len() - 8;
        bytes[crc] ^= 1;
        let input = futures_util::stream::iter([Ok(Bytes::from(bytes))]).boxed();
        let mut flow = decode(input, "deflate, gzip".into(), 4096);
        while let Some(item) = flow.next().await {
            if let Err(end) = item {
                assert_eq!(end.reason.code, "upstream_body_interrupted");
                return;
            }
        }
        panic!("corrupt outer coding produced clean EOF");
    }
    /// 编码成员收完后 HTTP 仍可能断流；只有运输层的 EOF 才能让外层发送 close。
    #[tokio::test]
    async fn valid_deflate_member_does_not_hide_a_later_transport_error() {
        use std::io::Write as _;
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        z.write_all(b"decoded before HTTP EOF").unwrap();
        let (tx, rx) = mpsc::channel(1);
        tx.send(Ok(Bytes::from(z.finish().unwrap())))
            .await
            .ok()
            .unwrap();
        let mut decoded = decode(receive(rx), "deflate".into(), 4096);
        assert_eq!(
            decoded.next().await.unwrap().ok().unwrap(),
            "decoded before HTTP EOF"
        );
        let _ = tx.send(Err(End::upstream())).await;
        drop(tx);
        assert!(
            matches!(decoded.next().await,Some(Err(e)) if e.reason.code=="upstream_body_interrupted")
        );
    }

    /// 在确实等待读取压缩字节的时刻，唯一一个 Tokio 阻塞名额仍可服务无关的文件操作。
    #[test]
    fn idle_compressed_stream_leaves_the_shared_blocking_pool_free() {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .max_blocking_threads(1)
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let (seen, active) = tokio::sync::oneshot::channel();
            let feed = futures_util::stream::once(async move {
                let _ = seen.send(());
                std::future::pending::<Result<Bytes, End>>().await
            })
            .boxed();
            let live = decode(feed, "gzip".into(), 4096);
            active.await.unwrap();
            let result =
                tokio::time::timeout(Duration::from_secs(5), tokio::task::spawn_blocking(|| 17))
                    .await;
            drop(live);
            assert_eq!(
                result
                    .expect("idle decoder occupied the only shared blocking thread")
                    .unwrap(),
                17
            );
        });
    }
    /// 三个分块跨一个四字节字符时，文本片段保持 UTF-8；真正无效的字节不被吞掉。
    #[test]
    fn split_utf8_is_carried_to_the_next_chunk() {
        let mut carry = Vec::new();
        assert_eq!(
            take_fragment(Bytes::from_static(b"a\xf0\x9f"), &mut carry),
            "a"
        );
        assert!(take_fragment(Bytes::from_static(b"\x98"), &mut carry).is_empty());
        assert_eq!(
            take_fragment(Bytes::from_static(b"\x80z"), &mut carry),
            "😀z"
        );
        assert!(carry.is_empty());
        assert_eq!(
            &*take_fragment(Bytes::from_static(b"\xff\xe2"), &mut carry),
            b"\xff"
        );
        assert_eq!(carry, b"\xe2");
    }
    /// 严格解码器确认成员结束之后，排空检查再次读取同一个输入仍得到 EOF。
    #[tokio::test]
    async fn decode_input_eof_can_be_read_twice() {
        let (tx, _rx) = mpsc::channel(1);
        let mut input = decoder_input(futures_util::stream::empty().boxed(), tx);
        assert!(input.next().await.is_none());
        assert!(input.next().await.is_none());
    }
    /// zlib 的前两字节分属不同网络块仍应识别包装，而不是把第一个单字节当成裸 deflate。
    #[tokio::test]
    async fn split_zlib_header_is_decoded_before_stream_data() {
        use std::io::Write as _;
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(b"decoded").unwrap();
        let data = Bytes::from(z.finish().unwrap());
        let input = futures_util::stream::iter([Ok(data.slice(..1)), Ok(data.slice(1..))]).boxed();
        let mut f = decode(input, "deflate".into(), 1024);
        let mut out = Vec::new();
        while let Some(b) = f.next().await {
            out.extend_from_slice(&b.ok().expect("valid zlib"));
        }
        assert_eq!(out, b"decoded");
    }
}
