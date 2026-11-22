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

use aproxy_ipc::desktop::{
    Authority, Bundle, Consent, DesktopEvent, ExtDecision, ExtKey, ExtOutcome, ExtStateReason,
    ExtensionResult, ExtensionWrite, InstanceOps, ScriptSupport, StartTarget,
};
use std::sync::Arc;
fn attach(h: &DesktopHome, support: ScriptSupport) {
    h.extension_attach("session".into(), support, Arc::new(|_| {}));
}
#[test]
fn themes_need_no_script_engine_and_replacement_requires_new_opt_in() {
    let h = TestHome::new();
    let t = fixture(&h, "paper-lilac");
    fixture(&h, "violet-desk");
    let host = host(&h);
    attach(&host, ScriptSupport::Unverified);
    let inv = host.extension_inventory();
    assert_eq!(inv.state_fault, None);
    let k = &inv.packages[0].principal.key;
    let page = &inv.packages[1].principal.key;
    assert_eq!(
        host.extension_enable(page, true),
        Err(Fault::DesktopExtensionRuntime {})
    );
    assert!(matches!(
        host.extension_open("session", page),
        Err(Fault::DesktopExtensionRuntime {})
    ));
    host.extension_enable(k, true).unwrap();
    assert!(matches!(
        host.extension_open("wrong", k),
        Err(Fault::DesktopChanged {})
    ));
    let Bundle::Theme { theme } = host.extension_open("session", k).unwrap() else {
        panic!()
    };
    assert!(theme.starts_with("aproxy-theme-v1:"));
    let bytes = fs::read(t.join("theme.txt")).unwrap();
    fs::write(t.join("theme.txt"), [bytes.clone(), vec![b' ']].concat()).unwrap();
    assert!(!host.extension_inventory().packages[0].enabled);
    fs::write(t.join("theme.txt"), bytes).unwrap();
    assert!(matches!(
        host.extension_open("session", k),
        Err(Fault::DesktopExtensionDenied {})
    ));
}
#[test]
fn unreadable_or_newer_authority_is_not_reset_or_treated_as_enabled() {
    let h = TestHome::new();
    fixture(&h, "paper-lilac");
    let host = host(&h);
    let p = h.path().join("desktop/extension-state.json");
    for (data, reason) in [
        ("{", ExtStateReason::Corrupt),
        (
            r#"{"schemaVersion":2,"future":true}"#,
            ExtStateReason::TooNew,
        ),
        (
            r#"{"schemaVersion":1,"enabled":{},"grants":[42],"audit":[]}"#,
            ExtStateReason::Corrupt,
        ),
    ] {
        fs::write(&p, data).unwrap();
        let i = host.extension_inventory();
        assert_eq!(i.state_fault, Some(Fault::DesktopExtensionState { reason }));
        assert_eq!(i.packages.len(), 1);
        assert!(!i.packages[0].enabled);
        assert_eq!(
            host.extension_enable(&i.packages[0].principal.key, true),
            Err(Fault::DesktopExtensionState { reason })
        );
        assert_eq!(fs::read_to_string(&p).unwrap(), data);
    }
}
struct AttachedTest {
    home: TestHome,
    host: DesktopHome,
    key: ExtKey,
    lease: String,
    target: StartTarget,
    port: u16,
    notice: tokio::sync::broadcast::Receiver<DesktopEvent>,
}
impl AttachedTest {
    async fn new() -> Self {
        let home = TestHome::new();
        let host = host(&home);
        let dir = fixture(&home, "violet-desk");
        let p = dir.join("manifest.json");
        let mut m: Value = serde_json::from_slice(&fs::read(&p).unwrap()).unwrap();
        m["permissions"] = json!(["instance-start", "instance-stop", "install-update"]);
        fs::write(p, m.to_string()).unwrap();
        let port = common::free_port();
        home.settings(r#"{"watchdog":{"enabled":false}}"#);
        fs::write(
            home.path().join("config.toml"),
            format!("base_url='http://127.0.0.1:9'\nlisten_addr='127.0.0.1:{port}'\n"),
        )
        .unwrap();
        let (tx, notice) = tokio::sync::broadcast::channel(64);
        host.extension_attach(
            "session".into(),
            ScriptSupport::Available,
            Arc::new(move |e| {
                let _ = tx.send(e);
            }),
        );
        let key = host.extension_inventory().packages[0].principal.key.clone();
        host.extension_enable(&key, true).unwrap();
        let Bundle::Page(b) = host.extension_open("session", &key).unwrap() else {
            panic!()
        };
        let target = host
            .extension_reader(&b.lease)
            .unwrap()
            .list_configs()
            .await
            .unwrap()
            .remove(0)
            .target;
        Self {
            home,
            host,
            key,
            lease: b.lease,
            target,
            port,
            notice,
        }
    }
    fn write(&self, op: ExtensionWrite) -> tokio::task::JoinHandle<Result<ExtensionResult, Fault>> {
        let h = self.host.clone();
        let l = self.lease.clone();
        tokio::spawn(async move {
            h.extension_write(
                l,
                op,
                Arc::new(|| panic!("instance action reserved installer")),
                Arc::new(|_| {}),
            )
            .await
        })
    }
    fn start(&self) -> tokio::task::JoinHandle<Result<ExtensionResult, Fault>> {
        self.write(ExtensionWrite::Start(self.target.clone()))
    }
    async fn prompts(&mut self, n: usize) -> Vec<Consent> {
        tokio::time::timeout(std::time::Duration::from_secs(20), async {
            loop {
                let v = self.host.extension_pending("session").unwrap();
                if v.len() == n {
                    return v;
                }
                self.notice.recv().await.unwrap();
            }
        })
        .await
        .unwrap()
    }
}
#[tokio::test]
async fn only_the_original_request_executes_and_a_remembered_verb_can_be_revoked() {
    let mut a = AttachedTest::new().await;
    let first = a.start();
    let c = a.prompts(1).await.remove(0);
    assert_eq!(
        a.host
            .extension_answer("other", &c.token, ExtDecision::Remember),
        Err(Fault::DesktopChanged {})
    );
    let path = a.home.path().join("desktop/extension-state.json");
    let data = fs::read_to_string(&path).unwrap();
    assert!(!data.contains(&c.token) && !data.contains(&a.lease));
    a.home.stop_on_drop(a.port);
    a.host
        .extension_answer("session", &c.token, ExtDecision::Remember)
        .unwrap();
    assert_eq!(
        a.host
            .extension_answer("session", &c.token, ExtDecision::Remember),
        Err(Fault::DesktopChanged {})
    );
    let ExtensionResult::Run(run) = first.await.unwrap().unwrap() else {
        panic!()
    };
    assert_eq!(run.port, a.port);
    let grant = a.host.extension_inventory().packages[0].grants.remove(0);
    let ExtensionResult::Run(existing) = a.start().await.unwrap().unwrap() else {
        panic!()
    };
    assert_eq!(existing, run);
    let audit = a.host.extension_audit().unwrap();
    assert!(matches!(&audit[0].authority,Authority::Grant{grant_id} if grant_id==&grant.grant_id));
    assert!(
        matches!(&audit[1].authority,Authority::Confirmation{decision_id,..} if decision_id==&c.decision_id)
    );
    assert!(audit.iter().all(|r| r.outcome == ExtOutcome::Succeeded));
    let stop = a.write(ExtensionWrite::Stop(run));
    let c = a.prompts(1).await.remove(0);
    a.host
        .extension_answer("session", &c.token, ExtDecision::Deny)
        .unwrap();
    assert!(matches!(
        stop.await.unwrap(),
        Err(Fault::DesktopExtensionDenied {})
    ));
    a.host.extension_revoke(&a.key, &grant.grant_id).unwrap();
    let next = a.start();
    a.prompts(1).await;
    a.host.extension_detach(None);
    assert!(matches!(
        next.await.unwrap(),
        Err(Fault::DesktopExtensionDenied {})
    ));
}
#[tokio::test]
async fn changed_package_or_launch_plan_cannot_consume_a_confirmation() {
    let mut a = AttachedTest::new().await;
    let first = a.start();
    let c = a.prompts(1).await.remove(0);
    a.home
        .settings(r#"{"watchdog":{"enabled":false},"defaults":{"timeouts":{"connect_secs":5}}}"#);
    a.host
        .extension_answer("session", &c.token, ExtDecision::Remember)
        .unwrap();
    assert!(matches!(
        first.await.unwrap(),
        Err(Fault::DesktopChanged {})
    ));
    assert!(a.host.extension_inventory().packages[0].grants.is_empty());
    let second = a.start();
    let c = a.prompts(1).await.remove(0);
    fs::write(
        a.home
            .path()
            .join("desktop/extensions/violet-desk/main.mjs"),
        b"export default {};",
    )
    .unwrap();
    a.host
        .extension_answer("session", &c.token, ExtDecision::Once)
        .unwrap();
    assert!(second.await.unwrap().is_err());
    assert!(!a.host.extension_inventory().packages[0].enabled);
    assert!(a.host.extension_reader(&a.lease).is_err());
}
#[tokio::test]
async fn pending_capacity_timeout_and_crash_are_fail_closed() {
    let mut a = AttachedTest::new().await;
    let one = a.start();
    let two = a.start();
    let pending = a.prompts(2).await;
    assert!(matches!(
        a.start().await.unwrap(),
        Err(Fault::DesktopBusy {})
    ));
    let path = a.home.path().join("desktop/extension-state.json");
    let pending_disk = fs::read(&path).unwrap();
    a.host.extension_close(&a.lease);
    assert!(one.await.unwrap().is_err() && two.await.unwrap().is_err());
    assert!(a.host.extension_pending("session").unwrap().is_empty());
    fs::write(&path, pending_disk).unwrap();
    let fresh = host(&a.home);
    assert!(
        fresh
            .extension_audit()
            .unwrap()
            .iter()
            .all(|r| r.outcome == ExtOutcome::Denied)
    );
    attach(&fresh, ScriptSupport::Available);
    assert_eq!(
        fresh.extension_answer("session", &pending[0].token, ExtDecision::Once),
        Err(Fault::DesktopChanged {})
    );
    let mut b = AttachedTest::new().await;
    let write = b.start();
    let c = b.prompts(1).await.remove(0);
    tokio::time::pause();
    tokio::time::advance(std::time::Duration::from_secs(61)).await;
    let result = write.await.unwrap();
    tokio::time::resume();
    assert!(matches!(result, Err(Fault::DesktopExtensionDenied {})));
    assert_eq!(
        b.host
            .extension_answer("session", &c.token, ExtDecision::Remember),
        Err(Fault::DesktopChanged {})
    );
}

#[tokio::test]
async fn install_is_one_shot_and_only_an_admitted_operation_reserves_the_worker() {
    use aproxy_ipc::desktop::{ChannelChoice, UpdateOps};
    use std::sync::atomic::{AtomicUsize, Ordering};
    let mut a = AttachedTest::new().await;
    let exe = aproxy::install::swap::bin_path_in(a.home.path());
    fs::create_dir_all(exe.parent().unwrap()).unwrap();
    let out = a
        .home
        .cmd_with("rustc")
        .args(["--edition=2024"])
        .arg(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/common/desktop_installer_fixture.rs"),
        )
        .arg("-o")
        .arg(exe)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let count = Arc::new(AtomicUsize::new(0));
    let start = |a: &AttachedTest, check: String| {
        let count = count.clone();
        let h = a.host.clone();
        let l = a.lease.clone();
        tokio::spawn(async move {
            h.extension_write(
                l,
                ExtensionWrite::Install(check),
                Arc::new(move || {
                    count.fetch_add(1, Ordering::SeqCst);
                    Ok(Box::new(()))
                }),
                Arc::new(|_| {}),
            )
            .await
        })
    };
    let check = a.host.check_update(ChannelChoice::Auto).await.unwrap();
    assert_eq!(
        a.host.extension_update_offer(&a.lease).unwrap(),
        Some(check.clone())
    );
    let id = check.check.unwrap();
    let denied = start(&a, id.clone());
    let c = a.prompts(1).await.remove(0);
    assert!(!c.can_remember);
    assert_eq!(
        a.host
            .extension_answer("session", &c.token, ExtDecision::Remember),
        Err(Fault::DesktopBadInput {})
    );
    assert!(denied.await.unwrap().is_err());
    assert_eq!(count.load(Ordering::SeqCst), 0);
    let stale = start(&a, id);
    let c = a.prompts(1).await.remove(0);
    let id = a
        .host
        .check_update(ChannelChoice::Auto)
        .await
        .unwrap()
        .check
        .unwrap();
    a.host
        .extension_answer("session", &c.token, ExtDecision::Once)
        .unwrap();
    assert!(stale.await.unwrap().is_err());
    assert_eq!(count.load(Ordering::SeqCst), 0);
    let ok = start(&a, id);
    let c = a.prompts(1).await.remove(0);
    a.host
        .extension_answer("session", &c.token, ExtDecision::Once)
        .unwrap();
    assert!(matches!(
        ok.await.unwrap(),
        Ok(ExtensionResult::Installed(_))
    ));
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert!(a.host.extension_update_offer(&a.lease).unwrap().is_none());
    assert!(a.host.extension_inventory().packages[0].grants.is_empty());
    // 已准许执行但没有终态的记录尤其不能重放。模拟在提交 executing 后中断。
    let f = a.home.path().join("desktop/extension-state.json");
    let mut d: Value = serde_json::from_slice(&fs::read(&f).unwrap()).unwrap();
    let row = d["audit"].as_array_mut().unwrap().last_mut().unwrap();
    row["outcome"] = json!("executing");
    row["finishedAt"] = Value::Null;
    fs::write(f, d.to_string()).unwrap();
    assert_eq!(
        host(&a.home).extension_audit().unwrap()[0].outcome,
        ExtOutcome::Unknown
    );
    assert_eq!(count.load(Ordering::SeqCst), 1);
}
