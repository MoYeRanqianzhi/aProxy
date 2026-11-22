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
