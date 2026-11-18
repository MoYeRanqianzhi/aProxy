//! 0.2 的公开契约，从外面看：对一个真的实例走控制通道、读它写在盘上的记录，逐项与这里
//! 的样本比对。
//!
//! 这些形状是「只增不改」的：以后的版本可以加字段、加 op、加参数、加码、加取值，已有的
//! 不删、不改名、不改类型、不改含义——别的版本的 aproxy（原地升级时新旧并存）、界面、
//! agent 与第三方客户端都按它们读写。所以这里的比对分两种：
//!
//! - **错误应答**逐字比对（`message` 除外，它是给日志看的英文，不是契约）；
//! - **快照与记录**用 [`assert_covers`]：样本里的每个键都必须在、类型必须相同，多出来的
//!   键不算错。加字段不必改样本；删掉或改掉样本里已有的东西会让测试失败，那就是破坏契约，
//!   **不要改样本来迁就代码**。
//!
//! 本文件用不到实例内部的东西，只看线上与盘上的字节。逐字钉住每个结构序列化结果的样本
//! 在各模块自己的单测里（它们要用模块私有的函数），清单见 [`PINNED_IN_MODULES`]，最后一个
//! 测试守着它们不被悄悄删掉或改名。
//!
//! 隔离：每个测试一个 `common::TestHome`，端口向系统要，结束时按端口 stop。

mod common;
use common::{TestHome, free_port};

use serde_json::{Value, json};
use std::path::Path;
use std::time::{Duration, Instant};

/// 各模块里逐字钉住形状的单测：(源文件, 测试函数名)。
const PINNED_IN_MODULES: &[(&str, &str)] = &[
    // 实例记录：盘上的 `run/<端口>.instance.json` 与快照里的 `instance`
    ("src/daemon.rs", "instance_record_shape_is_pinned"),
    // 每个 op 的请求行（含各个参数）
    (
        "src/daemon.rs",
        "request_lines_are_pinned_and_round_trip_through_the_server_parser",
    ),
    // 每种拒绝的应答行
    ("src/daemon.rs", "rejections_are_pinned"),
    // 成功应答与状态快照
    ("src/daemon.rs", "snapshot_shape_is_pinned"),
    // 客户端怎样读别的版本的应答
    ("src/daemon.rs", "client_reads_v1_errors_and_unknown_states"),
    // 拉起规格与期望状态记录
    ("src/spec.rs", "spec_serializes_to_the_documented_shape"),
    // 看护者记录 `run/watchdog.json`
    ("src/watchdog.rs", "record_shape_is_pinned"),
    // 只读清点的结果
    ("src/ops/enumerate.rs", "enumeration_shape_is_pinned"),
    // HTTP 错误体与 SSE 终态事件
    ("src/proxy/errors.rs", "payload_has_the_documented_shape"),
    // 控制端点的名字
    (
        "aproxy-ipc/src/endpoint.rs",
        "names_have_the_documented_shape",
    ),
    // 码表导出的 JSON
    ("aproxy-ipc/src/code.rs", "exported_json_shape"),
];

/// 断言 `actual` 覆盖 `sample`：
/// - 对象：样本里的每个键都在，值各自覆盖；多出来的键不管；
/// - 数组：样本给了元素时，实际的每个元素都要覆盖样本的第一个元素；样本是空数组时只要求
///   实际也是数组；
/// - 其余：JSON 类型相同（null、布尔、数、字符串），值不比。
fn assert_covers(actual: &Value, sample: &Value, at: &str) {
    match (actual, sample) {
        (Value::Object(actual), Value::Object(sample)) => {
            for (key, expected) in sample {
                let found = actual
                    .get(key)
                    .unwrap_or_else(|| panic!("{at}.{key} 不见了：已有的键不能删、不能改名"));
                assert_covers(found, expected, &format!("{at}.{key}"));
            }
        }
        (Value::Array(actual), Value::Array(sample)) => {
            if let Some(expected) = sample.first() {
                for (index, item) in actual.iter().enumerate() {
                    assert_covers(item, expected, &format!("{at}[{index}]"));
                }
            }
        }
        (actual, sample) => assert_eq!(
            std::mem::discriminant(actual),
            std::mem::discriminant(sample),
            "{at} 的类型变了：实际是 {actual}，样本是 {sample}"
        ),
    }
}

