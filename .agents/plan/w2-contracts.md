# W2 契约定稿（0.2.0）

> 状态：**定稿（2026-10-12）**。四轮对抗式审查依次发现 57、23、11 条问题，第四轮收口核对没有 S1、S2，主会话按用户授权
> 定稿。剩下 6 条 S3 标为「实现时处理」，见审查文件「第四轮收口核对」一节。实现排在 alpha.1 之后，从 §14.1 的前置重构开始；
> 实现时按当时的 HEAD 重核现状行号。它在第一版（d43ddc9）之上依据：
> - 对抗式审查 `.agents/review/2026-10-12-w2-contracts-review.md`：第二轮复审从第 665 行起，第三轮核对从第 968 行起，第四轮收口核对从第 1154 行起；
> - 用户随后的决策 D-12…D-16（计划 `release-0.2.0.md`「W2 设计稿审查后的用户决策」），以及 W2c 的用户追加（9e39930）；
> - 主会话按用户授权对复审与核对各条的决定（§13.3）。
>
> 写法：只读代码写成，没有改代码；现状行号对应 `73b7863`。
>
> 依据：
> - 计划的 W2、W2b、W2c、W3、W4、W5 节；
> - 第一轮审查综合稿与四份报告（`.agents/review/2026-10-11-0.2.0-audit/`）；
> - 记忆 0.2.0-direction、transform-format、bounded-retry-paths、random-log-names、ipc-v1-upgrade-contract。
>
> 怎么读：
> - §0 是摘要；§1 是对所有契约生效的总则；
> - §2–§10 每节都写「现状 / 设计 / 为什么」；
> - §11 冻结清单；§12 第一轮审查的对账；§13 用户决策记录；§14 实现切分；
> - §15 修订记录，按审查与复审的编号逐条交代。
>
> 编号约定：
> - C1–C8 指 W2b 的冻结项；
> - 「综C1」…「综C13」指综合稿 §1.4 的正确性条目；
> - S1-n、S2-n、S3-n 指第二轮审查的条目；R-S1-n、R-S2-n、R-S3-n 指它的复审；T-S2-n、T-S3-n 指第三轮核对。

---

## 0 一页摘要

最重要的决定：
1. **冻结分四级**。C1–C8 不动；本稿的契约只增不改。
   - 配置文件是严格的读者：任何不认识的键都报错（D-14）。
   - IPC 应答、持久记录、format 回信是宽容的读者（§1.1）。
2. **错误码单源**，放在新 crate aproxy-ipc。aproxy 自产的 HTTP 错误一律是 JSON：`error.type = "aproxy_<code>"`，并带
   `x-aproxy-error`、`x-aproxy-request-id` 两个头；SSE 终态事件用同一个 JSON（§2、§3）。
3. **home 是唯一的根**，删掉 APROXY_RUN_DIR。期望状态记录改为结构化的 spec，只有 stop 能删它（被新端口的记录接替时由拉起方删）。
   restart 先预检，停旧实例时保留记录（§4、§5）。
4. **IPC v1**：未知参数报错；只读 op 有 requests、request、body、subscribe；请求记录有界，事件限频；实例启动期间以 `starting` 应答（§6）。
5. **config.toml**：新增 `[timeouts]`、`[keepalive]`、`[retry]` 与 `[[retry.rules]]`、`[history]`、`[[format]]`。
   重试时序沿用现行：前 3 次立即，之后从 5 秒翻倍；不读 Retry-After，不加抖动。等多久只听 format 的（D-15，§7）。
6. **format 熔断**（D-12）：同一实例里，同一个程序连续 5 次进程级失败就熔断 30 秒，之后放一条消息试探。
   required 程序熔断期间，请求立即以 503 `format_unavailable` 结束；上游连不上仍然无限重试（§10.9）。
7. **请求历史**（D-13）：全部请求的体整份保留在内存里，总量默认 64 MiB、可配置，超出时从旧到新淘汰；单个体比整个预算还大才不保留。
   存的是解码后的字节，入库时按流替换已知密钥；经 IPC 分块读取（§6.5）。
8. **settings.json schema 1** 严格解析，`ui` 段是桌面私有的不透明对象。`defaults` 可以设上游、key、代理与两张头表，不收 `listen_addr`；
   凭据只发给它所属的目标。settings.json 因此含密钥，按 config.toml 同一套规则保护（D-16，§8）。
9. **CLI**：通用退出码 0–5，10 起归强制安装；查询与操作命令带 `--json`；`install --force` 拒绝与 `--check`、`--json` 同用（§9）。
10. **format 协议 1**（§10）：
    - spawn 进程也握手；程序在 hello 里按阶段声明要多少请求体：`none`、`digest`（原始请求体的 SHA-256）或 `full`；
    - 声明了程序不支持的阶段，一律在启动时报错；
    - decide 用 `reroute` 决定这次重试是否重跑重试链；
    - 声明 `retry` 的程序必须排在没声明的程序之后，所以没声明 `retry` 的程序不会因为重试而再收到消息。
11. **按请求选投递方式**（用户追加 2026-10-12，§10.10、§10.11）：
    - 投递方式 `delivery` 取 `forward`（仅转发）或 `retry`（无限重试）；配置里的是默认值，默认 `retry`；request 阶段的回信可以按请求改。
      推荐配方仍默认 retry，由 adaptive 对没失败过的请求回 forward，它不可用时一切回到 retry；
    - forward 请求在向客户端写出任何字节之前失败，decide 可以让它当场转入 retry，此后与一开始就用 retry 一样；
    - 请求结束时向声明了 notify 的程序发完成通知，只发不等，处理不过来就丢弃并计数；notify 有自己的熔断器与固定时限，不拖累数据通路；
    - forward 与 format 可以共存；官方 aproxy-format 按角色拆成子命令（§10.14）。

与 W2b 及 alpha.1 的接缝：
- unix 的安装宣告在 `<home>/install/announce`，C4 没有例外（alpha.1 已落地，§4.1）。
- 台账（C3）与实例记录分开（G3）。C3 的读者与启动标记推导迁进 aproxy-ipc，算法不变（§6.6、§14 B0）。
- `--version` 先于语言判定短路（G2，§9.2）；通用退出码只用 0–5（G5）。
- 安装的续作与早交接入口 `install --continue --handover-from` 保持原样，它已是永久接口（§5）。
  W2 以续作接手时，在广播之前检查管不了的实例，查到就以失败结束，并置 `halted`，免得 alpha.1 的续作反复接管（§4.3 第 7 条，R-S2-4、T-S3-2）。
  W2 之前的版本启动的实例，经强制安装迁移（§4.3）。

---

## 1 总则

### 1.1 冻结层级与「只增」机制

**现状**：
- 计划「目标」一节要求 IPC、布局、配置、CLI、错误体、信封都长期稳定，W2b 却把它们列为可以随意演进（G4）。
- 第一版把「读者忽略未知键」也用在配置上。S1-1 指出：条件类配置（规则的匹配器、`[[format]]`）会因此被旧版本读宽。

**设计**：

| 级 | 含义 | 范围 |
|---|---|---|
| F 绝对冻结 | 永不改动；JSON 只许加可选字段 | 只有 C1–C8，本稿不新增 |
| A 只增不改 | 可以加字段、键、op、码、事件、取值；不删、不改名、不改类型或语义 | 本稿的公开契约（§11） |
| E 可演进 | 随时可改 | 给人看的文字与各处的 `message`；日志行里除 `code=`、`request_id=` 之外的部分；实现与调参常量 |
| R 预留 | 名字与形状已定，以后实现，实现时按这里的形状 | 规则的 `body` 匹配器；format 的 admit 阶段；notify 的 attempt 事件 |

「只增」靠下面六条成立：
1. **读者分两类**。
   - 配置文件（config.toml，以及 settings.json 里 `ui` 段以外的部分）是严格的读者：不认识的键一律报错，指出文件与键路径（D-14）。
     新版本加的键要求新版本才能读；旧版本遇到它就拒绝启动，绝不当作没写。条件因此不会被旧版本读宽。
   - IPC 应答与事件、持久记录、format 回信是宽容的读者：忽略不认识的字段与事件。
   - IPC 请求的参数是例外：服务端对不认识的参数报错（§6.2），理由与配置相同。
2. **开放词表**。枚举一律用字符串取值，每个词表都写明读者的兜底：读作 `unknown`、按「不在正常服务」处理，或显示 `label`。
3. **能力先声明后使用**。
   - IPC 快照有 `ops` 与 `features`；
   - format 的 hello 里，双方各自列出 `stages` 与 `features`，aproxy 还列出它接受的覆盖键。
   - 调用方先查，查不到就不用。
4. **版本号只在不兼容时递增**：IPC 的 `v`、format 协议的 `v`、记录与 `--json` 文档的 `schema`。
   - 读者遇到更高的版本时跳过并报告，绝不删除。
   - settings.json 例外：它是配置文件，遇到更高的 schema 时报错（§8.2）。
5. **码只增**。错误码、动作类型、相位词表都只增；`message` 不是契约。
6. **用契约测试钉住样本**。每个形状都有 golden JSON，放在 aproxy-ipc 与 `tests/contract_v0_2.rs`。改样本就是破坏契约，测试注释写明这一点。

确实需要破坏时，另立并行的新版本（`v:2`、`schema:2`、协议 2），旧版本继续支持至少一个大版本。最后的兜底是强制安装：
C1–C8 保证任何版本都能装上任何版本；它也是 settings.json 被新版本写过之后，用户降级的出路（§8.2）。

**为什么**：
- 这是 G4 的统一表述。
- 配置严格，是用户的原话：「直接报错表示参数错误，不做任何兼容检测」。它同时让 S1-1 不再成立。
- 否决「全部绝对冻结」：成本太高。否决「C1–C8 以外随意演进」：界面、agent、跨版本的看护者与安装者都依赖这些形状。

### 1.2 共同约定

- **命名**：字段、配置键、码、枚举值一律用 snake_case 英文。
- **时间与数量**（机器可读的 JSON）：
  - 时刻用 Unix 毫秒整数，字段名以 `_at` 结尾；
  - 时长以 `_ms` 结尾，字节数以 `_bytes` 结尾，单调计数以 `_total` 结尾。
- **人写配置的单位**：`_secs`、`_mb`（实为 MiB）。文档如实写 MiB，不为此改名（DP-14）。
- **版本字段**：
  - 行协议（IPC、format）用 `v`，文档与记录用 `schema`；
  - C3 的 `ledger`、C6 的 `manifest` 是 F 级的既成名字，不动；
  - `version` 字段是不带 `v` 的 semver。
- **可选性**：
  - 样例里出现的字段，0.2.0 的写者都必须写出；可以为空的写成 null；形状描述里带 `?` 的字段可以省略。
  - 以后新增的字段，对宽容的读者一律是可选的。
  - 类型以样例为准，不用字符串表示数字。
- **进程身份**：`pid` 加 `start`。`start` 是 C3 推导的启动标记字符串，取代 `process_start: u64`（daemon.rs:156）与
  `created_at_process`（watchdog.rs:35）。
- **「0」的含义按键的种类固定**。现状里 `spool_limit_mb = 0` 被当作 1 MiB（proxy.rs:3086），与其余字段相反（K14）。

| 种类 | 0 表示 | 键 |
|---|---|---|
| 超时、等待上限 | 不限 | `timeouts.*`；format 的 `timeout_secs`、`decide_timeout_secs`、`worker_idle_secs`；`watchdog.idle_exit_secs` |
| 大小、次数上限 | 不限 | `max_request_mb`、`max_response_mb`、`logs.rotate_mb`、`watchdog.max_restarts`；format 的 `pool_max`、`breaker_failures`；规则的 `max_attempts` |
| 间隔 | 关闭 | `keepalive.interval_secs` |
| 保留量 | 不保留 | `history.body_budget_mb` |
| 阈值 | 字面的 0 秒 | `instances.idle_threshold_secs` |
| 不许为 0 | 报 `config_invalid_value` | format 的 `breaker_cooldown_secs`；`watchdog.heartbeat_secs`、`watchdog.stale_after_cycles` |
| 例外 | 不等待（立即重试） | `retry.max_backoff_secs`，沿用现行语义 |

### 1.3 同一概念同一名字

| 概念 | 名字 | 说明 |
|---|---|---|
| 请求标识 | `request_id`，形如 `19ac3f2e8b5d1a2b-42` | 用在 HTTP 头、错误体、IPC、format 消息、日志 `request_id=`；对外是不透明串 |
| 请求序号 | `request_seq` | 请求记录里的整数，供界面显示短序号 |
| 实例标识 | `instance_id`，16 位十六进制 | 由启动时刻毫秒的低 48 位加 pid 的低 16 位组成，跨重启可以区分（O1） |
| 错误码 | `code` | HTTP 里写成 `error.type = "aproxy_<code>"` 与头 `x-aproxy-error`，其余出口用裸码 |
| 尝试 | `attempt` 是序号，从 1 起；`attempts` 是总数；请求记录里的尝试明细列表叫 `tries` | S2-13：列表不能与总数同名 |
| 状态码 | 上游的叫 `upstream_status`；回给客户端的叫 `client_status`；format 消息的响应对象里叫 `status` | `client_status` 是已经写给客户端的状态码，响应头提交之前为 null；保活提交的骨架头也算（T-S3-5） |
| 地址 | `listen_addr`（`--listen-addr`）、`base_url`（`--base-url`） | 取代 `--listen`、`--baseurl`（D-4） |
| 日志 | 要求的位置叫 `log_file`；实际的位置叫 `log_path` | 两个概念 |
| 配置文件 | `config_path`；台账沿用 C3 的 `config` | |
| 闲置阈值 | `instances.idle_threshold_secs`，对应旗标 `--idle-threshold-secs` | 取代 settings 的 `idle_timeout_secs`（K12） |
| format 程序 | `[[format]].name` | 同一个名字也用于 IPC 的 `formats[].name`、`details.format`、`reports[].format` |
| 阶段 | 配置 `stages` 里的词，就是消息 `stage` 的取值 | |
| 投递方式 | `delivery`：`forward`（仅转发）或 `retry`（无限重试） | 配置里的 `delivery` 是默认值，format 可以按请求改（§10.10）；取代 0.1.x 的 `forward_only` |
| format 程序的进程方式 | `process`：`spawn` 或 `persistent` | |
| 实例的行为摘要 | 快照的 `behavior` | 默认投递方式、缓存、保活等开关（§6.4） |
| 请求体摘要 | `digest`，64 位小写十六进制 | 原始请求体的 SHA-256（§10.2） |
| 心跳拍号 | `heartbeat.beat` | 取代 `HeartbeatInfo.seq`；`seq` 留给消息序号 |

`state` 在 IPC 里指实例状态，在 format 协议里指程序存下的值。两者各在自己的协议里，不改名。

`mode` 不作任何字段名。早先的稿子里它同时指快照的行为对象和 `[[format]]` 的进程方式，用户追加的按请求切换又是第三样，
同一个协议里一个名字指三样东西，读者只能靠上下文猜。所以三者分别叫 `behavior`、`process`、`delivery`。
`delivery` 说的正是两者的差别：响应怎样交给客户端（边收边转，还是缓冲后回放）、失败了重不重试。按 D-14，config 里不留旧名。

### 1.4 语言

- **给人看的**（命令行的文本输出与提示、`--help`、桌面界面）：随语言环境，en 或 zh-CN（W3）。
- **固定英文的**：
  - 全部日志；
  - HTTP 错误体与 SSE 终态事件；
  - IPC 与 `--json` 的 `message`；
  - format 协议消息、install.state；
  - `--help` 末尾的 skill 提示行。
- **机器字段**（`code`、`details` 的键与枚举值、事件名、阶段名）是英文标识符。
- **显示端**按 `code` 加 `details` 本地化；遇到不认识的码，显示英文 `message`。
- **`details` 只放数据**。唯一的例外是上游库或 format 给出的原文，例如 reqwest 的错误原因、format 的 `reason`，它们作为数据原样显示。

### 1.5 密钥只写不读，打码只有一个出口

**现状**：
- URL 的打码在库里（config.rs:854）；不透明密钥的打码在二进制一侧（util.rs:39），保留前 6 个字符，短密钥会整段露出（CI-22）。
- format 的 stderr 现在被丢弃（transform.rs:426）。
- reqwest 错误的 Display 会附上完整 URL，现行代码先 `without_url()`（proxy.rs:3207），但没写成规则（S2-9）。

**设计**：

**只写不读的字段**：
- `api_key`、`proxy_password`；
- `proxy`、`base_url`、`update.download_proxy` 这三个 URL 里的 userinfo；
- `[[format]].env` 的值；
- `[[format]].args` 里密钥旗标之后的值（沿用现行 SECRET_ARG_FLAGS，commands/config.rs:14-22）；
- 名字像鉴权的请求头的值。名字不分大小写：`authorization`、`proxy-authorization`、`cookie`、`set-cookie`，
  以及名字里含 `key`、`token`、`secret`、`auth`、`session`、`signature` 的头。下文称「密钥头」。
- settings.json 的 `defaults` 里出现的上述键与密钥头，规则完全相同（D-16）。

**读出口一律给占位对象**：
- 适用于全部读出口：IPC、`config show/get`、`settings show/get`、`--json`、doctor、日志、错误消息，以及桌面的 Tauri 桥接层
  （界面经操作层拿到的配置与记录，交出去之前先换成占位）。
- 占位对象的形式：
  - 基本形式是 `{"secret":true}`；
  - 值长 24 个字符及以上时，附提示 `"hint":"sk-a…f3a9"`（首 4 尾 4）；
  - 文本输出写成 `<secret sk-a…f3a9>`。

**占位对象写回等于不改**。对得上原值的规则（S2-10）：
- 标量键按键对应；`env` 与两张头表按键名对应。
- `args` 按「同名程序里，同一个密钥旗标的第几次出现」对应。
- 对不上就报 `secret_placeholder_unmatched`，要求重新输入，不去猜。文档引导用户把密钥放进 `env`。

**URL**：userinfo 一律打码，查询串的值也一律打码（`?key=***`）。

**请求记录在入库时打码，不在输出时**：
- 请求头按上面的名字规则打码；
- URL，包括 `sent.url`，按 URL 规则打码；
- 体先按 content-encoding 解码，再做已知密钥替换（见下与 §6.5）；压缩体若不先解码，替换就看不见密钥（R-S2-2）。
- 这样内存里的历史本身就不含明文，以后新增的出口也不会漏。

**已知密钥替换**：
- 替换什么：aproxy 自己知道的、长度不少于 8 的密钥原文（含 settings `defaults` 里的），包括：
  - `api_key`、`proxy_password`、代理 URL 里的口令；
  - format `env` 的值、args 里的密钥；
  - 两张头表里鉴权头的值。
- 替换成 `<secret>`。
- 在哪里替换：请求历史的体；format stderr 的每一行（写日志之前）；`report`；`fail.reason`。
- 分块到达的体按流替换：每次留住「最长已知密钥的长度减 1」字节的尾巴不入库，等下一块到了或流结束再一起处理，
  所以被块边界切开的密钥也换得掉。
- format 自己配置里的 key，aproxy 替换不到；协议文档要求 format 不得输出它们。IPC 文档写明：体里不保证没有第三方密钥。

**上游库给出的原文**（例如 `details.reason`）：先 `without_url()`，再经打码出口。

**永不输出**：format 消息整条，以及 format 的 `state`，不进日志，也不经 IPC。

**唯一出口**：`aproxy_ipc::redact`，主 crate 与桌面共用。validate 的诊断在源头就打码。

**为什么**：
- 用户定了「IPC 永不返回明文密钥，密钥只写不读」。
- 否决按查询参数名打码：漏遮的代价远大于少显示一个 `api-version`。
- 入库时打码，消除了「新出口忘了打码」这一整类缺陷。

### 1.6 安全边界与访问控制

**现状**：
- 命名管道用默认安全描述符（daemon.rs:1377 附近），默认 DACL 给 Everyone 读权限。
- unix 的 socket、run 目录、spool 都按 umask 创建（CP-09、S2-8）。

**设计**（SE1）：
- **信任模型**：同一用户的进程互相信任，它们本来就能读配置里的明文 key；防的是别的用户与远程。所以不设令牌。
  控制只走本地 IPC，不开网络端口。
- **home 的权限**（S2-8）：
  - unix 上新建 home 时设 0700，其下 aproxy 写的文件一律 0600；三个安装脚本同步这样做。
  - Windows 上新建 home 时设受保护的显式 DACL（当前用户、SYSTEM、Administrators），子项继承。`%USERPROFILE%` 下默认就是这样。
  - 这一条覆盖了 spool、logs、install、run、settings 与 config。
  - 含明文密钥的文件由 aproxy 写入时，在 Windows 上另设同样的 DACL，不依赖 home 由谁建：config.toml 与 configs/ 下的配置、
    settings.json（D-16 之后它也含密钥）、期望状态记录、spec 文件。
  - 既有的 home 不是新建的，也要收紧（R-S3-12）：install 与第一次写配置时，若 home 属于当前用户而权限过宽，就收紧到上面的设置。
    0.1.x、alpha.1 留下的 home，以及用编辑器写的 settings.json，否则会一直按 umask 或盘符的 ACL。
  - 临时文件（`*.tmp-*`）创建时就带最终权限：unix 用 `mode(0o600)` 加 create_new，Windows 在 CreateFile 时给安全属性。
    否则 rename 之前会有一个明文可被他人读取的窗口。
  - doctor 对组或他人能访问的 home 告警，码为 `file_permissions_too_open`。
- **端点**：
  - Windows 每个管道实例都带 SDDL `D:P(A;;GA;;;<当前用户 SID>)(A;;GA;;;SY)`，拒绝远程客户端；客户端核对服务端 pid。
  - unix 的 socket 为 0600。服务端经 `peer_cred` 只接受同 uid 或 root 的连接（root 本来就无所不能，放行它，`sudo aproxy stop` 才能用）；
    客户端核对服务端的 uid 等于自己；客户端是 root 时，改为核对它等于 home 的属主。
- **敏感操作**：写类 op（`shutdown`、`prepare_swap`）与读体的 op（`body`）都靠端点访问控制保护；`body` 在访问控制落地之后才开放（§14 B7 → B8）。
- **桌面**：
  - 桥接层只暴露原型 `WriteCapability` 里的写操作：control（start、stop、restart）、config（写配置）、install。
    绝不暴露原始的 `prepare_swap`，也不暴露带任意参数的 `shutdown`。
  - 扩展的同意门在 Rust 侧强制（G9）；webview 禁止外部网络（W5）。

**为什么**：
- 否决令牌：能读配置的同用户进程同样能读到令牌。
- 否决经 IPC 写配置：写面越小越好。
- 让 home 本身收紧权限，比逐个文件列举权限可靠。第一版列漏了 spool，而 spool 里存的是对话全文。

---

## 2 错误码（K1）

**现状**：
- 失败原因只是字符串，同一段中文同时进入 HTTP 体、SSE、IPC、信封与日志（DP-03）。
- `note_upstream_failure`（proxy.rs:432）有 20 个调用点。
- IPC 的 `last_error` 是 `{message, at}`（daemon.rs:211），只有 4 个错误码（daemon.rs:279-289）。

**设计**：
- **单源**：码表放在 `aproxy_ipc::code`（§14 B0）。每个码登记这些元数据：
  - 名字与族；
  - `details` 每个键的名字与类型（字节数、毫秒、路径、字符串、字符串数组等），供 W3 按类型格式化；
  - 数据面终态码的 HTTP 状态：固定值，或「有上游或 format 的响应就透传，没有时为 N」；
  - 操作码的退出码：固定值，或「按子进程报的码」。
- **导出**：码表另导出一份 JSON，供界面的 i18n 使用。docs 与 skill 的码表由测试从它渲染并比对。
- **规则**：snake_case，全局唯一，只增不改。

### 2.1 数据面终态码

| 码 | HTTP | 何时 | `details` |
|---|---|---|---|
| `inbound_host_rejected` | 403 | Host 不在允许范围 | `host` |
| `inbound_origin_rejected` | 403 | 带了不允许的 Origin | `origin` |
| `request_too_large` | 413 | 请求体超过 `max_request_mb` | `limit_bytes` |
| `client_body_aborted` | 400 | 读客户端请求体时中途失败 | — |
| `upgrade_unsupported` | 501 | Upgrade（WebSocket）请求（综C7） | `upgrade` |
| `local_storage_failed` | 507 | 请求体或响应落盘失败（综C12） | `op` |
| `response_too_large` | 502 | 上游响应超过 `max_response_mb` | `limit_bytes` |
| `retry_exhausted` | 透传，否则 502 | 规则或按请求覆盖给的次数用尽 | `by`（`rule:<序号>` 或 `override`）、`last_code` |
| `retry_stopped` | 透传，否则 502 | format 的 decide 叫停 | `format`、`last_code` |
| `format_rejected` | 透传，否则 422 | 请求侧 format 回 `fail` 且 `retry:false` | `format`、`stage`、`fail_code` |
| `format_unavailable` | 503 | required 程序已熔断，请求方向的阶段用不了它（§10.9） | `format`、`stage`、`kind`（使它熔断的那次失败）、`probe_at` |
| `format_failed` | 502 | forward 请求的请求侧 format 失败或要求重试，且没有升级为 retry（§10.10） | `format`、`stage`、`kind` 或 `fail_code` |
| `upstream_failed` | 502 | forward 请求的上游失败，且没有升级（§10.10）；响应头已写出时不再发 502，只记录 | `last_code` |
| `decode_failed` | 只作 SSE 事件 | 流已提交，上游的压缩编码无法携带，解码又失败了 | `encoding` |
| `shutting_down` | 503 | 实例正在退出（综C10） | — |
| `internal` | 500 | 程序缺陷 | — |

「透传」指：有上游响应（或 format 给的响应）时，原样返回它，状态码是它的，只加 §3 的两个头。

### 2.2 尝试失败码

每一次上游尝试没有成功，都记一个尝试失败码。默认全部无限重试，规则表与 format 可以改变这一点。

| 码 | 含义 | `details` |
|---|---|---|
| `upstream_connect_failed` | 连不上：DNS、被拒绝、TLS | `reason`，已去 URL、已打码 |
| `upstream_connect_timeout` | 连接超时 | `timeout_ms` |
| `upstream_first_byte_timeout` | 请求已发出，等不到响应头 | `timeout_ms` |
| `upstream_read_idle_timeout` | 两次读到数据之间超时 | `timeout_ms`、`received_bytes` |
| `upstream_body_interrupted` | 响应体中途断开 | `received_bytes`、`reason` |
| `upstream_status` | 上游回 4xx 或 5xx | `status` |
| `upstream_error_body` | 上游回 2xx，但体是错误 | `status` |
| `format_error` | format 进程级失败或输出不合法，按 D-6 应重试 | `format`、`stage`、`kind`：`spawn`、`io`、`died`、`timeout`、`protocol`、`invalid_output` |
| `format_fail` | format 回 `fail` 且 `retry:true` | `format`、`stage`、`fail_code?` |
| `format_decision` | decide 把一次成功的尝试判为失败 | `format` |

### 2.3 IPC 协议码

