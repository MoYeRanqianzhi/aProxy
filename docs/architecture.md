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
`\\.\pipe\aproxy-<home_id>-<端口>`（unix 为 `<run 目录>/<端口>.sock`），杜绝控制路径与
客户端请求路径重叠。`home_id` 是 run 目录规范化路径的 SHA-256 前 16 位十六进制：
Windows 的管道名全机共享，带上它，一个 home（`APROXY_HOME`）的命令就只能控制本
home 的实例。新实例同时在 0.1.0 的管道名 `\\.\pipe\aproxy-<端口>` 上应答，供 0.1.0
的 CLI 与安装器在升级窗口里找到它（`src/compat_0_1_0.rs`，0.2.0 删除）。

启动顺序：bind TCP → 独占创建控制端点（失败即退出，写 startup.log）→ 写 `.pid` →
写 `.restore` → 心跳 → 服务。由此「有 `.pid` 记录 ⇒ 端点在应答」（挂死除外）。客户端
核对应答者自报的 pid 与连接对端进程（`GetNamedPipeServerProcessId` / `SO_PEERCRED`）：
端点名可预测，抢先占住它的进程只能报出自己的 pid，普查再拿这个 pid 与注册记录比对。

线上格式（v1，一连接一来回，各一行 JSON）：请求 `{"v":1,"op":"ping|shutdown|prepare_swap"}`
（可带 `args` 对象）；成功应答 `{"v":1,"ok":true,"result":InstanceStatus}`，所有 op 都回同一份
状态快照——`instance`（即 `.pid` 记录）、`run_dir`、`state`（`serving`/`swap_prepared`/
`stopping`，不认识的值读作 `unknown`）、`activity`、`ops`；失败应答
`{"v":1,"ok":false,"error":{"code","message"}}`。`v` 只在语义不兼容时递增，加 op、加字段、
加错误码、加状态值都不递增。没有 `v` 的请求与应答属于 0.1.0，由 `src/compat_0_1_0.rs` 应对。

上图的「重试循环 / spool / 回放」是默认模式的主流程；`forward_only` 模式下这两
环整体旁路（见「仅转发模式（forward_only）」）。

## 模块

| 模块 | 职责 |
|---|---|
| `src/main.rs` | 进程入口：控制台代码页、配置文件定位、日志初始化、子命令分派（仅 ~110 行） |
| `src/cli.rs` | clap 命令树定义（`Cli`/`Commands`/`AliasCmd`/`ConfigArgs`），只承载定义不含逻辑 |
| `src/commands/` | 十一个子命令各自一文件（start/status/stop/restart/restore/alias/doctor/find/logs/config/install），共享 target 解析在 `mod.rs` |
| `src/server.rs` | 服务承载：`serve_forever` 主循环、停止信号、日志初始化、配置错误落盘 startup.log |
| `src/proxy.rs` | 转发核心：入站来源校验（Host/Origin）、hop-by-hop 过滤、内存+磁盘双模 spool、错误判定、保活通道与提交点、断开保护、仅转发模式 |
| `src/transform.rs` | 外部转换器编排：stdin/stdout 一行 JSON 信封交给外部 format 程序改写（spawn 与 persistent 统一进程池、空闲 reaper、超时/崩溃 worker 剔除）；信封契约在独立 crate `aproxy-envelope` |
| `src/decode.rs` | 检查用解码：按 `content-encoding`（gzip/deflate/br/zstd，多层逆序）解出一份**仅供检查/预览**的副本，转发字节不受影响 |
| `src/retry.rs` | 重试判定：状态码、错误 JSON（含流式 NDJSON/SSE 形态） |
| `src/config.rs` | 代理配置加载/保存/校验（`~/.aproxy/config.toml`，可多份平行并存） |
| `src/settings.rs` | 内部配置（`~/.aproxy/settings.json`，唯一）：别名表、全局默认等程序管理状态 |
| `src/daemon.rs` | 守护编排：IPC（ping/shutdown/观测）、实例注册表、恢复记录、孤儿清理 |
| `src/watchdog.rs` | 看门狗：claim 选举、进程身份（pid + 创建时间）核验、进程探活/句柄等待、共享内存心跳、重拉退避状态机 |
| `src/util.rs` | bin 侧共用小工具：时间戳、时长人性化、凭据打码、key=value 解析 |

## workspace 三成员

- **aproxy**（根包）：代理本体。编译闭包不含 switchyard 等转换库——本体
  体积不受外部转换器功能影响。
- **aproxy-envelope**：信封契约 crate（一行 JSON 的 serde 类型 + base64
  携带），aproxy 与 aproxy-format 共同依赖，单源维护防契约漂移。信封带
  `stage`（`request`/`response`，字符串而非枚举——新增阶段时旧 format 照样
  能解析）、`request_id`（实例内第 N 个请求，即 status 的请求计数）与不透明的
  `state`：请求与响应转换器是不同进程、无共享内存，跨阶段的信息由 aproxy
  按请求保存（`transform::ExchangeCtx`，随 `OutboundRequest` 走完各通道）并
  转交，回信缺省 `state` = 不变。
