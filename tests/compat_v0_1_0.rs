//! 0.1.0 兼容基线。0.1.0 已经发布：用户用 0.1.0 的 `aproxy install` 升级到之后的
//! 版本时，0.1.0 的代码会读新版本写下的注册表（`run/<端口>.pid`）、恢复记录
//! （`.restore`）、看护者 claim 与安装状态（`run/install.state`），新版本也要读
//! 0.1.0 留下的同一批文件，并经 IPC 与对方的守护对话（谁在哪一步运行，见
//! .agents/plan/ipc-v1.md 的 (c) 节）。
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

    /// src/daemon.rs `IpcResponse`（0.1.0 的 `ipc_proto_v1_default` 返回 1）
    #[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
    pub struct IpcResponse {
        pub ok: bool,
        #[serde(default)]
        pub info: Option<InstanceInfo>,
        #[serde(default = "proto_v1")]
        pub proto: u32,
    }

    fn proto_v1() -> u32 {
        1
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

    /// src/install/state.rs `InstallPhase`
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
    #[serde(rename_all = "lowercase")]
    pub enum InstallPhase {
        Marking,
        Downloading,
        Downloaded,
        Broadcasting,
        Acked,
        Swapping,
        Swapped,
        Relaying,
        Restarting,
        Verifying,
        Cleaning,
        Done,
        Failed,
        Aborted,
    }

    /// src/install/state.rs `InstallSource`
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
    #[serde(rename_all = "lowercase")]
    pub enum InstallSource {
        #[default]
        Github,
        From,
        Cargo,
        Npm,
        Binstall,
        Url,
    }

    /// src/install/state.rs `SkillPhase`
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "lowercase")]
    pub enum SkillPhase {
        Pending,
        Downloading,
        Done,
        Failed,
        Skipped,
    }

    /// src/install/state.rs `SkillState`
    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    pub struct SkillState {
        pub status: SkillPhase,
        #[serde(default)]
        pub attempt: u32,
        #[serde(default)]
        pub version: String,
    }

    /// src/install/state.rs `PendingRestore`
    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    pub struct PendingRestore {
        pub port: String,
        #[serde(default)]
        pub args: Vec<String>,
        #[serde(default)]
        pub log_path: String,
    }

    /// src/install/state.rs `InstallState`（`run/install.state` 内容）
    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    pub struct InstallState {
        pub phase: InstallPhase,
        #[serde(default)]
        pub target_version: String,
        #[serde(default)]
        pub source: InstallSource,
        #[serde(default)]
        pub staged_path: Option<String>,
        #[serde(default)]
        pub from_path: Option<String>,
        #[serde(default)]
        pub sha256: Option<String>,
        #[serde(default)]
        pub old_path: Option<String>,
        #[serde(default)]
        pub instance_snapshot: Vec<String>,
        #[serde(default)]
        pub skill: Option<SkillState>,
        #[serde(default)]
        pub started_at: u64,
        #[serde(default)]
        pub updated_at: u64,
        #[serde(default)]
        pub installer_pid: u32,
        #[serde(default)]
        pub pending_restores: Vec<PendingRestore>,
        #[serde(default)]
        pub halted: bool,
        #[serde(default)]
        pub last_error: Option<String>,
    }
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("读取 {} 失败: {e}", path.display()))
}

fn new_instance(port: &str) -> aproxy::daemon::InstanceRecord {
    aproxy::daemon::InstanceRecord {
        pid: 4321,
        process_start: 133_000_000_000_000_000,
        version: "9.9.9".into(),
        listen_addr: format!("127.0.0.1:{port}"),
        config_path: "C:/cfg/config.toml".into(),
        base_url: "https://api.example.com".into(),
        started_at: 1_760_000_000,
        log_path: "C:/home/logs/a.log".into(),
    }
}

#[test]
fn pid_record_written_now_parses_as_0_1_0() {
    let dir = tempfile::tempdir().unwrap();
    let info = new_instance("59601");
    aproxy::daemon::write_instance_file_in(dir.path(), &info).unwrap();
    let path = aproxy::daemon::instance_file_path_in(dir.path(), &info.listen_addr);
    let old: v0_1_0::InstanceInfo =
        serde_json::from_str(&read(&path)).expect("0.1.0 解析不了新版本的 .pid，会把它当损坏删掉");
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
        version: Some(env!("CARGO_PKG_VERSION").into()),
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
    assert_eq!(
        read_back.created_at_process,
        written_by_old.created_at_process
    );
    // 0.1.0 的 claim 没有版本：新守护据此判定它更旧、该退役（R1）
    assert_eq!(read_back.version, None);
}

