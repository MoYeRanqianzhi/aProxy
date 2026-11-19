# settings.json reference

`settings.json` is aProxy's machine-wide state for one home: the file format version, the output
language, aliases, the default config, the directories `find` and `doctor` scan, global defaults for
config.toml keys, and the settings for the watchdog, log rotation, idle detection and
`aproxy install`. Edit it with `aproxy settings show|get|set|unset`; read it to add a global default,
tune the watchdog, or repair a file that stops commands with exit 3. The keys under `defaults` are
config.toml keys and follow the syntax in config-toml.md.

## Contents

- [Location and editing](#location-and-editing)
- [File shape](#file-shape)
- [Field summary](#field-summary)
- [aliases](#aliases) · [default_config](#default_config) · [config_dirs](#config_dirs)
- [defaults: global defaults for config.toml](#defaults-global-defaults-for-configtoml)
- [When settings.json is invalid](#when-settingsjson-is-invalid)
- [logs.rotate_mb](#logsrotate_mb) · [instances.idle_threshold_secs](#instancesidle_threshold_secs)
- [watchdog](#watchdog) · [Watchdog tuning](#watchdog-tuning)
- [update.download_chain](#updatedownload_chain) · [update.download_proxy](#updatedownload_proxy) ·
  [update.skill_auto_update](#updateskill_auto_update) · [ui](#ui)

## Location and editing

- One file per aProxy home, `<APROXY_HOME>/settings.json` (`~/.aproxy/settings.json` unless
  `APROXY_HOME` is set), shared by every instance under that home. It may hold API keys and proxy
  passwords in plain text, so it is written with owner-only permissions: mode `0600` on Unix, and on
  Windows a DACL granting only the current user, SYSTEM and Administrators.
- **Edit it with `aproxy settings`.** Keys are dotted paths, exactly as they appear in the file:
  `watchdog.enabled`, `aliases.work`, `defaults.timeouts.connect_secs`. A segment made only of
  letters, digits, `_` and `-` is written bare; any other segment, such as a header name with a dot,
  is quoted: `defaults.extra_headers."x.foo"`.

  | Command | Effect |
  |---|---|
  | `aproxy settings show` | print every effective value: the file's keys over the built-in defaults, secrets masked. Exits 3 after printing if the file has errors |
  | `aproxy settings get KEY` | print one effective value as JSON; a key that is neither stored nor built in prints `null`. It reads the file as it is, so it answers even when the file has errors |
  | `aproxy settings set KEY VALUE` | set one key. Only keys this version knows can be set |
  | `aproxy settings unset KEY` | remove one key, back to its built-in default. Any key can be removed, including one this version does not know |

- **Values.** `set` takes a JSON value (`true`, `30`, `["a"]`, `{"k":1}`). For keys whose value is a
  string the raw text is accepted too, so `aproxy settings set language zh-CN` needs no quotes; a
  value starting with `"` is read as a JSON string literal, which is how to write a newline. A value
  of `-` is read from one line of stdin, which keeps a key out of the shell history:
  `printf '%s\n' "$KEY" | aproxy settings set defaults.api_key -`.
- **Secrets are never printed back.** `show` and `get` replace a secret with
  `{"secret": true, "hint": "sk-a…f3a9"}` (the hint appears when the value is at least 24 characters)
  and mask URL credentials (`https://***@host/…?key=***`). Writing that placeholder object back
  keeps the stored value; a placeholder with no stored value to keep is refused.
- **Every write is checked before it lands.** A change that would introduce an error is refused and
  the file is left as it was; errors already in the file that this change does not touch do not block
  it, so a broken file can be repaired one key at a time. Writes take a lock shared with the other
  aproxy writers, keep every other key and its order, and create the file (with `"schema": 1` first)
  when it does not exist. The file is re-serialized as pretty JSON, so hand-made spacing and line
  breaks are not preserved. Two commands never lose each other's updates.
- `aproxy alias add|remove` and `aproxy settings set|unset default_config` edit only their own key
  through the same writer. They no longer rewrite the file wholesale, so keys they
  do not know are no longer dropped; a key this version does not know is an error until you remove it.
- **Hand editing is the fallback, and it is strict.** Read by hand only when the file cannot be parsed
  at all: a JSON syntax error, a file that is not a JSON object, or a `schema` newer than this
  version. In those cases no command can rewrite it, and `settings` says so instead of guessing.
  Otherwise prefer `aproxy settings set`, which stores the value in the shape this version expects.
  After any hand edit run `aproxy settings show` or `aproxy doctor`.
- Any key not documented here is an error (`config_unknown_key`), not a value that is ignored. That
  includes the flat field names of older releases; compatibility.md maps them to these keys.

The settings command's own exit codes: 0 stored, unchanged, or printed; 2 a malformed key path or a
value that is not valid JSON; 3 the change would make the file invalid, or the file cannot be read
(syntax error, not an object, newer schema); 1 the file could not be written (I/O or lock conflict).

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
| `ui` | object | `{}` | kept for the desktop window; the CLI validates only that it is an object |

Integers are non-negative. A wrong type, an unknown key, a value outside its allowed set, a missing
required key and a newer `schema` are all errors, reported with the file, the key path, line:column
when the JSON parser has a position (syntax errors), and a stable code in brackets, for example
`<file>: watchdog.heartbeat_secs: watchdog.heartbeat_secs must be a non-negative integer [config_type]`.

## aliases

Names for config files, so `aproxy start|stop|restart|logs NAME` work without a path or port. The
recommended way to change the table is `aproxy alias`.

```json
{ "aliases": { "work": "C:/Users/me/.aproxy/configs/work.toml" } }
```

(In JSON, a Windows path needs its backslashes doubled; forward slashes work as well and are easier
to read.)

- Add with `aproxy alias add NAME [PATH]`: the file must exist, `~` is expanded and the path is
  stored absolute. Without `PATH` the alias points to `<APROXY_HOME>/config.toml`.
- An alias finds its running instance by config path, not by port, so it keeps working after the
  port in that config changes.
- Not allowed as names: a port number (0 to 65535), and the reserved words `all`, `idle`, `default`
  and its misspelling `defult`, in any case, because `start`/`stop` would read them as a port or a
  reserved target. `alias add` refuses them; one written by hand is an error on every run.
- An alias whose file has gone is reported as `别名 "NAME" 指向的配置文件不存在` ("alias NAME points to a
  config file that does not exist"). That is the state of the home, not a file error: commands still
  run, and `doctor` lists it.
- `aproxy alias list` prints the names and paths; `settings get aliases` prints the whole table.

## default_config

The config used when a command is given neither `--config` nor a target: `aproxy start`,
`aproxy config`, and the reserved target `default`. Unset, it is `<APROXY_HOME>/config.toml`. Set it
with `aproxy settings set default_config PATH` (the file must exist; the absolute path is stored) to
make another file your everyday config.

If the file it names is deleted or moved, the commands that would read it (`aproxy start` without a
target, and `aproxy config` when showing or editing the toml) exit with
`settings.json 指定的默认配置文件不存在: <path>` ("the default config named in settings.json does not
exist") instead of silently using a different file. Other commands are unaffected. Restore the file,
point the setting elsewhere with `aproxy settings set default_config PATH`, or remove it with
`aproxy settings unset default_config`.

## config_dirs

Extra directories that `aproxy find` and `aproxy doctor` scan for `*.toml` files (top level only,
not subdirectories). `<APROXY_HOME>` and `<APROXY_HOME>/configs/` are always scanned; listing them
again is harmless. `~` is expanded. A listed directory that does not exist is a `doctor` warning,
`配置目录不存在` ("config directory does not exist").

## defaults: global defaults for config.toml

`defaults` holds config.toml keys under their config.toml names and nesting, written as JSON: an
object for a TOML table, an array for an array, an array of objects for an array of tables. Every
instance that does not set the key in its own config.toml takes this value. A key written in
config.toml always wins; the full precedence is in config-toml.md, "Precedence". A running instance
keeps the values it loaded until it restarts.

```json
{
  "defaults": {
    "base_url": "https://api.example.com",
    "max_request_mb": 128,
    "delivery": "retry",
    "allowed_origins": ["http://localhost:5173"],
    "retry": {
      "rules": [
        { "path": "/v1/messages/count_tokens(\\?.*)?", "max_attempts": 3 }
      ]
    },
    "extra_headers": { "x-team": "core" }
  }
}
```

- **Three keys cannot have a global default:** `listen_addr`, `log_file` and `format`. They belong
  to one instance, so two instances cannot share them; set them in that instance's config.toml. A
  value for them here is `config_unknown_key`.
- **Values are checked exactly like config.toml.** An invalid URL, regex, trigger or type is an error
  pointing at `defaults.<key>` in settings.json, and every instance that inherits it refuses to
  start. A default that is wrong therefore stops instances that were fine before; check a new global
  default with `aproxy config show --effective` on one instance before relying on it.
- **`aproxy config show --effective` names this layer.** A value inherited from here prints
  `（settings.json 全局默认）` ("settings.json global default"); one from the file prints `（config.toml）`,
  and one nothing set prints `（内置默认）` ("built-in default"). Read `show --effective` first when
  you need to know which layer a value comes from.
- In JSON, double every backslash: the regex `\?` is written `"\\?"`.

### Credentials in defaults follow their target

A credential must not reach an upstream it was not meant for, so `defaults` is not inherited blindly:

- `defaults.api_key` and the secret headers in `defaults.extra_headers` / `defaults.override_headers`
  are inherited only by an instance whose final `base_url` points at the same target as
  `defaults.base_url`. "Same target" compares the normalized URL: trimmed, trailing `/` dropped,
  scheme and host lowercased, a default port (`:80` for http, `:443` for https) dropped. **The path
  is compared too** — gateways that route different vendors by path under one host must not receive
  each other's keys. Userinfo is ignored, because it is a credential, not part of the target.
- `defaults.proxy_username` and `defaults.proxy_password` are inherited only when the instance's
  final `proxy` names the same scheme, host and port as `defaults.proxy`.
- Everything else is inherited as usual: all non-credential defaults, and headers that are not
  secrets.

When an instance's target differs, `aproxy config show --effective` lists each withheld item after the values:

```
全局默认里的这些凭据没有继承（它们属于另一个目标）:
  api_key  默认值属于 https://api.example.com，这份配置指向 https://relay.b.example
```

("these credentials in the global defaults were not inherited: they belong to another target"). If
the instance also has no credential of its own, the command adds a warning with code
`default_credential_withheld` naming the key to set, because otherwise the upstream would answer 401
and nothing would say why.

**A credential in `defaults` needs its target:** `defaults.api_key` or a secret header without
`defaults.base_url`, and a proxy password without `defaults.proxy`, is an error
(`config_missing_key`) that says which key to set first. This keeps a key from being sent to every
upstream. Set the target first:

```sh
aproxy settings set defaults.base_url https://api.example.com
printf '%s\n' "$KEY" | aproxy settings set defaults.api_key -
```

**Which headers count as credentials** is decided by name: `cookie`, `set-cookie`, or any name
containing `key`, `token`, `secret`, `auth`, `session` or `signature` (case-insensitive). A header
with an unusual name, such as `x-vendor-credential`, is **not** recognized: it is inherited by every
target and sent to an upstream it may not belong to. If an upstream needs a credential header like
that, put it in that instance's config.toml instead of `defaults`.

## When settings.json is invalid

settings.json is read strictly, because a silently ignored key would leave a user believing a setting
is in effect. Unknown keys, wrong types, invalid values and a higher `schema` are errors. The policy
for a command that meets them depends on what the command is for:

| Command | Behavior with an invalid settings.json |
|---|---|
| `aproxy settings …`, `aproxy doctor` | Run normally: they are how the file is inspected and repaired. `doctor` lists the errors; `settings show` still prints the effective values, then the errors, and exits 3 |
| `aproxy status`, `aproxy stop`, `aproxy install` / `upgrade` | Run on built-in defaults and print the errors as `[WARN]` lines. Aliases are unavailable: address instances by port or `all`. Exit with the command's normal code |
| every other command: `start`, `restart`, `logs`, `find`, `restore`, `config`, `alias`, `desktop` | Print the errors as `[ERROR]` lines with repair hints and exit 3, without doing anything. A started instance never runs with settings it did not get |

The warning form ends with:

```
[WARN] 本次按内置默认运行（别名不可用，请按端口或 all 指定实例）。用 aproxy settings show 查看、aproxy settings unset <键> 修复
```

("running on built-in defaults this time; aliases are unavailable, address instances by port or all.
Inspect with aproxy settings show and repair with aproxy settings unset KEY").

The error form adds a repair command for each bad key and one way out of the whole file:

```
settings.json 有错误:
  <file>: watchdog: watchdog must be an object [config_type]
  <file>: max_body_mb: unknown key max_body_mb [config_unknown_key]

逐项修复（删掉出问题的键，或用 aproxy settings set <键> <值> 改对）:
  aproxy settings unset 'watchdog'
  aproxy settings unset 'max_body_mb'
也可以删除整个文件回到内置默认（别名与全局默认会一并丢失）: <file>
```

("settings.json has errors; fix them one by one by removing the key or setting it correctly with
aproxy settings set; or delete the whole file to return to the built-in defaults, losing aliases and
global defaults"). Deleting the file is the crude repair: it also loses `aliases`, `default_config`
and `defaults`, so tell the user before you do it, and prefer `settings unset` for each bad key.

- **A file that cannot be parsed at all** — a JSON syntax error, or content that is not a JSON object
  — can only be repaired by hand or deleted. `settings show` prints
  `settings.json 不是合法的 JSON 对象，只能手工修复（或删除它，回到内置默认）:` ("not a valid JSON object;
  repair it by hand or delete it to return to the built-in defaults") and exits 3, and every write
  command refuses and leaves the file untouched (never "rewrite it with the defaults", which would
  lose the aliases).
- A file that is valid JSON with bad keys is repaired one key at a time; `settings unset` accepts
  the unknown key, and each later edit is checked against the file as it stands.
- A `schema` from a newer aProxy is a separate case: no key can be fixed, and the error says to
  upgrade aproxy, or to go back to the version you need with `aproxy install --force <version>`. Run
  that command only after the user agrees: it stops every aProxy process of the home.

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

Reserved for the desktop window's preferences; this version does not read it. The CLI only checks
that `ui` is an object, and everything inside is opaque and preserved through every `settings` write,
so the window can add a preference without an older command line refusing to start. Put UI
preferences here rather than at the top level: any top-level key not listed above is an error.