- **aproxy-format**：官方示例 format 二进制（协议转换 switchyard-translation
  + key 轮换/加权轮换 + 多模型多渠道聚合）。版本独立于 aproxy alpha 线，
  单独发 Release（format tag 触发独立 workflow）；它稳定后几乎不更新——
  更多功能用户直接让 agent 写 format（见 aproxy-format skill）。

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
全局默认或 toml 覆盖）关闭时回到纯内存行为。`forward_only` 模式下请求体与响应
均**不缓冲**，本节整节不适用——见「仅转发模式（forward_only）」。

## 入站来源校验（Host / Origin）

代理会把上游密钥注入每个转发请求，而本机浏览器里的任意网页都能向
`127.0.0.1` 发请求，所以 handler 的**第一步**先做来源校验（`proxy.rs`
的 `InboundPolicy`，启动时按配置归一一次，热路径只做小集合比较）：

- **Host 校验**（防 DNS 重绑定）：监听回环地址、或 `allowed_hosts` 非空时生效。
  放行 `localhost` / `127.0.0.1` / `[::1]`、监听地址自身的主机部分（`0.0.0.0` /
  `[::]` 这类通配地址除外）与 `allowed_hosts` 条目（追加而非替换）；比较时忽略
  端口、不区分大小写。监听非回环地址且列表为空时不校验——局域网/容器客户端
  的 Host 各式各样，默认拦截会破坏既有用法（另有非回环监听告警）。
- **Origin 校验**（防网页跨站调用）：任何带 `Origin` 头的请求默认拒绝，除非
  精确匹配 `allowed_origins`（不区分大小写、忽略末尾 `/`）；与监听地址无关。
  CLI 类客户端不发 `Origin`，对它们零影响。
- 两个列表都支持 `"*"` 关闭对应校验；`[]` 等同未配置。分层同
  `bounded_retry_paths`（toml 显式值 > settings.json > 内置空）。
- 被拒请求本地回 403，文案点名配置项与修法：**绝不转发、绝不注入 api_key、
  绝不进入重试循环、不计入请求数**。这是唯一不经上游就终结请求的入口，而它
  只作用于从未转发过的请求——「无限重试」对放行的请求毫无改变。

## 重试与流式回放

请求体完整读入（超 `max_body_mb` 即 413，不转发——客户端断开即中止上游请求，
避免无谓计费）后进入重试循环。响应体**逐块暂存**（spool）到内存/磁盘（上限
`spool_limit_mb` 256MiB，超限按不可重试终态处理）——只有拿到完整响应才能保证
「失败即重试」。响应完成后按原始字节序回放（零拷贝：内存模式用
`Bytes::slice_ref` 共享原分配）。

### 保活通道与提交点

缓冲期间客户端什么也收不到，所以保活适用的请求要靠心跳维持连接：默认是 SSE 注释
（`: keepalive`），`keepalive_heartbeat` 可换成任意字节（aProxy 不认识协议，心跳的形态
由用户按客户端决定；`heartbeat_problem` 只把关「写完后 SSE 解析器停在事件边界」）。
配置了 `heartbeat_transform` 时，请求转换完成后的每一拍交给心跳转换器生成（信封
`stage = "heartbeat"`）：每拍 `tokio::spawn` 一个任务跑完整次转换（drive 返回时不会
drop 在途转换，persistent worker 不会因此被剔除），上一拍未回来就发固定心跳；任务写
通道前在 `replay_gate` 锁内确认回放尚未开始，`send` 写第一块真实字节前在同一把锁内
关闸——迟到的动态心跳一律丢弃，不会插进回放的事件中间。**适用判定**（`keepalive_triggered`，在请求转换之前、按客户端视角）：
`keepalive_interval_secs` > 0、非 `forward_only`，且按 `keepalive_trigger`
命中——`accept`（Accept 含 `text/event-stream`，旧判定）/ `body_stream`（请求体
JSON 对象顶层 `"stream": true`，磁盘溢写的大请求体同样只看顶层键）/ `any`（任一，
默认）。默认必须把请求体算进来：真实 Claude Code 的流式主请求是
`Accept: application/json` + `"stream": true`（2026-10-04 实测），只看 Accept
的旧判定让它永远进不了保活通道。

