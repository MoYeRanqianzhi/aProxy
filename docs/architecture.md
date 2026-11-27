# 架构

aProxy 是单二进制的本地 HTTP 代理。本文面向贡献者，解释各组件职责与关键设计决策。

## 总览

```
agent 软件 ──HTTP──▶ [代理端口 12345] ──重试循环──▶ 上游 API
                        ▲                 │
                        │ 透传             │ spool（内存 ≤1MiB 驻留，
                    命名管道 IPC           ▼        超出溢写磁盘）
                        │          磁盘 spool ~/.aproxy/spool/<pid>-…/
                  [控制通道 <端口>]        │
                        ▲                回放
        aproxy status/stop/logs/restore
```

**铁律：代理端口完全用于透传。** 所有控制通信（ping/shutdown）走独立命名管道
`\\.\pipe\aproxy-<home_id>-<端口>`（unix 为 `<run 目录>/<端口>.sock`），杜绝控制路径与
客户端请求路径重叠。`home_id` 是 run 目录规范化路径的 SHA-256 前 16 位十六进制：
Windows 的管道名全机共享，带上它，一个 home（`APROXY_HOME`）的命令就只能控制本
home 的实例。

启动顺序：bind TCP → 独占创建控制端点（失败即退出，写 startup.log）→ 写实例记录
`<端口>.instance.json` → 写期望状态记录 `<端口>.restore.json`（前台实例不写）→ 开始应答控制
端点 → 心跳 → 服务。由此「有实例记录 ⇒ 端点已建好」（挂死除外），拉起方看到端点应答时
期望状态记录也已经落盘。客户端核对应答者自报的 pid 与连接对端进程
（`GetNamedPipeServerProcessId` / `SO_PEERCRED`）：端点名可预测，抢先占住它的进程只能
报出自己的 pid，普查再拿这个 pid 与注册记录比对。

控制端点只让启动实例的用户用（同一用户的进程互相信任——它们本来就读得到配置里的明文
密钥——所以不设令牌；防的是别的用户与远程）：
- Windows：每个管道实例都带受保护的显式 DACL `D:P(A;;GA;;;<当前用户 SID>)(A;;GA;;;SY)`，
  并拒绝远程客户端。建不出安全描述符时实例不启动，不退回谁都能连的默认权限。
- unix：socket 文件是 0600（被删后重建的也是）。服务端经 `SO_PEERCRED` 只接受同一 uid 或
  root 的连接，别的连接不读请求、不回应答，直接断开；客户端核对在端点上听的进程的 uid
  等于自己，客户端是 root 时改为核对它等于 run 目录的属主（或 root）——`sudo aproxy stop`
  因此可用。

home 本身同样只让属主访问——里面有明文密钥（config.toml、settings.json、期望状态记录）、
日志与落盘的请求内容，逐个文件设权限迟早会漏掉一处：
- 新建：unix 上 0700；Windows 上受保护的显式 DACL（当前用户、SYSTEM、Administrators 各自
  完全控制，子项继承）。三个安装脚本新建 home 时设同样的权限。
- 既有的 home 属于当前用户而别人也能访问（别的版本建的、放在公共目录里的）：aproxy 下一次
  往里写东西时收紧它。只读的命令（`status`、`doctor`、`settings get`）不改权限；不属于当前
  用户的 home（`sudo aproxy …`）不动。
- `aproxy doctor` 对别的用户能访问的 home 给一条告警，说明它会不会被自动收紧。Windows 上
  「别的用户」不包括 SYSTEM、Administrators 与同一用户的沙箱化应用（应用容器 SID）。
- 台账（C3）由冻结的写者自己建目录、写文件；进程登记之前先过上面的检查，所以 home 不会
  经它按 umask 被建出来。

线上格式（v1，一连接一来回，各一行 JSON）：请求 `{"v":1,"op":"ping|shutdown|prepare_swap"}`，
可带 `args` 对象：
- `shutdown` 的 `keep_restore`（为真时实例退出但不删期望状态记录）与 `if_idle_secs`（实例已
  闲置至少这么多秒才停，否则回 `condition_not_met`、什么都不做——判断与停止在实例里一次完成，
  `stop idle` 靠它避免打断刚到的请求）；
- `prepare_swap` 的 `ttl_secs`（更换阶段保持多久，默认 600 秒；到期自己回到 `serving`，安装者
  中途没了实例也不会永远停在这个状态）。

成功应答 `{"v":1,"ok":true,"result":InstanceStatus}`，所有 op 都回同一份状态快照：
- `instance`：实例记录去掉 `schema`（`pid`、启动标记 `start`、`instance_id`、`version`、
  `listen_addr`、`config_path`（没有配置文件为 null）、打过码的 `base_url`、`started_at`、
  `log_path`（前台实例为 null）、`foreground`）；
- `state`：`starting`/`serving`/`swap_prepared`/`stopping`，不认识的值读作 `unknown`；
- `behavior`：行为摘要（默认投递方式 `delivery`、`disk_cache`、`keepalive` 的触发方式与间隔、
  重试规则条数、请求历史的体预算 `history.body_budget_mb`），枚举值按字符串传；
- `activity`：最近请求时刻、请求数 `requests_total`、重试数 `retries_total`、救回数
  `rescued_total`（经过重试才成功的请求）、在途数 `in_flight`（算到响应体交付完为止）、其中
  重试中的 `retrying`、按码的失败计数 `by_code`（失败的上游尝试记尝试失败码，aproxy 自己
  结束的请求记终态码）、`buckets`（最近 30 个 120 秒的桶：请求、重试、失败的尝试，旧的在前）、
  最近一次失败 `last_error`（`code`、`details`、`message`、`at`）；时刻都是 Unix 毫秒；
- `ops` 与 `features`：实例支持的 op，与它认得的 op 参数（名字是 `<op>.<参数>`，如
  `shutdown.keep_restore`）。带一个参数之前先查 `features`——不认识的参数会让整个请求被拒。

失败应答 `{"v":1,"ok":false,"error":{"code","message","details"}}`：`code` 是码表里的 IPC
协议码（`bad_request`、`unsupported_version`、`unknown_op`、`unsupported_arg`、`invalid_state`、
`condition_not_met`），`details` 是这个码带的数据（键与类型见 [error-codes.md](error-codes.md)），
`message` 是英文、只给日志看。未知 op 与未知参数都没有副作用。`v` 只在语义不兼容时递增，
加 op、加可选参数、加应答字段、加错误码、加状态值都不递增；应答只增字段，读者忽略不认识的。
没有 `v` 的请求按 `bad_request` 拒绝，没有 `v` 的应答不予采信（0.1.0 的格式就是这样，0.2.0 起
与它不再互通）。别的版本的应答里客户端只认一种：`unsupported_version`（实例说它不支持请求的
版本，`details.supported` 列出它支持哪些），其余一律不采信。读请求历史的四个 op（`ops` 里列着才有）不回状态快照：
- `requests`（`args`：`before` 请求标识、`limit` 至多 200、默认 50）→
  `{"requests":[记录…],"more":bool}`，新的在前；
- `request`（`request_id`）→ `{"record":…,"request":{"headers","body"},"tries":[…]}`，头是
  `[名字, 值]` 对的数组，体是 `{"bytes","state","cause"?,"encoding"?}`；
- `body`（`request_id`、`part` 为 `request`/`sent`/`response`、后两种带 `attempt`、可选
  `offset` 与 `limit`，单块至多 1 MiB）→ `{"total_bytes","offset","data_b64","more","complete"}`，
  还在接收的体 `complete` 为 false；
- `subscribe`：这条连接不再一来一回。第一行是应答 `{"snapshot":状态快照,"requests":[在途的
  记录]}`，之后每行一个事件 `{"v":1,"event","seq","at","data"}`：`request`（整条记录，按
  `request_id` 替换）、`state`（状态快照，变了才发，至多每秒一个）、`heartbeat`（静了 15 秒）、
  `lagged`（`dropped`）、`closing`（`reason` 为 `shutdown` 或 `swap`，发完实例关连接）。`seq`
  每条连接从 1 递增，次序以它为准；`at` 是事件发生的时刻（`request` 事件是记录成为这个样子
  的时刻，不是写上连接的时刻，所以它可以比前一行略早）。同一实例至多 8 条订阅连接，满了回 `busy`（`details.limit`）；10 秒写不
  出去的连接被关掉。

查不到时回 `not_found`，`details.reason` 说明是哪一种：`unknown_request`、`no_such_part`、
`evicted`、`not_retained`（带 `cause`）。

每个 op 的请求行、每种拒绝与快照的
形状都由 `src/daemon.rs` 的单测逐字钉住；`tests/contract_v0_2.rs` 再对一个真实例走原始的控制
通道、读它写在盘上的记录，逐项比对样本（样本里的键不能少、类型不能变，多出来的不算错）。

只读清点（`ops::enumerate::enumerate_in(home)`）给 `status --json` 与界面的实例列表供数：读
`run/` 与 `ledger/`，同时向每个端口的控制通道问一次（单次 1 秒，没答上再试一次），不删任何
文件。结果能直接序列化：
- `instances`：每个端口一条，`liveness` 是 `live`（应答可采信，带 `instance` 与实时的
  `status`——快照去掉 `instance`）、`unresponsive`（记录里的进程经「pid + 启动标记」核验还在，
  控制通道不应答；只有这一种该建议 `stop --force`）、`incompatible`（应答了但无法采信，
  `reason` 是 `unsupported_version` 带 `supported`，或 `instance_incompatible` 带 `port` 与
  `reason`）、`down`（没有实例在运行、留着期望状态记录：`restore_pending` 为真，配置文件不在时
  `restore_reason` 是 `config_missing`）；读者遇到不认识的取值读作 `unknown`。只剩一份死进程的
  实例记录的端口不列出；
- `unreadable`：`run/` 里读不懂的记录，`file` 是相对 home 的路径，`reason.code` 是
  `record_unreadable` 或 `record_schema_too_new`（带 `schema`）；
- `gc_paused`：有读不懂的实例记录或期望状态记录时出现（`reason` 为 `unreadable_records`），
  日志清理在它们消失之前暂停；
- `watchdog`：属主进程还在的看护者记录，`state` 是 `running` 或 `stale`（最近一次扫描早于三个
  扫描周期）；没有时为 null；
- `processes`：台账里仍在运行、却不属于上面任何实例或看护者的进程——界面、安装者，以及本
  版本读不懂其记录的实例（别的版本启动的）。

上图的「重试循环 / spool / 回放」是默认模式的主流程；`delivery = "forward"` 模式下这两
环整体旁路（见「仅转发模式（delivery = "forward"）」）。

## 模块

