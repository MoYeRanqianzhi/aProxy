//! 判定完整上游响应是否携带错误；内存实体与大实体的增量扫描分别遵循实例的边界。

/// 内存驻留阈值：缓冲超过即溢写磁盘（disk_cache 开启时）。
/// 1 MiB：覆盖真实 agent 流量（SSE/JSON 错误体 KB 级）使其零磁盘开销，
/// 大请求体/大响应才进入磁盘路径。
pub const RESIDENT_LIMIT: usize = 1024 * 1024;

/// 判断响应体是否表示“错误内容”而非正常业务响应。
///
/// 启发式：若响应体为 JSON 且包含明确的错误字段，则视为可重试的错误。
/// 该检查用于“自动拦截一切内容为报错的内容并重试”的需求：
/// 即使 HTTP 状态码为 200，若 body 本身是错误 JSON，也应重试。
///
/// 规则：
/// - body 必须能解析为 JSON 对象
/// - 且包含 `error` 键（值为对象或字符串）或 `type == "error"`，则判定为错误内容
pub fn is_error_body(body: &[u8]) -> bool {
    // 命中要求键名或字符串解码后含 error；JSON 中这五个字母只能是原文或 \u 转义。
    // 这里只排除不可能的体，重复键、转义与不合法的输入仍由 JSON 解析器裁决。
    if memchr::memmem::find(body, b"error").is_none()
        && memchr::memmem::find(body, br"\u").is_none()
    {
        return false;
    }
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(body) else {
        return false;
    };
    let Some(obj) = v.as_object() else {
        return false;
    };
    if obj.contains_key("error") {
        // error: null 视为未携带有效错误——某些上游成功体会带 error:null 字段，
        // 若判为错误会导致 200 成功响应被无限重试
        if !obj.get("error").map(|v| v.is_null()).unwrap_or(false) {
            return true;
        }
    }
    obj.get("type").and_then(|t| t.as_str()) == Some("error")
}

/// 判断流式缓冲体中是否包含错误事件。
///
/// 流式场景下，即使 HTTP 状态为 200，流内的某个 SSE/NDJSON 数据块也可能
/// 携带错误（如 `data: {"type":"error",...}`）。若直接管道透传，客户端
/// 已收到部分数据便无法重试；因此需要在缓冲完成后扫描整个流内容，命中
/// 错误即视为可重试。
///
/// 检测策略（启发式，按优先级）：
/// 1. 扫描 SSE `data:` 行：对每一行 `data: <json>` 尝试 `is_error_body`
/// 2. 若未发现 SSE 结构，则按 NDJSON 逐行尝试 `is_error_body`
/// 3. 兜底：对整体 body 尝试 `is_error_body`
pub fn is_stream_error_body(body: &[u8]) -> bool {
    // SSE 路径：只要出现过 `data:` 就认为是 SSE 流，扫描其 data 载荷
    if let Ok(text) = std::str::from_utf8(body) {
        let mut saw_data_line = false;
        let mut sse_has_error = false;
        for line in text.lines() {
            let trimmed = line.trim();
            // SSE 允许 `data:` 后紧跟空格或直接跟 JSON
            let Some(rest) = trimmed
                .strip_prefix("data:")
                .or_else(|| trimmed.strip_prefix("data :"))
            else {
                continue;
            };
            let data = rest.trim();
            if data.is_empty() || data == "[DONE]" {
                continue;
            }
            saw_data_line = true;
            if is_error_body(data.as_bytes()) {
                sse_has_error = true;
                break;
            }
        }
        if saw_data_line {
            return sse_has_error;
        }

        // NDJSON 路径：逐行 JSON 检测
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            // 仅对看起来像 JSON 的行做检测，避免对普通 text 误判
            if line.starts_with('{') && is_error_body(line.as_bytes()) {
                return true;
            }
        }
    }

    // 兜底：整体作为 JSON 检测
    is_error_body(body)
}

/// 允许上游未标 Content-Type，但流内存在 SSE 数据行。
pub fn sniffs_stream(body: &[u8]) -> bool {
    // 嗅探 body：包含 SSE data 行则视为流式
    if let Ok(text) = std::str::from_utf8(body) {
        for line in text.lines() {
            let t = line.trim();
            if t.starts_with("data:") || t.starts_with("data :") {
                return true;
            }
        }
    }
    false
}

/// 判定时接受含流媒体类型的头；这不是客户端实际收到流的证明。
pub fn stream_content_type(ct: &str) -> bool {
    let lower = ct.to_ascii_lowercase();
    lower.contains("text/event-stream") || lower.contains("application/x-ndjson")
}

