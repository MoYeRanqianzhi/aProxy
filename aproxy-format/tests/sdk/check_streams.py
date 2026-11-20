"""用内存 HTTP 运输把两个真实工作进程的输出交给官方 SDK；不连接 API。"""
import argparse
import json
import subprocess
from pathlib import Path

import anthropic
import httpx2
import openai


PATHS = {
    "anthropic_messages": "/v1/messages",
    "openai_chat": "/v1/chat/completions",
    "openai_responses": "/v1/responses",
}


def exchange(binary, role, args, frames):
    """驱动真实 hello 与各阶段的序号，并验证不是悄悄跳过转换的 fail。"""
    hello = dict(stage="hello", protocols=[1], aproxy_version="fixture", name=role,
                 process="persistent", stages=[], features=[], overrides=[], worker_id=0)
    lines = [json.dumps(dict(f, v=1, seq=i)) for i, f in enumerate([hello, *frames])]
    result = subprocess.run([binary, role, *args], input="\n".join(lines) + "\n",
                            capture_output=True, text=True, encoding="utf-8", check=True, timeout=30)
    replies = [json.loads(line) for line in result.stdout.splitlines()]
    assert len(replies) == len(lines) and replies[0]["protocol"] == 1
    assert all(r["seq"] == i and "fail" not in r for i, r in enumerate(replies)), replies
    return replies[1:]


def translated(binary, source, target, sample):
    """模拟一个真实的成功 stream；按不规则块切分，不能假定一条管道消息恰是一条事件。"""
    stream = dict(stage="stream", stream_id="r/1")
    args = ["--from", target, "--to", source]
    body = {"model": "fixture", "input": "hello"} if target == "openai_responses" else {
        "model": "fixture", "messages": [{"role": "user", "content": "hello"}], "max_tokens": 8}
    req = dict(stage="request", request_id="r", delivery="retry", client={"path": PATHS[target]},
               request={"method": "POST", "url": "https://fixture.invalid" + PATHS[source],
                        "headers": {}, "body": json.dumps(body)})
    state = exchange(binary, "convert", args, [req])[0]["state"]
    frames = [dict(stream, event="open", request_id="r", state=state,
                   request={"method": "POST", "url": "https://fixture.invalid"},
                   response={"status": 200, "headers": {"content-type": "text/event-stream"}})]
    native = "".join((f"event: {e['type']}\r\n" if "type" in e else "") +
                     "data: " + json.dumps(e, ensure_ascii=False) + "\r\n\r\n" for e in sample["events"])
    if source == "openai_chat":
        native += "data: [DONE]\n\n"
    frames.extend(dict(stream, event="data", data=native[i:i+47]) for i in range(0, len(native), 47))
    frames.append(dict(stream, event="close"))
    replies = exchange(binary, "convert", args, frames)
    assert replies[0]["abort_tail"].endswith("\n\n")
    return "".join(r.get("data", "") for r in replies[1:])


def heartbeat(binary, target):
    """也从真实角色取心跳；未配置协议的输入注释保持原样。"""
    default = ": keepalive\n\n"
    frame = dict(stage="heartbeat", request_id="r", client={"path": PATHS[target], "stream": True},
                 heartbeat={"beat": 1, "elapsed_ms": 100, "attempt": 1}, data=default)
    (reply,) = exchange(binary, "heartbeat", [], [frame])
    data = reply.get("data", default)
    assert data.endswith("\n\n")
    assert (data.startswith("event: ping\n") if target == "anthropic_messages" else data == default)
    return data


