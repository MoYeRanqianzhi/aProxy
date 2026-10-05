#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Test harness for aProxy format programs, no aProxy needed.

Sends one mock envelope to the format program under test, pretty-prints the
envelope it writes back, then closes the program's stdin and checks that it
exits (the EOF duty of persistent mode). Run it first thing after writing a
format program.

Six mock envelopes: three protocols x request/response side. The bodies follow
the shapes in each provider's API reference.

  # Request side: a client request envelope (has "method")
  python test_format.py --format-spec anthropic --command ./my-format

  # Response side: an upstream response envelope (no "method")
  python test_format.py --side response --format-spec openai-chat --command ./my-format

  # Response side, streaming: the body is a complete SSE event sequence, which
  # is what a format program sees after aProxy has buffered the whole stream
  python test_format.py --side response --sse --format-spec anthropic --command ./my-format

  # Arguments after "--" go to the program verbatim; --body-file supplies a
  # custom body (non-UTF-8 content is sent as body_b64)
  python test_format.py --format-spec openai-chat --command node -- format-node.js
  python test_format.py --format-spec anthropic --command ./fmt --body-file payload.bin

Windows: this script forces UTF-8 on its own stdin/stdout. The program under
test must set UTF-8 on its own pipes as well (see guide.md); a script whose
stdout uses the system code page garbles or rejects non-ASCII text.
"""

import argparse
import base64
import json
import subprocess
import sys
import threading

if hasattr(sys.stdout, "reconfigure"):
    sys.stdin.reconfigure(encoding="utf-8")
    sys.stdout.reconfigure(encoding="utf-8")
    sys.stderr.reconfigure(encoding="utf-8")

# ---- Mock request bodies (shapes from each protocol's API reference) ---------
# The Chinese message text is deliberate: it makes every run exercise non-ASCII
# UTF-8 round trips, the most common encoding failure on Windows.

MOCK_REQUESTS = {
    "anthropic": {
        "model": "claude-sonnet-4-20250514",
        "system": [{"type": "text", "text": "You are a helpful assistant."}],
        "messages": [
            {"role": "user", "content": "我上一轮问了什么？"},
            {"role": "assistant", "content": [{"type": "text", "text": "你问了天气。"}]},
            {"role": "user", "content": [{"type": "text", "text": "那今天呢？"}]},
        ],
        "max_tokens": 256,
        "temperature": 0.7,
        "stop_sequences": ["END"],
        "stream": False,
        "metadata": {"user_id": "test-format-user"},
    },
    "openai-chat": {
        "model": "gpt-4o",
        "messages": [
            {"role": "system", "content": "You are a helpful assistant."},
            {"role": "user", "content": "我上一轮问了什么？"},
            {"role": "assistant", "content": "你问了天气。"},
            {"role": "user", "content": "那今天呢？"},
        ],
        "max_completion_tokens": 256,
        "temperature": 0.7,
        "stream": False,
    },
    "openai-responses": {
        "model": "gpt-4o",
        "instructions": "You are a helpful assistant.",
        "input": "那今天呢？",
        "max_output_tokens": 256,
        "stream": False,
    },
}

# ---- Mock response bodies (shapes from each protocol's API reference) --------

MOCK_RESPONSES = {
    "anthropic": {
        "id": "msg_01XFDUDYJgAACzvnptvVo6EL",
        "type": "message",
        "role": "assistant",
        "model": "claude-sonnet-4-20250514",
        "content": [{"type": "text", "text": "今天天气晴朗。"}],
        "stop_reason": "end_turn",
        "stop_sequence": None,
        "usage": {"input_tokens": 25, "output_tokens": 17},
    },
    "openai-chat": {
        "id": "chatcmpl-B7gHmQQ3ZtoZfu6gpioLOdDgUPIeb",
        "object": "chat.completion",
        "created": 1749000000,
        "model": "gpt-4o-2024-08-06",
        "choices": [
            {
                "index": 0,
                "message": {"role": "assistant", "content": "今天天气晴朗。"},
                "finish_reason": "stop",
            }
        ],
        "usage": {"prompt_tokens": 25, "completion_tokens": 9, "total_tokens": 34},
    },
    "openai-responses": {
        "id": "resp_67cad3e5e08c8111a28ad2be1f6d9e4f",
        "object": "response",
        "created_at": 1749000000,
        "status": "completed",
        "model": "gpt-4o-2024-08-06",
        "output": [
            {
                "type": "message",
                "id": "msg_67cad3e5e08c8111a28ad2be1f6d9e4f",
                "role": "assistant",
                "content": [{"type": "output_text", "text": "今天天气晴朗。"}],
            }
        ],
        "usage": {"input_tokens": 25, "output_tokens": 9, "total_tokens": 34},
    },
}

# ---- Mock SSE response text: a complete event sequence, as a format program ---
# ---- receives it after aProxy has buffered the whole stream -----------------

MOCK_SSE = {
    "anthropic": "\n".join(
        [
            'event: message_start',
            'data: {"type":"message_start","message":{"id":"msg_01SSE","type":"message",'
            '"role":"assistant","model":"claude-sonnet-4-20250514","content":[],'
            '"usage":{"input_tokens":25,"output_tokens":1}}}',
            "",
            'event: content_block_start',
            'data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}',
            "",
            'event: content_block_delta',
            'data: {"type":"content_block_delta","index":0,'
            '"delta":{"type":"text_delta","text":"今天天气晴朗。"}}',
            "",
            'event: content_block_stop',
            'data: {"type":"content_block_stop","index":0}',
            "",
            'event: message_delta',
            'data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},'
            '"usage":{"output_tokens":17}}',
            "",
            'event: message_stop',
            'data: {"type":"message_stop"}',
            "",
        ]
    ),
    "openai-chat": "\n".join(
        [
            'data: {"id":"chatcmpl-SSE","object":"chat.completion.chunk",'
            '"model":"gpt-4o-2024-08-06","choices":[{"index":0,'
            '"delta":{"role":"assistant"},"finish_reason":null}]}',
            "",
            'data: {"id":"chatcmpl-SSE","object":"chat.completion.chunk",'
            '"model":"gpt-4o-2024-08-06","choices":[{"index":0,'
            '"delta":{"content":"今天天气晴朗。"},"finish_reason":null}]}',
            "",
            'data: {"id":"chatcmpl-SSE","object":"chat.completion.chunk",'
            '"model":"gpt-4o-2024-08-06","choices":[{"index":0,'
            '"delta":{},"finish_reason":"stop"}]}',
            "",
            "data: [DONE]",
            "",
        ]
    ),
    "openai-responses": "\n".join(
        [
            'data: {"type":"response.created","sequence_number":0,'
            '"response":{"id":"resp_SSE"}}',
            "",
            'data: {"type":"response.output_item.added","sequence_number":1,"output_index":0,'
            '"item":{"type":"message","role":"assistant","content":[]}}',
            "",
            'data: {"type":"response.output_text.delta","item_id":"msg_SSE","output_index":0,'
            '"content_index":0,"delta":"今天天气晴朗。","sequence_number":2}',
            "",
            'data: {"type":"response.completed","sequence_number":3,'
            '"response":{"id":"resp_SSE","status":"completed"}}',
            "",
        ]
    ),
}

DEFAULT_URLS = {
    "anthropic": "https://api.anthropic.com/v1/messages",
    "openai-chat": "https://api.openai.com/v1/chat/completions",
    "openai-responses": "https://api.openai.com/v1/responses",
}

# Typical upstream response headers for the response-side envelope; with --sse
# the content-type becomes text/event-stream.
RESPONSE_HEADERS = {
    "anthropic": {
        "content-type": "application/json",
        "request-id": "req_anthropic_test_01",
        "anthropic-version": "2023-06-01",
    },
    "openai-chat": {
        "content-type": "application/json",
        "x-request-id": "req_openai_test_01",
        "openai-processing-ms": "412",
    },
    "openai-responses": {
        "content-type": "application/json",
        "x-request-id": "resp_openai_test_01",
    },
}


def build_body(spec: str, side: str, sse: bool) -> str:
    if side == "request":
        return json.dumps(MOCK_REQUESTS[spec], ensure_ascii=False)
    if sse:
        return MOCK_SSE[spec]
    return json.dumps(MOCK_RESPONSES[spec], ensure_ascii=False)


def build_envelope(spec: str, side: str, url: str, extra: str, body_file: str | None,
                   worker_id: int, sse: bool, state: str | None) -> dict:
    """Build an envelope as specified in references/protocol.md.

    A request-side envelope carries "method"; a response-side envelope has no
    "method", which is how a format program tells the two sides apart.
    """
    if body_file:
        with open(body_file, "rb") as f:
            raw = f.read()
        try:
            body: str = raw.decode("utf-8")
            envelope: dict = {"body": body}
        except UnicodeDecodeError:
            # Not UTF-8: standard-alphabet base64 with padding (not URL-safe)
            envelope = {"body_b64": base64.b64encode(raw).decode("ascii")}
    else:
        envelope = {"body": build_body(spec, side, sse)}
    envelope["url"] = url
    if side == "request":
        envelope["method"] = "POST"
        envelope["headers"] = {
            "content-type": "application/json",
            "authorization": "Bearer test-client-key",
        }
    else:
        headers = dict(RESPONSE_HEADERS[spec])
        if sse:
            headers["content-type"] = "text/event-stream"
        envelope["headers"] = headers
    envelope["worker_id"] = worker_id
    envelope["extra"] = extra
    # aProxy after 0.1.0 also names the stage and the request; `state` is what
    # the request side left for the response side (absent until one is set)
    envelope["stage"] = side
    envelope["request_id"] = "1"
    if state is not None:
        envelope["state"] = state
    return envelope


def read_reply(stdout, timeout_secs: float) -> str | None:
    """Read one reply line with a timeout (a persistent program never exits on
    its own, so waiting for EOF is not an option)."""
    box: list[str] = []

    def reader():
        line = stdout.readline()
        box.append(line)  # empty string on EOF

    t = threading.Thread(target=reader, daemon=True)
    t.start()
    t.join(timeout_secs)
    if not box:
        return None
    return box[0]


def checkpoints(spec: str, side: str, sse: bool) -> list[str]:
    if side == "request":
        return [
            '"headers" is present (without it aProxy cannot parse the reply: 502)',
            "url is rewritten as intended (omitted = the input url is kept)",
            "the body matches the target channel's protocol (check field names and shape)",
            "auth headers: the client's originals removed, the channel's injected correctly",
        ]
    checks = [
        '"headers" is present (without it aProxy cannot parse the reply)',
        "the body is converted back to the client's protocol"
        + (" (check every SSE event; the terminating events must be intact)" if sse
           else " (check field names and shape)"),
        "credential headers are removed (these headers go straight to the client; the "
        "official aproxy-format drops authorization/x-api-key/cookie/proxy-authorization; "
        "telemetry headers are harmless)",
        "content-type matches the body"
        + (" (SSE stays text/event-stream)" if sse else " (JSON is application/json)"),
    ]
    return checks


def main() -> None:
    ap = argparse.ArgumentParser(
        description="Feed a mock request- or response-side envelope (Anthropic, OpenAI "
        "Chat or OpenAI Responses) to a format program and check its reply."
    )
    ap.add_argument(
        "--format-spec",
        choices=sorted(MOCK_REQUESTS),
        required=True,
        help="protocol of the mock body",
    )
    ap.add_argument(
        "--side",
        choices=["request", "response"],
        default="request",
        help="envelope side: request = client request (has method); "
        "response = upstream response (no method). Default: request",
    )
    ap.add_argument(
        "--sse",
        action="store_true",
        help="response side only: use an SSE event sequence as the body instead of JSON",
    )
    ap.add_argument("--command", required=True, help="format program to run")
    ap.add_argument(
        "args", nargs="*", default=[],
        help="arguments for the format program (put them after --)",
    )
    ap.add_argument("--url", default=None,
                    help="envelope url (default: the protocol's usual endpoint)")
    ap.add_argument("--extra", default="", help="envelope extra string, passed verbatim")
    ap.add_argument("--worker-id", type=int, default=0,
                    help="envelope worker_id (default 0)")
    ap.add_argument("--state", default=None,
                    help="envelope state, as the request side would have left it "
                         "(default: absent)")
    ap.add_argument(
        "--body-file", default=None,
        help="use this file as the body (non-UTF-8 content is sent as body_b64)",
    )
    ap.add_argument(
        "--raw", action="store_true", help="print the reply line as is, without parsing"
    )
    ap.add_argument(
        "--timeout", type=float, default=10.0,
        help="seconds to wait for the reply (default 10)",
    )
    ns = ap.parse_args()

    if ns.sse and ns.side != "response":
        ap.error("--sse applies only to --side response (there is no streaming request mock)")

    url = ns.url or DEFAULT_URLS[ns.format_spec]
    envelope = build_envelope(
        ns.format_spec, ns.side, url, ns.extra, ns.body_file, ns.worker_id, ns.sse,
        ns.state,
    )
    line = json.dumps(envelope, ensure_ascii=False)

    side_label = (
        "request side (client request)" if ns.side == "request"
        else "response side (upstream response)"
    ) + (" [SSE]" if ns.sse else "")
    print(f"[test] {side_label}  protocol: {ns.format_spec}  "
          f"program: {ns.command} {' '.join(ns.args)}")
    print(f"[test] envelope sent ({len(line)} characters):")
    print(json.dumps(envelope, ensure_ascii=False, indent=2))
    print("-" * 60)

    try:
        proc = subprocess.Popen(
            [ns.command, *ns.args],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            encoding="utf-8",  # text pipes with explicit UTF-8 on both ends
        )
    except OSError as e:
        print(f"[FAIL] could not start the format program: {e}")
        sys.exit(1)

    try:
        proc.stdin.write(line + "\n")
        proc.stdin.flush()
    except BrokenPipeError:
        print("[FAIL] the program exited before reading its input "
              "(check the command and arguments)")
        sys.exit(1)

    reply = read_reply(proc.stdout, ns.timeout)
    if reply is None:
        print(f"[FAIL] no reply within {ns.timeout:.0f}s: the program hangs, writes nothing "
              "to stdout, or does not flush")
        proc.kill()
        sys.exit(1)
    if not reply.strip():
        print("[FAIL] stdout closed without a reply line (the program died; look for a crash)")
        sys.exit(1)

    if ns.raw:
        print(reply.rstrip("\n"))
    else:
        print("[test] reply (pretty-printed):")
        try:
            parsed = json.loads(reply)
            print(json.dumps(parsed, ensure_ascii=False, indent=2))
            problems = envelope_problems(parsed)
            if problems:
                for problem in problems:
                    print(f"[FAIL] aProxy would reject this reply: {problem}")
                sys.exit(1)
            if parsed.get("error") is not None:
                # aProxy treats any non-null `error`, even an empty string, as a failure
                print(f'\n[WARN] error reply: {parsed["error"]!r} (aProxy would answer 502 on '
                      'the request side, or pass the upstream response through on the '
                      'response side)')
            else:
                print(f"\n[OK] the reply is valid JSON. Check by hand ({side_label}):")
                for c in checkpoints(ns.format_spec, ns.side, ns.sse):
                    print(f"  - {c}")
        except json.JSONDecodeError as e:
            print(f"[FAIL] the reply is not valid JSON: {e}\nraw line: {reply.rstrip()}")
            sys.exit(1)

    # Persistent-mode check: a program that is still running is waiting for the
    # next line, as a loop should. Closing its stdin must make it exit.
    if proc.poll() is None:
        print("[test] the program is still running (correct for persistent mode); "
              "closing its stdin now, it should exit")
    proc.stdin.close()
    try:
        proc.wait(timeout=5)
        print(f"[test] the program exited on stdin EOF (exit={proc.returncode}): EOF duty met")
    except subprocess.TimeoutExpired:
        print("[WARN] still running 5s after stdin closed: the program ignores EOF, so a "
              "persistent worker would be left behind if aProxy exits without cleaning up")
        proc.kill()


def envelope_problems(env) -> list:
    """Reasons aProxy would reject this reply line, mirroring its envelope parser
    (aproxy-envelope): an object with a `headers` map of strings, `body` and
    `body_b64` not both present, `body_b64` in standard base64."""
    if not isinstance(env, dict):
        return ["the reply must be a JSON object (an envelope), not "
                f"{type(env).__name__}"]
    problems = []
    headers = env.get("headers")
    if headers is None:
        problems.append("missing `headers` (the only required field; send {} if empty)")
    elif not isinstance(headers, dict) or not all(
            isinstance(k, str) and isinstance(v, str) for k, v in headers.items()):
        problems.append("`headers` must map header names to string values")
    if env.get("body") is not None and env.get("body_b64") is not None:
        problems.append("`body` and `body_b64` are mutually exclusive")
    if env.get("body") is not None and not isinstance(env["body"], str):
        problems.append("`body` must be a string")
    for key in ("stage", "request_id", "state"):
        if env.get(key) is not None and not isinstance(env[key], str):
            problems.append(f"`{key}` must be a string or null")
    if env.get("body_b64") is not None:
        try:
            base64.b64decode(env["body_b64"], validate=True)
        except (ValueError, TypeError):
            problems.append("`body_b64` is not standard base64 with padding")
    return problems


if __name__ == "__main__":
    main()
