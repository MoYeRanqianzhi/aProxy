# 计划：IPC v1 重构 + 去 alpha 兼容 + 0.1.0 升级路径（2026-10-06 起）

## 为什么
用户 2026-10-06：发布前各部分都要足够优秀；去掉 alpha 时代的全部兼容；把 0.1.0 当作真正的第一版；
IPC 重新整理格式后协议版本从 1 重新计数。随后：「发现的问题就修复，别问我，你自己决定」。

## 主代理的取舍（在下方设计报告之上）
- 采纳报告的整体方案。0.1.0 → 下一版的原地升级必须可用（0.1.0 已发布），所以报告 c.5 的 S1–S5
  兼容垫片要做，集中在一个模块里，整个 0.1.x 线保留、0.2.0 删除；这不是 alpha 兼容，要向用户说明。
- `defult` 保留（alpha.5 有意加的笔误容错，13598b3），不在删除范围。
- 顺序调整：第 2 步只删 InstanceInfo 之外的 alpha 兼容；InstanceInfo 的 serde default 随第 3/4 步
  把 `.pid` 拆成 InstanceRecord 时一并处理，避免同一结构改两遍。
- T1-E2E（真实 v0.1.0 二进制驱动升级）只在 CI 跑，不在本机跑：本机有用户的生产实例，0.1.0 的
  Windows 管道是全机共享的。
- 设计报告由 Plan 子代理读码写成（未执行代码），标 [hypothesis] 的结论要靠对应测试确认。

## 进度
- 第 1 步（部分）：`tests/compat_v0_1_0.rs` 冻结 0.1.0 的 InstanceInfo / IpcRequest / RestoreRecord /
  WatchdogClaim，双向解析断言（5 项）。install.state 的冻结副本与 T1-E2E 留到第 6 步改 install 时补。
- 第 2 步（部分）：删掉 `RecordIdentity::Unverifiable`（登记值 0 一律按 Gone）、`force_terminate` 的
  IPC 归属退路与两个平台的 `imp::terminate_process`、status/stop idle 的 `last_activity_secs > 0` 守卫、
  若干「旧版本」注释与对应测试。InstanceInfo 的 serde default、`IPC_PROTO_V1`、`Stats` op、status 的
  `proto_version >= 2` 留给第 3/4 步（拆 InstanceRecord 时一起改）。install.state 的 serde default 不删：
  它同时是「新字段加入」的前向兼容规则，删掉收益小。
- Linux 编译只能靠 CI 的 ubuntu 门禁：本机 WSL（Debian）的 rustup 工具链清单损坏，且 2026-10-06 WSL 内无外网
  （官方源与 rsproxy 均连接超时），修不了；本机也没有 Linux C 交叉编译器（ring 需要）。

---

# aProxy IPC v1 redesign — design report

Scope: design only (no code changed). Line numbers are at HEAD `24d342a`. During the review `535efaa`
landed and already removed the `upstream_url` config alias and the `.restore` bare-array format;
those two items are therefore not repeated below. `src/install/` is byte-identical to tag `v0.1.0`
(`git diff --stat v0.1.0 -- src/install` is empty), so the install flow quoted here *is* the 0.1.0
installer that users will run.

Evidence labels: **[code]** = read in source; **[hypothesis]** = reasoned from code, not executed —
each has a test in (d) that settles it.

---

## (a) Alpha-era compat to delete

