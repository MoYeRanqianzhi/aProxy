# 示例集

## 1. 多 key 轮换（bash + jq 最小例）

多渠道聚合之外的「同渠道多 key 轮换」：信封 headers 改写 `authorization`。

```bash
#!/usr/bin/env bash
# rotate.sh — extra: {"keys":["sk-a","sk-b","sk-c"]}
COUNT_FILE_STATE=0   # 计数在进程内存（bash 变量），persistent 模式下跨请求连续
n=0
while IFS= read -r line; do
  # 从 extra 提取 keys（jq），按计数轮换取一个
  key=$(printf '%s' "$line" | jq -r --argjson i "$n" '
    (.extra | fromjson).keys[$i] // empty')
  out=$(printf '%s' "$line" | jq --arg k "Bearer $key" '.headers.authorization = $k | del(.extra)')
  printf '%s\n' "$out"
  n=$(( (n + 1) % 3 ))
done
```

config.toml：

```toml
request_transform = { command = "bash", args = ["rotate.sh"], mode = "persistent", extra = "{\"keys\":[\"sk-a\",\"sk-b\",\"sk-c\"]}" }
```

**spawn 模式下轮换会退化**（每请求新进程，计数恒 0，永远用第一个 key）——
轮换必须 `mode = "persistent"`。

## 2. 协议转换最小例（anthropic 请求 → openai-chat 请求，python）

```python
def process(env):
    import json
    req = json.loads(env.get("body") or "{}")
    msgs = req.get("messages", [])
    out = {
        "model": "gpt-4o",                                   # 或按映射表
        "messages": [{"role": m["role"], "content": m["content"]} for m in msgs if m["role"] != "system"],
    }
    if req.get("system"):
        out["messages"].insert(0, {"role": "system", "content": req["system"]})
    if req.get("max_tokens"):
        out["max_tokens"] = req["max_tokens"]
    env["url"] = "https://api.openai.com/v1/chat/completions"  # url 改写
    env["headers"] = {"content-type": "application/json",
                      "authorization": "Bearer " + env["extra"]}  # extra 传 key
    env["body"] = json.dumps(out)
    return env
```

真实的完整转换（含响应与 SSE 流的双向转换）不建议手写——用官方
aproxy-format（下节），或 skill 之外的转换库。

## 3. 官方 aproxy-format 二进制（协议转换 + 聚合，开箱即用）

下载：GitHub Release（`aproxy-format-<平台>` 资产）落 `~/.aproxy/bin/`，
或 `npm i -g @meowo/aproxy-format`（进 PATH）。**版本独立于 aproxy 主程序**
（稳定后几乎不更新——它只是示例，更多功能直接让 agent 写 format）。

聚合配置 `~/.aproxy/agg.toml`：

```toml
client_format = "auto"        # auto = 逐请求检测；或显式 "anthropic_messages"

[models]                      # 可选：客户端模型名 → 上游模型名
"claude-sonnet" = "claude-sonnet-4-5"

[[channel]]
name = "official"
format = "anthropic_messages"  # 该渠道上游协议
url = "https://api.anthropic.com"
keys = ["sk-ant-1", "sk-ant-2"]
# weights = [1, 1]            # 加权轮询时按 keys 展开轮转
# models = ["claude-*"]       # 该渠道服务的模型 glob；缺省全部
# preserve_path = false       # true = channel.url 后拼接原 path

[[channel]]
name = "relay"
format = "openai_chat"
url = "https://relay.example.com/v1/chat/completions"
keys = ["sk-relay-1"]
```

config.toml 接入（请求/响应各一条，args 相同；路径在 extra——worker 惰性
读入并缓存）：

```toml
request_transform = { command = "~/.aproxy/bin/aproxy-format", args = ["run"], mode = "persistent", extra = "~/.aproxy/agg.toml" }
response_transform = { command = "~/.aproxy/bin/aproxy-format", args = ["run"], mode = "persistent", extra = "~/.aproxy/agg.toml" }
```

运作：请求侧按 body 的 model 路由渠道 → 按策略选 key（轮询/加权轮询，
计数在 worker 内存——**聚合必须 persistent**）→ 协议不等则转换 → 改写
url + 鉴权头（anthropic_messages → `x-api-key` + `anthropic-version`；openai →
`authorization: Bearer`）；响应侧按信封 url 反查渠道 → 反向转换 → **剔除
鉴权头**（上游 key 不回传客户端）。

独立转换命令（不经 aproxy 也能用）：

```bash
aproxy-format convert --from anthropic_messages --to openai_chat < req.json > out.json
```

## 4. 响应侧自定义转换（改写上游响应后再回放）

```python
def process(env):
    # env["url"] = 请求侧最终上游地址（多渠道按它反查渠道——两侧对齐唯一线索）
    body = env.get("body", "")
    env["body"] = body.replace("sensitive-word", "***")
    env["headers"] = {"content-type": "application/json"}   # 整表替换；剔除鉴权头！
    return env
```

响应侧输出头表就是回放给客户端的头表——若请求侧曾把真实 key 放进上游头，
响应侧输出时务必重建干净头表。
