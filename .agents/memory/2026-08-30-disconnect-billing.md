---
name: disconnect-billing-protection
description: 客户端断开即中止上游请求（计费保护）已实现：send 检查 + watch 哨兵 + select 竞速，非 keepalive 靠 hyper drop future
metadata:
  type: project
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
