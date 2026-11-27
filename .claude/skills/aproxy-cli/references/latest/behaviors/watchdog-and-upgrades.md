# Runtime behavior: crash recovery, the watchdog and upgrades

Part of [behaviors.md](../behaviors.md), which lists the other parts. Crash and hang recovery, the watchdog, what `aproxy install` does to running instances, and the download proxy.

## Crash recovery and the watchdog

### Desired-state records

- A background instance writes `<home>/run/<port>.restore.json` when it starts. The record holds the
  launch spec: the config path (null for an instance started without a config file), the override
  flags, the pinned port of a port-0 listener, the launch-time proxy environment and `PATH`, and the
  log path. A `--foreground` instance writes no record, because closing its console is the user
  stopping it.
- **Only `stop` deletes a record.** A record that remains means "this instance should be running",
  whatever took the process away: a crash, a kill by PID, a signal, a reboot, or a failed restart.
  `aproxy status` never deletes records. A record this version cannot read (damaged, or written by a
  newer aproxy) is reported and left in place by every reader — `status`, `restore`, the watchdog and
  `stop all` — but `aproxy stop <port>` deletes the file named `<port>.restore.json` without reading
  it, so a direct stop still clears an unreadable record. Records written by
  0.2.0-alpha.1/alpha.2 under the old name `<port>.restore` are not read at all (compatibility.md,
  "Upgrading from 0.2.0-alpha.1 or 0.2.0-alpha.2").
- `aproxy restore` starts every recorded instance that is not running. It skips instances that
  answer, waits up to 30 s for each start, and reports the actual port. If the config file named by
  a record is missing, it keeps the record and says so, so a file that comes back later (a network
  drive that mounts late) can still be restored. With nothing to do it prints
  `No instances to restore.` and exits 0, so it is safe as a logon task for
  recovery after a reboot or power loss.
