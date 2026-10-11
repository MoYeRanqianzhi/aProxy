# 0.2.0 审查：转发数据面（2026-10-11）

> 只读审查，未改任何已有文件。范围：`src/proxy.rs`、`src/retry.rs`、`src/decode.rs`、`src/server.rs`、`src/transform.rs`、
> `aproxy-envelope/`、`aproxy-format/`（轻看），以及 `tests/proxy_integration.rs`、`tests/transform_integration.rs`。
> 代码基线：HEAD `c1d4b96`。
>
> 方法：`proxy.rs` 生产代码（1–3522 行）逐段通读，测试模块按需抽查；`retry.rs`、`decode.rs`、`server.rs`、`transform.rs`、
> 信封 crate 全读；`aproxy-format/src/main.rs` 读前 400 行。先读了记忆 transform-format、forward-only、bounded-retry-paths、
> compressed-body-inspection、disconnect-billing、claude-code-stream-watchdogs、codex-stream-idle-timeout，与用户已定的决策冲突的看法
> 单列在第 4 节，没有当缺陷报。中文字符串统计用脚本抽取非测试代码里含中文的字符串字面量，再逐条人工判定去向。
>
> 证据等级：标「事实」的结论都附 file:line，来自代码阅读；标「推断」的没有运行验证。
> 没做的事：没跑 cargo build/test/clippy（target 目录与其他代理共享），没启动实例，没做黑盒实测。
> 外部核实一项：crates.io API 显示 `aproxy-envelope` 只发布过 0.1.0（2026-10-01）；仓库里的 0.2.0 还没发布（2026-10-11 查询）。

---

## 1. 总体判断

上一轮审查后，数据面的功能修复做得扎实，几项使命级缺陷都已闭合：
- 保活触发看请求体 `stream:true`（`keepalive_trigger`，默认 `any`）。
- 首轮就在心跳节拍内驱动。2xx 且未压缩的 SSE 头先行提交，否则一个间隔后提交骨架头。
- 上游请求进行中也发心跳。
- spool 收尾补了 flush。
- 转换器 stdout 串包与死 worker 的问题已修。
- 入站校验 Host/Origin。
- every_attempt 提供了故障转移。

`drive()` + `KeepaliveSink` 的提交点状态机在设计上是清楚的，测试覆盖也密。

0.2.0 的问题不在「功能缺什么」，在于几处还没定稿就要对外长期稳定的东西，以及会成倍放大改动成本的结构：

1. **对外契约还是半成品**。代理自己产生的 HTTP 错误体是中文纯文本、没有机读码。SSE 终态事件的类型名不统一，其中一个的
   message 还是中文。IPC `last_error` 是一段中文句子，`status` 原样打印。信封还没发布，却缺版本号、响应状态码和往返序号。
   这些正好落在 0.2.0 的 i18n 范围里：改语言本身就是一次破坏性变更，结构应该同一次定下来（DP-01/02/03/13）。
2. **结构**。`proxy.rs` 是 3522 行生产代码的单文件。首轮快速路径加两条重试循环，等于同一套「判定→计数→last_error→终态」
   写了三份，文案已经开始分叉。0.2.0 的结构化错误、i18n 和 UI 事件埋点都要改这三处。建议先拆分、统一循环，再做其余三件（DP-10）。
3. **可观测性几乎为零**。日志没有请求标识，并发请求的重试行无法归因。没有在途计数，idle 判定还会误判正在长时间单次尝试的实例。
   也没有事件源。UI 需要的数据都得从这里长出来（DP-11/12，第 8 节）。
4. **几处会导致「静默错误」或「无限挂起」的正确性边角**：
   - 上游 URL 构造失败会被当成网络错误无限重试（DP-05）。
   - Upgrade 请求在 4xx 上无限重试（DP-06）。
   - `read_timeout` 也卡首字节，长时间的非流式生成会确定性超时（DP-07）。
   - spool 目录在 bind 之前被清空、端口 0 的实例共用同一目录（DP-08），spool 文件丢失时以空 body「成功」回放（DP-09）。
   - 心跳转换任务持有响应通道，会推迟流结束（DP-15）。

建议的落地顺序：

1. 纯搬移拆模块，行为零变化。
2. 统一 attempt 循环。
3. 引入结构化失败原因，在出口处统一渲染 HTTP、SSE、IPC、日志与信封，同时完成 i18n。
4. 定稿契约：错误体与终态事件、信封、重试与超时配置。
5. 做在途登记与事件流。

第 1–2 步是第 3–5 步的前提，否则同样的改动要做三遍。

---

## 2. 优先级总览

| 编号 | 级别 | 类别 | 一句话 | 成本 |
|---|---|---|---|---|
| DP-01 | P0 | 契约/i18n | 代理自产的 HTTP 错误体是中文纯文本、无错误码、和上游错误分不开 | M |
| DP-02 | P0 | 契约 | SSE 终态 error 事件命名不一、message 混中文、缺机读字段、五处手写 | S |
| DP-03 | P0 | 结构/i18n/IPC | 失败原因只有字符串，同一段中文同时进 HTTP、SSE、IPC、信封和日志 | M |
| DP-04 | P0（决策） | 契约 | 重试策略的配置形状（bounded_retry_paths、规则表、Retry-After、抖动）要在 0.2.0 前定 | S/M |
| DP-05 | P1 | 正确性 | 非法或相对的上游 URL（format 改写出来的）被当网络错误无限重试 | S |
| DP-06 | P1 | 正确性/契约 | Upgrade/WebSocket 请求被降级，在 4xx 上无限重试 | S |
| DP-07 | P1 | 正确性/契约 | `read_timeout_secs` 同时卡首字节：长时间的非流式生成确定性超时、无限重试 | M |
| DP-08 | P1 | 正确性 | spool 目录按配置端口共享、在 bind 前清空，会删掉活实例的在途文件 | S |
| DP-09 | P1 | 正确性 | spool 文件丢失时以空 body「成功」回放 | S |
| DP-10 | P1 | 结构 | proxy.rs 单文件；首轮与两条循环三份拷贝 | L |
| DP-11 | P1 | 可观测 | 日志无请求标识；request_id 重启后重复 | S |
| DP-12 | P1 | UI/正确性 | 无在途登记；idle 判定会误判长时间单次尝试中的实例 | M |
| DP-13 | P1 | 契约 | 信封 0.2.0 发布前定稿（版本、status、序号回显、机读原因、缺省语义、non_exhaustive） | M |
| DP-14 | P1 | 契约 | `spool_limit_mb = 0` 被当成 1 MiB，与其余字段的 0 语义相反 | S |
| DP-15 | P1 | 正确性 | 动态心跳任务持有 Sender，回放后流迟迟不结束（timeout 0 时可能永不结束） | S |
| DP-16 | P2 | 正确性 | aproxy-format 不看 stage，用有无 method 区分请求与响应，心跳阶段被当成请求 | S |
| DP-17 | P2 | 性能 | 每个 data 行都做完整 JSON 解析，内存模式扫两遍，且都在 async 线程上 | S–M |
| DP-18 | P2 | 性能/正确性 | 错误扫描器逐行 drain，单块内 O(n²)；超长块先截头再分行 | S |
| DP-19 | P2 | 性能 | 磁盘 spool 每块一次阻塞池往返；回放每块分配两次 | S |
| DP-20 | P2 | 正确性 | every_attempt 换了请求以后，心跳转换看到的还是首次转换的请求 | S |
| DP-21 | P2 | 正确性 | 「未提交 + 2xx 非 SSE」分支的响应转换不经 drive，客户端断开没人察觉 | S |
| DP-22 | P2 | 正确性 | 响应转换解码失败时把压缩字节交给 format，却剔掉了 content-encoding | S |
| DP-23 | P2 | 正确性 | 请求体带 content-encoding 时 body_stream 判定恒假 | S |
| DP-24 | P2 | 正确性/文档 | 本地磁盘故障回 400 不记录；SpoolFailed 的注释与行为不符；架构文档与退化行为相反 | S |
| DP-25 | P2 | 正确性 | 协议无关的错误判定有盲区（response.failed、按连接关闭分帧的截断流） | M |
| DP-26 | P2 | 正确性 | 退避不读 Retry-After、没有抖动 | S |
| DP-27 | P2 | 正确性 | 优雅停止 10 秒后强退，已提交的流被截断且没有终态事件 | S–M |
| DP-28 | P2 | 卫生/性能 | tcp_nodelay、无用依赖、逐头转换、HEAD content-length、过期注释、心跳超时默认值 | S |
| DP-29 | P2 | 结构 | 转换往返先写后读、body 多次物化 | S–M |
| DP-30 | P2 | i18n | 87 处日志与 3 处前台横幅要英文化；skill 引用了这些原文 | M |

