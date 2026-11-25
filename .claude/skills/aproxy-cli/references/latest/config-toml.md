# config.toml reference

Every field of an instance's `config.toml`: type, default, what 0 or an empty value means, and how
to choose a value; then how values combine with `start` override flags and `settings.json`, what is checked at
start, and how values are normalized. Read it before writing or editing a config, or when a start
fails with `Configuration errors:`. How each feature works at runtime is in
behaviors.md; this file keeps only what you need to pick a value.

## Parts

This reference is split by topic into the files below (paths relative to this file). Open only the part the task needs; a reference elsewhere to `config-toml.md, "<section>"` means the part that holds that section.

| File | Covers | Sections |
|---|---|---|
| [config-toml/editing.md](config-toml/editing.md) | Where config files live, an example, and which value wins when several layers set a field. | [Working with config files](config-toml/editing.md#working-with-config-files) · [Example](config-toml/editing.md#example) · [Precedence](config-toml/editing.md#precedence) |
| [config-toml/upstream-and-limits.md](config-toml/upstream-and-limits.md) | `base_url`, `listen_addr`, `api_key`, headers, the outgoing proxy, size limits and timeouts. | [base_url](config-toml/upstream-and-limits.md#base_url) · [listen_addr](config-toml/upstream-and-limits.md#listen_addr) · [api_key](config-toml/upstream-and-limits.md#api_key) · [extra_headers / override_headers](config-toml/upstream-and-limits.md#extra_headers--override_headers) · [proxy / proxy_username / proxy_password](config-toml/upstream-and-limits.md#proxy--proxy_username--proxy_password) · [max_request_mb](config-toml/upstream-and-limits.md#max_request_mb) · [max_response_mb](config-toml/upstream-and-limits.md#max_response_mb) · [timeouts](config-toml/upstream-and-limits.md#timeouts) |
| [config-toml/retry-and-keepalive.md](config-toml/retry-and-keepalive.md) | `keepalive`, `retry` rules and `delivery`. | [keepalive](config-toml/retry-and-keepalive.md#keepalive) · [retry](config-toml/retry-and-keepalive.md#retry) · [delivery](config-toml/retry-and-keepalive.md#delivery) |
| [config-toml/storage-and-access.md](config-toml/storage-and-access.md) | `disk_cache`, `allowed_hosts` / `allowed_origins`, `log_file`, `history`. | [disk_cache](config-toml/storage-and-access.md#disk_cache) · [allowed_hosts / allowed_origins](config-toml/storage-and-access.md#allowed_hosts--allowed_origins) · [log_file](config-toml/storage-and-access.md#log_file) · [history](config-toml/storage-and-access.md#history) |
| [config-toml/format.md](config-toml/format.md) | Configuring `[[format]]` entries on the aProxy side. | [format](config-toml/format.md#format) |
| [config-toml/checks.md](config-toml/checks.md) | What `start` validates and how values are normalized. | [Checks at start](config-toml/checks.md#checks-at-start) · [Normalization](config-toml/checks.md#normalization) |

## Field summary

"settings.json" in the Default column means the field falls back to a key of the same name and
nesting in the `defaults` section of settings.json, whose built-in value is shown (see
[Precedence](config-toml/editing.md#precedence)). Integers are non-negative.

| Field | Type | Default | 0 / empty means |
|---|---|---|---|
| `base_url` | string | none, required (may come from settings.json `defaults.base_url`) | start fails |
| `listen_addr` | string `host:port` | `"127.0.0.1:12345"` | port 0: the OS picks one |
| `api_key` | string | settings.json (`defaults.api_key`), unset | blank = unset |
| `extra_headers` | table of strings | `{}` | |
| `override_headers` | table of strings | `{}` | |
| `proxy` | string (URL) | settings.json (`defaults.proxy`), unset: environment proxy variables apply | blank = unset |
| `proxy_username` | string | settings.json (`defaults.proxy_username`), unset | blank = unset |
| `proxy_password` | string | settings.json (`defaults.proxy_password`), unset | blank = unset |
| `max_request_mb` | integer | settings.json (`defaults.max_request_mb`), `128` | 0 = no limit |
| `max_response_mb` | integer | settings.json (`defaults.max_response_mb`), `256` | 0 is treated as 1 |
| `[timeouts] connect_secs` | integer | settings.json (`defaults.timeouts.connect_secs`), `30` | 0 = no limit |
| `[timeouts] first_byte_secs` | integer | settings.json (`defaults.timeouts.first_byte_secs`), `300` | 0 = no limit |
| `[timeouts] read_idle_secs` | integer | settings.json (`defaults.timeouts.read_idle_secs`), `300` | 0 = no limit |
| `[keepalive] interval_secs` | integer | settings.json (`defaults.keepalive.interval_secs`), `15` | 0 = keepalive off |
| `[keepalive] trigger` | `"accept"` \| `"body_stream"` \| `"any"` | settings.json (`defaults.keepalive.trigger`), `"any"` | `"always"` is rejected |
| `[keepalive] heartbeat` | string | settings.json (`defaults.keepalive.heartbeat`), `": keepalive\n\n"` | must not be empty; must end at an SSE event boundary |
| `[retry] max_backoff_secs` | integer | settings.json (`defaults.retry.max_backoff_secs`), `320` | 0 = every retry immediate |
| `[[retry.rules]]` | array of tables | settings.json (`defaults.retry.rules`), `[]` | empty = every request retries without limit |
| `delivery` | `"retry"` \| `"forward"` | settings.json (`defaults.delivery`), `"retry"` | |
| `disk_cache` | boolean | settings.json (`defaults.disk_cache`), `true` | |
| `[history] body_budget_mb` | integer | settings.json (`defaults.history.body_budget_mb`), `64` | 0 = keep no bodies |
| `allowed_hosts` | array of strings | settings.json (`defaults.allowed_hosts`), `[]` | empty = built-in policy; `["*"]` = check off |
| `allowed_origins` | array of strings | settings.json (`defaults.allowed_origins`), `[]` | empty = built-in policy; `["*"]` = check off |
| `log_file` | string (path) | unset: random name in `<APROXY_HOME>/logs/` | |
| `[[format]]` | array of tables | `[]` | no format program |
