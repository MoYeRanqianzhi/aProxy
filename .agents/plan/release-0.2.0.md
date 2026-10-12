# 0.2.0 计划

> 状态（2026-10-11）：方向已由用户定下（见记忆 [0.2.0-direction](../memory/2026-10-11-0.2.0-direction.md)）；
> 四路审查与综合已完成：`.agents/review/2026-10-11-0.2.0-audit/`（dataplane、control-plane、config-cli-install、
> release-tests-docs，综合稿 synthesis.md：合并后 P0 16 条、工作流归属、待拍板决策 D-1…D-11、主代理可自定的
> 设计点 §3.12）。综合稿的 D-10（alpha 不公开发布）被用户「先发 alpha.1 验证可信发布」的定调取代；§3.12 里
> 「不建 aproxy-ipc / aproxy-ui crate」被用户「新增两个 crate」的提议取代（见 W5）。
> 2026-10-12：alpha.1 第 1 步（W1）完成并推送（fbcc738）；第 2 步（台账 + 强制安装）由子代理实现中；UI 原型按用户
> 第一轮风格反馈返工中（第一版仍带偏移硬阴影，未达要求）。

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

## 下一步

1. 收齐四份审查报告，汇总成一张发现清单（P0 / P1 / P2），与本计划的工作流对齐、排序。
2. 先做 W1（独立、确定），同时出 W2 的契约设计稿。
3. W2 定稿后再动 W3 基础设施、W4、W5。
