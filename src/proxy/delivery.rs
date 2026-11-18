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
    /// 在途数的守卫：从收到请求算起，到这里被丢弃为止
    _in_flight: crate::daemon::Counted,
}

impl Delivery {
    pub(super) fn new(rec: Recorder, in_flight: crate::daemon::Counted) -> Self {
        Self {
            rec,
            delivered: false,
            _in_flight: in_flight,
        }
    }

    /// 处理函数交出了响应：从这里起由响应体接着计。`bodiless`：这个响应按协议没有体可写
    /// （HEAD 请求的响应），连接层不会去读它。
    pub(super) fn attach(mut self, response: Response, bodiless: bool) -> Response {
        let status = response.status();
        self.rec.client_status(status.as_u16());
        response.map(|body| {
            use axum::body::HttpBody as _;
            // 没有体可写的响应，连接层一次都不读就丢掉它：这不是客户端断开
            self.delivered = bodiless
                || body.is_end_stream()
                || status.is_informational()
                || matches!(status, StatusCode::NO_CONTENT | StatusCode::NOT_MODIFIED);
            Body::new(Delivered {
                body,
                delivery: self,
            })
        })
    }
}

impl Drop for Delivery {
    fn drop(&mut self) {
        self.rec.end(self.delivered);
    }
}

/// 带着 [`Delivery`] 的响应体。字段的次序有讲究：先丢 `body`、再丢 `delivery`——体里的流
/// 在被丢弃时还要往请求记录里写（收了多少字节），记录的收尾得排在它们之后。
struct Delivered {
    body: Body,
    delivery: Delivery,
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
            Poll::Ready(Some(Ok(_))) if this.body.is_end_stream() => {
                this.delivery.delivered = true;
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
