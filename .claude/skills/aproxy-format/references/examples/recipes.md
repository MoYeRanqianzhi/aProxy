# The official aproxy-format binary: roles, chains and tools: recipes

Part of [examples.md](../examples.md), which lists the other parts. Routing then converting per destination, a full chain, and the known conversion limits.

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
