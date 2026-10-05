# Connecting agent clients

How to route an agent client (Claude Code, Codex, Gemini CLI and others) through an aProxy instance so
its requests survive upstream failures, and which client timeout has to be raised first. Read it when
setting up a client, or when a client disconnects during long waits. aProxy's side of the story is in
behaviors.md; symptoms are in troubleshooting.md.

## Setup checklist

1. **Find the instance.** Run `aproxy status` and note the port (`监听 http://…`, "listening on") and the
   upstream (`上游 …`). If no instance serves the upstream this client should use, configure and start
   one first (commands.md, config-toml.md).
2. **Point the client at it, with each path prefix in one place.** aProxy forwards to `base_url`
   followed by the client's path exactly as received. Set aProxy's `base_url` to the upstream's root
   and give the client the aProxy address where it would otherwise name that root:

   | Client | Client setting | aProxy `base_url` |
   |---|---|---|
   | Claude Code | `ANTHROPIC_BASE_URL=http://127.0.0.1:<port>` | `https://api.anthropic.com`, or the relay's root |
   | Codex | provider `base_url = "http://127.0.0.1:<port>/v1"` | `https://api.openai.com` (no `/v1`) |
   | Gemini CLI | `GOOGLE_GEMINI_BASE_URL=http://127.0.0.1:<port>` | `https://generativelanguage.googleapis.com` |

   If the upstream lives under its own prefix (a relay serving `https://relay.example/api/v1/...`), put
   that prefix in `base_url` and leave the client's usual path alone.
3. **Decide where the key lives.** Without `api_key` in aProxy, the client's own credentials pass
   through untouched. With `api_key` set, aProxy overwrites `Authorization` (as `Bearer <key>`) and
   `x-api-key` on every request, and the client can use any non-empty placeholder. Gemini clients send
   their key as `x-goog-api-key`, which `api_key` does not touch: give the client the real key, or set
   `x-goog-api-key` in `override_headers`.
4. **Raise or disable the client's event-level stream idle timeout** (next section, then the entry
   for your client). This is the step most setups miss, and it only shows up during long waits.
5. **Restart the client** so it reads the new settings, then verify (last section).

Do not test by stopping or restarting an instance that other sessions use, possibly including your
own: it cuts their in-flight requests (behaviors.md, Stopping).

## Why the stream idle timeout matters

To retry transparently even when a stream breaks halfway, aProxy buffers each upstream response
completely and replays it only after validating it. Until then a streaming client receives a response
head and SSE comment heartbeats, `: keepalive`, once per `keepalive_interval_secs` (behaviors.md,
"Keepalive heartbeats"). What happens next depends on what the client's idle timer counts:

- **Byte-level timers** (time between received bytes, such as undici `bodyTimeout`, the httpx read
  timeout, OpenCode `chunkTimeout`) are reset by every heartbeat. Nothing to change.
- **Event-level timers** (time between parsed SSE events) are not. Common SSE parsers
  (eventsource-stream, eventsource-parser, the decoders in the official OpenAI and Anthropic SDKs) drop
  comment lines before the application sees them. When such a timer expires during a long retry period
  or a long generation, the client disconnects and resends; aProxy aborts the old upstream request and
  starts the new one from scratch, and once the client's resend budget is spent the turn fails.

