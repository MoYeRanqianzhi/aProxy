//! aproxy-format 端到端测试：真实二进制 × 真实 switchyard 转换。
//!
//! 覆盖单测够不到的两条路径：
//! 1. **跨协议转换真的发生**（anthropic_messages ↔ openai_chat，单测全是
//!    同协议直通——switchyard 集成未经真实执行是此前最大的验证缺口）
//! 2. run 循环的 persistent 语义（逐行处理、跨请求轮换连续、error 行不退出）

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

struct FormatProc {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Drop for FormatProc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn spawn_run(args: &[&str]) -> FormatProc {
    let mut child = Command::new(env!("CARGO_BIN_EXE_aproxy-format"))
        .arg("run")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("aproxy-format spawn 失败");
    let stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    FormatProc {
        child,
        stdin,
        stdout: BufReader::new(stdout),
    }
}

fn send_and_read(proc: &mut FormatProc, line: &str) -> serde_json::Value {
    proc.stdin.write_all(line.as_bytes()).unwrap();
    proc.stdin.write_all(b"\n").unwrap();
    proc.stdin.flush().unwrap();
    let mut out = String::new();
    proc.stdout.read_line(&mut out).unwrap();
    assert!(!out.is_empty(), "format 未回行（EOF 过早）");
    serde_json::from_str(out.trim()).expect("回行非 JSON")
}

fn agg_config_for_cross_format() -> String {
    // 客户端显式 openai_chat，渠道 anthropic：请求方向做真实跨协议转换
    r#"
client_format = "openai_chat"

[[channel]]
name = "ant"
format = "anthropic_messages"
url = "https://ant.example.com/v1/messages"
keys = ["sk-cross-1"]
"#
    .to_string()
}

fn envelope_line(body: &str, url: &str) -> String {
    let env = serde_json::json!({
        "method": "POST",
        "url": url,
        "headers": {"content-type": "application/json"},
        "body": body,
        "worker_id": 0,
        "extra": "",
    });
    env.to_string()
}

#[test]
fn cross_format_request_conversion_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("agg.toml");
    std::fs::write(&cfg_path, agg_config_for_cross_format()).unwrap();
    let cfg_arg = format!("--config={}", cfg_path.display());

    let mut proc = spawn_run(&[&cfg_arg]);
    // 输入带 system+user 两条消息（验证多消息保留，不预设 switchyard 的
    // system 放置形态——顶层字段或 messages 内条目均为合法转换产物）
    let req = r#"{"model":"gpt-4o","messages":[{"role":"system","content":"be nice"},{"role":"user","content":"hi"}],"max_tokens":30}"#;
    let out = send_and_read(
        &mut proc,
        &envelope_line(req, "https://client.example.com/v1/chat/completions"),
    );

    // 转换后信封：url 改写到渠道、鉴权头为 anthropic 形态
    assert_eq!(out["url"], "https://ant.example.com/v1/messages");
    assert_eq!(out["headers"]["x-api-key"], "sk-cross-1");
    assert!(out["headers"].get("authorization").is_none());
    assert_eq!(out["headers"]["anthropic-version"], "2023-06-01");

    // **body 完成了 anthropic_messages 形态转换**（跨协议转换发生的实锤）：
    // openai 的 messages → anthropic 的 messages + max_tokens 保留
    let body: serde_json::Value = serde_json::from_str(out["body"].as_str().unwrap()).unwrap();
    assert_eq!(body["model"], "gpt-4o");
    assert_eq!(body["max_tokens"], 30);
    let msgs = body["messages"].as_array().expect("转换产物应有 messages");
    assert_eq!(msgs.len(), 1, "user 消息保留: {body}");
    assert_eq!(
        body["system"], "be nice",
        "openai 的 system role 消息应提升为 anthropic 顶层 system 字段: {body}"
    );
}

#[test]
fn cross_format_response_conversion_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("agg.toml");
    std::fs::write(&cfg_path, agg_config_for_cross_format()).unwrap();
    let cfg_arg = format!("--config={}", cfg_path.display());

    let mut proc = spawn_run(&[&cfg_arg]);
    // 响应侧信封（无 method）：url 反查渠道 → anthropic 响应转回 openai_chat
    let resp = r#"{"role":"assistant","content":[{"type":"text","text":"hello"}],"model":"gpt-4o","stop_reason":"end_turn"}"#;
    let out = send_and_read(
        &mut proc,
        &serde_json::json!({
            "url": "https://ant.example.com/v1/messages",
            "headers": {"content-type": "application/json", "x-api-key": "sk-upstream-real"},
            "body": resp,
        })
        .to_string(),
    );
    assert_eq!(out["url"], "https://ant.example.com/v1/messages");
    assert!(
        out["headers"].get("x-api-key").is_none(),
        "上游 key 不得回传: {out}"
    );
    let body: serde_json::Value = serde_json::from_str(out["body"].as_str().unwrap()).unwrap();
    assert!(
        body.get("choices").is_some(),
        "响应应转回 openai_chat 形态: {body}"
    );
}

#[test]
fn run_loop_survives_error_lines_and_keeps_rotating() {
    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("agg.toml");
    std::fs::write(
        &cfg_path,
        r#"
client_format = "auto"

[[channel]]
name = "ant"
format = "anthropic_messages"
url = "https://ant.example.com"
keys = ["k1", "k2"]
models = ["claude-*"]
"#,
    )
    .unwrap();
    let cfg_arg = format!("--config={}", cfg_path.display());

    let mut proc = spawn_run(&[&cfg_arg]);
    // 1. 未命中模型 → error 行（进程不退）
    let miss = send_and_read(
        &mut proc,
        &envelope_line(
            r#"{"system":"s","messages":[],"max_tokens":1,"model":"llama-nope"}"#,
            "https://c.example.com/v1/messages",
        ),
    );
    assert!(
        miss["error"]
            .as_str()
            .unwrap_or_default()
            .contains("未命中"),
        "error 行应带未命中原因: {miss}"
    );
    // 2. 后续请求照常（error 行不影响 worker 状态）且轮换跨请求连续
    let a = send_and_read(
        &mut proc,
        &envelope_line(
            r#"{"system":"s","messages":[],"max_tokens":1,"model":"claude-3"}"#,
            "https://c.example.com/v1/messages",
        ),
    );
    let b = send_and_read(
        &mut proc,
        &envelope_line(
            r#"{"system":"s","messages":[],"max_tokens":1,"model":"claude-3"}"#,
            "https://c.example.com/v1/messages",
        ),
    );
    assert_eq!(a["headers"]["x-api-key"], "k1");
    assert_eq!(b["headers"]["x-api-key"], "k2", "error 行后轮换序列应连续");
}
