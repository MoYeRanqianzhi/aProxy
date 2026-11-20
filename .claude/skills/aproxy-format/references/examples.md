# The official aproxy-format binary: roles, chains and tools

Working configurations built from the official roles. Read it when a user wants protocol
conversion, key rotation, routing, a retry policy or adaptive delivery and you would rather
configure than write code. Every role file and chain below was accepted by `aproxy-format check`.

## Contents

- [Install](#install)
- [How a role is wired in](#how-a-role-is-wired-in)
- [Starter scenarios (`init`)](#starter-scenarios-init)
  - [claude-relay: one protocol converter](#claude-relay-one-protocol-converter)
  - [key-rotation: several keys for one relay](#key-rotation-several-keys-for-one-relay)
  - [openrouter: a channel plus conversion](#openrouter-a-channel-plus-conversion)
  - [adaptive-relay: true streaming when it is safe](#adaptive-relay-true-streaming-when-it-is-safe)
- [Role reference](#role-reference): [convert](#convert), [keys](#keys), [route](#route),
  [policy](#policy), [adaptive](#adaptive), [rewrite](#rewrite), [heartbeat](#heartbeat)
- [Mixed channels: route, then convert per destination](#mixed-channels-route-then-convert-per-destination)
- [A full chain](#a-full-chain)
- [Known conversion limits](#known-conversion-limits)
- [Tools: check, try, translate](#tools-check-try-translate)

## Install

`aproxy-format` is released separately from aProxy and is not installed or upgraded by
`aproxy install`. The roles need a build that has them: `aproxy-format keys --help` printing a
usage line is the test (an older build answers with an error).

| Source | How | Notes |
|---|---|---|
| GitHub Releases | Release tagged `format-v<version>`, asset `aproxy-format-<target>[-v3][.exe]` with a `.sha256` beside it | Save it as `<home>/bin/aproxy-format` (`aproxy-format.exe` on Windows; the home is usually `~/.aproxy`); on Unix `chmod +x` it. `-v3` builds need an x86-64-v3 CPU (AVX2); take the plain build if unsure. |
| npm | `npm install -g @meowo/aproxy-format` | Installs a Node.js launcher on PATH. On Windows that launcher is a `.cmd` shim, which aProxy refuses as a `command`; use the release binary there. |
| crates.io | `cargo install aproxy-format --locked` | Builds from source. |

A bare `command = "aproxy-format"` is looked up in `<home>/bin/` first and then on PATH, so the
release binary saved there needs no path in the configuration.

## How a role is wired in

Each role is one `[[format]]` entry whose `args` start with the role name. A role declares in its
hello which stages it implements; `stages` in the entry picks from those. Three settings are the
same for every role and worth knowing before reading the examples:

- **`process = "persistent"` always.** The roles are long-lived workers; `stream` and `notify`
  require it anyway.
- **Role settings live in the role's own TOML file** (`--config <file>`), not in config.toml.
  The files are strict: an unknown key is an error. A relative path is resolved from the
  instance's home, because that is the worker's working directory.
- **Secrets are never written in a file.** Role files name environment variables; the values come
  from the instance's environment or from the entry's `env` table.

Entry order is the request direction, client side first. The rule that shapes most chains: once
an entry declares `retry`, every entry after it must declare `retry` too, because a retry reruns
the chain from the first `retry` entry and what follows has to be redone for the new destination.

## Starter scenarios (`init`)

`aproxy-format init <scenario> --output <dir>` writes a commented starting point and prints what
it created and what to do next. It never overwrites a file and never writes a secret. The
generated `args` hold absolute paths to the role files, so the result works wherever the files
stay; after moving them into the instance's home you can shorten the paths to relative ones. The
four scenarios, as hand-written equivalents:

### claude-relay: one protocol converter

A client that speaks Anthropic Messages (Claude Code) in front of a relay that speaks OpenAI Chat.

```toml
base_url = "https://relay.example.com"

[[format]]
name = "convert"
command = "aproxy-format"
args = ["convert", "--from", "anthropic_messages", "--to", "openai_chat"]
process = "persistent"
stages = ["request", "stream"]
required = true
```

`stream` covers streaming and non-streaming answers alike (an entry cannot declare both `response`
and `stream`). Subscribing only to `request` converts requests and leaves responses in the
upstream's protocol, which is almost never what a client wants; `check` reports it.

### key-rotation: several keys for one relay

```toml
# config.toml
base_url = "https://relay.example.com"

[[format]]
name = "keys"
command = "aproxy-format"
args = ["keys", "--config", "keys.toml"]
process = "persistent"
stages = ["request", "retry"]
pool_max = 1
worker_idle_secs = 0
```

```toml
# keys.toml
[[pool]]
origin = "https://relay.example.com"
header = "authorization"
scheme = "Bearer"
keys = [
    { id = "a", env = "RELAY_KEY_A" },
    { id = "b", env = "RELAY_KEY_B" },
]
```

`pool_max = 1` and `worker_idle_secs = 0` are not optional decoration: cooldowns and the rotation
position live in the worker's memory, so a second worker would hand out a key the first one just
cooled, and an idle-reaped worker forgets every cooldown.

### openrouter: a channel plus conversion

```toml
# config.toml
base_url = "https://openrouter.ai"

[[format]]
name = "route"
command = "aproxy-format"
args = ["route", "--config", "route.toml"]
process = "persistent"
stages = ["request", "retry"]

[[format]]
name = "convert"
command = "aproxy-format"
args = ["convert", "--from", "auto", "--to", "openai_chat"]
process = "persistent"
stages = ["request", "retry", "stream"]
required = true
```

```toml
# route.toml
[[channel]]
id = "openrouter"
url = "https://openrouter.ai/api/v1/chat/completions"
credential_env = "OPENROUTER_API_KEY"
```

`convert` sits after `route`, so it declares `retry` as well. `--from auto` recognises the
client's protocol per request from its path and body, so one instance serves Messages, Chat and
Responses clients.

### adaptive-relay: true streaming when it is safe

```toml
# config.toml
base_url = "https://relay.example.com"

[[format]]
name = "adaptive"
command = "aproxy-format"
args = ["adaptive", "--config", "adaptive.toml"]
process = "persistent"
stages = ["request", "decide", "notify"]
required = false
pool_max = 1
worker_idle_secs = 0

[[format]]
name = "convert"
command = "aproxy-format"
args = ["convert", "--from", "anthropic_messages", "--to", "openai_chat"]
process = "persistent"
stages = ["request", "stream"]
required = true

[[format]]
name = "keys"
command = "aproxy-format"
args = ["keys", "--config", "keys.toml"]
process = "persistent"
stages = ["request", "retry"]
pool_max = 1
worker_idle_secs = 0
```

```toml
# adaptive.toml
failure_threshold = 2
```

`keys.toml` is the same file as in key-rotation.

The generated config.toml also raises `timeouts.first_byte_secs` and caps attempts on the
token-counting endpoint; those are instance settings (aproxy-cli skill), not part of the chain.
Leave the instance's `delivery` at its default `retry`: `adaptive` is optional, and when it is
unavailable ordinary retrying must still be what happens.

## Role reference

### convert

Converts between `anthropic_messages`, `openai_chat` and `openai_responses`: requests, buffered
responses, and streams event by event.

```toml
# convert.toml (alternative to --from/--to on the command line)
from = "auto"                    # or a protocol name
to = "openai_chat"               # or "auto", see mixed channels below
stream_budget_bytes = 67108864   # default 64 MiB of source events per stream
```

- **Stages:** `request` and `retry` (full body), plus `response` or `stream`.
- **`from = "auto"`** decides per request; a request it cannot classify is rejected instead of
  being delivered in a guessed shape. On a non-standard path where the body fits two protocols,
  write `from` explicitly.
- **A written `to`** also moves the request to that protocol's standard endpoint path. `to =
  "auto"` leaves the URL exactly as the router set it.
- **Same protocol on both sides passes through untouched** - no JSON requirement, no header edits.
- **A cross-protocol `convert` must be `required = true`.** If it were skipped after part of a
  stream had been converted, the client would receive two protocols in one response. `check`
  rejects `required = false` here.
- **When a stream goes wrong after output began** (a malformed event, the budget exceeded), the
  client receives one error ending in its own protocol; what was already delivered stays. This is
  not a worker error and does not trip the breaker.
- Upstream error responses are not converted. To give the client an error in its own protocol,
  use a `policy` rule with `decision = "stop"` and a `response` template.

### keys

```toml
strategy = "weighted"              # round_robin (default), weighted, random
sticky = false
cooldown_status = [401, 403, 429]  # default
cooldown_secs = 30                 # default

[[pool]]
origin = "https://relay.example.com"
header = "x-api-key"               # must be a credential header
keys = [
    { id = "a", env = "RELAY_KEY_A", weight = 2 },
    { id = "b", env = "RELAY_KEY_B", weight = 1 },
]

[[pool]]
origin = "https://api.openai.example"
header = "authorization"
scheme = "Bearer"
keys = [{ id = "main", env = "OPENAI_KEY" }]
```

- **Stages:** `request` and `retry`. No request body unless `sticky = true`.
- **The pool is chosen by where the request is going** (scheme, host and port of the current
  upstream URL), not by what the client sent. A destination that matches no pool is rejected, so
  every channel a router can pick needs a pool or its own `credential_env`.
- **Cooldown:** a key whose attempt ended in a `cooldown_status` rests for `cooldown_secs` or the
  upstream's `Retry-After`, whichever is longer (capped at 24 hours). When every key of the pool
  is resting the attempt fails as retryable rather than sending a resting key.
- **Cooling only happens when the retry chain actually reruns.** It reruns by default. After a
  `decide` vote with `reroute: false` aProxy resends the request it already prepared and `keys`
  is not consulted, so a `policy` rule meant to rotate keys needs `reroute = true` (or no
  `reroute` at all), never `false`.
- **A one-key pool cannot rotate:** after a 429 its only key is resting, and retries are refused
  until the cooldown ends. Give the pool a second key or remove 429 from `cooldown_status`.
- **`sticky = true`** keeps a conversation on one key (useful for upstream prompt caches): the
  choice is a hash of the first user text, and yields to cooldown.
- Every key's environment variable is checked before the handshake; a missing one stops the
  worker from starting and the message names the variable, never the value.

### route

```toml
sticky = false

[aliases]
small = "claude-haiku-4-5"         # the client may ask for "small"

[[channel]]
id = "anthropic"
url = "https://relay.example.com/anthropic/v1/messages"
models = [{ prefix = "claude-" }]  # also { exact = "..." } and { regex = "..." }
priority = 1                       # lower number first; default 0
weight = 2                         # share within a priority; default 1

[[channel]]
id = "openai"
url = "https://api.openai.example/v1/chat/completions"
models = [{ prefix = "gpt-" }, { regex = "^o[0-9]" }]
priority = 1

[[channel]]
id = "fallback"
url = "https://api.openai.example/v1/chat/completions"
models = [{ prefix = "claude-" }]
priority = 2

[channel.overrides]                # applies to the channel just above
"timeouts.first_byte_secs" = 90

[[rule]]
model = "small"                    # matched against the model the client sent

[rule.overrides]
"retry.max_attempts" = 3
```

- **Stages:** `request` and `retry` (full body); optionally `response`, only to map an alias back.
- **Choosing:** aliases are resolved first; among the channels whose `models` match, the lowest
  `priority` group wins and `weight` splits traffic inside it. A channel without `models` matches
  every model.
- **Failover:** on a retry that reruns the chain, the next channel of the same group is tried in
  configuration order, then the next priority group; it wraps around only after every matching
  channel had its turn.
- **Credentials never cross destinations.** When the destination changes, `route` strips every
  credential header and cookie, then applies the channel's own `credential_env` (sent in
  `auth_header`, which must be a credential header) or leaves the slot for a `keys` entry later
  in the chain.
- **URL:** the channel's `url` is used as written. `preserve_path = true` appends the client's
  original path instead. The query string is kept within one origin and dropped across origins
  except for the names listed in `carry_query`.
- **Overrides** are quoted dotted config.toml keys with the value type that key has there; an
  unknown key or bad value stops the worker at startup. A `[[rule]]` for the exact client model
  beats the chosen channel's value. Overrides reset when the channel changes.
- **`sticky = true`** starts each conversation on a stable channel within the top group.
- **Mapping aliases back:** add `"response"` to `stages` and the upstream's model name in a JSON
  response is replaced by the alias the client used. That stage buffers the response, so leave it
  out for clients that stream.

### policy

```toml
[[rule]]
id = "rate-limited"                # optional; the rule number is used otherwise
status = [429, 503]
decision = "retry"
reroute = true                     # rerun the retry chain: new key, new channel
respect_retry_after = true

[[rule]]
id = "context"
failure = "upstream_error_body"
body_regex = "context length"
decision = "stop"
response = "context_too_long"      # or "model_not_configured"
```

```toml
[[format]]
name = "policy"
command = "aproxy-format"
args = ["policy", "--config", "policy.toml"]
process = "persistent"
stages = ["decide"]
required = false
```

- **Stage:** `decide` only, no request body, no state between requests.
- **Rules are tried in order; the first whose conditions all hold decides.** No match means no
  vote, and the instance's own retry rules apply.
- `status` matches the upstream status, `failure` the attempt's failure code (see
  `docs/error-codes.md` in the repository, or protocol.md), `body_regex` the response preview.
  The preview of a large body is its head and tail: a pattern that does not match there proves
  nothing about the middle.
- `delay_ms` sets the wait; `respect_retry_after` can only lengthen it, up to 24 hours.
- `decision = "stop"` with `response` ends the request with a freshly written error in the
  client's protocol: `context_too_long` (400) or `model_not_configured` (404). Nothing from the
  upstream body is copied.
- A rule that names success statuses makes the role ask for successful attempts as well, which is
  how a 200 carrying an error can be retried.

### adaptive

```toml
failure_threshold = 2          # all values shown are the defaults
max_entries = 4096
ttl_secs = 900
hold_secs = 60
recovery_samples = 10
recovery_success_ratio = 0.9   # must be greater than 0
recovery_latency_ms = 1000
```

- **Stages:** `request`, `decide` and `notify`; it needs only the request digest, never the body.
- A request whose digest has no bad history is delivered in `forward` mode: bytes flow to the
  client as the upstream produces them. If such an attempt fails before anything reached the
  client, it votes for an upgrade to `retry` mode, and a digest that failed or was upgraded
  `failure_threshold` times is pinned to `retry`.
- A pinned digest is released after `hold_secs` plus one full window of `recovery_samples`
  requests that meets both the success ratio and the latency bound.
- It learns from `notify`, which is dropped under heavy load; treat it as an optimisation.
  Configure it `required = false`, `pool_max = 1`, `worker_idle_secs = 0`.

### rewrite

```toml
remove_headers = ["x-legacy"]
set_headers = { "x-service" = "chat" }
remove = ["/metadata/debug"]       # JSON Pointers into the request body
set = { "/temperature" = 0.5 }
models = { small = "provider-small" }
```

```toml
[[format]]
name = "rewrite"
command = "aproxy-format"
args = ["rewrite", "--config", "rewrite.toml"]
process = "persistent"
stages = ["request", "retry"]
```

- **Stages:** `request` and `retry`. The body is requested only when `remove`, `set` or `models`
  is present.
- Order: removals, then sets, then the exact model rename. Setting needs an existing parent;
  an array index must exist, `-` appends. Removing something absent does nothing.
- `set_headers` refuses credential headers and the headers aProxy manages (`host`,
  `content-length` and the like): credentials belong to `keys` and `route`, which bind them to a
  destination. Removing a credential header is allowed.
- Untouched parts of the body keep their number precision and member order. Two things do change
  in a body that is edited: a duplicated member collapses to its last occurrence, and string
  escapes may be re-rendered to an equal string.

### heartbeat

```toml
[[preset]]
path = "/v1/messages"
protocol = "anthropic_messages"
event = "ping"
```

```toml
[[format]]
name = "heartbeat"
command = "aproxy-format"
args = ["heartbeat"]               # no file: the preset above is the built-in default
process = "persistent"
stages = ["heartbeat"]
required = false
```

- While a client waits in `retry` mode, aProxy emits a keepalive beat; this role turns the beat on
  `/v1/messages` into a native Anthropic `ping` event, which SDK watchdogs that count events
  accept. Given a file, only the listed paths are affected.
- **On Chat and Responses paths the beat is left as it is** (an SSE comment). Inventing an event
  there before the upstream's first real one would either give the response a false identity
  (Chat SDKs remember the first chunk's ID) or break the required event order (Responses must
  start with `response.created`). So for those two protocols the keepalive holds the connection
  open and nothing more; it does not feed a watchdog that counts events.

## Mixed channels: route, then convert per destination

Channels that speak different protocols behind one instance. `convert` goes after `route`,
declares `retry`, and uses `to = "auto"` with a table that says which destination speaks what:

```toml
# convert.toml
from = "auto"
to = "auto"

[[upstream]]
origin = "https://relay.example.com"
path_prefix = "/anthropic"         # optional; longest matching prefix wins
protocol = "anthropic_messages"

[[upstream]]
origin = "https://api.openai.example"
protocol = "openai_chat"
```

```toml
[[format]]
name = "route"
command = "aproxy-format"
args = ["route", "--config", "route.toml"]
process = "persistent"
stages = ["request", "retry"]

[[format]]
name = "convert"
command = "aproxy-format"
args = ["convert", "--config", "convert.toml"]
process = "persistent"
stages = ["request", "retry", "stream"]
required = true

[[format]]
name = "keys"
command = "aproxy-format"
args = ["keys", "--config", "keys.toml"]
process = "persistent"
stages = ["request", "retry"]
pool_max = 1
worker_idle_secs = 0
```

When a retry moves the request to another channel, `convert` runs again and converts for the new
destination; the response is converted back from whichever protocol the delivered attempt used.
A destination that is in no `[[upstream]]` row is recognised by a standard endpoint path, and
rejected if that fails too - it is never guessed. With `to = "auto"` each channel's `url` must be
the full endpoint, since `convert` does not touch the path.

## A full chain

Client side first. The observers (`adaptive`, `policy`, `heartbeat`) can sit anywhere among the
entries that do not declare `retry`; the request-shaping entries follow in the order route,
convert, rewrite, keys so that each works on the previous one's result and all four rerun on a
rerouted retry.

```toml
base_url = "https://relay.example.com"

[[format]]
name = "adaptive"
command = "aproxy-format"
args = ["adaptive", "--config", "adaptive.toml"]
process = "persistent"
stages = ["request", "decide", "notify"]
required = false
pool_max = 1
worker_idle_secs = 0

[[format]]
name = "policy"
command = "aproxy-format"
args = ["policy", "--config", "policy.toml"]
process = "persistent"
stages = ["decide"]
required = false

[[format]]
name = "heartbeat"
command = "aproxy-format"
args = ["heartbeat"]
process = "persistent"
stages = ["heartbeat"]
required = false

[[format]]
name = "route"
command = "aproxy-format"
args = ["route", "--config", "route.toml"]
process = "persistent"
stages = ["request", "retry"]

[[format]]
name = "convert"
command = "aproxy-format"
args = ["convert", "--config", "convert.toml"]
process = "persistent"
stages = ["request", "retry", "stream"]
required = true

[[format]]
name = "rewrite"
command = "aproxy-format"
args = ["rewrite", "--config", "rewrite.toml"]
process = "persistent"
stages = ["request", "retry"]

[[format]]
name = "keys"
command = "aproxy-format"
args = ["keys", "--config", "keys.toml"]
process = "persistent"
stages = ["request", "retry"]
pool_max = 1
worker_idle_secs = 0
```

## Known conversion limits

Two limits come from the conversion library and cannot be worked around in a chain. Tell the user
about them when they apply instead of promising a fix in configuration:

- **Chat upstream, Messages or Responses client, streaming tool calls:** the tool call's
  arguments arrive at the client in one piece when the call finishes, not incrementally. Nothing
  is lost; a client simply shows no progress during a long tool call.
- **Messages upstream with thinking enabled, Chat client, tool use:** Chat has no field for the
  signature that Messages attaches to a thinking block. When the client continues the
  conversation with a tool result, the thinking block is dropped from the converted request, and
  an upstream that requires it may reject that continuation. Prefer a Messages-speaking client
  for such an upstream, or turn thinking off.

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
