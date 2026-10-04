---
name: claude-code-stream-watchdogs
description: 设计或排查保活/首轮提交/客户端超时时回想——真实 Claude Code 的请求形态与三层流超时实测值，以及哪些心跳能骗过哪一层
metadata:
  type: reference
  scope: aProxy 与 Claude Code（2.1.288 实测）之间的流式交互；其他客户端未测
  status: active
  last_verified: 2026-10-04
---

# Claude Code 的请求形态与流超时（2026-10-04 黑盒实测）

**请求形态**：主请求 `POST /v1/messages?beta=true`，`Accept: application/json`，
请求体 `"stream": true`，`x-stainless-timeout: 600`。→ 只看 Accept 判定 SSE 的
保活通道（旧 `src/proxy.rs` 的 client_wants_sse）对 Claude Code 永远不生效。

**三层超时**（上游行为 → Claude Code 何时断开并重发请求）：

| 上游行为 | 结果 | 对应机制 |
|---|---|---|
| 连响应头都不回 | 约 360s 重发 | 首字节超时（`CLAUDE_STREAM_FIRST_BYTE_TIMEOUT_MS`） |
| 回 200+SSE 头后完全静默 | 300s 重发 | 字节级空闲（`CLAUDE_BYTE_STREAM_IDLE_TIMEOUT_MS`） |
| 头 + 每 5s 一条 `: keepalive` 注释 | 正好 600s 断开 | 事件级空闲（`CLAUDE_STREAM_IDLE_TIMEOUT_MS`） |
| 头 + 每 5s 一个 `event: ping` | 正好 600s 断开 | 同上（SDK 丢弃 ping，不算事件） |
| 头 + 每 5s 一个真实 content_block_delta | 超过 600s 仍存活 | 真实事件会重置事件级看门狗 |
| 注释心跳 + `CLAUDE_STREAM_IDLE_TIMEOUT_MS=3600000` | 超过 600s 仍存活 | 证实 600s 闸由该变量控制 |
| 注释心跳 + `CLAUDE_STREAM_IDLE_TIMEOUT_MS=86400000`（24h） | 780s 时仍存活（由测试 timeout 结束） | 大取值不会被截回默认，可作推荐值 |
| 注释心跳 + `API_TIMEOUT_MS=3600000` | 仍在 600s 断开 | `API_TIMEOUT_MS` 不控制这道闸 |
| 经 aProxy（b9b1f7d，默认配置）+ `CLAUDE_STREAM_IDLE_TIMEOUT_MS=86400000`，上游 8 类故障后 757s 才成功、再慢速生成 140s | 约 900s 后拿到完整结果，客户端从未自行重发 | 0.1.0 硬验收，详见 release-0.1.0 计划进度节 |

**对 aProxy 的含义**：注释心跳能覆盖首字节与字节级两层；事件级 600s 闸只能靠
真实事件或客户端配置解除。aProxy 为保住「流中途断开也能透明重试」会缓冲完整
响应、不转发上游事件，因此用 Claude Code 时**客户端必须把
`CLAUDE_STREAM_IDLE_TIMEOUT_MS` 设大**，否则任何超过 10 分钟的重试期或长生成都会
被客户端断开重发（aProxy 随之按计费保护中止上游）。伪造协议事件不可取（违背
「不内置协议特例」，且与真实 message_start 冲突）。

**Evidence**：mock 上游 + 隔离 `CLAUDE_CONFIG_DIR` + 假 key 驱动真实 claude 2.1.288
（`claude -p`），mock 记录每次请求到达时刻与连接重置时刻；二进制内报错文案
「A proxy or gateway that buffers streaming responses can cause this — set
CLAUDE_STREAM_FIRST_BYTE_TIMEOUT_MS」与上表一致。实验脚本与方法见
release-0.1.0 计划的证据节（计划已删除，见 `git show 58e62cd:.agents/plan/release-0.1.0.md`）。

**附带观察**（2026-10-04 验收时 claude 的 stderr）：请求经 127.0.0.1 网关时，Claude Code 提示 auto mode 的分类器请求无法享受新的免计费方式，需要网关实现 https://code.claude.com/docs/en/auto-mode-classifier-billing 。不影响功能，可作为 0.2.x 的功能候选，未评估。

**Recheck when**：Claude Code 大版本更新、或上述环境变量名/默认值在其文档中变化；
接入其他 agent 客户端（Codex 等）时需单独实测，不能套用本表。

相关：[[2026-08-30-disconnect-billing]] [[2026-09-14-forward-only]]
