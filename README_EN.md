# aProxy

> "The impediment to action advances action. What stands in the way becomes the way."
> — Marcus Aurelius, *Meditations*

**A local API proxy that holds the line for every agent request.**

Upstream rate limits, broken streams, timeouts, once-in-a-blue-moon glitches —
aProxy catches the request locally: on failure it **retries without limit**
(exponential backoff, capped and configurable), keeps streaming responses
alive with injected SSE heartbeats, and replays the successful response
byte-for-byte. Your client never notices the storm upstream; it only notices
that the request took a little longer. (Two explicit exceptions: `forward_only`
mode gives up that retry guarantee for true streaming passthrough, and
`bounded_retry_paths` passes through the real response after 3 failures for
matched paths; see Configuration.)

[中文](README.md)

## What it does

- **Infinite retries** — What stands in the way becomes the way. Network
  errors, 4xx/5xx, error JSON (a 200 carrying an error) all trigger retries;
  when the client disconnects, the upstream request is aborted immediately
  (billing protection). Two **explicit ways out**: `forward_only` mode (an
  explicit trade that drops retries for streamed request/response passthrough)
  and `bounded_retry_paths` (for upstream endpoints that fail deterministically,
  the real response is passed through after 3 attempts instead of waiting
  forever).
- **Total passthrough** — Transparency as a principle. Paths, queries, and
  headers forwarded untouched; control traffic rides a separate named pipe,
  so the proxy port does exactly one thing.
- **External transformers (optional)** — Requests and responses can be
  handed whole to an external format program that rewrites them (one JSON
  envelope line in, one out; any program that reads stdin and writes stdout
  works): OpenAI ↔ Anthropic protocol conversion, multi-key rotation,
  multi-model / multi-channel aggregation. The core only adds orchestration;
  all conversion logic lives outside it, and the official example binary
  `aproxy-format` works out of the box (released separately). When the
  persistent pool reuses an idle worker that has died, it transparently
  retries once with a fresh worker. **Limitation**: `aproxy-format` converts
  across protocols (e.g. Anthropic ↔ OpenAI) for **non-streaming** requests
  only; cross-protocol SSE responses are not supported — the response-side
  conversion fails and the original upstream response is passed through, so
  the client receives a stream in the channel's protocol. Same-protocol SSE
  passes through untouched, and key rotation / same-protocol aggregation are
  unaffected.
- **Background daemon** — `aproxy` starts detached (survives terminal close);
  `status` / `stop` / `logs` / `restore` for full instance management.
- **Watchdog** — Nothing slips through. A global supervisor process
  automatically revives crashed or hung instances (on by default; measured at
  +2.4% binary size, +2.9MB resident, zero hot-path overhead). Process identity
  is verified by PID + creation time, independent of the binary's file name.
  Not supported on macOS yet; see Platform support.
- **Multi-instance** — Every config.toml is its own instance, each on its own
  port, coexisting without interference.
- **Self-healing** — After a crash, power loss, or reboot, `aproxy restore`
  brings everything back in one command. Nothing to restore? It exits
  silently — safe for login autostart.

## Getting started

**Hand it to your agent** — send this line to your agent (Claude Code etc.)
and it will read the instructions and take care of installation:

```text
Read https://raw.githubusercontent.com/MoYeRanqianzhi/aProxy/main/docs/INSTALL_AGENT.md and install (or upgrade) aProxy on this machine exactly as it says.
```

**Manual install** (first install pulls the binary + skill docs, verifies
SHA256, places everything under `~/.aproxy/`). By default it installs the
latest **stable** release; while the repository has no stable release yet
(before 0.1.0) it falls back to the highest-versioned pre-release and says so.
Add `--pre` to let pre-releases compete too and take the highest version
(`-Pre` for PowerShell, or set `APROXY_PRE=1`). Only tags in the form `vX.Y.Z`
or `vX.Y.Z-(alpha|beta|rc).N` are considered:

```powershell
# Windows (PowerShell)
irm https://raw.githubusercontent.com/MoYeRanqianzhi/aProxy/main/scripts/install.ps1 | iex
```

```sh
# Linux / macOS / Git Bash (pre-release: curl ... | sh -s -- --pre)
curl -fsSL https://raw.githubusercontent.com/MoYeRanqianzhi/aProxy/main/scripts/install.sh | sh
```

```bat
rem Windows (cmd fallback; downloads are delegated to the built-in Windows PowerShell, which must be available)
curl -fsSL https://raw.githubusercontent.com/MoYeRanqianzhi/aProxy/main/scripts/install.cmd -o install.cmd && install.cmd
```

About the scripts:
- They never overwrite an existing install; upgrades go through `aproxy
  install`. The sh script needs one of `sha256sum`, `shasum` or `openssl` for
  the SHA256 check.
- Linux: systems with an old glibc, or musl systems, automatically get the
  statically linked musl build; the binary is self-tested with `--version`
  before being placed, and if the gnu build cannot run on the machine the
  script switches to musl.
