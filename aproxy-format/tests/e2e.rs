//! 用真实工作进程验证协议方向、私有状态、错误隔离和鉴权边界；不连接任何上游。
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
};
/// 持久进程，按流水号来回交换一行。
struct Worker {
    /// 测试创建的子进程，结束时先关闭 stdin 再等待退出。
    child: Child,
    /// 提前释放触发协议的 EOF 退役。
    input: Option<ChildStdin>,
    /// 单次只读一行，不把下一帧拿走。
    output: BufReader<ChildStdout>,
    /// 期望程序原样回显的序号。
    seq: u64,
    /// 首帧公布的能力上限，不由测试进程代为构造。
    hello: Value,
}
impl Worker {
    /// 启动编译出来的角色程序，给两个虚构的 key，不依赖父进程的密钥。
    fn new(args: &[&str]) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_aproxy-format"))
            .args(args)
            .env("FORMAT_A", "fixture-private-A")
            .env("FORMAT_B", "fixture-private-B")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let input = child.stdin.take();
        let output = BufReader::new(child.stdout.take().unwrap());
        let mut w = Self {
            child,
            input,
            output,
            seq: 0,
            hello: Value::Null,
        };
        let h=w.frame(json!({"stage":"hello","protocols":[1],"aproxy_version":"test","name":"role","process":"persistent","stages":[],"worker_id":0,"features":["decide.success"],"overrides":[]}));
        assert_eq!(h["protocol"], 1);
        assert_eq!(h["name"], args[0]);
        w.hello = h;
        w
    }
    /// 消息写入后立即要求回信，错误不能把后续请求串线。
    fn frame(&mut self, mut m: Value) -> Value {
        m["v"] = 1.into();
        m["seq"] = self.seq.into();
        writeln!(self.input.as_mut().unwrap(), "{m}").unwrap();
        let mut s = String::new();
        assert_ne!(self.output.read_line(&mut s).unwrap(), 0, "角色未应答");
        let r: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(r["seq"], self.seq);
        self.seq += 1;
        r
    }
}
impl Drop for Worker {
    /// 关闭管道是正常退出，不用按进程名结束任何后台程序。
    fn drop(&mut self) {
        drop(self.input.take());
        let _ = self.child.wait();
    }
}
/// 客户端路径与当前目标分开，能检出拿渠道的协议反推客户端的错误。
fn request(url: &str, path: &str, body: Value) -> Value {
    json!({"stage":"request","request_id":"r","delivery":"retry","client":{"path":path},"request":{"method":"POST","url":url,"headers":{"authorization":"from-client","cookie":"old"},"body":body.to_string()}})
}
/// 模拟另一进程拿到上一阶段给出的私有状态，与 URL 无关。
fn response(state: &Value, body: &str) -> Value {
    json!({"stage":"response","request_id":"r","state":state,"request":{"method":"POST","url":"https://custom.test/elsewhere"},"response":{"status":200,"headers":{"x-api-key":"fixture-private-A","authorization":"fixture-private-B","x-keep":"ok"},"body":body}})
}
/// Chat 的 system 消息提升为 Anthropic 顶层 system；标准路径换协议时保留查询。
#[test]
fn cross_format_request_conversion_end_to_end() {
    let mut w = Worker::new(&[
        "convert",
        "--from",
        "openai_chat",
        "--to",
        "anthropic_messages",
    ]);
    let r=w.frame(request("https://relay.test/v1/chat/completions?q=1","/v1/chat/completions",json!({"model":"gpt","max_tokens":30,"messages":[{"role":"system","content":"be nice"},{"role":"user","content":"hi"}]})));
    assert_eq!(r["report"]["summary"], "Selected the protocol conversion");
    let p = &r["request"];
    assert_eq!(p["url"], "https://relay.test/v1/messages?q=1");
    assert_eq!(p["headers"]["anthropic-version"], "2023-06-01");
    let b: Value = serde_json::from_str(p["body"].as_str().unwrap()).unwrap();
    assert_eq!(b["system"], "be nice");
    assert_eq!(b["messages"].as_array().unwrap().len(), 1);
    assert_eq!(b["max_tokens"], 30);
    assert_eq!(b["model"], "gpt");
}
/// auto 可跨协议；回程运行在不同工作进程，只依赖前程交出的私有状态。
#[test]
fn cross_format_response_conversion_end_to_end() {
    let args = ["convert", "--from", "auto", "--to", "anthropic_messages"];
    let mut first = Worker::new(&args);
    let r = first.frame(request(
        "https://relay.test/custom",
        "/v1/chat/completions",
        json!({"model":"gpt","messages":[{"role":"user","content":"hi"}],"max_tokens":30}),
    ));
    assert_eq!(r["state"], "openai_chat");
    assert!(r["request"].get("url").is_none());
    drop(first);
    let mut second = Worker::new(&args);
    let r=second.frame(response(&r["state"],r#"{"role":"assistant","content":[{"type":"text","text":"hello"}],"model":"gpt","stop_reason":"end_turn"}"#));
    assert_eq!(r["response"]["headers"], json!({"x-keep":"ok"}));
    let b: Value = serde_json::from_str(r["response"]["body"].as_str().unwrap()).unwrap();
    assert_eq!(b["choices"][0]["message"]["content"], "hello");
    assert!(!r.to_string().contains("fixture-private"));
}
/// 不可辨认的请求只拒绝这一帧；新的同协议请求与 SSE 均原样通过，不猜响应格式。
#[test]
fn auto_detect_rejects_unknown_input_and_requires_response_state() {
    let mut w = Worker::new(&["convert", "--from", "auto", "--to", "anthropic_messages"]);
    let bad = w.frame(request(
        "https://relay.test/",
        "/custom",
        json!({"other":1}),
    ));
    assert_eq!(
        bad["fail"]["reason"],
        "unknown client protocol; specify --from"
    );
    assert_eq!(bad["fail"]["retry"], false);
    assert!(bad.get("request").is_none());
    let good = w.frame(request(
        "https://relay.test/v1/messages",
        "/v1/messages",
        json!({"model":"m","messages":[],"max_tokens":10}),
    ));
    assert_eq!(good["state"], "anthropic_messages");
    assert!(good["request"].get("body").is_none());
    let same = w.frame(response(&good["state"], "event: ping\ndata: {}\n\n"));
    assert!(same.get("fail").is_none() && same["response"].get("body").is_none());
    let cross = w.frame(response(&json!("openai_chat"), "data: {}\n\n"));
    assert!(cross["fail"]["reason"].as_str().unwrap().contains("stream"));
    let missing = w.frame(response(&Value::Null, "{}"));
    assert_eq!(missing["fail"]["retry"], false);
}
/// 未知 origin 不能占掉轮换名额，更不能借地址前缀骗取其他源的 key。
#[test]
fn persistent_keys_keep_weighted_and_round_robin_orders_after_rejection() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("keys.toml");
    for (mode, expected) in [
        ("round_robin", ["A", "B", "A", "B", "A", "B"]),
        ("weighted", ["A", "A", "B", "A", "A", "B"]),
    ] {
        std::fs::write(&file,format!("strategy='{mode}'\n[[pool]]\norigin='https://relay.test'\nheader='x-api-key'\nkeys=[{{id='A',env='FORMAT_A',weight=2}},{{id='B',env='FORMAT_B'}}]")).unwrap();
        let mut w = Worker::new(&["keys", &format!("--config={}", file.display())]);
        for url in [
            "https://relay.test.evil.test/",
            "https://relay.test:8443/",
            "http://relay.test/",
        ] {
            let r = w.frame(request(url, "/", Value::Null));
            assert_eq!(r["fail"]["retry"], false);
            assert!(r.get("request").is_none());
        }
        for id in expected {
            let mut r = w.frame(request("https://relay.test:443/path", "/", Value::Null));
            let h = r.as_object_mut().unwrap().remove("request").unwrap();
            assert_eq!(
                h["headers"],
                json!({"x-api-key":format!("fixture-private-{id}")})
            );
            assert!(!r.to_string().contains("fixture-private"));
            assert_eq!(r["report"]["actions"][0]["key_id"], id);
        }
    }
}
/// 模型别名、渠道路径与查询串、未命中和反向别名都在实际角色上验证。
#[test]
fn route_alias_paths_and_reverse_alias_survive_worker_boundaries() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("route.toml");
    std::fs::write(&f,"[aliases]\nsmall='up-small'\nother='up-small'\n[[channel]]\nid='one'\nurl='https://relay.test/base'\npreserve_path=true\nmodels=[{prefix='up-'}]\ncredential_env='FORMAT_A'\nauth_header='x-api-key'\n[[channel]]\nid='two'\nurl='https://other.test/endpoint'\nmodels=[{exact='gpt'}]\n").unwrap();
    let args = ["route", "--config", f.to_str().unwrap()];
    let mut w = Worker::new(&args);
    let make = |model: &str| {
        request(
            "https://client.test/v1/messages?q=1",
            "/v1/messages?q=1",
            json!({"model":model}),
        )
    };
    let miss = w.frame(make("missing"));
    assert_eq!(miss["fail"]["retry"], false);
    let fallback = w.frame(make("gpt"));
    assert_eq!(fallback["request"]["url"], "https://other.test/endpoint");
    assert_eq!(fallback["request"]["headers"], json!({}));
    let r = w.frame(make("other"));
    assert_eq!(r["request"]["url"], "https://relay.test/base/v1/messages");
    assert_eq!(
        r["request"]["headers"],
        json!({"x-api-key":"fixture-private-A"})
    );
    let b: Value = serde_json::from_str(r["request"]["body"].as_str().unwrap()).unwrap();
    assert_eq!(b["model"], "up-small");
    drop(w);
    let mut w = Worker::new(&args);
    let r = w.frame(response(&r["state"], r#"{"model":"up-small"}"#));
    assert_eq!(r["response"]["body"], r#"{"model":"other"}"#);
}
/// 配置错误不会把 TOML 原行里的值或环境秘密复制到 stderr；旧 run 命令不再可用。
#[test]
fn invalid_role_configuration_does_not_echo_secrets() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("bad");
    for toml in [
        "unexpected='fixture-private-A'",
        "[[pool]]\norigin='https://x.test'\nheader='authorization'\nkeys=[]",
        "[[pool]]\norigin='https://x.test'\nheader='authorization'\nkeys=[{id='x',env='FORMAT_A',weight=0}]",
    ] {
        std::fs::write(&f, toml).unwrap();
        let out = Command::new(env!("CARGO_BIN_EXE_aproxy-format"))
            .args(["keys", "--config", f.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(!out.status.success() && out.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&out.stderr).contains("fixture-private"));
    }
    assert!(
        !Command::new(env!("CARGO_BIN_EXE_aproxy-format"))
            .arg("run")
            .output()
            .unwrap()
            .status
            .success()
    );
}

/// 每个子命令只公布它能处理的阶段；未参与正文协商的阶段不能擅自宣称 full。
#[test]
fn role_hello_declares_exact_stages_and_body_grades() {
    let dir = tempfile::tempdir().unwrap();
    let keys = dir.path().join("keys.toml");
    let route = dir.path().join("route.toml");
    std::fs::write(&keys,"[[pool]]\norigin='https://test.invalid'\nheader='x-api-key'\nkeys=[{id='one',env='FORMAT_A'}]").unwrap();
    std::fs::write(
        &route,
        "[[channel]]\nid='test'\nurl='https://test.invalid/'",
    )
    .unwrap();
    for (args, stages, body) in [
        (
            vec!["adaptive"],
            json!(["request", "decide", "notify"]),
            json!({"request":"digest","decide":"none","notify":"digest"}),
        ),
        (vec!["policy"], json!(["decide"]), json!({"decide":"none"})),
        (vec!["heartbeat"], json!(["heartbeat"]), json!({})),
        (
            vec!["rewrite"],
            json!(["request", "retry"]),
            json!({"request":"none","retry":"none"}),
        ),
        (
            vec!["keys", "--config", keys.to_str().unwrap()],
            json!(["request", "retry"]),
            json!({"request":"none","retry":"none"}),
        ),
        (
            vec!["route", "--config", route.to_str().unwrap()],
            json!(["request", "retry"]),
            json!({"request":"full","retry":"full"}),
        ),
        (
            vec!["convert", "--from", "auto", "--to", "openai_chat"],
            json!(["request", "retry", "response", "stream"]),
            json!({"request":"full","retry":"full"}),
        ),
    ] {
        let w = Worker::new(&args);
        assert_eq!(w.hello["stages"], stages, "{args:?}");
        assert_eq!(
            w.hello.get("request_body").cloned().unwrap_or(json!({})),
            body
        );
    }
}
/// translate 的输入没有 hello 或 seq，只转换一个 JSON 请求后在 EOF 退出。
#[test]
fn translate_is_a_one_off_request_converter() {
    let mut worker = Command::new(env!("CARGO_BIN_EXE_aproxy-format"))
        .args([
            "translate",
            "--from",
            "openai_responses",
            "--to",
            "openai_chat",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    worker
        .stdin
        .take()
        .unwrap()
        .write_all(br#"{"model":"m","input":"hello"}"#)
        .unwrap();
    let out = worker.wait_with_output().unwrap();
    assert!(out.status.success());
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(v["messages"].is_array() && v.get("seq").is_none());
}

/// 同一语义的三种原生样本：六条路径分别核对请求、整包响应、实时数据与截断收尾。
#[test]
fn convert_preserves_text_tools_usage_and_stop_in_all_six_directions() {
    let fixtures: Value = serde_json::from_str(include_str!("fixtures/protocols.json")).unwrap();
    let formats = ["anthropic_messages", "openai_chat", "openai_responses"];
    for client in formats {
        for upstream in formats {
            if upstream == client {
                continue;
            }
            let mut w = Worker::new(&["convert", "--from", client, "--to", upstream]);
            let req = w.frame(request(
                "https://relay.test/custom",
                "/custom",
                fixtures[client]["request"].clone(),
            ));
            let body: Value =
                serde_json::from_str(req["request"]["body"].as_str().unwrap()).unwrap();
            let tool = &body["tools"][0];
            assert_eq!(
                tool.get("name").unwrap_or(&tool["function"]["name"]),
                "lookup"
            );
            let messages = if upstream == "openai_responses" {
                &body["input"]
            } else {
                &body["messages"]
            };
            assert!(
                messages.to_string().contains("hello λ"),
                "{client}->{upstream}: {body}"
            );
            let reply = w.frame(response(
                &req["state"],
                &fixtures[upstream]["response"].to_string(),
            ));
            let buf: Value =
                serde_json::from_str(reply["response"]["body"].as_str().unwrap()).unwrap();
            let (text, name, args, stop, a, b) = match client {
                "anthropic_messages" => (
                    &buf["content"][0]["text"],
                    &buf["content"][1]["name"],
                    buf["content"][1]["input"].clone(),
                    &buf["stop_reason"],
                    &buf["usage"]["input_tokens"],
                    &buf["usage"]["output_tokens"],
                ),
                "openai_chat" => {
                    let c = &buf["choices"][0];
                    let tool = &c["message"]["tool_calls"][0]["function"];
                    (
                        &c["message"]["content"],
                        &tool["name"],
                        serde_json::from_value::<String>(tool["arguments"].clone())
                            .ok()
                            .and_then(|s| serde_json::from_str(&s).ok())
                            .unwrap_or(Value::Null),
                        &c["finish_reason"],
                        &buf["usage"]["prompt_tokens"],
                        &buf["usage"]["completion_tokens"],
                    )
                }
                _ => {
                    let out = buf["output"].as_array().unwrap();
                    let msg = out.iter().find(|x| x["type"] == "message").unwrap();
                    let tool = out.iter().find(|x| x["type"] == "function_call").unwrap();
                    (
                        &msg["content"][0]["text"],
                        &tool["name"],
                        serde_json::from_str(tool["arguments"].as_str().unwrap()).unwrap(),
                        &buf["status"],
                        &buf["usage"]["input_tokens"],
                        &buf["usage"]["output_tokens"],
                    )
                }
            };
            assert_eq!(
                (text, name, args, stop, a, b),
                (
                    &json!("hello λ"),
                    &json!("lookup"),
                    json!({"q":"x"}),
                    &json!(match client {
                        "anthropic_messages" => "tool_use",
                        "openai_chat" => "tool_calls",
                        _ => "completed",
                    }),
                    &json!(7),
                    &json!(3)
                ),
                "{upstream}->{client}: {buf}"
            );
            let open = json!({"stage":"stream","event":"open","request_id":"r","stream_id":"r/1","state":req["state"],"request":{"method":"POST","url":"https://relay.test/custom"},"response":{"status":200,"headers":{"content-type":"text/event-stream; charset=utf-8"}}});
            let tail = w.frame(open.clone())["abort_tail"]
                .as_str()
                .unwrap()
                .to_owned();
            let mut output = String::new();
            for event in fixtures[upstream]["events"].as_array().unwrap() {
                let r=w.frame(json!({"stage":"stream","event":"data","stream_id":"r/1","data":format!("data: {event}\r\n\r\n")}));
                output.push_str(r["data"].as_str().unwrap());
            }
            assert!(
                output.contains("hello λ"),
                "must emit before close: {upstream}->{client}"
            );
            let close = w.frame(json!({"stage":"stream","event":"close","stream_id":"r/1"}));
            output.push_str(close["data"].as_str().unwrap_or(""));
            assert!(
                output.contains("lookup") && output.contains("\\\"q\\\""),
                "{upstream}->{client}: {output}"
            );
            let events: Vec<Value> = output
                .lines()
                .filter_map(|l| l.strip_prefix("data: "))
                .filter(|s| *s != "[DONE]")
                .map(|s| serde_json::from_str(s).unwrap())
                .collect();
            let terminal = match client {
                "anthropic_messages" => events.iter().find(|e| {
                    e["type"] == "message_delta"
                        && e["delta"]["stop_reason"] == "tool_use"
                        && e["usage"]["input_tokens"] == 7
                        && e["usage"]["output_tokens"] == 3
                }),
                "openai_chat" => {
                    assert!(
                        output.ends_with("data: [DONE]\n\n")
                            && events
                                .iter()
                                .any(|e| e["choices"][0]["finish_reason"] == "tool_calls")
                    );
                    events.iter().find(|e| {
                        e["usage"]["prompt_tokens"] == 7 && e["usage"]["completion_tokens"] == 3
                    })
                }
                _ => events.iter().find(|e| {
                    e["type"] == "response.completed"
                        && e["response"]["usage"]["input_tokens"] == 7
                        && e["response"]["usage"]["output_tokens"] == 3
                }),
            };
            assert!(terminal.is_some(), "{upstream}->{client}: {output}");
            w.frame(open);
            let partial=w.frame(json!({"stage":"stream","event":"data","stream_id":"r/1","data":"data: {\"partial\":"}));
            assert_eq!(partial["data"], "");
            let abort=w.frame(json!({"stage":"stream","event":"abort","stream_id":"r/1","reason":{"code":"upstream_body_interrupted"}}));
            assert_eq!(abort["data"], tail);
            assert!(
                tail.contains("stream conversion aborted")
                    && tail.ends_with("\n\n")
                    && !tail.contains("[DONE]")
            );
        }
    }
}

/// 自定义入口的已知协议产生原生事件，未选路径与不明协议不能被静默变成 Messages 流。
#[test]
fn heartbeat_matches_only_explicit_client_paths() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("beats.toml");
    let p = "[[preset]]\npath='/custom/messages'\nprotocol='anthropic_messages'\nevent='ping'\n";
    std::fs::write(&path, p).unwrap();
    let args = ["heartbeat", "--config", path.to_str().unwrap()];
    let mut w = Worker::new(&args);
    let beat = |path: &str| json!({"stage":"heartbeat","request_id":"r","heartbeat":{"beat":1,"elapsed_ms":50,"attempt":1},"client":{"path":path},"data":": keepalive\n\n"});
    assert_eq!(
        w.frame(beat("/custom/messages?q=x"))["data"],
        "event: ping\ndata: {\"type\":\"ping\"}\n\n"
    );
    for path in ["/custom/messages/extra", "/v1/responses", "/v1/messages"] {
        let r = w.frame(beat(path));
        assert!(r.get("data").is_none() && r.get("state").is_none());
    }
    for invalid in [
        p.repeat(2),
        p.replace("'ping'", "'message_stop'"),
        p.replace("'anthropic_messages'", "'openai_responses'"),
    ] {
        std::fs::write(&path, invalid).unwrap();
        let r = Command::new(env!("CARGO_BIN_EXE_aproxy-format"))
            .args(args)
            .output()
            .unwrap();
        assert!(!r.status.success() && r.stdout.is_empty());
    }
}

/// 密钥名与 Cookie 采用和代理历史相同的识别边界；请求跨 origin 及转换回程都不透传。
#[test]
fn credential_headers_do_not_cross_role_boundaries() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("role.toml");
    let headers = json!({"X-Access-Token":"hidden", "x-secret-thing":"hidden", "auth-info":"hidden", "x-session":"hidden", "x-signature":"hidden", "Set-Cookie":"hidden", "x-keep":"ok"});
    for (role, toml) in [
        ("route", "[[channel]]\nid='b'\nurl='https://b.test/v1'"),
        (
            "keys",
            "[[pool]]\norigin='https://a.test'\nheader='authorization'\nkeys=[{id='a',env='FORMAT_A'}]",
        ),
    ] {
        std::fs::write(&config, toml).unwrap();
        let mut worker = Worker::new(&[role, "--config", config.to_str().unwrap()]);
        let mut m = request("https://a.test/v1", "/v1", json!({"model":"gpt"}));
        m["request"]["headers"] = headers.clone();
        let r = worker.frame(m);
        assert_eq!(r["request"]["headers"]["x-keep"], "ok");
        assert!(!r.to_string().contains("hidden"), "{role}: {r}");
    }
    let mut worker = Worker::new(&["convert", "--from", "openai_chat", "--to", "openai_chat"]);
    let mut m = response(&Value::Null, "unchanged");
    m["response"]["headers"] = headers;
    let r = worker.frame(m);
    assert_eq!(r["response"]["headers"], json!({"x-keep":"ok"}));
}

/// 环境凭据只能放进历史与日志都会脱敏的槽位，错误不回显配置里的值。
#[test]
fn credential_slots_require_history_protected_names() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("config");
    for (role, toml) in [
        (
            "keys",
            "[[pool]]\norigin='https://a.test'\nheader='x-access-code'\nkeys=[{id='a',env='FORMAT_A'}]",
        ),
        (
            "route",
            "[[channel]]\nid='b'\nurl='https://b.test'\nauth_header='X-Access-Code'\ncredential_env='FORMAT_A'",
        ),
    ] {
        std::fs::write(&file, toml).unwrap();
        let out = Command::new(env!("CARGO_BIN_EXE_aproxy-format"))
            .args([role, "--config", file.to_str().unwrap()])
            .env("FORMAT_A", "fixture-private")
            .output()
            .unwrap();
        let error = String::from_utf8(out.stderr).unwrap();
        assert!(
            !out.status.success() && out.stdout.is_empty() && error.contains("脱敏"),
            "{role}: {error}"
        );
        assert!(!error.contains("fixture-private"));
    }
}

/// 真实 origin（含端口）决定查询串边界；跨域只传点名的参数，重复值不能丢。
#[test]
fn route_drops_cross_origin_query_except_listed_parameters() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("route.toml");
    for (target, extra, expected) in [
        (
            "https://a.test/base",
            "",
            "https://a.test/base/chat?token=private&view=a&view=b",
        ),
        (
            "https://a.test:9443/base",
            "",
            "https://a.test:9443/base/chat",
        ),
        (
            "https://b.test/base",
            "carry_query=['view']",
            "https://b.test/base/chat?view=a&view=b",
        ),
    ] {
        std::fs::write(
            &file,
            format!("[[channel]]\nid='b'\nurl='{target}'\npreserve_path=true\n{extra}"),
        )
        .unwrap();
        let mut w = Worker::new(&["route", "--config", file.to_str().unwrap()]);
        let r = w.frame(request(
            "https://a.test/chat?token=private&view=a&view=b",
            "/chat?token=private&view=a&view=b",
            json!({"model":"m"}),
        ));
        assert_eq!(r["request"]["url"], expected);
    }
}

/// 上游的 200 损坏时不能用裸 fail 让代理回传其凭据与私有错误页。
#[test]
fn failed_response_conversion_replaces_the_entire_upstream_entity() {
    for (from, to) in [
        ("openai_chat", "anthropic_messages"),
        ("openai_responses", "openai_chat"),
        ("anthropic_messages", "openai_chat"),
        ("auto", "openai_chat"),
    ] {
        let mut w = Worker::new(&["convert", "--from", from, "--to", to]);
        let r = w.frame(response(&Value::Null, "fixture-private error page"));
        let fail = &r["fail"];
        let replacement = &fail["response"];
        assert_eq!(replacement["status"], 502, "{r}");
        assert_eq!(replacement["headers"]["content-type"], "application/json");
        assert!(!r.to_string().contains("fixture-private"));
        let body: Value = serde_json::from_str(replacement["body"].as_str().unwrap()).unwrap();
        assert!(body["error"]["message"].is_string());
        if from == "anthropic_messages" || from == "auto" {
            assert_eq!(body["type"], "error");
            assert_eq!(body["error"]["type"], "api_error");
        }
    }
}
