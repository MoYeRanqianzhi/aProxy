---
name: aproxy-format
description: Write, test and configure format programs - the external programs aProxy runs on a request's way upstream and a response's way back (stages request, retry, decide, response, stream, heartbeat, notify of a [[format]] entry) - and use the official aproxy-format binary: roles convert, keys, route, policy, adaptive, rewrite, heartbeat; tools init, check, try, translate. Covers format protocol 1, spawn vs persistent workers, conversion between OpenAI Chat, OpenAI Responses and Anthropic Messages including streaming, key rotation, routing and failover by model, retry policy, restarting one format, control methods, and diagnosing format failures. Use whenever a request must be rewritten, routed or re-keyed through aProxy, a response rewritten on its way back, or retry decisions customised, even if the user never says "format" - e.g. "convert Anthropic requests to OpenAI", "rotate between three keys", "my format fails every request with 422". For aProxy commands and the rest of config.toml use the aproxy-cli skill.
---

# aProxy format programs

A format program is a child process that aProxy talks to over stdin and stdout, one JSON object
per line. aProxy sends a message at each point of a request's life that the program subscribed
to (a *stage*); the program answers with exactly one line that may patch the request or the
response, cast a vote, or reject. Several programs form a chain: the order of the `[[format]]`
entries in config.toml is the order a request passes through them on its way upstream, and
responses pass through in reverse.

| Stage | When aProxy sends it | What a reply can do |
|---|---|---|
| `request` | Once per client request, before the first upstream attempt | Patch method, URL, headers, body; choose `forward` or `retry` delivery; override settings for this request; reject |
| `retry` | Before a retry, to the programs that declared it | Same as `request`, knowing why the last attempt failed (switch key or channel) |
| `decide` | After an attempt failed (after a successful one too, on request) | Vote retry or stop, set the wait, ask for a reroute, supply the final response |
| `response` | On a successful response, buffered whole | Patch status, headers, body |
| `stream` | On a successful response, piece by piece (`open`, `data`, `close`, `abort`) | Rewrite the byte stream as it flows |
| `heartbeat` | At each keepalive beat while the client waits | Replace the keepalive bytes with a protocol-native event |
| `notify` | After the request ended | Nothing: it is an observation, used to learn |

## First, see whether the official binary already does it

`aproxy-format <role>` runs one ready-made unit per `[[format]]` entry. Chain the ones you need;
they also mix with programs of your own.

| Role | Does | Stages |
|---|---|---|
| `convert` | Converts requests, responses and streams between `anthropic_messages`, `openai_chat` and `openai_responses` | request, retry, and response or stream |
| `keys` | Picks an API key per upstream origin; cools a key down after 401, 403, 429 | request, retry |
| `route` | Sends each model to a channel, with priorities, weights, failover and aliases | request, retry (response to map aliases back) |
| `policy` | Rule table over failed attempts: retry, stop, wait, reroute, honor `Retry-After` | decide |
| `adaptive` | Delivers requests that have not failed recently in `forward` mode (true streaming), the rest in `retry` | request, decide, notify |
| `rewrite` | Sets and removes headers and JSON body fields, renames models | request, retry |
| `heartbeat` | Turns the keepalive into a native Anthropic `ping` event | heartbeat |

Tools that are not part of a chain: `init` writes a starter configuration for a common scenario,
`check` validates a chain offline, `try` rehearses a sample request through a chain without any
network traffic, `translate` converts one request body. The binary is a separate download with
its own version line. Installation, every role's configuration and complete chains are in
[references/examples.md](references/examples.md).

Start there: `aproxy-format init <scenario> --output <dir>` writes `<dir>/config.toml` and the
role files. Edit them, then validate with `aproxy-format check <dir>/config.toml --home <home>` and
rehearse with `aproxy-format try <dir>/config.toml --request sample.json --home <home>`, where
`<home>` is the instance's aProxy home and `sample.json` a request you write
([references/examples/tools.md](references/examples/tools.md) has the sample format and what each
tool does not cover). Write your own program only for what the roles do not cover.

## Writing your own

