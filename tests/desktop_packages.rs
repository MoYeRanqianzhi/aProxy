//! 在临时主目录里构造用户会选的文件和目录，不依赖网络或已安装的包。
mod common;
use aproxy::desktop::DesktopHome;
use aproxy_ipc::desktop::*;
use common::TestHome;
use serde_json::{Value, json};
use std::{
    fs,
    io::{Cursor, Write},
    path::Path,
};
fn host(h: &TestHome) -> DesktopHome {
    DesktopHome::new(h.path().into(), env!("CARGO_BIN_EXE_aproxy").into()).unwrap()
}
fn manifest(kind: &str) -> Value {
    json!({"schema":1,"kind":kind,"id":"sample.one","name":{"en":"\u{202e} Sample \u{200b}"},"description":" A package ","version":"1.2.0","author":"author","repository":"https://github.com/ab-c/repo.git/","future":{"extra":42}})
}
fn members(kind: &str) -> Vec<(String, Vec<u8>)> {
    vec![
        (
            "manifest.json".into(),
            [&b"\xef\xbb\xbf"[..], manifest(kind).to_string().as_bytes()].concat(),
        ),
        if kind == "theme" {
            ("theme.json".into(), b"\xef\xbb\xbf{\"x\":42}".to_vec())
        } else {
            ("main.mjs".into(), b"export default 1".to_vec())
        },
        ("assets/a.PNG".into(), vec![0, 1]),
        ("README".into(), vec![]),
        ("assets/__MACOSX/file".into(), vec![]),
        (".DS_Store".into(), vec![]),
    ]
}
fn archive(m: &[(String, Vec<u8>)], prefix: &str) -> Vec<u8> {
    let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (n, b) in m {
        w.start_file(
            format!("{prefix}{n}"),
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated),
        )
        .unwrap();
        w.write_all(b).unwrap();
    }
    w.finish().unwrap().into_inner()
}
fn directory(dir: &Path, m: &[(String, Vec<u8>)]) {
    fs::create_dir_all(dir).unwrap();
    for (n, b) in m {
        let p = dir.join(n);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, b).unwrap();
    }
}
fn check_error(e: Fault) -> PackageIssue {
    if let Fault::DesktopPackage(issue) = e {
        issue
    } else {
        panic!("unexpected: {e:?}")
    }
}
#[tokio::test]
async fn staging_is_a_snapshot_and_checks_replace_one_another() {
    let h = TestHome::new();
    let host = host(&h);
    let base = h.path().join("desktop/extensions");
    let mut prev = None;
    for kind in ["extension", "theme"] {
        let m = members(kind);
        let path = h.path().join(kind);
        directory(&path, &m);
        for check in [0, 1, 2] {
            let c = match check {
                0 => host.package_check_path(path.to_str().unwrap().into()).await,
                1 => {
                    let p = h.path().join("packed.zip");
                    fs::write(&p, archive(&m, "")).unwrap();
                    host.package_check_path(p.to_str().unwrap().into()).await
                }
                _ => {
                    host.package_check_file("renamed.zip".into(), archive(&m, "wrapped/"))
                        .await
                }
            }
            .unwrap();
            let stage = base.join(format!(".staging-{}", c.token));
            if let Some(old) = prev.take() {
                assert!(!fs::exists(old).unwrap());
            }
            assert!(stage.join("assets/a.PNG").is_file());
            assert!(!stage.join(".DS_Store").exists());
            assert_eq!(c.files.len(), 4);
            assert!(c.files.windows(2).all(|w| w[0].path < w[1].path));
            assert_eq!(c.total_bytes, c.files.iter().map(|f| f.bytes).sum::<u64>());
            assert_eq!(
                c.manifest.repository.as_deref(),
                Some("https://github.com/ab-c/repo")
            );
            assert_eq!(
                serde_json::to_value(c.manifest.name).unwrap(),
                json!({"en":"Sample"})
            );
            assert_eq!(c.theme.is_some(), kind == "theme");
            host.package_discard("wrong".into()).await.unwrap();
            assert!(stage.is_dir());
            prev = Some(stage);
            if kind == "theme" && check == 2 {
                host.package_discard(c.token.clone()).await.unwrap();
                host.package_discard(c.token).await.unwrap();
                assert!(!prev.take().unwrap().exists());
            }
        }
    }
}
#[tokio::test]
async fn every_manifest_field_and_json_structure_are_checked_before_install() {
    let h = TestHome::new();
    let host = host(&h);
    let m = members("extension");
    let source = h.path().join("d");
    directory(&source, &m);
    let check = || host.package_check_path(source.to_str().unwrap().into());
    for (field, value) in [
        ("schema", json!(2)),
        ("schema", json!(1.0)),
        ("kind", json!("widget")),
        ("id", json!("con.name")),
        ("id", json!("a..b")),
        ("id", json!("Abc")),
        ("id", json!("-abc")),
        ("id", json!("ab")),
        ("name", json!({"en--US":"x"})),
        ("name", json!("\u{061c}\u{2060}")),
        ("description", json!("x".repeat(501))),
        ("version", json!("v1.0.0")),
        ("version", json!("01.2.3")),
        ("author", json!("x".repeat(65))),
        ("repository", json!("http://outside")),
        ("repository", json!("https://x@github.com/x/y")),
        ("repository", json!("https://github.com/o/../bad")),
        ("repository", json!("https://github.com/o/r?v")),
        ("license", json!("MIT/Apache")),
        ("formats", json!(["a", "a"])),
        ("network", json!(["Example.org"])),
        ("writes", json!(["files"])),
    ] {
        let mut v = manifest("extension");
        v[field] = value;
        fs::write(source.join("manifest.json"), v.to_string()).unwrap();
        let e = check_error(check().await.unwrap_err());
        assert!(
            e.field.as_deref() == Some(field) || e.reason == "unsupported-schema",
            "{field} {e:?}"
        );
    }
    for field in [
        "schema",
        "kind",
        "id",
        "name",
        "description",
        "version",
        "author",
    ] {
        let mut v = manifest("extension");
        v.as_object_mut().unwrap().remove(field);
        fs::write(source.join("manifest.json"), v.to_string()).unwrap();
        assert_eq!(
            check_error(check().await.unwrap_err()).field.as_deref(),
            Some(field)
        );
    }
    let raw = manifest("extension").to_string();
    for s in [
        raw.replace("\"extra\":42", "\"extra\":42,\"extra\":1"),
        raw.replace("42", &format!("{}0{}", "[".repeat(16), "]".repeat(16))),
        " ".repeat(65537),
    ] {
        fs::write(source.join("manifest.json"), s).unwrap();
        assert!(check().await.is_err());
    }
    for field in ["formats", "writes", "network"] {
        let mut v = manifest("theme");
        v[field] = json!([]);
        fs::write(source.join("manifest.json"), v.to_string()).unwrap();
        assert_eq!(
            check_error(check().await.unwrap_err()).field.as_deref(),
            Some(field)
        );
    }
}
#[tokio::test]
async fn layouts_paths_links_and_formats_do_not_expand_the_install_surface() {
    let h = TestHome::new();
    let host = host(&h);
    let check =
        |m: &[(String, Vec<u8>)]| host.package_check_file("test.zip".into(), archive(m, ""));
    for name in [
        "../escape",
        "/abs",
        "C:drive",
        "assets\\bad",
        "assets/nul.txt",
        "assets/space.",
        "assets/x ",
        "assets//x",
        "assets/./x",
        "assets/<x",
        "assets/A.png",
        "assets/a/b/c/d/e/f/g/h",
        "script.js",
        "assets/script.js",
    ] {
        let mut m = members("theme");
        m.push((name.into(), vec![]));
        assert!(check(&m).await.is_err(), "{name}");
    }
    let mut m = members("extension");
    m.retain(|(p, _)| p != "main.mjs");
    assert_eq!(check_error(check(&m).await.unwrap_err()).reason, "missing");
    m.push(("main.mjs".into(), vec![0]));
    assert_eq!(check_error(check(&m).await.unwrap_err()).reason, "text");
    let mut m = members("theme");
    m[1].1 = br#"{"x":1,"x":2}"#.into();
    assert_eq!(check_error(check(&m).await.unwrap_err()).reason, "json");
    let mut m = members("extension");
    m.extend((0..2000).map(|n| (format!("assets/{n}"), vec![])));
    assert_eq!(
        check_error(check(&m).await.unwrap_err()).limit.as_deref(),
        Some("files")
    );
    let src = h.path().join("linked");
    directory(&src, &members("theme"));
    let outside = h.path().join("outside");
    fs::create_dir(&outside).unwrap();
    let link = src.join("assets").join("redirect");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    #[cfg(windows)]
    assert!(
        h.cmd_with("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&link)
            .arg(&outside)
            .output()
            .unwrap()
            .status
            .success()
    );
    assert_eq!(
        check_error(
            host.package_check_path(src.to_str().unwrap().into())
                .await
                .unwrap_err()
        )
        .reason,
        "link"
    );
}
/// 只变更两个头的对应字段，不生成巨型压缩样本；实际解压长度独立核验。
#[tokio::test]
async fn zip_headers_do_not_override_resource_and_content_checks() {
    let h = TestHome::new();
    let host = host(&h);
    let mut m = members("extension");
    m.push(("assets/more".into(), vec![3]));
    let original = archive(&m, "");
    let offsets = |b: &[u8], sig: &[u8]| {
        b.windows(4)
            .enumerate()
            .filter_map(|(i, w)| (w == sig).then_some(i))
            .collect::<Vec<_>>()
    };
    let local = offsets(&original, b"PK\x03\x04");
    let central = offsets(&original, b"PK\x01\x02");
    for why in [
        "encrypted",
        "compression",
        "link",
        "file-bytes",
        "total-bytes",
        "size-mismatch",
    ] {
        let mut b = original.clone();
        for (i, (&l, &c)) in local.iter().zip(&central).enumerate() {
            if why != "total-bytes" && i != 0 {
                continue;
            }
            match why {
                "encrypted" => {
                    b[l + 6] |= 1;
                    b[c + 8] |= 1
                }
                "compression" => {
                    b[l + 8] = 99;
                    b[c + 10] = 99
                }
                "link" => {
                    b[c + 5] = 3;
                    b[c + 38..c + 42].copy_from_slice(&(0o120777u32 << 16).to_le_bytes())
                }
                _ => {
                    let n: u32 = match why {
                        "file-bytes" => (64 << 20) + 1,
                        "total-bytes" => 64 << 20,
                        _ => 2,
                    };
                    b[l + 22..l + 26].copy_from_slice(&n.to_le_bytes());
                    b[c + 24..c + 28].copy_from_slice(&n.to_le_bytes());
                }
            }
        }
        let err = check_error(
            host.package_check_file("x.zip".into(), b)
                .await
                .unwrap_err(),
        );
        assert!(
            (why == "size-mismatch" && err.reason == "content")
                || err.reason == why
                || err.limit.as_deref() == Some(why)
                || matches!(why, "compression" | "encrypted") && err.reason == "archive",
            "{why}: {err:?}"
        );
    }
}

/// 回环代理直接回答 API 请求，不解析目标域名；记录原始请求以核验路径、重定向与请求头。
#[tokio::test]
async fn github_repo_and_release_facts_use_only_bounded_anonymous_api_requests() {
    use aproxy::desktop::packages::github::facts;
    use std::time::Duration;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpSocket},
    };
    let h = TestHome::new();
    let host = host(&h);
    let checked = host
        .package_check_file("local.zip".into(), archive(&members("extension"), ""))
        .await
        .unwrap();
    let PackageSource::File { sha256, .. } = &checked.source else {
        panic!()
    };
    let digest = format!("sha256:{sha256}");
    let directory = PackageSource::Directory {
        name: "tree".into(),
    };
    let repository = Some("https://github.com/a/b");
    let api = "http://github.invalid";
    let timeout = Duration::from_secs(3);
    let repo = json!({"full_name":"renamed/repo","html_url":"https://github.com/renamed/repo","stargazers_count":432,"forks_count":8,"created_at":"2020-01-01T00:00:00Z","pushed_at":"2026-01-01T22:00:00Z","archived":true,"fork":false,"license":{"spdx_id":"MIT"}});
    let reply = |status: &str, headers: &str, body: &str| {
        format!(
            "HTTP/1.1 {status}\r\nConnection: close\r\n{headers}Content-Length: {}\r\n\r\n{body}",
            body.len()
        )
    };
    let ok = |v: &Value| reply("200 OK", "", &v.to_string());
    let missing = reply("404 Not Found", "", "");
    assert!(matches!(
        facts(None, "1.2.0", &directory, api, None, timeout).await,
        GithubFacts::NoRepository
    ));
    assert!(matches!(
        facts(
            Some("https://a.invalid"),
            "1.2.0",
            &directory,
            api,
            None,
            timeout
        )
        .await,
        GithubFacts::NotGithub { .. }
    ));
    let asset = |name: &str, hash: Value| json!({"name":name,"digest":hash,"browser_download_url":"https://github.com/a/b/releases/download/v1.2.0/pkg.zip"});
    let release = |tag: &str, assets: Value| ok(&json!({"tag_name":tag,"assets":assets}));
    for (state, replies) in [
        ("repo-not-found", vec![missing.clone()]),
        (
            "rate-403",
            vec![reply("403 Forbidden", "x-ratelimit-remaining: 0\r\n", "")],
        ),
        ("rate-429", vec![reply("429 Rate Limited", "", "")]),
        ("http", vec![reply("403 Forbidden", "", "")]),
        (
            "oversized",
            vec![reply("200 OK", "", &"x".repeat((1 << 20) + 1))],
        ),
        ("not-a-file", vec![ok(&repo)]),
        (
            "match",
            vec![
                reply(
                    "301 Moved",
                    "Location: http://github.invalid/repositories/7\r\n",
                    "",
                ),
                ok(&repo),
                missing.clone(),
                release("1.2.0", json!([asset("other.zip", json!(digest))])),
            ],
        ),
        (
            "mismatch",
            vec![
                ok(&repo),
                release(
                    "v1.2.0",
                    json!([asset("local.zip", json!("sha256:different"))]),
                ),
            ],
        ),
        (
            "no-digest",
            vec![
                ok(&repo),
                release("v1.2.0", json!([asset("other.zip", Value::Null)])),
            ],
        ),
        ("no-asset", vec![ok(&repo), release("v1.2.0", json!([]))]),
        (
            "no-release",
            vec![ok(&repo), missing.clone(), missing.clone()],
        ),
        (
            "release-unreachable",
            vec![ok(&repo), reply("500 Internal Error", "", "")],
        ),
    ] {
        let l = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let proxy = format!("http://{}", l.local_addr().unwrap());
        let worker = tokio::spawn(async move {
            let mut paths = Vec::new();
            for response in replies {
                let (mut s, _) = l.accept().await.unwrap();
                let mut data = Vec::new();
                while !data.windows(4).any(|s| s == b"\r\n\r\n") {
                    let mut b = [0; 1024];
                    let n = s.read(&mut b).await.unwrap();
                    assert!(n > 0);
                    data.extend_from_slice(&b[..n]);
                    assert!(data.len() < 8192);
                }
                let raw = String::from_utf8(data).unwrap().to_ascii_lowercase();
                assert!(
                    raw.contains(concat!(
                        "user-agent: aproxy-desktop/",
                        env!("CARGO_PKG_VERSION")
                    )) && raw.contains("accept: application/vnd.github+json")
                        && raw.contains("x-github-api-version: 2022-11-28")
                        && !raw.contains("authorization:")
                );
                paths.push(raw.lines().next().unwrap().to_owned());
                let _ = s.write_all(response.as_bytes()).await;
            }
            paths
        });
        let value = serde_json::to_value(
            facts(
                repository,
                "1.2.0",
                if state == "not-a-file" {
                    &directory
                } else {
                    &checked.source
                },
                api,
                Some(&proxy),
                timeout,
            )
            .await,
        )
        .unwrap();
        let paths = worker.await.unwrap();
        assert!(paths[0].contains("http://github.invalid/repos/a/b"));
        match state {
            "repo-not-found" => assert_eq!(value["state"], state),
            "rate-403" | "rate-429" => assert_eq!(value["reason"], "rate-limited"),
            "http" | "oversized" => assert_eq!(value["reason"], "http"),
            _ => {
                assert_eq!(value["state"], "checked");
                assert_eq!(value["repo"]["renamed"], true);
                assert_eq!(value["repo"]["stars"], 432);
                assert_eq!(value["repo"]["createdAt"], 1577836800000_i64);
                assert_eq!(
                    value["release"]["state"],
                    if state == "release-unreachable" {
                        "unreachable"
                    } else {
                        state
                    }
                );
                if state == "match" {
                    assert!(
                        paths[1].contains("/repositories/7")
                            && paths[2].contains("/tags/v1.2.0")
                            && paths[3].contains("/tags/1.2.0")
                    );
                    assert_eq!(value["release"]["asset"], "other.zip");
                }
            }
        }
    }
    // 绑定但不监听，端口仍由本测试占有，拒绝连接不能误中第三方服务。
    let socket = TcpSocket::new_v4().unwrap();
    socket
        .bind((std::net::Ipv4Addr::LOCALHOST, 0).into())
        .unwrap();
    let proxy = format!("http://{}", socket.local_addr().unwrap());
    assert!(matches!(
        facts(repository, "1.2.0", &directory, api, Some(&proxy), timeout).await,
        GithubFacts::Unreachable {
            reason: Unreachable::Network,
            ..
        }
    ));
    let _silent = socket.listen(1).unwrap();
    assert!(matches!(
        facts(
            repository,
            "1.2.0",
            &directory,
            api,
            Some(&proxy),
            Duration::from_millis(75)
        )
        .await,
        GithubFacts::Unreachable {
            reason: Unreachable::Timeout,
            ..
        }
    ));
}

