# The official aproxy-format binary: roles, chains and tools: install and starter scenarios

Part of [examples.md](../examples.md), which lists the other parts. Installing `aproxy-format`, wiring a role into config.toml, and the `init` starter scenarios.

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
