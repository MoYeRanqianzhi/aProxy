//! SSE 负责切事件，switchyard 负责语义。两份状态都属于一条已经 open 的流。
use crate::config::Fault;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use switchyard_translation::{
    LlmResponseChunk, StreamTranslationState, TranslationEngine, WireFormat as F,
};

/// 单个尚未结束的 SSE 事件上限，与整条流的长度无关。
const MAX_EVENT: usize = 8 * 1024 * 1024;

/// Messages 的用量可分多次发送，保留已知计数；OpenAI 的输入计数则包含缓存部分。
#[derive(Default)]
pub(super) struct UsageTotals {
    /// 输入计数，是否包含缓存取决于源协议。
    input: Option<u64>,
    /// 最新的累计输出，不把不同事件的累计值再次相加。
    output: Option<u64>,
    /// 从缓存读取的输入量。
    read: u64,
    /// 写入缓存的输入量。
    write: u64,
}
impl UsageTotals {
    /// 在送入编解码器之前检查它会重组的合计；允许缺少可选计数，不把非法数值解释为零。
    pub(super) fn check(&mut self, source: F, usage: &Value) -> Result<(), Fault> {
        if usage.is_null() {
            return Ok(());
        }
        let invalid = || Fault::new("上游用量计数无效", "invalid upstream usage counts");
        if !usage.is_object() {
            return Err(invalid());
        }
        let number = |v: &Value| {
            if v.is_null() {
                Ok(None)
            } else {
                v.as_u64().map(Some).ok_or_else(invalid)
            }
        };
        let (input, output, read, write) = match source {
            F::AnthropicMessages => (
                &usage["input_tokens"],
                &usage["output_tokens"],
                &usage["cache_read_input_tokens"],
                &usage["cache_creation_input_tokens"],
            ),
            F::OpenAiChat => {
                let details = &usage["prompt_tokens_details"];
                (
                    &usage["prompt_tokens"],
                    &usage["completion_tokens"],
                    &details["cached_tokens"],
                    details
                        .get("cache_write_tokens")
                        .unwrap_or(&details["cache_creation_tokens"]),
                )
            }
            F::OpenAiResponses => {
                let prompt = &usage["prompt_tokens_details"];
                let details = usage.get("input_tokens_details").unwrap_or(prompt);
                // 读取缓存量按整张明细表取别名，写入量则按各表中的计数字段回退。
                let write = [
                    &usage["input_tokens_details"]["cache_write_tokens"],
                    &prompt["cache_write_tokens"],
                    &prompt["cache_creation_tokens"],
                ];
                (
                    usage.get("input_tokens").unwrap_or(&usage["prompt_tokens"]),
                    usage
                        .get("output_tokens")
                        .unwrap_or(&usage["completion_tokens"]),
                    &details["cached_tokens"],
                    write
                        .into_iter()
                        .find(|v| !v.is_null())
                        .unwrap_or(&Value::Null),
                )
            }
        };
        if source != F::AnthropicMessages {
            *self = Self::default();
        }
        self.input = number(input)?.or(self.input);
        self.output = number(output)?.or(self.output);
        self.read = number(read)?.unwrap_or(self.read);
        self.write = number(write)?.unwrap_or(self.write);
        let cached = self.read.checked_add(self.write).ok_or_else(invalid)?;
        let input = if source == F::AnthropicMessages {
            self.input
                .unwrap_or(0)
                .checked_add(cached)
                .ok_or_else(invalid)?
        } else {
            if self.input.is_some_and(|n| n < cached) {
                return Err(invalid());
            }
            self.input.unwrap_or(cached)
        };
        let sum = input
            .checked_add(self.output.unwrap_or(0))
            .ok_or_else(invalid)?;
        if let Some(total) = number(&usage["total_tokens"])?
            && (total < sum || self.input.is_some() && self.output.is_some() && total != sum)
        {
            return Err(invalid());
        }
        Ok(())
    }
}

