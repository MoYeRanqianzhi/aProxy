# Version compatibility: 0.2.0 alpha builds

Part of [compatibility.md](../compatibility.md), which lists the other parts. Moving from a 0.2.0 alpha build.

## Upgrading from 0.2.0-alpha.1 or 0.2.0-alpha.2

Those alphas keep their runtime records under other names and shapes: the instance record is
`<home>/run/<port>.pid`, the desired-state record `<home>/run/<port>.restore`, the watchdog record
`<home>/run/watchdog.claim`, and their status snapshot has no `start`, `instance_id` or
`foreground`. This version reads only `<port>.instance.json`, `<port>.restore.json` and
`watchdog.json`, and it cannot use an alpha instance's snapshot. So an instance started by one of
those alphas is out of reach for this version's CLI:

- `aproxy status` lists it only among the other aproxy processes, and `aproxy stop <port>` answers
  `No running aProxy instance on port <port>.`, although the
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
`config` flags became `config set`/`unset`. See "Commands changed in 0.2.0" in [from-0.1.md](from-0.1.md).

If the binary was replaced another way while alpha instances were running (by hand, for example),
the alpha CLI is gone and the new one cannot manage them. `aproxy install --force` still can: it finds every aproxy process of
this home through the process ledger, whose format never changes, and terminates them (requests in
flight are cut off). It cannot restart them, because their desired-state records are in the old
format; start each one again with `aproxy start`.

## Alpha builds

Versions such as `0.1.0-alpha.17` are pre-releases from before 0.1.0. They are unsupported and
these docs do not track how they differ, so upgrade rather than look up alpha behavior:

1. Run `aproxy install 0.1.0`. Name the version: a pre-release CLI resolves `latest` in the
   pre-release channel and can pick a 0.2.0 pre-release, which an alpha cannot install any more
   than 0.1.x can. To reach 0.2.0, continue with [Moving from 0.1.x to 0.2.0](from-0.1.md#moving-from-01x-to-020).
2. If the build has no `install` command, reinstall with the project's install script or your
   package manager.
3. Check that `aproxy status` shows the new version for every instance, and run
   `aproxy restart <port>` for any that does not.

Config files from alpha builds need the same renames as 0.1.x (see "Config keys renamed in 0.2.0" in [renamed-keys.md](renamed-keys.md),
and rename `upstream_url` to `base_url` by hand), and their flat settings.json is refused the same
way ("settings.json keys renamed in 0.2.0", same file). `aproxy migrate` converts both, as for 0.1.x. From 0.2.0 on, a key the version does not know is an error
that stops the instance; it is no longer ignored. Missing keys still take their defaults. If
something stops working after the upgrade, look the symptom up in troubleshooting.md.
