# settings.json reference: logs, watchdog, updates and the window

Part of [settings-json.md](../settings-json.md), which lists the other parts. `logs.rotate_mb`, `instances.idle_threshold_secs`, the watchdog, update settings and `ui`.

## logs.rotate_mb

Size limit for each instance's daemon log, in MB (default 8). Each instance checks its log every
hour and empties it once it is larger; no rotated copies are kept, and `aproxy logs -f` keeps
following the emptied file. `0` = never. Applies to custom `log_file` paths too (config-toml.md,
log_file).

## instances.idle_threshold_secs

How long an instance must go without activity to count as idle, in seconds (default 1800). Used by
`aproxy status --idle` / `--busy` and `aproxy stop idle` / `aproxy restart idle`;
`--idle-threshold-secs SECS` overrides it for that one command.

Activity is a new client request, the start of each retry, and in `delivery = "forward"` mode each
relayed chunk. A single upstream attempt that runs longer than the threshold without a retry, such
as a very long generation, counts as idle, and `stop idle` would cut it off. Keep the threshold
above your longest single generation before using `stop idle`.

## watchdog

`watchdog.enabled` is `true` by default and keeps one watchdog process per aProxy home. It watches
every instance and restarts one that crashes or hangs, from that instance's launch spec.
`aproxy start` launches the watchdog if none is running, and every instance checks every
5 minutes and relaunches it if it has gone.

`false` stops a running watchdog at its next scan: the watchdog re-reads settings.json every scan,
removes its record and exits, leaving the instances and their desired-state records alone. It also
stops new launches by `aproxy start` and by instances started after the change. Behavior:
behaviors.md, "Crash recovery and the watchdog".

## Watchdog tuning

The watchdog re-reads this section every scan. `max_restarts` and `idle_exit_secs` follow the file
on the next scan, as does `enabled`; `heartbeat_secs` and `stale_after_cycles` are fixed when the
watchdog process starts, because other processes judge its liveness by the period it started with.
If settings.json is invalid, the watchdog keeps its last valid values.

| Key | Default | Meaning | Choosing a value |
|---|---|---|---|
| `watchdog.heartbeat_secs` | `30` | Scan period for hang detection and adoption (crashes are handled the moment the process exits, not at the next scan). An instance whose heartbeat is older than `heartbeat_secs × (stale_after_cycles + 1)` (60 s with the defaults) and that does not answer an IPC ping is killed and restarted. | Lower detects hangs sooner at the cost of more wake-ups. `0` is an error (it would spin the CPU). |
| `watchdog.stale_after_cycles` | `1` | Extra scan periods a heartbeat may lag before the ping check. | Raise it if healthy instances on a heavily loaded machine get killed. `0` behaves as `1`. |
| `watchdog.max_restarts` | `5` | Failed respawns in a row, for one instance, after which the watchdog stops the fast exponential backoff and retries only every 300 s. The first restart is immediate; after a failure it waits 1 s, 2 s, 4 s ... up to 300 s. It never gives up: it keeps retrying while the desired-state record exists. A respawned instance must stay up 60 s before its earlier failures are forgotten. | `0` = never restart automatically; the record is kept for a manual `aproxy restore`. |
| `watchdog.idle_exit_secs` | `300` | How long the watchdog stays after the last instance is gone before exiting. An instance whose respawn is still pending (including the 300 s slow retry) keeps it from exiting. | `0` = stay resident. |

## update.download_chain

The ordered sources `aproxy install` tries for the binary and the skill files. Each source gets 3
attempts before the next is tried. `null` (the default) means
`["github", "npm", "cargo-binstall", "cargo"]`. A configured list is used exactly as written, with
nothing appended, so include every source you want as a fallback.

| Element | Source |
|---|---|
| `"github"` | GitHub Releases, verified against the published SHA-256 |
| `"npm"` | The npm registry configured in `~/.npmrc`, so a configured mirror is used; Node.js is not required |
| `"cargo-binstall"` | The crate's cargo-binstall template, which usually points to GitHub; cannot fetch the skill files |
| `"cargo"` | Builds from crates.io; needs a Rust toolchain, so keep it last |
| `{"url": "TEMPLATE"}` | A mirror of your choice |

URL template placeholders: `{version}` (the bare version, such as `0.2.0`, no `v`), `{asset}` (the
release file name: `aproxy-x86_64-pc-windows-msvc-v3.exe` for the desktop build,
`aproxy-headless-x86_64-pc-windows-msvc-v3.exe` for the headless build, or `aproxy-skills.zip` for
the skill files), `{target}` (the Rust target triple) and `{variant}` (the CPU suffix `-v3` or
empty; it says nothing about desktop or headless, which `{asset}` already carries). A mirror should
host both builds of a release: an install can fall back from the desktop build to the headless one
when the desktop build does not run. When
`<url>.sha256` exists the download is checked against it; otherwise it is accepted with
`[警告] 来源为非官方镜像（url 模板），产物未经独立校验` ("warning: unofficial mirror (URL template);
artifact not independently verified").

```json
{
  "update": {
    "download_chain": [
      { "url": "https://mirror.example.com/aproxy/v{version}/{asset}" },
      "github",
      "npm"
    ]
  }
}
```

A misspelled source name is an error (`config_invalid_value` on `update.download_chain`); the file is
not accepted.

## update.download_proxy

Proxy URL for `aproxy install` downloads only; proxied API traffic uses config.toml `proxy`
instead. Precedence: `aproxy install --download-proxy URL` > this field > the environment's proxy
variables. Credentials in the URL are masked in error output. An install that resumes by itself
after an interruption has no command line and uses only this field, so set it here when downloads
always need a proxy. `aproxy install --force` reads no settings.json at all; give it
`--download-proxy` for that run.

## update.skill_auto_update

`true` (the default): `aproxy install` also updates the skill files under `<APROXY_HOME>/skills/`,
in parallel with the binary; a failed skill update does not fail the install. `false`: skip them.
For one run, `aproxy install --no-skills` skips them and `aproxy install --skills-only` updates only
them, even when this field is `false`.

## ui

Not used. The desktop window keeps its own preferences under `<home>/desktop/`, not in
settings.json. `ui` stays a valid key so that files which have it keep working: the CLI only checks
that it is an object and keeps whatever is inside through every `settings` write. Nothing reads it;
leave it as it is.
