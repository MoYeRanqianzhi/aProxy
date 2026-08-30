---
name: daemon-model
description: 守护进程运行模型（alpha.3）：后台启动+命名管道 IPC（不占代理端口铁律）+status/stop 多实例管理；start 挂起之谜已解（句柄泄漏，b31461c）
metadata:
  type: project
---

2026-08-30 落地（v0.1.0-alpha.3，提交 3948cad）：`aproxy` 默认**后台启动**——父进程预检后 `spawn_detached`（CREATE_NO_WINDOW、Stdio null）分离子进程（`--daemon-child`，hide）承载服务；终端关闭/常规清理不再连带杀进程。

**IPC 铁律（用户明确要求）**：控制通道绝不占用代理端口，代理端口完全用于透传（避免控制路径与客户端请求巧合重叠造成严重 bug）。实现：每实例一条命名管道 `\\.\pipe\aproxy-<端口>`（unix 为 `~/.aproxy/run/<端口>.sock`），一行 JSON 请求/响应（`ping`/`shutdown`），平台实现在 `src/daemon.rs::imp`，公共编解码 `exchange_over`/`handle_conn`。

**端口冲突两种情况区分**：IPC ping 通 = 同端口已有 aProxy（提示运行中，exit 0）；管道不通但 TCP bind 失败 = 被其他程序占用（exit 1）。

**多实例管理**：`aproxy status`（注册表 `~/.aproxy/run/<端口>.pid` 枚举 + IPC 验活，死亡记录自动清理）；`aproxy stop`（单实例可省略；多实例必须指定端口或 `all`；指定端口时不依赖注册表，直连管道）。停止 = IPC shutdown → 优雅关闭 → 10s 宽限强退 → 清注册表。日志：`~/.aproxy/logs/<端口>.log`（>2MiB 截断）；`--foreground` 保留前台模式。

**已解问题（原「测试环境 start 挂起之谜」，b31461c 修复）**：根因是 std `Command` 在 Windows 固定 `bInheritHandles=TRUE`——`spawn_detached` 经 std 启动时把调用方（cargo test/CI/agent 脚本）的 stdio 管道句柄泄漏给常驻守护进程，捕获输出的调用方永远等不到 EOF。修复：`spawn_detached` 改手写 `CreateProcessW`（windows-sys 0.52，`bInheritHandles=FALSE`+`CREATE_NO_WINDOW`+`CREATE_UNICODE_ENVIRONMENT`）。回归测试 `start_parent_command_output_returns` 守护此行为。

**审查修复轮（b31461c）其他关键语义**：IPC 客户端带 ERROR_PIPE_BUSY(231) 重试；`ipc_ping` 连续 3 次失败才判死（防瞬时失败误删注册表）；watch 通道关闭≠停止信号（IPC serve 故障不令守护自杀）；就绪判定用 IPC ping 而非 TCP connect；bind 失败按 10048/10013/其他分类；同端口不同 listen_addr 显式报错不并存；注册表 tmp+rename 原子写；测试端口动态派生（25000+(pid%20000)*2）+DaemonGuard 兜底清理，绝不碰固定端口/生产实例。有意跳过：CREATE_BREAKAWAY_FROM_JOB（作业不允许时 CreateProcess 直接失败）、命名管道 ACL/冒名校验（tokio 不暴露，误判方向 fail-safe）。

**2026-08-31 观察**：修复轮开始时 12345 生产实例（PID 14552）仍在，tests 域结束时 netstat 已无监听；tests agent 的 stop 操作全部只针对派生端口，agent 侧无法解释其消失——已在汇报中提示用户确认（可能用户自行停止）。

**Why:** 用户要求「避免常规内存清理等因素被关闭」+ 多配置多开后台共存 + stop 控制指令；铁律来自用户对端口透传纯净性的坚持。

**How to apply:** 改动进程模型/IPC/注册表时保持铁律与两种端口冲突语义；集成测试勿在测试进程树里嵌套 start 父进程。相关 [[baseurl-rename]] [[review-findings]]。
