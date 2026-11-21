//! Enumerate an explicit home without changing it: reconcile durable records,
//! verified IPC probes, watchdog freshness and the process ledger. The serialized
//! status document is additive and independent of any consumer's presentation.
//!
//! Only `run/`, `ledger/` and their local control endpoints are consulted. Unreadable
//! records are reported, never cleaned up; unknown liveness, watchdog and reason
//! strings are not evidence that a process is dead.
use crate::{
    client::{IpcError, ipc_probe_in},
    registry::{self, *},
    wire::{Activity, Behavior, InstanceRecord, InstanceState, InstanceStatus, error_code},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// 记录读不懂时用的码：内容不是这种记录应有的形状，或读不出来。
pub const CODE_RECORD_UNREADABLE: &str = "record_unreadable";
/// 记录读不懂时用的码：`schema` 比本版本认识的高（`details.schema` 是记录里的值）。
pub const CODE_RECORD_SCHEMA_TOO_NEW: &str = "record_schema_too_new";
/// 实例应答了却无法采信时用的码（`details.port`、`details.reason`）。
pub const CODE_INSTANCE_INCOMPATIBLE: &str = "instance_incompatible";

/// `restore_reason` 的取值：期望状态记录里的配置文件已经不在。
pub const RESTORE_REASON_CONFIG_MISSING: &str = "config_missing";
/// `gc_paused.reason` 的取值：`run/` 里有读不懂的实例记录或期望状态记录。
pub const GC_PAUSED_UNREADABLE_RECORDS: &str = "unreadable_records";
/// 看护者的 `state`：在任，最近一次扫描还新鲜。
pub const WATCHDOG_RUNNING: &str = "running";
/// 看护者的 `state`：进程还在，但最近一次扫描早于三个扫描周期（多半挂死）。
pub const WATCHDOG_STALE: &str = "stale";

/// 一次清点的结果。
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Enumeration {
    /// 按端口升序。一个端口至多一条
    pub instances: Vec<Enumerated>,
    /// `run/` 里读不懂的记录。它们原样留在盘上
    pub unreadable: Vec<UnreadableFile>,
    /// 日志清理此刻是否暂停；没暂停时不出现
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gc_paused: Option<GcPaused>,
    /// 在任的看护者；没有时为 null
    pub watchdog: Option<WatchdogView>,
    /// 台账里登记着、仍在运行、却不属于上面任何实例或看护者的进程：界面进程、安装者，
    /// 以及本版本读不懂其记录的实例（更旧或更新的版本启动的）
    pub processes: Vec<ProcessView>,
}

/// 一个端口上的实例。
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Enumerated {
    pub port: u16,
    pub liveness: Liveness,
    /// 实例记录：应答了的取应答里的（实时），不应答的取盘上的；没有可用的记录时为 null
    pub instance: Option<InstanceRecord>,
    /// 实时状态，只有 [`Liveness::Live`] 才有
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<LiveStatus>,
    /// 为什么无法采信它的应答，只有 [`Liveness::Incompatible`] 才有
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<Reason>,
    /// 这个端口有一份读得懂的期望状态记录、却没有实例在运行：`restore` 与看护者会拉起它
    #[serde(default)]
    pub restore_pending: bool,
    /// `restore_pending` 为真、并且看得出它为什么还没被拉起时才出现。取值见
    /// [`RESTORE_REASON_CONFIG_MISSING`]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restore_reason: Option<String>,
}

/// 一个端口上的实例是死是活。
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Liveness {
    /// 控制通道应答了，应答可以采信
    Live,
    /// 记录里的进程还在，控制通道却不应答（多半挂死）。只有这一种该建议 `stop --force`
    Unresponsive,
    /// 控制通道应答了，但本版本无法采信：协议版本不同，或应答者的身份对不上。用与实例
    /// 同版本的 aproxy 操作它
    Incompatible,
    /// 没有实例在运行，只留下一份期望状态记录
    Down,
    /// 更新的版本报出、本版本不认识的取值：按「不在正常服务」对待
    #[serde(other)]
    Unknown,
}

/// 应答了的实例的实时状态：IPC 状态快照去掉 `instance`（它在 [`Enumerated::instance`]）。
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct LiveStatus {
    pub state: InstanceState,
    pub behavior: Behavior,
    pub activity: Activity,
    pub ops: Vec<String>,
    #[serde(default)]
    pub features: Vec<String>,
}

/// 一个码与它带的数据（键与类型见码表 [`crate::code`]）。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Reason {
    pub code: String,
    pub details: serde_json::Value,
}

