# aProxy

> "The impediment to action advances action. What stands in the way becomes the way."
> — Marcus Aurelius, *Meditations*

**A local API proxy that holds the line for every agent request.**

Upstream rate limits, broken streams, timeouts, once-in-a-blue-moon glitches —
aProxy catches the request locally: on failure it **retries without limit**
(exponential backoff, capped and configurable), keeps streaming requests
(`Accept: text/event-stream` or a body with `"stream": true`) alive with
injected SSE heartbeats, and replays the successful response
byte-for-byte. Your client never notices the storm upstream; it only notices
that the request took a little longer. (Two explicit exceptions: `delivery =
"forward"` gives up that retry guarantee for true streaming passthrough, and a
`[[retry.rules]]` entry passes through the real response once its `max_attempts`
is reached, except that a streaming request whose headers were already committed
by keepalive ends with a terminal SSE error event; see Configuration.)

[中文](README.md)

## What it does

- **Infinite retries** — What stands in the way becomes the way. Network
  errors, 4xx/5xx, error JSON (a 200 carrying an error) all trigger retries;
  when the client disconnects, the upstream request is aborted immediately
  (billing protection). Two **explicit ways out**: `delivery = "forward"` (an
  explicit trade that drops retries for streamed request/response passthrough)
  and `[[retry.rules]]` (for upstream endpoints that fail deterministically, the
  real response is passed through once `max_attempts` is reached instead of
  waiting forever; only a streaming request whose headers keepalive already
  committed is told via a terminal SSE error event instead).
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
- **Background daemon** — `aproxy start` launches detached from a launch spec (survives terminal close;
  secrets stay in the spec file, not on the instance's command line); `status` / `stop` / `logs` /
  `restore` for full instance management.
- **Watchdog** — Nothing slips through. A global supervisor process
  automatically revives crashed or hung instances (on by default; measured at
  +2.4% binary size, +2.9MB resident, zero hot-path overhead). Process identity
  is verified by PID + creation time, independent of the binary's file name.
  After `watchdog.max_restarts` failed respawns it does not give up: it retries
  every 300 s until the instance starts or `aproxy stop` deletes its record,
  and a respawned instance must stay up 60 s before earlier failures are
  forgotten. It re-reads settings every scan, so disabling it stops a running
  watchdog. Not supported on macOS yet; see Platform support.
- **Multi-instance** — Every config.toml is its own instance, each on its own
  port, coexisting without interference.
- **Self-healing** — After a crash, power loss, or reboot, `aproxy restore`
  brings everything back in one command. Nothing to restore? It exits
  silently — safe for login autostart. An instance's desired-state record (its
  launch spec: config file, override flags, pinned port, launch-time proxy
  environment and PATH, log path) is deleted only by `stop`; a failed restart
  leaves it in place, so `restore` or the watchdog can still bring the instance
  back, and a temporarily missing config file does not delete it either.

## Getting started

**Hand it to your agent** — send this line to your agent (Claude Code etc.)
and it will read the instructions and take care of installation:

```text
Read https://raw.githubusercontent.com/MoYeRanqianzhi/aProxy/main/docs/INSTALL_AGENT.md and install (or upgrade) aProxy on this machine exactly as it says.
```

**Manual install** (first install pulls the binary + skill docs, verifies
SHA256, places everything under `~/.aproxy/`). By default it installs the
latest **stable** release; if the repository has no stable release at all it
falls back to the highest-versioned pre-release and says so.
Add `--pre` to let pre-releases compete too and take the highest version
(`-Pre` for PowerShell, or set `APROXY_PRE=1`). Only tags in the form `vX.Y.Z`
or `vX.Y.Z-(alpha|beta|rc).N` are considered.

> **0.2.0 is currently available as a pre-release only.** Unless you ask for
> it, every channel below installs the stable 0.1.0, which has none of the
> desktop window, the two variants or `install --force` described here. To
> install 0.2.0: add `--pre` to the install script, use `@next` with npm, and
> name the version with cargo. 0.1.x cannot be upgraded to 0.2.0 in place; see
> "Upgrading from 0.1.x" below.

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
- **Two variants**: every platform has a `desktop` build (with the window of
  `aproxy desktop`) and a `headless` build without it; the proxy and the
  commands are the same. musl comes only as headless. Windows and macOS get
  desktop. On Linux the desktop build links GTK and WebKitGTK and cannot even
  run `--version` without them, so the sh script checks the machine first:
  musl, glibc below 2.34, or no WebKitGTK 4.1 (`libwebkit2gtk-4.1-0` on
  Debian/Ubuntu) gets headless with the reason printed, and so does a desktop
  build that fails its self-test after the download (WebKitGTK older than
  2.40, for example). To choose, pass `--variant desktop|headless` to the sh
  script (or set `APROXY_VARIANT`); once installed, switch with
  `aproxy install --variant desktop` (or `headless`).
