//! 服务承载：前台与守护子进程共用的主循环（bind、注册、IPC、优雅关闭）、
//! 停止信号、日志初始化与配置错误落盘。
//!
//! 与 `commands/start.rs` 的分工：start 负责启动预检与后台 spawn 的父进程侧，
//! server 负责真正「跑起来」的服务进程本身。

use std::path::PathBuf;
use std::time::Duration;

use tracing_subscriber::EnvFilter;

use aproxy::config::{self, Config};
use aproxy::daemon;
use aproxy::settings;
use aproxy::watchdog;

use crate::util::{chrono_like_timestamp, now_unix};
use aproxy::config::mask_base_url;

/// 本进程守护日志的最终路径（resolve + init 后写入，serve_forever 读取）。
/// `None` = 前台实例（日志走控制台，无文件，注册表 log_path 落空串）。
/// OnceLock 保证「进程生命周期内一次解析、处处一致」：随机名在日志初始化时
/// 生成，注册表/恢复记录/轮转共用同一份，不会各自生成出不同的名字。
static RESOLVED_DAEMON_LOG: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();

/// 守护日志随机文件名：`<纳秒时间戳hex>-<pid低16位hex>.log`。
/// 端口是易变标识（换端口重启后按端口命名找不到旧日志），文件名改用启动
/// 时刻的随机标识；同 pid 同纳秒完成两次启动不可能，无需更强的随机源。
/// 无 chrono/rand 依赖，std 时间 + pid 即可。
fn generate_random_log_name() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{nanos:x}-{:04x}.log", std::process::id() & 0xffff)
}

/// 解析守护日志的最终路径：CLI `--log-file` > config.toml `log_file` >
/// 内置随机名。`~` 展开（settings::expand_path）；**相对路径相对 APROXY_HOME
/// 解析**——守护进程的工作目录不可靠（分离 spawn 维持继承 cwd，但终端关闭
/// 等场景下语义不明），主目录是唯一稳定基准。返回绝对化路径。
pub(crate) fn resolve_daemon_log_path(
    cli_log_file: Option<&str>,
    cfg_log_file: Option<&str>,
) -> PathBuf {
    match cli_log_file.or(cfg_log_file) {
        Some(raw) => {
            let expanded = aproxy::settings::expand_path(raw);
            if expanded.is_absolute() {
                expanded
            } else {
                aproxy::settings::home().join(expanded)
            }
        }
        None => aproxy::daemon::logs_dir().join(generate_random_log_name()),
    }
}

/// 本进程守护日志的最终路径（serve_forever 消费）。None = 前台实例。
/// serve_forever 必在日志初始化之后运行（main.rs 的分支顺序保证），未初始化
/// 的违规调用按前台语义落空串处理（不 panic，与 log_path 空串消费方语义一致）。
pub(crate) fn resolved_daemon_log() -> Option<PathBuf> {
    RESOLVED_DAEMON_LOG.get().cloned().flatten()
}

