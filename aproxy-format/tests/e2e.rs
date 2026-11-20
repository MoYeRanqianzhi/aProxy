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
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_aproxy-format"));
        cmd.args(args);
        Self::connect(cmd, args[0], false, None)
    }
    /// 固定时钟的子进程也读写真正的协议管道；仅先跳过 libtest 自己的启动横幅。
    fn connect(mut cmd: Command, role: &str, frozen: bool, limit: Option<usize>) -> Self {
        let mut child = cmd
            .env("FORMAT_A", "fixture-private-A")
            .env("FORMAT_B", "fixture-private-B")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let input = child.stdin.take();
        let mut output = BufReader::new(child.stdout.take().unwrap());
        if frozen {
            loop {
                let mut line = String::new();
                assert_ne!(output.read_line(&mut line).unwrap(), 0);
                if line.trim_end().ends_with("replay ready") {
                    break;
                }
            }
        }
        let mut w = Self {
            child,
            input,
            output,
            seq: 0,
            hello: Value::Null,
        };
        let h=w.frame(json!({"stage":"hello","protocols":[1],"aproxy_version":"test","name":"role","process":"persistent","stages":[],"worker_id":0,"limits":limit.map(|n| json!({"response":n})),"features":["decide.success"],"overrides":[]}));
        assert_eq!(h["protocol"], 1);
        assert_eq!(h["name"], role);
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
/// 只在测试二进制显式选中时运行；单调时间与日期固定，不给正式命令引入环境开关。
#[test]
#[ignore = "the parent drives the worker protocol over stdin"]
fn frozen_key_worker() {
    let config = std::env::var("FORMAT_SAMPLE_CONFIG").unwrap();
    println!("replay ready");
    std::io::stdout().flush().unwrap();
    aproxy_format::replay_keys_at(
        &["--config".into(), config],
        std::io::stdin().lock(),
        std::io::stdout().lock(),
        std::time::Instant::now(),
        std::time::SystemTime::UNIX_EPOCH,
    )
    .unwrap();
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
/// 协议相同不改 HTTP 表示，不解析私有扩展或压缩字节，也不复制地址中的用户信息。
#[test]
fn explicit_and_detected_same_protocol_leave_http_untouched() {
    for (from, to) in [
        ("anthropic_messages", "anthropic_messages"),
        ("auto", "auto"),
    ] {
        let mut w = Worker::new(&["convert", "--from", from, "--to", to]);
        let mut m = request(
            "https://account:fixture-private@relay.test/v1/messages",
            "/v1/messages",
            Value::Null,
        );
        m["request"]["headers"] =
            json!({"content-encoding":"gzip","content-type":"application/vendor+json"});
        m["request"]["body"] = "opaque entity".into();
        let r = w.frame(m);
        assert_eq!(
            r["state"],
            json!(["anthropic_messages", "anthropic_messages"]).to_string()
        );
        assert!(r.get("request").is_none() && r.get("fail").is_none(), "{r}");
        assert!(!r.to_string().contains("fixture-private"));
    }
}
/// 目的端先按 origin 与最长路径段匹配，再看标准端点；回程在另一进程不重猜当前 URL。
#[test]
fn automatic_upstream_uses_route_origin_and_longest_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("convert.toml");
    std::fs::write(&f,"from='openai_chat'\nto='auto'\n[[upstream]]\norigin='https://mixed.test'\nprotocol='openai_chat'\n[[upstream]]\norigin='https://mixed.test:443'\npath_prefix='/tenant'\nprotocol='anthropic_messages'\n[[upstream]]\norigin='https://mixed.test'\npath_prefix='/租户/'\nprotocol='anthropic_messages'\n[[upstream]]\norigin='https://other-provider.test'\nprotocol='anthropic_messages'").unwrap();
    let args = ["convert", "--config", f.to_str().unwrap()];
    let mut w = Worker::new(&args);
    for (url, proto) in [
        (
            "https://other-provider.test/v1/chat/completions?tenant=1",
            "anthropic_messages",
        ),
        ("https://mixed.test/tenant/custom", "anthropic_messages"),
        (
            "https://mixed.test/%E7%A7%9F%E6%88%B7/v1/responses",
            "anthropic_messages",
        ),
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
        assert!(r["request"].get("url").is_none(), "{url}: {r}");
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
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "frozen_key_worker", "--ignored", "--nocapture"])
        .env("FORMAT_SAMPLE_CONFIG", &file);
    let mut w = Worker::connect(command, "keys", true, None);
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
            if client == "openai_responses" {
                let event = abort["data"]
                    .as_str()
                    .unwrap()
                    .lines()
                    .find_map(|l| l.strip_prefix("data: "))
                    .unwrap();
                assert_eq!(
                    serde_json::from_str::<Value>(event).unwrap()["sequence_number"],
                    0
                );
                assert!(!tail.contains("sequence_number"));
            } else {
                assert_eq!(abort["data"], tail);
            }
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

/// 诊断指出配置所有者能修正的字段；不能显示错误的原值、原始行或自定义头名。
#[test]
fn config_diagnostic_names_the_field_not_the_value() {
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("config.toml");
    for (role, text, key) in [
        (
            "rewrite",
            "remove_headers=123 # private-fixture",
            "remove_headers",
        ),
        (
            "route",
            "[[channel]]\nid='private-fixture'\nurl='https://a.test'\nweight='private-fixture'",
            "weight",
        ),
        (
            "rewrite",
            "[set_headers]\nprivate-fixture=123",
            "set_headers",
        ),
        ("rewrite", "[set_headers]\nmodel=123", "set_headers"),
        ("rewrite", "[models]\nweight=123", "models"),
        ("route", "[aliases]\nurl=123", "aliases"),
    ] {
        std::fs::write(&f, text).unwrap();
        let r = Command::new(env!("CARGO_BIN_EXE_aproxy-format"))
            .args([role, "--config"])
            .arg(&f)
            .output()
            .unwrap();
        let err = String::from_utf8(r.stderr).unwrap();
        assert!(
            !r.status.success()
                && r.stdout.is_empty()
                && err.contains(key)
                && !err.contains("private-fixture")
                && !err.contains("123"),
            "{err}"
        );
    }
}

/// 未编辑的整数与小数不得经 f64 舍入，删除中间成员也不能改变其他成员的顺序。
#[test]
fn rewrite_preserves_untouched_numbers_and_member_order() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("rewrite.toml");
    std::fs::write(&file, "remove=['/drop']\n[set]\n'/edited'='new'").unwrap();
    let mut w = Worker::new(&["rewrite", "--config", file.to_str().unwrap()]);
    let mut m = request("https://a.test/", "/", Value::Null);
    let original = r#"{"z":184467440737095516161111,"drop":0,"f":0.123456789012345678901,"edited":"old","a":{"z":7,"b":2}}"#;
    m["request"]["body"] = original.into();
    assert_eq!(
        w.frame(m)["request"]["body"],
        original.replace("\"drop\":0,", "").replace("old", "new")
    );
    for value in ["7", "not-a-number"] {
        let original = r#"{"meta":{"\u0024serde_json::private::Number":"VALUE","$$serde_json::private::Number":0},"dup":1,"dup":2,"escape":"\u03bb","literal":"\"$key\":","edited":"old"}"#.replace("VALUE", value);
        let expected = r#"{"meta":{"$serde_json::private::Number":"VALUE","$$serde_json::private::Number":0},"dup":2,"escape":"λ","literal":"\"$key\":","edited":"new"}"#.replace("VALUE", value);
        let mut m = request("https://a.test/", "/", Value::Null);
        m["request"]["body"] = original.into();
        assert_eq!(w.frame(m)["request"]["body"], expected);
    }
}