- `irm | iex` cannot pass arguments, so the PowerShell script reads
  environment variables instead: `APROXY_PRE`, `APROXY_NO_SKILLS`,
  `APROXY_DL_PROXY` (download proxy; unrelated to the upstream request proxy).
- Windows PowerShell 5.1 fails to parse `install.ps1` when it is run locally
  with `-File` (the file is UTF-8 without BOM); use the `irm | iex` line
  above, or `pwsh -File`.

Other channels (afterwards `aproxy install --adopt` moves the install to the
standard location):

```sh
npm install -g @meowo/aproxy@next            # with the desktop interface; picks the right binary
npm install -g @meowo/aproxy-headless@next   # without it (servers, Linux without WebKitGTK); both provide the aproxy command, install one
cargo install aproxy --locked --version 0.2.0-alpha.2   # builds from source; needs rustc 1.90+
```

Pre-releases go to npm's `next` tag and the `latest` tag is kept for stable releases; cargo
without a version also takes only stable releases. 0.2.0 is a pre-release for now, so the
commands above carry the tag or the version (the `latest` tag of `@meowo/aproxy-headless` is only a
placeholder and must not be used). Once 0.2.0 is released as a stable version, drop `@next` and
`--version`.

`cargo install` builds the version with the desktop interface by default; on Linux, install the
WebKitGTK 4.1 development package first (Debian/Ubuntu: `libwebkit2gtk-4.1-dev`). On Linux
servers use `cargo install aproxy --locked --no-default-features` (with `--version` as well) for
the headless build.
`--locked` builds with the dependency versions in the package's Cargo.lock, the ones tested
before the release; without it cargo picks the newest compatible version of each dependency.
`cargo binstall aproxy` fetches the prebuilt desktop build (headless on musl systems); for the
headless build, give the asset address:

```sh
cargo binstall aproxy --pkg-url '{ repo }/releases/download/v{ version }/{ name }-headless-{ target }{ binary-ext }' --pkg-fmt bin
```

Manual downloads: `aproxy-<target>[-v3]` is the desktop build, `aproxy-headless-<target>[-v3]`
the headless one; check them against their `.sha256` or the release manifest
`aproxy-manifest.json`. When the npm desktop build cannot start on Linux for missing libraries,
its launcher suggests `@meowo/aproxy-headless` instead.

Upgrades: `aproxy install` replaces the binary and rolling-restarts running
instances one by one without client-visible downtime (`--from <path>` installs
a local binary, `--adopt` takes over an existing install).
- **Update channel**: the default `latest` picks the highest version inside a
  channel. If the current version is a stable release only stable releases are
  considered (you are never carried onto a pre-release); if it is a
  pre-release (e.g. an alpha) the default is already the pre-release channel;
  `--pre` lets pre-releases compete explicitly. Only `vX.Y.Z` /
  `vX.Y.Z-(alpha|beta|rc).N` are considered, compared by version number rather
  than creation time; `format-v*` and historical test tags are ignored. When
  the channel has nothing newer you get "already up to date" or "no version
  available" and nothing changes — it never switches channels silently. An
  explicit version (`aproxy install 0.2.0`) ignores the channel; a target below
  the current version is refused unless `--allow-downgrade` is given, and
  nothing below 0.2.0 is installed at all.
