---
name: aproxy-cli
description: Operate and configure aProxy, the local API proxy that retries failed LLM API requests indefinitely so agent workflows keep running. Covers every aproxy command (start, status, stop, restart, logs, restore, alias, find, doctor, config, install/upgrade), the config.toml and settings.json fields, runtime behavior (retries, keepalive heartbeats, multiple instances, logs, watchdog), connecting agent clients such as Claude Code and Codex, and troubleshooting. Use this skill whenever the user mentions aproxy or wants an agent client routed through a local retrying proxy, even if they do not ask for documentation - for example "point Claude Code at aproxy", "add a second instance on port 12346", "why is the port still in use after stop", "/compact hangs behind my relay API", "aproxy logs are garbled". Configuring request_transform / response_transform on the aProxy side is covered here; writing the format program itself belongs to the aproxy-format skill.
---

# aProxy

aProxy sits between an agent client and its LLM API. The client sends requests to
`http://127.0.0.1:PORT`; aProxy forwards the path, query and headers unchanged to the upstream
`base_url` from its config, and retries every failure until the upstream answers with a usable
response. The client sees a slow answer instead of an error, so long agent runs survive overloaded
or flaky upstreams.

## How it works, in enough detail to reason about it

- **It buffers, then replays.** aProxy reads the whole upstream response before sending anything
  back. That is what lets it retry failures the client would otherwise see half-way: network
  errors, 4xx, 5xx, `200` with an error JSON body, streams that break mid-way. Retries wait
  0, 0, 0, 5, 10, 20 ... seconds, capped at `max_retry_backoff_secs` (default 320), forever.
- **Heartbeats keep streaming requests alive.** While it retries a streaming request, aProxy sends
  the client response headers early and then an SSE comment (`: keepalive`) every 15 s. That
  defeats byte-level timeouts, but SSE parsers drop comments, so a client that times out on
  *events* (Claude Code, Codex and others) still gives up unless its stream idle timeout is raised.
  This is the most common integration mistake; see "Connect an agent client" below.
- **Two deliberate exceptions to infinite retry:** `forward_only = true` streams straight through
  with no buffering and no retries, and paths matched by `bounded_retry_paths` give up after 3
  attempts and pass the real upstream response through (the fix when an upstream never supports
  an endpoint, such as `count_tokens` behind some relay APIs).
- **Each instance is one config.toml with its own `listen_addr`.** Instances run in the background
  and are controlled over a local IPC channel (named pipe / Unix socket), never over the proxy
  port, so `status`, `stop` and `logs` work even when the proxy port is busy.
- **Browser-originated requests are refused.** Requests carrying an `Origin` header get a local
  403, and the `Host` header is checked, so a web page cannot spend the user's API key through the
  proxy. CLI agents are unaffected; Electron or web clients need `allowed_origins`.

## Before you touch a running instance

The agent reading this may itself be talking to its model through aProxy. Check where your own
client points (for example `ANTHROPIC_BASE_URL`, or `base_url` in `~/.codex/config.toml`). If it is
`127.0.0.1:PORT` of an instance you are about to stop or restart, that request is your own
lifeline: `stop` and `restart` give in-flight requests 10 seconds and then drop them, and a stopped
instance takes every other session that uses it down too.

- Run `aproxy status` first and act on a specific port or alias. Use `stop all` only when the
  user asked for exactly that.
- Never kill aProxy processes by name (`taskkill /IM aproxy.exe`, `pkill aproxy`): that takes down
  every instance and its watchdog at once, including the one you may be using. If an instance
  will not stop, use `aproxy stop PORT --force`, which verifies the process identity first.
- Ask before stopping or reconfiguring an instance the user did not mention.
- To experiment, run a separate instance under a temporary `APROXY_HOME` on a port no other
  instance uses, address it only by that port, and stop it when you are done. `APROXY_HOME` gives
  the experiment its own configs, settings, logs and registry, but on Windows the control pipe is
  named after the port for the whole machine: `stop 12345` from a test home still reaches the
  user's instance on 12345. Details: commands.md, "Experiment in an isolated home".

## Common tasks

### Connect an agent client

1. Make sure an instance is running and note its port: `aproxy status`. Use the instance the user
   names. One instance has one upstream, so clients with different protocols share it only if that
   upstream serves both; if unsure, say so and offer a second instance rather than switching.
