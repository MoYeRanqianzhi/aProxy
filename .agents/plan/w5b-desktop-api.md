# Desktop API — contract for the real Backend

**Ruled application contract.** Baseline `b2eadf6`. This is the local main window's application API, not IPC v1, the CLI JSON schema, or an extension API. Paths and lines below refer to that baseline. Examples of field names from files/CLI are **literals**, not bridge property names. A field derived by the bridge is identified as such; a Rust DTO is never serialized wholesale.

## 1. Calling convention and commands

Use `invoke(name, args)`. In the following TypeScript dictionary each member is `[args, fulfilled value]`, not a positional argument list. An empty object means no arguments; Rust unit fulfills with JSON `null`. For the new Backend commands reject with the **object** `Fault` (§6), not an Error's message or an encoded JSON string. A closed window, disabled ACL or absent Tauri runtime is a transport failure outside this API: show a localized connection failure; never display the raw rejection. All methods are asynchronous to JS. Untrusted invoke input, including unknown or wrongly typed fields and malformed channel descriptors, is decoded at the bridge without forwarding serde/Tauri's value-quoting diagnostics (including failures before a typed handler would run).

```ts
type None = Record<string, never>;
type SafeUint = number; // finite, nonnegative safe integer
// Time = nonnegative safe integer Unix ms. Seconds/durations specifically called out below are not epochs.
type Time = number;
type Session = string; // opaque Rust-issued attachment ID; not an OS or IPC handle
type Token = string;   // opaque Rust-issued lookup/comparison token; never a path or command
type Run = { port: number; instanceId: string }; // dto.rs:8 RunKey.port (u16), .instance_id (server identity, not client text)
type Ref = { run: Run; requestId: string }; // same run; RequestSummary.request_id (server-generated, not parsed by JS)
type Selection = { session: Session; generation: number }; // page attachment; page-issued positive safe integer, increasing per selection
// Channel is @tauri-apps/api/core.Channel, not a window event name.
type Commands = {
  desktop_info: [None, ShellInfo];
  titlebar_insets: [None, Insets];
  set_caption_color: [{color: string|null}, null];
  attach: [{channel: Channel<Frame>}, Session];
  detach: [{session: Session}, null];
  ack: [{session: Session; seq: SafeUint}, null];
  snapshot: [None, Snapshot];
  query_logs: [{run: Run|null; cursor: Token|null; limit: number}, LogPage];
  request_detail: [Selection & {ref: Ref}, Detail];
  body_chunk: [Selection & {ref: Ref; part: BodyPart; offset: SafeUint; limit: number}, Chunk];
  cancel_detail: [Selection, null];
  rescan: [None, null];
  list_configs: [None, Choice[]];
  start_instance: [{target: StartTarget}, Run];
  stop_instance: [{run: Run}, null];
  restart_instance: [{run: Run}, Run];
  force_stop_instance: [{run: Run}, null];
  restore_instance: [{port: number; expectedRestore: Token}, Run];
  forget_restore: [{port: number; expectedRestore: Token}, null];
  read_config: [{target: ConfigTarget}, ConfigView];
  validate_config: [{target: ConfigTarget; changes: Edit[]}, Validation];
  write_config: [{target: ConfigTarget; changes: Edit[]; acknowledgeExternalAcl: boolean}, Saved];
  read_settings: [None, SettingsView];
  set_watchdog: [{enabled: boolean}, boolean];
  read_desktop_state: [None, DesktopState];
  set_preferences: [{changes: Partial<Preferences>}, Preferences];
  set_pinned: [{ref: Ref; pinned: boolean}, Bookmark[]];
  read_autostart: [None, Registration];
  set_autostart: [{enabled: boolean}, Registration];
  check_update: [{channel: 'auto'|'pre'}, UpdateInfo];
  install_update: [{check: Token}, InstallEnd];
  about: [None, About];
};
```

The table gives per-command **additional** failures (`D_` in the table stands for `desktop_`; see details in §6). All new Backend commands have `D_bad_input` for malformed inputs and `D_unavailable` for an unavailable local service/unexpected failure. R = no product mutation, W = mutation, S = window/session state only. Unless stated otherwise progress is the pending promise (indeterminate), completion is its settlement, and there is **no cancel**: navigating away can ignore the answer but must not undo side effects or retry a write automatically. Disk/OS/network/IPC waits never run on Tauri's event-loop thread.

|Invoke|Kind, verification and behavior|Additional errors|
|---|---|---|
|`desktop_info`|R, immediate. Existing shell capability state; no application failure.|—|
|`titlebar_insets`|R, immediate. Only call on Windows with custom titlebar; read the invoking window's metrics.|Legacy native string (below)|
|`set_caption_color`|S, immediate, same window restriction; the caller sends `#rrggbb` or null (reset).|Legacy native string (below)|
|`attach`|S. Replace the one listener on this window, invalidate its predecessor's selections. First frame establishes the baseline; see §4.|—|
|`detach`|S. Cancel listener and detail only for the matching attachment; stale detach is a successful no-op. Not window shutdown.|—|
|`ack`|S. Release one frame only when session and sequence match; otherwise no-op. No disk or IPC.|—|
|`snapshot`|R. Return current home membership, recent first screen, watchdog and current in-memory update job. Memory only: no IPC, file read, subscription or body fetch.|—|
|`query_logs`|R. Latest or preceding summaries in the whole home (`run:null`) or exactly one run; first page is memory only in both scopes; only cursor continuations may issue bounded verified reads. Fresh query supersedes that scope's cursor. See §2.|D_cursor, D_changed, D_history|
|`request_detail`|R/S. Open the current detail selection and fetch **metadata only**, including for bookmarks. Verify live run, read capability, redaction feature and request identity, not just port.|D_changed, D_gone, D_history, D_cancelled|
|`body_chunk`|R. One bounded read for the current detail; verify attachment/generation/ref and body capability, part, attempt and bounds. Cancel via `cancel_detail` or selecting another request.|D_changed, D_gone, D_history, D_cancelled, D_busy|
|`cancel_detail`|S. Invalidate through the specified generation and drop pending reads, not just their results. Idempotent; old sessions cannot cancel a successor.|—|
|`rescan`|R/S. Wake the **existing** view's discovery driver (coalesce repeated wakes); resolves on scheduling, not on successful connection. Watch normal channel invalidations. No reconnect-by-port or second subscription.|—|
|`list_configs`|R, disk scan. Enumerate discovered TOMLs, configured aliases including outside scan directories, and the default target. No browsing.|D_config|
|`start_instance`|W, can wait for readiness. Re-resolve the offered choice inside this home, run the CLI's start preflight, launch with the running executable; return the verified run actually ready. Default start works without a file if defaults suffice; otherwise see first use below.|D_target, D_changed, D_config, D_port, D_start|
|`stop_instance`|W, waits for a graceful stop. Resolve by port, require same live identity at send; apply CLI stop intent (remove desire to run), never escalate.|D_changed, D_unresponsive, D_stop|
|`restart_instance`|W, precheck then graceful stop with restart intent then launch. Require same live run and usable restore spec; success may be on a different port.|D_changed, D_unresponsive, D_no_spec, D_config, D_port, D_stop, D_start|
|`force_stop_instance`|W. Re-enumerate and match the exact run; require **unresponsive**; verify recorded PID + process creation marker at termination, not a page PID. A responsive, incompatible or unverifiable target is refused; no fallback to force on normal stop.|D_changed, D_force_refused, D_stop|
|`restore_instance`|W. Resolve only that port's pending desired record, compare its observed token, recheck absence/ownership and call restore. Never expose private spec or fan out. Concurrent recovery is not authority to stop/replace it; refresh if already replaced.|D_changed, D_target, D_config, D_port, D_start|
|`forget_restore`|W. Resolve only this port’s pending desire in the supplied home. If absent reject `desktop_target`; if the observed token differs reject `desktop_changed`. Otherwise remove only that desire, never stop a process.|D_changed, D_target|
|`read_config`|R. Resolve allowed target afresh; resolve effective file layers exactly as the CLI (not a dump of live process memory). No-file runs have no editable target.|D_target, D_changed, D_config|
|`validate_config`|R. Validate typed patch against the on-disk target/defaults in memory. Return issues, including incompatible/occupied listener observation; never spawn a format or reserve an address. Validation is not a promise a later start will succeed.|D_target, D_changed, D_config|
|`write_config`|W, can wait for the common config lock. Re-resolve and revalidate as part of locked read/modify/write, change only supplied keys and return affected ports. No implicit restart.|D_target, D_changed, D_config, D_port, D_conflict, D_external_acl|
|`read_settings`|R. Read this home's current validated settings; no last-good substitution for a broken file. Return only the typed projection.|D_config|
|`set_watchdog`|W. Set only `watchdog.enabled` through settings edit; result is stored switch, not a claim a watchdog process is running.|D_config, D_conflict|
|`read_desktop_state`|R normally; repair of corrupt private desktop file is W. Reload bookmark/preference projection. Report reset rather than silently losing state.|D_state_io|
|`set_preferences`|W. Validate supplied preference keys, merge under lock and return effective preferences. Never writes settings.json or loads a theme/mascot program.|D_state_io, D_state_reset, D_limit|
|`set_pinned`|W. Pin resolves the ref to a currently verified, redacted record (cache or metadata read); unpin compares stored identity only and also works offline. No client-provided summary. Return replacement list.|D_state_io, D_state_reset, D_limit, D_history, D_gone, D_changed|
|`read_autostart`|R. Inspect this home's current-user OS registration and installed executable every time. No preferences fallback.|D_registration|
|`set_autostart`|W. Set/remove just this home's fixed registration, reread and verify. Missing on an unset is success. A foreign command in its slot isn't overwritten/deleted silently.|D_registration|
|`check_update`|R, may wait for downloads metadata; no percentage or cancel. Run only installed home executable, parse its check JSON, issue a confirmation token only for an available candidate.|D_binary, D_busy, D_install_protocol, install codes|
|`install_update`|W, long. Confirm last check token; install exactly the offered version, one job per window, observe `update-progress` and terminal promise. No cancel/force/shell or changing variant from JS.|D_changed, D_binary, D_busy, D_install_protocol, install codes|
|`about`|R. Use calling binary's build metadata and injected home (display only), not an online check or the installed executable's version.|—|

