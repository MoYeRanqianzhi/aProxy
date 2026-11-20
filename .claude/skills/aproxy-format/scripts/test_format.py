#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Protocol 1 test harness for aProxy format programs. No aProxy instance needed.

It plays aProxy's side of the pipe: it starts the program, sends the `hello`
line, checks the reply, then sends one realistic message for each stage the
program supports and checks every reply against the protocol rules (one line,
echoed `seq`, field shapes). Finally it closes stdin and checks that the
program exits, which is the duty of every worker.

  # Everything the program declares in its hello reply
  python test_format.py --command ./my-format

  # Only some stages; arguments after "--" go to the program verbatim
  python test_format.py --stage request,retry --command python -- my_format.py

  # The official roles
  python test_format.py --command aproxy-format -- heartbeat
  python test_format.py --command aproxy-format -- policy --config policy.toml
  python test_format.py --protocol openai-chat --upstream anthropic \
      --command aproxy-format -- convert --from openai_chat --to anthropic_messages

  # spawn mode: a fresh process per message, each one greeted with hello
  python test_format.py --process spawn --command ./my-format

  # Your own request body (sent as body_b64 when it is not UTF-8), and -v to
  # print every frame in both directions
  python test_format.py -v --body-file payload.json --command ./my-format

--protocol picks the mock client traffic: anthropic (Messages), openai-chat or
openai-responses. --upstream picks the protocol of the mock upstream responses
and streams when it differs (testing a converter). --url sets the upstream URL
the request is addressed to; programs that pick a key or channel by origin
need one they know.

Exit code 0 means every check passed, 1 means at least one failed. Warnings do
not change the exit code.

