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
    reason: wire::Failure,
    /// 已经转换到当前协议的中断结尾。
    tail: Option<Bytes>,
}
impl End {
    /// 上游中断还没有客户端协议的收尾，由外层决定。
    fn upstream() -> Self {
        Self {
            reason: wire::Failure::new("upstream_body_interrupted".into()),
            tail: None,
        }
    }
    /// 转发期间的读空闲、断流等沿用尝试的失败码；程序不会把读超时误诊为传输中断。
    pub(super) fn attempt(f: AttemptFailure) -> Self {
        let mut reason = wire::Failure::new(f.code.into());
        reason.details = Some(f.details);
        Self { reason, tail: None }
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
/// 严格解码器的逐块写端；阻塞工作在线程池，通道满时只阻塞这一段解码。
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
/// 用流读端桥接既有的严格 gzip/br/zstd/deflate 解码器。等上游的 read 也能被下游断开唤醒。
fn decode(input: Flow, encoding: String, limit: usize) -> Flow {
    let (tx, rx) = mpsc::channel(1);
    let out = tx.clone();
    tokio::task::spawn_blocking(move || {
        let mut failure = None;
        let stream = futures_util::stream::unfold((input, tx), |(mut f, tx)| async move {
            let item = tokio::select! {biased; _=tx.closed()=>None,v=f.next()=>v};
            item.map(|v| (v, (f, tx)))
        })
        .map(|v| {
            v.map_err(|end| {
                failure = Some(end);
                std::io::Error::other("upstream body interrupted")
            })
        })
        .boxed();
        let reader = tokio_util::io::SyncIoBridge::new(tokio_util::io::StreamReader::new(stream));
        let mut output = DecodeOutput(out);
        if crate::decode::decode_strict(&encoding, reader, &mut output, limit as u64).is_err() {
            let _ = output
                .0
                .blocking_send(Err(failure.unwrap_or_else(End::upstream)));
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
                    tokio::select! {biased; result=&mut frame=>Some(result), _=tx.closed()=>{
                        // 先停读上游，再限时收回已发帧；不能让旧序号的回信冒充 abort 的回信。
                        drop(input.take());
                        if gone.has_changed().is_ok() || !matches!(tokio::time::timeout(CANCEL_GRACE, frame).await, Ok(Ok(_))) {return}
                        None
                    }}
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
                        if self.required {
                            drop(input);
                            let _ = tx
                                .send(Err(End {
                                    reason,
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
                reason: e.reason.clone(),
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
                            reason,
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
    mut status: StatusCode,
    mut headers: HeaderMap,
    mut content: Content,
) -> Result<Response, Terminal> {
    let (client, gone) = tokio::sync::watch::channel(());
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
            content = Content::Saved(b);
            continue;
        }
        if !p.entry.has_stage(Stage::Stream) {
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
        let open = wire::Stage::Stream(wire::StreamMessage::new(
            id.clone(),
            wire::StreamEvent::Open(wire::StreamOpen::new(ctx.context(name), meta.clone(), head)),
        ));
        let first = p.open_stream(open.clone(), false).await;
        let result = match first {
            Err(f) if !matches!(f, Fault::Open { .. }) => p.open_stream(open, true).await,
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
        if let Some(f) = reply.fail {
            layer.worker.finish();
            if let Some(replacement) = f.response {
                status = StatusCode::from_u16(replacement.status).expect("validated status");
                headers = prepared.unwrap_or_default();
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
        if let Some(code) = reply.response.and_then(|p| p.status) {
            status = StatusCode::from_u16(code).expect("validated status");
        }
        if let Some(h) = prepared {
            headers = h;
        }
        headers.remove(http::header::CONTENT_ENCODING);
        headers.remove(http::header::CONTENT_LENGTH);
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
        tokio::spawn(layer.pump(f, data, tx, gone.clone()));
        content = Content::Flow(receive(rx));
    }
    let f = content.flow().await;
    let rec = ctx.rec.clone();
    let out = futures_util::stream::unfold((client, f), move |mut output| {
        let rec = rec.clone();
        async move {
            let item = output.1.next().await?;
            let b = match item {
                Ok(b) => b,
                Err(end) => match end.tail {
                    Some(b) => b,
                    None => return Some((Err(std::io::Error::other(end.reason.code)), output)),
                },
            };
            rec.response_bytes(b.len() as u64);
            Some((Ok::<_, std::io::Error>(b), output))
        }
    });
    let mut response = Response::new(Body::from_stream(out));
    *response.status_mut() = status;
    *response.headers_mut() = headers;
    Ok(response)
}

/// 已提交的头没有编码字段；即使所有流层都跳过，压缩字节也不能当作明文写进保活流。
pub(super) fn committed_body(
    response: Response,
    limit: usize,
) -> BoxStream<'static, Result<Bytes, std::io::Error>> {
    let encoding = encoding_to_undo(response.headers());
    let input = response.into_body().into_data_stream();
    if let Some(encoding) = encoding {
        return decode(
            input.map(|r| r.map_err(|_| End::upstream())).boxed(),
            encoding,
            limit,
        )
        .map(|r| r.map_err(|_| std::io::Error::other("stream decoding failed")))
        .boxed();
    }
    input.map(|r| r.map_err(std::io::Error::other)).boxed()
}

#[cfg(test)]
mod tests {
    use super::*;
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
