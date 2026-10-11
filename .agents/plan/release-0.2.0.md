# 0.2.0 计划

> 状态（2026-10-11）：方向已由用户定下（见记忆 [0.2.0-direction](../memory/2026-10-11-0.2.0-direction.md)）；
> 四路审查与综合已完成：`.agents/review/2026-10-11-0.2.0-audit/`（dataplane、control-plane、config-cli-install、
> release-tests-docs，综合稿 synthesis.md：合并后 P0 16 条、工作流归属、待拍板决策 D-1…D-11、主代理可自定的
> 设计点 §3.12）。综合稿的 D-10（alpha 不公开发布）被用户「先发 alpha.1 验证可信发布」的定调取代；§3.12 里
> 「不建 aproxy-ipc / aproxy-ui crate」被用户「新增两个 crate」的提议取代（见 W5）。W1 实现中。

## 目标

0.2.0 是去兼容、改契约的最后一次廉价机会：发布后 IPC、磁盘布局、config.toml / settings.json、CLI、HTTP
错误体、信封都要长期稳定。所以先把这些契约重新设计到满意，再在上面做 i18n、skill 命令与 UI。

## 发版节奏（用户 2026-10-11）

功能很多，所以先发一个测试版 **v0.2.0-alpha.1**，内容是早期 UI 原型 + 强制安装，同时验证可信发布（10 个 headless npm
包第一次经 OIDC 发布、crates.io、带 UI/无 UI 双矩阵、发布清单）。之后剩余内容逐项完成，主代理在值得的节点自行打 tag 发版。

**alpha.1 的范围与顺序**（本机空闲内存约 3 GB，任何时刻只跑一个 cargo 重编译；不编译的工作可并行）：
1. W1 去兼容 + 版本号改 0.2.0-alpha.0 + `--version` 新格式（C5）+ 安装状态迁到 `<home>/install/`（C4 的路径部分）+
   版本下限 0.2.0 + CI 的 upgrade-from-0-1-0 改为断言「0.1.0 干净拒绝」。
2. 进程台账 + 强制安装（C1–C8 全部，含锁、唯一中间名、契约测试、注释、AGENTS.md 一句）。
3. Tauri 外壳：`desktop` feature（默认开）、`aproxy desktop` 打开原型界面（假数据）、无 UI 版的 `desktop` 提示、build.rs、
   Tauri 只进 bin 不进 lib 与测试（RT-05）、自定义标题栏与 Mica（默认关）按两个 skill 落地。
4. 发布流水线：22 个产物双矩阵（Linux 带 UI 版在 ubuntu-22.04 原生构建，musl 带 UI 版在 Alpine 容器）、前端构建步骤、
   发布清单 `aproxy-manifest.json`、npm headless 组、crates.io 带前端产物发布；CI 先在推送时构建两种变体验证。
5. CHANGELOG、skill（强制安装用法、迁移步骤）、文档的最小更新 → 打 `v0.2.0-alpha.1` → 盯 Release 与发布结果。

## 工作流（顺序暂定，审查结果回来后细化）

### W1 去除兼容
- 删 `src/compat_0_1_0.rs`（S1–S5）、`continue_install` 里的 relaying 豁免、`tests/compat_v0_1_0.rs`、CI 的
  upgrade-from-0-1-0 任务与 `tests/upgrade_from_0_1_0.rs`。
- 0.1.0 用户执行 `aproxy upgrade` 拿到 0.2.0 时，必须在改动任何东西之前失败，不能半途坏掉（实例失联、
  二进制已换）。审查 config-cli-install 会读 `git show v0.1.0:src/install/...` 给出结论与不靠兼容代码的办法。
- CHANGELOG 写清 0.1.0 → 0.2.0 的迁移步骤（停实例、重新安装）。

### W2 契约重审与定稿
- 依据审查报告逐项决定：IPC（UI 要的只读枚举、事件订阅、日志流、能力声明、错误码、写类 op 的访问控制）、
  `~/.aproxy` 磁盘布局、config.toml 与 settings.json 字段、CLI 子命令与参数、退出码与 `--json`、发给客户端的
  错误体形态、信封协议。
- 定稿后重写记忆 ipc-v1-upgrade-contract 的永久接口清单（0.2.0 发布时冻结）。

