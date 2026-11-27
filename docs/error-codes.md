# Error codes

<!-- Generated from aproxy_ipc::code::TABLE by tests/code_table.rs. Do not edit by hand: change the table, then run `APROXY_BLESS=1 cargo test --test code_table`. -->

Every code aproxy reports, with what it carries. Codes are snake_case, unique across all families, and only ever added: a published code keeps its name, its family, its HTTP status or exit code, and the names and types of its `details` keys. A reader that meets a code it does not know should treat it as a generic failure of its family.

Human-readable messages are not part of the contract; match on codes. A `?` after a `details` key means the key is not always present. The same table is available to programs as JSON from `aproxy_ipc::code::table_json()`.

## Terminal codes (data plane)

How a proxied request finally ended when aproxy itself ended it. The code is in the JSON error body as `error.aproxy.code` (and `error.type` is `aproxy_` plus the code), in the `x-aproxy-error` response header, and in the terminal SSE `event: error` when the response headers were already sent. "passthrough" means: when there is an upstream response (or one given by a format), it is returned as is with its own status.

| Code | HTTP status | When | `details` |
|---|---|---|---|
| `inbound_host_rejected` | 403 | The Host header is not in the allowed set. | `host` (string) |
| `inbound_origin_rejected` | 403 | The request carries an Origin that is not allowed. | `origin` (string) |
| `request_too_large` | 413 | The request body exceeds max_request_mb. | `limit_bytes` (bytes) |
| `client_body_aborted` | 400 | Reading the client's request body failed midway. | — |
| `upgrade_unsupported` | 501 | Upgrade (WebSocket) requests are not supported. | `upgrade` (string) |
| `local_storage_failed` | 507 | Writing the request or response body to disk failed. | `op` (string) |
| `response_too_large` | 502 | The upstream response exceeds max_response_mb. | `limit_bytes` (bytes) |
| `retry_exhausted` | passthrough, else 502 | The attempts allowed by a retry rule or a per-request override are used up. | `by` (string), `last_code` (code) |
| `retry_stopped` | passthrough, else 502 | A format's decide stage stopped the retries. | `format` (string), `last_code` (code) |
| `format_rejected` | passthrough, else 422 | A request-side format answered fail with retry:false. | `format` (string), `stage` (string), `fail_code`? (string) |
| `format_unavailable` | 503 | The circuit breaker is temporarily blocking calls to a required request-side format program, so the request cannot proceed. | `format` (string), `stage` (string), `kind` (string), `probe_at` (Unix milliseconds) |
| `format_failed` | 502 | In forward mode, a request-side format program failed or asked for a retry, and the request was not switched to retry mode. | `format` (string), `stage` (string), `kind`? (string), `fail_code`? (string) |
| `upstream_failed` | 502 | In forward mode, the upstream attempt failed, and the request was not switched to retry mode. | `last_code` (code) |
| `decode_failed` | SSE event only | Response headers had already been sent to the client without a content encoding, and the body could not be decoded to match. | `encoding` (string) |
| `shutting_down` | 503 | The instance is shutting down. | — |
| `internal` | 500 | A defect in aProxy itself. | — |

## Attempt failure codes

Why one upstream attempt did not succeed. By default every one of them is retried without limit; retry rules and formats can change that. They appear as `last_code` in terminal codes and in request records.

| Code | When | `details` |
|---|---|---|
| `upstream_connect_failed` | Could not connect to the upstream server, for example because the DNS lookup failed, the connection was refused or the TLS handshake failed. | `reason` (string) |
| `upstream_connect_timeout` | Connecting to the upstream timed out. | `timeout_ms` (milliseconds) |
| `upstream_first_byte_timeout` | The request was sent but no response headers arrived in time. | `timeout_ms` (milliseconds) |
| `upstream_read_idle_timeout` | No data arrived on the response body for too long. | `timeout_ms` (milliseconds), `received_bytes` (bytes) |
| `upstream_body_interrupted` | The response body broke off midway. | `received_bytes` (bytes), `reason` (string) |
| `upstream_status` | The upstream answered 4xx or 5xx. | `status` (integer) |
| `upstream_error_body` | The upstream answered 2xx but the body is an error. | `status` (integer) |
| `format_error` | A format failed at the process level or produced invalid output. | `format` (string), `stage` (string), `kind` (string) |
| `format_fail` | A format answered fail with retry:true. | `format` (string), `stage` (string), `fail_code`? (string) |
| `format_decision` | A format's decide stage judged a successful attempt as failed. | `format` (string) |

## IPC protocol codes

The `code` of the error object an instance returns on its control endpoint.

