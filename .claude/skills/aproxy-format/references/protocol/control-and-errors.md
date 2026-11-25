# Format protocol 1: control, delivery, errors and security

Part of [protocol.md](../protocol.md), which lists the other parts. Control methods, forward versus retry delivery, `required` and the circuit breaker, security rules.

## control

A program that offers `control` in its hello reply can be called by tools that speak the instance
protocol, typically a desktop extension paired with it. The methods and their meaning are yours;
aProxy only carries the calls.

- **A separate process.** On the first call, aProxy starts the same `command` in the same working
  directory and environment, with the offered `args` in place of the configured ones. It is not a
  request worker: it never receives request messages, does not count toward `pool_max`, and its
  failures never touch the request breaker.
- **Its hello** has `seq` 0, `process:"persistent"`, `stages:["control"]` and `worker_id`
  4294967295. The reply must list `control` in `stages`.
- **Calls and replies**, one per line, `seq` counting up from 1. `method` matches
  `[A-Za-z0-9._-]{1,64}`; `params` is any JSON value, including null, at most 64 KiB serialized.
  Reply with exactly one of `result` (any JSON value, including null) or `fail`:

```json
-> {"v":1,"seq":1,"stage":"control","method":"settings.read","params":null}
<- {"v":1,"seq":1,"result":{"profile":"work"}}
-> {"v":1,"seq":2,"stage":"control","method":"profile.use","params":{"name":"home"}}
<- {"v":1,"seq":2,"fail":{"code":"unknown-profile","reason":"No profile named home"}}
```

- **`fail` is an answer, not an error**: the session continues and the caller receives your
  `code` and `reason`. Use it for an unknown method too.
- **One call at a time per entry**, with up to 16 waiting; the next caller is refused as busy.
  Each call must finish within 5 seconds, counting the wait, the start and the hello. A timeout,
  a wrong `seq`, an invalid or oversized reply (the limit is `limits.control`, 1 MiB) or an exit
  ends that process; the next call starts a new one.
- **Lifetime.** After 5 minutes without a call the process gets EOF. Restarting the entry or
  stopping the instance retires it the same way.
- **Redaction.** Before a reply reaches the caller, aProxy replaces the instance's known secrets
  in string values, object keys and both strings of a `fail`. Still, do not return keys: the
  caller may be an extension that shows or stores what it gets.
- **Control does not reconfigure workers.** Request workers keep running with what they loaded.
  If a call changes something they use, make them reread it (for example from a file), or have the
  user restart the entry with `aproxy restart <target> --format <name>`.

## Delivery: forward and retry

Every request has a delivery mode:

- **`retry`** (the default): aProxy reads the whole request body, buffers each response and
  replays it once it is judged good, retries failures by its rules and `decide`, and can send
  keepalive heartbeats.
- **`forward`**: the response is passed on as it arrives (true streaming). No keepalive, no
  `heartbeat` or `retry` stage, and no `decide` after a success.

The starting mode is the instance's `delivery` setting. A `request` reply can change it with
`delivery`; the first program in the chain to give one wins and later programs see it in their
input. Once response headers have gone to the client (a retry request's keepalive may commit a
200 while the chain is still running), the mode is fixed and a later `forward` is ignored. So a
program that wants to choose `forward` belongs at the front of the chain and should answer fast,
which a digest-only program does naturally.

**Upgrade within a request.** When a forward request's attempt fails before any byte was
written to the client, aProxy asks `decide` once. A `retry` vote turns the request into a retry
request on the spot (that attempt counts as the first). `stop` with a `response` returns it.
Otherwise the forward default applies: the upstream response is passed through, or the client
gets 502 `upstream_failed` (502 `format_failed` when a request-side program failed). After bytes
were written there is no upgrade; a stream that breaks midway ends as the stream stage dictates.

**Request bodies.** A request that starts in retry mode is always read in full first. A request
that starts in forward mode (instance `delivery = "forward"`) is streamed straight to the
upstream unless a program asked for `full` or `digest` at `request`, or some program declared
`decide` (the body must be kept for a possible resend).

## Errors, `required` and the circuit breaker

A **process-level failure** is: cannot start, I/O error, exit midway, timeout, protocol error
(`seq`), invalid output. It is different from `fail`, which is a valid reply.

- One failure in `request` or `retry`: for a required program the attempt fails with
  `format_error` and is retried; an optional program is skipped, its input passing on unchanged.
- Each `[[format]]` entry has a breaker per instance. Consecutive process-level failures on
  data-path messages are counted across requests and stages; any valid reply (including `fail`
  and an abstention) resets the count. At `breaker_failures` (default 5; 0 disables the breaker)
  it opens: idle workers are killed and no new message is sent to the program for
  `breaker_cooldown_secs` (default 30).

| While open | `required = true` | `required = false` |
|---|---|---|
| request, retry | The request ends at once: 503 `format_unavailable` (or the same code as a terminal SSE event), no retries | The layer is skipped; the request uses the default delivery mode |
| decide | No vote | Same |
| response, stream `open` | The layer is skipped | Same |
| heartbeat | The beat uses the input bytes | Same |
| notify | Dropped and counted | Same |

- After the cooldown one message is let through as a probe on a fresh worker. Success closes the
  breaker; failure reopens it for another cooldown.
- Network failures never trip a breaker: an unreachable upstream is still retried forever.
- Restarting the entry (`aproxy restart <target> --format <name>`) replaces its workers with fresh
  ones that start with a closed breaker and a zero failure count. If the new hello fails, nothing
  changes: the old workers, breaker and count stay as they were.
- State is visible in `aproxy status --json` under `formats[]` (`breaker`, `notify_breaker`,
  `notify_dropped_total`).
- With an optional judging program unavailable, requests follow the instance's default
  delivery. Keep that default at `retry`: then losing the judge costs only true streaming. With
  default `forward`, requests are not retried while the judge is down, and `aproxy doctor`
  warns with `forward_default_unguarded`.

## Security

- Request messages carry the client's credentials and any the instance adds. Do not log them,
  and send them nowhere but the upstream in `url`. When a program moves a request to another
  origin, remove every credential header (authorization, API keys, cookies) and attach that
  origin's own; do not carry the query string across origins unless a parameter is known to be
  needed there.
- Response headers a program returns go to the client. Never let an upstream credential header
  travel back; a `fail` without `response` on the response side passes the upstream response
  through, headers included.
- `report`, `fail.reason` and stderr are for people and logs: no keys, no header values, no
  conversation content.
- aProxy starts `command` directly with `args` as an argument vector, never through a shell.
  Put secrets a program needs in the entry's `env`, which is write-only in every aProxy read
  path, rather than in `args`.