### W2 的用户决策（2026-10-11，依据综合稿 §3）
- D-1 重试配置：规则表。`[retry]` 下放退避上限、抖动、是否读 Retry-After；`[[retry.rules]]` 每条可写 `path`（整体匹配、含
  查询串）、`status`、`body`、`max_attempts`；`bounded_retry_paths` 并入规则表（删除，未知键告警里提示改名）。默认仍无限
  重试，不内置任何 URL。0.2.0 先实现 path、status 与退避，`body` 的形状先定、0.2.x 加性实现。
- D-4 命令面：`config` / `settings` 的 `get`、`set`、`unset`（加 `show`）取代 15 个专用旗标，键就是文件里的键；`start` 的覆盖
  旗标按键名推出，`--baseurl` 改为 `--base-url`。
- D-6 请求侧转换失败：format 在回信里带约定的失败结构（是转换器的问题、`retry`、原因，可选附给客户端的完整响应）；
  应重试进正常重试循环，不应重试就原样返回 format 给的响应、没给才回 aproxy 统一终态错误；进程崩溃、超时、输出
  不合法一律按应重试。信封 0.2.0 定稿时纳入。
- D-7 裸 `aproxy` 只输出帮助；启动只走 `aproxy start`，覆盖参数归到 `start` 下。
- 主代理按推荐执行、已告知用户（未反对）：D-2 期望状态记录保存 `--api-key` 与代理相关环境变量（0600，永不进命令行）；
  D-3 退出码 0 成功/已达成、1 操作失败、2 用法错误、3 配置无效、4 目标未运行或不存在、5 部分成功，`stop` 已停的
  目标返回 0；D-5 settings.json 按节重整（`defaults` 不含 transform 与 `log_file`）；D-8 看护者连续失败后每 5 分钟慢速
  重试；D-9 skill references 这次不留存 0.1.0 版（对留存规则的一次例外）。
- D-10 被「先发 alpha.1」取代；D-11 按现稿确认并采纳 G2–G6。

### W2 的 format 定稿（用户 2026-10-11/12 提出，主代理设计，用户未反对）
- **format 决定一切按请求的行为**（用户：「是否重试、重试几次等几乎绝大多数配置都可以通过 format 变成动态的」）：原则是
  「配置是默认值，format 实时覆盖」。请求阶段的回信可按请求覆盖超时、是否保活、心跳方式、重试上限等；新增**决策阶段**
  （每次失败后，上下文含第几次尝试、状态码或错误类别、已耗时、上游错误体摘要、请求概要、state），回答是否继续、等多久、
  是否换路由（every_attempt）、停止时回什么（透传或 format 自带响应）；format 也能把「200 但内容是错误」判为失败。
  护栏：决策阶段超时很短，format 崩溃、超时、答非所问一律回落到静态配置（规则表 + 默认无限重试）；优先级 format 决策 >
  规则表 > 默认；不启用该阶段时行为与现在完全一样；每个决策与理由进日志并在界面可见。基础设施类设置（监听地址、日志文件、
  安全、format 命令本身、看护者）保持静态。与 D-6 的「约定失败结构」统一成同一套决策回信。
- **配置合并为一个 `[format]`**（用户问「上传和下载的 format 要不要合并」）：一个程序、一个进程池，`stages` 声明接管哪些阶段
  （request、retry、decide、response、stream、heartbeat），阶段专属设置放 `[format.stage.<名>]`；`[request_transform]`、
  `[response_transform]`、`[heartbeat_transform]` 与 `every_attempt` 一并取消（0.2.0 不兼容旧键，未知键告警里提示）。将来
  需要多程序串联时再只加不改地扩展。