/// 快照的样本：`{"v":1,"ok":true,"result":…}` 里的 `result`。有配置文件、有日志文件的后台
/// 实例；没有失败过。
fn snapshot_sample() -> Value {
    json!({
        "instance": {
            "pid": 4321,
            "start": "w:133712345678901234",
            "instance_id": "19ac3f2e8b5d1a2b",
            "version": "0.2.0",
            "listen_addr": "127.0.0.1:12346",
            "config_path": "/home/u/.aproxy/config.toml",
            "base_url": "https://api.example.com",
            "started_at": 1_760_190_000_123_u64,
            "log_path": "/home/u/.aproxy/logs/19ac3f2e8b5d-1a2b.log",
            "foreground": false
        },
        "state": "serving",
        "behavior": {
            "delivery": "retry",
            "disk_cache": true,
            "keepalive": {"trigger": "any", "interval_secs": 15},
            "retry_rules": 1
        },
        "activity": {
            "last_request_at": 1_760_190_000_123_u64,
            "requests_total": 0,
            "retries_total": 0,
            "rescued_total": 0,
            "in_flight": 0,
            "retrying": 0,
            "by_code": {},
            "buckets": {"width_secs": 120, "requests": [0], "retries": [0], "failures": [0]},
            "last_error": null
        },
        "ops": ["ping"],
        "features": ["shutdown.keep_restore"]
    })
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

/// 向控制端点原样写出 `request`（调用方自己带换行），读回一行应答。不经 aproxy 的客户端
/// 代码：别的程序连上来看到的就是这些字节。
async fn exchange(endpoint: &str, request: &str) -> String {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    async fn over<S>(stream: S, request: &str) -> String
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    {
        let (reader, mut writer) = tokio::io::split(stream);
        writer.write_all(request.as_bytes()).await.unwrap();
        let mut line = String::new();
        BufReader::new(reader).read_line(&mut line).await.unwrap();
        line
    }
    #[cfg(windows)]
    {
        use tokio::net::windows::named_pipe::ClientOptions;
        // 231 = ERROR_PIPE_BUSY：服务端正在两个连接之间重建监听实例，稍后再连
        let deadline = Instant::now() + Duration::from_secs(5);
        let client = loop {
            match ClientOptions::new().open(endpoint) {
                Ok(client) => break client,
                Err(e) if e.raw_os_error() == Some(231) && Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(e) => panic!("连不上控制端点 {endpoint}：{e}"),
            }
        };
        over(client, request).await
    }
    #[cfg(unix)]
    {
        let client = tokio::net::UnixStream::connect(endpoint)
            .await
            .unwrap_or_else(|e| panic!("连不上控制端点 {endpoint}：{e}"));
        over(client, request).await
    }
}

/// 一个在隔离 home 里启动好的实例，与向它发原始请求的办法。
struct Instance<'a> {
    home: &'a TestHome,
    port: u16,
    rt: tokio::runtime::Runtime,
}

impl Instance<'_> {
    /// 发一行请求，应答按 JSON 解析。应答必须是恰好一行。
    fn ask(&self, request: &str) -> Value {
        let endpoint =
            aproxy::daemon::endpoint_for_in(&self.home.path().join("run"), &self.port.to_string());
        let line = self.rt.block_on(exchange(&endpoint, request));
        assert!(line.ends_with('\n'), "应答应以换行结束：{line:?}");
        assert_eq!(line.matches('\n').count(), 1, "应答应是一行：{line:?}");
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("应答不是 JSON（{e}）：{line}"))
    }

    /// 成功应答里的快照；应答不成功就失败。
    fn snapshot(&self, request: &str) -> Value {
        let reply = self.ask(request);
        assert_eq!(reply["v"], 1, "{reply}");
        assert_eq!(reply["ok"], true, "{reply}");
        assert!(reply.get("error").is_none(), "成功应答不带 error：{reply}");
        reply["result"].clone()
    }

    /// 这个端口的控制端点是否已经没有人应答。
    fn gone(&self) -> bool {
        self.rt
            .block_on(aproxy::daemon::ipc_ping_in(
                &self.home.path().join("run"),
                &self.port.to_string(),
            ))
            .is_err()
    }
}

