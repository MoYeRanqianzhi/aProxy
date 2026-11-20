//! 真正通过启动命令协商协议，区分子进程不可用与静态不兼容。每个实例都有自己的 home。
mod common;
#[path = "common/manual_clock.rs"]
mod manual_clock;
use common::{TestHome, format_echo_path, free_port};
use manual_clock::Clock;
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

/// 先绑定再公布端口；监听器本身保留分配结果，不留关闭后被其他测试抢占的窗口。
async fn mock_listener() -> (u16, tokio::net::TcpListener) {
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    (listener.local_addr().unwrap().port(), listener)
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
    let (port, listener) = mock_listener().await;
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

/// retry 独立订阅就能区分两次限流和一次读到半截的实体；有 decide 时两者看到同一张头表。
#[tokio::test]
async fn retry_last_response_carries_each_upstream_head_without_the_body() {
    use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _};
    for policy in [false, true] {
        let home = TestHome::new();
        let (up, listener) = mock_listener().await;
        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        std::io::Write::write_all(&mut gzip, b"no").unwrap();
        let compressed = gzip.finish().unwrap();
        let compressed_len = compressed.len().to_string();
        let server = tokio::spawn(async move {
            for (status, wait, body) in [
                (429, 5, "no"),
                (429, 600, "no"),
                (503, 9, "x"),
                (200, 0, "ok"),
            ] {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut lines = tokio::io::BufReader::new(&mut sock).lines();
                while !lines.next_line().await.unwrap().unwrap().is_empty() {}
                let (encoding, len, payload) = if wait == 5 {
                    (
                        "Content-Encoding: gzip\r\n",
                        compressed.len(),
                        compressed.as_slice(),
                    )
                } else {
                    ("", 2, body.as_bytes())
                };
                let reply = format!(
                    "HTTP/1.1 {status} Test\r\nConnection: close\r\nRetry-After: {wait}\r\nX-Multi: first\r\nX-Multi: second\r\n{encoding}Content-Length: {len}\r\n\r\n"
                );
                sock.write_all(reply.as_bytes()).await.unwrap();
                sock.write_all(payload).await.unwrap();
                sock.shutdown().await.unwrap();
            }
        });
        let mut cfg = layer(&home, "keys", "'retry'", &["echo"]);
        if policy {
            cfg += &layer(
                &home,
                "policy",
                "'decide'",
                &["reply", r#"{"decision":"retry","reroute":true}"#],
            );
        }
        let (c, url) = running(&home, up, &cfg);
        assert_eq!(c.get(url).send().await.unwrap().text().await.unwrap(), "ok");
        server.await.unwrap();
        let input = frames(&home, "keys");
        assert_eq!(input.len(), 4);
        for (i, (status, wait)) in [(429, "5"), (429, "600"), (503, "9")]
            .into_iter()
            .enumerate()
        {
            let retry = &input[i + 1]["retry"];
            let head = &retry["last_response"];
            assert_eq!(head["status"], status, "{retry}");
            assert_eq!(head.as_object().unwrap().len(), 2);
            assert_eq!(head["headers"]["retry-after"], wait);
            assert_eq!(head["headers"]["x-multi"], "first");
            if i == 0 {
                assert_eq!(head["headers"]["content-encoding"], "gzip");
                assert_eq!(head["headers"]["content-length"], compressed_len);
                if policy {
                    let decide = &frames(&home, "policy")[i + 1]["response"];
                    assert!(decide["headers"].get("content-encoding").is_none());
                    assert_eq!(decide["body"], "no");
                }
            } else if policy {
                let mut expected = head["headers"].clone();
                assert_eq!(
                    expected
                        .as_object_mut()
                        .unwrap()
                        .remove("content-length")
                        .unwrap(),
                    "2"
                );
                assert_eq!(
                    expected,
                    frames(&home, "policy")[i + 1]["response"]["headers"]
                );
            }
        }
        assert_eq!(
            input[3]["retry"]["last_failure"]["code"],
            "upstream_body_interrupted"
        );
    }
}

/// 相邻尝试分别收到头、在链内失败、连接失败；无头的尝试不能继承更早的响应。
#[tokio::test]
async fn retry_last_response_omitted_after_chain_and_connect_failures() {
    let home = TestHome::new();
    let (up, _) = upstream(&[429, 200]).await;
    let port = free_port();
    let reserved = tokio::net::TcpSocket::new_v4().unwrap();
    reserved
        .bind((std::net::Ipv4Addr::LOCALHOST, port).into())
        .unwrap();
    let script = home.path().join("steps.json");
    std::fs::write(
        &script,
        serde_json::json!({
            "retry:2":{"fail":{"retry":true,"reason":"no dispatch"},"overrides":{"retry":{"max_attempts":3}}},
            "retry:3":{"request":{"url":format!("http://127.0.0.1:{port}/")}}
        })
        .to_string(),
    )
    .unwrap();
    let cfg = layer(
        &home,
        "keys",
        "'retry'",
        &["reply-by-stage", script.to_str().unwrap()],
    );
    let (c, url) = running(&home, up, &cfg);
    assert_eq!(c.get(url).send().await.unwrap().status(), 200);
    let input = frames(&home, "keys");
    assert_eq!(input.len(), 4);
    assert_eq!(input[1]["retry"]["last_response"]["status"], 429);
    for (frame, code) in input[2..]
        .iter()
        .zip(["format_fail", "upstream_connect_failed"])
    {
        assert_eq!(frame["retry"]["last_failure"]["code"], code);
        assert!(
            !frame["retry"]
                .as_object()
                .unwrap()
                .contains_key("last_response")
        );
    }
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

/// 渠道 A 的首字节时限不属于下一轮；B 不给值时恢复实例或未重跑前缀的时限。
#[tokio::test]
async fn rerouting_restores_timeout_before_channel_b_omits_it() {
    use std::sync::atomic::Ordering;
    for prefix in [false, true] {
        let home = TestHome::new();
        let (a, _) = upstream(&[503]).await;
        let (b, release, busy) = held_upstream(&[200], "wrong timeout").await;
        let (c, finished) = upstream(&[200]).await;
        let script = home.path().join("route.json");
        std::fs::write(&script, serde_json::json!({
            "request":{"request":{"url":format!("http://127.0.0.1:{a}/")},"overrides":{"timeouts":{"first_byte_secs":40}}},
            "retry:2":{"request":{"url":format!("http://127.0.0.1:{b}/")}},
            "retry:3":{"request":{"url":format!("http://127.0.0.1:{c}/")}}
        }).to_string()).unwrap();
        let mut cfg = format!("timeouts.first_byte_secs={}\n", if prefix { 80 } else { 4 });
        if prefix {
            cfg += &layer(
                &home,
                "prefix",
                "'request'",
                &[
                    "reply",
                    r#"{"overrides":{"timeouts":{"first_byte_secs":4}}}"#,
                ],
            );
        }
        cfg += &layer(
            &home,
            "route",
            "'request','retry'",
            &["reply-by-stage", script.to_str().unwrap()],
        );
        let (client, url, mut clock) = Clock::start(&home, a, &cfg);
        let response = tokio::spawn(client.get(url).send());
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while busy.load(Ordering::Relaxed) == 0 {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        clock.advance(5);
        release.add_permits(1);
        assert_eq!(
            response.await.unwrap().unwrap().text().await.unwrap(),
            "upstream bytes",
            "prefix={prefix}"
        );
        assert_eq!(finished.load(Ordering::Relaxed), 1);
        assert_eq!(
            frames(&home, "route")[3]["retry"]["last_failure"]["code"],
            "upstream_first_byte_timeout"
        );
    }
}
/// 第一层能在静默请求上开启保活；另一层慢到已提交后才返回，不能把流改用另一种心跳。
#[tokio::test]
async fn keepalive_overrides_observe_the_commit_between_layers() {
    let home = TestHome::new();
    let (up, _) = upstream(&[200]).await;
    let slow = home.path().join("slow.release");
    let wait = home.path().join("wait.release");
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
        + &layer(
            &home,
            "slow",
            "'request'",
            &["gate-request", slow.to_str().unwrap()],
        )
        + &layer(
            &home,
            "late",
            "'request'",
            &[
                "reply",
                r#"{"overrides":{"keepalive":{"heartbeat":": late\n\n"}}}"#,
            ],
        )
        + &layer(
            &home,
            "wait",
            "'request'",
            &["gate-request", wait.to_str().unwrap()],
        );
    let (client, url, mut clock) = Clock::start(&home, up, &cfg);
    let sending = tokio::spawn(client.post(url).body("blocked").send());
    observed_stage(&home, "slow", "request", 1).await;
    clock.advance(1);
    let mut res = sending.await.unwrap().unwrap();
    assert_eq!(res.headers()["content-type"], "text/event-stream");
    let mut bytes = Vec::new();
    while !bytes.ends_with(b": first\n\n") {
        bytes.extend_from_slice(&res.chunk().await.unwrap().unwrap());
    }
    std::fs::write(slow, b"ok").unwrap();
    observed_stage(&home, "wait", "request", 1).await;
    clock.advance(1);
    let before = bytes.len();
    while bytes.len() == before {
        bytes.extend_from_slice(&res.chunk().await.unwrap().unwrap());
    }
    std::fs::write(wait, b"ok").unwrap();
    bytes.extend_from_slice(&res.bytes().await.unwrap());
    let body = String::from_utf8(bytes).unwrap();
    assert!(body.matches(": first\n\n").count() >= 2, "{body}");
    assert!(!body.contains(": late"));
}

/// 两个程序逐层追加自己的名字。核对收到的字节而非日志先后，证明真实的数据依赖。
#[tokio::test]
async fn chain_runs_request_in_order_and_response_and_heartbeat_in_reverse() {
    let home = TestHome::new();
    let (up, listener) = mock_listener().await;
    let release = std::sync::Arc::new(tokio::sync::Semaphore::new(0));
    let gate = release.clone();
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = calls.clone();
    let app = axum::Router::new().fallback(move |body: String| {
        count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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
    let (client, url, mut clock) = Clock::start(&home, up, &config);
    let mut res = clock
        .first_head(
            client
                .post(format!("{url}/stream"))
                .header("accept", "text/event-stream")
                .body("q"),
            &calls,
            1,
        )
        .await;
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
    let (c, url, mut clock) = Clock::start(&home, up, &cfg);
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
    clock.advance(1);
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
    let (up, socket) = mock_listener().await;
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

/// 真实程序能在合法 JSON 帧内给出超出 HTTP 容量的头表；拒绝回信，不能让请求任务 panic。
#[tokio::test]
async fn oversized_worker_header_table_is_invalid_output() {
    let home = TestHome::new();
    let (up, calls) = upstream(&[200]).await;
    let cfg = layer(&home, "many", "'request'", &["oversized-headers"]) + "breaker_failures=1\n";
    let (client, url) = running(&home, up, &cfg);
    let resp = client.get(&url).send().await.unwrap();
    assert_eq!(resp.status(), 503);
    assert_eq!(resp.headers()["x-aproxy-error"], "format_unavailable");
    assert_eq!(
        breaker(&home, &url).await["last_failure"]["details"]["kind"],
        "invalid_output"
    );
    assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 0);
}

/// 持久进程自己正常退出时，日志所有者先排空 stderr；下一条请求握手的是另一个进程。
#[tokio::test]
async fn persistent_idle_exit_drains_its_final_stderr_and_replaces_the_worker() {
    let home = TestHome::new();
    let (up, _) = upstream(&[200]).await;
    let cfg = layer(&home, "idle", "'request'", &["die-idle", "300"]).replace(
        "env={",
        &format!(
            "env={{ ECHO_OBSERVE={},",
            serde_json::to_string(&home.path().join("obs")).unwrap()
        ),
    );
    let (client, url) = running(&home, up, &cfg);
    assert_eq!(client.get(&url).send().await.unwrap().status(), 200);
    let pid = frames(&home, "obs")
        .into_iter()
        .find(|v| v["event"] == "request")
        .unwrap()["pid"]
        .as_u64()
        .unwrap() as u32;
    let ping =
        aproxy::daemon::ipc_ping_in(&home.path().join("run"), url.rsplit(':').next().unwrap())
            .await
            .unwrap();
    let log = ping.instance.log_path.unwrap();
    let diagnostic = format!("format[idle] final idle diagnostic pid={pid}");
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while aproxy::process::process_exited(pid) != Some(true)
            || !std::fs::read_to_string(&log).unwrap().contains(&diagnostic)
        {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("old worker must exit with its final diagnostic");
    assert_eq!(client.get(&url).send().await.unwrap().status(), 200);
    let requests: Vec<_> = frames(&home, "obs")
        .into_iter()
        .filter(|v| v["event"] == "request")
        .collect();
    assert_eq!(requests.len(), 2);
    assert_ne!(requests[0]["pid"], requests[1]["pid"]);
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
    let out = home
        .cmd()
        .args(["restart", &key, "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8(out.stderr)
            .unwrap()
            .contains("is still starting")
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
    assert_eq!(
        original.status.code(),
        Some(1),
        "start stderr: {}",
        String::from_utf8_lossy(&original.stderr)
    );
    assert_eq!(
        observer.status.code(),
        Some(1),
        "observer stderr: {}",
        String::from_utf8_lossy(&observer.stderr)
    );
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
        let (up, listener) = mock_listener().await;
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
        let gate = home.path().join("release");
        let cfg = String::from(
            "[keepalive]\ninterval_secs=1\ntrigger='accept'\n[override_headers]\naccept='text/plain'\n",
        ) + &layer(
            &home,
            "wait",
            "'request'",
            &["gate-request", gate.to_str().unwrap()],
        ) + &layer(&home, "stop", "'request'", &["reply", &json.to_string()]);
        let (port, out) = launch(&home, up, &cfg);
        assert!(out.status.success(), "{out:?}");
        let start: Value = serde_json::from_slice(&out.stdout).unwrap();
        let url = format!("http://127.0.0.1:{port}");
        let mut res = reqwest::Client::builder()
            .no_proxy()
            .timeout(std::time::Duration::from_secs(6))
            .build()
            .unwrap()
            .post(&url)
            .body("blocked")
            .header("accept", "text/event-stream")
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
        let first = res.chunk().await.unwrap().unwrap();
        assert!(first.starts_with(b":"));
        std::fs::write(gate, b"ok").unwrap();
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
/// 先等客户端实际收到保活头，再让拒绝程序发送压缩 SSE 或解码后超限的 JSON。
#[tokio::test]
async fn committed_request_refusal_decodes_sse_and_bounds_json() {
    use base64::Engine as _;
    use std::io::Write as _;
    for mime in ["text/event-stream", "application/json"] {
        let home = TestHome::new();
        let (up, calls) = upstream(&[200]).await;
        let gate = home.path().join("allow");
        let plain = if mime == "application/json" {
            format!("\"{}\"", "x".repeat(1024 * 1024))
        } else {
            "event: denied\ndata: x\n\n".into()
        };
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
        gz.write_all(plain.as_bytes()).unwrap();
        let body = base64::engine::general_purpose::STANDARD.encode(gz.finish().unwrap());
        let reply = serde_json::json!({"fail":{"retry":false,"reason":"denied","response":{"status":422,"headers":{"content-type":mime,"content-encoding":"gzip"},"body_b64":body}}});
        let cfg = String::from("[keepalive]\ntrigger='accept'\ninterval_secs=1\n")
            + &layer(
                &home,
                "hold",
                "'request'",
                &["gate-request", gate.to_str().unwrap()],
            )
            + "timeout_secs=0\n"
            + &layer(&home, "stop", "'request'", &["reply", &reply.to_string()]);
        let (client, url) = running(&home, up, &cfg);
        let response = client
            .post(url)
            .header("accept", "text/event-stream")
            .body("blocked")
            .send()
            .await
            .unwrap();
        std::fs::write(gate, b"").unwrap();
        let b = response.bytes().await.unwrap();
        if mime == "application/json" {
            assert!(b.len() < 4096 && String::from_utf8_lossy(&b).contains("format_rejected"));
        } else {
            assert!(b.ends_with(plain.as_bytes()));
        }
        assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 0);
    }
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
    let (port, listener) = mock_listener().await;
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
        let (up, release, calls) = held_upstream(&[200], body).await;
        let cfg = String::from("[keepalive]\ninterval_secs=3\ntrigger='accept'\n")
            + &layer(
                &home,
                "shared",
                "'heartbeat','response'",
                &["held-beat", home.path().join("never").to_str().unwrap()],
            )
            + "pool_max=1\ntimeout_secs=0\n";
        let (client, url, mut clock) = Clock::start(&home, up, &cfg);
        let res = clock
            .first_head(
                client.post(&url).header("accept", "text/event-stream"),
                &calls,
                3,
            )
            .await;
        observed_stage(&home, "shared", "heartbeat", 1).await;
        release.add_permits(1);
        let text = tokio::time::timeout(std::time::Duration::from_secs(15), res.text())
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
    let (up, release, calls) = held_upstream(&[200], "data: done\n\n").await;
    let cfg = String::from("[keepalive]\ninterval_secs=1\n")
        + &layer(&home, "outer", "'heartbeat'", &["append", "outer"])
        + &layer(&home, "fault", "'heartbeat'", &["bad-beat"])
        + "breaker_failures=1\nbreaker_cooldown_secs=30\n"
        + &layer(&home, "inner", "'heartbeat'", &["alternate-beat"]);
    let (client, url, mut clock) = Clock::start(&home, up, &cfg);
    let mut res = clock
        .first_head(
            client
                .get(format!("{url}/beats"))
                .header("accept", "text/event-stream"),
            &calls,
            1,
        )
        .await;
    let mut text = String::new();
    for (i, expected) in [": keepalive\n\n: outer\n\n", ": second\n\n: outer\n\n"]
        .into_iter()
        .enumerate()
    {
        if i != 0 {
            clock.advance(1);
        }
        text.clear();
        while !text.ends_with(expected) {
            text.push_str(std::str::from_utf8(&res.chunk().await.unwrap().unwrap()).unwrap());
        }
        assert_eq!(text, expected);
    }
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

/// 一次性进程已经关闭 stdout，首拍不能为等待它的退出而退回固定字节。
#[tokio::test]
async fn spawn_heartbeat_is_delivered_while_its_worker_is_still_cleaning_up() {
    let home = TestHome::new();
    let (up, release, calls) = held_upstream(&[200], "data: done\n\n").await;
    let gate = home.path().join("cleanup");
    let cfg = String::from("[keepalive]\ninterval_secs=1\n")
        + &layer(
            &home,
            "beat",
            "'heartbeat'",
            &["cleanup-beat", gate.to_str().unwrap()],
        )
        .replace("process='persistent'", "process='spawn'");
    let (client, url, mut clock) = Clock::start(&home, up, &cfg);
    let mut response = clock
        .first_head(
            client.get(&url).header("accept", "text/event-stream"),
            &calls,
            1,
        )
        .await;
    let first = response.chunk().await.unwrap().unwrap();
    std::fs::write(&gate, b"").unwrap();
    release.add_permits(1);
    assert_eq!(&first[..], b": timely spawn\n\n");
    assert!(response.text().await.unwrap().ends_with("data: done\n\n"));
}

/// 慢外层错过截止时，只能写已及时返回的内层字节；上游不会等迟到的回信。
#[tokio::test]
async fn missed_beat_deadline_preserves_last_timely_fragment() {
    let home = TestHome::new();
    let (up, release, calls) = held_upstream(&[200], "data: done\n\n").await;
    let cfg = String::from("[keepalive]\ninterval_secs=1\n")
        + &layer(
            &home,
            "slow",
            "'heartbeat'",
            &["held-beat", home.path().join("hold").to_str().unwrap()],
        )
        + &layer(&home, "inner", "'heartbeat'", &["append", "timely"]);
    let (client, url, mut clock) = Clock::start(&home, up, &cfg);
    let mut res = clock
        .first_head(
            client.get(&url).header("accept", "text/event-stream"),
            &calls,
            1,
        )
        .await;
    observed_stage(&home, "slow", "heartbeat", 1).await;
    clock.advance(1);
    let first = tokio::time::timeout(std::time::Duration::from_secs(15), res.chunk())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(first.as_ref(), b": keepalive\n\n: timely\n\n");
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
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
        let (c, url, mut clock) = Clock::start(&home, up, &cfg);
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
            clock.advance(2);
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
    let (port, listener) = mock_listener().await;
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
    let (up, listener) = mock_listener().await;
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
    let (up, listener) = mock_listener().await;
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
        serde_json::json!({"decision":"stop","by":"rule:1","delay_ms":0})
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
    let (c, url, mut clock) = if enabled {
        let (c, url, clock) = Clock::start(&home, up, &cfg);
        (c, url, Some(clock))
    } else {
        let (c, url) = running(&home, up, &cfg);
        (c, url, None)
    };
    if let Some(clock) = clock.as_mut() {
        assert_eq!(breaker(&home, &url).await["state"], "open");
        clock.advance(1);
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
    let (up, release, calls) = held_upstream(&[200], "data: done\n\n").await;
    let gate = home.path().join("gate");
    let cfg = String::from("[keepalive]\ninterval_secs=1\n")
        + &layer(&home, "beat", "'heartbeat'", &["append", "chosen"])
        + &layer(
            &home,
            "slow",
            "'response'",
            &["gate-response", gate.to_str().unwrap()],
        );
    let (c, url, mut clock) = Clock::start(&home, up, &cfg);
    let mut res = clock
        .first_head(
            c.get(format!("{url}/hold"))
                .header("accept", "text/event-stream"),
            &calls,
            1,
        )
        .await;
    let first = res.chunk().await.unwrap().unwrap();
    assert!(first.ends_with(b": chosen\n\n"));
    release.add_permits(1);
    observed_stage(&home, "slow", "response", 1).await;
    let before = frames(&home, "beat").len();
    clock.advance(1);
    assert_eq!(res.chunk().await.unwrap().unwrap(), first);
    clock.advance(1);
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
    let (up, listener) = mock_listener().await;
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
    let (c, url, _clock) = Clock::start(&home, up, &cfg);
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
    let (up, listener) = mock_listener().await;
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
    let (up, listener) = mock_listener().await;
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
    let (up, listener) = mock_listener().await;
    const MIME: &str = r#"application/json; profile="text/event-stream""#;
    let app = axum::Router::new()
        .fallback(|| async { ([("content-type", MIME)], r#"{"accepted":true}"#) });
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
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
    let (c, url, mut clock) = Clock::start(&home, up, &cfg);
    let client = tokio::spawn(c.get(url).header("accept", "text/event-stream").send());
    observed_stage(&home, "policy", "decide", 1).await;
    // 咨询已写出且上游返回的内容确定；明确推进两拍再放行，排除初始请求本身耗时的影响。
    clock.advance(2);
    std::fs::write(gate, b"").unwrap();
    let response = client.await.unwrap().unwrap();
    assert_eq!(response.headers()["content-type"], MIME);
    assert_eq!(response.text().await.unwrap(), r#"{"accepted":true}"#);
}

/// 慢 SSE 的首个尝试随后被否决，它的 Cookie 和其他响应头不能先抵达客户端。
#[tokio::test]
async fn pending_success_vote_uses_neutral_headers_for_sse() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let home = TestHome::new();
    let (up, listener) = mock_listener().await;
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
        let (up, listener) = mock_listener().await;
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
    let (up, listener) = mock_listener().await;
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

/// JSON 恰好 1 MiB 时，压缩帧的开销可能让传输长度大于上限；上限只约束解码后的单值。
#[tokio::test]
async fn committed_stop_json_limit_measures_decoded_bytes() {
    use std::io::Write as _;
    let json = format!("\"{}\"", "x".repeat(1024 * 1024 - 2));
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::none());
    gz.write_all(json.as_bytes()).unwrap();
    let encoded = gz.finish().unwrap();
    assert!(encoded.len() > 1024 * 1024);
    let body = committed_stop_body("application/json", "gzip", encoded).await;
    assert!(body.ends_with(format!("event: error\ndata: {json}\n\n").as_bytes()));
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
    use std::time::Duration;
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
    let (_, url, mut clock) = Clock::start(&home, free_port(), &cfg);
    // 保持在途请求直到实例退出，客户端自己的超时不能先触发正常排空。
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(120))
        .build()
        .unwrap();
    let port = reqwest::Url::parse(&url).unwrap().port().unwrap();
    let request = tokio::spawn(client.get(&url).send());
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
    let pid = tokio::time::timeout(Duration::from_secs(30), async {
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
    let (_, mut events) =
        aproxy::daemon::ipc_subscribe_in(&home.path().join("run"), &port.to_string())
            .await
            .unwrap();
    let stopping = tokio::task::spawn_blocking(move || stop.output().unwrap());
    tokio::time::timeout(Duration::from_secs(30), async {
        while events.next().await.unwrap().unwrap()["event"] != "closing" {}
        // 以实例发出的排空事件为界，不把命令本身排队启动的时间当作清理延迟。
        while event("idle", "eof") != Some(idle) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("no idle EOF while the drain clock was frozen");
    // 在首个逻辑时刻就已交付 idle EOF；在途 worker 的无应答帧仍占着自己的进程。
    assert_eq!(aproxy::process::process_exited(pid), Some(false));
    clock.advance(9);
    assert_eq!(aproxy::process::process_exited(pid), Some(false));
    clock.step(1, true);
    let out = stopping.await.unwrap();
    assert!(out.status.success(), "{out:?}");
    let exited = aproxy::process::process_exited(pid);
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
    let (up, release) = split_forward_upstream().await;
    let cfg = String::from("delivery='forward'\n")
        + &layer(&home, "outer", "'stream'", &["stream-echo"])
        + &layer(&home, "inner", "'stream'", &["stream-fault"])
        + "required=false\n";
    let (client, url) = running(&home, up, &cfg);
    let mut resp = client.get(url).send().await.unwrap();
    let mut prefix = Vec::new();
    while prefix.len() < 7 {
        prefix.extend_from_slice(&resp.chunk().await.unwrap().unwrap());
    }
    assert_eq!(prefix, b"[[first");
    release.notify_one();
    assert_eq!(resp.text().await.unwrap(), "last]");
    assert_eq!(frames(&home, "outer").last().unwrap()["event"], "close");
    assert_eq!(
        frames(&home, "inner")
            .iter()
            .filter(|m| m["event"] == "data")
            .count(),
        1
    );
}

/// 从真实连接看到两层的输出后断开，检查各自独占的进程和连续序号；上游不会先发 EOF。
#[tokio::test]
async fn client_disconnect_aborts_open_streams_on_their_own_workers() {
    let home = TestHome::new();
    let (up, _last) = split_forward_upstream().await;
    let mut cfg = String::from("delivery='forward'\n");
    for name in ["outer", "inner"] {
        cfg += &layer(&home, name, "'stream'", &["stream-echo"]).replace(
            "env={",
            &format!(
                "env={{ECHO_OBSERVE={},",
                serde_json::to_string(&home.path().join(format!("{name}-pids"))).unwrap()
            ),
        );
    }
    let (client, url) = running(&home, up, &cfg);
    let mut res = client.get(url).send().await.unwrap();
    let mut prefix = Vec::new();
    while prefix.len() < 7 {
        prefix.extend_from_slice(&res.chunk().await.unwrap().unwrap());
    }
    assert_eq!(prefix, b"[[first");
    drop(res);
    for name in ["outer", "inner"] {
        let messages = tokio::time::timeout(std::time::Duration::from_secs(15), async {
            loop {
                let messages = observed_stage(&home, name, "stream", 1).await;
                if messages.last().is_some_and(|v| v["event"] == "abort") {
                    break messages;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            messages.last().unwrap()["reason"],
            serde_json::json!({"code":"client_gone"})
        );
        for (idx, frame) in messages.iter().enumerate() {
            assert_eq!(frame["seq"], idx + 1);
            assert_eq!(frame["stream_id"], messages[0]["stream_id"]);
        }
        let pids: Vec<_> = frames(&home, &format!("{name}-pids"))
            .into_iter()
            .filter(|v| v["event"] == "stream")
            .collect();
        assert_eq!(pids.len(), messages.len());
        assert!(pids.iter().all(|v| v["pid"] == pids[0]["pid"]));
    }
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
    split_forward_upstream_tail(b"last").await
}
/// 客户端上传作为首块；以指定的末块测试跨 UTF-8 边界和物理 EOF 的顺序。
async fn split_forward_upstream_tail(
    tail: &'static [u8],
) -> (u16, std::sync::Arc<tokio::sync::Notify>) {
    use futures_util::StreamExt as _;
    let (port, listener) = mock_listener().await;
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
                    Ok(bytes::Bytes::from_static(tail))
                }));
            axum::body::Body::from_stream(chunks)
        }
    });
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (port, resume)
}
/// 字符跨两个网络块，末尾还剩不完整的 UTF-8。空回信不复制输入，真正的 EOF 先清 carry 再 close。
#[tokio::test]
async fn stream_eof_flushes_binary_carry_after_an_empty_reply() {
    let home = TestHome::new();
    let (up, last) = split_forward_upstream_tail(b"\x98\x80\xe2").await;
    let cfg = String::from("delivery='forward'\n")
        + &layer(&home, "wire", "'stream'", &["stream-skip-x"]);
    let (client, url) = running(&home, up, &cfg);
    let response = client
        .post(url)
        .body(b"X\xf0\x9f".as_slice())
        .send()
        .await
        .unwrap();
    let data = observed_stage(&home, "wire", "stream", 2).await;
    assert_eq!(data[1]["data"], "X");
    last.notify_one();
    assert_eq!(
        &response.bytes().await.unwrap()[..],
        b"[\xf0\x9f\x98\x80\xe2]"
    );
    let wire = observed_stage(&home, "wire", "stream", 5).await;
    assert_eq!(wire.len(), 5);
    assert_eq!(wire[2]["data"], "😀");
    assert!(wire[2].get("data_b64").is_none());
    assert_eq!(wire[3]["data_b64"], "4g==");
    assert!(wire[3].get("data").is_none());
    assert_eq!(wire[4]["event"], "close");
}

/// 第一次 open 的进程故障要在不同进程重试；打开的门不拉起进程，已经在门内的流还能继续。
#[tokio::test]
async fn stream_open_retries_fresh_but_an_open_breaker_only_preserves_existing_leases() {
    for fresh in [true, false] {
        let home = TestHome::new();
        let flag = home.path().join("once");
        let cfg = String::from("delivery='forward'\n")
            + &layer(
                &home,
                "wire",
                "'stream'",
                &["stream-admit", flag.to_str().unwrap()],
            )
            .replace(
                "env={",
                &format!(
                    "env={{ECHO_OBSERVE={},",
                    serde_json::to_string(&home.path().join("pids")).unwrap()
                ),
            )
            + &format!(
                "breaker_failures={}\nbreaker_cooldown_secs=86400\n",
                if fresh { 2 } else { 1 }
            );
        let (up, last) = split_forward_upstream().await;
        let (client, url) = running(&home, up, &cfg);
        let url = url.trim_end_matches('/');
        if fresh {
            last.notify_one();
            assert_eq!(
                client
                    .get(format!("{url}/fresh"))
                    .send()
                    .await
                    .unwrap()
                    .text()
                    .await
                    .unwrap(),
                "[firstlast]"
            );
            let pids: Vec<_> = frames(&home, "pids")
                .into_iter()
                .filter(|p| p["event"] == "stream")
                .collect();
            assert_eq!(pids[0]["seq"], 1);
            assert_eq!(pids[1]["seq"], 1);
            assert_ne!(pids[0]["pid"], pids[1]["pid"]);
        } else {
            let mut live = client.get(format!("{url}/live")).send().await.unwrap();
            let mut first = Vec::new();
            while first.len() < 6 {
                first.extend_from_slice(&live.chunk().await.unwrap().unwrap())
            }
            assert_eq!(first, b"[first");
            // 另外两个请求只取响应头和第一块，第二块仍由各自的上游等待者持有。
            for path in ["trip", "refused"] {
                let mut plain = client.get(format!("{url}/{path}")).send().await.unwrap();
                let mut prefix = Vec::new();
                while prefix.len() < 5 {
                    prefix.extend_from_slice(&plain.chunk().await.unwrap().unwrap());
                }
                assert_eq!(prefix, b"first");
            }
            let before = frames(&home, "wire");
            assert_eq!(before.iter().filter(|v| v["stage"] == "hello").count(), 2);
            assert_eq!(before.iter().filter(|v| v["event"] == "open").count(), 2);
            assert_eq!(breaker(&home, url).await["state"], "open");
            last.notify_waiters();
            assert_eq!(live.bytes().await.unwrap(), "last]");
            let wire = frames(&home, "wire");
            let close = wire.last().unwrap();
            assert_eq!(close["event"], "close");
            assert_eq!(close["stream_id"], before[1]["stream_id"]);
            let pids: Vec<_> = frames(&home, "pids")
                .into_iter()
                .filter(|v| v["event"] == "stream")
                .collect();
            assert_eq!(pids.last().unwrap()["pid"], pids[0]["pid"]);
            assert_eq!(breaker(&home, url).await["state"], "open");
        }
    }
}
/// 真正启动的实例说明按阶段缓冲，不会承诺 forward 下磁盘缓存和响应限额失效。
#[test]
fn forward_startup_warns_about_format_buffering() {
    let home = TestHome::new();
    let cfg =
        String::from("delivery='forward'\n") + &layer(&home, "buffer", "'response'", &["echo"]);
    let (_, out) = launch(&home, free_port(), &cfg);
    assert!(out.status.success(), "{out:?}");
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    let log = std::fs::read_to_string(doc["log_path"].as_str().unwrap()).unwrap();
    assert!(
        log.contains("response programs and failure decisions can buffer responses"),
        "{log}"
    );
    assert!(log.contains("Disk cache and size limits apply to buffered bodies"));
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
/// 客户端在已观测的 data 往返中断开：未交付的上游前缀仍计入该次尝试，完成通知不能先于它。
#[tokio::test]
async fn forward_disconnect_during_data_preserves_upstream_counts_before_notify() {
    let home = TestHome::new();
    let (up, _resume) = split_forward_upstream().await;
    let gate = home.path().join("release");
    let cfg = String::from("delivery='forward'\n")
        + &layer(
            &home,
            "stream",
            "'stream'",
            &["stream-gate", gate.to_str().unwrap()],
        )
        + &layer(&home, "notice", "'notify'", &["echo"]);
    let (client, url) = running(&home, up, &cfg);
    let mut resp = client.get(&url).send().await.unwrap();
    assert_eq!(resp.chunk().await.unwrap().unwrap(), "[");
    let data = observed_stage(&home, "stream", "stream", 2).await;
    assert_eq!(data[1]["data"], "first");
    drop(resp);
    let notice = observed_stage(&home, "notice", "notify", 1).await;
    let rec =
        &request_detail(&home, &url, notice[0]["request_id"].as_str().unwrap()).await["record"];
    assert_eq!(rec["tries"][0]["response_bytes"], 5);
    assert!(rec["tries"][0]["duration_ms"].is_number());
    assert_eq!(rec["response_bytes"], 1);
    assert_eq!(notice[0]["outcome"], "client_gone");
    assert_eq!(notice[0]["stream_completed"], false);
    std::fs::write(gate, b"done").unwrap();
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
    let (up, listener) = mock_listener().await;
    let first = std::sync::Arc::new(tokio::sync::Notify::new());
    let observed = first.clone();
    let app = axum::Router::new().fallback(move |req: axum::extract::Request| {
        let first = first.clone();
        async move {
            let mut f = req.into_body().into_data_stream();
            // HTTP 不保留应用的分块边界；先确认前缀，再读到真正 EOF，免得早关上传吞掉响应。
            let mut received = Vec::new();
            while let Some(chunk) = f.next().await {
                received.extend_from_slice(&chunk.unwrap());
                if received == b"one" {
                    first.notify_one();
                }
            }
            assert_eq!(received, b"onetwo");
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
    let (up, listener) = mock_listener().await;
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
        let (up, listener) = mock_listener().await;
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
        assert!(
            h.get("truncated").is_none(),
            "head alone does not describe a body: {h}"
        );
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
                &["reply", r#"{"overrides":{"retry":{"max_attempts":4}}}"#],
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
            assert_eq!(f[4]["attempt"], 4);
            assert_eq!(
                f[4]["fallback"],
                serde_json::json!({"decision":"stop","by":"override","delay_ms":5000})
            );
            assert_eq!(
                f[6]["fallback"],
                serde_json::json!({"decision":"stop","by":"override","delay_ms":20000})
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

/// 先收完整次上传，再回一次 503 和之后的 200；不让半途关连接掩盖真实的管道字节数。
async fn digest_upstream(expected: usize) -> (u16, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    use futures_util::StreamExt as _;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let app = axum::Router::new().fallback(move |r: axum::extract::Request| {
        let count = count.clone();
        async move {
            let mut bytes = 0;
            let mut body = r.into_body().into_data_stream();
            while let Some(chunk) = body.next().await {
                bytes += chunk.unwrap().len();
            }
            assert_eq!(bytes, expected);
            if count.fetch_add(1, Ordering::Relaxed) == 0 {
                http::StatusCode::SERVICE_UNAVAILABLE
            } else {
                http::StatusCode::OK
            }
        }
    });
    let (port, listener) = mock_listener().await;
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (port, calls)
}

/// 请求和重试共用原始字节的摘要；以程序实际读到的 JSONL 字节数检查正文没有走入管道。
#[tokio::test]
async fn digest_program_gets_the_right_sha256_and_no_body_on_its_pipe() {
    let home = TestHome::new();
    let (up, calls) = digest_upstream(4 * 1024 * 1024).await;
    let cfg = layer(
        &home,
        "digest",
        "'request','retry','decide'",
        &["digest-only"],
    );
    let (client, url) = running(&home, up, &cfg);
    let response = client
        .post(url)
        .body(vec![b'x'; 4 * 1024 * 1024])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 2);
    let line_bytes = std::fs::metadata(home.path().join("digest")).unwrap().len();
    eprintln!("digest worker stdin: {line_bytes} bytes");
    assert!(
        line_bytes < 8192,
        "four messages should carry only metadata, got {line_bytes} bytes"
    );
    let seen = frames(&home, "digest");
    assert_eq!(
        seen.iter()
            .map(|f| f["stage"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["hello", "request", "decide", "retry"]
    );
    for frame in &seen[1..] {
        assert_eq!(frame["client"]["stream"], false);
        assert_eq!(
            frame["digest"],
            "baa7a6d36ffa957552df230235c2d51d735f28d49c58a5f3438a3a973a25a37d"
        );
        for key in ["body", "body_b64"] {
            assert!(frame["request"].get(key).is_none());
        }
    }
}

/// 即使前一层已解码并改写请求，后面看到的摘要仍以压缩上传为准，不随 retry 改变。
#[tokio::test]
async fn digest_uses_original_compressed_bytes_before_request_rewrites() {
    use std::io::Write as _;
    let home = TestHome::new();
    let (up, _) = digest_upstream(b"replacement".len()).await;
    let cfg = layer(
        &home,
        "rewrite",
        "'request'",
        &["reply", r#"{"request":{"body":"replacement"}}"#],
    ) + &layer(&home, "d", "'request','retry','decide'", &["digest-only"])
        + &layer(&home, "last", "'request','retry'", &["inspect"]);
    let (c, url) = running(&home, up, &cfg);
    let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gzip.write_all(b"original").unwrap();
    let compressed = gzip.finish().unwrap();
    let hash = <sha2::Sha256 as sha2::Digest>::digest(&compressed)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    assert_eq!(
        c.post(url)
            .header("content-encoding", "gzip")
            .body(compressed)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(frames(&home, "rewrite")[1]["request"]["body"], "original");
    let messages = frames(&home, "d");
    assert_eq!(messages.len(), 4);
    for f in &messages[1..] {
        assert_eq!(f["digest"], hash);
        assert!(f["request"].get("body").is_none() && f["request"].get("body_b64").is_none());
    }
    for f in &frames(&home, "last")[1..] {
        assert_eq!(f["request"]["body"], "replacement");
        assert!(f.get("digest").is_none());
    }
}

/// 客户端在上游确认前缀以后才发尾部；在同一请求上核对 request 与 response 所见的标记。
#[tokio::test]
async fn metadata_only_forward_request_keeps_the_upload_live() {
    use futures_util::StreamExt as _;
    let first = std::sync::Arc::new(tokio::sync::Notify::new());
    let observed = first.clone();
    const PREFIX: &str = r#"{"stream":"#;
    let app = axum::Router::new().fallback(move |req: axum::extract::Request| {
        let first = first.clone();
        async move {
            let mut body = req.into_body().into_data_stream();
            let mut received = Vec::new();
            while let Some(chunk) = body.next().await {
                received.extend_from_slice(&chunk.unwrap());
                if received == PREFIX.as_bytes() {
                    first.notify_one();
                }
            }
            let doc: Value = serde_json::from_slice(&received).unwrap();
            doc["stream"].to_string()
        }
    });
    let (port, listener) = mock_listener().await;
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    for (trigger, accept, head) in [
        ("body_stream", "application/json", None),
        ("accept", "application/json", Some(false)),
        ("any", "text/event-stream", Some(true)),
    ] {
        let home = TestHome::new();
        let cfg = format!("delivery='forward'\n[keepalive]\ntrigger='{trigger}'\n")
            + &layer(&home, "meta", "'request','response'", &["metadata-only"]);
        let (client, url) = running(&home, port, &cfg);
        for (suffix, bit) in [
            ("true }".to_string(), true),
            ("false}".to_string(), false),
            (
                format!(
                    r#"false,"padding":"{}","stream":true}}"#,
                    "x".repeat(128 * 1024)
                ),
                true,
            ),
        ] {
            let ack = observed.clone();
            let upload =
                futures_util::stream::once(async { Ok::<_, std::io::Error>(PREFIX.to_owned()) })
                    .chain(futures_util::stream::once(async move {
                        ack.notified().await;
                        Ok(suffix)
                    }));
            let res = client
                .post(&url)
                .header("accept", accept)
                .body(reqwest::Body::wrap_stream(upload))
                .send()
                .await
                .unwrap();
            assert_eq!(res.status(), 200);
            assert_eq!(res.text().await.unwrap(), bit.to_string());
            let f = frames(&home, "meta");
            let req = &f[f.len() - 2];
            let response = &f[f.len() - 1];
            assert_eq!(req["stage"], "request");
            assert_eq!(req["client"]["path"], "/");
            assert_eq!(req["client"].get("stream"), head.map(Value::Bool).as_ref());
            assert!(
                req.get("digest").is_none()
                    && req["request"].get("body").is_none()
                    && req["request"].get("body_b64").is_none()
            );
            assert_eq!(response["stage"], "response");
            assert_eq!(response["client"]["stream"], head.unwrap_or(bit));
            assert_eq!(response["request_id"], req["request_id"]);
        }
    }
}

/// 选择摘要的程序不收正文，但可以在 request 阶段就依赖从完整上传读出的客户端流标记。
#[tokio::test]
async fn forward_and_format_start_together_with_digest_programs() {
    let home = TestHome::new();
    let (port, _) = upstream(&[200]).await;
    let cfg = String::from("delivery='forward'\n[keepalive]\ntrigger='body_stream'\n")
        + &layer(&home, "digest", "'request'", &["digest-only"])
        + &layer(&home, "other", "'request','stream'", &["digest-only"]);
    let (client, url) = running(&home, port, &cfg);
    for bit in [true, false] {
        let response = client
            .post(&url)
            .json(&serde_json::json!({"stream":bit}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        response.bytes().await.unwrap();
        for name in ["digest", "other"] {
            let f = frames(&home, name);
            let last = f.iter().rfind(|f| f["stage"] == "request").unwrap();
            assert_eq!(last["client"]["stream"], bit);
            assert_eq!(last["digest"].as_str().unwrap().len(), 64);
            assert!(
                last["request"].get("body").is_none() && last["request"].get("body_b64").is_none()
            );
            if name == "other" {
                let open = f.iter().rfind(|f| f["event"] == "open").unwrap();
                assert_eq!(open["client"], last["client"]);
            }
        }
    }
}

/// 双向直通：上游先对已收到的请求前缀给出确认，客户端看到响应的第一块以后才给出 JSON 尾部。
#[tokio::test]
async fn forward_stream_can_open_before_the_client_finishes_its_upload() {
    use futures_util::StreamExt as _;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let home = TestHome::new();
    const PREFIX: &[u8] = br#"{"stream":"#;
    let (port, listener) = mock_listener().await;
    let app = axum::Router::new().fallback(|req: axum::extract::Request| async move {
        let mut input = req.into_body().into_data_stream();
        let mut body = Vec::new();
        while body.len() < PREFIX.len() {
            body.extend_from_slice(&input.next().await.unwrap().unwrap());
        }
        assert_eq!(body, PREFIX);
        let start = futures_util::stream::once(async {
            Ok::<_, std::io::Error>("upstream-ack".to_owned())
        });
        let tail = futures_util::stream::once(async move {
            while let Some(c) = input.next().await {
                body.extend_from_slice(&c.unwrap());
            }
            let doc: Value = serde_json::from_slice(&body).unwrap();
            Ok(format!("received-{}", doc["stream"]))
        });
        axum::body::Body::from_stream(start.chain(tail))
    });
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let cfg = String::from("delivery='forward'\nkeepalive.trigger='body_stream'\n")
        + &layer(&home, "meta", "'request'", &["metadata-only"])
        + &layer(&home, "convert", "'stream'", &["stream-echo"]);
    let (_, url) = running(&home, port, &cfg);
    let addr = url.strip_prefix("http://").unwrap();
    for (suffix, value) in [("true }", "true"), ("false}", "false")] {
        let mut client = tokio::net::TcpStream::connect(addr).await.unwrap();
        let head = format!(
            "POST / HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            PREFIX.len() + suffix.len()
        );
        client.write_all(head.as_bytes()).await.unwrap();
        client.write_all(PREFIX).await.unwrap();
        let mut wire = Vec::new();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let mut buf = [0; 512];
            while !wire
                .windows(b"upstream-ack".len())
                .any(|w| w == b"upstream-ack")
            {
                let n = client.read(&mut buf).await.unwrap();
                assert!(n > 0, "unexpected EOF: {wire:?}");
                wire.extend_from_slice(&buf[..n]);
            }
        })
        .await
        .expect("upstream confirmation must reach the client while the upload is live");
        let stages = frames(&home, "convert");
        let open = stages.iter().rfind(|f| f["event"] == "open").unwrap();
        assert_eq!(open["client"], serde_json::json!({"path":"/"}));
        client.write_all(suffix.as_bytes()).await.unwrap();
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            client.read_to_end(&mut wire),
        )
        .await
        .unwrap()
        .unwrap();
        let wire = String::from_utf8(wire).unwrap();
        assert!(wire.contains(&format!("received-{value}")), "{wire}");
    }
}

/// 就算流层在 open 被跳过，已提交的 SSE 也只能收到经整体验证的明文，不能先泄出损坏体的半段。
#[tokio::test]
async fn committed_replay_after_skipped_open_validates_all_codings_before_output() {
    use std::io::Write as _;
    for (case, stream) in [
        ("bad-trailer", true),
        ("separate-fields", true),
        ("bad-wrapper", true),
        ("four", true),
        ("too-deep", true),
        ("four", false),
        ("too-deep", false),
    ] {
        let home = TestHome::new();
        let (port, listener) = mock_listener().await;
        let gate = std::sync::Arc::new(tokio::sync::Semaphore::new(0));
        let allow = gate.clone();
        let plain = b"data: validated-member\n\n".repeat(8192);
        let mut raw = plain.clone();
        if case == "bad-wrapper" {
            let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
            z.write_all(&raw).unwrap();
            raw = z.finish().unwrap();
        }
        let count = match case {
            "too-deep" => 5,
            "four" => 4,
            _ => 1,
        };
        for _ in 0..count {
            let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
            gz.write_all(&raw).unwrap();
            raw = gz.finish().unwrap();
        }
        if matches!(case, "bad-trailer" | "bad-wrapper") {
            let crc = raw.len() - 8;
            raw[crc] ^= 1;
        }
        let app = axum::Router::new().fallback(move || {
            let allow = allow.clone();
            let raw = raw.clone();
            async move {
                allow.acquire().await.unwrap().forget();
                let mut response =
                    axum::response::Response::builder().header("content-type", "text/event-stream");
                if case == "separate-fields" {
                    response = response.header("content-encoding", "identity");
                }
                if case == "bad-wrapper" {
                    response = response.header("content-encoding", "deflate");
                }
                for _ in 1..count {
                    response = response.header("content-encoding", "gzip");
                }
                response
                    .header("content-encoding", "gzip")
                    .body(axum::body::Body::from(raw))
                    .unwrap()
            }
        });
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut cfg = String::from("[keepalive]\ntrigger='accept'\ninterval_secs=1\n");
        if stream {
            cfg += &layer(&home, "skip", "'stream'", &["bad-seq"]);
            cfg += "breaker_failures=1\n";
        }
        let (client, url) = running(&home, port, &cfg);
        let res = client
            .get(url)
            .header("accept", "text/event-stream")
            .send()
            .await
            .unwrap();
        assert_eq!(res.headers()["content-type"], "text/event-stream");
        gate.add_permits(1);
        let body = res
            .bytes()
            .await
            .expect("must end as SSE, not a body I/O error");
        if matches!(case, "separate-fields" | "four") {
            assert!(body.ends_with(&plain));
        } else {
            let body = std::str::from_utf8(&body).unwrap();
            assert!(
                body.contains("decode_failed") && !body.contains("validated-member"),
                "{case}: not an atomic decode failure"
            );
        }
    }
}

/// 两种默认值都由第一个合法表态改变；另一层既看见已选方式，也不能把它改回去。
#[tokio::test]
async fn first_delivery_vote_wins() {
    for (default, first, expected_status, calls_expected) in
        [("retry", "forward", 503, 1), ("forward", "retry", 200, 2)]
    {
        let home = TestHome::new();
        let (up, calls) = upstream(&[503, 200]).await;
        let cfg = format!("delivery='{default}'\n")
            + &layer(
                &home,
                "first",
                "'request'",
                &["reply", &format!(r#"{{"delivery":"{first}"}}"#)],
            )
            + &layer(
                &home,
                "second",
                "'request'",
                &["reply", &format!(r#"{{"delivery":"{default}"}}"#)],
            );
        let (client, url) = running(&home, up, &cfg);
        let res = client.get(&url).send().await.unwrap();
        assert_eq!(frames(&home, "first")[1]["delivery"], default);
        assert_eq!(frames(&home, "second")[1]["delivery"], first);
        assert_eq!(res.status(), expected_status);
        assert_eq!(res.text().await.unwrap(), "upstream bytes");
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::Relaxed),
            calls_expected
        );
        let id = frames(&home, "first")[1]["request_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let record = request_detail(&home, &url, &id).await;
        assert_eq!(record["record"]["delivery"], first);
        assert_eq!(record["record"]["upgraded"], false);
    }
}

/// 503 还没有向客户端写过头；决定重试才计一次升级，下一次成功保留上游实体。
#[tokio::test]
async fn forward_503_upgrades_through_decide_and_the_client_sees_only_the_retried_success() {
    let home = TestHome::new();
    let (up, listener) = mock_listener().await;
    let bodies = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let received = bodies.clone();
    let app = axum::Router::new().fallback(move |method: http::Method, body: axum::body::Bytes| {
        let mut seen = received.lock().unwrap();
        seen.push((method, body));
        let first = seen.len() == 1;
        async move {
            if first {
                (http::StatusCode::SERVICE_UNAVAILABLE, "failed prefix")
            } else {
                (http::StatusCode::OK, "successful prefix")
            }
        }
    });
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let cfg = String::from("delivery='forward'\n")
        + &layer(&home, "prefix", "'request'", &["echo"])
        + &layer(
            &home,
            "policy",
            "'decide'",
            &["reply", r#"{"decision":"retry","delay_ms":0}"#],
        )
        + &layer(&home, "finished", "'notify'", &["echo"]);
    let (c, url) = running(&home, up, &cfg);
    let raw = &b"retry my exact bytes\x00\xff"[..];
    let resp = c.post(&url).body(raw).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    assert!(!resp.headers().contains_key("x-aproxy-error"));
    assert_eq!(resp.text().await.unwrap(), "successful prefix");
    let policy_frames = frames(&home, "policy");
    assert_eq!(policy_frames.len(), 2);
    let m = &policy_frames[1];
    assert_eq!(m["delivery"], "forward");
    assert_eq!(m["fallback"]["decision"], "stop");
    assert_eq!(m["response"]["status"], 503);
    assert_eq!(m["attempt"], 1);
    let d = request_detail(&home, &url, m["request_id"].as_str().unwrap()).await;
    let rec = &d["record"];
    assert_eq!(rec["delivery"], "retry");
    assert_eq!(rec["upgraded"], true);
    assert_eq!(rec["tries"].as_array().unwrap().len(), 2);
    assert_eq!(rec["tries"][0]["decision"]["by"], "format:policy");
    let ping =
        aproxy::daemon::ipc_ping_in(&home.path().join("run"), url.rsplit(':').next().unwrap())
            .await
            .unwrap();
    let end = &observed_stage(&home, "finished", "notify", 1).await[0];
    assert_eq!(end["request_id"], m["request_id"]);
    assert_eq!(end["delivery"], "retry");
    assert_eq!(end["upgraded"], true);
    assert_eq!(end["attempts"], 2);
    assert_eq!(end["upstream_status"], 200);
    assert_eq!(ping.activity.upgraded_total, 1);
    let seen = bodies.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert!(
        seen.iter()
            .all(|(method, b)| method == http::Method::POST && b.as_ref() == raw)
    );
    let prefix = frames(&home, "prefix");
    assert_eq!(prefix.iter().filter(|m| m["stage"] == "request").count(), 1);
    assert!(prefix.iter().all(|m| m["stage"] != "retry"));
}

/// 上游在交付前缀后才报错，客户端已经收到的字节不能撤回；不发 decide，不重试。
#[tokio::test]
async fn forward_stream_cut_mid_way_does_not_upgrade() {
    use futures_util::StreamExt as _;
    let home = TestHome::new();
    let (up, listener) = mock_listener().await;
    let release = std::sync::Arc::new(tokio::sync::Notify::new());
    let gate = release.clone();
    let app = axum::Router::new().fallback(move || {
        let gate = gate.clone();
        async move {
            axum::body::Body::from_stream(
                futures_util::stream::once(async { Ok::<_, std::io::Error>("prefix") }).chain(
                    futures_util::stream::once(async move {
                        gate.notified().await;
                        Err(std::io::Error::other("interrupted"))
                    }),
                ),
            )
        }
    });
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let cfg = String::from("delivery='forward'\n")
        + &layer(
            &home,
            "policy",
            "'request','decide'",
            &["reply", r#"{"decision":"retry"}"#],
        )
        + &layer(&home, "finished", "'notify'", &["echo"]);
    let (c, url) = running(&home, up, &cfg);
    let mut resp = c.get(&url).send().await.unwrap();
    let mut bytes = Vec::new();
    while bytes.len() < 6 {
        bytes.extend_from_slice(&resp.chunk().await.unwrap().unwrap());
    }
    assert_eq!(bytes, b"prefix");
    release.notify_one();
    assert!(resp.chunk().await.is_err());
    let messages = frames(&home, "policy");
    assert_eq!(messages.len(), 2);
    let rec = request_detail(&home, &url, messages[1]["request_id"].as_str().unwrap()).await;
    assert_eq!(rec["record"]["code"], "upstream_failed");
    assert_eq!(rec["record"]["upgraded"], false);
    let end = &observed_stage(&home, "finished", "notify", 1).await[0];
    assert_eq!(end["request_id"], messages[1]["request_id"]);
    assert_eq!(end["stream_completed"], false);
    assert_eq!(end["code"], "upstream_failed");
    assert_eq!(end["delivery"], "forward");
}

/// 压缩和大体都只旁路检查；历史预算为零也应知道伪成功，客户端则逐字节收到原表示。
#[tokio::test]
async fn forward_200_error_body_is_recorded_as_upstream_error() {
    use std::io::Write as _;
    let plain = br#"{"error":"busy"}"#;
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(plain).unwrap();
    let mut large = b"data: 0\n".repeat(128 * 1024);
    large.extend_from_slice(b"data: {\"error\":\"last\"}\n\n");
    for (encoding, bytes) in [
        ("identity", plain.to_vec()),
        ("gzip", gz.finish().unwrap()),
        ("identity", large),
    ] {
        let home = TestHome::new();
        let (up, listener) = mock_listener().await;
        let raw = bytes::Bytes::from(bytes);
        let copy = raw.clone();
        let app = axum::Router::new().fallback(move || {
            let b = copy.clone();
            async move { ([("content-encoding", encoding)], b) }
        });
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let cfg = String::from("delivery='forward'\nhistory.body_budget_mb=0\n")
            + &layer(
                &home,
                "watch",
                "'decide'",
                &["reply", r#"{"decision":"retry"}"#],
            )
            + &layer(&home, "end", "'notify'", &["echo"]);
        let (c, url) = running(&home, up, &cfg);
        let resp = c.get(&url).send().await.unwrap();
        assert_eq!(resp.status(), 200);
        assert_eq!(resp.bytes().await.unwrap(), raw);
        let out = aproxy::daemon::ipc_read_in(
            &home.path().join("run"),
            url.rsplit(':').next().unwrap(),
            &aproxy::daemon::ReadOp::Requests {
                before: None,
                limit: Some(1),
            },
        )
        .await
        .unwrap();
        let rec = &out["requests"][0];
        assert_eq!(rec["outcome"], "upstream_error", "{out}");
        assert_eq!(rec["code"], "upstream_failed");
        let end = &observed_stage(&home, "end", "notify", 1).await[0];
        assert_eq!(end["outcome"], rec["outcome"]);
        assert_eq!(end["code"], rec["code"]);
        assert_eq!(rec["tries"][0]["code"], "upstream_error_body");
        assert_eq!(frames(&home, "watch").len(), 1);
    }
}

/// 客户端已经读到心跳才给出第一票时不能收回 200；反之必须把重定向头原样交付。
/// 两次请求均启动保活；手动时钟隔开首拍与实际工作进程的投票，及时投票后仍跨过两拍。
#[tokio::test]
async fn forward_vote_on_either_side_of_commitment() {
    let home = TestHome::new();
    let (up, listener) = mock_listener().await;
    let hold = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
    let gate_up = hold.clone();
    let (sent, mut arrived) = tokio::sync::mpsc::unbounded_channel();
    let app = axum::Router::new().fallback(move |body: String| {
        let sent = sent.clone();
        let gate = gate_up.clone();
        async move {
            sent.send(()).unwrap();
            gate.acquire().await.unwrap().forget();
            assert_eq!(body, "blocked");
            (
                axum::http::StatusCode::TEMPORARY_REDIRECT,
                [("location", "/redirected")],
                "redirect body",
            )
        }
    });
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let gate = home.path().join("release-vote");
    let cfg = String::from("delivery='retry'\n[keepalive]\ntrigger='accept'\ninterval_secs=1\n")
        + &layer(
            &home,
            "wait",
            "'request'",
            &["gate-request", gate.to_str().unwrap()],
        )
        + &layer(
            &home,
            "voter",
            "'request'",
            &["reply", r#"{"delivery":"forward"}"#],
        )
        + &layer(&home, "observer", "'request'", &["reply", "{}"]);
    let (_, url, mut clock) = Clock::start(&home, up, &cfg);
    let c = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .unwrap();
    let send = || {
        tokio::spawn(
            c.post(&url)
                .header("accept", "text/event-stream")
                .body("blocked")
                .send(),
        )
    };
    let first = send();
    observed_stage(&home, "wait", "request", 1).await;
    clock.advance(1);
    let mut response = first.await.unwrap().unwrap();
    assert_eq!(response.status(), 200);
    assert!(response.headers().get("location").is_none());
    let beat = response.chunk().await.unwrap().unwrap();
    assert!(beat.starts_with(b":"), "{beat:?}");
    assert_eq!(
        frames(&home, "voter").len(),
        1,
        "vote preceded the observed heartbeat"
    );
    std::fs::write(&gate, []).unwrap();
    assert!(response.text().await.unwrap().ends_with("redirect body"));
    let observer = frames(&home, "observer");
    assert_eq!(observer[1]["delivery"], "retry");
    let record = request_detail(&home, &url, observer[1]["request_id"].as_str().unwrap()).await;
    assert_eq!(record["record"]["delivery"], "retry");
    assert_eq!(record["record"]["tries"][0]["upstream_status"], 307);
    assert_eq!(record["record"]["upgraded"], false);

    arrived.recv().await.unwrap();
    let second = send();
    tokio::time::timeout(std::time::Duration::from_secs(30), arrived.recv())
        .await
        .unwrap()
        .unwrap();
    clock.advance(2);
    hold.add_permits(1);
    let response = second.await.unwrap().unwrap();
    assert_eq!(response.status(), 307);
    assert_eq!(response.headers()["location"], "/redirected");
    assert_eq!(response.text().await.unwrap(), "redirect body");
    let observer = frames(&home, "observer");
    assert_eq!(observer[2]["delivery"], "forward");
    let record = request_detail(&home, &url, observer[2]["request_id"].as_str().unwrap()).await;
    assert_eq!(record["record"]["delivery"], "forward");
    assert_eq!(record["record"]["heartbeats"], 0);
}

/// 等观察方向公布对应的状态；数据交付不等通知，不能拿 HTTP 返回当作通知已处理的证明。
async fn notify_snapshot(home: &TestHome, url: &str, ready: impl Fn(&Value) -> bool) -> Value {
    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        loop {
            let f = aproxy::daemon::ipc_ping_in(
                &home.path().join("run"),
                url.rsplit(':').next().unwrap(),
            )
            .await
            .unwrap()
            .formats
            .remove(0);
            if ready(&f) {
                return f;
            }
            tokio::time::sleep(std::time::Duration::from_millis(15)).await;
        }
    })
    .await
    .expect("notify state did not converge")
}

/// 直通和缓冲路径都在收齐原始上传后发摘要；另一个程序只见自己的私有状态，不见摘要或正文。
#[tokio::test]
async fn completion_notify_digest_and_state_match_each_finished_request() {
    let raw = br#"{"stream":false,"value":"before any patches"}"#;
    for mode in ["retry", "forward"] {
        let home = TestHome::new();
        let (up, release) = split_forward_upstream().await;
        let cfg = format!("delivery='{mode}'\n")
            + &layer(&home, "digest", "'request','notify'", &["notify-digest"])
            + &layer(&home, "private", "'notify'", &["echo"]);
        let (client, url) = running(&home, up, &cfg);
        release.notify_one();
        let r = client
            .post(format!("{url}/sample"))
            .body(raw.as_slice())
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
        let id = observed_stage(&home, "digest", "request", 1).await[0]["request_id"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(r.bytes().await.unwrap().starts_with(raw));
        let record = request_detail(&home, &url, &id).await;
        for name in ["digest", "private"] {
            let m = &observed_stage(&home, name, "notify", 1).await[0];
            assert_eq!(m["event"], "request_end");
            assert_eq!(m["request_id"], id);
            assert_eq!(m["delivery"], mode);
            assert_eq!(m["upgraded"], false);
            assert_eq!(m["attempts"], 1);
            assert_eq!(m["upstream_status"], 200);
            assert_eq!(m.get("stream_completed"), Some(&Value::Null));
            assert_eq!(m["outcome"], record["record"]["outcome"]);
            assert_eq!(m["code"], record["record"]["code"]);
            assert!(m["duration_ms"].as_u64().is_some());
            assert_eq!(m["client"]["stream"], false);
            assert_eq!(m["client"]["path"], "/sample");
            for k in ["request", "body", "body_b64"] {
                assert!(m.get(k).is_none());
            }
            if name == "digest" {
                assert_eq!(
                    m["digest"],
                    "abbb2acd0576a7257ee5962fb7f4207e0eccc99cbf5498607f81fd3002da9f5c"
                );
                assert_eq!(m["state"], "observer state");
            } else {
                assert!(m.get("digest").is_none() && m.get("state").is_none());
            }
        }
    }
}

/// 一条通知卡死只处罚观察方向；两个时限均为零时仍然换掉旧会话，业务继续有回信。
#[tokio::test]
async fn stuck_notify_is_replaced_after_two_seconds() {
    let home = TestHome::new();
    let (up, _) = upstream(&[200]).await;
    let cfg = layer(&home, "hung", "'request','notify'", &["stuck-notify"])
        + "pool_max=1\ntimeout_secs=0\ndecide_timeout_secs=0\n";
    let (c, url, mut clock) = Clock::start(&home, up, &cfg);
    assert_eq!(
        c.get(&url).send().await.unwrap().text().await.unwrap(),
        "upstream bytes"
    );
    observed_stage(&home, "hung", "notify", 1).await;
    clock.advance(2);
    assert_eq!(
        c.get(&url).send().await.unwrap().text().await.unwrap(),
        "upstream bytes"
    );
    let msgs = observed_stage(&home, "hung", "request", 2).await;
    assert_eq!(msgs[1]["seq"], 1, "request did not use a fresh worker");
    assert_eq!(observed_stage(&home, "hung", "hello", 2).await.len(), 2);
}

/// 通知坏回信或超时都要丢弃会话；成功的业务不把通知的连续失败数清掉。
#[tokio::test]
async fn notify_timeouts_do_not_touch_the_data_path_breaker() {
    for (mode, kind) in [("stuck-notify", "timeout"), ("invalid-notify", "protocol")] {
        let home = TestHome::new();
        let (up, _) = upstream(&[200]).await;
        let (c, url, mut clock) = Clock::start(
            &home,
            up,
            &(layer(&home, "mixed", "'request','notify'", &[mode])
                + "pool_max=1\nbreaker_failures=2\nbreaker_cooldown_secs=86400\n"),
        );
        for i in 1..=2 {
            assert_eq!(
                c.get(&url).send().await.unwrap().text().await.unwrap(),
                "upstream bytes"
            );
            observed_stage(&home, "mixed", "notify", i as usize).await;
            if mode == "stuck-notify" {
                clock.advance(2);
            }
            let snap = notify_snapshot(&home, &url, |f| f["notify_breaker"]["failures"] == i).await;
            assert_eq!(
                snap["breaker"],
                serde_json::json!({"state":"closed","failures":0})
            );
            assert_eq!(
                snap["notify_breaker"]["last_failure"]["details"]["kind"],
                kind
            );
        }
        assert_eq!(
            c.get(&url).send().await.unwrap().text().await.unwrap(),
            "upstream bytes"
        );
        let snap = notify_snapshot(&home, &url, |f| f["notify_dropped_total"] == 1).await;
        assert_eq!(snap["notify_breaker"]["state"], "open");
        assert_eq!(
            frames(&home, "mixed")
                .iter()
                .filter(|m| m["stage"] == "notify")
                .count(),
            2
        );
    }
}

/// 达到第五次真实超时才开窗；经过冷却后下一条通知能完成握手和探测，不需要触发业务帧。
#[tokio::test]
async fn notify_breaker_opens_after_five_timeouts_and_probes_after_cooldown() {
    let home = TestHome::new();
    let (up, _) = upstream(&[200]).await;
    let gate = home.path().join("reply-ok");
    let (c, url, mut clock) = Clock::start(
        &home,
        up,
        &(layer(
            &home,
            "observer",
            "'notify'",
            &["stuck-notify", gate.to_str().unwrap()],
        ) + "breaker_cooldown_secs=1\n"),
    );
    for i in 1..=5 {
        assert_eq!(
            c.get(&url).send().await.unwrap().text().await.unwrap(),
            "upstream bytes"
        );
        observed_stage(&home, "observer", "notify", i as usize).await;
        clock.advance(2);
        let snap = notify_snapshot(&home, &url, |f| f["notify_breaker"]["failures"] == i).await;
        assert_eq!(
            snap["notify_breaker"]["state"],
            if i == 5 { "open" } else { "closed" }
        );
    }
    std::fs::write(&gate, b"ready").unwrap();
    clock.advance(1);
    assert_eq!(
        c.get(&url).send().await.unwrap().text().await.unwrap(),
        "upstream bytes"
    );
    let snap = notify_snapshot(&home, &url, |f| f["notify_breaker"]["state"] == "closed").await;
    assert_eq!(snap["notify_breaker"]["failures"], 0);
    assert_eq!(
        observed_stage(&home, "observer", "notify", 6).await.len(),
        6
    );
}

/// 可选纯观察程序一开始不存在，不妨碍就绪；放好程序后只靠完成通知恢复。
#[tokio::test]
async fn notify_only_program_recovers_after_its_breaker_opens() {
    let home = TestHome::new();
    let (up, _) = upstream(&[200]).await;
    let path = home
        .path()
        .join(format!("later{}", std::env::consts::EXE_SUFFIX));
    let cfg = layer(&home, "later-log", "'notify'", &["echo"]).replace(
        &serde_json::to_string(&format_echo_path()).unwrap(),
        &serde_json::to_string(&path).unwrap(),
    ) + "required=false\nbreaker_cooldown_secs=1\n";
    let (c, url, mut clock) = Clock::start(&home, up, &cfg);
    let snap = notify_snapshot(&home, &url, |_| true).await;
    assert_eq!(snap["breaker"]["state"], "closed");
    assert_eq!(snap["notify_breaker"]["state"], "open");
    std::fs::copy(format_echo_path(), &path).unwrap();
    clock.advance(1);
    assert_eq!(
        c.get(&url).send().await.unwrap().text().await.unwrap(),
        "upstream bytes"
    );
    observed_stage(&home, "later-log", "notify", 1).await;
    assert_eq!(
        notify_snapshot(&home, &url, |f| f["notify_breaker"]["state"] == "closed").await["notify_breaker"]
            ["failures"],
        0
    );
}

/// Host、Origin、体积和不完整上传都在链前拒绝；随后的一条正常请求作为队尾标记。
#[tokio::test]
async fn requests_rejected_before_the_chain_send_no_notify() {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let home = TestHome::new();
    let (up, calls) = upstream(&[200]).await;
    let cfg = String::from("max_request_mb=1\n")
        + &layer(&home, "accepted", "'request','notify'", &["echo"]);
    let (c, url) = running(&home, up, &cfg);
    for (header, value) in [
        ("host", "unexpected.invalid"),
        ("origin", "https://unexpected.invalid"),
    ] {
        assert_eq!(
            c.get(&url)
                .header(header, value)
                .send()
                .await
                .unwrap()
                .status(),
            403
        );
    }
    assert_eq!(
        c.post(&url)
            .body(vec![1; 1024 * 1024 + 1])
            .send()
            .await
            .unwrap()
            .status(),
        413
    );
    let addr = url.trim_start_matches("http://");
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    s.write_all(
        format!("POST /partial HTTP/1.1\r\nHost: {addr}\r\nContent-Length: 100\r\n\r\nshort")
            .as_bytes(),
    )
    .await
    .unwrap();
    s.shutdown().await.unwrap();
    let mut raw = String::new();
    tokio::time::timeout(
        std::time::Duration::from_secs(12),
        s.read_to_string(&mut raw),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(raw.contains("client_body_aborted"), "{raw}");
    let response = c.get(&url).send().await.unwrap();
    let id = observed_stage(&home, "accepted", "request", 1).await[0]["request_id"].clone();
    assert_eq!(response.text().await.unwrap(), "upstream bytes");
    let ends = observed_stage(&home, "accepted", "notify", 1).await;
    assert_eq!(ends.len(), 1);
    assert_eq!(ends[0]["request_id"], id);
    assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 1);
}

/// 故意不读下一帧的纯观察进程不在请求路径上；业务请求能全部交付，故障留在独立熔断器。
#[tokio::test]
async fn notify_program_that_never_reads_does_not_slow_the_data_path() {
    let home = TestHome::new();
    let (up, calls) = upstream(&[200]).await;
    let (c, url) = running(
        &home,
        up,
        &(layer(&home, "deaf", "'notify'", &["deaf-notify"])
            + "breaker_failures=1\nbreaker_cooldown_secs=86400\n"),
    );
    for result in futures_util::future::join_all(
        (0..64).map(|_| async { c.get(&url).send().await.unwrap().text().await.unwrap() }),
    )
    .await
    {
        assert_eq!(result, "upstream bytes");
    }
    assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 64);
    let snap = notify_snapshot(&home, &url, |f| {
        f["notify_breaker"]["state"] == "open" && f["notify_dropped_total"] == 63
    })
    .await;
    assert_eq!(snap["breaker"]["failures"], 0);
    assert!(frames(&home, "deaf").iter().all(|f| f["stage"] == "hello"));
}

/// 空上传已有完整表示（空串的 SHA-256），区别于上游在客户端还没传完时就结束的 null。
#[tokio::test]
async fn live_empty_upload_notify_hashes_the_empty_representation() {
    let home = TestHome::new();
    let (up, _) = upstream(&[200]).await;
    let (c, url) = running(
        &home,
        up,
        &(String::from("delivery='forward'\n")
            + &layer(&home, "empty", "'notify'", &["notify-digest"])),
    );
    assert_eq!(
        c.get(&url).send().await.unwrap().text().await.unwrap(),
        "upstream bytes"
    );
    assert_eq!(
        observed_stage(&home, "empty", "notify", 1).await[0]["digest"],
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
}

/// 第一次仅被程序拒绝、没有发上游；回放帧的编号仍与实际尝试记录而非发送次数相同。
#[tokio::test]
async fn replay_stream_id_after_a_request_stage_failure_uses_the_logical_attempt() {
    let home = TestHome::new();
    let (up, calls) = upstream(&[200]).await;
    let cfg = layer(
        &home,
        "gate",
        "'request','retry'",
        &["reject-first-request"],
    ) + &layer(&home, "convert", "'stream'", &["stream-echo"]);
    let (c, url) = running(&home, up, &cfg);
    assert_eq!(
        c.get(&url).send().await.unwrap().text().await.unwrap(),
        "[upstream bytes]"
    );
    assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 1);
    let events = observed_stage(&home, "convert", "stream", 3).await;
    let id = events[0]["request_id"].as_str().unwrap();
    let rec = request_detail(&home, &url, id).await;
    assert_eq!(rec["record"]["tries"].as_array().unwrap().len(), 2);
    assert!(
        events.iter().all(|m| m["stream_id"] == format!("{id}/2")),
        "{events:?}"
    );
}

/// 第一位策略程序实际持有票时才开始暂停；后一位必须计入这段等待，不得复用第一帧的毫秒数。
#[tokio::test]
async fn later_decide_elapsed_includes_the_previous_voters_wait() {
    let home = TestHome::new();
    let (up, _) = upstream(&[503]).await;
    let gate = home.path().join("release-vote");
    let cfg = layer(
        &home,
        "first",
        "'decide'",
        &["gate-decide", gate.to_str().unwrap()],
    ) + &layer(
        &home,
        "last",
        "'decide'",
        &["reply", r#"{"decision":"stop"}"#],
    );
    let (c, url) = running(&home, up, &cfg);
    let request = tokio::spawn(c.get(url).send());
    let first = observed_stage(&home, "first", "decide", 1).await;
    tokio::time::sleep(std::time::Duration::from_millis(120)).await;
    std::fs::write(gate, b"ready").unwrap();
    request.await.unwrap().unwrap().bytes().await.unwrap();
    let last = observed_stage(&home, "last", "decide", 1).await;
    assert!(
        last[0]["elapsed_ms"].as_u64().unwrap() >= first[0]["elapsed_ms"].as_u64().unwrap() + 119,
        "first {first:?}; last {last:?}"
    );
}

/// 骨架头先真正交付，内层 open 随后试改状态和头。外层看到的必须仍是未打补丁的上游元数据。
#[tokio::test]
async fn committed_open_patches_do_not_change_the_next_stream_layers_input() {
    use base64::Engine as _;
    use std::io::Write as _;
    let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gzip.write_all(b"replacement").unwrap();
    let encoded = base64::engine::general_purpose::STANDARD.encode(gzip.finish().unwrap());
    for code in [None, Some(204), Some(409)] {
        let home = TestHome::new();
        let patch = if let Some(status) = code {
            serde_json::json!({"fail":{"retry":false,"reason":"replacement","response":{"status":status,"headers":{"x-hidden":"ignored","content-encoding":"gzip"},"body_b64":encoded}}}).to_string()
        } else {
            r#"{"response":{"status":409,"headers":{"x-hidden":"ignored"}}}"#.into()
        };
        let (up, release, _) = held_upstream(&[200], "data: ok\n\n").await;
        let cfg = String::from("keepalive.trigger='accept'\nkeepalive.interval_secs=1\n")
            + &layer(&home, "outer", "'stream'", &["stream-echo"])
            + &layer(&home, "inner", "'stream'", &["reply", &patch]);
        let (c, url) = running(&home, up, &cfg);
        let mut response = c
            .get(&url)
            .header("accept", "text/event-stream")
            .send()
            .await
            .unwrap();
        assert!(response.chunk().await.unwrap().is_some());
        release.add_permits(1);
        let tail = response.bytes().await.unwrap();
        let outer: Vec<_> = frames(&home, "outer")
            .into_iter()
            .filter(|v| v["stage"] == "stream")
            .collect();
        assert!(
            !outer.is_empty(),
            "outer skipped on committed replacement {code:?}"
        );
        if code.is_some() {
            assert_eq!(tail, "[replacement]");
        }
        assert_eq!(outer[0]["response"]["status"], 200);
        assert!(outer[0]["response"]["headers"].get("x-hidden").is_none());
    }
}

/// 不等最后一块才产出：先由客户端确认真正的转换数据，再放行下一块；墙钟数字另作负载测量。
#[tokio::test]
async fn forward_stream_conversion_does_not_batch_chunks() {
    let home = TestHome::new();
    let (up, release) = split_forward_upstream().await;
    let (c, url) = running(
        &home,
        up,
        &(String::from("delivery='forward'\n")
            + &layer(&home, "convert", "'stream'", &["stream-echo"])),
    );
    let mut response = c.get(url).send().await.unwrap();
    let mut prefix = Vec::new();
    while prefix.len() < 6 {
        prefix.extend_from_slice(&response.chunk().await.unwrap().unwrap());
    }
    assert_eq!(prefix, b"[first");
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    release.notify_one();
    assert_eq!(response.text().await.unwrap(), "last]");
}

/// 无正文是 HTTP 的正常结束，而不是 client_gone；open 自己改成 204 也应立刻 close。
#[tokio::test]
async fn bodyless_responses_never_open_streams_and_suppressed_open_closes_immediately() {
    for mode in ["retry", "forward"] {
        let home = TestHome::new();
        let (up, _) = upstream(&[200, 204, 304, 200]).await;
        let (c, url) = running(
            &home,
            up,
            &(format!("delivery='{mode}'\n")
                + &layer(&home, "shape", "'stream'", &["stream-echo"])),
        );
        for (method, status) in [
            (reqwest::Method::HEAD, 200),
            (reqwest::Method::GET, 204),
            (reqwest::Method::GET, 304),
        ] {
            let r = c.request(method, &url).send().await.unwrap();
            assert_eq!(r.status(), status);
            assert!(r.bytes().await.unwrap().is_empty());
        }
        assert_eq!(
            c.get(&url).send().await.unwrap().text().await.unwrap(),
            "[upstream bytes]"
        );
        assert_eq!(
            frames(&home, "shape")
                .iter()
                .filter(|m| m["event"] == "open")
                .count(),
            1,
            "{mode}"
        );
    }
    let home = TestHome::new();
    let (up, _hold_open) = split_forward_upstream().await;
    let cfg = String::from("delivery='forward'\n")
        + &layer(&home, "outer", "'stream'", &["stream-echo"])
        + &layer(
            &home,
            "suppress",
            "'stream'",
            &["reply", r#"{"response":{"status":204},"data":"not sent"}"#],
        );
    let (c, url) = running(&home, up, &cfg);
    let r = c.get(url).send().await.unwrap();
    assert_eq!(r.status(), 204);
    assert!(r.bytes().await.unwrap().is_empty());
    let seen = observed_stage(&home, "suppress", "stream", 2).await;
    assert_eq!(seen[1]["event"], "close");
    assert_eq!(frames(&home, "outer").len(), 1);
}

/// 中间层 abort 的字节只对它自己的协议有意义；外层健康却不回字节时也不得套用备用尾部。
#[tokio::test]
async fn only_the_outer_abort_reply_reaches_the_client_even_when_empty() {
    for ending in ["", "outer-end"] {
        let home = TestHome::new();
        let (up, _) = upstream(&[200]).await;
        let script = home.path().join("abort.json");
        std::fs::write(&script,serde_json::json!({"stream:open":{"abort_tail":"unused"},"stream:abort":if ending.is_empty(){serde_json::json!({})}else{serde_json::json!({"data":ending})}}).to_string()).unwrap();
        let cfg = layer(
            &home,
            "outer",
            "'stream'",
            &["reply-by-stage", script.to_str().unwrap()],
        ) + &layer(&home, "middle", "'stream'", &["stream-echo"])
            + &layer(&home, "break", "'stream'", &["stream-fault"]);
        let (c, url) = running(&home, up, &cfg);
        assert_eq!(
            c.get(url).send().await.unwrap().text().await.unwrap(),
            ending
        );
        for n in ["middle", "outer"] {
            assert_eq!(frames(&home, n).last().unwrap()["event"], "abort");
        }
    }
}

/// Location 和原实体在 forward 的 307 上不经整包或流式转换，即使这两个程序都会改动成功响应。
#[tokio::test]
async fn forward_redirect_bypasses_both_response_and_stream_programs() {
    let home = TestHome::new();
    let (port, l) = mock_listener().await;
    let app = axum::Router::new().fallback(|| async {
        (
            axum::http::StatusCode::TEMPORARY_REDIRECT,
            [("location", "/target")],
            "original redirect",
        )
    });
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    let cfg = String::from("delivery='forward'\n")
        + &layer(&home, "whole", "'response'", &["upper"])
        + &layer(&home, "stream", "'stream'", &["stream-echo"]);
    let (_, url) = running(&home, port, &cfg);
    let c = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .unwrap();
    let r = c.get(url).send().await.unwrap();
    assert_eq!(r.status(), 307);
    assert_eq!(r.headers()["location"], "/target");
    assert_eq!(r.text().await.unwrap(), "original redirect");
    for n in ["whole", "stream"] {
        assert_eq!(frames(&home, n).len(), 1);
    }
}

/// 普通根包测试会编译 example；定向测试也请 Cargo 核对指纹，文件存在并不能证明是本次源码。
fn official_binary() -> &'static std::path::Path {
    static BINARY: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    BINARY.get_or_init(|| {
        let profile_dir = std::path::Path::new(common::APROXY).parent().unwrap();
        let profile = profile_dir.file_name().unwrap().to_str().unwrap();
        let target = env!("APROXY_BUILD_TARGET");
        let mut build = std::process::Command::new(env!("CARGO"));
        build.current_dir(env!("CARGO_MANIFEST_DIR")).args([
            "build",
            "--offline",
            "--locked",
            "--example",
            "format-official",
            "--profile",
            if profile == "debug" { "dev" } else { profile },
        ]);
        let mut target_dir = profile_dir.parent().unwrap();
        if target_dir.file_name() == Some(std::ffi::OsStr::new(target)) {
            build.args(["--target", target]);
            target_dir = target_dir.parent().unwrap();
        }
        build.arg("--target-dir").arg(target_dir);
        if !cfg!(feature = "desktop") {
            build.arg("--no-default-features");
        }
        assert!(
            build.status().unwrap().success(),
            "official worker example build failed"
        );
        profile_dir
            .join("examples")
            .join(format!("format-official{}", std::env::consts::EXE_SUFFIX))
    })
}
/// 配置真正的角色，不依赖 PATH 或机器上另装的版本。
fn official_layer(name: &str, stages: &str, args: &[&str]) -> String {
    format!(
        "\n[[format]]\nname='{name}'\ncommand={}\nargs={}\nprocess='persistent'\npool_max=1\nworker_idle_secs=0\nstages=[{stages}]\n",
        serde_json::to_string(official_binary()).unwrap(),
        serde_json::to_string(args).unwrap()
    )
}
/// 完整 hello 来自每个独立子命令；能力全集与体粒度另外在直接协议测试中逐项核对。
#[test]
fn every_role_handshakes_alone_in_a_chain() {
    for (name, stages) in [
        ("convert", "'request','retry','stream'"),
        ("route", "'request','retry'"),
        ("keys", "'request','retry'"),
        ("policy", "'decide'"),
        ("adaptive", "'request','decide','notify'"),
        ("rewrite", "'request','retry'"),
        ("heartbeat", "'heartbeat'"),
    ] {
        let home = TestHome::new();
        let cfg = home.path().join("role.toml");
        let path = cfg.to_str().unwrap();
        let args = match name {
            "convert" => vec![name, "--from", "auto", "--to", "openai_chat"],
            "route" => {
                std::fs::write(&cfg, "[[channel]]\nid='local'\nurl='http://127.0.0.1/'").unwrap();
                vec![name, "--config", path]
            }
            "keys" => {
                std::fs::write(&cfg,"[[pool]]\norigin='http://127.0.0.1'\nheader='x-api-key'\nkeys=[{id='test',env='KEY'}]").unwrap();
                vec![name, "--config", path]
            }
            _ => vec![name],
        };
        let mut layer = official_layer(name, stages, &args);
        if name == "keys" {
            layer.push_str("env={KEY='fixture-key'}\n");
        }
        let (_, out) = launch(&home, free_port(), &layer);
        assert!(out.status.success(), "{name}: {out:?}");
        let snapshot: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(snapshot["instance"]["formats"][0]["protocol"], 1);
        assert_eq!(
            snapshot["instance"]["formats"][0]["breaker"]["state"],
            "closed"
        );
    }
}
/// 真实 adaptive 协商 digest；在管道上核对 request 和异步完成帧均无对话正文。
#[tokio::test]
async fn adaptive_gets_only_the_digest() {
    let home = TestHome::new();
    let (up, _) = upstream(&[200]).await;
    let bin = official_binary();
    let cfg = layer(
        &home,
        "adaptive",
        "'request','decide','notify'",
        &["record-worker", bin.to_str().unwrap(), "adaptive"],
    );
    let (c, url) = running(&home, up, &cfg);
    let bytes = "private fixture text";
    assert_eq!(
        c.post(&url)
            .body(bytes)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        "upstream bytes"
    );
    let notify = observed_stage(&home, "adaptive", "notify", 1).await;
    let request = observed_stage(&home, "adaptive", "request", 1).await;
    let expected = "85056b4d36b70a6a8f4b73cb8896af293dd047a185f96363f44a7113cdde4b7d";
    for m in [&request[0], &notify[0]] {
        assert_eq!(m["digest"], expected);
        assert!(!m.to_string().contains(bytes));
        assert!(m["request"].get("body").is_none() && m["request"].get("body_b64").is_none());
    }
    assert_eq!(notify[0]["delivery"], "forward");
}
/// 从 Anthropic 客户端经三角色走到 Chat 上游，再从 stream 方向送回完整的 Anthropic 消息。
#[tokio::test]
async fn sample_chain_of_section_7_2_runs_end_to_end() {
    let home = TestHome::new();
    let (up, listener) = mock_listener().await;
    let (sent, observed) = tokio::sync::oneshot::channel();
    let slot = std::sync::Arc::new(std::sync::Mutex::new(Some(sent)));
    let app=axum::Router::new().fallback(move|uri:axum::http::Uri,h:axum::http::HeaderMap,b:axum::body::Bytes|{let sent=slot.lock().unwrap().take();async move{if let Some(sent)=sent{sent.send((uri,h,b)).unwrap();}axum::Json(serde_json::json!({"id":"chatcmpl-local","object":"chat.completion","model":"local","created":0,"choices":[{"index":0,"finish_reason":"stop","message":{"role":"assistant","content":"fixture answer"}}],"usage":{"prompt_tokens":7,"completion_tokens":2,"total_tokens":9}}))}});
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let keys = home.path().join("keys.toml");
    std::fs::write(&keys,format!("[[pool]]\norigin='http://127.0.0.1:{up}'\nheader='authorization'\nscheme='Bearer'\nkeys=[{{id='local',env='LOCAL_KEY'}}]" )).unwrap();
    let cfg = official_layer("adaptive", "'request','decide','notify'", &["adaptive"])
        + "required=false\n"
        + &official_layer(
            "convert",
            "'request','stream'",
            &[
                "convert",
                "--from",
                "anthropic_messages",
                "--to",
                "openai_chat",
            ],
        )
        + &official_layer(
            "keys",
            "'request','retry'",
            &["keys", "--config", keys.to_str().unwrap()],
        )
        + "env={LOCAL_KEY='fictional-sample-key'}\n";
    let (c, url) = running(&home, up, &cfg);
    let response=c.post(format!("{url}/v1/messages")).header("x-api-key","incoming-client-key").json(&serde_json::json!({"model":"local","max_tokens":32,"system":"be nice","messages":[{"role":"user","content":"hello"}]})).send().await.unwrap();
    assert_eq!(response.status(), 200);
    let value: Value = response.json().await.unwrap();
    assert_eq!(value["content"][0]["text"], "fixture answer", "{value}");
    assert_eq!(value["stop_reason"], "end_turn");
    let (uri, h, b) = observed.await.unwrap();
    assert_eq!(uri.path(), "/v1/chat/completions");
    assert_eq!(h["authorization"], "Bearer fictional-sample-key");
    assert!(!h.contains_key("x-api-key"));
    let req: Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(req["messages"][0]["role"], "system");
    assert_eq!(req["messages"][1]["content"], "hello");
}
/// 可选判断程序握手失败时默认投递仍为 retry：熔断器真实打开，上游 503 后成功。
#[tokio::test]
async fn adaptive_unavailable_falls_back_to_retry() {
    let home = TestHome::new();
    let (up, calls) = upstream(&[503, 200]).await;
    let missing = home.path().join("unavailable.toml");
    let cfg = official_layer(
        "adaptive",
        "'request','decide','notify'",
        &["adaptive", "--config", missing.to_str().unwrap()],
    ) + "required=false\nbreaker_cooldown_secs=3600\n";
    let (c, url) = running(&home, up, &cfg);
    assert_eq!(breaker(&home, &url).await["state"], "open");
    assert_eq!(
        c.get(&url).send().await.unwrap().text().await.unwrap(),
        "upstream bytes"
    );
    assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 2);
}

/// 同一渠道默认允许重试，只有原始模型 limited 的 cap=1；实际 HTTP 结果与上游调用数均受覆盖约束。
#[tokio::test]
async fn official_route_model_override_caps_only_the_matching_request() {
    let home = TestHome::new();
    let (up, count) = upstream(&[503, 503, 200]).await;
    let file = home.path().join("route.toml");
    std::fs::write(&file,format!("[aliases]\nlimited='actual'\n[[channel]]\nid='one'\nurl='http://127.0.0.1:{up}/'\n[channel.overrides]\n'retry.max_attempts'=5\n[[rule]]\nmodel='limited'\n[rule.overrides]\n'retry.max_attempts'=1")).unwrap();
    let cfg = official_layer(
        "route",
        "'request','retry'",
        &["route", "--config", file.to_str().unwrap()],
    );
    let (c, url) = running(&home, up, &cfg);
    let limited = c
        .post(&url)
        .json(&serde_json::json!({"model":"limited"}))
        .send()
        .await
        .unwrap();
    assert_eq!(limited.status(), 503);
    assert_eq!(limited.headers()["x-aproxy-error"], "retry_exhausted");
    assert_eq!(
        c.post(url)
            .json(&serde_json::json!({"model":"actual"}))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(count.load(std::sync::atomic::Ordering::Relaxed), 3);
}

/// 两次已经交付的 forward 失败由真实完成通知记账。第三次同体进入 retry，而另一个摘要仍可直通。
#[tokio::test]
async fn official_adaptive_pins_twice_failed_digest_but_forwards_a_fresh_one() {
    let home = TestHome::new();
    let (up, calls) = upstream(&[503, 503, 503, 200, 200]).await;
    let cfg = layer(
        &home,
        "adaptive",
        "'request','notify'",
        &[
            "record-worker",
            official_binary().to_str().unwrap(),
            "adaptive",
        ],
    ) + "pool_max=1\nrequired=false\n"
        + &layer(&home, "observe", "'request'", &["echo"]);
    let (client, url) = running(&home, up, &cfg);
    for (i, (body, status, mode)) in [
        ("same", 503, "forward"),
        ("same", 503, "forward"),
        ("same", 200, "retry"),
        ("fresh", 200, "forward"),
    ]
    .into_iter()
    .enumerate()
    {
        let r = client.post(&url).body(body).send().await.unwrap();
        assert_eq!(r.status(), status);
        r.bytes().await.unwrap();
        let notes = observed_stage(&home, "adaptive", "notify", i + 1).await;
        assert_eq!(notes[i]["delivery"], mode);
        assert_eq!(
            observed_stage(&home, "observe", "request", i + 1).await[i]["delivery"],
            mode
        );
    }
    assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 5);
}
/// forward 尚未提交时，真实判定程序把同一请求升级，重放成功且只记一次升级。
#[tokio::test]
async fn official_adaptive_upgrades_forward_before_client_bytes() {
    let home = TestHome::new();
    let (up, calls) = upstream(&[503, 200]).await;
    let cfg = layer(
        &home,
        "adaptive",
        "'request','decide','notify'",
        &[
            "record-worker",
            official_binary().to_str().unwrap(),
            "adaptive",
        ],
    ) + "pool_max=1\nrequired=false\n";
    let (c, url) = running(&home, up, &cfg);
    assert_eq!(
        c.post(url)
            .body("same")
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        "upstream bytes"
    );
    let decide = observed_stage(&home, "adaptive", "decide", 1).await;
    assert_eq!(decide[0]["delivery"], "forward");
    let notify = observed_stage(&home, "adaptive", "notify", 1).await;
    assert_eq!(notify[0]["upgraded"], true);
    assert_eq!(notify[0]["attempts"], 2);
    assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 2);
}

/// 仅声明检查成功响应的规则会接到 200。错误藏在普通文本里，代理自己的兜底不会重试它。
#[tokio::test]
async fn official_policy_retries_a_200_with_an_error_marker() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let home = TestHome::new();
    let (up, listener) = mock_listener().await;
    let calls = std::sync::Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let app = axum::Router::new().fallback(move || {
        let i = seen.fetch_add(1, Ordering::Relaxed);
        async move {
            if i == 0 {
                "local busy marker"
            } else {
                "complete"
            }
        }
    });
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let file = home.path().join("policy.toml");
    std::fs::write(
        &file,
        "[[rule]]\nid='busy'\nstatus=[200]\nbody_regex='busy marker'\ndecision='retry'\ndelay_ms=0",
    )
    .unwrap();
    let cfg = official_layer(
        "policy",
        "'decide'",
        &["policy", "--config", file.to_str().unwrap()],
    );
    let (c, url) = running(&home, up, &cfg);
    assert_eq!(
        c.get(url).send().await.unwrap().text().await.unwrap(),
        "complete"
    );
    assert_eq!(calls.load(Ordering::Relaxed), 2);
}

/// 同一链的 Chat 渠道能原样返回；它在第二种模型上失败后，另一租户收到重新生成的 Messages 请求。
#[tokio::test]
async fn full_config_roundtrip_routes_mixed_protocols_and_reconverts_on_failover() {
    let home = TestHome::new();
    let (up, listener) = mock_listener().await;
    let (send, mut observed) = tokio::sync::mpsc::unbounded_channel();
    let app=axum::Router::new().fallback(move |u:axum::http::Uri, axum::Json(b):axum::Json<Value>| {let model=b["model"].as_str().unwrap().to_owned();let anth=u.path()=="/messages-tenant/run";send.send((u,b)).unwrap();async move {
        let (status,reply)=if anth {(200,serde_json::json!({"id":"m2","type":"message","role":"assistant","model":model,"content":[{"type":"text","text":"second"}],"stop_reason":"end_turn","usage":{"input_tokens":2,"output_tokens":1}}))}else if model=="switch"{(503,serde_json::json!({"error":"busy"}))}else{(200,serde_json::json!({"id":"m1","object":"chat.completion","model":model,"choices":[{"index":0,"message":{"role":"assistant","content":"first"},"finish_reason":"stop"}]}))};(axum::http::StatusCode::from_u16(status).unwrap(),axum::Json(reply))
    }});
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let origin = format!("http://127.0.0.1:{up}");
    let route = home.path().join("route.toml");
    let convert = home.path().join("convert.toml");
    std::fs::write(&route,format!("[[channel]]\nid='chat'\npriority=1\nurl='{origin}/chat-tenant/run'\n[[channel]]\nid='messages'\npriority=2\nurl='{origin}/messages-tenant/run'\n")).unwrap();
    std::fs::write(&convert,format!("from='auto'\nto='auto'\n[[upstream]]\norigin='{origin}'\npath_prefix='/chat-tenant'\nprotocol='openai_chat'\n[[upstream]]\norigin='{origin}'\npath_prefix='/messages-tenant'\nprotocol='anthropic_messages'\n")).unwrap();
    let cfg = official_layer(
        "route",
        "'request','retry'",
        &["route", "--config", route.to_str().unwrap()],
    ) + &official_layer(
        "convert",
        "'request','retry','response'",
        &["convert", "--config", convert.to_str().unwrap()],
    );
    let (client, url) = running(&home, up, &cfg);
    for (model, answer) in [("ordinary", "first"), ("switch", "second")] {
        let r=client.post(format!("{url}/v1/chat/completions")).json(&serde_json::json!({"model":model,"max_tokens":10,"messages":[{"role":"system","content":"instructions"},{"role":"user","content":"hi"}]})).send().await.unwrap().json::<Value>().await.unwrap();
        assert_eq!(r["choices"][0]["message"]["content"], answer, "{r}");
    }
    for expected in [
        "/chat-tenant/run",
        "/chat-tenant/run",
        "/messages-tenant/run",
    ] {
        let (uri, b) = observed.try_recv().unwrap();
        assert_eq!(uri.path(), expected);
        if expected.starts_with("/chat") {
            assert_eq!(b["messages"][0]["role"], "system");
        } else {
            assert_eq!(b["system"], "instructions");
            assert_eq!(b["messages"][0]["role"], "user");
        }
    }
}

/// 两个不同 origin 的真实上游分别观察地址、模型和认证；503 的下一次尝试不能沿用 A 的凭据。
#[tokio::test]
async fn official_route_failover_swaps_url_and_credentials_together() {
    let home = TestHome::new();
    let (send, mut observed) = tokio::sync::mpsc::unbounded_channel();
    let mut config = String::from("[aliases]\nsmall='provider-small'\n");
    for (id, priority, status, header, env) in [
        ("a", 1, 503, "authorization", "KEY_A"),
        ("b", 2, 200, "x-api-key", "KEY_B"),
    ] {
        let (port, listener) = mock_listener().await;
        let send = send.clone();
        let app = axum::Router::new().fallback(
            move |u: axum::http::Uri, h: axum::http::HeaderMap, b: axum::body::Bytes| {
                send.send((id, u, h, b)).unwrap();
                async move { (axum::http::StatusCode::from_u16(status).unwrap(), "reply") }
            },
        );
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        config += &format!(
            "[[channel]]\nid='{id}'\npriority={priority}\nurl='http://127.0.0.1:{port}/{id}'\ncredential_env='{env}'\nauth_header='{header}'\n"
        );
    }
    let file = home.path().join("route.toml");
    std::fs::write(&file, config).unwrap();
    let cfg = official_layer(
        "route",
        "'request','retry'",
        &["route", "--config", file.to_str().unwrap()],
    ) + "env={KEY_A='fixture-one', KEY_B='fixture-two'}\n";
    let (client, url) = running(&home, free_port(), &cfg);
    let res = client
        .post(url)
        .header("authorization", "old")
        .json(&serde_json::json!({"model":"small"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.text().await.unwrap(), "reply");
    for (name, path, selected, value, absent) in [
        (
            "a",
            "/a",
            "authorization",
            "Bearer fixture-one",
            "x-api-key",
        ),
        ("b", "/b", "x-api-key", "fixture-two", "authorization"),
    ] {
        let (id, u, h, b) = observed.try_recv().unwrap();
        assert_eq!(id, name);
        assert_eq!(u.path(), path);
        assert_eq!(h[selected], value);
        assert!(!h.contains_key(absent));
        assert_eq!(
            serde_json::from_slice::<Value>(&b).unwrap()["model"],
            "provider-small"
        );
    }
    assert!(observed.try_recv().is_err());
}

/// 429 使 A 冷却；下一次尝试和另一个请求都走 B，而不只是恰好轮转一次。
#[tokio::test]
async fn official_keys_cool_the_previous_key_on_429() {
    let home = TestHome::new();
    let up = free_port();
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = seen.clone();
    let app = axum::Router::new().fallback(move |h: axum::http::HeaderMap| {
        let mut guard = log.lock().unwrap();
        let first = guard.is_empty();
        guard.push(h["x-api-key"].to_str().unwrap().to_string());
        async move {
            (
                if first {
                    axum::http::StatusCode::TOO_MANY_REQUESTS
                } else {
                    axum::http::StatusCode::OK
                },
                [("retry-after", "600")],
                "reply",
            )
        }
    });
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, up))
        .await
        .unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let file = home.path().join("keys.toml");
    std::fs::write(&file,format!("cooldown_secs=0\n[[pool]]\norigin='http://127.0.0.1:{up}'\nheader='x-api-key'\nkeys=[{{id='a',env='KEY_A'}},{{id='b',env='KEY_B'}}]")).unwrap();
    let cfg = official_layer(
        "keys",
        "'request','retry'",
        &["keys", "--config", file.to_str().unwrap()],
    ) + "env={ KEY_A='fixture-one', KEY_B='fixture-two' }\n";
    let (client, url) = running(&home, up, &cfg);
    for _ in 0..2 {
        assert_eq!(client.get(&url).send().await.unwrap().status(), 200);
    }
    assert_eq!(
        *seen.lock().unwrap(),
        ["fixture-one", "fixture-two", "fixture-two"]
    );
}

/// 已经成功给过 forward 的真实角色失去配置后打开熔断。后续请求不能沿用上一请求的表态。
#[tokio::test]
async fn previously_healthy_adaptive_unavailable_falls_back_to_retry() {
    use std::sync::atomic::Ordering;
    let home = TestHome::new();
    let (up, calls) = upstream(&[200, 200, 503, 200]).await;
    let file = home.path().join("adaptive.toml");
    std::fs::write(&file, "").unwrap();
    let cfg = official_layer(
        "adaptive",
        "'request'",
        &["adaptive", "--config", file.to_str().unwrap()],
    )
    .replace("process='persistent'", "process='spawn'")
        + "required=false\nbreaker_failures=1\nbreaker_cooldown_secs=3600\n"
        + &layer(&home, "observe", "'request'", &["echo"]);
    let (client, url) = running(&home, up, &cfg);
    assert_eq!(
        client.get(&url).send().await.unwrap().text().await.unwrap(),
        "upstream bytes"
    );
    assert_eq!(frames(&home, "observe")[1]["delivery"], "forward");
    assert_eq!(breaker(&home, &url).await["state"], "closed");
    // 下一次真实握手读取这份测试自有的文件，失败发生在成功服务之后。
    std::fs::write(file, "invalid = [").unwrap();
    assert_eq!(
        client.get(&url).send().await.unwrap().text().await.unwrap(),
        "upstream bytes"
    );
    assert_eq!(breaker(&home, &url).await["state"], "open");
    assert_eq!(calls.load(Ordering::Relaxed), 2);
    assert_eq!(
        client.get(&url).send().await.unwrap().text().await.unwrap(),
        "upstream bytes"
    );
    assert_eq!(frames(&home, "observe")[3]["delivery"], "retry");
    assert_eq!(calls.load(Ordering::Relaxed), 4);
}

/// 链内的一次拒绝已触发 forward 升级；恢复后的请求层不能再次选择 forward 并重复计数。
#[tokio::test]
async fn upgrade_locks_delivery_before_resuming_the_failed_request_layer() {
    let home = TestHome::new();
    let (up, calls) = upstream(&[503, 200]).await;
    let cfg = String::from("delivery='forward'\n")
        + &layer(&home, "gate", "'request'", &["reject-first-request"])
        + &layer(
            &home,
            "late",
            "'request'",
            &["reply", r#"{"delivery":"forward"}"#],
        )
        + &layer(
            &home,
            "policy",
            "'decide'",
            &["reply", r#"{"decision":"retry","delay_ms":0}"#],
        );
    let (c, url) = running(&home, up, &cfg);
    assert_eq!(
        c.get(&url).send().await.unwrap().text().await.unwrap(),
        "upstream bytes"
    );
    assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 2);
    let votes = frames(&home, "policy");
    assert_eq!(votes.len(), 3);
    assert_eq!(votes[1]["failure"]["code"], "format_fail");
    assert_eq!(votes[2]["delivery"], "retry");
    let rec = request_detail(&home, &url, votes[1]["request_id"].as_str().unwrap()).await;
    assert_eq!(rec["record"]["delivery"], "retry");
    assert_eq!(rec["record"]["upgraded"], true);
    let ping =
        aproxy::daemon::ipc_ping_in(&home.path().join("run"), url.rsplit(':').next().unwrap())
            .await
            .unwrap();
    assert_eq!(ping.activity.upgraded_total, 1);
}

/// 默认 retry 即使所有请求层只看头，也先读完原体并在等待程序时维持 body_stream 保活。
#[tokio::test]
async fn retry_reads_body_and_beats_while_a_metadata_request_worker_is_held() {
    let home = TestHome::new();
    let (up, calls) = upstream(&[200]).await;
    let gate = home.path().join("metadata.release");
    let cfg = String::from(
        "[keepalive]\ntrigger='body_stream'\ninterval_secs=1\nheartbeat=\": waiting\\n\\n\"\n",
    ) + &layer(
        &home,
        "held",
        "'request'",
        &["gate-metadata", gate.to_str().unwrap()],
    );
    let (c, url, mut clock) = Clock::start(&home, up, &cfg);
    let task = tokio::spawn(c.post(url).body(r#"{"stream":true}"#).send());
    let input = observed_stage(&home, "held", "request", 1).await;
    if input[0]["client"]["stream"] != true {
        std::fs::write(&gate, b"go").unwrap();
        let _ = task.await.unwrap().unwrap().bytes().await;
        panic!(
            "request frame did not identify completed upload: {}",
            input[0]
        );
    }
    assert!(input[0]["request"].get("body").is_none());
    clock.advance(1);
    let mut response = task.await.unwrap().unwrap();
    let mut first = Vec::new();
    while !first.ends_with(b": waiting\n\n") {
        first.extend_from_slice(&response.chunk().await.unwrap().unwrap());
    }
    assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 0);
    std::fs::write(gate, b"go").unwrap();
    assert_eq!(response.bytes().await.unwrap(), "upstream bytes");
}

/// 整包响应层尚未交付任何字节时遇到上游中断：可以升级，也能按 forward 规则无响应停止。
#[tokio::test]
async fn interrupted_forward_gather_consults_decide_before_exposing_a_prefix() {
    use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _};
    for retry in [false, true] {
        let home = TestHome::new();
        let (port, listener) = mock_listener().await;
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut lines = tokio::io::BufReader::new(&mut sock).lines();
            while !lines.next_line().await.unwrap().unwrap().is_empty() {}
            sock.write_all(b"HTTP/1.1 200 OK\r\nRetry-After: 19\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nbad\r\n").await.unwrap();
            sock.shutdown().await.unwrap();
            drop(sock);
            if retry {
                axum::serve(listener, axum::Router::new().fallback(|| async { "whole" }))
                    .await
                    .unwrap();
            }
        });
        let cfg = String::from("delivery='forward'\n")
            + &layer(
                &home,
                "policy",
                "'decide'",
                &[
                    "reply",
                    if retry {
                        r#"{"decision":"retry","delay_ms":0}"#
                    } else {
                        r#"{"decision":"stop"}"#
                    },
                ],
            )
            // stop 分支的外层永不答 open，即使它已取得 worker，也不能阻塞内层已经报告的中断。
            + &if retry {String::new()} else {layer(&home,"outer","'stream'", &["no-stage-reply"]) + "timeout_secs=0\n"}
            + &layer(&home, "buffer", "'response'", &["echo"]);
        let (c, url) = running(&home, port, &cfg);
        let response = c.get(&url).send().await.unwrap();
        assert_eq!(response.status(), if retry { 200 } else { 502 });
        let body = response.bytes().await.unwrap();
        if retry {
            assert_eq!(body, "whole");
        } else {
            assert_eq!(
                serde_json::from_slice::<Value>(&body).unwrap()["error"]["aproxy"]["code"],
                "upstream_failed"
            );
        }
        let f = frames(&home, "policy");
        assert_eq!(f.len(), 2);
        assert_eq!(f[1]["failure"]["code"], "upstream_body_interrupted");
        assert_eq!(f[1]["response"]["headers"]["retry-after"], "19");
        assert!(f[1]["response"].get("body").is_none());
        let record = request_detail(&home, &url, f[1]["request_id"].as_str().unwrap()).await;
        assert_eq!(record["record"]["upgraded"], retry);
        if retry {
            assert_eq!(record["record"]["outcome"], "ok");
        }
    }
}

/// 真实转发链：上游尚不许结束时客户端已经收到目标协议的文本；随后中断只产生目标协议错误。
#[tokio::test]
async fn official_convert_delivers_live_text_then_protocol_abort() {
    use axum::body::{Body, Bytes};
    use futures_util::StreamExt as _;
    let home = TestHome::new();
    let (up, listener) = mock_listener().await;
    let halt = std::sync::Arc::new(tokio::sync::Semaphore::new(0));
    let gate = halt.clone();
    let app=axum::Router::new().fallback(move |_: Bytes| {let gate=gate.clone(); async move {
        let events=futures_util::stream::once(async {Ok::<_,std::io::Error>(Bytes::from_static(b"data: {\"id\":\"chatcmpl-test\",\"model\":\"m\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"live now\"}}]}\n\n"))}).chain(futures_util::stream::once(async move {gate.acquire().await.unwrap().forget();Err(std::io::Error::other("cut"))}));
        axum::response::Response::builder().header("content-type","text/event-stream").body(Body::from_stream(events)).unwrap()
    }});
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let cfg = String::from("delivery='forward'\n")
        + &official_layer(
            "convert",
            "'request','stream'",
            &["convert", "--from", "auto", "--to", "openai_chat"],
        )
        + "required=true\n";
    let (client, url) = running(&home, up, &cfg);
    let mut res=client.post(format!("{url}/v1/messages")).json(&serde_json::json!({"model":"m","messages":[{"role":"user","content":"hi"}],"max_tokens":20,"stream":true})).send().await.unwrap();
    assert_eq!(res.status(), 200);
    let mut bytes = Vec::new();
    while !String::from_utf8_lossy(&bytes).contains("live now") {
        bytes.extend(res.chunk().await.unwrap().expect("no real-time text"));
    }
    assert!(String::from_utf8_lossy(&bytes).contains("event: content_block_delta"));
    halt.add_permits(1);
    bytes.extend(res.bytes().await.unwrap());
    let body = String::from_utf8(bytes).unwrap();
    assert!(
        body.contains("event: error\ndata:")
            && body.contains("stream conversion aborted")
            && !body.contains("message_stop")
    );
}

/// 完成通知按实体的真实媒体类型与全部编码字段判定，不把参数中的 SSE 字样当作流。
#[tokio::test]
async fn forward_completion_observes_media_type_and_all_codings() {
    use std::io::Write as _;
    for compressed in [true, false] {
        let home = TestHome::new();
        let (port, listener) = mock_listener().await;
        let mut bytes = if compressed {
            br#"{"error":"busy"}"#.to_vec()
        } else {
            br#"{"ok":true}"#.to_vec()
        };
        if compressed {
            for _ in 0..2 {
                let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
                gz.write_all(&bytes).unwrap();
                bytes = gz.finish().unwrap();
            }
        }
        let payload = bytes.clone();
        let app = axum::Router::new().fallback(move || {
            let raw = payload.clone();
            async move {
                let mut head = axum::response::Response::builder();
                if compressed {
                    head = head
                        .header("content-type", "application/json")
                        .header("content-encoding", "gzip")
                        .header("content-encoding", "gzip");
                } else {
                    head = head.header(
                        "content-type",
                        r#"application/json; profile="text/event-stream""#,
                    );
                }
                head.body(axum::body::Body::from(raw)).unwrap()
            }
        });
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let (client, url) = running(
            &home,
            port,
            &(String::from("delivery='forward'\n")
                + &layer(&home, "observer", "'notify'", &["echo"])),
        );
        let response = client.get(url).send().await.unwrap();
        assert_eq!(response.bytes().await.unwrap().as_ref(), bytes.as_slice());
        let end = &observed_stage(&home, "observer", "notify", 1).await[0];
        assert_eq!(end.get("stream_completed"), Some(&Value::Null));
        if compressed {
            assert_eq!(end["code"], "upstream_failed");
        } else {
            assert_eq!(end["outcome"], "ok");
        }
    }
}
/// 只有真正重试才登记 reroute；转发失败且只投重跑路由票时仍停在第一次上游响应。
#[tokio::test]
async fn stopped_forward_attempt_has_no_reroute_in_history() {
    let home = TestHome::new();
    let (up, _) = upstream(&[503]).await;
    let cfg = String::from("delivery='forward'\n")
        + &layer(&home, "vote", "'decide'", &["reply", r#"{"reroute":true}"#]);
    let (c, url) = running(&home, up, &cfg);
    let r = c.get(&url).send().await.unwrap();
    assert_eq!(r.status(), 503);
    assert_eq!(r.text().await.unwrap(), "upstream bytes");
    let msg = &observed_stage(&home, "vote", "decide", 1).await[0];
    let record = request_detail(&home, &url, msg["request_id"].as_str().unwrap()).await;
    let d = &record["record"]["tries"][0]["decision"];
    assert_eq!(d["decision"], "stop");
    assert!(d.get("reroute").is_none(), "{d}");
}

/// 204 不交付实体，但 close 是正常阶段：它的状态与报告先于完成通知，并不适用断连宽限。
#[tokio::test]
async fn bodyless_close_state_reaches_notify_after_a_held_reply() {
    let home = TestHome::new();
    let (up, _) = upstream(&[200]).await;
    let gate = home.path().join("close.release");
    let cfg = String::from("delivery='forward'\n")
        + &layer(
            &home,
            "suppress",
            "'stream','notify'",
            &["suppress-and-close", gate.to_str().unwrap()],
        )
        + "timeout_secs=0\n";
    let (c, url, mut clock) = Clock::start(&home, up, &cfg);
    let request = tokio::spawn(c.get(&url).send());
    let stream = observed_stage(&home, "suppress", "stream", 2).await;
    clock.advance(3);
    std::fs::write(&gate, []).unwrap();
    let r = request.await.unwrap().unwrap();
    assert_eq!(r.status(), 204);
    assert!(r.bytes().await.unwrap().is_empty());
    let end = &observed_stage(&home, "suppress", "notify", 1).await[0];
    assert_eq!(end["state"], "finished close");
    let record = request_detail(&home, &url, stream[0]["request_id"].as_str().unwrap()).await;
    assert!(record.to_string().contains("closed bodyless response"));
}

/// 原 worker 的 open 时上传未完，在替换 worker 握手期间收完；第二次真实写帧必须看到已知值。
#[tokio::test]
async fn fresh_stream_open_reads_client_flag_after_its_hello() {
    use futures_util::StreamExt as _;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let home = TestHome::new();
    let (up, listener) = mock_listener().await;
    let (uploaded, mut seen) = tokio::sync::mpsc::unbounded_channel();
    let app = axum::Router::new().fallback(move |req: axum::extract::Request| {
        let uploaded = uploaded.clone();
        async move {
            let mut stream = req.into_body().into_data_stream();
            let mut original = stream.next().await.unwrap().unwrap().to_vec();
            let tail = futures_util::stream::once(async move {
                while let Some(b) = stream.next().await {
                    original.extend_from_slice(&b.unwrap());
                }
                uploaded.send(original).unwrap();
                Ok::<_, std::io::Error>("complete")
            });
            axum::body::Body::from_stream(
                futures_util::stream::once(async { Ok("prefix") }).chain(tail),
            )
        }
    });
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let gate = home.path().join("fresh.release");
    let cfg = String::from("delivery='forward'\nkeepalive.trigger='body_stream'\n")
        + &layer(
            &home,
            "open",
            "'stream'",
            &["fresh-open-upload", gate.to_str().unwrap()],
        );
    let (_, url) = running(&home, up, &cfg);
    let addr = url.strip_prefix("http://").unwrap();
    let mut client = tokio::net::TcpStream::connect(addr).await.unwrap();
    client.write_all(format!("POST / HTTP/1.1\r\nHost: {addr}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\na\r\n{{\"stream\":\r\n").as_bytes()).await.unwrap();
    observed_stage(&home, "open", "hello", 2).await;
    client.write_all(b"5\r\ntrue}\r\n0\r\n\r\n").await.unwrap();
    let bytes = tokio::time::timeout(std::time::Duration::from_secs(15), seen.recv())
        .await
        .unwrap()
        .unwrap();
    std::fs::write(gate, []).unwrap();
    let mut out = Vec::new();
    client.read_to_end(&mut out).await.unwrap();
    assert_eq!(bytes, br#"{"stream":true}"#);
    let opens: Vec<_> = frames(&home, "open")
        .into_iter()
        .filter(|m| m["event"] == "open")
        .collect();
    assert_eq!(opens.len(), 2);
    assert!(opens[0]["client"].get("stream").is_none());
    assert_eq!(opens[1]["client"]["stream"], true);
}

/// 元数据程序已经运行，替换的正文不改变原上传的大小限制、历史或完成通知摘要。
#[tokio::test]
async fn replaced_forward_upload_is_drained_and_post_entry_413_is_notified() {
    let home = TestHome::new();
    let (up, listener) = mock_listener().await;
    let (tx, mut sent) = tokio::sync::mpsc::unbounded_channel();
    let app = axum::Router::new().fallback(move |b: axum::body::Bytes| {
        tx.send(b).unwrap();
        async { "ok" }
    });
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let cfg = String::from(
        "delivery='forward'\ndisk_cache=false\nmax_request_mb=1\nkeepalive.trigger='body_stream'\n",
    ) + &layer(
        &home,
        "rewrite",
        "'request','notify'",
        &["replace-metadata"],
    );
    let (c, url) = running(&home, up, &cfg);
    let raw = format!(r#"{{"pad":"{}","stream":true}}"#, "x".repeat(256 * 1024));
    let ok = c.post(&url).body(raw.clone()).send().await.unwrap();
    assert_eq!(ok.text().await.unwrap(), "ok");
    assert_eq!(sent.recv().await.unwrap(), "replacement");
    let m = &observed_stage(&home, "rewrite", "notify", 1).await[0];
    assert_eq!(m["state"], "rewritten");
    assert_eq!(
        m["digest"],
        "d2174028b1de8c73a15c53fffc35f392fc2e998279d8fafb4c60ec52432b21d2"
    );
    assert_eq!(m["client"]["stream"], true);
    let history = request_detail(&home, &url, m["request_id"].as_str().unwrap()).await;
    assert_eq!(history["record"]["request_bytes"], raw.len());
    let over = c
        .post(&url)
        .body(vec![b'x'; 1024 * 1024 + 1])
        .send()
        .await
        .unwrap();
    assert_eq!(over.status(), 413);
    over.bytes().await.unwrap();
    let m = &observed_stage(&home, "rewrite", "notify", 2).await[1];
    assert_eq!(m["state"], "rewritten");
    assert_eq!(m["code"], "request_too_large");
    assert_eq!(m.get("digest"), Some(&Value::Null));
    assert!(sent.try_recv().is_err());
}

/// 不订阅 stream 也可双工：先收到完整响应，仍在传输的原始上传才是最终字节数与摘要的来源。
#[tokio::test]
async fn notify_only_waits_for_live_upload_accounting_after_response_end() {
    use futures_util::StreamExt as _;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let home = TestHome::new();
    let (up, listener) = mock_listener().await;
    let (tx, mut observed) = tokio::sync::mpsc::unbounded_channel();
    let app = axum::Router::new().fallback(move |req: axum::extract::Request| {
        let done = tx.clone();
        async move {
            let mut input = req.into_body().into_data_stream();
            let mut b = input.next().await.unwrap().unwrap().to_vec();
            tokio::spawn(async move {
                while let Some(part) = input.next().await {
                    b.extend_from_slice(&part.unwrap());
                }
                let _ = done.send(b);
            });
            "duplex-response"
        }
    });
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let cfg = String::from("delivery='forward'\nkeepalive.trigger='body_stream'\n")
        + &layer(&home, "observer", "'notify'", &["notify-digest"]);
    let (_, url) = running(&home, up, &cfg);
    let addr = url.strip_prefix("http://").unwrap();
    let mut tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
    tcp.write_all(format!("POST / HTTP/1.1\r\nHost: {addr}\r\nTransfer-Encoding: chunked\r\n\r\na\r\n{{\"stream\":\r\n").as_bytes()).await.unwrap();
    let mut received = vec![];
    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        let mut buf = [0; 512];
        while !received.ends_with(b"duplex-response") {
            let n = tcp.read(&mut buf).await.unwrap();
            assert!(n > 0);
            received.extend_from_slice(&buf[..n]);
        }
    })
    .await
    .unwrap();
    tcp.write_all(b"5\r\ntrue}\r\n0\r\n\r\n").await.unwrap();
    let actual = tokio::time::timeout(std::time::Duration::from_secs(20), observed.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(actual, br#"{"stream":true}"#);
    let end = &observed_stage(&home, "observer", "notify", 1).await[0];
    assert_eq!(
        end["digest"],
        "cf8db77a15bbcc48ba1b0836d18c5875053b7f036993b5ef66d2f1ec8150dd3a"
    );
    assert_eq!(end["client"]["stream"], true);
    let record = request_detail(&home, &url, end["request_id"].as_str().unwrap()).await;
    assert_eq!(record["record"]["request_bytes"], actual.len());
}

/// stream 在 open 已提交头；损坏的非流式实体保持原状态码，但绝不交付空正文、旧长度或上游凭据。
#[tokio::test]
async fn committed_json_stream_returns_sanitized_client_error() {
    let home = TestHome::new();
    let (up, listener) = mock_listener().await;
    let app = axum::Router::new().fallback(|| async {
        (
            [
                ("content-type", "application/octet-stream"),
                ("set-cookie", "private-fixture"),
                ("x-api-key", "private-fixture"),
            ],
            "{}",
        )
    });
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let cfg = official_layer(
        "convert",
        "'request','stream'",
        &[
            "convert",
            "--from",
            "anthropic_messages",
            "--to",
            "openai_chat",
        ],
    );
    let (c, url) = running(&home, up, &cfg);
    let r=c.post(format!("{url}/v1/messages")).json(&serde_json::json!({"model":"m","max_tokens":8,"messages":[{"role":"user","content":"hello"}]})).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["content-type"], "application/json");
    for key in ["x-api-key", "set-cookie"] {
        assert!(!r.headers().contains_key(key));
    }
    assert!(!r.headers().get("content-length").is_some_and(|v| v == "2"));
    let body: Value = r.json().await.unwrap();
    assert_eq!(body["type"], "error");
    assert_eq!(body["error"]["type"], "api_error");
}

/// 小实例的 3 MiB 回信容不下四次重复的 Responses 完成快照。转换层在仍收增量时正常报错，进程不熔断。
#[tokio::test]
async fn responses_snapshot_obeys_instance_reply_limit_without_breaker_failure() {
    use serde_json::json;
    let home = TestHome::new();
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    let delta = json!({"choices":[{"index":0,"delta":{"content":"x".repeat(4096)}}]});
    let bytes = format!("data: {delta}\n\n").repeat(208)
        + "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
    let app = axum::Router::new().fallback(move || {
        let bytes = bytes.clone();
        async move { ([("content-type", "text/event-stream")], bytes) }
    });
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let config = String::from("max_response_mb=1\ndelivery='forward'\n")
        + &official_layer(
            "convert",
            "'stream'",
            &[
                "convert",
                "--from",
                "openai_responses",
                "--to",
                "openai_chat",
            ],
        )
        + "breaker_failures=1\n";
    let (client, url) = running(&home, port, &config);
    let body = client
        .post(&url)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(body.contains("response.output_text.delta"));
    assert!(body.contains("response exceeds the configured limit"));
    assert_eq!(body.matches("event: error").count(), 1);
    assert!(!body.contains("response.completed"));
    assert_eq!(
        breaker(&home, &url).await,
        json!({"state":"closed","failures":0})
    );
}
