---
name: aproxy-cli
description: aProxy CLI 完整参考——本地 API 代理（无限重试保障 agent 工作流）的全部命令、参数、config.toml 与 settings.json 配置字段、运行行为语义与版本兼容性。凡涉及 aproxy 的启动/停止/状态/日志/别名/多开/配置修改、排障（端口占用、启动失败、日志乱码）、或为本机 agent 软件配置代理地址时使用本 skill，即使用户没有明说「查文档」——例如"帮我把 Claude Code 挂到 aproxy"、"再加一个 12346 端口的实例"、"stop 之后怎么还占着端口"。
---

# aProxy CLI

## 一分钟心智模型

aProxy 是本地 HTTP 代理：客户端把 API base URL 指向 `http://127.0.0.1:<端口>`，
aProxy 原样透传路径/查询/请求头到上游 `base_url`。请求失败（网络错误、4xx、5xx、
错误 JSON）时**无限重试**（指数退避，封顶 `max_retry_backoff_secs`），流式响应期间
向客户端发 SSE 心跳注释保活，成功后原样回放——客户端零感知。控制通道
（status/stop/logs）走命名管道 IPC，**永不占用代理端口**。

两种配置文件分工（勿混淆）：
- `config.toml`（~/.aproxy/config.toml）——人类可读可写，可多份平行并存（多开）
- `settings.json`（~/.aproxy/settings.json）——程序管理的内部状态（别名、全局默认），
  唯一，**不手改**，经 `aproxy alias`/`aproxy config --set-default` 管理

配置生效优先级（高 → 低）：
1. CLI 覆盖参数 `--baseurl/--listen/--proxy/--api-key`（仅本次运行，不落盘）
2. config.toml 显式配置的值（各实例独立）
3. settings.json 全局默认（仅 `max_body_mb`、`disk_cache` 两个字段参与此层）
4. 内置默认值

target 参数（start/stop/logs 的 `[目标]`）解析顺序：**别名 → `default` 保留字
（settings 的 default_config，含笔误 `defult`）→ 配置文件路径**。纯数字一律按
端口号解析。别名不得为纯数字或保留字 `all`/`idle`/`default`/`defult`。

## 按需查阅（读前先看这里，不要盲猜）

| 任务 | 读 |
|---|---|
| 启动/停止/状态/日志/恢复/别名/find/config 的**全部命令与参数** | [references/latest/commands.md](references/latest/commands.md) |
| 写或改 config.toml（全部字段、类型、默认值、0 值语义） | [references/latest/config-toml.md](references/latest/config-toml.md) |
| 别名/默认配置/全局默认等 settings.json 字段（一般经命令管理） | [references/latest/settings-json.md](references/latest/settings-json.md) |
| 重试判定、保活、多开、磁盘缓存、日志、自愈恢复等**行为语义与排障** | [references/latest/behaviors.md](references/latest/behaviors.md) |
| 当前版本是否适用本文档（版本判定、跨版本差异） | [references/latest/compatibility.md](references/latest/compatibility.md) |

**版本注意**：`references/latest/` 描述当前开发线。操作旧版本实例前先读
compatibility.md 确认行为差异（旧版本可能缺字段、语义不同）。

## 高频守则（细节都在 references，此处仅防最常见的错）

- `aproxy` 不带子命令 = 后台启动代理（分离子进程，关终端不掉）。前台调试用
  `aproxy --foreground`。
- 多实例必须先 `aproxy status` 再操作：`stop`/`logs` 在多实例时不接受省略参数，
  需要端口号或别名。`stop all` 停全部；`stop idle [秒]` 只停空闲实例。
- **改配置（toml 或 `aproxy config`）后用 `aproxy restart <端口或别名>` 使其
  生效**——一条命令完成停止+按原参数拉起+等就绪，改端口也适用。它只重启
  不启动：目标没在运行会报错退出 1，首次启动用 `aproxy start`。
- 端口占用排查：bind 失败分「被其他程序占用」与「无权限/被系统保留（Hyper-V
  排除区间）」，不要一律当占用处理——详见 behaviors.md 排障节。
- 守护日志是 UTF-8（无 BOM），终端乱码是控制台代码页问题，进程入口已自动切
  65001，无需 chcp。
