//! 真正通过启动命令协商协议，区分子进程不可用与静态不兼容。每个实例都有自己的 home。
mod common;
use common::{TestHome, format_echo_path, free_port};
use serde_json::Value;

/// 用裸名从 home/bin 找到本地测试程序。真实命令、注册记录与 IPC 应答共用同一个临时 home。
fn start(mode: &str, required: bool) -> (TestHome, std::process::Output) {
    let home = TestHome::new();
    let port = free_port();
    home.stop_on_drop(port);
    let bin = home.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::copy(
        format_echo_path(),
        bin.join(format!("local-format{}", std::env::consts::EXE_SUFFIX)),
    )
    .unwrap();
    let cfg = home.path().join("instance.toml");
    std::fs::write(&cfg, format!("base_url='http://127.0.0.1:{port}'\nlisten_addr='127.0.0.1:{port}'\n[[format]]\nname='test-layer'\ncommand='local-format'\nstages=['request']\nargs=['{mode}']\nrequired={required}\nenv={{ ECHO_SECRET='fixture-credential' }}\n")).unwrap();
    let out = home
        .cmd()
        .args(["start", "--json", "--config"])
        .arg(cfg)
        .output()
        .unwrap();
    (home, out)
}
/// 绑定成功不能代表已就绪；透过启动命令验证子进程的错误不被折叠成 start_failed。
fn failure(mode: &str, required: bool, code: &str, exit: i32) -> Value {
    let (_home, out) = start(mode, required);
    assert_eq!(out.status.code(), Some(exit), "{out:?}");
    let json: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["error"]["code"], code);
    assert_eq!(json["error"]["details"]["format"], "test-layer");
    json["error"]["details"].clone()
}
/// 必需程序握手不答是运行期不可用，而非配置错误。
#[test]
fn required_program_without_hello_reply_fails_start() {
    assert_eq!(
        failure("no-hello", true, "format_handshake_failed", 1)["kind"],
        "timeout"
    );
}
/// 不可用的可选程序允许服务上线，但在冷却前不再拉起。
#[test]
fn optional_program_without_hello_reply_starts_with_an_open_breaker() {
    let (_home, out) = start("no-hello", false);
    assert!(out.status.success(), "{out:?}");
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    let inst = &doc["instance"];
    assert_eq!(inst["state"], "serving");
    let f = &inst["formats"][0];
    assert_eq!(f["breaker"]["state"], "open");
    assert_eq!(f["breaker"]["last_failure"]["details"]["kind"], "timeout");
    assert!(f.get("protocol").is_none());
}
/// 静态阶段不匹配必须指出声明中缺失的是哪个阶段。
#[test]
fn program_missing_a_declared_stage_fails_start() {
    assert_eq!(
        failure("missing-stage", true, "format_stage_unsupported", 3)["stages"],
        serde_json::json!(["request"])
    );
}
/// 可选条目也不能以错误协议或无法识别的请求体粒度启动。
#[test]
fn optional_program_with_an_incompatible_protocol_fails_start() {
    assert_eq!(
        failure("bad-protocol", false, "format_handshake_failed", 3)["kind"],
        "protocol"
    );
    assert_eq!(
        failure("bad-body-need", false, "format_handshake_failed", 3)["kind"],
        "invalid_output"
    );
}
/// 通知永远拿不到正文；声明需要正文是静态错误，而不是暂时不可用。
#[test]
fn notify_requesting_a_full_body_fails_start() {
    for required in [true, false] {
        assert_eq!(
            failure("bad-notify-need", required, "format_handshake_failed", 3)["kind"],
            "invalid_output"
        );
    }
}
/// 子进程退出与协议不兼容的退出码不同。
#[test]
fn required_program_exiting_during_hello_fails_start() {
    assert_eq!(
        failure("exit-hello", true, "format_handshake_failed", 1)["kind"],
        "died"
    );
}

/// 从实例日志而不是测试进程捕获 stderr，逐行检查名字、级别和密钥替换。
#[test]
fn stderr_lines_reach_the_instance_log_with_secrets_replaced() {
    let (_home, out) = start("stderr", true);
    assert!(out.status.success(), "{out:?}");
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    let path = doc["log_path"].as_str().unwrap();
    let log = std::fs::read_to_string(path).unwrap();
    for expected in ["worker diagnostic", "value="] {
        assert!(
            log.lines().any(|line| line.contains("WARN")
                && line.contains(&format!("format[test-layer] {expected}"))),
            "{log}"
        );
    }
    assert!(!log.contains("fixture-credential"));
}

/// 配置一个本地持久进程并记录每帧；不同程序用不同的记录文件，测试不依赖日志调度。
fn layer(home: &TestHome, name: &str, stages: &str, args: &[&str]) -> String {
    let quoted = |p: std::path::PathBuf| serde_json::to_string(p.to_str().unwrap()).unwrap();
    format!(
        "\n[[format]]\nname='{name}'\ncommand={}\nprocess='persistent'\nworker_idle_secs=0\nstages=[{stages}]\nargs={}\nenv={{ ECHO_RECORD={} }}\n",
        quoted(format_echo_path()),
        serde_json::to_string(args).unwrap(),
        quoted(home.path().join(name))
    )
}

/// 实例仅在临时 home 内注册，裸 HTTP 请求走环回地址，不使用环境代理。
fn launch(home: &TestHome, upstream: u16, config: &str) -> (u16, std::process::Output) {
    launch_with_backoff(home, upstream, config, 0)
}
/// 指定实例的真实退避上限，让程序检查 fallback 后用零等待票覆盖，不拖慢测试。
fn launch_with_backoff(
    home: &TestHome,
    upstream: u16,
    config: &str,
    cap: u64,
) -> (u16, std::process::Output) {
    let port = free_port();
    home.stop_on_drop(port);
    let cfg = home.path().join("chain.toml");
    std::fs::write(&cfg,format!("base_url='http://127.0.0.1:{upstream}'\nlisten_addr='127.0.0.1:{port}'\nretry.max_backoff_secs={cap}\n{config}")).unwrap();
    let out = home
        .cmd()
        .env("NO_PROXY", "127.0.0.1,localhost")
        .args(["start", "--json", "--config"])
        .arg(cfg)
        .output()
        .unwrap();
    (port, out)
}

/// 断言启动成功，返回带客户端超时的本地请求入口，不让无限重试拖住整个测试。
fn running(home: &TestHome, upstream: u16, config: &str) -> (reqwest::Client, String) {
    let (port, out) = launch(home, upstream, config);
    assert!(out.status.success(), "{out:?}");
    (
        reqwest::Client::builder()
            .no_proxy()
            .timeout(std::time::Duration::from_secs(12))
            .build()
            .unwrap(),
        format!("http://127.0.0.1:{port}"),
    )
}

/// 测试上游按调用次数依次给出状态码，最后一个状态码持续生效。
async fn upstream(
    statuses: &'static [u16],
) -> (u16, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let app = axum::Router::new().fallback(move || {
        let i = count
            .fetch_add(1, Ordering::Relaxed)
            .min(statuses.len() - 1);
        async move {
            (
                axum::http::StatusCode::from_u16(statuses[i]).unwrap(),
                "upstream bytes",
            )
        }
    });
    let port = free_port();
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
        .await
        .unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (port, calls)
}

/// 逐帧重读测试程序落下的事实，不用推算调用顺序或进程数。
fn frames(home: &TestHome, name: &str) -> Vec<Value> {
    std::fs::read_to_string(home.path().join(name))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// 没写 name 时也要验证命令的文件名主干，并在启动子进程之前提示写明 name。
#[test]
fn oversized_derived_format_name_fails_start_as_invalid_config() {
    let home = TestHome::new();
    let command = format!("{}.exe", "x".repeat(65));
    let config = format!("[[format]]\ncommand='{command}'\nstages=['request']\n");
    let (_, out) = launch(&home, free_port(), &config);
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["error"]["code"], "config_invalid");
    let diagnostic = &doc["error"]["details"]["diagnostics"][0];
    assert_eq!(diagnostic["code"], "config_invalid_value");
    assert!(diagnostic["message"].as_str().unwrap().contains("set name"));
}

/// 不合法的重试段应在任何一个子进程被拉起前就返回配置诊断。
#[test]
fn retry_segment_violation_fails_start() {
    let home = TestHome::new();
    let cfg = layer(&home, "early", "'request','retry'", &["echo"])
        + &layer(&home, "late", "'request'", &["echo"]);
    let (_, out) = launch(&home, free_port(), &cfg);
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        doc["error"]["details"]["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["code"] == "config_conflict")
    );
    assert!(!home.path().join("early").exists());
}

/// 进程级失败恢复时重跑同一层、同一 stage，且只有成功的那次到达上游。
async fn recover_once(mode: &str) {
    let home = TestHome::new();
    let (up, calls) = upstream(&[200]).await;
    let stamp = home.path().join("once");
    let cfg = layer(&home, "work", "'request'", &[mode, stamp.to_str().unwrap()]);
    let (client, url) = running(&home, up, &cfg);
    assert_eq!(
        client.post(url).send().await.unwrap().text().await.unwrap(),
        "upstream bytes"
    );
    let f = frames(&home, "work");
    assert_eq!(
        f.iter()
            .map(|m| m["stage"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["hello", "request", "hello", "request"]
    );
    assert_eq!(f[1]["request_id"], f[3]["request_id"]);
    assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 1);
}

/// 错序的旧回信不能交给下一个请求，重新握手后才重新发送。
#[tokio::test]
async fn bad_seq_drops_the_worker_and_the_request_is_retried() {
    recover_once("bad-seq-once").await;
}

/// 进程崩溃不是业务拒绝；它恢复后客户端得到上游响应。
#[tokio::test]
async fn crash_mid_request_is_retried_and_succeeds() {
    recover_once("crash-once").await;
}

/// 上游失败只重跑配置的后段，后段的输入仍是前段的补丁结果。
#[tokio::test]
async fn program_before_the_retry_segment_is_not_called_again_on_retries() {
    let home = TestHome::new();
    let (up, _) = upstream(&[503, 200]).await;
    let cfg = layer(
        &home,
        "prefix",
        "'request'",
        &["reply", r#"{"request":{"body":"prefix"}}"#],
    ) + &layer(
        &home,
        "suffix",
        "'request','retry'",
        &["reply", r#"{"request":{"body":"suffix"}}"#],
    );
    let (client, url) = running(&home, up, &cfg);
    assert_eq!(client.post(url).send().await.unwrap().status(), 200);
    assert_eq!(frames(&home, "prefix").len(), 2);
    let f = frames(&home, "suffix");
    assert_eq!(f.len(), 3);
    assert_eq!(f[2]["stage"], "retry");
    assert_eq!(f[2]["retry"]["last_failure"]["details"]["status"], 503);
    assert_eq!(f[2]["request"]["body"], "prefix");
}

/// 两种主动拒绝都没有上游尝试；程序给的状态、头和体保持原样。
#[tokio::test]
async fn fail_without_retry_returns_the_given_response_or_422() {
    let (up, calls) = upstream(&[200]).await;
    for custom in [false, true] {
        let home = TestHome::new();
        let mut f = serde_json::json!({"retry":false,"reason":"fixture","code":"ruleset"});
        if custom {
            f["response"] =
                serde_json::json!({"status":409,"headers":{"x-fixture":"present"},"body":"no"});
        }
        let cfg = layer(
            &home,
            "gate",
            "'request'",
            &["reply", &serde_json::json!({"fail":f}).to_string()],
        );
        let (client, url) = running(&home, up, &cfg);
        let res = client.post(url).send().await.unwrap();
        assert_eq!(res.headers()["x-aproxy-error"], "format_rejected");
        assert!(res.headers().contains_key("x-aproxy-request-id"));
        if custom {
            assert_eq!(res.status(), 409);
            assert_eq!(res.headers()["x-fixture"], "present");
            assert_eq!(res.text().await.unwrap(), "no");
        } else {
            assert_eq!(res.status(), 422);
            assert_eq!(
                res.json::<Value>().await.unwrap()["error"]["aproxy"]["details"]["fail_code"],
                "ruleset"
            );
        }
    }
    assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 0);
}

/// 程序的终态体只走数据面；记下的是实际交付量与已脱敏的失败，不虚构上游响应。
#[tokio::test]
async fn program_terminal_bytes_stay_private_to_the_client() {
    use aproxy::daemon::{IpcError, ReadOp, ipc_read_in};
    let (up, calls) = upstream(&[200]).await;
    let secret = "fixture-terminal-token";
    let reason = format!("{secret}{}", "界".repeat(520));
    let safe = format!("<secret>{}", "界".repeat(504));
    for custom in [true, false] {
        let home = TestHome::new();
        let mut fail = serde_json::json!({"retry":false,"reason":reason,"code":"invalid/private"});
        if custom {
            fail["response"] = serde_json::json!({"status":409,"body":secret});
        }
        let cfg = layer(
            &home,
            "gate",
            "'request'",
            &["reply", &serde_json::json!({"fail":fail}).to_string()],
        )
        .replace("env={", &format!("env={{ TOKEN='{secret}',"));
        let (port, out) = launch(&home, up, &cfg);
        assert!(out.status.success(), "{out:?}");
        let startup: Value = serde_json::from_slice(&out.stdout).unwrap();
        let res = reqwest::Client::builder()
            .no_proxy()
            .timeout(std::time::Duration::from_secs(6))
            .build()
            .unwrap()
            .post(format!("http://127.0.0.1:{port}/"))
            .send()
            .await
            .unwrap();
        let id = res.headers()["x-aproxy-request-id"]
            .to_str()
            .unwrap()
            .to_owned();
        assert_eq!(res.status().as_u16(), if custom { 409 } else { 422 });
        if custom {
            assert_eq!(res.bytes().await.unwrap().as_ref(), secret.as_bytes());
        } else {
            let error = res.json::<Value>().await.unwrap();
            assert!(error["error"]["message"].as_str().unwrap().ends_with(&safe));
            assert!(!error.to_string().contains(secret));
        }
        let run = home.path().join("run");
        let port = port.to_string();
        let doc = ipc_read_in(
            &run,
            &port,
            &ReadOp::Request {
                request_id: id.clone(),
            },
        )
        .await
        .unwrap();
        assert_eq!(doc["record"]["code"], "format_rejected");
        assert_eq!(
            doc["record"]["client_status"],
            if custom { 409 } else { 422 }
        );
        assert_eq!(
            doc["record"]["response_bytes"],
            if custom { secret.len() } else { 0 }
        );
        assert_eq!(doc["record"]["tries"][0]["code"], "format_rejected");
        assert!(doc["record"]["tries"][0]["upstream_status"].is_null());
        assert_eq!(doc["tries"][0].get("response"), Some(&Value::Null));
        assert!(
            doc["tries"][0]["error"]["message"]
                .as_str()
                .unwrap()
                .ends_with(&safe)
        );
        assert!(!doc.to_string().contains(secret));
        let error = ipc_read_in(
            &run,
            &port,
            &ReadOp::Body {
                request_id: id,
                part: aproxy::history::Part::Response,
                attempt: Some(1),
                offset: None,
                limit: None,
            },
        )
        .await
        .unwrap_err();
        assert!(
            matches!(error,IpcError::Remote {code,details,..} if code=="not_found" && details["reason"]=="no_such_part")
        );
        let log = std::fs::read_to_string(startup["log_path"].as_str().unwrap()).unwrap();
        assert!(!log.contains(secret));
        assert!(log.contains(&safe));
        assert_eq!(log.matches("invalid format fail code ignored").count(), 1);
        assert!(!log.contains("invalid/private"));
        assert!(!doc.to_string().contains("invalid/private"));
    }
    assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 0);
}
/// 并列一个无效键不影响合法覆盖；下一个请求仍然遵守实例规则。
#[tokio::test]
async fn overrides_apply_to_one_request_and_unknown_keys_are_ignored() {
    let home = TestHome::new();
    let (up, calls) = upstream(&[503, 503, 200]).await;
    let cfg = layer(
        &home,
        "limit",
        "'request'",
        &[
            "reply-on-path",
            r#"{"overrides":{"retry":{"max_attempts":1,"unknown":3}}}"#,
        ],
    );
    let (client, url) = running(&home, up, &cfg);
    let first = client.post(format!("{url}/limited")).send().await.unwrap();
    assert_eq!(first.status(), 503);
    assert_eq!(first.headers()["x-aproxy-error"], "retry_exhausted");
    assert_eq!(first.text().await.unwrap(), "upstream bytes");
    assert_eq!(
        client
            .post(format!("{url}/other"))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 3);
}

/// 第一层能在静默请求上开启保活；另一层慢到已提交后才返回，不能把流改用另一种心跳。
#[tokio::test]
async fn keepalive_overrides_observe_the_commit_between_layers() {
    let home = TestHome::new();
    let (up, _) = upstream(&[200]).await;
    let cfg = String::from("[keepalive]\ninterval_secs=0\n")
        + &layer(
            &home,
            "arm",
            "'request'",
            &[
                "reply",
                r#"{"overrides":{"keepalive":{"trigger":"always","interval_secs":1,"heartbeat":": first\n\n"}}}"#,
            ],
        )
        + &layer(&home, "slow", "'request'", &["sleep", "1400"])
        + &layer(
            &home,
            "late",
            "'request'",
            &[
                "reply",
                r#"{"overrides":{"keepalive":{"heartbeat":": late\n\n"}}}"#,
            ],
        )
        + &layer(&home, "wait", "'request'", &["sleep", "1200"]);
    let (client, url) = running(&home, up, &cfg);
    let res = client.post(url).send().await.unwrap();
    assert_eq!(res.headers()["content-type"], "text/event-stream");
    let body = res.text().await.unwrap();
    assert!(body.matches(": first\n\n").count() >= 2, "{body}");
    assert!(!body.contains(": late"));
}

/// 两个程序逐层追加自己的名字。核对收到的字节而非日志先后，证明真实的数据依赖。
#[tokio::test]
async fn chain_runs_request_in_order_and_response_and_heartbeat_in_reverse() {
    let home = TestHome::new();
    let up = free_port();
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, up))
        .await
        .unwrap();
    let release = std::sync::Arc::new(tokio::sync::Semaphore::new(0));
    let gate = release.clone();
    let app = axum::Router::new().fallback(move |body: String| {
        let gate = gate.clone();
        async move {
            assert_eq!(body, "qAB");
            gate.acquire().await.unwrap().forget();
            (
                [("content-type", "text/event-stream")],
                "data: response\n\n",
            )
        }
    });
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let stages = "'request','response','heartbeat'";
    let config = String::from("[keepalive]\ninterval_secs=1\n")
        + &layer(&home, "outer", stages, &["append", "A"])
        + &layer(&home, "inner", stages, &["append", "B"]);
    let (client, url) = running(&home, up, &config);
    let mut res = client
        .post(format!("{url}/stream"))
        .header("accept", "text/event-stream")
        .body("q")
        .send()
        .await
        .unwrap();
    let mut bytes = Vec::new();
    let beat = b": keepalive\n\n: B\n\n: A\n\n";
    while !bytes.windows(beat.len()).any(|w| w == beat) {
        bytes.extend_from_slice(
            &res.chunk()
                .await
                .unwrap()
                .expect("stream ended before a converted beat"),
        );
    }
    release.add_permits(1);
    bytes.extend_from_slice(&res.bytes().await.unwrap());
    let text = String::from_utf8(bytes).unwrap();
    assert!(text.contains(": keepalive\n\n: B\n\n: A\n\n"), "{text:?}");
    assert!(text.ends_with("data: response\n\nBA"), "{text:?}");
    for (name, request, heartbeat, response) in [
        (
            "outer",
            "q",
            ": keepalive\n\n: B\n\n",
            "data: response\n\nB",
        ),
        ("inner", "qA", ": keepalive\n\n", "data: response\n\n"),
    ] {
        let f = frames(&home, name);
        let at = |stage: &str| f.iter().find(|m| m["stage"] == stage).unwrap();
        assert_eq!(at("request")["request"]["body"], request);
        assert_eq!(at("heartbeat")["data"], heartbeat);
        assert_eq!(
            at("heartbeat")["client"],
            serde_json::json!({"path":"/stream","stream":true})
        );
        assert_eq!(at("response")["response"]["body"], response);
        assert_eq!(at("heartbeat")["request_id"], at("response")["request_id"]);
        assert!(at("response").get("state").is_none());
    }
}

