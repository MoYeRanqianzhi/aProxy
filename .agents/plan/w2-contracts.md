# W2 契约定稿（0.2.0）

> 状态：设计稿（2026-10-12），待用户整体过目。只读代码写成，没有改代码；现状行号对应 `ebb4572`。
> 依据：计划 `release-0.2.0.md` 的 W2、W2b、W2c、W3、W4、W5 节；审查综合稿与四份报告
> （`.agents/review/2026-10-11-0.2.0-audit/`）；记忆 0.2.0-direction、transform-format、bounded-retry-paths、
> random-log-names、ipc-v1-upgrade-contract、features-are-config。
> 怎么读：§0 一页摘要；§1 总则对所有契约生效；§2–§10 每节写「现状 / 设计 / 为什么」；§11 冻结清单；
> §12 审查对账；§13 需要用户拍板的问题；§14 实现顺序与切分。
> 不改 W2b 的 C1–C8，只与之协调（接缝见 §4.1、§5、§9.2）。不重新讨论用户已拍板的 D-1…D-9 与 format 定稿。
> 编号约定：C1–C8 一律指 W2b 的冻结项；综合稿 §1.4 的正确性条目写作「综C1」…「综C13」，以免混淆。

---

## 0 一页摘要

最重要的决定：
1. **冻结分四级**：C1–C8 绝对冻结且不新增；本稿的全部契约「只增不改」，靠六条机制成立（§1.1）。
2. **错误码单源**（新 crate `aproxy-ipc` 的 `code` 模块）：snake_case；HTTP 写 `error.type = "aproxy_<code>"`，
   IPC、CLI、日志、format 用裸码；每个终态码有固定 HTTP 状态，每个操作码有固定退出码（§2）。
3. **HTTP 自产错误一律 JSON** 加 `x-aproxy-error` 与 `x-aproxy-request-id`；放弃重试后透传的上游响应也带这两个头；
   SSE 终态是 `event: error` 加同一个 JSON（§3）。
4. **home 是唯一的根**：删 `APROXY_RUN_DIR`，加 `--home`；`run/` 平铺、kind 后缀、`schema`；读者跳过读不懂的记录，
   绝不删除；读路径没有副作用（§4）。
5. **拉起改为结构化规格**：配置路径、覆盖项、代理环境变量与 PATH；内部入口改为 `__serve`、`__watchdog`、
   `__install-resume`；任何命令行里都不再出现密钥（§5）。
6. **IPC v1 定稿**：未知参数报错、错误带 `details`、`ops` 与 `features` 声明能力；新增只读的 requests、request、
   config、subscribe、logs；请求与状态的事件只有「整条记录」与「整份快照」两类；管道显式 DACL，socket 0600 并核对 peer uid（§6）。
7. **config.toml**：核心键保持平铺；新增 `[timeouts]`、`[keepalive]`、`[retry]` 与 `[[retry.rules]]`、`[history]`、
   `[[format]]`；`max_body_mb`、`spool_limit_mb` 改名为 `max_request_mb`、`max_response_mb`；「0」按键的种类统一含义（§7）。
8. **settings.json schema 1** 按节重组；逐字段容错，保留未知字段，CAS 写入（§8）。
9. **CLI**：退出码 0–5（6–9、13–19 预留，10–12 归 C7）；查询与操作命令都带 `--json`；stdout 只放主输出；
   `logs` 默认不跟随（§9）。
10. **format 协议 1**：hello 握手（版本、阶段、能力）；`seq` 必须回显；洋葱链；state 按程序隔离；回信省略即不变；
    `fail` 结构、按请求覆盖、`report`；admit、notify、`stream.interleave` 只定形状（§10）。

需要用户拍板（§13）：
- **Q1** 规则表加 `codes` 匹配器，让确定性的 format 故障与网络故障也能封顶。
- **Q2** 请求体与响应体默认在内存里保留，供界面查看。
- **Q3** 0.1.x 旧键里「承载行为」的四个，是告警还是拒绝启动。

与 W2b 的接缝：
- unix 的安装宣告挪进 `<home>/install/announce`，C4 不再有例外（alpha.1 已落地，§4.1）。
- 台账与实例记录分开（G3）。
- `--version` 先于语言判定短路（G2，§9.2）。
- 通用退出码只用 0–5（G5）。

---

## 1 总则

### 1.1 冻结层级与「只增」机制

**现状**：计划「目标」一节要求 IPC、布局、配置、CLI、错误体、信封都长期稳定，W2b 却把它们列为「可以随意演进」（G4）。

**设计**：分四级。

| 级 | 含义 | 范围 |
|---|---|---|
| F 绝对冻结 | 永不改动；JSON 只许加可选字段 | 只有 W2b 的 C1–C8。本稿不新增 F 项 |
| A 只增不改 | 可以加字段、键、op、码、事件、取值；不删、不改名、不改类型或语义 | 本稿定下的全部公开契约，§11 逐项列出 |
| E 可演进 | 随时可改 | 给人看的文字（i18n 目录、各处的 `message`）；日志行里除 `code=`、`request_id=` 之外的部分；内部实现与调参常量 |
| R 预留 | 名字与形状现在占住，以后实现；实现时必须按这里写的形状 | 见 §11 |

「只增」靠下面六条成立，缺任何一条都会退化成「改了就坏」：
1. **读者容忍**。所有读者忽略未知的字段、键与事件。会整份写回的写者（settings.json、config.toml）要保留自己不认识的内容（§7.6、§8.3）。
   记录只由属主整份写，不需要合并。
2. **开放词表**。枚举一律用字符串取值；每个词表都在本稿写明读者的兜底：读作 `unknown`、按「不在正常服务」处理，或者显示 `label`。
3. **能力先声明后使用**。IPC 快照里有 `ops` 与 `features`；format 的 hello 里双方各自列出 `stages` 与 `features`。
   调用方先查，查不到就不用。
4. **版本号只在不兼容时递增**，包括 IPC 的 `v`、format 协议的 `v`、记录与 `--json` 文档的 `schema`。读者遇到更高版本时跳过并报告，
   绝不删除；共享文件（settings.json）的写者遇到更高 schema 时拒绝写。
5. **码只增**。错误码、动作类型、相位词表都只增；`message` 不是契约。
6. **用契约测试钉住样本**。每个形状都有 golden JSON，放在 aproxy-ipc 与 `tests/contract_v0_2.rs`。改样本就是破坏契约，测试注释写明这一点。

确实需要破坏时，另立并行的新版本（`v:2`、`schema:2`、协议 2），旧版本继续支持至少一个大版本。最后的兜底是强制安装：
C1–C8 保证任何版本都能装上任何版本。

**为什么**：这是 G4 的统一表述。
- 否决「全部绝对冻结」：成本过高，与 W2b「冻结得越少越好」的原则相悖。
- 否决「C1–C8 以外可以随意演进」：界面、agent、跨版本的看护者与安装者都依赖这些形状。

### 1.2 共同约定

- **命名**：JSON 字段、配置键、码、枚举值一律用 snake_case 英文。
- **时间与数量**：机器可读 JSON 里的时刻是 Unix 毫秒整数，字段名以 `_at` 结尾；时长以 `_ms` 结尾；字节数以 `_bytes` 结尾；单调计数以 `_total` 结尾。
- **单位**：人手写的配置用人习惯的单位：`_secs`、`_mb`（实为 MiB）、`_kb`（实为 KiB）。文档如实写 MiB，不为此改名（DP-14）。
- **版本字段**：
  - 行协议（IPC、format）用 `v`；文档与记录用 `schema`。
  - C3 的 `ledger` 与 C6 的 `manifest` 是 F 级的既成名字，不动。
  - `version` 字段是不带 `v` 的 semver；`v<semver>` 只出现在 C5 的输出与 tag 里。
- **可选性**：
  - 本稿样例里出现的字段，0.2.0 的写者都必须写出；可以为空的字段在样例里写成 null，或注明「没有就省略」；形状描述里带 `?` 的字段可以省略。
  - 以后新增的字段，对读者一律是可选的。
  - 类型以样例为准：整数就是整数，不用字符串表示数字。
- **进程身份**：`pid` 加 `start`，`start` 是 C3 的启动标记字符串（`w:…`、`l:…`、`m:…`）。它取代 `InstanceRecord.process_start: u64`
  （daemon.rs:157）与 `WatchdogClaim.created_at_process`（watchdog.rs:35），也顺带解决 Linux 锚点跨重启不唯一的问题（CP-22）。
- **「0」的含义按键的种类固定**。现状里 `spool_limit_mb = 0` 被当成 1 MiB（proxy.rs:3086 的 `.max(1)`），与其余字段相反（K14）。

| 种类 | 0 表示 | 键 |
|---|---|---|
| 超时、等待上限 | 不限 | `timeouts.*`；format 的 `timeout_secs`、`decide_timeout_secs`、`worker_idle_secs`；`watchdog.idle_exit_secs` |
| 大小、数量上限 | 不限 | `max_request_mb`、`max_response_mb`、`history.body_max_kb`、`logs.rotate_mb`、format 的 `pool_max`、规则的 `max_attempts` |
| 间隔 | 关闭 | `keepalive.interval_secs` |
| 保留量 | 不保留 | `history.requests`、`history.body_budget_mb` |
| 例外 | 不等待（立即重试） | `retry.max_backoff_secs`，沿用现行语义 |

### 1.3 同一概念同一名字

| 概念 | 名字与出现处 | 取代或说明 |
|---|---|---|
| 请求标识 | `request_id`。出现在 HTTP 头 `x-aproxy-request-id`、错误体 `error.aproxy.request_id`、IPC 记录与事件、format 消息，以及日志字段 `request_id=` | 取代实例内序号（proxy.rs:1265）。形如 `<instance_id>-<序号>`，例如 `19ac3f2e8b5d1a2b-42`；对外是不透明串，不得解析 |
| 实例标识 | `instance_id`：16 位十六进制，由启动时刻毫秒的低 48 位加 pid 的低 16 位组成 | 新增。跨重启可以区分（O1） |
| 错误码 | `code`。出现在 IPC 的 `error.code` 与 `last_error.code`、`--json` 的 `error.code`、尝试记录、format 的 `failure.code`、日志 `code=`；HTTP 里写成 `error.type = "aproxy_<code>"` 与头 `x-aproxy-error: <code>` | 取代中文自由文本 |
| 尝试 | `attempt`：序号，从 1 起；`attempts`：总数 | |
| 上游状态码 | HTTP 错误体与 IPC 记录里叫 `upstream_status`；format 消息里在响应对象内，叫 `status` | |
| 监听地址 | `listen_addr`，对应旗标 `--listen-addr` | 取代 `--listen` |
| 上游地址 | `base_url`，对应旗标 `--base-url` | 取代 `--baseurl`（D-4） |
| 日志位置 | 要求的位置叫 `log_file`（配置与 `--log-file`）；实际的位置叫 `log_path`（记录与 IPC） | 两个不同的概念，名字有意不同 |
| 配置文件 | `config_path`（记录、IPC）；台账里沿用 C3 的 `config`（F 级，不改） | |
| 闲置阈值 | settings 的 `instances.idle_threshold_secs`，对应 `stop idle --idle-threshold-secs` | 取代 settings 的 `idle_timeout_secs`，它与转换器的同名字段含义不同（K12） |
| format 程序 | `[[format]].name`，同一个名字也用于 IPC 的 `mode.formats[].name`、码的 `details.format`、请求记录的 `reports[].format` | |
| 阶段 | 配置 `stages` 里的词，就是消息里 `stage` 的取值 | |
| 心跳拍号 | `heartbeat.beat` | 取代 `HeartbeatInfo.seq`，避免与消息的 `seq` 撞名 |

`state` 在 IPC 里指实例状态，在 format 协议里指程序存下的不透明值；两者各在自己的协议里，不改名。

### 1.4 语言

| 给谁 | 语言 | 出口 |
|---|---|---|
| 人 | 随语言环境（en / zh-CN，判定规则见 W3） | 命令行的文本输出与提示、`--help`、桌面界面 |
| agent 与程序，或用户看不到的地方 | 固定英文 | 全部日志（实例日志、startup.log、watchdog.log、install.log）、HTTP 错误体与 SSE 终态事件、IPC 的 `message`、`--json` 的 `message`、format 协议消息、install.state、`--help` 末尾的 skill 提示行 |
| 机器字段 | 英文标识符 | `code`、`details` 的键与枚举值、事件名、阶段名 |

**渲染规则**：
- 显示端按 `code` 加 `details` 本地化；遇到不认识的码，显示英文 `message`。
- `details` 只放数据（数字、标识、路径），不放需要翻译的句子。
- 唯一的例外是上游或第三方给出的原文，例如 reqwest 的错误原因、format 的 `reason`：它们作为数据原样显示。

### 1.5 密钥只写不读，打码只有一个出口

**现状**：
- URL 打码在库里（config.rs:852 `mask_base_url`）。
- 不透明密钥打码在二进制一侧（util.rs:39 `mask_secret`）：保留前 6 个字符，6 个字符以内的密钥整段明示（CI-22）。
- format 的 args 另有一套打码（commands/config.rs:13-48）。
- doctor 原样输出 validate 的错误，带凭据的代理 URL 会被明文打出（CI-14）。

**设计**：
- **只写不读的字段**：
  - `api_key`、`proxy_password`；
  - `proxy`、`base_url`、`update.download_proxy` 这三个 URL 里的 userinfo；
  - `[[format]].env` 的值；
  - `[[format]].args` 里密钥旗标之后的值（沿用 SECRET_ARG_FLAGS 的规则）；
  - 名字像鉴权的请求头的值。名字不分大小写，包括 `authorization`、`proxy-authorization`、`cookie`、`set-cookie`，
    以及名字里含 `key`、`token`、`secret`、`auth`、`session`、`signature` 的头。`extra_headers` 与 `override_headers` 里这些头的值同样处理。
- **读出时给占位对象**。所有读出口都这样做：IPC、`config show/get`、`settings show/get`、`--json`、doctor、日志、错误消息。
  - 基本形式是 `{"secret":true}`。
  - 值长 24 个字符及以上时附提示：`{"secret":true,"hint":"sk-a…f3a9"}`，取首 4 尾 4。
  - 文本输出写成 `<secret sk-a…f3a9>`。
- **写回占位对象等于不改**。界面与 CLI 因此可以安全地整份往返一份配置。
- **URL**：userinfo 一律打码；查询串的值也一律打码（`?key=***`，沿用现行，宁可多遮）。
- **请求记录**：请求头按上面的名字规则打码，路径的查询值打码；请求体与响应体不打码，它们只经过访问控制之后的 IPC 按需读取（§6）。
- **纵深防御**：format 的 `report` 或 `fail.reason` 里如果出现 aproxy 已知的密钥原文，aproxy 替换成占位文本再记录。
- **唯一出口**：打码只经 `aproxy_ipc::redact`，主 crate 与桌面共用；validate 产出的诊断在源头就已打码（SE2）。

**为什么**：用户定了「IPC 永不返回明文密钥，密钥只写不读」。
- 否决「按查询参数名打码」：漏遮一个密钥的代价，远大于少显示一个 `api-version`。
- 否决「提示保留前 6 个字符」：短密钥会整段暴露。

### 1.6 安全边界与访问控制

**现状**：
- 命名管道用默认安全描述符（daemon.rs:1373-1377），默认 DACL 给 Everyone 读权限。
- unix socket 与 run 目录的权限取决于 umask。
- 记录文件按默认权限落盘（CP-09、CI-14）。

**设计**（SE1）：
- **信任模型**：同一用户的进程互相信任，它们本来就能读 config.toml 里的明文 key；要防的是别的用户与远程。所以不设令牌。
- **控制通道只走本地 IPC**，不为任何控制开网络端口；代理端口只转发（铁律不变）。
- **Windows**：每个管道实例都带显式 SDDL `D:P(A;;GA;;;<当前用户 SID>)(A;;GA;;;SY)`。首个实例的 `first_pipe_instance` 不变，拒绝远程客户端；
  客户端照旧核对服务端 pid。
