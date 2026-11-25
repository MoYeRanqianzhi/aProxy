# Version compatibility: keys renamed in 0.2.0

Part of [compatibility.md](../compatibility.md), which lists the other parts. The old-to-new key tables for settings.json and config.toml, for editing by hand.

## settings.json keys renamed in 0.2.0

`aproxy migrate` applies this table; edit by hand only what it leaves for review. A flat 0.1.x
file is rejected field by field: every old top-level key is reported as `config_unknown_key`, `watchdog: true|false` as
`config_type` ("watchdog must be an object"), and commands that need settings stop with exit 3
(settings-json.md, "When settings.json is invalid"). `aproxy settings show`, `aproxy doctor` and the
other tolerated commands still run, so you can inspect the file and repair it one key at a time;
`aproxy settings unset` accepts the old keys even though this version does not know them.

Rewrite each old key with the table below. Keep `aliases`, `default_config` and `config_dirs` where
they are: their names and shapes did not change.

| 0.1.x settings.json | 0.2.0 |
|---|---|
| `max_body_mb = N` | `defaults.max_request_mb = N` (`0` = no limit) |
| `disk_cache = true\|false` | `defaults.disk_cache` |
| `forward_only = true\|false` | `defaults.delivery = "forward"\|"retry"` |
| `bounded_retry_paths = ["..."]` | `defaults.retry.rules`: one entry per pattern, `{ "path": "<pattern>", "max_attempts": 3 }` (0.1.x stopped after three attempts, so keep 3 to preserve behavior) |
| `allowed_hosts = [...]` | `defaults.allowed_hosts` |
| `allowed_origins = [...]` | `defaults.allowed_origins` |
| `keepalive_trigger = "..."` | `defaults.keepalive.trigger` |
| `watchdog = true\|false` | `watchdog.enabled` (the other watchdog fields move into the same object) |
| `watchdog_heartbeat_secs = N` | `watchdog.heartbeat_secs` |
| `watchdog_stale_after_cycles = N` | `watchdog.stale_after_cycles` |
| `watchdog_max_restarts = N` | `watchdog.max_restarts` |
| `watchdog_idle_exit_secs = N` | `watchdog.idle_exit_secs` |
| `log_rotate_mb = N` | `logs.rotate_mb` |
| `idle_timeout_secs = N` | `instances.idle_threshold_secs` |
| `download_chain = [...]` | `update.download_chain` |
| `download_proxy = "..."` | `update.download_proxy` |
| `skill_auto_update = true\|false` | `update.skill_auto_update` |

Two behaviors differ from a pure rename:

- **A global `api_key` now needs a target.** `defaults.api_key` (and with it any secret header in
  `defaults.extra_headers`) is an error without `defaults.base_url`, because the key would otherwise
  be inherited by every instance and sent to every upstream. Set `defaults.base_url` first. After
  that, only instances whose own `base_url` names the same target inherit the key; an instance that
  points elsewhere keeps working but sends no key unless its own config.toml sets one (settings-json.md,
  "Credentials in defaults follow their target").
- **0.1.x had no `language` or `ui`.** Both are optional; leave them out and take the built-in
  defaults (`"system"` and `{}`).

Convert the whole file before starting 0.2.0: a mixture of old and new keys is still invalid for the
new CLI (the old keys are unknown), and 0.1.x cannot read the new sections either. To edit by hand
with the 0.2.0 CLI, for example:

```sh
aproxy settings unset max_body_mb
aproxy settings set defaults.max_request_mb 128
aproxy settings unset forward_only
aproxy settings set defaults.delivery '"retry"'
aproxy settings unset watchdog
aproxy settings set watchdog.enabled false
```

`settings unset` removes an old key even though this version does not know it; `settings set` rejects
a key this version does not know. A change that would introduce a new error is refused, so do the
`watchdog` object in the order shown (remove the old boolean, then set the section). `settings show`
names whatever is still wrong, and `doctor` lists it with the rest of the home's problems.

## Config keys renamed in 0.2.0

config.toml is parsed strictly from 0.2.0 on. An old key is not ignored: it is reported as
`config_unknown_key`, naming the file, the key path and line:column, and the instance does not
start. Convert each file before the first start with the new CLI. `aproxy migrate` applies every
row except the transformer tables and format `extra`, which it leaves for the user.

| 0.1.x config.toml | 0.2.0 |
|---|---|
| `max_body_mb = N` | `max_request_mb = N` (`0` = no limit) |
| `spool_limit_mb = N` | `max_response_mb = N` |
| `forward_only = true\|false` | `delivery = "forward"\|"retry"` (default `"retry"`) |
| `connect_timeout_secs = N` | `[timeouts] connect_secs = N` |
| `read_timeout_secs = N` | `[timeouts] first_byte_secs = N` for the wait for response headers, and `[timeouts] read_idle_secs = N` for the gap between body reads; both default 300, and they now report `upstream_first_byte_timeout` and `upstream_read_idle_timeout` separately |
| `keepalive_interval_secs = N` | `[keepalive] interval_secs = N` |
| `keepalive_trigger = "..."` | `[keepalive] trigger = "..."`; `"always"` is rejected in the file (only a format may ask for it per request) |
| `keepalive_heartbeat = "..."` | `[keepalive] heartbeat = "..."` |
| `max_retry_backoff_secs = N` | `[retry] max_backoff_secs = N` |
| `bounded_retry_paths = ["..."]` | `[[retry.rules]]` entries with `path` (regex) and `max_attempts` (required; `0` = no limit), plus optional `status` |
| `[request_transform]`, `[response_transform]`, `[heartbeat_transform]` tables | `[[format]]` array entries with `command`, `args`, `process`, and `stages` (config-toml.md, format); `pool_max` and `timeout_secs` keep their names. The program itself must speak protocol 1; the official `aproxy-format run` command is replaced by one subcommand per role |
| format `mode = "spawn"\|"persistent"` | `process = "spawn"\|"persistent"` |
| format `idle_timeout_secs = N` | `worker_idle_secs = N` |
| format `every_attempt = true` | add `"retry"` to `stages` next to `"request"` |
| format `extra = "..."` | pass the data through `args` or `env` |

The 0.1.x `upstream_url` name is older still and also no longer read; it is `base_url` in 0.2.0 and
in 0.1.0's stable line (the "What these docs describe" table in [versions.md](versions.md)).

To edit by hand, changing one key at a time with `aproxy config set KEY VALUE` and
`aproxy config unset OLD` is safe: an unknown key does not block unrelated edits, and `unset`
accepts any key, including one this version does not know. After the renames, `aproxy config show`
prints the file's values and exits 3 if anything is still wrong.

If the binary was replaced while 0.1.x instances were still running (their ports stay busy, and
`aproxy start` reports the port in use by another program), stop them with the 0.1.x binary they run from, under the same home: get it back (the copy from
step 2 of [from-0.1.md](from-0.1.md), or the v0.1.0 release asset saved under another name outside
`<home>/bin/`, so the 0.2.0 binary stays in place) and, once the user agrees, run `<that binary> stop <port>` (or
`stop all`). The current CLI does not list 0.1.x instances, since their records have another name,
and cannot reach their control channel, so `aproxy stop --force` and `aproxy install --force` do
not end them either (`install --force` only lists them as processes it did not stop). Then continue
from step 5 in [from-0.1.md](from-0.1.md).
