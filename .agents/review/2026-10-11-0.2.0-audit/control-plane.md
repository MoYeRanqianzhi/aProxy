# 0.2.0 审查：控制面（守护进程 / IPC / 注册表 / 看门狗 / 实例管理命令）

- 日期：2026-10-11；基线 master `c1d4b96`（工作区干净）
- 范围：`src/daemon.rs`、`src/watchdog.rs`、`src/server.rs`、`src/main.rs`、`src/util.rs`、`src/cli.rs`、
  `src/settings.rs`（注册表/别名/写回相关）、`src/find.rs`（输出部分）、`src/commands/{start,stop,restart,status,logs,restore,alias,find,mod}.rs`，
  以及 `tests/` 中的守护/重启/看门狗类测试。为核对接口，顺带读了 `src/install/{announce,broadcast,flow,restart}.rs`
  的调用点与 `src/compat_0_1_0.rs`。
- 方法：只读。读代码、grep、对源码做一次控制字节扫描和一次 CJK 字面量统计（脚本放在仓库外的临时目录）。
  **没有编译、没有跑测试、没有启动任何 aproxy 进程、没有读写 `~/.aproxy`。**
- 证据标注：**事实** = 读到的代码或文件，附 `file:line`；**推断** = 由代码与平台语义推出、未实测，逐条标明。
- 前提（记忆 `0.2.0-direction`、计划 `.agents/plan/release-0.2.0.md`）：去掉全部兼容；0.2.0 之后对外接口长期稳定；
  UI 为 Tauri 2 编进本体（`aproxy desktop`），GUI 只经 IPC 与实例通信、按显式 home 寻址、不开新网络端口；
  给用户看的文本做 i18n，给 agent 或不可见的固定英文。`compat_0_1_0.rs` 正在删除，本报告不评它，只记一处
  已存在的缺陷（CP-28）。

---

## 0 总体判断

10-06 的 IPC v1 整理把上一轮最危险的几类问题收掉了：进程身份已与二进制名称解耦（pid + 创建时间，
Windows 同句柄核验后终止）；控制端点按 home 命名空间、先于注册表独占创建，建不成就拒绝启动；客户端核对
应答者 pid 与连接对端；挂死实例单列为「无响应」不再被删记录；restart 先预检后停止；`stop --force` 先摘恢复
记录再终止；看护者死亡事件即时重拉、换端口后退役旧记录。这些都有集成测试钉住。

但现在的控制面是按「让 0.1.0 能读懂新版本」设计的，还不是按「0.2.0 是今后所有版本里最老的那个读者」
设计的，直接冻结会把几处缺陷变成永久契约：

1. **持久格式的读者规则会锁死未来格式**：读不懂的 `.pid`、`.restore` 直接删，settings.json 写回丢掉不认识
   的字段并把默认值写死，`.restore` 存的是原始命令行——0.2.0 二进制发出去之后，这些行为决定了 0.3 能写什么（CP-01、CP-02）。
2. **IPC v1 尚未随任何正式版发布**（`git tag --contains 4018555` 为空），但它把未知参数静默忽略：将来带条件的
   `shutdown` 发给 0.2.0 实例会变成无条件停止（CP-03）。线上文本是中文。
3. **读路径有副作用**：status 顺带删日志（会删掉正在启动的实例的日志）、顺带 spawn `install --continue`；UI 一轮询
   就把偶发竞态变成常态（CP-06）。
4. **命令层直接打印并 `process::exit`**：没法同时服务 i18n 文本、`--json` 和编进本体的 UI（CP-08）。i18n 一上，
   agent 依赖的中文原文匹配就失效，`--json` 与退出码表必须同一版到位（CP-04）。
5. 上一轮看护者的鲁棒性条目几乎都还没动（就绪后再崩溃无退避、重拉不识别早退、放弃即永久、日志全部丢弃），
   unix 守护也没有脱离启动者的进程组——推断在启动终端按 Ctrl+C 会把守护和看护者一起优雅停掉（CP-11、CP-12）。

结论：控制面的「身份、命名空间、挂死处置」三根柱子已经可以冻结；「持久格式与读者规则、IPC 参数规则、
读写分离、操作层、退出码」这五件事必须在 0.2.0 定稿前改，之后改就是破坏性变更。

---

## 1 发现清单

| ID | 级别 | 类别 | 标题 | 成本 |
|---|---|---|---|---|
| CP-01 | P0 | 契约/持久格式 | 持久记录的读者会删掉看不懂的记录，settings 写回丢新字段：0.2.0 将锁死今后所有格式 | M |
| CP-02 | P0 | 契约/持久格式 | `.restore` 存原始命令行：把 CLI 语法与明文密钥冻结进期望状态 | M |
| CP-03 | P0 | 契约/IPC | IPC v1 静默忽略未知参数；线上文本中文；result 定型为状态快照 | S |
| CP-04 | P0 | 契约/CLI | 退出码不一致且无 `--json`：i18n 之后 agent 没有稳定接口 | M |
| CP-05 | P0 | 契约/布局 | `APROXY_RUN_DIR` 让命名空间与数据目录分家；GUI 无显式 home 入口 | S |
| CP-06 | P1 | 正确性/读写分离 | status 会删掉正在启动的实例的日志；每条命令都可能 spawn `install --continue` | M |
| CP-07 | P1 | 正确性/UX | 「协议不兼容/冒名」被当成「挂死」，并建议 `--force` | S |
| CP-08 | P1 | 结构 | 命令层直接打印并退出，无法复用为 `--json`、i18n、UI 的后端 | L |
| CP-09 | P1 | 安全 | IPC 访问控制依赖平台默认值；结案理由已过时 | S–M |
| CP-10 | P1 | UI 前置 | 缺在途请求、模式、看护者状态与可本地化的 last_error | M |
| CP-11 | P1 | 正确性/看门狗 | 就绪后再崩溃无退避且每轮新建日志文件；重拉不识别早退；补种删 claim；放弃即永久；日志全丢 | M |
| CP-12 | P1 | 正确性/unix | 守护未脱离启动者的进程组：Ctrl+C 会停掉守护与看护者（推断） | S |
| CP-13 | P1 | 契约/语义 | 守护的工作目录与环境取决于「谁拉起的」 | S/M |
| CP-14 | P1 | 安全 | 看护者命令行复制了启动者的全部参数（含 `--api-key`） | S |
| CP-15 | P1 | 平台/macOS | unix 心跳与安装宣告放在 `/dev/shm` | S |
| CP-16 | P1 | 布局/命名 | 磁盘布局与命名不一致；spool 按配置端口分目录且在 bind 前清空 | M |
| CP-17 | P1 | 结构 | daemon.rs / watchdog.rs 职责混杂、平台分支分散、关键逻辑多处重复 | L（可分步） |
| CP-18 | P1 | 语义 | 看门狗开关与调优只在进程启动时读取，UI 设置无法即时生效 | S |
| CP-19 | P1 | UI 前置 | UI 编进本体后的控制面影响（spawn 目标、版本错位、守护内存） | S（决策）+ 实测 |
| CP-20 | P2 | UX | `start` 的就绪等待落后于 restart/restore；端口 0 不等待即报成功 | S |
| CP-21 | P2 | 正确性 | 优雅停止到最后才删 `.restore`；`stop all` 不处理待重拉实例 | S |
| CP-22 | P2 | 正确性/Linux | Linux 身份锚点跨重启不唯一（推断） | S |
| CP-23 | P2 | 健壮性/Windows | 守护会随启动者所在的作业对象一起被杀（推断） | S |
| CP-24 | P2 | 自愈 | `.pid` 丢失后守护不补写 | S |
| CP-25 | P2 | 协议 | `prepare_swap` 置位后没有出口 | S |
| CP-26 | P2 | 数据安全 | 放在 logs/ 下的自定义 `log_file` 停止后被孤儿清理删除；启动时被截断 | S |
| CP-27 | P2 | CLI | 参数细节：阈值被静默忽略、global 参数泄漏、多行帮助被折行 | S |
| CP-28 | P2 | 缺陷（即将删除的代码） | `compat_0_1_0.rs:29` 的 Windows 旧宣告名里是 BEL 字节，S5 从未生效 | S |
| CP-29 | P2 | 记录卫生 | daemon-model 记忆与 TODO 的结案理由已过时 | S |

### CP-01（P0）持久记录的读者规则：0.2.0 将是今后所有版本面对的最老读者

**证据（事实）**
- 解析不了的 `.pid` 被普查直接删除：`src/daemon.rs:862-869`。
- 解析不了的 `.restore` 被列举直接删除：`src/daemon.rs:1122-1136`。status/stop/logs 的普查经
  `cleanup_orphan_logs_in(…, &list_restore_entries())` 调到它（`daemon.rs:782-793`），看护者每个 tick 的
  `adopt_scan` 也调它（`src/watchdog.rs:466`）。
- 注册记录全部字段必填，理由是「0.1.0 会读」（`daemon.rs:140-146`）。0.2.0 起这个理由不存在。
- settings.json：`Settings` 没有保留未知字段的容器（`src/settings.rs:42-151`），`save_to` 序列化整个结构
  （`settings.rs:306-320`），默认值一并写进文件。
- claim 读不懂 → `claim_is_in_effect_in` 判为不在任（`watchdog.rs:146-153`）→ 补种方先 `remove_claim_in`
  再拉起（`src/server.rs:221-222`、`src/commands/start.rs:341`）。
- 没有任何持久记录带格式版本：`InstanceRecord`（`daemon.rs:147-171`）、`RestoreRecord`（`daemon.rs:1096-1100`）、
  `WatchdogClaim`（`watchdog.rs:29-44`）。

