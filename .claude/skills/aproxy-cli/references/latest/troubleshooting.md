# Troubleshooting

Symptom, cause and fix for aProxy problems. Find the row by the message you see or by the behavior.
The semantics behind the fixes are in behaviors.md, client settings in clients.md,
fields in config-toml.md and settings-json.md. Errors produced inside a format program are covered by
the aproxy-format skill's troubleshooting.md.

The CLI prints its messages in the user's language, English or Chinese: `APROXY_LANG` decides when
set (a value starting with `zh` selects Chinese, any other English, `system` defers), then `language`
in settings.json (`en` or `zh-CN`), then the system's display language or locale, and English
otherwise. `aproxy install --force` is the exception: its own messages are always Chinese and are
quoted that way (commands.md, Force install). These references quote the English text. Logs,
`--json` output and error codes are always English, so match on codes (`port_in_use`,
`[config_type]`) and JSON fields where possible, not on message text.

`<home>` means `~/.aproxy`, or the directory named by `APROXY_HOME`.

## Parts

This reference is split by topic into the files below (paths relative to this file). Open only the part the task needs; a reference elsewhere to `troubleshooting.md, "<section>"` means the part that holds that section.

| File | Covers | Sections |
|---|---|---|
| [troubleshooting/start-stop.md](troubleshooting/start-stop.md) | An instance will not start, will not stop, or a port stays in use. | [Starting and stopping](troubleshooting/start-stop.md#starting-and-stopping) |
| [troubleshooting/requests.md](troubleshooting/requests.md) | Requests are slow, hang, fail, or the client disconnects or cannot parse the response. | [Slow, hanging or failing requests](troubleshooting/requests.md#slow-hanging-or-failing-requests) · [Client disconnects and parse errors](troubleshooting/requests.md#client-disconnects-and-parse-errors) |
| [troubleshooting/formats.md](troubleshooting/formats.md) | A format program fails or forward-only delivery behaves unexpectedly. | [Format programs and forward-only mode](troubleshooting/formats.md#format-programs-and-forward-only-mode) |
| [troubleshooting/watchdog-and-upgrades.md](troubleshooting/watchdog-and-upgrades.md) | Instances crash or are not brought back, or an install or upgrade fails. | [Crash recovery, watchdog and upgrades](troubleshooting/watchdog-and-upgrades.md#crash-recovery-watchdog-and-upgrades) |
| [troubleshooting/logs-and-config.md](troubleshooting/logs-and-config.md) | Logs or files are missing or too large, or a config or settings file is rejected. | [Logs and files](troubleshooting/logs-and-config.md#logs-and-files) · [Configuration files](troubleshooting/logs-and-config.md#configuration-files) |
| [troubleshooting/desktop.md](troubleshooting/desktop.md) | The desktop window does not open or looks wrong, or the wrong variant is installed. | [Desktop window and variants](troubleshooting/desktop.md#desktop-window-and-variants) |

## Before you change anything

- Run `aproxy status`. It lists each running instance with port, upstream, config file, the counts
  `Requests`, `in flight`, `retries` and `rescued` (succeeded after a retry), and
  `last error`, which reads `<code>: <message>`, for example
  `upstream_status: the upstream returned 503`. Identify the instance the user means, and whether your
  own session runs through it, before stopping or restarting anything: a stop or restart cuts every
  in-flight request on that instance.
- Instance log: `aproxy logs <port|alias>` prints the file path (`Log file: …`, on stderr) and the
  most recent 200 lines, then exits. Add `-f` to keep following until Ctrl+C or until the instance
  stops; in a non-interactive shell, run `aproxy logs -f …` under a timeout, or take the path and read
  the file directly.
- Startup failures: `<home>/logs/startup.log`.
- Configuration: `aproxy doctor` checks settings.json, aliased configs and port conflicts;
  `aproxy config show --effective` (add `--config <path>` for another file) prints every effective value with
  secrets masked and the layer it comes from, and exits 3 with diagnostics when the config is broken.
- Logs and `status` mask URL credentials and query values (`?beta=***`) and the keys aProxy knows,
  but logs also hold previews of upstream error bodies, which can contain anything the upstream sent
  back. Share only the lines a problem needs, after reading them, and with the user's agreement.
