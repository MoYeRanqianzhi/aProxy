# aproxy command reference

Every `aproxy` subcommand: syntax, options, how targets are resolved, what the output means and the
exit codes. Read it when you need the exact form of a command or have to interpret what one printed.
What happens inside a running instance is in behaviors.md; symptom-driven fixes are in
troubleshooting.md.

## Contents

- [Before you run commands](#before-you-run-commands): home directory, isolated experiments,
  targets, output, exit codes, processes
- [Options for start](#options-for-start)
- [start](#start) · [status](#status) · [stop](#stop) · [restart](#restart) · [logs](#logs) ·
  [restore](#restore)
- [alias](#alias) · [find](#find) · [doctor](#doctor) · [config](#config) · [settings](#settings) ·
  [migrate](#migrate): converting 0.1.x files
- [desktop](#desktop): the desktop window
- [install / upgrade](#install--upgrade), including recovery from a failed install and
  [force install](#force-install)

## Before you run commands

### Home directory

aProxy keeps everything under one home directory, written `<home>` below: the value of
`APROXY_HOME` when it is set to anything but blanks, otherwise `~/.aproxy` (`~` is `USERPROFILE`
on Windows and `HOME` elsewhere, the same directories the install scripts use). `APROXY_HOME` is
used exactly as written, so it must be an absolute path: with a relative one every command except
`--version` and `--help` stops with an error instead of guessing a directory from the working
directory, and `install --force` ends with 10.

The home is private to its owner, because it holds API keys in plain text, logs and cached
request bodies: aProxy creates it with mode `0700` on Unix and, on Windows, an ACL granting only
the current user, SYSTEM and Administrators. A home the current user owns that other users can
also access is tightened the next time a command writes into it; read-only commands such as
`status` leave it alone, and so does everything when the home belongs to someone else. `doctor`
reports a home that other users can access.

| Path | Holds |
|---|---|
| `<home>/config.toml` | the default instance config |
| `<home>/configs/` | the conventional place for further instance configs (`find` and `doctor` scan it) |
| `<home>/settings.json` | aliases, the default config, the `defaults` global defaults, watchdog and install settings (settings-json.md) |
| `<home>/run/` | instance records `<port>.instance.json`, desired-state records `<port>.restore.json`, the watchdog record `watchdog.json`; while an instance starts, a short-lived `<nanoseconds>-<pid>.spec.json` launch-spec handoff file. `APROXY_HOME` is the only root; the run directory cannot be relocated separately |
| `<home>/logs/` | instance logs (random file names), `startup.log`, and `watchdog.log` (what the watchdog did and why) |
| `<home>/bin/` | the binary managed by `aproxy install`; on Windows also `aproxy.bat` (see install / upgrade). Files named `aproxy.old-…` and `aproxy.tmp-…` are left by binary swaps; installs delete the ones they can |
| `<home>/install/` | `aproxy install`'s own files: the progress record `state.json` while an install runs, downloads and staged binaries in `staging/<version>/`, the install script's downloads in `bootstrap/`, force install's download in `force/`, on Linux and macOS the install announcement `announce`, and `lock`, held while `bin/` changes |
| `<home>/spool/<pid>-<hash>/` | one instance process's disk-cache scratch files (behaviors.md, Disk cache); removed when the instance stops, and after a crash by the next instance that starts or by the watchdog |
| `<home>/ledger/` | one `<pid>.json` per running aproxy process (instances, the watchdog, installers, the desktop window), written at start and deleted at exit. `aproxy install --force` finds the processes to stop here. Leave these files alone |
| `<home>/desktop/` | the desktop window's own files: its web view data and its preferences (see desktop) |
| `<home>/skills/` | skill documents placed by `aproxy install` |

### Experiment in an isolated home

To test something without touching the user's instances, give the experiment its own home and a
port that no other instance uses:

```sh
# POSIX shell; keep these in one command line if each tool call gets a fresh shell
export APROXY_HOME="$(mktemp -d)"
aproxy config set base_url https://api.example.com
aproxy config set listen_addr 127.0.0.1:18080
aproxy start && aproxy status
aproxy stop 18080
```

In PowerShell, set `$env:APROXY_HOME = "$env:TEMP\aproxy-test"` first. A separate home gives the
experiment its own configs, settings, registry, logs, watchdog and control channel: `status`,
`stop`, `restart` and `logs` under it see only instances started under it, so `aproxy stop 12345`
there cannot stop the user's instance on 12345.

**The port is still shared.** Ports belong to the whole machine, so a test instance on a port
another instance listens on fails to start with `端口 <port> 被其他程序占用` ("port in use by
another program"). Pick a port nothing uses.

With a 0.1.0 binary the control channel is not isolated on Windows: its pipe is named after the
port alone, so under a test home `stop 12345`, `restart 12345` and `logs 12345` reach the user's
instance on 12345. Address test instances only by their own port there.

### Targets

`stop`, `restart` and `logs` act on one target; `start` takes an alias or a config path.

| Target | Meaning | Accepted by |
|---|---|---|
| none | the only running instance; refused with a list of ports when several run | stop, restart, logs (`start` uses the default config) |
| `PORT` | the instance answering on that port, located over IPC without the registry | stop, restart, logs |
| `ALIAS` | the running instance whose config file is the alias's file, whatever port it now uses | start, stop, restart, logs |
| `default` | the default config: `default_config` in settings.json, else `<home>/config.toml` (any case; the typo `defult` also works) | start, stop, restart, logs |
| `PATH` | a config file; `~` is expanded and the path made absolute | start; stop, restart and logs match it against running instances |
| `all` | every instance in the registry (lowercase only) | stop, restart |
| `idle [SECS]` | instances whose last client request is at least SECS old (default `instances.idle_threshold_secs`, 1800) | stop, restart |

A number is always a port. Anything else is tried as an alias, then as `default`, then as a file
path. Alias names may not be numbers or reserved words, so these forms never collide. When two
running instances share one config file (started with different listen addresses), an alias or path
reaches only one of them; use ports.

### Output and exit codes

- Messages are in Chinese; this file quotes them verbatim with an English gloss. Results go to
  stdout, errors to stderr. The Windows console is switched to UTF-8 at startup.
- Log lines go to stderr too, warnings and errors only by default; set `RUST_LOG=info` (or
  `debug`) to see more. stdout carries only results, so `--json` output stays one parseable
  document whatever `RUST_LOG` says. The exception is `start --foreground`, whose instance writes
  its log to stdout.
- Secrets are never printed. A redacted secret prints as `<secret>` in text, or as
  `<secret sk-a…f3a9>` when it is at least 24 characters long (first and last four characters);
  `--json` prints `{"secret":true}` with a `"hint"` field in the same form. URLs lose their
  userinfo (`***@`) and query values (`?key=***`).
- Every command checks `<home>/settings.json` at startup, before doing anything else. When it is
  invalid, `settings …` and `doctor` work normally, `status`, `stop` and `install`/`upgrade` run on
  built-in defaults with `[WARN]` lines (aliases unavailable), and every other command prints the
  errors as `[ERROR]` lines with repair hints and exits 3. settings-json.md, "When settings.json is
  invalid", lists which command falls in which group.
- Argument errors exit 2. `aproxy <command> --help` prints the options (in Chinese). A bare
  `aproxy` with no command prints the help and exits 2; write `aproxy start` to start an instance.

Exit codes mean the same thing in every command: 0 done (or the requested state already holds),
1 failed, 2 usage error, 3 invalid configuration, 4 target or file not found, 5 partly done. The
table gives each command's cases, assuming a valid settings.json; the startup check above can end
a command with exit 3 before it runs (except the tolerated commands). Decide from the exit code and
the `--json` codes, not from the Chinese text.

| Command | 0 | 1 | 2 | 3 | 4 | 5 |
|---|---|---|---|---|---|---|
| start | started, or already running on that port | port taken, could not start, not ready in 30 s | usage | the config has errors | alias or config file not found | — |
| stop | stopped; the target was not running (a leftover record is deleted); `idle` with nothing idle | not confirmed stopped, `--force` failed, hung without `--force` | several instances and no target | — | unknown alias | some stopped, some not |
| restart | restarted; `all` or `idle` with nothing to restart | failed; foreground instance | several instances and no target | the pre-check found the config invalid | target not running, or not found | some restarted, some not |
| logs | lines printed | foreground instance (no log file) | several instances and no target; `all` | — | not running and no record to read; unknown alias | — |
| restore | everything restored, or nothing to restore | every record failed | — | — | — | some failed, or a config file is missing |
| status, find | always | — | — | — | — | — |
| alias | success | invalid name, missing file, unknown alias, write failure | — | — | — | — |
| doctor | no errors (warnings and infos do not count) | — | — | at least one error | — | — |
| config | the values printed, or the change saved | write conflict or I/O failure | a malformed key path, or a value that is not TOML | the result would be invalid, an unknown key, or the file has errors (`show` still prints, then exits 3) | the file does not exist (`get`, or `show` with an explicit `--config`) | — |
| settings | show/get printed; set/unset stored or unchanged | write failure | a malformed key path, or a value that is not JSON | the file cannot be read, or the change would make it invalid | — | — |
| desktop | the window was closed; a window is already open for this home; on Windows from a terminal, the window was opened | headless build; no web view runtime or no display; the window failed to start | — | — | — | — |
| install | installed; already latest; channel empty; nothing to abort; `--check` answered | failed, or refused before changing anything (instances it cannot restart) | usage | the precheck before the rolling restart failed | the requested version does not exist | — |

**`--json`** (on `start`, `stop`, `restart`, `restore`, `status`, `find`, `doctor`, `alias list`,
`install --check`, and the `config` and `settings` subcommands) prints exactly
one JSON document on stdout, starting with `"schema":1`; the human-readable lines move to stderr.
When the whole command fails, the document is
`{"schema":1,"error":{"code","message","details"}}`; `code` names the cause, for example
`target_not_found`, `target_not_running`, `target_ambiguous`, `config_invalid`, `port_in_use`,
`ready_timeout`, `start_failed`, `usage`. `stop`, `restart` and `restore` report one item per
instance:

```json
{"schema":1,"results":[{"target":"work","port":12346,"outcome":"stopped"}],"summary":{"ok":1,"failed":0}}
```

`target` is the target as written (`null` when none was given) and `port` the instance's port
(after `restart` or `restore`, the port the new instance listens on). `outcome` is one of `stopped`,
`already_stopped`, `killed`, `skipped` (with `"reason":"not_idle"`), `failed` for `stop`;
`restarted`, `failed` for `restart`; `restored`, `already_running`, `config_missing`, `failed` for
`restore`. A `failed` item carries an `error` object shaped like the failure document's (codes such
as `instance_unresponsive`, `stop_failed`, `restart_unsupported`, `config_invalid`).
An install with `--json` prints one JSON object per line instead (install / upgrade, "Exit codes
and `--json`").

`install --force` exits 0, 10, 11 or 12 instead; see install / upgrade, Force install.

### Processes you may see

Instances, the watchdog and background install steps all run the same executable, which is why
killing `aproxy` by image name takes everything down at once. Tell them apart by command line:

| Command line contains | Process |
|---|---|
| `__serve` | a background instance, started as `aproxy __serve --home <dir> --spec <file>`; `aproxy status` maps its pid to a port |
| `__watchdog` | the watchdog that restarts crashed or hung instances, started as `aproxy __watchdog --home <dir>` (behaviors.md, "Crash recovery and the watchdog") |
| `install --continue` | a background step of an install that is finishing or resuming |
| `install --force` | a force install in progress; it stops every other aproxy process of its home |
| `desktop` | the desktop window; it holds no port and closes with its window |

How an instance starts is written to a launch-spec file under `<home>/run/`, not to its command
line, so `--api-key` and other overrides never appear in the instance's process list entry. The
handoff file is deleted once the instance is ready or has failed; the instance's own copy lives in
its desired-state record (owner-only permissions, like the handoff).

`__serve`, `__watchdog` and `install --continue` are internal; never pass them yourself. Another
aProxy version may start this one through `__serve`, so the names and their two arguments are
fixed across versions.

## Options for start

Only `start` takes start options; each other command lists its own options with it. A bare
`aproxy` prints the help and exits 2, so write `aproxy start`.

| Option | Effect |
|---|---|
| `ALIAS\|PATH` (positional) | the config to start: an alias, a config path, or `default`; omitted, the default config |
| `--config PATH` | the config to start, equivalent to the positional path; force install prints this form in its recovery command |
| `--base-url URL` | overrides `base_url` for the instance being started |
| `--listen-addr ADDR` | overrides `listen_addr` |
| `--proxy URL` | overrides `proxy` |
| `--api-key KEY\|-` | overrides `api_key`; `-` reads one line from stdin |
| `--log-file PATH` | log file for the instance being started; overrides `log_file` |
| `--foreground` | run in this console instead of the background |
| `-h, --help` / `-V, --version` | help / print `aproxy v<version> (<variant>; <target>[; x86-64-v3])` (compatibility.md) |

- The positional target, `--config`, `--foreground` and the overrides all belong to `start`.
- The override options matter only when starting. They are not written to config.toml, but they
  become part of the instance's launch spec (in its desired-state record), so `restart`, `restore`,
  the watchdog and `install` reuse them. `config show` and `config show --effective` do not show
  them; `status` shows the effective listen address and upstream.
- `--api-key` does not appear on the instance's or the watchdog's command line: it travels in the launch-spec file and
  the desired-state record, both readable only by the current user. It is still in the shell history
  and briefly on the `aproxy start` command line, and it is stored in plain text in the record, so a
  backup or a copied home contains it; prefer config.toml for long-lived keys, and `--api-key -`
  when the key must not appear on the command line at all.
- The proxy environment (`HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY`, `NO_PROXY` and their lowercase
  forms on Unix) and `PATH` of the shell that runs `start` are captured in the launch spec.
  `restart`, `restore` and watchdog respawns therefore reach the network the same way and find the
  same format programs, even when they run from another shell. Change the environment by stopping
  and starting the instance again.
- `start` expands `~` and turns a relative config path into an absolute one against the current
  directory; the instance records that absolute path. `--log-file` expands `~` and resolves a
  relative path against `<home>`, like `log_file`.

**Which config file a command uses:** `start` uses its positional target, else `--config`, else
`default_config` from settings.json, else `<home>/config.toml`. `config …` uses its own `--config`
(an alias or a path), else `default_config`, else `<home>/config.toml`. If `default_config` points
to a file that no longer exists, the commands that need the config exit 4 with
`settings.json 指定的默认配置文件不存在: <path>` ("the default config named in settings.json does
not exist"): `aproxy start` without a target and `aproxy config …` without `--config`. Every other
command is unaffected; repair the setting with `aproxy settings set default_config PATH` or
`aproxy settings unset default_config`.

## start

```
aproxy start [OPTIONS] [ALIAS|PATH] [--json]   # start a config; the default config when no target is given
aproxy start --config PATH [OPTIONS] [--json]  # the same, with the path as an option
aproxy start --foreground [ALIAS|PATH]
```

An unknown alias or a config file that does not exist exits 4. With `--json` the success document
is `{"schema":1,"outcome":"started"|"already_running","port","pid","log_path","instance"}`, where
`instance` is the instance's status as `status` reports it. `--json` cannot be combined with
`--foreground`, which has no moment at which the start is complete.

What `start` checks, in order:

1. **Config.** The file is loaded strictly; a TOML syntax error, an unknown key (including every
   old 0.1.x name), an invalid value or a `listen_addr` without a valid port stops here with exit
   3, printing `配置错误: …` ("config error") followed by one `file:line:col: key: message [code]`
   line per problem; the codes are listed in config-toml.md, "Checks at start". A fresh install has no `base_url`, so
   run `aproxy config set base_url URL` and `aproxy config set api_key KEY` before the first start.
   Field rules: config-toml.md. When `start` is given neither `--config` nor a positional target and
   `<home>/config.toml` does not exist, the instance starts with no config file: the override flags
   and settings.json `defaults` supply everything, and `base_url` must come from one of them. Such
   an instance can still be restarted and restored.
2. **Same port.** If an aProxy instance already answers on the port with the same listen address,
   `start` prints `此端口已有 aProxy 在运行，无需重复启动：` ("already running on this port") and
   exits 0, so repeating `start` is safe. With a different listen address it refuses
   (`…两者不能并存。`, "the two cannot coexist"): instances are identified by port alone.
3. **Bind.** A trial bind distinguishes a port taken by another program from one the system
   reserves; the messages and fixes are in troubleshooting.md.
4. **Launch.** The instance is started from a launch spec and detaches from the launching terminal
   (on Unix it gets its own session, so Ctrl+C in that terminal does not reach it; on Windows it
   tries to leave the launcher's job object, and startup.log warns when it cannot). `start` waits
   up to 30 s for it to answer over IPC; an early exit is reported at once. Success prints pid,
   listen address, upstream, config and `日志: <path>` ("log"). A timeout prints
   `新实例（pid <pid>）未在预期时间内就绪（进程存活）` ("new instance not ready in time"; the parenthesis
   says `进程状态不可判` when liveness cannot be determined) and the reason is usually in
   `<home>/logs/startup.log`.

- A listen port of `0` lets the OS choose. `start` waits for the instance and reports the actual
  address; that port is pinned in the instance's desired-state record, so `restart`, `restore` and
  a watchdog respawn return to it while the config still says `0`. Once the config names a fixed
  port, the config wins and the pinned port is discarded.
- A non-loopback listen address prints `警告: 监听地址 <addr> 不是回环地址…` ("not a loopback
  address") but does not block the start.
- After a successful background start, aProxy makes sure a watchdog is running when
  `watchdog.enabled` is true in settings.json (the default).
- `--foreground` keeps the instance in the console: logs print there and Ctrl+C stops it. It is
  still registered, so `status` and `stop` work, but it has no desired-state record: closing the
  console ends it for good, `aproxy restore` does not bring it back, and `aproxy restart` refuses
  it with `…没有可用来重新拉起它的记录（前台运行的实例要在它自己的终端里重启）`. Start it again
  to bring it back.

## status

```
aproxy status [--idle | --busy] [--json]
```

Takes a read-only inventory of the home: every instance in the registry, desired-state records of
instances that are not running, records this version cannot read, the watchdog, and other aProxy
processes in the process ledger. A record whose process is gone (or whose pid now belongs to
another process) is not an instance. `status` deletes nothing: stale records, leftover launch-spec
files and old unreferenced logs are cleaned in the background by an instance after it starts
serving and by the watchdog (behaviors.md, Logs). The text output has these blocks, each printed
only when it has entries, in this order:

- `无响应的 aProxy 实例 (<n>)——进程仍在，但不应答控制通道:` ("unresponsive instances: the
  process is still there but does not answer the control channel"), with port, pid, version, config
  and the hint `aproxy stop <port> --force`. Such an instance is almost always hung; with the
  watchdog on, its hang detection terminates and respawns it on its own. This is the only state
  where `--force` is the right answer.
- `运行中的 aProxy 实例 (<n>):` ("running instances"), described below.
- `无法采信应答的 aProxy 实例 (<n>)——…` ("instances whose answer cannot be trusted": a different
  protocol version or an identity mismatch), with port, pid, version and a reason code. Operate such
  an instance with the aproxy version it runs; do not `--force` it.
- `等待恢复的实例 (<n>)——…` ("instances waiting to be restored"): not running, but their
  desired-state record is kept, so `aproxy restore` or the watchdog will start them again.
  `（配置文件找不到）` marks one whose config file is missing; restoring it fails until the file is
  back. `aproxy stop <port>` drops the record when the instance is no longer wanted.
- `读不懂的记录 (<n>)，原样保留…` ("unreadable records, kept as they are"): files in `run/` this
  version cannot parse, usually written by a newer aproxy; handle them with that version. While
  they exist, `日志清理已暂停` ("log cleanup is paused") is printed too.
- `看护者: pid <pid>  v<version>  最近扫描 <age> 前` ("watchdog, last scan <age> ago"), or
  `已 <age> 没有扫描（可能卡住了）` ("no scan for <age>; it may be stuck").
- `台账里的其他 aProxy 进程 (<n>):` ("other aProxy processes in the ledger"): pid, role and version
  of processes that belong to no instance or watchdog, such as the desktop window or instances
  started by a version before 0.2.0.

With nothing in any block, it prints `没有运行中的 aProxy 实例。` ("no running instances").

Per running instance: port, pid, `v<version>`, uptime, idle time, listen address, masked
upstream, config path (`（没有配置文件）` when it was started without one), and the line
`请求 <n>  在途 <n>  重试 <n>  救回 <n>  最近错误: …`: requests received, requests still in flight
(waiting for the upstream, backing off, retrying or still being delivered), upstream retries,
requests that succeeded only after a retry ("rescued"), and the latest error as
`"<code>: <English message>"（<age> 前）` or `无` ("none"). Extra lines can appear:

- `正在启动（还没开始接收请求）` ("starting; not accepting requests yet").

- `二进制更换中（install 滚动重启阶段，请勿手动干预此实例）` ("binary swap in progress; do not
  intervene"): `aproxy install` is about to restart this instance. Leave it alone.
- `正在退出` ("shutting down"): it received a stop and is finishing open requests (up to 10 s).
- `状态未知（实例比当前 CLI 新，用同版本的 aproxy 查看）` ("unknown state; the instance is newer
  than this CLI"): run the same version of aproxy as the instance.
- `注意: 实例版本 v<x> 与当前 CLI v<y> 不同…` ("instance version differs from this CLI"):
  see compatibility.md.

`--idle` shows only running instances idle for at least `instances.idle_threshold_secs`
(settings.json, default 1800); `--busy` the running ones that are not. An instance that has served
no request yet is idle from its start time. The filters apply to running instances only: the other
blocks are always shown, because each of them needs attention whatever the filter. `status` does not
print log paths; see [logs](#logs).

`--json` prints the same inventory as one document, which is the form to parse:

```json
{"schema":1,
 "instances":[{"port":12346,"liveness":"live","instance":{"pid":4321,…},"status":{"state":"serving",…},"restore_pending":false}],
 "unreadable":[{"file":"run/12349.instance.json","reason":{"code":"record_schema_too_new","details":{…}}}],
 "watchdog":{"pid":5555,"version":"0.2.0","state":"running","last_scan_at":1760190123456,…},
 "processes":[{"pid":7777,"role":"desktop","version":"0.2.0",…}]}
```

`liveness` is `live`, `unresponsive`, `incompatible`, `down`, or `unknown` for a state this version
does not know. A `live` entry carries `status` (state and the request counters); an `incompatible`
one carries `reason` (`code`, `details`); a `down` one has `instance: null`, `restore_pending: true`
and, when the config file is missing, `restore_reason: "config_missing"`. `gc_paused` appears only
while log cleanup is paused; `watchdog` is `null` without a watchdog, and its `state` is `running` or
`stale`. `--idle`/`--busy` filter only the `live` entries here as well.

## stop

```
aproxy stop [PORT|ALIAS|PATH|default|all] [--force] [--json]
aproxy stop idle [--idle-threshold-secs SECS] [--force] [--json]
```

Before stopping anything, run `aproxy status` and name the instance. Your own client may be
talking to its model through one of these instances (check `ANTHROPIC_BASE_URL` or your provider's
`base_url`); stopping that one leaves you unable to reach your model to finish the task or report
back, and drops every other session using it. Stop instances the user did not ask about only after
asking.

- With several instances and no target, `stop` refuses with
  `有 <n> 个实例在运行，必须指定端口号、别名或 all：` ("n instances running; give a port, alias or
  all"), followed by one `aproxy stop <port>` line per instance.
- **Graceful stop (default):** the instance stops accepting work, gives in-flight requests up to
  10 s and then drops them, removes its registry entry and its desired-state record, and exits.
  `stop` confirms within 12 s: `已停止 pid <pid>（端口 <port>）` ("stopped"). If it prints
  `pid <pid> 已收到停止请求但尚未退出，可用 aproxy status 稍后确认，或 aproxy stop <port> --force 强制结束`
  ("received the stop request but has not exited; check status later or force it"), check
  `aproxy status` again shortly; if it is still listed, use `--force`. `stop` exits 1 when a
  target was not confirmed stopped (5 when other targets of the same command did stop), so scripts
  can rely on the exit code.
- **A target that already crashed** still has its desired-state record, which `restore` and the
  watchdog would use to bring it back. `stop` on that target (port, alias or config path) deletes
  the record and prints
  `端口 <port> 的实例已不在运行；已删除它的期望状态记录，restore 与看护者不会再拉起它。` ("the
  instance is not running; its desired-state record was deleted, so restore and the watchdog will
  not bring it back"). By alias or path the message starts `配置 <path> 的实例已不在运行；…`.
- **A hung instance** (process alive, control channel silent) cannot be stopped gracefully.
  Naming it by port or alias prints
  `端口 <port> 的实例（pid <pid>）进程仍在，但不应答控制通道，无法优雅停止；用 --force 强制结束`
  ("the process is still there but does not answer; use --force") and exits 1. `stop all`
  without `--force` stops the answering instances, reports each hung one the same way, and exits 5.
- **`--force`:** terminates the process immediately, without the 10 s grace, after verifying by
  pid plus process start time that the pid still belongs to that instance (a reused pid is
  refused). It deletes the desired-state record before terminating, so the watchdog does not race
  it and neither the watchdog nor `restore` revives the instance. It finds hung instances through
  their registry record, so it works when IPC does not; `stop all --force` includes them.
- **`stop all`** stops every registered instance, including any your own session or the user's
  other sessions depend on. Use it only when the user asked for exactly that. It also deletes the
  desired-state records of instances that are already gone
  (`已删除不在运行的实例留下的期望状态记录（端口 …）。`) and leaves records it cannot read in
  place, saying which and why.
- **`stop idle --idle-threshold-secs SECS`** stops instances idle for at least SECS (default
  `instances.idle_threshold_secs`). `--idle-threshold-secs` is valid only with the `idle` target; on
  any other target it is a usage error (exit 2). A small SECS can include instances in active
  use. Prints `没有闲置超过 <n> 秒的实例。` ("no
  instance idle longer than n s") when none qualify. The instance itself re-checks its idle time
  when the stop arrives: one that received a request after it was selected is left running, with
  `pid <pid>（端口 <port>）刚才又收到了请求（已闲置 <n> 秒），不再算闲置，未停止` ("just received a
  request; no longer idle; not stopped"), and the command still exits 0. `--force` has no such
  re-check.
- An alias whose config is not running: `别名 <name>（配置 <path>）当前未在运行。`; a port with no
  instance: `端口 <port> 上没有运行中的 aProxy 实例。`. Both exit 0 (outcome `already_stopped`):
  the instance is not running, which is what `stop` asks for. An unknown alias exits 4.
- Do not stop instances by killing processes. Killing a pid leaves the desired-state record, so the
  watchdog (on by default) starts the instance again; killing by name (`taskkill /IM aproxy.exe`,
  `pkill aproxy`) takes down every instance and the watchdog.

## restart

```
aproxy restart [PORT|ALIAS|PATH|default|all] [--force] [--json]
aproxy restart idle [--idle-threshold-secs SECS] [--force] [--json]
aproxy restart [PORT|ALIAS|PATH|default|all|idle] --format all|NAME... [--json]
```

Restarts running instances from their launch specs, which is how config.toml edits take effect.
Targets are resolved the same way as for [stop](#stop), but `restart` never starts a non-running
target and never deletes a desired-state record. The same caution applies: restarting the instance
your own client uses drops its in-flight requests, and if the new instance fails to start you lose
your connection to the model.

- **Only running instances.** A target that is not running exits 4 (`target_not_running`), and
  so does `restart` with no target when nothing is running; use `start` for that.
- **Checks before stopping.** `restart` first dry-runs what the new instance will do: load and
  validate the config with settings.json defaults applied, and, if the listen address changed,
  check that the new address is free. On failure it prints
  `端口 <port> 未重启：预检未通过，旧实例保持运行。` ("not restarted: pre-check failed, old instance
  still running") and the reason; the old instance keeps serving. An invalid config exits 3, a
  config file that no longer exists 4, a listen address that cannot be bound 1.
- **Launch and confirm.** It stops the old instance, starts the new one from the same launch spec
  and waits up to 30 s, locating it by its new pid, so a port changed in config.toml is handled.
  Success prints `已重启（端口 <port>）` with the new and old pid.
- **The desired-state record survives the whole restart.** It is never deleted between the stop and
  the new start, so if the new instance fails, the port is down but the record is still there:
  `端口 <port> 重启失败（旧实例已停止，该端口服务已中断）：…` ("restart failed: old instance
  stopped, service on this port is down") includes the new lines from `startup.log` and says
  `修正上述问题后执行 aproxy restore 恢复服务（看护者开着的话也会自动重试）。` Fix the cause,
  then run `aproxy restore`; with the watchdog enabled it also retries by itself. There is no
  `aproxy start --config …` recovery command, and one-off options do not need to be repeated.
- `端口 <port> 停止未确认，重启中止（未启动新实例，以免与旧实例争抢端口）。` ("stop not
  confirmed, restart aborted"): the old instance may still be running; nothing new was started.
- With several targets (`all`, `idle`) a failing instance is skipped, the rest are restarted, and a
  summary follows; the exit code is 5 when some were restarted, and the failure's own code (usually
  1) when none were. Hung instances that `all` cannot reach without `--force` count as failed.
- The new instance runs the binary you invoked `aproxy` from, so `restart` also moves an instance
  to that binary's version.
- The config path (or the absence of one), the override flags, the pinned port, the launch-time
  proxy environment and `PATH`, and the log path all come from the instance's desired-state record.
  An instance without a record cannot be restarted this way: a `--foreground` instance is refused
  with
  `端口 <port> 未重启：这个实例没有可用来重新拉起它的记录（前台运行的实例要在它自己的终端里重启），旧实例保持运行。`
  and keeps running. Stop it in its terminal and start it in the background to give it a record.
- `--force` terminates the old instance immediately but keeps the record, so the watchdog and
  `restore` can still bring the instance back if the new one fails. The watchdog may respawn it
  first; both use the same launch spec, and a new instance that finds the port already taken by the
  same config is treated as a success.

### Restarting only the format programs

`--format` replaces the processes of selected `[[format]]` entries and leaves the instance itself
running. Use it after editing a format program's own files (a role file of `aproxy-format`, a
script, its settings) or to clear an open breaker. Nothing else about the instance changes, so it
is the gentle option when the instance carries your own session.

- `--format all` selects every entry of each target instance; repeat `--format NAME` for a subset
  (entry names from `[[format]].name`, case-sensitive). `all` cannot be mixed with names, and
  `--format` cannot be combined with `--force` or `--idle-threshold-secs`; those are usage errors
  (exit 2): `请单独使用 --format all，或指定一个或多个 format 条目名。`
- **config.toml is not reread.** Each new process starts from the configuration the instance was
  started with, and the output ends with `未重读 config.toml；修改 [[format]] 条目须重启实例。`
  ("config.toml was not reloaded; restart the instance to change a [[format]] entry"). Adding,
  removing or editing a `[[format]]` entry still needs a full `restart`.
- **Nothing in flight is cut.** Requests and streams already being handled finish on the old
  processes; later requests go to the new ones. A replacement starts with a closed breaker.
- Per entry it prints `端口 <port>：已重启 format <name>（<program> <version>）。` or
  `端口 <port>：format <name> 握手失败：<reason>；没有替换，仍按原样运行。`, where reason is a fault
  kind such as `spawn`, `timeout` or `invalid_output`. A failed handshake replaces nothing: the
  entry keeps its current processes, breaker and settings, as `restart` keeps an old instance whose
  pre-check failed. Fix the program and run the same command again.
- An entry already being restarted (by another command or the desktop window) refuses the whole
  selection at once; run it again when that restart has finished.
- An unknown entry name refuses the whole selection before anything changes. An instance started by
  an older aproxy prints `端口 <port>：此实例不支持单独重启 format。`; restart that instance fully
  once.
- Exit codes count entries: 0 all restarted, 1 only failures, 5 a mix, 2 for a bad selection;
  target resolution keeps its usual codes. With `--json`, stdout is one document with an item per
  entry (`target`, `port`, `outcome` `restarted` / `handshake-failed` / `failed`, `name`,
  `program`, `version` or `reason`, `configReloaded: false`) and a `summary` of `ok` and `failed`.

## logs

```
aproxy logs [PORT|ALIAS|PATH|default] [-n N] [-f]
```

Prints the instance's most recent log lines (200 by default) and exits. `-n N` / `--lines N` sets
the count; `-f` / `--follow` keeps printing new lines until the instance stops
(`实例已停止，日志跟踪结束。`, "instance stopped, following ended") or you press Ctrl+C. Log lines
go to stdout; the explanatory lines (`日志文件: <path>`, the follow notice) go to stderr, so a
program reading the pipe receives only log lines. It asks the instance for its log path, because
log files get a new random name at every start.

- **Without `-f` it returns by itself**, so it suits a tool call. With `-f` it does not return while
  the instance runs: run it with a timeout (`timeout 5 aproxy logs -f 12345` in a POSIX shell, Git
  Bash included), take the path from its `日志文件: <path>` ("log file") line and read that file
  directly. The same path is the `log_path` field of the instance record
  `<home>/run/<port>.instance.json` (one line of JSON; `null` for a foreground instance).
- Several instances and no target, and `all` (`aproxy logs 不支持 all…`), exit 2.
- A `--foreground` instance has no log file and exits 1:
  `端口 <port> 的实例以前台模式运行（--foreground），日志输出在它的控制台，没有日志文件。` ("runs in
  foreground; its log is in its console; no log file").
- **A target that is not running but still has its desired-state record** (it crashed or was
  killed) gets the end of the log file named in that record, which is where the crash is recorded.
  stderr says `端口 <port> 的实例没在运行，以下是它上次运行的日志。` ("not running; this is the log of
  its last run"), and `-f` does not follow. With no target and nothing running, `logs` does this
  when exactly one record exists and exits 2 listing the ports when there are several. With
  nothing to read it exits 4.
- `── 日志文件已被截断，重新从头跟踪 ──` ("log file truncated, following from the start") appears
  while following when log rotation empties the file.

## restore

```
aproxy restore [--json]
```

Starts every instance that has a desired-state record but is not running: instances that crashed,
were killed, or were running when the machine lost power or rebooted. A graceful `stop`, a
`stop --force`, or `stop` on an instance that had already crashed deletes the record, so stopped
instances are not revived. Each instance is started from the launch spec in its record and gets the
same 30 s readiness check.

| Output | Meaning |
|---|---|
| `没有需要恢复的实例。` | nothing to restore |
| `端口 <port> 已在运行，跳过。` | already running, skipped |
| `端口 <port> 的配置文件找不到（<path>），这次没有恢复；记录已保留，文件回来之后再执行 aproxy restore。不再需要这个实例时执行 aproxy stop <port>` | the config file is temporarily gone (an unmounted network drive, for example); the record is kept so a later `restore` works. The same applies to a watchdog respawn |
| `已恢复 pid <pid>（端口 <port>）` | restored. If the config now names another port, the actual port is reported and the old port's record is replaced by the new one |
| `已恢复 pid <pid>（端口 <port>；配置的端口已从 <old> 变更，原端口的记录已由新端口接替）` | restored on another port |
| `端口 <port> 恢复失败：新实例（pid <pid>）启动后立即退出…` / `…未在预期时间内就绪…` | failed; the record is kept and `startup.log` has the reason |
| `跳过读不懂的记录 <path>: <reason>` | the record is damaged or was written by a newer aproxy. It is left untouched; use the version that wrote it, or remove it by hand once it is obsolete |

An instance started without a config file (only override flags and settings.json `defaults`) has a
record with no config path and is restored the same way. `restore` is safe to repeat, which suits a
login task (Task Scheduler on Windows) for recovery after reboots.

Exit codes: 0 when every record was restored or already running, or there was nothing to restore;
1 when every record failed; 5 when some failed, or when a config file is missing (that record was
not restored, and is kept for a later `restore`). Unreadable records are reported on stderr and do
not change the exit code.

## alias

```
aproxy alias add NAME [PATH]
aproxy alias remove NAME
aproxy alias list [--json]
```

Aliases live in settings.json and name a config file, so `start`, `stop`, `restart` and `logs`
can address an instance by name even after its port changes.

- `add`: PATH defaults to `<home>/config.toml` (not `default_config`); `~` is expanded and the
  path stored absolute. The file must exist; its content is not checked (run `doctor`). Adding an
  existing name repoints it (`已更新别名`, "alias updated").
- Rejected names (`别名无效: …`, "invalid alias"): empty, a number from 0 to 65535, and `all`,
  `idle`, `default`, `defult` in any case. Lookup is case-sensitive.
- `remove` deletes the name only; a running instance keeps running. An unknown name exits 1.
- `list --json` prints `{"schema":1,"aliases":[{"name","path","exists"}]}`, sorted by name, with
  `path` absolute and `exists` false when the file is gone (the alias is kept).

`alias add/remove` and `settings set default_config` / `settings unset default_config` edit only
their own key in
settings.json through the same writer as `aproxy settings`: the rest of the file, including key order
and any `ui` content, is kept, and a key this version does not know stays in the file (it is an
error, not something the write may drop). A change that would introduce an error is refused and
nothing is written. When settings.json is invalid, these commands do not run at all: every command
outside the tolerated group exits 3 with the `[ERROR] settings.json 有错误` block and repair hints
before it reaches them (settings-json.md, "When settings.json is invalid"). Repair the file first
with `aproxy settings unset KEY` (that command does run), then add the alias.

## find

```
aproxy find [QUERY] [--aliased | --unaliased] [--port PORT] [--json]
```

Lists `*.toml` files directly inside the config directories (`config_dirs` from settings.json plus
`<home>` and `<home>/configs/`; subdirectories are not searched). Each line shows path, port,
masked upstream, aliases and `[默认]` ("default") for the default config; files that do not parse
are flagged `（无法解析为有效 aProxy 配置）`. A config outside these directories is not listed even
when an alias points to it; add its directory to `config_dirs`. QUERY matches a path fragment or
alias name, case-insensitively; `--port` matches the configured listen port.

`--json` prints `{"schema":1,"configs":[{"path","aliases","listen_addr","base_url","errors"}]}`.
`base_url` is masked; `listen_addr` and `base_url` are `null` for a file that does not parse.
`errors` lists the file's error-level diagnostics (each with `code`, `file`, `key`, `line`,
`column`, `message`), resolved the way `start` would resolve it, settings.json `defaults` included,
so an empty `errors` means the config would pass `start`'s config check.

## doctor

```
aproxy doctor [--json]
```

Checks, without changing anything, every file aproxy reads, with the same parsers `start` uses:
settings.json; the default config (when it exists or settings.json names it), every alias target and
every `*.toml` directly inside the config directories, each resolved the way `start` would resolve it,
settings.json `defaults` included; the permissions of the home and of each config; and records in
`run/` that cannot be read. Run it after editing configuration by hand, and when `start`, `restart`
or `status` report something you cannot explain.

Each finding is one line, `[ERROR]`, `[WARN]` or `[INFO]` followed by
`file:line:col: key: message [code]` (line, column and key appear when known; the message is in
English). An unknown key gets a second line with the command that deletes it, `删除这个键: aproxy
settings unset '<key>'` or `删除这个键: aproxy config --config '<file>' unset '<key>'` ("delete this
key"). The last line is `检查结果: <n> error, <m> warning, <k> info` ("result"); with nothing to
report the output is `全部配置正常，未发现问题。` ("all configuration fine").

**Exit 3 when there is at least one error, 0 otherwise**: warnings and infos never fail it. An error
in a config means `start` refuses that config; an error in settings.json means most commands refuse
to run (settings-json.md, "When settings.json is invalid").

| Severity | Findings and their codes |
|---|---|
| error | everything `start` rejects, in settings.json and in each config: syntax (`config_syntax`), an unknown key (`config_unknown_key`), a wrong type or value (`config_type`, `config_invalid_value`), a missing key (`config_missing_key`), a newer settings.json `schema` (`settings_schema_too_new`). In settings.json this includes `watchdog.heartbeat_secs` or `watchdog.stale_after_cycles` set to 0, a credential in `defaults` without the `base_url` or `proxy` it belongs to, and an invalid alias name |
| warning | `config_file_unreadable`: an alias or `default_config` points at a file that is missing or cannot be read. `config_dir_missing`: a directory added to `config_dirs` does not exist. `listen_not_loopback`: any host that can reach the port can use the proxy, with the key it injects. `port_shared`: configs on the same port cannot run at the same time (each alone is fine). `file_permissions_too_open`: other users can read the home or a config, which hold keys; when the current user owns the home, aproxy tightens it on its next write. `watchdog_slow_hang_detection`: `watchdog.heartbeat_secs` above 600 delays hang detection. `watchdog_restarts_disabled`: `watchdog.max_restarts` is 0. `record_unreadable`, `record_schema_too_new`: a record in `run/` is left as it is, and log cleanup pauses while it is there |
| info | `default_header_shared`: a header in settings.json `defaults` is sent to every target; a header that carries a credential belongs in the configs that need it. `config_legacy_shape`: every error in the file comes from the 0.1.x shape; convert it with `aproxy migrate` (below) |

`--json` prints one document:

```json
{"schema":1,"files":[{"file":"C:\\Users\\me\\.aproxy\\settings.json","kind":"settings","diagnostics":[]},{"file":"C:\\Users\\me\\.aproxy\\config.toml","kind":"config","diagnostics":[{"code":"config_unknown_key","severity":"error","file":"C:\\Users\\me\\.aproxy\\config.toml","key":"retyr","line":3,"column":1,"message":"unknown key retyr","details":{}}]}],"summary":{"errors":1,"warnings":0}}
```

`files` lists every file checked, in the order settings, configs (by path), home, unreadable
records (`kind` is `settings`, `config`, `home` or `record`); a file without findings has an empty
`diagnostics`, so the list also tells you what was covered. A diagnostic has `code`, `severity`
(`error`, `warning`, `info`), `file`, `message` and `details`, plus `key`, `line` and `column` when
known. `"gc_paused":{"reason":"unreadable_records"}` appears while log cleanup is paused. The exit
code is the same as in text mode.

## migrate

```
aproxy migrate [PATH...] [--dry-run] [--json]
```

Converts 0.1.x settings.json and config files into the 0.2.0 shape, applying only the renames that
have one mechanical answer, after copying each original to `<file>.0.1.bak`. Without `PATH` it takes
settings.json and every config the home knows (aliases, `default_config`, `config_dirs`, `*.toml` in
`<home>/configs/` and `<home>`); with paths, only those config files. `--dry-run` prints the plan and writes nothing. A
file with anything that needs a decision (a 0.1.x transformer, a global `api_key` without
`defaults.base_url`, an old key next to its new name, an unknown key) is left entirely unchanged and
reported with the key and line. Running instances keep what they loaded until they restart.

Exit 0 when every file is converted or already current, 5 when some are converted and others left
for review, 3 when only review points remain, 1 when a file operation failed (this wins over 5 and 3).
The text output follows the user's language; `--json` is always English. `--json` gives
`files[]` (`path`, `outcome` = `migrated`/`current`/`needs-review`/`failed`, `changes`, `issues`)
and `summary`.

The full procedure, what each review point means and how to settle it with the user:
compatibility.md, "Converting the files with aproxy migrate".

## config

```
aproxy config [--config ALIAS|PATH] show [--effective] [--json]
aproxy config [--config ALIAS|PATH] get KEY [--json]
aproxy config [--config ALIAS|PATH] set KEY VALUE|- [--json]
aproxy config [--config ALIAS|PATH] unset KEY [--json]
```

`--config` names the file to read or edit, by alias or path, and may go before or after the
subcommand: `aproxy config --config work set listen_addr 127.0.0.1:12346` and `aproxy config get
base_url --config work` are the same command. Without it the default config file is used. A `set`
creates the file and its parent directories; `get` exits 4 when the file does not exist, and so does
`show` when `--config` named the missing file explicitly (the default path may legitimately be
absent before the first configuration).

- **`show`** prints the values written in the file, with secrets masked. `--effective` also fills in
  the keys the file does not write from the settings.json global defaults and the built-in
  defaults, marks each value's source (`config.toml`, `settings.json 全局默认`, `环境变量`,
  `内置默认` or `未设置`), and lists credentials withheld from a global default. If the file has
  errors, `show` still prints the values, then lists the diagnostics on stderr as
  `file:line:col: key: message [code]` and exits 3.
- **`get KEY`** prints one key's effective value, with the layer it comes from.
- **`set KEY VALUE`** changes one key; **`unset KEY`** removes one key. One key per command, so
  setting several keys takes several commands.

The keys the CLI most often writes, as spelled in config.toml:

| KEY | Effect |
|---|---|
| `base_url` | upstream base URL; a trailing `/` is ignored; must start with `http://` or `https://` and contain no `?` or `#` (checked before saving) |
| `listen_addr` | listen address; checked only at start |
| `api_key` | the key injected as `Authorization: Bearer` (what it injects: config-toml.md); `set api_key -` reads it from stdin |
| `extra_headers.NAME` | send a header only when the request lacks it |
| `override_headers.NAME` | always replace the header |
| `keepalive.interval_secs` | heartbeat interval; `0` turns heartbeats off |
| `proxy` | upstream proxy, e.g. `http://127.0.0.1:7890` or `socks5://user:pass@127.0.0.1:7890` |
| `proxy_username`, `proxy_password` | proxy credentials; take precedence over those in the URL |

- Quote a header name that contains dots: `aproxy config set extra_headers."x.y" v`. Any other key
  is a dotted key path such as `timeouts.connect_secs`; array tables (`format`, `retry.rules`) can
  only be set or removed as a whole.
- **Values are TOML.** `true`, `30`, `["a"]`, `{ k = 1 }` are read as TOML; a key whose type is a
  string accepts raw text without quotes (`aproxy config set base_url https://api.example.com`),
  and a value starting with `"` is parsed as a TOML string literal (for a newline, as in
  `keepalive.heartbeat`). Only known keys can be set.
- **Setters edit only the target key.** Comments, key order and everything else in the file are
  preserved. Writes are serialized across processes with a lock in `<home>/run/`.
- **A change that would introduce a new error is refused**, with the diagnostics and exit 3; the
  file is not written. Errors already in the file that this edit does not touch do not block it, so
  a broken file can be repaired one key at a time. A file with a TOML syntax error cannot be edited
  at all: fix the syntax first (exit 3).
- **Running instances do not pick up changes.** `set` prints the `aproxy restart <port>` command for
  every running instance that uses this file on stderr, and `--json` returns the ports in
  `instances_to_restart`; run the restart for the change to take effect.
- **`show` and `get` read the file, not a running instance:** one-off `start` options and edits not
  yet applied are invisible to them. `show --effective` reports what an instance started from this
  file would use.
- **Secrets are never printed.** `show` and `get` print a secret as `<secret>` or, at 24 characters
  or more, `<secret sk-a…f3a9>`; `--json` prints `{"secret":true}` with a `"hint"`. Writing that
  placeholder back through `set` keeps the stored value. Credentials in settings.json `defaults`
  are inherited only by an instance whose final `base_url` (or `proxy`) points at the same target as
  the default's. When it does not, `show --effective` prints a `全局默认里的这些凭据没有继承…`
  ("these credentials in the global defaults were not inherited") block naming each withheld key
  and both targets, and warns with `default_credential_withheld` when the instance has no credential
  of its own. settings-json.md, "Credentials in defaults follow their target", explains the
  comparison and which header names count as credentials.
- Format `args` values after `--api-key`, `--api_key`, `--apikey`, `--key`, `--token`,
  `--secret` or `--password` are masked in `show`; `env` is shown as names only, never values.
- Most fields (timeouts, retry, disk cache, `delivery`, `[keepalive]`, `[history]`, allow-lists,
  `log_file`, `[[format]]`, size limits) have no shortcut of their own: edit config.toml
  (config-toml.md) or the settings.json global default (settings-json.md), or use `set` with the TOML key path.
- **`--json`** is accepted by all four subcommands and prints one JSON document on stdout with
  `"schema":1`. `set` and `unset` return
  `{"schema":1,"file","key","value","changed","instances_to_restart":[ports]}`.
- **Exit codes:** 0 done; 1 write conflict or I/O failure; 2 a malformed key path or a value that is
  not TOML; 3 the result would be invalid, the key is unknown, or the file has errors (`show` still
  prints, then exits 3); 4 the file does not exist (`get`, or `show` with an explicit `--config`).

Adding a second instance, for example:

```sh
aproxy config --config "~/.aproxy/configs/work.toml" set base_url https://api.example.com
aproxy config --config "~/.aproxy/configs/work.toml" set listen_addr 127.0.0.1:12346
aproxy alias add work "~/.aproxy/configs/work.toml"
aproxy start work
```

## settings

```
aproxy settings show [--json]
aproxy settings get KEY [--json]
aproxy settings set KEY VALUE [--json]
aproxy settings unset KEY [--json]
```

Reads and edits `<home>/settings.json`; the file's keys and their meanings are in settings-json.md.
KEY is a dotted path as written in the file: `watchdog.enabled`, `aliases.work`,
`defaults.timeouts.connect_secs`, `defaults.extra_headers."x-foo"`. `show` prints the effective
settings — the file's keys over the built-in defaults — preceded by a `# <path>` line; `get` prints
one value as JSON, or `null` for a key that is neither stored nor built in.

- **This group works when settings.json is invalid.** It is how the file is inspected and repaired:
  `show` still prints every effective value, then the errors, and exits 3; `set` accepts a change
  that makes the file more correct and refuses one that would introduce a new error.
- **Values.** `set` takes a JSON value (`true`, `30`, `["a"]`, `{"k":1}`); string-valued keys accept
  raw text (`aproxy settings set language zh-CN`), and a value starting with `"` is read as a JSON
  string literal. `-` reads the value from one line of stdin, for keys that should not appear in the
  shell history: `printf '%s\n' "$KEY" | aproxy settings set defaults.api_key -`. Only keys this
  version knows can be set; `unset` accepts any key, including an unknown one.
- **Secrets are write-only.** `show` and `get` print a secret as `{"secret": true, "hint": "sk-a…f3a9"}`
  and mask URL credentials. Writing the placeholder back keeps the stored value.
- **Results and exit codes.** `已保存到 <path>` ("saved to") after a change, `没有变化: <path>`
  ("no change") when the value was already there. Exit 0 for those and for a successful `show`/`get`;
  2 for a malformed key path or a value that is not valid JSON; 3 when the file cannot be read or the
  change would make it invalid (nothing is written); 1 when the file cannot be written.
- **Running instances read settings.json only when they start.** After a change under `defaults`,
  `set` and `unset` list every running instance on stderr (`运行中的实例要重启才用上这次改动:`,
  "running instances need a restart to use this change", then one `aproxy restart <port>` line each).
  Changes outside `defaults` need no restart.
- **`--json`** prints one document on stdout: `show` gives
  `{"schema":1,"file","values","diagnostics"}` (the document comes first even when the file has
  errors, then the exit code is 3); `get` gives `{"schema":1,"key","value"}`; `set` and `unset` give
  `{"schema":1,"file","key","value","changed","instances_to_restart":[ports]}`, where `value` is the
  key's effective value after the write (after `unset`, that can be the built-in default). Failures
  give `{"schema":1,"error":{…}}` with `usage`, `config_invalid` or `write_conflict`.
- `aproxy alias add|remove` and `aproxy settings set|unset default_config` remain the commands for
  their fields; all go through the same writer and touch only their own key.

Example: give every instance a global default without editing JSON by hand.

```sh
aproxy settings set defaults.max_request_mb 128
aproxy settings set defaults.retry.rules '[{"path": "/v1/messages/count_tokens(\\?.*)?", "max_attempts": 3}]'
aproxy settings get defaults.max_request_mb
```

## desktop

```
aproxy desktop
```

Opens the desktop window. It is a separate process that runs until its window is closed; opening
or closing it never affects running instances, and like every other `aproxy` command it resumes an
install that was interrupted (see install / upgrade). It opens on the user's screen, so run it only
when the user asks for the window.

The window shows this home's real instances, request history and configs, and lets the user start,
stop and restart instances, restart single format programs, edit configs, switch the watchdog and
start-at-login, and install updates. Everything it does goes through the same records and control
channel as the CLI, so `aproxy status` and the window agree; for your own work, use the CLI, which
you can read. Its preferences, the enabled extensions and their settings are kept in
`<home>/desktop/desktop.toml`.

**Themes and extensions** are installed from the window's settings page, from a local .zip or an
unpacked folder, into `<home>/desktop/extensions/`. An extension is JavaScript that runs inside the
window without a sandbox and can do anything the window can; before installing, the window explains
each risk that applies and asks the user to confirm each one. When the user asks whether a package
is safe, review it with the aproxy-audit-extension or aproxy-audit-theme skill rather than judging
from its description.

- **Windows, from a terminal:** the command returns at once with `已打开 aProxy 桌面界面（pid <pid>）。`
  ("desktop window opened") and the window runs on its own. Exit code 0 here only means the window
  process was started: anything that goes wrong after that is shown in a dialog box on the user's
  screen, never in your terminal, so ask the user whether the window appeared before you rely on
  it. Started from a shortcut, Explorer or a process without a console, its messages appear in a
  dialog box instead.
- **Linux and macOS:** the command stays in the foreground until the window is closed (on macOS
  the window has not been tried on real hardware). When you open the window for the user, start it
  in the background (for example `aproxy desktop &`, or your tool's background option); waiting for
  it would block you until the user closes the window.
- **One window per home.** A second `aproxy desktop` under the same `<home>` prints
  `aProxy 桌面界面已经在运行（pid <pid>）。这个版本还不会把它切到前台，请在任务栏里找到它。`
  ("already running; this version cannot bring it to the front, find it in the taskbar") and exits
  0. Under another `APROXY_HOME` it opens a separate window, with its own web view data in that
  home's `desktop/`.
- **When it cannot open**, it prints `[ERROR] …` on stderr and exits 1 without a window: no
  Microsoft Edge WebView2 runtime on Windows (Windows 11 ships it; Windows 10 may need it
  installed, and `WEBVIEW2_BROWSER_EXECUTABLE_FOLDER` pointing at the wrong directory has the
  same effect), or on Linux no graphical session (`DISPLAY` and `WAYLAND_DISPLAY` both unset, as
  in an ssh session). The proxy and every other command work without these, so a failing window
  is never a reason to touch the instances.
- **On Linux a desktop build needs the WebKitGTK 4.1 libraries to run at all.** Without them the
  system loader refuses to start the binary, so even `aproxy --version` fails, naming a missing
  library such as `libwebkit2gtk-4.1.so.0`. Such machines need the headless build; the install
  script and `aproxy install` pick it there on their own (install / upgrade, "Choosing the
  variant").
- **Headless builds have no window.** When `aproxy --version` says `headless`, `aproxy desktop`
  explains that and exits 1; the command is also missing from `aproxy --help` there. The way to
  the desktop build is `aproxy install --variant desktop`, a normal install that restarts the
  instances one at a time. musl builds come only as headless; on a glibc Linux the desktop build
  needs glibc 2.34 or later and WebKitGTK 2.40 or later (Debian and Ubuntu package it as
  `libwebkit2gtk-4.1-0`), and the install stops with an error when either is missing.

## install / upgrade

```
aproxy install [latest|VERSION] [--pre] [--allow-downgrade] [--cpu x86-64-v3|baseline]
               [--variant desktop|headless] [--download-proxy URL] [--no-skills] [--json]
aproxy install --from PATH [--variant desktop|headless] [--no-skills] [--download-proxy URL] [--json]
aproxy install --adopt [--no-path] [--no-shortcut] [--no-skills] [--json]
aproxy install --check [--pre] [--variant desktop|headless] [--download-proxy URL] [--json]
aproxy install [latest|VERSION] --skills-only [--pre] [--download-proxy URL]
aproxy install --abort
aproxy install --force [latest|VERSION] [--pre] [--variant desktop|headless]
               [--cpu x86-64-v3|baseline] [--download-proxy URL] [--yes]
aproxy install --force --from PATH [--variant desktop|headless] [--yes]
```

`aproxy upgrade` is the same command. `install` downloads (or copies) the binary into
`<home>/install/staging/`, checks that it runs and reports the expected version, swaps it into
`<home>/bin/`, restarts running instances one at a time on the new binary, verifies that each
reports the new version, and cleans up; each instance is restarted once, one at a time. How the
swap and rollback work: behaviors.md, "Upgrades with aproxy install". Only `--adopt` (and the install
scripts' first install, which runs the same step) edits `PATH`; after any other install check that
`aproxy` resolves to `<home>/bin/` (`where aproxy`, `command -v aproxy`), or you keep running another
copy.
`--force` replaces the binary another way, for when this one fails: see
[Force install](#force-install).

| Option | Effect |
|---|---|
| `VERSION` | a version without the `v`, e.g. `0.2.0`; default `latest` |
| `--pre` | resolve `latest` in the pre-release channel |
| `--allow-downgrade` | allow a VERSION lower than the running CLI's version, but never below 0.2.0. Not with `--force`, which installs a named VERSION regardless |
| `--cpu x86-64-v3\|baseline` | which x86-64 build to download; default `x86-64-v3` when the CPU has AVX2 and the target publishes one (x86_64 Windows, x86_64 Linux gnu and x86_64 macOS), baseline otherwise |
| `--download-proxy URL` | proxy for downloads only; default `update.download_proxy` in settings.json (not read by `--force`), then the environment's proxy settings. Unrelated to config.toml's `proxy` |
| `--no-skills` | skip the skill update this time (permanently: `update.skill_auto_update` in settings.json). Not with `--force`, which never updates skills |
| `--from PATH` | install a local binary; its `--version` output decides the version, with no downgrade check (below 0.2.0 is still refused) |
| `--adopt` | copy the binary you are running into `<home>/bin/`, move running instances onto it, then add `<home>/bin` to the user's `PATH` and, for the desktop build, an app menu entry that opens the window ([What --adopt sets up](#what---adopt-sets-up)) |
| `--no-path` | with `--adopt`: leave `PATH` alone |
| `--no-shortcut` | with `--adopt`: create no app menu entry |
| `--skills-only` | update only the skill documents, to VERSION or, by default, the channel's latest version (the running version if the channel is empty) |
| `--abort` | cancel an install that has not reached the swap |
| `--check` | only report which version `install` would install, and whether it is newer than the running one; downloads nothing and changes nothing ("Checking for an update" below). Combines only with `--pre`, `--variant`, `--download-proxy` and `--json` |
| `--json` | machine-readable output on stdout, human lines on stderr: one JSON object per line during an install, one document with `--check` ("Exit codes and `--json`" below). Not with `--abort` or `--skills-only` |
| `--force` | stop every aproxy process of this home and replace the binary without the hand-over ([Force install](#force-install)). `--adopt`, `--abort`, `--skills-only`, `--no-skills`, `--allow-downgrade`, `--check` and `--json` cannot be combined with it: the command stops with a usage error (exit 2) and does nothing |
| `--yes` | with `--force`: do not ask for confirmation; required when stdin is not a terminal |
| `--variant desktop\|headless` | the build to install, with or without the desktop interface; default the build you run, so upgrades keep it ("Choosing the variant" below). With `--from` it is a check, not a choice: the file must be that build, or nothing is installed. Not with `--adopt`, `--abort` or `--skills-only` |

**Choosing the version.**

- `latest` is the highest version in a channel. The stable channel holds releases without a
  pre-release suffix that GitHub does not mark as pre-release; the pre channel holds everything,
  alpha, beta and rc included. The channel is stable when the running CLI is a stable release and
  pre when it is a pre-release; `--pre` forces pre. There is no `--stable`: to leave a pre-release
  for a stable release, name the version.
- Versions come from GitHub releases, or npm dist-tags when GitHub is unreachable. Only tags of the
  form `vX.Y.Z` or `vX.Y.Z-(alpha|beta|rc).N` with this platform's binary attached count.
- Nothing newer: `已是最新：当前 <v>，<channel> 通道最新为 <v>，无需安装。` ("already up to date"),
  exit 0, nothing restarted. Empty channel: `<channel> 通道暂无可用的 <variant> 版本，保持当前 <v> 不变。`
  ("no <variant> version in this channel; keeping the current one"), exit 0. With `--variant`
  naming the other build, neither message appears: the current version is reinstalled in that
  variant ("Choosing the variant" below).
- An explicit VERSION below the running CLI's version stops with
  `目标版本 <v> 低于当前 <v>。确认要降级请加 --allow-downgrade。` ("target below current; add
  --allow-downgrade to confirm").
- Nothing below 0.2.0 is installed, `--allow-downgrade` and `--from` included:
  `目标版本 <v> 低于 0.2.0，不能安装：0.1.x 与 0.2.0 起的版本互不兼容…` ("target below 0.2.0
  cannot be installed: 0.1.x and 0.2.0 or later are incompatible"). compatibility.md, "Going back to
  0.1.x", explains the way back. Likewise a 0.1.x `aproxy install` cannot reach 0.2.0; see
  "Moving from 0.1.x to 0.2.0" there.
- Downloads try the channels in `update.download_chain` (settings-json.md), by default github, npm,
  cargo-binstall, cargo. Each downloaded binary must run its `--version` on this machine and report
  the target version and variant. When it does not run: a desktop build is replaced by the headless
  build of the same target, unless you gave `--variant desktop`; on Linux a gnu build is then
  replaced by the musl build, which comes only as headless. A desktop build that no channel
  delivers is not replaced, because a release without that file and a passing network failure look
  the same to the channels: the install stops with exit 1 and, without `--variant`, suggests
  `--variant headless`, or `install --force`, which reads the release manifest and installs the
  headless build only when the desktop build is really missing.

**Checking for an update.** `aproxy install --check` asks the same sources in the same channel as
`install latest` and prints one of:

- `有新版本 <latest>（当前 <current>，<channel> 通道）。执行 aproxy install 升级。` ("version <latest>
  is available; run aproxy install to upgrade");
- `已是最新：当前 <current>，<channel> 通道最新为 <latest>。` ("up to date");
- `<channel> 通道暂无可用版本，当前 <current>。` ("no version in this channel").

In the stable channel the last two add `（如需预发布版本，可加 --pre）` ("add --pre for
pre-releases"). It exits 0 whenever the sources answered, update or not, and 1 when they did not
(`install_download_failed`; `--download-proxy` may help). It takes no lock and starts nothing, so it
is safe to run at any time. With `--json`:

```json
{"schema":1,"current":"0.2.0-alpha.3","latest":"0.2.0-alpha.4","update_available":true,"channel":"pre","variant":"desktop","published_at":"2026-11-25T08:00:00Z","url":"https://github.com/MoYeRanqianzhi/aProxy/releases/tag/v0.2.0-alpha.4"}
```

`latest` is null when the channel has no version; `variant` is the build `install` would install;
`published_at` and `url` are present when the answer came from GitHub.

**Choosing the variant.** Every platform has two builds with the same proxy and commands: `desktop`,
with the window of `aproxy desktop`, and `headless`, without it. The first word in the parentheses
of `aproxy --version` tells which one runs. Upgrades keep it; `--variant` switches. When nothing
newer is available (or the channel is empty), switching reinstalls the current version in the
other variant.

On Linux the desktop build links GTK and WebKitGTK and cannot even print `--version` without them.
aproxy does not inspect the machine beforehand; the downloaded binary's own `--version` decides,
because what a build needs changes from release to release. Without `--variant`, a desktop build
that does not run is replaced by the headless build of the same target, and the output says
`改装的是无界面的 headless 版本…` ("installing the headless build instead") with how to switch
back once the libraries are there. With `--variant desktop` nothing is replaced: the install stops,
exit 1, after the loader's error and, when the system loader refused the binary,
`带桌面界面的版本在本机起不来：动态加载器的报错见上…` ("the desktop build cannot start here; see
the loader's error above"). The loader's error names what is missing: a library such as
`libwebkit2gtk-4.1.so.0` (Debian and Ubuntu ship it in `libwebkit2gtk-4.1-0`), an undefined symbol
(the library is too old), or `GLIBC_2.xx not found` (glibc is too old, which only a newer system
fixes). Install what it names and run the command again, or use `--variant headless`.

**Which instances move.** Every running instance of the home is restarted on the new binary, so an
install first makes sure it can restart all of them, and refuses before it downloads, locks or
writes anything (exit 1) when it cannot:

- An instance started by an older version whose records this version cannot read (`status` lists it
  among the other aproxy processes):
  `[ERROR] 这个 home 里有旧版本启动、本次安装管不了的实例还在运行（pid <pids>）。…`
  ("instances started by an older version, which this install cannot manage, are running"), code
  `install_unmanaged_instances`. Stop them with the version that started them (its `aproxy stop all`),
  then install; or use `install --force`, which stops every aproxy process of the home.
  compatibility.md has the steps for the 0.2.0 alphas.
- A foreground instance (`start --foreground`), which has no record to restart it from:
  `[ERROR] 端口 <port> 上的实例在前台运行，安装无法替它重启。…` ("the instance on port <port> runs in
  the foreground; the install cannot restart it"), code `install_foreground_instances` with
  `details.ports`. Stop it (Ctrl+C in its terminal, `aproxy stop <port>`, or the service manager that
  runs it), install, then start it again the way it was started.
- Online installs and `--from` also refuse while a running instance's binary lives outside
  `<home>/bin/` (`实例 <port> 的二进制不在安装管辖目录（…）下`, "instance binary outside the managed
  directory"). Upgrade that copy through its own channel (npm, cargo, a package manager) or run
  `aproxy install --adopt`, which moves every running instance onto `<home>/bin/`.

Right before it touches the first instance, after the download, the install checks the first two
points again (an instance may have started meanwhile) and prechecks every instance it will restart:
the instance's start spec, resolved by the new version against the current settings.json. When an
instance fails it, nothing is changed and the command prints
`[ERROR] 安装前的预检没有通过：端口 <port> 的实例按新版本解析配置时出错。什么都没有改动：实例照常运行，二进制没换。`
("the precheck failed: the new version cannot resolve the configuration of the instance on port
<port>; nothing was changed"), the diagnostics, and `修正上面的问题后重新执行 aproxy install。` ("fix
the problems above and run aproxy install again"), exit 3, code `install_precheck_failed` with
`details.port` and `details.diagnostics`. An invalid settings.json is only a warning for an install
with no running instance, but fails this precheck when one runs. A failure at this point leaves
`<home>/install/state.json` marked as halted, which nothing resumes in the background; fix the cause
and run `aproxy install` again. An instance you stop while the install runs is skipped, not an error.

**Reading the result.**

- Exit 0 with `安装完成：<path>（二进制已落位，实例已滚动到新版本）` ("installed; binary in place,
  instances rolled to the new version") means every running instance was verified on the new
  version. Before it, `新版本进程已接手安装（广播、交换、滚动重启、终验），等待其完成...` ("the new
  version's process has taken over the install; waiting for it") appears: once the download is
  verified, the downloaded binary runs the rest in a separate process while the command waits (up
  to 600 s) and then reports. A downgrade is run by the command itself and skips this line.
  `<home>/install/state.json` may linger a moment afterwards while the old binary and the staging
  copy are deleted.
- While instances roll, `status` marks them `二进制更换中…`; do not stop or restart those.
- Skill documents are updated alongside into `<home>/skills/<name>/`; a skill failure does not
  fail the install. They are not linked into any agent's skill directory.
- `已有安装进行中（…）` ("an install is already running"): wait for it; a record whose installer died
  is taken over, a live one only after 600 s without progress.

**When it fails.** `[ERROR] 安装失败: …` is followed by one of two hints:

- `滚动已中止，不会自动重试…` ("rolling aborted, no automatic retry"): an instance did not start on
  the new version (`install_restart_failed`, with `details.port`). It was brought back on the old binary (if that also failed, its desired-state
  record was kept for `aproxy restore`), and the remaining instances were not touched. Usually the new
  version rejects the existing config: fix the reported cause and run `aproxy install` again, or
  stay on the old version with `aproxy install <old version> --allow-downgrade`. The new binary is
  already in `<home>/bin/` at this point, so the CLI and some instances run different versions
  until you finish.
- `现场已保留：重新执行同一条命令即可接着装完…` ("state kept; rerun the same command to finish"):
  run the same command again. When the error reads `目标版本没有接手安装（…）`
  ("the target version did not take over"), the downloaded binary exited or hung before starting;
  no instance and no file in `<home>/bin/` was changed.

An install interrupted by a crash, kill or power loss leaves `<home>/install/state.json`. The
watchdog resumes it in the background, and so does any command that changes instances or settings
(`start`, `stop`, `restart`, `restore`, `alias add|remove`, `config set|unset`, `settings set|unset`);
commands that only look (`status`, `logs`, `find`, `doctor`, `alias list`, `config show|get`,
`settings show|get`) never do, and nothing resumes a failure marked halted. Do not delete that file
by hand: it is what lets the install finish. `--abort` works only before the swap
(`安装已中止，现场已清理（二进制与实例未受影响）。`, "aborted; binary and instances untouched");
after it, the install can only be completed. When none of this gets the install through, use
[Force install](#force-install).

**Exit codes and `--json`.** 0: installed, or nothing to do; 1: failed; 2: usage error; 3: the
precheck failed; 4: the requested version does not exist (`install_version_not_found`). Every failure
has an `install_*` code (`docs/error-codes.md`), for example `install_download_failed`,
`install_checksum_mismatch`, `install_probe_failed`, `install_swap_failed`, `install_rolled_back` (the
previous binary was put back), `install_restart_failed`, `install_in_progress`, `install_failed`.

With `--json` an install prints one JSON object per line on stdout, each with `"schema":1` and an
`event`:

```json
{"schema":1,"event":"plan","ports":[12346,12347]}
{"schema":1,"event":"phase","phase":"downloading","at":1763540000000}
{"schema":1,"event":"progress","phase":"downloading","bytes":4194304,"total":9437184}
{"schema":1,"event":"instance","port":12346,"outcome":"restarted"}
{"schema":1,"event":"result","outcome":"installed","version":"0.2.0-alpha.4"}
```

- `plan` comes first, once the target version is known and the checks above passed: the running
  instances the install will restart.
- `phase` marks each phase as it begins, in the order `marking`, `downloading`, `downloaded`,
  `broadcasting`, `acked`, `swapping`, `swapped`, `restarting`, `verifying`, `cleaning`; `at` is Unix
  milliseconds.
- `progress` reports the download, at most about ten times a second; `total` is null when unknown.
- `instance` gives each instance's outcome: `restarted`, `rolled_back` (back on the old binary),
  `failed`, or `skipped` (stopped during the install).
- `result` is the last line: `outcome` is `installed`, `up_to_date` or `failed`; a failure adds
  `"error":{"code","message","details"}`, and `version` is null when it failed before the target
  version was known. A failure that early, or a refusal by the checks above, prints only this line.

**If `aproxy` itself will not run.** On Windows the swap renames the old binary to a unique
`<home>/bin/aproxy.old-<nanoseconds>-<pid>.exe` before moving the new one in, and
`<home>/bin/aproxy.bat` runs the newest of those whenever `aproxy.exe` is missing, so the command
keeps working from cmd and PowerShell through the swap. On Linux and macOS the swap is a single
atomic rename, so there is no such gap. If no copy runs, force-install from a downloaded release
binary, as the end of the next section describes.

Do not replace the binary by hand with `stop all` followed by `restore`: a graceful stop deletes the
desired-state records, so `restore` finds nothing to start. Force install keeps those records and
runs `restore` itself.

### What --adopt sets up

`aproxy install --adopt` is the way to bring a copy installed elsewhere (npm, cargo, a package
manager, a manual download) under the home's management. After the binary is in place it makes
`aproxy` reachable and, for the desktop build, adds an entry that opens the window. Both steps are
per user, need no admin rights, and are idempotent: running adopt again changes nothing that is
already right. The install scripts run the same step at the end of a first install (skip `PATH` with
`--no-path` or `APROXY_NO_PATH=1` for install.sh, `-NoPath` or `APROXY_NO_PATH=1` for install.ps1).

| | `PATH` (skip with `--no-path`) | App menu entry (skip with `--no-shortcut`; desktop build only) |
|---|---|---|
| Windows | appends `<home>\bin` to the user's `Path` in `HKCU\Environment` (keeps `%VAR%` entries and the value type) and tells Explorer, so new terminals see it | Start menu shortcut `aProxy.lnk` running `<home>\bin\aproxy.exe desktop --installed-home`. On Windows 11 24H2 and later it opens no console window; older Windows shows one briefly |
| macOS | writes `<home>/env` and adds `. <home>/env # aProxy` to `~/.profile`, to `~/.bashrc` / `~/.bash_profile` when they exist, to `~/.zshenv`, and a `~/.config/fish/conf.d/aproxy.fish` when fish is installed | `~/Applications/aProxy.app`, whose executable links to `<home>/bin/aproxy`, so it keeps working after `aproxy install` replaces the binary |
| Linux | the same profile lines as macOS (`~/.zshenv` when zsh is the login shell or `~/.zshrc` exists) | `~/.local/share/applications/aproxy.desktop` (under `XDG_DATA_HOME` when set) and icons under `icons/hicolor/` |

When `<home>/bin` is already on `PATH` nothing is written. Terminals that are already open keep their
old `PATH`; open a new one. An entry always opens the home it was made for: for a home other than the
default, its name carries a short id (`aProxy-<id>`), so entries for several homes can coexist.

The binary install is the result that counts: a `PATH` or entry step that fails prints a warning with
the cause and the manual way, and does not change adopt's exit code. With `--json` the final event has
`integration.path` and `integration.shortcut`, each with `status` (`changed`, `unchanged`, `skipped`
or `failed`), `reason` and `touched` (the files or registry value written).

There is no uninstall command yet. To undo by hand: remove `<home>\bin` from the user's `Path`
(Windows: Settings, "Edit environment variables for your account") or the lines ending in `# aProxy`
from the profile files and `<home>/env`; delete the shortcut, the `.app` or the `.desktop` file and its
icons.

### Force install

`aproxy install --force` (0.2.0 and later) is the fallback for when a normal install keeps failing
or hangs, or when the installed version is too broken to run. It does not hand the install over to
the new version or restart instances one at a time. It:

1. gets the target and checks it on this machine before changing anything: online through the
   `aproxy-manifest.json` attached to the release (asset picked by platform, variant and CPU, then
   its sha256 checked), or the file given with `--from`; then it runs the target's `--version`.
   It only repairs an existing installation: when the home directory does not exist, usually a
   mistyped `APROXY_HOME`, it ends with 10 (use the install script for a first install);
2. asks for confirmation;
3. stops every process recorded in `<home>/ledger/`: installers first, then the watchdog, the
   instances and anything else. It checks each process's start time before stopping it, so a pid
   that now belongs to another program is never hit, and it never stops processes by name. It
   stops only processes of the user running it: one owned by another user is skipped and
   reported (`跳过 …（只结束当前用户的进程）`, "skipped (only the current user's processes are
   stopped)"), and keeps running the old binary;
4. empties `<home>/install/`, discarding any unfinished normal install (its progress record
   first);
5. replaces the binary in `<home>/bin/`;
6. runs the new binary's `aproxy restore`, which starts the stopped instances from their
   desired-state records, and checks the ledger for an instance of the new version on each of their
   ports;
7. lists processes that run from `<home>/bin/` but are not in the ledger, without stopping them
   (not on macOS, where the paths of other processes cannot be read).

**Before you run it.** It cuts every request in flight through aProxy at once, including your own
when your client points at one of these instances, and it stops instances the user may be using in
other sessions. Tell the user that and get their agreement first. The command lists the processes
it will stop and asks `确认强制安装？输入 y 继续 [y/N]:` ("confirm the force install? type y to
continue"). When stdin is not a terminal, as in an agent's tool call, it does not wait: it exits 10
and asks for `--yes`. Add `--yes` only once the user has agreed.

**Running it from a tool call.** Give the command a timeout of at least 10 minutes. A slow download
plus waiting for every instance to come back can take several minutes, and a tool timeout that
kills it halfway can leave the instances stopped and not restored; when your own client goes
through aProxy, your session drops with them, and you can no longer tell the user what happened.
Do not pipe it into `head` or anything else that stops reading early: the shell then reports the
exit code of the last command in the pipe instead of force install's, and you lose the lines that
say what happened. Let it run to the end and read its exit code; the table below says what each
one means.

**Choosing the target.** `latest` (the default) and VERSION follow the normal channel rules, `--pre`
included, but only releases that carry `aproxy-manifest.json` count, which means 0.2.0 and later.
A named VERSION is installed even when it is older than the running one, so downgrades need no
flag; nothing below 0.2.0 is installed. `latest`, however, never downgrades: when it resolves to a
version below the running one (a newer release that is missing its manifest), force install exits
10 and says so; name the version to install it anyway. Downloads use HTTPS only and give up after
60 s without data. `--variant` defaults to the variant of the binary you run, the first word in the
parentheses of `aproxy --version`. Like a normal install ("Choosing the variant" above), it judges a
build only by running the downloaded binary's `--version`: without `--variant`, a desktop build that
does not run, or that the release does not publish for this platform at all, is replaced by the
headless build of the same target, then on Linux by the musl one, and the output says so before the
confirmation; with `--variant desktop` it exits 10 instead, before changing anything, and when the
system loader refused the binary it adds the same `带桌面界面的版本在本机起不来…` hint. Without
`--cpu`, the x86-64-v3 build is taken on CPUs with AVX2 when the release has one, and the baseline
build otherwise; an explicit `--cpu` must exist as named, for the headless build it falls back to
as well (the musl build is always baseline). When you named the variant or the CPU level and the
manifest has no such build for this platform, force install exits 10 and the error lists what the
platform does have, for example `这个目标只有 headless（baseline）` ("this target only has headless
(baseline)"): musl builds, for one, come only without the desktop interface. Rerun with a
combination from that list. Force install reads no config file and no settings.json, so pass
`--download-proxy URL` if downloads need a proxy. It does not update the skill documents; run
`aproxy install --skills-only` afterwards.

**Reading the result.**

| Exit | Meaning | Next step |
|---|---|---|
| 0 | done: binary replaced, every stopped instance back on its port (`强制安装完成`, "force install complete") | none; `aproxy install --skills-only` for matching skill documents |
| 10 | nothing was stopped: the home does not exist, the download, checksum or `--version` check failed, the target is below 0.2.0 or of the wrong variant, there was no confirmation (`什么都没有改动`, "nothing was changed"), or a later step failed before any process had been stopped (`一个进程都没有结束`, "no process was stopped") | fix what the `[ERROR]` line names and run it again |
| 11 | at least one process was stopped, but the binary was not replaced: another process holds the install lock or the binary file, a process could not be stopped (for example one running as administrator), or a file could not be written | fix the reported cause and run the same command again; every step can be repeated. To give up for now, `aproxy restore` starts the stopped instances on the old binary |
| 12 | the binary was replaced, but the instances listed after `二进制已换好，但下面这些被结束的实例没有回来` ("binary replaced, but these stopped instances did not come back") are down | each line gives the port and the command to start it, `aproxy start --config "<config path>"`; `<home>/logs/startup.log` has the reason, often a config file that was moved or deleted |

Two force installs on the same home stop each other: the later one ends the earlier, which then
exits with 1 (Windows) or 137 (killed by a signal on Linux and macOS) instead of one of the codes
above; read the later one's result.

Two more notes can appear. `看护者已被结束…` ("the watchdog was stopped") is expected: a restored
instance relaunches it within 5 minutes, and so does the next `aproxy start`.
`注意：下面这些进程从 <dir> 运行，但不在台账里…` ("these processes run from `<dir>` but are not in the
ledger") lists processes force install did not stop. They may be 0.1.x instances or watchdogs, which
keep no ledger, or other programs placed in `bin/`; find out which before acting. Stop 0.1.x ones
with the 0.1.x CLI (compatibility.md, "Moving from 0.1.x to 0.2.0").

**When the installed `aproxy` does not run at all.** Download the release asset for this platform
from https://github.com/MoYeRanqianzhi/aProxy/releases, check it against its `.sha256`, and, once the
user has agreed, run the downloaded file itself:
`<downloaded binary> install --force --from <downloaded binary> --yes`.
