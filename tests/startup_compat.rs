//! 当前读者分别检查新旧写者的证据。运行 python tests/startup_compat.py 可同时编译并验证基线读者。
mod common;
use aproxy::{
    config::Overrides,
    spawn::{READY_TIMEOUT, SpawnError, spawn_from_spec},
    spec,
};
use common::ReportExit as _;
use common::TestHome;
use std::path::PathBuf;

#[tokio::test(flavor = "current_thread")]
async fn launch_slot_and_fallback_are_compatible_across_writers() {
    let current = PathBuf::from(common::APROXY);
    let legacy = std::env::var_os("APROXY_STARTUP_LEGACY_WRITER").map(PathBuf::from);
    for (writer, structured) in
        std::iter::once((current, true)).chain(legacy.map(|exe| (exe, false)))
    {
        let home = TestHome::new();
        let missing = home.path().join("missing-config.toml");
        let run = home.path().join("run");
        let spec = spec::capture(
            Some(&missing),
            Overrides::default(),
            &home.path().join("instance.log"),
        );
        // 先保留一次真实交接文件，确认失败来源：不能因为某一端完全没写结果而误判兼容。
        let path = spec::write_spec_file(&run, &spec).unwrap();
        let out = home
            .cmd_with(&writer)
            .env("APROXY_LANG", "zh-CN")
            .arg("__serve")
            .arg("--home")
            .arg(home.path())
            .arg("--spec")
            .arg(&path)
            .output()
            .report_exit()
            .unwrap();
        assert_eq!(out.status.code(), Some(1));
        let payload: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let phrase = if structured {
            assert_eq!(payload["code"], "target_not_found");
            assert_eq!(payload["render"]["id"], "missing");
            assert!(payload.get("schema").is_none() && payload.get("fault").is_none());
            "Configuration file not found:"
        } else {
            assert_eq!(payload["schema"], spec.schema);
            assert!(payload.get("render").is_none());
            "配置文件不存在:"
        };
        let err = spawn_from_spec(home.path(), &writer, &spec, READY_TIMEOUT)
            .await
            .unwrap_err();
        let SpawnError::Exited {
            format: None,
            startup_log,
            ..
        } = &err
        else {
            panic!("{err:?}")
        };
        let SpawnError::Exited { report, .. } = &err else {
            unreachable!()
        };
        assert_eq!(report.is_some(), structured);
        let zh = err.excerpt(aproxy_ipc::i18n::Lang::Zh);
        assert!(zh.contains("配置文件不存在:"));
        if structured {
            assert!(!zh.contains(phrase));
        } else {
            assert!(zh.contains(&startup_log.lines[0]));
        }
        assert_eq!(startup_log.lines.len(), 1, "{err:?}");
        assert!(
            startup_log.lines[0].contains(phrase)
                && startup_log.lines[0].ends_with("missing-config.toml")
        );
    }
}
