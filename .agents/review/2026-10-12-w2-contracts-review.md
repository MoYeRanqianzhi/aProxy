# W2 契约设计稿对抗式审查（2026-10-12）

> **审查对象**：`.agents/plan/w2-contracts.md`。基线是提交 d43ddc9（1680 行）；审查期间 7f89db8 同步了 §4.1 与 W2b 的接缝（+1 行，
> 原稿「C4 的文字不必改」那句已随之更正，本文不再列）。本文行号一律按审查结束时的文件（1681 行，HEAD daa20d0）。
>
> **方法**：只读，没有运行 cargo，没有碰任何 aproxy 进程与 `~/.aproxy`。
> 1. 逐节对照用户已拍板的原文：计划 `release-0.2.0.md` 的 W2 用户决策 D-1…D-9、format 定稿与补充、W2b C1–C8（含 65daf43、
>    3a41729 对 C4、C8 的修订）、W3、W4、W5；记忆 0.2.0-direction、bounded-retry-paths（features-are-config 的出处）、transform-format、
>    ipc-v1-upgrade-contract、gemini-cli-sse-parsing。
> 2. 对照综合稿与四份报告，重点核对 §12 的对账是否属实（CP-11、CP-19、CP §3、综C2、U6）。
> 3. 核对现有代码。稿子「现状」里的 28 处引用由一个只读子代理对照 ebb4572 与 HEAD 逐条核过（附录 A）；凡是支撑本文结论的代码点，
>    本人另行复核：main.rs、retry.rs、proxy.rs 的头覆盖顺序、transform.rs 的 stderr 去向、五处 8 秒就绪时限、restart.rs 的预检、
>    restore.rs 与 watchdog.rs 的删记录、cli.rs 的 `--force` 冲突表，以及 `git show v0.1.0:` 下 config.rs、settings.rs 的字段。
> 4. 依赖源码（`~/.cargo/registry`）：tokio 1.53.1 的 `create_with_security_attributes_raw` 与 `peer_cred`；reqwest 0.12.28 错误的 Display
>    会附完整 URL。
> 5. 把 UI 原型的后端接口 `desktop/src/lib/backend/types.ts` 与 `desktop/src/lib/types.ts` 当作 W5 实际要什么的旁证。
>
> **标注**：【已确认】有原文、代码或依赖源码为证；【推理】由规格文字与平台语义推出，未实测。
> 严重度按派单定义：S1 会导致错误或冲突，必须在实现前改；S2 应该改；S3 小问题。

---

## 0 结论

设计稿的骨架是对的。四级冻结、码表单源、home 唯一根、结构化 spec、「请求与状态的事件只有整条记录与整份快照」、
hello 协商、`seq` 回显，这些决定都站得住。它也基本做到了不重开已拍板的决定。但有四处必须在实现前改：

1. **S1-1**：「读者忽略未知键」用在规则表与 `[[format]]` 上，等于把条件放宽。以后新增的匹配器到了旧版本手里会变成
   「匹配一切」，settings.json 又是各版本共写的文件。§1.1 的「只增」机制在配置这一面不成立。
2. **S1-2**：在途请求的 `attempts`、`decisions` 无界增长，每次变化又整条重发。`max_backoff_secs = 0` 时几小时就能耗尽内存，
   这是新稿引入的风险。
3. **S1-3**：Q3 给用户的前提不完整。会悄悄改变行为的旧键不止四个，还有 `read_timeout_secs`、`keepalive_*`，以及
   settings 的 `forward_only`、`disk_cache`、`watchdog` 等。
4. **S1-4**：期望状态记录的删除规则与 D-8 的动机冲突（配置暂时不可达就删记录）。restart 的写法还丢掉了现行
   「先预检、后停止」的保护。

S2 有 16 条，集中在三类：
- 平台细节写错或会踩坑：`process_group(0)` 与 `setsid()` 不能同用；Windows 环境变量不分大小写；就绪时限 8 秒对上 hello 至少 10 秒。
- 安全出口漏项：format 的 stderr 进日志、`details.reason` 带 URL、settings 的 `defaults` 可含 `api_key`、spool 文件的权限。
- 契约缺口与命名冲突：`attempts` 一词两义，IPC 码到 CLI 的对应缺口，`install --force` 与新旗标的冲突要现在定。

可以删或推迟的：`logs`、`config` 两个 op，`[history]` 里的三个尺寸键，`settings.ui` 的具体键，四项 R 级预留，`subscribe` 的两个参数，
用户可见的全局 `--home`（§4）。这些都能以后加性补回，现在冻结只增加永久成本。

---

## 1 S1：必须在实现前改

### S1-1　「读者忽略未知键」在条件类配置里等于放宽条件
- **位置**：§1.1 机制 1（第 65 行）；§7.5「未知键：报 `config_unknown_key` 告警」（第 959 行）；§7.3「没有任何匹配器的规则匹配一切」
  （第 904 行）；§8.2「未知字段忽略」。
- **问题**：
  - 规则的每个匹配器都在收窄条件，读者忽略不认识的匹配器，规则就匹配得更宽。
  - 例：0.2.x 加了 `codes`（Q1），或者实现了 `method`、`model` 一类匹配器。0.2.0 读到 `{codes=["format_error"], max_attempts=20}`，
    只会告警，然后把它当成「无匹配器的规则」。结果**所有带上游响应的失败都封顶 20 次**，「默认无限重试」的使命被悄悄打破。
  - `[[format]]` 同理：以后若加一个限定作用范围的键（例如只对某些路径生效），0.2.0 会把这个转换器套到全部请求上。
- **为什么一定会遇到**：§8 自己写了 settings.json 是「同一台机器上所有版本共写的文件」（第 1061 行），新版本写
  `defaults.retry.rules`、旧版本读，是常态。降级或回退到 0.2.0 也会遇到。
- **依据**：稿子在两处已经承认这个原理，只是没上升为规则。
  - IPC 拒绝未知参数，理由正是「带条件的 shutdown 会变成无条件停止」（第 805 行，CP-03）。
  - `body` 匹配器「一出现就报 `config_not_supported`」（第 902 行）。
- **改法**：§1.1 加第 7 条「必须理解」。
  - `[[retry.rules]]` 与 `[[format]]` 的条目里出现不认识的键，一律报 error（`config_not_supported`，`details` 带键名，message 说明需要
    更新版本的 aproxy），拒绝启动。settings 的 `defaults` 里同样适用。
  - 其余位置的未知键仍只告警。
  - 这样以后新增匹配器的结果是「旧版本拒绝启动」，而不是「旧版本放宽」。
  - B2、B4 的验收各加一条：规则或 format 条目带未知键时启动失败。
- 【推理】（依据为稿内第 65、805、902、904、959、1061 行的规定）

### S1-2　在途请求记录无界增长，且每次变化整条重发
- **位置**：§6.6 RequestRecord 的 `attempts`、`decisions` 数组，以及「在途的不淘汰」（第 717–740 行）；§6.8 的 `request` 事件「整条
  RequestRecord，按 request_id 整条替换」，在「每次尝试开始与结束、进入退避」时发（第 795 行）；§6.1「应答行 ≤ 16 MiB」（第 636 行）。
- **问题**：使命就是让请求在上游故障时一直重试下去。在途记录不淘汰，数组随尝试线性增长：
  - 默认退避（封顶 320 秒）下，一条卡一天的请求约 270 次尝试。每次尝试 2–3 个 `request` 事件，每个事件都带全部尝试，
    累计发送量是 O(n²)。
  - `retry.max_backoff_secs = 0` 是稿子与现行都允许的写法（第 105 行，retry.rs:15-17）。上游拒绝连接时，每次尝试约 1 毫秒，
    记录每秒增长上千条，几小时内耗尽内存。事件同频率整条重发，订阅者会持续收到 `lagged`，反复重取快照。
  - `requests` 一次最多 500 条，`subscribe` 首行含全部在途请求，单行可能超过 16 MiB 的上限。
  - 0.1.x 没有逐次记录，这个风险是新稿引入的。
- **为什么现在就要定**：A 级形状一旦公布「`attempts` 是完整列表」，以后再截断就是改语义，客户端会按长度推算尝试次数。
- **改法**：
  - 记录里只保留前 N 次与最近 M 次尝试（例如 3 + 20），另加 `attempts_omitted`；总数字段见 S2-13。
  - `decisions` 并进对应的那次尝试，一起截断。
  - `request` 事件按 request_id 合并限频，例如每条每秒至多一次，终态立即发。
  - B8 验收加一个用例：`max_backoff_secs = 0`，上游拒绝连接，跑 10 分钟，断言实例常驻内存有界、单条记录大小有界。
- 【推理】（依据为稿内第 105、636、740、795 行与 retry.rs 的现行退避）

### S1-3　Q3 的前提不成立：会悄悄改变行为的旧键远不止四个
- **位置**：§13 Q3（第 1632–1642 行），尤其是第 1640 行「其余只是改了名、忽略后会回到默认值的旧键」；§7.5 旧键对照
  （第 964–975 行）；§8.1 第 1042 行。