/// 签名到达时，之前的推理文本也要进入两份内嵌 JSON；不保留文本的第二份拷贝。
#[derive(Default)]
struct ReasoningSize {
    /// 两份签名载荷中的文本开销，首次收到签名前暂不扣费。
    signed_text: usize,
    /// 有签名后不再输出原生加密载荷，后续文本直接计费。
    signed: bool,
    /// 原生加密载荷是替换而非增量。
    encrypted: usize,
}
/// 当前事件跨协议转换所需的字节边界、语义状态和终止标记。
pub(super) struct Events {
    /// 上游返回的协议。
    source: F,
    /// 客户端等待的协议；同协议不经过这里。
    target: F,
    /// 默认注册的三个协议编解码器，每条活跃流复用一份。
    engine: TranslationEngine,
    /// 源协议尚未成套的用量也须在下一次编码前检查。
    counts: UsageTotals,
    /// 工具调用、usage 与结束原因在多个源事件间关联。
    state: StreamTranslationState,
    /// 只对有语义的完整事件扣费，注释与网络分片不改变预算。
    remaining: usize,
    /// 已输出的 Responses 增量在同一次终态回信中必须重现的最少字节（含双层 JSON 转义）。
    snapshot_min: usize,
    /// 工具 ID 可被后续增量替换，只计最后一个值。
    call_ids: BTreeMap<usize, usize>,
    /// 推理块分别保留签名或原生加密载荷。
    reasoning_sizes: BTreeMap<usize, ReasoningSize>,
    /// 只根据已经交给客户端的完整回信推进；被拒的事件不能占用错误帧的序号。
    next_sequence: u64,
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
    /// Chat 客户端显式要求把用量放在空 choices 块，而不是工具完成块上。
    include_usage: bool,
    /// 最近一个有真实用量的目标头与用量；不把生成内容带入该块。
    usage_chunk: Option<Value>,
    /// Messages 的 stop_reason 提示原因而不保证完整；转成 Chat 时，须等 message_stop 再把这一帧交给编码器。
    message_finish: Option<Value>,
    /// Responses 的 call_id 对应已经向 Messages 客户端打开的块索引。
    open_tools: BTreeMap<String, u64>,
    /// 已按源项终点关闭的块，不重复输出编解码器在流末补的 stop。
    closed_tools: BTreeSet<u64>,
    /// 两个协议的终态快照并不共享同一组原因，暂存可映射的 Responses 原因直到目标终态。
    incomplete_reason: Option<&'static str>,
}
impl Events {
    /// 每次 open 初始化，不把上一条流的输出索引或 usage 带到下一条。
    pub(super) fn new(source: F, target: F, include_usage: bool, budget: usize) -> Self {
        Self {
            source,
            target,
            engine: TranslationEngine::default(),
            counts: UsageTotals::default(),
            state: Default::default(),
            remaining: budget,
            snapshot_min: 0,
            call_ids: Default::default(),
            reasoning_sizes: Default::default(),
            next_sequence: 0,
            frame: Vec::new(),
            after_cr: false,
            first: true,
            terminal: false,
            chat_finishes: Vec::new(),
            include_usage,
            usage_chunk: None,
            message_finish: None,
            open_tools: Default::default(),
            closed_tools: Default::default(),
            incomplete_reason: None,
        }
    }
    /// 将已经完整的事件变成立即可交付的目标帧，不等待下一次 data 或 close。
    pub(super) fn feed(
        &mut self,
        bytes: &[u8],
        capacity: usize,
    ) -> Result<String, (String, Fault)> {
        let mut output = Bounded::new(capacity);
        let result = self.read_events(bytes, &mut output);
        if let Some(next) = output.next_sequence {
            self.next_sequence = next;
        }
        match result {
            Ok(()) => Ok(output.text),
            Err(fault) => Err((output.text, fault)),
        }
    }
    /// 以源事件而非管道消息为交付边界。单个失败事件的部分渲染撤回，前面完整事件仍可交付。
    fn read_events(&mut self, bytes: &[u8], output: &mut Bounded) -> Result<(), Fault> {
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
                    let start = (output.text.len(), output.next_sequence);
                    if let Err(fault) = self.dispatch(output) {
                        output.text.truncate(start.0);
                        output.next_sequence = start.1;
                        return Err(fault);
                    }
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
        Ok(())
    }
    /// 逐行提取 data；注释、重连参数和空事件不是提供方的 JSON，不能送入语义层。
    fn dispatch(&mut self, output: &mut Bounded) -> Result<(), Fault> {
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
            return self.translate(serde_json::json!({"choices":finishes}), output);
        }
        self.remaining = self.remaining.checked_sub(data.len()).ok_or_else(budget)?;
        let mut event: Value = serde_json::from_str(data).map_err(|_| malformed())?;
        if !event.is_object() {
            return Err(malformed());
        }
        // 只看源协议正式的错误形状。私有提供方错误页和诊断永不直接进入客户端协议。
        let failed = match self.source {
            F::OpenAiChat => event["error"].is_object(),
            F::AnthropicMessages => event["type"] == "error",
            F::OpenAiResponses => {
                matches!(event["type"].as_str(), Some("error" | "response.failed"))
            }
        };
        if failed {
            return Err(Fault::new("上游事件流报告失败", "upstream stream failed"));
        }
        let usage = match self.source {
            F::OpenAiChat => &event["usage"],
            F::AnthropicMessages if event["type"] == "message_start" => &event["message"]["usage"],
            F::AnthropicMessages => &event["usage"],
            F::OpenAiResponses => &event["response"]["usage"],
        };
        self.counts.check(self.source, usage)?;
        visible_refusal(self.source, &mut event);
        if self.source == F::OpenAiResponses && event["type"] == "response.incomplete" {
            self.incomplete_reason = Some(match event["response"]["incomplete_details"]["reason"]
                .as_str()
            {
                Some("content_filter") => "content_filter",
                Some("max_output_tokens") => "max_output_tokens",
                _ => return Err(malformed()),
            });
            // completed 的译码会对齐快照与已有增量并读取 usage；目标终因在渲染之前单独还原。
            event["type"] = "response.completed".into();
        }
        self.terminal = match self.source {
            F::OpenAiChat => {
                if let Some(rows) = event.get_mut("choices").and_then(Value::as_array_mut) {
                    for row in rows {
                        // Chat 工具名按片到达，到结束原因才释放；不能等目标编码器真正开块才计费。
                        if self.target == F::OpenAiResponses
                            && let Some(calls) = row["delta"]["tool_calls"].as_array()
                        {
                            for name in calls.iter().filter_map(|c| c["function"]["name"].as_str())
                            {
                                self.snapshot_min += nested_string_bytes(name) * 3;
                            }
                            if self.snapshot_min > output.capacity {
                                return Err(budget());
                            }
                        }
                        if row["finish_reason"].is_string() {
                            self.incomplete_reason = match row["finish_reason"].as_str() {
                                Some("length") => Some("max_output_tokens"),
                                Some("content_filter") => Some("content_filter"),
                                _ => self.incomplete_reason,
                            };
                            self.chat_finishes.push(serde_json::json!({"index":row["index"],"delta":{},"finish_reason":row["finish_reason"].take()}));
                        }
                    }
                }
                // Chat 只有 error 对象表示错误；某些兼容服务会在普通块写 error:null。
                // 宽松的源解码器仅检查键是否存在，不能把这个占位误译成终态。
                if let Some(obj) = event.as_object_mut() {
                    obj.remove("error");
                }
                false
            }
            F::AnthropicMessages => event["type"] == "message_stop",
            F::OpenAiResponses => matches!(
                event["type"].as_str(),
                Some("response.completed" | "response.incomplete")
            ),
        };
        if self.source == F::AnthropicMessages && self.target == F::OpenAiChat {
            if event["type"] == "message_delta" && event["delta"]["stop_reason"].is_string() {
                self.message_finish = Some(event);
                return Ok(());
            }
            if event["type"] == "message_stop"
                && let Some(delta) = self.message_finish.take()
            {
                self.translate(delta, output)?;
            }
        }
        self.translate(event, output)
    }
    /// 已确认的源事件才进入语义层，目标端的终态不能早于源端的完成标志。
    fn translate(&mut self, event: Value, output: &mut Bounded) -> Result<(), Fault> {
        // 一份完成快照可解成很多中间块；目标块各自复制模型、ID 等上下文。
        // 保留原生输出类型的校验，但逐个编码、计费，绝不先收集整份放大后的目标事件数组。
        let closing_tool = (self.source == F::OpenAiResponses
            && self.target == F::AnthropicMessages
            && event["type"] == "response.output_item.done"
            && event["item"]["type"] == "function_call")
            .then(|| event["item"]["call_id"].as_str().map(str::to_owned))
            .flatten();
        let decoded = self
            .engine
            .decode_stream_event(&mut self.state, self.source, event)
            .map_err(|_| malformed())?;
        let (original, chunks) = decoded.into_parts();
        if let Some(original) = original {
            let (source, raw) = original.into_parts();
            let check =
                switchyard_translation::LlmResponseStreamEvent::preserved(source, raw, Vec::new());
            let checked = self
                .engine
                .encode_stream_event(&mut self.state, self.target, check)
                .map_err(|_| malformed())?;
            self.render_events(checked, output)?;
        }
        for chunk in chunks {
            if self.target == F::OpenAiResponses {
                self.charge_retained(&chunk, output.capacity)?;
            }
            let encoded = self
                .engine
                .encode_stream_event(&mut self.state, self.target, chunk.into())
                .map_err(|_| malformed())?;
            self.render_events(encoded, output)?;
        }
        if let Some(id) = closing_tool
            && let Some(index) = self.open_tools.remove(&id)
        {
            self.closed_tools.insert(index);
            render(
                self.target,
                vec![serde_json::json!({"type":"content_block_stop","index":index})],
                output,
            )?;
        }
        Ok(())
    }
    /// 在目标编码器保留数据前按终态副本计费。工具参数可能尚未生成任何可见增量。
    fn charge_retained(&mut self, chunk: &LlmResponseChunk, capacity: usize) -> Result<(), Fault> {
        match chunk {
            LlmResponseChunk::ToolCallDelta {
                index,
                id,
                name,
                arguments_delta,
            } => {
                if let Some(id) = id {
                    let size = nested_string_bytes(id) * 2;
                    self.snapshot_min -= self.call_ids.insert(*index, size).unwrap_or(0);
                    self.snapshot_min += size;
                }
                if self.source != F::OpenAiChat
                    && let Some(name) = name
                {
                    self.snapshot_min += nested_string_bytes(name) * 3;
                }
                if let Some(args) = arguments_delta {
                    self.snapshot_min += nested_string_bytes(args) * 3;
                }
            }
            LlmResponseChunk::ReasoningDelta { index, text }
            | LlmResponseChunk::ReasoningDetailsDelta { index, text, .. } => {
                let size = self.reasoning_sizes.entry(*index).or_default();
                let extra = signed_bytes(text) * 2;
                size.signed_text += extra;
                if size.signed {
                    self.snapshot_min += extra;
                }
                if let LlmResponseChunk::ReasoningDetailsDelta { details, .. } = chunk {
                    for d in details {
                        if d["type"] == "anthropic.signature_delta"
                            && let Some(sig) = d["signature"].as_str()
                        {
                            if !size.signed {
                                self.snapshot_min -= size.encrypted;
                                self.snapshot_min += size.signed_text;
                                size.signed = true;
                            }
                            self.snapshot_min += signed_bytes(sig) * 2;
                        }
                    }
                    if !size.signed
                        && let Some(encrypted) = details
                            .iter()
                            .find_map(|d| match d["type"].as_str() {
                                Some("reasoning.encrypted") => d["data"].as_str(),
                                Some("reasoning") => d["encrypted_content"].as_str(),
                                _ => None,
                            })
                            .filter(|s| !s.is_empty())
                    {
                        self.snapshot_min -= size.encrypted;
                        size.encrypted = nested_string_bytes(encrypted) * 2;
                        self.snapshot_min += size.encrypted;
                    }
                }
            }
            _ => (),
        }
        if self.snapshot_min > capacity {
            Err(budget())
        } else {
            Ok(())
        }
    }
    /// 按客户端的选项调整目标帧；最后只发一个独立用量块，且不能再带工具或文本。
    fn render_events(&mut self, mut events: Vec<Value>, output: &mut Bounded) -> Result<(), Fault> {
        if self.target == F::OpenAiResponses {
            // 文本和推理文本出现在四个 done/终态位置，工具参数出现在三个位置。
            // 在已确定一行装不下时立即结束，而非先在 finish 中构造全部重复快照。
            for e in &events {
                let (value, copies) = match e["type"].as_str() {
                    Some(
                        "response.output_text.delta" | "response.reasoning_summary_text.delta",
                    ) => (&e["delta"], 4),
                    Some("response.created") => {
                        self.snapshot_min += ["id", "model"]
                            .iter()
                            .filter_map(|k| e["response"][k].as_str())
                            .map(nested_string_bytes)
                            .sum::<usize>();
                        continue;
                    }
                    Some("response.output_item.added") => {
                        // 即使所有字符串均为空，done 帧和最终 output 仍重复项的类型、状态和结构。
                        let tool = e["item"]["type"] == "function_call";
                        self.snapshot_min += if tool { 240 } else { 128 };
                        (&e["item"]["id"], if tool { 3 } else { 2 })
                    }
                    _ => continue,
                };
                if let Some(text) = value.as_str() {
                    self.snapshot_min = self
                        .snapshot_min
                        .saturating_add(nested_string_bytes(text).saturating_mul(copies));
                    if self.snapshot_min > output.capacity {
                        return Err(budget());
                    }
                }
            }
        }
        if self.source == F::OpenAiResponses && self.target == F::AnthropicMessages {
            events.retain(|e| {
                let Some(index) = e["index"].as_u64() else {
                    return true;
                };
                match e["type"].as_str() {
                    Some("content_block_start") if e["content_block"]["type"] == "tool_use" => {
                        if let Some(id) = e["content_block"]["id"].as_str() {
                            self.open_tools.insert(id.to_owned(), index);
                        }
                    }
                    Some("content_block_stop") => {
                        if self.closed_tools.remove(&index) {
                            return false;
                        }
                        self.open_tools.retain(|_, i| *i != index);
                    }
                    Some("content_block_delta") if self.closed_tools.contains(&index) => {
                        return false;
                    }
                    _ => (),
                }
                true
            });
        }
        if let Some(reason) = self.incomplete_reason {
            for e in &mut events {
                match self.target {
                    F::OpenAiChat if e["choices"][0]["finish_reason"].is_string() => {
                        e["choices"][0]["finish_reason"] = if reason == "content_filter" {
                            "content_filter"
                        } else {
                            "length"
                        }
                        .into()
                    }
                    F::AnthropicMessages
                        if e["type"] == "message_delta"
                            && e["delta"]["stop_reason"].is_string() =>
                    {
                        e["delta"]["stop_reason"] = if reason == "content_filter" {
                            "refusal"
                        } else {
                            "max_tokens"
                        }
                        .into()
                    }
                    F::OpenAiResponses
                        if matches!(
                            e["type"].as_str(),
                            Some("response.completed" | "response.incomplete")
                        ) =>
                    {
                        e["type"] = "response.incomplete".into();
                        e["response"]["status"] = "incomplete".into();
                        e["response"]["incomplete_details"] = serde_json::json!({"reason":reason});
                    }
                    _ => (),
                }
            }
        }
        if self.include_usage && self.target == F::OpenAiChat {
            events.retain_mut(|e| {
                if e["usage"].is_object() {
                    let mut chunk = e.clone();
                    chunk["choices"] = serde_json::json!([]);
                    self.usage_chunk = Some(chunk);
                    if let Some(o) = e.as_object_mut() {
                        o.remove("usage");
                    }
                    return !e["choices"].as_array().is_some_and(Vec::is_empty);
                }
                true
            });
        }
        render(self.target, events, output)
    }
    /// 运输层正常 EOF 不代表协议完整。只在源协议已结束且没有半帧时合成剩余的结束事件。
    pub(super) fn finish(&mut self, capacity: usize) -> Result<String, Fault> {
        if !self.frame.is_empty() || !self.terminal {
            return Err(Fault::new(
                "事件流没有完整结束",
                "incomplete upstream event stream",
            ));
        }
        let mut output = Bounded::new(capacity);
        if self.snapshot_min > capacity {
            return Err(budget());
        }
        let events = self
            .engine
            .finish_stream(&mut self.state, self.target)
            .map_err(|_| malformed())?;
        self.render_events(events, &mut output)?;
        if self.target == F::OpenAiChat && !self.state.errored {
            if let Some(usage) = self.usage_chunk.take() {
                render(self.target, vec![usage], &mut output)?;
            }
            std::fmt::Write::write_str(&mut output, "data: [DONE]\n\n").map_err(|_| budget())?;
        }
        Ok(output.text)
    }
    /// 已提交后不能更改 HTTP 状态，原因使用本地固定文本而非上游响应。
    pub(super) fn ending(&self, fault: &Fault) -> String {
        self.error_event(match fault.wire {
            LIMIT_REASON => LIMIT_REASON,
            "upstream stream failed" => fault.wire,
            _ => "stream conversion aborted",
        })
    }
    /// 活 worker 知道已交付的位置。附带的 error 对象让 SDK 在聚合生命周期前就报告错误。
    fn error_event(&self, message: &str) -> String {
        let error = serde_json::json!({"type":"api_error","message":message});
        match self.target {
            F::AnthropicMessages => format!(
                "event: error\ndata: {}\n\n",
                serde_json::json!({"type":"error","error":error})
            ),
            F::OpenAiChat => format!("data: {}\n\n", serde_json::json!({"error":error})),
            F::OpenAiResponses => format!(
                "event: error\ndata: {}\n\n",
                serde_json::json!({"type":"error","code":"stream_interrupted","message":message,"param":null,"sequence_number":self.next_sequence,"error":error})
            ),
        }
    }
    /// 传输中断时不伪造成功事件或结束原因。
    pub(super) fn abort(&self) -> String {
        self.error_event("stream conversion aborted")
    }
    /// 保存的回退字节无法预知哪个事件是最后一帧；不捏造 Responses 序号，用 SDK 的错误信封。
    pub(super) fn fallback(&self) -> String {
        if self.target == F::OpenAiResponses {
            "data: {\"error\":{\"type\":\"api_error\",\"message\":\"stream conversion aborted\"}}\n\n".into()
        } else {
            self.abort()
        }
    }
}
/// 把拒绝的可见文字放在编码器认识的文本位置；只改规范列出的字段，不递归修改工具参数。
fn visible_refusal(source: F, event: &mut Value) {
    if source == F::OpenAiChat {
        if let Some(rows) = event.get_mut("choices").and_then(Value::as_array_mut) {
            for row in rows {
                if let Some(delta) = row.get_mut("delta").and_then(Value::as_object_mut)
                    && let Some(Value::String(refusal)) = delta.remove("refusal")
                {
                    let mut text = delta
                        .get("content")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned();
                    text.push_str(&refusal);
                    delta.insert("content".into(), text.into());
                }
            }
        }
    } else if source == F::OpenAiResponses {
        match event["type"].as_str() {
            Some("response.refusal.delta") => event["type"] = "response.output_text.delta".into(),
            Some("response.refusal.done") => {
                event["type"] = "response.output_text.done".into();
                event["text"] = event["refusal"].take();
            }
            _ => (),
        }
        if let Some(part) = event.get_mut("part") {
            visible_part(part);
        }
        if let Some(parts) = event
            .pointer_mut("/item/content")
            .and_then(Value::as_array_mut)
        {
            parts.iter_mut().for_each(visible_part);
        }
        if let Some(items) = event
            .pointer_mut("/response/output")
            .and_then(Value::as_array_mut)
        {
            for item in items {
                if let Some(parts) = item.get_mut("content").and_then(Value::as_array_mut) {
                    parts.iter_mut().for_each(visible_part);
                }
            }
        }
    }
}
/// Responses 增量与最终快照使用相同的内容位置，让编解码器按索引去重。
fn visible_part(part: &mut Value) {
    if part["type"] == "refusal" && part["refusal"].is_string() {
        part["text"] = part["refusal"].take();
        part["type"] = "output_text".into();
    }
}
/// 不把可能带敏感字段的上游事件或第三方库诊断复制到协议原因或 stderr。
fn malformed() -> Fault {
    Fault::new("无法转换流事件", "invalid stream event")
}
/// 客户端可区分限制与网络中断，不披露原始正文。
pub(super) const LIMIT_REASON: &str = "response exceeds the configured limit";
/// 输入与输出限制都按协议错误结束，不计入进程熔断。
fn budget() -> Fault {
    Fault::new("转换响应超过配置的限制", LIMIT_REASON)
}
/// serde_json 在字符串中只转义控制字符、双引号和反斜杠；其余 UTF-8 字节原样写出。
/// 两层字符串（目标事件放入回信 data）时，第一层引入的反斜杠也须计费。
fn string_cost<const NESTED: bool>(text: &str) -> usize {
    text.bytes()
        .map(|b| match b {
            b'"' | b'\\' => {
                if NESTED {
                    4
                } else {
                    2
                }
            }
            b'\n' | b'\r' | b'\t' | 8 | 12 => {
                if NESTED {
                    3
                } else {
                    2
                }
            }
            0..=31 => {
                if NESTED {
                    7
                } else {
                    6
                }
            }
            _ => 1,
        })
        .sum()
}
/// 已格式化 SSE 或 JSON 放入 data 时的实际字节数，不含字符串两端引号。
pub(super) fn escaped_bytes(text: &str) -> usize {
    string_cost::<false>(text)
}
/// 原始增量在嵌套 JSON 字符串中的实际字节数。
fn nested_string_bytes(text: &str) -> usize {
    string_cost::<true>(text)
}
/// 签名载荷本身是 JSON 字符串，比普通文本多一层转义（包括 JSON 转义符本身）。
fn signed_bytes(text: &str) -> usize {
    text.bytes()
        .map(|b| match b {
            b'"' | b'\\' => 8,
            b'\n' | b'\r' | b'\t' | 8 | 12 => 5,
            0..=31 => 9,
            _ => 1,
        })
        .sum()
}
/// 只为能写入当前回信的字节分配空间，逐笔按外层序列化后的长度扣费。
struct Bounded {
    /// 尚未转义的目标 SSE 字节。
    text: String,
    /// 单独保存初始容量，供将来终态回信的下界检查使用。
    capacity: usize,
    /// 当前回信余量。
    left: usize,
    /// 当前回信最后一个完整 Responses 事件之后的位置，回信实际交付才推进流状态。
    next_sequence: Option<u64>,
}
impl Bounded {
    /// 从实际序号和行长算出的可用字节开始计费。
    fn new(capacity: usize) -> Self {
        Self {
            text: String::new(),
            capacity,
            left: capacity,
            next_sequence: None,
        }
    }
}
impl std::fmt::Write for Bounded {
    /// 序列化可以分多次写片段；拒绝本次写入后不再扩容。
    fn write_str(&mut self, chunk: &str) -> std::fmt::Result {
        self.left = self
            .left
            .checked_sub(escaped_bytes(chunk))
            .ok_or(std::fmt::Error)?;
        self.text.push_str(chunk);
        Ok(())
    }
}
/// Chat 使用匿名 data 事件，其余两种协议都以 JSON 的 type 命名 SSE 事件。
fn render(target: F, events: Vec<Value>, output: &mut Bounded) -> Result<(), Fault> {
    use std::fmt::Write as _;
    for event in events {
        if target != F::OpenAiChat {
            let name = event["type"].as_str().ok_or_else(malformed)?;
            if name.contains(['\r', '\n']) {
                return Err(malformed());
            }
            writeln!(output, "event: {name}").map_err(|_| budget())?;
        }
        write!(output, "data: {event}\n\n").map_err(|_| budget())?;
        if target == F::OpenAiResponses
            && let Some(seq) = event["sequence_number"].as_u64()
        {
            output.next_sequence = Some(seq.saturating_add(1));
        }
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
                    let mut s = Events::new(source, target, false, 64 << 20);
                    let mut out = String::new();
                    for b in input.as_bytes().chunks(step) {
                        out += &s.feed(b, 64 << 20).map_err(|(_, e)| e.wire).unwrap();
                    }
                    out += &s.finish(64 << 20).map_err(|e| e.wire).unwrap();
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
        let create = || Events::new(F::OpenAiChat, F::AnthropicMessages, false, 64 << 20);
        for bad in [
            b"data: {x}\n\n".as_slice(),
            b"data:\xff\n\n",
            b"data: []\n\n",
        ] {
            assert!(create().feed(bad, 64 << 20).is_err());
        }
        let mut partial = create();
        assert!(partial.feed(b"data: {\"choices\":[", 64 << 20).is_ok());
        assert!(partial.finish(64 << 20).is_err());
        assert!(create().feed(&vec![b' '; MAX_EVENT + 1], 64 << 20).is_err());
        let mut no_stop = create();
        assert!(
            no_stop
                .feed(b"data: {\"choices\":[]}\n\n", 64 << 20)
                .is_ok()
        );
        assert!(no_stop.finish(64 << 20).is_err());
    }
    /// 六个方向都传足 4 MiB 正文而非用注释填充；逐块验增量并检查真正的协议终点。
    #[test]
    fn four_mib_of_small_deltas_completes_in_every_direction() {
        use serde_json::json;
        let corpus: Value =
            serde_json::from_str(include_str!("../tests/fixtures/protocols.json")).unwrap();
        let formats = [F::OpenAiChat, F::OpenAiResponses, F::AnthropicMessages];
        let text = "x".repeat(4096);
        for source in formats {
            let start = &corpus[source.to_string()]["events"][0];
            let (delta, end) = match source {
                F::OpenAiChat => (
                    json!({"choices":[{"index":0,"delta":{"content":text}}]}),
                    json!({"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}),
                ),
                F::OpenAiResponses => (
                    json!({"type":"response.output_text.delta","output_index":0,"content_index":0,"delta":text}),
                    json!({"type":"response.completed","response":{"output":[]}}),
                ),
                F::AnthropicMessages => (
                    json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":text}}),
                    json!({"type":"message_stop"}),
                ),
            };
            let delta = format!("data: {delta}\n\n");
            for target in formats.into_iter().filter(|t| *t != source) {
                let mut stream = Events::new(source, target, false, 64 << 20);
                let mut feed = |s: &str| {
                    stream
                        .feed(s.as_bytes(), 64 << 20)
                        .map_err(|(_, f)| f.wire)
                        .unwrap()
                };
                feed(&format!("data: {start}\n\n"));
                for _ in 0..1024 {
                    assert!(feed(&delta).contains(&text), "{source} -> {target}");
                }
                let mut tail = feed(&format!("data: {end}\n\n"));
                if source == F::OpenAiChat {
                    tail.push_str(&feed("data: [DONE]\n\n"));
                }
                tail.push_str(&stream.finish(64 << 20).map_err(|f| f.wire).unwrap());
                assert!(!tail.contains("event: error"));
                assert!(
                    tail.contains(match target {
                        F::OpenAiChat => "data: [DONE]",
                        F::AnthropicMessages => "event: message_stop",
                        F::OpenAiResponses => "event: response.completed",
                    }),
                    "{source} -> {target}"
                );
            }
        }
    }
    /// 比较标准序列化器的实际字节，覆盖内外两层转义以及 UTF-8 多字节字符。
    #[test]
    fn escaped_sizes_match_wire_serialization() {
        let text: String = (0..=127).map(char::from).chain("字λ".chars()).collect();
        let once = serde_json::to_string(&text).unwrap();
        assert_eq!(escaped_bytes(&text), once.len() - 2);
        assert_eq!(
            nested_string_bytes(&text),
            serde_json::to_string(&once[1..once.len() - 1])
                .unwrap()
                .len()
                - 2
        );
    }
    /// 不等终态才发现签名、延迟工具参数和一组小项不能放进同一行。
    #[test]
    fn retained_fields_exhaust_the_snapshot_before_close() {
        use serde_json::json;
        let text = "x".repeat(300);
        for (source, make) in [
            (F::AnthropicMessages, 0),
            (F::OpenAiChat, 1),
            (F::OpenAiChat, 2),
            (F::AnthropicMessages, 3),
            (F::AnthropicMessages, 4),
        ] {
            let mut stream = Events::new(source, F::OpenAiResponses, false, 1 << 20);
            let mut fault = None;
            for i in 0..24 {
                let event = match make {
                    0 => {
                        json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":text}})
                    }
                    1 => {
                        json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":text}}]}}]})
                    }
                    2 => {
                        json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":text}}]}}]})
                    }
                    3 => {
                        json!({"type":"content_block_start","index":i,"content_block":{"type":"tool_use","id":format!("{text}{i}"),"name":"t","input":{}}})
                    }
                    _ => {
                        json!({"type":"content_block_start","index":i,"content_block":{"type":"tool_use","id":"x","name":"t","input":{}}})
                    }
                };
                if let Err((_, f)) = stream.feed(format!("data: {event}\n\n").as_bytes(), 4096) {
                    fault = Some(f.wire);
                    break;
                }
            }
            assert_eq!(fault, Some(LIMIT_REASON), "case {make}");
        }
    }
    /// 完成快照可一次提供多个文本片段；目标 ID 与模型只能逐块复制到预算之内。
    #[test]
    fn completed_parts_share_one_encoded_reply_budget() {
        use serde_json::json;
        let mut stream = Events::new(F::OpenAiResponses, F::OpenAiChat, false, 1 << 20);
        let start =
            json!({"type":"response.created","response":{"id":"fixture","model":"m".repeat(256)}});
        stream
            .feed(format!("data: {start}\n\n").as_bytes(), 2048)
            .map_err(|(_, f)| f.wire)
            .unwrap();
        let parts = vec![json!({"type":"output_text","text":"part"}); 12];
        let end = json!({"type":"response.completed","response":{"output":[{"type":"message","content":parts}]}});
        assert_eq!(
            stream
                .feed(format!("data: {end}\n\n").as_bytes(), 2048)
                .unwrap_err()
                .1
                .wire,
            LIMIT_REASON
        );
    }
}