| 模块 | 职责 |
|---|---|
| `src/main.rs` | 进程入口：C5 的 `--version`、控制台代码页、在建 tokio 运行时之前分派 `desktop`，并把内部入口 `__serve`/`__watchdog` 的 `--home` 写进本进程的 `APROXY_HOME`；其余子命令在手建的运行时里分派（`__serve` 早于 settings 检查与续作拉起，自己初始化日志） |
| `src/cli.rs` | clap 命令树定义（`Cli`/`Commands`/`AliasCmd`/`ConfigArgs`），只承载定义不含逻辑；含两个隐藏内部入口 `__serve --home --spec` 与 `__watchdog --home`（跨版本互相拉起，名字与参数冻结） |
| `src/commands/` | 十三个子命令各自一文件（start/status/stop/restart/restore/alias/doctor/find/logs/config/settings/install/desktop）：调用操作层拿到结果，渲染成给人看的文字、决定退出码；`start.rs` 还承载 `__serve` 入口 |
| `src/ops/` | 操作层：选目标（`target`）、启动（`start`）、停止（`stop`）、重启（`restart`）、恢复（`restore`）、别名（`alias`）、列举（`list`）、只读清点（`enumerate`）。每个动作把带数据的结果交回调用方，自己不打印、不结束进程——命令行与桌面界面共用；`tests/lib_is_quiet.rs` 守着「库里没有 `println!` 与 `process::exit`」 |
| `src/spec.rs` | 拉起规格 `InstanceSpec`（配置文件、覆盖参数、钉住的端口、启动者环境里的代理变量与 `PATH`、日志文件）与期望状态记录 `run/<端口>.restore.json`；交接文件 `run/<纳秒>-<pid>.spec.json`。两种文件都以私有权限写；读路径（status/restore/list/看护者）遇到读不懂的记录只报告不删除，`stop <端口>` 按文件名删记录、不读内容 |
| `src/server.rs` | 服务承载：`serve_forever` 主循环、停止信号、日志初始化、期望状态记录的写与「用户要求停止」语义、配置错误落盘 startup.log |
| `src/proxy/` | 转发核心，按职责分文件：入站来源校验（Host/Origin）、hop-by-hop 过滤、内存+磁盘双模 spool、错误判定、尝试结局分类（`attempts::classify`）与唯一的尝试循环（`attempts::run_attempts`）、保活通道与提交点、断开保护、仅转发模式；format 链在这里接入：请求方向的重试游标与逐层补丁（`transforms.rs`）、decide 的表决（`decide.rs`）、响应方向的整包层与逐块会话（`streams.rs`）、直流上传里对顶层 `stream` 的扫描（`live_flag.rs`） |
| `src/transform.rs`、`src/transform/` | format 程序的运行时：协议 1 只走子进程的 stdin/stdout（一行 JSON 一问一答，`seq` 对回信），stderr 另读、打码后进实例日志；`pool.rs` 是每个 `[[format]]` 条目的进程池（握手、spawn 与 persistent、空闲回收、超时与坏回信的 worker 换新、流独占的 worker、熔断器），`pool/notify.rs` 是完成通知的队列与它自己的熔断器；线上类型在独立 crate `aproxy-envelope` |
| `src/history.rs` | 请求历史：记录（最近 1000 条）、体的保留与预算、已知密钥的替换、读取（列表、详情、分块读体）、事件与订阅者；转发路径经 `Recorder` 与 `BodyWriter` 往里写，控制通道的读 op 从这里取。见「请求历史」 |
| `src/decode.rs` | 检查用解码：按 `content-encoding`（gzip/deflate/br/zstd，多层逆序）解出一份**仅供检查/预览**的副本，转发字节不受影响 |
| `src/retry.rs` | 重试判定：状态码、错误 JSON（含流式 NDJSON/SSE 形态） |
| `src/config/` | 代理配置：`file.rs` 严格解析 config.toml（未知键即错误、带行列诊断）、`resolve.rs` 分层解析（命令行 > toml > settings 的 `defaults` > 内置）、`edit.rs` 只改目标键的写回（`~/.aproxy/config.toml`，可多份平行并存） |
| `src/settings/` | 内部配置（`~/.aproxy/settings.json`，整个 home 唯一）：`mod.rs` 内存结构与加载（无效文件沿用上一次有效值）、`file.rs` 严格解析（未知键/类型不对/更高 schema 都是带键路径的错误；`defaults` 段复用 config.toml 解析器）、`edit.rs` 按键路径打补丁的写回（密钥占位、读出口打码、按含密钥文件私有落盘） |
| `src/daemon.rs` | 守护编排：IPC（ping/shutdown/观测，`shutdown.keep_restore` 与 `features`）、实例注册表、分离拉起（`spawn_detached_with`：Windows 作业对象脱出、unix setsid；工作目录与代理环境按规格设置）、孤儿清理 |
| `src/spawn.rs` | 按拉起规格拉起并等就绪（`spawn_from_spec`）：start、restart、restore、安装的滚动重启、看护者重拉共用；规格经交接文件交给 `__serve`，按新 pid 认就绪，启动即死时立即返回并附上 startup.log 的新增内容；就绪时限 30 秒 |
| `src/process.rs` | 进程原语（平台层）：创建时间、存活与退出、镜像路径、按 pid + 创建时间核验身份（`record_identity`）与核验后终止、看护者等待进程死亡用的同步句柄 |
| `src/watchdog.rs` | 看门狗：看护者记录与选举、实例心跳、收养与处决、重拉退避状态机、`logs/watchdog.log` |
| `src/fsx.rs` | 原子写（`write_atomic`）：注册表、期望状态记录、安装状态、看护者记录共用；`write_atomic_private` 给含密钥的文件用（settings.json、config.toml、拉起规格、期望状态记录），临时文件创建的那一刻就带最终权限（unix 0600、Windows 只含本用户/SYSTEM/Administrators 的 DACL）。home 的访问权限也在这里：`create_dir_all` 与原子写在往 home 里写之前先保证 home 只让属主访问（`secure_home_in`），`access_of` 给 doctor 读权限 |
| `aproxy-ipc/` | aproxy 与同它打交道的程序（桌面界面、第三方客户端）共用的契约，独立 crate，依赖很少。`code.rs`：错误码表（名字、族、`details` 的键与类型、HTTP 状态与退出码），`docs/error-codes.md` 由它生成、`tests/code_table.rs` 比对；`redact.rs`：打码的唯一出口（URL、密钥头、占位对象、已知密钥替换）；`endpoint.rs`：控制端点、心跳节、安装宣告节的名字与 `home_id`；`home.rs`：home 的解析（永久兼容契约 C1）；`ledger.rs`：进程台账 `<home>/ledger/`（永久兼容契约 C3），长期运行的进程登记 pid、启动标记与角色，强制安装据此核验后结束进程 |
| `src/util.rs` | bin 侧共用小工具：时间戳、时长人性化、凭据打码、key=value 解析 |

## workspace 五成员

- **aproxy**（根包）：代理本体。编译闭包不含 switchyard 等转换库——本体
  体积不受 format 程序的功能影响。
- **aproxy-envelope**：format 协议 1 的线上类型（一行 JSON 的 serde 类型、hello、各阶段的
  消息与回信、补丁、报告），aproxy 与 aproxy-format 共同依赖，单源维护防契约漂移；第三方用
  Rust 写 format 程序时也可以直接用它。阶段名与决定类字段在线上是字符串，新增阶段只发给在
  hello 里声明了它的程序，旧程序永远收不到它不懂的消息。aproxy 0.2.0 正式版之前，它与
  aproxy、aproxy-desktop 同号发预发布版（发布前 `.github/scripts/align-version.py` 一起对齐），
  正式版时才发 0.2.0：正式版号一经发布就永久占用，而协议在 0.2.0 定稿之前还要改。
- **aproxy-format**：官方的 format 程序，按角色拆成子命令（`convert`、`keys`、`route`、
  `policy`、`adaptive`、`rewrite`、`heartbeat`），每个角色各占一个 `[[format]]` 条目、各有自己的
  worker 池；另有不走协议的工具子命令 `init`、`check`、`try`、`translate`。协议转换用
  switchyard-translation（钉住版本的第三方库），它只在这个包的依赖树里。`check` 与 `try`
  复用 `aproxy-ipc` 的配置解析（`config-file` 特性），所以与实例启动时用的是同一套校验。
  单独发 Release（format tag 触发独立 workflow）。角色的行为见 aproxy-format skill。
- **aproxy-ipc**：aproxy 与同它打交道的程序共用的契约：错误码、打码规则、端点命名、进程台账、
  home 的布局；配置文件的解析在可选的 `config-file` 特性后面。依赖刻意保持很少，因为本体、
  桌面界面与 aproxy-format 都链接它。版本与 aproxy 同步。
- **aproxy-desktop**：桌面界面（Tauri 2 外壳与嵌入的前端），aproxy 的 `desktop` 特性引入它，
  版本与 aproxy 同步。它不依赖 aproxy（否则成环）。见「桌面界面」。

CLI 定义（cli.rs）、动作本身（ops/）与子命令的渲染（commands/）分离；启动的父进程逻辑
（配置解析、端口预检、后台拉起）在 `ops/start.rs`，由 `commands/start.rs` 串起来，服务本身在
`server.rs`——守护子进程由 `commands/start.rs` 直接进入它。库不打印、不结束进程：看护者
主循环返回后由 `main.rs` 结束进程，「删台账条目再退出」的便利函数是二进制一侧的 `util::exit`。

## 内存+磁盘双模缓冲（disk_cache）

请求体与上游响应的缓冲有两种归宿，按大小自动切换：

- **内存**：≤ 1 MiB（`RESIDENT_LIMIT`）全程驻留内存，小流量零磁盘 IO（真实
  agent 流量的绝大多数）。
- **磁盘 spool**：超出 1 MiB 即溢写 `~/.aproxy/spool/<pid>-<启动标记短哈希>/*.spooltmp`
  （按进程分目录，不按端口：按端口时，同一端口上第二次启动的进程会在 bind 失败之前清掉运行中
  实例的缓存；启动标记防 pid 复用。实例正常停止时收掉自己的目录，已退出的进程留下的目录由
  开始服务之后的实例与看护者清理），
  重试重放与响应回放流式读文件——进程内存与负载大小解耦。读写走 OS page
  cache，SSD 上吞吐与纯内存持平（实测数据见 [benchmark-memory.md](benchmark-memory.md)）。

生命周期保证：临时文件在请求结束（Drop）、回放流 EOF、重试丢弃三路删除；
实例启动时清空本端口目录回收崩溃残留。磁盘写失败按 `SpoolFailed` 终态处理
（507 `local_storage_failed` / SSE error 事件），绝不退化成内存堆积。`disk_cache = false`（settings
的 `defaults.disk_cache` 或 toml 覆盖）关闭时回到纯内存行为。`delivery = "forward"` 模式下请求体与响应
均**不缓冲**，本节整节不适用——见「仅转发模式（delivery = "forward"）」。

## 请求历史

实例在内存里记住它处理过的请求（`src/history.rs`），供控制通道的 `requests`、`request`、
`body`、`subscribe` 读取。它只是旁路：转发路径把经过的东西交给它，任何一步都不等它，也不因为
它失败。

- **记录**：最近 1000 条，先进先出，在途的不淘汰。一条记录是方法、路径（查询串里的凭据打码）、
  顶层的 `model` 与是否要流、投递方式、阶段、结局与终态码、心跳次数、字节数，以及各次尝试——
  发出与结束的时刻、上游状态码、失败原因、重试决定（默认规则还是第几条 `[[retry.rules]]`）。
  尝试只留最前 3 次与最后 20 次，中间的记个数；一条记录序列化后不超过 64 KiB（超了就把
  `path`、`user_agent`、`model` 截短并带 `truncated`）。入站校验挡下的请求（Host、Origin）
  不进历史。
