//! 服务承载：前台与守护子进程共用的主循环（bind、注册、IPC、优雅关闭）、
//! 停止信号、日志初始化与配置错误落盘。
//!
//! 与 `commands/start.rs` 的分工：start 负责启动预检与后台拉起的父进程侧，
//! server 负责真正「跑起来」的服务进程本身。

use std::path::PathBuf;
use std::time::Duration;

use tracing_subscriber::EnvFilter;

use aproxy::config::{self, Config};
use aproxy::daemon;
use aproxy::settings;
use aproxy::watchdog;

use crate::util::chrono_like_timestamp;

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

/// 定出后台实例的日志路径。`log_file` 是解析后的配置里的值（命令行 `--log-file` 的覆盖
/// 已经并进去）；没有就在 logs 目录下生成一个随机名。`~` 展开；**相对路径相对 APROXY_HOME
/// 解析**——日志路径会被记进拉起规格、在别的进程里再用，不能依赖谁的工作目录。返回
/// 绝对路径。
pub(crate) fn resolve_daemon_log_path(log_file: Option<&str>) -> PathBuf {
    match log_file {
        Some(raw) => {
            // 只展开 `~`，不用 settings::expand_path：后者按当前工作目录绝对化，
            // 而守护进程的工作目录不可靠——相对路径一律相对 APROXY_HOME
            let expanded = PathBuf::from(aproxy::config::expand_tilde(raw));
            if expanded.is_absolute() {
                expanded
            } else {
                aproxy_ipc::home::home().join(expanded)
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

/// 服务主循环：前台实例与后台实例共用。bind、注册实例、启动 IPC 控制通道，
/// 一直服务到停止信号（Ctrl+C/SIGTERM 或 `aproxy stop` 经 IPC 触发）。
///
/// `cfg_path` 是这个实例的配置文件，None = 没有配置文件。`spec` 是后台实例的拉起规格，
/// None = 前台实例：日志在控制台，不写期望状态记录（关掉终端就是用户要它停下）。
pub(crate) async fn serve_forever(
    cfg: Config,
    cfg_path: Option<&std::path::Path>,
    spec: Option<aproxy::spec::InstanceSpec>,
) {
    let daemon_child = spec.is_some();
    let listen_addr = cfg.listen_addr.clone();
    let base_url = cfg.base_url.clone();
    // 给人看的配置文件路径；没有配置文件时为空串
    let cfg_display = cfg_path
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    // 进程台账（永久兼容契约 C3）：占用端口之前登记，`aproxy install --force` 靠它找到并
    // 结束本实例；端口取配置里的监听端口（强制安装据此核对 restore 有没有把实例拉回来），
    // config 是配置文件的绝对路径（拉不回的实例，强制安装据此告诉用户怎样启动），label
    // 是给人看的同一路径。正常退出由守卫删除；下面经 process::exit 的路径一律改走
    // util::exit，同样会删
    let cfg_abs = cfg_path.map(|p| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf()));
    let _ledger = crate::util::register_in_ledger(
        &aproxy_ipc::home::home(),
        aproxy_ipc::ledger::ROLE_INSTANCE,
        daemon::port_of(&listen_addr).parse().ok(),
        cfg_path.map(|_| cfg_display.as_str()),
        cfg_abs.as_deref(),
    );
    let behavior = daemon::Behavior::of(&cfg);
    let state = aproxy::proxy::AppState::new(cfg);
    // 磁盘缓存开着时，建出本进程自己的 spool 目录（按进程分，不按端口：见
    // daemon::spool_dir_name）。别的进程崩溃留下的目录不在这里清——这时还没 bind，本进程
    // 可能马上因为端口被占而退出；清理在开始服务之后做。
    if let Some(dir) = &state.spool_dir {
        daemon::prepare_spool_dir(dir);
    }
    let app = aproxy::proxy::router(state.clone());

    let listener = match tokio::net::TcpListener::bind(&listen_addr).await {
        Ok(l) => l,
        Err(e) => {
            // 守护子进程无控制台，eprintln 会被 Stdio::null 吞掉——失败原因必须
            // 走 report_config_error 落盘（startup.log），否则父进程超时提示指向
            // 的日志里查不到任何线索
            report_config_error(&bind_error_message(&listen_addr, &e), daemon_child);
            crate::util::exit(1);
        }
    };
    let actual_addr = listener.local_addr().expect("获取监听地址失败").to_string();
    let port = daemon::port_of(&actual_addr).to_string();

    // 配置的端口是 0（由系统分配）时，台账里登记的也是 0：强制安装据条目里的端口核对
    // 「实例有没有被拉回来」，0 对不上任何端口。拿到实际端口之后原子重写条目（同一个
    // 文件，守卫照旧负责删它）。重写不成只是少了这项核对，条目仍在、仍能据它结束本进程
    if daemon::port_of(&listen_addr) == "0"
        && let Some(entry) = aproxy_ipc::ledger::LedgerEntry::for_current_process(
            aproxy_ipc::ledger::ROLE_INSTANCE,
            port.parse().ok(),
            cfg_path.map(|_| cfg_display.as_str()),
            cfg_abs.as_deref(),
        )
        && let Err(e) = aproxy_ipc::ledger::write_entry_in(&aproxy_ipc::home::home(), &entry)
    {
        tracing::warn!(error = %e, "台账条目没能改写成实际端口");
    }

    // R1：先退役比自己旧的看护者，再创建控制端点——旧看护者处理死亡事件时会
    // 删掉同一路径上的 socket（见 watchdog::retire_older_watchdogs_in）
    watchdog::retire_older_watchdogs_in(&aproxy_ipc::home::home()).await;

    // 控制端点紧接 TCP 之后、注册表之前创建：失败就不登记、不写恢复记录、
    // 直接退出。带着一个 stop/status 都找不到的实例跑下去比启动失败更糟——
    // 它占着端口，用户只能按 pid 去杀；它的 .pid 还会覆盖同端口实例的记录。
    let ipc_endpoint = match daemon::bind_ipc(&port).await {
        Ok(endpoint) => endpoint,
        Err(e) => {
            report_config_error(
                &format!(
                    "控制通道 {} 创建失败：{e}。同一 APROXY_HOME 里可能已有使用端口 {port} 的 aProxy 实例（监听地址不同）；unix 上 run 目录路径过长也会导致失败",
                    daemon::endpoint_for(&port)
                ),
                daemon_child,
            );
            crate::util::exit(1);
        }
    };

    // 注册实例记录（bind 成功后才写，避免留下死记录）。实时观测值不进记录，
    // 由 IPC ping 应答携带。log_path 是本实例守护日志的最终路径（随机命名或
    // 用户自定义；前台实例无文件，为 null）——客户端（aproxy logs/start 提示）
    // 一律经 IPC/注册表向实例索取，不按端口拼路径。
    let daemon_log_path = resolved_daemon_log();
    let info = daemon::InstanceRecord {
        pid: std::process::id(),
        // 身份锚点：实例现算现写的启动标记（看门狗收养/处决/选举与 stop --force 按
        // 「pid + 本值」防 pid 复用，与二进制名无关）。算不出（没有定义标记的平台）落
        // 空串，这样的记录无法核验身份
        start: aproxy_ipc::ledger::start_marker(std::process::id()).unwrap_or_default(),
        instance_id: daemon::new_instance_id(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        listen_addr: actual_addr.clone(),
        config_path: cfg_path.map(|_| cfg_display.clone()),
        // 注册表与 IPC ping 都会把它交给 status 展示（只用于展示），脱敏后再登记
        base_url: aproxy_ipc::redact::url(&base_url),
        started_at: daemon::now_millis(),
        log_path: daemon_log_path.as_ref().map(|p| p.display().to_string()),
        foreground: !daemon_child,
    };
    if let Err(e) = daemon::write_instance_file(&info) {
        tracing::warn!(error = %e, "实例注册表写入失败（不影响代理功能）");
    }

    // IPC 控制通道（端点已在注册前创建好）：ping/shutdown 走命名管道，与代理
    // 端口完全隔离。活动时间戳与观测计数由 AppState 持有（请求热路径更新），IPC ping 实时读取——
    // 直接复用 `state.stats` 这**同一份**：此处曾另建一个 IpcStats 只共享活动
    // 时间戳，导致「请求 / 重试 / 最近错误」对任何实例都恒为 0（时间戳正常掩盖了它）。
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let control = std::sync::Arc::new(daemon::ControlState {
        record: info.clone(),
        behavior,
        run_dir: daemon::run_dir().display().to_string(),
        stats: state.stats.clone(),
        on_shutdown: stop_tx,
        stop_requested: std::sync::Mutex::new(false),
    });

    // 期望状态记录：后台实例到这里就算「应当在运行」，把拉起规格记下来。之后进程不在而
    // 记录还在（崩溃、断电、系统重启），restore 与看护者据此把它拉回来；只有 stop 删它。
    // 按实际端口命名。配置解析出的端口是 0（由系统分配）时，把实际拿到的端口钉进规格：
    // 否则记录按实际端口命名、规格却还是端口 0，每次恢复都换一个端口、多留一份拉不回的
    // 记录。
    //
    // 必须写在控制端点开始应答之前：拉起方把「控制端点应答了」当作就绪，就绪之后它（以及
    // 紧跟着的 restart、restore、看护者）就会去读这份记录。顺序反过来，就绪与记录落盘
    // 之间有一个窗口，窗口里的 restart 会把一个正常的后台实例当成「没有记录、不归我管」。
    // 这时还没有任何 stop 能到达本实例（端点上排队的连接要等下面的任务来读），所以不会
    // 出现「已经被要求停止、却又把记录写回来」；之后到达的 stop 经 ControlState 删它。
    if let Some(mut spec) = spec {
        if daemon::port_of(&listen_addr) == "0" {
            spec.pinned_port = port.parse().ok();
        }
        match control.write_restore(&spec) {
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "期望状态记录写入失败（不影响代理功能）"),
        }
    }

    {
        let control = control.clone();
        tokio::spawn(async move {
            daemon::serve_ipc(ipc_endpoint, control).await;
        });
    }

    // 实例记录每 30 秒核对一次：被删掉、或内容已不是本进程的（有人清 run 目录、别的工具
    // 误删），就补写。没有记录的实例 status 看不见、看护者不收养、restart 也找不到它，而它
    // 明明还占着端口在服务。已经在退出的实例不补写：退出时会删记录，补写会留下一份死记录。
    {
        let control = control.clone();
        let run_dir = daemon::run_dir();
        tokio::spawn(async move {
            let port = daemon::port_of(&control.record.listen_addr).to_string();
            loop {
                tokio::time::sleep(Duration::from_secs(30)).await;
                if *control.on_shutdown.borrow() {
                    return;
                }
                let intact = daemon::read_instance_file_in(&run_dir, &port)
                    .is_some_and(|on_disk| on_disk == control.record);
                if intact {
                    continue;
                }
                match daemon::write_instance_file_in(&run_dir, &control.record) {
                    Ok(()) => tracing::warn!("实例记录不在或已被改动，已补写"),
                    Err(e) => tracing::warn!(error = %e, "实例记录补写失败"),
                }
                // 补写期间开始退出的：退出路径可能已经删过记录，把刚写下的这一份收掉
                if *control.on_shutdown.borrow() {
                    let _ = std::fs::remove_file(daemon::instance_file_path_in(&run_dir, &port));
                    return;
                }
            }
        });
    }

    // 看门狗心跳：独立 ticker 每 10s 写一次毫秒时间戳——挂死的定义是「runtime 无法
    // 调度」，ticker 停摆与 runtime 死锁等价；不经请求热路径（零请求成本）。写入本身交给
    // 阻塞线程池：unix 上心跳是 home 里的一个文件，home 在网络文件系统上时一次写可能卡住，
    // 不能让它占着运行时的工作线程。创建失败只降级（看护者对该实例退化为纯进程死亡检测），
    // 绝不阻断启动。句柄存活于整个服务生命周期（随进程退出由系统回收）。
    if let Some(writer) = watchdog::HeartbeatWriter::create(&daemon::run_dir(), &port) {
        let writer = std::sync::Arc::new(writer);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(
                    watchdog::HEARTBEAT_WRITE_INTERVAL_SECS,
                ))
                .await;
                let writer = writer.clone();
                let _ = tokio::task::spawn_blocking(move || writer.beat()).await;
            }
        });
    } else {
        tracing::warn!("看门狗心跳创建失败，该实例将不受挂死检测保护（进程死亡检测不受影响）");
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
                if watchdog::watchdog_in_effect_in(&run_dir, fresh, watchdog::now_millis()) {
                    continue;
                }
                // 安装态宣告有效 → 跳过补种（install 的差异化行为之一）：
                // restarting 窗口内本守护（尚为旧版本）补种的看护者会把滚动
                // 重启误判为崩溃、用旧二进制重拉，与 install 拉锯。install
                // 结束宣告消失 → 自检恢复 → 新看护者出簇（新 exe）。
                if aproxy::install::announce::read(&aproxy_ipc::home::home(), &run_dir).is_some_and(
                    |a| aproxy::install::announce::is_active(&a, watchdog::now_millis()),
                ) {
                    continue;
                }
                if !watchdog::this_process_may_spawn_watchdog_in(&run_dir) {
                    continue; // 有更小 PID 的存活实例，拉起是它的事
                }
                // 无效的看护者记录留给新看护者接管时处理（它要靠记录找到假死的前任）；
                // spawn 失败下轮再试
                let exe = match std::env::current_exe() {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                match watchdog::spawn_watchdog(&exe, &aproxy_ipc::home::home()) {
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

    // 清理（见 daemon::collect_garbage_in）：实例已经登记、控制端点在应答，这时清掉别的
    // 进程崩溃留下的记录、交接文件与没人引用的旧日志。放在这里而不是放在 status 里：
    // 查看状态的命令不该删东西。文件系统操作放进阻塞线程，不占运行时的工作线程
    tokio::task::spawn_blocking(|| {
        let report = daemon::collect_garbage_in(&daemon::run_dir(), &daemon::logs_dir());
        if !report.removed.is_empty() {
            tracing::info!(
                removed = report.removed.len(),
                "清理了残留的运行期文件与旧日志"
            );
        }
        let spool = daemon::collect_stale_spool_in(&daemon::spool_root());
        if !spool.is_empty() {
            tracing::info!(removed = spool.len(), "清理了已退出的进程留下的 spool 目录");
        }
        if report.logs_paused {
            tracing::warn!(
                "run 目录里有读不懂的记录（多半由更新版本的 aproxy 写下），旧日志的清理已暂停"
            );
        }
    });

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
        base_url = %aproxy_ipc::redact::url(&base_url),
        config = %cfg_display,
        pid = %std::process::id(),
        "启动 aProxy"
    );
    // 仅转发模式是「主动放弃重试保障」的取舍，不是无害开关：用 warn 级（而非
    // info）显式提示，并列出本模式下失效的配置项——按老习惯调过的参数不再生效
    // 这件事，用户必须知道，否则排障时会往错误的方向找。取值一律走
    // forward_only_enabled()（settings 注入已完成）。
    if state.config.forward_only_enabled() {
        tracing::warn!(
            "仅转发模式已启用：请求体与响应均不缓冲、不重试（上游失败直接 502、响应流中断直接截断）；disk_cache / max_response_mb / keepalive.interval_secs / retry.max_backoff_secs 在本模式下不生效"
        );
    }
    // 保活适用的请求发往上游时 accept-encoding 一律改为 identity（往压缩流里插
    // 明文心跳会让客户端解压失败，见 proxy.rs 保活通道）。override_headers 里
    // 用户显式配的 accept-encoding 因此只对不走保活的请求生效——这条覆盖是
    // 静默的，启动时说一次，免得用户以为配置被无视了却查不到原因
    if let Some(value) = state
        .config
        .accept_encoding_override_shadowed_by_keepalive()
    {
        tracing::warn!(
            configured = %value,
            "override_headers 里的 accept-encoding 对保活适用的请求不生效：这些请求发往上游时一律改为 identity（压缩流里无法插入心跳），只有不走保活的请求按配置值发送"
        );
    }
    // 外部转换器改变了「原样透传」的承诺，启动时必须显式留痕：请求/响应将
    // 交给 format 程序改写，两侧失败语义不同（请求 502 不重试、响应透传）
    for (name, t) in [
        ("request", &state.config.request_transform),
        ("response", &state.config.response_transform),
        ("heartbeat", &state.config.heartbeat_transform),
    ] {
        if let Some(t) = t {
            tracing::info!(
                direction = name,
                command = %t.command,
                mode = ?t.mode,
                pool_max = t.effective_pool_max(),
                "外部转换器已启用（信封协议交给外部 format 程序改写）"
            );
        }
    }
    if !daemon_child {
        println!("aProxy 已启动（前台）");
        println!("  监听: http://{actual_addr}");
        println!("  Base URL: {}", aproxy_ipc::redact::url(&base_url));
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
            daemon::remove_registry_files(&actual_addr);
            crate::util::exit(0);
        } => {}
    }
    daemon::remove_registry_files(&actual_addr);
    // 在途请求都已结束，各自的临时文件已删，这里收掉本进程的 spool 目录。上面那条强制
    // 退出的路径不收：还有请求在写它，留给下一个开始服务的实例或看护者清理
    if let Some(dir) = &state.spool_dir {
        daemon::remove_spool_dir(dir);
    }
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
        let _ = aproxy::fsx::create_dir_all(parent);
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

/// 强制安装的日志：与前台命令一样走 stdout，但写失败一律忽略——永久兼容契约 C7：写输出
/// 失败不得改变退出码。`tracing_subscriber::fmt()` 默认在写失败时改用 `eprintln!` 报告，
/// 标准错误也断了的话 `eprintln!` 会 panic（退出码 101）。
pub(crate) fn init_force_logging() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .log_internal_errors(false)
        .init();
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
        let _ = aproxy::fsx::create_dir_all(daemon::logs_dir());
        let open = |path: &std::path::Path| {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .ok()
        };
        let mut file = open(&daemon::logs_dir().join("startup.log")).or_else(|| {
            let _ = aproxy::fsx::create_dir_all(config::config_dir());
            open(&config::config_dir().join("startup.log"))
        });
        if let Some(f) = file.as_mut() {
            use std::io::Write;
            let _ = writeln!(f, "[{}] {msg}", chrono_like_timestamp());
        }
    }
    eprintln!("{msg}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_log_file_resolves_against_aproxy_home() {
        // 相对路径一律相对 APROXY_HOME，与当前工作目录无关；绝对路径原样；`~` 展开
        let home = aproxy_ipc::home::home();
        assert_eq!(
            resolve_daemon_log_path(Some("logs/a.log")),
            home.join("logs/a.log")
        );
        let abs = std::env::temp_dir().join("aproxy-abs.log");
        assert_eq!(
            resolve_daemon_log_path(Some(&abs.display().to_string())),
            abs
        );
        let tilde = resolve_daemon_log_path(Some("~/aproxy-tilde.log"));
        assert!(
            tilde.is_absolute() && tilde.ends_with("aproxy-tilde.log"),
            "{}",
            tilde.display()
        );
    }
}
