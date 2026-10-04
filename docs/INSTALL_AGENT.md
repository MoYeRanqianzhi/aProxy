# Installing / Upgrading aProxy — Agent Instructions

You are an AI agent (Claude Code, Codex, or similar) asked to install or
upgrade aProxy on this machine. Follow this guide exactly, in order. Every
command is safe to run as written; none of them will touch processes or files
you did not create.

aProxy is a local API proxy that gives agent workloads infinite retries. After
installation, point your API base URL at it (step 4) — that is the whole point.

Platform support: Windows and Linux (x86_64 / aarch64, glibc and musl) are fully
supported. On macOS pre-built binaries are published but untested on real
hardware; the watchdog, `aproxy install` and process-query-based instance
management rely on Linux-only interfaces and are unavailable or degraded.

## 0. Check current state first

```sh
aproxy --version 2>/dev/null || echo "not installed"
```

- **Command not found** and `~/.aproxy/bin/` does not exist → fresh install, go
  to step 1.
- **A version prints** → aProxy is installed; go to step 2 (upgrade) only if
  the user asked for an upgrade, otherwise skip to step 4.

## 1. Fresh install — bootstrap script (preferred)

One command per platform. The script downloads the binary and the skill
documentation, verifies SHA256, and places everything under `~/.aproxy/`
(override with the `APROXY_HOME` environment variable). It never overwrites an
existing installation.

**Which version it installs:** only releases tagged `vX.Y.Z` or
`vX.Y.Z-(alpha|beta|rc).N` are considered (the repository also hosts
`format-v*` releases for `aproxy-format` and a few historical test tags such
as `v0.1.0-alpha.12t3`; those are ignored) and drafts are skipped. By default it installs the newest **stable**
release; if the repository has no stable release yet (before 0.1.0) it falls
back to the highest-versioned pre-release and prints a note. Pass `--pre`
(`-Pre` for PowerShell, or set `APROXY_PRE=1`) to take the highest version,
pre-release or not (by version number, not by creation time). An explicit tag (`sh install.sh v0.1.0`,
`-Version v0.1.0`) overrides all of this.

**Windows (PowerShell — preferred):**

```powershell
irm https://raw.githubusercontent.com/MoYeRanqianzhi/aProxy/main/scripts/install.ps1 | iex
```

**Windows (cmd fallback):** `install.cmd` is only a thin front-end — it still
downloads through the Windows PowerShell that ships with the system
(`Invoke-WebRequest`), so it does not help when PowerShell is entirely
unavailable. Use it when the `irm | iex` route is blocked or you prefer a batch
file.

```bat
curl -fsSL https://raw.githubusercontent.com/MoYeRanqianzhi/aProxy/main/scripts/install.cmd -o "%TEMP%\aproxy-install.cmd" && "%TEMP%\aproxy-install.cmd"
```

**Linux / macOS / Git Bash on Windows:**

```sh
curl -fsSL https://raw.githubusercontent.com/MoYeRanqianzhi/aProxy/main/scripts/install.sh | sh
# pre-release: curl -fsSL .../install.sh | sh -s -- --pre
```

Notes:

- The script may download through a proxy: set `APROXY_DL_PROXY` (sh and ps1)
  or `-DownloadProxy <URL>` (ps1) if the machine needs one. The default
  `HTTPS_PROXY` environment variable is honored automatically.
- Skip the skill docs with `APROXY_NO_SKILLS=1` (sh and ps1) or `-NoSkills`
  (ps1). `irm | iex` cannot pass arguments, so for PowerShell set the
  environment variables instead (`$env:APROXY_PRE = "1"; irm ... | iex`).
- Windows PowerShell 5.1 cannot parse `install.ps1` when it is run locally with
  `-File` (the file is UTF-8 without BOM). Use the `irm | iex` line above, or
  `pwsh -File scripts/install.ps1`.
- The sh script needs one of `sha256sum`, `shasum` or `openssl` to verify the
  download; without one it stops before downloading anything.
- Linux: the gnu build requires glibc 2.28 or newer. If glibc is older or the
  system is musl-based, the script picks the statically linked musl build
  automatically. Before placing the binary it runs
  `--version` as a self-test, and if the gnu build cannot run on the machine it
  switches to the musl build. Nothing is placed unless the self-test passes.
- If the script reports the download directory is not on `PATH`, follow its
  printed instruction (it differs per platform) or invoke the binary by full
  path `~/.aproxy/bin/aproxy`.

Source build instead (only if scripts are blocked): `git clone
https://github.com/MoYeRanqianzhi/aProxy.git && cd aProxy && cargo build
--release` — the binary lands in `target/release/`.

