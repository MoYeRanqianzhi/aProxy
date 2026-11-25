# config.toml reference: keepalive, retry and delivery

Part of [config-toml.md](../config-toml.md), which lists the other parts. `keepalive`, `retry` rules and `delivery`.

## keepalive

While aProxy waits for and retries a request that qualifies for keepalive, it sends the client
response headers early and then a heartbeat (by default the SSE comment `: keepalive`) every
`interval_secs`; the body is still buffered and replayed only after a successful attempt
(behaviors.md, "Keepalive heartbeats").

```toml
[keepalive]
interval_secs = 15
trigger = "any"
heartbeat = ": keepalive\n\n"
```

- `interval_secs` (default 15): the heartbeat period, and also how long aProxy waits for upstream
  headers before sending its own. Keep it below the shortest byte-level idle timeout between
  client and aProxy. `0` turns keepalive off for the instance. CLI: `aproxy config set
  keepalive.interval_secs N` writes this key.
- `trigger` (default `"any"`, or settings.json `defaults.keepalive.trigger`): which
  requests qualify. Set it with `aproxy config set keepalive.trigger VALUE`.

| Value | A request qualifies when |
|---|---|
| `"accept"` | the client's `Accept` header contains `text/event-stream` |
| `"body_stream"` | the client's body is a JSON object with top-level `"stream": true` |
| `"any"` | either holds |

Keep `"any"`. Claude Code's streaming requests send `Accept: application/json` with
`"stream": true`, so `"accept"` alone would leave them without heartbeats. Switch an instance to
`"accept"` when its upstream answers `"stream": true` with something other than SSE (Ollama's native
API streams NDJSON, for example) and often needs retries: once aProxy has sent SSE headers during a
retry, a non-SSE body no longer fits the response. `"always"` cannot be written in the file: it
would put SSE headers on every request and break non-streaming clients, so only a format program
may ask for it per request.

- The decision uses what the client sent, before any format program.
- `heartbeat` (default `": keepalive\n\n"`): the bytes written on every tick, as they are. aProxy
  knows no protocol, so pick what the client tolerates: a blank line (`"\n"`) for a client whose
  SSE parser chokes on comments, or a whole event such as `"event: ping\ndata: {}\n\n"`. A
  heartbeat does not reset an event-level idle timeout unless the client counts it as an event;
  Claude Code drops both comments and `ping` events, so raise its timeout instead (clients.md).
  The value must leave the client's SSE parser at an event boundary, or the heartbeat merges with
  the first replayed event: it must be non-empty, end with a line break, and, if it has any field
  line (`event:`, `data:` and so on), end with a blank line. `aproxy config show` prints it with
  `\n` escapes. Set it with `aproxy config set keepalive.heartbeat VALUE` and a value starting with
  `"` when it must contain newlines.
- No keepalive for requests that do not qualify (notably `"stream": false`: the client's own
  timeout must cover the whole generation), for `delivery = "forward"` instances, and when
  `interval_secs = 0`.
- Heartbeats defeat byte-level idle timeouts only. Clients that time out on SSE events need their
  own timeout raised (clients.md).

## retry

Retry pacing and the rules that cap retries for specific upstream failures.

```toml
[retry]
max_backoff_secs = 320

[[retry.rules]]
path = '/v1/messages/count_tokens(\?.*)?'
max_attempts = 3
```

`max_backoff_secs` (default 320) is the longest wait between retries. The first three retries are
immediate; from the fourth the wait is 5 s and doubles (10, 20, 40 ...) until it reaches this cap,
then stays there; a cap below 5 makes every delayed retry wait exactly the cap. Retries never
stop; the cap only sets how often aProxy tries. `0` makes every retry immediate, which hammers an
upstream that is down; for fast recovery prefer a small cap such as 10.

`[[retry.rules]]` describe failures that should stop retrying. A rule has these keys:

| Key | Type | Default | Meaning |
|---|---|---|---|
| `path` | string (regex) | unset = any path | Matched against the client's `path?query` as a whole; anchored at both ends automatically |
| `status` | array of integers | unset = any status | HTTP status codes 100-599 |
| `max_attempts` | integer | required | Attempt N that fails with an upstream response and matches this rule stops when N ≥ `max_attempts`; `0` = no limit |

- Rules are matched in order. The first rule whose matchers all hold decides; a rule with
  `max_attempts = 0` is an exemption and keeps retrying, so place it before a wider rule.