| 码 | 何时 | `details` |
|---|---|---|
| `bad_request` | 不是 JSON、缺 `v` 或 `op`、行过长、参数类型或取值不对 | `reason` |
| `unsupported_version` | `v` 不受支持 | `supported`，现状里放在错误对象顶层（daemon.rs:271-277） |
| `unknown_op` | 没有这个 op，无副作用 | `op` |
| `unsupported_arg` | 带了不认识的参数 | `args` |
| `invalid_state` | 此刻不允许 | `state` |
| `condition_not_met` | 带条件的 op 条件不成立，例如 `shutdown{if_idle_secs}` 时实例不闲 | `idle_ms` |
| `busy` | 流式连接已满 | `limit` |
| `not_found` | 请求或它的体取不到 | `request_id`、`reason`：`unknown_request`（请求已不在记录窗口里）、`evicted`、`not_retained`（另带 `cause`：`budget_disabled`、`exceeds_budget` 或 `decode_failed`）、`no_such_part` |
| `internal` | 程序缺陷 | — |

### 2.4 操作码、诊断码与退出码

**通用退出码**（D-3、G5、C7）：

| 码 | 含义 |
|---|---|
| 0 | 成功，或目标状态已经达成（stop 一个本来就停着的目标、start 一个已用同一配置在跑的实例） |
| 1 | 操作失败 |
| 2 | 用法错误：clap 的解析错误、该指定目标时没指定、home 无效 |
| 3 | 配置无效：config.toml 或 settings.json 有 error 级诊断 |
| 4 | 目标未运行或不存在（stop 例外） |
| 5 | 部分成功 |

- C7 规定通用退出码只用 0–5，10 起归强制安装。被信号结束时退出码由操作系统给出，不在表内。
- 强制安装遇到 home 无效时退出 10（C8 第 1 步），不是 2。

**操作码**：

| 码 | 退出码 | 何时 | `details` |
|---|---|---|---|
| `usage` | 2 | clap 以外检出的用法错误 | `reason` |
| `home_invalid` | 2 | APROXY_HOME 不符合 C1 | `value` |
| `target_ambiguous` | 2 | 多个实例在跑，却没指定目标 | `ports` |
| `target_not_found` | 4 | 别名、端口或配置文件不存在 | `target` |
| `target_not_running` | 4 | 目标没在运行 | `target` |
| `config_invalid` | 3 | 有 error 级诊断 | `diagnostics` |
| `write_conflict` | 1 | 写配置时文件被外部修改，重试后仍冲突 | `file` |
| `port_in_use` | 1 | 端口被占 | `port`、`owner`：`aproxy`（本 home 的实例）或 `unknown` |
| `start_failed` | 按子进程报的码，否则 1 | 守护起不来 | `code?`、`startup_log` |
| `ready_timeout` | 1 | 时限内没到 `serving` | `timeout_ms` |
| `restart_unsupported` | 1 | 目标没有期望状态记录（前台实例） | `port` |
| `instance_unresponsive` | 1 | 实例在，但不应答 | `port`、`pid` |
| `instance_incompatible` | 1 | 应答了，但协议不兼容或身份不符 | `port`、`reason` |
| `stop_failed` | 1 | 停不掉 | `port`、`reason` |
| `no_log_file` | 1 | 前台实例没有日志文件 | `port` |
| `desktop_unavailable` | 1 | 在 headless 版本上执行 `desktop` | `variant` |
| `skill_not_installed` | 4 | 本地没有这个 skill | `name` |
| `skill_file_not_found` | 4 | 文件不在 skill 清单里 | `name`、`file` |
| `internal` | 1 | 程序缺陷 | — |

**IPC 结果到 CLI 的对应**（S2-12）：

| IPC 的结果 | CLI 的结局或操作码 | 退出码 |
|---|---|---|
| 端点不存在 | stop 计为 `already_stopped`；其余报 `target_not_running` | 0；4 |
| 超时、管道忙 | `instance_unresponsive` | 1 |
| `unsupported_version`、对端身份不符 | `instance_incompatible` | 1 |
| `unknown_op`、`unsupported_arg` | 先按 `features` 探测，正常不会出现；出现就报 `instance_incompatible`，`details` 带 op 或参数名 | 1 |
| `condition_not_met`（idle 目标） | 结局 `skipped`，`reason` 为 `not_idle`，计为成功 | 0 |
| `invalid_state` | 对正在退出的实例再发 shutdown 是幂等成功；其余报 `stop_failed` | 0；1 |
| `bad_request`、`internal` | `internal` | 1 |

**配置诊断**：
- 每条诊断是一个对象 `{"code","severity","file","key"?,"line"?,"column"?,"message","details"}`。
- 它们出现在 `config_invalid` 的 `details.diagnostics`、doctor、`config show` 与 start 的输出里。
- `severity` 取 `error`、`warning` 或 `info`；兜底按 `warning` 显示。只有 `error` 影响退出码；`info` 是 doctor 的提示，不计入 doctor `--json` 的 `summary`（T-S3-8 ②）。

| 码 | 级别 | 何时 |
|---|---|---|
| `config_syntax` | error | TOML 或 JSON 语法错误，带行列 |
| `config_type` | error | 类型不对 |
| `config_invalid_value` | error | 取值不合法，带 `reason` |
| `config_missing_key` | error | 缺了必填的键 |
| `config_unknown_key` | error | 不认识的键（D-14），带键路径与文件 |
| `config_not_supported` | error | 只定了形状、还没实现的写法：规则的 `body`、阶段 `admit` |
| `config_conflict` | error | 互斥的组合，见 §7.4 |
| `secret_placeholder_unmatched` | error | 写回的占位对象对不上原值 |
| `default_credential_withheld` | warning | settings `defaults` 里的凭据因目标不同没有继承，而这个配置自己也没有这一项（D-16，§7.5），带 `key`、`reason` |
| `default_header_shared` | info | `defaults` 的头表里有不属于密钥头的头，它们会发给所有目标；若其中有凭据（名字少见的凭据头），应移进配置（§7.5） |
| `forward_default_unguarded` | warning | 默认 `delivery = "forward"`，且链上有声明 decide 的程序，或有声明 request 的可选程序：它们不可用时请求不会转入 retry，一律不重试（§10.9，T-S3-1）。required 的 request 程序不算，它不可用时请求以 503 结束，看得见；一个 format 都没有时不告警 |
| `settings_schema_too_new` | error | settings.json 的 schema 高于本版本 |
| `format_handshake_failed` | 见 §10.2 | 启动时握手失败，带 `format`、`kind` |
| `format_stage_unsupported` | error | 程序不支持配置声明的阶段，带 `format`、`stages` |
| `file_permissions_too_open` | warning | doctor 发现组或他人能访问 home 或配置文件 |

**记录码**：`record_unreadable`、`record_schema_too_new`，用于枚举结果的 `unreadable` 与 doctor（§4.3、§6.6）。

**安装码**用 `install_*` 一族，例如 `install_in_progress`、`install_version_not_found`（退出码 4）、`install_below_floor`、
`install_download_failed`、`install_checksum_mismatch`、`install_probe_failed`、`install_swap_failed`、`install_rolled_back`、
`install_unmanaged_instances`（退出码 1，§4.3 第 7 条）、`install_precheck_failed`（退出码 3，滚动重启之前的预检不通过，带 `port` 与 `diagnostics`，§5）。
- install 的实现者做 `--json` 时补齐；install.state 的 `error_code` 与 NDJSON 用同一套码。
- 强制安装只承诺 C7 的退出码。

**为什么**：
- 码是一切渲染的输入，必须最先定。
- HTTP 用 `error.type` 承载码：Anthropic 与 OpenAI 的 SDK 都在那里读字符串类型；Gemini 的 `error.code` 是整数（DP-01）。
- `format_rejected` 用 422：5xx 会触发 SDK 自己的重试（X-2）。
- `format_unavailable` 用 503：熔断是暂时的，冷却后会再试探。
- 综C6（非法上游 URL 归为终态）被 D-6 取代：format 改写出非法 URL 属于「输出不合法」，单次按应重试，连续出现由熔断兜住（D-12）。

---

## 3 HTTP 错误体与 SSE 终态事件（K2）

**现状**：
- 自产错误是 text/plain 的中文，散在 proxy.rs 多处：291-322、1296-1303、1613-1629、1654-1669、3042、3236-3244、3360-3375。
- SSE 终态有一个通用构造（proxy.rs:1633），另有四处手写（proxy.rs:2846-3025），其中一处用 `format!` 拼 JSON（DP-01、DP-02）。

**设计**：

响应体用 `content-type: application/json`：

```json
{"type":"error","error":{"type":"aproxy_retry_exhausted","message":"upstream returned 404 on all 3 attempts allowed by retry rule 1","aproxy":{"code":"retry_exhausted","request_id":"19ac3f2e8b5d1a2b-41","attempts":3,"upstream_status":404,"details":{"by":"rule:1","last_code":"upstream_status"}}}}
```

- `type` 恒为 `"error"`；`error.type` 等于 `"aproxy_"` 加码；`error.message` 是一句英文，不是契约。
- `error.aproxy` 是 A 级对象：`code`、`request_id`、`attempts`（已进行的上游尝试数）、`upstream_status`（没有就省略）、`details`。
- `error.code` 不放任何值。

响应头 `x-aproxy-request-id` 与 `x-aproxy-error`：
- 加在这些响应上：aproxy 自产的错误响应；放弃重试之后透传的上游响应（`retry_exhausted`、`retry_stopped`）；
  format 给出的终态响应（`format_rejected`、decide 叫停时附带的响应）。
- 不加在这些响应上：正常成功的透传；forward 请求没有升级时透传的上游非 2xx（这是 forward 的本义，aproxy 并没有放弃，S3-7）。

SSE 终态，用于响应头已提交之后才发生的终态：

```text
event: error
data: {"type":"error","error":{"type":"aproxy_format_unavailable","message":"format 'keys' is unavailable (circuit open)","aproxy":{"code":"format_unavailable","request_id":"19ac3f2e8b5d1a2b-43","attempts":2,"details":{"format":"keys","stage":"retry","kind":"died","probe_at":1760190030000}}}}

```

- 与 HTTP 体是同一个 JSON，全部经同一个构造函数用 serde 序列化。
- 提交之后，format 才给出终态响应时（S3-8）：
  - 它的 `content-type` 是 `text/event-stream`：把体原样写进流，然后结束；
  - 它的体是 JSON：压成一行，作为 `event: error` 的 data 写出；
  - 都不是：写 aproxy 的标准终态事件。

**为什么**：
- 结构与英文化一次到位，skill 与 agent 的锚点只断这一次（DP-01）。
- 放弃之后透传的响应也加头：带 `x-aproxy-error` 的非 2xx 就是 aProxy 已经放弃、不会再重试的最终结果；
  forward 请求原样透传的非 2xx 不带它。头让 agent 能分清这两种情形，体仍然保真。
- 验收要求用真实的 Anthropic 与 OpenAI SDK 各解析一次。

---

## 4 磁盘布局与持久记录（K5）

**现状**：
- 有两个根：APROXY_HOME 与 APROXY_RUN_DIR（daemon.rs:27）。
- unix 心跳在 /dev/shm（watchdog.rs:1436、1464）。
- `.pid` 的字段「全部必填」（daemon.rs:140-168）；`.restore` 存 `{args, log_path}`（daemon.rs:1028-1078）。
- status 普查时顺带删孤儿日志与解析不了的记录（daemon.rs:744-882）。
- spool 按配置端口分目录，并在 bind 之前清空（server.rs:92）。

### 4.1 布局

```text
<home>/                            唯一的根（C1）；unix 0700，Windows 显式 DACL（§1.6）
  config.toml  configs/            用户配置
  settings.json                    §8
  bin/                             C2（F）
  install/                         C4（F）；state.json，unix 的 announce
  ledger/<pid>.json                C3（F）
  skills/  skills/manifest.json    W4（A，schema 1，见 §4.2）
  desktop/                         桌面私有：webview 数据、置顶条目、扩展授权等（由 W5 定）
  logs/
    startup.log  watchdog.log  install.log    固定名（A），内容英文
    <纳秒hex>-<pid低16位hex>.log               实例日志，随机名（命名模式 A）
  spool/<pid>-<start 短哈希>/       按进程分目录（目录规则 A）
  run/                             0700
    <port>.instance.json           实例记录（A）
    <port>.restore.json            期望状态记录（A）
    <纳秒>-<pid>.spec.json          拉起规格的交接文件（A）
    <port>.sock                    unix 控制端点
    <port>.heart                   unix 心跳
    watchdog.json                  看护者记录（A，原 watchdog.claim）
    config-write.lock              配置写者之间的锁（§7.6）
    *.tmp-<pid>-<纳秒>              原子写的临时文件
Windows 内核对象（A）：
  \\.\pipe\aproxy-<home_id>-<port>      实例控制端点
  Local\aproxy-<home_id>-heart-<port>   心跳节
  Local\aproxy-<home_id>-install        安装宣告节
```

- **`home_id`**：规范化后的 `<home>/run` 路径做 SHA-256，取前 16 位十六进制（daemon.rs:107）。
- **`start 短哈希`**：启动标记字符串做 SHA-256，取前 8 位十六进制。
- **心跳**：
  - 节或文件的前 8 字节是小端 u64 毫秒时间戳。
  - 写心跳放在专用线程里，home 在网络文件系统上时也不阻塞 tokio 的工作线程（S3-36）。
- **安装宣告**：16 字节，[0,4) 是 pid（u32 小端），[4,8) 为 0，[8,16) 是心跳毫秒（u64 小端），与现行实现一致
  （announce.rs:212-217、296-299，S3-9）。
  - 已在 alpha.1 落地（6671ff2、bc18fda，W2b 负责人确认）：unix 的安装宣告是 `<home>/install/announce`，C4 随之改为「没有例外」，
    强制安装第 4 步清空 `install/` 时就清掉了；没有 state.json 时续作删掉过期的宣告。
  - 旧的 /dev/shm 位置只有早期开发构建写过（tmpfs，重启即无），不读也不清理。
- **一个端口一个实例**：记录与控制端点都按端口命名，所以一个 home 里同一端口只能有一个实例，127.0.0.1:12345 与 [::1]:12345 不能并存。
  这是现行限制，写成显式约束（S3-11）。
- **台账（C3）与实例记录分开**（G3）：台账只回答「有哪些 aproxy 进程、怎样结束它们」，实例记录服务 status 与界面。

### 4.2 记录

所有记录的共同点：
- 都是一个 JSON 对象，首个字段是 `schema`（当前为 1）；
- 由属主一个人写；
- 原子写入：create_new 建临时文件，写完 `sync_all`，再 rename；
- 文件权限 0600。

**实例记录** `run/<port>.instance.json`：
- 实例在 bind 之后写，正常退出时删。实例每 30 秒核对一次，记录丢了就补写（CP-24）。
- `base_url` 已打码。`foreground` 为 true 表示前台实例：日志写在控制台，`log_path` 为 null，也没有期望状态记录。
- IPC 快照里的 `instance`，就是这个对象去掉 `schema`。
- `config_path` 与 spec 的一样可以是 null：没有配置文件、只靠覆盖参数与 settings `defaults` 启动的实例（R-S3-13、T-S3-7）。
  显示端据此显示「没有配置文件」。

```json
{"schema":1,"pid":4321,"start":"w:133712345678901234","instance_id":"19ac3f2e8b5d1a2b","version":"0.2.0","listen_addr":"127.0.0.1:12345","config_path":"C:\\Users\\u\\.aproxy\\config.toml","base_url":"https://api.example.com","started_at":1760190000123,"log_path":"C:\\Users\\u\\.aproxy\\logs\\19ac3f2e8b5d-1a2b.log","foreground":false}
```

**拉起规格 InstanceSpec**：它是 spec 文件的全部内容，也内嵌在期望状态记录里。

```json
{"schema":1,"config_path":"C:\\Users\\u\\.aproxy\\configs\\work.toml","overrides":{"listen_addr":"127.0.0.1:12346","api_key":"sk-ant-…"},"pinned_port":null,"env":{"HTTPS_PROXY":"http://127.0.0.1:7890","HTTP_PROXY":null,"ALL_PROXY":null,"NO_PROXY":"localhost,127.0.0.1","PATH":"C:\\Windows\\system32;…"},"log_path":"C:\\Users\\u\\.aproxy\\logs\\19ac3f2e8b5d-1a2b.log"}
```

- **`config_path`**：没有配置文件、只靠覆盖参数与 settings `defaults` 启动的实例，记 null；restore 与看护者对它不做存在检查
  （R-S3-13）。现行允许这种用法：文件不在就用默认配置（config.rs:831），D-8 不能因此失效。
- **`overrides`**：键就是 config.toml 的键，只含 start 的覆盖旗标实际给过的。0.2.0 有五个：`base_url`、`listen_addr`、`proxy`、`api_key`、`log_file`。
- **`pinned_port`**：解析出的端口为 0 的实例，写期望状态记录时把实际端口记在这里（§5 第 7 步，R-S2-3）；其余为 null。
  - 拉起时，配置（含覆盖）解析出的端口仍是 0 才用它；配置已改成固定端口，就以配置为准（T-S3-6）。
    所以它不放进 `overrides`：覆盖的优先级高于配置（§7.5），放在那里，用户以后改的端口永远不生效，也不会被察觉。
  - 客户端配的是这个端口，恢复时要回到同一个端口；端口被占，就按固定端口的常规失败处理。
- **`env`**：启动者环境里的代理变量与 `PATH`（D-2）；null 表示没设，拉起时在子进程里删掉它。
  - Windows 上变量名不分大小写：每个变量只记一个大写的名字，快照时不分大小写地查找（S2-2）。
    样例就是 Windows 的形状：4 个代理变量加 `PATH`。
  - unix 上大小写两种名字各记一条：`HTTPS_PROXY` 与 `https_proxy`、`HTTP_PROXY` 与 `http_proxy`、`ALL_PROXY` 与 `all_proxy`、
    `NO_PROXY` 与 `no_proxy`，再加 `PATH`。
  - reqwest 用 rustls 与 webpki 根证书，TLS 相关的环境变量不影响它，所以不记。
- **`log_path`**：新启动时由拉起方生成随机名填进来；restart、restore、看护者重拉都续写同一个文件。
- **密钥**：`overrides.api_key` 与代理凭据按 D-2 以明文存放（权限见 §1.6），永不进入任何命令行。

**期望状态记录** `run/<port>.restore.json`：`{"schema":1,"spec":{…InstanceSpec…},"written_at":1760190000456}`。
- 实例启动时写，前台实例不写。
- 只有 stop 删它：不带 `keep_restore` 的 `shutdown`、`stop all`，以及 stop 一个已停的目标。
- 唯一的例外是被接替：用它拉起的实例落在了别的端口（配置里的端口改了），新实例就绪之后（或那个端口上已有同一配置的实例在跑），
  拉起方删掉这份旧记录，那个端口的记录接替了它（T-S3-6）。不删的话，看护者会一直按旧记录去拉一个已经搬走的实例。
- 配置文件暂时找不到时，restore 与看护者都保留它（S1-4、§5）；`config_path` 为 null 的不检查。

**看护者记录** `run/watchdog.json`：看护者每轮扫描时整份重写。
- `restarts` 保留最近 20 次；`kind` 的词表是 `crash`、`hang`。
- 读不懂这份记录的看护者按文件 mtime 判断它是否新鲜，不覆盖它。

```json
{"schema":1,"pid":5555,"start":"w:133712345000000000","version":"0.2.0","heartbeat_secs":30,"created_at":1760190000000,"last_scan_at":1760190123456,"restarts":[{"port":12345,"at":1760190100000,"kind":"crash","ok":true}]}
```

**安装状态** `install/state.json`：
- 保留现有字段，新增四个可选字段（A5）：`failed_at`（失败时的相位）、`installer_version`、`error_code`、`error_details`。
- 相位词表只增，读者把不认识的相位当作 `unknown`。
- 不存 spec，滚动重启靠 `keep_restore`（§5），所以它不含密钥。
- 现有的 `pending_restores[].args` 是启动参数，可能含 `--api-key`（state.rs:152-160）。W2 写出的 `pending_restores` 恒为空数组，
  只为旧版本的读者保留这个字段，W2 自己也不读（R-S3-11）。
- 现有的 `halted` 为 true，表示失败的原因不会自己消失：续作不再自动接管这个现场，用户排除原因后重新执行 install，由新的 install 接管它
  （alpha.1 的 flow.rs:713-731 已是这个语义）。W2 在 §4.3 第 7 条与 §5 的预检失败时都置它（T-S3-2、T-S2-2）。

**skill 本地清单** `skills/manifest.json`（W4 的跨版本文件）：`{"schema":1,"version":"0.2.0","files":[{"path","bytes","sha256"}]}`。
- 读者遇到解析失败或 schema 更高时，按「已安装、版本未知」处理，不删除。
- 有 skill 目录、没有清单时同样按「已安装、版本未知」处理：从 alpha.1 交棒的升级，skill 由 alpha.1 的安装者安装，它不写清单（R-S2-4）。

### 4.3 读者与清理规则

1. **读不懂的记录**：解析失败，或 `schema` 高于自己认识的，就跳过，并在 status 与 `--json` 的 `unreadable` 里报告；绝不删除。
2. **实例记录与看护者记录**：只在同时满足两个条件时清理：本版本的 schema；写者经 `pid` 加 `start` 核验已经不在。
   期望状态记录不归清理管，只有 stop 能删（被新端口的记录接替时例外，见 §4.2）；`stop all` 也只删本版本 schema 的，其余的报告出来（S3-13）。
3. **spec 文件**：拉起方在子进程就绪或失败之后立即删（S3-15）。临时文件与遗留的 spec 文件，在写者 pid 已不在、且早于 10 分钟时由清理删除。
4. **读路径不删任何文件**：status、enumerate、界面轮询都是读路径。清理只在实例进入 `serving` 之后与看护者的周期里做（综C1）。
5. **日志 GC**：
   - 只处理同时满足三个条件的文件：名字符合 `^[0-9a-f]{1,16}-[0-9a-f]{4}\.log$`；早于 10 分钟；不被任何实例记录或期望状态记录引用。
   - run/ 里有读不懂的记录时，引用集不完整，整轮暂停；status 与 doctor 说明暂停的原因（S3-12）。
   - 自定义的 `log_file` 与三个固定日志永远不删，只按 `logs.rotate_mb` 轮转。
6. **spool**：实例只写自己的目录；进入 `serving` 后，删除那些 pid 已不在、或 pid 还在但启动标记对不上的目录（S3-10）。
7. **W2 之前的运行期记录**：0.1.x 与 W2 落地之前的 0.2.0 预发布版写的 `run/*.pid`、`run/*.restore`、`run/watchdog.claim`，
   以及 /dev/shm 下的心跳文件，0.2.0 不读也不清（与 D-14 同一原则：不写识别旧格式的代码）。
   - 这些预发布版启动的进程都登记在台账（C3）里。强制安装按台账结束它们，并按台账的 `port` 与 `config` 告诉用户怎样重新启动（C8 第 6 步）。
     0.1.x 不写台账，C8 第 7 步只报告它们。
   - 检查管不了的实例：台账里有活着的 instance，而 run/ 里没有这个端口的本 schema 实例记录。查两次（R-S2-4、T-S3-3）：
     - 普通 install 在入口查，加锁与下载之前。查到就报错退出，不加锁、不下载、不写 install 状态，真正做到在改动任何东西之前失败。
     - 所有路径（含续作 `install --continue`，包括从 alpha.1 交棒过来的）在广播之前再查一次，也覆盖这段时间里新起的实例。
       查到就以失败结束，实例与 bin 都不动：
       - 置 `phase = failed`、`halted = true`、`error_code = install_unmanaged_instances`；`last_error` 写英文原因，alpha.1 的等待者只读这个字段；
       - 释放锁与宣告；
       - 提示用户先用当前版本执行 `aproxy stop all` 再安装，或改用 `install --force`。
   - 要置 `halted`（T-S3-2）：alpha.1 的任何非 install 命令看见 install/state.json，都会静默拉起续作，
     而它的续作只拒绝「failed 且 halted」的现场（73b7863 的 main.rs:128-146、flow.rs:713-731）。不置的话，用户照提示执行
     `aproxy stop all` 时，这条命令本身就会拉起一个续作去接管失败的现场。W2 自己的自动续作同样不接管它（§9.2 第 6 步）。
   - 只在入口查不够：从 alpha.1 升级时，管辖检查、加锁、下载与校验都由 alpha.1 的安装者做完，
     W2 以续作接手时入口早已过去。不检查的话，alpha.1 的实例会失去看护者，又不在 W2 的枚举里，成了没人管的孤儿。
   - 迁移文档（W7）写明怎样手动删除残留。

**为什么**：
- home 作为唯一的根：寻址只剩一个变量（CP-05）。
- 平铺加 kind 后缀：一个 0700 目录最简单（CP §5）。
- 心跳挪进 home：macOS 没有 /dev/shm，容器里它很小（CP-15）。
- 读者绝不删除：0.2.0 是今后所有版本面对的最老读者（K5）。
- 期望状态记录只有 stop 能删：它表达的是用户的意图，「进程不在」正是它要恢复的情形，不能当作垃圾清理（S1-4）。
- 旧格式靠台账迁移：W2b 不冻结运行期记录（计划 W2b「不冻结」一节），而台账是专为跨版本找到并结束进程而冻结的。
  对它们写读取代码，就是 D-14 否决的兼容层。

---

## 5 拉起、期望状态与进程生命周期（K6、D-2、D-8）

**现状**：
- 守护靠隐藏的全局旗标 `--daemon-child` 拉起（cli.rs:51），restore、restart、看护者各自拼一次命令行。
- 看护者复制启动者的全部参数，包括 `--api-key`（watchdog.rs:339）。
- unix 不 setsid；Windows 有意不加 BREAKAWAY（daemon.rs:598）。
- 就绪时限各处都是 8 秒（start.rs:274、watchdog.rs:968 等）。
- restart 先预检、后停止（restart.rs:8-14）；install 停止之后立即回写 `.restore`（install/restart.rs:17-24）。
- 配置文件不在时，restore 与看护者会删掉记录（restore.rs:30-36、watchdog.rs:954）。
- 安装的早交接以 `install --continue --handover-from <pid>` 拉起目标版本，代码已声明它是永久接口（install/flow.rs:12-18）。
- install 换二进制之前读 `watchdog.claim` 停掉旧看护者（install/flow.rs:958）。

**设计**：

**内部入口**（A）。它们会跨版本调用：早交接由旧版本拉起新版本；滚动重启中新版本起不来时，由新版本拉起旧版本。
- `aproxy __serve --home <dir> --spec <file>`：后台实例，取代隐藏旗标 `--daemon-child`；
- `aproxy __watchdog --home <dir>`：看护者，取代 `--daemon-watchdog`；
- `aproxy install --continue [--handover-from <pid>]`：续作与早交接，保持原样。alpha.1 的安装者靠它把安装交给目标版本，改名会让
  从 alpha.1 起的普通升级在交棒处失败。
- skill 的「Processes you may see」一节改为列出它们。

