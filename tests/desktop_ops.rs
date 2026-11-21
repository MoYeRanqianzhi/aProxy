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
    use aproxy_ipc::desktop::{ChoiceKind, Fault, InstanceOps};
    let home = TestHome::new();
    home.settings(r#"{"watchdog":{"enabled":false},"aliases":{"primary":"unused"}}"#);
    let file = home.path().join("config.toml");
    std::fs::write(
        &file,
        "base_url='http://127.0.0.1:9'\nlisten_addr='127.0.0.1:0'\napi_key='test-private-key'\n",
    )
    .unwrap();
    let host = DesktopHome::new(home.path().into(), env!("CARGO_BIN_EXE_aproxy").into()).unwrap();
    let ops: &dyn InstanceOps = &host;
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
