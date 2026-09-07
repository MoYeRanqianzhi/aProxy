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
//! - 首轮快速路径：attempt 1 的结果先做判定，成功则直接原样回放（status 与全部
//!   响应头保真）；仅当需要重试时才进入重试通道——首轮成功是常态路径，保真优先。
//! - 重试期间对客户端的保活：仅在「需要重试」且客户端接受 SSE 时，才立即返回
//!   SSE 流式骨架，由后台任务从 attempt 2 继续无限重试，并在重试间隙向下游发送
//!   SSE 注释保活（`: keepalive ...\n\n`），防止客户端因 idle 超时而断开；
//!   后台任务在客户端断开（channel 关闭）时立即退出，不空转。
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

use crate::{config::Config, retry};

/// 需要过滤的 hop-by-hop 头，避免透传导致协议错误或与 hyper/reqwest 的
/// 自动管理（content-length / transfer-encoding / host / connection）冲突。
const HOP_HEADERS: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "host",
    "content-length",
];

fn is_hop_header(name: &str) -> bool {
    HOP_HEADERS.contains(&name.to_ascii_lowercase().as_str())
}

/// 共享状态
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub client: reqwest::Client,
    /// 最近一次收到客户端请求的时刻（Unix 秒）。AtomicU64 供 IPC ping 读取
    /// 展示/判闲置，请求热路径上仅一次 store（Relaxed 足够：只用于粗粒度的
    /// 空闲判定，无需跨线程因果序）。
    pub last_activity_secs: Arc<std::sync::atomic::AtomicU64>,
    /// IPC 实时观测源（请求数/重试数/最近错误）：热路径各一条 Relaxed 原子
    /// 操作，量级同上；serve_forever 组装进 IpcStats 供 IPC 响应读取。
    pub stats: Arc<crate::daemon::IpcStats>,
    /// 磁盘缓存的 spool 临时目录；None = disk_cache 关闭（全程纯内存）。
    /// 生产路径 `~/.aproxy/spool/<端口>/`，测试可经 Config::spool_dir_override 注入。
    pub spool_dir: Option<PathBuf>,
}

impl AppState {
    pub fn new(config: Config) -> Self {
        // 超时策略：不设总时限（会掐断超过时限的慢流式生成，导致无限重试永不成功），
        // 只限制连接建立与两次读到数据之间的间隔（均可经 config.toml 调整）。
        // 注意 read_timeout 同样钳制首字节等待——LLM 上游排队时 TTFB 可达数十秒，
        // 阈值过小（如 60s）会把「慢但活着」的上游变成确定性无限重试。spool 设计
        // 本身容忍慢流。0 表示该项不设限。
        let mut builder = reqwest::Client::builder()
            .pool_idle_timeout(Duration::from_secs(90))
            .tcp_keepalive(Duration::from_secs(30))
            // 透明代理不跟随重定向：跟随会把 Authorization/api_key 与请求体外带到
            // 3xx 指向的任意主机，且 3xx 永远到不了客户端；禁用后 3xx 作为普通
            // 成功响应原样回放（is_retryable_status 本就排除 3xx）。
            .redirect(reqwest::redirect::Policy::none());
        if config.connect_timeout_secs > 0 {
            builder = builder.connect_timeout(Duration::from_secs(config.connect_timeout_secs));
        }
        if config.read_timeout_secs > 0 {
            builder = builder.read_timeout(Duration::from_secs(config.read_timeout_secs));
        }

        // 显式配置代理：所有上游请求经该代理转发；reqwest 在 .proxy() 时会自动关闭
        // 系统代理（不再读取 HTTP_PROXY 等环境变量），避免两者互相干扰。
        // 代理 URL 已在 Config::validate() 校验，此处 expect 不会失败。
        if let Some(proxy_url) = config.proxy.as_deref() {
            let mut proxy = reqwest::Proxy::all(proxy_url).expect("代理 URL 已在 validate() 校验");
            // 单独配置的用户名/密码优先于 URL 内嵌凭据；仅配置密码而无用户名则忽略
            if let Some(username) = config.proxy_username.as_deref() {
                proxy = proxy.basic_auth(username, config.proxy_password.as_deref().unwrap_or(""));
            }
            builder = builder.proxy(proxy);
        }

        let client = builder.build().expect("构建 reqwest client 失败");
        // 磁盘缓存目录：关闭（disk_cache=false）时为 None——全程纯内存；
        // 开启时用测试注入覆盖，否则按端口取 ~/.aproxy/spool/<端口>/
        let spool_dir = if config.disk_cache_enabled() {
            Some(config.spool_dir_override.clone().unwrap_or_else(|| {
                crate::daemon::spool_dir_for(crate::daemon::port_of(&config.listen_addr))
            }))
        } else {
            None
        };
        Self {
            spool_dir,
            config: Arc::new(config),
            client,
            last_activity_secs: Arc::new(std::sync::atomic::AtomicU64::new(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0),
            )),
            stats: Arc::new(crate::daemon::IpcStats::default()),
        }
    }

    /// 记录一次上游失败摘要（覆盖式，只保留最近一次；IPC/status 展示用）
    fn note_upstream_failure(&self, msg: &str) {
        self.stats.record_error(msg);
    }

    /// 实例闲置时长（秒）：距最近一次收到客户端请求。
    pub fn idle_secs(&self) -> u64 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        now.saturating_sub(
            self.last_activity_secs
                .load(std::sync::atomic::Ordering::Relaxed),
        )
    }
}

// ---------------------------------------------------------------------------
// 双模缓冲：内存（Bytes）与磁盘（临时文件）统一抽象
//
// 收集阶段超内存阈值即溢写磁盘；消费阶段（重试重放/回放给客户端）统一按
// Bytes 块产出。磁盘模式的内存在途占用为流式缓冲级（常数），与负载大小解耦。
// ---------------------------------------------------------------------------

/// 内存驻留阈值：缓冲超过即溢写磁盘（disk_cache 开启时）。
/// 1 MiB：覆盖真实 agent 流量（SSE/JSON 错误体 KB 级）使其零磁盘开销，
/// 大请求体/大响应才进入磁盘路径。
const RESIDENT_LIMIT: usize = 1024 * 1024;

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

/// 读取请求体错误：TooLarge=超出上限（413）；Io=连接中断/磁盘写入失败
enum ReadBodyError {
    TooLarge,
    Io(std::io::Error),
}

