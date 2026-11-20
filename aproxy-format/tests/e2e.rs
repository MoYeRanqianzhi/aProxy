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
    assert_eq!(
        r["state"],
        json!(["openai_chat", "anthropic_messages"]).to_string()
    );
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
    assert_eq!(
        good["state"],
        json!(["anthropic_messages", "anthropic_messages"]).to_string()
    );
    assert!(good["request"].get("body").is_none());
    let same = w.frame(response(&good["state"], "event: ping\ndata: {}\n\n"));
    assert!(same.get("fail").is_none() && same["response"].get("body").is_none());
    let cross = w.frame(response(
        &json!(json!(["openai_chat", "anthropic_messages"]).to_string()),
        "data: {}\n\n",
    ));
    assert!(cross["fail"]["reason"].as_str().unwrap().contains("stream"));
    let missing = w.frame(response(&Value::Null, "{}"));
    assert_eq!(missing["fail"]["retry"], false);
}
/// 目的端先按 origin 与最长路径段匹配，再看标准端点；回程在另一进程不重猜当前 URL。
#[test]
fn automatic_upstream_uses_route_origin_and_longest_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("convert.toml");
    std::fs::write(&f,"from='openai_chat'\nto='auto'\n[[upstream]]\norigin='https://mixed.test'\nprotocol='openai_chat'\n[[upstream]]\norigin='https://mixed.test:443'\npath_prefix='/tenant'\nprotocol='anthropic_messages'").unwrap();
    let args = ["convert", "--config", f.to_str().unwrap()];
    let mut w = Worker::new(&args);
    for (url, proto) in [
        ("https://mixed.test/tenant/custom", "anthropic_messages"),
        (
            "https://mixed.test/tenant/v1/responses",
            "anthropic_messages",
        ),
        ("https://mixed.test/tenantX/custom", "openai_chat"),
        ("https://other.test/v1/responses", "openai_responses"),
    ] {
        let r = w.frame(request(
            url,
            "/v1/chat/completions",
            json!({"model":"m","max_tokens":5,"messages":[{"role":"user","content":"hi"}]}),
        ));
        assert_eq!(
            serde_json::from_str::<Value>(r["state"].as_str().unwrap()).unwrap(),
            json!(["openai_chat", proto])
        );
        if proto == "anthropic_messages" {
            let r=Worker::new(&args).frame(response(&r["state"],r#"{"role":"assistant","content":[{"type":"text","text":"answer"}],"model":"served","stop_reason":"end_turn"}"#));
            let body: Value =
                serde_json::from_str(r["response"]["body"].as_str().unwrap()).unwrap();
            assert_eq!(body["choices"][0]["message"]["content"], "answer");
        }
    }
    let r = w.frame(request(
        "https://mixed.test:444/custom",
        "/v1/chat/completions",
        json!({"model":"m","messages":[]}),
    ));
    assert_eq!(
        r["fail"]["reason"],
        "unknown upstream protocol; configure a destination mapping"
    );
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
/// 粘性才声明 full；用相同首轮的不同请求确认选择不轮转，全部冷却时不能发出旧凭据。
#[test]
fn sticky_key_negotiation_and_exhausted_pool_retry() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("keys.toml");
    std::fs::write(&file,"sticky=true\n[[pool]]\norigin='https://relay.test'\nheader='x-api-key'\nkeys=[{id='one',env='FORMAT_A'},{id='two',env='FORMAT_B'}]").unwrap();
    let mut w = Worker::new(&["keys", "--config", file.to_str().unwrap()]);
    assert_eq!(
        w.hello["request_body"],
        json!({"request":"full","retry":"full"})
    );
    let mut m = request("https://relay.test/", "/", json!({"input":"hello"}));
    let first = w.frame(m.clone());
    m["request"]["body"] =
        json!({"messages":[{"role":"user","content":"hello"},{"role":"user","content":"more"}]})
            .to_string()
            .into();
    for _ in 0..3 {
        assert_eq!(w.frame(m.clone())["state"], first["state"]);
    }
    m["stage"] = "retry".into();
    m["state"] = first["state"].clone();
    m["retry"] = json!({"attempt":2,"elapsed_ms":0,"last_failure":{"code":"upstream_status"},"last_response":{"status":429,"headers":{}}});
    let second = w.frame(m.clone());
    assert_ne!(first["state"], second["state"]);
    assert!(second.get("request").is_some());
    m["state"] = second["state"].clone();
    let exhausted = w.frame(m);
    assert_eq!(exhausted["fail"]["retry"], true);
    assert!(exhausted.get("request").is_none());
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
    let state = r["state"].clone();
    let mut input = response(&state, r#"{ "model" : "up-small" }"#);
    for media in ["text/plain", "text/event-stream", "application/jsonish", ""] {
        input["response"]["headers"]["content-type"] = media.into();
        assert!(w.frame(input.clone()).get("response").is_none(), "{media}");
    }
    for media in [
        "Application/JSON; charset=utf-8",
        "application/vnd.example+json",
    ] {
        input["response"]["headers"]["content-type"] = media.into();
        let r = w.frame(input.clone());
        assert_eq!(r["response"]["body"], r#"{"model":"other"}"#);
    }
}
/// 不同进程与增长的会话落到同一渠道，重试票仍切走，不受粘性阻止。
#[test]
fn sticky_route_survives_worker_changes_and_yields_to_failover() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("route.toml");
    std::fs::write(&file,"sticky=true\n[[channel]]\nid='a'\nurl='https://a.test/'\n[[channel]]\nid='b'\nurl='https://b.test/'").unwrap();
    let args = ["route", "--config", file.to_str().unwrap()];
    let mut m = request("https://up.test/", "/", json!({"model":"m","input":"same"}));
    let r = Worker::new(&args).frame(m.clone());
    m["request"]["body"]=json!({"model":"m","messages":[{"role":"user","content":"same"},{"role":"user","content":"later"}]}).to_string().into();
    let mut other = Worker::new(&args);
    assert_eq!(other.frame(m.clone())["state"], r["state"]);
    m["stage"] = "retry".into();
    m["state"] = r["state"].clone();
    m["retry"] =
        json!({"attempt":2,"elapsed_ms":0,"last_failure":{"code":"upstream_connect_failed"}});
    assert_ne!(other.frame(m)["request"]["url"], r["request"]["url"]);
}
/// 无过滤的末级渠道应接住前级未匹配的模型，而不只是另一个精确匹配。
#[test]
fn unfiltered_channel_accepts_unmatched_models() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("route.toml");
    std::fs::write(&file, "[[channel]]\nid='specific'\nurl='https://one.test/'\nmodels=[{exact='known'}]\n[[channel]]\nid='fallback'\nurl='https://two.test/'").unwrap();
    let mut w = Worker::new(&["route", "--config", file.to_str().unwrap()]);
    let r = w.frame(request(
        "https://client.test/",
        "/",
        json!({"model":"unlisted"}),
    ));
    assert_eq!(r["request"]["url"], "https://two.test/", "{r}");
}
/// 单独订阅响应时显式协议足以翻译；未声明别名的模型仍由实际服务端决定。
#[test]
fn explicit_response_only_preserves_served_model_without_state() {
    let mut w = Worker::new(&[
        "convert",
        "--from",
        "openai_chat",
        "--to",
        "anthropic_messages",
    ]);
    let mut m = response(
        &Value::Null,
        r#"{"role":"assistant","content":[{"type":"text","text":"hello"}],"model":"served-model","stop_reason":"end_turn"}"#,
    );
    m.as_object_mut().unwrap().remove("state");
    let r = w.frame(m);
    let b: Value = serde_json::from_str(r["response"]["body"].as_str().unwrap()).unwrap();
    assert_eq!(b["model"], "served-model");
    assert_eq!(b["choices"][0]["message"]["content"], "hello");
}
/// 头部改写不读正文；正文规则在 retry 也重跑，报告不包含新旧字段值。
#[test]
fn rewrite_negotiates_only_needed_data_and_reports_positions() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("rewrite.toml");
    let headers = "remove_headers=['Cookie']\nset_headers={ 'X-Service'='private-header-value' }\n";
    std::fs::write(&f, headers).unwrap();
    let args = ["rewrite", "--config", f.to_str().unwrap()];
    let mut w = Worker::new(&args);
    assert_eq!(w.hello["request_body"]["retry"], "none");
    let mut m = request("https://relay.test/", "/", Value::Null);
    m["request"].as_object_mut().unwrap().remove("body");
    let r = w.frame(m.clone());
    assert_eq!(
        r["request"]["headers"],
        json!({"authorization":"from-client","x-service":"private-header-value"})
    );
    assert!(r["request"].get("body").is_none());
    drop(w);
    std::fs::write(
        &f,
        format!("{headers}remove=['/debug']\nset={{ '/nested/n'=4 }}\nmodels={{ small='served' }}"),
    )
    .unwrap();
    let mut w = Worker::new(&args);
    assert_eq!(
        w.hello["request_body"],
        json!({"request":"full","retry":"full"})
    );
    m["request"]["body"] =
        json!({"model":"small","debug":null,"nested":{},"retain":"private-content"})
            .to_string()
            .into();
    for stage in ["request", "retry"] {
        m["stage"] = stage.into();
        if stage == "retry" {
            m["retry"] =
                json!({"attempt":2,"elapsed_ms":0,"last_failure":{"code":"upstream_status"}});
        }
        let r = w.frame(m.clone());
        assert_eq!(
            serde_json::from_str::<Value>(r["request"]["body"].as_str().unwrap()).unwrap(),
            json!({"model":"served","nested":{"n":4},"retain":"private-content"})
        );
        assert_eq!(r["report"]["actions"].as_array().unwrap().len(), 5);
        let report = r["report"].to_string();
        for absent in ["private", "served", "from-client"] {
            assert!(!report.contains(absent), "{report}");
        }
    }
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
        .write_all("\u{a0}{\"model\":\"m\",\"input\":\"hello\"}\u{2003}".as_bytes())
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
            if upstream == "openai_chat" {
                output.push_str(
                    stream_frame(&mut w, "data", "data: [DONE]\n\n")["data"]
                        .as_str()
                        .unwrap(),
                );
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

/// 同一套分派入口不能让共享处理函数的阶段悄悄越过 hello 的能力上限。
#[test]
fn dispatch_rejects_stages_not_in_the_configured_hello() {
    let mut a = Worker::new(&["adaptive"]);
    let mut m = request("https://a.test/", "/", Value::Null);
    m["stage"] = "retry".into();
    m["retry"] = json!({"attempt":2,"elapsed_ms":10,"last_failure":{"code":"upstream_failed"}});
    let r = a.frame(m);
    assert_eq!(r["fail"]["reason"], "unsupported stage", "{r}");
    assert!(r.get("delivery").is_none());
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("route.toml");
    std::fs::write(&file, "[[channel]]\nid='b'\nurl='https://b.test'").unwrap();
    let mut route = Worker::new(&["route", "--config", file.to_str().unwrap()]);
    let r = route.frame(response(&Value::Null, "{}"));
    assert_eq!(r["fail"]["reason"], "unsupported stage");
    assert!(r.get("response").is_none());
}

/// 非标准端点没有协议先验，Messages 与 Chat 都接受的形态不能得到凑巧正确的回程。
#[test]
fn auto_detect_refuses_overlapping_message_shapes_on_custom_paths() {
    let mut worker = Worker::new(&["convert", "--from", "auto", "--to", "openai_chat"]);
    let body = json!({"model":"m","messages":[{"role":"user","content":"hi"}],"max_tokens":12});
    for ambiguous in [
        body.clone(),
        json!({"model":"m","messages":[],"input":"hi"}),
    ] {
        let r = worker.frame(request("https://a.test/custom", "/custom", ambiguous));
        assert_eq!(
            r["fail"]["reason"], "unknown client protocol; specify --from",
            "{r}"
        );
    }
    for (path, state) in [
        ("/v1/messages", "anthropic_messages"),
        ("/v1/chat/completions", "openai_chat"),
    ] {
        let r = worker.frame(request("https://a.test/custom", path, body.clone()));
        assert_eq!(r["state"], json!([state, "openai_chat"]).to_string());
    }
}

/// 上一上游的租户路径不属于客户端，两个基址各带前缀时只保留目标前缀一次。
#[test]
fn preserved_path_comes_from_the_client_not_the_previous_base() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("route.toml");
    std::fs::write(
        &file,
        "[[channel]]\nid='b'\nurl='https://b.test/tenant-b'\npreserve_path=true",
    )
    .unwrap();
    let mut w = Worker::new(&["route", "--config", file.to_str().unwrap()]);
    let r = w.frame(request(
        "https://a.test/tenant-a/v1/messages?q=secret",
        "/v1/messages?q=secret",
        json!({"model":"m"}),
    ));
    assert_eq!(r["request"]["url"], "https://b.test/tenant-b/v1/messages");
}

/// 长标识、转义字符及别名要按最终 state 的 JSON 字节计数，而不是仅按一个配置值计数。
#[test]
fn oversized_role_state_is_refused_during_configuration() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("roles.toml");
    let quoted = "\"".repeat(32768);
    for (role, txt) in [
        (
            "keys",
            format!(
                "[[pool]]\norigin='https://x.test'\nheader='authorization'\nkeys=[{{id='{quoted}',env='FORMAT_A'}}]"
            ),
        ),
        (
            "route",
            format!("[[channel]]\nid='{quoted}'\nurl='https://x.test'"),
        ),
        (
            "route",
            format!("[aliases]\n'{quoted}'='m'\n[[channel]]\nid='c'\nurl='https://x.test'"),
        ),
    ] {
        std::fs::write(&file, txt).unwrap();
        let out = Command::new(env!("CARGO_BIN_EXE_aproxy-format"))
            .args([role, "--config", file.to_str().unwrap()])
            .env("FORMAT_A", "test")
            .output()
            .unwrap();
        assert!(
            !out.status.success() && String::from_utf8_lossy(&out.stderr).contains("64 KiB"),
            "{role}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

/// 有一把可用钥匙不代表整个池健康；缺值、非法字符与代理无法转交的非 ASCII 必须在 hello 前报出变量名。
#[test]
fn key_pool_rejects_unusable_environment_values_before_hello() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("keys.toml");
    std::fs::write(&file,"[[pool]]\norigin='https://a.test'\nheader='authorization'\nkeys=[{id='a',env='FORMAT_A'},{id='b',env='FORMAT_B'}]").unwrap();
    for invalid in [None, Some(""), Some("private\r\nvalue"), Some("privateé")] {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_aproxy-format"));
        cmd.args(["keys", "--config", file.to_str().unwrap()])
            .env("FORMAT_A", "first-private")
            .env_remove("FORMAT_B");
        if let Some(v) = invalid {
            cmd.env("FORMAT_B", v);
        }
        let out = cmd.output().unwrap();
        let error = String::from_utf8(out.stderr).unwrap();
        assert!(
            !out.status.success()
                && out.stdout.is_empty()
                && error.contains("FORMAT_B")
                && !error.contains("private"),
            "status={}, {error}",
            out.status
        );
    }
}

/// 明确指定两端的流可由另一进程只凭 open 开始，不需要借用请求阶段的内存。
fn open_stream(w: &mut Worker, content_type: &str) -> Value {
    w.frame(json!({"stage":"stream","event":"open","stream_id":"r/1","request_id":"r","request":{"method":"POST","url":"https://relay.test/v1"},"response":{"status":200,"headers":{"content-type":content_type,"content-length":"32","set-cookie":"fixture-private-A"}}}))
}
/// data 与 close、abort 均由同一个工作进程顺序确认，测试无需模拟 HTTP 连接。
fn stream_frame(w: &mut Worker, event: &str, bytes: &str) -> Value {
    let mut frame = json!({"stage":"stream","event":event,"stream_id":"r/1"});
    if event == "data" {
        frame["data"] = bytes.into();
    } else if event == "abort" {
        frame["reason"] = json!({"code":"upstream_body_interrupted"});
    }
    w.frame(frame)
}
/// 已选择目标协议后，坏事件只交付一次目标错误，不回 fail，也不让后续原文穿过可选层。
#[test]
fn malformed_stream_ends_in_the_client_protocol_only_once() {
    let mut w = Worker::new(&[
        "convert",
        "--from",
        "anthropic_messages",
        "--to",
        "openai_chat",
    ]);
    open_stream(&mut w, "text/event-stream");
    let valid = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hello\"}}]}\n\n";
    assert!(
        stream_frame(&mut w, "data", valid)["data"]
            .as_str()
            .unwrap()
            .contains("message_start")
    );
    let err = stream_frame(&mut w, "data", "data: not JSON\n\n");
    assert!(err["fail"].is_null());
    assert!(err["data"].as_str().unwrap().starts_with("event: error\n"));
    assert_eq!(stream_frame(&mut w, "data", valid)["data"], "");
    assert!(
        stream_frame(&mut w, "close", "")["data"]
            .as_str()
            .unwrap_or("")
            .is_empty()
    );
}

/// 逐个发送的小事件也会让 Responses 保留整轮文本；到达总预算必须错误结束而不是等到 close 爆帧。
#[test]
fn semantic_budget_counts_complete_events_across_frames() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("convert.toml");
    std::fs::write(
        &config,
        "from='openai_responses'\nto='openai_chat'\nstream_budget_bytes=1048576",
    )
    .unwrap();
    let mut w = Worker::new(&["convert", "--config", config.to_str().unwrap()]);
    open_stream(&mut w, "text/event-stream");
    let event = json!({"choices":[{"index":0,"delta":{"content":"x".repeat(32*1024)}}]});
    let bytes = format!("data: {event}\n\n");
    let mut ended = false;
    for _ in 0..34 {
        let reply = stream_frame(&mut w, "data", &bytes);
        assert!(reply["fail"].is_null());
        let data = reply["data"].as_str().unwrap();
        if ended {
            assert!(data.is_empty());
        } else if data.contains("event: error\n") {
            assert!(data.contains("response exceeds the configured limit"));
            ended = true;
        } else {
            assert!(data.contains("response.output_text.delta"));
        }
    }
    assert!(ended);
    assert!(
        !stream_frame(&mut w, "close", "")
            .to_string()
            .contains("response.completed")
    );
}

/// Chat 的原因不是运输层终点；Messages 不接受 Chat 的哨兵。真终点之后连半帧也不能影响结果。
#[test]
fn only_native_terminal_events_complete_a_stream() {
    let mut w = Worker::new(&[
        "convert",
        "--from",
        "openai_responses",
        "--to",
        "openai_chat",
    ]);
    let finish = "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n";
    open_stream(&mut w, "text/event-stream");
    assert!(
        !stream_frame(&mut w, "data", finish)
            .to_string()
            .contains("response.completed")
    );
    assert!(
        stream_frame(&mut w, "close", "")["data"]
            .as_str()
            .unwrap()
            .contains("event: error")
    );
    open_stream(&mut w, "text/event-stream");
    stream_frame(&mut w, "data", finish);
    stream_frame(
        &mut w,
        "data",
        "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":8,\"completion_tokens\":3}}\n\n",
    );
    let mut out = stream_frame(&mut w, "data", "data: [DONE]\n\ndata: ignored")["data"]
        .as_str()
        .unwrap()
        .to_owned();
    out.push_str(
        stream_frame(&mut w, "close", "")["data"]
            .as_str()
            .unwrap_or(""),
    );
    assert!(out.contains("response.completed") && out.contains("\"input_tokens\":8"));
    let mut m = Worker::new(&[
        "convert",
        "--from",
        "openai_chat",
        "--to",
        "anthropic_messages",
    ]);
    open_stream(&mut m, "text/event-stream");
    assert!(
        stream_frame(&mut m, "data", "data: [DONE]\n\n")["data"]
            .as_str()
            .unwrap()
            .contains("\"error\"")
    );
}

/// 客户端看到工具完成后可以立即执行它；Messages 在原因帧与真正终点之间断开时不能先宣布成功。
#[test]
fn chat_finish_waits_for_messages_stop() {
    let mut w = Worker::new(&[
        "convert",
        "--from",
        "openai_chat",
        "--to",
        "anthropic_messages",
    ]);
    let corpus: Value = serde_json::from_str(include_str!("fixtures/protocols.json")).unwrap();
    let events = corpus["anthropic_messages"]["events"].as_array().unwrap();
    for cut in [true, false] {
        open_stream(&mut w, "text/event-stream");
        for e in &events[..events.len() - 1] {
            let r = stream_frame(&mut w, "data", &format!("data: {e}\n\n"));
            assert!(
                !r["data"]
                    .as_str()
                    .unwrap()
                    .contains("\"finish_reason\":\"tool_calls\"")
            );
        }
        if cut {
            let r = stream_frame(&mut w, "abort", "");
            assert!(r["data"].as_str().unwrap().contains("\"error\""));
        } else {
            let r = stream_frame(
                &mut w,
                "data",
                &format!("data: {}\n\n", events.last().unwrap()),
            );
            assert!(
                r["data"]
                    .as_str()
                    .unwrap()
                    .contains("\"finish_reason\":\"tool_calls\"")
            );
            assert!(
                stream_frame(&mut w, "close", "")["data"]
                    .as_str()
                    .unwrap()
                    .ends_with("data: [DONE]\n\n")
            );
        }
    }
}

/// 合法的 JSON 文档也可能不是成功的模型回复，不能让宽松的解码器虚构空消息。
#[test]
fn valid_json_without_source_response_fields_is_a_clean_502() {
    for (client, source, bad) in [
        ("anthropic_messages", "openai_chat", json!({})),
        (
            "openai_chat",
            "anthropic_messages",
            json!({"role":"assistant"}),
        ),
        (
            "openai_chat",
            "openai_responses",
            json!({"output":"not an array"}),
        ),
        ("openai_responses", "openai_chat", json!({"choices":[{}]})),
    ] {
        let mut w = Worker::new(&["convert", "--from", client, "--to", source]);
        let r = w.frame(response(&Value::Null, &bad.to_string()));
        assert_eq!(r["fail"]["response"]["status"], 502);
        assert!(r["response"].is_null());
        assert_eq!(
            r["fail"]["response"]["headers"],
            json!({"content-type":"application/json"})
        );
    }
}

/// Chat 能呈现 thinking 文本和工具调用，但没有可回放的 Anthropic 签名字段。续轮保留工具关联，不能伪造签名。
#[test]
fn streamed_thinking_tool_continuation_preserves_tools_but_not_the_signature() {
    let mut w = Worker::new(&[
        "convert",
        "--from",
        "openai_chat",
        "--to",
        "anthropic_messages",
    ]);
    open_stream(&mut w, "text/event-stream");
    let corpus: Value = serde_json::from_str(include_str!("fixtures/protocols.json")).unwrap();
    let native = corpus["anthropic_messages"]["events"].as_array().unwrap();
    let mut events = vec![
        native[0].clone(),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"checking"}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"fixture-signed"}}),
    ];
    events.extend_from_slice(&native[3..]);
    let mut text = String::new();
    for e in events {
        text.push_str(
            stream_frame(&mut w, "data", &format!("data: {e}\n\n"))["data"]
                .as_str()
                .unwrap(),
        );
    }
    text.push_str(
        stream_frame(&mut w, "close", "")["data"]
            .as_str()
            .unwrap_or(""),
    );
    let mut thinking = String::new();
    let mut id = String::new();
    let mut name = String::new();
    let mut args = String::new();
    for data in text
        .lines()
        .filter_map(|l| l.strip_prefix("data: "))
        .filter(|s| *s != "[DONE]")
    {
        let chunk: Value = serde_json::from_str(data).unwrap();
        let delta = &chunk["choices"][0]["delta"];
        thinking.push_str(delta["reasoning_content"].as_str().unwrap_or(""));
        let call = &delta["tool_calls"][0];
        id.push_str(call["id"].as_str().unwrap_or(""));
        name.push_str(call["function"]["name"].as_str().unwrap_or(""));
        args.push_str(call["function"]["arguments"].as_str().unwrap_or(""));
    }
    assert_eq!(
        (&*thinking, &*id, &*name),
        ("checking", "call_one", "lookup")
    );
    assert_eq!(
        serde_json::from_str::<Value>(&args).unwrap(),
        json!({"q":"x"})
    );
    assert!(!text.contains("fixture-signed"));
    let continuation = json!({"model":"fixture-model","max_tokens":128,"messages":[{"role":"user","content":"hello"},{"role":"assistant","reasoning_content":thinking,"tool_calls":[{"id":id,"type":"function","function":{"name":name,"arguments":args}}]},{"role":"tool","tool_call_id":id,"content":"found"}]});
    let r = w.frame(request(
        "https://relay.test/v1/chat/completions",
        "/v1/chat/completions",
        continuation,
    ));
    let body: Value = serde_json::from_str(r["request"]["body"].as_str().unwrap()).unwrap();
    assert!(
        body["messages"][1]["content"]
            .as_array()
            .unwrap()
            .iter()
            .all(|b| b["type"] != "thinking")
    );
    assert_eq!(body["messages"][2]["content"][0]["tool_use_id"], id);
}