- **unix**：`run/` 0700，socket 0600。服务端用 `peer_cred` 拒绝 uid 不同的连接；客户端核对服务端的 uid 与 pid。
- **文件**：
  - unix 上 aproxy 写的记录、日志、settings.json、config.toml 一律 0600，在新建时就设好。
  - Windows 上，含密钥的 restore 与 spec 文件另设显式 DACL（当前用户加 SYSTEM），不依赖 home 所在目录的继承。
  - doctor 对组或他人可读的 config 与 settings 告警，码为 `file_permissions_too_open`。
- **写类 op 与敏感读 op**：写类是 `shutdown`、`prepare_swap`，敏感读是 `request` 取请求体。它们都只靠上面的端点访问控制保护；
  请求体相关的 op 要等访问控制落地之后才开放（§14 B7 → B8）。
- **桌面扩展**的「写操作需要用户同意」在 Tauri 命令的 Rust 桥接层强制（G9）；IPC 本身不区分调用者。
  界面的 webview 禁止访问外部网络（W5）。

**为什么**：
- 否决令牌：能读配置文件的同用户进程同样能读到令牌，令牌只增加复杂度。
- 否决「经 IPC 写配置」：写面越小越好；GUI 与 CLI 是同一个用户，经操作层直接改文件即可（§6.4）。

---

## 2 错误码（K1）

**现状**：
- 失败原因只是字符串，同一段中文同时进入 HTTP 体、SSE、IPC、信封与日志（DP-03）。
- `note_upstream_failure`（proxy.rs:432）约有 20 个调用点。
- IPC 的 `LastError` 只有 `message` 与 `at`（daemon.rs:211）；`aproxy status` 原样打印它（status.rs:63-73）。
- IPC 只有 4 个码（daemon.rs:279-289）。

**设计**：
- **单源**：码表放在 `aproxy_ipc::code`（新 crate，见 §14 B0）。每个码一条元数据：名字、族、HTTP 状态（数据面终态码才有）、
  退出码（操作码才有）、`details` 的键。docs 与 skill 里的码表由测试从这里渲染并比对，改了码表却没改文档，测试就会红。
- **命名**：snake_case，全局唯一，只增不改（A 级）。
- **各族出现在哪里**：
  - §2.1 数据面终态码：HTTP 与 SSE。
  - §2.2 尝试失败码：`last_error`、请求记录、format 消息，以及终态码的 `details.last_code`。
  - §2.3 IPC 协议码。
  - §2.4 操作码与配置诊断码：CLI 的 `--json`、退出码、桌面界面。

### 2.1 数据面终态码（HTTP 状态固定）

| 码 | HTTP | 何时 | `details` |
|---|---|---|---|
| `inbound_host_rejected` | 403 | Host 不在允许范围（防 DNS 重绑定） | `host` |
| `inbound_origin_rejected` | 403 | 带了不允许的 Origin | `origin` |
| `request_too_large` | 413 | 请求体超过 `max_request_mb` | `limit_bytes` |
| `client_body_aborted` | 400 | 读客户端请求体时中途失败 | — |
| `upgrade_unsupported` | 501 | Upgrade（WebSocket）请求（综C7） | `upgrade` |
| `local_storage_failed` | 507 | 请求体或响应 spool 落盘失败（综C12） | `op`：`create`、`write` 或 `read` |
| `response_too_large` | 502 | 上游响应超过 `max_response_mb` | `limit_bytes` |
| `retry_exhausted` | 502¹ | 规则或按请求覆盖给的次数已用尽 | `by`（`rule:<序号>` 或 `override`）、`last_code` |
| `retry_stopped` | 502¹ | format 的 decide 叫停 | `format`、`last_code` |
| `format_rejected` | 422² | 请求侧 format 回 `fail`，且 `retry:false` | `format`、`stage`、`fail_code` |
| `format_failed` | 502 | 仅转发模式下请求侧 format 失败（这种模式没有重试循环） | `format`、`stage`、`kind` |
| `upstream_failed` | 502 | 仅转发模式下上游请求失败 | `last_code` |
| `decode_failed` | 只作 SSE 事件 | 流已提交，上游的压缩编码无法携带，解码又失败了 | `encoding` |
| `shutting_down` | 503 | 实例正在退出（综C10） | — |
| `internal` | 500 | 程序缺陷 | — |

¹ 有上游响应时，原样透传那个响应，状态码是上游的，只加 §3 的两个头；没有上游响应时，才回 502 与 aproxy 的错误体。
² format 附了自己的 `response` 时原样返回它（§10.4）；没附才回 422。

### 2.2 尝试失败码

每一次上游尝试没有成功，都记一个尝试失败码。默认情况下全部无限重试，规则表（§7.3）与 format（§10.5）可以改变这一点。

| 码 | 含义 | `details` |
|---|---|---|
| `upstream_connect_failed` | 连不上：DNS、被拒绝、TLS 握手失败 | `reason`：上游库给出的英文原文 |
| `upstream_connect_timeout` | 连接超时 | `timeout_ms` |
| `upstream_first_byte_timeout` | 请求已发出，迟迟等不到响应头 | `timeout_ms` |
| `upstream_read_idle_timeout` | 读响应体时，两次读到数据之间超时 | `timeout_ms`、`received_bytes` |
| `upstream_body_interrupted` | 响应体中途断开 | `received_bytes`、`reason` |
| `upstream_status` | 上游回 4xx 或 5xx | `status` |
| `upstream_error_body` | 上游回 2xx，但体是错误（包括流里的错误事件） | `status` |
| `format_error` | format 进程级失败，或输出不合法；按 D-6 一律按应重试处理 | `format`、`stage`、`kind`：`spawn`、`io`、`died`、`timeout`、`protocol`、`handshake` 或 `invalid_output` |
| `format_fail` | format 回 `fail`，且 `retry:true` | `format`、`stage`、`fail_code`（可缺） |
| `format_decision` | decide 把一次成功的尝试判为失败 | `format` |

### 2.3 IPC 协议码

| 码 | 何时 | `details` |
|---|---|---|
| `bad_request` | 不是 JSON、缺 `v` 或 `op`、行过长、参数类型或取值不对 | `reason` |
| `unsupported_version` | `v` 不受支持 | `supported`：整数数组。现状里它放在错误对象顶层（daemon.rs:271-277），挪进 `details` |
| `unknown_op` | 没有这个 op。没有副作用，可以拿来探测 | `op` |
| `unsupported_arg` | 带了不认识的参数 | `args`：参数名数组 |
| `invalid_state` | 此刻不允许，例如退出中收到 `prepare_swap` | `state` |
| `condition_not_met` | 带条件的 op 条件不成立，例如 `shutdown{if_idle_secs}` 时实例不闲 | `idle_ms` |
| `busy` | 流式连接已满 | `limit` |
| `not_found` | `request` 要的记录已不在保留窗口里 | `request_id` |
| `no_log_file` | 前台实例没有日志文件 | — |
| `internal` | 程序缺陷 | — |

### 2.4 操作码、诊断码与退出码

**通用退出码**（D-3、G5）：

| 退出码 | 含义 |
|---|---|
| 0 | 成功，或目标状态已经达成：`stop` 一个本来就停着的目标、`start` 一个已用同一配置在跑的实例 |
| 1 | 操作失败 |
| 2 | 用法错误：包括 clap 的解析错误、该指定目标时没指定、home 无效 |
| 3 | 配置无效：config.toml 或 settings.json 有 error 级诊断 |
| 4 | 目标未运行或不存在（`stop` 例外，见 0） |
| 5 | 部分成功：多目标操作里有成功也有失败 |
| 6–9 | 预留给通用含义 |
| 10–12 | `install --force`（C7，F 级） |
| 13–19 | 预留给 `install --force` |

被信号结束的进程（例如 Ctrl+C 退出 `logs -f`），退出码由操作系统给出（如 130），不在表内。

**操作码**（CLI 的 `--json`、桌面界面的操作结果）：

| 码 | 退出码 | 何时 | `details` |
|---|---|---|---|
| `usage` | 2 | clap 以外检出的用法错误 | `reason` |
| `home_invalid` | 2 | `--home` 或 APROXY_HOME 不符合 C1，例如不是绝对路径 | `value` |
| `target_ambiguous` | 2 | 有多个实例在跑，却没指定目标 | `ports` |
| `target_not_found` | 4 | 别名、端口或配置文件不存在 | `target` |
| `target_not_running` | 4 | 目标没在运行 | `target` |
| `config_invalid` | 3 | 配置有 error 级诊断 | `diagnostics` |
| `settings_invalid` | 3 | settings.json 有语法错误，而命令需要写它 | `diagnostics` |
| `settings_schema_too_new` | 1 | settings.json 的 schema 比本版本高，拒绝写 | `schema` |
| `write_conflict` | 1 | 写配置期间文件被并发修改，重试之后仍冲突 | `file` |
| `port_in_use` | 1 | 端口被别的进程占用，包括别的 home 或别的配置的实例 | `port`、`owner`（`aproxy` 或 `other`）、`home` |
| `start_failed` | 1；子进程报的是配置类错误时为 3 | 守护起不来 | `code`（子进程报的码）、`startup_log`（本次的摘录行） |
| `ready_timeout` | 1 | 守护起来了，但没在时限内就绪 | `timeout_ms` |
| `instance_unresponsive` | 1 | 实例在，但不应答 | `port`、`pid` |
| `instance_incompatible` | 1 | 实例应答了，但协议不兼容或身份不符 | `port`、`reason` |
| `stop_failed` | 1 | 停不掉 | `port`、`reason` |
| `desktop_unavailable` | 1 | 在 headless 版本上执行 `desktop` | `variant` |
| `skill_not_installed` | 4 | 本地没有这个 skill（W4） | `name` |
| `skill_file_not_found` | 4 | 文件不在 skill 清单里（W4） | `name`、`file` |
| `internal` | 1 | 程序缺陷 | — |

**配置诊断**：每条诊断是一个对象 `{"code","severity","file","key"?,"line"?,"column"?,"message","details"}`，`severity` 取 `error` 或 `warning`。
它们出现在 `config_invalid` 的 `details.diagnostics`、doctor 的输出、`config show` 与 start 的告警里。

| 码 | 级别 | 何时 |
|---|---|---|
| `config_syntax` | error | TOML 或 JSON 语法错误，带行列 |
| `config_type` | error | 类型不对 |
| `config_invalid_value` | error | 取值不合法，带 `reason` |
| `config_missing_key` | error | 缺了必填的键，例如 `base_url` |
| `config_unknown_key` | warning | 不认识的键，带 `suggestion` |
| `config_removed_key` | warning（见 Q3） | 0.1.x 的旧键，带 `replacement` |
| `config_not_supported` | error | 本版本只定了形状、还没实现的写法，例如规则的 `body`、阶段 `admit` 与 `notify` |
| `config_conflict` | error | 互斥的组合，例如仅转发模式下声明了 response 阶段 |
| `format_unavailable` | required 程序为 error，可选程序为 warning | 启动时与程序握手失败，带 `format`、`kind` |
| `format_stage_unsupported` | 同上 | 程序不支持配置声明的阶段，带 `format`、`stages` |
| `file_permissions_too_open` | warning | doctor 发现组或他人可读的配置文件 |
| `settings_invalid_value` | warning | settings.json 某个字段坏了，改用默认值，带 `key` |

**安装码**用 `install_*` 一族，例如 `install_in_progress`、`install_version_not_found`（退出码 4）、`install_below_floor`、
`install_download_failed`、`install_checksum_mismatch`、`install_probe_failed`、`install_swap_failed`、`install_rolled_back`。
完整列表由 install 的实现者在做 `--json` 时补齐，install.state 的 `error_code` 与 NDJSON 用同一套码。
强制安装只承诺 C7 的退出码，它的输出文本属于 E 级。

**为什么**：
- 码是一切渲染的输入，必须最先定（综合稿地基顺序第 1 条）。
- HTTP 用 `error.type` 承载码而不用 `error.code`：Anthropic 与 OpenAI 的 SDK 读字符串类型的位置都是 `error.type`；
  Gemini 的 `error.code` 是整数，在那里放字符串会让强类型客户端反序列化失败（DP-01）。
- 尝试失败码与终态码分开：前者回答「这一次为什么没成」，后者回答「这个请求怎样结束」。界面按前者聚合，按后者着色。
- `format_rejected` 用 422：5xx 会触发客户端 SDK 自己的重试（DP X-2），确定性的拒绝不该被白白重试。
- 否决把上游的错误语义编进 aproxy 的码：那由上游定义；aproxy 只说它观察到了什么，即状态码，以及体是不是错误。
- 综C6（非法上游 URL 归为终态 `bad_upstream_url`）被 D-6 取代：format 改写出非法 URL 属于「输出不合法」，按应重试处理
  （`format_error`，kind 为 `invalid_output`）；`base_url` 本身由配置校验保证合法。确定性的 format 故障怎样封顶，见 Q1。

---

## 3 HTTP 错误体与 SSE 终态事件（K2）

**现状**：
- 自产错误是 text/plain 的中文，散在多处：proxy.rs:291-322（403）、1296-1303（400）、1613-1629（转换失败 502）、1654-1669、3042、
  3236-3244（413）、3360-3375（仅转发模式）。
- SSE 终态有一个通用构造（proxy.rs:1633），另有四处手写（proxy.rs:2846-2861、2969-2971、3023-3025）。其中 `upstream_error` 用 `format!` 拼 JSON。
- skill 把这些中文原文当作排障锚点逐字引用（DP-01、DP-02）。

**设计**：

响应体，`content-type: application/json`：

```json
{"type":"error","error":{"type":"aproxy_retry_exhausted","message":"upstream returned 404 on all 3 attempts allowed by retry rule 1","aproxy":{"code":"retry_exhausted","request_id":"19ac3f2e8b5d1a2b-42","attempts":3,"upstream_status":404,"details":{"by":"rule:1","last_code":"upstream_status"}}}}
```

- `type` 恒为 `"error"`。
- `error.type` 等于 `"aproxy_"` 加码。
- `error.message` 是一句英文，不是契约。
- `error.aproxy` 是 A 级对象，字段如下：
  - `code`；
  - `request_id`；
  - `attempts`：已进行的上游尝试数，0 表示一次都没发出；
  - `upstream_status`：没有就省略；
  - `details`：各码的键见 §2。
- `error.code` 不放任何值。

响应头：
- 两个头：`x-aproxy-request-id: <request_id>`、`x-aproxy-error: <code>`。
- 加在以下响应上：
  - aproxy 自产的错误响应；
  - 放弃重试后透传的上游响应（`retry_exhausted`、`retry_stopped`）；
  - format 给出的终态响应（`format_rejected`，以及 decide 叫停时附带的响应）。
- 不加在正常成功的透传响应上，保持保真。

SSE 终态，用于响应头已提交之后才发生的终态：

```text
event: error
data: {"type":"error","error":{"type":"aproxy_shutting_down","message":"aproxy is shutting down","aproxy":{"code":"shutting_down","request_id":"19ac3f2e8b5d1a2b-43","attempts":2}}}

```

- 与 HTTP 体是同一个 JSON，全部经同一个构造函数用 serde 序列化，不再手拼。
- 提交之后的「放弃」用 `retry_exhausted`、`retry_stopped` 或 `format_rejected`，带上 `upstream_status`。
- 提交之后 format 才给出终态响应时：若它的 `content-type` 是 `text/event-stream`，把它的体原样写进流再结束；否则写上面的标准终态事件。

更多样例：

```text
HTTP/1.1 413 Payload Too Large
content-type: application/json
x-aproxy-error: request_too_large
x-aproxy-request-id: 19ac3f2e8b5d1a2b-44

{"type":"error","error":{"type":"aproxy_request_too_large","message":"request body exceeds max_request_mb (134217728 bytes)","aproxy":{"code":"request_too_large","request_id":"19ac3f2e8b5d1a2b-44","attempts":0,"details":{"limit_bytes":134217728}}}}
```

```json
{"type":"error","error":{"type":"aproxy_format_rejected","message":"format 'router' rejected the request: no channel configured for model gpt-9","aproxy":{"code":"format_rejected","request_id":"19ac3f2e8b5d1a2b-45","attempts":0,"details":{"format":"router","stage":"request","fail_code":"model_not_routed"}}}}
```

