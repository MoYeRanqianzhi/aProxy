# 0.2.0 计划

> 状态（2026-10-11）：方向已由用户定下（见记忆 [0.2.0-direction](../memory/2026-10-11-0.2.0-direction.md)）；
> 四路审查与综合已完成：`.agents/review/2026-10-11-0.2.0-audit/`（dataplane、control-plane、config-cli-install、
> release-tests-docs，综合稿 synthesis.md：合并后 P0 16 条、工作流归属、待拍板决策 D-1…D-11、主代理可自定的
> 设计点 §3.12）。综合稿的 D-10（alpha 不公开发布）被用户「先发 alpha.1 验证可信发布」的定调取代；§3.12 里
> 「不建 aproxy-ipc / aproxy-ui crate」被用户「新增两个 crate」的提议取代（见 W5）。
> 2026-10-12：alpha.1 第 1 步（W1）完成并推送（fbcc738）；第 2 步（台账 + 强制安装）由子代理实现中；UI 原型按用户
> 第一轮风格反馈返工中（第一版仍带偏移硬阴影，未达要求）。
> 2026-11-11：v0.2.0-alpha.1 已于 10-13 发布（五步全部完成）；余下工作的顺序、tag 节点与进度见文末「执行顺序与进度」。

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
4. 发布流水线：20 个产物双矩阵（musl 只出 headless；Linux gnu 带 UI 版在 ubuntu-24.04 / 24.04-arm 上用 `container: ubuntu:22.04` 构建）、前端构建步骤、
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
- **状态（2026-10-12）**：设计稿定稿，见 [w2-contracts](w2-contracts.md)，经四轮对抗式审查，记录在
  `.agents/review/2026-10-12-w2-contracts-review.md`。实现排在 alpha.1 之后，按它的 §14 切分。
- 依据审查报告逐项决定：IPC（UI 要的只读枚举、事件订阅、日志流、能力声明、错误码、写类 op 的访问控制）、
  `~/.aproxy` 磁盘布局、config.toml 与 settings.json 字段、CLI 子命令与参数、退出码与 `--json`、发给客户端的
  错误体形态、信封协议。
- W2 实现落地后重写记忆 ipc-v1-upgrade-contract 的永久接口清单（0.2.0 发布时冻结）；在那之前，它描述的仍是现有代码。

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
- **W2 设计稿审查后的用户决策（2026-10-12，依据 `.agents/review/2026-10-12-w2-contracts-review.md`）**：
  - D-12 **format 熔断**（用户选它，不选给规则表加 `codes` 匹配器）：同一个 format 程序跨请求连续出现进程级失败（崩溃、超时、
    输出不合法）达到阈值就判为不可用——required 的程序让请求快速失败（不再无限重试），`required = false` 的跳过；阈值与
    冷却后的试探有默认值、可配置。单次故障仍按 D-6 应重试，连续故障才熔断：D-6「一律按应重试」由此改为「单次按应重试」。
    「上游永远连不上」仍无限重试（记忆 bounded-retry-paths：网络错误不封顶）。
  - D-13 **请求历史保留全部请求的体**（用户原话「全部暂留在储存中，但是需要上限，上限有默认值，并且可配置」）：只在内存、
    重启即失（主代理把「储存」理解为内存暂存，已向用户说明），总量上限有默认值且可配置，超出按旧到新淘汰。
  - D-14 **配置严格解析**（用户原话「直接报错表示参数错误，不做任何兼容检测以保持简洁，避免还要浪费代码去对过去的旧键进行
    提示」）：config.toml 与 settings.json 里任何不认识的键都直接报错（指出文件与键名），不写任何识别 0.1.x 旧键、提示新写法
    的代码。取代 D-1 与 format 定稿里「旧键在未知键告警里提示新写法」；也顺带满足审查 S1-1（条件类配置不能把不认识的匹配器
    读成放宽）。
  - D-15 **重试时序不信上游**（用户原话「不管上游，对上游保持不信，只相信format给出的结构化内容，format说等那就等」）：不读
    Retry-After，不设 `respect_retry_after`；退避沿用现行（前 3 次立即，之后 5 秒起翻倍、封顶 `max_backoff_secs`），不加抖动；
    等多久的动态决定只来自 format 的结构化回信（decide 给出等待时长，aproxy 照办）。
  - 用户随后补充（2026-10-12）：「对于无限重试依旧保持默认，等待时间、判断不重试等全部通过format给出的结构化指令信息为主」。
    即：默认无限重试不变；等多久、要不要停，以 format 的结构化指令（decide 阶段与按请求覆盖）为主要机制；aproxy 自身只保留
    简单、确定的静态兜底（D-1 的规则表、现行退避），不内置任何「读上游脸色」的智能。D-12 的熔断只处理 format 自身坏掉、给不出
    指令的情形。
  - D-16 **settings 的 `defaults` 收哪些键**（第二版设计稿的 Q4；用户原话「不同情况得有不同做法，listen_addr显然不可能相同，
    直接不要。url和key有可能一样，可以放默认。extra_headers、override_headers是多条的，用合并策略。proxy也可以默认，因为也
    很常见……毕竟默认值本身也是为了方便」）：
    - `listen_addr` 不进 `defaults`（transform、`log_file` 照旧不进）；
    - `base_url`、`api_key`、`proxy` 与代理口令可以设默认；
    - `extra_headers`、`override_headers` 与配置里的同名表按头名合并（不区分大小写，同名以配置为准）。
    - 主代理补的约束，用户同意（「可以」）：凭据只发给它所属的目标。
      - 默认的 `api_key` 与默认头里的鉴权头，只在配置最终的 `base_url` 等于默认 `base_url` 时继承；
      - 默认的代理口令只跟默认 `proxy` 一起继承。
      - 防的是审查 S2-7 的风险：指向别家上游的配置悄悄继承全局 key，把 A 家的 key 发给 B 家。
    - 代价：settings.json 会含密钥，按 config.toml 同一套打码与文件权限处理。
    - 细化 D-5：`defaults` 不再是与 config.toml 字面同构。

### W2 的 format 定稿（用户 2026-10-11/12 提出，主代理设计，用户未反对）
- **format 决定一切按请求的行为**（用户：「是否重试、重试几次等几乎绝大多数配置都可以通过 format 变成动态的」）：原则是
  「配置是默认值，format 实时覆盖」。请求阶段的回信可按请求覆盖超时、是否保活、心跳方式、重试上限等；新增**决策阶段**
  （每次失败后，上下文含第几次尝试、状态码或错误类别、已耗时、上游错误体摘要、请求概要、state），回答是否继续、等多久、
  是否换路由（every_attempt）、停止时回什么（透传或 format 自带响应）；format 也能把「200 但内容是错误」判为失败。
  护栏：决策阶段超时很短，format 崩溃、超时、答非所问一律回落到静态配置（规则表 + 默认无限重试）；优先级 format 决策 >
  规则表 > 默认；不启用该阶段时行为与现在完全一样；每个决策与理由进日志并在界面可见。基础设施类设置（监听地址、日志文件、
  安全、format 命令本身、看护者）保持静态。与 D-6 的「约定失败结构」统一成同一套决策回信。
- **一种配置形状、可挂多个程序**（用户 2026-10-12 指出：只允许一个程序会让「协议转换用别人开源的 C++ 实现、其他功能
  各用 Rust/Python/PowerShell 写」的组合没法同时使用，而它们各司其职并不冲突）：`[[format]]` 是程序列表，每个程序有自己的
  `name`、`command`/`args`、`process`（spawn/persistent）、进程池、超时，并用 `stages` 声明接管哪些阶段（request、retry、
  decide、response、stream、heartbeat）。同一阶段有多个程序时按「洋葱」中间件组合：请求方向按列表顺序（离客户端最近的
  写在前面），响应与流方向按相反顺序；决策阶段按列表顺序，第一个给出明确决定的生效，都不表态就回落到规则表与默认行为。
  `state` 按程序隔离（各自只看到自己的），`request_id` 全链共享；每个程序可设 `required = false`，它出错时跳过而不影响
  请求（适合审计、记录类工具）。新阶段只是 `stages` 里多一个词，不新增配置段；`[request_transform]`、
  `[response_transform]`、`[heartbeat_transform]` 与 `every_attempt` 一并取消（0.2.0 不兼容旧键，未知键告警里提示）。
  握手时每个程序报告自己支持的阶段与协议版本，配置里声明了它不支持的阶段就在启动时报错。链上每多一个程序，该阶段就
  多一次本机往返，文档写明。以后可再加只观察、不阻塞请求的「旁路」程序，属于只加不改。
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

### W2 的 format 定稿补充（用户 2026-10-12）
- **回信带结构化说明 `report`**（可选、推荐）：`summary` 一句给人看的话 + `actions` 列表；`actions` 用一张只增不改的标准
  类型表（如 protocol_convert{from,to}、key_select{key_id}、route{channel,model_from,model_to}、custom{label}），界面按类型
  显示图标并做多语言渲染，不认识的类型显示 `label`。aproxy 每个请求记一行日志列出链上各程序的说明，界面日志详情的「转换」
  页签按步展示。说明里不得出现明文 key，只写编号或打码尾号（协议文档写明）。
- **准入阶段 `admit`（并发限制与排队）**：每次发往上游之前（含重试）问 format「现在能发吗」；能就发，不能就进队列，可附原因
  与建议多久后再问。有请求完成腾出位置、或建议时间到了，只问队首，放一个再问下一个。排队不算失败，客户端心跳照发；队列
  长度、等待时长、原因在 status 与界面可见。**生命周期通知**：尝试开始/结束、用量、上游的限流响应头都通知 format（不等回复），
  format 据此实现每分钟/同时在线/每小时、订阅 5 小时与每周额度等任意规则，或自己查上游状态接口；用量统计、按 key 限速、
  预算上限也建在这条通知上。护栏：配置里的静态并发上限是默认值，format 动态覆盖；format 崩溃或超时回落到静态上限，没配
  上限就放行，绝不因 format 坏了卡死队列；format 可一次批准「接下来 N 个 / T 时间内」以减少询问。跨实例共用一把 key 时，
  需要 format 自己共享状态（协议不拦）。订阅额度的读法先实测真实响应头再定，不靠猜。
- 排期：`report`、`admit`、生命周期通知的形状写进 0.2.0 协议定稿；aproxy 的排队放行与官方 aproxy-format 的规则实现放 0.2.0
  之后紧接着（握手保证以后只加不改）。

### W2c 官方 aproxy-format 的能力（用户 2026-10-12：不必全支持，但大部分功能要有可用的官方实现，让大多数情况直接用官方程序）
- 现状：协议转换（Anthropic Messages / OpenAI Chat / OpenAI Responses）只支持非流式，SSE 原样透传；key 轮换（轮流、加权）；
  按模型把请求路由到渠道（newapi 式聚合）；`client_format = "auto"` 只能路由到同协议渠道。只认旧的三段信封。
- **0.2.0 必须有**（按新协议重写接线：握手、阶段订阅、按程序隔离的 state、约定失败结构、按请求覆盖）：
  1. 协议转换全覆盖：请求、非流式响应、**流式实时转换**（stream 阶段，switchyard 流式接口 + 三种协议各自分帧），用三家官方
     SDK 的流解析器做对照测试。
  2. key 管理：轮流、加权、随机；key 遇到 401/403/429 等按规则冷却一段时间、自动换下一把；**会话粘性**（同一段对话尽量用同
     一把 key、同一个渠道，保住上游的提示词缓存命中，按对话特征哈希）。
  3. 渠道路由与故障转移：按模型精确/前缀/正则路由、模型名映射、渠道优先级与权重、在 retry/decide 阶段按规则换渠道。
  4. 决策规则（decide 阶段）：按状态码、错误类别、错误体模式决定重试/换 key/换渠道/停止，支持退避与 Retry-After；停止时按
     客户端协议生成响应（如上下文超长、模型未配置）；也能把「200 但内容是错误」判为失败。
  5. 请求改写：增删请求头、覆盖或删除 body 字段（如给不支持的上游去掉某参数）、模型改名。
  6. 心跳：按客户端协议生成真实的协议事件当心跳（例如 Anthropic 的 ping 事件），解决只认事件、不认注释的客户端（如
     Codex）的空闲超时；每种预设都要拿真实客户端实测过才放进文档（记忆 gemini-cli-sse-parsing 的教训）。
  7. 按请求覆盖：按模型或渠道设超时、保活、重试上限（例如长思考模型给更长的首字节超时）。
- **模块化**：每项能力可单独开关，aproxy-format 可以只在链里扮演其中一个角色（例如只做 key 轮换，转换交给别人的程序）；
  握手时只声明当前配置真正用到的阶段。
- **按角色拆成子命令（用户 2026-10-12 定）**：aproxy-format 是官方实现的合集，`aproxy-format <角色>` 只启用其中一个单元，
  各有自己的参数与配置，在 aproxy 的 `[[format]]` 链里按需挂几个，也能与第三方程序混用；去掉大一统的 `run`。用户的理由：
  性能与稳定性——消息处理有先后顺序，全靠单个进程在超高频时容易阻塞；拆成小单元后每个单元有自己的 worker 池（同一个流
  固定一个 worker 保序，不同请求并行），重活（流式转换）不拖累轻活，一个单元崩了只影响它自己（配合 D-12 按程序熔断，
  判断类单元 `required = false`，坏了就走默认模式）。拟定的角色：`convert`（协议转换含流式；显式 `--from/--to` 预定转换
  路径，省掉逐请求识别，`auto` 可选）、`keys`（轮换、按 401/403/429 冷却、会话粘性）、`route`（按模型路由、改名、渠道优先级
  与故障转移）、`policy`（decide 规则：重试、停止、等待、停止时生成响应）、`adaptive`（见下）、`rewrite`（改头与 body 字段）、
  `heartbeat`（按客户端协议生成心跳事件）；工具子命令 `init`、`check`、`try`；不走信封的一次性转换改名 `translate`，免得与
  `convert` 角色撞名。`init` 只生成场景真正需要的单元，链保持短（每个在请求路径上的单元多一跳本地管道往返）。
- **「先仅转发、失败多次再无限重试」（用户 2026-10-12 提出，主代理细化，用户同意）**：
  - 动机：仅转发是真增量流、对 agent 客户端兼容性好（重试模式要完整缓冲每轮响应再回放，保活心跳对 Claude Code 等客户端
    容易失效），而客户端自带的重试次数少、间隔长。
  - 默认投递方式仍是 retry（用户「对于无限重试依旧保持默认」）：推荐配方与 `init` 不把 forward 设为默认，而是由 adaptive
    对没失败过的请求回 forward；adaptive 坏了（熔断、握手失败）就一律回到 retry，只失去真流式，不失去重试（W2 第二轮复审
    R-S1-1，主代理据用户原话定）。
  - aproxy 只提供机制：请求阶段的回信可以给出本请求的 `delivery`（forward / retry），不给就用配置的默认；仅转发的请求若在向
    客户端写出任何字节之前失败，decide 可以让它当场转入无限重试（重发用的请求体 aproxy 本来就持有：链上有程序要看请求体
    时必须先读完整个体）；请求结束时向订阅了 notify 的程序发「完成通知」（模式、状态码、流是否正常结束、耗时），只发不等
    回信。notify 因此从「只定形状」提前到 0.2.0 实现。
  - 握手时程序除了声明阶段，还声明要多少请求体：不要 / 只要摘要 / 全文。摘要是 aproxy 对原始请求体算的 SHA-256（中立、
    廉价，不含策略），只要摘要的单元收不到正文，几 MB 的请求体不过管道。
  - 判断与暂存全在 format 里（`aproxy-format adaptive`）：请求经过时登记摘要；收到完成通知后把记录标为成功或失败并保留
    一段时间（客户端的重试在失败之后才来，不能一完成就删）；同一摘要再来且上次失败就累计，达到阈值转重试模式，成功清零；
    可配置「需要多次升级后保持重试模式一段时间、稳定后按成功率与延迟退回仅转发」的滞回策略。只存摘要与状态，不存对话
    全文；记录有总数上限与过期时间，内存恒定。重试时请求体是否逐字不变要用真实客户端实测。
  - 与界面的请求历史（D-13）无关：那是实例里给人看的，这是 format 进程里给判断用的。
- **易用性**：`aproxy-format init` 按常见场景生成起步配置（Claude Code 接 OpenAI 兼容中转、多 key 轮换、OpenRouter 等）；
  `aproxy-format check` 校验配置；`aproxy-format try` 用一条样例请求干跑整条处理链、打印每步结果。常见错误模式与渠道写法
  作为配置片段放在文档与 skill 里，不做隐藏的内置特例（记忆 features-are-config）。
- 近期（0.2.0 之后紧接着，见 TODO）：用量与费用统计、按 key 限速、预算上限、Gemini 协议、并发限制与排队。以后可能会做
  （低优先级）：embeddings、图片等非对话接口。
- aproxy-format skill 随之重写配方。format 线版本：协议改为新信封后发 aproxy-format 0.2.0，与 aproxy 0.2.0 同期。

### W2b 强制安装：永久兼容契约（用户 2026-10-11 要求「从现在开始保持绝对兼容」；alpha.1 第 2 步实现，2026-10-12 按独立审查修订）

**保证**：从 0.2.0 起，任意版本 X（≥0.2.0）的 `aproxy install --force` 都能把任意版本 Y（≥0.2.0）装进同一个 home，
并停掉这个 home 下由任意版本（≥0.2.0）启动的全部 aproxy 进程；**不依赖除下面 C1–C8 之外的任何东西兼容**。
这也覆盖「装着的版本坏到跑不起来」：手动下载新版 Y，执行 `Y install --force --from Y` 自装。

**设计原则**：冻结得越少越好（每一项都是永久成本）；只依赖操作系统和下面这几份冻结格式，绝不依赖 IPC、配置、
注册表、install.state 的内容；先验证、后破坏——新二进制下载、校验、并在本机实际跑通 `--version` 之前，什么都不停；
每一步可重入，中途被打断就再跑一次，最终收敛；只按台账里的 pid + 进程启动标记结束进程，绝不按名称；冻结格式只许
追加可选字段，读者忽略不认识的字段，永远不删、不改名、不改类型；万一将来非改不可，只能另立并行的新格式，旧格式继续写。

**冻结项**（2026-10-12 按独立审查 `.agents/review/2026-10-12-force-install-review.md` 修订；实现见 `src/ledger.rs`、
`src/install/force.rs`，样本钉在 `tests/force_contract.rs`）
- C1 home：`APROXY_HOME` 按原始字节读（不要求 UTF-8），为空或只含空白视为未设；设了就原样使用（不 trim），必须是绝对
  路径，否则一切要读 home 的命令报错退出（`--version` 不受影响）。未设时为 `<用户目录>/.aproxy`。用户目录：Windows 取
  非空的 `USERPROFILE`，否则 `FOLDERID_Profile`；unix 取非空的 `HOME`，否则 `getpwuid_r` 的家目录——与三个安装脚本用的
  `%USERPROFILE%`、`$HOME` 一致。一个 home 不支持跨操作系统或跨 pid 命名空间共用（WSL 与 Windows、容器与宿主）。
