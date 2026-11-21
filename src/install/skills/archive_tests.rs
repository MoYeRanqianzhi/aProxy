//! 用小型本地包覆盖白名单、解压限额与整组替换，不经下载链也不启动进程。
use super::{archive, manifest::*};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

/// 从原始内容给出对应记录，便于只改变某一项做反例。
fn record(b: &[u8]) -> Value {
    json!({"size":b.len(),"sha256":hex(&Sha256::digest(b))})
}
/// 小包带多个目录与嵌套文件，验证顶层目录不会再套一层。
pub(super) fn docs() -> [(&'static str, &'static [u8]); 3] {
    [
        ("aproxy-cli/SKILL.md", b"# cli"),
        ("aproxy-cli/references/a.md", b"# reference"),
        ("aproxy-format/SKILL.md", b"# format"),
    ]
}
/// 组装清单；未知属性也随原始清单保留。
pub(super) fn manifest_for(files: &[(&str, &[u8])]) -> Value {
    let files: serde_json::Map<_, _> = files
        .iter()
        .map(|(k, b)| (k.to_string(), record(b)))
        .collect();
    json!({"schema":1,"version":"1.0.0","files":files,"future":true})
}
/// 压缩正文与清单分开传入，以覆盖缺失、未列出、长度或摘要不一致等情况。
pub(super) fn package(home: &Path, manifest: Option<&Value>, files: &[(&str, &[u8])]) -> PathBuf {
    let path = home.join("test.zip");
    let mut w = zip::ZipWriter::new(fs::File::create(&path).unwrap());
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    if let Some(m) = manifest {
        w.start_file(MARKER, opts).unwrap();
        w.write_all(&serde_json::to_vec(m).unwrap()).unwrap();
    }
    for &(k, b) in files {
        w.start_file(k, opts).unwrap();
        w.write_all(b).unwrap();
    }
    w.finish().unwrap();
    path
}
/// 拒绝平台特有别名、大小写折叠与同一个键出现两次（不能让 map 覆盖证据）。
#[test]
fn key_namespace_is_portable_and_exact() {
    for key in [
        "../s/a",
        "s/../a",
        "s/./a",
        "/s/a",
        "C:/a",
        "\\\\host\\x",
        "s/a:b",
        "s/a\\b",
        "s//a",
        "s/",
        "s",
        "s/a.",
        "s/a ",
        "s/é",
        "s/lPt9.md",
        "Con/a",
        "s/NUL",
        "s/aux.cfg",
        "s/COM1",
        "manifest.JSON/a",
        ".lock/a",
    ] {
        assert!(
            Manifest::parse(&serde_json::to_vec(&manifest_for(&[(key, b"a")])).unwrap()).is_err(),
            "{key}"
        );
    }
    for pair in [["S/a", "s/A"], ["S/a", "s/b"], ["s/a", "s/a/b"]] {
        assert!(
            Manifest::parse(
                &serde_json::to_vec(&manifest_for(&[(pair[0], b""), (pair[1], b"")])).unwrap()
            )
            .is_err()
        );
    }
    let r = record(b"");
    let dup = format!(r#"{{"schema":1,"version":"1.0.0","files":{{"s/a":{r},"s/a":{r}}}}}"#);
    assert!(Manifest::parse(dup.as_bytes()).is_err());
}
/// 大小在累加前验证，字段不接受隐式转换，哈希值只认完整的小写形式。
#[test]
fn schema_fields_and_limits_are_checked() {
    let base = manifest_for(&[("s/a", b"")]);
    let parse = |m: &Value| Manifest::parse(&serde_json::to_vec(m).unwrap());
    assert!(parse(&base).is_ok());
    for (path, val) in [
        ("/schema", json!(2)),
        ("/schema", json!("1")),
        ("/version", json!("")),
        ("/files", json!({})),
        ("/files/s~1a/size", json!(-1)),
        ("/files/s~1a/size", json!(MAX_FILE + 1)),
        ("/files/s~1a/sha256", json!("A".repeat(64))),
        ("/files/s~1a/sha256", json!("a")),
    ] {
        let mut bad = base.clone();
        *bad.pointer_mut(path).unwrap() = val;
        assert!(parse(&bad).is_err(), "{path}");
    }
    for (count, size) in [
        (MAX_FILES + 1, 0),
        ((MAX_TOTAL / MAX_FILE + 1) as usize, MAX_FILE),
    ] {
        let mut bad = base.clone();
        bad["files"] = json!({});
        for i in 0..count {
            let mut r = record(b"");
            r["size"] = size.into();
            bad["files"][format!("s/{i}")] = r;
        }
        assert!(parse(&bad).is_err());
    }
    let mut large = serde_json::to_vec(&base).unwrap();
    large.resize(MAX_MANIFEST as usize + 1, b' ');
    assert!(Manifest::parse(&large).is_err());
}
/// 所有列出的文件逐字节落位；额外条目不展开，修订包清理过期的托管目录但不碰其他目录。
#[test]
fn install_skill_dir_places_every_top_level_dir() {
    let h = tempfile::tempdir().unwrap();
    let home = h.path();
    let files = docs();
    let m = manifest_for(&files);
    let mut extra = files.to_vec();
    extra.push(("../outside", b"ignore"));
    let zip = package(home, Some(&m), &extra);
    let (_, n) = archive::install(home, &zip, "1.0.0").unwrap();
    assert_eq!(n, 1);
    for (k, b) in files {
        assert_eq!(fs::read(home.join("skills").join(k)).unwrap(), b);
    }
    assert!(!home.join("outside").exists());
    fs::create_dir(home.join("skills/mine")).unwrap();
    let subset = &files[..1];
    let zip = package(home, Some(&manifest_for(subset)), subset);
    archive::install(home, &zip, "1.0.0").unwrap();
    assert!(!home.join("skills/aproxy-format").exists());
    assert!(!home.join("skills/aproxy-cli/references").exists());
    assert!(home.join("skills/mine").is_dir());
}
/// 所有校验失败都发生在发布之前，原来的文件与完成标记保持原样。
#[test]
fn rejected_archives_do_not_change_installation() {
    let h = tempfile::tempdir().unwrap();
    let home = h.path();
    let f = docs();
    let m = manifest_for(&f);
    let zip = package(home, Some(&m), &f);
    archive::install(home, &zip, "1.0.0").unwrap();
    assert!(archive::install(home, &zip, "2.0.0").is_err());
    let marker = fs::read(home.join("skills/manifest.json")).unwrap();
    let zip = package(home, None, &f);
    assert!(
        archive::install(home, &zip, "1.0.0")
            .unwrap_err()
            .to_string()
            .contains("0.2.0-alpha.4")
    );
    for body in [None, Some(&b"other"[..]), Some(&b"too large"[..])] {
        let data: Vec<_> = body.into_iter().map(|b| (f[0].0, b)).collect();
        let zip = package(home, Some(&manifest_for(&f[..1])), &data);
        assert!(archive::install(home, &zip, "1.0.0").is_err());
    }
    let repeated = vec![b'a'; MAX_FILE as usize];
    let large = [("s/f", repeated.as_slice())];
    let zip = package(home, Some(&manifest_for(&large)), &large);
    assert!(archive::install(home, &zip, "1.0.0").is_err());
    assert_eq!(fs::read(home.join("skills/manifest.json")).unwrap(), marker);
    assert_eq!(fs::read(home.join("skills").join(f[0].0)).unwrap(), f[0].1);
}
/// zip 内的链接不应当变成正文，即使链接文本的长度、摘要都与清单一致。
#[test]
fn zip_symlink_is_not_a_document() {
    let h = tempfile::tempdir().unwrap();
    let zip = h.path().join("link.zip");
    let mut w = zip::ZipWriter::new(fs::File::create(&zip).unwrap());
    let opts = zip::write::SimpleFileOptions::default();
    w.start_file(MARKER, opts).unwrap();
    w.write_all(&serde_json::to_vec(&manifest_for(&[("s/a", b"target")])).unwrap())
        .unwrap();
    w.add_symlink("s/a", "target", opts).unwrap();
    w.finish().unwrap();
    assert!(archive::install(h.path(), &zip, "1.0.0").is_err());
}

/// 列表不读正文；随后即便被同长度的硬链接替换，输出路径也要重新核对原始字节。
#[test]
fn reads_only_the_current_verified_utf8_buffer() {
    use super::installed::{Listing, list, show};
    let h = tempfile::tempdir().unwrap();
    let home = h.path();
    let mut f = docs().to_vec();
    f.push(("s/nonutf", b"\xff"));
    assert!(matches!(list(home).unwrap(), Listing::Absent));
    fs::create_dir_all(home.join("skills/old")).unwrap();
    assert!(matches!(list(home).unwrap(), Listing::Absent));
    fs::write(home.join("skills/old/SKILL.md"), b"old").unwrap();
    assert!(matches!(list(home).unwrap(), Listing::Legacy));
    let zip = package(home, Some(&manifest_for(&f)), &f);
    archive::install(home, &zip, "1.0.0").unwrap();
    assert!(
        matches!(list(home).unwrap(),Listing::Installed(s) if s.len()==3 && !s[0].matches_binary)
    );
    assert_eq!(show(home, "aproxy-cli", None).unwrap(), f[0].1);
    assert!(show(home, "aproxy-cli/references", Some("a.md")).is_err());
    assert!(show(home, "APROXY-cli", None).is_err());
    assert!(show(home, "s", Some("nonutf")).is_err());
    let doc = home.join("skills").join(f[0].0);
    fs::remove_file(&doc).unwrap();
    let secret = home.join("other");
    fs::write(&secret, b"other").unwrap();
    fs::hard_link(&secret, &doc).unwrap();
    let err = show(home, "aproxy-cli", None).unwrap_err().to_string();
    assert!(err.contains("aproxy skill install") && !err.contains("other"));
}
/// 两个写者同时竞争，拿锁者完成整包，另一个不碰它的工作区；旧进程残留的文件仍在原处。
#[test]
fn only_one_writer_publishes_and_stale_staging_is_ignored() {
    let h = tempfile::tempdir().unwrap();
    let home = h.path();
    let files = docs();
    let zip = package(home, Some(&manifest_for(&files)), &files);
    let stale = home.join("skills/.staging-dead");
    fs::create_dir_all(&stale).unwrap();
    fs::write(stale.join("partial"), b"old").unwrap();
    let ready = std::sync::Barrier::new(2);
    std::thread::scope(|s| {
        let run = || {
            let guard = super::skill_lock(home);
            ready.wait();
            let Ok(_guard) = guard else { return false };
            archive::install(home, &zip, "1.0.0").unwrap();
            true
        };
        let a = s.spawn(run);
        let b = s.spawn(run);
        assert_ne!(a.join().unwrap(), b.join().unwrap());
    });
    assert_eq!(fs::read(stale.join("partial")).unwrap(), b"old");
    for (k, b) in files {
        assert_eq!(fs::read(home.join("skills").join(k)).unwrap(), b);
    }
}

/// 缺少所有权凭据时，即使升级包里有同名目录也不能移走用户的文件。
#[test]
fn unowned_destination_survives_even_with_unreadable_manifest() {
    for broken in [false, true] {
        let h = tempfile::tempdir().unwrap();
        let f = docs();
        let z = package(h.path(), Some(&manifest_for(&f[..1])), &f[..1]);
        archive::install(h.path(), &z, "1.0.0").unwrap();
        let root = h.path().join("skills");
        let collision = if broken {
            fs::write(root.join(MARKER), b"bad").unwrap();
            "aproxy-cli"
        } else {
            fs::create_dir(root.join("aproxy-format")).unwrap();
            "aproxy-format"
        };
        let note = root.join(collision).join("notes");
        fs::write(&note, b"keep").unwrap();
        let marker = fs::read(root.join(MARKER)).unwrap();
        let z = package(h.path(), Some(&manifest_for(&f)), &f);
        let msg = archive::install(h.path(), &z, "1.0.0")
            .unwrap_err()
            .to_string();
        assert!(msg.contains(collision) && msg.contains("aproxy skill install"));
        assert_eq!(fs::read(note).unwrap(), b"keep");
        assert_eq!(fs::read(root.join(MARKER)).unwrap(), marker);
    }
}

/// 即使锁链接的目标尚不存在也不能在 skill 目录外建文件；已有目标也不能充当锁。
#[test]
fn skill_lock_refuses_link_and_non_file() {
    #[cfg(unix)]
    use std::os::unix::fs::symlink as link;
    #[cfg(windows)]
    use std::os::windows::fs::symlink_file as link;
    let h = tempfile::tempdir().unwrap();
    let lock = h.path().join("skills/.lock");
    fs::create_dir_all(&lock).unwrap();
    assert!(super::skill_lock(h.path()).is_err());
    fs::remove_dir(&lock).unwrap();
    let target = h.path().join("other");
    if let Err(e) = link(&target, &lock) {
        eprintln!("跳过符号链接检查：{e}");
        return;
    }
    assert!(super::skill_lock(h.path()).is_err());
    assert!(!target.exists());
    fs::write(&target, b"keep").unwrap();
    assert!(super::skill_lock(h.path()).is_err());
}