The two Windows-only caption commands predate `Backend`. Their existing `Result<_, String>` rejects with a native HWND/parse failure, not with a `Fault` (`aproxy-desktop/src/titlebar.rs:101,126`). Keep the released success **and rejection wire shapes**; do not route any configuration, records or values to them. The shell adapter maps that legacy rejection to `{code:"desktop_window",details:{}}` for display; it does not log/display OS prose. This is the one pre-existing exception to structured application rejections.

No command to open a project URL: remove the clickable homepage, retain About. No generic settings setter, config path, program, alias mutation, log-file reader or extension command. W5e may wrap host operations and check grants in Rust before writes; this page cannot assert an extension identity or grant consent on its behalf.

## 2. Read model, paging and detail

Field attribution convention: **D** below is `aproxy-ipc/src/view/dto.rs:8`–378. Each declaration names its source struct and first line; unless a comment says otherwise, a field is that struct's same-named snake_case field. All strings marked **H** are untrusted HTTP/client/upstream/format-derived **display text**, already redacted at the instance (headers and methods `src/history.rs:1124`, request shape `src/history.rs:1177`, body reader `src/history.rs:1474`, reports `src/history.rs:1929`). **L** = local config/record/file/operator text, projected in Rust under §5. **I** = program discriminator, number, key or identifier, not client content. These classifications apply to every use, including arrays, error details, events and bookmarks. Opaque IDs are for comparison, not authority. Never render returned text as HTML. Nulls below are explicit (not omitted); array absence is `[]`. Unknown source enum becomes `unknown` rather than peer text. All counts, byte lengths, offsets, durations, sequences and revisions are JSON numbers. Rust saturates at 2^53 − 1; none can realistically reach it. Reject inputs outside the nonnegative safe integers. The only integer-as-string is `Cell.integer.decimal` for exact file values. Wall clock values must be safe integers (invalid timestamps are not coerced to approximate dates).

```ts
import type { Channel } from '@tauri-apps/api/core';
type Snapshot = {
  instances: Card[]; // HomeView.instances(), ascending port; missing means retired
  recent: Row[]; // newest 50 available view rows, same merge order as first log page; no remote fetch
  watchdog: Watchdog|null; // enumeration.watchdog; null = no verified living owner/record
  update: Job|null; // window coordinator; null until an install is accepted; survives JS reload only
};
type Card = { // InstanceSummary D:39 plus explicitly marked local projections
  port: number; // discovered listener/restore port, also present without identity
  run: Run|null; // recorded run or null if none trustworthy
  pid: number|null; // record PID, null without record; display only
  version: string|null; // L record.version, null absent
  startedAt: Time|null; // record.started_at, null absent
  liveness: 'live'|'unresponsive'|'incompatible'|'down'|'unknown'; // Presence, view/mod.rs:48
  restorePending: boolean; // restore_pending, independent of liveness
  restoreReason: 'config-missing'|'unknown'|null; // I restore_reason, null when no reason supplied
  expectedRestore: Token|null; // bridge comparison handle for observed down/pending desired record; null otherwise
  feed: 'offline'|'joining'|'streaming'|'polling'|'disconnected'; // Feed, view/mod.rs:65
  historyUnavailable: 'source-redaction-missing'|'unsupported'|null; // I policy: see below
  reads: {historyFeed:boolean; detail:boolean; body:boolean}; // D:27 op bits gated by source redaction
  state: 'starting'|'serving'|'swap-prepared'|'stopping'|'unknown'|null; // D:69 state.state, null without verified snapshot
  activity: Activity|null; // D:69 state.activity, same null condition
  metadata: CardMeta|null; // proposed allowlisted join by same verified run; null absent/legacy/unverified
};
type CardMeta = { // Enumerated at aproxy-ipc/src/enumerate.rs:52; InstanceRecord and Behavior at aproxy-ipc/src/wire.rs:12,72
  listen: string; // L instance.listen_addr
  baseUrl: string; // L instance.base_url, URL redacted at registration AND display boundary
  configPath: string|null; // L instance.config_path, null when no file
  aliases: string[]; // L settings.aliases names for the actual config; Rust path-identity match before redaction (src/find.rs:29); [] without a match
  delivery: 'forward'|'retry'|'unknown'; // I status.behavior.delivery, not config-on-disk
  diskCache: boolean; // status.behavior.disk_cache
  keepaliveIntervalSecs: SafeUint; // status.behavior.keepalive.interval_secs, seconds (0 = off)
  keepaliveTrigger: Trigger; // I status.behavior.keepalive.trigger
  retryRuleCount: number; // status.behavior.retry_rules, u32, not rule contents
};
type Trigger = 'any'|'accept'|'body-stream'|'unknown';
type Activity = { // RunActivity D:95; all counts are THIS run only
  lastRequestAt: Time; // last_request_at, at least startup even before first request (src/daemon.rs:513)
  requestsTotal: SafeUint; retriesTotal: SafeUint; // received requests; attempts after first
  rescuedTotal: SafeUint; upgradedTotal: SafeUint; // successes after retry; forward->retry conversions
  inFlight: SafeUint; retrying: SafeUint; // unfinished deliveries; currently retrying
  byCode: Partial<Record<HistoryCode,SafeUint>>; // I by_code, counts by bounded known code; sum unknown into 'unknown'
  lastError: FailureAt|null; // D:127 last_error, null if none or no source redaction guarantee
  buckets: Buckets; // D:143, oldest first; empty arrays mean no series advertised
};
type Buckets = {
  widthSecs: SafeUint; // D:143 width_secs, seconds per bucket
  requests: SafeUint[]; retries: SafeUint[]; failures: SafeUint[]; // D:143 counts of receipts, retry attempts, failed attempts (NOT rescues)
};
type FailureAt = {code: HistoryCode; at: Time; status: number|null}; // D:127 code (I), at, details.status if u16, else null. No raw error object/message.
type Watchdog = { // aproxy-ipc/src/enumerate.rs:126,395; not currently a HomeView field
  pid: number; // watchdog.pid
  state: 'running'|'stale'|'unknown'; // I freshness, not the enabled setting
  lastScanAt: Time; // watchdog.last_scan_at
  restarts: {port:number; at:Time; kind:'crash'|'hang'|'unknown'; ok:boolean}[]; // registry.rs:519 listen port, completion time, I trigger, actual success
};
type Phase = 'reading-body'|'transforming'|'attempting'|'backoff'|'replaying'|'forwarding'|'done'|'unknown'; // I src/history.rs:58
// Rescued is a badge for ok after a retry, NOT an outcome; streaming is not an outcome either.
type Outcome = 'in-flight'|'ok'|'upstream-error'|'aproxy-error'|'format-response'|'client-gone'|'unknown'; // I src/history.rs:91
type Row = { // D:157 RequestSummary, plus the run namespace
  ref: Ref; // I enclosing run + request_id
  seq: SafeUint; revision: SafeUint; // request_seq, mutation revision (not channel seq)
  startedAt: Time; endedAt: Time|null; // received; ended (null in flight)
  method: string; path: string; // H not limited to POST/GET; URL path/query masked
  model: string|null; userAgent: string|null; // H recognized request model; User-Agent; null if absent
  stream: boolean; // client streaming request flag
  delivery: 'forward'|'retry'|'unknown'; upgraded: boolean; // I mode; whether it switched
  outcome: Outcome; code: HistoryCode|null; // I outcome and terminal code (null if none)
  clientStatus: number|null; // delivered HTTP status, null if not sent
  phase: Phase; nextAttemptAt: Time|null; // I phase; scheduled retry time or null
  attempts: number; triesOmitted: number; // total u32 attempts; omitted count (do not use tries.length as total)
  tries: Try[]; // D:218 retained summaries in execution order
  keepalive: boolean; heartbeats: SafeUint; // active keepalive; number sent
  requestBytes: SafeUint; responseBytes: SafeUint; // received client bytes; delivered bytes
  reports: Report[]; truncated: boolean; // D:255 whole-request reports; source shortened row
};
type Try = { // AttemptSummary D:218
  attempt: number; startedAt: Time; // one-based u32 number; start time
  durationMs: SafeUint|null; upstreamStatus: number|null; // elapsed ms when finished; HTTP status when received
  code: HistoryCode|null; responseBytes: SafeUint; // I failure code if failed; bytes received
  decision: Decision|null; reports: Report[]; // recorded retry decision or null; per-attempt reports
};
type Decision = { // D:241
  by: string; // H source-redacted rule/format identifier
  decision:'retry'|'stop'|'unknown'; // I source action
  delayMs:SafeUint|null; reroute:boolean|null; // chosen delay in ms if supplied; optional rerouting decision
};
type Report = { // D:255
  format:string; stage:'request'|'retry'|'decide'|'response'|'stream'|'heartbeat'|'notify'|'unknown'; summary:string; // H format name; I stage (aproxy-ipc/src/config_file.rs:139) or 'unknown'; H source-redacted program summary (diagnostic data, not a Rust UI sentence)
  actions: Action[]; // allowlisted structured projection, no raw JSON. All non-enum strings here H, src/history.rs:1929
};
type Action = ({ // aproxy-envelope/src/reports.rs:109,161; label H optional display label or null
  type:'protocol-convert'; from:string; to:string // H source and destination protocol
}|{type:'key-select'; keyId:string // H public key alias, never credential
}|{type:'route'; channel:string; modelFrom:string|null; modelTo:string|null // H channel and optional model names
}|{type:'header-edit'; name:string; op:'set'|'remove' // H header name, I operation, never its value
}|{type:'body-edit'; path:string; op:'set'|'remove' // H JSON pointer, I operation
}|{type:'other'}) & {label:string|null}; // unknown/custom action drops all other fields

type Detail = { // RequestDetail D:269; no body bytes and no fabricated raw transcript
  record:Row; request:Message; tries:TryDetail[]; // current row, client metadata, retained attempts
};
type Message = { // MessageDetail D:307
  headers:Header[]; body:BodyInfo|null; // ordered incl duplicates; null when not recorded
};
type Header = {name:string; value:string|{secret:true}}; // D:343 H header name; H non-secret text or secret marker (omit hint). Classify original name before scrubbing.
type TryDetail = { // AttemptDetail D:281
  attempt:number; // one-based; match summary by number, not array position
  sent:(Message & {method:string; url:string})|null; // SentDetail D:317 H method, masked URL, headers, body; null if not sent
  response:(Message & {upstreamStatus:number})|null; // ResponseDetail D:331 observed HTTP status, headers, body; null if no response
  error:{code:HistoryCode; message:string}|null; // AttemptError D:295 I failure code; H source-redacted diagnostic, null if none; no untyped details
};
type BodyInfo = { // D:353; source words in src/history.rs:119,228
  bytes:SafeUint; // retained decoded byte count (not necessarily original wire length)
  state:'retained'|'same-as-previous'|'evicted'|'not-retained'|'unknown'; // I body retention
  cause:'budget-disabled'|'exceeds-budget'|'decode-failed'|'unknown'|null; // I loss reason or null
  encoding:string|null; // H source-redacted original content encoding if recorded
};
type BodyPart = {kind:'request'}|{kind:'sent'|'response'; attempt:number}; // page choice; one-based retained attempt, wire.rs:733
// BodyChunk D:367; base64 is transport, NOT redaction. Offset advances by decoded byte length.
type Chunk = {totalBytes:SafeUint; offset:SafeUint; dataB64:string; more:boolean; complete:boolean}; // current size; echoed byte offset; H instance-redacted bytes; more now; writer finished

type LogPage = {
  entries: Row[]; // verified projected rows, newest first as specified below
  next: Token|null; // bridge cursor or null if no more accessible rows; not a request ID
  missing: {run:Run; reason:'changed'|'unavailable'|'disconnected'}[]; // incomplete runs in this traversal, not empty successes
};
```

