# Runtime behavior: keepalive heartbeats

Part of [behaviors.md](../behaviors.md), which lists the other parts. What the client receives while the upstream is slow or being retried.

## Keepalive heartbeats

A streaming request can spend minutes in retries before any upstream byte is usable. Keepalive keeps
the client's connection visibly alive meanwhile: aProxy commits a response head early and then writes
a heartbeat every `keepalive.interval_secs`. By default that is the SSE comment line `: keepalive`,
which SSE parsers discard by specification; `keepalive.heartbeat` replaces it with any bytes that end
at an event boundary (config-toml.md).

### Which requests get heartbeats

All of the following must hold:

- `keepalive.interval_secs` is above 0 and the instance is not in forward-only mode;
- `keepalive.trigger` matches: `accept` means the `Accept` header contains `text/event-stream`;
  `body_stream` means the JSON body has a top-level `"stream": true`; `any` means either.

The trigger looks at the request as the client sent it, before any format program, because the
question is whether the client is waiting for a stream. Every other request (a non-streaming call, or
a streaming request that matches neither test, such as Gemini CLI's) receives nothing until its final
response is ready, so the client's own HTTP timeout must cover all retries plus generation.

### Commit point

Nothing is written before the commit, and the commit happens once per request:

| Before any commit | Result |
|---|---|
| An attempt returns a 2xx head with an uncompressed `text/event-stream` content type, and no format program declares `response`, `stream` or `decide` | The upstream's real status and headers go to the client at once (without `content-length` and hop-by-hop headers) |
| One keepalive interval passes without such a head | A skeleton head is committed (`200`, `content-type: text/event-stream`, `cache-control: no-cache`) and a heartbeat follows immediately; the first byte therefore arrives within one interval |
| An attempt succeeds first | Replayed exactly like a request without keepalive, real status and headers included |
| An attempt returns a 2xx head that is not SSE (NDJSON, plain JSON) | The skeleton commit is paused for that attempt, so the content type is not rewritten. Success is replayed unchanged; if the attempt needs a retry, the skeleton is committed right after. During such an attempt the client may receive nothing for up to `timeouts.read_idle_secs` |

A failed attempt does not trigger a commit by itself, so a request that succeeds after a few quick
retries still gets the upstream's real head.

### After the commit

- A heartbeat goes out every interval while aProxy waits for upstream headers, buffers an upstream
  body, waits between retries, or runs a format program or a decode.
- Status and headers can no longer change. The successful attempt's complete, validated body is
  written into the committed response; bytes of failed attempts never appear.
- A successful body that is not SSE is still written, since dropping a good result would be worse, and
  the log warns `Replaying a successful non-SSE body on an already committed text/event-stream response; headers cannot be changed. …`
  For an upstream that answers `"stream": true` with NDJSON and often needs
  retries, set `keepalive.trigger = "accept"` on that instance.
- A request that has to stop after the commit ends with one SSE frame: a line `event: error`, then
  `data: <error JSON>` on one line, then a blank line. Before the commit the same JSON is the whole
  response body, sent with `content-type: application/json` and the code's own status.

#### Error responses

When aProxy itself ends a request, every response has this shape:

```json
{"type":"error","error":{"type":"aproxy_<code>","message":"<one English sentence>",
 "aproxy":{"code":"<code>","request_id":"01a1e0eef745bd28-42","attempts":3,
 "upstream_status":503,"details":{}}}}
```

- Two headers carry the same values: `x-aproxy-error: <code>` and `x-aproxy-request-id: <id>`. A
  normally proxied response carries neither, so a response with `x-aproxy-error` is final: aProxy
  has given up and will not retry it. A response passed through after a retry rule stopped the
  request keeps the upstream's status and body byte for byte and gains only these two headers.
- Match on `error.aproxy.code` or on `x-aproxy-error`, never on `message`, which is a human sentence
  and not a contract. `attempts` counts the upstream attempts made, `upstream_status` appears only
  when there was an upstream response, and `details` holds the keys in the table.
- `request_id` is opaque: it looks like `01a1e0eef745bd28-42`, 16 hex digits of an instance id, a
  hyphen, and the request's sequence number in that instance. Requests refused by the inbound
  Host/Origin check end in `-x<n>` instead. It is the same string in the header, in the body, in the
  `request_id` given to format programs, and in every log line of the request, prefixed
  `request{request_id=<id>}:`.

| `code` | Status | When | `details` |
|---|---|---|---|
| `inbound_host_rejected` | 403 | `Host` is not allowed | `host` |
| `inbound_origin_rejected` | 403 | `Origin` is not allowed | `origin` |
| `request_too_large` | 413 | The request body exceeds `max_request_mb` | `limit_bytes` |
| `client_body_aborted` | 400 | The client stopped sending its request body | none |
| `local_storage_failed` | 507 | A spool file could not be written or read | `op` = `write_request`, `read_request` or `write_response` |
| `response_too_large` | 502 | The upstream response exceeds `max_response_mb` | `limit_bytes` |
| `retry_exhausted` | the upstream's, else 502 | A retry rule used its attempts up | `by` = `rule:<1-based index>`, `last_code` |
| `retry_stopped` | the response in hand, else 502 | A format's `decide` stage stopped the retries | `format`, `last_code` |
| `format_rejected` | the format's response, else 422 | A request-side format answered `fail` with `retry: false` | `format`, `stage`, `fail_code`? |
| `format_unavailable` | 503 | A required format's breaker is open at a request-side stage | `format`, `stage`, `kind`, `probe_at` (Unix ms) |
| `format_failed` | 502 | In forward mode, a request-side format failed or asked for a retry, and the request was not switched to retry mode | `format`, `stage`, `kind`?, `fail_code`? |
| `upstream_failed` | 502 | In forward mode, the upstream attempt failed, and the request was not switched to retry mode | `last_code` |
| `decode_failed` | SSE frame only | The upstream compressed the body despite `identity` and decoding failed | `encoding` |
| `upgrade_unsupported` | 501 | A WebSocket upgrade request; it is refused on the spot, not forwarded or retried. Other upgrades such as `h2c` are ignored and proxied normally | `upgrade` |
| `internal` | 500 | A defect in aProxy | none |

Why one upstream attempt failed is a separate set of codes, shown as `last_code` and in logs:
`upstream_connect_failed`, `upstream_connect_timeout`, `upstream_first_byte_timeout`,
`upstream_read_idle_timeout`, `upstream_body_interrupted`, `upstream_status`,
`upstream_error_body`, and three that come from format programs: `format_error` (the program broke),
`format_fail` (it answered `fail` with `retry: true`) and `format_decision` (its `decide` stage judged
a successful attempt as failed). The full table is `docs/error-codes.md` in the repository.

### Compression

For requests that get heartbeats, aProxy sends `accept-encoding: identity` upstream, because plain-text
heartbeats cannot be inserted into a gzip or brotli stream. This overrides the client's value, a
request-side format's value and `override_headers`; when `override_headers` sets another
`accept-encoding`, startup logs `The accept-encoding override does not apply to keepalive requests. …`
If the upstream
compresses anyway, its head is not committed, the skeleton goes out at the interval, and the
successful body is fully decoded before it is written. Requests without heartbeats keep the client's
`accept-encoding` and get the upstream's compressed bytes unchanged.

### Disconnects

Every disconnect on this path logs `Client disconnected; aborting upstream request on the keepalive channel`.
When the client drops after waiting at least 60 s on a committed response that
has carried only heartbeats, aProxy also warns `Client disconnected after receiving only heartbeats for about N seconds. …`
Unless the user cancelled, that is
the client's own stream idle timeout: many clients time SSE events rather than bytes, and comments are
not events. The fix is on the client side; see clients.md.
