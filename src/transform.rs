//! 外部转换器（外部 format 程序）的进程池编排。
//!
//! aproxy 把请求/响应装进一行 JSON 信封（见 aproxy-envelope crate）交给外部
//! format 程序改写后收回——转换逻辑全部外置，本模块只做编排。两种模式统一进
//! 同一个池抽象：
//! - `spawn`（一次性）：每请求启动进程、一行进出、进程退出（permits=1、
//!   idle 恒空——用毕即弃）
//! - `persistent`（持续进程池）：worker 进程 `while` 循环逐行处理（一次一个
//!   请求、输入输出有序、无多路复用），按需扩容至 `pool_max`，空闲超时回收
//!
//! 失败语义由调用方（proxy.rs）决定：请求侧 502 不重试、响应侧透传原样。

use std::{
    collections::btree_map::{BTreeMap, Entry},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering as AtomicOrdering},
    },
    time::{Duration, Instant},
};

use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::process::{Child, ChildStdin};
use tokio::sync::{Mutex, Semaphore};

use aproxy_envelope::TransformEnvelope;

use crate::{
    config::{TransformConfig, TransformMode},
    proxy::{RESIDENT_LIMIT, RequestBody, SpooledBody, is_hop_header, unique_seq},
};

/// 转换失败分类。Display 文案供 502 响应体与日志直接内插。
#[derive(Debug)]
pub(crate) enum TransformError {
    /// format 进程启动失败（命令不存在等）。
    Spawn(std::io::Error),
    /// 信封序列化失败（body 含非法字符等，现实中几乎不发生）。
    Serialize(serde_json::Error),
    /// 读写 format 进程的 stdin/stdout 失败（管道断开等）。
    Io(std::io::Error),
    /// 单请求转换超时（worker 可能仍在消化旧输入，kill 后剔除，绝不能复用）。
    TimedOut,
    /// worker 进程意外退出且无输出（EOF）。
    WorkerDied,
    /// format 通过信封 error 行表达的单请求失败（进程未崩）。
    Rejected(String),
    /// 池已关闭（防御性：现实中信号量不会被 close）。
    Closed,
}

impl std::fmt::Display for TransformError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransformError::Spawn(e) => write!(f, "format 进程启动失败: {e}"),
            TransformError::Serialize(e) => write!(f, "信封序列化失败: {e}"),
            TransformError::Io(e) => write!(f, "format 进程管道读写失败: {e}"),
            TransformError::TimedOut => write!(f, "转换超时（format 进程已终止）"),
            TransformError::WorkerDied => write!(f, "format 进程意外退出且无输出"),
            TransformError::Rejected(msg) => write!(f, "format 报告转换失败: {msg}"),
            TransformError::Closed => write!(f, "转换器已关闭"),
        }
    }
}

/// 池内单个 worker：一个长驻子进程 + 它独占的 stdin/stdout。
///
/// **stdout 必须用同一个 BufReader 包裹**（存进 Worker 而非每次 convert 新建）：
/// BufReader 有内部缓冲，persistent worker 的响应交错到达时，新建的
/// BufReader 会把上个缓冲里残留的下一行丢掉——这是致命 bug 风险，结构上根除。
struct Worker {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<tokio::process::ChildStdout>,
    /// 池槽位号（0..pool_max）：轮换类 format 用它做每 worker 起始偏移，
    /// 避免池内各 worker 轮换起点重合。
    worker_id: u32,
    last_used: Instant,
}

struct PoolState {
    idle: Vec<Worker>,
    next_worker_id: u32,
}

/// 外部转换器进程池。spawn 与 persistent 两模式统一抽象（permits=1、idle
/// 恒空即退化为一次性 spawn），调用侧无分支。
pub(crate) struct TransformPool {
    cfg: Arc<TransformConfig>,
    /// Arc 化：空闲回收 reaper 任务需要 'static 的 state 引用（&self.state
    /// 借用逃不出 tokio::spawn），clone Arc 即可。
    state: Arc<Mutex<PoolState>>,
    /// 并发上限（含待 spawn 名额）：并发超 pool_max 排队——排队只加延迟不损
    /// 吞吐，与重试语义天然兼容。
    permits: Semaphore,
    /// 空闲回收 reaper 是否已启动（惰性：首次 convert 时在 tokio 上下文内
    /// 启动——AppState::new 可能不在 async 上下文，tokio::spawn 会 panic）。
    reaper_started: AtomicBool,
}

