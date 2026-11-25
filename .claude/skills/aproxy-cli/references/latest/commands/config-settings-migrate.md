# aproxy command reference: config, settings and migrate

Part of [commands.md](../commands.md), which lists the other parts. Reading or editing config.toml and settings.json from the command line, and converting 0.1.x files: `config`, `settings`, `migrate`.

## migrate

```
aproxy migrate [PATH...] [--dry-run] [--json]
```

Converts 0.1.x settings.json and config files into the 0.2.0 shape, applying only the renames that
have one mechanical answer, after copying each original to `<file>.0.1.bak`. Without `PATH` it takes
settings.json and every config the home knows (aliases, `default_config`, `config_dirs`, `*.toml` in
`<home>/configs/` and `<home>`); with paths, only those config files. `--dry-run` prints the plan and writes nothing. A
file with anything that needs a decision (a 0.1.x transformer, a global `api_key` without
`defaults.base_url`, an old key next to its new name, an unknown key) is left entirely unchanged and
reported with the key and line. Running instances keep what they loaded until they restart.

Exit 0 when every file is converted or already current, 5 when some are converted and others left
for review, 3 when only review points remain, 1 when a file operation failed (this wins over 5 and 3).
The text output follows the user's language; `--json` is always English. `--json` gives
`files[]` (`path`, `outcome` = `migrated`/`current`/`needs-review`/`failed`, `changes`, `issues`)
and `summary`.

The full procedure, what each review point means and how to settle it with the user:
compatibility.md, "Converting the files with aproxy migrate".

## config

```
aproxy config [--config ALIAS|PATH] show [--effective] [--json]
aproxy config [--config ALIAS|PATH] get KEY [--json]
aproxy config [--config ALIAS|PATH] set KEY VALUE|- [--json]
aproxy config [--config ALIAS|PATH] unset KEY [--json]
```

`--config` names the file to read or edit, by alias or path, and may go before or after the
subcommand: `aproxy config --config work set listen_addr 127.0.0.1:12346` and `aproxy config get
base_url --config work` are the same command. Without it the default config file is used. A `set`
creates the file and its parent directories; `get` exits 4 when the file does not exist, and so does
`show` when `--config` named the missing file explicitly (the default path may legitimately be
absent before the first configuration).

- **`show`** prints the values written in the file, with secrets masked. `--effective` also fills in
  the keys the file does not write from the settings.json global defaults and the built-in
  defaults, marks each value's source (`config.toml`, `settings.json 全局默认`, `环境变量`,
  `内置默认` or `未设置`), and lists credentials withheld from a global default. If the file has
  errors, `show` still prints the values, then lists the diagnostics on stderr as
  `file:line:col: key: message [code]` and exits 3.
- **`get KEY`** prints one key's effective value, with the layer it comes from.
- **`set KEY VALUE`** changes one key; **`unset KEY`** removes one key. One key per command, so
  setting several keys takes several commands.

The keys the CLI most often writes, as spelled in config.toml:

| KEY | Effect |
|---|---|
| `base_url` | upstream base URL; a trailing `/` is ignored; must start with `http://` or `https://` and contain no `?` or `#` (checked before saving) |
| `listen_addr` | listen address; checked only at start |
| `api_key` | the key injected as `Authorization: Bearer` (what it injects: config-toml.md); `set api_key -` reads it from stdin |
| `extra_headers.NAME` | send a header only when the request lacks it |
| `override_headers.NAME` | always replace the header |
| `keepalive.interval_secs` | heartbeat interval; `0` turns heartbeats off |
| `proxy` | upstream proxy, e.g. `http://127.0.0.1:7890` or `socks5://user:pass@127.0.0.1:7890` |
| `proxy_username`, `proxy_password` | proxy credentials; take precedence over those in the URL |

- Quote a header name that contains dots: `aproxy config set extra_headers."x.y" v`. Any other key
  is a dotted key path such as `timeouts.connect_secs`; array tables (`format`, `retry.rules`) can
  only be set or removed as a whole.
- **Values are TOML.** `true`, `30`, `["a"]`, `{ k = 1 }` are read as TOML; a key whose type is a
  string accepts raw text without quotes (`aproxy config set base_url https://api.example.com`),
  and a value starting with `"` is parsed as a TOML string literal (for a newline, as in
  `keepalive.heartbeat`). Only known keys can be set.
- **Setters edit only the target key.** Comments, key order and everything else in the file are
  preserved. Writes are serialized across processes with a lock in `<home>/run/`.
- **A change that would introduce a new error is refused**, with the diagnostics and exit 3; the
  file is not written. Errors already in the file that this edit does not touch do not block it, so
  a broken file can be repaired one key at a time. A file with a TOML syntax error cannot be edited
  at all: fix the syntax first (exit 3).
- **Running instances do not pick up changes.** `set` prints the `aproxy restart <port>` command for
  every running instance that uses this file on stderr, and `--json` returns the ports in
  `instances_to_restart`; run the restart for the change to take effect.
- **`show` and `get` read the file, not a running instance:** one-off `start` options and edits not
  yet applied are invisible to them. `show --effective` reports what an instance started from this
  file would use.