- **问题**：Q3 只把三个 transform 段与 `bounded_retry_paths` 列为「承载行为」，向用户提问时的前提是「其余旧键忽略后无害」。
  逐键核对 v0.1.0 的字段（完整表见 §6）后，至少还有这些：
  - **config.toml 的 `read_timeout_secs`**：为长时间非流式生成调大过它的用户会回到首字节 300 秒，每次尝试都超时、无限重试，
    agent 挂住。这正是 K13 要解决的场景。
  - **`keepalive_interval_secs = 0`（关掉保活）与 `keepalive_trigger`**：回到 15 秒与 `any`。不认注释心跳的客户端会出错，
    这类客户端的实测记录在记忆 gemini-cli-sse-parsing 与 codex-stream-idle-timeout 里。
  - **settings.json 的 `forward_only: true`**：回到 false，数据路径从直通变成缓冲加重试加保活。
  - **settings.json 的 `disk_cache: false`**：回到 true。为了不让请求体落盘而关掉它的用户，请求体又开始写进 spool，这是隐私问题。
  - **settings.json 的 `watchdog: false`**：类型从布尔变成对象（第 1021 行），旧值作废，看护者重新开启。
  - **settings.json 的 `download_proxy`**：失效后，需要代理才能访问 GitHub 的用户 install 与 upgrade 会失败。
    失败看得见，但没有提示就难查。
- **后果**：用户依据不完整的清单做决定。不论选告警还是拒绝，上面这些键都会无声地改变行为。
- **改法**：
  - Q3 改问「全部 0.1.x 旧键一律报错，还是一律告警」，不再区分是否承载行为，这种分类本身就容易漏。
  - 推荐一律报错并附上新写法。0.1.x 用户本来就要按迁移步骤重装，一次性的报错清单比任何无声变化都便宜。
  - 同时要让 `doctor` 一次列全，并让 `unset` 能删掉旧键（S2-15）。
  - 若用户仍选「部分报错」，至少把上面这些键补进清单。
- 【已确认】旧键清单与默认值（`git show v0.1.0:src/config.rs`、`:src/settings.rs`）；后果【推理】

### S1-4　期望状态记录在两条常见路径上丢失，并与 D-8 冲突
- **位置**：§5 的 restart（第 603 行「读目标的期望状态记录 → shutdown → 等它退出 → 用记录里的 spec 调用 spawn_from_spec」）；§9.4 restore
  的结局 `discarded（配置文件已不在）`（第 1151 行）；§12.2 把 CP-11、综C2 记为已着落在 §5（第 1542 行）。
- **问题一：配置暂时不可达就删记录，与 D-8 冲突。**
  - restore 的 `discarded` 与现行看护者，都在配置文件不在时删掉期望状态记录（restore.rs:25-37、watchdog.rs:946-955）。
  - D-8 是用户已接受的决定，它的动机原文就是「配置所在的网络盘开机后挂载得晚」。开机 `aproxy restore` 早于挂载时，记录被删，
    之后看护者再怎么慢速重试也没有记录可依，D-8 落空。
  - CP-11 与综C2 都明确建议「配置缺失时不删 `.restore`，标记并报告」。§12.2 说它们已着落在 §5，但 §5 里没有这一条。
- **问题二：restart 丢掉了现行的两道保护。**
  - 现行 restart 先用新守护将走的同一条解析路径干跑一遍，换端口时还探测新端口能否绑定，不通过就不碰旧实例。
    restart.rs:8-14 写明了原因：「restart 的主要用途是改完 config.toml 让它生效，配置笔误恰恰最常出现在这一步」。
    对应的回归测试在 `tests/restart_integration.rs:415`（control-plane.md 第 573 行，daemon-ipc-01）。
  - 现行 install 的滚动重启在「停止之后立即回写 `.restore`」（install/restart.rs:15-27、311-317）。
  - 稿子的 restart 两者都没有。0.2.0 里「界面或 `config set` 改配置，然后 restart」是主路径（§9.3 的 `instances_to_restart`，
    §6.4「写文件 + restart」），配置写错一次，健康的实例就永久下线，而且没有记录可以恢复。
- **改法**：
  - restore 与看护者遇到配置文件不在时，保留记录，结局为 `config_missing`，并按 D-8 慢速重试。只有 stop 才删记录。
  - §5 的 restart 写明「先预检、后停止」。
  - restart 与 install 在旧实例退出后，立即用同一份 spec 回写期望状态记录。也可以给 shutdown 加 `keep_restore:true`，由调用方声明
    「停了要再拉起」，并在 `features` 里声明这项能力。
- 【已确认】现行代码、D-8 原文、§12.2 对账

---

## 2 S2：应该改

### S2-1　unix 脱离启动者：`process_group(0)` 与 `setsid()` 不能同用
- **位置**：§5 第 577 行「unix 用 `process_group(0)` 加 `setsid()`」（综C3 原文也这样写）。
- **问题**：
  - setsid(2) 要求调用者不是进程组组长，否则返回 EPERM，而 `process_group(0)` 正好让子进程成为组长。
  - 反过来先 setsid 再 setpgid 也不行：对会话首进程调用 setpgid 同样返回 EPERM。
  - std 的 `pre_exec` 闭包返回错误会让 `spawn()` 失败。照字面实现，unix 上每次拉起守护都会失败。
- **改法**：只用 `setsid()`，经 `pre_exec` 调用，它同时建立新会话和以子进程为组长的新进程组。删掉 `process_group(0)`。
  综合稿 C3 的写法一并更正。
- 【推理】（依据 setsid(2)、setpgid(2) 的 EPERM 语义；本机没有 rust-src，没逐行核对 std 的执行顺序，但两种顺序都会失败）

### S2-2　Windows 上环境变量名不分大小写，spec.env 的 null 会删掉刚设的代理
- **位置**：§4.2 InstanceSpec 样例 `"HTTPS_PROXY":"…","https_proxy":null`（第 491 行）；第 497 行「值为 null 表示启动者没设，拉起时在子进程里
  删掉它」；§5 第 574 行。
- **问题**：
  - 样例里的状态在 Windows 上不可能出现：同一个变量，两个名字查到的是同一个值。
  - 如果快照按 `std::env::vars()` 记录实际名字，`https_proxy` 会记成 null。拉起时 `env_remove("https_proxy")` 删掉的正是刚设好的
    `HTTPS_PROXY`，因为 std 的 Command 在 Windows 上按不分大小写比较变量名。
  - 结果是实例丢掉代理，违背 D-2「重拉出来的实例走同一条网络路径」。
  - B5 的验收用例能不能测出它，取决于快照用哪个 API。
- **改法**：§4.2 写明：
  - Windows 上每个变量只记一个规范名（大写），快照用不分大小写的查找。
  - unix 上大小写两种名字各记一条。
  - 样例按平台分开写。
- 【推理】

### S2-3　「CAS」名不副实；临时名与持久化也不足
- **位置**：§7.6 第 980–987 行；§8.3 第 1057 行「原子写、0600、CAS，规则同 §7.6」；`*.tmp-<pid>`（第 454、981 行）。
- **问题**：
  - 「rename 之前再读一次比对」与 rename 之间没有锁。GUI 与 CLI 这两个 aproxy 写者正是 K7 所说的并发来源，它们可以读到
    同一个哈希、先后 rename，前一个的改动就丢了。这降低了丢更新的概率，但不是 CAS。A 级契约写成「CAS」，会让调用方以为有保证。
  - 临时名只对进程唯一。同一进程（GUI）里两个任务同时写同一个文件，会写进同一个临时文件。W2b 刚为 bin 交换吸取过同样的教训
    （C2「从不使用固定的中间文件名」）。
  - 没要求 rename 前 `sync_all`，断电后可能留下空的 config.toml。C2 对二进制有这项要求。
  - Windows 上目标被杀毒软件短暂占着时 rename 会失败，需要短暂退避重试，与 C8 第 5 步同理。
- **改法**：
  - aproxy 写者之间用操作系统锁串行化，锁原语与 C4 相同（unix `flock`、Windows `LockFileEx`），锁在旁路文件上。
  - 锁内依次做：读、改、写临时文件、`sync_all`、rename。
  - 哈希比对保留，用来发现编辑器之类的外部写者。
  - 临时名改为 `*.tmp-<pid>-<纳秒>` 并用 create_new 创建。
- 【推理】

### S2-4　就绪时限 8 秒，对上「hello 至少等 10 秒」且握手先于 bind
- **位置**：§5 实例启动顺序：先握手（第 587 行），再 bind，「控制端点应答即就绪」（第 581 行）；§10.2「aproxy 至少等 10 秒的 hello 回信」
  （第 1248 行）。
- **问题**：
  - 现行 start、restart、restore、看护者重拉、install 的就绪时限都是 8 秒（start.rs:274、restart.rs:141、restore.rs:52、watchdog.rs:968、
    install/restart.rs:203）。
  - 启动要 8–10 秒的 format 会让 `start` 报 `ready_timeout`，实例随后却又起来了。
  - 看护者会把它判为失败、再拉一个，第二个在 bind 时与第一个撞端口。
  - 多个 format 串行握手时，最坏要 N × 10 秒。
- **改法**：二选一写进 §5。
  - ① 拉起方的就绪时限不小于握手预算：多个程序并行握手，时限取 10 秒加余量，与 hello 的下限一起列为 A 级常量。
  - ② 先 bind 控制端点，以 `state:"starting"` 应答（快照里加这个取值），握手完成后才 bind 代理端口并转为 `serving`，就绪判定看
    `serving`。② 还能让 status 显示「正在启动」，推荐它。
- 【已确认】8 秒常量；后果【推理】

