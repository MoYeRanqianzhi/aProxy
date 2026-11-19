//! decide 只对已经完成的尝试投票，不接管客户端连接，也不改写上游历史。
use super::*;
use crate::config::file::Stage;
use aproxy_envelope as wire;

/// 投票程序可以指定重试等待或客户端的终态补丁。按链序，决定与 reroute 各保留第一票。
pub(super) enum Verdict {
    /// 判定本次未成功，由普通尝试循环负责重放请求。
    Retry {
        /// 已脱敏的配置名，用于日志、记录和终态详情。
        name: String,
        /// 只有明确给了等待时间才覆盖当前退避。
        delay: Option<Duration>,
    },
    /// 停止咨询；空补丁与显式的空响应不能混为一谈。
    Stop {
        /// 真正投下停止票的层，而不是随后负责转发的实例。
        name: String,
        /// 原补丁以及只解析一次的可选替换头表。
        patch: Option<(wire::ResponsePatch, Option<HeaderMap>)>,
    },
}
/// 本次咨询的两项独立结果；不表态时沿用配置和重试段的默认行为。
#[derive(Default)]
pub(super) struct Votes {
    /// 没有投票时保持原来的分类和上限。
    pub(super) decision: Option<Verdict>,
    /// 与决定独立；失败停在链内时这一票不能令游标跳过失败层。
    pub(super) reroute: Option<bool>,
}

/// 先保留 64 KiB 头，再用环形队列保留其余预算所能容纳的尾部。即使来自压缩流，
/// 总长也按解码后的字节计；中间丢掉的部分不分配内存，断开时阻塞线程在下一块退出。
struct Preview {
    /// 前缀固定不动，保留序列的开头与协议标记。
    head: Vec<u8>,
    /// 滚动保存最近的字节，它与前缀合起来不超过 1 MiB。
    tail: std::collections::VecDeque<u8>,
    /// 解码后的总长，用来计算被跳过的字节，而非输入流长。
    count: u64,
    /// 请求被取消后不再扫描剩余输入。
    cancel: tokio::sync::oneshot::Sender<()>,
}
impl std::io::Write for Preview {
    /// 只追加仍在头尾预算内的部分，向解码器确认整块已消费。
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        const HEAD: usize = 64 * 1024;
        const TAIL: usize = 1024 * 1024 - HEAD;
        if self.cancel.is_closed() {
            return Err(std::io::ErrorKind::BrokenPipe.into());
        }
        let first = (HEAD - self.head.len()).min(buf.len());
        self.head.extend_from_slice(&buf[..first]);
        let rest = &buf[first..];
        let keep = rest.len().min(TAIL);
        self.tail
            .drain(..(self.tail.len() + keep).saturating_sub(TAIL));
        self.tail.extend(&rest[rest.len() - keep..]);
        self.count += buf.len() as u64;
        Ok(buf.len())
    }
    /// 预览位于内存中，无需将任何数据刷回上游 spool。
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// 读取上游副本而不是拿走随后要交付的 spool。原始体始终留给停止透传或正常交付。
async fn observed(upstream: &UpstreamResponse) -> Option<wire::ObservedResponse> {
    let mut r = wire::Response::new(upstream.status.as_u16());
    r.headers = crate::transform::headers_to_btreemap(&upstream.headers);
    let encoding = request_encoding(&upstream.headers, true);
    let encoding = encoding
        .as_deref()
        .map(|v| v.to_str())
        .transpose()
        .ok()?
        .unwrap_or("")
        .to_owned();
    if !format_encoding_depth_allowed(&encoding) {
        return None;
    }
    r.headers.remove("content-encoding");
    let mut reader: Box<dyn std::io::Read + Send> = match &upstream.body {
        SpooledBody::Memory(b) => Box::new(std::io::Cursor::new(b.clone())),
        SpooledBody::Disk { path, .. } => {
            Box::new(tokio::fs::File::open(path).await.ok()?.into_std().await)
        }
    };
    let (cancel, alive) = tokio::sync::oneshot::channel();
    let (body, skipped) = tokio::task::spawn_blocking(move || {
        let mut preview = Preview {
            head: Vec::new(),
            tail: Default::default(),
            count: 0,
            cancel,
        };
        crate::decode::decode_strict(&encoding, &mut reader, &mut preview, u64::MAX).ok()?;
        let mut bytes = preview.head;
        bytes.extend(preview.tail);
        let omitted = preview.count - bytes.len() as u64;
        Some((
            wire::Body::from_bytes(bytes),
            (omitted != 0).then_some(omitted),
        ))
    })
    .await
    .ok()??;
    drop(alive);
    r.body = body;
    let mut observed = wire::ObservedResponse::new(r);
    observed.truncated = skipped;
    Some(observed)
}

