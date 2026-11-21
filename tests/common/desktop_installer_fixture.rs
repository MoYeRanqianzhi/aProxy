//! 安装流夹具：只在测试提供的临时 home 内读写同步标记，永不安装、联网或拉起实例。
use std::{env, fs, io::Write, path::PathBuf};
fn main() {
    let home = PathBuf::from(env::var_os("APROXY_HOME").expect("测试主目录"));
    assert_eq!(
        fs::canonicalize(&home).unwrap(),
        env::current_dir().unwrap().canonicalize().unwrap()
    );
    let args = env::args().skip(1).collect::<Vec<_>>();
    let mode = fs::read_to_string(home.join("fixture.mode")).unwrap_or_default();
    if args == ["install", "--check", "--json"] || args == ["install", "--check", "--json", "--pre"]
    {
        println!(
            "{}",
            r#"{"current":"1.0.0","latest":"1.1.0","update_available":true,"channel":"stable","variant":"headless","published_at":"2026-01-01T00:00:00Z","url":"http://private.invalid/secret"}"#
        );
        return;
    }
    assert_eq!(args, ["install", "--json", "1.1.0"]);
    for line in [
        r#"{"event":"plan","ports":[80]}"#,
        r#"{"event":"phase","phase":"future","at":1}"#,
        r#"{"event":"progress","bytes":18446744073709551615,"total":null}"#,
        r#"{"event":"instance","port":80,"outcome":"rolled_back"}"#,
    ] {
        println!("{line}");
    }
    if mode == "hold" {
        fs::write(home.join("fixture.started"), "").unwrap();
        for _ in 0..2000 {
            if home.join("fixture.release").is_file() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
    }
    if mode == "oversize" {
        println!("{}", "x".repeat(1024 * 1024 + 2));
    }
    if mode == "future-error" {
        println!(
            "{}",
            r#"{"event":"result","outcome":"failed","error":{"code":"future_private_code","details":{"value":"private-exception"}}}"#
        );
        std::process::exit(1)
    }
    if mode == "failed" {
        println!(
            "{}",
            r#"{"event":"result","outcome":"failed","error":{"code":"install_restart_failed","message":"private-exception","details":{"port":80}}}"#
        );
        std::process::exit(1)
    }
    if mode != "missing" {
        println!(
            "{}",
            r#"{"event":"result","outcome":"installed","version":"1.1.0"}"#
        );
    }
    if mode == "duplicate" {
        println!("{}", r#"{"event":"result","outcome":"installed"}"#);
    }
    std::io::stdout().flush().unwrap();
    if mode == "wrong-exit" {
        std::process::exit(2)
    }
}
