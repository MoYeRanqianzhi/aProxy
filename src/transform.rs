//! 外部 format 的运行时。协议只走子进程的 stdin/stdout，stderr 独立读走。
use crate::proxy::{
    RESIDENT_LIMIT, RequestBody, SpoolPath, SpooledBody, is_hop_header, unique_seq,
};
use aproxy_envelope as wire;
use std::{
    collections::{BTreeMap, btree_map::Entry},
    path::Path,
    sync::{Arc, Mutex},
};
use tokio::io::AsyncWriteExt;
mod pool;
pub(crate) use pool::{Fault, Pool};
/// 头表 → 信封：键小写（HeaderMap 迭代即小写）、跳过 hop-by-hop 头（含
/// content-length——由 aproxy 按实际字节回填，format 不该看到也不能改）。
/// 编码头按顺序合并；**其余多值头仅保留首值**（set-cookie 的 Expires 含逗号，逗号合并不合法；拒绝
/// 转换代价失衡——LLM API 请求侧无多值头，响应侧 set-cookie 在代理场景无
/// 会话意义）；非可见 ASCII 的头值丢弃 + warn。
pub(crate) fn headers_to_btreemap(headers: &axum::http::HeaderMap) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for (name, value) in headers.iter() {
        let key = name.as_str().to_string();
        if name == axum::http::header::CONTENT_ENCODING || is_hop_header(&key) {
            continue;
        }
        let Ok(v) = value.to_str() else {
            tracing::warn!("non-ASCII header omitted from format input");
            continue;
        };
        match map.entry(key) {
            Entry::Occupied(_) => {
                tracing::warn!("duplicate header omitted from format input");
            }
            Entry::Vacant(e) => {
                e.insert(v.to_string());
            }
        }
    }
    if let Some(encoding) = crate::proxy::request_encoding(headers, true) {
        match encoding.to_str() {
            Ok(v) => {
                map.insert("content-encoding".into(), v.into());
            }
            Err(_) => tracing::warn!("non-ASCII header omitted from format input"),
        }
    }
    map
}

/// 信封头表 → HeaderMap（整表替换）：非法头名/头值丢弃 + warn；hop-by-hop
/// 与 content-length 防御性再过滤（format 不该回传它们，回了也不生效）。
pub(crate) fn headers_from_btreemap(map: &BTreeMap<String, String>) -> axum::http::HeaderMap {
    let mut headers = axum::http::HeaderMap::new();
    for (k, v) in map {
        if is_hop_header(k) || k.eq_ignore_ascii_case("content-length") {
            tracing::warn!("managed header omitted from format output");
            continue;
        }
        let Ok(name) = axum::http::HeaderName::from_bytes(k.as_bytes()) else {
            tracing::warn!("invalid header name omitted from format output");
            continue;
        };
        let Ok(val) = axum::http::HeaderValue::from_str(v) else {
            tracing::warn!("invalid header value omitted from format output");
            continue;
        };
        if val.to_str().is_err() {
            tracing::warn!("invalid header value omitted from format output");
            continue;
        }
        headers.insert(name, val);
    }
    headers
}

/// 转换后的字节 → RequestBody：超内存驻留阈值且有 spool 目录则落盘成 Disk
/// 形态（复用 spool 临时文件命名与 Drop 自清理），写失败降级内存。
pub(crate) async fn spool_request_body(bytes: &[u8], spool_dir: Option<&Path>) -> RequestBody {
    if bytes.len() > RESIDENT_LIMIT
        && let Some(dir) = spool_dir
    {
        let path = SpoolPath(dir.join(format!(
            "req-{}-{}.spooltmp",
            std::process::id(),
            unique_seq()
        )));
        if let Ok(mut file) = tokio::fs::File::create(&path.0).await
            && file.write_all(bytes).await.is_ok()
            && file.flush().await.is_ok()
        {
            return RequestBody::Disk {
                path: path.into_path(),
                len: bytes.len() as u64,
            };
        }
        // 写失败或 future 中途取消，都由路径守卫删除半截文件。
    }
    RequestBody::Memory(bytes::Bytes::copy_from_slice(bytes))
}

/// 转换后的字节 → SpooledBody（同 spool_request_body 的落盘语义）。
pub(crate) async fn spool_response_body(bytes: &[u8], spool_dir: Option<&Path>) -> SpooledBody {
    if bytes.len() > RESIDENT_LIMIT
        && let Some(dir) = spool_dir
    {
        let path = SpoolPath(dir.join(format!(
            "res-{}-{}.spooltmp",
            std::process::id(),
            unique_seq()
        )));
        if let Ok(mut file) = tokio::fs::File::create(&path.0).await
            && file.write_all(bytes).await.is_ok()
            && file.flush().await.is_ok()
        {
            return SpooledBody::Disk {
                path: path.into_path(),
                len: bytes.len() as u64,
            };
        }
    }
    SpooledBody::Memory(bytes::Bytes::copy_from_slice(bytes))
}