**拉起函数** `spawn_from_spec(spec)`，start、restart、restore、看护者、install 五处共用：
1. 写 spec 文件（0600）。CLI 用自己的可执行文件拉起 `__serve`；桌面界面一律用 `<home>/bin/aproxy`，不存在就引导用户安装
   （U6、CP-19：否则从别处启动的界面拉起的实例，以后会被 install 拒绝管辖）。
2. 子进程环境等于拉起方的环境，再按 `spec.env` 设置或删除那些变量。工作目录固定为 home（综C4）。
3. 脱离启动者（综C3）：
   - unix 只用 `setsid()`，经 `pre_exec` 调用（S2-1：它与 `process_group(0)` 同用会得到 EPERM）；
   - Windows 先带 `CREATE_BREAKAWAY_FROM_JOB`，被拒就去掉再试一次，并在 startup.log 记一条告警：实例仍在启动者的作业对象里。
4. 等就绪：
   - 等到实例的状态变为 `serving`；子进程退出则立即判失败，退出码按 §2.4 映射；
   - 时限 30 秒（E），覆盖并行握手的 10 秒下限（S2-4）；
   - 失败时摘录 startup.log 里本次写下的行。

**实例的启动顺序**：
1. 读 spec，解析配置（§7.5）并校验。解析出的端口为 0、而 spec 带 `pinned_port` 时，改用它（§4.2）。
2. 写台账。C3 要求在占用端口或拉起子进程之前写。
3. bind 代理端口，先不接受连接。端口为 0 时，用实际端口原子重写台账条目（S3-16）。
4. bind 控制端点，以 `state:"starting"` 应答。
5. 写实例记录。
6. 与各 format 程序并行握手（§10.2）。
7. 写期望状态记录（S2-5）：它与 shutdown 的删除在同一把进程内锁下进行；收到不带 `keep_restore` 的 shutdown 之后，不再写它。
   解析出的端口为 0 时，把实际端口记进记录里 spec 的 `pinned_port`（R-S2-3、T-S3-6）。否则记录按实际端口命名、spec 却还是端口 0，
   每次恢复都换一个端口、多留一份拉不回的记录，看护者会越拉越多。
8. 开始接受代理连接，状态转为 `serving`；之后做清理（§4.3）。

这期间到达的代理连接在 TCP 积压队列里等待，几秒之内就会被接受。任何一步失败，都用英文写进 startup.log（带 `code=`），并按 §2.4 的退出码退出。

**停止**（综C10）：
- 不带 `keep_restore` 的 shutdown，立即删期望状态记录，然后排空：已提交的保活流写 `shutting_down` 终态事件；还没提交的请求回 503。
- `stop all`：停掉全部活实例，再删本 schema 的、没有活实例的期望状态记录。
- `stop <目标>` 遇到已停、但还留着期望状态记录的目标，删掉那份记录，结局为 `already_stopped`。

**restart**（S1-4）：
1. 读目标的期望状态记录。前台实例没有这份记录，报 `restart_unsupported`。
2. 预检：用记录里的 spec 走 §7.5 的管线（端口仍为 0 时用 `pinned_port`）；得出的地址与旧实例的不同，就探测新端口能否绑定。预检不通过就不碰旧实例（退出码 3 或 1）。
3. 先按 §6.2 第 5 条查旧实例快照的 `features`（R-S3-14）：
   - 有 `shutdown.keep_restore`：发 `shutdown{keep_restore:true}`，等旧实例退出，记录保留；
   - 没有：发不带参数的 shutdown（它会删掉记录），等旧实例退出后，立即用同一份 spec 回写记录。
   不能靠 `unsupported_arg` 判断：W2 之前的实例静默忽略未知参数（daemon.rs:1215），而 op 被拒时实例也不会退出。
4. `spawn_from_spec`。成功时，新实例覆盖记录；端口变了时，按 §4.2 删掉旧端口的记录。失败时记录还在：看护者按 D-8 继续拉，`aproxy restore` 也能拉回。
5. 拉起时端口已被同 home、同一配置的实例占着（看护者抢先拉起了），计为成功。

**install**：
- 滚动重启用同一套原语；install/state.json 只记端口。
- 换二进制之前，按台账（role 为 watchdog）找到并结束旧看护者，不读看护者记录，所以对任何版本的看护者都有效。
- 管不了的活实例：普通 install 在入口查一次，所有路径在广播之前再查一次，查到就以失败结束，实例与 bin 都不动（§4.3 第 7 条）。
- **预检**（T-S2-2）：执行续作的目标版本在广播之前，用自己的 §7.5 管线预检每个要滚动重启的实例，即它的 spec 加上当前的 settings.json。
  - 任何一个不过，就以失败结束，什么都不动：不广播、不换 bin、不停实例。置 `phase = failed`、`halted = true`、
    `error_code = install_precheck_failed`，`error_details` 带端口与诊断（哪个文件、哪个键），`last_error` 写英文原因。
  - 否则用户手改 settings.json 打错一处之后执行 install（R-S3-17 让它照常运行），健康的实例会被停掉，
    新旧二进制又都因 settings 无效起不来，看护者也已被结束，实例就此下线。
  - 这也覆盖「新版本不接受现有配置」的老情形：以前是先停、起不来再回退，中断一下；现在一个实例都不停。
  - 放在广播之前，而不是停每个实例之前：广播里的 ACK 收敛本身就可能重启实例（install/restart.rs:4-6）；
    换了 bin 之后再失败，又会留下「新 bin、旧实例」的现场。

**restore 与看护者**：
- 遇到配置文件暂时不在时，保留记录：restore 的结局为 `config_missing`，看护者照常慢速重试（D-8 原文的动机正是网络盘挂载得晚）。
  spec 的 `config_path` 为 null 时不做这项检查（§4.2）。
- 按记录拉起的实例落在了别的端口（配置里的端口改了）时，就绪之后删掉旧记录（§4.2，T-S3-6）。
- 看护者的命令行只有 `__watchdog --home`；每个 tick 重读 settings，开关关闭就退出，保留记录（CP-18）。
- settings.json 变得读不了时，看护者沿用上一次读到的有效值，并在日志里报错，限频（§8.2）。
- 实例存活满 60 秒，才清零它的失败计数。
- 连续失败达到 `watchdog.max_restarts` 之后，每 300 秒重试一次，直到记录被删（D-8）。
- 处决、重拉、慢速重试与它们的原因，都写进 logs/watchdog.log。

**不变的部分**：守护启动时退役比自己旧的看护者（server.rs:111），同样按台账找它；prepare_swap 的「ACK 齐了才交换」。

**为什么**：
- argv 落盘会把启动语法与明文 key 冻结成跨版本契约（CP-02）。结构化的 spec，任何版本的拉起方都能翻译成自己的命令行。
- 环境快照让重拉出来的实例走同一条网络路径（D-2）。
- 先 bind 端口、后握手，再以 `starting` 应答：端口冲突能尽早报出；界面能显示「正在启动」；拉起方不会把慢启动的 format 误判为失败。
  否决「先 bind 控制端点」：端口为 0 时，控制端点要等实际端口才能命名。
- `keep_restore` 比「停止后回写」少一个窗口：调用方在中途崩溃，记录也还在。
- 跨版本只依赖台账与内部入口：二者都是冻结的，其余记录可以随 schema 演进。

---

## 6 IPC v1（K4、G1、O2、SE1）

**现状**：
- 只有 ping、shutdown、prepare_swap 三个 op（daemon.rs:226-237），成功应答一律是状态快照。
- 未知参数被静默忽略（daemon.rs:1215）。
- 没有在途数、请求记录与事件；请求行上限 64 KiB，应答行上限 1 MiB（daemon.rs:1132-1172）。

### 6.1 传输与帧

- 端点见 §4.1，访问控制见 §1.6。
- 一元请求：客户端总超时 3 秒；服务端要求连上后 5 秒内发完请求行。
- 行长上限：请求行 64 KiB，应答行 16 MiB。体用 `body` 分块读取（§6.3），单块原始字节最多 1 MiB，所以应答行不随体变大。
- 帧：一行一个 JSON 对象，UTF-8 编码，以 `\n` 结尾。

```text
请求  {"v":1,"op":"shutdown","args":{"if_idle_secs":600}}
成功  {"v":1,"ok":true,"result":{…}}
失败  {"v":1,"ok":false,"error":{"code":"unsupported_arg","message":"unknown arg: drain_secs","details":{"args":["drain_secs"]}}}
事件  {"v":1,"event":"request","seq":42,"at":1760190000123,"data":{…}}
```

- 一元 op：一个请求、一个应答，然后关闭连接。
- `subscribe`：先回一行一元应答，之后每行一个事件，直到任一方关闭。
- 不做多路复用。

### 6.2 规则

以下每一条都用契约测试钉住。
1. 服务端用请求里的 `v` 应答；发布过的 `v` 一律保持支持；不支持就回 `unsupported_version`。
2. 未知 op 回 `unknown_op`；未知参数回 `unsupported_arg`；参数类型或取值不对回 `bad_request`。
3. 应答与事件只增字段；客户端忽略未知的字段与事件；`state` 读不懂就当作 `unknown`。
4. `message` 只给日志看，显示端按码渲染。
5. 快照的 `ops` 与 `features`（命名为 `<op>.<参数>`）先查再用。
6. 改变状态的 op 必须幂等或带条件；新的写类或敏感 op，只在访问控制落地之后加入。
7. 流式连接：
   - 每个实例最多 8 条；
   - 服务端至少每 15 秒发一行，客户端 45 秒收不到就判失联；
   - 实例退出前发 `closing`；
   - 事件的 `seq` 在每条连接上从 1 起递增；丢弃了事件就发 `lagged{dropped}`，客户端据此重取快照与记录。

### 6.3 op 一览

| op | 参数 | 结果 | 副作用 | 可能的错误 | features |
|---|---|---|---|---|---|
| `ping` | — | 快照 | 无 | — | |
| `shutdown` | `if_idle_secs?`、`keep_restore?`（默认 false） | 快照，state 为 `stopping` | 不带 `keep_restore` 时删期望状态记录；开始排空；重复调用结果相同 | `condition_not_met` | `shutdown.if_idle_secs`、`shutdown.keep_restore` |
| `prepare_swap` | `ttl_secs?`（默认 600） | 快照，state 为 `swap_prepared` | 到期自动回到 `serving`（CP-25） | `invalid_state` | `prepare_swap.ttl_secs` |
| `requests` | `before?`（一个 request_id）、`limit?`（1–200，默认 50） | `{"requests":[RequestRecord…],"more":bool}`，新的在前 | 无 | — | |
| `request` | `request_id` | RequestDetail（不含体） | 无 | `not_found` | |
| `body` | `request_id`、`part`（`request`、`sent` 或 `response`）、`attempt?`（`part` 为 `sent` 或 `response` 时必填）、`offset?`（默认 0）、`limit?`（1 字节到 1 MiB，默认 1 MiB） | `{"total_bytes","offset","data_b64","more","complete"}`，见 §6.5 | 无 | `not_found`；`offset` 超过 `total_bytes`、`limit` 为 0 或大于 1 MiB 时 `bad_request` | 访问控制（B7）落地后才列进 `ops` |
| `subscribe` | — | `{"snapshot":…,"requests":[在途的 RequestRecord]}`，之后是事件 | 无 | `busy` | |

- 日志不经 IPC：CLI 与界面按快照或记录里的 `log_path` 直接读文件；aproxy-ipc 提供一个跟随文件、能识别轮转的库函数，两边共用（审查 §4-1）。
- 配置也不经 IPC：界面经注入的操作层调用 §7.5 的 `resolve`，读写配置文件（审查 §4-2）；写完再 restart。

### 6.4 快照

```json
{"instance":{"pid":4321,"start":"w:133712345678901234","instance_id":"19ac3f2e8b5d1a2b","version":"0.2.0","listen_addr":"127.0.0.1:12345","config_path":"C:\\Users\\u\\.aproxy\\config.toml","base_url":"https://api.example.com","started_at":1760190000123,"log_path":"C:\\Users\\u\\.aproxy\\logs\\19ac3f2e8b5d-1a2b.log","foreground":false},
 "state":"serving",
 "behavior":{"delivery":"retry","disk_cache":true,"keepalive":{"trigger":"any","interval_secs":15},"retry_rules":1,"history":{"body_budget_mb":64}},
 "formats":[{"name":"adaptive","stages":["request","decide","notify"],"process":"persistent","required":false,"protocol":1,"version":"0.2.0","notify_dropped_total":0,
             "breaker":{"state":"open","failures":5,"opened_at":1760190000000,"probe_at":1760190030000,"last_failure":{"code":"format_error","details":{"kind":"died"},"at":1760189999000}},
             "notify_breaker":{"state":"closed","failures":0}}],
 "activity":{"last_request_at":1760190100000,"requests_total":120,"retries_total":7,"rescued_total":3,"upgraded_total":2,"in_flight":2,"retrying":1,
             "by_code":{"upstream_status":5,"format_error":5},
             "buckets":{"width_secs":120,"requests":[0,3,5],"retries":[0,0,2],"failures":[0,0,7]},
             "last_error":{"code":"format_error","details":{"format":"adaptive","kind":"died"},"message":"format 'adaptive' exited","at":1760189999000}},
 "ops":["ping","shutdown","prepare_swap","requests","request","body","subscribe"],
 "features":["shutdown.if_idle_secs","shutdown.keep_restore","prepare_swap.ttl_secs"]}
```

上面的样例为了易读分了行，线上仍是一行。
- `state` 的词表：`starting`、`serving`、`swap_prepared`、`stopping`；兜底 `unknown`。
- `behavior` 是实例的行为摘要：默认的投递方式与缓存、保活、规则、历史预算等开关。单个请求实际的投递方式，见请求记录的 `delivery`（§6.5）。
- `formats[].breaker.state`：`closed`、`open`、`half_open`（§10.9）；`opened_at`、`probe_at`、`last_failure` 没有时省略。
- `formats[].notify_dropped_total`：只在声明了 notify 的程序上出现（§10.11）。
- `formats[].notify_breaker`：notify 自己的熔断器（§10.9，T-S2-1），形状同 `breaker`，也只在声明了 notify 的程序上出现。
  只声明 notify 的程序没有数据通路的消息，它的 `breaker` 恒为 `closed`。
- `activity.upgraded_total`：从 forward 升级为 retry 的请求数（§10.10）。
- `buckets`：最近 30 个桶，每桶 120 秒，旧的在前；样例只画了 3 个。
- `rescued_total`：经过至少一次重试、最终成功的请求数。「今日救回」是界面侧的统计，W5 决定保留与否。
- 第一版的 `home` 字段删去：只有同一个 home 的客户端能连上这个端点，它没有信息量（S3-5）。

### 6.5 请求记录

**RequestRecord**：`requests` 的元素、`request` 结果里的 `record`，以及 `request` 事件的 `data`。

```json
{"request_id":"19ac3f2e8b5d1a2b-42","request_seq":42,"started_at":1760190000123,"ended_at":null,"method":"POST","path":"/v1/messages?beta=***","model":"claude-sonnet-4-5","stream":true,"user_agent":"claude-cli/2.1.0",
 "delivery":"retry","upgraded":true,
 "outcome":"in_flight","code":null,"client_status":200,"phase":"attempting","next_attempt_at":null,
 "attempts":2,"tries_omitted":0,
 "tries":[{"attempt":1,"started_at":1760190000200,"duration_ms":812,"upstream_status":529,"code":"upstream_status","response_bytes":96,
           "decision":{"by":"format:adaptive","decision":"retry","delay_ms":0},"reports":[]},
          {"attempt":2,"started_at":1760190001013,"duration_ms":null,"upstream_status":null,"code":null,"response_bytes":0,"decision":null,"reports":[]}],
 "keepalive":true,"heartbeats":3,"request_bytes":20480,"response_bytes":0,
 "reports":[{"format":"adaptive","stage":"request","summary":"no recent failure for this request; delivering forward","actions":[{"type":"custom","label":"deliver forward"}]},
            {"format":"convert","stage":"request","summary":"anthropic_messages -> openai_chat","actions":[{"type":"protocol_convert","from":"anthropic_messages","to":"openai_chat"}]},
            {"format":"keys","stage":"request","summary":"key k2","actions":[{"type":"key_select","key_id":"k2"}]}]}
```

- **有界**（S1-2）：
  - `tries` 只保留前 3 次与最近 20 次尝试，中间省略的个数记在 `tries_omitted`；
  - 每次尝试的决定（`decision`）与这次 retry、decide 阶段的报告（`tries[].reports`）随这次尝试一起保存，也就一起截断；
  - 顶层 `reports` 只放 request、response、stream 阶段的报告，条数受链长限制；
  - 单条记录序列化后不超过 64 KiB。超出时依次处理，处理过的记录带 `"truncated":true`（R-S3-4、T-S3-8 ①）：
    1. 从最旧的尝试起删掉 `tries[].reports`；
    2. 把 `path`、`user_agent`、`model` 截短，截短后的长度是 E 级常量；
    3. 仍超出就删掉顶层 `reports`。
    大小不受控的只有客户端给的这三个串和 format 给的报告，三步之后剩下的字段都有上限。
  - 被省略的尝试（计入 `tries_omitted` 的那些）的 `sent` 与 `response` 体随之丢弃，不再占预算。
    用 `body` 取它们，回 `not_found`，`reason` 为 `no_such_part`（R-S3-5）。
- **`delivery`**：请求此刻的投递方式，`forward` 或 `retry`；`upgraded` 表示它中途从 forward 转成了 retry（§10.10）。
  两者合起来就是这个请求实际走的路：forward 到底、一开始就 retry、forward 后升级为 retry。
- **`outcome`**：`in_flight`、`ok`、`upstream_error`、`aproxy_error`、`format_response`、`client_gone`；兜底 `unknown`。
  `upstream_error` 指放弃之后透传了上游的非成功响应；forward 请求的流中途断开，或被动判定为「200 但内容是错误」（§10.10），也记为它。
  不是 `ok` 或 `in_flight` 时，`code` 是终态码。
- **`phase`**：`reading_body`、`transforming`、`attempting`、`backoff`、`replaying`、`forwarding`（forward 请求正把上游响应实时转给客户端）、`done`；兜底 `unknown`。
- **`model`**：请求体是 JSON 对象、且顶层 `model` 是字符串时取它；`stream` 按 `keepalive.trigger` 的同一套判定得出。
- **`decision`**：这次尝试之后的决定，`{"by","decision","delay_ms"?,"reroute"?}`。
  - `decision` 取 `retry` 或 `stop`；尝试成功（或还没结束）时整个对象为 null。
  - `by`：`format:<name>`、`override`、`rule:<序号>`（序号从 1 起）、`default`。
  - `delay_ms` 是实际等待的时长；`reroute` 只在 format 表了态时出现。
- **记录的保留**：最多 1000 条（E），先进先出；在途的不淘汰，其数量受并发数限制。

**RequestDetail**（`request` 的结果）：`record`，加上请求头与每次尝试的明细；体只给大小与状态，内容另用 `body` 取。

```json
{"record":{"request_id":"19ac3f2e8b5d1a2b-42","…":"…"},
 "request":{"headers":[{"name":"x-api-key","value":{"secret":true,"hint":"sk-a…f3a9"}}],"body":{"bytes":20480,"state":"retained"}},
 "tries":[{"attempt":1,
           "sent":{"method":"POST","url":"https://relay.example.com/v1/chat/completions","headers":[{"name":"authorization","value":{"secret":true}}],"body":{"bytes":18200,"state":"retained"}},
           "response":{"upstream_status":529,"headers":[{"name":"content-type","value":"application/json"}],"body":{"bytes":96,"state":"retained"}},
           "error":null},
          {"attempt":2,
           "sent":{"method":"POST","url":"https://relay.example.com/v1/chat/completions","headers":[{"name":"authorization","value":{"secret":true}}],"body":{"bytes":18200,"state":"same_as_previous"}},
           "response":null,"error":null}]}
```

- 体对象是 `{"bytes","state","cause"?,"encoding"?}`。`state` 取 `retained`、`same_as_previous`、`evicted`、`not_retained`，兜底 `unknown`。
  - `same_as_previous` 只用于 `sent`：这次发出的体与上一次相同（第 1 次与客户端原始请求体比），只存一份。
    用 `body` 取这样的体，返回的就是那一份。
  - `not_retained` 时 `cause` 说明原因：`budget_disabled`（预算为 0）、`exceeds_budget`（这一个体就比整个预算大）或 `decode_failed`。
  - `encoding` 是线上原来的 content-encoding（例如 `br`），没有编码时省略。
- 没有上游响应的那次尝试，`response` 为 null，`error` 给出 `{code, details, message}`；进行中、还没拿到响应头的尝试，两者都为 null。

**体的保留**（D-13，用户原话「全部暂留在储存中，但是需要上限，上限有默认值，并且可配置」）：
- **保留哪些**：全部请求的客户端请求体、每次尝试发出的体，以及每次尝试的上游响应体。
  forward 请求也保留：边转发边复制，不拖慢转发。
- **只在内存**，重启即失。
- **整份保留**：不截断，只受一个上限约束：所有体加起来不超过 `history.body_budget_mb`（默认 64 MiB，0 表示不保留体）。
  - 进行中的复制也计入。超出时从最旧的请求开始淘汰体，记录本身留下，体的状态变为 `evicted`。
  - 单个体比整个预算还大时不保留（`not_retained`，`cause` 为 `exceeds_budget`）；复制途中才超出的，丢掉已复制的部分。
- **存解码后的字节**（R-S2-2）：带 content-encoding 的体先解码再入库，体对象的 `encoding` 记下原来的编码。
  - 上游回的常是压缩体：aproxy 原样转发客户端的 `accept-encoding`，reqwest 又关着自动解压（记忆 2026-09-14）。存原始字节的话，
    已知密钥替换对它无效，界面也只能显示二进制。
  - 解码后的大小计入预算；解码到超出整个预算就停，记为 `not_retained`（`exceeds_budget`），以防压缩炸弹。
  - 解码失败也不保留：`not_retained`，`cause` 为 `decode_failed`，`encoding` 照记。
  - 解码沿用现行检查路径的做法（decode.rs），只作用于副本；转发给客户端的字节始终是上游原样。
- **入库时**按流做已知密钥替换，留一段重叠的尾巴（§1.5）。
- **口径**：体对象的 `bytes` 与 `body` 的 `total_bytes` 是入库后的字节数，即解码、替换之后的。记录里的字节数都按线上的字节算（T-S3-5 ③）：
  `request_bytes` 是客户端发来的请求体；`tries[].response_bytes` 是这次尝试从上游收到的响应体；顶层的 `response_bytes` 是写给客户端的响应体，
  不含心跳，也不含 aproxy 自产的错误体与终态事件。
- **分块读取**：`body` 一次返回一块，所以体再大，应答行也不变大。

```json
{"total_bytes":5242880,"offset":0,"data_b64":"eyJtb2RlbCI6ImNsYXVkZS1zb25uZXQtNC01Ii…","more":true,"complete":true}
```

- `total_bytes` 是这个体目前入库的字节数；`offset` 是这一块的起点；`data_b64` 是这一块的字节，最多 `limit` 字节（1 字节到 1 MiB，E）。
- `more` 表示这一块之后还有字节；`complete` 为 false 表示体还在接收（进行中的请求），`total_bytes` 会变大。
  替换密钥要留一段尾巴，所以进行中的体的 `total_bytes` 比已收到的略少。
- `offset` 等于 `total_bytes` 时回空块，`more` 为 false，`complete` 照实；轮询进行中的体就是这种情形（R-S3-6）。
- 一律用 base64：块边界可能切开多字节字符，base64 让每一块都能单独解码；1 MiB 的块编码后约 1.4 MiB。
- 读的过程中体被淘汰，或复制途中变成 `not_retained`，下一块回 `not_found`，带 `reason` 与 `cause`。

### 6.6 只读枚举

这是 aproxy-ipc 的库 API，status `--json` 与桌面共用。`enumerate(home)` 读 run/ 与 ledger/，并发 ping（单次 1 秒，重试 1 次），返回：

```json
{"instances":[
  {"port":12345,"liveness":"live","instance":{"pid":4321,"…":"…"},"status":{"state":"serving","…":"…"},"restore_pending":false},
  {"port":12346,"liveness":"unresponsive","instance":{"pid":4400,"…":"…"},"restore_pending":false},
  {"port":12347,"liveness":"incompatible","instance":{"pid":4500,"…":"…"},"reason":{"code":"unsupported_version","details":{"supported":[2]}}},
  {"port":12348,"liveness":"down","instance":null,"restore_pending":true,"restore_reason":"config_missing"}],
 "unreadable":[{"file":"run/12349.instance.json","reason":{"code":"record_schema_too_new","details":{"schema":2}}}],
 "gc_paused":{"reason":"unreadable_records"},
 "watchdog":{"pid":5555,"start":"w:133712345000000000","version":"0.2.0","state":"running","last_scan_at":1760190123456,"restarts":[]},
 "processes":[{"pid":7777,"start":"w:133712349999999999","role":"desktop","version":"0.2.0"}]}
```

- **`liveness`**：`live`、`unresponsive`、`incompatible`、`down`；兜底 `unknown`。只对 `unresponsive` 建议 `--force`（CP-07）。
- **`restore_reason`**：只在 `restore_pending` 为 true、且看得出原因时出现；0.2.0 只有 `config_missing`（spec 里的配置文件不在）。
- **`unreadable`**：码为 `record_unreadable` 或 `record_schema_too_new`。
- **`gc_paused`**：日志清理暂停时才出现。
- **`watchdog`**：没有看护者时为 null；`state` 为 `running`，或 `stale`（最近一次扫描早于 3 个周期）；兜底 `unknown`。
- **`processes`**：台账里有、但不属于任何实例或看护者的进程，包括 W2 之前的版本启动的实例（§4.3 第 7 条）。
- **不删任何文件**。
- **界面怎样发现实例增减**：窗口可见时最多每 2 秒枚举一次；可以用文件系统通知优化（E）。0.2.0 没有 home 级的端点（审查 §7-8）。
- **枚举放在 aproxy-ipc**，所以 C3 的代码整体从主 crate 的 `src/ledger.rs` 迁进 aproxy-ipc 的一个文件（S2-16 b、T-S2-3）：
  条目结构与写者，读者与它的义务（`terminate`、`remove_if_unchanged`），启动标记推导。写者一起迁，读写两边才共用同一份启动标记推导；
  退出前删掉自己的条目、再 `process::exit` 的便利函数不在冻结范围内，按 P5 留在二进制一侧。
  - 算法不变；强制安装改为引用新位置。不在两处各写一份冻结算法。
  - 契约测试冻结的是断言，`use` 路径可以随代码移动：`tests/force_contract.rs` 只改 `use` 行，断言逐字不变（R-S3-15）。
  - AGENTS.md 与 CLAUDE.md（两份是独立副本）讲永久契约的那段逐字一致：列出 C3 冻结代码迁移后的全部文件，
    末句补上「契约测试冻结的是断言，`use` 路径可以随代码移动」。
  - P4 与 B0 都要改这个测试的 `use` 行：谁先落地，谁就同时改测试的文件头那句与 AGENTS.md、CLAUDE.md 的那半句；
    文件清单随 B0 改（T-S3-8 ③）。