**为什么**：
- 结构与英文化一次到位，skill 与 agent 的锚点只断这一次（DP-01）。
- 放弃之后透传的响应也加头，偏离了 K2「只加在自产响应上」的原建议，理由如下：
  - 在 aProxy 后面，客户端收到的任何非 2xx 都意味着 aProxy 已经放弃了。
  - `x-aproxy-error` 让 agent 能分清「aProxy 放弃了」与「这是上游自己的回答」。
  - 体仍是上游原样，保真不受影响。
- 否决用 `event: aproxy_error` 作为 SSE 事件名：客户端解析器普遍只认 `error`；类型信息已经放在 JSON 里。
- 验收要求用真实的 Anthropic 与 OpenAI SDK 各解析一次（综合稿 alpha.2 验收）。

---

## 4 磁盘布局与持久记录（K5）

**现状**：
- 有两个根：APROXY_HOME 与 APROXY_RUN_DIR（daemon.rs:27）。
- unix 的心跳与安装宣告放在 /dev/shm（watchdog.rs:1087-1106、announce.rs:24-36）。
- `.pid` 名不副实，字段「全部必填」（daemon.rs:140-168）。
- `.restore` 存的是 argv（daemon.rs:1023-1078）。
- status 普查时顺带删孤儿日志与解析不了的记录（daemon.rs:744-871）。
- spool 按配置端口分目录，并在 bind 之前清空（server.rs:92，DP-08）。

### 4.1 布局

```text
<home>/                          唯一的根：APROXY_HOME 或 --home（C1）
  config.toml  configs/          用户配置
  settings.json                  §8
  bin/                           C2（F）
  install/                       C4（F）；state.json、unix 的 announce 在这里
  ledger/<pid>.json              C3（F）
  skills/  skills/manifest.json  W4
  desktop/                       webview 数据（E）
  logs/
    startup.log  watchdog.log  install.log    固定名（A），内容英文
    <纳秒hex>-<pid低16位hex>.log               实例日志，随机名（命名模式 A）
  spool/<pid>/                   按进程分目录（目录规则 A，内容 E）
  run/                           0700
    <port>.instance.json         实例记录（A，schema 1）
    <port>.restore.json          期望状态记录（A，schema 1）
    <纳秒>-<pid>.spec.json        拉起规格的交接文件（A，schema 1）
    <port>.sock                  unix 控制端点（A）
    <port>.heart                 unix 心跳（A），原 /dev/shm
    watchdog.json                看护者记录（A，schema 1），原 watchdog.claim
    watchdog.sock                预留：home 级端点（R）
    *.tmp-<pid>                  原子写的临时文件
Windows 内核对象（A）：
  \\.\pipe\aproxy-<home_id>-<port>      实例控制端点
  \\.\pipe\aproxy-<home_id>-watchdog    预留（R）
  Local\aproxy-<home_id>-heart-<port>   心跳节
  Local\aproxy-<home_id>-install        安装宣告节
```

- `home_id` 是规范化后的 `<home>/run` 路径的 SHA-256 前 16 位十六进制，沿用现行算法（daemon.rs:107），A 级。
- 心跳：节或文件的前 8 字节是小端 u64 毫秒时间戳（A）。
- 安装宣告：沿用现行的 16 字节布局 `{installer_pid, heartbeat_ms}`（announce.rs:41-45，A）；unix 上的文件挪到 `<home>/install/announce`。
- **与 W2b 的协调**：
  - 已在 alpha.1 落地（6671ff2、bc18fda，W2b 负责人确认）：unix 的安装宣告是 `<home>/install/announce`，C4 随之改为「没有例外」，
    强制安装第 4 步清空 `install/` 时就清掉了；没有 state.json 时续作删掉过期的宣告。旧的 /dev/shm 位置只有早期开发构建写过
    （tmpfs，重启即无），不读也不清理。
  - 台账（C3）与实例记录分开（G3）：台账只回答「这个 home 里有哪些 aproxy 进程、怎样结束它们」；实例记录服务 status 与界面，可以演进。

### 4.2 记录

所有记录都是：
- 一个 JSON 对象，首个字段是 `schema`；
- 由属主一个人写，原子写入（先写 `*.tmp-<pid>`，再 rename）；
- 文件权限 0600。

**实例记录** `run/<port>.instance.json`：
- 写者是实例：bind 成功后写，正常退出时删。实例每 30 秒核对一次，发现自己的记录丢了就补写（CP-24）。
- `base_url` 已打码。
- `log_path` 为 null 表示没有日志文件（前台实例），取代现行的空串约定。
- IPC 快照里的 `instance` 就是这个对象去掉 `schema`。

```json
{"schema":1,"pid":4321,"start":"w:133712345678901234","instance_id":"19ac3f2e8b5d1a2b","version":"0.2.0","listen_addr":"127.0.0.1:12345","config_path":"C:\\Users\\u\\.aproxy\\config.toml","base_url":"https://api.example.com","started_at":1760190000123,"log_path":"C:\\Users\\u\\.aproxy\\logs\\19ac3f2e8b5d-1a2b.log","foreground":false}
```

**拉起规格 InstanceSpec**：它是 `run/<纳秒>-<pid>.spec.json` 的全部内容，也内嵌在期望状态记录里。

```json
{"schema":1,"config_path":"C:\\Users\\u\\.aproxy\\configs\\work.toml","overrides":{"listen_addr":"127.0.0.1:12346","api_key":"sk-ant-…"},"env":{"HTTPS_PROXY":"http://127.0.0.1:7890","https_proxy":null,"HTTP_PROXY":null,"http_proxy":null,"ALL_PROXY":null,"all_proxy":null,"NO_PROXY":"localhost,127.0.0.1","no_proxy":null,"PATH":"C:\\Windows\\system32;…"},"log_path":"C:\\Users\\u\\.aproxy\\logs\\19ac3f2e8b5d-1a2b.log","foreground":false}
```

- `overrides`：键就是 config.toml 的键，只含 `start` 的覆盖旗标实际给过的。0.2.0 有五个：`base_url`、`listen_addr`、`proxy`、`api_key`、`log_file`；
  以后加旗标，就是多一个键。
- `env`：启动者环境里的 8 个代理变量加 `PATH`（D-2 的方案 i）。
  - 值为 `null` 表示启动者没设，拉起时在子进程里删掉它。
  - reqwest 用 rustls 与 webpki 根证书，TLS 相关的环境变量不影响它，所以不记。
- `log_path`：这个实例写的日志文件。
  - 新启动时，由拉起方生成随机名填进来，这样 start 立刻就知道日志在哪。
  - restart、restore、看护者重拉都续写同一个文件。
- 密钥：`overrides.api_key` 与代理 URL 里的凭据按 D-2 以明文存放，文件 0600，Windows 上另设显式 DACL。它们永不进入任何命令行，所有出口都打码。

**期望状态记录** `run/<port>.restore.json`：
- 写者是实例：启动成功后写；收到 shutdown 立即删；`stop all` 删全部；前台实例不写。

```json
{"schema":1,"spec":{"schema":1,"config_path":"…","overrides":{},"env":{},"log_path":"…","foreground":false},"written_at":1760190000456}
```

**看护者记录** `run/watchdog.json`：
- 写者是看护者，每轮扫描整份重写。
- `restarts` 保留最近 20 次；`kind` 的词表是 `crash`、`hang`，只增。
- 读不懂这份记录的看护者按文件 mtime 判断它是否新鲜（CP-01），不覆盖它。

```json
{"schema":1,"pid":5555,"start":"w:133712345000000000","version":"0.2.0","heartbeat_secs":30,"created_at":1760190000000,"last_scan_at":1760190123456,"restarts":[{"port":12345,"at":1760190100000,"kind":"crash","ok":true}]}
```

**安装状态** `install/state.json`：
- 保留现有字段，新增四个可选字段（A5）：
  - `failed_at`：失败时所处的相位；
  - `installer_version`；
  - `error_code`：§2.4 的 `install_*` 码；
  - `error_details`。
- 相位词表只增；读者把不认识的相位当作 `unknown`。
- 内容仍只由 install 自己读写，现行定调不变。

### 4.3 读者与清理规则

1. 解析失败，或 `schema` 高于自己认识的，就跳过，并在 status 与 `--json` 的 `unreadable` 里报告；绝不删除。
2. 只清理同时满足两个条件的记录：是本版本的 schema；写者经 `pid` 加 `start` 核验已经不在。核验按 C3 的启动标记推导。
3. 临时文件与 spec 文件：pid 已不在、且早于 10 分钟的才删。spec 文件名里的 pid 是拉起方的 pid。
4. 读路径（status、enumerate、界面轮询）不删任何文件。清理只在两个时机做：实例启动时（bind 与控制端点都成功之后），以及看护者的周期里（综C1）。
5. 日志 GC：
   - 只处理同时满足三个条件的文件：名字符合内置随机名模式 `^[0-9a-f]{1,16}-[0-9a-f]{4}\.log$`；早于 10 分钟；不被任何实例记录或期望状态记录引用。
   - 只要 run/ 里有读不懂的记录，引用集就不完整，整轮跳过。
   - 自定义的 `log_file` 与三个固定日志永远不删（CP-26），只按 `logs.rotate_mb` 轮转。
6. spool：实例在 bind 与控制端点都成功之后，删除 `spool/` 下 pid 已不在的目录；自己只写 `spool/<自己的 pid>/`（综C5）。

**为什么**：
- **home 作为唯一的根**：寻址只剩一个变量；有了 `--home`，多 home 部署也不必改环境（CP-05）。
- **删 APROXY_RUN_DIR**：它让同一个 home 有两种寻址方式（TODO 第 322 行）。
- **平铺加 kind 后缀**：一个 0700 目录最简单。否决按类分子目录：要多管几个目录的权限与清理（CP §5）。
- **心跳与宣告挪进 home（unix）**：macOS 没有 /dev/shm；容器里的 /dev/shm 很小；home 之外的文件没人清理（CP-15）。
  否决综合稿 3.12「宣告改用 install/lock 的持锁状态」：锁只在修改 bin/ 期间持有，交接时持有者还会换人，表达不了「安装进行中」。
- **读者绝不删除**：0.2.0 是今后所有版本面对的最老读者。W2b「其余可以随意演进」能成立，前提正是旧读者不破坏自己看不懂的东西（K5）。
- **续写同一个日志**：现行做法是恢复成功后旧日志成了孤儿被删，崩溃证据随之丢失；续写让崩溃前后的日志留在同一个文件里。
- **config.toml 不加 schema**（CI 报告 §4）：这是人手写的文件，靠「只增、未知键告警、写回时保留」就能演进。

---

## 5 拉起、期望状态与进程生命周期（K6、D-2、D-8）

**现状**：
- 守护靠隐藏的全局旗标 `--daemon-child` 拉起（cli.rs:42）。restore.rs:39-40、restart.rs:125、watchdog.rs:953-954 各自拼一次命令行。
- 看护者的命令行复制启动者的全部参数，包括 `--api-key`（watchdog.rs:339-344）。
- Windows 上环境与工作目录都继承启动者（daemon.rs:602-694）；unix 不调用 setsid；代码有意不加 BREAKAWAY（daemon.rs:598）。
- 看护者开关只在启动时读一次（server.rs:211）。
- 停止时最后才删 `.restore`（CP-21）。
- 退出宽限 10 秒后强退（server.rs:343）。

**设计**：

**内部入口**改为三个隐藏子命令。它们是 A 级：跨版本拉起要用它们，安装的早交接就是旧版本拉起新版本。
- `aproxy __serve --home <dir> --spec <file>`：读规格，删掉规格文件，然后运行实例。
- `aproxy __watchdog --home <dir>`。
- `aproxy __install-resume --home <dir> [--handover-from <pid>]`：取代 `install --continue [--handover-from]`。

这三个名字不出现在帮助里；skill 的「Processes you may see」一节改为列出它们。

**一个拉起函数** `spawn_from_spec(spec)`，start、restart、restore、看护者、install 五处共用（S2 的合一）：
1. 写 spec 文件（0600），用拉起方自己的可执行文件运行 `__serve`。用自己的 exe，与现行一致。
2. 子进程环境等于拉起方的环境，再按 `spec.env` 设置或删除那 9 个变量。
3. 工作目录固定为 home（综C4）。配置里的相对路径（`log_file`、format 的 `command`）都相对 home 解析。
4. 脱离启动者（综C3）：
   - unix 用 `process_group(0)` 加 `setsid()`；
   - Windows 先带 `CREATE_BREAKAWAY_FROM_JOB`，被拒（ERROR_ACCESS_DENIED）就去掉它再试一次。
5. 等就绪：
   - 早退检测：子进程一退出就判失败，退出码按 §2.4 映射；
   - 控制端点应答即就绪；
   - 失败时摘录 startup.log 里本次写下的行。

**实例的启动顺序**：
1. 读 spec，解析配置（§7.5）并校验。
2. 写台账。C3 要求在占用端口或拉起子进程之前写，而 format 程序就是子进程，所以排在握手之前。
3. 与 format 程序握手（§10.2）。
4. bind 代理端口，再 bind 控制端点。
5. 写实例记录；写期望状态记录（前台实例不写）。
6. 清理 spool 与日志（§4.3），开始服务。

任何一步失败，都用英文写进 startup.log（带 `code=`），并按 §2.4 的退出码退出。

**停止**（综C10）：
- 收到 `shutdown` 立即删期望状态记录，然后排空：
  - 已提交的保活流写 `shutting_down` 终态事件；
  - 还没提交的请求回 503。
- `stop all` 把期望状态改为「全部停止」：连没有活实例的期望状态记录也一并删除。
- `stop <目标>` 遇到已停、但还留着期望状态记录的目标，也删掉那份记录，结局为 `already_stopped`。
- 要「停了再拉起」的调用方（restart、install）必须在发 shutdown 之前读走 spec。install 把读到的 spec 记进 install/state.json，
  中途被打断时由续作据此把实例拉回；否则 shutdown 删掉记录之后，实例就无人能恢复。

**restart**：读目标的期望状态记录 → shutdown → 等它退出 → 用记录里的 spec 调用 `spawn_from_spec`。覆盖项与环境原样重放（D-2）。

**看护者**：
- 命令行只有 `__watchdog --home`。
- 每个 tick 重读 settings，包括开关与调优参数；开关关闭就退出，保留期望状态记录（CP-18）。
- 实例存活满 60 秒才清零失败计数。
- 连续失败达到 `watchdog.max_restarts` 之后，转入慢速重试：每 300 秒一次，直到期望状态记录被删（D-8）。
- 处决、重拉、放弃、慢速重试、删记录，每一类事件都写进 logs/watchdog.log。

**不变的部分**：守护启动时退役比自己旧的看护者（现行 server.rs:111）；prepare_swap 的「ACK 齐了才交换」铁律。

**为什么**：
- argv 落盘会把整套启动语法与明文 key 冻结成跨版本契约（CP-02、CI-02）。结构化的 spec 让任何版本的拉起方都能翻译成自己的命令行。
- 否决「看护者命令行带全部参数」：命令行里的密钥能被本机任何进程枚举到（CP-14）。
- 快照环境（D-2 的方案 i）：使命要求重拉出来的实例与原实例走同一条网络路径；PATH 还决定 format 程序会解析到哪个可执行文件。
- 否决只记 argv、不记环境：看护者、GUI、开机 restore 的环境各不相同，同一份 argv 会拉起行为不同的实例。

---

## 6 IPC v1（K4、G1、O2、SE1）

**现状**：
- v1 只有 ping、shutdown、prepare_swap 三个 op，每个成功应答都是状态快照（daemon.rs:172-182）。
- 未知参数被静默忽略（daemon.rs:1215-1252）。
- 消息是中文（daemon.rs:291-311）。
- `last_error` 是字符串（daemon.rs:481）。
- 没有在途数、事件与日志流。
- 请求行上限 64 KiB，应答行上限 1 MiB（daemon.rs:1132-1172）。

### 6.1 传输与访问控制

- 端点见 §4.1，访问控制见 §1.6。
- 一元请求：客户端总超时 3 秒；服务端要求连上后 5 秒内发完请求行。
- 行长：请求行 ≤ 64 KiB；应答行 ≤ 16 MiB（`request` 带请求体时可能超过 1 MiB）。

### 6.2 帧

一行一个 JSON 对象，UTF-8 编码，以 `\n` 结尾。