**Card rules.** `live` means the peer answered and was verified; show lifecycle and activity then (idle is a presentation, not liveness). `unresponsive` is an existing process that did not answer; only here may the UI offer force-stop with confirmation. `incompatible`/`unknown` are not dead and never eligible for force. A `down` + `restorePending` row is “waiting for restore”, optionally “config missing”; forgetting the pending desire removes the row, not a saved profile. `feed` and policy are separate badges: `joining`, `streaming`, `polling` (busy or no usable stream), `disconnected` (last good cache), `offline`. Polling is not evidence of a hang. If the verified peer lacks `redacted_client_fields`, show `historyUnavailable:'source-redaction-missing'` first, keep identity/counters/state but no rows, metadata-derived diagnostics, `lastError`, detail or bodies. Otherwise `unsupported` denotes live but no feed; still honor independent detail/body bits. Non-live reads are disabled, null restriction means no policy result, not a guarantee of history. See `aproxy-ipc/src/view/mod.rs:141,582,706`. `CardMeta`, restore precondition and watchdog are **additional** projections, not fields promised by `HomeView.instances()` today: project metadata at membership discovery, join on actual run, and reuse the watchdog record reader (`aproxy-ipc/src/enumerate.rs:395`) on the same discovery clock. Never expose full records/restore specs or open another event stream.