#[test]
fn install_state_written_now_parses_as_0_1_0() {
    // 0.1.0 的安装器在 Windows 上接力给新版本后，靠读新版本写下的状态报告结局
    //（await_handover）；读不出来它就把损坏当「还在进行」，干等到超时。新版本
    // 可能在任何阶段写状态（续作接手 relaying 现场时，阶段仍是 relaying），
    // 所以每个阶段都要验
    use aproxy::install::state::{InstallPhase, InstallSource, InstallState, SkillPhase};
    let dir = tempfile::tempdir().unwrap();
    for &phase in InstallPhase::RUN_ORDER.iter().chain(&[
        InstallPhase::Done,
        InstallPhase::Failed,
        InstallPhase::Aborted,
    ]) {
        let mut state = InstallState::new_marking("9.9.9", InstallSource::From);
        state.phase = phase;
        state.staged_path = Some("C:/home/staging/9.9.9/aproxy.exe".into());
        state.from_path = Some("C:/download/aproxy.exe".into());
        state.sha256 = Some("ab".repeat(32));
        state.old_path = Some("C:/home/bin/aproxy.old.exe".into());
        state.instance_snapshot = vec!["12345".into(), "12346".into()];
        state.skill = Some(aproxy::install::state::SkillState {
            status: SkillPhase::Skipped,
            attempt: 0,
            version: "9.9.9".into(),
        });
        state.pending_restores = vec![aproxy::install::state::PendingRestore {
            port: "12346".into(),
            args: vec!["--config".into(), "C:/cfg/b.toml".into()],
            log_path: "C:/home/logs/b.log".into(),
        }];
        state.halted = phase == InstallPhase::Failed;
        state.last_error = Some("实例 12346 滚动重启失败".into());
        aproxy::install::state::write_in(dir.path(), &mut state).unwrap();

        let old: v0_1_0::InstallState =
            serde_json::from_str(&read(&aproxy::install::state::state_path_in(dir.path())))
                .unwrap_or_else(|e| panic!("0.1.0 读不出阶段 {phase:?} 的状态: {e}"));
        assert_eq!(
            serde_json::to_value(old.phase).unwrap(),
            serde_json::to_value(phase).unwrap()
        );
        assert_eq!(old.installer_pid, state.installer_pid);
        assert_eq!(old.halted, state.halted);
        assert_eq!(old.last_error, state.last_error);
        assert_eq!(old.instance_snapshot, state.instance_snapshot);
    }
}

#[test]
fn install_state_written_by_0_1_0_is_read() {
    // 新版本接手 0.1.0 留下的现场：Windows 上 0.1.0 写好 relaying 再拉起新二进制
    // 的 `install --continue`；0.1.0 中途死掉留下 restarting；实例级失败留下
    // failed + halted（交给 install --continue 判断能否收尾）
    use aproxy::install::state::InstallPhase;
    for (old_phase, phase) in [
        (v0_1_0::InstallPhase::Relaying, InstallPhase::Relaying),
        (v0_1_0::InstallPhase::Restarting, InstallPhase::Restarting),
        (v0_1_0::InstallPhase::Failed, InstallPhase::Failed),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let written_by_old = v0_1_0::InstallState {
            phase: old_phase,
            target_version: "9.9.9".into(),
            source: v0_1_0::InstallSource::Github,
            staged_path: Some("/home/u/.aproxy/staging/9.9.9/aproxy".into()),
            from_path: None,
            sha256: Some("cd".repeat(32)),
            old_path: Some("/home/u/.aproxy/bin/aproxy.old".into()),
            instance_snapshot: vec!["12345".into()],
            skill: Some(v0_1_0::SkillState {
                status: v0_1_0::SkillPhase::Downloading,
                attempt: 1,
                version: "9.9.9".into(),
            }),
            started_at: 1_760_000_000,
            updated_at: 1_760_000_100,
            installer_pid: 4242,
            pending_restores: vec![v0_1_0::PendingRestore {
                port: "12345".into(),
                args: vec!["--config".into(), "/cfg/a.toml".into()],
                log_path: "/home/u/.aproxy/logs/a.log".into(),
            }],
            halted: old_phase == v0_1_0::InstallPhase::Failed,
            last_error: Some("核验时找不到实例".into()),
        };
        std::fs::write(
            aproxy::install::state::state_path_in(dir.path()),
            serde_json::to_string_pretty(&written_by_old).unwrap(),
        )
        .unwrap();
        let st = aproxy::install::state::load_in(dir.path())
            .unwrap_or_else(|| panic!("应能读出 0.1.0 写下的 {old_phase:?} 现场"));
        assert_eq!(st.phase, phase);
        assert_eq!(st.installer_pid, 4242);
        assert_eq!(st.staged_path, written_by_old.staged_path);
        assert_eq!(st.old_path, written_by_old.old_path);
        assert_eq!(st.instance_snapshot, written_by_old.instance_snapshot);
        assert_eq!(st.pending_restores.len(), 1);
        assert_eq!(
            st.pending_restores[0].args,
            written_by_old.pending_restores[0].args
        );
        assert_eq!(st.halted, written_by_old.halted);
        assert_eq!(st.last_error, written_by_old.last_error);
    }
}

