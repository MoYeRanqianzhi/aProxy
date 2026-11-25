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
  file-writing commands (`config` setters, `alias`, `settings`) with the newest copy too: an older
  binary may drop the keys it does not know when it rewrites config.toml or settings.json, while
  0.2.0 and later keep them (and report an unknown key as an error in the first place). A 0.1.x copy
  and a 0.2.0 or later copy cannot control each other's instances at all (see
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

## Upgrading from 0.2.0-alpha.1 or 0.2.0-alpha.2

Those alphas keep their runtime records under other names and shapes: the instance record is
`<home>/run/<port>.pid`, the desired-state record `<home>/run/<port>.restore`, the watchdog record
`<home>/run/watchdog.claim`, and their status snapshot has no `start`, `instance_id` or
`foreground`. This version reads only `<port>.instance.json`, `<port>.restore.json` and
`watchdog.json`, and it cannot use an alpha instance's snapshot. So an instance started by one of
those alphas is out of reach for this version's CLI:

- `aproxy status` lists it only among the other aproxy processes, and `aproxy stop <port>` answers
  `端口 <port> 上没有运行中的 aProxy 实例。` ("no running aProxy instance on port"), although the
  process is still serving.
- Neither `aproxy restore` nor the watchdog brings it back after a crash.

Stop the instances **before** installing. An install started while they run stops before it
replaces anything: the alpha's `aproxy install` hands the work to this version, which finds the alpha
instances and fails with
`live instances started by an older version are in this home (pid …); stop them with the aproxy that started them (aproxy stop all), or run aproxy install --force`.
The alpha binary stays in place; follow the steps below and install again.

1. With the alpha CLI: `aproxy status`, note each instance's port, config and one-off flags, then
   `aproxy stop all` and check that `aproxy status` lists nothing.
2. Install this version, then `aproxy start <alias|path>` for each instance. `start` writes the
   current records.
3. Delete what the alpha left in `<home>/run/`: `*.pid`, `*.restore` and `watchdog.claim`. Nothing
   reads them.

The command line changed after alpha.2 as well: `aproxy` alone no longer starts an instance, and the
`config` flags became `config set`/`unset`. See "Commands changed in 0.2.0" below.

If the binary was replaced another way while alpha instances were running (by hand, for example),
the alpha CLI is gone and the new one cannot manage them. `aproxy install --force` still can: it finds every aproxy process of
this home through the process ledger, whose format never changes, and terminates them (requests in
flight are cut off). It cannot restart them, because their desired-state records are in the old
format; start each one again with `aproxy start`.

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
3. Replace the binary through the channel that installed it. Every channel picks the newest
   stable version unless told otherwise, and while 0.2.0 exists only as a pre-release that is
   still 0.1.x, so ask for the pre-release explicitly or you reinstall what you just removed:
   - install script or `aproxy install`: delete `<home>/bin/aproxy` (`aproxy.exe` on Windows, and
     `aproxy.old.exe` if present), then run the install script again with `--pre` (the PowerShell
     script reads `APROXY_PRE=1`); it does not overwrite an existing binary. If Windows refuses the
     delete, the 0.1.x watchdog is still running from the file; it exits by itself, by default 5
     minutes after the last instance stopped.
   - npm: `npm i -g @meowo/aproxy@next` (`@meowo/aproxy-headless@next` for the headless build); once
     0.2.0 is released as a stable version, `@latest`.
   - cargo: `cargo install aproxy --locked --force --version <version>`; once 0.2.0 is released as
     a stable version, `--version` can go. `--locked` builds with the dependency versions tested
     for the release.
   Then run `aproxy --version`: the first line must read `aproxy v0.2…` (0.1.x prints
   `aproxy 0.1.0`).
4. Delete what 0.1.x leaves behind: `<home>/run/install.state` and `<home>/staging/`, which 0.2.0
   never reads, and any `<home>/run/*.restore`. 0.2.0 reads only `<home>/run/*.restore.json`, so the
   old files are ignored rather than acted on; remove them so they do not mislead a later
   inspection. A crashed 0.1.x instance's record is dead too: if that instance matters, start it
   again with the new CLI after the upgrade.
