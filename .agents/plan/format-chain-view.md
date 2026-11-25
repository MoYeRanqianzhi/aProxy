# format 链路视图

**状态（2026-11-26）**：用户提出需求并定了一条（下面「用户定下的」）；其余是主代理的设计稿，用户尚未确认；没有任务书，没有代码。完整版随插件一起做
（[plugins](plugins.md)），0.2.0 里可选做一小块前端（见「分期建议」）。

## 来由

用户 2026-11-26 原话：「还需要考虑插件多了容易撞功能，设计一个功能为format链路展示，方便判断链路是否存在冲突，比如多个插件都
注册了协议转换format，可能导致请求内容经历Anthropic->OpenAI->Gemini->OpenAI->Anthropic->Chat->Anthropic这种绕了一大圈结果是
Anthropic进Anthropic出的情况，反而容易在中途丢失部分参数造成bug，同时还有性能浪费」。

随后又要求插件交接要有试运行（[plugins](plugins.md) 第 19 条），其中一个用途就是这里的链路展示：没在运行的配置，也能先试运行
插件，看合成出来的链路。

## 用户定下的

- **只做一个额外的检查命令，不影响自由度**（用户 2026-11-26 原话：「不需要有绕圈就不启动，只是作为一个额外指令用于检查配置，
  不要影响自由度」）。链路分析不阻止启动，也不在启动、doctor 或别的命令里主动提示；只有用户或 agent 运行检查命令、或在桌面上
  打开链路视图时才做。
- **链路只做展示，不下结论**（用户 2026-11-26 原话：「aproxy config check是全部的检查，其中可以细分有format检查，细分format检查
  中的链路显示，或者不能叫check，因为实际上没有check，只是显示了链路过程，没有起到检查，因为这是无法判断的」）。format 是黑箱，
  链路是不是「冲突」无法判断，所以链路视图只列出事实（经过哪些条目、协议怎么变、按协议规则谁先谁后），不叫检查、不给「冲突」「错误」
  之类的结论。命令怎么分，见下面「出口」（主代理的建议，待用户确认）。
- **0.2.0 先做桌面请求详情里的协议路径**（用户 2026-11-26：「放进0.2.0」）。

## 现在已有的（2026-11-26 查过）

- 状态快照的 `formats[]`：每个条目的协商与熔断状态，按配置顺序（`aproxy-ipc/src/wire.rs:59`）。
- 请求记录：请求级与每次尝试各有 `reports`（format 名、阶段、summary、actions，`aproxy-ipc/src/view/dto.rs:209` 与 `:235`），
  actions 里的 `protocol_convert` 带 `from` 与 `to`。桌面的请求详情页已经列出这些报告（`aproxy-desktop/ui/src/pages/LogDetail.svelte:478`）。
- 没有的：每一层花了多少时间；format 事先说明自己会做什么；把报告串成协议路径、判断有没有绕圈。
- format 握手回复对不认识的键是宽松的（`aproxy-envelope/src/messages.rs` 的 `HelloReply::try_from` 只取认识的字段），所以以后在
  hello 回复里加一个可缺省的声明，0.2.0 的实例会忽略它，新旧可以混用。0.2.0 发布之前不必为此改 format 协议。

## 设计（主代理建议）

### 一、format 声明自己会做什么

hello 回复加可缺省的 `effects`：数组，每项的写法与 `report.actions` 相同、用同一份词表（`protocol_convert` 带 `from` 与 `to`；
`key_select`、`route`、`header_edit`、`body_edit`、`custom`），只是不带每次请求才有的值（例如 `key_select` 不带 `key_id`）。它说的是
「这个程序可能做的事」，供展示与分析，不是保证；没声明的显示「未声明」，以运行时的报告为准。官方 aproxy-format 按角色与参数声明
（convert 声明它的 `from` 与 `to`，keys 声明 `key_select`，route 声明 `route`，依此类推）。

### 二、静态链路：不需要任何请求

对一个运行中的实例，或一份没在运行的配置（插件试运行、format 只握手不发请求：hello 之后给 EOF），给出：

- **合成过程**：每一层（内置默认、settings.json 的 `defaults`、config.toml、`start` 的覆盖参数、各插件）设了什么、谁盖掉了谁，即插件
  交接视图里的来源表。
- **format 链**：请求方向的顺序；每项的名字、来自哪一层（哪个插件）、程序名与版本、阶段、进程方式、`required`、熔断状态、声明的
  effects。按阶段分泳道：`request`/`retry` 与 `decide` 按列表顺序，`response`/`stream`/`heartbeat` 反向，`notify` 只旁观。
