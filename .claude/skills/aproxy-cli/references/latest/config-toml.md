# config.toml reference

Every field of an instance's `config.toml`: type, default, what 0 or an empty value means, and how
to choose a value; then how values combine with CLI flags and `settings.json`, what is checked at
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
  [max_retry_backoff_secs](#max_retry_backoff_secs) ·
  [connect_timeout_secs / read_timeout_secs](#connect_timeout_secs--read_timeout_secs) ·
  [bounded_retry_paths](#bounded_retry_paths) ·
  [keepalive_interval_secs / keepalive_trigger](#keepalive_interval_secs--keepalive_trigger) ·
  [max_body_mb](#max_body_mb) · [spool_limit_mb](#spool_limit_mb) · [disk_cache](#disk_cache) ·
  [forward_only](#forward_only) · [allowed_hosts / allowed_origins](#allowed_hosts--allowed_origins) ·
  [log_file](#log_file) · [request_transform / response_transform](#request_transform--response_transform)
- [Checks at start](#checks-at-start)
- [Normalization](#normalization)

## Working with config files

- **One file per instance.** The default config is `config.toml` in the aProxy home (`~/.aproxy/`,
  or the directory in `APROXY_HOME` when that is set), unless settings.json `default_config` names
  another file. Each further instance has its own file, selected with `--config PATH`,
  `aproxy start PATH` or an alias, and needs its own port in `listen_addr`, because instances are
  identified by port. Keep extra configs in `~/.aproxy/configs/`: `aproxy find` and `aproxy doctor`
  scan it without setup (other directories: settings-json.md, `config_dirs`).
- **Create or change a file with `aproxy config`, or by hand.**
  `aproxy config [--config PATH] --baseurl URL --listen ADDR ...` writes the file and creates it if
  needed (all flags: commands.md, `config`). Flags exist only for `base_url`, `listen_addr`,
  `api_key`, `extra_headers`, `override_headers`, `keepalive_interval_secs` and the three proxy
  fields; edit the rest by hand. The command rewrites the whole file in its own layout, dropping
  comments and unknown keys, so edit by hand any file whose comments matter. It will not touch a
  file it cannot parse: `现有配置文件解析失败，拒绝覆盖` ("existing config fails to parse; refusing to
  overwrite").
- **Misspelled keys are ignored silently.** aProxy does not reject unknown keys, so a typo in a key
  name simply has no effect. After editing, run `aproxy config --show [--config PATH]` and confirm
  each field shows the value you wrote. A wrong value type (a string where a number belongs,
  `mode = "Persistent"`) is different: it fails the whole file. Start then reports
  `配置文件解析失败（TOML 语法错误）` ("config file failed to parse (TOML syntax error)"), and `--show`
  warns `警告: 配置文件解析失败，以下展示的是回退默认值而非文件内容` ("the config failed to parse; the
  values below are fallback defaults, not the file's contents").
- **Apply a change with a restart.** An instance reads its config only when it starts. Run
  `aproxy restart PORT|ALIAS`: it checks the edited config first and leaves the old instance
  running if the check fails, so a typo does not take the instance down.
- **Check without applying.** `aproxy doctor` validates every config an alias points to and every
  other `*.toml` in the config directories, but not the default config unless an alias points to
  it. For the default config, `aproxy restart` or `aproxy start` is the check.

## Example

```toml
base_url = "https://api.anthropic.com"   # required
listen_addr = "127.0.0.1:12345"
# api_key = "sk-..."                      # inject the key here instead of in the client

# Optional, uncomment as needed
# proxy = "http://127.0.0.1:7890"
# read_timeout_secs = 600
# bounded_retry_paths = ['/v1/messages/count_tokens', '/v1/messages/count_tokens\?.*']
# allowed_origins = ["http://localhost:5173"]
# extra_headers = { "x-title" = "my-agent" }
```

## Field summary

"settings.json" in the Default column means the field falls back to the settings.json global
default, whose built-in value is shown (see [Precedence](#precedence)). Integers are non-negative.

| Field | Type | Default | 0 / empty means |
|---|---|---|---|
| `base_url` | string | none, required | start fails |
| `listen_addr` | string `host:port` | `"127.0.0.1:12345"` | port 0: the OS picks one |
| `api_key` | string | unset | blank = unset |
| `extra_headers` | table of strings | `{}` | |
| `override_headers` | table of strings | `{}` | |
| `proxy` | string (URL) | unset: environment proxy variables apply | blank = unset |
| `proxy_username` | string | unset | blank = unset |
| `proxy_password` | string | unset | blank = unset |
| `max_retry_backoff_secs` | integer | `320` | 0 = every retry immediate |
| `connect_timeout_secs` | integer | `30` | 0 = no limit |
| `read_timeout_secs` | integer | `300` | 0 = no limit |
| `bounded_retry_paths` | array of regex strings | settings.json, `[]` | empty = off |
| `keepalive_interval_secs` | integer | `15` | 0 = keepalive off |
| `keepalive_trigger` | `"accept"` \| `"body_stream"` \| `"any"` | settings.json, `"any"` | |
| `max_body_mb` | integer | settings.json, `128` | 0 = no limit |
| `spool_limit_mb` | integer | `256` | 0 is treated as 1 |
| `disk_cache` | boolean | settings.json, `true` | |
| `forward_only` | boolean | settings.json, `false` | |
| `allowed_hosts` | array of strings | settings.json, `[]` | empty = built-in policy; `["*"]` = check off |
| `allowed_origins` | array of strings | settings.json, `[]` | empty = built-in policy; `["*"]` = check off |
| `log_file` | string (path) | unset: random name in `<APROXY_HOME>/logs/` | |
| `request_transform` | table | unset | |
| `response_transform` | table | unset | |

## Precedence

Highest first:

1. **Run-only CLI flags** `--baseurl`, `--listen`, `--proxy`, `--api-key` and `--log-file`, given
   to `aproxy` before any subcommand, as in `aproxy --listen 127.0.0.1:12399 start work` (syntax:
   commands.md). They are never written to a file, but they become part of the instance's recorded
   start arguments, so `aproxy restart`, `aproxy restore` and watchdog restarts keep applying them.
   To drop one, stop the instance and start it again without the flag. Prefer the file over
   `--api-key`: command lines are visible to other processes on the machine.
2. **config.toml.**
3. **The settings.json global default**, which exists only for `max_body_mb`, `disk_cache`,
   `forward_only`, `bounded_retry_paths`, `allowed_hosts`, `allowed_origins` and
   `keepalive_trigger` (settings-json.md, "Global defaults for config.toml fields").
4. **The built-in default.**

A key present in config.toml always wins over settings.json, even when its value equals the
built-in default. So `allowed_hosts = []`, `allowed_origins = []` or `bounded_retry_paths = []` in
one config restores the built-in behavior for that instance even if settings.json sets a list.
When one of these seven fields is not in the toml, `aproxy config --show` prints
`（未在 toml 设置，运行时取 settings.json 全局默认）` ("not set in the toml; the settings.json global
default applies at runtime") instead of the effective value; read settings.json for that.

## Fields

### base_url

The upstream API root. aProxy appends the client's full request path and query unchanged, so
`base_url` plus the path the client sends must form the real upstream URL: if the client's base
URL is `http://127.0.0.1:12345/v1` and it calls `/v1/chat/completions`, set
`base_url = "https://api.example.com"`, not `.../v1`.

- Must start with `http://` or `https://` (any case) and must not contain `?` or `#`, which would
  turn the appended path into part of the query.
- A trailing `/` is removed.

### listen_addr

The `host:port` the instance listens on. The default `127.0.0.1:12345` is reachable only from this
machine.

- The port is required; without one, start fails with `listen_addr 缺少端口或端口无效` ("listen_addr
  lacks a port or the port is invalid").
- Every instance needs its own port. A second instance on a port that another aProxy instance
  already uses is refused, even with a different host part.
- Port `0` lets the OS choose; read the port from `aproxy status`. Clients need a fixed address, so
  use it only for throwaway tests.
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
`x-goog-api-key`, use `override_headers` instead.

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
- CLI: `aproxy config --extra-header KEY=VALUE` and `--override-header KEY=VALUE` (repeatable) add
  entries; `--clear-headers` empties both tables.

### proxy / proxy_username / proxy_password

The outbound proxy for requests to the upstream. Unset, aProxy honors the proxy variables the
daemon inherited from the shell that started it (`HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY`,
`NO_PROXY`). Set, every upstream request goes through this proxy and those variables are ignored.

- Schemes: `http`, `https`, `socks4`, `socks4a`, `socks5`, `socks5h`. Use `socks5h` when host names
  must be resolved by the proxy.
- Credentials can be embedded (`socks5://user:pass@127.0.0.1:1080`) or given separately.
  `proxy_username` (with `proxy_password`) replaces embedded credentials; `proxy_password` without
  `proxy_username` is ignored. Username or password without `proxy` fails start.
- Downloads by `aproxy install` do not use this; they use settings.json `download_proxy`.
- `aproxy config --clear-proxy` clears all three fields.

### max_retry_backoff_secs

The longest wait between retries. The first three retries are immediate; from the fourth the wait
is 5 s and doubles (10, 20, 40 ...) until it reaches this cap, then stays there; a cap below 5
makes every delayed retry wait exactly the cap. Retries never stop;
the cap only sets how often aProxy tries. Default 320. `0` makes every retry immediate, which hammers
an upstream that is down; for fast recovery prefer a small cap such as 10. Retry rules: behaviors.md,
"Retry decisions".

### connect_timeout_secs / read_timeout_secs

Limits on the connection to the upstream. A timeout counts as a network error and is retried.

- `connect_timeout_secs` (default 30): time allowed to establish the TCP/TLS connection.
- `read_timeout_secs` (default 300): the longest gap between two reads from the upstream, including
  the wait for the first byte. Set it above the longest time the upstream can stay silent while
  healthy: queueing before the first token, or a `"stream": false` request that sends nothing until
  the whole answer is ready. Too small, and a slow but healthy upstream times out on every attempt,
  so the request retries forever.
- `0` = no limit. There is no limit on total request time. Neither field affects the client's own
  timeouts (clients.md).

### bounded_retry_paths

Regex patterns for requests that should stop retrying. When a matching request keeps getting
failure responses (an error status, or `200` with an error body), aProxy stops after 3 attempts in
total and passes the last upstream response to the client unchanged. Network errors are still
retried without limit. Empty (the default) = off.

Use it when an upstream deterministically fails an endpoint the client depends on, so the client
gets the real error instead of waiting forever. The typical case: Claude Code behind a relay or
mirror API that does not implement `POST /v1/messages/count_tokens`, so `/compact` hangs until the
client times out. Which endpoints need this depends entirely on the upstream, so aProxy ships no
patterns.

```toml
# Single quotes: TOML keeps backslashes literally, so \? needs no doubling
bounded_retry_paths = [
  '/v1/messages/count_tokens',       # this path with no query string
  '/v1/messages/count_tokens\?.*',   # this path with any query string (Claude Code sends ?beta=true)
]
```

- Each pattern must match the whole request target, `path?query`; patterns are anchored at both
  ends. A plain path therefore matches only that path without a query string.
- Patterns are regular expressions. Write a literal `?` as `\?`, and escape `.`, `+`, `(`, `[`,
  `$`, `^` when you mean the character. A stray `$` or `^` gives a valid pattern that never matches,
  and nothing reports it.
- Matching is case-sensitive and sees the target exactly as the client sent it, percent-encoding
  included: write a space in a query as `%20`.
- Logs mask query values (`?beta=***`), but matching uses the real query, so write real values.
- Matching uses the client's original path, before any request transformer rewrites it.
- An invalid regex fails start, and the error quotes the pattern.

When the cap is reached, the log shows the warn line
`受限重试路径达到尝试上限，透传最后一次上游响应` ("bounded-retry path reached its attempt cap; passing
the last upstream response through"); look for it to confirm a pattern matches. If heartbeat headers
had already been sent to the client, the request ends with an SSE `event: error` instead of the
upstream status (behaviors.md, "Keepalive heartbeats").

### keepalive_interval_secs / keepalive_trigger

While aProxy waits for and retries a request that qualifies for keepalive, it sends the client
response headers early and then an SSE comment (`: keepalive`) every `keepalive_interval_secs`; the
body is still buffered and replayed only after a successful attempt (behaviors.md, "Keepalive
heartbeats").

- `keepalive_interval_secs` (default 15): the heartbeat period, and also how long aProxy waits for
  upstream headers before sending its own. Keep it below the shortest byte-level idle timeout
  between client and aProxy. `0` turns keepalive off for the instance. CLI:
  `aproxy config --keepalive-secs N`.
- `keepalive_trigger` (default `"any"`): which requests qualify. No CLI flag.

| Value | A request qualifies when |
|---|---|
| `"accept"` | the client's `Accept` header contains `text/event-stream` |
| `"body_stream"` | the client's body is a JSON object with top-level `"stream": true` |
| `"any"` | either holds |

Keep `"any"`. Claude Code's streaming requests send `Accept: application/json` with
`"stream": true`, so `"accept"` alone would leave them without heartbeats. Switch an instance to
`"accept"` when its upstream answers `"stream": true` with something other than SSE (Ollama's native
API streams NDJSON, for example) and often needs retries: once aProxy has sent SSE headers during a
retry, a non-SSE body no longer fits the response. Only the three lowercase values are accepted.

- The decision uses what the client sent, before any request transformer.
- No keepalive for requests that do not qualify (notably `"stream": false`: the client's own
  timeout must cover the whole generation), for `forward_only` instances, and when
  `keepalive_interval_secs = 0`.
- Heartbeats defeat byte-level idle timeouts only. Clients that time out on SSE events need their
  own timeout raised (clients.md).

### max_body_mb

The largest request body accepted, in MB. A larger body gets 413
`请求体超出上限（…），可在 settings.json 的 max_body_mb 或 config.toml 的 max_body_mb 调整` ("request body
exceeds the limit (…); adjust max_body_mb in settings.json or config.toml") and is not forwarded.
`0` = no limit, which lets one request grow memory or disk use without bound. The default 128 covers
long agent sessions with images; raise it only if you see that 413. Also enforced with
`forward_only`.

### spool_limit_mb

The largest upstream response aProxy buffers, in MB (default 256). A larger response is a
deterministic failure and is not retried: the client gets 502
`上游响应体超出 spool 上限，无法回放（重试无意义）` ("upstream response exceeds the spool limit; cannot
replay (retrying is pointless)"), or an SSE `event: error` of type `proxy_spool_limit` if heartbeat
headers were already sent. Raise it for instances that relay large files. `0` is treated as 1 MB,
not as unlimited. No effect with `forward_only`.

### disk_cache

Whether request and response bodies larger than 1 MiB spill to temporary files under
`<APROXY_HOME>/spool/<port>/` instead of staying in memory. On (the built-in default), memory per
request stays flat whatever the body size; off, every body is held in RAM. Bodies under 1 MiB stay in
memory either way, so leave it on unless disk writes are unwanted. When a disk write fails, the
request ends with 502 `本地磁盘缓存写入失败，无法回放` ("local disk cache write failed; cannot replay"),
or an SSE `event: error` if heartbeat headers were already sent, rather than falling back to memory.
No effect with `forward_only` (behaviors.md, "Disk cache (spool)").

### forward_only

`true` turns the instance into a plain streaming pass-through: request and response bytes are
relayed as they arrive, with no buffering, no retries, no heartbeats and no error-body detection.
This gives up aProxy's core guarantee: an upstream failure becomes a 502 to the client, and a stream
that breaks mid-way is cut off where it broke. Enable it only for a trusted upstream when the client
needs true incremental streaming, and keep the settings.json default `false` so other instances
keep retrying.

- Still applied: `api_key`, `extra_headers`, `override_headers`, the inbound checks, and
  `max_body_mb` (counted while streaming).
- Ignored: `disk_cache`, `spool_limit_mb`, `keepalive_interval_secs`, `keepalive_trigger`,
  `max_retry_backoff_secs`, `bounded_retry_paths`.
- Cannot be combined with `request_transform` or `response_transform`, which need the whole body;
  start fails, also when `forward_only` comes from settings.json.
- No CLI flag. When it is on, `aproxy start` prints `仅转发模式：不缓冲、不重试` ("forward-only mode: no
  buffering, no retries").

Runtime details: behaviors.md, "Forward-only mode".

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
- Desktop or web clients such as Cherry Studio or Open WebUI send `Origin`; add theirs, which the
  403 body quotes.

For both: an entry `"*"` turns that check off. An empty list means the built-in policy, not "allow
all" or "deny all".

```toml
allowed_hosts   = ["host.docker.internal"]
allowed_origins = ["http://localhost:5173"]
```

403 bodies start with `aProxy 拒绝了该请求（403，未转发上游）` ("aProxy refused the request (403, not
forwarded upstream)"), followed by `Host「…」不在允许列表内` ("Host … is not in the allowed list") or
`来自浏览器页面的请求（Origin「…」）默认不放行` ("requests from browser pages (Origin …) are refused by
default").

### log_file

The path of the instance's daemon log. Unset (the default), every start, restarts included, writes
a new file with a random name in `<APROXY_HOME>/logs/`. The name carries no port, so get the path
from `aproxy status` or `aproxy logs` rather than guessing it.

- Precedence: `--log-file PATH` (that run) > `log_file` > random name. There is no settings.json
  default; each instance chooses its own.
- `~` expands to the user's home directory; a relative path is resolved against `<APROXY_HOME>`,
  never against the working directory, which differs between `start`, `restart` and watchdog
  restarts. Missing parent directories are created.
- Set it for a stable path, or to keep a log after the instance stops: aProxy deletes `.log` files
  in `<APROXY_HOME>/logs/` that no running or restorable instance refers to whenever it lists
  instances. Files outside that directory are never deleted by aProxy.
- A custom file is appended to across restarts, but emptied at start when it is over 2 MiB, and
  while running when it grows past settings.json `log_rotate_mb`.
- Foreground instances (`--foreground`) log to the console and ignore it.

### request_transform / response_transform

Hand each request before forwarding, or each successful response before replay, to an external
"format" program that rewrites it: protocol conversion (OpenAI to Anthropic and back), key
rotation, routing models to different channels. aProxy has no transformer built in. The official
`aproxy-format` binary, the envelope protocol and how to write your own program are in the
aproxy-format skill; this section covers only the aProxy side. Default: unset.

```toml
request_transform  = { command = "~/.aproxy/bin/aproxy-format", args = ["run"], mode = "persistent", extra = "~/.aproxy/agg.toml" }
response_transform = { command = "~/.aproxy/bin/aproxy-format", args = ["run"], mode = "persistent", extra = "~/.aproxy/agg.toml" }
```

| Key | Type | Default | Meaning |
|---|---|---|---|
| `command` | string | required | The program. Run directly, not through a shell. A leading `~` expands to the user's home directory; a bare name is looked up on `PATH`. |
| `args` | array of strings | `[]` | Arguments, passed verbatim (no `~` expansion). |
| `mode` | `"spawn"` \| `"persistent"` | `"spawn"` | `spawn`: a new process per request. `persistent`: a pool of long-running workers; required when the program keeps state between requests (key rotation, counters). |
| `pool_max` | integer | `4` | Most workers in `persistent` mode; further requests queue. `0` fails start. Unused in `spawn` mode. |
| `idle_timeout_secs` | integer | `300` | A `persistent` worker idle this long is stopped. `0` = never. |
| `timeout_secs` | integer | `30` | Limit for one transformation. `0` = none. A worker that times out is killed and replaced, and that transformation counts as failed. |
| `extra` | string | unset | Copied verbatim into every envelope's `extra` field; its meaning is up to the program. The official binary expects the path of its config here and expands `~/` itself. |

- Write `command` with `~/` or an absolute path. A relative path resolves against the daemon's
  working directory, which depends on where `start` or `restore` happened to run.
- `~` means the user's home directory even under another `APROXY_HOME`; in an isolated test home,
  use absolute paths.
- With the official binary, give the response side the same `extra` as the request side: it reads
  its config from `extra`, and without it every response transformation fails.
- The two sides fail differently. A request-side failure returns 502 to the client, or an SSE
  `event: error` of type `proxy_transform_failed` if heartbeat headers were already sent; the
  request is not sent upstream and not retried. A response-side failure logs a warn and passes the
  upstream response through untransformed.
- Choose `timeout_secs` by how long a stuck program may hold one request, not by client timeouts:
  keepalive requests keep receiving heartbeats while a transformation runs. With `0`, a program
  that hangs holds that request indefinitely and its worker is never replaced.
- A request is transformed once and every retry replays the result, so a rotating key changes
  between requests, not between retries. Responses passed through by `bounded_retry_paths` are not
  transformed.
- aProxy discards the program's stderr; have the program write its own log file.
- Cannot be combined with `forward_only`.
- A table whose `command` is blank is ignored as if absent; a table without a `command` key fails
  to parse.

Runtime behavior and the 502 texts: behaviors.md, "External transformers"; aproxy-format skill,
troubleshooting.md.

## Checks at start

`aproxy start`, and the precheck of `aproxy restart`, load the file, apply CLI flags and settings.json
defaults, and check the result. On failure the instance does not start, and the error reads
`配置错误: <reason>` ("configuration error") followed by `位置: <file>` ("location"). A daemon that
fails during its own startup writes the error to `<APROXY_HOME>/logs/startup.log`. When the bad value
came from settings.json, the message adds `（该值来自 settings.json 的 … 全局默认，不在上述 toml 中）`
("the value comes from the settings.json global default, not from the toml above").

| Reason (verbatim prefix) | Meaning and fix |
|---|---|
| `配置文件解析失败（TOML 语法错误）` | Not valid TOML, a value of the wrong type, an invalid `mode`, or a transformer table without `command`. The message includes the parser's location. |
| `指定的配置文件不存在` | The file given with `--config` does not exist. |
| `base_url 不能为空` | `base_url` is missing or blank. This is also what you get when the default config file does not exist yet. |
| `base_url 必须以 http:// 或 https:// 开头` | "must start with http:// or https://". |
| `base_url 不应包含 ? 或 #` | "must not contain ? or #": move query parameters out of `base_url`. |
| `listen_addr 缺少端口或端口无效` | "lacks a port or the port is invalid". |
| `proxy 配置无效` | "proxy is invalid": the URL does not parse. |
| `proxy 仅支持 http/https/socks4/socks5 协议` | "proxy supports only http/https/socks4/socks5": unsupported scheme. |
| `proxy 缺少主机地址` | "proxy lacks a host". |
| `配置了 proxy_username/proxy_password 但未配置 proxy URL` | Credentials without `proxy`. |
| `bounded_retry_paths 含非法正则` | "contains an invalid regex"; the pattern is quoted. |
| `keepalive_trigger 取值无效` | "invalid value": use `accept`, `body_stream` or `any`. |
| `request_transform 的 pool_max 必须 >= 1` (or `response_transform …`) | `pool_max = 0` with `mode = "persistent"`. |
| `forward_only 与外部转换器互斥` | "forward_only and external transformers are mutually exclusive": keep one. |

## Normalization

Applied every time the file is loaded:

- A trailing `/` is removed from `base_url`.
- `api_key`, `proxy`, `proxy_username` and `proxy_password` are trimmed; blank means unset.
- Header names and values are trimmed; entries with a blank name are dropped.
- A transformer's `command` is trimmed and a leading `~` expanded; blank means the transformer is
  unset.
