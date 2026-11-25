# config.toml reference: format

Part of [config-toml.md](../config-toml.md), which lists the other parts. Configuring `[[format]]` entries on the aProxy side.

## format

A `[[format]]` entry runs an external "format" program at chosen points of a request's life: it can
rewrite the request before it goes upstream (protocol conversion, key rotation, routing by model),
rewrite the response on its way back, decide whether a failed attempt is retried, generate
heartbeats, and observe how requests ended. aProxy itself contains no conversion logic. The
official `aproxy-format` binary provides one subcommand per role; the wire protocol and how to
write your own program are in the aproxy-format skill. This section covers the aProxy side.

`[[format]]` is an array of tables, and **the order of the list is the order of the request
direction**: the first entry is closest to the client, the last one closest to the upstream.
Responses travel the list in reverse. Any number of programs may declare the same stage.

```toml
[[format]]
name = "adaptive"
command = "aproxy-format"
args = ["adaptive", "--config", "adaptive.toml"]
process = "persistent"
stages = ["request", "decide", "notify"]
required = false          # a judge: when it breaks, skip it and keep retrying as usual
pool_max = 1              # its memory of past requests lives in one process
worker_idle_secs = 0      # ... which must not be retired for being idle

[[format]]
name = "convert"
command = "aproxy-format"
args = ["convert", "--from", "anthropic_messages", "--to", "openai_chat"]
process = "persistent"
stages = ["request", "stream"]

[[format]]
name = "keys"
command = "aproxy-format"
args = ["keys", "--config", "keys.toml"]
process = "persistent"
stages = ["request", "retry"]
pool_max = 1
worker_idle_secs = 0
```

| Key | Type | Default | Meaning |
|---|---|---|---|
| `name` | string | file stem of `command` | Identifies the program in `config show`, logs, status and the error body's `details.format`. Unique in the list, 1 to 64 bytes, no control characters. `all` is reserved (it selects every entry in `aproxy restart --format all`) and is rejected as a name, including one derived from `command`. |
| `command` | string | required | The program, run directly and never through a shell. A leading `~` expands to the user's home directory; a relative path resolves against `<home>`; a bare name is looked up in `<home>/bin/` and then on `PATH` (the `PATH` in this entry's `env` if it sets one). On Windows a bare name without an extension means `<name>.exe`, and `.cmd` and `.bat` files are rejected: they only run through cmd.exe, and killing cmd.exe leaves the real program alive. To run a script, put the interpreter in `command` and the script in `args`. |
| `args` | array of strings | `[]` | Arguments, passed verbatim (no `~` expansion). The worker's working directory is `<home>`, so a relative path in `args` resolves there. |
| `env` | table of strings | `{}` | Extra environment variables for the program; the place for secrets. `config show` lists the names only, never the values. |
| `process` | `"spawn"` \| `"persistent"` | `"spawn"` | `spawn`: a new process per message. `persistent`: a pool of long-running workers. Required for `stream` and `notify`, and for any program that keeps state between requests. |
| `stages` | array of strings | required, non-empty | Which stages this program serves (below). |
| `required` | boolean | `true` | What happens when the program itself breaks (crash, timeout, invalid reply, open breaker) at the `request` or `retry` stage: a required program fails the attempt, an optional one is skipped. It does not affect a deliberate `fail` reply, which is always obeyed. |
| `pool_max` | integer | `4` | Most pooled workers in `persistent` mode, and most `spawn` processes handling messages at once; further messages queue. `0` is rejected. Workers that own a stream in progress are not counted, so the process count can reach `pool_max` plus the number of concurrent streams. Use `1` for a program whose in-memory state must be shared by all requests. |
| `worker_idle_secs` | integer | `300` | A `persistent` worker idle this long is stopped. `0` = never; use it together with `pool_max = 1` when losing the state would matter. |
| `timeout_secs` | integer | `30` | Time limit for one reply. `0` = none. A worker that times out is killed and replaced. Not used for `decide` and `notify`. |
| `decide_timeout_secs` | integer | `2` | Time limit for a `decide` reply. A late answer counts as "no opinion"; a decision program never ends a request by being slow. |
| `breaker_failures` | integer | `5` | This many process-level failures in a row open the program's circuit breaker. `0` = no breaker. |
| `breaker_cooldown_secs` | integer | `30` | How long an open breaker waits before letting one message through as a probe. `0` is rejected. |

