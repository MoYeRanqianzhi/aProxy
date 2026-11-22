//! 临时主目录里的真实包与畸形目录；错误只比较结构化字段，不依赖操作系统语言。
mod common;
use aproxy::desktop::DesktopHome;
use aproxy_ipc::desktop::{ExtField as F, Fault, InvalidReason as W};
use common::TestHome;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};

fn host(h: &TestHome) -> DesktopHome {
    DesktopHome::new(h.path().into(), env!("CARGO_BIN_EXE_aproxy").into()).unwrap()
}
fn copy(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for e in fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let dest = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy(&e.path(), &dest)
        } else {
            fs::copy(e.path(), dest).unwrap();
        }
    }
}
fn fixture(h: &TestHome, id: &str) -> PathBuf {
    let d = h.path().join("desktop/extensions").join(id);
    copy(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/desktop-extensions")
            .join(id),
        &d,
    );
    d
}
fn expect(h: &TestHome, why: W, field: F) {
    let (ok, bad) = host(h).extension_catalog();
    assert!(ok.is_empty());
    assert_eq!(bad.len(), 1, "{bad:?}");
    assert_eq!(
        bad[0].fault,
        Fault::DesktopExtensionPackage { reason: why, field }
    )
}
#[test]
fn all_kinds_are_inert_and_every_byte_selects_a_different_identity() {
    let h = TestHome::new();
    assert_eq!(host(&h).extension_catalog(), Default::default());
    for id in ["violet-desk", "little-orb", "paper-lilac"] {
        fixture(&h, id);
    }
    let read = || {
        let (good, bad) = host(&h).extension_catalog();
        assert!(bad.is_empty(), "{bad:?}");
        assert_eq!(good.len(), 3);
        assert!(good.iter().all(|p| !p.enabled && p.grants.is_empty()));
        good[2].principal.key.clone()
    };
    let before = read();
    fs::write(
        h.path()
            .join("desktop/extensions/violet-desk/assets/label.bin"),
        b"change",
    )
    .unwrap();
    let after = read();
    assert_eq!(before.id, after.id);
    assert_ne!(before.digest, after.digest);
    let path = h
        .path()
        .join("desktop/extensions/violet-desk/manifest.json");
    let mut v: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    v["ignored"] = json!({"script":"not an entry"});
    v["name"] = json!("Vi\u{202e}ol\next");
    fs::write(path, v.to_string()).unwrap();
    assert_ne!(after.digest, read().digest);
    assert_eq!(host(&h).extension_catalog().0[2].principal.name, "Violext");
}
#[test]
fn manifest_field_failures_and_unsafe_paths_never_acquire_an_identity() {
    let h = TestHome::new();
    let d = fixture(&h, "violet-desk");
    let p = d.join("manifest.json");
    let orig = fs::read(&p).unwrap();
    let base: Value = serde_json::from_slice(&orig).unwrap();
    for (k, v, w, f) in [
        (
            "manifestVersion",
            json!(2),
            W::ManifestVersion,
            F::ManifestVersion,
        ),
        ("id", json!("other"), W::Field, F::Id),
        ("name", json!("\n"), W::Field, F::Name),
        ("version", json!("v1"), W::Field, F::Version),
        ("kind", json!("iframe"), W::Field, F::Kind),
        ("entry", json!("theme.txt"), W::Field, F::Entry),
        ("permissions", json!(["config"]), W::Field, F::Permissions),
        (
            "permissions",
            json!(["instance-stop", "instance-stop"]),
            W::Field,
            F::Permissions,
        ),
    ] {
        let mut m = base.clone();
        m[k] = v;
        fs::write(&p, m.to_string()).unwrap();
        expect(&h, w, f)
    }
    for entry in [
        "../main.mjs",
        "/tmp/app",
        "C:\\app",
        "\\\\server\\share",
        "main.mjs:other",
        "main.mjs.",
        "com1.mjs",
        "MAIN~1.MJS",
        "x%2fy",
    ] {
        let mut m = base.clone();
        m["entry"] = entry.into();
        fs::write(&p, m.to_string()).unwrap();
        expect(&h, W::Path, F::Entry)
    }
    for text in ["{", "[]", r#"{"a":1,"a":2}"#, r#"{"a":[[[[[[[[0]]]]]]]]}"#] {
        fs::write(&p, text).unwrap();
        expect(&h, W::Json, F::Manifest)
    }
    fs::write(&p, orig).unwrap();
    fs::write(d.join("main.mjs"), [0xff]).unwrap();
    expect(&h, W::Text, F::Entry);
}
#[test]
fn limits_bound_traversal_and_never_admit_an_arbitrary_prefix() {
    let h = TestHome::new();
    let d = fixture(&h, "violet-desk");
    let module = d.join("main.mjs");
    fs::write(&module, vec![b'x'; 256 * 1024]).unwrap();
    assert_eq!(host(&h).extension_catalog().0.len(), 1);
    fs::write(&module, vec![b'x'; 256 * 1024 + 1]).unwrap();
    expect(&h, W::FileBytes, F::Entry);
    fs::write(&module, vec![b'x'; 256 * 1024]).unwrap();
    for i in 0..4 {
        fs::write(d.join(format!("assets/a{i}.bin")), vec![0u8; 64 * 1024]).unwrap();
    }
    expect(&h, W::PackageBytes, F::Directory);
    for i in 4..8 {
        fs::write(d.join(format!("assets/a{i}.bin")), []).unwrap();
    }
    expect(&h, W::FileCount, F::Assets);
    for i in 0..24 {
        fs::create_dir(d.parent().unwrap().join(format!("p{i}"))).unwrap()
    }
    expect(&h, W::PackageCount, F::Root);
}
#[test]
fn theme_layout_and_grammar_are_checked_before_display() {
    let h = TestHome::new();
    let d = fixture(&h, "paper-lilac");
    for text in [
        ".red{color:red}",
        "aproxy-theme-v2:{}",
        r##"aproxy-theme-v1:{"name":"x","light":{"primary":"url(test)"}}"##,
        r#"aproxy-theme-v1:{"name":"x","radius":2}"#,
        r#"aproxy-theme-v1:{"name":"x","titlebarMin":35}"#,
        r#"aproxy-theme-v1:{"name":"x","contrast":"bad"}"#,
    ] {
        fs::write(d.join("theme.txt"), text).unwrap();
        expect(&h, W::Theme, F::Theme)
    }
    fs::write(d.join("main.mjs"), b"unused").unwrap();
    expect(&h, W::Layout, F::Theme);
}
#[test]
fn a_non_directory_deep_tree_link_or_known_secret_cannot_be_loaded() {
    let h = TestHome::new();
    let d = fixture(&h, "violet-desk");
    fs::create_dir(d.join("assets/deep.bin")).unwrap();
    expect(&h, W::Depth, F::Assets);
    fs::remove_dir(d.join("assets/deep.bin")).unwrap();
    let other = h.path().join("private.dat");
    fs::write(&other, b"canary outside the tree").unwrap();
    fs::hard_link(other, d.join("assets/linked.bin")).unwrap();
    expect(&h, W::Link, F::Assets);
    let h = TestHome::new();
    let d = fixture(&h, "violet-desk");
    h.settings(
        r#"{"defaults":{"base_url":"https://upstream.invalid","api_key":"private-canary"}}"#,
    );
    fs::write(d.join("main.mjs"), b"// private-canary").unwrap();
    expect(&h, W::PrivateData, F::Entry);
    let h = TestHome::new();
    let root = h.path().join("desktop/extensions");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("fake"), b"not a directory").unwrap();
    expect(&h, W::FileType, F::Directory);
}
#[test]
fn linked_root_is_not_enumerated() {
    let h = TestHome::new();
    let desktop = h.path().join("desktop");
    fs::create_dir_all(&desktop).unwrap();
    let outside = h.path().join("elsewhere");
    fs::create_dir(&outside).unwrap();
    let link = desktop.join("extensions");
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside, link).unwrap();
    #[cfg(windows)]
    assert!(
        h.cmd_with("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(link)
            .arg(outside)
            .output()
            .unwrap()
            .status
            .success()
    );
    expect(&h, W::Link, F::Root);
}

