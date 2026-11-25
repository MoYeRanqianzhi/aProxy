# Format protocol 1: transport, hello and the chain

Part of [protocol.md](../protocol.md), which lists the other parts. How a program is started and talks to aProxy: transport, `hello`, common fields, the chain of programs.

## Transport

- **Frame.** One JSON object per line, UTF-8, ended by `\n`. Arrays and scalars are not frames.
  Vocabulary values (`decision`, `delivery`, `process`, `request_body` grades) are JSON strings.
- **One reply per message, in order, echoing `seq`.** `seq` starts at 0 for each process and
  increases by one. A reply with a missing or different `seq` is a protocol error and the worker
  is replaced; this is what keeps replies from ever being paired with the wrong request.
- **Null means omitted** in a reply: `{"body":null,"body_b64":"..."}` gives only `body_b64`.
  In messages from aProxy, `null` appears only where this document says so.
- **Reply size.** A reply line longer than the limit is treated as invalid output without being
  read to the end. The hello reply may be 64 KiB; `request` and `retry` replies twice
  `max_request_mb` plus 1 MiB; `response` and `stream` replies twice `max_response_mb` plus 1 MiB
  (no limit when the setting is unlimited); every other reply 1 MiB. The instance's actual
  numbers arrive in hello's `limits`.
- **Deadlines** run from the moment aProxy writes the message: `timeout_secs` (default 30) per
  message, `decide_timeout_secs` (default 2) for `decide`, a fixed 2 seconds for `notify`, the
  time to the next beat for `heartbeat`, at least 10 seconds for hello. A worker that misses a
  deadline is replaced, because a late reply would be paired with the next message.
- **stderr** goes to the instance log, line by line, prefixed `format[<name>]`, after known
  secrets are replaced. Lines are cut at 8 KiB and limited to 50 per second per program.
- **Lifetime.** Every process, in either mode, reads `hello` first. A `persistent` worker then
  answers messages until stdin reaches EOF and must exit on EOF (aProxy waits up to 2 seconds
  before killing it). A `spawn` process gets `hello`, one stage message, then EOF; up to
  `pool_max` such processes run at once. Both modes are the same loop: read a line, write a line,
  until EOF. A partial line before EOF is not a frame.
- **Restart without restarting the instance.** `aproxy restart <target> --format <name>` (or the
  same request from the desktop window) starts fresh workers for an entry, using the configuration
  the instance started with. The new workers must pass hello first; only then do they take over.
  Messages already sent finish on the old workers, which then get EOF, and later messages of the
  same request go to the new ones, so keep per-request facts in `state`. If the new hello fails,
  nothing is replaced. A program that reads its own settings at startup picks up an edit this way.

## hello

```json
-> {"v":1,"seq":0,"stage":"hello","protocols":[1],"aproxy_version":"0.2.0","name":"adaptive","process":"persistent","stages":["request","decide","notify"],"worker_id":0,"features":["decide.success"],"limits":{"hello":65536,"request":68157440,"response":269484032,"control":1048576},"overrides":["timeouts.connect_secs","timeouts.first_byte_secs","timeouts.read_idle_secs","keepalive.interval_secs","keepalive.trigger","keepalive.heartbeat","retry.max_backoff_secs","retry.max_attempts"]}
<- {"v":1,"seq":0,"protocol":1,"stages":["request","decide","notify"],"request_body":{"request":"digest","decide":"none","notify":"digest"},"features":[],"name":"adaptive","version":"1.0.0"}
```

The shape of this line never changes, and its `v` is always 1.

| Field (from aProxy) | Meaning |
|---|---|
| `protocols` | Versions the instance speaks. Pick one and return it in `protocol`. |
| `aproxy_version` | For diagnostics. |
| `name` | The `[[format]]` entry's `name`. |
| `process` | `"spawn"` or `"persistent"`. |
| `stages` | The stages this entry is configured for. |
| `worker_id` | Unique among this program's live workers, fixed for the life of the process; always 0 in spawn mode. Workers holding a stream are not counted in `pool_max`, so the number can reach `pool_max` or more. |
| `features` | Optional behaviors the instance supports. |
| `limits` | Optional. Largest reply line, in bytes including the newline, per group: `hello`, `request` (request and retry replies), `response` (response and stream replies), `control` (all others). A missing key means no limit. |
| `overrides` | The config keys a reply's `overrides` may set for one request. |