- **Upgrading from 0.1.x**: 0.1.x and 0.2.0 or later cannot control each
  other's instances, so a 0.1.x `aproxy install` cannot install 0.2.0 (it exits
  with `--version 输出无版本号` after the download and changes nothing). Stop the
  instances and reinstall, following
  [Moving from 0.1.x to 0.2.0](.claude/skills/aproxy-cli/references/latest/compatibility.md#moving-from-01x-to-020)
  (the Chinese version is "从 0.1.x 迁移" in the [CHANGELOG](CHANGELOG.md)). While
  0.2.0 is a pre-release, add `--pre` to the install script when reinstalling,
  or it installs 0.1.x again.
- **Download channels and proxy**: online installs try github → npm →
  cargo-binstall → cargo in that order; `update.download_chain` in settings.json
  reorders them or adds a mirror (URL template). Use `--download-proxy <URL>`
  or `update.download_proxy` in settings.json to download through a proxy; it is
  unrelated to the upstream request proxy. The fields are described in the skill
  document
  [settings-json.md](.claude/skills/aproxy-cli/references/latest/settings-json.md).
  Force install reads no settings.json and uses only the command line's
  `--download-proxy` (the system's proxy environment otherwise).
- **Variants**: upgrades keep the installed variant (the first word in the
  parentheses of `aproxy --version`); `aproxy install --variant desktop|headless`
  switches, reinstalling the current version in the other variant when
  nothing newer is available. The installer does not guess what the machine
  lacks; it trusts only the downloaded binary's `--version` self-test. Without
  `--variant`, a desktop build that cannot run (on Linux usually for lack of
  WebKitGTK) is replaced by the headless build of the same target, with the
  reason printed, and on Linux by the musl build after that; with
  `--variant desktop` the install stops with an error and changes nothing.
  `install --force` behaves the same.
- **Rollback on failure**: if an instance cannot start under the new version
  during the rolling restart, `install` brings it back with the old binary from
  the same launch spec, stops the rollout (the other instances are left
  alone) and exits non-zero; fix the cause and run `aproxy install` again.
- **Force install**: when a normal install fails or hangs, or the installed
  version is too broken to run, use `aproxy install --force [VERSION|latest]`
  (or `--from <downloaded binary>`). It skips the hand-over: it stops every
  aproxy process of this home that belongs to the current user, through the
  process ledger, swaps in the new
  binary, and brings the instances back with the new version's `restore` from
  their desired-state records. That cuts every request in flight through aproxy at
  once, so it asks for confirmation (add `--yes` when not interactive). If the
  installed version does not run at all, download the new version yourself and
  run `<new binary> install --force --from <new binary>`. Exit codes: 0 done;
  10 no process stopped and the binary not replaced; 11 processes stopped but
  the binary not replaced (run it again); 12 binary replaced but some instances
  did not come back (each listed with the command that starts it). It does not
  update the skill documents; run
  `aproxy install --skills-only` afterwards.

From source:

```powershell
git clone https://github.com/MoYeRanqianzhi/aProxy.git
cd aProxy/aproxy-desktop/ui
npm ci
npm run build          # the desktop front end, written to aproxy-desktop/dist; needs Node.js 20.19+ or 22.12+
cd ../..
cargo build --release
```

The desktop front end is compiled into the binary, so the build with the interface needs it built
first; without it, compilation stops with an error that says what to do. Without the interface,
skip the front end and run `cargo build --release --no-default-features` for the headless build,
which needs no Node.js. On Linux the build with the interface also needs the WebKitGTK 4.1
development package (see above).

## Platform support

| Platform | Status |
|---|---|
| Windows (x64 / x86 / arm64) | Full support |
| Linux (x86_64 / aarch64, glibc and musl) | Full support; the headless gnu build requires glibc ≥ 2.28, the desktop gnu build glibc ≥ 2.34 and WebKitGTK ≥ 2.40 (glibc floors asserted at build time in the release pipeline); older glibc and musl systems use the statically linked musl build (headless only) |
| macOS (Apple Silicon / Intel) | Pre-built binaries are published but untested on real hardware; the watchdog, `aproxy install` and process-query-based instance management rely on a Linux-only interface (`/proc`) and are unavailable or degraded on macOS (needs real hardware; contributions welcome) |

## Quick start

A fresh machine has no upstream configured, so running `aproxy start` right away
fails because `base_url` is empty — configure first, then start:

```sh
# 1. Configure the upstream (writes ~/.aproxy/config.toml; one key per command)
aproxy config set base_url https://api.anthropic.com
aproxy config set api_key sk-ant-...
#    keep the key out of the shell history: aproxy config set api_key -

# 2. Start (background daemon)
aproxy start

# 3. Point your agent's API base URL at the local proxy
#    https://api.anthropic.com  →  http://127.0.0.1:12345
```

## Using with agent clients

So that a stream which breaks mid-way can still be retried transparently, aProxy
buffers the whole upstream response, checks it, and only then replays it; while
it waits, the client receives nothing but SSE comment heartbeats. Heartbeats keep
alive any client timeout measured **between bytes**, but not one measured
**between SSE events** (comments and `ping` are not events). If a client has the
latter, you **must** raise it, or any retry period or long generation longer than
it makes the client disconnect and resend.

### Claude Code

```powershell
# PowerShell
$env:ANTHROPIC_BASE_URL = "http://127.0.0.1:12345"
$env:CLAUDE_STREAM_IDLE_TIMEOUT_MS = "86400000"
claude
```

```sh
# sh / bash / zsh
export ANTHROPIC_BASE_URL=http://127.0.0.1:12345
export CLAUDE_STREAM_IDLE_TIMEOUT_MS=86400000
claude
```

Or put them in the `env` field of Claude Code's `~/.claude/settings.json`, which
applies to every session:

```json
{
  "env": {
    "ANTHROPIC_BASE_URL": "http://127.0.0.1:12345",
    "CLAUDE_STREAM_IDLE_TIMEOUT_MS": "86400000"
  }
}
```

Claude Code's event-level idle timeout defaults to 600 s and is controlled by
`CLAUDE_STREAM_IDLE_TIMEOUT_MS`; `86400000` (24 h) is verified to work.
`API_TIMEOUT_MS` does not control this timer. The first-byte (about 360 s) and
byte-level idle (300 s) timeouts are covered by the heartbeats. (Black-box
measurements on Claude Code 2.1.288.)

### Codex

Add a provider pointing at aProxy to `~/.codex/config.toml`, and **raise**
`stream_idle_timeout_ms`:

```toml
model = "gpt-5"                     # use a model name your upstream serves
model_provider = "aproxy"

[model_providers.aproxy]
name = "aproxy"
base_url = "http://127.0.0.1:12345/v1"
env_key = "OPENAI_API_KEY"          # Codex reads the key from this variable; any non-empty value works if aProxy sets api_key
wire_api = "responses"
stream_idle_timeout_ms = 86400000   # 24 h
```

Codex's `stream_idle_timeout_ms` defaults to 300000 (5 min) and is measured
between SSE events, so heartbeats do not reset it; on timeout Codex reconnects,
and the turn fails after 5 attempts by default. Measured: with 60 s it
disconnects exactly 60 s after the headers; with 24 h a wait of over 10 minutes
completes normally. (codex-cli 0.160.0 source and black-box test.)

### Other clients

| Client | What to do with aProxy |
|---|---|
| Qwen Code | Set `QWEN_STREAM_IDLE_TIMEOUT_MS=0` and `QWEN_STREAM_MAX_LIFETIME_MS=0` (event-level idle defaults to 240 s, plus a 15-minute stream lifetime cap; turn off both) |
| dsh (DeepSeek Harness) | Through the `llm-pi-ai` adapter (OpenAI / Anthropic compatible gateways), set `streamIdleTimeoutMs` on that provider (e.g. `172800000`); the `llm-deepseek` adapter works as is |
| Gemini CLI | Connect with `GOOGLE_GEMINI_BASE_URL` and `GEMINI_API_KEY`; it works (measured). But its streaming requests get no keepalive channel, and its response-header timeout is hard-coded at 300 s (shorter in newer versions), so a retry period plus generation longer than that fails, with no setting to change it |
| pi, OpenCode, Aider, Cline, Roo Code, Kimi CLI | Defaults are fine (byte-level timeouts only); with OpenCode 1.18+ do not set `timeout`, which caps the whole request including the wait |

Apart from Claude Code, Codex and Gemini CLI, the table comes from reading
source code and was not tested client by client. Versions, config locations and
evidence are in the skill document
[clients.md](.claude/skills/aproxy-cli/references/latest/clients.md).

Note: plain requests with `"stream": false` in the body have no keepalive
channel (there is no response stream to inject heartbeats into), so the first
byte arrives only after the whole generation; the client must raise its own
timeout (for Claude Code, `API_TIMEOUT_MS`).

## Commands

| Command | Description |
|---|---|
| `aproxy start` | Start the proxy in the background (default config, default port 127.0.0.1:12345) |
| `aproxy start <alias\|path>` | Start by alias or config file path |
| `aproxy start --foreground` | Run in the foreground (logs to console, Ctrl+C to stop) |
| `aproxy status` | List the home's instances and watchdog: running, unresponsive and waiting-to-be-restored instances and unreadable records, each in its own block; `--idle`/`--busy` filter the running ones |
| `aproxy stop [PORT\|all\|alias]` | Stop instances; multi-instance requires a port, `all`, or an alias; `--force` kills instantly (the watchdog will not revive it); a target that already crashed has its desired-state record deleted |
| `aproxy restart [PORT\|all\|alias]` | Restart running instances (restart-only, never starts); the new config is pre-checked before the old instance is stopped, and a failed check leaves it running; a failed start keeps the record, so `restore` or the watchdog can bring it back; `--force` kills instantly |
| `aproxy logs [PORT\|ALIAS]` | Print the instance's most recent 200 log lines and exit; `-n N` sets the count, `-f` keeps following; `all` not supported |
| `aproxy restore` | Revive instances from their desired-state records; a temporarily missing config file keeps the record and is reported; exits silently if none |
| `aproxy alias add\|remove\|list` | Manage config aliases (stored in settings.json) |
| `aproxy doctor` | Check settings.json and every config by the rules `start` applies, plus home and config permissions and unreadable records; exits 3 when it finds an error |
| `aproxy find [keyword]` | Discover all configs in configured directories; `--aliased/--unaliased --port PORT` filters |
| `aproxy config show\|get\|set\|unset` | View or modify the config: `show` prints the values written in the file, `--effective` adds the global and built-in defaults with each value's source; `set`/`unset` change one key per command |
| `aproxy settings show\|get\|set\|unset` | View or modify the global settings (`~/.aproxy/settings.json`): aliases, `defaults` global defaults, watchdog, logs, install updates; works even when settings.json has errors, to repair it |
| `aproxy install [VERSION]` | Install or upgrade (alias `upgrade`), rolling-restarting instances; `--variant` switches the build, `--force` forces an install; see the upgrade notes under Getting started |
| `aproxy desktop` | Open the desktop window (a preview showing demo data; see below) |

`start` options: `<alias|path>` or `--config <PATH>` (the same thing;
multi-instance), `--listen-addr <ADDR>`, `--base-url <URL>`,
`--proxy <URL>`, `--api-key <KEY|->` (recorded in this launch's spec and reused by
`restart`/`restore`; never written to config.toml; `-` reads one line from stdin),
`--log-file <PATH>`, `--foreground`. A bare `aproxy` with no command only prints the
help and exits 2.

## Desktop window (preview)

`aproxy desktop` opens the desktop window. It is a separate process that exits when its window is
closed. The window itself starts, stops and changes no instance; like every other aproxy command,
it resumes an interrupted install in the background when it finds one, and finishing that install
may restart instances one at a time.

- **In the 0.2.0 alpha releases the window is a preview**: the instances, request logs and statistics in it
  are demo data, not the aProxy running on your computer, and a "Preview · demo data" label stays
  at the top of the window. Real data comes in a later version; until then `aproxy status` and
  `aproxy logs` are the source of truth. Interface preferences (theme, language, the Mica switch)
  are not saved in this version either: a reopened window starts with the defaults.
- Windows needs the Microsoft Edge WebView2 runtime (built into Windows 11; on Windows 10, install
  it when told to). Run from a terminal, the command returns at once and the window runs on its
  own.
- Linux needs a graphical session and WebKitGTK 4.1 (2.40 or later). The build with the interface
  links WebKitGTK dynamically, so without those libraries the program does not start at all (not
  even `aproxy --version`); use the headless build on machines without a graphical interface. The
  install script and `aproxy install` pick it on their own when the libraries are missing. musl
  builds come only as headless.
- On macOS the build with the interface is only guaranteed to compile; the window has not been tried on real
  hardware (like macOS support as a whole, see Platform support).
- One window per home; running it again says it is already open.
- Headless builds (`headless` as the first word in the parentheses of `aproxy --version`) have no
  window; `aproxy desktop` there explains how to switch to the build with the interface
  (`aproxy install --variant desktop`).

## Multi-instance & aliases

```sh
aproxy start ~/.aproxy/work.toml      # listen_addr = "127.0.0.1:12345"
aproxy start ~/.aproxy/personal.toml  # listen_addr = "127.0.0.1:12346"
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

Aliases live in `~/.aproxy/settings.json` (the home's program state; use
`aproxy alias`, or read and write it directly with `aproxy settings`). config.toml
stays human-readable and may exist in many parallel copies.

## Login autostart (optional)

Point a "run at logon" scheduled task at `aproxy.exe` with the argument
`restore`. Instances that were running come back; if none were, it exits
silently.

## Configuration

`~/.aproxy/config.toml` (one file per instance):

```toml
base_url = "https://api.anthropic.com"   # upstream address
listen_addr = "127.0.0.1:12345"          # local listener (port 0 = OS-assigned; the first port is pinned, so restart/restore return to it)
# api_key = "sk-..."                     # quick auth (overrides Authorization: Bearer)
# proxy = "http://127.0.0.1:7890"        # forward via proxy (socks5 supported)
# extra_headers / override_headers       # append/override request headers (tables, e.g. extra_headers = { "x-title" = "..." })
# max_request_mb = 128                   # request body cap (MB; 0 = unlimited; defaults to settings.json defaults.max_request_mb)
# max_response_mb = 256                  # upstream response buffer cap (MB; 0 is treated as 1)
# disk_cache = true                      # spool large bodies/responses to disk
# delivery = "retry"                     # "retry" (default: buffer and retry) / "forward" (stream both ways, no retries/buffering/heartbeats)
# log_file = "D:/aproxy-logs/a.log"      # custom log file (~ expanded; relative paths resolve against APROXY_HOME; default is a random per-start name, resolved via IPC)
# allowed_hosts = ["myproxy.local"]      # extra Hosts to allow (DNS-rebinding guard; appended to the built-in list, "*" disables the check)
# allowed_origins = ["http://localhost:5173"] # browser Origins to allow (any request carrying Origin is rejected by default, "*" disables the check)

[timeouts]
# connect_secs = 30                     # TCP/TLS connect timeout (0 = none)
# first_byte_secs = 300                 # from sending the request until response headers arrive (0 = none)
# read_idle_secs = 300                  # longest gap between two response-body reads (0 = none)

[keepalive]
# interval_secs = 15                    # SSE heartbeat interval, 0 disables keepalive
# trigger = "any"                       # accept (Accept has SSE) / body_stream (body "stream": true) / any (either, default)
# heartbeat = ": keepalive\n\n"         # heartbeat bytes per tick (default SSE comment; e.g. "\n" for clients that choke on comments)

[retry]
# max_backoff_secs = 320                # retry backoff cap (0 = retry instantly)
# [[retry.rules]]                      # attempt N with an upstream response stops once N >= max_attempts (0 = no limit, an exemption)
# path = '/v1/messages/count_tokens(\?.*)?'   # regex over path?query, anchored at both ends
# max_attempts = 3

[history]
# body_budget_mb = 64                   # cap on request/response bodies kept in the request history (MB of memory; 0 = keep none)

# [[format]]                             # external transformer: at most one per stage (request/response/heartbeat)
# name = "convert"
# command = "~/.aproxy/bin/aproxy-format"
# args = ["run", "--config", "~/.aproxy/agg.toml"]   # no extra key any more: pass configuration through args or env
# process = "persistent"                 # spawn (default) / persistent (required for stateful programs)
# stages = ["request", "retry"]          # request / response / heartbeat; adding retry re-runs it before every retry
# env = { KEYS = '{"keys": ["sk-a", "sk-b"]}' }  # extra environment variables (values are never shown by config show)
```

Multi-key rotation takes effect **between requests** by default: retries of the
same request keep the same transformed key (a request is transformed once and
the result is replayed on every retry). Add `"retry"` to the request format's
`stages` to transform again before every retry.

Runtime data lives in `~/.aproxy/`: `run/` (instance records `<port>.instance.json`, desired-state
records `<port>.restore.json`, the watchdog record `watchdog.json`, and a short-lived launch-spec
handoff while an instance starts), `logs/` (daemon logs, randomly named per
start, rotated and
orphan-cleaned; paths reported via IPC — customize with `log_file`),
`spool/<pid>-<hash>/` (disk-cache
scratch space, one directory per instance process; removed when the instance
stops, and leftovers of a crashed one are removed by the next instance that
starts or by the watchdog), `settings.json` (the home's program state: `aliases`,
`default_config`, `config_dirs`, `defaults` with global config.toml defaults, and
the `watchdog`, `logs`, `instances`, `update` and `ui` sections).

`settings.json` is read and written with
`aproxy settings show|get|set|unset` (keys are dotted paths such as
`watchdog.enabled` or `defaults.max_request_mb`); `aproxy alias add|remove` and
`aproxy settings set|unset default_config` edit their own keys. Hand-edit it
only when it cannot be parsed at all: it is parsed strictly, so an unknown key or
a wrong type is an error. In that state `status`, `stop` and `install` run on
built-in defaults with a warning (aliases unavailable) and every other command
exits 3 with the repair steps. Credentials in `defaults` (`api_key`, secret
headers, proxy passwords) are inherited only by instances whose `base_url` /
`proxy` names the same target; `aproxy config show --effective` lists what was withheld.
The fields are documented in the skill file
[settings-json.md](.claude/skills/aproxy-cli/references/latest/settings-json.md).

### Keepalive (SSE heartbeats)

A request is eligible for keepalive when `keepalive.interval_secs` > 0, the
instance's `delivery` is not `"forward"`, and it matches `keepalive.trigger`:
`Accept` contains `text/event-stream`, or the body has a top-level `"stream":
true` (default `any` = either). Eligible requests use the keepalive channel from
the **first** attempt:

- If any attempt gets a 2xx with an uncompressed `text/event-stream` from the
  upstream, its real status and headers are forwarded to the client immediately
  (when no `response`-stage format is configured); otherwise, after about one
  keepalive interval, skeleton headers (200 + SSE) are committed. Needing a
  retry does not commit anything by itself, so a request that succeeds after a
  few quick retries still gets the real upstream response.
- A `"stream": true` stream that is not SSE (e.g. NDJSON from Ollama's native
  API) passes through unaltered when no retry is needed; if that upstream often
  needs retries (a retry lasting past one keepalive interval commits SSE
  skeleton headers, rewriting the content type and mixing in heartbeats), set
  `[keepalive] trigger = "accept"` for that instance.
- Once committed, SSE comment heartbeats go out every
  `keepalive.interval_secs` while waiting for the first byte, while the
  upstream is in flight, while its stream is being buffered and during backoff;
  the body is still replayed only after it is fully buffered and checked, so
  data from failed attempts never leaks in.
- To avoid injecting plain-text heartbeats into a compressed stream, eligible
  requests are sent upstream with `accept-encoding: identity` — a deliberate
  exception to "total passthrough" that only costs some extra bytes between the
  upstream and this machine; ineligible requests are not rewritten.
- Ordinary `"stream": false` requests have no keepalive channel, nor do
  `delivery = "forward"` instances or `keepalive.interval_secs = 0`.

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
  under `defaults` in `settings.json` (toml wins).
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
- `api_key` is stored in plain text in `config.toml`. aProxy's home directory
  (`~/.aproxy` by default) is accessible only to your own user: it is created
  that way, an existing home with looser permissions is tightened the next
  time aProxy writes into it, and `aproxy doctor` points out a home that other
  users can read. Restrict the permissions yourself for a config kept outside
  the home (`--config` pointing elsewhere).
- The control channel between commands such as `status` and `stop` and the
  instance accepts only the user who started the instance (plus root on
  Unix), and it never opens a network port.
- External transformers (`[[format]]` entries) run
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

The default features include the desktop interface, so build the front end once before compiling
(see "From source"). Check the headless build too, excluding `aproxy-desktop` (it is a workspace
member, and `--no-default-features` does not reach it):

```sh
cargo test --workspace --exclude aproxy-desktop --no-default-features --locked
cargo clippy --workspace --exclude aproxy-desktop --all-targets --no-default-features --locked -- -D warnings
```

Interface work: `npm run dev` in `aproxy-desktop/ui` opens the prototype in a browser, with its
review toolbar and simulated window; afterwards `npm run build` (svelte-check first, zero
warnings) and compile aproxy.

Architecture docs in [`docs/`](docs/); agent-facing development docs in
`.agents/docs/`.

## License

MIT