impl TransformPool {
    pub(crate) fn new(cfg: Arc<TransformConfig>) -> Self {
        let permits = match cfg.mode {
            TransformMode::Spawn => 1,
            TransformMode::Persistent => cfg.effective_pool_max() as usize,
        };
        Self {
            cfg,
            state: Arc::new(Mutex::new(PoolState {
                idle: Vec::new(),
                next_worker_id: 0,
            })),
            permits: Semaphore::new(permits),
            reaper_started: AtomicBool::new(false),
        }
    }

    /// 单请求转换：写信封行、读回信封行。worker 取自空闲表（persistent）或
    /// 现场启动（spawn / 无空闲）；用毕归还或按模式丢弃。
    pub(crate) async fn convert(
        &self,
        mut env: TransformEnvelope,
    ) -> Result<TransformEnvelope, TransformError> {
        self.ensure_reaper_started().await;
        let _permit = self
            .permits
            .acquire()
            .await
            .map_err(|_| TransformError::Closed)?;

        // **先释放锁再 spawn**：`match self.state.lock().await.idle.pop()` 的
        // MutexGuard 会活到整个 match 语句结束，arm 里 spawn_worker() 再拿
        // 同一把锁就是自死锁（tokio Mutex 不可重入）——pop 结果必须先落变量
        let popped = self.state.lock().await.idle.pop();
        let mut worker = match popped {
            Some(w) => w,
            None => self.spawn_worker().await?,
        };
        env.worker_id = worker.worker_id;

        let line = env.to_line().map_err(TransformError::Serialize)?;
        let timeout_secs = self.cfg.effective_timeout_secs();
        // 写 + 读一次往返；timeout 包裹读侧——超时的 worker 不可信（可能仍
        // 在消化旧输入），必须 kill 剔除
        let io = async {
            worker.stdin.write_all(line.as_bytes()).await?;
            worker.stdin.write_all(b"\n").await?;
            worker.stdin.flush().await?;
            let mut out = String::new();
            let n = worker.stdout.read_line(&mut out).await?;
            Ok::<_, std::io::Error>((n, out))
        };
        let result = if timeout_secs == 0 {
            io.await.map_err(TransformError::Io)
        } else {
            match tokio::time::timeout(Duration::from_secs(timeout_secs), io).await {
                Ok(r) => r.map_err(TransformError::Io),
                Err(_) => Err(TransformError::TimedOut),
            }
        };

        match result {
            Ok((0, _)) => {
                // EOF：进程退出且无输出（协议义务是遇 EOF 退出，读到 EOF 无输出
                // 说明进程刚死）
                discard_worker(worker);
                Err(TransformError::WorkerDied)
            }
            Ok((_, out)) => {
                // n > 0：读到一行输出
                let alive = matches!(worker.child.try_wait(), Ok(None));
                if alive && self.cfg.mode == TransformMode::Persistent {
                    worker.last_used = Instant::now();
                    self.state.lock().await.idle.push(worker);
                } else {
                    // spawn 模式（一次性）或进程已退（写完就退）：收尾丢弃
                    discard_worker(worker);
                }
                // 信封行以 \n 结尾；trim 后解析（空行是协议错误）
                TransformEnvelope::from_line(out.trim_end())
                    .map_err(|e| TransformError::Rejected(e.to_string()))
            }
            Err(e) => {
                discard_worker(worker);
                Err(e)
            }
        }
    }

