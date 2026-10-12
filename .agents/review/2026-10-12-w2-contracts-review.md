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

---

## 第二轮复审（2026-10-12）

**对象**：`.agents/plan/w2-contracts.md` 第二版的工作副本（未提交，2016 行；稿内现状行号对应 73b7863）。本节的「第 N 行」都指第二版。

**依据**：
- 本文件第一轮的 57 条（S1 4、S2 16、S3 37）；
- 计划 `release-0.2.0.md` 的 W2 用户决策与 D-12…D-16（61–90 行），format 定稿、定稿补充与 W2c（92–183 行）；
- team-lead 转达的三条指示：命名（`delivery`、`behavior`、`process`）、D-13 的细化、D-16。

**方法**：
- 通读全稿。
- §15 是对本文件的逐条交代，逐条对照正文核对，不采信它的自述；§12 是对 10-11 综合稿的对账，抽查。
- 现状引用用 `git show 73b7863:<路径>` 抽查了 45 处。
- 永久契约对照 AGENTS.md 的 C1–C8 段与 `tests/force_contract.rs`。
- 只读：没有跑 cargo，没有起进程，没有改设计稿。

**编号**：设计稿把本文件第一轮叫作「第二轮审查」（它把 10-11 的综合稿算作第一轮），两边的 S1-n、S2-n、S3-n 编号一致。本节的新问题记作 R-S1-n、R-S2-n、R-S3-n。

### 结论

- 第一轮 57 条：**已解决 47，换了做法且理由成立 10，未解决 0，退化 0**。§15 的每条自述都能在正文里找到对应。
- D-12…D-16、format 定稿与三条指示都写进了正文，不只停在 §13、§15：
  - `mode` 不再作字段名；
  - `forward_only` 改为 `delivery`，默认 `retry`，不留旧名；
  - 4 MiB 的单体上限与头尾截断已删，`body` 改为分块读取。
- 新问题 23 条：R-S1 1 条，R-S2 4 条，R-S3 18 条。最要紧的三条：
  - **R-S1-1**：§7.2 的推荐配方把默认投递方式设为 `forward`，重试全靠可选的 adaptive。adaptive 一熔断，整台实例就不再重试。稿子说这时「走默认模式与静态兜底」，而 forward 根本没有静态兜底。
  - **R-S2-4**：从 alpha.1 早交接时，`install_unmanaged_instances` 的检查落空，alpha.1 启动的实例会变成没人管的孤儿。
  - **R-S2-2**：入库时的已知密钥替换没有考虑块边界与压缩体。上游回的常是压缩体，这时替换无效，界面也只能显示二进制。

### 一、第一轮 57 条的状态

| 编号 | 状态 | 证据位置（第二版行号） |
|---|---|---|
| S1-1 | 已解决，并由 D-14 加强为「一切未知键报错」 | §1.1:85-89；§7.3:989；§7.5:1052；§8.2:1122；B2:1821 |
| S1-2 | 已解决 | §6.5:792-796；§6.7:876；B8:1839-1842（截断顺序见 R-S3-4、R-S3-5） |
| S1-3 | 换了做法且理由成立：D-14 让旧键一律报错，不再有「悄悄改变行为」 | §7.5:1052；§15:1865、1939-1941（遗留的 settings.json 见 R-S3-17） |
| S1-4 | 已解决 | §4.2:560-563；§5:662-668、676；§6.6:854、862；§9.4:1238；B5:1824 |
| S2-1 | 已解决 | §5:638 |
| S2-2 | 已解决 | §4.2:551-555；B5:1824 |
| S2-3 | 已解决 | §7.6:1061-1064；§4.1:505-506；§4.2:532 |
| S2-4 | 换了做法且理由成立：先 bind 代理端口，以 `starting` 应答，时限 30 秒；否决方案 ② 的理由（端口 0）成立 | §5:640-655、688-689（端口 0 的另一个问题见 R-S2-3） |
| S2-5 | 已解决 | §5:652 |
| S2-6 | 已解决（方案 ②） | §10.3:1353；B10a:1829 |
| S2-7 | 换了做法且理由成立：用户改定为 D-16 | §7.1:906-910；§7.5:1041-1054；B3:1822（余量见 R-S3-9） |
| S2-8 | 已解决 | §1.6:247-253；§4.2:558、576（余量见 R-S3-11、R-S3-12） |
| S2-9 | 已解决 | §1.5:198、212-231；§10.1:1291；§2.2:316（体的替换见 R-S2-2） |
| S2-10 | 已解决 | §1.5:205-208 |
| S2-11 | 已解决 | §9.1:1184；B9:1828 |
| S2-12 | 已解决 | §2.4:381-391；§9.4:1236-1238 |
| S2-13 | 已解决 | §1.3:146-147；§6.5:784-786 |
| S2-14 | 换了做法且理由成立：D-15，默认时序不变 | §7.3:998-1002；B4:1823（样例里的残留见 R-S3-1） |
| S2-15 | 已解决 | §7.6:1067-1068；§9.3:1207 |
| S2-16 | 已解决：(a) P1–P5 写进 §14.1，并定为 W2 的起点；(b) C3 的读者迁入 aproxy-ipc；(c) B8 有可证伪的阈值 | §14.1:1803-1813；§6.6:869-870 与 B0:1819；§14:1836-1845（(b) 的措辞矛盾见 R-S3-15） |
| S3-1 | 已解决 | §2:282-283；§2.4:369 |
| S3-2 | 已解决 | §2.4:354 |
| S3-3 | 已解决 | §2.4:355；§9.1:1182 |
| S3-4 | 换了做法且理由成立：D-14 之后只有 error 一级 | §2.4:403 |
| S3-5 | 已解决 | §2.4:368；§6.4:774 |
| S3-6 | 已解决 | §10.4:1397-1401 |
| S3-7 | 已解决 | §3:451 |
| S3-8 | 已解决 | §3:462-465 |
| S3-9 | 已解决，偏移已与 announce.rs 核对 | §4.1:518-519 |
| S3-10 | 已解决 | §4.1:497、514；§4.3:592 |
| S3-11 | 已解决 | §4.1:523-524 |
| S3-12 | 已解决 | §4.3:590；§6.6:856、864；§9.4:1231 |
| S3-13 | 已解决 | §4.3:585；§5:659 |
| S3-14 | 已解决 | §4.2:537、547 |
| S3-15 | 已解决 | §4.3:586 |
| S3-16 | 已解决 | §5:648 |
| S3-17 | 换了做法且理由成立：单独的 `body` op，分块读取 | §6.1:706；§6.3:744；§6.5:834-843 |
| S3-18 | 已解决 | §6.5:786、803-806 |
| S3-19 | 换了做法且理由成立：按 D-13 的细化，体整份保留、不截断 | §6.5:830-832 |
| S3-20 | 已解决 | §7.1:930-931；§10.4:1383 |
| S3-21 | 已解决 | §7.1:932-934 |
| S3-22 | 已解决 | §7.3:984、992；§6.5:805 |
| S3-23 | 已解决 | §7.5:1040 |
| S3-24 | 已解决 | §7.4:1030；§10.10:1574 |
| S3-25 | 换了做法且理由成立：缺 schema 按 1 读，更高的 schema 按 D-14 报错 | §8.1:1104；§8.2:1123 |
| S3-26 | 已解决 | §9.3:1203-1205 |
| S3-27 | 已解决 | §9.2:1193 |
| S3-28 | 已解决 | §9.1:1186；§9.3:1210 |
| S3-29 | 已解决 | §10.2:1335-1339 |
| S3-30 | 已解决 | §10.5:1422 |
| S3-31 | 已解决 | §10.6:1470-1473 |
| S3-32 | 已解决 | §10.7:1493 |
| S3-33 | 换了做法且理由成立：保留空回信；notify 按用户追加提前到 0.2.0 实现 | §10.11:1587、1598、1613 |
| S3-34 | ①②④ 已解决；③ 换了做法且理由成立：加 `reroute`，并规定 retry 程序排在后段 | §10.1:1296；§10.14:1658；§10.2:1337-1338；§10.5:1429-1431；§7.4:1025；§9.1:1159、1275（③ 的副作用见 R-S3-2） |
| S3-35 | 已解决 | §1.6:256 |
| S3-36 | 已解决 | §4.1:517 |
| S3-37 | 已解决 | B10 拆为 B10a–d（1829-1832）；B3 依赖 B2（1822）；B6、B8 依赖 B1（1825、1827）；B6 改用 userinfo（1825）；跨用户测试必做（1826）；A5 归 B9（1828） |

