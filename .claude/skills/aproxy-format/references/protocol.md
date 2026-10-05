# Envelope protocol

The contract between aProxy and a format program: line framing, every envelope field in each
direction, what aProxy does with your reply, complete samples, and security notes. Read it when
you implement a format program in any language or need to know exactly what a field means.

## Framing

- An envelope is one JSON object serialized on one line, UTF-8 without a byte-order mark, ending
  in `\n`. JSON string escaping keeps newlines inside a body from breaking the frame, so do not
  escape anything yourself. aProxy tolerates a trailing `\r` on your line (Windows text-mode
  output); a blank line or a BOM is a protocol error.
- aProxy writes one envelope and waits for exactly one envelope back before it sends that worker
  anything else. There is no request id: the Nth line out answers the Nth line in. Any other
  output on stdout (a banner, a log line, a second line, pretty-printed JSON) breaks the pairing,
  so aProxy evicts the worker and fails the request (see troubleshooting.md).
- Lines can be large. The whole request body travels in one line (up to the instance's
  `max_body_mb`, default 128), and binary bodies grow by a third in base64. Read lines with a
  growable buffer, never a fixed-size one.
- Write non-ASCII characters as UTF-8 or as `\uXXXX` escapes; both are valid JSON. On Windows,
  set your stdin/stdout encoding to UTF-8 explicitly (see guide.md, language pitfalls).

## Fields

| Field | Type | Request side, in | Request side, out | Response side, in | Response side, out |
|---|---|---|---|---|---|
| `url` | string | Upstream URL aProxy computed (`base_url` + path + query) | New upstream URL; omitted = keep | Final upstream URL from the request side | Ignored |
| `method` | string | HTTP method, always present | New method; omitted = keep | **Absent** | Ignored |
| `headers` | object of string to string | Full header table, lowercase names | **Required.** Replaces the whole table | Upstream response headers | **Required.** Replaces the whole table |
| `body` | string | Body as UTF-8 text (`""` when empty) | New body as text | Same | Same |
| `body_b64` | string | Body as base64 when it is not valid UTF-8 | New body as base64 | Same | Same |
| `worker_id` | integer | Pool slot of this worker | Ignored | Same | Ignored |
| `extra` | string | The transform's `extra` setting, verbatim | Ignored | Same | Ignored |
| `error` | string | Never present | Marks this request as failed | Never present | Marks this conversion as failed |
| `stage` | string | `"request"` | Ignored | `"response"` | Ignored |
| `request_id` | string | Identifies the client request | Ignored | Same value as on the request side | Ignored |
| `state` | string | Absent (nothing earlier sets it) | Saved for the response side; omitted = keep | What the request side left, if anything | Ignored |

`stage` names the side. The presence of `method` tells them apart too (request envelopes always
carry it, response envelopes never do), and it is the only signal on aProxy 0.1.0, which sends
neither `stage`, `request_id` nor `state`; check for `method` if your program must also run there.

### Rules for your reply

| Rule | Detail |
|---|---|
| `headers` is required | A reply without it fails to parse: ``信封 JSON 解析失败: missing field `headers` `` ("envelope JSON parse failed"). `{}` is valid. |
| An omitted body means an empty body | `url` and `method` fall back to the originals when omitted, but if neither `body` nor `body_b64` is present the body becomes empty. To change only headers, write the received envelope back with your edits. |
| `body` and `body_b64` are mutually exclusive | Both present is a protocol error. |
| Types are strict | Header names and values are strings; `extra`, `stage`, `request_id` and `state` are strings; `worker_id` is a non-negative integer that fits in 32 bits. A wrong type fails the parse. |
| `null` | Allowed for `url`, `method`, `body`, `body_b64`, `error`, `stage`, `request_id` and `state` (same as omitting). Not allowed for `headers`, `extra` or `worker_id`. |
| Unknown keys | Ignored. Echoing back `worker_id`, `extra`, `stage`, `request_id` and `state` is harmless (an echoed `state` keeps the saved value). |
| `error` wins | If `error` is present (any string, even empty), aProxy ignores every other field and treats the request as failed. |

The simplest correct strategy is to modify the envelope you received and write it back whole.

## Field details

### url

On the request side `url` is where aProxy will send the request; rewriting it moves the request to
another host or path, which is the core of protocol conversion and multi-channel routing. Write an
absolute `http`/`https` URL. A malformed or unreachable URL is not caught at transform time: it
fails like a network error, and aProxy retries network errors indefinitely.