#[tokio::test(start_paused = true)]
async fn expired_check_does_not_install_or_verify_and_releases_its_stage() {
    let h = TestHome::new();
    let host = host(&h);
    let c = host
        .package_check_file("e.zip".into(), archive(&members("theme"), ""))
        .await
        .unwrap();
    tokio::time::advance(std::time::Duration::from_secs(1800)).await;
    assert_eq!(
        check_error(
            host.package_install(c.token.clone(), true)
                .await
                .unwrap_err()
        )
        .reason,
        "token"
    );
    assert_eq!(
        check_error(host.package_verify(c.token).await.unwrap_err()).reason,
        "token"
    );
    assert!(host.package_list().await.unwrap().is_empty());
    assert_eq!(
        fs::read_dir(h.path().join("desktop/extensions"))
            .unwrap()
            .count(),
        0
    );
}
#[tokio::test]
async fn lifecycle_preserves_data_order_and_provenance_but_not_removed_code() {
    let h = TestHome::new();
    let host = host(&h);
    let desktop = h.path().join("desktop");
    let base = desktop.join("extensions");
    fs::create_dir_all(base.join(".staging-stale")).unwrap();
    fs::create_dir_all(base.join(".old-stale")).unwrap();
    let check = |kind: &str, id: &str, version: &str| {
        let mut m = members(kind);
        let mut v = manifest(kind);
        v["id"] = json!(id);
        v["version"] = json!(version);
        v.as_object_mut().unwrap().remove("repository");
        m[0].1 = v.to_string().into_bytes();
        host.package_check_file("pick.zip".into(), archive(&m, ""))
    };
    let c = check("extension", "sample.one", "2.0.0").await.unwrap();
    assert!(host.package_list().await.unwrap().is_empty());
    assert!(!base.join(".staging-stale").exists() && !base.join(".old-stale").exists());
    assert!(matches!(
        host.package_verify(c.token.clone()).await.unwrap(),
        GithubFacts::NoRepository
    ));
    let e = host.package_install(c.token.clone(), true).await.unwrap();
    assert!(e.enabled && matches!(e.github, Some(GithubFacts::NoRepository)));
    assert!(host.package_install(c.token, false).await.is_err());
    let d = desktop.join("extension-data/sample.one");
    fs::create_dir_all(&d).unwrap();
    fs::write(d.join("storage.json"), b"{\"x\":1}").unwrap();
    let prefs = desktop.join("desktop.toml");
    fs::OpenOptions::new()
        .append(true)
        .open(&prefs)
        .unwrap()
        .write_all(b"\n[extensions.settings.\"sample.one\"]\nkept = 123\n")
        .unwrap();
    let c = check("extension", "sample.one", "1.0.0").await.unwrap();
    assert_eq!(c.installed.unwrap().version, "2.0.0");
    assert!(host.package_install(c.token, false).await.unwrap().enabled);
    assert_eq!(host.package_list().await.unwrap()[0].data_bytes, 7);
    assert!(fs::read_to_string(&prefs).unwrap().contains("kept = 123"));
    assert_eq!(
        check_error(check("theme", "sample.one", "3.0.0").await.unwrap_err()).reason,
        "kind-change"
    );
    let c = check("theme", "theme.two", "1.0.0").await.unwrap();
    let e = host.package_install(c.token, true).await.unwrap();
    assert!(!e.enabled && e.theme.unwrap()["x"] == 42);
    assert!(
        host.package_set_enabled("theme.two".into(), true)
            .await
            .is_err()
    );
    let p = serde_json::from_value(json!({"theme":"theme.two"})).unwrap();
    host.set_preferences(p).await.unwrap();
    host.package_uninstall("theme.two".into(), false)
        .await
        .unwrap();
    assert_eq!(
        host.read_desktop_state()
            .await
            .unwrap()
            .preferences
            .unwrap()
            .theme,
        "builtin"
    );
    assert!(
        !host
            .package_set_enabled("sample.one".into(), false)
            .await
            .unwrap()
            .enabled
    );
    host.package_uninstall("sample.one".into(), false)
        .await
        .unwrap();
    assert!(d.is_dir() && !base.join("sample.one").exists());
    assert!(fs::read_to_string(&prefs).unwrap().contains("kept = 123"));
    directory(&base.join("sample.one"), &members("extension"));
    let list = host.package_list().await.unwrap();
    assert_eq!(list.len(), 1);
    assert!(list[0].issue.is_none() && list[0].source.is_none());
    fs::write(base.join("sample.one/manifest.json"), b"{").unwrap();
    let list = host.package_list().await.unwrap();
    assert!(list[0].issue.is_some() && list[0].manifest.is_none());
    assert!(
        host.package_set_enabled("sample.one".into(), true)
            .await
            .is_err()
    );
    host.package_uninstall("sample.one".into(), true)
        .await
        .unwrap();
    assert!(!d.exists() && !fs::read_to_string(prefs).unwrap().contains("kept = 123"));
}