- Only failures that came with an upstream response are capped: an error status, or a 2xx carrying
  an error body. Network failures (connect, timeout, interrupted body) are always retried without
  limit, because there is no response to pass on.
- When a rule stops a request, the last upstream response goes to the client unchanged (status,
  headers, body), plus `x-aproxy-error: retry_exhausted` and `x-aproxy-request-id`. The error
  detail `by` is `rule:<1-based index>`, and `last_code` is the failure code of the last attempt.
- If heartbeat headers were already committed, the client gets the terminal SSE error frame with
  code `retry_exhausted` instead, because the real status line can no longer be sent.
- `path` is a regular expression over the real request target, percent-encoding included and
  before any format program. Write a literal `?` as `\?`; escape regex metacharacters when you
  mean the character. Matching is case-sensitive. Logs mask query values (`?beta=***`), but
  matching sees the real ones.
- A `body` matcher is reserved for a later release: writing it reports `config_not_supported`.
  An invalid regex, a status outside 100-599, or a missing `max_attempts` reports an error and the
  whole rule is dropped, because a rule with a matcher silently removed would be wider than
  written.
- An empty rule list (the default) means every request retries without limit. The settings.json
  `defaults.retry.rules` global default is inherited as a whole when config.toml has no rules;
  config.toml rules replace that array when present.

Use rules when an upstream deterministically fails an endpoint the client depends on, so the client
gets the real error instead of waiting forever. The typical case: Claude Code behind a relay or
mirror API that does not implement `POST /v1/messages/count_tokens`, so `/compact` hangs until the
client times out. Which endpoints need this depends entirely on the upstream, so aProxy ships no
rules.

```toml
[[retry.rules]]
path = '/v1/messages/count_tokens(\?.*)?'
max_attempts = 3

# Exempt 429 from a wider rule below it
[[retry.rules]]
status = [429]
max_attempts = 0

[[retry.rules]]
status = [400, 404, 410]
max_attempts = 2
```

When a rule stops a request, the log says `重试规则给的尝试次数用完，不再重试` ("the attempt count
given by the retry rule is used up; not retrying"), with the rule's index and the last failure
code. Look for it to confirm a rule matched. Runtime details: behaviors.md, "Retry rules".

## delivery

The delivery mode every request of this instance starts in.

- `"retry"` (the default, or settings.json `defaults.delivery`): the request body is read in full,
  each upstream response is buffered and judged, failures are retried without limit, and waiting
  streaming clients get heartbeats. This is the guarantee aProxy exists for.
- `"forward"`: request and response bytes are relayed as they arrive, with no retries and no
  heartbeats. An upstream failure becomes a `502` `upstream_failed`, and a stream that breaks
  mid-way is cut off where it broke. Choose it only for a trusted upstream when the client needs
  true incremental streaming.

The configured value is only the starting point. A format program may change the mode of a single
request (its `request` reply can carry `delivery`), and a `forward` request that fails before any
byte reached the client can be upgraded to `retry` by a program that declares `decide`. The
recommended setup therefore keeps `"retry"` here and lets the official `adaptive` role move
requests that have not failed before to `forward`: if that program breaks, everything falls back to
retrying. With `delivery = "forward"` as the default, a broken or absent decision program means
requests are simply not retried; `aproxy doctor` warns about that combination
(`forward_default_unguarded`).

In forward mode:

- Still applied: `api_key`, `extra_headers`, `override_headers`, the inbound checks, and
  `max_request_mb` (counted while streaming).
- Not used: `keepalive.*` and `retry.*`. `disk_cache` and `max_response_mb` matter only where a
  format forces buffering (below).
- A 200 whose body is an error object is still passed on unchanged, but it is recorded as a failed
  request (`upstream_error_body`) in the request history.

Forward mode and `[[format]]` can be combined:

- The request body streams straight to the upstream unless some program needs it: one that asks for
  the body or its digest at the `request` stage, or any program that declares `decide` (an upgrade
  to `retry` has to resend the body). Then aProxy reads the whole body first.
- The response streams through programs that declare `stream`, converted chunk by chunk. A program
  that declares `response` needs the whole response, so that layer buffers it and the client loses
  incremental streaming for those responses (`aproxy doctor`: `forward_response_buffered`).
- Only 2xx responses go through `response` and `stream`; other statuses pass through as they are.

Set it with `aproxy config set delivery "forward"`. When it is on, `aproxy start` prints
`仅转发模式：不缓冲、不重试` ("forward-only mode: no buffering, no retries").

Runtime details: behaviors.md, "Forward-only mode".
