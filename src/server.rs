//! 服务承载：前台与守护子进程共用的主循环（bind、注册、IPC、优雅关闭）、
//! 停止信号、日志初始化与配置错误落盘。
//!
//! 与 `commands/start.rs` 的分工：start 负责启动预检与后台 spawn 的父进程侧，
//! server 负责真正「跑起来」的服务进程本身。

use tracing_subscriber::EnvFilter;

use aproxy::config::{self, Config};
use aproxy::daemon;
use aproxy::settings;

use crate::util::{chrono_like_timestamp, now_unix};
use aproxy::config::mask_base_url;

/// 服务主循环：前台与守护子进程共用。bind、注册实例、启动 IPC 控制通道，
/// 一直服务到停止信号（Ctrl+C/SIGTERM 或 `aproxy stop` 经 IPC 触发）。
pub(crate) async fn serve_forever(cfg: Config, cfg_path: &std::path::Path, daemon_child: bool) {
    let listen_addr = cfg.listen_addr.clone();
    let base_url = cfg.base_url.clone();
    // spool 残留清理（disk_cache）：bind 前清空本端口的 spool 目录——目录
    // 归本进程独占，启动时清空即可回收崩溃/强杀残留的临时文件
    daemon::clean_spool_dir(daemon::port_of(&listen_addr));
    let state = aproxy::proxy::AppState::new(cfg);
    let app = aproxy::proxy::router(state.clone());

    let listener = match tokio::net::TcpListener::bind(&listen_addr).await {
        Ok(l) => l,
        Err(e) => {
            // 守护子进程无控制台，eprintln 会被 Stdio::null 吞掉——失败原因必须
            // 走 report_config_error 落盘（startup.log），否则父进程超时提示指向
            // 的日志里查不到任何线索
            report_config_error(&bind_error_message(&listen_addr, &e), daemon_child);
            std::process::exit(1);
        }
    };
    let actual_addr = listener.local_addr().expect("获取监听地址失败").to_string();
    let port = daemon::port_of(&actual_addr).to_string();

    // 注册实例信息（bind 成功后才写，避免留下死记录）。
    // last_activity_secs 落盘的是注册时刻快照（注册表仅供枚举展示），
    // 实时值由 IPC ping 响应携带。
    let info = daemon::InstanceInfo {
        pid: std::process::id(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        listen_addr: actual_addr.clone(),
        config_path: cfg_path.display().to_string(),
        base_url: base_url.clone(),
        started_at: now_unix(),
        last_activity_secs: state
            .last_activity_secs
            .load(std::sync::atomic::Ordering::Relaxed),
    };
    if let Err(e) = daemon::write_instance_file(&info) {
        tracing::warn!(error = %e, "实例注册表写入失败（不影响代理功能）");
    }

    // 自愈恢复记录：bind 成功即认为「此实例期望在运行」。记录的是启动参数
    // （从当前进程命令行取，即 start 父进程转发来的原始参数），崩溃/断电/
    // 系统重启后 aproxy restore 据此一键拉起；优雅退出时删除。前台实例也写
    // （前台被终端关闭属非正常退出，恢复合理）。
    let restore_args: Vec<String> = {
        let mut v: Vec<String> = std::env::args().skip(1).collect();
        v.retain(|a| a != "--daemon-child" && a != "--foreground");
        v
    };
    if let Err(e) = daemon::write_restore_file(&listen_addr, &restore_args) {
        tracing::warn!(error = %e, "自愈恢复记录写入失败（不影响代理功能）");
    }

    // IPC 控制通道：ping/shutdown 走命名管道，与代理端口完全隔离。
    // 活动时间戳由 AppState 持有（请求热路径更新），IPC ping 实时读取。
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let ipc_port = port.clone();
    let ipc_info = info.clone();
    let last_activity = state.last_activity_secs.clone();
    tokio::spawn(async move {
        if let Err(e) = daemon::serve_ipc(ipc_port, stop_tx, ipc_info, last_activity).await {
            tracing::error!(error = %e, "IPC 控制通道启动失败（aproxy stop/status 将不可用）");
        }
    });

    // 运行期日志轮转：守护日志只在启动时做过一次 2MiB 检查，长期运行的实例
    // （正是本项目的目标形态）仍会无限膨胀。每小时检查一次，超过 settings 的
    // log_rotate_mb（全局治理项，默认 8MB，0=不轮转）即截断；
    // `aproxy logs` 跟随器已有截断检测（文件变小时自动从头重跟），不会被破坏。
    // 仅守护实例执行——前台实例的日志走控制台，无文件可轮转。
    if daemon_child {
        let rotate_limit = settings::load().log_rotate_mb.saturating_mul(1024 * 1024);
        if rotate_limit > 0 {
            let rotate_path = daemon::logs_dir().join(format!("{port}.log"));
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
                    if let Ok(meta) = std::fs::metadata(&rotate_path)
                        && meta.len() > rotate_limit
                    {
                        // 截断而非 rename 轮转：跟随器与启动时的 2MiB 检查都按
                        // 「文件变小」设计，且不产生需要再治理的轮转文件堆
                        let _ = std::fs::write(&rotate_path, b"");
                        tracing::info!("日志文件超过 {} MB，已截断", rotate_limit / 1024 / 1024);
                    }
                }
            });
        }
    }

    // 日志与控制台都可能被粘贴分享，内嵌凭据的 base_url 一律打码后输出
    tracing::info!(
        listen = %actual_addr,
        base_url = %mask_base_url(&base_url),
        config = %cfg_path.display(),
        pid = %std::process::id(),
        "启动 aProxy"
    );
    if !daemon_child {
        println!("aProxy 已启动（前台）");
        println!("  监听: http://{actual_addr}");
        println!("  Base URL: {}", mask_base_url(&base_url));
        println!("按 Ctrl+C 退出。");
    }

    // 优雅关闭 + 宽限强退：keepalive 后台重试任务不会主动结束，axum::serve 等待
    // 在途连接完成时可能被其无限期挂起，收到停止信号后给 10 秒宽限窗口即强制退出。
    let graceful = stop_signal(stop_rx.clone());
    tokio::select! {
        result = axum::serve(listener, app).with_graceful_shutdown(graceful) => {
            result.unwrap();
        }
        _ = async {
            stop_signal(stop_rx).await;
            tracing::info!("10 秒后强制退出（在途请求将中断）");
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            daemon::remove_instance_file(&actual_addr);
            daemon::remove_restore_file(&actual_addr);
            std::process::exit(0);
        } => {}
    }
    daemon::remove_instance_file(&actual_addr);
    daemon::remove_restore_file(&actual_addr);
    tracing::info!("aProxy 已停止");
}

