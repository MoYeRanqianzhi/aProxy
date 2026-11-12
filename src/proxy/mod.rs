//! 代理核心：将本地请求完整透传到上游，并在失败时无限重试。
//!
//! 设计要点：
//! - 单一 upstream base URL，其余路径与查询参数完整透传（包含 `/health` 等，上游若有同名路径亦完整透传）
//! - 网络错误 / 4xx / 5xx / 错误 JSON 内容 → 全部无限重试，梯度延迟（`retry` 模块）
//!   原型阶段 4xx 同样视为易发瞬时错误（如限流、临时鉴权波动、上游误报）
//! - 非流式与流式统一缓冲策略（内存 + 磁盘双模，`disk_cache` 配置）：
//!   1. 对上游响应先完整缓冲（spool），期间任何网络中断均视为 `NetworkError`
//!      触发重试——满足“流式中断可重试”的需求。缓冲超过内存驻留阈值（1 MiB）
//!      时溢写到磁盘临时文件（`disk_cache` 开启时）：进程内存与负载大小解耦，
//!      page cache 提供读写弹性（SSD 上 IO 开销为噪声级）；关闭时全程纯内存
//!      （与旧行为一致）。
//!   2. 缓冲完成后，再做“是否可重试”的判定：
//!      - HTTP 状态码可重试（4xx / 5xx，`is_retryable_status`）
//!      - 非流式：`is_error_body` 命中（任意状态码下只要 body 语义为报错就重试）
//!      - 流式：`is_stream_error_body` 命中（扫描 SSE data 行 / NDJSON）。
//!        磁盘模式用增量行扫描（`StreamErrorScanner`，语义与行级判定一致）；
//!        「整体 body 单 JSON 兜底」仅在内存模式执行——错误 JSON 均为 KB 级，
//!        不会溢写到磁盘。
//!   3. 仅当整轮缓冲成功且判定为不可重试时，才将缓冲体回放给客户端；
//!      流式判定（或磁盘模式）以 chunked 流式分块原样回放，逐块产出保持与
//!      上游字节完全一致，SSE 解析器语义不受影响。
//! - 请求体同样双模：小请求体驻留内存（Bytes 零拷贝重试共享），大请求体
//!   溢写磁盘、每次重试重新流式读取。上限 `max_body_mb`（默认 128，0=不限）。
//! - 一个尝试循环（`attempts::run_attempts`）：每次尝试的结果由 `classify` 归成成功、
//!   重试、受限路径用尽或本地失败，两种出口（`attempts::Responder`）按结果收场。
//!   提交之前就成功的尝试（首轮成功是常态）一律走保真快速路径：status 与全部
//!   响应头原样回放。
//! - 保活出口：「保活适用」的请求（`keepalive_trigger`
//!   判定命中：Accept 含 text/event-stream 和/或客户端原始请求体顶层
//!   `"stream": true`；且 keepalive 开启、非 forward_only）的尝试循环整个由后台
//!   任务驱动。响应的「提交点」是一个三态机——未提交 → 提交上游真实头 / 提交骨架头：
//!   - 上游回 2xx + 未压缩的 text/event-stream 头 → 立即把上游真实 status 与
//!     响应头转给客户端（未配置 response_transform 时）
//!   - 一个 keepalive 间隔内仍无可提交的结果，或需要重试 → 提交骨架头
//!     （200 + text/event-stream）
//!   - 未提交前首轮就成功完成 → 走与上面相同的保真快速路径
//!
//!   提交之后，无论在等上游响应头、缓冲上游流还是退避，都每
//!   `keepalive_interval_secs` 发一行 SSE 注释（`: keepalive\n\n`）；完整缓冲、
//!   判定无误后才把成功那一次的原样字节写进同一个响应，期间的重试客户端只见到
//!   心跳。客户端断开（任一阶段）即中止在途上游请求、不再发起新请求。
//!   不适用保活的请求（如 stream:false 的普通 JSON 请求）走不保活出口：尝试循环
//!   就在 handler 的 future 里跑，成功后一次性回放，期间不向客户端写任何字节。
//! - 入站来源校验（handler 第一步）：Host 不在 `allowed_hosts`、或带了不在
//!   `allowed_origins` 里的 Origin 的请求本地 403——不转发、不注入 api_key、
//!   不进重试循环（防 DNS 重绑定与网页跨站调用；语义见 `Config` 同名字段）
//! - 头处理：`api_key` 快捷覆盖 `Authorization: Bearer` 与 `x-api-key`（Anthropic 风格），
//!   `extra_headers` 追加缺失头，`override_headers` 无条件覆盖（兼容非 Bearer 鉴权与额外头需求）
//! - 磁盘临时文件生命周期：请求体随请求结束（Drop）删除；响应 spool 在回放流
//!   结束/客户端断开/重试丢弃时删除；进程崩溃残留由启动时 `clean_spool_dir`
//!   回收（每实例独立子目录，互不干扰）。

use axum::{
    Router,
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use bytes::Bytes;
use futures_util::{Stream, StreamExt as _};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, atomic::Ordering as AtomicOrdering},
    task::{Poll, ready},
    time::Duration,
};
use tokio::io::{AsyncRead as _, AsyncReadExt as _, AsyncWriteExt as _};
use tokio_util::io::ReaderStream;

use crate::{
    config::Config,
    retry,
    transform::{ExchangeCtx, HeartbeatSource},
};

mod attempts;
mod body;
mod errors;
mod forward_only;
mod handler;
mod headers;
mod inbound;
mod inspect;
mod keepalive;
mod replay;
mod spool;
mod state;
#[cfg(test)]
mod tests;
mod transforms;
mod trigger;
mod upstream;

// 兄弟模块与 tests 经各自的 `use super::*;` 互相看到对方的条目（拆分前它们同在一个文件里）
use attempts::*;
use body::*;
use errors::*;
use forward_only::*;
use headers::*;
use inbound::*;
use inspect::*;
use keepalive::*;
use replay::*;
use spool::*;
use transforms::*;
use trigger::*;
use upstream::*;

pub(crate) use body::RESIDENT_LIMIT;
pub use body::RequestBody;
pub use handler::router;
pub(crate) use headers::is_hop_header;
pub use spool::SpooledBody;
pub(crate) use spool::unique_seq;
pub use state::AppState;