- C2 二进制位置：`<home>/bin/aproxy`（Windows 为 `aproxy.exe`）。**交换过程从不使用固定的中间文件名**（用户 2026-10-11
  指出：短时间内连续更新两次，上一次的 `.old` 还被进程占着删不掉，固定名必然改名失败）：旧二进制改名为
  `aproxy.old-<纳秒时间戳>-<pid>[.exe]`，新文件先落在 `aproxy.tmp-<纳秒时间戳>-<pid>[.exe]`。时间戳是 19 位十进制纳秒，
  字典序就是时间序（入口兜底脚本据此挑最新的 old）。临时文件用 create_new 建、改名到位之前 `sync_all`；Windows 的改名
  用不覆盖的 `MoveFileExW(flags=0)`——`std::fs::rename` 在 Windows 上会覆盖已有目标，不能用来判断撞名——目标名已存在就
  换一个名字重试。`bin/` 下以 `aproxy.old-`、`aproxy.tmp-` 开头的文件，任何版本都可在删得掉时删除；删不掉（仍被占用）
  就跳过，残留永远不会挡住下一次安装。普通安装也用同一套命名。
- C3 进程台账 `<home>/ledger/`：每个长期运行的 aproxy 进程（后台实例、前台实例、看护者、续作中的安装者、桌面界面）
  启动时、在占用端口或拉起子进程之前，原子写入 `<pid>.json`（先写 `<pid>.json.tmp` 再改名），正常退出时删除。内容是一行
  JSON：`{"ledger":1,"pid":…,"start":"…","role":"…","version":"…"}`，可选 `port`、`label`（自由文本，不可依赖）、`exe`、
  `config`（实例配置文件的绝对路径，拉不回的实例据此告诉用户怎样启动）。`start` 是进程启动标记，推导方式冻结：Windows
  `w:<GetProcessTimes 创建时间的 FILETIME，64 位十进制>`；Linux `l:<boot_id>:<stat 第 22 字段 starttime>`（带 boot_id，
  重启后 pid 与滴答巧合重复也不会认错）；macOS `m:<pbi_start_tvsec>.<pbi_start_tvusec>`（不补零）。`role` 词表只增不减：
  installer、watchdog、instance、desktop；不认识的角色照样结束，排在最后。读者的义务：跳过解析不了的文件；只处理 `start`
  前缀属于本平台的条目，其余既不结束也不删除；删除条目之前重读文件，`start` 一致才删；只结束属主与自己相同的进程（Windows
  比较 token 的 user SID，Linux 比较 uid）。Windows 创建时间的精度是时钟滴答，同一 pid 在同一滴答内被复用理论上可能，接受。
  写台账失败时告警但照常运行（Windows 上写入与删除先短暂重试几次）。
- C4 安装状态收拢：二进制安装的一切状态（状态文件、备料、下载临时文件、安装宣告）只能放在 `<home>/install/` 下，强制安装可随时
  整目录清空（skill 文档的下载暂存在 `<home>/skills/` 下，要与目标目录同卷才能原子替换；强制安装不碰 skill，所以不受影响）；
  `<home>/install/lock` 是操作系统级排他锁（进程死亡自动释放），任何版本修改 `bin/` 期间都要持有它。锁原语冻结：unix
  `flock(fd, LOCK_EX|LOCK_NB)`；Windows `LockFileEx(LOCKFILE_EXCLUSIVE_LOCK|LOCKFILE_FAIL_IMMEDIATELY)`，offset 0、长度
  u64::MAX，CreateFile 的共享模式为 READ|WRITE、不带 DELETE（不用 std 的 `try_lock`：std 声明其实现可能改变）。打开与加锁
  都在有上限的重试循环里。没有例外：unix 的安装宣告是 `<home>/install/announce`（早期开发构建曾放在 `/dev/shm`，alpha.1
  起挪进来），强制安装清空 `install/` 时一并清掉；Windows 的宣告是随进程消失的命名共享内存节，不落盘。
- C5 `--version`：标准输出第一行 `aproxy v<semver> (<variant>; <target>[; x86-64-v3])`；退出码 0；不读写 home、
  不联网、不拉起任何进程，并且在一切副作用之前短路——将来 i18n 在解析参数前读 settings.json 判定语言，`--version`
  必须排在它前面（G2）。输出里任何以空白分隔的词都不得以数字开头（G6）：0.1.0 的安装器取「第一个以数字开头的词」
  当版本，这条保证它在交换前失败（见 W1）。写者在 `)` 之后不得追加任何内容；读者只取版本号之后的第一个括号组、忽略其后
  内容（这是容错，不是许可）。
- C6 发布清单：每个主线 GitHub Release（tag `v<semver>`）都附 `aproxy-manifest.json`：
  `{"manifest":1,"version":"…","assets":[{"target":"<Rust 目标三元组>","variant":"desktop|headless","cpu":"baseline|x86-64-v3","file":"…","sha256":"…"}]}`。
  强制安装只靠它找资产，所以以后资产怎么改名都不影响旧版本的强制安装。写者：不带 BOM 的 UTF-8；`version` 等于 tag 去掉
  `v`；`file` 永远是同一 release 里的裸可执行文件（不是压缩包），每个 release 都为它发布的每个目标保留旧版本能运行的裸
  二进制条目；assets 的每个元素都带齐五个字段。读者：容忍 BOM；assets 逐个元素解析、跳过坏元素与不认识的 variant/cpu；
  清单上限 1 MiB、资产 256 MiB、只走 HTTPS；sha256 在执行之前比对（不分大小写）。永久保证的是 `--from`；在线路径依赖
  GitHub API 的形状与仓库名，是尽力而为。`latest` 只认带清单的 release；解析出的版本低于正在运行的版本时退出 10，不悄悄
  降级。
- C7 命令行：`aproxy install --force [<版本>|latest] [--from <文件>] [--variant desktop|headless]
  [--cpu baseline|x86-64-v3] [--pre] [--download-proxy <地址>] [--yes]`，`aproxy upgrade --force` 等同
  `install --force latest`；`aproxy restore` 子命令永远存在（强制安装后尽力调用它拉回实例）。`--variant` 与 `--from` 同用时
  是核对（目标必须是这个变体），不是选择；显式的 `--cpu` 精确匹配。与 `--force` 冲突的选项（`--no-skills`、
  `--allow-downgrade` 等）由参数解析拒绝（用法错误，退出码 2，什么都不做）；以后放宽不破坏调用方，收紧才会。
  退出码冻结：0 完成；10 什么都没动（一个进程都没结束；非交互且没给 `--yes` 也是 10）；11 至少结束了一个进程，但二进制
  没有替换；12 二进制已替换，但实例没有全部拉回。写输出失败（例如管道已关闭）不得改变退出码。被另一个强制安装结束时退出码
  是 1 或 137，不在契约内。通用退出码只用 0–5（G5，见 W2 的退出码表），10 起留给强制安装，互不重叠。
- C8 流程顺序：
  1. home 不存在就退出 10（多半是 APROXY_HOME 写错了；首次安装用一键脚本）。清掉 pid 已不在的旧 `install/force/<pid>/`。
     取得目标（C6 清单或 `--from`）放进 `<home>/install/force/<本进程 pid>/`，校验 sha256，在本机执行 `--version`（有时限）
     并核对版本；失败即退出码 10，什么都没动（例如 Linux 上缺 webkit2gtk 跑不了带 UI 的版本，就在这里挡住并提示
     `--variant headless`）。
  2. 确认（提示写到 stderr；会中断所有经 aproxy 的在途请求，包括 agent 自己的会话；`--yes` 跳过；非交互且没给 `--yes` 退出 10）。
  3. 按台账结束 installer → 取得 C4 的锁 → 结束 watchdog、instance、desktop 与未知角色；反复扫描直到连续两轮没有存活
     条目（有上限）。结束前在同一进程句柄（Windows）或 pidfd（Linux）上核验启动标记与属主，杜绝 pid 复用的竞态。结束不掉
     就停在这里：一个都没结束是 10，否则 11。
  4. 先删 `install/state.json`，再清空 `<home>/install/`（保留锁与本次的工作目录；unix 的安装宣告随之清掉）。
  5. 替换 `bin/` 下的二进制：先尽力清掉删得掉的旧残留；unix 把 `aproxy.tmp-…` 直接 rename 覆盖到位（原子）；Windows 先把
     现文件改名为唯一的 `aproxy.old-…`，再把 `aproxy.tmp-…` 改名到位，第二步失败立即改回。现文件被不带共享删除权限的句柄
     占着（例如杀毒软件正在扫描）导致改名失败时，短暂退避重试；仍失败就停下，按第 3 步的规则给退出码：一个进程都没结束是 10，
     否则 11（二进制未动，可再跑一次）。
  6. 用新位置的二进制再跑一次 `--version` 核对（有时限）；尽力调用新版本的 `restore`；按台账判断被结束的实例是否回来：同一
     `port` 上出现 `version` 等于目标版本的 instance 条目。没回来的按条目的 `port` 与 `config` 列出来，告诉用户怎样启动
     （退出码 12）。
  7. 另外报告不在台账里、但从 `<home>/bin/` 运行的进程（例如 0.1.x 留下的，但不断言），只报告、不结束；macOS 上不做。
- 强制安装从不读写配置、settings、日志、注册表，也不走 IPC。由桌面界面发起时，界面先把强制安装作为独立进程拉起再退出
  （它自己也在被结束之列），拉起时 stdio 一律为 null 或分离——写输出失败虽不改变退出码，也不该让它写进一个已关闭的管道。

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
- `skill install` / `skill upgrade`：只下载 skill；~~删掉 `install --skills-only`~~（2026-11-21 改：不能删。
  冻结的 `src/install/force.rs` 提示用户执行它，`tests/force_contract.rs` 也传它；改为不在帮助里显示的别名，行为同 `skill install`）。默认取与当前二进制同版本的
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
- 接入真实数据（换掉演示数据）的架构、去留与分段另见 [w5-real-data](w5-real-data.md)（2026-11-19）。
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
    浅/深变体、标题栏最小高度 `titlebarMin`（像素，36–64，默认 44；eb3f730）），设置里导入/导出，不含脚本，只认白名单字段；界面的颜色、圆角、字体、阴影、动效全部走 CSS 变量，根元素暴露稳定的
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
- **Tauri 嵌入调研结论（2026-10-12，证据与来源见 [tauri-embedding-research](tauri-embedding-research.md)；未实际构建，
  §9 的实验 U1–U13 在第 3、4 步做）**。主代理据此定下：
  - Windows：`tauri.conf.json` 写 `build.windows.staticVCRuntime: false`。tauri-build 2.7.x 的默认值会让依赖它的包在
    x64/x86 MSVC 上链接失败（tauri#16238，修复 PR 未合）；aproxy 本来就是动态 CRT，关掉与现状一致。
  - Common-Controls v6：tauri 默认特性 `common-controls-v6` 会静态导入只有 v6 才有的 `TaskDialogIndirect`，exe 没有 v6
    清单就在 main 之前以 0xC0000139 退出——连 `--version`（C5）都跑不了，而清单只经 `rustc-link-arg-bins` 嵌给声明它的包
    自己的 bin，到不了依赖它的 aproxy.exe（tauri#13948）。采用「关掉 v6」：tauri `default-features = false`，特性只开
    `wry`、`compression`、`x11`、`custom-protocol`（About 与「缺 WebView2」对话框降级为 MessageBox）。不依赖清单传递，
    也就不怕以后谁给 aproxy-desktop 加个 bin 让清单悄悄消失。U1 实测确认。
  - RT-05：aproxy 的 lib 永不引用 aproxy_desktop；aproxy-desktop 设 `[lib] test = false`、`doctest = false`；bin 里引用它的
    代码加 `#[cfg(all(feature = "desktop", not(test)))]`。CI 门：每个产物（两种变体）跑 `--version` 冒烟并按 C5 解析；
    `cargo tree -e normal --no-default-features` 里没有 tauri。
  - MSRV：tauri 一族要 1.90，根包 `rust-version` 第 3 步提到 1.90（第 2 步为 `File::lock` 先提到 1.89）。
  - 进程与线程：`aproxy desktop` 在进入 tokio 运行时之前分派（main 改为手建运行时：先 C5 的 `--version`，再 `desktop`，
    其余命令 `block_on`）；desktop 路径 `rt.enter()` 后 `tauri::async_runtime::set(handle)` 一次，事件循环占主线程，
    不在 setup 里 `block_on`。
  - 运行期失败多是 panic（无显示时 tao `expect`，setup 失败 `panic!`），所以建窗口前自己预检：Windows 查 WebView2
    （`tauri::webview_version()`），Linux 查 `DISPLAY`/`WAYLAND_DISPLAY` 与 webkit；不过就友好报错、非零退出。
    `app.windows` 留空，预检通过后用 `WebviewWindowBuilder` 建窗口；Mica 的 `visible:false` 另加 Rust 侧超时兜底 `show()`；
    titlebar 的 `install` 失败降级为原生标题栏，不让 setup 失败。webview 数据目录设到 `<home>/desktop/` 下。
  - 目录：新成员 `aproxy-desktop/`（与 aproxy-envelope、aproxy-format 并列），前端从 `desktop/` 挪到 `aproxy-desktop/ui/`，
    Vite 产物出到 `aproxy-desktop/dist/`；crate 的 `include` 带 `dist/**`、`tauri.conf.json`、`capabilities/**`、`icons/**`，
    不带 `ui/**`；`gen/` 忽略。`tauri-build` 校验时往包根写 `gen/schemas`，`cargo publish` 的校验可能报「Source directory
    was modified by build.rs」，第 4 步用 U2 决定预生成并纳入包，还是 `--no-verify` 加单独的解包构建验证。
  - 安全：CSP 按「不联网」写（草案见附件 §7），原型产物里 6 处内联 `style="animation-delay:…"` 改成 CSS 变量或类，
    不开 `style-src-attr 'unsafe-inline'`（U8 确认）；单一 capability、自定义命令用 app manifest 收紧。**扩展与界面同一
    JS 域时能直接调 invoke，「写操作需同意」只能在 Rust 侧强制**，JS 层的同意包装拦不住——扩展系统（W5 后续）按这个设计。
  - Mica：直接调用钉死版本的 window-vibrancy（`=0.8.1`，cfg(windows)）拿到支持与否（Tauri 会吞掉错误）。
  - CI：ubuntu-22.04 runner 2026-09-17 起弃用、2027-04-17 退役，Linux gnu 带界面版改用 `container: ubuntu:22.04` 跑在
    ubuntu-24.04 与 24.04-arm 上；该变体 glibc 下限约 2.35、运行期要 WebKitGTK ≥ 2.40（headless 仍用 zigbuild 压到 2.28）。
    Alpine 的 arm64 不能用 job container，要宿主 checkout 后 `docker run`。
  - crate 名 `aproxy-desktop`、`aproxy-ipc` 在 crates.io 未被占（2026-10-12）。
- **原型交接给第 3 步的事项（UI 原型代理 2026-10-12）**：原型已提交（60c1924，`desktop/`）；接 Tauri 时删掉 `src/proto/`（审核
  工具条、仿系统窗口与壁纸、模拟 Mica 的 blur）与开发态的 `window.__aproxyProto`。自定义吉祥物在 `<iframe sandbox="allow-scripts">`
  里经 blob URL 导入，CSP 只给 `mascot-sandbox.html` 放行 `script-src blob:`。在 WebView2 里实测总览横幅的 `.rain`（重试时出现，
  动画改 background-position 且带 mask-image，每帧重绘整个横幅），开销明显就改成 transform 移动大图层。图表数据色有意不随
  主题变（按色盲可区分度校验过）。哪些配置项「实时生效 / 需重启」是原型的提议，接入时对照真实的热重载行为改准。
- 待定：
  - **所有平台都发两个版本（用户定）**：现有 11 个构建（Windows x64 / x64-v3 / x86 / arm64，Linux gnu x64 /
    x64-v3 / arm64，musl x64 / arm64，macOS arm64 / x64）各出带 UI 与无 UI 两份，共 22 个；2026-10-12 用户改为 musl 只出 headless，实为 20 个（见下）。由此带来的事：
    - Linux 的 UI 版动态链接 webkit2gtk / GTK：缺库时 ld.so 在 main 之前就失败，连 `--version` 与代理都起不来；
      wry/tao 没有延迟加载 webkit 的机制（附件 §5 列了 L1–L5 五种做法）。`cargo install aproxy` 默认编带界面版，
      没装 webkit 开发包的 Linux 服务器上会编译失败。**用户 2026-10-12 选定「自动选择变体」**：命名不变、两种都发；
      一键脚本（install.sh）与 `aproxy install` / `install --force` 在 Linux 上探测 webkit2gtk-4.1（≥ 2.40）与 glibc
      （≥ 带界面版的下限），满足才装带界面版，否则装 headless 并说明原因与之后怎样切换（`--variant desktop`）；npm 的 JS
      启动器认出这种加载失败（退出码 127、stderr 含 `error while loading shared libraries`）时提示改装
      `@meowo/aproxy-headless`；README 与 skill 写明 Linux 服务器上 `cargo install aproxy --no-default-features`、binstall
      与手动下载选 `aproxy-headless-<target>`。升级时保持原变体（`--version` 的变体段，W1 已实现）。
    - gnu 的 UI 版不能沿用 cargo-zigbuild 压低 glibc 下限，glibc 下限由 webkit2gtk 决定（构建方式见上一节的 CI 一条）。
    - musl 的 UI 版不可能静态链接：要关 `crt-static`、在 Alpine 里对 musl 版 webkit2gtk 动态链接（rustup 的 musl 工具链
      默认 crt-static，GTK 程序启动即段错误，gtk-rs-core#387），只能跑在装了 webkit2gtk 的 musl 发行版上；没找到任何
      公开的 Tauri 2 musl 预编译先例（tauri#12296 未解决）。**用户 2026-10-12 选定 musl 只出 headless**：产物共
      20 个（9 个带界面 + 11 个 headless）；musl 目标的 `install` / `install --force` 默认且只能选 headless，指定
      `--variant desktop` 时说明 musl 没有带界面版（清单 C6 里也就没有这一项）。
    - **命名（用户 2026-10-11 选定 headless）**：带 UI 的沿用现有名字——GitHub 资产 `aproxy-<target>[-v3][.exe]`、
      npm `@meowo/aproxy` 与 9 个 `@meowo/aproxy-<平台>`、crates.io `aproxy`（feature `desktop` 默认开，binstall
      的默认命名正好对应它）；无 UI 的叫 headless——资产 `aproxy-headless-<target>[-v3][.exe]`、npm
      `@meowo/aproxy-headless` 与 9 个 `@meowo/aproxy-headless-<平台>`、cargo 用 `--no-default-features`，不另建 crate。
      `npm/build-and-publish.sh` 已有按组切换包名与资产前缀的结构（format 组），headless 加一组即可；JS 转发器按主包
      名推导子包前缀，不用改。两个主包都提供 `aproxy` 命令，不能同时全局安装。
    - **npm 种子已发（2026-10-11）**：10 个 headless 包以 0.0.0 占位版本发布；**可信发布绑定待所有者完成**
      （GAT 做不了，见记忆 trusted-publishing-p0 的 2026-10-11 一节）。crates.io 不需要新配置，已核对现有绑定都在。
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