/// 一份读不懂的记录。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct UnreadableFile {
    /// 相对 home 的路径，分隔符一律是 `/`，例如 `run/12349.instance.json`
    pub file: String,
    /// [`CODE_RECORD_UNREADABLE`] 或 [`CODE_RECORD_SCHEMA_TOO_NEW`]
    pub reason: Reason,
}

/// 日志清理暂停的原因。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct GcPaused {
    /// 取值见 [`GC_PAUSED_UNREADABLE_RECORDS`]
    pub reason: String,
}

/// 在任的看护者。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct WatchdogView {
    pub pid: u32,
    pub start: String,
    pub version: String,
    /// [`WATCHDOG_RUNNING`] 或 [`WATCHDOG_STALE`]；不认识的取值按「状态不明」对待
    pub state: String,
    /// 最近一次扫描结束的时刻（Unix 毫秒）
    pub last_scan_at: u64,
    /// 最近的重拉，旧的在前
    pub restarts: Vec<RestartEvent>,
}

/// 台账里的一个进程。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ProcessView {
    pub pid: u32,
    pub start: String,
    /// 台账里的角色（`instance`、`watchdog`、`installer`、`desktop`，词表只增）
    pub role: String,
    pub version: String,
}

/// 清点 `home`。所有端口同时探测，单个端口至多等两秒多一点（见
/// [`ipc_probe_in`]）。
pub async fn enumerate_in(home: &Path) -> Enumeration {
    let run_dir = home.join("run");
    let (instances, mut unreadable) = membership_in(&run_dir).await;
    // 读不懂的实例记录或期望状态记录让日志清理暂停（清理靠它们知道哪些日志还有人用）
    let gc_paused = (!unreadable.is_empty()).then(|| GcPaused {
        reason: GC_PAUSED_UNREADABLE_RECORDS.to_string(),
    });

    let watchdog = read_watchdog(&run_dir, &mut unreadable);

    // 台账里其余的活进程：去掉上面已经列出的实例与看护者
    let listed: Vec<(u32, &str)> = instances
        .iter()
        .filter_map(|item| item.instance.as_ref())
        .map(|record| (record.pid, record.start.as_str()))
        .chain(watchdog.iter().map(|w| (w.pid, w.start.as_str())))
        .collect();
    let mut processes: Vec<ProcessView> = crate::ledger::read_all_in(home)
        .into_iter()
        .map(|found| found.entry)
        .filter(|entry| !listed.contains(&(entry.pid, entry.start.as_str())))
        .filter(crate::ledger::is_alive)
        .map(|entry| ProcessView {
            pid: entry.pid,
            start: entry.start,
            role: entry.role,
            version: entry.version,
        })
        .collect();
    processes.sort_by_key(|process| process.pid);

    Enumeration {
        instances,
        unreadable,
        gc_paused,
        watchdog,
        processes,
    }
}

// Blocking jobs own only paths and record data, never a connection or view. A
// cancelled scan releases its receiver; at most the current file read finishes.
async fn discovery_io<T: Send + 'static>(
    work: impl FnOnce(&tokio::sync::oneshot::Sender<T>) -> T + Send + 'static,
) -> T {
    let (send, recv) = tokio::sync::oneshot::channel();
    tokio::task::spawn_blocking(move || {
        if !send.is_closed() {
            let result = work(&send);
            let _ = send.send(result);
        }
    });
    recv.await.expect("discovery worker panicked")
}