### S2-5　启动顺序有「停了又被拉回」的竞态
- **位置**：§5 启动顺序第 4、5 步（第 588–589 行：先 bind 控制端点，再写期望状态记录）；第 595 行「收到 shutdown 立即删期望状态记录」。
- **问题**：
  - 第 4 步之后、第 5 步之前，实例已能收到 shutdown，例如脚本里的 `start && stop`。这时 shutdown 删不到记录，第 5 步随后把记录写下。
  - 实例退出后，看护者把用户刚停的实例拉回来。
  - `stop all` 与正在启动的实例之间有同样的竞态。
  - 这正是综C10「收到 shutdown 立即删」要防的事。第 6 步「开始服务」是否包括控制端点开始处理请求，稿子没写，竞态是否存在取决于这句的解释。
- **改法**：写明「期望状态记录在控制端点开始处理请求之前写」；或者收到 shutdown 后置一个标志，之后不再写记录。
- 【推理】

### S2-6　同一程序对同一请求的并发消息会争写 `state`
- **位置**：§10.3「`state` 按程序 × 请求隔离」（第 1275 行）；§10.7「之后各拍不再重复，需要的信息请写进 state」（第 1402 行）；
  §10.7「一拍没在下一拍之前回来」（第 1404 行），说明心跳与其他阶段并行。
- **问题**：
  - 心跳每拍独立发送，会与 decide、retry 同时在途。
  - 两个回信都带 `state` 时，后到的覆盖先到的，丢哪一次更新取决于时序。
  - 稿子还鼓励心跳写 `state`，这个竞态因此会经常发生。
- **改法**：二选一写进协议。
  - ① 同一个（程序，请求）的消息串行投递。
  - ② 只有 request、retry、decide、response、stream 的回信能改 `state`，心跳回信里的 `state` 忽略。心跳需要的信息由 aproxy 每拍给，
    §10.7 已经在给 `last_failure`。
  - ② 更简单。
- 【推理】

### S2-7　settings 的 `defaults` 可以装 `api_key`，会把密钥发给别家上游
- **位置**：§7.1 键表的 `defaults` 一列（第 829–842 行），`base_url`、`listen_addr`、`api_key`、`proxy_password`、`extra_headers`、
  `override_headers` 都打了 ✓；§8.1 第 1035 行。
- **问题**：
  - 0.1.x 的全局默认只有 7 个非敏感键（max_body_mb、disk_cache、forward_only、bounded_retry_paths、allowed_hosts、allowed_origins、
    keepalive_trigger）。稿子按 D-5「与 config.toml 同构」把凭据也放进了全局默认。
  - `api_key` 会同时覆盖 `Authorization` 与 `x-api-key`（proxy.rs:1196-1210）。某个配置指向另一家中转、本来靠客户端自带的 key，
    只要它没写 `api_key` 就会继承全局 key：A 家的 key 被发给 B 家，客户端自己的 key 还被覆盖掉。
  - `override_headers` 里的鉴权头、`proxy_password` 同理。
  - `listen_addr` 进了全局默认，多个配置会继承同一个端口，必然冲突。
  - settings.json 由此也变成含密钥的文件，界面与 CLI 都整份读写它。
- **改法**：
  - `defaults` 排除 `api_key`、`proxy_username`、`proxy_password`、`listen_addr`。
  - `extra_headers` 与 `override_headers` 若保留，鉴权类头名同样排除。
  - 这偏离了 D-5 的字面（D-5 只排除 format 与 `log_file`），需要用户确认，并写清理由。
- 【已确认】现行字段与头覆盖逻辑；风险【推理】

### S2-8　含敏感内容的文件清单不全
- **位置**：§1.6「文件」（第 183–186 行）。
- **问题**：
  - **spool**：`spool/<pid>/` 里是请求体与响应体全文，也就是对话内容。现行代码里没有任何 0600 或 0700 的设置（只有 bin 的 0755），
    install.sh 用 `mkdir -p` 建目录（scripts/install.sh:293），都按 umask 来。家目录是 0755 的发行版上，同机其他用户可读。
  - **install/state.json**：稿子要求 install 把 spec 记进它（第 600 行），spec 里有 `overrides.api_key` 与代理凭据。现行 install 已经
    把含 `--api-key` 的启动参数记进在途清单（install/restart.rs:296-305）。§1.6 的 Windows 一句只提了 restore 与 spec 文件。
  - **日志**：若采纳稿子让 format 的 stderr 进日志，日志也会成为敏感文件（S2-9）。
- **改法**：
  - 最简单的办法是让 **home 本身 0700**（unix 上新建时设好），三个安装脚本同步，doctor 对组或他人可访问的 home 告警。
    这一条就覆盖了 spool、logs、install、run、settings、config。
  - Windows 上的「含密钥文件」清单改为 restore、spec、install/state.json，并说明 spool 依赖 home 的继承 ACL：`%USERPROFILE%`
    下默认只有本人、SYSTEM、Administrators。
- 【已确认】代码与脚本里没有权限设置；可读性【推理】

### S2-9　打码的出口漏了四处
- **位置**：§1.5（第 140–168 行）；§10.1「程序的 stderr 进实例日志」（第 1215 行）；§2.2 `upstream_connect_failed.details.reason`
  「上游库给出的英文原文」（第 245 行）；§1.5「请求体与响应体不打码」（第 162 行）。
- **问题**：
  1. **format 的 stderr**：现在被丢弃（transform.rs:426 `Stdio::null()`），稿子改为写进实例日志。format 拿得到明文 key，§10.4 样例
     第 1287 行的请求头里就有，任何一行调试输出都会把 key 写进日志。日志会被贴进 issue，也会显示在界面的日志视图里。
     §1.5 的「纵深防御」只覆盖 `report` 与 `fail.reason`。
  2. **`details.reason`**：reqwest 的错误 Display 会附上完整 URL（reqwest-0.12.28 `src/error.rs:268`），它的文档专门提醒 URL 里
     有 key 时要先 `without_url()`。`base_url` 不许带查询串，但 format 改写后的 URL 可以带，例如 Gemini 的 `?key=`。
     `details.reason` 会进 HTTP 错误体（发给客户端，而 `listen_addr` 不一定是回环地址）、IPC 的 `last_error` 和日志。
     现行代码用 `without_url()` 加打码（proxy.rs:3206-3210），稿子没把它写成规则，新实现很容易直接 `to_string()`。
  3. **请求体与响应体**：经 IPC 原样返回。用户的规则是「IPC 永不返回明文密钥」。体里一般没有 key，但 format 可以把 key
     写进体（有的中转要求这样）。aproxy 自己知道的密钥（`api_key`、`proxy_password`、format `env` 的值、args 里的密钥）
     可以在返回前做一次原文替换，与 `report` 的纵深防御同理。
  4. **桌面的 Tauri 桥接层**：不在出口清单里。GUI 进程经操作层读 config.toml 与期望状态记录，拿到的是明文。
- **改法**：§1.5 的「所有读出口」补上四项：
  - format stderr 的每一行，写日志前做已知密钥替换；
  - HTTP 错误体与 SSE 终态里的 `details.reason`，一律先 `without_url()` 再过 redact；
  - IPC 返回的体，做已知密钥替换，并在 `bodies` 的说明里写明「不保证体内没有第三方密钥」；
  - 桌面桥接层。

  另外写明：format 消息与 `state` 永不整条进日志，也永不经 IPC 输出。
- 【已确认】transform.rs:426、reqwest 源码、proxy.rs:3206；其余【推理】

### S2-10　占位对象往返，对数组与表里的密钥没有定义
- **位置**：§1.5「写回占位对象等于不改」（第 160 行）；§9.3「数组表（`retry.rules`、`format`）只能整体读写」（第 1126 行）。
- **问题**：
  - 标量键好定义；数组与表里的密钥没定义：`[[format]]` 的 `args`（密钥旗标之后的值）、`env` 的值，以及 `extra_headers`、
    `override_headers` 里的鉴权头。
  - `format` 又只能整体写。界面整份往返时，`args` 里的一个占位对象对应原来哪个值？用户调了参数顺序、删了一个程序、
    改了 `name` 之后，怎么对得上？
  - 另外，SECRET_ARG_FLAGS 是 7 个旗标名的启发式（commands/config.rs:14-22），对 `--key=…`、`-k` 这类写法无效，
    这些密钥会经 `config show` 与 IPC 原样输出。
- **改法**：
  - 写明解析规则：`env` 与头表按键名对应；`args` 按「同名程序、同一个密钥旗标之后」对应。对不上就报 `config_invalid_value`，
    要求重新输入，不去猜。
  - 文档引导用户把密钥放进 `env`。
- 【推理】

### S2-11　`install --force` 与新加的 `--check`、`--json`：C7 要求现在就定冲突
- **位置**：§9.1「加 `--check` 与 `--json`」（第 1108 行）；C7 原文「与 --force 冲突的选项…由参数解析拒绝…以后放宽不破坏调用方，
  收紧才会」。
- **问题**：
  - 现行 cli.rs 的 `--force` 用 `conflicts_with_all` 拒绝 adopt、abort、skills_only、continue_、no_skills、allow_downgrade。
  - 稿子给 install 加了两个旗标，没说它们和 `--force` 的关系。
  - 若 0.2.0 发布时 `install --force --json` 被接受而 `--json` 静默无效，以后再拒绝它就是 C7 所说的「收紧」。
