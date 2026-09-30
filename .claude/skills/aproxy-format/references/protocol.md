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