- **仅转发模式的协议转换 + 实时流式转换**（用户：「这个是需要的，并且仅中转下需要实时的流式转换」）：
  - 现状：配置校验拒绝「仅转发 + 转换器」（`src/config.rs:592`）；aproxy-format 连缓冲模式下的 SSE 也原样透传、不做转换
    （`aproxy-format/src/translate.rs:7-9`）——跨协议的流式请求在任何模式下都不可用。
  - 请求侧：配了 format 时仅转发模式也先读完请求体（受 `max_body_mb` 限制）再转换。
  - 新增 `stream` 阶段：开流时 aproxy 交上游响应头，format 回给客户端的响应头，并附一段「中止收尾」（format 中途崩溃时按客户端
    协议发出的错误结尾）；之后逐块交原始字节、format 回零到多段已转换字节、aproxy 立即写给客户端；上游正常结束时 format
    收尾，上游中断或客户端断开时 format 收到中止。传原始字节而非事件，不绑定 SSE；一条流在生命周期内固定由一个 format
    进程处理，消息都带 `stream_id`，将来 format 声明能多路复用时再只加不改地支持；每块多一次本机往返（约 0.1–1 ms）。
  - 缓冲模式也走 `stream` 阶段，回放时边放边转；「200 但内容是错误」等判定仍在转换前对上游原始数据做。
  - **握手**：每个 format 进程启动时先与 aproxy 交换协议版本与能力；以后加阶段、加能力都靠它协商，旧 format 不会收到它
    不懂的消息。这是信封冻结后仍能演进的关键。
  - aproxy-format 用 switchyard 的流式接口，按 Anthropic、OpenAI Chat、OpenAI Responses 各自分帧实现真正的流式转换。

### W2b 强制安装：永久兼容契约（用户 2026-10-11 要求「从现在开始保持绝对兼容」；设计稿，待用户确认）

**保证**：从 0.2.0 起，任意版本 X（≥0.2.0）的 `aproxy install --force` 都能把任意版本 Y（≥0.2.0）装进同一个 home，
并停掉这个 home 下由任意版本（≥0.2.0）启动的全部 aproxy 进程；**不依赖除下面 C1–C8 之外的任何东西兼容**。
这也覆盖「装着的版本坏到跑不起来」：手动下载新版 Y，执行 `Y install --force --from Y` 自装。

**设计原则**：冻结得越少越好（每一项都是永久成本）；只依赖操作系统和下面这几份冻结格式，绝不依赖 IPC、配置、
注册表、install.state 的内容；先验证、后破坏——新二进制下载、校验、并在本机实际跑通 `--version` 之前，什么都不停；
每一步可重入，中途被打断就再跑一次，最终收敛；只按台账里的 pid + 进程启动标记结束进程，绝不按名称；冻结格式只许
追加可选字段，读者忽略不认识的字段，永远不删、不改名、不改类型；万一将来非改不可，只能另立并行的新格式，旧格式继续写。

**冻结项**
- C1 home：`APROXY_HOME`（绝对路径），否则 `<用户目录>/.aproxy`。
- C2 二进制位置：`<home>/bin/aproxy`（Windows 为 `aproxy.exe`）。**交换过程从不使用固定的中间文件名**（用户 2026-10-11
  指出：短时间内连续更新两次，上一次的 `.old` 还被进程占着删不掉，固定名必然改名失败）：旧二进制改名为
  `aproxy.old-<纳秒时间戳>-<pid>[.exe]`，新文件先落在 `aproxy.tmp-<纳秒时间戳>-<pid>[.exe]`；目标名已存在就换一个后缀重试
  （Windows 的改名在目标存在时失败而不覆盖，正好用来判断）。`bin/` 下以 `aproxy.old-`、`aproxy.tmp-` 开头的文件，任何版本
  都可在删得掉时删除；删不掉（仍被占用）就跳过，残留永远不会挡住下一次安装。普通安装（W2）也改用同一套命名，现在的
  固定名 `aproxy.old(.exe)` 加秒级时间戳让位的做法在同一秒内两次让位时仍会撞名。
- C3 进程台账 `<home>/ledger/`：每个长期运行的 aproxy 进程（后台实例、前台实例、看护者、续作中的安装者、桌面界面）
  启动时、在占用端口或拉起子进程之前，原子写入 `<pid>.json`，正常退出时删除。内容是一行 JSON：
  `{"ledger":1,"pid":…,"start":"…","role":"…","version":"…"}`，可选 `port`、`label`、`exe`。`start` 是进程启动标记，
  推导方式冻结：Windows `w:<GetProcessTimes 创建时间的 FILETIME 十进制>`；Linux
  `l:<boot_id>:<stat 第 22 字段 starttime>`（带 boot_id，重启后 pid 与滴答巧合重复也不会认错）；macOS
  `m:<pbi_start_tvsec>.<pbi_start_tvusec>`。`role` 词表只增不减：installer、watchdog、instance、desktop；不认识的角色
  照样结束，排在最后。读者跳过解析不了的文件。写台账失败时告警但照常运行。