- **体**：请求体、每次尝试发给上游的体（与上一次是同一份时只记 `same_as_previous`）、每次尝试
  收到的响应体，整份保留，合计不超过 `history.body_budget_mb`（默认 64 MiB，0 = 不保留）。超了
  从最旧的请求开始丢体（状态 `evicted`），记录留着；单个体比整个预算大就不保留
  （`not_retained`，`cause` 为 `exceeds_budget`）。带 `content-encoding` 的体先攒原始字节，收完
  后在阻塞线程池里解码再入库，解不开的为 `decode_failed`。
- **密钥**：配置里已知的密钥（`Config::known_secrets`）在入库时就被替换，头与体都是；按流
  替换，留一段「最长密钥减一」的尾巴等下一块，被块边界切开的密钥也换得掉。历史里从来没有
  这些密钥的原文，所以读出来的东西不需要再打码。认不出来的第三方密钥原样保留。
- **不拖慢转发**：留在内存里的请求体（≤ 1 MiB）与重试重放共用同一份内存，不拷贝；溢写磁盘的
  请求体，溢写之前攒的那 1 MiB 整块进历史，其余的与响应体一样拷进 64 KiB 的块。网络库交出
  来的块不直接留——它们是读缓冲区的切片，留一小片会把整个缓冲区留住，实际占的内存就对不上
  预算。64 KiB 的块与 1 MiB 的缓冲都循环使用（各至多留 8 MiB 空闲的），所以历史占的内存是
  「预算加至多约 16 MiB」；丢体时内存的释放放在记录表的锁之外。验收的门槛是 64 并发、2 MiB
  请求体的压测（`examples/loadgen` 对 `examples/mock_upstream`）下，没有订阅者时吞吐不低于
  没有历史时的 95%、p99 不高于 110%；2026-11 在 Windows 上与前一个提交交替各跑七轮 30 秒，
  平均是 96% 与 103%，进程的 CPU 时间多 3% 到 4%。单轮吞吐本身有 5% 上下的抖动（单轮的比值
  从 93% 到 101%），比较时要交替跑、取平均，并对照 CPU 时间。
- **事件**：有订阅者时，请求开始与结束各发一个 `request` 事件（整条记录），中间的变化每条
  记录每秒至多一个（按事件的 `at` 算，相邻两个至少隔一秒），由一个 250 毫秒一轮的任务补发；没有订阅者时不序列化任何东西。每个订阅者
  排队 1024 个事件，跟不上的丢最旧的并收到 `lagged`（丢了几个），之后该重取快照与列表。推送
  一侧把排着的事件合成一批一次写出，实例忙的时候也跟得上。

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
- 两个列表都支持 `"*"` 关闭对应校验；`[]` 等同未配置。分层：config.toml >
  settings.json 的 `defaults.allowed_hosts`/`defaults.allowed_origins` > 内置空。
- 被拒请求本地回 403，文案点名配置项与修法：**绝不转发、绝不注入 api_key、
  绝不进入重试循环、不计入请求数**。这是唯一不经上游就终结请求的入口，而它
  只作用于从未转发过的请求——「无限重试」对放行的请求毫无改变。

## 重试与流式回放

请求体完整读入（超 `max_request_mb` 即 413，不转发——客户端断开即中止上游请求，
避免无谓计费）后进入重试循环。回 413 之前先把剩余请求体读掉丢弃（总时长 10 秒、
空闲 2 秒为限）：关闭一个还有未读请求数据的连接会让对端收到复位，客户端接收缓冲里
已到达的 413 随之丢失，它只看到连接被中止（Windows 上必现）。响应体**逐块暂存**（spool）到内存/磁盘（上限
`max_response_mb` 256MiB，超限按不可重试终态处理）——只有拿到完整响应才能保证
「失败即重试」。响应完成后按原始字节序回放（零拷贝：内存模式用
`Bytes::slice_ref` 共享原分配）。

### 保活通道与提交点

缓冲期间客户端什么也收不到，所以保活适用的请求要靠心跳维持连接：默认是 SSE 注释
（`: keepalive`），`[keepalive] heartbeat` 可换成任意字节（aProxy 不认识协议，心跳的形态
由用户按客户端决定；`heartbeat_problem` 只把关「写完后 SSE 解析器停在事件边界」）。
配置了声明 `heartbeat` 阶段的 `[[format]]` 时，每一拍的字节依次交给这些程序，各自可以换掉它
（见「format 链」）：一拍的时限就是到下一拍为止，迟到、出错或熔断的那一层按跳过处理，照发它
收到的字节；上游的响应到了、要交付时，还没回来的那一拍作废，不会插进回放的事件中间。**适用判定**（`keepalive_triggered`，在请求转换之前、按客户端视角）：
`keepalive.interval_secs` > 0、`delivery` 不是 `"forward"`，且按 `keepalive.trigger`
命中——`accept`（Accept 含 `text/event-stream`，旧判定）/ `body_stream`（请求体
JSON 对象顶层 `"stream": true`，磁盘溢写的大请求体同样只看顶层键）/ `any`（任一，
默认）。默认必须把请求体算进来：真实 Claude Code 的流式主请求是
`Accept: application/json` + `"stream": true`（2026-10-04 实测），只看 Accept
的旧判定让它永远进不了保活通道。

适用的请求**从首轮起**交给后台任务（`proxy_with_keepalive`）驱动，响应头的
**提交点**是个只发生一次、不可撤回的三态机：未提交 → 提交上游真实头 / 提交
骨架头。推动提交的只有时间与上游响应头，「需要重试」本身不提交——重试几次后
很快成功的请求照样拿到上游真实头。尚未提交时（每一次尝试、每一段退避）：
- 上游 2xx + 未压缩的 `text/event-stream`（且没有 format 程序声明 `response`、`stream` 或
  `decide`：它们还可能改写或否决这个响应）→ 立即
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

format 链同样在这个节拍内：请求链在保活通道的后台任务里、首轮之前进行，重试链、decide、
响应方向的各层与「上游无视 identity 的压缩体」的回放前解码也经同一个 select 驱动。程序慢
（`timeout_secs = 0` 时没有上界）时客户端照样在一个间隔后收到骨架头与心跳；请求侧在提交之后
才失败时以终态 error 事件收场。唯一例外是「尚未提交 + 上游回 2xx 非 SSE」时的 response
阶段——与上面的暂停同理，此时提交骨架会把非 SSE 的响应改坏，只能直接等它完成。

取舍：
- **上游编码改写**：保活适用的请求发往上游时 `accept-encoding` 一律改为
  `identity`（放在请求链之后，覆盖 format 可能写入的同名头；`override_headers`
  里配的同名头同样被覆盖，启动时 warn 一次）——往压缩流里插
  明文心跳会让客户端解压失败，所以这是对「保真透传」的有意例外；上游无视该要求
  仍压缩时（`head_is_committable` 兜底）不提交真实头，成功体回放前经
  `decode::decode_strict` 完整解码（截断/损坏即失败，绝不回放半截）。不适用保活
  的请求不改写。
- **提交后失去状态码自由**：状态行与响应头一旦提交就改不了，所以**已提交后**的
  终态失败（`max_response_mb` 超限、spool 写盘失败、压缩体解码失败、重试规则
  达上限）只能以 SSE `event: error` 事件收场，不能再回错误状态码/真实状态码；尚未提交
  时它们与不保活路径一致（JSON 错误响应 / 原样透传）。事件的 data 与错误响应的体是
  同一个 JSON，见「数据面的错误出口」。已提交后才拿到非 SSE 的成功体同样
  撤不回：照常回放并 warn，建议该实例设 `[keepalive] trigger = "accept"`。
- **非 SSE 流的暂停窗口**：上游先回 2xx 非 SSE 头、再迟迟不发体时，那次尝试期间
  客户端收不到任何字节（最长约 `timeouts.read_idle_secs`）——非 SSE 的流本就没有可用
  的保活手段，与不保活路径的等待特性一致。

没有保活通道的请求（`"stream": false`、`delivery = "forward"`、间隔为 0）沿用首轮快速
路径 + `proxy_without_keepalive`：成功后一次性回放，期间不向客户端写任何字节。

客户端在「只收到心跳」的等待中（已提交、尚未写出任何上游真实字节）等了至少
60 秒才断开时，守护日志会 warn 一条提示：请检查客户端的流空闲超时，并带上已等
秒数。不少 agent 客户端按 SSE 事件计时（Claude Code 600s、Codex 300s、Qwen Code
240s），注释与 ping 都不算事件，重置不了它（见 `.agents/memory/claude-code-stream-watchdogs.md`
与 `codex-stream-idle-timeout` 的黑盒实测）。提示不点名客户端，行为对任何客户端
都一样；开始回放之后的断开、以及 60 秒以内的断开（多为主动取消）不提示。

重试退避：前 3 次零延迟，第 4 次起 5s→10s→20s→…封顶 `retry.max_backoff_secs`
（默认 320s），无限重试。客户端断开立即中止上游请求并停止重试（计费保护）。

例外——**重试规则**（`[[retry.rules]]` 配置，`path` 正则对「路径?查询串」
整体匹配并自动锚定，`status` 限定状态码，`max_attempts` 必填；哪些请求受限由
用户按上游配置，不内置任何 URL）：按书写顺序取第一条全部匹配器都成立的规则，
第 N 次带上游响应的失败命中且 N ≥ `max_attempts` 时原样透传并终止重试；
`max_attempts = 0` 是不限，写在宽规则前面当豁免。网络错误不触发，继续重试——
部分上游对特定端点确定性报错，重试到天荒地老也不可能成功，只会让客户端永远
等不到终态。
保活适用的请求同样如此：达上限时响应头尚未提交（常态——失败都很快）就原样
透传；只有响应头已被保活节拍以骨架提交的，才因无法再透传真实状态码而改以终态
SSE `event: error` 事件收场。透传的响应状态码与体保真，只多 `x-aproxy-error:
retry_exhausted`（`details.by` 为 `rule:<序号>`）与 `x-aproxy-request-id` 两个头。

## format 链（协议 1）

`[[format]]` 的每个条目是一个外部程序。aproxy 与它之间只有 stdin/stdout：一行 JSON 发过去，一行
JSON 回来，带同一个 `seq`；回信对不上、多出一行、不是合法 JSON，这个 worker 就被换掉——否则
下一个请求会读到上一个请求的回信。每个进程启动后先握手一次（hello）：协议版本、它支持的阶段、
各阶段要多少请求体（`full`、`digest`、`none`）、可选的能力；实例启动时对每个程序握手一遍，
配置声明的阶段它不支持就不启动。

**链**：列表顺序就是请求方向的顺序，离客户端最近的在前；响应方向逆序。同一个阶段可以有多个
程序，前一个的输出是后一个的输入（洋葱）。每个程序有自己按请求保存的不透明 `state`，由 aproxy
存着、在之后的阶段交还给同一个程序，落在哪个 worker 上都一样；程序之间不互相读 `state`。