适用的请求**从首轮起**交给后台任务（`proxy_with_keepalive`）驱动，响应头的
**提交点**是个只发生一次、不可撤回的三态机：未提交 → 提交上游真实头 / 提交
骨架头。推动提交的只有时间与上游响应头，「需要重试」本身不提交——重试几次后
很快成功的请求照样拿到上游真实头。尚未提交时（每一次尝试、每一段退避）：
- 上游 2xx + 未压缩的 `text/event-stream`（且未配 `response_transform`）→ 立即
  把这次尝试的上游真实 status 与响应头转给客户端（去掉 content-length 与
  hop-by-hop）；
- 一个保活间隔到点仍无可提交的结果 → 提交骨架头（200 + `text/event-stream`），
  首字节因此 ≤ 一个保活间隔；
- 上游 2xx 但非 SSE（`"stream": true` 的 NDJSON、上游无视 stream 回 JSON）→
  **只在这次尝试期间**暂停骨架提交（`drive` 的 `skeleton_paused`）：骨架是 SSE、
  心跳是 SSE 注释，提交了就把非 SSE 的流改坏。这次尝试成功 → 原样直通；需要
  重试（200 + error JSON）→ 暂停随之结束，被跳过的那拍在下一段驱动开头补提交
  （`skeleton_due`）；
- 某次尝试成功 → 走与不保活相同的保真快速路径；受限重试路径达上限 → 原样回放
  最后一次失败响应。

提交之后，无论在等上游响应头、缓冲上游流还是退避，都按间隔发心跳——「心跳
全程覆盖」靠把每次上游尝试/每段退避都放进同一个 select（与心跳节拍、客户端断开
信号一起）实现。完整缓冲、判定无误后才把成功那一次的字节写进同一个响应；
期间的重试客户端只见到心跳。

外部转换器同样在这个节拍内：请求转换在保活通道的后台任务里、首轮之前进行，
响应转换与「上游无视 identity 的压缩体」的回放前解码也经同一个 select 驱动。
format 慢（`timeout_secs = 0` 时没有上界）时客户端照样在一个间隔后收到骨架头
与心跳；请求转换在提交之后才失败时以终态 error 事件（`proxy_transform_failed`）
收场。唯一例外是「尚未提交 + 上游回 2xx 非 SSE」时的响应转换——与上面的暂停
同理，此时提交骨架会把非 SSE 的响应改坏，只能直接等转换完成。

取舍：
- **上游编码改写**：保活适用的请求发往上游时 `accept-encoding` 一律改为
  `identity`（放在请求转换之后，覆盖 format 可能写入的同名头；`override_headers`
  里配的同名头同样被覆盖，启动时 warn 一次）——往压缩流里插
  明文心跳会让客户端解压失败，所以这是对「保真透传」的有意例外；上游无视该要求
  仍压缩时（`head_is_committable` 兜底）不提交真实头，成功体回放前经
  `decode::decode_strict` 完整解码（截断/损坏即失败，绝不回放半截）。不适用保活
  的请求不改写。
- **提交后失去状态码自由**：状态行与响应头一旦提交就改不了，所以**已提交后**的
  终态失败（`spool_limit_mb` 超限、spool 写盘失败、压缩体解码失败、受限重试路径
  达上限）只能以 SSE `event: error` 事件收场，不能再回 502/真实状态码；尚未提交
  时它们与不保活路径一致（502 / 原样透传）。已提交后才拿到非 SSE 的成功体同样
  撤不回：照常回放并 warn，建议该实例设 `keepalive_trigger = "accept"`。
- **非 SSE 流的暂停窗口**：上游先回 2xx 非 SSE 头、再迟迟不发体时，那次尝试期间
  客户端收不到任何字节（最长约 `read_timeout_secs`）——非 SSE 的流本就没有可用
  的保活手段，与不保活路径的等待特性一致。

没有保活通道的请求（`"stream": false`、`forward_only`、间隔为 0）沿用首轮快速
路径 + `proxy_without_keepalive`：成功后一次性回放，期间不向客户端写任何字节。

客户端在「只收到心跳」的等待中（已提交、尚未写出任何上游真实字节）等了至少
60 秒才断开时，守护日志会 warn 一条提示：请检查客户端的流空闲超时，并带上已等
秒数。不少 agent 客户端按 SSE 事件计时（Claude Code 600s、Codex 300s、Qwen Code
240s），注释与 ping 都不算事件，重置不了它（见 `.agents/memory/claude-code-stream-watchdogs.md`
与 `codex-stream-idle-timeout` 的黑盒实测）。提示不点名客户端，行为对任何客户端
都一样；开始回放之后的断开、以及 60 秒以内的断开（多为主动取消）不提示。

重试退避：前 3 次零延迟，第 4 次起 5s→10s→20s→…封顶 `max_retry_backoff_secs`
（默认 320s），无限重试。客户端断开立即中止上游请求并停止重试（计费保护）。

