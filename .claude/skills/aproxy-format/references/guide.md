# Writing a format program

How to write, test and install a program of your own. Read it after SKILL.md when the official
roles do not cover the job. The exact shape of every frame is in [protocol.md](protocol.md).

## Contents

- [Plan the program](#plan-the-program)
- [Choose a process model](#choose-a-process-model)
- [A minimal program](#a-minimal-program)
- [A deciding program with state](#a-deciding-program-with-state)
- [Techniques](#techniques)
- [Language pitfalls](#language-pitfalls)
- [Test without aProxy](#test-without-aproxy)
- [Configure it in aProxy](#configure-it-in-aproxy)

## Plan the program

Answer three questions before writing code; they become the hello reply.

1. **Which stages?** Subscribe to the fewest that do the job: every stage is one more pipe
   round trip on the request path.

   | Goal | Stages |
   |---|---|
   | Change the URL, headers or body sent upstream | `request` (add `retry` to choose again on each retry) |
   | Switch key or channel after a failure | `request`, `retry` |
   | Decide whether and when to retry, or replace an error | `decide` |
   | Rewrite a whole non-streaming response | `response` |
   | Rewrite a response as it streams | `stream` |
   | Send protocol-native keepalives | `heartbeat` |
   | Learn from how requests ended | `notify` (with `request` or `decide` to act on it) |

2. **How much request body per stage?** `none` when you only touch the URL and headers or only
   vote, `digest` when you need to recognise the same request again, `full` when you read or
   rewrite the body. Asking for less keeps megabytes off your pipe and conversation content out
   of your process.
3. **What must be remembered, and for how long?** Within one request: `state`. Across requests:
   process memory, with the constraints in [Techniques](#techniques).

One program should do one job. Two small programs in a chain each get their own worker pool
and breaker, so a slow or broken one does not take the other down.

## Choose a process model

| | `process = "spawn"` (default) | `process = "persistent"` |
|---|---|---|
| Lifetime | A new process per message: hello, one message, EOF; up to `pool_max` (default 4) at once | Workers answer messages until EOF; up to `pool_max` (default 4) run at once |
| Cost | Process start plus handshake on every message | One handshake per worker |
| Stages | All except `stream` and `notify` | All |
| Memory between messages | None | Per worker; see below |

Use `persistent` unless the program is rarely called. The loop is the same in both modes (read
a line, answer it, until EOF), so a program written for one runs in the other.

## A minimal program

It tags every upstream request with a header. It asks for no request body, handles `retry` the
same way as `request`, and answers anything else with an empty reply.

```python
#!/usr/bin/env python3
"""Minimal protocol 1 format program: tags every upstream request with a header."""
import json
import sys

# Windows pipes default to the system code page; force UTF-8 both ways, and
# keep "\n" from being rewritten to "\r\n".
sys.stdin.reconfigure(encoding="utf-8")
sys.stdout.reconfigure(encoding="utf-8", newline="\n")

HELLO = {
    "protocol": 1,
    "stages": ["request", "retry"],
    # Only headers are edited, so do not ask for the body.
    "request_body": {"request": "none", "retry": "none"},
    "features": [],
    "name": "tag-team",
    "version": "1.0.0",
}


def handle(msg):
    """Return the reply fields for one stage message; {} changes nothing."""
    if msg["stage"] in ("request", "retry"):
        headers = dict(msg["request"]["headers"])  # a headers patch replaces the whole table
        headers["x-team"] = "search"
        return {
            "request": {"headers": headers},
            "report": {
                "summary": "tagged the request",
                "actions": [{"type": "header_edit", "name": "x-team", "op": "set"}],
            },
        }
    return {}


for line in sys.stdin:  # one message per line, until aProxy closes stdin
    msg = json.loads(line)
    reply = dict(HELLO) if msg["stage"] == "hello" else handle(msg)
    reply["v"], reply["seq"] = 1, msg["seq"]  # every reply echoes seq
    sys.stdout.write(json.dumps(reply, ensure_ascii=False) + "\n")
    sys.stdout.flush()
```

What the skeleton gets right, and why each matters:

- It answers `hello` with every stage it implements. aProxy only sends the stages configured for
  the entry, but refuses to start if a configured stage is missing from this list.
- It copies the header table before adding to it. A `headers` patch replaces the table, so
  returning only the new header would drop `content-type` and the credentials.
- It writes one compact line per message and flushes. Pretty-printed JSON is several lines, and
  an unflushed reply looks like a timeout.
- It ends when the `for` loop sees EOF. That is how aProxy retires workers.
- It never prints anything else to stdout. Use `print(..., file=sys.stderr)` for diagnostics;
  those lines land in the instance log as `format[<name>] ...`.

## A deciding program with state

This one stops the retries when the upstream reports that the prompt exceeds the context
window, and answers the client in the client's own protocol. It shows three things: carrying a
fact from `request` to `decide` in `state`, abstaining, and a stop with a response.

```python
#!/usr/bin/env python3
"""Stops retrying when the upstream says the prompt is too long, in the client's protocol."""
import json
import re
import sys

sys.stdin.reconfigure(encoding="utf-8")
sys.stdout.reconfigure(encoding="utf-8", newline="\n")

HELLO = {
    "protocol": 1,
    "stages": ["request", "decide"],
    # request: only the path is needed; decide: the response preview is enough.
    "request_body": {"request": "none", "decide": "none"},
    "features": [],
    "name": "context-guard",
    "version": "1.0.0",
}
TOO_LONG = re.compile(r"context.{0,20}(length|window)|prompt is too long", re.I)


def handle(msg):
    stage = msg["stage"]
    if stage == "request":
        # Remember which protocol the client speaks. The decide message may be
        # answered by another worker, so the fact travels in state, not in memory.
        anthropic = msg["client"]["path"].startswith("/v1/messages")
        return {"state": json.dumps({"anthropic": anthropic})}
    if stage == "decide":
        body = (msg.get("response") or {}).get("body") or ""
        if msg["outcome"] != "failure" or not TOO_LONG.search(body):
            return {}  # no vote: the next program, or aProxy's own rules, decide
        state = json.loads(msg.get("state") or "{}")
        text = "The conversation is longer than the model's context window."
        if state.get("anthropic"):
            error = {"type": "error", "error": {"type": "invalid_request_error", "message": text}}
        else:
            error = {"error": {"type": "invalid_request_error", "code": "context_length_exceeded", "message": text}}
        return {
            "decision": "stop",
            "response": {
                "status": 400,
                "headers": {"content-type": "application/json"},
                "body": json.dumps(error),
            },
            "report": {
                "summary": "context too long; stopped retrying",
                "actions": [{"type": "custom", "label": "stop: context too long"}],
            },
        }
    return {}


for line in sys.stdin:
    msg = json.loads(line)
    reply = dict(HELLO) if msg["stage"] == "hello" else handle(msg)
    reply["v"], reply["seq"] = 1, msg["seq"]
    sys.stdout.write(json.dumps(reply, ensure_ascii=False) + "\n")
    sys.stdout.flush()
```

Notes on it:

- The response is written from scratch. Never echo the upstream's body or headers to the client
  from a program: they can hold another provider's error format or credentials.
- `decide` is answered within `decide_timeout_secs` (default 2 seconds). Keep it to a lookup and
  a comparison; a slow vote is treated as no vote.
- The official `policy` role covers this case with a rule table and no code
  ([examples.md](examples.md)); write a program only when the rule needs logic.

## Techniques

**Carry facts in `state`.** `state` is a string (serialize your own JSON into it) of at most
64 KiB, private to your program and the request. Set it in any reply and it comes back on every
later message of that request, on any worker. Keep it small and free of secrets: store a key's
id, never the key.

**Memory across requests.** A counter, a cooldown table or a learned history lives in one
process only. If every request must see it, configure `pool_max = 1` and `worker_idle_secs = 0`
(never retire the worker for being idle). Even then the worker is replaced after a timeout, a
protocol error or an open breaker, so losing the memory may make decisions worse but must never
make a reply wrong.

**Reject with `fail`.** `{"fail":{"retry":false,"reason":"..."}}` ends the request with 422
`format_rejected` and your reason, or with the response you attach. `"retry":true` fails only
this attempt and lets the retry loop continue, which is the right answer for "every key is
cooling down right now". Do not exit or raise: a crash loses the reason and counts toward the
breaker.

**Catch your own errors.** Wrap the handler so that an unexpected exception becomes a `fail`
reply with the right `seq` (or an empty reply in `decide`, `heartbeat` and `notify`, where
abstaining is the safe answer) and the loop goes on.

**Change behavior for one request with `overrides`.** A `request` or `retry` reply can set
`{"overrides":{"timeouts":{"first_byte_secs":900}}}`; the allowed keys arrive in hello's
`overrides`.

**Choose retries per failure.** In `retry`, `retry.last_failure.code` and
`retry.last_response` (status and headers, including `retry-after`) say what happened; pick
another key or channel accordingly and record your choice in `state`. A `retry` message arrives
only when the retry segment is actually rerun; when a `decide` program voted `reroute: false`,
aProxy resends the previous request and your program is not called.

**Streams.** In `stream`, one worker owns the stream from `open` to `close`/`abort`, so parser
state can live in a dictionary keyed by `stream_id`. A `data` piece can end in the middle of an
SSE event: append to a buffer, emit only complete events, and reply with no `data` when nothing
is complete yet. Give an `abort_tail` at `open` (a complete error event in the client's
protocol) so that the client gets a clean ending even if your process dies. Answer `abort`
with an error ending, never with a normal end-of-stream.

**Never guess `client.stream`.** It is omitted when aProxy does not know yet. If the program
must know at `request`, ask for `digest` or `full` there.

**One program, several entries.** The same command can appear in several `[[format]]` entries
with different `args` and `stages`; each entry has its own workers, state and breaker.

## Language pitfalls

| Language | Pitfall | Fix |
|---|---|---|
| Python on Windows | Pipes use the system code page: non-ASCII text is garbled or raises `UnicodeEncodeError` | `reconfigure(encoding="utf-8")` on stdin and stdout, as in the template |
| Python | Reading with `sys.stdin.read()` or `json.load(sys.stdin)` waits for EOF, which a persistent worker never gets between messages | Iterate over lines |
| Python | `print` without `flush=True` | Flush after each reply |
| bash + jq | `jq` pretty-prints over several lines by default | Always `jq -c`; and remember the hello line needs its own reply |
| Node.js | `console.log` debugging mixed with replies on stdout | `console.error` for diagnostics |
| Go | `bufio.Scanner` stops at lines over 64 KiB | `scanner.Buffer(make([]byte, 0, 1<<20), 512<<20)` or `bufio.Reader.ReadString('\n')` |
| Go, Rust, C | Buffered writer never flushed | Flush after each line |
| PowerShell | `ConvertTo-Json` is multi-line and truncates nesting at depth 2 | `ConvertTo-Json -Compress -Depth 100`; set `[Console]::OutputEncoding` to UTF-8 |
| Any | A runtime or library prints a banner or warning on stdout at startup | Silence it or send it to stderr; the first line aProxy reads must be the hello reply |
| Any | JSON library writes absent values as `null` | Harmless: in replies `null` means omitted |

In Rust, the `aproxy-envelope` crate has typed messages and replies for protocol 1 (`Message`,
`Stage`, `Reply`, `HelloReply`, the HTTP objects and patches), so a program can deserialize a
line into `Message` and serialize a `Reply`.

Messages can be many megabytes when a stage asked for the `full` body, so read lines into a
growable buffer in any language.

## Test without aProxy

`scripts/test_format.py` plays aProxy's side of the pipe. It sends `hello`, checks the reply,
sends a realistic message for each stage the program declared, checks every reply (one line,
`seq`, field shapes, patch rules, vote ranges), threads `state` from reply to message as aProxy
does, then closes stdin and checks that the program exits.

```bash
# every stage the program declares; arguments after -- go to the program
python scripts/test_format.py --command python -- my_format.py

# see every frame in both directions
python scripts/test_format.py -v --command ./my-format

# only some stages, in spawn mode (a fresh process and hello per message)
python scripts/test_format.py --stage request,retry --process spawn --command ./my-format

# OpenAI-shaped client traffic; a converter whose upstream speaks another protocol
python scripts/test_format.py --protocol openai-chat --upstream anthropic --command ./my-format

# your own request body, addressed to an upstream your program knows
python scripts/test_format.py --body-file payload.json --url https://relay.example.com/v1/messages --command ./my-format
```

Exit code 0 means every check passed. Lines marked `warn` are things aProxy tolerates but
ignores or drops (a malformed report, `state` in a heartbeat reply). The program's stderr is
shown at the end. Pass the program its configuration the way config.toml will: arguments after
`--`, secrets in the environment.

To look at one exchange by hand, pipe two lines in:

```bash
printf '%s\n' \
  '{"v":1,"seq":0,"stage":"hello","protocols":[1],"aproxy_version":"0.2.0","name":"x","process":"persistent","stages":["request"],"worker_id":0,"features":[],"overrides":[]}' \
  '{"v":1,"seq":1,"stage":"request","request_id":"r-1","delivery":"retry","request":{"method":"POST","url":"https://relay.example.com/v1/messages","headers":{"content-type":"application/json"},"body":"{\"model\":\"m\",\"messages\":[]}"},"client":{"path":"/v1/messages","stream":false}}' \
  | python my_format.py
```

Expect exactly two lines back, with `seq` 0 and 1.

The harness checks the protocol, not the meaning of your output. For a converter, also send
the result through the target SDK's parser or a real endpoint once. To see a whole chain work
together, including the official roles, use `aproxy-format try` ([examples.md](examples.md)).

## Configure it in aProxy

```toml
[[format]]
name = "tag-team"                 # shown in logs, errors and reports
command = "python"                # bare name: <home>/bin/ first, then PATH
args = ["formats/tag_team.py"]    # relative paths resolve against the aProxy home
process = "persistent"
stages = ["request", "retry"]
# required = true                 # default; false = skip this layer when the program is broken
# env = { TEAM_TOKEN = "..." }    # secrets go here, not in args
```

- The order of `[[format]]` entries is the order of the chain on the way upstream; the first
  entry is nearest the client.
- Rules aProxy enforces at startup (`config_conflict` otherwise): a program cannot declare both
  `response` and `stream`; a program declaring `stream` or `notify` must be `persistent`; after
  the first entry that declares `retry`, every later request-direction entry must declare
  `retry` too.
- On Windows `command` must be an executable: `.cmd` and `.bat` files are refused. Put the
  interpreter in `command` and the script in `args`.
- Tuning keys: `pool_max` (4), `worker_idle_secs` (300; 0 = never retire), `timeout_secs` (30),
  `decide_timeout_secs` (2), `breaker_failures` (5; 0 = no breaker), `breaker_cooldown_secs`
  (30). The full table is in the aproxy-cli skill, `references/latest/config-toml.md`.
- After editing a `[[format]]` entry, run `aproxy doctor` to validate the file, then restart the
  instance; the chain is fixed for the life of an instance. A restart cuts the connections going
  through that instance, so pick a quiet moment.
- After changing only the program or its own files (a script, a role file, its settings), run
  `aproxy restart <port|alias> --format <name>` instead: the instance stays up, requests in flight
  finish on the old processes, and new ones reach a fresh process. config.toml is not reread. If
  the new process fails its hello, nothing is replaced and the old one keeps serving. A program
  that loads its settings once at startup needs exactly this after each edit.
- Watch it work with `aproxy logs <port|alias>`: your stderr lines carry the prefix
  `format[<name>]`, and breaker changes are logged as warnings.

## Offer a control interface

A desktop extension (or another tool speaking the instance protocol) can call methods you define
on your program: read its settings, change them, ask for its state. Offer it by adding
`"control":{"args":[...]}` to the hello reply. When the first call arrives, aProxy starts a separate
process of the same executable with those arguments instead of the configured ones and sends it
`stage:"control"` messages. That process never sees requests, and request workers never see
control calls, so a slow call cannot hold up traffic.

Control changes nothing by itself. If a call changes settings that request workers read, decide
how they learn about it: write a file the workers reread, or tell the user to run
`aproxy restart <port> --format <name>`. The frames, limits and failure handling are in
[protocol/control-and-errors.md](protocol/control-and-errors.md#control).
