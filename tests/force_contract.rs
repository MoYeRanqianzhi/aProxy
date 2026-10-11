//! 永久兼容契约 C1–C8（强制安装）的钉子：台账的位置与 JSON 格式、三个平台的启动
//! 标记、唯一中间名、安装锁的位置、`--version` 首行、发布清单、命令行写法与退出码。
//! 设计全文见 `.agents/plan/release-0.2.0.md` 的「W2b 强制安装：永久兼容契约」一节。
//!
//! **改这些测试就是破坏契约。** 从 0.2.0 起，任意版本的 `aproxy install --force` 都要能
//! 把任意版本装进同一个 home、停掉任意版本启动的进程；这里每一条断言都是别的版本已经
//! 写进二进制、收不回来的假设。某条断言挡住了你的改动，该改的是你的改动，不是断言——
//! 契约只许追加可选字段（追加时在这里补新的断言，旧的一条不动）。
//!
//! 隔离：会运行 aproxy 的测试都把 APROXY_HOME 设为临时目录（见 [`temp_home`]）。

use aproxy::install::force::{self, Manifest, VersionLine};
use aproxy::ledger::{self, LedgerEntry};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// 新建一个临时 home，并确认它确实在临时目录里、不是用户真正在用的 `~/.aproxy`：
/// 这些测试会执行强制安装，home 指错了就会结束用户的进程、替换用户的二进制。
fn temp_home() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let real = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let home = real(dir.path());
    assert!(
        home.starts_with(real(&std::env::temp_dir())),
        "测试 home 必须在临时目录里: {}",
        home.display()
    );
    if let Some(user_home) = dirs::home_dir() {
        assert!(
            !home.starts_with(real(&user_home.join(".aproxy"))),
            "测试 home 绝不能是用户的 ~/.aproxy: {}",
            home.display()
        );
    }
    dir
}