```text
请求  {"v":1,"op":"shutdown","args":{"if_idle_secs":600}}
成功  {"v":1,"ok":true,"result":{…}}
失败  {"v":1,"ok":false,"error":{"code":"unsupported_arg","message":"unknown arg: drain_secs","details":{"args":["drain_secs"]}}}
事件  {"v":1,"event":"request","seq":42,"at":1760190000123,"data":{…}}
```

- 一元 op：一个请求、一个应答，然后关闭连接。
- 流式 op（`subscribe`，以及带 `follow` 的 `logs`）：先回一行一元应答，之后每行一个事件，直到任一方关闭连接。
- 不做多路复用。

### 6.3 规则

以下每一条都用契约测试钉住。
1. **版本**：服务端用请求里的 `v` 应答；发布过的 `v` 一律保持支持。不支持时回 `unsupported_version`，`details.supported` 列出支持的版本。
2. **op 与参数**：
   - 未知 op 回 `unknown_op`，没有副作用，可用于探测；
   - 未知参数回 `unsupported_arg`；
   - 参数类型或取值不对回 `bad_request`。
3. **只增**：应答与事件只增字段；客户端忽略未知字段与未知事件；枚举都有兜底，例如 `state` 读不懂就当作 `unknown`，即「不在正常服务」。
4. **message**：固定英文，只给日志看；显示端按 `code` 加 `details` 渲染。
5. **能力声明**：快照的 `ops` 列出支持的 op，`features` 列出 op 之下的能力，命名为 `<op>.<参数或能力>`。客户端先查再用。
6. **状态变更**：改变实例状态的 op 必须幂等，或者带条件参数。新的写类 op 或敏感 op，只在访问控制（§1.6）落地之后加入。
7. **流式连接**：
   - 每个实例最多 8 个流式连接，超出回 `busy`。
   - 服务端在流式连接上至少每 15 秒发一行，没有别的事件时发 `heartbeat`；客户端 45 秒收不到任何行，就判对端失联。
   - 实例退出前发 `closing`。
   - 事件的 `seq` 在每条连接上从 1 起递增。服务端丢弃了事件就发 `lagged{dropped}`，客户端据此重新取快照与记录。

### 6.4 op 一览

| op | 参数 | 结果 | 副作用 | 可能的错误 | features |
|---|---|---|---|---|---|
| `ping` | — | 快照 | 无 | `bad_request` | |
| `shutdown` | `if_idle_secs?` | 快照，state 为 `stopping` | 删期望状态记录，开始排空；重复调用返回同样的结果 | `condition_not_met`、`unsupported_arg` | `shutdown.if_idle_secs` |
| `prepare_swap` | `ttl_secs?`，默认 600 | 快照，state 为 `swap_prepared` | 到期自动回到 `serving`（CP-25） | `invalid_state` | `prepare_swap.ttl_secs` |
| `requests` | `before?`（一个 request_id）、`limit?`（1–500，默认 100） | `{"requests":[RequestRecord…],"more":bool}`，新的在前 | 无 | `bad_request` | |
| `request` | `request_id`、`bodies?`（默认 false） | RequestDetail | 无 | `not_found` | `request.bodies`（B7 之后才有） |
| `config` | — | 生效配置（已打码）与每个键的来源 | 无 | — | |
| `subscribe` | `topics?`（`requests`、`state`，默认全部）、`history?`（0–500，默认 100） | `{"snapshot":…,"requests":[…]}`，之后是事件 | 无 | `busy` | `subscribe.topics` |
| `logs` | `lines?`（默认 200，上限 10000）、`follow?`（默认 false） | `{"log_path":…,"lines":[…]}`，带 `follow` 时之后是 `log` 事件 | 无 | `no_log_file`、`busy` | `logs.follow` |
| `reload` | — | — | R：配置热重载，0.2.x 再做 | — | |

- `subscribe` 首行里的 `requests` 包含全部在途请求，加上最近结束的 `history` 条。
- `config` 的结果形如 `{"config_path","values":{…与 TOML 同构，密钥是占位对象…},"sources":{"retry.max_backoff_secs":"settings",…},"diagnostics":[…]}`。
- 不经 IPC 写配置：桌面与 CLI 都经操作层直接改文件，再 restart（3.12）。

### 6.5 快照

下面的样例为了易读分了行，线上仍是一行。

```json
{"instance":{"pid":4321,"start":"w:133712345678901234","instance_id":"19ac3f2e8b5d1a2b","version":"0.2.0","listen_addr":"127.0.0.1:12345","config_path":"C:\\Users\\u\\.aproxy\\config.toml","base_url":"https://api.example.com","started_at":1760190000123,"log_path":"C:\\Users\\u\\.aproxy\\logs\\19ac3f2e8b5d-1a2b.log","foreground":false},
 "home":"C:\\Users\\u\\.aproxy",
 "state":"serving",
 "mode":{"forward_only":false,"disk_cache":true,"keepalive":{"trigger":"any","interval_secs":15},"retry_rules":1,"history":{"requests":500,"bodies":true},
         "formats":[{"name":"aproxy-format","stages":["request","stream"],"mode":"persistent","required":true,"state":"ready","protocol":1,"version":"0.2.0"}]},
 "activity":{"last_request_at":1760190100000,"requests_total":120,"retries_total":7,"rescued_total":3,"in_flight":2,"retrying":1,
             "by_code":{"upstream_status":5,"upstream_connect_failed":2},
             "buckets":{"width_secs":120,"requests":[0,3,5],"retries":[0,0,2],"failures":[0,0,2]},
             "last_error":{"code":"upstream_status","details":{"status":529},"message":"upstream returned 529","at":1760190090000}},
 "ops":["ping","shutdown","prepare_swap","requests","request","config","subscribe","logs"],
 "features":["shutdown.if_idle_secs","prepare_swap.ttl_secs","subscribe.topics","logs.follow"]}
```

- `state` 的词表：`serving`、`swap_prepared`、`stopping`；兜底 `unknown`。
- `mode` 取代 TODO 第 247 行说的「状态里没有模式标识」，例如看不出实例是不是仅转发模式。
- `buckets`：最近 30 个桶，每桶 120 秒，旧的在前，最后一个是当前还没满的桶，供界面画迷你图。样例只画了 3 个。
- `rescued_total`：经过至少一次重试、最终成功的请求数，即界面上的「救回」。
- `by_code`：自启动以来按尝试失败码分别计数。
- `home` 取代现行的 `run_dir`：端口被别的 home 的实例占着时，据此认出是谁。
- `formats[].state`：`ready` 或 `unavailable`（可选程序握手失败，见 §10.2）。

### 6.6 请求记录

**RequestRecord** 是 `requests` 的结果元素，也是 `request` 结果里的 `record` 与 `request` 事件的 `data`。

```json
{"request_id":"19ac3f2e8b5d1a2b-42","started_at":1760190000123,"ended_at":null,"method":"POST","path":"/v1/messages?beta=***","model":"claude-sonnet-4-5","stream":true,"user_agent":"claude-cli/2.1.0",
 "outcome":"in_flight","code":null,"status":null,"phase":"backoff","next_attempt_at":1760190005123,
 "attempts":[{"attempt":1,"started_at":1760190000200,"duration_ms":812,"status":529,"code":"upstream_status","response_bytes":96}],
 "decisions":[{"attempt":1,"by":"default","decision":"retry","delay_ms":5000}],
 "keepalive":true,"heartbeats":3,"request_bytes":20480,"response_bytes":0,
 "reports":[{"format":"aproxy-format","stage":"request","summary":"anthropic_messages -> openai_chat; key k2","actions":[{"type":"protocol_convert","from":"anthropic_messages","to":"openai_chat"},{"type":"key_select","key_id":"k2"}]}]}
```

- `outcome` 的取值：
  - `in_flight`、`ok`；
  - `upstream_error`：放弃之后透传了上游的非成功响应；
  - `aproxy_error`：aproxy 自产的错误；
  - `format_response`：format 给出的终态响应；
  - `client_gone`；
  - 兜底按 `unknown` 显示。
  - 不是 `ok` 或 `in_flight` 时，`code` 是终态码（§2.1）。
- `phase` 的取值：`reading_body`、`transforming`、`attempting`、`backoff`、`replaying`、`done`；兜底 `unknown`。
- `model`：请求体是 JSON 对象、且顶层 `model` 是字符串时取它，否则为 null。这与 `body_stream` 触发条件只看顶层键是同一类读法。
  `stream` 按 `keepalive.trigger` 的同一套判定得出。
- `decisions[].by` 的取值：`format:<name>`、`override`、`rule:<序号>`、`default`。
- 记录按 `history.requests` 的条数先进先出淘汰；在途的不淘汰。

**RequestDetail** 是 `request` 的结果：

```json
{"record":{"request_id":"19ac3f2e8b5d1a2b-42","…":"…"},
 "request":{"headers":[{"name":"content-type","value":"application/json"},{"name":"x-api-key","value":{"secret":true,"hint":"sk-a…f3a9"}}],"body":"{\"model\":\"claude-sonnet-4-5\",…}","body_bytes":20480,"body_truncated":false},
 "attempts":[{"attempt":1,
              "sent":{"method":"POST","url":"https://relay.example.com/v1/chat/completions","headers":[{"name":"authorization","value":{"secret":true}}],"body":"{…}"},
              "response":{"status":529,"headers":[{"name":"content-type","value":"application/json"}],"body":"{\"error\":…}","body_bytes":96,"body_truncated":false},
              "error":null}],
 "bodies":"retained"}
```

- `bodies` 说明体有没有取到：
  - `retained`：取到了；
  - `not_requested`：参数是 `bodies:false`；
  - `evicted`：超出预算，已经丢了；
  - `disabled`：配置了 `history.bodies = false`。
- 体用 `body`（UTF-8）或 `body_b64` 表示；单个体超过 `history.body_max_kb` 就截断，并标 `body_truncated`。
- `sent.body` 只在与上一次发出的体不同时才出现，第 1 次尝试与客户端的原始请求体比较；不出现就表示相同。
- `error`：没有上游响应的那次尝试，给出 `{code, details, message}`。
- 仅转发模式不缓冲，所以只记元数据，不保留体。

### 6.7 只读枚举

这是 aproxy-ipc 提供的库 API，status `--json` 与桌面共用。`enumerate(home)` 的过程：读 run/ 与 ledger/，再并发 ping（单次 1 秒，重试 1 次）。返回：

```json
{"instances":[
  {"port":12345,"liveness":"live","instance":{"pid":4321,"…":"…"},"status":{"state":"serving","…":"…"},"restore_pending":false},
  {"port":12346,"liveness":"unresponsive","instance":{"pid":4400,"…":"…"},"restore_pending":false},
  {"port":12347,"liveness":"incompatible","instance":{"pid":4500,"…":"…"},"reason":{"code":"unsupported_version","details":{"supported":[2]}}},
  {"port":12348,"liveness":"down","instance":null,"restore_pending":true}],
 "unreadable":[{"file":"run/12349.instance.json","reason":{"code":"record_schema_too_new","details":{"schema":2}}}],
 "watchdog":{"pid":5555,"start":"w:133712345000000000","version":"0.2.0","state":"running","last_scan_at":1760190123456,"restarts":[]},
 "processes":[{"pid":7777,"start":"w:133712349999999999","role":"desktop","version":"0.2.0"}]}
```

- `liveness` 的取值：
  - `live`：应答了，且身份相符；
  - `unresponsive`：进程在，但不应答；
  - `incompatible`：应答了，但版本不兼容，或自报的 pid 与记录不符；
  - `down`：进程不在；留有期望状态记录时 `restore_pending` 为 true；
  - 兜底 `unknown`。
- 只对 `unresponsive` 建议 `--force`（CP-07）。
- `unreadable` 里的码：`record_unreadable`、`record_schema_too_new`。
- `watchdog.state`：`running`，或 `stale`（记录还在，但进程不在或心跳过期）；没有看护者时为 null。
- `processes`：台账里有、但不属于上面任何实例或看护者的条目，例如安装者、桌面、未知角色。
- 不删任何文件（综C1）。

### 6.8 事件

| 事件 | topic | `data` | 何时发 |
|---|---|---|---|
| `request` | requests | 整条 RequestRecord，按 request_id 整条替换 | 请求开始、每次尝试开始与结束、进入退避、提交、结束 |
| `state` | state | 整份快照 | 状态变化时；活动计数变化时合并发送，最多每秒一次 |
| `log` | logs op 的连接 | `{"lines":[…]}` | 日志有新行 |
| `rotated` | logs op 的连接 | `{}` | 日志文件被轮转截断 |
| `heartbeat` | 全部 | `{}` | 15 秒内没有别的事件时 |
| `lagged` | 全部 | `{"dropped":n}` | 订阅者跟不上，事件被丢弃 |
| `closing` | 全部 | `{"reason":"shutdown"}` 或 `{"reason":"swap"}` | 实例退出前 |

热路径用 `broadcast`，没有订阅者时连事件对象都不构造；改动前后用 `examples/loadgen` 各测一次并记录（O2）。

**为什么**：
- 采纳 CP §3.4 的全部规则。其中「未知参数报错」最关键：否则以后带条件的 `shutdown` 发给 0.2.0 实例，会变成无条件停止（CP-03）。
- 请求与状态的事件只有「整条记录」与「整份快照」两类：客户端只做替换，丢一条增量也不会错乱，事件集也不会随字段增加而膨胀。
  否决 CP §3.7 的细粒度事件（request_started、attempt_failed 等）：每加一个阶段就要加一种事件，客户端还得自己拼出状态。
- 请求历史与请求体保留（G1）：界面打开之前发生的失败，正是用户最想看的。它们只在内存里，按条数与总字节封顶，重启即失。
- `prepare_swap` 保留，并加 ttl：「ACK 齐了才交换」是用户定的铁律，所以否决 D3 的简化。

---

## 7 config.toml（K3、K13、K14、D-1、format 定稿）

**现状**：
- 26 个平铺键（config.rs:191-385），外加三个 transform 段（config.rs:130-182）。
- `read_timeout_secs` 同时卡住首字节等待（proxy.rs:342，K13）。
- `bounded_retry_paths` 是唯一的限次出口（config.rs:294-309、retry.rs:53）。
- 未知键被静默忽略（CI-09）。
- 写回时整份重写，而且不是原子写（config.rs:939）。

### 7.1 键表

表中「defaults」一列表示该键能否写进 settings.json 的 `defaults` 节，「覆盖」一列表示 format 能否按请求覆盖它。

| 键 | 类型 | 默认 | defaults | start 旗标 | 覆盖 | 说明 |
|---|---|---|---|---|---|---|
| `base_url` | URL | 必填 | ✓ | `--base-url` | — | 只能是 http 或 https，不含 `?` 与 `#` |
| `listen_addr` | 地址 | `127.0.0.1:12345` | ✓ | `--listen-addr` | — | |
| `api_key` | 密钥 | — | ✓ | `--api-key` | — | 等效于覆盖 `Authorization: Bearer` |
| `extra_headers` | 表 | {} | ✓ | — | — | 请求没带这个头时追加 |
| `override_headers` | 表 | {} | ✓ | — | — | 无条件覆盖 |
| `proxy` | URL | 不设时用环境变量 | ✓ | `--proxy` | — | http、https、socks5、socks5h |
| `proxy_username` | 字符串 | — | ✓ | — | — | |
| `proxy_password` | 密钥 | — | ✓ | — | — | |
| `max_request_mb` | 整数 | 128 | ✓ | — | — | 原 `max_body_mb` |
| `max_response_mb` | 整数 | 256 | ✓ | — | — | 原 `spool_limit_mb`；0 改为不限 |
| `disk_cache` | 布尔 | true | ✓ | — | — | |
| `forward_only` | 布尔 | false | ✓ | — | — | |
| `allowed_hosts` | 字符串数组 | [] | ✓ | — | — | |
| `allowed_origins` | 字符串数组 | [] | ✓ | — | — | |
| `log_file` | 路径 | 随机名 | ✗（用户定） | `--log-file` | — | |
| `timeouts.connect_secs` | 整数 | 30 | ✓ | — | ✓ | 原 `connect_timeout_secs` |
| `timeouts.first_byte_secs` | 整数 | 300 | ✓ | — | ✓ | 拆自 `read_timeout_secs` |
| `timeouts.read_idle_secs` | 整数 | 300 | ✓ | — | ✓ | 拆自 `read_timeout_secs` |
| `keepalive.interval_secs` | 整数 | 15 | ✓ | — | ✓ | 原 `keepalive_interval_secs`；0 表示关闭 |
| `keepalive.trigger` | 字符串 | `any` | ✓ | — | ✓ | `accept`、`body_stream`、`any`，新增 `always` |
| `keepalive.heartbeat` | 字符串 | `": keepalive\n\n"` | ✓ | — | ✓ | 原 `keepalive_heartbeat`；校验规则同 config.rs:30 |
| `retry.max_backoff_secs` | 整数 | 320 | ✓ | — | ✓ | 原 `max_retry_backoff_secs` |
| `retry.jitter` | 布尔 | true | ✓ | — | ✓ | 新增 |
| `retry.respect_retry_after` | 布尔 | true | ✓ | — | ✓ | 新增 |
| `retry.rules` | 表数组 | [] | ✓ | — | — | 见 §7.3；取代 `bounded_retry_paths` |
| `retry.max_attempts` | 整数 | — | ✗ | — | ✓（只能由 format 覆盖） | 见 §7.3 |
| `history.requests` | 整数 | 500 | ✓ | — | — | 见 §6.6 |
| `history.bodies` | 布尔 | true（见 Q2） | ✓ | — | — | |
| `history.body_max_kb` | 整数 | 1024 | ✓ | — | — | |
| `history.body_budget_mb` | 整数 | 16 | ✓ | — | — | |
| `format` | 表数组 | [] | ✗（用户定） | — | — | 见 §7.4、§10 |

