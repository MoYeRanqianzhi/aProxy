//! 响应的交付：一个请求到什么时候算结束，结束时是交付完了还是客户端先走了。
//!
//! 处理函数返回时，保活流、回放与仅转发的响应体都还在往外写，所以「结束」只能从响应体上
//! 判断：它被读到头了是交付完，没读到头就被丢掉是客户端断开。在途请求数与请求历史里的
//! 记录都靠这一处收尾。

use super::*;
use crate::history::Recorder;
use std::pin::Pin;

/// 一个请求从收到到响应交付完的全程。被丢弃时给请求记录收尾、把在途数减回去——不管是
/// 响应体写完了、客户端中途断开，还是处理函数还没交出响应就被取消。
pub(super) struct Delivery {
    rec: Recorder,
    /// 响应已经完整交给了客户端
    delivered: bool,
    /// 仅在请求进入配置的格式链之后才登记，丢弃响应也能发出最终结果。
    notify: Option<(Arc<[Arc<crate::transform::Pool>]>, ExchangeCtx)>,
    /// 以客户端实际拿到的媒体类型判断是否应给出流的完成状态。
    streamed: bool,
    /// 在途数的守卫：从收到请求算起，到这里被丢弃为止
    _in_flight: crate::daemon::Counted,
}

/// 以原始客户端方法和最终状态为准；重试链可以改写发往上游的方法，却不能改变这里的语义。
pub(super) fn delivers_body(head: bool, status: StatusCode) -> bool {
    !head
        && !status.is_informational()
        && !matches!(status, StatusCode::NO_CONTENT | StatusCode::NOT_MODIFIED)
}

impl Delivery {
    pub(super) fn new(rec: Recorder, in_flight: crate::daemon::Counted) -> Self {
        Self {
            rec,
            delivered: false,
            notify: None,
            streamed: false,
            _in_flight: in_flight,
        }
    }

    /// 没有订阅程序时不克隆上下文，也不在完成路径构造或排队任何消息。
    pub(super) fn arm_notify(&mut self, state: &AppState, ctx: Option<&ExchangeCtx>) {
        if state.has_format(crate::config::file::Stage::Notify) {
            self.notify = Some((
                state.formats.clone(),
                ctx.expect("notify has format context").clone(),
            ));
        }
    }
    /// 处理函数交出了响应：从这里起由响应体接着计。`bodiless`：这个响应按协议没有体可写
    /// （HEAD 请求的响应），连接层不会去读它。
    pub(super) fn attach(mut self, response: Response, bodiless: bool) -> Response {
        let status = response.status();
        self.rec.client_status(status.as_u16());
        self.streamed = self.notify.is_some()
            && delivers_body(bodiless, status)
            && retry::is_streaming_content_type(response.headers());
        // 未缓冲的 forward 体没有自己的精确 size_hint；HTTP 定长帧写满时，连接层不会再来取 EOF。
        let remaining = response
            .headers()
            .get(http::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok()?.parse::<u64>().ok());
        response.map(|body| {
            use axum::body::HttpBody as _;
            // 没有体可写的响应，连接层一次都不读就丢掉它：这不是客户端断开
            self.delivered =
                !delivers_body(bodiless, status) || body.is_end_stream() || remaining == Some(0);
            Body::new(Delivered {
                body,
                delivery: self,
                remaining,
            })
        })
    }
}

impl Drop for Delivery {
    fn drop(&mut self) {
        self.rec.end(self.delivered);
        if let Some((pools, ctx)) = &self.notify
            && let Some(mut end) = self.rec.completion(self.delivered, self.streamed)
        {
            end.client = Some(ctx.wire_client());
            for pool in pools
                .iter()
                .filter(|p| p.entry.has_stage(crate::config::file::Stage::Notify))
            {
                end.context =
                    ctx.body_context(&pool.entry.name, &aproxy_envelope::BodyNeed::Digest);
                pool.enqueue_notify(end.clone());
            }
        }
    }
}

/// 带着 [`Delivery`] 的响应体。字段的次序有讲究：先丢 `body`、再丢 `delivery`——体里的流
/// 在被丢弃时还要往请求记录里写（收了多少字节），记录的收尾得排在它们之后。
struct Delivered {
    body: Body,
    delivery: Delivery,
    /// 定长响应到 HTTP 消息末尾还差的字节；chunked 仍以真实 EOF 为准。
    remaining: Option<u64>,
}

impl axum::body::HttpBody for Delivered {
    type Data = Bytes;
    type Error = axum::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Bytes>, axum::Error>>> {
        let this = &mut *self;
        let polled = Pin::new(&mut this.body).poll_frame(cx);
        match &polled {
            Poll::Ready(None) => this.delivery.delivered = true,
            // 长度已知的体写完最后一帧就算到头，连接层不会再来读一次
            Poll::Ready(Some(Ok(frame))) => {
                if let (Some(left), Some(data)) = (&mut this.remaining, frame.data_ref()) {
                    *left = left.saturating_sub(data.len() as u64);
                }
                this.delivery.delivered |= this.body.is_end_stream() || this.remaining == Some(0);
            }
            _ => {}
        }
        polled
    }

    fn is_end_stream(&self) -> bool {
        self.body.is_end_stream()
    }

    fn size_hint(&self) -> http_body::SizeHint {
        self.body.size_hint()
    }
}