/// 在隔离的 home 里运行 aproxy（标准输入为空、不是终端）。
fn aproxy_in(home: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_aproxy"))
        .args(args)
        .env("APROXY_HOME", home)
        .env("NO_PROXY", "127.0.0.1,localhost")
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

// ---------------------------------------------------------------------------
// C1、C2、C4：位置与名字
// ---------------------------------------------------------------------------

#[test]
fn c1_c2_c4_locations_and_names() {
    let home = Path::new("/h");
    // C2：bin 的位置与二进制的名字
    let exe = if cfg!(windows) {
        "aproxy.exe"
    } else {
        "aproxy"
    };
    assert_eq!(aproxy::install::swap::bin_dir_in(home), home.join("bin"));
    assert_eq!(
        aproxy::install::swap::bin_path_in(home),
        home.join("bin").join(exe)
    );
    // C4：安装状态目录与安装锁
    assert_eq!(aproxy::install::install_dir_in(home), home.join("install"));
    assert_eq!(aproxy::install::LOCK_FILE, "lock");
    assert_eq!(
        aproxy::install::lock_path_in(home),
        home.join("install").join("lock")
    );
    // C3：台账的位置与文件名
    assert_eq!(ledger::LEDGER_DIR, "ledger");
    assert_eq!(ledger::ledger_dir_in(home), home.join("ledger"));
    assert_eq!(
        ledger::entry_path_in(home, 4242),
        home.join("ledger").join("4242.json")
    );
}

#[test]
fn c2_unique_intermediate_names() {
    use aproxy::install::swap::{OLD_PREFIX, TMP_PREFIX, is_residue_name, unique_name};
    assert_eq!(OLD_PREFIX, "aproxy.old-");
    assert_eq!(TMP_PREFIX, "aproxy.tmp-");
    // `<前缀><纳秒时间戳>-<pid>[.exe]`
    for prefix in [OLD_PREFIX, TMP_PREFIX] {
        let name = unique_name(prefix);
        let rest = name.strip_prefix(prefix).unwrap();
        let rest = if cfg!(windows) {
            rest.strip_suffix(".exe").expect(&name)
        } else {
            rest
        };
        let (nanos, pid) = rest.split_once('-').expect(&name);
        assert!(nanos.parse::<u64>().is_ok(), "{name}");
        assert_eq!(pid, std::process::id().to_string(), "{name}");
        assert!(is_residue_name(&name), "{name}");
    }
    // 别的版本留下的中间文件，任何版本都认得、清得掉
    for name in [
        "aproxy.old-1760256000123456789-4242.exe",
        "aproxy.tmp-1760256000123456789-4242",
    ] {
        assert!(is_residue_name(name), "{name}");
    }
    for name in ["aproxy.exe", "aproxy", "aproxy.bat", "aproxy.old.exe"] {
        assert!(!is_residue_name(name), "{name}");
    }
}

// ---------------------------------------------------------------------------
// C3：台账
// ---------------------------------------------------------------------------

#[test]
fn c3_ledger_entry_json() {
    assert_eq!(ledger::LEDGER_FORMAT, 1);
    assert_eq!(ledger::ROLE_INSTALLER, "installer");
    assert_eq!(ledger::ROLE_WATCHDOG, "watchdog");
    assert_eq!(ledger::ROLE_INSTANCE, "instance");
    assert_eq!(ledger::ROLE_DESKTOP, "desktop");

    // 全部字段：落盘就是这一行，字段名、顺序、类型都冻结
    let full = r#"{"ledger":1,"pid":4242,"start":"w:133730123456789012","role":"instance","version":"0.2.0","port":12345,"label":"C:\\Users\\me\\.aproxy\\config.toml","exe":"C:\\Users\\me\\.aproxy\\bin\\aproxy.exe"}"#;
    let entry = LedgerEntry::parse(full).expect("完整条目可解析");
    assert_eq!(
        entry,
        LedgerEntry {
            ledger: 1,
            pid: 4242,
            start: "w:133730123456789012".into(),
            role: "instance".into(),
            version: "0.2.0".into(),
            port: Some(12345),
            label: Some(r"C:\Users\me\.aproxy\config.toml".into()),
            exe: Some(r"C:\Users\me\.aproxy\bin\aproxy.exe".into()),
        }
    );
    assert_eq!(entry.to_json(), full);

    // 可选字段缺省时不写出，也能读
    let minimal = r#"{"ledger":1,"pid":7,"start":"l:6f1c2d3e-0000-4000-8000-000000000001:123456","role":"watchdog","version":"0.2.0"}"#;
    let entry = LedgerEntry::parse(minimal).expect("最小条目可解析");
    assert_eq!(entry.port, None);
    assert_eq!(entry.label, None);
    assert_eq!(entry.exe, None);
    assert_eq!(entry.to_json(), minimal);
}

#[test]
fn c3_readers_tolerate_the_future_and_skip_garbage() {
    // 以后的版本追加的字段与角色：今天的读者照样读得出来，不认识的角色排在最后结束
    let future = r#"{"ledger":1,"pid":9,"start":"m:1760256000.5","role":"tray","version":"9.0.0","port":1,"since":"2031-01-01","nested":{"a":[1,2]}}"#;
    let entry = LedgerEntry::parse(future).expect("带未知字段与未知角色的条目可解析");
    assert_eq!(entry.role, "tray");
    assert_eq!(entry.port, Some(1));
    let order = |role: &str| {
        LedgerEntry {
            role: role.into(),
            ..entry.clone()
        }
        .stop_order()
    };
    // 安装者 → 看护者 → 实例 → 桌面界面 → 不认识的角色
    assert!(order("installer") < order("watchdog"));
    assert!(order("watchdog") < order("instance"));
    assert!(order("instance") < order("desktop"));
    assert!(order("desktop") < order("tray"));

    // 缺必填字段、类型不对、不是 JSON：解析不了（读者跳过，不当成条目）
    for bad in [
        r#"{"pid":1,"start":"w:1","role":"instance","version":"0.2.0"}"#,
        r#"{"ledger":1,"pid":"1","start":"w:1","role":"instance","version":"0.2.0"}"#,
        r#"{"ledger":1,"pid":1,"role":"instance","version":"0.2.0"}"#,
        "not json",
    ] {
        assert_eq!(LedgerEntry::parse(bad), None, "{bad}");
    }

    // 读者只看 `*.json`：写到一半的 `<pid>.json.tmp` 与解析不了的文件都跳过，且不删
    let home = temp_home();
    let dir = ledger::ledger_dir_in(home.path());
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("9.json"), future).unwrap();
    std::fs::write(
        dir.join("10.json.tmp"),
        future.replace("\"pid\":9", "\"pid\":10"),
    )
    .unwrap();
    std::fs::write(dir.join("11.json"), "{ half").unwrap();
    let found = ledger::read_all_in(home.path());
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].entry.pid, 9);
    assert!(dir.join("11.json").exists() && dir.join("10.json.tmp").exists());
}

