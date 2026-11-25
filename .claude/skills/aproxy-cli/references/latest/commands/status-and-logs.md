# aproxy command reference: status and logs

Part of [commands.md](../commands.md), which lists the other parts. Reading what runs and what it logged: `status`, `logs`.

## status

```
aproxy status [--idle | --busy] [--json]
```

Takes a read-only inventory of the home: every instance in the registry, desired-state records of
instances that are not running, records this version cannot read, the watchdog, and other aProxy
processes in the process ledger. A record whose process is gone (or whose pid now belongs to
another process) is not an instance. `status` deletes nothing: stale records, leftover launch-spec
files and old unreferenced logs are cleaned in the background by an instance after it starts
serving and by the watchdog (behaviors.md, Logs). The text output has these blocks, each printed
only when it has entries, in this order:

- `无响应的 aProxy 实例 (<n>)——进程仍在，但不应答控制通道:` ("unresponsive instances: the
  process is still there but does not answer the control channel"), with port, pid, version, config
  and the hint `aproxy stop <port> --force`. Such an instance is almost always hung; with the
  watchdog on, its hang detection terminates and respawns it on its own. This is the only state
  where `--force` is the right answer.
- `运行中的 aProxy 实例 (<n>):` ("running instances"), described below.
- `无法采信应答的 aProxy 实例 (<n>)——…` ("instances whose answer cannot be trusted": a different
  protocol version or an identity mismatch), with port, pid, version and a reason code. Operate such
  an instance with the aproxy version it runs; do not `--force` it.
- `等待恢复的实例 (<n>)——…` ("instances waiting to be restored"): not running, but their
  desired-state record is kept, so `aproxy restore` or the watchdog will start them again.
  `（配置文件找不到）` marks one whose config file is missing; restoring it fails until the file is
  back. `aproxy stop <port>` drops the record when the instance is no longer wanted.
- `读不懂的记录 (<n>)，原样保留…` ("unreadable records, kept as they are"): files in `run/` this
  version cannot parse, usually written by a newer aproxy; handle them with that version. While
  they exist, `日志清理已暂停` ("log cleanup is paused") is printed too.
- `看护者: pid <pid>  v<version>  最近扫描 <age> 前` ("watchdog, last scan <age> ago"), or
  `已 <age> 没有扫描（可能卡住了）` ("no scan for <age>; it may be stuck").
- `台账里的其他 aProxy 进程 (<n>):` ("other aProxy processes in the ledger"): pid, role and version
  of processes that belong to no instance or watchdog, such as the desktop window or instances
  started by a version before 0.2.0.

With nothing in any block, it prints `没有运行中的 aProxy 实例。` ("no running instances").

Per running instance: port, pid, `v<version>`, uptime, idle time, listen address, masked
upstream, config path (`（没有配置文件）` when it was started without one), and the line
`请求 <n>  在途 <n>  重试 <n>  救回 <n>  最近错误: …`: requests received, requests still in flight
(waiting for the upstream, backing off, retrying or still being delivered), upstream retries,
requests that succeeded only after a retry ("rescued"), and the latest error as
`"<code>: <English message>"（<age> 前）` or `无` ("none"). Extra lines can appear:

- `正在启动（还没开始接收请求）` ("starting; not accepting requests yet").

- `二进制更换中（install 滚动重启阶段，请勿手动干预此实例）` ("binary swap in progress; do not
  intervene"): `aproxy install` is about to restart this instance. Leave it alone.
- `正在退出` ("shutting down"): it received a stop and is finishing open requests (up to 10 s).
- `状态未知（实例比当前 CLI 新，用同版本的 aproxy 查看）` ("unknown state; the instance is newer
  than this CLI"): run the same version of aproxy as the instance.
- `注意: 实例版本 v<x> 与当前 CLI v<y> 不同…` ("instance version differs from this CLI"):
  see compatibility.md.

`--idle` shows only running instances idle for at least `instances.idle_threshold_secs`
(settings.json, default 1800); `--busy` the running ones that are not. An instance that has served
no request yet is idle from its start time. The filters apply to running instances only: the other
blocks are always shown, because each of them needs attention whatever the filter. `status` does not
print log paths; see [logs](#logs).

`--json` prints the same inventory as one document, which is the form to parse:

```json
{"schema":1,
 "instances":[{"port":12346,"liveness":"live","instance":{"pid":4321,…},"status":{"state":"serving",…},"restore_pending":false}],
 "unreadable":[{"file":"run/12349.instance.json","reason":{"code":"record_schema_too_new","details":{…}}}],
 "watchdog":{"pid":5555,"version":"0.2.0","state":"running","last_scan_at":1760190123456,…},
 "processes":[{"pid":7777,"role":"desktop","version":"0.2.0",…}]}
```

`liveness` is `live`, `unresponsive`, `incompatible`, `down`, or `unknown` for a state this version
does not know. A `live` entry carries `status` (state and the request counters); an `incompatible`
one carries `reason` (`code`, `details`); a `down` one has `instance: null`, `restore_pending: true`
and, when the config file is missing, `restore_reason: "config_missing"`. `gc_paused` appears only
while log cleanup is paused; `watchdog` is `null` without a watchdog, and its `state` is `running` or
`stale`. `--idle`/`--busy` filter only the `live` entries here as well.

## logs

```
aproxy logs [PORT|ALIAS|PATH|default] [-n N] [-f]
```

Prints the instance's most recent log lines (200 by default) and exits. `-n N` / `--lines N` sets
the count; `-f` / `--follow` keeps printing new lines until the instance stops
(`实例已停止，日志跟踪结束。`, "instance stopped, following ended") or you press Ctrl+C. Log lines
go to stdout; the explanatory lines (`日志文件: <path>`, the follow notice) go to stderr, so a
program reading the pipe receives only log lines. It asks the instance for its log path, because
log files get a new random name at every start.

- **Without `-f` it returns by itself**, so it suits a tool call. With `-f` it does not return while
  the instance runs: run it with a timeout (`timeout 5 aproxy logs -f 12345` in a POSIX shell, Git
  Bash included), take the path from its `日志文件: <path>` ("log file") line and read that file
  directly. The same path is the `log_path` field of the instance record
  `<home>/run/<port>.instance.json` (one line of JSON; `null` for a foreground instance).
- Several instances and no target, and `all` (`aproxy logs 不支持 all…`), exit 2.
- A `--foreground` instance has no log file and exits 1:
  `端口 <port> 的实例以前台模式运行（--foreground），日志输出在它的控制台，没有日志文件。` ("runs in
  foreground; its log is in its console; no log file").
- **A target that is not running but still has its desired-state record** (it crashed or was
  killed) gets the end of the log file named in that record, which is where the crash is recorded.
  stderr says `端口 <port> 的实例没在运行，以下是它上次运行的日志。` ("not running; this is the log of
  its last run"), and `-f` does not follow. With no target and nothing running, `logs` does this
  when exactly one record exists and exits 2 listing the ports when there are several. With
  nothing to read it exits 4.
- `── 日志文件已被截断，重新从头跟踪 ──` ("log file truncated, following from the start") appears
  while following when log rotation empties the file.
