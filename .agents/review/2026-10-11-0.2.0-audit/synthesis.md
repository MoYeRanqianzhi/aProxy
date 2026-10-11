# 0.2.0 审查综合：发现清单、工作流排序、待拍板决策与里程碑（2026-10-11）

> 输入：同目录四份报告——dataplane（DP-）、control-plane（CP-）、config-cli-install（CI-）、release-tests-docs（RT-）。
> 用户定调以记忆 `0.2.0-direction`、`ui-design-style` 与 `.agents/plan/release-0.2.0.md` 为准，含 23:19、23:27 两次提交新增的
> 「安全边界在实例与 IPC」和 W2b 强制安装契约 C1–C8（W2b 在计划里仍标「设计稿，待用户确认」）。
>
> 方法：只读综合。未改任何已有文件，未运行 cargo，未启动 aproxy，未读写 `~/.aproxy`。结论的证据来自四份报告，事实与推断的标注沿用原报告。
> 本稿另外核对了三处：`src/main.rs`（`--version` 在 `Cli::parse()` 内退出，之前只调了控制台代码页、不碰 home）、`.agents/TODO.md` 的现行行号
> （RT 附表 B 用的是 `c1d4b96` 的行号，`b751a0f` 之后整体下移），以及相关记忆原文。
>
> 记号：P0 = 0.2.0 前必须做或必须定；P1 = 0.2.0 应做，⚑ = 只有 0.2.0 能低成本改；P2 = 可以之后。「决策 D-n」见第 3 节。
>
> 怎么用：主代理先就第 3 节问用户（D-1…D-11），同时按第 2.5 节开 Phase 0/1；用第 2、4 节改写计划的工作流与顺序；第 5 节是记录卫生待办。

---

## 0 综合判断

1. 四份报告互相印证，没有实质矛盾。0.2.0 的风险集中在两处：一是还没定稿就要长期稳定的契约，二是会把每次改动放大三倍的结构。功能缺失不是主要问题。
2. 0.1.0 → 0.2.0 的干净失败已经有不写兼容代码的解法：`--version` 改为 `aproxy v<semver> …`（CI-01，与 W2b C5 一致），0.1.0 的自证在交换二进制之前就失败。
   RT-01 担心「改动之前失败」无法由 0.2.0 保证，这个担忧由此化解（RT 写报告时没看到 CI 的断点分析）。
3. 契约有明确的地基顺序：
   - 错误码体系（K1）是 HTTP 错误体、IPC、`--json`、i18n、UI、skill 的共同输入，必须最先定。
   - CLI 形状与 settings 形状决定 i18n 翻译什么、skill 写什么。
   - IPC 快照、事件与本地访问控制决定 UI 能做什么。
4. 结构重构是契约落地的前置条件。proxy.rs 有三份「判定→计数→终态」拷贝（S1）；控制面有四份「spawn + 等就绪」，命令层打印即退出（S2/S3）。不先收拢，K1、K8、i18n、埋点都要做三遍。
   这些纯重构不依赖用户决策，可以和 W1、W2 设计稿并行先做。
5. 审查建议与用户定调冲突的有 14 处，全部按用户定调处理（1.11 节）。综合时另发现 9 处缺口与接缝（1.12 节）。
   其中「W3 语言判定要在解析前读 settings.json」与「W2b C5：`--version` 不读 home」直接冲突，实现前要定好短路顺序。
   UI 日志视图需要的数据源（最近请求的历史、请求体按需读取与保留策略）四份报告都没覆盖完整。

---

## 1 去重合并后的发现清单

合并原则：几份报告说同一件事的合成一条，保留全部原编号；一条原发现跨两个主题时拆到两条（见 1.13 对照表）。
P0 共 **16** 条：A1、A2、A3、K1–K10、SE1、U1、U2。W2b 是计划项、不是审查发现，不计入 P 级，它与各条的接缝见 1.12。

### 1.1 升级断点与版本（W0 / W1）

| 编号 | 级别 | 结论 | 来源 | 工作流 |
|---|---|---|---|---|
| A1 | P0 | 第一个破坏性提交之前，把根包与 Cargo.lock 改成 `0.2.0-alpha.0`。否则开发构建与真 0.1.0 同号不同协议，R1 退役旧看护者、降级判定、status 的版本差异提示全部失灵，作者本机最先踩到。选 `alpha.0` 而不是 `-dev`，是因为它小于任何 `alpha.N` | RT-02 | W0 |
| A2 | P0 | 版本边界两个方向都封死。`--version` 改为 `aproxy v<semver> (…)`，0.1.0 的四条下载渠道与 `--from` 都会在交换前失败。0.2.0 的 probe 接受 `v` 前缀。`INSTALL_FLOOR = 0.2.0` 在四处拒绝，包括续作带进来的 0.1.x 残留；`--from`/`--adopt` 补上降级检查。CI 的 upgrade-from-0-1-0 改为断言拒绝：非零退出、bin 哈希不变、实例仍应答、没有 install.state。必须先于第一个 0.2.0 预发布 tag 合入 | CI-01、CI-06、RT-01（第 1–3 点）、计划 W1、W2b C5 | W1 |
| A3 | P0 | 删除兼容代码：`compat_0_1_0.rs`（顺带消除 CP-28：Windows 旧宣告名里是 BEL 字节，S5 从未生效）、relaying 豁免与 `Relaying` 相位、两份 0.1.0 测试。保留 R1（迁移窗口里残留的 0.1.0 看护者靠它退役）与 `fleet_already_on_target`，只删掉其中引用 0.1.0 的注释 | 计划 W1、CI-20(a)(c)(e)、CP-28 | W1 |
| A4 | P1 | 迁移路径：CHANGELOG、Release 正文、README、INSTALL_AGENT 写迁移步骤（CI §3.6 草稿可直接用）。三个引导脚本加 `--reinstall`/`-Force`；检测到 0.1.x 时打印迁移步骤，而不是只说「已安装，不覆盖」 | RT-01（第 4–5 点）、CI §3.6 | W1 起草、W7 定稿 |
| A5 | P1⚑ | install 状态清理后冻结。加 `failed_at`，修两个问题：`--abort` 拒绝一切 failed；在线来源失败后永远续不了（install-07 未修）。另加 `installer_version`、可选的 `error_code`，未知相位用 `#[serde(other)]` 容忍。只读命令不再拉起续作，续作只在状态可接管时拉起。Windows 交换后的第二次交棒可以省掉。状态位置按 W2b C4 收进 `<home>/install/`。D3 推断的「收敛重启用 staging 镜像导致双重启」在这里实测 | CI-07、CI-03（第 2 点）、CI-20(b)、CI-19、CP-06（第 3 点）、CP §4、W2b C4、D3 | W1 |

### 1.2 对外契约（W2）

| 编号 | 级别 | 结论 | 来源 | 工作流 |
|---|---|---|---|---|
| K1 | P0 | **统一错误码体系：码表单源，在出口渲染。** 数据面 `FailureReason`、控制面 `OpError`、配置 `Diagnostic`、安装错误都给稳定的 snake_case 码加结构化参数。HTTP/SSE、IPC 的 `last_error` 与错误应答、信封 `RetryInfo.reason`、日志 `code=` 字段、install.state、`--json` 共用这一套码，显示端按码本地化。码只增不改，message 不是契约。现状是同一段中文同时进 HTTP 体、SSE、IPC、信封与日志（数据面 39 处、format 24 处混用），UI 无法按原因聚合 | DP-03、DP §6 M1–M7、CP-03（message/details）、CP-08（OpError）、CP-10（last_error）、CP §3.6 码初稿、CI-08（Diagnostic）、CI-12、CI §5.2、RT-16 第 1 点 | W2→W6a/b |
| K2 | P0 | **aProxy 自产的 HTTP 错误体与 SSE 终态事件定形。** 体为 JSON：`{"type":"error","error":{"type":"aproxy_<码>","message":"<英文>","aproxy":{request_id,attempts,upstream_status}}}`，不在 `error.code` 放字符串。`x-aproxy-error`、`x-aproxy-request-id` 两个头只加在自产响应上，上游透传的响应保真不变。SSE 为 `event: error` 加同一 JSON。`aproxy_retry_exhausted` 取代泛名 `upstream_error`，并带上真实状态码。五处手写 JSON 改为单一构造（现在有一处用 `format!` 拼 JSON）。码表包含 501 `aproxy_upgrade_unsupported`、本地存储故障、`aproxy_shutting_down`。至少用真实 SDK 做一次解析验证 | DP-01、DP-02、DP-06、DP-24、DP-27 | W2→W6a |
| K3 | P0·**决策 D-1** | **重试策略的配置形状要在冻结前定：规则表，还是平铺的 `bounded_retry_paths`。** 以下几件迟早都要加，并且要走同一个出口：Retry-After 与抖动（DP-26）；按状态码或错误体限次（X-4：400/401/404 在默认配置下会让 agent 永久等待）；协议无关判定的盲区（DP-25：`response.failed`、按连接关闭分帧的截断流）。默认仍是无限重试，不内置任何 URL 或协议特例 | DP-04、DP-25、DP-26、X-4、上一轮路线图 0.1.x #6/#7 | W2→W6a |
| K4 | P0 | **IPC v1 定稿**（v1 没随任何正式版发布，改它不用升版本号）：<br>- 未知参数回 `unsupported_arg`，类型错回 `bad_request`。不这样做，将来带条件的 `shutdown` 发到 0.2.0 实例上会变成无条件停止。<br>- result 按 op 定型。<br>- message 固定英文，另加 `details`。<br>- 用 `features` 声明能力。<br>- 改状态的 op 要幂等或带条件。<br>- `prepare_swap{ttl_secs}` 到期自动回到 serving。<br>- 预留 home 级端点名，流式 op 的帧格式现在定下。<br>- 状态快照新增可选字段：`in_flight`、`retrying`、`mode`、`last_error{code,details,message,at}`、看护者状态。<br>- 一切 URL 与密钥在实例侧打码。用户 10-11 定「IPC 永不返回明文密钥」，现有快照样例里的 `base_url` 也要打码。<br>- 用 golden JSON 契约测试钉住 | CP-03、CP-10、CP-25、CP §3.1–3.9、DP §8、计划 W5「安全边界」 | W2→W6b |
| K5 | P0 | **持久层定稿。**<br>- home 是唯一的根：删 `APROXY_RUN_DIR`，加全局 `--home`，库 API 一律以 home 为参数。<br>- 布局与命名一次理顺：`run/` 权限 0700；记录带 kind 后缀与 `schema`；临时文件统一为 `*.tmp-<pid>`；`process_start` 统一命名，按 W2b C3 的启动标记推导（Linux 带 boot_id）；磁盘类型与线上类型拆开；unix 心跳从 `/dev/shm` 挪到 `run/`；安装状态与宣告按 W2b C4 收进 `<home>/install/`。<br>- 读者规则：未知字段忽略；看不懂或 schema 更高的记录跳过并报告，绝不删除；只清理「本 schema、且写者经身份核验已死」的记录；claim 读不懂时按 mtime 判新鲜。<br>- 用 `tests/contract_v0_2.rs` 取代 `compat_v0_1_0`。<br>W2b 说 C1–C8 以外的东西可以随意演进，这个说法成立的前提，正是 0.2.0 的读者不破坏自己看不懂的东西 | CP-01、CP-05、CP-15、CP-16、CP-22、CP §4/§5、CI-20(d)、W2b C3/C4、tests-12 | W2→W6b |
| K6 | P0 | **进程拉起与期望状态。**<br>- `.restore` 改为带 schema 的结构化记录（config_path、overrides、log_path），任何版本的拉起方都把记录翻译成自己的命令行再 spawn；父进程把同一结构写成规格文件交给子进程。<br>- 内部模式改为隐藏的内部子命令，不再寄生在用户语法的全局旗标与 `install` 下面。<br>- 看护者的命令行只带内部入口与 `--home`。现在它复制启动者的全部参数，包括 `--api-key`。<br>- 看护者、restore、restart、install 四处重放共用一个 `spawn_from_record`。<br>- 读不懂的记录报告出来，不删。<br>`--api-key` 与环境变量存不存进记录 → **D-2** | CP-02、CP-13（环境部分）、CP-14、CI-02、CI-03、security-09 | W2→W6b |
| K7 | P0 | **配置写回保真。**<br>- config.toml：用 `toml_edit` 只改目标键，原子写。现在是整份重新序列化、先截断再写，会丢注释和未知键，还把默认值固化进文件。<br>- settings.json：可调字段改成 `Option` + `skip_serializing_if`；用 `#[serde(flatten)]` 保留未知字段；加 `schema`，遇到更高的 schema 拒绝写入。<br>- CAS 或锁、逐字段容错是 P1 子项（UI 与 CLI 会并发写） | CI-04、CI-10、CP-01（第 3 点）、ui-01/02、config-04 | W2→W6b |
| K8 | P0 | **机读面。**<br>- 定一张退出码表，取值见 **D-3**，必须避开 W2b C7 冻结的 10/11/12。<br>- status、stop、restart、restore、start、alias list、find、doctor、config show 加带 `schema` 的 `--json`，错误为 `{"error":{"code","details"}}`，不随语言变化。<br>- CLI 进程的 tracing 改到 stderr。现在 INFO 行混进 stdout，会破坏 JSON。<br>- install 用 NDJSON 输出进度，加 `--check`；后台续作写 `logs/install.log`。<br>i18n 一上线，agent 按中文原文匹配的做法就失效，所以这条必须和 W3 同一个版本到位 | CP-04、CP §8、CI-13、daemon-ipc-06/S01、ui-S04 | W2→W6b |
| K9 | P0·**决策 D-4** | `config` 写入面：是否用通用的 `config show/get/set/unset`（以及同构的 `settings` 命令）取代 15 个专用旗标。这些旗标只覆盖约三分之一的字段，名字与键名也对不上。机密用 `-` 从 stdin 读 | CI-05、CI-14（第 4 点）、docs-12、D1 | W2→W6b |
| K10 | P0 | **信封 0.2.0 定稿，并且先于 envelope 0.2.0 发布。**<br>- 输入加 `v`。<br>- 往返序号 `seq` 由 format 回显，根治 transform.rs 自述的串包残余窗口。<br>- 响应侧补上 `status`。<br>- `RetryInfo.reason` 给机读原因（与 K1 同码），`error` 改英文；`EnvelopeError` 也改英文。<br>- 把输出侧「缺省 = 不变」写成契约。现在只想换个 key 的 format 也得把整个 body 原样写回。<br>- `#[non_exhaustive]` 加赋值式构造。<br>- `worker_id` 的文档语义与实现一致。<br>TODO 里的「format 扩展开放问题」（state 的大小上限、心跳信封缺上游失败摘要）一并定。DP 原评 P1；因为发布即冻结，而且它决定发版顺序（RT-06），合并后升为 P0 | DP-13、RT-06、DP §6 M6/M7 | W2→W6a |
| K11 | P1⚑·**决策 D-7** | CLI 形状：<br>- 启动覆盖参数归到 `start` 下；`--config` 只挂在需要它的子命令上；`--log-file` 只在 start。<br>- `upgrade` 改为 `visible_alias`；`--pre` 与 VERSION 互斥；`stop idle --older-than`；非 idle 目标带阈值时报用法错误；多行帮助不再被折成一行。<br>- `desktop` 与 `skill` 子命令放在哪。headless 版也要认得 `desktop`，说明后以非零退出。<br>裸 `aproxy` 的语义 → D-7 | CI-17、CP-27、D4、daemon-ipc-10、计划 W4/W5 | W2→W6b |
| K12 | P1⚑·**决策 D-5** | settings.json 的形状与分层。现在混装了三类东西：程序状态、全局策略、7 个字段的全局默认；其余可调字段没有全局默认层。`idle_timeout_secs` 与转换器的同名字段含义不同。0.2.0 还要加 `language` 与 UI 偏好 | CI-11、计划 W3/W5 | W2→W6b |
| K13 | P1⚑ | `read_timeout_secs` 同时卡住首字节等待：长时间的非流式生成每次尝试都会确定性超时，然后被无限重试。拆成 `first_byte_timeout_secs` 与 `read_idle_timeout_secs`，实现改为 `timeout(send())` 加 `timeout(chunk())`；首字节超时单独给一个码。默认值见 3.12 | DP-07 | W2→W6a |
| K14 | P1⚑ | 「0」与单位的语义。`spool_limit_mb = 0` 现在被当成 1 MiB，与其余字段「0 = 不限」相反，改为不限。`*_mb` 实际按 MiB 计算：文档如实写，不改名 | DP-14、CI-11（第 3 点） | W2→W6a |