#[test]
fn c3_start_marker_formats() {
    assert_eq!(ledger::START_WINDOWS, "w:");
    assert_eq!(ledger::START_LINUX, "l:");
    assert_eq!(ledger::START_MACOS, "m:");
    // 本平台的推导方式：本进程的标记现算两次相同，且是冻结的形状
    #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
    {
        let marker = ledger::start_marker(std::process::id()).expect("本进程的启动标记可查");
        assert_eq!(
            ledger::start_marker(std::process::id()).as_deref(),
            Some(marker.as_str())
        );
        let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
        // Windows：`w:<创建时间 FILETIME 的 64 位无符号十进制>`
        #[cfg(windows)]
        assert!(marker.strip_prefix("w:").is_some_and(digits), "{marker}");
        // Linux：`l:<boot_id 原文>:<stat 第 22 字段>`，boot_id 是带连字符的小写 UUID
        #[cfg(target_os = "linux")]
        {
            let (boot_id, starttime) = marker
                .strip_prefix("l:")
                .and_then(|rest| rest.rsplit_once(':'))
                .expect(&marker);
            let expected = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").unwrap();
            assert_eq!(boot_id, expected.trim(), "{marker}");
            assert!(digits(starttime), "{marker}");
        }
        // macOS：`m:<pbi_start_tvsec>.<pbi_start_tvusec>`，两段都是不补零的十进制
        #[cfg(target_os = "macos")]
        {
            let (sec, usec) = marker
                .strip_prefix("m:")
                .and_then(|rest| rest.split_once('.'))
                .expect(&marker);
            assert!(digits(sec) && digits(usec), "{marker}");
            assert!(usec == "0" || !usec.starts_with('0'), "{marker}");
            assert!(usec.parse::<u32>().unwrap() < 1_000_000, "{marker}");
        }
        // 自己的条目判为在运行；同一个 pid 配上别的标记，就不是它
        let mine = LedgerEntry::for_current_process(ledger::ROLE_INSTALLER, None, None).unwrap();
        assert!(ledger::is_alive(&mine));
        let stranger = LedgerEntry {
            start: format!("{marker}0"),
            ..mine
        };
        assert!(!ledger::is_alive(&stranger));
    }
}

// ---------------------------------------------------------------------------
// C5：`--version` 首行
// ---------------------------------------------------------------------------

