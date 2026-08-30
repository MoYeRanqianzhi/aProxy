---
name: daemon-model
description: 守护进程运行模型（alpha.3）：后台启动+命名管道 IPC（不占代理端口铁律）+status/stop 多实例管理；测试环境 start 挂起之谜待解
metadata:
  type: project
---

2026-08-30 落地（v0.1.0-alpha.3，提交 3948cad）：`aproxy` 默认**后台启动**——父进程预检后 `spawn_detached`（CREATE_NO_WINDOW、Stdio null）分离子进程（`--daemon-child`，hide）承载服务；终端关闭/常规清理不再连带杀进程。

**IPC 铁律（用户明确要求）**：控制通道绝不占用代理端口，代理端口完全用于透传（避免控制路径与客户端请求巧合重叠造成严重 bug）。实现：每实例一条命名管道 `\\.\pipe\aproxy-<端口>`（unix 为 `~/.aproxy/run/<端口>.sock`），一行 JSON 请求/响应（`ping`/`shutdown`），平台实现在 `src/daemon.rs::imp`，公共编解码 `exchange_over`/`handle_conn`。

**端口冲突两种情况区分**：IPC ping 通 = 同端口已有 aProxy（提示运行中，exit 0）；管道不通但 TCP bind 失败 = 被其他程序占用（exit 1）。

**多实例管理**：`aproxy status`（注册表 `~/.aproxy/run/<端口>.pid` 枚举 + IPC 验活，死亡记录自动清理）；`aproxy stop`（单实例可省略；多实例必须指定端口或 `all`；指定端口时不依赖注册表，直连管道）。停止 = IPC shutdown → 优雅关闭 → 10s 宽限强退 → 清注册表。日志：`~/.aproxy/logs/<端口>.log`（>2MiB 截断）；`--foreground` 保留前台模式。

**已知未解问题**：在 cargo test 测试进程里通过 `Command::output()` 跑 `aproxy start`（嵌套 spawn：测试→start 父进程→spawn_detached 守护）时 start 偶发永不退出、挂死测试；同命令在 Git Bash 手动执行完全正常；单层 spawn（status/stop/config）从未挂。绕过：集成测试直接 `daemon::spawn_detached` 起 `--daemon-child`，不起 start 父进程。根因不明（审查 workflow 专项调查中）。

**Why:** 用户要求「避免常规内存清理等因素被关闭」+ 多配置多开后台共存 + stop 控制指令；铁律来自用户对端口透传纯净性的坚持。

**How to apply:** 改动进程模型/IPC/注册表时保持铁律与两种端口冲突语义；集成测试勿在测试进程树里嵌套 start 父进程。相关 [[baseurl-rename]] [[review-findings]]。