### 6.7 事件

| 事件 | `data` | 何时发 |
|---|---|---|
| `request` | 整条 RequestRecord，按 request_id 整条替换 | 开始与结束立即发；中间的变化按 request_id 合并，每条每秒最多一次（S1-2） |
| `state` | 整份快照 | 状态或熔断状态变化时立即发；活动计数的变化合并，最多每秒一次 |
| `heartbeat` | `{}` | 15 秒内没有别的事件时 |
| `lagged` | `{"dropped":n}` | 订阅者跟不上，事件被丢 |
| `closing` | `{"reason"}`，取 `shutdown` 或 `swap` | 实例退出前 |

热路径用 `broadcast`，没有订阅者时连事件对象都不构造。

**为什么**：
- 未知参数报错，免得带条件的 shutdown 在旧实例上变成无条件停止（CP-03）。
- 事件只有「整条记录」与「整份快照」两类：客户端只做替换，丢一条增量也不会错乱。限频与记录有界一起，挡住长重试下的 O(n²) 重发。
- 体拆成单独的 op，并分块读取：应答行有确定的上限，体再大也不用加大它；也正合「体只在点开某条日志时按需取」。
- 体整份保留、只有总量一个上限：用户要的是「全部」，上限只要一个且可配置（D-13）。固定的单体上限既不能配，又会丢掉大体的中段。
- 删掉 `logs`、`config` 两个 op 与 `subscribe` 的参数：它们都有更小的替代，以后用 `ops`、`features` 加性补回（审查 §4）。

---

## 7 config.toml（K3、K13、K14、D-1、D-14、D-15）

**现状**：
- 25 个平铺的配置键，外加三个 transform 段（config.rs:191-385、130-187）。
- `read_timeout_secs` 同时卡住首字节等待（proxy.rs:342）。
- `bounded_retry_paths` 是唯一的限次出口（retry.rs:53）。
- 未知键被静默忽略；写回时整份重写，也不是原子写（config.rs:941-947）。

### 7.1 键表

| 键 | 类型 | 默认 | 可进 settings `defaults`（D-16） | start 旗标 | format 可覆盖 |
|---|---|---|---|---|---|
| `base_url` | URL，必填 | — | ✓ | `--base-url` | — |
| `listen_addr` | 地址 | `127.0.0.1:12345` | ✗（实例身份） | `--listen-addr` | — |
| `api_key` | 密钥 | — | ✓，只跟默认 `base_url`（§7.5） | `--api-key` | — |
| `extra_headers`、`override_headers` | 表 | {} | ✓，按头名合并；密钥头只跟默认 `base_url`（§7.5） | — | — |
| `proxy` | URL | 不设时用环境变量 | ✓ | `--proxy` | — |
| `proxy_username`、`proxy_password` | 字符串、密钥 | — | ✓，只跟默认 `proxy`（§7.5） | — | — |
| `max_request_mb` | 整数 | 128 | ✓ | — | — |
| `max_response_mb` | 整数 | 256 | ✓ | — | — |
| `disk_cache` | 布尔 | true | ✓ | — | — |
| `delivery` | `forward` 或 `retry` | `retry` | ✓ | — | — |
| `allowed_hosts`、`allowed_origins` | 字符串数组 | [] | ✓ | — | — |
| `log_file` | 路径 | 随机名 | ✗（用户定） | `--log-file` | — |
| `timeouts.connect_secs` | 整数 | 30 | ✓ | — | ✓ |
| `timeouts.first_byte_secs` | 整数 | 300 | ✓ | — | ✓ |
| `timeouts.read_idle_secs` | 整数 | 300 | ✓ | — | ✓ |
| `keepalive.interval_secs` | 整数 | 15 | ✓ | — | ✓ |
| `keepalive.trigger` | `accept`、`body_stream` 或 `any`，语义沿用现行的 `keepalive_trigger` | `any` | ✓ | — | ✓ |
| `keepalive.heartbeat` | 字符串 | `": keepalive\n\n"` | ✓ | — | ✓ |
| `retry.max_backoff_secs` | 整数 | 320 | ✓ | — | ✓ |
| `retry.rules` | 表数组 | [] | ✓ | — | — |
| `history.body_budget_mb` | 整数 | 64 | ✓ | — | — |
| `format` | 表数组 | [] | ✗（用户定） | — | — |

- `delivery` 是请求的默认投递方式，取代 0.1.x 的 `forward_only`（D-14，不留旧名；命名理由见 §1.3）。
  format 不经 overrides 改它，而是在 request 回信里给 `delivery`（§10.10）。它与 `[[format]]` 不再互斥（现行的互斥在 config.rs:586-592）。
- `api_key` 同时覆盖 `Authorization: Bearer` 与 `x-api-key`（proxy.rs:1198-1209）。
- 头的覆盖（`api_key`、`extra_headers`、`override_headers`）在请求链之前应用，format 看得到也改得了（现行顺序，S3-20）。
- **只能按请求覆盖的键**（S3-21），format 可以给，配置文件里不能写：
  - `retry.max_attempts`；
  - `keepalive.trigger = "always"`：配置里写它会破坏非流式客户端，只给知道协议的 format 用（审查 §4-11）。

### 7.2 样例

```toml
base_url = "https://relay.example.com"
# delivery 用默认的 "retry"：adaptive 对没失败过的请求回 forward；它不可用时一切回到 retry（R-S1-1）

[timeouts]
first_byte_secs = 600          # 长时间的非流式生成

[[retry.rules]]
path = '/v1/messages/count_tokens(\?.*)?'
max_attempts = 3

[[format]]
name = "adaptive"
command = "aproxy-format"
args = ["adaptive", "--config", "adaptive.toml"]
process = "persistent"
stages = ["request", "decide", "notify"]
required = false               # 判断类单元：坏了就熔断跳过，请求按默认的 retry 投递
pool_max = 1                   # 跨请求的判断状态在一个进程里（§10.3）
worker_idle_secs = 0           # 不因闲置回收，判断状态不会白白丢掉（§10.3）

[[format]]
name = "convert"
command = "aproxy-format"
args = ["convert", "--from", "anthropic_messages", "--to", "openai_chat"]
process = "persistent"
stages = ["request", "stream"]

[[format]]
name = "keys"
command = "aproxy-format"
args = ["keys", "--config", "keys.toml"]
process = "persistent"
stages = ["request", "retry"]
```

### 7.3 重试（D-1、D-15）

**判定的优先级**，每次尝试失败后从高到低：
1. format 的 decide（§10.5）；
2. 按请求覆盖的 `retry.max_attempts`；
3. 规则表；
4. 默认：无限重试。

这是 retry 请求的判定。forward 请求不进重试循环：它在一个字节都没写出时失败，只问一次 decide 要不要升级（§10.10）；升级之后才按上面的优先级判定。

**规则表** `[[retry.rules]]`：
- 按书写顺序匹配，第一条全部匹配器都成立的规则生效；规则从 1 起编号。
- 匹配器：
  - `path`：对客户端请求的「路径?查询串」整体做正则匹配，自动锚定，查询串不可省略（沿用 bounded_retry_paths）。
  - `status`：上游状态码的数组。
  - `body`：R 级，0.2.x 实现；0.2.0 遇到就报 `config_not_supported`。
  - 规则里出现其他任何键，都按 D-14 报 `config_unknown_key`，所以以后新增的匹配器在旧版本上是拒绝启动，而不是放宽。
- 规则只匹配有上游响应的失败（`upstream_status`、`upstream_error_body`）；网络类失败不封顶，format 自身的连续故障由熔断处理（§10.9）。
- **`max_attempts`**（必填）：
  - 计数口径是这个请求的尝试序号，与 bounded_retry_paths 一致：第 N 次尝试失败、命中这条规则，且 N ≥ `max_attempts`，就停（S3-22）。
  - 停时透传最后一次上游响应，码为 `retry_exhausted`。
  - 0 表示不限，可以写在宽规则前面当作豁免。

**按请求覆盖的 `retry.max_attempts`**：计入一切类型的失败，用尽时码为 `retry_exhausted`，`by` 为 `override`。

**退避**（D-15）：沿用现行（retry.rs:14-32）：
- 前 3 次立即重试，之后从 5 秒开始翻倍，封顶 `retry.max_backoff_secs`；
- 不读上游的 Retry-After，不加抖动。

等多久的动态决定只来自 format：decide 回信里的 `delay_ms`，aproxy 照办。format 想尊重 Retry-After，就自己从 decide 收到的响应头里读。

### 7.4 `[[format]]`

| 键 | 类型 | 默认 | 说明 |
|---|---|---|---|
| `name` | 字符串 | `command` 的文件名主干 | 在列表内唯一 |
| `command` | 路径或命令名 | 必填 | 相对路径相对 home；裸命令名先找 `<home>/bin/`，再找 PATH |
| `args` | 字符串数组 | [] | 打码规则见 §1.5 |
| `env` | 表 | {} | 追加给程序的环境变量；值只写不读，密钥建议放这里 |
| `process` | `spawn` 或 `persistent` | `spawn` | 进程方式（命名见 §1.3） |
| `stages` | 字符串数组 | 必填 | `request`、`retry`、`decide`、`response`、`stream`、`heartbeat`、`notify`；`admit` 为 R 级 |
| `required` | 布尔 | true | 只管程序出错与熔断时怎么办，见 §10 |
| `pool_max` | 整数 | 4 | persistent 的进程池上限；进行中的流不计入（§10.6）；需要跨请求共享内存状态的程序配 1（§10.3） |
| `worker_idle_secs` | 整数 | 300 | |
| `timeout_secs` | 整数 | 30 | 每条消息的回信时限；notify 不用它，用固定的 2 秒（§10.11） |
| `decide_timeout_secs` | 整数 | 2 | |
| `breaker_failures` | 整数 | 5 | 连续进程级失败达到这个数就熔断；0 表示不熔断（D-12） |
| `breaker_cooldown_secs` | 整数 | 30 | 熔断之后，过多久放一条消息试探；不许为 0 |

违反下面任一条，都报 `config_conflict`：
- 同一个程序不能同时声明 `response` 与 `stream`。
- 声明 `stream` 或 `notify` 的程序必须是 `persistent`。
- 请求方向上，声明了 `retry` 的程序之后（离上游更近的一侧），只能是同样声明了 `retry` 的程序（§10.3）。
  例如 route 之后的 convert、rewrite 也要声明 `retry`，每次重试按新的路由重做（§10.14，R-S3-2）。

顺序：列表顺序就是请求方向的顺序，离客户端最近的写在最前面。

forward 与 format 可以共存（§10.10）：链上有程序要请求体或摘要时，先读完请求体再处理；forward 请求的响应边收边转，可以经过 stream 阶段。
只配了请求侧协议转换、没配 `stream` 时，响应仍是上游协议；文档写明这一点，doctor 给出提示（S3-24）。

### 7.5 解析、校验与诊断

- **单一管线** `config::resolve(path, overrides, settings) -> {config, sources, withheld, diagnostics}`。
  start、`__serve`、doctor、`config show --effective`、find 与桌面（经注入的操作层）共用它（综C9）。
- **层级**：start 的覆盖 > config.toml > settings 的 `defaults` > 内置默认。
  - 表按键合并，数组整体替换。
  - `extra_headers`、`override_headers` 按头名合并（D-16）：头名不分大小写，同名以配置为准；两张表各自合并，不交叉。
    没有删除继承来的头的写法，以后需要时加性补上（§12）。
  - `sources` 的取值：`override`、`toml`、`settings`、`env`、`default`。`proxy` 来自环境变量时就是 `env`（S3-23）；头表按每个头名记来源。
- **凭据只发给它所属的目标**（D-16）：
  - settings `defaults` 里的 `api_key`，以及两张默认头表里的密钥头（§1.5），只在这个配置最终的 `base_url` 与 `defaults.base_url`
    指向同一目标时继承；
  - `defaults.proxy_username`、`defaults.proxy_password` 只在最终的 `proxy` 与 `defaults.proxy` 指向同一目标时继承；
  - 「同一目标」：两边按 base_url 的规范化处理之后相等。W2 的 base_url 解析统一用这套规范化：去掉首尾空白与末尾斜杠，
    scheme 与主机名转小写，去掉默认端口。userinfo 不参与比较，它是凭据，不是目标。proxy 比较 scheme、主机与端口。
  - base_url 连路径一起比较（R-S3-9 ⑤）。按路径分渠道的网关在同一主机上，例如 Cloudflare AI Gateway 用 `/anthropic`、`/openai`
    转给不同的厂商，只比主机就会把 A 家的 key 送去 B 家。代价是末尾多一个 `/v1` 也算不同目标、key 会被扣下；
    下面的 `withheld` 与告警让用户看得见、改得了。
  - URL 里内嵌的 userinfo 属于那个 URL 值本身（R-S3-9 ③）：配置自己写了 `proxy` 或 `base_url`，就整体以配置的值为准，
    不与默认值的 userinfo 拼接。独立的 `proxy_username`、`proxy_password` 按上面的同目标规则继承。
  - 这条规则只管继承的那一刻（R-S3-9 ①）。format 改写 url（§10.4，例如 route 换渠道）时，继承来的 key 会随请求去别的主机；
    换上游的程序要同时换凭据（keys 角色正是这样做），协议文档写明这一点。
  - 密钥头按名字识别（§1.5，R-S3-9 ②）：名字少见的凭据头（例如 `x-access-code`）认不出来，会继承到所有目标。
    doctor 对 `defaults` 头表里不属于密钥头的头给出 `default_header_shared` 提示。
  - ①②⑤ 这三处余量都写进文档（B12）：① 进 format 协议文档，②⑤ 进 settings 与配置的文档。
  - 没继承的凭据列进 `withheld`，URL 经 §1.5 打码；`reason` 取 `base_url_differs` 或 `proxy_differs`：
    `[{"key":"api_key","from":"settings","reason":"base_url_differs","default_target":"https://api.a.example","target":"https://relay.b.example"}]`。
    `config show --effective` 显示它，实例启动时写一行 INFO 日志。
  - 这个配置自己也没有这一项时（最终没有 `api_key`、没有同名的头、没有代理口令），另报 `default_credential_withheld`（warning）。
    否则用户设了默认 key、换了上游之后只会看到 401，不知道为什么。
- **严格**（D-14）：任何不认识的键都报 `config_unknown_key`（error），带文件、键路径与行列。不识别 0.1.x 的旧键，也不提示新写法。
  `defaults` 里有 `api_key` 或密钥头却没有 `base_url` 时，报 `config_missing_key`（键为 `defaults.base_url`）；有代理口令却没有
  `proxy` 时同理。否则这把 key 没有归属，会被继承到每一个配置。
  写 settings 时这条也在写前校验里：先设 `defaults.api_key` 会被拒绝，报错直接说「先设 defaults.base_url」；
  文档与 skill 写明这个顺序（R-S3-10）。
- **补全校验**：头名与头值、`api_key` 的字符、`listen_addr`、规则的正则、状态码 100–599、format 的各个键。

### 7.6 写回（K7）

- **只改目标键**：用 toml_edit，注释、键序与其余内容原样保留。同一个键可能写成表、点号键或内联表，三种都要处理。
  整体替换数组表时，原条目里的注释会丢，可以接受。
- **串行化**（S2-3）：本 home 的 aproxy 写者在 `run/config-write.lock` 上持锁，锁原语与 C4 相同。锁内依次：读、改、写临时文件
  （`*.tmp-<pid>-<纳秒>`，create_new）、`sync_all`、rename。Windows 上目标被短暂占用时，退避重试。
- **外部修改检测**：读入时记下内容哈希，rename 之前再比一次。不一致说明有编辑器之类的外部写者，重新读入、重新应用这次改动，最多 3 次，
  仍冲突就报 `write_conflict`。
- **写入前先校验结果**：这次改动引入了新的 error 级诊断，就拒绝写入（退出码 3）。文件里原有的、与这次改动无关的错误，不挡住逐项修复。
- **权限与链接**：unix 上 0600；目标是符号链接时，写到它指向的真实文件。
- **`unset` 接受任何键路径**，包括不认识的键（S2-15）：删除总是安全的，删一个不存在的键也退出 0。
  doctor 为每个不认识的键给出删除它的命令。

**为什么**：
- 只在语义成组的地方建表，核心键保持平铺。否决把核心键也收进表：改动面大，收益小。
- `max_request_mb`、`max_response_mb` 与终态码 `request_too_large`、`response_too_large` 同名同义。`spool_limit_mb` 的 0 语义本来就要改（K14）。
- 首字节超时与读空闲超时分开，默认值不变：保活路径上，首字节超时是挂死上游的唯一出口（3.12）。
- 重试时序不信上游（D-15），由此不改变现行的默认时序（S2-14 不再成立）。
- 用锁串行化，而不是只比哈希：比对与 rename 之间没有原子性，只比哈希防不住 GUI 与 CLI 同时写。

---

## 8 settings.json（D-5、D-14、D-16）

**现状**：
- 20 个平铺字段（settings.rs:164），混着程序状态、全局策略与 7 个 config 字段的全局默认。
- 任一字段解析失败，运行期整份回退默认（settings.rs:361-367）；写入路径对解析失败拒绝写回（settings.rs:378-392）。
- 没有 schema。

### 8.1 形状

```json
{"schema":1,
 "language":"system",
 "default_config":null,
 "aliases":{"work":"C:\\Users\\u\\.aproxy\\configs\\work.toml"},
 "config_dirs":[],
 "defaults":{"base_url":"https://api.example.com","api_key":"sk-…","extra_headers":{"x-team":"core"},"retry":{"max_backoff_secs":120}},
 "watchdog":{"enabled":true,"heartbeat_secs":30,"stale_after_cycles":1,"max_restarts":5,"idle_exit_secs":300},
 "logs":{"rotate_mb":8},
 "instances":{"idle_threshold_secs":1800},
 "update":{"download_chain":null,"download_proxy":null,"skill_auto_update":true},
 "ui":{}}
```

| 键 | 说明 |
|---|---|
| `schema` | 缺省按 1 读；写者总是写上 1（S3-25） |
| `language` | `system`、`en` 或 `zh-CN` |
| `default_config`、`aliases`、`config_dirs` | 语义沿用现行 |
| `defaults` | config.toml 键的全局默认，收 §7.1 里标 ✓ 的键（D-16）：行为与限额类，加上 `base_url`、`api_key`、`proxy` 与代理口令、两张头表；不收 `listen_addr`、`format`、`log_file`。头表合并与凭据继承见 §7.5 |
| `watchdog.*`、`logs.rotate_mb`、`instances.idle_threshold_secs`、`update.*` | 由现行的平铺字段分节而来；0 的含义按 §1.2 统一，其余语义不变 |
| `ui` | 桌面界面私有的不透明对象 |

**`ui` 段**：
- 核心只检查它是对象，从不读取、也不校验它的内部；它的键由桌面自己定义与校验（W5）。
- 扩展授权、置顶条目、开机自启这类桌面状态，放 `<home>/desktop/` 下，不进 settings.json。

**密钥**（D-16）：`defaults` 可以含 `api_key`、代理口令、URL 里的 userinfo 与密钥头，所以 settings.json 与 config.toml 一样含密钥。
- 打码、只写不读、IPC 与桌面桥接层永不返回明文，都按 §1.5 的同一套规则：`settings show/get` 给占位对象，占位写回等于不改。
- 文件权限按 §1.6：unix 0600；Windows 上由 aproxy 写入时另设显式 DACL。
- 写密钥用 `settings set defaults.api_key -`，从 stdin 读一行（§9.3），不让密钥进命令行历史。

### 8.2 读

- **严格**（D-14）：不认识的键、类型或取值不对，都是 error，带文件与键路径；不识别 0.1.x 的旧字段。
- **schema 更高**时报 `settings_schema_too_new`，说明这份文件是更新的 aproxy 写的，`settings` 子命令也拒绝写它。
  出路有两个：升级；或者用强制安装装回需要的版本，强制安装从不读 settings（C8）。
- **settings.json 无效时**，需要它的命令以退出码 3 报错。例外如下：
  - `--version`、`--help`、强制安装：根本不读它。
  - `settings` 的各个子命令与 `doctor`：用来查看与修复它。JSON 语法错误时只能手工修。
  - `stop`、`status`、`install`：任何时候都要能停实例、看状态、装上能修好问题的版本（R-S3-17）。它们用内置默认运行，并把错误作为告警打印出来：
    stop 与 status 按端口或 `all` 寻址（别名不可用）；install 用内置的下载链与代理。
    install 要滚动重启实例时，目标版本照样按 settings 预检，过不了就以 `install_precheck_failed` 结束、一个实例都不停（§5，T-S2-2）。
  - 其余命令（例如 start）的报错说清怎样修：指出文件与键，给出 `aproxy settings unset <键>` 或删除这个文件的做法。
- **0.1.x 留下的 settings.json** 是平铺字段、没有 schema，按 schema 1 读必然报错。迁移文档（W7）写明要删除或重写它；
  install 照常可用，所以升级本身不会被它挡住。
- **运行中的看护者**沿用上一次读到的有效值，并在日志里报错，限频。实例只在启动时读 settings，读不了就启动失败（startup.log 里说明原因）。
- **语言判定**在 clap 之前只读 `language` 一个字段，读不了就按系统语言，不因此失败（§9.2）。

### 8.3 写

- 在 `serde_json::Value` 上按路径打补丁（set 或 unset），开启 `preserve_order`。
- 串行化、外部修改检测、写前校验与 `unset` 的宽容，都同 §7.6。
- 新建的文件只写显式给出的键与 `schema`。

**为什么**：
- settings.json 是所有版本共写的文件，以后再重整就得写双读代码（D-5）。
- 严格解析会让「新版本写过、降级再读」报错，这是用户接受的代价（D-14）。所以报错必须说清是哪个文件、哪个键，以及出路。
- `ui` 定为不透明对象：界面加一个偏好，不应让同一台机器上的旧版 CLI 起不来。严格解析要防的是「条件被读宽」，而核心根本不依据界面偏好行事，
  所以这一段不需要核心去校验。
- 否决把 `ui` 的具体键写进本稿：原型还在按风格反馈返工，现在冻结它们，是为一个没定的设计付永久成本（审查 §4-4）。
- `defaults` 的范围按 D-16：默认值本来就是为了方便，上游、key、代理都常常相同，可以设默认；`listen_addr` 是实例身份，两个实例不可能相同。
  凭据只跟它的目标走，防的是审查 S2-7 的风险：指向别家上游的配置悄悄继承全局 key，把 A 家的 key 发给 B 家。

---

## 9 命令行（D-3、D-4、D-7、K8、K11）

**现状**：
- 启动覆盖参数挂在顶层，`--config` 与 `--log-file` 是全局参数（cli.rs:16、40）。
- `Upgrade` 是重复的变体（cli.rs:179）；`config` 有 15 个专用旗标；裸 `aproxy` 等于启动（main.rs:183）。
- 退出码多为 0 或 1；CLI 的 tracing 写到 stdout（server.rs:423）；没有 `--json`。

### 9.1 命令树

```text
aproxy                          按 clap 惯例：帮助写到 stderr，退出码 2（D-7）
aproxy start   [<ALIAS|PATH>] [--base-url <URL>] [--listen-addr <ADDR>] [--proxy <URL>]
               [--api-key <KEY|->] [--log-file <PATH>] [--foreground] [--json]
aproxy stop    [<PORT|ALIAS|all|idle>] [--idle-threshold-secs <N>] [--force] [--json]
aproxy restart [<PORT|ALIAS|all|idle>] [--idle-threshold-secs <N>] [--force] [--json]
aproxy status  [--idle|--busy] [--json]
aproxy logs    [<PORT|ALIAS>] [-n|--lines <N>] [-f|--follow]
aproxy restore [--json]
aproxy alias   add <NAME> [<PATH>] | remove <NAME> | list [--json]
aproxy config  show [--effective] [--json] | get <KEY> [--json] | set <KEY> <VALUE|-> | unset <KEY>   [--config <ALIAS|PATH>]
aproxy settings show [--json] | get <KEY> [--json] | set <KEY> <VALUE|-> | unset <KEY>
aproxy doctor  [--json]
aproxy find    [<QUERY>] [--aliased|--unaliased] [--port <PORT>] [--json]
aproxy install [<VERSION>|latest] [--pre] [--from <FILE>] [--adopt] [--abort] [--cpu …]
               [--allow-downgrade] [--download-proxy <URL>] [--no-skills] [--check] [--json]
aproxy install --force …        C7，语法不动
aproxy upgrade …                install 的 visible_alias
aproxy skill   install|upgrade|list|show …      W4
aproxy desktop                  W5
全局：-h/--help、-V/--version
隐藏：__serve、__watchdog；install 的 --continue、--handover-from（§5）
```

- **用户可见的 `--home` 不做**（审查 §4-8）：APROXY_HOME 已经够用；以后需要时加旗标，属于加性变更。
- **`install` 的变化**：删 `--skills-only`（W4）；加 `--check` 与 `--json`；隐藏的 `--continue`、`--handover-from` 不变。
- **与 `--force` 冲突的旗标**（S2-11）：`--check`、`--json` 与 `--force` 冲突，由参数解析拒绝（退出码 2），并加进 force_contract 的命令行样本。
  以后要支持时按 C7 的「放宽」处理。
- **其余互斥**：`start --json` 与 `--foreground` 互斥（S3-28）。
- **`--json` 不做全局旗标**：免得它在不支持的命令上被静默接受（CI-17）。

### 9.2 启动顺序（main）

1. argv 恰好是单独一个 `--version` 或 `-V`：打印 C5 那一行，退出 0（main.rs:27）。不读 home，不判定语言（G2）。
2. 第一个参数以 `__` 开头：分派到内部入口。内部入口在建 tokio 运行时之前，把自己的 `--home` 设进 APROXY_HOME。
3. 第一个不以 `-` 开头的参数是 `install` 或 `upgrade`，且 argv 里有 `--force`：走强制安装（S3-27）。它的语言只看 APROXY_LANG 与系统（C8）。
4. 判定语言：APROXY_LANG > settings 的 `language`（只读这一个字段，读不了就跳过）> 系统界面语言 > en。
5. clap 解析，帮助按语言注入；`desktop` 在建运行时之前分派（W5）。
6. 按 C1 校验 home，失败报 `home_invalid`、退出码 2；然后分派。
   只有变更类命令、并且 install 状态可以接管时，才拉起续作 `install --continue`（A5）。
   install 状态的 `halted` 为 true 时不拉起：失败的原因不会自己消失，要用户排除之后重新执行 install（§4.2，T-S3-2）。

### 9.3 config 与 settings 命令（D-4）

- **键路径**：TOML 的点号路径，例如 `retry.max_backoff_secs`、`extra_headers."x-foo"`、`defaults.timeouts.connect_secs`。
  数组表（`retry.rules`、`format`）只能整体读写；按下标改元素以后再加。
- **值的语法**（S3-26）：
  - config 只收 TOML 值，settings 只收 JSON 值；
  - 字符串类型的键可以直接写原文，以 `"` 开头时按带转义的字符串字面量解析（用来写换行，例如 `keepalive.heartbeat`）。
