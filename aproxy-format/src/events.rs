//! SSE 负责切事件，switchyard 负责语义。两份状态都属于一条已经 open 的流。
use crate::config::Fault;
use serde_json::Value;
use switchyard_translation::{StreamTranslationState, TranslationEngine, WireFormat as F};

/// 单个尚未结束的 SSE 事件上限，与整条流的长度无关。
const MAX_EVENT: usize = 8 * 1024 * 1024;

/// 编解码器可能保留整轮文本和工具参数，Responses 的最后一帧还会重复这些数据。
/// 分别限制累计送入语义层的 JSON 字节、单条回信的目标字节；为 8 MiB 管道帧中的 JSON 转义留余量。
pub(super) const SEMANTIC_BYTES: usize = 1024 * 1024;

/// 当前事件跨协议转换所需的字节边界、语义状态和终止标记。
pub(super) struct Events {
    /// 上游返回的协议。
    source: F,
    /// 客户端等待的协议；同协议不经过这里。
    target: F,
    /// 默认注册的三个协议编解码器，每条活跃流复用一份。
    engine: TranslationEngine,
    /// 工具调用、usage 与结束原因在多个源事件间关联。
    state: StreamTranslationState,
    /// 只对有语义的完整事件扣费，注释与网络分片不改变预算。
    remaining: usize,
    /// 只积累尚未被空行分隔的一帧，行结束已统一成 LF。
    frame: Vec<u8>,
    /// CR 已结束上一行，下一字节若是 LF 则属于同一换行。
    after_cr: bool,
    /// 仅第一帧可以有 UTF-8 BOM。
    first: bool,
    /// 已见协议结束标志；Chat 的 usage 块可能在 finish_reason 之后。
    terminal: bool,
    /// Chat 的 finish_reason 可能早于最后一个 usage 块；只保留原因，到 [DONE] 才交给编解码器。
    chat_finishes: Vec<Value>,
    /// Messages 的 stop_reason 提示原因而不保证完整；转成 Chat 时，须等 message_stop 再把这一帧交给编码器。
    message_finish: Option<Value>,
}
impl Events {
    /// 每次 open 初始化，不把上一条流的输出索引或 usage 带到下一条。
    pub(super) fn new(source: F, target: F) -> Self {
        Self {
            source,
            target,
            engine: TranslationEngine::default(),
            state: Default::default(),
            remaining: SEMANTIC_BYTES,
            frame: Vec::new(),
            after_cr: false,
            first: true,
            terminal: false,
            chat_finishes: Vec::new(),
            message_finish: None,
        }
    }
    /// 将已经完整的事件变成立即可交付的目标帧，不等待下一次 data 或 close。
    pub(super) fn feed(&mut self, bytes: &[u8]) -> Result<String, Fault> {
        let mut output = String::new();
        for &b in bytes {
            if self.terminal {
                break;
            }
            if std::mem::take(&mut self.after_cr) && b == b'\n' {
                continue;
            }
            if matches!(b, b'\r' | b'\n') {
                self.after_cr = b == b'\r';
                if self.frame.is_empty() || self.frame.ends_with(b"\n") {
                    self.dispatch(&mut output)?;
                    self.frame.clear();
                    continue;
                }
                self.frame.push(b'\n');
            } else {
                self.frame.push(b);
            }
            if self.frame.len() > MAX_EVENT {
                return Err(Fault::new(
                    "单个流事件超过 8 MiB",
                    "stream event exceeds 8 MiB",
                ));
            }
        }
        Ok(output)
    }
    /// 逐行提取 data；注释、重连参数和空事件不是提供方的 JSON，不能送入语义层。
    fn dispatch(&mut self, output: &mut String) -> Result<(), Fault> {
        let mut frame = std::str::from_utf8(&self.frame).map_err(|_| malformed())?;
        if std::mem::take(&mut self.first) {
            frame = frame.trim_start_matches('\u{feff}');
        }
        let mut data = String::new();
        for line in frame.lines() {
            let (key, value) = line.split_once(':').unwrap_or((line, ""));
            if key == "data" {
                data.push_str(value.strip_prefix(' ').unwrap_or(value));
                data.push('\n');
            }
        }
        let data = data.trim();
        if data.is_empty() {
            return Ok(());
        }
        if data == "[DONE]" {
            if self.source != F::OpenAiChat {
                return Err(malformed());
            }
            self.terminal = true;
            let finishes = std::mem::take(&mut self.chat_finishes);
            if finishes.is_empty() {
                return Ok(());
            }
            return self.translate(&serde_json::json!({"choices":finishes}), output);
        }
        self.remaining = self.remaining.checked_sub(data.len()).ok_or_else(budget)?;
        let mut event: Value = serde_json::from_str(data).map_err(|_| malformed())?;
        if !event.is_object() {
            return Err(malformed());
        }
        self.terminal = match self.source {
            F::OpenAiChat => {
                if let Some(rows) = event.get_mut("choices").and_then(Value::as_array_mut) {
                    for row in rows {
                        if row["finish_reason"].is_string() {
                            self.chat_finishes.push(serde_json::json!({"index":row["index"],"delta":{},"finish_reason":row["finish_reason"].take()}));
                        }
                    }
                }
                false
            }
            F::AnthropicMessages => {
                matches!(event["type"].as_str(), Some("message_stop" | "error"))
            }
            F::OpenAiResponses => matches!(
                event["type"].as_str(),
                Some("response.completed" | "response.incomplete" | "response.failed" | "error")
            ),
        } || event.get("error").is_some();
        if self.source == F::AnthropicMessages && self.target == F::OpenAiChat {
            if event["type"] == "message_delta" && event["delta"]["stop_reason"].is_string() {
                self.message_finish = Some(event);
                return Ok(());
            }
            if event["type"] == "message_stop"
                && let Some(delta) = self.message_finish.take()
            {
                self.translate(&delta, output)?;
            }
        }
        self.translate(&event, output)
    }
    /// 已确认的源事件才进入语义层，目标端的终态不能早于源端的完成标志。
    fn translate(&mut self, event: &Value, output: &mut String) -> Result<(), Fault> {
        let events = self
            .engine
            .translate_event(&mut self.state, self.source, self.target, event)
            .map_err(|_| malformed())?;
        render(self.target, events, output)
    }
    /// 运输层正常 EOF 不代表协议完整。只在源协议已结束且没有半帧时合成剩余的结束事件。
    pub(super) fn finish(mut self) -> Result<String, Fault> {
        if !self.frame.is_empty() || !self.terminal {
            return Err(Fault::new(
                "事件流没有完整结束",
                "incomplete upstream event stream",
            ));
        }
        let mut output = String::new();
        let events = self
            .engine
            .finish_stream(&mut self.state, self.target)
            .map_err(|_| malformed())?;
        render(self.target, events, &mut output)?;
        if self.target == F::OpenAiChat && !self.state.errored {
            std::fmt::Write::write_str(&mut Bounded(&mut output), "data: [DONE]\n\n")
                .map_err(|_| budget())?;
        }
        Ok(output)
    }
    /// 同一条错误收尾既交给 open 备用，也在收到 abort 时直接返回，绝不伪造正常结束。
    pub(super) fn abort(&self) -> &'static str {
        match self.target {
            F::AnthropicMessages => {
                "event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"api_error\",\"message\":\"stream conversion aborted\"}}\n\n"
            }
            F::OpenAiChat => {
                "data: {\"error\":{\"type\":\"api_error\",\"message\":\"stream conversion aborted\"}}\n\n"
            }
            F::OpenAiResponses => {
                "event: error\ndata: {\"type\":\"error\",\"code\":\"stream_interrupted\",\"message\":\"stream conversion aborted\",\"param\":null,\"sequence_number\":0}\n\n"
            }
        }
    }
}
/// 不把可能带敏感字段的上游事件或第三方库诊断复制到协议原因或 stderr。
fn malformed() -> Fault {
    Fault::new("无法转换流事件", "invalid stream event")
}
/// 不暴露内容或长度，工作进程在超限后直接输出目标协议的错误结尾。
fn budget() -> Fault {
    Fault::new(
        "流转换超过 1 MiB 语义预算",
        "stream conversion exceeds the semantic budget",
    )
}
/// 直接限制渲染目的地，而不是等终态快照与多份 done 事件全部写进字符串后才检查。
struct Bounded<'a>(&'a mut String);
impl std::fmt::Write for Bounded<'_> {
    /// 格式化器可分多次写片段，总是先检查余量再分配目标空间。
    fn write_str(&mut self, chunk: &str) -> std::fmt::Result {
        if chunk.len() > SEMANTIC_BYTES.saturating_sub(self.0.len()) {
            return Err(std::fmt::Error);
        }
        self.0.push_str(chunk);
        Ok(())
    }
}
/// Chat 使用匿名 data 事件，其余两种协议都以 JSON 的 type 命名 SSE 事件。
fn render(target: F, events: Vec<Value>, output: &mut String) -> Result<(), Fault> {
    use std::fmt::Write as _;
    let mut output = Bounded(output);
    for event in events {
        if target != F::OpenAiChat {
            let name = event["type"].as_str().ok_or_else(malformed)?;
            if name.contains(['\r', '\n']) {
                return Err(malformed());
            }
            writeln!(output, "event: {name}").map_err(|_| budget())?;
        }
        write!(output, "data: {event}\n\n").map_err(|_| budget())?;
    }
    Ok(())
}