## 执行顺序与进度（2026-11-11 起）

用户 2026-11-11：「不需要单独发版。处理以上问题。完成0.2.0的核心功能，现在发的版本功能还缺很多，需要发出真正功能完备、
可以使用、有精美UI的0.2.0版本。过程中持续提交和设置tag」。据此：提交与推送 main、在里程碑处打预发布 tag 都已授权；安全修复
不单独发版，随下一个 tag 发出。

**做法**（每个后来接手的会话照此执行）：
- 主代理直接实现各块（P1 已派出的子代理除外），少用子代理；只把边界清楚的小任务交出去。本机可用内存约 4.4 GB
  （15.6 GB 总量，2026-11-11），同一时刻只跑一个 cargo 重编译，所以 Rust 的块一个接一个做；不编译的工作（前端对 mock
  开发、文档、skill）可以并行，并行改前端时用独立 worktree（`npm run build` 会改 `aproxy-desktop/dist/`，与 cargo 构建带
  界面版冲突）。
- 每块的验收按 [w2-contracts](w2-contracts.md) §14 的表；提交用 `git commit -- <路径>`；两个变体 clippy `-D warnings`
  零输出；推送后看 CI（windows、ubuntu 两种变体与 upgrade 任务是门槛，macOS 那 4 个 lib 测试是已知红）和 Review
  （cargo-deny 也要绿，它会因新公告变红而与代码无关）。
- 打 tag 的步骤：用 `.github/scripts/align-version.py` 的规则改版本号（根包、aproxy-desktop、aproxy-envelope 同号，
  见 fe88cdf）→ 推送、CI 绿 → 自上次成功发布以来 release.yml 或打包有改动就先跑一次 workflow_dispatch 演练 →
  打 tag 推送 → 盯发布 run → 核对 GitHub Release、npm 两组的 `next`、crates.io。
- **新 crate 的首发**：✓ aproxy-ipc 0.0.0 已于 2026-11-11 种子发布，可信发布配置 #33128（release.yml / release）。B0 落地时
  还要：release.yml 的 crates 发布顺序改为 aproxy-envelope、aproxy-ipc → aproxy-desktop → aproxy；align-version.py 一并对齐
  aproxy-ipc 的版本与根包、aproxy-desktop 对它的 `=` 要求；`rehearse-crates` 打包它。

**2026-11-20 起**：实现、审查、看 CI 与性能测量都交给 Codex，主代理只做分配、裁定、合并与给人看的文字。接下来的任务按先后
列在 [codex-tasks-0.2.0](codex-tasks-0.2.0.md)；性能的基线、重测与优化见 [perf-0.2.0](perf-0.2.0.md)。上面「做法」里
「主代理直接实现各块」一句已不适用，其余（验收、提交、打 tag 的步骤）照旧。

**阶段与 tag**（✓ 为已完成）：
- 阶段 0（2026-11-11）：✓ 依赖安全修复 9e45495（12 条 RUSTSEC，zip 升 9）；✓ Actions 升级 9087969；✓ TODO 整理。
  dependabot：#8、#9、#11–#15 由这两个提交覆盖；#10（base64 0.23）关闭——headless 构建里 base64 只有一份 0.22，
  envelope 单独升会多一份重复，等 reqwest、hyper-util 换版本时再看；#6（toml 1.x）、#7（sha2 0.11）归 P3。
- 阶段 1，前置重构：P1 → P2 → P4 → P3 → P5（§14.1；内存所限不并行）。
- 阶段 2，契约块：✓ B0 → ✓ B1 → ✓ B2 → ✓ B3 → ✓ B4 → ✓ **tag alpha.2**（2026-11-17）→ ✓ B5 → ✓ B6 → ✓ B7 → ✓ B8 → ✓ B9 → ✓ **tag alpha.3**（2026-11-19）→
  B10a → B10b、B10c → B10d → B11 与 W2c → ✓ **tag alpha.4**（2026-11-21，打在 cf8916b；Release 47 个资产，四个 crate 已在 crates.io）。B12（文档、skill、记忆）随各块。alpha.2 之后补跨版本强制安装
  的 CI 测试（TODO 的 W2b 一条）。
- 阶段 3：W4 `aproxy skill`、W3 i18n、W5 界面接入真实数据（前端可提前对 §6 的形状做 mock 侧的工作）→ **tag beta.1**。
- 阶段 4：W6 修复（含 restart 偶发、`install --from` 降级检查）、0.1.0 skill 支线的防护（W4 一节最后一条：skills 资产换名、
  npm 包内路径换、crate 不再带 `.claude/skills/`）、性能重测与优化（[perf-0.2.0](perf-0.2.0.md)）、文档与 README 截图、W7 撤回预发布措辞与重写 SECURITY.md →
  **tag rc.1** → **v0.2.0**（aproxy-envelope 同时发 0.2.0；aproxy-format 0.2.0 随后走 format-v* 线，见 TODO 的 envelope 一条）。

**进度记录**（每块完成时追加一行：块、提交、验证）：
- 2026-11-11 CI 卡死修复 8e97317：依赖升级后 `interrupted_stream_is_retried` 在 Windows 上必现卡死，是 mock 不读请求就关闭
  （RST 吞响应），见记忆 disconnect-billing-protection 的 2026-11-11 一节。
- P1 c8b2ac9（tests/common 的 TestHome、源码检查 tests/test_home_check.rs）：Windows headless 全量通过（lib 287、
  proxy_integration 102 等，各文件测试数与改动前一致）；带界面变体与 ubuntu 看 CI。观察项 `alias_start_and_stop_roundtrip`
  没有单独循环复测（子代理中途停了），全量里通过。
- P2 第 1 步 a5995d6（proxy.rs 拆成 src/proxy/ 15 个模块，纯搬移）：库单测 287、proxy_integration 102、
  transform_integration 30 与拆分前一致。第 2、3 步：`attempts::classify` + 唯一的尝试循环 `run_attempts`（两种出口
  `Responder::Buffered / Keepalive`），557f748；Windows headless 全量通过（库单测 292 = 287 + classify 的 5 个，其余各组数目不变）。
- P3（2026-11-12）：4a181e2 删 tower / tower-http（R2a），452afec axum 0.8（测试里假 GitHub 服务的路由参数语法随之改），
  0ddfd17 toml 1.x（aproxy 与 aproxy-format，与 tauri-utils 共用一份），43ddda8 sha2 0.11；zip 9 已在阶段 0。
  **reqwest 0.13 推迟到 0.2.0 之后**（主代理 2026-11-12 定）：0.13 的 `rustls` 特性改用 aws-lc-rs（musl、aarch64 交叉编译
  更麻烦）并固定带上系统信任库校验器，要保住现在「内置 webpki 根证书」的行为得自己用 `use_preconfigured_tls` 拼 rustls
  配置；系统代理的读取方式也变了，需要对着记忆 proxy-config 的 7890 场景重新实测。收益很小：tauri 带入的 0.13 只开
  json、stream，不带 TLS，不构成两套 TLS 栈。0.12.28 目前没有安全公告。
- P4（2026-11-12）：516b4c9 拉起并等就绪合成 `spawn::spawn_instance`（start、restart、restore、安装的滚动重启、看护者
  重拉五处共用；start 有了早退检测，`tests/spawn_instance.rs` 钉住「启动即死 1 秒内报告并附 startup.log」）与原子写合成
  `fsx::write_atomic`（临时名带纳秒、create_new、sync_all，unix 0600）；e55c660 进程原语从 watchdog.rs 搬进
  `src/process.rs`（纯搬移，相关单测随行）。Windows headless 全量通过（库单测 295），两个变体 clippy 与 Linux 交叉 clippy
  零输出。sync_all 拉长了守护「写注册表 → 写恢复记录」之间的窗口，暴露了 install_flow_lib 测试只等注册表就读 .restore
  的竞态（ubuntu 带界面变体失败），5af8b39 修。
  **`home.rs` 改随 B0 做**（主代理 2026-11-12 定）：B0 本来就要改 force_contract 的 `use` 行、文件头那句与 AGENTS.md、
  CLAUDE.md 的契约段，home 的解析放到那时一起搬，这几处只改一次；home 的解析也可能该进 aproxy-ipc（`home_id` 与端点名在
  那里），先搬进主 crate 会白搬一次。注意 force_contract 第 28 行 `aproxy::settings::user_dir()` 是函数体里的全路径、不是
  `use` 行，搬 home 时它也得改——同属「`use` 路径随代码移动」（R-S3-15），B0 的提交说明里写明。
- P5（2026-11-16）：b426201 操作层 `src/ops/`（target、start、stop、restart、restore、alias、list），命令层只渲染；
  输出文字、去向与退出码不变（restart_integration、install_flow 等原有集成测试照过）。`tests/lib_is_quiet.rs` 守着库里没有
  `println!`、`eprintln!`、`process::exit`。附带的挪动：cli.rs 进库、`remove_registry_files` 进 daemon、`ledger::exit` 改为
  二进制一侧的 `util::exit`、看护者 `tick` 返回 `Tick::Retire` 由 main 结束进程。Windows headless 全量通过（库单测 295），
  两个变体 clippy 与 Linux 交叉 clippy 零输出。
  **操作层现在的边界**（给 B5、B9、W3 接手时看）：结果是带数据的枚举，但从底层带上来的原因仍是中文字符串
  （`reason`、`error` 字段：配置校验、bind、IPC 错误），`resolve_runtime_config` 也还返回 `Result<Config, String>`——
  错误码表（§2.4）随 B0 进 aproxy-ipc 之后，B1/B2 把这些字符串换成码加数据，B9 再接 `--json` 与退出码表。
  restart 的预检还要用 clap 解析原启动参数（所以 cli.rs 在库里），B5 换成 spec 文件后这层解析可以拿掉。
  install、config、logs 三个命令还没有进操作层：install 的流程已在库（`install::flow`），命令文件里是渲染与退出；
  config 与 logs 留到 B9 随 `config get/set/unset` 与 `--json` 一起做。
- f9ff451（2026-11-16）：本机与 CI 的 stable 升到 Rust 1.100。cargo 1.100 把单测程序放到
  `target/<profile>/build/<包名>/<hash>/out/`，`transform::format_echo_path` 改为逐级往上找；clippy 新报的 needless_bool 一处。
- B0（2026-11-16）：新 crate aproxy-ipc，直接依赖只有 serde、serde_json、sha2，Windows 上钉版的 windows-sys，unix 上 libc
  （没有 tokio：这一步迁入的东西都不做异步 IO，B6 做 IPC 客户端时再看要不要）。
  - 1607caa：home 的解析（C1）进 `aproxy-ipc/src/home.rs`，台账（C3）进 `aproxy-ipc/src/ledger.rs`，算法逐字不变。
    `tests/force_contract.rs` 只改三处 `use` 路径与文件头一句，断言不变并通过。AGENTS.md 与 CLAUDE.md 的契约段同步改，
    `tests/contract_docs.rs` 守着两份逐字一致、列出的四个文件都存在、末句在。发布侧：align-version.py 对齐 aproxy-ipc
    （本机用 DRY_RUN_VERSION 试跑过，四个包与 Cargo.lock 的四个条目都改到），release.yml 的 crates 顺序加入它。
    **release.yml 改过，打 alpha.2 之前要先跑一次 workflow_dispatch 演练。**
  - 41b604d：端点名与 `home_id` 进 `aproxy_ipc::endpoint`。
  - 3b34cc8：码表 `aproxy_ipc::code`（§2 的全部码，带族、`details` 的键与类型、HTTP 状态、退出码、诊断级别；
    `table_json()` 导出），`docs/error-codes.md` 由它生成、`tests/code_table.rs` 比对；打码 `aproxy_ipc::redact`
    （`url` 由 `config::mask_base_url` 迁入并改掉全部调用点；密钥头规则、占位对象、已知密钥替换与流式替换是新写的，
    还没有调用方）。
  - 验证：Windows headless 全量通过（库单测 280 + aproxy-ipc 28），两个变体 clippy 与 Linux 交叉 clippy 零输出。
  - **与设计稿的出入，接手时注意**：
    1. `redact::url` 判断 authority 是否规整不再用 url crate，用自己的检查，比 URL 标准严（国际化域名、带 `%` 的主机、
       非四段点分十进制的数字主机按畸形处理）。迁移时拿旧实现在 23520 个输入上差分过：不同的 972 个都等于「按最后一个
       @ 保守切分」的结果，没有露得更多的。
    2. 安装码（`install_*`）只登记了设计稿点了名的十个，`details` 多数留空：§2.4 写明由 install 的 `--json`（B9）补齐。
    3. 码表目前只是登记，没有出口在用：B1 接数据面，B2/B3 接诊断，B6 接 IPC，B9 接操作码与退出码。
    4. `ledger::register` 不再自己告警（aproxy-ipc 不带 tracing），原因放在返回的守卫里，由 `util::register_in_ledger` 告警。
- B1（2026-11-16）：6062b8c 数据面的错误出口。`src/proxy/errors.rs` 是唯一的构造处：`Terminal`（终态，JSON 错误体或
  SSE `event: error`，同一个 JSON）、`AttemptFailure`（尝试失败码）、`mark_given_up`（给放弃后透传的响应加两个头）。
  `request_id` = `<instance_id>-<序号>`，在头、体、日志（span）与 format 信封里相同。skill 文档已按新形状改
  （子代理改、主代理审，纠正了一处「响应侧日志也带 code」的错误说法）。
  - 验证：Windows headless 全量通过（库单测 284，proxy_integration 106，transform_integration 30）；两个变体 clippy 与
    Linux 交叉 clippy 零输出；CI 上 windows、ubuntu 两种变体与 upgrade 任务全绿（macOS 是已知红）。
  - **真实 SDK 解析**（anthropic 2.8.0、openai 3.51.0，Python，`max_retries=0`，对着本机构建的 aproxy）：
    | 场景 | anthropic SDK | openai SDK |
    |---|---|---|
    | 自产 413 `request_too_large` | `ContentTooLargeError`，`status_code` 413，`e.type` = `aproxy_request_too_large`，`e.body` 是整个错误体（含 `error.aproxy`） | `APIStatusError`，413，`e.type` = `aproxy_request_too_large`，`e.body` 是 `error` 对象（含 `aproxy`），`e.code` 为 None |
    | 放弃后透传的上游 404（`retry_exhausted`） | `NotFoundError`，体是上游原样；`e.response.headers` 里读得到 `x-aproxy-error` 与 `x-aproxy-request-id` | `NotFoundError`，同左 |
    | 流里的终态 `event: error` | 抛 `APIStatusError`（`e.type` = `aproxy_retry_exhausted`，`e.body` 是整个 JSON） | 抛 `APIError`（`e.type` 同，`e.body` 是 `error` 对象，`e.message` 是英文说明） |
    两个 SDK 都把 `error.type` 当字符串读、`error.aproxy` 原样保留在 `body` 里，流里的终态事件都当错误抛出、不当数据。
    openai SDK 的 `e.code` 是 None——设计稿定的就是「`error.code` 不放任何值」。流式那两行喂的是与 `Terminal::sse_event`
    同形状的一帧（由脚本里的假服务端写出），不是经 aproxy 的保活通道触发的；保活通道写出这一帧由集成测试核对。
  - **没有集成测试的终态**：`local_storage_failed`（要让磁盘缓存写失败，没有不侵入的办法；`errors.rs` 的单测核对了它的
    形状）、`internal`（程序缺陷，触发不了）。
  - **与设计稿的出入**：
    1. 现在只有一个请求转换器、没有名字，`details.format` 一律是 `request_transform`；B10 引入 `[[format]]` 链之后才是配置里的名字。
    2. `retry_exhausted` 的 `details.by` 现在是 `bounded_retry_paths`（设计稿的 `rule:<序号>` / `override` 要等 B4 的规则表）。
    3. `format_unavailable`、`retry_stopped`、`shutting_down` 还没有触发处：分别等 B10 的熔断、decide 阶段，与 B5/B6 的退出状态。
    4. 首字节超时与读空闲超时现在共用 `read_timeout_secs`（reqwest 的 read_timeout 同时钳制两者），码已经分开；B4 把两个时限拆成各自的配置。
    5. `upgrade_unsupported` 只拦 WebSocket；`h2c` 之类按 HTTP 的规定忽略、照常代理（设计稿写的是「Upgrade（WebSocket）请求」）。