/// 在 `home` 里以 `args` 启动一个后台实例，等它就绪（`aproxy start` 返回即就绪）。
fn start<'a>(home: &'a TestHome, port: u16, args: &[&str]) -> Instance<'a> {
    home.stop_on_drop(port);
    let out = home
        .cmd()
        .args(args)
        .env("NO_COLOR", "1")
        .output()
        .expect("运行 aproxy 失败");
    assert!(
        out.status.success(),
        "start 失败：{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    Instance {
        home,
        port,
        rt: runtime(),
    }
}

fn wait_for(timeout: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if cond() {
            return true;
        }
        if Instant::now() > deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// 读 `run/` 里的一份记录：必须是恰好一行 JSON。
fn record_on_disk(home: &Path, name: &str) -> Value {
    let path = home.join("run").join(name);
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("读不到 {}：{e}", path.display()));
    assert_eq!(
        text.trim_end_matches('\n').matches('\n').count(),
        0,
        "{name} 应是一行 JSON：{text:?}"
    );
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{name} 不是 JSON（{e}）：{text}"))
}

/// 控制协议 v1：每个 op 的请求与成功应答、每种拒绝，走真实例的控制端点。
#[test]
fn control_channel_speaks_v1_as_sampled() {
    let home = TestHome::new();
    let port = free_port();
    let cfg = home.path().join("contract.toml");
    // 上游地址带 userinfo：快照与记录里只许出现打过码的
    std::fs::write(
        &cfg,
        format!(
            "base_url = \"https://alice:WIREPASS123@contract-test.example.com/v1\"\nlisten_addr = \"127.0.0.1:{port}\"\n"
        ),
    )
    .unwrap();
    let instance = start(&home, port, &["start", "--config", cfg.to_str().unwrap()]);

    // ---- ping ----
    let snapshot = instance.snapshot("{\"v\":1,\"op\":\"ping\"}\n");
    assert_covers(&snapshot, &snapshot_sample(), "result");
    assert_eq!(snapshot["state"], "serving");
    assert_eq!(
        snapshot["instance"]["listen_addr"],
        format!("127.0.0.1:{port}")
    );
    assert_eq!(snapshot["instance"]["foreground"], false);
    assert_eq!(snapshot["activity"]["requests_total"], 0);
    assert_eq!(snapshot["activity"]["buckets"]["width_secs"], 120);
    for series in ["requests", "retries", "failures"] {
        assert_eq!(
            snapshot["activity"]["buckets"][series]
                .as_array()
                .unwrap()
                .len(),
            30,
            "{series}"
        );
    }
    // 调用方靠这两张表决定用不用某个 op、带不带某个参数：已有的名字不能少
    for op in ["ping", "shutdown", "prepare_swap"] {
        assert!(
            snapshot["ops"].as_array().unwrap().contains(&json!(op)),
            "{op}"
        );
    }
    for feature in [
        "shutdown.keep_restore",
        "shutdown.if_idle_secs",
        "prepare_swap.ttl_secs",
    ] {
        assert!(
            snapshot["features"]
                .as_array()
                .unwrap()
                .contains(&json!(feature)),
            "{feature}"
        );
    }
    // 密钥不出线：userinfo 已打码
    let wire = snapshot.to_string();
    assert!(
        !wire.contains("WIREPASS123") && !wire.contains("alice"),
        "{wire}"
    );
    assert!(
        snapshot["instance"]["base_url"]
            .as_str()
            .unwrap()
            .contains("contract-test.example.com"),
        "{wire}"
    );

    // 盘上的实例记录 = `schema` 加上快照里的 `instance`
    let mut on_disk = record_on_disk(home.path(), &format!("{port}.instance.json"));
    assert_eq!(on_disk["schema"], 1);
    on_disk.as_object_mut().unwrap().shift_remove("schema");
    assert_eq!(on_disk, snapshot["instance"]);

    // 请求里多出来的顶层字段被忽略（以后的客户端可能带上）；`args` 给空对象等于没给
    let tolerant =
        instance.snapshot("{\"v\":1,\"op\":\"ping\",\"args\":{},\"trace\":{\"id\":7}}\n");
    assert_eq!(tolerant["instance"], snapshot["instance"]);

    // ---- 拒绝：除 message 外逐字 ----
    let rejections = [
        (
            "{\"v\":1,\"op\":\"what\"}\n",
            json!({"code": "unknown_op", "details": {"op": "what"}}),
        ),
        (
            "{\"v\":2,\"op\":\"ping\"}\n",
            json!({"code": "unsupported_version", "details": {"supported": [1]}}),
        ),
        (
            "{\"v\":1,\"op\":\"shutdown\",\"args\":{\"drain_secs\":5,\"keep_restore\":true,\"later\":1}}\n",
            json!({"code": "unsupported_arg", "details": {"args": ["drain_secs", "later"]}}),
        ),
        (
            "{\"v\":1,\"op\":\"ping\",\"args\":{\"x\":1}}\n",
            json!({"code": "unsupported_arg", "details": {"args": ["x"]}}),
        ),
        (
            "{\"v\":1,\"op\":\"shutdown\",\"args\":{\"keep_restore\":\"yes\"}}\n",
            json!({"code": "bad_request", "details": {"reason": "invalid_arg", "arg": "keep_restore"}}),
        ),
        (
            "{\"v\":1,\"op\":\"prepare_swap\",\"args\":{\"ttl_secs\":0}}\n",
            json!({"code": "bad_request", "details": {"reason": "invalid_arg", "arg": "ttl_secs"}}),
        ),
        (
            "not json\n",
            json!({"code": "bad_request", "details": {"reason": "not_json"}}),
        ),
        (
            // 0.1.0 的请求没有 v
            "{\"op\":\"ping\"}\n",
            json!({"code": "bad_request", "details": {"reason": "missing_v"}}),
        ),
        (
            "{\"v\":1}\n",
            json!({"code": "bad_request", "details": {"reason": "missing_op"}}),
        ),
        (
            "{\"v\":1,\"op\":\"ping\",\"args\":3}\n",
            json!({"code": "bad_request", "details": {"reason": "args_not_object"}}),
        ),
    ];
    for (request, expected) in &rejections {
        let mut reply = instance.ask(request);
        assert!(
            reply["error"]["message"]
                .as_str()
                .is_some_and(|m| m.is_ascii() && !m.is_empty()),
            "message 是一句英文：{reply}"
        );
        reply["error"]
            .as_object_mut()
            .unwrap()
            .shift_remove("message");
        assert_eq!(
            reply,
            json!({"v": 1, "ok": false, "error": expected}),
            "{request}"
        );
        // 码与 details 的键都登记在码表里
        let code = aproxy_ipc::code::lookup(expected["code"].as_str().unwrap())
            .unwrap_or_else(|| panic!("码表里没有 {}", expected["code"]));
        for key in expected["details"].as_object().unwrap().keys() {
            assert!(
                code.details.iter().any(|detail| detail.key == key),
                "{} 的 details 里有码表没登记的键 {key}",
                code.name
            );
        }
    }
    // 被拒的请求没有任何效果：实例照常服务，期望状态记录还在
    assert_eq!(
        instance.snapshot("{\"v\":1,\"op\":\"ping\"}\n")["state"],
        "serving"
    );
    let restore = home.path().join("run").join(format!("{port}.restore.json"));
    assert!(restore.exists());

    // ---- 带条件的 shutdown：不够闲就不停，并说出实际闲置了多久 ----
    let mut reply =
        instance.ask("{\"v\":1,\"op\":\"shutdown\",\"args\":{\"if_idle_secs\":3600}}\n");
    assert!(reply["error"]["details"]["idle_ms"].is_u64(), "{reply}");
    let error = reply["error"].as_object_mut().unwrap();
    error.shift_remove("message");
    error.insert("details".into(), json!({"idle_ms": 0}));
    assert_eq!(
        reply,
        json!({"v": 1, "ok": false, "error": {"code": "condition_not_met", "details": {"idle_ms": 0}}})
    );
    assert!(restore.exists(), "没停成的 shutdown 不删期望状态记录");

    // ---- prepare_swap：回同一份快照，状态变成 swap_prepared，之后的 ping 也看得到 ----
    let swapped =
        instance.snapshot("{\"v\":1,\"op\":\"prepare_swap\",\"args\":{\"ttl_secs\":30}}\n");
    assert_covers(&swapped, &snapshot_sample(), "result");
    assert_eq!(swapped["state"], "swap_prepared");
    assert_eq!(
        instance.snapshot("{\"v\":1,\"op\":\"ping\"}\n")["state"],
        "swap_prepared"
    );

    // ---- shutdown 带 keep_restore：实例退出，期望状态记录留着 ----
    let stopping =
        instance.snapshot("{\"v\":1,\"op\":\"shutdown\",\"args\":{\"keep_restore\":true}}\n");
    assert_covers(&stopping, &snapshot_sample(), "result");
    assert_eq!(stopping["state"], "stopping");
    assert!(
        wait_for(Duration::from_secs(15), || instance.gone()),
        "收到 shutdown 的实例应退出"
    );
    assert!(
        restore.exists(),
        "keep_restore 的 shutdown 不删期望状态记录"
    );
    assert!(
        wait_for(Duration::from_secs(5), || {
            !home
                .path()
                .join("run")
                .join(format!("{port}.instance.json"))
                .exists()
        }),
        "正常退出的实例删掉自己的实例记录"
    );

    // 清点里它成了「没在运行、等着恢复」的一条
    let listing = instance
        .rt
        .block_on(aproxy::ops::enumerate::enumerate_in(home.path()));
    let listing = serde_json::to_value(&listing).unwrap();
    assert_eq!(
        listing["instances"],
        json!([{"port": port, "liveness": "down", "instance": null, "restore_pending": true}])
    );
}

