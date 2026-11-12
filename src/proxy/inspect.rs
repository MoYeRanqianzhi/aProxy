//! 响应检查：是否需要重试的判定（状态码、错误体、流内错误事件）与日志预览。

use super::*;

/// 扫描器可依赖的行长度上界：错误 JSON 行远小于此；超出即截断该行（丢一行
/// 启发式判定正确性，换内存上界恒定）。
pub(super) const SCAN_MAX_LINE: usize = 512 * 1024;

/// 磁盘模式的流式错误扫描器：增量逐行判定（SSE data 行 / NDJSON 行），
/// 与 `retry::is_stream_error_body` 的行级语义一致（见其测试锚定：
/// 流尾 error 事件必须命中、CRLF/前导空格变体兼容）。
pub(super) struct StreamErrorScanner {
    /// 跨 chunk 的未成行尾巴
    pub(super) tail: Vec<u8>,
    /// 已判定出现 data 行（SSE 模式锁定）
    pub(super) saw_data_line: bool,
    /// SSE 模式下的判定结果（锁定后不再改写）
    pub(super) sse_has_error: bool,
    /// 已锁定结论
    pub(super) decided: bool,
    /// 非 SSE 的 NDJSON 命中
    pub(super) ndjson_error: bool,
}

impl StreamErrorScanner {
    pub(super) fn new() -> Self {
        Self {
            tail: Vec::new(),
            saw_data_line: false,
            sse_has_error: false,
            decided: false,
            ndjson_error: false,
        }
    }

