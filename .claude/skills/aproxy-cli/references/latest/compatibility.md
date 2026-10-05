# Version compatibility

How to tell which aProxy version is actually running, which versions these references describe, and
what to do when they differ. Read it when `aproxy --version` or `aproxy status` shows something
other than 0.1.0, or when output does not match what these docs quote.

## Find the version in use

Two versions matter, and they can differ:

| Command | Reports |
|---|---|
| `aproxy --version` | the CLI binary that `aproxy` resolves to, as `aproxy <version>` |
| `aproxy status` | `v<version>` per running instance: the binary that instance was started from |

- A running instance keeps its version until it restarts; replacing the binary on disk changes
  only the CLI. `status` then adds
  `注意: 实例版本 v<x> 与当前 CLI v<y> 不同，替换二进制后执行 aproxy restart <port> 可完成升级`
  ("instance version differs from this CLI; restart it to finish the upgrade").
- Judge runtime behavior (retries, heartbeats, log messages) by the instance's version, and
  command syntax by the CLI's.
- Several copies may be installed: `aproxy install` manages `<home>/bin/` (`<home>` is
  `~/.aproxy` unless `APROXY_HOME` is set), while npm, cargo or a package manager put theirs
  elsewhere. `where aproxy` (Windows) or `command -v aproxy` shows which one runs. `restart`
  launches the copy you invoke, so restarting from an older copy downgrades the instance. Run
  file-writing commands (`config` setters, `alias`) with the newest copy too: an older binary drops
  the keys it does not know when it rewrites config.toml or settings.json.
- An `aproxy install` that halts mid-way leaves the new binary in place while some instances still
  run the old one; commands.md, install / upgrade, explains how to finish.

## What these docs describe

These references describe aProxy 0.1.0, the first stable release, and the development line after
it. A development build (built from source) reports the version of the last release until the next
release changes the number, so a build that reports 0.1.0 may contain changes that the 0.1.0
release lacks. Those changes are listed under "[Unreleased]" in `CHANGELOG.md` at the root of the
repository, https://github.com/MoYeRanqianzhi/aProxy; notes for each release are on its Releases
page.

When a behavior or a Chinese log line quoted in these docs does not show up on a 0.1.0 instance,
check "[Unreleased]" before treating it as a fault: the docs may describe a change that release
does not have. These development-line changes alter what you see or do on 0.1.0:

| On 0.1.0 | These docs describe |
|---|---|
| A `default_config` pointing to a missing file makes every command without `--config` exit 1, `status`, `stop` and `config --clear-default` included; add `--config <any path>` to get past it | only commands that read the config fail (commands.md, Global options) |
| With a corrupt settings.json, `alias add` and `config --set-default/--clear-default` save the empty defaults over it, losing every alias | these commands refuse to write (commands.md, alias) |
| A relative `log_file` / `--log-file` resolves against the working directory of the process that starts the instance; use absolute paths | relative paths resolve against `<home>` |
| `--config` is used as given (no `~` expansion); a relative path is recorded relative, so `restart` or `restore` run from another directory cannot find it and `restore` deletes the record; pass absolute paths | `~` is expanded and the path made absolute (commands.md, Global options) |
| `aproxy install VERSION --skills-only` is rejected; `--skills-only` always takes the channel's latest | a VERSION is accepted |
| The hint about the client's stream idle timeout appears only after about 590 s of heartbeats and names Claude Code alone | it appears after 60 s for any client (troubleshooting.md) |
| No heartbeats while a request or response transformer runs, so a slow format program can leave the client without a byte | transformers run within the heartbeat cycle (behaviors.md) |
| `status` deletes the record of an instance that does not answer even if its process is alive (hung), after which `stop --force` cannot find it; `stop` exits 0 even when a stop was not confirmed | hung instances are listed as unresponsive and `stop --force` reaches them; `stop` exits 1 when a target was not confirmed stopped (commands.md, status and stop) |
| The watchdog respawns a crashed instance at its next scan, up to `watchdog_heartbeat_secs` (30 s) later | it respawns at once (behaviors.md) |
| `upstream_url` is accepted as another name for `base_url` | only `base_url` is read; rename the key |

For a version newer than these docs, refresh them: `aproxy install` updates the skill documents in
`<home>/skills/` together with the binary, and `aproxy install --skills-only` updates only the
documents. If your agent loads this skill from a different directory, copy or link it from there.

aproxy-format, the companion transformer, is released separately (tags `format-v*`) with its own
version numbers; see the aproxy-format skill.

## Alpha builds

Versions such as `0.1.0-alpha.17` are pre-releases from before 0.1.0. They are unsupported and
these docs do not track how they differ, so upgrade rather than look up alpha behavior:

1. Run `aproxy install 0.1.0`, or name a later stable version from the Releases page. Name it
   explicitly: a pre-release CLI resolves `latest` in the pre-release channel and could pick a
   newer pre-release. Once a stable release is installed, plain `aproxy install` stays on stable
   releases.
2. If the build has no `install` command, reinstall with the project's install script or your
   package manager.
3. Check that `aproxy status` shows the new version for every instance, and run
   `aproxy restart <port>` for any that does not.

Config files from alpha builds load unchanged, with one exception on the development line: rename
`upstream_url` to `base_url`. Otherwise keys a version does not know are ignored and missing keys take
their defaults. If something stops working after the upgrade, look the symptom up in
troubleshooting.md.