### 7.2 样例

```toml
base_url = "https://api.example.com"
listen_addr = "127.0.0.1:12345"
api_key = "sk-…"

[timeouts]
first_byte_secs = 600          # 长时间的非流式生成

[retry]
max_backoff_secs = 320

[[retry.rules]]
path = '/v1/messages/count_tokens(\?.*)?'
max_attempts = 3

[[retry.rules]]
status = [400, 401, 403, 404]
max_attempts = 5

[[format]]
name = "aproxy-format"
command = "aproxy-format"
args = ["run", "--config", "format.toml"]
mode = "persistent"
stages = ["request", "stream"]
```

### 7.3 重试（D-1）

**判定的优先级**，每次尝试失败后从高到低依次看：
1. format 的 decide（§10.5）；
2. 按请求覆盖的 `retry.max_attempts`；
3. 规则表；
4. 默认：无限重试。

**规则表** `[[retry.rules]]`：按书写顺序匹配，第一条所有匹配器都成立的规则生效，后面的不再看。

匹配器：
- `path`：对客户端请求的「路径?查询串」整体做正则匹配，自动锚定为 `^(?:…)$`，查询串不可省略。语义沿用 bounded_retry_paths（记忆 bounded-retry-paths）。
- `status`：上游状态码的数组。
- `body`：对解码后的上游响应体做非锚定的正则搜索。0.2.0 只定形状，配置里一出现就报 `config_not_supported`，0.2.x 再实现（D-1）。
- 没有任何匹配器的规则匹配一切。
- 规则只匹配有上游响应的失败，即 `upstream_status` 与 `upstream_error_body`；网络类与 format 类的失败不受规则限制。Q1 提议为此加 `codes`。

**`max_attempts`**（必填）：这类失败允许的总尝试数，包含第一次。
- 用尽时透传最后一次上游响应，终态码为 `retry_exhausted`，`details.by` 为 `rule:<序号>`。
- 0 表示不限，可以写在宽规则前面当作豁免。

**按请求覆盖** `retry.max_attempts`（只能由 format 给出）：
- 计入一切类型的失败。
- 用尽时的终态同上，`by` 为 `override`。
- 它不能写进配置文件；要全局封顶，就写一条没有匹配器的规则。

**退避**：
- 前 3 次立即重试（现行）；之后从 5 秒开始翻倍，封顶 `max_backoff_secs`（0 表示不等待）。
- `jitter = true` 时，每次的等待时间先乘以 [0.8, 1.2] 之间的随机系数，再封顶。
- `respect_retry_after = true` 时：
  - 上游的 Retry-After（秒数或 HTTP 日期）作为这一次等待的下限，但仍受封顶约束；
  - 带 Retry-After 的失败不走「前 3 次立即重试」。

「默认无限重试，不内置任何 URL」不变。

### 7.4 `[[format]]`

| 键 | 类型 | 默认 | 说明 |
|---|---|---|---|
| `name` | 字符串 | `command` 的文件名主干 | 在列表内唯一；日志、界面、`details.format` 都用它 |
| `command` | 路径或命令名 | 必填 | 相对路径相对 home；裸命令名先找 `<home>/bin/`，再找 PATH |
| `args` | 字符串数组 | [] | 打码规则见 §1.5 |
| `env` | 表 | {} | 追加给程序的环境变量；值只写不读 |
| `mode` | `spawn` 或 `persistent` | `spawn` | `spawn` 每条消息起一个进程；`persistent` 用进程池 |
| `stages` | 字符串数组 | 必填 | 取值 `request`、`retry`、`decide`、`response`、`stream`、`heartbeat`；`admit` 与 `notify` 已预留，一出现就报 `config_not_supported` |
| `required` | 布尔 | true | 为 false 时，这个程序出错就跳过这一层，不影响请求 |
| `pool_max` | 整数 | 4 | persistent 模式的进程池上限，0 表示不限；进行中的流各自独占一个 worker，不计入这个上限（§10.6） |
| `worker_idle_secs` | 整数 | 300 | 原 `idle_timeout_secs` |
| `timeout_secs` | 整数 | 30 | 每条消息的回信时限 |
| `decide_timeout_secs` | 整数 | 2 | decide 阶段的回信时限，起护栏作用 |

- **校验**：违反以下任一条报 `config_conflict`。
  - 同一个程序不能同时声明 `response` 与 `stream`；
  - 声明 `stream` 的程序必须是 `persistent`；
  - 仅转发模式下只允许 `request` 与 `stream` 两个阶段。
- **顺序**：列表顺序就是请求方向的顺序，离客户端最近的写在最前面。
- **取消的写法**：
  - `[request_transform]`、`[response_transform]`、`[heartbeat_transform]` 三段；
  - `every_attempt`，等价于 `stages` 里含 `retry`；
  - `extra`，改用 `args`、`env` 或程序自己的配置文件。
- **仅转发模式与 format 可以共存**（用户定）：声明了 `request` 阶段时，先把请求体读完（受 `max_request_mb` 限制）再转换。

### 7.5 解析、校验与诊断

- **单一管线** `config::resolve(path, overrides, settings) -> {config, sources, diagnostics}`：start、`__serve`、doctor、`config show --effective`、
  find 与桌面共用（综C9）。
- **层级**：start 的覆盖 > config.toml > settings 的 `defaults` > 内置默认。
  - 表按键合并；数组整体替换，例如 `retry.rules`、`allowed_hosts`。
  - `sources` 给出每个键的来源：`override`、`toml`、`settings` 或 `default`。
- **未知键**：报 `config_unknown_key` 告警，附上编辑距离不超过 2 的近似建议。
- **0.1.x 的旧键**：按用户的决定报 `config_removed_key` 告警，并附新写法；Q3 提议把其中承载行为的几个改为报错。
- 告警出现在 start（stderr 与实例日志）、doctor 与 `config show` 里。
- **旧键对照**，也写进 CHANGELOG 的迁移步骤：

| 0.1.x | 0.2.0 |
|---|---|
| `max_retry_backoff_secs` | `retry.max_backoff_secs` |
| `bounded_retry_paths = [p…]` | 每个 p 写一条 `[[retry.rules]]`，`path = p`、`max_attempts = 3` |
| `connect_timeout_secs` | `timeouts.connect_secs` |
| `read_timeout_secs` | `timeouts.first_byte_secs` 与 `timeouts.read_idle_secs` |
| `keepalive_interval_secs`、`keepalive_trigger`、`keepalive_heartbeat` | `keepalive.interval_secs`、`keepalive.trigger`、`keepalive.heartbeat` |
| `max_body_mb` | `max_request_mb` |
| `spool_limit_mb` | `max_response_mb`，0 的含义变为不限 |
| `[request_transform]` 等三段与 `every_attempt` | `[[format]]` 加 `stages` |
| transform 的 `idle_timeout_secs`、`extra` | `worker_idle_secs`；`args` 或 `env` |
| `upstream_url` | `base_url` |

- **补全校验**（综C9）：头名与头值、`api_key` 里的字符、`listen_addr`、规则的正则、状态码范围 100–599，以及 format 的各个键。错误都带行列。

### 7.6 写回（K7）

- 用 toml_edit 只改目标键；注释、键序与未知键原样保留。新建的文件只写显式给出的键。
- 原子写：先写 `*.tmp-<pid>`，再 rename；unix 上权限 0600。目标是符号链接时，写到它指向的真实文件。
- CAS：
  - 读入时记下内容哈希；rename 之前再读一次比对；
  - 不一致就重新读入、重新应用这次改动，最多 3 次；
  - 仍然冲突就报 `write_conflict`。
- 写入的结果先过一遍校验：
  - 这次改动引入了新的 error 级诊断，就拒绝写入（退出码 3）；
  - 文件里原本就有、与这次改动无关的错误只告警，不挡住逐项修复。

**为什么**：
- **只在语义成组的地方建表**（超时、保活、重试、历史、format），核心键保持平铺，常用写法不变。
  否决把核心键也收进表（例如 `[upstream] base_url`）：改动面大，收益小。
- **改名 `max_request_mb` 与 `max_response_mb`**：它们与终态码 `request_too_large`、`response_too_large` 同名同义；`spool` 是实现细节。
  `spool_limit_mb` 的 0 语义本来就要改（K14），改名也让旧值不会悄悄换了意思。
- **首字节超时与读空闲超时分开，默认值不变**（3.12）：在保活路径上，首字节超时是挂死上游的唯一出口，所以不改成 0；
  长时间的非流式生成按需调大 `first_byte_secs`。
- **新增 `trigger = "always"`**：给 format 按请求开启保活用，例如流式信号写在 URL 里的协议；配置里也能用，文档写明它会破坏非流式客户端的响应。
  否决同时加 `never`：它与 `interval_secs = 0` 重复。
- **规则取「第一条命中」而不是「取最严」**：结果可预测，还能写豁免。规则只匹配有上游响应的失败，沿用 bounded_retry_paths「网络错误不封顶」的定调。
- **jitter 与 Retry-After 默认开启**：它们不改变「重试与否」，只是让并发请求错开、尊重上游的限流提示（DP-26）。

---

## 8 settings.json（D-5、K12）

**现状**：
- 20 个平铺字段（settings.rs:159-266），混着程序状态、全局策略与 7 个 config 字段的全局默认。
- 任一字段解析失败，整份回退默认（settings.rs:349-362），例如 `watchdog:false` 会悄悄变回 true（CI-10）。
- 写回时整份序列化，丢掉未知字段；也没有 schema。

### 8.1 形状

```json
{"schema":1,
 "language":"system",
 "default_config":null,
 "aliases":{"work":"C:\\Users\\u\\.aproxy\\configs\\work.toml"},
 "config_dirs":[],
 "defaults":{"max_request_mb":128,"retry":{"max_backoff_secs":120}},
 "watchdog":{"enabled":true,"heartbeat_secs":30,"stale_after_cycles":1,"max_restarts":5,"idle_exit_secs":300},
 "logs":{"rotate_mb":8},
 "instances":{"idle_threshold_secs":1800},
 "update":{"download_chain":null,"download_proxy":null,"skill_auto_update":true},
 "ui":{"mode":"system","palette":"standard","mica":false,"mascot":"tumbler","theme":null}}
```

| 键 | 默认 | 说明 |
|---|---|---|
| `schema` | 1 | 写者遇到更高的值就拒绝写（`settings_schema_too_new`） |
| `language` | `system` | 取值 `system`、`en`、`zh-CN`；不认识的值按 `system` 处理，这样以后加语言不会破坏旧版本 |
| `default_config` | null | |
| `aliases` | {} | 名字到配置文件绝对路径的映射 |
| `config_dirs` | [] | |
| `defaults` | {} | 与 config.toml 同构，但不含 `format` 与 `log_file`（用户定） |
| `watchdog.*` | 见样例 | 原 `watchdog` 与 `watchdog_*` |
| `logs.rotate_mb` | 8 | 原 `log_rotate_mb` |
| `instances.idle_threshold_secs` | 1800 | 原 `idle_timeout_secs` |
| `update.*` | 见样例 | 原 `download_chain`、`download_proxy`、`skill_auto_update` |
| `ui.*` | 见样例 | 桌面界面的偏好，键由 W5 增补（A 级）；界面语言就用顶层的 `language` |

0.1.x 的平铺键（`watchdog` 为布尔、`log_rotate_mb`、`max_body_mb` 等）按 §7.5 的同一套规则报 `config_removed_key`，附新位置。

### 8.2 读

- 逐字段容错：坏字段报 `settings_invalid_value` 告警，用默认值代替，其余字段照常使用。
- 未知字段忽略，写回时保留。
- `download_chain` 里不认识的渠道跳过并告警。
- JSON 语法错误时：
  - 运行时全部按默认值处理，并告警；
  - 任何写操作都拒绝，报 `settings_invalid`、退出码 3，不让「回退默认值再写回」冲掉原文件。现行 settings.rs:673 正是这样做的。
- 守护与看护者每次需要时重读；看护者每个 tick 读一次。

### 8.3 写

- 在 `serde_json::Value` 上按路径打补丁（set 或 unset），开启 `preserve_order`，以保留键序与未知字段。新建的文件只写显式给出的键与 `schema`。
- 原子写、0600、CAS，规则同 §7.6。
- `defaults` 在配置解析里所处的层级见 §7.5。

**为什么**：
- settings.json 是同一台机器上所有版本共写的文件，以后再重整就得写双读代码（D-5）。分节之后，`settings get/set` 与界面设置页都能按节组织。
- 否决把界面偏好放进单独的文件：一个文件只需一套容错与 CAS 规则，界面与 CLI 都能读写它。
- `language` 取 `system` 而不是 `auto`：与原型的取值一致，读起来就是「跟随系统」。

---

## 9 命令行（D-3、D-4、D-7、K8、K11）

**现状**：
- 启动用的覆盖参数挂在顶层，`--config` 与 `--log-file` 是全局参数（cli.rs:14、cli.rs:33，CI-17）。
- `Upgrade` 是一个重复的变体（cli.rs:155-156）。
- `config` 有 15 个专用旗标。
- 裸 `aproxy` 等于启动（main.rs:178）。
- 退出码多为 0 或 1，status 与 restore 恒为 0；stop 在多处直接 `exit(1)`（stop.rs:77-169）。
- CLI 的 tracing 写到 stdout（server.rs:423）。
- 没有 `--json`。

### 9.1 命令树

```text
aproxy                          帮助写到 stderr，退出码 2（D-7）
aproxy start   [<ALIAS|PATH>] [--base-url <URL>] [--listen-addr <ADDR>] [--proxy <URL>]
               [--api-key <KEY|->] [--log-file <PATH>] [--foreground] [--json]
aproxy stop    [<PORT|ALIAS|all|idle>] [--idle-threshold-secs <N>] [--force] [--json]
aproxy restart [<PORT|ALIAS|all|idle>] [--idle-threshold-secs <N>] [--force] [--json]
aproxy status  [--idle|--busy] [--json]
aproxy logs    [<PORT|ALIAS>] [-n|--lines <N>] [-f|--follow]
aproxy restore [--json]
aproxy alias   add <NAME> [<PATH>] | remove <NAME> | list [--json]
aproxy config  show [--effective] [--json] | get <KEY> [--json] | set <KEY> <VALUE|-> | unset <KEY>
               （四个子命令都接受 --config <ALIAS|PATH>，默认是默认配置）
aproxy settings show [--json] | get <KEY> [--json] | set <KEY> <VALUE|-> | unset <KEY>
aproxy doctor  [--json]
aproxy find    [<QUERY>] [--aliased|--unaliased] [--port <PORT>] [--json]
aproxy install [<VERSION>|latest] [--pre] [--from <FILE>] [--adopt] [--abort] [--cpu …]
               [--allow-downgrade] [--download-proxy <URL>] [--no-skills] [--check] [--json]
aproxy install --force …        C7（F 级），语法不动
aproxy upgrade …                install 的 visible_alias
aproxy skill   install|upgrade|list|show …      由 W4 定
aproxy desktop                  由 W5 定；headless 版本给出说明后以退出码 1 退出
全局：--home <DIR>、-h/--help、-V/--version
隐藏：__serve、__watchdog、__install-resume（§5）
```

