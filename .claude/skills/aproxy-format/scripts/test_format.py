#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""format 联调测试器：不经 aproxy，直接把模拟信封喂给被测 format，格式化打印
回信封——写完 format 第一件事就是跑它。

六种模拟（三协议 × 请求/响应侧，body 均为官方 API 文档核对过的真实形态）：

  # 请求侧：模拟客户端请求信封（带 method），测请求转换
  python test_format.py --format-spec anthropic --command ./my-format

  # 响应侧：模拟上游响应信封（无 method——响应侧标志），测响应反向转换
  python test_format.py --side response --format-spec openai-chat --command ./my-format

  # 响应侧 SSE：body 为真实流式事件序列文本（aproxy 整缓冲后 format 看到的就是它）
  python test_format.py --side response --sse --format-spec anthropic --command ./my-format

  # 参数透传给被测程序（-- 之后原样），--body-file 自定义 body（非 UTF-8 自动走 body_b64）
  python test_format.py --format-spec openai-chat --command node -- format-node.js
  python test_format.py --format-spec anthropic --command ./fmt --body-file payload.bin

注意（Windows）：本脚本强制 UTF-8 stdin/stdout；被测 format 也必须按
guide.md 处理自身编码（脚本语言的 stdout 默认代码页是高频死法）。
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

# ---- 模拟请求体（对齐各协议官方文档的真实请求形态） -------------------------

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

# ---- 模拟响应体（对齐各协议官方文档的真实响应形态） -------------------------

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

# ---- 模拟 SSE 流式响应文本（真实事件序列；aproxy 整缓冲后 format 收到的整段文本） ---

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

# 真实上游响应头（响应侧信封 headers；SSE 场景 content-type 为 event-stream）
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
                   worker_id: int, sse: bool) -> dict:
    """按 skill protocol.md 的 JSON 完整规范组装信封。

    请求侧带 method；响应侧无 method（协议约定的响应侧标志）。
    """
    if body_file:
        with open(body_file, "rb") as f:
            raw = f.read()
        try:
            body: str = raw.decode("utf-8")
            envelope: dict = {"body": body}
        except UnicodeDecodeError:
            # 非 UTF-8：标准字母表 + padding 的 base64（URL-safe 变体不行）
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
    return envelope


def read_reply(stdout, timeout_secs: float) -> str | None:
    """带超时读一行回信（persistent format 不退出，不能等 EOF）。"""
    box: list[str] = []

    def reader():
        line = stdout.readline()
        box.append(line)  # EOF 时为空串

    t = threading.Thread(target=reader, daemon=True)
    t.start()
    t.join(timeout_secs)
    if not box:
        return None
    return box[0]


def checkpoints(spec: str, side: str, sse: bool) -> list[str]:
    if side == "request":
        return [
            "headers 必含（缺键 = 解析失败 502）",
            "url 是否按预期改写（缺省 = 沿用输入 url）",
            "body 转换产物是否符合目标渠道协议（字段名/形态逐项核对）",
            "鉴权头：客户端原头是否剥离、渠道头是否正确注入",
        ]
    checks = [
        "headers 必含（缺键 = 解析失败 502）",
        "body 反向转换产物是否对齐客户端协议"
        + ("（SSE 事件序列逐帧核对，终止事件必须完整）" if sse else "（字段名/形态逐项核对）"),
        "鉴权/凭据类头是否剥离（响应头表直接回放客户端；官方 aproxy-format 自动"
        "剔除 authorization/x-api-key/cookie/proxy-authorization，遥测头无害可留）",
        "content-type 是否与 body 形态一致"
        + ("（SSE 应保持 text/event-stream）" if sse else "（JSON 应为 application/json）"),
    ]
    return checks