/// 保存本次输入的实际响应与错误；本地存储/大小的硬终态不是可由格式程序替代的上游尝试判定。
pub(super) async fn consult(
    state: &AppState,
    req: &OutboundRequest,
    outcome: &Outcome,
    attempt: u32,
    elapsed: Duration,
    limit: Option<u32>,
) -> Votes {
    use wire::Decision::{Retry, Stop};
    let mut votes = Votes::default();
    let (action, rule, failure, response) = match outcome {
        Outcome::Retry {
            failure,
            rule,
            response,
            ..
        } => (
            if limit.is_some_and(|n| n > 0 && attempt >= n) {
                Stop
            } else {
                Retry
            },
            *rule,
            Some(failure),
            response.as_ref(),
        ),
        Outcome::Exhausted {
            failure,
            rule,
            response,
        } => (Stop, *rule, Some(failure), Some(response)),
        Outcome::Deliver(response) => (Stop, None, None, Some(response)),
        Outcome::TooLarge | Outcome::SpoolFailed(..) => return votes,
    };
    let by = rule.map_or_else(
        || {
            if failure.is_some() && limit.is_some() {
                "override"
            } else {
                "default"
            }
            .to_owned()
        },
        |i| format!("rule:{i}"),
    );
    let mut fallback = wire::Fallback::new(action, by);
    if fallback.decision == Retry {
        fallback.delay_ms = Some(
            retry::delay_for_attempt(attempt, state.config.max_retry_backoff_secs)
                .as_millis()
                .min(u128::from(u64::MAX)) as u64,
        );
    }
    let ctx = req.ctx.as_ref().expect("format context");
    let mut preview = None;
    for (layer, pool) in state
        .formats
        .iter()
        .enumerate()
        .filter(|(_, p)| p.entry.has_stage(Stage::Decide))
    {
        let supports_success =
            |h: &wire::HelloReply| h.features.iter().any(|f| f == "decide.success");
        if failure.is_none()
            && !pool
                .negotiated
                .lock()
                .unwrap()
                .as_deref()
                .is_some_and(supports_success)
        {
            continue;
        }
        let name = &pool.entry.name;
        let safe = state.history.secrets().replace(name);
        let turn = match pool.turn(false).await {
            Ok(turn) => turn,
            Err(e) => {
                tracing::warn!(format=%safe,error=%e,"decide unavailable; no vote");
                continue;
            }
        };
        if failure.is_none() && !supports_success(turn.hello()) {
            turn.unused().await;
            continue;
        }
        if let Some(response) = response
            && preview.is_none()
        {
            preview = observed(response).await;
            if preview.is_none() {
                turn.unused().await;
                tracing::warn!(format=%safe,"could not prepare decide response; using fallback");
                break;
            }
        }
        let body = if turn.hello().request_body.get("decide") == Some(&wire::BodyNeed::Full) {
            let bytes = match req.sent_body() {
                RequestBody::Memory(b) => b.to_vec(),
                RequestBody::Disk { path, .. } => match tokio::fs::read(path).await {
                    Ok(b) => b,
                    Err(_) => {
                        turn.unused().await;
                        continue;
                    }
                },
            };
            wire::Body::from_bytes(bytes)
        } else {
            wire::Body::Absent
        };
        let mut msg = wire::DecideMessage::new(
            ctx.context(name),
            attempt,
            elapsed.as_millis() as u64,
            wire::Delivery::Retry,
            if failure.is_some() {
                "failure"
            } else {
                "success"
            }
            .into(),
            wire::DecideRequest::new(
                wire::RequestMeta::new(req.method.to_string(), req.target_url.clone()),
                body,
            ),
            fallback.clone(),
        );
        msg.failure = failure.map(|f| {
            let mut x = wire::Failure::new(f.code.into());
            x.details = Some(f.details.clone());
            x
        });
        msg.response = preview.clone();
        let (reply, headers) = match turn
            .send(wire::Stage::Decide(msg), pool.entry.decide_timeout_secs)
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(format=%safe,error=%e,"decide failed; no vote");
                continue;
            }
        };
        if let wire::ReportField::Present(report) = reply.report {
            ctx.rec
                .format_report((layer, name), "decide", Some(attempt), report);
        }
        ctx.absorb(name, reply.state);
        if reply.fail.is_some() {
            continue;
        }
        if votes.reroute.is_none() {
            votes.reroute = reply.reroute;
        }
        if votes.decision.is_none() {
            votes.decision = reply.decision.and_then(|d| {
                Some(match d {
                    Retry => Verdict::Retry {
                        name: safe,
                        delay: reply.delay_ms.map(|d| Duration::from_millis(d.millis())),
                    },
                    Stop => Verdict::Stop {
                        name: safe,
                        patch: reply.response.map(|p| (p, headers)),
                    },
                    _ => return None,
                })
            });
        }
        if matches!(votes.decision, Some(Verdict::Stop { .. }))
            || (votes.decision.is_some()
                && (votes.reroute.is_some() || !state.has_format(Stage::Retry)))
        {
            break;
        }
    }
    votes
}