/// bind 失败分类：「地址被占用」/「权限不足或被系统保留」（Windows 上常见于
/// Hyper-V/WinNAT 的排除端口区间——netstat 查不到监听者，按占用排查会走弯路）/
/// 其他原因原样给出 io 错误。避免把非占用失败一律误诊为「被其他程序占用」。
pub(crate) fn bind_error_message(addr: &str, e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::AddrInUse => format!("端口 {addr} 被其他程序占用，无法启动"),
        std::io::ErrorKind::PermissionDenied => format!(
            "端口 {addr} 无法绑定：无权限或端口被系统保留（如 Hyper-V/WinNAT 排除区间，可用 netsh interface ipv4 show excludedportrange protocol=tcp 查看）"
        ),
        _ => format!("无法监听 {addr}: {e}"),
    }
}

/// 优雅停止信号：控制台 Ctrl+C/SIGTERM，或 `aproxy stop` 经 IPC 触发，任一即到。
///
/// 注意 watch 通道关闭（Err）不是停止信号：唯一的 Sender 在 IPC serve 任务里，
/// 该任务任何故障退出都会关闭通道——若把关闭当信号，IPC 的任何失败都会令守护
/// 进程静默自杀（与错误日志宣称的「仅 stop/status 不可用、代理继续运行」相反）。
/// 因此通道关闭后转为永久挂起，只等控制台信号；仅值变为 true 才是 IPC 停止请求。
async fn stop_signal(mut ipc_rx: tokio::sync::watch::Receiver<bool>) {
    tokio::select! {
        _ = shutdown_signal() => {},
        _ = async {
            loop {
                match ipc_rx.changed().await {
                    Ok(()) if *ipc_rx.borrow() => return,
                    Ok(()) => {}
                    Err(_) => std::future::pending::<()>().await,
                }
            }
        } => {},
    }
    tracing::info!("收到停止信号，正在关闭...");
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c().await.expect("安装 Ctrl+C 监听失败");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("安装 SIGTERM 监听失败")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    // 停止原因的日志由调用方 stop_signal 统一输出
}

pub(crate) fn init_stdout_logging() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
}

/// 守护子进程日志：写 `~/.aproxy/logs/<端口>.log`（超过 2 MiB 截断重写，
/// 保留最近一次运行的日志即可，避免无限膨胀）。文件创建/截断时写入 UTF-8
/// BOM——无 BOM 的 UTF-8 会被按 ANSI 探测的查看器（记事本旧版/部分编辑器）
/// 误判为 GBK 而显示中文乱码。
pub(crate) fn init_daemon_logging(listen_addr: &str) {
    let _ = std::fs::create_dir_all(daemon::logs_dir());
    let path = daemon::logs_dir().join(format!("{}.log", daemon::port_of(listen_addr)));
    if let Ok(meta) = std::fs::metadata(&path)
        && meta.len() > 2 * 1024 * 1024
    {
        let _ = std::fs::write(&path, b"");
    }
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        Ok(file) => {
            tracing_subscriber::fmt()
                .with_env_filter(
                    EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
                )
                .with_writer(std::sync::Mutex::new(file))
                .with_ansi(false)
                .init();
        }
        // 日志文件打不开则回退 stdout（会被 Stdio::null 吞掉，但至少不 panic）
        Err(_) => init_stdout_logging(),
    }
}

/// 配置错误报告：前台/父进程走 stderr；守护子进程无控制台，错误写
/// startup.log——就绪等待超时的提示虽指向端口日志，但配置错误发生在
/// 日志初始化之前，只有这里能留下线索。logs 目录不可写时退回 config 目录，
/// 尽量留下线索，两级都失败才放弃。
pub(crate) fn report_config_error(msg: &str, daemon_child: bool) {
    if daemon_child {
        let _ = std::fs::create_dir_all(daemon::logs_dir());
        let open = |path: &std::path::Path| {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .ok()
        };
        let mut file = open(&daemon::logs_dir().join("startup.log")).or_else(|| {
            let _ = std::fs::create_dir_all(config::config_dir());
            open(&config::config_dir().join("startup.log"))
        });
        if let Some(f) = file.as_mut() {
            use std::io::Write;
            let _ = writeln!(f, "[{}] {msg}", chrono_like_timestamp());
        }
    }
    eprintln!("{msg}");
}
