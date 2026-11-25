# format 链路视图与冲突提示

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
- **提示**（只在检查时给出，从不阻止启动；每条点名涉及的条目与它们来自哪一层）：
  1. **绕圈**：路径回到走过的协议，或者进出是同一种协议却转换过（用户举的例子）。
  2. **多次转换**：一条路径转换不止一次；每次转换都可能丢字段，也都要花时间。
  3. **遮蔽**：两项声明了只会有一个生效的动作。请求方向上后面的 `key_select`、`route` 会盖掉前面的；`decide` 与 `delivery` 是先表态的
     胜出，后面的不起作用。
  4. **轮不到**：某个转换对任何客户端协议都对不上 `from`，等于白挂在链上。
  5. **响应没还原**：请求被转成另一种协议，响应方向却没有对应的还原，客户端会收到另一种协议的响应。
- **出口：一个专门的检查命令**（用户定的）。建议 `aproxy config check [--config …] [--json]`：按 `start` 的规则校验配置，试运行
  插件（`mode` 为 `dry_run`、`reason` 为 `check`），让每个 format 只握手不收请求（hello 之后给 EOF），然后列出合成过程、format 链与
  提示。它会运行第三方程序，这一点写进命令说明；它不改任何文件、不启动实例。提示不影响退出码（配置本身有错才按现有规则退出 3）。
  运行中的实例用 `aproxy status <target> --chain` 看实际的链（不试运行，用实例自己合成好的配置与握手结果）。桌面：实例页可以打开
  链路视图（泳道、箭头、每一段的协议、提示标记），配置页有「检查配置」按钮，走同一个检查（`reason` 为 `preview`）。别的命令与
  启动过程都不做链路分析。

### 三、请求实际走过的链路：运行时

- 请求记录加可缺省的 `layers`：每调用一次 format 记一项（名字、阶段、耗时、结果：完成、跳过、出错、拒绝），有条数上限。与已有的
  `reports` 一起，在桌面的请求详情与 `aproxy logs` 的详情里显示：实际经过了哪些 format、各花了多少时间、format 合计耗时与上游耗时
  的对比。
- 把报告里的 `protocol_convert` 按链上顺序串成协议路径，例如 `anthropic_messages → openai_chat → anthropic_messages`；绕圈或多次
  转换就标出来。
- 这一层以实际发生的为准，没声明 effects 的程序也抓得到（只要它写了报告）。

### 四、与插件的关系

插件多了才最容易撞功能，所以完整版随插件一起做，提示要点名是哪个插件带来的条目。插件交接时看得到当前的 format 列表（视图），
可以 keep 或去掉前面各层的条目（[plugins](plugins.md) 的「顺序与合并」）；链路视图让用户与插件作者看清最后合成的结果。

## 分期建议

- **0.2.0（可选，主代理做，只动前端）**：桌面请求详情把已有报告里的 `protocol_convert` 串成一行协议路径，绕圈或转换不止一次时
  标出来。数据已经有，不改后端。
- **0.2.0 之后，随插件**：一（hello 的 `effects` 与官方 aproxy-format 的声明）、二（静态链路与提示：`config check`、
  `status --chain`、桌面）、三（`layers`）。

## 待用户定

- 0.2.0 要不要先做前端那一小块。
