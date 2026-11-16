# Troubleshooting transformers

Error messages and symptoms caused by a format program, with their causes and fixes. Read it when
a request fails with a 422 or 502 that names the transformer, when a log line names it, or when
conversion silently does not happen. The messages are quoted exactly as aProxy and aproxy-format
print them, so search for the Chinese text in real output.

## Where the messages appear

- **The client** gets `422` with `x-aproxy-error: format_rejected` when the program replied with an
  `error` envelope; the message is `format '<name>' rejected the request: <reason>`, where `<name>`
  is the `[[format]]` entry's `name`.
  When the program itself failed (spawn, crash, pipe, timeout, protocol) it gets `502` with
  `x-aproxy-error: format_failed` and `details.kind` naming which. Nothing was sent upstream, and
  the request is not retried. A streaming client that had already received keepalive headers gets
  the same error JSON as a final SSE `event: error` frame instead of an HTTP status. Match on
  `error.aproxy.code` or `x-aproxy-error`, not on `message` (aproxy-cli skill, behaviors.md,
  After the commit).
- **The daemon log** (`aproxy logs <port|alias>`) records request-side failures as
  `请求转换失败，终态返回（请求侧转换失败按约定不重试）` ("request transform failed; final
  response returned, not retried") and response-side failures as `响应转换失败，透传上游原始响应`
  ("response transform failed; passing the upstream response through"), each with an `error` field
  holding the reason. The request-side line also has a `code` field (`format_rejected`,
  `format_failed` or `local_storage_failed`); the response-side line has none, because that request
  did not fail.
  `aproxy status` shows the latest request-side failure as the instance's last error, written as
  `<code>: <English message>`.
- **Your program's stderr** is discarded by aProxy. To see a crash, run the configured command by
  hand with one envelope on stdin (stderr then goes to your terminal):

```bash
E='{"method":"POST","url":"https://api.anthropic.com/v1/messages","headers":{"content-type":"application/json"},"body":"{\"model\":\"claude-sonnet-4-5\",\"max_tokens\":16,\"system\":\"s\",\"messages\":[{\"role\":\"user\",\"content\":\"hi\"}]}","worker_id":0,"extra":""}'
printf '%s\n' "$E" | <command> <args...>          # expect exactly one JSON line
printf '%s\n' "$E" "$E" | <command> <args...> | wc -l   # expect 2
```

Run the command with the same `env` and `args` as the config. `scripts/test_format.py` (guide.md,
Test without aProxy) checks more but hides stderr.

## Reasons reported by aProxy

An `error` reply becomes `422` `format_rejected` with the message
`format '<name>' rejected the request: <reason>`; the reason is your program's own text. For the
official binary, see Errors from aproxy-format below.

A failure of the program itself becomes `502` `format_failed` with the message
`format '<name>' failed (<kind>): <detail>`. `details.kind` names the failure, and the
`<detail>` begins with one of these (the Chinese text is unchanged):

| `details.kind` | Detail begins with | Meaning | Fix |
|---|---|---|---|
| `spawn` | `format 进程启动失败（检查 command 路径）` ("format process failed to start; check the command path") | The program could not be started: not found, not executable, or a relative path that does not resolve from the daemon's working directory | Use an absolute path in `command`. On Windows a bare name is found only as an `.exe`, so npm `.cmd` shims and scripts need their interpreter as `command`. |
| `died` | `format 进程意外退出且无输出` ("format process exited unexpectedly without output") | The program exited or crashed before writing a reply. A worker that merely died while idle is not the cause: aProxy retries that case once on a fresh process | Run the command by hand (above). Usual causes: a wrong script path in `args` (not `~`-expanded), a missing runtime or module, an unhandled exception, the official binary started without `args = ["run"]` (it prints usage and exits). |
| `io` | `format 进程管道读写失败` ("format process pipe read/write failed") | The process died or closed its pipes mid-exchange | Same as above. |
| `timeout` | `转换超时` ("conversion timed out") | No reply line within `timeout_secs` (default 30). The worker is killed | Flush after each reply; read one line at a time (a program that reads all of stdin, like `json.load(sys.stdin)`, waits forever); raise `timeout_secs` only if the conversion is slow by nature. |
| `protocol` | `format 输出违反信封协议:` ("format output violates the envelope protocol") | Your stdout was not exactly one valid envelope line (the worker is evicted), or its `body_b64` did not decode. The detail follows the colon; see the next table | Fix the output. |

`body 临时文件读取失败（本地磁盘故障）` is not a format failure: aProxy could not read its own
spooled body from disk, which returns `507` `local_storage_failed` with `details.op = read_request`;
check free space and permissions in the aProxy home directory.

Details after `format 输出违反信封协议:`:

| Detail begins with | Cause | Fix |
|---|---|---|
| `输出不是 UTF-8` ("output is not UTF-8") | stdout uses a legacy code page | Set stdout to UTF-8 (guide.md, Language pitfalls). |
| `信封 JSON 解析失败:` ("envelope JSON parse failed") | The line is not a valid envelope: a banner or log line came first, a blank line, a BOM, pretty-printed JSON, a wrong type, or ``missing field `headers` `` | Write only compact envelopes to stdout; always include `headers`. |
| `body 与 body_b64 互斥，不能同时出现` ("body and body_b64 are mutually exclusive") | Both fields in one reply | Send one of them. |
| `body_b64 解码失败:` ("body_b64 decode failed") | Not standard padded base64 (URL-safe alphabet, line breaks) | Use the standard encoder (protocol.md, body and body_b64). |
| `对单个请求输出了不止一行` ("more than one line for a single request") | Extra output after the reply: debug prints, `jq` without `-c`, a reply written twice | One line per request; diagnostics to stderr. |

A related warning without a failed request: `空闲 worker 的 stdout 冒出了未被请求的输出`
("an idle worker printed output nobody asked for"). The worker printed something between
requests, typically from a background thread; aProxy evicted it and used a fresh worker. Remove
the stray output.

## Errors from aproxy-format

On the request side these are the `<reason>` in the `422 format_rejected` message; on the response
side they appear in the `响应转换失败` log line while the client gets the unconverted response.

| Message | Cause | Fix |
|---|---|---|
| `未提供聚合配置（--config 或 transform extra 均为空）` ("no aggregation config: --config and extra are both empty") | `args` for this entry has no `--config`, and the envelope `extra` is empty (which it always is when configured from config.toml) | Add `"--config", "<aggregation config path>"` to the `args` of every aproxy-format entry. |
| `聚合配置加载失败: 读取 <path>: ...` ("failed to load aggregation config: reading <path>") | File missing or unreadable | Use an absolute path or one starting with `~/`. |
| `聚合配置加载失败: 解析失败: ...` ("... parse failed") | TOML syntax error, missing `client_format`, or an unknown protocol name (`unknown variant`) | Protocol names are `anthropic_messages`, `openai_chat`, `openai_responses` (and `auto` for `client_format`). |
| `聚合配置加载失败: 至少需要一个 [[channel]]` | No channels | Add a `[[channel]]` table. |
| `聚合配置加载失败: 渠道 <name> 的 keys 不能为空` | Empty `keys` | Give the channel at least one key. |
| `聚合配置加载失败: 渠道 <name> 的 weights 长度 (N) 必须与 keys (M) 一致` | `strategy = "weighted"` without one weight per key | Add `weights` of the same length, or drop `strategy`. |
| `聚合配置加载失败: 渠道 <name> 的 weights 不能含 0` | A zero weight | Use positive weights; remove the key instead. |
| `body 非 JSON` / `body 缺少 model 字段` ("body is not JSON" / "body has no model field") | A request without a JSON body naming a model, such as `GET /v1/models` | Expected for such requests on an aggregating instance; point clients that need them at another instance. |
| `model <name> 未命中任何渠道（检查渠道表的 models 模式）` ("model matches no channel") | No channel's `models` pattern matches the model after `[models]` mapping | Fix the patterns (case-sensitive, matched against the upstream name), add a catch-all channel without `models`, or check for a misspelled key. |
| `client_format=auto 检测失败：请求形态不像已知协议（检查字段名）` ("auto detection failed") | The body does not look like any supported protocol | Declare `client_format` explicitly. |
| `client_format = "auto" 只支持同协议：...` ("auto supports same-protocol only") | Under `auto`, the request was routed to a channel of another protocol; rejected before reaching the upstream. Also happens when an Anthropic request without `system` is detected as `openai_chat` | Declare `client_format` explicitly. |
| `协议转换失败: ...` ("protocol conversion failed") | The converter could not express this request or response in the other protocol | Route this model to a same-protocol channel. |
| `url <url> 反查不到渠道（检查渠道表的 url 配置）` ("url matches no channel") | Response side: the request went to a URL no channel `url` prefixes, usually because the request and response entries use different `--config` files | Give both entries the same `--config` path, and restart after editing the config. |
| `SSE 流式响应的跨协议转换尚未支持（渠道协议 <a> ≠ 客户端协议 <b>）` ("cross-protocol SSE conversion is not supported") | A streaming response from a channel of another protocol; the client receives the upstream stream unchanged and cannot parse it | Route streaming clients to a channel of their own protocol (examples.md, Limits). |
| `响应 body 非 JSON` ("response body is not JSON") | The upstream answered with a non-JSON, non-SSE body that aProxy accepted as success | Check the channel URL; it may point at a web page rather than the API. |

## Symptoms without an error

| Symptom | Cause | Fix |
|---|---|---|
| Responses reach the client unconverted | No `[[format]]` entry has the `response` stage; or the response format failed (look for `响应转换失败` in the log); or the response was an upstream error, which is never transformed | Add a `response` entry; read the logged reason. |
| A response header change takes effect only sometimes | When aProxy has already sent keepalive headers to a waiting client, only the body change applies | Expected; do not rely on response headers for keepalive-eligible streaming requests. |
| Requests hang after enabling a transformer, and the log repeats `上游返回可重试状态码，重试` ("upstream returned a retryable status; retrying") or `上游返回错误内容，重试` ("upstream returned an error body; retrying") | The upstream rejects the transformed request (bad key, wrong model name, malformed body, wrong path). aProxy retries 4xx like any failure, always with the same transformed request | Read the upstream error preview logged next to that line, fix the conversion, and test it with `scripts/test_format.py`. A malformed `url` shows up as repeated network errors instead. |
| Rotation always uses the first key | `process` is not `persistent` (a misspelling such as `mode` is now `config_unknown_key`, so the instance would not start instead of silently defaulting) | `process = "persistent"`. |
| Some keys are used more than others | Each persistent worker counts from the first key on its own; idle reaping restarts counters | Offset by `worker_id` in your own program (examples.md, Key rotation); the official binary does not. |
| Conversions are slow under load, one at a time | `spawn` process runs one conversion at a time per stage | Switch to `persistent`; raise `pool_max` if messages queue. |
| A transformer you configured never runs, and startup prints no transformer line | `command` is blank, which is now a `config_invalid_value` error: the entry is not silently dropped | Set `command`. |
| The instance refuses to start with `config_conflict` on `delivery` | `delivery = "forward"` is set for the instance, possibly inherited from settings.json `defaults.delivery` | Set `delivery = "retry"` in this instance's config.toml, or remove the format. |
| The instance refuses to start with `config_invalid_value` on `format[N].pool_max`: `pool_max must be at least 1` | `pool_max = 0` | Use 1 or more. |
| Non-ASCII text arrives garbled, or the program raises `UnicodeEncodeError` | The program's pipes use the system code page | Set UTF-8 on stdin and stdout (guide.md, Language pitfalls). |
| Format processes keep running after the instance stopped | The program ignores EOF, and aProxy exited without cleaning up (crash or forced stop) | Make the loop exit on EOF. End the leftovers by PID after confirming they are the format program; never kill by image name, which would also hit `aproxy` instances that agent sessions depend on. |
| The client receives an upstream key in a response header | The response side copied headers that contain credentials | Rebuild or allow-list response headers (protocol.md, Security). |