2. Point the client's API base URL at `http://127.0.0.1:PORT` (with `/v1` appended where the client
   expects it, as Codex does).
3. Raise the client's event-level stream idle timeout. Without this, any retry period or long
   generation beyond the client's default (Claude Code 600 s, Codex 300 s) makes the client
   disconnect and resend, which restarts the work from scratch.

| Client | Settings |
|---|---|
| Claude Code | `ANTHROPIC_BASE_URL=http://127.0.0.1:PORT` and `CLAUDE_STREAM_IDLE_TIMEOUT_MS=86400000` (shell environment or the `env` block of `~/.claude/settings.json`) |
| Codex | a provider in `~/.codex/config.toml` with `base_url = "http://127.0.0.1:PORT/v1"`, `wire_api = "responses"` and `stream_idle_timeout_ms = 86400000` |

Every other client, the reasoning behind the timeouts, and how to verify the setup are in
[clients.md](references/latest/clients.md).

### Start, inspect and stop instances

- `aproxy` or `aproxy start [ALIAS|PATH]` starts an instance in the background; it keeps running
  after the terminal closes. Add `--foreground` to watch it in the console while debugging.
- `aproxy status` lists running instances. When more than one runs, `stop`, `restart` and `logs`
  need a port or alias, because guessing the target would hit the wrong instance.
- A target is resolved as: alias, then the reserved word `default` (the default config), then a
  config file path. A bare number is always a port.
- `aproxy restart PORT|ALIAS` applies config changes: it stops the instance and starts it again
  with its original arguments. It does not start an instance that is not running; use `start`.

### Change configuration

- `config.toml` is the per-instance file people edit; several can coexist, one per instance.
  `settings.json` is aProxy's own state (aliases, the default config, global defaults); change it
  through `aproxy alias ...` and `aproxy config ...` rather than by hand: if the file stops
  parsing, aProxy prints a warning and runs on built-in defaults, losing every alias.
- Precedence, highest first: CLI flags (this run only, never saved), config.toml, the global
  defaults in settings.json (only a few fields have one), built-in defaults.
- Apply an edit with `aproxy restart PORT|ALIAS`. It parses and validates the edited config, and
  checks that a changed listen address can be bound, before it stops anything, so a bad edit is
  reported while the old instance keeps serving. `aproxy doctor` is the wider check: aliased
  configs and the files in the config directories, including port conflicts between them.

### Investigate a problem

- `aproxy logs PORT|ALIAS` follows an instance's log and starts by printing the log file's path.
  It never returns on its own, so from a non-interactive tool call run it under a timeout, take the
  path, and read the file. Log files get a random name at each start; never guess the name from
  the port. Details: commands.md, logs.
- Log lines are in Chinese. [troubleshooting.md](references/latest/troubleshooting.md) maps
  symptoms and the exact log strings to causes and fixes.
- A bind failure can mean "port taken by another program" or "port reserved by the system"
  (Hyper-V/WinNAT on Windows); the fixes differ, so read the error before choosing one.

### Install or upgrade

`aproxy install` (alias `aproxy upgrade`) installs the newest version into `~/.aproxy/bin/` and
restarts running instances one at a time, so clients barely notice. Options, channels and
recovery from an interrupted install are in [commands.md](references/latest/commands.md).

## Where to look

| You need | Read |
|---|---|
| A command's exact syntax, flags, exit codes | [commands.md](references/latest/commands.md) |
| A config.toml field: type, default, what 0 or empty means | [config-toml.md](references/latest/config-toml.md) |
| Aliases, the default config, global defaults in settings.json | [settings-json.md](references/latest/settings-json.md) |
| Setting up Claude Code, Codex, Gemini CLI or another client | [clients.md](references/latest/clients.md) |
| How retries, heartbeats, streaming, disk cache, forward-only mode, transformers, IPC, logs, watchdog and install work | [behaviors.md](references/latest/behaviors.md) |
| A symptom or log message to explain | [troubleshooting.md](references/latest/troubleshooting.md) |
| Whether these docs match the installed version | [compatibility.md](references/latest/compatibility.md) |

These references describe aProxy 0.1.x and the current development line. Check
`aproxy --version` (and the versions `aproxy status` reports for running instances) before relying
on a detail; compatibility.md explains what to do on a mismatch.
