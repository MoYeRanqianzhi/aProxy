# aproxy command reference: alias, find and doctor

Part of [commands.md](../commands.md), which lists the other parts. Naming configs, locating them, and checking a setup: `alias`, `find`, `doctor`.

## alias

```
aproxy alias add NAME [PATH]
aproxy alias remove NAME
aproxy alias list [--json]
```

Aliases live in settings.json and name a config file, so `start`, `stop`, `restart` and `logs`
can address an instance by name even after its port changes.

- `add`: PATH defaults to `<home>/config.toml` (not `default_config`); `~` is expanded and the
  path stored absolute. The file must exist; its content is not checked (run `doctor`). Adding an
  existing name repoints it (`Updated alias <name> -> <path>`).
- Rejected names (`Invalid alias: …`): empty, a number from 0 to 65535, and `all`,
  `idle`, `default`, `defult` in any case. Lookup is case-sensitive.
- `remove` deletes the name only; a running instance keeps running. An unknown name exits 1.
- `list --json` prints `{"schema":1,"aliases":[{"name","path","exists"}]}`, sorted by name, with
  `path` absolute and `exists` false when the file is gone (the alias is kept).

`alias add/remove` and `settings set default_config` / `settings unset default_config` edit only
their own key in
settings.json through the same writer as `aproxy settings`: the rest of the file, including key order
and any `ui` content, is kept, and a key this version does not know stays in the file (it is an
error, not something the write may drop). A change that would introduce an error is refused and
nothing is written. When settings.json is invalid, these commands do not run at all: every command
outside the tolerated group exits 3 with the `[ERROR] settings.json has errors:` block and repair hints
before it reaches them (settings-json.md, "When settings.json is invalid"). Repair the file first
with `aproxy settings unset KEY` (that command does run), then add the alias.

## find

```
aproxy find [QUERY] [--aliased | --unaliased] [--port PORT] [--json]
```

Lists `*.toml` files directly inside the config directories (`config_dirs` from settings.json plus
`<home>` and `<home>/configs/`; subdirectories are not searched). Each line shows path, port,
masked upstream, aliases and `[default]` for the default config; files that do not parse
are flagged `(not a valid aProxy configuration)`. A config outside these directories is not listed even
when an alias points to it; add its directory to `config_dirs`. QUERY matches a path fragment or
alias name, case-insensitively; `--port` matches the configured listen port.

`--json` prints `{"schema":1,"configs":[{"path","aliases","listen_addr","base_url","errors"}]}`.
`base_url` is masked; `listen_addr` and `base_url` are `null` for a file that does not parse.
`errors` lists the file's error-level diagnostics (each with `code`, `file`, `key`, `line`,
`column`, `message`), resolved the way `start` would resolve it, settings.json `defaults` included,
so an empty `errors` means the config would pass `start`'s config check.

## doctor

```
aproxy doctor [--json]
```

Checks, without changing anything, every file aproxy reads, with the same parsers `start` uses:
settings.json; the default config (when it exists or settings.json names it), every alias target and
every `*.toml` directly inside the config directories, each resolved the way `start` would resolve it,
settings.json `defaults` included; the permissions of the home and of each config; and records in
`run/` that cannot be read. Run it after editing configuration by hand, and when `start`, `restart`
or `status` report something you cannot explain.

Each finding is one line, `[ERROR]`, `[WARN]` or `[INFO]` followed by
`file:line:col: key: message [code]` (line, column and key appear when known; the message is in
English). An unknown key gets a second line with the command that deletes it, `Remove this key: aproxy
settings unset '<key>'` or `Remove this key: aproxy config --config '<file>' unset '<key>'`.
The last line is `Results: errors <n>, warnings <m>, info <k>`; with nothing to
report the output is `All configurations passed. No problems found.`

**Exit 3 when there is at least one error, 0 otherwise**: warnings and infos never fail it. An error
in a config means `start` refuses that config; an error in settings.json means most commands refuse
to run (settings-json.md, "When settings.json is invalid").

| Severity | Findings and their codes |
|---|---|
| error | everything `start` rejects, in settings.json and in each config: syntax (`config_syntax`), an unknown key (`config_unknown_key`), a wrong type or value (`config_type`, `config_invalid_value`), a missing key (`config_missing_key`), a newer settings.json `schema` (`settings_schema_too_new`). In settings.json this includes `watchdog.heartbeat_secs` or `watchdog.stale_after_cycles` set to 0, a credential in `defaults` without the `base_url` or `proxy` it belongs to, and an invalid alias name |
| warning | `config_file_unreadable`: an alias or `default_config` points at a file that is missing or cannot be read. `config_dir_missing`: a directory added to `config_dirs` does not exist. `listen_not_loopback`: any host that can reach the port can use the proxy, with the key it injects. `port_shared`: configs on the same port cannot run at the same time (each alone is fine). `file_permissions_too_open`: other users can read the home or a config, which hold keys; when the current user owns the home, aproxy tightens it on its next write. `watchdog_slow_hang_detection`: `watchdog.heartbeat_secs` above 600 delays hang detection. `watchdog_restarts_disabled`: `watchdog.max_restarts` is 0. `record_unreadable`, `record_schema_too_new`: a record in `run/` is left as it is, and log cleanup pauses while it is there |
| info | `default_header_shared`: a header in settings.json `defaults` is sent to every target; a header that carries a credential belongs in the configs that need it. `config_legacy_shape`: every error in the file comes from the 0.1.x shape; convert it with `aproxy migrate` (below) |

`--json` prints one document:

```json
{"schema":1,"files":[{"file":"C:\\Users\\me\\.aproxy\\settings.json","kind":"settings","diagnostics":[]},{"file":"C:\\Users\\me\\.aproxy\\config.toml","kind":"config","diagnostics":[{"code":"config_unknown_key","severity":"error","file":"C:\\Users\\me\\.aproxy\\config.toml","key":"retyr","line":3,"column":1,"message":"unknown key retyr","details":{}}]}],"summary":{"errors":1,"warnings":0}}
```

`files` lists every file checked, in the order settings, configs (by path), home, unreadable
records (`kind` is `settings`, `config`, `home` or `record`); a file without findings has an empty
`diagnostics`, so the list also tells you what was covered. A diagnostic has `code`, `severity`
(`error`, `warning`, `info`), `file`, `message` and `details`, plus `key`, `line` and `column` when
known. `"gc_paused":{"reason":"unreadable_records"}` appears while log cleanup is paused. The exit
code is the same as in text mode.