/// 成功握手后，配置顺序的身份、能力与熔断状态一起出现在快照里。
#[test]
fn snapshot_lists_formats_with_their_breaker() {
    let (_home, out) = start("echo", true);
    assert!(out.status.success(), "{out:?}");
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        doc["instance"]["formats"][0],
        serde_json::json!({"name":"test-layer","stages":["request"],"required":true,"process":"spawn","protocol":1,"version":"1.0","breaker":{"state":"closed","failures":0}})
    );
}

/// 从独立实例的控制端点取得明细，区分顶层报告与每次尝试的报告，并核对未知动作的标签。
#[tokio::test]
async fn reports_land_in_the_request_record() {
    use aproxy::daemon::{ReadOp, ipc_read_in};
    let home = TestHome::new();
    let (up, _) = upstream(&[503, 200]).await;
    let mut actions = vec![serde_json::json!({"type":"other","label":"seen fictional-token"}); 35];
    actions[1]["label"] = format!("{}fictional-token", "界".repeat(249)).into();
    actions[2]["label"] = format!("{}密匙甲", "x".repeat(252)).into();
    let report = serde_json::json!({"report":{"summary":format!("fictional-token {}","界".repeat(520)),"actions":actions}});
    let cfg = layer(
        &home,
        "reporter",
        "'request','retry','response'",
        &["reply", &report.to_string()],
    )
    .replace("env={", "env={ KEY='密匙甲', TOKEN='fictional-token',");
    let (client, url) = running(&home, up, &cfg);
    let res = client.post(&url).send().await.unwrap();
    assert_eq!(res.status(), 200);
    res.text().await.unwrap();
    let f = frames(&home, "reporter");
    let id = f[1]["request_id"].as_str().unwrap();
    let port = url.rsplit(':').next().unwrap();
    let detail = ipc_read_in(
        &home.path().join("run"),
        port,
        &ReadOp::Request {
            request_id: id.into(),
        },
    )
    .await
    .unwrap();
    let r = &detail["record"];
    assert_eq!(r["reports"].as_array().unwrap().len(), 2);
    assert_eq!(r["tries"][0]["reports"], serde_json::json!([]));
    assert_eq!(r["tries"][1]["reports"].as_array().unwrap().len(), 1);
    for (report, stage) in [
        (&r["reports"][0], "request"),
        (&r["reports"][1], "response"),
        (&r["tries"][1]["reports"][0], "retry"),
    ] {
        assert_eq!(report["stage"], stage);
        assert_eq!(report["format"], "reporter");
        assert_eq!(report["summary"].as_str().unwrap().chars().count(), 512);
        assert_eq!(report["actions"].as_array().unwrap().len(), 32);
        assert_eq!(
            report["actions"][2]["label"],
            format!("{}<sec", "x".repeat(252))
        );
        let clipped = report["actions"][1]["label"].as_str().unwrap();
        assert!(!clipped.contains("fiction"));
        assert!(clipped.chars().count() <= 256);
        assert_eq!(
            report["actions"][0],
            serde_json::json!({"type":"other","label":"seen <secret>"})
        );
    }
    assert!(!r.to_string().contains("fictional-token"));
}

/// 通过临时 home 的快照读熔断状态，而不是依据一次 HTTP 响应猜测。
async fn breaker(home: &TestHome, url: &str) -> Value {
    let port = url.rsplit(':').next().unwrap();
    aproxy::daemon::ipc_ping_in(&home.path().join("run"), port)
        .await
        .unwrap()
        .formats[0]["breaker"]
        .clone()
}

/// 第五条坏回信后停止派发；熔断期间的下一次请求不再启动进程，也不会访问上游。
#[tokio::test]
async fn breaker_opens_after_five_failures_and_answers_503() {
    let home = TestHome::new();
    let (up, calls) = upstream(&[200]).await;
    let cfg = layer(&home, "broken", "'request'", &["bad-seq"]);
    let (c, url) = running(&home, up, &cfg);
    for _ in 0..2 {
        let r = c.post(&url).send().await.unwrap();
        assert_eq!(r.status(), 503);
        assert_eq!(r.headers()["x-aproxy-error"], "format_unavailable");
    }
    let ping =
        aproxy::daemon::ipc_ping_in(&home.path().join("run"), url.rsplit(':').next().unwrap())
            .await
            .unwrap();
    let log = std::fs::read_to_string(ping.instance.log_path.unwrap()).unwrap();
    assert_eq!(
        log.lines()
            .filter(|line| line.contains("format request unavailable")
                && line.contains("format_unavailable"))
            .count(),
        2,
        "{log}"
    );
    let f = frames(&home, "broken");
    assert_eq!(f.len(), 10);
    assert_eq!(f.iter().filter(|m| m["stage"] == "request").count(), 5);
    let b = breaker(&home, &url).await;
    assert_eq!(b["state"], "open");
    assert_eq!(b["failures"], 5);
    assert_eq!(b["last_failure"]["details"]["kind"], "protocol");
    assert!(b["probe_at"].as_u64() > b["opened_at"].as_u64());
    assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 0);
}

/// 冷却结束后的一条新消息重新握手；有效回信清零，之后的请求可复用试探进程。
#[tokio::test]
async fn breaker_closes_after_a_successful_probe() {
    let home = TestHome::new();
    let (up, _) = upstream(&[200]).await;
    let stamp = home.path().join("healthy");
    let cfg = layer(
        &home,
        "flaky",
        "'request'",
        &["bad-until-file", stamp.to_str().unwrap()],
    ) + "breaker_failures=1\nbreaker_cooldown_secs=1\n";
    let (c, url) = running(&home, up, &cfg);
    let (snapshot, mut events) =
        aproxy::daemon::ipc_subscribe_in(&home.path().join("run"), url.rsplit(':').next().unwrap())
            .await
            .unwrap();
    assert_eq!(
        snapshot["snapshot"]["formats"][0]["breaker"]["state"],
        "closed"
    );
    assert_eq!(c.post(&url).send().await.unwrap().status(), 503);
    assert_eq!(breaker(&home, &url).await["state"], "open");
    std::fs::write(stamp, b"ok").unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(1050)).await;
    for _ in 0..2 {
        assert_eq!(c.post(&url).send().await.unwrap().status(), 200);
    }
    assert_eq!(
        frames(&home, "flaky")
            .iter()
            .filter(|m| m["stage"] == "hello")
            .count(),
        2
    );
    let b = breaker(&home, &url).await;
    assert_eq!(b["state"], "closed");
    assert_eq!(b["failures"], 0);
    assert!(b.get("opened_at").is_none());
    // 此时三个转变都已发生；读事件流而不是从随后的一次 ping 推测中间状态。
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let mut last = String::from("closed");
        for expected in ["open", "half_open", "closed"] {
            loop {
                let event = events.next().await.unwrap().unwrap();
                assert_ne!(event["event"], "lagged");
                if event["event"] == "state" {
                    let state = event["data"]["formats"][0]["breaker"]["state"]
                        .as_str()
                        .unwrap();
                    if state != last {
                        assert_eq!(state, expected);
                        last = state.to_owned();
                        break;
                    }
                }
            }
        }
    })
    .await
    .unwrap();
}

/// 上游连续接收请求后断开六次，超过程序的阈值；第七次仍能在同一程序会话内成功。
#[tokio::test]
async fn network_failures_do_not_trip_the_breaker() {
    use tokio::io::AsyncReadExt;
    let home = TestHome::new();
    let up = free_port();
    let socket = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, up))
        .await
        .unwrap();
    tokio::spawn(async move {
        for _ in 0..6 {
            let (mut stream, _) = socket.accept().await.unwrap();
            let mut buf = [0u8; 1024];
            assert!(stream.read(&mut buf).await.unwrap() > 0);
        }
        axum::serve(socket, axum::Router::new().fallback(|| async { "ok" }))
            .await
            .unwrap();
    });
    let cfg = layer(&home, "healthy", "'request','retry'", &["echo"]);
    let (c, url) = running(&home, up, &cfg);
    assert_eq!(
        c.post(&url).send().await.unwrap().text().await.unwrap(),
        "ok"
    );
    let b = breaker(&home, &url).await;
    assert_eq!(b["failures"], 0);
    assert_eq!(b["state"], "closed");
    assert!(b.get("last_failure").is_none());
    let f = frames(&home, "healthy");
    assert_eq!(f.iter().filter(|m| m["stage"] == "hello").count(), 1);
    assert!(f.iter().filter(|m| m["stage"] == "retry").count() >= 5);
}

/// 不配置程序时保留原来的上游重试和响应正文，快照不凭空出现格式程序。
#[tokio::test]
async fn instance_without_formats_is_unaffected() {
    let home = TestHome::new();
    let (up, calls) = upstream(&[503, 200]).await;
    let (c, url) = running(&home, up, "");
    assert_eq!(
        c.post(&url).send().await.unwrap().text().await.unwrap(),
        "upstream bytes"
    );
    assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 2);
    assert!(
        aproxy::daemon::ipc_ping_in(&home.path().join("run"), url.rsplit(':').next().unwrap())
            .await
            .unwrap()
            .formats
            .is_empty()
    );
}

/// 非法投递提示不抹掉响应补丁，日志只在请求阶段告警且不回显外部数据。
#[tokio::test]
async fn malformed_delivery_preserves_patches_and_only_warns_on_request() {
    let home = TestHome::new();
    let (up, calls) = upstream(&[200]).await;
    let cfg = layer(
        &home,
        "voter",
        "'request','response'",
        &[
            "reply",
            r#"{"delivery":{"untrusted-vote":42},"response":{"body":"patched"}}"#,
        ],
    );
    let (port, out) = launch(&home, up, &cfg);
    assert!(out.status.success(), "{out:?}");
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    let response = reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap()
        .get(format!("http://127.0.0.1:{port}/"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.text().await.unwrap(), "patched");
    assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 1);
    let log = std::fs::read_to_string(doc["log_path"].as_str().unwrap()).unwrap();
    assert_eq!(
        log.matches("invalid format delivery ignored").count(),
        1,
        "{log}"
    );
    assert!(!log.contains("untrusted-vote"));
}

/// 畸形展示信息不让请求重试、不吞补丁；两种告警都不复制程序提供的文本。
#[tokio::test]
async fn malformed_report_and_action_warn_without_discarding_data() {
    let home = TestHome::new();
    let (up, calls) = upstream(&[200]).await;
    let cfg = layer(
        &home,
        "first",
        "'request'",
        &["reply", r#"{"report":"untrusted-content"}"#],
    ) + &layer(
        &home,
        "last",
        "'response'",
        &[
            "reply",
            r#"{"response":{"body":"patch survived"},"report":{"summary":"ok","actions":[{"type":"key_select","key_id":{"untrusted-content":42}},{"type":"custom","label":"valid"}]}}"#,
        ],
    );
    let (port, out) = launch(&home, up, &cfg);
    assert!(out.status.success(), "{out:?}");
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    let response = reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap()
        .get(format!("http://127.0.0.1:{port}/"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.text().await.unwrap(), "patch survived");
    assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 1);
    let log = std::fs::read_to_string(doc["log_path"].as_str().unwrap()).unwrap();
    for warning in [
        "invalid format report ignored",
        "invalid format report actions ignored",
    ] {
        assert_eq!(log.matches(warning).count(), 1, "{log}");
    }
    assert!(!log.contains("untrusted-content"));
}

/// 静态超长与未写完就退出是不同的失败；后者允许可选程序以后试探。
#[test]
fn hello_line_limits_and_partial_eof_observe_the_startup_table() {
    for required in [true, false] {
        assert_eq!(
            failure("unfinished-hello", required, "format_handshake_failed", 3)["kind"],
            "invalid_output"
        );
    }
    assert_eq!(
        failure("partial-hello", true, "format_handshake_failed", 1)["kind"],
        "died"
    );
    let (_home, out) = start("partial-hello", false);
    assert!(out.status.success(), "{out:?}");
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    let b = &doc["instance"]["formats"][0]["breaker"];
    assert_eq!(b["state"], "open");
    assert_eq!(b["last_failure"]["details"]["kind"], "died");
}

/// 第一个进程退出、第二个不换行；重发仍使用响应阶段的预算，不影响原响应或上游调用数。
#[tokio::test]
async fn fresh_response_worker_cannot_exceed_the_reply_limit() {
    let home = TestHome::new();
    let (up, calls) = upstream(&[200]).await;
    std::fs::write(
        home.path().join("settings.json"),
        r#"{"defaults":{"max_response_mb":1}}"#,
    )
    .unwrap();
    let marker = home.path().join("first-worker");
    let cfg = layer(
        &home,
        "bounded",
        "'response'",
        &["crash-then-unfinished", marker.to_str().unwrap()],
    ) + "timeout_secs=3\n";
    let (client, url) = running(&home, up, &cfg);
    let response = client.get(&url).send().await.unwrap();
    assert_eq!(response.text().await.unwrap(), "upstream bytes");
    let b = breaker(&home, &url).await;
    assert_eq!(b["failures"], 2);
    assert_eq!(b["last_failure"]["details"]["kind"], "invalid_output");
    assert_eq!(
        frames(&home, "bounded")
            .iter()
            .filter(|f| f["stage"] == "response")
            .count(),
        2
    );
    assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 1);
}