/// 扫描器可依赖的行长度上界：错误 JSON 行远小于此；超出即截断该行（丢一行
/// 启发式判定正确性，换内存上界恒定）。
const SCAN_MAX_LINE: usize = 512 * 1024;

/// 磁盘模式的流式错误扫描器：增量逐行判定（SSE data 行 / NDJSON 行），
/// 与 `retry::is_stream_error_body` 的行级语义一致（见其测试锚定：
/// 流尾 error 事件必须命中、CRLF/前导空格变体兼容）。
#[derive(Default)]
pub struct StreamErrorScanner {
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
    /// 按每次上游尝试独立累计行，不保留前一次结果。
    pub fn new() -> Self {
        Self {
            tail: Vec::new(),
            saw_data_line: false,
            sse_has_error: false,
            decided: false,
            ndjson_error: false,
        }
    }

    /// 以传输块更新尚未成行的尾部与已判定的错误。
    pub fn feed(&mut self, chunk: &[u8]) {
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
            if !data.is_empty() && data != "[DONE]" && is_error_body(data.as_bytes()) {
                self.sse_has_error = true;
                self.decided = true;
            }
        } else if trimmed.starts_with('{') && is_error_body(trimmed.as_bytes()) {
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
    pub fn finish(mut self) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;

    // 对照组不经字节筛选，总是构建完整的 JSON 值。
    fn parsed_error_body(body: &[u8]) -> bool {
        let Ok(v) = serde_json::from_slice::<serde_json::Value>(body) else {
            return false;
        };
        let Some(obj) = v.as_object() else {
            return false;
        };
        if obj.contains_key("error") {
            // error: null 视为未携带有效错误——某些上游成功体会带 error:null 字段，
            // 若判为错误会导致 200 成功响应被无限重试
            if !obj.get("error").map(|v| v.is_null()).unwrap_or(false) {
                return true;
            }
        }
        obj.get("type").and_then(|t| t.as_str()) == Some("error")
    }

    // 固定种子让生成的输入与切分点可以复现，不依赖系统的随机源。
    fn sample(state: &mut u32, limit: usize) -> usize {
        *state ^= *state << 13;
        *state ^= *state >> 17;
        *state ^= *state << 5;
        *state as usize % limit
    }

    #[test]
    fn byte_filter_agrees_with_full_parse() {
        let check = |body: &[u8]| {
            assert_eq!(is_error_body(body), parsed_error_body(body), "{body:?}");
        };
        for body in [
            br#"{"error": "overloaded"}"#.as_slice(),
            br#"{"error": {"message": "x"}}"#,
            br#"{"type": "error", "message": "x"}"#,
            br#"{"content": "hello"}"#,
            br#"{"error": []}"#,
            br#"{"error": null}"#,
            br#"{"error": null, "ok": true}"#,
            br#"{"type": "Error"}"#,
            br#"{"data":{"error":"x"}}"#,
            br#"{"\u0065rror":1}"#,
            br#"{"type":"\u0065rror"}"#,
            br#"{"err\u006fr":null}"#,
            br#"{"error":1,"error":null}"#,
            br#"{"type":"error","type":"x"}"#,
            b"not json",
            b"error outside JSON",
            b"[]",
            br#"[{"error":1}]"#,
            b"null",
            b"42",
            br#""hello""#,
            br#"{"error":1"#,
            b"\xff",
            b"{\"error\":\"\xff\"}",
        ] {
            check(body);
        }

        // 手工拼 JSON 源码：序列化一个 map 会先折叠重复键，也保不住指定字母的转义。
        let keys = [
            "error",
            r"\u0065rror",
            r"err\u006fr",
            "type",
            r"typ\u0065",
            "data",
            "other",
        ];
        let values = [
            "null",
            "false",
            "1",
            "[]",
            r#""x""#,
            r#""error""#,
            r#""\u0065rror""#,
            r#""\\u0065rror""#,
            r#"{"error":"x"}"#,
        ];
        let mut state = 0xa30d51;
        for _ in 0..2048 {
            let mut object = String::from("{");
            for i in 0..sample(&mut state, 7) {
                if i != 0 {
                    object.push(',');
                }
                let key = keys[sample(&mut state, keys.len())];
                let value = values[sample(&mut state, values.len())];
                object.push_str(&format!(r#""{key}":{value}"#));
            }
            object.push('}');
            check(object.as_bytes());
        }
    }
}
