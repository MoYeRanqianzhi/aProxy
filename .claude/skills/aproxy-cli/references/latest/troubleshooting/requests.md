# Troubleshooting: requests

Part of [troubleshooting.md](../troubleshooting.md), which lists the other parts. Requests are slow, hang, fail, or the client disconnects or cannot parse the response.

## Slow, hanging or failing requests

| Symptom | Cause | Fix |
|---|---|---|
| The client waits a long time before answering | Normal during upstream trouble: aProxy is retrying and heartbeats hold the connection | Nothing, or see why: `aproxy status` (`重试`, `最近错误`) or the log (`上游返回可重试状态码，重试`, `上游网络错误，重试`) |
| Every request hangs; the log repeats the same 4xx followed by `错误响应预览` ("error response preview") | A deterministic rejection (bad key, unknown model, wrong path) is retried forever by design | Read the upstream's message in the preview, fix `api_key`, `base_url` or the client's model, then `aproxy restart <port\|alias>`. For an endpoint this upstream never supports, add a retry rule (next rows) |
| The log's `target=` shows a doubled prefix (`/v1/v1/…`) and the upstream answers 404 | The path prefix is set both in `base_url` and in the client | Keep it on one side (clients.md, Setup checklist) |
| Claude Code `/compact` (or another command) never finishes behind a relay or mirror API | The upstream does not implement `POST /v1/messages/count_tokens`; its deterministic 404 is retried forever | In that instance's config.toml add a retry rule and run `aproxy restart <port\|alias>`; the failure then reaches the client after three attempts: `[[retry.rules]]` with `path = '/v1/messages/count_tokens(\?.*)?'` and `max_attempts = 3`. The pattern must cover the query string Claude Code sends (`?beta=true`): `'/v1/messages/count_tokens'` alone does not match it. For other endpoints copy the path from the log's `path=`; query values show as `***` there, but the pattern is matched against the real values |
| Log `重试规则给的尝试次数用完，不再重试` ("the attempt count given by the retry rule is used up; not retrying") | Expected for a request matched by a `[[retry.rules]]` entry | If that endpoint should retry without limit, remove the rule and restart |
| A non-streaming request times out at the client | Non-streaming requests get no heartbeats, so the client sees nothing until the end | Raise the client's overall timeout (Claude Code: `API_TIMEOUT_MS`). A lower `retry.max_backoff_secs` also helps: requests finish sooner once the upstream recovers |
| A streaming request gets no heartbeat, or its first byte comes late | `keepalive.trigger = "accept"` excludes clients that send `Accept: application/json` with `"stream": true` (Claude Code); or `keepalive.interval_secs = 0`; or forward-only mode | Use `keepalive.trigger = "any"`. Check the effective value with `aproxy config show --effective` (it names the layer) and the settings.json `defaults.keepalive.trigger` default, then restart |
| The client got `text/event-stream` with a JSON or NDJSON body; the log warns `已以 text/event-stream 提交的保活响应里回放的是非 SSE 的成功响应体…` | The upstream answered a stream request with a non-SSE body after the skeleton head was committed | Set `[keepalive] trigger = "accept"` on that instance |
| The client receives an SSE `event: error` frame, or an HTTP response with `x-aproxy-error` | aProxy ended the request, after the response head was already committed or by giving up | Read the code from `error.aproxy.code` or the header; behaviors.md, After the commit lists each code and its status. Most fixes are the rows below |
| 502 with `x-aproxy-error: response_too_large` (`details.limit_bytes`) | Response larger than `max_response_mb` | Raise `max_response_mb` |
| 507 with `x-aproxy-error: local_storage_failed` (`details.op` is `write_request`, `read_request` or `write_response`) | Disk full, or the instance's directory under `<home>/spool/` not writable | Free space, or set `disk_cache = false` and restart |
| 413 with `x-aproxy-error: request_too_large` (`details.limit_bytes`) | Body larger than `max_request_mb` | Raise it, or `0` for no limit |
| 400 with `x-aproxy-error: client_body_aborted` | The client stopped sending its request body | Usually client-side; have the client resend |
| 403 with `x-aproxy-error: inbound_host_rejected` or `inbound_origin_rejected` (`details.host` / `details.origin`); log `入站请求被拒绝…` ("inbound request rejected") | Inbound check: browser and Electron clients send `Origin`, rejected by default; a non-default host name is rejected on loopback listeners | Add the exact value from `details` to `allowed_origins` or `allowed_hosts` (config-toml.md), only if the user trusts that client; restart |
| 501 with `x-aproxy-error: upgrade_unsupported` (`details.upgrade` is `websocket`) | The client tried a WebSocket upgrade; aProxy refuses it on the spot and does not forward or retry it. Other upgrades such as `h2c` are proxied normally | Use the client's HTTP/SSE mode, or point that client at the upstream directly; aProxy does not proxy WebSocket |
| 500 with `x-aproxy-error: internal` | An internal defect in aProxy | Resend; if it repeats, collect the log around it for a bug report |
| Upstream traffic goes through an unexpected proxy, or bypasses the expected one | Without `proxy` in config.toml, the instance uses `HTTPS_PROXY`/`HTTP_PROXY`/`ALL_PROXY`/`NO_PROXY` from the environment it was started (or respawned) in | Set `proxy` explicitly, which disables those variables, and restart |
| `aproxy status` shows the request count not moving while the client works | The client is not using aProxy | clients.md, Verify the setup |

## Client disconnects and parse errors

| Symptom | Cause | Fix |
|---|---|---|
| Log `客户端在只收到心跳的等待中断开（已等约 N 秒）…` ("client disconnected while receiving only heartbeats, after about N s") | The client's event-level stream idle timeout, about N s; comment heartbeats do not reset it | Raise or disable it (clients.md). Unless the user cancelled on purpose |
| Claude Code drops and resends after about 10 minutes of waiting | Its event-level watchdog | `CLAUDE_STREAM_IDLE_TIMEOUT_MS=86400000` in the shell or in `~/.claude/settings.json` `env`; `API_TIMEOUT_MS` does not control it (clients.md, Claude Code) |
| Codex prints `idle timeout waiting for SSE`, then `Reconnecting... n/5`, then fails the turn | Provider `stream_idle_timeout_ms` (300000 by default) counts SSE events | Set `stream_idle_timeout_ms = 86400000` under `[model_providers.<id>]` in `~/.codex/config.toml` (clients.md, Codex) |
| Qwen Code drops after about 4 minutes, or always at 15 minutes | Event-level idle timeout and a per-stream lifetime cap | `QWEN_STREAM_IDLE_TIMEOUT_MS=0` and `QWEN_STREAM_MAX_LIFETIME_MS=0` (from source, not run) |
| Gemini CLI fails with `Incomplete JSON segment at the end` | An SSE comment reached its pinned `@google/genai` 1.30.0 parser. aProxy never sends heartbeats to Gemini CLI's requests, so another hop in the chain inserted it | Remove or reconfigure the other proxy; do not try to enable heartbeats for Gemini CLI (clients.md, Gemini CLI) |
| Gemini CLI fails on long generations or during upstream outages | Its hard-coded response-header timeout (60 s in 0.62.0) runs out, because aProxy sends the head only after a complete successful attempt | No fix in aProxy or the client today (clients.md, Gemini CLI) |