#[test]
fn package_urls_serve_only_regular_descendants_without_a_window() {
    use aproxy::desktop::packages::scheme::respond;
    let h = TestHome::new();
    let root = h.path().join("desktop/extensions");
    let d = root.join("sample.one");
    directory(&d, &members("extension"));
    fs::create_dir_all(root.join(".staging-secret")).unwrap();
    fs::write(root.join(".staging-secret/x"), b"staged").unwrap();
    fs::write(root.join("outside"), b"secret").unwrap();
    let run = |method: &str, path: &str| {
        respond(
            h.path(),
            &http::Request::builder()
                .method(method)
                .uri(format!("aproxy-pkg://localhost/{path}"))
                .body(vec![])
                .unwrap(),
        )
    };
    for (ext, mime) in [
        ("mjs", "text/javascript; charset=utf-8"),
        ("js", "text/javascript; charset=utf-8"),
        ("css", "text/css; charset=utf-8"),
        ("json", "application/json"),
        ("svg", "image/svg+xml"),
        ("png", "image/png"),
        ("jpg", "image/jpeg"),
        ("jpeg", "image/jpeg"),
        ("webp", "image/webp"),
        ("gif", "image/gif"),
        ("avif", "image/avif"),
        ("woff2", "font/woff2"),
        ("woff", "font/woff"),
        ("ttf", "font/ttf"),
        ("otf", "font/otf"),
        ("wasm", "application/wasm"),
        ("html", "text/html; charset=utf-8"),
        ("htm", "text/html; charset=utf-8"),
        ("txt", "text/plain; charset=utf-8"),
        ("md", "text/plain; charset=utf-8"),
        ("bin", "application/octet-stream"),
    ] {
        fs::write(d.join(format!("assets/a b.{ext}")), b"abc").unwrap();
        let path = format!("sample.one/assets/a%20b.{ext}?v=2&n=1#frag");
        let r = run("GET", &path);
        assert_eq!(r.status(), 200);
        assert_eq!(r.body(), b"abc");
        for (k, v) in [
            ("content-type", mime),
            ("access-control-allow-origin", "*"),
            ("x-content-type-options", "nosniff"),
            ("cache-control", "no-cache"),
        ] {
            assert_eq!(r.headers()[k], v);
        }
        let head = run("HEAD", &path);
        assert!(head.body().is_empty());
        assert_eq!(head.headers()["content-length"], "3");
    }
    for path in [
        "sample.one/../outside",
        "sample.one/%2e%2e/outside",
        "sample.one/%252e%252e/outside",
        "sample.one/assets%2fa.PNG",
        "sample.one/assets%5ca.PNG",
        "sample.one//outside",
        "sample.one/C:/outside",
        "sample.one/nul.txt",
        ".staging-secret/x",
        "missing.one/x",
        "sample.one/absent",
        "sample.one/assets",
        "sample.one/%zz",
    ] {
        let r = run("GET", path);
        assert_eq!(r.status(), 404, "{path}");
        assert!(r.body().is_empty());
    }
    assert_eq!(run("POST", "sample.one/main.mjs").status(), 405);
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(root.join("outside"), d.join("assets/link")).unwrap();
        assert_eq!(run("GET", "sample.one/assets/link").status(), 404);
    }
}