- C4 安装状态收拢：一切安装者状态（状态文件、备料、下载临时文件）只能放在 `<home>/install/` 下，强制安装可随时整目录清空；
  `<home>/install/lock` 是操作系统级排他锁（进程死亡自动释放），任何版本修改 `bin/` 期间都要持有它。
- C5 `--version`：标准输出第一行 `aproxy v<semver> (<variant>; <target>[; x86-64-v3])`；退出码 0；不读写 home、
  不联网、不拉起任何进程，并且在一切副作用之前短路——将来 i18n 在解析参数前读 settings.json 判定语言，`--version`
  必须排在它前面（G2）。输出里任何以空白分隔的词都不得以数字开头（G6）：0.1.0 的安装器取「第一个以数字开头的词」
  当版本，这条保证它在交换前失败（见 W1）。
- C6 发布清单：每个主线 GitHub Release（tag `v<semver>`）都附 `aproxy-manifest.json`：
  `{"manifest":1,"version":"…","assets":[{"target":"<Rust 目标三元组>","variant":"desktop|headless","cpu":"baseline|x86-64-v3","file":"…","sha256":"…"}]}`。
  强制安装只靠它找资产，所以以后资产怎么改名都不影响旧版本的强制安装。
- C7 命令行：`aproxy install --force [<版本>|latest] [--from <文件>] [--variant desktop|headless] [--yes]`，
  `aproxy upgrade --force` 等同 `install --force latest`；`aproxy restore` 子命令永远存在（强制安装后尽力调用它拉回实例）。
  退出码冻结：0 完成；10 未做任何改动就中止；11 进程已停但二进制未替换；12 已替换但实例未全部拉回。通用退出码只用
  0–5（G5，见 W2 的退出码表），10 起留给强制安装，互不重叠。
- C8 流程顺序：
  1. 取得目标（C6 清单或 `--from`）放进 `<home>/install/force/`，校验 sha256，在本机执行 `--version` 并核对版本；
     失败即退出码 10，什么都没动（例如 Linux 上缺 webkit2gtk 跑不了带 UI 的版本，就在这里挡住并提示 `--variant headless`）。
  2. 确认（会中断所有经 aproxy 的在途请求，包括 agent 自己的会话；`--yes` 跳过）。
  3. 按台账结束 installer → 取得 C4 的锁 → 结束 watchdog、instance、desktop 与未知角色；反复扫描直到连续两轮没有存活
     条目（有上限）。结束前在同一进程句柄（Windows）或 pidfd（Linux）上核验启动标记，杜绝 pid 复用的竞态。结束不掉
     （例如权限不足）就停在这里，退出码 11。
  4. 清空 `<home>/install/`（保留锁与本次的目标文件）。
  5. 替换 `bin/` 下的二进制：先尽力清掉删得掉的旧残留；unix 把 `aproxy.tmp-…` 直接 rename 覆盖到位（原子）；Windows 先把
     现文件改名为唯一的 `aproxy.old-…`，再把 `aproxy.tmp-…` 改名到位，第二步失败立即改回。现文件被不带共享删除权限的句柄
     占着（例如杀毒软件正在扫描）导致改名失败时，短暂退避重试，仍失败则退出码 11（进程已停、二进制未动，可再跑一次）。
  6. 用新位置的二进制再跑一次 `--version` 核对；尽力调用新版本的 `restore`；拉不回的实例按台账里的 `port`、`label` 列出来，
     请用户手动启动（退出码 12）。
  7. 另外报告不在台账里、但从 `<home>/bin/` 运行的进程（例如 0.1.0 留下的），只报告、不结束。
- 强制安装从不读写配置、settings、日志、注册表，也不走 IPC。由桌面界面发起时，界面先把强制安装作为独立进程拉起再退出
  （它自己也在被结束之列）。

**两层表述（G4）**：C1–C8 里，结构与语义「绝对冻结」（永不改动）；JSON 格式「只增不改」（可追加可选字段，读者忽略
未知字段）。台账与实例注册记录分开（G3）：注册记录可随意演进，台账只承担「找到并结束这个进程」。

**不冻结**：IPC、注册表与 `.restore` 格式、install.state 的内容、配置文件、日志、下载渠道与资产命名（C6 之外）、`restore`
的具体语义——这些都可以随意演进。

