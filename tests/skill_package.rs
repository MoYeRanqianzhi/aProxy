//! 用仓库真实的 Git 对象生成发布包，再从环回镜像安装；没有 Python 时明确报告跳过。
mod common;
use common::ReportExit as _;
use common::TestHome;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
};

/// 兼容 Windows 的 python 与 Unix 的 python3，不把打包失败当作未安装解释器。
fn python(h: &TestHome) -> Option<&'static str> {
    // Windows 原生子进程传入盘符路径，MSYS 的 Python 却按 POSIX 路径解析它。
    let probe = if cfg!(windows) {
        "import os,sys; sys.exit(sys.version_info.major != 3 or os.name != 'nt')"
    } else {
        "import sys; sys.exit(sys.version_info.major != 3)"
    };
    for exe in ["python", "python3"] {
        if h.cmd_with(exe)
            .args(["-c", probe])
            .status()
            .report_exit()
            .is_ok_and(|s| s.success())
        {
            return Some(exe);
        }
    }
    eprintln!("跳过 skill 打包检查：没有可读取本机路径的 Python 3");
    None
}
/// 通过脚本的实际入口生成文件；Rust 校验器也在临时 home 下运行。
fn package(h: &TestHome, exe: &str, dir: &str) -> PathBuf {
    let out = h.path().join(dir);
    let cmd = h
        .cmd_with(exe)
        .arg(".github/scripts/package-skills.py")
        .arg(env!("CARGO_PKG_VERSION"))
        .arg(&out)
        .args(["--validator", common::APROXY])
        .output()
        .report_exit()
        .unwrap();
    assert!(
        cmd.status.success(),
        "{}",
        String::from_utf8_lossy(&cmd.stderr)
    );
    out.join("aproxy-skills.zip")
}
/// 两次独立打包的整个容器与校验行必须相同，不只是解压后的文件内容相同。
#[test]
fn reproducible_zip_and_checksum() {
    let h = TestHome::new();
    let Some(py) = python(&h) else { return };
    let a = package(&h, py, "one");
    let b = package(&h, py, "two");
    let bytes = fs::read(&a).unwrap();
    assert_eq!(bytes, fs::read(&b).unwrap());
    let hex: String = Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    for path in [a, b] {
        assert_eq!(
            fs::read(path.with_extension("zip.sha256")).unwrap(),
            format!("{hex}  aproxy-skills.zip\n").as_bytes()
        );
    }
}
/// 所有已跟踪的文档逐字节等于 Git 对象，不能漏掉某个 skill 或把工作副本的 CRLF 装进去。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn packaged_index_installs_through_the_download_chain() {
    let h = TestHome::new();
    let Some(py) = python(&h) else { return };
    let zip = package(&h, py, "published");
    let bytes = fs::read(zip).unwrap();
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = axum::Router::new().route(
        "/aproxy-skills.zip",
        axum::routing::get(move || async move { bytes }),
    );
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    h.settings(&serde_json::json!({"watchdog":{"enabled":false},"update":{"download_chain":[{"url":format!("http://127.0.0.1:{port}/{{asset}}")}]}}).to_string());
    let installed = h
        .cmd()
        .args(["skill", "install", "--json"])
        .output()
        .report_exit()
        .unwrap();
    assert!(
        installed.status.success(),
        "{}",
        String::from_utf8_lossy(&installed.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&installed.stdout).unwrap();
    let result = h
        .cmd_with("git")
        .args(["ls-files", "--stage", "-z", "--", ".claude/skills/"])
        .output()
        .report_exit()
        .unwrap();
    assert!(result.status.success());
    let root = h.path().join("skills");
    let mut count = 0;
    for line in result.stdout.split(|&b| b == 0).filter(|s| !s.is_empty()) {
        let (meta, path) = std::str::from_utf8(line).unwrap().split_once('\t').unwrap();
        let oid = meta.split_whitespace().nth(1).unwrap();
        let raw = h
            .cmd_with("git")
            .args(["cat-file", "blob", oid])
            .output()
            .report_exit()
            .unwrap();
        assert!(raw.status.success());
        let rel = Path::new(path).strip_prefix(".claude/skills").unwrap();
        assert_eq!(fs::read(root.join(rel)).unwrap(), raw.stdout, "{path}");
        count += 1;
    }
    assert!(count > 0);
    assert_eq!(report["file_count"], count);
}