第一轮的其余几节：
- **§4「建议删减或推迟」**：12 项里 11 项采纳。第 6 项（admit 与 notify 改为草案）没采纳，理由是用户要求把形状写进 0.2.0 的定稿（计划 135 行），这个理由成立。
- **§7 的 13 个缺口**：都有了着落（§15:1943-1956），逐项与正文对得上。

### 二、新问题

#### R-S1-1　推荐配方让无限重试依赖一个可选程序

- **位置**：§7.2 第 940、955 行；§10.9 第 1524 行；§10.14 第 1647、1652 行；§15 第 1983 行；B11 第 1833 行。
- **问题**：
  - 样例把默认 `delivery` 设为 `forward`。按请求转 retry、失败时升级，都只靠 adaptive，而它配的是 `required = false`、`pool_max = 1`。
  - adaptive 不可用时：
    - request 阶段跳过它，请求走默认的 forward（1524 行）；
    - decide 视为不表态，forward 请求的 fallback 是 stop（1420 行），所以不升级；
    - 结果是这台实例上的请求一律不重试，客户端直接拿到上游的 5xx。
  - 让它不可用的情形不少：
    - 启动握手失败，例如 `adaptive.toml` 写错了，只换来一行告警，熔断器以 open 起步（1339 行）；
    - decide 超时，时限 2 秒，计入熔断（1434 行）；
    - notify 的回信超时，同样计入熔断（1602 行）。
  - 越是上游大面积出错，越容易熔断：失败越多，decide 消息越多，全挤在一个 worker 上。只要排队时间计入时限（R-S2-1 没定），连续 5 次超时就熔断了。最需要重试的时候，重试被整体关掉。
  - 第 1652 行与第 955 行说熔断之后「请求走默认模式与静态兜底」。可 forward 请求不进重试循环（§7.3 第 981 行），规则表与退避都不生效，这个配方下根本没有静态兜底。
- **为什么要紧**：
  - aProxy 的使命是无限重试；用户也补充过「对于无限重试依旧保持默认」（计划 75 行）。
  - 判断类单元配 `required = false`，前提是「坏了就走默认模式」（计划 159 行）。默认模式若是 forward，这条退路就不再安全。
  - 这份样例是 B11 的验收对象，也会成为 `init` 生成的起步配置。
- **改法**：协议机制不用改，要改的是配方，以及 §10.9、§10.14 的表述。请用户拍板：
  - **推荐**：样例与 `init` 沿用 §7.1 的默认 `delivery = "retry"`，由 adaptive 对没失败过的请求回 `forward`。
    - adaptive 坏了，一切回到 retry，只失去真流式。
    - 这仍是用户说的「先仅转发、失败多次再无限重试」，只是判断的方向反过来写。
  - **若保留 forward 作默认**，二选一：
    - 照实写：§10.9、§10.14 写明「adaptive 不可用期间，请求一律不重试」；doctor 对「默认 forward，而能升级的程序都是可选的」给出告警；
    - 或者在冻结之前给 §10.9 加一条 A 级语义：声明了 request 阶段的程序因熔断或失败被跳过时，这个请求按 retry 投递。
- **性质**：推理，链上每一步都有上面引的行号。

#### R-S2-1　notify 与请求路径共用 worker，「绝不拖慢数据通路」不成立

- **位置**：§10.11 第 1597–1602 行；§10.1 第 1292–1294 行；§10.3 第 1355 行；§7.4 第 1015–1018 行；§10.14 第 1647 行；B10d 第 1832 行。
- **问题**：
  - **worker 占用没定义**：adaptive 在一个程序里同时声明 request、decide 与 notify，并配 `pool_max = 1`。
    - §10.1 的「一问一答」没写：一条消息的回信还没到时，这个 worker 算不算忙。「aproxy 不等它」也没说 notify 在途时能不能再写下一条。
    - 不论哪种答案，程序都按顺序读 stdin，排在在途 notify 之后的 request、decide 都要等它处理完。
  - **计时起点没定义**：`timeout_secs`、`decide_timeout_secs` 从入队还是从写出开始计时，没写。
    - 从入队计时：并发一高，decide 就排队超时，并计入熔断，这又接上了 R-S1-1。
    - 从写出计时：排队没有上限。
  - **观察拖累判断**：notify 的回信超时计入同一个熔断器（1602 行），观察路径一慢，判断路径就跟着被关掉。
  - **跨请求状态会丢**：第 1355 行说它「只在 pool_max = 1 时可靠」，这还不够。以下几种情况都会换掉 worker，状态随之丢失：
    - `worker_idle_secs` 默认 300 秒，闲置的 worker 会被回收；
    - 熔断时「杀掉这个程序的空闲 worker」（1518 行）；
    - 协议错误会剔除 worker（1294 行）；超时之后 worker 怎么处理，没写。
  - **验收缺口**：B10d 只测了「独立的 notify 程序从不读 stdin」，没有覆盖 §7.2 那种共用 worker 的链。
- **为什么要紧**：
  - 「只发不等，绝不拖慢数据通路」是 A 级的保证（1597 行），它最主要的用户 adaptive 却正好不满足它。
  - 用户拆单元的理由，正是「全靠单个进程在超高频时容易阻塞」（计划 157 行）。
- **改法**：
  - **占用与计时**：
    - 消息写到 worker 上才开始计时；
    - notify 在途时 worker 算忙；
    - 有请求路径的消息在排队时，不派发 notify。
  - **notify 与熔断解耦**：notify 用短时限，例如等于 `decide_timeout_secs`；它的超时不计入熔断，或者单独计数、只影响 notify。
  - **跨请求状态**：§10.3 写明它要能承受 worker 重启；adaptive 的样例配 `worker_idle_secs = 0`（§1.2：0 表示不限）。
  - **验收**：B10d 加一条：§7.2 的链在 64 个并发连接下，吞吐与 p99 满足 B8「没有订阅者」的阈值。
  - **以后（加性）**：hello 的 `features` 可以协商「一个 worker 上同时有多条在途消息」。`seq` 已经能把回信对上，单进程的程序可以自己开线程，`pool_max = 1` 也就不再是瓶颈。
- **性质**：推理。

#### R-S2-2　入库时的已知密钥替换：块边界与压缩体