- **改法**：§9.1 写明两者与 `--force` 冲突（用法错误，退出码 2），并加进 force_contract 的命令行样本。以后要支持时按「放宽」加。
- 【已确认】cli.rs 的冲突表与 C7 原文

### S2-12　IPC 码到 CLI 的对应有缺口
- **位置**：§2.3（第 256–269 行）、§2.4（第 289–311 行）、§9.4 stop、restart、restore 的结局词（第 1151 行）。
- **问题**：
  - IPC 协议码怎样对应到 CLI 没写，至少有一处会直接卡住实现。`stop idle`、`restart idle` 用 `shutdown{if_idle_secs}`，实例在两次
    检查之间变忙，会收到 `condition_not_met`。stop 的结局词只有 `stopped`、`already_stopped`、`killed`、`failed`，没有「跳过：不再闲置」；
    退出码算 0、1 还是 5 也没定。
  - 其余对应也没写：新 CLI 连旧实例得到 `unknown_op`、`unsupported_arg`、`unsupported_version` 时，是报 `instance_incompatible` 还是
    降级处理；`bad_request` 与 IPC 的 `internal` 各对应哪个操作码。
- **改法**：
  - §2.4 加一张「IPC 码 → CLI 结局、操作码、退出码」表。
  - stop 与 restart 的结局词加 `skipped`，`details.reason` 取 `not_idle`，对 `idle` 目标计作成功（退出码 0）。
- 【已确认】稿内词表缺这一项

### S2-13　同一个名字两种含义
- **位置**：§1.3 第 114–115 行；§3 第 370 行；§6.6 第 721–722 行、第 747 行。
- **问题**：
  - **`attempts`**：§1.3 定义为总数，§3 的 HTTP 错误体也按总数用；§6.6 的 RequestRecord 与 RequestDetail 里，它却是尝试对象的数组。
    同一个名字两种类型，违反稿子自己的 §1.3。
  - **上游状态码**：§1.3 说 IPC 记录里叫 `upstream_status`，RequestRecord 却用 `status`（顶层与 `attempts[].status` 都是）。
    顶层的 `status` 指上游状态，还是回给客户端的状态（可能是 aproxy 自己的 502），也没说。
- **改法**：
  - RequestRecord 的数组改名（例如 `tries`），总数仍叫 `attempts`。
  - 记录里的上游状态码统一叫 `upstream_status`，回给客户端的状态另起名字。
  - 这些都在 A 级形状里，只能现在改。
- 【已确认】

### S2-14　默认重试时序变了，却没交给用户
- **位置**：§7.1 中 `retry.jitter`、`retry.respect_retry_after` 的默认值为 true（第 851–852 行）；§7.3（第 917–921 行）；「为什么」（第 1001 行）。
- **问题**：
  - 这改变了默认的重试时序：带 Retry-After 的失败不再走「前 3 次立即重试」，等待时长以上游给的值为下限（封顶 320 秒）。
    现行 retry.rs:14-20 不读 Retry-After，也没有抖动。
  - 它不改变「重试与否」，但改变 agent 感受到的恢复速度。
  - 综合稿定过「默认行为不变」的原则（综合稿 1.11 第 14 条）。D-1 只要求「下放」这两个开关，没定默认值。
  - 稿子把它当作主代理自定的事，没列进 §13，§0 的摘要里也没有。
- **改法**：在 §13 或给用户的摘要里单列「默认重试时序的变化」，写明利弊：尊重限流、减少无效重试，代价是恢复可能变慢。
  或者保持现行默认（两个开关默认 false），让用户按需打开。
- 【已确认】现行行为

### S2-15　旧键删不掉，迁移没法用命令完成
- **位置**：§9.3「只能设置已知的键，未知键报 `config_unknown_key`、退出码 3」（第 1128 行）；§8.2「未知字段忽略，写回时保留」
  （第 1047 行）。
- **问题**：
  - 0.1.x 的平铺 settings 字段与 config 里的旧名，都属于「未知键」。如果 `unset` 也受「只能设置已知的键」约束，用户就无法用命令
    删掉它们。
  - 写者又保留未知字段，这些旧键会永远留在 settings.json 里：告警永远在；若 Q3 选「报错」，则每次启动都失败，只能手改 JSON。
  - 界面（W5）同样删不掉。
- **改法**：
  - 写明 `unset` 接受任何键：删除总是安全的，删不存在的键退出 0。
  - `doctor` 列出清理旧键的完整命令，或提供一个专门清理已移除键的子命令。
- 【推理】

### S2-16　§14：前置没人负责；crate 边界与冻结算法；验收不可证伪
- **(a) 前置无人排期**：
  - §14 第 1648–1653 行把 S1、S2、S3、R1、R2b 定为「不属于本稿的块」，但数据面整条线（B1 → B4 → B10 → B11）与控制面整条线都依赖它们。
  - 计划 alpha.1 的五步里没有它们，TODO 第 27 行只有笼统的「W6 审查发现的修复与重构」，没有负责人。
  - 改法：在计划里给它们排期、定负责人，或者在 §14 写明 W2 的实现从它们开始。【已确认】
- **(b) aproxy-ipc 与冻结算法的 crate 边界**：
  - B0（第 1662 行）把实例枚举放进 aproxy-ipc。§6.7 的枚举要读台账，并按 pid 加启动标记判断进程在不在；依赖却只列了 serde、
    serde_json、tokio、sha2、windows-sys。
  - 启动标记的推导与台账读者的义务属于 C3 的冻结代码，现在在主 crate 的 `src/ledger.rs`，AGENTS.md 点名了这个位置。
    aproxy-desktop 又不能依赖主 crate，否则成环。
  - 改法：二选一。要么把 C3 的读者与推导挪进 aproxy-ipc，同时改 AGENTS.md 与契约测试的路径（macOS 的 libproc 还要 libc）；
    要么枚举留在主 crate，经注入接口交给桌面。不能各写一份冻结算法。【推理】
- **(c) B8 的验收不可证伪**：
  - 「改动前后用 loadgen 各测一次并记录数据」（第 1670 行）没有阈值，记下什么都算通过，而 O2 的风险（热路径加锁、体拷贝、
    事件构造）正要靠它发现。
  - 改法：定阈值，例如有订阅者与无订阅者两种情况下，p50、p99 延迟与吞吐相对基线的退化上限；再加上 S1-2 的长重试内存用例。【已确认】

---

## 3 S3：小问题