/// 决定阶段的响应补丁应用于真实上游体，而不是给程序看的有界预览。停止以后不进响应转换链。
pub(super) async fn finish(
    r: &mut Responder,
    trace: &RequestTrace,
    terminal: Terminal,
    response: Option<UpstreamResponse>,
    patch: Option<(wire::ResponsePatch, Option<HeaderMap>)>,
    client_head: bool,
) {
    let supplied = patch.is_some();
    if !supplied && (response.is_none() || r.is_committed()) {
        r.finish(terminal, trace).await;
        return;
    }
    let mut response = response.unwrap_or_else(|| UpstreamResponse {
        status: StatusCode::BAD_GATEWAY,
        headers: HeaderMap::new(),
        raw_headers: HeaderMap::new(),
        body: SpooledBody::Memory(Bytes::new()),
    });
    if let Some((patch, headers)) = patch {
        if let Some(status) = patch.status {
            response.status = StatusCode::from_u16(status).expect("validated response status");
        }
        if let Some(bytes) = patch.body.as_bytes() {
            response.body = SpooledBody::Memory(Bytes::copy_from_slice(bytes));
            response.headers.remove(http::header::CONTENT_ENCODING);
            response.headers.remove(http::header::CONTENT_LENGTH);
        }
        if let Some(headers) = headers {
            response.headers = headers;
        }
    }
    let rec = &trace.rec;
    rec.settle(
        if supplied {
            crate::history::Outcome::FormatResponse
        } else {
            crate::history::Outcome::UpstreamError
        },
        Some(terminal.code()),
    );
    rec.phase(crate::history::Phase::Replaying);
    if !r.is_committed() {
        if delivers_body(client_head, response.status) {
            rec.response_bytes(response.body.len());
        }
        let mut out = replay_failure_as_is(
            response.status,
            response.headers,
            &response.raw_headers,
            response.body,
        )
        .await;
        mark_given_up(out.headers_mut(), terminal.code(), trace);
        r.respond(out);
        return;
    }
    if terminal_is_sse(&response.headers) {
        let mut stream = response.body.into_stream().await;
        while let Some(chunk) = stream.next().await {
            let Ok(chunk) = chunk else {
                break;
            };
            let len = chunk.len();
            if !r.send(Ok(chunk)).await {
                break;
            }
            rec.response_bytes(len as u64);
        }
    } else {
        let bytes = match &response.body {
            SpooledBody::Memory(b) => Some(b.clone()),
            SpooledBody::Disk { path, .. } => tokio::fs::read(path).await.ok().map(Bytes::from),
        };
        if let Some(event) = bytes.as_deref().and_then(terminal_json_event) {
            let len = event.len();
            if r.send(Ok(event)).await {
                rec.response_bytes(len as u64);
            }
        } else {
            r.finish(terminal, trace).await;
        }
    }
}
