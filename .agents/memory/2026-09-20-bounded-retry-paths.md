---
name: bounded-retry-paths
description: compact 事故与受限重试路径补丁——/count_tokens 等确定性 404 端点失败 3 次即透传，不再无限重试
metadata:
  type: project
---

# 受限重试路径（2026-09-20 compact 事故）

现象：Claude Code compact 总是出错；仅转发模式与直连都正常。

根因：compact 依赖 `POST /v1/messages/count_tokens`，镜像上游
（opencode.ai/zen、hub.oaifree 等）普遍未实现该端点，确定性 404（HTML/JSON
错误页）；「4xx/5xx 一律无限重试」让该调用永远等不到终态，Claude Code 等到
自己的超时后报错。12233 复现日志实证（2026-09-20 13:44 UTC：count_tokens
404 风暴 attempt 1→7+、退避 5/10/15s…封顶 320s）；2026-09-15 的 12345 日志有
同一风暴（另一上游、同一疾病）。compact 的总结请求（POST /v1/messages）本身
是成功的——害死 compact 的是这个辅助调用。教训：**「无限重试」对瞬时错误是
保障，对确定性错误是纯伤害**。

修复（用户定向「部分上游不支持的 URL 路径失败一定次数后不重试」）：

- `retry.rs`：`BOUNDED_RETRY_PATH_SUFFIXES = ["/count_tokens"]`（**后缀匹配**
  客户端请求路径，与上游 base_url 是否带前缀无关；`?beta=true` 查询串先剥）
  + `BOUNDED_RETRY_MAX_ATTEMPTS = 3`（含首轮；前两次重试零延迟，总耗时
  ≈ 3×上游 RTT，真正的瞬时故障大概率仍能在此窗口自愈）
- 两个重试通道在「Response 失败 + 受限路径 + 达上限」时终止：无保活通道
  `build_replay_response` 原样回放最后一次失败响应（客户端拿到真实 404，
  实测 compact 自会处理）；保活通道发终态 SSE error 事件（骨架 200 已发出，
  真实 status/headers 不可再回放，同 TooLarge/SpoolFailed 先例）
- **网络错误不封顶**：真瞬时类（使命核心），且没有响应可供回放
- 非受限路径完全不受影响——集成测试带对照（404 持续时重试必须越过 3 次），
  防止封顶被错误地全局化（那会违背无限重试使命）

诊断技巧：生产日志的 WARN 风暴是这类问题的第一现场；重试行**没有请求 ID**，
归因靠相邻 INFO 行（`代理请求` 的 method/path）与错误预览内容（HTML 404 页
vs 错误 JSON）交叉比对，并发请求交错时尤其如此。

相关：[[forward-only]] [[compressed-body-inspection]] [[disconnect-billing-protection]]