- **协议路径模拟**：对每种客户端协议（按 `client.path` 认得的 `anthropic_messages`、`openai_chat`、`openai_responses`，加上 effects
  里出现过的），沿请求方向套用 `from` 对得上的 `protocol_convert`，得出路径与发往上游的协议；响应方向按相反顺序还原。
- **标注的事实**（不是结论；按声明算出，声明可能不全，没声明的条目标「未声明」；每条点名涉及的条目与它们来自哪一层）：
  1. 每种客户端协议的路径与转换次数，例如「Anthropic → OpenAI → Gemini → OpenAI → Anthropic，转换 4 次，进出相同」（用户举的例子）。
  2. 按协议规则谁说了算：请求方向上后面的 `key_select`、`route` 会盖掉前面的；`decide` 与 `delivery` 是先表态的胜出。
  3. 某个转换的 `from` 对不上任何到达它的协议（对这些客户端它不会动手）。
  4. 请求方向转成了另一种协议、响应方向按声明没有转回来的情况。
- **出口**（主代理按用户的两种说法给的建议，待确认）：检查与展示分成两个命令，名字说实话。
  - `aproxy config check [--config …] [--formats] [--json]`：全部的检查，只查能判断对错的事，有结论、影响退出码：按 `start` 的规则
    校验配置；试运行每个插件（`mode` 为 `dry_run`、`reason` 为 `check`），看它能否按时写出合法的四行；拉起每个 format 只握手、
    不收请求（hello 之后给 EOF），看它起得来、协议对、声明的阶段都支持。`--formats` 只做 format 这一部分。它会运行第三方程序，
    这一点写进命令说明；不改任何文件，不启动实例。
  - `aproxy config show --chain [--config …] [--json]`：只展示，没有结论，不影响退出码：合成过程、format 链与上面「标注的事实」。
    要拿到插件的覆盖与 format 的声明，同样要试运行插件、让 format 握手（`reason` 为 `preview`）。
  - `aproxy status <target> --chain`：运行中的实例实际的链，不试运行，用实例自己合成好的配置与握手结果。
  - 桌面：实例页可以打开链路视图（泳道、箭头、每一段的协议、标注），配置页有「检查配置」与「查看链路」两个按钮，分别走上面两个命令。
  - 别的命令与启动过程都不做链路分析。

### 三、请求实际走过的链路：运行时

- 请求记录加可缺省的 `layers`：每调用一次 format 记一项（名字、阶段、耗时、结果：完成、跳过、出错、拒绝），有条数上限。与已有的
  `reports` 一起，在桌面的请求详情与 `aproxy logs` 的详情里显示：实际经过了哪些 format、各花了多少时间、format 合计耗时与上游耗时
  的对比。
- 把报告里的 `protocol_convert` 按链上顺序串成协议路径，例如 `anthropic_messages → openai_chat → anthropic_messages`；绕圈或多次
  转换就标出来。
- 这一层以实际发生的为准，没声明 effects 的程序也抓得到（只要它写了报告）。

### 四、与插件的关系

插件多了才最容易撞功能，所以完整版随插件一起做，标注要点名是哪个插件带来的条目。插件交接时看得到当前的 format 列表（视图），
可以 keep 或去掉前面各层的条目（[plugins](plugins.md) 的「顺序与合并」）；链路视图让用户与插件作者看清最后合成的结果。

## 分期建议

- **0.2.0（用户定的，主代理做，只动前端）**：已做完（2026-11-26，72c887bd）。桌面请求详情的 format 页签顶部「协议路径」：
  发往上游（请求级 request 报告）、每次尝试前重跑的一段（各次尝试的 retry 报告，相邻且路径相同的并成一行）、回给客户端（请求级
  response/stream 报告），各自把 `protocol-convert` 按报告先后串起来，接不上处显示断开；下面一行只写转换次数、首尾是否同一种
  协议。演示数据里 12346 的 Anthropic 请求带一对转换报告。用 `npm run check` 与演示数据在中英两种语言、窄窗口下实看过；没有在
  连着真实实例与真实 convert 的窗口里看过。
- **0.2.0 之后，随插件**：一（hello 的 `effects` 与官方 aproxy-format 的声明）、二（`config check`、`config show --chain`、
  `status --chain`、桌面）、三（`layers`）。

## 待用户定

- 「出口」里检查与展示分成两个命令的建议；`config check` 要不要也提前进 0.2.0（没有插件时，它查配置与 format 握手）。