**为什么重要**：去掉 0.1.0 兼容之后，0.2.0 会读到更新版本写下的文件——npm/cargo 渠道里落后的 CLI、install
换版后仍在运行的旧 GUI 进程（UI 编进本体）、升级窗口里的旧看护者。按现行逻辑，旧读者看不懂就删：`.pid` 被删，
实例从 status、看护者收养、install 滚动重启的视野里消失（守护不会补写，CP-24）；`.restore` 被删，崩溃恢复意图
丢失；旧 CLI 的 `alias add` 写回 settings.json，新字段丢失、默认值被固化。0.2.0 发出去之后这些行为改不了。

**建议**
1. 每种持久记录加 `schema`（整数，从 1 起）。只增可选字段；改字段语义或删字段才升 schema。
2. 读者规则：未知字段忽略；schema 高于自己认识的、或解析失败 → 跳过并在 status/doctor 报告「无法识别的记录」，
   **不删除**；只有「本 schema、且写者经 pid + 创建时间核验已死」的记录才清理。
3. settings.json：`#[serde(flatten)] extra: serde_json::Map<String, Value>` 保留未知字段；数值/布尔字段改 `Option`，
   只写用户显式设置的值（默认值留在代码里，以后才能改默认）。与 config-cli-install 审查的 config.toml 写回问题
   （上一轮 ui-02，`src/config.rs:939-945` 仍是整体重序列化 + 非原子写）同一原则。
4. claim 读不懂时按文件 mtime 判新鲜：新鲜视为「有在任者、格式更新」，不补种；过期才清。
5. 新增 `tests/contract_v0_2.rs`：冻结 0.2.0 的全部持久与线上形状（golden JSON、双向解析），取代将要删除的
   `compat_v0_1_0.rs`，作为之后版本「新读旧」的基线。

**成本** M

### CP-02（P0）`.restore` 存原始命令行

**证据（事实）**
- 恢复记录 = 守护自身命令行去掉 `--daemon-child`/`--foreground`：`src/server.rs:152-159`；格式 `{args, log_path}`：
  `daemon.rs:1094-1100`。
- 重放：看护者 `watchdog.rs:952-955`、`restore` `src/commands/restore.rs:39-41`、`restart`
  `src/commands/restart.rs:106-110, 123-126`。restart 的预检用当前版本的 clap 解析旧 argv（`restart.rs:169-185`，
  失败报「原启动参数无法被当前版本解析」）。
- 记忆 `ipc-v1-upgrade-contract` 把「`.restore` 里可能出现的每个启动参数」列为永久接口。
- `--api-key` 帮助写「仅本次运行生效」（`src/cli.rs:43-46`），实际明文进 `.restore`（`daemon.rs:1028-1029` 注释自述），
  并出现在守护与看护者的命令行（CP-14）。

**为什么重要**：只要 argv 是持久的期望状态，所有能出现在启动命令行上的参数（`--config/--listen/--baseurl/--proxy/
--api-key/--log-file`，以及 `start <别名>` 这种位置参数形态）就永远不能改名、改语义；重放由「读记录的那个版本」的
clap 解析，语义漂移会静默改变被恢复实例的行为。这等于把 CLI 语法当成了存储格式。

**建议**：改为结构化期望状态，例如

```json
{"schema":1,
 "config_path":"C:/Users/u/.aproxy/work.toml",
 "overrides":{"listen":"127.0.0.1:12346","base_url":null,"proxy":null,"log_file":null},
 "log_path":"C:/Users/u/.aproxy/logs/19ac3f2e8b5d-1a2b.log"}
```

spawner（无论哪个版本）把记录翻译成**自己**的命令行再 spawn，CLI 语法不再跨版本。`--api-key` 覆盖的去留需用户定：
(a) 不进恢复记录——恢复/重拉后回到配置文件里的 `api_key`，status 标注「启动时的 key 覆盖未保留」；(b) 明文写入并把
帮助改成如实描述。倾向 (a)：命令行与持久文件都不该成为密钥的长期存放处。看护者、restore、restart、install 四处重放
共用一个 `spawn_from_record`（见 CP-17）。

**成本** M

### CP-03（P0）IPC v1：未知参数被静默忽略；线上文本与 result 定型

**证据（事实）**
- 只检查 `args` 是不是对象，内容不看：`daemon.rs:1283-1292`；三个 op 都不读参数：`daemon.rs:1209-1249`。
- 版本规则「加 op、加可选参数、加应答字段……都不递增」：`daemon.rs:221-224`。
- 失败应答的 `message` 是中文：`daemon.rs:1217, 1256, 1259, 1274, 1281, 1284, 1290`。
- `WireResponse.result` 的类型就是 `InstanceStatus`：`daemon.rs:262-270`。
- v1 在 `4018555`（2026-10-06）引入，`git tag --contains 4018555` 为空：**v1 没有随任何正式版发布**，0.1.0 讲的是
  无 `v` 的旧格式（`src/compat_0_1_0.rs:12-16`）。

**为什么重要**：最自然的扩展都要参数——`shutdown{if_idle_secs}`（把 `stop idle` 的「判定闲置 → 发停止」变成实例内
原子操作，消掉上一轮 daemon-ipc-05 的竞态）、`shutdown{drain_secs}`、`prepare_swap{ttl_secs}`（CP-25）、`logs{follow}`。
新客户端把它们发给 0.2.0 实例时会被当作无参 op 执行：条件停止变成无条件停止。客户端可以先查 `ops`，但 `ops` 只到
op 粒度。0.2.0 发布后这个行为收不回。

**建议**（v1 未发布，改它不需要升版本号）
- 未知 args 键 → 新错误码 `unsupported_arg`（`details.args` 列出键名），绝不忽略；类型不对 → `bad_request`。
- result 按 op 定型：ping/shutdown/prepare_swap 继续返回状态快照，新 op 自定；客户端按 op 解码。
- `message` 改英文、只给日志；新增 `details` 对象承载参数，显示端按 `code` 本地化。
- 写明：服务端用请求的 `v` 应答；0.2.0 起发布过的版本号一律保持支持（目前只有 1）。
- 预留 home 级端点名（第 3 节）。

**成本** S

### CP-04（P0）退出码与 `--json`

**证据（事实）**
- status 没有任何非零退出（`src/commands/status.rs` 全文）；restore 有实例起不来仍退 0（`restore.rs:10-86`）。
- stop：端口未运行 exit 1（`stop.rs:111-113`）、别名未运行 exit 1（`stop.rs:80-81`），无实例/`all`/`idle` 退 0
  （`stop.rs:49, 133, 139`）；logs 同样两样（`logs.rs:49-50` 对 `logs.rs:58-59`）。
- 这些不一致已被 skill 写成对外文档：`.claude/skills/aproxy-cli/references/latest/commands.md:90-101`（含 doctor 恒 0）。
- 全仓没有 `--json`（`cli.rs`）。skill 的解析方式是逐字引用中文输出（`commands.md:82`）。

**为什么重要**：W3 之后文本输出随语言环境变化，skill 里「按中文原文匹配」在英文系统上失效——agent 是一等用户，
却失去了唯一的接口。退出码在 0.2.0 公布即冻结，现在的表把「目标状态已达成」与「操作失败」混在一起。

**建议**（退出码取值需用户定）
- 退出码：0 成功或目标状态已达成；1 操作失败；2 用法错误（clap 已用 2；未知别名、多实例未指定 target 也归 2）；
  3 部分成功（`all`/`idle`/restore 等多目标里有失败）；4 目标未运行（restart、logs 这类需要运行实例的命令）。
  `stop` 遇到「本来就没在运行」是返回 0（幂等）还是 4，需用户定。restore 有失败非 0；doctor 有 error 非 0。
- `--json`：status、stop、restart、restore、start、alias list、find、doctor、`config --show`；install 用 NDJSON 输出进度。
  顶层 `{"schema":1,…}`，错误 `{"error":{"code","details"}}`，不随语言变化。样例见第 8 节。
- `APROXY_LANG=en` 作兜底；skill 改为指导 agent 用 `--json`。

**成本** M（依赖 CP-08 的操作层，否则每个命令都要写两套输出）

### CP-05（P0）两个环境变量定义 home

**证据（事实）**
- run 目录先读 `APROXY_RUN_DIR`（`daemon.rs:27-32`），logs/spool 永远取 home（`daemon.rs:35-43`）。
- 端点命名空间按 run 目录求 `home_id`（`daemon.rs:88-97, 107`）。
- 普查用 HOME 的 logs 目录对照 RUN_DIR 的记录删日志（`daemon.rs:782-793, 803-835`）。
- skill 已对外文档化这个变量（`commands.md:30`、`behaviors.md:334`）；`tests/*.rs` 没有任何使用；没有 `--home`
  参数（`cli.rs:9-61`）。

**为什么重要**：两个变量能组合出「共享 logs、各自 run」的状态，普查会把另一个 run 目录里实例的日志当孤儿删掉
（上一轮 tests-12）；「一个 home = 一个命名空间」是 GUI 显式寻址的前提；随 0.2.0 再发一次，它就成了要长期维护的
公开开关。

**建议**：删 `APROXY_RUN_DIR`，run 目录恒为 `<home>/run`（`home_id` 算法与端点名都不变）；新增全局 `--home <DIR>`，
解析后设进进程环境（`spawn_detached` 会继承），并显式写进看护者/守护的命令行；库 API 一律以 home 为参数
（现有 `*_in(run_dir)` 改 `*_in(home)`）。`APROXY_WATCHDOG_SCAN_SECS`（`watchdog.rs:376-384`）改为测试专用或
在文档里声明为内部开关。

**成本** S

### CP-06（P1）读路径的副作用与竞态

**证据（事实）**
- 守护先创建日志文件（`src/main.rs:44-52`），之后才解析配置、bind TCP、退役旧看护者（最长约 5 秒，
  `watchdog.rs:82-87`）、创建控制端点，最后写 `.pid`（`server.rs:83-142`）。
