# 计划：自定义心跳 + format 扩展（2026-10-06 起）

## 为什么
用户 2026-10-06：「加入可自定义心跳，将 format 继续拓展，心跳也可基于内容动态生成，实际上
aproxy 很多配置都可以和 format 搭配，使其实现超高自由度的拓展能力，深入思考一下。」默认仍是
`: keepalive` 注释。随后又说「发现的问题就修复，别问我，你自己决定」。

## 约束（来自记忆与既有设计，改设计前先复核）
- aProxy 本体不内置任何协议特例、不伪造协议事件（[[claude-code-stream-watchdogs]]：与真实
  `message_start` 冲突）。协议知识只能在用户配置或 format 程序里——format 是扩展点。
- 事件级空闲超时（Claude Code 600s、Codex 300s）只认真实事件：注释、`event: ping` 都不算
  （Claude Code 黑盒实测）。所以静态心跳解决不了它们，只有 format 按协议生成真实事件才可能。
- Gemini 的 @google/genai 1.30.0 会被注释心跳触发解析 bug，空行无害
  （[[gemini-cli-sse-parsing]]）——静态可配心跳的一个现成用途。
- 信封契约在 aproxy-envelope crate，aproxy-format 独立发版（format-v*），协议变更两边都要跟。
  「请求/响应转换器是不同进程、无共享状态」是既有铁律；跨阶段状态只能经 aProxy 转交。
- 请求转换每个请求只跑一次、各次重试复用结果；心跳在提交点之后每个 keepalive 间隔一次，
  `drive()`（src/proxy.rs）统一驱动；心跳用 try_send，绝不能阻塞上游读取。

## 设计（草案，实现前再审一遍）
1. 静态心跳 `keepalive_heartbeat`（config.toml，字符串，默认 `": keepalive\n\n"`）：每拍原样写出。
   校验保证回放开始时 SSE 解析器处在事件边界——必须以换行结尾；含字段行（非 `:` 开头的非空行）
   时必须以空行结尾，否则会与回放的第一个事件拼在一起。只在 toml 层（协议相关，按实例）。
2. 心跳转换器 `heartbeat_transform`（与 request_transform 同形：command/args/mode/timeout_secs/extra）：
   提交后每拍发一个心跳信封，回信的 `body` 就是这一拍写出的字节（空 = 本拍不写）。失败、超时、
   回信不合 1 的边界规则 → 本拍退回静态心跳并 warn（同一请求只 warn 一次）。心跳转换器慢时不得
   拖住节拍：上一拍的调用没回来就跳过本拍的调用，直接发静态心跳。
   实现要点（2026-10-06 推敲，未写代码）：
   - 不能在 `drive()` 的 select 里直接 await 转换：drive 返回时会 drop 在途的 convert future，
     而 persistent worker 在往返中途被 drop 会被剔除（进程被杀），每拍都可能发生。改为每拍
     `tokio::spawn` 一个任务跑完整次转换，任务自己把结果写进响应通道。
   - 顺序保证：心跳任务写通道与「开始回放」必须互斥，否则心跳字节可能插进回放的事件中间。
     做法：sink 里放一个 `std::sync::Mutex<bool /*已开始回放*/>`；`send()` 写第一块真实字节前在锁内
     置位，心跳任务在锁内检查未置位才 `try_send`。try_send 不阻塞，持锁时间极短。
   - state 要在心跳任务与主流程间共享（心跳回信可改 state，响应转换要看到），`ExchangeCtx.state`
     需改成 `Arc<Mutex<Option<String>>>` 或在 sink 里另存一份、回放前合并。
   - 心跳专属输入（seq、elapsed_ms、attempt、最近失败摘要）放信封里一个嵌套对象，避免顶层字段
     越来越多；首拍带请求体，之后不带。
3. 跨阶段状态：信封加 `stage`（`request` / `heartbeat` / `response`）、`request_id`（按请求唯一）
   与不透明的 `state`（字符串）。任一阶段回信带 `state` 即替换该请求保存的值，缺省 = 不变；
   下一阶段原样收到。心跳信封另带 `seq`（第几拍）、`elapsed_ms`（自提交起）、`attempt`（当前
   第几次尝试）与最近一次上游失败的摘要；首拍带请求体，之后不带（体可能很大）。
   这让「心跳先发了真实协议事件 → 响应转换器据 state 去掉上游重复的开头事件」成为可能。