| # | 位置 | 问题与依据 | 改法 | 性质 |
|---|---|---|---|---|
| 1 | §0 第 19 行；§2.1 注 ¹（第 236 行）；§2.4 `start_failed`（第 303 行） | 「每个终态码有固定 HTTP 状态，每个操作码有固定退出码」不准：`retry_exhausted`、`retry_stopped`、`format_rejected` 有上游或 format 的响应时，状态码是它们的；`start_failed` 有 1 和 3 两种退出码。码表元数据要能表达这两种情况 | 改措辞；元数据允许「透传」与「按子码」两种取值 | 已确认 |
| 2 | §2.4 第 283 行 | 「6–9 预留给通用含义」与 C7（F 级）原文「通用退出码只用 0–5」字面冲突 | 删掉 6–9 的预留；以后新的通用含义只加 `--json` 的码 | 已确认 |
| 3 | §2.4 `home_invalid` 退出 2（第 294 行）；§9.2 第 1115 行 | 强制安装对同一情况退出 10（main.rs:62-68、C8 第 1 步），§2.4 没注明这个例外。用户可见的 `--home` 给相对路径时，`install --force` 在第 2 步就得到 2，而 APROXY_HOME 是相对路径时得到 10，两者不一致 | §2.4 注明强制安装的例外；`--home` 推迟（§4 第 8 条）则这个问题消失 | 已确认 |
| 4 | §2.4 第 322 行；§9.3 第 1128 行 | `config_unknown_key` 是 warning 级，`set` 遇到未知键却报它并退出 3：同一个码两种级别 | `set` 另用一个码，或诊断里带 error 级 | 已确认 |
| 5 | §2.4 `port_in_use.details.owner/home`（第 302 行）；§6.5 第 712 行 | 别的 home 的实例，控制端点名里含的是它的 home_id，CLI 不知道；unix 上它的 socket 在对方 home 的 run/ 下；按二进制名判断进程又被用户的身份规则禁止（记忆 identity-no-name）。所以 `home` 填不出来，`owner` 只能尽力 | `home` 改为可选；`owner` 只取「本 home 的 aproxy」或 `unknown`；§6.5 里 `home` 字段的理由随之改写 | 推理 |
| 6 | §2.1 `format_failed`（第 230 行）；§10.4 第 1309–1316 行 | 仅转发模式下 format 回 `fail{retry:true}` 时回什么没写。`required = false` 的程序回 `fail{retry:false}` 时，它是被跳过的「错误」，还是要执行的「决定」，也没写 | 两处各写一句；建议可选程序的 `fail` 也按「决定」执行，`required` 只管程序可不可用 | 已确认 |
| 7 | §3 第 377–381 行 | 仅转发模式下透传的上游非 2xx 带不带 `x-aproxy-*` 头没写。这时 aProxy 并没有「放弃」，与「为什么」里的论证（第 413 行）不符 | 写明不带，或写明带 `x-aproxy-request-id` | 已确认 |
| 8 | §3 第 393 行 | 提交之后 format 给出非 SSE 的终态响应时，写的是 aproxy 自己的终态事件，format 按客户端协议生成的错误被丢掉 | 把 format 的 JSON 体作为 `event: error` 的 data 写出 | 推理 |
| 9 | §4.1 第 464 行 | 安装宣告的「16 字节布局」定为 A 级，却没写字节偏移。现行布局：[0,4) pid u32 小端，[4,8) 填 0，[8,16) 心跳毫秒 u64 小端（announce.rs） | 写明偏移 | 已确认 |
| 10 | §4.1 `spool/<pid>/`（第 445 行）；§4.3 第 6 条（第 539 行） | 只按 pid 判断「已不在」，pid 被复用时目录清不掉；其余记录都用 pid 加启动标记 | 目录名带启动标记的短哈希，或目录里放属主文件 | 推理 |
| 11 | §4.1 第 447–456 行 | 记录与控制端点按端口命名，意味着一个 home 里一个端口只能有一个实例：127.0.0.1:12345 与 [::1]:12345 不能并存。这是现行限制，冻结后成为永久约束 | 写成显式约束 | 已确认 |
| 12 | §4.3 第 537 行 | run/ 里只要有一条读不懂的记录，日志 GC 就整轮跳过；新版本留下的一条记录可以让日志永远不被清理 | 至少在 status 与 doctor 里提示原因 | 推理 |
| 13 | §4.3 第 531 行；§5 第 598 行 | `stop all` 要删全部期望状态记录，读者规则却禁止删读不懂的（schema 更高的）记录 | 写明 `stop all` 只删本 schema 的记录，并报告其余的 | 已确认 |
| 14 | §4.2 InstanceSpec 的 `foreground`（第 491、508 行） | 前台实例既不经拉起，也不写期望状态记录，文件里这个字段恒为 false | 从 spec 删掉，只留在实例记录里 | 推理 |
| 15 | §4.3 第 533 行 | spec 文件含明文密钥，却只靠「pid 已不在且早于 10 分钟」的 GC 清理 | 拉起方在子进程就绪或失败后立即删 | 推理 |
| 16 | §5 第 586 行 | 台账在 bind 之前写。`listen_addr` 端口为 0 时，台账的 `port` 不是实际端口，C8 第 6 步按端口判断实例回没回来，会误报 12（W2b 本地检查点已记下这类误报） | 写明 bind 之后重写台账条目，带上实际端口 | 推理 |
| 17 | §6.1 第 636 行 | `request` 带多次尝试的体时，JSON 转义后可能超过 16 MiB：预算按原始字节算，转义会放大 | 按尝试分页取体，或按转义后的大小控制 | 推理 |
| 18 | §6.6 第 739 行 | `decisions[]` 与 `attempts[]` 是两个平行数组，客户端要按序号拼起来 | 合并为每次尝试里的 `decision` 字段，也方便 S1-2 的截断 | 已确认 |
| 19 | §6.6 第 759 行；§1.2 第 103–104 行 | 截断保留开头，而 agent 请求体最有用的是末尾（最新一轮），Claude Code 的请求体常超过 1 MiB。同在 `[history]` 里，`body_max_kb = 0` 是不限，`body_budget_mb = 0` 却是不保留，对用户是陷阱 | 保留头尾或只保留尾部；尺寸键改为常量（§4 第 3 条） | 推理 |
| 20 | §7.1 `api_key`（第 831 行）；§10 | 「等效于覆盖 `Authorization: Bearer`」不完整，现行同时覆盖 `x-api-key`（proxy.rs:1196-1210）。§10 也没写头覆盖在请求链之前还是之后应用：现行在之前（proxy.rs:1272 先于 1349），format 能改写它们。retry 阶段说「从客户端的原始请求开始」，实际应是「应用头覆盖之后的请求」 | 两处写全 | 已确认 |
| 21 | §7.1 `retry.max_attempts`（第 854 行） | 列在配置键表里，却不能写进配置 | 单列「只能按请求覆盖的键」 | 已确认 |
| 22 | §7.3 第 907 行；§13 Q1 第 1619 行 | `max_attempts`「这类失败允许的总尝试数」的计数口径没写死：是请求的总尝试序号，还是命中这条规则的失败次数？bounded_retry_paths 用的是前者。Q1 配方说「连续出错 20 次」，规则表表达不了「连续」。`rule:<序号>` 从 0 还是 1 起也没写 | 写死口径与起始编号；改配方措辞 | 已确认 |
| 23 | §7.5 `sources`（第 958 行） | 没有「环境变量」这一来源。`proxy` 不设时走 spec.env 里的 `HTTPS_PROXY`，`config show --effective` 与界面却会显示「没有代理」 | 加 `env` 来源 | 推理 |
| 24 | §7.4 第 944 行 | 仅转发模式只配了 request 阶段的协议转换时，响应仍是上游协议，校验发现不了 | 文档写明，doctor 可以提示 | 推理 |
| 25 | §8.1、§8.2 | 没有 `schema` 字段的文件（0.1.x 留下的）按几处理、第一次写回时补不补 `schema:1`，没写。读者遇到 schema 2 时是整份跳过（全部回默认，即 S1-3 的问题）还是逐字段读，也没写 | 写明：缺失按 1；读者逐字段读、只拒绝写 | 已确认 |
| 26 | §9.3 第 1126–1129 行 | 「字符串键原样取」，`-` 又只读一行：含换行的值写不出来，而 `keepalive.heartbeat` 必须以空行结尾，PowerShell 与 cmd 下尤其难写。「TOML 内联或 JSON 都接受」是两套语法 | 值以 `"` 开头时按带转义的字符串字面量解析；config 只收 TOML，settings 只收 JSON | 推理 |
| 27 | §9.2 第 1117 行 | 预扫描 `install`、`upgrade` 加 `--force` 时按什么位置认子命令，没定义 | 写明取第一个非旗标参数 | 推理 |
| 28 | §9.4、§9.5 第 1173、1179 行 | `start --json` 应与 `--foreground` 互斥。`config get` 在文件不存在时退出 4，这对 `settings get` 不适用：没有 settings.json 时应返回默认值 | 各写一句 | 推理 |
| 29 | §10.2 第 1260 行 | required 程序握手失败一律退出 3（配置无效），但 `spawn`、`timeout` 这类可能是瞬时的（杀毒软件占着文件、冷启动慢），让 agent 去改配置是误导 | 按失败类型分 1 与 3 | 推理 |
| 30 | §10.5 第 1333 行 | decide 收「解码后的完整内容，上限与 response 阶段相同」，而 response 阶段没写上限（实为 `max_response_mb`，256 MiB）。对着 2 秒的 decide 时限，大体会稳定超时 | 给 decide 的体单设上限（例如 1 MiB），超出就截断并标记 | 推理 |
| 31 | §10.6 第 1382 行 | 流 worker 不计入 pool_max，但流结束后它回到池里（可能使池超过 pool_max）还是退出，没写；并发的长流有多少条，就有多少个进程 | 写明回收规则，文档写明进程数 | 推理 |
| 32 | §10.7 第 1402 行 | 第 1 拍带客户端的完整请求体（可能数 MB），spawn 模式下每个请求都要把它写给一个新进程 | 只给 §10.4 的 `client` 对象（足够判断协议），体按 features 按需给 | 推理 |
| 33 | §10.9 第 1438 行；§10.1 第 1216 行 | notify「不得回复」打破了「每条消息恰好一个回信」和「读一行、回一行」的最低门槛（第 1457 行） | 改为 format 回空应答 `{v,seq}`，aproxy 不等它 | 推理 |
| 34 | 第 327、1220、1278、1457 行；D-7 | **偏离用户原话，稿子却没标为偏离**，应列进 §13 或给用户的摘要，请用户确认：①spawn 模式的进程不握手，用户原话是「每个 format 进程启动时先与 aproxy 交换协议版本与能力」；②可选程序声明了不支持的阶段只告警，用户原话是「配置里声明了它不支持的阶段就在启动时报错」，建议这一项不论 required 与否都报错，只有起不来、超时对可选程序降级为告警；③decide 的回信表达不了「换路由」（用户原话列了「是否换路由（every_attempt）」），稿子改为「有程序声明 retry 就每次重跑整条请求链」，等价能力要经 state 在 decide 与 retry 之间传递，稿子没写这个映射；未声明 retry 的程序每次重试还会再收到 `stage:"request"`，对只想每个请求处理一次的审计、计数类程序是意外；④裸 `aproxy` 退出 2、帮助写到 stderr，D-7 只说「只输出帮助」，属合理细化，列明即可 | 列明并说明理由 | 已确认 |
| 35 | §1.6 第 182 行 | 服务端按 peer uid 拒绝不同用户，会连 root 一起拒绝（`sudo aproxy stop`），允不允许没写 | 写明 | 推理 |
| 36 | §4.1 第 451 行 | 心跳从 /dev/shm 挪进 run/ 之后，home 在网络文件系统上时，每 10 秒一次的写可能阻塞 tokio 工作线程（现行心跳是同步文件写，watchdog.rs:1447-1460） | 心跳写放到阻塞线程或专用线程 | 推理 |
| 37 | §14 | B10 的体量远大于其他块（hello、`seq`、链、decide、两种模式的 stream、池改造、envelope API），建议拆成协议核心、decide、stream 三块。B3 中 `defaults` 的校验依赖 B2 的键表。B6 的 `by_code`、B8 的记录都要用 B1 的码，依赖里却没列 B1。B6 的验收「`base_url` 带 `?key=`」构造不出来：§7.1 与现行校验（config.rs:611）都拒绝带 `?` 的 base_url，改用 userinfo。B7「CI 允许时另做跨用户的连接测试」应改为必做：GitHub 的 ubuntu runner 有免密 sudo，可以建第二个用户。§4.2 install/state.json 的四个新字段（A5）没落到任何块：W1 没做，state.rs 里没有 | 逐项改 | 已确认 |

