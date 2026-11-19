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