| # | Location | What | Note |
|---|---|---|---|
| 1 | `src/daemon.rs:110,115,118,121,124,127,132,140,154` | `#[serde(default)]` on every `InstanceInfo` field after `started_at` (+ doc text at 108-109, 112-113, 134-139, 149-153) | 0.1.0 serializes the whole struct, so every 0.1.0 `.pid` file / ping reply contains all fields; the defaults only serve alpha files. In the redesign the live counters leave the record anyway. |
| 2 | `daemon.rs:161-165` | `IPC_PROTO_VERSION = 2`, `IPC_PROTO_V1 = 1` | numbering restarts at 1 |
| 3 | `daemon.rs:194-200` | `#[serde(default = "ipc_proto_v1_default")] proto` + `fn ipc_proto_v1_default` | alpha.5 replies without `proto` |
| 4 | `daemon.rs:175-177` + `Ping \| Stats` arm in `handle_conn` | `IpcRequest::Stats` | alpha capability probe; answered identically to `Ping` |
| 5 | `daemon.rs:355-382` | `force_terminate` branch for `process_start == 0` (re-ping ownership proof) | 0.1.0 always writes a non-zero `process_start` on Windows/Linux |
| 6 | `daemon.rs:1087` (Windows), `daemon.rs:1168` (unix) | `imp::terminate_process` | only caller is #5 |
| 7 | `daemon.rs:658-662` | survey doc "未登记创建时间的记录…按旧语义清理" | comment |
| 8 | `daemon.rs:785-797` | `retire_moved_port_records_in` `Unverifiable` arm | |
| 9 | `src/watchdog.rs:165-170`, `189-190` | `RecordIdentity::Unverifiable` variant and its production | |
| 10 | `watchdog.rs:222-241` | `confirm_registered_instance` IPC-ownership fallback for unverifiable records (doc 226-229) | |
| 11 | `watchdog.rs:268-272`, `452`, `509`, `800`, `1101` | "旧版本守护" comments | 509/1101: keep the *creation-failed* degrade, drop the old-daemon wording |
| 12 | `src/commands/status.rs:25-26`, `47-51` | `last_activity_secs > 0` guard ("0 = 旧版本守护") | since 0.1.0 the atomic is initialised to "now" (`src/proxy.rs:366-370`) |
| 13 | `status.rs:66-68` | `if info.proto_version >= 2` (v1 instances show "—") | |
| 14 | `src/commands/stop.rs:45-47` | same `> 0` guard for `stop idle` | |
| 15 | `src/server.rs:196-201` | comment about old daemons not knowing the announcement | 0.1.0 knows it; comment only |
| 16 | `src/install/broadcast.rs:4-11`, `33` | "旧实例（无此 op）" wording | 0.1.0 supports PrepareSwap. Keep the convergence-restart mechanism (it also handles faulty instances); fix the docs |
| 17 | `src/commands/restart.rs:105` | ".restore 缺失（旧版本守护/记录异常）" | fallback still valid for a lost record; comment only |
| 18 | `src/install/state.rs:163-165` + all `#[serde(default)]` at 141-212 | defaults on fields 0.1.0 already writes | 0.1.0 writes every field. Rule going forward: any field added after 0.1.0 *must* be `default` (N's continuator reads 0.1.0 state) |
| 19 | `install/state.rs:577-587` | test fixture `0.1.0-alpha.8` with missing fields | |
| 20 | tests: `daemon.rs:1313` (process_start=0 half of `force_terminate_checks_identity_not_name`), `1332` (`force_terminate_legacy_record_uses_fresh_ipc_ownership`), `1404` (`write("59745", pid, 0)`), `1527` (comment "旧实例同样回 ok:false"), `1537` (`ipc_v1_compat_old_response_and_registry`); `watchdog.rs:1714`, `1791`, `1859` | legacy cases | |
| 21 | `.claude/skills/aproxy-cli/references/latest/compatibility.md` | alpha per-version rows | docs |
| — | `src/settings.rs:382-385`, `399` (`defult`) | **not version compat.** 0.1.0 ships it as a typo convenience for the `default` reserved word (introduced by 13598b3). Removing it is a user-visible 0.1.0 behaviour change; decide separately (CHANGELOG if removed). |

---

## (b) IPC v1 design

### b.1 Problems found in the current layer (beyond the brief)

1. **Windows control plane is machine-wide** [code]: `endpoint_for_in` ignores `run_dir` on Windows (`daemon.rs:81-87`). Consequences:
   - `stop 12345` from a test home stops the production instance (`stop.rs:95-118` pings by port, no registry).
   - `start` in a test home on a port the production home uses prints "已有 aProxy 在运行，无需重复启动" for the *other* home's instance (`start.rs:185-193`).
   - `logs <port>` reads another home's log path (`logs.rs:46`).
   - Two homes on the same port with different loopback addresses (127.0.0.1 vs 127.0.0.2) can both bind TCP but collide on the pipe; the second daemon's `first_pipe_instance(true)` fails (`daemon.rs:1143`).
2. **Heartbeat section and install announcement are also global** [code]: `Local\aproxy-heart-<port>` / `/dev/shm/aproxy-heart-<port>` (`watchdog.rs:1048-1056`), `Local\aproxy-install` / `/dev/shm/aproxy-install` (`install/announce.rs:19-28`). One home's install switches another home's watchdog into install mode and suppresses its daemons' watchdog re-seeding (`server.rs:202-206`). On unix `/dev/shm` is shared by all users.
3. **Survey trusts any responder** [code]: `survey_instances_in` pushes `live_info` from whoever answers the endpoint without checking `live_info.pid == record.pid` (`daemon.rs:683-688`).
4. **Daemon runs on without IPC** [code]: `serve_ipc` is spawned and its failure only logged (`server.rs:157-162`); the registry and `.restore` are written *before* the endpoint exists (`server.rs:129`, `146` vs `159`; also noted at `daemon.rs:741-744`). An instance can be listed but uncontrollable.
5. **unix bind steals endpoints** [code]: `imp::serve` unlinks any existing socket before bind (`daemon.rs:1201`) — a second daemon with the same port key silently takes over the live one's endpoint.
6. **Error model** [code]: replies are `{ok:false}` with no reason; unknown ops rely on deserialization failure (`daemon.rs:1051`). All client errors are `String`; `wait_until_gone` (`daemon.rs:406-424`) counts *any* error (timeout, busy) as "gone", so a hung instance can be reported as stopped.
7. **Unbounded client read / no server read timeout** [code]: client `read_line` has no cap (`daemon.rs:959`); `handle_conn` has no read timeout (`daemon.rs:968-985`), so an idle client pins a task (and on Windows a pipe instance) forever.
8. **Env-derived helpers bypass the injected run dir** [code]: the watchdog uses `crate::daemon::ipc_ping(&w.port)` (`watchdog.rs:516`) and `remove_socket_file(port)` / `remove_heartbeat_file(port)` (`watchdog.rs:558-559`) instead of `self.cfg.run_dir`. Same bug class the comment at `daemon.rs:76-80` warns about.
9. **Claim refresh is not atomic** [code]: `refresh_claim` uses `std::fs::write` in place (`watchdog.rs:756-758`); a concurrent reader can see a truncated file → "no claim" → spurious watchdog spawn. (`.pid`/`.restore`/`install.state` already use tmp+rename.)
10. **Pipe squatting / forged ownership** [code + reasoning]: pipe names are predictable and global; whoever creates `\.\pipe\aproxy-<port>` first owns it. `force_terminate` terminates the pid+start reported *by the responder* (`daemon.rs:366-368`), so a squatter can steer `stop --force` at an arbitrary process of the user. Nothing checks the transport peer.

### b.2 Endpoint naming and the home namespace

- `home_id` = first 16 hex chars of SHA-256 over the canonical run dir (`std::fs::canonicalize`; Windows: strip `\?\`, lowercase). Computed once by a new `RunDir` newtype that carries `{path, home_id}` and is passed explicitly everywhere; delete the env-derived convenience functions (`endpoint_for`, `ipc_ping`, `ipc_request`, `remove_socket_file(port)`, `read_heartbeat(port)`, `instance_file_path(listen_addr)` etc.) so #8 cannot recur.
- Why run dir and not `APROXY_HOME`: every coordination artifact (`.pid`, `.restore`, `.sock`, `watchdog.claim`, `install.state`) lives in run dir, and `APROXY_RUN_DIR` redirects it independently (`daemon.rs:24-29`). The run dir *is* the namespace.
- Names:
  - Windows pipe: `\.\pipe\aproxy-<home_id>-<port>`
  - Windows heartbeat: `Local\aproxy-<home_id>-heart-<port>`; announcement: `Local\aproxy-<home_id>-install`
  - unix socket: **unchanged** `run/<port>.sock` (already per-home; also the 0.1.0 path, so it costs nothing for the upgrade). Fail start if the path exceeds `sun_path` (107 bytes) instead of running without IPC.
  - unix shm: `/dev/shm/aproxy-<home_id>-heart-<port>`, `/dev/shm/aproxy-<home_id>-install` (fixes cross-user leakage too).
- Discovery when the registry is lost: same-home `stop <port>` still computes the endpoint from `(home_id, port)` — no registry needed, exactly as today. What disappears is reaching another home, which is the goal.
- Diagnosing "port held by another home's aProxy": the ping result carries `run_dir`; optionally `doctor` on Windows enumerates `\.\pipe\aproxy-*` (FindFirstFileW on `\.\pipe\*`) and pings them to name the owning home.
- Trade-offs: names are opaque (mitigated by `run_dir` in the reply). Moving/renaming a home or changing `APROXY_HOME` while instances run orphans them from the CLI (unix already behaves this way).
- Port remains the instance key (registry, restore, socket, pipe, heartbeat, spool). Keep it, but enforce it in the daemon, not only in `start`'s preflight (`start.rs:180-198`), because watchdog respawn / restore / install bypass that preflight: the namespaced endpoint must be exclusively created (Windows `first_pipe_instance(true)`; unix connect-probe before unlinking — refuse if a live server answers, unlink only on ECONNREFUSED/ENOENT).

### b.3 Startup invariant

`bind TCP → create IPC endpoint (fatal on failure, exit 1 with startup.log line) → write .pid → write .restore → heartbeat → serve`.
Result: "a `.pid` record exists ⇒ its endpoint answers" (modulo hang). Readiness checks no longer need the registry-then-ping dance, and an instance can never run invisible.

### b.4 Framing

Unchanged in shape: one UTF-8 JSON line + `\n` per request, one per response, one exchange per connection, connection closed after the response.
- Request cap 64 KiB (exists, `daemon.rs:983`); **new** client response cap 1 MiB; **new** server read timeout 5 s; client total timeout 3 s (exists).
- Distinguish "endpoint absent" (ENOENT / ERROR_FILE_NOT_FOUND → definitive, no retry) from busy/timeout (retry). Today `ping_endpoint` retries 3× even when nothing exists (`daemon.rs:223-238`).

### b.5 Request/response schema

Request:
```json
{"v":1,"op":"ping"}
{"v":1,"op":"shutdown"}
{"v":1,"op":"prepare_swap"}
{"v":1,"op":"<future op>","args":{...}}
```
- `v` (required): wire major version.
- `op` (required): snake_case op name.
- `args` (optional object): op arguments; absent = `{}`.
- No request id (one exchange per connection).

Success response (all v1 ops return the status snapshot; op-specific fields are added under `result` as needed):
```json
{"v":1,"ok":true,"result":{
  "instance":{"pid":1234,"process_start":133000000000000000,"version":"0.1.1",
              "listen_addr":"127.0.0.1:12345","config_path":"C:/Users/x/.aproxy/config.toml",
              "base_url":"https://api.example.com","started_at":1760000000,
              "log_path":"C:/Users/x/.aproxy/logs/1f3a2b-00ab.log"},
  "run_dir":"C:/Users/x/.aproxy/run",
  "state":"serving",
  "activity":{"last_request_at":1760000100,"requests_total":42,"retries_total":3,
              "last_error":{"message":"upstream 502 ...","at":1760000090}},
  "ops":["ping","shutdown","prepare_swap"]}}
```
- `instance` = `InstanceRecord`, the **same struct** written to `run/<port>.pid` (see b.7).
- `state`: `serving | swap_prepared | stopping` (replaces `swap_phase: bool`; extensible, e.g. `draining`). Modeled with `#[serde(other)] Unknown`; clients treat `Unknown` as "not serving".
- `activity.last_request_at`: never 0 (initialised to start time); `last_error`: `null` when none (replaces `last_error` + `last_error_at = 0` sentinel).
- `ops`: capability list; clients consult it before invoking side-effecting newer ops.
- `shutdown`: response is written before the stop is triggered (as today); `state` in that reply = `stopping`.
- `prepare_swap`: idempotent; reply shows `state = swap_prepared` (ACK = the observable state, as today).

Failure response:
```json
{"v":1,"ok":false,"error":{"code":"unknown_op","message":"unknown op \"what\""}}
{"v":1,"ok":false,"error":{"code":"unsupported_version","message":"...","supported":[1]}}
```
Error codes (stable snake_case strings; `message` is for humans and is never parsed):
- `bad_request` — not JSON, missing `v`/`op`, line too long, malformed `args`
- `unsupported_version` — `v` not served; carries `supported`
- `unknown_op` — op not implemented (no side effects; safe capability probe)
- `invalid_state` — op not allowed now (e.g. `prepare_swap` while `stopping`)
- `internal` — server-side failure
Clients map unknown codes to a generic remote failure.

Client-side error type (replaces `Result<_, String>`):
```rust
enum IpcError {
    Unreachable,               // endpoint absent: definitive "no instance here"
    Transient(String),         // busy / timeout / io: retry, "maybe hung"
    Protocol(String),          // unparsable reply, wrong v, oversize, peer-pid mismatch
    Remote { code: ErrorCode, message: String },
}
```
`wait_until_gone`, `stop`, survey and install use `Unreachable` vs `Transient` to tell "stopped" from "hung".

### b.6 Versioning rules

- `v` bumps only for semantic breaks. Additive changes never bump it: new ops, new optional `args`, new `result` fields, new error codes, new `state` values.
- Neither side uses `deny_unknown_fields`; every enum crossing the wire has an `Unknown` catch-all.
- A daemon may serve several majors by dispatching on `v`.
- Files (`.pid`, `.restore`, `watchdog.claim`, `install.state`) carry **no** version field. They evolve additively (new fields `Option`/`default`). A breaking change uses a **new file name/extension**, because old readers *delete* unparsable files of known extensions (0.1.0 survey, 0.1.0 `list_restore_entries_in`), whereas files with unknown extensions are ignored.

### b.7 Records and identity

- `.pid` = `InstanceRecord` {`pid`, `process_start`, `version`, `listen_addr`, `config_path`, `base_url`, `started_at`, `log_path`} — all required, exactly the names 0.1.0 writes. Live counters (`last_activity_secs`, `proto_version`, `requests_total`, `retries_total`, `last_error`, `last_error_at`, `swap_phase`) leave the file (IPC only).
  - Constraint for 0.1.0 readers: the 6 fields 0.1.0 requires (`pid`, `version`, `listen_addr`, `config_path`, `base_url`, `started_at`) must stay present; `log_path` must be `""` (never `null`, 0.1.0 types it `String`); `process_start` a number.
  - N's required set ⊆ the set 0.1.0 writes ⇒ N reads 0.1.0 files with zero compat code.
- `.restore` = {`args`, `log_path`} unchanged (strict since 535efaa). **Never rename `args`**: 0.1.0 parses `.restore` with `#[serde(default)]` on both fields, so a renamed field reads as `args = []` and the 0.1.0 watchdog would respawn the instance with the default config.
- `watchdog.claim` = {`pid`, `created_at_process`, `heartbeat_secs`, `version`?}. Keep the three fields (0.1.0 must parse it for its yield logic, see c.3); `version` optional (0.1.0 claims lack it). Write via tmp+rename.
- `process_start` required. `record_identity` loses `Unverifiable`: a 0 value is "not a valid record" (never kill, never adopt). Unsupported platforms (macOS, out of scope) simply never verify.
- Transport-peer check: client compares the OS-reported peer pid (Windows `GetNamedPipeServerProcessId` — needs windows-sys feature `Win32_System_Pipes`; Linux `SO_PEERCRED`) with `result.instance.pid`; mismatch → `Protocol` error. Survey additionally requires `peer pid == record.pid && reported process_start == record.process_start`, otherwise the record is stale. This makes the "endpoint ownership proof" unforgeable and closes b.1 #3 and #10.

### b.8 Install jurisdiction (problem 3)

- [code] The online path never runs the jurisdiction check: `run_online` (`commands/install.rs:208-384`) calls `run_install_online` directly; only `run_plan` (`install.rs:503`, used by `--from`/`--adopt`) calls `check_jurisdiction`. Instances launched from npm/cargo/dev binaries in the same home are silently restarted onto `<home>/bin/aproxy`, contradicting `.agents/plan/install-v1.md` ("管辖检查（安装前）").
- [code] Instance enumeration is per-run-dir (`flow.rs:87-93` → `list_instances_in(run_dir)`), but on Windows each record is validated by pinging the global pipe and the responder is not checked against the record (b.1 #3). So a stale `.pid` for port P in home A plus a live home-B instance on P makes A's online install: broadcast PrepareSwap to B's instance, then `restart_instance` stops it via `Shutdown` and respawns A's `.restore` args on A's binary (`restart.rs:282-313`). `--from` would be caught by `check_jurisdiction` (B's image is not under A/bin); online is not.
- ACK-convergence restarts use the installer's own `current_exe()` (`broadcast.rs:53`).
- Fix: namespaced endpoint + responder/peer identity check (b.7) + run `check_jurisdiction` on every entry point, before downloading (online included); `--continue` does not re-check (instances may be mid-roll).

---

## (c) 0.1.0 -> N upgrade analysis

### c.1 Who runs each step

| Step | Windows | Linux |
|---|---|---|
| latest query, download, `--version` probe of staged N (`install.rs:208-364`, `staging.rs:45-58`) | 0.1.0 | 0.1.0 |
| create `run/install.state` (lock) + announcement `aproxy-install` (`flow.rs:464-484`) | 0.1.0 | 0.1.0 |
| jurisdiction check | **none** (online) | **none** |
| stop old watchdog (`flow.rs:286-287`, `701-718`) | 0.1.0 | **not done** (`#[cfg(windows)]`) |
| PrepareSwap broadcast + ACK convergence | 0.1.0 to 0.1.0 instances | same |
| swap (`swap.rs:73-116` / `143-169`) + probe N in bin | 0.1.0 | 0.1.0 |
| relay: spawn `N install --continue` (`swap.rs:229`), wait up to 30 s for phase restarting or later (`swap.rs:241`) | 0.1.0 spawns, **N** parses 0.1.0 `install.state` (`flow.rs:513-613`) | n/a |
| restarting: Shutdown old instance, spawn N daemon, readiness | **N** (stops 0.1.0 daemons: old pipe, old wire, reads 0.1.0 `.pid`/`.restore`) | **0.1.0**: readiness = `registry_contains_pid_in` with the 0.1.0 parser on N's `.pid` (`restart.rs:202`) |
| verifying: ping, `version == target && !swap_phase` (`flow.rs:389-396`) | **N** | **0.1.0 client to N daemon** on `run/<port>.sock` |
| verify-failure re-restart (`flow.rs:405-421`) | N | 0.1.0 (sends `Shutdown` to N daemons) |
| cleaning (delete `.old`, staging, `install.state`) | N (`.old` = 0.1.0 installer's image, locked, retained; pre-existing) | 0.1.0 |
| user-facing report: `await_handover` polls `install.state` (`install.rs:440-474`) | **0.1.0 reads N-written state** | n/a |
| relay fallback if no takeover in 30 s (`flow.rs:331-335`) | **0.1.0** runs restart/verify itself (like Linux), concurrently with a late N | n/a |
| watchdog after install | N daemons re-seed an N watchdog once the announcement is gone (`server.rs:185-235`) | 0.1.0 watchdog keeps the claim (see c.3) |
| `install --continue` on interruption | spawned by watchdog/CLI with *their* `current_exe()` (`watchdog.rs:824-845`, `1019-1030`) | 0.1.0 watchdog's exe |

### c.2 What each proposed change would break

| Artifact | If changed naively | Decision |
|---|---|---|
| `.pid` name/location/required fields | 0.1.0 readiness fails: 8 s timeout, `reap_unready_child` kills the N child, rollback to `.old`, `halted` (Linux; Windows fallback). 0.1.0 survey **deletes** unparsable `.pid`. | Keep (b.7). Zero shim. |
| `.restore` field names | 0.1.0 watchdog/`restore` respawn with `args = []` (default config) or delete the file | Keep. Zero shim. |
| Windows pipe name | N continuator cannot reach 0.1.0 daemons; 0.1.0 fallback verify, 0.1.0 CLIs and 0.1.0 survey cannot reach N daemons, and 0.1.0's survey (which at `v0.1.0` deletes a record on any ping failure) would delete every N `.pid` | Shims S1 + S2 |
| unix socket path | same as above on Linux | Keep `run/<port>.sock`. Zero shim. |
| Request format | none: 0.1.0 accepts `{"v":1,"op":"ping"}` because serde's internally-tagged unit-variant visitor ignores extra keys (`serde-1.0.229/src/private/de.rs:2990-2994`) | Lock with a frozen-struct test |
| Response format | 0.1.0 needs `{ok, info{..., version, swap_phase}}` (0.1.0 `ping_endpoint`: `info` missing means error); N continuator must parse 0.1.0 replies | Shims S3 + S4 |
| Heartbeat section name | 0.1.0 watchdog sees "no heartbeat" for N daemons and only loses hang detection; it is retired anyway (R1) | No shim |
| Announcement name | 0.1.0 daemons still alive during N's rolling restart (Windows) see no install and may re-seed a 0.1.0 watchdog that "respawns" rolled instances with the old binary (the race `server.rs:196-201` exists to prevent). N daemons spawned by a 0.1.0 installer (Linux) don't see the install either. | Shim S5 |
| Claim fields | 0.1.0 watchdog must parse the claim to yield when another pid owns it (`watchdog.rs:741-750`); unparsable means it overwrites the claim and two watchdogs run | Keep 3 fields; add optional `version` |
| `install.state` phase names / field names | 0.1.0 `wait_for_takeover` cannot see `restarting`, so after 30 s 0.1.0 runs `run_tail` **concurrently** with N (double rolling restart); 0.1.0 `await_handover` cannot read `failed`, waits 600 s and reports timeout | Freeze `phase` vocabulary and field names (permanent rule); new fields optional |
| CLI surface | 0.1.0 runs `N --version` (first digit-led token = version), `N install --continue`, `N <.restore args> --daemon-child` | Keep these and every flag that can appear in a `.restore` |
| config.toml / settings.json | N daemons are started with 0.1.0 configs; a stricter validation means start failure, rollback, `halted` | No new hard validation on existing fields |
| Release assets / npm layout | 0.1.0's downloader picks N's asset by name | Keep names |

### c.3 Pre-existing Linux hazards in the 0.1.0-driven path [hypothesis, settled by T1-E2E and T5b]

0.1.0 does not stop the old watchdog on unix (`flow.rs:286-287` is `#[cfg(windows)]`). That watchdog:
1. Processes death events only at its tick (`v0.1.0` `watchdog.rs:988-997`, default 30 s), and `handle_death` unlinks `run/<port>.sock` and the `/dev/shm` heartbeat file **before** checking the install announcement (`v0.1.0` `watchdog.rs:558-559` vs `567`). The installer spawns the N daemon within about 1 s of the old one exiting, so the unlink very likely hits the **new** daemon's socket: N daemon unreachable by IPC, 0.1.0 verify fails, re-restart, bind conflict, `halted` (service up but uncontrollable; heartbeat writes go to an unlinked inode, so hang detection is lost too). Current code narrows the window (deaths handled immediately) but the unix death poller sleeps 1 s (`watchdog.rs:1226-1234`), so the race remains.
2. Keeps a valid claim after the install, so N daemons never re-seed; its respawns use `current_exe()`, which on Linux reads `/proc/self/exe` of a replaced file (`... (deleted)`), so crash recovery after the upgrade would fail.

Fix (permanent rule **R1**, also in N for all future upgrades): claim carries `version`; a daemon at startup, **before creating its IPC endpoint**, terminates (pid + `created_at_process` verified, `terminate_verified_process`) any live watchdog whose claim has no `version` or an older one, waits for the process to exit, removes the claim; re-seeding happens through the normal self-check once no install is announced. "Newest version wins" avoids flapping in mixed homes. Additionally: no process unlinks another process's socket or heartbeat file (drop `watchdog.rs:558-559`; the binder's connect-probe in b.2 handles stale sockets; a daemon removes its own heartbeat/socket on graceful exit).

### c.4 Windows relay-fallback residue

If N's continuator does not advance `install.state` within 30 s, 0.1.0 verifies via the legacy pipe. Without S1 this ends `failed + halted` although the instances run fine on N. N's `--continue` then refuses halted states (`flow.rs:526-531`) and `install latest` reports "already up to date" (`install.rs:246-257`), leaving a stuck `install.state` that the watchdog re-spawns `--continue` for daily. Fix (permanent): `--continue` treats "every snapshot instance answers with `version == target` and `state == serving`" as success regardless of `failed`/`halted`, then cleans up.

### c.5 Minimal compat surface (one module `src/compat_0_1_0.rs`)

- **S1** (Windows, daemon): also listen on `\.\pipe\aproxy-<port>` as a best-effort second listener (creation failure non-fatal, logged), served by S3 only. Covers the 0.1.0 relay fallback, 0.1.0 CLIs still on PATH (npm/scoop), and prevents 0.1.0 survey from deleting N records. Cross-home reach via this pipe exists only for 0.1.0 clients: no regression vs today.
- **S2** (Windows, client): if the namespaced endpoint is `Unreachable` and this run dir holds a `.pid` for the port, try `\.\pipe\aproxy-<port>`; accept only if peer pid == record pid and reported `process_start` == record. Used by N's continuator and N's CLI against 0.1.0 daemons.
- **S3** (daemon, both platforms): a request **without `v`** gets the 0.1.0 reply shape `{"ok":bool,"info":<0.1.0 InstanceInfo: all 15 fields, proto_version 2, swap_phase = (state == swap_prepared)>,"proto":2}`; ops `ping`, `stats` (= ping), `shutdown`, `prepare_swap`; unknown gives `{"ok":false,"info":null,"proto":2}`. Needed on Linux (0.1.0 drives restart/verify) and behind S1.
- **S4** (client): a reply without `v` is parsed as the 0.1.0 shape and mapped into the v1 status (`swap_phase` to `state`).
- **S5** (announcement): read both the namespaced and the legacy name; N's installer publishes both.

Not shims (permanent rules, zero compat code): `.pid` required set, `.restore` names, claim fields, frozen `install.state` vocabulary, unix socket path, CLI surface, R1.

### c.6 Retirement

0.1.0's `install latest` can jump directly to any later release, and that jump is driven by 0.1.0 code. Therefore keep S1-S5 through the 0.1.x line and delete the module at 0.2.0, with a CHANGELOG "upgrade floor: install a 0.1.x >= N first". A direct 0.1.0 to 0.2.0 jump without the shim fails safe on Linux (verify fails, rollback/`halted`, the running N daemon keeps serving) but leaves a failed state; that is the cost of retirement.

Make retirement one-directional from N onward with **early handover** (permanent change in N's installer): after download and verification, N spawns the **staged target binary** with `install --continue` (both platforms) and only waits and reports. The target binary performs broadcast, swap, restarting, verifying and cleaning. From then on every upgrade needs only *new reads old* compat (new binary parsing old records/state and acting as an old-dialect client), which a release can drop once its floor rises; *old understands new* is needed only for `install.state` (the waiting process reports from it, hence the frozen phase vocabulary). Windows detail: the staged process cannot delete its own image at cleaning; either relay once more to `bin/aproxy.exe` after the swap (existing relay code) or leave the staging directory to the next install's cleanup.

---

## (d) Ordered implementation plan with tests

"Fails today" = expected to fail on current HEAD, i.e. the test proves the change.

1. **0.1.0 compat harness (first, so every later step is checked against it).**
   - Capture real 0.1.0 artifacts from the v0.1.0 binary into `tests/fixtures/v0_1_0/`: `.pid`, `.restore`, `watchdog.claim`, `install.state` (relaying, restarting, failed), request lines, ping/prepare_swap/shutdown replies. Add frozen copies of the 0.1.0 serde structs (`InstanceInfo`, `IpcRequest`, `IpcResponse`, `RestoreRecord`, `WatchdogClaim`, `InstallState`) in a test module.
   - T1a: N parses every 0.1.0 fixture.
   - T1b: the frozen 0.1.0 structs parse everything N writes: `.pid`, `.restore`, claim, `install.state` at every phase N writes, S3 replies; frozen 0.1.0 `IpcRequest` parses `{"v":1,"op":"ping|shutdown|prepare_swap"}` (locks the serde behaviour).
   - T1-E2E (CI, windows + ubuntu): download the v0.1.0 asset, isolated `APROXY_HOME`, copy it to `<home>/bin`, start 2 instances (watchdog running), run `<home>/bin/aproxy install --from <N>`; assert exit 0, all instances report N, `install.state` gone, every `.restore` intact; then `kill -9` one instance and assert respawn on N within the scan period. **Likely fails today on Linux** (c.3); that result settles the hypothesis.
2. **Delete the alpha compat in (a).** Remove the legacy tests (#20); add: a `.pid` without `process_start` is treated as corrupt; `record_identity(pid, 0)` never yields a killable/adoptable identity.
3. **Namespace + startup invariant + identity checks.** `RunDir`/`home_id`; namespaced pipe/heartbeat/announcement; delete env-derived helpers; IPC-before-registry, fatal IPC failure; unix connect-probe before unlink; `sun_path` check; survey responder check; transport-peer check; atomic claim write.
   - T3a (Windows): run dirs A and B; instance in A on P; from B, `stop P` reports no instance and A's instance stays alive; `start` in B on P reports a port conflict, not "already running". **Fails today.**
   - T3b: `endpoint_for_in(A, P) != endpoint_for_in(B, P)` on both platforms. **Fails today on Windows.**
   - T3c: survey with a record whose pid differs from the responder's pid treats the record as stale and does not list the responder as this record's instance. **Fails today.**
   - T3d: a daemon whose endpoint cannot be created (pipe pre-created / socket path held by a live listener) exits non-zero, writes a startup.log line, leaves no `.pid`/`.restore`. **Fails today.**
   - T3e (unix): a second daemon with the same port key does not unlink a live socket. **Fails today.**
   - T3f: an announcement created under run dir A is not active when read under run dir B. **Fails today.**
   - T3g: client rejects a reply whose reported pid differs from the OS peer pid.
4. **v1 envelope + error model + shims S1-S5.**
   - T4a: unknown op returns `error.code == "unknown_op"`. **Fails today** (bare `ok:false`).
   - T4b: `{"v":2,...}` returns `unsupported_version` with `supported:[1]`; oversize/garbage returns `bad_request`.
   - T4c: unknown request/response fields ignored; unknown `state` parses to `Unknown`.
   - T4d: `wait_until_gone` returns false while the endpoint times out (hung server stub) and true only on `Unreachable`. **Fails today.**
   - T4e: a request without `v` gets a reply the frozen 0.1.0 `IpcResponse` parses with correct `info.version` and `swap_phase`; `shutdown` without `v` stops the daemon.
   - T4f (Windows): S2 reaches a fake 0.1.0 responder on `\.\pipe\aproxy-<port>` only when this run dir holds a matching `.pid`; refused otherwise.
   - T4g: server closes an idle connection after the read timeout; client rejects a reply over 1 MiB.
5. **Watchdog: claim `version`, rule R1, no foreign unlinks; N's installer stops the old watchdog on all platforms.**
   - T5a: a live unversioned (or older-version) claim held by an unrelated child process is terminated by daemon startup before its endpoint exists; equal/newer version untouched.
   - T5b (Linux): after `handle_death(port)`, a socket that a different process has bound at `run/<port>.sock` still exists. **Fails today** (`watchdog.rs:559`).
   - T5c: the frozen 0.1.0 `WatchdogClaim` parses N's claim; a 0.1.0-style `refresh_claim` simulation yields when the pid differs.
6. **Install.** Jurisdiction check on the online path before download; `--continue` completes a failed/halted state when the fleet already matches the target; early handover to the staged binary.
   - T6a: online install with an instance whose image is outside `<home>/bin` is rejected before any download. **Fails today.**
   - T6b: N's `continue_install` from the 0.1.0 relaying fixture with fake 0.1.0 instances (S2/S3 stubs) completes and leaves no `install.state`.
   - T6c: a `failed + halted` state whose snapshot instances all answer `version == target`, `state == serving` is cleaned up by `--continue`. **Fails today** (`flow.rs:526-531`).
   - T6d: early handover: the waiting process exits 0 only after the staged binary reaches `cleaning`/`done`; the failure reason propagates via `last_error`.
   - Re-run T1-E2E: must pass on both platforms.
7. **Docs and records.** CHANGELOG (breaking notes, upgrade floor), `docs/architecture.md` (namespace, invariant, wire), skill references (status fields, error codes, isolation), memory entry for the IPC v1 contract and the 0.1.x compat window; move this plan into `.agents/plan/` if the work spans sessions.
