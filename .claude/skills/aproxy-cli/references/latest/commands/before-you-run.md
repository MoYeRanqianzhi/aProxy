# aproxy command reference: before you run commands

Part of [commands.md](../commands.md), which lists the other parts. What to know before running any command: the home directory, isolated experiments, how targets are resolved, output and `--json`, exit codes, which processes exist.

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
