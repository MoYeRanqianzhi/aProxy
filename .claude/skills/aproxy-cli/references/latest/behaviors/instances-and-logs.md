# Runtime behavior: instances, ports and logs

Part of [behaviors.md](../behaviors.md), which lists the other parts. Several instances, ports, the control channel, and what goes into which log.

## Instances, ports and the control channel

- One config file runs as one instance, and the port is the instance's identity: the instance record
  `<home>/run/<port>.instance.json`, the desired-state record `<home>/run/<port>.restore.json`, the control
  endpoint and the watchdog heartbeat are all keyed by port. Log files and the spool directory are
  the exceptions (see Logs and Disk cache). Give each config.toml its own port.
- A background instance is started from a **launch spec**, passed through a short-lived handoff file
  under `<home>/run/` (`aproxy __serve --home <dir> --spec <file>`), not from an inherited command
  line. The spec holds the config path (or null), the override flags, a pinned port for a port-0
  listener, the launching environment's proxy variables and `PATH`, and the log path. The handoff
  file is deleted once the instance is ready or has failed; the instance writes the same spec into
  its desired-state record. Both files are readable only by the current user, so `--api-key` and
  other secrets never appear on the instance's or the watchdog's command line (the short-lived
  `aproxy start` process is the one that was given the flag).
- A background instance detaches from its launcher: on Unix it calls `setsid`, so a Ctrl+C sent to
  the launching terminal's process group does not reach it; on Windows it tries to leave the
  launcher's job object (`CREATE_BREAKAWAY_FROM_JOB`) and, when the job does not allow that, stays
  in it and appends a warning to `startup.log`. It also gets no console and inherits no handles.