例外——**受限重试路径**（`bounded_retry_paths` 配置，正则对「路径?查询串」
整体匹配，自动锚定；哪些路径受限由用户按上游配置，不内置任何 URL）：命中的
请求从总尝试第 3 次起，任何一次有响应的失败立即原样透传并终止重试（第 1、2
次照常重试；网络错误不触发，继续重试）——部分上游对特定端点确定性报错，
重试到天荒地老也不可能成功，只会让客户端永远等不到终态。
保活适用的请求同样如此：达上限时响应头尚未提交（常态——失败都很快）就原样
透传；只有响应头已被保活节拍以骨架提交的，才因无法再透传真实状态码而改以终态
SSE `event: error` 事件收场。

`forward_only` 模式整体旁路本节：不缓冲完整请求体、不进重试循环、不发心跳——
见下一节。

## 压缩响应体：检查解码、转发不解码

agent 客户端普遍发 `accept-encoding: gzip, deflate, br, zstd`，而上游字节必须
原样转发（保真契约：status/头/体逐字节一致），故 reqwest 刻意不开自动解压。
代价是**检查**跑在压缩字节上会静默失效——`retry::is_error_body` 的 JSON 解析
必然失败（HTTP 200 携带 error JSON 不再触发重试）、`is_stream_error_body` 的
SSE 行扫描全失效（流尾 error 事件检测不到）、预览只剩 hex 摘要（brotli 没有
magic number，连「这是压缩体」都认不出；2026-09-14 的 Cloudflare brotli 404
页事故即此）。`src/decode.rs` 因此解出一份**副本**喂给检查路径，两条路径互不
影响：**检查解码、转发不解码**。

已知边界：解码只接在内存判定路径（`should_retry_response`）上。磁盘路径
（响应 >1 MiB）的增量扫描器（`proxy::StreamErrorScanner`）吃的仍是原始字节，
压缩体上不判内容（仍按状态码判），只为日志预览解一份头部快照——>1 MiB 的压缩
错误体现实中不存在，为它改造成流式解码的收益与风险不成比例。

## 仅转发模式（forward_only）

`forward_only = true`（toml 显式值，或 settings.json 全局默认）时，实例走一条与
上面主流程完全不同的极简路径：**不缓冲、不重试、不保活**。请求体边收边发上游，
响应边收边回客户端。这是给「上游可信、且要真流式」场景的**显式取舍**——它放弃
了本产品最核心的重试保障，请勿当作普通开关随手打开。

- **分叉点**：在 `read_request_body` 之前判定（否则缓冲已发生，模式失去意义），
  且在 `requests_total.fetch_add` 之后（否则 status 的请求数恒为 0）。
- **请求体**：`req.into_body().into_data_stream()` 经计数适配器交给
  `reqwest::Body::wrap_stream` 流式直发——一律流式，不做空 body 特判；不缓冲、
  不落盘。计数在流式途中进行，超 `max_body_mb` 即让请求体流产出 `Err`（令
  reqwest 中止上游请求），回 413（复用既有文案）。**`max_body_mb` 是本模式下
  唯一仍然强制的限制。**
- **响应**：`resp.bytes_stream()` 直回客户端，状态码与响应头原样透传；
  hop-by-hop 照旧过滤，但 **`content-length` 必须保留**（字节未经变换，
  上游声明仍精确）。
- **上游请求失败**：502 + 原因，不重试，并调 `state.note_upstream_failure`
  记录——否则 status 的「最近错误」对这类实例永久显示「无」。
- **响应流中途中断**：**直接截断**，不注入任何上游未发出的字节；
  `tracing::warn!` 记录错误与已转发字节数，并调 `note_upstream_failure`。
- **客户端断开**：响应 Body 被 drop，reqwest 连接随之关闭——既有计费保护靠
  Drop 天然成立。
- **不进入的路径**：重试循环、SSE 保活骨架、错误内容拦截
  （`is_error_body`/`is_stream_error_body`）、spool、`keepalive_trigger` 判定。
- **本模式下不生效**：`disk_cache`/`spool_limit_mb`/`keepalive_interval_secs`/`keepalive_trigger`/`keepalive_heartbeat`/
  `max_retry_backoff_secs`（磁盘 spool 完全不参与）。
- 消费方一律走 `Config::forward_only_enabled()`（`unwrap_or(DEFAULT_FORWARD_ONLY)`），
  **不得 `unwrap()`**——doctor 的 `parse_config_file`、`find::discover` 与大量
  测试的 `AppState::new` 都不经 settings 注入。`proxy::router(AppState)` 签名不变。

## 配置分层

三级优先级（仅 `max_body_mb`/`disk_cache`/`forward_only`/`bounded_retry_paths`/
`allowed_hosts`/`allowed_origins`/`keepalive_trigger` 七个字段有 settings 层；其余
字段 toml > 内置默认）：

```
CLI 覆盖参数（--baseurl 等，仅本次） > config.toml 显式值 > settings.json 全局默认 > 内置默认
```

