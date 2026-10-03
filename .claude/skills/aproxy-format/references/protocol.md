# 信封协议参考（v1）

信封 = 一行 JSON。aproxy 写一行到 format 的 stdin，format 写一行回 stdout。
请求侧与响应侧同构，字段用法有差异。

## 字段总表

| 字段 | 类型 | 请求侧（进） | 请求侧（出） | 响应侧（进） | 响应侧（出） |
|---|---|---|---|---|---|
| `url` | string? | aproxy 算出的上游地址 | **可改**（缺省=沿用） | 请求侧最终上游地址（反查键） | 被忽略 |
| `method` | string? | `"POST"` 等 | 可改（缺省=沿用） | **缺省（=响应侧标志）** | 被忽略 |
| `headers` | object | 头表（键小写） | **整表替换** | 头表 | **整表替换** |
| `body` | string? | UTF-8 body 文本 | 转换后 body（文本） | 同左 | 同左 |
| `body_b64` | string? | 非 UTF-8 的 base64 | 同左（二进制输出用） | 同左 | 同左 |
| `worker_id` | int | 池槽位（spawn 恒 0） | 被忽略 | 同左 | 被忽略 |
| `extra` | string | transform extra 原样 | 不必回传 | 同左 | 不必回传 |
| `error` | string? | 不出现 | **单请求失败原因**（exit 0） | 不出现 | 同左 |

## JSON 完整规范（任意语言实现以此为准）

本节是信封的**语言无关 wire 规范**——aproxy 侧由 Rust serde 实现，本节把它
完整翻译成任何 JSON 库都能遵循的规则。按本节实现的 format 与语言无关。

### 必填性（违反 = aproxy 解析失败 → 请求侧 502）

| 字段 | aproxy 发给你 | 你输出回 aproxy |
|---|---|---|
| `headers` | 必有（可能为 `{}`） | **必填**（缺这个键 = 解析失败，可为 `{}`） |
| `worker_id` | 必有（int ≥ 0） | 可省略；给了必须是**非负整数** |
| `extra` | 必有（string，可能为 `""`） | 可省略；给了必须是 **string** |
| 其余（url/method/body/body_b64/error） | 缺省时**键不出现** | 可选；给 `null` 等价于省略 |

**最常见死法**：输出 `{"body": "..."}`——缺 `headers` 键，aproxy 解析失败
502，错误日志为「信封 JSON 解析失败: missing field `headers`」。最小合法输出：

```json
{"headers": {}, "body": "..."}
```

### 序列化规则

- **缺省即不出现**：可选字段未设置时，aproxy 写出的行里**没有这个键**（不是
  `null`）。你的解析代码要按「键可能不存在」处理；你输出时省略与写 `null`
  两者都合法。
- **未知字段忽略**：你输出的信封里多余的键（调试字段、内部状态）aproxy
  静默忽略——不会报错也不会透传到任何地方。
- **类型严格**：`worker_id` 必须是 0..=4294967295 的整数（负数/小数/字符串
  = 解析失败）；`extra` 必须是 string；`headers` 的键值都必须是 string。
- **互斥**：`body` 与 `body_b64` 不能同时出现（两侧都校验）。
- **error 与正常输出**：`error` 非空时，aproxy 只读 error 文案、忽略信封
  其余字段——失败输出用 `{"headers": {}, "error": "..."}` 即可。

### base64 精确变体

`body_b64` 用**标准字母表**（`A-Z a-z 0-9 + /`）**带 `=` padding**、无换行：
- Python：`base64.b64encode` / `base64.b64decode` ✓（不要用 `urlsafe_b64encode`）
- Go：`base64.StdEncoding`（✗ StdURL/.Raw）
- Node：`Buffer.from(b64, "base64")` / `buf.toString("base64")` ✓
- 换行插入（MIME 式 76 列）✗——解码会失败

### 行与编码

- 一行 = 一个紧凑 JSON + `\n`（无 `\r`）；JSON 字符串转义保证 body 内的
  换行不会破帧——**不要自己再转义一层**。
- **每个请求恰好回一行**：信封协议没有请求序号，aproxy 靠「一请求一行、按序」
  把输出对应到请求。多出一行（stdout 里夹了日志/横幅、`jq` 没加 `-c`、空闲时
  冒出输出）即协议错误：该 worker 被剔除，当次请求 502，错误带「format 输出违反
  信封协议:」前缀。所以 **stdout 只写信封行，日志写 stderr**（aproxy 会丢弃
  stderr）。
- 全程 UTF-8、无 BOM；非 ASCII 字符原样 UTF-8 输出（aproxy 不做 `\uXXXX`
  转义，你的语言也不必）。**Windows 注意**：脚本语言的 stdout 默认编码可能
  是系统代码页（如 GBK）——必须显式 UTF-8（见 guide.md 多语言坑表），否则
  中文 body/文案会乱码或抛编码异常。