- B2（2026-11-16）：5518090 config.toml 的新键、严格解析、分层解析与写回；26320b5 模块文档。`src/config/` 现在是四个文件：
  `file.rs`（文本 → `FileConfig` + 带行列的诊断，不补默认值、不读文件系统）、`resolve.rs`（四层合并 → 运行时 `Config`、
  `sources`、诊断；`Defaults::from_settings` 把现行 settings.json 变成全局默认那一层）、`edit.rs`（toml_edit 写回）、
  `mod.rs`（运行时 `Config` 与语义校验 `Config::problems`）。诊断类型在 `aproxy-ipc/src/diag.rs`。
  - 验收逐条：§7.2 样例能解析（`file.rs` 单测）；未知键让启动失败并指出文件、键、行列（集成测试
    `unknown_config_keys_fail_start_naming_file_and_key`，顶层、子表、规则、format 条目四种）；四层 `sources`
    （`resolve.rs` 单测）；写回保留注释（单测 + 集成测试 `config_command_edits_in_place_and_unsets_unknown_keys`）；并发写不丢更新
    （单测 8 线程 × 6 次，集成测试 6 个进程）；`unset` 删未知键（同上）。外部修改检测与 `Conflict` 有单测（用注入的钩子扮演
    不拿锁的写者）。
  - 验证：Windows headless 全量通过（库单测 305，proxy_integration 109，transform_integration 30）；两个变体 clippy 与 Linux
    交叉 clippy 零输出。**没有在 unix 上跑过**的：`edit.rs` 的 0600 断言与符号链接测试（只在 unix 编译），等 CI。
  - **顺手做掉的 B4 部分**（它们与键的改名分不开）：规则表接进了尝试循环（第一条命中生效、`max_attempts = 0` 豁免、按尝试序号
    计数，`attempts.rs` 单测）；`retry_exhausted` 的 `details.by` 是 `rule:<序号>`；首字节与读空闲两个时限各自计时
    （`upstream.rs` 的 `send_with_timeout`、`next_chunk`，不再用 reqwest 的 read_timeout）。B1 记的出入 1、2、4 由此消除。
    剩下的由 dc5ee49 补齐：`max_response_mb = 0` 表示不限；集成测试覆盖两个超时各报各的码、读空闲时限在每一块上
    重新计时、上游的 `retry-after: 600` 不影响第 2 次尝试立即发出。**B4 至此完成**（按请求覆盖的 `retry.max_attempts`
    与 decide 属于 B10）。仅转发模式也走同一对计时函数，但没有单独的超时测试。
  - **定下的事（设计稿没写）**：
    1. 旧的三个 transform 表在 B10 之前怎么办：不留。文件只认 `[[format]]`，`resolve.rs` 的 `assign_formats` 把条目派给现有的
       请求、响应、心跳三个位置（每个阶段至多一个程序；`retry` + `request` = 旧的 `every_attempt`）。`decide`、`stream`、`notify`、
       同一阶段第二个程序、只有 `retry` 没有 `request`，报 `config_not_supported`。`required`、`decide_timeout_secs`、`breaker_*`、
       `history.body_budget_mb` 只解析校验、还不起作用。旧的 `extra` 键没有对应物（用 `args` 或 `env`），信封的 `extra` 因此为空。
    2. `delivery = "forward"` 与 `[[format]]` 仍互斥（`config_conflict`，键为 `delivery`），到 B10d 才放开。
    3. `set` 没有「已知键」表：不认识的键、类型不对的值都会让写前校验多出一条诊断，于是被拒绝。
    4. 写前校验按「码 + 键 + 说明」比对新旧诊断，行列不参与（改动会让行号移动）。
    5. 带换行的字符串写回成一行、换行写成 `\n`（toml_edit 默认写多行字符串，开头多一个换行，心跳这种值看不清）。
    6. 运行时 `Config` 的字段名没有跟着改（`max_body_mb`、`spool_limit_mb`、`forward_only` 等仍是 0.1.x 的叫法），对应关系写在
       `src/config/mod.rs` 的模块文档里。改名是纯机械的大改动，留给以后；对外只有文件里的键名。
  - **与设计稿的出入**：
    1. `aproxy config` 还是旧的旗标形态（加了 `--unset`），`config set/get/unset/show` 的命令树、`--json`、`-` 从 stdin 读、
       `instances_to_restart` 属于 B9。`config --show` 已经是「生效值 + 来源 + 诊断，有 error 时退出 3」。
    2. 凭据只发给所属目标、`withheld`、`default_credential_withheld`（D-16）没有做：现行 settings.json 里没有 `base_url`、`api_key`
       这些默认值，要等 B3 的 `defaults`。`Defaults` 这一层已经能装它们。
    3. 「同一目标」的 base_url 规范化（scheme、主机小写，去默认端口）没有做，随 B3。现在仍只去末尾斜杠。
    4. `[[format]]` 的 `command` 仍按旧规则找（`~` 展开、裸名走 PATH）；「相对路径相对 home、裸名先找 `<home>/bin/`」随 B10。
    5. 写锁等 10 秒拿不到时报「另一个 aproxy 正在写配置」，退出 1；码表里没有专门的码（`write_conflict` 只指外部修改）。
    6. `resolve` 的签名是 `resolve(path, &Overrides, &Defaults)`，第三个参数是已经取好的全局默认层而不是 `Settings`：这样
       B3 换 settings 的形状时不用动它。
- B3（2026-11-17）：26883f1 settings.json 的新形状、严格解析、`settings` 命令与凭据继承规则。`src/settings.rs` 拆成
  `src/settings/`：`file.rs`（文本 → `Settings` + 带行列的诊断；`defaults` 一节转成 TOML 交给 `config::file::parse`，键名前面
  加 `defaults.`）、`edit.rs`（set/unset、占位对象的还原、读出口打码）、`mod.rs`（`read`/`load`/`update`）。写入走
  `config::edit::rewrite_locked`（同一把 `run/config-write.lock`），用 `fsx::write_atomic_private`。
  - 验收：四个集成测试（`invalid_settings_block_start_but_not_stop_status_or_repair`、
    `settings_command_reads_writes_and_never_shows_secrets`、`settings_defaults_reach_config_and_credentials_follow_their_target`、
    `corrupt_settings_json_is_never_overwritten`）；库单测 324。
  - 验证：Windows headless 全量通过，只有 `ipc_stats_reflect_forward_only_traffic` 红过一次——`free_port()` 取到的端口在
    实例启动前被别的并行测试占了（startup.log 写的是「端口被其他程序占用」），单跑通过，是测试自身的老竞争，同 B3 无关。
    两个变体 clippy 与 Linux 交叉 clippy 零输出。unix 的 0600 断言等 CI。
  - **定下的事（设计稿没写）**：
    1. settings.json 无效时各命令怎么办：`settings` 与 `doctor` 照常（自己报告）；`status`、`stop`、`install`/`upgrade` 用内置
       默认运行，把错误打成 `[WARN]`；其余命令退出 3 并给修法（每个出问题的键一条 `aproxy settings unset`）。实例不启动。
       不是 JSON 对象的文件只能手工修，写回型命令一律拒绝、原样保留。
    2. 长驻进程（看护者）读到无效的 settings 时沿用上一次有效的那份（按路径缓存），只告警一次；没有上一次就用内置默认。
    3. 内存里的 `Settings` 字段名没有跟着分节（仍是 `log_rotate_mb`、`watchdog_heartbeat_secs` 这些），只有文件里的键变了。
    4. `defaults` 里不收 `listen_addr`、`log_file`、`format`；凭据要先有归属（`defaults.base_url`、`defaults.proxy`），否则
       `config_missing_key`。
    5. `settings set` 的值：字符串类型的键可以直接写原文，以 `"` 开头按 JSON 字符串解析；`-` 从 stdin 读一行。
    6. Windows 上的私有写入：DACL 只留当前用户、SYSTEM、Administrators，受保护（不继承）。config.toml 还没有这样写（B7）。
  - **与设计稿的出入、没做的**：
    1. `settings` 命令提前做了（命令树属于 B9）；`--json` 输出还没有。
    2. `language` 只解析、不起作用（W3）；clap 之前读语言也没做。
    3. doctor 的 `default_header_shared`（info 级）没做：doctor 现在是自己的两级 `Level`，随 B9 的 doctor 一起换成诊断类型。
    4. 0.1.x 的平铺 settings.json 直接被拒（退出 3 + 修法），没有自动迁移。对照表写进技能的 compatibility.md；
       **W7 发版前要定**：是否在 `install` 换版时自动迁移（0.1.x 用户升级后第一条命令就会撞上）。`upgrade_from_0_1_0` 的 CI
       测试用的 settings 仍是平铺的（给 0.1.0 的二进制读），升级后没有再跑要读 settings 的命令，所以没暴露这一点。
    5. 「0 的含义统一」没有逐项核对。
- **v0.2.0-alpha.2 已发布**（2026-11-17，tag 在 4cfc363）：打 tag 前 workflow_dispatch 演练 run 43929031519 全绿（B0 改过
  release.yml）；发布 run 43934123484 成功。核对过：GitHub 预发布、47 个资产；npm `@meowo/aproxy` 与
  `@meowo/aproxy-headless` 的 `next` 都是 0.2.0-alpha.2（`latest` 没动）；crates.io 上 aproxy、aproxy-ipc、aproxy-envelope、
  aproxy-desktop 都是 0.2.0-alpha.2。发布前补了 CHANGELOG（两张键名对照表、统一的错误出口、settings 命令）、技能与 README
  （7a… 之后的 `docs:` 提交），文档里「alpha.1 的界面是预览版」改成了「各 alpha 版」。
  跨版本强制安装的 CI 测试已在 2026-11-19 补上，见文末「跨版本强制安装」一条。
- B5 第一部分（2026-11-17）：b7d2b27 拉起规格与期望状态记录。新模块 `src/spec.rs`（`InstanceSpec`、
  `run/<端口>.restore.json`、交接文件 `run/<纳秒>-<pid>.spec.json`）；内部入口 `aproxy __serve --home --spec` 与
  `aproxy __watchdog --home` 取代两个隐藏旗标；`spawn::spawn_from_spec` 是唯一的拉起函数（就绪时限 30 秒）；
  `daemon::spawn_detached_with` 脱离启动者（unix `setsid`，Windows `CREATE_BREAKAWAY_FROM_JOB`、被拒就退回并在 startup.log
  记告警）、可改环境与工作目录；shutdown 的 `keep_restore` 与快照的 `features`；`APROXY_RUN_DIR` 删除。
  - 验收里已有测试的（都在 `tests/restart_integration.rs`，另有 `tests/spawn_instance.rs` 与各模块单测）：命令行里没有密钥
    （实例、看护者、看护者重拉出的实例；Windows 用 `Get-CimInstance` 读命令行，unix 用 `ps`）；只设大写 `HTTP_PROXY` 启动、
    restart 之后仍走代理；对启动者进程组发 SIGINT 实例仍在（unix，等 CI）；restart 遇坏配置不动旧实例（原有测试）；restore
    遇暂时缺失的配置保留记录；`stop` 之后不被拉回（原有测试 + 崩溃后 stop 删记录）；端口 0 的实例崩溃后回到同一端口、
    run/ 里只有一份记录；钉过端口的配置改成固定端口后 restart 用新端口、旧记录被接替；没有配置文件的实例能 restart、restore；
    前台实例 restart 被拒；读不懂的记录不被删除（`spec.rs` 单测）。
  - 验证：Windows headless 全量 665 通过、0 失败；两个变体 clippy 与 Linux 交叉 clippy 零输出。
  - 第一部分做完时列的「还没做的」已经被下面第二到第四部分接过去，当前的清单在「B5 第二到第四部分」末尾。
  - **定下的事（设计稿没写）**：
    1. restart 没有期望状态记录的实例（前台实例）时报「没有可用来重新拉起它的记录」，退出 1；`restart_unsupported` 这个码
       等 B9 接操作码时再用。
    2. restart 的强制终止（`restart --force`）不删记录；看护者可能抢先拉起，新实例绑不上端口时若端口上已是同一配置的
       新实例就算成功（§5 restart 第 5 条）。
    3. 旧实例（没有 `shutdown.keep_restore` 这个 feature 的）：先发不带参数的 shutdown，等端点消失后把记录原样写回。
       alpha.1、alpha.2 的实例写的是旧的 `.restore`，本版本不读，所以这条路径实际只会遇到没有记录的情形。
    4. 给了配置路径而文件不在一律是错误（原来只有命令行显式 `--config` 才是）；默认的 `config.toml` 不存在、又没显式给
       路径时，规格的 `config_path` 记 null，启动提示写「没有配置文件」。
    5. 子进程的 home 由 `--home` 定（`main` 把它写进本进程的 `APROXY_HOME`），不由继承来的环境定。
    6. 看护者重拉时配置文件不在：报失败、进退避队列，记录保留；退避期间记录被 stop 删掉的条目直接出队。
    7. 安装状态的 `pending_restores` 不再写也不再读（恒为空），在途实例靠「上次落盘的端口快照 + 期望状态记录还在」认出来。
- B5 第二到第四部分（2026-11-17）：看护者慢速重试与重读 settings、读路径只读与清理、心跳挪进 home、启动顺序的一处竞态。
  提交依次是 dcc6afa、8df5c69、1889c19，文档 6148ae4。
  - **看护者**（`src/watchdog.rs`）：连续失败达 `watchdog.max_restarts` 后不放弃，每 300 秒重试一次（`SLOW_RETRY_SECS`），
    直到实例起来或记录被 stop 删掉；`max_restarts = 0` 仍是「不自动重拉」，只有这种情形才 `GaveUp`。重拉出的实例存活满
    60 秒（`HEALTHY_AFTER_SECS`）才清零失败计数，起得来又秒崩的按失败计数退避。每个 tick 重读 settings：
    `watchdog.enabled` 关掉就删 claim 退出（实例与记录不动），`max_restarts`、`idle_exit_secs` 下一轮生效；
    `heartbeat_secs`、`stale_after_cycles` 仍只在看护者启动时读。
  - **读路径只读**（`src/daemon.rs`）：`survey_instances_in` 不再删任何文件。清理是 `collect_garbage_in(run_dir, logs_dir)`，
    只在两处跑：实例开始服务之后（`spawn_blocking`）、看护者每个 tick。规则写在它的文档注释里，要点：实例记录只在读得懂
    且写者经「pid + 创建时间」核验已不在时删；`process_start == 0` 的记录（平台读不到创建时间）只有 `process_exited(pid)`
    明确为真才算死；交接文件与原子写临时文件在写者不在且早于 10 分钟时删；日志只删随机名形状、早于 10 分钟、没人引用的，
    run 目录里有读不懂的记录时这一轮不删日志（`logs_paused`）；unix 的心跳文件在该端口没有活实例且早于 10 分钟时删。
    期望状态记录不归清理管。
  - **心跳**：unix 从 `/dev/shm/aproxy-<home_id>-heart-<端口>` 挪到 `run/<端口>.heart`（`watchdog::heartbeat_location`），
    Windows 仍是命名共享内存节。实例里 ticker 还在 tokio 上（停摆 = runtime 无法调度，这正是挂死的定义），写入本身放进
    `spawn_blocking`。设计稿写的是「专用线程」，没照做：专用线程在 runtime 死锁时照样跳，挂死检测就失效了。
  - **启动顺序的竞态**（CI 的 Windows 桌面构建上两个用例红过）：期望状态记录原来写在控制端点开始应答之后，而拉起方把
    「端点应答」当就绪，紧跟着的 restart 会读不到记录。现在记录写在 `serve_ipc` 任务启动之前；这时没有 stop 能到达，
    不会出现「被要求停止后又写回记录」。台账条目的端口在配置端口为 0 时已用实际端口重写（第三部分）。
  - 验证：Windows headless 全量 668 通过、0 失败；两个变体 clippy 与 Linux 交叉 clippy 零输出。unix 的心跳路径只经
    Linux 交叉 clippy 与 CI 的 ubuntu 任务验证，本机跑不了。
  - 文档已跟上（6148ae4）：技能、README、architecture、CHANGELOG 都按上面的行为改过，子代理写、我抽查过 CHANGELOG 与
    README 的改动，引用的程序输出对过源码。
- B5 第五、第六部分（2026-11-17）：4dae49c spool 按进程分目录；5149a46 看护者记录与看护者日志。
  - **spool**：`spool/<pid>-<启动标记短哈希>/`（`daemon::spool_dir_name`，哈希是启动标记 SHA-256 的前 4 字节）。
    实例只在磁盘缓存开着时建目录（bind 之前），正常停止时收掉；别的进程留下的目录由 `collect_stale_spool_in` 清，
    在两处跑：实例开始服务之后、看护者每个 tick。判据是「pid 的启动标记算出的名字与目录名不同」或「读不到标记且
    确认 pid 上没有进程」；读不到标记又确认不了的保留。只删 `*.spooltmp`，目录空了才删目录。旧的按端口命名的目录不碰。
  - **看护者记录** `run/watchdog.json`（`watchdog::WatchdogRecord`，形状由单测 `record_shape_is_pinned` 钉着）：
    1. 在任 = 记录读得懂、属主进程还在（pid + 启动标记）、`last_scan_at` 新鲜。新鲜窗口按记录自己的 `heartbeat_secs`
       算三个周期，**且不短于 90 秒**（`FRESH_FLOOR_SECS`，设计稿没写）：一个 tick 里可能要等重拉出来的实例就绪
       （每个最多 30 秒），窗口更短的话忙着重拉的看护者会被当成假死、在重拉途中被结束。集成测试用
       `APROXY_WATCHDOG_SCAN_SECS=1` 驱动看护者，没有这个下限窗口只有 3 秒。
    2. 读不懂的记录（schema 更高、损坏、或另一个进程刚 `create_new` 还没写完）：不解读、不覆盖，按文件修改时间
       判断新鲜；在任者遇到它就让位退出。过期的读不懂的记录在接管时只删、不杀任何进程（不知道属主是谁）。
    3. 接管假死的前任：按记录里的 pid + 启动标记，用台账的 `ledger::terminate` 结束（不改台账那个冻结文件，
       只是把记录里的身份填成它要的 `LedgerEntry`）。删旧记录之前再读一次，只删刚才判为无效的那一份。
    4. `aproxy start` 与实例的自检不再替新看护者删无效记录：新看护者要靠它找到假死的前任。
    5. 实例启动时退役旧看护者、install 换二进制之前停看护者：都走 `terminate_watchdogs_in(home, older_than)`，按台账
       里 role 为 watchdog 的条目找。**不再认 0.1.0 的看护者**（它没有台账条目）：0.1.0 的实例与看护者要按 CHANGELOG
       的迁移步骤先停掉；实例记录改名为 `.instance.json`（B6）之后，残留的 0.1.0 看护者也看不到新实例。
    6. 重拉的起因 `kind`：被看护者按挂死结束的记 `hang`，其余记 `crash`；起因跟着退避队列的条目走。
  - **看护者日志** `logs/watchdog.log`（`append_watchdog_log`，英文，时间戳前缀同 startup.log）：启动、接管前任、
    结束挂死的实例、每次重拉的结果、转入慢速重试、摘除看护、退出。重拉失败的原因是 `RespawnError` 的文本，目前是
    中文——等 W3 的错误码与双语文案落地后换成英文。上限跟 `logs.rotate_mb`，达到时清空重写。
  - 验证：Windows headless 全量 675 通过、0 失败；两个变体 clippy 与 Linux 交叉 clippy 零输出。
  - **B5 还没做的**（接手时从这里继续，设计稿 §4、§5）：
    1. 实例记录仍是 `run/<端口>.pid`、旧形状（`process_start` 是数字）；要改成 `<端口>.instance.json`：带 `schema`、
       `start`（台账的启动标记字符串）、`instance_id`、`foreground`，`log_path`、`config_path` 可为 null；实例每 30 秒
       核对一次，记录丢了就补写。IPC 快照的 `instance` 是它去掉 `schema`——与 B6（IPC v1 的快照形状）连在一起做。
       `state:"starting"` 也在这一步。
    2. `tests/contract_v0_2.rs`（钉住全部记录的形状）没建：等实例记录定形后一次写齐，现在 spec、期望状态记录、
       看护者记录的形状各由所在模块的单测钉着。
    3. 广播之前的预检（`install_precheck_failed`）与「管不了的活实例」检查（`install_unmanaged_instances`）归 B9。
    4. 启动要 9 秒的 format 不被判为就绪超时：时限已是 30 秒，没有专门的测试（format 在启动时握手属于 B10a）。
    5. status 与 doctor 还不展示看护者记录（在任与否、最近的重拉）与「日志清理因读不懂的记录而暂停」：等 B9 的
       status 输出改版一起做。
