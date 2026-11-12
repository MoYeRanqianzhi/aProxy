//! 保活通道：提交点三态机、心跳、等待驱动（drive）与客户端断开哨兵。

use super::*;

/// 「只收到心跳的等待中断开」提示的下限：客户端在已提交、但还没收到任何上游
/// 真实字节的响应上等了至少这么久才断开，多半是它自己的流空闲超时到点——不少
/// agent 客户端按 SSE **事件**计时，而注释心跳在 SSE 解析层就被丢弃、重置不了它
/// （Claude Code 600s、Codex 300s、Qwen Code 240s，前两者黑盒实测）。下限只为
/// 滤掉开头几十秒内的主动取消；已知的事件级超时都远高于它，各客户端到点断开时
/// 都能提示到，而不像旧阈值（590s）那样只覆盖 Claude Code 一家。
pub(super) const HEARTBEAT_ONLY_HINT_MIN: Duration = Duration::from_secs(60);

/// 保活通道的响应出口——「提交点」状态机的两个状态：
/// 尚未向客户端写出任何字节（Pending）→ 响应头已发出（Committed）。
/// 提交只发生一次、不可撤回：之后 status 与响应头再也改不了，只能往 SSE 体里写。
pub(super) enum KeepaliveSink {
    Pending {
        /// handler 正 await 这个 oneshot 等 `Response`。Option 只为提交时能把
        /// Sender 取走（取走后立刻转为 Committed；send 失败 = 客户端已断开）
        resp_tx: Option<tokio::sync::oneshot::Sender<Response>>,
        /// 有个 tick 本该提交骨架、却因当次尝试收到「2xx 但非 SSE」的响应头而
        /// 暂停跳过了（见 `drive`）。暂停只限那一次尝试：它若需要重试，下一段
        /// 驱动一开始就补提交，绝不把「首字节 ≤ 一个保活间隔」再往后拖一整拍
        skeleton_due: bool,
        /// 心跳来源，提交时带进 Committed
        beat: Heartbeat,
    },
    Committed {
        tx: tokio::sync::mpsc::Sender<Result<Bytes, std::io::Error>>,
        /// 客户端断开信号：哨兵（`ClientGoneGuard`）随响应 Body 被 hyper drop 时置位
        gone_rx: tokio::sync::watch::Receiver<bool>,
        /// 提交时刻：断开提示的计时起点（客户端从这一刻起开始等事件）
        at: tokio::time::Instant,
        /// 至今只发过响应头与心跳、还没写出任何上游真实字节（回放 / 终态 error
        /// 事件都算真实字节）。只有这种等待中的断开才可能是客户端的流空闲超时
        /// 到点，断开提示据此判定
        heartbeats_only: bool,
        beat: Heartbeat,
        /// 「真实字节已开始写出」闸门：动态心跳在独立任务里写通道，必须与回放
        /// 互斥，否则心跳可能插进回放的事件中间。`send` 写第一块真实字节前在
        /// 锁内置位；心跳任务在锁内确认未置位才写（try_send 不阻塞，持锁极短）
        replay_gate: Arc<std::sync::Mutex<bool>>,
    },
}

/// 心跳的来源：config 的固定字节（keepalive_heartbeat 生效值），以及配置了
/// heartbeat_transform、且请求转换已完成时由心跳转换器逐拍生成的动态心跳。
#[derive(Clone)]
pub(super) struct Heartbeat {
    pub(super) fixed: Bytes,
    pub(super) dynamic: Option<Arc<DynamicHeartbeat>>,
}

/// 一个请求的动态心跳生成器（在各拍的独立任务间共享）。
pub(super) struct DynamicHeartbeat {
    pub(super) pool: Arc<crate::transform::TransformPool>,
    pub(super) ctx: ExchangeCtx,
    pub(super) source: HeartbeatSource,
    /// 已发起的心跳调用次数（信封 heartbeat.seq）
    pub(super) seq: std::sync::atomic::AtomicU64,
    /// 当前第几次上游尝试，重试循环每轮更新
    pub(super) attempt: std::sync::atomic::AtomicU32,
    /// 有一次调用还没回来：这一拍不再排队调用，直接发固定心跳
    pub(super) in_flight: std::sync::atomic::AtomicBool,
    /// 本请求已就心跳转换失败 warn 过（只 warn 一次，免得每拍刷屏）
    pub(super) warned: std::sync::atomic::AtomicBool,
}