| 阶段 | 时机 | 回信能做什么 |
|---|---|---|
| `request` | 请求体读完（或直流开始）之后、第一次尝试之前 | 请求补丁（方法、URL、头、体）、`delivery`、按请求覆盖、`fail` |
| `retry` | 重试之前重跑重试链时；带上一次的失败与上游的状态、响应头 | 同上（不含 `delivery`） |
| `decide` | 尝试失败之后；声明了 `decide.success` 的程序在成功之后也问 | `decision`（retry 或 stop）、`delay_ms`、`reroute`、stop 附带的响应 |
| `response` | 判定成功的响应交付之前，整体 | 响应补丁，或 `fail` |
| `stream` | 判定成功的响应逐块：open、data、close、abort | 每块的输出字节；open 时可改头并给出 `abort_tail` |
| `heartbeat` | 每一拍保活 | 这一拍的字节 |
| `notify` | 请求结束之后 | 空回信；aproxy 不在数据通路上等它 |

几条贯穿各阶段的规则：

- **决定类字段取第一个表态的程序**（`delivery`、`decision`、`reroute`）：配置里的顺序就是
  优先级，排在后面的程序不会悄悄推翻前面的决定。decide 问到两项都有着落为止。
- **重试链**：声明了 `retry` 的程序在请求方向上必须连成后段（配置校验）。重试时默认重跑这一段，
  每个程序从链的原始输入重新给出请求；decide 回 `reroute:false` 时沿用上一次发出的请求，不调
  任何程序。按请求覆盖（超时、重试上限等）属于这一轮的输出，重跑时上一轮的覆盖作废。
- **`fail` 是决定，出错是故障**：`fail` 不论程序是否 required 都照办（`retry:false` 结束请求，
  `retry:true` 让这次尝试失败并重试）。进程级故障（起不来、退出、超时、回信不合法）才看
  `required`：请求方向上 required 程序让这次尝试失败（`format_error`），可选程序被跳过；decide
  出错一律算不表态；响应方向出错一律跳过这一层、把上游的响应原样交付并告警；心跳出错这一拍
  用输入的字节。**绝不因为 decide、响应侧或心跳的程序出错而结束请求。**
- **熔断**：每个条目一个熔断器，只在内存里。连续 `breaker_failures` 次进程级故障后打开，杀掉空闲
  worker，`breaker_cooldown_secs` 之后放一条消息试探。打开期间，required 程序在请求方向上让
  请求立即以 503 `format_unavailable` 结束，其余情况按跳过处理。notify 另有自己的熔断器，
  观察路径出问题不会把判断路径一起关掉。
- **stream 的 worker**：一条流固定由一个 worker 处理，独占到 close 或 abort，不计入 `pool_max`
  （进程数等于池子大小加并发的流数）。流中途某一层失败时，aproxy 停读上游，向它外侧的各层发
  abort；写给客户端的收尾只有一个来源：最外层对 abort 的回信，最外层自己失败时才是它在 open
  时存下的 `abort_tail`。
- **notify 只发不等**：有空闲 worker 且没有数据通路的消息排队时才派发，固定 2 秒时限，队列有
  条数与字节上限，超出丢最旧的并计数（快照的 `formats[].notify_dropped_total`）。

**投递方式按请求选**：`delivery` 的配置值只是起点。request 阶段第一个给出 `delivery` 的程序
决定这个请求按 `retry`（缓冲、重试、保活）还是 `forward`（真流式、不重试）处理；响应头一旦
写给客户端就不再换。forward 请求在一个字节都没写出之前失败时问一次 decide，回 `retry` 就当场
升级为 retry，此后与一开始就是 retry 完全一样。起始是 retry 的请求总是先读完请求体；起始是
forward 的请求，链上有程序要请求体或摘要、或有程序声明了 decide 时也先读完（升级之后要重发），
否则请求体直流上游。forward 请求只有 2xx 的响应经过 response 与 stream；response 程序要整个
响应，那一层会先攒齐。默认保持 retry、由判断类程序把安全的请求改成 forward，是有意的：判断
程序坏了，退路是无限重试而不是不重试。

**单独重启与 control。** `aproxy restart <目标> --format all|<name>`（窗口的实例页也能做）只换
某些条目的进程，不换实例、不重读 config.toml：新进程先握手，通过之后才接手；已经发出的消息在
旧进程上收尾，同一请求之后的消息交给新进程（所以按请求的事实要放进 `state`）。握手不过就什么
都不替换，与 `restart` 预检不过时旧实例照常运行是同一个道理。程序可以在 hello 回复里提供
`control`：第一次有调用时，aproxy 用它给的参数另起一个同一程序的进程，只收 `stage:"control"`
的调用，方法与含义由作者定；这个进程不占请求的 worker，也不碰熔断器。每个条目同时只办一个
调用、最多 16 个排队，连排队与启动在内以 5 秒为限。扩展经 `api.formats` 调用它，做与某个
format 配对的界面。

官方角色与这些阶段的对应（`convert`、`keys`、`route`、`policy`、`adaptive`、`rewrite`、
`heartbeat`）见 aproxy-format skill；配置键见 aproxy-cli skill 的 `config-toml.md`。

## 数据面的错误出口

aproxy 自己结束一个请求时说的话只在 `src/proxy/errors.rs` 构造，码取自码表
（`aproxy_ipc::code`，全表见 [error-codes.md](error-codes.md)）：

- **错误响应**（响应头还没写出）：`content-type: application/json`，体是
  `{"type":"error","error":{"type":"aproxy_<码>","message":…,"aproxy":{…}}}`。
  `error.aproxy` 给程序读：`code`、`request_id`、`attempts`（已进行的上游尝试数）、
  `upstream_status`（有上游响应时）、`details`（键与类型由码表登记）。`message`
  是一句英文，不是契约。状态码取码表登记的值。
- **SSE 终态事件**（响应头已提交）：`event: error`，data 是压成一行的同一个 JSON。
  源码里只有 `Terminal::sse_event` 一处构造它。
- **两个头**：`x-aproxy-request-id` 与 `x-aproxy-error`。自产的错误响应带；放弃重试
  之后原样透传的上游响应也带（体与状态码保真）；正常透传的响应不带——带
  `x-aproxy-error` 的响应就是 aproxy 已经放弃、不会再重试的最终结果。
- **请求标识** `request_id` = `<instance_id>-<序号>`：`instance_id` 是 16 位十六进制
  （启动时刻毫秒的低 48 位与 pid 的低 16 位），序号是实例内第几个请求。响应头、错误
  体、交给 format 程序的消息里是同一个串；这个请求的每一行日志都挂在带
  `request_id=` 的 span 下（保活通道的后台任务也是）。入站校验拒掉的请求不计入
  请求数，标识用单独的序号（`<instance_id>-x<n>`）。
- **尝试失败码**：每次上游尝试没成功都归成一个码（`AttemptFailure`）——连不上、
  连接超时、等不到响应头、读空闲超时、连接中途断开、上游回 4xx/5xx、2xx 但体是
  错误——进日志（`code=`）与实例的「最近错误」，也是终态里 `last_code` 的来源。

现在能触发的终态：`inbound_host_rejected`、`inbound_origin_rejected`（403）、
`request_too_large`（413）、`client_body_aborted`（400）、`upgrade_unsupported`
（501，只拦 WebSocket 升级）、`local_storage_failed`（507）、`response_too_large`
（502）、`retry_exhausted`（透传）、`retry_stopped`（format 的 decide 叫停：它给的响应，或
透传，都没有时 502）、`format_rejected`（请求侧 format 回 `fail` 且 `retry:false`：它给的
响应，否则 422）、`format_unavailable`（503，required 程序已熔断）、`format_failed`（502，
forward 请求的请求侧 format 失败且没有升级为 retry）、`upstream_failed`（502，forward 请求的
上游尝试失败且没有升级）、`decode_failed`（只作 SSE 事件）、`internal`（500）。尝试一级另有
三个来自 format 的失败码：`format_error`（程序出错）、`format_fail`（回 `fail` 且
`retry:true`）、`format_decision`（decide 把成功的尝试判为失败）。完整的表由
`aproxy-ipc/src/code.rs` 生成在 [error-codes.md](error-codes.md)。

仅转发模式整体旁路本节：不缓冲完整请求体、不进重试循环、不发心跳——
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

## 仅转发模式（delivery = "forward"）

`delivery = "forward"`（toml 显式值，或 settings.json 的 `defaults.delivery`）时，实例的请求
起始就按仅转发处理；默认 `retry` 的实例上，format 程序也可以把单个请求改成 forward（见
「format 链」的「投递方式按请求选」）。下面先说没有 format 程序时的路径，它与上面的主流程
完全不同：**不缓冲、不重试、不保活**。请求体边收边发上游，
响应边收边回客户端。这是给「上游可信、且要真流式」场景的**显式取舍**——它放弃
了本产品最核心的重试保障，请勿当作普通开关随手打开。

- **分叉点**：在 `read_request_body` 之前判定（否则缓冲已发生，模式失去意义），
  且在请求计数（`IpcStats::note_request`）之后（否则 status 的请求数恒为 0）。
- **请求体**：`req.into_body().into_data_stream()` 经计数适配器交给
  `reqwest::Body::wrap_stream` 流式直发——一律流式，不做空 body 特判；不缓冲、
  不落盘。计数在流式途中进行，超 `max_request_mb` 即先读掉客户端剩余的请求体（同
  缓冲路径的上限与理由），再让请求体流产出 `Err`（令 reqwest 中止上游请求），回
  413（复用既有文案）。**`max_request_mb` 是本模式下
  唯一仍然强制的限制。**
- **响应**：`resp.bytes_stream()` 直回客户端，状态码与响应头原样透传；
  hop-by-hop 照旧过滤，但 **`content-length` 必须保留**（字节未经变换，
  上游声明仍精确）。
- **上游请求失败**：502 `upstream_failed`（`details.last_code` 是那次尝试的失败码），不重试，并调 `state.note_upstream_failure`
  记录——否则 status 的「最近错误」对这类实例永久显示「无」。
- **响应流中途中断**：**直接截断**，不注入任何上游未发出的字节；
  `tracing::warn!` 记录错误与已转发字节数，并调 `note_upstream_failure`。
- **客户端断开**：响应 Body 被 drop，reqwest 连接随之关闭——既有计费保护靠
  Drop 天然成立。
- **不进入的路径**：重试循环、SSE 保活骨架、错误内容拦截
  （`is_error_body`/`is_stream_error_body`）、spool、`keepalive.trigger` 判定。
- **本模式下不生效**：`disk_cache`/`max_response_mb`/`keepalive.*`/`retry.*`
  （没有 format 程序时磁盘 spool 完全不参与）。
- **与 format 共存**：需要整包的层在它的输入处缓冲（请求体被程序要走、或有 decide 程序时先
  读完；response 程序那一层攒齐响应），其余照旧直流；失败且未写出字节时可由 decide 升级为
  retry。「200 但内容是错误」不拦截，但在旁路上照做判定并记入请求记录与完成通知
  （`upstream_error_body`），判断类程序才看得见这类失败。
- 消费方一律走 `Config::forward_only_enabled()`（`unwrap_or(DEFAULT_FORWARD_ONLY)`），
  **不得 `unwrap()`**——doctor 的 `parse_config_file`、`find::discover` 与大量
  测试的 `AppState::new` 都不经 settings 注入。`proxy::router(AppState)` 签名不变。

## 配置分层

四级优先级：

```
CLI 覆盖参数（`start` 的 `--base-url` 等，仅本次） > config.toml 显式值 > settings.json 的 defaults 段 > 内置默认
```