### 1.3 结构（W6a / W6b）

| 编号 | 级别 | 结论 | 来源 | 工作流 |
|---|---|---|---|---|
| S1 | P1（顺序前置） | proxy.rs 有 3522 行生产代码，分三步：<br>1. 拆成 `src/proxy/` 下约 15 个模块，纯搬移、行为零变化、测试冻结。<br>2. 抽出纯函数 `classify` 并补单测。<br>3. 合并成单一的 attempt 循环：两种 Responder（Buffered / Keepalive），首轮快速路径就是第 1 次迭代。<br>三份拷贝的文案已经开始分叉；K1、K2、O1、O2 和数据面 i18n 都要改这三处 | DP-10、DP §7 | W6a |
| S2 | P1 | 控制面拆分：<br>- 进程身份等平台原语移出 watchdog.rs；新增 `home.rs`、`fsx.rs`（四份原子写合一）。<br>- `ipc/`、`registry/`、`platform/`、`watchdog/` 分模块。<br>- `spawn.rs`：四份「spawn + 等就绪」合一，start 因此获得早退检测与 startup.log 摘录，端口 0 也会等就绪。<br>- `serve_forever` 迁入 lib；bin 里带单测的模块迁入 lib，`[[bin]] test = false`（这也是 U1 的前提） | CP-17、CP-20、CP §6、RT-05（第 2 点） | W6b |
| S3 | P1（顺序前置） | 操作层：`ops::{enumerate,start,stop,restart,restore,alias_*}` 返回 `Result<Outcome, OpError>`，lib 不打印、不 `process::exit`。CLI 文本（i18n）、`--json`、GUI 三个前端共用它。UI 编进本体后，GUI 在自己的进程里调用这些操作，不能被它们 exit 掉 | CP-08、CI-12（第 2 点）、CI-19（第 2 点） | W6b |
| S4 | P2 | config.rs 拆分；新建 `redact.rs`，作为唯一的打码出口，UI 也能用。新建 `paths.rs`，收拢三处路径不一致：两个语义不同的 `~` 展开器、三种默认配置路径来源、两种大小写规则不同的路径比较。doctor 与 find 的目录扫描去重，下载代理的归一逻辑去重。与 C9 一起做 | CI-18、CI-14（第 3 点） | W6b |

### 1.4 正确性（W6c）

| 编号 | 级别 | 结论 | 来源 |
|---|---|---|---|
| C1 | P1 | 读路径不带副作用：<br>- 拆出只读的并发枚举 `registry::enumerate(home)`，返回四态（Live / Unresponsive / Incompatible / Dead）加 Unreadable，不删任何文件。<br>- `--force` 只对 Unresponsive 建议。现在协议不兼容与冒名应答也被当成挂死，并建议强杀。<br>- 清理移到守护启动时或看护者的周期 GC，加宽限期，只处理内置随机名的日志。现在自定义 `log_file` 若放在 logs/ 下，会被删或在启动时被截断。<br>- 守护补写丢失的 `.pid`。<br>- install 续作只由变更类命令、在状态可接管时拉起。<br>UI 秒级轮询会把「status 删掉正在启动的实例的日志」从偶发变成常态 | CP-06、CP-07、CP-24、CP-26、CI-19、ui-04/S10、daemon-ipc-04/S04 |
| C2 | P1 | 看护者鲁棒性：<br>- 实例存活满 60 秒才清零失败计数。现在就绪后再崩溃会形成零退避的紧循环，每轮还新建一个日志文件。<br>- 重拉复用早退检测。<br>- 补种前不再先删 claim，否则真挂死的前任会永久残留。<br>- 挂死阈值与 claim 新鲜阈值设下限。<br>- 看护者日志写到 `logs/watchdog.log`（现在全部丢失），处决、重拉、放弃、删记录四类事件必写。<br>- 配置暂时不可达时不删 `.restore`。<br>- 开关与调优每个 tick 重读，关闭即退出并保留 `.restore`。<br>连续失败之后怎么办 → **D-8** | CP-11、CP-18、watchdog-02/06/07/08/09/S03/S06 |
| C3 | P1 | 守护脱离启动者：<br>- unix：加 `process_group(0)` + `setsid()`。推断现在在启动终端按 Ctrl+C，会把守护与看护者一起优雅停掉，而且不会自愈；Linux CI 补一个进程组 SIGINT 用例。<br>- Windows：先带 `CREATE_BREAKAWAY_FROM_JOB` 调用，遇拒绝再去掉。推断现在守护会随「关闭即杀」的作业对象一起被杀 | CP-12、CP-23、daemon-ipc-11 |
| C4 | P1⚑ | 守护的工作目录固定为 home，配置里相对路径的解析基准写死。现在它取决于「谁拉起的」，skill 已把这种不确定写成对外行为；推断 Windows 上常驻进程还会占住启动时所在的目录 | CP-13（cwd 部分）、watchdog-05 |
| C5 | P1 | spool 按进程分目录：清理移到 bind 与控制端点都成功之后，只清 pid 已死的目录。现在按配置端口共享目录并且在 bind 之前清空：端口 0 的实例会互删；同端口的第二个守护在退出前就删光原实例的在途文件。spool 文件打开失败或提前 EOF 一律报错，现在却以空 body「成功」回放 | DP-08、DP-09、CP-16（spool 部分） |
| C6 | P1 | 上游 URL 非法或为相对路径（format 改写出来的）：校验转换输出必须是 http/https；`is_builder()` 归为终态 `aproxy_bad_upstream_url`。现在被当成网络错误无限重试，客户端在心跳里永远等下去 | DP-05 |
| C7 | P1 | Upgrade/WebSocket 请求：入站校验后识别出来，回 501 `aproxy_upgrade_unsupported`，不进重试。现在它被降级成普通 GET，在 4xx 上无限重试；如果 Codex 默认改走 WebSocket，经 aProxy 就会挂死 | DP-06、proxy-core-07 |
| C8 | P1 | 动态心跳任务改持 `WeakSender`。现在回放结束后，客户端的流要等在途的心跳转换返回才结束；`timeout_secs = 0` 时可能永远不结束（这个功能还没随正式版发布） | DP-15 |
| C9 | P1 | 配置解析合成一条管线：`config::resolve(path, overrides, settings) -> {config, sources, diagnostics}`，start、doctor、`config show --effective`、find、UI 共用。<br>- validate 补上头名、头值、api_key、listen_addr 的检查。<br>- doctor 深检默认配置，有 error 时非零退出，报出行列。<br>- 别名指向缺失文件时报准确的错误。<br>- 未知键告警并给出近似建议，不报错（含 0.1.0 残留的 `upstream_url`）。<br>- settings 逐字段容错，未知下载渠道跳过。现在一个字段写错就整份回退默认，`watchdog:false` 会悄悄变回 true | CI-08、CI-09、CI-10、config-03/05/06/10/11/12 |
| C10 | P2 | 停止语义：收到 shutdown 立即删 `.restore` 再排空。否则排空期间若崩溃，看护者会把用户刚停的实例拉回来。已提交的保活流先写一个 `aproxy_shutting_down` 终态事件，未提交的回 503。`stop all` 怎么处理 `.restore` 见 3.12 | CP-21、DP-27、install-09 |
| C11 | P2 | 转换器的边角问题：<br>- every_attempt 换了请求以后，心跳源同步更新（DP-20）。<br>- 「未提交 + 2xx 非 SSE」分支的响应转换也经 drive 驱动（DP-21）。<br>- 响应解码失败时跳过转换、原样透传（DP-22）。<br>- 客户端压缩上传的请求体先解出再判定与转换（DP-23）。<br>- aproxy-format 按 `stage` 分派（DP-16）。现在把它配成 heartbeat_transform 会扰乱 key 轮换。<br>- 转换往返改为读写并发（DP-29）。现在边读边写的 format 在 body 超过管道缓冲时会死锁，直到超时 | DP-16、DP-20–DP-23、DP-29、transform-06/11 |
| C12 | P2 | 本地磁盘故障统一报 `aproxy_local_storage_failed` 并记进 last_error；现在请求体写盘失败回 400，而且不记录。`ForwardResult` 的注释，以及 docs/architecture.md「绝不退化成内存堆积」的说法，以代码实际行为为准改写 | DP-24、proxy-core-08/11、perf-07 |
| C13 | P2 | 小项：<br>- 下游连接没开 `tcp_nodelay`；`is_hop_header` 每次都分配一次；头部逐个转换；HEAD/304 的 content-length 被改写；有过期的测试注释；心跳转换的默认超时 30 秒大于默认保活间隔 15 秒（DP-28）。<br>- 错误信息不对症，随 i18n 重写文案时修（CI-21）。<br>- 代码异味（CI-24） | DP-28、CI-21、CI-24、perf-03、proxy-core-06 |
| PF1 | P2 | 错误扫描：解析前先用 memchr 预过滤；内存模式不再扫两遍；超过 256 KiB 的解码放进 `spawn_blocking`；扫描器改成游标扫描（现在单个 chunk 内是 O(n²)），长度上限只施加在还没成行的尾巴上 | DP-17、DP-18、proxy-core-10、perf-04 |
| PF2 | P2 | 磁盘 spool 的写句柄外包 `BufWriter`，回放改用 `BytesMut` 读 | DP-19、perf-09 |

