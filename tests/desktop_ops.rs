//! 桌面宿主的测试只观察自己的临时主目录，不启动 Tauri，也不依赖当前用户配置。
mod common;
use aproxy::{desktop::DesktopHome, spec};
use aproxy_ipc::enumerate::{Enumerated, Liveness};
use common::{TestHome, free_port};
use serde_json::json;

#[test]
fn discovery_secrets_and_desire_tokens_are_bound_to_the_home_and_window() {
    let home = TestHome::new();
    let host = DesktopHome::new(home.path().into(), env!("CARGO_BIN_EXE_aproxy").into()).unwrap();
    let port = free_port();
    let file = home.path().join("test.toml");
    std::fs::write(&file,"base_url='https://api.invalid'\nproxy='http://user:pa%22ssword-long@proxy.invalid/?q=query-credential'\napi_key='file-credential'\n[extra_headers]\nx-safe-name='header-credential'\n").unwrap();
    std::fs::write(
        home.path().join("settings.json"),
        serde_json::to_vec(&json!({"defaults":{"base_url":"https://other.invalid", "api_key":"withheld-credential"},"update":{"download_proxy":"http://user:update-proxy-secret@localhost","download_chain":[{"url":"https://example.invalid/{asset}?key=chain-credential"}]}})).unwrap(),
    )
    .unwrap();
    let mut desired = spec::InstanceSpec {
        schema: spec::SCHEMA,
        config_path: Some(file.display().to_string()),
        overrides: Default::default(),
        pinned_port: None,
        env: Default::default(),
        log_path: home.path().join("logs/test").display().to_string(),
    };
    desired.overrides.api_key = Some(r#"private-"\credential"#.into());
    let run = home.path().join("run");
    spec::write_restore_in(&run, &port.to_string(), &desired).unwrap();
    let sample = [Enumerated {
        port,
        liveness: Liveness::Down,
        instance: None,
        status: None,
        reason: None,
        restore_pending: true,
        restore_reason: None,
    }];
    let observe = |host: &DesktopHome| host.inspect(&sample).remove(&port).unwrap();
    let f = observe(&host);
    for secret in [
        "query-credential",
        "pa\"ssword-long",
        "file-credential",
        "header-credential",
        "withheld-credential",
        "update-proxy-secret",
        "chain-credential",
        r#"private-"\credential"#,
    ] {
        assert!(!f.secrets.replace(secret).contains(secret));
    }
    let token = f.restore.unwrap();
    assert_eq!(Some(token.clone()), observe(&host).restore);
    let other = DesktopHome::new(home.path().into(), env!("CARGO_BIN_EXE_aproxy").into()).unwrap();
    assert_ne!(Some(token.clone()), observe(&other).restore);
    desired.overrides.api_key = Some("different-credential".into());
    spec::write_restore_in(&run, &port.to_string(), &desired).unwrap();
    assert_ne!(Some(token), observe(&host).restore);
    std::fs::write(&file, "api_key={ secret='private-credential' }").unwrap();
    assert!(!host.inspect(&sample).contains_key(&port));
}

#[tokio::test]
async fn explicit_home_control_keeps_the_selected_process_alive_on_identity_mismatch() {
    use aproxy::{config::Overrides, ops::start};
    use aproxy_ipc::client::shutdown_for_run_in;
    let home = TestHome::new();
    home.settings(r#"{"watchdog":{"enabled":false},"defaults":{"base_url":"http://127.0.0.1:9"}}"#);
    let file = home.path().join("config.toml");
    std::fs::write(&file, "listen_addr='127.0.0.1:0'\n").unwrap();
    let cfg =
        start::resolve_runtime_config_in(home.path(), &Overrides::default(), Some(&file)).unwrap();
    let log = start::daemon_log_path_in(home.path(), Some("logs/own.log"));
    assert!(log.starts_with(home.path()));
    let spec = spec::capture(Some(&file), Default::default(), &log);
    let exe = std::path::Path::new(env!("CARGO_BIN_EXE_aproxy"));
    assert!(matches!(
        start::preflight_in(home.path(), &cfg.listen_addr)
            .await
            .unwrap(),
        start::Preflight::Clear
    ));
    let live = start::launch_in(home.path(), exe, &spec)
        .await
        .unwrap()
        .instance;
    let port = aproxy::daemon::port_of(&live.listen_addr);
    home.stop_on_drop(port.parse().unwrap());
    let dir = home.path().join("run");
    let mut old = live.clone();
    old.instance_id.push('x');
    assert!(matches!(
        shutdown_for_run_in(&dir, port, &old, false).await,
        Err(aproxy_ipc::client::IpcError::RunChanged)
    ));
    let mut recycled = live.clone();
    recycled.start.push('x');
    assert!(matches!(
        shutdown_for_run_in(&dir, port, &recycled, false).await,
        Err(aproxy_ipc::client::IpcError::RunChanged)
    ));
    assert_eq!(
        aproxy::daemon::ipc_ping_in(&dir, port)
            .await
            .unwrap()
            .instance,
        live
    );
    shutdown_for_run_in(&dir, port, &live, false).await.unwrap();
    assert!(
        aproxy::daemon::wait_until_gone_in(&dir, port, std::time::Duration::from_secs(12)).await
    );
}

#[tokio::test]
async fn window_operations_refuse_stale_choices_replaced_runs_and_responsive_force() {
    use aproxy_ipc::desktop::{ChoiceKind, Fault};
    let home = TestHome::new();
    home.settings(r#"{"watchdog":{"enabled":false},"aliases":{"primary":"unused"}}"#);
    let file = home.path().join("config.toml");
    std::fs::write(
        &file,
        "base_url='http://127.0.0.1:9'\nlisten_addr='127.0.0.1:0'\napi_key='test-private-key'\n",
    )
    .unwrap();
    let host = DesktopHome::new(home.path().into(), env!("CARGO_BIN_EXE_aproxy").into()).unwrap();
    let ops: &dyn aproxy_ipc::desktop::DesktopOps = &host;
    let first = ops.list_configs().await.unwrap();
    let stale = first[0].target.clone();
    let fresh = ops.list_configs().await.unwrap();
    assert!(matches!(
        ops.start_instance(stale).await,
        Err(Fault::DesktopChanged {})
    ));
    let start = fresh
        .iter()
        .find(|o| o.target.kind == ChoiceKind::Default)
        .unwrap();
    let one = ops.start_instance(start.target.clone()).await.unwrap();
    home.stop_on_drop(one.port);
    assert!(matches!(
        ops.force_stop_instance(one.clone()).await,
        Err(Fault::DesktopForceRefused { .. })
    ));
    let two = ops.restart_instance(one.clone()).await.unwrap();
    home.stop_on_drop(two.port);
    assert_ne!(one.instance_id, two.instance_id);
    assert!(matches!(
        ops.stop_instance(one.clone()).await,
        Err(Fault::DesktopChanged {})
    ));
    assert!(matches!(
        ops.restart_instance(one.clone()).await,
        Err(Fault::DesktopChanged {})
    ));
    assert!(matches!(
        ops.force_stop_instance(one).await,
        Err(Fault::DesktopChanged {})
    ));
    let dir = home.path().join("run");
    let p = two.port.to_string();
    let saved = spec::read_restore_in(&dir, &p).unwrap().unwrap().spec;
    assert_eq!(
        aproxy::daemon::ipc_ping_in(&dir, &p)
            .await
            .unwrap()
            .instance
            .instance_id,
        two.instance_id
    );
    ops.stop_instance(two.clone()).await.unwrap();
    assert!(matches!(
        ops.forget_restore(two.port, String::new()).await,
        Err(Fault::DesktopTarget {})
    ));
    spec::write_restore_in(&dir, &p, &saved).unwrap();
    let sample = aproxy_ipc::enumerate::enumerate_in(home.path())
        .await
        .instances;
    let observe = || {
        host.inspect(&sample)
            .remove(&two.port)
            .unwrap()
            .restore
            .unwrap()
    };
    let token = observe();
    let mut other = saved.clone();
    other.overrides.api_key = Some("replaced-private-key".into());
    spec::write_restore_in(&dir, &p, &other).unwrap();
    assert!(matches!(
        ops.forget_restore(two.port, token.clone()).await,
        Err(Fault::DesktopChanged {})
    ));
    assert!(matches!(
        ops.restore_instance(two.port, token).await,
        Err(Fault::DesktopChanged {})
    ));
    let three = ops.restore_instance(two.port, observe()).await.unwrap();
    home.stop_on_drop(three.port);
    ops.stop_instance(three).await.unwrap();
    spec::write_restore_in(&dir, &p, &saved).unwrap();
    ops.forget_restore(two.port, observe()).await.unwrap();
    assert!(spec::read_restore_in(&dir, &p).unwrap().is_none());
}

#[test]
fn unresponsive_test_child() {
    if std::env::var_os("APROXY_TEST_SLEEP_FOR_PARENT").is_some() {
        std::thread::sleep(std::time::Duration::from_secs(60));
    }
}

#[tokio::test]
async fn force_targets_only_the_owned_unresponsive_test_process() {
    use aproxy_ipc::desktop::{InstanceOps, Run};
    let home = TestHome::new();
    let port = free_port();
    let dir = home.path().join("run");
    let mut child = home
        .cmd_with(std::env::current_exe().unwrap())
        .args(["--exact", "unresponsive_test_child"])
        .env("APROXY_TEST_SLEEP_FOR_PARENT", "1")
        .spawn()
        .unwrap();
    let pid = child.id();
    let start = aproxy_ipc::ledger::start_marker(pid).unwrap();
    let id = "aced000000000001".to_owned();
    let rec = aproxy::daemon::InstanceRecord {
        pid,
        start,
        instance_id: id.clone(),
        version: "0.2.0".into(),
        listen_addr: format!("127.0.0.1:{port}"),
        config_path: None,
        base_url: "http://127.0.0.1:9".into(),
        started_at: 1,
        log_path: None,
        foreground: false,
    };
    aproxy::daemon::write_instance_file_in(&dir, &rec).unwrap();
    let host = DesktopHome::new(home.path().into(), env!("CARGO_BIN_EXE_aproxy").into()).unwrap();
    let result = host
        .force_stop_instance(Run {
            port,
            instance_id: id,
        })
        .await;
    if result.is_err() {
        let _ = child.kill();
    }
    child.wait().unwrap();
    result.unwrap();
    assert!(aproxy::daemon::read_instance_file_in(&dir, &port.to_string()).is_none());
}

#[tokio::test]
async fn file_form_preserves_unedited_text_and_never_echoes_credentials() {
    use aproxy_ipc::desktop::{ConfigOps, ConfigTarget, Edit, Fault, InstanceOps};
    let home = TestHome::new();
    let host = DesktopHome::new(home.path().into(), env!("CARGO_BIN_EXE_aproxy").into()).unwrap();
    let p = home.path().join("config.toml");
    let old = r#"# keep this comment
base_url = 'https://example.invalid/v1?token=private-query'
api_key = 'private-"\long-key' # private
proxy = 'http://proxy-user:private-password@localhost:8080'
[extra_headers]
'x.team' = 'private-header' # keep header comment
"#;
    std::fs::write(&p, old).unwrap();
    home.settings(r#"{"ui":{"raw":"ui-private-text"},"watchdog":{"enabled":false}}"#);
    let target = ConfigTarget::Default {
        id: host.list_configs().await.unwrap()[0].target.id.clone(),
    };
    let read = host.read_config(target.clone()).await.unwrap();
    assert!(!read.external_acl);
    let assert_safe = |v: serde_json::Value| {
        let raw = v.to_string();
        for secret in [
            r#"private-\"\\long-key"#,
            "private-query",
            "proxy-user",
            "private-password",
            "private-header",
            "private-new-header",
            "ui-private-text",
        ] {
            assert!(!raw.contains(secret), "secret escaped projection");
        }
    };
    assert!(read.fields.iter().any(|f| f.key == "api_key"
        && matches!(f.value, aproxy_ipc::desktop::Cell::Secret { set: true })));
    assert_safe(json!(read));
    assert_safe(json!(host.read_settings().await.unwrap()));
    let patch = || -> Vec<Edit> {
        serde_json::from_value(json!([{ "op":"set-header","key":"extra_headers","name":"x.team","value":"private-new-header"},{"op":"set","key":"timeouts.connect_secs","value":19}])).unwrap()
    };
    let validation = host.validate_config(target.clone(), patch()).await.unwrap();
    assert!(validation.can_save);
    assert_safe(json!(validation));
    assert_eq!(std::fs::read_to_string(&p).unwrap(), old);
    let saved = host
        .write_config(target.clone(), patch(), false)
        .await
        .unwrap();
    assert!(saved.changed && saved.instances_to_restart.is_empty());
    let updated = std::fs::read_to_string(&p).unwrap();
    assert!(updated.starts_with(old.split("'x.team'").next().unwrap()));
    assert!(updated.contains("# keep header comment") && updated.contains("private-new-header"));
    assert!(
        !host
            .write_config(target.clone(), patch(), false)
            .await
            .unwrap()
            .changed
    );
    let invalid: Vec<Edit> = serde_json::from_value(
        json!([{"op":"set","key":"listen_addr","value":"private-new-header"}]),
    )
    .unwrap();
    let fault = host.write_config(target, invalid, false).await.unwrap_err();
    assert!(matches!(fault, Fault::DesktopConfig { .. }));
    assert_safe(json!(fault));
    assert_eq!(std::fs::read_to_string(&p).unwrap(), updated);
    assert!(host.set_watchdog(true).await.unwrap());
    assert!(host.read_settings().await.unwrap().watchdog_enabled);
    assert!(
        std::fs::read_to_string(home.path().join("settings.json"))
            .unwrap()
            .contains("ui-private-text")
    );
}

#[tokio::test]
async fn implicit_default_can_be_created_but_external_target_needs_windows_acknowledgement() {
    use aproxy_ipc::desktop::{ConfigOps, ConfigTarget, Edit, Fault, InstanceOps};
    let home = TestHome::new();
    let elsewhere = TestHome::new();
    let host = DesktopHome::new(home.path().into(), env!("CARGO_BIN_EXE_aproxy").into()).unwrap();
    let patch = || -> Vec<Edit> {
        serde_json::from_value(json!([{"op":"set","key":"base_url","value":"http://127.0.0.1:9"}]))
            .unwrap()
    };
    let target = ConfigTarget::Default {
        id: host.list_configs().await.unwrap()[0].target.id.clone(),
    };
    assert!(
        host.read_config(target.clone())
            .await
            .unwrap()
            .fields
            .iter()
            .any(|f| f.key == "base_url" && f.editable)
    );
    assert!(
        host.write_config(target, patch(), false)
            .await
            .unwrap()
            .changed
    );
    let external = elsewhere.path().join("outside.toml");
    std::fs::write(&external, "# external\n").unwrap();
    home.settings(&json!({"aliases":{"outside":external}}).to_string());
    let alias = host
        .list_configs()
        .await
        .unwrap()
        .into_iter()
        .find(|c| c.label == "outside")
        .unwrap();
    let t = ConfigTarget::Alias {
        id: alias.target.id,
    };
    let v = host.validate_config(t.clone(), patch()).await.unwrap();
    assert_eq!(v.external_acl, cfg!(windows));
    assert_eq!(
        host.read_config(t.clone()).await.unwrap().external_acl,
        cfg!(windows)
    );
    if cfg!(windows) {
        assert!(matches!(
            host.write_config(t.clone(), patch(), false).await,
            Err(Fault::DesktopExternalAcl {})
        ));
        assert_eq!(std::fs::read_to_string(&external).unwrap(), "# external\n");
    }
    assert!(
        host.write_config(t.clone(), patch(), true)
            .await
            .unwrap()
            .changed
    );
    home.settings(&json!({"aliases":{"outside":home.path().join("config.toml")}}).to_string());
    assert!(matches!(
        host.write_config(t, patch(), true).await,
        Err(Fault::DesktopChanged {})
    ));
}

#[tokio::test]
async fn private_window_state_is_bounded_resettable_and_write_only_for_secrets() {
    use aproxy_ipc::desktop::{Bookmark, Fault, PinChange, PreferencesPatch, StateFile, StateOps};
    let home = TestHome::new();
    home.settings(
        r#"{"defaults":{"base_url":"http://127.0.0.1:9","api_key":"private-state-key"}}"#,
    );
    let host = DesktopHome::new(home.path().into(), env!("CARGO_BIN_EXE_aproxy").into()).unwrap();
    assert_eq!(
        host.about().unwrap().variant,
        aproxy_ipc::desktop::Variant::Headless
    );
    let initial = host.read_desktop_state().await.unwrap();
    assert!(initial.reset.is_empty() && initial.bookmarks.is_empty());
    assert_eq!(initial.preferences.mascot, "tumbler");
    let patch = |v| serde_json::from_value::<PreferencesPatch>(v).unwrap();
    let p=host.set_preferences(patch(json!({"lang":"zh-Hant-TW","mascot":"future-cat","theme":"aproxy-theme-v1:{\"unknown\":true}"}))).await.unwrap();
    assert_eq!(p.lang, "zh-Hant-TW");
    assert_eq!(p.mascot, "future-cat");
    assert!(p.theme.is_some());
    assert!(matches!(
        host.set_preferences(patch(json!({"theme":"aproxy-theme-v1:not json"})))
            .await,
        Err(Fault::DesktopBadInput {})
    ));
    assert!(matches!(
        host.set_preferences(patch(json!({"lang":"EN bad"}))).await,
        Err(Fault::DesktopBadInput {})
    ));
    let p = host
        .set_preferences(patch(
            json!({"theme":"aproxy-theme-v1:\"private-state-key\""}),
        ))
        .await
        .unwrap();
    assert_eq!(p.theme, None);
    let item:Bookmark=serde_json::from_value(json!({"ref":{"run":{"port":free_port(),"instanceId":"000000000000ba11"},"requestId":"000000000000ba11-1"},"startedAt":1,"method":"GET","path":"/private-state-key","model":null,"outcome":"ok","attempts":1})).unwrap();
    let list = host.set_pinned(PinChange::Pin(item.clone())).await.unwrap();
    assert_eq!(list.len(), 1);
    assert!(!list[0].path.contains("private-state-key"));
    assert_eq!(
        host.set_pinned(PinChange::Pin(item.clone())).await.unwrap(),
        list
    );
    assert!(
        host.set_pinned(PinChange::Unpin(item.r#ref))
            .await
            .unwrap()
            .is_empty()
    );
    let dir = home.path().join("desktop");
    std::fs::write(dir.join("preferences.json"), b"[").unwrap();
    std::fs::write(dir.join("bookmarks.json"), vec![b' '; 256 * 1024 + 1]).unwrap();
    let repaired = host.read_desktop_state().await.unwrap();
    assert_eq!(
        repaired.reset,
        vec![StateFile::Preferences, StateFile::Bookmarks]
    );
    assert_eq!(repaired.preferences, initial.preferences);
    assert!(host.read_desktop_state().await.unwrap().reset.is_empty());
    std::fs::write(dir.join("preferences.json"), b"{}").unwrap();
    assert!(matches!(
        host.set_preferences(patch(json!({"mica":true}))).await,
        Err(Fault::DesktopStateReset {
            file: StateFile::Preferences
        })
    ));
}

#[tokio::test]
async fn update_children_are_home_bound_confirmed_bounded_and_terminal_only_after_exit() {
    use aproxy_ipc::desktop::{ChannelChoice, Fault, Job, UpdateOps};
    let home = TestHome::new();
    let exe = aproxy::install::swap::bin_path_in(home.path());
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    let host = DesktopHome::new(home.path().into(), env!("CARGO_BIN_EXE_aproxy").into()).unwrap();
    assert!(matches!(
        host.check_update(ChannelChoice::Auto).await,
        Err(Fault::DesktopBinary {})
    ));
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/common/desktop_installer_fixture.rs");
    let built = home
        .cmd_with("rustc")
        .arg("--edition=2024")
        .arg(fixture)
        .arg("-o")
        .arg(&exe)
        .output()
        .unwrap();
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    let one = host.check_update(ChannelChoice::Pre).await.unwrap();
    assert_eq!(one.published_at.unwrap().get(), 1767225600000);
    let token = one.check.unwrap();
    let two = host.check_update(ChannelChoice::Auto).await.unwrap();
    let jobs = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Job>::new()));
    let collected = jobs.clone();
    let progress: aproxy_ipc::desktop::Progress =
        std::sync::Arc::new(move |j| collected.lock().unwrap().push(j));
    assert!(matches!(
        host.install_update(token, progress.clone()).await,
        Err(Fault::DesktopChanged {})
    ));
    let end = host
        .install_update(two.check.unwrap(), progress.clone())
        .await
        .unwrap();
    assert_eq!(end.version.as_deref(), Some("1.1.0"));
    {
        let frames = jobs.lock().unwrap();
        let last = frames.last().unwrap();
        assert_eq!(last.end.as_ref(), Some(&end));
        assert!(last.fault.is_none());
        assert_eq!(last.download.as_ref().unwrap().bytes.get(), (1 << 53) - 1);
        assert!(
            frames[..frames.len() - 1]
                .iter()
                .all(|j| j.end.is_none() && j.fault.is_none())
        );
        assert!(!json!(last).to_string().contains("private"));
    }
    for mode in ["oversize", "wrong-exit", "duplicate", "missing", "failed"] {
        std::fs::write(home.path().join("fixture.mode"), mode).unwrap();
        let token = host
            .check_update(ChannelChoice::Auto)
            .await
            .unwrap()
            .check
            .unwrap();
        let err = host
            .install_update(token, progress.clone())
            .await
            .unwrap_err();
        if mode == "failed" {
            assert!(matches!(err, Fault::InstallRestartFailed { port: 80 }))
        } else {
            assert!(matches!(err, Fault::DesktopInstallProtocol {}))
        }
        assert_eq!(jobs.lock().unwrap().last().unwrap().fault, Some(err));
    }
    let token = host
        .check_update(ChannelChoice::Auto)
        .await
        .unwrap()
        .check
        .unwrap();
    std::fs::write(&exe, "replacement, do not execute").unwrap();
    assert!(matches!(
        host.install_update(token, progress).await,
        Err(Fault::DesktopChanged {})
    ));
}

#[test]
fn hidden_login_restore_uses_the_argument_home_and_refuses_relative_paths() {
    let decoy = TestHome::new();
    let actual = TestHome::new();
    decoy.settings("{not settings");
    let restore = |args: &[&str]| decoy.cmd().args(args).output().unwrap();
    let rel = restore(&["restore-at-login", "--home", "relative"]);
    assert_ne!(rel.status.code(), Some(0));
    let empty = restore(&[
        "restore-at-login",
        "--home",
        actual.path().to_str().unwrap(),
    ]);
    assert_eq!(empty.status.code(), Some(0));
    let port = free_port().to_string();
    let desire = spec::capture(
        Some(&actual.path().join("missing.toml")),
        Default::default(),
        &actual.path().join("logs/test.log"),
    );
    spec::write_restore_in(&actual.path().join("run"), &port, &desire).unwrap();
    let login = restore(&[
        "restore-at-login",
        "--home",
        actual.path().to_str().unwrap(),
    ]);
    let public = actual.cmd().arg("restore").output().unwrap();
    assert_eq!(login.status.code(), public.status.code());
    assert_eq!(login.stdout, public.stdout);
    assert_eq!(login.stderr, public.stderr);
    assert_eq!(login.status.code(), Some(5));
    assert!(
        spec::read_restore_in(&actual.path().join("run"), &port)
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn registration_reads_actual_per_home_slot_and_never_overwrites_a_foreign_entry() {
    use aproxy_ipc::desktop::{DesktopOps, Fault, RegistrationIssue, RegistrationReason};
    let home = TestHome::new();
    let other = TestHome::new();
    let mut bytes = [0; 16];
    getrandom::fill(&mut bytes).unwrap();
    let slot = bytes.iter().map(|n| format!("{n:02x}")).collect::<String>();
    let service = |h: &TestHome| {
        DesktopHome::new(h.path().into(), env!("CARGO_BIN_EXE_aproxy").into())
            .unwrap()
            .with_isolated_login_slot(&slot)
            .unwrap()
    };
    let h = service(&home);
    let b = service(&other);
    #[cfg(windows)]
    let (key, _guard) = {
        use winreg::{RegKey, enums::HKEY_CURRENT_USER};
        struct Cleanup(String);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let result = RegKey::predef(HKEY_CURRENT_USER).delete_subkey(&self.0);
                if !std::thread::panicking() {
                    result.unwrap()
                }
            }
        }
        let sub = format!(r"Software\aProxy-test-{slot}");
        let hk = RegKey::predef(HKEY_CURRENT_USER);
        assert!(hk.open_subkey(&sub).is_err());
        let (k, _) = hk.create_subkey(&sub).unwrap();
        (k, Cleanup(sub))
    };
    assert_eq!(
        h.read_autostart().await.unwrap().issue,
        Some(RegistrationIssue::BinaryMissing)
    );
    assert!(matches!(
        h.set_autostart(true).await,
        Err(Fault::DesktopRegistration {
            reason: RegistrationReason::BinaryMissing
        })
    ));
    for t in [&home, &other] {
        let exe = aproxy::install::swap::bin_path_in(t.path());
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(exe, "inert installed marker").unwrap();
    }
    assert!(!h.read_autostart().await.unwrap().enabled);
    assert!(h.set_autostart(true).await.unwrap().enabled);
    #[cfg(windows)]
    let (n, original) = {
        let (n, _) = key.enum_values().next().unwrap().unwrap();
        let text: String = key.get_value(&n).unwrap();
        assert!(text.contains("restore-at-login --home \""));
        assert!(!text.contains("APROXY_HOME"));
        assert!(text.contains("aproxy.exe\""));
        (n, text)
    };
    #[cfg(unix)]
    let (file, original) = {
        let d = home.path().join(format!("login-test-{slot}"));
        let f = std::fs::read_dir(d)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let text = std::fs::read_to_string(&f).unwrap();
        assert!(text.contains("restore-at-login") && !text.contains("APROXY_HOME"));
        (f, text)
    };
    assert!(!b.read_autostart().await.unwrap().enabled);
    assert!(b.set_autostart(true).await.unwrap().enabled);
    #[cfg(windows)]
    key.set_value(&n, &"unrelated content").unwrap();
    #[cfg(unix)]
    std::fs::write(&file, "unrelated content").unwrap();
    assert_eq!(
        h.read_autostart().await.unwrap().issue,
        Some(RegistrationIssue::EntryMismatch)
    );
    assert!(matches!(
        h.set_autostart(false).await,
        Err(Fault::DesktopRegistration {
            reason: RegistrationReason::EntryMismatch
        })
    ));
    #[cfg(windows)]
    key.set_value(&n, &original).unwrap();
    #[cfg(unix)]
    std::fs::write(&file, &original).unwrap();
    let exe = aproxy::install::swap::bin_path_in(home.path());
    std::fs::remove_file(exe).unwrap();
    assert_eq!(
        h.read_autostart().await.unwrap().issue,
        Some(RegistrationIssue::BinaryMissing)
    );
    assert!(!h.set_autostart(false).await.unwrap().enabled);
    assert!(b.read_autostart().await.unwrap().enabled);
    assert!(!b.set_autostart(false).await.unwrap().enabled);
    #[cfg(windows)]
    assert_eq!(key.enum_values().count(), 0);
}
