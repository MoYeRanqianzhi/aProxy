//! 安装入口与交棒后的安全检查：实例、台账、控制端点与二进制都在独立的临时 home 中。
mod common;
use common::CopiedExecutable;

#[test]
fn copied_executable_retries_only_the_pre_exec_busy_error() {
    use std::io::ErrorKind;
    let mut attempts = 0;
    common::retry_exec(|| {
        attempts += 1;
        if attempts == 1 {
            Err(ErrorKind::ExecutableFileBusy.into())
        } else {
            Ok(())
        }
    })
    .unwrap();
    assert_eq!(attempts, 2);
    let err = common::retry_exec(|| {
        attempts += 1;
        Err::<(), _>(ErrorKind::PermissionDenied.into())
    })
    .unwrap_err();
    assert_eq!((err.kind(), attempts), (ErrorKind::PermissionDenied, 3));
}
use aproxy::install::{
    state::{self, InstallPhase, InstallSource, InstallState},
    swap,
};
use common::{ChildGuard, TestHome, free_port};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    time::Duration,
};

/// 复制本次构建的产物，作为本地发布源或规范位置的实例镜像，不请求发布网站。
fn binary(path: &Path) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::copy(env!("CARGO_BIN_EXE_aproxy"), path).unwrap();
}

/// 准备 --from 源与可辨认的 bin 内容，续作也不允许启动 skill 下载。
fn fixture() -> (TestHome, PathBuf, PathBuf) {
    let h = TestHome::new();
    h.settings(r#"{"watchdog":{"enabled":false},"update":{"skill_auto_update":false}}"#);
    let from = h.path().join(aproxy::install::staging::binary_name());
    binary(&from);
    let bin = swap::bin_path_in(h.path());
    fs::create_dir_all(bin.parent().unwrap()).unwrap();
    fs::write(&bin, b"original image").unwrap();
    (h, from, bin)
}

/// 失败时把子进程输出带进断言，尤其是无控制台的交棒结局。
fn run(h: &TestHome, args: &[&str], code: i32) -> Output {
    let out = h.cmd().args(args).output().unwrap();
    assert_eq!(out.status.code(), Some(code), "{out:?}");
    out
}

/// 已退出的安装者留在 downloaded 相位；不用模拟旧进程，也不引入在线安装。
fn downloaded(h: &TestHome) {
    let v = env!("CARGO_PKG_VERSION");
    let staged = aproxy::install::staging::staging_dir_in(h.path(), v)
        .join(aproxy::install::staging::binary_name());
    binary(&staged);
    let mut st = InstallState::new_marking(v, InstallSource::From);
    st.phase = InstallPhase::Downloaded;
    st.staged_path = Some(staged.display().to_string());
    st.installer_pid = u32::MAX;
    st.installer_version = None;
    state::write_in(h.path(), &mut st).unwrap();
}

/// 活进程写合法台账，却没有可读的实例记录。守卫只终止自己创建的这个子进程。
fn unmanaged(h: &TestHome) -> ChildGuard {
    use aproxy_ipc::ledger::{self, LedgerEntry};
    #[cfg(windows)]
    let mut cmd = {
        let mut cmd = Command::new("ping");
        cmd.args(["-n", "60", "127.0.0.1"]);
        cmd
    };
    #[cfg(not(windows))]
    let mut cmd = {
        let mut cmd = Command::new("sleep");
        cmd.arg("60");
        cmd
    };
    let child = ChildGuard(
        cmd.stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let mut entry =
        LedgerEntry::for_current_process(ledger::ROLE_INSTANCE, Some(free_port()), None, None)
            .unwrap();
    entry.pid = child.0.id();
    entry.start = ledger::start_marker(entry.pid).unwrap();
    ledger::write_entry_in(h.path(), &entry).unwrap();
    child
}

/// 从指定镜像运行实例，直至控制通道应答；守护实例还须完成期望记录的写入。
async fn instance(h: &TestHome, exe: &Path, foreground: bool) -> (u16, ChildGuard) {
    let port = free_port();
    let cfg = h.path().join("instance.toml");
    fs::write(
        &cfg,
        format!("base_url = \"https://install.example.com\"\nlisten_addr = \"127.0.0.1:{port}\"\n"),
    )
    .unwrap();
    h.stop_on_drop(port);
    let mut cmd = if foreground {
        let mut c = h.cmd_with(exe);
        c.arg("start").arg(&cfg).arg("--foreground");
        c
    } else {
        h.serve_cmd_with(exe, &cfg)
    };
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let child = ChildGuard(cmd.spawn_ready().unwrap());
    let dir = h.path().join("run");
    for _ in 0..100 {
        if let Ok(status) = aproxy::daemon::ipc_ping_in(&dir, &port.to_string()).await
            && status.instance.pid == child.0.id()
            && (foreground || aproxy::spec::restore_path_in(&dir, &port.to_string()).is_file())
        {
            return (port, child);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("实例未就绪");
}

/// 可观察的 pid 比仅有的端口更能说明原实例从未被替换。
async fn pid(h: &TestHome, p: u16) -> u32 {
    aproxy::daemon::ipc_ping_in(&h.path().join("run"), &p.to_string())
        .await
        .unwrap()
        .instance
        .pid
}

/// 入口不占锁、不落状态，规范镜像仍是原内容。
#[test]
fn install_fails_before_touching_anything_with_unmanaged_instances() {
    let (h, from, bin) = fixture();
    let child = unmanaged(&h);
    let lines = events(&run(
        &h,
        &[
            "install",
            "--from",
            from.to_str().unwrap(),
            "--no-skills",
            "--json",
        ],
        1,
    ));
    let result = lines.last().unwrap();
    assert_eq!(result["event"], "result");
    assert_eq!(result["outcome"], "failed");
    assert_eq!(result["error"]["code"], "install_unmanaged_instances");
    assert_eq!(lines.len(), 1);
    let out = run(
        &h,
        &["install", "--from", from.to_str().unwrap(), "--no-skills"],
        1,
    );
    assert!(
        String::from_utf8(out.stderr)
            .unwrap()
            .contains(&format!("pid {}", child.0.id()))
    );
    assert!(!state::state_path_in(h.path()).exists());
    assert_eq!(fs::read(bin).unwrap(), b"original image");
}

/// 指令里明说如何停止前台实例；实际停止之后，普通安装能成功。
#[tokio::test]
async fn install_refuses_foreground_instances_before_touching_anything() {
    let (h, from, bin) = fixture();
    let (p, child) = instance(&h, &from, true).await;
    let lines = events(&run(
        &h,
        &[
            "install",
            "--from",
            from.to_str().unwrap(),
            "--no-skills",
            "--json",
        ],
        1,
    ));
    let result = lines.last().unwrap();
    assert_eq!(result["event"], "result");
    assert_eq!(result["outcome"], "failed");
    assert_eq!(result["error"]["code"], "install_foreground_instances");
    assert_eq!(lines.len(), 1);
    assert_eq!(result["error"]["details"], serde_json::json!({"ports":[p]}));
    let zh = h
        .cmd()
        .env("APROXY_LANG", "zh-CN")
        .args([
            "install",
            "--from",
            from.to_str().unwrap(),
            "--no-skills",
            "--json",
        ])
        .output()
        .unwrap();
    assert_eq!(zh.status.code(), Some(1));
    assert_eq!(events(&zh), lines);
    assert!(
        String::from_utf8(zh.stderr)
            .unwrap()
            .contains("上的实例在前台运行")
    );
    let out = run(
        &h,
        &["install", "--from", from.to_str().unwrap(), "--no-skills"],
        1,
    );
    assert!(
        String::from_utf8(out.stderr)
            .unwrap()
            .contains("running in the foreground")
    );
    assert!(!state::state_path_in(h.path()).exists());
    assert_eq!(fs::read(bin).unwrap(), b"original image");
    assert_eq!(pid(&h, p).await, child.0.id());
    run(&h, &["stop", &p.to_string()], 0);
    run(
        &h,
        &["install", "--from", from.to_str().unwrap(), "--no-skills"],
        0,
    );
}

/// 失败相位、英文原因与原镜像一起验证，避免只转告错误却已经交换。
fn halted(h: &TestHome, bin: &Path, code: &str, prefix: &str) -> InstallState {
    let st = state::load_in(h.path()).unwrap();
    assert!(st.halted);
    assert_eq!(st.phase, InstallPhase::Failed);
    assert_eq!(st.failed_at, Some(InstallPhase::Downloaded));
    assert_eq!(st.error_code.as_deref(), Some(code));
    let msg = st.last_error.as_deref().unwrap();
    assert!(msg.is_ascii() && msg.starts_with(prefix), "{msg}");
    assert_eq!(fs::read(bin).unwrap(), b"original image");
    st
}

/// 续作接手时不能信任先前安装者的检查；restore 不得继续推进这个现场。
#[test]
fn continuation_fails_before_broadcast_with_unmanaged_instances() {
    let (h, _, bin) = fixture();
    let _child = unmanaged(&h);
    downloaded(&h);
    run(&h, &["install", "--continue"], 1);
    let st = halted(
        &h,
        &bin,
        "install_unmanaged_instances",
        "live instances started by an older version",
    );
    assert_eq!(st.error_details, Some(serde_json::json!({})));
    run(&h, &["restore"], 0);
    std::thread::sleep(Duration::from_secs(3));
    let after = state::load_in(h.path()).unwrap();
    assert_eq!(
        (after.updated_at, after.installer_pid),
        (st.updated_at, st.installer_pid)
    );
}

/// 续作拒绝前台实例后保持同一进程，排除原因的显式安装可以接手。
#[tokio::test]
async fn continuation_fails_before_broadcast_with_foreground_instances() {
    let (h, from, bin) = fixture();
    let (p, child) = instance(&h, &from, true).await;
    downloaded(&h);
    run(&h, &["install", "--continue"], 1);
    let st = halted(
        &h,
        &bin,
        "install_foreground_instances",
        "The instance on port",
    );
    assert_eq!(st.error_details, Some(serde_json::json!({"ports":[p]})));
    assert_eq!(pid(&h, p).await, child.0.id());
    run(&h, &["stop", &p.to_string()], 0);
    run(
        &h,
        &["install", "--from", from.to_str().unwrap(), "--no-skills"],
        0,
    );
}

/// 无效 settings 在安装入口只是告警，但目标版本不能拿它去停掉仍在服务的实例。
#[tokio::test]
async fn install_precheck_failure_changes_nothing() {
    let (h, from, bin) = fixture();
    binary(&bin);
    let (p, child) = instance(&h, &bin, false).await;
    let before = fs::metadata(&bin).unwrap().modified().unwrap();
    h.settings(r#"{"schema":1,"nope":1}"#);
    let lines = events(&run(
        &h,
        &[
            "install",
            "--from",
            from.to_str().unwrap(),
            "--no-skills",
            "--json",
        ],
        3,
    ));
    let result = lines.last().unwrap();
    assert_eq!(result["event"], "result");
    assert_eq!(result["outcome"], "failed");
    assert_eq!(result["error"]["code"], "install_precheck_failed");
    assert_eq!(result["error"]["details"]["port"], p);
    assert_eq!(pid(&h, p).await, child.0.id());
    let out = run(
        &h,
        &["install", "--from", from.to_str().unwrap(), "--no-skills"],
        3,
    );
    let err = String::from_utf8(out.stderr).unwrap();
    assert!(
        err.contains("Precheck failed for the instance on port")
            && err.contains("config_unknown_key")
            && !err.contains("Rolling restarts stopped"),
        "{err}"
    );
    let st = state::load_in(h.path()).unwrap();
    assert!(st.halted);
    assert_eq!(st.phase, InstallPhase::Failed);
    assert_eq!(st.failed_at, Some(InstallPhase::Downloaded));
    assert_eq!(st.error_code.as_deref(), Some("install_precheck_failed"));
    let details = st.error_details.unwrap();
    assert_eq!(details["port"], p);
    let d = &details["diagnostics"][0];
    assert_eq!(d["severity"], "error");
    assert_eq!(d["key"], "nope");
    assert_eq!(
        st.last_error.as_deref(),
        result["error"]["message"].as_str()
    );
    assert!(d["message"].as_str().unwrap().contains("nope"));
    assert_eq!(st.error_render.as_ref().unwrap()["id"], "precheck");
    assert!(
        result["error"].get("render").is_none()
            && result["error"]["details"].get("error_render").is_none()
    );
    assert_eq!(pid(&h, p).await, child.0.id());
    assert_eq!(fs::metadata(bin).unwrap().modified().unwrap(), before);
}

/// 没有应重启的端口就没有预检目标；settings 的错误不妨碍安装能修好它的版本。
#[test]
fn install_with_invalid_settings_and_no_instances_completes() {
    let (h, from, _) = fixture();
    h.settings(r#"{"schema":1,"nope":1}"#);
    let out = run(
        &h,
        &["install", "--from", from.to_str().unwrap(), "--no-skills"],
        0,
    );
    assert!(String::from_utf8(out.stderr).unwrap().contains("[WARN]"));
}

/// 上一轮记下的端口即便此刻没有应答，也不能漏掉它损坏的期望状态。
#[test]
fn continuation_prechecks_an_unreadable_record_on_a_down_port() {
    let (h, _, bin) = fixture();
    downloaded(&h);
    let p = free_port();
    let mut st = state::load_in(h.path()).unwrap();
    st.instance_snapshot = vec![p.to_string()];
    state::write_in(h.path(), &mut st).unwrap();
    let dir = h.path().join("run");
    fs::create_dir_all(&dir).unwrap();
    fs::write(aproxy::spec::restore_path_in(&dir, &p.to_string()), "bad").unwrap();
    run(&h, &["install", "--continue"], 3);
    let st = halted(
        &h,
        &bin,
        "install_precheck_failed",
        "Precheck failed for the instance on port",
    );
    let d = st.error_details.unwrap();
    assert_eq!(d["port"], p);
    assert_eq!(d["diagnostics"][0]["code"], "install_restore_unreadable");
}

/// 同一份终态现场能被 restore 拉起的续作立即清掉；只读的 status 应让它逐字保持。
#[test]
fn status_does_not_resume_a_leftover_install() {
    let h = TestHome::new();
    let mut st = InstallState::new_marking("x", InstallSource::From);
    st.phase = InstallPhase::Aborted;
    state::write_in(h.path(), &mut st).unwrap();
    let path = state::state_path_in(h.path());
    let before = fs::read(&path).unwrap();
    run(&h, &["status"], 0);
    std::thread::sleep(Duration::from_secs(3));
    assert_eq!(fs::read(path).unwrap(), before);
}

/// 这个 halted 现场的实例均在目标镜像上，显式续作会清掉它，所以能检测到多余的 spawn。
#[tokio::test]
async fn restore_does_not_spawn_a_continuation_for_a_halted_install() {
    let (h, _, bin) = fixture();
    binary(&bin);
    let (p, _child) = instance(&h, &bin, false).await;
    let mut st = InstallState::new_marking(env!("CARGO_PKG_VERSION"), InstallSource::From);
    st.phase = InstallPhase::Failed;
    st.halted = true;
    st.instance_snapshot.push(p.to_string());
    st.installer_pid = u32::MAX;
    state::write_in(h.path(), &mut st).unwrap();
    let path = state::state_path_in(h.path());
    let before = fs::read(&path).unwrap();
    run(&h, &["restore"], 0);
    std::thread::sleep(Duration::from_secs(3));
    assert_eq!(fs::read(&path).unwrap(), before);
    run(&h, &["install", "--continue"], 0);
    assert!(!path.exists());
}

/// GitHub 和 npm 共用指定的下载代理，关闭的环回端口让两次查询都在本地失败。
#[test]
fn install_check_reports_a_failed_query() {
    let h = TestHome::new();
    let proxy = format!("http://127.0.0.1:{}", free_port());
    let out = run(
        &h,
        &["install", "--check", "--json", "--download-proxy", &proxy],
        1,
    );
    let doc: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["schema"], 1);
    assert_eq!(doc["error"]["code"], "install_download_failed");
    assert_eq!(doc["error"]["details"], serde_json::json!({}));
    assert!(doc["error"]["message"].is_string());
    assert!(
        String::from_utf8(out.stderr)
            .unwrap()
            .contains("Checking the latest version")
    );
    for name in ["ledger", "install", "bin"] {
        assert!(!h.path().join(name).exists());
    }
}

/// 每一行都是独立文档，以便管道消费者在安装尚未结束时就能使用它。
fn events(out: &Output) -> Vec<serde_json::Value> {
    std::str::from_utf8(&out.stdout)
        .unwrap()
        .lines()
        .map(|line| {
            let doc: serde_json::Value = serde_json::from_str(line).unwrap();
            assert_eq!(doc["schema"], 1);
            doc
        })
        .collect()
}

/// 一次从规范镜像运行的真实滚动跨过交棒，验证完整的相位、实例和最终应答。
#[tokio::test]
async fn install_json_reports_plan_phases_instances_and_result() {
    let (h, from, bin) = fixture();
    binary(&bin);
    let (p, child) = instance(&h, &bin, false).await;
    let out = run(
        &h,
        &[
            "install",
            "--from",
            from.to_str().unwrap(),
            "--json",
            "--no-skills",
        ],
        0,
    );
    let lines = events(&out);
    assert_eq!(
        lines[0],
        serde_json::json!({"schema":1,"event":"plan","ports":[p]})
    );
    let phases: Vec<_> = lines
        .iter()
        .filter(|d| d["event"] == "phase")
        .map(|d| {
            assert!(d["at"].as_u64().unwrap() > 0);
            serde_json::from_value::<InstallPhase>(d["phase"].clone()).unwrap()
        })
        .collect();
    assert_eq!(
        phases,
        [InstallPhase::RUN_ORDER, &[InstallPhase::Done]].concat()
    );
    let instances: Vec<_> = lines.iter().filter(|d| d["event"] == "instance").collect();
    assert_eq!(
        instances,
        vec![&serde_json::json!({"schema":1,"event":"instance","port":p,"outcome":"restarted"})]
    );
    assert_eq!(
        lines.last().unwrap(),
        &serde_json::json!({"schema":1,"event":"result","outcome":"installed","version":env!("CARGO_PKG_VERSION")})
    );
    assert_ne!(pid(&h, p).await, child.0.id());
}

/// 即便还可以通过期望记录找到应答者，缺失本 schema 的实例记录也不能把它列进滚动计划。
#[tokio::test]
async fn install_plan_requires_a_readable_instance_record_not_only_an_ipc_reply() {
    let (h, from, bin) = fixture();
    binary(&bin);
    let (p, child) = instance(&h, &bin, false).await;
    let path = aproxy::daemon::instance_file_path_in(&h.path().join("run"), &p.to_string());
    let original = fs::read(&path).unwrap();
    fs::write(&path, r#"{"schema":999}"#).unwrap();
    let out = run(
        &h,
        &[
            "install",
            "--from",
            from.to_str().unwrap(),
            "--no-skills",
            "--json",
        ],
        1,
    );
    fs::write(path, original).unwrap();
    let lines = events(&out);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["error"]["code"], "install_unmanaged_instances");
    assert_eq!(pid(&h, p).await, child.0.id());
    assert!(!state::state_path_in(h.path()).exists());
}

/// 无法决定目标的失败也必须以单行结束，不能先输出空计划来表示「没有实例」。
#[test]
fn install_json_probe_failure_has_no_plan() {
    let h = TestHome::new();
    let missing = h.path().join("no-image");
    let lines = events(&run(
        &h,
        &["install", "--from", missing.to_str().unwrap(), "--json"],
        1,
    ));
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["outcome"], "failed");
    assert_eq!(lines[0]["error"]["code"], "install_probe_failed");
    assert!(lines[0]["version"].is_null());
}

/// 不认识排版参数不影响失败码或完整英文证据；退出凭据仍严格匹配事务和大小界限。
#[test]
fn private_install_causes_survive_handover_and_unknown_ids_fall_back() {
    use aproxy::install::{InstallFailure, reason::Cause, state::ExitEvidence};
    use aproxy_ipc::i18n::Lang;
    let h = TestHome::new();
    let mut st = InstallState::new_marking("test", InstallSource::From);
    let failure =
        InstallFailure::because("install_unmanaged_instances", Cause::Unmanaged(vec![77]));
    state::fail_in(h.path(), &mut st, &failure);
    let mut saved = state::load_in(h.path()).unwrap();
    assert_eq!(
        saved.error_details.as_ref().unwrap(),
        &serde_json::json!({})
    );
    let en = saved.last_error.clone().unwrap();
    let restore = |s: &InstallState| InstallFailure::recorded(s, en.clone());
    assert!(restore(&saved).render(Lang::Zh).contains("旧版本启动"));
    for unknown in [None, Some(serde_json::json!({"id":"future_reason"}))] {
        saved.error_render = unknown;
        assert_eq!(restore(&saved).render(Lang::Zh), en);
        assert_eq!(restore(&saved).code, failure.code);
    }
    ExitEvidence::record(
        h.path(),
        &st,
        "install_failed",
        &Cause::NoTerminal(Some(259)),
    )
    .unwrap();
    let evidence = ExitEvidence::read(h.path(), &st).unwrap();
    assert!(evidence.reason.contains("exit status 259 (0x00000103)"));
    assert!(evidence.cause().render(Lang::Zh).contains("退出状态 259"));
    let path = h.path().join("install/last-exit.json");
    let mut old: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    old.as_object_mut().unwrap().remove("render");
    old.as_object_mut().unwrap().remove("code");
    fs::write(&path, serde_json::to_vec(&old).unwrap()).unwrap();
    assert_eq!(
        ExitEvidence::read(h.path(), &st)
            .unwrap()
            .cause()
            .render(Lang::Zh),
        old["reason"]
    );
    ExitEvidence::record(
        h.path(),
        &st,
        "install_failed",
        &Cause::Panic("x".repeat(10000)),
    )
    .unwrap();
    let doc: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(doc["reason"].as_str().unwrap().chars().count(), 4096);
    assert!(doc.get("render").is_none());
}