- B6 第一部分（2026-11-17）：defb916 实例记录 `run/<端口>.instance.json`（`daemon::InstanceRecord`，形状由单测
  `instance_record_shape_is_pinned` 钉着；盘上是 `schema` 在前、其余与快照里的 `instance` 相同）。
  - 身份锚点从数字的进程创建时间换成台账的启动标记：`process::record_identity(pid, &str)`、`RecordIdentity::Alive`
    不再带值；按记录结束进程走 `process::terminate_marked_process`（内部是 `ledger::terminate`，会等进程退出、
    属主不同不结束）。数字创建时间的 `terminate_verified_process` 只剩两处用：install 里刚拉起、还没写记录的子进程。
  - 时刻统一为 Unix 毫秒：`started_at`、`activity.last_request_at`、`last_error.at`（`daemon::now_millis`）；
    `ops::list::Listing.now` 也是毫秒，闲置秒数除以 1000。
  - 实例每 30 秒核对一次自己的记录，丢了或内容不是自己的就补写；已在退出的不补写。
  - **alpha.1、alpha.2 启动的实例对本版本不可见**（快照缺 `start` 等字段，解析失败；记录名也不同）：`stop <端口>` 报
    「端口上没有运行中的 aProxy 实例」。升级说明写进了 CHANGELOG 与技能的 compatibility.md：升级前用旧版本 stop all；
    已换二进制的用 `install --force`。「`install --force` 能结束 alpha 实例」当时是按 C3 契约推出来的；
    2026-11-19 已由跨版本强制安装的测试实测（见文末那一条）。
  - 验证：Windows headless 全量 678 通过、0 失败；两个变体 clippy 与 Linux 交叉 clippy 零输出。
  - **B6 还没做的**（设计稿 §6.1–§6.4、§6.6，验收见 §14.2 的 B6 行）：
    1. 协议规则：未知参数回 `unsupported_arg`（现在静默忽略）、参数类型不对回 `bad_request`；错误对象带 `details`
       （`unsupported_version` 的 `supported` 现在在错误对象顶层，要挪进 `details`）；应答行上限 16 MiB。
    2. op 的参数：`shutdown.if_idle_secs`（不闲回 `condition_not_met`）、`prepare_swap.ttl_secs`（默认 600，到期回到
       `serving`）；`features` 列全。
    3. 快照：删掉 `run_dir`（先查谁在用）；`state` 加 `starting`；加 `behavior`、`formats`（熔断器字段等 B10）、
       `activity` 的 `in_flight`、`retrying`、`rescued_total`、`by_code`、`buckets`，`last_error` 带 `code` 与 `details`。
    4. §6.6 只读枚举 `enumerate(home)`：四种 liveness（`live`、`unresponsive`、`incompatible`、`down`）、`unreadable`、
       `gc_paused`、`watchdog`、`processes`。设计稿要它放在 aproxy-ipc（那里没有 tokio，客户端要写成同步的）；
       status 改用它之后，别的版本的实例显示为 `incompatible` 而不是「没有实例」，届时改 compatibility.md 那一节。
    5. 每个 op 的请求、成功、每种错误的 golden；`tests/contract_v0_2.rs` 钉住全部记录的形状。
- B6 第二部分（2026-11-17）：9981217 协议规则与 op 参数。
  - 未知参数回 `unsupported_arg`、类型或取值不对回 `bad_request`（`details.reason` 的词表：`incomplete_line`、`not_json`、
    `missing_v`、`missing_op`、`args_not_object`、`invalid_arg`；`invalid_arg` 时另带 `details.arg`——码表里补了这个
    可选键）。错误对象带 `details`，`message` 是英文。客户端的 `IpcError::Remote` 带 `details`。
  - `shutdown.if_idle_secs`：条件只在实例还没开始退出时判断（重复的 shutdown 结果相同）。`ops::stop::stop_if_idle` 给
    `stop idle` 用；不认这个参数的实例退回无条件停止。`--force` 没有条件可带。
  - `prepare_swap.ttl_secs`：`IpcStats.swap_until_ms` 存到期时刻，快照现算；0 不接受（一进入就过期）。安装器发的是不带
    参数的 `IpcOp::PREPARE_SWAP`（默认 600 秒）——安装的广播到滚动重启之间若超过 10 分钟，实例会回到 `serving`，
    续作重入时广播是幂等置位，会重新进入。
  - 快照：删了 `run_dir`（没人用），加了 `behavior`（枚举值按字符串传）与 `starting` 状态。**`starting` 只进了词表，
    实例还不会报它**：现在控制端点开始应答时实例已经能服务；等 B10a（format 在启动时握手）才有真实的启动阶段，
    届时就绪判定要改成「state 为 serving」。
  - golden：`request_lines_are_pinned_…`、`rejections_are_pinned`、`snapshot_shape_is_pinned`（都在 `src/daemon.rs`）。
  - 验证：Windows headless 全量 680 通过、0 失败；两个变体 clippy 与 Linux 交叉 clippy 零输出。
  - **B6 还没做的**：
    1. 快照的 `activity`：`in_flight`、`retrying`、`rescued_total`、`by_code`、`buckets`（30 个 120 秒的桶），`last_error`
       带 `code` 与 `details`。`formats` 与 `behavior.history` 分别等 B10 与 B8。
    2. §6.6 只读枚举 `enumerate(home)`（四种 liveness、`unreadable`、`gc_paused`、`watchdog`、`processes`）。
    3. `tests/contract_v0_2.rs`：把各模块里钉形状的单测归到一处（或在那里引用它们），验收条目要它。
- B6 第三部分（2026-11-17）：ca07ac8 快照的 `activity`。
  - 定下的口径（设计稿只给了样例，没给定义）：
    1. `in_flight`：从收到请求算到**响应体交付完**（或客户端断开、响应被丢弃）。守卫 `daemon::Counted` 存在响应体里
       （`proxy/handler.rs` 的 `hold_until_body_ends`，用 `http_body_util::BodyExt::map_frame`），保活、回放、仅转发三条
       路径一致。入站校验拒掉的请求不计。
    2. `retrying`：第二次尝试开始（含退避等待）到交付或放弃；交付回放期间不算。
    3. `rescued_total`：`Outcome::Deliver` 且之前重试过。
    4. `by_code`：失败的上游尝试记尝试失败码，aproxy 自己结束的请求记终态码——规则给的次数用完时两个都记
       （`upstream_status` 与 `retry_exhausted`），所以它的总和不等于失败的尝试数。
    5. `buckets`：30 个 120 秒的桶，旧的在前；`failures` 只数失败的上游尝试。读快照时桶也随时间前移。
    6. `last_error`：`code`、`details`、`message`（不再带码前缀）、`at`。
  - 没做的字段：`upgraded_total`（forward 升级为 retry，§10.10 属 B10）、`formats`（B10）、`behavior.history`（B8）。
  - 每个请求、每次重试、每次失败各锁一次 `IpcStats` 里的互斥锁（临界区只有整数运算）；没有压测过它对吞吐的影响，
    `docs/benchmark-*.md` 的数字是这之前测的。
  - 验证：Windows headless 全量 681 通过、0 失败；两个变体 clippy 与 Linux 交叉 clippy 零输出。
- B6 第四部分（2026-11-18）：§6.6 只读清点与 `tests/contract_v0_2.rs`，**B6 至此做完**。
  - `src/ops/enumerate.rs`：`enumerate_in(home) -> Enumeration`（可直接序列化，形状由单测
    `enumeration_shape_is_pinned` 钉着）。端口取 `run/` 里文件名是「数字 + `.instance.json` / `.restore.json`」的并集，
    所有端口同时探测（`daemon::ipc_probe_in`：单次 1 秒、`Transient` 再试一次）。
  - **与设计稿不同的地方**：
    1. 清点放在主 crate 的操作层，没有放进 aproxy-ipc。原因：IPC 客户端是 tokio 写的，aproxy-ipc 没有 tokio；
       把客户端搬过去或另写一份同步的都是大改，而 aproxy-desktop 现在根本不依赖主 crate，也不依赖 aproxy-ipc。
       界面要用它时（W5）经注入的操作层调用；第三方若要链接，届时再决定搬不搬。C3 的代码早已在 aproxy-ipc 里（B0）。
    2. 只剩一份「进程已死的实例记录」、没有期望状态记录的端口**不列出**（设计稿没说；它既不在运行也不会被恢复，
       记录等后台清理删）。
    3. `incompatible` 的 `reason`：实例回 `unsupported_version` 时原样带它的码与 `details.supported`；应答无法采信
       （不是 v1 的形状、自报 pid 与对端不符）或 ping 被别的码拒绝时用 `instance_incompatible`，`details` 带 `port`
       与英文的 `reason`。为此客户端的 `parse_reply` 先看 `v`：别的版本的应答里只认 `unsupported_version` 这一种。
    4. 看护者记录读不懂时 `watchdog` 给 null、并把文件列进 `unreadable`（设计稿的兜底是 `state: unknown`，但读不懂
       的记录给不出 pid、版本这些必填字段）。它不触发 `gc_paused`——与 `collect_garbage_in` 的口径一致，那里只有
       读不懂的实例记录与期望状态记录让日志清理暂停。
    5. `gc_paused` 是按同一条规则在清点时现算的，不是从清理那边读来的状态。
  - `spec::parse_record` / `read_record` 多了「本版本读得懂的最高 schema」参数，实例记录与看护者记录的读取共用它
    来区分「损坏」与「太新」；`daemon::read_instance_record` 是带原因的读法，`parse_instance_file` 仍是 Option。
  - `tests/contract_v0_2.rs`（4 个测试）：对真实例走**原始**的控制通道（不经 aproxy 的客户端代码）核对每个 op 的
    成功应答与十种拒绝、`condition_not_met`、`swap_prepared`、`stopping`；`base_url` 带 userinfo 时线上与盘上都已
    打码；没有配置文件的实例 `config_path` 为 null；盘上的实例记录、期望状态记录、看护者记录与清点结果比对样本
    （`assert_covers`：样本里的键不能少、类型不能变，多出来的不算错）；最后一个测试守着各模块里逐字钉形状的
    单测清单（`PINNED_IN_MODULES`）不被删或改名。
  - 四种 liveness 的区分：`tests/restart_integration.rs` 的 `enumeration_tells_the_four_kinds_of_instance_apart`
    （真实例 + 本测试进程顶替的挂死记录 + 在端点上回 v2 `unsupported_version` 的冒充者 + 只剩期望状态记录的端口）。
  - **没接线的**：`aproxy status` 还在用 `ops::list::enumerate`（`survey_instances`），不显示 incompatible、down、
    看护者与读不懂的记录——留给 B9 的 status 改版与 `--json` 一起做，免得 status 的文档改两遍。所以别的版本启动的
    实例在 `status` 里仍然看不见（只在清点结果的 `processes` 里），compatibility.md 那一节现在的说法仍然成立。
  - 验收表 B6 行里「模拟新旧客户端与服务端互连，多出的字段被忽略」：服务端忽略请求里多出的顶层字段
    （contract_v0_2 实测）、客户端忽略应答里多出的字段（`client_reads_v1_errors_and_unknown_states`）；与真的
    旧版本二进制互连没有测（alpha.1/alpha.2 的协议本来就不互通，见 B6 第一部分）。
  - 验证：Windows headless 全量 690 通过、0 失败；两个变体 clippy 与 Linux 交叉 clippy 零输出。Linux/macOS 上
    新测试的运行结果看推送后的 CI。
- B7（2026-11-18）：访问控制（设计稿 §1.6）。两个提交：f6b2425 + d35249d（端点），以及本条所在的提交（home）。
  - **端点**：
    - Windows：每个管道实例带 `D:P(A;;GA;;;<当前用户 SID>)(A;;GA;;;SY)`（`daemon.rs` 的 `imp::PipeSecurity`，经 tokio 的
      `create_with_security_attributes_raw`），拒绝远程客户端是 tokio 的默认值。建不出描述符时 `bind` 失败，实例不启动。
      读回来「完全控制」是 `FA`（单测 `control_pipe_admits_only_its_owner_and_system` 读两个管道实例的 DACL）。
    - unix：socket 0600（`bind_private`：bind 之后 chmod，重建的也一样）；接受循环里核对对端 uid，只接受本用户与 root，
      别的不读请求直接断开；客户端核对服务端 uid（`trusted_server_uid`），root 作客户端时信 **run 目录的属主**或 root。
      设计稿写的是「home 的属主」——run 目录由 home 的属主建，取它省得从端点路径倒推 home。
    - 跨用户实测：`tests/access_control.rs` 的 `control_socket_refuses_other_users_and_admits_root`（`#[ignore]`，CI 的
      ubuntu 两个变体里 `sudo useradd` 之后带 `APROXY_TEST_OTHER_USER` 运行；客户端是 python3）。CI run 44289316523
      两个变体都跑了并通过：0600 时别的用户 `connect` 得 PermissionError；把 socket 放开到 0666 之后别的用户连上即被
      断开、读不到应答；root 得到正常应答。
    - 没做：Windows 的心跳节与安装宣告节（`Local\aproxy-…`）仍是默认安全描述符——设计稿的「端点」只列了管道与
      socket，节里只有时间戳与安装宣告；macOS 上没有跨用户实测（CI 的 macos 任务本来就是红的）。
  - **home**（`src/fsx.rs`）：
    - `secure_home_in(home)`：不存在就建成只让属主访问（unix 0700；Windows 受保护的 DACL，当前用户、SYSTEM、
      Administrators 各自完全控制，子项继承），属于当前用户而过宽就收紧，不属于当前用户不动。
    - 触发点：`fsx::create_dir_all`（库里往 home 建目录的地方都换成了它）、原子写、台账登记之前
      （`util::register_in_ledger`）。每个进程对一个 home 只检查一次。只读的命令不触发。
      **与设计稿的出入**：设计稿说「install 与第一次写配置时」收紧；现在是「任何往 home 里写的动作」（启动实例、
      写记录、install、写配置），范围更大，原因是 home 也可能由这些动作第一次建出来。强制安装（C7/C8，冻结文件）
      没有改：它经 `handle_force_cmd` 的台账登记走到同一处检查。
    - `access_of(path)`：unix 看组与他人的权限位；Windows 看 DACL 里的允许项，当前用户、SYSTEM、Administrators、
      CREATOR OWNER / OWNER RIGHTS 与应用容器 SID（`S-1-15-…`）之外的都算「别人」。应用容器那一条是实测加的：
      本机的临时目录上就有两个能力 SID 与另一个账户的项。**没有处理拒绝项与继承标志的细节**（只读允许项的 SID）。
    - Windows 上含密钥的临时文件现在用 `CreateFileW` 在创建时就带 DACL（原来是建好再 `SetNamedSecurityInfo`）。
      R-S3-12 的这一条在 unix 上本来就成立（`mode(0o600)` + create_new）。
    - doctor：`check_home_access` 一条 WARN（文字，不带码——doctor 的 `--json` 与码 `file_permissions_too_open` 等 B9）。
    - 三个安装脚本新建 home 时设同样的权限（sh：`chmod 700`；ps1 与 cmd：`icacls /inheritance:r /grant:r`，账户用 SID）。
      两个 Windows 写法在临时目录里实测，读回都是 `D:PAI(A;OICI;FA;;;BA)(A;OICI;FA;;;SY)(A;OICI;FA;;;<用户>)`；
      **脚本整体没有重跑**（它们要从 GitHub 下载发布资产），sh 只过了 `bash -n`。
    - 测试 home（`tests/common`）起步就收紧：`tempfile::tempdir()` 在 unix 上按 umask 建目录（多半 0755，不是 0700——
      第一次推送时 doctor 的单测就错在这个假设上，CI run 44300790832 的 ubuntu 红，随后修正），在有的 Windows 机器上
      还给了别的账户权限，都会让 doctor 多一条告警。
  - 验收表 B7 行逐项：管道 DACL ✓（单测）；第二个用户连不上、root 连得上 ✓（CI 实测）；home 为 0700 ✓
    （`a_new_home_is_private_to_its_owner`，两个平台）；权限过宽的既有 home 在第一次写配置后被收紧 ✓
    （`a_loose_home_is_tightened_by_the_first_config_write`）；临时文件一创建就是最终权限 ✓（unix 单测
    `written_files_are_private_to_the_owner` 看的是成品，Windows 单测 `private_temp_file_is_restricted_from_the_moment_it_exists`
    看的是刚建出来、还没写内容的文件）；「`body` 只在这一块之后出现在 `ops` 里」——`body` 属于 B8，现在 `ops` 里没有它。
  - 验证：Windows headless 全量 695 通过、0 失败；两个变体 clippy 与 Linux 交叉 clippy 零输出。unix 上新加的 home
    测试看推送后的 CI。