/// 即使 stderr 最后一段输出之后再无新行，窗口也会写出丢弃数；边界密钥不能泄漏半截。
#[tokio::test]
async fn stderr_truncates_redacts_and_reports_suppressed_lines() {
    let home = TestHome::new();
    let (up, _) = upstream(&[200]).await;
    let cfg = layer(&home, "noisy", "'request'", &["stderr-burst"]).replace(
        "env={ ECHO_RECORD=",
        "env={ ECHO_SECRET='private-fixture', ECHO_RECORD=",
    );
    let (port, out) = launch(&home, up, &cfg);
    assert!(out.status.success(), "{out:?}");
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    let log_path = doc["log_path"].as_str().unwrap();
    reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(8))
        .build()
        .unwrap()
        .get(format!("http://127.0.0.1:{port}/"))
        .send()
        .await
        .unwrap();
    let log = tokio::time::timeout(std::time::Duration::from_secs(4), async {
        loop {
            let text = std::fs::read_to_string(log_path).unwrap();
            if text.contains("dropped 8 stderr lines") {
                break text;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert!(log.contains("<secret>…(truncated)"));
    assert!(!log.contains("private-f"));
    assert_eq!(log.matches("small-stderr-line").count(), 49);
}

/// 探测用的一次性进程会在 stdin 关闭后临退出时写最后一行，正常回收要读完它。
#[test]
fn normal_spawn_exit_drains_the_final_stderr_line() {
    let home = TestHome::new();
    let cfg = layer(&home, "drain", "'request'", &["stderr-on-eof"])
        .replace("process='persistent'", "process='spawn'");
    let (_, out) = launch(&home, free_port(), &cfg);
    assert!(out.status.success(), "{out:?}");
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    let log = std::fs::read_to_string(doc["log_path"].as_str().unwrap()).unwrap();
    assert!(
        log.contains("format[drain] last diagnostic after stdin EOF"),
        "{log}"
    );
}

/// 启动父进程不阻塞测试，只有专属 home 里的实例才可能出现在随后的 IPC 断言里。
fn background_start(
    home: &TestHome,
    port: u16,
    up: u16,
    config: &str,
) -> (std::process::Child, std::path::PathBuf) {
    home.stop_on_drop(port);
    let cfg = home.path().join("startup.toml");
    std::fs::write(
        &cfg,
        format!("base_url='http://127.0.0.1:{up}'\nlisten_addr='127.0.0.1:{port}'\n{config}"),
    )
    .unwrap();
    let child = home
        .cmd()
        .env("NO_PROXY", "127.0.0.1,localhost")
        .args(["start", "--json", "--config"])
        .arg(&cfg)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    (child, cfg)
}
/// 等待程序写下握手帧，不依赖创建进程到开始读 stdin 的时延。
async fn observed_hello(home: &TestHome, name: &str) {
    tokio::time::timeout(std::time::Duration::from_secs(6), async {
        loop {
            let file = std::fs::read_to_string(home.path().join(name)).unwrap_or_default();
            if file
                .lines()
                .filter_map(|l| serde_json::from_str::<Value>(l).ok())
                .any(|v| v["stage"] == "hello")
            {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

/// 两个 hello 都到达才释放同一道闸门；persistent 探测的 pid 继续用，spawn 探测确实读到了 EOF。
#[tokio::test]
async fn startup_probes_run_in_parallel_and_respect_worker_lifetimes() {
    let home = TestHome::new();
    let (up, _) = upstream(&[200]).await;
    let gate = home.path().join("release");
    let args = ["gate-hello", gate.to_str().unwrap()];
    let mut cfg = String::new();
    for (name, process) in [("persist", "persistent"), ("once", "spawn")] {
        let obs = serde_json::to_string(&home.path().join(format!("{name}-obs"))).unwrap();
        cfg += &layer(&home, name, "'request'", &args)
            .replace("process='persistent'", &format!("process='{process}'"))
            .replace(
                "env={ ECHO_RECORD=",
                &format!("env={{ ECHO_OBSERVE={obs}, ECHO_RECORD="),
            );
    }
    let port = free_port();
    let (parent, _) = background_start(&home, port, up, &cfg);
    tokio::join!(
        observed_hello(&home, "persist"),
        observed_hello(&home, "once")
    );
    let run = home.path().join("run");
    let key = port.to_string();
    assert!(
        matches!(aproxy::daemon::ipc_request_in(&run,&key,aproxy::daemon::IpcOp::PREPARE_SWAP).await,Err(aproxy::daemon::IpcError::Remote{code,details,..}) if code=="invalid_state" && details["state"]=="starting")
    );
    std::fs::write(gate, b"").unwrap();
    let out = parent.wait_with_output().unwrap();
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        aproxy::daemon::ipc_ping_in(&run, &key).await.unwrap().state,
        aproxy::daemon::InstanceState::Serving
    );
    let once = frames(&home, "once-obs");
    assert_eq!(once[0]["event"], "hello");
    assert_eq!(once[1]["event"], "eof");
    assert_eq!(once[0]["pid"], once[1]["pid"]);
    let r = reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(8))
        .build()
        .unwrap()
        .get(format!("http://127.0.0.1:{port}/"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let persist = frames(&home, "persist-obs");
    assert_eq!(persist.len(), 2);
    assert_eq!(persist[0]["pid"], persist[1]["pid"]);
    assert_eq!(persist[1]["seq"], 1);
    assert_ne!(frames(&home, "once-obs")[2]["pid"], once[0]["pid"]);
}

/// 启动中的实例拒绝更换；第二个启动者等到失败，不会报告 already_running，也不会再起一个实例。
#[tokio::test]
async fn second_start_waits_for_failed_hello_and_prepare_swap_changes_nothing() {
    let home = TestHome::new();
    let port = free_port();
    let cfg = layer(&home, "stuck", "'request'", &["no-hello"]);
    let (first, path) = background_start(&home, port, free_port(), &cfg);
    observed_hello(&home, "stuck").await;
    let run = home.path().join("run");
    let key = port.to_string();
    let first_info = aproxy::daemon::ipc_ping_in(&run, &key).await.unwrap();
    assert_eq!(first_info.state, aproxy::daemon::InstanceState::Starting);
    let err = aproxy::daemon::ipc_request_in(&run, &key, aproxy::daemon::IpcOp::PREPARE_SWAP)
        .await
        .unwrap_err();
    assert!(
        matches!(err,aproxy::daemon::IpcError::Remote{code,details,..} if code=="invalid_state" && details["state"]=="starting")
    );
    let second = observing_start(&home, &path).await;
    let original = first.wait_with_output().unwrap();
    let observer = second.wait_with_output().unwrap();
    assert_eq!(original.status.code(), Some(1));
    assert_eq!(observer.status.code(), Some(1));
    let result: Value = serde_json::from_slice(&observer.stdout).unwrap();
    let first: Value = serde_json::from_slice(&original.stdout).unwrap();
    assert_eq!(result["error"], first["error"]);
    assert_eq!(result["error"]["code"], "format_handshake_failed");
    assert!(result.get("outcome").is_none());
    assert_eq!(frames(&home, "stuck").len(), 1);
}

/// 有答复但不兼容时，可选性不改变退出码；缺失或错误的 seq 是错位而不是正文不合法。
#[test]
fn static_hello_errors_and_exits_follow_the_required_optional_table() {
    for required in [true, false] {
        for (mode, kind) in [
            ("wrong-hello-seq", "protocol"),
            ("missing-seq", "protocol"),
            ("bad-protocol", "protocol"),
            ("bad-body-need", "invalid_output"),
            ("malformed-hello", "invalid_output"),
        ] {
            assert_eq!(
                failure(mode, required, "format_handshake_failed", 3)["kind"],
                kind
            );
        }
        assert_eq!(
            failure("missing-stage", required, "format_stage_unsupported", 3)["stages"],
            serde_json::json!(["request"])
        );
        let home = TestHome::new();
        let entry = format!(
            "[[format]]\nname='absent'\ncommand='nonexistent-format-test-387'\nstages=['request']\nrequired={required}"
        );
        let (_, out) = launch(&home, free_port(), &entry);
        let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
        if required {
            assert_eq!(out.status.code(), Some(1));
            assert_eq!(doc["error"]["details"]["kind"], "spawn");
        } else {
            assert!(out.status.success());
            assert_eq!(
                doc["instance"]["formats"][0]["breaker"]["last_failure"]["details"]["kind"],
                "spawn"
            );
        }
    }
    let (_home, out) = start("exit-hello", false);
    assert!(out.status.success());
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        doc["instance"]["formats"][0]["breaker"]["last_failure"]["details"]["kind"],
        "died"
    );
}

/// 在握手期间收到停止通知，管道随被取消的探测关闭，程序进程不遗留。Unix 直接送 SIGTERM。
#[tokio::test]
async fn stopping_during_hello_cancels_the_probe_and_child() {
    let home = TestHome::new();
    let port = free_port();
    let obs = serde_json::to_string(&home.path().join("probe-obs")).unwrap();
    let cfg = layer(&home, "probe", "'request'", &["no-hello"]).replace(
        "env={ ECHO_RECORD=",
        &format!("env={{ ECHO_OBSERVE={obs}, ECHO_RECORD="),
    );
    let (parent, _) = background_start(&home, port, free_port(), &cfg);
    observed_hello(&home, "probe").await;
    let run = home.path().join("run");
    let status = aproxy::daemon::ipc_ping_in(&run, &port.to_string())
        .await
        .unwrap();
    assert_eq!(status.state, aproxy::daemon::InstanceState::Starting);
    let child_pid = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if let Some(pid) = std::fs::read_to_string(home.path().join("probe-obs"))
                .ok()
                .and_then(|s| serde_json::from_str::<Value>(s.trim()).ok())
                .and_then(|v| v["pid"].as_u64())
            {
                return pid as u32;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    #[cfg(unix)]
    {
        assert_eq!(
            unsafe { libc::kill(status.instance.pid.try_into().unwrap(), libc::SIGTERM) },
            0
        );
    }
    #[cfg(not(unix))]
    {
        aproxy::daemon::ipc_request_in(&run, &port.to_string(), aproxy::daemon::IpcOp::STOP)
            .await
            .unwrap();
    }
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while aproxy::process::process_exited(child_pid) != Some(true) {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(!parent.wait_with_output().unwrap().status.success());
}

/// 按请求标识读取实际实例的完成记录，避免把别的请求的规则或报告误判为本次结果。
async fn request_detail(home: &TestHome, url: &str, id: &str) -> Value {
    aproxy::daemon::ipc_read_in(
        &home.path().join("run"),
        url.rsplit(':').next().unwrap(),
        &aproxy::daemon::ReadOp::Request {
            request_id: id.into(),
        },
    )
    .await
    .unwrap()
}

/// 同一回信的报告、状态与上限先于 fail 生效；invalid 嵌套状态不受合法外层状态遮蔽。
#[tokio::test]
async fn format_failure_keeps_overrides_and_rejects_bad_nested_status() {
    let (up, calls) = upstream(&[200]).await;
    for invalid in [false, true] {
        let home = TestHome::new();
        let reply = if invalid {
            r#"{"response":{"status":200},"fail":{"retry":false,"reason":"bad","response":{"status":600}}}"#
        } else {
            r#"{"state":"local","report":{"summary":"attempt note","actions":[]},"overrides":{"retry":{"max_attempts":1}},"fail":{"retry":true,"reason":"try"}}"#
        };
        let cfg = layer(&home, "fail", "'request'", &["reply", reply]) + "breaker_failures=1\n";
        let (client, url) = running(&home, up, &cfg);
        let res = client.post(&url).send().await.unwrap();
        let id = res.headers()["x-aproxy-request-id"]
            .to_str()
            .unwrap()
            .to_owned();
        assert_eq!(
            res.headers()["x-aproxy-error"],
            if invalid {
                "format_unavailable"
            } else {
                "retry_exhausted"
            }
        );
        res.bytes().await.unwrap();
        let d = request_detail(&home, &url, &id).await;
        let tries = d["record"]["tries"].as_array().unwrap();
        assert_eq!(
            tries[0]["code"],
            if invalid {
                "format_error"
            } else {
                "format_fail"
            }
        );
        assert!(d["tries"][0]["response"].is_null());
        if invalid {
            assert_eq!(tries[1]["code"], "format_unavailable");
            assert_eq!(d["tries"][0]["error"]["details"]["kind"], "invalid_output");
        } else {
            assert_eq!(tries.len(), 1);
            assert_eq!(tries[0]["decision"]["by"], "override");
            assert_eq!(d["record"]["reports"][0]["summary"], "attempt note");
        }
    }
    assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 0);
}

/// 每个程序、每个请求独享 state；省略、清空、恰好达到与越过字节上限都从下一帧验证。
#[tokio::test]
async fn format_state_is_isolated_and_bounded_on_the_wire() {
    let home = TestHome::new();
    let (up, _) = upstream(&[503, 503, 503, 503, 200, 503, 503, 503, 503, 200]).await;
    let full = "界".repeat(21845) + "x";
    assert_eq!(full.len(), 65536);
    let mut cfg = String::new();
    for (name, initial) in [("a", full.as_str()), ("b", "separate")] {
        let script = home.path().join(format!("{name}.json"));
        let mut program = serde_json::json!({"request":{"state":initial},"retry:4":{"state":""}});
        if name == "a" {
            program["retry:3"] = serde_json::json!({"state":format!("{full}界")});
        }
        std::fs::write(&script, program.to_string()).unwrap();
        cfg += &layer(
            &home,
            name,
            "'request','retry','response'",
            &["reply-by-stage", script.to_str().unwrap()],
        );
        cfg += "required=false\n";
    }
    let (client, url) = running(&home, up, &cfg);
    for _ in 0..2 {
        let res = client.post(&url).send().await.unwrap();
        assert_eq!(res.status(), 200);
        res.bytes().await.unwrap();
    }
    for (name, initial) in [("a", full.as_str()), ("b", "separate")] {
        let f = frames(&home, name);
        let stages: Vec<_> = f.iter().filter(|f| f["stage"] != "hello").collect();
        assert_eq!(stages.len(), 12);
        assert_ne!(stages[0]["request_id"], stages[6]["request_id"]);
        for request in stages.as_chunks::<6>().0 {
            assert_eq!(request[0]["stage"], "request");
            assert!(request[0].get("state").is_none());
            for frame in &request[1..4] {
                assert_eq!(frame["stage"], "retry");
                assert_eq!(frame["state"], initial);
            }
            for frame in &request[4..] {
                assert!(frame.get("state").is_none());
            }
        }
        if name == "a" {
            assert_eq!(f.iter().filter(|f| f["stage"] == "hello").count(), 3);
        }
    }
}

/// 可选层的进程错误只略过本层；主动拒绝则直接结束，不能误当成进程故障。
#[tokio::test]
async fn optional_format_faults_preserve_input_and_warn_once() {
    let (up, calls) = upstream(&[200]).await;
    for args in [
        &["exit1"][..],
        &["sleep", "1800"],
        &["invalid"],
        &["reply", r#"{"request":{"url":"relative"}}"#],
        &["error"],
    ] {
        let home = TestHome::new();
        let cfg = layer(&home, "optional", "'request'", args)
            + "required=false\ntimeout_secs=1\n"
            + &layer(&home, "suffix", "'request'", &["reply", "{}"]);
        let (port, out) = launch(&home, up, &cfg);
        assert!(out.status.success(), "{out:?}");
        let startup: Value = serde_json::from_slice(&out.stdout).unwrap();
        let res = reqwest::Client::builder()
            .no_proxy()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .unwrap()
            .post(format!("http://127.0.0.1:{port}/"))
            .body("input")
            .send()
            .await
            .unwrap();
        let rejected = args[0] == "error";
        assert_eq!(res.status().as_u16(), if rejected { 422 } else { 200 });
        res.bytes().await.unwrap();
        let f = frames(&home, "suffix");
        if rejected {
            assert_eq!(f.len(), 1);
        } else {
            assert_eq!(f[1]["request"]["body"], "input");
        }
        let log = std::fs::read_to_string(startup["log_path"].as_str().unwrap()).unwrap();
        assert_eq!(
            log.matches("optional format layer skipped").count(),
            usize::from(!rejected)
        );
    }
    assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 4);
}

/// 一次覆盖压过实际配置的一次尝试规则，但只对命中路径的这份请求有效。
#[tokio::test]
async fn retry_rule_and_override_sources_are_recorded_per_request() {
    let home = TestHome::new();
    let (up, calls) = upstream(&[503, 200, 503]).await;
    let cfg = String::from("[[retry.rules]]\npath='.*'\nstatus=[503]\nmax_attempts=1\n")
        + &layer(
            &home,
            "chooser",
            "'request'",
            &[
                "reply-on-path",
                r#"{"overrides":{"retry":{"max_attempts":2,"misspelled":"never-log-this-value"}}}"#,
            ],
        );
    let (port, out) = launch(&home, up, &cfg);
    assert!(out.status.success(), "{out:?}");
    let start: Value = serde_json::from_slice(&out.stdout).unwrap();
    let url = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(6))
        .build()
        .unwrap();
    for (path, expected, by) in [("limited", 200, "override"), ("other", 503, "rule:1")] {
        let response = client.post(format!("{url}/{path}")).send().await.unwrap();
        assert_eq!(response.status(), expected);
        response.bytes().await.unwrap();
        let f = frames(&home, "chooser");
        let id = f.last().unwrap()["request_id"].as_str().unwrap();
        assert_eq!(
            request_detail(&home, &url, id).await["record"]["tries"][0]["decision"]["by"],
            by
        );
    }
    let log = std::fs::read_to_string(start["log_path"].as_str().unwrap()).unwrap();
    assert_eq!(
        log.matches("invalid or unknown format override ignored")
            .count(),
        1
    );
    assert!(!log.contains("never-log-this-value"));
    assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 3);
}

/// 同一组头在层间、重试前缀与真实 HTTP 上保持一致；编码不同的历史体不能只按字节版本合并。
#[tokio::test]
async fn request_format_headers_encoding_and_history_match_the_upstream() {
    use base64::Engine as _;
    use std::io::Write as _;
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gz.write_all(b"original").unwrap();
    let compressed = gz.finish().unwrap();
    for mode in [
        "decoded",
        "two-fields",
        "bad-gzip",
        "header-only",
        "no-program",
        "metadata-only",
    ] {
        let home = TestHome::new();
        let data = if mode == "bad-gzip" {
            b"broken".to_vec()
        } else {
            compressed.clone()
        };
        let up = free_port();
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, up))
            .await
            .unwrap();
        let observations = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = observations.clone();
        let app =
            axum::Router::new().fallback(move |h: axum::http::HeaderMap, b: axum::body::Bytes| {
                let seen = seen.clone();
                async move {
                    let mut obs = seen.lock().unwrap();
                    obs.push((h, b));
                    if obs.len() == 1 && mode != "no-program" {
                        axum::http::StatusCode::SERVICE_UNAVAILABLE
                    } else {
                        axum::http::StatusCode::OK
                    }
                }
            });
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut cfg = String::new();
        if mode == "metadata-only" {
            cfg += &layer(&home, "meta", "'request','retry'", &["metadata-only"]);
        } else if mode != "no-program" {
            let script = home.path().join("patch.json");
            let mut h = serde_json::json!({"X-Test":"ok","Connection":"close","Content-Length":"900","bad key":"ignored"});
            if mode == "bad-gzip" {
                h["Content-Encoding"] = "gzip".into();
            }
            let first = serde_json::json!({"request":{"headers":h}});
            h["Content-Encoding"] = "gzip".into();
            std::fs::write(
                &script,
                serde_json::json!({"request":first,"retry":{"request":{"headers":h}}}).to_string(),
            )
            .unwrap();
            cfg += &layer(
                &home,
                "edit",
                "'request','retry'",
                &["reply-by-stage", script.to_str().unwrap()],
            );
            cfg += &layer(&home, "check", "'request','retry'", &["reply", "{}"]);
        }
        let (client, url) = running(&home, up, &cfg);
        let mut req = client.post(&url).body(data.clone());
        if mode == "two-fields" {
            req = req.header("content-encoding", "identity");
        }
        if mode != "header-only" {
            req = req.header("content-encoding", "gzip");
        }
        let res = req.send().await.unwrap();
        assert_eq!(res.status(), 200);
        res.bytes().await.unwrap();
        let expected = if matches!(mode, "decoded" | "two-fields") {
            b"original".as_slice()
        } else {
            data.as_slice()
        };
        {
            let obs = observations.lock().unwrap();
            assert_eq!(obs.len(), if mode == "no-program" { 1 } else { 2 });
            for (i, (h, b)) in obs.iter().enumerate() {
                assert_eq!(b.as_ref(), expected);
                assert_eq!(
                    h["content-length"].to_str().unwrap(),
                    expected.len().to_string()
                );
                assert_eq!(
                    h.get("content-encoding").is_some(),
                    i == 1 || matches!(mode, "bad-gzip" | "no-program" | "metadata-only")
                );
                if !matches!(mode, "no-program" | "metadata-only") {
                    assert_eq!(h["x-test"], "ok");
                }
            }
        }
        if matches!(mode, "no-program" | "metadata-only") {
            continue;
        }
        let edit = frames(&home, "edit");
        let check = frames(&home, "check");
        assert_eq!(edit[1]["request"], edit[2]["request"]);
        for frame in &check[1..] {
            let h = frame["request"]["headers"].as_object().unwrap();
            assert_eq!(h["x-test"], "ok");
            assert!(
                h.keys()
                    .all(|k| matches!(k.as_str(), "x-test" | "content-encoding"))
            );
        }
        if mode == "header-only" {
            let id = edit[1]["request_id"].as_str().unwrap();
            let detail = request_detail(&home, &url, id).await;
            assert_eq!(detail["tries"][1]["sent"]["body"]["state"], "retained");
            let data = aproxy::daemon::ipc_read_in(
                &home.path().join("run"),
                url.rsplit(':').next().unwrap(),
                &aproxy::daemon::ReadOp::Body {
                    request_id: id.into(),
                    part: aproxy::history::Part::Sent,
                    attempt: Some(2),
                    offset: None,
                    limit: None,
                },
            )
            .await
            .unwrap();
            assert_eq!(
                base64::engine::general_purpose::STANDARD
                    .decode(data["data_b64"].as_str().unwrap())
                    .unwrap(),
                b"original"
            );
        }
    }
}

/// 已提交后保留客户端状态行，只按程序给的类型写 SSE、紧凑 JSON 或标准终态。
#[tokio::test]
async fn committed_format_rejection_uses_the_terminal_event_rules() {
    let (up, calls) = upstream(&[200]).await;
    for (mime, body, tail) in [
        (
            "Text/Event-Stream; charset=utf-8",
            "event: end\ndata: custom\n\n",
            "event: end\ndata: custom\n\n",
        ),
        (
            "application/json",
            "{\r\n \"id\": 123456789012345678901234567890, \"n\": 1e999\n}",
            "event: error\ndata: { \"id\": 123456789012345678901234567890, \"n\": 1e999}\n\n",
        ),
        ("text/plain", "not an event", ""),
        ("application/not-text/event-stream", "not SSE", ""),
    ] {
        let home = TestHome::new();
        let json = serde_json::json!({"fail":{"retry":false,"reason":"stop","response":{"status":409,"headers":{"content-type":mime},"body":body}},"overrides":{"keepalive":{"unknown":"never-log-late","interval_secs":"bad-late-value"}}});
        let cfg = String::from(
            "[keepalive]\ninterval_secs=1\ntrigger='accept'\n[override_headers]\naccept='text/plain'\n",
        ) + &layer(&home, "wait", "'request'", &["sleep", "1300"])
            + &layer(&home, "stop", "'request'", &["reply", &json.to_string()]);
        let (port, out) = launch(&home, up, &cfg);
        assert!(out.status.success(), "{out:?}");
        let start: Value = serde_json::from_slice(&out.stdout).unwrap();
        let url = format!("http://127.0.0.1:{port}");
        let res = reqwest::Client::builder()
            .no_proxy()
            .timeout(std::time::Duration::from_secs(6))
            .build()
            .unwrap()
            .get(&url)
            .header("accept", "text/event-stream")
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
        let bytes = res.text().await.unwrap();
        if tail.is_empty() {
            assert!(bytes.contains("event: error\ndata:") && bytes.contains("format_rejected"));
            assert!(!bytes.contains(body));
        } else {
            assert!(bytes.ends_with(tail), "{bytes}");
        }
        let f = frames(&home, "stop");
        assert_eq!(f[1]["client"]["stream"], true);
        let d = request_detail(&home, &url, f[1]["request_id"].as_str().unwrap()).await;
        assert_eq!(d["record"]["response_bytes"], tail.len());
        assert_eq!(d["record"]["code"], "format_rejected");
        let log = std::fs::read_to_string(start["log_path"].as_str().unwrap()).unwrap();
        assert_eq!(
            log.matches("invalid or unknown format override ignored")
                .count(),
            2
        );
        assert!(!log.contains("never-log-late") && !log.contains("bad-late-value"));
    }
    assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 0);
}
/// 关闭尚未提交的真实 HTTP 连接：不启动心跳也能取消卡在请求或非 SSE 响应阶段的进程。
#[tokio::test]
async fn disconnected_format_request_releases_the_only_worker_without_heartbeat() {
    use tokio::io::AsyncWriteExt as _;
    for (mode, stage) in [("gate-request", "request"), ("gate-response", "response")] {
        let home = TestHome::new();
        let (up, _) = upstream(&[200]).await;
        let gate = home.path().join("hold");
        let cfg = String::from("[keepalive]\ninterval_secs=0\n")
            + &layer(
                &home,
                "gated",
                "'request','response'",
                &[mode, gate.to_str().unwrap()],
            )
            + "pool_max=1\ntimeout_secs=0\n";
        let (client, url) = running(&home, up, &cfg);
        let mut conn = tokio::net::TcpStream::connect(url.strip_prefix("http://").unwrap())
            .await
            .unwrap();
        conn.write_all(
            b"POST /hold HTTP/1.1\r\nHost: localhost\r\nContent-Length: 7\r\n\r\nblocked",
        )
        .await
        .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(4), async {
            loop {
                if std::fs::read_to_string(home.path().join("gated"))
                    .unwrap_or_default()
                    .lines()
                    .filter_map(|s| serde_json::from_str::<Value>(s).ok())
                    .any(|f| f["stage"] == stage)
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        drop(conn);
        let res = client
            .post(format!("{url}/next"))
            .body("second")
            .timeout(std::time::Duration::from_secs(4))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
        res.bytes().await.unwrap();
        assert_eq!(
            frames(&home, "gated")
                .iter()
                .filter(|f| f["stage"] == "hello")
                .count(),
            2,
            "{mode}: {:?}",
            frames(&home, "gated")
        );
        assert!(!gate.exists());
    }
}

/// 测试控制上游给出最终响应的时刻；先观察实际子进程读帧，再释放对应的一份响应。
async fn held_upstream(
    statuses: &'static [u16],
    body: &'static str,
) -> (
    u16,
    std::sync::Arc<tokio::sync::Semaphore>,
    std::sync::Arc<std::sync::atomic::AtomicUsize>,
) {
    let port = free_port();
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
        .await
        .unwrap();
    let release = std::sync::Arc::new(tokio::sync::Semaphore::new(0));
    let gate = release.clone();
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = calls.clone();
    let app = axum::Router::new().fallback(move || {
        let gate = gate.clone();
        let i = count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            .min(statuses.len() - 1);
        async move {
            gate.acquire().await.unwrap().forget();
            (
                axum::http::StatusCode::from_u16(statuses[i]).unwrap(),
                [("content-type", "text/event-stream")],
                body,
            )
        }
    });
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (port, release, calls)
}

/// 测试进程每次写入完整的一行，观察确认带超时；未写完的一行不当成确认。
async fn observed_stage(home: &TestHome, name: &str, stage: &str, count: usize) -> Vec<Value> {
    tokio::time::timeout(std::time::Duration::from_secs(8), async {
        loop {
            let seen: Vec<Value> = std::fs::read_to_string(home.path().join(name))
                .unwrap_or_default()
                .lines()
                .filter_map(|s| serde_json::from_str::<Value>(s).ok())
                .filter(|f| f["stage"] == stage)
                .collect();
            if seen.len() >= count {
                return seen;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap()
}

/// 同一个槽里响应不能排在慢心跳后面；即使响应体为空，下游也立即收到 EOF。
#[tokio::test]
async fn pending_heartbeat_does_not_delay_a_response_or_its_eof() {
    for body in ["data: result\n\n", ""] {
        let home = TestHome::new();
        let (up, release, _) = held_upstream(&[200], body).await;
        let cfg = String::from("[keepalive]\ninterval_secs=3\ntrigger='accept'\n")
            + &layer(
                &home,
                "shared",
                "'heartbeat','response'",
                &["held-beat", home.path().join("never").to_str().unwrap()],
            )
            + "pool_max=1\ntimeout_secs=0\n";
        let (client, url) = running(&home, up, &cfg);
        let res = client
            .post(&url)
            .header("accept", "text/event-stream")
            .send()
            .await
            .unwrap();
        observed_stage(&home, "shared", "heartbeat", 1).await;
        release.add_permits(1);
        let text = tokio::time::timeout(std::time::Duration::from_millis(1500), res.text())
            .await
            .expect("response waited on heartbeat")
            .unwrap();
        assert_eq!(text, body);
        assert_eq!(breaker(&home, &url).await["failures"], 0);
        assert_eq!(
            frames(&home, "shared")
                .iter()
                .filter(|f| f["stage"] == "hello")
                .count(),
            2
        );
        assert_eq!(breaker(&home, &url).await["failures"], 0);
    }
}

/// 中间层出错或熔断均跳过；省略 data 保持上一层的字节，不让它覆盖外层的正常结果。
#[tokio::test]
async fn heartbeat_skips_failed_or_open_layers_and_keeps_each_client() {
    let home = TestHome::new();
    let (up, release, _) = held_upstream(&[200], "data: done\n\n").await;
    let cfg = String::from("[keepalive]\ninterval_secs=1\n")
        + &layer(&home, "outer", "'heartbeat'", &["append", "outer"])
        + &layer(&home, "fault", "'heartbeat'", &["bad-beat"])
        + "breaker_failures=1\nbreaker_cooldown_secs=30\n"
        + &layer(&home, "inner", "'heartbeat'", &["alternate-beat"]);
    let (client, url) = running(&home, up, &cfg);
    let mut res = client
        .get(format!("{url}/beats"))
        .header("accept", "text/event-stream")
        .send()
        .await
        .unwrap();
    let mut text = String::new();
    let expected = ": keepalive\n\n: outer\n\n: second\n\n: outer\n\n";
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !text.contains(expected) {
            text.push_str(std::str::from_utf8(&res.chunk().await.unwrap().unwrap()).unwrap());
        }
    })
    .await
    .unwrap();
    release.add_permits(1);
    assert!(res.text().await.unwrap().ends_with("data: done\n\n"));
    assert_eq!(
        frames(&home, "fault")
            .iter()
            .filter(|f| f["stage"] == "heartbeat")
            .count(),
        1
    );
    for n in ["inner", "outer"] {
        let f = observed_stage(&home, n, "heartbeat", 2).await;
        for (i, b) in f[..2].iter().enumerate() {
            assert_eq!(b["heartbeat"]["beat"], i + 1);
            assert_eq!(
                b["client"],
                serde_json::json!({"path":"/beats","stream":true})
            );
            if n == "outer" {
                assert_eq!(
                    b["data"],
                    if i == 0 {
                        ": keepalive\n\n"
                    } else {
                        ": second\n\n"
                    }
                );
            }
        }
    }
}

/// 慢外层错过截止时，只能写已及时返回的内层字节；上游不会等迟到的回信。
#[tokio::test]
async fn missed_beat_deadline_preserves_last_timely_fragment() {
    let home = TestHome::new();
    let (up, release, _) = held_upstream(&[200], "data: done\n\n").await;
    let cfg = String::from("[keepalive]\ninterval_secs=1\n")
        + &layer(
            &home,
            "slow",
            "'heartbeat'",
            &["held-beat", home.path().join("hold").to_str().unwrap()],
        )
        + &layer(&home, "inner", "'heartbeat'", &["append", "timely"]);
    let (client, url) = running(&home, up, &cfg);
    let mut res = client
        .get(&url)
        .header("accept", "text/event-stream")
        .send()
        .await
        .unwrap();
    observed_stage(&home, "slow", "heartbeat", 1).await;
    let first = tokio::time::timeout(std::time::Duration::from_secs(3), res.chunk())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(first.as_ref(), b": keepalive\n\n: timely\n\n");
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            if breaker(&home, &url).await["last_failure"]["details"]["kind"] == "timeout" {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("written heartbeat deadline was not charged");
    release.add_permits(1);
    assert!(!res.text().await.unwrap().contains(": late"));
}

/// 故障后最多换进程一次；熔断已开或半开探针已占用时，响应透传且不写 hello。
#[tokio::test]
async fn response_recovery_and_breaker_admission_use_only_allowed_workers() {
    let (up, calls) = upstream(&[200]).await;
    for (mode, expected, n) in [
        ("recover-response", "fresh reply", 2),
        ("exit1", "upstream bytes", 2),
        ("open-skip", "upstream bytes", 1),
        ("probe-response", "upstream bytes", 1),
    ] {
        let home = TestHome::new();
        let flag = home.path().join("probe");
        let fixture = if mode == "open-skip" {
            "probe-response"
        } else {
            mode
        };
        let mut cfg = layer(
            &home,
            "res",
            "'response'",
            &[fixture, flag.to_str().unwrap()],
        );
        if mode == "open-skip" {
            cfg += "breaker_failures=1\nbreaker_cooldown_secs=86400\n";
        }
        if mode == "probe-response" {
            cfg += "breaker_failures=1\nbreaker_cooldown_secs=2\n";
        }
        let (c, url) = running(&home, up, &cfg);
        let get = || c.get(&url).send();
        assert_eq!(get().await.unwrap().text().await.unwrap(), expected);
        assert_eq!(
            frames(&home, "res")
                .iter()
                .filter(|f| f["stage"] == "hello")
                .count(),
            n
        );
        assert_eq!(observed_stage(&home, "res", "response", n).await.len(), n);
        if mode == "open-skip" {
            assert_eq!(get().await.unwrap().text().await.unwrap(), "upstream bytes");
            assert_eq!(frames(&home, "res").len(), 2);
        }
        if mode == "probe-response" {
            std::fs::write(&flag, b"").unwrap();
            let at = breaker(&home, &url).await["probe_at"].as_u64().unwrap();
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64;
            tokio::time::sleep(std::time::Duration::from_millis(at.saturating_sub(now) + 1)).await;
            let probe = tokio::spawn(get());
            observed_stage(&home, "res", "response", 2).await;
            assert_eq!(breaker(&home, &url).await["state"], "half_open");
            assert_eq!(get().await.unwrap().text().await.unwrap(), "upstream bytes");
            assert_eq!(frames(&home, "res").len(), 4);
            std::fs::write(flag.with_extension("release"), b"").unwrap();
            assert_eq!(
                probe.await.unwrap().unwrap().text().await.unwrap(),
                "probe reply"
            );
        }
    }
    assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 7);
}

/// 内层明确拒绝的替代响应仍向外走；没有替代时外层看到原数据，两种都不重发上游。
#[tokio::test]
async fn explicit_response_failure_passes_only_the_selected_response_outward() {
    for custom in [false, true] {
        let home = TestHome::new();
        let (up, calls) = upstream(&[200]).await;
        let mut fail = serde_json::json!({"retry":true,"reason":"stop"});
        if custom {
            fail["response"] =
                serde_json::json!({"status":409,"headers":{"x-inner":"given"},"body":"custom"});
        }
        let cfg = layer(&home, "outer", "'response'", &["append", "!"])
            + &layer(
                &home,
                "inner",
                "'response'",
                &["reply", &serde_json::json!({"fail":fail}).to_string()],
            );
        let (c, url) = running(&home, up, &cfg);
        let res = c.get(url).send().await.unwrap();
        let expected = if custom { "custom" } else { "upstream bytes" };
        assert_eq!(res.status().as_u16(), if custom { 409 } else { 200 });
        assert_eq!(res.headers().contains_key("x-inner"), custom);
        assert_eq!(res.text().await.unwrap(), format!("{expected}!"));
        assert_eq!(frames(&home, "outer")[1]["response"]["body"], expected);
        assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 1);
    }
}

/// 只有判成成功且将交付的响应跑逆序链；保活是否已经提交不能改变派发次数。
#[tokio::test]
async fn response_runs_only_on_the_delivered_success_buffered_or_committed() {
    for committed in [false, true] {
        for exhausted in [false, true] {
            let home = TestHome::new();
            let (up, release, calls) = held_upstream(
                if exhausted { &[503] } else { &[503, 200] },
                "data: upstream\n\n",
            )
            .await;
            let mut cfg = String::from("[keepalive]\ninterval_secs=1\ntrigger='accept'\n");
            if exhausted {
                cfg += "[[retry.rules]]\nstatus=[503]\nmax_attempts=1\n";
            }
            cfg += &layer(&home, "outer", "'response'", &["append", "A"]);
            cfg += &layer(&home, "inner", "'response'", &["append", "B"]);
            let (c, url) = running(&home, up, &cfg);
            if !committed {
                release.add_permits(2);
            }
            let mut req = c.get(url);
            if committed {
                req = req.header("accept", "text/event-stream");
            }
            let mut res = req.send().await.unwrap();
            if committed {
                let first = res.chunk().await.unwrap().unwrap();
                assert!(first.starts_with(b":"));
                release.add_permits(2);
            }
            let body = res.text().await.unwrap();
            if exhausted && committed {
                assert!(body.contains("retry_exhausted"));
            } else {
                assert_eq!(
                    body,
                    if exhausted {
                        "data: upstream\n\n"
                    } else {
                        "data: upstream\n\nBA"
                    }
                );
            }
            assert_eq!(
                calls.load(std::sync::atomic::Ordering::Relaxed),
                if exhausted { 1 } else { 2 }
            );
            for name in ["inner", "outer"] {
                assert_eq!(
                    frames(&home, name)
                        .iter()
                        .filter(|f| f["stage"] == "response")
                        .count(),
                    usize::from(!exhausted)
                );
            }
        }
    }
}

/// 解码后的输入不带上游编码；即使回信只保留原样也交付解码字节，而程序写的新编码必须保留。
#[tokio::test]
async fn response_program_encoding_replaces_only_the_upstream_encoding() {
    use base64::Engine as _;
    use std::io::Write as _;
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gz.write_all(b"decoded").unwrap();
    let raw = gz.finish().unwrap();
    let encoded = raw.clone();
    let port = free_port();
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
        .await
        .unwrap();
    let app = axum::Router::new().fallback(move || {
        let b = encoded.clone();
        async move {
            (
                [
                    ("content-encoding", "gzip"),
                    ("content-type", "application/octet-stream"),
                ],
                b,
            )
        }
    });
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    for (writes, outer) in [(false, false), (true, false), (true, true)] {
        let home = TestHome::new();
        let json = if writes {
            serde_json::json!({"response":{"headers":{"Content-Encoding":"gzip","content-length":"999"},"body_b64":base64::engine::general_purpose::STANDARD.encode(&raw)}})
        } else {
            serde_json::json!({})
        };
        let mut cfg = if outer {
            layer(&home, "outer", "'response'", &["reply", "{}"])
        } else {
            String::new()
        };
        cfg += &layer(&home, "codec", "'response'", &["reply", &json.to_string()]);
        let (c, url) = running(&home, port, &cfg);
        let res = c.get(url).send().await.unwrap();
        if outer {
            let f = frames(&home, "outer");
            assert_eq!(f[1]["response"]["body"], "decoded");
            assert!(
                f[1]["response"]["headers"]
                    .get("content-encoding")
                    .is_none()
            );
        }
        let expected = if writes && !outer {
            raw.as_slice()
        } else {
            b"decoded"
        };
        assert_eq!(
            res.headers().get("content-encoding").is_some(),
            writes && !outer
        );
        assert_eq!(
            res.headers()["content-length"].to_str().unwrap(),
            expected.len().to_string()
        );
        assert_eq!(res.bytes().await.unwrap().as_ref(), expected);
        let f = frames(&home, "codec");
        assert_eq!(f[1]["response"]["body"], "decoded");
        assert!(
            f[1]["response"]["headers"]
                .get("content-encoding")
                .is_none()
        );
    }
}

/// 决定和是否换路各自取第一票；单独决定已经足够时不再给后续层机会覆盖。
#[tokio::test]
async fn first_decide_vote_wins() {
    let home = TestHome::new();
    let (up, calls) = upstream(&[503, 200]).await;
    let cfg = layer(
        &home,
        "first",
        "'decide'",
        &["reply", r#"{"decision":"retry"}"#],
    ) + &layer(
        &home,
        "last",
        "'decide'",
        &["reply", r#"{"decision":"stop"}"#],
    );
    let (c, url) = running(&home, up, &cfg);
    assert_eq!(c.get(&url).send().await.unwrap().status(), 200);
    assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 2);
    let first = frames(&home, "first");
    assert_eq!(first.len(), 2);
    assert!(
        first[0]["features"]
            .as_array()
            .unwrap()
            .contains(&Value::from("decide.success"))
    );
    let m = &first[1];
    assert_eq!(m["attempt"], 1);
    assert_eq!(m["delivery"], "retry");
    assert_eq!(m["outcome"], "failure");
    assert_eq!(m["failure"]["code"], "upstream_status");
    assert_eq!(
        m["fallback"],
        serde_json::json!({"decision":"retry","delay_ms":0,"by":"default"})
    );
    assert_eq!(m["response"]["body"], "upstream bytes");
    assert!(m["response"]["truncated"].is_null());
    assert!(m["request"].get("headers").is_none());
    assert!(m["request"].get("body").is_none());
    assert_eq!(frames(&home, "last").len(), 1);
}

/// 用两层分别给出 decision 和 reroute，确保第一票不会短路尚未取得的另一个字段。
async fn exercise_reroute(again: bool) {
    let home = TestHome::new();
    let up = free_port();
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, up))
        .await
        .unwrap();
    let sent = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured = sent.clone();
    let app = axum::Router::new().fallback(move |req: axum::extract::Request| {
        let sent = captured.clone();
        async move {
            let key = req.headers()["authorization"].to_str().unwrap().to_owned();
            let body = axum::body::to_bytes(req.into_body(), 1024).await.unwrap();
            let mut sent = sent.lock().unwrap();
            sent.push((key, body));
            if sent.len() == 1 {
                http::StatusCode::SERVICE_UNAVAILABLE
            } else {
                http::StatusCode::OK
            }
        }
    });
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let cfg = layer(
        &home,
        "route",
        "'request','retry'",
        &["rotate", r#"["first","second"]"#],
    ) + &layer(
        &home,
        "vote",
        "'decide'",
        &["reply", r#"{"decision":"retry"}"#],
    ) + &layer(
        &home,
        "choose",
        "'decide'",
        &[
            "reply",
            &serde_json::json!({"decision":"stop","reroute":again}).to_string(),
        ],
    );
    let (c, url) = running(&home, up, &cfg);
    assert_eq!(
        c.post(&url).body("payload").send().await.unwrap().status(),
        200
    );
    {
        let sent = sent.lock().unwrap();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0].0, "Bearer first");
        assert_eq!(
            sent[1].0,
            if again {
                "Bearer second"
            } else {
                "Bearer first"
            }
        );
        assert_eq!(sent[0].1, "payload");
        assert_eq!(sent[0].1, sent[1].1);
    }
    let f = frames(&home, "route");
    assert_eq!(f.iter().filter(|v| v["stage"] == "request").count(), 1);
    assert_eq!(
        f.iter().filter(|v| v["stage"] == "retry").count(),
        usize::from(again)
    );
    let d = request_detail(&home, &url, f[1]["request_id"].as_str().unwrap()).await;
    assert_eq!(d["record"]["tries"][0]["decision"]["reroute"], again);
    assert_eq!(frames(&home, "choose").len(), 2);
}
/// false 保留请求段最后发出的完整表示。
#[tokio::test]
async fn reroute_false_reuses_the_previous_request() {
    exercise_reroute(false).await;
}
/// true 回到第一层 retry 前的缓存，而非重新执行普通前缀。
#[tokio::test]
async fn reroute_true_reruns_the_retry_segment() {
    exercise_reroute(true).await;
}

/// 超时没有投票；即使这个程序是必需的，下一次上游尝试仍能成功。
#[tokio::test]
async fn decide_timeout_falls_back_and_counts_toward_the_breaker() {
    let home = TestHome::new();
    let (up, calls) = upstream(&[503, 503, 200]).await;
    let cfg = layer(&home, "slow", "'decide'", &["sleep", "2500"])
        + "decide_timeout_secs=1\ntimeout_secs=0\nbreaker_failures=1\n";
    let (c, url) = running(&home, up, &cfg);
    assert_eq!(c.get(&url).send().await.unwrap().status(), 200);
    assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 3);
    let b = breaker(&home, &url).await;
    assert_eq!(b["state"], "open");
    assert_eq!(b["last_failure"]["details"]["kind"], "timeout");
    assert_eq!(frames(&home, "slow").len(), 2);
}
/// 进程提前退出只使这次咨询失效，不用 503 替代客户端本来可以得到的成功响应。
#[tokio::test]
async fn decide_crash_never_ends_the_request() {
    let home = TestHome::new();
    let (up, _) = upstream(&[503, 200]).await;
    let cfg = layer(&home, "crash", "'decide'", &["exit1"]);
    let (c, url) = running(&home, up, &cfg);
    let response = c.get(&url).send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.text().await.unwrap(), "upstream bytes");
    assert_eq!(breaker(&home, &url).await["failures"], 1);
}
/// 只给协商了成功咨询的程序看 200；可以判为失败，也可以明确停止或完全不表态。
#[tokio::test]
async fn decide_success_can_fail_a_200() {
    let home = TestHome::new();
    let (up, calls) = upstream(&[200]).await;
    let script = home.path().join("decisions.json");
    std::fs::write(
        &script,
        r#"{"decide:1":{"decision":"retry"},"decide:2":{"decision":"stop"}}"#,
    )
    .unwrap();
    let cfg = layer(
        &home,
        "policy",
        "'decide'",
        &["reply-by-stage", script.to_str().unwrap()],
    )
    .replace(
        "env={",
        "env={ ECHO_DECIDE_SUCCESS='1', ECHO_DECIDE_FULL='1',",
    ) + &layer(&home, "later", "'decide'", &["reply", "{}"])
        .replace("env={", "env={ ECHO_DECIDE_SUCCESS='1',");
    let (c, url) = running(&home, up, &cfg);
    let response = c.post(&url).body("request").send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert!(!response.headers().contains_key("x-aproxy-error"));
    assert_eq!(response.text().await.unwrap(), "upstream bytes");
    assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 2);
    let f = frames(&home, "policy");
    assert_eq!(f.len(), 3);
    assert_eq!(f[1]["outcome"], "success");
    assert!(f[1].get("failure").is_none());
    assert_eq!(f[1]["request"]["body"], "request");
    assert!(f[1]["request"].get("headers").is_none());
    assert_eq!(frames(&home, "later").len(), 1);
    let detail = request_detail(&home, &url, f[1]["request_id"].as_str().unwrap()).await;
    let record = &detail["record"];
    assert_eq!(record["outcome"], "ok");
    assert_eq!(record["tries"][0]["code"], "format_decision");
    assert_eq!(record["tries"][1]["decision"]["decision"], "stop");
    assert_eq!(record["tries"][1]["decision"]["by"], "format:policy");
}
/// 无论原来成功还是失败，程序只改体的补丁都继承原状态；终态不再经过响应转换层。
#[tokio::test]
async fn decide_stop_with_response_returns_it_as_retry_stopped() {
    for status in [200, 503] {
        let home = TestHome::new();
        let (up, _) = upstream(if status == 200 { &[200] } else { &[503] }).await;
        let cfg = layer(
            &home,
            "policy",
            "'decide'",
            &[
                "reply",
                r#"{"decision":"stop","response":{"body":"replacement"}}"#,
            ],
        )
        .replace("env={", "env={ ECHO_DECIDE_SUCCESS='1',")
            + &layer(&home, "outer", "'response'", &["upper"]);
        let (c, url) = running(&home, up, &cfg);
        let response = c.get(&url).send().await.unwrap();
        assert_eq!(response.status().as_u16(), status);
        assert_eq!(response.headers()["x-aproxy-error"], "retry_stopped");
        let id = response.headers()["x-aproxy-request-id"]
            .to_str()
            .unwrap()
            .to_owned();
        assert_eq!(response.text().await.unwrap(), "replacement");
        assert_eq!(frames(&home, "outer").len(), 1);
        let detail = request_detail(&home, &url, &id).await;
        assert_eq!(detail["record"]["code"], "retry_stopped");
        assert_eq!(
            detail["record"]["tries"][0]["code"],
            if status == 200 {
                "format_decision"
            } else {
                "upstream_status"
            }
        );
    }
}
/// 只占住端口而不 listen，确保整个尝试期间都拒绝连接，不误连另一个测试的监听器。
fn refused_address() -> (tokio::net::TcpSocket, std::net::SocketAddr) {
    let reservation = tokio::net::TcpSocket::new_v4().unwrap();
    loop {
        let addr = ([127, 0, 0, 1], free_port()).into();
        match reservation.bind(addr) {
            Ok(()) => return (reservation, addr),
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => continue,
            Err(e) => panic!("{e}"),
        }
    }
}
/// 有上游时保留原错体，没有上游时生成带 last_code 的 502。
#[tokio::test]
async fn decide_stop_without_response_passes_the_upstream_response_through() {
    let (up, _) = upstream(&[503]).await;
    let (_reservation, addr) = refused_address();
    for (port, status) in [(up, 503), (addr.port(), 502)] {
        let home = TestHome::new();
        let cfg = layer(
            &home,
            "stopper",
            "'decide'",
            &["reply", r#"{"decision":"stop"}"#],
        );
        let (c, url) = running(&home, port, &cfg);
        let res = c.get(url).send().await.unwrap();
        assert_eq!(res.status().as_u16(), status);
        assert_eq!(res.headers()["x-aproxy-error"], "retry_stopped");
        if status == 503 {
            assert_eq!(res.text().await.unwrap(), "upstream bytes");
        } else {
            let j = res.json::<Value>().await.unwrap();
            assert_eq!(
                j["error"]["aproxy"]["details"]["last_code"],
                "upstream_connect_failed"
            );
        }
    }
}
/// 实际延迟和记录中的 delay_ms 都来自投票，非 default 的零退避。
#[tokio::test]
async fn decide_delay_overrides_the_backoff() {
    let home = TestHome::new();
    let (up, _) = upstream(&[503, 200]).await;
    let cfg = layer(
        &home,
        "delay",
        "'decide'",
        &["reply", r#"{"decision":"retry","delay_ms":180}"#],
    );
    let (c, url) = running(&home, up, &cfg);
    let start = std::time::Instant::now();
    let res = c.get(&url).send().await.unwrap();
    assert_eq!(res.status(), 200);
    res.bytes().await.unwrap();
    assert!(start.elapsed() >= std::time::Duration::from_millis(180));
    let f = frames(&home, "delay");
    let d = request_detail(&home, &url, f[1]["request_id"].as_str().unwrap()).await;
    let waited = d["record"]["tries"][0]["decision"]["delay_ms"]
        .as_u64()
        .unwrap();
    assert!(
        waited >= 180 && u128::from(waited) <= start.elapsed().as_millis(),
        "{waited}"
    );
}
/// 真正的磁盘体越过 1 MiB 后只给首尾，转发字节不受预览截断影响。
#[tokio::test]
async fn large_upstream_body_reaches_decide_truncated() {
    let home = TestHome::new();
    let up = free_port();
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, up))
        .await
        .unwrap();
    let text = format!(
        "{}{}{}",
        "a".repeat(65536),
        "b".repeat(32768),
        "z".repeat(1024 * 1024 - 65536)
    );
    let raw = text.clone();
    let app = axum::Router::new().fallback(move || {
        let t = raw.clone();
        async move { (axum::http::StatusCode::SERVICE_UNAVAILABLE, t) }
    });
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let cfg = layer(
        &home,
        "preview",
        "'decide'",
        &["reply", r#"{"decision":"stop"}"#],
    );
    let (c, url) = running(&home, up, &cfg);
    let response = c.get(url).send().await.unwrap();
    assert_eq!(response.status(), 503);
    assert_eq!(response.text().await.unwrap(), text);
    let f = frames(&home, "preview");
    assert_eq!(f[1]["response"]["truncated"], 32768);
    assert_eq!(
        f[1]["response"]["body"],
        format!("{}{}", "a".repeat(65536), "z".repeat(1024 * 1024 - 65536))
    );
}
/// 格式程序可以推翻规则的停止判定；尝试保存真正的决策来源、报告，第二次成功无票。
#[tokio::test]
async fn decision_and_reports_are_recorded_in_the_attempt() {
    let home = TestHome::new();
    let (up, _) = upstream(&[503, 200]).await;
    let cfg = String::from("[[retry.rules]]\nstatus=[503]\nmax_attempts=1\n")
        + &layer(
            &home,
            "voter",
            "'decide'",
            &[
                "reply",
                r#"{"decision":"retry","report":{"summary":"decision note","actions":[]}}"#,
            ],
        );
    let (c, url) = running(&home, up, &cfg);
    let response = c.get(&url).send().await.unwrap();
    assert_eq!(response.status(), 200);
    response.bytes().await.unwrap();
    let f = frames(&home, "voter");
    assert_eq!(
        f[1]["fallback"],
        serde_json::json!({"decision":"stop","by":"rule:1"})
    );
    let d = request_detail(&home, &url, f[1]["request_id"].as_str().unwrap()).await;
    let tries = &d["record"]["tries"];
    assert_eq!(tries[0]["decision"]["by"], "format:voter");
    assert_eq!(tries[0]["reports"][0]["summary"], "decision note");
    assert_eq!(tries[0]["reports"][0]["stage"], "decide");
    assert!(tries[1]["decision"].is_null());
}

/// 成功咨询没有投票时，也不能因为程序回了报告就凭空生成决定或错误终态。
#[tokio::test]
async fn successful_decide_abstention_keeps_a_null_decision() {
    let home = TestHome::new();
    let (up, _) = upstream(&[200]).await;
    let cfg = layer(
        &home,
        "observe",
        "'decide'",
        &["reply", r#"{"report":{"summary":"seen","actions":[]}}"#],
    )
    .replace("env={", "env={ ECHO_DECIDE_SUCCESS='1',");
    let (c, url) = running(&home, up, &cfg);
    let response = c.get(&url).send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert!(!response.headers().contains_key("x-aproxy-error"));
    response.bytes().await.unwrap();
    let f = frames(&home, "observe");
    let d = request_detail(&home, &url, f[1]["request_id"].as_str().unwrap()).await;
    assert_eq!(d["record"]["outcome"], "ok");
    let attempt = &d["record"]["tries"][0];
    assert!(attempt["decision"].is_null());
    assert!(attempt["code"].is_null());
    assert_eq!(attempt["reports"][0]["summary"], "seen");
}

/// HEAD 和无正文状态不能把程序提供但没发出的诊断体计入客户端交付量。
#[tokio::test]
async fn bodyless_format_terminals_record_zero_delivered_bytes() {
    let (up, _) = upstream(&[200]).await;
    for (method, status) in [
        (reqwest::Method::HEAD, 409),
        (reqwest::Method::GET, 204),
        (reqwest::Method::GET, 304),
    ] {
        let home = TestHome::new();
        let reply = serde_json::json!({"fail":{"retry":false,"reason":"stop","response":{"status":status,"body":"not delivered"}}});
        let cfg = layer(&home, "stop", "'request'", &["reply", &reply.to_string()]);
        let (c, url) = running(&home, up, &cfg);
        let res = c.request(method, &url).send().await.unwrap();
        assert_eq!(res.status().as_u16(), status);
        let id = res.headers()["x-aproxy-request-id"]
            .to_str()
            .unwrap()
            .to_owned();
        assert!(res.bytes().await.unwrap().is_empty());
        assert_eq!(
            request_detail(&home, &url, &id).await["record"]["response_bytes"],
            0
        );
    }
}

/// 禁用熔断的可选程序暂缺时，每条请求仍尝试启动；恢复可执行文件后立即工作。
#[tokio::test]
async fn disabled_breaker_optional_startup_recovers_on_the_next_request() {
    exercise_optional_startup_recovery(false).await;
}

/// 启动时打开的可选层能在首次冷却后通过真正的数据帧恢复。
#[tokio::test]
async fn optional_startup_recovers_after_its_initial_cooldown() {
    exercise_optional_startup_recovery(true).await;
}

/// 启动时程序文件不存在；安装同一条目指向的程序后，只有合法的阶段应答才能恢复。
async fn exercise_optional_startup_recovery(enabled: bool) {
    let home = TestHome::new();
    let (up, _) = upstream(&[200]).await;
    let exe = home
        .path()
        .join(format!("later{}", std::env::consts::EXE_SUFFIX));
    let cfg = layer(&home, "optional", "'request'", &["echo"]).replace(
        &serde_json::to_string(&format_echo_path()).unwrap(),
        &serde_json::to_string(&exe).unwrap(),
    ) + if enabled {
        "required=false\nbreaker_failures=1\nbreaker_cooldown_secs=1\n"
    } else {
        "required=false\nbreaker_failures=0\n"
    };
    let (c, url) = running(&home, up, &cfg);
    if enabled {
        assert_eq!(breaker(&home, &url).await["state"], "open");
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    } else {
        let ping =
            aproxy::daemon::ipc_ping_in(&home.path().join("run"), url.rsplit(':').next().unwrap())
                .await
                .unwrap();
        let log = std::fs::read_to_string(ping.instance.log_path.unwrap()).unwrap();
        assert_eq!(
            log.lines()
                .filter(|l| l.contains("optional format unavailable at startup")
                    && l.contains("optional")
                    && l.contains("spawn"))
                .count(),
            1,
            "{log}"
        );
        assert_eq!(breaker(&home, &url).await["state"], "closed");
        assert_eq!(c.get(&url).send().await.unwrap().status(), 200);
        assert_eq!(breaker(&home, &url).await["failures"], 2);
    }
    std::fs::copy(format_echo_path(), exe).unwrap();
    assert_eq!(c.get(&url).send().await.unwrap().status(), 200);
    assert_eq!(breaker(&home, &url).await["failures"], 0);
    assert_eq!(breaker(&home, &url).await["state"], "closed");
    assert_eq!(frames(&home, "optional").len(), 2);
}

/// 上游已完成而响应程序仍被门闩挡住的各拍，复用最后的及时产物，不再咨询程序。
#[tokio::test]
async fn response_ready_heartbeat_reuses_last_timely_bytes_without_more_calls() {
    let home = TestHome::new();
    let (up, release, _) = held_upstream(&[200], "data: done\n\n").await;
    let gate = home.path().join("gate");
    let cfg = String::from("[keepalive]\ninterval_secs=1\n")
        + &layer(&home, "beat", "'heartbeat'", &["append", "chosen"])
        + &layer(
            &home,
            "slow",
            "'response'",
            &["gate-response", gate.to_str().unwrap()],
        );
    let (c, url) = running(&home, up, &cfg);
    let mut res = c
        .get(format!("{url}/hold"))
        .header("accept", "text/event-stream")
        .send()
        .await
        .unwrap();
    let first = res.chunk().await.unwrap().unwrap();
    assert!(first.ends_with(b": chosen\n\n"));
    release.add_permits(1);
    observed_stage(&home, "slow", "response", 1).await;
    let before = frames(&home, "beat").len();
    assert_eq!(res.chunk().await.unwrap().unwrap(), first);
    assert_eq!(res.chunk().await.unwrap().unwrap(), first);
    assert_eq!(frames(&home, "beat").len(), before);
    std::fs::write(gate, b"").unwrap();
    assert!(res.text().await.unwrap().ends_with("data: done\n\n"));
}

/// 限制最终写入日志的 UTF-8 正文字节，而不是替换非法字节之前的原始管道长度。
#[test]
fn stderr_rendered_utf8_observes_the_byte_limit() {
    let (_home, out) = start("stderr-bytes", true);
    assert!(out.status.success(), "{out:?}");
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    let log = std::fs::read_to_string(doc["log_path"].as_str().unwrap()).unwrap();
    for tag in ["invalid:", "text:"] {
        let line = log
            .lines()
            .find_map(|l| {
                l.split_once("format[test-layer] ")
                    .map(|(_, s)| s)
                    .filter(|s| s.starts_with(tag))
            })
            .unwrap();
        let payload = line
            .strip_suffix("…(truncated)")
            .expect("rendered line must carry marker");
        assert!(payload.len() <= 8192);
        if tag == "text:" {
            assert!(!payload.contains('�'));
        }
    }
}

/// 本地无法解码不表示进程出错；下一次合法响应继续使用原握手的进程。
#[tokio::test]
async fn local_response_decode_failure_keeps_the_admitted_worker() {
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, free_port()))
        .await
        .unwrap();
    let up = listener.local_addr().unwrap().port();
    let app = axum::Router::new()
        .route(
            "/broken",
            axum::routing::get(|| async { ([("content-encoding", "gzip")], "invalid gzip") }),
        )
        .fallback(|| async { "ok" });
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let home = TestHome::new();
    let (c, url) = running(
        &home,
        up,
        &layer(&home, "response", "'response'", &["reply", "{}"]),
    );
    assert_eq!(
        c.get(format!("{url}/broken"))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        "invalid gzip"
    );
    assert_eq!(
        c.get(&url).send().await.unwrap().text().await.unwrap(),
        "ok"
    );
    let f = frames(&home, "response");
    assert_eq!(f.len(), 2);
    assert_eq!(f[0]["stage"], "hello");
    assert_eq!(f[1]["stage"], "response");
}

/// 各层看到的 Accept-Encoding 跟随当前保活选择；关闭后恢复最近补丁提供的上游值。
#[tokio::test]
async fn request_layers_see_managed_encoding_as_keepalive_changes() {
    let home = TestHome::new();
    let up = free_port();
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, up))
        .await
        .unwrap();
    let app = axum::Router::new().fallback(|h: axum::http::HeaderMap| async move {
        h["accept-encoding"].to_str().unwrap().to_owned()
    });
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let cfg = String::from("[keepalive]\ninterval_secs=10\ntrigger='accept'\n")
        + &layer(
            &home,
            "put",
            "'request'",
            &[
                "reply",
                r#"{"request":{"headers":{"Accept-Encoding":"gzip"}}}"#,
            ],
        )
        + &layer(
            &home,
            "disable",
            "'request'",
            &[
                "reply",
                r#"{"overrides":{"keepalive":{"interval_secs":0}}}"#,
            ],
        )
        + &layer(&home, "read", "'request','retry'", &["reply", "{}"]);
    let (c, url) = running(&home, up, &cfg);
    let r = c
        .get(url)
        .header("accept", "text/event-stream")
        .header("accept-encoding", "br")
        .send()
        .await
        .unwrap();
    assert_eq!(r.text().await.unwrap(), "gzip");
    for (name, seen) in [
        ("put", "identity"),
        ("disable", "identity"),
        ("read", "gzip"),
    ] {
        let f = frames(&home, name);
        assert_eq!(
            f[1]["request"]["headers"]["accept-encoding"], seen,
            "{name}"
        );
    }
}
/// 元数据层入场时后续可选程序尚无握手；真正取得恢复进程后仍须按它的 full 约定准备原体。
#[tokio::test]
async fn recovered_full_layer_decodes_the_original_after_a_metadata_layer() {
    use std::io::Write as _;
    let home = TestHome::new();
    let up = free_port();
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, up))
        .await
        .unwrap();
    let app = axum::Router::new().fallback(|h: http::HeaderMap, b: axum::body::Bytes| async move {
        (h.get("content-encoding")
            .map_or("identity", |v| v.to_str().unwrap())
            .to_owned()
            + ":")
            .into_bytes()
            .into_iter()
            .chain(b)
            .collect::<Vec<_>>()
    });
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let exe = home
        .path()
        .join(format!("later{}", std::env::consts::EXE_SUFFIX));
    let cfg = layer(&home, "meta", "'request'", &["metadata-only"])
        + &layer(&home, "full", "'request'", &["inspect"]).replace(
            &serde_json::to_string(&format_echo_path()).unwrap(),
            &serde_json::to_string(&exe).unwrap(),
        )
        + "required=false\nbreaker_failures=0\n";
    let (client, url) = running(&home, up, &cfg);
    std::fs::copy(format_echo_path(), exe).unwrap();
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gz.write_all(b"original").unwrap();
    let reply = client
        .post(url)
        .header("content-encoding", "gzip")
        .body(gz.finish().unwrap())
        .send()
        .await
        .unwrap();
    let msgs = frames(&home, "full");
    assert_eq!(msgs[1]["request"]["body"], "original");
    assert!(
        msgs[1]["request"]["headers"]
            .get("content-encoding")
            .is_none()
    );
    assert_eq!(reply.bytes().await.unwrap().as_ref(), b"identity:original");
}