---

## 3. 发现明细

### DP-01 ［P0｜契约/i18n］代理自产的 HTTP 错误体：中文纯文本、无机读码、与上游错误不可区分

- **证据（事实）**：以下错误体都经 `(StatusCode, String).into_response()` 产出，content-type 为 text/plain，没有任何头或字段标明来自 aProxy。
  - 403 入站拒绝：多行中文文案，`proxy.rs:293-322`。
  - 400 请求体读取失败：`proxy.rs:1298-1302`、`3360-3364`。
  - 413：`proxy.rs:3236-3244`，内含 `limit_text` 的「不设限」（3227-3233）。
  - 502 请求转换失败：`proxy.rs:1623-1629`，内插 `TransformError` 的中文 Display（`transform.rs:79-108`）。
  - 502 spool 上限、本地磁盘故障：`proxy.rs:1654-1669`。
  - 502 仅转发模式上游失败：`proxy.rs:3368-3375`。
  - 502 保活任务异常：`proxy.rs:3040-3043`。
- skill 把这些中文原文当排障锚点逐字引用：
  - aproxy-cli `behaviors.md:70、79、261`，`troubleshooting.md:73-97`，`config-toml.md:321、393`，`clients.md:192、205`。
  - aproxy-format `protocol.md:226-256`，`troubleshooting.md:11-16、39-43`。
  - `transform.rs:81-83` 的注释还写明各变体文案的开头被文档按「错误含 xxx」检索。
- **为什么重要**：
  - 0.2.0 已定调「发给 HTTP 客户端的错误体固定英文」。改文案本身就是一次对外破坏，skill、用户脚本和 agent 的错误识别都锚定了原文，这种破坏只该发生一次，所以结构要和语言一起定。
  - 客户端 SDK 按 JSON 解析非 2xx 的体：Anthropic 是 `{"type":"error","error":{type,message}}`，OpenAI 是 `{"error":{message,type,code}}`。纯文本只能原样显示（推断：Claude Code 会把整段中文多行文本当错误信息打出来）。
  - aProxy 的价值在于吸收上游错误。受限透传给客户端的是上游原体，客户端得能分清「代理的终态」和「上游透传」。
- **建议**：用 DP-03 的结构化原因统一渲染。
  - 体：`{"type":"error","error":{"type":"aproxy_<code>","message":"<English>","aproxy":{"request_id":"…","attempts":N,"upstream_status":…}}}`，`content-type: application/json`。
    - `error.type` 正是 Anthropic 与 OpenAI SDK 读取字符串类型的位置。
    - **不要**在 `error.code` 放字符串：Gemini 的 `error.code` 是整数，推断强类型客户端会反序列化失败。
  - 头：`x-aproxy-error: <code>`、`x-aproxy-request-id: <id>`。只加在 aProxy 自产的响应上，上游透传的响应保真不变。
  - 码表初稿：
    - 403：`aproxy_inbound_host_rejected`、`aproxy_inbound_origin_rejected`。
    - 413：`aproxy_request_too_large`。
    - 400：`aproxy_client_body_aborted`。
    - 500 或 507：`aproxy_local_storage_failed`。
    - 502：`aproxy_transform_failed`（子类放 `aproxy.kind`，取值 rejected、protocol、spawn、io、timeout、died）、`aproxy_response_too_large`、`aproxy_upstream_failed`（仅转发模式）、`aproxy_bad_upstream_url`（DP-05）。
    - 501：`aproxy_upgrade_unsupported`（DP-06）。
    - 500：`aproxy_internal`。
  - 码表写进 docs 与 skill，声明「码只增不改、message 不是契约」。skill 改成按 `error.type` 或 `x-aproxy-error` 排障。
  - SDK 兼容性要用各 SDK 的实际解析做一次契约测试（本次未验证，属推断）。
- **成本**：M。渲染层本身是 S，替换全部调用点并同步 skill 是 M。

### DP-02 ［P0｜契约］保活通道终态 SSE error 事件：命名不一致、message 混中文、缺机读字段、五处手写

- **证据（事实）**：
  - 通用构造 `terminal_error_event`（`proxy.rs:1633-1639`）只有 `proxy_transform_failed` 在用（2704），而它的 message 是中文 `请求转换失败: …`（1614）。
  - 另外四处是手写的 JSON 字节串：
    - `proxy_spool_limit`：2846-2848。
    - `proxy_spool_failed`：2859-2861。
    - `proxy_decode_failed`：2969-2971。
    - `upstream_error`：3023-3025。这条用 `format!` 拼 JSON，上游状态码只出现在英文句子里。
  - skill 把这些类型名列成了对外表格（`behaviors.md:163-167`）。
- **为什么重要**：
  - 这是 agent 在已提交响应上唯一能看到的终态信号，0.2.0 之后应长期稳定。
  - `upstream_error` 是泛名，推断会与上游自己的错误类型混淆。
  - 受限重试路径用尽时，真实状态码机读不到。
  - 手写 JSON 一旦内插变量就有转义风险：3023 已经在 `format!` 里拼 JSON，目前只因 status 不含引号才安全。
- **建议**：
  - 与 DP-01 共用一套码与 JSON，帧为 `event: error\ndata: <同一 JSON>\n\n`。
  - 类型统一加 `aproxy_` 前缀。用 `aproxy_retry_exhausted` 取代 `upstream_error`，并带上 `aproxy.upstream_status`、`aproxy.attempts`。
  - 全部经同一个构造函数生成，message 走 serde 序列化。
  - 「已提交后放弃」类事件统一带 request_id。
- **成本**：S。

### DP-03 ［P0｜结构/i18n/IPC］失败原因只有字符串，同一段中文同时进 HTTP 体、SSE、IPC、信封与日志

- **证据（事实）**：
  - `AppState::note_upstream_failure(&str)`（`proxy.rs:432-434`）约有 20 个调用点，传入的全是中文 `format!`，例如 1370、1376、1380、1393、2004、2051、2055、2839、2961、3009、3020、3029、3368、3431。
  - 这些串写进 `IpcStats.last_error: Mutex<Option<(String,u64)>>`（`daemon.rs:496、501-517`，空串兜底为「未知错误」）。
  - 再经 IPC v1 的 `LastError { message, at }`（`daemon.rs:213-219`）原样交给 `aproxy status`，后者逐字打印（`commands/status.rs:63-74`）。
  - `ForwardResult::NetworkError(String)`、`SpoolFailed(String)`（`proxy.rs:3062-3080`）与 `TransformError` 的 Display（`transform.rs:79-108`）也都是一段文本被多个出口共用。
  - 网络错误还把中文前缀拼在 reqwest 的英文原因前面（`proxy.rs:3179-3182`）。
- **为什么重要**：
  - 0.2.0 定调「IPC 线上文本英文、status 输出随系统语言」，字符串形态的 last_error 两头都满足不了。
  - UI 要按原因聚合、着色、计数，字符串做不到。
  - skill 被迫锚定文案（见 DP-01）。
- **建议**：引入 `FailureReason`（或 `ProxyError`）枚举，每个变体给出稳定的 snake_case `code()` 与结构化 `params()`。
  - 变体示例：
    - `Network{kind: connect|timeout|dns|tls|body_read|other, detail}`
    - `UpstreamStatus{status}`、`UpstreamErrorBody{status}`
    - `ResponseTooLarge`、`LocalStorage{op}`
    - `Transform{kind}`、`DecodeFailed{encoding}`、`BadUpstreamUrl`
  - 只在出口渲染：
    - HTTP 与 SSE（DP-01/02），英文。
    - 日志，英文，并带 `code=` 字段。
    - IPC：`LastError` 加 `code` 与 `params`，`message` 改英文。IPC v1 约定加应答字段不升版本，所以这是加性变更。
    - status 与 UI 按 code 本地化。
    - 信封 `RetryInfo`（DP-13）。
  - reqwest 错误按 `is_connect()`、`is_timeout()`、`is_body()`、`is_builder()` 分类（`is_builder` 见 DP-05）。
- **成本**：M。