- B8（2026-11-19）：请求历史、`body`、`subscribe`、事件（设计稿 §6.3、§6.5、§6.7）。提交 e6060a1（存储）、d185316
  （接进数据面与控制通道）、8cebecf（事件的 `at`），以及本条所在的提交（文档、skill、记忆）。
  - **做了什么**：`src/history.rs` 是存储与读取；数据面经 `Recorder` 与 `BodyWriter` 往里写（`proxy/handler.rs`、
    `attempts.rs`、`upstream.rs`、`forward_only.rs`、`body.rs`、`delivery.rs`）；控制通道多了 `requests`、`request`、
    `body`、`subscribe` 四个 op（`daemon.rs` 的 `ReadOp`、`read_reply`、`serve_events`，客户端 `ipc_read_in`、
    `ipc_subscribe_in`、`EventStream`）。实例快照的 `behavior.history.body_budget_mb` 与 `ops` 里的四个新名字随之出现。
  - **与设计稿的出入、以及设计稿没写而这里定下的**：
    1. 记录里的 `reports` 恒为空数组、`upgraded` 恒为 false：报告与 forward 升级为 retry 属于 B10。结局
       `format_response` 也要等 B10 才会出现。
    2. `bad_request` 的 `reason` 多了一个 `missing_arg`（读 op 缺必需的参数），码表与 error-codes.md 已有。
    3. 请求标识的前半段与快照的 `instance.instance_id` 原来是两个各自生成的值，现在是同一个
       （`AppState.instance_id`，`daemon::new_instance_id` 删了）——历史靠它从请求标识里认出序号。
    4. 仅转发模式不缓冲请求体，`model` 与 `stream` 只从请求体的前 64 KiB 里读；读不到就是 null / false。
    5. 体的记账是精确的，代价是拷贝：网络库交出来的块不直接留（理由与量法见记忆 request-history）。64 KiB 的块与
       1 MiB 的缓冲各有一张空闲表（各至多 8 MiB），所以历史占的内存是「预算加至多约 16 MiB」，不是设计稿字面上的
       「不超过预算」。文档与 skill 按这个口径写。
    6. `state` 事件是每秒比一次快照文本、变了才发（进入更换阶段与开始退出时立即发），不是每个计数变化都发。
    7. 事件的 `at`：`request` 事件取记录成为这个样子的时刻（限频按它算），别的事件取写出的时刻。所以 `at` 不保证
       逐行不减，次序以 `seq` 为准。设计稿没有定义 `at`，这里定了。
    8. 推送一侧把排着的事件合成一批一次写（至多 256 KiB 一批）。设计稿没提；不这样做，64 并发下订阅者九成事件
       变成 `lagged`。
    9. `model` 的读取挪进了读请求体的函数（溢写那一刻从内存里的开头一段读）；保活触发的判定没有动。
    10. **没接线的**：命令行与界面都还没有用这四个 op（B9、W5）。`aproxy status` 不显示历史。
  - **验收阈值逐项**（设计稿 §14 的 B8 清单）：
    - 吞吐与延迟：loadgen（2.1 MiB 请求体、约 300 KiB 响应）对 mock_upstream，64 并发，每轮 30 秒，基线是提交
      e433bee，本机 Windows，三者交替跑。没有订阅者，七轮：基线平均 348 rps、p99 348 ms，新版 334 rps（96.1%）、
      p99 358 ms（103%）——过线（≥95%、≤110%），余量很薄，单轮的比值在 92.7% 到 100.7% 之间（基线自己各轮
      335–359 rps）。一个订阅者，最终构建四轮：330 对 350 rps（94.4%），p99 368 对 352 ms（104%）——过线（≥90%、
      ≤120%），事件零丢失（每轮 21000 个 `request` 事件、`lagged` 为 0）。进程 CPU 时间多 2.8%–4.4%。
      头一版只有 92–94%；拉回来的几处改动与量法在记忆 request-history。这些数字是在 d185316 的构建上量的，
      8cebecf（只给事件多带一个时刻）之后没有重跑压测。
    - 长重试（退避 0、上游拒绝连接、8 并发、10 分钟，一个订阅者全程在线）：跑了两遍。第 1 分钟之后常驻内存
      最多涨 2.9 MiB（≤32 MiB）；最大的记录 5.3 KiB（≤64 KiB）。Windows 上连接被拒要等约 2 秒，所以这个用例里每个
      请求十分钟只有约三百次尝试。
      每条请求每秒的 `request` 事件：限频按实例产生事件的时刻算，相邻两个中间事件至少隔 1000 ms。头几遍事件的
      `at` 取的是写上连接的时刻，前后差着几毫秒的排队时间，量到的最小间隔是 1002、1000、994 ms（最后一个是 30 分钟
      那一遍，当时机器上同时在跑全量测试与压测），按一秒的滑动窗口偶尔数到 2 个。于是把 `request` 事件的 `at` 改成
      产生时刻（提交 8cebecf），订阅者按 `at` 数就严格是每秒至多 1 个：改完后同样的用例跑 5 分钟（同时在跑 clippy），1888 个事件，每条请求任何一秒里至多 1 个，最小间隔 1000 ms。
      集成测试 `subscribers_see_each_request_start_and_end` 断言同一件事。
      另跑了一遍 30 分钟的看有没有泄漏：私有内存从第 13 分钟到第 29 分钟停在 30.2–30.3 MiB，不涨。工作集不能用来
      看这个：同一遍里它被系统从 33 MiB 修剪到 22 MiB。（10 分钟的用例里基线的工作集约 17 MiB、新版 34–36 MiB，差的
      是 8 个 2.1 MiB 的请求体。）
    - 体的总量：`the_budget_holds_under_sustained_load`（64 MiB 预算推入 200 MiB，任何时刻不超、留下的是最新的 32 个）
      与 `bodies_are_evicted_oldest_first`。
    - 体整份保留：`a_large_body_round_trips_in_chunks`（10 MiB，带密钥替换）、`a_body_larger_than_the_budget_is_not_retained`；
      端到端 `a_request_body_spilled_to_disk_is_stored_whole`（3 MiB，真的溢写磁盘）。
    - 分块读取：单块上限 1 MiB（base64 之后约 1.4 MiB，远小于 16 MiB 的行上限）；进行中的体 `complete` 为 false
      （`a_large_body_round_trips_in_chunks`、`encoded_bodies_are_decoded_before_they_are_stored`）。
    - 解码与替换：`known_secrets_are_replaced_across_chunk_boundaries`、`encoded_bodies_are_decoded_before_they_are_stored`
      （gzip）、`brotli_bodies_are_decoded_too`、`undecodable_and_exploding_bodies_are_not_retained`；端到端
      `compressed_responses_are_stored_decoded`。
    - 边界：`read_rejections_are_pinned`（`limit` 为 0 或超过 1 MiB、`offset` 越界、不存在的尝试回 `no_such_part`）、
      `sent_bodies_are_shared_and_dropped_with_their_tries`（被省略的尝试的体回 `no_such_part`）、
      `a_large_body_round_trips_in_chunks`（`offset` 等于 `total_bytes` 回空块）、`oversized_records_are_truncated_to_the_limit`。
      「含 `actions` 很长的报告」那一半等 B10 有了报告再测。
    - 真实例上的原始通道：`tests/contract_v0_2.rs` 的 `request_history_is_read_over_the_control_channel`（三个读 op、
      各种拒绝、8 条订阅连接之后的 `busy`、线上没有密钥、退出时的 `closing`）。
  - 验证：d185316 上 Windows headless 全量 734 通过、0 失败、3 个忽略；8cebecf 之后重跑的是受影响的部分（lib 的
    history 与 daemon 54 个、history_integration 11 个、contract_v0_2 5 个，都过），没有再跑全量。两个变体 clippy 与
    Linux 交叉 clippy 在最终源码上零输出。
    Linux/macOS 上新测试的运行结果看推送后的 CI。压测脚本在本机临时目录里，没有进仓库；三个程序在 `examples/`。
- 跨版本强制安装（2026-11-19，TODO 的 W2b 剩余项）：`tests/force_cross_version.rs`（4 个测试，`#[ignore]`，要
  `APROXY_PREV_BINS`）与 CI 的 force-cross-version 任务（windows、ubuntu；下载 `PREV_VERSIONS` 里各版本的无界面资产并核对
  sha256）。对每个旧版本走四种组合：当前装当前（现场是旧的）、旧装当前（现场是旧的）、旧装旧（现场是当前的）、当前装旧
  （现场是当前的）。断言退出码是 0 或 12、强制安装之前台账里活着的实例与看护者全部结束、`bin/` 里是目标二进制；12 时输出
  里有端口、端口能重新绑上，0 时实例在台账里且版本是目标版本。
  - 本机 Windows 用 v0.2.0-alpha.1 与 v0.2.0-alpha.2 的发布资产实跑：8 种组合全过，退出码都是 12——实例都没回来，
    因为期望状态记录的格式在 alpha.2 之后换了（`.restore` → `.restore.json`），目标版本的 `restore` 读不懂另一个版本
    留下的记录。这在契约允许的范围内（记录格式不冻结），CHANGELOG 的升级说明写的也是「再逐个 `aproxy start`」。
    「回来了（0）」那一支这里走不到，同版本的由 `install_force.rs` 覆盖。
  - 当前构建的版本号还是 0.2.0-alpha.2，与发布的 alpha.2 相同；测试输出里用「当前构建」与「发布的」区分。
  - **发版时要做的**：每发布一个版本（预发布也算），把 tag 加进 ci.yml 的 `PREV_VERSIONS`。
  - CI：force-cross-version 在 windows 与 ubuntu 上都过了（run 的提交 e41e160）。同一次推送里 windows 与 ubuntu 的常规
    任务红了：新测试文件里取版本号的那一处直接 `Command::new(exe)`，没经 `TestHome`，被 `tests/test_home_check.rs`
    拦下——加这个文件之后没有重跑全量就推了。随后改成经 `TestHome::cmd_with` 运行。**新增或改动 tests/ 下的文件后，
    推送前要跑全量**（至少 `--test test_home_check`）。
- B9 的分块（2026-11-19 开工前定的做法；设计稿 §9、§2.4，验收见 §14.2 的 B9 行）。每块一个提交，做完一块在这里
  打勾并写下与设计稿的出入：
  - [x] B9-1 命令树（§9.1）：启动的覆盖参数与 `--config`、`--log-file` 从顶层挪进 `start`（`--base-url`、`--listen-addr`、
    `--api-key <KEY|->`）；裸 `aproxy` 打印帮助、退出 2；`upgrade` 改成 install 的 visible_alias；stop/restart 的 idle 阈值
    改成 `--idle-threshold-secs`（配在别的目标上退出 2）；`logs` 默认打最近 200 行就退出，`-n`、`-f`，说明文字写 stderr。
    测试 `tests/cli_tree.rs`。验证：headless 全量 742 过、三路 clippy 干净。**与设计稿的出入**：
    1. **`start --config <PATH>` 保留，而且是永久的**（§9.1 的 start 只有位置参数）。冻结的 `src/install/force.rs`
       在实例没能恢复时打印 `aproxy start --config "<文件>"`，已经发布的每个版本的强制安装都会打印这一行，所以以后的
       每个版本都要认它。它与位置参数互斥；`tests/cli_tree.rs` 的
       `start_takes_the_config_as_a_path_or_with_the_config_flag` 钉住。顶层的 `aproxy --config X start` 不再接受。
    2. `install --skills-only` 没删：设计稿把它的删除划给 W4（`aproxy skill`），在那之前删掉会让技能没有单独安装的办法。
    3. `config` 这一步只把 `--config` 改成了自己的参数（可以写别名），15 个专用旗标留给 B9-2。
    4. 文档、技能、安装脚本与提示文字里的旧写法（`aproxy config --baseurl …` 等，约 75 处）还没改：等 B9-2 把 config
       的写法定下来一起扫，免得同一处改两遍。**在那之前 README 与技能里的命令行与实际不符**，B9-2 之后必须接着做。
  - [x] B9-2 `config show|get|set|unset`（§9.3）：专用旗标一个不留；`--config <ALIAS|PATH>` 是 config 自己的参数
    （clap 的 global，写在子命令前后都行）；四个子命令都有 `--json`。读出口在新模块 `src/config/view.rs`：文件值与
    生效值的 JSON、打码（settings 的 `defaults` 共用同一份 `redact_table`）、占位写回（`restore_placeholders`，
    `config::edit` 在 `set` 时调用）、键表 `key_kind`、JSON 转 TOML 文本。测试 `tests/config_cli.rs` 与 view 的单测。
    文档、技能、安装脚本、CHANGELOG 里 B9-1 与 B9-2 的旧写法一并扫过。**与设计稿的出入、留给后面的**：
    1. `get` 给的是**生效值**与来源（`source`），文件不存在时退出 4；文件只有别处的错误时照样给值、退出 0，只有语法
       错误（一个值都读不出）才退出 3。设计稿 §9.5 的「文件无效 → 3」按这个口径实现。
    2. `show` 不带 `--effective` 时只列文件里写了的键，但 `diagnostics` 两种显示都是整份解析的诊断（这份配置能不能
       启动），所以「全局默认的 key 没继承」的告警在不带 `--effective` 时也在。
    3. `set`/`unset` 的 `--json` 比设计稿多一个 `changed`（加性）；没变化时 `instances_to_restart` 为空。
    4. 文本输出里的密钥统一成 `<secret …>`（§1.5），`show --effective` 原来的「前 6 个字符加 ***」没有了，
       `util::mask_secret` 删了。format 的 `args` 里写成 `--flag=值` 的密钥，JSON 里是字符串 `--flag=<secret>`，
       写回时按「同一旗标第几次出现」对回原值。`args` 里别的参数 JSON 里原样（设计稿只点了密钥旗标之后的值），
       给人看的文本另过一遍 URL 打码。
    5. `settings set default_config <路径>` 把原文按当前目录绝对化（原来 `config --set-default` 做的事）。
    6. **留给 B9-3**：写配置时的 I/O 失败（`EditError::Io`）码表里没有对应的操作码，现在只有 stderr 与退出码 1，
       `--json` 时 stdout 上没有错误文档；写锁等不到（`LockBusy`）暂用 `write_conflict`。
    7. **留给 B9-4**：`settings show|get|set|unset` 的 `--json`（设计稿 `set`/`unset` 的 `instances_to_restart` 对
       settings 也适用：改了 `defaults` 时是哪些实例）；`--json` 文档的 golden。`config.rs` 里的 `fail`/`print_doc`
       到时挪成各命令共用的。
  - [x] B9-3 退出码表（§9.5）与操作码（§2.4）：start、stop、restart、restore、logs、config、settings 七行。共用的出口在
    `src/commands/mod.rs`：`Failure`（码、message、details；退出码查码表；`--json` 时 stdout 上是错误文档）、`Results`
    （stop/restart/restore 逐实例的结局与汇总，`{"results":[…],"summary":{"ok","failed"}}`）、`print_doc`、`say`
    （`--json` 时给人看的行改写 stderr）、`port_value`、`spawn_failure`、`bind_failure`。配置解析的失败有了类别
    （`ops::start::ConfigError`），restart 预检带着它（`PrecheckError::Config(ConfigError)`）。start、stop、restart、
    restore 有了 `--json`（这四个的文档随这一步定下，B9-4 不再做）。测试 `tests/exit_codes.rs`（每格至少一个断言）；
    技能 commands.md 的退出码表与各命令的相关段落、CHANGELOG 随之改写。验证：headless 全量 776 过（0 失败、7 忽略），
    三路 clippy 干净。
    **与设计稿的出入、留给后面的**：
    1. stop 一个没在运行的目标（端口或别名）是 `already_stopped`、退出 0（原来是 1），按 §2.4「端点不存在」那一行。
    2. 选目标时因不应答而跳过的实例（不带 `--force` 的 all 与省略目标）现在逐个记为 `failed`（`instance_unresponsive`），
       所以 `stop all` 遇到挂死的实例以 5 结束。`ops::target::Selection::unresponsive_skipped` 从计数改成记录本身。
    3. restart 省略目标而没有运行中的实例：`target_not_running`（4），`details.target` 为 null（码表写的是 String；
       null 与结果项里「没写目标」的约定一致）。`restart all` 没有实例时是 0、空结果。
    4. restart、restore 结果项的 `port` 是新实例实际监听的端口（配置改了端口时与旧的不同）。
    5. restore 的 `config_missing` 不计入 `summary` 的 ok 与 failed，命令以 5 结束；读不懂的记录只在 stderr 上报，
       不进结果。
    6. logs（没有 `--json`）：目标没在运行、但有期望状态记录时，读记录里的 `log_path`（§9.6），`-f` 时不跟随；省略
       目标而没有运行中的实例时，只有一条记录就读它，几条时 `target_ambiguous` 列出记录的端口；`logs all` 是 `usage`（2）。
    7. `details.reason` 的取值：`stop_failed` 是 `still_running`、`kill_failed`；`usage` 是
       `idle_threshold_without_idle`、`logs_target_all`。restart 预检：记录读不懂是 `restart_unsupported`，配置文件
       不在是 `target_not_found`（4），配置无效 `config_invalid`（3），端口被占 `port_in_use`，别的绑定失败 `start_failed`。
    8. **没做的**：`start_failed` 的 `details.code`（子进程的退出码）没填，`__serve` 遇到配置错误以 1 退出，所以
       restore 时配置改坏是 `start_failed`/1 而不是 3（start 自己在拉起之前就解析配置，报 3）；`instance_incompatible`
       还没有产生它的路径（随 B9-4 status 改用 `ops::enumerate`）；`EditError::Io` 仍没有操作码；status、doctor、
       find、install、skill、desktop 几行随 B9-4、B9-5、B9-6、W4。
  - [x] B9-4 `--json`（§9.4）：status（改用 `ops::enumerate`，顺带显示 incompatible、down、看护者、读不懂的记录）、
    start、stop、restart、restore、alias list、config/settings 的 show 与 get、find；每个一份 golden。
    进度：start、stop、restart、restore 随 B9-3；alias list 与 find 在 d7da787（`tests/json_docs.rs` 整份文档比对，
    find 的 `errors` 按启动的分层管线解析，含 settings 的全局默认），技能与 CHANGELOG 在 f507fca；settings 的
    show/get/set/unset 在 7e7f765 与 50ff99f（改了 `defaults` 时 `instances_to_restart` 列出全部运行中的实例，别的
    键是空表；按解析后的键路径判断，加引号的写法也算）。status 在 40d584a（Codex t03）：改用 `ops::enumerate`，`--json` 输出
    §6.6 的枚举结果；文字输出按存活状态分块（无响应、运行中、无法采信应答、等待恢复、读不懂的记录、日志清理暂停、看护者、
    台账里的其他进程）；`--idle`/`--busy` 只筛选运行中的实例；删掉 `ops::list`。`main.rs` 里 `status --json` 暂时跳过
    `init_foreground_logging`，B9-7 统一换成 stderr 日志时去掉这个特例。技能与 CHANGELOG 随后补上。
    验证：headless 全量 783 过，headless 与 desktop 两种 clippy 干净（Linux 交叉未跑：这一步没有平台相关代码）。
    偏离：设计稿要每个命令一份 golden；status 的文档里有 pid、时刻等每次不同的值，测试改为断言结构与关键字段。
  - [x] B9-5 doctor：换成诊断类型，`--json`，码（`file_permissions_too_open`、`default_header_shared` 等）。
    188de2e、3d9956d（Codex t04）。doctor 按文件给出诊断，顺序是 settings.json、各配置文件（去重）、home 的权限、
    读不懂的记录；配置走 start 的分层管线，同端口的配置互相点名，不认识的键给出删除它的 `unset` 命令；文字与
    `--json` 都只在有 error 时退出 3；`gc_paused` 与 status 出自同一次只读枚举。码表在 §2.4 之外加了六个 warning 码
    （`listen_not_loopback`、`port_shared`、`config_dir_missing`、`config_file_unreadable`、
    `watchdog_slow_hang_detection`、`watchdog_restarts_disabled`），调用方不必解析文字。`watchdog.stale_after_cycles = 0`
    原来被看护者静默按 1 处理，现在是 settings 的错误；心跳周期过长与 `max_restarts = 0` 合法但会推迟或关掉自愈，
    仍是 warning。验证（Windows、headless）：aproxy-ipc 30、code_table 1、`--lib doctor` 9、`--lib settings` 39、
    `--lib find` 3、json_docs 9、exit_codes 的 doctor 1、access_control 2。§9.4 的文档与 §9.5 的退出码不变。
    forward 与 format 同用的告警要等 B10d 放开这个组合；面向用户的文档归 B9-8。
  - [x] B9-6 install：`--check`、`--json`（NDJSON）、预检（`install_precheck_failed`、`install_unmanaged_instances`）、
    A5 的四个字段、`halted` 时不拉起续作、与 `--force` 冲突的旗标进 force_contract 的命令行样本。
    ef82416、5337bda、53eeac9、a3602b6、9c0ed52、d928fd7（Codex t05），帮助与提示文字 41217c3。
    - 失败记录：install.state 加 `failed_at`、`installer_version`、`error_code`、`error_details`（A5），以及滚动重启里
      每个端口的结局 `instance_outcomes`（`restarted`、`rolled_back`、`failed`、`skipped`），都是可选字段。退出码按失败的
      码查表；码表加 `install_restart_failed`（带 `port`）与兜底的 `install_failed`。
    - 动手之前的两道检查：普通安装在加锁、下载、写状态之前，查管不了的实例（台账里活着的 instance，run/ 里却没有本版本
      读得懂的实例记录）与前台实例；下载之后、广播之前（含续作与交棒后的接手者）再查一次，并用目标版本按当前的
      settings.json 预检每个有期望状态记录的快照端口。查到或预检不过就置 failed 与 halted，二进制与实例都不动。
      前台实例一条是 2026-11-19 定的（原来要等换完 bin 才失败，见设计稿 §5 的 install），码表加
      `install_foreground_instances`（带 `ports`）。预检用两个 error 级诊断码 `install_restore_unreadable`、
      `install_config_unreadable`：`record_unreadable` 不是诊断码，`config_file_unreadable` 是 warning、说的是另一件事。
      安装者自己的台账登记（C3）在入口检查之前，不算改动（设计稿 §4.3 第 7 条）。
    - 安装期间被停掉的端口（既没有活实例，也没有期望状态记录）跳过，记 `skipped`，终验不再要求它；崩溃了、记录还在的照旧拉起。
    - 续作只由变更类命令拉起（start、stop、restart、restore、alias add/remove、config set/unset、settings set/unset，
      main.rs 里穷举匹配）；halted 的 failed 现场谁都不拉起。
    - `--check` 与 `install latest` 同一套查询与通道规则，不登记台账、不建状态；`--json` 的文档在答复来自 GitHub 时带
      `published_at` 与 `url`。`--json`：plan、phase、progress、instance、result 五种事件；交棒之后接手者记下每个相位的
      首个时刻，并把最终状态原子改名留给等它的那个进程读，所以快速完成的接手者也不漏报相位与实例结局；更早的接手者没有
      这些字段，只能报轮询时看到的相位。目标版本还没定下就失败时 `result.version` 为 null。
    - 与 `--force` 冲突的 `--json`、`--check` 在 cli_tree 里测，不改冻结的 force_contract。
    验证（Windows）：fmt、headless 与 desktop 两种 clippy、Linux 交叉 clippy 干净；headless 全量 809 过、0 败、7 忽略，
    其中 aproxy-ipc 30、`--lib install` 60、code_table 1、install_cli 13、install_flow 13、install_flow_lib 16、
    force_contract 18（一个子进程辅助用例忽略）、cli_tree 10、exit_codes 22、json_docs 9。新工作树里跑 desktop clippy
    之前，先在 `aproxy-desktop/ui` 里 `npm ci` 与 `npm run build`：Rust 构建要嵌入前端产物，缺 `dist/index.html` 就在
    lint 之前停下。
    没测的：设计稿 B9 验收里「之后执行 alpha.1 的 `aproxy stop all`，它拉起的续作不接管这个现场」要 alpha.1 的二进制。
  - [x] B9-7 输出纪律（§9.6）：CLI 的 tracing 写 stderr、默认 WARN；遍历命令树的帮助测试；`RUST_LOG=info` 时 stdout
    没有日志行。
    6c254f2（Codex t04）。CLI 的 tracing 写 stderr（`init_cli_logging`），默认 WARN，`RUST_LOG` 可调；stderr 被捕获时
    不带颜色。前台实例照旧把日志写在控制台；分离拉起的看护者、`__serve`、桌面与强制安装（C7，写失败不改退出码）
    各自初始化日志，不变。去掉了 t03 在 main.rs 里给 `status --json` 开的特例。帮助遍历覆盖根命令与 23 个可见的
    命令路径；`RUST_LOG=info` 时七个 `--json` 命令的 stdout 都只有一行带 schema 1 的文档，被扣下的凭据那条 INFO
    出现在 stderr、默认级别时不出现。restart_integration 的并发看护者测试原来从 stdout 读 INFO，改为开 INFO 读 stderr。
    验证（Windows、headless）：fmt、clippy 干净；cli_tree 9、test_home_check 2、exit_codes 22、proxy_integration 117、
    restart_integration 23。帮助末尾的 skill 提示归 W4（§9.6）。
  - [x] B9-8 文档：技能的 commands.md、compatibility.md（0.2.0 之内的命令行变化）、README、CHANGELOG、安装脚本。
    87a09a4（status）、b70e482（doctor、日志改走 stderr、`stale_after_cycles = 0`；commands.md 的退出码表原来有五行落在
    表外，一并修好）、d752047（install 的 `--check`、`--json`、两道检查与预检、续作的触发范围、退出码 3 与 4；
    compatibility.md 补上 0.2.0 的命令行对照表，0.1.x 与前两个 alpha 适用）、0bd9f5b（三个安装脚本里 skill 下载失败的
    重试提示改为 `--skills-only`：已是最新时 `aproxy install` 直接返回，不重下 skill）。