- 普查顺带删除不被任何 `.pid`/`.restore` 引用的 `.log`（`daemon.rs:780-793, 803-835`）；status、stop、logs 都触发
  （`status.rs:13`、`stop.rs:57, 119`、`logs.rs:24, 53`）。代码注释写着「status 是唯一可靠的清理时机」（`daemon.rs:765`）。
- 每条非 install 命令只要 install.state 存在就 spawn `install --continue`（`main.rs:88-100`），status 也一样；
  install.state 在一次正常安装的全程都存在。

**推断**：在「建日志 → 写 `.pid`」窗口里执行的任何一次普查都会删掉新实例的日志。std 在 Windows 上默认带
`FILE_SHARE_DELETE` 打开文件，删除会成功，守护此后写进的是一个已删除的文件，`aproxy logs` 报「日志文件迟迟未生成」。
UI 以秒级轮询 status 时，这个窗口从偶发变成常态（开机 restore 批量拉起时尤甚）；安装期间每次轮询都多拉起一个
短命的续作进程。

**建议**
- 拆出只读枚举 `registry::enumerate(home)`：并发 ping、单次短超时、返回四态（CP-07），不删除、不清日志。UI 与
  `--json` 用它。
- 清理移到守护启动（清本 home 的死记录与孤儿日志）或看护者的周期 GC；日志清理加宽限：只删 mtime 早于若干分钟、
  文件名符合内置随机名格式（CP-26）、且不被任何记录引用的文件。
- install 续作兜底只由变更类命令触发，或先确认宣告已失效（`announce::is_active`）再 spawn。

**成本** M

### CP-07（P1）「协议不兼容/冒名」被当成「挂死」

**证据（事实）**：普查对 ping 的任何错误都转入身份核验，进程在即列为无响应（`daemon.rs:889-895`）；status 对无响应
实例建议 `stop --force`（`status.rs:104-116`）；stop 同（`stop.rs:100-109`）。而 `IpcError` 本来就区分
Unreachable/Transient/Protocol/Remote（`daemon.rs:296-306`）。

**为什么重要**：版本不匹配的实例（换版后旧 GUI/旧 CLI 面对新实例，或反之）和冒名应答者，都会被显示为「无响应，
可用 `--force` 结束」——引导用户强杀健康实例。UI 也需要区分这几种状态。

**建议**：四态——Live / Unresponsive（Transient）/ Incompatible（Protocol、Remote `unsupported_version`）/ Dead。
Incompatible 提示「用与实例同版本的 aproxy 操作」；`--force` 只对 Unresponsive 给出建议。

**成本** S

### CP-08（P1）命令层直接打印并退出

**证据（事实）**：目标解析内部打印并 `process::exit(1)`（`stop.rs:30-150`，7 处 exit；`logs.rs` 7 处）；restart、
restore、start 的结局全部是 println（`restart.rs:46-95`、`restore.rs:10-86`、`start.rs:276-321`）；库里也打印：
`src/find.rs:105-135`（`print_list`）、`settings.rs:244-248`（`load_from` 的 eprintln）。

**为什么重要**：W3 要求文案集中在渲染层；`--json` 要求结构化结果；W5 的 UI 编进本体，GUI 进程里调用
start/stop/restart/restore/alias 不能 exit 掉自己，也需要结构化结局。不先抽操作层，三件事各自重写一遍输出。

**建议**：`ops::{enumerate, start, stop, restart, restore, alias_*}` 返回 `Result<Outcome, OpError>`（OpError = code +
details，英文 Display 供日志），CLI 文本（i18n）、`--json`、UI 三个前端共用；lib 不打印。先迁控制面命令，再随 W3
迁其余。

**成本** L

### CP-09（P1）IPC 访问控制

**证据（事实）**
- 管道以默认安全描述符创建（`daemon.rs:1416-1419, 1438`）；tokio 1.53.1 默认 `reject_remote_clients: true`
  （`~/.cargo/registry/src/…/tokio-1.53.1/src/net/windows/named_pipe.rs:1773`），远程客户端已被拒。
- unix socket 与 run 目录不收紧权限（`daemon.rs:1504-1526`）；全仓 `set_permissions` 只出现在 install 的 0755
  （`src/install/staging.rs:105-106`、`src/install/download/crates.rs:164-165`）。
- 服务端不校验客户端（`handle_conn`，`daemon.rs:1187-1250`）；客户端核对服务端自报 pid 与对端进程一致
  （`daemon.rs:403-413, 1399-1406, 1486-1491`）。
- `.agents/TODO.md:474` 与 daemon-model 记忆仍写「命名管道 ACL（tokio 不暴露）」，已不成立。

**为什么重要**：命名管道默认 DACL 给 Everyone 读（写需要所有者），现有 op 不会因此泄露；但 umask 为 002 的系统上，
同组用户能连上 socket 并停实例。UI 会让 IPC 承载日志流、配置读取等敏感内容，那时不能再靠平台默认值。按同用户
信任模型，这些措施不改变能力边界，只是把边界写死。

**建议**
- Windows：`create_with_security_attributes_raw`，SDDL 用当前进程令牌的用户 SID 构造
  `D:P(A;;GA;;;<用户SID>)(A;;GA;;;SY)`，首个与后续实例同用。
- unix：run 目录 0700、socket bind 后 0600、服务端 `peer_cred().uid() == geteuid()`、客户端也核对服务端 uid
  （tokio 在 Linux 与 macOS 都提供 `peer_cred`，macOS 含 pid）。
- 更正 TODO 与记忆中的结案理由。

**成本** S–M（windows-sys 需加 `Win32_Security_Authorization` feature）

### CP-10（P1）UI 需要的观测数据缺失

**证据（事实）**：`Activity` 只有累计计数与最近一条错误（`daemon.rs:200-219`）；全仓没有在途请求计数（`in_flight`
只在 `src/proxy.rs:2121` 作为心跳标志出现）；`InstanceStatus` 没有模式字段（`daemon.rs:173-183`）；status 不显示
看护者（`status.rs` 全文）；`last_error` 是 `src/proxy.rs` 里约 20 处 `note_upstream_failure` 拼出的中文串
（如 `proxy.rs:1370, 1376, 1380, 1393, 3369, 3432`）。

**为什么重要**：使命可视化的核心是「哪个请求正卡在重试、为什么」；`stop idle` 的闲置判定忽略在途长流（上一轮
daemon-ipc-05）；仅转发实例在 status 里与普通实例同形，运维会误判。

**建议**（全部是状态快照的新增可选字段，样例见第 3.6 节）：`activity.in_flight`、`activity.retrying`（RAII guard 维护）；
`mode{forward_only, disk_cache, keepalive_secs, request_transform, response_transform, bounded_retry_paths}`；
`last_error{code, details, message(英文), at}`；枚举结果附看护者 `{pid, version, heartbeat_age_secs}`（读 claim）。
事件订阅与日志流作为新增 op，可在 0.2.x 增量上线，但帧格式现在定（第 3 节）。错误码清单与数据面审查对齐。

**成本** M

### CP-11（P1）看护者鲁棒性

**证据（事实）**
- 就绪后再崩溃无退避：重拉成功即把失败计数清零（`watchdog.rs:711-717`），下一次死亡 `failures=0` → 退避 0 → 立即
  重拉（`watchdog.rs:623-628, 910-916`）。每次启动都生成新的随机日志名（`server.rs:30-36`），旧日志只在 status 时才被
  孤儿清理。
- 重拉等就绪 8 秒，不检查子进程是否已退出（`watchdog.rs:963-972`）；同文件之外已有带早退检测的
  `wait_spawned_instance_ready`（`daemon.rs:947-968`）。
- 补种前删 claim（`server.rs:221-222`、`start.rs:341`）：新看护者直接 `acquire` 成功，`serve` 里处决假死前任的分支
  （`watchdog.rs:999-1010`）走不到，真挂死的旧看护者永久残留。
- 挂死阈值 `scan_secs × (cycles+1)` 无下限（`watchdog.rs:542`），守护心跳写入间隔固定 10 秒（`watchdog.rs:1094`）；
  doctor 只拦 0 与 >600（`src/doctor.rs:154-166`）。
- 连续失败达上限即永久放弃（`watchdog.rs:744-755`）。
- 看护者走 `init_foreground_logging`（`main.rs:53-58`）→ stdout → 被 spawn 的空标准流吞掉，所有 tracing 丢失；
  只有「放弃」写 startup.log（`watchdog.rs:750-752`）。
- 配置文件缺失即删除 `.restore`（`watchdog.rs:942-951`；`restore.rs:33-44` 同），只留 tracing（等于不留）。

**为什么重要**：就绪后立刻崩溃的实例会形成无退避的紧循环，每轮在 logs/ 留一个新文件；看护者的处决、重拉、放弃、
删记录都没有持久证据；配置所在的盘暂时不可达（网络盘、开机挂载晚）时，期望状态被永久删掉。

**建议**
- 稳定窗口：存活满 60 秒才把失败计数清零；窗口内再死按 `failures+1` 退避。
- 重拉复用 `wait_spawned_instance_ready`（并入 CP-17 的统一 spawn）。
- 补种不删 claim，交给新看护者核验并处决前任。
- 挂死阈值下限 `3 × HEARTBEAT_WRITE_INTERVAL_SECS`；claim 新鲜阈值同样设下限。
- 看护者日志写 `logs/watchdog.log`（英文、参与轮转、排除在孤儿清理外），处决/重拉/放弃/删记录四类事件必写。
- 放弃后转慢速重试（每 300 秒），需用户拍板（上一轮 watchdog-06 同）。
- 配置缺失时不删 `.restore`，标记并报告。

**成本** M

### CP-12（P1）unix 守护没有脱离启动者的进程组

**证据（事实）**：unix 分支只是 `Command::spawn`，没有 `process_group`/`setsid`（`daemon.rs:706-715`）；守护监听 SIGINT
（`tokio::signal::ctrl_c`）并优雅退出（`server.rs:388-409`）；优雅退出删 `.restore`（`server.rs:346-351`），看护者因此
按「优雅退出」摘除、不重拉（`watchdog.rs:616-621`）。看护者由 `aproxy start` 父进程或守护拉起，同样在同一进程组。