/// 在独立任务里跑完一次心跳转换并写出结果。不在 drive 的 select 里直接
/// await：drive 一返回就会 drop 在途的转换，persistent worker 在往返中途被
/// drop 会被剔除（进程被杀），每拍都可能发生。
pub(super) fn spawn_dynamic_heartbeat(
    d: Arc<DynamicHeartbeat>,
    tx: tokio::sync::mpsc::Sender<Result<Bytes, std::io::Error>>,
    gate: Arc<std::sync::Mutex<bool>>,
    fixed: Bytes,
    elapsed: Duration,
) {
    tokio::spawn(async move {
        let info = aproxy_envelope::HeartbeatInfo {
            seq: d.seq.fetch_add(1, AtomicOrdering::Relaxed) + 1,
            elapsed_ms: elapsed.as_millis() as u64,
            attempt: d.attempt.load(AtomicOrdering::Relaxed),
        };
        let bytes =
            match crate::transform::transform_heartbeat(&d.pool, &d.ctx, &d.source, info).await {
                Ok(b) => b,
                Err(why) => {
                    if !d.warned.swap(true, AtomicOrdering::Relaxed) {
                        tracing::warn!(
                            request_id = %d.ctx.request_id,
                            error = %why,
                            "心跳转换失败，这一拍改发固定心跳（本请求之后的失败不再重复告警）"
                        );
                    }
                    Some(fixed)
                }
            };
        if let Some(bytes) = bytes {
            let replaying = gate
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !*replaying {
                let _ = tx.try_send(Ok(bytes));
            }
        }
        d.in_flight.store(false, AtomicOrdering::Release);
    });
}

impl KeepaliveSink {
    pub(super) fn is_committed(&self) -> bool {
        matches!(self, Self::Committed { .. })
    }

    /// 客户端断开时完成。未提交：hyper 因断开 drop 了 handler future，oneshot
    /// 接收端随之销毁；已提交：响应 Body 被 drop，哨兵置位（或发送端已随哨兵
    /// 销毁——`wait_for` 返回 Err，同样视为断开）。
    pub(super) async fn client_gone(&mut self) {
        match self {
            Self::Pending {
                resp_tx: Some(tx), ..
            } => tx.closed().await,
            // Sender 已被取走只发生在提交失败的瞬间，调用方随即返回，不会再等
            Self::Pending { resp_tx: None, .. } => std::future::pending().await,
            Self::Committed { gone_rx, .. } => {
                let _ = gone_rx.wait_for(|gone| *gone).await;
            }
        }
    }

    /// 提交响应头：把以 mpsc 接收端为体的流式响应交给 handler 返回给 hyper。
    /// 返回 false = 客户端已断开（handler 已被 drop，没人接收这个 Response）。
    /// 已提交时调用是空操作（返回 true）。
    pub(super) fn commit(&mut self, status: StatusCode, headers: HeaderMap) -> bool {
        let Self::Pending {
            resp_tx: slot,
            beat,
            ..
        } = self
        else {
            return true;
        };
        let beat = beat.clone();
        let Some(resp_tx) = slot.take() else {
            return false;
        };
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(32);
        let (gone_tx, gone_rx) = tokio::sync::watch::channel(false);
        let gone_guard = ClientGoneGuard { tx: gone_tx };
        // 哨兵 move 进 map 闭包：Body 被客户端断开而 drop 时闭包销毁 → 置位断开信号
        let stream = tokio_stream::wrappers::ReceiverStream::new(rx).map(move |chunk| {
            let _ = &gone_guard;
            chunk
        });
        let mut resp = Response::builder().status(status);
        for (name, value) in headers.iter() {
            resp = resp.header(name, value);
        }
        // header 逐条来自已校验的 HeaderMap，body() 的错误态不可能出现
        let resp = resp
            .body(Body::from_stream(stream))
            .unwrap()
            .into_response();
        if resp_tx.send(resp).is_err() {
            return false;
        }
        *self = Self::Committed {
            tx,
            gone_rx,
            at: tokio::time::Instant::now(),
            heartbeats_only: true,
            beat,
            replay_gate: Arc::default(),
        };
        true
    }

