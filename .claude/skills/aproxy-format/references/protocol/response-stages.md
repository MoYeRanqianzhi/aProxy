# Format protocol 1: response, stream, heartbeat, notify and report

Part of [protocol.md](../protocol.md), which lists the other parts. Messages on the way back to the client and the notifications.

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