fn changes(v: Value) -> serde_json::Map<String, Value> {
    v.as_object().unwrap().clone()
}
#[tokio::test]
async fn extension_settings_round_trip_without_writing_over_a_broken_document() {
    let h = TestHome::new();
    let host = host(&h);
    let id = "sample.one";
    let c = host
        .package_check_file("code.zip".into(), archive(&members("extension"), ""))
        .await
        .unwrap();
    host.package_install(c.token, true).await.unwrap();
    let path = h.path().join("desktop/desktop.toml");
    let text = "# neighbor\nmode = 'dark'  # exact\n[extensions.settings.\"sample.one\"]\nwhen = 2025-04-05T09:08:07Z\nbig = 9007199254740993\n[extensions.settings.elsewhere]\nother = 'no change'\n";
    fs::write(&path, text).unwrap();
    let initial = host.extension_settings_read(id.into()).await.unwrap();
    assert_eq!(initial["when"], "2025-04-05T09:08:07Z");
    assert_eq!(initial["big"], "9007199254740993");
    let patch = json!({"when":null,"nested":{"a.b":[42,true,{"deep":"x"}]},"num":-12.5});
    let written = host
        .extension_settings_write(id.into(), changes(patch))
        .await
        .unwrap();
    assert_eq!(
        host.extension_settings_read(id.into()).await.unwrap(),
        written
    );
    assert!(!written.contains_key("when"));
    assert_eq!(written["nested"]["a.b"][2]["deep"], "x");
    let stable = fs::read_to_string(&path).unwrap();
    assert!(
        stable.starts_with("# neighbor\nmode = 'dark'  # exact\n")
            && stable.ends_with("[extensions.settings.elsewhere]\nother = 'no change'\n")
    );
    for patch in [
        json!({"x":[null]}),
        json!({"x":{"y":null}}),
        json!({"bad.flag":true}),
        json!({"big":9007199254740993_i64}),
        json!({"x":"a".repeat(65536)}),
    ] {
        assert!(
            host.extension_settings_write(id.into(), changes(patch))
                .await
                .is_err()
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), stable);
    }
    fs::write(
        &path,
        "extensions = { settings = { 'sample.one' = { x = 1 } } } # inline\n",
    )
    .unwrap();
    let stored = host
        .extension_settings_write(id.into(), changes(json!({"x":2})))
        .await
        .unwrap();
    assert_eq!(stored["x"], 2);
    assert_eq!(
        host.extension_settings_read(id.into()).await.unwrap(),
        stored
    );
    fs::write(&path, "[broken\n").unwrap();
    assert!(host.extension_settings_read(id.into()).await.is_err());
    assert!(
        host.extension_settings_write(id.into(), changes(json!({"x":1})))
            .await
            .is_err()
    );
    assert_eq!(fs::read(&path).unwrap(), b"[broken\n");
}
#[tokio::test]
async fn extension_data_roundtrip_limits_reset_and_link_refusal() {
    let h = TestHome::new();
    let host = host(&h);
    let id = "sample.one";
    assert!(host.extension_storage_read(id.into()).await.is_err());
    let dir = h.path().join("desktop/extensions/sample.one");
    directory(&dir, &members("theme"));
    assert!(host.extension_files_list(id.into()).await.is_err());
    fs::write(dir.join("manifest.json"), manifest("extension").to_string()).unwrap();
    fs::write(dir.join("main.mjs"), b"export {}").unwrap();
    let d = h
        .path()
        .join("desktop")
        .join("extension-data")
        .join("sample.one");
    assert!(
        host.extension_storage_read(id.into())
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        host.extension_files_list(id.into())
            .await
            .unwrap()
            .is_empty()
    );
    assert!(!d.exists());
    let put = |v, r| host.extension_storage_write(id.into(), changes(v), r);
    let v = json!({"prefs":{"yes":[null,"ok"]},"empty":true});
    put(v, false).await.unwrap();
    let store = put(json!({"empty":null}), false).await.unwrap();
    assert_eq!(store["prefs"]["yes"][0], Value::Null);
    assert_eq!(store, host.extension_storage_read(id.into()).await.unwrap());
    for p in [
        json!({"":true}),
        json!({"bad\n":1}),
        json!({"x":"a".repeat(4<<20)}),
    ] {
        assert!(put(p, false).await.is_err());
    }
    fs::write(d.join("storage.json"), b"[").unwrap();
    assert_eq!(
        check_error(host.extension_storage_read(id.into()).await.unwrap_err()).reason,
        "corrupt-storage"
    );
    assert!(put(json!({}), false).await.is_err());
    assert_eq!(put(json!({"ok":true}), true).await.unwrap()["ok"], true);
    let write = |n: &str, b: Vec<u8>| host.extension_file_write(id.into(), n.into(), b);
    let read = |n: &str| host.extension_file_read(id.into(), n.into());
    let del = |n: &str| host.extension_file_delete(id.into(), n.into());
    for n in [
        "", ".hidden", "a.", "a/b", "a\\b", "a:b", "PRN.txt", "lPt9", "é",
    ] {
        assert!(write(n, vec![]).await.is_err());
        assert!(read(n).await.is_err());
        assert!(del(n).await.is_err());
    }
    let f = write("z.bin", vec![0, 255]).await.unwrap();
    assert_eq!(f.bytes, 2);
    assert_eq!(read("z.bin").await.unwrap(), [0, 255]);
    write("a.bin", vec![4]).await.unwrap();
    let list = host.extension_files_list(id.into()).await.unwrap();
    assert_eq!(
        list.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
        ["a.bin", "z.bin"]
    );
    del("a.bin").await.unwrap();
    del("a.bin").await.unwrap();
    let files = d.join("files");
    for i in 0..999 {
        fs::File::create(files.join(format!("x{i}"))).unwrap();
    }
    assert_eq!(
        check_error(write("overflow", vec![]).await.unwrap_err())
            .limit
            .as_deref(),
        Some("extension-files")
    );
    write("z.bin", vec![1]).await.unwrap();
    fs::File::create(files.join("x0"))
        .unwrap()
        .set_len(512 << 20)
        .unwrap();
    assert_eq!(
        check_error(write("z.bin", vec![]).await.unwrap_err())
            .limit
            .as_deref(),
        Some("extension-bytes")
    );
    assert_eq!(
        check_error(put(json!({"x":1}), false).await.unwrap_err())
            .limit
            .as_deref(),
        Some("extension-bytes")
    );
    assert_eq!(
        check_error(read("x0").await.unwrap_err()).limit.as_deref(),
        Some("file-bytes")
    );
    assert_eq!(
        check_error(write("z.bin", vec![0; (64 << 20) + 1]).await.unwrap_err())
            .limit
            .as_deref(),
        Some("file-bytes")
    );
    let outside = h.path().join("other");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("important"), b"original").unwrap();
    let link = files.join("redirect");
    #[cfg(windows)]
    {
        assert!(
            h.cmd_with("cmd.exe")
                .args(["/d", "/c", "mklink", "/J"])
                .arg(&link)
                .arg(&outside)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside.join("important"), &link).unwrap();
    assert!(
        read("redirect").await.is_err()
            && write("redirect", vec![]).await.is_err()
            && del("redirect").await.is_err()
    );
    assert!(
        host.extension_files_list(id.into())
            .await
            .unwrap()
            .iter()
            .all(|f| f.name != "redirect")
    );
    fs::rename(&files, d.join("saved")).unwrap();
    #[cfg(windows)]
    assert!(
        h.cmd_with("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&files)
            .arg(&outside)
            .output()
            .unwrap()
            .status
            .success()
    );
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, &files).unwrap();
    assert!(
        read("important").await.is_err()
            && write("important", vec![]).await.is_err()
            && del("important").await.is_err()
    );
    assert_eq!(fs::read(outside.join("important")).unwrap(), b"original");
}