- **`-` 表示从 stdin 读一行**，用来传密钥；直接写在命令行上照样接受。
- **`set` 只能设置已知的键**，未知键报 `config_unknown_key`（退出码 3）。**`unset` 接受任何键**（§7.6）。
- **读**：
  - 密钥的 `get` 与 `show` 输出占位对象；
  - `settings get` 在没有 settings.json 时返回默认值（S3-28）；
  - `config show` 在文件有 error 时照样显示，附上诊断，退出码 3。
- **`set` 与 `unset` 的 `--json`**：`{"schema":1,"file","key","value","instances_to_restart":[12345]}`。
- **专用旗标一个不留，也不做别名**。

### 9.4 `--json`

**共同规则**：
- stdout 只有这一个 JSON 文档，带 `"schema":1`；install 例外，输出 NDJSON。
- 给人看的提示写到 stderr。
- 整条命令失败时，文档是 `{"schema":1,"error":{"code","message","details"}}`。
- clap 的用法错误不受 `--json` 影响。

| 命令 | 成功时的文档 |
|---|---|
| `status` | §6.6 的枚举结果 |
| `start` | `{"outcome","port","pid","log_path","instance"}`；outcome 为 `started` 或 `already_running` |
| `stop`、`restart`、`restore` | `{"results":[{"target","port","outcome","error"?,"reason"?}],"summary":{"ok","failed"}}` |
| `alias list` | `{"aliases":[{"name","path","exists"}]}` |
| `config show`、`settings show` | `{"file","values","sources"?,"withheld"?,"diagnostics"}`；`sources` 与 `withheld` 只在 `config show --effective` 时出现（§7.5） |
| `config get`、`settings get` | `{"key","value","source"?}` |
| `doctor` | `{"files":[{"file","kind","diagnostics"}],"gc_paused"?,"summary":{"errors","warnings"}}` |
| `find` | `{"configs":[{"path","aliases","listen_addr","base_url","errors"}]}` |
| `install --check` | `{"current","latest","update_available","channel","variant","published_at"?,"url"?}` |

`stop`、`restart`、`restore` 的 `outcome` 取值：
- stop：`stopped`、`already_stopped`、`killed`、`skipped`、`failed`；
- restart：`restarted`、`skipped`、`failed`；
- restore：`restored`、`already_running`、`config_missing`、`failed`。

`install` 的 NDJSON，每行都带 `"schema":1`：
- 开头一行：`{"event":"plan","ports":[…]}`，列出将重启的实例；
- 进度行：`{"event":"phase","phase","at"}`，相位就是 install.state 的相位；`{"event":"progress","phase":"downloading","bytes","total"}`；
  `{"event":"instance","port","outcome"}`；
- 末行：`{"event":"result","outcome","version","error"?}`。

桌面的升级就是拉起 `install --json`、读这些行，所以 NDJSON 是 A 级契约。界面怎样把相位归并成自己的几个步骤，由 W5 定。

### 9.5 各命令的退出码

| 命令 | 0 | 1 | 2 | 3 | 4 | 5 |
|---|---|---|---|---|---|---|
| start | 已启动或已在运行 | 端口被占、起不来、就绪超时 | 用法 | 配置无效（含子进程报的） | 别名或文件不存在 | — |
| stop | 已停，或 idle 目标被跳过 | 停不掉 | 多实例时没指定目标 | — | 别名不存在 | 部分失败 |
| restart | 已重启或被跳过 | 失败、前台实例 | 同上 | 预检发现配置无效 | 目标没在运行或不存在 | 部分失败 |
| status | 枚举完成 | — | — | — | — | — |
| logs | 输出完毕 | 前台实例没有日志文件 | 多实例时没指定目标 | — | 没在运行，也没有记录可读 | — |
| restore | 全部恢复，或无事可做 | 全部失败 | — | — | — | 部分失败，或有 `config_missing` |
| config、settings | 完成 | 写冲突 | 用法 | 结果无效、未知键、文件无效 | `config get` 时文件不存在 | — |
| doctor | 没有 error | — | — | 有 error | — | — |
| find | 完成 | — | — | — | — | — |
| install | 已安装或已是最新 | 失败（含回滚、管不了的实例） | 用法 | 滚动重启之前的预检不通过（`install_precheck_failed`，§5）；settings 无效本身只告警 | 版本不存在 | — |
| skill | 完成 | 下载或读写失败 | 用法 | — | 没装，或文件不在清单里 | — |
| desktop | 正常退出 | headless 版本、起不来 | — | — | — | — |

### 9.6 输出纪律

- stdout 只放主输出；CLI 的 tracing 写到 stderr，交互命令默认 WARN 级。
- `logs` 默认打印最近 200 行就退出，`-f` 才跟随，这样 agent 调用它不会挂住。
  实例没在运行、但留有期望状态记录时，读记录里的 `log_path`，那里就是崩溃现场。
- 所有 `--help` 末尾都有一行固定英文的 skill 提示（W4）。

**为什么**：
- 退出码一公布就冻结；i18n 之后，agent 只能依靠退出码与 `--json`（D-3）。
- 旗标名由键名机械推出，帮助、文档与界面映射都不必为谁破例（D-4）。
- 裸 `aproxy` 用 clap 的 `arg_required_else_help`，退出码是 clap 的惯例 2（S3-34 ④）。

---

## 10 format 协议 1（K10、D-6、D-12、format 定稿）

**现状**：
- 信封是 stdin/stdout 上的一行 JSON（aproxy-envelope/src/lib.rs:29-137），没有协议版本，也没有往返序号。
- aproxy-format 按有无 `method` 区分请求与响应（main.rs:189-199，DP-16）。
- 回信的 `headers` 必填，缺 body 就等于空体（DP-13）。
- `worker_id` 实为取模自增（transform.rs:445）；程序的 stderr 被丢弃（transform.rs:426）。
- 仅转发模式与转换互斥；SSE 原样透传（translate.rs:7-9）。

### 10.1 传输

- **帧**：一行一个 JSON 对象，UTF-8 编码，以 `\n` 结尾。
- **stderr**：每一行先做已知密钥替换，再写进实例日志，前缀为 `format[<name>]`。
- **一问一答**：
  - 每条消息恰好一个回信，按顺序，回信必须原样带回消息的 `seq`。
  - `seq` 不符或缺失按协议错误处理（`format_error`，kind 为 `protocol`），剔除这个 worker。这根治了串包。
- **占用与计时**（R-S2-1）：
  - 一条消息从写到 worker 的 stdin 起，到读到它的回信为止，这个 worker 算忙；notify 也一样。
  - 每条消息的时限从写到 worker 的那一刻起算，在 aproxy 里排队的时间不计入。
  - 派发顺序：数据通路的消息（notify 以外的一切）优先；notify 只在有空闲 worker、且没有数据通路的消息排队时才派发（§10.11）。
    所以数据通路的消息最多等一条已经写出的 notify：等它回信，或等它超过固定时限（2 秒，§10.11）之后换上的新 worker。
  - 消息超时的 worker 一律换掉：它可能还在处理那条消息，之后的回信会错位。是否计入熔断见 §10.9。
- **会话**：
  - 每个进程的第一行都是 `hello`（§10.2），spawn 模式也一样（S3-34 ①）。
  - persistent 在握手之后处理消息，直到 stdin EOF；遇到 EOF 必须退出。
  - spawn 在握手之后收一条阶段消息、回一次，然后 aproxy 关闭 stdin。
  - 两种模式都是同一个「读一行、回一行、直到 EOF」的循环。
- **公共字段**：
  - aproxy 发出：`v`（协议版本）、`seq`（每个会话从 0 起递增）、`stage`、`request_id`、`state`（这个程序在这个请求上存下的值，没有就省略）；
    在这个阶段要了摘要的程序另收 `digest`（§10.2）。
  - format 回：`v`、`seq`；可选的 `state`（字符串表示替换，空串表示清除，省略表示不变；上限 64 KiB）；可选的 `report`。
- **HTTP 对象**：
  - 请求对象 `{"method","url","headers":{小写名: 值},"body"|"body_b64"}`；响应对象 `{"status","headers","body"|"body_b64"}`。
  - 回信里的同名对象是补丁：字段省略表示不变；给出 `headers` 就整表替换；`body` 与 `body_b64` 都省略表示不变，要清空就写 `"body":""`。
  - 多值头只保留第一个值。
- **字节流**（stream、heartbeat 阶段）用 `data` 或 `data_b64`。aproxy 把块尾不完整的 UTF-8 序列留到下一块。

### 10.2 hello 与能力协商

```json
→ {"v":1,"seq":0,"stage":"hello","protocols":[1],"aproxy_version":"0.2.0","name":"adaptive","process":"persistent","stages":["request","decide","notify"],"worker_id":0,"features":[],"overrides":["timeouts.connect_secs","timeouts.first_byte_secs","timeouts.read_idle_secs","keepalive.interval_secs","keepalive.trigger","keepalive.heartbeat","retry.max_backoff_secs","retry.max_attempts"]}
← {"v":1,"seq":0,"protocol":1,"stages":["request","decide","notify"],"request_body":{"request":"digest","notify":"digest"},"features":[],"name":"aproxy-format adaptive","version":"0.2.0"}
```

- **hello 这一行的形状永久不变**，其中 `v` 恒为 1。
  - aproxy 在 `protocols` 里列出它会说的协议版本，format 选一个，回在 `protocol`。之后每一行的 `v` 都是选定的版本。
  - aproxy 至少等 10 秒的 hello 回信。
- **`stages`**：aproxy 发出的是配置声明的阶段，format 回的是它支持的全部阶段。
- **`overrides`**：aproxy 接受的按请求覆盖键。format 据此知道哪些覆盖会生效；回信里不在列表中的覆盖，被忽略并告警。
- **`worker_id`**：0 到 pool_max 之间、在世 worker 之间唯一的槽位号，取最小的空闲号；spawn 模式恒为 0。
- **`request_body`**（format 回）：按阶段声明要多少请求体，取 `none`、`digest` 或 `full`（用户追加 2026-10-12）。
  - 只对带请求体的阶段有意义：request、retry、decide、notify；其余阶段的键被忽略。
  - 省略时 request 与 retry 为 `full`，decide 与 notify 为 `none`；notify 只能取 `none` 或 `digest`。
  - `digest` 是 aproxy 对原始请求体算的 SHA-256，64 位小写十六进制，放在消息的 `digest` 字段。原始请求体指客户端发来的字节：
    不解码 content-encoding，不经链上改写，所以同一个请求重发时摘要不变。
  - 要 `digest` 或 `none` 的程序，消息里的请求对象不带 `body` 与 `body_b64`；它的补丁照样可以改头与 url。
  - 取值不认识时，按 hello 回信不合法处理。
  - 这是性能契约：几 MB 的请求体不必经管道交给只做判断的程序。
  - 什么时候要先读完请求体，以 §10.10 为准（R-S3-3）：request 阶段有程序要 `full` 或 `digest`，或者链上有 decide 程序。
    只在 notify 上要 `digest` 时不必缓冲：摘要边转发边算；请求体不完整（客户端中途断开）时，notify 里的 `digest` 为 null。
- **`features`**：双方都列出才生效。协议 1 定义 `decide.success`，由 format 声明（§10.5）；以后的 notify 事件用 `notify.attempt` 声明（R，§10.11）。
- **启动时**，对每个程序并行握手一次。persistent 的这次探测进程留作第一个 worker；spawn 的探测进程握手后关闭 stdin、等它退出。
  失败的处理（S3-34 ②、S3-29）：

| 失败 | required 程序 | 可选程序 |
|---|---|---|
| 配置声明的阶段不在回信里 | 实例不启动，`format_stage_unsupported`，退出码 3 | 同左 |
| 协议版本不符、hello 回信不合法 | 实例不启动，`format_handshake_failed`，退出码 3 | 同左 |
| 起不来、10 秒内没回、中途退出 | 实例不启动，`format_handshake_failed`，退出码 1 | 告警；它的熔断器以 `open` 起步，冷却后试探（§10.9；只声明 notify 的程序是 notify 的熔断器） |

- **跨版本**：新阶段、新能力只在双方都声明时才使用，旧的 format 永远收不到它不懂的消息。

### 10.3 链（洋葱）与 state

- **方向**：
  - 请求方向（request、retry）按列表顺序调用；
  - 响应方向（response、stream、heartbeat）按相反的顺序；
  - decide 按列表顺序。
- 每个阶段只调用声明了它的程序；上一层的输出就是下一层的输入。
- **决定与内容**：决定类字段（request 回信的 `delivery`，decide 回信的 `decision` 与 `reroute`）取链序第一个表态的程序；
  内容类字段（请求、响应对象的补丁）逐层叠加，后一层在前一层的结果上改。
- **`state`** 按「程序 × 请求」隔离；`request_id` 全链共享。
  heartbeat 回信里的 `state` 被忽略：心跳与其他阶段并行，让它写 state 会互相覆盖（S2-6）。
- **worker 与跨请求状态**：同一个请求的各条消息、不同请求的消息，都不保证落在同一个 worker 上（流例外，§10.6）。
  - 跨消息的状态放在 `state` 里。
  - 跨请求的内存状态只在 `pool_max = 1` 时才在同一个进程里，而这个进程仍会被换掉：闲置超过 `worker_idle_secs`、熔断、
    协议错误、消息超时，都会换掉它（R-S2-1）。所以程序要能承受 worker 重启：状态丢了只让判断变差，不能出错。
  - adaptive 这类程序配 `pool_max = 1` 与 `worker_idle_secs = 0`（不因闲置回收，§1.2）；要多进程共享，由程序自己做。
- **重试时的链**（S3-34 ③）：
  - 配置校验保证声明了 `retry` 的程序都在链的后段（§7.4）。
  - 重跑时，从第一个声明了 `retry` 的程序开始，发 `stage:"retry"`；它的输入是前段最后一层缓存的输出。
  - 前段那些没声明 `retry` 的程序，对每个请求只成功处理一次。唯一的例外是它自己这一层失败了，见下一条。
  - 请求链在某一层失败（进程级失败或 `fail{retry:true}`）时，下一次尝试从失败的那一层接着跑，前面各层的输出沿用；
    这一层原本收到的是 request，就再发 request。
  - 是否重跑由 decide 的 `reroute` 决定（§10.5）。没人表态时，只要有程序声明了 `retry` 就重跑，等价于原来的 `every_attempt`。

### 10.4 请求方向：request 与 retry

```json
→ {"v":1,"seq":3,"stage":"request","request_id":"19ac3f2e8b5d1a2b-42","delivery":"retry","digest":"9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
   "request":{"method":"POST","url":"https://relay.example.com/v1/messages?beta=true","headers":{"content-type":"application/json","x-api-key":"sk-…"}},
   "client":{"path":"/v1/messages?beta=true","stream":true}}
← {"v":1,"seq":3,"delivery":"forward","state":"{\"digest\":\"9f86d081…\"}",
   "report":{"summary":"no recent failure for this request; delivering forward","actions":[{"type":"custom","label":"deliver forward"}]}}
→ {"v":1,"seq":7,"stage":"request","request_id":"19ac3f2e8b5d1a2b-42","delivery":"forward",
   "request":{"method":"POST","url":"https://relay.example.com/v1/messages?beta=true","headers":{"content-type":"application/json","x-api-key":"sk-…"},"body":"{\"model\":\"claude-sonnet-4-5\",\"stream\":true}"},
   "client":{"path":"/v1/messages?beta=true","stream":true}}
← {"v":1,"seq":7,
   "request":{"url":"https://relay.example.com/v1/chat/completions","body":"{…}"},
   "overrides":{"timeouts":{"first_byte_secs":900}},
   "report":{"summary":"anthropic_messages -> openai_chat","actions":[{"type":"protocol_convert","from":"anthropic_messages","to":"openai_chat"}]}}
```

前一组是只要摘要的 adaptive：收不到正文；这个摘要最近没失败过，它把请求从默认的 retry 改为 forward。
后一组是 convert：输入里已是 forward，它转换协议，不表态投递方式。

- **`request`**：输入是应用了头覆盖之后的请求。回信里的 `url` 必须是绝对的 http 或 https URL，否则按输出不合法处理
  （`format_error`，kind 为 `invalid_output`）。
- **`delivery`**：输入是当前的投递方式；回信可以给 `forward` 或 `retry`，链上第一个给出的生效（§10.10）。
- **`client`**：客户端视角的路径与是否流式，链上每一层看到的都一样。
- **retry 阶段**的输入多一个 `retry` 对象：`{"attempt":3,"last_failure":{"code","details"},"elapsed_ms"}`，其中 `attempt` 是即将进行的尝试序号。
- **`overrides`**：键是 config.toml 的键，范围见 hello 的 `overrides`。
  - 回信是宽容的读者（§1.1）：不认识的键或非法的值被忽略并告警，不让请求因此失败。
  - 保活已经提交之后才到的 `keepalive.*` 覆盖不生效。
- **失败结构**（D-6）：

```json
← {"v":1,"seq":7,"fail":{"retry":false,"code":"model_not_routed","reason":"no channel configured for model gpt-9","response":{"status":404,"headers":{"content-type":"application/json"},"body":"{\"type\":\"error\",\"error\":{\"type\":\"not_found_error\",\"message\":\"model gpt-9 is not configured\"}}"}}}
```

  - `retry:true`：计为一次失败（`format_fail`），进入正常的重试循环；退避、计数、decide 与规则都照常。
    forward 请求没有重试循环：先按 §10.10 问 decide 要不要升级，不升级就回 502 `format_failed`（S3-6）。
  - `retry:false`：带了 `response` 就原样返回它，并加上 §3 的两个头，码为 `format_rejected`；没带就回 422 `format_rejected`。两种模式相同。
  - `code` 是 format 自定的机读串，可以省略，进入 `details.fail_code`；`reason` 用英文，不超过 512 个字符。
  - `fail` 是程序的决定，不是错误：可选程序回的 `fail` 同样执行。`required` 只管程序出错与熔断（S3-6）。
- **进程崩溃、超时、回信不合法**：
  - required 程序：单次按应重试（`format_error`），连续出现由熔断处理（§10.9）；forward 请求同 `retry:true`。
  - 可选程序：跳过这一层，把它的输入原样当作输出，并记一行告警。

### 10.5 decide

```json
→ {"v":1,"seq":12,"stage":"decide","request_id":"19ac3f2e8b5d1a2b-44","state":"…",
   "attempt":6,"elapsed_ms":42000,"delivery":"retry","outcome":"failure",
   "failure":{"code":"upstream_status","details":{"status":429}},
   "request":{"method":"POST","url":"https://relay.example.com/v1/chat/completions"},
   "response":{"status":429,"headers":{"retry-after":"60"},"body":"{\"error\":{\"message\":\"rate limited\"}}","truncated":null},
   "fallback":{"decision":"retry","delay_ms":20000,"by":"default"}}
← {"v":1,"seq":12,"decision":"retry","delay_ms":60000,"reroute":true,"report":{"summary":"429 on k2; upstream asks 60s; switch key and wait","actions":[{"type":"custom","label":"cooldown k2 60s"}]}}
```

样例里第 6 次尝试失败，下一次是第 6 次重试，现行退避给 20 秒（retry.rs:14-32），所以 `fallback.delay_ms` 是 20000，与上游的
`retry-after: 60` 无关。尊重 Retry-After 的是 format：它回了 60000（D-15，R-S3-1）。
这是另一个请求（`-44`）。§6.5、§10.4、§10.6 的前一段、§10.7、§10.11 的样例讲的是同一个请求 `-42`，时间线互相对得上；其余样例各用别的 `request_id`（T-S3-5 ④）。

- **时机**：retry 请求的每次尝试失败之后。声明了 `decide.success` 的程序，在尝试成功之后也会收到：`outcome` 为 `success`，没有 `failure`。
  这用来把「200 但内容是错误」判为失败。
- **forward 请求**（§10.10）：只在一个字节都没写出时失败，问一次。此时 `delivery` 为 `forward`，`fallback` 为 `{"decision":"stop","by":"default"}`，
  回 `retry` 就是让它升级。forward 请求成功时不问。
- **`response`**：有上游响应时给出，体是解码后的内容，上限 1 MiB；超出时保留开头 64 KiB 与结尾，`truncated` 标出省略了多少（S3-30）。
  没有上游响应就省略。
- **`fallback`**：不问 format 时 aproxy 会怎么做，即覆盖、规则表与默认三者得出的结论。
- **回信**（`decision` 与 `reroute` 各自取第一个表态的程序）：
  - `decision:"retry"`：重试。`delay_ms` 可选，取值 0–86 400 000；省略就用 `fallback.delay_ms`，fallback 是 stop 时按现行退避算。
    对成功的尝试回 retry，就是把它判为失败（`format_decision`），这个判决对这次尝试是终局。
  - `decision:"stop"`：停止。带了 `response` 就返回它（码 `retry_stopped`）；不带就透传上游响应；没有上游响应时回 502 `retry_stopped`。
  - `reroute`：这次重试要不要重跑重试链（§10.3）。true 重跑，false 沿用上一次发出的请求；没人表态时按默认处理。
    这就是用户原话里「是否换路由（every_attempt）」的落点。
    没有程序声明 retry 时，或上一次尝试在链里就失败了（没有发出请求）时，`reroute` 不起作用。
  - 省略 `decision`：表示不表态，交给下一个程序；都不表态，就用 `fallback`。
- **护栏**：
  - 超时（`decide_timeout_secs`）、崩溃、回信不合法，都视为不表态，并计入熔断；绝不因为 decide 出错而结束这个请求。
  - 每个决定连同理由，写进日志与请求记录的 `tries[].decision`。

### 10.6 响应方向：response 与 stream

**response 阶段**：整体处理一个响应。

```json
→ {"v":1,"seq":15,"stage":"response","request_id":"…","state":"…","request":{"method":"POST","url":"https://relay.example.com/v1/chat/completions"},"response":{"status":200,"headers":{"content-type":"application/json"},"body":"{…openai…}"}}
← {"v":1,"seq":15,"response":{"body":"{…anthropic…}"}}
```

- **范围**：只处理判定为成功、要交给客户端的响应。放弃之后透传的错误不经过它（X-3）；要按客户端协议改写错误，用 decide 的 `stop` 附带 `response`。
- **forward 请求**：响应要先在这一层攒齐再交给它，这一段失去真流式（§10.10）。
- **失败**（进程级失败，或熔断已开）：换一个新 worker 重做一次，仍失败就跳过这一层（用户 09-23：响应侧失败就透传，并告警）。
- **`fail`**：带 `response` 就用它，否则跳过这一层；`fail.retry` 在响应方向不适用。

**stream 阶段**：逐块处理，只能由 `persistent` 程序声明。

```json
→ {"v":1,"seq":20,"stage":"stream","event":"open","stream_id":"19ac3f2e8b5d1a2b-42/2","request_id":"…","state":"…","request":{"method":"POST","url":"…"},"response":{"status":200,"headers":{"content-type":"text/event-stream"}}}
← {"v":1,"seq":20,"response":{"headers":{"content-type":"text/event-stream"}},"abort_tail":"event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"api_error\",\"message\":\"stream conversion aborted\"}}\n\n"}
→ {"v":1,"seq":21,"stage":"stream","event":"data","stream_id":"19ac3f2e8b5d1a2b-42/2","data":"data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n"}
← {"v":1,"seq":21,"data":"event: content_block_delta\ndata: {…}\n\n"}
→ {"v":1,"seq":30,"stage":"stream","event":"close","stream_id":"19ac3f2e8b5d1a2b-42/2"}
← {"v":1,"seq":30,"data":"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"}
→ {"v":1,"seq":31,"stage":"stream","event":"abort","stream_id":"19ac3f2e8b5d1a2b-45/1","reason":{"code":"upstream_body_interrupted"}}
← {"v":1,"seq":31,"data":"…"}
```

前三组是请求 `-42` 第 2 次尝试的流（升级为 retry 之后，回放时转换）；最后一组是另一个 forward 请求 `-45` 的流，上游中途断开。

- **四种事件**：
  - open 的回信可以改状态与头，可以给出 `abort_tail`（format 中途失败时，aproxy 按客户端协议写出的收尾），也可以带初始的 `data`；
  - data 的回信可以没有字节；
  - 上游正常结束时发 close；
  - 上游中断或客户端断开时发 abort。客户端已经断开时，回信会被丢弃。
- **`stream_id`** 等于 `<request_id>/<attempt>`。
- **worker**（S3-31）：
  - 一条流固定由一个 worker 处理，独占它直到 close 或 abort；
  - 这些 worker 不计入 `pool_max`，以免长流占满池子；
  - 流结束后，池子没满就回到池里，否则退出。文档写明进程数等于池子的大小加上并发的流数。
- **字节**：交给 stream 的永远是解码后的字节；输出时去掉 `content-length` 与 `content-encoding`。
- **模式**：
  - retry 请求的响应缓冲后回放，回放时边放边转；「200 但内容是错误」的判定在转换之前，对上游的原始数据做。
  - forward 请求的响应边收边转（§10.10）。
  - 响应头已经提交之后，open 回信里的状态与头被忽略。
- **失败**：
  - open 时的失败，同 response 阶段的处理；
  - 流中途某一层失败，aproxy 停读上游，向失败层外侧的各层依次发 abort；失败的是最外层，就写它的 `abort_tail`；
  - 可选程序在流中途失败，之后的块跳过这一层。
- **熔断**只拦新的阶段入口（open）；进行中的流继续由原来的 worker 处理。

### 10.7 heartbeat

```json
→ {"v":1,"seq":40,"stage":"heartbeat","request_id":"…","state":"…","heartbeat":{"beat":3,"elapsed_ms":45000,"attempt":2,"last_failure":{"code":"upstream_status","details":{"status":529}}},"client":{"path":"/v1/messages","stream":true},"data":": keepalive\n\n"}
← {"v":1,"seq":40,"data":"event: ping\ndata: {\"type\":\"ping\"}\n\n"}
```

- **`data`**：最内层收到 `keepalive.heartbeat` 的静态字节，之后每层收到内侧一层的输出；回信省略 `data` 表示不变。
- **每一拍都带 `client`**，判断协议够用了；不再像现行那样在第 1 拍附上请求体（S3-32）。
  需要更多信息的程序，在 request 阶段写进 `state`，心跳时读出。
- **迟到与失败**：一拍没在下一拍之前回来，就照发上一层的字节；出错或熔断时，这一拍用输入的字节。

### 10.8 report

- 形状：`{"summary":"<一句英文，最多 512 个字符>","actions":[{"type":"…",…,"label"?}]}`，`actions` 最多 32 条，超出就截断并告警。
- 动作类型表只增不改；遇到不认识的类型，显示它的 `label`。

| type | 字段 |
|---|---|
| `protocol_convert` | `from`、`to`：开放词表，例如 `anthropic_messages`、`openai_chat`、`openai_responses`、`gemini` |
| `key_select` | `key_id`：编号或别名，绝不是 key 本身 |
| `route` | `channel`、`model_from?`、`model_to?` |
| `header_edit` | `name`、`op`（`set` 或 `remove`） |
| `body_edit` | `path`（JSON Pointer）、`op`（`set` 或 `remove`） |
| `custom` | `label` |

