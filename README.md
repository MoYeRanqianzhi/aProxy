# aProxy

本地 API 代理：为 agent 软件的请求提供**无限重试**与**不中断保障**。

上游过载、断流、超时……aProxy 在本地完整缓冲请求与流式响应（SSE 逐块暂存），失败自动重试（指数退避，间隔封顶 320s），期间向客户端注入 SSE 注释心跳保活连接，成功后原样回放——客户端零感知，工作流不中断。

## 特性

- **无限重试**：网络错误、4xx/5xx、错误 JSON（200 携带 error）都触发重试；客户端断开即中止上游请求（计费保护）
- **完全透传**：路径、查询、请求头原样转发；端口只做透传（控制通道走独立 IPC，绝不占用代理端口）
- **后台守护**：`aproxy` 默认后台启动（分离子进程，关终端不掉）；`status` / `stop` / `logs` / `restore` 全套实例管理
- **看门狗**：全局看护进程自动重拉崩溃/挂死的实例（默认开启，实测 +2.4% 体积 / +2.9MB 常驻 / 转发热路径零损耗，见 docs/benchmark-watchdog.md）
- **多开**：`--config` 各自指定配置文件与端口，多实例并存
- **自愈**：崩溃/断电/系统重启后 `aproxy restore` 一键恢复，空清单静默成功——可配置为开机自启

## 安装

**引导脚本（推荐，首装二进制 + skill 文档一步到位）**：

```powershell
# Windows (PowerShell)
irm https://raw.githubusercontent.com/MoYeRanqianzhi/aProxy/master/scripts/install.ps1 | iex
```

```sh
# Linux / macOS / Git Bash
curl -fsSL https://raw.githubusercontent.com/MoYeRanqianzhi/aProxy/master/scripts/install.sh | sh
```

```bat
rem Windows (cmd 兜底)
curl -fsSL https://raw.githubusercontent.com/MoYeRanqianzhi/aProxy/master/scripts/install.cmd -o install.cmd && install.cmd
```

**升级**：替换二进制后 `aproxy install` 自动滚动重启实例（后续版本提供）。

**源码构建**：

```powershell
git clone https://github.com/MoYeRanQianZhi/aProxy.git
cd aProxy
cargo build --release
# 可执行文件: target/release/aproxy.exe
```

要求 Rust 1.85+（edition 2024）。

## 快速开始

```powershell
# 1. 配置上游（交互写入 ~/.aproxy/config.toml）
aproxy config --baseurl https://api.anthropic.com --api-key sk-ant-...

# 2. 启动（默认后台）
aproxy

# 3. 把 agent 软件的 API base URL 指向本地代理
#    https://api.anthropic.com  →  http://127.0.0.1:12345
```

## 命令

| 命令 | 说明 |
|---|---|
| `aproxy [start]` | 后台启动代理（默认端口 127.0.0.1:12345） |
| `aproxy start <别名\|路径>` | 按别名或配置文件路径启动（别名见下方「别名」） |
| `aproxy --foreground` | 前台运行（日志走控制台，Ctrl+C 停止） |
| `aproxy status` | 列出运行中的实例（端口/pid/版本/上游/配置） |
| `aproxy stop [PORT\|all\|别名]` | 停止实例；单实例可省略；多实例指定端口、`all` 或别名 |
| `aproxy restart [PORT\|all\|别名]` | 重启运行中的实例（只重启不启动；改配置生效用）；`--force` 立即强杀重启 |
| `aproxy logs [PORT]` | 连接实例实时输出日志；单实例可省略；不支持 `all` |
| `aproxy restore` | 恢复崩溃/重启前在运行的实例；无实例则静默结束（开机自启友好） |
| `aproxy alias add\|remove\|list` | 管理配置别名（存于 settings.json） |
| `aproxy doctor` | 配置体检：settings.json error 级检查 + 别名配置与目录 toml 审查（warning） |
| `aproxy find [关键字]` | 从配置目录发现全部配置；`--aliased/--unaliased --port PORT` 过滤 |
| `aproxy config [选项]` | 查看或修改配置（`--show` 打印当前配置） |