/// 曾协商到 full、现在正熔断的可选层不能让另一个只要元数据的程序改写原压缩请求。
#[tokio::test]
async fn open_optional_full_layer_preserves_the_compressed_request() {
    use std::io::Write as _;
    let home = TestHome::new();
    let up = free_port();
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, up))
        .await
        .unwrap();
    let app = axum::Router::new().fallback(
        |h: axum::http::HeaderMap, b: axum::body::Bytes| async move {
            (h.get("content-encoding")
                .map(|v| v.to_str().unwrap())
                .unwrap_or("identity")
                .to_owned()
                + ":"
                + &b.len().to_string())
                .into_bytes()
        },
    );
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let cfg = layer(&home, "full", "'request'", &["bad-seq"])
        + "required=false\nbreaker_failures=1\nbreaker_cooldown_secs=86400\n"
        + &layer(&home, "meta", "'request'", &["metadata-only"]);
    let (c, url) = running(&home, up, &cfg);
    assert_eq!(c.post(&url).send().await.unwrap().status(), 200);
    assert_eq!(breaker(&home, &url).await["state"], "open");
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gz.write_all(b"abc").unwrap();
    let compressed = gz.finish().unwrap();
    let expected = format!("gzip:{}", compressed.len());
    assert_eq!(
        c.post(url)
            .header("content-encoding", "gzip")
            .body(compressed)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        expected
    );
}