/// 没有配置文件的实例：快照与实例记录的 `config_path` 是 null。不带参数的 shutdown 让
/// 实例退出并删掉期望状态记录。
#[test]
fn instance_without_a_config_file_reports_null_config_path() {
    let home = TestHome::new();
    home.settings(
        r#"{"watchdog": {"enabled": false},
            "defaults": {"base_url": "https://defaults-only.example.com"}}"#,
    );
    let port = free_port();
    let instance = start(&home, port, &["--listen", &format!("127.0.0.1:{port}")]);

    let snapshot = instance.snapshot("{\"v\":1,\"op\":\"ping\"}\n");
    assert!(snapshot["instance"]["config_path"].is_null(), "{snapshot}");
    assert!(snapshot["instance"]["log_path"].is_string(), "{snapshot}");
    // 除了那个 null，其余与有配置文件的实例同形
    let mut sample = snapshot_sample();
    sample["instance"]["config_path"] = Value::Null;
    assert_covers(&snapshot, &sample, "result");
    assert!(record_on_disk(home.path(), &format!("{port}.instance.json"))["config_path"].is_null());

    let restore = home.path().join("run").join(format!("{port}.restore.json"));
    assert!(restore.exists());
    let stopping = instance.snapshot("{\"v\":1,\"op\":\"shutdown\"}\n");
    assert_eq!(stopping["state"], "stopping");
    assert!(
        wait_for(Duration::from_secs(15), || instance.gone()),
        "收到 shutdown 的实例应退出"
    );
    assert!(
        !restore.exists(),
        "不带 keep_restore 的 shutdown 是「用户要它停下」：记录删掉"
    );
}

