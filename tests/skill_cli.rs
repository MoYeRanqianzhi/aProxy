//! 在隔离 home 用环回镜像运行真正的 CLI，验证文档的原始字节、退出码与安装支线。
mod common;
use common::{TestHome, free_port};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Cursor, Write},
    process::Output,
};

/// 预发布测试也必须向镜像请求自身的版本而非 latest。
const VERSION: &str = env!("CARGO_PKG_VERSION");
/// 刻意混用有无尾换行、CRLF 和 UTF-8，输出不能做文本规范化。
const DOCS: [(&str, &str); 3] = [
    ("a/SKILL.md", "# 一"),
    ("a/refs/b.md", "\r\nB\r\n"),
    ("b/SKILL.md", "# b\n"),
];
/// 独立组包，既能正常回包，也能在发送正文之前暂停镜像请求。
fn archive(version: &str) -> Vec<u8> {
    let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default();
    let mut files = json!({});
    for (key, text) in DOCS {
        w.start_file(key, opts).unwrap();
        w.write_all(text.as_bytes()).unwrap();
        files[key] = json!({"size":text.len(),"sha256":Sha256::digest(text.as_bytes()).iter().map(|b|format!("{b:02x}")).collect::<String>()});
    }
    w.start_file("manifest.json", opts).unwrap();
    w.write_all(&serde_json::to_vec(&json!({"schema":1,"version":version,"files":files})).unwrap())
        .unwrap();
    w.finish().unwrap().into_inner()
}
/// 只有精确版本和总包路径返回文档，其他请求均在本机立即返回 404。
async fn mirror(h: &TestHome, version: &str) {
    let bytes = archive(version);
    let app = axum::Router::new().route(
        &format!("/{VERSION}/aproxy-skills.zip"),
        axum::routing::get(move || async move { bytes }),
    );
    serve(h, app).await;
}
/// 每个 home 使用自己的环回镜像，不回退到外网的下载链。
async fn serve(h: &TestHome, app: axum::Router) {
    let port = free_port();
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
        .await
        .unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    h.settings(&json!({"watchdog":{"enabled":false},"update":{"download_chain":[{"url":format!("http://127.0.0.1:{port}/{{version}}/{{asset}}")}]}}).to_string());
}
/// 对 stdout 与 stderr 分别断言，避免看似正确的输出夹带诊断。
fn run(h: &TestHome, args: &[&str], code: i32) -> Output {
    let o = h.cmd().args(args).output().unwrap();
    assert_eq!(
        o.status.code(),
        Some(code),
        "{args:?}: {} {}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    o
}
/// 安装、别名、同版本旧写法与读取结果均经过子进程与下载链，不直接预置安装目录。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn roundtrip_and_aliases() {
    let h = TestHome::new();
    mirror(&h, VERSION).await;
    let install = run(&h, &["skill", "install", "--json"], 0).stdout;
    let doc: Value = serde_json::from_slice(&install).unwrap();
    assert_eq!(doc["version"], VERSION);
    assert_eq!(doc["file_count"], DOCS.len());
    assert_eq!(doc["result"], "installed");
    assert_eq!(run(&h, &["skill", "upgrade", "--json"], 0).stdout, install);
    let plain = run(&h, &["skill", "install"], 0).stdout;
    assert_eq!(
        run(&h, &["install", VERSION, "--skills-only"], 0).stdout,
        plain
    );
    assert!(
        !String::from_utf8(run(&h, &["install", "--help"], 0).stdout)
            .unwrap()
            .contains("--skills-only")
    );
    let bad = run(&h, &["install", "1.0.0", "--skills-only"], 2);
    assert!(String::from_utf8(bad.stderr).unwrap().contains("先升级"));
    run(&h, &["skill", "install", VERSION], 2);
    let list: Value =
        serde_json::from_slice(&run(&h, &["skill", "list", "--json"], 0).stdout).unwrap();
    assert_eq!(list["state"], "installed");
    let mut count = 0;
    for skill in list["skills"].as_array().unwrap() {
        let name = skill["name"].as_str().unwrap();
        assert_eq!(skill["version"], VERSION);
        assert_eq!(skill["matches_binary"], true);
        let results =
            String::from_utf8(run(&h, &["skill", "show", name, "--files"], 0).stdout).unwrap();
        let listed: Vec<_> = skill["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(results.lines().collect::<Vec<_>>(), listed);
        for file in listed {
            let key = format!("{name}/{file}");
            let expected = DOCS.iter().find(|(k, _)| *k == key).unwrap().1;
            assert_eq!(
                run(&h, &["skill", "show", name, file], 0).stdout,
                expected.as_bytes()
            );
            if file == "SKILL.md" {
                assert_eq!(
                    run(&h, &["skill", "show", name], 0).stdout,
                    expected.as_bytes()
                );
            }
            count += 1;
        }
    }
    assert_eq!(count, DOCS.len());
    fs::write(h.path().join("skills/a/SKILL.md"), "SECRET").unwrap();
    for args in [
        &["skill", "show", "a"][..],
        &["skill", "show", "a", "MISSING"],
        &["skill", "show", "unknown", "--files"],
    ] {
        let o = run(&h, args, 1);
        assert!(
            o.stdout.is_empty()
                && !o.stderr.is_empty()
                && !String::from_utf8(o.stderr).unwrap().contains("SECRET")
        );
    }
}
/// 空安装与旧安装有各自的可执行提示；坏包不发布标记，主程序的安装不因此失败。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_legacy_and_version_mismatch() {
    let h = TestHome::new();
    mirror(&h, "1.0.0").await;
    for (code, state) in [(4, "not_installed"), (1, "without_manifest")] {
        if code == 1 {
            fs::create_dir_all(h.path().join("skills/old")).unwrap();
        }
        let doc: Value =
            serde_json::from_slice(&run(&h, &["skill", "list", "--json"], code).stdout).unwrap();
        assert_eq!(doc["state"], state);
        assert!(
            String::from_utf8(run(&h, &["skill", "list"], code).stdout)
                .unwrap()
                .contains("aproxy skill install")
        );
        let o = run(&h, &["skill", "show", "old"], 1);
        assert!(
            o.stdout.is_empty()
                && String::from_utf8(o.stderr)
                    .unwrap()
                    .contains("aproxy skill install")
        );
    }
    let fail = run(&h, &["skill", "install"], 1);
    assert!(String::from_utf8(fail.stderr).unwrap().contains("版本"));
    let o = run(&h, &["install", "--from", common::APROXY], 0);
    assert!(String::from_utf8_lossy(&o.stderr).contains("aproxy skill install"));
    assert!(!h.path().join("skills/manifest.json").exists());
}
/// 常规二进制安装仍刷新配套文档，不能只让独立的 skill 命令工作。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn binary_install_also_refreshes_skills() {
    let h = TestHome::new();
    mirror(&h, VERSION).await;
    run(&h, &["install", "--from", common::APROXY], 0);
    assert_eq!(
        run(&h, &["skill", "show", "a"], 0).stdout,
        DOCS[0].1.as_bytes()
    );
}