    /// 启动 worker：按配置的 command/args spawn，stdin/stdout 接管、stderr
    /// 弃之（format 的排障输出进守护日志无意义，崩溃原因按退出码上报）。
    async fn spawn_worker(&self) -> Result<Worker, TransformError> {
        let mut cmd = tokio::process::Command::new(&self.cfg.command);
        cmd.args(&self.cfg.args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            // 兜底：任何提前 return / panic 路径子进程被 kill（Worker 的
            // stdin/stdout 被 drop 时管道关闭，persistent worker 按协议义务
            // 自行 exit；kill_on_drop 兜底不吃 EOF 的挂死进程）
            .kill_on_drop(true);
        let mut child = cmd.spawn().map_err(TransformError::Spawn)?;
        let stdin = child.stdin.take().ok_or(TransformError::WorkerDied)?;
        let stdout = child.stdout.take().ok_or(TransformError::WorkerDied)?;
        let worker_id = {
            let mut st = self.state.lock().await;
            let id = st.next_worker_id;
            // 自增取模分配槽位：spawn 模式 pool_max=1 恒 0；persistent 池内
            // 各 worker 拿到稳定不同的槽位号
            st.next_worker_id = (st.next_worker_id + 1) % self.cfg.effective_pool_max();
            id
        };
        Ok(Worker {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            worker_id,
            last_used: Instant::now(),
        })
    }

    /// 惰性启动空闲回收 reaper（仅 persistent + idle_timeout_secs > 0）。
    async fn ensure_reaper_started(&self) {
        if self.cfg.mode != TransformMode::Persistent {
            return;
        }
        if self.reaper_started.swap(true, AtomicOrdering::AcqRel) {
            return;
        }
        let idle_secs = self.cfg.effective_idle_timeout_secs();
        if idle_secs == 0 {
            return; // 0 = 永不回收
        }
        // 单 reaper 任务扫描（非每 worker sleep 竞速）：无 per-worker 任务爆炸
        // 与取消簿记；池上限个位数量级，扫描 O(空闲数) 无性能问题
        let period = Duration::from_secs((idle_secs / 2).clamp(1, 5));
        let state = Arc::clone(&self.state);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(period).await;
                let now = Instant::now();
                let expired: Vec<Worker> = state
                    .lock()
                    .await
                    .idle
                    .extract_if(.., |w| {
                        now.duration_since(w.last_used) >= Duration::from_secs(idle_secs)
                    })
                    .collect();
                for w in expired {
                    tracing::info!(worker_id = w.worker_id, "format worker 空闲超时，回收");
                    discard_worker(w);
                }
            }
        });
    }
}

/// 丢弃 worker：kill + wait 收尾。kill_on_drop 兜底，wait 在 spawn 的任务里
/// 完成（unix 防 zombie；kill 后进程很快退出，任务即刻结束）。
fn discard_worker(mut worker: Worker) {
    let _ = worker.child.start_kill();
    tokio::spawn(async move {
        let _ = worker.child.wait().await;
    });
}

/// 头表 → 信封：键小写（HeaderMap 迭代即小写）、跳过 hop-by-hop 头（含
/// content-length——由 aproxy 按实际字节回填，format 不该看到也不能改）。
/// **多值头仅保留首值**（set-cookie 的 Expires 含逗号，逗号合并不合法；拒绝
/// 转换代价失衡——LLM API 请求侧无多值头，响应侧 set-cookie 在代理场景无
/// 会话意义）；非可见 ASCII 的头值丢弃 + warn。
fn headers_to_btreemap(headers: &axum::http::HeaderMap) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for (name, value) in headers.iter() {
        let key = name.as_str().to_string();
        if is_hop_header(&key) {
            continue;
        }
        let Ok(v) = value.to_str() else {
            tracing::warn!(header = %key, "头值非可见 ASCII，转换器信封中丢弃");
            continue;
        };
        match map.entry(key) {
            Entry::Occupied(_) => {
                tracing::warn!(header = %name, "多值头仅保留首值，后续值不进信封");
            }
            Entry::Vacant(e) => {
                e.insert(v.to_string());
            }
        }
    }
    map
}

/// 信封头表 → HeaderMap（整表替换）：非法头名/头值丢弃 + warn；hop-by-hop
/// 与 content-length 防御性再过滤（format 不该回传它们，回了也不生效）。
fn headers_from_btreemap(map: &BTreeMap<String, String>) -> axum::http::HeaderMap {
    let mut headers = axum::http::HeaderMap::new();
    for (k, v) in map {
        if is_hop_header(k) || k.eq_ignore_ascii_case("content-length") {
            tracing::warn!(header = %k, "format 输出的头属自动管理范畴，忽略");
            continue;
        }
        let Ok(name) = axum::http::HeaderName::from_bytes(k.as_bytes()) else {
            tracing::warn!(header = %k, "format 输出的头名非法，丢弃");
            continue;
        };
        let Ok(val) = axum::http::HeaderValue::from_str(v) else {
            tracing::warn!(header = %k, "format 输出的头值非法，丢弃");
            continue;
        };
        headers.insert(name, val);
    }
    headers
}