**落地**：代码放 `src/ledger.rs`（台账）与 `src/install/force.rs`；两处文件头与每个冻结常量写明「永久兼容契约：无论如何都
不得改动，只许追加可选字段」；`tests/force_contract.rs` 用 0.2.0 写下的样本（台账条目、`--version`、发布清单、命令行
解析、退出码）钉死，测试注释写明「改这些测试就是破坏契约」；AGENTS.md 加一句原则指向它；release.yml 生成并校验
`aproxy-manifest.json`；有两个带强制安装的版本后，CI 加双向跨版本强制安装测试。台账必须在 0.2.0 发布前就由所有长期
进程写入，所以排在 alpha.1。0.1.0 不写台账，它的进程由第 7 步报告，迁移步骤里先用 0.1.0 `stop all`。

### W3 i18n
- 规则（用户定）：给用户看的随语言环境（en / zh-CN），给 agent 看的或不可见的固定英文。
- 语言判定：`APROXY_LANG` > settings.json `language` > 系统界面语言（Windows 用
  `GetUserPreferredUILanguages`，不用区域格式；unix 按 `LC_ALL` > `LC_MESSAGES` > `LANG`，`C`/`POSIX` 为英文）> 英文；
  `zh*` 为中文，其余英文。windows-sys 0.52 已有这些 API（加 `Win32_Globalization` feature）。
- 消息机制待定：内联 `tr!(en:, zh:)`（编译期检查参数与漏译，零依赖）与「每种语言一个目录文件 + 构建期生成
  带类型参数的函数」之间选。UI 前端也要目录文件，倾向两边共用一套目录；W3 开工时定。
- `--help`：clap 派生属性接受运行时表达式（`#[arg(help = ...)]`）；clap 内置标题用 `next_help_heading`、
  `subcommand_help_heading`、自定义 help 参数覆盖；clap 自己的解析错误写死英文，第一期保留英文。
- 经 IPC / HTTP 传出、再被显示的错误改为错误码 + 参数，由显示的一端本地化。
- 测试统一 `APROXY_LANG=en`，断言改英文。
- 先建基础设施，再随各模块重构迁移该模块的字符串，避免翻译要删的代码。

### W4 `aproxy skill` 命令（用户需求）
- `skill install` / `skill upgrade`：只下载 skill；删掉 `install --skills-only`。默认取与当前二进制同版本的
  skill（skill 描述的是这个二进制的行为）；`aproxy install` 仍随二进制更新 skill。
- `skill list`：已装 skill、版本、与二进制版本是否一致、位置；没装就提示怎么装。
- `skill show <name> [<file>]`：默认输出 SKILL.md，也能输出 references 等文件，给读不了目录的 agent 用。
- 安全：`show` 只能读 skill 清单里登记的文件。拟在发布的 skills 包里带清单（版本、文件列表、哈希）作白名单；
  名字与文件路径必须精确命中清单；拒绝绝对路径、`..`、盘符 / UNC / ADS；逐级 `symlink_metadata` 拒绝符号链接
  与 junction，规范化后再核对前缀；只读普通文件、有大小上限、UTF-8。
- 不把 skill 嵌进二进制；本地没有就提示 `aproxy skill install`，非零退出。
- 所有 `--help` 末尾加一行固定英文提示（给 agent 看），指向 `aproxy skill list` / `show`。
- SKILL.md 里写明打不开 references 时用 `aproxy skill show`。
- 审查补充（config-cli-install.md §8，主代理采纳）：
  - 版本：`skill install` 严格取正在运行的二进制的版本，`skill upgrade` 是它的别名；想要新 skill 就升级二进制。
  - 白名单：安装时写下本地记录 `<home>/skills/manifest.json`（版本、文件、大小、sha256，兼作「装完」标记）；发布的 skills
    包自带同格式清单，解包改为按清单白名单进行，并限制大小与条目数。
  - `show` 的决定性一步：把文件读进内存，核对大小与 sha256，再输出同一块内存——符号链接、junction、硬链接、ADS、
    竞争都绕不过它。另加：名字与路径逐字节精确匹配清单键、用户输入不拼进路径、严格字符集、逐级拒绝 reparse point、
    打开时不跟随链接、上限 1 MiB、必须 UTF-8。
  - `--help` 提示行：构建命令后递归给所有子命令设 after_help，配一个遍历全部子命令的测试；文案例如
    `Agents: run 'aproxy skill list' and 'aproxy skill show <name>' for full docs.`
  - 打包：一个脚本（输入取 `git ls-files`）打出一个带清单的可复现 zip，GitHub、npm、crate 三个渠道共用；删掉没有消费者的单
    skill 包；校验规则只在 Rust 里写一份；`.gitattributes` 给 skill 文件定 `eol=lf`。
  - 现有缺陷：`--skills-only` 与安装的 skill 支线之间没有锁，共用 `.dl`/`.staging`，并发时互删半成品。
  - **0.1.0 的 skill 支线**：0.1.0 的 `upgrade` 一开始就并行下载目标版本的 skill，结局与二进制无关，C5 只拦得住二进制。
    0.2.0 正式版之前，skills 资产要换名、npm 包内路径要换、crate 不再带 `.claude/skills/`，让 0.1.0 的三条渠道都找不到，
    skill 原样不动。预发布版不受影响（0.1.0 的稳定通道不选预发布）。