/// 只在第二个 CLI 已选择旁观已有实例的就绪等待后返回；子进程是否仍活着不能证明走到了哪一步。
async fn observing_start(home: &TestHome, config: &std::path::Path) -> std::process::Child {
    let log = home.path().join("observer.stderr");
    let mut child = home
        .cmd()
        .env("RUST_LOG", "aproxy::spawn=debug")
        .args(["start", "--json", "--config"])
        .arg(config)
        .stdout(std::process::Stdio::piped())
        .stderr(std::fs::File::create(&log).unwrap())
        .spawn()
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(8), async {
        loop {
            let text = std::fs::read_to_string(&log).unwrap();
            if text.contains("waiting for existing instance readiness") {
                break;
            }
            assert!(
                child.try_wait().unwrap().is_none(),
                "observer exited before readiness wait: {text}"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("observer did not enter its readiness wait");
    child
}

/// 两个启动者拿到同一份静态契约错误和退出码；非公开的启动规格不必交给第二个进程。
#[tokio::test]
async fn observing_start_preserves_a_static_handshake_failure() {
    let home = TestHome::new();
    let port = free_port();
    let gate = home.path().join("release");
    let cfg = layer(
        &home,
        "gate",
        "'request'",
        &["gate-hello", gate.to_str().unwrap()],
    )
    .replace("env={", "env={ ECHO_REFUSE_STAGE='1',");
    let (first, path) = background_start(&home, port, free_port(), &cfg);
    observed_hello(&home, "gate").await;
    let second = observing_start(&home, &path).await;
    std::fs::write(gate, b"").unwrap();
    let mut errors = Vec::new();
    for child in [first, second] {
        let out = child.wait_with_output().unwrap();
        assert_eq!(out.status.code(), Some(3), "{out:?}");
        let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(doc["error"]["code"], "format_stage_unsupported");
        errors.push(doc["error"].clone());
    }
    assert_eq!(errors[0], errors[1]);
    assert_eq!(frames(&home, "gate").len(), 1);
}

/// 成功的非 SSE 体已经到齐，等策略表态时不能改发骨架头或往正文插心跳。
#[tokio::test]
async fn pending_success_vote_preserves_non_sse() {
    let home = TestHome::new();
    let (up, _) = upstream(&[200]).await;
    let gate = home.path().join("decision.release");
    let cfg = String::from("[keepalive]\ntrigger='accept'\ninterval_secs=1\n")
        + &layer(
            &home,
            "policy",
            "'decide'",
            &["gate-decide", gate.to_str().unwrap()],
        )
        .replace(
            "env={",
            "decide_timeout_secs=0\nenv={ ECHO_DECIDE_SUCCESS='1',",
        );
    let (c, url) = running(&home, up, &cfg);
    let mut client = tokio::spawn(c.get(url).header("accept", "text/event-stream").send());
    observed_stage(&home, "policy", "decide", 1).await;
    // 从已收到咨询的时刻起跨过真正的心跳间隔；若响应头已发出，不能再恢复原媒体类型。
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(1250), &mut client)
            .await
            .is_err(),
        "SSE headers escaped while a non-SSE success was being voted on"
    );
    std::fs::write(gate, b"").unwrap();
    let response = client.await.unwrap().unwrap();
    assert_eq!(
        response.headers()["content-type"],
        "text/plain; charset=utf-8"
    );
    assert_eq!(response.text().await.unwrap(), "upstream bytes");
}

/// 慢 SSE 的首个尝试随后被否决，它的 Cookie 和其他响应头不能先抵达客户端。
#[tokio::test]
async fn pending_success_vote_uses_neutral_headers_for_sse() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let home = TestHome::new();
    let up = free_port();
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, up))
        .await
        .unwrap();
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let gate = release.clone();
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let app = axum::Router::new().fallback(move || {
        let gate = gate.clone();
        let i = count.fetch_add(1, Ordering::Relaxed);
        async move {
            let chunk = futures_util::stream::once(async move {
                if i == 0 {
                    gate.acquire().await.unwrap().forget();
                }
                Ok::<_, std::convert::Infallible>(if i == 0 {
                    "data: first\n\n"
                } else {
                    "data: second\n\n"
                })
            });
            (
                [
                    ("content-type", "text/event-stream"),
                    ("set-cookie", "unsettled=yes"),
                ],
                axum::body::Body::from_stream(chunk),
            )
        }
    });
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let script = home.path().join("votes.json");
    std::fs::write(
        &script,
        r#"{"decide:1":{"decision":"retry"},"decide:2":{"decision":"stop"}}"#,
    )
    .unwrap();
    let cfg = String::from("[keepalive]\ntrigger='accept'\ninterval_secs=1\n")
        + &layer(
            &home,
            "policy",
            "'decide'",
            &["reply-by-stage", script.to_str().unwrap()],
        )
        .replace("env={", "env={ ECHO_DECIDE_SUCCESS='1',");
    let (c, url) = running(&home, up, &cfg);
    let response = c
        .get(url)
        .header("accept", "text/event-stream")
        .send()
        .await
        .unwrap();
    release.add_permits(1);
    assert!(
        !response.headers().contains_key("set-cookie"),
        "headers of a vetoed attempt escaped"
    );
    let body = response.text().await.unwrap();
    assert!(!body.contains("data: first"));
    assert!(body.ends_with("data: second\n\n"), "{body:?}");
    assert_eq!(calls.load(Ordering::Relaxed), 2);
}