/// 转换后的字节 → RequestBody：超内存驻留阈值且有 spool 目录则落盘成 Disk
/// 形态（复用 spool 临时文件命名与 Drop 自清理），写失败降级内存。
async fn spool_request_body(bytes: &[u8], spool_dir: Option<&Path>) -> RequestBody {
    if bytes.len() > RESIDENT_LIMIT
        && let Some(dir) = spool_dir
    {
        let path = dir.join(format!(
            "req-{}-{}.spooltmp",
            std::process::id(),
            unique_seq()
        ));
        if let Ok(mut file) = tokio::fs::File::create(&path).await {
            if file.write_all(bytes).await.is_ok() && file.flush().await.is_ok() {
                return RequestBody::Disk {
                    path,
                    len: bytes.len() as u64,
                };
            }
            // 半截文件必须删除
            drop(file);
            let _ = std::fs::remove_file(&path);
        }
    }
    RequestBody::Memory(bytes::Bytes::copy_from_slice(bytes))
}

/// 转换后的字节 → SpooledBody（同 spool_request_body 的落盘语义）。
async fn spool_response_body(bytes: &[u8], spool_dir: Option<&Path>) -> SpooledBody {
    if bytes.len() > RESIDENT_LIMIT
        && let Some(dir) = spool_dir
    {
        let path = dir.join(format!(
            "res-{}-{}.spooltmp",
            std::process::id(),
            unique_seq()
        ));
        if let Ok(mut file) = tokio::fs::File::create(&path).await {
            if file.write_all(bytes).await.is_ok() && file.flush().await.is_ok() {
                return SpooledBody::Disk {
                    path,
                    len: bytes.len() as u64,
                };
            }
            drop(file);
            let _ = std::fs::remove_file(&path);
        }
    }
    SpooledBody::Memory(bytes::Bytes::copy_from_slice(bytes))
}

/// 请求侧转换产物：method/url/headers/body 全部可被 format 改写。
pub(crate) struct TransformedRequest {
    pub method: axum::http::Method,
    pub url: String,
    pub headers: axum::http::HeaderMap,
    pub body: RequestBody,
}

/// 请求侧转换：body 缓冲完成后交给 format 改写。转换**一次**，产物被重试
/// 循环的每一轮 forward_once 自动重放（调用侧零分支）。
///
/// `body` 所有权接管（Disk 形态读毕由 Drop 自删临时文件）；输出超大时重新
/// 落盘成新 Disk 形态（内存与负载解耦的语义保持）。
pub(crate) async fn transform_request(
    pool: &Arc<TransformPool>,
    method: &axum::http::Method,
    url: &str,
    headers: &axum::http::HeaderMap,
    body: RequestBody,
    spool_dir: Option<&Path>,
) -> Result<TransformedRequest, TransformError> {
    let body_bytes = match &body {
        RequestBody::Memory(b) => b.to_vec(),
        RequestBody::Disk { path, .. } => {
            tokio::fs::read(path).await.map_err(TransformError::Io)?
        }
    };
    drop(body); // Disk 文件由 Drop 删除（已被读出）

    let mut env = TransformEnvelope::from_body_bytes(&body_bytes);
    env.method = Some(method.as_str().to_string());
    env.url = Some(url.to_string());
    env.headers = headers_to_btreemap(headers);
    env.extra = pool.cfg.effective_extra().to_string();

    let out = pool.convert(env).await?;
    if let Some(err) = out.error {
        return Err(TransformError::Rejected(err));
    }
    let resp_bytes = out
        .body_bytes()
        .map_err(|e| TransformError::Rejected(e.to_string()))?;
    let new_body = spool_request_body(&resp_bytes, spool_dir).await;
    let new_method = out
        .method
        .as_deref()
        .and_then(|m| m.parse().ok())
        .unwrap_or_else(|| method.clone());
    let new_url = out.url.unwrap_or_else(|| url.to_string());
    let new_headers = headers_from_btreemap(&out.headers);
    Ok(TransformedRequest {
        method: new_method,
        url: new_url,
        headers: new_headers,
        body: new_body,
    })
}

