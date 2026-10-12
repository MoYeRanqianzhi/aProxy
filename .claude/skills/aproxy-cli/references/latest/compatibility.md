# Version compatibility

How to tell which aProxy version is actually running, which versions these references describe, and
what to do when they differ. Read it when the version you see is not one these docs describe, when
output does not match what these docs quote, or before moving an installation from 0.1.x to 0.2.0,
which cannot be done in place.

## Find the version in use

Two versions matter, and they can differ:

| Command | Reports |
|---|---|
| `aproxy --version` | the CLI binary that `aproxy` resolves to |
| `aproxy status` | `v<version>` per running instance: the binary that instance was started from |

From 0.2.0 on, pre-releases included, `aproxy --version` (or `-V`) prints one line in a format that
every later version keeps: `aproxy v<version> (<variant>; <target>[; x86-64-v3])`, for example
`aproxy v0.2.0 (desktop; x86_64-pc-windows-msvc; x86-64-v3)`. The variant is `desktop` for builds
with the desktop interface and `headless` for builds without it; the target is the platform the
binary was built for, such as `x86_64-unknown-linux-musl`; `x86-64-v3` marks a build for CPUs with
AVX2. The version is the text after `aproxy v` up to the first space. 0.1.x prints
`aproxy <version>` instead. `--version` reads and writes nothing under `<home>` and starts no
process, so it is safe to run at any time, even in the middle of an install.

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
  the keys it does not know when it rewrites config.toml or settings.json. A 0.1.x copy and a 0.2.0
  or later copy cannot control each other's instances at all (see
  [Moving from 0.1.x to 0.2.0](#moving-from-01x-to-020)).
- An `aproxy install` that halts mid-way leaves the new binary in place while some instances still
  run the old one; commands.md, install / upgrade, explains how to finish.

## What these docs describe

These references describe aProxy 0.1.0, the first stable release, and the development line after
it, which becomes 0.2.0. Builds from source and the 0.2.0 pre-releases report a 0.2.0 pre-release
version such as `0.2.0-alpha.0`. Their changes since 0.1.0 are listed in `CHANGELOG.md` at the root
of the repository, https://github.com/MoYeRanqianzhi/aProxy, under "[Unreleased]" until they ship;
notes for each release are on its Releases page.

When a behavior or a Chinese log line quoted in these docs does not show up on a 0.1.0 instance,
check the CHANGELOG before treating it as a fault: the docs may describe a change that release
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
| An instance whose control channel cannot be created keeps proxying without it (logging `IPC 控制通道启动失败…`); `stop` and `status` cannot reach it, and its registry record may replace another instance's | it refuses to start (troubleshooting.md, Starting and stopping) |
| The watchdog respawns a crashed instance at its next scan, up to `watchdog_heartbeat_secs` (30 s) later | it respawns at once (behaviors.md) |
| On Windows the control pipe is named after the port alone, so `stop`, `restart` and `logs` under any `APROXY_HOME` reach whatever instance listens on that port, and `start` reports it as already running | the control channel belongs to one home (commands.md, Experiment in an isolated home) |
| `aproxy install` runs the whole upgrade itself; on Windows with running instances it prints `交换完成，剩余阶段（滚动重启/终验）由新版本进程继续，等待其完成...` after the swap and the new binary finishes the rollout | the downloaded binary runs everything after the download on every platform and the line is `新版本进程已接手安装…` (commands.md, install / upgrade) |
| An online `aproxy install` moves every running instance onto `<home>/bin/`, including instances started from an npm, cargo or package-manager copy | an online install refuses while such an instance runs, as `--from` does (commands.md, install / upgrade) |
| `aproxy install` keeps its progress in `<home>/run/install.state` and downloads into `<home>/staging/` | `<home>/install/state.json` and `<home>/install/staging/` (commands.md, Home directory) |
| `upstream_url` is accepted as another name for `base_url` | only `base_url` is read; rename the key |
| `keepalive_heartbeat` is ignored; heartbeats are always `: keepalive` | the heartbeat is configurable (config-toml.md) |
| `heartbeat_transform` is ignored, and transformer envelopes carry no `stage`, `request_id` or `state` | heartbeats can come from a format program, and both transformer sides of a request share `request_id` and `state` (aproxy-format skill) |
| There is no `aproxy desktop` | builds with the desktop interface open a window with it; in 0.2.0-alpha.1 it shows demo data only (commands.md, desktop) |
| There is no `aproxy install --force` and no process ledger | force install stops every process of the home through `<home>/ledger/` and replaces the binary (commands.md, Force install) |
| The CPU build option is `--variant v3\|baseline`, and the swap renames the old binary to the fixed name `aproxy.old(.exe)` | the option is `--cpu x86-64-v3\|baseline` (`--variant` now picks `desktop` or `headless`), and the swap uses unique names `aproxy.old-<nanoseconds>-<pid>[.exe]` (commands.md, install / upgrade) |

For a version newer than these docs, refresh them: `aproxy install` updates the skill documents in
`<home>/skills/` together with the binary, and `aproxy install --skills-only` updates only the
documents. If your agent loads this skill from a different directory, copy or link it from there.

aproxy-format, the companion transformer, is released separately (tags `format-v*`) with its own
version numbers; see the aproxy-format skill.

## Moving from 0.1.x to 0.2.0

0.2.0 changed the control protocol, so the two lines cannot work on each other's instances: a 0.2.0
or later CLI cannot stop, restart or inspect a 0.1.x instance, and a 0.1.x CLI cannot reach a 0.2.0
one. An in-place upgrade would break halfway, with the binary
replaced and the instances out of reach. To prevent that, 0.1.x refuses before it changes anything:
`aproxy install` (or `upgrade`) downloads the new version, runs its `--version` to check it, cannot
read the new format, and exits 1 with an `[ERROR]` line containing
`--version 输出无版本号: "aproxy v0.2.0 (…)` ("no version number in the --version output").
`install --from` stops the same way. The download itself worked, so ignore any download-proxy advice
printed after the error. Instances, `<home>/bin/`, the skill documents and the install state are as
they were; the download is left in `<home>/staging/<version>/` and can be deleted.

Move by reinstalling instead. Stopping instances cuts the requests they are serving, so agree on a
moment with the user first.

1. With the 0.1.x CLI, note what runs: `aproxy status` (ports and config files) and
   `aproxy alias list`. Back up `settings.json` and the `*.toml` files under `<home>`.
2. With the 0.1.x CLI, run `aproxy stop all`, then `aproxy status` until it lists no instance. Do
   this before the binary changes: afterwards no command can stop these instances gracefully.
3. Replace the binary through the channel that installed it:
   - install script or `aproxy install`: delete `<home>/bin/aproxy` (`aproxy.exe` on Windows, and
     `aproxy.old.exe` if present), then run the install script again; it does not overwrite an
     existing binary. If Windows refuses the delete, the 0.1.x watchdog is still running from the
     file; it exits by itself, by default 5 minutes after the last instance stopped.
   - npm: `npm i -g @meowo/aproxy@latest` (`@next` for a pre-release).
   - cargo: `cargo install aproxy --locked --force` (add `--version <version>` for a pre-release).
     `--locked` builds with the dependency versions tested for the release.
4. Delete what 0.1.x leaves behind: `<home>/run/install.state` and `<home>/staging/`, which 0.2.0
   never reads, and any `<home>/run/*.restore`. A restore record that survived step 2 belongs to an
   instance that had crashed, and `aproxy restore` would start it with 0.1.x's command line.
5. Keep `settings.json` and the config files. Apply the renames listed in the CHANGELOG (for
   example `upstream_url` is now `base_url`), then run `aproxy doctor`.
6. Start each instance with the new CLI: `aproxy start <alias or config path>`. Ports are unchanged,
   so agent clients need no change.
7. Refresh these documents with `aproxy install --skills-only`, and check that the agent's skills
   directory links to or copies `<home>/skills/aproxy-cli`.

If the binary was replaced while 0.1.x instances were still running, `aproxy status` lists them
under `无响应的 aProxy 实例 (<n>)——进程仍在，但不应答控制通道` ("unresponsive instances: the process
is still there but does not answer the control channel") with version `v0.1.<x>`. Once the user
agrees, end each with `aproxy stop <port> --force`, then continue from step 4.

### Going back to 0.1.x

0.2.0 and later never install a version below 0.2.0, whether named or given with `--from`, and
`--allow-downgrade` does not change that. They stop with
`目标版本 <v> 低于 0.2.0，不能安装：0.1.x 与 0.2.0 起的版本互不兼容…` ("target version below 0.2.0
cannot be installed: 0.1.x and 0.2.0 or later are incompatible"). To go back, stop every instance
with the current CLI (`aproxy stop all`), delete the binary from `<home>/bin/` as in step 3, and
install 0.1.x with that release's install script, naming its tag, for example
`curl -fsSL https://raw.githubusercontent.com/MoYeRanqianzhi/aProxy/v0.1.0/scripts/install.sh | sh -s -- v0.1.0`.
On Windows, put the release binary from the Releases page at `<home>/bin/aproxy.exe` instead. Then
start the instances with 0.1.x.

## Alpha builds

Versions such as `0.1.0-alpha.17` are pre-releases from before 0.1.0. They are unsupported and
these docs do not track how they differ, so upgrade rather than look up alpha behavior:

1. Run `aproxy install 0.1.0`. Name the version: a pre-release CLI resolves `latest` in the
   pre-release channel and can pick a 0.2.0 pre-release, which an alpha cannot install any more
   than 0.1.x can. To reach 0.2.0, continue with [Moving from 0.1.x to 0.2.0](#moving-from-01x-to-020).
2. If the build has no `install` command, reinstall with the project's install script or your
   package manager.
3. Check that `aproxy status` shows the new version for every instance, and run
   `aproxy restart <port>` for any that does not.

Config files from alpha builds load unchanged, with one exception on the development line: rename
`upstream_url` to `base_url`. Otherwise keys a version does not know are ignored and missing keys take
their defaults. If something stops working after the upgrade, look the symptom up in
troubleshooting.md.