| Code | When | `details` |
|---|---|---|
| `internal` | A defect in aProxy itself. | — |
| `bad_request` | Not JSON, missing v or op, line too long, or an argument of the wrong type or value. | `reason` (string), `arg`? (string) |
| `unsupported_version` | The protocol version v is not supported. | `supported` (integer list) |
| `format_control_error` | The dedicated format control channel did not deliver a valid reply. | `reason` (string) |
| `unknown_format` | No configured format has this name. Nothing was restarted. | `name` (string) |
| `unknown_op` | There is no such op. Nothing was done. | `op` (string) |
| `unsupported_arg` | The request carries arguments this instance does not know. | `args` (string list) |
| `invalid_state` | Not allowed in the instance's current state. | `state` (string), `reason`? (string) |
| `condition_not_met` | The action was not performed because its condition did not hold, for example the instance had not been idle long enough to stop. | `idle_ms`? (milliseconds) |
| `busy` | All streaming connections are in use. | `limit` (integer) |
| `not_found` | The request, or the body asked for, cannot be retrieved. | `request_id` (string), `reason` (string), `cause`? (string) |

## Operation codes

Why an operation started from the command line or the desktop window failed. With `--json` the code is in the output; the process exits with the listed exit code. General exit codes: 0 success or the target state already holds, 1 the operation failed, 2 usage error, 3 invalid configuration, 4 the target is not running or does not exist, 5 partial success. Exit codes from 10 on belong to `install --force`.

| Code | Exit code | When | `details` |
|---|---|---|---|
| `internal` | 1 | A defect in aProxy itself. | — |
| `usage` | 2 | A usage error found outside the argument parser. | `reason` (string) |
| `home_invalid` | 2 | aProxy could not determine its home directory. Set APROXY_HOME to an absolute path. | `value` (string) |
| `target_ambiguous` | 2 | Several instances are running and no target was given. | `ports` (integer list) |
| `target_not_found` | 4 | The alias, port or config file does not exist. | `target` (string) |
| `target_not_running` | 4 | The target is not running. | `target` (string) |
| `config_invalid` | 3 | The configuration has error-level diagnostics. | `diagnostics` (diagnostic list) |
| `command_failed` | 1 | The command could not complete. | — |
| `write_conflict` | 1 | The config file was modified externally while writing, and retrying still conflicted. | `file` (path) |
| `port_in_use` | 1 | The port is already in use. | `port` (integer), `owner` (string) |
| `start_failed` | the child's code if 1–255, else 1 | The instance could not start. | `code`? (integer), `startup_log` (string list) |
| `ready_timeout` | 1 | aProxy could not confirm within the time limit that the instance was ready to accept requests. | `timeout_ms` (milliseconds) |
| `format_restart_failed` | 1 | The format restart operation was not completed. | `port` (integer), `reason` (string) |
| `restart_unsupported` | 1 | There is no usable desired-state record for this instance (for example, it runs in the foreground), so it cannot be restarted automatically. | `port` (integer) |
| `instance_unresponsive` | 1 | The instance's process is there but it does not answer. | `port` (integer), `pid` (integer) |
| `instance_incompatible` | 1 | The instance answered, but its protocol is incompatible or its identity does not match. | `port` (integer), `reason` (string) |
| `stop_failed` | 1 | The instance could not be stopped. | `port` (integer), `reason` (string) |
| `no_log_file` | 1 | A foreground instance has no log file. | `port` (integer) |
| `desktop_unavailable` | 1 | The desktop command was run on a headless build. | `variant` (string) |
| `skill_not_installed` | 4 | This skill is not installed locally. | `name` (string) |
| `skill_file_not_found` | 4 | The file is not in the skill's manifest. | `name` (string), `file` (path) |

## Configuration diagnostic codes

Problems found in config.toml and settings.json. Each diagnostic is an object with `code`, `severity`, `file`, optional `key`, `line`, `column`, a `message` and `details`. Only `error` diagnostics affect the exit code.