pub(crate) async fn membership_in(run_dir: &Path) -> (Vec<Enumerated>, Vec<UnreadableFile>) {
    let dir = run_dir.to_owned();
    let (ports, unreadable) = discovery_io(move |job| {
        let run_dir = dir;
        let mut unreadable = Vec::new();

        // 每个端口在盘上有什么：实例记录、期望状态记录，各自读得懂与否
        let mut ports: BTreeMap<u16, OnDisk> = BTreeMap::new();
        let mut names: Vec<String> = std::fs::read_dir(&run_dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        for name in &names {
            if job.is_closed() {
                break;
            }
            let path = run_dir.join(name);
            if let Some(port) = port_in_name(name, registry::INSTANCE_FILE_SUFFIX) {
                // 读不懂的记录仍然占着这个端口：照样去问一次，上面也许有实例
                let on_disk = ports.entry(port).or_default();
                match registry::read_instance_record(&path) {
                    Ok(record) => on_disk.instance = Some(record),
                    Err(reason) => report_unreadable(&mut unreadable, &path, name, reason),
                }
            } else if let Some(port) = port_in_name(name, registry::RESTORE_SUFFIX) {
                if *name != format!("{port}{}", registry::RESTORE_SUFFIX) {
                    continue;
                }
                match registry::read_optional_record::<RestoreRecord>(&path, SCHEMA) {
                    Ok(Some(entry)) => {
                        ports.entry(port).or_default().restore = Some(entry.spec.config_file())
                    }
                    Ok(None) => {}
                    Err(reason) => {
                        // 读不懂的记录仍然占着这个端口：照样去问一次，上面也许有实例
                        ports.entry(port).or_default();
                        report_unreadable(&mut unreadable, &path, name, reason);
                    }
                }
            }
        }
        (ports, unreadable)
    })
    .await;
    let probes = ports.into_iter().map(|(port, on_disk)| async move {
        let answer = ipc_probe_in(run_dir, &port.to_string()).await;
        discovery_io(move |_| classify(port, on_disk, answer)).await
    });
    let instances: Vec<Enumerated> = futures_util::future::join_all(probes)
        .await
        .into_iter()
        .flatten()
        .collect();

    (instances, unreadable)
}

#[test]
fn filesystem_discovery_does_not_execute_on_the_runtime_thread() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let worker = rt.block_on(discovery_io(|_| std::thread::current().id()));
    assert_ne!(std::thread::current().id(), worker);
}

/// 一个端口在 `run/` 里留下的东西。
#[derive(Default)]
struct OnDisk {
    /// 读得懂的实例记录
    instance: Option<InstanceRecord>,
    /// 读得懂的期望状态记录
    restore: Option<Option<std::path::PathBuf>>,
}

/// 文件名是 `<端口><suffix>` 时取出端口；别的名字（包括端口不是数字的）不归清点管。
fn port_in_name(name: &str, suffix: &str) -> Option<u16> {
    name.strip_suffix(suffix)?.parse().ok()
}

/// 把一份读不懂的记录记进 `unreadable`。清点与删除之间没有锁：记录恰在这一刻被删掉
/// （stop、实例退出）不算读不懂。
fn report_unreadable(out: &mut Vec<UnreadableFile>, path: &Path, name: &str, reason: Unreadable) {
    let reason = match reason {
        Unreadable::Io(_) if !path.exists() => return,
        Unreadable::Io(_) | Unreadable::Parse(_) => Reason {
            code: CODE_RECORD_UNREADABLE.to_string(),
            details: serde_json::json!({}),
        },
        Unreadable::SchemaTooNew(schema) => Reason {
            code: CODE_RECORD_SCHEMA_TOO_NEW.to_string(),
            details: serde_json::json!({ "schema": schema }),
        },
    };
    out.push(UnreadableFile {
        file: format!("run/{name}"),
        reason,
    });
}

