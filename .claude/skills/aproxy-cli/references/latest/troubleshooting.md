# Troubleshooting

Symptom, cause and fix for aProxy problems. Find the row by the message you see or by the behavior.
aProxy prints its messages in Chinese; they are quoted verbatim so you can match real output, each with
an English gloss. The semantics behind the fixes are in behaviors.md, client settings in clients.md,
fields in config-toml.md and settings-json.md. Errors produced inside a format program are covered by
the aproxy-format skill's troubleshooting.md.

`<home>` means `~/.aproxy`, or the directory named by `APROXY_HOME`.

## Contents

- [Before you change anything](#before-you-change-anything)
- [Starting and stopping](#starting-and-stopping)
- [Slow, hanging or failing requests](#slow-hanging-or-failing-requests)
- [Client disconnects and parse errors](#client-disconnects-and-parse-errors)
- [Transformers and forward-only mode](#transformers-and-forward-only-mode)
- [Logs and files](#logs-and-files)
- [Crash recovery, watchdog and upgrades](#crash-recovery-watchdog-and-upgrades)
- [Configuration files](#configuration-files)

## Before you change anything

- Run `aproxy status`. It lists each running instance with port, upstream, config file, request and
  retry counts, and `最近错误` ("latest error"). Identify the instance the user means, and whether your
  own session runs through it, before stopping or restarting anything: a stop or restart cuts every
  in-flight request on that instance.
- Instance log: `aproxy logs <port|alias>` prints the file path (`日志文件: …`) and recent lines, then
  keeps following until Ctrl+C or until the instance stops. In a non-interactive shell, run it with a
  timeout, or take the path and read the file directly.
- Startup failures: `<home>/logs/startup.log`.
- Configuration: `aproxy doctor` checks settings.json, aliased configs and port conflicts;
  `aproxy config --show` (add `--config <path>` for another file) prints a config with secrets masked
  and says which values come from settings.json defaults.
- Logs and `status` mask URL credentials and query values (`?beta=***`) and are safe to share.

## Starting and stopping

| Symptom | Cause | Fix |
|---|---|---|
| `端口 <addr> 被其他程序占用，无法启动` ("port in use by another program") | Another program listens on the port, or an aProxy instance started under a different `APROXY_HOME` (an instance of this home would be reported as already running) | Find the owner (`netstat -ano \| findstr :<port>` on Windows, `ss -ltnp \| grep :<port>` on Linux), then pick another port or ask the user about that program |
| `端口 <addr> 无法绑定：无权限或端口被系统保留（如 Hyper-V/WinNAT 排除区间，…）` ("cannot bind: no permission, or the port is reserved by the system") | Windows reserves port ranges for Hyper-V/WinNAT; netstat shows no listener | `netsh interface ipv4 show excludedportrange protocol=tcp`, then choose a port outside every range |
| startup.log has `控制通道 <endpoint> 创建失败：…` ("control channel could not be created") and the instance did not start | Another aProxy instance in the same home already uses that port on a different listen address (both can bind TCP, only one can own the control channel), or on Linux/macOS the run directory path is too long for a socket | Give one of the two configs another port; for a long path, use a shorter `APROXY_HOME` or `APROXY_RUN_DIR` |
| `后台进程未在预期时间内就绪（pid N），启动失败的原因通常记录在:` ("background process not ready in time; the cause is usually logged in:") | The daemon exited during startup (config error, bind failure) or started slowly (antivirus scan on a cold start) | Read the end of `<home>/logs/startup.log`. If it shows nothing new, run `aproxy status`: the instance may have come up late |
| `配置文件解析失败（TOML 语法错误）` ("config parse failed: TOML syntax") or `配置错误: …` ("config error") | The message names the file and field, and says so when the bad value came from settings.json | Fix that field (config-toml.md); `aproxy doctor` checks every config |
| `listen_addr 缺少端口或端口无效` ("listen_addr lacks a port or the port is invalid") | `listen_addr` without `:port` | Write it as `127.0.0.1:12345` |
| `此端口已有 aProxy 在运行，无需重复启动：` ("an aProxy instance already runs on this port") | That port's instance is running | Nothing to start. To apply config changes, `aproxy restart <port>` |
| `端口 <port> 已被监听地址 <addr> 的 aProxy 实例使用（…），两者不能并存。` ("port used by an aProxy instance on another address; they cannot coexist") | Instances are identified by port | Give one of the two configs another port |
| `警告: 监听地址 … 不是回环地址…` ("warning: listen address is not loopback") | `listen_addr` is `0.0.0.0`, a LAN address or a hostname; any host that reaches the port can use the upstream, and the configured key if any | Use `127.0.0.1:<port>` unless the user wants LAN access; then only on a trusted network |
| `有 N 个实例在运行，必须指定端口号、别名或 all：` ("N instances running; give a port, alias or all") | `stop` or `restart` without a target while several instances run | Pick the instance the user means from `aproxy status`. Use `all` only when the user asked for every instance |
| `pid N 已收到停止请求但尚未退出，…` ("pid N got the stop request but has not exited") | Still shutting down (open requests get 10 s), or hung | Run `aproxy status` after a few seconds; if it is still listed, `aproxy stop <port> --force`. Prefer that to killing the pid yourself: it verifies the process identity and removes the restore record, so the watchdog does not bring the instance back |
| `pid N 不应答停止请求（…），可用 aproxy stop <port> --force 强制结束` ("pid N does not answer the stop request") | The process is alive but its control channel is broken: it hung | `aproxy stop <port> --force` |
| `status` lists `无响应的 aProxy 实例 …——进程仍在，但不应答控制通道` ("unresponsive instances"), or `stop` says `…进程仍在，但不应答控制通道，无法优雅停止；用 --force 强制结束` | The instance hung: its process is alive but answers neither requests on the control channel nor a graceful stop | `aproxy stop <port> --force` (or `aproxy restart <port> --force` to bring it back). With the watchdog on, it also terminates and respawns a hung instance by itself |
| The proxy answers on a port but `aproxy status` does not list it | The instance runs under another `APROXY_HOME`: its registry and control channel belong to that home | Run `aproxy status` (and `stop`, `logs`) with the same `APROXY_HOME` the instance was started with |
| Port still busy after `aproxy stop` | Shutdown not finished yet (up to about 12 s); or the process had been killed outside aproxy and the watchdog respawned it; or another program owns the port | `aproxy status`: an instance listed on that port is running (respawned), so stop it with `aproxy stop <port>`. Otherwise find the owner with netstat or ss |
| `端口 N 未重启：预检未通过，旧实例保持运行。` ("not restarted: pre-check failed; old instance keeps running") | The edited config is invalid, or its new port is taken | Fix the reason printed below it; the old instance is still serving |
| `端口 N 停止未确认，重启中止（…）。` ("stop not confirmed; restart aborted") | The old instance did not exit | `aproxy status`, then `aproxy restart <port> --force` if it is hung |
| `端口 N 重启失败（旧实例已停止，该端口服务已中断）：…` ("restart failed: old instance stopped, service on this port is down") | The new instance failed to come up; the message includes the new startup.log lines | Fix the cause, then run the printed `aproxy start --config "<path>"`. It does not carry the old CLI overrides; add them back if there were any |

## Slow, hanging or failing requests

| Symptom | Cause | Fix |
|---|---|---|
| The client waits a long time before answering | Normal during upstream trouble: aProxy is retrying and heartbeats hold the connection | Nothing, or see why: `aproxy status` (`重试`, `最近错误`) or the log (`上游返回可重试状态码，重试`, `上游网络错误，重试`) |
| Every request hangs; the log repeats the same 4xx followed by `错误响应预览` ("error response preview") | A deterministic rejection (bad key, unknown model, wrong path) is retried forever by design | Read the upstream's message in the preview, fix `api_key`, `base_url` or the client's model, then `aproxy restart <port\|alias>`. For an endpoint this upstream never supports, use a bounded retry path (next rows) |
| The log's `target=` shows a doubled prefix (`/v1/v1/…`) and the upstream answers 404 | The path prefix is set both in `base_url` and in the client | Keep it on one side (clients.md, Setup checklist) |
| Claude Code `/compact` (or another command) never finishes behind a relay or mirror API | The upstream does not implement `POST /v1/messages/count_tokens`; its deterministic 404 is retried forever | In that instance's config.toml set `bounded_retry_paths = ['/v1/messages/count_tokens(\?.*)?']` and run `aproxy restart <port\|alias>`; the failure then reaches the client after three attempts. The pattern must cover the query string Claude Code sends (`?beta=true`): `'/v1/messages/count_tokens'` alone does not match it. For other endpoints copy the path from the log's `path=`; query values show as `***` there, but the pattern is matched against the real values |
| Log `受限重试路径达到尝试上限` ("bounded retry path reached its attempt limit") | Expected for a path listed in `bounded_retry_paths` | If that endpoint should retry without limit, remove its pattern and restart |
| A non-streaming request times out at the client | Non-streaming requests get no heartbeats, so the client sees nothing until the end | Raise the client's overall timeout (Claude Code: `API_TIMEOUT_MS`). A lower `max_retry_backoff_secs` also helps: requests finish sooner once the upstream recovers |
| A streaming request gets no heartbeat, or its first byte comes late | `keepalive_trigger = "accept"` excludes clients that send `Accept: application/json` with `"stream": true` (Claude Code); or `keepalive_interval_secs = 0`; or forward-only mode | Use `keepalive_trigger = "any"`. Check the effective value with `aproxy config --show` and the settings.json default, then restart |
| The client got `text/event-stream` with a JSON or NDJSON body; the log warns `已以 text/event-stream 提交的保活响应里回放的是非 SSE 的成功响应体…` | The upstream answered a stream request with a non-SSE body after the skeleton head was committed | Set `keepalive_trigger = "accept"` on that instance |
| The client receives `event: error` with `error.type` `upstream_error` or `proxy_…` | aProxy ended an already committed response | See behaviors.md, After the commit, for each type; the fixes are the 502 rows below |
| 502 `上游响应体超出 spool 上限，无法回放（重试无意义）` ("upstream response exceeds the spool limit") | Response larger than `spool_limit_mb` | Raise `spool_limit_mb` |
| 502 `本地磁盘缓存写入失败，无法回放: …` ("local disk cache write failed"), or `proxy_spool_failed` | Disk full, or `<home>/spool/<port>/` not writable | Free space, or set `disk_cache = false` and restart |
| 413 `请求体超出上限（N MiB），可在 settings.json 的 max_body_mb 或 config.toml 的 max_body_mb 调整` ("request body over the limit") | Body larger than `max_body_mb` | Raise it, or `0` for no limit |
| 400 `请求体读取失败（连接中断或本地磁盘缓存写入失败）: …` ("reading the request body failed") | The client disconnected while uploading, or spilling the body to disk failed | Usually client-side; otherwise check disk space |
| 403 starting `aProxy 拒绝了该请求（403，未转发上游）：` naming a Host or an Origin; log `入站请求被拒绝…` ("inbound request rejected") | Inbound check: browser and Electron clients send `Origin`, rejected by default; a non-default host name is rejected on loopback listeners | Add the exact value from the message to `allowed_origins` or `allowed_hosts` (config-toml.md), only if the user trusts that client; restart |
| 502 `aProxy 内部错误：保活通道异常结束` ("internal error: keepalive channel ended abnormally") | An internal failure in aProxy | Resend; if it repeats, collect the log around it for a bug report |
| Upstream traffic goes through an unexpected proxy, or bypasses the expected one | Without `proxy` in config.toml, the instance uses `HTTPS_PROXY`/`HTTP_PROXY`/`ALL_PROXY`/`NO_PROXY` from the environment it was started (or respawned) in | Set `proxy` explicitly, which disables those variables, and restart |
| `aproxy status` shows the request count not moving while the client works | The client is not using aProxy | clients.md, Verify the setup |

## Client disconnects and parse errors

| Symptom | Cause | Fix |
|---|---|---|
| Log `客户端在只收到心跳的等待中断开（已等约 N 秒）…` ("client disconnected while receiving only heartbeats, after about N s") | The client's event-level stream idle timeout, about N s; comment heartbeats do not reset it | Raise or disable it (clients.md). Unless the user cancelled on purpose |
| Claude Code drops and resends after about 10 minutes of waiting | Its event-level watchdog | `CLAUDE_STREAM_IDLE_TIMEOUT_MS=86400000` in the shell or in `~/.claude/settings.json` `env`; `API_TIMEOUT_MS` does not control it (clients.md, Claude Code) |
| Codex prints `idle timeout waiting for SSE`, then `Reconnecting... n/5`, then fails the turn | Provider `stream_idle_timeout_ms` (300000 by default) counts SSE events | Set `stream_idle_timeout_ms = 86400000` under `[model_providers.<id>]` in `~/.codex/config.toml` (clients.md, Codex) |
| Qwen Code drops after about 4 minutes, or always at 15 minutes | Event-level idle timeout and a per-stream lifetime cap | `QWEN_STREAM_IDLE_TIMEOUT_MS=0` and `QWEN_STREAM_MAX_LIFETIME_MS=0` (from source, not run) |
| Gemini CLI fails with `Incomplete JSON segment at the end` | An SSE comment reached its pinned `@google/genai` 1.30.0 parser. aProxy never sends heartbeats to Gemini CLI's requests, so another hop in the chain inserted it | Remove or reconfigure the other proxy; do not try to enable heartbeats for Gemini CLI (clients.md, Gemini CLI) |
| Gemini CLI fails on long generations or during upstream outages | Its hard-coded response-header timeout (60 s in 0.62.0) runs out, because aProxy sends the head only after a complete successful attempt | No fix in aProxy or the client today (clients.md, Gemini CLI) |

## Transformers and forward-only mode

| Symptom | Cause | Fix |
|---|---|---|
| 502 `请求转换失败: …` ("request transform failed"), or the event `proxy_transform_failed` | The request transform failed; the request was not sent upstream and is not retried | The reason after the colon says which: `format 进程启动失败` (process failed to start: check `command`), `format 报告转换失败` (the format reported an error), `转换超时` (timed out: raise `timeout_secs` or fix a stuck format), `format 输出违反信封协议` (broke the envelope protocol: extra stdout lines or invalid JSON), `format 进程意外退出且无输出` (exited without output), `format 进程管道读写失败` (pipe failure), `body 临时文件读取失败` (local disk). Details: aproxy-format skill, troubleshooting.md |
| A transformer is configured but responses come back untransformed | `response_transform` is not configured (it is separate from `request_transform`); or it failed and the original was passed through (log `响应转换失败，透传上游原始响应`); or the response was a bounded-retry pass-through, which is never transformed | Configure it, fix the format, or accept that pass-through failures stay raw |
| Header changes from the response transform are missing on streaming requests | A skeleton head was already committed, because retries or the response transform itself outlasted one keepalive interval; after that only the body transform applies | Expected (behaviors.md, External transformers) |
| Start fails with `forward_only 与外部转换器互斥：…` ("forward_only and external transformers are mutually exclusive") | Forward-only does not buffer bodies, transformers need them whole | Keep one of the two |
| The user wants retries off and true incremental streaming | That is forward-only mode | `forward_only = true` in that instance's config.toml (or as a settings.json default), then `aproxy restart <port\|alias>`. Make sure the user accepts losing retries, buffering and heartbeats |
| Forward-only instance: 502 `上游请求失败: …（仅转发模式不重试）`, or a stream cut off midway | Expected: forward-only never retries and never injects bytes the upstream did not send | Nothing to fix in aProxy; switch back to the default mode if retries are needed |

## Logs and files

| Symptom | Cause | Fix |
|---|---|---|
| Chinese text in a log looks garbled | The reading tool decodes the BOM-less UTF-8 file with the system code page; Windows PowerShell 5.1 `Get-Content` does this by default. aProxy sets the console to UTF-8 (65001) itself, so `chcp` is not needed | Use `aproxy logs <port>`, or `Get-Content -Encoding UTF8 <file>`, or PowerShell 7 |
| No log file for the port can be found | Log names are random and new at every start; they do not contain the port | `aproxy logs <port\|alias>` prints the path; never construct it from the port |
| A stopped instance's log is gone | Commands that list instances delete logs no instance refers to | Copy logs you need before stopping, or set `log_file` |
| `aproxy logs` says `该实例以前台模式运行（--foreground），日志输出在它的控制台，无日志文件可跟随。` ("this instance runs in the foreground; its log goes to its console") | Foreground instances have no log file | Read the console it runs in |
| `aproxy logs` never returns | It follows the file until Ctrl+C or the instance stops | Run it with a timeout, or read the file at the printed path |
| The error preview reads `（二进制/压缩内容，共 N 字节，hex 前 48: …）` ("binary/compressed content, N bytes, first 48 bytes in hex") | The body is binary or could not be decoded (unknown encoding, corrupt data, over 8 MiB decoded) | Check the `content_encoding` field on the same line; `1f 8b` starts gzip, `28 b5 2f fd` starts zstd |
| `*.spooltmp` files remain in `<home>/spool/<port>/` | Left by a crash or kill | Harmless; they are deleted the next time an instance starts on that port |

## Crash recovery, watchdog and upgrades

| Symptom | Cause | Fix |
|---|---|---|
| An instance came back after it was killed | It was killed by PID (or crashed), so its restore record stayed and the watchdog respawned it | Stop instances with `aproxy stop <port>`, or `--force` if hung |
| After a crash, an instance came back with different settings or on another port | A respawn replays the original command line but reads the config file as it is now | Keep config.toml in the state you want running; apply edits with `aproxy restart` instead of leaving them pending |
| A crashed instance stays down; startup.log has `[watchdog] 端口 <port> 的实例连续重拉 N 次失败，已放弃自动重拉；…` ("gave up respawning after N failures") | Crash loop: every respawn failed | Fix the cause shown earlier in startup.log, then `aproxy restore` |
| Is the watchdog running? | `aproxy status` does not show it | Read `<home>/run/watchdog.claim` and check that its PID is alive. Instance logs show `看护者缺席，已由守护补种（选举胜出者）` when an instance relaunched it. It is off when settings.json has `watchdog: false`, and unsupported on macOS. `aproxy start` launches it if absent, and instances check every 5 minutes |
| Instances are not back after a reboot | The watchdog does not survive a reboot; the restore records do | `aproxy restore`; register it as a logon task to make this automatic |
| `aproxy restore` reports `端口 N 的实例（pid P）启动后立即退出…` or `…未在预期时间内就绪…` ("exited right after start" / "not ready in time") | Config error, bind failure, or slow start | Read `<home>/logs/startup.log` |
| `aproxy status` shows `二进制更换中（install 滚动重启阶段，请勿手动干预此实例）` ("binary swap in progress; do not intervene") | `aproxy install` is mid-rollout | Leave the instance alone until install finishes |
| Install aborts with `实例未表达（已按重试/restart 收敛）` or `实例 <port> 收敛重启失败` | That instance did not enter the swap phase even after restarts | Ask the user whether it may be stopped; stop it and run `aproxy install` again |
| Install exits non-zero with `滚动已中止，不会自动重试：…` ("rollout halted, will not retry") | The new version could not start an instance; it was brought back on the old binary | `last_error` in `<home>/run/install.state` and startup.log give the reason (often stricter validation of an existing config); fix it and run `aproxy install` again |
| `aproxy status` notes `注意: 实例版本 vX 与当前 CLI vY 不同，…` ("instance version differs from the CLI") | That instance still runs an older binary | `aproxy restart <port>` moves it to the installed version; it cuts in-flight requests, so confirm the instance is idle or the user agrees |

## Configuration files

| Symptom | Cause | Fix |
|---|---|---|
| `警告: settings.json 解析失败（…），别名等内部配置已回退为空` ("settings.json failed to parse; aliases and other internal settings fell back to empty") | Invalid JSON in `<home>/settings.json` | Fix the JSON at the reported position. Deleting the file also clears aliases and global defaults, so ask the user first |
| A config change has no effect | Instances read their config only at start; and an instance started with CLI overrides keeps them across restarts | `aproxy restart <port\|alias>`; to drop a CLI override, stop the instance and start it without the flag |
| The user wants everything kept in memory, with no spool files | Disk cache is on by default | `disk_cache = false` in config.toml (or as a settings.json default), then restart |