---

## 4 建议删减或推迟

原则：A 级的每一项都是永久成本，要维护、要测试、要写文档、要翻译。下面这些在 0.2.0 没有消费者，或者有更小的替代。
删掉之后，以后可以加性补回：IPC 加 op 用 `ops` 与 `features` 声明，配置加键对旧版本只是未知键（条件类的键按 S1-1 是拒绝），
format 加阶段或能力靠 hello 协商。没有一条会破坏「只增」。

| # | 项 | 位置 | 为什么 0.2.0 不需要 | 以后怎样补回 | 省下的永久成本 |
|---|---|---|---|---|---|
| 1 | `logs` op，`log`、`rotated` 事件，`no_log_file` 码，`logs.follow` | §6.4 第 682 行；§6.8 第 797–798 行 | CLI 与界面按快照或记录里的 `log_path` 直接读文件即可。实例不在时本来就只能这样读，§9.6 第 1190 行已经这样写。aproxy-ipc 提供一个跟随文件的库函数，CLI 与界面共用 | 以后有远程界面再加 op | 一个流式 op、两类事件、一个码、一个 feature、日志轮转的线上语义 |
| 2 | `config` op | §6.4 第 680、685 行 | 界面编辑的是「文件里的配置」，经操作层读文件；实例启动时用的覆盖项在期望状态记录的 `spec.overrides` 里；`sources` 与 `diagnostics` 都能由 `config::resolve` 在 GUI 进程里算出（稿子已定 resolve 由桌面共用，第 954 行） | 加 op | 一个带 `sources`、`diagnostics` 形状的 op |
| 3 | `[history]` 的 `requests`、`body_max_kb`、`body_budget_mb` | §7.1 第 855–858 行 | 只留开关 `history.bodies`（见 Q2，建议改成三值），条数与预算做 E 级常量 | 加键 | 三个 A 级键及其「0」的语义；S3 第 19 条的陷阱也随之消失 |
| 4 | `settings.ui` 的五个键 | §8.1 第 1025 行 | 原型还在按风格反馈返工（计划第 9 行），现在把 `palette`、`mascot`、`mode`、`theme` 的取值冻结，是为一个没定的设计付永久成本 | 0.2.0 只占住 `ui` 这个名字（归桌面所有，其他读者忽略），W5b 接入后再定键 | 五个 A 级键 |
| 5 | R 级预留：`watchdog.sock` 与 `\\.\pipe\aproxy-<home_id>-watchdog`、`reload`、`shutdown.drain_secs`、`stream.interleave` | §11 第 1485 行；§4.1、§6.4、§10.2 | 这些名字不会被别人占用，预留不让今天多做到任何事；而 R 级「实现时必须按这里写的形状」会把没经过实现检验的形状变成永久约束 | 从契约里删掉，留在 TODO；以后加入都是加性的 | 四项永久约束 |
| 6 | admit、notify 的细节形状 | §10.9 第 1424–1446 行 | 用户要求「形状写进 0.2.0 协议定稿」，不能删。但 hello 协商已经保证双方都声明之前不会使用它们（format 定稿原话「握手保证以后只加不改」），所以首次实现时修改它们没有兼容成本。0.2.0 真正要冻结的只有协议框架对它们的容纳：允许不需要回信的消息，或者用空回信（S3 第 33 条） | R 级改为「草案，首次实现时可改」；需用户确认 | 一套没经实现检验的永久形状 |
| 7 | `subscribe` 的 `topics` 与 `history` 参数 | §6.4 第 681 行 | 只有两个 topic，界面总是两个都要；历史由 `requests` 分页取，原型的 `queryLogs` 正是这样用的（backend/types.ts:79） | 加参数，用 `features` 声明 | 两个参数与一个 feature |
| 8 | 用户可见的全局 `--home` | §9.1 第 1101 行；§9.2 第 1115 行 | APROXY_HOME 已经能指定 home；`--home` 带来 main 的预扫描，以及与 `--version`、强制安装退出码交叉的情形（S3 第 3 条）。内部入口的 `--home` 保留 | 加旗标 | 一个全局旗标与预扫描逻辑 |
| 9 | InstanceSpec 的 `foreground` | §4.2 | 文件里它恒为 false（S3 第 14 条） | 不需要 | 一个字段 |
| 10 | heartbeat 第 1 拍的完整请求体 | §10.7 第 1402 行 | `client` 对象已足够（S3 第 32 条） | 用 feature 按需给 | 每个请求一次大消息 |
| 11 | 在配置文件里写 `keepalive.trigger = "always"` | §7.1 第 848 行；第 998 行 | format 定稿要「按请求决定是否保活」，所以按请求覆盖需要它；写进配置文件，稿子自己承认会破坏非流式客户端 | 只允许按请求覆盖时用；以后放开是放宽 | 一个容易踩的配置取值 |
| 12 | `set` 的双语法 | §9.3 第 1127 行 | 见 S3 第 26 条 | — | 一套解析器 |

**不建议删**：
- `requests` op：原型的 `queryLogs` 分页要用。
- `buckets`、`by_code`、`rescued_total`：原型的 `spark`、`req1h`、`rescuedTotal` 要用。
- `decide.success`：用户要的「200 但内容是错误」靠它。
- `report`：用户要求。
- `install --check` 与 `--json`：原型的 `checkUpdate`、`installUpdate` 要用。

**合计**：上表减少两个 op、两类事件、一个 IPC 码、两到三个 feature、三个配置键、五个 settings 键、一个全局旗标、四项 R 级预留。

---

## 5 对 §13 三个问题的意见

### Q1　规则表加 `codes` 匹配器
- **是否属于用户决定**：是。它触及用户定过的「网络错误不封顶」（记忆 bounded-retry-paths），也是在 D-6 之上加出口。
- **推荐是否合理**：基本合理，但漏了几项取舍。
  1. **与 S1-1 的关系决定了「现在不加」的真实代价。** 0.2.0 若既不加 `codes`，也不加「必须理解」规则，以后加上 `codes` 时，
     0.2.0 读者会把带 `codes` 的规则读成无条件规则，封顶一切带响应的失败。有了 S1-1，`codes` 就可以放心推迟到 0.2.x，
     旧版本会拒绝启动而不是放宽。
  2. 「format 坏掉时这两条路都会失效」（第 1621 行）不完全对。decide 按程序生效，链上另一个只做决策的程序仍然能封顶；
     只是为封顶另写一个程序的门槛太高，结论不变。
  3. **没列出的方案：format 熔断。** 同一个程序跨请求连续 N 次进程级失败，就标为 `unavailable`；required 程序的请求快速失败
     （502 `format_failed`），不再无限重试。它不需要用户写规则，但改变了 D-6「一律按应重试」的默认语义，所以也要用户定。
  4. 配方措辞：规则表按总尝试数计，表达不了「连续出错 20 次」（S3 第 22 条）。
- **我的意见**：先加 S1-1。`codes` 只是一个匹配器，而确定性 format 故障导致的无限挂起是 D-6 直接带来的，建议 0.2.0 就加。
  若用户不想在 0.2.0 扩大配置面，有 S1-1 兜底也可以推迟。

### Q2　请求体与响应体是否默认在内存里保留
- **是否属于用户决定**：是，取舍在隐私、内存与排障便利之间。
- **推荐是否合理**：理由成立（G1：用户最想看的是界面打开之前的失败），但取舍写漏了四点。
  1. **隐私面写轻了。** 体就是对话全文，包括代码，以及提示词里可能有的凭据与个人信息。默认保留，意味着任何同用户进程
     和界面里的第三方扩展（W5：读不区分权限）都能经 IPC 读到最近 16 MiB 的对话。CSP 挡得住外发，挡不住读。
     稿子只写了「敏感内容在进程内多留一段时间」。
  2. **内存。** 每个实例常驻最多 16 MiB，相对 aproxy 现在的常驻内存是数倍；headless 服务器上开多个实例时按实例数线性增加。
  3. **截断保留开头**，对 agent 的大请求体最没用（S3 第 19 条）。
  4. **漏掉的折中方案：只保留出过失败的请求的体。** 请求体在请求期间本来就在内存或 spool 里，第一次失败时拷一份（不超过 1 MiB）；
     失败尝试的响应体是错误体，通常很小；成功的请求不保留体。它覆盖 G1 的主要场景（点开一条失败的请求看体），
     内存与隐私面都小一个数量级。`bodies` 的取值加一个「这条请求没失败，未保留」即可，开放词表，是加性的。
- **我的意见**：
  - `history.bodies` 改为三值：`off`、`failed`、`all`。不用布尔，给这个折中留位置；默认 `failed`。
  - 若用户想要最简单的形状，就默认关闭，界面里提供一键打开。
  - 不论选哪个，打码按 S2-9 第 3 条补上已知密钥替换。