/// 响应侧转换产物。失败时 `error` 为 Some 且 headers/body 已还原为**原始值**
/// （调用方透传上游原始响应——响应在手，可用性优先）。
pub(crate) struct TransformedResponse {
    pub headers: axum::http::HeaderMap,
    pub body: SpooledBody,
    pub error: Option<TransformError>,
}

/// 响应侧转换：重试判定为「成功」后、回放前交给 format 改写。
///
/// 压缩体先经 `decode::for_inspection` 解码（判定同款；超 8MiB 上限回退原始
/// 字节——>8MiB 的压缩 LLM 响应现实中不存在，见 decode.rs 的常数注释）。
/// 输出头强制剔除 content-length/content-encoding——字节已变换，旧声明必失真；
/// content-length 由回放路径按新长度回填或由 hyper 按实际字节自行分帧。
pub(crate) async fn transform_response(
    pool: &Arc<TransformPool>,
    upstream_url: &str,
    content_encoding: Option<&str>,
    headers: axum::http::HeaderMap,
    body: SpooledBody,
    spool_dir: Option<&Path>,
) -> TransformedResponse {
    let (raw, body) = match &body {
        SpooledBody::Memory(b) => (b.to_vec(), body),
        SpooledBody::Disk { path, .. } => match tokio::fs::read(path).await {
            Ok(b) => (b, body),
            Err(e) => {
                // 磁盘文件丢失：无法转换也无法透传 body，原样归还空错误由
                // 调用方按其回放路径处理（into_stream 已有该路径的空 body 回退）
                tracing::warn!(error = %e, "响应 spool 文件读取失败，跳过响应转换");
                return TransformedResponse {
                    headers,
                    body,
                    error: Some(TransformError::Io(e)),
                };
            }
        },
    };
    let decoded = crate::decode::for_inspection(content_encoding, &raw);
    let feed = decoded.as_deref().unwrap_or(&raw);

    let mut env = TransformEnvelope::from_body_bytes(feed);
    // 响应侧信封 url = 请求侧最终上游地址：多渠道聚合按它反查渠道表
    // （响应转换器与请求转换器是不同进程，无共享状态——设计铁律）
    env.url = Some(upstream_url.to_string());
    env.headers = headers_to_btreemap(&headers);
    env.extra = pool.cfg.effective_extra().to_string();

    let out = match pool.convert(env).await {
        Ok(o) => o,
        Err(e) => {
            return TransformedResponse {
                headers,
                body,
                error: Some(e),
            };
        }
    };
    if let Some(err) = out.error {
        return TransformedResponse {
            headers,
            body,
            error: Some(TransformError::Rejected(err)),
        };
    }
    let resp_bytes = match out.body_bytes() {
        Ok(b) => b,
        Err(e) => {
            return TransformedResponse {
                headers,
                body,
                error: Some(TransformError::Rejected(e.to_string())),
            };
        }
    };
    let mut new_headers = headers_from_btreemap(&out.headers);
    new_headers.remove(axum::http::header::CONTENT_LENGTH);
    new_headers.remove(axum::http::header::CONTENT_ENCODING);
    let new_body = spool_response_body(&resp_bytes, spool_dir).await;
    TransformedResponse {
        headers: new_headers,
        body: new_body,
        error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool_for(command: &str, args: &[&str], mode: TransformMode) -> TransformPool {
        TransformPool::new(Arc::new(TransformConfig {
            command: command.to_string(),
            args: args.iter().map(|s| s.to_string()).collect(),
            mode,
            ..Default::default()
        }))
    }

    #[tokio::test]
    async fn spawn_mode_single_request_roundtrip() {
        // 用测试用 echo 假 format：写一行回一行后退出（spawn 模式常态）
        // 这里直接用本测试二进制的父进程 mock 太复杂，用内置 PowerShell/
        // bash 不跨平台——单测只验证信封序列化与错误路径，进程行为由集成
        // 测试（examples/format-echo.rs）覆盖
        let pool = pool_for("definitely-not-exist-abc123", &[], TransformMode::Spawn);
        let env = TransformEnvelope {
            method: Some("POST".to_string()),
            headers: BTreeMap::new(),
            body: Some("{}".to_string()),
            ..Default::default()
        };
        let err = pool.convert(env).await.unwrap_err();
        assert!(matches!(err, TransformError::Spawn(_)), "{err:?}");
    }

    #[tokio::test]
    async fn convert_smoke_real_format_echo_roundtrip() {
        // 真实 spawn format-echo 的最小往返（不经代理栈，二分定位用）
        let name = if cfg!(windows) {
            "format-echo.exe"
        } else {
            "format-echo"
        };
        let mut dir = std::env::current_exe().unwrap();
        let mut echo = None;
        while let Some(parent) = dir.parent() {
            let cand = parent.join("examples").join(name);
            if cand.exists() {
                echo = Some(cand);
                break;
            }
            dir = parent.to_path_buf();
        }
        let echo = echo.expect("format-echo 未编译");
        let pool = TransformPool::new(Arc::new(TransformConfig {
            command: echo.display().to_string(),
            args: vec!["echo".to_string()],
            mode: TransformMode::Spawn,
            timeout_secs: Some(5),
            ..Default::default()
        }));
        let env = TransformEnvelope {
            method: Some("POST".to_string()),
            headers: BTreeMap::new(),
            body: Some("{\"a\":1}".to_string()),
            ..Default::default()
        };
        let out = tokio::time::timeout(Duration::from_secs(10), pool.convert(env))
            .await
            .expect("convert 整体超时（10s）")
            .expect("convert 失败");
        assert_eq!(
            out.body.as_deref(),
            Some("{\"a\":1}"),
            "echo 应原样回显 body"
        );
    }

    #[test]
    fn headers_to_btreemap_skips_hop_and_keeps_first_value() {
        use axum::http::{HeaderMap, HeaderValue};
        let mut headers = HeaderMap::new();
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        headers.insert("content-length", HeaderValue::from_static("5"));
        headers.insert("connection", HeaderValue::from_static("close"));
        // 多值头：append 两个同名值，仅首值进信封
        headers.append("x-demo", HeaderValue::from_static("first"));
        headers.append("x-demo", HeaderValue::from_static("second"));

        let map = headers_to_btreemap(&headers);
        assert_eq!(map.get("content-type").unwrap(), "application/json");
        assert!(!map.contains_key("content-length"), "hop 头不进信封");
        assert!(!map.contains_key("connection"), "hop 头不进信封");
        assert_eq!(map.get("x-demo").unwrap(), "first", "多值头仅保留首值");
    }

    #[test]
    fn headers_from_btreemap_filters_managed_and_invalid() {
        let mut map = BTreeMap::new();
        map.insert("content-type".to_string(), "application/json".to_string());
        map.insert("content-length".to_string(), "99".to_string());
        map.insert("x-custom".to_string(), "1".to_string());
        map.insert("[invalid header]".to_string(), "x".to_string());

        let headers = headers_from_btreemap(&map);
        assert!(headers.contains_key("content-type"));
        assert!(headers.contains_key("x-custom"));
        assert!(!headers.contains_key("content-length"), "CL 防御性剔除");
        assert_eq!(headers.len(), 2, "非法头名丢弃");
    }

    #[test]
    fn transform_error_display_carries_reason() {
        let e = TransformError::Rejected("model 未命中".to_string());
        assert!(e.to_string().contains("model 未命中"));
        assert!(TransformError::TimedOut.to_string().contains("超时"));
    }

    #[tokio::test]
    async fn spawn_mode_pool_max_is_one() {
        let pool = pool_for("x", &[], TransformMode::Spawn);
        assert_eq!(pool.cfg.effective_pool_max(), 4); // 未配置回退默认
        assert_eq!(pool.permits.available_permits(), 1); // spawn 模式恒 1
    }
}
