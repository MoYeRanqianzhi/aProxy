# settings.json reference

`settings.json` holds aProxy's machine-wide state: aliases, the default config, global defaults
that seven config.toml fields fall back to, and settings for log rotation, idle detection, the
watchdog and `aproxy install`. Read it to add or repair an alias or the default config, to set a
default for every instance, or to tune the watchdog or install downloads. Fields shared with
config.toml are explained in config-toml.md.

## Contents

- [Location and editing](#location-and-editing)
- [Field summary](#field-summary)
- [aliases](#aliases) · [default_config](#default_config) · [config_dirs](#config_dirs)
- [Global defaults for config.toml fields](#global-defaults-for-configtoml-fields)
- [log_rotate_mb](#log_rotate_mb) · [idle_timeout_secs](#idle_timeout_secs)
- [watchdog](#watchdog) · [Watchdog tuning](#watchdog-tuning)
- [download_chain](#download_chain) · [download_proxy](#download_proxy) ·
  [skill_auto_update](#skill_auto_update)

## Location and editing

- One file per aProxy home, `<APROXY_HOME>/settings.json` (`~/.aproxy/settings.json` unless
  `APROXY_HOME` is set), shared by every instance under that home.
- Two fields have commands: `aliases` (`aproxy alias add|remove|list`) and `default_config`
  (`aproxy config --set-default PATH` / `--clear-default`). Edit the others by hand; a field left
  out takes its default.
- Edit by hand with care, because the file is read as a whole. A JSON syntax error, a value of the
  wrong type (`"watchdog": "false"`) or an unknown name in `download_chain` makes the entire file
  unreadable. Read-only commands then print
  `警告: settings.json 解析失败（…），别名等内部配置已回退为空` ("warning: settings.json failed to parse
  (…); aliases and other internal settings fell back to empty") and run on built-in defaults, so no
  alias resolves. Commands that write the file (`alias add|remove`,
  `config --set-default|--clear-default`) refuse and exit 1 with
  `settings.json 解析失败（…），为免覆盖其中的别名等内容，本次不做修改。请先修复或删除该文件: <path>`
  ("settings.json failed to parse (…); not modified, to avoid overwriting the aliases and other
  content in it. Fix or delete the file first").
- Run `aproxy doctor` right after a hand edit. Every other aproxy command also checks the JSON, the
  aliases, `bounded_retry_paths` and `keepalive_trigger`, printing problems to stderr as `[ERROR] …`
  lines without stopping, so `status` and `stop` stay usable; only `doctor` checks the watchdog
  fields.
- A command that writes the file saves every field at its current value and drops keys it does not
  know. A file listing fields you never set is normal.

## Field summary

The last column says when a change takes effect. "Instance start" means running instances keep the
old value until `aproxy restart PORT|ALIAS`.

| Field | Type | Default | Changed by | Takes effect |
|---|---|---|---|---|
| `aliases` | object, name to path | `{}` | `aproxy alias` | next command |
| `default_config` | string or `null` | `null` | `aproxy config --set-default` / `--clear-default` | next command |
| `config_dirs` | array of paths | `[]` | hand | next `find` / `doctor` |
| `max_body_mb` | integer | `128` | hand | instance start |
| `disk_cache` | boolean | `true` | hand | instance start |
| `forward_only` | boolean | `false` | hand | instance start |
| `bounded_retry_paths` | array of regex strings | `[]` | hand | instance start |
| `allowed_hosts` | array of strings | `[]` | hand | instance start |
| `allowed_origins` | array of strings | `[]` | hand | instance start |
| `keepalive_trigger` | string | `"any"` | hand | instance start |
| `log_rotate_mb` | integer | `8` | hand | instance start |
| `idle_timeout_secs` | integer | `1800` | hand | next `status` / `stop` / `restart` |
| `watchdog` | boolean | `true` | hand | see [watchdog](#watchdog) |
| `watchdog_heartbeat_secs` | integer | `30` | hand | watchdog start |
| `watchdog_stale_after_cycles` | integer | `1` | hand | watchdog start |
| `watchdog_max_restarts` | integer | `5` | hand | watchdog start |
| `watchdog_idle_exit_secs` | integer | `300` | hand | watchdog start |
| `download_chain` | array or `null` | `null` (built-in chain) | hand | next install |
| `download_proxy` | string or `null` | `null` | hand | next install |
| `skill_auto_update` | boolean | `true` | hand | next install |

## aliases

Names for config files, so `aproxy start|stop|restart|logs NAME` work without a path or port.

```json
"aliases": { "work": "C:\\Users\\me\\.aproxy\\configs\\work.toml" }
```

- Add with `aproxy alias add NAME [PATH]`: the file must exist, `~` is expanded and the path is
  stored absolute. Without `PATH` the alias points to `<APROXY_HOME>/config.toml`.
- An alias finds its running instance by config path, not by port, so it keeps working after the
  port in that config changes.
- Not allowed as names: a port number (0 to 65535), and the reserved words `all`, `idle`, `default`
  and its misspelling `defult`, in any case, because `start`/`stop` would read them as a port or a
  reserved target. `alias add` refuses them; one written by hand is reported as an error on every
  run.
- An alias whose file has gone is reported as `别名 "NAME" 指向的配置文件不存在` ("alias NAME points to a
  config file that does not exist").

## default_config

The config used when a command is given neither `--config` nor a target: `aproxy`,
`aproxy start`, `aproxy config`, and the reserved target `default`. Unset, it is
`<APROXY_HOME>/config.toml`. Set it with `aproxy config --set-default PATH` (the file must exist; the
absolute path is stored) to make another file your everyday config.

If the file it names is deleted or moved, the commands that would read it (`aproxy` or
`aproxy start` without a target, and `aproxy config` when showing or editing the toml) exit with
`settings.json 指定的默认配置文件不存在: <path>` ("the default config named in settings.json does not
exist") instead of silently using a different file. Other commands are unaffected. Restore the file,
point the setting elsewhere with `aproxy config --set-default PATH`, or remove it with
`aproxy config --clear-default`.

## config_dirs

Extra directories that `aproxy find` and `aproxy doctor` scan for `*.toml` files (top level only,
not subdirectories). `<APROXY_HOME>` and `<APROXY_HOME>/configs/` are always scanned; listing them
again is harmless. `~` is expanded. A listed directory that does not exist is a `doctor` warning,
`配置目录不存在` ("config directory does not exist").

## Global defaults for config.toml fields

`max_body_mb`, `disk_cache`, `forward_only`, `bounded_retry_paths`, `allowed_hosts`,
`allowed_origins` and `keepalive_trigger` apply to every instance whose config.toml does not set the
same key. A key present in config.toml always wins (config-toml.md, Precedence). Values and meaning
are the same as the config.toml fields of the same name; see config-toml.md for each.

```json
{
  "allowed_origins": ["http://localhost:5173"],
  "bounded_retry_paths": ["/v1/messages/count_tokens", "/v1/messages/count_tokens\\?.*"]
}
```

- In JSON, double every backslash: the regex `\?` is written `"\\?"`.
- Use this layer for policies that should hold on every instance, such as the Origin of a desktop
  client used with all of them. Leave `forward_only` false here: `true` would remove retries from
  every instance that does not set it back.
- An invalid `bounded_retry_paths` regex or `keepalive_trigger` value here stops every instance that
  inherits it from starting. `aproxy doctor` and the per-command check report it as `[ERROR]`, with
  `（来源 settings.json；start 该项全局默认的实例会失败）` ("source settings.json; instances that use this
  global default will fail to start").
- `aproxy config --show` does not print the settings.json values; for a field the toml leaves unset
  it prints
  `（未在 toml 设置，运行时取 settings.json 全局默认）` ("not set in the toml; the settings.json global
  default applies at runtime").

## log_rotate_mb

Size limit for each instance's daemon log, in MB (default 8). Each instance checks its log every
hour and empties it once it is larger; no rotated copies are kept, and `aproxy logs` keeps
following the emptied file. `0` = never. Applies to custom `log_file` paths too (config-toml.md,
log_file).

## idle_timeout_secs

How long an instance must go without activity to count as idle, in seconds (default 1800). Used by
`aproxy status --idle` / `--busy` and `aproxy stop idle` / `aproxy restart idle`; a number after
`idle` overrides it for that one command.

Activity is a new client request, the start of each retry, and in `forward_only` mode each relayed
chunk. A single upstream attempt that runs longer than the threshold without a retry, such as a very
long generation, counts as idle, and `stop idle` would cut it off. Keep the threshold above your
longest single generation before using `stop idle`.

## watchdog

`true` (the default) keeps one watchdog process per aProxy home. It watches every instance and
restarts one that crashes or hangs, with that instance's original start arguments. `aproxy start`
launches the watchdog if none is running, and every instance checks every 5 minutes and relaunches
it if it has gone.

`false` stops new launches by `aproxy start` and by instances started after the change. It does
not stop a watchdog that is already running: that one keeps working until it exits by itself
(`watchdog_idle_exit_secs` after the last instance is gone), and instances started while the field
was `true` keep relaunching it until they are restarted. Behavior: behaviors.md, "Crash recovery and the watchdog".

## Watchdog tuning

The watchdog reads these when its process starts; a running watchdog keeps its values.

| Field | Default | Meaning | Choosing a value |
|---|---|---|---|
| `watchdog_heartbeat_secs` | `30` | Scan period. An instance whose heartbeat is older than `watchdog_heartbeat_secs × (watchdog_stale_after_cycles + 1)` (60 s with the defaults) and that does not answer an IPC ping is killed and restarted. | Lower detects hangs sooner at the cost of more wake-ups. `0` is a `doctor` error (it runs as 1 s); above `600` is a `doctor` warning. |
| `watchdog_stale_after_cycles` | `1` | Extra scan periods a heartbeat may lag before the ping check. | Raise it if healthy instances on a heavily loaded machine get killed. `0` is a `doctor` error (it runs as 1). |
| `watchdog_max_restarts` | `5` | Failed restarts in a row, for one instance, before the watchdog gives up. The first restart is immediate; after a failure it waits 1 s, 2 s, 4 s ... up to 300 s. After giving up, the instance's restore record is kept, so `aproxy restore` can bring it back. | `0` = never restart, only observe (`doctor` warning). |
| `watchdog_idle_exit_secs` | `300` | How long the watchdog stays after the last instance is gone before exiting. | `0` = stay resident. |

## download_chain

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

URL template placeholders: `{version}` (the bare version, such as `0.1.0`, no `v`), `{asset}` (the
release file name, such as `aproxy-x86_64-pc-windows-msvc-v3.exe`, or `aproxy-skills.zip` for the
skill files), `{target}` (the Rust target triple) and `{variant}` (`-v3` or empty). When
`<url>.sha256` exists the download is checked against it; otherwise it is accepted with
`[警告] 来源为非官方镜像（url 模板），产物未经独立校验` ("warning: unofficial mirror (URL template);
artifact not independently verified").

```json
"download_chain": [
  { "url": "https://mirror.example.com/aproxy/v{version}/{asset}" },
  "github",
  "npm"
]
```

A misspelled source name makes the whole file unreadable (see
[Location and editing](#location-and-editing)).

## download_proxy

Proxy URL for `aproxy install` downloads only; proxied API traffic uses config.toml `proxy`
instead. Precedence: `aproxy install --download-proxy URL` > this field > the environment's proxy
variables. Credentials in the URL are masked in error output. An install that resumes by itself
after an interruption has no command line and uses only this field, so set it here when downloads
always need a proxy.

## skill_auto_update

`true` (the default): `aproxy install` also updates the skill files under `<APROXY_HOME>/skills/`,
in parallel with the binary; a failed skill update does not fail the install. `false`: skip them.
For one run, `aproxy install --no-skills` skips them and `aproxy install --skills-only` updates only
them, even when this field is `false`.