**推断**（依据 POSIX 作业控制，未实测）：`aproxy start` 等待就绪的 8 秒里按 Ctrl+C、`aproxy start && <其他命令>` 这类
复合作业被 Ctrl+C、或 agent/脚本按进程组终止子进程时，信号会送达整个前台进程组，守护与看护者都在其中；结果等同
「用户主动停止」，不会自愈。Windows 走 `CREATE_NO_WINDOW` 脱离控制台，不受影响。

**建议**：`CommandExt::process_group(0)`（std 已稳定）+ `pre_exec` 里 `setsid()`（像 `kill` 一样用 extern 声明，不加依赖）；
在 Linux CI 加测试：对启动 CLI 的进程组发 SIGINT 后实例仍在。按 ci-unix-blindspot 记忆，改 unix 路径后先等 ubuntu job。

**成本** S

### CP-13（P1）守护的工作目录与环境不确定

**证据（事实）**：Windows spawn 时 `lpCurrentDirectory=NULL`、`lpEnvironment=NULL`（`daemon.rs:687-688`），unix 同样继承
（`daemon.rs:709-714`）；看护者重拉继承看护者自己的 cwd 与环境（`watchdog.rs:952-955`）。aproxy-format skill 已把这件事写成
对外行为：「相对路径按守护的工作目录解析，取决于实例碰巧在哪启动」（`.claude/skills/aproxy-format/references/guide.md:266-268`）。

**为什么重要**：transform 的相对 `command`、裸命令名的 PATH 查找、`HTTP(S)_PROXY` 等，在「终端启动 / 看护者重拉 /
GUI 启动 / 开机 restore」之间结果不同——GUI 进程拿不到 shell 改过的 PATH。**推断**：Windows 上常驻进程持有 cwd 句柄，
用户无法删除或重命名启动 aproxy 时所在的目录。0.2.0 定下的语义会冻结。

**建议**：spawn 时把 cwd 固定为 home；配置里的相对路径按配置文件所在目录解析（需与 config 审查对齐）。代理类环境变量要么
在 CP-02 的结构化记录里快照（评估明文风险），要么文档写死「代理只认 config.toml 的 proxy」。

**成本** S（cwd）/ M（环境快照）

### CP-14（P1）看护者命令行复制了启动者的全部参数

**证据（事实）**：`watchdog_spawn_args` 取 `std::env::args().skip(1)` 原样加 `--daemon-watchdog`（`watchdog.rs:338-343`），
调用方是 `start.rs:342`（CLI 父进程的参数）与 `server.rs:223`（守护的参数）。

**为什么重要**：`aproxy --api-key sk-… start` 之后，常驻的看护者命令行里也有这把 key，进程列表可见；看护者还带着
`start <别名>`、`--config` 等无关参数，home 只靠环境隐式传递。

**建议**：看护者参数只放 `--daemon-watchdog` 与显式 `--home`。

**成本** S

### CP-15（P1）unix 心跳与安装宣告放在 `/dev/shm`

**证据（事实）**：`watchdog.rs:1429, 1457, 1473`；`src/install/announce.rs:24-35`。记忆 macos-support-required 的推荐方案就是挪到
`<home>/run`。

**为什么重要**：这是 macOS 看护者失效的根因之一；`/dev/shm` 全机共享，名字由 home 路径可推出，他人可以抢先创建同名文件，
让心跳创建失败（降级为无挂死检测）。0.2.0 不做 macOS，但换介质是平台无关的小改动，能直接消掉 macOS 的一半工作，且名字
属于跨版本接口（守护写、看护者读），晚改要过渡。

**建议**：unix 心跳 `run/<port>.heart`、宣告 `run/install.announce`（0700 目录内）；注释写明「非易失，靠心跳过期判定」。

**成本** S

### CP-16（P1）磁盘布局与命名

**证据（事实）**
- `run/<port>.pid` 存的是 JSON 记录，不是 pid；`.restore`、`watchdog.claim`、`install.state` 各用各的扩展名。
- 临时文件四种写法：`daemon.rs:747`（`.pid.tmp`）、`daemon.rs:1063`（`.restore.tmp`）、`watchdog.rs:802`（`.claim.<pid>.tmp`）、
  `settings.rs:311`（`.json.tmp`）。
- 同义不同名：claim 的 `created_at_process`（`watchdog.rs:35`）与注册记录的 `process_start`（`daemon.rs:159`）。
- `InstanceRecord` 一身二用：既是 `.pid` 磁盘格式，又是 IPC 状态快照里的 `instance`（`daemon.rs:140-146, 173-183`）。
- spool 按**配置**端口分目录（`src/proxy.rs:362`），且在 bind 之前清空（`server.rs:79-83`）。**推断**：端口为 0 的实例共用
  `spool/0/` 并在每次启动时互相清空；端口被占时起不来的那个进程（并发 start、restore 与看护者赛跑）也会先清掉在跑实例的
  spool 文件——影响取决于 spool 是否按路径重开，需数据面审查确认。

**为什么重要**：0.2.0 之后再改名就要写迁移代码；按扩展名过滤（`*.pid`）在混入新文件类型时容易误伤；一身二用的类型
让「只加磁盘字段」或「只加线上字段」都不可能。

**建议**：见第 5 节的布局草案。核心是：home 为唯一根；`run/` 0700；每类记录 `*.json` 带 kind 后缀并带 `schema`；临时文件
统一 `*.tmp-<pid>`；claim 字段名与注册记录统一为 `process_start`；磁盘记录与线上类型拆开（或明确同源并在契约测试里一起
冻结）；spool 按 pid 分目录，启动时只清「pid 已死」的目录（与数据面协调）。

**成本** M

### CP-17（P1）结构：职责混杂、平台分支分散、关键逻辑多处重复

**证据（事实）**
- `daemon.rs` 生产代码约 1580 行（测试从 `:1579` 起），同时含布局、端点命名、线协议、客户端、服务端、手写
  `CreateProcessW`、注册表、恢复记录、普查与日志清理、startup.log。
- `watchdog.rs` 承载与看护无关的进程身份原语（`record_identity`、`terminate_verified_process`、`process_exited`、
  `process_image_path`，`watchdog.rs:164-285`），被 daemon、stop、install 使用。
- 平台模块散在两个文件：`daemon.rs:1356, 1461`，`watchdog.rs:1179, 1258, 1346, 1425, 1480, 1585`。
- 「spawn + 等就绪」至少 4 份：`start.rs:250-321`（只 ping 端口，无早退检测）、`restart.rs:123-170`、`restore.rs:39-81`、
  `watchdog.rs:935-973`（另有 `src/install/restart.rs` 一份）。
- 原子写 4 份（`daemon.rs:743-757, 1050-1072`、`watchdog.rs:798-810`、`settings.rs:306-320`）；端口/别名目标解析 2 份
  （`stop.rs:30-150`、`logs.rs:16-73`）。

**为什么重要**：macOS 要改的是进程身份与共享介质，现在散在两个大文件里；UI 与 `--json` 要复用的是操作层与只读枚举，
现在和打印、清理缠在一起；四份 spawn 等待已经出现能力分化（start 没有早退检测）。

**建议**：拆分方案见第 6 节；先做纯移动，再合并重复，最后抽操作层。

**成本** L（可分四步，每步独立可验证）

### CP-18（P1）看门狗开关与调优只在进程启动时读取

**证据（事实）**：守护在启动时读一次 `settings.watchdog`，之后每 5 分钟补种（`server.rs:199-237`）；看护者启动时读调优值
（`watchdog.rs:378-392`）。skill 已把「改为 false 不会停掉正在运行的看护者，旧实例仍会补种」写成对外行为
（`.claude/skills/aproxy-cli/references/latest/settings-json.md:164-167`）。

**为什么重要**：UI 设置页的「关闭看门狗」要等所有实例重启才真正生效，向用户解释不清；写进文档就冻结。

**建议**：自检每轮重读开关；看护者每个 tick 读开关，关闭即退出（保留 `.restore`）；调优值每 tick 读或提供 reload。

**成本** S

### CP-19（P1）UI 编进本体后的控制面影响（对已定决策的补充，不推翻）

**事实**：install 拒绝管辖 `<home>/bin` 之外启动的实例（`src/install/flow.rs:55-60`）；拉起实例一律用 `current_exe`
（`start.rs:250`、`restart.rs:123`、`restore.rs:16`、`watchdog.rs:954`）。

**推断**
- GUI 若从 `<home>/bin` 之外的副本启动（npm 全局、下载目录），它拉起的实例日后会被 install 拒绝升级。
- install 换版后正在运行的 GUI 仍是旧版本，面对新实例——IPC 的「旧客户端读新服务端」方向必须成立（CP-03 的规则）。
- 守护、看护者、续作进程与 GUI 是同一个可执行文件：Linux 版动态链接 webkit2gtk 时，每个守护与看护者进程启动都要加载
  GUI 库（计划已列「缺库连代理都起不来」）；常驻内存与冷启动时间需要实测，看门狗「约 2–3MB」的设计前提可能不再成立。

**建议**：UI 拉起实例一律以 `<home>/bin/aproxy` 为目标（不存在时引导 install）；install 结束时提示 GUI 重启（或 GUI 发现
实例版本高于自己时提示）；W5 开工前在 ui feature 下测守护与看护者的 RSS 与冷启动时间。

**成本** S（决策）+ 实测

### CP-20（P2）`start` 的就绪等待落后

**证据（事实）**：`start` 只按端口 ping 8 秒（`start.rs:274-321`），不识别早退、不展示 startup.log 新增内容；端口 0 直接报
「正在后台启动」并退 0（`start.rs:261-266`）。restart 已用 `wait_spawned_instance_ready` + `startup_log_excerpt`
（`restart.rs:138-170, 228`）。