### DP-04 ［P0｜决策｜契约］重试策略的配置形状需要在 0.2.0 前定稿

- **证据（事实）**：
  - 现在唯一的出口是 `bounded_retry_paths`（`config.rs:294-309`），toml 与 settings.json 两层都有。
  - 尝试次数写死为 `BOUNDED_RETRY_MAX_ATTEMPTS = 3`（`retry.rs:43-53`），只按路径匹配（`proxy.rs:425-429`）。
  - 状态码判定写死为 4xx/5xx（`retry.rs:39-41`）。
  - 退避是确定性的：不读 Retry-After，没有抖动（`retry.rs:14-32`）。`max_retry_backoff_secs` 平铺在顶层（`config.rs:251-254`）。
  - 上一轮路线图 0.1.x 第 6 项（Retry-After 与抖动）、第 7 项（确定性错误出口）都没做，第 7 项还特意提醒「避免与 bounded_retry_paths 形成两套并行机制」。
- **为什么重要**：这三项迟早要加，DP-25 的协议盲区也需要同一个出口。如果 0.2.0 先把 `bounded_retry_paths` 作为稳定接口带出去，之后再加按状态码或错误体的规则，就只能做成第二套并行机制，或者再来一次破坏性迁移。
- **建议**：两个方案，由用户拍板。
  - **A. 规则表**。`[retry]` 下放 `max_backoff_secs`、`jitter`、`respect_retry_after`，再用 `[[retry.rules]]` 写规则，字段如 `path = '…'`、`status = [404]`、`body = '…'`、`max_attempts = 3`。`bounded_retry_paths` 删除，或保留为展开成规则的单行语法糖。匹配沿用现有语义：整体锚定的正则，查询串不可省略。
  - **B. 保持平铺**。声明 `bounded_retry_paths` 是永久原语，并且现在就写清将来的 `retry_rules` 与它并存时的优先级。
  - 无论选哪个：内置默认不变（无限重试），不内置任何 URL 或协议特例，遵守记忆 features-are-config 的定调。顺带定 keepalive 系字段的命名（`keepalive_*` 与 `heartbeat_transform` 混用，`keepalive` 还容易和 HTTP/TCP keep-alive 混淆），要改名只能在 0.2.0。
- **成本**：决策加文档 S，实现 M（含 DP-25、DP-26）。

### DP-05 ［P1｜正确性］构造不出上游请求（非法或相对 URL）被当网络错误无限重试

- **证据（事实）**：
  - 请求转换输出的 url 不做任何校验（`transform.rs:794`）。
  - `forward_once` 调 `state.client.request(method, url)`（`proxy.rs:3107`）。reqwest 把 URL 解析失败推迟到 `send()`，以 builder error 返回。
  - 代码把 `send()` 的所有错误都映射成 `ForwardResult::NetworkError`（`proxy.rs:3132-3136`）。
  - 网络错误在两条循环里都无限重试，也不受 bounded 封顶（`proxy.rs:2003-2007、2837-2841`）。
- **失败场景（推断）**：第三方 format 回了 `"url":"v1/messages"`（漏了 scheme）或带空格的 url。每次尝试都在本地立即失败，从不到达上游。客户端在心跳里无限等待，status 的最近错误只有一句 builder error。
- **建议**：
  - transform_request 里用 `url::Url::parse` 校验并要求 http/https，失败归为 `TransformError::Protocol`（502）。
  - `forward_once` 遇到 `e.is_builder()` 返回新的终态变体（`aproxy_bad_upstream_url`）。
  - every_attempt 的重新转换同样校验，失败按既有规则沿用上一次的请求。
  - 补测试：format 回相对 url 时客户端拿到 502 终态，而不是挂起。
- **成本**：S。

### DP-06 ［P1｜正确性/契约］Upgrade（WebSocket）请求被静默降级后在 4xx 上无限重试（上一轮 proxy-core-07，未修）

- **证据（事实）**：`connection` 与 `upgrade` 在 `HOP_HEADERS` 中被剥掉（`proxy.rs:78-89`），全仓没有任何 Upgrade 处理；4xx 一律可重试（`retry.rs:39-41`）。
- **为什么重要（推断）**：
  - 上游收到的是一个不带 Upgrade 的普通 GET，多半回 4xx。aProxy 随即无限重试，客户端走非保活路径，收不到任何字节。
  - 记忆 codex-stream-idle-timeout 已把「Codex 默认改用 WebSocket 传输」列为复查触发点。一旦发生，经 aProxy 的 Codex 会挂死，而不是回退到 HTTP。
- **建议**：在 handler 入口（入站校验之后）识别 `Connection: upgrade` 或 `Upgrade` 头，立即回 501 + `aproxy_upgrade_unsupported`（DP-01 的形状），不进重试，并写进文档。真正的 WS 透传语义上等同 forward_only，等需求出现再做。
- **成本**：S。

### DP-07 ［P1｜正确性/契约］`read_timeout_secs` 同时钳制首字节等待，长时间的非流式生成会确定性超时并被无限重试

- **证据（事实）**：
  - `AppState::new` 把 `read_timeout_secs` 交给 reqwest 的 `read_timeout`（`proxy.rs:341-343`）。
  - 注释自己承认它「同样钳制首字节等待」（`proxy.rs:326-330`、`config.rs:262-266`），默认值 300（`config.rs:418-420`）。
  - 超时算 NetworkError，无限重试，不受 bounded 封顶。
- **失败场景（推断）**：
  - stream:false 的请求，上游完整生成要 6 分钟（高推理强度模型的非流式调用）。每次尝试都在 300s 处被 aProxy 自己掐断，永远不会成功。
  - 上游是否对被掐断的请求计费取决于上游，未核实；如果计费，每一轮都在花钱。
  - 保活路径上的 stream:true 请求不受影响，因为上游很快回头并持续发事件。
- **建议**：0.2.0 拆成两个语义：
  - `first_byte_timeout_secs`：等响应头。
  - `read_idle_timeout_secs`：两次读之间。
  - 实现上不再用 reqwest 的 read_timeout，改为 `timeout(send())` 加 `timeout(chunk())`。
  - 默认值由团队定。建议首字节默认显著大于空闲超时，或取 0 = 不限：aProxy 本来就为等待而生，保活通道会维持住客户端。
  - 这是语义变更，只能在 0.2.0 做。
- **成本**：M。

### DP-08 ［P1｜正确性］spool 目录按「配置端口」共享，且在 bind 之前清空，会删掉另一个活实例的在途临时文件

- **证据（事实）**：
  - `serve_forever` 第一步就执行 `clean_spool_dir(port_of(&listen_addr))`（`server.rs:73-80`），早于 TCP bind（83-92）与控制端点创建（103-115）。
  - 清理会删除目录下全部 `*.spooltmp`（`daemon.rs:45-72`）。
  - AppState 的 spool 目录同样按配置端口计算（`proxy.rs:360-366`）。
- 由此有两条触发路径：
  - **(a) 端口 0**：`listen_addr` 为端口 0 的多个实例共用 `spool/0`，每个新实例启动都会清掉其他实例的在途文件。start 的同端口预检按端口探测控制端点（`commands/start.rs:180-200`），推断它对端口 0 探测不到任何实例。
  - **(b) 同端口第二个守护**：同一 home、同一端口的第二个守护进程会在 bind 失败退出之前就清空原实例的目录。测试 `daemon_second_instance_on_same_port_exits`（`tests/proxy_integration.rs:5284-5327`）走的正是这条路径，但只断言原进程还活着。
- **后果（事实加推断）**：
  - 请求体文件被删：下一次重试 `apply_to` 打不开文件，走 SpoolFailed 终态 502（`proxy.rs:3124-3130`）。
  - 响应 spool 被删：回放时打不开，按 DP-09 回放为空并当作成功。
  - Windows 上 Rust std 以 FILE_SHARE_DELETE 打开文件，删除对持有句柄的进程同样生效（`proxy.rs:776-778` 的注释自述）。
- **建议**：
  - 清理移到 bind 与控制端点都成功之后，按实际端口或进程身份建目录。
  - 或者利用文件名已含 pid 这一点（`proxy.rs:554-558、1002-1006`），只删 pid 已经不存在的文件。
  - 补测试：第二个守护启动失败之后，原实例在途的磁盘 spool 请求仍能完成。
- **成本**：S。