/// 服务主循环：前台与守护子进程共用。bind、注册实例、启动 IPC 控制通道，
/// 一直服务到停止信号（Ctrl+C/SIGTERM 或 `aproxy stop` 经 IPC 触发）。
pub(crate) async fn serve_forever(cfg: Config, cfg_path: &std::path::Path, daemon_child: bool) {
    let listen_addr = cfg.listen_addr.clone();
    let base_url = cfg.base_url.clone();
    // spool 残留清理（disk_cache）：bind 前清空本端口的 spool 目录——目录
    // 归本进程独占，启动时清空即可回收崩溃/强杀残留的临时文件。
    // 仅转发模式下 spool 链路整体不进，本实例永不产生 spool 文件；但这里**不是**
    // no-op：清理逻辑自身会 create_dir_all，于是每个仅转发实例启动时仍会创建
    // ~/.aproxy/spool/<端口>/ 这个空目录，只是无文件可清。空目录无害（不占空间、
    // 与磁盘缓存实例的目录互不干扰），故不做特判，只把注释说准。
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
    // 实时值由 IPC ping 响应携带。log_path 是本实例守护日志的最终路径
    // （随机命名或用户自定义；前台实例无文件落空串）——客户端（aproxy
    // logs/start 提示）一律经 IPC/注册表向实例索取，不按端口拼路径。
    let daemon_log_path = resolved_daemon_log();
    let daemon_log_path_str = daemon_log_path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
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
        proto_version: daemon::IPC_PROTO_VERSION,
        requests_total: 0,
        retries_total: 0,
        last_error: None,
        last_error_at: 0,
        swap_phase: false,
        log_path: daemon_log_path_str,
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
    if let Err(e) = daemon::write_restore_file(&listen_addr, &restore_args, &info.log_path) {
        tracing::warn!(error = %e, "自愈恢复记录写入失败（不影响代理功能）");
    }

    // IPC 控制通道：ping/shutdown 走命名管道，与代理端口完全隔离。
    // 活动时间戳与观测计数由 AppState 持有（请求热路径更新），IPC ping 实时读取——
    // 直接复用 `state.stats` 这**同一份**：此处曾另建一个 IpcStats 只共享活动
    // 时间戳，导致「请求 / 重试 / 最近错误」对任何实例都恒为 0（时间戳正常掩盖了它）。
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let ipc_port = port.clone();
    let ipc_info = info.clone();
    let ipc_stats = state.stats.clone();
    tokio::spawn(async move {
        if let Err(e) = daemon::serve_ipc(ipc_port, stop_tx, ipc_info, ipc_stats).await {
            tracing::error!(error = %e, "IPC 控制通道启动失败（aproxy stop/status 将不可用）");
        }
    });

    // 看门狗心跳（共享内存节）：独立 ticker 每 10s 写一次毫秒时间戳——挂死的
    // 定义是「runtime 无法调度」，ticker 停摆与 runtime 死锁等价；不经请求
    // 热路径（零请求成本）。创建失败只降级（看护者对该实例退化为纯进程死亡
    // 检测），绝不阻断启动。句柄存活于整个服务生命周期（随进程退出由系统回收）。
    if let Some(writer) = watchdog::HeartbeatWriter::create(&port) {
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(
                    watchdog::HEARTBEAT_WRITE_INTERVAL_SECS,
                ))
                .await;
                writer.beat();
            }
        });
    } else {
        tracing::warn!("看门狗心跳节创建失败，该实例将不受挂死检测保护（进程死亡检测不受影响）");
    }

    // 守护侧互保（watchdog 总开关，settings 层）：看护者是增值层而非依赖层——
    // 它崩溃实例不死，本任务只负责把「无人看护」状态在 5 分钟内修复。
    // 选举规范：claim 判定缺席后，存活实例中 PID 最小者才有权 spawn（防
    // N 个守护同时拉起看护者）；claim 原子接管兜底唯一性。
    if settings::load().watchdog {
        tokio::spawn(async move {
            const SELF_CHECK_INTERVAL: Duration = Duration::from_secs(300);
            loop {
                tokio::time::sleep(SELF_CHECK_INTERVAL).await;
                let run_dir = daemon::run_dir();
                let fresh = watchdog::heartbeat_fresh_secs(&settings::load());
                if watchdog::claim_is_in_effect_in(&run_dir, fresh, watchdog::now_secs()) {
                    continue;
                }
                // 安装态宣告有效 → 跳过补种（install 的差异化行为之一）：
                // restarting 窗口内本守护（尚为旧版本）补种的看护者会把滚动
                // 重启误判为崩溃、用旧二进制重拉，与 install 拉锯。install
                // 结束宣告消失 → 自检恢复 → 新看护者出簇（新 exe）。旧版本
                // 守护不认识宣告节的混版本窗口由 install 的 verifying 版本
                // 校验兜底收敛（诚实边界，不做全版本兼容 hack）。
                if aproxy::install::announce::read().is_some_and(|a| {
                    aproxy::install::announce::is_active(&a, watchdog::now_millis())
                }) {
                    continue;
                }
                if !watchdog::this_process_may_spawn_watchdog_in(&run_dir) {
                    continue; // 有更小 PID 的存活实例，拉起是它的事
                }
                // 残留 claim（已判无效）清理后拉起；spawn 失败下轮再试
                watchdog::remove_claim_in(&run_dir);
                let args = watchdog::watchdog_spawn_args();
                let exe = match std::env::current_exe() {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                match daemon::spawn_detached(&exe, &args) {
                    Ok(pid) => {
                        tracing::info!(pid, "看护者缺席，已由守护补种（选举胜出者）");
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "看护者补种失败（下轮自检重试）");
                    }
                }
            }
        });
    }

    // 运行期日志轮转：守护日志只在启动时做过一次 2MiB 检查，长期运行的实例
    // （正是本项目的目标形态）仍会无限膨胀。每小时检查一次，超过 settings 的
    // log_rotate_mb（全局治理项，默认 8MB，0=不轮转）即截断；
    // `aproxy logs` 跟随器已有截断检测（文件变小时自动从头重跟），不会被破坏。
    // 路径是本实例解析出的最终日志路径（随机命名或用户自定义 log_file 均适用）。
    // 仅守护实例执行——前台实例的日志走控制台，无文件可轮转。
    if daemon_child {
        let rotate_limit = settings::load().log_rotate_mb.saturating_mul(1024 * 1024);
        if rotate_limit > 0
            && let Some(rotate_path) = daemon_log_path
        {
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
    // 仅转发模式是「主动放弃重试保障」的取舍，不是无害开关：用 warn 级（而非
    // info）显式提示，并列出本模式下失效的配置项——按老习惯调过的参数不再生效
    // 这件事，用户必须知道，否则排障时会往错误的方向找。取值一律走
    // forward_only_enabled()（settings 注入已完成）。
    if state.config.forward_only_enabled() {
        tracing::warn!(
            "仅转发模式已启用：请求体与响应均不缓冲、不重试（上游失败直接 502、响应流中断直接截断）；disk_cache / spool_limit_mb / keepalive_interval_secs / max_retry_backoff_secs 在本模式下不生效"
        );
    }
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
            remove_registry_files(&actual_addr);
            std::process::exit(0);
        } => {}
    }
    remove_registry_files(&actual_addr);
    tracing::info!("aProxy 已停止");
}