/// 无法解码也必须咨询，错误的编码头和原始字节让程序自己判断；大输入只保留首尾。
#[tokio::test]
async fn decide_receives_raw_bounded_bytes_when_decoding_fails() {
    use base64::Engine as _;
    for coding in ["unknown", "gzip"] {
        let home = TestHome::new();
        let up = free_port();
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, up))
            .await
            .unwrap();
        let expected = [vec![b'h'; 65536], vec![b't'; 1024 * 1024 - 65536]].concat();
        let raw = bytes::Bytes::from(
            [
                vec![b'h'; 65536],
                vec![b'x'; 16384],
                vec![b't'; 1024 * 1024 - 65536],
            ]
            .concat(),
        );
        let app = axum::Router::new().fallback(move || {
            let raw = raw.clone();
            async move {
                (
                    axum::http::StatusCode::SERVICE_UNAVAILABLE,
                    [("content-encoding", coding), ("x-policy", "visible")],
                    raw,
                )
            }
        });
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let cfg = String::from("[[retry.rules]]\nstatus=[503]\nmax_attempts=1\n")
            + &layer(
                &home,
                "policy",
                "'decide'",
                &[
                    "reply",
                    r#"{"decision":"stop","response":{"body":"consulted"}}"#,
                ],
            );
        let (c, url) = running(&home, up, &cfg);
        let response = c.get(url).send().await.unwrap();
        assert_eq!(
            response.headers()["x-aproxy-error"],
            "retry_stopped",
            "{coding}"
        );
        assert_eq!(response.text().await.unwrap(), "consulted");
        let f = frames(&home, "policy");
        let r = &f[1]["response"];
        assert!(r.get("body").is_none());
        assert_eq!(r["headers"]["content-encoding"], coding);
        assert_eq!(r["headers"]["x-policy"], "visible");
        assert_eq!(r["truncated"], 16384);
        assert!(
            base64::engine::general_purpose::STANDARD
                .decode(r["body_b64"].as_str().unwrap())
                .unwrap()
                == expected
        );
    }
}

