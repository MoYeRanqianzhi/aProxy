# Version compatibility: moving from 0.1.x to 0.2.0

Part of [compatibility.md](../compatibility.md), which lists the other parts. Moving a 0.1.x installation to 0.2.0: the steps, the commands that changed, and going back.

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
   `aproxy alias list`. Note how each instance was started, not only its config: 0.1.x keeps each
   running instance's command line in `<home>/run/<port>.restore` (JSON with an `args` list), which
   shows the flags given at start (`--listen`, `--baseurl`, `--api-key`, `--proxy` ...). Those flags
   are not in any config file, and they may hold a key in plain text: do not print it; note which
   flags each instance uses, and agree with the user where the values will live under 0.2.0 (usually
   in that instance's config.toml).
2. Copy what going back would need into a private folder only the user can read: `settings.json`,
   every config file the instances and aliases use (including those outside `<home>`), the
   `<home>/run/*.restore` files and the 0.1.x binary from `<home>/bin/`. Keep the folder until the
   new setup has run without trouble; it holds credentials, so tell the user where it is and delete
   it together once it is no longer needed.
3. With the 0.1.x CLI, run `aproxy stop all`, then `aproxy status` until it lists no instance. Do
   this before the binary changes: afterwards no command can stop these instances gracefully.
4. Replace the binary through the channel that installed it. Every channel picks the newest
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
5. Delete what 0.1.x leaves behind: `<home>/run/install.state` and `<home>/staging/`, which 0.2.0
   never reads, and any `<home>/run/*.restore` (the copies from step 2 are kept). 0.2.0 reads only
   `<home>/run/*.restore.json`, so the old files are ignored rather than acted on; remove them so
   they do not mislead a later inspection. A crashed 0.1.x instance's record is dead too: if that
   instance matters, start it again with the new CLI after the upgrade.
6. Keep `settings.json` and the config files, but convert both: 0.2.0 refuses the 0.1.x shapes, so
   until they are converted most commands stop with exit 3 and no instance starts. Use
   `aproxy migrate` as [Converting the files with aproxy migrate](migrate.md#converting-the-files-with-aproxy-migrate)
   describes: preview with `aproxy migrate --dry-run`, show the user the planned changes, run
   `aproxy migrate`, then settle with the user whatever it leaves for review. Finish with
   `aproxy doctor`, which must report no errors.
7. Start each instance with the new CLI: `aproxy start <alias or config path>`, with the settings
   from step 1 now in its config (or given again as `start` flags, which 0.2.0 renamed: table
   below). Check each port with `aproxy status` against the list from step 1; when every instance
   is back on its old port, agent clients need no change.
8. Refresh these documents with `aproxy install --skills-only`, and check that the agent's skills
   directory links to or copies `<home>/skills/aproxy-cli`.

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

### Going back to 0.1.x

0.2.0 and later never install a version below 0.2.0, whether named or given with `--from`, and
`--allow-downgrade` does not change that. They stop with
`Target version <v> is below the minimum 0.2.0. Versions 0.1.x and 0.2.0 or later have incompatible control protocols and disk layouts. …`
Going back replaces the user's binary and files, so agree on it with the user first. Stop every
instance with the current CLI (`aproxy stop all`) and delete the binary from `<home>/bin/` as in
step 4. Put the 0.1.x binary back: the copy from step 2, or install it with that release's install
script, naming its tag, for example
`curl -fsSL https://raw.githubusercontent.com/MoYeRanqianzhi/aProxy/v0.1.0/scripts/install.sh | sh -s -- v0.1.0`
(on Windows, put the release binary from the Releases page at `<home>/bin/aproxy.exe`). 0.1.x cannot
read the converted files, so restore the originals before starting anything: the copies from step 2,
or the `<file>.0.1.bak` that `aproxy migrate` left next to each file it converted. Then start the
instances with 0.1.x, using the flags noted in step 1.