- A restored or respawned instance reuses the spec but reads its config file as it is now: unapplied
  edits take effect, and a changed port moves the instance (the old port's record is replaced by the
  new port's). It keeps the launch-time proxy environment and `PATH`, so it reaches the network the
  same way as the original `start`. `aproxy restart` reuses the same spec, so changing a CLI
  override needs a stop followed by a start with the new flags.

### Watchdog

One background process watches every instance: the same executable started as
`aproxy __watchdog --home <dir>` (no public subcommand), a few MB of memory regardless of instance
count. It is on by default; the switch and tuning fields are in settings-json.md
(`watchdog.enabled` and the other `watchdog` keys). It runs on Windows, Linux and macOS.

- **Death** (crash, kill by PID): the watchdog sees the process exit at once and acts on it at once,
  so the instance is back within its startup time (typically under a second). With a desired-state
  record present it respawns the instance from that record and waits up to 30 s for it to come up;
  failed respawns are retried with backoff from 1 s doubling to 300 s. Without a record (graceful
  stop) it just stops watching. A respawn that lands on another port moves the watch there and
  replaces the old port's record.
- **Hang** (process alive, runtime stuck): each instance writes a heartbeat every 10 s (a
  shared-memory section on Windows, the file `<home>/run/<port>.heart` elsewhere). When it is stale past the scan tolerance and the instance does not answer on the control
  channel, the watchdog re-verifies PID plus creation time, kills the process and respawns it.
- **Crash loop**: after `watchdog.max_restarts` consecutive failed respawns the watchdog no longer
  gives up. It retries once every 300 s until the instance starts or its record is deleted
  (`aproxy stop <port>`), keeping the record throughout. The first time the limit is crossed it
  appends to startup.log:
  `[watchdog] the instance on port <port> failed to come back N times in a row; retrying every 300 seconds until it starts or aproxy stop <port> is run`.
  `watchdog.max_restarts = 0` is the exception: the watchdog never restarts an instance
  automatically (the record is kept for a manual `aproxy restore`).
- **Failure counting**: a respawned instance must stay up for 60 s before its earlier consecutive
  failures are forgotten. An instance that starts and dies again after a few seconds keeps counting
  and backs off instead of being respawned in a tight loop.
- **Settings are re-read every scan.** Setting `watchdog.enabled = false` makes a running watchdog
  exit at its next scan (it removes its record and leaves the instances and their records alone);
  `max_restarts` and `idle_exit_secs` follow the file on the next scan. `heartbeat_secs` and
  `stale_after_cycles` are fixed when the watchdog process starts, because other processes judge its
  liveness by the period it started with. If settings.json is invalid, the watchdog keeps its last
  valid values.
- **Single watchdog**: `<home>/run/watchdog.json` names the active one: `pid`, `start` (its start
  marker), `version`, `heartbeat_secs` (scan period), `created_at`, `last_scan_at` and `restarts`,
  the last 20 respawns as `{port, at, kind, ok}` with `kind` being `crash` or `hang` (times are Unix
  milliseconds). The watchdog rewrites it every scan; it counts as active while its process is alive
  and `last_scan_at` is within three scan periods (never less than 90 s). `aproxy start` launches
  one if none is active, and every instance checks every 5 minutes
  and launches a replacement if needed, logging "Watchdog was absent; this instance won the election and
  started it" in the instance's log. With no instances left and no respawn pending for `watchdog.idle_exit_secs`,
  it exits; an instance still waiting for its 300 s slow retry keeps it running.
- `aproxy status` ends with a `Watchdog:` line: its PID, version and the time since its last scan, or
  "no scan for ..." when it may be stuck; no line means no watchdog is recorded. For details, read
  `run/watchdog.json` and `<home>/logs/watchdog.log`, where the watchdog writes in English
  what it did and why: instances it terminated as hung, every respawn and its result, the switch to
  slow retries, instances it stopped watching, takeovers and its own exit.
- Limits: a respawn does not save requests in flight when the process died; their clients see a reset
  connection and must resend, which agent clients with their own retries do after a pause. What the
  watchdog prevents is an outage that lasts until someone notices.

## Upgrades with aproxy install

`aproxy install` (alias `upgrade`) puts the new binary in `<home>/bin/` and restarts running instances
one at a time, so at most one instance is restarting at any moment. Flags and modes are in
commands.md. During the run:

- Before the swap it asks each instance over the control channel to enter the binary-swap phase.
  `aproxy status` then shows `Replacing the binary during a rolling install. Do not stop or restart this instance.`
  Do not stop, restart or kill such an instance: install is about to
  restart it, and interfering makes the two fight.
- An instance that does not acknowledge (it is in a faulty state) is restarted onto the installer's
  version, for up to three rounds. If it still fails, install aborts with `These instances still have not confirmed that the binary can be replaced, even after retries and restarts: …`
  or `Could not restart the instance on port <port> to get its confirmation that the binary can be replaced: …`
  and kills nothing. Ask the user whether that instance may be stopped, stop
  it, and run install again.
- If the new binary cannot bring an instance up (stricter validation of the existing config,
  antivirus, not ready within 30 s), install restarts that instance from the old binary with the
  same launch spec, halts the rollout (remaining instances stay on the old version), prints
  `Rolling restarts stopped and will not retry automatically. …` and exits non-zero. The
  binary in `bin/` and the install phase are not rolled back: `<home>/install/state.json` holds phase
  `failed` and the cause in `last_error`. Read startup.log, fix the cause, run `aproxy install` again.
  If the old binary cannot start the instance either, it is down but its desired-state record is
  kept, and `aproxy restore` brings it back after the fix.
- The old binary is kept under a unique name, `bin/aproxy.old-<nanoseconds>-<pid>[.exe]`, for that
  rollback and removed at the end. One that is still locked (Windows) stays until a later install
  can delete it, and never blocks one. While `bin/` changes, the installer holds the lock
  `<home>/install/lock`.
- After the download is verified, the installer hands the install to the downloaded binary and waits
  for it (a downgrade is run by the installer itself). If that wait times out
  (`Could not confirm that the installation finished within N seconds. It may still be running. …`),
  check instance versions with `aproxy status`.
- An interrupted install (crash, power loss) resumes by itself when `<home>/install/state.json` is
  left over: the watchdog, `aproxy desktop`, and every command that changes instances or settings
  start `install --continue`; commands that only look (`status`, `logs`, `doctor` ...) do not.
  commands/install.md lists them.
- Inside the swap window the watchdog gives a dying instance up to 5 × 3 s to come back on its own
  before respawning it, and instances do not relaunch a missing watchdog.

None of this applies to `aproxy install --force`, which stops every process of the home, replaces
the binary and then restores the instances (commands.md, Force install).

### Skill documentation

Alongside the binary, install downloads the skill bundle of the same version (every skill aProxy
ships: aproxy-cli, aproxy-format, the aproxy-audit-* skills and the others) into
`<home>/skills/<name>/`, replacing each directory atomically. A failed download does not
fail the install; its result is in the `skill` field of `<home>/install/state.json`, `aproxy install --skills-only`
retries it, and `install --continue` does not. `--no-skills`, or `update.skill_auto_update = false` in
settings.json, skips it. Install never writes into an agent's own skills directory; link or copy
`<home>/skills/aproxy-cli` there yourself, for example to `~/.claude/skills/aproxy-cli` for Claude Code.

`aproxy skill` works with that bundle: `skill install` (alias `upgrade`) installs the skills of the
running version, with `--download-proxy URL` and `--json`; `skill list [--json]` shows each installed
skill with its version, `matches_binary` (whether that version is the running binary's), location and
files; `skill show NAME [FILE]` prints one file after checking it against the bundle's manifest
(`SKILL.md` by default), and `skill show NAME --files` lists the files. A matching manifest proves
the file is the installed copy, not that it matches the binary: check `matches_binary` for that.

## Download proxy versus request proxy

| Setting | Used for |
|---|---|
| config.toml `proxy` (with `proxy_username` / `proxy_password`) | That instance's upstream API traffic |
| settings.json `update.download_proxy`, or `aproxy install --download-proxy` for one run | Only `aproxy install` downloads (binary and skills) |

Without `proxy`, an instance's upstream traffic follows `ALL_PROXY` / `HTTPS_PROXY` / `HTTP_PROXY` /
`NO_PROXY` from the environment the daemon inherited: the shell that ran `aproxy start`, or, after a
respawn, the watchdog's environment. Set `proxy` explicitly when upstream traffic must or must not use
a particular proxy; an explicit `proxy` turns the environment variables off. With neither download
setting, `aproxy install` uses the same environment variables.