#[test]
fn c5_version_line() {
    let parse = |line: &str| VersionLine::parse(line);
    assert_eq!(
        parse("aproxy v0.2.0 (desktop; x86_64-pc-windows-msvc; x86-64-v3)"),
        Some(VersionLine {
            version: "0.2.0".into(),
            variant: Some("desktop".into()),
            target: Some("x86_64-pc-windows-msvc".into()),
            x86_64_v3: true,
        })
    );
    assert_eq!(
        parse("aproxy v0.2.0-alpha.1 (headless; aarch64-unknown-linux-musl)\n"),
        Some(VersionLine {
            version: "0.2.0-alpha.1".into(),
            variant: Some("headless".into()),
            target: Some("aarch64-unknown-linux-musl".into()),
            x86_64_v3: false,
        })
    );
    // 只读首行；版本是 `aproxy v` 之后到空白为止的合法 semver
    assert_eq!(
        parse("aproxy v1.2.3 (desktop; x86_64-apple-darwin)\nsomething else").map(|l| l.version),
        Some("1.2.3".into())
    );
    assert_eq!(
        aproxy::install::staging::parse_version_line("aproxy v1.2.3 (desktop; t)").as_deref(),
        Some("1.2.3")
    );
    // 0.1.x 的格式（没有 v）与别的东西都不是
    for line in ["aproxy 0.1.0", "aproxy vX.Y", "aproxyv0.2.0", "", "0.2.0"] {
        assert_eq!(parse(line), None, "{line:?}");
    }

    // 本构建自己的输出：正是这个格式，变体与目标是本构建的
    let out = Command::new(env!("CARGO_BIN_EXE_aproxy"))
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let line = parse(&stdout).expect(&stdout);
    assert_eq!(line.version, env!("CARGO_PKG_VERSION"));
    assert!(
        matches!(line.variant.as_deref(), Some("desktop" | "headless")),
        "{stdout}"
    );
    assert_eq!(line.target, VersionLine::own().target, "{stdout}");
}

// ---------------------------------------------------------------------------
// C6：发布清单
// ---------------------------------------------------------------------------

#[test]
fn c6_release_manifest() {
    assert_eq!(force::MANIFEST_ASSET, "aproxy-manifest.json");
    assert_eq!(force::MANIFEST_FORMAT, 1);
    assert_eq!(force::VARIANT_DESKTOP, "desktop");
    assert_eq!(force::VARIANT_HEADLESS, "headless");
    assert_eq!(force::CPU_BASELINE, "baseline");
    assert_eq!(force::CPU_X86_64_V3, "x86-64-v3");

    let sha = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    let text = format!(
        r#"{{
  "manifest": 1,
  "version": "0.2.0",
  "published": "2031-01-01",
  "assets": [
    {{"target": "x86_64-pc-windows-msvc", "variant": "desktop", "cpu": "baseline", "file": "aproxy-x86_64-pc-windows-msvc.exe", "sha256": "{sha}"}},
    {{"target": "x86_64-pc-windows-msvc", "variant": "desktop", "cpu": "x86-64-v3", "file": "aproxy-x86_64-pc-windows-msvc-v3.exe", "sha256": "{sha}"}},
    {{"target": "x86_64-unknown-linux-gnu", "variant": "headless", "cpu": "baseline", "file": "aproxy-x86_64-unknown-linux-gnu-headless", "sha256": "{sha}", "size": 1}},
    {{"target": "x86_64-unknown-linux-gnu", "variant": "tray", "cpu": "baseline", "file": "future-variant", "sha256": "{sha}"}},
    {{"target": "x86_64-unknown-linux-gnu", "variant": "headless", "cpu": "x86-64-v4", "file": "future-cpu", "sha256": "{sha}"}}
  ]
}}"#
    );
    let manifest = Manifest::parse(&text).expect("清单可解析（未知字段忽略）");
    assert_eq!(manifest.manifest, 1);
    assert_eq!(manifest.version, "0.2.0");
    assert_eq!(manifest.assets.len(), 5);
    let asset = &manifest.assets[1];
    assert_eq!(asset.target, "x86_64-pc-windows-msvc");
    assert_eq!(asset.variant, "desktop");
    assert_eq!(asset.cpu, "x86-64-v3");
    assert_eq!(asset.file, "aproxy-x86_64-pc-windows-msvc-v3.exe");
    assert_eq!(asset.sha256, sha);

    let pick = |target: &str, variant: &str, v3: bool| {
        manifest
            .select(target, variant, v3)
            .map(|a| a.file.as_str())
    };
    assert_eq!(
        pick("x86_64-pc-windows-msvc", "desktop", true),
        Some("aproxy-x86_64-pc-windows-msvc-v3.exe")
    );
    assert_eq!(
        pick("x86_64-pc-windows-msvc", "desktop", false),
        Some("aproxy-x86_64-pc-windows-msvc.exe")
    );
    // 没有 v3 档退回基线档；不认识的变体与指令集档不参与选择
    assert_eq!(
        pick("x86_64-unknown-linux-gnu", "headless", true),
        Some("aproxy-x86_64-unknown-linux-gnu-headless")
    );
    assert_eq!(pick("x86_64-unknown-linux-gnu", "desktop", true), None);
    assert_eq!(pick("x86_64-pc-windows-msvc", "headless", true), None);

    // 必填字段缺了就不是清单
    for bad in [
        r#"{"version":"0.2.0","assets":[]}"#,
        r#"{"manifest":1,"assets":[]}"#,
        r#"{"manifest":1,"version":"0.2.0","assets":[{"target":"t","variant":"desktop","cpu":"baseline","file":"f"}]}"#,
    ] {
        assert!(Manifest::parse(bad).is_err(), "{bad}");
    }
}