settings.json 的 `defaults` 段收的就是 config.toml 的键（同名同嵌套，以 JSON 写出），所以除
`listen_addr`、`log_file`、`format` 这三个只能属于单个实例的键外，每个键都有全局默认层；这三
个键写进 `defaults` 是 `config_unknown_key`。标量与数组整体取最高层（toml 里写空数组也算显式
设置），两张头表按头名跨层合并、toml 优先。

`defaults` 里的凭据不按普通层继承，而是**跟随它的目标**：`api_key` 与 `extra_headers`/
`override_headers` 里的密钥头，只在这份配置最终的 `base_url` 与 `defaults.base_url` 指向同一目
标时继承；`proxy_username`/`proxy_password` 只在 `proxy` 相同（scheme、主机、端口）时继承。
比较前先规范化 URL（去空白与结尾斜杠、scheme 与主机小写、去默认端口；路径参与比较，userinfo
不参与——它是凭据不是目标）。没继承的项进 `Resolved::withheld`，`config show --effective` 列出它们；这
份配置自己也没有该凭据时另发一条 `default_credential_withheld` 告警，否则用户只会看到上游
401。密钥头按名字识别（`aproxy_ipc::redact::is_secret_header`）；名字不常见的凭据头不会被识别，
会随每个目标继承，文档因此建议这类头写进各实例的 config.toml。

`config.toml` 人类可读可写、可多份（多开各自指定）；`settings.json` 是整个 home 的程序状态，
**全局唯一**，分节存放：`schema`、`language`、`aliases`、`default_config`、`config_dirs`、
`defaults`、`watchdog`、`logs`、`instances`、`update`、`ui`（保留不用：只查它是对象、写入时原样保留；
界面偏好存在 `<home>/desktop/` 下，不在这里）。它被严格解析：
未知键、类型不对、取值非法、更高 `schema` 都是 error 级诊断；没有自动迁移，0.1.x 的平铺字段
按未知键（`watchdog` 布尔值则按类型不对）报错。写入走 config.toml 同一把写锁与原子写，只打补丁目标键，并因可能含明文密钥而以
所有者私有权限落盘（`write_atomic_private`）。无效文件不参与任何实例的启动：`main` 在分派
前统一检查，`settings`/`doctor` 照常运行并自己报告，`status`/`stop`/`install` 按内置默认运行并
告警，其余命令退出码 3——带着没生效的设置去启动，比不启动更糟。

### 配置别名（settings.json）

- `aproxy alias add <名> [路径]`：路径省略时指向默认 config.toml；
  别名不得为 `all`/`idle`/`default`/`defult`/纯数字（保留字与端口解析冲突）
- `aproxy start/stop <别名>`：start 把别名解析出的配置文件路径与覆盖参数写进拉起规格（别名解析只发生在父进程），子进程经 `__serve --home <dir> --spec <file>` 接手；stop 按实例注册的
  config_path 归一匹配（大小写/分隔符），端口变了别名依然有效

## 守护进程模型

- `aproxy start`（默认配置）= 后台启动；不给子命令的 `aproxy` 只打印帮助并以 2 退出：父进程预检（IPC ping → TCP bind 探测）→ 把这次启动写成拉起规格
  `InstanceSpec`（配置文件或 null、覆盖参数、启动者环境里的代理变量与 `PATH`、日志文件；端口为 0
  时首次启动后钉住实际端口）→ 经交接文件 `run/<纳秒>-<pid>.spec.json` 交给子进程
  `aproxy __serve --home <dir> --spec <file>`（`spawn_detached_with`，Windows 手写
  `CreateProcessW`，`bInheritHandles=FALSE` + `CREATE_NO_WINDOW` + `CREATE_BREAKAWAY_FROM_JOB`，
  unix `setsid`）→ 父进程按新 pid 轮询 IPC 就绪（≤30 秒）后返回，并删掉交接文件。密钥只在交接
  文件与期望状态记录里（都只本用户可读），不进实例与看护者的命令行（`aproxy start` 自己的
  短暂命令行上仍有用户给的覆盖参数）。
- 子进程承载服务：读交接文件里的规格 → 宽松解析定出日志路径并初始化日志 → 严格校验配置（失败写
  startup.log 并退出）→ 建本进程的 spool 目录 → bind → 写实例注册表
  （`run/<端口>.instance.json`，含实例上报的 log_path；之后每 30 秒核对一次，丢了就补写）→ 写期望状态记录（`run/<端口>.restore.json`，内嵌
  规格）→ 启动 IPC 管道 → 创建看门狗心跳节 + 心跳 ticker（10s）→ 守护侧互保任务（5 分钟自检）
  → serve。前台实例（`--foreground`）走同一服务主循环，但不写期望状态记录、日志写控制台。
- 停止：IPC `shutdown` → 优雅关闭（10s 宽限强退）→ 清注册表与期望状态记录。带
  `keep_restore` 的 shutdown（`restart` 与安装滚动重启使用）只清注册表，记录留着；不认这个
  参数的旧实例会照常删记录，调用方等它退出后再把记录写回（见 `features`）。
- 实例脱离启动者：unix 上 `setsid`，终端的 Ctrl+C 到不了它；Windows 上先带
  `CREATE_BREAKAWAY_FROM_JOB` 拉起，作业对象不允许脱出时去掉该标志重试并往 startup.log 写一条
  告警（它仍在作业里，作业结束会连带结束它）。工作目录固定为 home，环境是拉起方环境套上规格里
  记下的代理变量与 `PATH`。

### 端口冲突的两种情况

1. IPC ping 通 → 同端口已有 aProxy 在运行（不重复启动，exit 0）
2. 管道不通但 bind 失败 → 被其他程序占用 / 无权限或被系统保留
   （Hyper-V/WinNAT 排除区间，按错误类别报因，exit 1）

### 实例身份

端口号是实例唯一键：同端口不同监听地址的第二实例会在启动时被显式拒绝
（注册表与 IPC 管道按端口命名，无法并存）。

## 看门狗（watchdog）

全局单看护进程（没有公开子命令，同二进制以内部入口 `aproxy __watchdog --home <dir>` 分离
启动），把「进程级死亡/挂死 → 永久断流直到人工发现」降级为「秒级检出 → 退避重拉」。实测
+2.4% 体积、+2.9MB 常驻、热路径零损耗（[benchmark-watchdog.md](benchmark-watchdog.md)）。

- **进程身份**：一律按「pid + 进程创建时间」核验，与二进制文件名无关（改名
  部署的实例同样受看护）。实例在注册表登记 `process_start`（Windows FILETIME /
  Linux `/proc` starttime）；收养、处决、接管假死前任、install 换血停旧看护者、
  `stop --force` 都走这一判据。登记值为 0（平台读不到创建时间）的记录证明不了
  身份，一律不收养、不处决、不强杀，保守优先（杀错进程不可逆）。
- **死亡检测**：收养时 `OpenProcess(SYNCHRONIZE|TERMINATE)` 取进程句柄，
  `spawn_blocking` 内核阻塞等待——死亡信号即时、零轮询线程。死亡事件到达即
  唤醒主循环处理，崩溃实例在其启动耗时内回来，不等扫描周期。主动强停的一方
  （`stop --force`）因此必须在终止**之前**摘掉期望状态记录
  （终止失败再放回），否则看护者会先于它的收尾读到「记录在 = 崩溃」，把刚
  强停的实例拉回来；`restart --force` 则保留记录，两边用同一份规格，看护者抢先拉起也算收敛。
- **挂死检测**：实例侧独立 ticker 每 10s 写心跳（8 字节毫秒时间戳）：Windows 写命名共享
  内存节 `Local\aproxy-<home_id>-heart-<端口>`（`home_id` 同控制管道），unix 写 run 目录里的
  文件 `run/<端口>.heart`（在 home 里，不用 `/dev/shm`；写入交给阻塞线程池，文件系统卡住
  不占运行时的工作线程）——挂死 = runtime 无法调度 = ticker 停摆，与请求热路径零耦合。
  崩溃留下的心跳文件由清理收走。看护侧扫描过期 +
  IPC ping 二意见都失败才终止进程（句柄绑定原进程，PID 复用免疫）。
- **重拉与退避**：期望状态记录残留 = 异常死亡信号；优雅退出（记录已删）摘除
  看护。重拉从记录里的拉起规格启动，首次崩溃立即重拉；重拉失败进退避队列，按指数退避
  1/2/4/8…封顶 300s 由主循环时间驱动重试（等待不阻塞 tick——否则长退避会让看护者记录停止更新、
  现任被竞争者按「假死」夺权），就绪判定按新 pid 定位；重拉后实例落在不同
  端口（toml 改了端口）时退役旧端口的期望状态记录与实例记录并按实际端口续看护。
  连续失败达 `watchdog.max_restarts` **不再放弃**：转入每 300s 一次的慢速重试，直到实例起来或
  记录被 `stop` 删掉，并在刚越过上限时往 startup.log 写一条英文说明；`max_restarts = 0` 才是
  「不自动重拉」（记录保留，交给 `aproxy restore`）。重拉出来的实例存活满 60 秒，之前的连续
  失败才清零——起得来、几秒后又崩的实例按失败计数退避，不会每次都被立即重拉。
- **设置热更新**：每个 tick 重读 settings.json。`watchdog.enabled` 关掉时删掉看护者记录退出
  （记录不动，之后重新打开时崩溃实例照样能被拉回）；`max_restarts`、`idle_exit_secs` 下一轮
  生效；`heartbeat_secs`、`stale_after_cycles` 不在运行期改——别的进程按看护者启动时的周期判断
  它是否在任。文件无效时沿用上一次读到的有效值。
- **选举规范**（多守护并发拉起看护者的唯一性保障）：看护者记录
  `run/watchdog.json`（`schema`、PID、启动标记、版本、扫描周期 `heartbeat_secs`、`created_at`、
  `last_scan_at`、最近 20 次重拉 `restarts`）为在任真相源，在任的看护者每个 tick 整份重写——
  排序定发起者（核验在世的实例中 PID 最小者才有权 spawn；CLI `start` 不在在世
  集合内，视为有权，多发起由记录的原子接管兜底）+ `create_new` 原子接管
  定在任者；启动标记（台账用的同一个）比对拒绝 PID 复用冒名；假死前任（`last_scan_at` 早于
  三个扫描周期、且不短于 90 秒）经核验后终止接管。读不懂的记录（schema 更高）不解读、不覆盖，
  只按文件修改时间判断新鲜；在任者发现记录已不是自己的就让位退出。
- **找旧看护者靠台账**：实例启动时退役比自己旧的看护者、install 换二进制之前停掉看护者，都按
  台账里 role 为 watchdog 的条目找，按「pid + 启动标记」核验后结束，不读看护者记录——记录的
  格式随版本变，台账是永久兼容契约。
- **看护者日志**：处决（挂死）、重拉的结果与起因、转入慢速重试、摘除看护、接管前任与退出，
  都以英文写进 `logs/watchdog.log`（达到 `logs.rotate_mb` 时清空重写）。看护者是脱离终端的
  后台进程，这是它的处置唯一看得见的地方。
- **互保**：守护与看护者完全解耦（看护者死亡实例不受影响）；守护每 5 分钟
  自检，缺席即按选举规范补种。全部实例清零、且没有待重拉的条目后，看护者闲置自灭
  （默认 300s），系统回到零常驻；慢速重试队列非空时它一直留着。
