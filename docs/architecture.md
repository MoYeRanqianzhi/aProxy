# 架构

aProxy 是单二进制的本地 HTTP 代理。本文面向贡献者，解释各组件职责与关键设计决策。

## 总览

```
agent 软件 ──HTTP──▶ [代理端口 12345] ──重试循环──▶ 上游 API
                        ▲                 │
                        │ 透传             │ spool（内存 ≤1MiB 驻留，
                    命名管道 IPC           ▼        超出溢写磁盘）
                        │          磁盘 spool ~/.aproxy/spool/<端口>/
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
| `src/main.rs` | 进程入口：控制台代码页、配置文件定位、日志初始化、子命令分派（仅 ~110 行） |
| `src/cli.rs` | clap 命令树定义（`Cli`/`Commands`/`AliasCmd`/`ConfigArgs`），只承载定义不含逻辑 |
| `src/commands/` | 九个子命令各自一文件（start/status/stop/restore/alias/doctor/find/logs/config），共享 target 解析在 `mod.rs` |
| `src/server.rs` | 服务承载：`serve_forever` 主循环、停止信号、日志初始化、配置错误落盘 startup.log |
| `src/proxy.rs` | 转发核心：hop-by-hop 过滤、内存+磁盘双模 spool、错误判定、keepalive、断开保护 |
| `src/retry.rs` | 重试判定：状态码、错误 JSON（含流式 NDJSON/SSE 形态） |
| `src/config.rs` | 代理配置加载/保存/校验（`~/.aproxy/config.toml`，可多份平行并存） |
| `src/settings.rs` | 内部配置（`~/.aproxy/settings.json`，唯一）：别名表、全局默认等程序管理状态 |
| `src/daemon.rs` | 守护编排：IPC（ping/shutdown）、实例注册表、恢复记录、孤儿清理 |
| `src/util.rs` | bin 侧共用小工具：时间戳、时长人性化、凭据打码、key=value 解析 |

CLI 定义（cli.rs）与子命令处理（commands/）分离；启动父进程逻辑（预检/spawn）
在 `commands/start.rs`，服务本身在 `server.rs`——守护子进程由前者直接进入后者。

## 内存+磁盘双模缓冲（disk_cache）

请求体与上游响应的缓冲有两种归宿，按大小自动切换：

- **内存**：≤ 1 MiB（`RESIDENT_LIMIT`）全程驻留内存，小流量零磁盘 IO（真实
  agent 流量的绝大多数）。
- **磁盘 spool**：超出 1 MiB 即溢写 `~/.aproxy/spool/<端口>/*.spooltmp`，
  重试重放与响应回放流式读文件——进程内存与负载大小解耦。读写走 OS page
  cache，SSD 上吞吐与纯内存持平（实测数据见 [benchmark-memory.md](benchmark-memory.md)）。

生命周期保证：临时文件在请求结束（Drop）、回放流 EOF、重试丢弃三路删除；
实例启动时清空本端口目录回收崩溃残留。磁盘写失败按 `SpoolFailed` 终态处理
（502 / SSE error 事件），绝不退化成内存堆积。`disk_cache = false`（settings
全局默认或 toml 覆盖）关闭时回到纯内存行为。

## 重试与流式回放

请求体完整读入（超 `max_body_mb` 即 413，不转发——客户端断开即中止上游请求，
避免无谓计费）后进入重试循环。响应体**逐块暂存**（spool）到内存/磁盘（上限
`spool_limit_mb` 256MiB，超限按不可重试终态处理）——只有拿到完整响应才能保证
「失败即重试」；期间每 `keepalive_interval_secs` 向客户端写一行 SSE 注释
（`: keepalive`）防超时。响应完成后按原始字节序回放（零拷贝：内存模式用
`Bytes::slice_ref` 共享原分配）。

重试退避：前 3 次零延迟，第 4 次起 5s→10s→20s→…封顶 `max_retry_backoff_secs`
（默认 320s），无限重试。客户端断开立即中止上游请求并停止重试（计费保护）。

## 配置分层

三级优先级（仅 `max_body_mb`/`disk_cache` 有 settings 层；其余字段 toml > 内置默认）：

```
CLI 覆盖参数（--baseurl 等，仅本次） > config.toml 显式值 > settings.json 全局默认 > 内置默认
```

`config.toml` 人类可读可写、可多份（多开各自指定）；`settings.json` 程序管理的
内部配置，**全局唯一**（JSON 原子写，损坏回退默认），存别名表、default_config、
config_dirs、日志轮转阈值、空闲阈值与上述两个字段的全局默认。

### 配置别名（settings.json）

- `aproxy alias add <名> [路径]`：路径省略时指向默认 config.toml；
  别名不得为 `all`/`idle`/`default`/`defult`/纯数字（保留字与端口解析冲突）
- `aproxy start/stop <别名>`：start 把别名解析出的配置路径以 `--config` 绝对
  路径注入守护子进程命令行（别名解析只发生在父进程）；stop 按实例注册的
  config_path 归一匹配（大小写/分隔符），端口变了别名依然有效

## 守护进程模型

- `aproxy`（默认）= 后台启动：父进程预检（IPC ping → TCP bind 探测）→
  `spawn_detached` 分离子进程（Windows 手写 `CreateProcessW`，
  `bInheritHandles=FALSE` + `CREATE_NO_WINDOW`）→ 父进程轮询 IPC ping 就绪后返回。
- 子进程（`--daemon-child`）承载服务：bind → 清 spool 目录 → 写实例注册表
  （`run/<端口>.pid`）→ 写恢复记录（`run/<端口>.restore`）→ 启动 IPC 管道 → serve。
- 停止：IPC `shutdown` → 优雅关闭（10s 宽限强退）→ 清注册表与恢复记录。

### 端口冲突的两种情况

1. IPC ping 通 → 同端口已有 aProxy 在运行（不重复启动，exit 0）
2. 管道不通但 bind 失败 → 被其他程序占用 / 无权限或被系统保留
   （Hyper-V/WinNAT 排除区间，按错误类别报因，exit 1）

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
- 全部输出对凭据打码（api_key/头值/代理密码/base_url 内嵌密码）——可安全粘贴分享

## 测试

- 单元测试 93 个（lib）+ 4 个（bin）：重试判定、配置分层、IPC 协议、注册表/恢复记录、打码
- 集成测试 34 个：mock 上游 + 真实代理联调、守护生命周期、logs/restore 端到端、
  双模缓冲（磁盘 spool 字节保真/EOF 清理/流尾错误重试）
- 测试端口从测试进程 pid 派生（25000-65000 区间），绝不触碰用户实例；
  `DaemonGuard` 保证断言失败路径也清理守护

## 平台

Windows 优先（开发与测试都在 Windows）；unix 分支（UDS IPC、spawn）已实现
但未在类 Unix 环境实测。欢迎在 Linux/macOS 上反馈。

## 版本路线

- **0.1.x**：当前开发线，alpha → beta；无 UI，纯 CLI + 守护。
- **0.2.0**：引入 UI（Web 面板形态待定）——前提是基础功能齐备、稳定性经 beta
  期验证；属远期。
- 正式发布（脱离预发布后缀）起，GitHub CI 构建指令集多版本（baseline +
  x86-64-v3），见 `.agents/memory/2026-09-07-release-engineering.md`。