- `irm | iex` cannot pass arguments, so the PowerShell script reads
  environment variables instead: `APROXY_PRE`, `APROXY_NO_SKILLS`,
  `APROXY_DL_PROXY` (download proxy; unrelated to the upstream request proxy).
- Windows PowerShell 5.1 fails to parse `install.ps1` when it is run locally
  with `-File` (the file is UTF-8 without BOM); use the `irm | iex` line
  above, or `pwsh -File`.

Other channels (afterwards `aproxy install --adopt` moves the install to the
standard location):

```sh
npm install -g @meowo/aproxy     # picks the right binary for your platform
cargo install aproxy             # builds from source; needs rustc 1.88+
```

Upgrades: `aproxy install` replaces the binary and rolling-restarts running
instances one by one without client-visible downtime (`--from <path>` installs
a local binary, `--adopt` takes over an existing install). Manual fallback:
`aproxy stop all` → replace the binary → `aproxy restore`.

From source:

```powershell
git clone https://github.com/MoYeRanqianzhi/aProxy.git
cd aProxy
cargo build --release
```

## Platform support

| Platform | Status |
|---|---|
| Windows (x64 / x86 / arm64) | Full support |
| Linux (x86_64 / aarch64, glibc and musl) | Full support |
| macOS (Apple Silicon / Intel) | Pre-built binaries are published but untested on real hardware; the watchdog, `aproxy install` and process-query-based instance management rely on Linux-only interfaces (`/dev/shm`, `/proc`) and are unavailable or degraded on macOS (needs real hardware; contributions welcome) |

## Quick start

A fresh machine has no upstream configured, so running `aproxy` right away
fails because `base_url` is empty — configure first, then start:

```sh
# 1. Configure the upstream (writes ~/.aproxy/config.toml)
aproxy config --baseurl https://api.anthropic.com --api-key sk-ant-...

# 2. Start (background daemon)
aproxy

# 3. Point your agent's API base URL at the local proxy
#    https://api.anthropic.com  →  http://127.0.0.1:12345
```

## Commands

| Command | Description |
|---|---|
| `aproxy [start]` | Start the proxy in the background (default port 127.0.0.1:12345) |
| `aproxy start <alias\|path>` | Start by alias or config file path |
| `aproxy --foreground` | Run in the foreground (logs to console, Ctrl+C to stop) |
| `aproxy status` | List running instances (port/pid/version/upstream/config) |
| `aproxy stop [PORT\|all\|alias]` | Stop instances; multi-instance requires a port, `all`, or an alias; `--force` kills instantly (the watchdog will not revive it) |
| `aproxy restart [PORT\|all\|alias]` | Restart running instances (restart-only, never starts); the new config is pre-checked before the old instance is stopped, and a failed check leaves it running; `--force` kills instantly |
| `aproxy logs [PORT\|ALIAS]` | Tail an instance's live logs; `all` not supported |
| `aproxy restore` | Revive instances that were running before a crash/reboot; exits silently if none |
| `aproxy alias add\|remove\|list` | Manage config aliases (stored in settings.json) |
| `aproxy doctor` | Config health check: settings.json errors + alias/toml review (warnings) |
| `aproxy find [keyword]` | Discover all configs in configured directories; `--aliased/--unaliased --port PORT` filters |
| `aproxy config [options]` | View or modify the config (`--show` prints it) |

Global flags: `--config <PATH>` (multi-instance), `--listen <ADDR>`,
`--proxy <URL>`, `--api-key <KEY>` (current launch only).

## Multi-instance & aliases

```sh
aproxy --config ~/.aproxy/work.toml      # listen_addr = "127.0.0.1:12345"
aproxy --config ~/.aproxy/personal.toml  # listen_addr = "127.0.0.1:12346"
aproxy status                            # both visible
aproxy stop 12346                        # manage by port
```

```sh
aproxy alias add openrouter ~/.aproxy/openrouter.toml
aproxy alias add anthropic               # no path = default ~/.aproxy/config.toml
aproxy start openrouter                  # start by alias
aproxy stop openrouter                   # stop by alias (survives port changes)
aproxy alias list
```

Aliases live in `~/.aproxy/settings.json` (program-managed; use `aproxy
alias`). config.toml stays human-readable and may exist in many parallel
copies.

## Login autostart (optional)

Point a "run at logon" scheduled task at `aproxy.exe` with the argument
`restore`. Instances that were running come back; if none were, it exits
silently.

## Configuration

`~/.aproxy/config.toml` (one file per instance):

