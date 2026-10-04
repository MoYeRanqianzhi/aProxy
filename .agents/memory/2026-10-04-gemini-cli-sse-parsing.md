---
name: gemini-cli-sse-parsing
description: 写 Gemini CLI 接入文档、设计「按路径触发保活 / 心跳格式」功能、或排查 Gemini CLI 经 aProxy 报 Incomplete JSON segment 时回想
metadata:
  type: reference
  scope: Gemini CLI 0.35.3（本机）及锁定 @google/genai 1.30.0 的版本（调研称至 v0.62.0 与 main 均锁 1.30.0，未逐版实测）
  status: active
  last_verified: 2026-10-04
---

# Gemini CLI 与 aProxy 保活（2026-10-04 黑盒实测 + 源码）

**请求形态**（经 aProxy v0.1.0 实测）：主对话 `POST /v1beta/models/<model>:streamGenerateContent?alt=sse`，
`Accept: */*`，请求体**没有** `stream` 字段；辅助调用是非流式 `:generateContent`。指向 aProxy 用
`GOOGLE_GEMINI_BASE_URL` + `GEMINI_API_KEY`（API key 认证；OAuth / Code Assist 路径不走自定义地址）。

**默认配置下的行为**：两类请求都不满足 `keepalive_trigger = any`（无 SSE Accept、无 `"stream": true`），
走不保活的缓冲路径——**不会注入注释，结果正常**（G1 实测输出正确、退出码 0）。代价：aProxy 要等上游完整
成功才回响应头，而 Gemini CLI 的 undici `headersTimeout` / `bodyTimeout` 是写死的常量（0.35.3 均为
300000；调研称新版 headersTimeout 降到 60000），没有用户配置可调——重试期加生成超过该值即失败（之后客户端
有限次重发）。这一条来自源码调研，未做超时实测。

**为什么不能直接给它开注释保活**：Gemini CLI 锁定的 `@google/genai 1.30.0` 用以 `^\s*data: ` 锚定 buffer
开头的正则切事件，没有丢弃非 data 行的分支。流里先出现 `: keepalive` 注释后，之后的真实数据永远匹配不上，
流结束时抛 `Incomplete JSON segment at the end`。直连 mock 实测：注释前缀 → 4 次流式请求（1 + 3 次流中重试）后
`Retry attempts exhausted`、退出码 1、无输出；**空行前缀（`\r\n`）→ 一次成功**（正则的 `\s*` 吞掉前导空白）。
@google/genai 1.32.0 起改为按空行切事件并忽略非 data 事件，已无此问题（调研读源码，未实测）。

**对设计的含义**：若要让 Gemini 流也有保活（解决响应头超时），需要（1）可按路径/查询配置的保活触发（不内置
URL，参照 bounded_retry_paths 的正则写法）与（2）可选的心跳格式（空行而非注释）。空行在 SSE 规范里是「无
data 则不分发」，对其他规范解析器同样无害（推断，未逐个客户端实测）。

**Recheck when**：Gemini CLI 升级 @google/genai 到 ≥ 1.32.0；aProxy 增加按路径触发或心跳格式配置。

相关：[[claude-code-stream-watchdogs]] [[codex-stream-idle-timeout]]
