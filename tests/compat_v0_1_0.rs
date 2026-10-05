//! 0.1.0 兼容基线。0.1.0 已经发布：用户用 0.1.0 的 `aproxy install` 升级到之后的
//! 版本时，0.1.0 的代码会读新版本写下的注册表（`run/<端口>.pid`）、恢复记录
//! （`.restore`）与看护者 claim，新版本也要读 0.1.0 留下的同一批文件，并经 IPC 与
//! 对方的守护对话（谁在哪一步运行，见 .agents/plan/ipc-v1.md 的 (c) 节）。
//!
//! 这里冻结 0.1.0 的 serde 结构——逐字段照抄 v0.1.0 tag 的源码，**不要随新版本
//! 修改**——并断言两个方向都能解析：
//! - 新版本写出的文件与请求，0.1.0 的结构能解析；
//! - 0.1.0 写出的文件，新版本的读取函数能读出。
//!
//! 改了这些格式而让本文件失败，就是破坏了 0.1.0 → 新版本的原地升级。0.1.0 的读取
//! 一侧还会**删掉**解析不了的 `.pid` / `.restore`，所以这里的失败不是展示瑕疵，而是
//! 升级途中实例记录被清掉。

use std::path::Path;

/// v0.1.0 的结构原样冻结（字段、类型、serde 属性与 tag `v0.1.0` 一致）。
mod v0_1_0 {
    use serde::{Deserialize, Serialize};

    /// src/daemon.rs `InstanceInfo`：`.pid` 文件内容，也是 IPC ping 应答的载荷
    #[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
    pub struct InstanceInfo {
        pub pid: u32,
        pub version: String,
        pub listen_addr: String,
        pub config_path: String,
        pub base_url: String,
        pub started_at: u64,
        #[serde(default)]
        pub last_activity_secs: u64,
        #[serde(default)]
        pub proto_version: u32,
        #[serde(default)]
        pub requests_total: u64,
        #[serde(default)]
        pub retries_total: u64,
        #[serde(default)]
        pub last_error: Option<String>,
        #[serde(default)]
        pub last_error_at: u64,
        #[serde(default)]
        pub swap_phase: bool,
        #[serde(default)]
        pub log_path: String,
        #[serde(default)]
        pub process_start: u64,
    }

    /// src/daemon.rs `IpcRequest`
    #[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
    #[serde(tag = "op", rename_all = "snake_case")]
    pub enum IpcRequest {
        Ping,
        Shutdown,
        Stats,
        PrepareSwap,
    }

    /// src/daemon.rs `RestoreRecord`（`.restore` 文件内容）
    #[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
    pub struct RestoreRecord {
        #[serde(default)]
        pub args: Vec<String>,
        #[serde(default)]
        pub log_path: String,
    }

    /// src/watchdog.rs `WatchdogClaim`（`run/watchdog.claim` 内容）
    #[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
    pub struct WatchdogClaim {
        pub pid: u32,
        pub created_at_process: u64,
        pub heartbeat_secs: u64,
    }
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("读取 {} 失败: {e}", path.display()))
}

fn new_instance(port: &str) -> aproxy::daemon::InstanceInfo {
    aproxy::daemon::InstanceInfo {
        pid: 4321,
        version: "9.9.9".into(),
        listen_addr: format!("127.0.0.1:{port}"),
        config_path: "C:/cfg/config.toml".into(),
        base_url: "https://api.example.com".into(),
        started_at: 1_760_000_000,
        last_activity_secs: 1_760_000_100,
        proto_version: aproxy::daemon::IPC_PROTO_VERSION,
        requests_total: 7,
        retries_total: 2,
        last_error: None,
        last_error_at: 0,
        swap_phase: false,
        log_path: "C:/home/logs/a.log".into(),
        process_start: 133_000_000_000_000_000,
    }
}

#[test]
fn pid_record_written_now_parses_as_0_1_0() {
    let dir = tempfile::tempdir().unwrap();
    let info = new_instance("59601");
    aproxy::daemon::write_instance_file_in(dir.path(), &info).unwrap();
    let path = aproxy::daemon::instance_file_path_in(dir.path(), &info.listen_addr);
    let old: v0_1_0::InstanceInfo = serde_json::from_str(&read(&path))
        .expect("0.1.0 解析不了新版本的 .pid，会把它当损坏删掉");
    assert_eq!(old.pid, info.pid);
    assert_eq!(old.version, info.version);
    assert_eq!(old.listen_addr, info.listen_addr);
    assert_eq!(old.config_path, info.config_path);
    assert_eq!(old.process_start, info.process_start);
    assert_eq!(old.log_path, info.log_path);
}

