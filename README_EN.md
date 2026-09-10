# aProxy

> "The impediment to action advances action. What stands in the way becomes the way."
> — Marcus Aurelius, *Meditations*

**A local API proxy that holds the line for every agent request.**

Upstream rate limits, broken streams, timeouts, once-in-a-blue-moon glitches —
aProxy catches the request locally: on failure it **retries without limit**
(exponential backoff, capped and configurable), keeps streaming responses
alive with injected SSE heartbeats, and replays the successful response
byte-for-byte. Your client never notices the storm upstream; it only notices
that the request took a little longer.

[中文](README.md)

## What it does

- **Infinite retries** — What stands in the way becomes the way. Network
  errors, 4xx/5xx, error JSON (a 200 carrying an error) all trigger retries;
  when the client disconnects, the upstream request is aborted immediately
  (billing protection).
- **Total passthrough** — Transparency as a principle. Paths, queries, and
  headers forwarded untouched; control traffic rides a separate named pipe,
  so the proxy port does exactly one thing.
- **Background daemon** — `aproxy` starts detached (survives terminal close);
  `status` / `stop` / `logs` / `restore` for full instance management.
- **Watchdog** — Nothing slips through. A global supervisor process
  automatically revives crashed or hung instances (on by default; measured at
  +2.4% binary size, +2.9MB resident, zero hot-path overhead).
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
SHA256, places everything under `~/.aproxy/`):

```sh
# Linux / macOS / Git Bash
curl -fsSL https://raw.githubusercontent.com/MoYeRanqianzhi/aProxy/main/scripts/install.sh | sh
```

```powershell
# Windows (PowerShell)
irm https://raw.githubusercontent.com/MoYeRanqianzhi/aProxy/main/scripts/install.ps1 | iex
```

Upgrades: `aproxy install` rolling-restarts running instances after a binary
swap (shipping in a later release); on current alphas do it manually —
`aproxy stop all`, replace the binary, `aproxy restore`.

From source:

```powershell
git clone https://github.com/MoYeRanqianzhi/aProxy.git
cd aProxy
cargo build --release
```

## Quick start

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
| `aproxy stop [PORT\|all\|alias]` | Stop instances; multi-instance requires a port, `all`, or an alias |
| `aproxy restart [PORT\|all\|alias]` | Restart running instances (restart-only, never starts); `--force` kills instantly |
| `aproxy logs [PORT]` | Tail an instance's live logs; `all` not supported |
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
# connect_timeout_secs = 30              # upstream connect timeout (0 = none)
# read_timeout_secs = 300                # inter-read timeout (0 = none)
```

Runtime data lives in `~/.aproxy/`: `run/` (registry, restore records,
watchdog claim), `logs/` (daemon logs, rotated), `spool/<port>/` (disk-cache
scratch space), `settings.json` (internal state: aliases, defaults, watchdog
fields — program-managed).

## Development

```sh
cargo test --locked                                # full suite (115 lib + 4 bin + 38 integration)
cargo clippy --all-targets --locked -- -D warnings # must be warning-free (project rule)
cargo fmt --all -- --check
```

Architecture docs in [`docs/`](docs/); agent-facing development docs in
`.agents/docs/`.

## License

MIT
