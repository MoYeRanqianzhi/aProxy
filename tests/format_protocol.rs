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
    let port = free_port();
    home.stop_on_drop(port);
    let cfg = home.path().join("chain.toml");
    std::fs::write(&cfg,format!("base_url='http://127.0.0.1:{upstream}'\nlisten_addr='127.0.0.1:{port}'\n[retry]\nmax_backoff_secs=0\n{config}")).unwrap();
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
    let app = axum::Router::new().fallback(|body: String| async move {
        assert_eq!(body, "qAB");
        tokio::time::sleep(std::time::Duration::from_millis(1400)).await;
        (
            [("content-type", "text/event-stream")],
            "data: response\n\n",
        )
    });
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let stages = "'request','response','heartbeat'";
    let config = String::from("[keepalive]\ninterval_secs=1\n")
        + &layer(&home, "outer", stages, &["append", "A"])
        + &layer(&home, "inner", stages, &["append", "B"]);
    let (client, url) = running(&home, up, &config);
    let res = client
        .post(format!("{url}/stream"))
        .header("accept", "text/event-stream")
        .body("q")
        .send()
        .await
        .unwrap();
    let text = res.text().await.unwrap();
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
    let report = serde_json::json!({"report":{"summary":format!("fictional-token {}","界".repeat(520)),"actions":vec![serde_json::json!({"type":"other","label":"seen fictional-token"});35]}});
    let cfg = layer(
        &home,
        "reporter",
        "'request','retry','response'",
        &["reply", &report.to_string()],
    )
    .replace("env={", "env={ TOKEN='fictional-token',");
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
            report["actions"][0],
            serde_json::json!({"type":"other","label":"seen <secret>"})
        );
    }
    assert!(!r.to_string().contains("fictional-token"));
}
