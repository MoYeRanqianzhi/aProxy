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
# forward_only = false
# bounded_retry_paths = [ '/v1/messages/count_tokens' ]
# log_file = "D:/aproxy-logs/inst-a.log"
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
| `forward_only` | bool? | settings 层 | 仅转发模式：放弃重试/缓冲/心跳，请求体与响应流式直通 |
| `bounded_retry_paths` | string[]? | settings 层（空） | 受限重试路径（正则）：命中者失败 3 次即透传，不再无限重试 |
| `log_file` | string? | 无（随机命名） | 自定义守护日志文件路径；缺省按启动随机命名（见下） |

**优先级**（`max_body_mb`/`disk_cache`/`forward_only`/`bounded_retry_paths`
四个 Option 字段独有）：
toml 显式值 > settings.json 全局默认 > 内置默认（128 / true / false / 空）。
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
再次超限）——**直接回 502，不向客户端转发任何字节**（已缓冲的部分一并丢弃）。
若该请求已进入重试保活的 SSE 通道（HTTP 200 骨架已发出、状态行不可再改），
则以 `event: error` 事件（`error.type = proxy_spool_limit`）收尾替代 502。
转发超大文件（模型权重下载等）时调大。

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

### forward_only

**仅转发模式开关**（默认 `false`）。开启后实例**放弃本产品最核心的重试保障**，
换取请求体与响应的真流式直通：请求体边收边发上游、响应边收边回客户端——
**不缓冲、不重试、不落盘、不发心跳**。这是给「上游可信 + 客户端要真流式」场景
的**显式取舍**，不是普通开关：开启前请确认上游无需重试兜底，且客户端自己能
处理上游的错误与断流。

- **仍然强制**：`max_body_mb`——流式途中计数，超限即中止上游请求并回 413
  （复用既有文案）。这是本模式下唯一仍生效的限制。
- **不生效**：`disk_cache`/`spool_limit_mb`/`keepalive_interval_secs`/
  `max_retry_backoff_secs`——磁盘 spool 完全不参与（无缓冲可 spool）。
- **不再有**：无限重试、SSE 保活心跳、错误内容拦截（HTTP 200 携带 error 不再
  判失败）、缓冲后的原字节回放。
- **上游请求失败**：502 + 原因，**不重试**（同时记入 status 的「最近错误」）。
- **响应流中途中断**：**直接截断**——不注入任何上游未发出的字节，日志留痕
  （tracing 以 warn 记录错误与已转发字节数）。
- **客户端断开**：连接随之关闭（计费保护行为不变）。
- **`content-length` 仅响应侧保留**：响应字节未经变换，上游声明的长度仍精确
  （其余 hop-by-hop 头照旧过滤）。**请求侧不保留**——请求体不再整体持有，没有
  精确长度可回填，一律以 `chunked` 发往上游。
- 未在 toml 显式配置时取 settings.json 的 `forward_only`（全局默认 false）。
  **无对应 CLI 旗标**，只能写 toml 或 settings.json；改后
  `aproxy restart <端口或别名>` 生效。

### bounded_retry_paths

**受限重试路径**（正则数组，默认空 = 功能关闭）。命中的请求在上游「有响应的
失败」达到 3 次尝试后不再重试，把最后一次上游响应**原样透传**给客户端。

动机：部分上游对特定端点确定性报错（例如某些 API 聚合/镜像服务未实现客户端
依赖的辅助端点），无限重试只会让客户端永远等不到终态；错误秒回时客户端反而
能自行处理。哪些端点属于这一类**完全因上游而异**——因此哪些路径受限由你按
自己的上游配置，aProxy 不内置任何具体 URL。

匹配语义（每个模式对「`路径?查询串` 整体」做正则匹配，编译时自动锚定两端）：

- 不含元字符的普通路径即**精准匹配**：`"/v1/messages/count_tokens"` 只命中
  不带查询串的该路径，带任何查询串的请求都不命中
- **查询串必须显式出现在模式里**：`?` 是正则元字符，字面量写 `\?`（toml 强烈
  建议用单引号字符串免转义）；带查询串的精准匹配写
  `'/v1/messages/count_tokens\?beta=true'`
