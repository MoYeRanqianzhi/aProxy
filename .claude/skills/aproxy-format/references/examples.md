# Examples

Complete, tested setups: key rotation, a protocol converter, the official aproxy-format binary
(install, aggregation config, behavior and limits), and a response-side transform. Read it when
you need a working starting point; the loop structure and duties are explained in guide.md.

## Contents

- [Key rotation](#key-rotation)
- [Protocol conversion](#protocol-conversion)
- [The official aproxy-format binary](#the-official-aproxy-format-binary)
  - [Install](#install)
  - [Connect it to an instance](#connect-it-to-an-instance)
  - [Aggregation config](#aggregation-config)
  - [What it does with each request](#what-it-does-with-each-request)
  - [Limits](#limits)
  - [The convert subcommand](#the-convert-subcommand)
- [Response-side transform](#response-side-transform)

## Key rotation

Gives each request the next key from a list, for a single upstream. Configure it as `request_transform`
only.

```python
#!/usr/bin/env python3
"""Rotate API keys across requests (request_transform only, mode = "persistent").

extra = '{"keys": ["sk-a", "sk-b", "sk-c"], "header": "authorization"}'
"header" is "authorization" (Bearer) or "x-api-key" (Anthropic style).
"""
import json
import sys

sys.stdin.reconfigure(encoding="utf-8")
sys.stdout.reconfigure(encoding="utf-8")

handled = 0  # requests served by this worker; lives as long as the process

for line in sys.stdin:
    try:
        env = json.loads(line)
        cfg = json.loads(env["extra"])
        keys = cfg["keys"]
        # Start each pool worker at a different key so busy pools spread evenly.
        key = keys[(env.get("worker_id", 0) + handled) % len(keys)]
        handled += 1
        headers = env["headers"]
        headers.pop("authorization", None)
        headers.pop("x-api-key", None)
        if cfg.get("header") == "x-api-key":
            headers["x-api-key"] = key
        else:
            headers["authorization"] = f"Bearer {key}"
        reply = env  # write the whole envelope back: an omitted body means an empty body
    except Exception as e:
        reply = {"headers": {}, "error": f"rotate: {type(e).__name__}: {e}"}
    sys.stdout.write(json.dumps(reply, ensure_ascii=False) + "\n")
    sys.stdout.flush()
```

```toml
request_transform = { command = "/usr/bin/python3", args = ["/home/me/formats/rotate.py"], mode = "persistent", extra = '{"keys": ["sk-a", "sk-b", "sk-c"], "header": "x-api-key"}' }
```

- `mode = "persistent"` is what makes rotation work: in spawn mode `handled` is always 0.
- The key is chosen once per client request. aProxy retries a failed request by replaying it with
  the same key, and it retries upstream 401/403 like any other error, so a revoked key makes every
  request that draws it retry indefinitely. Remove dead keys from the list and restart the
  instance.
- The program overwrites whatever key the client or the instance's `api_key` supplied.

## Protocol conversion

An Anthropic Messages client (Claude Code style) talking to an OpenAI Chat Completions upstream.
This is a readable skeleton for non-streaming text: no tools, images, or SSE. For real traffic use
the official binary below, which converts the full request and response schemas.

```python
#!/usr/bin/env python3
"""Anthropic Messages client -> OpenAI Chat Completions upstream.

Non-streaming text only: no tools, images or SSE. Configure this one file as
both request_transform and response_transform, mode = "persistent", with
extra = '{"url": "https://api.openai.com/v1/chat/completions", "key": "sk-...", "model": "gpt-4o"}'
"""
import json
import sys

sys.stdin.reconfigure(encoding="utf-8")
sys.stdout.reconfigure(encoding="utf-8")

STOP_REASON = {"stop": "end_turn", "length": "max_tokens", "tool_calls": "tool_use"}


def text_of(content):
    # Anthropic content is a string or a list of blocks; keep the text blocks.
    if isinstance(content, str):
        return content
    return "".join(b.get("text", "") for b in content if b.get("type") == "text")


def convert_request(env, cfg):
    req = json.loads(env["body"])
    if req.get("stream"):
        raise ValueError("streaming requests are not supported by this converter")
    messages = []
    if req.get("system"):
        messages.append({"role": "system", "content": text_of(req["system"])})
    for m in req["messages"]:
        messages.append({"role": m["role"], "content": text_of(m["content"])})
    body = {"model": cfg.get("model", req["model"]), "messages": messages,
            "max_tokens": req["max_tokens"]}
    for k in ("temperature", "top_p"):
        if k in req:
            body[k] = req[k]
    if "stop_sequences" in req:
        body["stop"] = req["stop_sequences"]
    env["url"] = cfg["url"]
    # Build the header table from scratch: the client's credentials must not
    # reach the new upstream.
    env["headers"] = {"content-type": "application/json",
                      "authorization": f"Bearer {cfg['key']}"}
    env["body"] = json.dumps(body, ensure_ascii=False)
    return env


def convert_response(env):
    resp = json.loads(env["body"])
    choice = resp["choices"][0]
    usage = resp.get("usage", {})
    body = {
        "id": resp.get("id", ""),
        "type": "message",
        "role": "assistant",
        "model": resp.get("model", ""),
        "content": [{"type": "text", "text": choice["message"].get("content") or ""}],
        "stop_reason": STOP_REASON.get(choice.get("finish_reason"), "end_turn"),
        "stop_sequence": None,
        "usage": {"input_tokens": usage.get("prompt_tokens", 0),
                  "output_tokens": usage.get("completion_tokens", 0)},
    }
    env["headers"] = {"content-type": "application/json"}
    env["body"] = json.dumps(body, ensure_ascii=False)
    return env


for line in sys.stdin:
    try:
        env = json.loads(line)
        if "method" in env:
            reply = convert_request(env, json.loads(env["extra"]))
        else:
            reply = convert_response(env)
    except Exception as e:
        reply = {"headers": {}, "error": f"convert: {type(e).__name__}: {e}"}
    sys.stdout.write(json.dumps(reply, ensure_ascii=False) + "\n")
    sys.stdout.flush()
```

```toml
request_transform  = { command = "/usr/bin/python3", args = ["/home/me/formats/convert.py"], mode = "persistent", extra = '{"url": "https://api.openai.com/v1/chat/completions", "key": "sk-...", "model": "gpt-4o"}' }
response_transform = { command = "/usr/bin/python3", args = ["/home/me/formats/convert.py"], mode = "persistent" }
```

Test both halves before connecting a client:

```bash
X='{"url": "https://api.openai.com/v1/chat/completions", "key": "sk-test", "model": "gpt-4o"}'
python scripts/test_format.py --format-spec anthropic --extra "$X" --command python3 -- convert.py
python scripts/test_format.py --side response --format-spec openai-chat --command python3 -- convert.py
```

This keeps the upstream key in plain text in config.toml (`aproxy config --show` masks `extra`,
the file does not). To keep it out of the config, put a file path in `extra` and read the key from
that file.

## The official aproxy-format binary

`aproxy-format` is a ready-made format program: protocol conversion between `anthropic_messages`,
`openai_chat` and `openai_responses`, key rotation (round robin or weighted), and routing by model
name across several upstream channels, all driven by one TOML file.

### Install

It is released separately from aProxy and is not installed or upgraded by `aproxy install`. Use
version 0.1.1 or later; 0.1.0 lets `client_format = "auto"` route across protocols and returns
responses the client cannot read. The binary has no `--version` flag, so note the version you
download.

| Source | How | Notes |
|---|---|---|
| GitHub Releases | Release tagged `format-v<version>`, asset `aproxy-format-<target>[-v3][.exe]` with a `.sha256` beside it | Save it as `~/.aproxy/bin/aproxy-format` (`aproxy-format.exe` on Windows); on Unix `chmod +x` it. `-v3` builds need an x86-64-v3 CPU (AVX2); take the plain build if unsure. |
| npm | `npm install -g @meowo/aproxy-format` | Installs a Node.js (18+) launcher on PATH that runs the platform binary. On Windows the launcher is a `.cmd` shim, which aProxy cannot start by bare name; use the release binary there. |
| crates.io | `cargo install aproxy-format` | Builds from source; needs Rust 1.96.1 or later. |

Release targets: `x86_64-pc-windows-msvc` (also `-v3`), `i686-pc-windows-msvc`,
`aarch64-pc-windows-msvc`, `x86_64-unknown-linux-gnu` (also `-v3`), `x86_64-unknown-linux-musl`,
`aarch64-unknown-linux-gnu`, `aarch64-unknown-linux-musl`, `aarch64-apple-darwin`,
`x86_64-apple-darwin`.

### Connect it to an instance

```toml
request_transform  = { command = "~/.aproxy/bin/aproxy-format", args = ["run"], mode = "persistent", extra = "~/.aproxy/agg.toml" }
response_transform = { command = "~/.aproxy/bin/aproxy-format", args = ["run"], mode = "persistent", extra = "~/.aproxy/agg.toml" }
```

- Both sides, same `args` and `extra`, `persistent` mode (rotation counters live in memory).
- `run` reads the aggregation config path from `extra`; `run --config <path>` (or
  `--config=<path>`) takes precedence over `extra`. aproxy-format expands a leading `~/` itself
  (from `HOME`, else `USERPROFILE`); other relative paths resolve against the daemon's working
  directory, so avoid them.
- Each worker loads the config on its first request and keeps it. After editing the file, restart
  the instance so all workers load the new version (restarting cuts every connection through it,
  see guide.md, Configure it in aProxy). A config that fails to load is retried on every request,
  so fixing a broken file needs no restart.
- The instance still needs a valid `base_url`: aProxy refuses to start without one, and with
  `preserve_path` its path becomes part of the forwarded path. The channel URLs decide where
  requests actually go.

### Aggregation config

```toml
client_format = "anthropic_messages"    # protocol your clients speak; "auto" only for same-protocol setups

[models]                                # optional: client model name -> upstream model name
"claude-sonnet" = "claude-sonnet-4-5"

[[channel]]                             # channels are tried in order; the first whose models match wins
name = "official"
format = "anthropic_messages"
url = "https://api.anthropic.com"       # origin only, with preserve_path: each endpoint keeps its path
preserve_path = true
keys = ["sk-ant-1", "sk-ant-2"]         # round robin by default
models = ["claude-*"]

[[channel]]
name = "relay"
format = "openai_chat"
url = "https://relay.example.com/v1/chat/completions"   # full endpoint: the protocol changes (non-streaming only)
keys = ["sk-relay-1", "sk-relay-2"]
strategy = "weighted"
weights = [3, 1]                        # sk-relay-1 three times, then sk-relay-2 once
# no models: this channel takes every model the channels above did not
```

| Key | Required | Meaning |
|---|---|---|
| `client_format` | Yes | `anthropic_messages`, `openai_chat`, `openai_responses`, or `auto`. Declares the clients' protocol; conversion happens when it differs from the channel's `format`. `auto` detects per request and only allows same-protocol routing (see Limits). |
| `[models]` | No | Client model name to upstream model name. Applied before routing, so channel `models` patterns match the upstream name. The request body gets the upstream name; a non-streaming JSON response gets the client name back. |
| `[[channel]] name` | Yes | Label in error messages; also identifies the channel's rotation counter, so keep names unique. |
| `format` | Yes | The channel's protocol, same names as `client_format` (no `auto`). |
| `url` | Yes | Without `preserve_path`, the exact URL every routed request is sent to. With `preserve_path = true`, a prefix: the request's path and query are appended. |
| `keys` | Yes | Non-empty list. Sent as `x-api-key` plus `anthropic-version: 2023-06-01` for `anthropic_messages`, as `authorization: Bearer` for the OpenAI formats. |
| `strategy` | No | `round_robin` (default) or `weighted`. |
| `weights` | With `weighted` | One positive integer per key; `[2, 1]` yields A, A, B, A, A, B, ... |
| `models` | No | Glob patterns (`claude-*`), case-sensitive. Omitted = every model. |
| `preserve_path` | No | Default `false`. |

Misspelled keys are ignored without warning (`model = [...]` instead of `models` leaves the
channel matching everything), and an invalid glob simply never matches. If routing looks wrong,
check spelling first.

### What it does with each request

Request side:

1. Parses the body as JSON and reads `model`; a request without a JSON body or a `model` field is
   rejected.
2. Maps the model through `[models]`, then picks the first channel whose `models` match.
3. Determines the client protocol (`client_format`, or detection under `auto`).
4. Picks the channel's next key, converts the body if the protocols differ, and writes the upstream
   model name into the body.
5. Sets `url` (the channel URL, plus the original path and query with `preserve_path`).
6. Removes `authorization`, `x-api-key`, `cookie` and `proxy-authorization` from the client's
   headers, adds the channel's credentials, and adds `content-type: application/json` if missing.
   Other client headers pass through.

Response side:

1. Finds the channel whose `url` is the longest prefix of the envelope `url`.
2. Removes the same four credential headers from the response.
3. Streaming body (any line starting with `data:` or `event:`): passed through unchanged when the
   protocols match, otherwise an error. JSON body: converted back to the client protocol, with the
   model name reverse-mapped.

### Limits

- **Cross-protocol conversion is non-streaming only.** A streaming response from a channel of
  another protocol fails the response transform, and aProxy passes the upstream stream through, so
  the client receives events it cannot parse. Clients that always stream (Claude Code sends
  `"stream": true`) need a channel of their own protocol. Same-protocol streams pass through
  untouched.
- **`auto` allows only same-protocol routing.** The response side cannot know what the client
  spoke, so `auto` rejects a request whose detected protocol differs from the channel's, before
  anything is sent upstream. Detection is a heuristic: `system` + `messages` + `max_tokens` means
  Anthropic, `input` or `instructions` means OpenAI Responses, other `messages` means OpenAI Chat.
  An Anthropic request without `system` is therefore detected as OpenAI Chat. Declare
  `client_format` explicitly for anything but a pure same-protocol key pool.
- **A full channel URL captures every path.** Without `preserve_path`, a token-count request
  (`/v1/messages/count_tokens`) is sent to the same endpoint as a chat request. For a
  same-protocol channel, use the origin with `preserve_path = true`.
- **Rotation is per worker.** Each persistent worker keeps its own counter per channel, starting
  at the first key, so with several busy workers the first keys see somewhat more traffic. It does
  not use `worker_id`.
- **No failover between channels or keys.** A request that drew a failing key keeps retrying with
  it (see Key rotation above).

### The convert subcommand

Converts a single request body between protocols without aProxy and without an envelope:

```bash
aproxy-format convert --from anthropic_messages --to openai_chat < request.json > converted.json
```

Input and output are plain request JSON (not envelopes); responses cannot be converted this way.
On failure it prints the reason to stderr and exits with code 1.

## Response-side transform

Rewrites what the client sees: keeps only an allow-list of upstream headers and renames the model
in JSON responses. Use it with the Python loop template in guide.md, configured as
`response_transform` only.

```python
KEEP = {"content-type", "request-id", "x-request-id"}


def transform(env: dict) -> dict:
    # Allow-list the headers the client may see; everything else is dropped.
    env["headers"] = {k: v for k, v in env["headers"].items() if k in KEEP}
    if env["headers"].get("content-type", "").startswith("application/json") and "body" in env:
        body = json.loads(env["body"])
        if "model" in body:
            body["model"] = "house-model"
        env["body"] = json.dumps(body, ensure_ascii=False)
    return env
```

The response side runs only on responses aProxy accepted as successful, and after aProxy has
already sent keepalive headers only the body change takes effect (protocol.md, When aProxy calls
you).