def parse(target, data):
    """使用 SDK 的高层流解析与聚合；不仅验证 JSON 可读，还检查最终工具参数和用量。"""
    def handle(request):
        assert request.url.host == "fixture.invalid"
        return httpx2.Response(200, headers={"Content-Type": "text/event-stream"}, content=data.encode())

    with httpx2.Client(transport=httpx2.MockTransport(handle), trust_env=False) as http:
        opts = dict(api_key="fixture", base_url="https://fixture.invalid/v1", http_client=http, max_retries=0)
        msgs = [{"role": "user", "content": "hello"}]
        if target == "anthropic_messages":
            with anthropic.Anthropic(**opts).messages.stream(model="fixture", messages=msgs, max_tokens=128) as stream:
                text = "".join(stream.text_stream)
                final = stream.get_final_message()
            tool = next(b for b in final.content if b.type == "tool_use")
            assert final.stop_reason == "tool_use"
            assert text == next(b.text for b in final.content if b.type == "text")
            return text, tool.name, tool.input, final.usage.input_tokens, final.usage.output_tokens
        client = openai.OpenAI(**opts)
        if target == "openai_chat":
            with client.chat.completions.stream(model="fixture", messages=msgs) as stream:
                text = "".join(e.delta for e in stream if e.type == "content.delta")
                final = stream.get_final_completion()
            choice = final.choices[0]
            assert choice.finish_reason == "tool_calls" and choice.message.content == text
            tool = choice.message.tool_calls[0].function
            return text, tool.name, json.loads(tool.arguments), final.usage.prompt_tokens, final.usage.completion_tokens
        with client.responses.stream(model="fixture", input="hello") as stream:
            text = "".join(e.delta for e in stream if e.type == "response.output_text.delta")
            final = stream.get_final_response()
        assert final.status == "completed" and final.output_text == text
        tool = next(x for x in final.output if x.type == "function_call")
        return text, tool.name, json.loads(tool.arguments), final.usage.input_tokens, final.usage.output_tokens


def check_interrupted_responses(binary):
    """真实 SDK 在首帧之前及增量之后都须报告同一中断；只有活进程能给出正确的序号。"""
    base = dict(stage="stream", stream_id="r/1")
    opening = dict(base, event="open", request_id="r", request={"method": "POST", "url": "https://fixture.invalid"},
                   response={"status": 200, "headers": {"content-type": "text/event-stream"}})
    delta = {"id": "c", "model": "m", "choices": [{"index": 0, "delta": {"role": "assistant", "content": "visible"}}]}
    for started in [False, True]:
        frames = [opening]
        if started:
            frames.append(dict(base, event="data", data="data: " + json.dumps(delta) + "\n\n"))
        frames.append(dict(base, event="abort", reason={"code": "upstream_body_interrupted"}))
        replies = exchange(binary, "convert", ["--from", "openai_responses", "--to", "openai_chat"], frames)
        prefix = replies[1]["data"] if started else ""
        previous = [json.loads(l[6:])["sequence_number"] for l in prefix.splitlines() if l.startswith("data: ")]
        assert not started or "visible" in prefix
        live = replies[-1]["data"]
        event = json.loads(next(l[6:] for l in live.splitlines() if l.startswith("data: ")))
        assert event["sequence_number"] == (previous[-1] + 1 if previous else 0)
        for ending in [live, replies[0]["abort_tail"]]:
            try:
                parse("openai_responses", prefix + ending)
            except openai.APIError as exc:
                assert "stream conversion aborted" in str(exc), str(exc)
            else:
                raise AssertionError("interrupted Responses stream returned normally")
    print("Responses before and after output: live and saved endings all raised APIError")


def main():
    """每个目标的基线与穿插心跳版本必须保留相同的、由源样本预先约定的语义。"""
    cli = argparse.ArgumentParser(description=__doc__)
    cli.add_argument("binary", type=Path, help="已编译的 aproxy-format 可执行文件")
    binary = cli.parse_args().binary.resolve(strict=True)
    fixtures = json.loads((Path(__file__).resolve().parents[1] / "fixtures/protocols.json").read_text(encoding="utf-8"))
    check_interrupted_responses(binary)
    for target in PATHS:
        beat = heartbeat(binary, target)
        for source, sample in fixtures.items():
            if source == target:
                continue
            body = translated(binary, source, target, sample)
            middle = body.index("\n\n") + 2
            for data in [body, beat + body[:middle] + beat + body[middle:]]:
                assert parse(target, data) == ("hello λ", "lookup", {"q": "x"}, 7, 3), (source, target)
            mode = "native ping" if target == "anthropic_messages" else "unchanged SSE comment"
            print(f"{source} -> {target}: baseline and {mode} passed")


if __name__ == "__main__":
    main()