/// 独立于传输分块方式与源协议的尾部检查。
#[cfg(test)]
mod tests {
    use super::*;
    /// 六条转换路径逐字节输入；SSE 多行、三种换行、BOM 和 UTF-8 的分割都不改变事件结果。
    #[test]
    fn byte_fragmented_native_events_have_identical_target_framing() {
        let all: Value =
            serde_json::from_str(include_str!("../tests/fixtures/protocols.json")).unwrap();
        let formats = [F::OpenAiChat, F::OpenAiResponses, F::AnthropicMessages];
        for source in formats {
            for target in formats {
                if source == target {
                    continue;
                }
                let mut input = String::from("\u{feff}:ignored\r\n\r\n");
                for (i, e) in all[source.to_string()]["events"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .enumerate()
                {
                    let lines = serde_json::to_string_pretty(e).unwrap();
                    let eol = ["\n", "\r", "\r\n"][i % 3];
                    for l in lines.lines() {
                        input.push_str("data:");
                        input.push_str(l);
                        input.push_str(eol);
                    }
                    input.push_str(eol);
                }
                if source == F::OpenAiChat {
                    input.push_str("data: [DONE]\n\n");
                }
                let run = |step| {
                    let mut s = Events::new(source, target);
                    let mut out = String::new();
                    for b in input.as_bytes().chunks(step) {
                        out += &s.feed(b).map_err(|e| e.wire).unwrap();
                    }
                    out += &s.finish().map_err(|e| e.wire).unwrap();
                    out
                };
                let out = run(1);
                assert_eq!(out, run(input.len()));
                assert!(out.contains("hello λ") && out.ends_with("\n\n"));
            }
        }
    }
    /// 输入不完整、非法或无界时不得补出成功的协议结束事件。
    #[test]
    fn malformed_partial_or_oversized_events_fail_closed() {
        let create = || Events::new(F::OpenAiChat, F::AnthropicMessages);
        for bad in [
            b"data: {x}\n\n".as_slice(),
            b"data:\xff\n\n",
            b"data: []\n\n",
        ] {
            assert!(create().feed(bad).is_err());
        }
        let mut partial = create();
        assert!(partial.feed(b"data: {\"choices\":[").is_ok());
        assert!(partial.finish().is_err());
        assert!(create().feed(&vec![b' '; MAX_EVENT + 1]).is_err());
        let mut no_stop = create();
        assert!(no_stop.feed(b"data: {\"choices\":[]}\n\n").is_ok());
        assert!(no_stop.finish().is_err());
    }
}