`config.toml` 人类可读可写、可多份（多开各自指定）；`settings.json` 程序管理的
内部配置，**全局唯一**（JSON 原子写，损坏回退默认），存别名表、default_config、
config_dirs、日志轮转阈值、空闲阈值与上述七个字段的全局默认。

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
- 子进程（`--daemon-child`）承载服务：清 spool 目录（bind 前，回收崩溃残留）→
  bind → 写实例注册表（`run/<端口>.pid`，含实例上报的 log_path）→ 写恢复记录
  （`run/<端口>.restore`，同样携带 log_path——崩溃实例的日志凭此保留）→
  启动 IPC 管道 → 创建看门狗心跳节 + 心跳 ticker
  （10s）→ 守护侧互保任务（5 分钟自检）→ serve。
- 停止：IPC `shutdown` → 优雅关闭（10s 宽限强退）→ 清注册表与恢复记录。

### 端口冲突的两种情况

1. IPC ping 通 → 同端口已有 aProxy 在运行（不重复启动，exit 0）
2. 管道不通但 bind 失败 → 被其他程序占用 / 无权限或被系统保留
   （Hyper-V/WinNAT 排除区间，按错误类别报因，exit 1）

### 实例身份

端口号是实例唯一键：同端口不同监听地址的第二实例会在启动时被显式拒绝
（注册表与 IPC 管道按端口命名，无法并存）。

## 看门狗（watchdog）

全局单看护进程（没有公开子命令，同二进制以 `--daemon-watchdog` 隐藏标记分离
启动），把「进程级死亡/挂死 → 永久断流直到人工发现」降级为「秒级检出 → 退避
重拉」。实测 +2.4% 体积、+2.9MB 常驻、热路径零损耗（[benchmark-watchdog.md](benchmark-watchdog.md)）。

- **进程身份**：一律按「pid + 进程创建时间」核验，与二进制文件名无关（改名
  部署的实例同样受看护）。实例在注册表登记 `process_start`（Windows FILETIME /
  Linux `/proc` starttime）；收养、处决、接管假死前任、install 换血停旧看护者、
  `stop --force` 都走这一判据。登记值为 0（平台读不到创建时间）的记录证明不了
  身份，一律不收养、不处决、不强杀，保守优先（杀错进程不可逆）。
- **死亡检测**：收养时 `OpenProcess(SYNCHRONIZE|TERMINATE)` 取进程句柄，
  `spawn_blocking` 内核阻塞等待——死亡信号即时、零轮询线程。死亡事件到达即
  唤醒主循环处理，崩溃实例在其启动耗时内回来，不等扫描周期。主动强停的一方
  （`stop --force` / `restart --force`）因此必须在终止**之前**摘掉 `.restore`
  （终止失败再放回），否则看护者会先于它的收尾读到「记录在 = 崩溃」，把刚
  强停的实例拉回来。
- **挂死检测**：实例侧独立 ticker 每 10s 向命名共享内存节
  （`Local\aproxy-heart-<端口>`，8 字节原子 u64 毫秒时间戳）写心跳——挂死 =
  runtime 无法调度 = ticker 停摆，与请求热路径零耦合。看护侧扫描过期 +
  IPC ping 二意见都失败才终止进程（句柄绑定原进程，PID 复用免疫）。
- **重拉与退避**：`.restore` 残留 = 异常死亡信号；优雅退出（记录已删）摘除
  看护。首次崩溃立即重拉；重拉失败进退避队列，按指数退避 1/2/4/8…封顶 300s
  由主循环时间驱动重试（等待不阻塞 tick——否则长退避会让 claim 心跳停摆、
  现任被竞争者按「假死」夺权），就绪判定按新 pid 定位；重拉后实例落在不同
  端口（toml 改了端口）时退役旧端口的 `.restore`/`.pid` 并按实际端口续看护；
  连续失败达 `watchdog_max_restarts` 放弃并写 startup.log（`.restore` 保留
  人工兜底）。
- **选举规范**（多守护并发拉起看护者的唯一性保障）：claim 文件
  `run/watchdog.claim`（PID + 进程创建时间 + 心跳）为在任真相源——
  排序定发起者（核验在世的实例中 PID 最小者才有权 spawn；CLI `start` 不在在世
  集合内，视为有权，多发起由 claim 原子接管兜底）+ `create_new` 原子接管
  定在任者；进程创建时间比对拒绝 PID 复用冒名；假死前任（心跳过期）经验证
  后终止接管。
- **互保**：守护与看护者完全解耦（看护者死亡实例不受影响）；守护每 5 分钟
  自检，缺席即按选举规范补种。全部实例清零后看护者闲置自灭（默认 300s），
  系统回到零常驻。
- 配置：settings.json `watchdog` 五字段（总开关/扫描周期/挂死容忍/重拉上限/
  闲置自灭）——系统级单例故无 toml 层。