- **位置**：§1.5 第 212–225 行；§6.5 第 826–843 行；B8 第 1844 行。
- **问题**：
  - **块边界**：forward 请求「边转发边复制」，响应体也是分块到的。已知密钥被块边界切开时，逐块替换看不见它，稿子没写重叠窗口。
  - **压缩体**：
    - aproxy 原样转发客户端的 `accept-encoding`，reqwest 又关着自动解压，所以上游回的常是压缩体。记忆 2026-09-14-compressed-body-inspection 记过一例：Cloudflare 回的 brotli 404 页在日志里反复出现。
    - §6.5 没写入库的是线上的原始字节，还是解码后的字节；而 §10.5、§10.6 交给 format 的内容，都写明了是解码后的。
    - 存原始字节，替换对压缩体完全无效，界面也只能把体显示成二进制。
  - **长度口径**：替换会改变长度。`bytes`、`total_bytes` 与记录里的 `request_bytes`、`response_bytes` 各按哪种口径算，没写。
- **为什么要紧**：
  - §1.5 第 216 行把「内存里的历史本身就不含明文」当作不变量，用户定的「IPC 永不返回明文」就靠它，现在的写法保证不了它。
  - 用户要的日志详情要「能深入查看请求体」（记忆 ui-design-style），遇到压缩体也会落空。
- **改法**：
  - **存解码后的字节**：响应体先解码再入库；解码失败时标出 `encoding` 并不保留它（`not_retained`，另加一个 `cause`）。
  - **按流替换**：留住「最长已知密钥的长度减 1」那么多字节的尾巴，等下一块到了或流结束再一起发布。进行中的体的 `total_bytes` 因此略为滞后，写明这一点。
  - **口径**：体对象的 `bytes` 与 `body` 的 `total_bytes` 是入库后的字节数，也就是解码、替换之后的；记录里的 `request_bytes`、`response_bytes` 是线上的字节数。
  - **验收**：B8 加两个用例：密钥跨两块；gzip 与 br 的响应体里带着密钥。
- **性质**：压缩体部分已确认（见上述记忆的取证），块边界部分是推理。

#### R-S2-3　端口为 0 的后台实例，期望状态记录会越恢复越多

- **位置**：§4.2 第 560–563 行；§5 第 648、652、667 行；§15 第 2012 行。
- **问题**：
  - 期望状态记录按实际端口命名（`run/<port>.restore.json`），spec 里存的却是配置的 `listen_addr`，端口为 0。
  - 实例在端口 N1 上崩溃之后：
    - 看护者或 restore 按 spec 拉起，新实例绑到另一个端口 N2，写下 `run/N2.restore.json`；
    - `run/N1.restore.json` 只有 stop 能删，而端口 N1 永远是 down；
    - 于是下一轮再拉起一个。
  - restart 与 install 的滚动重启也一样：`keep_restore` 保住了 N1 的记录，新实例却在 N2。
  - 强制安装第 6 步按端口判断实例回没回来，也会因此误报。
- **为什么要紧**：
  - 看护者每轮多拉起一个实例，直到资源耗尽。
  - 稿子明确支持端口 0：第 648 行在 bind 之后重写台账，第 689 行拿端口 0 作否决方案 ② 的理由。
- **改法**，二选一：
  - 后台实例不许用端口 0，报 `config_invalid_value`，只有 `--foreground` 可以用；
  - 或者第 7 步写期望状态记录时，把实际地址钉进 `spec.overrides.listen_addr`，此后 restart、restore 与看护者都用同一个端口。这也合 D-8 的本意：要恢复的是「那个端口上的实例」。
- **性质**：推理。

#### R-S2-4　从 alpha.1 早交接时，「管不了的实例」的检查落空

- **位置**：§4.3 第 593–599 行；§5 第 621、629–630 行；B9 第 1828 行。
- **问题**：
  - **检查时机已过**：
    - §4.3 第 7 条只规定「普通 install …在改动任何东西之前失败」。
    - 从 alpha.1 升级时，前几步由 alpha.1 的安装者执行：管辖检查，加锁与宣告，下载与校验。做完这些，它才以 `install --continue --handover-from <pid>` 拉起目标版本（73b7863 的 flow.rs:4-17、362-368、445）。
    - alpha.1 不知道这条检查。等 W2 的二进制接手时，「改动任何东西之前」早已过去。
  - **不检查的后果**：稿子没要求续作入口（对应 flow.rs:697 的 `continue_install`）做这条检查。那么 W2 会照常走完广播、交换、滚动、终验与清理，而 alpha.1 的实例：
    - 不在 W2 的实例枚举里，只出现在 `processes` 里（§6.6 第 866 行），所以不会被重启；
    - 已经失去看护者：换二进制之前，install 按台账结束了旧看护者（672 行）；
    - `stop all`、`restore` 与 W2 的看护者都不管它们，崩溃了也没人拉起。
  - **skill**：交棒之后，skill 由还在等待的 alpha.1 安装者用 alpha.1 的代码安装（flow.rs:754，`skill_should_run = !named`），它不写 `skills/manifest.json`。
    §4.2 第 579 行只规定了清单解析失败或 schema 更高时怎么读，没规定「有 skill 目录、没有清单」。
- **为什么要紧**：
  - alpha.1 → W2 是每个 alpha.1 用户都要走的路。§5 保留这个入口，理由正是这条路要走得通。
  - B9 分别测了「交棒」与「普通安装遇到管不了的实例」，两者叠在一起的那条路径没人测。
- **改法**：
  - §4.3 第 7 条改为「普通 install 与续作都在广播之前检查」。W2 接手时先查台账，有管不了的活实例就：
    - 置 `phase = failed`、`error_code = install_unmanaged_instances`；
    - `last_error` 写英文原因，alpha.1 的等待者只读这个字段；
    - 释放锁与宣告，不动 bin。
  - B9 加一个用例：模拟 alpha.1 的安装者交棒，同时 home 里有 alpha.1 启动的活实例。
  - §4.2 补一句：有 skill 目录、没有清单时，按「已安装、版本未知」处理。
- **性质**：推理；alpha.1 一侧的代码路径已确认。

#### R-S3　小问题

