# 守护进程运行模型——决策记录

> 沉淀自 alpha.3 开发与两轮审查。改动进程模型/IPC/注册表前先读本文。

## 为什么控制通道绝不用代理端口（铁律）

用户明确要求：端口完全用于透传，避免控制路径与客户端请求路径巧合重叠造成
严重 bug（例如某请求恰好长成控制报文形状）。实现：每实例一条命名管道
`\\.\pipe\aproxy-<端口>`（unix UDS `~/.aproxy/run/<端口>.sock`），一行 JSON
请求/响应（`ping`/`shutdown`）。

## 实例键 = 端口号

注册表/IPC 端点/日志文件都按端口命名。同端口不同监听地址的两个实例无法
并存（管道与注册表会互相顶替，stop 会停错实例）——启动时显式检测并拒绝
（ping 响应的 listen_addr 与本次不同即报错），而非静默顶替。

## spawn_detached 为何手写 CreateProcessW

std 的 `Command` 在 Windows 固定 `bInheritHandles=TRUE` 且无稳定 API 关闭
（`CommandExt::inherit_handles` 仍 unstable）。后果：调用方经
`Command::output()` 捕获输出运行 `aproxy start` 时，stdio 管道写端句柄被
常驻守护继承，EOF 永不到来——cargo test / CI / agent 脚本全部挂死
（即曾经的「测试环境 start 挂起之谜」）。修复：手写 `CreateProcessW`，
`bInheritHandles=FALSE` + `CREATE_NO_WINDOW` + `CREATE_UNICODE_ENVIRONMENT`。
回归测试 `start_parent_command_output_returns` 守护此行为。

刻意不加 `CREATE_BREAKAWAY_FROM_JOB`：作业对象不允许 breakaway 时该标志让
CreateProcess 直接失败，把「父环境关闭连带杀守护」的罕见场景恶化为
「根本启动不了」。

## IPC 可靠性语义

- 客户端带 `ERROR_PIPE_BUSY(231)` 重试（serve 循环 connect→create 间存在
  零监听窗口，tokio 文档明确要求客户端重试此错误）
- `ipc_ping` 连续 3 次失败（间隔 200ms）才判死；`wait_until_gone` 连续 2 轮
  失败才算退出——单次失败可能是瞬态 busy/超时，误判会删活实例的注册记录
- watch 通道关闭 ≠ 停止信号：唯一的 Sender 在 IPC serve 任务里，该任务任何
  故障退出都会关闭通道；若把关闭当信号，IPC 失败就等于守护自杀。通道关闭后
  转为永久挂起，只等控制台信号
- 请求行长 64KB 上限；注册表/恢复记录 tmp+rename 原子写

## .restore 的生命周期（自愈语义）

| 事件 | .pid 注册表 | .restore 记录 |
|---|---|---|
| 守护 bind 成功 | 写 | 写（含启动参数） |
| 优雅退出（stop/Ctrl+C） | 删 | 删 |
| 崩溃/断电/系统重启 | 残留 | **保留** |

`aproxy restore` 按记录拉起；空清单静默 exit 0（开机自启友好）；幂等跳过
已在运行。**status 的孤儿日志清理绝不动 .restore**——崩溃实例恰恰靠它存活
到 restore 执行，先跑一次 status 就清记录会让自愈失效；待恢复实例的日志
同理保留（排障线索，复活后同端口继续追加）。

前台实例（`--foreground`）也写 .restore：终端关闭属非正常退出，restore
以守护方式拉起是合理恢复。

## 测试端口派生

测试端口 = `25000 + (pid % 20000) * 2 + offset`（u16 内不回绕）：跨 worktree
并行的测试进程 pid 不同 → 端口天然错开；数学上排除 12345 与常见手工端口。
预清理 stop 只针对派生端口。`DaemonGuard`（Drop 执行 stop）保证断言失败
的 unwind 路径也清理守护，杜绝泄漏进真实 `~/.aproxy`。

## 有意跳过项（勿重复报告）

- 命名管道 ACL/冒名校验：tokio 不暴露 SECURITY_ATTRIBUTES；误判方向
  fail-safe（被冒名时提示「已在运行」而非错误停止服务）
- `aproxy stop <端口>` 无实例 exit 1、无参/all 扫描为空 exit 0：操作失败与
  扫描为空是不同语义
- status/stop/logs 忽略 `--config`：实例管理是全局视角，不按配置作用域