/// 即使语法合法，代理不会转交的头也不能作认证或改写的赋值目标。
#[test]
fn managed_header_slots_fail_before_hello() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("role.toml");
    for (role, text) in [
        (
            "keys",
            "[[pool]]\norigin='https://a.test'\nheader='Proxy-Authorization'\nkeys=[{id='a',env='KEY'}]",
        ),
        (
            "route",
            "[[channel]]\nid='a'\nurl='https://a.test'\nauth_header='Proxy-Authenticate'\ncredential_env='KEY'",
        ),
        (
            "rewrite",
            "[set_headers]\nTransfer-Encoding='private-fixture'",
        ),
    ] {
        std::fs::write(&file, text).unwrap();
        let out = Command::new(env!("CARGO_BIN_EXE_aproxy-format"))
            .args([role, "--config"])
            .arg(&file)
            .env("KEY", "private-fixture")
            .output()
            .unwrap();
        assert!(!out.status.success() && out.stdout.is_empty());
        let err = String::from_utf8(out.stderr).unwrap();
        assert!(
            err.contains("代理管理") && !err.contains("private-fixture"),
            "{err}"
        );
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
        (
            "https://b.test/base?view=fixed",
            "carry_query=['view']",
            "https://b.test/base/chat?view=fixed",
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
/// 双向经过真实请求帧，文字/图片的顺序、工具名、调用标识以及每条结果的归属都保留。
#[test]
fn multipart_image_and_tool_results_roundtrip_over_the_wire() {
    let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII=";
    let body = json!({"model":"m","max_tokens":16,"messages":[{"role":"user","content":[{"type":"text","text":"look"},{"type":"image","source":{"type":"base64","media_type":"image/png","data":png}},{"type":"text","text":"after"}]},{"role":"assistant","content":[{"type":"tool_use","id":"call_a","name":"left","input":{"p":"a"}},{"type":"tool_use","id":"call_b","name":"right","input":{"p":"b"}}]},{"role":"user","content":[{"type":"tool_result","tool_use_id":"call_a","content":"left output"},{"type":"tool_result","tool_use_id":"call_b","content":"right output"}]}]});
    let convert = |from, to, v| {
        let mut w = Worker::new(&["convert", "--from", from, "--to", to]);
        let r = w.frame(request("https://relay.test/custom", "/custom", v));
        serde_json::from_str::<Value>(r["request"]["body"].as_str().unwrap()).unwrap()
    };
    for target in ["openai_chat", "openai_responses"] {
        let outgoing = convert("anthropic_messages", target, body.clone());
        let image = if target == "openai_chat" {
            &outgoing["messages"][0]["content"][1]["image_url"]["url"]
        } else {
            &outgoing["input"][0]["content"][1]["image_url"]
        };
        assert_eq!(*image, format!("data:image/png;base64,{png}"));
        let restored = convert(target, "anthropic_messages", outgoing);
        assert_eq!(restored["messages"][0], body["messages"][0]);
        let parts: Vec<_> = restored["messages"].as_array().unwrap()[1..]
            .iter()
            .flat_map(|m| m["content"].as_array().unwrap())
            .collect();
        assert_eq!(parts.len(), 4);
        for expected in body["messages"].as_array().unwrap()[1..]
            .iter()
            .flat_map(|m| m["content"].as_array().unwrap())
        {
            assert!(parts.contains(&expected), "{restored}");
        }
    }
}
/// 混合发送两个工具的参数片段；分别核对输出项和内容块的索引、完整 JSON、起止次数与次序。
#[test]
fn parallel_tool_deltas_have_exact_target_lifecycles() {
    let chunks = vec![
        json!({"role":"assistant","tool_calls":[{"index":0,"id":"call_a","type":"function","function":{"name":"left","arguments":"{\"p\":"}},{"index":1,"id":"call_b","type":"function","function":{"name":"right","arguments":"{\"q\":"}}]}),
        json!({"tool_calls":[{"index":1,"function":{"arguments":"2}"}},{"index":0,"function":{"arguments":"1}"}}]}),
        json!({}),
    ];
    let input:Vec<_>=chunks.into_iter().map(|delta|json!({"id":"c","model":"m","created":1,"choices":[{"index":0,"finish_reason":if delta==json!({}){Some("tool_calls")}else{None},"delta":delta}]})).collect();
    for target in ["anthropic_messages", "openai_responses"] {
        let mut w = Worker::new(&["convert", "--from", target, "--to", "openai_chat"]);
        open_stream(&mut w, "text/event-stream");
        let ev = translated_events(&mut w, &input, true);
        let kinds: Vec<_> = ev.iter().map(|e| e["type"].as_str().unwrap()).collect();
        if target == "anthropic_messages" {
            assert_eq!(
                kinds,
                [
                    "message_start",
                    "content_block_start",
                    "content_block_delta",
                    "content_block_stop",
                    "content_block_start",
                    "content_block_delta",
                    "content_block_stop",
                    "message_delta",
                    "message_stop"
                ]
            );
            for (i, (id, name, args)) in [
                ("call_a", "left", json!({"p":1})),
                ("call_b", "right", json!({"q":2})),
            ]
            .into_iter()
            .enumerate()
            {
                let group = &ev[1 + 3 * i..4 + 3 * i];
                assert!(group.iter().all(|e| e["index"] == i));
                assert_eq!(group[0]["content_block"]["id"], id);
                assert_eq!(group[0]["content_block"]["name"], name);
                assert_eq!(
                    serde_json::from_str::<Value>(
                        group[1]["delta"]["partial_json"].as_str().unwrap()
                    )
                    .unwrap(),
                    args
                );
            }
        } else {
            assert_eq!(
                kinds,
                [
                    "response.created",
                    "response.output_item.added",
                    "response.function_call_arguments.delta",
                    "response.output_item.added",
                    "response.function_call_arguments.delta",
                    "response.function_call_arguments.done",
                    "response.output_item.done",
                    "response.function_call_arguments.done",
                    "response.output_item.done",
                    "response.completed"
                ]
            );
            for (i, e) in ev.iter().enumerate() {
                assert_eq!(e["sequence_number"], i);
            }
            for (i, (id, name, args)) in [
                ("call_a", "left", json!({"p":1})),
                ("call_b", "right", json!({"q":2})),
            ]
            .into_iter()
            .enumerate()
            {
                let added = &ev[1 + i * 2];
                let done = &ev[6 + i * 2];
                assert_eq!(added["item"]["call_id"], id);
                assert_eq!(done["output_index"], i);
                assert_eq!(done["item"]["id"], added["item"]["id"]);
                assert_eq!(done["item"]["name"], name);
                assert_eq!(
                    serde_json::from_str::<Value>(done["item"]["arguments"].as_str().unwrap())
                        .unwrap(),
                    args
                );
                assert_eq!(ev[9]["response"]["output"][i], done["item"]);
            }
        }
    }
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

/// 编解码器既可能拒绝原生项，也可能发现完成快照撤回了已经发出的文本；两者都是一次类型化错误。
#[test]
fn codec_errors_have_one_typed_ending_even_on_abort() {
    let mut w = Worker::new(&[
        "convert",
        "--from",
        "anthropic_messages",
        "--to",
        "openai_responses",
    ]);
    for (prefix, event) in [
        (
            None,
            json!({"type":"response.output_item.added","output_index":0,"item":{"type":"image_generation_call"}}),
        ),
        (
            Some(
                json!({"type":"response.output_text.delta","output_index":0,"content_index":0,"delta":"hello"}),
            ),
            json!({"type":"response.completed","response":{"output":[{"type":"message","content":[{"type":"output_text","text":"different"}]}]}}),
        ),
    ] {
        open_stream(&mut w, "text/event-stream");
        if let Some(p) = prefix {
            stream_frame(&mut w, "data", &format!("data: {p}\n\n"));
        }
        let reply = stream_frame(&mut w, "data", &format!("data: {event}\n\n"));
        let data = reply["data"].as_str().unwrap();
        let frame: Value =
            serde_json::from_str(data.strip_prefix("event: error\ndata: ").unwrap().trim())
                .unwrap();
        assert_eq!(frame["error"]["type"], "api_error");
        assert_eq!(stream_frame(&mut w, "abort", "")["data"], "");
    }
}

/// 有签名的推理会出现在摘要和封装载荷中；协商上限包括真实响应头、序号和换行。
#[test]
fn buffered_replies_fit_the_negotiated_line() {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_aproxy-format"));
    cmd.args([
        "convert",
        "--from",
        "openai_responses",
        "--to",
        "anthropic_messages",
    ]);
    let mut w = Worker::connect(cmd, "convert", false, Some(4096));
    let body = json!({"role":"assistant","model":"m","content":[{"type":"thinking","thinking":"\\".repeat(380),"signature":"signed"}]}).to_string();
    let rejected = w.frame(response(&Value::Null, &body));
    assert_eq!(rejected["fail"]["response"]["status"], 502);
    assert!(
        rejected["fail"]["response"]["body"]
            .as_str()
            .unwrap()
            .contains("configured limit")
    );
    open_stream(&mut w, "application/json");
    stream_frame(&mut w, "data", &body);
    let close = stream_frame(&mut w, "close", "");
    assert!(close["data"].as_str().unwrap().contains("configured limit"));
    let small = json!({"role":"assistant","content":[{"type":"text","text":"ok"}]}).to_string();
    let mut headers = response(&Value::Null, &small);
    headers["response"]["headers"]["x-long"] = "h".repeat(4000).into();
    assert_eq!(w.frame(headers)["fail"]["response"]["status"], 502);
    assert!(w.frame(response(&Value::Null, &small))["response"]["body"].is_string());
}
/// 合法边界与分帧用量合计都经过同一工作进程；不一致的计数只结束该流，不能污染下一条。
#[test]
fn token_usage_is_checked_before_encoding() {
    let mut w = Worker::new(&[
        "convert",
        "--from",
        "openai_chat",
        "--to",
        "anthropic_messages",
    ]);
    for (input, cache, output, valid) in [
        (u64::MAX - 3, 2, 1, true),
        (10, 1, 2, true),
        (u64::MAX - 2, 2, 1, false),
    ] {
        open_stream(&mut w, "text/event-stream");
        let start = json!({"type":"message_start","message":{"id":"m","model":"m","role":"assistant","content":[],"usage":{"input_tokens":input,"cache_read_input_tokens":cache}}});
        let delta = json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":output}});
        let mut data = String::new();
        for event in [start, delta, json!({"type":"message_stop"})] {
            data += stream_frame(&mut w, "data", &format!("data: {event}\n\n"))["data"]
                .as_str()
                .unwrap();
        }
        data += stream_frame(&mut w, "close", "")["data"]
            .as_str()
            .unwrap_or("");
        assert_eq!(data.contains("[DONE]"), valid);
        assert_eq!(data.contains("stream conversion aborted"), !valid);
    }
    let mut w = Worker::new(&[
        "convert",
        "--from",
        "anthropic_messages",
        "--to",
        "openai_responses",
    ]);
    // 缓冲解码器同时接受 Chat 拼法；首选字段存在时不能再把别名计入总量。
    for (usage, valid) in [
        (
            json!({"prompt_tokens":u64::MAX,"completion_tokens":1}),
            false,
        ),
        (
            json!({"prompt_tokens":1,"prompt_tokens_details":{"cached_tokens":2}}),
            false,
        ),
        (
            json!({"input_tokens":1,"input_tokens_details":{},"prompt_tokens_details":{"cache_creation_tokens":2}}),
            false,
        ),
        (
            json!({"input_tokens":1,"prompt_tokens":u64::MAX,"output_tokens":1,"completion_tokens":u64::MAX,"input_tokens_details":{"cached_tokens":0,"cache_write_tokens":1},"prompt_tokens_details":{"cached_tokens":20,"cache_creation_tokens":20}}),
            true,
        ),
    ] {
        let reply = w.frame(response(
            &Value::Null,
            &json!({"model":"m","output":[],"usage":usage}).to_string(),
        ));
        assert_eq!(reply["fail"].is_null(), valid, "{reply}");
    }
    for usage in [
        json!({"input_tokens":5,"output_tokens":3,"total_tokens":7}),
        json!({"input_tokens":1,"output_tokens":1,"input_tokens_details":{"cached_tokens":2}}),
        json!({"input_tokens":u64::MAX,"output_tokens":1}),
    ] {
        open_stream(&mut w, "text/event-stream");
        let event =
            json!({"type":"response.completed","response":{"model":"m","output":[],"usage":usage}});
        assert!(
            stream_frame(&mut w, "data", &format!("data: {event}\n\n"))["data"]
                .as_str()
                .unwrap()
                .contains("event: error")
        );
    }
}

/// 相同 SSE 字节合在一帧或拆开发送，都应先交付正常增量，再以客户端协议报错。
#[test]
fn valid_prefix_before_bad_event_is_independent_of_pipe_framing() {
    let delta = "data: {\"id\":\"c\",\"model\":\"m\",\"created\":1,\"choices\":[{\"index\":0,\"delta\":{\"content\":\"visible text\"}}]}\n\n";
    for bad in ["data: broken\n\n", "data: {\"error\":{}}\n\n"] {
        let mut w = Worker::new(&[
            "convert",
            "--from",
            "openai_responses",
            "--to",
            "openai_chat",
        ]);
        for split in [true, false] {
            open_stream(&mut w, "text/event-stream");
            let mut transcript = String::new();
            for chunk in if split {
                vec![delta.into(), bad.into()]
            } else {
                vec![format!("{delta}{bad}")]
            } {
                transcript += stream_frame(&mut w, "data", &chunk)["data"]
                    .as_str()
                    .unwrap();
            }
            transcript += stream_frame(&mut w, "close", "")["data"]
                .as_str()
                .unwrap_or("");
            let events: Vec<Value> = transcript
                .lines()
                .filter_map(|l| l.strip_prefix("data: "))
                .map(|l| serde_json::from_str(l).unwrap())
                .collect();
            assert!(events.iter().any(|e| e["delta"] == "visible text"));
            for (i, e) in events.iter().enumerate() {
                assert_eq!(e["sequence_number"], i);
            }
            assert_eq!(events.last().unwrap()["type"], "error");
            assert_eq!(events.iter().filter(|e| e["type"] == "error").count(), 1);
        }
    }
}

/// 六个方向都把源协议的失败变成一个有类型的本地错误；正常 EOF 和随后的断线均不再重复。
#[test]
fn provider_errors_emit_one_typed_client_error() {
    for (source, error) in [
        (
            "openai_chat",
            json!({"error":{"message":"private-fixture"}}),
        ),
        (
            "anthropic_messages",
            json!({"type":"error","error":{"type":"api_error","message":"private-fixture"}}),
        ),
        (
            "openai_responses",
            json!({"type":"response.failed","response":{"error":{"message":"private-fixture"}}}),
        ),
        (
            "openai_responses",
            json!({"type":"error","message":"private-fixture"}),
        ),
    ] {
        for target in ["openai_chat", "anthropic_messages", "openai_responses"]
            .into_iter()
            .filter(|t| *t != source)
        {
            let mut w = Worker::new(&["convert", "--from", target, "--to", source]);
            for end in ["close", "abort"] {
                open_stream(&mut w, "text/event-stream");
                let r = stream_frame(&mut w, "data", &format!("data: {error}\n\n"));
                let text = r["data"].as_str().unwrap();
                assert!(!text.contains("private-fixture"));
                let fields: Vec<Value> = text
                    .lines()
                    .filter_map(|l| l.strip_prefix("data: "))
                    .map(|l| serde_json::from_str(l).unwrap())
                    .collect();
                assert_eq!(fields.len(), 1, "{text}");
                assert_eq!(fields[0]["error"]["type"], "api_error");
                assert_eq!(fields[0]["error"]["message"], "upstream stream failed");
                if target != "openai_chat" {
                    assert_eq!(fields[0]["type"], "error");
                }
                assert_eq!(stream_frame(&mut w, "data", "ignored")["data"], "");
                assert!(
                    stream_frame(&mut w, end, "")["data"]
                        .as_str()
                        .unwrap_or("")
                        .is_empty()
                );
            }
        }
    }
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
        if client == "openai_responses" {
            let mut invalid = corpus[client]["request"].clone();
            invalid["stream"] = true.into();
            invalid["stream_options"] = false.into();
            assert!(
                w.frame(request("https://relay.test/custom", "/custom", invalid))["fail"]
                    .is_object()
            );
        }
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
    for (kind, id) in [
        ("function_call", "c1"),
        ("function_call", "call/a"),
        ("custom_tool_call", "call/b"),
    ] {
        open_stream(&mut w, "text/event-stream");
        let mut events = vec![
            json!({"type":"response.output_item.added","output_index":0,"item":{"id":"fc1","type":kind,"call_id":id,"name":"one","arguments":""}}),
            json!({"type":"response.function_call_arguments.delta","output_index":0,"item_id":"fc1","delta":"{}"}),
            json!({"type":"response.output_item.done","output_index":0,"item":{"id":"fc1","type":kind,"call_id":id,"name":"one","arguments":"{}"}}),
            json!({"type":"response.output_text.delta","output_index":1,"content_index":0,"delta":"after"}),
            json!({"type":"response.completed","response":{"output":[]}}),
        ];
        if kind == "custom_tool_call" {
            events.remove(1);
            events[1]["item"]["input"] = "opaque".into();
        }
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
}

/// 非终态帧可以携带名为 error 的扩展；只能按源协议判定终止，否则后续正文会被整个丢弃。
#[test]
fn native_termination_ignores_unrelated_error_fields() {
    let corpus: Value = serde_json::from_str(include_str!("fixtures/protocols.json")).unwrap();
    for (source, target, extension) in [
        (
            "anthropic_messages",
            "openai_chat",
            json!({"type":"ping","error":null}),
        ),
        (
            "openai_responses",
            "openai_chat",
            json!({"type":"response.in_progress","error":{"tag":"extension"}}),
        ),
        (
            "openai_chat",
            "anthropic_messages",
            json!({"choices":[],"error":null}),
        ),
    ] {
        let mut w = Worker::new(&["convert", "--from", target, "--to", source]);
        open_stream(&mut w, "text/event-stream");
        let mut bytes = String::new();
        for event in std::iter::once(&extension).chain(corpus[source]["events"].as_array().unwrap())
        {
            let r = stream_frame(&mut w, "data", &format!("data: {event}\n\n"));
            bytes.push_str(r["data"].as_str().unwrap());
        }
        if source == "openai_chat" {
            bytes.push_str(
                stream_frame(&mut w, "data", "data: [DONE]\n\n")["data"]
                    .as_str()
                    .unwrap(),
            );
        }
        bytes.push_str(stream_frame(&mut w, "close", "")["data"].as_str().unwrap());
        assert!(
            bytes.contains("hello λ") && !bytes.contains("\"error\""),
            "{source}: {bytes}"
        );
        assert!(bytes.contains(if target == "openai_chat" {
            "data: [DONE]"
        } else {
            "event: message_stop"
        }));
    }
}

/// 空对象不能靠解码器的默认角色变成成功；工具调用和纯 thinking 回复不需要可见文本。
#[test]
fn buffered_and_streamed_json_require_an_assistant_message_not_a_shell() {
    for (source, client, msg, valid) in [
        (
            "openai_chat",
            "anthropic_messages",
            json!({"choices":[{"message":{}}]}),
            false,
        ),
        (
            "openai_chat",
            "anthropic_messages",
            json!({"choices":[{"message":{"role":"assistant"}}]}),
            false,
        ),
        (
            "openai_chat",
            "anthropic_messages",
            json!({"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"t","type":"function","function":{"name":"lookup","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}),
            true,
        ),
        (
            "anthropic_messages",
            "openai_chat",
            json!({"type":"message","role":"assistant","id":"i","model":"m","content":[{"type":"thinking","thinking":"checking","signature":"s"}]}),
            true,
        ),
    ] {
        let mut worker = Worker::new(&["convert", "--from", client, "--to", source]);
        let reply = worker.frame(response(&Value::Null, &msg.to_string()));
        if valid {
            assert!(reply["response"]["body"].is_string(), "{reply}");
        } else {
            assert_eq!(reply["fail"]["response"]["status"], 502);
        }
        open_stream(&mut worker, "application/json");
        stream_frame(&mut worker, "data", &msg.to_string());
        let tail = stream_frame(&mut worker, "close", "");
        let body: Value = serde_json::from_str(tail["data"].as_str().unwrap()).unwrap();
        assert_eq!(body.get("error").is_none(), valid, "{body}");
    }
}

/// 配置加载区分删除和写入；即使头名大小写混用也不能让固定密钥跨渠道继承。
#[test]
fn rewrite_refuses_secret_assignments_but_can_remove_them() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rewrite.toml");
    for header in ["Authorization", "COOKIE", "X-Custom-Token"] {
        std::fs::write(
            &path,
            format!("set_headers={{'{header}'='private-fixture'}}"),
        )
        .unwrap();
        let result = Command::new(env!("CARGO_BIN_EXE_aproxy-format"))
            .args(["rewrite", "--config", path.to_str().unwrap()])
            .output()
            .unwrap();
        let diagnostic = String::from_utf8(result.stderr).unwrap();
        assert!(
            !result.status.success()
                && result.stdout.is_empty()
                && diagnostic.contains("set_headers")
                && !diagnostic.contains("private-fixture")
        );
    }
    std::fs::write(&path, "remove_headers=['Authorization','Cookie']").unwrap();
    let mut worker = Worker::new(&["rewrite", "--config", path.to_str().unwrap()]);
    let r = worker.frame(request("https://a.test/", "/", json!({})));
    assert_eq!(r["request"]["headers"], json!({}));
}

/// 从生成物到有限上游样例完整试跑。路径与正文可以携带私有值，但所有可见步骤只能给出结构投影。
#[test]
fn tools_try_runs_the_sample_chain_without_exposing_the_sample() {
    let d = tempfile::tempdir().unwrap();
    let binary = env!("CARGO_BIN_EXE_aproxy-format");
    let init = Command::new(binary)
        .args(["init", "adaptive-relay", "--output"])
        .arg(d.path())
        .output()
        .unwrap();
    assert!(init.status.success(), "{init:?}");
    let config = d.path().join("config.toml");
    let sample = d.path().join("request.json");
    let check = Command::new(binary)
        .arg("check")
        .arg(&config)
        .output()
        .unwrap();
    assert!(check.status.success(), "{check:?}");
    let token = "private-fixture-392";
    let corpus: Value = serde_json::from_str(include_str!("fixtures/protocols.json")).unwrap();
    let request = json!({"path":format!("/v1/messages?auth={token}"),"headers":{"authorization":token},"body":{"model":"m","messages":[{"role":"user","content":token}],"max_tokens":8},"attempts":[{"status":503,"headers":{"retry-after":"0"}},{"status":200,"headers":{"content-type":"application/json"},"body":corpus["openai_chat"]["response"]}]});
    std::fs::write(&sample, request.to_string()).unwrap();
    let result = Command::new(binary)
        .arg("try")
        .arg(&config)
        .arg("--request")
        .arg(&sample)
        .env("RELAY_KEY_A", token)
        .env("RELAY_KEY_B", token)
        .output()
        .unwrap();
    assert!(result.status.success(), "{result:?}");
    let text = String::from_utf8(result.stdout).unwrap();
    assert!(
        !text.contains(token) && !text.contains("relay.example.com") && !text.contains("\"state\"")
    );
    assert!(!String::from_utf8_lossy(&result.stderr).contains(token));
    let rows: Vec<Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let steps: Vec<_> = rows
        .iter()
        .map(|r| (r["stage"].as_str().unwrap(), r["role"].as_str().unwrap()))
        .collect();
    for step in [
        ("hello", "adaptive"),
        ("request", "adaptive"),
        ("request", "convert"),
        ("request", "keys"),
        ("decide", "adaptive"),
        ("retry", "keys"),
        ("stream:open", "convert"),
        ("stream:data", "convert"),
        ("stream:close", "convert"),
        ("notify", "adaptive"),
    ] {
        assert!(steps.contains(&step), "{step:?} {text}");
    }
    assert!(
        rows.iter()
            .any(|r| r["stage"] == "result" && r["output"]["status"] == 200)
    );
}
/// 默认连外部进程都不启动。显式授权后同一份无害测试命令留下标记，但它不懂握手，不能取得后续帧。
#[test]
fn dry_run_external_execution_requires_opt_in_and_discloses_the_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let fixture = root.join(format!("sample-peer{}", std::env::consts::EXE_SUFFIX));
    let binary = env!("CARGO_BIN_EXE_aproxy-format");
    std::fs::copy(binary, &fixture).unwrap();
    let marker = root.join("observed");
    let file = root.join("config.toml");
    let sample = root.join("s.json");
    std::fs::write(&sample, r#"{"path":"/","body":"private-fixture"}"#).unwrap();
    std::fs::write(&file,format!("base_url='https://relay.test'\n[[format]]\nname='peer'\ncommand={}\nargs=['init','key-rotation','--output',{}]\nstages=['request']\nrequired=false",json!(fixture),json!(marker))).unwrap();
    for allow in [false, true] {
        let mut cmd = Command::new(binary);
        cmd.arg("try").arg(&file).arg("--request").arg(&sample);
        if allow {
            cmd.arg("--allow-external");
        }
        let out = cmd.output().unwrap();
        assert!(out.status.success(), "{out:?}");
        assert_eq!(marker.join("config.toml").exists(), allow);
        let err = String::from_utf8(out.stderr).unwrap();
        assert_eq!(err.contains("外部程序会收到样例和环境变量"), allow);
        let text = String::from_utf8(out.stdout).unwrap();
        assert!(text.contains("skipped"));
        assert!(!text.contains("private-fixture") && !err.contains("private-fixture"));
    }
}

/// 帮助模式不消费标准输入，不与协议 hello 混用，并指出独立转换与链中角色的区别。
#[test]
fn translate_help_distinguishes_single_requests_from_chain_roles() {
    let run = Command::new(env!("CARGO_BIN_EXE_aproxy-format"))
        .args(["translate", "--help"])
        .output()
        .unwrap();
    assert!(run.status.success() && run.stderr.is_empty());
    let help = String::from_utf8(run.stdout).unwrap();
    for term in [
        "--from",
        "--to",
        "openai_responses",
        "标准输入",
        "convert",
        "check",
        "try",
    ] {
        assert!(help.contains(term));
    }
}

/// 三种离线入口共用输入边界；合法的 TOML/JSON 用空白填充，不能依靠解析恰巧失败来通过测试。
#[test]
fn offline_files_are_bounded_before_parsing() {
    let dir = tempfile::tempdir().unwrap();
    let big = dir.path().join("big");
    let proxy = dir.path().join("proxy.toml");
    let sample = dir.path().join("sample.json");
    let padding = " ".repeat(8 * 1024 * 1024 + 1);
    std::fs::write(&big, &padding).unwrap();
    std::fs::write(&sample, format!("{padding}{{\"path\":\"/\"}}")).unwrap();
    std::fs::write(&proxy, "base_url='https://relay.test'\n[[format]]\nname='rewrite'\ncommand='aproxy-format'\nargs=['rewrite','--config','big']\nstages=['request']").unwrap();
    let p = proxy.to_str().unwrap();
    let b = big.to_str().unwrap();
    for args in [
        vec!["check", b],
        vec!["check", "rewrite", "--config", b],
        vec!["check", p],
        vec!["try", p, "--request", sample.to_str().unwrap()],
    ] {
        let out = Command::new(env!("CARGO_BIN_EXE_aproxy-format"))
            .args(&args)
            .output()
            .unwrap();
        assert!(!out.status.success(), "{args:?}: {out:?}");
    }
}
