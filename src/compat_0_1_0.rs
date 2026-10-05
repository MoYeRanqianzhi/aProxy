//! 与 0.1.0 互通的兼容层：只为「0.1.0 → 新版本」的原地升级存在，0.1.x 线保留，
//! 0.2.0 删除整个模块（届时在 CHANGELOG 写明升级下限）。
//!
//! 为什么需要：0.1.0 的 Windows 控制管道名只含端口（`\\.\pipe\aproxy-<端口>`），
//! 新版本按 run 目录加了命名空间（见 `daemon::endpoint_for_in`）。用户用 0.1.0 的
//! `aproxy install` 升级时，有一段时间新旧两边同时在跑：
//! - 0.1.0 的安装器、留在 PATH 上的 0.1.0 CLI 只认旧管道名——新守护要在旧名字
//!   上也应答（S1），否则 0.1.0 一侧找不到新实例，它的注册表普查还会把新实例的
//!   记录当死记录删掉；
//! - 新版本的 `install --continue` 要停掉、确认还在跑旧版本的实例——它们只在旧
//!   名字上应答，新客户端在命名空间端点找不到实例时，要能回退到旧名字（S2）。
//!
//! 回退有严格条件，避免重新打开「跨 home 误操作」：只有本 run 目录里登记了该
//! 端口的实例、且旧管道上应答者自报的 pid 与进程创建时间都与记录一致时，
//! 才认它是本 home 的实例。unix 的 socket 一直在 run 目录里，路径没变，不需要
//! 这一层。完整分析见 .agents/plan/ipc-v1.md 的 (c) 节。

/// 0.1.0 的控制管道名（只含端口，全机共享）
#[cfg(windows)]
fn legacy_pipe(port: &str) -> String {
    format!(r"\\.\pipe\aproxy-{port}")
}

/// S1：新守护在 0.1.0 的管道名上也提供控制服务（尽力而为）。名字已被占用
/// （例如别的 home 里同端口的 0.1.0 实例）时只记一条 warn，不影响本实例。
#[cfg(windows)]
pub(crate) fn serve_legacy_pipe(
    port: &str,
    on_shutdown: tokio::sync::watch::Sender<bool>,
    info: crate::daemon::InstanceInfo,
    stats: std::sync::Arc<crate::daemon::IpcStats>,
) {
    let endpoint = legacy_pipe(port);
    tokio::spawn(async move {
        if let Err(e) = crate::daemon::serve_endpoint(endpoint, on_shutdown, info, stats).await {
            tracing::warn!(
                error = %e,
                "0.1.0 兼容控制管道创建失败：不影响本实例，只是 0.1.0 的 CLI 与安装器找不到它"
            );
        }
    });
}

/// S2：命名空间端点上找不到实例时，本 run 目录若登记了该端口的实例，试 0.1.0
/// 的管道名；应答者自报的 pid 与进程创建时间都和记录一致才返回该端点。
/// 0.1.0 在 Windows 上总会登记创建时间；记录里是 0 就无从核验，不认——与
/// 看门狗、stop --force 对「0 = 无法核验」的处理一致。
#[cfg(windows)]
pub(crate) async fn legacy_endpoint_for(run_dir: &std::path::Path, port: &str) -> Option<String> {
    let record = crate::daemon::read_instance_file_in(run_dir, port)?;
    let endpoint = legacy_pipe(port);
    let resp = crate::daemon::request_raw(&endpoint, &crate::daemon::IpcRequest::Ping)
        .await
        .ok()?;
    let live = resp.info?;
    let same_process = record.process_start != 0
        && live.pid == record.pid
        && live.process_start == record.process_start;
    (resp.ok && same_process).then_some(endpoint)
}

/// unix 的 socket 路径与 0.1.0 相同，不需要回退
#[cfg(unix)]
pub(crate) async fn legacy_endpoint_for(_run_dir: &std::path::Path, _port: &str) -> Option<String> {
    None
}
