# aproxy command reference: stop, restart and restore

Part of [commands.md](../commands.md), which lists the other parts. Stopping, restarting or bringing back instances: `stop`, `restart`, `restore`.

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