- B10a（2026-11-20，Codex 实现；master 上是 format 协议 1 的类型与运行时那个提交起的 18 个提交）：aproxy 改说 format 协议 1。
  - **启动握手**：对每个配置的程序并行握手。静态契约不符（缺阶段、协议不符、回信不合法）时可选程序也让实例不启动；
    只是不可用（起不来、10 秒没回、中途退出）的可选程序以熔断 `open` 起步，冷却后经数据通路的试探恢复。握手期间实例
    报 `starting`：再执行一次 `start` 要等它出结果，`prepare_swap` 回 `invalid_state`；安装器照旧要每个实例都 ACK 才换。
  - **链**：request、retry 按配置顺序跑，从失败的那一层接着跑，前段的输出在各次上游重试之间沿用；response、heartbeat
    按逆序。进程级失败、程序自己回的 `fail`、被取消的调用，对熔断的影响各不相同。只用来展示的字段（报告、动作）为空或
    不合法，不让合法的补丁作废。程序自己给的终态响应原样交给客户端、不做密钥替换；请求历史只记终态码与交付的字节数，
    不编造一份上游响应。快照列出每个程序的握手结果与熔断器。
  - **worker**：每个 worker 有自己的 `seq`，编号保留到进程被回收。回信的长度在读管道时就设上限，不是读完再查。排队中的
    消息只拿着溢写文件的句柄，轮到它才读体。头表的替换先归一再交给下一层；程序自己写的 `content-encoding` 描述的是它
    给的字节。响应要转换之前先撤销还没回来的心跳，心跳任务对响应通道只持弱引用：只有一个 worker 的程序也不会拖住响应
    或 EOF。正常停机时，健康的 worker 并行地各得 2 秒读到 EOF 退出。
  - **还没做的**（归后面各块）：配置仍拒绝 decide、stream、notify，以及 forward 与 `[[format]]` 同配；这些阶段的类型已经
    有了，但不声明、不派发。请求体与摘要的协商、forward 升级、notify、官方 aproxy-format 迁到协议 1（它现在从 `legacy`
    模块引用旧信封）都在后面。请求历史的 schema 没有为程序给的体而改。64 KiB 的记录上限里，HTTP 方法名同另外三个客户端
    串一样只在超出时截短，发给上游的方法不变。
  - **设计稿的改动**：这一块的实现与五轮审查引出了设计稿 §6、§7.4、§10 的多处补充（2026-11-19、11-20 的标注），
    以设计稿为准。
  - **验证**（Codex 在 Windows 上跑的，84a030b）：`cargo fmt --all` 无改动；headless 全量 881 通过、0 失败、7 忽略；
    headless、desktop 与 Linux 交叉三种 clippy 零输出。契约测试的夹具与冻结的强制安装实现没有改动。
- B10b（2026-11-20，Codex 实现；提交「format 的 decide 阶段」及其前后的修复）：decide 阶段。
  - **表态**：retry 投递的请求在每次尝试失败之后（声明了 `decide.success` 的程序在成功之后也）按配置顺序问 decide。
    第一个明确表态的程序说了算；都不表态就照重试策略。等待多久与要不要重跑重试段（`reroute`）可以各自表态。对成功的尝试
    回 stop：不带响应是认可这次成功，照常交付；带响应是用它取代，不再经过 response 阶段。这次尝试的记录里，表态与报告
    同上游的状态码分开记。交给 decide 的响应体是有上限的预览（开头 64 KiB 加结尾，共 1 MiB），但原样透传或在它上面打补丁时，
    交付的仍是完整的上游响应。forward 投递的请求这一块还不问 decide（归 B10d）。
  - **由实现方定、主代理认可的几处**：本地的溢写或大小上限失败（`request_too_large`、`response_too_large` 等）是终局，
    不问 decide；嵌套的 content-encoding 最多解 4 层，再多就原样交出、头表不动；动作里的字符串在 256 个字符处截断时，
    截断处若可能是半个已知密钥也一并遮掉；第二个 `start` 观察同一次启动时，读的是一份只在失败时写下、只含脱敏后的握手诊断的
    临时结果（按启动时刻与 pid 命名，过了保护期由清理回收），没有为此新增 IPC 操作。
  - **第二轮审查引出的修复**（设计稿 2026-11-20 的各处补充）：先取得 worker 再解码请求体；只有阶段消息的合法回信清零熔断
    计数；`breaker_failures` 为 0 时，握手时不可用的可选程序以 `closed` 起步；半开的试探被取消时让出试探、不清零；已写出的
    心跳逾期计入熔断，响应就绪而作废的不计；熔断的冷却用单调时钟；熔断状态的每次变化依序发给订阅者；停机时等已借出的
    worker 退出；头表容量、UTF-8 与报告的上限。
  - **CI 的那次失败**（5739219 的 windows (desktop)，`watchdog_respawns_on_death_event_and_force_stop_stays_stopped`）：
    B10a 之后实例在登记之前就以 `starting` 应答 ping，这个测试原来靠「ping 通再等 2 秒」当作已被看护者收养，于是偶尔在收养
    之前就杀掉了实例。改了测试：等到 `serving` 并确认已被收养再模拟崩溃，失败时打印最后一次 ping 与两边的 stderr。
    产品代码没动。本地改之前两种变体各 20 次都没有复现。macOS 上的存活判断改为信号 0，没有在 macOS 上运行过。
  - **验证**（Codex 在 Windows 上跑的，4cc4994）：`cargo fmt --all` 无改动；headless 全量 925 通过、0 失败、7 忽略；
    headless、desktop 与 Linux 交叉三种 clippy 零输出。
- B10c（2026-11-20，Codex 实现；提交「format 的 stream 阶段」「forward 与 [[format]] 可以同配」及其前后的修复）：stream 阶段，
  以及 forward 请求过链。
  - **stream**：一条流从 open 到 close 或 abort 固定由一个 persistent worker 处理。forward 请求的响应边收边交给 stream 程序；
    response 程序只在它自己那一层攒齐。没有 request 阶段的程序要跑时，forward 的请求体直接流向上游。retry 请求先对上游的
    原始响应做判定，再开回放的流。required 程序在流中途失败，向外侧各层发 abort；可选程序失败，保留它当前的输入，之后的块
    跳过这一层。块尾不完整的 UTF-8 留到下一块；解码与交接有界、有背压。
  - **占用与断开**：流占的是一次对话，不占池子的名额（否则 `pool_max = 1` 加一条长流就挡住所有请求）：流拿走空闲 worker 时
    把名额还回去，但子进程的句柄与 worker 编号留到操作系统确认它退出。客户端断开时，abort 发给同一个 worker，不另起；
    有一帧正在途中就先在限时内等它的回信，不把 abort 写进那一帧中间；收不完的 worker 丢弃。abort 的 `reason.code`
    在客户端断开时是 `client_gone`。
  - **停机**：一开始停机就退役空闲的 worker；排空期限一到，杀掉还占着消息的，最多等 1 秒确认，确认不了的逐个告警（format
    名与 pid）。format 的 worker 不进进程台账（C3 不为它扩充），实例退出之后没有东西再去找它，这是有意留下的缺口。
  - **第二段审查引出的修复**：成功响应的保活头等 decide 表态之后再交付；decide 的终态先解码再写进 SSE，单个 JSON 事件限
    1 MiB；解不了的上游体原样（base64）交给 decide；`elapsed_ms` 从收到请求算起；断开时保留这次尝试已知的事实与实际等待的
    时长；记录里区分明确的 `reroute` 表态；上游失败的诊断与请求记录的路径里遮住已知密钥；订阅者跟不上时报出全部漏掉的
    条数、再给一份当前状态，不补发；可选程序恢复之后按实际握手决定要不要解码请求体。
  - **还没做的**：forward 请求仍不升级、不对投递方式表态、不发 notify（归 B10d）。
  - **CI**：第二段合入后 windows (desktop) 红在 `observing_start_preserves_a_static_handshake_failure`（靠固定 sleep 当同步），
    已改成等第二个 start 进入就绪等待；同类的几个测试一并改了（并发启动、流式上传、Unix 管道残帧）。
  - **验证**（Codex 在 Windows 上跑的，17cfe73）：`cargo fmt --all` 无改动；headless 全量 964 通过、0 失败、7 忽略；
    headless、desktop 与 Linux 交叉三种 clippy 零输出。临时 home 里用 CLI 实测：带一个不回信的 worker 时 `stop` 在等待期限内完成。
- B10d（2026-11-20，Codex 实现；提交「request_body 按阶段给正文、摘要或不给」「按请求选投递方式」「format 的完成通知 notify」
  「doctor 提示默认 forward 时的两种配置风险」及其前后的修复）：按请求选投递方式、摘要、notify。
  - **投递方式**：request 阶段的程序对 `delivery` 表态，第一个表态的生效，直到响应头写给客户端为止；保活已经提交了 200 之后才到的
    `forward` 被忽略。forward 请求在一个字节都没写出时失败，可以经 decide 升级成 retry：原来那次尝试留在记录里，快照里计一次升级。
    forward 的成功响应原样流过，旁路检查「200 但体是错误」，检查的上限与历史留体的预算无关。
  - **请求体的粒度**：按实际所在的阶段协商。摘要是客户端原始请求体的 SHA-256，不是解码或改写之后的。只有 notify 要摘要时，
    不为它缓冲正在上传的体。上传与响应可以同时进行，所以后面的阶段里 `client.stream` 仍可能不知道，不知道就省略。
  - **notify**：请求进入链之后，完成通知要等「交付给客户端」与「上游字节数入账」两件事都结束才发，但从不等旁观程序的读写。
    只声明 notify 的程序坏了之后不靠数据面的调用也能恢复；它的队列、空闲 worker 的准入与熔断，都与请求的失败计数无关。
  - **第三段审查引出的修复**：流的收尾区分「没回信的帧」与「已同步的拒绝」；已提交的压缩回放先验完整个编码再给出明文；
    终态 JSON 限解码后 1 MiB；订阅者跟不上、又赶上关闭时，给一份当前状态，不补发过时的变化；decide 的 `elapsed_ms` 逐层取；
    回放的流用逻辑尝试序号；forward 只有 2xx 过响应方向的链；启动中被叫停也限时确认 worker 退出。
  - **测试**：靠墙上时钟的测试改成看真实的帧、套接字或 EOF 边界，心跳与半开探测用隔离的手动时钟推进（时钟只换代理一侧的，
    客户端与防挂死的期限仍走真时间，否则会把卡住的交互藏起来）。
  - **三个偶发失败的旧测试**：`install_cli` 的 `Text file busy` 与 `install_flow_lib` 的改名共享冲突，都只对观察到的那一种系统错误
    短暂重试。`cli_logs_stay_off_stdout` 那次（`status --json` 一行没输出）的原因没有查明：CI 没留下退出码与 stderr；
    补了两条 JSON 回归（stderr 被关掉、home 不合法），断言失败时现在会带上退出码与 stderr。
  - **吞吐没有过线**（本机，64 并发、每轮 30 秒、三轮交替；不是服务器上的严格测量）：对照 275.6 请求/秒、p99 471.9 ms；
    挂一个从不读 stdin 的 notify 程序时吞吐是对照的 94.3%、p99 是 110.4%（门槛 95% 与 110%，都差一点）；§7.2 形状的
    只要摘要的 request、decide、notify 链是 90.9% 与 102.6%（吞吐没过）。对照自己在三轮之间从 226 变到 346 请求/秒，
    这台机器上的噪声比差距大，结论要等服务器上的重测（[性能计划](perf-0.2.0.md)）。内存与首字节过线：200 MiB 的 forward
    下载、留体预算为 0 时私有内存涨 3.4 MiB（上限 32）；上游每 100 ms 一块时，转换后的首块在 0.6 到 0.8 ms 内到客户端。
  - **验证**（Codex 在 Windows 上跑的，bdb1f19）：`cargo fmt --all` 无改动；headless 全量 1009 通过、0 失败、9 忽略；
    headless、desktop 与 Linux 交叉三种 clippy 零输出。