- `install` 的变化：
  - 删 `--skills-only`（W4）；
  - 内部用的 `--continue` 与 `--handover-from` 移进 `__install-resume`；
  - 加 `--check` 与 `--json`；
  - `--variant` 是否放开给普通安装，由 W5 的变体选择决定，属于加性放宽。
- `--json` 不做成全局旗标：全局旗标在不支持它的命令上会被接受后静默忽略，这正是 CI-17 要消除的问题。

### 9.2 启动顺序（main）

1. argv 恰好是单独一个 `--version` 或 `-V`：打印 C5 那一行，退出 0（现行 main.rs:27）。这一步不读 home，也不判定语言（G2）。
2. 预扫描 `--home <DIR>`：按 C1 的同一规则校验它是绝对路径，然后在建 tokio 运行时之前设进本进程的 APROXY_HOME。`set_var` 只在单线程时安全。
3. 第一个参数以 `__` 开头：分派到内部入口，不判定语言，也不构建帮助。
4. 预扫描到 `install` 或 `upgrade`，同时带 `--force`：语言只看 APROXY_LANG 与系统设置，因为强制安装不读 settings（C8）；然后走强制安装。
5. 判定语言：APROXY_LANG > settings 的 `language` > 系统界面语言 > en（W3）。
6. clap 解析，帮助按语言注入；`desktop` 在建运行时之前分派（W5）。
7. 按 C1 校验 home，失败报 `home_invalid`、退出码 2；然后分派。
   只有变更类命令、并且 install 状态可以接管时，才拉起 `__install-resume`；取消现行「任何命令都可能拉起续作」的做法（A5）。

### 9.3 config 与 settings 命令（D-4）

- **键的语法**：TOML 的点号路径，例如 `retry.max_backoff_secs`、`extra_headers."x-foo"`、`ui.mica`、`defaults.timeouts.connect_secs`。
  数组表（`retry.rules`、`format`）只能整体读写，值用 TOML 内联数组或 JSON；按下标改单个元素以后再加，属于加性变更。
- **值的解析**：按键的类型来。字符串键原样取；数字与布尔按字面解析；数组与表接受 TOML 内联写法或 JSON。
  只能设置已知的键，未知键报 `config_unknown_key`、退出码 3。
- **`-` 表示从 stdin 读一行**（去掉行尾换行），用来传密钥。帮助与 skill 推荐这种写法；直接把密钥写在命令行上照样接受。
- 密钥的 `get` 与 `show` 输出占位对象（§1.5）。
- `set` 与 `unset` 的 `--json` 输出：`{"schema":1,"file":"…","key":"…","value":…,"instances_to_restart":[12345]}`。
  `instances_to_restart` 列出正在用这个配置文件的实例：改动要在它们 restart 之后才生效。
- `config show --effective`：给出经 §7.5 管线解析后的生效值及其来源。
- **专用旗标一个不留，也不做别名**，对应关系例如：
  - `--baseurl` 改用 `set base_url`；
  - 设默认配置改用 `settings set default_config <PATH>`；
  - 清空 api_key 改用 `unset api_key`。

### 9.4 `--json` 的形状

共同规则：
- stdout 只有这一个 JSON 文档，带 `"schema":1`；install 例外，输出 NDJSON，每行一个对象。
- 给人看的提示写到 stderr。
- 整条命令失败时，文档是 `{"schema":1,"error":{"code","message","details"}}`，退出码按 §2.4。
- clap 的用法错误不受 `--json` 影响：照旧在 stderr 输出文本，退出码 2。

| 命令 | 成功时的文档 |
|---|---|
| `status` | §6.7 的枚举结果，外加 `"home"` |
| `start` | `{"outcome","port","pid","log_path","instance":{…}}`，`outcome` 为 `started` 或 `already_running` |
| `stop`、`restart`、`restore` | `{"results":[{"target","port","outcome","error"?}],"summary":{"ok":n,"failed":m}}`。`outcome` 的取值：stop 为 `stopped`、`already_stopped`、`killed`、`failed`；restart 为 `restarted`、`failed`；restore 为 `restored`、`already_running`、`discarded`（配置文件已不在）、`failed` |
| `alias list` | `{"aliases":[{"name","path","exists"}]}` |
| `config show` | `{"file","values":{…},"sources":{…}?,"diagnostics":[…]}` |
| `config get`、`settings get` | `{"key","value","source"?}` |
| `settings show` | `{"file","values":{…},"diagnostics":[…]}` |
| `doctor` | `{"files":[{"file","kind","diagnostics":[…]}],"summary":{"errors":n,"warnings":m}}`，`kind` 为 `config` 或 `settings` |
| `find` | `{"configs":[{"path","aliases":[…],"listen_addr","base_url","errors":n}]}` |
| `install --check` | `{"current","latest","update_available","channel","variant"}` |
| `install` | NDJSON，见下 |

`install` 的 NDJSON：
- 进度行：
  - `{"schema":1,"event":"phase","phase":"<install.state 的相位>","at":…}`；
  - `{"schema":1,"event":"progress","phase":"downloading","bytes":…,"total":…}`；
  - `{"schema":1,"event":"instance","port":…,"outcome":"restarted"}`，`outcome` 也可能是 `failed`。
- 末行：`{"schema":1,"event":"result","outcome":"installed","version":…}`；`outcome` 也可能是 `up_to_date` 或 `failed`，失败时另带 `error`。
- 桌面界面的升级就是拉起 `install --json` 并读这些行（CP §3.1），所以它是 A 级契约。

### 9.5 各命令的退出码

| 命令 | 0 | 1 | 2 | 3 | 4 | 5 |
|---|---|---|---|---|---|---|
| start | 已启动，或已在运行 | 端口被占、起不来、就绪超时 | 用法 | 配置无效，包括子进程报的配置类错误 | 别名或文件不存在 | — |
| stop | 已停，包括本来就停着 | 停不掉 | 多实例时没指定目标 | — | 别名不存在 | 多目标部分失败 |
| restart | 已重启 | 失败 | 同上 | 配置无效 | 目标没在运行或不存在 | 部分失败 |
| status | 枚举完成 | — | — | — | — | — |
| logs | 输出完毕 | 前台实例没有日志文件 | 多实例时没指定目标 | — | 没在运行，也没有记录可读 | — |
| restore | 全部恢复，或无事可做 | 全部失败 | — | — | — | 部分失败 |
| config、settings | 完成 | 写冲突、schema 过新 | 用法 | 结果无效或未知键 | 文件不存在（`get`） | — |
| doctor | 没有 error | — | — | 有 error | — | — |
| find | 完成 | — | — | — | — | — |
| install | 已安装或已是最新 | 失败，包括回滚 | 用法 | settings 无效 | 版本不存在 | — |
| desktop | 正常退出 | headless 版本、起不来 | — | — | — | — |

### 9.6 输出纪律

- stdout 只放主输出。CLI 进程的 tracing 一律写到 stderr，交互命令默认 WARN 级；现行写的是 stdout（server.rs:423）。
- `logs` 默认打印最近 200 行后退出，加 `-f` 才跟随；现行默认一直跟随。
  - 这样 agent 调用 `logs` 不会再挂住，也与 `docker logs`、`tail` 的习惯一致。
  - 对没在运行、但留有期望状态记录的实例，直接读记录里的 `log_path`，那里就是崩溃现场；stderr 提示实例未在运行。
- 所有 `--help` 的末尾都有一行固定英文的提示（W4）。

**为什么**：
- 退出码一公布就冻结；i18n 上线之后，agent 只能依靠退出码与 `--json`（D-3）。
- `stop` 幂等地返回 0，`--json` 里的 `already_stopped` 保留了细节。
- 旗标名由键名机械推出（`--base-url`、`--listen-addr`、`--idle-threshold-secs`），帮助、文档与界面映射都不必为谁破例（D-4）。
- 否决 `version --json`（CP §4）：C5 已经给出变体与目标，真需要时再加，属于加性变更。

---

## 10 format 协议 1（K10、D-6、format 定稿）

**现状**：
- 信封是 stdin/stdout 上的一行 JSON（aproxy-envelope/src/lib.rs:29-137），没有协议版本，也没有往返序号；transform.rs 的注释自述有串包的残余窗口。
- aproxy 发出的信封带 `stage` 字符串，但 aproxy-format 按有没有 `method` 来区分请求与响应（aproxy-format/src/main.rs:189-199，DP-16）。
- 回信的 `headers` 必填，缺 body 就等于空体（DP-13 第 6 点）。
- `RetryInfo.error` 与 `EnvelopeError` 是中文。
- `worker_id` 的文档说是槽位，实现却是取模自增（transform.rs:445）。
- 三段配置各指一个程序；仅转发模式与转换互斥。
- aproxy-format 把 SSE 原样透传（translate.rs:7-9）。
- crates.io 上只有 envelope 0.1.0。

### 10.1 传输

- 一行一个 JSON 对象，UTF-8 编码，以 `\n` 结尾。aproxy 写 stdin、读 stdout；程序的 stderr 进实例日志，前缀为 `format[<name>]`。
- 每条消息恰好一个回信，按顺序（预留的 notify 不回信，见 §10.9）。aproxy 发出的每条消息都带 `seq`，回信必须原样带回。
  `seq` 不符或缺失按协议错误处理：剔除这个 worker，记 `format_error`（kind 为 `protocol`）。这根治了串包窗口（DP-13 第 2 点）。
- 两种模式的会话：
  - persistent：一个 worker 的会话是先 `hello`，再处理消息，直到 stdin EOF。遇到 EOF 必须退出，这条铁律不变。
  - spawn：每个进程只收一行。实例启动时收的是 `hello` 探测；之后收的是一条阶段消息。回信后退出。
  - 同一个「读一行、回一行、直到 EOF」的循环，两种模式都适用。
- 公共字段，aproxy 发给 format：
  - `v`：协议版本；
  - `seq`：每个 worker 会话从 0 起递增；
  - `stage`；
  - `request_id`：全链共享；
  - `state`：这个程序在这个请求上存下的值；没有就省略。
- 公共字段，format 回给 aproxy：
  - `v`、`seq`；
  - 可选 `state`：字符串表示替换，空串表示清除，省略表示不变；上限 64 KiB，超出按 `invalid_output` 处理；
  - 可选 `report`（§10.8）。
- HTTP 对象：
  - 请求对象是 `{"method","url","headers":{小写名: 值},"body"|"body_b64"}`；响应对象是 `{"status","headers","body"|"body_b64"}`。
  - 回信里的同名对象是补丁：字段省略表示不变；给出 `headers` 就整表替换；`body` 与 `body_b64` 互斥，两者都省略表示不变，要清空就写 `"body":""`。
  - 多值头只保留第一个值，与现行一致。
- 字节流（stream 与 heartbeat 阶段）用 `data`（UTF-8）或 `data_b64` 表示。aproxy 会把块尾不完整的 UTF-8 序列留到下一块，能用 `data` 时就用 `data`。

### 10.2 hello 与能力协商

```json
→ {"v":1,"seq":0,"stage":"hello","protocols":[1],"aproxy_version":"0.2.0","name":"aproxy-format","mode":"persistent","stages":["request","stream"],"worker_id":0,"features":[]}
← {"v":1,"seq":0,"protocol":1,"stages":["request","retry","decide","response","stream","heartbeat"],"features":["decide.success"],"name":"aproxy-format","version":"0.2.0"}
```

- **hello 这一行的形状永久不变**（A 级），其中 `v` 恒为 1。
  - aproxy 在 `protocols` 里列出自己会说的协议版本，format 选一个，回在 `protocol` 里。
  - 之后每一行的 `v` 都是选定的版本。
  - aproxy 至少等 10 秒的 hello 回信。
- **`stages`**：
  - aproxy 发出的是配置里给这个程序声明的阶段，format 可以只启用这些模块；
  - format 回的是它支持的全部阶段；
  - 配置声明的某个阶段不在回信里，报 `format_stage_unsupported`。
- **`worker_id`**：0 到 pool_max 之间、在世 worker 之间唯一的槽位号，分配时取最小的空闲号（DP-13 第 8 点）；spawn 模式恒为 0。
  它只在 hello 里给出，因为一个会话内不会变。
- **`features`**：双方都列出的能力才生效。协议 1 定义了两个：
  - `decide.success`，由 format 声明：成功的尝试也送去 decide（§10.5）；
  - `stream.interleave`，R 级：一个 worker 交错处理多条流，以后实现。
- **时机**：
  - 实例启动时对每个程序握手一次；persistent 程序的这次探测进程留下来当第一个 worker。
  - required 程序握手失败，实例不启动，报 `format_unavailable` 或 `format_stage_unsupported`，退出码 3。失败的情形包括：起不来、10 秒内没回、回信不合法、协议或阶段不符。
  - 可选程序握手失败，只告警，状态记为 `unavailable`；之后每个新 worker 照常握手，成功了就恢复。
  - persistent 模式的每个新 worker 都先握手。
- **跨版本的兼容**：
  - 新 aproxy 遇到旧 format：hello 里多出的字段与 `protocols` 里的新版本都被忽略，format 回 1，双方就说协议 1。
  - 新 format 遇到旧 aproxy：format 只能选 aproxy 列出的版本。
  - 新阶段、新能力只在双方都声明时才使用，旧 format 永远收不到它不懂的消息。

### 10.3 链（洋葱）与 state

- **方向**：
  - 请求方向（request、retry）按 `[[format]]` 列表顺序调用；
  - 响应方向（response、stream、heartbeat）按相反顺序；
  - decide 按列表顺序，第一个给出明确决定的生效。
- 每个阶段只调用声明了它的程序；上一层的输出就是下一层的输入。
- **`state`** 按「程序 × 请求」隔离，每个程序只看到自己存下的值；`request_id` 全链共享。
- **重试时的链**：
  - 只要有程序声明了 `retry`，每次重试都从客户端的原始请求开始，把请求方向的整条链重跑一遍。
    声明了 `retry` 的程序收到 `stage:"retry"`，附带 §10.4 的 `retry` 上下文；其余程序再收一次 `stage:"request"`。
  - 没有任何程序声明 `retry`：沿用第一次的链输出。
  - 请求链在某次尝试里失败了：下一次尝试从头重跑。
- 链上每多一个程序，该阶段就多一次本机往返，文档写明这一点。

### 10.4 请求方向：request 与 retry

```json
→ {"v":1,"seq":7,"stage":"request","request_id":"19ac3f2e8b5d1a2b-42",
   "request":{"method":"POST","url":"https://api.example.com/v1/messages?beta=true","headers":{"content-type":"application/json","x-api-key":"sk-…"},"body":"{\"model\":\"claude-sonnet-4-5\",\"stream\":true}"},
   "client":{"path":"/v1/messages?beta=true","stream":true}}
← {"v":1,"seq":7,
   "request":{"url":"https://relay.example.com/v1/chat/completions","headers":{"content-type":"application/json","authorization":"Bearer k2-…"},"body":"{…}"},
   "overrides":{"timeouts":{"first_byte_secs":900},"retry":{"max_attempts":20}},
   "state":"{\"channel\":\"relay\",\"key\":\"k2\"}",
   "report":{"summary":"anthropic_messages -> openai_chat via relay; key k2","actions":[{"type":"protocol_convert","from":"anthropic_messages","to":"openai_chat"},{"type":"route","channel":"relay"},{"type":"key_select","key_id":"k2"}]}}
```

- **`request.url`**：输入里是 aproxy 算出的上游地址。回信里给出的 `url` 必须是绝对的 http 或 https URL，否则按输出不合法处理（`format_error`，kind 为 `invalid_output`）。
- **`client`**：客户端视角的信息，即原始路径与查询、是否流式；链上每一层看到的都相同。
- **retry 阶段**：输入多一个 `retry` 对象，例如 `{"attempt":3,"last_failure":{"code":"upstream_status","details":{"status":429}},"elapsed_ms":42000}`。
  其中 `attempt` 是即将进行的那次尝试的序号。