/// Chat 的计量开关只属于流式请求；Messages 和 Responses 不应要求操作者手工补上游字段。
#[test]
fn translating_a_stream_to_chat_requests_native_usage() {
    let corpus: Value = serde_json::from_str(include_str!("fixtures/protocols.json")).unwrap();
    for client in ["anthropic_messages", "openai_responses"] {
        let mut w = Worker::new(&["convert", "--from", client, "--to", "openai_chat"]);
        for stream in [true, false] {
            let mut body = corpus[client]["request"].clone();
            body["stream"] = stream.into();
            let r = w.frame(request("https://relay.test/custom", "/custom", body));
            let patch: Value =
                serde_json::from_str(r["request"]["body"].as_str().unwrap()).unwrap();
            assert_eq!(
                patch["stream_options"]["include_usage"].as_bool(),
                stream.then_some(true)
            );
        }
    }
}

/// 独立的流进程只读请求交回的状态；即使上游在结束前报了用量，Chat 也恰好收到一个最后的空 choices 块。
#[test]
fn requested_chat_usage_is_one_empty_choices_chunk_before_done() {
    let corpus: Value = serde_json::from_str(include_str!("fixtures/protocols.json")).unwrap();
    for source in ["anthropic_messages", "openai_responses"] {
        let args = ["convert", "--from", "openai_chat", "--to", source];
        let mut req = Worker::new(&args);
        let mut body = corpus["openai_chat"]["request"].clone();
        body["stream"] = true.into();
        body["stream_options"] = json!({"include_usage":true});
        let state =
            req.frame(request("https://relay.test/custom", "/custom", body))["state"].clone();
        drop(req);
        let mut w = Worker::new(&args);
        w.frame(json!({"stage":"stream","event":"open","stream_id":"r/1","request_id":"r","state":state,"request":{"method":"POST","url":"https://relay.test/"},"response":{"status":200,"headers":{"content-type":"text/event-stream"}}}));
        let mut text = String::new();
        for e in corpus[source]["events"].as_array().unwrap() {
            text.push_str(
                stream_frame(&mut w, "data", &format!("data: {e}\n\n"))["data"]
                    .as_str()
                    .unwrap(),
            );
        }
        text.push_str(stream_frame(&mut w, "close", "")["data"].as_str().unwrap());
        let chunks: Vec<Value> = text
            .strip_suffix("data: [DONE]\n\n")
            .unwrap()
            .lines()
            .filter_map(|s| s.strip_prefix("data: "))
            .map(|s| serde_json::from_str(s).unwrap())
            .collect();
        let (last, rest) = chunks.split_last().unwrap();
        assert_eq!(last["choices"], json!([]));
        assert_eq!(last["usage"]["prompt_tokens"], 7);
        assert_eq!(last["usage"]["completion_tokens"], 3);
        assert!(rest.iter().all(|c| c["usage"].is_null()));
    }
}

