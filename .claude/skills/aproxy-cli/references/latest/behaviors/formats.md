# Runtime behavior: format programs

Part of [behaviors.md](../behaviors.md), which lists the other parts. How an instance runs the `[[format]]` programs: stages, workers, failures.

## Format programs

A `[[format]]` entry names an external program that aProxy talks to over stdin and stdout, one JSON
line per message and one JSON line back. This section is aProxy's side of that; the message fields
and how to write a program are in the aproxy-format skill, and the configuration keys are in
config-toml.md, "format" (per instance in config.toml only: no settings.json default, no CLI flag).

```
request direction:   client -> format[0] -> format[1] -> ... -> upstream
response direction:  client <- format[0] <- format[1] <- ... <- upstream
```

- **A chain, in list order.** Every program that declares a stage takes part in it; the output of
  one is the input of the next. Each program can keep a small opaque `state` per request, which
  aProxy stores and hands back to that same program at later stages, even on another worker.
- **Handshake.** Each process starts with a `hello` exchange that fixes the protocol version, the
  stages the program supports and how much of the request body it wants (`full`, `digest` or
  `none`). A program that only judges never receives the conversation's content.
- **`request` and `retry`.** A request-side program may rewrite method, URL, headers and body;
  rewriting the URL is how routing and protocol conversion change host and path. aProxy applies
  `api_key`, `override_headers` and `extra_headers` first, so programs see them, and their output
  headers are final except `accept-encoding` on keepalive requests (see Compression). The chain
  runs once per request. Before a retry, the programs that declare `retry` run again only when the
  retry chain is rerun: a `decide` program asks for that with `reroute: true`, and it is the
  default when nobody says otherwise. With `reroute: false` aProxy resends the request it prepared
  last time, so a key-rotating program is not consulted and does not rotate.
- **`decide`.** After each failed attempt of a `retry` request aProxy asks the programs that
  declare `decide`, in order, until both the decision (`retry` or `stop`) and `reroute` are
  settled. A program may also set the wait, and a `stop` may carry a response for the client
  (`retry_stopped`). A program that asked for it is also consulted after successful attempts, to
  reject a "200 that is really an error". No answer, a late answer or a crash counts as no
  opinion: the retry rules and the default backoff then apply, so a broken decision program never
  ends a request.
- **`response` and `stream`.** Only a response judged successful and about to be delivered goes
  through them. `response` receives the whole decoded body. `stream` receives the response as
  `open`, `data` chunks, then `close` or `abort`, on one worker held for the whole stream, and can
  emit converted bytes as they flow: on a `forward` request truly incrementally, on a `retry`
  request while the buffered response is replayed. aProxy drops `content-length` and
  `content-encoding` from the output, since the bytes changed. Once the response head has been
  committed (by a heartbeat skeleton, for example) a program's changes to status and headers are
  ignored and only its body is used.
- **`heartbeat`.** On every keepalive tick the programs receive the bytes about to be sent and may
  replace them. A tick is never delayed: a reply that is late, failed or malformed sends the input
  bytes instead, and once the real response is ready pending ticks are abandoned.
- **`notify`.** After a request ended, each program that declares `notify` gets one message with
  the outcome (delivery mode, attempts, last upstream status, whether the stream completed).
  aProxy does not wait on the data path for the answer; under load notifications are dropped and
  counted (`notify_dropped_total` in the status snapshot).
- **Delivery per request.** The first program whose `request` reply names a `delivery` decides
  whether this request is handled in `retry` or `forward` mode; see Forward-only mode.
- **Failures and the breaker.** A deliberate `fail` reply is obeyed: `retry: false` ends the
  request with `422` `format_rejected` or the response the program supplied; `retry: true` fails
  the attempt (`format_fail`) and it is retried. A program that breaks (cannot start, exits, times
  out, replies with something invalid) is a different matter and depends on `required`: on the
  request side a required program fails the attempt (`format_error`, retried), an optional one is
  skipped; on the response side, at `decide`, `heartbeat` and `notify`, any broken program is
  skipped. `breaker_failures` such failures in a row open the program's breaker: its idle workers
  are killed and no message is sent to it until a probe after `breaker_cooldown_secs` succeeds.
  While a required program's breaker is open, requests that need it on the request side end at
  once with `503` `format_unavailable`. If a keepalive response was already committed, any of
  these errors reaches the client as the terminal SSE frame instead of a status (see After the
  commit).
- **Keepalive.** On keepalive requests the programs run within the heartbeat cadence, so a slow
  one does not leave the client without bytes: if nothing has been committed when the interval
  ends, the skeleton head goes out and heartbeats continue. When any program declares `response`,
  `stream` or `decide`, aProxy never commits the upstream's real head early, because a program may
  still change or reject the response.
- **Process pool.** `process = "spawn"` starts one process per message. `process = "persistent"`
  keeps workers that handle one message at a time each, grows to `pool_max` (beyond that, messages
  queue), retires workers idle for `worker_idle_secs`, and kills a worker that exceeds its time
  limit. Workers that own a stream are outside `pool_max`. When the instance exits, workers see
  end of input on stdin and are expected to exit; aProxy kills the ones that do not.
- **Output discipline.** A worker answers each message with exactly one line on stdout, echoing the
  message's sequence number. A missing or wrong number, an extra line, invalid JSON or output while
  idle gets the worker replaced; otherwise one request could read another's reply, and with it
  another session's body or key. stderr is copied to the instance log with known secrets replaced.
- **State across requests** (key cooldowns, learned decisions) lives in the program's own process.
  Such a program needs `process = "persistent"`, `pool_max = 1` and `worker_idle_secs = 0`, and
  loses that state when the instance restarts.
- **Not sent through `response`/`stream`:** failures passed through because a retry rule gave up,
  and non-2xx responses of forward requests.
- Apply configuration changes with `aproxy restart <port|alias>`. `aproxy doctor` checks the chain
  the way `start` would; `aproxy-format check <config.toml>` checks the official roles' own files.