## 自愈恢复（restore）

`run/<端口>.restore` 存实例的启动参数：

- **写入**：守护 bind 成功时（无论前台/后台）
- **删除**：优雅退出（stop、Ctrl+C）
- **保留**：崩溃、断电、系统重启

`aproxy restore` 按记录重新拉起（`--daemon-child`），IPC ping 就绪后才报成功；
已在运行跳过（幂等）；空清单静默 exit 0（开机自启友好）。

`.restore` 绝不在 status 清理时删除——崩溃实例恰恰靠它存活到 restore 执行。
`restore` 按新 pid 定位实例并报告实际端口；实例落在不同于记录的端口时清理旧
端口记录。`stop --force` 来不及做优雅退出的自清，由 CLI 在强杀成功后按守护自清
的同一顺序删除 `.restore`/`.pid`/socket/心跳，否则残留的 `.restore` 会被当作
崩溃信号复活实例；`restart --force` 的强杀路径则保留它作为自愈兜底。

### restart：先预检、后停止

`restart` 的主要用途是「改完 config.toml 让它生效」，而配置笔误恰恰最常出现
在这一步。先停旧实例再由新守护读配置，配置有错时新守护秒死，旧实例的
`.restore` 又已随优雅退出删除——健康的服务就此下线。因此 stop 之前先按新守护将
走的同一条解析路径（`start::resolve_runtime_config`：严格 TOML 加载、CLI 覆盖、
settings 注入、validate）干跑一遍，换监听地址时再探测新地址可否绑定；预检不通过
就不碰旧实例。`restart all` 逐实例预检，失败的跳过、其余照常重启，最后汇总并
以非零退出；新实例启动即退出时展示 startup.log 本次新增内容与恢复命令。

## 日志治理

- 守护日志 `logs/`：按启动时刻**随机命名**（十六进制时间戳-pid 格式，如
  `19ac3f2e8b5d-1a2b.log`），每次启动（含 restart、restore）都是新文件——
  换端口后日志不断档；启动时 >2MiB 截断；运行期每小时检查，>8MiB 截断
- **路径解析序列**（`main.rs`/`server.rs`）：宽松 load 配置（解析失败不阻断
  日志初始化）→ resolve（CLI `--log-file` > config.toml `log_file` > 内置
  随机名；`~` 展开，相对路径相对 APROXY_HOME——守护 cwd 不可靠）→ 日志
  init → 实例经 IPC 上报 log_path（`InstanceRecord.log_path`），注册表与
  `.restore` 记录均携带
- **地址以 IPC 为准**：`aproxy logs [PORT|别名]`（tail -f 语义：末尾 8KB/
  30 行 + 增量轮询，实例停止自动退出）与 start 成功提示都向实例询问真实
  路径，客户端不拼路径；`--foreground` 实例 log_path 为空，logs 立即报
  「日志输出在它的控制台」
- 孤儿清理：`status` 时删除「活实例上报的 log_path ∪ `.restore` 记录的
  log_path」之外的 `*.log`（startup.log 除外，非 .log 文件不动）——不再按
  文件名端口归属判定。推论：优雅停止实例的日志在下次清理时删除；崩溃实例的
  日志由 `.restore` 引用保留到恢复成功；旧版按端口命名的日志升级后视为孤儿
- 全部输出对凭据打码——可安全粘贴分享。URL 展示的**唯一出口**是
  `config::mask_base_url`（名称沿用最早只用于 base_url 的叫法，现在 base_url、
  代理 URL、上游目标 URL、请求的 `路径?查询串`、reqwest 错误里的 URL 一律走它）：
  userinfo（用户名或密码任一存在）整体 `***@`，查询串保留键名、值变 `***`，片段
  整体遮掉，畸形 URL 保守多遮；api_key 与头值走 `util::mask_secret`（前 6 字符 +
  `***`）。`config --show` 另对转换器 `args` 的密钥旗标之后的值与 `extra` 打码

## 安装与升级（install）

`aproxy install` 把二进制安全落位到 `~/.aproxy/bin/`，对客户端 ≈ 无感。
四条铁律：先标记（install.state 先于一切动作，崩溃可识别）、先下载后
rename（staging 备料校验全过才动 bin）、ACK 齐了才交换（IPC PrepareSwap
广播，实例把自己的可观测状态置为 `swap_prepared`）、逐个重启最后删除（.old 在
终验后清理）。模块 `src/install/`：