## 2. Upgrade

**Preferred: the built-in updater** — `aproxy install` replaces the binary and
rolling-restarts every running instance automatically:

```sh
aproxy install          # latest version in your channel; `aproxy upgrade` is an alias
aproxy install --pre    # let pre-releases (alpha/beta/rc) compete as well
```

**Update channel:** `latest` picks the highest version *inside a channel* (by
version number, not creation time). If the installed version is a stable release
only stable releases are considered, so you are never carried onto a pre-release;
if it is a pre-release (e.g. an alpha) the default is already the pre-release
channel; `--pre` selects the pre-release channel explicitly. Only `vX.Y.Z` and
`vX.Y.Z-(alpha|beta|rc).N` releases count (`format-v*` and historical test tags
do not). If the channel has nothing newer you get "already up to date" or "no
version available" and nothing changes — it never switches channels on its own.
A specific version (`aproxy install 0.1.0`) ignores the channel; a target below
the current version is refused unless `--allow-downgrade` is given.

**Rollback:** if an instance cannot start under the new version during the
rolling restart, `install` brings it back with the old binary and the original
arguments, stops the rollout (other instances are left untouched) and exits
non-zero. Read `~/.aproxy/logs/startup.log`, fix the cause and run `aproxy
install` again; `~/.aproxy/run/install.state` (phase `failed`, `last_error`)
keeps the details. On Windows the rolling restart finishes in a background
process after the command returns, so check `install.state` and `aproxy status`
rather than the exit code.

**Version gate:** the `install` command exists from 0.1.0-alpha.10 on. Check
`aproxy --version`; if `install` is not a known command on the installed build,
upgrade manually (on macOS, where `aproxy install` is not supported yet, use
this manual route as well):

1. `aproxy status` — list running instances.
2. `aproxy stop all` — stop them (on Windows the running exe locks the binary;
   stopping releases the lock).
3. Replace the binary at `~/.aproxy/bin/aproxy[.exe]` with the new one
   (download the matching `aproxy-<target>` asset from
   <https://github.com/MoYeRanqianzhi/aProxy/releases> and verify its
   `.sha256`).
4. `aproxy restore` — bring the instances back.

Caution: only stop aProxy processes that belong to this installation
(verified via `aproxy status`). Never kill unknown processes.

## 3. Verify

```sh
aproxy --version   # prints the installed version
aproxy status      # lists running instances (empty is fine after install)
```

## 4. Point agent traffic at aProxy

aProxy transparently forwards everything to the configured upstream. Default
local endpoint: **`http://127.0.0.1:12345`**.

- **A fresh install has no upstream configured**, and starting without a
  `base_url` fails. Configure it first, then start:

  ```sh
  aproxy config --baseurl <UPSTREAM_URL> --api-key <KEY>
  aproxy
  ```