On the response side `url` is the URL the request was actually sent to, after your request-side
rewrite. The request and response transformers are separate processes with no shared memory; the
URL tells a response transformer which upstream answered, and `state` (below) can carry anything
else the request side decided.

### method

Present only on the request side. A replacement that is not a valid HTTP method token is ignored
and the original kept, with the warning `format 输出的 method 非法，沿用原方法` ("format output
method is invalid; keeping the original").

### headers

What you receive:

- Names are lowercase. Values are the client's (or upstream's) values.
- On the request side the table already includes the instance's `api_key`, `override_headers`
  and `extra_headers`; aProxy applies them before calling you.
- Hop-by-hop headers and `content-length` are removed: `connection`, `keep-alive`,
  `proxy-authenticate`, `proxy-authorization`, `te`, `trailer`, `transfer-encoding`, `upgrade`,
  `host`, `content-length`.
- A header that appears several times keeps only its first value
  (`多值头仅保留首值，后续值不进信封`, "multi-value header: only the first value enters the
  envelope"). A value that is not visible ASCII is dropped (`头值非可见 ASCII，转换器信封中丢弃`).
  LLM API requests do not depend on either case.

What you return replaces the table completely: a header you leave out is not sent. aProxy then:

- ignores any hop-by-hop header or `content-length` you return (`format 输出的头属自动管理范畴，忽略`,
  "header is managed automatically; ignored") and computes framing from the actual body;
- drops header names or values that are not valid HTTP (`format 输出的头名非法，丢弃` /
  `format 输出的头值非法，丢弃`);
- on the response side, always removes `content-length` and `content-encoding`, because the body
  you return is uncompressed;
- on the request side, sets `accept-encoding: identity` after your transform for requests that use
  the keepalive channel, so heartbeats can be inserted into an uncompressed stream.

### body and body_b64

aProxy sends `body` when the bytes are valid UTF-8 and `body_b64` otherwise. Use the same rule for
your reply. `body_b64` uses the standard base64 alphabet (`A-Z a-z 0-9 + /`) with `=` padding and
no line breaks:

| Language | Use | Not |
|---|---|---|
| Python | `base64.b64encode` / `b64decode` | `urlsafe_b64encode` |
| Go | `base64.StdEncoding` | `URLEncoding`, `RawStdEncoding` |
| Node | `Buffer.from(s, "base64")` / `buf.toString("base64")` | `"base64url"` |
| Any | single-line output | MIME-style 76-column wrapping |

Response bodies are decoded before you see them: aProxy undoes `gzip`, `deflate`, `br` and `zstd`
even though the `content-encoding` header is still in the table you receive. If decoding fails or
the decoded body would exceed 8 MiB, you receive the raw compressed bytes in `body_b64` instead.

A streaming (SSE) response arrives as one body containing the complete event stream: aProxy
buffers the whole response before calling you, so convert it as text, event by event.

### worker_id

In `persistent` mode, the pool slot of the worker that handles the request: aProxy numbers workers
`0, 1, ... pool_max - 1` as it starts them, wrapping around, and a worker keeps its number for its
lifetime. Use it as a starting offset for rotation (`(worker_id + n) % len(keys)`) so pool workers
do not all begin with the same key. Do not treat it as a unique identity: after a worker is
replaced, two live workers can share a number. In `spawn` mode it is always `0`.

### extra

The `extra` string from the transform's configuration, passed through untouched (`""` when not
set). It is the only configuration channel into your program besides `args`. aProxy does not
interpret it and does not expand `~` in it; if you pass a path, use an absolute one or expand `~`
in your program. Request and response transforms each have their own `extra`.

### stage, request_id and state

aProxy versions after 0.1.0 add these three fields so that the two sides of one client request can
cooperate.

- `stage` is `"request"` or `"response"`. Treat any other value as a stage you do not handle and
  reply with the envelope unchanged: later aProxy versions may add stages.
- `request_id` is the same string on both sides of one client request and differs between
  requests. It counts the instance's requests from 1 and restarts when the instance restarts, so
  it is unique only within one instance run. Use it to correlate your logs; it is not a secret and
  carries no meaning beyond identity.
- `state` is an opaque string aProxy keeps for the request without reading it. A request-side reply
  that includes `state` sets it; the response side then receives it. Omitting it, or replying with
  `null`, leaves the saved value unchanged. Typical use: the request side records which channel or
  key it picked (for example as a small JSON string), and the response side reads it instead of
  looking the channel up again by `url`. It travels on every envelope line of that request, so keep
  it small, and do not put credentials in it if your program logs envelopes.

