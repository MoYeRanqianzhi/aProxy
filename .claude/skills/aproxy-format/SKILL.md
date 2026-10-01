---
name: aproxy-format
description: aProxy 外部转换器（format 程序）的完整参考——信封协议逐字段、编写指南、示例集与排障。凡涉及为 aProxy 的 request_transform/response_transform 编写或配置 format 程序、实现 OpenAI ↔ Anthropic 等协议转换、多 key 轮换、多渠道聚合（newapi 式）、或使用官方 aproxy-format 二进制时使用本 skill，即使用户没有明说「写 format」——例如"帮我把这个实例挂到 OpenAI 兼容端点"、"转换 anthropic 到 openai 协议"、"加几个 key 轮换着用"、"多渠道聚合不同模型"。
---

# aproxy-format skill

## 一分钟心智模型

aProxy 把整个请求（或上游响应）装进**一行 JSON 信封**，写到你的 format 程序的
stdin；你的程序改写后**写一行信封回 stdout**。信封是唯一接口——url、method、
headers、body 全部可改写，这就是协议转换与聚合的全部机制。

两种运行模式（`request_transform`/`response_transform` 的 `mode`）：
- **spawn**（默认）：每请求启动你的进程、一行进出、进程退出。最简单，任何
  能读写 stdin/stdout 的程序都行。**注意：spawn 模式下转换在单实例内是
  串行的**（每请求一次进程启动，实例内逐个执行）——高并发场景选 persistent。
- **persistent**：进程池。你的进程以 `while` 循环逐行处理（一次一个请求、
  输入输出有序），按需扩容至 `pool_max`（并发上限），空闲超时被回收。
  **聚合类 format（轮换计数等状态在进程内存）必须用本模式**；高并发场景
  也用它（进程免重启、池按并发扩容）。

失败语义（两侧不同，写 format 前先想清楚你在哪一侧）：
- **请求侧转换失败 → 502 不重试**（确定性失败）
- **响应侧转换失败 → 透传上游原始响应**（响应已在手，可用性优先）

## 按需查阅（读前先看这里，不要盲猜）

| 任务 | 文件 |
|---|---|
| 信封每个字段的确切语义（进/出、互斥、必填性） | [references/protocol.md](references/protocol.md) |
| 从零写一个 format（选型、循环模板、测试方法） | [references/guide.md](references/guide.md) |
| 完整示例：多 key 轮换、协议转换、官方 aproxy-format 用法 | [references/examples.md](references/examples.md) |
| 502 了、error 行没输出、persistent worker 不退出 | [references/troubleshooting.md](references/troubleshooting.md) |
| **联调测试器**：不起 aproxy 直接测你的 format（三协议模拟 + 回包检查 + EOF 义务判定） | [scripts/test_format.py](scripts/test_format.py) |

## 高频守则（最常见的错误，细节全部下放 references）

1. **单行进出**：信封是一行 JSON（JSON 序列化天然转义换行，body 再长不破帧）。
   读 stdin 按行读、写 stdout 按行写、**写后必须 flush**。
2. **error 行表达单请求失败，exit 0**：输出 `{"headers":{},"error":"原因"}`
   后正常退出（exit 0）。非零 exit code = 进程级失败（崩溃/挂死），会触发
   请求侧 502 / 响应侧透传——不要用非零 exit 表达单请求失败。
3. **persistent 必须 `while` 循环 + EOF 退出**：读到 stdin EOF 就 exit（这是
   铁律——aProxy 实例停止时靠关闭管道回收 worker；不吃 EOF 的 format 会
   挂成孤儿进程）。一行处理完立即写回，再等下一行。
4. **headers 键是小写规范名，整表替换**：你收到完整头表，输出的头表**就是**
   发往上游的头表（多 key 轮换 = 改写 `authorization` 即可）。不要输出
   `content-length` / `transfer-encoding` 等**传输控制头**（aProxy 按实际
   字节回填，你输出的也会被忽略）；`content-encoding` 在响应侧会被强制
   剔除（aProxy 交给你的 body 已解码）——不要试图在信封层处理压缩。
5. **输出必须含 `headers` 键（可为 `{}`）**：它是唯一**必填**字段——
   `{"body": "..."}` 这种自然写法会因缺键解析失败、请求侧直接 502（日志：
   missing field \`headers\`）。字段级必填性/类型约束/编码规则见
   [references/protocol.md](references/protocol.md) 的「JSON 完整规范」。
6. **大 body 走 `body_b64`**：body 不是合法 UTF-8 时用 base64 字段，其余场景
   用 `body` 文本字段（两者互斥；**标准字母表 + `=` padding**，URL-safe 变体
   解码会失败）。
7. **不要设 content-length**：aProxy 按实际字节回填，你设了也会被忽略。
8. **官方示例**：`aproxy-format` 二进制开箱即用（协议转换 + key 轮换 + 多渠道
   聚合），写自定义 format 前先看它能不能直接满足——见
   [references/examples.md](references/examples.md)。