1. **Decide which stages you need and how much of the request body.** Ask for as little as the
   job needs: a program that only edits headers or only votes should request body `none`, so
   megabytes of conversation never cross its pipe. `request_body` in the
   [hello](references/protocol/transport.md#hello) reply sets this per stage; each stage's section
   in [references/protocol.md](references/protocol.md) lists what else it receives (`decide`, for
   one, gets the upstream response body whatever you choose).
2. **Pick a process model.** `process = "spawn"` (the default) starts a process per message;
   `process = "persistent"` keeps workers that loop over lines, which is faster and is required
   for `stream` and `notify`.
3. **Start from the template** in [references/guide.md](references/guide.md): read a line,
   answer the `hello`, then answer every message with one line that echoes its `seq`.
4. **Test it without aProxy:**
   `python <this skill's directory>/scripts/test_format.py --command <program> -- <args>` plays
   aProxy's side: `hello`, then one representative message for each data-plane stage the program
   declares, each reply checked against the protocol. It runs the program with your environment and
   prints its stderr, so use it on code you trust, with made-up keys and payloads
   ([references/guide.md](references/guide.md), "Test without aProxy").
5. **Configure it** as a `[[format]]` entry (keys in the aproxy-cli skill,
   `references/latest/config-toml.md`), run `aproxy doctor`, and restart the instance. Later
   changes to the program itself only need `aproxy restart <port> --format <name>`, which keeps
   the instance and its in-flight requests.
6. **Optionally offer control methods** that a desktop extension can call, served by a separate
   process (`control` in
   [references/protocol/control-and-errors.md](references/protocol/control-and-errors.md#control)).

## The rules that matter most, and why

- **The first line is always `hello`; answer it before anything else.** It fixes the protocol
  version, tells aProxy which stages you implement and how much request body each needs. An
  instance will not start when a required program fails the handshake or lacks a configured stage.
- **One line in, one line out, and echo `seq`.** aProxy pairs replies with messages by `seq`. A
  missing, extra or mismatched line is a protocol error: the worker is replaced and the failure
  counts toward the circuit breaker.
- **stdout carries replies and nothing else; flush after each one.** Send diagnostics to stderr:
  aProxy writes those lines to the instance log, prefixed `format[<name>]`, with known secrets
  redacted.
- **A reply is a patch.** Whatever you leave out stays as it was; `{"v":1,"seq":7}` alone means
  "no change" (or "no vote" in `decide`). Two exceptions to keep in mind: `headers`, when given,
  replaces the whole table, and a body is cleared with `"body":""`, not by omitting it.
- **Do not keep per-request facts in memory.** The next message of the same request may reach
  another worker. Put them in `state`, a string aProxy stores per program and request and hands
  back with every later message. Memory that must survive across requests needs
  `pool_max = 1` and `worker_idle_secs = 0`, and must tolerate being lost: workers are replaced
  after errors and timeouts.
- **Say no with `fail`, not by crashing.** `fail` is a decision (`retry: true` counts as a failed
  attempt, `retry: false` ends the request with your reason); a crash, a timeout or an invalid
  line is an error that trips the breaker after `breaker_failures` in a row.
- **Exit when stdin reaches EOF.** That is how aProxy retires a worker; a program that ignores it
  is killed after a two-second grace.

What an error in your program costs depends on `required` (default `true`) and on the stage:

| Stage | `required = true` | `required = false` |
|---|---|---|
| request, retry | The attempt fails with `format_error` and is retried; with the breaker open the request ends at once with 503 `format_unavailable` | The layer is skipped: its input passes on unchanged |
| decide | Counts as "no vote" | Same |
| response, stream open | The layer is skipped: the response passes through unconverted | Same |
| heartbeat | That beat uses the input bytes | Same |
| notify | Dropped and counted | Same |

Make a program whose output the client cannot do without (a protocol converter) `required`, and
a program that only improves decisions (`policy`, `adaptive`) optional.

## Where to look

If you cannot open the files linked below (a sandbox, or the skill installed under another home), read them through the binary instead: `aproxy skill show aproxy-format --files` lists them and `aproxy skill show aproxy-format references/protocol.md` prints one. `skill show` checks each file against the installed manifest: that proves you read the installed copy intact, not that it matches the aProxy you run, because skills can be installed at another version. `aproxy skill list --json` gives each skill's `version` and `matches_binary`; when `matches_binary` is false, check version-sensitive details against that binary's `--help`. The official `aproxy-format` binary has its own version, independent of both.

protocol.md and examples.md are indexes: each lists its parts with the sections they hold. Open the
index, then only the part the task needs.

| You need | Read |
|---|---|
| Every frame field by field, with sample frames for each stage; limits; delivery modes; the breaker | [references/protocol.md](references/protocol.md) |
| Templates, process model, state, testing, wiring a program into an instance | [references/guide.md](references/guide.md) |
| Installing `aproxy-format`, each role's configuration, complete chains, known conversion limits, the tools | [references/examples.md](references/examples.md) |
| A 422, 502 or 503 that names a format, a chain that will not start, conversion that does not happen, keys that do not rotate | [references/troubleshooting.md](references/troubleshooting.md) |
| The test harness | [scripts/test_format.py](scripts/test_format.py) |
