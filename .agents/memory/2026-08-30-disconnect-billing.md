---
name: disconnect-billing-protection
description: 客户端断开即中止上游请求（计费保护）已实现：send 检查 + watch 哨兵 + select 竞速，非 keepalive 靠 hyper drop future
metadata:
  type: project
  scope: src/proxy 的重试与保活通道，以及测试里手写的 mock 上游
  status: active
  last_verified: 2026-11-11
---

2026-08-30 落地的计费保护语义：**客户端断开 → 该请求的所有上游活动立即终止**。

三层机制（src/proxy.rs）：
1. **不启动新请求**：keepalive 后台任务所有 `tx.send` 检查 `is_err()`，断开（rx 随 Body drop）即 return。
2. **中止 in-flight**：响应 Body 的流闭包持有 `ClientGoneGuard`（watch 哨兵），hyper 因断开 drop Body 时置位；后台任务 `tokio::select!` 让 `forward_once` 与信号竞速，断开即丢弃 future → reqwest 连接关闭 → 上游停止生成。
3. **非 keepalive 通道**：重试循环活在 handler future 内，hyper drop future 自动中止，无需显式代码。

已知边界：断开前上游已生成的 token 是否计费由上游政策决定，代理只能保证连接立即关闭。

测试：`client_disconnect_stops_upstream_requests`（断开后上游计数停滞）、`interrupted_stream_is_retried`。

**Why:** 用户明确指出断开后继续请求会造成计费浪费（断开→计费不完全；继续→计费浪费），要求断开即断。

**How to apply:** 改动 keepalive/重试路径时必须保持三层机制语义；写「流中断」类 mock 上游必须用 chunked 半截（connection-close framing 下 FIN 是合法流结束，不构成中断——曾致测试误判）。审查遗留的测试缺口已全部补齐（59 单测 + 19 集成）。相关 [[review-findings]] [[baseurl-rename]]。

**手写 TCP mock 上游必须先读完请求再响应或关闭（2026-11-11 实测）：** 接收缓冲里留着未读的请求时，Windows 以 RST 而不是
FIN 关闭连接，RST 把对端还没读的响应一起丢掉，客户端只看到「error sending request」——代理按设计无限重试，测试表现为
卡死（不是代理的缺陷）。升级到 tokio 1.54 后 accept 出来的套接字一开始就可写（tokio#8435），竞态变成必现：
`interrupted_stream_is_retried` 在本机与 CI 的 windows job 上都卡到超时，修复见 8e97317（用文件里现成的
`read_http_request`）。有意「accept 后立即断开」模拟连接失败的 mock 不受此限。

**Evidence:** 2026-11-11 带 trace 日志单跑该测试：每次尝试都是 `error sending request`，包括 mock 本该成功的第二次；
先读请求之后 0.06 秒通过。

**Recheck when:** 改写 mock 上游的写法，或 tokio 的 accept 就绪语义再变。
