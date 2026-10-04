# 运行行为语义与排障

描述代理运行时的可观察行为——排障、编写依赖此代理的客户端、回答「为什么会
这样」时读此文件。配置字段本身的写法在 [config-toml.md](config-toml.md)。

## 目录

- [请求生命周期](#请求生命周期)
- [重试判定](#重试判定)
- [保活心跳](#保活心跳)
- [接入 agent 客户端](#接入-agent-客户端)（Claude Code、Codex、Gemini CLI 及其他）
- [流式响应处理](#流式响应处理)
- [磁盘缓存（spool）](#磁盘缓存spool)
- [仅转发模式（forward_only）](#仅转发模式forward_only)
- [外部转换器（request_transform / response_transform）](#外部转换器request_transform--response_transform)
- [多开与实例区分](#多开与实例区分)
- [控制通道（IPC）](#控制通道ipc)
- [日志](#日志)
- [自愈恢复](#自愈恢复)
- [排障速查](#排障速查)

---

## 请求生命周期

```
客户端 → aProxy(127.0.0.1:<端口>) → 上游 base_url

入站来源校验（两种模式都先走）：Host 不在白名单、或带了不在 allowed_origins
         里的 Origin → 本地 403，不转发、不重试（见 config-toml.md）

默认模式（forward_only = false）：
         1. 读入并缓冲完整请求体（超 max_body_mb 即 413，不转发）
         2. 透传 method/路径/查询/头（extra/override 头在此注入）
         2.5 配了 request_transform → 整个请求交给 format 程序改写
             （body/headers/url/method 全可变；改写产物贯穿后续全部重试）
         3. 上游响应缓冲到内存/磁盘 spool
         4. 判定成功/需重试（见下）
            需重试 → 退避后从第 2 步重来（请求体可重放）
            成功   → 配了 response_transform 则先交给 format 改写响应，
                     然后把响应回放给客户端
         保活适用的请求（见「保活心跳」）在 3~4 全程由保活通道驱动：先提交响应头，
         等待/缓冲/退避期间向客户端发 SSE 注释心跳；不适用的请求期间不向客户端
         写任何字节

仅转发模式（forward_only = true）：
         1. 不缓冲——请求体流式直发上游（途中计数，超 max_body_mb 即中止上游 + 413）
         2. 上游响应流式直回客户端（status/头原样透传，content-length 保留）
         3. 上游失败 → 502（不重试）；响应流中断 → 直接截断 + 日志留痕
            全程无重试、无心跳、无 spool（见「仅转发模式（forward_only）」）
```

- 客户端主动断开 → **立即中止**上游请求（计费保护），重试一并终止。
- 路径、查询参数、绝大多数请求头原样透传；仅 extra/override 头与鉴权覆盖
  会修改请求。
- 响应回放字节保真（status/头/体）；流式响应保持流式（chunked）。
- 上面是默认模式；`forward_only = true` 走另一条极简路径——见「仅转发模式
  （forward_only）」。

## 重试判定

以下情况判定失败并重试（**无次数上限**，直到成功或客户端放弃；受限重试路径
例外，见下）：

1. 连接/发送/读取中的网络错误（含超时——read_timeout_secs 的间隔超时）
2. 错误状态码 4xx / 5xx（原型决策：限流 429、临时鉴权波动等在真实上游高频
   出现，确定性 4xx 也重试是已接受的代价——重试期间客户端连接由心跳维持，
   表现为变慢而非失败）
3. **错误 JSON**：HTTP 200 但 body 是携带 error 的 JSON（某些上游用 200 包错误）
4. 流式响应中途断流（SSE 流尾出现错误标记、连接中断）

**受限重试路径**（`bounded_retry_paths` 配置，正则数组，默认空 = 功能关闭）：
命中任一模式的请求（模式对「`路径?查询串` 整体」做正则匹配、自动锚定两端，
普通路径即精准匹配；**查询串必须显式写进模式**，`?` 转义为 `\?`，通配用
`.*`——写法见 config-toml.md 的 `bounded_retry_paths` 节），总尝试第 1、2 次
的有响应失败照常重试，**从第 3 次尝试起任何一次有响应的失败立即原样透传并
终止重试**（确定性 404 场景恰为 3 次总尝试；前两次重试零延迟，总耗时约
3×上游 RTT）。网络错误不触发透传——它是真正的瞬时类，继续无限重试（其后
首个有响应的失败仍受同一规则约束）。

动机：部分上游对特定端点确定性报错（哪些端点**完全因上游而异**，因此交由
用户按自己的上游配置，aProxy 不内置任何 URL），重试到天荒地老也不可能成功，
只会让客户端永远等不到终态——错误秒回时客户端反而能自行处理。保活适用的请求
同样如此：达上限时响应头尚未提交（三次失败都很快，常态）就原样透传真实失败
响应；只有失败本身慢到响应头已被保活节拍以骨架提交，才以终态 SSE error 事件
收场（状态行已发出，真实状态码无法再回放）。

**不重试**（确定性失败，原样回放）：

- **受限重试路径（`bounded_retry_paths` 命中）从总尝试第 3 次起的有响应
  失败**（透传该次失败响应，见上）
- 响应超过 `spool_limit_mb`（重试注定再超限）
- 磁盘缓存模式下 spool 写盘失败（SpoolFailed → 502 或 SSE error 事件）
- **`forward_only` 模式下的全部失败**（该模式不分类型一律不重试：上游错误回
  502、响应流中断直接截断——见「仅转发模式（forward_only）」）

重试节奏：指数退避 5s→10s→20s→… 封顶 `max_retry_backoff_secs`（默认 320s；
0 = 立即重试）。**重试期间客户端连接一直被心跳维持**（见下），因此对客户端
表现为「这次请求变慢了」而非「失败了」。

## 保活心跳

保活适用的请求，aProxy 向客户端按 `keepalive_interval_secs`（默认 15s）发送 SSE
注释行（`: keepalive`）——SSE 规范中注释行被客户端忽略，但让中间层
（Nginx/浏览器/agent 客户端）知道连接活着，不触发读超时。

**哪些请求适用**：`keepalive_interval_secs` > 0、非 `forward_only`，且按
`keepalive_trigger` 命中（`accept`：Accept 含 `text/event-stream`；`body_stream`：
请求体顶层 `"stream": true`；`any`（默认）：任一）。判定在请求转换之前、按客户端
视角。

**提交点**：适用的请求从首轮起由保活通道驱动，响应头的提交只发生一次、由时间与
上游响应头推动——「需要重试」本身不触发提交。尚未提交时（每一次尝试、每一段
退避都适用）：
- 上游 2xx + 未压缩的 `text/event-stream`（且没配 `response_transform`）→ **立即**
  把这次尝试的上游真实 status 与响应头转给客户端（去掉 content-length 与
  hop-by-hop）；
- 一个保活间隔到点仍没有可提交的结果 → 提交**骨架头**（200 +
  `text/event-stream`），首字节因此 ≤ 一个保活间隔；配了 `response_transform` 时
  不提交上游真实头，只走骨架；
- 上游 2xx 但**不是** SSE（如 `"stream": true` 的 NDJSON 流、上游无视 stream 回了
  普通 JSON）→ 这次尝试期间暂停骨架提交：成功就原样直通（content-type 与正文都
  不被改写）；需要重试（如 200 + error JSON）则暂停随这次尝试结束，立即补提交
  骨架、照常心跳。代价：这类上游先回头、再迟迟不发体时，客户端在该次尝试期间
  收不到字节（最长约 `read_timeout_secs`）；
- 某次尝试成功 → 走与不保活相同的保真快速路径（status/头原样）——重试几次后
  很快成功的请求同样拿到上游真实头。
已提交之后才拿到非 SSE 的成功响应时无法撤回，照常回放并在日志里 warn，建议该
实例设 `keepalive_trigger = "accept"`。

**心跳全程覆盖**：提交之后，无论在等上游响应头（首字节）、上游请求在途、缓冲上游
流还是退避，都按间隔发注释心跳。响应体仍是**缓冲完整、校验无误后才回放**成功
那一次的原样字节——流中途出错就在同一个响应里继续重试，客户端只见到心跳，失败
尝试的数据不会混入。客户端断开（任一阶段）即中止在途上游请求、不再发起新请求。

- **上游编码**：保活适用的请求发往上游时 `accept-encoding` 一律改为 `identity`
  （往压缩流里插明文心跳会让客户端解压失败）——对「完全透传」的有意例外；上游
  无视该要求仍压缩时不提交真实头，等间隔到点提交骨架，成功体回放前完整解码
  （压缩体截断/损坏则以终态 SSE error 事件收场，不回放半截）。不适用保活的请求
  不改写。
- **没有保活通道的请求**：`"stream": false` 的普通请求没有可注入心跳的响应流——
  首字节延迟 = 完整生成时长，客户端需自行配置足够大的 HTTP 超时（重试可能长达
  退避封顶的数倍时间）；`forward_only` 实例同样没有。
- 0 = 关闭保活（不推荐：长退避时客户端/中间层会主动断连）。
- **客户端长等待后断开的日志提示**：已提交的响应等待 ≥590 秒后客户端断开，守护
  日志会 warn 一条提示「若客户端是 Claude Code，请设置
  CLAUDE_STREAM_IDLE_TIMEOUT_MS」（只是日志文案，行为对任何客户端都一样）。

## 接入 agent 客户端

**共同原理**：aProxy 为保证「流中途断开也能透明重试」，会缓冲完整响应、校验无误后
才回放，等待期间客户端只收到响应头和 SSE 注释心跳（`: keepalive`）。所以要看客户端
的流超时是按什么计时：

- **按字节计时**（两次收到字节的间隔，如 undici `bodyTimeout`、httpx read timeout、
  OpenCode `chunkTimeout`）：注释心跳会重置它，无需处理。
- **按 SSE 事件计时**（计时器在 SSE 解析之后）：注释心跳**重置不了**——主流 SSE 解析
  库（eventsource-stream、eventsource-parser、各官方 SDK 的解码器）都在解析层丢弃
  注释行，上层看不到它。这类超时必须调大或关闭，否则任何超过它的重试期或长生成都会
  被客户端断开重发：aProxy 随之按计费保护中止上游，新请求从头重试；客户端的重发次数
  用完后整轮失败。

### Claude Code（2.1.288 黑盒实测）

经 `ANTHROPIC_BASE_URL` 指向 aProxy，且**必须**设置 `CLAUDE_STREAM_IDLE_TIMEOUT_MS`：

```sh
export ANTHROPIC_BASE_URL=http://127.0.0.1:12345      # aProxy 的监听端口
export CLAUDE_STREAM_IDLE_TIMEOUT_MS=86400000          # 24 小时，实测可用
```

PowerShell 用 `$env:ANTHROPIC_BASE_URL = "..."`。也可写进 Claude Code 的
`~/.claude/settings.json` 的 `env` 字段（Claude Code 官方设置文档支持该字段，
对每个会话生效）：
`{"env": {"ANTHROPIC_BASE_URL": "http://127.0.0.1:12345", "CLAUDE_STREAM_IDLE_TIMEOUT_MS": "86400000"}}`。

- 三层流超时：首字节（约 360s）与字节级空闲（300s）能被注释心跳覆盖；**事件级空闲
  （默认 600s）覆盖不了**——注释与 `event: ping` 都不算事件。`86400000` 实测有效；
  `API_TIMEOUT_MS` **不控制**这道闸。
- 流式主请求是 `POST /v1/messages?beta=true`，`Accept: application/json` + 请求体
  `"stream": true`——默认 `keepalive_trigger = "any"` 才能让它进入保活通道（只配
  `accept` 会让它失去保活）。
- 2026-10-04 在 v0.1.0 上验收：上游 8 类故障持续 757s 后才成功、再慢速生成 140s，
  Claude Code 拿到完整结果，全程零重发。

### Codex（codex-cli 0.160.0 源码 + 黑盒实测）

用自定义 provider 指向 aProxy，并**必须**调大 `stream_idle_timeout_ms`。写在
`~/.codex/config.toml`（或 `$CODEX_HOME/config.toml`）：

```toml
model = "gpt-5"                     # 换成上游提供的模型名
model_provider = "aproxy"

[model_providers.aproxy]
name = "aproxy"
base_url = "http://127.0.0.1:12345/v1"
env_key = "OPENAI_API_KEY"          # Codex 从该环境变量取 key；aProxy 配了 api_key 时填任意非空值
wire_api = "responses"
stream_idle_timeout_ms = 86400000   # 24 小时
```

- `stream_idle_timeout_ms` 默认 300000（5 分钟），按 **SSE 事件**计时（SSE 解析库
  eventsource-stream 丢弃注释），注释心跳续不住；超时报 `idle timeout waiting for SSE`，
  随后自动重连，最多 `stream_max_retries` 次（默认 5，上限 100），用完本轮失败。
- 实测（aProxy v0.1.0 默认配置，上游持续 529）：设 60000 时，骨架头提交后恰好 60s
  断开重连，期间的 4 个心跳无效；设 86400000 后，只有心跳的等待持续 620s，最终拿到
  完整结果，全程 1 次请求。
- 请求是 `POST <base_url>/responses`，带 `Accept: text/event-stream` 与 `"stream": true`，
  `keepalive_trigger` 取 `accept` 或 `any` 都能进入保活通道。

### Gemini CLI（0.35.3 黑盒实测 + 源码）

用 API key 认证接入：`GOOGLE_GEMINI_BASE_URL=http://127.0.0.1:<端口>` 与
`GEMINI_API_KEY`（aProxy 配了 `api_key` 时可填任意值）。OAuth / Code Assist 登录
不走自定义地址，无法经 aProxy。

- 流式请求是 `POST /v1beta/models/<模型>:streamGenerateContent?alt=sse`，`Accept: */*`，
  请求体**没有** `stream` 字段，所以**不进保活通道**，走缓冲路径——结果正常（实测）。
- **限制**：缓冲路径要等上游完整成功才回响应头，而 Gemini CLI 的响应头 / 响应体超时是
  写死的常量（0.35.3 均为 300s，据源码新版响应头超时更短），没有配置可调——重试期加
  生成超过它就会失败。目前 aProxy 对此无解。
- **不能强行给它开注释保活**：Gemini CLI 锁定的 `@google/genai 1.30.0` 的 SSE 切分
  正则遇到开头的注释行后再也匹配不上，整段响应被吞掉、报 `Incomplete JSON segment at
  the end`（实测：注释前缀 → 4 次请求后失败；空行前缀 → 正常）。aProxy 目前没有按路径
  触发保活或改用空行心跳的配置。

### 其他客户端（源码调研，未逐个实测）

| 客户端（调研版本） | 流超时类型 | 接 aProxy 需要做什么 |
|---|---|---|
| Qwen Code 0.24.7 | 事件级空闲 240s + 流总时长上限 15 分钟 | **两个都要关**：环境变量 `QWEN_STREAM_IDLE_TIMEOUT_MS=0`、`QWEN_STREAM_MAX_LIFETIME_MS=0`（后者只能用环境变量） |
| dsh（DeepSeek Harness）`llm-pi-ai` 适配器（OpenAI / Anthropic 兼容网关） | 事件级空闲 300s | **必须**给该 provider 设 `streamIdleTimeoutMs`（如 `172800000`），写在 `$DSH_HOME/profiles/<profile>/cordis.patch.yml` |
| dsh `llm-deepseek` 适配器 | 事件级，但注释会重置（`onComment` 续命） | 默认即可 |
| pi 1.0.2（`@earendil-works/pi-coding-agent`） | 字节级 300s（`httpIdleTimeoutMs`） | 默认即可；它的 Anthropic 路径发 `Accept: application/json`，靠 `"stream": true` 进保活 |
| OpenCode 1.18.x | 字节级 `chunkTimeout` 300s、`headerTimeout` 300s | 默认即可；**不要**设 `timeout`（它限制含等待在内的整请求时长） |
| OpenCode 1.2.x | 无流超时 | 只设 provider 的 `baseURL` |
| Aider 0.86、Kimi CLI 1.52 | httpx read timeout 600s（字节级） | 默认即可 |
| Cline 4.1 | 运行时 fetch 默认 5 分钟（字节级） | 默认即可 |
| Roo Code 3.54 | `roo-cline.apiRequestTimeout` 600s | 默认即可（可设 0） |

另外，各客户端自带的重试会和 aProxy 叠加：客户端每重发一次，对 aProxy 都是一个新
请求，上一轮的上游请求随之中止。

### 通用注意

- 请求体 `"stream": false` 的普通请求没有保活通道（没有可注入心跳的响应流），首字节
  延迟等于完整生成时长，客户端需自行调大超时（Claude Code 为 `API_TIMEOUT_MS`）。
- aProxy 配了 `api_key` 时会同时覆盖 `Authorization` 与 `x-api-key`，客户端里的 key
  可以随便填。
- CLI 类客户端不发 `Origin`、Host 为 `127.0.0.1:端口`，不受入站来源校验影响。

## 流式响应处理

- `Content-Type` 为 `text/event-stream` 等 SSE 类型时按流式处理：上游的每个
  chunk 到达即顺序缓冲，成功后按原顺序回放。
- 错误检测对 SSE 是**行级增量**的：`data:` 行携带 error 标记（如
  `"type":"error"` / overloaded）即判失败重试——流已发给缓冲区、不发给客户端，
  因此客户端不会看到半截错误流。
- 行级判定语义：磁盘模式与内存模式一致——**例外是压缩响应体**。上游按
  `accept-encoding` 压缩时（gzip/deflate/br/zstd），检查路径先解出一份
  **仅供检查**的副本再判定（**检查解码、转发不解码**：发给客户端的始终是上游
  原始字节）；但这条解码只接在内存判定路径（`should_retry_response`）上。
  磁盘路径（响应 >1 MiB）的增量扫描器（`StreamErrorScanner`）吃的仍是原始
  字节流，压缩体上不判内容（仍按状态码判，仍解一份头部快照供日志预览）——
  **这是已知边界**：>1 MiB 的压缩错误体现实中不存在。
- 内存模式额外有整体 JSON 兜底判定（仅影响 >1 MiB 的单体 JSON 错误体，现实
  中不存在）。

## 磁盘缓存（spool）

`disk_cache` 开启时（默认）：

- 请求体与上游响应超过内存驻留阈值（1 MiB）即溢写
  `~/.aproxy/spool/<端口>/` 下的临时文件（`*.spooltmp`）；重试重放与响应回放
  流式读文件，进程内存与负载大小解耦（高并发大负载不再线性吃内存）。
- IO 走 OS page cache：SSD 上吞吐与纯内存模式持平。
- 临时文件清理三路保证：请求完成（含 Drop）、回放流 EOF、重试丢弃；实例启动
  时清空本端口目录回收崩溃残留。正常退出后目录应为空。
- 磁盘满/写失败：该请求按 SpoolFailed 终态处理（502 或 SSE error 事件），
  **不会**退化成内存堆积。
- 关闭（`disk_cache = false`）= 全内存行为：<1 MiB 的负载两种模式行为一致。
- **`forward_only` 模式下本机制完全不参与**：请求体与响应都不缓冲，也就无
  spool——见下节。

## 仅转发模式（forward_only）

`forward_only = true` 时实例走一条完全不同的极简路径：**不缓冲、不重试、不保活**。
请求体边收边发上游，响应边收边回客户端。**这是显式取舍**——放弃了本产品最核心
的重试保障，只适合「上游可信 + 客户端要真流式」的实例；开启前请确认上游无需
重试兜底、客户端自己能处理上游的错误与断流。

- **请求体**：流式直发上游，不缓冲不落盘；途中计数超 `max_body_mb` 即中止上游
  请求并回 413（复用既有文案）——**`max_body_mb` 是该模式唯一仍强制生效的限制**。
  空 body 也走流式，无特判。
- **响应**：`status` 与响应头原样透传（hop-by-hop 头照旧过滤，但 `content-length`
  **保留**——字节未经变换，上游声明仍精确）；响应体流式直回，不判定是否 SSE。
- **上游请求失败**：502 + 原因，**不重试**，并记入 status 的「最近错误」
  （否则该实例的「最近错误」会永久显示「无」）。
- **响应流中途中断**：**直接截断**，不注入任何上游未发出的字节；日志以
  `tracing::warn!` 记录错误与已转发字节数。
- **客户端断开**：连接随之关闭（计费保护行为不变）。
- **不走**：重试循环、SSE 保活骨架、心跳、错误内容拦截（HTTP 200 携带 error
  不再判失败）、`disk_cache`/`spool_limit_mb`/`keepalive_interval_secs`/
  `max_retry_backoff_secs` 全部不生效。

改配置后 `aproxy restart <端口或别名>` 生效（本模式**无 CLI 旗标**，只能写 toml
或 settings.json）。

## 外部转换器（request_transform / response_transform）

配了转换器（默认关）后，请求/响应在缓冲边界上被交给外部 format 程序改写
（一行 JSON 信封进出，协议与编写指南见 **aproxy-format skill**——本节只列
aProxy 侧行为语义）。

```
请求：缓冲完成 → [请求 format] → 发上游 → 重试重放（转换产物，不重复转换）
响应：spool + 成功判定 → [响应 format] → 回放客户端
```

- **可改写面**：请求侧 body/headers/url/method 全部可变（url 改写 = 协议
  转换的路径/域名迁移）；响应侧 body/headers 可变。
- **失败语义两侧不同**：
  - 请求侧转换失败（进程崩溃/超时/输出 error 行/输出违反信封协议）→ **502 +
    原因，请求不发上游、不重试**，记入 status 的「最近错误」。错误文案按成因
    分类（进程/管道层故障、format 自报错误、协议违规），按文案区分排障方向。
  - 响应侧转换失败 → **透传上游原始响应** + warn 日志（响应已在手，可用性
    优先）；保活通道同样透传（不发 SSE error 事件）。
- **persistent 进程池**：`mode = "persistent"` 时 format 进程以 while 循环
  逐行处理，池按并发扩容至 `pool_max`（超限排队——只加延迟不损吞吐），
  空闲 `idle_timeout_secs` 后回收（0=永不）；单请求超 `timeout_secs` 未回行
  则 kill 该 worker。**worker 回收主机制 = stdin EOF**：实例退出时 aProxy
  关闭管道，format 按协议义务自行退出。
- **池内自动重试一次**：persistent 池取到空闲期间已死的 worker（尚未产出
  任何输出就失败）时，自动换新 worker 重做一次——池状态问题，与请求内容
  无关。新开的 worker 也失败则按上面的请求侧/响应侧语义处理（502「format 进程
  意外退出且无输出」= 新 worker 同样起不来）。format 自报 error、协议违规、
  超时不重试。
- **一请求恰好一行输出**：信封协议没有请求序号，worker 的第 N 行输出只能靠
  「一请求一行、按序」对应第 N 个请求。format 往 stdout 多打一行（日志、横幅、
  `jq` 漏 `-c`）、输出非法 JSON 或空闲期间 stdout 冒出输出，该 worker 立即被
  剔除（否则下一个请求会读到上一个请求的输出，A 会话的 body/key 发往 B），
  当次请求 502「format 输出违反信封协议」。format 的 stderr 被丢弃。
- **key 轮换只在请求之间生效**：请求只转换一次，同一请求的所有重试重放同一
  份转换产物——沿用同一个 key，不会重试时换 key。
- **跨协议转换仅非流式**（官方 aproxy-format）：跨协议的 SSE 响应不支持，
  响应侧转换报错后透传上游原始响应；同协议 SSE 原样直通。
- **headers 语义**：信封头表键小写、整表替换；hop-by-hop 与 content-length
  不进信封（aProxy 按实际字节回填）；多值头仅保留首值（warn 留痕）；format
  输出的非法头名/头值丢弃 + warn；非法 method 同样 warn + 沿用原方法。
  响应侧强制剔除 content-length 与 content-encoding（字节已变换，旧声明
  失真）。**保活通道例外**：响应头一旦提交（保活适用的请求，配了
  `response_transform` 时提交的是 200 + text/event-stream 骨架）状态行与响应头
  不可再改——保活通道下 format 对响应 headers 的改写不生效，仅 body 转换生效。
- **command 支持 `~/` 展开**：`request_transform`/`response_transform` 的
  command 在加载时做 `~` 前缀展开（`~/.aproxy/bin/aproxy-format` 可直接
  使用）。
- **不进转换器**：`bounded_retry_paths` 命中且达到上限的透传路径（错误响应
  不经 format）；仅转发模式（与转换器互斥，启动报错）。
- **配置**：仅 toml 每实例字段（无 settings 全局层、无 CLI 旗标），子字段与
  校验见 config-toml.md；官方示例 aproxy-format 二进制单独发 Release。
- 改配置后 `aproxy restart <端口或别名>` 生效。

## 多开与实例区分

- 每份 config.toml 一个实例，`listen_addr` 端口必须互不相同——**实例按端口号
  区分**（注册表、IPC 管道名、spool 目录都含端口；守护日志自随机命名起例外，
  地址经 IPC 向实例询问）。
- 同端口不同监听地址的两个实例不能并存（会被拒绝启动）。
- 别名按配置文件路径匹配运行实例（端口变了依然有效）；端口号定位不依赖注册表
  （注册表丢失也能 stop/logs）。

## 控制通道（IPC）

`status`/`stop`/`logs` 探活/关停走命名管道 `aproxy-<端口>`，与代理端口完全
隔离——**控制通道绝不占用代理端口**，代理端口上只有 HTTP 转发流量。
IPC 通道故障（启动失败）只影响管理命令，代理转发继续（日志会报 error）。

## 日志

- 守护实例：`~/.aproxy/logs/` 下按启动时刻**随机命名**（十六进制时间戳-pid
  格式，如 `19ac3f2e8b5d-1a2b.log`；UTF-8 无 BOM；`aproxy logs` 跟随）。
  **每次启动（含 restart、restore 恢复）都是新文件**——换端口后日志不断档；
  文件名不含端口，**地址一律经 IPC 向实例询问**（`aproxy status`/`aproxy
  logs`/start 成功提示均来自实例上报的 log_path，客户端不拼路径），不要按
  端口猜文件名。自定义日志文件用 toml `log_file` 或 CLI `--log-file`
  （优先级 CLI > toml > 随机名；相对路径相对 APROXY_HOME 解析）。
- 启动失败的根因：`~/.aproxy/logs/startup.log`（配置错误、bind 失败写这里）。
- 截断策略：启动时 >2 MiB 清空（固定）+ 运行期每小时检查超 `log_rotate_mb`
  （默认 8 MB，0=不轮转）清空。
- 孤儿清理：`status` 时删除「活实例上报的 log_path ∪ `.restore` 记录的
  log_path」之外的 `*.log`（startup.log 除外，非 .log 文件不动）。推论：
  实例优雅停止后其日志在下次清理时删除；崩溃实例的日志由 `.restore` 引用
  保留到恢复成功；旧版按端口命名的日志在升级后视为孤儿清理。
- 所有日志对凭据打码，日志可安全粘贴分享：api_key/头值保留前 6 字符 + `***`；
  URL（base_url、代理、上游目标、请求的 `路径?查询串`、reqwest 错误里的 URL）
  的 userinfo（用户名或密码任一存在）整体打成 `***@`，**查询串保留键名、值变
  `***`**（`?beta=true` 显示为 `?beta=***`，status 的「最近错误」同理），片段
  `#…` 整体遮掉。用日志调试 `bounded_retry_paths` 时注意：日志里的查询串值
  是打码的，匹配仍按真实查询串。入站 403 的 warn 日志同样走此出口。
- 启动时监听地址不是回环地址会告警（start 的 stderr、守护日志与 startup.log、
  `aproxy doctor` 都有；配置了 api_key 时措辞更重——任何能连到该端口的主机都能
  用你的 key）。
- 「错误响应预览」行附带 `content-type` / `content-encoding`，并按
  `content-encoding` **解压后**展示正文（gzip / deflate / br / zstd）——上游压缩
  的错误页不再只显示一串 hex。**解压只用于日志与错误判定，转发给客户端的字节
  始终是上游原样**（保真透传不变）。

## 自愈恢复

- 实例 bind 成功即写恢复记录 `run/<端口>.restore`（启动参数快照），优雅退出删除。
- `aproxy restore` 按记录逐个拉起：幂等（已在运行跳过）、配置文件已删除的记录
  清理、就绪判定与 start 相同（按新 pid 定位实例，至多 8 秒）。实例落在**别的
  端口**时（记录里的端口对应的 toml 已改了端口、或 listen 端口为 0）报告
  实际端口并清理旧端口的恢复记录。
- 空清单静默成功退出 0——把 `aproxy restore` 配为任务计划程序登录项即实现
  崩溃/断电/重启后自动恢复。

`.restore` 绝不在 status 清理时删除——崩溃实例恰恰靠它存活到 restore 执行。

## 看门狗（watchdog）

默认开启的全局看护进程（与 aproxy 同一二进制、以内部标记分离启动，没有公开
子命令；~2-3MB 内存与实例数无关），把「进程级死亡 = 永久断流直到人工发现」降级为「约 1 秒自愈空窗」。
配置见 settings-json.md 的 watchdog 五字段。

**身份判定**：实例与看护者的「是不是原来那个进程」一律按 **pid + 进程创建时间**
核验，与二进制文件名无关——改名部署（如官方资产 `aproxy-<target>`）的实例照常
受看护，`stop --force` 也不再看镜像名。实例在注册表登记创建时间（`process_start`
字段）；旧版本实例没有该字段，按保守策略处理：挂死的旧版本实例不收养、对它们的
`--force` 需 IPC 确认（见 commands.md）。

**检测与恢复**：

- **进程死亡**（崩溃/taskkill/断电）：看护者对每个实例持有进程句柄，内核阻塞
  等待死亡信号（零轮询）。死亡后查 `.restore`：在 = 崩溃，立即重拉并等 IPC
  就绪；重拉失败进退避队列，按指数退避（1s→2s→4s…封顶 300s）由看护者主循环
  时间驱动重试（等待不阻塞心跳续写——否则长退避会让看护者被误判假死夺权）；
  不在 = 优雅退出，摘除看护。重拉后若实例落在**不同端口**（toml 改了端口），
  看护者清理旧端口的恢复/注册记录，并按新端口继续看护。
- **挂死**（进程活着但 runtime 死锁）：实例每 10s 向共享内存节写心跳
  （`aproxy-heart-<端口>`）；看护者每扫描周期（默认 30s）检查，过期 + IPC ping
  无响应才判定挂死，终止进程后走同一重拉路径。
- **crashloop 防护**：同一实例连续重拉失败达 `watchdog_max_restarts`（默认 5）
  次即放弃，写 startup.log 大声报出，`.restore` 保留供人工 `aproxy restore`。
- **唯一性**（选举规范）：claim 文件 `run/watchdog.claim` 记录看护者身份
  （PID + 进程创建时间 + 心跳）；任何时刻至多一个有效看护者——并发拉起由
  claim 原子接管裁决，多守护并发发现缺席时由「存活实例 PID 最小者」发起。
  PID 复用冒名被进程创建时间比对拒绝。
- **互保**：看护者崩溃/假死时守护不受伤（完全解耦）；每个守护每 5 分钟自检，
  发现缺席即按选举规范补种；`aproxy start` 启动新实例时若看护者缺席也会补拉。
- **闲置自灭**：全部实例清零后看护者等待 `watchdog_idle_exit_secs`（默认
  300s）自动退出并清理 claim，系统回到零常驻。

**诚实边界**：进程级死亡时在途请求全部中断（客户端见连接 reset），重拉后
客户端需重发请求——看门狗救的是「之后没人服务的永久断流」，不是在途请求。
对自带重试的客户端（Claude Code 等）表现为「卡一下」而非「会话死了」。
升级二进制后请一并重启看护者（旧看护者会用旧 exe 重拉实例）。
平台：看门狗在 Windows 与 Linux 上可用，macOS 暂不支持。

## 二进制更换阶段（install 运行期）

`aproxy install` 滚动重启期间，正在换血的实例经 IPC 广播进入「二进制更换
阶段」——`aproxy status` 对该实例显示「二进制更换中」，且语义为：
- 该实例随时会被 stop + 新二进制重拉（秒级窗口）
- **外部不要在此窗口 stop/kill 该实例**（与 install 拉锯）
- 看门狗进入差异化模式：此窗口内的实例死亡先复查 5×3s 等回归（install 主动
  重启的正常形态），复查耗尽才走重拉；install 自身挂死由看护者保活续作；
  守护自检补种被抑制（防旧版本看护者复活拉锯）
- ACK 失败处置：某实例多轮未表达「进入更换阶段」= 该实例有隐患（旧版本/
  故障），install 会先按轮次 restart 收敛（顺带拉到安装器版本）；终失败则
  abort 安装并明确指出问题实例——**不强杀**（绝对避免服务中断）。skill 指引：
  将该实例关闭后重试安装
- **滚动重启失败时的服务回滚**：某实例被优雅停止后，新二进制若起不来（新版本对
  旧配置校验更严、被杀软拦截、8 秒内没就绪），install 会先把该实例的恢复参数
  回写进 `.restore`，再用**旧二进制**按原参数把它拉回，并中止滚动——其余实例
  不再动，install 以非零退出（实例保持旧版本、服务恢复）。回滚的只是这一个
  实例的服务：bin 里的新二进制与状态机阶段都不回退（阶段落 `failed`，原因在
  `install.state` 的 `last_error`），自动续作也不会再重试；排除原因（看
  startup.log）后重新执行 `aproxy install` 继续向前。旧二进制拉回也失败时，
  实例下线但 `.restore` 已保住，排除原因后 `aproxy restore` 恢复。
- **旧二进制的位置**：交换前保留的旧二进制是回滚用的——Windows 为
  `bin/aproxy.old.exe`（也是入口脚本的 fallback），unix 为 `bin/aproxy.old`
  （交换前硬链接或复制）；安装完成后的清理阶段删除（Windows 上被运行中的
  镜像锁住时保留到下次）。Windows 上交棒后滚动由续作进程完成，回滚结果看
  `install.state` 与 `aproxy status`，不看命令的退出码。

## skill 文档更新（install 支线）

install 时随二进制**并行**更新 `~/.aproxy/skills/` 下的 skill 文档（aproxy-cli 与
aproxy-format，同一份总包，与二进制同一版本 tag）。非强制：下载失败重试后放弃，**安装照常成功**；子状态在
install.state 的 `skill` 字段可查。`--skills-only` 单独更新。安装到 agent 侧
（如 `~/.claude/skills/`）由用户/agent 自行链接——install 不越界触碰各 agent
目录。`--continue` 续作时 failed 不自动重试（避免每次续作拖一遍下载）。

## 下载代理（≠ 请求代理）

两套代理严格分离，勿混淆：
- **请求代理**：config.toml 的 `proxy`——管**上游 API 转发**
- **下载代理**：settings.json 的 `download_proxy` / `aproxy install
  --download-proxy`——只管 **install 的下载**（二进制/skill）
- 两者都未配置时 install 回退环境变量（HTTPS_PROXY 等系统代理），用户零
  配置可用；错误信息与 `config --show` 同样对内嵌凭据打码

## 排障速查

| 症状 | 原因与处理 |
|---|---|
| 启动报「端口被占用」 | 真被其他程序占用；`netstat -ano \| findstr <端口>` 找进程 |
| 启动报「无权限或被系统保留」 | Hyper-V/WinNAT 排除区间：`netsh interface ipv4 show excludedportrange protocol=tcp` 换端口 |
| 启动超时「未就绪」 | 读 `~/.aproxy/logs/startup.log`（配置/绑定错误）；实例日志地址经 `aproxy status` 向实例询问（按启动随机命名，不按端口拼路径） |
| status 看到实例但 stop 说无响应 | 实例已死注册表未清——按提示 taskkill；下次版本会自清 |
| `stop` 要求指定端口 | 多实例安全机制：先 `aproxy status` 再指定端口/别名/all |
| 客户端等很久才收到回复 | 正常——上游在重试，心跳在维持连接；`aproxy logs <端口或别名>` 看重试原因 |
| 客户端非流式请求超时 | `"stream": false` 的请求没有保活通道（没有可注入心跳的响应流）：调大客户端 HTTP 超时或调小 max_retry_backoff_secs |
| Claude Code 等待约 10 分钟后自行断开/重发请求（日志有「客户端在已提交的响应上等待约 N 秒后断开」） | 未设 `CLAUDE_STREAM_IDLE_TIMEOUT_MS`：Claude Code 的事件级空闲超时（默认 600s）不被注释心跳重置。设为 `86400000`（shell 环境变量或 `~/.claude/settings.json` 的 `env`），详见本文「接入 agent 客户端」；`API_TIMEOUT_MS` 不控制这道闸 |
| Codex 约 5 分钟后报 `idle timeout waiting for SSE` 并 `Reconnecting...`，几次后本轮失败 | 未调大 Codex provider 的 `stream_idle_timeout_ms`（默认 300000，按 SSE 事件计时，注释心跳续不住）。在 `~/.codex/config.toml` 的 `[model_providers.<id>]` 下设 `stream_idle_timeout_ms = 86400000`，详见「接入 agent 客户端」 |
| Qwen Code 约 4 分钟后断开、或满 15 分钟必断 | 设环境变量 `QWEN_STREAM_IDLE_TIMEOUT_MS=0` 与 `QWEN_STREAM_MAX_LIFETIME_MS=0`（源码调研，未实测） |
| Gemini CLI 报 `Incomplete JSON segment at the end` | 响应流里出现了 SSE 注释（Gemini CLI 锁定的 `@google/genai 1.30.0` 解析不了）。aProxy 默认不会给 Gemini 流发注释心跳；若上游或其他中间层插入了注释即会触发 |
| 流式请求没有心跳 / 首字节等很久 | 看 `keepalive_trigger`：只配了 `accept` 时，Accept 不含 `text/event-stream` 的流式请求（如 Claude Code）进不了保活通道；默认 `any` 同时认请求体 `"stream": true`。`aproxy config --show` 核对生效值 |
| Claude Code /compact 无限卡住/超时（走非官方 API） | 上游（聚合/镜像服务常见）未实现 compact 依赖的 `POST /v1/messages/count_tokens`，确定性 404 被无限重试、客户端永远等不到终态。解决：该实例 toml 的 `bounded_retry_paths` 加 `'/v1/messages/count_tokens\?.*'`（或客户端实际使用的确切路径），`aproxy restart <端口或别名>` 生效——失败 3 次即透传真实响应。其他 agent 软件/其他端点的同类问题同理 |
| 日志刷「受限重试路径达到尝试上限，透传最后一次上游响应」 | 该请求命中 `bounded_retry_paths`：失败 3 次即透传，属预期行为；不想受限就从配置移除对应模式并 restart |
| 客户端收到 403，文案提到 `allowed_origins` / `allowed_hosts` | 入站来源校验拒绝了请求（未转发上游）：浏览器/Electron 类客户端会发 `Origin`，默认全部拒绝。把文案里的 Origin（或主机名）原样加入 toml 的 `allowed_origins`（或 `allowed_hosts`），`aproxy restart <端口或别名>`；信任场景可写 `["*"]` 关闭该项校验。守护日志有「入站请求被拒绝」warn |
| 413 Request Entity Too Large | 请求体超 `max_body_mb`：调大 toml/settings 的值或设 0 |
| 中文乱码 | 控制台代码页问题；进程入口已自动切 65001，若仍乱查终端自身设置 |
| 错误响应预览是一串 hex | 该响应体确实是二进制，或其 `content-encoding` 本地无法解码（未知编码/内容损坏/解压后超 8 MiB）——看同一行的 `content-encoding` 字段判断是什么编码 |
| spool 目录残留 .spooltmp | 异常退出的残留；重启该端口实例即清理 |
| 实例崩溃后被自动拉起但配置是旧的 | 看门狗按 .restore 记录重拉——改配置后执行 `aproxy restart <端口>`，重启成功即以当前参数重写记录 |
| 怀疑看护者没在运行 | `status` 目前不展示看护者状态。看 `~/.aproxy/run/watchdog.claim` 是否存在、其中 pid 是否在世；守护日志会记「看护者缺席，已由守护补种」。看护者缺席时守护 5 分钟内自动补种，启动新实例（`aproxy start`）也会补拉 |
| 磁盘缓存想关 | toml 或 settings 写 `disk_cache = false`，重启实例 |
| 请求 502 且错误含「format」 | 外部转换器失败：按文案区分（「启动失败」=command 路径、「报告转换失败」=format 业务判定、「超时」=调 timeout_secs、「违反信封协议」=format 往 stdout 多打了行/输出非法 JSON）——详见 aproxy-format skill 排障节 |
| 配置了转换器但响应没转换 | response_transform 是**独立配置**（忘配 = 响应原样）；或响应转换失败透传了原样（查 `aproxy logs` 的「响应转换失败」warn）；bounded_retry 透传路径本就不进转换器 |
| 启动报「forward_only 与外部转换器互斥」 | 两配置同开是矛盾（forward_only 不缓冲、转换器要全量 body）——留一个 |
| 想关掉重试（要真流式直通） | 该实例 toml 或 settings 写 `forward_only = true`，`aproxy restart <端口或别名>` 生效——**代价是放弃重试/缓冲/心跳保障**，仅上游可信时用 |
| `forward_only` 下上游报错直接 502 / 流中断被截断 | 符合预期：该模式不重试、不注入上游未发出的字节，错误原样暴露给客户端（日志有记录） |
| settings.json 报「解析失败」 | 修复 JSON 或删除该文件（回退默认，别名需重新 add） |