### 1.5 可观测（W6d）

| 编号 | 级别 | 结论 | 来源 |
|---|---|---|---|
| O1 | P1 | 请求标识：handler 入口建 tracing span（path 脱敏），保活任务继承它。`request_id` 改为跨重启可区分（例如 `<启动时刻 hex>-<序号>`），日志、信封、事件和 `x-aproxy-request-id` 用同一个值。现在并发请求的重试日志无法归因到请求 | DP-11 |
| O2 | P1（W5b 前置） | 在途登记：<br>- `RequestRegistry` + RAII 的 `InFlightGuard`。保活任务与仅转发模式的流，都要把 guard move 进去。<br>- 按原因分别计数。<br>- 用 `broadcast` 发事件，没有订阅者时连事件对象都不构造。<br>- idle 判定改为「在途数为 0，且距上次请求结束超过阈值」。现在会把正在长时间单次尝试的实例判为闲置，`stop idle` 可能把它停掉。<br>事件集与帧格式按 K4。0.2.0 的 UI 日志视图依赖它（见 G1） | DP-12、DP §8、CP-10、daemon-ipc-05、ui-S02/S06 |

### 1.6 安全（W6e）

| 编号 | 级别 | 结论 | 来源 |
|---|---|---|---|
| SE1 | **P0**（原 P1；用户 10-11 定「安全边界在实例与 IPC」，据此上调） | 本地访问控制：<br>- Windows 命名管道用显式 SDDL：当前用户 SID + SYSTEM，首个实例与后续实例都用。<br>- unix：run 目录 0700，socket 0600，服务端与客户端双向核对 `peer_cred` 的 uid。<br>- config、settings、`.restore`、日志一律 0600；doctor 对权限过宽的文件告警。<br>0.2.0 的 UI 要经 IPC 读日志、按需读取请求体，不能再依赖平台默认值：命名管道的默认 DACL 给 Everyone 读权限；umask 为 002 的系统上，同组用户能连上 socket 并停掉实例 | CP-09、CI-14（第 1 点）、daemon-ipc-08、security-07/08、ui-06 |
| SE2 | P1 | 凭据出口：<br>- validate 产出类型化错误，并在源头打码。现在 doctor 会原样输出带凭据的 proxy URL 与带 `?key=` 的 base_url。<br>- `redact` 成为唯一的打码出口。<br>- 短密钥整段打码；现在不超过 6 个字符的值原样显示。<br>- 机密从 stdin 读，安装脚本的收尾提示不再教人把 key 写在命令行上 | CI-14（第 2–4 点）、CI-22、docs-02 |
| SE3 | P1 | 下载渠道校验：binstall 取 GitHub 资产时复用其 `.sha256` 做强校验；cargo 渠道比对 crates.io API 给出的 checksum。模块文档如实描述，现在是零校验却写着有校验 | CI-16、security-10、install-13 |

### 1.7 i18n（W3）

| 编号 | 级别 | 结论 | 来源 |
|---|---|---|---|
| I1 | P1（计划 W3 的基础设施） | - 语言判定：`APROXY_LANG` > settings 的 `language` > 系统界面语言 > en。守护、看护者、续作进程不需要语言。<br>- 消息类型化（K1 的码 + 参数），在输出边界渲染；库不打印。<br>- clap 帮助在运行时注入：clap 条目上的 `///` 改成 `//`，自定义 help/version 参数与 `help_template`，用 `try_parse` 本地化常见的解析错误；元变量与 `--version` 不本地化。<br>- 写一个遍历整个命令树的完整性测试：en 下帮助里没有 CJK，zh-CN 下不回落到英文。<br>- 语言显式传参或用线程局部覆盖，不用进程级 OnceLock 缓存（集成测试同进程并发，会互相污染）。<br>- 目录完整性测试，以及专门的 zh-CN 覆盖用例 | CI-12、CI §5.3、RT-16、计划 W3 |
| I2 | P1 | 字符串迁移按模块、随重构一起做，不翻译要删掉的代码：<br>- 数据面 167 处、aproxy-format 32 处（其中日志 87 处改英文）。<br>- 控制面约 144 条用户文案、121 行帮助、66 条日志与 panic 文案。<br>- 配置与安装约 400 处，其中 169 处是混用。<br>- 测试里约 116 处按中文子串的断言，改为断言错误码、`--json` 或英文 | DP-30、DP §6、CP §7、CI §5.1、RT-16、RT 附表 C、CI-21 |
| I3 | P2 | 引导脚本（install.sh 111 行、install.ps1 74 行中文，install.cmd 是英文）与 npm 转发器 `aproxy.js` 里 30 行中文错误消息，纳入同一条语言判定规则。单文件脚本不能依赖外部目录，内联两套文案 | RT-11（第 3 点）、RT-24、CI §5.1 |

### 1.8 skill（W4）

| 编号 | 级别 | 结论 | 来源 |
|---|---|---|---|
| SK1 | P1（W4 前置） | skills 包只从一个来源打包：同一个脚本供 release.yml 与 npm 组包调用，在 zip 根放 `manifest.json`（版本 = tag、每个文件的 sha256、文件数上限）。`skill show` 只认清单里登记的路径；`skill list` 用清单里的版本判断 skill 与二进制是否同版本。现在有两条生成路径、没有清单，两份 zip 的哈希还不同 | RT-21、计划 W4 |
| SK2 | P1 | skill 下载支线交给接手者（目标版本）执行。现在是旧安装者去下载、解包目标版本的 skill 包，属于「旧驱动新」的残留，等于把 skill 包格式变成了跨版本契约 | CI-15 |
| SK3 | P1 | skill 内容：<br>- 237 行引用的中文程序输出原文，改为引用英文目录文本与错误码；这是重写，不是翻译。<br>- 教 agent 用 `--json`、退出码与 `APROXY_LANG=en`。<br>- 校验脚本现在放在 gitignore 的本地目录里，转正进 CI：反引号里引用的文本必须在英文目录中逐字存在；compatibility.md 的版本行与 Cargo.toml 一致。<br>- 码表与英文化必须和 skill 同一次提交，否则 skill 立刻与程序脱节。<br>references 是否留存 0.1.0 版 → **D-9** | DP-01（M8）、DP-30、CP-04、RT-17、docs-08 |

### 1.9 桌面与双变体发布矩阵（W5a / W5b）

| 编号 | 级别 | 结论 | 来源 |
|---|---|---|---|
| U1 | P0 | Tauri 不能进 lib，也不能进任何测试 harness。原因：Windows 上 Tauri 经 muda/rfd 静态导入 comctl32 的 `TaskDialogIndirect`，而 `tauri-build` 只给 bin 嵌 Common-Controls v6 清单；lib 的单测与集成测试 exe 一启动就以 `0xc0000139` 退出（tauri#13419）。做法：<br>- `src/ui/**` 只由 main.rs 声明，lib 里只放不依赖 Tauri 的 view-model。<br>- bin 的单测迁入 lib，`[[bin]] test = false`。<br>- 根包 `build.rs` 只在 feature 开启时调用 tauri-build。<br>- 不另建 aproxy-ui crate：多一个包就多一次首发与可信发布绑定。<br>- `custom-protocol` feature 是否必需，由 spike 确认 | RT-05 |
| U2 | P0 | 带 UI 版在缺库机器上的可行性与变体选择。用户已定：所有平台都出两版，带 UI 的沿用现名，无 UI 的叫 headless。<br>风险与约束：<br>- Linux 带 UI 版动态链接 webkit2gtk-4.1/GTK，缺库时连 `--version` 都跑不起来，代理与看护者全部起不来。<br>- gnu 带 UI 版不能沿用 zigbuild 的 2.28 方案，要在 ubuntu-22.04（及其 arm runner）上构建，并单独断言 glibc 下限。<br>- musl 带 UI 版要在 Alpine 里动态链接，可行性由 spike 先验证。<br>因此每个零配置入口都要有选择策略：<br>- 一键脚本按库探测，缺库就选 headless。<br>- `aproxy install` 升级时保持当前变体，`--version` 报出变体；W2b 的强制安装在第 1 步核对，并提示 `--variant headless`。<br>- npm 转发器在子包二进制加载失败时，提示改装 `@meowo/aproxy-headless`。<br>- Windows 缺 WebView2 运行时是否影响守护启动，由 spike 实测 | RT-04、RT-03（风险部分）、RT-20（风险部分）、CP-19、计划 W5 待定、W2b C8 第 1 步 |
| U3 | P1 | 资产清单只维护一份：内部清单（target × variant × cpu × runner × glibc 下限 × rc 时是否必需）生成 workflow 矩阵、W2b C6 的 `aproxy-manifest.json`、install 用的数据（`include_str!`）、脚本与 npm 组包需要的映射，并配断言测试。现在资产名在 7 处各写一份。install 的「release 已齐」判据按自身变体检查。`-v3` 的选择改为看清单，并补全 BMI1/2、FMA、LZCNT、MOVBE 检测；现在只看 AVX2，而 v3 资产只有两个 target | RT-03（单源部分）、RT-08（v3 部分）、release-08/13、W2b C6 |
| U4 | P1 | CI 双矩阵与守卫：<br>- headless 门禁：`--no-default-features`，不装 GTK 与 Node，是核心使命的主门禁。<br>- desktop 门禁：装 apt 依赖与 Node，ubuntu 用 22.04。<br>- 依赖隔离守卫：`cargo tree --no-default-features` 的输出里不得出现 tauri/wry/tao/webkit2gtk/gtk/muda/tray-icon/window-vibrancy；加 headless 体积预算（0.1.0 基线：windows x64 6.51 MiB、linux gnu 6.11 MiB）。<br>- 加 MSRV job；发布门禁钉工具链版本，另设 stable 金丝雀。<br>- deny.toml 逐条登记 gtk-rs GTK3 绑定的 unmaintained 通告（RUSTSEC-2024-0411…0420）。<br>- rust-cache 按变体分 key；dependabot 加 npm 生态，并给 tauri 一族分组 | RT-07、release-16 |
| U5 | P1 | 发布矩阵 22 格：desktop 格放进独立 job，失败不连坐 headless 产物。rc 时容忍 desktop 格缺失并在正文列出，正式版必须全齐。凡能在 runner 上原生运行的格，都跑 `--version`，再起一个隔离 home 的守护做 ping/stop 冒烟；现在只有 zigbuild 目标跑 `--version` | RT-08、release-13 |
| U6 | P1 | GUI 与控制面共存：<br>- GUI 拉起实例一律用 `<home>/bin/aproxy`，不存在时引导用户 install。否则从 npm 全局目录或下载目录启动的 GUI，拉起的实例日后会被 install 拒绝管辖。<br>- install 换版后提示 GUI 重启。<br>- GUI 发起强制安装时，先把它拉成独立进程再退出自己（W2b）。<br>- 在 desktop 版下实测守护与看护者的 RSS 与冷启动时间；看门狗「约 2–3 MB」的设计前提可能不再成立 | CP-19、RT D.2、W2b |
| U7 | P1 | 渠道语义（按用户的命名）：<br>- crates.io 的 `aproxy` 默认 feature 含 UI，所以包里要带构建好的前端产物，并更新 `include`（发布 job 用 `cargo package --list` 对照清单）；README 写清源码构建的前置依赖（Node、GTK）。<br>- `aproxy install` 的 cargo 渠道按当前变体决定是否加 `--no-default-features`。<br>- binstall 默认取带 UI 版。<br>- README 的渠道表加一列变体 | RT-20（按用户命名改写） |
| U8 | P1 | 前端供应链与 UI 安全面：`package-lock.json` 入库，CI 用 `npm ci --ignore-scripts`；webview 用严格 CSP 禁止一切外部网络（用户 10-11）；SECURITY.md 加 UI、CSP、IPC 访问控制一节；为 Rust 与前端依赖生成第三方许可证清单 | RT-23（UI 部分）、计划 W5 |

