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
  `There are <n> running instances. Specify a port, alias or all:`,
  followed by one `aproxy stop <port>` line per instance.
- **Graceful stop (default):** the instance stops accepting work, gives in-flight requests up to
  10 s and then drops them, removes its registry entry and its desired-state record, and exits.
  `stop` confirms within 12 s: `Stopped pid <pid> (port <port>)`. If it prints
  `Pid <pid> received the stop request but has not exited. Check with aproxy status, or run aproxy stop <port> --force.`,
  check `aproxy status` again shortly; if it is still listed, use `--force`. `stop` exits 1 when a
  target was not confirmed stopped (5 when other targets of the same command did stop), so scripts
  can rely on the exit code.
- **A target that already crashed** still has its desired-state record, which `restore` and the
  watchdog would use to bring it back. `stop` on that target (port, alias or config path) deletes
  the record and prints
  `The instance on port <port> is no longer running. Its desired-state record was removed. Restore and the watchdog will not start it again.`
  By alias or path the message starts `The instance for configuration <path> is no longer running. …`.
- **A hung instance** (process alive, control channel silent) cannot be stopped gracefully.
  Naming it by port or alias prints
  `Instance on port <port> (pid <pid>) is running but does not respond on the control channel. Use --force to terminate it.`
  and exits 1. `stop all`
  without `--force` stops the answering instances, reports each hung one the same way, and exits 5.
- **`--force`:** terminates the process immediately, without the 10 s grace, after verifying by
  pid plus process start time that the pid still belongs to that instance (a reused pid is
  refused). It deletes the desired-state record before terminating, so the watchdog does not race
  it and neither the watchdog nor `restore` revives the instance. It finds hung instances through
  their registry record, so it works when IPC does not; `stop all --force` includes them.
- **`stop all`** stops every registered instance, including any your own session or the user's
  other sessions depend on. Use it only when the user asked for exactly that. It also deletes the
  desired-state records of instances that are already gone
  (`Removed desired-state records left by stopped instances (ports …).`) and leaves records it cannot read in
  place, saying which and why.
- **`stop idle --idle-threshold-secs SECS`** stops instances idle for at least SECS (default
  `instances.idle_threshold_secs`). `--idle-threshold-secs` is valid only with the `idle` target; on
  any other target it is a usage error (exit 2). A small SECS can include instances in active
  use. Prints `No instances have been idle for more than <n> seconds.`
  when none qualify. The instance itself re-checks its idle time
  when the stop arrives: one that received a request after it was selected is left running, with
  `Pid <pid> on port <port> received a request in the meantime and has been idle for only <n> seconds, so it was not stopped.`,
  and the command still exits 0. `--force` has no such
  re-check.
- An alias whose config is not running: `Alias <name> (configuration <path>) is not running.`; a port with no
  instance: `No running aProxy instance on port <port>.`. Both exit 0 (outcome `already_stopped`):
  the instance is not running, which is what `stop` asks for. An unknown alias exits 4.
- Do not stop instances by killing processes. Killing a pid leaves the desired-state record, so the
  watchdog (on by default) starts the instance again; killing by name (`taskkill /IM aproxy.exe`,
  `pkill aproxy`) takes down every instance and the watchdog.

## restart

```
aproxy restart [PORT|ALIAS|PATH|default|all] [--force] [--reason R] [--note TEXT] [--json]
aproxy restart idle [--idle-threshold-secs SECS] [--force] [--reason R] [--note TEXT] [--json]
aproxy restart [PORT|ALIAS|PATH|default|all|idle] --format all|NAME... [--json]
```

