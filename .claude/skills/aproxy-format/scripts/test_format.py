#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""format 联调测试器：不经 aproxy，直接模拟三种协议格式的请求信封喂给
被测 format，格式化打印回信封——写完 format 第一件事就是跑它。

用法示例：
  python test_format.py --format-spec anthropic --command python -- args fmt.py
  python test_format.py --format-spec openai-chat --command ./my-format --url https://relay.example.com/v1/chat/completions
  python test_format.py --format-spec openai-responses --command aproxy-format --args run --extra agg.toml --raw

注意（Windows）：本脚本强制 UTF-8 stdin/stdout；被测 format 也必须按
guide.md 处理自身编码（脚本语言的 stdout 默认代码页是高频死法）。
"""

import argparse
import base64
import json
import subprocess
import sys
import threading
import time

if hasattr(sys.stdout, "reconfigure"):
    sys.stdin.reconfigure(encoding="utf-8")
    sys.stdout.reconfigure(encoding="utf-8")
    sys.stderr.reconfigure(encoding="utf-8")

# 三种 wire format 的最小合法模拟请求（形态对齐各协议官方最小请求）
MOCK_BODIES = {
    "anthropic": {
        "system": "You are a helpful assistant.",
        "messages": [{"role": "user", "content": "你好，format 测试"}],
        "max_tokens": 64,
        "model": "claude-3-5-sonnet",
    },
    "openai-chat": {
        "model": "gpt-4o",
        "messages": [
            {"role": "system", "content": "You are a helpful assistant."},
            {"role": "user", "content": "你好，format 测试"},
        ],
        "max_tokens": 64,
    },
    "openai-responses": {
        "model": "gpt-5o",
        "instructions": "You are a helpful assistant.",
        "input": "你好，format 测试",
        "max_output_tokens": 64,
    },
}

DEFAULT_URLS = {
    "anthropic": "https://api.anthropic.com/v1/messages",
    "openai-chat": "https://api.openai.com/v1/chat/completions",
    "openai-responses": "https://api.openai.com/v1/responses",
}


def build_envelope(spec: str, url: str, extra: str, body_file: str | None,
                   worker_id: int) -> dict:
    """按 skill protocol.md 的 JSON 完整规范组装请求侧信封。"""
    if body_file:
        with open(body_file, "rb") as f:
            raw = f.read()
        try:
            body = raw.decode("utf-8")
            envelope = {"body": body}
        except UnicodeDecodeError:
            # 非 UTF-8：标准字母表 + padding 的 base64（URL-safe 变体不行）
            envelope = {"body_b64": base64.b64encode(raw).decode("ascii")}
    else:
        envelope = {"body": json.dumps(MOCK_BODIES[spec], ensure_ascii=False)}
    envelope.update(
        {
            "method": "POST",
            "url": url,
            "headers": {
                "content-type": "application/json",
                "authorization": "Bearer test-client-key",
            },
            "worker_id": worker_id,
            "extra": extra,
        }
    )
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


def main() -> None:
    ap = argparse.ArgumentParser(
        description="format 联调测试器：模拟三种协议格式的请求信封喂给被测 format"
    )
    ap.add_argument(
        "--format-spec",
        choices=sorted(MOCK_BODIES),
        required=True,
        help="模拟的请求协议格式",
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

    url = ns.url or DEFAULT_URLS[ns.format_spec]
    envelope = build_envelope(ns.format_spec, url, ns.extra, ns.body_file, ns.worker_id)
    line = json.dumps(envelope, ensure_ascii=False)

    print(f"[test] 协议格式: {ns.format_spec}  被测: {ns.command} {' '.join(ns.args)}")
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
                print("\n[OK] 回信是合法信封。检查点：")
                print("  - headers 是否必含（缺键 = 解析失败 502）")
                print("  - url 是否按预期改写（缺省 = 沿用输入 url）")
                print("  - body 转换产物形态是否符合目标协议")
                print("  - 鉴权头：客户端原头是否剥离、渠道头是否正确注入")
        except json.JSONDecodeError as e:
            print(f"[FAIL] 回行不是合法 JSON: {e}\n原文: {reply.rstrip()}")
            sys.exit(1)

    # persistent 语义联调：进程还活着就再喂一行（EOF 退出测试交给 Ctrl-C 或
    # kill——脚本退出关闭 stdin，format 应按协议义务自行退出）
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