    /// 提交骨架头（200 + text/event-stream，不设 connection 头——hyper 按协议
    /// 自动管理）并立刻发首个心跳：客户端的空闲计时从收到首批字节起算。
    /// false = 客户端已断开。
    pub(super) fn commit_skeleton(&mut self) -> bool {
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::CONTENT_TYPE,
            HeaderValue::from_static("text/event-stream"),
        );
        headers.insert(
            http::header::CACHE_CONTROL,
            HeaderValue::from_static("no-cache"),
        );
        self.commit(StatusCode::OK, headers) && self.heartbeat()
    }

    /// 发一个心跳。用 try_send 而非 send：通道满说明客户端还有没读走的字节，
    /// 这一拍可以省；而在这里阻塞会连带停住对上游响应的读取。配置了心跳转换器
    /// 且上一次调用已回来时，这一拍交给它生成（独立任务，结果稍后写出）；否则
    /// 发固定心跳。false = 客户端已断开（接收端随响应 Body 销毁）。
    pub(super) fn heartbeat(&self) -> bool {
        match self {
            Self::Committed {
                tx,
                beat,
                at,
                replay_gate,
                ..
            } => {
                if tx.is_closed() {
                    return false;
                }
                if let Some(d) = &beat.dynamic
                    && !d.in_flight.swap(true, AtomicOrdering::AcqRel)
                {
                    spawn_dynamic_heartbeat(
                        d.clone(),
                        tx.clone(),
                        replay_gate.clone(),
                        beat.fixed.clone(),
                        at.elapsed(),
                    );
                    return true;
                }
                !matches!(
                    tx.try_send(Ok(beat.fixed.clone())),
                    Err(tokio::sync::mpsc::error::TrySendError::Closed(_))
                )
            }
            Self::Pending { .. } => true,
        }
    }

    /// 请求转换完成后启用动态心跳（之前各拍不知道最终请求，只发固定心跳）
    pub(super) fn arm_dynamic_heartbeat(&mut self, d: Arc<DynamicHeartbeat>) {
        match self {
            Self::Pending { beat, .. } | Self::Committed { beat, .. } => beat.dynamic = Some(d),
        }
    }

    /// 告诉动态心跳当前是第几次上游尝试
    pub(super) fn note_attempt(&self, attempt: u32) {
        let (Self::Pending { beat, .. } | Self::Committed { beat, .. }) = self;
        if let Some(d) = &beat.dynamic {
            d.attempt.store(attempt, AtomicOrdering::Relaxed);
        }
    }

    /// 往已提交的响应体里写一块（成功回放 / 终态 error 事件）。
    /// false = 客户端已断开（或尚未提交——调用方保证只在提交后调用）。
    pub(super) async fn send(&mut self, chunk: Result<Bytes, std::io::Error>) -> bool {
        match self {
            Self::Committed {
                tx,
                heartbeats_only,
                replay_gate,
                ..
            } => {
                if *heartbeats_only {
                    // 第一块真实字节：先关闸，之后到达的动态心跳一律丢弃
                    *replay_gate
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
                }
                *heartbeats_only = false;
                tx.send(chunk).await.is_ok()
            }
            Self::Pending { .. } => false,
        }
    }

    /// 记下「有个 tick 因暂停而跳过了骨架提交」（已提交时无意义，忽略）
    pub(super) fn mark_skeleton_due(&mut self) {
        if let Self::Pending { skeleton_due, .. } = self {
            *skeleton_due = true;
        }
    }

    /// 取走「待补提交骨架」标记
    pub(super) fn take_skeleton_due(&mut self) -> bool {
        match self {
            Self::Pending { skeleton_due, .. } => std::mem::take(skeleton_due),
            Self::Committed { .. } => false,
        }
    }

    /// 尚未提交时交出完整响应（首轮快速路径 / 502 终态）。客户端已断开时
    /// send 失败，响应随之丢弃。
    pub(super) fn respond(self, resp: Response) {
        if let Self::Pending {
            resp_tx: Some(tx), ..
        } = self
        {
            let _ = tx.send(resp);
        }
    }

    /// 客户端断开的日志（含流空闲超时提示，见 `log_client_gone_after`）
    pub(super) fn log_client_gone(&self, during: &str) {
        let heartbeat_only_wait = match self {
            Self::Committed {
                at,
                heartbeats_only: true,
                ..
            } => Some(at.elapsed()),
            _ => None,
        };
        log_client_gone_after(during, heartbeat_only_wait);
    }
}