/// 把同一批源事件逐个交给独占进程。解析真实 SSE 而非匹配转义后的协议管道字符串。
fn translated_events(w: &mut Worker, events: &[Value], chat_sentinel: bool) -> Vec<Value> {
    let mut text = String::new();
    for e in events {
        text.push_str(
            stream_frame(w, "data", &format!("data: {e}\n\n"))["data"]
                .as_str()
                .unwrap(),
        );
    }
    if chat_sentinel {
        text.push_str(
            stream_frame(w, "data", "data: [DONE]\n\n")["data"]
                .as_str()
                .unwrap(),
        );
    }
    text.push_str(stream_frame(w, "close", "")["data"].as_str().unwrap_or(""));
    text.lines()
        .filter_map(|l| l.strip_prefix("data: "))
        .filter(|s| *s != "[DONE]")
        .map(|s| serde_json::from_str(s).unwrap())
        .collect()
}
/// Responses 的 incomplete 可带最终快照和用量；过滤与 token 限额不是同一种停止原因。
#[test]
fn incomplete_reason_usage_and_terminal_snapshot_cross_both_directions() {
    let corpus: Value = serde_json::from_str(include_str!("fixtures/protocols.json")).unwrap();
    for (reason, chat, messages) in [
        ("content_filter", "content_filter", "refusal"),
        ("max_output_tokens", "length", "max_tokens"),
    ] {
        let mut snapshot = corpus["openai_responses"]["response"].clone();
        snapshot["status"] = "incomplete".into();
        snapshot["incomplete_details"] = json!({"reason":reason});
        for (client, stop) in [("openai_chat", chat), ("anthropic_messages", messages)] {
            let mut w = Worker::new(&["convert", "--from", client, "--to", "openai_responses"]);
            open_stream(&mut w, "text/event-stream");
            let out = translated_events(
                &mut w,
                &[json!({"type":"response.incomplete","response":snapshot})],
                false,
            );
            assert!(
                out.iter().any(|c| c["choices"][0]["finish_reason"] == stop
                    || c["delta"]["stop_reason"] == stop),
                "{client} {reason}"
            );
            assert!(
                out.iter()
                    .any(|c| c["usage"]["input_tokens"] == 7 || c["usage"]["prompt_tokens"] == 7)
            );
            assert!(
                out.iter()
                    .any(|c| c["usage"]["output_tokens"] == 3
                        || c["usage"]["completion_tokens"] == 3)
            );
            assert!(out.iter().any(|c| c["delta"]["text"] == "hello λ"
                || c["choices"][0]["delta"]["content"] == "hello λ"));
        }
        let mut w = Worker::new(&[
            "convert",
            "--from",
            "openai_responses",
            "--to",
            "openai_chat",
        ]);
        open_stream(&mut w, "text/event-stream");
        let out = translated_events(
            &mut w,
            &[
                json!({"choices":[{"index":0,"delta":{"content":"text"},"finish_reason":chat}],"usage":{"prompt_tokens":7,"completion_tokens":3}}),
            ],
            true,
        );
        let end = out.last().unwrap();
        assert_eq!(end["type"], "response.incomplete");
        assert_eq!(end["response"]["incomplete_details"]["reason"], reason);
        assert_eq!(end["response"]["usage"]["input_tokens"], 7);
    }
}

