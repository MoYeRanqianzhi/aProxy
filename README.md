# aProxy

> 「飘风不终朝，骤雨不终日。」——《道德经》

**本地 API 代理，为 agent 软件的每一次请求护道。**

上游限流、断流、超时、五百年一遇的抖动——aProxy 在本地把请求完整接住：失败**无限重试**（指数退避，封顶可配），流式响应期间注入 SSE 心跳保活，成功后按原字节回放。客户端零感知：它以为的那次请求，只是慢了一点。

[English](README_EN.md)

## 其术

- **无限重试** —— 夫唯不争，故天下莫能与之争。网络错误、4xx/5xx、错误 JSON（200 携带 error）皆触发重试；客户端断开即中止上游请求（计费保护）。
- **完全透传** —— 大音希声，大象无形。路径、查询、请求头原样转发；控制通道走独立命名管道，代理端口只做透传一件事。
- **守护进程** —— `aproxy` 后台启动（分离子进程，关终端不掉）；`status` / `stop` / `logs` / `restore` 全套实例管理。
- **看门狗** —— 天网恢恢，疏而不失。全局看护进程自动重拉崩溃/挂死的实例（默认开启；实测 +2.4% 体积、+2.9MB 常驻、转发热路径零损耗）。
- **多开** —— 万物并育而不相害。每份 config.toml 一实例，端口各自独立，并存不扰。
- **自愈** —— 复命曰常，知常曰明。崩溃/断电/系统重启后 `aproxy restore` 一键恢复，空清单静默成功——可配开机自启。

## 入门

**交给 agent 一句话即可**——把下面这行发给你的 agent（Claude Code 等），它会读文档并完成安装与配置：

```text
Read https://raw.githubusercontent.com/MoYeRanQianzhi/aProxy/main/docs/INSTALL_AGENT.md and install (or upgrade) aProxy on this machine exactly as it says.
```

**人类自装**（首装二进制 + skill 文档，SHA256 校验，落位 `~/.aproxy/`）：

```powershell
# Windows (PowerShell)
irm https://raw.githubusercontent.com/MoYeRanqianzhi/aProxy/main/scripts/install.ps1 | iex
```

```sh
# Linux / macOS / Git Bash
curl -fsSL https://raw.githubusercontent.com/MoYeRanqianzhi/aProxy/main/scripts/install.sh | sh
```

```bat
rem Windows (cmd 兜底)
curl -fsSL https://raw.githubusercontent.com/MoYeRanqianzhi/aProxy/main/scripts/install.cmd -o install.cmd && install.cmd
```

升级：`aproxy install`（在线下载链条自动滚动重启，逐实例无感；`--from <路径>` 本地安装、`--adopt` 收编既有安装）。手动兜底：`aproxy stop all` → 覆盖二进制 → `aproxy restore`。

源码构建：

```powershell
git clone https://github.com/MoYeRanqianzhi/aProxy.git
cd aProxy
cargo build --release
```

## 起手

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
| `aproxy start <别名\|路径>` | 按别名或配置文件路径启动 |
| `aproxy --foreground` | 前台运行（日志走控制台，Ctrl+C 停止） |
| `aproxy status` | 列出运行中的实例（端口/pid/版本/上游/配置） |
| `aproxy stop [PORT\|all\|别名]` | 停止实例；多实例必须指定端口、`all` 或别名 |
| `aproxy restart [PORT\|all\|别名]` | 重启运行中的实例（只重启不启动；改配置生效用）；`--force` 立即强杀重启 |
| `aproxy logs [PORT]` | 连接实例实时输出日志；不支持 `all` |
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

```powershell
aproxy alias add openrouter ~/.aproxy/openrouter.toml
aproxy alias add anthropic               # 省略路径 = 默认 ~/.aproxy/config.toml
aproxy start openrouter                  # 按别名启动
aproxy stop openrouter                   # 按别名停止（端口变了依然有效）
aproxy alias list
```

别名存于 `~/.aproxy/settings.json`（程序管理的内部配置，唯一；用 `aproxy alias` 管理）。config.toml 人类可读可写、可多份平行并存。

## 开机自启（可选）

任务计划程序创建「登录时运行」任务，操作指向 `aproxy.exe`，参数 `restore`。之前在运行的实例自动恢复；之前全关了则静默结束，无副作用。

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
# disk_cache = true                      # 磁盘缓存：大请求体/响应溢写磁盘，内存与负载解耦
# connect_timeout_secs = 30              # 上游连接建立超时（0 = 不设限）
# read_timeout_secs = 300                # 两次读到数据间隔超时（0 = 不设限）
```

运行数据在 `~/.aproxy/`：`run/`（实例注册与恢复记录、看护者 claim）、`logs/`（守护日志，自动清理与轮转）、
`spool/<端口>/`（磁盘缓存临时文件，启动时自动清理）、
`settings.json`（内部配置：别名、默认配置文件、日志轮转阈值、看门狗五字段等，程序管理不建议手改）。

## 开发

```powershell
cargo test --locked           # 全量测试（115 lib + 4 bin + 38 集成）
cargo clippy --all-targets --locked -- -D warnings   # 必须零警告（项目纪律）
cargo fmt --all -- --check
```

架构与开发文档见 [`docs/`](docs/)；面向 agent 的开发文档在 `.agents/docs/`。

## License

MIT