| # | 位置 | 问题与为什么 | 改法 | 性质 |
|---|---|---|---|---|
| 1 | §10.5 第 1413–1414 行 | decide 样例里，上游带 `retry-after: 20`，`fallback.delay_ms` 是 20000。按 D-15 与 retry.rs:14-32，第 3 次尝试失败之后是第 3 次重试，应当立即发出，fallback 应为 0。这个数字是第一版尊重 Retry-After 时留下的。golden 样例与 D-15 相悖，B4 的测试会照着它写错 | 改为 0；或者把 `attempt` 改为 6，fallback 保留 20000，并注明它来自退避，与 `retry-after` 无关 | 已确认 |
| 2 | §7.4 第 1025 行；§10.14 第 1643、1648 行 | 规则是「声明了 retry 的程序之后，只能是声明了 retry 的」。route（request、retry）后面接 convert 或 rewrite，会被 `config_conflict` 拒绝，因为表里这两个角色都不含 retry。可多协议渠道的路由，正需要在 route 选完渠道之后按渠道的协议转换；按渠道删字段的 rewrite 也一样 | §10.14 写明：convert、rewrite 排在 route 之后时也声明 retry，每次重试按新的路由重做；`init` 与 `check` 照此生成与校验 | 推理 |
| 3 | §10.2 第 1330 行；§10.10 第 1557 行 | 第 1330 行说「链上有程序要 digest 或 full，就先读完请求体」；第 1557 行只算 request 阶段的 full、digest，加上声明了 decide。只有 notify 要 digest 时（例如单独的统计程序），两处结论相反。其实 digest 可以边转发边算，用不着缓冲。客户端中途断开时摘要是什么，也没写 | 以第 1557 行为准，改第 1330 行；notify 的 digest 边转发边算，请求体不完整时为 null | 已确认 |
| 4 | §6.5 第 796 行；§11 第 1680 行 | 「单条记录不超过 64 KiB」已列入 A 级，超出时先截哪里却没写。粗估：每份 report 的 summary 最多 512 个字符、actions 最多 32 条，链上 3 个程序、23 次尝试，单是 `tries[].reports` 就能超过 64 KiB；`path`、`user_agent` 也没有上限 | 写明截断顺序，例如先从旧的尝试删 `tries[].reports`，再截 summary，再截 `path`、`user_agent`，并标出 `truncated` | 推理 |
| 5 | §6.5 第 793、827 行 | 被省略的尝试（计入 `tries_omitted` 的那些），它们的 `sent` 与 `response` 体留不留，没写。留着就会悄悄占预算，挤掉别的请求的体；拿 `body` 去取这些尝试时回什么，也没写 | 体随 `tries` 一起截断；取被省略的尝试，回 `not_found`，`reason` 为 `no_such_part` | 推理 |
| 6 | §6.3 第 744 行；§6.5 第 840–843 行 | `body` 的边界没写全：① `offset` 等于 `total_bytes`，轮询进行中的体时正是这种情形；② `limit` 为 0 或大于 1 MiB；③ 复制途中超出预算而变成 `not_retained` 时，正在读的一方下一块收到什么。第 843 行只写了 `evicted` | ① 回空块，`more:false`，`complete` 照实；② 回 `bad_request`；③ 同样回 `not_found`，带 `reason` 与 `cause` | 推理 |
| 7 | §10.11 第 1590、1600 行 | ① 在进链之前就被拒绝的请求（403、413、`client_body_aborted`）发不发 notify，没写。② 队列只按条数限制（1024），每条却能带最多 64 KiB 的 `state`，一个程序最多占 64 MiB，不在体的预算里。③「默认 1024 条」像是可以配置，实为 E 级常量 | ① 写明不发；② 队列另按字节设上限，或者 `state` 不进队列，发送时再取；③「默认」改为「上限」 | 推理 |
| 8 | §10.10 第 1546 行；§10.11 第 1594–1595 行 | forward 请求拿到「200 但内容是错误」时，记录与 notify 的 `outcome` 都是 `ok`。adaptive 靠完成通知判断成败（计划 173-174 行），对这类常见故障（记忆 2026-09-14）是盲的；界面也会把它显示为成功 | 在历史副本上被动地做同一套 `upstream_error_body` 判定，写进记录与 notify，不改动转发的字节；或者在协议文档里写明这个盲区 | 推理 |
| 9 | §7.5 第 1041–1054 行 | D-16 的余量：① format 改写 url（§10.4 的 `url`、route 换渠道）时，继承来的 key 随请求去了别的主机，D-16 只在继承的那一刻生效；② 密钥头按名字识别，名字少见的凭据头（例如 `x-access-code`）会被继承到所有目标；③ `defaults.proxy` 自带 userinfo、配置写了同一个代理却不带 userinfo 时，继不继承，没写；④「代理口令只跟默认 proxy」被实现为「指向同一个代理」，比指示宽，属于细化，§13.2 没标出来；⑤ base_url 末尾多一个 `/v1` 也算不同目标，key 会被扣下 | ①②⑤ 写进 §7.5 与协议文档，doctor 对 ② 给出提示；③ 写明不继承，并列进 `withheld`；④ 在 §13.2 标为细化，请用户过目 | 推理 |
| 10 | §7.5 第 1053–1054 行；§7.6 第 1065 行；§8.2 第 1129 行 | `defaults` 有 `api_key` 却没有 `base_url` 时报 error，这会让所有实例都起不来，包括根本不用默认值的配置。写入前的校验又会拒绝「先 set `defaults.api_key`、后 set `defaults.base_url`」这个顺序 | 文档与 skill 写明先设 `base_url`；或者允许一次 set 多个键；B3 加这个顺序的用例 | 推理 |
| 11 | §4.2 第 574–576 行 | 「保留现有字段」与「不含密钥」冲突：现有的 `pending_restores[].args` 是启动参数，可能含 `--api-key`（73b7863 的 state.rs:152-160） | 写明 W2 写出的 `pending_restores` 恒为空，只为旧版本的读者保留，W2 自己也不读 | 已确认 |
| 12 | §1.6 第 247–253 行 | ① 只有新建的 home 才设 0700 或显式 DACL。0.1.x、alpha.1 留下的 home 不会收紧，用编辑器写的 settings.json 会按 umask，或者继承盘符的 ACL。② 临时文件 `*.tmp-*` 若不在创建时就带上权限，rename 之前会有一个明文可被他人读取的窗口 | install 与第一次写配置时，收紧属于当前用户的既有 home；临时文件用最终权限创建：unix 用 `mode(0o600)` 加 create_new，Windows 在 CreateFile 时就给安全属性 | 推理 |
| 13 | §4.2 第 546–550 行；§5 第 676 行；§6.6 第 862 行 | 没有配置文件、只靠覆盖参数与 `defaults` 启动的实例，现行是允许的：文件不在就用默认配置（config.rs:812-831）。W2 的 spec 里 `config_path` 总有值，指向一个本来就不存在的文件，restore 会永远报 `config_missing`，D-8 对这类实例失效。现行的 restore 只在给了 `--config` 时才检查（restore.rs:24-38），所以这是回退 | 二选一：start 要求配置文件存在，没有就报 `target_not_found`；或者 spec 记 `config_path: null`，restore 不检查它 | 推理 |
| 14 | §5 第 665–666 行 | 条件「旧实例回 `unsupported_arg`」不会成立：W2 的实例从 v1 起就支持 `keep_restore`，W2 之前的实例则静默忽略未知参数（daemon.rs:1215），也不会回这个码。就算真的回了，op 被拒，实例也没退出，「在它退出后回写」无从谈起 | 按 §6.2 第 5 条先查 `features`：没有 `shutdown.keep_restore`，就发不带参数的 shutdown，等它退出后用同一份 spec 回写 | 推理 |
| 15 | §6.6 第 869–870 行；B0 第 1819 行；P4 第 1810 行 | ①「契约测试改为引用新位置」与 B0 验收的「force_contract 原样通过」矛盾，而 `tests/force_contract.rs` 的文件头写着「改这些测试就是破坏契约」。② P4 若把 home 的解析移出 settings.rs，测试第 94 行的 `use aproxy::settings::{HOME_ENV, …}` 也会断。③ CLAUDE.md 是 AGENTS.md 的独立副本（两份都受版本管理，都是 18879 字节），里面同样写着 `src/ledger.rs` | ① 写明只许改 `use` 路径，断言逐字不变，并同步改文件头那句；或者保留门面模块，从新位置再导出。② 同上。③ 改路径时 AGENTS.md 与 CLAUDE.md 一起改 | 已确认 |
| 16 | §10.9 第 1522–1528 行 | required 程序只在 request、retry 阶段快速失败，其余阶段与可选程序一样跳过。这与 D-12 的字面「required 的程序让请求快速失败」不同。理由成立：定稿的护栏要求 decide 出错一律回落，用户 09-23 也定过响应侧失败就透传。但这是对 D-12 的细化，稿子没有列出来 | 在 §13 标为对 D-12 的细化 | 已确认 |
| 17 | §8.2 第 1125 行；§9.5 第 1261 行 | 0.1.x 留下的 settings.json 是平铺字段、没有 schema，按 schema 1 读必然报错。普通 install 因此退出 3，每次 start 也都失败；出路只有逐个 unset 或强制安装。§4.3 第 7 条与迁移文档只讲了 run/ 里的残留 | 迁移文档写明；或者让 install 像 stop、status 一样，用内置默认运行并告警 | 推理 |
| 18 | 多处 | 杂项：① §3 第 469 行「任何非 2xx 都意味着它已经放弃了」，有了 forward 就不成立；② §12 第 1729–1730 行引的 TODO 第 44、247 行已失效，现在在 TODO.md:59、262，在 73b7863 也不是那两行；③ 稿首第 3–5 行只列了 D-12…D-15，D-16 其实已经并入；④ 第 1371 行的 report 文本写着 `using retry mode`、`mode retry`，它不是字段名，但可以顺手改；⑤ 计划 release-0.2.0.md 第 102、167 行仍写 `mode`，要由主代理同步；⑥ 已知密钥替换包括 settings `defaults` 里的密钥（§1.5 第 219 行），B8 的依赖却没列 B3 | ① 改为「带 `x-aproxy-error` 的非 2xx」；② 按条目名引用；③④⑤⑥ 逐项改 | 已确认 |