- B11（2026-11-20，Codex 实现；提交「retry 阶段的输入带上一次尝试的上游响应头」「官方 aproxy-format 改说协议 1，
  每个角色一个子命令」「aproxy-envelope 删去旧信封」）：官方 aproxy-format 迁到协议 1。
  - **按角色拆开**：`aproxy-format <角色>` 一个子命令一个角色（convert、keys、route、policy、adaptive、rewrite、heartbeat），
    没有大一统的 `run`，也没有旧信封了；原来不走信封的一次性转换改名 `translate`。这一段搬过来的是已有的能力：
    请求与非流式响应的协议转换、key 轮换、按模型路由；policy、adaptive、rewrite、heartbeat 只有握手与最小行为，
    流式转换、冷却、决策与工具子命令在之后几段。各角色的设计在 [format-roles](format-roles.md)。
  - **状态放哪里**：同一个请求的 request 与 response 可能落在不同的 worker 上，所以 convert 把认出的客户端协议、keys 把
    选中的 key 的 ID 放进协议的 `state`，不放进程里按请求编号的表，也不放密钥本身。keys 按解析后的 origin 选池，不按主机名前缀。
    route 换上游时连同认证一起换；配了别名的 route 可以多订一个缓冲的 response，把模型名换回客户端用的那个别名。
  - **`retry.last_response`**（§10.4）：重跑重试链时，带上一次尝试的上游状态与头（体中途断了也带）；连不上或在链里失败时没有；
    心跳不带。不为了拼这个头去读请求体。两条由此而来的限制写进了角色设计：decide 表态 `reroute:false` 时 keys 不被调用，
    记不到那次失败；冷却只在一个 worker 里，所以 keys 配 `pool_max = 1`。
  - **给谁看用什么语言**：命令行与 stderr 的诊断用中文；写进协议的 `fail.reason` 与 `report.summary` 用英文。
  - **实测**（用真实的 aproxy-format 二进制，本机上游）：§7.2 的链把 Anthropic 请求转成 Chat、带上凭据发给上游、
    把响应转回 Anthropic；adaptive 的管道上只有摘要、没有正文；可选的 adaptive 不可用时普通的重试还在；
    七个角色各自单独握手，只声明自己要的阶段与请求体粒度。跨协议的 SSE 与各角色的进阶行为不在这次的结果里。
  - **验证**（Codex 在 Windows 上跑的，c0b91c7）：`cargo fmt --all` 无改动；headless 全量 985 通过、0 失败、9 忽略
    （比上一段少，是旧信封的测试随旧信封删了）；headless、desktop 与 Linux 交叉三种 clippy 零输出。
- W2c 第一部分（2026-11-20，Codex 实现；提交「aproxy-format convert：三种协议之间的请求、响应与流式实时转换」
  「aproxy-format heartbeat：按客户端协议生成心跳事件」，以及第四、第五段审查引出的三十来个修复）：convert 与 heartbeat。
  - **convert**：Messages、Chat、Responses 六个方向都能转，包括增量事件、工具参数、结束原因、用量与中止时的收尾。
    完整的事件立即发出，没收完的事件最多留 8 MiB。
  - **heartbeat**：Messages 发原生的 `ping`。Chat 与 Responses 原样留下收到的 SSE 注释，不造事件：在上游开始之前造一个事件，
    Chat 的 SDK 会把它的 ID 当成这次补全的 ID，Responses 的 SDK 会因为第一个事件不是 `response.created` 报错。
    所以对这两种协议，心跳只是传输层的保活。
  - **SDK 核对**：用官方 Python SDK 的流解析器离线跑了 12 组（每个方向带与不带心跳），核对文本、工具、结束与用量；不调真实的 API。
    跑法记在 [format-roles](format-roles.md) 的验证一节。
  - **第五段审查引出的修复**：换去向时按统一的「密钥头」规则去掉凭据；keys 与 route 的凭据头名必须是密钥头；查询串跨 origin 时
    只带显式列出的参数；convert 转不了成功的响应时回一个新生成的 502，不带上游的头与字节；非标准路径上两种协议都说得通的请求被拒绝；
    官方角色只回应握手里声明的阶段；读帧有上限、必须以换行结束；重跑重试链时上一轮给的覆盖作废（aproxy 一侧）；
    `retry.last_response` 的头保持上游发来的样子，不借用 decide 解码之后的那份。
  - **第四段审查引出的修复**：起始为 retry 的请求总是先读完请求体，慢的 request 程序面前保活照常；decide 升级之后投递方式锁定；
    forward 的响应在写出之前断了，交回 decide；直流的上传分块判断 `stream`，不占共享的阻塞线程，解析用的内存有上限；
    多层编码逐层验到末尾；压缩流的解码线程在运行时里释放上游流；程序换了请求体时原上传读完即弃；摘要只在有阶段协商了它时才算；
    开始排空时归还排队的通知并计入丢弃数；`--json` 的命令在预检失败或 stderr 被关掉时也给出终态文档。
  - **测试**：根包的测试用一个与发布的命令共用入口的 example 来启动官方角色，干净的检出里 `cargo test` 不再因为缺旁边的可执行文件而失败，
    也不会跑到旧的。时序敏感的测试继续改成看管道事件与手动时钟。
  - **验证**（Codex 在 Windows 上跑的，217b578；这一段按任务书只跑轻量 gate）：`cargo fmt --all` 无改动；headless 的工作区 clippy
    零输出；锁定依赖的测试 841 通过、0 失败、2 忽略（时钟夹具的入口，由父测试驱动）。desktop 与 Linux 交叉的 clippy、
    全量测试留给下一次完整 gate 与 CI。
  - **合入之后 main 红了**（CI 44718520924，四个门槛任务）：守卫测试 `every_aproxy_child_process_is_spawned_through_test_home`
    拦下了这一段新加的两处测试代码（`format_protocol.rs` 里用 `Command::new(env!("CARGO"))` 构建官方角色、`cli_tree.rs` 里
    自己给子进程设 `APROXY_HOME`）。轻量 gate 不含这个测试，所以没在本机发现；已要求 Codex 立刻修，并把它加进轻量 gate
    （任务书 Revision 23）。同一轮 windows (desktop) 还红了旧测试 `restart_all_skips_failed_instance_and_exits_nonzero`
    （实例刚起来、还没写下可重新拉起它的记录时就去重启），一并交给它查。
- W2c 第二部分（2026-11-20，Codex 实现；提交「aproxy-format keys：随机轮换、按上游退避冷却与会话粘性」「aproxy-format route：
  加权优先级、粘性渠道与重试切换」「aproxy-format convert 按路由后的目的端选择协议」「aproxy-format rewrite：按请求改头、
  编辑 JSON 指针与模型名」）：keys、route、rewrite，以及混合渠道的协议选择。
  - **keys**：冷却取配置的间隔与 `Retry-After`（秒数或 HTTP 日期）里较长的那个，记在失败那次尝试用的 origin 与 key 上，
    哪怕下一次已经换了去向；都在冷却时这次尝试以可重试的失败结束，不拿冷却中的凭据去发。
  - **粘性**：keys 与 route 各自用第一条用户文本的前缀算同一张票，不用完整请求体的摘要（那个认的是重放，对话每多一轮就变）。
  - **route**：票在每个优先级组里按权重定出起始渠道；重跑时走完同组别的渠道再到下一个优先级，都轮过一遍才回头。
  - **混合渠道**：convert 的 `to = "auto"` 按请求到达时的去向定上游协议（先查配置里的表，再看标准端点），两种协议都随请求走；
    route 与 convert 不共享状态。旧版每个渠道各有协议的能力由此恢复。
  - **rewrite**：只改头时不要请求体；JSON Pointer 的编辑先删、再设、最后改模型名，报告只写位置不写值。
  - **实测**（真实的官方角色，本机上游）：两个去向在故障切换前后各自只见到自己的凭据；一条链把 Chat 请求重试成 Messages、
    交回 Chat 的响应；一次 429 让 key 从 A 换到 B，下一个独立的请求仍用 B；12 组离线 SDK 解析都过。
  - **验证**（Codex 在 Windows 上跑的，1843b58；轻量 gate）：`cargo fmt --all` 无改动；headless 的工作区 clippy 零输出；
    锁定依赖的测试 854 通过、0 失败、2 忽略。
- W2c 第三部分（2026-11-20，Codex 实现；提交「aproxy-format policy：按状态和错误体判定重试、切换或停止」「aproxy-format adaptive：
  按摘要记失败并以滞回恢复转发」「aproxy-format 按模型或渠道给出按请求覆盖」，以及 main 的修复与第六段审查的 P1）：
  policy、adaptive、按请求覆盖。
  - **policy**：规则按顺序求值，条件是状态、失败类别与留下的响应字节（不跨过预览里省略的部分）。重试可以要求换路由、
    尊重 `Retry-After`；停止可以给一个客户端协议的新响应。
  - **adaptive**：没见过的摘要回 forward；还没写出字节的失败经 decide 升级；按请求最终的结果记账，升级过的算一次对 forward 不利，
    哪怕重试成功了；固定在 retry 之后，要一个完整的恢复窗口同时达到成功比例与延迟才放开。只存摘要与有界的计数。
  - **按请求覆盖**：route 按渠道、再按原始模型合并覆盖键。
  - **main 的修复**：守卫测试拦下的两处改成经 TestHome 构造无效的 home、把离线的 Cargo 构建精确登记为例外；
    `restart` 现在区分「实例还在启动」与「没有可重新拉起它的记录」，对应的旧测试也改成等实例真的就绪。
  - **第六段审查的 P1**：流完整只按源协议的终止事件认；Messages 转 Chat 时结束原因等 `message_stop`；成功的 JSON 先查形状；
    已经提交头的非流式转换出错时回客户端协议的错误体；转换错误只发一次错误结尾；拼渠道路径前拒绝带 `.` 段的客户端路径。
  - **两条转换限制**（转换库里的，补不了，记在 [format-roles](format-roles.md)）：Chat 的流式工具参数要到结束才交付；
    Messages 的 thinking 签名过不了 Chat，带工具结果续接时会丢。
  - **待纠正**：这一段给流式转换定的累计预算只有 1 MiB，长回答会被截断；已要求改成默认 64 MiB（任务书 Revision 24）。
  - **实测**（真实的官方角色与 aproxy）：200 里的错误标记被判为失败并重试；同一个摘要失败两次之后改走 retry、新摘要仍走 forward；
    提交之前的失败被升级；只对一个模型生效的尝试上限。
  - **验证**（Codex 在 Windows 上跑的，0f0ab3b）：`cargo fmt --all` 无改动；headless 的工作区 clippy 零输出；轻量 gate
    （这次起含守卫测试）874 通过、0 失败、2 忽略；headless 全量 1056 通过、0 失败、9 忽略；12 组离线 SDK 解析都过。
- W2c 第四部分（2026-11-20 到 21，Codex 实现；提交「aproxy-format init、check、try」「aproxy-format translate 的说明跟上角色命令」
  「decide 的 fallback 总是带上现行退避」，以及第六到第八段审查引出的四十来个修复）：工具子命令，t06 到此九段全部做完。
  - **工具**：`init` 生成四个场景的起步配置（`claude-relay`、`key-rotation`、`openrouter`、`adaptive-relay`），不覆盖已有文件、不写密钥；
    `check` 用 worker 启动时的同一套类型校验角色配置与整条链，不执行配置里的命令；`try` 拿本地样例干跑（含重试、forward 升级、
    流的回信与 notify），只打印结构上的变化，默认不启动第三方命令，要启动得显式加旗标并提示样例与环境会交给它。
  - **流式转换的预算**：`convert.stream_budget_bytes` 默认 64 MiB（先前的 1 MiB 已废）；单次回信以实例在 hello 的 `limits` 里
    给的行长上限为准，放不下时以客户端协议的错误收尾，已交付的增量保留，不触发熔断。
  - **协议一侧的两处补充**：hello 带可选的 `limits`（§10.2）；decide 的 `fallback.delay_ms` 总是给出（§10.5）。
  - **粘性与故障切换**：keys 与 route 各自用带权重的 rendezvous 哈希，去掉或冷却一个成员只挪动原来落在它身上的对话；
    `[A, B, C]` 选中 B 之后依次是 A、C；`to = "auto"` 不改 route 选定的地址；rewrite 不能设密钥头与 aproxy 自己管的头；
    改体时保持数字精度与成员顺序。
  - **Windows 上启动失败的退出**（CI 上出现过一次以 `0xC0000374` 堆损坏退出）：改成先收回运行时再退出进程，
    去掉了「在运行时线程还活着时结束进程」这个已知的隐患。**原来那次崩溃没有复现，也没能证明就是这个原因**，再出现要接着查。
  - **测试**：测试自己的监听直接绑定系统分配的端口（先选端口后绑定在 CI 上撞过一次）；守卫测试的 Cargo 例外按整句匹配。
  - **两条转换限制**不变（见 [format-roles](format-roles.md)）。第四段两项吞吐没过线也仍然留给服务器上的重测。
  - **验证**（Codex 在 Windows 上跑的，aeb902b）：`cargo fmt --all` 无改动；headless 全量 1094 通过、0 失败、10 忽略；
    headless、desktop 与 Linux 交叉三种 clippy 零输出；离线 SDK 16 组（12 组解析、4 组中止）都过。
    合入后 main 的 CI（44751631017）九个门槛任务全绿。
- **W5b — desktop operations and Tauri bridge (implemented).** The window has one home view, 32 explicitly granted commands, acknowledged invalidations and on-demand history. The injected host handles identity-checked control, locked redacted forms, private preferences/bookmarks, the installed binary’s update stream and opt-in login restore. A hidden window drains an active installer through its terminal event and exit. No public CLI or installer wire format changed. The handover waiter rereads durable state after exit and preserves private failure evidence; the original CI child's exact exit was not recoverable.
  - **Verification before final acceptance:** `cargo fmt --all -- --check`, both workspace Clippy variants, `cargo test -p aproxy-ipc --locked` (33; all features 88 + 1 doctest), headless library (458), `contract_v0_2` (5), `install_flow` (13) and `desktop_ops` (9 through update/state) passed. The isolated Windows registration and hidden-help tests subsequently passed individually. In a real WebView2 window, reload/resync and close-during-install worked against temporary homes and an offline installer. The Windows login carrier was measured, not registered in the real Run key. The arm64 macOS headless library was cross-checked; no Mac login was tested.
  - **Reviewed bridge behavior:** Run forms echo the read file token; history withholds unsafe rows and checks decoded body bytes across chunk boundaries. Pinning verifies a live source; unrelated churn no longer invalidates a cursor. Closing the window reaps a check but drains an accepted installer independently of inherited output handles. Release alignment resolves the desktop’s exact IPC pin in a separately retagged scratch workspace. Targeted Windows checks: `cargo test -p aproxy-ipc --locked --all-features` (94 unit + 1 doc), and headless `desktop_ops` (16), `proxy_integration` (118) and `contract_v0_2` (5). The intermittent Windows `stop` failure had no exit status in the CI log; the assertion now records it.
  - **W5c inherits** [the command, event and migration contract](w5b-desktop-api.md), not a finished frontend. Replace the mock backend, handle invalidations and unavailable history, and do not invent stopped profiles, daily rescue counts or release notes. The installed version can differ from the running window until it is reopened.
- **W3 — CLI language and fixed-English worker output (implemented).** Both CLIs resolve the reader's language once; paired Rust messages check parameters and count forms. Machine documents, IPC, role stderr and forensic records use English, independently of the worker's environment. Help options retain their original order; editable format templates translate only comments. Three private evidence slots allow a later reader to explain an error in its own language. Old, incomplete or foreign evidence remains verbatim. The force route and public wire shapes are unchanged. See [the contributor note](w3-i18n.md).
  - **Verification before final acceptance:** `cargo clippy --workspace --exclude aproxy-desktop --all-targets --no-default-features --locked -- -D warnings`; headless `cargo test --no-default-features --locked --lib` (463), `--test contract_v0_2` (5), `--test proxy_integration` (118), `--test format_protocol` (166, 2 ignored), `--test worker_language` (1) and `--test startup_compat` (1). Mixed-version startup was also exercised with the original reader and original writer. `cargo test -p aproxy-format --locked` (56 unit + 61 e2e, 1 ignored); generated TOML compared as parsed values at the same path in both languages. `python tests/i18n_origins.py`: 880 definitions, 156 Chinese drafts for previously English-only text. Whole-suite and cross-platform gates follow this record.
  - **Measurement:** Paired catalog text is 60,216 Chinese and 61,145 English UTF-8 bytes across count forms. Release executable deltas are measured separately for headless, desktop and the format tool; string bytes are not a proxy for binary size. The featureless IPC dependency closure and code table remain unchanged.
- **W5e — Desktop extension packages and Rust authorization (implemented; renderer qualification outstanding).** Copied page, mascot and theme packages are inspected without running them. Enablement follows the digest of all files. The [host contract](w5e-extensions.md) specifies the frame/port binding and 19 desktop commands; the Rust bridge separates extension reads from page selections. Each write is attributed and checked against the exact target before execution. Install always asks, instance verbs may be remembered per digest and home, and config writes are not offered.
  - **Verification on Windows:** `cargo test --workspace --exclude aproxy-desktop --no-default-features --locked --no-fail-fast` (1298 passed, 0 failed, 11 ignored); `cargo test --workspace --locked --no-fail-fast` (1298 passed, 0 failed, 11 ignored). Gate L included the headless `desktop_extensions` (17), `desktop_ops` (16) and `worker_language` (2) targets. Both workspace all-targets Clippy variants and the headless `x86_64-unknown-linux-gnu` cross-Clippy passed with warnings denied. Local mirrors and the manual-clock child now report their bound ephemeral ports; the language comparison waits on the raw status document without dropping any fields.
  - **Release boundary:** Themes need no script engine. Windows has a verified-shell check, a top-document-only keyed sender and native WebView2 hooks, but its compiled qualification switch is closed. The lead's shipped shell, host and real-window checks are separate; the Rust gates did not exercise a browser or run native Unix tests. Linux/macOS scripts remain unverified. Enabling a script would trust it with redacted reads; the headers and hooks do not prove that speculative loads, DNS or WebRTC cannot disclose them.
- **W5e——主题包与扩展包（Rust 一侧，t10b，2026-11-23）。** 本地 ZIP 或目录的快照、可选的匿名 GitHub 仓库与 release 核对、
  原子的包变更、有类型的 `desktop.toml`、扩展的设置、存储与文件，以及 `aproxy-pkg` 资源协议，都已实现。它取代上面沙箱与
  租约那一条：装好的扩展在主页面里运行；安装之前，页面对每条适用的提醒分别要一次确认。见[页面契约](w5e-extensions.md)。
  - **验证：** Windows 上 `cargo test --workspace --exclude aproxy-desktop --no-default-features --locked --no-fail-fast` 与
    `cargo test --workspace --locked --no-fail-fast` 各为 1,290 通过、0 失败、11 忽略。Gate L 含 39 个 IPC 单测与 4 个文档测试、
    467 个 headless 库测试、5 项冻结的 v0.2 契约检查、10 个包测试、16 个桌面操作测试与码表测试。headless、带界面与交叉编译到
    Linux 的 headless 全目标 Clippy 都在拒绝警告的设置下通过。在空的 target 里，先构建 `format-echo` 示例再跑只含库的测试。
  - **边界：** 这里没有 format 的 control 进程（t10c），页面一侧由主代理另做（`596bb92c`，错误对齐在 `3e47277e`）。GitHub API
    的测试只走回环地址。真实窗口里的拖放、相对路径的字体与模块、注入的样式、重新激活，以及 macOS 原生的进程存活判断，
    留给主代理与 CI。
