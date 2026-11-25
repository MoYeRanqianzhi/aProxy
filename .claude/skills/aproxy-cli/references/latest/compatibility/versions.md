# Version compatibility: what these docs describe

Part of [compatibility.md](../compatibility.md), which lists the other parts. Which version these docs describe and how older versions differ.

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
| A `default_config` pointing to a missing file makes every command without `--config` exit 1, `status`, `stop` and `config --clear-default` included; add `--config <any path>` to get past it | only commands that read the config fail (commands.md, Options for start) |
| With a corrupt settings.json, `alias add` and `config --set-default/--clear-default` save the empty defaults over it, losing every alias | every command outside the tolerated group exits 3 before changing anything; the file is never rewritten from the defaults (commands.md, alias) |
| A relative `log_file` / `--log-file` resolves against the working directory of the process that starts the instance; use absolute paths | relative paths resolve against `<home>` |
| `--config` is used as given (no `~` expansion); a relative path is recorded relative, so `restart` or `restore` run from another directory cannot find it and `restore` deletes the record; pass absolute paths | `~` is expanded and the path made absolute (commands.md, Options for start) |
| `aproxy install VERSION --skills-only` is rejected; `--skills-only` always takes the channel's latest | a VERSION is accepted |
| The hint about the client's stream idle timeout appears only after about 590 s of heartbeats and names Claude Code alone | it appears after 60 s for any client (troubleshooting.md) |
| No heartbeats while a request or response transformer runs, so a slow format program can leave the client without a byte | format programs run within the heartbeat cycle (behaviors.md) |
| `status` deletes the record of an instance that does not answer even if its process is alive (hung), after which `stop --force` cannot find it; `stop` exits 0 even when a stop was not confirmed | hung instances are listed as unresponsive and `stop --force` reaches them; `stop` exits 1 when a target was not confirmed stopped (commands.md, status and stop) |
| An instance whose control channel cannot be created keeps proxying without it (logging `IPC 控制通道启动失败…`); `stop` and `status` cannot reach it, and its registry record may replace another instance's | it refuses to start (troubleshooting.md, Starting and stopping) |
| The watchdog respawns a crashed instance at its next scan, up to `watchdog_heartbeat_secs` (30 s) later | it respawns at once (behaviors.md) |
| On Windows the control pipe is named after the port alone, so `stop`, `restart` and `logs` under any `APROXY_HOME` reach whatever instance listens on that port, and `start` reports it as already running | the control channel belongs to one home (commands.md, Experiment in an isolated home) |
| `aproxy install` runs the whole upgrade itself; on Windows with running instances it prints `交换完成，剩余阶段（滚动重启/终验）由新版本进程继续，等待其完成...` after the swap and the new binary finishes the rollout | the downloaded binary runs everything after the download on every platform and the line is `新版本进程已接手安装…` (commands.md, install / upgrade) |
| An online `aproxy install` moves every running instance onto `<home>/bin/`, including instances started from an npm, cargo or package-manager copy | an online install refuses while such an instance runs, as `--from` does (commands.md, install / upgrade) |
| `aproxy install` keeps its progress in `<home>/run/install.state` and downloads into `<home>/staging/` | `<home>/install/state.json` and `<home>/install/staging/` (commands.md, Home directory) |
| `upstream_url` is accepted as another name for `base_url` | only `base_url` is read; rename the key |
| `settings.json` is a flat object whose unknown fields are ignored, and a corrupt one makes commands warn and run on built-in defaults | it is sectioned (aliases, `defaults`, `watchdog`, `logs`, `instances`, `update`, `ui`) and parsed strictly: any unknown or 0.1.x field is an error, and commands outside `settings`/`doctor`/`status`/`stop`/`install` exit 3 (settings-json.md) |
| `keepalive_heartbeat` is ignored; heartbeats are always `: keepalive` | the heartbeat is configurable (config-toml.md) |
| A transformer is one program per direction, fed a 0.1.x envelope without `stage`, `request_id` or `state`; `heartbeat_transform` is ignored | format programs form a chain and speak protocol 1: a `hello` handshake, seven stages (`request`, `retry`, `decide`, `response`, `stream`, `heartbeat`, `notify`), a `request_id` and per-program `state` on every message. A 0.1.x transformer does not work unchanged (aproxy-format skill) |
| There is no `aproxy desktop` | builds with the desktop interface open a window with it; in the 0.2.0 alpha releases it shows demo data only (commands.md, desktop) |
| There is no `aproxy install --force` and no process ledger | force install stops every process of the home through `<home>/ledger/` and replaces the binary (commands.md, Force install) |
| The CPU build option is `--variant v3\|baseline`, and the swap renames the old binary to the fixed name `aproxy.old(.exe)` | the option is `--cpu x86-64-v3\|baseline` (`--variant` now picks `desktop` or `headless`), and the swap uses unique names `aproxy.old-<nanoseconds>-<pid>[.exe]` (commands.md, install / upgrade) |
| `aproxy install` exits 1 on any failure, has no `--check` or `--json`, and is resumed in the background by any command after an interruption | it refuses up front while instances it cannot restart run, prechecks every instance before it touches one (exit 3 when that fails), exits 4 for a version that does not exist, has `--check` and `--json`, and only commands that change something resume it (commands.md, install / upgrade) |
| `aproxy doctor` exits 0 whatever it finds, checks the default config only for a non-loopback address, and calls an alias that points to a missing file an error | it exits 3 when it finds an error, checks every config the way `start` would, and reports a missing alias target as a warning; `--json` gives the whole report (commands.md, doctor) |
| Commands print their log lines, `INFO` included, on stdout | log lines go to stderr, warnings and errors only unless `RUST_LOG` asks for more, so stdout holds only results (commands.md, Output and exit codes) |

For a version newer than these docs, refresh them: `aproxy install` updates the skill documents in
`<home>/skills/` together with the binary, and `aproxy install --skills-only` updates only the
documents. If your agent loads this skill from a different directory, copy or link it from there.

aproxy-format, the companion transformer, is released separately (tags `format-v*`) with its own
version numbers; see the aproxy-format skill.
