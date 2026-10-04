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
# keepalive_trigger = "any"
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
# allowed_hosts = ["myproxy.local"]
# allowed_origins = ["http://localhost:5173"]
# request_transform = { command = "~/.aproxy/bin/aproxy-format", args = ["run"], mode = "persistent", extra = "~/.aproxy/agg.toml" }
# response_transform = { command = "~/.aproxy/bin/aproxy-format", args = ["run"], mode = "persistent", extra = "~/.aproxy/agg.toml" }
```

## 字段总表

| 字段 | 类型 | 默认值 | 说明 |
|---|---|---|---|
| `base_url` | string | （空，必填） | 上游 API base URL；末尾 `/` 自动去除 |
| `listen_addr` | string | `"127.0.0.1:12345"` | 本地监听地址，须含端口 |
| `api_key` | string? | 无 | 快捷鉴权（同时覆盖 `Authorization: Bearer` 与 `x-api-key`） |
| `extra_headers` | map | 空 | 仅当请求未携带该头时追加（大小写不敏感判定） |
| `override_headers` | map | 空 | 无条件覆盖请求头（大小写不敏感匹配） |
| `keepalive_interval_secs` | u64 | 15 | 保活 SSE 心跳间隔秒（也是首轮提交骨架头的等待上限）；0=关闭保活 |
| `keepalive_trigger` | string? | settings 层（`"any"`） | 哪些请求走保活通道：`"accept"` / `"body_stream"` / `"any"`；非法值启动报错 |
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
| `allowed_hosts` | string[]? | settings 层（空） | 入站 Host 白名单（防 DNS 重绑定）：Host 校验生效时**追加**放行的主机名；含 `"*"` 关闭 Host 校验 |
| `allowed_origins` | string[]? | settings 层（空） | 入站 Origin 白名单：默认拒绝一切携带 Origin 头的请求，列表项精确放行；含 `"*"` 关闭 Origin 校验 |
| `request_transform` | table? | 无 | 外部转换器（请求侧）：交给 format 程序改写 body/headers/url/method；失败 502 不重试；与 forward_only 互斥 |
| `response_transform` | table? | 无 | 外部转换器（响应侧）：改写上游响应后回放；失败透传原样 |

**优先级**（`max_body_mb`/`disk_cache`/`forward_only`/`bounded_retry_paths`/
`allowed_hosts`/`allowed_origins`/`keepalive_trigger` 七个 Option 字段独有）：
toml 显式值 > settings.json 全局默认 > 内置默认（128 / true / false / 空 /
空 / 空 / `"any"`）。其余字段无 settings 层：toml 显式值 > 内置默认。
注意 `allowed_hosts`/`allowed_origins` 在 toml 里写 `[]` 也算「显式值」——
它等同未配置（内置默认策略），因此可用来把 settings.json 的全局列表在单个
实例上恢复成内置默认。

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

设置后等效于把 `Authorization: Bearer <api_key>` 覆盖进每个转发请求，并同时用同一个值覆盖
`x-api-key`（Anthropic 风格上游用它携带原始 key，只覆盖 Authorization 会让客户端
原带的 x-api-key 漏到上游）。适用场景：
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

### keepalive_interval_secs / keepalive_trigger

**保活通道**：保活适用的请求从首轮起先向客户端提交响应头，再按
`keepalive_interval_secs` 间隔发 SSE 注释行（`: keepalive`）心跳，覆盖等首字节、
上游在途、缓冲上游流与退避的全过程；响应体仍是缓冲完整、校验无误后才回放
（失败尝试的数据不会混入）。语义细节见 behaviors.md「保活心跳」。

- **适用条件**：`keepalive_interval_secs` > 0、非 `forward_only`，且按
  `keepalive_trigger` 命中。`0` = 完全关闭保活；`forward_only` 下无效。
- **`keepalive_trigger`**（字符串，默认 `"any"`，只认小写原文）：
  - `"accept"`：客户端 `Accept` 头含 `text/event-stream`（0.1.0 之前的唯一判定）
  - `"body_stream"`：客户端原始请求体是 JSON 对象且顶层 `"stream": true`
    （只看请求体字段，与 URL 无关；磁盘溢写的大请求体同样只看顶层键）
  - `"any"`：两者任一。真实 Claude Code 的流式主请求是
    `Accept: application/json` + 请求体 `"stream": true`，只看 Accept 的旧判定
    让它永远进不了保活通道，所以默认必须把请求体也算进来
  - 判定在 `request_transform` **之前**、按客户端视角，转换器改写 Accept/请求体
    不影响保活选择。
  - **`"stream": true` 的非 SSE 流**（如 Ollama 原生 API 的 NDJSON）：上游回 2xx
    非 SSE 头时该次尝试不提交骨架，不需要重试就原样直通；若上游常需重试（重试
    期间会提交 SSE 骨架，非 SSE 的成功体只能落进 SSE 响应里），建议该实例设
    `keepalive_trigger = "accept"`。
- 非法取值：toml 里的由 start 校验点名拒绝；settings.json 里的由 `aproxy doctor`
  报 error，且以它为全局默认的实例启动失败。toml 显式值 > settings.json >
  内置 `"any"`；`aproxy config --show` 展示生效来源。改后
  `aproxy restart <端口或别名>` 生效。无 CLI 旗标（`--keepalive-secs` 只管间隔）。
- **上游编码改写（对「完全透传」的有意例外）**：保活适用的请求发往上游时
  `accept-encoding` 一律改为 `identity`——往压缩流里插入明文心跳会让客户端解压
  失败。客户端照旧拿到合法响应，代价只是上游到本机这一段多传一些字节；不适用
  保活的请求不改写。
- **没有保活通道的请求**：`"stream": false` 的普通请求（没有可注入心跳的响应
  流，首字节延迟 = 完整生成时长，客户端需自行调大超时）、`forward_only` 实例、
  `keepalive_interval_secs = 0`。

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

用日志调试匹配时注意：日志里请求路径的查询串值会被打码显示（`?beta=true`
显示为 `?beta=***`），这只影响展示——匹配始终针对收到的原始请求目标，模式
仍按真实查询串书写。

行为细节：网络错误不受此封顶（仍无限重试）；保活适用的请求达上限时，若响应头
尚未提交（失败都很快，常态）同样原样透传真实失败响应，只有响应头已被保活节拍
以骨架提交（失败本身慢于一个保活间隔）才以终态 `event: error` 事件收场（状态行
已发出，真实状态码无法再回放）。未在 toml 显式配置时取 settings.json 的
`bounded_retry_paths`（全局默认空）。改后 `aproxy restart <端口或别名>` 生效。

> 典型场景：Claude Code 走非官方 API（聚合/镜像上游）时 /compact 无限卡住、
> 最终超时报错，多半是上游未实现 compact 依赖的
> `POST /v1/messages/count_tokens`（确定性 404），该请求被无限重试、永远
> 等不到终态。把该路径加入 `bounded_retry_paths`（如上例）即可解决——失败
> 3 次即透传真实响应，compact 立即恢复。其他 agent 软件/其他端点的同类问题
> 同理，按实际路径配置。

### allowed_hosts / allowed_origins

**入站来源校验**（字符串数组，默认空 = 内置默认策略）。代理会把上游密钥
注入每个转发请求，而本机浏览器里的任意网页都能向 `127.0.0.1` 发请求——这两
项防的是网页借本机代理花你的额度。CLI 类客户端（Claude Code 等）既不发
`Origin`，Host 也恒为 `127.0.0.1:端口`，不受影响。

- **Host 校验**（`allowed_hosts`，防 DNS 重绑定）：监听回环地址（127.0.0.0/8、
  `::1`、`localhost`），**或** `allowed_hosts` 非空时生效。生效时放行
  `localhost` / `127.0.0.1` / `[::1]`、监听地址自身的主机部分（`0.0.0.0` /
  `[::]` 这类通配地址除外）、以及列表条目——列表是**追加**而不是替换。比较时
  忽略端口（Host 头与条目里写的端口都不参与）、不区分大小写。监听非回环地址
  且列表为空时**不做** Host 校验（局域网/容器客户端的 Host 五花八门，默认
  拦截会破坏既有用法；启动时另有非回环告警）。
- **Origin 校验**（`allowed_origins`）：任何携带 `Origin` 头的请求默认拒绝
  （只有浏览器与 Electron/WebView 类客户端会发），除非精确匹配列表项——
  不区分大小写、忽略末尾 `/`，写法与浏览器发出的完全一致
  （如 `"http://localhost:5173"`）。与监听地址无关，非回环监听同样生效。
- 两项都支持 `"*"`：含 `"*"` 即关闭对应校验。**空列表 `[]` 等同未配置**
  （不是「全部拒绝」或「全部放行」）。
- 被拒请求在本地直接返回 **403**，响应文案点名对应配置项与修法，并写一条
  warn 日志。它**从未转发上游、不注入 `api_key`、不进入重试循环、不计入
  请求数**——只作用于从未转发过的请求，对放行的请求「无限重试」毫无改变。
- 会发 `Origin` 的客户端（Cherry Studio、Open WebUI 等基于 Electron/浏览器的
  应用）升级后需要配置 `allowed_origins`，否则全部 403；真实 Claude Code
  实测不发 `Origin`、Host 为 `127.0.0.1:端口`，无需配置。
- settings.json 同名字段是全局默认；toml 显式值优先。改后
  `aproxy restart <端口或别名>` 生效。

```toml
allowed_hosts = ["myproxy.local"]            # 容器内用 myproxy.local 访问本机代理时
allowed_origins = ["http://localhost:5173"]  # 本机前端页面/Electron 客户端的 Origin
```

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

### request_transform / response_transform

外部转换器（table，默认无 = 功能关闭）：把整个请求/响应装进一行 JSON 信封
交给外部 format 程序改写后收回——实现 OpenAI ↔ Anthropic 等协议转换、
多 key 轮换、多模型多渠道聚合（newapi 式）。aproxy 本体不内置任何转换器，
全部用户配置；写 format 与配置的完整指南见 **aproxy-format skill**。

```toml
request_transform  = { command = "~/.aproxy/bin/aproxy-format", args = ["run"], mode = "persistent", extra = "~/.aproxy/agg.toml" }
response_transform = { command = "~/.aproxy/bin/aproxy-format", args = ["run"], mode = "persistent", extra = "~/.aproxy/agg.toml" }
```

`command` 与 `extra` 请写 `~/` 前缀或绝对路径——守护进程的工作目录不可靠
（取决于谁在哪个目录执行了 start / restore），相对路径会在换个启动方式后
找不到。响应侧同样要配 `extra`（官方 aproxy-format 靠它读聚合配置，缺了响应
侧必失败并透传）。

子字段：

| 字段 | 类型 | 默认 | 说明 |
|---|---|---|---|
| `command` | string | （必填） | format 程序命令；不走 shell 按 argv 执行（无注入面）；`~/` 前缀由 aProxy 展开，写 `~/` 或绝对路径最稳 |
| `args` | string[] | `[]` | 程序参数（如官方示例的 `["run"]`） |
| `mode` | string | `"spawn"` | `spawn`=每请求一次性进程；`persistent`=持续进程池（轮换/计数状态必须用它） |
| `pool_max` | u32 | 4 | persistent 池上限（并发 worker，超限排队）；0 启动报错 |
| `idle_timeout_secs` | u64 | 300 | persistent worker 空闲回收秒；0=永不回收 |
| `timeout_secs` | u64 | 30 | 单请求转换超时秒；0=不限；超时 worker 被剔除 |
| `extra` | string? | 无 | 原样透传进信封（格式无要求，format 自解；官方示例传聚合配置路径，其中的 `~/` 由 aproxy-format 自己展开——aProxy 不展开 extra） |

语义要点：

- **失败语义两侧不同**（用户拍板的设计，不要混淆）：请求侧转换失败
  （进程崩溃/超时/format 报 error 行/输出违反信封协议）→ **502 + 原因，请求
  不发往上游、不重试**；响应侧失败 → **透传上游原始响应** + warn（响应已在手，
  可用性优先）。错误文案按成因分类（进程/管道层故障、format 自报错误、协议
  违规各不相同），不要把它们都当成「配置错了」。
- **池内唯一的重试**：persistent 池取到空闲期间已死的 worker（还没产出任何
  输出就失败）时，池会**自动换新 worker 重试一次**——这是池状态问题，与请求
  内容无关；看到 502「format 进程意外退出且无输出」说明新开的 worker 也死了
  （format 本身起不来）。format 自报 error、协议违规、超时一律不重试。
- **输出必须是恰好一行合法信封**：format 往 stdout 多打了一行（日志/横幅、
  `jq` 漏了 `-c`）、输出非 UTF-8 或非法 JSON、`body` 与 `body_b64` 并存，都
  按协议错误处理：该 worker 被剔除，请求 502（错误含「format 输出违反信封
  协议」）。aProxy 会丢弃 format 的 stderr，排障日志请写进你自己的文件。
- **key 轮换只在请求之间生效**：请求体只转换一次，同一请求的全部重试重放
  同一份转换产物——沿用同一个 key，不会在重试时换 key。
- **与 `forward_only` 互斥**：同开启动即报错（forward_only 不缓冲请求体，
  转换器需要全量 body）。
- **仅 toml 每实例配置**：无 settings.json 全局默认层、无 CLI 旗标（转换是
  场景特定功能，不同实例连不同上游用不同 format——设计决策）。
- 转换是**整流**的：请求体缓冲完成后转换一次（重试全程重放转换产物）；
  响应在重试判定成功后、回放前转换。SSE 响应整流转文本交给 format。官方
  aproxy-format 的**跨协议转换只支持非流式**：跨协议的 SSE 响应不支持，响应侧
  报错后按上面的语义透传上游原始响应（客户端收到渠道协议格式的流）；同协议
  SSE 原样直通。
- 重试重放的是转换后的请求（转换不重复执行）；`bounded_retry_paths` 命中
  的透传路径不进响应转换（错误响应不经 format）。
- 官方示例二进制 `aproxy-format`（协议转换 + 轮换 + 聚合）单独发 Release，
  落 `~/.aproxy/bin/`；版本独立于 aproxy alpha 线。
- 改后 `aproxy restart <端口或别名>` 生效。

## 校验规则

启动时校验失败即拒绝启动（错误信息含文件位置与修复指引）：

1. `base_url` 非空、http/https 开头、无 `?`/`#`
2. `proxy` 若设置：URL 可解析、协议受支持、有主机
3. 配置了 `proxy_username`/`proxy_password` 则必须同时配置 `proxy`
4. `bounded_retry_paths` 每项必须是能编译的正则（非法模式启动即报错，
   错误含模式原文）
5. `request_transform`/`response_transform`：`command` 非空；
   `mode = "persistent"` 时 `pool_max >= 1`；与 `forward_only` 不同存
   （同开报互斥错误）

## 归一化行为

加载后自动执行（保存时同样应用）：

- base_url 末尾 `/` 去除
- api_key/代理三项：trim；空白视为未设置
- 头表：键值 trim；空键剔除；同键保留先出现者
- transform 的 `command` trim；trim 后为空 = 该字段视为未设置
