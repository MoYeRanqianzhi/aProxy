---
name: codex-stream-idle-timeout
description: 写 Codex 接入文档、排查「Codex 经 aProxy 约 5 分钟断开重连 / idle timeout waiting for SSE」时回想——Codex 的事件级空闲超时与 aProxy 注释心跳的关系、必须调的配置
metadata:
  type: reference
  scope: OpenAI Codex CLI（codex-cli 0.160.0，源码 tag rust-v0.160.0）经 aProxy 的流式请求；其他版本以其源码为准
  status: active
  last_verified: 2026-10-04
---

# Codex 的流空闲超时（2026-10-04 源码 + 黑盒实测）

**请求形态**：自定义 provider（`wire_api = "responses"`）发 `POST <base_url>/responses`，
`Accept: text/event-stream` + 请求体 `"stream": true`——`keepalive_trigger` 取 accept 或
any 都能让它进保活通道。

**超时机制**（源码）：
- `codex-rs/codex-api/src/sse/responses.rs` 的 SSE 循环对每次 `stream.next()` 套
  `timeout(idle_timeout, …)`，超时即报 `idle timeout waiting for SSE`——计的是**两次 SSE
  事件之间**，不是两次收到字节之间。
- 事件来自 `eventsource-stream 0.2.3`：注释行在 `add()` 里被丢弃（`RawEventLine::Comment(_) => {}`），
  只含注释的块在 `dispatch()` 里因 data 为空返回 None——**aProxy 的 `: keepalive` 注释
  永远不会让 `next()` 返回，续不了命**。
- 默认值（`codex-rs/model-provider-info/src/lib.rs`）：`stream_idle_timeout_ms = 300000`、
  `stream_max_retries = 5`（上限 100）、`request_max_retries = 4`（上限 100）。
- 超时后 `core/src/session/turn.rs` 在同一 turn 内重新发请求，最多 `stream_max_retries` 次，
  用完则本轮失败。

**黑盒实测**（aProxy v0.1.0 正式二进制、默认配置、隔离 CODEX_HOME + 假 key，mock 上游先持续 529）：
- A：`stream_idle_timeout_ms = 60000`、`stream_max_retries = 1` → 骨架头提交后整 60s 断开
  （其间 4 个注释心跳无效），`Reconnecting... 1/1`，重发后再 60s 断开，退出码 1；aProxy 两次
  都按计费保护中止上游。
- B：`stream_idle_timeout_ms = 86400000` → 只有注释心跳的等待持续 620s（超过默认 300s），
  第 11 次上游尝试成功后拿到完整结果，退出码 0，全程 1 次请求、0 次断开。

**对用户的要求**：Codex 接 aProxy 必须在 provider 配置里设 `stream_idle_timeout_ms` 为大值
（推荐 `86400000`），否则任何超过 5 分钟的重试期或长生成都会被 Codex 断开重发，约 5 次后整轮失败。
这与 Claude Code 的 `CLAUDE_STREAM_IDLE_TIMEOUT_MS` 是同一类问题，见 [[claude-code-stream-watchdogs]]。

**Recheck when**：Codex 升级（尤其 SSE 解析库或 provider 配置键变化）；Codex 改用 WebSocket
传输（`supports_websockets`）作为自定义 provider 默认。