### 三、决策与指示的落实，以及章节之间是否一致

- **D-12**：
  - 落在 §0 第 40–41 行、§1.2 的表、§2.1 的 `format_unavailable`、§6.4 的 `formats[].breaker`、§7.4 的两个键、§10.9、§11 与 B10a。
  - 对 required 程序的细化见 R-S3-16。
- **D-13**：
  - 落在 §0 第 42–43 行、§1.2 的「保留量」、§6.3、§6.5 第 826–843 行、§7.1 的 `history.body_budget_mb`、§11 与 B8 的两条体用例。
  - 全稿剩下的两处「4 MiB」：一处在 B10d，是测试用的请求体大小；一处在 §15，是修订史。都不是单体上限。
  - 「保留开头 64 KiB 与结尾」只剩 decide 的管道（§10.5），与请求历史无关，§15 的 S3-30 写明了这一点。
- **D-14**：落在 §1.1、规则的未知键（989 行）、§7.5、§8.2、§9.3 的 `set` 与 B2、B3。
- **D-15**：
  - 落在 §0 第 39 行、§7.3 第 998–1002 行与 B4。
  - 唯一的矛盾是 decide 样例，见 R-S3-1。
- **D-16**：
  - 落在 §0 第 44–45 行、§1.5 第 195 行、§1.6 第 252 行、§7.1、§7.5 第 1041–1054 行、§8.1、§2.4 的 `default_credential_withheld`、§11、B3 与 §13.2。
  - 指示 3 的各项都在：规范化、代理口令、凭据没有目标就报错、`withheld`。
  - `withheld` 里的 URL 经 §1.5 打码（1047 行），启动时的 INFO 日志也是 §1.5 的出口之一（198 行）。没有发现 userinfo 外泄的路径。
  - 余量见 R-S3-9。
- **format 定稿**：按角色拆子命令在 §10.14；按请求选投递方式在 §10.10；判断与暂存都在 format 里，见 §10.10 第 1576–1578 行与 §10.14 的 adaptive。
- **命名**：
  - `delivery` 见 §1.3 第 154 行、§7.1、§10.4、§10.10；`behavior` 见 §6.4；`process` 见 §7.4 与 §10.2 的 hello。
  - `mode` 只出现在「现状」与自由文本里（R-S3-18 ④）。
- **章节之间的矛盾**：
  - 新发现的有：R-S1-1（§10.14、§7.2 对 §7.3）；R-S3-1（§10.5 对 §7.3）；R-S3-3（§10.2 对 §10.10）；R-S3-11（§4.2 内部）；R-S3-15（§6.6 对 B0）；R-S3-18 ①（§3 内部）。
  - 除此之外，§0、§2、§6、§7、§8、§9、§10、§11、§14 之间没有发现矛盾。

### 四、现状引用与 §12 的抽查

- **现状引用**：用 `git show 73b7863:<路径>` 核对了 45 处，全部属实，偏差最多一行。包括：
  - install/flow.rs:12-18（永久接口的声明在第 18 行）、958；
  - announce.rs:212-217、296-299：偏移与稿子一致；Windows 的 [4,8) 靠新建的节自动清零；
  - install/restart.rs:17-24；restore.rs:30-36；
  - watchdog.rs:339、954、968、1436、1464；
  - config.rs:586-592、611、854、941-947；
  - daemon.rs:27、107、156、211、226-237、279-289、598、1132、1215；
  - transform.rs:426、445；aproxy-format 的 translate.rs:7-9 与 main.rs:189-199；
  - proxy.rs:342、432、1198-1209、1633、3086、3207；
  - settings.rs:164、361-367；util.rs:39；commands/config.rs:14-22；
  - server.rs:92、111、423；start.rs:274；restart.rs:8-14；main.rs:27、183；cli.rs:51、179。
- **§12 抽查**：K3、K5、K8–K14、G1–G9、D2 与 D3、CP-19 的着落都与正文一致。
  - K5 原本建议加全局 `--home`，稿子没采纳；表里没注明，只在下方的「不在 0.2.0 做的」里列出。
  - 两个 TODO 行号已失效，见 R-S3-18 ②。
- **实现时要重核**：本轮开始时，HEAD 已比 73b7863 多出 14 个提交，其中有改动 main.rs 与 cli.rs 的桌面提交。实现时要按当时的 HEAD 重核行号。

### 五、与永久契约的冲突

- **C1–C8 本身**：没有发现改名、删除或改类型的地方。
  - §4.1 的布局保留了 `bin/`、`install/`（含 unix 的 announce）与 `ledger/`。
  - `install --force` 的语法不动（§9.1）。新加的 `--check`、`--json` 与 `--force` 冲突，按 force_contract 文件头「只追加」的规矩补样本（第 1184 行）。
  - 通用退出码只用 0–5（§2.4）。
- **冻结代码的位置**：C3 的读者迁进 aproxy-ipc，与 AGENTS.md 里「frozen in `src/ledger.rs`」那句、与契约测试的文件头都有冲突，见 R-S3-15。要定下来的只有一件事：契约测试里的 `use` 路径算不算契约。
- **`install --continue --handover-from`**：
  - 第二版撤回了改名、保持原样，与 flow.rs:18 的永久接口声明一致。
  - 稿子给的理由属实：alpha.1 的安装者就是以这个命令行交棒的（flow.rs:362-368）。
  - 现行的 `--force` 已与 `continue_` 冲突（cli.rs:227），不受影响。
  - 这个入口接手之后还该做的检查，见 R-S2-4。

---

## 第三轮核对（2026-10-12）

**对象**：设计稿在 34cf61d 的版本（2188 行），即按第二轮复审改过的那一版。改动是 `git diff ff9b0ce 34cf61d -- .agents/plan/w2-contracts.md`，增 250 行、删 78 行。本节的「第 N 行」都指 34cf61d。

**依据**：
- 本文件第二轮复审的 23 条（第 665 行起）；
- 设计稿 §13.3（主会话的决定）与 §15「第二轮复审」一节（w2-design 的逐条说明）；
- AGENTS.md 的 C1–C8 段，计划 release-0.2.0.md:242-249 的 C7；
- 73b7863 上 alpha.1 的安装代码与 decode.rs。

**方法**：
- 逐条对照 diff 与改后的正文，改动过的各节通读；
- §6.5 的样例与 §10 的各个样例互相对照；
- alpha.1 的续作逻辑用 `git show 73b7863:` 核对；
- 只读：没有跑 cargo，没有起进程，没有改设计稿。

**编号**：本节的新问题记作 T-S2-n、T-S3-n。

### 结论