| Code | Severity | When | `details` |
|---|---|---|---|
| `config_syntax` | error | TOML or JSON syntax error. | — |
| `config_type` | error | A value has the wrong type. | — |
| `config_invalid_value` | error | A value is not allowed. | `reason` (string) |
| `config_missing_key` | error | A required key is missing. | — |
| `config_legacy_shape` | info | A file uses the 0.1.x configuration shape; preview its migration before writing. | — |
| `config_unknown_key` | error | The key is not recognized. | — |
| `config_not_supported` | error | This version recognizes the setting but does not support it yet. | — |
| `format_name_reserved` | error | The format name is reserved for selecting all entries. | `name` (string) |
| `config_conflict` | error | The configuration contains entries that conflict with each other. | — |
| `secret_placeholder_unmatched` | error | A secret placeholder written back does not match any original value. | — |
| `default_credential_withheld` | warning | A credential in the settings defaults was not inherited because the target differs, and this config has none of its own. | `key` (string), `reason` (string) |
| `default_header_shared` | info | These default headers apply to every configuration that does not set them itself. If one carries a credential, set it only in the configurations that need it. | — |
| `forward_default_unguarded` | warning | The default delivery is forward while the chain has a decide program or an optional request program: when they are unavailable, requests are not retried. | — |
| `forward_response_buffered` | info | The default delivery is forward and the chain has a response program; that layer gathers each response, so those responses are not streamed. Declare stream instead where the program supports it. | `format` (string) |
| `settings_schema_too_new` | error | settings.json has a schema newer than this version understands. | — |
| `format_handshake_failed` | varies | A format's handshake failed at startup. | `format` (string), `kind` (string) |
| `format_stage_unsupported` | error | A format does not support a stage the config declares for it. | `format` (string), `stages` (string list) |
| `file_permissions_too_open` | warning | Other users can access the aProxy home or a configuration file. | — |
| `listen_not_loopback` | warning | The config listens on a non-loopback address, so other hosts can reach the proxy. | `listen_addr` (string) |
| `port_shared` | warning | Several config files listen on the same port, so they cannot run at the same time. | `port` (integer), `files` (string list) |
| `config_dir_missing` | warning | A directory listed in settings.json `config_dirs` does not exist. | `dir` (path) |
| `config_file_unreadable` | warning | A config file named by an alias or by `default_config` does not exist or cannot be read. | `reason` (string) |
| `watchdog_slow_hang_detection` | warning | settings.json sets `watchdog.heartbeat_secs` so high that a hung instance is detected only after minutes. | `heartbeat_secs` (integer) |
| `watchdog_restarts_disabled` | warning | settings.json sets `watchdog.max_restarts` to 0, so the watchdog never restarts a crashed or hung instance. | — |
| `install_restore_unreadable` | error | The desired-state record needed for the rolling restart cannot be read. | — |
| `install_config_unreadable` | error | The instance configuration needed for the rolling restart is missing or unreadable. | — |

## Record codes

Why a persistent record under the home could not be used. Such records are reported, never deleted.

| Code | When | `details` |
|---|---|---|
| `record_unreadable` | A saved record cannot be read. | — |
| `record_schema_too_new` | A saved record uses a newer format than this version understands. | `schema` (integer) |

## Desktop faults

Faults returned to the bundled desktop page. They do not change CLI exit codes.

| Code | When | `details` |
|---|---|---|
| `desktop_format` | The format operation on the selected instance could not be completed. | `reason` (string) |
| `desktop_format_fail` | The format program reported that the control operation failed. | `code` (string), `reason` (string) |
| `desktop_package` | Package validation or a package operation failed. | `reason` (string), `field`? (string), `path`? (path), `limit`? (string), `max`? (integer) |
| `desktop_toml` | The desktop preferences file could not be edited. | `kind` (string), `line`? (integer), `column`? (integer), `key`? (string) |

## Install codes

Why an install or upgrade failed. `install --force` does not use these; it only promises its own exit codes.

| Code | Exit code | When | `details` |
|---|---|---|---|
| `install_in_progress` | 1 | Another install is in progress in this home. | — |
| `install_version_not_found` | 4 | The requested version does not exist. | — |
| `install_below_floor` | 1 | The requested version is too old to use this aProxy home's file layout and control protocol. | — |
| `install_download_failed` | 1 | Downloading the binary failed. | — |
| `install_checksum_mismatch` | 1 | The downloaded binary does not match its checksum. | — |
| `install_probe_failed` | 1 | The new binary did not run, or reported a version other than the one being installed. | — |
| `install_swap_failed` | 1 | Replacing the binary in bin/ failed. | — |
| `install_rolled_back` | 1 | The install failed and the previous binary was put back. | — |
| `install_restart_failed` | 1 | An instance did not start on the new binary, so the rolling restart stopped. | `port` (integer) |
| `install_failed` | 1 | The install failed for a reason that no more specific code covers. | — |
| `install_unmanaged_instances` | 1 | Live instances started by an older version are in this home and cannot be restarted by this install. | — |
| `install_foreground_instances` | 1 | Live foreground instances are in this home; an install cannot restart a foreground instance. | `ports` (integer list) |
| `install_precheck_failed` | 3 | The precheck before the rolling restart did not pass. | `port` (integer), `diagnostics` (diagnostic list) |