### DP-09 ［P1｜正确性］spool 文件丢失时以空 body「成功」回放

- **证据（事实）**：
  - `SpooledBody::into_stream` 打开文件失败时只 warn，然后返回空流（`proxy.rs:1110-1126`）。`DiskSpoolStream` 提前 EOF 也按正常结束（1179-1183）。
  - 保活已提交路径先用 `body.len()` 判断非空（2985），随后回放的是空流。客户端收到上游真实头或骨架头、0 字节，然后正常结束。
  - 未提交路径：如果上游带 content-length，会因长度不符被 hyper 中止；否则就是空的 200。
  - 响应转换读文件失败也会落到这条路（`transform.rs:829-840`）。
- **建议**：打开失败与提前 EOF 一律产出 `Err`，让 hyper 中止连接、客户端能感知失败；或者回放前确认文件存在，否则走 SpoolFailed 终态。与 DP-08 一起修。
- **成本**：S。

### DP-10 ［P1｜结构］proxy.rs 单文件 3522 行生产代码；首轮与两条重试循环是三份「判定→计数→终态」拷贝

- **证据（事实）**：
  - 三份拷贝的位置：
    - handler 里的首轮判定与终态：`proxy.rs:1355-1439`。
    - 非保活循环：1935-2060。
    - 保活循环：2654-3044。
  - TooLarge、SpoolFailed、NetworkError 与 bounded 处理及其 last_error 文案，三处各写一遍：1375-1383 / 2008-2017 / 2842-2867，以及 2044-2054 / 2999-3028。
  - 文案已经开始分叉：「达上限透传」与「达上限终止」。
  - 活动时间戳刷新出现四次：1256-1262、1959-1965、2742-2748、3416-3422。
- **为什么重要**：0.2.0 的结构化错误（DP-03）、i18n（DP-30）和事件埋点（DP-12）都要改这些位置。不先统一，每件事都要同步改三份，漂移风险与审查成本都乘三。
- **建议**：见第 7 节。
  - 核心是一个 attempt 循环加两种 Responder：Buffered 不提交、依靠 hyper drop 取消；Keepalive 用现有的 KeepaliveSink 与 drive。首轮就是循环的第 1 次。
  - 判定抽成纯函数 `classify(attempt, result, bounded) -> Outcome`，可以单测。
  - 先纯搬移拆模块（行为零变化），再统一循环。
- **成本**：L（拆分 M + 统一 M）。

### DP-11 ［P1｜可观测］日志没有请求标识，并发请求的重试日志无法归因

- **证据（事实）**：
  - proxy.rs 不建任何 tracing span，日志行不带 request_id。grep `span|instrument|request_id` 只命中注释，以及心跳告警一处（2148）。
  - 记忆 bounded-retry-paths 记录过这个排障痛点：「重试行没有请求 ID，归因靠相邻 INFO 行」。
  - request_id 取实例内的请求计数（`proxy.rs:1265-1269、1321`），重启后从 1 重来。
- **建议**：
  - handler 入口建 `info_span!("req", id, method, path)`（path 脱敏），保活任务用 `.instrument(span)` 继承。
  - request_id 改为跨重启可区分，例如 `<启动时刻 hex>-<序号>`。同一个值用于日志、信封、事件（DP-12）和 `x-aproxy-request-id`（DP-01）。
  - 信封 0.2.0 还没发布，现在改 request_id 的文档语义没有成本。
- **成本**：S。

### DP-12 ［P1｜UI/正确性］没有在途请求与每请求状态，闲置判定会把正在长时间单次尝试的实例判为闲置

- **证据（事实）**：
  - 观测数据只有累计计数与最近错误（`IpcStats`，`daemon.rs:487-499`）。
  - `last_activity_secs` 只在收到请求时与每个重试轮开头刷新（`proxy.rs:1256-1262、1959-1965、2739-2748`）；仅转发模式每个 chunk 都刷新（3416-3422）。
  - 缓冲路径上，单次尝试期间不刷新，例如一个 40 分钟的长生成，或给慢客户端的长回放。
  - `status --idle` 与 `stop idle` 按 settings 的 `idle_timeout_secs`（默认 1800）判定（`commands/status.rs:10-12`、`commands/stop.rs:40`）。
  - 这就是上一轮的 daemon-ipc-05，仍未修。
- **建议**：AppState 增加 `RequestRegistry` 与 RAII `InFlightGuard`（详见第 8 节）。idle 判定改为「在途数为 0，且距最近一次请求结束超过阈值」。
- **成本**：M。

### DP-13 ［P1｜契约］信封 0.2.0 发布前定稿（crates.io 只有 0.1.0，现在改不花成本）

- **证据（事实）**：`aproxy-envelope/src/lib.rs:29-114`；`transform.rs:145-147` 自述的残余串包窗口；`proxy.rs:1476-1486`。
- **建议**（逐项）：
  1. **协议版本 `v`**：输入侧恒写 1。IPC 已经有 `v`，信封没有，format 无从判断自己面对的是哪一代契约。
  2. **往返序号回显**：输入加 `seq`（每次往返单调递增），format 回信时应原样带回。aProxy 发现不一致就按 Protocol 错误剔除 worker，这样能根治 `transform.rs:145-147` 自述的串包残余窗口。回信不带 seq 时照旧接受，兼容第三方 format；aproxy-format 必须回显。
  3. **响应侧补 `status`**：输入是上游状态码，输出缺省表示不变。现在响应转换器既看不到状态码，也改不了。
  4. **`RetryInfo` 加机读原因 `reason`**：取值如 network、status、error_body、stream_error，与 DP-03 共用一套码；`error` 改成英文。现在是中文「上游返回 {status}」（`proxy.rs:1476-1486`），200 带错误 JSON 与 429 在 format 看来只差一个数字。
  5. **`EnvelopeError` 的 Display 改英文**（`lib.rs:127-137`）：它经 `TransformError::Protocol` 进入 502 体与 last_error。
  6. **定死输出侧的缺省语义**：现在 `headers` 必填且整表替换，`body` 与 `body_b64` 都缺时等于空 body（`lib.rs:39-49`）。只想换 key 的 format 也得把可能有几 MB 的 body 原样写回。要把「缺省 = 不变」定为契约，只能趁现在，以后再改就是语义破坏。
  7. **`#[non_exhaustive]` 加赋值式构造**：今后加字段不再迫使 crate 升次版本，这正是记忆 transform-format 里「改信封公开 API 必须升版本」的根因。aproxy-format 里的结构体字面量（`aproxy-format/src/main.rs:41、300、363`）改为 `default()` 后逐字段赋值。
  8. **`worker_id`**：文档写「池槽位号 0..pool_max」「各 worker 稳定不同」，实现却是取模自增（`transform.rs:434-447`），worker 剔除后重生会出现重复。二选一：改实现为分配最小的空闲槽位，或把契约改成「近似均匀的提示值」。上一轮 transform-10 只修了文档的一半。
- **成本**：M（含 aproxy-format 与 skill `protocol.md` 的同步）。

### DP-14 ［P1｜契约］`spool_limit_mb = 0` 被当成 1 MiB，与其余字段的 0 语义相反；`*_mb` 实为 MiB

- **证据（事实）**：
  - `spool_limit_bytes` 对 0 取 `.max(1)`（`proxy.rs:3085-3087`），skill 也照此记载（`config-toml.md:97`：「0 is treated as 1」）。
  - 其余字段的 0 都表示不限或永不：`max_body_mb = 0` 不限（`config.rs:387-396`）；`read_timeout_secs`、`connect_timeout_secs`、转换器的 `timeout_secs` 与 `idle_timeout_secs` 同样如此。
  - 两个 `*_mb` 字段都按 1024×1024 换算，名字却叫 MB。
- **建议**：0.2.0 统一为「0 = 不限」，或者让 validate 拒绝 0 并说明原因；文档写 MiB。把字段改名为 `_mib` 的收益抵不上破坏，不建议。
- **成本**：S。

### DP-15 ［P1｜正确性］动态心跳任务持有响应通道的 Sender，回放结束后客户端的流要等在途心跳调用返回才结束

- **证据（事实）**：
  - `KeepaliveSink::heartbeat` 把 `tx.clone()` 交给 `spawn_dynamic_heartbeat`（`proxy.rs:2268-2278、2129-2166`）。任务在等待心跳转换期间一直持有这个 Sender。
  - 响应体是 `ReceiverStream`（2209），所有 Sender 都 drop 之后流才会结束。
  - 保活任务回放完毕 return 时，只 drop 了自己那一份（2996）。回放闸门只阻止写入，并不释放 Sender（2157-2162）。