- 配置：settings.json `watchdog` 段的五个键（`enabled` 总开关 / `heartbeat_secs` 扫描周期 /
  `stale_after_cycles` 挂死容忍 / `max_restarts` 快速重拉上限（之后转每 300s 慢速重试）/
  `idle_exit_secs` 闲置自灭）——系统级单例故无 toml 层。

## 自愈恢复（restore）

`run/<端口>.restore.json` 是期望状态记录，内嵌拉起规格：配置文件（可为 null）、覆盖参数、钉住
的端口、启动者环境里的代理变量与 `PATH`、日志路径。

- **写入**：后台实例 bind 成功后写（前台实例不写——关掉终端就是用户要它停下）
- **删除**：只有 `stop` 的各条路径删它（优雅停止由实例自己删，`stop --force` 在终止前由 CLI
  删，`stop` 一个已崩溃的目标 / `stop all` 删掉已不在运行的实例留下的记录）；`restart` 与安装
  的滚动重启从头到尾不删，新实例就绪时以同一份规格覆盖
- **保留**：崩溃、断电、系统重启、`restart` 失败、配置文件暂时找不到、读不懂的记录（读路径只
  报告、不删除；`stop <端口>` 按文件名删，不读内容）

`aproxy restore` 按记录经 `spawn_from_spec` 拉起（`__serve`），就绪判定按新 pid（≤30 秒）；
已在运行跳过（幂等）；空清单静默 exit 0（开机自启友好）。配置文件找不到时记录保留并说明，文件
回来后再执行一次即可；没有配置文件的规格（只靠覆盖参数与全局默认启动）不做这项检查。

读路径（`status`、选目标、`logs`、界面轮询）只读，不删任何文件；实例的期望状态记录更不
归它们管——崩溃实例恰恰靠它存活到 restore 执行。`restore` 按新 pid 定位
实例并报告实际端口；实例落在不同于记录的端口时，新端口写了新记录、旧端口那份被接替。端口为 0
的实例把首次拿到的端口钉在规格里，配置仍是 0 时回到那个端口；配置改成固定端口后以配置为准。

### restart：先预检、后停止

`restart` 的主要用途是「改完 config.toml 让它生效」，而配置笔误恰恰最常出现
在这一步。先停旧实例再由新实例读配置，配置有错时新实例秒死，一个健康的服务就此中断。因此
stop 之前先按新实例将走的同一条解析路径（`start::resolve_runtime_config`：严格 TOML 加载、
CLI 覆盖、settings 注入、validate）干跑一遍，换监听地址时再探测新地址可否绑定；预检不通过
就不碰旧实例。停止用带 `keep_restore` 的 shutdown，期望状态记录始终在；新实例起不来时记录
留着，修好原因后 `aproxy restore` 或看护者能把它拉回。`restart all` 逐实例预检，失败的跳过、
其余照常重启，最后汇总并以非零退出；新实例启动即退出时展示 startup.log 本次新增内容与
`aproxy restore` 的恢复方式。没有记录的实例（前台实例，或 alpha.1/alpha.2 写的旧格式记录）
明确拒绝，旧实例不动。

## 日志治理

- 守护日志 `logs/`：按启动时刻**随机命名**（十六进制时间戳-pid 格式，如
  `19ac3f2e8b5d-1a2b.log`），每次启动（含 restart、restore）都是新文件——
  换端口后日志不断档；启动时 >2MiB 截断；运行期每小时检查，>8MiB 截断
- **路径解析序列**（`main.rs`/`server.rs`）：宽松 load 配置（解析失败不阻断
  日志初始化）→ resolve（CLI `--log-file` > config.toml `log_file` > 内置
  随机名；`~` 展开，相对路径相对 APROXY_HOME——守护 cwd 不可靠）→ 日志
  init → 实例经 IPC 上报 log_path（`InstanceRecord.log_path`），注册表与
  期望状态记录均携带
- **地址以 IPC 为准**：`aproxy logs [PORT|别名] [-n N] [-f]`（默认打印最近 N=200 行后退出；
  `-f` 时按 tail -f 语义继续增量轮询，实例停止自动退出）与 start 成功提示都向实例询问真实
  路径，客户端不拼路径；`--foreground` 实例 log_path 为空，logs 立即报
  「日志输出在它的控制台」
- 清理（`daemon::collect_garbage_in`，实例开始服务之后一次、看护者每轮一次；读路径不删文件）：
  - 实例记录 `<端口>.instance.json`：读得懂、且写者经「pid + 启动标记」核验已不在时才删；读不懂的不删
  - 期望状态记录：不归清理管，只有 `stop` 能删
  - 交接文件 `*.spec.json` 与原子写的临时文件 `*.tmp-<pid>-<纳秒>`：写者已不在（或核验不到它
    仍在运行）且早于 10 分钟时删（交接文件含明文密钥，不能一直留在盘上）
  - 实例日志：只删同时满足「内置随机名（`<十六进制>-<四位十六进制>.log`）、早于 10 分钟、不被
    任何实例记录或期望状态记录引用」的；`run/` 里有读不懂的记录时这一轮不删任何日志；自定义
    `log_file` 与 `startup.log` 等固定名永不动。引用集按 `path_match_key` 归一比较
    （大小写/分隔符差异不会误删）。推论：优雅停止实例的日志在下次清理时删除；崩溃实例的
    日志由期望状态记录引用保留到恢复成功
- 全部输出对凭据打码——可安全粘贴分享。URL 展示的**唯一出口**是
  `aproxy_ipc::redact::url`（base_url、代理 URL、上游目标 URL、请求的 `路径?查询串`、
  reqwest 错误里的 URL 一律走它）：
  userinfo（用户名或密码任一存在）整体 `***@`，查询串保留键名、值变 `***`，片段
  整体遮掉，畸形 URL 保守多遮；api_key 与头值走 `aproxy_ipc::redact` 的占位（文本形式
  `<secret>`，值不短于 24 字符时为 `<secret sk-a…f3a9>`；`--json` 为 `{"secret":true}` 与
  `"hint"`）。`config show` 另对 format `args` 的密钥旗标之后的值打码，`env` 只列名字、不显示值

## 安装与升级（install）

`aproxy install` 把二进制安全落位到 `~/.aproxy/bin/`，再逐个重启实例；
每次重启沿用优雅停止的 10 秒宽限，超过 10 秒的在途请求会被中断，新进程就绪前的新连接可能失败。
四条铁律：先标记（状态文件先于一切动作，崩溃可识别；它是 `<home>/install/state.json`，
代码与文档里习称 install.state）、先下载后
rename（staging 备料校验全过才动 bin）、ACK 齐了才交换（IPC PrepareSwap
广播，实例把自己的可观测状态置为 `swap_prepared`）、逐个重启最后删除（让位的旧二进制
`aproxy.old-…` 在终验后清理）。普通安装走不通时另有强制安装（见下文 force）。模块
`src/install/`：

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
- **安装者状态收在 `<home>/install/`**（`install::install_dir_in`）：状态文件
  `state.json`、备料与下载暂存 `staging/<版本>/`、一键脚本的下载暂存 `bootstrap/`，
  别处一律不写（skill 目录自己的 `.dl`、`.staging` 除外，要与目标同卷原子替换）。
  这是强制安装的永久兼容契约 C4 的路径部分：它可以整目录清空而不必认识内容。
  其中 `lock` 是操作系统级排他锁（`install::lock_install_in`，标准库文件锁，进程死亡
  即释放），任何版本修改 `bin/` 期间都持有它——state 文件管「同一时刻只有一个安装」，
  锁管「改 bin 的那一刻没有别人在改」，强制安装不读 state，只认锁。
  0.1.0 写在 `run/install.state` 与 `staging/` 的现场因此读不到，与新安装器自然隔离
- **版本**：`--version` 首行是永久格式 `aproxy v<semver> (<变体>; <目标三元组>[; x86-64-v3])`
  （`aproxy::VERSION_LINE`，契约 C5），在一切副作用之前输出；安装器靠它认版本
  （`staging::parse_version_line`），也靠它让 0.1.0 的安装器在交换前失败——0.1.0 取第一个
  以数字开头的词当版本，这一行没有。可安装版本的下限是 0.2.0（`install::INSTALL_FLOOR`，
  按主.次.修订比较），在线安装与 `--from` 都查，`--allow-downgrade` 越不过
- **state**：状态文件即安装锁（原子重写，updated_at 自动刷新供
  stale 判定）；阶段状态机线性主线 + failed/aborted 旁路，abort 仅
  swapping 前可回滚。交棒后等待结局的往往是较旧的二进制，所以阶段词表与
  字段名冻结、新字段一律可选
- **staging**：备料区（复制/chmod/sha256/`--version` 试跑自证）
- **swap**：平台收口点（契约 C2）。新文件先以唯一名 `aproxy.tmp-<纳秒>-<pid>` 落进
  bin，旧二进制让位为唯一名 `aproxy.old-<纳秒>-<pid>`（Windows 用不覆盖的
  MoveFileExW 改名——`std::fs::rename` 在 Windows 上会替换已存在的目标；unix 留一个
  硬链接，不支持时复制），再把新文件改名到位；落位后在新位置跑一次 `--version`
  核对，不符就换回。从不用固定的中间名：上一次让位的旧文件还被占着时，固定名必然
  撞名。删得掉的 `aproxy.old-*`、`aproxy.tmp-*` 残留在每次交换前清掉，删不掉的跳过。
  Windows 另有 PATHEXT 兜底入口 `aproxy.bat`（`aproxy.exe` 缺席的空窗里运行最新的
  `aproxy.old-*.exe`），由 swap 在每次交换前写入
- **announce**：安装态宣告（Windows 是命名节，进程退出即解除；unix 是
  `<home>/install/announce` 文件，正常结束时删掉，崩溃残留靠心跳过期失效，之后由
  发现没有状态文件可续的 `install --continue` 删掉，强制安装清空 `install/` 时也一并
  清掉）——看护者/守护的差异化行为全部由它门控：实例死亡复查 5×3s、install 保活
  续作、守护自检补种抑制
- **restart 与服务回滚**：实例重启原语（`install/restart.rs`）用带 `keep_restore` 的 shutdown
  停止实例，期望状态记录全程留在原处（旧实例不认这个参数时由调用方在它退出后写回）；新二进制
  起不来（bind 前退出、30 秒内没就绪）则用旧二进制按同一份规格把该实例拉回并置 `halted`，
  由 flow 中止滚动、剩余实例不动。安装状态因此不再存启动参数（`pending_restores` 恒为空）。
  回滚的只是这一个实例的服务——与 forward-fix
  铁律（swapping 之后只进不退）兼容：bin 里的新二进制与阶段都不回退（阶段落
  failed 保留现场），自动续作不再重试。旧二进制的来源：交换让位之前记进 install.state
  的 `old_path`（`aproxy.old-…`：Windows 是改名让位的原文件；unix 是单步 rename 覆盖前
  留下的硬链接或副本——覆盖后旧 inode 只挂在运行中进程上，文件系统里已无路径可
  执行，所以必须先留）；cleaning 阶段删除
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
  （settings `update.download_chain` 严格数组可配 + url 模板通道，CDN 域名不硬
  编码）；github=.sha256 强校验、npm=integrity 强校验（跟随 ~/.npmrc
  镜像）、cargo=本地编译自证；下载代理（`update.download_proxy`/`--download-proxy`）
  与上游请求代理绝对分离