/// 把一个端口的探测结果与盘上的记录合成一条。None = 这个端口上什么都没有（只剩一份
/// 已死进程的实例记录，或只有读不懂的记录），不列出。
fn classify(
    port: u16,
    on_disk: OnDisk,
    answer: Result<InstanceStatus, IpcError>,
) -> Option<Enumerated> {
    let incompatible = |instance: Option<InstanceRecord>, reason: Reason| Enumerated {
        port,
        liveness: Liveness::Incompatible,
        instance,
        status: None,
        reason: Some(reason),
        restore_pending: false,
        restore_reason: None,
    };
    match answer {
        // 应答者的 pid 已经与连接对端核对过（见 daemon 的请求函数），它就是此刻占着这个
        // 端口的控制通道的实例：记录以它自报的为准，盘上的那份可能是上一个实例留下的
        Ok(status) => Some(Enumerated {
            port,
            liveness: Liveness::Live,
            instance: Some(status.instance),
            status: Some(LiveStatus {
                state: status.state,
                behavior: status.behavior,
                activity: status.activity,
                ops: status.ops,
                features: status.features,
            }),
            reason: None,
            restore_pending: false,
            restore_reason: None,
        }),
        // 实例明说它不支持本版本的协议：把它支持哪些版本原样带上
        Err(IpcError::Remote { code, details, .. }) if code == error_code::UNSUPPORTED_VERSION => {
            Some(incompatible(on_disk.instance, Reason { code, details }))
        }
        // 探测是最基本的请求，被拒绝只能是协议对不上
        Err(IpcError::Remote { code, .. }) => Some(incompatible(
            on_disk.instance,
            Reason {
                code: CODE_INSTANCE_INCOMPATIBLE.to_string(),
                details: serde_json::json!({ "port": port, "reason": format!("ping rejected: {code}") }),
            },
        )),
        Err(IpcError::Protocol(_) | IpcError::RunChanged | IpcError::LineTooLong) => {
            Some(incompatible(
                on_disk.instance,
                Reason {
                    code: CODE_INSTANCE_INCOMPATIBLE.to_string(),
                    details: serde_json::json!({ "port": port, "reason": "the reply could not be trusted" }),
                },
            ))
        }
        Err(IpcError::Unreachable(_) | IpcError::Transient(_)) => {
            // 不应答不等于已死：记录里的进程经「pid + 启动标记」核验还在，就是挂死
            if let Some(record) = on_disk.instance.filter(|record| {
                record_identity(record.pid, &record.start) == RecordIdentity::Alive
            }) {
                return Some(Enumerated {
                    port,
                    liveness: Liveness::Unresponsive,
                    instance: Some(record),
                    status: None,
                    reason: None,
                    restore_pending: false,
                    restore_reason: None,
                });
            }
            // 没有实例在运行。留着期望状态记录的才列出来：它在等 restore 或看护者
            let restore = on_disk.restore?;
            let config_missing = restore.is_some_and(|config| !config.exists());
            Some(Enumerated {
                port,
                liveness: Liveness::Down,
                instance: None,
                status: None,
                reason: None,
                restore_pending: true,
                restore_reason: config_missing.then(|| RESTORE_REASON_CONFIG_MISSING.to_string()),
            })
        }
    }
}