- **`overrides`**：按请求覆盖配置，键就是 config.toml 的键，范围是 §7.1「覆盖」一列，外加 `retry.max_attempts`。
  - 不认识的键或非法的值被忽略，并在日志里告警，不让请求因此失败。
  - 保活已经提交之后才回来的 `keepalive.trigger` 不生效。
- **失败**（D-6）：

```json
← {"v":1,"seq":7,"fail":{"retry":false,"code":"model_not_routed","reason":"no channel configured for model gpt-9","response":{"status":404,"headers":{"content-type":"application/json"},"body":"{\"type\":\"error\",\"error\":{\"type\":\"not_found_error\",\"message\":\"model gpt-9 is not configured\"}}"}}}
```

  - `retry:true`：计为一次失败的尝试（`format_fail`），进入正常的重试循环；退避、计数、decide 与规则都照常。
  - `retry:false`：
    - 带了 `response`，就原样返回它，并加上 §3 的两个头，码为 `format_rejected`；
    - 没带，就回 422 `format_rejected`。
  - `code` 是 format 自定的机读串，可以缺，进入 `details.fail_code`。
  - `reason` 用英文，不超过 512 个字符。
- **进程崩溃、超时、回信不合法**：一律按应重试处理（`format_error`）。仅转发模式没有重试循环，此时回 502 `format_failed`。
- **`required = false` 的程序**出任何错，都跳过这一层：把它的输入原样当作它的输出，并记一行告警。

### 10.5 decide

```json
→ {"v":1,"seq":12,"stage":"decide","request_id":"19ac3f2e8b5d1a2b-42","state":"…",
   "attempt":3,"elapsed_ms":42000,"outcome":"failure",
   "failure":{"code":"upstream_status","details":{"status":429}},
   "request":{"method":"POST","url":"https://relay.example.com/v1/chat/completions"},
   "response":{"status":429,"headers":{"retry-after":"20"},"body":"{\"error\":{\"message\":\"rate limited\"}}"},
   "fallback":{"decision":"retry","delay_ms":20000,"by":"default"}}
← {"v":1,"seq":12,"decision":"retry","delay_ms":60000,"report":{"summary":"429 on k2; cooling it down","actions":[{"type":"custom","label":"cooldown k2 60s"}]}}
```

- **时机**：每次尝试失败之后。声明了 `decide.success` 的程序在尝试成功之后也会收到，此时 `outcome` 为 `success`、没有 `failure`，`response` 带完整的体。
  这用来把「状态 200，但内容是错误」判为失败。
- **输入**：
  - `response`：有上游响应时给出，体是解码后的完整内容，上限与 response 阶段相同；没有响应（网络类失败）就省略。
  - `fallback`：不问 format 时 aproxy 会怎么做，即覆盖、规则表与默认三者得出的结论。
- **回信**：
  - `decision:"retry"`：重试。`delay_ms` 可选，取值 0–86 400 000；省略就用 `fallback.delay_ms`。
    对成功的尝试回 retry，就是把它判为失败（`format_decision`）；这个判决对这次尝试是终局，不会再问一遍。
  - `decision:"stop"`：停止。带了 `response` 就返回它，码为 `retry_stopped`；不带就透传上游的响应；没有上游响应时回 502 `retry_stopped`。
  - 省略 `decision`：表示不表态，交给下一个程序；所有程序都不表态，就用 `fallback`。
- **护栏**：
  - decide 超时（`decide_timeout_secs`，默认 2 秒）、崩溃或回信不合法，都视为不表态，并告警；绝不因为 decide 出错而结束请求。
  - 每个决定连同理由，都写进日志与请求记录的 `decisions`。

### 10.6 响应方向：response 与 stream

**response 阶段**：整体处理一个响应。

```json
→ {"v":1,"seq":15,"stage":"response","request_id":"…","state":"…","request":{"method":"POST","url":"https://relay.example.com/v1/chat/completions"},"response":{"status":200,"headers":{"content-type":"application/json"},"body":"{…openai…}"}}
← {"v":1,"seq":15,"response":{"headers":{"content-type":"application/json"},"body":"{…anthropic…}"}}
```

- **范围**：只处理 aproxy 判定为成功、要交给客户端的响应。
  - 放弃重试之后透传的上游错误不经过 response 阶段（X-3）。
  - 要按客户端协议改写错误，用 decide 的 `stop` 附带 `response`。
- **失败**：
  - 进程级失败：换一个新 worker 重做一次，仍然失败就跳过这一层。这延续了用户 09-23 的决定：响应侧失败时透传，并告警。
  - 回 `fail` 并带 `response`：用它。
  - 回 `fail` 不带 `response`：跳过这一层。
  - `fail.retry` 在响应方向不适用，因为判定点已经过去了。

**stream 阶段**：逐块处理，只有 `persistent` 程序能声明。

```json
→ {"v":1,"seq":20,"stage":"stream","event":"open","stream_id":"19ac3f2e8b5d1a2b-42/1","request_id":"…","state":"…","request":{"method":"POST","url":"https://relay.example.com/v1/chat/completions"},"response":{"status":200,"headers":{"content-type":"text/event-stream"}}}
← {"v":1,"seq":20,"response":{"headers":{"content-type":"text/event-stream"}},"abort_tail":"event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"api_error\",\"message\":\"stream conversion aborted\"}}\n\n"}
→ {"v":1,"seq":21,"stage":"stream","event":"data","stream_id":"19ac3f2e8b5d1a2b-42/1","data":"data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n"}
← {"v":1,"seq":21,"data":"event: content_block_delta\ndata: {…}\n\n"}
→ {"v":1,"seq":30,"stage":"stream","event":"close","stream_id":"19ac3f2e8b5d1a2b-42/1"}
← {"v":1,"seq":30,"data":"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n","state":"…"}
→ {"v":1,"seq":31,"stage":"stream","event":"abort","stream_id":"19ac3f2e8b5d1a2b-42/2","reason":{"code":"upstream_body_interrupted"}}
← {"v":1,"seq":31,"data":"…"}
```

- **消息**：
  - open 的回信可以改状态码与头；可以给出 `abort_tail`，即 format 中途失败时 aproxy 按客户端协议写出的收尾；也可以带初始的 `data`。
  - data 的回信可以不带任何字节。
  - 上游正常结束时发 close；上游中断或客户端断开时发 abort。客户端已经断开时，abort 的回信会被丢弃，这条消息只是给 format 一个清理的机会。
- **`stream_id`** 等于 `<request_id>/<attempt>`。
- **worker 的占用**：一条流在生命周期内固定由一个 worker 处理。
  - 没有 `stream.interleave` 时，它独占这个 worker 直到 close 或 abort。
  - 这些 worker 不计入 `pool_max`，否则长流会把池占满，饿死其他请求的 request 阶段。
- **字节**：交给 stream 阶段的永远是解码后的字节；输出给客户端时，去掉 `content-length` 与 `content-encoding`。
- **两种模式**：
  - 缓冲模式也走 stream 阶段，回放时边放边转。「状态 200 但内容是错误」等判定，在转换之前对上游的原始数据做。
  - 响应头已经提交（骨架或上一次尝试）之后，open 回信里的状态与头被忽略，open 只用来初始化 format 的流状态。
  - 仅转发模式的流直接经过这一阶段实时转换。
- **失败**：
  - open 时失败：与 response 阶段相同，重做一次，仍失败就跳过这一层。
  - 流中途某一层失败：aproxy 停止读上游，向失败层外侧的各层依次发 abort，它们回的字节照常向外传；失败的是最外层时，写它在 open 时给的 `abort_tail`。
  - 可选程序在流中途失败：之后的块都跳过这一层。
- **混用**：同一个程序不能同时声明 response 与 stream（§7.4），但链上可以混用；缓冲模式里，aproxy 在两种形态之间按需拼接或拆分。

### 10.7 heartbeat

```json
→ {"v":1,"seq":40,"stage":"heartbeat","request_id":"…","state":"…","heartbeat":{"beat":3,"elapsed_ms":45000,"attempt":2,"last_failure":{"code":"upstream_status","details":{"status":529}}},"data":": keepalive\n\n"}
← {"v":1,"seq":40,"data":"event: ping\ndata: {\"type\":\"ping\"}\n\n"}
```

- **`data`**：最内层收到的是 `keepalive.heartbeat` 的静态字节，之后每一层收到它内侧那一层的输出；回信省略 `data` 表示不变。
- **`beat` 为 1 的那一拍**另带 `request`，即客户端的请求，含请求体；之后各拍不再重复，需要的信息请写进 `state`。
- **`last_failure`**：最近一次尝试的失败。它补上 TODO 里「心跳信封缺上游失败摘要」这一条。
- **迟到与失败**：一拍没在下一拍之前回来，就照发上一层的字节；出错时，这一拍用输入的字节。

### 10.8 report

- 形状是 `{"summary":"<一句英文，最多 512 个字符>","actions":[{"type":"…",…,"label"?}]}`，`actions` 最多 32 条，超出就截断并告警。
  任何阶段的回信里都可以带 report。
- 动作类型表只增不改。遇到不认识的类型，显示它的 `label`；没有 `label` 就显示类型名。

| type | 字段 |
|---|---|
| `protocol_convert` | `from`、`to`：协议标识，开放词表，例如 `anthropic_messages`、`openai_chat`、`openai_responses`、`gemini` |
| `key_select` | `key_id`：key 的编号或别名，绝不是 key 本身 |
| `route` | `channel`、`model_from?`、`model_to?` |
| `header_edit` | `name`、`op`（`set` 或 `remove`） |
| `body_edit` | `path`（JSON Pointer）、`op`（`set` 或 `remove`） |
| `custom` | `label` |

- 说明里不得出现明文密钥，协议文档写明这一点；aproxy 另外再做一次替换（§1.5）。
- aproxy 在每个请求结束时记一行日志，列出各程序的 summary；界面的「转换」页签按步显示请求记录里的 `reports`。

### 10.9 预留：admit 与 notify（只定形状，在 0.2.0 之后实现）

**admit**：每次发往上游之前，问 format「现在能发吗」。

```json
→ {"v":1,"seq":50,"stage":"admit","request_id":"…","state":"…","attempt":1,"request":{"method":"POST","url":"…"},"queue":{"length":3,"position":1,"waited_ms":1200}}
← {"v":1,"seq":50,"admit":false,"retry_after_ms":5000,"reason":"5h quota window full"}
← {"v":1,"seq":50,"admit":true,"grant":{"count":10,"ttl_ms":60000}}
```

- 排队不算失败，客户端的心跳照发。
- 只问队首；`grant` 表示一次批准接下来 N 个请求，或 T 时间内的请求。
- 护栏：format 崩溃或超时，回落到静态的并发上限；这个上限在实现时以加性的配置键给出，没配就放行。

**notify**：只通知，不等回信。format 不得回复，回复了按协议错误处理。

```json
→ {"v":1,"seq":51,"stage":"notify","event":"attempt_end","request_id":"…","attempt":1,"at":1760190000999,"outcome":"failure","failure":{"code":"upstream_status","details":{"status":429}},"response":{"status":429,"headers":{"x-ratelimit-remaining-requests":"0"}}}
```

- 事件有 `attempt_start`、`attempt_end`；后者带上游的响应头，供限流与额度规则使用。以后可以再加事件。
- 订阅额度相关的头怎么读，先实测真实响应再定。

### 10.10 aproxy-envelope 0.2.0

- **类型**：`Hello` 与 `HelloReply`、各阶段的输入与回信、`HttpRequest` 与 `HttpResponse`（含补丁形态）、`Fail`、`Overrides`、`Report` 与 `Action`、`StreamEvent`。
- 结构体一律标 `#[non_exhaustive]`，并提供构造函数与 builder。这样今后加字段，就不再迫使 crate 升次版本（DP-13 第 7 点）。
- `Display` 全部用英文。
- **版本**：预发布期间随主线用 `0.2.0-alpha.N` 或 `0.2.0-rc.N`，与 aproxy 0.2.0 同期发布 0.2.0（RT-06）；aproxy-format 同步发布 0.2.0。

**为什么**：
- **hello 是信封冻结之后仍能演进的关键**（用户定）：新阶段、新能力只在双方都声明时才使用。
- **`seq` 必须回显**：DP-13 原建议是「不带就照旧接受」。但协议 1 是全新的契约，本来就要求 hello，没有需要兼容的第三方实现；只有强制回显才能根治串包。
- **spawn 模式不额外握手**：保住「读一行、回一行」的最低门槛；程序的能力在启动探测时就已经知道了。
- **回信省略表示不变**：只想换 key 的 format，不必把几 MB 的体原样写回（DP-13 第 6 点）。
- **流的 worker 不计入池上限**：用户定「一条流由一个进程处理」，多路复用以后再加。在那之前如果计入池上限，超过 `pool_max` 条的并发长流会让新请求一直等在 request 阶段。
- **否决只给 decide 送体的摘要**：「200 但内容是错误」的标志可能在流的末尾（例如 `response.failed`，DP-25），只给开头不够。
  成功时也送 decide 是显式的选择（`decide.success`），成本由做这个选择的人承担。
- **否决让 response 阶段的 fail 也能要求重试**：判定点在 decide；两处都能触发重试，就会出现互相矛盾的结论。

---

## 11 冻结清单

0.2.0 发布时，据此重写记忆 ipc-v1-upgrade-contract。

| 契约 | 级 | 形状所在 |
|---|---|---|
| C1 home、C2 bin 交换、C3 台账、C4 install/ 与锁、C5 `--version`、C6 发布清单、C7 强制安装的命令行与退出码 10–12、C8 强制安装的流程 | F | 计划 W2b |
| 错误码表；各码对应的 HTTP 状态与退出码；`details` 的键 | A | §2 |
| HTTP 错误体；`x-aproxy-error` 与 `x-aproxy-request-id`；SSE 终态事件 | A | §3 |
| `request_id`（不透明串）与 `instance_id` | A | §1.3 |
| home 唯一根与 `--home`；目录与文件名；记录 schema 1（instance、restore、spec、watchdog）；心跳与宣告的名字与二进制布局；`home_id` 算法与端点名；spool 与日志 GC 的规则；日志的固定名与随机名模式；install/state.json 的字段与相位词表 | A | §4 |
| 内部入口 `__serve`、`__watchdog`、`__install-resume` 及其参数 | A | §5 |
| IPC v1：帧、规则、op 与参数、结果形状、错误码、事件、`features` 名、流式连接上至少每 15 秒一行 | A | §6 |
| config.toml 的键、类型、默认值、0 的含义、层级与合并规则 | A | §7 |
| settings.json schema 1 | A | §8 |
| 命令树、旗标、退出码表、各命令的 `--json`、install 的 NDJSON、stdout 与 stderr 的纪律 | A | §9 |
| format 协议 1：hello、`seq`、各阶段的消息、补丁语义、fail、overrides、report 的动作类型、`features` 名、hello 至少等 10 秒 | A | §10 |
| 日志行里的 `code=` 与 `request_id=` 字段 | A | §1.1 |
| 所有 `message` 文本、i18n 目录、日志行的其余部分、调参常量（GC 宽限 10 分钟、流式连接上限 8 等）、doctor 的检查项 | E | — |
| `watchdog.sock` 与 `\\.\pipe\aproxy-<home_id>-watchdog`；IPC 的 `reload`；`shutdown.drain_secs`；阶段 `admit` 与 `notify`；能力 `stream.interleave`；规则的 `body` 匹配器 | R | §4、§6、§7、§10 |

---

## 12 审查对账

### 12.1 综合稿归 W2 的条目