```toml
base_url = "https://api.anthropic.com"   # upstream address
listen_addr = "127.0.0.1:12345"          # local listener
# api_key = "sk-..."                     # quick auth (overrides Authorization: Bearer)
# keepalive_interval_secs = 15           # SSE heartbeat during retries, 0 disables
# proxy = "http://127.0.0.1:7890"        # forward via proxy (socks5 supported)
# extra_headers / override_headers       # append/override request headers
# max_retry_backoff_secs = 320           # retry backoff cap (0 = retry instantly)
# spool_limit_mb = 256                   # upstream response buffer cap (MB)
# max_body_mb = 128                      # request body cap (MB; 0 = unlimited)
# disk_cache = true                      # spool large bodies/responses to disk
# forward_only = false                   # forward-only (gives up retries): stream both ways, no retries/buffering/heartbeats
# bounded_retry_paths = [ '/v1/x' ]      # bounded retry paths (regex): matched requests pass through after 3 failures
# log_file = "D:/aproxy-logs/a.log"      # custom log file (~ expanded; relative paths resolve against APROXY_HOME; default is a random per-start name, resolved via IPC)
# connect_timeout_secs = 30              # upstream connect timeout (0 = none)
# read_timeout_secs = 300                # inter-read timeout (0 = none)
# allowed_hosts = ["myproxy.local"]      # extra Hosts to allow (DNS-rebinding guard; appended to the built-in list, "*" disables the check)
# allowed_origins = ["http://localhost:5173"] # browser Origins to allow (any request carrying Origin is rejected by default, "*" disables the check)
# request_transform = { command = "~/.aproxy/bin/aproxy-format", args = ["run"], mode = "persistent", extra = "~/.aproxy/agg.toml" }
                                         # external transformer (request side): hand body/headers/url to a format program
                                         # (protocol conversion, key rotation, multi-channel aggregation; failure = 502, no retry; exclusive with forward_only)
# response_transform = { command = "~/.aproxy/bin/aproxy-format", args = ["run"], mode = "persistent", extra = "~/.aproxy/agg.toml" }
                                         # external transformer (response side; use the same extra as the request side): rewrite upstream responses before replay (failure passes through)
                                         # use ~/ or absolute paths for command/extra: the daemon's working directory is unreliable, so relative paths may not resolve
```

Multi-key rotation only takes effect **between requests**: retries of the same
request keep the same transformed key (a request is transformed once and the
result is replayed on every retry).

Runtime data lives in `~/.aproxy/`: `run/` (registry, restore records,
watchdog claim), `logs/` (daemon logs, randomly named per start, rotated and
orphan-cleaned; paths reported via IPC — customize with `log_file`),
`spool/<port>/` (disk-cache
scratch space), `settings.json` (internal state: aliases, defaults, watchdog
fields — program-managed).

### Inbound origin checks

The proxy serves local CLI-style clients by default and defends against
web-page origins (see [SECURITY.md](SECURITY.md)):

- **Host** (DNS-rebinding guard): when listening on a loopback address, only
  `localhost` / `127.0.0.1` / `[::1]` and the listen address's own host name
  are accepted; `allowed_hosts` entries are **added** to that set (port
  ignored, case-insensitive). When listening on a non-loopback address with an
  empty `allowed_hosts`, no Host check is done.
- **Origin**: any request carrying an `Origin` header is rejected by default
  (only browsers and Electron/WebView-style clients send it) unless it exactly
  matches an `allowed_origins` entry (case-insensitive, trailing `/` ignored);
  independent of the listen address.
- Either list may contain `"*"` to disable that check; `[]` is the same as
  unset. Both can be set per instance in `config.toml` and as global defaults
  in `settings.json` (toml wins).
- A rejected request gets a local **403** whose message names the setting to
  change. It was never forwarded, no `api_key` was injected and no retry
  happens; requests that pass are unaffected.
- Affected clients: apps that send `Origin` (Cherry Studio, Open WebUI, ...)
  need their origin added to `allowed_origins`. CLI clients such as Claude Code
  send no `Origin` and use `127.0.0.1:<port>` as Host, so they are unaffected.

## Security notes

aProxy is a local single-user proxy: it holds the upstream key and injects it
into every forwarded request, and the proxy port itself has no authentication.

- It listens on loopback (`127.0.0.1`) by default. **Do not set `listen_addr`
  to `0.0.0.0` or a LAN address** — that lets any host on the network use your
  key without authentication; if you do need it, start-up, `startup.log` and
  `aproxy doctor` will warn you.
- `api_key` is stored in plain text in `config.toml`; restrict the file's
  permissions to your own user.
- External transformers (`request_transform` / `response_transform`) run
  arbitrary commands from the config; only configure programs you trust and do
  not let others edit your `config.toml`.
- Logs and `status` mask URL-embedded credentials and query-string values
  (`?key=***`), so they are safe to paste.

The full threat model and how to report vulnerabilities are in
[SECURITY.md](SECURITY.md).

## Development

```sh
cargo test --workspace --locked                                # full suite (incl. aproxy-envelope / aproxy-format)
cargo clippy --workspace --all-targets --locked -- -D warnings # must be warning-free (project rule)
cargo fmt --all -- --check
```

Architecture docs in [`docs/`](docs/); agent-facing development docs in
`.agents/docs/`.

## License

MIT