5. Keep `settings.json` and the config files, but convert both: 0.2.0 refuses the 0.1.x shapes, so
   until they are converted most commands stop with exit 3 and no instance starts. Use
   `aproxy migrate` as [Converting the files with aproxy migrate](#converting-the-files-with-aproxy-migrate)
   describes: preview with `aproxy migrate --dry-run`, show the user the planned changes, run
   `aproxy migrate`, then settle with the user whatever it leaves for review. Finish with
   `aproxy doctor`, which must report no errors.
6. Start each instance with the new CLI: `aproxy start <alias or config path>`. Ports are unchanged,
   so agent clients need no change.
7. Refresh these documents with `aproxy install --skills-only`, and check that the agent's skills
   directory links to or copies `<home>/skills/aproxy-cli`.

### Converting the files with aproxy migrate

`aproxy migrate` rewrites 0.1.x files into the 0.2.0 shape using the two rename tables further down
("settings.json keys renamed in 0.2.0" and "Config keys renamed in 0.2.0"). Only 0.2.0 and later
have it, so run it with the new CLI after step 3. It changes only what has one mechanical answer and
leaves alone any file that needs a decision, so it never guesses on the user's behalf.

**Which files.** Without arguments it takes `<home>/settings.json` and every config file the home
knows: alias targets, `default_config`, and the `*.toml` files in each `config_dirs` entry, in
`<home>/configs/` and in `<home>` itself, the same set `aproxy doctor` checks. It reads that list from the 0.1.x settings.json even though 0.2.0 otherwise refuses
the file, so no alias is missed. `aproxy migrate PATH...` converts only the named config files, each
once even when two paths lead to the same file; settings.json is converted only by the form without
paths. Backups, files under `<home>/run/` and files already in the 0.2.0 shape are not rewritten.

**What happens to each file.**

1. The new text is built from the rename tables. A key moves into its section; comments and the
   rest of the file stay as they were. A few values change form: `forward_only = true` becomes
   `delivery = "forward"` and `false` becomes `"retry"`; each `bounded_retry_paths` pattern becomes
   a rule with that `path` and `max_attempts = 3` (what 0.1.x did); `read_timeout_secs` becomes both
   `timeouts.first_byte_secs` and `timeouts.read_idle_secs`; a format's `every_attempt = true` adds
   `"retry"` after `"request"` in its `stages`. settings.json also gains `"schema": 1`.
2. The new text is checked the way `start` would check it, against the settings being converted in
   the same run. If it would still be invalid, the file is not written.
3. The original is copied to `<file>.0.1.bak`, readable only by the user, and then replaced, unless
   someone edited the file in the meantime. If a startup check of the written file fails, the
   original is put back from the backup. An existing `.0.1.bak` with different content is never
   overwritten.

Running it again is safe: converted files report "Already current", and a backup with the same
content is reused.

Settle settings.json first. Each config is checked against the settings it will run with, so while
settings.json still has a point to review, every config file reports "Repair settings.json first so
startup can validate this configuration." and waits.

**What is left for the user.** A file is "Left unchanged; review needed" when any part of it has no
single correct translation. Nothing in such a file is written, not even the keys that could have
moved, so the user sees it exactly as it was. Each point names the original key and line:

| Message | Why | What to do |
|---|---|---|
| on `request_transform`, `response_transform` or `heartbeat_transform`: "Choose the format program, stages and arguments before migrating this file." | A 0.1.x transformer has no rename: 0.2.0 speaks a different protocol, so the program itself has to change. | Replace the table with a `[[format]]` entry whose `command` speaks protocol 1 (an official `aproxy-format` role or the user's rewritten program) and whose `stages` list what it serves; `mode` becomes `process`, `idle_timeout_secs` becomes `worker_idle_secs`, `pool_max` and `timeout_secs` keep their names. The aproxy-format skill covers choosing or writing the program. Or delete the table if the user no longer needs it. Then run migrate again for the remaining keys. |
| on a format's `extra`: the same message | 0.2.0 has no `extra`. | Pass the data through the entry's `args` or `env` and change the program to read it from there. |
| on `defaults.api_key`, a top-level `api_key`, or a secret header in `defaults.extra_headers` / `override_headers`: "Set defaults.base_url first, so it is clear which upstream this credential belongs to." | 0.2.0 sends a global credential only to the upstream it is bound to (settings-json.md, "Credentials in defaults follow their target"). 0.1.0 had no global key; these come from files written for a 0.2.0 pre-release. | Ask the user which upstream the key belongs to and set `defaults.base_url` to it; move a top-level `api_key` to `defaults.api_key`. Or move the key into the config.toml of the instances that use it. |
| "Cannot move this key without overwriting `<new key>`." | The file has both the old and the new key, possibly with different values. | Ask the user which value is right, delete the other key, run migrate again. |
| "Unrecognized key; it is not a documented 0.1.x rename." | Neither version knows the key: a typo, `upstream_url` from an alpha build, or a top-level `api_key` once `defaults.base_url` is set. | Rename `upstream_url` to `base_url`. A key that holds a credential (`api_key`, a token) is never just deleted: ask the user where it belongs and move it there (`defaults.api_key`, or the config of the instance that uses it). Fix or delete anything else (`aproxy config unset KEY --config FILE`, `aproxy settings unset KEY`). |
| "This member occurs more than once; choose which value to keep." | settings.json names one key twice. | Keep one. |
| "This value or file is not valid in the target format." | 0.2.0 rejects the value even after the rename, or the file is not valid TOML or JSON. | Fix what the line points to, using config-toml.md or settings-json.md. |
| "Repair settings.json first so startup can validate this configuration." | A config file waits for settings.json. | Settle settings.json first, then run migrate again. |
| "This alias or default_config points to a file that does not exist; remove the alias or restore the file." | settings.json names a config that is gone. | Ask the user: remove the alias (`aproxy alias remove NAME`) or `default_config`, or put the file back. |
| "This config_dirs entry does not exist; remove it from settings.json or create it." | A configured directory is gone. | Ask the user: remove the entry from `config_dirs` or create the directory. |
| "Cannot reuse the existing .0.1.bak as an identical private backup; neither file was replaced." | An earlier backup with other content is in the way. | Ask the user before moving it aside: it may be the only copy of an earlier original. |

Settle these with the user rather than on your own: each one decides where a credential is sent,
which program sees the user's requests, or which of two values wins. Then run
`aproxy migrate --dry-run` again until nothing is left for review.

**Output.** One block per file: the path and "Would migrate" (with `--dry-run`), "Migrated",
"Already current", "Left unchanged; review needed" or "Failed", then one line per change
(`old → new = value`) or per point to settle (`key:line: message (section)`; a point about the whole
file has no key or line and prints as `: message (section)`). Secrets are masked in the report, not
in the files. The last line reminds that running instances keep the configuration they loaded.

The text follows the user's language, so on a Chinese system the headings and messages are Chinese
(`将迁移`, `未改动，需要人工确认` …). To match messages against the table above, use `--json`: it
prints the same report as one document, always in English, with `files[]` (`path`, `outcome` =
`migrated`, `current`, `needs-review` or `failed`, `changes`, `issues` with `key`, `line`, `message`,
`section`) and a `summary` with the counts.

| Exit | Meaning |
|---|---|
| 0 | every file was converted (or would be, with `--dry-run`) or was already current |
| 5 | some files were converted, others are left for review |
| 3 | nothing was converted and some files are left for review |
| 1 | a file operation failed on at least one file (permission denied, disk full and the like); takes precedence over 5 and 3 |

A file edited by someone else while migrate ran, or one whose written text failed the startup check
and was put back from its backup, is left for review (exit 5 or 3), not a failure: run migrate again.

**What it leaves alone.** Running instances, until they restart; desired-state records; the 0.1.x
leftovers of step 4; the programs that format entries run.

Elsewhere 0.2.0 points at migrate when it meets a 0.1.x file. Most commands end their error list
with ``Preview the 0.1.x changes with `aproxy migrate --dry-run`, then run `aproxy migrate`.``,
and `doctor` adds that line as an `[INFO]` finding with the code `config_legacy_shape`. When `start`
was given the file as an alias or path, or `config show` got `--config`, the line names the file:
``… `aproxy migrate --dry-run <file>`, then run `aproxy migrate <file>`.`` On a 0.1.x home
settings.json is almost always old too, so expect the short form first. The hint appears only when
every error in the file comes from the 0.1.x shape, so a plain typo does not send the user here.

A session on a 0.1.x home with one plain config and one that uses a transformer:

```text
$ aproxy migrate --dry-run
<home>/settings.json: Would migrate
  max_body_mb → defaults.max_request_mb = 64
  forward_only → defaults.delivery = "retry"
  bounded_retry_paths → defaults.retry.rules = [{"path":"/v1/messages/count_tokens","max_attempts":3}]
  watchdog → watchdog.enabled = true
  download_proxy → update.download_proxy = "http://***@127.0.0.1:7890"
  — → schema = 1
<home>/relay.toml: Left unchanged; review needed
  request_transform:5: Choose the format program, stages and arguments before migrating this file. (compatibility.md#config-keys-renamed-in-020)
<home>/work.toml: Would migrate
  max_body_mb → max_request_mb = 32
  read_timeout_secs → timeouts.first_byte_secs = 600
  read_timeout_secs → timeouts.read_idle_secs = 600
Running instances keep the configuration they loaded; restart them to use the migrated files.
$ aproxy migrate               # exit 5: settings.json and work.toml converted
  ... replace [request_transform] in relay.toml with the [[format]] entry the user chose ...
$ aproxy migrate relay.toml    # exit 0: forward_only → delivery = "forward"
$ aproxy doctor                # no errors
```

Windows prints the paths in their long form, beginning with `\\?\`.

### Commands changed in 0.2.0

The command line was reorganized. 0.2.0-alpha.1 and alpha.2 still had the 0.1.x command line apart
from the `install` options in the last row, so this table applies to them as well. A bare `aproxy` no
longer starts an instance: it prints the help and exits 2.

| 0.1.x | Now |
|---|---|
| `aproxy`, `aproxy --config <file>` | `aproxy start`, `aproxy start <file>` (`aproxy start --config <file>` works too) |
| `aproxy --foreground` | `aproxy start --foreground` |
| `aproxy --baseurl <URL>`, `--listen <addr>` | `aproxy start --base-url <URL>`, `--listen-addr <addr>` |
| top-level `--proxy`, `--api-key`, `--log-file`, `--config` | options of `start`, written after it; `--api-key -` reads one line from stdin |
| `aproxy stop idle 600`, `aproxy restart idle 600` | `aproxy stop idle --idle-threshold-secs 600` (`restart` likewise) |
| `aproxy logs`, which follows until interrupted | `aproxy logs` prints the last 200 lines and exits; `-n <lines>` sets the count, `-f` follows |
| `aproxy config`, `aproxy config --show` | `aproxy config show` (the values the file sets), `aproxy config show --effective` (effective values and where each comes from) |
| `aproxy config --baseurl <URL>`, `--listen`, `--api-key`, `--proxy`, `--proxy-username`, `--proxy-password`, `--keepalive-secs <s>` | `aproxy config set <key> <value>` with the keys `base_url`, `listen_addr`, `api_key`, `proxy`, `proxy_username`, `proxy_password`, `keepalive.interval_secs`; one key per command |
| `aproxy config --extra-header k=v`, `--override-header k=v` | `aproxy config set extra_headers.k v`, `aproxy config set override_headers.k v` |
| `aproxy config --clear-api-key`, `--clear-headers`, `--clear-proxy` | `aproxy config unset <key>` |
| `aproxy config --set-default <file>`, `--clear-default` | `aproxy settings set default_config <file>`, `aproxy settings unset default_config` |
| `aproxy --config <file> config …` | `aproxy config --config <alias or file> …`, before or after the subcommand |
| `aproxy install --variant v3\|baseline` | `aproxy install --cpu x86-64-v3\|baseline`; `--variant` now picks `desktop` or `headless` |

Exit codes now follow one table in every command (commands.md, "Output and exit codes"): a script
that treated any nonzero exit as failure should know 3 (invalid configuration), 4 (not found) and 5
(partly done), and that stopping an instance that is not running exits 0. Log lines no longer appear on
stdout, so a script can read the output of `--json` directly.

### settings.json keys renamed in 0.2.0

`aproxy migrate` applies this table; edit by hand only what it leaves for review. A flat 0.1.x
file is rejected field by field: every old top-level key is reported as `config_unknown_key`, `watchdog: true|false` as
`config_type` ("watchdog must be an object"), and commands that need settings stop with exit 3
(settings-json.md, "When settings.json is invalid"). `aproxy settings show`, `aproxy doctor` and the
other tolerated commands still run, so you can inspect the file and repair it one key at a time;
`aproxy settings unset` accepts the old keys even though this version does not know them.

Rewrite each old key with the table below. Keep `aliases`, `default_config` and `config_dirs` where
they are: their names and shapes did not change.

| 0.1.x settings.json | 0.2.0 |
|---|---|
| `max_body_mb = N` | `defaults.max_request_mb = N` (`0` = no limit) |
| `disk_cache = true\|false` | `defaults.disk_cache` |
| `forward_only = true\|false` | `defaults.delivery = "forward"\|"retry"` |
| `bounded_retry_paths = ["..."]` | `defaults.retry.rules`: one entry per pattern, `{ "path": "<pattern>", "max_attempts": 3 }` (0.1.x stopped after three attempts, so keep 3 to preserve behavior) |
| `allowed_hosts = [...]` | `defaults.allowed_hosts` |
| `allowed_origins = [...]` | `defaults.allowed_origins` |
| `keepalive_trigger = "..."` | `defaults.keepalive.trigger` |
| `watchdog = true\|false` | `watchdog.enabled` (the other watchdog fields move into the same object) |
| `watchdog_heartbeat_secs = N` | `watchdog.heartbeat_secs` |
| `watchdog_stale_after_cycles = N` | `watchdog.stale_after_cycles` |
| `watchdog_max_restarts = N` | `watchdog.max_restarts` |
| `watchdog_idle_exit_secs = N` | `watchdog.idle_exit_secs` |
| `log_rotate_mb = N` | `logs.rotate_mb` |
| `idle_timeout_secs = N` | `instances.idle_threshold_secs` |
| `download_chain = [...]` | `update.download_chain` |
| `download_proxy = "..."` | `update.download_proxy` |
| `skill_auto_update = true\|false` | `update.skill_auto_update` |

Two behaviors differ from a pure rename:

- **A global `api_key` now needs a target.** `defaults.api_key` (and with it any secret header in
  `defaults.extra_headers`) is an error without `defaults.base_url`, because the key would otherwise
  be inherited by every instance and sent to every upstream. Set `defaults.base_url` first. After
  that, only instances whose own `base_url` names the same target inherit the key; an instance that
  points elsewhere keeps working but sends no key unless its own config.toml sets one (settings-json.md,
  "Credentials in defaults follow their target").
- **0.1.x had no `language` or `ui`.** Both are optional; leave them out and take the built-in
  defaults (`"system"` and `{}`).

Convert the whole file before starting 0.2.0: a mixture of old and new keys is still invalid for the
new CLI (the old keys are unknown), and 0.1.x cannot read the new sections either. To edit by hand
with the 0.2.0 CLI, for example:

```sh
aproxy settings unset max_body_mb
aproxy settings set defaults.max_request_mb 128
aproxy settings unset forward_only
aproxy settings set defaults.delivery '"retry"'
aproxy settings unset watchdog
aproxy settings set watchdog.enabled false
```

`settings unset` removes an old key even though this version does not know it; `settings set` rejects
a key this version does not know. A change that would introduce a new error is refused, so do the
`watchdog` object in the order shown (remove the old boolean, then set the section). `settings show`
names whatever is still wrong, and `doctor` lists it with the rest of the home's problems.

### Config keys renamed in 0.2.0

config.toml is parsed strictly from 0.2.0 on. An old key is not ignored: it is reported as
`config_unknown_key`, naming the file, the key path and line:column, and the instance does not
start. Convert each file before the first start with the new CLI. `aproxy migrate` applies every
row except the transformer tables and format `extra`, which it leaves for the user.

| 0.1.x config.toml | 0.2.0 |
|---|---|
| `max_body_mb = N` | `max_request_mb = N` (`0` = no limit) |
| `spool_limit_mb = N` | `max_response_mb = N` |
| `forward_only = true\|false` | `delivery = "forward"\|"retry"` (default `"retry"`) |
| `connect_timeout_secs = N` | `[timeouts] connect_secs = N` |
| `read_timeout_secs = N` | `[timeouts] first_byte_secs = N` for the wait for response headers, and `[timeouts] read_idle_secs = N` for the gap between body reads; both default 300, and they now report `upstream_first_byte_timeout` and `upstream_read_idle_timeout` separately |
| `keepalive_interval_secs = N` | `[keepalive] interval_secs = N` |
| `keepalive_trigger = "..."` | `[keepalive] trigger = "..."`; `"always"` is rejected in the file (only a format may ask for it per request) |
| `keepalive_heartbeat = "..."` | `[keepalive] heartbeat = "..."` |
| `max_retry_backoff_secs = N` | `[retry] max_backoff_secs = N` |
| `bounded_retry_paths = ["..."]` | `[[retry.rules]]` entries with `path` (regex) and `max_attempts` (required; `0` = no limit), plus optional `status` |
| `[request_transform]`, `[response_transform]`, `[heartbeat_transform]` tables | `[[format]]` array entries with `command`, `args`, `process`, and `stages` (config-toml.md, format); `pool_max` and `timeout_secs` keep their names. The program itself must speak protocol 1; the official `aproxy-format run` command is replaced by one subcommand per role |
| format `mode = "spawn"\|"persistent"` | `process = "spawn"\|"persistent"` |
| format `idle_timeout_secs = N` | `worker_idle_secs = N` |
| format `every_attempt = true` | add `"retry"` to `stages` next to `"request"` |
| format `extra = "..."` | pass the data through `args` or `env` |

The 0.1.x `upstream_url` name is older still and also no longer read; it is `base_url` in 0.2.0 and
in 0.1.0's stable line (the "What these docs describe" table above).

To edit by hand, changing one key at a time with `aproxy config set KEY VALUE` and
`aproxy config unset OLD` is safe: an unknown key does not block unrelated edits, and `unset`
accepts any key, including one this version does not know. After the renames, `aproxy config show`
prints the file's values and exits 3 if anything is still wrong.

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

Config files from alpha builds need the same renames as 0.1.x (see "Config keys renamed in 0.2.0",
and rename `upstream_url` to `base_url` by hand), and their flat settings.json is refused the same
way ("settings.json keys renamed in 0.2.0"). `aproxy migrate` converts both, as for 0.1.x. From 0.2.0 on, a key the version does not know is an error
that stops the instance; it is no longer ignored. Missing keys still take their defaults. If
something stops working after the upgrade, look the symptom up in troubleshooting.md.