### error

Reply `{"headers": {}, "error": "<reason>"}` to fail one request without failing the process. Keep
running afterwards: a persistent worker that replies with `error` stays in the pool. The reason
reaches the user:

- Request side: the client receives HTTP 502 with the body
  `请求转换失败: format 报告转换失败: <reason>（请求侧转换失败不重试，未发往上游）` ("request transform
  failed: format reported a conversion failure: <reason> (not retried, not sent upstream)"), or
  the same text in a `proxy_transform_failed` SSE error event if keepalive headers were already
  sent (see When aProxy calls you).
- Response side: the client receives the upstream response unchanged, and the daemon log records
  `响应转换失败，透传上游原始响应` ("response transform failed; passing the upstream response
  through") with the reason.

aProxy never reads your exit code. What it sees is whether a line arrived: a process that exits or
crashes without replying fails the request with `format 进程意外退出且无输出` ("format process exited
unexpectedly without output"), and your reason is lost.

## When aProxy calls you

- **Request side:** once per client request, after aProxy has buffered the body and before the
  first upstream attempt. Every retry replays the transformed request unchanged, including the key
  you chose. aProxy retries upstream 4xx and 5xx responses as well as network errors (paths in
  `bounded_retry_paths` stop after a few attempts), so a request you converted into something the
  upstream rejects is retried indefinitely rather than failing.
- **Response side:** only for the upstream response aProxy accepts as successful, just before it
  is replayed to the client. Error responses never reach you, including a final error that
  `bounded_retry_paths` passes through.
- **Keepalive:** for a request that uses the keepalive channel (a streaming request, by default),
  aProxy sends the client skeleton headers (`200`, `text/event-stream`) and heartbeats whenever a
  keepalive interval (default 15 s) passes without a result, including time spent in your request
  transform and, for SSE responses, your response transform. Once those headers are out, only your
  response `body` takes effect and header changes are ignored. A request-side failure after that
  point reaches the client as a final SSE `event: error` instead of a 502; its `error.type` is
  `proxy_transform_failed` and its `error.message` is the reason, starting with `请求转换失败: `.

## Samples

Request side, in (shown wrapped; on the wire it is one line):

```json
{"url":"https://api.anthropic.com/v1/messages","method":"POST",
 "headers":{"anthropic-version":"2023-06-01","content-type":"application/json","x-api-key":"sk-client"},
 "body":"{\"model\":\"claude-sonnet-4-5\",\"max_tokens\":100,\"messages\":[{\"role\":\"user\",\"content\":\"hi\"}]}",
 "worker_id":2,"extra":"/home/me/.aproxy/agg.toml","stage":"request","request_id":"42"}
```

Request side, out (moved to another endpoint with a channel key; body rewritten; the chosen channel
saved for the response side):

```json
{"url":"https://relay.example.com/v1/chat/completions","method":"POST","headers":{"authorization":"Bearer sk-relay-1","content-type":"application/json"},"body":"{\"model\":\"gpt-4o\",\"max_tokens\":100,\"messages\":[{\"role\":\"user\",\"content\":\"hi\"}]}","state":"{\"channel\":\"relay\"}"}
```

Response side, in (no `method`; `url` is the rewritten request URL):

```json
{"url":"https://relay.example.com/v1/chat/completions",
 "headers":{"content-type":"application/json","x-request-id":"req_1"},
 "body":"{\"id\":\"chatcmpl-1\",\"object\":\"chat.completion\",\"choices\":[...]}",
 "worker_id":0,"extra":"/home/me/.aproxy/agg.toml",
 "stage":"response","request_id":"42","state":"{\"channel\":\"relay\"}"}
```

Failure, either side:

```json
{"headers":{},"error":"model gpt-nope matches no channel"}
```

## Security

- The request envelope carries the client's credentials (`authorization`, `x-api-key`, cookies)
  and, if the instance sets `api_key` or credential headers, those too. Do not log them, and do
  not send them anywhere except the upstream in `url`. When you switch a request to another
  upstream, build the credential headers from scratch rather than editing the client's table, so
  the client's key never reaches a third party.
- Response headers you return go straight to the client. If your request side injected an
  upstream key, make sure no credential header comes back on the response side; the official
  aproxy-format removes `authorization`, `x-api-key`, `cookie` and `proxy-authorization` from
  responses for this reason.
- aProxy starts `command` directly with `args` as an argument vector, never through a shell, so
  configuration values cannot inject shell commands. Your program should apply the same care to
  anything it runs.