def main() -> None:
    ap = argparse.ArgumentParser(
        description="format 联调测试器：模拟三协议格式的请求/响应信封喂给被测 format"
    )
    ap.add_argument(
        "--format-spec",
        choices=sorted(MOCK_REQUESTS),
        required=True,
        help="模拟的协议格式",
    )
    ap.add_argument(
        "--side",
        choices=["request", "response"],
        default="request",
        help="信封方向：request=客户端请求（带 method）；response=上游响应（无 method）",
    )
    ap.add_argument(
        "--sse",
        action="store_true",
        help="响应侧专用：body 用真实 SSE 流式事件序列文本（替代 JSON 响应）",
    )
    ap.add_argument("--command", required=True, help="被测 format 程序命令")
    ap.add_argument(
        "args", nargs="*", default=[], help="format 程序参数（-- 之后的原样透传）"
    )
    ap.add_argument("--url", default=None, help="信封 url（缺省按协议给典型端点）")
    ap.add_argument("--extra", default="", help="信封 extra 原样透传")
    ap.add_argument("--worker-id", type=int, default=0, help="池槽位号（默认 0）")
    ap.add_argument(
        "--body-file", default=None, help="自定义 body 文件（非 UTF-8 自动走 body_b64）"
    )
    ap.add_argument(
        "--raw", action="store_true", help="只打印回行原文（不格式化）"
    )
    ap.add_argument(
        "--timeout", type=float, default=10.0, help="等待回行秒数（默认 10）"
    )
    ns = ap.parse_args()

    if ns.sse and ns.side != "response":
        ap.error("--sse 仅用于 --side response（请求侧不存在流式模拟）")

    url = ns.url or DEFAULT_URLS[ns.format_spec]
    envelope = build_envelope(
        ns.format_spec, ns.side, url, ns.extra, ns.body_file, ns.worker_id, ns.sse
    )
    line = json.dumps(envelope, ensure_ascii=False)

    side_label = (
        "请求侧（客户端请求）" if ns.side == "request" else "响应侧（上游响应）"
    ) + (" [SSE]" if ns.sse else "")
    print(f"[test] {side_label}  协议格式: {ns.format_spec}  被测: {ns.command} {' '.join(ns.args)}")
    print(f"[test] 信封输入（{len(line)} 字节）：")
    print(json.dumps(envelope, ensure_ascii=False, indent=2))
    print("-" * 60)

    try:
        proc = subprocess.Popen(
            [ns.command, *ns.args],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            encoding="utf-8",  # 文本管道：stdin/stdout 全程 str，编码显式 UTF-8
        )
    except OSError as e:
        print(f"[FAIL] format 进程启动失败: {e}")
        sys.exit(1)

    try:
        proc.stdin.write(line + "\n")
        proc.stdin.flush()
    except BrokenPipeError:
        print("[FAIL] format 进程在收到输入前就退出了（检查命令/参数）")
        sys.exit(1)

    reply = read_reply(proc.stdout, ns.timeout)
    if reply is None:
        print(f"[FAIL] {ns.timeout:.0f}s 内无回行——format 挂死或没写 stdout/忘了 flush")
        proc.kill()
        sys.exit(1)
    if not reply.strip():
        print("[FAIL] format 输出 EOF 且无回行（进程刚死，检查崩溃）")
        sys.exit(1)

    if ns.raw:
        print(reply.rstrip("\n"))
    else:
        print("[test] format 回信（格式化）：")
        try:
            parsed = json.loads(reply)
            print(json.dumps(parsed, ensure_ascii=False, indent=2))
            if parsed.get("error"):
                print(f'\n[WARN] error 行：{parsed["error"]}（请求侧将 502 / 响应侧透传）')
            else:
                print(f"\n[OK] 回信是合法信封。检查点（{side_label}）：")
                for c in checkpoints(ns.format_spec, ns.side, ns.sse):
                    print(f"  - {c}")
        except json.JSONDecodeError as e:
            print(f"[FAIL] 回行不是合法 JSON: {e}\n原文: {reply.rstrip()}")
            sys.exit(1)

    # persistent 语义联调：进程还活着说明它按循环语义等待下一行（脚本退出关闭
    # stdin，format 应按协议义务自行退出）
    if proc.poll() is None:
        print("[test] format 进程仍存活（persistent 语义正确；脚本退出将关闭其 stdin，"
              "合格 format 应随之退出）")
    proc.stdin.close()
    try:
        proc.wait(timeout=5)
        print(f"[test] format 已随 stdin EOF 退出（exit={proc.returncode}）——EOF 义务 ✓")
    except subprocess.TimeoutExpired:
        print("[WARN] stdin 已关闭 5s 仍未退出——persistent 协议义务（EOF 即退）不达标，"
              "实例停止时它会挂成孤儿进程")
        proc.kill()


if __name__ == "__main__":
    main()
