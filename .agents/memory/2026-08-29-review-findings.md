---
name: review-findings
description: 首轮大规模审查（143 agent / 7 维度 / 对抗验证）确认 66 项问题：核心缺陷、修复分派与主动跳过项
metadata:
  type: project
---

2026-08-29 首轮全库审查（workflow `review-aproxy`）确认 66 项（68 候选中 2 项被驳斥）。去重后核心缺陷四组：

1. **总超时掐断长流**（proxy.rs `timeout(300s)`）：reqwest 该超时覆盖整个响应体 spool，>5 分钟的流式生成被无限掐断重试、永不成功——直接违背项目使命。修复方向：`read_timeout(60s)` + `connect_timeout(30s)`，移除总超时。
2. **keepalive 路径保真缺陷群**：拿到上游响应前就发 200+强制 SSE 头骨架 → 成功后丢弃上游 status/全部响应头；非 SSE body 被当 SSE 帧回放（SSE 解析器静默丢数据）；空 body 伪造 `data: [DONE]`。重构为「首轮快速路径」：attempt 1 成功走原样回放（保真），需重试才发 SSE 骨架。
3. **生命周期**：后台任务三处错误路径 `let _ = tx.send(...)` 丢弃客户端断开信号 → 可能永久空转（keepalive_interval_secs ≥ 320 时必现）；优雅关闭被永不结束的重试任务卡死。
4. **中危群**：reqwest 默认跟随重定向（api_key 可被 3xx 外带 + 3xx 到不了客户端）→ `Policy::none()`；响应 spool 无上限（256 MiB 封顶，超限 502 不重试）；响应多值头被 insert 坍缩（改 append）；`{"error":null}` 误判错误无限重试；`api_key` 不覆盖 `x-api-key`（Anthropic 风格原始密钥外泄）；base_url 含 `?`/`#` 拼接错路由；`aproxy config` 会用默认值覆盖解析失败的配置文件（数据丢失）；api_key 打码按字节切片多字节 panic。

**主动跳过项**（记录在此避免复查循环）：多行 data: SSE 错误事件漏判（启发式已知限制）；Connection 头值中列举的自定义 hop 令牌不剥离（RFC 9110 严格性）；压缩响应体的错误嗅探盲区（未启用解压 feature 的固有取舍）；并发数无上限（本地单用户工具）；POST 重放重复执行语义（无限重试设计的固有代价）；listen_addr 不预校验（bind 报错已清晰）；未知配置键静默忽略（deny_unknown_fields 影响前向兼容）；./docs 与 ./.agents/docs 文档树缺失（待专项）。

**Why:** 审查发现大量集中于「keepalive 骨架先于上游响应」这一架构决策的连锁后果；总超时问题在开发期被 `client.timeout(300)` 顺手感掩盖。

**修复轮复核（2026-08-30，50 agent 审查确认 18 项并已全部修复）**：read_timeout 60s 钳制 TTFB→300s；16/19 测试未隔离环境代理→local_client()+NO_PROXY；断开保护测试观测窗口落在退避间隙（删保护仍绿）→17s 窗口+时间戳断言；keepalive 通道 spool 超限静默结束流→发 `event: error`（proxy_spool_limit）终态事件；config 保存前校验 base_url（validate_base_url 拆出）；normalized 头 trim 冲突 first-wins；--show 解析失败警告；base_url 内嵌凭据打码（mask_base_url）；main mask/parse_kv 补单测；413 消息中性化；keepalive_during_retry 补心跳/计数断言。提交 339c0ae/dddfadf/43dbf18/ecb58bf，59 单测+5 bin 测试+19 集成全绿。修复轮审查中发现验证 agent 曾修改工作区做实验（select 竞速被移除、新增临时测试文件），已 git checkout 恢复——**审查 workflow 的验证 agent 必须要求只读，修复验证一律不许改工作区**。

**How to apply:** 复审时以本文件为基线核对跳过项是否仍成立。相关 [[baseurl-rename]] [[proxy-config]] [[disconnect-billing-protection]]。