- **skills**：skill 文档支线（并行下载、失败不影响安装、原子目录替换）
- **force**（`aproxy install --force`，契约 C6–C8）：普通安装失败、卡住或装着的版本坏了
  时的兜底。从 0.2.0 起，任意版本的强制安装都要能装任意版本、结束任意版本启动的进程，
  所以它只依赖冻结的 C1–C8，不走 IPC、不读注册表与 install.state、不读写配置与
  settings（main 在读它们之前就分流）。先验证、后破坏：目标取进 `install/force/<pid>/`
  （在线读 release 附带的发布清单 `aproxy-manifest.json` 选资产并比对 sha256，只走 HTTPS、
  空闲 60 秒超时、清单 1 MiB 与资产 256 MiB 上限，latest 不低于正在运行的版本；或
  `--from`），在本机跑通 `--version` 之前什么都不停；确认（非终端要 `--yes`）→ 按台账
  结束安装者 → 拿锁 → 结束其余角色（连续两轮没有存活条目为止，有上限）→ 清空
  `install/` → swap → 用新二进制的 `restore` 拉回实例（被强制结束的实例留着
  期望状态记录），按台账看被结束实例的端口上有没有新版本的 instance 条目 → 报告台账外、从
  bin 运行的进程（只报告；macOS 不做）。home 不存在直接 10。退出码冻结、按实际发生的事
  给：0 完成；10 一个进程都没结束；11 结束过进程、二进制没换；12 二进制已换、有实例
  没回来。写输出失败不改变退出码。契约样本钉在 `tests/force_contract.rs`
- **台账**（`aproxy-ipc/src/ledger.rs`，契约 C3）：实例、看护者与安装者在占端口或拉子进程之前原子
  写入 `<home>/ledger/<pid>.json`（pid、启动标记、角色、版本，可选端口、说明与实例配置
  文件的绝对路径 `config`），正常退出删除（Windows 上写入与删除短暂重试），
  `process::exit` 之前一律先删自己的条目（二进制一侧的 `util::exit`）。启动标记按平台冻结（Windows 创建时间
  FILETIME、Linux boot_id + starttime、macOS proc_bsdinfo 的启动秒与微秒）；读者只处理
  本平台前缀的条目，结束前在同一个进程句柄（Windows）或 pidfd（Linux）上核验启动标记
  与属主（只结束当前用户的进程），pid 复用不会误杀；删条目前重读，`start` 仍一致才删。台账与实例
  注册表分开（G3）：注册表可以随版本演进，台账只回答「这个 home 有哪些 aproxy 进程、
  怎样确认 pid 仍是它」。端口为 0 的实例在 bind 拿到实际端口后原子改写自己的台账条目（原来
  登记的 0 对不上任何端口，强制安装的「实例有没有回来」核对会失效）。

跨平台分工：状态机/IPC/恢复全部平台无关，平台分支只收口在交换原语、
宣告节介质、可执行位，以及台账的启动标记与核验后结束（`ledger::imp`）。

## 桌面界面（aproxy-desktop）

`aproxy desktop` 打开的窗口：Tauri 2 外壳加嵌入的前端（`aproxy-desktop/ui/`，Vite + Svelte 5 +
TypeScript）。窗口显示的是这个 home 下真实的实例、请求历史与配置：数据由 aproxy 本体读取
（`src/desktop/`），经 Tauri 命令交给页面。在浏览器里打开开发服务器时没有 Tauri，页面换用演示
后端，顶部常驻「演示数据」标签，免得把假的实例当成这台电脑上正在跑的。

**两种变体。** 根包的 `desktop` 特性（默认开）就是对 aproxy-desktop 的可选依赖；
`--no-default-features` 出无界面的 headless 版本，依赖树里没有任何界面 crate（CI 用
`cargo tree` 断言）。`--version` 的变体段按特性输出（C5）。headless 版本也认得 `desktop`
子命令（帮助里隐藏），执行时只说明怎样换装，不让用户看到 clap 的「unrecognized subcommand」。

**不依赖 Common-Controls 清单（RT-05）。** Tauri 默认静态导入只有 Common-Controls v6 才有的
`TaskDialogIndirect`，没有 v6 清单的 exe 在 main 之前就退出，连 `aproxy --version` 都跑不了。
清单在 tauri-build 编出的 Windows 资源里：aproxy-desktop 只有 lib，资源经 `rustc-link-lib`
跟着它进入链接它的 exe，所以 aproxy.exe 现在带着清单；可它一旦有了 bin，资源就只给那个 bin，
aproxy.exe 随之悄悄失去清单。所以不靠清单：tauri 关掉默认特性 `common-controls-v6`，exe 不再
导入 `TaskDialogIndirect`，没有清单也能启动；aproxy 的 lib 永不引用 aproxy_desktop，bin 里
引用它的代码只在 `#[cfg(all(feature = "desktop", not(test)))]` 下编译，rustc 不链接没被引用的
依赖，所以单元测试与 `tests/` 的集成测试都不含 Tauri；aproxy-desktop 自己 `[lib] test = false`。
细节在 `aproxy-desktop/Cargo.toml`。

**进程与线程。** `main` 先兑现 C5 的 `--version`，解析参数后在建 tokio 运行时之前分派
`desktop`：Tauri 的事件循环要占住主线程（macOS 硬性要求），而 `block_on` 里的主线程处在
异步上下文，Tauri 回调里再 `block_on` 会 panic。`desktop` 自己建多线程运行时，`enter` 之后经
`tauri::async_runtime::set` 交给 Tauri，再进事件循环；其余命令在手建的运行时里 `block_on`，
与原先的 `#[tokio::main]` 等价。

**启动与退出**（`src/commands/desktop.rs`）：解析 home（C1）→ 同一个 home 已有存活的界面
（台账角色 `desktop`）就提示并以 0 退出，读到的死条目顺手删掉 → 预检（Windows 问 WebView2
的版本，没装或 `WEBVIEW2_BROWSER_EXECUTABLE_FOLDER` 指错时报错；Linux 先看
`DISPLAY`/`WAYLAND_DISPLAY`）→ 登记台账（C3）→ 建窗口、进事件循环 → 窗口关掉后删台账条目，
按事件循环的退出码退出；登记之后的失败也经 `util::exit` 删掉条目再退出。
Tauri 在许多运行期失败上直接 panic，所以预检放在前面，setup 里的失败记下来、请事件循环以
非零退出码结束，图形后端初始化时的 panic 被接住换成说明。webview 的数据目录是
`<home>/desktop/webview`：不显式指定时 Tauri 用按 identifier 建的应用数据目录，跑到
`APROXY_HOME` 之外，测试与多份部署就隔离不了。

**Windows 的控制台。** aproxy.exe 是控制台程序。从终端启动时控制台与 shell 共用：检查都在
终端里做完，再拉起一个与终端分离的子进程承载窗口（`CREATE_NO_WINDOW`：它有一个自己的、
看不见的控制台，随即按下面快捷方式的情形释放），本进程立即退出，终端随即可用。从快捷方式
或资源管理器启动时控制台是系统专为它新建的：立刻 `FreeConsole`，控制台窗口只闪一下，再把
三个标准句柄置空（释放后进程记着的旧句柄值可能被之后打开的句柄复用，置空后 std 的写入一律
静默成功），之后的说明改用对话框；没有控制台（父进程以 `DETACHED_PROCESS` 拉起）时说明也用
对话框。几种情形由 `GetConsoleProcessList` 区分。改用对话框时还装一个 panic 钩子：主线程上的
panic（事件循环里的回调出错）照常交给默认钩子后再弹框说明。

**窗口。** 主窗口由 Rust 建（`tauri.conf.json` 的 `app.windows` 为空），以 `visible: false`
创建，页面脚本跑起来后自己 `show()`，启动时不先闪白底；3 秒内没显示由 Rust 兜底显示。
Windows 上装 tauri-custom-titlebar 的自定义标题栏：原生的最小化/最大化/关闭按钮与 Snap
Layouts 留在右上角，页面画标题栏的其余部分；`src/titlebar.rs` 是那个 skill 的原样文件，
装不上时退回原生标题栏，不让界面因此打不开。Mica 默认关，设置页的开关经 Tauri 的窗口接口
切换；支持与否由 Rust 判定（build ≥ 22523，且 window-vibrancy 在隐藏的窗口上试用成功），
系统的「透明效果」关着时设置页标明 Mica 会显示为纯色。

**数据怎样到页面。** 页面与 Rust 之间的约定在 `aproxy-ipc/src/desktop/`（DTO、错误码、事件帧），
与窗口一同发版，不是给第三方的稳定协议；页面一侧对应的类型在 `ui/src/lib/backend/contract.ts`。
aproxy-desktop 只做传输：`bridge.rs` 的命令自己解码请求体，把结果交回页面；读写的逻辑在
aproxy 的 `src/desktop/`，经 `Options` 注入。实例的读取与操作走实例 IPC，与命令行用同一条控制
通道；有副作用的操作带着页面当时看到的运行标识，端口换了主人就拒绝，不会落到别的实例上。
页面接上一个事件通道（`attach`），逐帧确认；缺帧或积压时 Rust 先发 `resync`，页面从快照重来。
错误只给码与固定的细节，句子都在页面的字典里；密钥在 Rust 一侧打码之后才离开。

**主题与扩展。** 主题包（`theme.json` 的颜色，可选的 `theme.css`）与扩展包（一个 ES module）装在
`<home>/desktop/extensions/`。安装只接受本地的 ZIP 或解开的目录（文件选择或拖进窗口）：Rust 先
完整读一遍，校验清单、路径与各项上限，再由页面把适用的风险逐条讲清（闭源、与 GitHub release
的摘要对不上、仓库星数少、核对不了），每条单独确认之后才落盘。不做沙箱：启用的扩展在主页面里
运行，能做页面能做的一切，风险由安装前的提醒负责（用户 2026-11-23 定）。包里的文件经自定义协议
`aproxy-pkg` 提供，地址的第一段带这次激活的代号（`<id>~<代号>`），升级之后相对引用的模块、
图片与字体不会沿用页面缓存里的旧内容。窗口偏好、启用列表与扩展的设置在
`<home>/desktop/desktop.toml`，扩展的数据在 `<home>/desktop/extension-data/<id>/`。

**联网与权限。** 窗口自己只在两处联网：检查与下载更新，以及安装包时可选地匿名查询 GitHub
（仓库信息与 release 的摘要）；扩展联网是扩展的事，安装前已经提醒。CSP 放行嵌入的资源、IPC、
`aproxy-pkg` 与 HTTPS（扩展可以加载远程的模块与资源），`object-src`、`base-uri`、`form-action`
一律禁止；Tauri 对 CSP 的补写只关掉 style-src 这一项（加了 nonce，浏览器就会忽略
`'unsafe-inline'`，动态样式随之失效）。窗口只许停在嵌入的页面上，按平台精确比对源（Windows
只认 `http://tauri.localhost`，其余平台只认 `tauri://localhost`，带端口的都不认：Linux 与 macOS
上 `http(s)://tauri.localhost` 是真实的回环地址），新窗口与下载一律拒绝；CSP 不管顶层导航，
这一条就是闸门。权限（`capabilities/main.json`）只授权页面用到的窗口接口、事件监听与本包的
命令，不含 `core:default`；命令表在 `command_list.rs`，构建脚本核对它与权限文件两个方向一致。
各项取舍写在 `aproxy-desktop/src/lib.rs` 的开头（`tauri.conf.json` 写不了注释）。