/// 拒绝文本有时只在终态快照里，有时先逐段发送；目标端都应看到一次完整文本而非空消息或重复。
#[test]
fn refusal_text_survives_deltas_and_snapshot_reconciliation() {
    for deltas in [false, true] {
        let mut w = Worker::new(&[
            "convert",
            "--from",
            "anthropic_messages",
            "--to",
            "openai_responses",
        ]);
        open_stream(&mut w, "text/event-stream");
        let mut events = Vec::new();
        if deltas {
            events.push(json!({"type":"response.refusal.delta","item_id":"m1","output_index":0,"content_index":0,"delta":"No thanks"}));
        }
        events.push(json!({"type":"response.completed","response":{"id":"r","output":[{"type":"message","id":"m1","role":"assistant","content":[{"type":"refusal","refusal":"No thanks"}]}]}}));
        let out = translated_events(&mut w, &events, false);
        let text = out
            .iter()
            .filter_map(|e| e["delta"]["text"].as_str())
            .collect::<String>();
        assert_eq!(text, "No thanks");
    }
    let mut w = Worker::new(&[
        "convert",
        "--from",
        "openai_responses",
        "--to",
        "openai_chat",
    ]);
    open_stream(&mut w, "text/event-stream");
    let out = translated_events(
        &mut w,
        &[
            json!({"choices":[{"index":0,"delta":{"refusal":"No thanks"},"finish_reason":"content_filter"}]}),
        ],
        true,
    );
    assert_eq!(
        out.iter()
            .filter(|c| c["type"] == "response.output_text.delta")
            .filter_map(|c| c["delta"].as_str())
            .collect::<String>(),
        "No thanks"
    );
}