**建议**：并入 CP-17 的统一 spawn；端口 0 也按 pid 等就绪。

**成本** S

### CP-21（P2）优雅停止到最后才删 `.restore`；`stop all` 不处理待重拉实例

**证据（事实）**：`.restore` 在排空结束（或 10 秒宽限强退）时才删（`server.rs:321-337, 346-351`）；`stop all` 只停应答的实例
（`stop.rs:117-141`）。

**推断**：排空期间守护若崩溃，看护者按「`.restore` 在 = 崩溃」把用户刚停的实例拉回来；`stop all` 时恰在退避队列里的崩溃
实例之后仍会被拉起。

**建议**：收到 shutdown 立即删 `.restore`（期望状态已变为「停止」）再排空；`stop all` 是否同时清掉无存活进程的 `.restore`，
需定语义。

**成本** S

### CP-22（P2）Linux 身份锚点跨重启不唯一（推断）

**证据（事实）**：Linux 的创建时间取 `/proc/<pid>/stat` 的 starttime，单位是开机以来的时钟滴答（`watchdog.rs:1592-1599`）；
崩溃或断电后 `.pid`/`.restore` 按设计残留（`server.rs:144-151`）。

**推断**：开机顺序固定的机器上，下一次开机的另一个进程恰好拿到同一 pid 与同一 starttime 的概率不为零；status 会把它列为
「无响应」并建议 `--force`。Windows 的 FILETIME 是绝对时间，不受影响。

**建议**：Linux 额外记录 `boot_id`（`/proc/sys/kernel/random/boot_id`），或把 starttime 换算成绝对时间（btime + starttime/CLK_TCK）。
作为 CP-01 的记录格式一起定。

**成本** S

### CP-23（P2）Windows 作业对象（推断）

**证据（事实）**：刻意不加 `CREATE_BREAKAWAY_FROM_JOB`，理由是不允许脱离时 CreateProcess 会失败（`daemon.rs:620-622`）。

**推断**：启动者处在「关闭即杀」的作业对象里（部分终端、IDE、CI、agent 宿主）时，守护随作业关闭被杀。

**建议**：先带 `CREATE_BREAKAWAY_FROM_JOB` 调用，遇 `ERROR_ACCESS_DENIED` 去掉重试——两种情况都不比现在差。

**成本** S

### CP-24（P2）`.pid` 丢失后守护不补写

**证据（事实）**：`.pid` 只在启动时写一次（`server.rs:140-142`）；上一轮 daemon-ipc-04/-S04 建议的周期补写未做。

**建议**：心跳 ticker 每拍检查自己的 `.pid` 是否在，缺失则补写。只补 `.pid`，不补 `.restore`——否则与 `stop --force`
「终止前先摘 `.restore`」冲突（`stop.rs:232-260`）。

**成本** S

### CP-25（P2）`prepare_swap` 置位后没有出口

**证据（事实）**：置位后只有重启才清除（`daemon.rs:1213-1229`）；install 广播部分失败即返回错误（`src/install/flow.rs:902-907`），
已 ACK 的实例停在 `swap_prepared`，status 一直显示「二进制更换中，请勿手动干预」（`status.rs:79-81`）。

**建议**：`prepare_swap{ttl_secs}` 到期自动回到 serving，或加 `cancel_swap`（依赖 CP-03 的参数规则）。

**成本** S

### CP-26（P2）自定义 `log_file` 被孤儿清理删除、启动时被截断

**证据（事实）**：孤儿清理处理 logs/ 下所有 `.log`（`daemon.rs:819-834`），相对 `log_file` 按 home 解析（`server.rs:42-59`），
`log_file = "logs/work.log"` 正落在 logs/ 下；启动时超过 2 MiB 即清空（`server.rs:432-436`），对自定义文件同样生效。

**为什么重要**：用户指定的文件在实例停止后的下一次 status 被删除，或在重启时被清空，与「自定义日志位置」的预期相反。

**建议**：孤儿清理只处理符合内置随机名格式（`<hex>-<hex>.log`）的文件；截断只作用于内置日志。

**成本** S

### CP-27（P2）CLI 参数细节

**证据（事实）**：阈值参数只在 `idle` 分支使用（`stop.rs:39-51`），`stop 12345 600` 的 600 被静默忽略（`cli.rs:93` 只要求
有 target）；`--config` 与 `--log-file` 为 global，出现在每个子命令的帮助里（`cli.rs:16, 41`）；多行 doc comment 没有
`verbatim_doc_comment`，被 clap 折成一行（上一轮 daemon-ipc-10）。

**建议**：非 idle 目标带阈值报用法错误；`--log-file` 只挂在 start；帮助文本随 W3 迁到目录文件时一并处理。

**成本** S

### CP-28（P2）`compat_0_1_0.rs:29` 的 Windows 旧宣告名里是 BEL 字节

**证据（事实）**：十六进制 `4c 6f 63 61 6c 07 70 72 6f 78 79 …` = `"Local\x07proxy-install"`；应为 `Local\aproxy-install`
（`git show v0.1.0:src/install/announce.rs` 第 22 行为 `5c 61`）。它被 `src/install/announce.rs:87, 134` 使用。全仓控制字节
扫描只有这一处。

**影响**：S5 在 Windows 上从未生效——0.1.0 的守护与看护者看不到新安装器的宣告。0.2.0 删模块后自然消失；若 0.2.0 之前
还要发 0.1.x，必须先修。教训：写入工具把 `\a` 解释成了 BEL；提交前可加一道控制字节扫描。

**成本** S

### CP-29（P2）过期记录

**证据（事实）**：daemon-model 记忆仍写管道名 `\\.\pipe\aproxy-<端口>`、日志 `logs/<端口>.log`、只有 `ping`/`shutdown`、
「命名管道 ACL（tokio 不暴露）」；`.agents/TODO.md:474` 同样的结案理由。

**建议**：W2 定稿后重写 daemon-model 记忆与 ipc-v1-upgrade-contract 记忆（后者按第 4 节改为 0.2.0 冻结清单）。

**成本** S

---

## 2 上一轮相关条目现状

来源：`.agents/review/2026-10-04-0.1.0发布前-13维审查.md`。「部分」= 有实质修复但仍有缺口。

### 2.1 daemon-ipc / watchdog / security / ui 条目