- 23 条：**已解决 18，换了做法且理由成立 5，未解决 0，退化 0**。§15 的逐条说明与正文对得上。
- 偏离建议的五处，理由都成立。其中两处有副作用要补：
  - R-S2-1 的「半开只用数据通路」与「notify 不计熔断」，见 T-S2-1；
  - R-S2-3 的「只在端口 0 时钉」，见 T-S3-6。
- 新问题 11 条：S1 0 条，S2 3 条，S3 8 条。三条 S2 都是这一轮的改法带出来的：
  - **T-S2-1**：notify 路径失去了一切保护。
    - 只声明 notify 的程序一旦熔断就永不恢复；
    - notify 处理卡住时，每条通知都会换掉 worker，而熔断永远不开；
    - notify 的时限借用 `decide_timeout_secs`，它取 0 表示不限，这时一条卡住的 notify 会让请求永久挂起。
  - **T-S2-2**：install 在 settings.json 无效时也照常运行（R-S3-17），而滚动重启没写明要预检。健康的实例会被停掉，新旧二进制又都因 settings 无效起不来，回退也拉不回。
  - **T-S2-3**：B0 的验收要求「AGENTS.md 与 CLAUDE.md 里不再出现 `src/ledger.rs`」。台账的写者若仍留在 `src/ledger.rs`，AGENTS.md 就不再指向这部分冻结代码；新文件若叫 `aproxy-ipc/src/ledger.rs`，这条按字面又永远过不了。
- **永久契约**：§9.5 去掉 install 的退出码 3，与 C7、C8 不冲突。B0 改 AGENTS.md 的写法，见 T-S2-3。
- **§6.5 的样例**：与 §10.4、§10.7、§10.11 讲的是同一个请求，时间线对得上；不一致的四处见 T-S3-5。

### 一、23 条的状态

| 编号 | 状态 | 证据位置（34cf61d 行号） |
|---|---|---|
| R-S1-1 | 已解决：照推荐默认 retry，改正「静态兜底」的说法，另加 doctor 告警 | §0:54-55；§7.2:988、1003-1005；§10.9:1615-1617；§10.10:1639、1678；§10.14:1751、1756-1759；§2.4:419；§6.4:785；§10.4:1444-1459；B11:1963（告警的条件见 T-S3-1） |
| R-S2-1 | 换了做法且理由成立：同一 worker 上数据通路优先，另加三条规则 | §10.1:1361-1366；§10.3:1429-1432；§10.9:1598-1599、1618-1619；§10.11:1695-1703；B10d:1962（副作用见 T-S2-1） |
| R-S2-2 | 已解决 | §0:44；§1.5:218、228-229；§2.3:347；§6.5:857-860、871-878、885-890；B8:1976-1977 |
| R-S2-3 | 换了做法且理由成立：只在配置端口为 0 时钉 | §4.2:566-567；§5:677-679；§11:1785；B5:1954（余量见 T-S3-6） |
| R-S2-4 | 已解决 | §0:65；§4.2:599；§4.3:617-623；§5:702；B9:1958（余量见 T-S3-2、T-S3-3） |
| R-S3-1 | 换了做法且理由成立：改为第 6 次尝试、fallback 20000，上游的 retry-after 改为 60 | §10.5:1488-1497。已按 retry.rs:14-32 核对：第 6 次重试是 5×2² = 20 秒 |
| R-S3-2 | 已解决 | §7.4:1075；§10.14:1747、1752、1757；B11:1963 |
| R-S3-3 | 已解决 | §10.2:1403-1404；§10.11:1694 |
| R-S3-4 | 已解决 | §6.5:827-830；B8:1978（余量见 T-S3-8 ①） |
| R-S3-5 | 已解决 | §6.5:831-832 |
| R-S3-6 | 已解决 | §6.3:774；§6.5:885-890；B8:1978 |
| R-S3-7 | ①③ 已解决；② 换了做法且理由成立：按字节设上限（16 MiB，`state` 计入） | §10.11:1688、1700；§11:1793；B10d:1962 |
| R-S3-8 | 已解决 | §10.10:1662-1667；§6.5:836；B10d:1962（旁路副本的大小见 T-S3-4） |
| R-S3-9 | ①②④⑤ 已解决；③ 换了做法且理由成立：不列进 `withheld` | §7.5:1097-1105；§2.4:418；§13.2:1900-1901；B3:1952 |
| R-S3-10 | 已解决 | §7.5:1115-1116；B3:1952 |
| R-S3-11 | 已解决 | §4.2:594-595；B9:1958 |
| R-S3-12 | 已解决 | §1.6:258-261；B7:1956 |
| R-S3-13 | 已解决 | §4.2:563-564、580；§5:705-706；§11:1785；B5:1954（实例记录这一侧见 T-S3-7） |
| R-S3-14 | 已解决 | §5:692-695；B5:1954 |
| R-S3-15 | 已解决：`use` 路径不算契约 | §6.6:916-918；P4:1940；B0:1949（B0 的新验收见 T-S2-3） |
| R-S3-16 | 已解决 | §10.9:1613-1614；§13.3:1919 |
| R-S3-17 | 已解决 | §8.2:1190-1194；§9.5:1327；B9:1958（带出的新问题见 T-S2-2） |
| R-S3-18 | 已解决（①–⑥） | §3:481-482；§12:1836-1837；稿首第 3–6 行；§10.4:1447-1448；计划 release-0.2.0.md:102、167（ff9b0ce 已改）；B8:1957 |

**偏离建议的五处**：
- **R-S2-1**：
  - 选「同一 worker 上数据通路优先」，不单开 worker：理由成立。adaptive 在 request 阶段查的、在 notify 时记的是同一张表，拆进两个进程，判断就看不到结果。
  - notify 失败时换掉 worker、但不计入熔断：理由成立。观察路径出问题，不该把判断路径一起关掉。可这一来，notify 路径自身就没有任何保护了，见 T-S2-1。
  - notify 的合法回信不清零计数：理由成立。否则一个数据通路坏了、却还能应答通知的程序永远熔断不了。
  - 半开试探只用数据通路的消息：对同时有数据通路阶段的程序，理由成立；对只声明 notify 的程序，它们永远等不到试探，见 T-S2-1。
- **R-S2-3**：只在端口为 0 时钉，理由成立：要是一律钉，用户以后改了配置里的端口，restart 也会被旧端口盖掉。但同样的问题在端口 0 的配置被钉过之后仍会出现，见 T-S3-6。
- **R-S3-1**：改用第 6 次尝试与 20000，并把 retry-after 改为 60，理由成立：数字与退避一致，也不再与 Retry-After 碰巧相同。
- **R-S3-7 ②**：按 16 MiB 字节设上限，理由成立：等待发送的通知总得留着 `state`，只有字节上限才真正约束内存。
- **R-S3-9 ③**：不列进 `withheld`，理由成立。这是普通的上层覆盖，`sources` 已经显示 `proxy` 来自配置。可选的补充：doctor 给一条 info，提示「配置的代理与默认代理是同一个，但没有带默认值里的 userinfo」。

### 二、新问题

#### T-S2-1　notify 路径失去了一切保护