/// 工具名也由 Chat 分片传入，参数开始后名称仍可能继续；目标工具开头不能撤销或更名。
#[test]
fn chat_tool_name_fragments_and_argument_delivery_boundary() {
    for client in ["anthropic_messages", "openai_responses"] {
        let mut w = Worker::new(&["convert", "--from", client, "--to", "openai_chat"]);
        open_stream(&mut w, "text/event-stream");
        let mut early = String::new();
        for tool in [
            json!({"index":0,"id":"c1","function":{"name":"look","arguments":"{"}}),
            json!({"index":0,"function":{"name":"up","arguments":"}"}}),
        ] {
            let event = json!({"choices":[{"index":0,"delta":{"tool_calls":[tool]}}]});
            early.push_str(
                stream_frame(&mut w, "data", &format!("data: {event}\n\n"))["data"]
                    .as_str()
                    .unwrap(),
            );
        }
        assert!(
            !early.contains("input_json_delta")
                && !early.contains("response.function_call_arguments.delta"),
            "{client}: {early}"
        );
        let out = translated_events(
            &mut w,
            &[json!({"choices":[{"delta":{},"finish_reason":"tool_calls"}]})],
            true,
        );
        assert!(
            out.iter()
                .any(|e| e["content_block"]["name"] == "lookup" || e["item"]["name"] == "lookup")
        );
        let args = out
            .iter()
            .filter_map(|e| {
                if e["type"] == "response.function_call_arguments.delta" {
                    e["delta"].as_str()
                } else {
                    e["delta"]["partial_json"].as_str()
                }
            })
            .collect::<String>();
        assert_eq!(args, "{}");
    }
}

