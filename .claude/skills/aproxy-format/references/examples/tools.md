# The official aproxy-format binary: roles, chains and tools: tools

Part of [examples.md](../examples.md), which lists the other parts. `check`, `try` and `translate`.

## Tools: check, try, translate

Every subcommand and every role answers `--help` (or `-h`).

### check

```bash
aproxy-format check config.toml --home ~/.aproxy     # a whole chain
aproxy-format check keys --config keys.toml          # one role file
```

`check` parses with the same types the workers use at startup, so what passes here does not then
fail the handshake for a structural reason. It never runs a configured command and opens no
network connection. Exit code 0 means valid; the output says exactly what that covers:

```text
format 链与官方角色配置有效（7 个程序）。
没有检查：
  - 环境变量 OPENAI_KEY、RELAY_KEY_A、RELAY_KEY_B 是否已设置（启动密钥角色或选用渠道时需要）
  - 实例的其余配置（监听地址、超时等），由 aproxy doctor 检查
```

So after a clean `check`, still make sure the listed variables are set for the instance, and run
`aproxy doctor` for the rest of config.toml. Pass `--home <dir>` (the instance's home) so that
relative commands and `--config` paths resolve as the instance resolves them; without it they
resolve next to the checked file and a notice on stderr says so.

### try

```bash
aproxy-format try config.toml --request sample.json --home ~/.aproxy
```

Rehearses one request through the chain and prints one JSON line per step. The sample describes
the client request and, optionally, what the upstream answers on each attempt:

```json
{
  "path": "/v1/messages",
  "headers": { "content-type": "application/json" },
  "body": { "model": "gpt-4o", "max_tokens": 64, "messages": [{ "role": "user", "content": "hello" }] },
  "attempts": [
    { "status": 503, "headers": { "retry-after": "1" }, "body": { "error": { "message": "overloaded" } } },
    { "status": 200, "headers": { "content-type": "application/json" },
      "body": { "id": "chatcmpl-1", "object": "chat.completion", "model": "gpt-4o",
                "choices": [{ "index": 0, "message": { "role": "assistant", "content": "hi" }, "finish_reason": "stop" }],
                "usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 } } }
  ]
}
```

| Sample field | Meaning |
|---|---|
| `path` | The client's path, with query. Required. |
| `method`, `headers` | Optional; a JSON `POST` by default. |
| `body` | Required. An object is sent as JSON, a string as the exact raw body. |
| `attempts[]` | One upstream result per attempt: `status`, `headers`, and either `body` or `chunks` (a list of strings delivered as stream pieces); `failure` instead of `status` for an attempt with no response (a failure code such as `upstream_connect_failed`); `interrupted: true` when the stream breaks after the last chunk. Without attempts only the request direction runs. |
| `heartbeat` | `true` rehearses one keepalive beat. |

Each output line names the stage, the role and what changed, never a body, a header value, a
credential or the real URL:

```text
{"stage":"request","role":"route","input":{...},"output":{...},"changed":["url"]}
{"stage":"request","role":"keys",...,"changed":["headers"]}
{"stage":"decide","role":"policy",...,"changed":["retry","reroute","delay"]}
{"stage":"retry","role":"route",...,"changed":["url"]}
{"stage":"stream:open","role":"convert",...,"changed":["head"]}
{"stage":"result","role":"proxy","input":null,"output":{"status":200,"decided_by":"fallback","outcome":"ok","code":null},"changed":[]}
{"stage":"notify","role":"adaptive",...,"changed":["request_end"]}
```

What to rely on and what not:

- **`try` itself opens no network connection;** the sample goes nowhere.
- **Programs that are not official roles are skipped by default** and shown as skipped.
  `--allow-external` starts them; they then receive the sample and the environment and can do
  anything with them, network included. Use it only for programs the user trusts.
- **The trace shows what the instance would do, or `try` stops.** Where it does not simulate a
  situation (a worker failing mid-rehearsal, a failure inside a stream with several stream
  layers) it prints `未模拟：…`, says what the real instance does, and exits with code 2 rather
  than printing a guess.
- When the sample runs out of attempts while the chain still wants to retry, the last line is
  `"changed":["more_attempts_needed"]`: add attempts to see further.
- Roles that read credentials need their environment variables set for the `try` run (any
  placeholder value will do).

### translate

```bash
aproxy-format translate --from anthropic_messages --to openai_chat < request.json
```

Converts one JSON request body from stdin to stdout, with no handshake and no protocol frames. Use it to
see what `convert` would send for a given body; chains use `convert`.
