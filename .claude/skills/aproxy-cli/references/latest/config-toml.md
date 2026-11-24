# config.toml reference

Every field of an instance's `config.toml`: type, default, what 0 or an empty value means, and how
to choose a value; then how values combine with `start` override flags and `settings.json`, what is checked at
start, and how values are normalized. Read it before writing or editing a config, or when a start
fails with `配置错误` ("configuration error"). How each feature works at runtime is in
behaviors.md; this file keeps only what you need to pick a value.

## Contents

- [Working with config files](#working-with-config-files)
- [Example](#example)
- [Field summary](#field-summary)
- [Precedence](#precedence)
- [Fields](#fields): [base_url](#base_url) · [listen_addr](#listen_addr) · [api_key](#api_key) ·
  [extra_headers / override_headers](#extra_headers--override_headers) ·
  [proxy / proxy_username / proxy_password](#proxy--proxy_username--proxy_password) ·
  [max_request_mb](#max_request_mb) · [max_response_mb](#max_response_mb) ·
  [timeouts](#timeouts) · [keepalive](#keepalive) · [retry](#retry) · [delivery](#delivery) ·
  [disk_cache](#disk_cache) · [allowed_hosts / allowed_origins](#allowed_hosts--allowed_origins) ·
  [log_file](#log_file) · [history](#history) · [[[format]]](#format)
- [Checks at start](#checks-at-start)
- [Normalization](#normalization)

## Working with config files

- **One file per instance.** The default config is `config.toml` in the aProxy home (`~/.aproxy/`,
  or the directory in `APROXY_HOME` when that is set), unless settings.json `default_config` names
  another file. Each further instance has its own file, selected with `aproxy start PATH`,
  `aproxy start --config PATH` or an alias, and needs its own port in `listen_addr`, because instances are
  identified by port. Keep extra configs in `~/.aproxy/configs/`: `aproxy find` and `aproxy doctor`
  scan it without setup (other directories: settings-json.md, `config_dirs`).
- **Create or change a file with `aproxy config`, or by hand.** `aproxy config set KEY VALUE`
  writes one key and creates the file if needed; `aproxy config --config PATH set KEY VALUE`
  targets another file, and `aproxy config unset KEY` removes one key. Any dotted key path can be
  set (the common ones: commands.md, `config`); array tables (`format`, `retry.rules`) can only be
  set or removed as a whole. A write changes only the target key and keeps comments, key order and
  everything else in the file. Writes are serialized across
  processes by a lock, and a change that would introduce a new error is refused with exit 3 while
  unrelated errors already in the file do not block it, so a broken file can be repaired one key
  at a time. `unset` removes any key, including one this version does not know.
- **Every key is checked strictly; an unknown key is an error.** A misspelled key, an old 0.1.x
  name, or a key from a later release produces `config_unknown_key`, naming the file, the key path
  and line:column, and the instance does not start. This is deliberate: an ignored matcher would
  make a retry rule wider than written, and silently dropping a key hides the typo until the
  behavior matters. Remove a key you no longer want with `aproxy config unset KEY`.
- **A wrong type or value is an error too.** After editing, run `aproxy config show --effective
  [--config PATH]`: it prints every effective value annotated with its source layer, then lists any
  diagnostics as `file:line:col: key: message [code]` and exits 3 when the file has errors. The
  message is English; `[code]` is the stable part (see [Checks at start](#checks-at-start)).
- **Apply a change with a restart.** An instance reads its config only when it starts. Run
  `aproxy restart PORT|ALIAS`: it checks the edited config first and leaves the old instance
  running if the check fails, so a typo does not take the instance down.
- **Check without applying.** `aproxy doctor` validates every config an alias points to and every
  other `*.toml` in the config directories, but not the default config unless an alias points to
  it. For the default config, `aproxy restart` or `aproxy start` is the check, and `aproxy config
  show --effective` reports its diagnostics without starting anything.

## Example

```toml
base_url = "https://api.anthropic.com"   # required
listen_addr = "127.0.0.1:12345"
# api_key = "sk-..."                      # inject the key here instead of in the client

# Optional, uncomment as needed
# proxy = "http://127.0.0.1:7890"

[timeouts]
first_byte_secs = 600                     # long non-streaming generations

[[retry.rules]]
path = '/v1/messages/count_tokens(\?.*)?' # path?query, anchored at both ends
max_attempts = 3

# [[format]]
# name = "keys"
# command = "aproxy-format"               # bare name: <home>/bin/ first, then PATH
# args = ["keys", "--config", "keys.toml"] # relative paths resolve against <home>
# process = "persistent"
# stages = ["request", "retry"]
# pool_max = 1
# worker_idle_secs = 0
```

## Field summary

"settings.json" in the Default column means the field falls back to a key of the same name and
nesting in the `defaults` section of settings.json, whose built-in value is shown (see
[Precedence](#precedence)). Integers are non-negative.

| Field | Type | Default | 0 / empty means |
|---|---|---|---|
| `base_url` | string | none, required (may come from settings.json `defaults.base_url`) | start fails |
| `listen_addr` | string `host:port` | `"127.0.0.1:12345"` | port 0: the OS picks one |
| `api_key` | string | settings.json (`defaults.api_key`), unset | blank = unset |
| `extra_headers` | table of strings | `{}` | |
| `override_headers` | table of strings | `{}` | |
| `proxy` | string (URL) | settings.json (`defaults.proxy`), unset: environment proxy variables apply | blank = unset |
| `proxy_username` | string | settings.json (`defaults.proxy_username`), unset | blank = unset |
| `proxy_password` | string | settings.json (`defaults.proxy_password`), unset | blank = unset |
| `max_request_mb` | integer | settings.json (`defaults.max_request_mb`), `128` | 0 = no limit |
| `max_response_mb` | integer | settings.json (`defaults.max_response_mb`), `256` | 0 is treated as 1 |
| `[timeouts] connect_secs` | integer | settings.json (`defaults.timeouts.connect_secs`), `30` | 0 = no limit |
| `[timeouts] first_byte_secs` | integer | settings.json (`defaults.timeouts.first_byte_secs`), `300` | 0 = no limit |
| `[timeouts] read_idle_secs` | integer | settings.json (`defaults.timeouts.read_idle_secs`), `300` | 0 = no limit |
| `[keepalive] interval_secs` | integer | settings.json (`defaults.keepalive.interval_secs`), `15` | 0 = keepalive off |
| `[keepalive] trigger` | `"accept"` \| `"body_stream"` \| `"any"` | settings.json (`defaults.keepalive.trigger`), `"any"` | `"always"` is rejected |
| `[keepalive] heartbeat` | string | settings.json (`defaults.keepalive.heartbeat`), `": keepalive\n\n"` | must not be empty; must end at an SSE event boundary |
| `[retry] max_backoff_secs` | integer | settings.json (`defaults.retry.max_backoff_secs`), `320` | 0 = every retry immediate |
| `[[retry.rules]]` | array of tables | settings.json (`defaults.retry.rules`), `[]` | empty = every request retries without limit |
| `delivery` | `"retry"` \| `"forward"` | settings.json (`defaults.delivery`), `"retry"` | |
| `disk_cache` | boolean | settings.json (`defaults.disk_cache`), `true` | |
| `[history] body_budget_mb` | integer | settings.json (`defaults.history.body_budget_mb`), `64` | 0 = keep no bodies |
| `allowed_hosts` | array of strings | settings.json (`defaults.allowed_hosts`), `[]` | empty = built-in policy; `["*"]` = check off |
| `allowed_origins` | array of strings | settings.json (`defaults.allowed_origins`), `[]` | empty = built-in policy; `["*"]` = check off |
| `log_file` | string (path) | unset: random name in `<APROXY_HOME>/logs/` | |
| `[[format]]` | array of tables | `[]` | no format program |

## Precedence

Highest first:

1. **`start` override flags** `--base-url`, `--listen-addr`, `--proxy`, `--api-key` and
   `--log-file`, given to `aproxy start`, as in `aproxy start --listen-addr 127.0.0.1:12399 work`
   (syntax: commands.md, "Options for start"). They are never written to a file, but they become
   part of the instance's launch
   spec, so `aproxy restart`, `aproxy restore` and watchdog restarts keep applying them.
   To drop one, stop the instance and start it again without the flag. Prefer the file over
   `--api-key`: the flag is visible in the shell history and is stored in plain text in the
   desired-state record (owner-only), even though it no longer appears on the instance's command
   line; `--api-key -` reads the key from stdin instead.
2. **config.toml.**
3. **The `defaults` section of settings.json.** It can hold any key of this file except
   `listen_addr`, `log_file` and `format`, under the same name and nesting, written as JSON
   (settings-json.md, "defaults"). A key present in config.toml always wins, even when its value
   equals the built-in default; an explicit empty array in the toml (for example
   `allowed_hosts = []`) also counts as set where the key is an array.
4. **The built-in default.**

Scalars and arrays take the highest layer that sets them, as a whole. Arrays are replaced, not
merged: a `retry.rules` or `allowed_hosts` written in config.toml replaces the `defaults` value
whole. The two header tables are the exception: they merge by header name (case-insensitive) across
config.toml and `defaults`, with config.toml winning a conflict. `listen_addr`, `log_file` and
`format` are per-instance and have no settings.json layer.

**Credentials in `defaults` follow their target.** `defaults.api_key` and the secret headers in
`defaults.extra_headers` / `defaults.override_headers` are used only when this instance's final
`base_url` points at the same target as `defaults.base_url`; `defaults.proxy_username` and
`defaults.proxy_password` only when its final `proxy` names the same scheme, host and port as
`defaults.proxy`. Otherwise they are withheld — a key meant for one vendor's API must not be sent to
another. Everything else, including non-secret headers, is inherited as usual.
`aproxy config show --effective` lists what was withheld and warns when the instance ends up with no
credential of its own; settings-json.md, "Credentials in defaults follow their target", describes the
comparison and which header names are recognized as secrets.

`aproxy config show --effective` prints each effective value with its source layer: `config.toml`,
`settings.json 全局默认` (settings.json global default), `环境变量` (environment, `proxy` only),
`内置默认` (built-in default) or `未设置` (unset). `start` override flags are not visible to it; they
are described in step 1 above.

`proxy` has one extra source: with no `proxy` in either layer, the HTTP client reads the usual
environment variables (`HTTPS_PROXY`, `https_proxy`, `HTTP_PROXY`, `http_proxy`, `ALL_PROXY`,
`all_proxy`), and `show --effective` reports the source as `环境变量`.

## Fields

### base_url

The upstream API root. aProxy appends the client's full request path and query unchanged, so
`base_url` plus the path the client sends must form the real upstream URL: if the client's base
URL is `http://127.0.0.1:12345/v1` and it calls `/v1/chat/completions`, set
`base_url = "https://api.example.com"`, not `.../v1`.

- Must start with `http://` or `https://` (any case) and must not contain `?` or `#`, which would
  turn the appended path into part of the query.
- A trailing `/` is removed.
- It may come from settings.json `defaults.base_url`. Credentials in `defaults` are tied to that
  value: an instance whose own `base_url` names another target does not inherit them (see
  [Precedence](#precedence)).

### listen_addr

The `host:port` the instance listens on. The default `127.0.0.1:12345` is reachable only from this
machine.

- The port is required; without one, start fails with `config_invalid_value` on `listen_addr`
  (`listen_addr has no port or the port is not valid`).
- Every instance needs its own port. A second instance on a port that another aProxy instance
  already uses is refused, even with a different host part.
- Port `0` lets the OS choose. The instance reports the actual address, and that port is pinned in
  its desired-state record: `restart`, `restore` and watchdog respawns return to the same port
  while the config still says `0`. Once the config names a fixed port, the config wins and the
  pinned port is discarded. Clients need a fixed address, so use `0` only for throwaway tests.
- A non-loopback address (`0.0.0.0`, a LAN IP) lets every host that can reach the port use the
  upstream through the proxy, with any key the instance injects. `aproxy start`, the startup log and
  `aproxy doctor` warn with a message containing `不是回环地址` ("is not a loopback address"). It
  also switches off the default Host check (see [allowed_hosts](#allowed_hosts--allowed_origins)).

### api_key

Injects a key into every forwarded request: sets `Authorization: Bearer <key>` and
`x-api-key: <key>`, replacing whatever the client sent in those headers. Setting both serves
OpenAI-style and Anthropic-style upstreams and keeps a stale client key from reaching the upstream
through the other header. Use it when the client cannot hold the key or you want one place to
change it. For an upstream that expects the key in another header, such as Gemini's
`x-goog-api-key`, use `override_headers` instead. It may come from settings.json `defaults.api_key`;
that value is inherited only by instances whose `base_url` names the same target as
`defaults.base_url` ([Precedence](#precedence)).

### extra_headers / override_headers

Tables of header name to value.

```toml
extra_headers    = { "http-referer" = "https://example.com", "x-title" = "my-agent" }
override_headers = { "user-agent" = "my-agent/1.0" }
```

- `extra_headers` adds a header only when the client did not send it (names compare
  case-insensitively): use it for defaults the client may override.
- `override_headers` always sets the header, replacing the client's value: use it to force a value.
- Order of application: `api_key`, then `override_headers`, then `extra_headers`. An
  `Authorization` entry in `override_headers` therefore beats `api_key`, and `extra_headers` never
  replaces a header that `api_key` set.
- An `accept-encoding` entry in `override_headers` reaches the upstream only on requests that get
  no keepalive. Keepalive requests always go upstream with `accept-encoding: identity`, because
  heartbeats cannot be inserted into a compressed stream. When your entry is shadowed this way, the
  log shows a warn at start: `override_headers 里的 accept-encoding 对保活适用的请求不生效`
  ("accept-encoding in override_headers does not apply to keepalive requests").
- An entry whose name or value is not a legal HTTP header (a space in the name, a line break in the
  value) is skipped without any message; verify the upstream receives a header that matters. Do not
  list one header twice with different capitalization: which entry wins is unspecified.
- CLI: `aproxy config set extra_headers.KEY VALUE` and `aproxy config set override_headers.KEY
  VALUE` add entries one at a time; `aproxy config unset extra_headers` and `aproxy config unset
  override_headers` empty both tables. Quote a header name that contains dots, as in
  `extra_headers."x.y"`.

### proxy / proxy_username / proxy_password

The outbound proxy for requests to the upstream. Unset, aProxy honors the proxy variables the
daemon inherited from the shell that started it (`HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY`,
`NO_PROXY`). Set, every upstream request goes through this proxy and those variables are ignored.

- Schemes: `http`, `https`, `socks4`, `socks4a`, `socks5`, `socks5h`. Use `socks5h` when host names
  must be resolved by the proxy.
- Credentials can be embedded (`socks5://user:pass@127.0.0.1:1080`) or given separately.
  `proxy_username` (with `proxy_password`) replaces embedded credentials; `proxy_password` without
  `proxy_username` is ignored. Username or password without `proxy` fails start.
- Downloads by `aproxy install` do not use this; they use settings.json
  `update.download_proxy`.
- `aproxy config unset proxy` clears the URL; also `unset proxy_username` and `unset
  proxy_password` when they were set.

### max_request_mb

The largest request body accepted, in MB (default 128, or settings.json
`defaults.max_request_mb`). A larger body gets `413` `x-aproxy-error: request_too_large` (`details.limit_bytes`) and
is not forwarded. `0` = no limit, which lets one request grow memory or disk use without bound.
Raise it for long agent sessions with images, only when you see that 413. Also enforced with
`delivery = "forward"`.

### max_response_mb

The largest upstream response aProxy buffers, in MB (default 256). A larger response is a
deterministic failure and is not retried: the client gets `502` `x-aproxy-error:
response_too_large` (`details.limit_bytes`), or the same error as the SSE frame if heartbeat
headers were already sent (behaviors.md, After the commit). Raise it for instances that relay large
files. `0` is treated as 1 MB, not as unlimited. No effect with `delivery = "forward"`.

### timeouts

Limits on talking to the upstream. Each timeout counts as a network error and is retried; each has
its own failure code in logs and in `status`'s latest error.

```toml
[timeouts]
connect_secs = 30      # TCP/TLS connection
first_byte_secs = 300  # from sending the request until the response headers arrive
read_idle_secs = 300   # gap between two reads of the response body
```

- `connect_secs` (default 30): time allowed to establish the TCP/TLS connection. On expiry the
  attempt fails with `upstream_connect_timeout`.
- `first_byte_secs` (default 300): time from sending the request (connection setup and upload of
  the request body included) until the upstream sends its response headers. On expiry the attempt
  fails with `upstream_first_byte_timeout`. Set it above the longest time an upstream can stay
  silent before its headers while healthy: queueing before the first token, or a
  `"stream": false` request that sends nothing until the whole answer is ready.
- `read_idle_secs` (default 300): the longest gap between two reads of the response body. It is
  re-armed on every read, so it does not cap the total duration of a response. On expiry the
  attempt fails with `upstream_read_idle_timeout`.
- `0` = no limit for that timeout. There is no limit on total request time. None of these affects
  the client's own timeouts (clients.md).

### keepalive

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

### retry

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

### delivery

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

### disk_cache

Whether request and response bodies larger than 1 MiB spill to temporary files under
`<APROXY_HOME>/spool/<pid>-<hash>/` instead of staying in memory. On (the built-in default, or
settings.json `defaults.disk_cache`), memory per request stays flat whatever the body size; off,
every body is held in RAM. Bodies under 1 MiB stay in
memory either way, so leave it on unless disk writes are unwanted. When a disk write or read fails,
the request ends with `507` `x-aproxy-error: local_storage_failed` (`details.op` is
`write_request`, `read_request` or `write_response`), or the same error as the SSE frame if
heartbeat headers were already sent, rather than falling back to memory.
No effect with `delivery = "forward"` (behaviors.md, "Disk cache (spool)").

### allowed_hosts / allowed_origins

Inbound checks that stop web pages in a local browser from using the proxy, and the key it injects.
CLI clients send no `Origin` header and reach the proxy as `127.0.0.1:PORT`, so they need neither
field. A refused request gets a local 403 that names the field to change and is logged as warn; it
is never forwarded, never gets a key injected and is never retried.

`allowed_hosts`, a Host header check against DNS rebinding:

- Active when `listen_addr` is a loopback address (`127.0.0.0/8`, `::1`, `localhost`) or when the
  list is non-empty. A non-loopback listener with an empty list does no Host check, because LAN and
  container clients use many different names.
- When active, it accepts `localhost`, `127.0.0.1`, `[::1]`, the host part of `listen_addr`
  (except `0.0.0.0` and `[::]`), and the list entries. Entries add to these names; they do not
  replace them.
- Compared case-insensitively, ignoring ports on both sides. Write host names only
  (`"host.docker.internal"`); a URL entry is reduced to its host.
- Add the name a container or another machine uses to reach the proxy.

`allowed_origins`, an Origin header check:

- A request carrying an `Origin` header is refused unless it matches an entry. Only browsers and
  Electron or WebView apps send `Origin`.
- Entries match case-insensitively and ignore a trailing `/`; write them as the browser sends them,
  scheme and port included: `"http://localhost:5173"`.
- Applies whatever `listen_addr` is.
- Desktop or web clients such as Cherry Studio or Open WebUI send `Origin`; add theirs, which
  `details.origin` carries and the error message quotes.

For both: an entry `"*"` turns that check off. An empty list means the built-in policy, not "allow
all" or "deny all".

```toml
allowed_hosts   = ["host.docker.internal"]
allowed_origins = ["http://localhost:5173"]
```

A refused request returns `403` with `x-aproxy-error: inbound_host_rejected`
(`details.host`) or `inbound_origin_rejected` (`details.origin`), and is logged as warn with
`入站请求被拒绝…` ("inbound request rejected"); it is never forwarded, never gets a key injected and
is never retried. The error body is the standard error JSON (behaviors.md, After the commit).

### log_file

The path of the instance's daemon log. Unset (the default), every start, restarts included, writes
a new file with a random name in `<APROXY_HOME>/logs/`. The name carries no port, so get the path
from `aproxy status` or `aproxy logs` rather than guessing it.

- Precedence: `--log-file PATH` (a `start` option, that run) > `log_file` > random name. There is no settings.json
  default; each instance chooses its own.
- `~` expands to the user's home directory; a relative path is resolved against `<APROXY_HOME>`,
  never against the working directory, which differs between `start`, `restart` and watchdog
  restarts. Missing parent directories are created.
- Set it for a stable path, or to keep a log after the instance stops: aProxy deletes `.log` files
  in `<APROXY_HOME>/logs/` that no running or restorable instance refers to whenever it lists
  instances. Files outside that directory are never deleted by aProxy.
- A custom file is appended to across restarts, but emptied at start when it is over 2 MiB, and
  while running when it grows past settings.json `logs.rotate_mb`.
- Foreground instances (`--foreground`) log to the console and ignore it.

### history

```toml
[history]
body_budget_mb = 64
```

Each instance keeps a history of the requests it handled, in memory only: the last 1000 requests
(method, path, model, every attempt and why it was retried, outcome, timing), and for as many of
the most recent ones as fit, the request body, the body sent upstream on each attempt and each
response body. `body_budget_mb` caps the memory those bodies take together (default 64).

- When the bodies exceed the budget the oldest requests lose their bodies first; their records
  stay. A single body larger than the whole budget is not kept at all.
- `0` keeps no bodies. The records themselves are always kept; they are small (a few KiB each, at
  most 64 KiB).
- Lower it, or set it to `0`, when the instance must stay small or when request and response
  content should not sit in the instance's memory. Raise it to look further back when requests
  are large: 64 MiB holds the bodies of about thirty requests of 2 MiB each.
- The instance's memory grows by up to the budget plus about 16 MiB of reusable buffers, and
  stays there; it is not returned while the instance runs.
- Bodies are stored decoded (a gzip or brotli response is kept as text). Values aProxy knows to be
  secret - `api_key`, the proxy password, secret header values, secret `[[format]]` arguments and
  environment values - are replaced before a body or header is stored. Other secrets that travel
  inside a body (a token pasted into a prompt, a key some tool printed) are not recognised and are
  kept as they are.
- The history is lost when the instance stops or restarts. Nothing is written to disk.
- This version has no command that prints the history; it is read over the instance's control
  channel (ops `requests`, `request`, `body`, `subscribe`), which only the user who started the
  instance can open.

### format

A `[[format]]` entry runs an external "format" program at chosen points of a request's life: it can
rewrite the request before it goes upstream (protocol conversion, key rotation, routing by model),
rewrite the response on its way back, decide whether a failed attempt is retried, generate
heartbeats, and observe how requests ended. aProxy itself contains no conversion logic. The
official `aproxy-format` binary provides one subcommand per role; the wire protocol and how to
write your own program are in the aproxy-format skill. This section covers the aProxy side.

`[[format]]` is an array of tables, and **the order of the list is the order of the request
direction**: the first entry is closest to the client, the last one closest to the upstream.
Responses travel the list in reverse. Any number of programs may declare the same stage.

```toml
[[format]]
name = "adaptive"
command = "aproxy-format"
args = ["adaptive", "--config", "adaptive.toml"]
process = "persistent"
stages = ["request", "decide", "notify"]
required = false          # a judge: when it breaks, skip it and keep retrying as usual
pool_max = 1              # its memory of past requests lives in one process
worker_idle_secs = 0      # ... which must not be retired for being idle

[[format]]
name = "convert"
command = "aproxy-format"
args = ["convert", "--from", "anthropic_messages", "--to", "openai_chat"]
process = "persistent"
stages = ["request", "stream"]

[[format]]
name = "keys"
command = "aproxy-format"
args = ["keys", "--config", "keys.toml"]
process = "persistent"
stages = ["request", "retry"]
pool_max = 1
worker_idle_secs = 0
```

| Key | Type | Default | Meaning |
|---|---|---|---|
| `name` | string | file stem of `command` | Identifies the program in `config show`, logs, status and the error body's `details.format`. Unique in the list, 1 to 64 bytes, no control characters. `all` is reserved (it selects every entry in `aproxy restart --format all`) and is rejected as a name, including one derived from `command`. |
| `command` | string | required | The program, run directly and never through a shell. A leading `~` expands to the user's home directory; a relative path resolves against `<home>`; a bare name is looked up in `<home>/bin/` and then on `PATH` (the `PATH` in this entry's `env` if it sets one). On Windows a bare name without an extension means `<name>.exe`, and `.cmd` and `.bat` files are rejected: they only run through cmd.exe, and killing cmd.exe leaves the real program alive. To run a script, put the interpreter in `command` and the script in `args`. |
| `args` | array of strings | `[]` | Arguments, passed verbatim (no `~` expansion). The worker's working directory is `<home>`, so a relative path in `args` resolves there. |
| `env` | table of strings | `{}` | Extra environment variables for the program; the place for secrets. `config show` lists the names only, never the values. |
| `process` | `"spawn"` \| `"persistent"` | `"spawn"` | `spawn`: a new process per message. `persistent`: a pool of long-running workers. Required for `stream` and `notify`, and for any program that keeps state between requests. |
| `stages` | array of strings | required, non-empty | Which stages this program serves (below). |
| `required` | boolean | `true` | What happens when the program itself breaks (crash, timeout, invalid reply, open breaker) at the `request` or `retry` stage: a required program fails the attempt, an optional one is skipped. It does not affect a deliberate `fail` reply, which is always obeyed. |
| `pool_max` | integer | `4` | Most pooled workers in `persistent` mode, and most `spawn` processes handling messages at once; further messages queue. `0` is rejected. Workers that own a stream in progress are not counted, so the process count can reach `pool_max` plus the number of concurrent streams. Use `1` for a program whose in-memory state must be shared by all requests. |
| `worker_idle_secs` | integer | `300` | A `persistent` worker idle this long is stopped. `0` = never; use it together with `pool_max = 1` when losing the state would matter. |
| `timeout_secs` | integer | `30` | Time limit for one reply. `0` = none. A worker that times out is killed and replaced. Not used for `decide` and `notify`. |
| `decide_timeout_secs` | integer | `2` | Time limit for a `decide` reply. A late answer counts as "no opinion"; a decision program never ends a request by being slow. |
| `breaker_failures` | integer | `5` | This many process-level failures in a row open the program's circuit breaker. `0` = no breaker. |
| `breaker_cooldown_secs` | integer | `30` | How long an open breaker waits before letting one message through as a probe. `0` is rejected. |

Stages:

| Stage | When the program is called | Typical use |
|---|---|---|
| `request` | Once per request, before the first upstream attempt | Convert the protocol, choose a key or channel, pick the delivery mode |
| `retry` | Before a retry whose request is rebuilt, with the previous failure and the previous response's status and headers | Switch key or channel after a failure |
| `decide` | After a failed attempt (and after a successful one for programs that ask) | Retry or stop, how long to wait, whether to rebuild the request |
| `response` | On a whole successful response before it is delivered | Convert a non-streaming response |
| `stream` | On a successful response chunk by chunk (`open`, `data`, `close`, `abort`) | Convert a streaming response as it flows |
| `heartbeat` | On every keepalive tick, with the bytes about to be sent | Emit a heartbeat the client's protocol understands |
| `notify` | After a request ended; aProxy does not wait for the answer | Learn from outcomes |

Rules checked at start (`config_conflict`):

- One program cannot declare both `response` and `stream`.
- A program that declares `stream` or `notify` must use `process = "persistent"`.
- In the request direction, every program listed after one that declares `retry` and that serves
  `request` must declare `retry` too. On a retry the earlier program produces a new request; a
  later one that did not run again would send the previous attempt's output. For example a
  `convert` or `rewrite` placed after `route` declares `retry`.
- Names are unique.

`admit` is reserved and reported as `config_not_supported`. Any other stage name is
`config_invalid_value`.

At start aProxy runs a handshake with every program (it must answer within 10 seconds). If a
program does not support a stage the entry declares, or speaks another protocol version, the
instance does not start (`format_stage_unsupported`, `format_handshake_failed`). If it cannot be
started at all, a required program stops the start and an optional one only produces a warning and
begins with its breaker open.

What a broken program does to requests:

| Situation | `required = true` | `required = false` |
|---|---|---|
| Crash, timeout or invalid reply at `request`/`retry` | The attempt fails with `format_error` and is retried like any failed attempt | The layer is skipped: its input is used as its output, with a warning in the log |
| Breaker open at `request`/`retry` | The request ends at once with `503` `format_unavailable` (`details.probe_at` says when the next probe is due) | The layer is skipped |
| Any failure at `decide` | Counts as "no opinion"; the retry rules and the default backoff apply | same |
| Any failure at `response` or when a stream opens | The layer is skipped and the upstream response is delivered unconverted | same |
| Any failure at `heartbeat` | That tick sends the bytes it was given | same |
| Any failure at `notify` | The notification is dropped and counted | same |

A deliberate answer is different from a failure: a program that replies `fail` with `retry: false`
ends the request with `422` `format_rejected` (or with the response it supplies), and `fail` with
`retry: true` makes the attempt fail with `format_fail` and be retried.

Other points:

- Because a skipped cross-protocol converter would hand the client the upstream's protocol, keep
  protocol converters `required = true` and make only judges (`policy`, `adaptive`) optional.
- A request-side converter alone does not convert responses. If only `request` is declared, the
  client receives the upstream's protocol; add `stream` (or `response`) to the same entry.
- Choose `timeout_secs` by how long a stuck program may hold one request, not by client timeouts:
  keepalive requests keep receiving heartbeats while a program works.
- A response passed through because a retry rule gave up is not sent through `response` or
  `stream`. To rewrite such errors in the client's protocol, use a `decide` program that stops with
  a response of its own (the official `policy` role does this).
- A program's stderr goes to the instance log, one line per line, with known secrets replaced.
  stdout carries only protocol lines.
- There is no `extra` key. Pass data through `args` or `env`.
- Apply changes with `aproxy restart <port|alias>`.

Runtime behavior and the error codes: behaviors.md, "Format programs"; ready-made roles, `init`,
`check` and `try`: the aproxy-format skill.

## Checks at start

`aproxy start`, and the precheck of `aproxy restart`, load the file strictly, apply `start` override
flags and settings.json defaults, and check the result. On failure the instance does not start and the error
reads `配置错误:` ("configuration error") followed by one line per error:

```
<file>:<line>:<column>: <key>: <message> [<code>]
```

Parts that do not apply are omitted (`<file>: <message> [<code>]` when there is no position, and
no `<key>:` when the problem is the file itself). When the bad value came from settings.json, the
line names settings.json and the key under `defaults.` instead of the toml file. `aproxy config
show` prints the same diagnostics on stderr and exits 3.

| Code | Meaning and fix |
|---|---|
| `config_syntax` | Not valid TOML. The message includes the parser's location. |
| `config_unknown_key` | A key this version does not know, including every old 0.1.x name. Remove it with `aproxy config unset KEY` or rename it (compatibility.md lists the renames). |
| `config_type` | The value has the wrong type, for example a string where an integer belongs. |
| `config_invalid_value` | A value outside its allowed set: `delivery` not `forward`/`retry`, `keepalive.trigger = "always"`, an invalid regex, a status outside 100-599, `pool_max = 0`, `breaker_cooldown_secs = 0`, a bad `base_url`, or a `keepalive.heartbeat` that breaks SSE framing. |
| `config_missing_key` | A required key is absent: `base_url` at start, `max_attempts` in a retry rule, `command` or `stages` in a format. |
| `config_conflict` | Two settings cannot hold together: two formats with the same name, one format declaring both `response` and `stream`, `stream` or `notify` without `process = "persistent"`, or a request format after a `retry` format without declaring `retry`. |
| `config_not_supported` | A shape this version parses but does not implement: the retry-rule `body` matcher, or the format stage `admit`. |

Other start checks that are not file diagnostics: the file named by `--config` must exist
(`指定的配置文件不存在`), and the listen address must bind. Both exit 1.

## Normalization

Applied every time the file is loaded:

- A trailing `/` is removed from `base_url`.
- `api_key`, `proxy`, `proxy_username` and `proxy_password` are trimmed; blank means unset.
- Header names and values are trimmed; entries with a blank name are dropped. Header tables from
  settings.json `defaults` and config.toml merge by name, case-insensitively; secret headers are
  withheld from an instance whose target differs (see [Precedence](#precedence)).
- A format's `name` and `command` are trimmed; a leading `~` in `command` is expanded; blank
  `command` is reported as an error (a format entry is not silently dropped).
- Arrays replace lower layers as a whole; the two header tables merge per name.