/// 在确认客户端已收到骨架头后才放行上游；stop 只改状态，不替换可能落盘或压缩的正文。
async fn committed_stop_body(
    mime: &'static str,
    coding: &'static str,
    raw: Vec<u8>,
) -> bytes::Bytes {
    let home = TestHome::new();
    let up = free_port();
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, up))
        .await
        .unwrap();
    let release = std::sync::Arc::new(tokio::sync::Semaphore::new(0));
    let gate = release.clone();
    let raw = bytes::Bytes::from(raw);
    let app = axum::Router::new().fallback(move || {
        let gate = gate.clone();
        let raw = raw.clone();
        async move {
            gate.acquire().await.unwrap().forget();
            (
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                [("content-type", mime), ("content-encoding", coding)],
                raw,
            )
        }
    });
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let cfg = String::from("[keepalive]\ntrigger='accept'\ninterval_secs=1\n")
        + &layer(
            &home,
            "stop",
            "'decide'",
            &["reply", r#"{"decision":"stop","response":{"status":429}}"#],
        );
    let (c, url) = running(&home, up, &cfg);
    let r = c
        .get(url)
        .header("accept", "text/event-stream")
        .send()
        .await
        .unwrap();
    release.add_permits(1);
    let out = r.bytes().await.unwrap();
    assert_eq!(frames(&home, "stop")[1]["response"]["status"], 503);
    out
}

/// 状态补丁继承的是有效头表描述的正文；已提交的 SSE 里不能出现压缩字节或半个流。
#[tokio::test]
async fn committed_stop_decodes_inherited_body_or_uses_terminal_event() {
    use std::io::Write as _;
    for (mime, plain, tail) in [
        ("text/event-stream", "data: denied\n\n", "data: denied\n\n"),
        (
            "application/json",
            "{\n\"n\":9}",
            "event: error\ndata: {\"n\":9}\n\n",
        ),
    ] {
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gz.write_all(plain.as_bytes()).unwrap();
        let b = committed_stop_body(mime, "gzip", gz.finish().unwrap()).await;
        assert!(b.ends_with(tail.as_bytes()), "unexpected event for {mime}");
    }
    let out = committed_stop_body("text/event-stream", "gzip", b"not encoded".to_vec()).await;
    let text = std::str::from_utf8(&out).unwrap();
    assert!(
        text.contains("retry_stopped") && !text.contains("not encoded"),
        "{text}"
    );
}

/// 状态补丁不能把超过 1 MiB 的磁盘 JSON 搬回内存，或当作单个 SSE 事件写出去。
#[tokio::test]
async fn committed_stop_limits_disk_json_conversion() {
    let raw = format!("{{\"message\":\"{}\"}}", "x".repeat(1024 * 1024));
    let b = committed_stop_body("application/json", "identity", raw.into_bytes()).await;
    assert!(b.len() < 4096, "terminal response was {} bytes", b.len());
    assert!(std::str::from_utf8(&b).unwrap().contains("retry_stopped"));
}

/// 路由写入 URL 路径的已知凭据不能从自动生成的停止诊断回到客户端或进入实例日志。
#[tokio::test]
async fn decide_stop_masks_configured_secret_in_upstream_failure() {
    let home = TestHome::new();
    let (_reservation, addr) = refused_address();
    let secret = "fixture-private-route-segment";
    let patch = serde_json::json!({"request":{"url":format!("http://{addr}/{secret}")}});
    let cfg = layer(&home, "route", "'request'", &["reply", &patch.to_string()])
        .replace("env={", &format!("env={{ TOKEN='{secret}',"))
        + &layer(
            &home,
            "stop",
            "'decide'",
            &["reply", r#"{"decision":"stop"}"#],
        );
    let (port, out) = launch(&home, addr.port(), &cfg);
    assert!(out.status.success(), "{out:?}");
    let startup: Value = serde_json::from_slice(&out.stdout).unwrap();
    let url = format!("http://127.0.0.1:{port}");
    let r = reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(8))
        .build()
        .unwrap()
        .get(&url)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 502);
    let id = r.headers()["x-aproxy-request-id"]
        .to_str()
        .unwrap()
        .to_owned();
    let json = r.json::<Value>().await.unwrap();
    assert_eq!(json["error"]["aproxy"]["code"], "retry_stopped");
    assert!(
        !json.to_string().contains(secret),
        "credential in generated 502"
    );
    assert!(
        json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("<secret>")
    );
    let history = request_detail(&home, &url, &id).await;
    assert!(!history.to_string().contains(secret));
    let log = std::fs::read_to_string(startup["log_path"].as_str().unwrap()).unwrap();
    assert!(!log.contains(secret));
}

/// 在途消息无回信时，从真正的停止命令观察截止时间；正常空闲的另一进程应该在排空开始时看到 EOF。
#[tokio::test]
async fn stop_deadline_reaps_a_worker_with_an_unanswered_message() {
    use std::time::{Duration, Instant};
    let home = TestHome::new();
    let watched = |name: &str, stage: &str, args: &[&str]| {
        let obs = serde_json::to_string(&home.path().join(format!("{name}.events"))).unwrap();
        layer(&home, name, stage, args).replace(
            "env={",
            &format!("timeout_secs=0\nenv={{ ECHO_OBSERVE={obs},"),
        )
    };
    let cfg = watched("busy", "'request'", &["no-stage-reply"])
        + &watched("idle", "'response'", &["echo"]);
    let (port, out) = launch(&home, free_port(), &cfg);
    assert!(out.status.success(), "{out:?}");
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();
    let request = tokio::spawn(client.get(format!("http://127.0.0.1:{port}/")).send());
    observed_stage(&home, "busy", "request", 1).await;
    let event = |name: &str, what: &str| -> Option<u32> {
        std::fs::read_to_string(home.path().join(format!("{name}.events")))
            .ok()?
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .find(|v| v["event"] == what)?["pid"]
            .as_u64()
            .map(|v| v as u32)
    };
    let pid = tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            if let Some(pid) = event("busy", "request") {
                break pid;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let idle = event("idle", "hello").unwrap();
    assert_eq!(aproxy::process::process_exited(pid), Some(false));
    let mut stop = home.cmd();
    stop.args(["stop", &port.to_string(), "--json"]);
    let start = Instant::now();
    let stopping = tokio::task::spawn_blocking(move || stop.output().unwrap());
    let early_eof = tokio::time::timeout(Duration::from_secs(5), async {
        while event("idle", "eof") != Some(idle) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .is_ok();
    let out = stopping.await.unwrap();
    let elapsed = start.elapsed();
    let exited = aproxy::process::process_exited(pid);
    assert!(out.status.success(), "{out:?}");
    assert!(elapsed < Duration::from_secs(12), "stop took {elapsed:?}");
    assert!(early_eof, "idle EOF waited for the request drain deadline");
    assert_eq!(exited, Some(true));
    let _ = request.await;
}

/// 首轮 503 不进入转换；成功回放中的四种帧字段按实际 attempt 编号分配。
#[tokio::test]
async fn retry_replay_runs_through_the_stream_program() {
    let home = TestHome::new();
    let (up, calls) = upstream(&[503, 200]).await;
    let (client, url) = running(
        &home,
        up,
        &layer(&home, "convert", "'stream'", &["stream-echo"]),
    );
    let resp = client.get(url).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    assert!(resp.headers().get("content-length").is_none());
    assert_eq!(resp.text().await.unwrap(), "[upstream bytes]");
    assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 2);
    let frames = frames(&home, "convert");
    let events = &frames[1..];
    assert_eq!(
        events
            .iter()
            .map(|e| e["event"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["open", "data", "close"]
    );
    let id = format!("{}/2", events[0]["request_id"].as_str().unwrap());
    for e in events {
        assert_eq!(e["stream_id"], id);
    }
    assert!(events[1].get("request_id").is_none());
    assert!(events[2].get("state").is_none());
}
/// 必选的最外层在 data 崩溃：先交付已提交的前缀，再写 open 约定的收尾，不重试上游。
#[tokio::test]
async fn mid_stream_failure_writes_abort_tail() {
    let home = TestHome::new();
    let (up, calls) = upstream(&[200]).await;
    let (client, url) = running(
        &home,
        up,
        &format!(
            "{}required=true\n",
            layer(&home, "broken", "'stream'", &["stream-fault"])
        ),
    );
    assert_eq!(
        client.get(url).send().await.unwrap().text().await.unwrap(),
        "[[aborted]"
    );
    assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 1);
}
/// 可选层失败后仍交付当前块与其余块；外层正常关闭而不是收到 abort。
#[tokio::test]
async fn optional_stream_program_failing_mid_stream_is_skipped() {
    let home = TestHome::new();
    let (up, _) = upstream(&[200]).await;
    let cfg = layer(&home, "outer", "'stream'", &["stream-echo"])
        + &layer(&home, "inner", "'stream'", &["stream-fault"])
        + "required=false\n";
    let (client, url) = running(&home, up, &cfg);
    assert_eq!(
        client.get(url).send().await.unwrap().text().await.unwrap(),
        "[[upstream bytes]"
    );
    assert_eq!(frames(&home, "outer").last().unwrap()["event"], "close");
}

/// 客户端在内层 data 往返中断开：先观察外层 abort 再放行内层，禁止把迟到的 data 回信读成 abort 回信。
#[tokio::test]
async fn client_disconnect_aborts_open_streams_on_their_own_workers() {
    use tokio::io::AsyncWriteExt as _;
    let home = TestHome::new();
    let (up, _) = upstream(&[200]).await;
    let gate = home.path().join("release");
    let cfg = layer(&home, "outer", "'stream'", &["stream-echo"])
        + &layer(
            &home,
            "inner",
            "'stream'",
            &["stream-gate", gate.to_str().unwrap()],
        )
        .replace(
            "env={",
            &format!(
                "env={{ECHO_OBSERVE={},",
                serde_json::to_string(&home.path().join("pids")).unwrap()
            ),
        );
    let (_, url) = running(&home, up, &cfg);
    let mut conn = tokio::net::TcpStream::connect(url.strip_prefix("http://").unwrap())
        .await
        .unwrap();
    conn.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .unwrap();
    observed_stage(&home, "inner", "stream", 2).await;
    observed_stage(&home, "outer", "stream", 2).await;
    drop(conn);
    let outer = observed_stage(&home, "outer", "stream", 3).await;
    assert_eq!(
        outer[2]["reason"],
        serde_json::json!({"code":"client_gone"})
    );
    std::fs::write(gate, b"release").unwrap();
    let inner = observed_stage(&home, "inner", "stream", 3).await;
    for f in [&outer, &inner] {
        assert_eq!(f[2]["event"], "abort");
        assert_eq!(f[2]["reason"], serde_json::json!({"code":"client_gone"}));
        assert_eq!(f[2]["stream_id"], f[0]["stream_id"]);
        assert_eq!(f[2]["seq"], 3);
    }
    let pids: Vec<_> = frames(&home, "pids")
        .into_iter()
        .filter(|o| o["event"] == "stream")
        .collect();
    assert_eq!(pids.len(), 3);
    assert!(pids.iter().all(|o| o["pid"] == pids[0]["pid"]));
}
/// 内层拒绝与进程故障是不同的中断；外层收到 abort 而不是 close，使用自己的协议生成结尾。
#[tokio::test]
async fn inner_stream_failures_reach_the_outer_abort_with_their_own_code() {
    for (mode, code) in [
        ("stream-reject", "format_fail"),
        ("stream-fault", "format_error"),
    ] {
        let home = TestHome::new();
        let (up, _) = upstream(&[200]).await;
        let cfg = layer(&home, "outer", "'stream'", &["stream-echo"])
            + &layer(&home, "inner", "'stream'", &[mode])
            + "required=true\n";
        let (client, url) = running(&home, up, &cfg);
        assert_eq!(
            client.get(url).send().await.unwrap().text().await.unwrap(),
            "[[[abort reply]"
        );
        let f = frames(&home, "outer");
        assert_eq!(f.last().unwrap()["event"], "abort");
        assert_eq!(f.last().unwrap()["reason"]["code"], code);
    }
}

/// 第一块立刻发送，只有测试亲眼见到目标层的进展后才准发第二块；不用睡眠模拟流。
async fn split_forward_upstream() -> (u16, std::sync::Arc<tokio::sync::Notify>) {
    use futures_util::StreamExt as _;
    let port = free_port();
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
        .await
        .unwrap();
    let resume = std::sync::Arc::new(tokio::sync::Notify::new());
    let gate = resume.clone();
    let app = axum::Router::new().fallback(move |request: axum::extract::Request| {
        let gate = gate.clone();
        async move {
            let body = axum::body::to_bytes(request.into_body(), 4096)
                .await
                .unwrap();
            let first = if body.is_empty() {
                bytes::Bytes::from_static(b"first")
            } else {
                body
            };
            let chunks = futures_util::stream::once(async move { Ok::<_, std::io::Error>(first) })
                .chain(futures_util::stream::once(async move {
                    gate.notified().await;
                    Ok(bytes::Bytes::from_static(b"last"))
                }));
            axum::body::Body::from_stream(chunks)
        }
    });
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (port, resume)
}
/// 请求只改写一次，响应的第一块在上游仍打开时到客户端，最后由同一进程收尾。
#[tokio::test]
async fn forward_stream_conversion_end_to_end() {
    let home = TestHome::new();
    let (up, resume) = split_forward_upstream().await;
    let cfg = String::from("delivery='forward'\n")
        + &layer(
            &home,
            "req",
            "'request','retry','decide','heartbeat'",
            &["upper"],
        )
        + &layer(&home, "stream", "'stream'", &["stream-echo"]);
    let (client, url) = running(&home, up, &cfg);
    let mut res = client.post(&url).body("hello").send().await.unwrap();
    let mut prefix = Vec::new();
    while prefix.len() < 6 {
        prefix.extend_from_slice(&res.chunk().await.unwrap().unwrap())
    }
    assert_eq!(&prefix, b"[HELLO");
    resume.notify_one();
    assert_eq!(res.text().await.unwrap(), "last]");
    let sent = frames(&home, "req");
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[1]["stage"], "request");
    assert_eq!(sent[1]["delivery"], "forward");
    let d = request_detail(&home, &url, sent[1]["request_id"].as_str().unwrap()).await;
    assert_eq!(d["record"]["response_bytes"], 11);
    assert_eq!(d["record"]["tries"][0]["response_bytes"], 9);
    assert_eq!(frames(&home, "stream").last().unwrap()["event"], "close");
}
/// 内侧先收到 data，整包层还没有输入；EOF 后它只处理一次，外层处理它的输出。
#[tokio::test]
async fn forward_with_a_response_program_gathers_the_response_at_that_layer() {
    let home = TestHome::new();
    let (up, resume) = split_forward_upstream().await;
    let cfg = String::from("delivery='forward'\n")
        + &layer(&home, "outer", "'stream'", &["stream-echo"])
        + &layer(&home, "whole", "'response'", &["upper"])
        + &layer(&home, "inner", "'stream'", &["stream-echo"]);
    let (client, url) = running(&home, up, &cfg);
    let req = tokio::spawn(client.get(url).send());
    let events = observed_stage(&home, "inner", "stream", 2).await;
    assert_eq!(events[1]["data"], "first");
    for name in ["whole", "outer"] {
        assert_eq!(frames(&home, name).len(), 1);
    }
    resume.notify_one();
    assert_eq!(
        req.await.unwrap().unwrap().text().await.unwrap(),
        "[[FIRSTLAST]]"
    );
    assert_eq!(frames(&home, "whole")[1]["response"]["body"], "[firstlast]");
}
/// 没有 request 阶段的链不等上传 EOF；上游先确认收到前缀，客户端才发送最后一块。
#[tokio::test]
async fn forward_without_request_programs_streams_the_request_body() {
    use futures_util::StreamExt as _;
    let home = TestHome::new();
    let up = free_port();
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, up))
        .await
        .unwrap();
    let first = std::sync::Arc::new(tokio::sync::Notify::new());
    let observed = first.clone();
    let app = axum::Router::new().fallback(move |req: axum::extract::Request| {
        let first = first.clone();
        async move {
            let mut f = req.into_body().into_data_stream();
            let a = f.next().await.unwrap().unwrap();
            assert_eq!(a, "one");
            first.notify_one();
            let b = f.next().await.unwrap().unwrap();
            assert_eq!(b, "two");
            "received"
        }
    });
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let (client, url) = running(
        &home,
        up,
        &(String::from("delivery='forward'\n")
            + &layer(&home, "convert", "'stream'", &["stream-echo"])),
    );
    let body = futures_util::stream::once(async { Ok::<_, std::io::Error>("one") }).chain(
        futures_util::stream::once(async move {
            observed.notified().await;
            Ok("two")
        }),
    );
    assert_eq!(
        client
            .post(url)
            .body(reqwest::Body::wrap_stream(body))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        "[received]"
    );
}

