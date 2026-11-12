//! 测试源码检查：测试拉起的每一个 aproxy 进程都必须经 `common::TestHome` 的 `cmd()` 或
//! `cmd_with()` 拉起。
//!
//! 为什么要拦：TestHome 是隔离环境的唯一注入点——APROXY_HOME（自己新建的临时目录）、
//! `APROXY_LANG=en`、清掉的代理变量，以及测试结束时按端口 stop、按台账兜底的清理。绕过它
//! 直接 `Command::new(aproxy)` 的测试，漏注入 APROXY_HOME 就会落到开发者真实的 `~/.aproxy`：
//! 写真实的注册表与日志，`stop` 还可能停掉用户正在用的实例（跑测试的会话本身可能就经它连
//! API）。这类错误编译期发现不了，所以用这个测试扫描源码。
//!
//! 规则（`tests/common/` 与本文件之外的全部 `tests/**/*.rs`，注释行不算）：
//! - `Command::new(...)` 的参数只能是 [`EXTERNAL_PROGRAMS`] 里的字面量（测试自己的哑进程、
//!   查进程表、按 pid 结束测试自己的子进程），否则要在 [`ALLOWED`] 里登记并写明原因；
//! - 不得自己给子进程设置或删除 APROXY_HOME（`.env(` / `.env_remove(` 的第一个参数是它）；
//! - 不得在进程级设置 APROXY_HOME（`set_var`），[`ALLOWED`] 里登记的除外。

use std::path::{Path, PathBuf};

/// `Command::new` 可以直接拉起的外部程序（都不是 aproxy）
const EXTERNAL_PROGRAMS: &[&str] = &["ping", "sleep", "tasklist", "ps", "taskkill", "kill"];

/// 例外：(相对 tests/ 的路径, 该行包含的片段, 原因)。整份文件的例外片段写空串。
const ALLOWED: &[(&str, &str, &str)] = &[
    (
        "force_contract.rs",
        "",
        "永久兼容契约 C1–C8 的契约测试：断言与拉起方式一起冻结，一个字都不改",
    ),
    (
        "install_force.rs",
        "Command::new(&stray_launch)",
        "拉起的是本测试程序自己的副本（模拟从 bin 运行、不在台账里的进程），不是 aproxy",
    ),
    (
        "install_flow_lib.rs",
        "std::env::set_var(\"APROXY_HOME\"",
        "库层测试在进程内调用安装流程，库函数分离拉起的守护只继承测试进程的环境，\
         APROXY_HOME 只能设在进程上；值都是 TestHome 的路径",
    ),
];

/// 本文件自己（规则里的片段以字符串出现在这里，不扫描）与公共设施所在的目录
const SELF_FILE: &str = "test_home_check.rs";
const COMMON_DIR: &str = "common";

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == COMMON_DIR) {
                continue;
            }
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs")
            && path.file_name().is_some_and(|n| n != SELF_FILE)
        {
            out.push(path);
        }
    }
}

/// `rel` 文件的这一行是否在例外里
fn allowed(rel: &str, line: &str) -> bool {
    ALLOWED
        .iter()
        .any(|(file, snippet, _)| *file == rel && line.contains(snippet))
}

/// 一行里违反规则的写法（空 = 没有）
fn violations_in(line: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = line;
    while let Some(at) = rest.find("Command::new(") {
        let arg = &rest[at + "Command::new(".len()..];
        let literal = arg
            .strip_prefix('"')
            .and_then(|a| a.split_once('"'))
            .map(|(name, _)| name);
        if !literal.is_some_and(|name| EXTERNAL_PROGRAMS.contains(&name)) {
            found.push("Command::new 的参数不是允许的外部程序".to_string());
        }
        rest = arg;
    }
    // 去掉空白后取调用的第一个参数：字面量 "APROXY_HOME"，或以 HOME_ENV 结尾的常量路径
    // （`HOME_ENV`、`aproxy::settings::HOME_ENV`）
    let compact: String = line.chars().filter(|c| !c.is_whitespace()).collect();
    for call in [".env(", ".env_remove(", "set_var("] {
        let mut rest = compact.as_str();
        while let Some(at) = rest.find(call) {
            let arg = &rest[at + call.len()..];
            let first = arg.split([',', ')']).next().unwrap_or("");
            if first == "\"APROXY_HOME\"" || first.ends_with("HOME_ENV") {
                found.push(format!("{call}{first}) 自己处理 APROXY_HOME"));
            }
            rest = arg;
        }
    }
    found
}

#[test]
fn every_aproxy_child_process_is_spawned_through_test_home() {
    let tests_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut files = Vec::new();
    rust_sources(&tests_dir, &mut files);
    assert!(files.len() > 5, "扫描到的测试文件太少，路径不对？{files:?}");

    let mut problems = Vec::new();
    for path in &files {
        let rel = path
            .strip_prefix(&tests_dir)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let text = std::fs::read_to_string(path).unwrap();
        for (i, line) in text.lines().enumerate() {
            if line.trim_start().starts_with("//") || allowed(&rel, line) {
                continue;
            }
            for what in violations_in(line) {
                problems.push(format!("tests/{rel}:{}: {what}: {}", i + 1, line.trim()));
            }
        }
    }
    assert!(
        problems.is_empty(),
        "下列写法绕过了 common::TestHome（改用 TestHome::cmd / cmd_with；确实要例外的，登记进 \
         ALLOWED 并写明原因）：\n{}",
        problems.join("\n")
    );
}

/// 检查本身的正反例：规则真的认得出违规写法，也不误报允许的写法
#[test]
fn the_check_recognizes_what_it_should() {
    for bad in [
        r#"let out = Command::new(env!("CARGO_BIN_EXE_aproxy"))"#,
        "let mut child = std::process::Command::new(&bin)",
        r#"Command::new("aproxy")"#,
        r#"    .env("APROXY_HOME", home.path())"#,
        "cmd.env(aproxy::settings::HOME_ENV, &home)",
        r#"cmd.env_remove("APROXY_HOME")"#,
        r#"unsafe { std::env::set_var("APROXY_HOME", dir.path()) };"#,
    ] {
        assert!(!violations_in(bad).is_empty(), "应判为违规: {bad}");
    }
    for good in [
        r#"let dummy = Command::new("ping")"#,
        r#"let _ = Command::new("taskkill")"#,
        r#"Command::new("kill").args(["-9", &pid.to_string()])"#,
        "let out = home.cmd().arg(\"status\").output().unwrap();",
        r#"home.cmd_with(&bin).env("APROXY_WATCHDOG_SCAN_SECS", "1")"#,
    ] {
        assert!(violations_in(good).is_empty(), "不应判为违规: {good}");
    }
    assert!(allowed("force_contract.rs", "任何一行"));
    assert!(!allowed(
        "install_flow.rs",
        r#"std::env::set_var("APROXY_HOME", p)"#
    ));
}