/// 同一份发布输入在 npm 重打时可凭已核验的包作证；版本不同或非法不能覆盖先前的产物。
#[test]
fn package_validation_precedes_publication() {
    let h = TestHome::new();
    let Some(py) = python(&h) else { return };
    let reference = package(&h, py, "reference");
    let out = h.path().join("npm");
    let run = |version: &str, flag: &str, validator: &Path| {
        h.cmd_with(py)
            .arg(".github/scripts/package-skills.py")
            .arg(version)
            .arg(&out)
            .arg(flag)
            .arg(validator)
            .output()
            .report_exit()
            .unwrap()
    };
    assert!(
        run(env!("CARGO_PKG_VERSION"), "--verified-archive", &reference)
            .status
            .success()
    );
    let initial = fs::read(out.join("aproxy-skills.zip")).unwrap();
    assert!(
        !run(
            if env!("CARGO_PKG_VERSION_MAJOR") == "0" {
                "1.0.0"
            } else {
                "0.0.0"
            },
            "--verified-archive",
            &reference
        )
        .status
        .success()
    );
    assert!(
        !run("not-a-version", "--validator", Path::new(common::APROXY))
            .status
            .success()
    );
    assert_eq!(fs::read(out.join("aproxy-skills.zip")).unwrap(), initial);
}

/// 独立索引将 CRLF 对象、工作副本与链接类型分开控制，不依赖当前仓库或墙钟时间。
#[test]
fn fixture_checks_git_objects_attributes_and_zip_metadata() {
    use std::io::Read;
    let h = TestHome::new();
    let Some(py) = python(&h) else { return };
    let root = h.path().join("repo");
    let dir = root.join(".claude/skills/s");
    fs::create_dir_all(&dir).unwrap();
    let script = root.join(".github/scripts/package-skills.py");
    fs::create_dir_all(script.parent().unwrap()).unwrap();
    fs::copy(".github/scripts/package-skills.py", &script).unwrap();
    fs::copy(".gitattributes", root.join(".gitattributes")).unwrap();
    let git = |args: &[&str]| {
        let o = h
            .cmd_with("git")
            .current_dir(&root)
            .args(args)
            .output()
            .report_exit()
            .unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8(o.stdout).unwrap()
    };
    git(&["init", "-q"]);
    fs::write(dir.join("z"), b"LF\r\n").unwrap();
    git(&["add", "--", ".gitattributes", ".claude/skills/s/z"]);
    assert!(
        git(&["check-attr", "eol", "--", ".claude/skills/s/z"])
            .trim_end()
            .ends_with("eol: lf")
    );
    // 故意绕开 clean 过滤器构造 CRLF blob，保证打包器读对象而非磁盘上的文件。
    let oid = git(&["hash-object", "-w", "--no-filters", ".claude/skills/s/z"]);
    git(&[
        "update-index",
        "--add",
        "--cacheinfo",
        &format!("100755,{},.claude/skills/s/a", oid.trim()),
    ]);
    fs::write(dir.join("a"), b"changed").unwrap();
    fs::write(dir.join("z"), b"changed").unwrap();
    fs::write(dir.join("untracked"), b"ignore").unwrap();
    let out = h.path().join("out");
    let run = || {
        h.cmd_with(py)
            .env("PYTHONIOENCODING", "utf-8")
            .arg(&script)
            .arg(env!("CARGO_PKG_VERSION"))
            .arg(&out)
            .args(["--validator", common::APROXY])
            .output()
            .report_exit()
            .unwrap()
    };
    let result = run();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let path = out.join("aproxy-skills.zip");
    let saved = fs::read(&path).unwrap();
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(&saved)).unwrap();
    assert_eq!(z.len(), 3);
    for (index, key) in ["manifest.json", "s/a", "s/z"].into_iter().enumerate() {
        let mut e = z.by_index(index).unwrap();
        assert_eq!(e.name().unwrap(), key);
        assert_eq!(e.unix_mode(), Some(0o100644));
        assert_eq!(e.compression(), zip::CompressionMethod::Stored);
        let dt = e.last_modified().unwrap();
        assert_eq!(
            (
                dt.year(),
                dt.month(),
                dt.day(),
                dt.hour(),
                dt.minute(),
                dt.second()
            ),
            (1980, 1, 1, 0, 0, 0)
        );
        let mut body = Vec::new();
        e.read_to_end(&mut body).unwrap();
        if key == "s/a" {
            assert_eq!(body, b"LF\r\n");
        } else if key == "s/z" {
            assert_eq!(body, b"LF\n");
        }
    }
    git(&[
        "update-index",
        "--add",
        "--cacheinfo",
        &format!("120000,{},.claude/skills/s/link", oid.trim()),
    ]);
    let result = run();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("普通文件"));
    assert_eq!(fs::read(path).unwrap(), saved);
}