/// 上游的错误响应原样透传；请求侧要求重试却不能升级时本地返回 502，不能暗中再请求一次。
#[tokio::test]
async fn forward_format_failures_do_not_enter_the_retry_chain() {
    let (up, calls) = upstream(&[503]).await;
    for (mode, status, code) in [
        ("{}", 503, None),
        (
            r#"{"fail":{"retry":true,"reason":"not ready"}}"#,
            502,
            Some("format_failed"),
        ),
        (
            r#"{"fail":{"retry":false,"reason":"blocked"}}"#,
            422,
            Some("format_rejected"),
        ),
    ] {
        let home = TestHome::new();
        let cfg = String::from("delivery='forward'\n")
            + &layer(&home, "req", "'request','retry'", &["reply", mode])
            + &layer(&home, "convert", "'stream'", &["stream-echo"]);
        let (client, url) = running(&home, up, &cfg);
        let resp = client.get(url).send().await.unwrap();
        assert_eq!(resp.status(), status);
        if let Some(code) = code {
            let d: Value = resp.json().await.unwrap();
            assert_eq!(d["error"]["aproxy"]["code"], code);
        } else {
            assert_eq!(resp.text().await.unwrap(), "upstream bytes");
        }
        assert_eq!(frames(&home, "req").len(), 2);
        assert_eq!(frames(&home, "convert").len(), 1);
    }
    assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 1);
}
/// 伪成功的 JSON 在原始输入上判定；它即使经过转换就不再像错误，也不得进入回放层。
#[tokio::test]
async fn retry_stream_never_opens_for_an_error_disguised_as_200() {
    let home = TestHome::new();
    let up = free_port();
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, up))
        .await
        .unwrap();
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let app = axum::Router::new().fallback(move || {
        let i = calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        async move {
            axum::Json(if i == 0 {
                serde_json::json!({"error":{"message":"busy"}})
            } else {
                serde_json::json!({"ok":true})
            })
        }
    });
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let (client, url) = running(
        &home,
        up,
        &layer(&home, "convert", "'stream'", &["stream-echo"]),
    );
    assert_eq!(
        client.get(url).send().await.unwrap().text().await.unwrap(),
        r#"[{"ok":true}]"#
    );
    let f = frames(&home, "convert");
    assert_eq!(f.iter().filter(|v| v["event"] == "open").count(), 1);
    assert!(f[1]["stream_id"].as_str().unwrap().ends_with("/2"));
}

/// 读到 503 的头但实体没读完：程序可按头投票，停止时却没有完整响应可以继承。
#[tokio::test]
async fn decide_sees_interrupted_head_but_stop_does_not_inherit_it() {
    use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _};
    for patch in [false, true] {
        let home = TestHome::new();
        let up = free_port();
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, up))
            .await
            .unwrap();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut r = tokio::io::BufReader::new(&mut sock);
            let mut line = String::new();
            loop {
                line.clear();
                assert!(r.read_line(&mut line).await.unwrap() > 0);
                if line == "\r\n" {
                    break;
                }
            }
            sock.write_all(b"HTTP/1.1 503 Unavailable\r\nRetry-After: 7\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nbad\r\n").await.unwrap();
            sock.shutdown().await.unwrap();
        });
        let cfg = layer(
            &home,
            "policy",
            "'decide'",
            &[
                "reply",
                if patch {
                    r#"{"decision":"stop","response":{"body":"patched"}}"#
                } else {
                    r#"{"decision":"stop"}"#
                },
            ],
        );
        let (c, url) = running(&home, up, &cfg);
        let resp = c.get(url).send().await.unwrap();
        assert_eq!(resp.status(), 502);
        assert!(!resp.headers().contains_key("retry-after"));
        if patch {
            assert_eq!(resp.text().await.unwrap(), "patched");
        } else {
            assert_eq!(
                resp.json::<Value>().await.unwrap()["error"]["aproxy"]["code"],
                "retry_stopped"
            );
        }
        let f = frames(&home, "policy");
        assert_eq!(f[1]["failure"]["code"], "upstream_body_interrupted");
        let h = &f[1]["response"];
        assert_eq!(h["status"], 503);
        assert_eq!(h["headers"]["retry-after"], "7");
        assert!(h.get("body").is_none() && h.get("body_b64").is_none());
    }
}

/// 等待的是实例报告的实际阶段，而不是假定一个固定等待后代理已进入那一阶段。
async fn in_phase(home: &TestHome, url: &str, id: &str, phase: &str) -> Value {
    tokio::time::timeout(std::time::Duration::from_secs(8), async {
        loop {
            let doc = request_detail(home, url, id).await;
            if doc["record"]["phase"] == phase {
                return doc["record"].clone();
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap()
}
/// 客户端断开时只取消未完成的咨询或等待，已经完成的尝试仍保留真实字节和失败码。
#[tokio::test]
async fn decide_disconnect_preserves_attempt_and_actual_backoff() {
    use tokio::io::AsyncWriteExt as _;
    for during_wait in [false, true] {
        let home = TestHome::new();
        let (up, calls) = upstream(&[503]).await;
        let gate = home.path().join("hold");
        let args = if during_wait {
            ["reply", r#"{"decision":"retry","delay_ms":86400000}"#]
        } else {
            ["gate-decide", gate.to_str().unwrap()]
        };
        let (_, url) = running(&home, up, &layer(&home, "policy", "'decide'", &args));
        let mut sock = tokio::net::TcpStream::connect(url.strip_prefix("http://").unwrap())
            .await
            .unwrap();
        sock.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        let f = observed_stage(&home, "policy", "decide", 1).await;
        let id = f[0]["request_id"].as_str().unwrap();
        if during_wait {
            in_phase(&home, &url, id, "backoff").await;
        }
        drop(sock);
        let r = in_phase(&home, &url, id, "done").await;
        assert_eq!(r["outcome"], "client_gone");
        let t = &r["tries"][0];
        assert_eq!(t["response_bytes"], 14);
        assert_eq!(t["code"], "upstream_status");
        assert!(t["duration_ms"].is_number());
        if during_wait {
            assert!(t["decision"]["delay_ms"].as_u64().unwrap() < 86400000);
        } else {
            assert!(t["decision"].is_null());
        }
        assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 1);
    }
}
/// 实例确认正在读上传后才刻意暂停；这段时间必须出现在 decide 的 elapsed_ms 里。
#[tokio::test]
async fn decide_clock_includes_client_upload() {
    use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _};
    let home = TestHome::new();
    let (up, _) = upstream(&[503]).await;
    let (_, url) = running(
        &home,
        up,
        &layer(
            &home,
            "policy",
            "'decide'",
            &["reply", r#"{"decision":"stop"}"#],
        ),
    );
    let mut sock = tokio::net::TcpStream::connect(url.strip_prefix("http://").unwrap())
        .await
        .unwrap();
    sock.write_all(b"POST / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: 1\r\nExpect: 100-continue\r\n\r\n").await.unwrap();
    let mut reader = tokio::io::BufReader::new(&mut sock);
    let mut line = String::new();
    tokio::time::timeout(
        std::time::Duration::from_secs(4),
        reader.read_line(&mut line),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(line.contains("100 Continue"), "{line}");
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    sock.write_all(b"x").await.unwrap();
    let mut rest = Vec::new();
    tokio::time::timeout(
        std::time::Duration::from_secs(6),
        sock.read_to_end(&mut rest),
    )
    .await
    .unwrap()
    .unwrap();
    let f = frames(&home, "policy");
    assert!(f[1]["elapsed_ms"].as_u64().unwrap() >= 150, "{}", f[1]);
}
/// 第一层确认上游已结束后移开这一个临时请求文件；后续 full 层不能把本地存储失败当作弃权。
#[tokio::test]
async fn decide_unreadable_request_spool_is_a_local_terminal() {
    let home = TestHome::new();
    let (up, calls) = upstream(&[503]).await;
    let gate = home.path().join("release");
    let cfg = layer(
        &home,
        "gate",
        "'decide'",
        &["gate-decide", gate.to_str().unwrap()],
    ) + &layer(&home, "full", "'decide'", &["reply", "{}"])
        .replace("env={", "env={ ECHO_DECIDE_FULL='1',")
        + &layer(
            &home,
            "late",
            "'decide'",
            &["reply", r#"{"decision":"stop"}"#],
        );
    let (c, url) = running(&home, up, &cfg);
    let req = tokio::spawn(c.post(url).body(vec![b'x'; 1024 * 1024 + 8]).send());
    observed_stage(&home, "gate", "decide", 1).await;
    let mut files = Vec::new();
    for dir in std::fs::read_dir(home.path().join("spool"))
        .unwrap()
        .flatten()
    {
        for entry in std::fs::read_dir(dir.path()).unwrap().flatten() {
            if entry.file_name().to_string_lossy().starts_with("req-") {
                files.push(entry.path());
            }
        }
    }
    assert_eq!(files.len(), 1);
    std::fs::rename(&files[0], home.path().join("saved-body")).unwrap();
    std::fs::write(gate, b"release").unwrap();
    let resp = req.await.unwrap().unwrap();
    assert_eq!(resp.headers()["x-aproxy-error"], "local_storage_failed");
    let e = resp.json::<Value>().await.unwrap();
    assert_eq!(e["error"]["aproxy"]["details"]["op"], "read_request");
    assert_eq!(frames(&home, "late").len(), 1);
    assert_eq!(frames(&home, "full").len(), 1);
    assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 1);
}

/// 一个程序的 stop 即使别的字段合法，也不能从非法 delay 中抢救出来；改用下一票并记录 invalid_output。
#[tokio::test]
async fn decide_invalid_delay_discards_vote_and_counts_invalid_output() {
    let home = TestHome::new();
    let (up, _) = upstream(&[503, 200]).await;
    let cfg = layer(
        &home,
        "bad",
        "'decide'",
        &["reply", r#"{"decision":"stop","delay_ms":86400001}"#],
    ) + "breaker_failures=1\n"
        + &layer(
            &home,
            "next",
            "'decide'",
            &["reply", r#"{"decision":"retry"}"#],
        );
    let (c, url) = running(&home, up, &cfg);
    assert_eq!(c.get(&url).send().await.unwrap().status(), 200);
    assert_eq!(frames(&home, "next").len(), 2);
    let b = breaker(&home, &url).await;
    assert_eq!(b["last_failure"]["details"]["kind"], "invalid_output");
    assert_eq!(b["state"], "open");
}
/// 第六次失败的非零退避和请求上限的 stop 都应出现在管道里，而不是取代理的实际零等待投票。
#[tokio::test]
async fn decide_wire_fallback_exposes_nonzero_backoff_and_exhausted_override() {
    for capped in [false, true] {
        let home = TestHome::new();
        let (up, _) = upstream(&[503, 503, 503, 503, 503, 503, 200]).await;
        let mut cfg = String::new();
        if capped {
            cfg += &layer(
                &home,
                "limits",
                "'request'",
                &["reply", r#"{"overrides":{"retry":{"max_attempts":1}}}"#],
            );
        }
        cfg += &layer(
            &home,
            "vote",
            "'decide'",
            &["reply", r#"{"decision":"retry","delay_ms":0}"#],
        );
        let (port, out) = launch_with_backoff(&home, up, &cfg, 320);
        assert!(out.status.success(), "{out:?}");
        let c = reqwest::Client::builder()
            .no_proxy()
            .timeout(std::time::Duration::from_secs(8))
            .build()
            .unwrap();
        assert_eq!(
            c.get(format!("http://127.0.0.1:{port}/"))
                .send()
                .await
                .unwrap()
                .status(),
            200
        );
        let f = frames(&home, "vote");
        assert_eq!(f[6]["attempt"], 6);
        if capped {
            assert_eq!(
                f[6]["fallback"],
                serde_json::json!({"decision":"stop","by":"override"})
            );
        } else {
            assert_eq!(
                f[6]["fallback"],
                serde_json::json!({"decision":"retry","delay_ms":20000,"by":"default"})
            );
        }
    }
}
/// 每张有效票在日志只有一行，且这一行同时给出程序和失败原因，不能靠拼两条日志猜。
#[tokio::test]
async fn decide_log_has_one_source_and_reason_per_effective_vote() {
    for vote in ["retry", "stop"] {
        let home = TestHome::new();
        let (up, _) = upstream(&[503, 200]).await;
        let patch = serde_json::json!({"decision":vote}).to_string();
        let (port, out) = launch(
            &home,
            up,
            &layer(&home, "policy", "'decide'", &["reply", &patch]),
        );
        assert!(out.status.success(), "{out:?}");
        let d: Value = serde_json::from_slice(&out.stdout).unwrap();
        let c = reqwest::Client::builder()
            .no_proxy()
            .timeout(std::time::Duration::from_secs(8))
            .build()
            .unwrap();
        let resp = c
            .get(format!("http://127.0.0.1:{port}/"))
            .send()
            .await
            .unwrap();
        resp.bytes().await.unwrap();
        let log = std::fs::read_to_string(d["log_path"].as_str().unwrap()).unwrap();
        let lines: Vec<_> = log
            .lines()
            .filter(|l| l.contains("format:policy") && l.contains("decision"))
            .collect();
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].contains("upstream_status") && lines[0].contains("503"),
            "{}",
            lines[0]
        );
    }
}