/// 读看护者记录。属主进程已经不在的记录是上一个看护者留下的，等同于没有看护者；读不懂的
/// 记录（损坏，或更新的版本写的）记进 `unreadable`，这里同样给不出看护者是谁。
fn read_watchdog(run_dir: &Path, unreadable: &mut Vec<UnreadableFile>) -> Option<WatchdogView> {
    let path = registry::record_path_in(run_dir);
    if !path.exists() {
        return None;
    }
    let record: WatchdogRecord = match registry::read_record(&path, registry::RECORD_SCHEMA) {
        Ok(record) => record,
        Err(reason) => {
            report_unreadable(unreadable, &path, "watchdog.json", reason);
            return None;
        }
    };
    if !record.owner_alive() {
        return None;
    }
    let state = if record.is_fresh(registry::now_millis()) {
        WATCHDOG_RUNNING
    } else {
        WATCHDOG_STALE
    };
    Some(WatchdogView {
        pid: record.pid,
        start: record.start,
        version: record.version,
        state: state.to_string(),
        last_scan_at: record.last_scan_at,
        restarts: record.restarts,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{ActivityBuckets, KeepaliveBehavior};

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

    fn live_status() -> LiveStatus {
        LiveStatus {
            state: InstanceState::Serving,
            behavior: Behavior {
                delivery: "retry".into(),
                disk_cache: true,
                keepalive: KeepaliveBehavior {
                    trigger: "any".into(),
                    interval_secs: 15,
                },
                retry_rules: 1,
                history: crate::wire::HistoryBehavior { body_budget_mb: 64 },
            },
            activity: Activity {
                last_request_at: 1_760_190_000_000,
                requests_total: 3,
                retries_total: 1,
                rescued_total: 1,
                upgraded_total: 0,
                in_flight: 0,
                retrying: 0,
                by_code: Default::default(),
                buckets: ActivityBuckets::default(),
                last_error: None,
            },
            ops: vec!["ping".into()],
            features: vec![],
        }
    }

    fn plain(port: u16, liveness: Liveness, instance: Option<InstanceRecord>) -> Enumerated {
        Enumerated {
            port,
            liveness,
            instance,
            status: None,
            reason: None,
            restore_pending: false,
            restore_reason: None,
        }
    }

    /// 清点结果的 JSON 形状。`status --json` 与界面都读它：这里的样本只能加字段，改动
    /// 已有的键、类型或取值就是破坏契约。
    #[test]
    fn enumeration_shape_is_pinned() {
        let enumeration = Enumeration {
            instances: vec![
                Enumerated {
                    status: Some(live_status()),
                    ..plain(12346, Liveness::Live, Some(record(12346, 4321, "w:1")))
                },
                plain(
                    12347,
                    Liveness::Unresponsive,
                    Some(record(12347, 4400, "w:2")),
                ),
                Enumerated {
                    reason: Some(Reason {
                        code: "unsupported_version".into(),
                        details: serde_json::json!({ "supported": [2] }),
                    }),
                    ..plain(12348, Liveness::Incompatible, None)
                },
                Enumerated {
                    restore_pending: true,
                    restore_reason: Some(RESTORE_REASON_CONFIG_MISSING.into()),
                    ..plain(12349, Liveness::Down, None)
                },
            ],
            unreadable: vec![UnreadableFile {
                file: "run/12350.instance.json".into(),
                reason: Reason {
                    code: CODE_RECORD_SCHEMA_TOO_NEW.into(),
                    details: serde_json::json!({ "schema": 2 }),
                },
            }],
            gc_paused: Some(GcPaused {
                reason: GC_PAUSED_UNREADABLE_RECORDS.into(),
            }),
            watchdog: Some(WatchdogView {
                pid: 5555,
                start: "w:3".into(),
                version: "0.2.0".into(),
                state: WATCHDOG_RUNNING.into(),
                last_scan_at: 1_760_190_123_456,
                restarts: vec![RestartEvent {
                    port: 12346,
                    at: 1_760_190_100_000,
                    kind: registry::KIND_CRASH.into(),
                    ok: true,
                }],
            }),
            processes: vec![ProcessView {
                pid: 7777,
                start: "w:4".into(),
                role: "desktop".into(),
                version: "0.2.0".into(),
            }],
        };
        let json = serde_json::to_string(&enumeration).unwrap();
        assert_eq!(
            json,
            concat!(
                r#"{"instances":["#,
                r#"{"port":12346,"liveness":"live","instance":{"pid":4321,"start":"w:1","instance_id":"19ac3f2e8b5d1a2b","version":"0.2.0","listen_addr":"127.0.0.1:12346","config_path":null,"base_url":"https://api.example.com","started_at":1760190000000,"log_path":null,"foreground":false},"#,
                r#""status":{"state":"serving","behavior":{"delivery":"retry","disk_cache":true,"keepalive":{"trigger":"any","interval_secs":15},"retry_rules":1,"history":{"body_budget_mb":64}},"#,
                r#""activity":{"last_request_at":1760190000000,"requests_total":3,"retries_total":1,"rescued_total":1,"upgraded_total":0,"in_flight":0,"retrying":0,"by_code":{},"buckets":{"width_secs":0,"requests":[],"retries":[],"failures":[]},"last_error":null},"#,
                r#""ops":["ping"],"features":[]},"restore_pending":false},"#,
                r#"{"port":12347,"liveness":"unresponsive","instance":{"pid":4400,"start":"w:2","instance_id":"19ac3f2e8b5d1a2b","version":"0.2.0","listen_addr":"127.0.0.1:12347","config_path":null,"base_url":"https://api.example.com","started_at":1760190000000,"log_path":null,"foreground":false},"restore_pending":false},"#,
                r#"{"port":12348,"liveness":"incompatible","instance":null,"reason":{"code":"unsupported_version","details":{"supported":[2]}},"restore_pending":false},"#,
                r#"{"port":12349,"liveness":"down","instance":null,"restore_pending":true,"restore_reason":"config_missing"}],"#,
                r#""unreadable":[{"file":"run/12350.instance.json","reason":{"code":"record_schema_too_new","details":{"schema":2}}}],"#,
                r#""gc_paused":{"reason":"unreadable_records"},"#,
                r#""watchdog":{"pid":5555,"start":"w:3","version":"0.2.0","state":"running","last_scan_at":1760190123456,"restarts":[{"port":12346,"at":1760190100000,"kind":"crash","ok":true}]},"#,
                r#""processes":[{"pid":7777,"start":"w:4","role":"desktop","version":"0.2.0"}]}"#
            )
        );

        // 没有暂停、没有看护者时：`gc_paused` 不出现，`watchdog` 是 null
        let empty = Enumeration {
            instances: vec![],
            unreadable: vec![],
            gc_paused: None,
            watchdog: None,
            processes: vec![],
        };
        assert_eq!(
            serde_json::to_string(&empty).unwrap(),
            r#"{"instances":[],"unreadable":[],"watchdog":null,"processes":[]}"#
        );

        // 读者这一侧：更新的版本多给的字段忽略，不认识的 liveness 读作 Unknown
        let newer: Enumeration = serde_json::from_str(
            r#"{"instances":[{"port":1,"liveness":"draining","instance":null,"later":1}],"unreadable":[],"watchdog":null,"processes":[],"later":{}}"#,
        )
        .unwrap();
        assert_eq!(newer.instances[0].liveness, Liveness::Unknown);
        assert!(!newer.instances[0].restore_pending);
    }

    /// 应答决定是哪一种：采信的是 live；实例明说版本不对、或应答无法采信的是
    /// incompatible；不应答时看记录里的进程还在不在。
    #[test]
    fn the_answer_decides_between_incompatible_and_the_rest() {
        let on_disk = |restore: bool| OnDisk {
            // 这个 pid 上没有进程
            instance: Some(record(20100, u32::MAX - 4242, "w:1")),
            restore: restore.then_some(None),
        };

        let item = classify(
            20100,
            on_disk(true),
            Err(IpcError::Remote {
                code: "unsupported_version".into(),
                message: "x".into(),
                details: serde_json::json!({ "supported": [2] }),
            }),
        )
        .unwrap();
        assert_eq!(item.liveness, Liveness::Incompatible);
        let reason = item.reason.unwrap();
        assert_eq!(reason.code, "unsupported_version");
        assert_eq!(reason.details["supported"][0], 2);
        // 有实例在应答，就没有什么在等恢复；盘上的记录照样带出来，好让人知道是谁
        assert!(!item.restore_pending);
        assert_eq!(item.instance.unwrap().listen_addr, "127.0.0.1:20100");

        for answer in [
            IpcError::Protocol("garbled".into()),
            IpcError::Remote {
                code: "bad_request".into(),
                message: "x".into(),
                details: serde_json::Value::Null,
            },
        ] {
            let item = classify(20100, on_disk(false), Err(answer)).unwrap();
            assert_eq!(item.liveness, Liveness::Incompatible);
            let reason = item.reason.unwrap();
            assert_eq!(reason.code, CODE_INSTANCE_INCOMPATIBLE);
            // 码表给这个码定的两个键
            assert_eq!(reason.details["port"], 20100);
            assert!(reason.details["reason"].is_string());
        }

        // 不应答、记录里的进程也不在：留着期望状态记录的算 down，否则这个端口上什么都没有
        for silence in [
            IpcError::Unreachable("x".into()),
            IpcError::Transient("x".into()),
        ] {
            let item = classify(20100, on_disk(true), Err(silence)).unwrap();
            assert_eq!(item.liveness, Liveness::Down);
            assert!(item.restore_pending && item.instance.is_none());
            assert_eq!(item.restore_reason, None);
        }
        assert!(
            classify(
                20100,
                on_disk(false),
                Err(IpcError::Unreachable("x".into()))
            )
            .is_none()
        );
    }
}