**前端产物。** `generate_context!` 在编译期把 `aproxy-desktop/dist/` 嵌进二进制。dist 不进
git，缺失时 build.rs 报错并给出构建步骤，不拿占位页面凑数——凑出来的二进制看起来正常、
界面却是空的，发布时也认不出来。CI 带界面的任务先 `npm ci && npm run build`。crates.io 上的
aproxy-desktop 带着构建好的 dist、不带 `ui/` 的源码，`cargo install aproxy` 不需要 Node.js。
tauri-build 每次构建都在包根写 `gen/schemas/`，包里没有这些文件时 `cargo package` 的校验
构建报「Source directory was modified by build.rs」；所以包里也带着它们，发布前先在同一平台上
构建一次本包，校验构建写出的内容逐字节相同，就不算改动（2026-10-12 在 Windows 上实测，做法
见 `aproxy-desktop/Cargo.toml` 的 include 说明）。

**接缝。** 界面组件只经 `Link` 接口（`ui/src/lib/backend/link.ts`）发命令、收事件：窗口里是
Tauri 实现（`tauri.ts`），浏览器里是演示实现（`demo/`），两者的命令表、事件与错误完全相同。
aproxy-desktop 需要的 home 与运行时由 bin 经 `Options` 交进去。浏览器开发模式（`npm run dev`）里另有原型件
（`ui/src/proto/`：审核工具条、模拟的窗口按钮与 Mica、吉祥物预览页），只在开发服务器里动态
加载，生产构建不含它们；它们的文案与吉祥物预览页的 `#/mascots` 路由也只在开发构建里。

## 测试

- 单元测试（lib + bin）：重试判定、配置分层、IPC 协议、注册表/期望状态记录、打码、看护者记录与选举、退避状态机、安装状态机/下载链条/zip 安全
- 集成测试：mock 上游 + 真实代理联调、守护生命周期、logs/restore 端到端、
  双模缓冲（磁盘 spool 字节保真/EOF 清理/流尾错误重试）、看门狗（强杀重拉/
  优雅停不复活/并发看护者唯一性）、install 流程（恢复矩阵崩溃注入 + CLI
  级完整链路）、交换原语（双 rename 舞 + fallback 脚本）、备料全链
- **CI**（`.github/workflows/ci.yml`）：windows job 跑 fmt + clippy + test，ubuntu
  job 跑 clippy + test（windows 的 clippy 不编译 `#[cfg(unix)]` 代码，零警告纪律在
  ubuntu 补上 unix 面）；clippy 与 test 一律带 `--workspace`（根包是 aproxy 且
  `[workspace]` 未设 default-members，不带时 aproxy-envelope 与 aproxy-format 的测试
  从不进入门禁），test 带 `--no-fail-fast` 一次拿到完整失败面。windows 与 ubuntu 都按
  带界面（desktop，先用 Node 构建前端，ubuntu 另装 `libwebkit2gtk-4.1-dev`）与 headless
  （`--no-default-features --exclude aproxy-desktop`）两种变体各跑一遍，每种都对构建出的
  aproxy 跑 `--version` 冒烟（`.github/scripts/version-smoke.sh`，按 C5 核对版本、变体与
  目标三元组）；headless 另断言依赖树里没有任何界面 crate。`upgrade-from-0-1-0` 任务下载
  真实的 v0.1.0 二进制（windows 与 ubuntu），断言它的 `install --from` 装当前构建时在改动任何
  东西之前失败（`tests/upgrade_from_0_1_0.rs`，平时 `#[ignore]`，只在 CI 跑）。`force-cross-version`
  任务下载已经发布的每个 0.2.0 版本（预发布也算；清单在 ci.yml 的 `PREV_VERSIONS`，每发一个
  版本加一个）的无界面资产，让它与当前构建互相强制安装——谁执行、装上谁、被结束的进程是谁
  启动的，四种组合都走——断言进程按台账结束、二进制换好、端口放开
  （`tests/force_cross_version.rs`，同样平时 `#[ignore]`）。macOS 与 windows、ubuntu 同为
  门槛：两种变体都跑 clippy、全量测试与 `--version` 冒烟
- **发布门禁**（`release.yml`）：tag 触发后先构建一次前端，再在 windows、ubuntu 与 macOS 上按
  两种变体复跑 fmt/clippy/test/`--version` 冒烟（tag 可能落后于 main），全绿才进入构建。
  构建出 22 个二进制（每个平台 desktop 与 headless 各一份，musl 只有
  headless；Linux gnu 的带界面版在 `ubuntu:22.04` 容器里构建），每个都核对嵌着的 C5 版本行、
  能跑的再真的执行 `--version`；之后由 `examples/release-manifest.rs` 生成发布清单
  `aproxy-manifest.json` 并用强制安装读清单的代码读回核对。渠道发布幂等：npm 与 crates.io
  两个 job 都先查询注册表、版本已存在才跳过，所以任一渠道失败后可单独 re-run，已成功的不会
  重复发布；GitHub Release 的 Latest 标记交给 API 默认值（预发布永不成为 Latest）。手动触发
  （workflow_dispatch）是演练：同样的门槛、构建与清单核对，npm 与 crates.io 只跑
  `--dry-run`，不建 Release、不打 tag
- 测试端口 bind 试探选取（排除区间/占用者自动跳过），绝不触碰用户实例；
  `DaemonGuard` 保证断言失败路径也清理守护

## 平台

| 平台 | 状态 |
|---|---|
| Windows（x64 / x86 / arm64） | 全功能（开发与主测试平台） |
| Linux（x86_64 / aarch64，glibc 与 musl） | 全功能；headless 的 gnu 产物 glibc 下限 2.28，带界面的 gnu 产物要 glibc 2.34 与 WebKitGTK 2.40；更低的 glibc 与 musl 系统用 musl 产物（只有 headless） |
| macOS（Apple Silicon / Intel） | 全功能；CI 在 Apple Silicon 上跑全部测试，看护者、安装与桌面窗口在 GitHub 的 macOS runner 上实测过；Intel 版交叉编译，没有在 Intel 上运行过 |

unix 分支（UDS IPC、run 目录里的心跳文件、单步 rename 交换）在 Linux 与 macOS 上共用；进程查询
分平台实现：Linux 读 `/proc`，macOS 用 `proc_pidinfo`（创建时间取 `proc_bsdinfo` 的启动秒与微秒、
僵尸按进程状态判定、镜像路径用 `proc_pidpath`），所以一切靠「pid + 创建时间」核验身份的环节
（看护者的收养与处决、`stop --force`、install 宣告）在两个平台上同样工作，见 `src/process.rs` 与
`aproxy-ipc/src/registry.rs`。

桌面界面另有平台前提：Windows 要 WebView2 运行时（Windows 11 自带）；Linux 的带界面构建
动态链接 WebKitGTK 4.1，缺库时整个二进制都起不来，所以 musl 只有 headless，没有图形界面的
机器也该用 headless；macOS 用系统自带的 WebKit，不需要另装。`aproxy install` 与 `install --force`
不预先猜本机缺什么，只看下载下来的二进制自己的 `--version` 能不能跑：系统要求属于目标版本的
构建，会随以后的发布变化（换构建基线、换 WebKitGTK 的 API 与 soname），而安装器的代码随正在
运行的旧版本永远不变——强制安装尤其要能装以后任何一版。没写 `--variant` 时按
`DownloadCtx::attempts` / `force::fallbacks` 回退：带界面的版本跑不起来就换同一目标的
headless（写了 `--cpu` 就守同一档），linux-gnu 再换 musl 的 headless 基线；强制安装在发布清单里
这个目标根本没有本变体时也这样回退。写了 `--variant desktop` 就不换，失败时只有被动态加载器挡在
main 之前（`staging::is_loader_failure`）才提示装库或改装 headless，提示里不写具体的库与版本。只有一键安装
脚本（每次从 main 现取，跟得上最新的构建）在首装、没指定变体时先看本机：musl、glibc 低于下限、
动态加载器缓存里找不到 `libwebkit2gtk-4.1.so.0` 的装 headless。

### Linux gnu 产物的 glibc 下限

在 ubuntu-24.04 runner 上直接构建的 gnu 产物会链接 glibc 2.39，Debian 12、
Ubuntu 22.04 等系统启动即报 `GLIBC_2.39 not found`（alpha.17 实测如此）。发布流程
因此用 cargo-zigbuild 按 `GNU_GLIBC_FLOOR`（2.28，`release.yml` 顶部 env）链接，
并在构建后用 `objdump -p` 断言产物引用的 `GLIBC_` 符号版本都不高于它（非数字的
需求如 `GLIBC_ABI_DT_RELR` 同样判失败）。这个数字必须与 `scripts/install.sh` 和
`npm/aproxy/bin/aproxy.js` 里的 `MIN_GLIBC` 保持一致：它们在 glibc 低于下限的
系统上改选静态链接的 musl 产物（脚本落位前还用 `--version` 自证，gnu 跑不起来
同样改拉 musl）。musl 产物静态链接，任何 Linux 上都能跑。

带界面的 gnu 产物链接系统的 GTK 与 WebKitGTK，不能用 zigbuild 压低 glibc：它在
`ubuntu:22.04` 容器里构建（跑在 ubuntu-24.04 与 24.04-arm 上），下限 `DESKTOP_GLIBC_FLOOR`
（2.34）同样在构建后断言。这个数与 `scripts/install.sh`、`npm/aproxy/bin/aproxy.js` 判断
「本机能不能跑带界面版」用的数必须一致。

### npm dist-tag

预发布（版本含 `-alpha`/`-beta`/`-rc`）发在 `next`，正式版发 `latest`
（`npm/build-and-publish.sh` 的 `npm_dist_tag()` 与 `release.yml` 里 GitHub Release
的 `prerelease` 判定是同一条规则，改一处必须同步另一处）。首个正式版发布后
`npm i -g @meowo/aproxy` 只会装到正式版，预发布需显式 `@meowo/aproxy@next`。
带界面与无界面各一组包（`@meowo/aproxy` 与 `@meowo/aproxy-headless`，各一个主包加 9 个
平台包），dist-tag 规则相同。

## 版本路线

- **0.1.x**：0.1.0 是首个稳定版（此前为 `0.1.0-alpha.*` 预发布）；无 UI，纯 CLI + 守护。
- **0.2.0**：当前开发线（`0.2.0-alpha.*` 预发布）。与 0.1.x 互不兼容、不能原地升级
  （迁移步骤见 CHANGELOG），可安装版本的下限是 0.2.0；引入桌面界面与主题、扩展（早期的 alpha
  版只显示演示数据，现在接的是真实数据，见「桌面界面」）。
- 发布产物：x86_64 的 Windows、Linux gnu 与 macOS 同时提供 baseline 与 x86-64-v3
  （AVX2）两种指令集变体。`aproxy install` 运行时检测 AVX2，只在这三个目标上选 v3
  （它按名字找资产，别的目标取 `-v3` 只会 404）；强制安装按发布清单选，清单里没有 v3
  档就用 baseline；一键安装脚本保守地装 baseline；musl 与其他架构只有 baseline。