| 条目 | 摘要 | 现状 | 证据 |
|---|---|---|---|
| daemon-ipc-01 | restart 先停后校验 | 已修 | `restart.rs:169-215` 预检；测试 `tests/restart_integration.rs:415` |
| daemon-ipc-02 | `stop --force` 不清 `.restore` | 已修 | `stop.rs:232-260` 先摘后杀、失败放回；测试 `tests/proxy_integration.rs:6325` |
| daemon-ipc-03 | IPC 起不来实例仍跑 | 已修（sun_path 无预检，只在报错文案里提示） | `server.rs:100-115`；测试 `proxy_integration.rs:4928` |
| daemon-ipc-04 | 瞬时停顿即删 `.pid` | 部分：核验后才删、挂死单列；周期补写未做（CP-24） | `daemon.rs:842-904`；测试 `proxy_integration.rs:4747` |
| daemon-ipc-05 | idle 忽略在途请求 | 未修（CP-10） | 无在途计数 |
| daemon-ipc-06 | 失败退出码为 0 | 部分：stop、restart 已非零；restore 仍恒 0，无退出码表（CP-04） | `stop.rs:155-171`、`restart.rs:46-95`、`restore.rs` |
| daemon-ipc-07 | unix/Windows 端点语义不一 | 已修（按 home 命名空间）；`APROXY_RUN_DIR` 双变量仍在（CP-05） | `daemon.rs:80-97`；测试 `proxy_integration.rs:4836` |
| daemon-ipc-08 | unix 权限 | 未修（CP-09） | 全仓无相关 `set_permissions` |
| daemon-ipc-09 | IPC 无读超时/并发上限 | 部分：5 秒读超时；无并发上限 | `daemon.rs:1198-1208` |
| daemon-ipc-10 | CLI 帮助与参数 | 部分：阈值静默忽略、global 泄漏、折行、无 `--json` 仍在（CP-27、CP-04） | `cli.rs`、`stop.rs:39-51` |
| daemon-ipc-11 | unix spawn 未脱离会话 | 未修，并升为 P1（CP-12） | `daemon.rs:706-715` |
| daemon-ipc-12 | 文案与协议注释漂移 | 已修（Stats 删除、taskkill 提示改为 `stop --force`） | `stop.rs:204-214` |
| daemon-ipc-S01 | `--json` 与退出码表 | 未做（CP-04） | — |
| daemon-ipc-S02 | restart 预检 + `--force` 清理 + 回归测试 | 已做 | 同 01/02/04 |
| daemon-ipc-S03 | IPC 可订阅/可读配置/日志流 | 未做；能力表与错误码已就位（第 3 节） | — |
| daemon-ipc-S04 | 注册表自愈 | 部分（同 04） | — |
| daemon-ipc-S05 | unix 权限 + 跨 home 语义 | 跨 home 语义已定；权限未做 | — |
| daemon-ipc-S07 | 秒死时展示 startup.log | 部分：restart 已做，start 未做（CP-20） | `restart.rs:138-170`、`start.rs:274-321` |
| watchdog-01 | 换端口后旧 `.restore` 残留 | 已修 | `daemon.rs:970-1001`、`watchdog.rs:681-698`；测试 `restart_integration.rs:697, 761, 844` |
| watchdog-02 | 看护者日志全丢 | 未修（CP-11） | `main.rs:53-58` |
| watchdog-03 | macOS 看护者失效 | 按用户定调不在范围，未变 | — |
| watchdog-04 | 身份依赖名称 | 已修 | `watchdog.rs:217-285`；测试 `restart_integration.rs:599, 912` |
| watchdog-05 | 重拉继承看护者环境 | 未修（CP-13） | `watchdog.rs:952-955` |
| watchdog-06 | 约 1 分钟即永久放弃、不识别早退 | 未修（CP-11） | `watchdog.rs:744-755, 963-972` |
| watchdog-07 | 就绪后再崩溃无退避 | 未修（CP-11） | `watchdog.rs:711-717` |
| watchdog-08 | 补种先删 claim | 未修（CP-11） | `server.rs:221-222`、`start.rs:341` |
| watchdog-09 | 心跳阈值无下限 | 未修（CP-11） | `watchdog.rs:542`、`doctor.rs:154-166` |
| watchdog-10 | 安装宣告名全局 | 已修（按 home）；Windows 上的 0.1.x 回退因 BEL 字节无效（CP-28） | `announce.rs:24-35` |
| watchdog-S03 | 看护者可观测性 | 未做（CP-10、CP-11） | — |
| watchdog-S04 | 看门狗回归测试 | 部分：换端口、改名二进制已有；就绪后崩溃、注入时钟未做 | `restart_integration.rs:599, 697, 761` |
| watchdog-S05 | `.restore` 记录环境变量 | 未做（CP-13） | — |
| watchdog-S06 | 慢速重试 + 早退判失败 | 未做（CP-11） | — |
| watchdog-S07 | 看护者 IPC 端点 | 未做；建议先预留名字（第 3.1 节） | — |
| security-03 | 日志/last_error 泄露凭据 | 已修（数据面，本审查未深查） | `proxy.rs:3207` `without_url`；测试 `proxy_integration.rs:4480` |
| security-08 | 管道 ACL 结案理由错误 | 部分：客户端已核对服务端 pid；SDDL 未做，结案理由未更正（CP-09、CP-29） | `daemon.rs:403-413`；`TODO.md:474` |
| security-09 | `--api-key` 明文持久化 | 未修，且新发现看护者命令行泄漏（CP-02、CP-14） | `cli.rs:43-46`、`server.rs:152-159` |
| security-S02 | local_token | 未做 | — |
| security-S07 | UI 安全前置约束 | 未落成计划（`release-0.2.0.md` W5 只有原则三句） | — |
| security-S08 | IPC 纵深防御 | 部分：读超时、客户端 pid 核对；SDDL、uid 未做 | — |
| ui-01 | settings 写入 | 部分：损坏时拒写；未知字段丢弃、默认值固化未修（CP-01） | `settings.rs:256-276, 306-320` |
| ui-02 | config.toml 写回 | 未修（config 审查范围） | `config.rs:939-945` |
| ui-03 | 能力协商与错误码 | 已修；未知参数规则缺失（CP-03） | `daemon.rs:181-182, 283-293` |
| ui-04 | 枚举有副作用且串行 | 部分：只删核验已死的记录；仍串行、仍顺带清日志（CP-06） | `daemon.rs:780-904` |
| ui-05 | 寻址依赖环境变量 | 部分：库层 `*_in` 齐全；无 `--home`，`APROXY_RUN_DIR` 仍在（CP-05） | — |
| ui-06 | 管道 ACL | 未修（CP-09） | `daemon.rs:1416-1419` |
| ui-S01 | caps + error.code | 已做（`ops` 列表 + 稳定错误码） | `daemon.rs:1339, 283-293` |
| ui-S02 | mode 字段 | 未做（CP-10） | — |
| ui-S03 | aproxy-ipc crate | 未做；UI 已定编进本体，独立 crate 不再必要（第 6 节） | — |
| ui-S04 | `--json` | 未做（CP-04） | — |
| ui-S05 | 结构化错误码 | 部分：IPC 层有 code；last_error 仍是中文串（CP-10） | — |
| ui-S06/S07/S08 | 事件订阅 / 指标环 / 日志流 | 未做（第 3 节给形态） | — |
| ui-S09 | 配置读写 API | 部分（settings 损坏拒写） | — |
| ui-S10 | 只读并发枚举 | 未做（CP-06） | — |
| ui-S12 | UI 不内置 aproxy | 被用户推翻（UI 编进本体） | 记忆 0.2.0-direction |
| ui-S13 | latest 只认主线 tag | 已修 | `src/install/download/github.rs:64-137` |
| ui-S15 | Windows serve 健壮性 + 读超时 | 已修 | `daemon.rs:1428-1458, 1198-1208` |

### 2.2 路线图与 UI 后端前置清单

| 项 | 现状 |
|---|---|
| 0.1.x-3 注册表自愈与在途计数 | 部分：普查按身份核验；无在途计数、无 `.pid` 补写 |
| 0.1.x-4 机器可读输出/退出码/模式/能力 | 部分：能力与错误码有；`--json`、退出码表、mode、看护者状态无 |
| 0.1.x-10 unix 权限与 macOS 资源上限 | 未做 |
| 0.1.x-11 看护者可观测性与重拉语义 | 未做（换端口一项属 watchdog-01，已修） |
| 0.1.x-13 供应链与 IPC 纵深（IPC 部分） | 部分（同 security-08） |
| 0.1.x-19 local_token / 热重载 / autostart | 未做 |
| 0.2.0-1 UI 安全边界前置 | 未做 |
| 0.2.0-2 IPC 契约 crate + 契约测试 | 未做；crate 不再必要，契约测试必须做（CP-01） |
| 0.2.0-3 可观测数据面 | 未做 |
| 0.2.0-5 日志经 IPC 流式 | 未做 |
| 前置：latest 选择修复 | 已修 |
| 前置：Host/Origin 校验与统一脱敏 | Host/Origin 已有（`settings.rs:87-97` 的全局默认层）；脱敏非本范围 |
| 前置：配置写回安全 | 部分（settings 损坏拒写；toml 未修；未知字段未保留） |
| 前置：mode、caps、error.code | caps 与 error.code 有；mode 无 |
| 前置：`--json` 与退出码表 | 无 |
| 前置：在途计数与只读并发枚举 | 无 |
| 前置：Windows serve 退避重建与读超时 | 已修 |
| 前置：SDDL、对端 pid、unix 0700 | 客户端侧 pid 核对有；其余无 |
| 前置：身份去名称化 | 已修 |
| 前置：IPC crate、显式 home、契约测试 | 无 |

---

## 3 0.2.0 IPC 建议形态

### 3.1 谁走 IPC

- **实例级端点**（每实例一个，现状保留）：观测（状态快照、事件流、日志流）与少量状态变更（停止、更换阶段）。
- **实例不存在时的操作不走 IPC**：start、restore、alias、配置编辑由操作层完成（CP-08）——CLI 调它，GUI 在本进程里调同一个；
  install 要替换二进制，GUI spawn `<home>/bin/aproxy install --json` 读 NDJSON 进度。
- **home 级端点**（看护者）：0.2.0 不实现，但现在预留名字——Windows `\\.\pipe\aproxy-<home_id>-watchdog`，unix
  `<home>/run/watchdog.sock`。非数字后缀不会与端口冲突。看护者状态先由 claim + `logs/watchdog.log` 提供。
- 不新开网络端口，「控制通道不占代理端口」的铁律不变。

### 3.2 传输与访问控制

- 端点名不变：Windows `\\.\pipe\aproxy-<home_id>-<port>`（`home_id` = 规范化 run 目录路径的 SHA-256 前 16 位十六进制，
  `daemon.rs:99-133`），unix `<home>/run/<port>.sock`。这两条与 `home_id` 算法本身都是永久接口（第 4 节）。
- 服务端：Windows 显式 SDDL（用户 SID + SYSTEM）、`first_pipe_instance`、拒绝远程（tokio 默认）；unix run 目录 0700、socket 0600、
  `peer_cred` uid 核对。
- 客户端：核对服务端 pid 与自报 pid（现状），unix 加 uid 核对。
- 一元请求：请求行 ≤ 64 KiB、连上后 5 秒内发完（现状）；客户端总超时 3 秒（现状）。

### 3.3 帧与信封

一行一个 JSON 对象，UTF-8，`\n` 结尾。一元 op 一请求一应答一连接（现状保留：简单，在管道实例模型下没有队头阻塞）。
流式 op 在同一连接上先回一行一元应答，之后每行一个事件，直到任一方关闭。**不做多路复用 RPC。**

```text
请求      {"v":1,"op":"shutdown","args":{"drain_secs":30}}
成功      {"v":1,"ok":true,"result":{...op 专属...}}
失败      {"v":1,"ok":false,"error":{"code":"unsupported_arg","message":"unknown arg: drain_secs","details":{"args":["drain_secs"]}}}
事件      {"v":1,"event":"attempt_failed","seq":42,"ts":1760190000123,"data":{...}}
```

### 3.4 规则（写进契约文档并用测试钉住）

1. 服务端用请求的 `v` 应答；0.2.0 起发布过的 `v` 一律保持支持；不支持时回 `unsupported_version` 并在 `details.supported` 列出。
2. 未知 op → `unknown_op`（无副作用，可用于探测）；未知参数 → `unsupported_arg`；参数类型不对 → `bad_request`。
3. 应答与事件只增字段；客户端忽略未知字段与未知事件类型；枚举一律有 `unknown` 兜底（`InstanceState` 已有 `#[serde(other)]`）。
4. `message` 只给日志，固定英文；显示端按 `code` + `details` 本地化。
5. `ops` 列出支持的 op；新增 `features` 列出 op 之下的能力（如 `shutdown.drain_secs`），客户端先查再用。
6. 改变实例状态的 op 必须幂等或带条件参数；新的破坏性或敏感 op 只在 CP-09 落地后加入。

错误码：现有 `bad_request`、`unsupported_version`、`unknown_op`、`invalid_state`，新增 `unsupported_arg`、`busy`（流式连接超额）、
`internal`。

### 3.5 op 集合

