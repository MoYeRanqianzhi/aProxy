---
name: agent-client-timeouts-survey
description: 更新「接入 agent 客户端」文档、新增客户端支持、或排查某客户端经 aProxy 中途断开时回想——各客户端流超时的源码依据与核实程度
metadata:
  type: reference
  scope: 2026-10-04 的源码调研（调研代理 + 子代理读 GitHub 源码），除注明外均未运行客户端实测
  status: active
  last_verified: 2026-10-04
---

# agent 客户端流超时调研（2026-10-04）

用户面向的结论与配置写法只放在 skill 的 `references/latest/behaviors.md`「接入 agent 客户端」
（README 双语有摘要）；本条只记**依据**，方便日后复查。实测过的三个客户端各有独立条目：
[[claude-code-stream-watchdogs]]、[[codex-stream-idle-timeout]]、[[gemini-cli-sse-parsing]]。

判断法：计时器在 SSE 解析**之前**（字节级）→ 注释心跳能续；在解析**之后**（事件级）→ 续不住。
主流解析器都在解析层丢弃注释：eventsource-stream 0.2.3、eventsource-parser 3.x（onComment 未设时）、
openai-node / anthropic-sdk-typescript / openai-python / anthropic-sdk-python 的 SSE 解码器。

| 客户端（版本） | 结论 | 依据（调研给出的位置，行号由子代理读出、主代理未逐条复核） |
|---|---|---|
| Qwen Code v0.24.7 | 事件级 240s + 总时长 15 分钟，须 `QWEN_STREAM_IDLE_TIMEOUT_MS=0` 与 `QWEN_STREAM_MAX_LIFETIME_MS=0` | `core/stream-guards.ts` withStreamGuards 包在已解析 chunk 外；`openaiContentGenerator/constants.ts`；文档 `docs/users/configuration/settings.md:243-252`（总时长只认环境变量） |
| dsh = DeepSeek Harness（deepseek-ai/deepseek-harness，dsh-v0.2.1-alpha.1） | `llm-deepseek`：`sse.ts` 的 `onComment` 调 watchdog.pulse()，注释能续；`llm-pi-ai`：watchdog 只包事件，须设 `streamIdleTimeoutMs`（推荐值 172800000 取自仓库 `bundle/sdk-minimal/cordis.patch.yml`） | `packages/util/timeout/src/index.ts:126-170`；`llm-pi-ai/src/adapter.ts:384-397` |
| pi 1.0.2（earendil-works/pi，包 @earendil-works/pi-coding-agent；@mariozechner/* 已 deprecated） | 字节级 undici body/headers 300s（`httpIdleTimeoutMs`），默认即可；Anthropic 路径 Accept 为 application/json，靠 stream:true 进保活 | `packages/coding-agent/src/core/http-dispatcher.ts` |
| OpenCode v1.18.34（anomalyco/opencode）/ 本机 1.2.21 | 1.18：`chunkTimeout`（字节级，wrapSSE 包原始 body）与 `headerTimeout` 默认 300s，`timeout` 无默认值（官方文档写 300000 与源码不符）；1.2.21 无流超时 | `packages/opencode/src/provider/provider.ts:37-126` |
| Aider 0.86 / Kimi CLI 1.52 | httpx read 600s 字节级，默认即可 | aider `models.py:28`；kimi `openai_common.py` |
| Cline 4.1.22 / Roo Code 3.54 | 字节级（undici 默认 / apiRequestTimeout 600s），默认即可 | cline `providers/ai-sdk.ts`；roo `providers/utils/timeout-config.ts` |

**Recheck when**：任一客户端大版本更新或换 SSE 解析库；有人报告上表「默认即可」的客户端经 aProxy
中途断开（先怀疑调研结论，再用 mock 黑盒复现，方法同 Codex 条目）。
