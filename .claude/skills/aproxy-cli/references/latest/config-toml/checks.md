# config.toml reference: checks at start and normalization

Part of [config-toml.md](../config-toml.md), which lists the other parts. What `start` validates and how values are normalized.

## Checks at start

`aproxy start`, and the precheck of `aproxy restart`, load the file strictly, apply `start` override
flags and settings.json defaults, and check the result. On failure the instance does not start and the error
reads `Configuration errors:` followed by one line per error:

```
<file>:<line>:<column>: <key>: <message> [<code>]
```

Parts that do not apply are omitted (`<file>: <message> [<code>]` when there is no position, and
no `<key>:` when the problem is the file itself). When the bad value came from settings.json, the
line names settings.json and the key under `defaults.` instead of the toml file. `aproxy config
show` prints the same diagnostics on stderr and exits 3.

| Code | Meaning and fix |
|---|---|
| `config_syntax` | Not valid TOML. The message includes the parser's location. |
| `config_unknown_key` | A key this version does not know, including every old 0.1.x name. Remove it with `aproxy config unset KEY` or rename it (compatibility.md lists the renames). |
| `config_type` | The value has the wrong type, for example a string where an integer belongs. |
| `config_invalid_value` | A value outside its allowed set: `delivery` not `forward`/`retry`, `keepalive.trigger = "always"`, an invalid regex, a status outside 100-599, `pool_max = 0`, `breaker_cooldown_secs = 0`, a bad `base_url`, or a `keepalive.heartbeat` that breaks SSE framing. |
| `config_missing_key` | A required key is absent: `base_url` at start, `max_attempts` in a retry rule, `command` or `stages` in a format. |
| `config_conflict` | Two settings cannot hold together: two formats with the same name, one format declaring both `response` and `stream`, `stream` or `notify` without `process = "persistent"`, or a request format after a `retry` format without declaring `retry`. |
| `config_not_supported` | A shape this version parses but does not implement: the retry-rule `body` matcher, or the format stage `admit`. |

Other start checks that are not file diagnostics: the file named by `--config` must exist
(`Configuration file not found: <path>`, code `target_not_found`, exit 4), and the listen address
must bind (exit 1). commands/start.md has the full list.

## Normalization

Applied every time the file is loaded:

- A trailing `/` is removed from `base_url`.
- `api_key`, `proxy`, `proxy_username` and `proxy_password` are trimmed; blank means unset.
- Header names and values are trimmed; entries with a blank name are dropped. Header tables from
  settings.json `defaults` and config.toml merge by name, case-insensitively; secret headers are
  withheld from an instance whose target differs (see [Precedence](editing.md#precedence)).
- A format's `name` and `command` are trimmed; a leading `~` in `command` is expanded; blank
  `command` is reported as an error (a format entry is not silently dropped).
- Arrays replace lower layers as a whole; the two header tables merge per name.
