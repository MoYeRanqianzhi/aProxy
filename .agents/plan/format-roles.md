# 官方 aproxy-format 的各个角色

W2c 的设计稿：`aproxy-format <角色>` 每个子命令做什么、要哪些阶段与请求体、配置长什么样、状态放在哪里。
协议一侧的契约在 [w2-contracts](w2-contracts.md) 的 §10（各角色用到的阶段见 §10.14）；这里只写角色自己的行为。
由 Codex 在 t06 第五段起草（2026-11-20），主代理改写成中文；实现分在 t06 的第五到第九段。

**现状（2026-11-20，第六段做完）**：各角色的协议 1 入口，三种协议之间请求、响应与流式的转换，Messages 的原生心跳，
基本的 key 轮换与按模型路由已经实现。下面写到的冷却、粘性、决策与工具子命令属于之后几段，起步的骨架还不都接受这些配置。

## 共同约定

- 一个子命令只做一个角色。重试、传输与投递归 aproxy，角色不重复实现。
- 角色读一行协议 1 的 hello，回它支持的阶段与各阶段要多少请求体；之后每收一帧回一帧，带上 `v` 与 `seq`，直到 EOF。
  `[[format]]` 里配的 `stages` 是从它声明的阶段里选，不是让 aproxy 调别的阶段的许可。
  只有双方都声明了 `decide.success`，成功的尝试才会来问。类型用 `aproxy-envelope` 的协议 1 类型。
- 给本机看的（命令行输出、stderr 的诊断）用中文；写进协议的 `fail.reason` 与 `report.summary` 用英文，因为读它们的是别的程序。
- 诊断不上 stdout。报错里不引用坏掉的帧、TOML 的原文行、环境变量的值、认证头。
- 一帧必须以换行结束；两帧之间的 EOF 是正常退出，EOF 前的残行不算一帧。读一行有上限（含换行）：hello 64 KiB；
  只要元数据的角色 8 MiB（decide 的预览经 JSON 转义之后也在内）；收完整请求体或响应体的角色 257 MiB。
  后一个是线上字节数的上限，不保证任意转义之后、或 aproxy 不限体大小时都放得下。
- 只回应自己在 hello 里声明过的阶段。
- 配置是严格的 TOML：`--config FILE` 指向这个角色自己的文件（下面每个样例是一个单独的文件，不是 config.toml 里的一节），
  不认识的键报错。样例里的密钥写的是**环境变量的名字**，值由 worker 的环境给出，从不写进文件。
- 状态：
  - 同一个请求的后一个阶段可能落在另一个 worker 上，所以请求内要传的东西放进协议的 `state`，不放进程里按请求编号的表。
    `state` 要小，用稳定的、不是密钥的 ID 作键。
  - 跨请求的状态只在进程里。要它不丢，就配 persistent、`pool_max = 1`、`worker_idle_secs = 0`。
  - worker 没了，丢的只能是偏好与计数；不能因此弄错一个请求的协议，也不能漏出凭据。

## convert

`aproxy-format convert --from anthropic_messages --to openai_chat`，或 `convert --config convert.toml`：

```toml
from = "auto"
to = "openai_chat"
```

- **阶段**：request 与 retry（请求体 `full`）、response、stream。一个条目在 response 与 stream 里只订一个。
  不参与 decide 与 notify。
- **认协议**：`from = "auto"` 时按原始的客户端路径与体逐个请求判断；判断不了就拒绝，不悄悄交付错的形状。
  上游与客户端协议不同时，`to` 必须写明。
- **回程**：认出的客户端协议记在 `state` 里（不塞进体），配置的上游协议是另一半。响应方向转回 `state` 记的协议，
  不从上游的响应体去猜。只订了请求方向的条目不转响应，这是有意的。