/// 客户端断开的日志。`heartbeat_only_wait` = 客户端在「只收到心跳」的状态下
/// 等了多久（None = 尚未提交，或已经开始收到上游真实字节）；等了至少
/// `HEARTBEAT_ONLY_HINT_MIN` 时追加一条 warn，提示检查客户端的流空闲超时。
/// 只是日志文案：行为对任何客户端都一样（断开即中止上游），不点名、不按客户端
/// 特判——各客户端的具体写法放在用户文档里；等待秒数一并打出，便于用户对照
/// 自己客户端的超时取值。
pub(super) fn log_client_gone_after(during: &str, heartbeat_only_wait: Option<Duration>) {
    tracing::info!(during, "客户端已断开，中止上游请求（保活通道）");
    if let Some(waited) = heartbeat_only_wait
        && waited >= HEARTBEAT_ONLY_HINT_MIN
    {
        tracing::warn!(
            waited_secs = waited.as_secs(),
            "客户端在只收到心跳的等待中断开（已等约 {} 秒）。若不是主动取消，多半是客户端的流空闲超时到点：\
             不少客户端按 SSE 事件计时，aProxy 的注释心跳不算事件、重置不了它，超过该超时的重试期或长生成\
             都会被客户端断开重发。请调大或关闭客户端的流空闲超时，常见客户端的写法见 README「接入 agent 客户端」",
            waited.as_secs()
        );
    }
}

/// 响应头的 content-type 是否为 SSE（`text/event-stream`）——保活心跳（SSE 注释）
/// 只在 SSE 里合法，提交点的各条规则都以此为界
pub(super) fn is_event_stream(headers: &HeaderMap) -> bool {
    headers
        .get(http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.to_ascii_lowercase().contains("text/event-stream"))
}

/// 响应声明的、回放前必须解掉的编码；无该头或只有 identity 层 → None
pub(super) fn encoding_to_undo(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(http::header::CONTENT_ENCODING)?;
    // 非 ASCII 的头值原样交给严格解码器，由它按「不支持的编码」报错
    let value = raw.to_str().map_or_else(
        |_| String::from_utf8_lossy(raw.as_bytes()).to_string(),
        str::to_string,
    );
    value
        .split(',')
        .map(str::trim)
        .any(|layer| !layer.is_empty() && !layer.eq_ignore_ascii_case("identity"))
        .then_some(value)
}

/// 上游响应头能否原样提交给客户端（保活通道尚未提交时）：2xx、
/// `text/event-stream`、且未经压缩。未压缩是硬条件——提交后要往体里插
/// `: keepalive` 注释，压缩流里插入明文会让客户端解压失败。保活适用的请求已
/// 要求上游以 identity 回应，这里兜住无视该要求仍压缩的上游（不提交真实头，
/// 等间隔到点提交骨架；成功体回放前再完整解码，见 `decode_for_committed_replay`）。
pub(super) fn head_is_committable(status: StatusCode, headers: &HeaderMap) -> bool {
    status.is_success() && is_event_stream(headers) && encoding_to_undo(headers).is_none()
}

