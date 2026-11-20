# Troubleshooting format programs

Symptoms caused by a format program or a chain, with causes and fixes. Read it when a request
ends with a code that names a format, an instance will not start, conversion does not happen, or
keys and channels do not rotate as expected.

## Contents

- [Find out what happened](#find-out-what-happened)
- [The instance does not start](#the-instance-does-not-start)
- [A request ends with an aProxy error](#a-request-ends-with-an-aproxy-error)
- [Process-level failures by kind](#process-level-failures-by-kind)
- [The breaker is open](#the-breaker-is-open)
- [Wrong or missing conversion](#wrong-or-missing-conversion)
- [Keys and channels](#keys-and-channels)
- [Retries and delivery](#retries-and-delivery)
- [Leftover processes](#leftover-processes)

## Find out what happened

Work from the code, never from message text. Messages are for people and change between versions.

- **The client's error.** aProxy's own errors carry the code in three places: the JSON body
  (`error.aproxy.code`, with `error.aproxy.details`), the `x-aproxy-error` response header, and,
  when response headers were already committed by a keepalive, a final SSE `event: error`. The
  details name the entry (`format`) and the `stage`.
- **The instance log** (`aproxy logs <port|alias>`). A program's stderr lines appear there
  prefixed `format[<name>]`. aProxy's own lines about formats use fixed English messages; the
  useful ones to search for are `format circuit opened`, `format circuit half-open`,
  `format circuit closed`, `optional format unavailable at startup`, `decide failed; no vote`,
  `stream format layer skipped`, and the `... omitted from format output` family (a header your
  program returned was dropped).
- **Breaker state:** `aproxy status --json`, under `formats[]`: `breaker`, `notify_breaker`,
  `notify_dropped_total`.
- **Reproduce without the instance.** For a program of your own, `python scripts/test_format.py
  -v --command <program> -- <args>` shows every frame and the program's stderr. For a chain of
  official roles, `aproxy-format check <config.toml> --home <home>` and then `aproxy-format try`
  with a sample that reproduces the upstream's answers (examples.md, Tools).

## The instance does not start

| Diagnostic | Cause | Fix |
|---|---|---|
| `config_conflict`, one entry declares `response` and `stream` | A program gets a response either whole or in pieces, not both | Keep `stream`; it also handles non-streaming bodies. |
| `config_conflict`, `stream` or `notify` with `process = "spawn"` | Those stages need a worker that lives across messages | `process = "persistent"`. |
| `config_conflict`, an entry after a `retry` entry does not declare `retry` | A retry reruns the chain from the first `retry` entry; everything after it must be redone for the new destination | Add `"retry"` to the later entries' `stages` (the usual case: `convert` or `rewrite` placed after `route`), or move the entry before the first `retry` entry. |
| `config_invalid_value` on `command` | On Windows, a `.cmd` or `.bat` file (npm shims are `.cmd`). They only run through `cmd.exe`, which aProxy could kill while the real program lived on | Put the interpreter in `command` and the script in `args`, or use the release `.exe`. |
| `format_stage_unsupported` (exit code 3) | `stages` lists a stage the program's hello does not | Remove the stage, or implement it. For an official role, see its stage list in examples.md. |
| `format_handshake_failed`, exit code 3 | The hello reply was invalid or chose a protocol version aProxy does not speak | Run the harness; the first stdout line must be the hello reply and nothing else. |
| `format_handshake_failed`, exit code 1 (`details.kind` is `spawn`, `died`, `timeout` or `io`) | A required program could not be started, exited, or did not answer hello within 10 seconds | Run the configured command by hand. For `keys` and `route`: an environment variable named in the role file is not set for the instance. A relative path resolves from the instance's home. |
| Startup succeeds with the warning `optional format unavailable at startup` | Same causes, on an entry with `required = false`: it starts with its breaker open and is probed after the cooldown | Fix as above; no restart is needed once the probe succeeds. |

`aproxy doctor` reports the configuration diagnostics without starting anything. Two notes it
may print about a chain are not errors: `forward_default_unguarded` (the instance's default
delivery is `forward` while the chain relies on a `decide` program or an optional request program;
when those are unavailable, requests are not retried) and `forward_response_buffered` (a
`response` program gathers each response of a forward request, so it is not streamed; declare
`stream` instead).

## A request ends with an aProxy error

| Code and status | Meaning | What to do |
|---|---|---|
| `format_rejected`, 422 (or the program's own response) | A request-side program answered `fail` with `retry: false`. `details.format` and `details.stage` say which; `details.fail_code` is the program's own code | It is a decision, not a malfunction. Read the reason. Common with official roles: `convert` cannot tell the client's protocol or the body is not valid JSON for it; `route` has no channel for the model; `keys` has no pool for the destination origin. |
| `format_unavailable`, 503 | A `required` program's breaker is open, so the request cannot pass that layer. `details.probe_at` is when aProxy will try the program again | See [The breaker is open](#the-breaker-is-open). Requests are refused at once instead of retried, on purpose: retrying into a broken converter only hides the outage. |
| `retry_stopped`, 502 (or the program's response, or the upstream's) | A `decide` program voted stop. `details.last_code` is why the last attempt failed | Expected when a `policy` rule matched. If it was not wanted, find the rule: the request record's `decision.by` names the program, and its report names the rule. |
| `format_failed`, 502 | Forward request only: a request-side program failed or asked for a retry, and no `decide` program upgraded the request to retry mode | Either fix the program (`details.kind`, below), or deliver in retry mode so that such failures are retried. |
| `upstream_failed`, 502 | Forward request only: the upstream attempt failed before any byte reached the client, and nobody upgraded the request | Not a format error. It is what `forward` means; add `adaptive` or a `policy` rule that votes retry to upgrade such requests, or use retry delivery. |

A request in retry mode whose program fails at the process level does **not** end: the attempt
fails with `format_error` and is retried like an upstream failure. So a broken required program
shows up as requests that hang and retry until the breaker opens, and then as 503s.

## Process-level failures by kind

`details.kind` in `format_error`, `format_failed`, `format_unavailable` and
`format_handshake_failed`:

| `kind` | Meaning | Usual cause and fix |
|---|---|---|
| `spawn` | The process could not be started | `command` not found or not executable. A bare name is looked up in `<home>/bin/`, then PATH; on Windows a bare name finds only `<name>.exe`. |
| `died` | It exited before replying | An unhandled exception, a missing module, a wrong script path in `args`. Run the command by hand and read stderr. |
| `io` | A pipe broke mid-exchange | The program closed stdout or crashed while writing. |
| `timeout` | No reply line in time (`timeout_secs`, 30 by default; `decide_timeout_secs`, 2, for `decide`) | Output not flushed; reading stdin to EOF instead of line by line; slow work inside `decide`. Raise the timeout only for work that is slow by nature. |
| `protocol` | A reply's `seq` did not match, or the hello was unusable | A reply written twice, a message skipped, a stray line on stdout. |
| `invalid_output` | A reply line was not a valid frame: not JSON, not an object, both `body` and `body_b64`, bad base64, a non-absolute `url`, a `delay_ms` out of range, or longer than the line limit | Print diagnostics to stderr only; write compact single-line JSON; keep large replies under hello's `limits`. |
| `open` | The breaker is open; the program was not called | Next section. |

The harness reproduces all of these except `open`.

## The breaker is open

`breaker_failures` (default 5) process-level failures in a row open an entry's breaker for
`breaker_cooldown_secs` (default 30); then one message probes a fresh worker. The log shows
`format circuit opened`, later `format circuit half-open`, and `format circuit closed` when the
probe succeeds.

- Find the first failure before `format circuit opened` in the log and its `kind`; fix that. The
  breaker is the consequence.
- A deliberate `fail` reply, an abstention and any other valid reply reset the count. If the
  breaker opens on rejections, the program is crashing or timing out where it should answer
  `fail`.
- While open: a `required` entry on the request path refuses requests with 503
  `format_unavailable`; an optional one is skipped. `decide` gives no vote, response-side layers
  are skipped, running streams continue.
- There is nothing to reset by hand. The probe after the cooldown closes it, and a restart is not
  needed.

## Wrong or missing conversion

| Symptom | Cause | Fix |
|---|---|---|
| The client receives the upstream's protocol | The `convert` entry declares only request-side stages. Conversion of responses is a separate subscription | Add `"stream"` (or `"response"`) to its `stages`. `aproxy-format check` reports this case. |
| The client receives the upstream's error body unconverted | Failed responses never go through `response` or `stream`: aProxy does not know they are final until retries are given up | Add a `policy` rule with `decision = "stop"` and a `response` template, or write a `decide` program that returns a response in the client's protocol. |
| A stream ends early with an error event in the client's protocol | `convert` met a malformed upstream event or its `stream_budget_bytes` (64 MiB by default), or one reply would exceed the instance's line limit (twice `max_response_mb` plus 1 MiB) | For very long answers raise `stream_budget_bytes` and, for Responses clients, `max_response_mb`: the final Responses event repeats the whole output in one line. |
| The client gets 200 with an error body in its own protocol | A non-streaming JSON answer went through a `stream` subscription and turned out unusable after the status was committed | Read the error; usually the channel URL points at something that is not the API. |
| Tool-call arguments appear all at once at the end of a streamed tool call | Known limit, Chat upstream to a Messages or Responses client | None in configuration (examples.md, Known conversion limits). |
| An upstream with thinking enabled rejects the turn after a tool result | Known limit, Messages upstream behind a Chat client: the thinking signature cannot travel through Chat | Use a Messages-speaking client for that upstream, or turn thinking off. |
| `convert` rejects requests on a custom path | `from = "auto"` could not tell the protocol: the path is not a standard endpoint and the body fits more than one | Set `from` explicitly. |
| `convert` rejects with `to = "auto"` | The destination matches no `[[upstream]]` row and no standard endpoint path | Add a row for that origin (and `path_prefix` if one origin serves two protocols). |
| An SDK watchdog times out on a Chat or Responses client although keepalives are sent | On those paths the keepalive is an SSE comment; `heartbeat` leaves it alone because no event can safely precede the upstream's first one | Raise the client's timeout, or deliver in forward mode (`adaptive`) so that real events flow as they are produced. |
| A response header your program sets is sometimes missing | The keepalive had already committed the response headers; only the body can change after that | Do not depend on response headers for streaming requests. |

## Keys and channels

| Symptom | Cause | Fix |
|---|---|---|
| The same key is used again after a 429 | A `decide` program voted `reroute: false` for that failure (a `policy` rule with `reroute = false`, or a program of your own): aProxy then resends the request it already prepared and `keys` is not called. With no vote the retry chain reruns and `keys` rotates | Set `reroute = true` on the rule that matches those statuses, or remove `reroute` from it. |
| Every retry is refused by `keys` after a 429 | The pool has one key and it is cooling down; `keys` fails the attempt (retryable) rather than send a resting key | Add a key, shorten the cooldown, or drop 429 from `cooldown_status`. Requests resume when the cooldown ends. |
| A cooled key is used anyway, or rotation looks uneven | More than one `keys` worker: cooldowns and position are per process. Or the worker was idle-reaped and restarted | `pool_max = 1` and `worker_idle_secs = 0` on the `keys` entry. |
| `keys` rejects every request | The request's destination origin (scheme, host, port) is in no `[[pool]]` | Add a pool for it; with `route`, every channel's origin needs a pool or the channel its own `credential_env`. |
| The upstream answers 401 after adding `route` | When `route` changes the destination it removes all credential headers, including the client's, so that a key never reaches the wrong provider | Give the channel `credential_env` (and `auth_header`), or put a `keys` entry after `route` with a pool for that origin. |
| Failover never tries the other channel | The second channel's `models` do not match the model (after alias resolution), or a `decide` vote of `reroute: false` kept the retry on the prepared request (see the first row) | Check the matchers and the `policy` rules. |
| `route` or `rewrite` refuses to load a header name | `auth_header` and `keys`' `header` must be credential headers; `set_headers` in `rewrite` must not be one, nor a header aProxy manages | Move credentials to `keys` or `route`. |
| The client sees the provider's model name instead of its alias | Alias mapping on the way back is a separate, buffered `response` subscription of `route` | Add `"response"` to `route`'s `stages` for non-streaming clients. |

## Retries and delivery

| Symptom | Cause | Fix |
|---|---|---|
| Requests hang, and the log shows the same upstream status again and again | The upstream rejects what the chain sends (wrong key, model name, path or body), and aProxy retries without limit by design | Look at the upstream error in the log; rehearse with `aproxy-format try`; add a `policy` rule that stops on errors that cannot succeed. |
| A `policy` rule with `body_regex` does not fire | The pattern is matched against a preview: at most 1 MiB, head and tail of larger bodies | Match text near the start or end of the body, or match on `status` or `failure`. |
| A `decide` vote seems ignored | An earlier program in the list already voted; the first decision wins. Or the reply came after `decide_timeout_secs` and counted as no vote | Order the entries by authority; keep `decide` fast. |
| Streaming is not truly incremental | The request is delivered in retry mode, which buffers each attempt until it is judged good | Use `adaptive`, or have a `request` program reply `"delivery":"forward"`. Any entry that declares `response` also gathers the body; declare `stream`. |
| `adaptive` never switches to forward, or forgets what it learned | It is configured with several workers or is idle-reaped; or its notifications are dropped under load (`notify_dropped_total`) | `pool_max = 1`, `worker_idle_secs = 0`. Dropped notifications only slow learning. |
| `"delivery":"forward"` from a program has no effect | It was not the first program to give one, or the keepalive had already committed response headers | Put the deciding program first in the chain and keep it fast (ask for `digest`, not `full`). |

## Leftover processes

A worker must exit when stdin reaches EOF; aProxy waits two seconds and then kills it. If
processes of a format program are still running after their instance stopped, the instance was
killed before it could retire them and the program ignores EOF. Fix the read loop. End the
leftovers by PID after confirming each is the format program; never kill by image name, which
would also hit `aproxy` instances that agent sessions depend on.