### Q3　0.1.x 旧键：告警还是拒绝启动
- **是否属于用户决定**：是。拒绝启动推翻了 D-1 与 format 定稿里「未知键告警里提示」的原话。
- **推荐是否合理**：方向对，前提错。详见 S1-3，逐键结果见 §6。
- **我的意见**：
  - 改问「全部旧键一律报错，还是一律告警」，推荐一律报错并附新写法。
  - 配套三件事：`unset` 能删旧键（S2-15）；迁移步骤加「重装后先跑 `aproxy doctor`」；报错信息指明出处是 config.toml 还是
    settings.json。settings.json 是全局的，一条旧键会让所有实例都起不来，信息不说清楚就难查。
  - 看护者重拉的实例同样会因报错起不来，但 0.1.x 的期望状态记录本来就不被 0.2.0 读取，不会出现重拉风暴。

---

## 6 迁移：0.1.x 的每个旧键在 0.2.0 得到什么

依据：`git show v0.1.0:src/config.rs` 有 25 个配置键，`:src/settings.rs` 有 20 个字段。「开发线」表示 0.1.0 之后才加、只有开发构建的用户才有的键。
「风险」指留着不改时的后果。

**config.toml**

| 旧键 | 0.2.0 的去处 | 留着不改 | 风险 | 稿子是否覆盖 |
|---|---|---|---|---|
| `base_url`、`listen_addr`、`api_key`、`extra_headers`、`override_headers`、`proxy`、`proxy_username`、`proxy_password`、`disk_cache`、`forward_only`、`allowed_hosts`、`allowed_origins`、`log_file` | 不变 | 照常 | — | — |
| `max_retry_backoff_secs` | `retry.max_backoff_secs` | 回到 320 | 低 | 对照表有 |
| `connect_timeout_secs` | `timeouts.connect_secs` | 回到 30 | 低 | 有 |
| `read_timeout_secs` | `timeouts.first_byte_secs` 与 `read_idle_secs` | 回到 300 与 300 | **高**：长时间非流式生成每次都超时、无限重试 | 有，Q3 没列 |
| `keepalive_interval_secs` | `keepalive.interval_secs` | 回到 15（原来的 0 表示关闭） | 中 | 有，Q3 没列 |
| `keepalive_trigger` | `keepalive.trigger` | 回到 `any` | 中 | 有，Q3 没列 |
| `keepalive_heartbeat`（开发线） | `keepalive.heartbeat` | 回到 `: keepalive\n\n` | 中：空行心跳的变通失效 | 有，Q3 没列 |
| `max_body_mb` | `max_request_mb` | 回到 128 | 低到中（413 看得见） | 有 |
| `spool_limit_mb` | `max_response_mb` | 回到 256 | 低 | 有 |
| `bounded_retry_paths` | `[[retry.rules]]` | 无限重试 | 高 | Q3 |
| `[request_transform]`、`[response_transform]` | `[[format]]` | 转换消失 | 高 | Q3 |
| `[heartbeat_transform]`、`every_attempt`（开发线） | `[[format]]` 的 `stages` | 同上 | 高 | Q3 |
| transform 段内的 `command`、`args`、`mode`、`pool_max`、`timeout_secs`、`idle_timeout_secs`、`extra` | 随段迁到 `[[format]]` | 随段失效 | — | 对照表只列了 `idle_timeout_secs` 与 `extra`，同名迁移的几个没写 |
| `upstream_url`（0.1.0 之前） | `base_url` | base_url 为空，启动失败（看得见） | 低 | 有 |

**settings.json**

| 旧字段 | 0.2.0 的去处 | 留着不改 | 风险 | 稿子是否覆盖 |
|---|---|---|---|---|
| `aliases`、`default_config`、`config_dirs` | 不变 | 照常 | — | — |
| `watchdog`（布尔） | `watchdog.enabled` | 同名换了类型，false 作废，看护者重新开启 | 中 | 只有第 1042 行一句 |
| `watchdog_heartbeat_secs`、`watchdog_stale_after_cycles`、`watchdog_max_restarts`、`watchdog_idle_exit_secs` | `watchdog.*` | 回默认 | 低 | 「见样例」 |
| `log_rotate_mb` | `logs.rotate_mb` | 回到 8 | 低 | 有 |
| `idle_timeout_secs` | `instances.idle_threshold_secs` | 回到 1800 | 低 | 有 |
| `download_chain`、`skill_auto_update` | `update.*` | 回默认 | 低 | 有 |
| `download_proxy` | `update.download_proxy` | 下载不再走代理 | 中：需要代理才能访问 GitHub 的网络里 install 失败 | 有，Q3 没列 |
| `max_body_mb` | `defaults.max_request_mb` | 回到 128 | 低 | 「等」 |
| `disk_cache` | `defaults.disk_cache` | false 作废，请求体重新落盘 | **中到高**（隐私） | 没列 |
| `forward_only` | `defaults.forward_only` | true 作废，数据路径变成缓冲加重试 | **高** | 没列 |
| `bounded_retry_paths` | `defaults.retry.rules` | 无限重试 | 高 | Q3 |
| `allowed_hosts`、`allowed_origins` | `defaults.*` | 回到 []，会多拒绝，403 看得见 | 低 | 没列 |
| `keepalive_trigger` | `defaults.keepalive.trigger` | 回到 `any` | 中 | 没列 |
| 没有 `schema` 字段 | `schema: 1` | 见 S3 第 25 条 | — | 没定 |

**是否清楚、可操作**：
- **不够清楚。** §7.5 的对照表只覆盖 config.toml，settings.json 只有一句「等」，而稿子说对照表要写进 CHANGELOG。
- **提示不一定看得到。** 告警只出现在 start 的 stderr、实例日志、doctor 与 `config show`（第 961 行）；看护者重拉的实例只在
  日志里留下告警，稿子自己也承认了（第 1639 行）。迁移步骤应加一步「重装后先跑 `aproxy doctor`」。
- **用命令改不完。** 新键可以用 `set` 写，旧键却删不掉（S2-15）。
- **0.1.x 的运行期残留没人清理。** `run/*.pid`、`run/watchdog.claim`、0.1.x 随机名的日志、/dev/shm 的心跳文件，0.2.0 都不认也不清，
  因为读者规则是不删看不懂的东西。CHANGELOG 迁移第 4 步只提了 install.state、staging 与 `.restore`，应补上这几项。

---

## 7 缺口：界面、agent、W3、W4 要用，稿子却没定的

以 UI 原型的后端接口为准，它代表 W5 实际要什么。

**界面（W5）**
1. **置顶**：原型有 `setPinned`（backend/types.ts:82），mock 淘汰旧条目时跳过置顶的（mock/engine.ts:328），稿子没有对应的 op。
   置顶会让实例保留超出 `history.requests` 的记录，是改实例状态的写操作。两个办法：定为界面侧功能，界面把置顶条目连同已经取到的
   体存到 `<home>/desktop/` 下；或者以后加一个 op，这是加性的。要在 §6 里写明选哪个。
2. **今日救回**：原型有 `rescuedToday`（backend/types.ts:39），快照只有自启动以来的 `rescued_total`，跨重启的「今日」要持久化统计。
   要么界面去掉这一项，要么稿子给出数据源。
3. **更新信息**：原型要 `publishedAt` 与多语言的 `notes`（types.ts:285-287），`install --check --json` 只有 current、latest、
   update_available、channel、variant（第 1158 行）。用户定了 CHANGELOG 保持中文，英文发布说明从哪里来没有定。
4. **安装进度**：原型的 `UpdateProgress.targets`（开始时涉及的实例，types.ts:300）在 NDJSON 里没有；NDJSON 的相位词表
   （第 1162–1165 行）与原型的 download、verify、notify、swap、restart、done 怎样对应，也要写明。
5. **扩展授权与开机自启存在哪**：扩展授权「由后端保存」（types.ts:311），还有 `autostart`（types.ts:272）。settings.json 的
   schema 里都没有，稿子又否决了「把界面偏好放进单独的文件」（第 1062 行）。授权是安全相关的状态，要写明位置与读写规则。
6. **短序号**：原型要显示 `seq`（types.ts:85），稿子定了 request_id 是不透明串、不得解析（第 111 行）。界面要显示短序号，
   记录里就得单独给一个 `seq` 字段。
7. **转换器信息**：原型的 `TransformInfo.keys`、`channels`（types.ts:29-31）是 format 的内部配置，aproxy 不知道。
   要么 hello 回信里给 format 一个可选的 `info` 对象，要么界面不显示。
8. **实例的增减**：IPC 是每个实例一个端点，home 级端点只是预留，所以新实例起来、实例退出都没有事件。0.2.0 的界面只能轮询
   enumerate 或监视 run/，要写明用哪种、多久一次。§4.3 第 4 条已经预期了「界面轮询」。
9. **界面拉起实例用哪个可执行文件**：§5 写「用拉起方自己的可执行文件」（第 573 行）。CP-19 与综合稿 U6 建议界面一律用
   `<home>/bin/aproxy`：从 npm 全局目录或下载目录启动的界面，拉起的实例日后会被 install 拒绝管辖（flow.rs:44-48 的 `check_jurisdiction`）。
   §12 的对账表里没有 CP-19 与 U6。