### 1.10 发布、测试、文档（W0 / W6f / W7）

| 编号 | 级别 | 结论 | 来源 |
|---|---|---|---|
| R1 | P1 | 建 `tests/common`：<br>- `TestHome`：tempdir 加 `{"watchdog": false}`；`cmd()` 统一注入 `APROXY_HOME`、`APROXY_LANG=en`，并清掉代理环境变量；Drop 时只按端口执行 `stop`，禁止 `stop all`。<br>- 守卫合一（现在 3 套）；端口优先用 `:0`（现在 2 套派生算法）；format-echo 改用稳定的定位方式。<br>- 之后再把 7091 行的 proxy_integration.rs 按域拆成 5–6 个文件。<br>i18n 的断言迁移以它为前提，否则要逐个改 65 处 `Command::new` | RT-15、tests-11、TODO 里测试基建三条 |
| R2a | P1 | 删掉没有任何引用的 `tower`、`tower-http`，以及与正式依赖重复的 dev-deps。删除不改变依赖图 | RT-12、DP-28、CI-23、perf-13、security-11 |
| R2b | P1 | 趁 0.2.0 窗口做大版本升级：axum 0.8、reqwest 0.13、toml 1.x、zip 9、sha2 0.11、dirs 7，每个单独提交、全量测试。处置悬着的 #6–#10：#10 base64 要看 envelope 0.2.0 的公开 API 是否暴露 base64 类型再定。dependabot 加分组 | RT-12、release-09 |
| R3 | P1 | CI 恒红：最近 30 次运行没有一次整体成功（macOS 28/28 失败；现在是 6 个 lib 测试失败，注释还写 3 个）。处理方式见 3.12 | RT-13、TODO「CI 现状处理」 |
| R4 | P1 | 发版流程：新增 `docs/RELEASING.md`，写明版本号位置清单、CHANGELOG 节、skill 留存、新 crate 首发前置、tag 规则（禁止 `tN` 这类测试 tag）、发布后核验与补发流程。0.2.0 走 rc 演练。兄弟 crate 的变更必须在 tag 之前 bump。npm 的 `next` 现在落后于 `latest` | RT-14、RT-06 |
| R5 | P1 | Release 正文从 CHANGELOG 抽对应版本节，找不到就让 job 失败；现在 Release 页没有变更内容。CHANGELOG 的 0.2.0 节按「破坏性变更 / 迁移 / 新增 / 变更 / 修复」五段重写，删掉写给 0.1.x 补丁线读者的兼容叙述 | RT-10、RT-18 |
| R6 | P1 | 引导脚本：URL 改为取随 Release 发布的资产。现在取自 main 分支，开发期间对脚本的改动会先影响新用户。CI 加 shellcheck、PSScriptAnalyzer 与三平台冒烟；Release 列表分页。与 A4、U2、I3 的脚本改动合并做 | RT-11 |
| R7 | P1 | workflow 去重：抽出 `_build.yml`（`workflow_call`）。publish job 必须留在顶层文件里，因为可信发布按 workflow 文件名匹配 | RT-09 |
| R8 | P1 | 文档同步：<br>- README 双语：迁移节、变体列、`skill`/`desktop`/`APROXY_LANG`、`language` 与 `ui` 配置、`every_attempt` 示例。<br>- architecture.md：版本路线仍写「0.2.0 引入 UI…属远期」，CI 节要改，compat 叙述要删。<br>- INSTALL_AGENT。<br>- SECURITY：改为 0.2.x，加 UI 威胁面 | RT 附表 E |
| R9 | P2 | 测试覆盖：read/connect 超时、HTTPS 与 HTTP/SOCKS 上游、下载渠道的离线 mock。经现有的 url 模板通道注入，不为此加生产环境变量 | RT-22、tests-06/07/08 |
| R10 | P2 | 供应链：action 钉 SHA、build provenance、给 `v*`/`format-v*` 加 tag ruleset；Windows 代码签名推迟决策 | RT-23、release-10 |
| R11 | P2 | 文档补全：CONTRIBUTING、人类版排障页、systemd `--user`/launchd、两个子 crate 的 README、aproxy-format 的 binstall 元数据（它的 tag 是 `format-v*`，默认模板会 404） | RT-24 |
| H1 | P2 | 把源码控制字节扫描加进 CI。CP-28 的成因就是写入工具把 `\a` 写成了 BEL | CP-28 |

### 1.11 与用户定调冲突的建议及处置

| # | 审查建议 | 冲突的定调 | 处置 |
|---|---|---|---|
| 1 | RT-03：无后缀资产保持 headless，UI 版加 `-ui` 后缀 | 用户 10-11 选定 headless 命名：带 UI 的沿用现名，无 UI 的加 `-headless` | 按用户。RT-03 担心零配置渠道拿到需要 GTK 的二进制，这个风险改由 U2 的选择策略承担 |
| 2 | RT-03：UI 版只出 baseline，不与 `-v3` 组合 | 用户：「现有每个二进制都翻倍」（22 个） | 按用户 |
| 3 | RT-03/RT-04：musl 与 aarch64-musl 永远只出 headless | 用户：「musl 与 aarch64 Linux 也不例外」 | 按用户。RT 依据的是计划的旧版本。musl 带 UI 版的可行性由 W5a spike 先验证，不可行再回报用户 |
| 4 | RT-20：npm 与 binstall 一律 headless；cargo 的 default features 不含 UI | 用户：`@meowo/aproxy` 是带 UI 版；「默认内置」 | 按用户。U7 记下必须配套的几件事 |
| 5 | RT-01 (b)：0.2.x 再保留 S1–S3 一个 minor | 用户：去掉全部兼容 | 不采纳；由 A2 的门解决 |
| 6 | RT-01 第 2 点：保留 upgrade-from-0-1-0，改成一次性的「结局实测」 | 主代理已定改为断言拒绝 | 改为断言拒绝。「不设门」的对照推演（CI §7.1）可以在 CI 里跑一次留证，不作为长期任务 |
| 7 | RT-03：不要动 `--version` 第一行 | CI-01 与 W2b C5 都要改它 | 按 C5 改。0.2.0 及以后的 probe 接受 `v` 前缀，RT 的顾虑随之消失 |
| 8 | CI-11：settings 的 `defaults` 与 config.toml 完全同构 | 用户 09-23：transform 配置只在 toml 层；用户 09-22：`log_file` 不进 settings | `defaults` 排除三个 transform 与 `log_file`（写进 D-5 的推荐方案） |
| 9 | CI D2：删掉顶层 `--api-key` | 这个旗标是用户要求接上的（记忆 baseurl-rename） | 列入 D-2，由用户定 |
| 10 | CI D1：`--baseurl` 改为 `--base-url` | 用户要求的命名 | 列入 D-4 的子问题 |
| 11 | CI D3：去掉 PrepareSwap 标志与收敛重启 | 铁律 3「ACK 齐了才交换」是用户定调（plan/install-v1.md） | 不问、不改；按 CP-25 加 `ttl_secs` |
| 12 | DP X-1/X-2：调整请求侧转换失败的语义 | 用户 09-23 的决策 #1（记忆 transform-format） | 列入 D-6 |
| 13 | DP-04：用规则表取代 `bounded_retry_paths` | `bounded_retry_paths` 是用户定向后的设计（记忆 bounded-retry-paths） | 列入 D-1 |
| 14 | DP-07：首字节超时默认取 0 或取一个大值 | 综合原则「默认行为不变」 | 由主代理自定：默认值不变（理由见 3.12） |

审查推翻的是代理自己写下的东西，不涉及用户定调，由主代理自定：
- CP §4 的永久接口清单、`.restore` 存 argv（CP-02）。
- daemon-model 记忆里「有意跳过 CREATE_BREAKAWAY_FROM_JOB」（CP-23）、TODO「已结案」里的管道 ACL 理由（CP-09/CP-29）。
- skill 里写的「关掉看门狗不停已在运行的看护者」（CP-18）。
- CP §4 原拟冻结现有的隐藏旗标，CI-03 主张改为内部子命令：采纳 CI-03。

### 1.12 综合时发现的缺口与接缝

- **G1 UI 日志的数据源（四份报告都没覆盖完整）**。用户要的日志视图是「一条一条的结构化条目，能深入查看请求体」，计划写「请求体只在点开某条日志时按需取」。
  - 现有数据只有两样：英文文本日志（K1 之后带 `code=`），以及 O2 的事件（只含在途请求）。
  - 还缺三样：UI 打开之前发生的最近请求的历史；请求体的保留策略（spool 文件在请求结束时就删了）；请求头的打码。
  - 建议 W2 设计稿补一节：实例在内存里保留最近请求的结构化记录，条数与总字节都有上限、可配置；`subscribe` 的首行带上这份历史。请求体经新 op 按需读取，只返回仍在保留窗口内的，头部在实例侧打码。默认不在磁盘上长期保留请求体。这些 op 只在 SE1 落地之后开放（CP §3.4 第 6 条）。文本日志仍是 agent 读的权威日志。
- **G2 W2b C5 与 W3 语言判定冲突**。C5 要求 `--version` 不读写 home；I1 却要在 clap 构建命令之前读 settings.json 的 `language` 来本地化帮助。
  - 现状：`--version` 在 `Cli::parse()` 内就退出了，之前不碰 home（已核对 `src/main.rs`）。
  - W3 之后必须保住这一点：argv 里出现 `--version`/`-V` 时先短路；或者遇到版本请求时，语言判定只看环境变量、不读 settings。
- **G3 台账（C3）与注册记录（`run/<port>.pid`）大量重合**：pid、启动标记、版本、端口都有。
  - 建议两者分开。台账保持最小且冻结，只服务强制安装，以及回答「这个 home 里有哪些 aproxy 进程」。注册记录可以演进，服务 status 与 UI。
  - 启动标记的推导统一用 C3 的写法，顺带解决 CP-22。枚举时用台账补出看护者、安装者、桌面进程，CP-10 要的看护者状态就有了来源。
  - 也可以把注册记录并成台账的可选字段，但那样这些字段就永远不能删、不能改名，与 W2b「冻结得越少越好」的原则相悖。
- **G4 计划内部表述不一致**。计划的「目标」说发布后 IPC、磁盘布局、配置、CLI、错误体、信封都要长期稳定；W2b 却把 IPC、注册表、`.restore`、配置文件列为「不冻结、可随意演进」。
  - 建议统一为：C1–C8 绝对冻结；其余在 0.2.0 定稿后只增不改，确实需要破坏时由强制安装兜底。
- **G5 退出码区间**。W2b C7 已冻结 `install --force` 的退出码 10/11/12，D-3 的通用退出码表只用 0–5 来错开。
- **G6 `--version` 括号里不得出现形如版本号的裸 token**。
  - 0.1.0 的 `probe_version` 取输出里第一个以数字开头的词。如果括号里恰好出现一个等于目标版本的 token，自证就会通过，门随之失效。
  - 这条要写进 C5，并补一个测试钉住。
- **G7 cargo feature 名不一致**。计划 W5 构建一节、TODO、记忆 0.2.0-direction 写的是 `ui`；计划命名一节写的是 `desktop`；W2b C6/C7 的变体词是 `desktop`。建议统一为 `desktop`（见 3.12）。
- **G8 强制安装的测试安全**。C8 会按台账结束一个 home 下的全部 aproxy 进程；测试一旦回落到默认 home，就会结束用户的生产实例并切断 agent 会话（记忆 never-kill-aproxy-by-name、protect-production-instances）。强制安装相关测试的入口要断言 `APROXY_HOME` 指向临时目录，否则直接失败。
- **G9 扩展「写操作需用户同意」该放在哪一层**。GUI 在自己的进程里调用操作层（CP §3.1），所以同意门要放在 Tauri command 桥接层：停止/重启、改配置、执行安装都经过它。IPC 本身不区分调用者。W5b 设计时写清。

### 1.13 原编号 → 合并编号

- **DP**：01→K2　02→K2　03→K1　04→K3　05→C6　06→C7（码在 K2）　07→K13　08→C5　09→C5　10→S1　11→O1　12→O2　13→K10　14→K14　15→C8
  16→C11　17→PF1　18→PF1　19→PF2　20–23→C11　24→C12　25→K3　26→K3　27→C10　28→C13/R2a　29→C11　30→I2；X-1/X-2→D-6　X-3→3.12　X-4→K3