/// 既检查整棵实际命令树（含隐藏入口与 help），也检查进程实际用这棵树输出帮助。
#[test]
fn every_help_ends_in_the_skill_pointer() {
    const LINE: &str =
        "Agents: run 'aproxy skill list' and 'aproxy skill show <name>' for full docs.";
    /// 逐层查长帮助，避免只改根命令却漏掉孙命令。
    fn walk(c: &mut clap::Command) {
        let help = c.render_long_help().to_string();
        assert!(help.trim_end().ends_with(LINE), "{}: {help}", c.get_name());
        for child in c.get_subcommands_mut() {
            walk(child);
        }
    }
    walk(&mut aproxy::cli::command_tree());
    let h = TestHome::new();
    for args in [
        &["--help"][..],
        &["skill", "upgrade", "--help"],
        &["help", "skill", "show"],
        &["__serve", "--help"],
    ] {
        assert!(
            String::from_utf8(run(&h, args, 0).stdout)
                .unwrap()
                .trim_end()
                .ends_with(LINE)
        );
    }
}

/// 让普通安装的支线停在下载中，再从另一个进程尝试单独更新：锁必须在首次请求之前就持有。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn binary_and_standalone_install_share_the_download_lock() {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };
    let h = TestHome::new();
    let count = Arc::new(AtomicUsize::new(0));
    let arrived = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let app = axum::Router::new().route(
        &format!("/{VERSION}/aproxy-skills.zip"),
        axum::routing::get({
            let count = count.clone();
            let arrived = arrived.clone();
            let release = release.clone();
            move || {
                let count = count.clone();
                let arrived = arrived.clone();
                let release = release.clone();
                async move {
                    if count.fetch_add(1, Ordering::SeqCst) == 0 {
                        arrived.notify_one();
                        let _ =
                            tokio::time::timeout(Duration::from_secs(20), release.notified()).await;
                    }
                    archive(VERSION)
                }
            }
        }),
    );
    serve(&h, app).await;
    let first = tokio::process::Command::from(h.cmd())
        .kill_on_drop(true)
        .args(["install", "--from", common::APROXY])
        .stderr(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(15), arrived.notified())
        .await
        .unwrap();
    let contender = h.cmd().args(["skill", "upgrade"]).output().unwrap();
    release.notify_one();
    let first = first.wait_with_output().await.unwrap();
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert_eq!(contender.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&contender.stderr).contains("另一个进程"));
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(
        run(&h, &["skill", "show", "a"], 0).stdout,
        DOCS[0].1.as_bytes()
    );
}