10. **前端的码表**：B0 只说码表渲染给 docs 与 skill。W3 倾向前后端共用一套目录，界面也要拿到码与 `details` 的键，
    所以 B0 要同时导出一份 JSON 码表。

**agent 与 skill**

11. **没有查看请求历史的 CLI**：`requests`、`request` 两个 op 只有界面在用，agent 只能 grep 日志里的 `request_id=`。0.2.0 可以不做，
    但 skill 要写明这条路。以后加 `aproxy requests --json` 是加性的。

**W4**

12. **`skill` 命令的形状与本地清单**：稿子写「由 W4 定」（第 1099 行）。`<home>/skills/manifest.json` 列在布局里（第 440 行），
    却没有级别与 schema。它由一个版本写、另一个版本的 `skill list` 读（比较 skill 版本与二进制版本），是跨版本文件，需要 `schema`
    与读者规则。`skill list --json` 的形状也没有，§9.5 的退出码表里没有 skill 这一行。

**W3**

13. **`details` 键的类型**：除了码表，W3 还要知道每个 `details` 键的类型（字节数、毫秒、路径、字符串数组），才能按类型格式化。
    §2 说元数据里有「`details` 的键」，建议连类型一起登记。

---

## 8 可行性逐项判断（派单第 3 条点名的几项）

| 项 | 判断 | 依据或困难 |
|---|---|---|
| Windows 命名管道的显式 DACL 与 peer 核对 | 可行，风险低 | tokio 1.53.1 有 `ServerOptions::create_with_security_attributes_raw`（named_pipe.rs:2304）。SDDL 用 `ConvertStringSecurityDescriptorToSecurityDescriptorW` 生成，需要 windows-sys 的 `Win32_Security_Authorization` 特性。首个实例与后续实例都要带上。客户端用 `GetNamedPipeServerProcessId` 核对服务端 pid。同一用户的提权进程 SID 相同，可以连上；AppContainer 进程会被拒，正合预期 |
| unix 的 peer uid 核对 | 可行 | tokio 的 `UnixStream::peer_cred`：Linux 用 SO_PEERCRED，macOS 用 getpeereid 加 LOCAL_PEEREPID（ucred.rs）。另：root 是否放行要写明（S3 第 35 条） |
| config.toml 写回保留注释（toml_edit） | 可行 | toml_edit 0.22.27 已经经由 toml 0.8.23 在 Cargo.lock 里，R2b 升级 toml 后版本随之变。要处理同一个键的三种写法：`[timeouts]` 表、点号键、内联表。数组表整体替换时，原有条目里的注释会丢，可以接受 |
| config.toml 与 settings.json 的 CAS | 照写不可行 | 比对与 rename 之间没有锁（S2-3）。用 C4 同款锁原语串行化即可 |
| 内部子命令、BREAKAWAY 回退 | 可行 | 先带 `CREATE_BREAKAWAY_FROM_JOB`，被拒（ERROR_ACCESS_DENIED）就去掉重试，这是标准做法。回退之后实例仍在作业对象里，应在 startup.log 记一条告警，现在没写 |
| unix 进程组 | 照写不可行 | 见 S2-1 |
| `seq` 回显 | 可行，成本低 | — |
| 洋葱链 | 可行，但有两处没定义 | 同一程序并发阶段对 state 的竞争（S2-6）；缓冲模式下 response 与 stream 两种形态在链上混用时，aproxy 要累积、拆分字节，复杂度集中在 B10 |
| 流 worker | 可行 | 不计入 pool_max 意味着并发的长流有多少条，就有多少个进程；回收规则没写（S3 第 31 条） |
| 请求历史与事件流对数据面的影响 | 有风险 | 历史不论有没有订阅者都要维护：每个请求更新共享结构，保留体时还要拷贝最多 1 MiB，落盘的请求体要回读。事件「没有订阅者时不构造」是对的，但有订阅者时 `request` 事件不限频，整条重发是 O(n²)（S1-2）。B8 的验收没有阈值（S2-16 (c)） |

---

## 9 安全问题的逐项回答（派单第 6 条）

- **请求历史保留体时，密钥会不会随体或请求头经 IPC 泄露**：
  - 请求头按名字打码，覆盖面够：`x-api-key`、`authorization`、`api-key`、`x-goog-api-key` 都会命中。
  - 路径里的查询值会打码；`RequestDetail.attempts[].sent.url` 也应写明按 URL 规则打码，现在只写了「路径的查询值」。
  - 体不打码，见 S2-9 第 3 条。
  - 打码时机建议放在写入记录的时候，而不是输出的时候：内存里的历史从此不含明文，以后新增的出口也不会漏。
- **写类 op 的访问控制**：
  - 「端点访问控制加 Tauri 桥接层同意门」与用户的信任模型一致：同用户互信，扩展只在写操作上需要同意。
  - 补两点。一是桥接层只暴露 start、stop、restart、写配置、install 这几类写操作，与原型的 `WriteCapability` 一致，
    永不暴露原始的 `prepare_swap` 与带任意参数的 `shutdown`。二是 root 是否放行（S3 第 35 条）。
- **错误体会不会带出上游的敏感信息**：`details.reason` 会带出 URL，见 S2-9 第 2 条。上游错误体原样透传是现行行为，不属于 aproxy 的出口。
  入站拒绝时 `details.host` 回显的是客户端自己发来的值，无害。
- **format 信封有没有泄露密钥**：信封必然把 key 交给 format，format 是本机受信的子进程。泄露的路径有四条：
  - stderr 进日志（S2-9 第 1 条）；
  - `report` 与 `fail.reason`：稿子已对 aproxy 知道的密钥做替换，format 自己配置里的 key 替换不到，只能靠协议文档约束；
  - 信封整条进日志；
  - `state` 经 IPC 输出。

  后两条稿子没禁止，按 S2-9 写明「永不」。

---

## 10 实现切分（§14）的修订建议

1. **先排期前置**：给 S1–S3、R1、R2b 排期并定负责人（S2-16 (a)）。
2. **缩小 B0**：只放码表（含 HTTP 状态、退出码、`details` 的键与类型，并导出 JSON）、redact、端点名与 home_id，以及按 S2-16 (b) 的选择
   迁入的 C3 读者。各线上类型随消费它的块落地，golden 样本也随之入库，免得先写样本、实现时再改样本。
3. **拆分 B10**：拆成协议核心（hello、`seq`、链、request、response、heartbeat 迁到协议 1，fail、overrides、report）、decide、
   stream（缓冲回放与仅转发两种模式）三块，B11 跟着拆。W2c 的转换正确性仍是 B11 之后的大头。
4. **补依赖**：B3 依赖 B2；B6、B8 依赖 B1。
5. **验收补充**，每条都能真的暴露缺陷：
   - S1-1：规则带未知键时启动失败。
   - S1-2：长重试下内存有界。
   - S1-4：restart 遇到写错的配置时旧实例不动；restore 遇到暂时缺失的配置时不删记录。
   - S2-2：Windows 上只设了大写 `HTTPS_PROXY`，restart 后仍走代理。
   - S2-4：启动 9 秒的 format 不会被误判为就绪超时。
   - S2-5：`start && stop` 之后不会被拉回。
   - S2-6：心跳与 decide 并发时 state 不丢。
   - B7 的跨用户测试改为必做；B8 的 loadgen 定阈值。

---

## 附录 A　「现状」引用核对（子代理只读核对，ebb4572 对照 HEAD）

28 处引用中，没有完全不属实的；8 处部分属实，都不影响设计结论：
- **§4「`.restore` 存的是 argv」**：实际是 `{args, log_path}`，`args` 是去掉 `--daemon-child` 与 `--foreground` 之后的启动参数
  （daemon.rs:1074-1078、server.rs:165-169）。实质相同。
- **§4「status 普查顺带删」**：实际删得更多，还会删已死或已被复用的 `.pid`，以及解析不了的 `.restore`（daemon.rs:744-882、1109-1114）。
- **§4 心跳路径**：拼 /dev/shm 路径的代码在 watchdog.rs:1432、1460、1476，不在 1087-1106。
- **§6「last_error 是字符串（daemon.rs:481）」**：线上是 `{message, at}` 对象（daemon.rs:210-216），与 §2 的现状描述自相矛盾。
  「v1 只有三个 op」在 daemon.rs:223-255，不在 172-182；服务端的中文消息也不在 291-311（那里是客户端侧）。
- **§7「26 个平铺键」**：Config 有 26 个字段，其中 25 个是配置键，另一个是测试注入用的 `spool_dir_override`。
- **§8「settings.rs:673」**：那是一行测试注释，真正的逻辑在 381-392。现行实现对一切解析失败都拒绝写回，不只是语法错误；
  稿子有意收窄为只对语法错误拒绝，配合逐字段容错，这个改动合理。
- **§9 cli.rs 与 main.rs 的行号**：`--config` 在 16-17 行，`--log-file` 在 40-41 行，`Upgrade` 在 179 行；main.rs:178 在 HEAD 上是 183。
- **基线行号**：稿子说现状行号对应 ebb4572，但 server.rs 用的其实是 276a65e 的行号，两者差 3 行。

其余属实，例如：`note_upstream_failure` 恰好 20 个调用点；`spool_limit_mb.max(1)`；管道用默认安全描述符；`mask_secret` 保留前 6 个字符；
home_id 是对规范化的 run 路径做 SHA-256 后取前 8 字节。