- **后果（推断）**：
  - 回放结束时如果恰好有一拍心跳转换还在途，客户端在收到完整响应之后，还要等这次调用返回才会看到流结束。等待上限是 `timeout_secs`，默认 30 秒；配成 0 且 format 卡住时没有上限。
  - SDK 的流迭代在 EOF 之前不会结束，agent 的这一轮会随之挂起。
  - heartbeat_transform 是 2026-10-06 的新功能，还没有随正式版发布。
- **建议**：任务里改持 `tx.downgrade()`（tokio mpsc 的 `WeakSender`），写之前再 upgrade。补测试：慢心跳 format 加快速成功的上游，断言回放后流立即结束。
- **成本**：S。

### DP-16 ［P2｜正确性］aproxy-format 按「有无 method」区分请求与响应，不看 stage

- **证据（事实）**：`aproxy-format/src/main.rs:189-199`；心跳信封带 method（`src/transform.rs:705`）。
- **后果（事实加推断）**：
  - 把 aproxy-format 配成 heartbeat_transform 时，心跳信封会被当成请求处理。
  - 首拍会走路由，`pick_key` 计数前进，扰乱 key 轮换。回信是一段请求 JSON，不符合心跳边界规则，被丢弃并改发固定心跳（`transform.rs:721-724`）。
  - 之后各拍信封没有 body，报「body 缺少 model 字段」。
  - retry 阶段被当成请求处理，只是碰巧正确。
- **建议**：按 `stage` 分派：request 与 retry 走请求侧，response 走响应侧；heartbeat 与未知阶段回 `{"headers":{}}`（空 body）或 error。stage 缺省时才退回按 method 判断。随 format 线发版。
- **成本**：S。

### DP-17 ［P2｜性能］错误扫描对每个 SSE data 行做完整 JSON 解析，内存模式扫两遍，都在 async 线程上（上一轮 proxy-core-10、perf-04，未修）

- **证据（事实）**：
  - forward_once 对每个 chunk 无条件调用 `scanner.feed`（`proxy.rs:3171`）。
  - scan_line 对每个 data 行调 `retry::is_error_body`，后者做 `serde_json::from_slice::<Value>`（`proxy.rs:939-958`、`retry.rs:64-82`）。
  - 内存模式的 finish 丢弃扫描结论（`proxy.rs:832`），之后 `should_retry_response` 再对整个 body 做解码、`is_streaming_response` 与 `is_stream_error_body`（1789-1810）。
  - 这些都在 tokio worker 上同步执行，其中包括最多 8 MiB 的解压（`decode.rs:70`）。
- **建议**：
  - (a) 解析前先用 memchr 预过滤：错误行必然包含字节串 `error`，绝大多数 delta 行可以不解析直接跳过。
  - (b) 内存模式不再 feed 扫描器，或者两种模式统一只用扫描器结论。
  - (c) 超过 256 KiB 的解码与整体判定放进 spawn_blocking。
  - 改动前后用 `examples/loadgen` 量化。
- **成本**：S–M。

### DP-18 ［P2｜性能/正确性］StreamErrorScanner 逐行 drain，单块内 O(n²)；超长块先截头再分行

- **证据（事实）**：`proxy.rs:915-937`。`self.tail.drain(..=pos).collect()` 每处理一行就搬移一次剩余字节。`tail.len() > SCAN_MAX_LINE` 时在分行之前就截掉头部（921-924），一个大于 512 KiB 的 chunk 会连同其中完整的行一起被丢弃。推断：hyper 单次读最多约 400 KiB，常态下触发不到。
- **建议**：用游标逐行扫描，最后一次性 drain；长度上限只施加在「未成行的尾巴」上。
- **成本**：S。

### DP-19 ［P2｜性能］磁盘 spool 每块一次阻塞池往返；回放每块双分配（上一轮 perf-09，未修）

- **证据（事实）**：溢写之后每块都单独 `file.write_all(chunk).await`（`proxy.rs:1023-1031`），SSE 的几百字节小块在超过 1 MiB 后会产生成千上万次 spawn_blocking。回放时 `vec![0u8; want]` 之后再 `Bytes::copy_from_slice`（1169-1185）。
- **建议**：写句柄外包一层 tokio `BufWriter`（64–256 KiB），finish 之前 flush，perf-01 的保证不变。回放改为用 BytesMut 读，读完 freeze。
- **成本**：S。

### DP-20 ［P2｜正确性］every_attempt 重新转换后，心跳转换仍拿着首次转换的请求

- **证据（事实）**：`HeartbeatSource` 只在首次转换后构造一次（`proxy.rs:2723-2733`）。重试前的重新转换替换了 method、url、headers 与 body（2788-2798），却没有更新它。磁盘上的请求体换新后旧文件被删，首拍心跳若晚于这次替换，读文件会失败（`transform.rs:697-699`）。
- **建议**：HeartbeatSource 放进可替换的 `Arc<Mutex<…>>`，或随 `note_attempt` 一起更新。
- **成本**：S。

### DP-21 ［P2｜正确性］保活通道「未提交 + 上游 2xx 非 SSE」时，响应转换不经 drive

- **证据（事实）**：`proxy.rs:2892-2893` 直接 `transform.await`。客户端在这段时间断开没有人察觉。`timeout_secs = 0`（不限）的 format 卡住时，推断后台任务与池 permit 会被永久占用。
- **建议**：这一分支也经 drive 驱动，只是 tick 时不提交骨架，道理与 drive 里的 `skeleton_paused` 相同。
- **成本**：S。

### DP-22 ［P2｜正确性］响应转换解码失败时把压缩字节交给 format，输出却剔除了 content-encoding

- **证据（事实）**：`transform.rs:843-846` 解不出时直接喂原始字节；`883-885` 输出一律删掉 content-encoding。触发条件是未知编码，或解码后超过 8 MiB（`decode.rs:70`）。原样回传 body 的 format 会让客户端收到一份没有编码标注的压缩体。
- **建议**：解不出时跳过转换、原样透传，与「响应侧失败透传」的语义一致；或保留 content-encoding 头。
- **成本**：S。

### DP-23 ［P2｜正确性］客户端压缩上传（请求带 content-encoding）时 body_stream 判定恒假，转换器拿到压缩字节

- **证据（事实）**：`request_body_wants_stream` 直接解析原始字节，不看请求的 content-encoding（`proxy.rs:728-745`）；transform_request 同样把原始字节装进信封（`transform.rs:747-755`）。哪些 agent 客户端会压缩请求体未核实，属推断。
- **建议**：判定时用 `decode::for_inspection` 解出一份副本；转换前要么同样解出，要么拒绝转换并告警。
- **成本**：S。

### DP-24 ［P2｜正确性/文档］本地磁盘故障的归因、注释与文档

- **证据（事实）**：
  - 请求体写盘失败回 400，且不记 last_error（`proxy.rs:1296-1303`）。上一轮 proxy-core-11 只改了文案。
  - SpoolFailed 在三处都是终态（1379-1383、2013-2017、2855-2867），但 `ForwardResult` 的注释写「交由调用方选择终态或继续」（3066-3069），实际上没有任何调用方选择继续（上一轮 perf-07）。
  - `docs/architecture.md:88-89` 说写盘失败「绝不退化成内存堆积」，代码却恰好在建文件失败或首次写失败时退化为内存（`proxy.rs:987-1021、549-571`，上一轮 proxy-core-08）。
- **建议**：本地故障统一用 `aproxy_local_storage_failed`（500 或 507）并记 last_error；文档以代码为准改写；注释与行为对齐。
- **成本**：S。

### DP-25 ［P2｜正确性］协议无关的错误判定有已知盲区

- **证据**：
  - （事实）`is_error_body` 只认顶层的 `error` 非 null，或 `type == "error"`（`retry.rs:64-82`）。
  - （推断，依据 OpenAI 公开协议）OpenAI Responses 的 `response.failed` 事件把错误放在 `response.error` 里，不会被判为失败。
  - （事实，记忆 disconnect-billing）既无 content-length 也无 chunked 的上游提前关连接是合法 EOF，截断的流会被当成功。