- **CP**：01→K5/K7　02→K6　03→K4/K1　04→K8　05→K5　06→C1/A5　07→C1　08→S3　09→SE1　10→K4/O2　11→C2　12→C3　13→C4/K6　14→K6　15→K5
  16→K5/C5　17→S2　18→C2　19→U6/U2　20→S2　21→C10　22→K5　23→C3　24→C1　25→K4　26→C1　27→K11　28→A3/H1　29→第 5 节
- **CI**：01→A2　02→K6　03→K6/A5　04→K7　05→K9　06→A2　07→A5　08→C9/K1　09→C9　10→K7/C9　11→K12　12→I1/K1/S3　13→K8　14→SE1/SE2/K9
  15→SK2　16→SE3　17→K11　18→S4　19→C1/A5/S3　20→A3/A5/K5　21→I2/C13　22→SE2　23→R2a　24→C13；D1→D-4　D2→D-2　D3→3.12　D4→D-7
- **RT**：01→A2/A4　02→A1　03→U2/U3　04→U2　05→U1　06→K10/R4　07→U4　08→U3/U5　09→R7　10→R5　11→R6/I3　12→R2a/R2b　13→R3　14→R4　15→R1
  16→I1/I2　17→SK3/D-9　18→R5　19→第 5 节　20→U7　21→SK1　22→R9　23→U8/R10　24→R11/I3　25→第 5 节

---

## 2 工作流归属、依赖与执行顺序

### 2.1 工作流定义（新增 W0、W5a，W6 拆成 a–f）

| 工作流 | 内容 | 合并条目 |
|---|---|---|
| **W0 准备**（新增） | 版本号、删无用依赖、处理 CI 恒红、控制字节扫描；U4 里的 MSRV job 与工具链钉版也可以并进来 | A1、R2a、R3、H1 |
| **W1 去兼容与升级断点** | 删兼容代码、版本边界、install 状态清理、迁移说明草稿 | A2、A3、A4（起草）、A5 |
| **W2 契约定稿** | 主代理写设计稿，用户整体过目、回答决策；然后分轨实现 | K1–K14 |
| **W2b 强制安装与进程台账**（计划项） | C1–C8。台账在 alpha.1 落地，强制安装在 alpha.2 | 与 K5、A4、U2、U3、D-3 有接缝（G2–G5、G8） |
| **W3 i18n** | 基础设施，然后按模块迁移 | I1–I3 |
| **W4 `aproxy skill`** | 命令本身（计划 W4）与 skill 打包、支线、内容 | SK1–SK3 |
| **W5a 桌面可行性 spike**（新增） | 先验证 U1、U2 的全部可行性问题，并测体积与 RSS；不依赖原型 | U1、U2 |
| **W5b 桌面接入与双变体发布** | 用户审过原型之后接入 | U1–U8、R7、O2 的 UI 侧、G1、G9 |
| **W6a 数据面结构** | | S1 |
| **W6b 控制面与配置结构** | | S2、S3、S4 |
| **W6c 正确性** | | C1–C13、PF1–PF2 |
| **W6d 可观测** | | O1、O2 |
| **W6e 安全** | | SE1–SE3 |
| **W6f 测试基建** | | R1、R9 |
| **W7 发布** | | R4–R8、R10、R11、A4 定稿、记录收尾（R2b 提前到 Phase 2） |

### 2.2 必须先定稿的契约（地基顺序）

后面的 i18n、skill、UI 都建在这些契约上。先定的放前面；括号里是依赖它的东西：

1. **K1 错误码体系**（K2 的 `error.type`、K4 的 `last_error`、K8 的 `--json`、K10 的 `reason`、I1/I2 的渲染、SK3 的排障表、W5b 的着色与聚合）。码只能先定、后翻译。
2. **CLI 面**：K8 退出码与 `--json`、K9 `config`/`settings` 命令、K11 形状、K6 内部入口、`--home`、`desktop` 与 `skill` 的位置、W2b C5 的 `--version`（I1 要翻译的帮助目录、SK3、W4 的 `--help` 末尾提示行）。先定稿，才不会翻译随后要删的旗标。
3. **K12 settings 形状**，含 `language` 与 `ui` 段（I1 的语言来源、W5b 的设置页、K9 的 `settings` 命令）。
4. **K4 IPC，加上 SE1 的访问控制规则与 G1 的日志/请求体 op**（W5b 与 O2 的全部数据）。
5. **K5 持久层，加上 W2b C1–C4**（台账实现、W4 的 skills 目录、SE1 的权限对象）。
6. **K2 与 K10**（SK3 里写给 format 作者的协议、aproxy-format 的同步）。
7. **K3、K13、K14 数据面配置语义**（UI 的配置编辑器、skill 的配置文档）。

建议把这些写成一份 W2 设计稿（例如放在 `.agents/plan/`，实现后公开部分移入 `docs/`），一次性交给用户整体过目。逐项发问既慢又容易前后不一致。

### 2.3 proxy.rs 拆分与重试循环统一排在哪

- **放在 Phase 1，W0 一合入就开工**，与 W1、W2 设计稿并行。它不依赖任何用户决策。
- **必须在以下工作之前完成**：任何 K1/K2/K3/K13/K14 的数据面实现、C5–C8、O1/O2、数据面 i18n。否则同样的改动要在三份拷贝里各做一遍（DP-10）。
- **顺序**：纯搬移（测试冻结；用独立 target 目录；汇报时附测试数，见记忆 shared-target-dir-stale-tests）→ 抽 `classify` 并单测 → 合并成单一循环。
- **reqwest/axum 大版本升级（R2b）放在循环合并之后、K1 数据面实现之前**。K1 要按 reqwest 的错误 API（`is_builder`/`is_connect`/`is_timeout`）分类，先升级再写分类，以免返工。拆分期间不升级，避免冲突。
- **控制面同理**：S2 排在 W1 合入之后，因为 W1 改 watchdog/daemon/install；在 K4/K5/K6/C1–C4 之前。S3 排在 S2 之后、K8 与 I1 之前。

### 2.4 依赖关系

| 先 | 后 | 原因 |
|---|---|---|
| W0 | 一切 | 版本号与依赖基线（A1 不先做，dev 构建与 0.1.0 同号） |
| A2 的门 | 第一个 `v0.2.0-*` tag | 0.1.0 用户加 `--pre` 或写明版本号就能装到预发布 |
| W1 | S2、R1 的 install 相关测试 | 同一批文件 |
| S1 | K1/K2/K3/K13/K14 的数据面实现、C5–C8、O1、O2、数据面 I2 | 三份拷贝 |
| S2 | K4、K5、K6、C1–C4 | 平台原语、spawn 合一 |
| S3 | K8、I1 的 CLI 渲染、W5b | 结构化结局，lib 不打印 |
| W2 设计稿过目、D-* 有答复 | 全部 K 的实现 | 契约先定 |
| K1 | K2、K4、K8、I1、SK3、W5b | 码是一切渲染的输入 |
| W2b 获确认 | 台账写入（alpha.1）→ 强制安装（alpha.2）→ C6 发布清单（W5b/rc） | C3 要求所有长期进程在 0.2.0 发布前写台账 |
| K5 布局 | W2b C3/C4 落地、W4 的 skills 路径、SE1 | 文件放在哪 |
| K4 + SE1 + O2 + G1 | W5b 的日志视图与请求体查看 | 安全边界在实例与 IPC |
| R1 | I2 的测试断言迁移 | `APROXY_LANG=en` 统一注入 |
| R2b（toml） | K7 | toml_edit 的版本 |
| SK1 | W4 的 `skill show` 白名单 | 清单是白名单的来源 |
| W5a spike | W5b、U2 定案、D-10 | 可行性 |
| 用户审过原型 | W5b | 用户定的审核方式 |

### 2.5 并行编排（总并发 ≤5，含主代理）

原型轨（ui-prototype）在用户审核之前占一个名额，所以 Phase 1 实际只能开 3 个轨。按记忆 workflow-concurrency-budget：非关键阶段用 sonnet，effort 不默认 max。

| 阶段 | 轨（同时最多 4 个代理 + 主代理） | 进入条件 |
|---|---|---|
| **Phase 0** | 主代理独做 W0（小，先合入）；同时起草 W2 设计稿与 D-1…D-11 的提问稿 | 立即 |
| **Phase 1** | ① W1（opus）② S1：拆分 → classify → 合循环（opus，测试冻结）③ W5a spike（独立 worktree；Windows 本机加 CI/WSL；不合入，或只合入审过的骨架）＋ 原型轨；主代理：W2 设计稿、用户决策、W2b 确认 | W0 合入 |
| **Phase 2** | ① S2 第 1–2 步，加上 bin 单测迁入 lib ② S1 收尾后做 R2b：reqwest/axum 在前，toml/zip/sha2/dirs 随后，全部在 Phase 3 前合入 ③ R1：只动 helper、不动断言；S1 合入后再按域拆分测试文件 ④ 原型轨空出来之后：W2b 台账写入（以 W2b 获确认为前提） | W1 合入 |
| **Phase 3**（alpha.2 主体） | **数据面**：K1 码表单源（先合入骨架，其他轨往里加码）→ K2、K3、K13、K14、C5–C8、C12、O1。**控制面**：S2 第 3 步 → K4、K5、K6、C1–C4、C10。**配置与 CLI**：K7、C9 + S4、K9、K12、SE2 → S3 → K8、K11。**安装与格式**：W2b 强制安装 C1–C8 → K10 信封、aproxy-format、C11、SE3 | W2 设计稿过目、D-* 有答复、S1/S2 合入 |
| **Phase 4**（alpha.3） | ① I1 → I2 分批（sonnet 按模块，不超过名额）② W4 命令 + SK1 + SK2 ③ SE1 + O2 + G1 的 op ④ SK3（码表与英文目录稳定后），再写 R5/R8 的草稿 | Phase 3 的契约合入 |
| **Phase 5**（alpha.4） | ① W5b 接入（opus）② U3 + U4 + U5 + R7（CI 与发布）③ U7 + R6 + I3（渠道与脚本）④ U8 + R4 | 用户审过原型、Phase 4 合入、spike 结论可行 |
| **Phase 6** | W7：rc.N → 0.2.0 | 见第 4 节 |

### 2.6 执行纪律（各轨都要遵守）

- 测试与实测一律用隔离的 `APROXY_HOME`，停实例只按端口，绝不 `stop all`、绝不按名字结束进程。用户自己的会话就经 aproxy 转发（记忆 never-kill-aproxy-by-name、protect-production-instances）。强制安装相关测试额外加 G8 的守卫。
- 改了 unix 路径，先等 ubuntu job 的结论再往上叠改动；本机不能编译 Linux（记忆 ci-unix-blindspot）。macOS job 按 3.12 的方式处理。
- 每个 worktree 用独立的 target 目录，汇报时附测试数，防止测到旧代码（记忆 shared-target-dir-stale-tests）。
- 改了 CLI、配置、行为或文案，skill 必须在同一提交、或紧随其后的提交里同步（记忆 aproxy-cli-skill-versioning）。K1、I2、SK3 尤其要成批一起提交。
- 零警告纪律：clippy `-D warnings`，headless 与 desktop 两个变体都要过（记忆 warning-zero-policy）。

---

## 3 需要用户拍板的决策

只列产品语义、或推翻已有用户定调的事项。主代理可以自己定的，放在 3.12。

### D-1 重试策略的配置形状（K3）
- **背景**：现在唯一的出口是 `bounded_retry_paths`：整体锚定的路径正则（含查询串）匹配后最多尝试 3 次，这是用户 09-20 定向的设计。状态码判定写死；退避不读 Retry-After，也没有抖动。按状态码或错误体限次（X-4）、协议盲区（DP-25）迟早都需要同一个出口。
- **选项**：
  - A 规则表：`[retry]` 下放 `max_backoff_secs`、`jitter`、`respect_retry_after`；`[[retry.rules]]` 每条可写 `path`（沿用整体锚定、含查询串）、`status`、`body`、`max_attempts`；删掉 `bounded_retry_paths`。
  - B 与 A 同形，但保留 `bounded_retry_paths` 作为展开成规则的单行写法。
  - C 保持平铺：把 `bounded_retry_paths` 定为永久原语，现在就写清将来的 `retry_rules` 与它并存时的优先级。
- **推荐 A**：一个机制覆盖路径、状态码、错误体三类出口，不会形成两套并行机制。默认仍是无限重试，不内置任何 URL 或协议特例（符合记忆 features-are-config）。0.2.0 先实现 path、status 与退避三项，`body` 等字段的形状先定下来，0.2.x 再加性实现。B 的语法糖等于永久多一种写法，收益很小。settings 里是否也有这一层，跟随 D-5。
- **为何在 0.2.0 定**：配置键随 0.2.0 冻结。如果先把 `bounded_retry_paths` 作为稳定键发出去，以后加规则只能两套并行，或者再做一次破坏性迁移。