### 10.9 熔断（D-12）

- **范围**：每个运行中的实例、每个 `[[format]]` 条目各有一个熔断器（声明了 notify 的另有一个 notify 的熔断器，见下），只在内存里，重启即复位。
- **计数**：
  - 这个程序在数据通路的消息（notify 以外）上的进程级失败，跨请求、跨阶段按完成顺序连续计数：起不来、I/O 错误、中途退出、
    超时（含 decide 与 hello）、协议错误、输出不合法。
  - 任何一个合法回信都把计数清零；`fail` 回信与「不表态」都算合法回信。客户端断开不计。
  - notify 不计入这个熔断器，它的回信也不清零这里的计数：notify 有自己的熔断器（见下，T-S2-1）。
    否则一个数据通路坏掉、却还能应答通知的程序永远熔断不了；反过来，通知这条路出了问题，也不该把判断路径一起关掉。
- **熔断**：计数达到 `breaker_failures`（默认 5，0 表示永不熔断），状态转为 `open`：
  - 杀掉这个程序的空闲 worker；
  - 记下 `opened_at`，`probe_at` 等于 `opened_at` 加上 `breaker_cooldown_secs`（默认 30）。
- **熔断期间**，新的阶段入口都不再发给这个程序；熔断那一刻已在排队、还没写出的消息也一样（T-S3-8 ④）：

| 阶段 | required 程序 | 可选程序 |
|---|---|---|
| request、retry | 请求立即结束：没提交时回 503 `format_unavailable`，已提交时写同码的 SSE 终态；不再重试 | 跳过这一层；它也就没给 `delivery`，请求走默认的投递方式 |
| decide | 视为不表态 | 同左 |
| response、stream 的 open | 跳过这一层（与响应侧失败相同） | 同左 |
| heartbeat | 这一拍用输入的字节 | 同左 |
| notify | 丢弃并计数（§10.11） | 同左 |

- required 程序只在 request、retry 阶段让请求快速失败，其余阶段与可选程序一样跳过或回落。这是对 D-12 的细化（§13.3，R-S3-16）：
  定稿要求 decide 出错一律回落，用户 09-23 也定过响应侧失败就透传。
- 可选程序熔断时，请求走配置的默认投递方式（R-S1-1）：
  - 默认 `retry`：照常无限重试，规则表与现行退避就是静态兜底；
  - 默认 `forward`：请求转入 retry，靠 decide 程序升级，或 request 程序回 `delivery`。decide 程序不可用时视为不表态，不论 required 与否都不升级；
    可选的 request 程序被跳过，也给不出 retry。这期间请求一律不重试，doctor 对这种配置给出 `forward_default_unguarded`（§2.4，T-S3-1）。
- **半开**：到了 `probe_at`，下一条要发给这个程序的数据通路消息作为试探放行（同一时刻只放一条），状态为 `half_open`；其余消息仍按熔断处理，
  notify 不作这个熔断器的试探，熔断与半开期间一律丢弃并计数。
  - persistent 程序的试探用新起的 worker，先握手。
  - 试探成功：转为 `closed`，计数清零。
  - 试探失败：重新 `open`，冷却重新计时，时长不翻倍。试探所在的请求按单次故障处理；它的下一次尝试遇到熔断，就立即结束。
- **notify 的熔断器**（T-S2-1）：声明了 notify 的程序另有一个，只管 notify。规则与上面相同，沿用同样的 `breaker_failures` 与 `breaker_cooldown_secs`：
  - 计数：notify 的失败按完成顺序连续计数，包括为它起的 worker 起不来、写不进 stdin、进程退出、超时（§10.11 的固定时限）、回信不合法；
    notify 的合法回信清零。
  - 熔断：暂停派发 notify，排队的与新到的通知丢弃并计数；不杀 worker，数据通路照常。
  - 半开：冷却之后放一条 notify 试探（其余的仍丢弃），成功就恢复，失败就重新熔断。
  - 数据通路的熔断器打开期间，notify 照上表丢弃，这些丢弃不计入 notify 的熔断器。
  - 只声明 notify 的程序只有这一个熔断器起作用：启动握手失败时由它以 `open` 起步，冷却后用 notify 试探，所以能恢复；
    其余程序握手失败时，是数据通路的熔断器以 `open` 起步。
- **单次故障仍按 D-6 应重试**，只有连续故障才熔断。网络类失败从不触发熔断：上游永远连不上时，仍然无限重试。
- **可见性**：
  - 快照的 `formats[].breaker` 给出状态、计数、`opened_at`、`probe_at` 与最近一次失败；notify 的熔断器在 `formats[].notify_breaker`，形状相同；
  - 状态变化立即发 `state` 事件，界面据此显示「不可用、试探中」。
- **日志**：
  - 熔断写一行 WARN，带 `format=`、`failures=`、`cooldown_secs=`；半开与恢复各写一行 INFO；notify 的熔断器另带 `stage=notify`；
  - 因熔断而结束的请求，日志带 `code=format_unavailable`。

### 10.10 投递方式：按请求在 forward 与 retry 之间选（用户追加 2026-10-12）

**两种投递方式**（`delivery`）：
- `retry`：现行的重试模式。先读完请求体；响应缓冲后回放；失败按 §7.3 与 decide 重试；可以保活。
- `forward`：仅转发。响应边收边转（真流式）；不保活，不调用 heartbeat 与 retry 阶段；成功时也不调用 decide，
  `decide.success` 只对 retry 请求生效，否则就得先缓冲响应。

**每个请求的投递方式**：
1. 起始值是配置的 `delivery`（默认 `retry`）。推荐配方保持这个默认，由判断程序把它认为安全的请求改成 forward（§10.14，R-S1-1）。
2. request 阶段的消息带当前的 `delivery`；回信可以给 `delivery`（`forward` 或 `retry`）。
   链上第一个给出它的程序说了算；之后的程序在输入里看到它，它们给的被忽略。
   其余阶段的回信给了也忽略；取值不认识的，忽略并告警。
3. 请求记录与完成通知里，`delivery` 是请求所处的投递方式，`upgraded` 表示它中途从 forward 转成了 retry（§6.5、§10.11）。

**forward 请求的请求体**：
- 链上有程序在 request 阶段要 `full` 或 `digest`，或者有程序声明了 decide（升级之后要重发），aproxy 就先读完整个请求体，大的照现行做法溢写；
- 否则请求体直接流向上游，与现行的仅转发一致。这时没有程序能让它升级，也就不必留体。

**请求内升级**：
- forward 请求的这次尝试失败，并且还没有向客户端写出任何字节（响应头也算）时，aproxy 问一次 decide。
  失败指 §2.2 的尝试失败：连不上、等不到响应头、上游回非成功状态、请求侧 format 失败等。
- 链上有 decide 程序时，上游的非成功响应先按 retry 请求的做法缓冲（上限 `max_response_mb`），问过 decide 再处理。
- decide 的结局：
  - `retry`：当场转为 retry 模式，此后与一开始就用 retry 完全一样。这次尝试算第 1 次，规则表与按请求覆盖从此生效；
    下一次尝试是否重跑重试链按 `reroute`；等待按 `delay_ms`，省略时按现行退避，也就是立即重试；可以开始保活。
  - `stop` 并带 `response`：返回它，码为 `retry_stopped`。
  - 其余情况（`stop` 不带响应、不表态、decide 出错或熔断）按 forward 的默认结束：透传上游响应，不加 `x-aproxy-*` 头；
    没有上游响应时回 502 `upstream_failed`；请求侧 format 失败时回 502 `format_failed`。
- 已经向客户端写出字节之后不再升级，与现行一致。例如 2xx 的流中途断开：连接照现行做法结束（经过 stream 程序时写出 `abort_tail`，§10.6），
  记录的终态码为 `upstream_failed`，完成通知的 `stream_completed` 为 false。

**forward 请求的「200 但内容是错误」**（R-S3-8）：
- 转发的字节一个不改。aproxy 在旁边被动地做 retry 请求现行的那套判定，做法也一样，与历史是否保留了体无关（T-S3-4）：
  - 响应不超过 1 MiB（`RESIDENT_LIMIT`，proxy.rs:459）时，留一份原始字节的副本；压缩体先解码（输出上限 8 MiB，记忆 2026-09-14），
    再用 `is_error_body`、`is_stream_error_body` 判定；
  - 超过 1 MiB 的不留副本，改用现行磁盘模式的增量行扫描（`StreamErrorScanner`，proxy.rs:891），只看原始字节、不解码
    （decode.rs:35-38 记的已知边界）。
  - 所以判定不在预算之外占内存：几百 MB 的下载也只多占这 1 MiB。
- 判定为错误时，这次尝试的码记为 `upstream_error_body`，记录的 `outcome` 为 `upstream_error`、`code` 为 `upstream_failed`，
  完成通知也这样报。否则 adaptive 靠完成通知判断成败时，对这类常见故障是盲的，界面也会把它显示为成功。

**forward 请求的响应方向**：响应边收边转，经过声明了 stream 的程序实时转换（§10.6）。
链上有声明 response 的程序时，响应要在它那一层攒齐再交给它，这一段失去真流式；doctor 对「默认 forward、链上有 response 程序」提示改用 stream。

**为什么**：
- forward 是真增量流，对 agent 客户端兼容最好；retry 要缓冲每轮响应再回放，保活心跳对一些客户端容易失效，而客户端自带的重试次数少、间隔长。
  所以让 format 按请求在两者之间选，aproxy 只提供机制（用户 2026-10-12）。
- 「第一个表态的说了算」与 decide 的 `decision`、`reroute` 是同一条规则（§10.3）：配置里的顺序就是优先级，后加到链尾的程序不会悄悄推翻前面的决定。
- 只在一个字节都没写出时升级：写出之后再换模式，客户端会收到两份响应的拼接。
- 有 decide 程序就留体：否则 decide 叫升级时无体可重发，这个决定就成了空话。
- 默认仍是 retry：用户补充过「对于无限重试依旧保持默认」。判断程序坏了，退路是 retry 而不是 forward，失去的只是真流式（R-S1-1）。

### 10.11 notify：完成通知（0.2.0 实现，用户追加 2026-10-12）

```json
→ {"v":1,"seq":51,"stage":"notify","event":"request_end","request_id":"19ac3f2e8b5d1a2b-42","state":"{\"digest\":\"9f86d081…\"}","digest":"9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08","delivery":"retry","upgraded":true,"attempts":2,"upstream_status":200,"stream_completed":true,"duration_ms":48210,"outcome":"ok","code":null}
← {"v":1,"seq":51}
```

- **时机**：请求结束之后（响应交付完毕、客户端断开或终态错误），给每个声明了 notify 的程序发一条。发完，这个请求的 state 就丢弃。
  进链之前就被拒绝的请求（403 的 Host 或 Origin、413、`client_body_aborted`）不发：它们没进过链，format 也没见过它们（R-S3-7 ①）。
- **字段**：
  - `delivery`、`upgraded` 见 §10.10；`attempts` 是总尝试数；
  - `upstream_status` 是最后一次上游响应的状态码，没有时为 null；
  - `stream_completed`：流式响应是否正常结束，不是流式响应时为 null；
  - `outcome`、`code` 与请求记录相同（§6.5）；
  - `digest` 只给在 notify 上声明了 digest 的程序（§10.2）；请求体不完整时为 null。
- **只发不等，绝不拖慢数据通路**（R-S2-1、T-S2-1）：
  - format 照常回空回信 `{v, seq}`，保持一问一答；aproxy 不在数据通路上等它。
  - 派发：notify 只在这个程序有空闲 worker、且没有数据通路的消息排队时才写出（§10.1）。数据通路的消息最多等一条已经写出的 notify，
    上限是下面的固定时限，加上超时之后换 worker 的时间。
  - 时限：固定 2 秒（E 级常量），从写出时起算，不随 `timeout_secs`、`decide_timeout_secs` 变。所以它们取 0（不限）时，
    一条卡住的 notify 也不会让请求永久挂起。超时或回信不合法时，那个 worker 被换掉。
  - 熔断：notify 的失败只计入 notify 自己的熔断器（§10.9），连续达到 `breaker_failures` 就暂停派发 notify，冷却后放一条 notify 试探。
    数据通路的熔断器不受影响，所以观察路径出问题，不会把判断路径一起关掉；一个总是超时的 notify，也不会让数据通路每次都等。
  - 队列：每个程序一个，按条数与字节都设上限（E 级常量：1024 条、16 MiB，`state` 计入字节，R-S3-7 ②③）。
    超出时丢弃最旧的并计数；任一熔断器打开期间的通知直接丢弃并计数。计数在快照的 `formats[].notify_dropped_total`，日志里限频告警。
  - 高负载：数据通路一直有消息排队时，notify 可能一直派发不出去，只能丢弃并计数。协议文档写明：判断类程序可以把
    「同一个摘要再次出现」当作补充信号，客户端重发往往说明上次失败了（T-S3-8 ⑤）。
- **约束**：声明 notify 的程序必须是 `persistent`（§7.4）。
- **以后（加性）**：hello 的 `features` 可以协商「一个 worker 上同时有多条在途消息」。`seq` 已经能把回信对上，
  单进程的程序可以自己开线程，`pool_max = 1` 也就不再是瓶颈。
- **以后的事件**（R）：`attempt_start`、`attempt_end`，带上游的限流响应头，供额度与限速规则使用。
  只发给在 hello 的 `features` 里声明了 `notify.attempt` 的程序，旧程序不会收到。订阅额度相关的头怎么读，先实测再定。

```json
→ {"v":1,"seq":52,"stage":"notify","event":"attempt_end","request_id":"…","attempt":1,"at":1760190000999,"outcome":"failure","failure":{"code":"upstream_status","details":{"status":429}},"response":{"status":429,"headers":{"x-ratelimit-remaining-requests":"0"}}}
```

**为什么**：
- 完成通知让 format 在请求之外积累判断，例如 adaptive 按摘要统计失败（用户 2026-10-12）。只发不等、处理不过来就丢，
  它拖累数据通路的上限是一条在途的 notify。
- 保留空回信：一问一答是串包检测的根基（§10.1），通知也不例外（S3-33）。aproxy 不在数据通路上等它。
- notify 有自己的熔断器：观察路径慢了或乱了，不该把同一个程序的判断路径也关掉（R-S2-1）；可它自己也要有保护，
  否则一个总是卡住的 notify 会让每条通知都换一次 worker，只声明 notify 的程序熔断之后也永远等不到试探（T-S2-1）。
- 时限是固定常量：借用 `decide_timeout_secs` 的话，它取 0（不限）时一条卡住的 notify 会让 worker 一直忙着，
  排在后面的请求就此挂起（T-S2-1）。记一笔账用不了 2 秒，这一项也不需要用户来调。
- 不给 notify 单独开 worker：adaptive 在 request 阶段查、在 notify 时记的是同一张表，拆到两个进程里，判断就看不到结果了。
- 丢最旧的：判断类程序更需要新近的结果。
- 只许 persistent：spawn 模式每条通知都要起一个进程，代价不成比例，也存不住跨请求的状态。

### 10.12 预留：admit（R，只定形状，在 0.2.0 之后实现）

admit：每次发往上游之前问 format「现在能发吗」。

```json
→ {"v":1,"seq":50,"stage":"admit","request_id":"…","state":"…","attempt":1,"request":{"method":"POST","url":"…"},"queue":{"length":3,"position":1,"waited_ms":1200}}
← {"v":1,"seq":50,"admit":false,"retry_after_ms":5000,"reason":"5h quota window full"}
← {"v":1,"seq":50,"admit":true,"grant":{"count":10,"ttl_ms":60000}}
```

- 排队不算失败，客户端的心跳照发；只问队首。
- format 崩溃或超时时，回落到静态的并发上限（实现时以加性的配置键给出）；没配上限就放行。

### 10.13 aproxy-envelope 0.2.0

- **类型**：hello 与它的回信（含 `request_body`）、各阶段的消息与回信（含 `delivery` 与 notify 事件）、HTTP 对象及其补丁、`Fail`、`Overrides`、`Report`。
- 结构体都标 `#[non_exhaustive]`，并提供构造函数（DP-13 第 7 点）；`Display` 用英文。
- **版本**：预发布期间随主线用 `0.2.0-alpha.N`，与 aproxy 0.2.0 同期发布（RT-06）。

### 10.14 官方 aproxy-format：按角色拆成子命令（用户追加 2026-10-12）

aproxy-format 是官方单元的合集：`aproxy-format <角色>` 只启用一个单元，各自挂进 `[[format]]` 链，也能与第三方程序混用；去掉大一统的 `run`。
下表是各角色预计用到的阶段与请求体粒度，用来核对 aproxy 一侧的契约够用；各角色的参数与行为属于 W2c。

| 角色 | 阶段 | 请求体 | 依赖 aproxy 一侧的契约 |
|---|---|---|---|
| `convert` | request、stream；排在 route 之后时加 retry | full | 请求补丁；stream 的四种事件与 `abort_tail`，forward 下实时转换，retry 下回放时转换（§10.6、§10.10） |
| `keys` | request、retry | none；开会话粘性时 full | 只改头的请求补丁；retry 输入里的 `last_failure` 用来冷却 key；`state` 记下这次用的 key |
| `route` | request、retry | full | 读 model、改写 url 与 model；换不换渠道由 decide 的 `reroute` 定；换上游时凭据由 keys 一起换（§7.5） |
| `policy` | decide | none | `decision`、`delay_ms`、`reroute`、stop 附带的 `response`；`decide.success` |
| `adaptive` | request、decide、notify | request 与 notify 要 digest | `delivery`：默认 retry 之下，对没失败过的请求回 forward；forward 请求失败时用 decide 升级；完成通知。配 `required = false`、`pool_max = 1`、`worker_idle_secs = 0` |
| `rewrite` | request；排在 route 之后时加 retry | full | 请求补丁 |
| `heartbeat` | heartbeat | — | 每拍的 `client` 与 `last_failure`（§10.7） |

- 工具子命令 `init`、`check`、`try` 与一次性转换 `translate` 不走信封协议，不受本稿约束；`try` 干跑整条链时，按本节的语义驱动各单元。
- `init` 生成的起步配置沿用默认 `delivery = "retry"`，由 adaptive 把没失败过的请求改成 forward（R-S1-1）。
  它把 route 之后的 convert、rewrite 也声明 retry，每次重试按新的路由重做；`check` 照此校验（R-S3-2）。
- 判断类单元（`policy`、`adaptive`）建议配 `required = false`：坏了就熔断跳过。跳过之后请求按配置的默认投递方式走（§10.9）：
  默认 retry 时照常无限重试，规则表与现行退避兜底；默认 forward 时，在它们不可用期间请求不再重试，doctor 对此告警。

**为什么**（§10 整体）：
- 按角色拆成单元：每个单元有自己的 worker 池，重活不拖累轻活，一个单元崩了只影响它自己（用户 2026-10-12）。
  代价是请求路径上每个单元多一跳本地管道往返，所以 `init` 只生成场景真正需要的单元。
- 请求体按粒度给：只做判断的单元拿摘要就够，几 MB 的正文不必经过它的管道，它也看不到对话内容。
- spawn 模式也握手：用户原话是「每个 format 进程启动时先交换协议版本与能力」。多一次往返，比起起进程的开销可以忽略，所以没有困难。
- `seq` 必须回显：协议 1 是全新的契约，没有需要兼容的第三方实现。
- 「声明 retry 的程序排在后段」：这样没声明 `retry` 的程序不会因为重试而被再次调用，链的输入也始终一致。
  否决「重试时给它们再发一次 `stage:"request"`」：对只想每个请求处理一次的审计、计数类程序是意外（S3-34 ③）。
- 熔断按实例 × 程序：程序的健康状况由它所在的进程池决定，跨实例共享状态没有依据。半开只放一条，所以坏掉的程序每个冷却周期最多拖累一个请求。
- 默认 5 次、30 秒：零延迟的前 3 次重试足以让一个确定坏掉的程序在一两秒内熔断；而偶发的单次故障之间夹着成功，不会被误熔断。
- 心跳不许写 `state`：一拍与 decide、retry 可能同时在途，后到的回信会覆盖先到的。

---

## 11 冻结清单

0.2.0 发布时，据此重写记忆 ipc-v1-upgrade-contract。

| 契约 | 级 | 所在 |
|---|---|---|
| C1–C8 | F | 计划 W2b |
| 错误码表及其 HTTP 状态、退出码、`details` 的键与类型；IPC 结果到 CLI 的对应 | A | §2 |
| HTTP 错误体、两个 `x-aproxy-*` 头、SSE 终态事件 | A | §3 |
| `request_id`、`request_seq`、`instance_id` | A | §1.3 |
| 布局与文件名；记录 schema 1（instance、restore、spec、watchdog、skills manifest）；spec 与实例记录的 `config_path` 可为 null；spec 的 `pinned_port`（端口 0 钉成的实际端口，配置改成固定端口后不用）；被新端口的记录接替的期望状态记录由拉起方删；install.state 的 `halted`；心跳与宣告的布局；`home_id` 与端点名；清理规则；一个端口一个实例 | A | §4 |
| 内部入口 `__serve`、`__watchdog`、`install --continue --handover-from` 及其参数；`starting` 状态与就绪判定；install 在广播之前的检查与预检 | A | §5 |
| IPC v1：帧、规则、op 与参数、结果形状、错误码、事件、`features`、记录的上界（前 3 加后 20、单条 64 KiB 与截短顺序）、`body` 的分块读取与体的口径（解码、替换之后的字节） | A | §6 |
| config.toml 的键、类型、默认值、0 的含义、层级；严格解析；头表按名合并与凭据只发给所属目标（D-16） | A | §7 |
| settings.json schema 1；`defaults` 的范围（D-16）；`ui` 归桌面所有 | A | §8 |
| 命令树、旗标、退出码、`--json`、install 的 NDJSON、stdout 纪律 | A | §9 |
| format 协议 1：hello（含 `request_body` 粒度与摘要算法）、`seq`、各阶段的消息、补丁语义、fail、overrides、`delivery` 与请求内升级、notify 的 `request_end` 事件与丢弃语义、worker 的占用与计时、notify 的派发与熔断规则、report、熔断的语义与配置键、hello 至少等 10 秒 | A | §10 |
| 日志里的 `code=`、`request_id=` 字段 | A | §1.1 |
| 文案与 `message`、调参常量（30 秒就绪时限、1000 条记录、`body` 单块 1 MiB、notify 队列 1024 条与 16 MiB、notify 时限 2 秒、10 分钟宽限、流式连接上限 8 等）、doctor 的检查项 | E | — |
| 规则的 `body` 匹配器；format 的 admit 阶段；notify 的 attempt 事件 | R | §7.3、§10.11、§10.12 |

---

## 12 第一轮审查（综合稿与四份报告）的对账

| 编号 | 着落 |
|---|---|
| K1、DP-03、CI-12（码的部分）、RT-16 第 1 点 | §2 |
| K2、DP-01、DP-02 | §3 |
| K3、D-1、DP-04、DP-25、DP-26、X-4 | §7.3；DP-25 由 `decide.success` 承担，规则的 `body` 在 0.2.x 实现；DP-26 的 Retry-After 与抖动按 D-15 不做 |
| K4、CP-03、CP §3、CP-10、CP-25 | §6 |
| K5、CP-01、CP-05、CP-15、CP-16、CP-22、CP-24、CP-26、CP §5、CI-20(d) | §4；K5 建议的全局 `--home` 没有采纳（审查 §4-8，理由见 §9.1；列在下方「不在 0.2.0 做的」） |
| K6、D-2、CP-02、CP-13、CP-14、CI-02、CI-03 | §4.2、§5 |
| K7、CI-04、CI-10 | §7.6、§8.3 |
| K8、D-3、CP-04、CP §8、CI-13 | §2.4、§9 |
| K9、D-4、CI-05、D1 | §9.3 |
| K10、DP-13、RT-06 | §10 |
| K11、D-7、CP-27、CI-17、D4 | §9.1、§9.2 |
| K12、D-5、CI-11 | §8 |
| K13、DP-07 | §7.1 |
| K14、DP-14 | §1.2 |
| D-6、X-1、X-2 | §10.4；单次故障应重试，连续故障由 D-12 熔断 |
| D-8、CP-11、CP-18、综C2 | §5 |
| D-9 | W4、W7 执行 |
| D-10 | 被「先发 alpha.1」取代 |
| D-11 | W2b，本稿只协调 |
| DP-05、综C6 | 被 D-6 取代，连续出现由熔断兜住 |
| DP-06、综C7；DP-24、综C12；DP-27、综C10 | 各自的码在 §2.1；停止语义见 §5 |
| DP-11、O1 | `request_id`（§1.3） |
| DP-12、O2、DP §8 | §6.4–§6.7 |
| X-3 | 0.2.0 不做，decide 的 `stop` 附带响应可以覆盖 |
| CP-06、CP-07、综C1 | §4.3、§6.6 |
| CP-09、SE1、CI-14（权限） | §1.6 |
| CP-12、CP-23、综C3；综C4；综C5 | §5、§4.3 |
| CP-19、U6 | §5：界面拉起实例用 `<home>/bin/aproxy` |
| CP-21 | §5 |
| CP §4 | §11 |
| CI-08、CI-09、综C9 | §7.5；CI-09「未知键只告警」被 D-14 改为报错 |
| CI-14（打码与 stdin）、CI-22、SE2 | §1.5、§9.3 |
| D2、D3 | D-2；D3 不采纳，`prepare_swap` 加 `ttl_secs` |
| A5 | §4.2 |
| TODO「format 扩展的开放问题」 | state 上限 64 KiB；心跳带 `last_failure` |
| TODO「仅转发模式在 status/find/doctor 里没有模式标识」 | 快照的 `behavior`；请求记录的 `delivery` |
| G1 | §6.3、§6.5 |
| G2 | §9.2 第 1 步 |
| G3 | §4.1 |
| G4 | §1.1 |
| G5 | §2.4 |
| G6、G8 | 属于 W2b |
| G7 | 不属于本稿 |
| G9 | §1.6 |

**不在 0.2.0 做的**（以后都能加性补回）：
- 配置热重载；
- `shutdown.drain_secs`；home 级看护者端点；`stream.interleave`；
- `logs` 与 `config` 两个 op；`subscribe` 的参数；用户可见的 `--home`；
- `version --json`；`config set` 按下标改元素；
- admit 的实现；notify 的 attempt 事件；
- 删除从 `defaults` 继承来的头的写法（D-16；加性补回，旧版本遇到新写法按 D-14 拒绝启动，不会被读宽）；
- 错误响应交给 format 转换（X-3）；
- 查看请求历史的 CLI（agent 先 grep 日志里的 `request_id=`）；
- hello 回信里给界面看的程序内部信息；
- 一个 worker 上同时有多条在途消息（由 hello 的 `features` 协商，§10.11）；
- Web UI。

---

## 13 用户决策记录

### 13.1 第一版的三个问题，已由用户决定

