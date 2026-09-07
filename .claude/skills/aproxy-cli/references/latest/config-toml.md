# config.toml 配置参考

配置文件位置：默认 `~/.aproxy/config.toml`；多开时每份 toml 独立（`--config`
或别名指向）。所有字段可省略——省略即取默认（有 settings.json 全局默认层的
字段见「优先级」标注）。

## 目录

- [完整示例](#完整示例)
- [字段总表](#字段总表)
- [各字段语义](#各字段语义)
- [校验规则](#校验规则)
- [归一化行为](#归一化行为)

---

## 完整示例

```toml
base_url = "https://api.anthropic.com"   # 必填（唯一无默认的字段）
listen_addr = "127.0.0.1:12345"
# api_key = "sk-..."
# extra_headers = { "x-custom" = "v" }
# override_headers = { "user-agent" = "my-agent/1.0" }
# keepalive_interval_secs = 15
# proxy = "http://127.0.0.1:7890"
# proxy_username = "u"
# proxy_password = "p"
# max_retry_backoff_secs = 320
# spool_limit_mb = 256
# connect_timeout_secs = 30
# read_timeout_secs = 300
# max_body_mb = 128
# disk_cache = true
```

## 字段总表

| 字段 | 类型 | 默认值 | 说明 |
|---|---|---|---|
| `base_url` | string | （空，必填） | 上游 API base URL；末尾 `/` 自动去除 |
| `listen_addr` | string | `"127.0.0.1:12345"` | 本地监听地址，须含端口 |
| `api_key` | string? | 无 | 快捷鉴权（覆盖 Authorization: Bearer） |
| `extra_headers` | map | 空 | 仅当请求未携带该头时追加（大小写不敏感判定） |
| `override_headers` | map | 空 | 无条件覆盖请求头（大小写不敏感匹配） |
| `keepalive_interval_secs` | u64 | 15 | 重试期间 SSE 心跳间隔秒；0=关闭 |
| `proxy` | string? | 无 | 上游代理 URL（http/https/socks4/socks4a/socks5/socks5h） |
| `proxy_username` | string? | 无 | 代理用户名，优先于 URL 内嵌 |
| `proxy_password` | string? | 无 | 代理密码，优先于 URL 内嵌 |
| `max_retry_backoff_secs` | u64 | 320 | 重试指数退避封顶秒；0=所有重试零延迟 |
| `spool_limit_mb` | u64 | 256 | 响应缓冲上限 MB，超出判确定性失败（不可重试） |
| `connect_timeout_secs` | u64 | 30 | 上游连接建立超时秒；0=不设限 |
| `read_timeout_secs` | u64 | 300 | 两次读到数据间隔超时秒（钳制首字节等待）；0=不设限 |
| `max_body_mb` | u64? | settings 层 | 请求体上限 MB，超出 413；0=不设限 |
| `disk_cache` | bool? | settings 层 | 磁盘缓存开关 |

**优先级**（`max_body_mb`/`disk_cache` 两个 Option 字段独有）：
toml 显式值 > settings.json 全局默认 > 内置默认（128 / true）。
其余字段无 settings 层：toml 显式值 > 内置默认。

## 各字段语义

### base_url

上游的根地址。请求转发 = `<base_url>/<原路径>?<原查询>`，路径与查询完整透传。
校验：必须 `http://` 或 `https://` 开头（大小写不敏感）；**不得含 `?` 或 `#`**
（拼接会错路由）。末尾斜杠自动去除。旧字段名 `upstream_url` 仍可读取（兼容别名），
保存时写为新名 `base_url`。

### listen_addr

监听地址。必须带端口（缺失/越界在启动时拦截报错，不会误诊为端口占用）。
默认仅绑定 127.0.0.1（不暴露局域网）。端口 0 = 系统随机分配（status 查实际值）。
多开实例的 listen_addr 必须互不相同（实例按端口号区分）。

### api_key

设置后等效于把 `Authorization: Bearer <api_key>` 覆盖进每个转发请求。适用场景：
客户端不便配置鉴权头时集中注入。请求自带 Authorization 头时它作为 override
参与覆盖逻辑（见下）。空串/纯空白视为未设置（归一化剔除）。

### extra_headers / override_headers

TOML 内联表，键值均为字符串：

```toml
extra_headers = { "x-title" = "my-app", "http-referer" = "https://example.com" }
override_headers = { "user-agent" = "my-agent/1.0" }
```

- `extra_headers`：请求**未携带**同名头（大小写不敏感）时才追加——适合补默认头。
- `override_headers`：**无条件覆盖**同名头——适合强制改写（含覆盖客户端的
  Authorization）。
- 键 trim 后为空剔除；trim 后同键冲突保留先出现者。
- `api_key` 的实现等价于一条 override_headers 的 `Authorization` 条目。

### keepalive_interval_secs

上游未就绪/重试期间，对客户端（流式响应）注入 SSE 注释行心跳的间隔。
0 = 完全关闭。非流式请求无心跳——客户端超时请自行调大其 HTTP 超时。

### proxy / proxy_username / proxy_password

上游出口代理。`proxy` 未设置时走系统/环境变量代理；设为具体 URL 后仅经该代理。
支持 `http`/`https`/`socks4`/`socks4a`/`socks5`/`socks5h`；URL 可内嵌
`user:pass@`。`proxy_username`/`proxy_password` 显式给出时优先于 URL 内嵌凭据。
**配置了用户名/密码但未配置 proxy URL 是校验错误**。三者由 `--clear-proxy`
一并清空。

### max_retry_backoff_secs

指数退避序列 5s→10s→20s→… 增长到该值后封顶继续重试。**0 = 所有重试零延迟**
（立即重试——对快速切换上游的场景有用，但对持续故障的上游会高频打点，慎用）。

### spool_limit_mb

上游响应缓冲上限（MB）。响应超过此大小视为**确定性失败，不重试**（重试注定
再次超限）——返回透传已缓冲部分。转发超大文件（模型权重下载等）时调大。

### connect_timeout_secs / read_timeout_secs

- connect：与上游建立 TCP/TLS 连接的超时。
- read：**两次读到数据之间**的最大间隔（同样钳制首字节等待 TTFB）。
  LLM 上游排队久（TTFB 数十秒）应调大；调过小会把「慢但活着」的上游变成
  确定性无限重试。
- 两者的 0 = 不设限。仅影响上游侧，客户端侧超时由客户端自己管理。

### max_body_mb

请求体大小上限（MB）。超出直接返回 413（带配置指引），**不转发**（客户端断开
时上游请求即中止，避免无谓计费）。0 = 不设限（慎用：内存/磁盘随负载无上界）。
未在 toml 显式配置时取 settings.json 的 `max_body_mb`（全局默认 128）。
50 万 token 会话/带图会话的请求体可达数十 MB——默认 128 已覆盖。

### disk_cache

磁盘缓存开关。开启时：请求体与上游响应超过内存驻留阈值（1 MiB）即溢写
`~/.aproxy/spool/<端口>/*.spooltmp` 临时文件，重试重放与响应回放流式读文件——
**进程内存与负载大小解耦**（实测每并发内存成本 -77%，SSD 上吞吐无损；
见 docs/benchmark-memory.md）。关闭 = 全内存旧行为。临时文件在请求结束/回放
完成/实例启动时清理，磁盘写失败按 SpoolFailed 终态处理（不打爆内存）。
未在 toml 显式配置时取 settings.json 的 `disk_cache`（全局默认开）。
低并发小流量实例可关（<1 MiB 的负载本就全程内存，不产生磁盘 IO）。

## 校验规则

启动时校验失败即拒绝启动（错误信息含文件位置与修复指引）：

1. `base_url` 非空、http/https 开头、无 `?`/`#`
2. `proxy` 若设置：URL 可解析、协议受支持、有主机
3. 配置了 `proxy_username`/`proxy_password` 则必须同时配置 `proxy`

## 归一化行为

加载后自动执行（保存时同样应用）：

- base_url 末尾 `/` 去除
- api_key/代理三项：trim；空白视为未设置
- 头表：键值 trim；空键剔除；同键保留先出现者
