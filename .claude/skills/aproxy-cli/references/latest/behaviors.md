# Runtime behavior

What a running aProxy instance does with each request and how instances look after themselves:
retries, keepalive heartbeats, buffering, forward-only mode, format programs, instance identity, logs,
crash recovery, the watchdog and upgrades. Read it to explain or predict aProxy's behavior, or before
acting on a running instance. Field syntax and defaults are in config-toml.md and settings-json.md,
client setup in clients.md, symptom lookup in troubleshooting.md.

`<home>` below means `~/.aproxy`, or the directory named by `APROXY_HOME` when that is set.

## Contents

- [Request lifecycle](#request-lifecycle)
- [Retry decisions](#retry-decisions) (backoff, retry rules)
- [Keepalive heartbeats](#keepalive-heartbeats) (which requests, commit point, error responses, compression, disconnects)
- [Error detection in response bodies](#error-detection-in-response-bodies)
- [Disk cache (spool)](#disk-cache-spool)
- [Request history](#request-history) (what an instance remembers, memory use)
- [Forward-only mode](#forward-only-mode)
- [Format programs](#format-programs)
- [Instances, ports and the control channel](#instances-ports-and-the-control-channel) (stopping, isolated experiments)
- [Logs](#logs)
- [Crash recovery and the watchdog](#crash-recovery-and-the-watchdog)
- [Upgrades with aproxy install](#upgrades-with-aproxy-install) (skill documentation)
- [Download proxy versus request proxy](#download-proxy-versus-request-proxy)

## Request lifecycle

A client sets its API base URL to `http://127.0.0.1:<port>`. In the default mode every request goes
through these steps:

1. **Inbound check.** A request whose `Host` or `Origin` header is not allowed gets a local `403`
   (`x-aproxy-error: inbound_host_rejected` or `inbound_origin_rejected`) and is never forwarded or
   retried (config-toml.md, `allowed_hosts` / `allowed_origins`). CLI clients send no `Origin` and
   use `127.0.0.1` or `localhost` as host, so they pass.
2. **Header rewrite.** `api_key` replaces both `Authorization` (as `Bearer <key>`) and `x-api-key`;
   `override_headers` replace headers; `extra_headers` are added only when the client did not send
   them. Hop-by-hop headers (`Connection`, `Upgrade`, `Transfer-Encoding`, `Host`, ...) are dropped, so
   WebSocket upgrades do not pass through. Method, path, query and all other headers are unchanged.
3. **Body buffering.** The whole request body is read before anything goes upstream, so every retry
   can replay it. A body over `max_request_mb` is not sent upstream and gets `413`
   `x-aproxy-error: request_too_large`; aProxy first reads and
   discards the rest of the upload (for at most 10 s) so that the client can read the 413 instead of a
   reset connection.
4. **Keepalive decision**, made on the request as the client sent it (see Keepalive heartbeats).
5. **Request chain**, if `[[format]]` programs are configured: each program that declares
   `request` runs once, in list order. Retries replay the result unless the retry chain is rerun
   (see Format programs).
6. **Upstream URL** = `base_url` without a trailing `/`, followed by the client's path and query
   verbatim. aProxy adds nothing, so a path prefix such as `/v1` belongs on exactly one side: either in
   `base_url` or in the client's base URL, not both (clients.md, Setup checklist).
7. **Attempts.** Each attempt buffers the complete upstream response and then judges it (see Retry
   decisions). A failed attempt is discarded and retried; its bytes never reach the client.
8. **Replay.** The successful response, after the `response` or `stream` programs if any are configured, goes to
   the client with the upstream's status, headers and body bytes. Streaming responses are replayed as a
   chunked stream; compressed bodies are forwarded still compressed.

If the client disconnects at any step, aProxy aborts the in-flight upstream request and stops
retrying, so an abandoned request stops consuming tokens. A client that resends starts a new request
from step 1.

Redirects are not followed: a 3xx counts as success and is replayed to the client, because following
it would carry the key and body to whatever host the redirect names.

Forward-only mode replaces steps 3 to 8 with plain streaming; see Forward-only mode.

## Retry decisions

| Upstream outcome | Decision |
|---|---|
| Connect, send or read error, including `timeouts.connect_secs`, `timeouts.first_byte_secs` or `timeouts.read_idle_secs` expiring, and a body cut off midway | Retry |
| Status 400-599, every 4xx included | Retry |
| Status 2xx/3xx whose body is a JSON object with a non-null `error` field or `"type": "error"` | Retry |
| Streaming body in which any SSE `data:` payload or NDJSON line is such an error object | Retry |
| Response larger than `max_response_mb` | Stop with `502` `x-aproxy-error: response_too_large` (`details.limit_bytes`) |
| Local disk write or read failed after the response spilled to disk | Stop with `507` `x-aproxy-error: local_storage_failed` (`details.op` names the step) |
| A failure that came with a response and matches a `[[retry.rules]]` entry whose `max_attempts` is reached | Pass that response through (see Retry rules) |
| Anything with `delivery = "forward"` | No retry (see Forward-only mode) |

Otherwise retries are unlimited: the request ends when an attempt succeeds or the client gives up.
Retrying every 4xx is deliberate. Rate limits (429) and transient auth failures are common on real
upstreams, and while aProxy retries, the client sees a slow answer rather than an error. The price is
that a deterministic error (wrong key, wrong path, unknown model) is retried forever too, and the
client waits indefinitely. The log shows it as repeated `上游返回可重试状态码，重试` ("upstream returned a
retryable status, retrying") lines, each followed by `错误响应预览` ("error response preview") with the
upstream's own error body; fix the configuration, or add a retry rule.

When a request has to stop after a keepalive response was already committed, the client gets a
terminal SSE error frame instead of an HTTP error response (see After the commit).

### Backoff

The first three retries go out immediately. From the fourth, the delay doubles from 5 s (5, 10, 20,
40 s, ...) until it reaches `retry.max_backoff_secs` and then stays there; `0` makes every retry
immediate. Once an outage outlasts the ramp, aProxy probes the upstream once per cap interval, so a
recovered upstream can take up to one cap interval to be noticed. Lower the cap when quick recovery
matters more than load on the upstream.

### Retry rules

`[[retry.rules]]` (config-toml.md) stop retrying for failures that will not improve. A rule matches
on `path` (a regex over the client's `path?query`, anchored at both ends, before any format
program), `status` (a list of HTTP status codes), or both; `max_attempts` is required.

- Rules are checked in order. The first rule whose matchers all hold decides; a `max_attempts = 0`
  rule is an exemption and keeps retrying, so place it before a wider rule.
- The cap applies only to failures that came with an upstream response: an error status, or a 2xx
  carrying an error body. Attempt N failing that way stops when N ≥ `max_attempts`, and the last
  upstream response is passed to the client unchanged (status, headers, body). The pass-through adds
  only `x-aproxy-error: retry_exhausted` and `x-aproxy-request-id`, so it can be told apart from a
  normal response; the error detail `by` is `rule:<1-based index>` and `last_code` is the last
  attempt's failure code.
- Network errors are still retried without limit: they are transient and carry no response to pass on.
- A passed-through failure never goes through a response format.
- If a keepalive response was already committed when the rule stops the request, the client gets the
  terminal SSE frame with code `retry_exhausted` instead, because the real status line can no longer
  be sent.
- An invalid regex, a status outside 100-599, or a missing `max_attempts` reports an error and drops
  the whole rule, because a rule with a matcher silently removed is wider than written. A `body`
  matcher is reserved and reports `config_not_supported`.

The log says `重试规则给的尝试次数用完，不再重试` ("the attempt count given by the retry rule is used
up; not retrying"). Use rules for endpoints a particular upstream never supports; troubleshooting.md
has the Claude Code `count_tokens` case.

The settings.json `defaults.retry.rules` global default is used as the rule list when config.toml has
none; rules written in config.toml replace that array as a whole.

## Keepalive heartbeats

A streaming request can spend minutes in retries before any upstream byte is usable. Keepalive keeps
the client's connection visibly alive meanwhile: aProxy commits a response head early and then writes
a heartbeat every `keepalive.interval_secs`. By default that is the SSE comment line `: keepalive`,
which SSE parsers discard by specification; `keepalive.heartbeat` replaces it with any bytes that end
at an event boundary (config-toml.md).

### Which requests get heartbeats

All of the following must hold:

- `keepalive.interval_secs` is above 0 and the instance is not in forward-only mode;
- `keepalive.trigger` matches: `accept` means the `Accept` header contains `text/event-stream`;
  `body_stream` means the JSON body has a top-level `"stream": true`; `any` means either.

The trigger looks at the request as the client sent it, before any format program, because the
question is whether the client is waiting for a stream. Every other request (a non-streaming call, or
a streaming request that matches neither test, such as Gemini CLI's) receives nothing until its final
response is ready, so the client's own HTTP timeout must cover all retries plus generation.

### Commit point

Nothing is written before the commit, and the commit happens once per request:

| Before any commit | Result |
|---|---|
| An attempt returns a 2xx head with an uncompressed `text/event-stream` content type, and no format program declares `response`, `stream` or `decide` | The upstream's real status and headers go to the client at once (without `content-length` and hop-by-hop headers) |
| One keepalive interval passes without such a head | A skeleton head is committed (`200`, `content-type: text/event-stream`, `cache-control: no-cache`) and a heartbeat follows immediately; the first byte therefore arrives within one interval |
| An attempt succeeds first | Replayed exactly like a request without keepalive, real status and headers included |
| An attempt returns a 2xx head that is not SSE (NDJSON, plain JSON) | The skeleton commit is paused for that attempt, so the content type is not rewritten. Success is replayed unchanged; if the attempt needs a retry, the skeleton is committed right after. During such an attempt the client may receive nothing for up to `timeouts.read_idle_secs` |

A failed attempt does not trigger a commit by itself, so a request that succeeds after a few quick
retries still gets the upstream's real head.

### After the commit

- A heartbeat goes out every interval while aProxy waits for upstream headers, buffers an upstream
  body, waits between retries, or runs a format program or a decode.
- Status and headers can no longer change. The successful attempt's complete, validated body is
  written into the committed response; bytes of failed attempts never appear.
- A successful body that is not SSE is still written, since dropping a good result would be worse, and
  the log warns `已以 text/event-stream 提交的保活响应里回放的是非 SSE 的成功响应体（响应头已发出，无法撤回）…`
  ("a non-SSE success body was replayed into a keepalive response already committed as
  text/event-stream"). For an upstream that answers `"stream": true` with NDJSON and often needs
  retries, set `keepalive.trigger = "accept"` on that instance.
- A request that has to stop after the commit ends with one SSE frame: a line `event: error`, then
  `data: <error JSON>` on one line, then a blank line. Before the commit the same JSON is the whole
  response body, sent with `content-type: application/json` and the code's own status.

#### Error responses

When aProxy itself ends a request, every response has this shape:

```json
{"type":"error","error":{"type":"aproxy_<code>","message":"<one English sentence>",
 "aproxy":{"code":"<code>","request_id":"01a1e0eef745bd28-42","attempts":3,
 "upstream_status":503,"details":{}}}}
```

- Two headers carry the same values: `x-aproxy-error: <code>` and `x-aproxy-request-id: <id>`. A
  normally proxied response carries neither, so a response with `x-aproxy-error` is final: aProxy
  has given up and will not retry it. A response passed through after a retry rule stopped the
  request keeps the upstream's status and body byte for byte and gains only these two headers.
- Match on `error.aproxy.code` or on `x-aproxy-error`, never on `message`, which is a human sentence
  and not a contract. `attempts` counts the upstream attempts made, `upstream_status` appears only
  when there was an upstream response, and `details` holds the keys in the table.
- `request_id` is opaque: it looks like `01a1e0eef745bd28-42`, 16 hex digits of an instance id, a
  hyphen, and the request's sequence number in that instance. Requests refused by the inbound
  Host/Origin check end in `-x<n>` instead. It is the same string in the header, in the body, in the
  `request_id` given to format programs, and in every log line of the request, prefixed
  `request{request_id=<id>}:`.

| `code` | Status | When | `details` |
|---|---|---|---|
| `inbound_host_rejected` | 403 | `Host` is not allowed | `host` |
| `inbound_origin_rejected` | 403 | `Origin` is not allowed | `origin` |
| `request_too_large` | 413 | The request body exceeds `max_request_mb` | `limit_bytes` |
| `client_body_aborted` | 400 | The client stopped sending its request body | none |
| `local_storage_failed` | 507 | A spool file could not be written or read | `op` = `write_request`, `read_request` or `write_response` |
| `response_too_large` | 502 | The upstream response exceeds `max_response_mb` | `limit_bytes` |
| `retry_exhausted` | the upstream's, else 502 | A retry rule used its attempts up | `by` = `rule:<1-based index>`, `last_code` |
| `retry_stopped` | the response in hand, else 502 | A format's `decide` stage stopped the retries | `format`, `last_code` |
| `format_rejected` | the format's response, else 422 | A request-side format answered `fail` with `retry: false` | `format`, `stage`, `fail_code`? |
| `format_unavailable` | 503 | A required format's breaker is open at a request-side stage | `format`, `stage`, `kind`, `probe_at` (Unix ms) |
| `format_failed` | 502 | A forward request's request-side format failed or asked for a retry, and the request was not upgraded to retry | `format`, `stage`, `kind`?, `fail_code`? |
| `upstream_failed` | 502 | A forward request's upstream attempt failed and the request was not upgraded to retry | `last_code` |
| `decode_failed` | SSE frame only | The upstream compressed the body despite `identity` and decoding failed | `encoding` |
| `upgrade_unsupported` | 501 | A WebSocket upgrade request; it is refused on the spot, not forwarded or retried. Other upgrades such as `h2c` are ignored and proxied normally | `upgrade` |
| `internal` | 500 | A defect in aProxy | none |

Why one upstream attempt failed is a separate set of codes, shown as `last_code` and in logs:
`upstream_connect_failed`, `upstream_connect_timeout`, `upstream_first_byte_timeout`,
`upstream_read_idle_timeout`, `upstream_body_interrupted`, `upstream_status`,
`upstream_error_body`, and three that come from format programs: `format_error` (the program broke),
`format_fail` (it answered `fail` with `retry: true`) and `format_decision` (its `decide` stage judged
a successful attempt as failed). The full table is `docs/error-codes.md` in the repository.

### Compression

For requests that get heartbeats, aProxy sends `accept-encoding: identity` upstream, because plain-text
heartbeats cannot be inserted into a gzip or brotli stream. This overrides the client's value, a
request-side format's value and `override_headers`; when `override_headers` sets another
`accept-encoding`, startup logs `override_headers 里的 accept-encoding 对保活适用的请求不生效…`
("accept-encoding in override_headers does not apply to keepalive requests"). If the upstream
compresses anyway, its head is not committed, the skeleton goes out at the interval, and the
successful body is fully decoded before it is written. Requests without heartbeats keep the client's
`accept-encoding` and get the upstream's compressed bytes unchanged.

### Disconnects

Every disconnect on this path logs `客户端已断开，中止上游请求（保活通道）` ("client disconnected; aborting
the upstream request"). When the client drops after waiting at least 60 s on a committed response that
has carried only heartbeats, aProxy also warns `客户端在只收到心跳的等待中断开（已等约 N 秒）…` ("client
disconnected while receiving only heartbeats, after about N s"). Unless the user cancelled, that is
the client's own stream idle timeout: many clients time SSE events rather than bytes, and comments are
not events. The fix is on the client side; see clients.md.

## Error detection in response bodies

- A response counts as streaming when its content type is `text/event-stream` or
  `application/x-ndjson`, or when a body of at most 1 MiB contains `data:` lines.
- Streaming bodies are checked line by line: any SSE `data:` payload other than `[DONE]`, or any
  NDJSON line, that is an error object marks the attempt failed. Other bodies of at most 1 MiB are
  checked as one JSON document.
- Compressed bodies (`gzip`, `x-gzip`, `deflate`, `br`, `zstd`, stacked encodings too) are decoded
  into a separate copy of up to 8 MiB for checking and for log previews. The client still receives the
  original bytes.
- Limit: a response over 1 MiB is scanned as it spills to disk, on its raw bytes, so a compressed
  error body of that size is not recognized by content. Its status code still is.

## Disk cache (spool)

With `disk_cache` on, request bodies and responses larger than 1 MiB spill to
`<home>/spool/<pid>-<hash>/` (one directory per instance process) as `req-*.spooltmp` and
`spool-*.spooltmp` files. Retries and replays stream from the file, so the memory a request
needs while it is in flight does not grow with payload size; through the OS page cache an SSD
performs about as well as memory. (The request history below keeps its own copy of recent bodies,
within a fixed budget.)

- A temp file is deleted when its request finishes, when its attempt is retried, or when the client
  disconnects. Files left by a crash are deleted the next time an instance starts on that port; after
  a clean exit the directory is empty.
- If the file cannot be created when a body crosses 1 MiB, buffering continues in memory. If a write
  or read after the body is already on disk fails (disk full, permissions), the request stops with
  `507` `local_storage_failed` (`details.op` names the step) and is not retried.
- With `disk_cache = false` everything stays in memory; for payloads under 1 MiB both modes behave
  the same.

## Request history

Each instance remembers the requests it handled, in memory: the last 1000 records, plus the bodies
of the most recent ones within `[history] body_budget_mb` (default 64 MiB, `0` = no bodies). A
record holds the method, path, model, every upstream attempt with its failure and the retry
decision, heartbeats sent, byte counts and the outcome. Forward-only instances record requests too.

- Memory: an instance that has seen enough traffic sits at its baseline plus up to the budget plus
  about 16 MiB, and stays there. That is the history, not a leak. `body_budget_mb = 0` (then
  restart) brings it back to the baseline.
- Known secrets (the API key, proxy password, secret header and `[[format]]` values) are replaced
  before anything is stored; secrets aProxy cannot know about that appear inside a body are kept as
  sent. Set the budget to `0` when body content must not be retained.
- The history is per instance and is gone after `stop`, `restart` or a crash.
- No command prints it in this version; it is served over the control channel to the user who
  started the instance. Details and the full option: config-toml.md, "history".

## Forward-only mode

A request in `forward` delivery is a plain streaming pass-through: no retries and no heartbeats. An
instance starts all its requests that way with `delivery = "forward"`; with the default
`delivery = "retry"`, a format program can still move single requests to `forward` (the official
`adaptive` role does that for requests that have not failed before). Make `forward` the instance
default only when the upstream is trusted and the client copes with upstream errors itself, because
it gives up the retry guarantee aProxy exists for.

- Still applied: the inbound check, the header rewrite, and `max_request_mb`, counted while the body
  streams; over the limit the upstream request is aborted and the client gets 413.
- The request body streams upstream chunked, unless a format program needs the body or its digest
  at the `request` stage or some program declares `decide`; then it is read in full first. The
  response streams back with the upstream's status and headers; `content-length` is kept unless
  the upstream used `transfer-encoding`.
- Upstream request fails before any byte reached the client: programs that declare `decide` are
  asked once. Answering `retry` upgrades the request to `retry` delivery for the rest of its life
  (heartbeats and retry rules apply from then on). Otherwise the upstream's response is passed
  through, or, when there is none, the client gets `502` `upstream_failed`, whose message ends
  `(this request is not retried)` and whose `details.last_code` says why the attempt failed. It is
  recorded as the latest error in `aproxy status` as `<code>: <English message>`.
- Response stream breaks after bytes were written: no upgrade is possible. The connection ends
  there, and aProxy injects nothing the upstream did not send; only a `stream` program may write a
  closing event in the client's protocol.
- A 200 whose body is an error object is passed on unchanged but recorded as a failed request
  (`upstream_error_body`), so the request history and `notify` programs see it as a failure.
- Not used: `keepalive.*`, `retry.*`, and the `heartbeat` and `retry` stages. Responses reach
  `response` and `stream` programs only when they are 2xx; a `response` program makes aProxy gather
  the whole response for that layer, so prefer `stream` on forward instances.
- Set the default with `aproxy config set delivery forward`, by editing config.toml, or as the
  settings.json `defaults.delivery` default. Apply it with `aproxy restart <port|alias>`.

## Format programs

A `[[format]]` entry names an external program that aProxy talks to over stdin and stdout, one JSON
line per message and one JSON line back. This section is aProxy's side of that; the message fields
and how to write a program are in the aproxy-format skill, and the configuration keys are in
config-toml.md, "format" (per instance in config.toml only: no settings.json default, no CLI flag).

```
request direction:   client -> format[0] -> format[1] -> ... -> upstream
response direction:  client <- format[0] <- format[1] <- ... <- upstream
```

- **A chain, in list order.** Every program that declares a stage takes part in it; the output of
  one is the input of the next. Each program can keep a small opaque `state` per request, which
  aProxy stores and hands back to that same program at later stages, even on another worker.
- **Handshake.** Each process starts with a `hello` exchange that fixes the protocol version, the
  stages the program supports and how much of the request body it wants (`full`, `digest` or
  `none`). A program that only judges never receives the conversation's content.
- **`request` and `retry`.** A request-side program may rewrite method, URL, headers and body;
  rewriting the URL is how routing and protocol conversion change host and path. aProxy applies
  `api_key`, `override_headers` and `extra_headers` first, so programs see them, and their output
  headers are final except `accept-encoding` on keepalive requests (see Compression). The chain
  runs once per request. Before a retry, the programs that declare `retry` run again only when the
  retry chain is rerun: a `decide` program asks for that with `reroute: true`, and it is the
  default when nobody says otherwise. With `reroute: false` aProxy resends the request it prepared
  last time, so a key-rotating program is not consulted and does not rotate.
- **`decide`.** After each failed attempt of a `retry` request aProxy asks the programs that
  declare `decide`, in order, until both the decision (`retry` or `stop`) and `reroute` are
  settled. A program may also set the wait, and a `stop` may carry a response for the client
  (`retry_stopped`). A program that asked for it is also consulted after successful attempts, to
  reject a "200 that is really an error". No answer, a late answer or a crash counts as no
  opinion: the retry rules and the default backoff then apply, so a broken decision program never
  ends a request.
- **`response` and `stream`.** Only a response judged successful and about to be delivered goes
  through them. `response` receives the whole decoded body. `stream` receives the response as
  `open`, `data` chunks, then `close` or `abort`, on one worker held for the whole stream, and can
  emit converted bytes as they flow: on a `forward` request truly incrementally, on a `retry`
  request while the buffered response is replayed. aProxy drops `content-length` and
  `content-encoding` from the output, since the bytes changed. Once the response head has been
  committed (by a heartbeat skeleton, for example) a program's changes to status and headers are
  ignored and only its body is used.
- **`heartbeat`.** On every keepalive tick the programs receive the bytes about to be sent and may
  replace them. A tick is never delayed: a reply that is late, failed or malformed sends the input
  bytes instead, and once the real response is ready pending ticks are abandoned.
- **`notify`.** After a request ended, each program that declares `notify` gets one message with
  the outcome (delivery mode, attempts, last upstream status, whether the stream completed).
  aProxy does not wait on the data path for the answer; under load notifications are dropped and
  counted (`notify_dropped_total` in the status snapshot).
- **Delivery per request.** The first program whose `request` reply names a `delivery` decides
  whether this request is handled in `retry` or `forward` mode; see Forward-only mode.
- **Failures and the breaker.** A deliberate `fail` reply is obeyed: `retry: false` ends the
  request with `422` `format_rejected` or the response the program supplied; `retry: true` fails
  the attempt (`format_fail`) and it is retried. A program that breaks (cannot start, exits, times
  out, replies with something invalid) is a different matter and depends on `required`: on the
  request side a required program fails the attempt (`format_error`, retried), an optional one is
  skipped; on the response side, at `decide`, `heartbeat` and `notify`, any broken program is
  skipped. `breaker_failures` such failures in a row open the program's breaker: its idle workers
  are killed and no message is sent to it until a probe after `breaker_cooldown_secs` succeeds.
  While a required program's breaker is open, requests that need it on the request side end at
  once with `503` `format_unavailable`. If a keepalive response was already committed, any of
  these errors reaches the client as the terminal SSE frame instead of a status (see After the
  commit).
- **Keepalive.** On keepalive requests the programs run within the heartbeat cadence, so a slow
  one does not leave the client without bytes: if nothing has been committed when the interval
  ends, the skeleton head goes out and heartbeats continue. When any program declares `response`,
  `stream` or `decide`, aProxy never commits the upstream's real head early, because a program may
  still change or reject the response.
- **Process pool.** `process = "spawn"` starts one process per message. `process = "persistent"`
  keeps workers that handle one message at a time each, grows to `pool_max` (beyond that, messages
  queue), retires workers idle for `worker_idle_secs`, and kills a worker that exceeds its time
  limit. Workers that own a stream are outside `pool_max`. When the instance exits, workers see
  end of input on stdin and are expected to exit; aProxy kills the ones that do not.
- **Output discipline.** A worker answers each message with exactly one line on stdout, echoing the
  message's sequence number. A missing or wrong number, an extra line, invalid JSON or output while
  idle gets the worker replaced; otherwise one request could read another's reply, and with it
  another session's body or key. stderr is copied to the instance log with known secrets replaced.
- **State across requests** (key cooldowns, learned decisions) lives in the program's own process.
  Such a program needs `process = "persistent"`, `pool_max = 1` and `worker_idle_secs = 0`, and
  loses that state when the instance restarts.
- **Not sent through `response`/`stream`:** failures passed through because a retry rule gave up,
  and non-2xx responses of forward requests.
- Apply configuration changes with `aproxy restart <port|alias>`. `aproxy doctor` checks the chain
  the way `start` would; `aproxy-format check <config.toml>` checks the official roles' own files.

## Instances, ports and the control channel

- One config file runs as one instance, and the port is the instance's identity: the instance record
  `<home>/run/<port>.instance.json`, the desired-state record `<home>/run/<port>.restore.json`, the control
  endpoint and the watchdog heartbeat are all keyed by port. Log files and the spool directory are
  the exceptions (see Logs and Disk cache). Give each config.toml its own port.
- A background instance is started from a **launch spec**, passed through a short-lived handoff file
  under `<home>/run/` (`aproxy __serve --home <dir> --spec <file>`), not from an inherited command
  line. The spec holds the config path (or null), the override flags, a pinned port for a port-0
  listener, the launching environment's proxy variables and `PATH`, and the log path. The handoff
  file is deleted once the instance is ready or has failed; the instance writes the same spec into
  its desired-state record. Both files are readable only by the current user, so `--api-key` and
  other secrets never appear on the instance's or the watchdog's command line (the short-lived
  `aproxy start` process is the one that was given the flag).
- A background instance detaches from its launcher: on Unix it calls `setsid`, so a Ctrl+C sent to
  the launching terminal's process group does not reach it; on Windows it tries to leave the
  launcher's job object (`CREATE_BREAKAWAY_FROM_JOB`) and, when the job does not allow that, stays
  in it and appends a warning to `startup.log`. It also gets no console and inherits no handles.
- Two instances on one port with different listen addresses cannot coexist; the second refuses with
  `端口 <port> 已被监听地址 <addr> 的 aProxy 实例使用（本进程将监听 <addr>），两者不能并存。` ("port already
  used by an aProxy instance listening on another address; the two cannot coexist").
- An alias resolves to a config file path and finds the running instance by that path, so it keeps
  working after the port changes. A port target goes straight to that port's control endpoint and
  works even if the registry is lost.
- Control channel: on Windows a named pipe whose name carries the port and an identifier of the
  home's run directory, on Linux and macOS the socket `<home>/run/<port>.sock`. Either way it
  belongs to one home: commands find only instances started under the same home. `status`, `stop`,
  `restart`, `logs` and `install` use it and never touch the proxy port, which carries only proxied
  traffic. An instance creates its channel before it registers, and refuses to start if it cannot:
  startup.log gets `控制通道 <endpoint> 创建失败：…` ("control channel could not be created"). So a
  registered instance is always reachable unless it hung.
- `aproxy status` lists instances that answer on their control channel and lists separately those
  whose process is alive but does not answer (hung). It deletes nothing: a record whose process is
  gone (or whose pid now belongs to another process) is ignored for this run and cleaned later by an
  instance that starts serving or by the watchdog.

### Stopping

- `aproxy stop` asks the instance to shut down. Requests still open after 10 s are cut
  (`10 秒后强制退出（在途请求将中断）`, "forcing exit in 10 s; in-flight requests will be interrupted"), so a
  long stream or a request still in retry is cut. `aproxy restart` stops the same way before starting
  again.
- That includes your own requests if you reach your model through the instance. Before stopping or
  restarting, match the instance from `aproxy status` (port, upstream, config) against what the user
  asked for and against your own base URL, and ask before touching an instance that may carry the
  user's other sessions. Never stop every instance or kill aproxy processes by name to clean up: that
  takes down every instance at once, possibly including the one carrying this conversation.
- `aproxy stop --force` terminates at once, after verifying the process by PID plus creation time
  (never by executable name), and removes the desired-state record before terminating so the
  watchdog does not race it. A process killed any other way (`taskkill /PID`, `kill`, a crash)
  leaves its desired-state record behind, so the watchdog brings it back (see Crash recovery and the
  watchdog). Only `stop` deletes records: `restart` and the install rollout keep them throughout.
- A target that is already gone but still has a record is stopped by deleting the record; `stop all`
  does the same for every record whose port has no instance process. A record this version cannot
  read is reported and left in place.

### Isolated experiments

`APROXY_HOME` replaces `~/.aproxy` as the home for the default config.toml, settings.json, `run/`,
`logs/`, `spool/`, `bin/`, `install/`, `ledger/`, `desktop/` and `skills/`. It is the only root:
there is no separate variable for the run directory. The control channel
follows the home, so commands under an isolated home cannot reach the user's instances.
Neither isolates ports: a test instance on a port a real instance listens on fails to start as
`被其他程序占用` ("in use by another program"). Run `aproxy status` without the override first and
pick a port nothing uses. An isolated home also gets its own watchdog, which exits by itself some
minutes after its last instance stops.

## Logs

- A daemon instance logs to `<home>/logs/<hex-timestamp>-<hex-pid>.log`, a new file at every start
  (restart, restore and watchdog respawns included). The name carries no port, so do not guess it:
  `aproxy logs <port|alias>` prints it as `日志文件: …` ("log file"), and `aproxy start` printed it as
  `日志: …`. A `--foreground` instance logs only to its console.
- A custom path comes from `log_file` in config.toml or `--log-file` (a `start` option; the flag wins); a relative path is
  resolved against `<home>`.
- `<home>/logs/startup.log` collects what happens before an instance has its own log or without a
  console: configuration errors, bind failures, non-loopback warnings of daemon starts, the Windows
  job-object warning of a detach that was not allowed, and watchdog slow-retry notices. Read it
  first when an instance fails to start.
- Size: at startup an existing log file over 2 MiB is emptied (this matters for a fixed `log_file`); a
  running daemon empties its log once an hourly check finds it over `settings.json logs.rotate_mb` (0
  disables). `aproxy logs -f` keeps following across truncation.
- Cleanup happens in the background, never in a read-only command. An instance runs one cleanup
  right after it starts serving, and the watchdog does one on every scan. It removes an instance
  record `<port>.instance.json` only when the record is readable and the process that wrote it is
  verifiably gone (pid plus start marker); it removes leftover launch-spec and atomic-write temp files only
  when their writer is no longer running (or cannot be found) and they are older than 10 minutes;
  and it removes an instance log only
  when the name has the built-in random shape (`<hex>-<4 hex>.log`), the file is older than
  10 minutes, and no instance record or desired-state record refers to it. If `run/` holds a record
  this version cannot read, no log is deleted in that round. Custom `log_file` paths and fixed names
  such as `startup.log` are never deleted. `aproxy status`, `stop`, `restart`, `logs` and target
  selection delete no files.
- Encoding: UTF-8 without BOM. aProxy switches the Windows console to code page 65001 itself; garbled
  text usually comes from the tool reading the file (troubleshooting.md, Logs and files).
- Secrets: logs do not contain `api_key` or header values. Every URL (base_url, proxy, upstream target,
  request path) is masked: userinfo becomes `***@`, query values become `***` with keys kept
  (`?beta=true` shows as `?beta=***`), and a fragment becomes `***`. `aproxy status` follows the same
  rules, so both can be shared as they are. A retry rule's `path` still matches the real query string.
- Each failed attempt that is retried logs `本次尝试失败，重试` ("this attempt failed; retrying") with
  `attempt`, `code`, `details` and `reason`. Decision lines such as `上游返回可重试状态码，重试` carry a
  `code` field too.
- Each failed attempt logs `错误响应预览` ("error response preview") with `content_type`,
  `content_encoding` and the body, decoded first when compressed. A preview of the form
  `（二进制/压缩内容，共 N 字节，hex 前 48: …）` ("binary/compressed content, N bytes, first 48 bytes in hex")
  means the body is binary or could not be decoded (unknown encoding, corrupt data, or over 8 MiB once
  decoded).
- An instance listening on a non-loopback address warns `监听地址 … 不是回环地址…` ("listen address is not
  a loopback address") in the start output, the daemon log, startup.log and `aproxy doctor`, more
  strongly when it injects credentials: any host that reaches the port can then spend the user's key.

## Crash recovery and the watchdog

### Desired-state records

- A background instance writes `<home>/run/<port>.restore.json` when it starts. The record holds the
  launch spec: the config path (null for an instance started without a config file), the override
  flags, the pinned port of a port-0 listener, the launch-time proxy environment and `PATH`, and the
  log path. A `--foreground` instance writes no record, because closing its console is the user
  stopping it.
- **Only `stop` deletes a record.** A record that remains means "this instance should be running",
  whatever took the process away: a crash, a kill by PID, a signal, a reboot, or a failed restart.
  `aproxy status` never deletes records. A record this version cannot read (damaged, or written by a
  newer aproxy) is reported and left in place by every reader — `status`, `restore`, the watchdog and
  `stop all` — but `aproxy stop <port>` deletes the file named `<port>.restore.json` without reading
  it, so a direct stop still clears an unreadable record. Records written by
  0.2.0-alpha.1/alpha.2 under the old name `<port>.restore` are not read at all (compatibility.md,
  "Upgrading from 0.2.0-alpha.1 or 0.2.0-alpha.2").
- `aproxy restore` starts every recorded instance that is not running. It skips instances that
  answer, waits up to 30 s for each start, and reports the actual port. If the config file named by
  a record is missing, it keeps the record and says so, so a file that comes back later (a network
  drive that mounts late) can still be restored. With nothing to do it prints
  `没有需要恢复的实例。` ("no instances to restore") and exits 0, so it is safe as a logon task for
  recovery after a reboot or power loss.
- A restored or respawned instance reuses the spec but reads its config file as it is now: unapplied
  edits take effect, and a changed port moves the instance (the old port's record is replaced by the
  new port's). It keeps the launch-time proxy environment and `PATH`, so it reaches the network the
  same way as the original `start`. `aproxy restart` reuses the same spec, so changing a CLI
  override needs a stop followed by a start with the new flags.

### Watchdog

One background process watches every instance: the same executable started as
`aproxy __watchdog --home <dir>` (no public subcommand), a few MB of memory regardless of instance
count. It is on by default; the switch and tuning fields are in settings-json.md
(`watchdog.enabled` and the other `watchdog` keys). It works on Windows and Linux and is not
supported on macOS.

- **Death** (crash, kill by PID): the watchdog sees the process exit at once and acts on it at once,
  so the instance is back within its startup time (typically under a second). With a desired-state
  record present it respawns the instance from that record and waits up to 30 s for it to come up;
  failed respawns are retried with backoff from 1 s doubling to 300 s. Without a record (graceful
  stop) it just stops watching. A respawn that lands on another port moves the watch there and
  replaces the old port's record.
- **Hang** (process alive, runtime stuck): each instance writes a heartbeat every 10 s (a
  shared-memory section on Windows, the file `<home>/run/<port>.heart` elsewhere). When it is stale past the scan tolerance and the instance does not answer on the control
  channel, the watchdog re-verifies PID plus creation time, kills the process and respawns it.
- **Crash loop**: after `watchdog.max_restarts` consecutive failed respawns the watchdog no longer
  gives up. It retries once every 300 s until the instance starts or its record is deleted
  (`aproxy stop <port>`), keeping the record throughout. The first time the limit is crossed it
  appends to startup.log:
  `[watchdog] the instance on port <port> failed to come back N times in a row; retrying every 300 seconds until it starts or aproxy stop <port> is run`.
  `watchdog.max_restarts = 0` is the exception: the watchdog never restarts an instance
  automatically (the record is kept for a manual `aproxy restore`).
- **Failure counting**: a respawned instance must stay up for 60 s before its earlier consecutive
  failures are forgotten. An instance that starts and dies again after a few seconds keeps counting
  and backs off instead of being respawned in a tight loop.
- **Settings are re-read every scan.** Setting `watchdog.enabled = false` makes a running watchdog
  exit at its next scan (it removes its record and leaves the instances and their records alone);
  `max_restarts` and `idle_exit_secs` follow the file on the next scan. `heartbeat_secs` and
  `stale_after_cycles` are fixed when the watchdog process starts, because other processes judge its
  liveness by the period it started with. If settings.json is invalid, the watchdog keeps its last
  valid values.
- **Single watchdog**: `<home>/run/watchdog.json` names the active one: `pid`, `start` (its start
  marker), `version`, `heartbeat_secs` (scan period), `created_at`, `last_scan_at` and `restarts`,
  the last 20 respawns as `{port, at, kind, ok}` with `kind` being `crash` or `hang` (times are Unix
  milliseconds). The watchdog rewrites it every scan; it counts as active while its process is alive
  and `last_scan_at` is within three scan periods (never less than 90 s). `aproxy start` launches
  one if none is active, and every instance checks every 5 minutes
  and launches a replacement if needed, logging `看护者缺席，已由守护补种（选举胜出者）` ("watchdog absent;
  relaunched by an instance"). With no instances left and no respawn pending for `watchdog.idle_exit_secs`,
  it exits; an instance still waiting for its 300 s slow retry keeps it running.
- `aproxy status` does not show the watchdog. To check it, read `watchdog.json` (is `last_scan_at`
  recent, is the PID alive) and `<home>/logs/watchdog.log`, where the watchdog writes in English
  what it did and why: instances it terminated as hung, every respawn and its result, the switch to
  slow retries, instances it stopped watching, takeovers and its own exit.
- Limits: a respawn does not save requests in flight when the process died; their clients see a reset
  connection and must resend, which agent clients with their own retries do after a pause. What the
  watchdog prevents is an outage that lasts until someone notices.

## Upgrades with aproxy install

`aproxy install` (alias `upgrade`) puts the new binary in `<home>/bin/` and restarts running instances
one at a time, so at most one instance is restarting at any moment. Flags and modes are in
commands.md. During the run:

- Before the swap it asks each instance over the control channel to enter the binary-swap phase.
  `aproxy status` then shows `二进制更换中（install 滚动重启阶段，请勿手动干预此实例）` ("binary swap in
  progress; do not intervene"). Do not stop, restart or kill such an instance: install is about to
  restart it, and interfering makes the two fight.
- An instance that does not acknowledge (it is in a faulty state) is restarted onto the installer's
  version, for up to three rounds. If it still fails, install aborts with `实例未表达（已按重试/restart 收敛）`
  ("instances did not acknowledge after retries and restarts") or `实例 <port> 收敛重启失败` ("restarting
  instance <port> failed") and kills nothing. Ask the user whether that instance may be stopped, stop
  it, and run install again.
- If the new binary cannot bring an instance up (stricter validation of the existing config,
  antivirus, not ready within 30 s), install restarts that instance from the old binary with the
  same launch spec, halts the rollout (remaining instances stay on the old version), prints
  `滚动已中止，不会自动重试：…` ("rollout halted, will not retry automatically") and exits non-zero. The
  binary in `bin/` and the install phase are not rolled back: `<home>/install/state.json` holds phase
  `failed` and the cause in `last_error`. Read startup.log, fix the cause, run `aproxy install` again.
  If the old binary cannot start the instance either, it is down but its desired-state record is
  kept, and `aproxy restore` brings it back after the fix.
- The old binary is kept under a unique name, `bin/aproxy.old-<nanoseconds>-<pid>[.exe]`, for that
  rollback and removed at the end. One that is still locked (Windows) stays until a later install
  can delete it, and never blocks one. While `bin/` changes, the installer holds the lock
  `<home>/install/lock`.
- After the download is verified, the installer hands the install to the downloaded binary and waits
  for it (a downgrade is run by the installer itself). If that wait times out
  (`… 秒内未确认安装完成（可能仍在后台进行）…`, "completion not confirmed in time; may still be running"),
  check instance versions with `aproxy status`.
- An interrupted install (crash, power loss) resumes by itself: any later `aproxy` command, or the
  watchdog, starts `install --continue` when `<home>/install/state.json` is left over.
- Inside the swap window the watchdog gives a dying instance up to 5 × 3 s to come back on its own
  before respawning it, and instances do not relaunch a missing watchdog.

None of this applies to `aproxy install --force`, which stops every process of the home, replaces
the binary and then restores the instances (commands.md, Force install).

### Skill documentation

Alongside the binary, install downloads the skill bundle (aproxy-cli and aproxy-format) of the same
version into `<home>/skills/<name>/`, replacing each directory atomically. A failed download does not
fail the install; its result is in the `skill` field of `<home>/install/state.json`, `aproxy install --skills-only`
retries it, and `install --continue` does not. `--no-skills`, or `update.skill_auto_update = false` in
settings.json, skips it. Install never writes into an agent's own skills directory; link or copy
`<home>/skills/aproxy-cli` there yourself, for example to `~/.claude/skills/aproxy-cli` for Claude Code.

## Download proxy versus request proxy

| Setting | Used for |
|---|---|
| config.toml `proxy` (with `proxy_username` / `proxy_password`) | That instance's upstream API traffic |
| settings.json `update.download_proxy`, or `aproxy install --download-proxy` for one run | Only `aproxy install` downloads (binary and skills) |

Without `proxy`, an instance's upstream traffic follows `ALL_PROXY` / `HTTPS_PROXY` / `HTTP_PROXY` /
`NO_PROXY` from the environment the daemon inherited: the shell that ran `aproxy start`, or, after a
respawn, the watchdog's environment. Set `proxy` explicitly when upstream traffic must or must not use
a particular proxy; an explicit `proxy` turns the environment variables off. With neither download
setting, `aproxy install` uses the same environment variables.