| 第一版的问题 | 用户的决定 | 落点 |
|---|---|---|
| Q1 规则表加 `codes` 匹配器 | 不加，改用 format 熔断（D-12） | §10.9、§7.4 |
| Q2 请求体与响应体是否默认保留 | 全部保留，只在内存，有可配置的总量上限（D-13） | §6.5、§7.1 |
| Q3 旧键告警还是拒绝 | 任何不认识的键都报错，不写任何识别旧键的代码（D-14） | §1.1、§7.5、§8.2 |

另有 D-15（重试时序不信上游）及用户的补充：默认无限重试不变，等多久、停不停都以 format 的结构化指令为主，aproxy 只保留规则表与现行退避这两样静态兜底。
落点在 §7.3。

用户追加（2026-10-12，计划 W2c，9e39930）：
- aproxy-format 按角色拆成子命令，去掉 `run`；
- 按请求在 forward 与 retry 之间切换，判断与暂存都在 format 里；aproxy 只提供投递方式 `delivery` 的覆盖、请求内升级、完成通知与请求体摘要。
落点在 §10.2、§10.4、§10.5、§10.10、§10.11、§10.14，处理细节见 §15。

### 13.2 第二版的 Q4，已由用户决定：D-16 settings 的 `defaults`（2026-10-12）

第二版的 Q4 推荐把上游与凭据类的键都排除在 `defaults` 之外。用户没有采纳，原话：

> 不同情况得有不同做法，listen_addr显然不可能相同，直接不要。url和key有可能一样，可以放默认。extra_headers、override_headers是多条的，
> 用合并策略。proxy也可以默认，因为也很常见……毕竟默认值本身也是为了方便

主代理补了「凭据只发给它所属的目标」这条约束，用户同意（「可以」）。原文在计划的 D-16（53afebd）。

| 决定 | 落点 |
|---|---|
| `defaults` 收 §7.1 标 ✓ 的键：行为与限额类，加上 `base_url`、`api_key`、`proxy` 与代理口令、两张头表 | §7.1、§8.1 |
| `listen_addr` 不收（实例身份，两个实例不可能相同）；`format`、`log_file` 照旧不收 | §7.1 |
| 两张头表按头名合并：不分大小写，同名以配置为准，各自合并不交叉；暂不提供删除继承来的头的写法 | §7.5、§12 |
| 默认的 `api_key` 与默认头表里的密钥头，只在最终的 `base_url` 与 `defaults.base_url` 指向同一目标时继承；代理口令只跟默认 `proxy` | §7.5 |
| `defaults` 有凭据却没有它的目标时报错；没继承的凭据经 `withheld` 与 `default_credential_withheld` 让用户看得见 | §7.5、§2.4 |
| settings.json 因此含密钥：打码、只写不读、文件权限都按 config.toml 的同一套规则 | §1.5、§1.6、§8.1 |

这也细化了 D-5：`defaults` 不再与 config.toml 字面同构。

细化（R-S3-9 ④）：「代理口令只跟默认 proxy」实现为「最终的 proxy 与 `defaults.proxy` 指向同一个代理」，比较 scheme、主机与端口，
不比 userinfo，比字面略宽。它满足用户同意的原则「凭据只发给它所属的目标」，主会话已定（§13.3），不再请用户过目。

### 13.3 主会话按用户授权定下的（第二轮复审与第三轮核对，2026-10-12）

用户授权主会话在已有原话的范围内决定复审各条。下表记下依据；没有直接用户原话的，写明实际依据。

**第二轮复审**（R-S1-1…R-S3-18）。其中 R-S2-1、R-S2-3 的一部分被第三轮的决定取代，见下一张表。

| 复审条目 | 决定 | 依据 | 落点 |
|---|---|---|---|
| R-S1-1 | 推荐配方与 `init` 沿用默认 `delivery = "retry"`，adaptive 对没失败过的请求回 forward；不加「跳过就按 retry 投递」的 A 级覆盖（会推翻用户显式写的 forward）；doctor 对「默认 forward、能升级的程序都可选」告警 | 用户：「对于无限重试依旧保持默认」（计划 D-15 后的补充） | §2.4、§7.2、§10.9、§10.10、§10.14 |
| R-S2-1 | 数据通路的消息优先，notify 只在有空闲 worker、且没有数据通路的消息排队时派发；计时从写出起；notify 除写不进去与进程退出外，一律不计入熔断（第三轮改为 notify 一切失败都只计入它自己的熔断器，见 T-S2-1） | 用户拆单元的理由：「消息处理有先后顺序，全靠单个进程在超高频时容易阻塞」；完成通知「只发不等回信」（计划 W2c，主代理细化、用户同意） | §10.1、§10.9、§10.11 |
| R-S2-2 | 历史存解码后的字节并记下原来的 content-encoding；解码到超出预算就停，记为 `not_retained`；按流替换已知密钥，重叠窗口不短于最长密钥减一 | 用户：「全部暂留在储存中」（D-13）；「aproxy的ui只是ui而已，实际看不到任何东西，敏感内容全在aproxy其他实例中」（2026-10-11，记忆 0.2.0-direction），由此有「IPC 永不返回明文密钥」 | §1.5、§6.5 |
| R-S2-3 | 端口为 0 时把实际端口钉进 spec；恢复时回到同一个端口，被占就按固定端口的常规失败处理（第三轮补充：配置改成固定端口后以配置为准，见 T-S3-6） | 无直接原话。主会话的理由：客户端配的是这个端口，恢复时要回到同一个端口；对应项目的核心目标（AGENTS.md：「Enable infinite retries (and uninterrupted operation) for the agent program's requests」） | §4.2、§5 |
| R-S2-4 | 续作入口在广播之前检查管不了的实例，查到就以 failed 加 `last_error` 结束 | 无直接原话；依据计划 W1 对升级路径的要求：「必须在改动任何东西之前失败，不能半途坏掉（实例失联、二进制已换）」 | §4.2、§4.3、§5 |
| R-S3-8 | forward 请求的「200 但内容是错误」在副本上被动判定，写进记录与完成通知，不改动转发的字节 | 用户提出的「先仅转发、失败多次再无限重试」（2026-10-12）：要数失败，就得先看得见失败 | §6.5、§10.10 |
| R-S3-9 ③④⑤ | ③ URL 内嵌的 userinfo 随 URL 值整体以配置为准，不拼接；④ 按「同一个代理」比较；⑤ base_url 连路径一起比较 | 「凭据只发给它所属的目标」（D-16，主代理补、用户「可以」） | §7.5、§13.2 |
| R-S3-10 | 保留「`defaults` 有凭据却没有目标」的报错；写入时的报错直接说「先设 base_url」 | 同上（D-16） | §7.5、B3 |
| R-S3-13 | 没有配置文件启动的实例，spec 记 `config_path: null`，restore 不检查它 | 用户：「毕竟默认值本身也是为了方便」（D-16）：只靠默认值启动的用法要保留 | §4.2、§5 |
| R-S3-15 | 契约测试里的 `use` 路径不算契约：B0、P4 只许改 `use` 行，断言逐字不变；AGENTS.md 与 CLAUDE.md 一起改 | 用户：「从现在开始保持绝对兼容」（W2b，2026-10-11）：契约测试的断言一个字都不能动 | §6.6、§14 |
| R-S3-16 | 标为对 D-12 的细化：required 程序只在 request、retry 阶段让请求快速失败，其余阶段回落或跳过（按复审建议） | D-12 的用户决定，以及用户 2026-09-23 拍板的「响应侧失败 → 透传上游原始响应 + warn」（记忆 transform-format 的记载） | §10.9 |
| R-S3-17 | install 与 stop、status 一样，settings.json 读不了时用内置默认运行并告警；start 的报错说清怎样修；迁移文档写明 0.1.x 的 settings.json 要删除或重写 | 用户：「直接报错表示参数错误，不做任何兼容检测」（D-14）。用内置默认运行不是兼容代码，只保证用户总有修复的路 | §8.2、§9.5 |

**第三轮核对**（T-S2-1…T-S3-8）。主会话定的原则：用最简单、最少新机制的改法收口；能用一个固定常量或一句明确规定解决的，就不加可配置项。
其余各条按复审建议改，见 §15。

| 核对条目 | 决定 | 依据 | 落点 |
|---|---|---|---|
| T-S2-1 | notify 单独计数、单独暂停派发，相当于 notify 自己的熔断，不影响数据通路；只声明 notify 的程序用 notify 做半开试探；notify 的时限改为固定常量，不借用 `decide_timeout_secs` | D-12 的用户决定（计划的记载：连续故障才熔断，冷却后试探）；完成通知「只发不等回信」（计划 W2c）；项目目标的 uninterrupted operation（AGENTS.md）：请求不能因为一条通知卡住而永久挂起 | §6.4、§10.1、§10.9、§10.11 |
| T-S2-2 | 滚动重启停掉实例之前，先用目标版本的 §7.5 管线预检（含 settings），不过就不停，并给出原因 | 无直接原话；依据计划 W1 对升级路径的要求「必须在改动任何东西之前失败，不能半途坏掉（实例失联、二进制已换）」，以及 install/restart.rs 模块文档的「升级是『不中断运行』使命里最脆弱的一环」 | §2.4、§5、§8.2、§9.5 |
| T-S2-3 | B0 的验收改为：AGENTS.md 与 CLAUDE.md 讲永久契约的那段逐字一致，列出 C3 冻结代码所在的全部文件；同一句补上「冻结的是断言，`use` 路径可以随代码移动」 | 用户：「从现在开始保持绝对兼容」（W2b，2026-10-11）：讲永久契约的那段必须一直指向全部冻结代码 | §6.6、B0 |
| T-S3-1 | 改正 `forward_default_unguarded` 的条件，让「默认 forward，adaptive 可选，convert 与 keys 是 required」的配置能报出来 | 用户：「对于无限重试依旧保持默认」 | §2.4、§10.9 |
| T-S3-2 | R-S2-4 的失败现场置 `halted = true` | 无直接原话；alpha.1 已发布的续作只拒绝「failed 且 halted」的现场（flow.rs:713-731），不置就会被反复接管 | §4.2、§4.3、§9.2 |
| T-S3-6 | 钉进 spec 的端口只在配置解析出的端口仍是 0 时生效；配置改成固定端口后以配置为准 | 无直接原话；R-S2-3 只在端口 0 时钉的理由（不让旧端口盖掉用户后来改的配置），对钉过之后的修改同样成立 | §4.2、§5 |

没有待用户拍板的问题。

---

## 14 实现顺序与切分

**通用纪律**：
- 改了对外行为的块，skill 在同一提交或紧随其后的提交里同步（记忆 aproxy-cli-skill-versioning）。
- 测试一律用隔离的 home，停实例只按端口。
- 两个变体都要通过 clippy `-D warnings`。

### 14.1 前置重构（W2 的实现从这里开始）

| 块 | 内容 | 依赖 | 验收 |
|---|---|---|---|
| P1 | R1：`tests/common` 的 TestHome。所有 CLI 子进程经它拉起，统一注入 `APROXY_HOME` 与 `APROXY_LANG=en`，清掉代理变量；守卫合一 | 无 | 全仓测试里没有绕过 `TestHome::cmd()` 直接拉起 aproxy 的地方（有 grep 检查）；Windows 与 ubuntu 全绿 |
| P2 | S1：proxy.rs 拆进 `src/proxy/`，抽出 `classify`，合并成单一的尝试循环 | P1 | 拆分前后测试数一致（用独立的 target 目录核对）；`classify` 有单测；源码里只剩一个尝试循环 |
| P3 | R2b：reqwest、axum 先升，toml、zip、sha2 随后；每个升级单独提交 | P2 | `cargo tree` 显示新版本；每个提交全量测试通过 |
| P4 | S2：平台原语移出 watchdog.rs；`home.rs`、`fsx.rs`；四处「拉起并等就绪」合一。home 的解析若移出 settings.rs，`tests/force_contract.rs` 的 `use aproxy::settings::{HOME_ENV, …}` 只改 `use` 行（R-S3-15）；比 B0 先落地时，同时改测试的文件头那句与 AGENTS.md、CLAUDE.md 的那半句（§6.6，T-S3-8 ③） | P1 | 四处调用同一个函数；start 有早退检测：子进程因配置错误退出时，1 秒内报错；force_contract 除 `use` 行与文件头那句外逐字不变并通过 |
| P5 | S3：操作层，lib 不打印、不 `process::exit` | P4 | lib 里没有 `println!` 与 `process::exit`（有 grep 检查）；CLI 只负责渲染 |

顺序：P1 → P2 与 P4 并行 → P3 跟在 P2 之后，P5 跟在 P4 之后。

### 14.2 契约块

| 块 | 内容 | 依赖 | 验收 |
|---|---|---|---|
| B0 | 新建 aproxy-ipc：码表（含 `details` 的键与类型，并导出 JSON）、`redact`、端点名与 `home_id`；C3 的代码整体从 `src/ledger.rs` 迁入一个文件：条目结构与写者、读者与它的义务（`terminate`、`remove_if_unchanged`）、启动标记推导（§6.6，T-S2-3）。`tests/force_contract.rs` 只改 `use` 行，断言逐字不变；文件头那句与 AGENTS.md、CLAUDE.md 讲永久契约的那段同步改（两份是独立副本，R-S3-15） | P1 | 依赖只有 serde、serde_json、tokio、sha2、钉版的 windows-sys，以及 unix 上的 libc；码唯一且是 snake_case，每个码都按族带有 HTTP 状态或退出码；渲染出的文档与仓库一致；force_contract 除 `use` 行与文件头那句外逐字不变并通过；AGENTS.md 与 CLAUDE.md 讲永久契约的那段逐字一致，列出 C3 冻结代码迁移后的全部文件，列出的文件都存在，末句写明「契约测试冻结的是断言，`use` 路径可以随代码移动」（T-S2-3） |
| B1 | 数据面的错误出口：§2.1、§2.2、§3、`request_id` | P2、P3、B0 | 每个能触发的自产 4xx/5xx 都有集成测试：体是 JSON、带两个头；源码里只有一个 `event: error` 构造函数；同一请求在头、体、日志里的 `request_id` 相同；真实 SDK 各解析一次，结果记进计划 |
| B2 | config.toml：§7 的键、严格解析、`resolve`、写回 | P3、B0 | §7.2 的样例能解析；任何未知键（含规则与 `[[format]]` 条目里的）都让启动失败，并指出文件与键；四层 `sources` 正确；写回保留注释；两个写者并发 `set` 不丢更新；`unset` 能删未知键 |
| B3 | settings.json：§8，含 D-16 的继承规则（§7.5） | B2 | 未知键报错并指出键路径；schema 2 被拒，提示出路；无效时 stop 与 status 仍能按端口工作，`settings set` 能逐项修复；`ui` 里任意内容都不报错。D-16：头表按名合并（只差大小写的算同名，配置优先）；最终 `base_url` 与默认不同时，默认 key 与密钥头不继承，`withheld` 列出它们，配置自己也没有 key 时报 `default_credential_withheld`；`https://API.example.com/` 与 `https://api.example.com` 算同一目标；`defaults` 有 `api_key` 却没有 `base_url` 时报 `config_missing_key`；`settings show` 对密钥给占位，占位写回不改原值；Windows 上 settings.json 的 DACL 只有当前用户、SYSTEM、Administrators。先设 `defaults.api_key` 被拒，报错提示先设 `defaults.base_url`，按这个顺序再设就成功（R-S3-10）；`defaults.proxy` 带 userinfo、配置写了不带 userinfo 的同一个代理时，用配置的值，不拼接 |
| B4 | 重试与超时：§7.3、K13、K14 | P2、B2 | 规则第一条命中、`max_attempts = 0` 豁免、按尝试序号计数；不读 Retry-After（上游给 `retry-after: 600` 时，第 2 次尝试仍立即发出）；首字节超时与读空闲超时各报各的码；`max_response_mb = 0` 表示不限 |
| B5 | 持久层与拉起：§4、§5 | P4、B0、B3 | `tests/contract_v0_2.rs` 钉住全部记录；读不懂的记录不被删除；两个平台上守护与看护者的命令行里都没有密钥；Windows 上只设大写 `HTTPS_PROXY` 时，restart 之后仍走代理；ubuntu 上对启动者的进程组发 SIGINT，实例仍在；restart 遇到写错的配置时不动旧实例；restore 遇到暂时缺失的配置时保留记录；`start && stop` 之后实例不会被拉回；启动要 9 秒的 format 不被判为就绪超时；端口为 0 的后台实例崩溃后，被拉回到同一个端口，run/ 里始终只有一份期望状态记录（R-S2-3）；这样钉过端口的配置改成固定端口之后，restart 与看护者都用新端口，旧端口的记录被删掉（T-S3-6）；没有配置文件、只靠 `defaults` 启动的实例能被 restore 拉回（R-S3-13）；restart 对快照里没有 `shutdown.keep_restore` 的旧实例发不带参数的 shutdown，之后记录仍在（R-S3-14） |
| B6 | IPC v1：§6.1–§6.4、§6.6 | B5、B1 | 每个 op 的请求、成功、每种错误都有 golden；未知参数回 `unsupported_arg`；模拟新旧客户端与服务端互连，多出的字段被忽略；`base_url` 带 userinfo 时快照里已打码；没有配置文件的实例，快照的 `instance.config_path` 为 null（T-S3-7）；四种 liveness 都能区分出来 |
| B7 | 访问控制：§1.6 | B6 | Windows 上读回管道的 DACL，只有当前用户与 SYSTEM；ubuntu 的 CI 里建第二个用户（runner 有免密 sudo），断言它连不上、root 能连上；home 为 0700；权限过宽的既有 home 在第一次写配置后被收紧；临时文件一创建就是 0600，rename 之前不可被他人读取（R-S3-12）；`body` 只在这一块之后出现在 `ops` 里 |
| B8 | 请求历史、`body`、`subscribe`、事件：§6.3、§6.5、§6.7 | P2、B1、B3、B6、B7（B3：已知密钥包括 settings `defaults` 里的） | 见下面的阈值 |
| B9 | 命令行：§9（含 install 的 `--json`、`--check`、A5 的四个字段，§4.3 第 7 条的拒绝，以及 §5 的预检） | P5、B2、B3、B5、B6 | §9.5 的每一格都有测试；每个命令的 `--json` 都有 golden；带 `RUST_LOG=info` 运行时 stdout 里没有日志行；`install --force --json` 退出 2；遍历命令树的帮助测试；以 `install --continue --handover-from` 接手的路径有测试；台账里登记为 instance、却没有实例记录的活进程在场时，普通安装在加锁与下载之前就失败（`install_unmanaged_instances`，T-S3-3）；模拟 alpha.1 的安装者交棒、同时 home 里有 alpha.1 启动的活实例时，续作在广播之前以 failed 结束，`halted` 为 true，`error_code` 为 `install_unmanaged_instances`，`last_error` 为英文，bin 与实例都没动；之后在这个 home 里执行 alpha.1 的 `aproxy stop all`，它拉起的续作不接管这个现场（R-S2-4、T-S3-2）；W2 写出的 `pending_restores` 恒为空（R-S3-11）；settings.json 无效、没有活实例时 install 照常完成并告警（R-S3-17）；settings.json 无效、有活实例时 install 以 `install_precheck_failed` 结束，退出码 3，`halted` 为 true，实例没被停、bin 没换（T-S2-2）；`halted` 的现场在场时，W2 的变更类命令不拉起续作（T-S3-2） |
| B10a | format 协议核心：hello（含 spawn）、`seq`、链与重试段、request、retry、response、heartbeat、fail、overrides、report、熔断 | P2、B1、B4 | format-echo 升级到协议 1，覆盖坏 `seq`、缺 hello、中途崩溃；链的顺序；配置违反「retry 在后段」时启动失败；没声明 retry 的程序成功之后，重试时收不到消息；熔断：连续 5 次失败之后返回 503，冷却后的试探成功就恢复；网络失败不触发熔断；心跳回信改不了 state |
| B10b | decide：`decide.success`、`reroute`、体上限 | B10a | 第一个表态者生效；`reroute:false` 时重用上一次的请求；decide 超时回落，并计入熔断 |
| B10c | stream：retry 的回放转换与 forward 的实时转换 | B10a | forward 下的流式转换端到端通过；中途失败时写出 `abort_tail`；流结束后 worker 的回收符合 §10.6 |
| B10d | 按请求选投递方式与 notify：§10.2 的 `request_body` 与摘要、§10.10、§10.11；删掉 config.rs:586-592 的互斥 | B10b、B10c | `delivery = "forward"` 与 `[[format]]` 同配能启动；两个程序先后给 `delivery` 时第一个生效；forward 请求遇上游 503、一个字节都没写出时，decide 回 retry 就升级，客户端只收到重试成功的响应，记录的 `upgraded` 为 true；2xx 的流中途断开不升级；只要摘要的程序收到正确的 SHA-256，它的 stdin 上没有正文（4 MiB 的请求体，统计写进管道的字节数）；一个从不读 stdin 的 notify 程序在场时，吞吐与 p99 仍满足 B8「没有订阅者」的阈值，`notify_dropped_total` 增长；forward 下经 stream 转换不攒块：上游每 100 毫秒发一块时，客户端在上游首块之后 200 毫秒内收到首块；§7.2 那种 adaptive 同时声明 request、decide、notify 的链，在 64 个并发连接下吞吐与 p99 满足 B8「没有订阅者」的阈值（R-S2-1）；notify 回信超时不改变数据通路的熔断计数；一个 notify 总是超时的程序，连续 5 次之后 notify 暂停派发、数据通路不再等它，冷却后放一条 notify 试探；只声明 notify 的程序熔断之后（含握手失败、起步即 open）能恢复；`decide_timeout_secs = 0` 时，卡住的 notify 在 2 秒后被换掉，排在后面的请求照常完成（T-S2-1）；forward 请求拿到「200 但内容是错误」时，记录与完成通知的 `outcome` 为 `upstream_error`，客户端收到的字节与上游逐字节一致（R-S3-8）；预算为 0 时，一个 200 MiB 的 forward 响应让常驻内存的增长不超过 32 MiB（T-S3-4）；被 403、413 拒绝的请求不发 notify（R-S3-7） |
| B11 | aproxy-format 接到协议 1，按角色拆成子命令（convert、keys、route、policy、adaptive、rewrite、heartbeat）与工具（init、check、try、translate），去掉 `run` | B10a–B10d | 每个角色单独挂进链都能握手，只声明自己用到的阶段与粒度；adaptive 只要摘要，测试断言它的管道里没有正文；§7.2 的三单元链端到端通过；adaptive 握手失败或熔断时，请求按默认的 retry 照常重试成功（R-S1-1）；`check` 拒绝 route 之后没声明 retry 的 convert、rewrite（R-S3-2）；转换的正确性，以及客户端重试时请求体是否逐字不变的实测，属于 W2c 的验收 |
| B12 | 文档、skill、记忆 | 随各块 | 码表与 CLI 写进 skill，SK3 的校验脚本进 CI；0.2.0 发布时按 §11 重写 ipc-v1-upgrade-contract |

**B8 的验收阈值**（loadgen 对本机的模拟上游，64 个并发连接，跑 30 秒；基线是 B8 之前的提交，同一台机器）：
- 没有订阅者：吞吐不低于基线的 95%，p99 延迟不高于基线的 110%。
- 一个订阅者：吞吐不低于基线的 90%，p99 延迟不高于基线的 120%。
- 长重试（S1-2）：`max_backoff_secs = 0`，上游拒绝连接，8 个并发请求，跑 10 分钟。
  - 第 1 分钟之后，常驻内存的增长不超过 32 MiB；
  - 每条记录序列化后不超过 64 KiB；
  - 订阅者对每条请求每秒最多收到 1 个 `request` 事件。
- 体的总量：以 64 MiB 的预算推入 200 MiB 的体之后，保留的体合计不超过 64 MiB，并且最旧的先被淘汰。
- 体整份保留：一个 10 MiB 的请求体分块读回后，与原文逐字节一致（已知密钥已替换）；预算 8 MiB 时，同一个体为 `not_retained`，`cause` 为 `exceeds_budget`。
- 分块读取：任何一块的应答行都不超过 16 MiB；读进行中的响应体时 `complete` 为 false，`total_bytes` 随之增长。
- 解码与替换（R-S2-2）：密钥被块边界切开时也被替换；gzip 与 br 的响应体以解码后的文本保留，其中的密钥被替换，`encoding` 照记；
  解码后超过预算的压缩体为 `not_retained`（`exceeds_budget`），解码失败的为 `decode_failed`。
- 边界（R-S3-4、R-S3-6）：`offset` 等于 `total_bytes` 时回空块；`limit` 为 0 或大于 1 MiB 时回 `bad_request`；取被省略的尝试的体回
  `no_such_part`；超过 64 KiB 的记录（含 `actions` 很长的报告）按 §6.5 的顺序处理之后不超过 64 KiB，并标 `truncated`（T-S3-8 ①）。

**并行编排**（总并发不超过 5）：
- 先做 P1，再并行 P2 与 P4；B0 可以与 P2、P4 并行。
- 数据面一条线：P3 → B1 → B4 → B10a → B10b、B10c → B10d → B11。
- 控制面一条线：B5 → B6 → B7 → B8。
- B2、B3 在 P3 之后；B9 最后；B12 贯穿始终。

---

## 15 修订记录（第二版）

依据：第二轮审查 `.agents/review/2026-10-12-w2-contracts-review.md`，以及用户决策 D-12…D-15。「采纳」指照审查的改法做；「改写」指方向相同但做法不同；「不采纳」附理由。

**S1**

| 编号 | 处理 |
|---|---|
| S1-1 | 采纳，并由 D-14 扩大：配置的一切未知键都报错（§1.1、§7.5、§8.2） |
| S1-2 | 采纳：`tries` 只保留前 3 加后 20；`decision` 并入每次尝试；`request` 事件限频；单条记录不超过 64 KiB；B8 加长重试用例（§6.5、§6.7、§14） |
| S1-3 | 前提随 D-14 消失：不再区分旧键，任何不认识的键都报错，也就无从漏列 |
| S1-4 | 采纳：只有 stop 能删期望状态记录；restore 与看护者遇到配置缺失时保留记录（`config_missing`）；restart 先预检，停止时用 `keep_restore`（§4.2、§5、§6.3） |

**S2**

