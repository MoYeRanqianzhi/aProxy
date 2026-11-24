//! 本进程 home 的清点入口；显式指定 home 的读取不依赖进程全局设置。
pub use aproxy_ipc::enumerate::*;
/// 清点本进程的 home。
pub async fn enumerate() -> Enumeration {
    enumerate_in(&aproxy_ipc::home::home()).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        daemon::{self, InstanceRecord},
        spec,
        watchdog::{self, WatchdogRecord},
    };
    use std::path::Path;
    fn record(port: u16, pid: u32, start: &str) -> InstanceRecord {
        InstanceRecord {
            pid,
            start: start.to_string(),
            instance_id: "19ac3f2e8b5d1a2b".into(),
            version: "0.2.0".into(),
            listen_addr: format!("127.0.0.1:{port}"),
            config_path: None,
            base_url: "https://api.example.com".into(),
            started_at: 1_760_190_000_000,
            log_path: None,
            foreground: false,
        }
    }

    fn spec_with_config(config: Option<&Path>) -> spec::InstanceSpec {
        spec::InstanceSpec {
            schema: spec::SCHEMA,
            config_path: config.map(|path| path.display().to_string()),
            overrides: Default::default(),
            pinned_port: None,
            env: Default::default(),
            log_path: "unused.log".into(),
        }
    }

    /// `run/` 与 `ledger/` 里的全部文件名，用来核对清点前后盘上没有变化。
    fn files_in(home: &Path) -> Vec<String> {
        let mut names: Vec<String> = ["run", "ledger"]
            .iter()
            .flat_map(|dir| {
                std::fs::read_dir(home.join(dir))
                    .into_iter()
                    .flatten()
                    .flatten()
                    .map(move |entry| format!("{dir}/{}", entry.file_name().to_string_lossy()))
            })
            .collect();
        names.sort();
        names
    }

    fn ledger_entry(
        pid: u32,
        start: &str,
        role: &str,
        version: &str,
    ) -> aproxy_ipc::ledger::LedgerEntry {
        aproxy_ipc::ledger::LedgerEntry {
            ledger: aproxy_ipc::ledger::LEDGER_FORMAT,
            pid,
            start: start.to_string(),
            role: role.to_string(),
            version: version.to_string(),
            port: None,
            label: None,
            exe: None,
            config: None,
        }
    }

    #[tokio::test]
    async fn noncanonical_restore_name_is_not_a_restore_obligation() {
        let home = tempfile::tempdir().unwrap();
        let run = home.path().join("run");
        std::fs::create_dir(&run).unwrap();
        spec::write_restore_in(&run, "00080", &spec_with_config(None)).unwrap();
        std::fs::write(run.join("00081.restore.json"), "invalid").unwrap();
        let scan = enumerate_in(home.path()).await;
        assert!(scan.instances.is_empty() && scan.gc_paused.is_none());
    }

    /// 没有任何实例应答的 home：挂死的、等恢复的、死进程留下的、读不懂的各占一个端口，
    /// 清点把它们分清，并且不删任何文件。
    #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
    #[tokio::test]
    async fn silent_ports_are_told_apart_and_nothing_is_deleted() {
        let home = tempfile::tempdir().unwrap();
        let run = home.path().join("run");
        std::fs::create_dir_all(&run).unwrap();

        // 20101：记录里的进程还在（一个与 aproxy 无关的子进程顶替），控制通道没有人听
        let mut child = crate::process::tests::spawn_unrelated_child();
        let child_start =
            aproxy_ipc::ledger::start_marker(child.id()).expect("子进程的启动标记可查");
        daemon::write_instance_file_in(&run, &record(20101, child.id(), &child_start)).unwrap();
        spec::write_restore_in(&run, "20101", &spec_with_config(None)).unwrap();
        // 20102、20103：只有期望状态记录；前一个的配置文件不在
        let missing = home.path().join("gone.toml");
        spec::write_restore_in(&run, "20102", &spec_with_config(Some(&missing))).unwrap();
        let present = home.path().join("here.toml");
        std::fs::write(&present, "").unwrap();
        spec::write_restore_in(&run, "20103", &spec_with_config(Some(&present))).unwrap();
        // 20104：进程已经不在的实例记录，没有期望状态记录
        daemon::write_instance_file_in(&run, &record(20104, u32::MAX - 4242, &child_start))
            .unwrap();
        // 20105、20106：读不懂的记录（更新的版本写的；损坏的）
        std::fs::write(
            run.join("20105.instance.json"),
            r#"{"schema":2,"shape":"new"}"#,
        )
        .unwrap();
        std::fs::write(run.join("20106.restore.json"), "{not json").unwrap();
        // 名字不是端口的文件不归清点管
        std::fs::write(run.join("notes.instance.json"), "x").unwrap();

        // 台账：20101 的实例（已列为实例，不重复列）、一个没有实例记录的活进程（更旧的版本
        // 启动的实例就是这样）、一条进程已经不在的条目
        let own = std::process::id();
        let own_start = aproxy_ipc::ledger::start_marker(own).unwrap();
        for entry in [
            ledger_entry(child.id(), &child_start, "instance", "0.2.0"),
            ledger_entry(own, &own_start, "instance", "0.2.0-alpha.1"),
            ledger_entry(u32::MAX - 4243, &child_start, "desktop", "0.2.0"),
        ] {
            aproxy_ipc::ledger::write_entry_in(home.path(), &entry).unwrap();
        }

        let before = files_in(home.path());
        let found = enumerate_in(home.path()).await;
        assert_eq!(files_in(home.path()), before, "清点不删、不建任何文件");

        let summary: Vec<(u16, Liveness, bool, Option<&str>)> = found
            .instances
            .iter()
            .map(|item| {
                (
                    item.port,
                    item.liveness,
                    item.restore_pending,
                    item.restore_reason.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                (20101, Liveness::Unresponsive, false, None),
                (
                    20102,
                    Liveness::Down,
                    true,
                    Some(RESTORE_REASON_CONFIG_MISSING)
                ),
                (20103, Liveness::Down, true, None),
            ]
        );
        assert_eq!(
            found.instances[0].instance.as_ref().unwrap().pid,
            child.id()
        );
        assert!(found.instances[1].instance.is_none());

        let unreadable: Vec<(&str, &str)> = found
            .unreadable
            .iter()
            .map(|item| (item.file.as_str(), item.reason.code.as_str()))
            .collect();
        assert_eq!(
            unreadable,
            [
                ("run/20105.instance.json", CODE_RECORD_SCHEMA_TOO_NEW),
                ("run/20106.restore.json", CODE_RECORD_UNREADABLE),
            ]
        );
        assert_eq!(found.unreadable[0].reason.details["schema"], 2);
        assert_eq!(
            found.gc_paused,
            Some(GcPaused {
                reason: GC_PAUSED_UNREADABLE_RECORDS.into()
            })
        );
        assert_eq!(found.watchdog, None);
        assert_eq!(
            found.processes,
            [ProcessView {
                pid: own,
                start: own_start,
                role: "instance".into(),
                version: "0.2.0-alpha.1".into(),
            }]
        );

        child.kill().unwrap();
        child.wait().unwrap();
    }

    /// 看护者：属主在、扫描新鲜是 running；属主在、扫描停了是 stale；属主不在等于没有；
    /// 读不懂的记录列进 `unreadable`，但它不影响日志清理。
    #[tokio::test]
    async fn watchdog_states_follow_the_record() {
        let home = tempfile::tempdir().unwrap();
        let run = home.path().join("run");
        std::fs::create_dir_all(&run).unwrap();
        let path = watchdog::record_path_in(&run);
        let write = |record: &WatchdogRecord| {
            std::fs::write(&path, serde_json::to_string(record).unwrap()).unwrap();
        };

        let mut record = WatchdogRecord::for_current_process(30);
        write(&record);
        let found = enumerate_in(home.path()).await;
        let view = found.watchdog.expect("本进程的记录，属主在");
        assert_eq!(view.state, WATCHDOG_RUNNING);
        assert_eq!(
            (view.pid, view.last_scan_at),
            (record.pid, record.last_scan_at)
        );
        assert!(found.unreadable.is_empty() && found.gc_paused.is_none());

        record.last_scan_at = 1;
        write(&record);
        let found = enumerate_in(home.path()).await;
        assert_eq!(found.watchdog.unwrap().state, WATCHDOG_STALE);

        record.pid = u32::MAX - 4242;
        write(&record);
        assert_eq!(enumerate_in(home.path()).await.watchdog, None);

        std::fs::write(&path, r#"{"schema":2}"#).unwrap();
        let found = enumerate_in(home.path()).await;
        assert_eq!(found.watchdog, None);
        assert_eq!(found.unreadable.len(), 1);
        assert_eq!(found.unreadable[0].file, "run/watchdog.json");
        assert_eq!(found.unreadable[0].reason.code, CODE_RECORD_SCHEMA_TOO_NEW);
        assert_eq!(found.gc_paused, None, "看护者记录读不懂不让日志清理暂停");
        assert!(path.exists());
    }
}