/// 读取请求体：内存缓冲，超 `RESIDENT_LIMIT` 且 disk_cache 可用则溢写磁盘。
/// `limit` 超出返回 TooLarge；客户端中断或写盘失败返回 Io。
async fn read_request_body(
    body: axum::body::Body,
    limit: usize,
    spool_dir: Option<&Path>,
) -> Result<RequestBody, ReadBodyError> {
    let mut body = body.into_data_stream();
    let mut mem: Vec<u8> = Vec::new();
    let mut disk: Option<(tokio::fs::File, PathBuf, u64)> = None;
    loop {
        let chunk = match body.next().await {
            Some(Ok(c)) => c,
            Some(Err(e)) => {
                if let Some((_, path, _)) = disk.take() {
                    let _ = std::fs::remove_file(&path);
                }
                return Err(ReadBodyError::Io(std::io::Error::other(e)));
            }
            None => break,
        };
        // 上限判定：磁盘模式下基于累计总量；内存模式基于 Vec 长度
        let total = disk.as_ref().map_or(mem.len(), |(_, _, l)| *l as usize);
        if limit != usize::MAX && total + chunk.len() > limit {
            if let Some((_, path, _)) = disk.take() {
                let _ = std::fs::remove_file(&path);
            }
            return Err(ReadBodyError::TooLarge);
        }
        match &mut disk {
            Some((file, _, len)) => {
                if file.write_all(&chunk).await.is_err() {
                    let (_, path, _) = disk.take().expect("disk 存在");
                    let _ = std::fs::remove_file(&path);
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
                        let path = dir.join(format!(
                            "req-{}-{}.spooltmp",
                            std::process::id(),
                            unique_seq()
                        ));
                        // 目录不可用（create 失败）：内存退化
                        if let Ok(mut file) = tokio::fs::File::create(&path).await {
                            if file.write_all(&mem).await.is_ok()
                                && file.write_all(&chunk).await.is_ok()
                            {
                                disk = Some((file, path, (mem.len() + chunk.len()) as u64));
                                mem = Vec::new();
                                continue;
                            }
                            // 半截文件必须删除
                            drop(file);
                            let _ = std::fs::remove_file(&path);
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
                let _ = std::fs::remove_file(&path);
                return Err(ReadBodyError::Io(std::io::Error::other(
                    "请求体磁盘缓存写入失败",
                )));
            }
            drop(file);
            RequestBody::Disk { path, len }
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

/// 响应 spool 缓冲：内存累积，超 `RESIDENT_LIMIT` 溢写磁盘。
/// 完成后统一为 `SpooledBody` 消费（判定/回放）。
enum SpoolBuffer {
    Memory(Vec<u8>),
    Disk {
        file: tokio::fs::File,
        path: PathBuf,
        len: u64,
    },
    /// 收集中途写盘失败：内部状态不可用，调用方转为可重试的 NetworkError
    Poisoned,
}

impl SpoolBuffer {
    fn len(&self) -> u64 {
        match self {
            SpoolBuffer::Memory(mem) => mem.len() as u64,
            SpoolBuffer::Disk { len, .. } => *len,
            SpoolBuffer::Poisoned => 0,
        }
    }

    /// 丢弃收集结果（TooLarge / 读取中断路径）：磁盘模式删除半截临时文件
    ///（先关写句柄再删，Windows 要求）。Poisoned 的半截文件已在写失败处删除。
    fn discard(self) {
        if let SpoolBuffer::Disk { file, path, .. } = self {
            drop(file);
            let _ = std::fs::remove_file(&path);
        }
    }

    /// 收集完成：内存 → SpooledBody::Memory（重试判定走整体扫描旧路径，
    /// disk_scan=None）；磁盘 → 关写句柄产出 SpooledBody::Disk +（增量扫描
    /// 结论, 头部快照）。消费 scanner。
    async fn finish(self, scanner: StreamErrorScanner) -> (SpooledBody, Option<(bool, Vec<u8>)>) {
        match self {
            SpoolBuffer::Memory(mem) => (SpooledBody::Memory(Bytes::from(mem)), None),
            SpoolBuffer::Disk { file, path, len } => {
                drop(file); // 关写句柄，头部快照以只读重新打开
                let head = match tokio::fs::File::open(&path).await {
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
                (SpooledBody::Disk { path, len }, Some((error, head)))
            }
            // 不可达：Poisoned 在 forward_once 收集循环中提前返回。防御性产出
            // 空 body 成功（与磁盘判定缺失同样按不重试处理，服务不因内部
            // 不变量被破坏而失能）
            SpoolBuffer::Poisoned => (SpooledBody::Memory(Bytes::new()), Some((false, Vec::new()))),
        }
    }
}

/// 扫描器可依赖的行长度上界：错误 JSON 行远小于此；超出即截断该行（丢一行
/// 启发式判定正确性，换内存上界恒定）。
const SCAN_MAX_LINE: usize = 512 * 1024;

/// 磁盘模式的流式错误扫描器：增量逐行判定（SSE data 行 / NDJSON 行），
/// 与 `retry::is_stream_error_body` 的行级语义一致（见其测试锚定：
/// 流尾 error 事件必须命中、CRLF/前导空格变体兼容）。
struct StreamErrorScanner {
    /// 跨 chunk 的未成行尾巴
    tail: Vec<u8>,
    /// 已判定出现 data 行（SSE 模式锁定）
    saw_data_line: bool,
    /// SSE 模式下的判定结果（锁定后不再改写）
    sse_has_error: bool,
    /// 已锁定结论
    decided: bool,
    /// 非 SSE 的 NDJSON 命中
    ndjson_error: bool,
}

impl StreamErrorScanner {
    fn new() -> Self {
        Self {
            tail: Vec::new(),
            saw_data_line: false,
            sse_has_error: false,
            decided: false,
            ndjson_error: false,
        }
    }

    fn feed(&mut self, chunk: &[u8]) {
        if self.decided {
            return;
        }
        self.tail.extend_from_slice(chunk);
        // 行缓存上界：超出即剪裁（丢判定正确性，换内存上界恒定）
        if self.tail.len() > SCAN_MAX_LINE {
            let drain = self.tail.len() - SCAN_MAX_LINE;
            self.tail.drain(..drain);
        }
        while let Some(pos) = self.tail.iter().position(|&b| b == b'\n') {
            let mut line: Vec<u8> = self.tail.drain(..=pos).collect();
            line.pop(); // 去 \n
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            self.scan_line(&line);
            if self.decided {
                self.tail.clear();
                return;
            }
        }
    }

    fn scan_line(&mut self, line: &[u8]) {
        let Ok(text) = std::str::from_utf8(line) else {
            return;
        };
        let trimmed = text.trim();
        let data = trimmed
            .strip_prefix("data:")
            .or_else(|| trimmed.strip_prefix("data :"))
            .map(str::trim);
        if let Some(data) = data {
            self.saw_data_line = true;
            if !data.is_empty() && data != "[DONE]" && retry::is_error_body(data.as_bytes()) {
                self.sse_has_error = true;
                self.decided = true;
            }
        } else if trimmed.starts_with('{') && retry::is_error_body(trimmed.as_bytes()) {
            self.ndjson_error = true;
            self.decided = true;
        }
    }

    /// 流结束后的最终判定：语义对齐 is_stream_error_body 的行级部分——
    /// 出现过 data 行则只以 SSE 结论为准；否则 NDJSON 命中为真。
    /// （「整体 body 单 JSON 兜底」不在此处：仅内存模式保留，见
    /// needs_retry_response 的 Memory 分支。差异依据：错误 JSON 均为 KB 级，
    /// 超过内存驻留阈值的响应几乎不可能是错误 JSON，且「首行即 JSON」的
    /// NDJSON 已由 scan_line 覆盖。）
    fn finish(mut self) -> bool {
        if self.decided {
            return self.sse_has_error || self.ndjson_error;
        }
        // 未成行的最后一行按行扫描（is_stream_error_body 的 lines() 同样
        // 会产出无换行结尾的最后一行）
        if !self.tail.is_empty() {
            let tail = std::mem::take(&mut self.tail);
            self.scan_line(&tail);
        }
        if self.saw_data_line {
            self.sse_has_error
        } else {
            self.ndjson_error
        }
    }
}

/// 收集一段响应体到 SpoolBuffer（内存累积 + 溢写磁盘）。
/// 磁盘不可用（目录创建失败/写失败）时回退纯内存继续收集——磁盘满不应使
/// 代理失能，最多退化为内存模式；唯有「已落盘后中途写失败」才 Poisoned。
async fn spool_chunk(buf: &mut SpoolBuffer, chunk: &[u8], spool_dir: Option<&Path>) {
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
            let path = dir.join(format!(
                "spool-{}-{}.spooltmp",
                std::process::id(),
                unique_seq()
            ));
            match tokio::fs::File::create(&path).await {
                Ok(mut file) => {
                    if file.write_all(mem).await.is_ok() && file.write_all(chunk).await.is_ok() {
                        let len = (mem.len() + chunk.len()) as u64;
                        *buf = SpoolBuffer::Disk { file, path, len };
                        return;
                    }
                    // 写失败：文件句柄已创建但内容不完整，删除；保持 Memory 退化
                    //（create 成功 write 失败极罕见，但半截文件绝不能留下）
                    drop(file);
                    let _ = std::fs::remove_file(&path);
                    mem.extend_from_slice(chunk);
                }
                Err(_) => mem.extend_from_slice(chunk),
            }
        }
        SpoolBuffer::Disk { file, len, path } => {
            // 已落盘后写失败：进程内 spool 无法回退（数据已在盘上），只能
            // Poisoned。半截文件在此删除（先关句柄）。
            if file.write_all(chunk).await.is_err() {
                let p = path.clone();
                *buf = SpoolBuffer::Poisoned;
                let _ = std::fs::remove_file(&p);
                return;
            }
            *len += chunk.len() as u64;
        }
        SpoolBuffer::Poisoned => {}
    }
}

/// 临时文件名唯一序号：进程内单调即可（同进程不重名；跨进程由 pid 区分）
fn unique_seq() -> u64 {
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
    fn take_memory(&mut self) -> Bytes {
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
struct DiskSpoolStream {
    /// Some((file, 剩余字节))；None = 已结束
    inner: Option<(tokio::fs::File, u64)>,
    path: PathBuf,
}

impl DiskSpoolStream {
    fn finish(&mut self) {
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

/// 构建路由：全量透传（无额外健康检查路径，避免与上游 `/health` 等冲突）
pub fn router(state: AppState) -> Router {
    Router::new().fallback(proxy_handler).with_state(state)
}

/// 将配置中的头覆盖/追加逻辑应用到待发往上游的 HeaderMap（大小写不敏感判定）。
fn apply_header_overrides(headers: &mut HeaderMap, config: &Config) {
    // api_key 快捷：等效覆盖 Authorization: Bearer <key>（大小写不敏感覆盖）。
    // 同时覆盖 x-api-key（Anthropic 风格上游使用该头携带原始 key，若只覆盖
    // Authorization，客户端原带的 x-api-key 会原样漏到上游造成鉴权混乱）。
    if let Some(key) = config.api_key.as_deref() {
        let val = format!("Bearer {key}");
        if let Ok(v) = HeaderValue::from_str(&val) {
            headers.remove(http::header::AUTHORIZATION);
            headers.insert(http::header::AUTHORIZATION, v);
        }
        if let Ok(raw) = HeaderValue::from_str(key) {
            headers.remove("x-api-key");
            headers.insert(HeaderName::from_static("x-api-key"), raw);
        }
    }

    // override_headers：无条件覆盖
    for (k, v) in &config.override_headers {
        let Ok(name) = HeaderName::from_bytes(k.as_bytes()) else {
            continue;
        };
        let Ok(val) = HeaderValue::from_str(v) else {
            continue;
        };
        headers.remove(&name);
        headers.insert(name, val);
    }

    // extra_headers：仅当未携带时追加
    for (k, v) in &config.extra_headers {
        let Ok(name) = HeaderName::from_bytes(k.as_bytes()) else {
            continue;
        };
        if headers.contains_key(&name) {
            continue;
        }
        let Ok(val) = HeaderValue::from_str(v) else {
            continue;
        };
        headers.insert(name, val);
    }
}

/// 核心代理处理器：完整透传 + 无限重试 + 流式 spool 后回放 + 保活心跳
async fn proxy_handler(State(state): State<AppState>, req: Request) -> Response {
    let method = req.method().clone();
    let uri = req.uri().clone();
    let mut headers = req.headers().clone();

    // 活动时间戳：收到请求即更新（stop idle / status 筛选的判定依据）
    state.last_activity_secs.store(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        std::sync::atomic::Ordering::Relaxed,
    );
    // IPC 观测计数（一条 Relaxed fetch_add，与上面 store 同量级）
    state
        .stats
        .requests_total
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    // 在本地侧先应用覆盖/追加，避免重试间重复计算
    apply_header_overrides(&mut headers, &state.config);

    // 缓冲请求体以支持重试重放；上限 max_body_mb（默认 128 MB，0=不限），
    // 超出直接 413。disk_cache 开启时超过内存驻留阈值（1 MiB）溢写磁盘，
    // 大请求体的进程内存在途占用恒定。
    let body_limit = state.config.body_limit_bytes();
    let req_body = match read_request_body(req.into_body(), body_limit, state.spool_dir.as_deref())
        .await
    {
        Ok(b) => b,
        Err(ReadBodyError::TooLarge) => {
            let limit_text = if body_limit == usize::MAX {
                "不设限".to_string()
            } else {
                format!("{} MiB", body_limit / 1024 / 1024)
            };
            tracing::warn!(limit = %limit_text, "请求体超出上限");
            return (
                StatusCode::PAYLOAD_TOO_LARGE,
                format!("请求体超出上限（{limit_text}），可在 settings.json 的 max_body_mb 或 config.toml 的 max_body_mb 调整"),
            )
                .into_response();
        }
        Err(ReadBodyError::Io(e)) => {
            tracing::error!(error = %e, "读取请求体失败");
            return (
                StatusCode::BAD_REQUEST,
                format!("请求体读取失败（连接中断或本地磁盘缓存写入失败）: {e}"),
            )
                .into_response();
        }
    };

    let path_and_query = uri.path_and_query().map(|pq| pq.as_str()).unwrap_or("/");

    let upstream_base = state.config.base_url.trim_end_matches('/');
    let target_url = format!("{}{}", upstream_base, path_and_query);

    tracing::info!(method = %method, path = %path_and_query, target = %target_url, "代理请求");

    // 是否启用 SSE 保活心跳（仅当客户端接受 SSE 且配置启用）
    let client_wants_sse = headers
        .get(http::header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_ascii_lowercase().contains("text/event-stream"))
        .unwrap_or(false);
    let keepalive_dur = state.config.keepalive_interval();
    let keepalive_enabled = state.config.keepalive_enabled() && keepalive_dur.as_secs() > 0;

    // 首轮（attempt 1）先行：成功则完整保真回放（status/headers 不失真），
    // 需要重试才进入重试通道——首轮成功是常态路径。
    let max_spool_bytes = spool_limit_bytes(&state.config);
    let first = forward_once(
        &state.client,
        &method,
        &target_url,
        &headers,
        &req_body,
        max_spool_bytes,
        state.spool_dir.as_deref(),
    )
    .await;

    let needs_retry = match &first {
        ForwardResult::NetworkError(e) => {
            state.note_upstream_failure(&format!("网络错误: {e}"));
            tracing::warn!(error = %e, "首轮上游网络错误，进入重试");
            true
        }
        // spool 上限 / 本地磁盘故障重试无意义（确定性失败），直接终态回放 502
        ForwardResult::TooLarge => {
            state.note_upstream_failure("上游响应体超出 spool 上限");
            false
        }
        ForwardResult::SpoolFailed(e) => {
            state.note_upstream_failure(&format!("本地 spool 故障: {e}"));
            tracing::error!(error = %e, "首轮本地 spool 故障，终态返回");
            false
        }
        ForwardResult::Response {
            status,
            raw_headers,
            body,
            disk_scan,
            ..
        } => {
            let retry = needs_retry_response(1, status, raw_headers, body, disk_scan);
            if retry {
                state.note_upstream_failure(&format!("上游返回 {status}（错误内容，重试）"));
            }
            retry
        }
    };

    if !needs_retry {
        return match first {
            ForwardResult::TooLarge => (
                StatusCode::BAD_GATEWAY,
                "上游响应体超出 spool 上限，无法回放（重试无意义）",
            )
                .into_response(),
            ForwardResult::SpoolFailed(e) => (
                StatusCode::BAD_GATEWAY,
                format!("本地磁盘缓存写入失败，无法回放: {e}"),
            )
                .into_response(),
            ForwardResult::Response {
                status,
                headers: resp_headers,
                raw_headers,
                body,
                ..
            } => {
                let is_streaming = match &body {
                    // 内存模式：整体判定（含 body 嗅探，与旧行为一致）
                    SpooledBody::Memory(_) => {
                        retry::is_streaming_response(&raw_headers, body.memory_bytes())
                    }
                    // 磁盘模式：content-type 判定（全量嗅探需读回整个文件，
                    // 而磁盘回放本就是 chunked 流式）
                    SpooledBody::Disk { .. } => retry::is_streaming_content_type(&raw_headers),
                };
                tracing::info!(attempt = 1, status = %status, is_streaming, "首轮成功");
                build_replay_response(status, resp_headers, &raw_headers, body, is_streaming).await
            }
            ForwardResult::NetworkError(_) => unreachable!("NetworkError 必定 needs_retry"),
        };
    }

    // 需要重试：按 keepalive 条件选择通道（req_body 所有权移交，随通道结束
    // 自动 Drop 删除磁盘临时文件）
    if keepalive_enabled && client_wants_sse {
        return proxy_with_keepalive(
            state,
            method,
            target_url,
            headers,
            req_body,
            max_spool_bytes,
        )
        .await;
    }
    proxy_without_keepalive(
        state,
        method,
        target_url,
        headers,
        req_body,
        max_spool_bytes,
    )
    .await
}

/// 统一的重试判定入口：内存模式走整体判定（旧行为），磁盘模式走增量扫描
/// 结论 + 头部快照预览。三处（首轮/两个重试通道）共用，保证判定语义一致。
fn needs_retry_response(
    attempt: u32,
    status: &StatusCode,
    raw_headers: &reqwest::header::HeaderMap,
    body: &SpooledBody,
    disk_scan: &Option<(bool, Vec<u8>)>,
) -> bool {
    match (body, disk_scan) {
        (SpooledBody::Memory(b), _) => should_retry_response(attempt, status, raw_headers, b),
        (SpooledBody::Disk { .. }, Some((error, head))) => {
            should_retry_response_disk(attempt, status, raw_headers, *error, head)
        }
        // 不可能：磁盘模式必有增量扫描结果。防御性回退到「不重试」并留痕，
        // 而非 panic——代理服务不能因内部不变量被破坏而失能
        (SpooledBody::Disk { .. }, None) => {
            tracing::error!("内部不变量破坏：磁盘 spool 缺少扫描结果，按成功处理");
            false
        }
    }
}

/// 判定一次上游响应是否需要重试，并输出对应的诊断日志。
///
/// 判定顺序：可重试状态码（4xx/5xx）→ 内容错误（流式扫描 data 行，或非流式错误 JSON）。
/// 首轮与两个重试通道共用，保证三处判定语义一致。
fn should_retry_response(
    attempt: u32,
    status: &StatusCode,
    raw_headers: &reqwest::header::HeaderMap,
    body: &[u8],
) -> bool {
    let is_streaming = retry::is_streaming_response(raw_headers, body);

    // 1. HTTP 状态码可重试：无论是否流式，都重试
    if retry::is_retryable_status(status.as_u16()) {
        tracing::warn!(attempt, status = %status, is_streaming, "上游返回可重试状态码，重试");
        tracing::warn!(preview = %preview_body(body, 500), "错误响应预览");
        return true;
    }

    // 2. 内容错误检测：对所有错误都重试——原型期 4xx 亦视为易发生的瞬时错误。
    //    即使状态码未命中可重试（如 200 携带 error JSON），只要 body 语义为报错就重试。
    let is_error = if is_streaming {
        retry::is_stream_error_body(body)
    } else {
        retry::is_error_body(body)
    };
    if is_error {
        tracing::warn!(attempt, is_streaming, preview = %preview_body(body, 1000), "上游返回错误内容，重试");
        return true;
    }

    false
}

/// 磁盘模式的重试判定：状态码/流式判定用响应头；内容错误用收集期间的增量
/// 扫描结论；预览用头部快照（磁盘全量不可得，头部 1 KiB 足够排障）。
fn should_retry_response_disk(
    attempt: u32,
    status: &StatusCode,
    raw_headers: &reqwest::header::HeaderMap,
    scan_error: bool,
    head_snapshot: &[u8],
) -> bool {
    let is_streaming = retry::is_streaming_content_type(raw_headers);

    // 1. HTTP 状态码可重试：无论是否流式，都重试
    if retry::is_retryable_status(status.as_u16()) {
        tracing::warn!(attempt, status = %status, is_streaming, "上游返回可重试状态码，重试");
        tracing::warn!(preview = %preview_body(head_snapshot, 500), "错误响应预览");
        return true;
    }

    // 2. 内容错误：增量扫描结论（SSE data 行 / NDJSON 行级判定）
    if scan_error {
        tracing::warn!(attempt, is_streaming, preview = %preview_body(head_snapshot, 1000), "上游返回错误内容，重试");
        return true;
    }

    false
}

/// 错误响应体的日志预览。上游可能返回压缩/二进制错误体（如 zstd/gzip 压缩的
/// 错误页——reqwest 未开自动解压以保真透传），直接 from_utf8_lossy 会把控制
/// 字节渲染成整片乱码污染日志。判定：替换符/控制字符占比超阈值视为二进制，
/// 改为 hex 摘要（可直接识别压缩 magic：zstd 28 b5 2f fd、gzip 1f 8b 等）。
fn preview_body(body: &[u8], limit: usize) -> String {
    let head = &body[..body.len().min(limit)];
    // lossy 渲染后统计非文本占比：U+FFFD 与 C0 控制字符
    let text = String::from_utf8_lossy(head);
    let total = text.chars().count().max(1);
    let weird = text
        .chars()
        .filter(|&c| c == '\u{FFFD}' || (c.is_control() && c != '\t' && c != '\n' && c != '\r'))
        .count();
    if weird * 10 >= total {
        // 二进制/压缩内容：hex 前 48 字节足够识别压缩 magic 与排障
        let hex: String = head.iter().take(48).map(|b| format!("{b:02x} ")).collect();
        format!(
            "（二进制/压缩内容，共 {} 字节，hex 前 48: {}…）",
            body.len(),
            hex.trim_end()
        )
    } else {
        let s = text.trim_end();
        if body.len() > limit {
            format!("{s}…（截断，共 {} 字节）", body.len())
        } else {
            s.to_string()
        }
    }
}

/// 非 SSE 客户端的重试通道：从 attempt 2 起无限重试（attempt 1 已在 proxy_handler 完成），
/// 响应在成功后一次性返回。
async fn proxy_without_keepalive(
    state: AppState,
    method: http::Method,
    target_url: String,
    headers: HeaderMap,
    req_body: RequestBody,
    max_spool_bytes: usize,
) -> Response {
    let mut attempt: u32 = 1;
    let max_backoff = state.config.max_retry_backoff_secs;
    loop {
        attempt += 1;
        state
            .stats
            .retries_total
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let delay = retry::delay_for_attempt(attempt - 1, max_backoff);
        if !delay.is_zero() {
            tracing::warn!(attempt, delay_ms = delay.as_millis() as u64, "重试延迟");
            tokio::time::sleep(delay).await;
        } else {
            tracing::warn!(attempt, "立即重试");
        }

        let result = forward_once(
            &state.client,
            &method,
            &target_url,
            &headers,
            &req_body,
            max_spool_bytes,
            state.spool_dir.as_deref(),
        )
        .await;

        match result {
            ForwardResult::NetworkError(e) => {
                state.note_upstream_failure(&format!("网络错误: {e}"));
                tracing::warn!(attempt, error = %e, "上游网络错误，重试");
                continue;
            }
            ForwardResult::TooLarge => {
                state.note_upstream_failure("上游响应体超出 spool 上限");
                tracing::error!(attempt, "上游响应体超出 spool 上限，终止重试");
                return (
                    StatusCode::BAD_GATEWAY,
                    "上游响应体超出 spool 上限，无法回放（重试无意义）",
                )
                    .into_response();
            }
            ForwardResult::SpoolFailed(e) => {
                state.note_upstream_failure(&format!("本地 spool 故障: {e}"));
                tracing::error!(attempt, error = %e, "本地 spool 故障，终止重试");
                return (
                    StatusCode::BAD_GATEWAY,
                    format!("本地磁盘缓存写入失败: {e}"),
                )
                    .into_response();
            }
            ForwardResult::Response {
                status,
                headers: resp_headers,
                raw_headers,
                body,
                disk_scan,
            } => {
                if needs_retry_response(attempt, &status, &raw_headers, &body, &disk_scan) {
                    state.note_upstream_failure(&format!("上游返回 {status}（错误内容，重试）"));
                    continue; // body Drop：磁盘临时文件删除
                }

                // 成功：按是否流式选择回放方式，保证“原样流式”
                let is_streaming = match &body {
                    SpooledBody::Memory(_) => {
                        retry::is_streaming_response(&raw_headers, body.memory_bytes())
                    }
                    SpooledBody::Disk { .. } => retry::is_streaming_content_type(&raw_headers),
                };
                tracing::info!(attempt, status = %status, is_streaming, "重试后成功");
                return build_replay_response(
                    status,
                    resp_headers,
                    &raw_headers,
                    body,
                    is_streaming,
                )
                .await;
            }
        }
    }
}

/// 带保活的重试通道：仅在上游首轮已失败后进入。立即以 SSE 流响应并在流中发送
/// `: keepalive\n\n` 注释，后台从 attempt 2 起无限重试，成功后将上游 body 分块转发。
///
/// 注意：此通道的骨架响应已先行发出（200 + text/event-stream），上游真实 status
/// 与响应头无法再回放——这是「先保活、后成功」的固有取舍；首轮成功走的是
/// proxy_handler 的保真快速路径，不受影响。
async fn proxy_with_keepalive(
    state: AppState,
    method: http::Method,
    target_url: String,
    headers: HeaderMap,
    req_body: RequestBody,
    max_spool_bytes: usize,
) -> Response {
    let keepalive_dur = state.config.keepalive_interval();
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(32);

    // 客户端断开信号：watch(false→true)。哨兵被 move 进响应 Body 的流闭包，
    // hyper 因客户端断开而 drop Body 时闭包随之销毁，哨兵 Drop 中置位；
    // 后台任务据此立即中止 in-flight 的上游请求（避免断开后上游继续生成白自计费）。
    let (gone_tx, mut gone_rx) = tokio::sync::watch::channel(false);
    let gone_guard = ClientGoneGuard { tx: gone_tx };

    // 心跳为 SSE 注释（": keepalive\n\n"），合法 SSE 客户端按规范忽略
    let heartbeat = || Bytes::from_static(b": keepalive\n\n");

    // 后台任务：无限重试上游，期间按 keepalive_dur 发送 SSE 注释；成功后将上游响应分块转发。
    // 所有 send 都检查客户端是否已断开（channel 关闭 → 立即退出，不空转）。
    let state_bg = state.clone();
    tokio::spawn(async move {
        // 骨架发出后立即发首个心跳：客户端 idle 计时从收到字节起算
        if tx.send(Ok(heartbeat())).await.is_err() {
            return;
        }

        let mut attempt: u32 = 1;
        let max_backoff = state.config.max_retry_backoff_secs;
        loop {
            attempt += 1;
            state_bg
                .stats
                .retries_total
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let delay = retry::delay_for_attempt(attempt - 1, max_backoff);
            if !delay.is_zero() {
                // 在延迟期间按 keepalive_dur 切片发送心跳，避免客户端 idle 超时
                let mut elapsed = Duration::ZERO;
                while elapsed < delay {
                    let slice = std::cmp::min(keepalive_dur, delay - elapsed);
                    tokio::time::sleep(slice).await;
                    elapsed += slice;
                    if elapsed < delay {
                        // 仅在尚未到下一轮重试时发送心跳
                        if tx.send(Ok(heartbeat())).await.is_err() {
                            return;
                        }
                    }
                }
            } else {
                tracing::warn!(attempt, "立即重试（保活通道）");
            }

            // in-flight 期间与客户端断开信号竞速：断开即丢弃 forward_once future，
            // reqwest 连接随之关闭，上游（如 LLM API）会因连接断开停止生成——
            // 这是「客户端断开后本条请求立即断开」的关键点，防止计费浪费。
            let client_gone = async {
                // 信号置位或哨兵随 Body 提前销毁（channel 关闭）都视为断开
                let _ = gone_rx.wait_for(|v| *v).await;
            };
            let result = tokio::select! {
                r = forward_once(
                    &state_bg.client,
                    &method,
                    &target_url,
                    &headers,
                    &req_body,
                    max_spool_bytes,
                    state_bg.spool_dir.as_deref(),
                ) => r,
                _ = client_gone => {
                    tracing::info!("客户端已断开，中止 in-flight 上游请求（保活通道）");
                    return;
                }
            };

            match result {
                ForwardResult::NetworkError(e) => {
                    state_bg.note_upstream_failure(&format!("网络错误: {e}"));
                    tracing::warn!(attempt, error = %e, "上游网络错误，重试（保活通道）");
                    if tx.send(Ok(heartbeat())).await.is_err() {
                        return;
                    }
                    continue;
                }
                ForwardResult::TooLarge => {
                    state_bg.note_upstream_failure("上游响应体超出 spool 上限");
                    tracing::error!(attempt, "上游响应体超出 spool 上限，终止重试（保活通道）");
                    // 骨架 200 已发出、状态行不可再改：静默结束流与「上游成功返回
                    // 空 body」在客户端视角不可区分。发一个终态错误事件让客户端
                    // 明确感知代理放弃了这条请求。
                    let err_event = Bytes::from_static(
                        b"event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"proxy_spool_limit\",\"message\":\"upstream response exceeded proxy spool limit\"}}\n\n",
                    );
                    let _ = tx.send(Ok(err_event)).await;
                    return;
                }
                ForwardResult::SpoolFailed(e) => {
                    state_bg.note_upstream_failure(&format!("本地 spool 故障: {e}"));
                    tracing::error!(attempt, error = %e, "本地 spool 故障，终止重试（保活通道）");
                    let err_event = Bytes::from_static(
                        b"event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"proxy_spool_failed\",\"message\":\"local disk cache write failed\"}}\n\n",
                    );
                    let _ = tx.send(Ok(err_event)).await;
                    return;
                }
                ForwardResult::Response {
                    status,
                    raw_headers,
                    mut body,
                    disk_scan,
                    ..
                } => {
                    if needs_retry_response(attempt, &status, &raw_headers, &body, &disk_scan) {
                        state_bg
                            .note_upstream_failure(&format!("上游返回 {status}（错误内容，重试）"));
                        // 发心跳前先丢弃（body Drop 删临时文件，杜绝任何
                        // return 路径上的泄漏）
                        drop(body);
                        if tx.send(Ok(heartbeat())).await.is_err() {
                            return;
                        }
                        continue;
                    }

                    let is_streaming = match &body {
                        SpooledBody::Memory(_) => {
                            retry::is_streaming_response(&raw_headers, body.memory_bytes())
                        }
                        SpooledBody::Disk { .. } => retry::is_streaming_content_type(&raw_headers),
                    };
                    tracing::info!(attempt, status = %status, is_streaming, "重试后成功（保活通道）");

                    // 成功：将完整 body 按块转发；若为 SSE，保持 SSE 语义（心跳为注释，不影响解析）。
                    // 空 body 直接结束流，不注入任何上游未发送的字节。
                    if body.is_empty() {
                        return;
                    }
                    let mut stream = body.into_stream().await;
                    while let Some(item) = stream.next().await {
                        if tx.send(item).await.is_err() {
                            return; // 客户端断开：Drop 链负责删临时文件
                        }
                    }
                    return;
                }
            }
        }
    });

    // SSE 流式骨架；仅在重试间隙注入 ": keepalive\n\n"（SSE 注释，客户端会忽略）。
    // 哨兵 move 进 map 闭包：Body 被客户端断开而 drop 时闭包销毁 → 置位断开信号。
    let rx_stream = tokio_stream::wrappers::ReceiverStream::new(rx).map(move |r| {
        let _ = &gone_guard;
        r.map_err(|e| std::io::Error::other(e.to_string()))
    });
    let body = Body::from_stream(rx_stream);

    let mut resp = Response::builder().status(StatusCode::OK);
    resp = resp.header(
        http::header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    resp = resp.header(
        http::header::CACHE_CONTROL,
        HeaderValue::from_static("no-cache"),
    );
    // 不设置 connection 头：hyper 按协议自动管理
    resp.body(body).unwrap().into_response()
}

/// 客户端断开哨兵：持有 watch 发送端，Drop 时置位断开信号。
/// 被 move 进 keepalive 响应 Body 的流闭包，Body 随客户端断开被 hyper drop 时触发。
struct ClientGoneGuard {
    tx: tokio::sync::watch::Sender<bool>,
}

impl Drop for ClientGoneGuard {
    fn drop(&mut self) {
        let _ = self.tx.send(true);
    }
}

// Response 变体比其他变体大（status + 双头表 + SpooledBody）：body 本身已是
// 引用计数 Bytes 或文件句柄，装箱只能省这点栈空间，而本枚举只在单一路径按值
// 传递一次，不构成热点。保持扁平匹配更直接。
#[allow(clippy::large_enum_variant)]
enum ForwardResult {
    NetworkError(String),
    /// 上游响应体超出 spool 上限：确定性失败，重试无意义，各通道直接终态处理。
    TooLarge,
    /// 本地磁盘 spool 写入失败（磁盘满/IO 故障）：磁盘模式专属的确定性失败。
    /// 与 NetworkError 不同——同一轮重试无法恢复（文件句柄已坏），但下一轮
    /// 可能恢复；交由调用方选择终态或继续。
    SpoolFailed(String),
    /// 完整缓冲后的响应；`raw_headers` 保留原始 reqwest 头用于流式嗅探，
    /// `headers` 为已过滤 hop-by-hop 后的待透传头。`disk_scan` 仅为磁盘模式
    /// 增量扫描结论（Some((是否错误, 头部快照)))；内存模式恒 None。
    Response {
        status: StatusCode,
        headers: HeaderMap,
        raw_headers: reqwest::header::HeaderMap,
        body: SpooledBody,
        disk_scan: Option<(bool, Vec<u8>)>,
    },
}

/// 响应体 spool 上限（字节）：配置 spool_limit_mb（MB，最小 1）换算，
/// 防止失控/恶意上游把内存打爆。超过上限的响应无法完整缓冲，直接按
/// TooLarge 终态处理。
fn spool_limit_bytes(config: &Config) -> usize {
    (config.spool_limit_mb.max(1) as usize).saturating_mul(1024 * 1024)
}

async fn forward_once(
    client: &reqwest::Client,
    method: &http::Method,
    url: &str,
    headers: &HeaderMap,
    req_body: &RequestBody,
    max_spool_bytes: usize,
    spool_dir: Option<&Path>,
) -> ForwardResult {
    let reqwest_method =
        reqwest::Method::from_bytes(method.as_str().as_bytes()).unwrap_or(reqwest::Method::GET);

    let mut builder = client.request(reqwest_method, url);

    // 透传请求头（过滤 hop-by-hop），已在 proxy_handler 中应用了覆盖/追加
    for (name, value) in headers.iter() {
        let name_str = name.as_str();
        if is_hop_header(name_str) {
            continue;
        }
        if let Ok(n) = reqwest::header::HeaderName::from_bytes(name_str.as_bytes())
            && let Ok(v) = reqwest::header::HeaderValue::from_bytes(value.as_bytes())
        {
            builder = builder.header(n, v);
        }
    }

    // 请求体字节源：内存零拷贝 clone / 磁盘流式重读。磁盘模式显式回填
    // content-length（hop 过滤剥掉了原头，而我们持有精确字节数，保真回放）。
    builder = match req_body.apply_to(builder).await {
        Ok(b) => b,
        Err(e) => {
            // 请求体临时文件打不开 = 本地确定性故障，重试同样打不开
            return ForwardResult::SpoolFailed(e.to_string());
        }
    };

    let resp = match builder.send().await {
        Ok(r) => r,
        Err(e) => return ForwardResult::NetworkError(e.to_string()),
    };

    let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let raw_headers = resp.headers().clone();

    // 过滤后的响应头（透传用）
    let mut resp_headers = HeaderMap::new();
    for (name, value) in resp.headers().iter() {
        let name_str = name.as_str();
        if is_hop_header(name_str) {
            continue;
        }
        if let Ok(n) = HeaderName::from_bytes(name_str.as_bytes())
            && let Ok(v) = HeaderValue::from_bytes(value.as_bytes())
        {
            // append 而非 insert：Set-Cookie 等同名多值头不能坍缩为最后一个
            resp_headers.append(n, v);
        }
    }

    // 关键：完整 spool（分块累积）——任何流式中断都会在此处以 Err 形式暴露，从而触发重试；
    // 超出上限按 TooLarge 终态处理；磁盘写入失败按 SpoolFailed 处理
    let mut spool = SpoolBuffer::Memory(Vec::new());
    let mut scanner = StreamErrorScanner::new();
    let mut resp_stream = resp;
    loop {
        match resp_stream.chunk().await {
            Ok(Some(chunk)) => {
                if spool.len() + chunk.len() as u64 > max_spool_bytes as u64 {
                    spool.discard();
                    return ForwardResult::TooLarge;
                }
                scanner.feed(&chunk);
                spool_chunk(&mut spool, &chunk, spool_dir).await;
                if matches!(spool, SpoolBuffer::Poisoned) {
                    spool.discard();
                    return ForwardResult::SpoolFailed("spool 磁盘写入失败".to_string());
                }
            }
            Ok(None) => break,
            Err(e) => {
                spool.discard();
                return ForwardResult::NetworkError(format!("读取上游响应体失败: {e}"));
            }
        }
    }

    let (body, disk_scan) = spool.finish(scanner).await;
    ForwardResult::Response {
        status,
        headers: resp_headers,
        raw_headers,
        body,
        disk_scan,
    }
}

fn build_response(status: StatusCode, headers: HeaderMap, body: Bytes) -> Response {
    let mut resp = Response::builder().status(status);
    for (name, value) in headers.iter() {
        resp = resp.header(name, value);
    }
    resp.body(Body::from(body)).unwrap().into_response()
}

/// 统一回放入口：内存模式按流式判定选路（旧行为，保真优先）；磁盘模式一律
/// chunked 流式回放（回放即流式读临时文件），上游带 content-length 时回填
/// 保真（hop 过滤剥掉了它，而我们持有精确字节数）。
async fn build_replay_response(
    status: StatusCode,
    headers: HeaderMap,
    raw_headers: &reqwest::header::HeaderMap,
    mut body: SpooledBody,
    is_streaming: bool,
) -> Response {
    // 磁盘模式一律 chunked 流式回放（回放即流式读临时文件）；上游带
    // content-length 时回填保真（hop 过滤剥掉了它，我们持有精确字节数）
    if matches!(body, SpooledBody::Disk { .. }) {
        let replay_len = body.len();
        let mut resp = Response::builder().status(status);
        for (name, value) in headers.iter() {
            resp = resp.header(name, value);
        }
        if replay_len > 0 && raw_headers.get(http::header::CONTENT_LENGTH).is_some() {
            resp = resp.header(http::header::CONTENT_LENGTH, replay_len.to_string());
        }
        let stream = body.into_stream().await;
        resp.body(Body::from_stream(stream))
            .unwrap()
            .into_response()
    } else if is_streaming {
        // 内存流式：8 KiB 零拷贝分块 chunked 回放（与旧行为一致）
        let b = body.take_memory();
        build_stream_replay_response(status, headers, b)
    } else {
        // 内存非流式：Body::from 一次性回放（content-length 由 hyper 自动
        // 设置，不走 chunked——保真优先）
        let b = body.take_memory();
        build_response(status, headers, b)
    }
}

/// 流式回放：将已完整 spool 的 body 按块以 chunked 流式产出，字节与上游完全一致。
///
/// 逐块大小 8 KiB，对 SSE 而言任意切分均安全（解析器按行缓冲），且保证
/// `Transfer-Encoding: chunked`，客户端仍以流式增量消费，避免因一次性
/// `Body::from(bytes)` 被某些客户端误判为非流式而产生的潜在 bug。
fn build_stream_replay_response(status: StatusCode, headers: HeaderMap, body: Bytes) -> Response {
    let mut resp = Response::builder().status(status);
    for (name, value) in headers.iter() {
        resp = resp.header(name, value);
    }

    if body.is_empty() {
        return resp.body(Body::empty()).unwrap().into_response();
    }

    // 将 Bytes 按 8 KiB 零拷贝切块：slice_ref 是引用计数切片，共享同一底层
    // 分配——峰值内存为响应体一份而非深拷贝的两份。代价是任一未消费的块
    // 都会把整份分配钉到流结束/断开；body 本就因重试保障全量驻留 spool，
    // 此处无额外驻留
    const CHUNK_SIZE: usize = 8 * 1024;
    let chunks: Vec<Bytes> = body.chunks(CHUNK_SIZE).map(|c| body.slice_ref(c)).collect();

    let stream = futures_util::stream::iter(chunks.into_iter().map(Ok::<Bytes, std::io::Error>));
    let body = Body::from_stream(stream);
    resp.body(body).unwrap().into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    // ---- 流式回放零拷贝：切块必须共享原 body 的底层分配（slice_ref 引用
    // 计数切片），而非 copy_from_slice 深拷贝——深拷贝会让峰值内存瞬时
    // 翻倍（50MB SSE 实测曾达 64MB）。指针区间断言：任何拷贝都会落到
    // 原分配之外的新地址 ----

    #[tokio::test]
    async fn replay_chunks_are_zero_copy_views_of_source() {
        // 20 KiB → 3 块（8K + 8K + 4K），含非整除边界
        let body = Bytes::from(vec![0xA5u8; 20 * 1024]);
        let base = body.as_ptr() as usize;
        let end = base + body.len();

        let resp = build_stream_replay_response(StatusCode::OK, HeaderMap::new(), body.clone());
        // 逐帧轮询响应体：axum 的 Body::from_stream 原样透传我们产出的 Bytes
        //（不再复制），故帧数据指针可直接检验
        let mut streamed = resp.into_body();
        let mut frames = Vec::new();
        while let Some(frame) = http_body_util::BodyExt::frame(&mut streamed)
            .await
            .transpose()
            .unwrap()
        {
            if let Some(data) = frame.data_ref() {
                let p = data.as_ptr() as usize;
                assert!(
                    (base..end).contains(&p),
                    "回放块指针 {p:#x} 必须落在原 body 分配 [{base:#x}, {end:#x}) 内（零拷贝）"
                );
                frames.push(data.to_vec());
            }
        }
        assert_eq!(frames.len(), 3, "20 KiB 应切为 3 块");
        // 各块按序还原 == 原 body（切块只分不变）
        let reassembled: Vec<u8> = frames.into_iter().flatten().collect();
        assert_eq!(reassembled, body.as_ref());
    }

    // ---- preview_body：二进制/压缩体不污染日志（回归：zstd 错误体曾被
    // from_utf8_lossy 渲染成整片乱码）----

    #[test]
    fn preview_of_binary_shows_hex_not_mojibake() {
        // zstd magic 开头的压缩体（真实日志中出现过）
        let mut zstd_like = vec![0x28, 0xb5, 0x2f, 0xfd];
        zstd_like.extend((0..200u8).map(|i| i.wrapping_mul(37)));
        let p = preview_body(&zstd_like, 500);
        assert!(p.contains("二进制/压缩内容"), "{p}");
        assert!(p.contains("28 b5 2f fd"), "hex 应含 zstd magic: {p}");
        assert!(!p.contains('\u{FFFD}'), "不应输出替换符: {p}");
    }

    #[test]
    fn preview_of_text_is_plain_and_truncated() {
        let text = "上游过载：请稍后重试".repeat(50);
        let p = preview_body(text.as_bytes(), 100);
        assert!(p.contains("上游过载"), "{p}");
        assert!(p.contains("截断"), "{p}");
        // 正常 JSON 错误体
        let json = br#"{"type":"error","error":{"type":"overloaded"}}"#;
        assert_eq!(preview_body(json, 500), String::from_utf8_lossy(json));
    }

    // ---- is_hop_header：hop-by-hop 头识别（大小写不敏感）----

    #[test]
    fn hop_headers_are_all_recognized() {
        for name in [
            "connection",
            "keep-alive",
            "proxy-authenticate",
            "proxy-authorization",
            "te",
            "trailer",
            "transfer-encoding",
            "upgrade",
            "host",
            "content-length",
        ] {
            assert!(is_hop_header(name), "{name} 应为 hop 头");
        }
    }

    #[test]
    fn non_hop_headers_are_not_filtered() {
        for name in ["x-custom", "authorization", "content-type", "accept"] {
            assert!(!is_hop_header(name), "{name} 不应被当作 hop 头");
        }
    }

    #[test]
    fn hop_matching_is_case_insensitive() {
        for name in [
            "Connection",
            "Keep-Alive",
            "HOST",
            "Content-Length",
            "Transfer-Encoding",
            "Proxy-Authorization",
            "TE",
            "Upgrade",
        ] {
            assert!(is_hop_header(name), "大小写不应影响 {name} 的 hop 判定");
        }
    }

    // ---- apply_header_overrides：api_key / override_headers / extra_headers 三层头策略 ----

    #[test]
    fn api_key_sets_bearer_authorization() {
        let cfg = Config {
            base_url: "https://api.example.com".to_string(),
            api_key: Some("sk-test".to_string()),
            ..Default::default()
        };
        let mut headers = HeaderMap::new();
        apply_header_overrides(&mut headers, &cfg);
        assert_eq!(headers.get("authorization").unwrap(), "Bearer sk-test");
    }

    #[test]
    fn api_key_overwrites_existing_authorization() {
        let cfg = Config {
            base_url: "https://api.example.com".to_string(),
            api_key: Some("sk-test".to_string()),
            ..Default::default()
        };
        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            HeaderValue::from_static("Basic dXNlcjpwYXNz"),
        );
        apply_header_overrides(&mut headers, &cfg);
        assert_eq!(headers.get("authorization").unwrap(), "Bearer sk-test");
    }

    #[test]
    fn missing_api_key_adds_no_authorization() {
        let cfg = Config {
            base_url: "https://api.example.com".to_string(),
            ..Default::default()
        };
        let mut headers = HeaderMap::new();
        headers.insert("x-custom", HeaderValue::from_static("keep"));
        apply_header_overrides(&mut headers, &cfg);
        assert!(
            headers.get("authorization").is_none(),
            "未配置 api_key 不应新增 Authorization"
        );
        assert_eq!(headers.get("x-custom").unwrap(), "keep", "其余头应保持不变");
    }

    #[test]
    fn override_headers_unconditionally_replace_existing() {
        // 配置键 "X-Foo" 应覆盖已有 "x-foo"（大小写不敏感），且同名多值全部收敛为单个新值
        let cfg = Config {
            base_url: "https://api.example.com".to_string(),
            override_headers: HashMap::from([("X-Foo".to_string(), "new".to_string())]),
            ..Default::default()
        };
        let mut headers = HeaderMap::new();
        headers.append("x-foo", HeaderValue::from_static("old1"));
        headers.append("x-foo", HeaderValue::from_static("old2"));
        apply_header_overrides(&mut headers, &cfg);
        assert_eq!(
            headers.get_all("x-foo").iter().count(),
            1,
            "覆盖后应只剩一个值"
        );
        assert_eq!(headers.get("x-foo").unwrap(), "new");
    }

    #[test]
    fn override_headers_add_missing_headers() {
        let cfg = Config {
            base_url: "https://api.example.com".to_string(),
            override_headers: HashMap::from([("x-added".to_string(), "v".to_string())]),
            ..Default::default()
        };
        let mut headers = HeaderMap::new();
        apply_header_overrides(&mut headers, &cfg);
        assert_eq!(
            headers.get("x-added").unwrap(),
            "v",
            "override 对缺失头应直接新增"
        );
    }

    #[test]
    fn extra_headers_only_fill_missing() {
        let cfg = Config {
            base_url: "https://api.example.com".to_string(),
            extra_headers: HashMap::from([
                ("x-extra".to_string(), "added".to_string()),
                ("x-present".to_string(), "ignored".to_string()),
            ]),
            ..Default::default()
        };
        let mut headers = HeaderMap::new();
        // 大小写不同的同名头也应保留原值，不被 extra_headers 覆盖
        headers.insert("X-PRESENT", HeaderValue::from_static("keep"));
        apply_header_overrides(&mut headers, &cfg);
        assert_eq!(headers.get("x-extra").unwrap(), "added", "缺失头应被追加");
        assert_eq!(
            headers.get("x-present").unwrap(),
            "keep",
            "已存在头不应被覆盖"
        );
    }

    #[test]
    fn override_headers_beat_api_key_for_authorization() {
        // 优先级：api_key 先应用、override_headers 后应用 → authorization 最终取 override 值
        let cfg = Config {
            base_url: "https://api.example.com".to_string(),
            api_key: Some("sk-api".to_string()),
            override_headers: HashMap::from([(
                "Authorization".to_string(),
                "Bearer sk-override".to_string(),
            )]),
            ..Default::default()
        };
        let mut headers = HeaderMap::new();
        apply_header_overrides(&mut headers, &cfg);
        assert_eq!(headers.get("authorization").unwrap(), "Bearer sk-override");
    }

    #[test]
    fn extra_headers_do_not_overwrite_existing_authorization() {
        // extra_headers 优先级最低（只补缺失）：api_key 已写入 authorization 时不再覆盖
        let cfg = Config {
            base_url: "https://api.example.com".to_string(),
            api_key: Some("sk-api".to_string()),
            extra_headers: HashMap::from([(
                "Authorization".to_string(),
                "should-not-win".to_string(),
            )]),
            ..Default::default()
        };
        let mut headers = HeaderMap::new();
        apply_header_overrides(&mut headers, &cfg);
        assert_eq!(headers.get("authorization").unwrap(), "Bearer sk-api");
    }

    #[test]
    fn invalid_names_and_values_are_silently_skipped() {
        // 非法头名（含空格/控制字符）与非法头值（含控制字符）应被静默跳过，不 panic
        let cfg = Config {
            base_url: "https://api.example.com".to_string(),
            api_key: Some("bad\nkey".to_string()), // 含换行控制符 → HeaderValue 非法，api_key 应被跳过
            override_headers: HashMap::from([
                ("bad name".to_string(), "v".to_string()), // 含空格 → HeaderName 非法
                ("x-good".to_string(), "ok".to_string()),
            ]),
            extra_headers: HashMap::from([("x-bad-val".to_string(), "bad\rval".to_string())]),
            ..Default::default()
        };
        let mut headers = HeaderMap::new();
        apply_header_overrides(&mut headers, &cfg);
        assert!(
            headers.get("authorization").is_none(),
            "非法 api_key 不应写入 Authorization"
        );
        assert!(headers.get("bad name").is_none(), "非法头名应跳过");
        assert_eq!(
            headers.get("x-good").unwrap(),
            "ok",
            "合法 override 应正常生效"
        );
        assert!(headers.get("x-bad-val").is_none(), "非法头值应跳过");
    }
}
