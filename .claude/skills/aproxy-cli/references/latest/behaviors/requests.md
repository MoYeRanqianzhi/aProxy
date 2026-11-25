# Runtime behavior: requests and retries

Part of [behaviors.md](../behaviors.md), which lists the other parts. How a request travels through an instance and when it is retried: lifecycle, retry decisions, errors inside response bodies, the disk cache, request history, forward-only delivery.

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