4. 其余可交给 format 的决策点（逐个评估，不一次做完）：按请求决定是否走保活（替代只看
   Accept/stream 的 keepalive_trigger，Gemini 的 `?alt=sse` 场景）、终态 error 事件的渲染、
   每次重试前改写请求（换 key/换上游的故障转移）、重试判定。每个都要先回答：调用频率与成本、
   失败时的安全回退、与「无限重试」核心目标是否冲突。

## 状态
- 第 1 步（静态 `keepalive_heartbeat`）已实现：校验规则 `config::heartbeat_problem`，心跳字节随
  `KeepaliveSink` 从 Pending 带进 Committed；单测 `keepalive_heartbeat_default_and_boundary_rule`、
  集成测试 `keepalive_heartbeat_is_configurable`，`config --show`/`doctor`/启动校验手工核对过。
- 第 3 步（`stage`/`request_id`/`state`）已实现，目前只有 request/response 两个阶段：
  `transform::ExchangeCtx` 随 `OutboundRequest` 走完三条通道；`request_id` 取 status 的请求计数
  （实例内第 N 个，1 起）；`stage` 用字符串而非枚举，旧版 format 遇到新阶段名照样能解析。
  验证：envelope 单测 `stage_request_id_and_state_roundtrip_and_stay_optional`、集成测试
  `transform_stages_share_request_id_and_state`（保活与非保活通道各一次，format-echo 的 `stateful`
  子命令）；aproxy-format 二进制经 test_format.py 喂新字段无异常（全仓库无 deny_unknown_fields）。
- 第 2 步（`heartbeat_transform`）已实现，按上面「实现要点」：每拍 spawn 独立任务、`in_flight` 防排队、
  `replay_gate` 闸门、state 改为 `ExchangeCtx` 内共享的锁。信封加 `heartbeat {seq, elapsed_ms, attempt}`；
  首拍带请求体。验证：集成测试 `heartbeat_transform_generates_heartbeats_and_hands_state_to_response`、
  `heartbeat_transform_failure_falls_back_to_fixed_heartbeat`；单测
  `dynamic_heartbeat_arriving_after_replay_started_is_dropped`（临时关掉闸门时失败，已核对）；
  test_format.py 新增 `--side heartbeat`。未做：每请求的「最近一次上游失败」摘要（重试循环里没有
  按请求保存的错误串，需要时再加）；真实客户端（Claude Code 等）对 format 生成的协议事件的反应未实测。
- 第 4 步评估（2026-10-06，读码所得，未实现）。每项回答三问：调用频率与成本、失败时的安全回退、与无限重试
  是否冲突。
  - 每次重试前改写请求（新阶段 `retry`）——**值得做，排第一**。频率：每次重试一次，受
    `max_retry_backoff_secs` 节流；回退：沿用上一次发出的请求；不冲突，反而补上无限重试的盲点：aproxy-format
    的轮换/加权/多渠道路由只在请求首次转换时选一次，某个渠道或 key 持续失败时，无限重试会一直打同一个坏渠道。
    要点：信封带 attempt、上一次失败的摘要（状态码、错误类型；目前重试循环里没有按请求保存的失败串，要先加）
    与上一次实际发出的请求；回信给新请求或空（沿用）；state 照常交接。两条重试循环（proxy.rs 的保活与非保活
    通道）都要接。信封与 aproxy-format 都是独立发版的公开契约，加字段要升 aproxy-envelope 版本。
  - 终态 error 事件的渲染——**暂不做，需要时先做静态模板**。频率极低（保活已提交而请求最终放弃：转换失败、
    spool 超限、解码失败、受限重试耗尽）；回退：内置渲染（Anthropic 形状的 `event: error`）；不冲突。价值在
    跨协议场景：switchyard 把 OpenAI 客户端接到 Anthropic 上游时，客户端不认这个形状。静态模板就够，不必为
    极少发生的事件拉起 format 进程。
  - 按请求决定是否走保活——**不做**。唯一已知需求是 Gemini 的 `?alt=sse`，用户 2026-10-06 定「gemini 不管」。
    真要做，优先让请求转换的回信带一个可选 `keepalive` 字段（零额外调用），或按 path+query 正则配置（同
    bounded_retry_paths），不新开阶段。
  - 重试判定——**不做**。format 能让请求不再重试，直接违背核心承诺；确定性失败的场景已有
    bounded_retry_paths。只允许单向放宽（不重试改为重试）的话，内置判定几乎对所有失败都重试，放宽空间很小。
- 下一步：第 4 步的 `retry` 阶段（见上）。
- 待定：`state` 的大小上限（目前不限，文档建议保持很小）。