### D-2 期望状态记录要保存哪些启动时的状态（K6）
- **背景**：`.restore` 改为结构化记录由主代理定，剩下的两件事属于产品语义。
  - 顶层 `--api-key` 是用户要求接上的旗标。现在它明文写进 `.restore`，也出现在守护与看护者的命令行里，帮助却写「仅本次运行生效」。
  - 代理环境变量（reqwest 默认读取 `HTTP(S)_PROXY` 等）与 PATH 取决于「谁拉起的」：终端启动、看护者重拉、GUI 拉起、开机 restore 之间可能各不相同。
- **选项**：
  - key：a 不保存——崩溃重拉或 restore 之后回落到配置文件里的 key，status 标注出来；b 存进记录（文件 0600、目录 0700），永不进命令行，重拉后保持同一把 key；c 删掉顶层 `--api-key`，key 只放在配置里，用 `config set api_key -` 从 stdin 写入。
  - 环境：i 启动时把代理相关变量与 PATH 快照进记录，重拉时还原（具体变量清单在 W2 设计稿里定）；ii 不快照，文档写「要稳定，请在 config.toml 里写 proxy」。
- **推荐 b + i**：
  - 使命要求重拉出来的实例与原实例行为一致。a 会让看护者重拉出一个没有 key 的实例，请求 401 之后无限重试，agent 挂住。
  - c 收回了用户要的功能。b 的存放强度与 config.toml 里的明文 key 相同（SE1 落地后同为 0600）。
  - i 的道理相同：保证网络路径一致；记录里的代理 URL 在 status 中打码。
- **为何在 0.2.0 定**：记录格式、帮助文案的语义与重拉行为都随 0.2.0 定型；明文 key 出现在命令行里是现存的安全问题。

### D-3 退出码表（K8）
- **背景**：现在多为 0/1，而且不一致：status 恒为 0；restore 失败也退 0；doctor 有错误也退 0；stop 遇到未运行的目标时，指定端口或别名退 1，`all`/`idle` 退 0。skill 已把这些写成对外文档。i18n 之后文本随语言变化，agent 只能靠退出码与 `--json`。W2b C7 已为 `install --force` 冻结了 10/11/12。
- **选项**：
  - A（控制面稿）：0 成功或目标状态已达成；1 操作失败；2 用法错误（含未知别名、多实例时未指定目标）；3 部分成功；4 目标未运行。
  - B（配置稿）：0 成功；1 一般失败；2 用法错误；3 配置无效；4 目标不存在；5 部分失败。
  - C（合并稿）：0 成功或已达成；1 操作失败；2 用法错误；3 配置无效；4 目标未运行或不存在；5 部分成功；10–12 留给 `install --force`。
  - 子问题：`stop` 遇到本来就没在运行的目标，返回 0（幂等）还是 4。
- **推荐 C，`stop` 幂等返回 0**（`--json` 里 outcome 为 `already_stopped`，细节不丢）：配置无效单独成码，agent 就知道该改配置而不是重试；幂等停止让脚本与 agent 不必区分「已停」和「刚停」；与 C7 的区间不冲突。
- **为何在 0.2.0 定**：退出码一公布就冻结；i18n 同一版本上线后，agent 不再有稳定的文本接口。

### D-4 `config` 命令面，以及 `--baseurl` 的命名（K9）
- **背景**：15 个专用旗标只覆盖 26 个配置键中的约三分之一，旗标名与键名对不上（`--baseurl`↔`base_url`、`--listen`↔`listen_addr`）。settings.json 的 20 个字段几乎没有命令可设。UI 与 agent 都需要一个通用后端；每个专用旗标都要单独翻译、单独校验。
- **选项**：
  - A 用 `config show [--effective] [--json] / get / set / unset` 取代：键就是文件里的键，表用点号，`-` 表示从 stdin 读机密；settings 同构为 `settings get/set/unset`。删掉专用旗标，不留别名。
  - B 保留专用旗标，另加 get/set，两套面长期并存、都要翻译。
  - C 维持专用旗标，按字段逐个补。
  - 子问题（D1）：`start` 的覆盖旗标是否改为由键名机械推出（`--base-url`、`--listen-addr`）。这意味着把用户当初点名的 `--baseurl` 改掉。
- **推荐 A**。子问题倾向改名：与 `--api-key`、`--log-file` 一致，帮助、文档、UI 映射都不必为它破例。但这是用户当初点名的写法，由用户定。
- **为何在 0.2.0 定**：CLI 随 0.2.0 冻结；否则 i18n 要先翻译一批随后就删的旗标。

### D-5 settings.json 的形状与分层（K12）
- **背景**：settings.json 里混着三类东西：程序状态（别名、default_config、config_dirs）、全局策略（日志轮转、闲置阈值、看门狗、下载、skill 自动更新）、7 个 config.toml 字段的全局默认。其余可调字段没有全局默认层。`idle_timeout_secs` 与转换器的同名字段含义不同。0.2.0 还要加 `language` 与 UI 偏好（Mica、主题、吉祥物）。
- **选项**：
  - A 重整：`{schema, language, ui:{…}, aliases, default_config, config_dirs, defaults:{…}, watchdog:{…}, logs:{rotate_mb}, instances:{idle_threshold_secs}, update:{download_chain, download_proxy, skill_auto_update}}`。`defaults` 与 config.toml 同构，但排除三个 transform 与 `log_file`（尊重用户 09-23、09-22 的定调）。优先级统一为「CLI > toml > settings.defaults > 内置」，UI 能标出每个字段来自哪一层。
  - B 保持平铺：只给 `idle_timeout_secs` 改名，加 `language` 与 `ui`，全局默认层以后逐字段补。
  - C 同 A，但 `defaults` 也纳入 transform 与 `log_file`，需要推翻两条既有定调。
- **推荐 A**：这是最后一次低成本重整的机会；`settings get/set`（D-4）与 UI 设置页都可以按节组织；不触碰两条既有定调，配置两层制（用户定）也不变。
- **为何在 0.2.0 定**：settings.json 是同一台机器上所有版本共写的一份文件，以后再重整就得写双读代码。

### D-6 请求侧转换失败的语义（X-1 / X-2）
- **背景**：用户 09-23 定「请求侧转换失败 → 502，不重试」。every_attempt 落地后出现了不对称：
  - 首次转换遇到进程层的瞬时故障（Spawn/Io/WorkerDied/TimedOut）时，请求直接 502 终止；重试前的转换遇到同类故障，却被容忍并沿用上一次的请求。
  - 502 会触发客户端 SDK 自己的重试（推断 Anthropic/OpenAI SDK 默认 2 次），所以「不重试」只在 aProxy 内部成立。而 Rejected（例如「model 未命中任何渠道」）属于请求本身的确定性问题。
- **选项**：A 维持现状；B 进程层瞬时故障进入正常的重试循环（带退避、计入重试数），Rejected 与 Protocol 仍然 502 终止；C 在 B 的基础上，Rejected 改回 4xx（400 或 422，码 `aproxy_transform_rejected`）。
- **推荐 C**：瞬时故障不应该结束 agent 的这一轮（使命）；确定性拒绝用 4xx，SDK 才不会白白重试。用户原决策的核心——不把转换失败的请求发给上游——不变。
- **为何在 0.2.0 定**：状态码与终态事件属于 K2 冻结的 HTTP 契约。

### D-7 裸 `aproxy` 的语义与启动参数的位置（K11 / D4 / CI-17）
- **背景**：裸 `aproxy` 现在等于在后台启动代理。agent 或 UI 为了看帮助而误敲 `aproxy`，就会起一个守护。为它服务的覆盖参数（`--baseurl/--listen/--proxy/--api-key/--foreground`）挂在顶层；`--config` 与 `--log-file` 是全局参数，在无关的子命令上被接受后静默忽略。
- **选项**：A 裸 `aproxy` 输出帮助（或 status），启动只走 `aproxy start`，覆盖参数归到 `start` 下面；B 维持裸 `aproxy` 即启动、覆盖参数留在顶层，只修全局参数泄漏。
- **推荐 A**：agent 是一等用户，误启动守护是实际存在的风险；启动入口只有一个，帮助与 i18n 的面也更小。
- **为何在 0.2.0 定**：CLI 随 0.2.0 冻结。

### D-8 看护者连续失败之后怎么办（C2 / CP-11）
- **背景**：重拉在约一分钟内连续失败到上限，看护者就永久放弃，只在 startup.log 留一行。配置所在的网络盘开机后挂载得晚、端口被临时占用，这类本可恢复的原因也会让实例一直停着。
- **选项**：A 维持永久放弃；B 转入慢速重试（例如每 300 秒一次），直到用户 stop 或 `.restore` 被移除，每次尝试写进 watchdog.log；C 同 B，但设总时限（例如 24 小时）。
- **推荐 B**：符合「不间断」的使命；代价只是每 5 分钟一次廉价的拉起尝试，而且有日志可查。
- **为何在 0.2.0 定**：C2 正好在重写这段逻辑，行为也写在 skill 里。不定也可以放到 0.2.x，但一起定成本最低。

### D-9 skill references 是否留存 0.1.0 版（SK3 / RT-17）
- **背景**：用户定的规则是：大版本更替时，把 `references/latest/` 复制为 `references/<旧版本>/` 留存。但 v0.1.0 tag 里的 skill 是中文旧版，现在的 latest 描述的是 0.1.0 之后的开发线。字面执行，归档下来的东西既不是 0.1.0 发布时的内容，也不描述 0.1.0 的行为。
- **选项**：A 这次不留存，`compatibility.md` 保留 0.1.0 → 0.2.0 的差异表与迁移步骤；B 用 `git show v0.1.0:` 原样留存中文旧版；C 照 v0.1.0 的代码重写一份英文的 0.1.0 references。
- **推荐 A**：0.1.0 用户必须重装到 0.2.0，SECURITY 也只对最新版提供修复；旧行为文档只在迁移期有用，差异表已经覆盖。规则本身不变，0.2 → 0.3 照常执行。
- **为何在 0.2.0 定**：留存动作就发生在 0.2.0 发布时。

### D-10 预发布节奏（W7）
- **背景**：发布到 GitHub prerelease、npm `next`、crates.io 都不可撤回，版本号会被永久占用。`--version` 门必须先于第一个预发布。带 UI 的变体要到 W5b 才完整。headless 的 10 个 npm 包，要等第一次 CI 发布才能验证可信发布的绑定。
- **选项**：
  - A alpha 只作为内部里程碑（不打 `v*` tag、不发布）。第一个公开预发布是 rc.1，前提是 22 个资产齐全、门已合入、信封已定稿；之后 rc.N → 0.2.0。
  - B 每个 alpha 都公开发布到三个渠道。
  - C alpha 只发 GitHub prerelease，不发 npm 与 crates.io。
- **推荐 A**：不发布半成品的变体，不浪费版本号。门的拒绝测试、spike、双矩阵 CI 都可以在 CI 里完成；渠道实测集中在 rc 阶段做，与 0.1.0 当年的 rc.1 演练一致。
- **为何在 0.2.0 定**：它决定 envelope 的版本策略（RT-06），也决定 W5b 的发布矩阵何时必须就绪。

### D-11 确认 W2b（强制安装 C1–C8），连同综合发现的四处修正
- **背景**：计划里 W2b 标着「设计稿，待用户确认」，而 C3 要求台账在 alpha.1 就由所有长期进程写入，所以要先于其他实现确认。综合发现四处需要随之修正，都由主代理改，不必另问：
  - G2：`--version`/`-V` 先于语言判定短路。
  - G5：通用退出码只用 0–5。
  - G4：统一「绝对冻结 / 只增不改」两层表述。
  - G6：括号里不得出现形如版本号的裸 token。
  G3（台账与注册记录分开）也一并告诉用户。
- **选项**：A 按现稿确认，并采纳上述修正；B 修改 C1–C8 中的某些项。
- **推荐 A**。

### 3.12 主代理可以自行决定的设计点（建议的决定）