### W5 UI（Tauri 2，编进本体）
- 构建：cargo feature `desktop` 进 default features（与 `aproxy desktop` 命令、`--version` 的变体词一致）；
  `--no-default-features` 出无 UI 版本，不得多出任何依赖（CI 用 `cargo tree` 断言依赖树里没有 tauri）。
- **crate 布局（用户 2026-10-11 提议「新增两个 crate」，主代理细化切法，用户未反对）**：
  - `aproxy-ipc`：控制协议类型、客户端、实例发现、日志与事件流——「读数据、转给界面」。不依赖 Tauri，可完整测试；
    守护（服务端）、命令行、桌面三方共用这一份，协议契约测试放这里。
  - `aproxy-desktop`：Tauri 外壳 + 嵌入的前端，读数据只经 `aproxy-ipc`。主二进制依赖它，所以它**不能**依赖主 crate
    （否则成环）：改配置、拉起实例、安装与强制安装等需要核心逻辑的操作，由它定义一个接口，主 crate 实现后在
    `aproxy desktop` 启动时注入。不放单元测试（Tauri 链进测试程序会让 Windows 上的 `cargo test` 崩溃，RT-05），
    逻辑尽量放进 `aproxy-ipc` 测。
  - `aproxy`：核心 + 命令行 + 可执行文件；`desktop` 特性 = 对 `aproxy-desktop` 的可选依赖。
  - 发布：crates.io 发布顺序 ipc → desktop → aproxy，版本与主线同步（同 aproxy-envelope 的做法）；两个新 crate 首发要用
    所有者给的 token 先发一次，再经 API 绑定可信发布（release.yml / release）；desktop 包带上 CI 里构建好的前端产物，
    用 `cargo package --list` 核对。综合稿 §3.12 与 RT-05「不建新 crate」的建议被这个切法取代。
- 进程模型：`aproxy desktop`（用户定的启动方式）起独立的 GUI 进程（窗口 + 托盘），守护进程永不加载 GUI；无 UI 版本也认得 `desktop`（帮助里隐藏），执行时说明这个版本没有桌面界面、该装哪个版本，非零退出——不让用户看到 clap 的「unrecognized subcommand」；GUI 只经 IPC 与实例通信、
  按显式 home 寻址，不开新网络端口。
- 窗口外观按两个 skill 的做法（实现时用 Skill 工具调用它们，照 recipe 复制 assets）：
  - tauri-custom-titlebar：Windows 保留原生框架与标题栏按钮（Snap Layouts、系统菜单），`windows_titlebar.rs`
    原样使用；标题栏高度取 `--titlebar-height`；可交互元素 `app-region: no-drag`；按钮颜色只用
    `setTitlebarColor`。Linux 用原生装饰或 `decorations: false` + `enableDragRegion()`。
  - tauri-windows-mica：**默认关闭，在 UI 设置里开启，可运行时切换**。窗口始终 `transparent: true`、
    `visible: false` 并由页面调 `show()`，配置里不写 `windowEffects` 与 `backgroundColor`；页面用 mica.css 的
    令牌。关闭时 `data-mica="off"`（不透明回退）；开启时 `setEffects(mica)` + `data-mica="on"` +
    `setTitlebarColor(null)`；关闭时先设 `data-mica="off"` 再 `clearEffects()`（反过来窗口会透明），并把标题栏
    颜色设回页面色。主题用 `setTheme` / `theme`，不用 `micaDark` / `micaLight`。权限：`core:window:allow-show`、
    `allow-set-effects`、`allow-set-theme`。
  - **没有 Mica 的 Windows 11**：是否支持由 Rust 端判定，不只看 UA-CH——按系统 build（22523+ 走文档化的
    `DWMWA_SYSTEMBACKDROP_TYPE`；22000–22522 只有未文档化属性、skill 未验证，倾向按不支持处理）并直接调用
    window-vibrancy 拿到结果（Tauri 会吞掉错误）。不支持时设置里的开关禁用并说明原因；系统关了透明效果、
    节电模式或高对比度时 Mica 显示为纯灰，这是系统行为，设置里给出提示（`EnableTransparency` 注册表值）。
  - 验证脚本：titlebar 的会实际移动鼠标约 15 秒（运行前先告诉用户），mica 的会把窗口置前约 8 秒；人工检查项
    （切换系统主题、透明效果）交给用户。skill 验证于 tauri 2.12.0 / wry 0.57.0 / tao 0.37.1，升级 Tauri 后重跑。
