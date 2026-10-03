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

上图的「重试循环 / spool / 回放」是默认模式的主流程；`forward_only` 模式下这两
环整体旁路（见「仅转发模式（forward_only）」）。

## 模块

| 模块 | 职责 |
|---|---|
| `src/main.rs` | 进程入口：控制台代码页、配置文件定位、日志初始化、子命令分派（仅 ~110 行） |
| `src/cli.rs` | clap 命令树定义（`Cli`/`Commands`/`AliasCmd`/`ConfigArgs`），只承载定义不含逻辑 |
| `src/commands/` | 十一个子命令各自一文件（start/status/stop/restart/restore/alias/doctor/find/logs/config/install），共享 target 解析在 `mod.rs` |
| `src/server.rs` | 服务承载：`serve_forever` 主循环、停止信号、日志初始化、配置错误落盘 startup.log |
| `src/proxy.rs` | 转发核心：入站来源校验（Host/Origin）、hop-by-hop 过滤、内存+磁盘双模 spool、错误判定、keepalive、断开保护、仅转发模式 |
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
  携带），aproxy 与 aproxy-format 共同依赖，单源维护防契约漂移。
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
「失败即重试」；期间每 `keepalive_interval_secs` 向客户端写一行 SSE 注释
（`: keepalive`）防超时。响应完成后按原始字节序回放（零拷贝：内存模式用
`Bytes::slice_ref` 共享原分配）。

重试退避：前 3 次零延迟，第 4 次起 5s→10s→20s→…封顶 `max_retry_backoff_secs`
（默认 320s），无限重试。客户端断开立即中止上游请求并停止重试（计费保护）。

例外——**受限重试路径**（`bounded_retry_paths` 配置，正则对「路径?查询串」
整体匹配，自动锚定；哪些路径受限由用户按上游配置，不内置任何 URL）：命中的
请求从总尝试第 3 次起，任何一次有响应的失败立即原样透传并终止重试（第 1、2
次照常重试；网络错误不触发，继续重试）——部分上游对特定端点确定性报错，
重试到天荒地老也不可能成功，只会让客户端永远等不到终态。

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
  （`is_error_body`/`is_stream_error_body`）、spool、`client_wants_sse` 判定。
- **本模式下不生效**：`disk_cache`/`spool_limit_mb`/`keepalive_interval_secs`/
  `max_retry_backoff_secs`（磁盘 spool 完全不参与）。
- 消费方一律走 `Config::forward_only_enabled()`（`unwrap_or(DEFAULT_FORWARD_ONLY)`），
  **不得 `unwrap()`**——doctor 的 `parse_config_file`、`find::discover` 与大量
  测试的 `AppState::new` 都不经 settings 注入。`proxy::router(AppState)` 签名不变。

## 配置分层

三级优先级（仅 `max_body_mb`/`disk_cache`/`forward_only`/`bounded_retry_paths`/
`allowed_hosts`/`allowed_origins` 六个字段有 settings 层；其余字段 toml > 内置默认）：

```
CLI 覆盖参数（--baseurl 等，仅本次） > config.toml 显式值 > settings.json 全局默认 > 内置默认
```

`config.toml` 人类可读可写、可多份（多开各自指定）；`settings.json` 程序管理的
内部配置，**全局唯一**（JSON 原子写，损坏回退默认），存别名表、default_config、
config_dirs、日志轮转阈值、空闲阈值与上述六个字段的全局默认。

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
  Linux `/proc` starttime；0 = 未登记）；收养、处决、接管假死前任、install 换血
  停旧看护者、`stop --force` 都走这一判据。旧版本记录（无创建时间）退回
  「IPC ping 回报的 pid 一致」作归属证明——挂死的旧实例不收养，`--force` 终止前
  再 ping 一次，保守优先（杀错进程不可逆）。
- **死亡检测**：收养时 `OpenProcess(SYNCHRONIZE|TERMINATE)` 取进程句柄，
  `spawn_blocking` 内核阻塞等待——死亡信号即时、零轮询线程。
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
  init → 实例经 IPC 上报 log_path（`InstanceInfo.log_path`），注册表与
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
广播，实例置位自己的可观测状态 swap_phase）、逐个重启最后删除（.old 在
终验后清理）。模块 `src/install/`：

- **state**：状态文件即安装锁（run/ 下，原子重写，updated_at 自动刷新供
  接力存活判据）；阶段状态机线性主线 + failed/aborted 旁路，abort 仅
  swapping 前可回滚
- **staging**：备料区（复制/chmod/sha256/`--version` 试跑自证）
- **swap**：平台收口点——Windows copy+双 rename 舞（bin 永不空窗 + .old
  固定名 + PATHEXT fallback 入口脚本常驻）；unix 单步 rename 原子覆盖
  （无空窗无 .old）
- **announce**：安装态宣告节（Windows 命名节 / unix /dev/shm，进程退出即
  解除）——看护者/守护的差异化行为全部由它门控：实例死亡复查 5×3s、
  install 保活续作、守护自检补种抑制
- **flow**：编排（管辖检查 → 备料 → 广播 ACK → 交换 → Windows 接力交棒 →
  滚动重启 → 终验 → 清理）；任何中断点由 `--continue` 幂等续作
  （恢复矩阵：看护者主责 + CLI 入口兜底，全自动无询问）
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
- 测试端口 bind 试探选取（排除区间/占用者自动跳过），绝不触碰用户实例；
  `DaemonGuard` 保证断言失败路径也清理守护

## 平台

| 平台 | 状态 |
|---|---|
| Windows（x64 / x86 / arm64） | 全功能（开发与主测试平台） |
| Linux（x86_64 / aarch64，glibc 与 musl） | 全功能 |
| macOS（Apple Silicon / Intel） | 代理转发可用；看门狗与 `aproxy install` 暂不支持 |

unix 分支（UDS IPC、`/dev/shm` 宣告与心跳、`/proc` 进程查询、单步 rename
交换）按 **Linux** 实现：`#[cfg(unix)]` 路径依赖 `/dev/shm` 与 `/proc`，这两族
原语在 macOS 上不存在，所以 macOS 上共享内存心跳、进程创建时间读取与 install
的宣告节无法工作。这部分需要在 macOS 真机上补齐实现与验证（后续贡献者），
补齐之前 README 的平台矩阵如实标注为暂不支持。macOS 的代理转发路径不依赖它们，
可用。

## 版本路线

- **0.1.x**：当前开发线。0.1.0 是首个稳定版（此前为 `0.1.0-alpha.*` 预发布），
  之后 0.1 系列内的小版本直接发布；无 UI，纯 CLI + 守护。
- **0.2.0**：引入 UI（Web 面板形态待定）——前提是基础功能齐备、稳定性经 beta
  期验证；属远期。
- 发布产物：x86_64 的 Windows 与 Linux gnu 同时提供 baseline 与 x86-64-v3
  （AVX2）两种指令集变体，`aproxy install` 运行时检测后自动选择（失败回退
  baseline），一键安装脚本保守地装 baseline；musl 与其他架构只有 baseline。