- **建议**：不内置协议特例。放进 DP-04 的规则表，例如用 `body` 正则匹配 data 行，或配置「必须出现的结束标记」正则，由用户按客户端协议配置。
- **成本**：M（随 DP-04）。

### DP-26 ［P2｜正确性］退避不读 Retry-After、没有抖动、前 3 次零延迟（上一轮 proxy-core-04，未修）

- **证据（事实）**：`retry.rs:14-32`；两条循环直接调用 `delay_for_attempt`（`proxy.rs:1970、2753`）。
- **建议**：随 DP-04 的 `[retry]` 设计实现：
  - Retry-After 作为退避下限，并钳制到 max_backoff，不因此放弃重试。
  - 加 full 或 decorrelated 抖动。
  - 响应带 Retry-After 时跳过前三次零延迟重试。
- **成本**：S。

### DP-27 ［P2｜正确性］优雅停止 10 秒宽限后 process::exit，已提交的保活流被截断且没有终态事件

- **证据（事实）**：`server.rs:321-335`；保活任务是脱离的 spawn（`proxy.rs:2662`），不受 graceful shutdown 管理。
- **建议**：收到停止信号时广播给在途的保活任务。已提交的先写一个 `aproxy_shutting_down` 终态事件再结束（与 DP-02 同形），未提交的回 503 加同一个码。宽限时长要不要随在途数自适应，留给 install 的排空设计（上一轮 install-09）。
- **成本**：S–M。

### DP-28 ［P2｜卫生/性能］小项（证据均为事实）

- 下游连接没开 `tcp_nodelay`（`server.rs:325`，上一轮 perf-03）。
- `tower` 与 `tower-http` 依赖没有人用（`Cargo.toml:46-47`，perf-13）。
- `is_hop_header` 每次都 `to_ascii_lowercase` 分配一次，而 `HeaderName` 本来就是小写（`proxy.rs:91-93`）。
- forward_once 与 forward_only 逐个头 `from_bytes`，在同一个 http 类型之间来回转换（3110-3120、3271-3281）。`reqwest::Method::from_bytes(...).unwrap_or(GET)` 的回退不可达（3104-3105）。
- HEAD 与 304 的 content-length 被改写（3450-3456、3488-3491，proxy-core-06）。
- `retry.rs:275` 的测试注释仍写「min(6) 封顶」，已经不成立。
- heartbeat_transform 默认 `timeout_secs = 30`，大于默认保活间隔 15（`config.rs:63-65、402-404`）。
- **成本**：S。

### DP-29 ［P2｜结构］转换往返先写后读、body 多次物化（上一轮 transform-06、transform-11，未修）

- **证据（事实）**：
  - `transform.rs:312-330` 先写完再读。边读边写的 format 在 body 超过管道缓冲时会死锁，直到超时。
  - 请求体至少拷贝三份：`transform.rs:747-755` 加信封的 `lib.rs:157-168`。
  - every_attempt 会把磁盘上的请求体整份读进内存，并在整个请求生命期内常驻（`proxy.rs:1566-1586`）。
- **建议**：读写并发（join）；body 改为消费式取用；文档写明请求侧的内存峰值。
- **成本**：S–M。

### DP-30 ［P2｜i18n］87 处日志与 3 处前台横幅

- 分类见第 6 节。建议与 DP-01/02/03 同一批改：日志英文化并带 `code=` 字段；skill 排障改为按码检索。
- **成本**：M。

---

## 4. 已定决策存疑（单列，不作缺陷）

- **X-1　请求侧转换失败一律 502 不重试（transform-format 决策 #1）**。every_attempt 落地后，同一功能内部出现了不对称：
  - 首次转换遇到进程层瞬时故障（Spawn、Io、WorkerDied、TimedOut）时，请求直接 502 终止（`proxy.rs:1613-1618`）。
  - 重试前的转换遇到同类故障时，却被容忍并沿用上一次的请求（`proxy.rs:1519-1529`）。
  - 建议：进程层瞬时类进入既有的重试循环（带退避、计入重试数）；Rejected 与 Protocol 维持 502。
- **X-2　aProxy 自产的 502 会触发客户端 SDK 自己的重试**。「不重试」只在 aProxy 内部成立。推断：SDK 普遍对 5xx 自动重试，Anthropic 与 OpenAI SDK 默认 2 次，Claude Code 更多。format 的 Rejected 属于请求本身的确定性问题，比如「model 未命中任何渠道」（`aproxy-format/src/main.rs:218`）。建议 Rejected 用 4xx（400 或 422）加 `aproxy_transform_rejected`，进程层故障才用 502 或 503。和 DP-01 一起定。
- **X-3　错误响应不进 format**（受限透传与终态错误）。跨协议转换时，受限透传给客户端的是上游协议形状的错误体，比如 Anthropic 客户端收到 OpenAI 形状的 404，推断 SDK 可能解析不出 message。建议保持默认，另允许 `response_transform` 显式开启对错误体的转换。
- **X-4　4xx 一律无限重试**。默认值维持不变。但 400（上下文超长）、401（key 失效）、404（模型名错）在默认配置下会让 agent 永久等待。出口应由 DP-04 的规则表提供，而不是改默认值，这与上一轮的结论一致。

---

## 5. 上一轮审查相关条目现状（`.agents/review/2026-10-04-0.1.0发布前-13维审查.md`）