- 保留现有的非流式转换与同协议直通。回给客户端的响应补丁里去掉上游的凭据头。报告只写协议名，不写内容。
- **流式**：一条流独占的那个 worker 持有源协议的帧解码器与转换状态，直到 close 或 abort。
  - 管道里的一帧 data 不等于一个 SSE 事件：要处理半行、一帧里的多个事件、各协议自己的事件名与结束方式。
  - 不把整条流攒起来：每个完整的事件立即发出；一个没收完的源事件最多留 8 MiB。转换器留的是各协议的语义状态
    （例如工具参数、Responses 结束快照里要的输出），不是上游的原始字节。
  - 流被截断时按 abort 的规则收尾，不补一个正常的结束。
  - 上游回的是非流式的整体、却进了 stream 订阅时，在整体的边界上用非流式的转换。
- 成功的响应转不了（体坏了、流开不起来）时，回一个新生成的 502：客户端协议的 JSON 错误，不带上游的头，也不带上游的字节。
  `state` 里没有客户端协议时，用通用的嵌套错误对象。这不是重试的表态，重不重试仍由那次尝试的判定决定。
- 上游的非成功响应不走成功响应的转换；要按客户端协议改写错误，用 policy 的 stop 附带响应。

## keys

`aproxy-format keys --config keys.toml`：

```toml
strategy = "weighted" # round_robin、weighted、random
sticky = false
cooldown_status = [401, 403, 429]
cooldown_secs = 30
[[pool]]
origin = "https://relay.example.com"
header = "authorization"
scheme = "Bearer"
keys = [{ id = "a", env = "RELAY_A", weight = 2 }, { id = "b", env = "RELAY_B", weight = 1 }]
```

- **阶段**：request 与 retry，请求体都是 `none`；开了粘性才要 `full`。
- **选哪个池**：按**当前要去的上游**选，不按客户端带来的凭据。比的是解析后的 origin（含实际端口），不是字符串前缀。
  去向不在任何池里就拒绝。
- **怎么改请求**：无关的头不动；一组凭据作为整体替换。不在返回的请求补丁之外轮换或追加认证值。
- **状态与报告**：选中的池与 key 的 ID 记进 `state`；报告里只用 ID 指代 key。
- **冷却**：
  - 重试时记账的是上一次用的 key，不是换了渠道之后新选的那个。
  - 每个 key 留一个单调时钟的「何时可再用」。按权重选不必展开一张与权重成正比的表。
  - 读 `retry.last_response`（上一次尝试的状态与头，体中途断了也有）与 `last_failure`；连不上、或请求在链里就失败时没有头。
    对配置的冷却状态尊重 `Retry-After`，不改 aproxy 自己的退避。
  - **冷却只在重试链真的重跑时发生**。decide 表态 `reroute:false` 时 aproxy 重发的是上次准备好的请求：keys 不被调用，
    不轮换，也记不到那次失败。要让每次失败都记账，就让 policy 表态 `reroute:true`。
  - 可用性、轮换位置与粘性都只在一个进程里：池子大于 1 时，一个 worker 冷却掉的 key 另一个 worker 还会选。
    所以 `init` 给 keys 生成 `pool_max = 1`、persistent、不因闲置回收（与 adaptive 一样）。worker 重启之后没有冷却记录。
- **粘性**：在可用的 key 里按对话的稳定前缀的哈希选；不保留对话内容；让位于冷却。

## route

`aproxy-format route --config route.toml`：

```toml
[aliases]
small = "provider-small"
[[channel]]
id = "primary"
url = "https://relay.example.com/v1/messages"
models = [{ exact = "provider-small" }, { prefix = "team-" }, { regex = "^chat-[0-9]+$" }]
priority = 1
weight = 2
preserve_path = false
credential_env = "PRIMARY_KEY"
auth_header = "x-api-key"
[channel.overrides]
"timeouts.first_byte_secs" = 90
[[rule]]
model = "small"
[rule.overrides]
"retry.max_attempts" = 3
```

- **阶段**：request 与 retry，请求体 `full`。不对 decide 表态，不订 notify。
- **选渠道**：先把模型别名换成真名，再匹配渠道；在优先级最高（数字最小）的可用组里选，同级按权重。
- **重试**：收到 retry 就是重试链真的在重跑，这时用上一次的渠道与 `last_failure` 做故障切换。
  `reroute:false` 时 aproxy 重发准备好的请求，任何角色都不该自己换。
