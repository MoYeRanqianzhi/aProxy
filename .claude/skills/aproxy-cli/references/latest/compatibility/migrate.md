# Version compatibility: converting files with aproxy migrate

Part of [compatibility.md](../compatibility.md), which lists the other parts. Converting 0.1.x settings.json and config files with `aproxy migrate`, and settling with the user what it leaves for review.

## Converting the files with aproxy migrate

`aproxy migrate` rewrites 0.1.x files into the 0.2.0 shape using the two rename tables in [renamed-keys.md](renamed-keys.md)
("settings.json keys renamed in 0.2.0" and "Config keys renamed in 0.2.0"). Only 0.2.0 and later
have it, so run it with the new CLI after step 4 of [from-0.1.md](from-0.1.md). It changes only what has one mechanical answer and
leaves alone any file that needs a decision, so it never guesses on the user's behalf. That
decision is made per file: in one run some files can be converted while others are left for review,
and a review point about a path (an alias whose file is gone) is reported on its own, so the
settings file itself may already be converted when that point is still open. Read the report file by
file.

**Which files.** Without arguments it takes `<home>/settings.json` and every config file the home
knows: alias targets, `default_config`, and the `*.toml` files in each `config_dirs` entry, in
`<home>/configs/` and in `<home>` itself, the same set `aproxy doctor` checks. It reads that list from the 0.1.x settings.json even though 0.2.0 otherwise refuses
the file, so no alias is missed. `aproxy migrate PATH...` converts only the named config files;
settings.json is converted only by the form without paths, but is still read, because each config is
checked against the settings it will run with. A file reached by two paths is converted once (paths
are compared after resolving links and `..`). Backups, files under `<home>/run/` and files already in
the 0.2.0 shape are not rewritten. On Unix a file with other hard links is left for review
("This file has other hard links; migrating it would separate them."), because replacing it would
leave the other names pointing at the old content.

**What happens to each file.**

1. The new text is built from the rename tables. A key moves into its section; comments and the
   rest of the file stay as they were. A few values change form: `forward_only = true` becomes
   `delivery = "forward"` and `false` becomes `"retry"`; each `bounded_retry_paths` pattern becomes
   a rule with that `path` and `max_attempts = 3` (what 0.1.x did); `read_timeout_secs` becomes both
   `timeouts.first_byte_secs` and `timeouts.read_idle_secs`; a format's `every_attempt = true` adds
   `"retry"` after `"request"` in its `stages`. settings.json also gains `"schema": 1`.
2. The new text is checked the way `start` would check it, against the settings being converted in
   the same run. If it would still be invalid, the file is not written.
3. The original is copied to `<file>.0.1.bak`, readable only by the user, and then replaced, unless
   someone edited the file in the meantime. If a startup check of the written file fails, the
   original is put back from the backup. An existing `.0.1.bak` with different content is never
   overwritten.

Running it again is safe: converted files report "Already current", and a backup with the same
content is reused.

Settle settings.json first. Each config is checked against the settings it will run with, so while
settings.json still has a point to review, every config file reports "Repair settings.json first so
startup can validate this configuration." and waits.

**What is left for the user.** A file is "Left unchanged; review needed" when any part of it has no
single correct translation. Nothing in such a file is written, not even the keys that could have
moved, so the user sees it exactly as it was. Each point names the original key and line:

| Message | Why | What to do |
|---|---|---|
| on `request_transform`, `response_transform` or `heartbeat_transform`: "Choose the format program, stages and arguments before migrating this file." | A 0.1.x transformer has no rename: 0.2.0 speaks a different protocol, so the program itself has to change. | Replace the table with a `[[format]]` entry whose `command` speaks protocol 1 (an official `aproxy-format` role or the user's rewritten program) and whose `stages` list what it serves; `mode` becomes `process`, `idle_timeout_secs` becomes `worker_idle_secs`, `pool_max` and `timeout_secs` keep their names. The aproxy-format skill covers choosing or writing the program. Or delete the table if the user no longer needs it. Then run migrate again for the remaining keys. |
| on a format's `extra`: the same message | 0.2.0 has no `extra`. | Pass the data through the entry's `args` or `env` and change the program to read it from there. |
| on `defaults.api_key`, a top-level `api_key`, or a secret header in `defaults.extra_headers` / `override_headers`: "Set defaults.base_url first, so it is clear which upstream this credential belongs to." | 0.2.0 sends a global credential only to the upstream it is bound to (settings-json.md, "Credentials in defaults follow their target"). 0.1.0 had no global key; these come from files written for a 0.2.0 pre-release. | Ask the user which upstream the key belongs to and set `defaults.base_url` to it; move a top-level `api_key` to `defaults.api_key`. Or move the key into the config.toml of the instances that use it. |
| "Cannot move this key without overwriting `<new key>`." | The file has both the old and the new key, possibly with different values. | Ask the user which value is right, delete the other key, run migrate again. |
| "Unrecognized key; it is not a documented 0.1.x rename." | Neither version knows the key: a typo, `upstream_url` from an alpha build, or a top-level `api_key` once `defaults.base_url` is set. | Rename `upstream_url` to `base_url`. A key that holds a credential (`api_key`, a token) is never just deleted: ask the user where it belongs and move it there (`defaults.api_key`, or the config of the instance that uses it). Fix or delete anything else (`aproxy config unset KEY --config FILE`, `aproxy settings unset KEY`). |
| "This member occurs more than once; choose which value to keep." | settings.json names one key twice. | Keep one. |
| "This value or file is not valid in the target format." | 0.2.0 rejects the value even after the rename, or the file is not valid TOML or JSON. | Fix what the line points to, using config-toml.md or settings-json.md. |
| "Repair settings.json first so startup can validate this configuration." | A config file waits for settings.json. | Settle settings.json first, then run migrate again. |
| "This alias or default_config points to a file that does not exist; remove the alias or restore the file." | settings.json names a config that is gone. | Ask the user: remove the alias (`aproxy alias remove NAME`) or `default_config`, or put the file back. |
| "This config_dirs entry does not exist; remove it from settings.json or create it." | A configured directory is gone. | Ask the user: remove the entry from `config_dirs` or create the directory. |
| "Cannot reuse the existing .0.1.bak as an identical private backup; neither file was replaced." | An earlier backup with other content is in the way. | Ask the user before moving it aside: it may be the only copy of an earlier original. |

Settle these with the user rather than on your own: each one decides where a credential is sent,
which program sees the user's requests, or which of two values wins. Then run
`aproxy migrate --dry-run` again until nothing is left for review.

**Output.** One block per file: the path and "Would migrate" (with `--dry-run`), "Migrated",
"Already current", "Left unchanged; review needed" or "Failed", then one line per change
(`old → new = value`) or per point to settle (`key:line: message (section)`; a point about the whole
file has no key or line and prints as `: message (section)`). Secrets are masked in the report, not
in the files. The last line reminds that running instances keep the configuration they loaded.

The text follows the user's language, so on a Chinese system the headings and messages are Chinese.
To match messages against the table above, use `--json`: it
prints the same report as one document, always in English, with `files[]` (`path`, `outcome` =
`migrated`, `current`, `needs-review` or `failed`, `changes`, `issues` with `key`, `line`, `message`,
`section`) and a `summary` with the counts.

| Exit | Meaning |
|---|---|
| 0 | every file was converted (or would be, with `--dry-run`) or was already current |
| 5 | some files were converted, others are left for review |
| 3 | nothing was converted and some files are left for review |
| 1 | a file operation failed on at least one file (permission denied, disk full and the like); takes precedence over 5 and 3 |

A file edited by someone else while migrate ran, or one whose written text failed the startup check
and was put back from its backup, is left for review (exit 5 or 3), not a failure: run migrate again.

**What it leaves alone.** Running instances, until they restart; desired-state records; the 0.1.x
leftovers of step 5 in [from-0.1.md](from-0.1.md); the programs that format entries run.

Elsewhere 0.2.0 points at migrate when it meets a 0.1.x file. Most commands end their error list
with ``Preview the 0.1.x changes with `aproxy migrate --dry-run`, then run `aproxy migrate`.``,
and `doctor` adds that line as an `[INFO]` finding with the code `config_legacy_shape`. When `start`
was given the file as an alias or path, or `config show` got `--config`, the line names the file:
``… `aproxy migrate --dry-run <file>`, then run `aproxy migrate <file>`.`` On a 0.1.x home
settings.json is almost always old too, so expect the short form first. The hint appears only when
every error in the file comes from the 0.1.x shape, so a plain typo does not send the user here.

A session on a 0.1.x home with one plain config and one that uses a transformer:

```text
$ aproxy migrate --dry-run
<home>/settings.json: Would migrate
  max_body_mb → defaults.max_request_mb = 64
  forward_only → defaults.delivery = "retry"
  bounded_retry_paths → defaults.retry.rules = [{"path":"/v1/messages/count_tokens","max_attempts":3}]
  watchdog → watchdog.enabled = true
  download_proxy → update.download_proxy = "http://***@127.0.0.1:7890"
  — → schema = 1
<home>/relay.toml: Left unchanged; review needed
  request_transform:5: Choose the format program, stages and arguments before migrating this file. (compatibility/renamed-keys.md#config-keys-renamed-in-020)
<home>/work.toml: Would migrate
  max_body_mb → max_request_mb = 32
  read_timeout_secs → timeouts.first_byte_secs = 600
  read_timeout_secs → timeouts.read_idle_secs = 600
Running instances keep the configuration they loaded; restart them to use the migrated files.
$ aproxy migrate               # exit 5: settings.json and work.toml converted
  ... replace [request_transform] in relay.toml with the [[format]] entry the user chose ...
$ aproxy migrate relay.toml    # exit 0: forward_only → delivery = "forward"
$ aproxy doctor                # no errors
```

Windows prints the paths in their long form, beginning with `\\?\`.
