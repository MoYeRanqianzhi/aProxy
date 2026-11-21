//! 桌面宿主的测试只观察自己的临时主目录，不启动 Tauri，也不依赖当前用户配置。
mod common;
use aproxy::{desktop::DesktopHome, spec};
use aproxy_ipc::enumerate::{Enumerated, Liveness};
use common::{TestHome, free_port};
use serde_json::json;

#[test]
fn discovery_secrets_and_desire_tokens_are_bound_to_the_home_and_window() {
    let home = TestHome::new();
    let host = DesktopHome::new(home.path().into()).unwrap();
    let port = free_port();
    let file = home.path().join("test.toml");
    std::fs::write(&file,"base_url='https://api.invalid'\nproxy='http://user:pa%22ssword-long@proxy.invalid/?q=query-credential'\napi_key='file-credential'\n[extra_headers]\nx-safe-name='header-credential'\n").unwrap();
    std::fs::write(
        home.path().join("settings.json"),
        serde_json::to_vec(&json!({"defaults":{"base_url":"https://other.invalid", "api_key":"withheld-credential"}})).unwrap(),
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
        r#"private-"\credential"#,
    ] {
        assert!(!f.secrets.replace(secret).contains(secret));
    }
    let token = f.restore.unwrap();
    assert_eq!(Some(token.clone()), observe(&host).restore);
    let other = DesktopHome::new(home.path().into()).unwrap();
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