- **换上游时的凭据**：去向变了就去掉 authorization、API key、cookie 与代理凭据，哪怕后面还排着 keys。
  然后用这个渠道自己的凭据，或交给绑定去向的 keys 来填。
- **地址**：用渠道写明的 URL；配了 `preserve_path` 就接上客户端原始路径（`client.path`）的路径部分，
  不是上一个上游地址里已经拼好的路径。
- **查询串**：同一个 origin 之内整段保留。跨 origin 时只带渠道的 `carry_query` 列出的参数名（默认一个都不带）：
  保持它们的顺序与重复值，不覆盖渠道 URL 里写死的参数。
- **覆盖**：只回 hello 里列出的覆盖键。同一个键上，精确匹配原始模型的规则压过所选渠道的默认值。
- **状态与报告**：原始模型与选中的渠道 ID 记进 `state`；报告写路由与模型映射，不写凭据，也不写带 userinfo 的 URL。
- **把别名换回来**：配了别名时，另外提供一个可选的 response 订阅（要缓冲响应）。只有上游回的是映射后的那个模型名时，
  才换回 `state` 里记的原始别名，这样几个别名指向同一个模型时客户端看到的仍是它自己用的那个。
  真正流式的客户端不要订这个阶段。

## policy

`aproxy-format policy --config policy.toml`：

```toml
[[rule]]
status = [429, 503]
decision = "retry"
reroute = true
respect_retry_after = true
[[rule]]
failure = "upstream_error_body"
body_regex = "context length"
decision = "stop"
response = "context_too_long"
```

- **阶段**：只有 decide，**请求体**要 `none`。没有跨请求的状态。
- **规则**：按顺序求值，一条规则里的条件要同时成立；都不命中就不表态。有规则要看成功的响应时才声明 `decide.success`。
- **看响应体**：看的是 decide 给的预览，连同它是否完整的标记。预览被截短时，没匹配上不能说明整个体里没有。
- **重试**：`Retry-After` 从这条消息的响应头里读，不超过协议允许的延迟上限；给不出可用的值时保留 fallback 的延迟。
- **停止并给响应**：按原始的客户端路径（非标准路径时按配置写明的协议）选客户端的协议，不按路由之后的 URL。
  响应是新生成的、安全的说明，不照抄上游的体或凭据。
- **报告**：规则的 ID 与动作。

## adaptive

`aproxy-format adaptive --config adaptive.toml`：

```toml
failure_threshold = 2
max_entries = 4096
ttl_secs = 900
hold_secs = 60
recovery_samples = 10
recovery_success_ratio = 0.9
recovery_latency_ms = 1000
```

- **阶段**：request（`digest`）、decide（`none`）、notify（`digest`）。
- **判断**：没见过的摘要回 forward。反复失败或升级的摘要固定用 retry。还没向客户端写出字节的 forward 失败，
  在 decide 上回 retry；已经在重试的请求上，不去推翻别的重试策略。
- **学习**：用 `request_end` 的结果、是否升级、耗时与流是否完整，在交付**之后**更新历史。
  摘要为 null 不等于空体的哈希。
- **存什么**：只存摘要、有界的计数、上一次的结果与时间。过期的先淘汰，满了淘汰最久没用的。
  回到 forward 之前要过一段保持时间和一个有界的恢复窗口。
- **状态与报告**：阶段之间需要时用 `state` 带摘要；报告写模式的变化，不写内容。
- **推荐配法**：`required = false`、`pool_max = 1`、`worker_idle_secs = 0`，实例的默认投递方式保持 retry：
  这个旁观者没了，普通的重试必须还在。

## rewrite

`aproxy-format rewrite --config rewrite.toml`：