### 完整样例

**请求侧输入**（aproxy 发给你的一行，此处为展示加了折行——实际是单行）：

```json
{
  "url": "https://api.anthropic.com/v1/messages",
  "method": "POST",
  "headers": {
    "content-type": "application/json",
    "authorization": "Bearer sk-client-value",
    "x-api-key": "sk-client-value"
  },
  "body": "{\"model\":\"claude-3\",\"messages\":[...],\"max_tokens\":100}",
  "worker_id": 2,
  "extra": "~/.aproxy/agg.toml"
}
```

**响应侧输入**（无 `method`——这是响应侧的唯一标志；`url` 是请求侧最终
上游地址）：

```json
{
  "url": "https://api.anthropic.com/v1/messages",
  "headers": {"content-type": "application/json"},
  "body": "{\"role\":\"assistant\",\"content\":[...]}",
  "worker_id": 0,
  "extra": "~/.aproxy/agg.toml"
}
```

**成功输出**（只回传你改写的字段，其余省略即可）：

```json
{"url": "https://relay.example.com/v1/chat/completions", "method": "POST", "headers": {"content-type": "application/json", "authorization": "Bearer sk-relay-1"}, "body": "{...}"}
```

**失败输出**（error 行，exit 0）：

```json
{"headers": {}, "error": "model gpt-nope 未命中任何渠道"}
```

## 语义细节

### body 与 body_b64

互斥（同时出现 = 信封错误）。两者都缺 = 空 body。aproxy 发非 UTF-8 body
（如 multipart 上传）时自动走 `body_b64`（标准 base64、无换行）；你的输出
同理——二进制产物用 `body_b64`。

### headers

- 键为 HTTP 小写规范名（`authorization`、`x-api-key`）。
- **整表替换语义**：你输出的 `headers` 就是 aProxy 发往上游（请求侧）或回放
  客户端（响应侧）的头表。要删头就输出时不含它。
- 进信封时 hop-by-hop 头（connection/transfer-encoding/host/…）与
  content-length 已被剔除；你回传这些头会被忽略。
- 多值头（如多个 set-cookie）**仅保留首值**进信封（有损边界，warn 留痕）——
  LLM API 场景无多值请求头，可忽略。
- 响应侧注意：**aProxy 不改你在响应侧的鉴权头**——若你把上游 key 放进了
  请求侧 headers，响应侧输出头表时务必剔除（官方 aproxy-format 自动剔除）。

### url（请求侧）

改写它 = 协议转换的路径/域名迁移核心机制。例如把
`https://api.anthropic.com/v1/messages` 改写到
`https://relay.example.com/v1/chat/completions`。缺省 = 沿用 aProxy 计算的
原始上游地址。

### url（响应侧）

响应信封的 `url` 是**请求侧最终发往上游的地址**（你自己在请求侧改写后的），
用它可以反查「这个响应来自哪个渠道」——请求转换器与响应转换器是不同进程、
无共享状态，信封 url 是两侧对齐的唯一线索（官方 aproxy-format 的多渠道
聚合靠它做响应侧反向转换）。

### error 行

```json
{"headers": {}, "error": "model 未命中任何渠道"}
```

输出 error 行（**exit 0**）= 该请求转换失败。后果按方向：请求侧 → 客户端
收到 502（含你的 error 文案）且不发上游；响应侧 → 客户端收到上游原始响应。

### worker_id

persistent 池的槽位号（0..pool_max）。轮换类 format 可用它做起始偏移
（`首key序号 = worker_id % key数`），避免池内各 worker 轮换起点重合导致
key 使用不均。spawn 模式恒 0。

### extra

aProxy 配置 `request_transform.extra` / `response_transform.extra` 的原样
透传（格式无要求，format 自解）。官方 aproxy-format 约定：extra = 聚合配置
文件的路径。

## 安全须知（写 format 前必读）

- **信封含客户端完整凭据**：`headers` 里有客户端发来的 `authorization` /
  `x-api-key` 等鉴权头原值。你的 format 进程能读到它们——**不要写入日志、
  不要转发到信封 url 之外的目的地**。请求侧改写鉴权头时先删旧值再写渠道值。
- **响应侧输出头表直接回放客户端**：若你在请求侧曾把真实渠道 key 写进
  上游头，响应侧输出时务必重建干净头表（官方 aproxy-format 自动剔除
  `authorization` / `x-api-key` / `cookie` / `proxy-authorization`）。
- **保活通道例外**：客户端走 SSE 保活通道时（上游首轮失败进入重试），
  响应头表已随 SSE 骨架发出——此通道下 format 对 headers 的改写**不生效**，
  仅 body 转换生效。