通用参数：`--config <PATH>`（指定配置文件，多开用）、`--listen <ADDR>`、`--proxy <URL>`、`--api-key <KEY>`（启动路径仅本次生效）。

## 多开与别名

```powershell
aproxy --config ~/.aproxy/work.toml      # listen_addr = "127.0.0.1:12345"
aproxy --config ~/.aproxy/personal.toml  # listen_addr = "127.0.0.1:12346"
aproxy status                            # 两个实例都可见
aproxy stop 12346                        # 按端口管理
```

给配置起别名后可按名字快捷启停（不必记端口）：

```powershell
aproxy alias add openrouter ~/.aproxy/openrouter.toml
aproxy alias add anthropic               # 省略路径 = 默认 ~/.aproxy/config.toml
aproxy start openrouter                  # 按别名启动
aproxy stop openrouter                   # 按别名停止（端口变了依然有效）
aproxy alias list                        # 查看全部别名
```

别名存于 `~/.aproxy/settings.json`（程序管理的内部配置，唯一；不建议手改，
用 `aproxy alias` 管理）。config.toml 保持人类可读可写、可多份平行并存。

## 开机自启（可选）

任务计划程序创建「登录时运行」任务，操作指向 `aproxy.exe`，参数 `restore`。
之前在运行的实例自动恢复；之前全关了则静默结束，无副作用。

## 配置文件

`~/.aproxy/config.toml`（多开时各配置文件独立）：

```toml
base_url = "https://api.anthropic.com"   # 上游地址
listen_addr = "127.0.0.1:12345"          # 本地监听
# api_key = "sk-..."                     # 快捷鉴权（等效覆盖 Authorization: Bearer）
# keepalive_interval_secs = 15           # 重试期间 SSE 心跳间隔，0 关闭
# proxy = "http://127.0.0.1:7890"        # 上游经代理转发（支持 socks5，可配用户名密码）
# extra_headers / override_headers       # 追加/覆盖请求头
# max_retry_backoff_secs = 320           # 重试退避封顶（0 = 所有重试零延迟）
# spool_limit_mb = 256                   # 上游响应缓冲上限（MB）
# max_body_mb = 128                      # 请求体上限（MB；0 = 不设限。未设时取 settings.json 全局默认）
# disk_cache = true                      # 磁盘缓存：大请求体/响应溢写磁盘，内存与负载解耦（实测见 docs/benchmark-memory.md）
# connect_timeout_secs = 30              # 上游连接建立超时（0 = 不设限）
# read_timeout_secs = 300                # 两次读到数据间隔超时（0 = 不设限）
```

运行数据在 `~/.aproxy/`：`run/`（实例注册与恢复记录、看护者 claim）、`logs/`（守护日志，自动清理与轮转）、
`spool/<端口>/`（磁盘缓存临时文件，启动时自动清理）、
`settings.json`（内部配置：别名、默认配置文件、配置目录列表、日志轮转阈值、
`max_body_mb`/`disk_cache` 全局默认（toml 可按实例覆盖）、
`watchdog` 五字段（总开关/扫描周期/挂死容忍/crashloop 上限/闲置自灭）等，程序管理不建议手改）。

配置目录列表默认含 `~/.aproxy/` 与 `~/.aproxy/configs/`，`aproxy find`/`aproxy doctor`
会扫描其中的 `*.toml`（不递归）。

## 开发

```powershell
cargo test          # 全量测试（112 lib + 4 bin + 38 集成）
cargo clippy --all-targets   # 必须零警告（项目纪律）
cargo fmt --check
```

架构与开发文档见 [`docs/`](docs/)；面向 agent 的开发文档在 `.agents/docs/`。

## License

MIT