| 条目 | 状态 | 证据 / 说明 |
|---|---|---|
| proxy-core-01 首轮 spool 完才写头 | 部分已修 | 保活适用的请求（Accept SSE 或 stream:true，默认 any）首轮就由 drive 驱动：2xx 未压缩 SSE 头先行提交，否则一个间隔后提交骨架（`proxy.rs:1331-1340、2442-2504、2803-2831`；测试 `keepalive_forwards_upstream_sse_head_immediately` tests/proxy_integration.rs:2117）。stream:false 的请求仍是「首字节 = 完整生成」，属设计取舍（`default_mode_buffers_whole_response_before_first_byte`:3389）。body 仍整体缓冲，事件级空闲超时靠客户端配置（README.md:105-131） |
| proxy-core-02 上游在途不发心跳 | 已修 | drive 在 forward_once 期间 select ticker（`proxy.rs:2442-2504`）；测试 `keepalive_heartbeat_gaps_are_bounded_while_upstream_in_flight`:2261 |
| proxy-core-03 只凭 Accept 判定 | 已修 | `keepalive_trigger`（`config.rs:75-110`，`proxy.rs:751-769`）；对磁盘请求体也流式判定 |
| proxy-core-04 Retry-After / 抖动 | 未修 | `retry.rs:14-32`（→ DP-26） |
| proxy-core-05 确定性 4xx 无限重试 | 未修 | 只有按路径的 bounded（`retry.rs:39-41`）（→ DP-04） |
| proxy-core-06 HEAD 的 content-length | 未修 | `proxy.rs:3450-3456`（→ DP-28） |
| proxy-core-07 Upgrade 降级 | 未修 | `proxy.rs:78-89`（→ DP-06） |
| proxy-core-08 文档称不退化到内存 | 未修 | `docs/architecture.md:88-89` 与 `proxy.rs:987-1021` 不符（→ DP-24） |
| proxy-core-09 断开计费保护的测试 | 部分 | 保活的已提交与未提交、非保活 spool 中途断开都有测试（`client_disconnect_before/after_commit_*`、`spool_file_removed_when_client_disconnects_mid_spool` 覆盖 stream:false）。非保活重试循环中断开（恒 429 加 Accept json）后上游计数停止，仍无测试 |
| proxy-core-10 内存模式双扫 | 未修 | `proxy.rs:3171、832、1789-1810`（→ DP-17） |
| proxy-core-11 写盘失败回 400 | 部分 | 文案已注明可能是本地磁盘；状态码仍为 400，也不记 last_error（`proxy.rs:1296-1303`） |
| perf-01 spool 收尾 flush | 已修 | `proxy.rs:838-850`；单测 `spool_finish_surfaces_error_of_last_write`（proxy.rs:3864） |
| perf-03 下游 tcp_nodelay | 未修 | `server.rs:325` |
| perf-04 扫描 CPU | 未修 | 同 proxy-core-10 |
| perf-06 TTFB | 部分 | 同 proxy-core-01 |
| perf-07 SpoolFailed 终态 | 未改 | 三处终态，注释与行为不符（→ DP-24） |
| perf-08 Content-Length 预检 | 未修 | `proxy.rs:535-539`（413 前先读完剩余请求体，已于 5148d80 改为 lingering close） |
| perf-09 磁盘 IO 细节 | 未修 | `proxy.rs:1169-1185、1023-1031、632、791`（→ DP-19） |
| perf-10 全局 spool 预算 | 未修 | 无 |
| perf-12 日志里的 URL 打码 | 已修 | `proxy.rs:1281-1284、1641-1651、3201-3212` |
| perf-13 无用依赖 tower | 未修 | `Cargo.toml:46-47` |
| transform-01 串包 | 已修 | `transform.rs:369-416`；测试 `request_persistent_banner_fails_loudly_and_never_crosswires` 等 |
| transform-02 / features-01 重试时重新路由 | 已修（默认关） | every_attempt（`proxy.rs:1463-1532、1979-1986、2772-2801`）；测试 `every_attempt_*`（transform_integration.rs:832 起） |
| transform-03 auto 模式不反向转换 | 已修 | 请求侧拒绝跨协议的 auto（`aproxy-format/src/main.rs:229-242`） |
| transform-05 死 worker 直接 502 | 已修 | `transform.rs:220-247、256-287`；测试 `request_persistent_idle_worker_death_respawns` 等 |
| transform-06 先写后读 | 未修 | `transform.rs:312-330`（→ DP-29） |
| transform-07 README 示例缺 extra | 已修 | `README.md:226-233` |
| transform-09 「确定性」文案 | 文案已修 | `transform.rs:45-108`；语义按决策保留（见 X-1） |
| transform-10 worker_id | 部分 | 文档已改为「官方不使用」（skill `examples.md:306`）；取模重复仍在（`transform.rs:434-447`，→ DP-13 第 8 项） |
| transform-11 body 多次物化 | 未修 | `transform.rs:747-755`（→ DP-29） |
| features-02 客户端超时指引 | 已修 | `README.md:105-131` |
| security-01 Host/Origin（位于 proxy.rs） | 已修 | `proxy.rs:148-322、1245-1251`；测试 `inbound_*`（tests/proxy_integration.rs:4171 起） |
| 路线图 0.1.x #2 首轮流式提交与在途心跳 | 已修（附前述限定） | 同 proxy-core-01/02 |
| 路线图 0.1.x #5 转换器重新路由 | 已修 | 同 transform-02 |
| 路线图 0.1.x #6 Retry-After 与抖动 | 未修 | → DP-04/26 |
| 路线图 0.1.x #7 确定性错误出口 | 未修 | → DP-04 |
| 路线图 0.1.x #16 aproxy-format 能力补全 | 大部分未修 | transform-03 已修；06、10（实现）、11 未修；08（Windows .cmd）的文档说明这次没核对 |
| 路线图 0.1.x #17 小清理 | 大部分未修 | perf-03/13、proxy-core-06/07/08/10/11、perf-04/07–10 见上 |
| 路线图 0.2.0 #3 可观测数据面 | 未开始 | → DP-12，第 8 节 |
| 路线图 0.2.0 #6 混合流式模式 | 未开始 | 真实头先行提交只是部分替代：先发头，不先发体。长生成仍受客户端事件级空闲超时约束 |
| 路线图 0.2.0 #7 结构化错误码与 i18n | 未开始 | SSE 事件的 type 是唯一半结构化的出口（→ DP-01/02/03/30） |
| 路线图 0.2.0 #9 运营类增强 | 未开始 | 依赖第 8 节的数据面 |

---

## 6. i18n 分类

**方法**：脚本抽取非测试代码里含中文的字符串字面量（按调用点计数，同一文案出现多次就计多次），再逐条人工判定去向。
范围内 aproxy 主二进制（proxy.rs、retry.rs、decode.rs、server.rs、transform.rs）加信封 crate 共 **167** 处；
retry.rs 的生产代码里没有中文字面量。aproxy-format 另计 **32** 处。

| 类别 | 处置 | aproxy 主二进制 + 信封 | aproxy-format |
|---|---|---|---|
| 日志（tracing 字段与消息、`during` 标签、预览文案） | 固定英文 | **87**（proxy 58 / server 13 / transform 16） | 0 |
| 只发给 HTTP 客户端的响应体（403/400/413/502 文案） | 固定英文 + 结构化（DP-01） | **10** | — |
| 只进 IPC `last_error`，经 `status` 与 UI 显示给用户 | 线上改结构化与英文，出口处本地化（DP-03） | **17** | — |
| 只进信封、发给 format（`RetryInfo.error`） | 固定英文 + 机读原因（DP-13） | **1** | — |
| **多去向混用**（同一字符串同时进入以上多个出口） | 拆成「错误码 + 参数」，在各出口渲染 | **39** | **24**（20 条 error 行原因 + 4 条配置校验，都会变成信封 error，再进 aProxy 的 502/SSE/last_error/日志） |
| 给用户看的 CLI 输出 | 随系统语言 | **3**（`server.rs:315-318` 前台横幅） | **7**（用法与 `die` 提示） |
| 内部文本（expect/unreachable、serde expecting、错误链里被丢弃的文本） | 英文即可，不翻译 | **10** | **1** |
| 合计 | | **167** | **32** |

另：`config.rs:30-46` 的 `heartbeat_problem` 有 3 条文案，同时用于 validate（用户在 CLI 看到）与心跳转换失败日志，属混用，不在上面的计数内。

**混用点**（都需要改成「错误码 + 参数」并在出口渲染）：

- **M1** `IpcStats::record_error` 的约 20 个中文调用点 → IPC `LastError.message`（线上） → `status` 逐字打印（用户），将来还会进 UI（`proxy.rs:432-434`；`daemon.rs:213-219、501-517`；`commands/status.rs:63-74`）。
- **M2** `TransformError` 的 Display（9 个变体，加上 Protocol 细节 3 条、`EnvelopeError` 3 条）→ 502 体、SSE `proxy_transform_failed` 的 message、last_error、日志，四处共用一份文案（`transform.rs:79-108、373-412`；`aproxy-envelope/src/lib.rs:127-137`）。
- **M3** `ForwardResult::NetworkError/SpoolFailed(String)` 与解码错误（`decode.rs:177-245`）→ 502 体、last_error、`RetryInfo.error`、日志。网络错误还是「中文前缀 + reqwest 英文原因」的混语言串（`proxy.rs:3179-3182`）。
- **M4** `bind_error_message` 与控制通道创建失败 → `report_config_error` → stderr（用户）与 startup.log（日志）同一字符串（`server.rs:105-112、353-364、469-489`）。
- **M5** `heartbeat_problem` → validate（用户）与心跳转换失败日志（`config.rs:30-46`；`transform.rs:721-724`）。
- **M6** aproxy-format 的 error 行原因 → 信封 `error` → aProxy 的 502、SSE、last_error、日志（`aproxy-format/src/main.rs:81-248、328-388`）。
- **M7** `RetryInfo.error` → 第三方 format，可能被它记日志或显示（`proxy.rs:1476-1486`）。
- **M8** skill 排障表逐字引用中文原文（见 DP-01 的证据）。英文化与码表必须和 skill 同一次提交，否则 skill 立刻失真。

---

## 7. proxy.rs 拆分与循环统一建议

**目标结构**：`src/proxy/` 目录模块，`mod.rs` 只做再导出。对外保留 `AppState`、`router`、`RequestBody`、`SpooledBody`、`RESIDENT_LIMIT`、`is_hop_header`、`unique_seq` 这些 `transform.rs` 等处在用的名字。