Stages:

| Stage | When the program is called | Typical use |
|---|---|---|
| `request` | Once per request, before the first upstream attempt | Convert the protocol, choose a key or channel, pick the delivery mode |
| `retry` | Before a retry whose request is rebuilt, with the previous failure and the previous response's status and headers | Switch key or channel after a failure |
| `decide` | After a failed attempt (and after a successful one for programs that ask) | Retry or stop, how long to wait, whether to rebuild the request |
| `response` | On a whole successful response before it is delivered | Convert a non-streaming response |
| `stream` | On a successful response chunk by chunk (`open`, `data`, `close`, `abort`) | Convert a streaming response as it flows |
| `heartbeat` | On every keepalive tick, with the bytes about to be sent | Emit a heartbeat the client's protocol understands |
| `notify` | After a request ended; aProxy does not wait for the answer | Learn from outcomes |

Rules checked at start (`config_conflict`):

- One program cannot declare both `response` and `stream`.
- A program that declares `stream` or `notify` must use `process = "persistent"`.
- In the request direction, every program listed after one that declares `retry` and that serves
  `request` must declare `retry` too. On a retry the earlier program produces a new request; a
  later one that did not run again would send the previous attempt's output. For example a
  `convert` or `rewrite` placed after `route` declares `retry`.
- Names are unique.

`admit` is reserved and reported as `config_not_supported`. Any other stage name is
`config_invalid_value`.

At start aProxy runs a handshake with every program (it must answer within 10 seconds). If a
program does not support a stage the entry declares, or speaks another protocol version, the
instance does not start (`format_stage_unsupported`, `format_handshake_failed`). If it cannot be
started at all, a required program stops the start and an optional one only produces a warning and
begins with its breaker open.

What a broken program does to requests:

| Situation | `required = true` | `required = false` |
|---|---|---|
| Crash, timeout or invalid reply at `request`/`retry` | The attempt fails with `format_error` and is retried like any failed attempt | The layer is skipped: its input is used as its output, with a warning in the log |
| Breaker open at `request`/`retry` | The request ends at once with `503` `format_unavailable` (`details.probe_at` says when the next probe is due) | The layer is skipped |
| Any failure at `decide` | Counts as "no opinion"; the retry rules and the default backoff apply | same |
| Any failure at `response` or when a stream opens | The layer is skipped and the upstream response is delivered unconverted | same |
| Any failure at `heartbeat` | That tick sends the bytes it was given | same |
| Any failure at `notify` | The notification is dropped and counted | same |

A deliberate answer is different from a failure: a program that replies `fail` with `retry: false`
ends the request with `422` `format_rejected` (or with the response it supplies), and `fail` with
`retry: true` makes the attempt fail with `format_fail` and be retried.

Other points:

- Because a skipped cross-protocol converter would hand the client the upstream's protocol, keep
  protocol converters `required = true` and make only judges (`policy`, `adaptive`) optional.
- A request-side converter alone does not convert responses. If only `request` is declared, the
  client receives the upstream's protocol; add `stream` (or `response`) to the same entry.
- Choose `timeout_secs` by how long a stuck program may hold one request, not by client timeouts:
  keepalive requests keep receiving heartbeats while a program works.
- A response passed through because a retry rule gave up is not sent through `response` or
  `stream`. To rewrite such errors in the client's protocol, use a `decide` program that stops with
  a response of its own (the official `policy` role does this).
- A program's stderr goes to the instance log, one line per line, with known secrets replaced.
  stdout carries only protocol lines.
- There is no `extra` key. Pass data through `args` or `env`.
- Apply changes with `aproxy restart <port|alias>`.

Runtime behavior and the error codes: behaviors.md, "Format programs"; ready-made roles, `init`,
`check` and `try`: the aproxy-format skill.
