# The official aproxy-format binary: roles, chains and tools: role reference

Part of [examples.md](../examples.md), which lists the other parts. Every official role and its options.

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
