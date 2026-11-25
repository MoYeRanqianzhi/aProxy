# config.toml reference: working with config files

Part of [config-toml.md](../config-toml.md), which lists the other parts. Where config files live, an example, and which value wins when several layers set a field.

## Working with config files

- **One file per instance.** The default config is `config.toml` in the aProxy home (`~/.aproxy/`,
  or the directory in `APROXY_HOME` when that is set), unless settings.json `default_config` names
  another file. Each further instance has its own file, selected with `aproxy start PATH`,
  `aproxy start --config PATH` or an alias, and needs its own port in `listen_addr`, because instances are
  identified by port. Keep extra configs in `~/.aproxy/configs/`: `aproxy find` and `aproxy doctor`
  scan it without setup (other directories: settings-json.md, `config_dirs`).
- **Create or change a file with `aproxy config`, or by hand.** `aproxy config set KEY VALUE`
  writes one key and creates the file if needed; `aproxy config --config PATH set KEY VALUE`
  targets another file, and `aproxy config unset KEY` removes one key. Any dotted key path can be
  set (the common ones: commands.md, `config`); array tables (`format`, `retry.rules`) can only be
  set or removed as a whole. A write changes only the target key and keeps comments, key order and
  everything else in the file. Writes are serialized across
  processes by a lock, and a change that would introduce a new error is refused with exit 3 while
  unrelated errors already in the file do not block it, so a broken file can be repaired one key
  at a time. `unset` removes any key, including one this version does not know.
- **Every key is checked strictly; an unknown key is an error.** A misspelled key, an old 0.1.x
  name, or a key from a later release produces `config_unknown_key`, naming the file, the key path
  and line:column, and the instance does not start. This is deliberate: an ignored matcher would
  make a retry rule wider than written, and silently dropping a key hides the typo until the
  behavior matters. Remove a key you no longer want with `aproxy config unset KEY`.
- **A wrong type or value is an error too.** After editing, run `aproxy config show --effective
  [--config PATH]`: it prints every effective value annotated with its source layer, then lists any
  diagnostics as `file:line:col: key: message [code]` and exits 3 when the file has errors. The
  message is English; `[code]` is the stable part (see [Checks at start](checks.md#checks-at-start)).
- **Apply a change with a restart.** An instance reads its config only when it starts. Run
  `aproxy restart PORT|ALIAS`: it checks the edited config first and leaves the old instance
  running if the check fails, so a typo does not take the instance down.
- **Check without applying.** `aproxy doctor` validates every config an alias points to and every
  other `*.toml` in the config directories, but not the default config unless an alias points to
  it. For the default config, `aproxy restart` or `aproxy start` is the check, and `aproxy config
  show --effective` reports its diagnostics without starting anything.

## Example

```toml
base_url = "https://api.anthropic.com"   # required
listen_addr = "127.0.0.1:12345"
# api_key = "sk-..."                      # inject the key here instead of in the client

# Optional, uncomment as needed
# proxy = "http://127.0.0.1:7890"

[timeouts]
first_byte_secs = 600                     # long non-streaming generations

[[retry.rules]]
path = '/v1/messages/count_tokens(\?.*)?' # path?query, anchored at both ends
max_attempts = 3

# [[format]]
# name = "keys"
# command = "aproxy-format"               # bare name: <home>/bin/ first, then PATH
# args = ["keys", "--config", "keys.toml"] # relative paths resolve against <home>
# process = "persistent"
# stages = ["request", "retry"]
# pool_max = 1
# worker_idle_secs = 0
```

## Precedence

Highest first:

1. **`start` override flags** `--base-url`, `--listen-addr`, `--proxy`, `--api-key` and
   `--log-file`, given to `aproxy start`, as in `aproxy start --listen-addr 127.0.0.1:12399 work`
   (syntax: commands.md, "Options for start"). They are never written to a file, but they become
   part of the instance's launch
   spec, so `aproxy restart`, `aproxy restore` and watchdog restarts keep applying them.
   To drop one, stop the instance and start it again without the flag. Prefer the file over
   `--api-key`: the flag is visible in the shell history and is stored in plain text in the
   desired-state record (owner-only), even though it no longer appears on the instance's command
   line; `--api-key -` reads the key from stdin instead.
2. **config.toml.**
3. **The `defaults` section of settings.json.** It can hold any key of this file except
   `listen_addr`, `log_file` and `format`, under the same name and nesting, written as JSON
   (settings-json.md, "defaults"). A key present in config.toml always wins, even when its value
   equals the built-in default; an explicit empty array in the toml (for example
   `allowed_hosts = []`) also counts as set where the key is an array.
4. **The built-in default.**

Scalars and arrays take the highest layer that sets them, as a whole. Arrays are replaced, not
merged: a `retry.rules` or `allowed_hosts` written in config.toml replaces the `defaults` value
whole. The two header tables are the exception: they merge by header name (case-insensitive) across
config.toml and `defaults`, with config.toml winning a conflict. `listen_addr`, `log_file` and
`format` are per-instance and have no settings.json layer.

**Credentials in `defaults` follow their target.** `defaults.api_key` and the secret headers in
`defaults.extra_headers` / `defaults.override_headers` are used only when this instance's final
`base_url` points at the same target as `defaults.base_url`; `defaults.proxy_username` and
`defaults.proxy_password` only when its final `proxy` names the same scheme, host and port as
`defaults.proxy`. Otherwise they are withheld — a key meant for one vendor's API must not be sent to
another. Everything else, including non-secret headers, is inherited as usual.
`aproxy config show --effective` lists what was withheld and warns when the instance ends up with no
credential of its own; settings-json.md, "Credentials in defaults follow their target", describes the
comparison and which header names are recognized as secrets.

`aproxy config show --effective` prints each effective value with its source layer: `config.toml`,
`settings.json 全局默认` (settings.json global default), `环境变量` (environment, `proxy` only),
`内置默认` (built-in default) or `未设置` (unset). `start` override flags are not visible to it; they
are described in step 1 above.

`proxy` has one extra source: with no `proxy` in either layer, the HTTP client reads the usual
environment variables (`HTTPS_PROXY`, `https_proxy`, `HTTP_PROXY`, `http_proxy`, `ALL_PROXY`,
`all_proxy`), and `show --effective` reports the source as `环境变量`.