- Two instances on one port with different listen addresses cannot coexist; the second refuses with
  `端口 <port> 已被监听地址 <addr> 的 aProxy 实例使用（本进程将监听 <addr>），两者不能并存。` ("port already
  used by an aProxy instance listening on another address; the two cannot coexist").
- An alias resolves to a config file path and finds the running instance by that path, so it keeps
  working after the port changes. A port target goes straight to that port's control endpoint and
  works even if the registry is lost.
- Control channel: on Windows a named pipe whose name carries the port and an identifier of the
  home's run directory, on Linux and macOS the socket `<home>/run/<port>.sock`. Either way it
  belongs to one home: commands find only instances started under the same home. `status`, `stop`,
  `restart`, `logs` and `install` use it and never touch the proxy port, which carries only proxied
  traffic. An instance creates its channel before it registers, and refuses to start if it cannot:
  startup.log gets `控制通道 <endpoint> 创建失败：…` ("control channel could not be created"). So a
  registered instance is always reachable unless it hung.
- `aproxy status` lists instances that answer on their control channel and lists separately those
  whose process is alive but does not answer (hung). It deletes nothing: a record whose process is
  gone (or whose pid now belongs to another process) is ignored for this run and cleaned later by an
  instance that starts serving or by the watchdog.

### Stopping

- `aproxy stop` asks the instance to shut down. Requests still open after 10 s are cut
  (`10 秒后强制退出（在途请求将中断）`, "forcing exit in 10 s; in-flight requests will be interrupted"), so a
  long stream or a request still in retry is cut. `aproxy restart` stops the same way before starting
  again.
- That includes your own requests if you reach your model through the instance. Before stopping or
  restarting, match the instance from `aproxy status` (port, upstream, config) against what the user
  asked for and against your own base URL, and ask before touching an instance that may carry the
  user's other sessions. Never stop every instance or kill aproxy processes by name to clean up: that
  takes down every instance at once, possibly including the one carrying this conversation.
- `aproxy stop --force` terminates at once, after verifying the process by PID plus creation time
  (never by executable name), and removes the desired-state record before terminating so the
  watchdog does not race it. A process killed any other way (`taskkill /PID`, `kill`, a crash)
  leaves its desired-state record behind, so the watchdog brings it back (see Crash recovery and the
  watchdog). Only `stop` deletes records: `restart` and the install rollout keep them throughout.
- A target that is already gone but still has a record is stopped by deleting the record; `stop all`
  does the same for every record whose port has no instance process. A record this version cannot
  read is reported and left in place.

### Isolated experiments

`APROXY_HOME` replaces `~/.aproxy` as the home for the default config.toml, settings.json, `run/`,
`logs/`, `spool/`, `bin/`, `install/`, `ledger/`, `desktop/` and `skills/`. It is the only root:
there is no separate variable for the run directory. The control channel
follows the home, so commands under an isolated home cannot reach the user's instances.
Neither isolates ports: a test instance on a port a real instance listens on fails to start as
`被其他程序占用` ("in use by another program"). Run `aproxy status` without the override first and
pick a port nothing uses. An isolated home also gets its own watchdog, which exits by itself some
minutes after its last instance stops.

## Logs

- A daemon instance logs to `<home>/logs/<hex-timestamp>-<hex-pid>.log`, a new file at every start
  (restart, restore and watchdog respawns included). The name carries no port, so do not guess it:
  `aproxy logs <port|alias>` prints it as `日志文件: …` ("log file"), and `aproxy start` printed it as
  `日志: …`. A `--foreground` instance logs only to its console.
- A custom path comes from `log_file` in config.toml or `--log-file` (a `start` option; the flag wins); a relative path is
  resolved against `<home>`.
- `<home>/logs/startup.log` collects what happens before an instance has its own log or without a
  console: configuration errors, bind failures, non-loopback warnings of daemon starts, the Windows
  job-object warning of a detach that was not allowed, and watchdog slow-retry notices. Read it
  first when an instance fails to start.
- Size: at startup an existing log file over 2 MiB is emptied (this matters for a fixed `log_file`); a
  running daemon empties its log once an hourly check finds it over `settings.json logs.rotate_mb` (0
  disables). `aproxy logs -f` keeps following across truncation.
- Cleanup happens in the background, never in a read-only command. An instance runs one cleanup
  right after it starts serving, and the watchdog does one on every scan. It removes an instance
  record `<port>.instance.json` only when the record is readable and the process that wrote it is
  verifiably gone (pid plus start marker); it removes leftover launch-spec and atomic-write temp files only
  when their writer is no longer running (or cannot be found) and they are older than 10 minutes;
  and it removes an instance log only
  when the name has the built-in random shape (`<hex>-<4 hex>.log`), the file is older than
  10 minutes, and no instance record or desired-state record refers to it. If `run/` holds a record
  this version cannot read, no log is deleted in that round. Custom `log_file` paths and fixed names
  such as `startup.log` are never deleted. `aproxy status`, `stop`, `restart`, `logs` and target
  selection delete no files.
- Encoding: UTF-8 without BOM. aProxy switches the Windows console to code page 65001 itself; garbled
  text usually comes from the tool reading the file (troubleshooting.md, Logs and files).
- Secrets: logs do not contain `api_key` or header values. Every URL (base_url, proxy, upstream target,
  request path) is masked: userinfo becomes `***@`, query values become `***` with keys kept
  (`?beta=true` shows as `?beta=***`), and a fragment becomes `***`. `aproxy status` follows the same
  rules, so both can be shared as they are. A retry rule's `path` still matches the real query string.
- Each failed attempt that is retried logs `本次尝试失败，重试` ("this attempt failed; retrying") with
  `attempt`, `code`, `details` and `reason`. Decision lines such as `上游返回可重试状态码，重试` carry a
  `code` field too.
- Each failed attempt logs `错误响应预览` ("error response preview") with `content_type`,
  `content_encoding` and the body, decoded first when compressed. A preview of the form
  `（二进制/压缩内容，共 N 字节，hex 前 48: …）` ("binary/compressed content, N bytes, first 48 bytes in hex")
  means the body is binary or could not be decoded (unknown encoding, corrupt data, or over 8 MiB once
  decoded).
- An instance listening on a non-loopback address warns `监听地址 … 不是回环地址…` ("listen address is not
  a loopback address") in the start output, the daemon log, startup.log and `aproxy doctor`, more
  strongly when it injects credentials: any host that reaches the port can then spend the user's key.