- **位置**：§10.9 第 1598–1599、1618–1619 行；§10.11 第 1697–1698 行；§10.1 第 1363–1366 行；§10.2 第 1413 行；§1.2 第 133 行。
- **问题**：
  - **只声明 notify 的程序熔断后永不恢复**（例如只做用量统计的程序）：
    - 半开时只放行数据通路的消息（1618 行），notify 又不作试探（1619 行），这类程序永远等不到试探。
    - 启动握手失败的可选程序以 open 起步（1413 行），这类程序从此不再被试探，直到实例重启。
    - 在那之前，通知全部丢弃，只有 `notify_dropped_total` 在涨。
    - notify 的计数只认「写不进 stdin 与进程退出」（1598 行），「起不来」算不算，没写。
  - **notify 处理卡住时没有任何保护**：
    - 每条 notify 超时（时限默认 2 秒）都要换一次 worker，换的时候要重新握手；熔断永远不开。
    - `pool_max = 1` 时，每次都让数据通路多等这 2 秒再加上重启的时间，进程里的判断状态也随之清空。
    - 这种情形没有计数，外面看不见；B10d 只验了「回信超时不改变熔断计数」。
  - **`decide_timeout_secs = 0` 时请求会永久挂起**：notify 的时限借用 `decide_timeout_secs`（1698 行），而它取 0 表示不限（§1.2 第 133 行）。
    - 用户为了让 decide 慢慢判断而设 0 时，一条卡住的 notify 就让这个 worker 永远处于忙碌状态。
    - 数据通路的消息排队时不计时（1363 行），请求就此永久挂起。
    - 熔断也永远不会打开。
- **为什么要紧**：R-S2-1 的改法把 notify 从数据通路的熔断里解耦出来，这是对的；可 notify 路径自己从此毫无保护。第三种情形会让请求永久挂起，这正是 aProxy 要防的。
- **改法**：
  - notify 单独计数：连续的超时、不合法回信与起不来都计入。达到 `breaker_failures` 时，暂停派发 notify，丢弃并计数；冷却之后放一条 notify 试探。这个计数不影响数据通路的熔断器。
  - 没有数据通路阶段的程序，半开时用 notify（或新 worker 的 hello）试探。
  - notify 的时限定为 E 级常量（例如 2 秒），不再借用 `decide_timeout_secs`，也就不会变成不限。
  - 快照加一个 notify 失败计数，例如 `formats[].notify_failed_total`；日志限频告警。
  - B10d 加两个用例：只声明 notify 的程序熔断之后能恢复；一个 notify 总是超时的程序不会让数据通路每次都等。
- **性质**：推理。

#### T-S2-2　settings.json 无效时 install 照常运行，而滚动重启没写明预检

- **位置**：§8.2 第 1190–1194、1195 行；§9.5 第 1327 行；§5 第 700 行（「滚动重启用同一套原语」）与第 691 行（restart 的预检）；B9 第 1958 行。
- **问题**：
  - R-S3-17 之后，install 在 settings.json 无效时用内置默认继续走。
  - 可实例「只在启动时读 settings，读不了就启动失败」（1195 行），新版本的 `__serve` 如此，旧版本的也如此。
  - 于是一种常见情形是：用户手改 settings.json 时打错一处，正在跑的实例不受影响，因为它们启动时就读过了。之后执行 install，或在桌面界面里点更新：
    - 滚动重启停掉一个健康的实例；
    - 新二进制起不来；
    - 回退到旧二进制（§5 第 651 行「新版本起不来时，由新版本拉起旧版本」）也起不来；
    - 实例就此下线。看护者在换二进制之前已经被结束，也不会把它拉回来。
  - 改之前，install 在入口就以退出码 3 停下，一个实例都不碰。
  - 第 700 行只说「滚动重启用同一套原语」，没写明这套原语包括 restart 的预检（第 691 行）。
- **为什么要紧**：用户为修问题而做的升级，反倒造成停机。这是「不中断运行」最脆弱的一环（install/restart.rs:16-19 的模块文档）。
- **改法**：
  - §5 写明：滚动重启在停掉每个实例之前，都用目标版本的 §7.5 管线预检，settings 也包括在内。预检不过就不停这个实例；install 以 halted 结束，说明是哪个实例、哪个文件。
  - 这条也覆盖「新版本不接受现有配置」的老情形：以前要先中断一下再回退，现在不中断。
  - B9 加一个用例：settings.json 无效、有活实例时执行 install，实例不被停掉。
- **性质**：推理。

#### T-S2-3　B0 的验收「AGENTS.md 与 CLAUDE.md 里不再出现 `src/ledger.rs`」

- **位置**：B0 第 1949 行；§6.6 第 916–918 行；AGENTS.md:80（CLAUDE.md 同）；src/ledger.rs:1-30。
- **问题**：
  - §6.6 与 B0 只把 C3 的「读者与启动标记推导」迁进 aproxy-ipc。台账的写者留在哪里，没有写：每个长期运行的进程都要登记、退出时删除（src/ledger.rs:16-19）。
    - src/ledger.rs 的文件头写明：目录名、文件名、条目的 JSON 字段、启动标记的推导与 role 词表，全部冻结。
    - 写者若仍留在主 crate 的 `src/ledger.rs`，这条验收就等于让 AGENTS.md 不再指向一部分冻结代码，正削弱了它对永久契约的表述。
  - 新文件若按惯例叫 `aproxy-ipc/src/ledger.rs`，它本身就含子串 `src/ledger.rs`，按字面检查永远过不了。
  - AGENTS.md 的末句「a contract test that blocks a change means the change is wrong」若不跟着改，就与测试文件头新加的「`use` 路径不算契约」各说各的：编译失败算不算「挡住」，读 AGENTS.md 的 agent 无从判断。
- **结论**：「`use` 路径不算契约」本身与 C1–C8 一致。C1–C8 冻结的是布局、名字、格式与命令行，不是 Rust 的模块路径。要改的只是这条验收的写法。
- **改法**：
  - B0 写明 C3 的哪些部分迁走：条目结构、读者与它的义务（`terminate`、`remove_if_unchanged`）、启动标记推导、写者。
  - 验收改为：AGENTS.md 与 CLAUDE.md 逐字相同，列出迁移后存放 C3 冻结代码的全部文件，列出的文件都存在。
  - 在同一句里补半句：契约测试冻结的是断言，`use` 路径可以随代码移动而改。这样不但没有削弱 AGENTS.md，反而把边界写清了。
- **性质**：推理。src/ledger.rs 里读者、写者的分工已确认。

#### T-S3　小问题