| 编号 | 处理 |
|---|---|
| S2-1 | 采纳：只用 `setsid()` |
| S2-2 | 采纳：Windows 上记一个大写名、不分大小写查找；unix 上两种名字各记一条；样例按平台写 |
| S2-3 | 采纳：用 C4 同款锁串行化，临时名带纳秒并 create_new，rename 前 `sync_all`，Windows 上退避重试；哈希只用来发现外部写者；不再叫 CAS |
| S2-4 | 改写：先 bind 代理端口，再以 `starting` 应答控制端点，握手完成才 `serving`；就绪时限 30 秒；握手并行。没有选「先 bind 控制端点」：端口为 0 时，端点要等实际端口才能命名 |
| S2-5 | 采纳：期望状态记录与 shutdown 的删除同锁，收到 shutdown 之后不再写 |
| S2-6 | 采纳方案 ②：心跳回信不能改 state |
| S2-7 | 风险判断采纳；做法由用户改定为 D-16（§13.2）：上游与凭据可以进 `defaults`，但凭据只发给它所属的目标 |
| S2-8 | 采纳：home 本身 0700 或显式 DACL；install/state.json 不再存 spec，所以不含密钥 |
| S2-9 | 采纳：format 的 stderr、`details.reason`、IPC 返回的体、桌面桥接层都列进出口；format 消息与 state 永不输出 |
| S2-10 | 采纳：写明占位对象怎样对应原值，对不上就报 `secret_placeholder_unmatched` |
| S2-11 | 采纳：`--check`、`--json` 与 `--force` 冲突，并进 force_contract |
| S2-12 | 采纳：§2.4 加 IPC 到 CLI 的对应表；结局词加 `skipped` |
| S2-13 | 采纳：尝试明细改叫 `tries`；上游状态码统一叫 `upstream_status`，回给客户端的叫 `client_status` |
| S2-14 | 随 D-15 消失：不读 Retry-After、不加抖动，默认时序不变 |
| S2-15 | 采纳：`unset` 接受任何键；doctor 为每个未知键给出删除它的命令 |
| S2-16 | 采纳：(a) 前置重构写成 P1–P5；(b) C3 的读者迁入 aproxy-ipc；(c) B8 给出可证伪的阈值 |

**S3**

| 编号 | 处理 |
|---|---|
| 1 | 采纳：码表元数据允许「透传，否则 N」与「按子进程的码」 |
| 2 | 采纳：删去 6–9 的预留 |
| 3 | 采纳：注明强制安装退出 10；用户可见的 `--home` 已删 |
| 4 | 随 D-14 消失：`config_unknown_key` 只有 error 一个级别 |
| 5 | 采纳：`owner` 只取 `aproxy` 或 `unknown`；快照删去 `home` |
| 6 | 采纳：forward 请求的 `fail{retry:true}` 先问 decide 要不要升级，不升级回 502 `format_failed`；`retry:false` 两种模式相同；可选程序的 `fail` 照样执行 |
| 7 | 采纳：forward 请求不升级时透传的非 2xx 不加头 |
| 8 | 采纳：JSON 体压成一行，作为 `event: error` 写出 |
| 9 | 采纳：写明字节偏移，已与 announce.rs 核对 |
| 10 | 采纳：目录名带启动标记的短哈希 |
| 11 | 采纳：一个端口一个实例，写成显式约束 |
| 12 | 采纳：`gc_paused` 进 status 与 doctor |
| 13 | 采纳：`stop all` 只删本 schema 的记录 |
| 14 | 采纳：spec 删掉 `foreground` |
| 15 | 采纳：拉起方立即删 spec 文件 |
| 16 | 采纳：bind 之后用实际端口重写台账 |
| 17 | 采纳：体改用单独的 `body` op，一次一个（参数用 `attempt`，与 §1.3 一致）；后按 D-13 的细化改为分块读取，应答行不再需要 32 MiB 的特例 |
| 18 | 采纳：`decision` 并入每次尝试 |
| 19 | 第二版采纳（截断时保留开头与结尾），之后按 D-13 的细化撤回：体整份保留、不截断，尺寸键只留总量 |
| 20 | 采纳：写全 `api_key` 的两个头，以及头覆盖在请求链之前应用 |
| 21 | 采纳：单列「只能按请求覆盖的键」 |
| 22 | 采纳：按尝试序号计数，规则从 1 起编号；Q1 的配方随 D-12 删除 |
| 23 | 采纳：`sources` 加 `env` |
| 24 | 采纳：文档写明，doctor 提示 |
| 25 | 改写：缺 schema 按 1 读；更高的 schema 按 D-14 报错，不再逐字段读 |
| 26 | 采纳：config 收 TOML，settings 收 JSON；以 `"` 开头时按字符串字面量解析 |
| 27 | 采纳：取第一个不以 `-` 开头的参数 |
| 28 | 采纳：两处各写一句 |
| 29 | 采纳：起不来、超时退 1；协议、阶段不符退 3 |
| 30 | 采纳：decide 收到的体上限 1 MiB，超出时保留开头 64 KiB 与结尾。这只管经管道交给 decide 的那份，与请求历史无关 |
| 31 | 采纳：写明回收规则与进程数 |
| 32 | 采纳：每拍只带 `client`，不带体 |
| 33 | 改写：notify 回空回信、aproxy 不等它；admit 的形状按用户要求留在定稿里，仍为 R 级；notify 已按用户追加提前到 0.2.0 实现（§10.11） |
| 34 | ① spawn 也握手，没有困难；② 阶段不符一律报错，只有起不来与超时对可选程序降级；③ decide 加 `reroute`，并约束 retry 程序排在后段；④ 列明退出码 2 |
| 35 | 采纳：服务端放行 root |
| 36 | 采纳：心跳写入放进专用线程 |
| 37 | 采纳：B10 拆成三块；补上依赖；B6 的用例改为 userinfo；B7 的跨用户测试必做；A5 归 B9 |

**第 4 节「建议删减或推迟」**：
- 1–5、7–12 全部采纳。
- 第 3 项的例外：按 D-13 保留总量上限 `history.body_budget_mb`。
- 第 6 项（admit 与 notify 改为草案）不采纳：用户要求把它们的形状写进 0.2.0 的定稿。admit 仍为 R 级，notify 按用户追加提前实现（§10.11），两者都吸收了 S3-33。
- 删掉的各项以后怎样补回：op 与参数用 `ops`、`features` 声明；配置键对旧版本是拒绝启动；format 的能力靠 hello 协商。

**第 5 节（对 Q1–Q3 的意见）**：由用户的 D-12、D-13、D-14 取代（§13.1）。

**第 6 节（0.1.x 的逐键迁移表）**：
- 程序里不识别旧键（D-14）。
- 这张表可供 W7 写迁移文档时参考；运行期残留怎样手动清理，也由迁移文档交代（§4.3 第 7 条）。

**第 7 节（缺口）**：
- 1 置顶：界面侧的功能，存在 `<home>/desktop/`。
- 2 今日救回：界面侧的统计。
- 3 更新信息：`install --check` 加 `published_at` 与 `url`；多语言的发布说明归 W5、W7。
- 4 安装进度：NDJSON 加 `plan` 行；相位怎样映射到界面的步骤由 W5 定。
- 5 扩展授权与开机自启：放 `<home>/desktop/`。
- 6 短序号：加 `request_seq`。
- 7 转换器信息：0.2.0 不提供，以后在 hello 回信里加性地加。
- 8 实例增减：界面轮询枚举。
- 9 拉起用的可执行文件：界面用 `<home>/bin/aproxy`。
- 10 前端码表：B0 导出 JSON。
- 11 查看请求历史的 CLI：以后再加。
- 12 skill 清单：§4.2 定了 schema 与读者规则，退出码见 §9.5。
- 13 `details` 键的类型：登记进码表。

**第 8、9 节**：
- BREAKAWAY 回退时记告警；
- `sent.url` 按 URL 规则打码；
- 记录在入库时打码；
- 桥接层只暴露三类写操作；
- 放行 root。
这些都已写入 §1.5、§1.6、§5。

**第 10 节**：并入 §14。

**附录 A**：现状行号已按 73b7863 核对更新。其中 `last_error` 的现状改为 `{message, at}` 对象。

**用户追加（2026-10-12）**

依据：team-lead 转达的、用户已同意的设计，原文在计划 W2c（9e39930）的「按角色拆成子命令」与「先仅转发、失败多次再无限重试」两段。
D-12…D-15 不变，与这批设计共存。

| 追加 | 处理 |
|---|---|
| 1 按请求选模式 | request 回信的 `delivery`（命名见下一节），只在 request 阶段有效；链上第一个表态的生效，与 decide 的 `decision`、`reroute` 是同一条规则，理由在 §10.10 |
| 2 请求内升级 | forward 请求在写出任何字节（含响应头）之前失败，就问一次 decide；回 retry 当场升级，此后与 retry 完全一样，这次尝试算第 1 次（§10.5、§10.10） |
| 3 forward 与 format 共存 | 删掉 config.rs:586-592 的互斥，以及第二版的「仅转发只允许 request 与 stream」；何时先读完请求体写在 §10.10 |
| 4 notify 提前实现 | §10.11：0.2.0 只有 `request_end` 一种事件；只发不等，请求路径优先，有界队列满了丢最旧的并计数；attempt 事件仍为 R |
| 5 数据粒度 | §10.2 的 `request_body`：按阶段取 none、digest、full；digest 是原始字节的 SHA-256 |
| 6 aproxy-format 按角色拆分 | §10.14 列出各角色用到的阶段与粒度；§7.2 的样例改为三个单元的链；新增 B10d，B11 按新结构验收 |
| 与 D-12…D-15 共存 | 熔断按程序生效：adaptive 熔断时它那一层被跳过，请求按配置的默认投递方式走（第二轮复审之后，推荐配方的默认是 retry，见下方 R-S1-1）；notify 熔断期间丢弃并计数（§10.9）。adaptive 与 policy 建议 `required = false` |

为补全这批设计，本稿另做了几处决定，都可以推翻：
- 链上有 decide 程序时，forward 请求也先读完请求体：否则 decide 叫升级时无体可重发。
- forward 请求成功时不调用 decide：`decide.success` 只对 retry 请求生效，否则就得缓冲响应。
- forward 请求经过声明 response 的程序时，响应在那一层攒齐，失去真流式；doctor 提示改用 stream。
- notify 只许 persistent 程序声明；notify 保留空回信，维持一问一答。
- 跨请求的内存状态只在 `pool_max = 1` 时可靠，adaptive 据此配置。第二轮复审补充：这个进程仍会被换掉，程序要能承受重启，
  adaptive 另配 `worker_idle_secs = 0`（§10.3，R-S2-1）。
- 命名：按请求的投递方式叫 `delivery`，快照的行为摘要叫 `behavior`，`[[format]]` 的进程方式叫 `process`；`mode` 不作任何字段名（§1.3）。
- forward 请求的 `fail{retry:false}` 与 retry 请求相同。第二版写成「两种 fail 都回 502」，超出了 S3-6 的范围。

**第二版之后：D-16、D-13 的细化与命名（2026-10-12）**

依据：team-lead 对第二版的审查意见，以及用户随后的决定 D-16（计划 53afebd）。

| 事项 | 处理 |
|---|---|
| Q4 改定为 D-16 | 改写 §7.1、§7.5、§8、§13.2。`defaults` 收上游、key、代理与两张头表，不收 `listen_addr`；头表按名合并；凭据只发给它所属的目标，没继承的经 `withheld` 与 `default_credential_withheld` 让用户看得见；settings.json 因此含密钥，按 §1.5、§1.6 保护。「同一目标」按 base_url 的规范化比较：在现行的去掉末尾斜杠之外，加上 scheme 与主机名转小写、去掉默认端口，不比 userinfo。第二版 Q4 里「settings.json 不含密钥」「排除不会让任何现有用法消失」两句随之删除 |
| D-13 的细化 | 去掉 4 MiB 的单体上限与头尾截断：体整份保留，只受 `history.body_budget_mb` 约束；单个体比整个预算还大才不保留（`not_retained`，`cause` 说明原因）。`body` 改为分块读取（`offset`、`limit`，单块最多 1 MiB，结果带 `total_bytes`、`more`、`complete`），删掉 32 MiB 的行长特例（§6.1、§6.3、§6.5） |
| 命名 | `mode` 曾同时指快照的行为对象、`[[format]]` 的进程方式与按请求的投递方式，改为 `behavior`、`process`、`delivery`。config 的 `forward_only` 一并改为 `delivery`，按 D-14 不留旧名（§1.3） |
| `not_found` 的原因 | 区分 `unknown_request`（请求已不在记录窗口里）与体的 `evicted`、`not_retained`（§2.3） |

**第二轮复审（R-S1-1…R-S3-18，2026-10-12）**

依据：复审 `.agents/review/2026-10-12-w2-contracts-review.md` 第 665–964 行，以及主会话按用户授权的决定（§13.3）。
「照建议」指按复审的改法做；与建议不同的，写明理由。

| 编号 | 处理 |
|---|---|
| R-S1-1 | 按主会话的决定（即复审的推荐）：§7.2 的样例与 `init` 沿用默认 retry，adaptive 对没失败过的请求回 forward；改正 §10.9、§10.14 里「走默认模式与静态兜底」的说法；不加 A 级覆盖。另按主会话要求加 doctor 告警 `forward_default_unguarded`（§2.4）。B11 加 adaptive 不可用时的用例。告警的条件第三轮改正，见 T-S3-1 |
| R-S2-1 | 主会话把「同一 worker 上数据通路优先」与「notify 单独开 worker」交由本稿选，选前者：adaptive 在 request 阶段查、在 notify 时记的是同一张表，拆到两个进程里判断就看不到结果。占用、计时、派发写进 §10.1；notify 的时限等于 `decide_timeout_secs`（照建议）。留下的余量照实写明：notify 一旦写出，之后到的数据通路消息要等它这一条，最长是它的时限；彻底消除要靠多条在途消息。超出建议的三处：notify 回信超时或不合法时换掉 worker（不然后面的回信会错位），但不计入熔断；notify 的合法回信也不清零熔断计数（不然一个数据通路坏掉、却能应答通知的程序永远熔断不了）；半开试探只用数据通路的消息。跨请求状态与 `worker_idle_secs = 0` 照建议；B10d 加共用 worker 的链的吞吐用例；多条在途消息列为以后加性补的。notify 的时限与熔断第三轮已改，见 T-S2-1 |
| R-S2-2 | 照主会话的决定与复审的建议：存解码后的字节并记 `encoding`；解码超出预算就停（`exceeds_budget`）；解码失败为 `decode_failed`；按流替换，尾巴为最长密钥减一；口径写明（§6.5）。B8 加跨块与 gzip、br 的用例 |
| R-S2-3 | 按主会话的决定，选「把实际端口钉进 spec」。只在配置的端口为 0 时钉：一律钉的话，用户以后改了配置里的端口，restart 也会被旧端口盖掉。钉的位置第三轮改为 `pinned_port`，见 T-S3-6 |
| R-S2-4 | 照建议：普通 install 与续作都在广播之前检查，以 failed、`install_unmanaged_instances` 与英文 `last_error` 结束，释放锁与宣告；§4.2 补「有 skill 目录、没有清单」；B9 加交棒叠加活实例的用例。第三轮补上入口那次检查与 `halted`，见 T-S3-2、T-S3-3 |
| R-S3-1 | 照主会话「与现行退避一致」：样例改为第 6 次尝试失败、`fallback.delay_ms` 为 20000（retry.rs:14-32 的第 6 次重试）。另把上游的 `retry-after` 从 20 改为 60，免得 20 秒与 20000 碰巧相同、被读成 aproxy 尊重了 Retry-After；format 的回信原本就是 60000，现在正好表明尊重它的是 format，report 的 summary 随之写明 |
| R-S3-2 | 照建议：§7.4 与 §10.14 写明 route 之后的 convert、rewrite 也声明 retry；`init` 照此生成，`check` 照此校验 |
| R-S3-3 | 照建议：以 §10.10 为准改 §10.2；notify 的摘要边转发边算，请求体不完整时为 null |
| R-S3-4 | 照建议：写明截短顺序（旧尝试的 reports、summary、path 与 user_agent），标 `truncated`；截短长度为 E 级常量。第三轮改为三步，见 T-S3-8 ① |
| R-S3-5 | 照建议：被省略的尝试的体随之丢弃，取它们回 `no_such_part` |
| R-S3-6 | 照建议：`offset` 等于 `total_bytes` 回空块；`limit` 为 0 或大于 1 MiB 回 `bad_request`；复制途中变成 `not_retained` 的，下一块回 `not_found` 并带 `cause` |
| R-S3-7 | ① 照建议，进链之前就被拒绝的请求不发；② 选按字节设上限（16 MiB，`state` 计入），而不是「发送时再取 `state`」：等着发送的通知总要留着 `state`，只有字节上限才真正约束内存；③ 改为「上限」，写明是 E 级常量 |
| R-S3-8 | 按主会话的决定：在历史副本上被动判定，体没入库时用旁路副本，解码上限沿用现行检查路径的 8 MiB；这次尝试记 `upstream_error_body`，记录与通知的 `outcome` 为 `upstream_error`、`code` 为 `upstream_failed`。判定的做法第三轮改为与 retry 请求现行的完全相同，见 T-S3-4 |
| R-S3-9 | ①②⑤ 写进 §7.5，并由协议文档写明；② 的 doctor 提示定为 `default_header_shared`，为此加了诊断级别 `info`（只影响显示，不影响退出码）。③ 按主会话的决定，URL 内嵌的 userinfo 随 URL 值整体以配置为准、不拼接；没有照建议列进 `withheld`，因为这是普通的上层覆盖，不是因目标不同而扣下的凭据。④ 在 §13.2 标为细化，按主会话的决定不再请用户过目。⑤ 保持连路径比较，理由写进 §7.5 |
| R-S3-10 | 按主会话的决定保留报错；写入时的报错直接说「先设 defaults.base_url」，文档与 skill 写明顺序；B3 加这个顺序的用例 |
| R-S3-11 | 照建议：W2 写出的 `pending_restores` 恒为空，只为旧读者保留，W2 不读 |
| R-S3-12 | 照建议：install 与第一次写配置时收紧属于当前用户的既有 home；临时文件以最终权限创建；B7 加用例 |
| R-S3-13 | 按主会话的决定：spec 的 `config_path` 可为 null，restore 与看护者不检查它；B5 加用例 |
| R-S3-14 | 照建议：先查 `features`，没有 `shutdown.keep_restore` 就发不带参数的 shutdown，退出后回写记录 |
| R-S3-15 | 按主会话的决定：`use` 路径不算契约，B0 与 P4 只改 `use` 行，断言逐字不变，文件头那句同步；AGENTS.md 与 CLAUDE.md 一起改。B0 的验收第三轮改写，见 T-S2-3 |
| R-S3-16 | 照建议：在 §13.3 与 §10.9 标为对 D-12 的细化 |
| R-S3-17 | 按主会话的决定：install 与 stop、status 一样，settings.json 读不了时用内置默认并告警；start 的报错说清怎样修；迁移文档写明 0.1.x 的 settings.json 要删除或重写。§9.5 里 install 的退出码 3 随之去掉；第三轮又以「预检不通过」的含义加回，见 T-S2-2 |
| R-S3-18 | ① 改为「带 `x-aproxy-error` 的非 2xx」；② 改为按条目名引用 TODO；③ 稿首补上 D-16、W2c 与复审；④ 样例里的 report 文本改了，adaptive 现在回 forward（随 R-S1-1）；⑤ 主会话已改计划；⑥ B8 的依赖加上 B3 |

§12 随之改了两处 TODO 引用，在 K5 一行注明全局 `--home` 没有采纳，并在「不在 0.2.0 做的」里加了「一个 worker 上多条在途消息」。

**第三轮核对（T-S2-1…T-S3-8，2026-10-12）**

依据：`.agents/review/2026-10-12-w2-contracts-review.md` 第 968–1150 行，以及主会话的决定与原则（§13.3）：用最简单、最少新机制的改法收口。
「照建议」指按核对的改法做；与建议不同的，写明理由。

| 编号 | 处理 |
|---|---|
| T-S2-1 | 照建议与主会话的决定。notify 有自己的熔断器（§10.9），沿用 `breaker_failures` 与 `breaker_cooldown_secs`，不加配置键：超时、回信不合法、起不来、写不进 stdin、进程退出都计入；熔断时只暂停派发 notify，不杀 worker、不影响数据通路；冷却后用 notify 试探。只声明 notify 的程序只有这一个熔断器起作用，握手失败时由它以 open 起步，所以能恢复。时限固定 2 秒（E 级，§10.11）。与建议不同的两处：可见性用 `formats[].notify_breaker`（形状同 `breaker`），不另加 `notify_failed_total`，因为机制就是一个熔断器，沿用现成的形状，界面还能看到「暂停中」；notify 的进程级失败原先计入数据通路的熔断器，现在只计入 notify 的，因为主会话要的是「不影响数据通路」，程序真坏了，数据通路的下一条消息自己就会数到。B10d 加了对应的用例 |
| T-S2-2 | 照建议与主会话的决定，位置更早：预检放在广播之前，由执行续作的目标版本对每个要滚动重启的实例做；任何一个不过就什么都不动（不广播、不换 bin、不停实例），以 `install_precheck_failed`（退出码 3，带端口与诊断）加 `halted` 结束（§5）。没放在「停每个实例之前」：广播里的 ACK 收敛本身就可能重启实例；换了 bin 之后再失败，又会留下「新 bin、旧实例」的现场。§2.4、§8.2、§9.5 同步；B9 加用例 |
| T-S2-3 | 照建议与主会话的决定。C3 的代码整体迁进 aproxy-ipc 的一个文件，写者也迁，读写两边共用同一份启动标记推导；退出前删自己条目、再 `process::exit` 的便利函数按 P5 留在二进制一侧（§6.6）。B0 的验收改为：AGENTS.md 与 CLAUDE.md 讲永久契约的那段逐字一致，列出迁移后的全部文件且都存在，末句补上「契约测试冻结的是断言，`use` 路径可以随代码移动」 |
| T-S3-1 | 照建议：条件改为「默认 forward，且有 decide 程序，或有可选的 request 程序」；required 的 request 程序不算，它不可用时请求以 503 结束，看得见；一个 format 都没有时不告警（§2.4、§10.9） |
| T-S3-2 | 照建议：失败现场置 `halted = true`；§4.2 写明 `halted` 的语义（alpha.1 已是如此）；§9.2 第 6 步写明 W2 的自动续作遇到它不拉起；B9 加「alpha.1 的 `stop all` 拉起的续作不接管」的用例 |
| T-S3-3 | 照建议：普通 install 在入口（加锁与下载之前）查一次，查到就报错退出，什么都没写；所有路径在广播之前再查一次（§4.3 第 7 条、§5）；B9 改为「加锁与下载之前就失败」 |
| T-S3-4 | 照建议，写法更简单：不再分「历史副本」与「旁路副本」，判定一律与 retry 请求现行的做法相同（1 MiB 以内留副本，解码后判定；超出就用现行的增量行扫描，只看原始字节），与历史是否保留了体无关（§10.10）。B10d 加大响应的内存用例 |
| T-S3-5 | ①②③ 照建议：`client_status` 定义为已经写给客户端的状态码，样例改为 200（§1.3）；顶层 `reports` 补上 adaptive 的那份；字节数的口径写明（§6.5）。④ decide 样例改用 `-44`，不用建议的 `-43`：`-43` 已被 §3 的 SSE 终态样例用作另一条时间线。同类问题一并改了：§3 的 HTTP 错误样例（第 3 次尝试后 `retry_exhausted`）改用 `-41`；§10.6 的流样例原是 `-42/1` 正常结束、`-42/2` 中断，与「第 1 次 529、第 2 次成功」对不上，改为 `-42/2` 正常结束、另一个请求 `-45/1` 中断；RequestDetail 样例补上进行中的第 2 次尝试，并写明进行中的尝试 `response` 与 `error` 都为 null（原规则「没有响应就给 error」不适用于还没结束的尝试）；§10.5 写明哪些样例讲同一个请求 |
| T-S3-6 | 照建议，取「单独记一个字段」：spec 加 `pinned_port`，不再写进 `overrides`；解析出的端口仍是 0 才用它（§4.2、§5）。连带补了一条规则：用期望状态记录拉起的实例落在了别的端口（配置里的端口改了），新实例就绪之后，拉起方删掉旧记录；否则看护者会一直按旧记录去拉一个已经搬走的实例。这个缺口对固定端口的配置本来就有，T-S3-6 让钉过的端口也走到这里。§0 与 §4.3 第 2 条的「只有 stop 能删」随之注明这个例外；B5 加用例 |
| T-S3-7 | 照建议：实例记录与快照的 `config_path` 同样可以是 null（§4.2）；B6 加一例 |
| T-S3-8 | ① 换了更简单的做法：截短改为三步，即删旧尝试的 reports、截短 `path`、`user_agent`、`model`、仍超出就删掉顶层 reports；不去逐个截 `actions` 里的字符串，删整份报告就能收住上限，实现也简单。② 写明 info 不计入 `summary`，不加 `infos` 字段。③ 写明 P4、B0 谁先落地，谁就同时改测试的文件头与 AGENTS.md、CLAUDE.md 的那半句（§6.6、P4）。④ 熔断那一刻已在排队的消息，与新到的一样按熔断表处理（§10.9）。⑤ 协议文档写明「同一个摘要再次出现」可作补充信号（§10.11） |

核对里 R-S3-9 ③ 的可选补充（doctor 提示「配置的代理与默认代理是同一个，却没带默认值里的 userinfo」）没有做：按本轮的原则，不加检查项。
§11 随之加上 `pinned_port`、被接替的记录、install.state 的 `halted`、install 在广播之前的检查与预检，以及 notify 的固定时限。

**审查之外的自查**：
- **安装的续作入口**：第一版把 `install --continue --handover-from` 改名为 `__install-resume`，第二版撤回。
  install/flow.rs:12-18 已声明它是永久接口：早交接由旧版本的安装者以这个命令行拉起目标版本，alpha.1 也这样做；
  改名会让从 alpha.1 起的普通升级在交棒处失败（§5）。
- **W2 之前的运行期记录**：新版本不读它们；普通 install 在入口、所有路径在广播之前检查，遇到这些版本启动的活实例就以失败结束（R-S2-4、T-S3-3），迁移走强制安装（§4.3 第 7 条）。
  旧看护者按台账结束，不读看护者记录（§5）。
- **期望状态记录不归清理管**：第一版「写者已死就清理」会删掉崩溃实例的记录，正与 D-8 冲突（§4.3 第 2 条）。
- **端口 0**：S2-4 的方案 ②「先 bind 控制端点」与端口 0 冲突，改为先 bind 代理端口（§5）。
- **命名**：请求记录里的序号叫 `request_seq`，与事件和 format 消息的 `seq` 区分（§1.3）；`body` op 的尝试参数叫 `attempt`。
- **0 的含义**：补上 `watchdog.stale_after_cycles`（不许为 0）与 `instances.idle_threshold_secs`（字面值，§1.2）。
- **样例**：§6.5 请求记录样例的 `attempts` 由 2 改为 1：样例里只进行了 1 次尝试，第 2 次还在等待。
  第二轮复审时自查又发现：第 1 次尝试的决定已改成 adaptive 的升级、`delay_ms` 为 0（现行退避的第 1 次重试立即发出），
  `phase` 却仍是 `backoff`、`next_attempt_at` 在 5 秒之后，自相矛盾。改为第 2 次尝试进行中：`phase` 为 `attempting`，
  `next_attempt_at` 为 null，`attempts` 为 2，`tries` 里加上进行中的第 2 次（`decision` 为 null）。
  §6.4 快照样例的 `behavior.delivery` 随 R-S1-1 改为 `retry`：原来的 forward 加上可选的 adaptive，正是 `forward_default_unguarded` 告警的配置。
- **行号**：按 73b7863 重核，改正了第一版的 daemon.rs:156、config.rs:854、config.rs:941-947、proxy.rs:1198-1209 等。