| op | 何时 | 参数 | 结果 | 说明 |
|---|---|---|---|---|
| `ping` | 现有 | — | 状态快照 | 扩字段见 3.6 |
| `shutdown` | 现有 | `drain_secs?`、`if_idle_secs?` | 状态快照（stopping） | 参数可在 0.2.x 实现，但 0.2.0 起未知参数必须报错；收到即删 `.restore`（CP-21） |
| `prepare_swap` | 现有 | `ttl_secs?` | 状态快照 | install 内部用；到期回 serving（CP-25） |
| `subscribe` | 0.2.x | `topics` | 流 | 首行结果带快照（当前在途请求），之后增量 |
| `logs` | 0.2.x | `tail_lines?`、`follow?` | 流 | 服务端读自己的日志文件（知道轮转）；前台实例回 `invalid_state` |
| `config` | 0.2.x | — | 脱敏后的生效配置 + 每字段来源（toml/settings/cli/default） | 只读；写配置走文件 + restart（以后再加 reload） |

不建议经 IPC 写配置：GUI 与 CLI 同用户，直接经操作层改文件即可，IPC 的写面越小越好。

### 3.6 状态快照（新增字段全部可选）

```json
{"instance":{"pid":4321,"process_start":133000000000,"version":"0.2.0","listen_addr":"127.0.0.1:12345",
             "config_path":"C:/Users/u/.aproxy/config.toml","base_url":"https://api.example.com","started_at":1760190000,
             "log_path":"C:/Users/u/.aproxy/logs/19ac3f2e8b5d-1a2b.log"},
 "run_dir":"C:/Users/u/.aproxy/run","state":"serving",
 "activity":{"last_request_at":1760190100,"requests_total":120,"retries_total":7,
             "in_flight":2,"retrying":1,
             "last_error":{"code":"upstream_status","details":{"status":502},
                           "message":"upstream returned 502, retrying","at":1760190090}},
 "mode":{"forward_only":false,"disk_cache":true,"keepalive_secs":20,
         "request_transform":true,"response_transform":false,"bounded_retry_paths":1},
 "ops":["ping","shutdown","prepare_swap"],"features":[]}
```

`last_error.code` 初稿（以数据面审查的错误分类为准）：`upstream_network`、`upstream_status`、`upstream_error_body`、`spool_limit`、
`spool_io`、`transform_request`、`forward_upstream`（仅转发模式）、`stream_interrupted`。

### 3.7 事件流

- 事件：`request_started{request_id, method, path(脱敏)}`、`attempt_failed{request_id, attempt, code, details, backoff_ms}`、
  `request_completed{request_id, status, attempts, duration_ms, bytes}`、`keepalive{request_id}`、`state{state}`、
  `lagged{dropped}`、`heartbeat`（每 15 秒，让客户端识别挂死实例）、`closing`（实例退出前）。
- `request_id` 与转换器信封、日志用同一个，便于串联。
- 热路径：`tokio::sync::broadcast`，`receiver_count()==0` 时不构造事件；慢订阅者收到 `lagged`；每实例订阅连接数设上限（超额 `busy`）。
  进入热路径前用 loadgen 实测（上一轮已提，仍成立）。

### 3.8 客户端只读枚举（库 API，GUI 与 `--json` 共用）

`enumerate(home) -> Vec<Entry>`：读 run 目录 → 对每条记录并发 ping（单次 1 秒超时、最多重试 1 次）→ 返回
`Live(status)` / `Unresponsive(record)` / `Incompatible(record, reason)` / `Dead(record)` / `Unreadable(file, reason)`，**不删任何文件**。
附看护者状态（读 claim + 身份核验）。清理另走 CP-06 的路径。

### 3.9 契约测试

golden JSON：每个 op 的请求、成功应答、每种错误应答、事件样例；持久记录样例。CI 双向解析。之后版本「新读旧」以它为基线。

---

## 4 永久接口清单重审（记忆 ipc-v1-upgrade-contract）

清单是为「0.1.0 → 新版本」写的；0.2.0 不再兼容 0.1.0，清单里的每一项都可以重新设计一次，0.2.0 发布时再冻结。

| 条目（现清单） | 0.2.0 建议 | 理由 |
|---|---|---|
| `run/<port>.pid` 必填字段集 | **改**：加 `schema`、只增可选字段、读不懂不删（CP-01）；文件名改 `<port>.instance.json`（CP-16）；磁盘类型与线上类型拆开或一起冻结 | 「全部必填」只为 0.1.0 读者；`.pid` 名不副实 |
| `.restore` 字段名 `args`、`log_path` | **改**：结构化期望状态（CP-02） | argv 把 CLI 语法与明文密钥冻结进存储 |
| `watchdog.claim` 三字段 + `version` | **改小**：保留语义，`created_at_process` 改名 `process_start`、加 `schema`；读不懂按 mtime 判新鲜（CP-01） | 与注册记录统一；避免旧读者夺权 |
| `install.state` 阶段词表冻结、新字段可选 | **清理一次再冻结**（install 审查定）：去掉只为 0.1.0 存在的相位（如 relaying） | 早交接后只有「较旧的安装者读较新的状态」这一方向 |
| unix socket `run/<port>.sock`；控制协议 v1 | **路径保留；v1 按第 3 节定稿后再冻结** | v1 没有随任何正式版发布，现在改不需要升版本号 |
| CLI：`--version` 输出 | **保留**，另加机器可读形式（如 `aproxy version --json`，含协议与记录 schema 版本、变体） | install 靠它识别二进制版本；UI 双版本需要区分变体 |
| CLI：`install --continue`、`--handover-from <pid>` | **保留** | 早交接的基础 |
| CLI：`.restore` 里可能出现的每个启动参数 | **删除这一条**（CP-02 之后不再成立） | — |
| R1：守护启动时退役更旧的看护者 | **保留**；0.2.0 起「无 version 的 claim」不会再出现，判断可简化 | 新版本胜出、不拉锯 |
| 早交接：安装由目标版本执行 | **保留** | — |
| **应补入**：Windows 管道名格式与 `home_id` 算法 | 冻结 | 客户端与服务端各自计算；GUI 也要算 |
| **应补入**：心跳与宣告的名字/路径 | 先按 CP-15 挪到 run 目录再冻结 | 守护写、看护者读，可能跨版本 |
| **应补入**：home 级端点预留名 | 冻结名字（实现以后再说） | 防止以后与别的名字冲突 |
| **应补入**：settings.json 字段与「保留未知字段」规则 | 冻结 | 所有版本共写一份文件 |
| **应补入**：退出码表、`--json` schema | 冻结 | agent 与脚本依赖 |
| **应补入**：`APROXY_HOME`、`--home`；删除 `APROXY_RUN_DIR` | 冻结 | 寻址的唯一根 |
| **应补入**：隐藏标志 `--daemon-child`、`--daemon-watchdog` | 冻结（skill 已教 agent 用它们辨认进程，`commands.md` 「Processes you may see」） | 识别进程、跨版本重拉 |
| **应补入**：`logs/startup.log`、`logs/watchdog.log` 的路径 | 冻结 | 排障入口写在文档里 |

---

## 5 磁盘布局

### 5.1 现状

```text
<home>  (APROXY_HOME，默认 ~/.aproxy)
  config.toml  configs/            用户配置
  settings.json                    程序管理 + 各实例字段的全局默认层
  bin/  staging/<ver>/  skills/    install 管辖
  logs/  startup.log  <nanos>-<pid>.log
  spool/<配置端口>/*.spooltmp
  run/   (可被 APROXY_RUN_DIR 整体改指)
    <port>.pid        JSON 实例记录
    <port>.restore    JSON {args, log_path}
    <port>.sock       unix 控制端点
    watchdog.claim    JSON
    install.state     JSON
    <port>.pid.tmp / <port>.restore.tmp / watchdog.claim.<pid>.tmp
/dev/shm/aproxy-<home_id>-heart-<port>、aproxy-<home_id>-install    (unix，home 之外)
Windows：\\.\pipe\aproxy-<home_id>-<port>、Local\aproxy-<home_id>-heart-<port>、Local\aproxy-<home_id>-install
```

问题：两个根（CP-05）；home 之外的 `/dev/shm`（CP-15）；扩展名各自为政、`.pid` 名不副实、临时文件四种写法（CP-16）；spool 按
配置端口（CP-16）；run 目录无权限收紧（CP-09）。

### 5.2 建议（需与 install、数据面审查对齐后定稿）

```text
<home>                               唯一根（APROXY_HOME / --home）
  config.toml  configs/  settings.json
  bin/  staging/  skills/
  logs/
    startup.log                      启动失败与看护者重大事件（英文）
    watchdog.log                     看护者日志（新增，英文，参与轮转，不进孤儿清理）
    <nanos>-<pid>.log                实例日志（随机名，用户定调不变）
  spool/<pid>/                       按进程分目录；启动时只清 pid 已死的目录
  run/                               0700
    <port>.instance.json             实例记录（原 .pid），带 schema
    <port>.restore.json              结构化期望状态（CP-02），带 schema
    <port>.sock                      unix 控制端点（0600）
    <port>.heart                     unix 心跳（原 /dev/shm）
    watchdog.json                    看护者 claim（原 watchdog.claim），带 schema
    watchdog.sock                    预留：home 级端点
    install.json / install.announce  install 审查定
    *.tmp-<pid>                      统一的临时文件命名
```

也可以按类分子目录（`run/instances/`、`run/restore/`），枚举时不靠扩展名过滤；代价是权限与清理要多管几个目录。倾向平铺加
kind 后缀，一个 0700 目录最简单。

---

## 6 结构拆分建议

UI 已定编进本体，独立的 `aproxy-ipc` crate 不再必要：GUI 与 CLI 在同一个 crate 里共用库代码。仍然要做的是把协议、客户端、
端点命名收成独立模块并用契约测试冻结；以后真有第三方消费者，这几块就是 crate 的边界（依赖只有 serde、serde_json、
tokio 的 net、sha2、windows-sys）。

