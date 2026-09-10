# Installing / Upgrading aProxy — Agent Instructions

You are an AI agent (Claude Code, Codex, or similar) asked to install or
upgrade aProxy on this machine. Follow this guide exactly, in order. Every
command is safe to run as written; none of them will touch processes or files
you did not create.

aProxy is a local API proxy that gives agent workloads infinite retries. After
installation, point your API base URL at it (step 4) — that is the whole point.

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

**Windows (PowerShell — preferred):**

```powershell
irm https://raw.githubusercontent.com/MoYeRanqianzhi/aProxy/master/scripts/install.ps1 | iex
```

**Windows (cmd fallback, when PowerShell is unavailable):**

```bat
curl -fsSL https://raw.githubusercontent.com/MoYeRanqianzhi/aProxy/master/scripts/install.cmd -o "%TEMP%\aproxy-install.cmd" && "%TEMP%\aproxy-install.cmd"
```

**Linux / macOS / Git Bash on Windows:**

```sh
curl -fsSL https://raw.githubusercontent.com/MoYeRanqianzhi/aProxy/master/scripts/install.sh | sh
```

Notes:

- The script may download through a proxy: set `APROXY_DL_PROXY` (sh) or
  `-DownloadProxy <URL>` (ps1) if the machine needs one. The default
  `HTTPS_PROXY` environment variable is honored automatically.
- Skip the skill docs with `APROXY_NO_SKILLS=1` (sh) or `-NoSkills` (ps1).
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
aproxy install          # latest version; `aproxy upgrade` is an alias
```

**Version gate:** the `install` command ships with a later release than
0.1.0-alpha.7. Check `aproxy --version`; if `install` is not a known command
on the installed build, upgrade manually:

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

- Start it: `aproxy` (background daemon) or `aproxy --foreground` (debugging).
- Configure your API base URL to the local endpoint; keep the request path
  unchanged (e.g. `https://api.anthropic.com/v1/messages` becomes
  `http://127.0.0.1:12345/v1/messages`).
- The upstream address is set in `~/.aproxy/config.toml` (`base_url = ...`);
  edit it, then apply with `aproxy restart` (or `aproxy config --baseurl
  <URL>` before starting).

## 5. Skill documentation (optional, for agent self-service)

The installer places the full CLI reference at
`~/.aproxy/skills/aproxy-cli/`. To use it as your own skill documentation, link
it into your skills directory, e.g. for Claude Code:

```sh
ln -s ~/.aproxy/skills/aproxy-cli ~/.claude/skills/aproxy-cli
```

It covers every command, config field, behavior semantics, and
troubleshooting — read it before improvising CLI flags.

## 6. Troubleshooting quick table

| Symptom | Action |
|---|---|
| Start fails: port occupied | `netstat -ano \| findstr <port>` (Windows); another program owns it |
| Start fails: permission / reserved range | Hyper-V/WinNAT excluded range — pick another port |
| Start fails: other | read `~/.aproxy/logs/startup.log` |
| Client sees timeouts during long retries | non-streaming requests have no keepalive channel; raise client timeout |
| Everything else | `aproxy status`, instance logs `~/.aproxy/logs/<port>.log` |

The full reference lives in the skill docs (step 5) or
[docs/](https://github.com/MoYeRanqianzhi/aProxy/tree/master/docs).
