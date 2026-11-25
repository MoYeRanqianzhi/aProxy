# settings.json reference

`settings.json` is aProxy's machine-wide state for one home: the file format version, the output
language, aliases, the default config, the directories `find` and `doctor` scan, global defaults for
config.toml keys, and the settings for the watchdog, log rotation, idle detection and
`aproxy install`. Edit it with `aproxy settings show|get|set|unset`; read it to add a global default,
tune the watchdog, or repair a file that stops commands with exit 3. The keys under `defaults` are
config.toml keys and follow the syntax in config-toml.md.

## Parts

This reference is split by topic into the files below (paths relative to this file). Open only the part the task needs; a reference elsewhere to `settings-json.md, "<section>"` means the part that holds that section.

| File | Covers | Sections |
|---|---|---|
| [settings-json/editing.md](settings-json/editing.md) | Where settings.json lives, how to edit it, and what happens when it is invalid. | [Location and editing](settings-json/editing.md#location-and-editing) · [When settings.json is invalid](settings-json/editing.md#when-settingsjson-is-invalid) |
| [settings-json/instances.md](settings-json/instances.md) | `aliases`, `default_config`, `config_dirs` and the global `defaults` for config.toml. | [aliases](settings-json/instances.md#aliases) · [default_config](settings-json/instances.md#default_config) · [config_dirs](settings-json/instances.md#config_dirs) · [defaults: global defaults for config.toml](settings-json/instances.md#defaults-global-defaults-for-configtoml) · [Credentials in defaults follow their target](settings-json/instances.md#credentials-in-defaults-follow-their-target) |
| [settings-json/runtime.md](settings-json/runtime.md) | `logs.rotate_mb`, `instances.idle_threshold_secs`, the watchdog, update settings and `ui`. | [logs.rotate_mb](settings-json/runtime.md#logsrotate_mb) · [instances.idle_threshold_secs](settings-json/runtime.md#instancesidle_threshold_secs) · [watchdog](settings-json/runtime.md#watchdog) · [Watchdog tuning](settings-json/runtime.md#watchdog-tuning) · [update.download_chain](settings-json/runtime.md#updatedownload_chain) · [update.download_proxy](settings-json/runtime.md#updatedownload_proxy) · [update.skill_auto_update](settings-json/runtime.md#updateskill_auto_update) · [ui](settings-json/runtime.md#ui) |

## File shape

Keys are grouped by subject. The sample below writes out the built-in defaults, so a file that
contains it behaves exactly like no file at all:

```json
{
  "schema": 1,
  "language": "system",
  "default_config": null,
  "aliases": {},
  "config_dirs": [],
  "defaults": {},
  "watchdog": {
    "enabled": true,
    "heartbeat_secs": 30,
    "stale_after_cycles": 1,
    "max_restarts": 5,
    "idle_exit_secs": 300
  },
  "logs": { "rotate_mb": 8 },
  "instances": { "idle_threshold_secs": 1800 },
  "update": {
    "download_chain": null,
    "download_proxy": null,
    "skill_auto_update": true
  },
  "ui": {}
}
```

Every key is optional; leave one out to take its built-in default. `schema` may be omitted and is
read as 1; any write adds it as the first key. A file written by a newer aProxy, with a higher
`schema`, is refused rather than half-read: upgrade aproxy, or go back with
`aproxy install --force <version>`.

## Field summary

"When it takes effect" says when an already running instance picks up the change. "Instance start"
means running instances keep the old value until `aproxy restart PORT|ALIAS`.

| Key | Type | Default | When it takes effect |
|---|---|---|---|
| `schema` | integer, `1` | `1` | next command |
| `language` | `"system"`, `"en"` or `"zh-CN"` | `"system"` | next command; this version only validates it and keeps printing Chinese |
| `aliases` | object, name to path | `{}` | next command |
| `default_config` | string or `null` | `null` | next command |
| `config_dirs` | array of paths | `[]` | next `find` / `doctor` |
| `defaults` | object of config.toml keys | `{}` | instance start |
| `watchdog.enabled` | boolean | `true` | next watchdog scan (a running watchdog exits) |
| `watchdog.heartbeat_secs` | integer | `30` | watchdog start |
| `watchdog.stale_after_cycles` | integer | `1` | watchdog start |
| `watchdog.max_restarts` | integer | `5` | next watchdog scan |
| `watchdog.idle_exit_secs` | integer | `300` | next watchdog scan |
| `logs.rotate_mb` | integer | `8` | instance start |
| `instances.idle_threshold_secs` | integer | `1800` | next `status` / `stop` / `restart` |
| `update.download_chain` | array or `null` | `null` (built-in chain) | next install |
| `update.download_proxy` | string or `null` | `null` | next install |
| `update.skill_auto_update` | boolean | `true` | next install |
| `ui` | object | `{}` | not used; the CLI checks only that it is an object and keeps its content |

Integers are non-negative. A wrong type, an unknown key, a value outside its allowed set, a missing
required key and a newer `schema` are all errors, reported with the file, the key path, line:column
when the JSON parser has a position (syntax errors), and a stable code in brackets, for example
`<file>: watchdog.heartbeat_secs: watchdog.heartbeat_secs must be a non-negative integer [config_type]`.