/// 在保活通道里驱动一个 future（一次上游尝试 / 一段退避等待）直到完成，
/// 与心跳节拍、客户端断开信号一起 select——这正是「心跳全程覆盖」的实现点：
/// 上游请求进行中（等响应头、缓冲上游流）与退避期间同样按间隔发心跳。
///
/// - 开始时：上一次尝试因暂停而跳过了骨架提交（`skeleton_due`）→ 立即补提交
/// - 每个 tick：已提交 → 发一个心跳；尚未提交 → 提交骨架头（一个间隔内上游没
///   给出可提交的结果）——除非本次尝试处于暂停中，此时只记下 `skeleton_due`
/// - `head_rx`（尚未提交时每次尝试都传入）收到上游响应头：
///   - 2xx 但不是 text/event-stream（stream:true 的 NDJSON、或上游无视 stream
///     回了普通 JSON）→ **本次尝试进行期间**暂停骨架提交：骨架是 SSE、心跳是
///     SSE 注释，提交了就把非 SSE 的流改坏（content-type 被改写、正文混入
///     `: keepalive` 行）。本次尝试成功 → 不提交，走保真快速路径原样回放；
///     需要重试（如 200 + error JSON）→ 暂停随本次尝试结束，下一段驱动开头
///     补提交骨架，之后照常心跳。取舍：上游先回 2xx 非 SSE 头、再迟迟不发体
///     时，客户端在本次尝试期间收不到任何字节，最长约 read_timeout_secs
///     （默认 300s，两次读到数据的间隔上限）——非 SSE 的流本就没有可用的
///     保活手段，这与不保活路径的等待特性一致
///   - 可直接提交（2xx + 未压缩 SSE，且 `real_head_allowed`：未配置
///     response_transform）→ 立即提交上游真实 status 与响应头
/// - 客户端断开或心跳发送失败 → 返回 None：`fut` 随本函数返回被 drop，在途的
///   reqwest 连接关闭，上游停止生成（计费保护）
pub(super) async fn drive<F: std::future::Future>(
    sink: &mut KeepaliveSink,
    ticker: &mut tokio::time::Interval,
    fut: F,
    mut head_rx: Option<tokio::sync::oneshot::Receiver<(StatusCode, HeaderMap)>>,
    real_head_allowed: bool,
    during: &str,
) -> Option<F::Output> {
    if sink.take_skeleton_due() {
        tracing::info!("上一次尝试暂停期间已到保活间隔，补提交骨架头（保活通道）");
        if !sink.commit_skeleton() {
            sink.log_client_gone(during);
            return None;
        }
    }
    let mut skeleton_paused = false;
    tokio::pin!(fut);
    loop {
        tokio::select! {
            // 结果优先：同一轮里 fut 已完成就不再因并发就绪的 tick/头部通知先行
            // 提交——未提交时尝试一完成即走保真快速路径
            biased;
            out = &mut fut => return Some(out),
            () = sink.client_gone() => {
                sink.log_client_gone(during);
                return None;
            }
            head = async { head_rx.as_mut().expect("前置条件保证 Some").await }, if head_rx.is_some() => {
                // 每次尝试只通知一次（Err = forward_once 没拿到响应头就结束了）
                head_rx = None;
                if let Ok((status, headers)) = head
                    && !sink.is_committed()
                {
                    if status.is_success() && !is_event_stream(&headers) {
                        skeleton_paused = true;
                        tracing::info!(status = %status, "上游回了 2xx 非 SSE 响应头：本次尝试期间暂停提交骨架头（保活通道）");
                    } else if real_head_allowed && head_is_committable(status, &headers) {
                        if !sink.commit(status, headers) {
                            sink.log_client_gone(during);
                            return None;
                        }
                        tracing::info!(status = %status, "上游 SSE 响应头已到，先行提交给客户端（保活通道）");
                    }
                }
            }
            _ = ticker.tick() => {
                let alive = if sink.is_committed() {
                    sink.heartbeat()
                } else if skeleton_paused {
                    sink.mark_skeleton_due();
                    true
                } else {
                    tracing::info!("一个保活间隔内上游未给出可提交的结果，先提交骨架头（保活通道）");
                    sink.commit_skeleton()
                };
                if !alive {
                    sink.log_client_gone(during);
                    return None;
                }
            }
        }
    }
}

/// 客户端断开哨兵：持有 watch 发送端，Drop 时置位断开信号。
/// 被 move 进 keepalive 响应 Body 的流闭包，Body 随客户端断开被 hyper drop 时触发。
pub(super) struct ClientGoneGuard {
    pub(super) tx: tokio::sync::watch::Sender<bool>,
}

impl Drop for ClientGoneGuard {
    fn drop(&mut self) {
        let _ = self.tx.send(true);
    }
}