- **匹配的是收到的原始请求目标**（percent-encoded 原样、不解码）且**区分
  大小写**：查询串按客户端实际发送的编码形态书写——空格是 `%20`，模式里写
  字面空格不会命中 `/c?a=%20`
- `$` 与 `^` 是正则锚点**不是字面量**：查询串里真有 `$`/`^` 字符时写 `\$`/`\^`
  （写错的模式能通过校验但永不命中——这类「合法但死掉」的正则不会报错）
- 通配用正则语法：`'/v1/messages/count_tokens\?.*'` 命中该路径带任意查询串；
  `"/v1/messages/.*"` 命中 `/v1/messages/` 下全部路径
- 非法正则在启动校验时即报错拒绝，不会静默失效

```toml
# 示例：把记数端点设为「失败 3 次即透传」。单引号字符串内 \ 不需要双写
bounded_retry_paths = [
  '/v1/messages/count_tokens',
  '/v1/messages/count_tokens\?.*',
]
```

行为细节：网络错误不受此封顶（仍无限重试）；保活通道（SSE 骨架已发出的请求）
达上限以 `event: error` 事件收场。未在 toml 显式配置时取 settings.json 的
`bounded_retry_paths`（全局默认空）。改后 `aproxy restart <端口或别名>` 生效。

> 典型场景：Claude Code 走非官方 API（聚合/镜像上游）时 /compact 无限卡住、
> 最终超时报错，多半是上游未实现 compact 依赖的
> `POST /v1/messages/count_tokens`（确定性 404），该请求被无限重试、永远
> 等不到终态。把该路径加入 `bounded_retry_paths`（如上例）即可解决——失败
> 3 次即透传真实响应，compact 立即恢复。其他 agent 软件/其他端点的同类问题
> 同理，按实际路径配置。

### log_file

自定义守护日志文件路径（字符串，默认无 = 内置随机命名）。未设置时日志落
`~/.aproxy/logs/`，按启动时刻随机命名（十六进制时间戳-pid 格式，如
`19ac3f2e8b5d-1a2b.log`）——**每次启动（含 restart、restore 恢复）都是新文件**，
换端口后日志不断档；代价是文件名不含端口，**不要按端口猜文件名**，日志地址
一律以实例上报为准（`aproxy status`/`aproxy logs`/start 成功提示经 IPC 向
实例询问，客户端不拼路径）。

- **优先级**：CLI `--log-file <PATH>` > toml `log_file` > 内置随机名
  （与 `--baseurl` 等覆盖参数同款：CLI 值仅本次运行生效，不写任何配置文件）。
- **路径解析**：支持 `~` 展开；**相对路径相对 APROXY_HOME 解析**（守护进程
  的 cwd 不可靠，不按它解析）。
- **落盘与轮转**：自定义路径（含父目录创建）由守护进程负责；运行期轮转
  （settings.json 的 `log_rotate_mb`）对自定义文件同样适用。
- **有意不设 settings.json 全局默认层**（与 `max_body_mb`/`disk_cache`/
  `forward_only`/`bounded_retry_paths` 四字段的三层分层形成对照）：
  日志去向是每实例的运行习惯而非「同一台机器该统一」的全局策略，不存在
  「所有实例都该默认写同一个文件」的合理语义；toml 每实例显式配置 +
  CLI 临时覆盖已经覆盖全部场景。
- 改后 `aproxy restart <端口或别名>` 生效；重启后随机名会变（自定义路径不变），
  实例停止后旧日志按孤儿清理（见 behaviors.md 日志节）。

## 校验规则

启动时校验失败即拒绝启动（错误信息含文件位置与修复指引）：

1. `base_url` 非空、http/https 开头、无 `?`/`#`
2. `proxy` 若设置：URL 可解析、协议受支持、有主机
3. 配置了 `proxy_username`/`proxy_password` 则必须同时配置 `proxy`
4. `bounded_retry_paths` 每项必须是能编译的正则（非法模式启动即报错，
   错误含模式原文）

## 归一化行为

加载后自动执行（保存时同样应用）：

- base_url 末尾 `/` 去除
- api_key/代理三项：trim；空白视为未设置
- 头表：键值 trim；空键剔除；同键保留先出现者