| 设计点 | 建议的决定 | 依据 |
|---|---|---|
| cargo feature 名 | `desktop`；同时修正计划 W5 构建一节、TODO、记忆 0.2.0-direction 里的 `ui` | G7；与 `aproxy desktop` 命令、W2b 的变体词一致 |
| `--version` | 按 W2b C5：`aproxy v<semver> (<variant> <target>)`；不新增 `version --json`（以后可以加性加）；遵守 G2、G6 | CI-01、RT-03、CP §4 |
| 错误码命名 | snake_case，全局唯一。HTTP/SSE 的 `error.type` 用 `aproxy_<码>`；IPC、日志、信封、`--json` 用裸码。码只增不改，message 不是契约。码表放在一个单源文件里，由它生成 docs 与 skill 的附表 | DP-01、DP-03、CP §3.6 |
| HTTP 错误体与响应头 | 按 K2。`error.code` 不放字符串：Gemini 的 `error.code` 是整数，强类型客户端会反序列化失败（推断）。`x-aproxy-*` 头只加在自产响应上 | DP-01 |
| 超时默认值 | 拆分后默认值不变：首字节 300、读空闲 300。首字节超时单独成码，文档写清长时间非流式生成该怎么调。不改成 0：aProxy 自己的接入指引让 Claude Code、Codex 关掉了客户端的空闲超时，保活路径上首字节超时是挂死上游的唯一出口 | DP-07；记忆 claude-code-stream-watchdogs、codex-stream-idle-timeout；「默认行为不变」 |
| `spool_limit_mb = 0` | 改为不限；文档写 MiB，不改名 | DP-14 |
| 内部入口名 | `__serve --spec <文件>`、`__watchdog --home <目录>`、`__install-resume [--handover-from <pid>]`；同步 skill 里的「Processes you may see」 | CI-03（不采纳 CP §4 冻结现有旗标的意见） |
| 台账与注册记录 | 分开（G3） | W2b 的「冻结得越少越好」 |
| 磁盘布局 | 按 CP §5 平铺加 kind 后缀；采纳 W2b 的 `ledger/` 与 `install/`；宣告改用 `install/lock` 的持锁状态 | CP §5、CI-20(d)、W2b C4 |
| `.restore` 结构 | `{schema, config_path, overrides{listen,base_url,proxy,log_file}, log_path}`，再加 D-2 决定的字段；四处共用 `spawn_from_record` | CP-02、CI-02 |
| IPC 细节 | 采纳 CP §3.4 的全部规则。保留 `prepare_swap` 并加 `ttl_secs`，维持用户定的铁律 3，不做 D3 的简化。`subscribe`、`logs` 与请求体按需读取在 0.2.0 实现（0.2.0 的 UI 需要），只在 SE1 落地后开放。不经 IPC 写配置 | CP §3、CP-25、D3、G1 |
| keepalive 系的键名 | 在 W2 设计稿的键表里一次理顺，看是否收进 `[keepalive]` 表；随设计稿给用户过目 | DP-04 末段 |
| 看护者开关 | 每个 tick 重读，关闭即退出并保留 `.restore`；skill 里的旧描述同步改 | CP-18 |
| `stop all` | 期望状态变为「全部停止」：同时清掉本 home 下所有 `.restore`，包括排队待重拉的。shutdown 一收到就删 `.restore` | CP-21 |
| 守护的工作目录与相对路径 | spawn 时工作目录固定为 home。配置里的相对路径（transform 的 `command`/`args`、`log_file`）统一相对 home 解析，与 `log_file` 现行语义一致；对默认配置来说，home 就是配置文件所在的目录。CHANGELOG 写明 | CP-13 |
| 守护脱离启动者 | unix 用 `setsid`；Windows 先试 BREAKAWAY，被拒再回退。daemon-model 记忆里「有意跳过」的理由被「先试后退」化解 | CP-12、CP-23 |
| 下载渠道 | 不删 binstall 与 cargo 渠道，补上校验。删渠道会让写了该渠道的 settings 整份失效（C9） | CI-16 |
| 变体选择 | 首次安装：一键脚本按库探测。升级：保持当前变体（编译期已知），`--variant` 可覆盖。npm 转发器在加载失败时提示改装 headless 包。cargo 渠道按变体传 `--no-default-features` | U2、U7 |
| crate 布局 | 按 U1；不建 aproxy-ui，也不建 aproxy-ipc——IPC 做成模块加契约测试 | RT-05、CP §6、RT D.3 |
| 资产清单 | 内部清单单源生成 C6 的 `aproxy-manifest.json`；rc 时容忍 desktop 格缺失，正式版 22 格全齐 | U3、U5 |
| 信封细节 | DP-13 的八项全做。`worker_id` 实现为分配最小的空闲槽位，契约为「0..pool_max，在世 worker 之间唯一」。预发布期间 envelope 用 `0.2.0-alpha.N/rc.N` 并由对齐步骤同步，正式版才发 0.2.0 | DP-13、RT-06 |
| 错误响应交给 format 转换（X-3） | 0.2.0 不做；以后加一个可选开关属于加性变更 | DP X-3 |
| i18n 机制 | W3 开工时定。倾向「每种语言一个目录文件，构建期生成带类型参数的函数」，前端共用同一套目录——同一个错误码在 CLI 与 GUI 里的译文必须一致。不用进程级 OnceLock | 计划 W3、CI §5.3、RT-16 |
| macOS CI | 按 RT-13 (a)：`--no-default-features` 下的 build 与 clippy 必须绿；test 设 `continue-on-error`，并把固定的失败清单写进 job summary，数量变化即告警。原 TODO 标「待用户定」；这个方案不减少覆盖，执行后告知用户即可 | RT-13 |
| `config get` 遇到密钥字段 | 默认打码输出，与「IPC 不返回明文密钥、密钥只写不读」一致；写入用 `config set <键> -` | 计划 W5「安全边界」、SE2 |
| 配置热重载 | 推迟到 0.2.x（加性的 IPC op）；0.2.0 的 UI 改配置走「写文件 + restart」 | CP §3.5、TODO 中的 H 项 |
| Windows 代码签名 | 不在 0.2.0 做；rc 阶段观察主渠道（脚本、npm、install）是否触发 SmartScreen，再议 | RT-23、RT D.2 |
| 已移除的 0.1.0 配置键 | 不写兼容读取；C9 的未知键告警附带「已移除/已改名」的提示（例如 `upstream_url`，以及 D-1 选 A 时的 `bounded_retry_paths`） | CI-09；去兼容的定调 |

---

## 4 里程碑草案

如果 D-10 选 A，alpha.1–4 只是内部里程碑的名字：不打 `v*` tag，Cargo.toml 保持 `0.2.0-alpha.0`，每个里程碑的提交号记进计划。实际版本号由 rc 的 tag 对齐步骤写入。

### alpha.1　去兼容、版本断点、契约设计定稿、结构先行
- **范围**：W0；W1（A2、A3、A5，A4 起草）；W2 设计稿（用户整体过目，D-1…D-11 有答复）；W2b 获确认，台账由实例（后台与前台）、看护者、续作中的安装者写入；S1 全部三步；S2 第 1–2 步；R1；R3；W5a spike 的结论。
- **验收**：
  - `src/compat_0_1_0.rs`、`tests/compat_v0_1_0.rs`、`tests/upgrade_from_0_1_0.rs` 已删除；`Relaying` 已从相位词表移除；全仓 grep 只剩 R1 与迁移文档里还提到 0.1.0。
  - CI 的拒绝用例（原 upgrade-from-0-1-0）在 windows 与 ubuntu 上为绿：真实 v0.1.0 在隔离 home 里起实例，再执行它的 `install --from <本构建>`，断言非零退出、`bin/` 哈希不变、实例仍能用 0.1.0 协议应答、没有 install.state。可选：用 `download_chain` 的 url 模板指向本地文件服务，覆盖在线路径。
  - `aproxy --version` 的输出符合 C5，并有单测断言括号里没有形如版本号的裸 token（G6）；0.2.0 的 probe 接受 `v` 前缀，测试替身已同步。
  - `INSTALL_FLOOR` 的拒绝用例覆盖四处：带 `--allow-downgrade` 时、`--from`、`--adopt`、续作遇到 0.1.x 残留状态。
  - install.state 的 `failed_at` 有测试：交换前失败可以 `--abort`；在线来源失败后能续作或给出准确的提示。
  - 每种长期进程的台账条目有测试：启动时写、退出时删、启动标记推导符合 C3（Linux 带 boot_id）；`tests/force_contract.rs` 已建骨架，冻结的样本入库。
  - S1：`src/proxy/` 按模块拆开，只剩一个 attempt 循环，`classify` 有单测；拆分前后测试数一致（用独立的 target 目录核对）。
  - S2：平台原语已移出 watchdog.rs；start、restart、restore、看护者共用一份「spawn + 等就绪」，start 有早退检测。
  - 测试里所有 CLI 子进程都经 `TestHome::cmd()` 拉起，统一带 `APROXY_HOME` 与 `APROXY_LANG=en`。
  - CI：windows 与 ubuntu 全绿；macOS 的 build/clippy 为绿，test 的固定失败清单写进 job summary。
  - spike 结论已回报，至少包括：Windows 上带 desktop feature 的 `cargo test --workspace` 能起来；ubuntu-22.04 上带 UI 版的 glibc 下限；musl 带 UI 版能否构建；desktop 构建下守护的 RSS 与冷启动时间；Linux 缺 webkit2gtk 时与 Windows 缺 WebView2 时，守护分别是什么表现。

### alpha.2　契约落地
- **范围**：K1–K14；W2b 强制安装 C1–C8；C1–C10；O1；SE2；SE3；K10 与 aproxy-format 的同步；R2b。
- **验收**：
  - 码表只有一个来源；所有自产的 4xx/5xx 响应体都是 JSON，带 `aproxy_` 类型与两个 `x-aproxy-*` 头；SSE 终态事件只经一个构造函数生成。至少用真实的 Anthropic 与 OpenAI SDK 各做一次解析验证，并记录结果。
  - IPC 的 golden JSON 契约测试覆盖每个 op 的请求、成功应答、每种错误应答，以及状态快照；未知参数回 `unsupported_arg`；`base_url` 带 `?key=` 时，快照里是打码后的值。
  - `tests/contract_v0_2.rs` 钉住全部持久记录的形状。读者容忍测试：放入 schema 更高或无法解析的记录，断言不被删除。settings 写回后未知字段还在；config.toml 写回后注释与未知键还在。
  - 测试断言守护与看护者的命令行里不出现 key；记录里有没有 key 按 D-2 的决定断言。`aproxy status --daemon-child` 不再起服务。
  - `--home` 生效，`APROXY_RUN_DIR` 已删除。
  - 退出码表已实现并写进文档；列出的命令都支持带 `schema` 的 `--json`；有测试断言 stdout 里没有日志行。
  - `config`/`settings` 命令按 D-4、D-5 实现；重试按 D-1 实现，含 Retry-After 与抖动；超时已拆分；`spool_limit_mb = 0` 表示不限。
  - 强制安装：`tests/force_contract.rs` 覆盖台账样本、`--version`、发布清单解析、命令行解析、退出码 10/11/12；端到端测试只在隔离 home 里跑，带 G8 的守卫，按台账结束进程、绝不按名字。
  - C5–C8 各有回归测试：第二个守护启动失败后，原实例的磁盘 spool 请求仍能完成；spool 文件丢失时报错；非法 URL 得到 502 终态而不是挂起；Upgrade 请求得到 501；慢心跳加快速上游时，回放结束后流立即结束。
  - C2：就绪后立即崩溃的实例退避会累加；子进程早退时立即判失败；补种不删 claim 时，假死的前任被处决。C3：ubuntu 上对启动 CLI 的进程组发 SIGINT 后，实例仍在。
  - envelope 的 API 定稿；aproxy-format 回显 `seq`、按 `stage` 分派；envelope 的预发布版本策略已落实。
  - tower 与 tower-http 已删除；每个大版本升级单独一个提交，全量测试通过。

### alpha.3　i18n、`aproxy skill`、安全与可观测
- **范围**：I1、I2；W4（命令本身与 SK1–SK3）；SE1；O2 与 G1 的 op；C11–C13 视余量。
- **验收**：
  - 语言判定顺序符合计划；`--version`/`-V` 不读 settings（G2，有测试）。
  - 遍历整个命令树的测试：en 下帮助里没有 CJK，zh-CN 下不回落到英文；有目录完整性测试；有一组以 `APROXY_LANG=zh-CN` 跑关键命令的覆盖用例。
  - 日志、startup.log、IPC、HTTP 错误体、信封全部是英文（测试抽查这些出口里没有 CJK）；约 116 处中文子串断言已迁移完毕。
  - `skill install/upgrade/list/show` 可用；`show` 只认清单里的路径，有路径穿越、符号链接、junction、ADS、超大文件的测试；所有 `--help` 末尾都有那行提示；`install --skills-only` 已删除。
  - skill 的 references 改为引用英文目录与错误码；校验脚本已进 CI。
  - SE1：Windows 管道带上 SDDL，unix 是 0700/0600，`peer_cred` 双向核对；ubuntu 上有权限测试。
  - O2：长时间的单次尝试不会被判为闲置（有测试）；没有订阅者时事件不构造；用 `examples/loadgen` 记录改动前后的数据。

