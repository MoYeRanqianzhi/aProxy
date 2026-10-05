# Runtime behavior

What a running aProxy instance does with each request and how instances look after themselves:
retries, keepalive heartbeats, buffering, forward-only mode, transformers, instance identity, logs,
crash recovery, the watchdog and upgrades. Read it to explain or predict aProxy's behavior, or before
acting on a running instance. Field syntax and defaults are in config-toml.md and settings-json.md,
client setup in clients.md, symptom lookup in troubleshooting.md.

`<home>` below means `~/.aproxy`, or the directory named by `APROXY_HOME` when that is set.

## Contents

- [Request lifecycle](#request-lifecycle)
- [Retry decisions](#retry-decisions) (backoff, bounded retry paths)
- [Keepalive heartbeats](#keepalive-heartbeats) (which requests, commit point, compression, disconnects)
- [Error detection in response bodies](#error-detection-in-response-bodies)
- [Disk cache (spool)](#disk-cache-spool)
- [Forward-only mode](#forward-only-mode)
- [External transformers](#external-transformers)
- [Instances, ports and the control channel](#instances-ports-and-the-control-channel) (stopping, isolated experiments)
- [Logs](#logs)
- [Crash recovery and the watchdog](#crash-recovery-and-the-watchdog)
- [Upgrades with aproxy install](#upgrades-with-aproxy-install) (skill documentation)
- [Download proxy versus request proxy](#download-proxy-versus-request-proxy)

## Request lifecycle

A client sets its API base URL to `http://127.0.0.1:<port>`. In the default mode every request goes
through these steps:

1. **Inbound check.** A request whose `Host` or `Origin` header is not allowed gets a local 403 and is
   never forwarded or retried (config-toml.md, `allowed_hosts` / `allowed_origins`). CLI clients send
   no `Origin` and use `127.0.0.1` or `localhost` as host, so they pass.
2. **Header rewrite.** `api_key` replaces both `Authorization` (as `Bearer <key>`) and `x-api-key`;
   `override_headers` replace headers; `extra_headers` are added only when the client did not send
   them. Hop-by-hop headers (`Connection`, `Upgrade`, `Transfer-Encoding`, `Host`, ...) are dropped, so
   WebSocket upgrades do not pass through. Method, path, query and all other headers are unchanged.
3. **Body buffering.** The whole request body is read before anything goes upstream, so every retry
   can replay it. A body over `max_body_mb` gets 413 at once.
4. **Keepalive decision**, made on the request as the client sent it (see Keepalive heartbeats).
5. **Request transform**, if configured, runs once; every retry replays its output.
6. **Upstream URL** = `base_url` without a trailing `/`, followed by the client's path and query
   verbatim. aProxy adds nothing, so a path prefix such as `/v1` belongs on exactly one side: either in
   `base_url` or in the client's base URL, not both (clients.md, Setup checklist).
7. **Attempts.** Each attempt buffers the complete upstream response and then judges it (see Retry
   decisions). A failed attempt is discarded and retried; its bytes never reach the client.
8. **Replay.** The successful response, after the response transform if one is configured, goes to
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
| Connect, send or read error, including `connect_timeout_secs` / `read_timeout_secs` expiring and a body cut off midway | Retry |
| Status 400-599, every 4xx included | Retry |
| Status 2xx/3xx whose body is a JSON object with a non-null `error` field or `"type": "error"` | Retry |
| Streaming body in which any SSE `data:` payload or NDJSON line is such an error object | Retry |
| Response larger than `spool_limit_mb` | Stop with 502 `上游响应体超出 spool 上限，无法回放（重试无意义）` ("upstream response exceeds the spool limit; cannot replay, retrying is pointless") |
| Local disk write failed after the response spilled to disk | Stop with 502 `本地磁盘缓存写入失败，无法回放: …` ("local disk cache write failed; cannot replay") |
| A failure that came with a response, on a bounded retry path, from attempt 3 on | Pass that response through (see Bounded retry paths) |
| Anything in forward-only mode | No retry (see Forward-only mode) |

Otherwise retries are unlimited: the request ends when an attempt succeeds or the client gives up.
Retrying every 4xx is deliberate. Rate limits (429) and transient auth failures are common on real
upstreams, and while aProxy retries, the client sees a slow answer rather than an error. The price is
that a deterministic error (wrong key, wrong path, unknown model) is retried forever too, and the
client waits indefinitely. The log shows it as repeated `上游返回可重试状态码，重试` ("upstream returned a
retryable status, retrying") lines, each followed by `错误响应预览` ("error response preview") with the
upstream's own error body; fix the configuration, or bound the endpoint.

When a request has to stop after a keepalive response was already committed, the client gets a
terminal SSE error event instead of a 502 (see After the commit).

### Backoff

The first three retries go out immediately. From the fourth, the delay doubles from 5 s (5, 10, 20,
40 s, ...) until it reaches `max_retry_backoff_secs` and then stays there; `0` makes every retry
immediate. Once an outage outlasts the ramp, aProxy probes the upstream once per cap interval, so a
recovered upstream can take up to one cap interval to be noticed. Lower the cap when quick recovery
matters more than load on the upstream.

### Bounded retry paths

`bounded_retry_paths` (config-toml.md) holds regular expressions matched against the client's
`path?query` as a whole, anchored at both ends, before any request transform. For a matching request:

- attempts 1 and 2 are retried as usual, with no delay;
- from attempt 3, the first failure that came with a response is passed to the client unchanged
  (status, headers, body) and retrying stops, so a deterministic error comes back after about three
  round trips;
- network errors are still retried without limit: they are transient and carry no response to pass on;
- a passed-through failure never goes through the response transform;
- if a keepalive response was already committed when the limit is hit, the client gets the terminal
  event `upstream_error` instead, because the real status line can no longer be sent.

The log says `受限重试路径达到尝试上限` ("bounded retry path reached its attempt limit"). Use bounded paths
for endpoints a particular upstream never supports; troubleshooting.md has the Claude Code
`count_tokens` case.

## Keepalive heartbeats

A streaming request can spend minutes in retries before any upstream byte is usable. Keepalive keeps
the client's connection visibly alive meanwhile: aProxy commits a response head early and then writes
a heartbeat every `keepalive_interval_secs`. By default that is the SSE comment line `: keepalive`,
which SSE parsers discard by specification; `keepalive_heartbeat` replaces it with any bytes that end
at an event boundary (config-toml.md).

### Which requests get heartbeats

All of the following must hold:

- `keepalive_interval_secs` is above 0 and the instance is not in forward-only mode;
- `keepalive_trigger` matches: `accept` means the `Accept` header contains `text/event-stream`;
  `body_stream` means the JSON body has a top-level `"stream": true`; `any` means either.

The trigger looks at the request as the client sent it, before any request transform, because the
question is whether the client is waiting for a stream. Every other request (a non-streaming call, or
a streaming request that matches neither test, such as Gemini CLI's) receives nothing until its final
response is ready, so the client's own HTTP timeout must cover all retries plus generation.

### Commit point

Nothing is written before the commit, and the commit happens once per request:

| Before any commit | Result |
|---|---|
| An attempt returns a 2xx head with an uncompressed `text/event-stream` content type, and no `response_transform` is configured | The upstream's real status and headers go to the client at once (without `content-length` and hop-by-hop headers) |
| One keepalive interval passes without such a head | A skeleton head is committed (`200`, `content-type: text/event-stream`, `cache-control: no-cache`) and a heartbeat follows immediately; the first byte therefore arrives within one interval |
| An attempt succeeds first | Replayed exactly like a request without keepalive, real status and headers included |
| An attempt returns a 2xx head that is not SSE (NDJSON, plain JSON) | The skeleton commit is paused for that attempt, so the content type is not rewritten. Success is replayed unchanged; if the attempt needs a retry, the skeleton is committed right after. During such an attempt the client may receive nothing for up to `read_timeout_secs` |

A failed attempt does not trigger a commit by itself, so a request that succeeds after a few quick
retries still gets the upstream's real head.

### After the commit

- A heartbeat goes out every interval while aProxy waits for upstream headers, buffers an upstream
  body, waits between retries, or runs a transform or a decode.
- Status and headers can no longer change. The successful attempt's complete, validated body is
  written into the committed response; bytes of failed attempts never appear.
- A successful body that is not SSE is still written, since dropping a good result would be worse, and
  the log warns `已以 text/event-stream 提交的保活响应里回放的是非 SSE 的成功响应体（响应头已发出，无法撤回）…`
  ("a non-SSE success body was replayed into a keepalive response already committed as
  text/event-stream"). For an upstream that answers `"stream": true` with NDJSON and often needs
  retries, set `keepalive_trigger = "accept"` on that instance.
- A request that has to stop ends with one SSE event, `event: error` followed by
  `data: {"type":"error","error":{"type":"<type>","message":"…"}}`:

| `error.type` | Cause |
|---|---|
| `upstream_error` | Bounded retry path exhausted |
| `proxy_spool_limit` | Response exceeded `spool_limit_mb` |
| `proxy_spool_failed` | Local disk write failed |
| `proxy_decode_failed` | The upstream compressed the body despite `identity` (see Compression) and decoding failed |
| `proxy_transform_failed` | The request transform failed after the skeleton head was committed |

### Compression

For requests that get heartbeats, aProxy sends `accept-encoding: identity` upstream, because plain-text
heartbeats cannot be inserted into a gzip or brotli stream. This overrides the client's value, a
request transform's value and `override_headers`; when `override_headers` sets another
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

With `disk_cache` on, request bodies and responses larger than 1 MiB spill to `<home>/spool/<port>/`
as `req-*.spooltmp` and `spool-*.spooltmp` files. Retries and replays stream from the file, so memory
use does not grow with payload size; through the OS page cache an SSD performs about as well as
memory.

- A temp file is deleted when its request finishes, when its attempt is retried, or when the client
  disconnects. Files left by a crash are deleted the next time an instance starts on that port; after
  a clean exit the directory is empty.
- If the file cannot be created when a body crosses 1 MiB, buffering continues in memory. If a write
  fails after the body is already on disk (disk full), the request stops with the 502 or
  `proxy_spool_failed` above and is not retried.
- With `disk_cache = false` everything stays in memory; for payloads under 1 MiB both modes behave
  the same.

## Forward-only mode

`forward_only = true` turns an instance into a plain streaming pass-through: no buffering, no retries,
no heartbeats. Use it only when the upstream is trusted and the client needs true incremental
streaming and copes with upstream errors itself, because it gives up the retry guarantee aProxy exists
for. Startup logs a warning beginning `仅转发模式已启用` ("forward-only mode enabled").

- Still applied: the inbound check, the header rewrite, and `max_body_mb`, counted while the body
  streams; over the limit the upstream request is aborted and the client gets 413.
- The request body streams upstream chunked. The response streams back with the upstream's status and
  headers; `content-length` is kept unless the upstream used `transfer-encoding`.
- Upstream request fails: 502 `上游请求失败: …（仅转发模式不重试）` ("upstream request failed; forward-only
  mode does not retry"), recorded as the latest error in `aproxy status`.
- Response stream breaks: the client connection is cut at that point, and aProxy never injects bytes
  the upstream did not send. Log: `上游响应流中断，连接就此截断（仅转发模式不重试）` ("upstream response
  stream broke; connection truncated").
- Not used: error-content detection (a 200 carrying an error JSON is passed on), `disk_cache`,
  `spool_limit_mb`, `keepalive_interval_secs`, `max_retry_backoff_secs`. Transformers cannot be
  combined with it; the instance refuses to start.
- Set it in config.toml or as a settings.json default; there is no CLI flag. Apply it with
  `aproxy restart <port|alias>`.

## External transformers

`request_transform` and `response_transform` hand a request or a response to an external format
program as a one-line JSON envelope and use what comes back. This section is aProxy's side; the
envelope fields and how to write a format program are in the aproxy-format skill, and the config
fields in config-toml.md (per instance in config.toml only: no settings.json default, no CLI flag).

```
request:  buffered -> keepalive decision -> request transform -> attempts replay the transformed request
response: attempt judged successful -> response transform -> replay
```

- **What can change.** A request transform may rewrite method, URL, headers and body; rewriting the URL
  is how protocol conversion changes path and host. A response transform may rewrite headers and body.
  aProxy applies `api_key`, `override_headers` and `extra_headers` before the request transform, so the
  format sees them and its output headers are final, except `accept-encoding` on keepalive requests
  (see Compression).
- **One transform per request.** Every retry replays the same transformed request. A format that
  rotates keys therefore switches keys between requests, never between retries of one request.
- **Request-side failure** (the process failed to start, crashed, timed out, reported an error, or broke
  the envelope protocol): the request is neither sent upstream nor retried. The client gets 502
  `请求转换失败: <reason>（请求侧转换失败不重试，未发往上游）` ("request transform failed; not retried, not
  sent upstream"), or the terminal event `proxy_transform_failed` if a keepalive response was already
  committed, and the reason becomes the latest error in `aproxy status`. The aproxy-format skill's
  troubleshooting.md maps each reason.
- **Response-side failure**: the original upstream response is replayed unchanged and the log warns
  `响应转换失败，透传上游原始响应` ("response transform failed; passing the original upstream response
  through"). The response is already in hand, so availability wins.
- **Keepalive.** On keepalive requests both transforms run within the heartbeat cadence, so a slow
  format does not leave the client without bytes: if nothing has been committed when the interval
  ends, the skeleton head goes out and heartbeats continue. The exception is a successful 2xx
  response that is not SSE, arriving before any commit: committing a skeleton would corrupt it, so the
  response transform runs to completion first and the client receives nothing meanwhile. With
  `response_transform` set, aProxy never commits the upstream's real head early, because the format
  may change it; once the skeleton is committed (before or during the response transform), header
  changes from the format are ignored and only its body is used.
- **Response body.** The response transform receives the decoded body when the upstream compressed it
  (up to 8 MiB decoded; larger bodies arrive as received), and aProxy drops `content-length` and
  `content-encoding` from its output headers, since the bytes changed.
- **Not transformed:** failures passed through on a bounded retry path. Forward-only instances cannot
  have transformers.
- **Process pool.** `mode = "spawn"` starts one process per request. `mode = "persistent"` keeps worker
  processes that handle one request at a time each, grows to `pool_max` (beyond that, requests queue),
  retires workers idle for `idle_timeout_secs`, and kills a worker that exceeds `timeout_secs`. When the
  instance exits, workers see end of input on stdin and are expected to exit. If a reused idle worker
  turns out to have died before producing any output, the request is redone once on a fresh worker.
- **Output discipline.** A worker answers each request with exactly one line on stdout. An extra line,
  invalid JSON, or output while idle gets the worker removed from the pool; otherwise the next request
  would read the previous request's output and one session's body or key would go to another. The
  format's stderr is discarded.
- `command` expands a leading `~/`. Apply changes with `aproxy restart <port|alias>`.

## Instances, ports and the control channel

- One config file runs as one instance, and the port is the instance's identity: the registry record
  `<home>/run/<port>.pid`, the restore record `<home>/run/<port>.restore`, the control endpoint, the
  spool directory and the watchdog heartbeat are all keyed by port. Log files are the exception (see
  Logs). Give each config.toml its own port.
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
- `aproxy status` lists instances that answer on their control channel, lists separately those whose
  process is alive but does not answer (hung), and deletes registry records of processes that are
  gone.

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
  (never by executable name), and removes the restore record. A process killed any other way
  (`taskkill /PID`, `kill`, a crash) leaves its restore record behind, so the watchdog brings it back
  (see Crash recovery and the watchdog).

### Isolated experiments

`APROXY_HOME` replaces `~/.aproxy` as the home for the default config.toml, settings.json, `run/`,
`logs/`, `spool/`, `bin/` and `skills/`; `APROXY_RUN_DIR` moves only `run/`. The control channel
follows the run directory, so commands under an isolated home cannot reach the user's instances.
Neither isolates ports: a test instance on a port a real instance listens on fails to start as
`被其他程序占用` ("in use by another program"). Run `aproxy status` without the override first and
pick a port nothing uses. An isolated home also gets its own watchdog, which exits by itself some
minutes after its last instance stops.

## Logs

- A daemon instance logs to `<home>/logs/<hex-timestamp>-<hex-pid>.log`, a new file at every start
  (restart, restore and watchdog respawns included). The name carries no port, so do not guess it:
  `aproxy logs <port|alias>` prints it as `日志文件: …` ("log file"), and `aproxy start` printed it as
  `日志: …`. A `--foreground` instance logs only to its console.
- A custom path comes from `log_file` in config.toml or `--log-file` (the flag wins); a relative path is
  resolved against `<home>`.
- `<home>/logs/startup.log` collects what happens before an instance has its own log or without a
  console: configuration errors, bind failures, non-loopback warnings of daemon starts, and watchdog
  give-ups. Read it first when an instance fails to start.
- Size: at startup an existing log file over 2 MiB is emptied (this matters for a fixed `log_file`); a
  running daemon empties its log once an hourly check finds it over `log_rotate_mb` (settings.json; 0
  disables). `aproxy logs` keeps following across truncation.
- Cleanup: every command that lists instances (`aproxy status`, and `stop`, `restart` or `logs` unless
  given a port) deletes each `*.log` in `<home>/logs/` that no live instance and no restore record
  refers to, `startup.log` excepted. A stopped instance's log therefore disappears soon
  after it stops, and a crashed instance's log stays until the instance is restored. Files outside
  `<home>/logs/` are never touched; copy a log elsewhere if it must outlive its instance.
- Encoding: UTF-8 without BOM. aProxy switches the Windows console to code page 65001 itself; garbled
  text usually comes from the tool reading the file (troubleshooting.md, Logs and files).
- Secrets: logs do not contain `api_key` or header values. Every URL (base_url, proxy, upstream target,
  request path) is masked: userinfo becomes `***@`, query values become `***` with keys kept
  (`?beta=true` shows as `?beta=***`), and a fragment becomes `***`. `aproxy status` follows the same
  rules, so both can be shared as they are. `bounded_retry_paths` still matches the real query string.
- Each failed attempt logs `错误响应预览` ("error response preview") with `content_type`,
  `content_encoding` and the body, decoded first when compressed. A preview of the form
  `（二进制/压缩内容，共 N 字节，hex 前 48: …）` ("binary/compressed content, N bytes, first 48 bytes in hex")
  means the body is binary or could not be decoded (unknown encoding, corrupt data, or over 8 MiB once
  decoded).
- An instance listening on a non-loopback address warns `监听地址 … 不是回环地址…` ("listen address is not
  a loopback address") in the start output, the daemon log, startup.log and `aproxy doctor`, more
  strongly when it injects credentials: any host that reaches the port can then spend the user's key.

## Crash recovery and the watchdog

### Restore records

- After binding its port, an instance writes `<home>/run/<port>.restore`: the command line it was
  started with (config path plus any CLI overrides) and its log path. A graceful stop deletes it, so a
  remaining record means "this instance should be running". `aproxy status` never deletes these
  records.
- `aproxy restore` starts every recorded instance that is not running. It skips instances that answer,
  deletes records whose config file no longer exists, waits up to 8 s for each start, and reports the
  actual port. With nothing to do it prints `没有需要恢复的实例。` ("no instances to restore") and exits 0,
  so it is safe as a logon task for recovery after a reboot or power loss.
- A restored or respawned instance replays the recorded command line but reads its config file as it
  is now: unapplied edits take effect, and a changed port moves the instance (the old port's records
  are then removed). `aproxy restart` reuses the recorded command line in the same way, so changing a
  CLI override needs a stop followed by a start with the new flags.

### Watchdog

One background process watches every instance: the same executable started with an internal flag (no
public subcommand), a few MB of memory regardless of instance count. It is on by default; the switch
and tuning fields are in settings-json.md (`watchdog`, `watchdog_*`). It works on Windows and Linux and
is not supported on macOS.

- **Death** (crash, kill by PID): the watchdog sees the process exit at once and acts on it at once,
  so the instance is back within its startup time (typically under a second). With a restore
  record present it respawns the instance from that record and waits up to 8 s for it to come up; failed respawns are retried with backoff from 1 s doubling to 300 s. Without a record
  (graceful stop) it just stops watching. A respawn that lands on another port moves the watch there
  and removes the old port's records.
- **Hang** (process alive, runtime stuck): each instance writes a heartbeat to shared memory every
  10 s. When it is stale past the scan tolerance and the instance does not answer on the control
  channel, the watchdog re-verifies PID plus creation time, kills the process and respawns it.
- **Crash loop**: after `watchdog_max_restarts` consecutive failed respawns it gives up, keeps the
  restore record, and appends to startup.log
  `[watchdog] 端口 <port> 的实例连续重拉 N 次失败，已放弃自动重拉；可执行 aproxy restore 手工恢复` ("gave up
  respawning the instance on <port> after N failures; run aproxy restore by hand").
- **Single watchdog**: `<home>/run/watchdog.claim` names the active one (PID, creation time,
  heartbeat). `aproxy start` launches one if none is active, and every instance checks every 5 minutes
  and launches a replacement if needed, logging `看护者缺席，已由守护补种（选举胜出者）` ("watchdog absent;
  relaunched by an instance"). With no instances left for `watchdog_idle_exit_secs`, it exits.
- `aproxy status` does not show the watchdog. To check it, read the claim file and see whether its PID
  is alive.
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
  antivirus, not ready within 8 s), install restarts that instance from the old binary with its
  original arguments, halts the rollout (remaining instances stay on the old version), prints
  `滚动已中止，不会自动重试：…` ("rollout halted, will not retry automatically") and exits non-zero. The
  binary in `bin/` and the install phase are not rolled back: `<home>/run/install.state` holds phase
  `failed` and the cause in `last_error`. Read startup.log, fix the cause, run `aproxy install` again.
  If the old binary cannot start the instance either, it is down but its restore record is kept, and
  `aproxy restore` brings it back after the fix.
- The old binary is kept as `bin/aproxy.old.exe` (Windows) or `bin/aproxy.old` (Unix) for that
  rollback and removed at the end; on Windows it stays until the next install if it is still locked.
- On Windows the installer hands the rollout to a relay process and waits for it. If that wait times
  out (`… 秒内未确认安装完成（可能仍在后台进行）…`, "completion not confirmed in time; may still be running"),
  check instance versions with `aproxy status`.
- An interrupted install (crash, power loss) resumes by itself: any later `aproxy` command, or the
  watchdog, starts `install --continue` when `install.state` is left over.
- Inside the swap window the watchdog gives a dying instance up to 5 × 3 s to come back on its own
  before respawning it, and instances do not relaunch a missing watchdog.

### Skill documentation

Alongside the binary, install downloads the skill bundle (aproxy-cli and aproxy-format) of the same
version into `<home>/skills/<name>/`, replacing each directory atomically. A failed download does not
fail the install; its result is in the `skill` field of install.state, `aproxy install --skills-only`
retries it, and `install --continue` does not. `--no-skills`, or `skill_auto_update = false` in
settings.json, skips it. Install never writes into an agent's own skills directory; link or copy
`<home>/skills/aproxy-cli` there yourself, for example to `~/.claude/skills/aproxy-cli` for Claude Code.

## Download proxy versus request proxy

| Setting | Used for |
|---|---|
| config.toml `proxy` (with `proxy_username` / `proxy_password`) | That instance's upstream API traffic |
| settings.json `download_proxy`, or `aproxy install --download-proxy` for one run | Only `aproxy install` downloads (binary and skills) |

Without `proxy`, an instance's upstream traffic follows `ALL_PROXY` / `HTTPS_PROXY` / `HTTP_PROXY` /
`NO_PROXY` from the environment the daemon inherited: the shell that ran `aproxy start`, or, after a
respawn, the watchdog's environment. Set `proxy` explicitly when upstream traffic must or must not use
a particular proxy; an explicit `proxy` turns the environment variables off. With neither download
setting, `aproxy install` uses the same environment variables.