// ---------------------------------------------------------------------------
// C7：命令行写法与退出码
// ---------------------------------------------------------------------------

#[test]
fn c7_exit_codes() {
    assert_eq!(force::EXIT_DONE, 0);
    assert_eq!(force::EXIT_UNCHANGED, 10);
    assert_eq!(force::EXIT_STOPPED, 11);
    assert_eq!(force::EXIT_NOT_RESTORED, 12);
}

/// 这些写法都被接受（参数解析通过，用法错误的退出码是 2），并且在动任何东西之前
/// 就以 10 结束：源文件不存在、版本低于 0.2.0、下载代理连不上。
#[test]
fn c7_accepted_command_lines_end_with_10_before_touching_anything() {
    let home = temp_home();
    let missing = home.path().join("no-such-aproxy");
    let missing = missing.to_str().unwrap();
    // 下载代理指向本机一个没人监听的端口：在线取目标立即失败，不会真的联网
    let dead_proxy = "http://127.0.0.1:9";
    let cases: Vec<Vec<&str>> = vec![
        vec!["install", "--force", "--from", missing, "--yes"],
        vec!["upgrade", "--force", "--from", missing, "--yes"],
        vec![
            "install",
            "--force",
            "--from",
            missing,
            "--yes",
            "--variant",
            "headless",
        ],
        vec!["install", "--force", "0.1.0", "--yes"],
        vec!["upgrade", "--force", "0.1.9", "--yes"],
        vec![
            "install",
            "--force",
            "latest",
            "--yes",
            "--variant",
            "desktop",
            "--cpu",
            "baseline",
            "--pre",
            "--download-proxy",
            dead_proxy,
        ],
        vec![
            "install",
            "--force",
            "0.2.0",
            "--yes",
            "--cpu",
            "x86-64-v3",
            "--download-proxy",
            dead_proxy,
        ],
        vec![
            "install",
            "--force",
            "--yes",
            "--download-proxy",
            dead_proxy,
        ],
    ];
    for args in cases {
        let out = aproxy_in(home.path(), &args);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(10), "{args:?}: {stderr}");
    }
    assert!(
        !aproxy::install::swap::bin_dir_in(home.path()).exists(),
        "以 10 结束的强制安装不碰 bin"
    );
}

/// 没有 --yes、标准输入又不是终端：不卡着等输入，以 10 结束，并提示加 --yes。
#[test]
fn c7_confirmation_needs_yes_when_not_interactive() {
    let home = temp_home();
    let source: PathBuf = home
        .path()
        .join(format!("aproxy-source{}", std::env::consts::EXE_SUFFIX));
    std::fs::copy(env!("CARGO_BIN_EXE_aproxy"), &source).unwrap();
    let out = aproxy_in(
        home.path(),
        &["install", "--force", "--from", source.to_str().unwrap()],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(10), "{stderr}");
    assert!(stderr.contains("--yes"), "{stderr}");
    assert!(!aproxy::install::swap::bin_dir_in(home.path()).exists());
}
