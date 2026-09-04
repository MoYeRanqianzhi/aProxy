# 架构

aProxy 是单二进制的本地 HTTP 代理。本文面向贡献者，解释各组件职责与关键设计决策。

## 总览

```
agent 软件 ──HTTP──▶ [代理端口 12345] ──重试循环──▶ 上游 API
                        ▲                 │
                        │ 透传             │ spool
                    命名管道 IPC           ▼
                        │            内存缓冲（≤256MiB）
                  [控制通道 <端口>]        │
                        ▲                回放
        aproxy status/stop/logs/restore
```

**铁律：代理端口完全用于透传。** 所有控制通信（ping/shutdown）走独立命名管道
`\\.\pipe\aproxy-<端口>`（unix 为 `~/.aproxy/run/<端口>.sock`），杜绝控制路径与
客户端请求路径重叠。

## 模块

| 模块 | 职责 |
|---|---|
| `src/proxy.rs` | 转发核心：hop-by-hop 过滤、请求/响应 spool、错误判定、keepalive、断开保护 |
| `src/retry.rs` | 重试判定：状态码、错误 JSON（含流式 NDJSON/SSE 形态） |
| `src/config.rs` | 配置加载/保存/校验（`~/.aproxy/config.toml`） |
| `src/daemon.rs` | 守护编排：IPC（ping/shutdown）、实例注册表、恢复记录、孤儿清理 |
| `src/main.rs` | CLI（start/status/stop/logs/restore/config）、serve_forever、日志 |

## 重试与流式回放

请求体完整读入后进入重试循环。响应体**逐块暂存**（spool）到内存（上限
256MiB，超限按不可重试终态处理）——只有拿到完整响应才能保证「失败即重试」；
期间每 `keepalive_interval_secs` 向客户端写一行 SSE 注释（`: keepalive`）防超时。
响应完成后按原始字节序回放。

重试退避：前 3 次零延迟，第 4 次起 5s→10s→20s→…封顶 320s，无限重试。
客户端断开立即中止上游请求并停止重试（计费保护）。

## 守护进程模型

- `aproxy`（默认）= 后台启动：父进程预检（IPC ping → TCP bind 探测）→
  `spawn_detached` 分离子进程（Windows 手写 `CreateProcessW`，
  `bInheritHandles=FALSE` + `CREATE_NO_WINDOW`）→ 父进程轮询 IPC ping 就绪后返回。
- 子进程（`--daemon-child`）承载服务：bind → 写实例注册表（`run/<端口>.pid`）→
  写恢复记录（`run/<端口>.restore`）→ 启动 IPC 管道 → serve。
- 停止：IPC `shutdown` → 优雅关闭（10s 宽限强退）→ 清注册表与恢复记录。

### 端口冲突的两种情况

1. IPC ping 通 → 同端口已有 aProxy 在运行（不重复启动，exit 0）
2. 管道不通但 bind 失败 → 被其他程序占用 / 无权限（按错误类别报因，exit 1）

### 实例身份

端口号是实例唯一键：同端口不同监听地址的第二实例会在启动时被显式拒绝
（注册表与 IPC 管道按端口命名，无法并存）。

## 自愈恢复（restore）

`run/<端口>.restore` 存实例的启动参数：

- **写入**：守护 bind 成功时（无论前台/后台）
- **删除**：优雅退出（stop、Ctrl+C）
- **保留**：崩溃、断电、系统重启

`aproxy restore` 按记录重新拉起（`--daemon-child`），IPC ping 就绪后才报成功；
已在运行跳过（幂等）；空清单静默 exit 0（开机自启友好）。

`.restore` 绝不在 status 清理时删除——崩溃实例恰恰靠它存活到 restore 执行。

## 日志治理

- 守护日志 `logs/<端口>.log`：启动时 >2MiB 截断；运行期每小时检查，>8MiB 截断
- `aproxy logs [PORT]`：tail -f 语义（末尾 8KB/30 行 + 增量轮询），实例停止自动退出
- 孤儿清理：`status` 时删除既无存活实例也无 `.restore` 的端口日志

## 测试

- 单元测试 69 个（lib）：重试判定、配置、IPC 协议、注册表/恢复记录
- 集成测试 28 个：mock 上游 + 真实代理联调、守护生命周期、logs/restore 端到端
- 测试端口从测试进程 pid 派生（25000-65000 区间），绝不触碰用户实例；
  `DaemonGuard` 保证断言失败路径也清理守护

## 平台

Windows 优先（开发与测试都在 Windows）；unix 分支（UDS IPC、spawn）已实现
但未在类 Unix 环境实测。欢迎在 Linux/macOS 上反馈。
