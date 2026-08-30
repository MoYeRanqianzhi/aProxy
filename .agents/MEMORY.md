# Memory Index
- [502 via 7890](memory/2026-08-28-502-via-7890.md) — 502 回透说明重试未生效而非 7890 代理问题，已更正归因与排查方向 [[502-via-7890]]
- [proxy-config](memory/2026-08-29-proxy-config.md) — 配置文件代理功能已落地（含可选用户名密码），reqwest 代理机制关键事实与 7890 实测结论 [[proxy-config]] [[502-via-7890]]
- [baseurl-rename](memory/2026-08-29-baseurl-rename.md) — upstream 已更名为 baseurl/base_url（alias 兼容旧配置），顶层 --api-key 已修复接线；生产实例以 aproxy-using.exe 运行锁 release exe [[baseurl-rename]] [[proxy-config]]
- [review-findings](memory/2026-08-29-review-findings.md) — 首轮审查确认 66 项：总超时掐断长流、keepalive 保真缺陷群、生命周期泄漏等；含修复分派与主动跳过项基线 [[review-findings]]
- [disconnect-billing-protection](memory/2026-08-30-disconnect-billing.md) — 客户端断开即中止上游请求的三层机制（计费保护）；流中断 mock 须用 chunked 半截的教训 [[disconnect-billing-protection]] [[review-findings]]
- [daemon-model](memory/2026-08-30-daemon-model.md) — 守护进程运行模型（alpha.3）：后台启动+命名管道 IPC（不占代理端口铁律）+status/stop 多实例；测试环境 start 挂起之谜待解 [[daemon-model]] [[review-findings]]