/// Responses 的函数项在后续文本之前已经结束；Messages 客户端不应同时看见两个打开的内容块。
#[test]
fn responses_function_done_closes_the_messages_block_before_later_text() {
    let mut w = Worker::new(&[
        "convert",
        "--from",
        "anthropic_messages",
        "--to",
        "openai_responses",
    ]);
    open_stream(&mut w, "text/event-stream");
    let events = [
        json!({"type":"response.output_item.added","output_index":0,"item":{"id":"fc1","type":"function_call","call_id":"c1","name":"one","arguments":""}}),
        json!({"type":"response.function_call_arguments.delta","output_index":0,"item_id":"fc1","delta":"{}"}),
        json!({"type":"response.output_item.done","output_index":0,"item":{"id":"fc1","type":"function_call","call_id":"c1","name":"one","arguments":"{}"}}),
        json!({"type":"response.output_text.delta","output_index":1,"content_index":0,"delta":"after"}),
        json!({"type":"response.completed","response":{"output":[]}}),
    ];
    let out = translated_events(&mut w, &events, false);
    let mut active = None;
    let mut starts = Vec::new();
    let mut stops = Vec::new();
    for e in &out {
        match e["type"].as_str() {
            Some("content_block_start") => {
                assert!(active.is_none(), "overlap: {e}");
                let index = e["index"].as_u64().unwrap();
                active = Some(index);
                starts.push(index);
            }
            Some("content_block_stop") => {
                let index = e["index"].as_u64().unwrap();
                assert_eq!(active.take(), Some(index));
                stops.push(index);
            }
            Some("content_block_delta") => assert_eq!(active, e["index"].as_u64()),
            _ => (),
        }
    }
    assert_eq!(starts, [0, 1]);
    assert_eq!(stops, starts);
    assert!(active.is_none());
}
