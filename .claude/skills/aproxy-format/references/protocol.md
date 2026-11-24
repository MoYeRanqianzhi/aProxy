# Format protocol 1

The complete contract between aProxy and a format program. Read it when writing a program,
checking a field, or working out why aProxy rejected a reply. In the samples, `->` is a line
aProxy writes to the program's stdin and `<-` is the program's reply on stdout; each is a single
line on the wire.

## Contents

- [Transport](#transport): framing, limits, timing, process lifetime
- [hello](#hello): the handshake and what is negotiated
- [Common fields](#common-fields): `v`, `seq`, `request_id`, `state`, `digest`, `report`, HTTP objects
- [The chain](#the-chain): order, who decides, retry reruns
- [request and retry](#request-and-retry)
- [fail](#fail): rejecting on purpose
- [decide](#decide)
- [response](#response)
- [stream](#stream)
- [heartbeat](#heartbeat)
- [notify](#notify)
- [report](#report)
- [control](#control): a separate process for calls from tools and extensions
- [Delivery: forward and retry](#delivery-forward-and-retry)
- [Errors, `required` and the circuit breaker](#errors-required-and-the-circuit-breaker)
- [Security](#security)

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
  the instance started with. Messages already sent finish on the old workers, which then get EOF;
  new messages go to the new ones. A program that reads its own settings at startup picks up an
  edit this way.

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
| `control` | no | `{"args":[...]}`: offers a control process, started with these arguments instead of the configured ones ([control](#control)). At most 32 strings of up to 4096 bytes each; an empty list is allowed. A malformed offer makes the hello reply invalid. Omit it when the program offers no control. |

About `request_body`:

- `digest` is the SHA-256 of the body exactly as the client sent it (before content decoding and
  before any program changed it), 64 lowercase hex digits, in the message's `digest` field. The
  same request sent again has the same digest.
- With `none` or `digest`, the `request` object has no `body`; the reply may still patch the
  URL and headers.
- It is a performance contract and a privacy one: a program that only votes never sees the
  conversation.

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
| `report` | What the program did, for display ([report](#report)). |
| `fail` | A deliberate rejection ([fail](#fail)). |

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
- **Retries.** Programs that declare `retry` must form the tail of the chain. When the retry
  segment reruns, it starts at the first program that declared `retry`, which receives
  `stage: "retry"` with the cached output of the layer before it. Programs before that point
  handle each request once. Whether the segment reruns is decided by `reroute` in `decide`; when
  nobody votes, it reruns whenever some program declared `retry`. With `reroute: false`, aProxy
  resends the request it sent last time and calls nobody.
- If a request-direction layer itself fails (a process error, or `fail` with `retry: true`), the
  next attempt resumes at that layer with the same stage and input; earlier layers are not
  called again.

## request and retry

```json
-> {"v":1,"seq":3,"stage":"request","request_id":"19ac3f2e8b5d1a2b-42","delivery":"retry","request":{"method":"POST","url":"https://relay.example.com/v1/messages?beta=true","headers":{"content-type":"application/json","x-api-key":"sk-client"},"body":"{\"model\":\"claude-sonnet-4-5\",\"max_tokens\":64,\"stream\":true,\"messages\":[{\"role\":\"user\",\"content\":\"hi\"}]}"},"client":{"path":"/v1/messages?beta=true","stream":true}}
<- {"v":1,"seq":3,"state":"[\"anthropic_messages\",\"openai_chat\"]","request":{"url":"https://relay.example.com/v1/chat/completions?beta=true","body":"{\"model\":\"claude-sonnet-4-5\",\"messages\":[{\"role\":\"user\",\"content\":\"hi\"}],\"max_completion_tokens\":64,\"stream\":true}"},"overrides":{"timeouts":{"first_byte_secs":900}},"report":{"summary":"anthropic_messages -> openai_chat","actions":[{"type":"protocol_convert","from":"anthropic_messages","to":"openai_chat"}]}}
```

```json
-> {"v":1,"seq":9,"stage":"retry","request_id":"19ac3f2e8b5d1a2b-42","state":"[\"https://relay.example.com\",\"a\"]","delivery":"retry","request":{"method":"POST","url":"https://relay.example.com/v1/chat/completions","headers":{"content-type":"application/json"}},"client":{"path":"/v1/messages","stream":true},"retry":{"attempt":2,"last_failure":{"code":"upstream_status","details":{"status":429}},"last_response":{"status":429,"headers":{"retry-after":"30"}},"elapsed_ms":1200}}
<- {"v":1,"seq":9,"state":"[\"https://relay.example.com\",\"b\"]","request":{"headers":{"content-type":"application/json","authorization":"Bearer sk-b"}},"report":{"summary":"Selected an upstream key","actions":[{"type":"key_select","key_id":"b"}]}}
```

Message fields beyond the common ones:

| Field | Meaning |
|---|---|
| `delivery` | The request's current delivery mode, `"retry"` or `"forward"`. |
| `request` | The request as it will be sent upstream after the previous layer: header names lowercase, headers aProxy manages itself already removed. No body unless this stage's grade is `full`. |
| `client.path` | The path and query exactly as the client sent it; the same for every layer, whatever earlier layers did to the URL. Use it, not `request.url`, to tell which protocol the client speaks. |
| `client.stream` | Whether the client asked for a streaming response. **Omitted when aProxy does not know yet**, never as a stand-in for `false`: with a forward request whose body is streamed straight to the upstream, or a body that is not one valid JSON document. A program that needs the answer at `request` asks for `digest` or `full`, which makes aProxy read the body first. |
| `retry` (retry stage only) | `attempt`: the number of the attempt about to run. `last_failure`: `{"code","details"}` of the previous attempt (codes below). `last_response`: `{"status","headers"}` of the previous upstream response, without a body, with headers as the upstream sent them; omitted when no response headers arrived. `elapsed_ms`: time since aProxy received the request. Read `Retry-After` and rate-limit headers here. |

Reply fields:

| Field | Meaning |
|---|---|
| `request` | A patch. `url` must be an absolute `http` or `https` URL, or the reply is invalid output. |
| `delivery` | `"forward"` or `"retry"`: how this request's response should be delivered ([below](#delivery-forward-and-retry)). Only read from `request` replies; the first program to give it wins. |
| `overrides` | Settings for this request only, written as nested tables with config.toml's names: `{"retry":{"max_attempts":3}}`. Hello's `overrides` lists the allowed leaves in dotted form (`retry.max_attempts`); anything else, and any value of the wrong type, is ignored with a warning without spoiling the rest of the reply. A `keepalive.*` override that arrives after the keepalive has already committed the response headers has no effect. Overrides belong to this run of the program: when the retry segment reruns, the previous run's overrides are dropped and this run's replace them. |
| `fail` | Reject ([fail](#fail)). |

Attempt failure codes seen in `last_failure.code`, in `decide`'s `failure.code` and in a stream
`abort`: `upstream_connect_failed`, `upstream_connect_timeout`, `upstream_first_byte_timeout`,
`upstream_read_idle_timeout`, `upstream_body_interrupted`, `upstream_status` (`details.status`),
`upstream_error_body` (a 2xx whose body is an error), `format_error`, `format_fail`,
`format_decision`. Treat an unknown code as a generic failure; the list grows.

## fail

```json
<- {"v":1,"seq":7,"fail":{"retry":false,"code":"model_not_routed","reason":"no channel configured for model gpt-9","response":{"status":404,"headers":{"content-type":"application/json"},"body":"{\"type\":\"error\",\"error\":{\"type\":\"not_found_error\",\"message\":\"model gpt-9 is not configured\"}}"}}}
```

| Field | Meaning |
|---|---|
| `retry` | Required. `true`: this attempt failed (`format_fail`) and goes through the normal retry loop, including backoff, `decide` and retry rules. `false`: the request ends now. |
| `reason` | Required. Short English text, at most 512 characters; it is shown to the client and written to logs. |
| `code` | Optional machine-readable string of your own: 1 to 64 of ASCII letters, digits, `_`, `.`, `-`. It appears as `details.fail_code`. A code of another shape is dropped; the `fail` still applies. |
| `response` | Optional complete response for the client, in the client's protocol. |

- `retry: false` with `response`: the client receives that response with aProxy's two
  `x-aproxy-*` headers; the terminal code is `format_rejected`. Without `response`: 422
  `format_rejected`.
- On a forward request there is no retry loop: `retry: true` leads to one `decide` round about
  upgrading, and to 502 `format_failed` when nobody upgrades it.
- `fail` is a decision, not an error. It applies whether the entry is `required` or not, and it
  resets the breaker's failure count like any valid reply. `state`, `overrides` and `report` in
  the same reply still take effect.
- In `response` and at stream `open`, `fail` with `response` substitutes that response, and
  `fail` without one skips the layer (the response passes on unchanged, which includes upstream
  headers). `fail.retry` has no meaning there.
- A `fail.response` is data-plane bytes: it reaches the client as written, with no secret
  replacement. Build it from scratch rather than from the upstream's response.

## decide

```json
-> {"v":1,"seq":12,"stage":"decide","request_id":"19ac3f2e8b5d1a2b-44","client":{"path":"/v1/messages","stream":true},"attempt":6,"elapsed_ms":42000,"delivery":"retry","outcome":"failure","failure":{"code":"upstream_status","details":{"status":429}},"request":{"method":"POST","url":"https://relay.example.com/v1/chat/completions"},"response":{"status":429,"headers":{"retry-after":"60"},"body":"{\"error\":{\"message\":\"rate limited\"}}","truncated":null},"fallback":{"decision":"retry","by":"default","delay_ms":20000}}
<- {"v":1,"seq":12,"decision":"retry","delay_ms":60000,"reroute":true,"report":{"summary":"429; upstream asks 60s; switch key and wait","actions":[{"type":"custom","label":"cooldown 60s"}]}}
```

**When.** After each failed attempt of a retry-mode request. A program that listed
`decide.success` in its hello features (and the instance lists it too) is also asked after a
successful attempt, with `outcome: "success"` and no `failure`; this is how a "200 but the body
is an error" case can be judged a failure. For a forward request, `decide` is asked once, only
when the attempt failed before a single byte was written to the client, with `delivery:
"forward"` and a fallback of stop: voting `retry` upgrades the request to retry mode.

| Field | Meaning |
|---|---|
| `attempt` | The attempt that just finished. |
| `elapsed_ms` | Since aProxy received the request, including reading its body. |
| `outcome` | `"failure"` or `"success"`. |
| `failure` | `{"code","details"}`; absent on success. |
| `request` | `{"method","url"}` of the request that was sent; plus the body when this program asked for `full` on decide. |
| `response` | Present when the upstream answered. The body is decoded and capped at 1 MiB: beyond that, the first 64 KiB and the end are kept and `truncated` is the number of bytes left out. `"truncated": null` means the body is complete. When headers arrived but the body broke off, there is only `status` and `headers`, and no `truncated` key. A body that could not be decoded is given raw as `body_b64` with `content-encoding` left in the headers. |
| `fallback` | What aProxy will do if nobody votes: `decision`, `by` (where it came from), and `delay_ms`, the wait the current backoff gives. `delay_ms` is present even when the decision is stop, so a program that overrules a stop knows the baseline. |

Reply:

| Field | Meaning |
|---|---|
| `decision` | `"retry"` or `"stop"`. Omit it to abstain. |
| `delay_ms` | With retry: wait before the next attempt, an integer from 0 to 86400000 (24 h). Omitted: the fallback's delay. A value out of range makes the reply invalid; it is not clamped. |
| `reroute` | With retry: `true` reruns the retry segment (new key, new channel), `false` resends the request sent last time. Has no effect when no program declared `retry`, or when the last attempt failed inside the chain. |
| `response` | With stop: a response patch for the client. Applied on top of the upstream response when there is one, otherwise on an empty 502. |

- `stop` without `response` passes the upstream response through; when there is none, the client
  gets 502 `retry_stopped`. `stop` with `response` returns it with code `retry_stopped`.
- `retry` on a successful attempt judges it failed (`format_decision`). `stop` without
  `response` on a successful attempt confirms it; with `response` it replaces it.
- Programs are asked in list order until both `decision` and `reroute` are settled (after a stop,
  `reroute` no longer matters). Later programs are not asked: `decide` is a question, not a
  notification. A program that only wants to watch uses `notify`.
- A timeout, a crash or an invalid reply counts as abstaining (and toward the breaker). A
  `decide` error never ends a request.
- To rewrite an upstream error into the client's protocol, vote stop with a `response`; failed
  responses do not go through the `response` or `stream` stage.

## response

```json
-> {"v":1,"seq":15,"stage":"response","request_id":"19ac3f2e8b5d1a2b-42","state":"[\"anthropic_messages\",\"openai_chat\"]","client":{"path":"/v1/messages","stream":false},"request":{"method":"POST","url":"https://relay.example.com/v1/chat/completions"},"response":{"status":200,"headers":{"content-type":"application/json"},"body":"{\"id\":\"chatcmpl-1\",\"object\":\"chat.completion\",\"model\":\"gpt-4o\",\"choices\":[{\"index\":0,\"message\":{\"role\":\"assistant\",\"content\":\"Hello.\"},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":9,\"completion_tokens\":2,\"total_tokens\":11}}"}}
<- {"v":1,"seq":15,"response":{"headers":{"content-type":"application/json"},"body":"{\"id\":\"chatcmpl-1\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"gpt-4o\",\"content\":[{\"type\":\"text\",\"text\":\"Hello.\"}],\"stop_reason\":\"end_turn\",\"stop_sequence\":null,\"usage\":{\"input_tokens\":9,\"output_tokens\":2}}"}}
```

- Runs only on a response judged successful and about to be delivered. Errors passed through
  after retries were given up do not come here. For a forward request, only 2xx responses do.
- The whole body is in one message, decoded; aProxy drops the upstream's `content-encoding` and
  recomputes `content-length` on the way out.
- The reply's `response` is a patch (remember: `headers` replaces the table).
- A program cannot declare both `response` and `stream`. A program that declares `response` in
  a chain whose requests are delivered forward makes that layer gather each response first, so
  true streaming is lost there; prefer `stream`.
- On a process-level failure aProxy tries once more on a fresh worker, then skips the layer.

## stream

```json
-> {"v":1,"seq":20,"stage":"stream","event":"open","stream_id":"19ac3f2e8b5d1a2b-42/2","request_id":"19ac3f2e8b5d1a2b-42","state":"[\"anthropic_messages\",\"openai_chat\"]","client":{"path":"/v1/messages","stream":true},"request":{"method":"POST","url":"https://relay.example.com/v1/chat/completions"},"response":{"status":200,"headers":{"content-type":"text/event-stream"}}}
<- {"v":1,"seq":20,"response":{"headers":{"content-type":"text/event-stream"}},"abort_tail":"event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"api_error\",\"message\":\"stream conversion aborted\"}}\n\n"}
-> {"v":1,"seq":21,"stage":"stream","event":"data","stream_id":"19ac3f2e8b5d1a2b-42/2","data":"data: {\"id\":\"chatcmpl-1\",\"object\":\"chat.completion.chunk\",\"model\":\"gpt-4o\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"Hel\"},\"finish_reason\":null}]}\n\n"}
<- {"v":1,"seq":21,"data":"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_chatcmpl-1\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"gpt-4o\",\"content\":[],\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":0,\"output_tokens\":0}}}\n\n"}
-> {"v":1,"seq":30,"stage":"stream","event":"close","stream_id":"19ac3f2e8b5d1a2b-42/2"}
<- {"v":1,"seq":30,"data":"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"}
```

```json
-> {"v":1,"seq":31,"stage":"stream","event":"abort","stream_id":"19ac3f2e8b5d1a2b-45/1","reason":{"code":"upstream_body_interrupted"}}
<- {"v":1,"seq":31,"data":"event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"api_error\",\"message\":\"upstream stream interrupted\"}}\n\n"}
```

Only a `persistent` program can declare `stream`. One worker owns a stream from `open` until
`close` or `abort`, so it may keep parser state in memory for that time. Workers holding a
stream do not count toward `pool_max`: the number of processes is the pool size plus the number
of concurrent streams.

| Event | Message | Reply |
|---|---|---|
| `open` | `stream_id` (`<request_id>/<attempt>`), `request_id`, `state`, `client`, `request` (`method`, `url`), `response` (`status`, `headers`) | May patch `response` status and headers, give `abort_tail`, give first `data`. Once the response headers have already been sent to the client, a status or headers patch is ignored. |
| `data` | `stream_id`, `data` or `data_b64` | `data` or `data_b64`: the bytes to pass on. A reply with neither passes on nothing, which is how a program buffers until it has a whole event. |
| `close` | `stream_id` | Final `data`, if any. Sent when the upstream ended normally. |
| `abort` | `stream_id`, `reason` (`{"code"}`) | A protocol-correct error ending in `data`. Sent when the upstream broke off, an inner layer failed, or the client left. |

- **Piece borders are not event borders.** One `data` value may hold half an SSE event or
  several; keep the unfinished tail until the next piece.
- Bytes arrive decoded; aProxy removes `content-length` and `content-encoding` on output.
- `abort.reason.code` is the attempt's failure code when the upstream broke off, `format_error`
  or `format_fail` when an inner layer failed, and `client_gone` when the client disconnected
  (the reply is then discarded). Treat unknown codes as a plain abort.
- **`abort_tail`** is what aProxy writes to the client if *this* layer dies mid-stream and it is
  the outermost stream layer. As long as the outermost layer can still answer, the ending is
  whatever it returns for `abort` (nothing, if it returns no bytes). Bytes that inner layers
  return for `abort` are discarded: only the outermost layer speaks the client's protocol.
- A response with no body to deliver (HEAD, 204, 304) opens no stream.
- A non-streaming upstream response also goes through `stream` when that is the stage the
  program declared: the JSON body arrives as `data` pieces after `open`. By then the status
  and headers are committed, so the program must decide at `open` what headers the converted
  entity needs, and can only signal a late problem inside the body.
- If an optional layer fails mid-stream, later pieces skip it. If a required one does, aProxy
  stops reading the upstream and sends `abort` to the layers outside it.
- An open breaker blocks only new streams (`open`); running streams continue.

## heartbeat

```json
-> {"v":1,"seq":40,"stage":"heartbeat","request_id":"19ac3f2e8b5d1a2b-42","heartbeat":{"beat":3,"elapsed_ms":45000,"attempt":2,"last_failure":{"code":"upstream_status","details":{"status":529}}},"client":{"path":"/v1/messages","stream":true},"data":": keepalive\n\n"}
<- {"v":1,"seq":40,"data":"event: ping\ndata: {\"type\":\"ping\"}\n\n"}
```

Sent at each keepalive beat of a retry-mode request while the client waits; never for forward
requests. The innermost layer receives the instance's configured `keepalive.heartbeat` bytes,
each outer layer the output of the one inside it.

- `heartbeat`: `beat` (from 1), `elapsed_ms` since waiting began, `attempt`, and `last_failure`
  when there was one. `client` is on every beat, enough to tell the protocol.
- Reply `data` or `data_b64` to replace the bytes; omit both to keep them. `state` in the reply
  is ignored: beats run alongside other stages and would overwrite their state.
- End the output on an SSE event boundary (a blank line), and invent nothing the client's
  parser would take for model output.
- A beat must be answered before the next one. A late or failed beat sends the input bytes;
  a beat still unanswered at the next beat replaces the worker and counts toward the breaker.
  When the real response arrives, a beat in flight is abandoned and never delays it.

## notify

```json
-> {"v":1,"seq":51,"stage":"notify","event":"request_end","request_id":"19ac3f2e8b5d1a2b-42","digest":"9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08","client":{"path":"/v1/messages","stream":true},"delivery":"retry","upgraded":true,"attempts":2,"upstream_status":200,"stream_completed":true,"duration_ms":48210,"outcome":"ok","code":null}
<- {"v":1,"seq":51}
```

Sent once after a request that entered the chain has ended (delivered, client gone, or terminal
error), to each program that declared `notify`. Only `persistent` programs may declare it.

| Field | Meaning |
|---|---|
| `event` | `"request_end"`. Ignore events you do not know. |
| `state` | The program's last state for the request; the state is discarded afterwards. |
| `digest` | Only with grade `digest`; `null` when the request body was incomplete. |
| `delivery`, `upgraded` | Final delivery mode, and whether the request was upgraded from forward to retry on the way. |
| `attempts` | Total attempts. |
| `upstream_status` | Status of the last upstream response, or `null`. |
| `stream_completed` | Whether a streaming response ended normally; `null` for a non-streaming one. |
| `duration_ms` | Total lifetime of the request. |
| `outcome`, `code` | As in request records: `outcome` is `"ok"` on success; `code` is the terminal code, `null` on success. |

- Reply with the empty reply `{"v","seq"}`; nothing in it is used except `seq`.
- Notifications never slow the data path. They are sent only when the program has an idle
  worker and no data-path message is waiting, they have a fixed 2-second deadline, and they are
  dropped (oldest first) when the queue exceeds 1024 entries or 16 MiB, or while a breaker is
  open. A learning program must therefore tolerate gaps; a digest seen again soon is a useful
  second signal that the earlier request failed.
- `notify` failures count toward a separate breaker, so a broken observer path does not close
  the program's decision path.

## report

```json
"report": {"summary": "anthropic_messages -> openai_chat", "actions": [{"type": "protocol_convert", "from": "anthropic_messages", "to": "openai_chat"}]}
```

A report tells people what the program did; it appears in logs, request records and the desktop
UI. `summary` is one English sentence of at most 512 characters; `actions` holds at most 32
entries, each string cut to 256 characters. Both `summary` and `actions` are required when a
report is given. A malformed report or action is dropped with a warning and never invalidates
the rest of the reply.

| `type` | Fields |
|---|---|
| `protocol_convert` | `from`, `to` (open vocabulary: `anthropic_messages`, `openai_chat`, `openai_responses`, ...) |
| `key_select` | `key_id`: an id or alias, never the key |
| `route` | `channel`, optional `model_from`, `model_to` |
| `header_edit` | `name`, `op` (`"set"` or `"remove"`) |
| `body_edit` | `path` (JSON Pointer), `op` (`"set"` or `"remove"`) |
| `custom` | `label` (required) |

Every action may carry a `label`. An unknown `type` is displayed by its `label`. Never put
credentials, header values or body content in a report.

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
- Restarting the entry (`aproxy restart <target> --format <name>`) starts fresh workers with a
  closed breaker and a zero failure count. If their hello fails, the entry behaves as after a
  failed startup handshake: an optional entry is skipped, a required one answers 503
  `format_unavailable` until a later restart or probe succeeds.
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