- UI 文本走 i18n（与命令行同一套语言判定）。
- **原型先行（用户 2026-10-11）**：先在 `desktop/` 做浏览器里可看的高保真原型（假数据，不接后端），用户只审风格与
  美观度，要求见记忆 [ui-design-style](../memory/2026-10-11-ui-design-style.md)；审核通过后才真实接入，并解决
  海量日志的性能（虚拟滚动等）。日志行高统一、详情在侧面板打开，为虚拟滚动留路。
- **可自定义界面（用户 2026-10-11 同意）**：用户自己改界面拦不住也不该拦（WebView2 远程调试、替换前端资源都能做到），
  所以完整注入作为正式功能提供；要守住的是「别人做的扩展经官方渠道装进来时默认拿多少权限」。分三层：
  - 主题：可复制分享的声明式字符串 `aproxy-theme-v1:{...}`（主色、文字、表面、对比度、圆角、字体、动效强度、状态语义色、
    浅/深变体），设置里导入/导出，不含脚本；界面的颜色、圆角、字体、阴影、动效全部走 CSS 变量，根元素暴露稳定的
    状态属性（data-theme、data-variant、data-mood 等），主题与扩展不依赖内部 DOM。
  - 吉祥物：组件契约——输入 MascotState（mood、重试次数、尺寸、减少动效、配色），每个吉祥物实现同一接口；内置几种
    放注册表，设置可选或不显示；默认是不倒翁，也作软件图标候选（exe 图标编译时固定，托盘图标可随所选吉祥物变）。
  - **安全边界在实例与 IPC，不在 UI**（用户 2026-10-11 纠正：UI 只是 UI，自己不持有 key、不经手流量，敏感内容都在
    实例里）：IPC 永不返回明文密钥——key 在实例侧打码，配置里的密钥字段只写不读；请求体只在点开某条日志时按需取；
    UI 的 webview 用严格 CSP 禁止一切外部网络（数据只走 IPC），注入的代码看得到也发不出去。
  - 扩展包（本地文件夹 + 清单）：读不区分权限（扩展能看到的就是 UI 能看到的）；只管「写」——停止/重启实例、改配置
    （改 base_url 等于把流量导去别处）、执行安装——扩展第一次调用这类操作时请用户同意一次。吉祥物仍在隔离 iframe 里
    运行，目的是稳定（坏掉的吉祥物拖不垮界面）。另给稳定的事件与状态接口，减少扩展依赖内部结构。
  - 用法做成 skill（给 agent 用来帮用户做主题、吉祥物、扩展）。参考：Codex 桌面版的 `codex-theme-v1:` 主题字符串与
    社区经本机 CDP 注入 CSS 的工具（其中有的只收数据、CSS 与图片，直接拒绝主题脚本）；用户说明只是受其启发，具体
    自行设计。
- **Web UI 暂不做（主代理建议，2026-10-11）**：它要新开管理端口并从零做令牌、Host/Origin、CSRF（远程还要 TLS），打破
  「代理端口只转发、控制只走本地 IPC」的边界，日志里的请求体也会暴露到网络上。现在只留好接缝：前端一切数据与命令都经
  一层 Backend 接口，组件不直接碰 Tauri；原型里是 mock 实现，正式版是 Tauri IPC 实现，将来要做 Web UI 时再加一个
  HTTP/WebSocket 实现即可复用整套界面（headless 版也能带上，前端只是几百 KB 静态文件）。等真有 headless/服务器用户要界面
  再做：默认只监听回环、`aproxy webui` 显式开启、启动时打印一次性令牌。