```text
src/
  home.rs              home()/--home、run/logs/spool/bin 路径与各文件名（布局单源）
  fsx.rs               write_atomic（4 处合一）、临时文件命名
  ipc/
    proto.rs           线上类型、错误码、规则（契约测试的对象）
    endpoint.rs        home_id、端点名（两端共用）
    client.rs          请求/ping/重试/对端核验/流式读取
    server.rs          ControlState、handle_conn、快照、事件广播
  registry/
    record.rs          实例记录（磁盘）
    restore.rs         结构化期望状态
    enumerate.rs       只读并发枚举（四态）
    gc.rs              死记录与孤儿日志清理（带宽限）
  spawn.rs             spawn_detached + spawn_from_record + wait_ready（四份合一）
  platform/
    mod.rs             函数级门面：进程身份/退出/核验后终止/镜像路径/死亡等待、管道或 UDS、心跳介质、脱离会话
    windows/           process.rs、pipe.rs、shm.rs、spawn.rs
    unix/              uds.rs、spawn.rs（kill、setsid、process_group）
    linux/process.rs   /proc
    macos/process.rs   以后用 libproc 实现，只需这一个文件（心跳已在 run 目录）
  watchdog/
    claim.rs  election.rs  heartbeat.rs  supervisor.rs（WatchdogState / serve）
  ops/                 操作层：enumerate/start/stop/restart/restore/alias（CP-08）
  instance.rs          serve_forever（现 bin 的 server.rs，迁入 lib 供 UI 与测试复用）
bin：cli.rs、render/{text.rs(i18n), json.rs}、commands/* 变成「解析参数 → 调 ops → 渲染」的薄壳
```

分四步，每步独立提交、独立验证（改了 unix 路径的步骤先等 ubuntu job，见 ci-unix-blindspot）：
1. 纯移动，不改行为：平台原语移出 watchdog.rs；home/fsx；ipc 拆分。
2. 合并四份 spawn 等待（行为变化：start 获得早退检测与 startup.log 摘录）。
3. registry 拆分 + 只读枚举 + 清理迁移（CP-06/07）。
4. ops 层 + render（CP-08），为 `--json`、W3、W5 铺路。

---

## 7 i18n 分类

方法：脚本扫描范围内文件的非测试代码，统计含 CJK 的字符串字面量，按所在宏/调用归类，再人工校正。数字是约数（以字面量
计，一条消息可能由几段拼成）。`--help` 以 doc comment 行计。

### 7.1 按受众

| 类别 | 内容 | 数量（约） | 处理 |
|---|---|---|---|
| 给用户看（需 i18n） | clap 帮助 doc comment（`cli.rs`） | 121 行 | 迁到目录文件，运行时注入 |
| | CLI 的 println/eprintln（start 24、stop 18、status 12、logs 12、alias 11、restart 8、restore 7、find.rs 4、前台横幅 3、settings 加载警告 1） | 100 | 渲染层 |
| | CLI 侧拼出的提示与错误（restart 预检与恢复指引 12、settings 别名保留字与检查 16、status 4、logs 3、restore 2、时长人性化 4、find 2、默认配置失效 1） | 44 | 渲染层 |
| 固定英文（日志/不可见） | tracing（watchdog 24、server 13、daemon 7、start 2） | 46 | 直接改英文 |
| | 只进看护者日志的重拉错误（`watchdog.rs:940, 950, 954, 955, 969`） | 5 | 改英文 |
| | startup.log 专写（看护者放弃 `watchdog.rs:751`） | 1 | 改英文 |
| | panic/expect 文案 | 14 | 改英文 |
| 经 IPC 传出的人类文本（固定英文 + 错误码） | `WireError.message`（`daemon.rs:1217, 1256, 1259, 1274, 1281, 1284, 1290`）+ last_error 兜底「未知错误」（`daemon.rs:512`） | 8 | message 改英文，显示端按 code 渲染 |
| | `last_error.message`：`proxy.rs` 约 20 处 `note_upstream_failure`（范围外文件，但经 IPC 与 status 展示） | ~20 | 改为 `{code, details}` + 英文 message（CP-10） |
| 双用途（同一串中文既给人看又进日志/另一端，须拆成码 + 参数） | `IpcError` 文案（`daemon.rs:312, 397, 410, 420, 425, 428, 438, 1170, 1172, 1384, 1390, 1392, 1478`）：被 CLI 拼进提示，也进 install/看护者日志 | 13 | 结构化错误，CLI 渲染、日志用英文 Display |
| | 核验后终止的错误（`watchdog.rs:270, 1235, 1240, 1243, 1246, 1323, 1326, 1332, 1339`）：`stop --force` 失败展示 + 看护者日志 | 9 | 同上 |
| | bind 失败分类（`server.rs:358-362`）与控制通道创建失败（`server.rs:108`）：前台/父进程 stderr + 守护 startup.log | 4 | 同上；startup.log 写英文，restart 摘录 startup.log 时显示英文可以接受 |
| | 配置错误（`start.rs` `resolve_runtime_config` 6 处 + config.rs 的 validate 文案）：CLI + startup.log | 6+ | 同上（与 config 审查协调） |

合计：给用户看约 144 条字面量 + 121 行帮助；固定英文约 66 条；IPC 线上约 28 条；双用途约 32 条。

### 7.2 特别说明

- `startup.log` 同时被人和程序读（restart 会把新增内容摘给用户看，`restart.rs:228-262`）：按「日志 = 英文」处理，CLI 在摘录时
  识别已知错误码再本地化，未知行原样显示。
- 状态行里的 `last_error` 与 `WireError` 目前是唯一经 IPC 传出的人类文本；CP-03 与 CP-10 落地后，线上只剩英文 message 与码。
- clap 自己的解析错误是英文，计划已决定第一期保留。
- i18n 让文本输出随机器变化，`--json` 必须同一版到位（CP-04）。

---

## 8 退出码与 `--json` 草案

退出码见 CP-04。`status --json` 样例：

```json
{"schema":1,
 "home":"C:/Users/u/.aproxy",
 "watchdog":{"state":"running","pid":1234,"version":"0.2.0","heartbeat_age_secs":12},
 "instances":[
   {"port":12345,"liveness":"live","status":{ "...":"第 3.6 节的状态快照" }},
   {"port":12346,"liveness":"unresponsive","record":{"pid":5555,"version":"0.2.0","listen_addr":"127.0.0.1:12346","config_path":"…"}},
   {"port":12347,"liveness":"incompatible","record":{"...":"…"},"reason":{"code":"unsupported_version","details":{"supported":[2]}}}],
 "unreadable":[{"file":"run/12348.instance.json","reason":{"code":"schema_too_new","details":{"schema":2}}}]}
```

stop/restart/restore 的 `--json`：每个目标一个结果对象 `{"port","outcome":"stopped|already_stopped|failed|skipped","error":{…}}`，
退出码按汇总规则。install：NDJSON，每行一个阶段事件（阶段词表与 install.state 相同），末行结局。

---

## 9 对已定决策的意见

没有发现需要推翻的用户定调。逐条说明：

- **进程身份不依赖名称**：已完全落实，本报告的建议都在这个前提下（CP-22 是加强锚点）。
- **日志随机命名、IPC 是日志地址权威、`log_file` 不进 settings**：保持。要改的是配套的清理机制——「引用集 + 在 status 时清」
  是代理（不是用户）当时的设计，它带来 CP-06 的竞态与 CP-11 的文件堆积，CP-26 的误删。
- **IPC 铁律**：保持；第 3 节的设计不新增任何网络端口。
- **UI 编进本体**：不推翻。它对控制面有三处影响需要在 W5 前定（CP-19）：拉起实例用哪个二进制、换版后旧 GUI 面对新实例、
  守护/看护者与 GUI 同一可执行文件带来的加载与内存成本（Linux 动态链接 webkit2gtk 时最明显）。
- **永久接口清单**（代理所写，非用户定调）：按第 4 节重订。
- **`.restore` 存 argv**（代理所写）：建议改（CP-02）。

---

## 10 测试建议

- `tests/contract_v0_2.rs`：冻结 0.2.0 的持久记录与 IPC 形状（取代 compat_v0_1_0）；之后的 CI「从 0.2.0 升级」任务取代
  upgrade-from-0-1-0。
- IPC：未知参数 → `unsupported_arg`；新客户端对旧服务端、旧客户端对新服务端（模拟多出的字段与事件类型）。
- 读者容忍：放入 schema 更高或无法解析的 `.pid`/`.restore`/claim/settings，断言不被删除、settings 写回保留未知字段。
- 竞态回归：守护「建日志 → 写 `.pid`」窗口内跑 status，断言日志仍在（可在测试构建里加延迟钩子）。
- 看门狗：就绪后立即崩溃的实例累计退避；重拉遇子进程早退立即判失败；补种不删 claim 时假死前任被处决。
- unix：对启动 CLI 的进程组发 SIGINT 后实例仍在（CP-12）。
- 枚举四态：版本不匹配的应答者被归为 incompatible，不建议 `--force`。

---

## 11 未覆盖与不确定

- 没有编译或运行任何代码；所有「推断」项都需要实测：CP-06 的 Windows 删除行为（依据 std 默认共享模式）、CP-12 的进程组信号
  传播（依据 POSIX 作业控制）、CP-16 的 spool 影响（取决于数据面是否按路径重开 spool 文件）、CP-22、CP-23。
- 本机不能编译 Linux 目标（记忆 ci-unix-blindspot），unix 相关改动要交 CI/WSL 裁决。
- install 流程只读了与 IPC 相关的调用点，阶段词表的清理留给 config-cli-install 审查；last_error 的错误码清单以数据面审查为准；
  config.toml 写回、settings 的全局默认层设计属 config 审查。
- macOS 不在范围；建议里与 macOS 相关的只有「别让它更难做」的部分（CP-15、第 6 节的 platform 目录）。
