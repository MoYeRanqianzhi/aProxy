# Runtime behavior

What a running aProxy instance does with each request and how instances look after themselves:
retries, keepalive heartbeats, buffering, forward-only mode, format programs, instance identity, logs,
crash recovery, the watchdog and upgrades. Read it to explain or predict aProxy's behavior, or before
acting on a running instance. Field syntax and defaults are in config-toml.md and settings-json.md,
client setup in clients.md, symptom lookup in troubleshooting.md.

`<home>` below means `~/.aproxy`, or the directory named by `APROXY_HOME` when that is set.

## Parts

This reference is split by topic into the files below (paths relative to this file). Open only the part the task needs; a reference elsewhere to `behaviors.md, "<section>"` means the part that holds that section.

| File | Covers | Sections |
|---|---|---|
| [behaviors/requests.md](behaviors/requests.md) | How a request travels through an instance and when it is retried: lifecycle, retry decisions, errors inside response bodies, the disk cache, request history, forward-only delivery. | [Request lifecycle](behaviors/requests.md#request-lifecycle) · [Retry decisions](behaviors/requests.md#retry-decisions) · [Backoff](behaviors/requests.md#backoff) · [Retry rules](behaviors/requests.md#retry-rules) · [Error detection in response bodies](behaviors/requests.md#error-detection-in-response-bodies) · [Disk cache (spool)](behaviors/requests.md#disk-cache-spool) · [Request history](behaviors/requests.md#request-history) · [Forward-only mode](behaviors/requests.md#forward-only-mode) |
| [behaviors/keepalive.md](behaviors/keepalive.md) | What the client receives while the upstream is slow or being retried. | [Keepalive heartbeats](behaviors/keepalive.md#keepalive-heartbeats) · [Which requests get heartbeats](behaviors/keepalive.md#which-requests-get-heartbeats) · [Commit point](behaviors/keepalive.md#commit-point) · [After the commit](behaviors/keepalive.md#after-the-commit) · [Compression](behaviors/keepalive.md#compression) · [Disconnects](behaviors/keepalive.md#disconnects) |
| [behaviors/formats.md](behaviors/formats.md) | How an instance runs the `[[format]]` programs: stages, workers, failures. | [Format programs](behaviors/formats.md#format-programs) |
| [behaviors/instances-and-logs.md](behaviors/instances-and-logs.md) | Several instances, ports, the control channel, and what goes into which log. | [Instances, ports and the control channel](behaviors/instances-and-logs.md#instances-ports-and-the-control-channel) · [Stopping](behaviors/instances-and-logs.md#stopping) · [Isolated experiments](behaviors/instances-and-logs.md#isolated-experiments) · [Logs](behaviors/instances-and-logs.md#logs) |
| [behaviors/watchdog-and-upgrades.md](behaviors/watchdog-and-upgrades.md) | Crash and hang recovery, the watchdog, what `aproxy install` does to running instances, and the download proxy. | [Crash recovery and the watchdog](behaviors/watchdog-and-upgrades.md#crash-recovery-and-the-watchdog) · [Desired-state records](behaviors/watchdog-and-upgrades.md#desired-state-records) · [Watchdog](behaviors/watchdog-and-upgrades.md#watchdog) · [Upgrades with aproxy install](behaviors/watchdog-and-upgrades.md#upgrades-with-aproxy-install) · [Skill documentation](behaviors/watchdog-and-upgrades.md#skill-documentation) · [Download proxy versus request proxy](behaviors/watchdog-and-upgrades.md#download-proxy-versus-request-proxy) |