Restarts running instances from their launch specs, which is how config.toml edits take effect.
Targets are resolved the same way as for [stop](#stop), but `restart` never starts a non-running
target and never deletes a desired-state record. The same caution applies: restarting the instance
your own client uses drops its in-flight requests, and if the new instance fails to start you lose
your connection to the model. Say why you restart with `--note`; `--reason` and `--note` are
described in [start](start.md#options-for-start), "Reason and note".

- **Only running instances.** A target that is not running exits 4 (`target_not_running`), and
  so does `restart` with no target when nothing is running; use `start` for that.
- **Checks before stopping.** `restart` first dry-runs what the new instance will do: load and
  validate the config with settings.json defaults applied, and, if the listen address changed,
  check that the new address is free. On failure it prints
  `Port <port> was not restarted: preflight failed. The old instance is still running.`
  and the reason; the old instance keeps serving. An invalid config exits 3, a
  config file that no longer exists 4, a listen address that cannot be bound 1.
- **Launch and confirm.** It stops the old instance, starts the new one from the same launch spec
  and waits up to 30 s, locating it by its new pid, so a port changed in config.toml is handled.
  Success prints `Restarted (port <port>)` with the new and old pid.
- **The desired-state record survives the whole restart.** It is never deleted between the stop and
  the new start, so if the new instance fails, the port is down but the record is still there:
  `Restart failed on port <port>. The old instance has stopped and service is interrupted: …`
  includes the new lines from `startup.log` and says
  `Correct the problem and run aproxy restore. If enabled, the watchdog also retries automatically.` Fix the cause,
  then run `aproxy restore`; with the watchdog enabled it also retries by itself. There is no
  `aproxy start --config …` recovery command, and one-off options do not need to be repeated.
- `Stop was not confirmed on port <port>. Restart was aborted. No new instance was started to avoid competing for the port.`:
  the old instance may still be running; nothing new was started.
- With several targets (`all`, `idle`) a failing instance is skipped, the rest are restarted, and a
  summary follows; the exit code is 5 when some were restarted, and the failure's own code (usually
  1) when none were. Hung instances that `all` cannot reach without `--force` count as failed.
- The new instance runs the binary you invoked `aproxy` from, so `restart` also moves an instance
  to that binary's version.
- The config path (or the absence of one), the override flags, the pinned port, the launch-time
  proxy environment and `PATH`, and the log path all come from the instance's desired-state record.
  An instance without a record cannot be restarted this way: a `--foreground` instance is refused
  with
  `Port <port> was not restarted: there is no desired-state record for this instance, so aProxy cannot restart it automatically. To restart a foreground instance, stop it and start it again yourself. The old instance is still running.`
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
  (exit 2): `Use --format all alone, or specify one or more format names.`
- **config.toml is not reread.** Each new process starts from the configuration the instance was
  started with, and the output ends with `config.toml was not reloaded; restart the instance to change a [[format]] entry.`
  Adding,
  removing or editing a `[[format]]` entry still needs a full `restart`.
- **Nothing in flight is cut, but in-flight requests see the switch.** A message already sent to a
  program, and a stream already open, finish on the old processes. Every later message goes to the
  new ones, including the next stage of a request that is still running (its response, a retry, a
  decision). A program whose per-request data lives in `state` handles that; one that changes the
  meaning of its `state` or keeps request facts in memory can break the requests caught in between,
  so restart such a program when the instance is idle (aproxy-format skill, protocol transport.md,
  "Transport"). A replacement starts with a closed breaker.
- Per entry it prints `Port <port>: restarted format <name> (<program> <version>).` or
  `Port <port>: format <name> failed its handshake: <reason>; nothing was replaced.`, where reason is a fault
  kind such as `spawn`, `timeout` or `invalid_output`. A failed handshake replaces nothing: the
  entry keeps its current processes, breaker and settings, as `restart` keeps an old instance whose
  pre-check failed. Fix the program and run the same command again.
- An entry already being restarted (by another command or the desktop window) refuses the whole
  selection at once; run it again when that restart has finished.
- An unknown entry name refuses the whole selection before anything changes. An instance started by
  an older aproxy prints `Port <port>: this instance does not support restarting formats.`; restart that instance fully
  once.
- Exit codes count entries: 0 all restarted, 1 only failures, 5 a mix, 2 for a bad selection;
  target resolution keeps its usual codes. With `--json`, stdout is one document with an item per
  entry (`target`, `port`, `outcome` `restarted` / `handshake-failed` / `failed`, `name`,
  `program`, `version` or `reason`, `configReloaded: false`) and a `summary` of `ok` and `failed`.

## restore

```
aproxy restore [--reason R] [--note TEXT] [--json]
```

Starts every instance that has a desired-state record but is not running: instances that crashed,
were killed, or were running when the machine lost power or rebooted. A graceful `stop`, a
`stop --force`, or `stop` on an instance that had already crashed deletes the record, so stopped
instances are not revived. Each instance is started from the launch spec in its record and gets the
same 30 s readiness check. Each restored instance records the reason `restore` (or `--reason`)
and the `--note`, as described in [start](start.md#options-for-start), "Reason and note".

| Output | Meaning |
|---|---|
| `No instances to restore.` | nothing to restore |
| `An instance is already running on port <port>. Skipping.` | already running, skipped |
| `Configuration file not found for port <port> (<path>). The instance was not restored. Its record is retained. Run aproxy restore when the file is back, or aproxy stop <port> if this instance is no longer needed.` | the config file is temporarily gone (an unmounted network drive, for example); the record is kept so a later `restore` works. The same applies to a watchdog respawn |
| `Restored pid <pid> (port <port>)` | restored. If the config now names another port, the actual port is reported and the old port's record is replaced by the new one |
| `Restored pid <pid> (port <port>; the configured port changed from <old>, and the record now belongs to the new port)` | restored on another port |
| `Restore failed on port <port>: The new instance (pid <pid>) exited immediately after startup…` / `…did not become ready in time…` | failed; the record is kept and `startup.log` has the reason |
| `Skipping unreadable record <path>: <reason>` | the record is damaged or was written by a newer aproxy. It is left untouched; use the version that wrote it, or remove it by hand once it is obsolete |

An instance started without a config file (only override flags and settings.json `defaults`) has a
record with no config path and is restored the same way. `restore` is safe to repeat, which suits a
login task (Task Scheduler on Windows) for recovery after reboots.

Exit codes: 0 when every record was restored or already running, or there was nothing to restore;
1 when every record failed; 5 when some failed, or when a config file is missing (that record was
not restored, and is kept for a later `restore`). Unreadable records are reported on stderr and do
not change the exit code.