#[test]
fn pid_record_written_by_0_1_0_is_read() {
    let dir = tempfile::tempdir().unwrap();
    let old = v0_1_0::InstanceInfo {
        pid: 1234,
        version: "0.1.0".into(),
        listen_addr: "127.0.0.1:59602".into(),
        config_path: "C:/cfg/config.toml".into(),
        base_url: "https://api.example.com".into(),
        started_at: 1_760_000_000,
        last_activity_secs: 1_760_000_000,
        proto_version: 2,
        requests_total: 0,
        retries_total: 0,
        last_error: None,
        last_error_at: 0,
        swap_phase: false,
        log_path: "C:/home/logs/b.log".into(),
        process_start: 133_000_000_000_000_001,
    };
    std::fs::write(
        dir.path().join("59602.pid"),
        serde_json::to_string_pretty(&old).unwrap(),
    )
    .unwrap();
    let info = aproxy::daemon::read_instance_file_in(dir.path(), "59602")
        .expect("新版本应能读出 0.1.0 写的 .pid");
    assert_eq!(info.pid, old.pid);
    assert_eq!(info.process_start, old.process_start);
    assert_eq!(info.log_path, old.log_path);
}

#[test]
fn restore_record_round_trips_with_0_1_0() {
    let dir = tempfile::tempdir().unwrap();
    // 新版本写 → 0.1.0 读：字段名一旦改动，0.1.0 的 serde default 会读成空 args，
    // 0.1.0 的看门狗随即用默认配置把实例拉起来
    let args = vec!["--config".to_string(), "C:/cfg/a.toml".to_string()];
    aproxy::daemon::write_restore_file_in(dir.path(), "127.0.0.1:59603", &args, "C:/l/a.log")
        .unwrap();
    let old: v0_1_0::RestoreRecord =
        serde_json::from_str(&read(&dir.path().join("59603.restore"))).unwrap();
    assert_eq!(old.args, args);
    assert_eq!(old.log_path, "C:/l/a.log");

    // 0.1.0 写 → 新版本读
    let written_by_old = v0_1_0::RestoreRecord {
        args: vec!["--config".into(), "C:/cfg/b.toml".into()],
        log_path: "C:/l/b.log".into(),
    };
    std::fs::write(
        dir.path().join("59604.restore"),
        serde_json::to_string(&written_by_old).unwrap(),
    )
    .unwrap();
    let entries = aproxy::daemon::list_restore_entries_in(dir.path());
    let entry = entries
        .iter()
        .find(|e| e.port == "59604")
        .expect("新版本应能读出 0.1.0 写的 .restore（读不出会被当损坏删掉）");
    assert_eq!(entry.args, written_by_old.args);
    assert_eq!(entry.log_path, written_by_old.log_path);
}

#[test]
fn watchdog_claim_round_trips_with_0_1_0() {
    // 0.1.0 的看护者要能解析新版本的 claim 才会让位；解析失败它会覆盖 claim，
    // 两个看护者同时在任
    let dir = tempfile::tempdir().unwrap();
    let claim = aproxy::watchdog::WatchdogClaim {
        pid: 777,
        created_at_process: 133_000_000_000_000_002,
        heartbeat_secs: 1_760_000_200,
    };
    assert!(aproxy::watchdog::acquire_claim_in(dir.path(), &claim).is_some());
    let old: v0_1_0::WatchdogClaim =
        serde_json::from_str(&read(&aproxy::watchdog::claim_path_in(dir.path()))).unwrap();
    assert_eq!(old.pid, claim.pid);
    assert_eq!(old.created_at_process, claim.created_at_process);

    let other = tempfile::tempdir().unwrap();
    let written_by_old = v0_1_0::WatchdogClaim {
        pid: 778,
        created_at_process: 133_000_000_000_000_003,
        heartbeat_secs: 1_760_000_300,
    };
    std::fs::write(
        aproxy::watchdog::claim_path_in(other.path()),
        serde_json::to_string(&written_by_old).unwrap(),
    )
    .unwrap();
    let read_back = aproxy::watchdog::read_claim_in(other.path()).expect("应能读出 0.1.0 的 claim");
    assert_eq!(read_back.pid, written_by_old.pid);
    assert_eq!(read_back.created_at_process, written_by_old.created_at_process);
}

#[test]
fn ipc_requests_parse_as_0_1_0() {
    // 新版本的 CLI / 安装器发给 0.1.0 守护的请求行，0.1.0 必须认得出 op
    for (req, want) in [
        (aproxy::daemon::IpcRequest::Ping, v0_1_0::IpcRequest::Ping),
        (aproxy::daemon::IpcRequest::Shutdown, v0_1_0::IpcRequest::Shutdown),
        (
            aproxy::daemon::IpcRequest::PrepareSwap,
            v0_1_0::IpcRequest::PrepareSwap,
        ),
    ] {
        let line = serde_json::to_string(&req).unwrap();
        let old: v0_1_0::IpcRequest =
            serde_json::from_str(&line).unwrap_or_else(|e| panic!("0.1.0 解析不了 {line}: {e}"));
        assert_eq!(old, want, "{line}");
    }
    // 带版本号与参数的请求行同样认得出（internally tagged 的单元变体忽略多余键）——
    // 新协议在请求里加字段不会让 0.1.0 守护拒收
    let old: v0_1_0::IpcRequest =
        serde_json::from_str(r#"{"v":1,"op":"shutdown","args":{}}"#).unwrap();
    assert_eq!(old, v0_1_0::IpcRequest::Shutdown);
}