- **更新通道**（`download::Channel`，`latest` 解析的单一真相源，github/npm 两个
  查询入口与 CLI 的「是否需要安装」决策都以它为准）：Stable 只在正式版（无预发布
  后缀且 GitHub 未标 prerelease）中取 semver 最大，Pre 在全部版本中取最大。
  默认通道跟随当前运行版本（当前是预发布 → Pre，当前是正式版 → Stable），
  `--pre` 显式切到 Pre。通道内选版（`github::pick_latest`）：非 draft、tag 是
  `v` + 项目版本号文法（`X.Y.Z` 或 `X.Y.Z-(alpha|beta|rc).N`——`format-v*` 与
  `alpha.12t3` 这类历史测试 tag 都被排除，后者在 semver 里反而大于 alpha.17）、
  已含本平台 baseline 资产（先建 release 后传资产）。通道为空 = 保持现状，绝不
  跨通道偷装预发布；npm 兜底按 dist-tags（Stable 读 `latest` 且其为预发布时视为
  无稳定版，Pre 取 `latest`/`next` 较大者）。选这个设计的原因：二进制内置的选版
  逻辑随版本发出就收不回，「默认只取稳定版」不能指望发布侧标签永远打对
- **state**：状态文件即安装锁（run/ 下，原子重写，updated_at 自动刷新供
  stale 判定）；阶段状态机线性主线 + failed/aborted 旁路，abort 仅
  swapping 前可回滚。交棒后等待结局的往往是较旧的二进制，所以阶段词表与
  字段名冻结、新字段一律可选
- **staging**：备料区（复制/chmod/sha256/`--version` 试跑自证）
- **swap**：平台收口点——Windows copy+双 rename 舞（bin 永不空窗 + .old
  固定名 + PATHEXT fallback 入口脚本常驻）；unix 单步 rename 原子覆盖
  （无空窗无 .old）
- **announce**：安装态宣告节（Windows 命名节 / unix /dev/shm，进程退出即
  解除）——看护者/守护的差异化行为全部由它门控：实例死亡复查 5×3s、
  install 保活续作、守护自检补种抑制
- **restart 与服务回滚**：实例重启原语（`install/restart.rs`）在停止之前把恢复
  参数记进 install.state 的在途清单，停止之后立即回写 `.restore`；新二进制起不来
  （bind 前退出、8 秒内没就绪）则用旧二进制按原参数把该实例拉回并置 `halted`，
  由 flow 中止滚动、剩余实例不动。回滚的只是这一个实例的服务——与 forward-fix
  铁律（swapping 之后只进不退）兼容：bin 里的新二进制与阶段都不回退（阶段落
  failed 保留现场），自动续作不再重试。旧二进制的来源：Windows `aproxy.old.exe`
  （双 rename 的产物）；unix 在单步 rename 覆盖前把旧二进制硬链接（失败则复制）
  为 `bin/aproxy.old`——覆盖后旧 inode 只挂在运行中进程上，文件系统里已无路径可
  执行，所以必须先留副本；cleaning 阶段删除
- **flow**：编排（管辖检查 → 备料 → 早交接 → 广播 ACK → 交换 → 滚动重启 →
  终验 → 清理）；任何中断点由 `--continue` 幂等续作（恢复矩阵：看护者主责 +
  CLI 入口兜底，全自动无询问）。早交接：备料校验通过后，安装者以
  `install --continue --handover-from <自己的 pid>` 拉起 staging 里的目标二进制，
  之后的一切由它执行，安装者只等待并转告结局；Windows 上它交换后再把剩余阶段
  交给 bin 里的同一个二进制（cleaning 要删 staging，运行中的镜像删不掉）。选这个
  设计的原因：每次升级都由新版本驱动，新版本能修旧安装器的缺陷，兼容只需要
  「新读旧」。降级由较新的安装者自己执行（旧版本不认交棒参数）。没被指名的续作
  （CLI 入口、看护者拉起的）在安装者健在时一律不接手
- **download**：有序下载链条 github → npm → cargo-binstall → cargo
  （settings download_chain 严格数组可配 + url 模板通道，CDN 域名不硬
  编码）；github=.sha256 强校验、npm=integrity 强校验（跟随 ~/.npmrc
  镜像）、cargo=本地编译自证；下载代理（download_proxy/--download-proxy）
  与上游请求代理绝对分离
- **skills**：skill 文档支线（并行下载、失败不影响安装、原子目录替换）

跨平台分工：状态机/IPC/恢复全部平台无关，平台分支只收口在交换原语、
宣告节介质、可执行位三处。

## 测试

- 单元测试（lib + bin）：重试判定、配置分层、IPC 协议、注册表/恢复记录、打码、claim 选举、退避状态机、安装状态机/下载链条/zip 安全
- 集成测试：mock 上游 + 真实代理联调、守护生命周期、logs/restore 端到端、
  双模缓冲（磁盘 spool 字节保真/EOF 清理/流尾错误重试）、看门狗（强杀重拉/
  优雅停不复活/并发看护者唯一性）、install 流程（恢复矩阵崩溃注入 + CLI
  级完整链路）、交换原语（双 rename 舞 + fallback 脚本）、备料全链
