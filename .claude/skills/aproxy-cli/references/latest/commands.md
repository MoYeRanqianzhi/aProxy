# aproxy command reference

Every `aproxy` subcommand: syntax, options, how targets are resolved, what the output means and the
exit codes. Read it when you need the exact form of a command or have to interpret what one printed.
What happens inside a running instance is in behaviors.md; symptom-driven fixes are in
troubleshooting.md.

## Contents

- [Before you run commands](#before-you-run-commands): home directory, isolated experiments,
  targets, output, exit codes, processes
- [Global options](#global-options)
- [start](#start) · [status](#status) · [stop](#stop) · [restart](#restart) · [logs](#logs) ·
  [restore](#restore)
- [alias](#alias) · [find](#find) · [doctor](#doctor) · [config](#config)
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

| Path | Holds |
|---|---|
| `<home>/config.toml` | the default instance config |
| `<home>/configs/` | the conventional place for further instance configs (`find` and `doctor` scan it) |
| `<home>/settings.json` | aliases, the default config, global defaults (settings-json.md) |
| `<home>/run/` | instance registry `<port>.pid`, restore records `<port>.restore`; `APROXY_RUN_DIR` relocates this directory alone |
| `<home>/logs/` | instance logs (random file names) and `startup.log` |
| `<home>/bin/` | the binary managed by `aproxy install`; on Windows also `aproxy.bat` (see install / upgrade). Files named `aproxy.old-…` and `aproxy.tmp-…` are left by binary swaps; installs delete the ones they can |
| `<home>/install/` | `aproxy install`'s own files: the progress record `state.json` while an install runs, downloads and staged binaries in `staging/<version>/`, the install script's downloads in `bootstrap/`, force install's download in `force/`, and `lock`, held while `bin/` changes |
| `<home>/ledger/` | one `<pid>.json` per running aproxy process (instances, the watchdog, installers), written at start and deleted at exit. `aproxy install --force` finds the processes to stop here. Leave these files alone |
| `<home>/skills/` | skill documents placed by `aproxy install` |

### Experiment in an isolated home

To test something without touching the user's instances, give the experiment its own home and a
port that no other instance uses:

```sh
# POSIX shell; keep these in one command line if each tool call gets a fresh shell
export APROXY_HOME="$(mktemp -d)"
aproxy config --baseurl https://api.example.com --listen 127.0.0.1:18080
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
| `idle [SECS]` | instances whose last client request is at least SECS old (default `idle_timeout_secs`, 1800) | stop, restart |

A number is always a port. Anything else is tried as an alias, then as `default`, then as a file
path. Alias names may not be numbers or reserved words, so these forms never collide. When two
running instances share one config file (started with different `--listen`), an alias or path
reaches only one of them; use ports.

### Output and exit codes

- Messages are in Chinese; this file quotes them verbatim with an English gloss. Results go to
  stdout, errors to stderr. The Windows console is switched to UTF-8 at startup.
- Credentials are masked everywhere: keys and header values keep their first 6 characters then
  `***`; URLs lose their userinfo (`***@`) and query values (`?key=***`).
- Every command except `doctor` first checks settings.json and prints problems as `[ERROR] …` on
  stderr, then carries on.
- Argument errors exit 2. `aproxy <command> --help` prints the options (in Chinese).

| Command | Exit 0 | Exit 1 |
|---|---|---|
| start, bare `aproxy` | started; already running on that port; listen port 0 (not waited for) | the config cannot be resolved, validated or bound, or the instance is not ready in 8 s |
| status, find | always | never |
| stop | every target confirmed stopped; nothing running for no target, `all` or `idle` | a stop not confirmed or `--force` failed; several instances and no target; unknown alias or port; target not running; target hung and no `--force` |
| restart | every target restarted; nothing running for no target, `all` or `idle` | any target failed; target cannot be resolved |
| logs | the followed instance stopped; nothing running | `all`, ambiguous or unknown target, foreground instance, unreadable log |
| restore | always, even when an instance fails to come back | never |
| alias | success | invalid name, missing file, unknown alias, unparsable settings.json, write failure |
| doctor | always, **even when it reports errors** | never |
| config | success | invalid value, unparsable existing toml or settings.json, missing config file on read, write failure |
| install | installed; already latest; channel empty; nothing to abort | any failure |

`install --force` exits 0, 10, 11 or 12 instead; see install / upgrade, Force install.

### Processes you may see

Instances, the watchdog and background install steps all run the same executable, which is why
killing `aproxy` by image name takes everything down at once. Tell them apart by command line:

| Command line contains | Process |
|---|---|
| `--daemon-child` | a background instance; `aproxy status` maps its pid to a port |
| `--daemon-watchdog` | the watchdog that restarts crashed or hung instances (behaviors.md, "Crash recovery and the watchdog") |
| `install --continue` | a background step of an install that is finishing or resuming |
| `install --force` | a force install in progress; it stops every other aproxy process of its home |

These flags are internal; never pass them yourself.

## Global options

| Option | Position | Effect |
|---|---|---|
| `--config PATH` | anywhere | config file for this command (see resolution below) |
| `--log-file PATH` | anywhere | log file for the instance being started; overrides `log_file` |
| `--foreground` | before the subcommand | run in this console instead of the background |
| `--baseurl URL` | before the subcommand | overrides `base_url` for the instance being started |
| `--listen ADDR` | before the subcommand | overrides `listen_addr` |
| `--proxy URL` | before the subcommand | overrides `proxy` |
| `--api-key KEY` | before the subcommand | overrides `api_key` |
| `-h, --help` / `-V, --version` | | help / print `aproxy v<version> (<variant>; <target>[; x86-64-v3])` (compatibility.md) |

- Only `--config` and `--log-file` may follow a subcommand. Write
  `aproxy --listen 127.0.0.1:12346 start work`; `aproxy start work --listen …` is rejected.
- The override options matter only when starting. They are not written to config.toml, but they
  become part of the instance's recorded start arguments, so `restart`, `restore`, the watchdog
  and `install` reuse them. `config --show` does not show them; `status` shows the effective
  listen address and upstream.
- `--api-key` on a command line is visible to other local processes and is stored in plain text
  in the restore record; put long-lived keys in config.toml.
- `--config` expands `~` and turns a relative path into an absolute one against the current
  directory; the instance records that absolute path. `--log-file` expands `~` and resolves a
  relative path against `<home>`, like `log_file`.

**Which config file a command uses:** the `start` target, else `--config`, else `default_config`
from settings.json, else `<home>/config.toml`. If `default_config` points to a file that no
longer exists, the commands that need the config exit 1 with
`settings.json 指定的默认配置文件不存在: <path>` ("the default config named in settings.json does
not exist"): bare `aproxy`, `aproxy start` without a target, and `aproxy config` showing or
editing a toml without `--config`. Every other command is unaffected; repair the setting with
`aproxy config --set-default PATH` or `aproxy config --clear-default`.

## start

```
aproxy [OPTIONS]                       # start the default config in the background
aproxy [OPTIONS] start [ALIAS|PATH]    # start a specific config
aproxy --foreground [start [ALIAS|PATH]]
```

What `start` checks, in order:

1. **Config.** The file is loaded strictly; a TOML syntax error, an invalid value or a
   `listen_addr` without a valid port stops here with exit 1, printing `配置错误: …` ("config
   error") or the parse error together with the file path. A fresh install has no `base_url`, so
   run `aproxy config --baseurl URL [--api-key KEY]` before the first start. Field rules:
   config-toml.md.
2. **Same port.** If an aProxy instance already answers on the port with the same listen address,
   `start` prints `此端口已有 aProxy 在运行，无需重复启动：` ("already running on this port") and
   exits 0, so repeating `start` is safe. With a different listen address it refuses
   (`…两者不能并存。`, "the two cannot coexist"): instances are identified by port alone.
3. **Bind.** A trial bind distinguishes a port taken by another program from one the system
   reserves; the messages and fixes are in troubleshooting.md.
4. **Launch.** The instance is spawned detached, so it outlives the terminal, and `start` waits up
   to 8 s for it to answer over IPC. Success prints pid, listen address, upstream, config and
   `日志: <path>` ("log"). Otherwise it prints `后台进程未在预期时间内就绪（pid <pid>）` ("background
   process not ready in time") and the reason is usually in `<home>/logs/startup.log`.

- A listen port of `0` lets the OS choose; `start` does not wait and tells you to read the actual
  address from `aproxy status`.
- A non-loopback listen address prints `警告: 监听地址 <addr> 不是回环地址…` ("not a loopback
  address") but does not block the start.
- After a successful background start, aProxy makes sure a watchdog is running when `watchdog` is
  enabled in settings.json (the default).
- `--foreground` keeps the instance in the console: logs print there and Ctrl+C stops it. It is
  still registered, so `status` and `stop` work. If the console closes without Ctrl+C, its restore
  record remains and `aproxy restore` brings it back as a background instance.

## status

```
aproxy status [--idle | --busy]
```

Lists instances in the registry that answer over IPC. A record that does not answer is deleted
when its process is gone (or its pid now belongs to another process); when the process is still
there, the record is kept and listed first, whatever the filter:
`无响应的 aProxy 实例 (<n>)——进程仍在，但不应答控制通道:` ("unresponsive instances: the process is
still there but does not answer the control channel"), with port, pid, version, config and the hint
`aproxy stop <port> --force`. Such an instance is almost always hung; with the watchdog on, its
hang detection terminates and respawns it on its own. Per answering instance: port, pid, `v<version>`, uptime, idle time, listen address, masked
upstream, config path, and request and retry counts with the latest error. Extra lines can
appear:

- `二进制更换中（install 滚动重启阶段，请勿手动干预此实例）` ("binary swap in progress; do not
  intervene"): `aproxy install` is about to restart this instance. Leave it alone.
- `正在退出` ("shutting down"): it received a stop and is finishing open requests (up to 10 s).
- `状态未知（实例比当前 CLI 新，用同版本的 aproxy 查看）` ("unknown state; the instance is newer
  than this CLI"): run the same version of aproxy as the instance.
- `注意: 实例版本 v<x> 与当前 CLI v<y> 不同…` ("instance version differs from this CLI"):
  see compatibility.md.

`--idle` shows only instances idle for at least `idle_timeout_secs` (settings.json, default
1800); `--busy` the rest. An instance that has served no request yet is idle from its start time.
`status` does not print log paths; see [logs](#logs).

## stop

```
aproxy stop [PORT|ALIAS|PATH|default|all] [--force]
aproxy stop idle [SECS] [--force]
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
  10 s and then drops them, removes its registry and restore records, and exits. `stop` confirms
  within 12 s: `已停止 pid <pid>（端口 <port>）` ("stopped"). If it prints
  `pid <pid> 已收到停止请求但尚未退出，可用 aproxy status 稍后确认，或 aproxy stop <port> --force 强制结束`
  ("received the stop request but has not exited; check status later or force it"), check
  `aproxy status` again shortly; if it is still listed, use `--force`. `stop` exits 1 whenever a
  target was not confirmed stopped, so scripts can rely on the exit code.
- **A hung instance** (process alive, control channel silent) cannot be stopped gracefully.
  Naming it by port or alias prints
  `端口 <port> 的实例（pid <pid>）进程仍在，但不应答控制通道，无法优雅停止；用 --force 强制结束`
  ("the process is still there but does not answer; use --force") and exits 1. `stop all`
  without `--force` stops the answering instances and says how many hung ones it left out.
- **`--force`:** terminates the process immediately, without the 10 s grace, after verifying by
  pid plus process start time that the pid still belongs to that instance (a reused pid is
  refused). It then deletes the restore record so neither the watchdog nor `restore` revives the
  instance. It finds hung instances through their registry record, so it works when IPC does not;
  `stop all --force` includes them.
- **`stop all`** stops every registered instance, including any your own session or the user's
  other sessions depend on. Use it only when the user asked for exactly that.
- **`stop idle [SECS]`** stops instances idle for at least SECS (default `idle_timeout_secs`).
  A small SECS can include instances in active use. Prints `没有闲置超过 <n> 秒的实例。` ("no
  instance idle longer than n s") when none qualify.
- An alias whose config is not running: `别名 <name>（配置 <path>）当前未在运行。`; a port with no
  instance: `端口 <port> 上没有运行中的 aProxy 实例。`; both exit 1.
- Do not stop instances by killing processes. Killing a pid leaves the restore record, so the
  watchdog (on by default) starts the instance again; killing by name (`taskkill /IM aproxy.exe`,
  `pkill aproxy`) takes down every instance and the watchdog.

## restart

```
aproxy restart [PORT|ALIAS|PATH|default|all] [--force]
aproxy restart idle [SECS] [--force]
```

Restarts running instances with their original start arguments, which is how config.toml edits
take effect. Targets, the multi-instance rule and `--force` work as for [stop](#stop), and the
same caution applies: restarting the instance your own client uses drops its in-flight requests,
and if the new instance fails to start you lose your connection to the model.

- **Only running instances.** A target that is not running exits 1; use `start` for that.
- **Checks before stopping.** `restart` first dry-runs what the new instance will do: parse the
  original arguments with this CLI, load and validate the config with settings.json defaults
  applied, and, if the listen address changed, check that the new address is free. On failure
  it prints `端口 <port> 未重启：预检未通过，旧实例保持运行。` ("not restarted: pre-check failed,
  old instance still running") and the reason; the old instance keeps serving.
- **Launch and confirm.** It stops the old instance, starts the new one and waits up to 8 s,
  locating it by its new pid, so a port changed in config.toml is handled. Success prints
  `已重启（端口 <port>）` with the new and old pid.
- **Failure after the stop.** `端口 <port> 重启失败（旧实例已停止，该端口服务已中断）：…` ("restart
  failed: old instance stopped, service on this port is down") includes the new lines from
  `startup.log` and a recovery command, `aproxy start --config "<path>"`. That command does not
  repeat one-off options such as `--listen` or `--api-key`; add them if the instance had any.
- `端口 <port> 停止未确认，重启中止…` ("stop not confirmed, restart aborted"): the old instance may
  still be running; nothing new was started.
- With several targets (`all`, `idle`) a failing instance is skipped, the rest are restarted, and a
  summary follows; the exit code is 1 if any failed.
- The new instance runs the binary you invoked `aproxy` from, so `restart` also moves an instance
  to that binary's version. A `--foreground` instance comes back in the background.
- Arguments come from the instance's restore record. If the record is missing, `restart` falls
  back to `--config <registered config path>` and one-off options are lost.
- `--force` removes the old instance's restore record before terminating it, as `stop --force`
  does, so the watchdog does not start a second copy alongside the new one. If the new instance
  then fails, recover with the `aproxy start` command the failure message prints.

## logs

```
aproxy logs [PORT|ALIAS|PATH|default]
```

Prints the last 30 lines of the instance's log and then follows it until the instance stops
(`实例已停止，日志跟踪结束。`, "instance stopped, following ended") or you press Ctrl+C. It asks the
instance for its log path, because log files get a new random name at every start.

- **It does not return while the instance runs.** In a non-interactive tool call, run it with a
  timeout (`timeout 5 aproxy logs 12345` in a POSIX shell, Git Bash included), take the path from
  its `日志文件: <path>` ("log file") line and read that file directly. The same path is the
  `log_path` field of the JSON registry file `<home>/run/<port>.pid`.
- Several instances and no target exit 1; `all` is refused (`aproxy logs 不支持 all…`).
- A `--foreground` instance has no log file:
  `该实例以前台模式运行（--foreground），日志输出在它的控制台…` ("runs in foreground; its log is in
  its console").
- `── 日志文件已被截断，重新从头跟踪 ──` ("log file truncated, following from the start") appears
  when log rotation empties the file.

## restore

```
aproxy restore
```

Starts every instance that has a restore record but is not running: instances that crashed, were
killed, or were running when the machine lost power or rebooted. A graceful `stop` deletes the
record, so stopped instances are not revived. Each instance gets its original arguments and the
same 8 s readiness check.

| Output | Meaning |
|---|---|
| `没有需要恢复的实例。` | nothing to restore |
| `端口 <port> 已在运行，跳过。` | already running, skipped |
| `端口 <port> 的配置文件已不存在（<path>），跳过并清理该记录。` | config gone; record deleted |
| `已恢复 pid <pid>（端口 <port>）` | restored; if the config now names another port, the old port's record is cleaned |
| `端口 <port> 的实例（pid <pid>）启动后立即退出…` / `未在预期时间内就绪…` | failed; reason in `startup.log` |

`restore` is safe to repeat, which suits a login task (Task Scheduler on Windows) for recovery
after reboots.

## alias

```
aproxy alias add NAME [PATH]
aproxy alias remove NAME
aproxy alias list
```

Aliases live in settings.json and name a config file, so `start`, `stop`, `restart` and `logs`
can address an instance by name even after its port changes.

- `add`: PATH defaults to `<home>/config.toml` (not `default_config`); `~` is expanded and the
  path stored absolute. The file must exist; its content is not checked (run `doctor`). Adding an
  existing name repoints it (`已更新别名`, "alias updated").
- Rejected names (`别名无效: …`, "invalid alias"): empty, a number from 0 to 65535, and `all`,
  `idle`, `default`, `defult` in any case. Lookup is case-sensitive.
- `remove` deletes the name only; a running instance keeps running. An unknown name exits 1.

`alias add/remove` and `config --set-default/--clear-default` rewrite settings.json. If the file
does not parse, they change nothing and exit 1 with `settings.json 解析失败（…），为免覆盖其中的别名等内容，本次不做修改。请先修复或删除该文件: <path>`
("settings.json failed to parse; not modified, to avoid overwriting its aliases; repair or delete
it first"); see settings-json.md, "Location and editing".

## find

```
aproxy find [QUERY] [--aliased | --unaliased] [--port PORT]
```

Lists `*.toml` files directly inside the config directories (`config_dirs` from settings.json plus
`<home>` and `<home>/configs/`; subdirectories are not searched). Each line shows path, port,
masked upstream, aliases and `[默认]` ("default") for the default config; files that do not parse
are flagged `（无法解析为有效 aProxy 配置）`. A config outside these directories is not listed even
when an alias points to it; add its directory to `config_dirs`. QUERY matches a path fragment or
alias name, case-insensitively; `--port` matches the configured listen port.

## doctor

```
aproxy doctor
```

Prints `[ERROR]` and `[WARN]` findings and `检查结果: <n> error, <m> warning`, or
`全部配置正常，未发现问题。` ("all configuration fine"). It exits 0 either way, so read the output.

| Level | Checks |
|---|---|
| ERROR | settings.json: JSON syntax, invalid alias names, aliases pointing to missing files, invalid `bounded_retry_paths` or `keepalive_trigger`, watchdog values that break hang detection or spin the CPU |
| WARN | configs that aliases point to, and other `*.toml` in the config directories: parse and validation failures, non-loopback listen addresses, port conflicts within each group (legal as long as the configs do not run at the same time); questionable watchdog values |

**The default config gets only the non-loopback check** unless an alias points at it. To validate
it fully, give it an alias (`aproxy alias add main <path>`; without a path the alias names
`<home>/config.toml`), or rely on `aproxy restart`, which validates before it stops anything.

## config

```
aproxy [--config PATH] config [--show]
aproxy [--config PATH] config SETTER...
aproxy config --set-default PATH | --clear-default
```

Reads or edits one config.toml: `--config`, else `default_config`, else `<home>/config.toml`.
Without setters it prints the file, as `--show` does.

| Option | Effect |
|---|---|
| `--show` | print the file's values with secrets masked; combine with setters to print the result |
| `--baseurl URL` | `base_url`; a trailing `/` is removed; must start with `http://` or `https://` and contain no `?` or `#` (checked before saving) |
| `--listen ADDR` | `listen_addr`; checked only at start |
| `--api-key KEY` / `--clear-api-key` | set or remove `api_key` (what it injects: config-toml.md) |
| `--extra-header K=V` | add or replace an `extra_headers` entry: sent only when the request lacks that header; repeatable |
| `--override-header K=V` | add or replace an `override_headers` entry: always replaces the header; repeatable |
| `--clear-headers` | empty both header maps, before any header given in the same command is added |
| `--keepalive-secs N` | `keepalive_interval_secs`; `0` turns heartbeats off |
| `--proxy URL` | upstream proxy, e.g. `http://127.0.0.1:7890` or `socks5://user:pass@127.0.0.1:7890` |
| `--proxy-username U`, `--proxy-password P` | proxy credentials; take precedence over those in the URL |
| `--clear-proxy` | remove proxy URL, username and password |
| `--set-default PATH` | set `default_config` in settings.json; the file must exist |
| `--clear-default` | remove `default_config` |

- **Setters rewrite the whole file** from the values this binary parsed: comments, formatting and
  keys it does not know are lost. Edit by hand when the file holds anything worth keeping. If the
  existing file does not parse, setters refuse with `现有配置文件解析失败，拒绝覆盖…` ("existing
  config fails to parse; refusing to overwrite") instead of replacing it with defaults.
- A missing file is created, parent directories included; this is how a config for a new
  instance is made. Reading a missing file named by `--config` exits 1 with
  `指定的配置文件不存在`.
- Empty values for `--api-key`, `--proxy`, `--proxy-username` and `--proxy-password` are rejected;
  use the `--clear-…` options. `--set-default` and `--clear-default` cannot be combined, and when
  either is given, other setters in the same command are ignored.
- **Running instances do not pick up changes.** Run `aproxy restart PORT|ALIAS` afterwards.
- `--show` reads the file, not a running instance: one-off start options and edits not yet
  applied are invisible to it. Fields left to settings.json print
  `（未在 toml 设置，运行时取 settings.json 全局默认）` ("not set in the toml; the settings.json
  global default applies"). If the file does not parse, it warns `警告: 配置文件解析失败，以下展示的是回退默认值而非文件内容` ("parse
  failed; showing fallback defaults, not the file").
- Transformer `args` values after `--api-key`, `--api_key`, `--apikey`, `--key`, `--token`,
  `--secret` or `--password`, and the whole `extra` string, are masked in `--show`.
- Every other field (timeouts, backoff, disk cache, `forward_only`, `bounded_retry_paths`,
  `allowed_hosts`, `allowed_origins`, `keepalive_trigger`, `log_file`, transformers, size limits)
  has no option here; edit config.toml (config-toml.md) or the settings.json global default
  (settings-json.md).

Adding a second instance, for example:

```sh
aproxy --config "~/.aproxy/configs/work.toml" config --baseurl https://api.example.com --listen 127.0.0.1:12346
aproxy alias add work "~/.aproxy/configs/work.toml"
aproxy start work
```

## install / upgrade

```
aproxy install [latest|VERSION] [--pre] [--allow-downgrade] [--cpu x86-64-v3|baseline]
               [--download-proxy URL] [--no-skills]
aproxy install --from PATH [--no-skills] [--download-proxy URL]
aproxy install --adopt [--no-skills]
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
swap and rollback work: behaviors.md, "Upgrades with aproxy install". `install` does not edit `PATH`; check that `aproxy` resolves
to `<home>/bin/` (`where aproxy`, `command -v aproxy`), or you keep running another copy.
`--force` replaces the binary another way, for when this one fails: see
[Force install](#force-install).

| Option | Effect |
|---|---|
| `VERSION` | a version without the `v`, e.g. `0.2.0`; default `latest` |
| `--pre` | resolve `latest` in the pre-release channel |
| `--allow-downgrade` | allow a VERSION lower than the running CLI's version, but never below 0.2.0 |
| `--cpu x86-64-v3\|baseline` | which x86-64 build to download; default `x86-64-v3` when the CPU has AVX2. Only some targets publish an x86-64-v3 build |
| `--download-proxy URL` | proxy for downloads only; default `download_proxy` in settings.json (not read by `--force`), then the environment's proxy settings. Unrelated to config.toml's `proxy` |
| `--no-skills` | skip the skill update this time (permanently: `skill_auto_update` in settings.json) |
| `--from PATH` | install a local binary; its `--version` output decides the version, with no downgrade check (below 0.2.0 is still refused) |
| `--adopt` | copy the binary you are running into `<home>/bin/` and move running instances onto it |
| `--skills-only` | update only the skill documents, to VERSION or, by default, the channel's latest version (the running version if the channel is empty) |
| `--abort` | cancel an install that has not reached the swap |
| `--force` | stop every aproxy process of this home and replace the binary without the hand-over ([Force install](#force-install)) |
| `--yes` | with `--force`: do not ask for confirmation; required when stdin is not a terminal |
| `--variant desktop\|headless` | with `--force`: the build to install, with or without the desktop interface; default the build you run |

**Choosing the version.**

- `latest` is the highest version in a channel. The stable channel holds releases without a
  pre-release suffix that GitHub does not mark as pre-release; the pre channel holds everything,
  alpha, beta and rc included. The channel is stable when the running CLI is a stable release and
  pre when it is a pre-release; `--pre` forces pre. There is no `--stable`: to leave a pre-release
  for a stable release, name the version.
- Versions come from GitHub releases, or npm dist-tags when GitHub is unreachable. Only tags of the
  form `vX.Y.Z` or `vX.Y.Z-(alpha|beta|rc).N` with this platform's binary attached count.
- Nothing newer: `已是最新：当前 <v>，<channel> 通道最新为 <v>，无需安装。` ("already up to date"),
  exit 0, nothing restarted. Empty channel: `<channel> 通道暂无可用版本，保持当前 <v> 不变。`
  ("no version in this channel; keeping the current one"), exit 0.
- An explicit VERSION below the running CLI's version stops with
  `目标版本 <v> 低于当前 <v>。确认要降级请加 --allow-downgrade。` ("target below current; add
  --allow-downgrade to confirm").
- Nothing below 0.2.0 is installed, `--allow-downgrade` and `--from` included:
  `目标版本 <v> 低于 0.2.0，不能安装：0.1.x 与 0.2.0 起的版本互不兼容…` ("target below 0.2.0
  cannot be installed: 0.1.x and 0.2.0 or later are incompatible"). compatibility.md, "Going back to
  0.1.x", explains the way back. Likewise a 0.1.x `aproxy install` cannot reach 0.2.0; see
  "Moving from 0.1.x to 0.2.0" there.
- Downloads try the channels in `download_chain` (settings-json.md), by default github, npm,
  cargo-binstall, cargo. On Linux, a gnu build that cannot run is replaced by the musl build
  automatically.

**Which instances move.** Online installs and `--from` refuse while a running instance's binary
lives outside `<home>/bin/` (`实例 <port> 的二进制不在安装管辖目录（…）下`, "instance binary outside
the managed directory"); an online install refuses before downloading anything. Upgrade that copy
through its own channel (npm, cargo, a package manager) or run `aproxy install --adopt`, which moves
every running instance onto `<home>/bin/`.

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
  the new version. It was brought back on the old binary (if that also failed, its restore record
  was kept for `aproxy restore`), and the remaining instances were not touched. Usually the new
  version rejects the existing config: fix the reported cause and run `aproxy install` again, or
  stay on the old version with `aproxy install <old version> --allow-downgrade`. The new binary is
  already in `<home>/bin/` at this point, so the CLI and some instances run different versions
  until you finish.
- `现场已保留，中断后重试同一命令或任何 aproxy 命令可自动续作…` ("state kept; rerun, or any aproxy
  command resumes"): run the same command again. When the error reads `目标版本没有接手安装（…）`
  ("the target version did not take over"), the downloaded binary exited or hung before starting;
  no instance and no file in `<home>/bin/` was changed.

An install interrupted by a crash, kill or power loss leaves `<home>/install/state.json`; the
watchdog and every other `aproxy` command resume it in the background. Do not delete that file by
hand: it is what lets the install finish. `--abort` works only before the swap
(`安装已中止，现场已清理（二进制与实例未受影响）。`, "aborted; binary and instances untouched");
after it, the install can only be completed. When none of this gets the install through, use
[Force install](#force-install).

**If `aproxy` itself will not run.** On Windows the swap renames the old binary to a unique
`<home>/bin/aproxy.old-<nanoseconds>-<pid>.exe` before moving the new one in, and
`<home>/bin/aproxy.bat` runs the newest of those whenever `aproxy.exe` is missing, so the command
keeps working from cmd and PowerShell through the swap. On Linux and macOS the swap is a single
atomic rename, so there is no such gap. If no copy runs, force-install from a downloaded release
binary, as the end of the next section describes.

Do not replace the binary by hand with `stop all` followed by `restore`: a graceful stop deletes the
restore records, so `restore` finds nothing to start. Force install keeps those records and runs
`restore` itself.

### Force install

`aproxy install --force` (0.2.0 and later) is the fallback for when a normal install keeps failing
or hangs, or when the installed version is too broken to run. It does not hand the install over to
the new version or restart instances one at a time. It:

1. gets the target and checks it on this machine before changing anything: online through the
   `aproxy-manifest.json` attached to the release (asset picked by platform, variant and CPU, then
   its sha256 checked), or the file given with `--from`; then it runs the target's `--version`;
2. asks for confirmation;
3. stops every process recorded in `<home>/ledger/`: installers first, then the watchdog, the
   instances and anything else. It checks each process's start time before stopping it, so a pid
   that now belongs to another program is never hit, and it never stops processes by name;
4. empties `<home>/install/`, discarding any unfinished normal install;
5. replaces the binary in `<home>/bin/`;
6. runs the new binary's `aproxy restore`, which starts the stopped instances with their original
   arguments, and checks the ledger for an instance on each of their ports;
7. lists processes that run from `<home>/bin/` but are not in the ledger, without stopping them.

**Before you run it.** It cuts every request in flight through aProxy at once, including your own
when your client points at one of these instances, and it stops instances the user may be using in
other sessions. Tell the user that and get their agreement first. The command lists the processes
it will stop and asks `确认强制安装？输入 y 继续 [y/N]:` ("confirm the force install? type y to
continue"). When stdin is not a terminal, as in an agent's tool call, it does not wait: it exits 10
and asks for `--yes`. Add `--yes` only once the user has agreed.

**Choosing the target.** `latest` (the default) and VERSION follow the normal channel rules, `--pre`
included, but only releases that carry `aproxy-manifest.json` count, which means 0.2.0 and later.
The version is not compared with the current one, so downgrades need no flag; nothing below 0.2.0
is installed. `--variant` defaults to the variant of the binary you run, the first word in the
parentheses of `aproxy --version`; on Linux without the desktop libraries (WebKitGTK), install
`--variant headless`. Without `--cpu`, the x86-64-v3 build is taken on CPUs with AVX2 when the
release has one, and the baseline build otherwise; an explicit `--cpu` must exist as named. When
the manifest has no build for this platform in the requested variant (and CPU level), force install
exits 10 and the error lists what the platform does have, for example
`这个目标只有 headless（baseline）` ("this target only has headless (baseline)"): musl builds, for
one, come only without the desktop interface. Rerun with a combination from that list. Force install reads no config file and no settings.json, so pass
`--download-proxy URL` if downloads need a proxy. It does not update the skill documents; run
`aproxy install --skills-only` afterwards.

**Reading the result.**

| Exit | Meaning | Next step |
|---|---|---|
| 0 | done: binary replaced, every stopped instance back on its port (`强制安装完成`, "force install complete") | none; `aproxy install --skills-only` for matching skill documents |
| 10 | stopped before changing anything: the download, checksum or `--version` check failed, the target is below 0.2.0 or of the wrong variant, or there was no confirmation (`什么都没有改动`, "nothing was changed") | fix what the `[ERROR]` line names and run it again |
| 11 | processes were stopped but the binary was not replaced: another process holds the install lock or the binary file, a process could not be stopped (another user's, or an elevated one), or a file could not be written | fix the reported cause and run the same command again; every step can be repeated. To give up for now, `aproxy restore` starts the stopped instances on the old binary |
| 12 | the binary was replaced, but the instances listed after `二进制已换好，但下面这些被结束的实例没有回来` ("binary replaced, but these stopped instances did not come back") are down | start each with `aproxy start <alias or config path>`; `<home>/logs/startup.log` has the reason, often a config file that was moved or deleted |

Two more notes can appear. `看护者已被结束…` ("the watchdog was stopped") is expected: a restored
instance relaunches it within 5 minutes, and so does the next `aproxy start`.
`注意：下面这些进程从 <dir> 运行，但不在台账里…` ("these processes run from `<dir>` but are not in the
ledger") lists processes force install did not stop, usually 0.1.x instances or watchdogs, which
keep no ledger. Stop those with the 0.1.x CLI (compatibility.md, "Moving from 0.1.x to 0.2.0").

**When the installed `aproxy` does not run at all.** Download the release asset for this platform
from https://github.com/MoYeRanqianzhi/aProxy/releases, check it against its `.sha256`, and, once the
user has agreed, run the downloaded file itself:
`<downloaded binary> install --force --from <downloaded binary> --yes`.
