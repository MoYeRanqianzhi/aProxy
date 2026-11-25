# Troubleshooting

Symptom, cause and fix for aProxy problems. Find the row by the message you see or by the behavior.
aProxy prints its messages in Chinese; they are quoted verbatim so you can match real output, each with
an English gloss. The semantics behind the fixes are in behaviors.md, client settings in clients.md,
fields in config-toml.md and settings-json.md. Errors produced inside a format program are covered by
the aproxy-format skill's troubleshooting.md.

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
  `请求` (requests), `在途` (in flight), `重试` (retries) and `救回` (succeeded after a retry), and
  `最近错误` ("latest error"), which reads `<code>: <English message>`, for example
  `upstream_status: the upstream returned 503`. Identify the instance the user means, and whether your
  own session runs through it, before stopping or restarting anything: a stop or restart cuts every
  in-flight request on that instance.
- Instance log: `aproxy logs <port|alias>` prints the file path (`日志文件: …`, on stderr) and the
  most recent 200 lines, then exits. Add `-f` to keep following until Ctrl+C or until the instance
  stops; in a non-interactive shell, run `aproxy logs -f …` under a timeout, or take the path and read
  the file directly.
- Startup failures: `<home>/logs/startup.log`.
- Configuration: `aproxy doctor` checks settings.json, aliased configs and port conflicts;
  `aproxy config show --effective` (add `--config <path>` for another file) prints every effective value with
  secrets masked and the layer it comes from, and exits 3 with diagnostics when the config is broken.
- Logs and `status` mask URL credentials and query values (`?beta=***`) and are safe to share.