- **Secrets are never printed.** `show` and `get` print a secret as `<secret>` or, at 24 characters
  or more, `<secret sk-a…f3a9>`; `--json` prints `{"secret":true}` with a `"hint"`. Writing that
  placeholder back through `set` keeps the stored value. Credentials in settings.json `defaults`
  are inherited only by an instance whose final `base_url` (or `proxy`) points at the same target as
  the default's. When it does not, `show --effective` prints a `全局默认里的这些凭据没有继承…`
  ("these credentials in the global defaults were not inherited") block naming each withheld key
  and both targets, and warns with `default_credential_withheld` when the instance has no credential
  of its own. settings-json.md, "Credentials in defaults follow their target", explains the
  comparison and which header names count as credentials.
- Format `args` values after `--api-key`, `--api_key`, `--apikey`, `--key`, `--token`,
  `--secret` or `--password` are masked in `show`; `env` is shown as names only, never values.
- Most fields (timeouts, retry, disk cache, `delivery`, `[keepalive]`, `[history]`, allow-lists,
  `log_file`, `[[format]]`, size limits) have no shortcut of their own: edit config.toml
  (config-toml.md) or the settings.json global default (settings-json.md), or use `set` with the TOML key path.
- **`--json`** is accepted by all four subcommands and prints one JSON document on stdout with
  `"schema":1`. `set` and `unset` return
  `{"schema":1,"file","key","value","changed","instances_to_restart":[ports]}`.
- **Exit codes:** 0 done; 1 write conflict or I/O failure; 2 a malformed key path or a value that is
  not TOML; 3 the result would be invalid, the key is unknown, or the file has errors (`show` still
  prints, then exits 3); 4 the file does not exist (`get`, or `show` with an explicit `--config`).

Adding a second instance, for example:

```sh
aproxy config --config "~/.aproxy/configs/work.toml" set base_url https://api.example.com
aproxy config --config "~/.aproxy/configs/work.toml" set listen_addr 127.0.0.1:12346
aproxy alias add work "~/.aproxy/configs/work.toml"
aproxy start work
```

## settings

```
aproxy settings show [--json]
aproxy settings get KEY [--json]
aproxy settings set KEY VALUE [--json]
aproxy settings unset KEY [--json]
```

Reads and edits `<home>/settings.json`; the file's keys and their meanings are in settings-json.md.
KEY is a dotted path as written in the file: `watchdog.enabled`, `aliases.work`,
`defaults.timeouts.connect_secs`, `defaults.extra_headers."x-foo"`. `show` prints the effective
settings — the file's keys over the built-in defaults — preceded by a `# <path>` line; `get` prints
one value as JSON, or `null` for a key that is neither stored nor built in.

- **This group works when settings.json is invalid.** It is how the file is inspected and repaired:
  `show` still prints every effective value, then the errors, and exits 3; `set` accepts a change
  that makes the file more correct and refuses one that would introduce a new error.
- **Values.** `set` takes a JSON value (`true`, `30`, `["a"]`, `{"k":1}`); string-valued keys accept
  raw text (`aproxy settings set language zh-CN`), and a value starting with `"` is read as a JSON
  string literal. `-` reads the value from one line of stdin, for keys that should not appear in the
  shell history: `printf '%s\n' "$KEY" | aproxy settings set defaults.api_key -`. Only keys this
  version knows can be set; `unset` accepts any key, including an unknown one.
- **Secrets are write-only.** `show` and `get` print a secret as `{"secret": true, "hint": "sk-a…f3a9"}`
  and mask URL credentials. Writing the placeholder back keeps the stored value.
- **Results and exit codes.** `已保存到 <path>` ("saved to") after a change, `没有变化: <path>`
  ("no change") when the value was already there. Exit 0 for those and for a successful `show`/`get`;
  2 for a malformed key path or a value that is not valid JSON; 3 when the file cannot be read or the
  change would make it invalid (nothing is written); 1 when the file cannot be written.
- **Running instances read settings.json only when they start.** After a change under `defaults`,
  `set` and `unset` list every running instance on stderr (`运行中的实例要重启才用上这次改动:`,
  "running instances need a restart to use this change", then one `aproxy restart <port>` line each).
  Changes outside `defaults` need no restart.
- **`--json`** prints one document on stdout: `show` gives
  `{"schema":1,"file","values","diagnostics"}` (the document comes first even when the file has
  errors, then the exit code is 3); `get` gives `{"schema":1,"key","value"}`; `set` and `unset` give
  `{"schema":1,"file","key","value","changed","instances_to_restart":[ports]}`, where `value` is the
  key's effective value after the write (after `unset`, that can be the built-in default). Failures
  give `{"schema":1,"error":{…}}` with `usage`, `config_invalid` or `write_conflict`.
- `aproxy alias add|remove` and `aproxy settings set|unset default_config` remain the commands for
  their fields; all go through the same writer and touch only their own key.

Example: give every instance a global default without editing JSON by hand.

```sh
aproxy settings set defaults.max_request_mb 128
aproxy settings set defaults.retry.rules '[{"path": "/v1/messages/count_tokens(\\?.*)?", "max_attempts": 3}]'
aproxy settings get defaults.max_request_mb
```
