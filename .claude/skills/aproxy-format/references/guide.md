# Writing a format program

How to build a format program that aProxy can run: choosing spawn or persistent mode, the duties
every program has, loop templates, testing without aProxy, and wiring the program into an
instance. Read it before writing a new program or when adapting one to aProxy. Field-level rules
for the envelope are in protocol.md; complete working programs are in examples.md.

## Contents

- [Choose a mode](#choose-a-mode)
- [Protocol duties](#protocol-duties)
- [Loop templates](#loop-templates)
- [Language pitfalls](#language-pitfalls)
- [Techniques](#techniques)
- [Test without aProxy](#test-without-aproxy)
- [Configure it in aProxy](#configure-it-in-aproxy)

## Choose a mode

| | `spawn` (default) | `persistent` |
|---|---|---|
| Process lifetime | One process per conversion; aProxy kills it after reading the reply | A pool of long-lived workers, each looping over lines |
| Concurrency | One conversion at a time per transform (request and response each) | Up to `pool_max` (default 4) at once; further requests wait for a free worker |
| State between requests | None: every request sees a fresh process | Kept in each worker's memory |
| `worker_id` | Always `0` | Slot number `0 .. pool_max - 1` |
| Cost per request | A process start | A pipe round trip |

Use `persistent` when the program keeps state (a rotation counter, a loaded config file, a cache)
or when the instance serves concurrent requests; use `spawn` for a quick stateless script on a
lightly used instance.

How the persistent pool behaves, so you can reason about your program's lifetime:

- Workers start on demand, not at instance start, and an idle worker is reused before a new one is
  started. Requests beyond `pool_max` queue; the time spent queueing does not count toward
  `timeout_secs`.
- A worker idle for `idle_timeout_secs` (default 300; `0` = never) is killed. State held in
  memory is lost with it, so a rotation counter restarts from the beginning.
- A worker that times out, breaks the protocol, or dies is killed and removed; the next request
  starts a fresh one.
- If an idle worker turns out to be dead when reused (it exited while idle), aProxy retries that
  request once on a fresh worker. If the fresh worker also dies without replying, the request
  fails; aProxy does not keep restarting a program that cannot start.

Write the loop form regardless of mode. A program that loops until EOF also works in spawn mode,
where aProxy simply kills it after one reply.

## Protocol duties

| Duty | If you break it |
|---|---|
| Read stdin one line at a time; one line is one envelope | aProxy keeps stdin open between requests, so a program that reads all of stdin (`sys.stdin.read()`, `json.load(sys.stdin)`) never gets past the read; the request times out. |
| Write exactly one envelope line per request, and nothing else, to stdout | Protocol error: the worker is evicted and the request fails. |
| Flush stdout after every reply | aProxy waits until `timeout_secs` (default 30), kills the worker, and the request fails with a timeout. |
| Send logs and diagnostics to stderr | aProxy discards stderr, so this is safe; on stdout they break the pairing. |
| Always include `headers` in a reply, and write the body back even if unchanged | A missing `headers` fails the parse; a missing body sends an empty body. |
| Report a per-request failure as `{"headers":{},"error":"..."}` and keep running | A crash or exit without a reply fails the request with no reason and costs a process restart. |
| Exit when stdin reaches EOF | aProxy normally kills its workers itself, but if the aProxy process dies without cleaning up (crash, forced stop), EOF is the only signal your worker gets; a program that ignores it keeps running as an orphan. |
| Catch errors around the conversion, not just around parsing | An unhandled exception is a crash (see above). |

The exact messages each violation produces are in troubleshooting.md.

## Loop templates

Each template echoes the envelope unchanged. Put your conversion in `transform`; the presence of
`method` tells you which side you are on (see protocol.md).

### Python

```python
#!/usr/bin/env python3
import json
import sys

# Windows pipes default to the system code page; force UTF-8 both ways.
sys.stdin.reconfigure(encoding="utf-8")
sys.stdout.reconfigure(encoding="utf-8")


def transform(env: dict) -> dict:
    return env


for line in sys.stdin:  # the loop ends at EOF and the program exits
    try:
        reply = transform(json.loads(line))
    except Exception as e:
        reply = {"headers": {}, "error": f"{type(e).__name__}: {e}"}
    sys.stdout.write(json.dumps(reply, ensure_ascii=False) + "\n")
    sys.stdout.flush()
```

### Node.js

```javascript
#!/usr/bin/env node
const readline = require('readline');

function transform(env) {
  return env;
}

const rl = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
rl.on('line', (line) => {
  let reply;
  try {
    reply = transform(JSON.parse(line));
  } catch (e) {
    reply = { headers: {}, error: String(e) };
  }
  process.stdout.write(JSON.stringify(reply) + '\n');
});
// No exit handler needed: when stdin closes, Node exits after pending writes drain.
```

Both templates pass `scripts/test_format.py`, including the EOF check.

### Rust

The `aproxy-envelope` crate (crates.io) is the envelope type aProxy itself uses, so parsing,
serialization and the `body`/`body_b64` checks match aProxy exactly.

```rust
// Cargo.toml: aproxy-envelope = "0.1"
use std::io::{BufRead, Write};

use aproxy_envelope::TransformEnvelope;

fn transform(env: TransformEnvelope) -> Result<TransformEnvelope, String> {
    // body_bytes() decodes body or body_b64; set env.body or env.body_b64 on the way out.
    Ok(env)
}

fn main() {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break }; // EOF or a broken pipe ends the loop
        let reply = TransformEnvelope::from_line(line.trim_end())
            .map_err(|e| e.to_string())
            .and_then(transform)
            .unwrap_or_else(|reason| TransformEnvelope {
                error: Some(reason),
                ..Default::default()
            });
        let text = reply
            .to_line()
            .unwrap_or_else(|_| r#"{"headers":{},"error":"serialize failed"}"#.to_string());
        let _ = writeln!(out, "{text}");
        let _ = out.flush();
    }
}
```

### Other languages

Any language works the same way: read a line with a growable buffer (envelope lines can be many
megabytes), parse it with a JSON library, write one compact line, flush. In C or C++ use
`getline` rather than a fixed `fgets` buffer, and remember that C/C++ have no standard base64 for
`body_b64`.

## Language pitfalls

| Language | Pitfall | Fix |
|---|---|---|
| Python on Windows | Pipes use the system code page (GBK and the like): non-ASCII text is garbled or raises `UnicodeEncodeError` | `reconfigure(encoding="utf-8")` on stdin and stdout, as in the template |
| Python | `print` without `flush=True`, or `sys.stdout.write` without `flush()` | Flush after each reply |
| bash + jq | `jq` pretty-prints over several lines by default | Always `jq -c` |
| Node.js | Mixing `console.log` debugging with replies on stdout | `console.error` for diagnostics |
| Go | `bufio.Scanner` stops at lines over 64 KiB | `scanner.Buffer(make([]byte, 0, 1<<20), 512<<20)` or `bufio.Reader.ReadString('\n')` |
| Go, Rust, C | Buffered writer never flushed | Flush after each line |
| PowerShell | `ConvertTo-Json` is multi-line by default and truncates nesting at depth 2 | `ConvertTo-Json -Compress -Depth 100`; set `[Console]::OutputEncoding` to UTF-8 |
| Any | A library or runtime prints a banner or warning on stdout at startup | Silence it or redirect it to stderr; the first line aProxy reads must be your reply |

## Techniques

**One program for both sides.** Branch on whether `method` is present and configure the same
command for `request_transform` and `response_transform`. Each side still runs in its own process
pool, so the two halves cannot share memory; carry anything the response side needs in the URL
(the response envelope's `url` is your request-side URL) or derive it from configuration.

**Pass settings through `extra`.** JSON is the most flexible encoding. In TOML, a single-quoted
literal string avoids escaping: `extra = '{"keys": ["sk-a", "sk-b"]}'`. For anything long or
secret, put a file path in `extra` and read the file once per worker. Keep secrets out of `args`:
command lines are visible to other processes on the machine.

**Rewrite the URL, keep the path when it matters.** To change only the host, replace the scheme
and host and keep the rest of `url`, so every endpoint the client calls (for example
`/v1/messages/count_tokens` beside `/v1/messages`) keeps its own path. Replacing the whole URL
sends every request to the same endpoint.

**Spread rotation across workers.** Start each worker's key index at `worker_id` (examples.md, Key
rotation).

**SSE bodies.** A streaming response arrives as the complete event text. Split it on blank lines
into events, convert each `data:` payload, and join the result with the same framing. Keep the
`content-type: text/event-stream` header.

**Binary bodies.** Check for `body_b64` before assuming `body` exists. Return binary output in
`body_b64`.

## Test without aProxy

`scripts/test_format.py` (in this skill) starts your program, sends one mock envelope, prints the
reply, then closes stdin and checks that the program exits. The mock bodies are realistic
Anthropic Messages, OpenAI Chat and OpenAI Responses payloads with non-ASCII text, so encoding
problems show up immediately.

```bash
# request side: a client request envelope
python scripts/test_format.py --format-spec anthropic --command python -- my_format.py

# response side: an upstream response envelope (no method)
python scripts/test_format.py --side response --format-spec openai-chat --command python -- my_format.py

# response side, streaming: the body is a complete SSE event sequence
python scripts/test_format.py --side response --sse --format-spec anthropic --command ./my-format

# pass the same extra your config will use; supply your own body
python scripts/test_format.py --format-spec anthropic --extra '{"keys":["sk-a"]}' --command ./my-format
python scripts/test_format.py --format-spec anthropic --body-file payload.bin --command ./my-format
```

| Flag | Meaning |
|---|---|
| `--format-spec` | `anthropic`, `openai-chat` or `openai-responses`: protocol of the mock body (required) |
| `--side` | `request` (default) or `response` |
| `--sse` | Response side only: an SSE body instead of JSON |
| `--command` | Program to run (required); its arguments go after `--` |
| `--url` | Envelope `url` (default: the protocol's usual endpoint) |
| `--extra` | Envelope `extra` (default empty) |
| `--worker-id` | Envelope `worker_id` (default 0) |
| `--body-file` | Use a file as the body; non-UTF-8 content is sent as `body_b64` |
| `--raw` | Print the reply as received instead of pretty-printing it |
| `--timeout` | Seconds to wait for the reply (default 10) |

The harness reports `[FAIL]` when the program cannot start, never replies, dies, or replies with
invalid JSON, and `[WARN]` for an `error` reply or a program that ignores EOF. It prints a
checklist for the rest but does not verify it: confirm by eye that `headers` is present, the body
is there, and nothing else was printed.

To check the one-line rule directly, pipe two envelopes in and count the lines out:

```bash
E='{"method":"POST","url":"https://up.example.com/v1/x","headers":{},"body":"{}","worker_id":0,"extra":""}'
printf '%s\n' "$E" "$E" | python my_format.py | wc -l   # expect 2
```

Also feed an input that should fail and confirm you get an `error` reply rather than a crash.

## Configure it in aProxy

Transformers are per instance, set in that instance's config.toml (there is no global default and
no command-line flag):

```toml
request_transform  = { command = "/usr/bin/python3", args = ["/home/me/formats/convert.py"], mode = "persistent", pool_max = 4, idle_timeout_secs = 300, timeout_secs = 30, extra = '{"model": "gpt-4o"}' }
response_transform = { command = "/usr/bin/python3", args = ["/home/me/formats/convert.py"], mode = "persistent" }
```

`pool_max`, `idle_timeout_secs` and `timeout_secs` are shown at their defaults; the full field
reference is in the aproxy-cli skill, config-toml.md. Points that trip up format authors:

- **Two independent settings.** A protocol converter needs both; with only `request_transform`,
  responses reach the client unconverted. `extra` and every other field are set per side.
- **Use absolute paths.** aProxy runs `command` directly, without a shell. A bare name is looked
  up on PATH; a relative path resolves against the daemon's working directory, which is wherever
  the instance happened to be started. `~/` at the start of `command` expands to the user's home
  directory, but `args` and `extra` are passed untouched, so a script path in `args` must be
  absolute. On Windows, write paths with forward slashes or in single-quoted TOML strings
  (`'C:\fmt\convert.py'`); a bare command name is found on PATH only as an `.exe`.
- **Stateful programs need `persistent`.** In spawn mode every request starts a fresh process, so
  a rotation counter is always zero.
- **Not with `forward_only`.** An instance with `forward_only` enabled (in config.toml, or
  inherited from settings.json) refuses to start with a transformer, because it never buffers the
  body.
- **Every request on the instance goes through the transformer**, including requests your
  program was not written for (model listing, token counting, `GET` requests with an empty body).
  Pass those through unchanged or reply with a clear `error`.

Apply the change with `aproxy restart <port|alias>`. Restarting cuts every connection through that
instance. If you are an agent whose own model traffic goes through it, that includes your session
and the user's other sessions; ask before restarting it, or try the transformer first on a
separate instance with its own config file and a spare port (`aproxy start <config path>`).

Confirm it is active:

- `aproxy start` prints a line beginning `外部转换器：请求/响应将交给 format 程序改写` ("external
  transformer: requests/responses will be rewritten by the format program").
- The daemon log (`aproxy logs <port|alias>`) records `外部转换器已启用（信封协议交给外部 format 程序改写）`
  ("external transformer enabled") once per configured side at startup, and
  `请求已由外部转换器改写` ("request rewritten by the external transformer") with the new URL for
  every converted request.
- `aproxy config --show` prints the transformer settings as aProxy parsed them (`extra` masked).