- 前端栈：Vite + Svelte 5 + TypeScript（产物嵌进二进制，Svelte 编译后运行时小、自带过渡与弹簧动效；不用大型组件库
  与图表库，字体本地打包）。这是主代理的选择，用户可改。
- 待定：
  - **所有平台都发两个版本（用户定）**：现有 11 个构建（Windows x64 / x64-v3 / x86 / arm64，Linux gnu x64 /
    x64-v3 / arm64，musl x64 / arm64，macOS arm64 / x64）各出带 UI 与无 UI 两份，共 22 个。由此带来的事：
    - Linux 的 UI 版动态链接 webkit2gtk / GTK，缺库的机器上连代理都起不来；install 与一键脚本要选对版本，
      升级时保持原版本（二进制要能报告自己是哪个版本，例如 `--version` 带上变体）。
    - gnu 的 UI 版不能沿用 cargo-zigbuild 压低 glibc 下限，glibc 下限由 webkit2gtk 决定，拟在 ubuntu-22.04
      （及其 arm runner）上构建。
    - musl 的 UI 版不可能静态链接：要在 Alpine 容器里对 musl 版 webkit2gtk 动态链接，只能跑在装了 webkit2gtk
      的 musl 发行版上。发布说明要写清。
    - **命名（用户 2026-10-11 选定 headless）**：带 UI 的沿用现有名字——GitHub 资产 `aproxy-<target>[-v3][.exe]`、
      npm `@meowo/aproxy` 与 9 个 `@meowo/aproxy-<平台>`、crates.io `aproxy`（feature `desktop` 默认开，binstall
      的默认命名正好对应它）；无 UI 的叫 headless——资产 `aproxy-headless-<target>[-v3][.exe]`、npm
      `@meowo/aproxy-headless` 与 9 个 `@meowo/aproxy-headless-<平台>`、cargo 用 `--no-default-features`，不另建 crate。
      `npm/build-and-publish.sh` 已有按组切换包名与资产前缀的结构（format 组），headless 加一组即可；JS 转发器按主包
      名推导子包前缀，不用改。两个主包都提供 `aproxy` 命令，不能同时全局安装。
    - **npm 种子已发（2026-10-11）**：10 个 headless 包以 0.0.0 占位版本发布；**可信发布绑定待所有者完成**
      （GAT 做不了，见记忆 trusted-publishing-p0 的 2026-10-11 一节）。crates.io 不需要新配置，已核对现有绑定都在。
    - 0.1.0 安装器的干净失败另有一条与命名无关的路：0.1.0 的 `probe_version` 取 `--version` 输出里第一个以数字
      开头的词，且必须与目标版本完全相等，否则在交换前中止（`git show v0.1.0:src/install/staging.rs`）。0.2.0 的
      `--version` 本来就要带上变体（install 升级时保持变体），若格式写成 `aproxy v0.2.0 (desktop)` 之类，0.1.0
      的四条下载渠道都会在交换前失败。待审查 config-cli-install 的断点分析印证后定。
    - 发布流水线时长与产物体积显著增加。
  - aproxy.exe 是控制台程序，从快捷方式启动 `aproxy desktop` 会闪控制台窗口：拟让 `desktop` 分离出自身后父进程退出，
    或在独占控制台时释放它，实现时实测。
  - crates.io：默认 feature 含 UI，`cargo install aproxy` 需要包里带构建好的前端，Linux 还要 GTK 开发库；
    无 UI 用 `--no-default-features`。
  - 前端技术栈（体积会编进二进制）、UI 偏好（Mica 开关、主题）存哪里（倾向 settings.json 的 `ui` 段，随 W2 定）、
    发版矩阵与体积、macOS（不在 0.2.0 范围）。

### W6 审查发现的修复与重构
- 待审查报告。

### W7 发布
- 版本 0.2.0（是否先发预发布版待定）、CHANGELOG、README / docs / skill 同步、skill references 是否留存
  0.1.0 份（按 aproxy-cli-skill-versioning 记忆判断）、CI 与发版矩阵。

## 下一步

1. 收齐四份审查报告，汇总成一张发现清单（P0 / P1 / P2），与本计划的工作流对齐、排序。
2. 先做 W1（独立、确定），同时出 W2 的契约设计稿。
3. W2 定稿后再动 W3 基础设施、W4、W5。