| 新模块 | 来自现 proxy.rs（行） | 约行数 | 职责 |
|---|---|---|---|
| `state.rs` | 118-147, 324-447 | 155 | AppState 构造、client、池、计数（将来的 RequestRegistry 也放这里） |
| `inbound.rs` | 148-322 | 175 | Host/Origin 策略与 403（渲染改走 errors） |
| `headers.rs` | 76-116, 1196-1238 | 90 | hop 过滤、api_key/override/extra 注入 |
| `body.rs` | 456-635 | 180 | RequestBody、read_request_body、drain |
| `trigger.rs` | 637-769 | 135 | StreamFlagProbe、keepalive_triggered |
| `spool.rs` | 771-882, 984-1189, 2506-2613 | 425 | SpoolPath、SpoolBuffer、spool_chunk、SpooledBody、DiskSpoolStream、SpillWriter、committed 解码 |
| `inspect.rs`（或并入 retry.rs） | 884-982, 1749-1930 | 280 | StreamErrorScanner、needs_retry_response、预览；DP-17/18 在这里改 |
| `upstream.rs` | 3058-3223 | 165 | ForwardResult、forward_once、URL 拼接、reqwest 错误分类（DP-05） |
| `forward_only.rs` | 3246-3448 | 205 | 仅转发路径（不参与循环统一） |
| `replay.rs` | 1671-1747, 2615-2629, 3450-3520 | 165 | 保真回放、响应转换入口 |
| `transform_glue.rs` | 1442-1620 | 180 | OutboundRequest、PerAttemptTransform、apply_request_transform |
| `keepalive.rs` | 2062-2504, 3046-3056 | 455 | KeepaliveSink、Heartbeat、drive、提交规则、断开哨兵 |
| `attempts.rs` | 1342-1440, 1932-2060, 2631-3044 | 现 640 → 统一后预计约 350 | 唯一的 attempt 循环 |
| `errors.rs`（新） | 293-322, 1622-1669, 3225-3244 等 | 新 | FailureReason/ProxyError 与各出口的渲染（DP-01/02/03） |
| `handler.rs` | 1191-1194, 1240-1341 | 105 | 入口：入站校验 → 计数 → forward_only 分叉 → 读 body → 选 Responder |

**循环统一草图**：

```rust
enum Outcome { Success(Resp), Retry(FailureReason), Exhausted(Resp), Fatal(ProxyError) }
fn classify(attempt: u32, r: ForwardResult, bounded: bool) -> Outcome;   // 纯函数，单测覆盖

trait Responder {                       // 或 enum { Buffered, Keepalive(KeepaliveSink) }
    async fn wait<F: Future>(&mut self, f: F, head: Option<HeadRx>, during: Phase) -> Option<F::Output>;
    fn forces_identity(&self) -> bool;  // Keepalive = true
    async fn finish_success(self, ...); async fn finish_passthrough(self, ...); async fn finish_error(self, ProxyError);
}

async fn run(state, req, mut r: impl Responder) {
    for attempt in 1.. {
        if attempt > 1 { /* tracker.note_retry；backoff 经 r.wait；every_attempt 重转换经 r.wait */ }
        let res = r.wait(forward_once(..), head_rx, Phase::Upstream).await?;   // None = 客户端已断开
        match classify(attempt, res, bounded) { /* 唯一一份 last_error、事件、日志与终态出口 */ }
    }
}
```

- Buffered 的 `wait` 就是普通 await，取消依靠 hyper drop handler future，现有计费保护的第 3 层不变。Keepalive 的 `wait` 就是现有的 `drive`。
- handler 对 Buffered 内联调用 `run`，对 Keepalive 则 `spawn(run)` 后等待 oneshot，结构与现状一致。
- 首轮快速路径就是第 1 次迭代：未提交时成功即走保真回放，这对两种 Responder 本来就成立。
- **执行顺序**：
  1. 纯搬移拆模块。逐个模块 `git mv` 式迁移，每步全量测试，行为零变化。
  2. 抽出 `classify` 及其单测，让三处都改为调用它。
  3. 合并两条循环。
  4. 接入 FailureReason（DP-03）与埋点（第 8 节）。
- 注意：`tests/proxy_integration.rs`（7091 行）是唯一的行为基线，拆分期间不改测试；测试数要按记忆 shared-target-dir-stale-tests 核对。

---

## 8. UI 就绪度：埋点位置与热路径开销

**现状（事实）**：可观测数据只有 `requests_total`、`retries_total`、`last_error`（字符串）和 `last_activity_secs`（`daemon.rs:487-499`），IPC 只有 ping、shutdown、prepare_swap 三个 op。没有在途请求、没有分原因的重试计数、没有事件流，日志也没有请求标识（DP-11）。

**建议的数据模型**（放在 `state.rs`）：

- `RequestRegistry`：`Mutex<HashMap<u64, Arc<RequestSlot>>>`，按 request_id 索引。
  - `RequestSlot` 的不变部分：method、脱敏 path、started_at、mode（buffered / keepalive / forward_only）。
  - 可变部分用原子量：phase（读 body、转换、等上游、spool、退避、回放）、attempt、committed（无、真实头、骨架）、next_retry_at、spooled_bytes。
  - 每次失败时，在 `Mutex<Option<FailureReason>>` 里记最近一次失败。
- `InFlightGuard`（RAII）：插入在入站校验之后，Drop 时移除，这样 hyper drop、取消和 panic 都能覆盖。
  - 保活路径：guard 必须 move 进 spawn 的任务，因为提交之后 handler 已经返回。
  - forward_only：guard 要 move 进响应流的 map 闭包，在途时长才能覆盖整个流。
- 分原因计数：`[AtomicU64; FailureCode::COUNT]`。
- 事件：`broadcast::Sender<Arc<ProxyEvent>>`（容量约 1024）。
  - 发送前先检查 `receiver_count() > 0`，没有订阅者时连事件对象都不构造。
  - 订阅者落后（Lagged）时，用快照 op 重新同步。
  - 事件集合：RequestStarted、AttemptStarted、AttemptFailed{reason}、BackoffStarted{delay}、Committed{kind}、ClientGone、RequestFinished{outcome, attempts, duration}。
  - 事件 schema 本身就是契约，应与 IPC 的 `v` 一起版本化（IPC 与 crate 边界见其他维度的报告）。

**埋点位置**（现有代码）：

| 事件 / 数据 | 位置 |
|---|---|
| 请求开始、计数（建 guard 与 span） | `proxy.rs:1253-1269`（入站校验 1249-1251 之后） |
| forward_only 分叉与流式字节 | 1280-1286；流 map 3413-3440 |
| 读 body 完成或失败 | 1291-1304 |
| 保活判定（mode） | 1311-1314 |
| 请求转换开始与结束 | 1349 / 2683-2711 |
| 每次尝试开始 | 首轮 1357；非保活 1988；保活 2811（统一后只剩一处） |
| 上游响应头到达 | `forward_once` 3155-3157（已有 on_head 通道） |
| spool 字节 | 3164-3185（每 chunk 一次 Relaxed fetch_add） |
| 尝试失败（带原因） | 1368-1397 / 2002-2057 / 2836-3031（统一后只剩 classify 之后一处） |
| 退避开始（带 delay） | 1970-1976 / 2753-2769 |
| 提交（真实头或骨架） | `KeepaliveSink::commit` 2192-2234 / `commit_skeleton` 2239-2250 |
| 客户端断开 | `log_client_gone` 2354-2364 |
| 终态（成功、透传、错误） | `replay_success` 1679、`replay_failure_as_is` 2618、各终态出口 |
| IPC 快照组装 | `daemon.rs:1310-1342` `status_snapshot` |

**开销估计（推断，需要用 `examples/loadgen` 实测）**：

- 每个请求：一次 Mutex 插入与删除（约百纳秒级）加十余次原子操作。
- 每次尝试：几次原子 store；失败时多一次 Mutex 写。
- 每个 chunk：一次 Relaxed fetch_add（spooled_bytes）。
- 没有订阅者时每个事件点是一次原子 load。

与网络 IO 和现有的逐行 JSON 解析（DP-17）相比可以忽略。在途数和每请求状态让 idle 判定（DP-12）、install 排空与 UI 共用同一个数据源。

---

## 9. 证据边界与未核实项

- 所有「推断」都没有运行验证，包括：
  - 客户端对各种错误体的解析与显示（DP-01、X-2/X-3）。
  - Codex 的 WebSocket 行为（DP-06）。
  - 上游对被掐断请求的计费（DP-07）。
  - 端口 0 场景下 start 预检的实际行为（DP-08）。
  - 心跳 Sender 延迟流结束的实际时长（DP-15）。
  - 哪些客户端会压缩请求体（DP-23）。
  - OpenAI `response.failed` 的形状（DP-25）。
  - 埋点开销（第 8 节）。
- 没有运行任何测试。文中提到的测试名只证明测试存在，不代表当前通过。
- 没有核对的部分：
  - `aproxy-format` 的 aggregate、config、detect、translate 只看了 grep 命中处。
  - transform-08（Windows .cmd）的文档。
  - perf-05（opt-level 的 A/B 数据）。
  - perf-11（IPC 读超时，属 daemon 范围）。
- i18n 计数基于脚本加人工判定。如果同一文案在多处出现，按调用点重复计数。