/// 盘上的记录与只读清点：实例记录、期望状态记录、看护者记录，以及清点结果的 JSON。
#[test]
fn records_on_disk_and_the_enumeration_match_the_samples() {
    let home = TestHome::new();
    home.enable_watchdog();
    let port = free_port();
    let cfg = home.path().join("records.toml");
    std::fs::write(
        &cfg,
        format!(
            "base_url = \"https://contract-test.example.com\"\nlisten_addr = \"127.0.0.1:{port}\"\n"
        ),
    )
    .unwrap();
    let instance = start(&home, port, &["start", "--config", cfg.to_str().unwrap()]);

    // 实例记录
    let record = record_on_disk(home.path(), &format!("{port}.instance.json"));
    let mut sample = snapshot_sample()["instance"].clone();
    sample
        .as_object_mut()
        .unwrap()
        .insert("schema".into(), json!(1));
    assert_covers(&record, &sample, "instance.json");
    assert_eq!(record["schema"], 1);

    // 期望状态记录：拉起规格外面包一层
    let restore = record_on_disk(home.path(), &format!("{port}.restore.json"));
    assert_covers(
        &restore,
        &json!({
            "schema": 1,
            "spec": {
                "schema": 1,
                "config_path": "/home/u/.aproxy/config.toml",
                "overrides": {},
                "pinned_port": null,
                "env": {},
                "log_path": "/home/u/.aproxy/logs/19ac3f2e8b5d-1a2b.log"
            },
            "written_at": 1_760_190_000_123_u64
        }),
        "restore.json",
    );
    assert_eq!(restore["schema"], 1);
    assert_eq!(
        restore["spec"]["log_path"], record["log_path"],
        "实例写日志的地方就是规格里记的那个"
    );
    // 环境快照：每个记下的变量要么是字符串，要么是 null（启动者没设）
    let env = restore["spec"]["env"].as_object().unwrap();
    assert!(
        env.contains_key("PATH") && env.contains_key("HTTPS_PROXY"),
        "{env:?}"
    );
    assert!(
        env.values()
            .all(|value| value.is_string() || value.is_null())
    );

    // 看护者记录：start 拉起了看护者，它接管之后写下记录
    assert!(
        wait_for(Duration::from_secs(20), || {
            home.path().join("run").join("watchdog.json").exists()
        }),
        "开着看护者开关时 start 应拉起看护者"
    );
    let watchdog = record_on_disk(home.path(), "watchdog.json");
    assert_covers(
        &watchdog,
        &json!({
            "schema": 1,
            "pid": 5555,
            "start": "w:133712345000000000",
            "version": "0.2.0",
            "heartbeat_secs": 30,
            "created_at": 1_760_190_000_000_u64,
            "last_scan_at": 1_760_190_123_456_u64,
            "restarts": []
        }),
        "watchdog.json",
    );
    assert_eq!(watchdog["schema"], 1);

    // 只读清点
    let listing = instance
        .rt
        .block_on(aproxy::ops::enumerate::enumerate_in(home.path()));
    let listing = serde_json::to_value(&listing).unwrap();
    let mut live = snapshot_sample();
    let live = live.as_object_mut().unwrap();
    let instance_sample = live.shift_remove("instance").unwrap();
    assert_covers(
        &listing,
        &json!({
            "instances": [{
                "port": 12346,
                "liveness": "live",
                "instance": instance_sample,
                "status": Value::Object(live.clone()),
                "restore_pending": false
            }],
            "unreadable": [],
            "watchdog": {
                "pid": 5555,
                "start": "w:133712345000000000",
                "version": "0.2.0",
                "state": "running",
                "last_scan_at": 1_760_190_123_456_u64,
                "restarts": []
            },
            "processes": []
        }),
        "enumeration",
    );
    assert_eq!(
        listing["instances"].as_array().unwrap().len(),
        1,
        "{listing}"
    );
    assert_eq!(listing["instances"][0]["port"], port);
    assert_eq!(listing["instances"][0]["liveness"], "live");
    assert_eq!(listing["instances"][0]["instance"]["pid"], record["pid"]);
    assert_eq!(listing["watchdog"]["state"], "running");
    assert_eq!(listing["watchdog"]["pid"], watchdog["pid"]);
    assert_eq!(listing["unreadable"], json!([]));
    assert!(listing.get("gc_paused").is_none(), "{listing}");
    // 实例与看护者都已各自列出，「其余进程」里没有它们
    assert_eq!(listing["processes"], json!([]), "{listing}");
}

/// [`PINNED_IN_MODULES`] 里的每个单测都还在原处。它们逐字钉住的样本是契约的一部分：
/// 要挪动或改名，连同这张清单一起改；不能删。
#[test]
fn the_samples_pinned_in_the_modules_are_still_there() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for (file, test) in PINNED_IN_MODULES {
        let source = std::fs::read_to_string(root.join(file))
            .unwrap_or_else(|e| panic!("读不到 {file}：{e}"));
        assert!(
            source.contains(&format!("fn {test}()")),
            "{file} 里应有钉住契约样本的单测 {test}"
        );
    }
}