### alpha.4　桌面界面接入与双变体发布矩阵
- **范围**：用户审过原型之后的 W5b（U1–U8）；C6 发布清单；R7；`aproxy desktop`；自定义标题栏与 Mica（照两个 skill 的做法）；UI 的 i18n；主题、吉祥物、扩展三层；写操作的同意门（G9）；严格 CSP。
- **验收**：
  - CI 的 headless 门禁（`--no-default-features`，不装 GTK 与 Node）与 desktop 门禁（windows + ubuntu-22.04）都为绿。
  - 依赖隔离守卫与 headless 体积预算通过（对照 6.51/6.11 MiB）；Windows 上默认 features 的 `cargo test --workspace` 不再因 comctl32 崩溃。
  - 发布流水线（经 workflow_dispatch 或在测试仓库里演练）产出 22 个资产、`aproxy-manifest.json` 与各自的 `.sha256`；能原生运行的格都通过 `--version` 与隔离守护的冒烟。
  - Linux 带 UI 版的 glibc 下限有断言；一键脚本在缺库机器上选 headless；`aproxy install` 升级保持变体；npm 转发器会给出提示。
  - headless 版执行 `aproxy desktop` 时，说明这个版本没有桌面界面，并以非零退出。
  - UI 只经 IPC 通信、不开新端口；IPC 不出现明文密钥（有测试）；webview 无法访问外部网络。Mica 默认关闭、可以切换；没有 Mica 的 Windows 11 上开关被禁用并说明原因。两个 skill 的验证脚本已运行——运行前先告诉用户，它们会移动鼠标或把窗口置前。
  - desktop 版下守护与看护者的 RSS 与冷启动数据已记录。

### rc.1…rc.N　公开预发布（D-10 选 A 时，这是第一次公开发布）
- **验收**：
  - 22 个资产与发布清单齐全；npm 两组包都发布成功，headless 包第一次 CI 发布即验证了可信发布；crates.io 按 envelope 的版本策略发布；GitHub prerelease 的正文从 CHANGELOG 抽出。
  - 发布过程按 RELEASING.md 执行。
  - 渠道实测：
    - 真实 v0.1.0 在隔离 home 里执行 `install 0.2.0-rc.1`，在交换之前被拒绝。
    - 一键脚本在干净的 Windows、带与不带 webkit 的 Ubuntu、Alpine 上各跑一遍。
    - rc.N 与 rc.N+1 之间双向执行 `install --force`。
  - D-9 的决定已应用；README、architecture、INSTALL_AGENT、SECURITY 已更新。

### 0.2.0
- **验收**：rc 试用期间没有 P0/P1 回归。记忆 ipc-v1-upgrade-contract 重写为 0.2.0 冻结清单：W2b C1–C8 是「绝对冻结」，其余是「只增不改」（G4）。CHANGELOG 的 0.2.0 节按五段写好。npm `latest` 与 GitHub Latest 指向 0.2.0。TODO 已清理，计划已删除，持久的内容已转入记忆。

---

## 5 记录卫生待办

### 5.1 `.agents/TODO.md`（行号为现行文件，491 行）

RT 附表 B 用的是 `c1d4b96` 的行号，下表已换成现行行号（`b751a0f` 重排后，`2c8a713` 又插入了 W2b 一行）。RT 的第 21 行（「先装某个 0.1.x 再升 0.2.0」）已被 0.2.0 节取代，不需要再处理。

| 现行行 | 条目 | 处置 |
|---|---|---|
| 18 | 四路审查收口 | 计划按本稿改写后关闭 |
| 19–26 | W1、W2、W2b、W3–W7 | 按 2.1 节补上 W0、W5a、W6 的子轨；第 24 行 W5 的 `ui` feature 改为 `desktop`（G7） |
| 44 | format 扩展的开放问题 | 并入 K10，TODO 里删除 |
| 82 | 其余全局 run 目录测试的隔离化 | 关闭：`a8ac56a` 已完成，RT 逐个核对过 |
| 145 | restore_recovers 写真实 run 目录 | 关闭：与已完成项重复 |
| 153 | resolve_config_target 抽公共函数 | 删除：S3 的操作层会整体重排 |
| 188 | 随机命名日志的 e2e | 并入 K4 的契约测试 |
| 247 | forward_only 在 status/find/doctor 里没有模式标识 | 并入 K4（快照的 `mode` 字段；前提「需要 bump 协议版本」已经过时） |
| 250 | 判定侧与回放侧的 is_streaming 可能分叉 | 并入 S1：classify 统一时，让判定与回放共用同一个判定。RT 写「转数据面审查」，但 DP 没收这一条 |
| 254 | forward_only 落地确认 | 关闭 |
| 275 | mock 上游覆盖压缩 | 关闭：单测加一个集成用例已覆盖 |
| 278 | 磁盘模式的压缩体判定（已知边界） | 删除：记忆 compressed-body-inspection 已记录 |
| 322 | unix 上 APROXY_RUN_DIR 影响 IPC 寻址 | 并入 K5（删除 `APROXY_RUN_DIR`） |
| 327 | upstream client 开 TCP_NODELAY | 并入 C13，TODO 里删除 |
| 329、331 | 看门狗测试 RAII 化与 UDS roundtrip 单测；测试守护泄漏治理 | 并入 R1 |
| 374 | F2：is_aproxy_process 依赖二进制名 | 关闭：身份判定已去名称化 |
| 383 | 指令集多版本 | 关闭：v0.1.0 已发 `-v3`；选择逻辑见 U3 |
| 384 | H：配置热重载 | 推迟到 0.2.x（3.12），留一行 |
| 422 | alias_start_and_stop_roundtrip 偶发失败 | 观察项：R1 落地后本机循环复测，不复现就删 |
| 452 | 看门狗二期 | 保留（远期） |
| 464 | macOS 平台支持修复 | 保留；「3 个 lib 测试失败」改为 6 个；补一句：K5 把心跳与宣告挪进 home 之后，macOS 只剩进程查询（libproc）这一半工作 |
| 480 | CI 现状处理（待用户定） | 按 3.12 执行后删除 |
| 58 个 `[x]` | 已完成项 | 全部删除（git 保留历史）；「0.1.0 已发布」一节留两行事实性指针 |
| 487–491「已结案」 | 五条 | ① CREATE_BREAKAWAY_FROM_JOB：重新打开（C3）② 命名管道 ACL「tokio 不暴露」：**理由错误**（CP-09/CP-29：tokio 1.53 有 `create_with_security_attributes_raw`，客户端也已核对服务端 pid），改为 SE1 在 0.2.0 做 ③ stop 退出码差异、忽略 `--config`：由 D-3 与 K11 取代，删除 ④ settings 并发写丢更新可以接受：UI 会与 CLI 并发写，K7 加 CAS，删除这条结案 ⑤ 划掉的「unix 分支实测」：已完成，删除 |

### 5.2 记忆更正

| 记忆 | 要改的内容 | 时机 |
|---|---|---|
| daemon-model | 管道名已不是 `\\.\pipe\aproxy-<端口>`，现为按 home 命名空间的 `aproxy-<home_id>-<port>`；日志已不是 `logs/<端口>.log`，现为随机名；op 已不止 ping/shutdown；「命名管道 ACL（tokio 不暴露）」是错的；「有意跳过 CREATE_BREAKAWAY_FROM_JOB」已重新打开；frontmatter 缺 scope/status/last_verified | 两条错误的结案理由**现在就改**；其余等 W2 落地后整体重写 |
| ipc-v1-upgrade-contract | 永久接口清单以 CP §4 为输入，按 G4 的两层表述，在 0.2.0 发布时重写；How to apply 里引用的 compat 测试会随 W1 删除 | W1 合入时改 How to apply；0.2.0 发布时重写全文 |
| 0.2.0-direction | 第 39 行的 `ui` feature 改为 `desktop`（如果采纳）；把 D-1…D-11 的答复补进来 | 用户答复后 |
| transform-format | 决策 #1 按 D-6 更新；「改 envelope 公开 API 必须升版本」在加上 `#[non_exhaustive]` 后，对加性字段放宽 | D-6 答复后；K10 合入后 |
| bounded-retry-paths | 按 D-1 更新 | D-1 答复后 |
| alias-settings | 「`--daemon-child` 必须 global」的教训，在 K6 改为内部子命令后不再适用，加注 | K6 合入后 |
| random-log-names | 清理判据改为 C1 的 GC 规则：只处理内置随机名，并有宽限期 | C1 合入后 |
| aproxy-cli-skill-versioning | 「五个分文件」改为 7 个；写入 D-9 的结论 | D-9 答复后 |
| release-engineering | 版本路线一段；产物命名示例（`aproxy-x86_64-v3.exe`）与实际的 `aproxy-<target>-v3` 不符；「发布时出基准数据」一直没兑现（RT-08） | U3 合入后 |
| macos-support-required、ci-unix-blindspot | 失败数「3 个」统一改为 6 个；后者第 35 行与第 57 行自相矛盾 | 现在 |
| unix-first-test | 「APROXY_RUN_DIR 平台语义待决策」随 K5 结案 | K5 合入后 |
| 10 条没有 frontmatter 的记忆 | ci-unix-blindspot、install-pitfalls、install-three-platform-e2e、never-kill-aproxy-by-name、trusted-publishing-p0、install-online-deep-test、macos-support-required、release-version-alignment、compressed-body-inspection、transform-format：补上 AGENTS.md 要求的 frontmatter（另有约 20 条缺 scope/status/last_verified） | 低优先，可交给 sonnet 批量做；只补元数据，不改正文 |
| 新增 | 发布变体与资产命名契约（U3 定稿后）；发版清单指向 `docs/RELEASING.md` | 对应工作完成时 |

### 5.3 plan/、DOCS.md 与其他

- `plan/release-0.2.0.md`：按本稿第 2、4 节改写工作流、顺序与里程碑；记下 D-* 的答复；「下一步」一节同步。
- `plan/install-v1.md`（620 行，工作已完成）：把仍有效的取舍（四条铁律、forward-fix、ACK 协议）补进 architecture.md 的 install 节或一条记忆；5 处源码注释改为指向那里；然后删除。在 W1 与 W2b 改完 install 流程之后做。
- `plan/watchdog-v1.md`（工作已完成，内容已与现状漂移）：C2 改完看护者之后，同样处理。
- `plan/skill-standardization.md`：并入 release-0.2.0 的 W3/W4 后删除；`.agents/skill-evals/` 的用例集保留，作为回归资产，i18n 之后同步其中的断言。
- `.agents/DOCS.md` 的过时点：
  - 「待同步」一句已经过时。
  - skill 的 references 已是 7 个文件。
  - 有一批文件未建索引：INSTALL_AGENT、benchmark-watchdog、SECURITY、CHANGELOG、README_EN、plan/*、skill-evals、本次的审查目录。
  - `.agents/docs/` 下的三份历史报告标注「历史，不随代码更新」。
- 当前工作区：四份审查报告与本稿都还是未跟踪文件（git status），确认后入库；`.gitignore` 有未提交的修改，`desktop/` 未跟踪，这些属于原型轨，不在本稿范围内。

---

## 6 证据边界

- 本稿的事实来自四份报告；它们都没有运行 cargo，也没有启动实例。报告里标「推断」的，本稿照样只是推断。尤其是以下几项：
  - CI §3 断点推演里的结局 B/C（门的拒绝测试只能验证门本身）。
  - CP-06 的 Windows 删除语义；CP-12 的进程组信号传播；CP-22、CP-23。
  - DP 里客户端 SDK 怎样解析错误体、Codex 的 WebSocket 行为、被掐断的请求是否计费、端口 0 的预检。
  - RT-04、RT-05 的 Tauri 相关事实（要靠 spike 验证）。
  - D3 推断的双重启。
- 本稿新做的核对只有三处：`src/main.rs` 中 `--version` 的路径；TODO 的现行行号；相关记忆的原文。G1–G9 是综合推理，没有代码层面的验证。其中 G1（UI 日志的数据源）是四份报告都没有覆盖的空白，需要在 W2 设计稿里补上设计。
- macOS 不在 0.2.0 范围内。本稿与 macOS 相关的只有「别让它更难做」的部分：K5 把心跳与宣告挪进 home，以及 S2 的 platform 目录。
