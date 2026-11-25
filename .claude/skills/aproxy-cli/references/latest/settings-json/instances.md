# settings.json reference: aliases, config discovery and defaults

Part of [settings-json.md](../settings-json.md), which lists the other parts. `aliases`, `default_config`, `config_dirs` and the global `defaults` for config.toml.

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