- Start it: `aproxy` (background daemon) or `aproxy --foreground` (debugging).
- **Claude Code** — set both of these (shell environment variables, or the `env`
  field of Claude Code's `~/.claude/settings.json`):

  ```sh
  export ANTHROPIC_BASE_URL=http://127.0.0.1:12345
  export CLAUDE_STREAM_IDLE_TIMEOUT_MS=86400000
  ```

  The second one is **required**. To keep "a stream that breaks midway is still
  retried transparently", aProxy buffers the whole response and replays it only
  once it checks out; while waiting it sends SSE comment heartbeats. Those cover
  Claude Code's first-byte timeout (about 360 s) and byte-level idle timeout
  (300 s) but not its event-level idle timeout (600 s by default; neither
  comments nor `ping` count as events), so without this variable any retry
  period or long generation beyond 10 minutes makes Claude Code disconnect and
  resend. `API_TIMEOUT_MS` does not control this timer. (Measured on Claude Code
  2.1.288; ordinary `"stream": false` requests have no keepalive channel and
  need a large client timeout — `API_TIMEOUT_MS` for Claude Code.)
- **Codex** — add a provider pointing at aProxy to `~/.codex/config.toml` (or
  `$CODEX_HOME/config.toml`) and raise `stream_idle_timeout_ms` (**required**):

  ```toml
  [model_providers.aproxy]
  name = "aproxy"
  base_url = "http://127.0.0.1:12345/v1"
  env_key = "OPENAI_API_KEY"   # any non-empty value works if aProxy sets api_key
  wire_api = "responses"
  stream_idle_timeout_ms = 86400000
  ```

  Then select it with `model_provider = "aproxy"` (and a `model` the upstream
  serves). Merge this into the user's existing config instead of overwriting
  it, and ask before switching their default `model_provider`. Codex's
  `stream_idle_timeout_ms` defaults to 300 s and is measured between SSE events,
  which the heartbeats do not reset: without it any wait beyond 5 minutes makes
  Codex disconnect and reconnect, and the turn fails after 5 attempts.
  (Measured on codex-cli 0.160.0.)
- **Other clients** — see the README section "Using with agent clients". In
  short: Qwen Code needs `QWEN_STREAM_IDLE_TIMEOUT_MS=0` and
  `QWEN_STREAM_MAX_LIFETIME_MS=0`; dsh through its `llm-pi-ai` adapter needs
  `streamIdleTimeoutMs` on that provider; Gemini CLI works with
  `GOOGLE_GEMINI_BASE_URL` + `GEMINI_API_KEY` but is limited by its hard-coded
  300 s response-header timeout; pi, OpenCode, Aider, Cline, Roo Code and Kimi
  CLI work with their defaults.
- Configure your API base URL to the local endpoint; keep the request path
  unchanged (e.g. `https://api.anthropic.com/v1/messages` becomes
  `http://127.0.0.1:12345/v1/messages`).
- The upstream address is set in `~/.aproxy/config.toml` (`base_url = ...`);
  edit it, then apply with `aproxy restart` (or `aproxy config --baseurl
  <URL>` before starting). `restart` checks the new configuration before
  stopping the running instance, so a typo leaves the old instance running.
- By default the proxy rejects requests that carry an `Origin` header (browser
  and Electron/WebView-style clients send one) with a local 403, and only
  accepts `localhost` / `127.0.0.1` / `[::1]` as Host. CLI agents such as Claude
  Code are unaffected. For a client that does send `Origin`, add it to
  `allowed_origins` in `config.toml` (see the README, "Inbound origin checks").
- Keep the proxy on `127.0.0.1`. Do not set `listen_addr` to `0.0.0.0` or a LAN
  address: the proxy has no authentication of its own and injects your upstream
  key into every request (see SECURITY.md).

## 5. Skill documentation (optional, for agent self-service)

The installer places all skill references under `~/.aproxy/skills/`
(`aproxy-cli` = CLI/config/behavior reference; `aproxy-format` = guide for
writing external transformer programs). To use them as your own skill
documentation, link them into your skills directory, e.g. for Claude Code:

```sh
ln -s ~/.aproxy/skills/aproxy-cli ~/.claude/skills/aproxy-cli
ln -s ~/.aproxy/skills/aproxy-format ~/.claude/skills/aproxy-format
```

They cover every command, config field, behavior semantics, and
troubleshooting — read them before improvising CLI flags.

## 6. Troubleshooting quick table

| Symptom | Action |
|---|---|
| Start fails: port occupied | `netstat -ano \| findstr <port>` (Windows); another program owns it |
| Start fails: permission / reserved range | Hyper-V/WinNAT excluded range — pick another port |
| Start fails: `base_url` is empty | fresh install — run `aproxy config --baseurl <URL> --api-key <KEY>` first |
| Start fails: other | read `~/.aproxy/logs/startup.log` |
| Client gets a local 403 mentioning `allowed_origins` / `allowed_hosts` | the inbound origin check rejected it (never forwarded upstream): add the Origin / host name from the message to `config.toml`, then `aproxy restart <port>` |
| `install.ps1` fails to parse under Windows PowerShell 5.1 | run it via `irm ... \| iex` or `pwsh -File`, not `powershell -File` |
| Install script: no SHA256 tool | install `sha256sum`, `shasum` or `openssl`, then rerun |
| Linux binary will not start (glibc errors) | rerun the install script (it falls back to the musl build), or download the `aproxy-<arch>-unknown-linux-musl` asset |
| Claude Code gives up / resends after about 10 minutes of waiting | `CLAUDE_STREAM_IDLE_TIMEOUT_MS` is not set: Claude Code's event-level idle timeout (600 s) is not reset by aProxy's comment heartbeats. Set it to `86400000` (step 4) |
| Non-streaming (`"stream": false`) requests time out during long retries | they have no keepalive channel (nothing to inject heartbeats into): raise the client's own timeout |
| `aproxy install` stopped with an instance rolled back | the new version failed to start for that instance; read `startup.log`, fix, rerun `aproxy install` |
| `npm i -g @meowo/aproxy` installs an old version | pre-releases are published under the `next` tag: use `@meowo/aproxy@next` |
| Everything else | `aproxy status`, then `aproxy logs <port>` — log files are randomly named per start; never guess them by port |

The full reference lives in the skill docs (step 5) or
[docs/](https://github.com/MoYeRanqianzhi/aProxy/tree/main/docs).