#[test]
fn ipc_requests_parse_as_0_1_0() {
    // 新版本的 CLI / 安装器发给 0.1.0 守护的请求行（带 v），0.1.0 必须认得出 op
    use aproxy::daemon::IpcOp;
    for (op, want) in [
        (IpcOp::Ping, v0_1_0::IpcRequest::Ping),
        (IpcOp::Shutdown, v0_1_0::IpcRequest::Shutdown),
        (IpcOp::PrepareSwap, v0_1_0::IpcRequest::PrepareSwap),
    ] {
        let line = op.request_line();
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

/// 0.1.0 的客户端连上新守护：发它那种不带 `v` 的请求行，读回一行应答。
async fn legacy_exchange(endpoint: &str, request: &str) -> String {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    #[cfg(windows)]
    let stream = tokio::net::windows::named_pipe::ClientOptions::new()
        .open(endpoint)
        .unwrap();
    #[cfg(unix)]
    let stream = tokio::net::UnixStream::connect(endpoint).await.unwrap();
    let (reader, mut writer) = tokio::io::split(stream);
    writer
        .write_all(format!("{request}\n").as_bytes())
        .await
        .unwrap();
    let mut line = String::new();
    BufReader::new(reader).read_line(&mut line).await.unwrap();
    line
}

#[test]
fn daemon_answers_0_1_0_clients_in_the_0_1_0_shape() {
    // 0.1.0 的 CLI 与安装器（升级途中它们在驱动）发的是不带 v 的请求，读应答用的
    // 是 0.1.0 的结构：新守护必须回它们解析得了的形状，prepare_swap 的 ACK 也要
    // 落在 swap_phase 上
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join("settings.json"), r#"{"watchdog": false}"#).unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let cfg = home.path().join("c.toml");
    std::fs::write(
        &cfg,
        format!("base_url = \"https://api.example.com\"\nlisten_addr = \"127.0.0.1:{port}\"\n"),
    )
    .unwrap();
    let exe = env!("CARGO_BIN_EXE_aproxy");
    let started = std::process::Command::new(exe)
        .args(["start", "--config"])
        .arg(&cfg)
        .env("APROXY_HOME", home.path())
        .output()
        .unwrap();
    assert!(
        started.status.success(),
        "{}",
        String::from_utf8_lossy(&started.stderr)
    );
    struct Stop<'a>(&'a Path, u16);
    impl Drop for Stop<'_> {
        fn drop(&mut self) {
            let _ = std::process::Command::new(env!("CARGO_BIN_EXE_aproxy"))
                .args(["stop", &self.1.to_string()])
                .env("APROXY_HOME", self.0)
                .output();
        }
    }
    let _stop = Stop(home.path(), port);

    let endpoint = aproxy::daemon::endpoint_for_in(&home.path().join("run"), &port.to_string());
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let ask = |request: &str| -> v0_1_0::IpcResponse {
        let line = rt.block_on(legacy_exchange(&endpoint, request));
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("0.1.0 解析不了 {line}: {e}"))
    };

    let ping = ask(r#"{"op":"ping"}"#);
    assert!(ping.ok);
    assert_eq!(ping.proto, 2);
    let info = ping.info.expect("0.1.0 的 ping 应答带实例信息");
    assert_eq!(info.listen_addr, format!("127.0.0.1:{port}"));
    assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
    assert!(!info.swap_phase);

    let swap = ask(r#"{"op":"prepare_swap"}"#);
    assert!(swap.ok && swap.info.is_some_and(|i| i.swap_phase));

    let unknown = ask(r#"{"op":"what"}"#);
    assert!(!unknown.ok);

    // 0.1.0 的安装器靠 shutdown 停掉新实例
    let stop = ask(r#"{"op":"shutdown"}"#);
    assert!(stop.ok);
}
