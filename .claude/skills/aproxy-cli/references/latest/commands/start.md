# aproxy command reference: start

Part of [commands.md](../commands.md), which lists the other parts. Starting an instance: `start` and every option it takes.

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
| `--reason start\|restart\|restore\|plugin` | why this start happens; default `start`. See below |
| `--note TEXT` | a one-line note recorded with this start, such as who started it and why. See below |
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

**Reason and note.** `start`, `restart` and `restore` accept `--reason` and `--note`. They describe
one start, not the instance: they are not part of the launch spec, so a later restart or a watchdog
respawn records its own reason instead of repeating yours. The instance writes both to its log
(`Instance started launch_reason=… launch_note=…`), to its registry record and to `status`
(`Last start: restarted (…)`; `launch_reason` and `launch_note` under `instance` in `--json`).

- `--reason` defaults to the command (`start`, `restart`, `restore`). Of the other values only
  `plugin` can be given; aProxy sets the rest itself: `login` (restored by the logon task),
  `watchdog` (respawned after a crash or hang; the note says `previous process exited with code
  <n>`, `previous process exited` or `previous process stopped responding`), `upgrade` (restarted
  by `aproxy install`; the note is `<old> -> <new>` when the old version is known) and `unknown`
  (started by a version that predates this, or with no reason passed). Any other value is a usage
  error (exit 2) and nothing starts. `install --force` ends with a plain `restore`, so instances it
  brings back show `restored`.
- `--note` is 1 to 512 bytes of UTF-8 on one line, without control characters; anything else, an
  empty note included, is a usage error and nothing starts. The instance replaces the secrets it
  knows (from its config and launch spec) before recording the note, but anything else in it is
  shown as written: never put keys or private details there.
- `restart all`, `restart idle` and a `restore` of several instances give each the same reason and
  note. `restart --format` does not start an instance and records nothing.
- An instance of an older version accepts the start but records nothing; `status` shows its
  reason as `unknown`.

**Which config file a command uses:** `start` uses its positional target, else `--config`, else
`default_config` from settings.json, else `<home>/config.toml`. `config …` uses its own `--config`
(an alias or a path), else `default_config`, else `<home>/config.toml`. If `default_config` points
to a file that no longer exists, the commands that need the config exit 4 with
`Default configuration file in settings.json does not exist: <path>`:
`aproxy start` without a target and `aproxy config …` without `--config`. Every other
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
   3, printing `Configuration errors:` followed by one `file:line:col: key: message [code]`
   line per problem; the codes are listed in config-toml.md, "Checks at start". A fresh install has no `base_url`, so
   run `aproxy config set base_url URL` and `aproxy config set api_key KEY` before the first start.
   Field rules: config-toml.md. When `start` is given neither `--config` nor a positional target and
   `<home>/config.toml` does not exist, the instance starts with no config file: the override flags
   and settings.json `defaults` supply everything, and `base_url` must come from one of them. Such
   an instance can still be restarted and restored.
2. **Same port.** If an aProxy instance already answers on the port with the same listen address,
   `start` prints `aProxy is already running on this port:` and
   exits 0, so repeating `start` is safe. With a different listen address it refuses
   (`Port <port> is in use by an aProxy instance listening on <addr>. …`): instances are identified by port alone.
3. **Bind.** A trial bind distinguishes a port taken by another program from one the system
   reserves; the messages and fixes are in troubleshooting.md.
4. **Launch.** The instance is started from a launch spec and detaches from the launching terminal
   (on Unix it gets its own session, so Ctrl+C in that terminal does not reach it; on Windows it
   tries to leave the launcher's job object, and startup.log warns when it cannot). `start` waits
   up to 30 s for it to answer over IPC; an early exit is reported at once. Success prints pid,
   listen address, upstream, config and `Log: <path>`. A timeout prints
   `The new instance (pid <pid>) did not become ready in time (process is running)` (the parenthesis
   says `process state unknown` when liveness cannot be determined) and the reason is usually in
   `<home>/logs/startup.log`.

- A listen port of `0` lets the OS choose. `start` waits for the instance and reports the actual
  address; that port is pinned in the instance's desired-state record, so `restart`, `restore` and
  a watchdog respawn return to it while the config still says `0`. Once the config names a fixed
  port, the config wins and the pinned port is discarded.
- A non-loopback listen address prints `Warning: Listening address <addr> is not a loopback address. …`
  but does not block the start.
- After a successful background start, aProxy makes sure a watchdog is running when
  `watchdog.enabled` is true in settings.json (the default).
- `--foreground` keeps the instance in the console: logs print there and Ctrl+C stops it. It is
  still registered, so `status` and `stop` work, but it has no desired-state record: closing the
  console ends it for good, `aproxy restore` does not bring it back, and `aproxy restart` refuses
  it with `…there is no desired-state record for this instance, so aProxy cannot restart it automatically. To restart a foreground instance, stop it and start it again yourself.` Start it again
  to bring it back.