Windows: this script forces UTF-8 on its own pipes. The program under test
must do the same on its stdin and stdout (see references/guide.md).
"""

import argparse
import base64
import hashlib
import json
import os
import re
import subprocess
import sys
import threading

if hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(encoding="utf-8")
    sys.stderr.reconfigure(encoding="utf-8")

# Stages a protocol 1 instance can send. `hello` is not in the list: it always comes first.
STAGES = ["request", "retry", "decide", "response", "stream", "heartbeat", "notify"]
# Stages whose messages can carry the client's request body, and the grade a
# program gets when its hello reply does not name the stage.
BODY_DEFAULT = {"request": "full", "retry": "full", "decide": "none", "notify": "none"}
MAX_DELAY_MS = 86_400_000
MAX_STATE = 64 * 1024
MAX_HELLO_REPLY = 64 * 1024

# ---- Mock traffic ------------------------------------------------------------
# The Chinese text is deliberate: every run then exercises a non-ASCII UTF-8
# round trip, the most common encoding failure on Windows.

REQUESTS = {
    "anthropic": {
        "model": "claude-sonnet-4-20250514",
        "system": "You are a helpful assistant.",
        "messages": [{"role": "user", "content": "今天天气怎么样？"}],
        "max_tokens": 256,
        "stream": True,
    },
    "openai-chat": {
        "model": "gpt-4o",
        "messages": [
            {"role": "system", "content": "You are a helpful assistant."},
            {"role": "user", "content": "今天天气怎么样？"},
        ],
        "max_completion_tokens": 256,
        "stream": True,
    },
    "openai-responses": {
        "model": "gpt-4o",
        "instructions": "You are a helpful assistant.",
        "input": "今天天气怎么样？",
        "max_output_tokens": 256,
        "stream": True,
    },
}

RESPONSES = {
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
                "status": "completed",
                "content": [{"type": "output_text", "text": "今天天气晴朗。", "annotations": []}],
            }
        ],
        "usage": {"input_tokens": 25, "output_tokens": 9, "total_tokens": 34},
    },
}


def _sse(events):
    """Render (event name or None, JSON text) pairs as SSE."""
    out = []
    for name, data in events:
        if name:
            out.append("event: " + name)
        out.append("data: " + data)
        out.append("")
    return "\n".join(out) + "\n"


STREAMS = {
    "anthropic": _sse(
        [
            (
                "message_start",
                '{"type":"message_start","message":{"id":"msg_01SSE","type":"message",'
                '"role":"assistant","model":"claude-sonnet-4-20250514","content":[],'
                '"stop_reason":null,"stop_sequence":null,'
                '"usage":{"input_tokens":25,"output_tokens":1}}}',
            ),
            (
                "content_block_start",
                '{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}',
            ),
            (
                "content_block_delta",
                '{"type":"content_block_delta","index":0,'
                '"delta":{"type":"text_delta","text":"今天天气晴朗。"}}',
            ),
            ("content_block_stop", '{"type":"content_block_stop","index":0}'),
            (
                "message_delta",
                '{"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},'
                '"usage":{"output_tokens":17}}',
            ),
            ("message_stop", '{"type":"message_stop"}'),
        ]
    ),
    "openai-chat": _sse(
        [
            (
                None,
                '{"id":"chatcmpl-SSE","object":"chat.completion.chunk","created":1749000000,'
                '"model":"gpt-4o-2024-08-06","choices":[{"index":0,'
                '"delta":{"role":"assistant","content":""},"finish_reason":null}]}',
            ),
            (
                None,
                '{"id":"chatcmpl-SSE","object":"chat.completion.chunk","created":1749000000,'
                '"model":"gpt-4o-2024-08-06","choices":[{"index":0,'
                '"delta":{"content":"今天天气晴朗。"},"finish_reason":null}]}',
            ),
            (
                None,
                '{"id":"chatcmpl-SSE","object":"chat.completion.chunk","created":1749000000,'
                '"model":"gpt-4o-2024-08-06","choices":[{"index":0,'
                '"delta":{},"finish_reason":"stop"}]}',
            ),
            (None, "[DONE]"),
        ]
    ),
    "openai-responses": _sse(
        [
            (
                "response.created",
                '{"type":"response.created","sequence_number":0,"response":{"id":"resp_SSE",'
                '"object":"response","status":"in_progress","model":"gpt-4o-2024-08-06","output":[]}}',
            ),
            (
                "response.output_item.added",
                '{"type":"response.output_item.added","sequence_number":1,"output_index":0,'
                '"item":{"id":"msg_SSE","type":"message","role":"assistant","status":"in_progress",'
                '"content":[]}}',
            ),
            (
                "response.output_text.delta",
                '{"type":"response.output_text.delta","sequence_number":2,"item_id":"msg_SSE",'
                '"output_index":0,"content_index":0,"delta":"今天天气晴朗。"}',
            ),
            (
                "response.completed",
                '{"type":"response.completed","sequence_number":3,"response":{"id":"resp_SSE",'
                '"object":"response","status":"completed","model":"gpt-4o-2024-08-06","output":['
                '{"id":"msg_SSE","type":"message","role":"assistant","status":"completed","content":['
                '{"type":"output_text","text":"今天天气晴朗。","annotations":[]}]}],'
                '"usage":{"input_tokens":25,"output_tokens":9,"total_tokens":34}}}',
            ),
        ]
    ),
}

URLS = {
    "anthropic": "https://api.anthropic.com/v1/messages",
    "openai-chat": "https://api.openai.com/v1/chat/completions",
    "openai-responses": "https://api.openai.com/v1/responses",
}
PATHS = {
    "anthropic": "/v1/messages",
    "openai-chat": "/v1/chat/completions",
    "openai-responses": "/v1/responses",
}
# What an instance offers in hello: the config keys a reply may override per request.
OVERRIDES = [
    "timeouts.connect_secs",
    "timeouts.first_byte_secs",
    "timeouts.read_idle_secs",
    "keepalive.interval_secs",
    "keepalive.trigger",
    "keepalive.heartbeat",
    "retry.max_backoff_secs",
    "retry.max_attempts",
]

# ---- Result bookkeeping ------------------------------------------------------


class Results:
    """Collects failures and warnings so one run reports everything it found."""

    def __init__(self):
        self.failed = 0
        self.warned = 0

    def ok(self, text):
        print("  ok    " + text)

    def fail(self, text):
        self.failed += 1
        print("  FAIL  " + text)

    def warn(self, text):
        self.warned += 1
        print("  warn  " + text)


class Dead(Exception):
    """The program stopped answering; nothing more can be tested on this process."""


# ---- The process under test --------------------------------------------------


class Worker:
    """One process of the program under test, driven line by line like aProxy does."""

    def __init__(self, argv, timeout, verbose, res):
        self.timeout, self.verbose, self.res = timeout, verbose, res
        self.seq = 0
        self.proc = subprocess.Popen(
            argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE
        )
        self.stderr = []
        # Drain stderr on a thread: a program that logs a lot would otherwise
        # block on a full pipe and look like a hang.
        self._err = threading.Thread(target=self._drain, daemon=True)
        self._err.start()

    def _drain(self):
        for raw in self.proc.stderr:
            self.stderr.append(raw.decode("utf-8", "replace").rstrip())

    def _readline(self):
        """Read one stdout line with a deadline; None on timeout, b'' on EOF."""
        box = []
        t = threading.Thread(target=lambda: box.append(self.proc.stdout.readline()), daemon=True)
        t.start()
        t.join(self.timeout)
        return box[0] if box else None

    def exchange(self, message):
        """Send one message and return the parsed reply object, or raise Dead."""
        message = dict(message, v=1, seq=self.seq)
        line = json.dumps(message, ensure_ascii=False, separators=(",", ":"))
        if self.verbose:
            print("  ->    " + line)
        try:
            self.proc.stdin.write(line.encode("utf-8") + b"\n")
            self.proc.stdin.flush()
        except OSError:
            raise Dead("the program closed its stdin (did it exit?)")
        raw = self._readline()
        if raw is None:
            raise Dead(
                "no reply within %ss: the program did not write a line, or did not flush it"
                % self.timeout
            )
        if raw == b"":
            raise Dead("the program exited without replying")
        if not raw.endswith(b"\n"):
            raise Dead("the reply does not end with a newline (the program exited mid-line)")
        try:
            text = raw.decode("utf-8")
        except UnicodeDecodeError:
            raise Dead("the reply is not UTF-8 (on Windows, set stdout to UTF-8 explicitly)")
        if self.verbose:
            print("  <-    " + text.rstrip("\n"))
        try:
            reply = json.loads(text)
        except ValueError:
            raise Dead("the reply is not JSON: %r" % text[:200])
        if not isinstance(reply, dict):
            raise Dead("the reply is not a JSON object")
        if reply.get("v") != 1:
            self.res.fail('reply "v" must be 1, got %r' % (reply.get("v"),))
        if reply.get("seq") != self.seq:
            self.res.fail(
                'reply "seq" must echo %d, got %r (aProxy discards the worker on a mismatch)'
                % (self.seq, reply.get("seq"))
            )
        self.seq += 1
        reply["_bytes"] = len(raw)
        return reply

    def close(self):
        """Close stdin and check that the program exits and wrote nothing extra."""
        try:
            self.proc.stdin.close()
        except OSError:
            pass
        try:
            self.proc.wait(self.timeout)
        except subprocess.TimeoutExpired:
            self.res.fail(
                "the program did not exit within %ss of stdin closing; a worker must exit on EOF"
                % self.timeout
            )
            self.proc.kill()
            self.proc.wait()
            return
        extra = self.proc.stdout.read()
        if extra.strip():
            self.res.fail(
                "extra output on stdout after the last reply (%d bytes); stdout carries replies only"
                % len(extra)
            )
        else:
            self.res.ok("exited on EOF (exit code %d)" % self.proc.returncode)

    def kill(self):
        if self.proc.poll() is None:
            self.proc.kill()
            self.proc.wait()


# ---- Reply checks ------------------------------------------------------------


def given(obj, key):
    """A field written as null means the same as leaving it out."""
    return obj.get(key) is not None


def check_bytes(res, obj, text_key, b64_key, where):
    """`body`/`body_b64` and `data`/`data_b64` are two spellings; never both."""
    if given(obj, text_key) and given(obj, b64_key):
        res.fail("%s: %s and %s are mutually exclusive" % (where, text_key, b64_key))
    if given(obj, text_key) and not isinstance(obj[text_key], str):
        res.fail("%s: %s must be a string" % (where, text_key))
    if given(obj, b64_key):
        try:
            base64.b64decode(obj[b64_key], validate=True)
        except (ValueError, TypeError):
            res.fail("%s: %s is not standard base64 with padding" % (where, b64_key))


def check_headers(res, obj, where):
    if not given(obj, "headers"):
        return
    h = obj["headers"]
    if not isinstance(h, dict) or not all(isinstance(v, str) for v in h.values()):
        res.fail("%s: headers must be an object of string values" % where)
        return
    upper = [k for k in h if k != k.lower()]
    if upper:
        res.warn("%s: header names should be lowercase: %s" % (where, ", ".join(upper)))


def check_response_object(res, obj, where, patch):
    """A response patch (omitted fields inherit) or a complete response (fail.response)."""
    if not isinstance(obj, dict):
        res.fail("%s must be an object" % where)
        return
    if given(obj, "status"):
        s = obj["status"]
        if not isinstance(s, int) or isinstance(s, bool) or not 100 <= s <= 999:
            res.fail("%s: status %r is not an HTTP status" % (where, s))
    elif not patch:
        res.fail("%s: a complete response needs a status" % where)
    check_headers(res, obj, where)
    check_bytes(res, obj, "body", "body_b64", where)


def check_common(res, reply, stage):
    """Checks that apply to every stage reply."""
    if given(reply, "state"):
        s = reply["state"]
        if not isinstance(s, str):
            res.fail("state must be a string (serialize your own JSON into it)")
        elif len(s.encode("utf-8")) > MAX_STATE:
            res.fail("state is %d bytes; the limit is 64 KiB" % len(s.encode("utf-8")))
    if given(reply, "report"):
        r = reply["report"]
        if (
            not isinstance(r, dict)
            or not isinstance(r.get("summary"), str)
            or not isinstance(r.get("actions"), list)
        ):
            res.warn('report is not {"summary": string, "actions": [...]}; aProxy drops it')
        else:
            if len(r["summary"]) > 512:
                res.warn("report.summary is longer than 512 characters")
            for a in r["actions"]:
                if not isinstance(a, dict) or not isinstance(a.get("type"), str):
                    res.warn("a report action has no string type; aProxy drops that action")
                elif a["type"] == "custom" and not isinstance(a.get("label"), str):
                    res.warn("a custom report action needs a label; aProxy drops that action")
    if given(reply, "fail"):
        f = reply["fail"]
        if (
            not isinstance(f, dict)
            or not isinstance(f.get("retry"), bool)
            or not isinstance(f.get("reason"), str)
        ):
            res.fail('fail must be {"retry": bool, "reason": string, ...}')
        else:
            if len(f["reason"]) > 512:
                res.warn("fail.reason is longer than 512 characters")
            if given(f, "code") and not (
                isinstance(f["code"], str) and re.fullmatch(r"[A-Za-z0-9_.-]{1,64}", f["code"])
            ):
                res.warn("fail.code is not 1-64 of [A-Za-z0-9_.-]; aProxy drops the code")
            if given(f, "response"):
                check_response_object(res, f["response"], "fail.response", patch=False)
    if given(reply, "overrides") and not isinstance(reply["overrides"], dict):
        res.fail("overrides must be an object")
    if given(reply, "delivery") and stage != "request":
        res.warn("delivery is only read from a request reply; here it is ignored")


def check_request_reply(res, reply, stage):
    if given(reply, "delivery") and reply["delivery"] not in ("forward", "retry"):
        res.warn('delivery %r is not "forward" or "retry"; aProxy ignores it' % (reply["delivery"],))
    if not given(reply, "request"):
        return
    p = reply["request"]
    if not isinstance(p, dict):
        res.fail("request must be an object (a patch)")
        return
    if given(p, "url") and not re.match(r"^https?://[^/\s]+", str(p["url"])):
        res.fail("request.url must be an absolute http or https URL, got %r" % (p["url"],))
    if given(p, "method") and not (isinstance(p["method"], str) and p["method"]):
        res.fail("request.method must be a non-empty string")
    check_headers(res, p, "request")
    check_bytes(res, p, "body", "body_b64", "request")


def check_decide_reply(res, reply):
    if given(reply, "decision") and reply["decision"] not in ("retry", "stop"):
        res.fail('decision must be "retry" or "stop", got %r' % (reply["decision"],))
    if given(reply, "delay_ms"):
        d = reply["delay_ms"]
        if not isinstance(d, int) or isinstance(d, bool) or not 0 <= d <= MAX_DELAY_MS:
            res.fail("delay_ms must be an integer from 0 to %d, got %r" % (MAX_DELAY_MS, d))
    if given(reply, "reroute") and not isinstance(reply["reroute"], bool):
        res.fail("reroute must be a boolean")
    if given(reply, "response"):
        check_response_object(res, reply["response"], "response", patch=True)


def check_heartbeat_reply(res, reply):
    check_bytes(res, reply, "data", "data_b64", "heartbeat")
    if given(reply, "data") and isinstance(reply["data"], str):
        d = reply["data"]
        if d and not d.endswith("\n\n"):
            res.warn(
                "heartbeat data does not end on an SSE event boundary (blank line); "
                "the client's parser would merge it with the next event"
            )
    if given(reply, "state"):
        res.warn("state in a heartbeat reply is ignored")


def describe(reply):
    """One-line summary of what a reply did, for the non-verbose listing."""
    keys = [k for k in reply if k not in ("v", "seq", "_bytes") and reply[k] is not None]
    return ", ".join(keys) if keys else "no change"


# ---- Messages ----------------------------------------------------------------


class Scenario:
    """The mock request and what the harness remembers between stages."""

    def __init__(self, args):
        self.protocol = args.protocol
        # Responses come from the upstream, which may speak another protocol
        # than the client when the program under test converts between them.
        self.upstream = args.upstream or args.protocol
        self.url = args.url or URLS[args.protocol]
        self.path = args.path or PATHS[args.protocol]
        if args.body_file:
            with open(args.body_file, "rb") as f:
                self.body = f.read()
        else:
            self.body = json.dumps(REQUESTS[args.protocol], ensure_ascii=False).encode("utf-8")
        self.digest = hashlib.sha256(self.body).hexdigest()
        self.request_id = "19ac3f2e8b5d1a2b-42"
        self.state = None

    def remember(self, reply):
        """Apply the state rule: a string replaces, the empty string clears, absence keeps."""
        s = reply.get("state")
        if isinstance(s, str):
            self.state = s or None

    def context(self, need):
        m = {"request_id": self.request_id}
        if self.state is not None:
            m["state"] = self.state
        if need == "digest":
            m["digest"] = self.digest
        return m

    def body_fields(self, need):
        if need != "full":
            return {}
        try:
            return {"body": self.body.decode("utf-8")}
        except UnicodeDecodeError:
            return {"body_b64": base64.b64encode(self.body).decode("ascii")}

    def client(self):
        return {"path": self.path, "stream": True}

    def request(self, need):
        req = {
            "method": "POST",
            "url": self.url,
            "headers": {
                "content-type": "application/json",
                "authorization": "Bearer sk-test-harness-not-a-real-key",
            },
        }
        req.update(self.body_fields(need))
        m = {"stage": "request", "delivery": "retry", "request": req, "client": self.client()}
        m.update(self.context(need))
        return m

    def retry(self, need):
        m = self.request(need)
        m["stage"] = "retry"
        m["retry"] = {
            "attempt": 2,
            "last_failure": {"code": "upstream_status", "details": {"status": 429}},
            "last_response": {
                "status": 429,
                "headers": {"content-type": "application/json", "retry-after": "2"},
            },
            "elapsed_ms": 1200,
        }
        return m

    def decide(self, need):
        req = {"method": "POST", "url": self.url}
        req.update(self.body_fields(need))
        m = {
            "stage": "decide",
            "client": self.client(),
            "attempt": 1,
            "elapsed_ms": 800,
            "delivery": "retry",
            "outcome": "failure",
            "failure": {"code": "upstream_status", "details": {"status": 429}},
            "request": req,
            "response": {
                "status": 429,
                "headers": {"content-type": "application/json", "retry-after": "2"},
                "body": '{"error":{"message":"rate limited","type":"rate_limit_error"}}',
                "truncated": None,
            },
            "fallback": {"decision": "retry", "by": "default", "delay_ms": 0},
        }
        m.update(self.context(need))
        return m

    def response(self):
        m = {
            "stage": "response",
            "client": self.client(),
            "request": {"method": "POST", "url": self.url},
            "response": {
                "status": 200,
                "headers": {"content-type": "application/json"},
                "body": json.dumps(RESPONSES[self.upstream], ensure_ascii=False),
            },
        }
        m.update(self.context("none"))
        return m

    def stream_open(self):
        m = {
            "stage": "stream",
            "event": "open",
            "stream_id": self.request_id + "/1",
            "client": self.client(),
            "request": {"method": "POST", "url": self.url},
            "response": {"status": 200, "headers": {"content-type": "text/event-stream"}},
        }
        m.update(self.context("none"))
        return m

    def stream_event(self, event, data=None):
        m = {"stage": "stream", "event": event, "stream_id": self.request_id + "/1"}
        if data is not None:
            m["data"] = data
        return m

    def heartbeat(self):
        m = {
            "stage": "heartbeat",
            "heartbeat": {"beat": 1, "elapsed_ms": 15000, "attempt": 1},
            "client": self.client(),
            "data": ": keepalive\n\n",
        }
        m.update(self.context("none"))
        return m

    def notify(self, need):
        m = {
            "stage": "notify",
            "event": "request_end",
            "client": self.client(),
            "delivery": "retry",
            "upgraded": False,
            "attempts": 2,
            "upstream_status": 200,
            "stream_completed": True,
            "duration_ms": 4200,
            "outcome": "ok",
            "code": None,
        }
        m.update(self.context(need))
        return m


def hello_message(process, stages):
    return {
        "stage": "hello",
        "protocols": [1],
        "aproxy_version": "0.2.0",
        "name": "test-format",
        "process": process,
        "stages": stages,
        "worker_id": 0,
        "features": ["decide.success"],
        "limits": {"hello": MAX_HELLO_REPLY, "control": 1024 * 1024},
        "overrides": OVERRIDES,
    }


def check_hello(res, reply, wanted):
    """Validate the hello reply; return (supported stages, body needs) or None."""
    good = True
    if reply.get("protocol") != 1:
        res.fail('hello reply "protocol" must be 1, got %r' % (reply.get("protocol"),))
        good = False
    stages = reply.get("stages")
    if not isinstance(stages, list) or not all(isinstance(s, str) for s in stages):
        res.fail('hello reply "stages" must be a list of strings')
        return None
    for key, kind in (("features", list), ("name", str), ("version", str)):
        if not isinstance(reply.get(key), kind):
            res.fail('hello reply "%s" is required (%s)' % (key, kind.__name__))
            good = False
    needs = dict(BODY_DEFAULT)
    declared = reply.get("request_body") or {}
    if not isinstance(declared, dict):
        res.fail('hello reply "request_body" must be an object')
        good = False
        declared = {}
    for stage, need in declared.items():
        if stage not in BODY_DEFAULT:
            continue  # ignored for other stages, by the protocol
        if need not in ("none", "digest", "full") or (stage == "notify" and need == "full"):
            res.fail('request_body.%s = %r is not allowed' % (stage, need))
            good = False
        else:
            needs[stage] = need
    if reply["_bytes"] > MAX_HELLO_REPLY:
        res.fail("hello reply is %d bytes; the limit is 64 KiB" % reply["_bytes"])
        good = False
    missing = [s for s in wanted if s not in stages]
    if missing:
        res.fail(
            "the program does not support: %s (an instance configured with them would not start)"
            % ", ".join(missing)
        )
    if good:
        res.ok("hello: stages %s" % (", ".join(stages) or "(none)"))
    return (stages, needs) if good else None


# ---- Driving one stage -------------------------------------------------------


def run_stage(worker, res, sc, stage, needs):
    """Send the messages of one stage on an already greeted worker and check the replies."""
    if stage in ("request", "retry"):
        msg = sc.request(needs[stage]) if stage == "request" else sc.retry(needs[stage])
        reply = worker.exchange(msg)
        check_common(res, reply, stage)
        check_request_reply(res, reply, stage)
        sc.remember(reply)
        res.ok("%s: %s" % (stage, describe(reply)))
    elif stage == "decide":
        reply = worker.exchange(sc.decide(needs["decide"]))
        check_common(res, reply, stage)
        check_decide_reply(res, reply)
        sc.remember(reply)
        vote = reply.get("decision") or "abstains"
        res.ok("decide (429 with retry-after: 2): %s" % vote)
    elif stage == "response":
        reply = worker.exchange(sc.response())
        check_common(res, reply, stage)
        if given(reply, "response"):
            check_response_object(res, reply["response"], "response", patch=True)
        sc.remember(reply)
        res.ok("response: %s" % describe(reply))
    elif stage == "stream":
        text = STREAMS[sc.upstream]
        cut = len(text) // 2  # split mid-event: chunk borders are not event borders
        steps = [
            ("open", sc.stream_open()),
            ("data", sc.stream_event("data", text[:cut])),
            ("data", sc.stream_event("data", text[cut:])),
            ("close", sc.stream_event("close")),
        ]
        out = 0
        for name, msg in steps:
            reply = worker.exchange(msg)
            check_common(res, reply, stage)
            check_bytes(res, reply, "data", "data_b64", "stream " + name)
            if name == "open" and given(reply, "response"):
                check_response_object(res, reply["response"], "stream open response", patch=True)
            if given(reply, "abort_tail") and not isinstance(reply["abort_tail"], str):
                res.fail("abort_tail must be a string")
            if isinstance(reply.get("data"), str):
                out += len(reply["data"].encode("utf-8"))
            sc.remember(reply)
        res.ok("stream: open, 2 data, close; %d bytes of text out" % out)
    elif stage == "heartbeat":
        reply = worker.exchange(sc.heartbeat())
        check_common(res, reply, stage)
        check_heartbeat_reply(res, reply)
        res.ok("heartbeat: %s" % ("replaced the bytes" if given(reply, "data") or given(reply, "data_b64") else "kept the input"))
    elif stage == "notify":
        reply = worker.exchange(sc.notify(needs["notify"]))
        check_common(res, reply, stage)
        sc.remember(reply)
        res.ok("notify: answered")


def greet(argv, args, res, stages):
    """Start a process and complete the handshake; return (worker, hello result)."""
    worker = Worker(argv, args.timeout, args.verbose, res)
    try:
        reply = worker.exchange(hello_message(args.process, stages))
    except Dead as e:
        res.fail("hello: %s" % e)
        worker.kill()
        return worker, None
    return worker, check_hello(res, reply, stages)


def main():
    ap = argparse.ArgumentParser(
        description="Protocol 1 test harness for aProxy format programs.",
        epilog="Arguments after -- are passed to the program verbatim.",
    )
    ap.add_argument("--command", required=True, help="program to test (path or name on PATH)")
    ap.add_argument(
        "--stage",
        default="all",
        help="comma-separated stages to test, or 'all' for every stage the program declares",
    )
    ap.add_argument("--protocol", choices=sorted(REQUESTS), default="anthropic")
    ap.add_argument(
        "--upstream",
        choices=sorted(REQUESTS),
        help="protocol of the mock upstream responses (default: same as --protocol)",
    )
    ap.add_argument("--process", choices=["persistent", "spawn"], default="persistent")
    ap.add_argument("--url", help="upstream URL of the mock request")
    ap.add_argument("--path", help="client path of the mock request, as the client sent it")
    ap.add_argument("--body-file", help="file whose bytes become the request body")
    ap.add_argument("--timeout", type=float, default=10.0, help="seconds to wait for each reply")
    ap.add_argument("-v", "--verbose", action="store_true", help="print every frame")
    ap.add_argument("rest", nargs=argparse.REMAINDER, help=argparse.SUPPRESS)
    args = ap.parse_args()
    rest = args.rest[1:] if args.rest[:1] == ["--"] else args.rest
    # Windows cannot start a relative path written with forward slashes; resolve
    # a command that names an existing file, and leave bare names to PATH.
    command = os.path.abspath(args.command) if os.path.isfile(args.command) else args.command
    argv = [command] + rest

    wanted = [] if args.stage == "all" else [s.strip() for s in args.stage.split(",") if s.strip()]
    unknown = [s for s in wanted if s not in STAGES]
    if unknown:
        ap.error("unknown stage: %s (known: %s)" % (", ".join(unknown), ", ".join(STAGES)))

    res = Results()
    sc = Scenario(args)
    print("program: %s" % " ".join(argv))
    print(
        "process: %s, client protocol: %s, upstream protocol: %s"
        % (args.process, sc.protocol, sc.upstream)
    )

    try:
        worker, hello = greet(argv, args, res, wanted)
    except OSError as e:
        print("  FAIL  cannot start the program: %s" % e)
        return 1
    if hello:
        supported, needs = hello
        todo = [s for s in STAGES if s in supported and (not wanted or s in wanted)]
        if "stream" in todo and args.process == "spawn":
            res.warn("stream needs process = \"persistent\"; skipping it in spawn mode")
            todo.remove("stream")
        if "notify" in todo and args.process == "spawn":
            res.warn("notify needs process = \"persistent\"; skipping it in spawn mode")
            todo.remove("notify")
        if not todo:
            res.warn("nothing to test: the program declares none of the requested stages")
        if args.process == "persistent":
            # One long-lived worker answers everything, in order.
            try:
                for stage in todo:
                    run_stage(worker, res, sc, stage, needs)
            except Dead as e:
                res.fail(str(e))
                worker.kill()
            else:
                worker.close()
        else:
            # spawn: the probe process only shakes hands; each message then gets
            # its own process, greeted with hello first.
            worker.close()
            for stage in todo:
                w, again = greet(argv, args, res, wanted)
                if not again:
                    continue
                try:
                    run_stage(w, res, sc, stage, needs)
                except Dead as e:
                    res.fail(str(e))
                    w.kill()
                else:
                    w.close()
                worker = w

    if worker.stderr:
        print("stderr of the program (aProxy writes these lines to the instance log):")
        for line in worker.stderr[-20:]:
            print("  | " + line)
    print(
        "result: %s (%d failed, %d warnings)"
        % ("PASS" if not res.failed else "FAIL", res.failed, res.warned)
    )
    return 1 if res.failed else 0


if __name__ == "__main__":
    sys.exit(main())