/// 库内单测用的 examples/format-echo(.exe)：跨平台的假 format 程序（不依赖系统里的 python），
/// 本模块与 proxy 的单测共用这一处。example 在 `target/<profile>/examples/` 里，库的单测程序
/// 在同一个 profile 目录下面，但深度随 cargo 版本而变（1.99 及更早是 `deps/`，1.100 起是
/// `build/<包名>/<hash>/out/`），所以从单测程序自己的路径逐级往上，找第一个带着这个 example
/// 的目录。
///
/// 它是 example，不是单测的依赖：只跑 `cargo test --lib` 时 cargo 不编译它，干净的构建目录下
/// 要先 `cargo build --example format-echo`，否则这里 panic 并给出这条命令。
#[cfg(test)]
pub(crate) fn format_echo_path() -> std::path::PathBuf {
    let exe = std::env::current_exe().unwrap();
    let relative =
        Path::new("examples").join(format!("format-echo{}", std::env::consts::EXE_SUFFIX));
    exe.ancestors()
        .map(|dir| dir.join(&relative))
        .find(|path| path.is_file())
        .unwrap_or_else(|| {
            panic!(
                "从 {} 往上找不到 {}：只跑库的单测时 cargo 不编译 example，先执行 `cargo build --example format-echo`",
                exe.display(),
                relative.display()
            )
        })
}

/// 子进程通过本次启动专属的文件交回握手结果，不解析共享日志里的自由文本。
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct StartupError {
    /// 已打码的配置条目名。
    pub name: String,
    /// 只保留协议分类，不携带任意 stdout。
    pub(crate) fault: Fault,
}
impl StartupError {
    /// 稳定诊断码区分阶段与协议不匹配。
    pub fn code(&self) -> &'static str {
        if matches!(self.fault, Fault::Stages(_)) {
            "format_stage_unsupported"
        } else {
            "format_handshake_failed"
        }
    }
    /// 不可用是运行错误，静态不匹配是配置错误。
    pub fn exit_code(&self) -> u8 {
        if self.fault.unavailable() { 1 } else { 3 }
    }
    /// 与码表对应的诊断细节；阶段取配置值，不取进程的任意文本。
    pub fn details(&self) -> serde_json::Value {
        match &self.fault {
            Fault::Stages(s) => serde_json::json!({"format":self.name,"stages":s}),
            f => serde_json::json!({"format":self.name,"kind":f.kind()}),
        }
    }
}
impl std::fmt::Display for StartupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "format {:?}: {} [{}]",
            self.name,
            self.fault,
            self.code()
        )
    }
}

/// 不同程序的状态分开保存；心跳只读，不会覆盖同时进行的请求转换。
#[derive(Clone)]
pub(crate) struct ExchangeCtx {
    /// 请求链共享的请求标识。
    pub(crate) request_id: Arc<str>,
    /// 客户端路径与流标记不受路由改写影响。
    pub(crate) client: wire::Client,
    /// 只在读取一份状态或写入一个回信时持锁。
    states: Arc<Mutex<BTreeMap<String, String>>>,
    /// 报告写进原请求的同一份记录。
    pub(crate) rec: crate::history::Recorder,
}
impl ExchangeCtx {
    /// 为客户端请求建立隔离的程序状态。
    pub(crate) fn new(
        request_id: Arc<str>,
        path: &str,
        stream: bool,
        rec: crate::history::Recorder,
    ) -> Self {
        Self {
            request_id,
            client: wire::Client::new(path.into(), stream),
            states: Default::default(),
            rec,
        }
    }
    /// 本层只收到自己之前给出的字符串。
    pub(crate) fn context(&self, name: &str) -> wire::Context {
        let mut c = wire::Context::new(self.request_id.to_string());
        c.state = self.states.lock().unwrap().get(name).cloned();
        c
    }
    /// 缺省时不改动，显式空串清除状态。
    pub(crate) fn absorb(&self, name: &str, value: Option<String>) {
        if let Some(v) = value {
            let mut state = self.states.lock().unwrap();
            if v.is_empty() {
                state.remove(name);
            } else {
                state.insert(name.into(), v);
            }
        }
    }
}
