# Format protocol 1: request, retry, fail and decide

Part of [protocol.md](../protocol.md), which lists the other parts. Messages on the way upstream and the retry decision.

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
| `delivery` | `"forward"` or `"retry"`: how this request's response should be delivered ([below](control-and-errors.md#delivery-forward-and-retry)). Only read from `request` replies; the first program to give it wins. |
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