    pub(super) fn feed(&mut self, chunk: &[u8]) {
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

    pub(super) fn scan_line(&mut self, line: &[u8]) {
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
    pub(super) fn finish(mut self) -> bool {
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

/// 统一的重试判定入口：内存模式走整体判定（旧行为），磁盘模式走增量扫描
/// 结论 + 头部快照预览。三处（首轮/两个重试通道）共用，保证判定语义一致。
pub(super) fn needs_retry_response(
    attempt: u32,
    status: &StatusCode,
    raw_headers: &reqwest::header::HeaderMap,
    body: &SpooledBody,
    disk_scan: &Option<(bool, Vec<u8>)>,
) -> bool {
    match (body, disk_scan) {
        (SpooledBody::Memory(b), _) => should_retry_response(attempt, status, raw_headers, b),
        // 真实响应大小取自 spool 长度（`head` 只是 1 KiB 头部快照，其长度不能
        // 当响应大小用——预览日志的「共 N 字节」正因此改成显式传入）
        (SpooledBody::Disk { len, .. }, Some((error, head))) => {
            should_retry_response_disk(attempt, status, raw_headers, *error, head, *len as usize)
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
pub(super) fn should_retry_response(
    attempt: u32,
    status: &StatusCode,
    raw_headers: &reqwest::header::HeaderMap,
    body: &[u8],
) -> bool {
    // 压缩响应体（上游按客户端 accept-encoding 压缩，而 reqwest 为保真透传刻意
    // 不解压）在**原始字节**上做任何内容判定都是无效的：JSON 解析与 SSE 行扫描
    // 会静默失败，预览只能打 hex。故检查一律走解码副本——转发给客户端的字节
    // 不受影响（`decode` 模块文档记录了这组失效的完整清单与事故由来）。
    // 无 content-encoding 时 for_inspection 立即返回 None，不产生任何拷贝。
    let inspected = crate::decode::for_inspection(
        header_opt(raw_headers, http::header::CONTENT_ENCODING),
        body,
    );
    let body = inspected.as_deref().unwrap_or(body);

    let is_streaming = retry::is_streaming_response(raw_headers, body);

    // 1. HTTP 状态码可重试：无论是否流式，都重试
    if retry::is_retryable_status(status.as_u16()) {
        tracing::warn!(attempt, status = %status, is_streaming, "上游返回可重试状态码，重试");
        warn_preview(raw_headers, body, 500, body.len());
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
        tracing::warn!(attempt, is_streaming, "上游返回错误内容，重试");
        warn_preview(raw_headers, body, 1000, body.len());
        return true;
    }

    false
}

/// 磁盘模式的重试判定：状态码/流式判定用响应头；内容错误用收集期间的增量
/// 扫描结论；预览用头部快照（磁盘全量不可得，头部 1 KiB 足够排障）。
///
/// `total_bytes` 是 spool 里的**实际响应字节数**（`SpooledBody::Disk::len`）：
/// 内存模式那份 body 就是全部内容，长度自证；磁盘模式给到这里的只是 1 KiB 快照，
/// 快照长度与响应大小无关，日志里的字节数必须由调用方从 spool 长度补进来。
pub(super) fn should_retry_response_disk(
    attempt: u32,
    status: &StatusCode,
    raw_headers: &reqwest::header::HeaderMap,
    scan_error: bool,
    head_snapshot: &[u8],
    total_bytes: usize,
) -> bool {
    let is_streaming = retry::is_streaming_content_type(raw_headers);

    // 预览用解码：头部快照正是压缩流的开头，解出的前缀足以看清错误页内容
    //（截断流取已解出部分）。**判定**不在此列——`scan_error` 来自原始字节的
    // 增量扫描结论；为 >1 MiB 的响应改造成流式解码，收益与风险不成比例，
    // 见 `decode` 模块文档的「已知边界」。
    let inspected = crate::decode::for_inspection(
        header_opt(raw_headers, http::header::CONTENT_ENCODING),
        head_snapshot,
    );
    let head = inspected.as_deref().unwrap_or(head_snapshot);

    // 1. HTTP 状态码可重试：无论是否流式，都重试
    if retry::is_retryable_status(status.as_u16()) {
        tracing::warn!(attempt, status = %status, is_streaming, "上游返回可重试状态码，重试");
        warn_preview(raw_headers, head, 500, total_bytes);
        return true;
    }

    // 2. 内容错误：增量扫描结论（SSE data 行 / NDJSON 行级判定）
    if scan_error {
        tracing::warn!(attempt, is_streaming, "上游返回错误内容，重试");
        warn_preview(raw_headers, head, 1000, total_bytes);
        return true;
    }

    false
}

/// 取响应头值的字符串视图；缺失或非 UTF-8 → `None`。
/// 保留 `None`（而非直接给占位符）是因为调用方需要区分「有没有这个头」——
/// 解码与否正取决于此（`"-"` 会被当成一个未知编码）。
pub(super) fn header_opt(
    headers: &reqwest::header::HeaderMap,
    name: http::header::HeaderName,
) -> Option<&str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

/// 错误响应预览日志：预览内容 + `content-type` / `content-encoding`。
///
/// 两个头在任何情况下都打——**解码失败退化为 hex 摘要时，它们正是判断「上游
/// 到底回了什么」的关键线索**：brotli 没有 magic number，光看字节连「这是压缩体」
/// 都认不出（2026-09-14 事故：Cloudflare 的 brotli 404 页在日志里只剩一串 hex）。
/// 缺失时以 `"-"` 占位，日志字段不留空以便 grep 与扫读。
/// `total_bytes` 是「共 N 字节」的口径，由调用方按自己掌握的信息给出：
/// 内存模式传整份 body 的长度（body 就在手上，长度即响应大小）；磁盘模式只能
/// 传真值——传 `SpooledBody::Disk::len`（spool 里的实际字节数）。见 `preview_body`。
pub(super) fn warn_preview(
    raw_headers: &reqwest::header::HeaderMap,
    body: &[u8],
    limit: usize,
    total_bytes: usize,
) {
    tracing::warn!(
        content_type = %header_opt(raw_headers, http::header::CONTENT_TYPE).unwrap_or("-"),
        content_encoding = %header_opt(raw_headers, http::header::CONTENT_ENCODING).unwrap_or("-"),
        preview = %preview_body(body, limit, total_bytes),
        "错误响应预览"
    );
}

/// 错误响应体的日志预览。**压缩体已在调用方按 `content-encoding` 解码**
/// （见 `decode` 模块），走到这里的是解不出的、本就二进制/压缩但编码未知的体；
/// 直接 from_utf8_lossy 会把控制字节渲染成整片乱码污染日志。判定：替换符/控制
/// 字符占比超阈值视为二进制，改为 hex 摘要（hex 可识别压缩 magic：zstd
/// 28 b5 2f fd、gzip 1f 8b 等，据此判断解码为何没成功）。
/// `total_bytes` 是日志里「共 N 字节」用的**实际响应字节数**，不是本函数手里
/// 这份字节的长度。两者只在内存模式相同；磁盘模式传入的是 1 KiB **头部快照**
/// （且可能已被 `content-encoding` 解码成更短的副本），其长度只反映快照本身，
/// 与响应大小无关——照快照长度打印会让读日志的人以为上游只回了这么点数据，
/// 把排障方向带偏（磁盘模式 > 1 MiB 的响应恰恰最需要看真实量级）。
pub(super) fn preview_body(body: &[u8], limit: usize, total_bytes: usize) -> String {
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
            "（二进制/压缩内容，共 {total_bytes} 字节，hex 前 48: {}…）",
            hex.trim_end()
        )
    } else {
        let s = text.trim_end();
        if body.len() > limit {
            format!("{s}…（截断，共 {total_bytes} 字节）")
        } else {
            s.to_string()
        }
    }
}