#[cfg(windows)]
#[test]
fn sharing_conflicts_are_not_projected_as_valid_empty_files() {
    use std::os::windows::fs::OpenOptionsExt;
    let h = TestHome::new();
    let d = fixture(&h, "little-orb");
    let _held = fs::File::options()
        .read(true)
        .share_mode(0)
        .open(d.join("main.mjs"))
        .unwrap();
    expect(&h, W::Io, F::Directory);
}
#[cfg(unix)]
#[test]
fn fifo_and_component_links_are_refused_before_reading() {
    use std::{
        ffi::CString,
        os::unix::{ffi::OsStrExt, fs::symlink},
    };
    let h = TestHome::new();
    let d = fixture(&h, "violet-desk");
    let f = d.join("assets/a.bin");
    let c = CString::new(f.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    expect(&h, W::FileType, F::Assets);
    fs::remove_file(&f).unwrap();
    symlink(h.path().join("settings.json"), &f).unwrap();
    expect(&h, W::Link, F::Assets);
}
#[test]
fn digest_uses_path_lengths_and_unmodified_file_bytes() {
    let h = TestHome::new();
    let d = h.path().join("desktop/extensions/d");
    fs::create_dir_all(&d).unwrap();
    fs::write(d.join("main.mjs"), b"x").unwrap();
    fs::write(d.join("manifest.json"),br#"{"manifestVersion":1,"id":"d","kind":"page","entry":"main.mjs","name":"D","version":"1.0.0","permissions":[]}"#).unwrap();
    assert_eq!(
        host(&h).extension_catalog().0[0].principal.key.digest,
        "2420988b183670f0de4c5f401c5869f3017ede1d4345a7795285bf25e800b9be"
    );
}