- **CI**（`.github/workflows/ci.yml`）：windows job 跑 fmt + clippy + test，ubuntu
  job 跑 clippy + test（windows 的 clippy 不编译 `#[cfg(unix)]` 代码，零警告纪律在
  ubuntu 补上 unix 面）；clippy 与 test 一律带 `--workspace`（根包是 aproxy 且
  `[workspace]` 未设 default-members，不带时 aproxy-envelope 与 aproxy-format 的测试
  从不进入门禁），test 带 `--no-fail-fast` 一次拿到完整失败面。macOS 是独立 job，
  **已知红**（`/dev/shm`、`/proc` 依赖，见「平台」）：不设 skip 也不设
  `continue-on-error`，红色就是「未完成」的如实信号，独立成 job 后不淹没其他平台
  的结论
- **发布门禁**（`release.yml`）：tag 触发后先在 windows 与 ubuntu 上复跑
  fmt/clippy/test（tag 可能落后于 main），全绿才进入多平台构建；macOS 不进门禁。
  渠道发布幂等：npm 与 crates.io 两个 job 都先查询注册表、版本已存在才跳过，所以
  任一渠道失败后可单独 re-run，已成功的不会重复发布；GitHub Release 的 Latest 标记
  交给 API 默认值（预发布永不成为 Latest）
- 测试端口 bind 试探选取（排除区间/占用者自动跳过），绝不触碰用户实例；
  `DaemonGuard` 保证断言失败路径也清理守护

## 平台

| 平台 | 状态 |
|---|---|
| Windows（x64 / x86 / arm64） | 全功能（开发与主测试平台） |
| Linux（x86_64 / aarch64，glibc 与 musl） | 全功能；gnu 产物 glibc 下限 2.28，更低的 glibc 与 musl 系统用 musl 产物 |
| macOS（Apple Silicon / Intel） | 提供预构建二进制，但未经真机验证；看门狗、`aproxy install` 与依赖进程查询的实例管理用到 Linux 专有接口（`/dev/shm`、`/proc`），在 macOS 上不可用或退化 |

unix 分支（UDS IPC、`/dev/shm` 宣告与心跳、`/proc` 进程查询、单步 rename
交换）按 **Linux** 实现：`#[cfg(unix)]` 路径依赖 `/dev/shm` 与 `/proc`，这两族
原语在 macOS 上不存在，所以 macOS 上共享内存心跳、进程创建时间读取与 install
的宣告节无法工作。这部分需要在 macOS 真机上补齐实现与验证（后续贡献者），
补齐之前 README 的平台矩阵如实标注为暂不支持。macOS 的代理转发路径不依赖它们，
可用。

### Linux gnu 产物的 glibc 下限

在 ubuntu-24.04 runner 上直接构建的 gnu 产物会链接 glibc 2.39，Debian 12、
Ubuntu 22.04 等系统启动即报 `GLIBC_2.39 not found`（alpha.17 实测如此）。发布流程
因此用 cargo-zigbuild 按 `GNU_GLIBC_FLOOR`（2.28，`release.yml` 顶部 env）链接，
并在构建后用 `objdump -p` 断言产物引用的 `GLIBC_` 符号版本都不高于它（非数字的
需求如 `GLIBC_ABI_DT_RELR` 同样判失败）。这个数字必须与 `scripts/install.sh` 和
`npm/aproxy/bin/aproxy.js` 里的 `MIN_GLIBC` 保持一致：它们在 glibc 低于下限的
系统上改选静态链接的 musl 产物（脚本落位前还用 `--version` 自证，gnu 跑不起来
同样改拉 musl）。musl 产物静态链接，任何 Linux 上都能跑。

### npm dist-tag

预发布（版本含 `-alpha`/`-beta`/`-rc`）发在 `next`，正式版发 `latest`
（`npm/build-and-publish.sh` 的 `npm_dist_tag()` 与 `release.yml` 里 GitHub Release
的 `prerelease` 判定是同一条规则，改一处必须同步另一处）。首个正式版发布后
`npm i -g @meowo/aproxy` 只会装到正式版，预发布需显式 `@meowo/aproxy@next`。

## 版本路线

- **0.1.x**：当前开发线。0.1.0 是首个稳定版（此前为 `0.1.0-alpha.*` 预发布），
  之后 0.1 系列内的小版本直接发布；无 UI，纯 CLI + 守护。
- **0.2.0**：引入 UI（Web 面板形态待定）——前提是基础功能齐备、稳定性经 beta
  期验证；属远期。
- 发布产物：x86_64 的 Windows 与 Linux gnu 同时提供 baseline 与 x86-64-v3
  （AVX2）两种指令集变体，`aproxy install` 运行时检测后自动选择（失败回退
  baseline），一键安装脚本保守地装 baseline；musl 与其他架构只有 baseline。