Set the event-level timeout longer than any outage you want to ride out, or disable it. The sign that
this is missing is the log warning `客户端在只收到心跳的等待中断开（已等约 N 秒）…` ("client disconnected
while receiving only heartbeats, after about N s"); N approximates the client's effective timeout.

Two more client-side effects to plan for:

- **Requests without heartbeats** (`"stream": false` calls, and streaming requests that match no
  `keepalive_trigger` test, such as Gemini CLI's) receive nothing until the final answer is complete.
  The client's overall HTTP timeout must cover retries plus generation.
- **Client retries stack on aProxy's.** Each client resend is a new request to aProxy and cancels the
  previous one's upstream attempt, so a client that resends often keeps restarting aProxy's retry
  schedule.

## Claude Code

Scope: black-box tested with Claude Code 2.1.288 through aProxy, consistent with Claude Code's
documentation of its streaming watchdogs.

```sh
export ANTHROPIC_BASE_URL=http://127.0.0.1:12345
export CLAUDE_STREAM_IDLE_TIMEOUT_MS=86400000
```

```powershell
$env:ANTHROPIC_BASE_URL = "http://127.0.0.1:12345"
$env:CLAUDE_STREAM_IDLE_TIMEOUT_MS = "86400000"
```

To make it permanent, add the same variables to the `env` object in `~/.claude/settings.json` (merge
into the existing file rather than overwriting it). Settings-file values reach every session,
including background agents, which may not inherit your shell's environment:

```json
{
  "env": {
    "ANTHROPIC_BASE_URL": "http://127.0.0.1:12345",
    "CLAUDE_STREAM_IDLE_TIMEOUT_MS": "86400000"
  }
}
```

- With default settings, Claude Code abandons a stream that has carried only heartbeats after about
  600 s and resends. Its documentation calls this the event-level watchdog: 300 s by default, which
  arriving bytes can postpone for about five more minutes. Heartbeat comments and `event: ping` only
  postpone it; real content events reset it.
- `CLAUDE_STREAM_IDLE_TIMEOUT_MS` controls it; `86400000` (24 h) was verified to keep a heartbeat-only
  wait alive well past 600 s. Claude Code raises values below 5 minutes to 5 minutes and caps its
  byte-level watchdog at 30 minutes, which does not matter here because heartbeats keep bytes flowing.
  `API_TIMEOUT_MS` does not control this watchdog.
- The main request is `POST /v1/messages?beta=true` with `Accept: application/json` and
  `"stream": true` in the body. It gets heartbeats with `keepalive_trigger` set to `any` (the default)
  or `body_stream`; `accept` leaves it without.
- Non-streaming calls such as `POST /v1/messages/count_tokens` get no heartbeats and are bounded by
  `API_TIMEOUT_MS` (per Claude Code's documentation: default 600000, maximum 2147483647). Raise it if
  such calls time out during long upstream outages.
- With 2.1.288 and default aProxy settings, an upstream that failed for 757 s and then generated for
  140 s produced a complete answer with no client resend.
- Behind relays that lack `count_tokens`, `/compact` can hang; see troubleshooting.md.

## Codex

Scope: codex-cli 0.160.0, source and black-box tested through aProxy.

Define your own provider in `~/.codex/config.toml` (or `$CODEX_HOME/config.toml`); built-in provider
IDs such as `openai` cannot be redefined:

```toml
model = "<model name your upstream serves>"
model_provider = "aproxy"

[model_providers.aproxy]
name = "aproxy"
base_url = "http://127.0.0.1:12345/v1"
env_key = "OPENAI_API_KEY"         # Codex reads the key from this variable; any non-empty value if aProxy sets api_key
wire_api = "responses"             # the only value Codex accepts
stream_idle_timeout_ms = 86400000  # 24 h
```

- `stream_idle_timeout_ms` defaults to 300000 and measures time between SSE events (eventsource-stream
  drops comments), so heartbeats do not reset it. On expiry Codex reports
  `idle timeout waiting for SSE`, shows `Reconnecting... n/5` and resends up to `stream_max_retries`
  times (default 5, maximum 100); then the turn fails.
- Measured with default aProxy settings and a failing upstream: at 60000, Codex disconnected exactly
  60 s after the skeleton head despite four heartbeats in between; at 86400000, a 620 s heartbeat-only
  wait ended in a complete answer from a single request.
- Requests are `POST <base_url>/responses` with `Accept: text/event-stream` and `"stream": true`, so
  every `keepalive_trigger` value gives them heartbeats.
- Leave `supports_websockets` off (the default for custom providers): aProxy proxies HTTP only and does
  not pass WebSocket upgrades.

## Gemini CLI

Scope: Gemini CLI 0.35.3 tested through aProxy against a mock upstream; timeout values read from the
source of 0.35.3 and 0.62.0.

- Use API-key authentication: `GOOGLE_GEMINI_BASE_URL=http://127.0.0.1:<port>` plus `GEMINI_API_KEY`.
  OAuth and Code Assist logins do not use a custom base URL and cannot go through aProxy.
- The key travels as `x-goog-api-key`, which aProxy's `api_key` does not set (Setup checklist, step 3).
- Streaming requests are `POST /v1beta/models/<model>:streamGenerateContent?alt=sse` with `Accept: */*`
  and no `stream` field, so no `keepalive_trigger` value gives them heartbeats. aProxy buffers them and
  replays each response whole, which works.
- Limit: Gemini CLI's HTTP timeouts are hard-coded with no user setting. In 0.62.0 the response-header
  timeout is 60 s and the body timeout 300 s (both 300 s in 0.35.3). aProxy sends the response head only
  after a complete successful attempt, so any request whose retries plus generation take longer than
  the header timeout (over a minute on current versions) fails on the client side, which then resends a
  limited number of times. This follows from the source and was not run against a real slow upstream.
  aProxy has no remedy for it today.
- Do not try to force comment heartbeats onto it. Gemini CLI pins `@google/genai` 1.30.0 (still the
  case in 0.62.0), whose SSE splitter stops matching after a comment line, swallows the rest of the
  response and fails with `Incomplete JSON segment at the end`. Measured directly: a comment prefix
  failed after 4 requests, a blank-line prefix worked. aProxy has no setting to trigger heartbeats by
  path or to send blank-line heartbeats.

## Other clients

Scope: from reading each client's source at the version shown; not run against aProxy. "Nothing" means
the client only has byte-level timers, which heartbeats reset.

| Client (version read) | Stream timeout | What to set |
|---|---|---|
| Qwen Code 0.24.7 | Event-level idle 240 s, plus a 15-minute cap on each streaming response | Both environment variables `QWEN_STREAM_IDLE_TIMEOUT_MS=0` and `QWEN_STREAM_MAX_LIFETIME_MS=0`. Current Qwen Code docs also offer a per-model `generationConfig.streamIdleTimeoutMs` for the first; for the lifetime cap they give only the environment variable |
| dsh (DeepSeek Harness), `llm-pi-ai` adapter for OpenAI/Anthropic-compatible gateways | Event-level 300 s | The provider's `streamIdleTimeoutMs`, for example `172800000`, in `$DSH_HOME/profiles/<profile>/cordis.patch.yml` |
| dsh, `llm-deepseek` adapter | Event-level, but comments reset it | Nothing |
| pi 1.0.2 (`@earendil-works/pi-coding-agent`) | Byte-level 300 s (`httpIdleTimeoutMs`) | Nothing. Its Anthropic path sends `Accept: application/json` and gets heartbeats through `"stream": true` |
| OpenCode 1.18.x | Byte-level `chunkTimeout` and `headerTimeout`, 300 s each | Nothing. Do not set `timeout`: it bounds the whole request, waiting included |
| OpenCode 1.2.x | None | Only the provider's `baseURL` |
| Aider 0.86, Kimi CLI 1.52 | httpx read timeout 600 s, byte-level | Nothing |
| Cline 4.1 | Runtime fetch default of 5 minutes, byte-level | Nothing |
| Roo Code 3.54 | `roo-cline.apiRequestTimeout`, 600 s | Nothing (0 disables it) |

If a client listed as "Nothing" still drops during long waits, suspect this table first: look for the
disconnect warning in the log and check the client's current source.

## Browser and Electron clients

They send an `Origin` header, which aProxy rejects by default with a local 403 that is never forwarded:
`aProxy 拒绝了该请求（403，未转发上游）：来自浏览器页面的请求（Origin「…」）默认不放行，…` ("aProxy rejected the
request (403, not forwarded): requests from browser pages are not allowed by default"). The check stops
a web page from spending the user's key through the local proxy. If the user trusts the client, add
the exact Origin from the message to `allowed_origins` (config-toml.md) and restart the instance.

## Verify the setup

1. Send one request from the client, then run `aproxy status`: the instance's `请求 N` ("requests")
   count rises. If it does not, the client is not using aProxy: its settings did not load (restart the
   client), the port is wrong, or Codex is still on a built-in provider.
2. Read the instance's log (troubleshooting.md, Before you change anything, explains how). Each
   request logs `代理请求` ("proxied request") with a `target=` field holding the full upstream URL. A
   doubled prefix such as `/v1/v1/` means the prefix is set on both sides.
3. Repeated `上游返回可重试状态码，重试` lines with an `错误响应预览` preview mean the upstream rejects the
   request (key, model, path). aProxy keeps retrying such errors, so fix the cause instead of waiting.
4. The idle-timeout setting is only exercised by a long outage or generation. If
   `客户端在只收到心跳的等待中断开（已等约 N 秒）` appears in the log later, the client's event-level timeout
   is still active.