```toml
remove_headers = ["x-legacy"]
set_headers = { "x-service" = "chat" }
remove = ["/metadata/debug"]
set = { "/temperature" = 0.5 }
models = { small = "provider-small" }
```

- **阶段**：request 与 retry。配了体的指针或模型规则时请求体要 `full`，只改头时要 `none`。
  不需要请求内的状态、失败详情、decide 或 notify。
- 没提到的字段不动。JSON Pointer 的转义与数组下标按规范解释；目标不存在与值是 JSON null 是两回事。
- 报告写做了哪些操作，不写旧值与新值。

## heartbeat

`aproxy-format heartbeat --config heartbeat.toml`：

```toml
[[preset]]
path = "/v1/messages"
protocol = "anthropic_messages"
event = "ping"
```

- **阶段**：只有 heartbeat。不协商请求体，不写 `state`，不参与 decide 与 notify，报告里不带与 key 有关的东西。
- **选预设**：按原始的客户端路径与写明的协议选，不从路由之后的请求去猜，也不把不知道的 `client.stream` 当成 false。
  没有预设匹配时，原样留下收到的字节。
- **输出**：每次输出都停在 SSE 的事件边界上（例如 `event: ping\ndata: {"type":"ping"}\n\n`）。
  预设不能编造文本、工具调用、用量或结束事件。
- 一个预设要先与真实转换出来的事件交错着喂给目标 SDK 的解析器，验证通过，才能写成「支持」。
- **现有的预设**（2026-11-20，主代理认可）：
  - 不给配置文件时，`/v1/messages` 用 `anthropic_messages` 的 `ping`：这是上游还没回任何东西时也安全的原生事件。
    给了配置文件，就只对文件里列出的路径生效。
  - Chat 与 Responses 的路径上原样留下收到的字节（通常是 aproxy 发的 SSE 注释），不造事件：实测 Chat 的 SDK 会记住第一个块的
    ID，哪怕它的 delta 是空的；Responses 的 SDK 要求第一个事件是 `response.created`。在上游的第一个真实事件之前造一个事件，
    要么给了假的响应身份，要么违反事件顺序。
  - 所以对这两种协议，心跳只是传输层的保活，不保证喂得住按事件计时的看门狗。

## 工具子命令与验证

- `init <场景> --output <目录>`：生成最小的一组角色配置与 config.toml。
  - 投递方式保持 retry；只含这个场景用到的角色；排在 route 之后的 convert 或 rewrite 要声明 retry。
  - 不写密钥，不悄悄覆盖已有的配置。
- `check <文件>` 检查一条链，`check <角色> --config <文件>` 检查一个角色：
  - 用 worker 启动时的同一套模型解析：不认识的键、不认识的取值、非法的正则、指针、权重与阶段组合都报错。
  - 检查重试链的后段是否合规；只转了请求方向、响应一侧没有转换器时报出来。
  - 检查静态配置时不执行配置里的命令。
- `try <链> --request <样例>`：拿本地的样例按声明的阶段干跑一遍，照 §10.14 的方向用 hello、序号、`state` 与表态驱动各角色。
  样例绝不发往上游。配置里的密钥、凭据头的值与 URL 里的敏感部分，在**每一步打印之前**就遮住，报错里也一样。
- `translate --from <协议> --to <协议>`：一次性的转换，从 stdin 读一个 JSON 请求。与 `convert` 不同，它不读 hello，也不输出信封。
- **验证**：
  - 协议抄本覆盖每个角色单独运行，以及 §7.2 的链对着本机的上游。核对的是管道里实际的字段、上游收到的 URL 与凭据、
    客户端收到的响应，不只是退出码。
  - 留一条直接的测试：默认 retry 之下可选的 adaptive 不可用时，普通的重试还在。
  - 流式：每个方向都跑，并把每个心跳预设交错进去，用官方 Python SDK 的解析器断言文本、工具、结束与用量；不调真实的 API。
    怎么跑：先 `cargo build -p aproxy-format --locked`，再在一个单独的 Python 环境里（`anthropic==2.8.0`、`openai==3.51.0`）
    运行 `python aproxy-format/tests/sdk/check_streams.py <构建出的 aproxy-format 的路径>`。脚本只同角色进程与 SDK 的
    内存 HTTP 传输打交道；除了不带心跳的对照，还检查心跳插在第一个事件之前与事件之间两种情况。