| # | 位置 | 问题与为什么 | 改法 | 性质 |
|---|---|---|---|---|
| 1 | §2.4 第 419 行；§10.9 第 1617 行 | `forward_default_unguarded` 的条件是「能让请求转入 retry 的程序（声明了 request 或 decide 的）都是可选的」。① 第二版 §7.2 那条链（默认 forward，adaptive 可选，convert、keys 是 required）正是 R-S1-1 说的配置：convert、keys 声明了 request 又是 required，条件不成立，不告警。② required 也护不住重试：熔断时，required 程序在 request 阶段让请求以 503 快速失败，在 decide 阶段与可选程序一样视为不表态（1607–1608 行），都不会把请求转入 retry。③ 一个 format 都没有、只写了 `delivery = "forward"`（0.1.x forward_only 的用法）时，「都是可选的」空泛地成立，按字面也会告警 | 条件改为：默认 forward，且链上有声明 decide 的程序，或者有声明 request 的可选程序。重试靠的正是这些程序，而它们会被跳过。没有任何 format 时不告警 | 推理 |
| 2 | §4.3 第 617–621 行；B9 第 1958 行 | R-S2-4 的失败现场只置了 `phase = failed`，没置 `halted`。alpha.1 的每个非 install 命令，只要看见 install/state.json，就会静默拉起 `install --continue`（73b7863 的 main.rs:128-146）。它的续作只拒绝「failed 且 halted」的现场（flow.rs:718-731），其余的 failed 现场照常接管：`--from` 来源的安装会重新备料、再交棒、再失败；网络来源的续作会写下自己的 pid、短暂发出宣告，然后报「续作缺少 --from 源记录」退出（flow.rs:747-749、773-781）。用户照提示执行 `aproxy stop all` 时，这条命令本身也会拉起一个续作 | 同时置 `halted = true`。alpha.1 对它的处理正好是「原因不会自己消失，不自动重试，排除原因后重新执行 install」（flow.rs:713-731）。W2 自己的自动续作（§9.2 第 6 步）也按这个字段不再重试 | 已确认 |
| 3 | §4.3 第 617 行；§5 第 702 行；B9 第 1958 行 | 第二版写的是「普通 install…在改动任何东西之前失败」，这一版改成「普通 install 与续作都在广播之前检查」，B9 的验收却仍是「在改动任何东西之前失败」。对普通 install 来说，「广播之前」允许它先加锁、下载、校验，然后才失败，两处说法对不上 | 普通 install 在入口（加锁与下载之前）先查一次；所有路径在广播之前再查一次，覆盖续作，也覆盖这段时间里新起的实例 | 已确认 |
| 4 | §10.10 第 1662–1667 行 | 「体没有入库时，在一份旁路副本上做」：沿用的 8 MiB 只限制解码后的输出，没有限制副本本身。现行的检查路径只在内存里处理不超过 1 MiB 的响应，更大的就不解码、不判定（decode.rs:35-38）。照字面，一个大的 forward 响应（例如几百 MB 的文件下载）会在预算之外被完整复制一份 | 旁路副本只留原始字节的前 1 MiB，与现行一致，超出就不判定；SSE 用增量扫描，不留副本 | 已确认 |
| 5 | §6.5 第 811–820 行；§10.5 第 1487–1493 行 | 见下文「三」：① `client_status` 为 null，可保活已经提交；② 顶层 `reports` 缺了 adaptive 的那份；③ 顶层 `response_bytes` 的口径没写；④ decide 样例沿用同一个 request_id，讲的却是另一条时间线 | 见下文「三」 | 已确认 |
| 6 | §4.2 第 566–567 行；§5 第 677–679、691 行 | 只在端口 0 时钉，是为了不让旧端口盖掉用户后来改的配置。可端口 0 的配置被钉过之后，同样的事还是会发生：用户把配置改成固定端口再 restart，预检用的是记录里的 spec，而钉进去的 `overrides.listen_addr` 优先于配置（§7.5 的层级）。新端口不会生效，也不会被察觉：第 691 行的「listen_addr 变了」比的是 spec 解析出的地址 | 钉上去的值只在配置解析出的端口仍是 0 时才用。为此要单独记一个字段，或者在 overrides 里标明它是钉上去的、不是用户给的覆盖 | 推理 |
| 7 | §4.2 第 554、563 行；§6.4 第 783 行；§1.2 第 124 行 | spec 的 `config_path` 可以是 null，这写明了；实例记录与快照里的 `instance.config_path` 却没写。样例里它都是字符串，读者按 §1.2 会以为它总有值。没有配置文件的实例，这一项写什么（null，还是那个不存在的默认路径），status、find 与界面又怎么显示，都还没定 | 实例记录与快照的 `config_path` 同样可以是 null，§4.2 写明；B6 的 golden 加一例 | 推理 |
| 8 | 多处 | 杂项：① 64 KiB 的截短顺序没有覆盖 report 的 `actions`（§10.8 第 1580 行：各字段的字符串没有长度上限）与顶层的 `model`，三步做完仍可能超限。② 加了 `info` 级别之后，doctor `--json` 的 `summary` 只有 `errors` 与 `warnings`（第 1297 行）。③ P4 与 B0 都要改 force_contract：先落地的那一块，若不同时改文件头那句，改 `use` 行时文件头还写着「改这些测试就是破坏契约」。④ 熔断打开时，已在排队、还没写出的数据通路消息怎么处理，没写明。⑤ 持续高负载下，数据通路总有消息在排队，notify 可能一直派发不出去，adaptive 就学不到流中途断开这类失败 | ① 再超出就截 actions 的字符串字段、删掉整份 reports，`model` 与 `path` 一样截短；② 加 `infos`，或写明 info 不计数；③ 写明谁先落地，谁就同时改文件头；④ 写明按熔断表处理（跳过，或回 503）；⑤ 协议文档写明：程序可以把「同一个摘要再次出现」当作补充信号 | 已确认 |

### 三、§6.5 请求记录样例的核对

- **对得上的部分**：样例（第 811–820 行）与 §10.4、§10.7、§10.11 的样例讲的是同一个请求，时间线如下：
  1. adaptive 回 forward（§10.4）。
  2. 第 1 次 forward 尝试拿到 529，用时 812 毫秒。adaptive 在 decide 里让请求升级：`upgraded` 为 true，`decision.by` 为 `format:adaptive`，`delay_ms` 为 0。
  3. 按现行退避，第 1 次重试立即发出，所以第 2 次尝试在第 1 次结束之后 1 毫秒开始。
  4. 升级之后开始保活：心跳样例的第 3 拍在 45 秒、第 2 次尝试、上次 529（§10.7）。
  5. 第 2 次尝试成功：通知里 `attempts` 为 2、用时 48 秒（§10.11）。

  其余各项也都自洽：
  - `phase` 为 `attempting`，`next_attempt_at` 为 null，`attempts` 为 2；
  - 进行中的那次尝试 `decision` 为 null，与第 841 行的规定一致。
- **不一致的四处**：
  1. `keepalive:true`、`heartbeats:3` 说明 200 与 SSE 的响应头已经写给了客户端，`client_status` 却是 null。§1.3 只说它是「回给客户端的」状态码，没说提交之前还是之后。
     建议定义为「已经写给客户端的状态码，提交之前为 null」，样例改为 200。
  2. 顶层 `reports` 只有 convert 与 keys，少了 §10.4 里 adaptive 在 request 阶段给的那份（`delivering forward`）。顶层本来就放 request 阶段的报告（第 826 行）。
  3. 顶层 `response_bytes` 是 0，可三拍心跳已经写给了客户端。第 878 行的「线上的字节数」没说算不算心跳，也没说指上游这一侧还是客户端那一侧。
     建议写明：它是交给客户端的上游响应字节，不含心跳。
  4. §10.5 的 decide 样例沿用了同一个 request_id，讲的却是另一条时间线：42 秒时已经是第 6 次尝试，而且是 429。这与上面那条时间线冲突。
     建议换一个 request_id，例如 `-43`。

### 四、与永久契约

- **§9.5 去掉 install 的退出码 3**：没有冲突。
  - C7 冻结的是 `install --force` 的退出码：0、10、11、12，以及用法错误的 2（计划 release-0.2.0.md:242-249）；§2.4 也写明「强制安装只承诺 C7 的退出码」。
  - §9.5 的 install 一行指普通安装，是 W2 自己定的 A 级契约，还没有发布，去掉一个码不算破坏。
  - 强制安装从不读 settings（C8），所以不受影响。
  - 这个改动带来的风险在运行层面，见 T-S2-2。
- **B0 与 AGENTS.md**：「`use` 路径不算契约」与 C1–C8 一致，问题出在验收的写法上，见 T-S2-3。
- **其余**：
  - `install --continue --handover-from`、布局、台账、锁与宣告都没动。
  - R-S2-3 钉端口不影响台账，台账在 bind 之后已用实际端口重写。
  - R-S2-4 的失败现场要照顾 alpha.1 已经发出去的续作逻辑，见 T-S3-2。