/// 优雅退出的注册表清理。顺序是刻意的：先删 .restore 再删 .pid——看门狗的
/// 死亡事件若恰在两个 unlink 之间触发，读到的状态是「无恢复记录」= 优雅退出；
/// 反序则会误判为崩溃而重拉一个用户刚停掉的实例。末尾顺带清 IPC 端点文件与
/// 心跳文件（Windows 侧均为内核回收，这两个调用是 no-op；unix 清 unix socket
/// 与 /dev/shm 心跳，消除残留）。
fn remove_registry_files(listen_addr: &str) {
    daemon::remove_restore_file(listen_addr);
    daemon::remove_instance_file(listen_addr);
    watchdog::remove_heartbeat_file(daemon::port_of(listen_addr));
    daemon::remove_socket_file(daemon::port_of(listen_addr));
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

/// 守护子进程日志：写解析出的日志文件（随机命名或用户自定义，见
/// `resolve_daemon_log_path`；超过 2 MiB 截断重写，保留最近一次运行的日志
/// 即可，避免无限膨胀）。UTF-8 无 BOM——历史上曾写过 BOM 后撤销（部分
/// 工具链对 BOM 敏感），日志查看依赖终端/编辑器自身的 UTF-8 解码；Windows
/// 控制台乱码与文件无关（进程入口已切 65001）。路径在此写入 OnceLock，
/// 供 serve_forever 组装注册表/恢复记录/轮转共用。
pub(crate) fn init_daemon_logging(log_path: &std::path::Path) {
    let _ = RESOLVED_DAEMON_LOG.set(Some(log_path.to_path_buf()));
    if let Some(parent) = log_path.parent() {
        // 自定义 log_file 的父目录可能不存在（内置随机名已由调用方在
        // resolve 阶段指向既有 logs 目录，此调用对其是无害的重复创建）
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(meta) = std::fs::metadata(log_path)
        && meta.len() > 2 * 1024 * 1024
    {
        let _ = std::fs::write(log_path, b"");
    }
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
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
        // 日志文件打不开则回退 stdout（会被 Stdio::null 吞掉，但至少不 panic）。
        // OnceLock 保持已写入的 Some：注册表仍指向该路径，logs 命令会以
        // 「日志文件迟迟未生成」暴露这个极端退化（磁盘满/权限），不静默伪装正常
        Err(_) => init_stdout_logging(),
    }
}

/// 前台实例的日志初始化：stdout + 标记「本进程无日志文件」（OnceLock 落
/// None，serve_forever 据此在注册表/恢复记录里写空串 log_path）。
pub(crate) fn init_foreground_logging() {
    let _ = RESOLVED_DAEMON_LOG.set(None);
    init_stdout_logging();
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