## 第五段审查之后定下的（2026-11-20）

这些是对上文的修正。第六段已经实现、并且并进了上面各节的：凭据不跨去向的四条、convert 认客户端协议、`preserve_path`、
别名只在 JSON 响应里换回、keys 握手前检查凭据、ID 上限、协议循环。还没实现的：convert 的 `to = "auto"` 与 route 的粘性
（第七段），`try` 的两条（第九段）；它们的配置键由实现定，定了之后并进上面各节。

- **凭据不跨去向**（convert、keys、route 共同）：
  - 去向变了，或把上游的响应交还客户端时，去掉的是全部「密钥头」（[w2-contracts](w2-contracts.md) §1.5 的名字规则，
    即 `aproxy_ipc::redact::is_secret_header`），加上配置里写明的凭据头；不是一张写死的短名单。
  - keys 的 `header` 与 route 的 `auth_header` 只能是「密钥头」：别的名字 aproxy 的历史不会打码，配置加载时就拒绝。
  - 查询串不跨 origin：route 把请求改到另一个 origin 时不带原来的查询串，渠道可以列出要保留的参数名。同 origin 照旧保留。
    §1.5 不按参数名猜哪个是密钥，这里也不猜。
  - convert 在响应方向失败时，回的 `fail` 必须带一个安全的 `response`（客户端协议的错误、头已去掉凭据）。
    不带 `response` 的 `fail` 等于把上游响应原样透传（§10.6），凭据头会跟着出去。
- **convert 的上游协议可以逐请求定**：`to` 写明时整条链只有一个上游协议。`to = "auto"` 时，按请求到达 convert 那一刻的去向判断：
  先查配置里「origin（可带路径前缀）对协议」的表，再按标准路径；判断不了就拒绝。客户端协议与上游协议都记进 `state`，
  响应方向照 `state` 转回。混合渠道的链把 convert 排在 route 之后并声明 retry，换了渠道就按新的去向重做。
  convert 不读 route 的 `state`（§10.3）。
- **convert 认客户端协议**：非标准路径上，体的形状两种协议都说得通时算判断不了，要求写明 `from`，不按「有 `messages`」猜。
- **route**：
  - `preserve_path` 接的是客户端原始路径（`client.path`）的路径部分，不是上一个上游地址里已经拼好的路径。
  - 粘性也归 route：开了粘性时，在最高优先级的可用渠道里按对话稳定前缀的哈希选；keys 再在这个 origin 的池里按同一个哈希选。
    两边各算各的，不互相读状态。
  - 渠道的覆盖在换渠道时自动复位（§10.4：覆盖属于这一轮的输出），不需要每个渠道把每个键都写一遍。
  - 把别名换回来只对声明为 JSON 的响应做；别的内容类型原样透传，哪怕字节恰好能解析成 JSON。
- **keys**：池里每个 key 的环境变量在握手之前就检查（有值、能当头值发出去），缺了就启动失败，消息里只写变量名。
- **写进 `state` 的 ID 有上限**：key、渠道、别名的 ID 在配置加载时检查，保证序列化后的 `state` 远小于 64 KiB。
- **协议循环**：只回应自己声明过的阶段；一帧必须以换行结束，EOF 前的残行不算一帧；读一行有上限，hello 不超过 64 KiB。
- **`try`**：
  - 默认只在进程内模拟官方角色，不启动链上的第三方命令（它们拿到样例与环境之后可以自己发出去）；
    第三方条目显示为跳过，要真的运行它们得显式加旗标，并在 stderr 说明后果。
  - 每一步显示的是投影（阶段、角色、改了什么、打码后的 URL 与头名），不显示原始的帧，也不显示 `state`，报错里也一样。