**Paging.** `HomeView.requests(run)` (`aproxy-ipc/src/view/mod.rs:571`) gives a recent window: 256/run, 1024/home, with active rows preferred; not an archive end marker. `query_logs` should also reach older records still held by a live redaction-capable instance: add an identity-checked page read to the view using `ReadOp::Requests {before,limit}` (`aproxy-ipc/src/wire.rs:377`, `src/history.rs:1398`), not another subscription. `limit` = 1..100. A null cursor reads only cached rows, with no IPC even on an empty cache; return a continuation for any eligible live run whose archive tail is unknown. It starts a traversal with the currently eligible runs (including disconnected runs' safe cached rows; mark those incomplete); exclude policy-blocked data. For an explicitly selected run reject replaced identity or policy denial rather than silently omitting it. Merge by descending **numeric seq within each run**. Among current per-run heads choose largest `startedAt`, tie by ascending port then binary UTF-8 instance ID. This defines the order even when a clock goes backwards; it is not sorting wall clock across all historical records. Dedup by Ref; terminal precedes nonterminal when choosing the better copy, then larger revision. Freeze each run's starting upper sequence and advance exclusive per-run positions; later arrivals appear only on refresh. Cursor state is per-window, opaque, single-use and bounded to one all-runs and one selected-run traversal; a new first page replaces the respective traversal. Reject expired/wrong-scope/replayed cursor with `desktop_cursor`. When a peer is lost mid-traversal, return available rows and `missing`, never substitute a successor. Do not materialize all history or persist pages; UI virtualizes/discards pages. A pin can outlive both page cursor and view cache.

**Detail lifetime.** On attach generation starts at 1 in JS. Every selection, even reopening the same row, increments it. Register `request_detail`'s generation **before** the first await, retire older work, reject an old/retired generation. `cancel_detail` advances a tombstone (`max(retired,generation)`); a cancel arriving before an older open is effective, an old cancel cannot close a newer detail. Permit `body_chunk` only after matching detail has opened, one in flight per selection, `limit` = 1..1,048,576 raw bytes (`aproxy-ipc/src/wire.rs:722`); safe integer offsets only. Race the view future against invalidation and drop it on cancel (`aproxy-ipc/src/view/mod.rs:607,631`); discard any late result in JS too. `more:false,complete:false` means no more **now**; only read again on user request. Close, switch, detach, reload and window destruction revoke outstanding reads. Never persist, preload, log or cache detail/body on either side; display bounded chunks only. A retired run or evicted record behind a bookmark is “no longer in instance history”, not a request to read a log file.

## 3. Targets, config, settings, bookmarks, update and shell DTOs

```ts
type StartTarget = {kind:'default'|'config'|'alias'; id:Token}; // page returns Rust choice ID; no path, PID, arbitrary alias string or options
// For file-backed instances resolve record.config_path, never infer it from a port or display path.
type ConfigTarget = StartTarget|{kind:'run'; run:Run; expectedFile?:Token}; // read without expectedFile; validate/write echo the fileToken from that read
type Choice = {
  target: StartTarget; // bridge handle scoped to window + current resolved membership
  label: string; // L alias name, displayed config filename or 'default' (localize kind, not the file)
  path: string; // L display path, not authority; src/find.rs:9 and src/settings/mod.rs:265
  exists: boolean; // actual regular-file check, not success resolving an empty config layer
  isDefault: boolean; // matches actual settings.default_config or injected home/config.toml fallback
};
type Source = 'override'|'toml'|'settings'|'env'|'default'; // src/config/resolve.rs:46; 'env' says environment may select proxy, not its credentials
// Preference/config/diagnostic strings hereafter are L unless noted. Config key *values* are CLI/file spellings.
type TextKey = 'base_url'|'listen_addr'|'api_key'|'proxy'|'proxy_username'|'proxy_password'|'keepalive.heartbeat';
type SecondsKey = 'timeouts.connect_secs'|'timeouts.first_byte_secs'|'timeouts.read_idle_secs'|'keepalive.interval_secs';
type HeaderTable = 'extra_headers'|'override_headers';
type Edit =
  | {op:'set'; key:TextKey; value:string} // new text; send a full URL or new credential only on explicit replacement
  | {op:'set'; key:SecondsKey; value:SafeUint} // reject negative, non-finite, fractional or unsafe seconds
  | {op:'set'; key:'keepalive.trigger'; value:Exclude<Trigger,'unknown'>} // body-stream -> file's body_stream
  | {op:'unset'; key:TextKey|SecondsKey|'keepalive.trigger'} // remove file key; defaults may become visible
  | {op:'set-header'; key:HeaderTable; name:string; value:string} // one exact header member, fresh value
  | {op:'unset-header'; key:HeaderTable; name:string}; // one local member only
// Read-only display tree: distinguishes a large integer from text and a credential from a literal mask.
// Names in map entries are data (file keys, header/env names), NOT bridge object properties.
type Cell = {kind:'none'}|{kind:'secret'; set:boolean}|{kind:'text'; text:string}|{kind:'bool'; value:boolean}
  | {kind:'integer'; decimal:string}|{kind:'list'; items:Cell[]}|{kind:'map'; entries:{name:string; value:Cell}[]};
type ConfigField = {
  key:string; // I known dotted CLI spelling, or L quoted per-header spelling; no synthetic free-form write key
  value:Cell; // effective projected value; 'none' = absent; fixed secret scalars use {kind:'secret',set:false} when absent
  source:Source|null; // Resolved.sources[key], null when no source; header-member sources separate
  header:{table:HeaderTable; name:string}|null; // I table + L display name for header-member rows only, null otherwise; use for Edit only if editable
  editable:boolean; // exact allowlist above; false for redacted/unaddressable header names and all other keys
};
type Issue = { // projected Diagnostic, aproxy-ipc/src/diag.rs:23; messages/details quoting source values are NOT passed through
  code:DiagCode; severity:'error'|'warning'|'info'; // I known code + Diagnostic.severity
  key:string|null; file:string; // L Diagnostic.key or null if file-level; L Diagnostic.file
  line:number|null; column:number|null; // 1-based line and character column; null if not known
};
type ConfigView = {
  fileToken:Token|null; // window-private run/file binding, null for a choice
  file:string; // L resolved file path (may be absent on disk for first-use default)
  fields:ConfigField[]; // one row per scalar/array and header member (see enumeration below)
  withheld:{key:string; from:'settings'; reason:'base-url-differs'|'proxy-differs'; defaultTarget:string; target:string}[];
  // L key/URL display strings, I from/reason; src/config/resolve.rs:78; empty target means no effective target
  diagnostics:Issue[]; // Resolved.diagnostics incl warnings and errors, not parser snippets
  externalAcl:boolean; // Windows: canonical destination outside canonical home, before atomic replacement
};
type Validation = {
  canSave:boolean; // same new-error test as write, plus listener conflict; not the absence of all pre-existing issues
  diagnostics:Issue[]; // projected candidate errors/warnings, no values echoed
  occupiedPort:number|null; // conflicting different run/known bind conflict, null if none known
  externalAcl:boolean; // same Windows warning as ConfigView
};
type Saved = {changed:boolean; instancesToRestart:number[]}; // Applied.changed; sorted unique responsive + unresponsive ports using file, [] for no-op

type SettingsView = {
  watchdogEnabled:boolean; // settings.watchdog.enabled, writable only through set_watchdog
  readOnly:{key:string; value:Cell}[]; // I declared dotted settings paths below; L projected effective values
  diagnostics:Issue[]; // valid file may still have informational/warning diagnostics
};
type Preferences = { // private <home>/desktop/preferences.json; defaults from ui/src/lib/backend/mock/index.ts:108
  lang:string; // system or BCP-47-shaped tag, at most 35 ASCII characters
  mode:'system'|'light'|'dark'; // color-scheme choice
  palette:'soft'|'standard'|'vivid'; // builtin palette intensity
  mica:boolean; // requested effect, not OS support
  mascot:string; // [a-z0-9-]{1,64}; the page resolves IDs
  theme:string|null; // L canonical aproxy-theme-v1: JSON string, null = builtin, parse via ui/src/lib/theme.ts:304
};
type Bookmark = { // private <home>/desktop/bookmarks.json; immutable summary captured on pin, not a second log
  ref:Ref; // namespace + request ID (never retarget by port)
  startedAt:Time; method:string; path:string; // Row received time, H method, H masked path
  model:string|null; // Row.model H or null, no headers/body/reports
  outcome:Outcome; attempts:number; // observed Row.outcome and count AT pin (not guaranteed final)
};
type DesktopState = {
  preferences:Preferences; bookmarks:Bookmark[]; // validated files; newest pin first
  reset:('preferences'|'bookmarks')[]; // I files replaced with defaults in this read; show recovery notice
};
type Registration = { // computed every call, no cached boolean
  enabled:boolean; // true only if exact OS item names expected command/home and installed binary exists
  issue:'binary-missing'|'entry-mismatch'|null; // I observed inconsistency; absent entry alone = disabled/null
};
type About = {version:string; variant:'desktop'|'headless'; home:string}; // I version/variant of running binary (src/lib.rs:118); L display-only injected absolute home

type UpdateInfo = { // src/commands/install.rs:341,398–412; no release notes or actionable URL
  channel:'stable'|'pre'; // I actual child channel, not necessarily 'stable' for 'auto'
  current:string; latest:string|null; // I verified semvers from child, latest null if none in channel
  updateAvailable:boolean; variant:'desktop'|'headless'; // actual update_available and child variant
  publishedAt:Time|null; checkedAt:Time; // optional valid published_at RFC3339 -> ms, null if absent/unparseable; bridge clock at successful check
  check:Token|null; // comparison with last successful eligible check, null unless updateAvailable and latest exists
};
type InstallPhase = 'marking'|'downloading'|'downloaded'|'broadcasting'|'acked'|'swapping'|'swapped'|'restarting'|'verifying'|'cleaning'|'done'|'failed'|'aborted'|'unknown'; // src/install/state.rs:31
// Job is a replacement, even when a child line describes an increment. No pretend percentage.
type Job = {
  id:Token; requestedVersion:string; // Rust job identity; I checked candidate semver, NOT proof installed
  targets:number[]|null; // plan.ports; null until plan (distinct from no instances)
  phase:InstallPhase|null; phaseAt:Time|null; // last phase.phase/at; both null before first phase
  download:{bytes:SafeUint;total:SafeUint|null}|null; // latest progress.bytes/total, null until observed; total null when unknown
  instances:{port:number;outcome:'restarted'|'rolled-back'|'failed'|'unknown'}[]; // one outcome per observed instance
  end:InstallEnd|null; fault:Fault|null; // null while pending; exactly one populated AFTER child result and exit processed
};
type InstallEnd = {outcome:'installed'|'up-to-date'; version:string|null}; // result.outcome / optional verified result.version, not guessed from request

type ShellInfo = {customTitlebar:boolean; mica:'available'|'unsupported'|'transparency-off'; windowsBuild:number|null}; // DesktopInfo aproxy-desktop/src/lib.rs:233, custom titlebar installed, I Mica availability, OS build or null off Windows
// Geometry in logical CSS px from invoking HWND, aproxy-desktop/src/titlebar.rs:91. No IO or page geometry supplied.
type Insets = {top:number; height:number; right:number}; // hidden top rows; visible caption height; reserved button width
```

**Selection and mutations.** Resolve `default` via `settings::default_config_path_for` (`src/settings/mod.rs:265`); union `find::discover_in` (`src/find.rs:28`) with settings.aliases, which may name files outside the scan. One config choice per unique discovered file, one alias choice per registered alias, one default choice (even for missing default). There is no accumulating token table. One new random nonce supersedes a previous choice listing; Rust recomputes candidate IDs from that nonce and fresh canonical targets. A per-window keyed restore token compares the current private record without retaining past records. A new check supersedes the single outstanding update offer. All IDs (including default) stand for a Rust-side lookup including observed canonical target; compare fresh resolution per act. Aliases are never fed to CLI's arbitrary-path fallback (`src/ops/target.rs:14`). A `run` target compares both port and ID against this home's verified record and uses its real config path; no file => `desktop_target`. Treat a retargeted alias/symlink or replaced run as `desktop_changed`, including when waiting for the write lock. A missing **implicit** default is valid for first use: open default form, set base URL (and desired listener/key), save to the fixed home config, then start default. A missing explicitly configured default or alias is reported, not silently recreated elsewhere. Start never creates a stock preset or persists a stopped instance. A CLI preflight same-listener/ready result is idempotent; it is not authorization to restart the incumbent (`src/ops/start.rs:214,268,298`).

For stop/restart, resolve fresh snapshot/record by port, compare Run and recorded PID/start/verified peer, and bind the **actual control connection before writing the destructive frame**. Ping followed by a new unverified connection is insufficient (`src/ops/stop.rs:80`, `aproxy-ipc/src/client.rs:149,173,217,543,648`). Hold the connected endpoint, compare transport PID/owner and recheck instance/start identity on that peer before sending; refuse if unable to prove it. Don't add a wire argument older peers interpret differently. For force recheck that a ping is still unresponsive immediately before the verified native termination, preserving the CLI's intent/restore handling (`src/process.rs:29`, `src/ops/stop.rs:61`). `restore_instance` takes the stored desire's comparison token (issued only for this observed port and a readable record, compare against current private record without exporting a digest of secrets); use `src/spec.rs:144` and `src/ops/restore.rs:15`. A replacement desire is not the one the page saw. Claimed IDs, tokens and display text are never passed as filenames, PIDs or executable selections.

**File form.** `src/commands/config.rs:97,109` resolves the selected file with empty CLI overrides; use the same sources, withheld credentials, diagnostics and `config::view::effective_value` (`src/config/view.rs:66`). Label it “effective file configuration on next start”, **not** the running process's private launch overrides. Source `override` is in the source vocabulary but not fabricated here. Use the output below for a valid file; if the file parses with errors, keep diagnostics, emit only validated projected fields, never raw malformed input. Header members have individually quoted key paths per `src/config/resolve.rs:604`. Include the scalar rows even for an unset optional key; header table rows themselves are read-only summaries (maps); editable member rows carry `header.table/name` (never parse/install a displayed TOML key); use the exact original name if safe, otherwise display only. Editing a header changes one file member, not the effective merged table. Removing an inherited value does not remove the global default. `Cell.secret` exposes set/not-set only. `Cell.integer.decimal` is the exact base-10 file integer; `Cell.none` represents null/unset. Lists preserve order, map entries sorted by original name; per-member sources are never inferred from the parent. `Cell.text` for an enum spelled in a config/settings **document** retains its file spelling, e.g. `body_stream`; edit enum fields use `body-stream`. For map/list read-only trees keep only recognized schema fields (absent optional members omitted), never unknown input verbatim.

|Config keys / source|Form and CLI spelling|
|---|---|
|`base_url`, `listen_addr`, `api_key`, `proxy`, `proxy_username`, `proxy_password`, `extra_headers`, `override_headers` (`src/config/view.rs:87`)|Editable as `Edit` above; URL displays masked, credentials and **all configured header values** write-only. Never send display mask back; no edit means no operation. CLI member example: `aproxy config set 'extra_headers."x.team"' '<new value>'`.|
|`timeouts.connect_secs`, `timeouts.first_byte_secs`, `timeouts.read_idle_secs`, `keepalive.interval_secs`, `keepalive.trigger`, `keepalive.heartbeat` (`src/config/view.rs:117`)|Editable scalars. Only set/unset changed keys. Seconds are unsigned integers; preserve heartbeat text as text.|
|`max_request_mb`, `max_response_mb`, `disk_cache`, `delivery`, `allowed_hosts`, `allowed_origins`, `log_file` (`src/config/view.rs:98`)|Read-only. Show `aproxy config set KEY VALUE` replacing KEY with exactly the displayed key. Limits are MiB; lists use a JSON array, booleans true/false.|
|`retry.max_backoff_secs`, `retry.rules`, `history.body_budget_mb`, `format` (`src/config/view.rs:134`, `aproxy-ipc/src/config_file.rs:187,215`)|Read-only. Same command with literal dotted key. For rules show `aproxy config set retry.rules '<JSON array>'` (path/status/max_attempts); for programs `aproxy config set format '<JSON array>'` (known format keys only, env and secret args masked, no format state). Replace whole arrays, not indexed writes.|

For a non-default target, the CLI display template is `aproxy config --config <alias-or-path> set <key> <value>` (`src/cli.rs:508`); show as tokens/placeholders, **never** run an interpolated command, copy credentials or suggest a redacted value as a replacement. Reject duplicate edits or overlapping header members case-insensitively, invalid header names/values and marker-shaped replacements. Bound a patch to 128 edits and 64 KiB serialized UTF-8. Dry run `config::edit::apply_to_text`, re-resolve candidate and compare existing/new errors like `src/config/edit.rs:159`; use the actual file validation rules (`src/config/mod.rs:369,540`), not the prototype's promise to detect every route pointing back to the proxy. No DNS or network probes. A changed listen port occupied by a different home run is a conflict; ignore the target's own unchanged listener. Do not block incremental repairs for unrelated existing errors. Apply the whole batch once via `config::edit::apply` (`src/config/edit.rs:96,126`); retain atomic write, external conflict retry, comments/order, no global reserialization. List ports as `instances_using` does (`src/commands/config.rs:390,426`); resolve actual run again if user subsequently requests restart. Windows outside-home destination: warn on read/validate; require `acknowledgeExternalAcl:true` on actual write, recalculate against the resolved file (or canonical parent for creation), fail closed if uncertain. Do not change ACLs or CLI behavior.

**Settings.** `settings::read` + validated effective schema, not fallback cache (`src/settings/file.rs:36,112,167`; `src/settings/edit.rs:228,244,273`). `readOnly` rows are `schema`, `language`, `aliases`, `default_config`, `config_dirs`, `defaults`, `watchdog.heartbeat_secs`, `watchdog.stale_after_cycles`, `watchdog.max_restarts`, `watchdog.idle_exit_secs`, `logs.rotate_mb`, `instances.idle_threshold_secs`, `update.download_chain`, `update.download_proxy`, `update.skill_auto_update`, `ui`. Preserve actual strings/bools/integers/arrays through `Cell`; URL-mask every custom download URL too. `defaults` gets the config rules above; **opaque** `settings.ui` only reports `{kind:'map',entries:[]}` (contents not exposed; not desktop prefs). For units: `_secs` seconds, `_mb` MiB, cycles/restarts are counts, schema version an integer. `set_watchdog` uses `settings::edit::apply` (`src/settings/edit.rs:45`) for exactly one key. On invalid settings, report diagnostic locations, no arbitrary repair API.

**Desktop state.** Separate fixed private files, atomically replaced under a per-home writer lock, each ≤256 KiB on disk; limit reads before allocation. Default prefs = `system/system/standard/false/tumbler/null`; absent bookmarks = `[]`. Store at most 100 unique bookmarks (no eviction to make room; show limit), new pins at front, existing pin and absent unpin idempotent. Capture at most 256 UTF-8 bytes each of method/path/model after substitution (known-secret cut-prefix protection), on character boundaries; no readable summary => don't pin. Rust accepts a null theme or at most 16 KiB of UTF-8 starting with `aproxy-theme-v1:` followed by syntactically valid JSON. Only the page interprets theme contents; it also falls back to defaults for unknown languages and mascots. This does not enable extensions. Missing file is not corruption; malformed/oversized file is atomically reset and reported (`DesktopState.reset` on read; on mutation reset then reject `desktop_state_reset`, let user retry). Permission/I/O error is **not** corruption; do not replace symlinks/reparse paths outside private state or overwrite inaccessible data. Limit/corrupt/old bookmark never enables a path read or a new target.

**Update and login.** Check via `Command(installed_exe)` with an argv vector `install --check --json`, optional `--pre` only. `auto` follows the installed binary, not the version of the WebView host (`src/install/download/mod.rs:183`). Capture only the verified check response; `install_update` accepts the opaque token for its latest still-current eligible check; invalidate on a new check/changed home binary and once a job is accepted. Execute `install --json <checked-semver>` (no `latest` race, alternate source, downgrade, `--force` or page argv), explicitly bind `APROXY_HOME` for both children. `about` can differ from `current` after an update. Parse bounded NDJSON records incrementally, discard/drain stderr without forwarding; never infer success from an intermediate phase. Do not kill an installation on detach/close: it owns its own ledger/transaction. On ordinary close during install, hide the window immediately. Keep the desktop process, ledger entry and pipe reader until the child exits, then clean up the ledger and exit. Normal install only stops instances, not the desktop: `about.version` is the old running binary until reopened and can differ from the next `UpdateInfo.current`. An externally killed host cannot promise terminal delivery. No background window timer or HomeView survives close. No claim to reconstruct another installer's history after host exit.

Start at login always reads the system. One item per canonical home (short SHA-256 suffix over normalized native path identity); register the installed executable with argv `restore-at-login --home <absolute-home>`, never via `APROXY_HOME` or PATH. The hidden command refuses relative paths and uses the same explicit-home restore implementation; public `restore` is unchanged. Windows uses this home’s HKCU Run value: quote argv by CommandLineToArgvW rules. On systems exporting `CreatePseudoConsole` (Windows 10 version 1809 or later), prefix the command with the absolute system `conhost.exe --headless`; older systems use the direct command and may flash a console. From a no-console parent on build 26300 the wrapper showed no visible window, while the direct form opened a terminal. Reject commands longer than the Run value’s 260 UTF-16-unit limit. Linux XDG uses safely encoded `.desktop` Exec arguments and `Hidden=false` (paths containing `%` are refused rather than expanded as field codes); macOS uses a per-user LaunchAgent with `ProgramArguments` and `RunAtLoad`, managed via `launchctl`. Known macOS limit: the loaded label is checked, not its effective program and arguments. Reject paths not safely representable with `desktop_registration` reason `unsupported-path`. Treat these as structured platform formats, not shell commands. No user input for item name, executable or registry/file target. Missing binary, unreadable system state or removed/mismatched registration must not look enabled; if previously enabled and now missing the page can explain removal by comparing its previous read. Without a previous observation, absent and manually removed are indistinguishable. Test-only injected OS slots, never the real login registration. For an entry mismatch return `enabled:false, issue:"entry-mismatch"`; if the executable is missing report `enabled:false, issue:"binary-missing"` even if the entry still exists. An unreadable OS store rejects with `desktop_registration` (do not label it off). Setting true without the binary rejects; setting false may clean up an exact orphan registration.

## 4. One event channel per window

```ts
type Frame = {
  session:Session; seq:SafeUint; // Rust attachment ID + consecutive frame number from 1, not view revision or IPC seq
  event:DesktopEvent; // exactly one payload; no app-wide broadcast
};
type DesktopEvent =
  | {type:'resync'} // invalidate EVERYTHING; use snapshot and reread open secondary panels/first page
  | {type:'instances-changed'} // membership, state, activity or watchdog invalidation; replace from snapshot
  | {type:'requests-changed'; run:Run} // invalidate cached recent rows for THIS run, including evictions/removal
  | {type:'rescued'; ref:Ref; at:Time} // ephemeral increment, at = row.endedAt in Unix ms
  | {type:'update-progress'; job:Job} // full replacement of in-memory job, including terminal failure/success
  | {type:'desktop-state-changed'} // replace bookmarks/preferences by read_desktop_state
  | {type:'settings-changed'}; // reread settings after successful watchdog write
```

1. Install `Channel.onmessage` **before** `attach`. Serialize attach attempts in JS. First frame is always `resync`; it may arrive before `attach` resolves: buffer that frame, compare returned session, then process. Rust subscribes to `HomeView::changes()` **before** initial read (`aproxy-ipc/src/view/mod.rs:556`). Keep a single reader/coordinator for the window; reload creates a new attachment, not a second view. A standalone `snapshot` is just a sample, not a subscription barrier. It may initially be empty while discovery runs; never start an instance automatically on an empty sample.
2. Apply frames serially for the current session, ignore obsolete sessions/duplicates. On resync, drop virtual-page cursors, selected stale work and transient animations, replace membership/recent/last job via `snapshot`, reread visible panels including bookmark files. On `instances-changed` replace cards/watchdog, drop data for retired runs, reread recent head. On `requests-changed`, replace the affected run's **recent window**, not append duplicates (for a paged list, invalidate and refetch its visible query from the head; optional retention only of matching-revision rows). `snapshot.recent` is a home-wide head, **not** a complete per-run membership list. Never retain an invisible row as proof it still exists. Already pinned summaries remain. `settings-changed`/`desktop-state-changed` also notify the caller's own page: replacements are idempotent. No separate update/ack IPC subscription to the instance.
3. Acknowledge `ack({session,seq})` **after** applying/re-reading, including a localized error display if the read fails. One outstanding frame normally. If unacknowledged for 10 seconds, collapse pending state to a single forced `resync` and send it without waiting; retire the previous seq. One forced frame per stall, rearm only on a matching acknowledgement. Rate-limit `instances-changed` and `requests-changed` (independently per run) to one emission per 250 ms, coalescing intervening changes. While waiting, coalesce repeated invalidations by domain/run and update jobs to their newest full value. Up to 128 pending distinct invalidations/notifications per attachment; on bridge overflow or view `broadcast::RecvError::Lagged`, collapse to a single `resync`, drop pending celebrations, resume after it is acked. No lossless audit log or replay. Any sequence gap/failed delivery: detach if possible, create a fresh channel and attach; a stalled listener cannot accumulate unbounded messages. New attachment supersedes old, clears its pending queue, selection and cursors. Install job state and view continue. Serialize concurrent page rereads per domain (or discard superseded responses); a non-atomic sample of separate domains is fine: changes during reads stay dirty until after the outstanding frame. On window destruction abort reads/driver, await shutdown off the event loop (`aproxy-ipc/src/view/mod.rs:658`); cancel timers, discard window state.
4. `Change::Instance(port)` => `instances-changed` (membership includes removals); `Change::Requests(key)` => `requests-changed` including an old run on replacement; `Change::Completed{run,request_id}` => look up **that exact row in that run**, skip if evicted/replaced. Only emit `rescued` if terminal `outcome==='ok'`, `endedAt!==null`, `attempts>=2` and **(`attempts-1>=3` OR `max(0,endedAt-startedAt)>=15000` ms)**. Use total attempts, not `tries.length`; `at` is completion, not notification time. No synthetic notification on first load, restore, page, lag catch-up, duplicate terminal, poll or reconnect; suppress pre-attachment/completion backlog across resets. Source transition `aproxy-ipc/src/view/mod.rs:82,355`; prototype thresholds `aproxy-desktop/ui/src/lib/backend/mock/engine.ts:16,285`. Best effort: skipping an animation is preferable to a duplicate. Ordinary rescued counts come from activity, not counting notifications. Independent watchdog tick changes => `instances-changed`; successful local desktop/settings writes invalidate their respective domains (don't notify forever on an unchanged reread).
5. For install lines (`src/commands/install/events.rs:19,45,50,87`), fold `plan.ports` into targets, `phase.phase/at` into phase, `progress.bytes/total` into download, `instance.port/outcome` into per-port outcome (`src/install/restart.rs:60`), emit the resulting **whole Job** each time (may coalesce). On `result`, capture `installed`/`up_to_date`/`failed`, optional version, and project only code/allowlisted details on failure. Publish terminal job **and** settle the promise after checking exit status; `done`/`cleaning` phase alone is never success. Missing/duplicate/malformed terminal, successful result + nonzero exit, or success exit + failed result => `desktop_install_protocol`; unknown future event *type* ignored, unknown phase/instance outcome rendered `unknown`, never mapped to success. Spawn failure/nonexistent installed binary => `desktop_binary`; check uses one JSON object including the error form at `src/commands/install.rs:365`. Ignore extra fields and URL; never publish stderr, entire lines, or error.message. Cap individual input lines at 1 MiB with bounded reads; on parse/size failure keep draining without storing output until exit, report protocol fault, never kill an active transaction. No synthetic progress fraction if total absent. A terminal progress frame may arrive after the invoke promise; never start a second job based on event timing.

## 5. Trust boundary

Only redacted projections cross Rust → page (also on copy, errors, dry run, installer failure, events and persisted bookmarks). `aproxy-ipc/src/redact.rs:18,49,203,225,270,384,399`, `src/config/mod.rs:626`, `src/config/view.rs:173`, `src/settings/edit.rs:228` are the shared policy and its **inputs**, not proof that an arbitrary serialized object is safe.

|Source|Allowed representation|
|---|---|
|`api_key`, separate proxy username/password, both configured header tables' values; same in `settings.defaults` including withheld/overridden entries|Write-only input; effective config returns `{kind:'secret',set:…}`, no hint. Preserve unchanged via omitted patch, not fake asterisk value. Header names are display text; compare normalized HTTP names against the name-specific known-secret matcher. Never authorize editing a member by its masked name.|
|Userinfo and query values of base/proxy/download URLs, custom download-chain URLs; actual proxy env and captured launch env|URL shape only, userinfo and all query values masked; don't expand environment values for the page. Separate credentials never shown. Private spec (`src/spec.rs:31,108`) and full env never serialized.|
|`format[].env` values and operands of secret flags (`src/config/mod.rs:230`), including inline `--flag=value`|Env values are set flags, secret arg operands marked, inline value masked; no secret command-line replay. Do not expose process state, arbitrary report data or stderr.|
|H strings, URL/headers/bodies, format actions, errors and messages|Peer must advertise source redaction. Header names and nonsecret values have known-secret replacement; secret headers only marker; body bytes already stream-substituted before base64, never “scrub the base64”. Known-secret substitution is not a guarantee against unknown third-party secrets in user content. Treat as sensitive, body only on demand.|
|Raw config incl invalid types, default credentials not inherited by this config, pending patch, path/alias, saved support data, input/peer/child/OS errors|Allowlist structured leaves first. Build known set from the relevant file, settings defaults, protected URL components, private run overrides where applicable, **both** overwritten/withheld and candidate values; apply §1.5 substitution (known values ≥8 bytes, `<secret>`) to every L/H display string and dynamic map name **before** escaping, truncation, logging, persistence or serialization. Source-backed H is scrubbed at source; applying a JSON-string replacement after serialization is not equivalent for quotes/backslashes. Never expose raw file trees or parse messages even on malformed input. A secret in its native slot is masked at any length. If a string's safety cannot be established, omit it/return a fixed error, not the unredacted fallback.|

Identifiers/codes from disk or peers are not trusted merely because the type is `String`: validate program-generated vocabulary, lengths and expected identity; never forward arbitrary status/phase/code names as localized keys. If an identity is unsafe to expose (including if known-secret substitution would change it), withhold it: no Run/actions on that card, and no addressable request/bookmark for that identity. Never match two redacted strings to authorize a write. A malformed persisted bookmark is not a new read capability. Record IDs normally contain only the generated run ID and decimal seq (`src/proxy/state.rs:152`, `aproxy-ipc/src/view/mod.rs:311`). On legacy peers suppress unsafe content, rather than attempting to make it safe in JS. No arbitrary URL/command/path, no clear diff, shell snippet with secrets, raw error envelope, TOML excerpt or structured `details` pass-through. Validate boundaries even if the intended caller is our own TS.

## 6. Errors and strings to localize

Stable **codes** are snake_case identifiers (not enum display values). Keep published CLI code names/details intact; the definitions here are *desktop projections*. `desktop_*` are an enum private to the bundled app contract in `aproxy_ipc::desktop`, never entries of the shared code table or `docs/error-codes.md`. Test the HistoryCode, DiagCode and install-family classifications against the shared code table so that new members require an explicit projection decision. Fixed detail keys, no `message`. In TS `F<C,D>` distributes the code union into tagged variants.

```ts
type F<C extends string,D=None> = C extends unknown ? {code:C; details:D} : never;
type InstallSimple = 'install_in_progress'|'install_version_not_found'|'install_below_floor'|'install_download_failed'|'install_checksum_mismatch'|'install_probe_failed'|'install_swap_failed'|'install_rolled_back'|'install_failed'|'install_unmanaged_instances';
type Fault =
  | F<'desktop_bad_input'|'desktop_unavailable'|'desktop_window'|'desktop_cursor'|'desktop_changed'|'desktop_history'|'desktop_gone'|'desktop_cancelled'|'desktop_busy'|'desktop_target'|'desktop_external_acl'|'desktop_conflict'|'desktop_binary'|'desktop_install_protocol'|InstallSimple>
  | F<'desktop_unresponsive'|'desktop_force_refused'|'desktop_no_spec'|'desktop_port'|'install_restart_failed',{port:number}>
  | F<'desktop_config',{diagnostics:Issue[]}>
  | F<'desktop_start',{reason:'spawn'|'not-ready'; oldStopped:boolean}> // after restart stop succeeded vs initial start (false)
  | F<'desktop_stop',{port:number;reason:'still-running'|'unverified'|'permission'|'failed'}>
  | F<'desktop_state_io'|'desktop_state_reset',{file:'preferences'|'bookmarks'}>
  | F<'desktop_limit',{resource:'bookmark-count'|'bookmark-bytes'|'preference-bytes'|'theme-bytes';max:number}> // 100 pins, 262144 bytes/file, 16384 bytes/theme; no silent eviction
  | F<'desktop_registration',{reason:'unsupported-path'|'permission'|'binary-missing'|'entry-mismatch'|'system-unavailable'|'verification-failed'}>
  | F<'install_foreground_instances',{ports:number[]}>
  | F<'install_precheck_failed',{port:number;diagnostics:Issue[]}>;
// All fields below I: exact name known to local code table, otherwise the single literal 'unknown'.
type HistoryCode = 'unknown'|'inbound_host_rejected'|'inbound_origin_rejected'|'request_too_large'|'client_body_aborted'|'upgrade_unsupported'|'local_storage_failed'|'response_too_large'|'retry_exhausted'|'retry_stopped'|'format_rejected'|'format_unavailable'|'format_failed'|'upstream_failed'|'decode_failed'|'shutting_down'|'internal'|'upstream_connect_failed'|'upstream_connect_timeout'|'upstream_first_byte_timeout'|'upstream_read_idle_timeout'|'upstream_body_interrupted'|'upstream_status'|'upstream_error_body'|'format_error'|'format_fail'|'format_decision';
type DiagCode = 'unknown'|'config_syntax'|'config_type'|'config_invalid_value'|'config_missing_key'|'config_unknown_key'|'config_not_supported'|'config_conflict'|'secret_placeholder_unmatched'|'default_credential_withheld'|'default_header_shared'|'forward_default_unguarded'|'forward_response_buffered'|'settings_schema_too_new'|'format_handshake_failed'|'format_stage_unsupported'|'file_permissions_too_open'|'listen_not_loopback'|'port_shared'|'config_dir_missing'|'config_file_unreadable'|'watchdog_slow_hang_detection'|'watchdog_restarts_disabled'|'install_restore_unreadable'|'install_config_unreadable';
```

Details not listed do not cross. `desktop_bad_input` invalid shape/value; `desktop_unavailable` service/other IO failure; `desktop_window` native window call failed; `desktop_cursor` restart paging; `desktop_changed` refresh target/check identity; `desktop_history` history/policy/peer unavailable; `desktop_gone` request/body no longer retained; `desktop_cancelled` stale selection; `desktop_busy` operation already in progress; `desktop_target` target missing or has no backing file; `desktop_external_acl` confirm external-file warning; `desktop_conflict` lock busy/external write conflict; `desktop_binary` installed executable unavailable; `desktop_install_protocol` unusable child output/exit. `desktop_unresponsive` no graceful reply; `desktop_force_refused` not a positively verified unresponsive run; `desktop_no_spec` cannot restart without desired state; `desktop_port` listener unavailable; `desktop_config` invalid settings/config; `desktop_start` failed to launch/reach ready; `desktop_stop` stop unconfirmed or refused; `desktop_state_io` cannot read/replace private state; `desktop_state_reset` corrupt private file was reset, reread; `desktop_limit` local state would exceed limit; `desktop_registration` could not inspect/change/verify OS registration. `unknown` means not known by this bridge, not an assertion about source severity. `ReadFailure::{RunChanged,Gone,Unavailable}` map respectively to `desktop_changed`, `desktop_gone`, `desktop_history` (`aproxy-ipc/src/view/mod.rs:100`). Child code allowlist is the install family in `docs/error-codes.md:137`; unknown child failure code -> `install_failed` with `{}`, never propagate peer prose or invent an exit-code mapping. For secondary diagnostic/history code meanings see `docs/error-codes.md:13,36,97` (also listed in the receipt).

## 7. Injection and permissions

`aproxy desktop` already resolves the home, checks its ledger, creates/enters the Tokio runtime and calls `run` (`src/commands/desktop.rs:103,153,167`). Extend `Options`, not command arguments. No reverse dependency on the binary crate or test harness linked against the graphical runtime. Signature notation below: Rust DTOs have the same variants/fields as §§1–6, Rust numeric/Path types internally, explicit serde projection at the edge. `Run`/`Ref` identify a run/request inside Rust; numbers are encoded as specified above. `Choice` handles, restore tokens and check tokens are minted/validated by the same owning service, not JS. `R<T>` is the only error type crossing these methods. `PreferencesPatch` is the exact optional-key `Partial<Preferences>` (absent theme vs explicit null distinct). `ChannelChoice = Auto | Pre`. The callback accepts a **projected** `Job`, not raw child data. `PinChange` is Rust-only `Pin(Bookmark) | Unpin(Ref)`; pin data comes from the verified view, never from the invoke body. Existing shell `desktop_info` is not `about`.

```rust
pub struct Options {
    pub data_dir: PathBuf, pub runtime: tokio::runtime::Handle, // existing, see aproxy-desktop/src/lib.rs:74
    pub home: PathBuf, pub inspect: aproxy_ipc::desktop::InspectHome,
    pub ops: Arc<dyn DesktopOps>,
}
type R<T> = Result<T, Fault>;
type Task<'a,T> = Pin<Box<dyn Future<Output=R<T>> + Send + 'a>>;
type Progress = Arc<dyn Fn(Job) + Send + Sync>;
pub trait InstanceOps: Send + Sync + 'static {
    fn list_configs(&self) -> Task<'_,Vec<Choice>>;
    fn start_instance(&self, target: StartTarget) -> Task<'_,Run>;
    fn stop_instance(&self, run: Run) -> Task<'_,()>;
    fn restart_instance(&self, run: Run) -> Task<'_,Run>;
    fn force_stop_instance(&self, run: Run) -> Task<'_,()>;
    fn restore_instance(&self, port: u16, expected_restore: Token) -> Task<'_,Run>;
    fn forget_restore(&self, port: u16, expected_restore: Token) -> Task<'_,()>;
}
pub trait ConfigOps: InstanceOps {
    fn read_config(&self, target: ConfigTarget) -> Task<'_,ConfigView>;
    fn validate_config(&self, target: ConfigTarget, changes: Vec<Edit>) -> Task<'_,Validation>;
    fn write_config(&self, target: ConfigTarget, changes: Vec<Edit>, acknowledge_external_acl: bool) -> Task<'_,Saved>;
    fn read_settings(&self) -> Task<'_,SettingsView>;
    fn set_watchdog(&self, enabled: bool) -> Task<'_,bool>;
}
pub trait StateOps: ConfigOps {
    fn read_desktop_state(&self) -> Task<'_,DesktopState>;
    fn set_preferences(&self, changes: PreferencesPatch) -> Task<'_,Preferences>;
    fn set_pinned(&self, change: PinChange) -> Task<'_,Vec<Bookmark>>;
    fn about(&self) -> R<About>;
}
pub trait UpdateOps: StateOps {
    fn check_update(&self, channel: ChannelChoice) -> Task<'_,UpdateInfo>;
    fn install_update(&self, check: Token, progress: Progress) -> Task<'_,InstallEnd>;
}
pub trait DesktopOps: UpdateOps {
    fn read_autostart(&self) -> Task<'_,Registration>;
    fn set_autostart(&self, enabled: bool) -> Task<'_,Registration>;
}
```

`home` is the resolved absolute path; `running_exe` the current executable used to launch a run, `installed_exe` exactly `install::swap::bin_path_in(home)` (`src/install/swap.rs:32`), even when the running binary is elsewhere. Only `DesktopHome` knows the executable paths: the CLI binds the running executable at construction and the installed path is derived from the absolute home. `Options.ops` and the discovery callback `Options.inspect` share that very same host. All methods above are callable on that one trait object. `about` is synchronous on a blocking worker; its display path is freshly scrubbed using current configuration. Private-state operations queue in the host; persistent cross-process lock contention is `desktop_state_io` for the relevant file. All `Task` methods may wait for disk/locks; control also waits for verified peer/child readiness, update for a child/network, registration for the system. Move blocking filesystem/registry/platform process work onto blocking workers; update progress callback only posts a bounded notification. Keep `attach/detach/ack`, snapshot/paging/detail/cancel/rescan, job snapshot and OS window operations in the window adapter; open `HomeView::open` exactly once on the injected runtime. Add a read-side accessor for home/record metadata needed for the view, single scan wake and verified pages, rather than having every UI command subscribe or run another enumerator. Restore token is an opaque comparison generated from observed canonical desired-state contents by a common Rust-only issue/verify mechanism (per-window keyed digest over port and private record; never unkeyed secret hash, never serialize the record); if absent/unreadable no restore button. The same per-window context is injected into the read projection and ops. `DesktopOps` validates it again. Use explicit-home variants within **the same** CLI operation functions (including config defaults, spawn environment, watchdog setup and cleanup), not in-process CLI argument parsing, `set_var` or per-window global home. Reserve OS-target injection for tests. The `aproxy-ipc` feature `desktop` implies `client` and adds no dependency. `aproxy_ipc::desktop` contains the DTOs, `Fault`, `DesktopOps`, pure projections and pure coordinator rules (coalescing, rescued threshold and cursor merge order), with unit tests. This is the bundled window’s application interface, versioned with it, not stable for third parties. `aproxy-format` does not enable it; keep the featureless dependency check. The `aproxy` library implements the trait in both build variants so headless `TestHome` integration tests exercise the real implementation. `aproxy-desktop` owns only Tauri commands, the per-window coordinator and handles, and the window itself.

**ACL (complete grant set).** `capabilities/main.json` grants `allow-` + command name with every `_` changed to `-`, for **exactly** these names:

```
desktop_info titlebar_insets set_caption_color attach detach ack snapshot query_logs request_detail
body_chunk cancel_detail rescan list_configs start_instance stop_instance restart_instance
force_stop_instance restore_instance forget_restore read_config validate_config write_config read_settings set_watchdog
read_desktop_state set_preferences set_pinned read_autostart set_autostart check_update install_update about
```

Keep just `core:window:allow-show` (show after page ready), `core:window:allow-set-effects` (Mica toggle), `core:window:allow-set-theme` (selected window appearance). No `core:default`, event plugin listen/emit, remote capability, opener, HTTP, fs, shell, process, URL navigation, raw IPC, clipboard plugin or generic dispatch. The JS Channel is passed in invoke, not a `listen` subscription (`tauri@2.12.3 src/ipc/channel.rs:271`, internal fetch exception `src/webview/mod.rs:2082`; `Cargo.lock:3794`). Keep the current CSP, local main window, non-same-origin sandbox and navigation limits (`aproxy-desktop/src/lib.rs:13,346`). No grant to a future extension frame. Non-Windows frontend never requests native caption operations; current manifest lists both, current handler uses Windows-only registrations (`aproxy-desktop/src/lib.rs:166`).

**Four-place build check (32 command names).** Per name `n`, (1) `aproxy-desktop/build.rs:10` app manifest `commands` contains `n`; (2) committed `aproxy-desktop/permissions/autogenerated/<n>.toml` defines `allow-<kebab>` for just `commands.allow=[n]` and corresponding deny; (3) unique `aproxy-desktop/capabilities/main.json:6` grants that allow once; (4) invoke handler in `aproxy-desktop/src/lib.rs:166` registers `n`. `aproxy-desktop/src/command_list.rs` generates both the manifest input and the actual handler expression; Rust rejects missing functions. Independently compare parsed checked-in permission allow/deny mappings, complete capability grants and the generated names **before** tauri-build can regenerate stale files. Fail the build on missing/extra/duplicate/wrong mapping, extra capability/core grant, or unexpectedly public handler. Check the two existing cfg(windows) entries as platform-scoped (the full list includes them; non-Windows effective native handlers absent by design); do not remove cross-platform permissions files on a Linux build. One checked list, not a comment promising four lists agree.

## 8. Prototype migration checklist

Names on the left are from `aproxy-desktop/ui/src/lib/backend/types.ts:32,43,75` and `aproxy-desktop/ui/src/lib/types.ts:36,77,103,141,202,275,283`. No value comes from the demo RNG, startup scene or fake saved profiles. “Drop” means omit/hide in W5c, not zero/empty placeholder data.

|Backend method/property|Real adapter|
|---|---|
|`demo`, `snapshot`, `subscribe`|Constant false; `snapshot`; one `attach`/`ack`/`detach` per window shared by pages.|
|`queryLogs`, `requestDetail`, `setPinned`|`query_logs` (run identity, cursor), `request_detail` + `body_chunk`/`cancel_detail`, `set_pinned` + `read_desktop_state`.|
|`startInstance`, `stopInstance`, `restartInstance`, `forceStopInstance`, `reconnectInstance`|`list_configs`/`start_instance`; `stop_instance`, `restart_instance`, `force_stop_instance` with observed run; `rescan`. `restore_instance` retries and `forget_restore` dismisses an observed pending desire, not a saved profile.|
|`readConfig`, `validateConfig`, `writeConfig`|`read_config`, `validate_config`, `write_config` (typed leaf batch, ports to restart).|
|`readSettings`, `writeSettings`|`read_settings` + `read_desktop_state` + `read_autostart`; route individual mutations to `set_watchdog`, `set_preferences`, `set_autostart` (not one atomic cross-store write).|
|`checkUpdate`, `installUpdate`|`check_update` with auto/pre, `install_update` with the offered check handle.|
|`listExtensions`, `setExtensionEnabled`, `revokeGrant`, `answerConsent`, `readMascot`|Deferred W5e, no W5b Tauri command or in-page extension identity. Builtins stay local.|

|Prototype event / snapshot field|Real replacement|
|---|---|
|`snapshot`, `instances`, `stats`|`resync` + snapshot; `instances-changed` + replace cards/watchdog; totals and buckets live on each run.|
|`logs-appended`, `logs-updated`, `logs-removed`|`requests-changed` invalidates recent rows for the run; replace/requery (including evictions), no assumed append-only delta.|
|`rescued`, `update-progress`|Exact completion rule in §4; full Job folded from child events, never demo ticks.|
|`extensions`, `consent-request`|Deferred W5e, no W5b event.|
|`Snapshot.instances`, `.logs`, `.watchdog`, `.rescuedTotal`, `.rescuedToday`|`instances`, `recent` **newest** first, `watchdog`; label `activity.rescuedTotal` per current run (sum as numbers only if clearly labeled sum of visible runs); daily no source: dropped. No hourly rescued bucket; hourly **requests/retries/failures** only.|

|`Instance` field|Real source or change|
|---|---|
|`port`, `alias`, `pid`, `version`|`Card.port`; use `metadata.aliases` (possibly many; never join redacted path labels in JS); `pid`, nullable `version`.|
|`status`, `silentSince`|`liveness` + `restorePending` + `state` + `feed` + `historyUnavailable`; no timestamp for first silence: drop. No stopped/restarting invention.|
|`forwardOnly`, `baseUrl`, `listen`, `configPath`, `proxy`|`metadata.delivery==='forward'`, `metadata.baseUrl`, `.listen`, `.configPath`; redacted `proxy` in file config only, not live-card data.|
|`startedAt`, `lastRequestAt`, `req1h`, `retry1h`, `lastError`, `spark`|`startedAt`, `activity.lastRequestAt` (hide when no requests), bucket sums only for advertised hour width/count, `activity.lastError`, `activity.buckets` with actual width/order. No inferred missed samples.|
|`transform`, `keepalive`, `backoffCap`, `boundedPaths`, `diskCache`|Read-only config `format` (no invented key count/channel map); `metadata.keepaliveIntervalSecs`/`.keepaliveTrigger`; `retry.max_backoff_secs` and structured `retry.rules` from config **not** live; `metadata.diskCache`.|

|`LogEntry` / `Attempt` field|Real source or change|
|---|---|
|`key`, `seq`, `port`, `time`|`ref` (use tuple, not mutable port or split key), numeric `seq`, `ref.run.port`, `startedAt`.|
|`method`, `path`, `model`, `stream`, `client`|`method` arbitrary H HTTP token, `path`, nullable `model`, `stream`, nullable `userAgent`, not a guessed client product.|
|`outcome`, `attempts`, `phase`, `phaseStartedAt`, `phaseEndsAt`|`Outcome`, `tries` + count `attempts` + `triesOmitted`, `Phase`; no source for every phase start: drop; `nextAttemptAt` for backoff only.|
|`durationMs`, `reqBytes`, `respBytes`, `heartbeats`|Derive `max(0,(endedAt ?? displayClock)-startedAt)`; `requestBytes`, `responseBytes`, `heartbeats` as numbers.|
|`transformed`, `keyIndex`, `pinned`, `fresh`|Show recorded reports/actions, not a boolean proving body changed; key-select may have public keyId, never fabricate an index; join bookmarks by Ref; local-only animation.|
|`Attempt.n`, `.startedAt`, `.durationMs`, `.status`, `.reason`, `.backoffMs`, `.respBytes`|`Try.attempt`, `startedAt`, nullable numeric `durationMs`, nullable `upstreamStatus`, `code`, nullable `decision.delayMs` (not invented 0), `responseBytes`.|

|`RequestDetail` / nested field|Real source or change|
|---|---|
|`key`, `request.headers`, `request.body`, `request.curl`, `request.raw`|`record.ref`, `request.headers` with markers, `request.body` metadata + chunk with `{kind:'request'}`. No source for original curl/raw wire: dropped; don't reconstruct an executable credential-bearing request.|
|`attempts`, `attempts[].headers`, `.transportError`, `.errorBody`, `.sse`, `.body`, `.raw`|`tries` keyed by number; distinguish sent/response headers, `error` code + diagnostic, actual response metadata + bounded response chunk. No separate error-body/SSE/JSON/raw store. Optional local bounded display parsing only.|
|`transform`|`record.reports` and per-try reports/actions; not a made-up before/after diff.|

|`InstanceConfig` + `ConfigValues` field|Real source or change|
|---|---|
|`port`, `path`, `values`, `apiKeyMasked`, `live`|Use caller's observed target (may not be a port); `file`; `fields` + `withheld`/`diagnostics`; `api_key` set/not set; no live-reload keys: use `Saved.instancesToRestart`.|
|`base_url`, `listen_addr`, `proxy`, `connect_timeout_secs`, `read_timeout_secs`|Editable `base_url`, `listen_addr`, `proxy`, `timeouts.connect_secs`, `timeouts.read_idle_secs`; separate `timeouts.first_byte_secs`.|
|`max_retry_backoff_secs`, `bounded_retry_paths`, `forward_only`|Read-only `retry.max_backoff_secs`, `retry.rules` (path/status/max_attempts; not simply paths), `delivery`.|
|`spool_limit_mb`, `max_body_mb`, `disk_cache`|Read-only `max_response_mb`, `max_request_mb`, `disk_cache`.|
|`keepalive_interval_secs`, `keepalive_trigger`, `keepalive_heartbeat`|Editable `keepalive.interval_secs`, `keepalive.trigger`, `keepalive.heartbeat`.|
|`request_transform`, `allowed_hosts`, `allowed_origins`|Read-only `format` array, `allowed_hosts`, `allowed_origins`; command/args/process/stages only as projected actual entries, no imaginary extra/every_attempt toggle.|

|Settings, watchdog and update field|Real source or change|
|---|---|
|`AppSettings.watchdog`, `.autostart`, `.ui`|`SettingsView.watchdogEnabled`, fresh `Registration.enabled` (respect issue/error), `DesktopState.preferences`.|
|`UiPrefs.lang`, `.mode`, `.palette`, `.mica`, `.mascot`, `.theme`|Same preference names; `mica` is desire vs `desktop_info.mica` support, unknown language/mascot falls back on the page, theme serialized versioned string/null, parsed by existing theme code.|
|`WatchdogInfo.pid`, `.lastScanAt`, `.respawns`|`Watchdog.pid`, `.lastScanAt`, `.restarts` including actual `.port`, `.at`, `.kind`, `.ok`; show stale vs running, no owner => null.|
|`UpdateInfo.channel`, `.current`, `.latest`, `.variant`, `.publishedAt`, `.checkedAt`, `.notes`|Actual channel/current/nullable latest/variant reported by the **installed child**, nullable publishedAt, bridge checkedAt; release notes no source: dropped. Use `updateAvailable` rather than comparing version strings.|
|`UpdateProgress.version`, `.stage`, `.progress`, `.restarted`, `.targets`, `.downloadedMb`, `.totalMb`|`Job.requestedVersion` vs `end.version`, actual `phase`, ratio only from nonzero known byte total, successes in `instances`, nullable `targets`, `download.bytes`, nullable `download.total` (convert for display).|

## 9. Contract decisions

The interface and its implementation boundary above are approved. Older retained history is on demand; a memory-only first page, bounded frame stream and generation-fenced selection keep ordinary refreshes cheap. No saved instance list, generic write, script, URL opener, or extension authority is added. External-file warnings and write-only secrets apply even to trusted local pages. Start at login is explicitly opt-in and bound to one absolute home.
