---
name: aproxy-format
description: Write, test and configure format programs - the external transformers aProxy runs for request_transform, response_transform and heartbeat_transform - and use the official aproxy-format binary. Covers the one-line JSON envelope protocol field by field, spawn vs persistent mode, protocol conversion between OpenAI Chat, OpenAI Responses and Anthropic Messages, API key rotation, multi-channel aggregation by model (newapi style), testing a format without aProxy, and diagnosing 502s caused by a transformer. Use this skill whenever a request must be rewritten on its way through aProxy or a response rewritten on its way back, even if the user never says "format" - for example "point this instance at an OpenAI-compatible endpoint", "convert Anthropic requests to OpenAI", "rotate between these three keys", "route claude models to one channel and gpt models to another", "my transformer makes every request 502". For aProxy commands and the rest of config.toml use the aproxy-cli skill.
---

# aProxy format programs

A format program is a filter that aProxy runs as a child process. For each request (or each
successful upstream response) aProxy writes **one line of JSON**, the envelope, to the program's
stdin; the program writes **one envelope line** back to stdout. The envelope carries the method,
URL, headers and body, and everything the program writes back replaces what aProxy sends. That
one mechanism is enough for protocol conversion, key rotation and multi-channel routing.

- `request_transform` runs once per client request, after aProxy has buffered the body and before
  the first upstream attempt. Every retry replays the transformed request, so a rotated key stays
  the same across that request's retries and changes only on the next request.
- `response_transform` runs on the upstream response aProxy has judged successful, just before
  replaying it to the client. Its envelope `url` is the final upstream URL from the request side.
  The two transformers are separate processes with no shared memory: the URL tells a response
  transformer which channel answered, and anything else the request side decided can be handed
  over in the envelope's `state` field (references/protocol.md).

## First, check whether the official binary already does it

`aproxy-format` converts between OpenAI Chat, OpenAI Responses and Anthropic Messages, rotates
keys, and routes models to channels from a TOML file passed in `extra`. It is a separate download
with its own version line (not installed by `aproxy install`); installation and configuration are
in [references/examples.md](references/examples.md). Reach for it before writing code. Its limits
decide whether it fits:

- Cross-protocol conversion works for non-streaming responses only. A streaming (SSE) response
  from a channel with a different protocol cannot be converted; the response transform fails and
  aProxy passes the upstream stream through unchanged, which the client will not understand.
- With `client_format = "auto"` it only routes a request to a channel of the same protocol. To
  convert across protocols, declare the client's protocol explicitly.

## Writing your own

1. **Pick a mode.** `spawn` (the default) starts a fresh process per request: any script that reads
   a line and prints a line works, but transformations on one instance then run one at a time.
   `persistent` keeps a pool of worker processes (up to `pool_max`, default 4) that loop over
   lines. Use `persistent` whenever the program keeps state between requests (a rotation counter,
   a cache) or the instance handles concurrent traffic.
2. **Start from a loop template** in [references/guide.md](references/guide.md) for your language,
   and keep to the protocol duties below.
3. **Test it without aProxy:** `python scripts/test_format.py` feeds your program request- and
   response-side envelopes for each protocol, checks the replies, and checks that a persistent
   program exits on EOF. See [references/guide.md](references/guide.md) for usage.
4. **Configure it** in the instance's config.toml (`request_transform` / `response_transform`;
   field reference in the aproxy-cli skill, config-toml.md) and `aproxy restart` the instance.

## Protocol duties and why they matter

aProxy matches each reply to its request purely by order: the Nth line out answers the Nth line
in, and it judges a worker only by whether a valid reply line arrives (it never reads exit codes).
Most rules follow from that; guide.md, "Protocol duties", lists what each violation costs.

- **Read stdin one line at a time.** aProxy keeps stdin open between requests, so reading all of
  stdin (`sys.stdin.read()`, `json.load(sys.stdin)`) never returns and the request times out.
- **Write exactly one line per request, then flush.** An extra line (a banner, a debug print,
  pretty-printed JSON, `jq` without `-c`) breaks the pairing, so aProxy treats it as a protocol
  error: the worker is evicted and, on the request side, the request fails. Unflushed output
  makes aProxy wait until the transform timeout (default 30 s).
- **Log to stderr.** aProxy discards it, which is exactly why stdout must carry nothing else.
- **Write the whole envelope back.** `headers` is the only required field (a reply like
  `{"body": "..."}` fails to parse), but whatever you omit is not "unchanged": a reply without
  `body` or `body_b64` sends an empty body. Start from the envelope you received and modify it.
- **The headers you return are the headers sent.** aProxy hands you the full table with lowercase
  names and replaces it with yours, so rotating a key is just rewriting `authorization`. Leave out
  `content-length` and `transfer-encoding`; aProxy computes framing itself and ignores yours.
- **Use `body` for UTF-8 text and `body_b64` otherwise** (standard alphabet with `=` padding; the
  URL-safe variant fails to decode). Never both. Response bodies arrive already decompressed.
- **Report a per-request failure as `{"headers":{},"error":"reason"}` and keep running.** A program
  that crashes or exits instead fails the request without a reason and costs a process restart.
- **In persistent mode, exit when stdin reaches EOF.** aProxy kills the workers it retires, but if
  the aProxy process itself ends without cleaning up, the closed stdin is the only signal your
  worker gets; a program that ignores it keeps running as an orphan.

What a failure costs differs by side, so know which side you are on:

| Side | When the transform fails (error reply, crash, timeout, bad output) |
|---|---|
| Request | Nothing is sent upstream and the request is not retried. The client gets a 502 with the reason, or, on a streaming request that has already received heartbeat headers, a final SSE error event of type `proxy_transform_failed`. |
| Response | aProxy replays the original upstream response unchanged and logs a warning. |

A transform that succeeds but produces a request the upstream rejects (a wrong conversion, a revoked
key) is a different story: aProxy retries upstream errors indefinitely, with the same transformed
request each time, so the client just waits. Check conversions with the test harness and one real
request before relying on them.

## Where to look

| You need | Read |
|---|---|
| Exact envelope fields: direction, required, encoding, samples, security notes | [references/protocol.md](references/protocol.md) |
| Loop templates, mode choice, testing, wiring it into aProxy | [references/guide.md](references/guide.md) |
| Complete key-rotation, protocol-conversion and aggregation setups | [references/examples.md](references/examples.md) |
| A 502 or log message that names the transformer | [references/troubleshooting.md](references/troubleshooting.md) |
| The test harness | [scripts/test_format.py](scripts/test_format.py) |