| Field (your reply) | Required | Meaning |
|---|---|---|
| `v`, `seq` | yes | `1` and `0`. |
| `protocol` | yes | The chosen version: `1`. Every later line carries it in `v`. |
| `stages` | yes | Every stage the program implements, not only those configured. A configured stage missing from this list stops the instance from starting (`format_stage_unsupported`). |
| `request_body` | no | Per stage, how much of the client's request body to send: `"none"`, `"digest"` or `"full"`. Only `request`, `retry`, `decide` and `notify` are read; other keys are ignored. Defaults: `full` for request and retry, `none` for decide and notify. `notify` cannot be `full`. An unknown value makes the hello reply invalid. |
| `features` | yes | Features the program wants; a feature is active only when both sides list it. Protocol 1 defines `decide.success`. |
| `name`, `version` | yes | The program's own name and version, shown in diagnostics. |
| `control` | no | `{"args":[...]}`: offers a control process, started with these arguments instead of the configured ones ([control](control-and-errors.md#control)). At most 32 strings of up to 4096 bytes each; an empty list is allowed. A malformed offer makes the hello reply invalid. Omit it (or send null) when the program offers no control. |

About `request_body`:

- `digest` is the SHA-256 of the body exactly as the client sent it (before content decoding and
  before any program changed it), 64 lowercase hex digits, in the message's `digest` field. The
  same request sent again has the same digest.
- With `none` or `digest`, the `request` object has no `body`; the reply may still patch the
  URL and headers.
- It is a performance contract and a privacy one, for the client's request body only. With `none`
  a program never receives what the client sent, but responses still reach it: `decide` carries the
  upstream response body (up to 1 MiB; on a success too, with `decide.success`), and `response` and
  `stream` carry the whole answer.

If a line your reply must fit in one frame cannot fit under `limits` (a stream ending that
repeats all output, for example), end with an in-protocol error for the client instead of
sending a line aProxy will drop and count against the worker.

At startup aProxy shakes hands with every entry once, in parallel:

| Failure | `required = true` | `required = false` |
|---|---|---|
| A configured stage is not in the reply | Instance does not start, `format_stage_unsupported`, exit code 3 | Same |
| Wrong protocol version, invalid hello reply | Instance does not start, `format_handshake_failed`, exit code 3 | Same |
| Cannot start, no reply in 10 s, exits midway | Instance does not start, `format_handshake_failed`, exit code 1 | Warning; the entry starts with its breaker open and is probed after the cooldown |

## Common fields

From aProxy, on every stage message: `v`, `seq`, `stage`, `request_id` (shared by the whole
chain), `state` when this program stored one for this request, and `digest` when the program
asked for it on this stage. Exception: the `data`, `close` and `abort` events of a stream carry
only `stream_id`.

In every reply: `v`, `seq`, and optionally:

| Field | Meaning |
|---|---|
| `state` | A string of at most 64 KiB kept per program and request. A string replaces it, `""` clears it, omission keeps it. It comes back on every later message of the request, whichever worker receives it. Ignored in heartbeat replies. |
| `report` | What the program did, for display ([report](response-stages.md#report)). |
| `fail` | A deliberate rejection ([fail](request-stages.md#fail)). |

**HTTP objects.** A request is `{"method","url","headers":{lowercase-name: value},"body"|"body_b64"}`;
a response is `{"status","headers","body"|"body_b64"}`.

- `body` holds UTF-8 text; `body_b64` holds any bytes in standard base64 with padding. Never both.
- Objects from aProxy always carry `headers` (`{}` when empty). Only the first value of a
  repeated header is kept, except `content-encoding`, whose values are joined with `, `.
- In `decide`, `response` and `stream` messages, `request` is only `{"method","url"}`: no
  headers, and a body only for a program that asked for `full` on `decide`.
- The same-named object in a reply is a **patch**: an omitted field is unchanged; `headers`,
  when given, replaces the whole table; omitting both `body` and `body_b64` keeps the body, and
  `"body":""` empties it.
- Bodies reach programs decoded. When any program asks for `full`, aProxy decodes a
  `content-encoding` it knows and removes that header; the upstream receives the decoded body.
  `content-length` is always recomputed by aProxy. A `content-encoding` you set yourself is sent
  as written: you answer for those bytes.

**Byte streams** (`stream`, `heartbeat`) use `data` (UTF-8) or `data_b64`. aProxy never splits a
UTF-8 sequence across two `data` values. Messages always carry one of the two (`"data":""` for
an empty piece); only replies may omit both.

## The chain

- `request` and `retry` run in list order; `response`, `stream` and `heartbeat` in reverse
  order; `decide` in list order. Each stage calls only the programs that declared it, and one
  layer's output is the next layer's input.
- **Decisions** (`delivery` in a request reply; `decision` and `reroute` in a decide reply) are
  taken from the first program in list order that gives one. **Content** (request and response
  patches) accumulates layer by layer.
- `state` is private to one program and one request. Programs cannot read each other's state.
- Messages of one request, and of different requests, may reach different workers. A stream is
  the exception: one worker handles it from `open` to `close` or `abort`.
- **Retries.** Among the programs that serve `request`, those that declare `retry` must form the
  tail: once an entry declares `retry`, every later entry that declares `request` must declare
  `retry` too. Entries without `request` (response, stream, decide, heartbeat or notify only) are
  not bound by this. When the retry segment reruns, it starts at the first program that declared `retry`, which receives
  `stage: "retry"` with the cached output of the layer before it. Programs before that point
  handle each request once. Whether the segment reruns is decided by `reroute` in `decide`; when
  nobody votes, it reruns whenever some program declared `retry`. With `reroute: false`, aProxy
  resends the request it sent last time and calls nobody.
- If a request-direction layer itself fails (a process error, or `fail` with `retry: true`), the
  next attempt resumes at that layer with the same stage and input; earlier layers are not
  called again.