| 编号 | 着落 | 0.2.0 |
|---|---|---|
| K1 码表 | §2 | 做 |
| K2 错误体 | §3；头的范围比原建议多了「放弃后透传」 | 做 |
| K3 / D-1 重试形状 | §7.3 | 做；`body` 匹配器 0.2.x 实现 |
| K4 IPC | §6 | 做 |
| K5 持久层 | §4 | 做 |
| K6 / D-2 拉起与期望状态 | §4.2、§5 | 做 |
| K7 写回保真 | §7.6、§8.3 | 做 |
| K8 / D-3 机读面 | §2.4、§9.4–9.6 | 做 |
| K9 / D-4 config 命令面 | §9.3 | 做 |
| K10 信封 | §10；DP-13 的八项：v、seq、补丁语义与响应的 status 在 §10.1，reason 改为码在 §10.4 的 `last_failure`，英文与 non_exhaustive 在 §10.10，worker_id 在 §10.2 | 做 |
| K11 / D-7 CLI 形状 | §9.1、§9.2 | 做 |
| K12 / D-5 settings | §8 | 做 |
| K13 超时拆分 | §7.1 | 做 |
| K14 0 与单位 | §1.2 | 做 |
| D-6 请求侧转换失败 | §10.4 | 做 |
| D-8 看护者慢速重试 | §5 | 做 |
| D-9 skill references 不留存 0.1.0 版 | 不属于契约形状，W4/W7 执行 | — |
| D-10 预发布节奏 | 被「先发 alpha.1」取代 | — |
| D-11 / W2b | 不改，只协调（§4.1） | — |

### 12.2 四份报告的原编号

| 原编号 | 着落 |
|---|---|
| DP-01、DP-02 | §3 |
| DP-03 | §2 |
| DP-04、DP-26、X-4 | §7.3 |
| DP-25 | 由 format 的 `decide.success` 承担；规则的 `body` 匹配器在 0.2.x 实现 |
| DP-05、综C6 | 被 D-6 取代（`format_error`/`invalid_output`），见 §2 的「为什么」与 Q1 |
| DP-06、综C7 | 码 `upgrade_unsupported`（§2.1）；行为由 W6c 实现 |
| DP-07 | §7.1 |
| DP-11、O1 | `request_id`（§1.3） |
| DP-12、O2、DP §8 | §6.5（`in_flight`、`retrying`）、§6.6、§6.8 |
| DP-13 | §10 |
| DP-14 | §1.2、§7.1 |
| DP-24、综C12 | 码 `local_storage_failed` |
| DP-27、综C10 | 码 `shutting_down`（§2.1）；停止语义见 §5 |
| X-1、X-2 | D-6，见 §10.4；422（§2） |
| X-3 | 0.2.0 不做，由 decide 的 `stop` 附带响应覆盖这个需求（§10.6） |
| CP-01 | §4.3 |
| CP-02、CP-14、CI-02、CI-03 | §4.2、§5 |
| CP-03、CP §3 | §6 |
| CP-04、CP §8、CI-13 | §9.4–9.6 |
| CP-05 | §4（`--home`，删除 APROXY_RUN_DIR） |
| CP-06、CP-07、综C1 | §4.3、§6.7 |
| CP-09、SE1、CI-14（权限部分） | §1.6 |
| CP-10 | §6.5 |
| CP-11、CP-18、综C2 | §5 |
| CP-12、CP-23、综C3 | §5 |
| CP-13、综C4 | §5 |
| CP-15、CP-16、综C5 | §4.1、§4.3 |
| CP-21 | §5 |
| CP-22 | §1.2（`start`） |
| CP-24 | §4.2 |
| CP-25 | §6.4 |
| CP-26 | §4.3 |
| CP-27、CI-17 | §9.1 |
| CP §4（永久接口清单） | §11 |
| CP §5（磁盘布局） | §4.1 |
| CI-04、CI-10 | §7.6、§8 |
| CI-05、D1 | §9.3（D-4 已定 `--base-url`） |
| CI-08、CI-09、综C9 | §7.5 |
| CI-11 | §7.1、§8 |
| CI-12（码的部分）、RT-16 第 1 点 | §2、§1.4 |
| CI-14（打码与 stdin 部分）、CI-22、SE2 | §1.5、§9.3 |
| D2 | D-2 |
| D3 | 不采纳；`prepare_swap` 加 `ttl_secs`（§6.4） |
| D4 | D-7 |
| RT-06 | §10.10 |
| A5 | §4.2（install.state） |
| TODO 第 44 行（format 扩展的开放问题） | state 上限 64 KiB（§10.1）；心跳带 `last_failure`（§10.7） |
| TODO 第 247 行（模式标识） | §6.5 的 `mode` |

### 12.3 综合时发现的缺口（G1–G9）

| 编号 | 着落 |
|---|---|
| G1 UI 日志的数据源 | §6.4、§6.6；`[history]` 见 §7.1；默认值见 Q2 |
| G2 `--version` 先于语言判定 | §9.2 第 1 步 |
| G3 台账与实例记录分开 | §4.1 |
| G4 两层表述 | §1.1 |
| G5 退出码区间 | §2.4 |
| G6 `--version` 括号里不得出现形似版本号的词 | 属于 C5，已由 W2b 落地，本稿不动 |
| G7 feature 名统一为 `desktop` | 不属于本稿；命令树沿用 `desktop`（§9.1） |
| G8 强制安装测试的安全守卫 | 属于 W2b 的测试 |
| G9 扩展同意门的位置 | §1.6 |

### 12.4 与综合稿 3.12 建议不同的地方

| 3.12 的建议 | 本稿 | 理由 |
|---|---|---|
| 宣告改用 install/lock 的持锁状态 | unix 宣告改为 `install/announce` 文件，Windows 不变 | 锁只在修改 bin/ 期间持有，交接时还会换持有者（§4） |
| `x-aproxy-*` 头只加在自产响应上 | 放弃之后透传的响应也加 | §3 的「为什么」 |
| 内部入口 `__serve --spec <文件>` | 另加 `--home` | 子进程不依赖继承来的环境变量去定位 home |
| `.restore` 结构 `{schema, config_path, overrides, log_path}` | 改为 `{schema, spec, written_at}`，spec 里另含 `env` 与 `foreground` | D-2 已定要保存环境；spec 文件与期望状态记录共用同一个形状 |
| 不建 aproxy-ipc crate | 码表放进 aproxy-ipc | 用户 10-11 定新增这个 crate（W5） |

### 12.5 不在 0.2.0 做的

| 项 | 理由 |
|---|---|
| IPC `reload`（配置热重载） | 0.2.0 的界面改配置走「写文件 + restart」；以后加 op 属于加性变更（3.12） |
| `shutdown.drain_secs` | 没有需求；参数名已预留 |
| admit、notify 的实现；官方 aproxy-format 的规则实现 | 用户定排在 0.2.0 之后；形状已定（§10.9） |
| `stream.interleave` | 用户定以后再加；0.2.0 用「每条流独占一个 worker、不计入池上限」避免饿死 |
| 规则的 `body` 匹配器 | D-1 定 0.2.x 实现 |
| 错误响应交给 format 转换（X-3） | decide 的 `stop` 附带响应已能覆盖；以后加开关属于加性变更 |
| `version --json` | C5 已够用 |
| `config set` 按下标改数组元素；start 的通用 `--set` | 不为假想的需求设计，以后加属于加性变更 |
| home 级看护者端点 | 只预留名字；看护者的状态先由 `run/watchdog.json` 提供 |
| Web UI | 计划 W5 定暂不做 |

---

## 13 需要用户拍板的问题

### Q1 规则表加 `codes` 匹配器

- **背景**：D-6 定「format 进程崩溃、超时、输出不合法一律按应重试」；D-1 的规则只匹配有上游响应的失败。两者相加，有两种情况会让请求无限重试，
  agent 一直挂在心跳里，而没有任何配置能封顶：
  - format 确定性地坏掉，例如每次都改写出一个相对 URL（综C6 的场景）；
  - 上游永远连不上。
- **推荐**：规则加一个匹配器 `codes = ["format_error", …]`，取值是尝试失败码的数组；带 `codes` 的规则可以匹配没有上游响应的失败。
  - 默认不配任何规则，行为与 D-1、D-6 完全一致。
  - 文档给出配方，例如「format 连续出错 20 次就停」。
- **取舍**：键表与校验多一个匹配器，换来「无限重试」的每一类出口都能由配置封顶。不加的话，这两类失败只能靠 format 自己的 decide
  或 `retry.max_attempts` 覆盖来封顶，而 format 坏掉时这两条路都会失效。

### Q2 请求体与响应体是否默认在内存里保留

- **背景**：界面要能点开一条失败的请求查看它的体（G1），而用户最想看的往往是界面打开之前发生的那些。默认保留意味着：
  - 每个实例常驻最多 `history.body_budget_mb`（默认 16 MiB）的最近请求内容，headless 服务器也一样；
  - 这些体只在内存里，只能经访问控制之后的 IPC 按需读取，重启即失。
- **推荐**：默认开启。`history.bodies = true`，单个体上限 1 MiB，总预算 16 MiB。
- **取舍**：开启的代价是内存，以及敏感内容在进程内多留一段时间；换来的是一打开界面就能排障。
  默认关闭的话，界面得先提示用户打开开关，再复现一次问题。

### Q3 0.1.x 旧键中「承载行为」的那几个：告警还是拒绝启动

- **背景**：D-1 与 format 定稿都定了「旧键删除，在未知键告警里提示新写法」。细化时发现，其中有四个键被忽略后，会悄悄改变流量的去向或重试的行为：
  - `[request_transform]`、`[response_transform]`、`[heartbeat_transform]` 被忽略后，请求不经协议转换就发往上游，上游回 4xx，
    于是无限重试，agent 挂住；
  - `bounded_retry_paths`（无论在 config.toml 还是 settings.json 里）被忽略后，count_tokens 一类确定性的 404 又变回无限重试，
    这正是它当初要解决的问题。
  - 看护者重拉出来的实例只在日志里留下告警，用户与 agent 都看不到。
- **推荐**：这四个键报 `config_removed_key` 错误、拒绝启动（退出码 3），错误里写出等价的新写法；其余只是改了名、忽略后会回到默认值的旧键，
  仍按用户的决定只告警。
- **取舍**：旧配置第一次在 0.2.0 上启动就会失败，必须改完才能用；不这样做，就要承受半工作状态下的无声故障。0.1.0 用户本来就要按迁移步骤重装。

---

## 14 实现顺序与切分

**前置**（来自综合稿 §2.3、§2.4，不属于本稿的块）：
- S1：proxy.rs 拆分，合并成单一的 attempt 循环。数据面的每一块都等它。
- S2：控制面拆分，spawn 合一。
- S3：操作层，lib 不打印、不退出。
- R1：`tests/common`，统一注入 `APROXY_HOME` 与 `APROXY_LANG=en`。
- R2b：大版本依赖升级，其中 toml 的升级决定 toml_edit 用哪个版本。

**通用纪律**：
- 每块改了对外行为，skill 就在同一个提交或紧随其后的提交里同步（记忆 aproxy-cli-skill-versioning）。
- 测试一律用隔离的 home，停实例只按端口。
- headless 与 desktop 两个变体都要通过 clippy `-D warnings`。

| 块 | 内容 | 依赖 | 验收 |
|---|---|---|---|
| B0 | 新建 aproxy-ipc：码表（§2）、全部线上与记录类型（§4.2、§6）、`redact`、端点名与 `home_id` | 无，可以立即开工 | 依赖只有 serde、serde_json、tokio、sha2，以及钉版的 windows-sys；§2、§4、§6 的每个样例都有 golden 往返测试；测试断言码唯一、是 snake_case，并且每个码都按族带有 HTTP 状态或退出码；从码表渲染的文档与仓库里的 docs 文件一致 |
| B1 | 数据面的错误出口：§2.1、§2.2、§3、`request_id` | S1、B0 | 每个可以触发的自产 4xx/5xx 都有集成测试，断言体是 JSON、类型带 `aproxy_`、两个头齐全；源码里除了那一个构造函数，不再有手写的 `event: error`；同一个请求的头、体与日志里 `request_id` 相同；用真实的 Anthropic 与 OpenAI SDK 各解析一次，结果记进计划 |
| B2 | config.toml：§7.1、§7.4（配置形状）、§7.5、§7.6 | R2b、B0 | §7.2 的样例能解析；每个旧键都给出带 `replacement` 的诊断；未知键给出建议；四层来源 `sources` 正确；写回前后的 golden 对比里注释与未知键都还在；有 CAS 冲突测试；unix 上有符号链接目标的测试 |
| B3 | settings.json：§8 | B0 | 有坏字段时其余字段照用（例如 `watchdog` 类型不对，别名不丢）；`settings set` 之后未知字段还在；schema 为 2 的文件拒绝写；语法错误的文件拒绝写 |
| B4 | 重试与超时语义：§7.3、K13、K14 | S1、B2 | 规则第一条命中生效，`max_attempts = 0` 起豁免作用；覆盖计入一切失败；Retry-After 作为下限并受封顶约束；jitter 落在 [0.8, 1.2]（测试里用确定的随机源）；首字节超时与读空闲超时各报各的码；`max_response_mb = 0` 表示不限 |
| B5 | 持久层与拉起：§4、§5、`--home`、删除 APROXY_RUN_DIR、内部子命令 | S2、B0、B3 | `tests/contract_v0_2.rs` 钉住全部记录；schema 为 2 或乱码的记录不被删除；两个平台上守护与看护者的命令行里都没有密钥；restart 之后环境不变（启动时设了 `HTTPS_PROXY`，从不带它的环境发起 restart，实例仍走代理）；ubuntu 上向启动者的进程组发 SIGINT 后实例仍在；Windows 上 BREAKAWAY 被拒时能回退；spool 按进程分目录；日志 GC 不碰自定义 `log_file`，并遵守宽限期 |
| B6 | IPC v1 定稿：§6.1–6.5、§6.7 | B5 | 每个 op 的请求、成功应答、每种错误应答都有 golden；未知参数回 `unsupported_arg`；模拟新客户端连旧服务端、旧客户端连新服务端，多出的字段与事件都被忽略；`base_url` 带 `?key=` 时快照里是打码后的值；枚举能分出四种状态，`incompatible` 不建议 `--force` |
| B7 | 访问控制：§1.6 | B6 | Windows 上用 GetSecurityInfo 读出管道的 DACL，只有当前用户 SID 与 SYSTEM；ubuntu 上 run/ 为 0700、socket 为 0600；peer uid 的核对有单测，CI 允许时另做跨用户的连接测试；`request.bodies` 只在这一块之后出现在 `features` 里 |
| B8 | 请求历史、事件与日志流：§6.4 的 requests、request、subscribe、logs，以及 §6.6、§6.8 | S1、B6、B7 | 改动前后用 loadgen 各测一次并记录数据；没有订阅者时不构造事件（计数器测试）；慢订阅者收到 `lagged`；长时间的单次尝试不被判为闲置；按条数淘汰与按体预算淘汰都生效，后者表现为 `bodies` 变为 `evicted` |
| B9 | 命令行：§9 | S3、B2、B3、B5、B6 | §9.5 的每一格都有测试；每个命令的 `--json` 都有 golden；带 `RUST_LOG=info` 运行时 stdout 里没有日志行；裸 `aproxy` 退出 2，帮助在 stderr；有遍历整个命令树的帮助测试；`config set api_key -` 从 stdin 读，且从不回显；`logs` 默认打印完就退出 |
| B10 | format 协议 1：envelope 0.2.0 与 aproxy 一侧的实现，§10 | S1、B1、B4 | §10 的样例成为 aproxy-envelope 的 golden；format-echo 升级到协议 1，覆盖坏 `seq`、缺 hello、中途崩溃等情形；有链顺序测试（请求顺序、响应反序、decide 第一个表态者生效）；失败语义的每一格都有测试；仅转发模式加 stream 转换有端到端测试；`required = false` 会跳过；decide 超时会回落 |
| B11 | aproxy-format 接到协议 1：握手、`seq`、按 `stage` 分派、stream 接线 | B10 | 通过 B10 的链测试；转换的正确性（三种协议的流，对照 SDK 的解析器）属于 W2c 的验收 |
| B12 | 文档、skill 与记忆 | 随各块进行 | 码表与 CLI 写进 skill，SK3 的校验脚本进 CI；CHANGELOG 写好迁移对照（§7.5）；0.2.0 发布时按 §11 重写 ipc-v1-upgrade-contract |

**并行编排**（总并发不超过 5）：
- 可以立刻开工的只有 B0。B0 合入之后 B3 就能开工；B2 还要等 R2b 里 toml 的升级。
- 数据面一条线：B1 → B4 → B10 → B11，都在 S1 之后。
- 控制面一条线：B5 → B6 → B7 → B8，都在 S2 之后。
- B9 在 S3 与上面几块之后。
- B12 贯穿始终。
