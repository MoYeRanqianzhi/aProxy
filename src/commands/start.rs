//! `aproxy`（无子命令）/ `aproxy start [别名|路径]`：启动代理。
//!
//! 父进程侧：target 解析、配置校验、端口预检、后台 spawn 与就绪等待；
//! 服务本身由 `crate::server::serve_forever` 承载（守护子进程直接进入其中）。

use std::path::PathBuf;

use aproxy::config::{self, Config, mask_base_url};
use aproxy::daemon;
use aproxy::settings;

use crate::cli::Cli;
use crate::commands::resolve_config_target;
use crate::server::{bind_error_message, report_config_error, serve_forever};

/// 启动路径共用的配置解析：显式配置路径必须存在（打错路径时报明确错误而非
/// 静默回退默认配置）、CLI 覆盖参数、normalize、validate。
pub(crate) fn resolve_runtime_config(
    cli: &Cli,
    cfg_path: &std::path::Path,
) -> Result<Config, String> {
    if cli.config.is_some() && !cfg_path.exists() {
        return Err(format!("指定的配置文件不存在: {}", cfg_path.display()));
    }
    let mut cfg = load_with_cli_overrides(cli, cfg_path);
    // settings.json 全局默认注入：toml 显式值 > settings 值 > 内置默认。
    // get_or_insert 只在 toml 未显式配置时写入 settings 值——这正是
    // 「settings 公用默认、toml 按实例覆盖」的优先级实现点。
    {
        let s = settings::load();
        cfg.max_body_mb.get_or_insert(s.max_body_mb);
        cfg.disk_cache.get_or_insert(s.disk_cache);
    }
    // listen_addr 必须带端口（port_of 取最后一个 ':' 之后）：缺端口/端口越界的
    // bind 失败不是占用，提前拦截给出明确错误，避免被误诊为「被其他程序占用」
    if daemon::port_of(&cfg.listen_addr).parse::<u16>().is_err() {
        return Err(format!(
            "listen_addr 缺少端口或端口无效: {}（示例: 127.0.0.1:12345）",
            cfg.listen_addr
        ));
    }
    // validate 的 proxy 错误消息会内嵌 proxy 原文（URL 里可能带 user:pass），
    // 转给用户前打码，与日志/展示处的保密策略保持一致
    cfg.validate().map_err(|msg| {
        let msg = match cfg.proxy.as_deref() {
            Some(p) => msg.replace(p, &crate::util::mask_proxy_url(p)),
            None => msg,
        };
        format!(
            "配置错误: {msg}\n位置: {}\n\n请执行以下任一操作后重试:\n  aproxy config --baseurl https://api.anthropic.com\n  或手动编辑 {}",
            cfg_path.display(),
            cfg_path.display()
        )
    })?;
    Ok(cfg)
}

/// 宽松加载配置并应用 CLI 覆盖参数（不做校验）：启动路径与守护子进程日志
/// 初始化共用，保证两者对 listen_addr 的认知一致（日志文件名 = 实际监听端口）。
pub(crate) fn load_with_cli_overrides(cli: &Cli, cfg_path: &std::path::Path) -> Config {
    let mut cfg = config::load_from(cfg_path);
    if let Some(u) = &cli.baseurl {
        cfg.base_url = u.clone();
    }
    if let Some(l) = &cli.listen {
        cfg.listen_addr = l.clone();
    }
    if let Some(p) = &cli.proxy {
        cfg.proxy = Some(p.clone());
    }
    if let Some(k) = &cli.api_key {
        cfg.api_key = Some(k.clone());
    }
    cfg.normalized()
}

/// `aproxy`（无子命令）/ `aproxy start [别名|路径]`：后台启动守护进程；
/// --foreground 前台运行。
///
/// target 解析：settings.json 别名表优先（`aproxy alias add` 管理），
/// 其次按配置文件路径；都落空则报错。
///
/// 端口冲突的情况严格区分（控制通道走 IPC，不触碰代理端口）：
/// - IPC ping 通且监听地址一致 → 同端口已有 aProxy 实例，提示正在运行，不重复启动；
/// - IPC ping 通但监听地址不同 → 同端口不同地址的另一实例，实例键（端口号）
///   无法区分，明确拒绝启动；
/// - TCP bind 失败 → 按错误类别区分「被其他程序占用」/「无权限或被系统保留」。
pub(crate) async fn handle_start_cmd(cli: &Cli, cfg_path: PathBuf, target: Option<String>) {
    // start <别名|路径>：别名解析出的路径只在本进程可见，守护子进程的命令行
    // 里必须显式带上 --config（见下方转发参数构造），否则子进程会回退默认配置
    let cfg_path = match target.as_deref() {
        None => cfg_path,
        Some(t) => match resolve_config_target(t) {
            Some(p) => p,
            None => {
                eprintln!(
                    "未知的别名或配置文件: {t}\n用 aproxy alias list 查看已有别名，或 aproxy alias add {t} <路径> 添加"
                );
                std::process::exit(1);
            }
        },
    };
    let cfg = match resolve_runtime_config(cli, &cfg_path) {
        Ok(c) => c,
        Err(msg) => {
            report_config_error(&msg, cli.daemon_child);
            std::process::exit(1);
        }
    };
    let listen_addr = cfg.listen_addr.clone();
    let port = daemon::port_of(&listen_addr).to_string();

    // 守护子进程：直接承载服务，不再走启动预检/后台 spawn（父进程已做）
    if cli.daemon_child {
        serve_forever(cfg, &cfg_path, true).await;
        return;
    }

    // 预检 1：同端口是否已有 aProxy 实例（IPC 探测，不经代理端口）。管道名只含
    // 端口号，同端口不同监听地址的另一实例也会应答——此时实例键（端口号）无法
    // 区分两者，注册表与 IPC 管道会互相顶替（stop 会停错实例），必须明确拒绝。
    if let Ok(info) = daemon::ipc_ping(&port).await {
        if info.listen_addr == listen_addr {
            println!("此端口已有 aProxy 在运行，无需重复启动：");
            println!(
                "  pid {}  监听 http://{}  v{}",
                info.pid, info.listen_addr, info.version
            );
            println!("查看实例: aproxy status    停止: aproxy stop {port}");
            return;
        }
        eprintln!(
            "端口 {port} 已被监听地址 {} 的 aProxy 实例使用（本进程将监听 {listen_addr}），两者不能并存。",
            info.listen_addr
        );
        eprintln!("实例按端口号区分，请为其中一个更换监听地址/端口。");
        std::process::exit(1);
    }

    // 预检 2：端口能否绑定（探测 listener 随即释放，端口让给守护子进程）
    if let Err(e) = tokio::net::TcpListener::bind(&listen_addr).await {
        // bind 探测失败与子进程 IPC 管道建立之间存在时序窗口：上一条 start 的
        // 实例可能 TCP 已就绪但管道/注册表尚未落盘（预检 1 才会 ping 不通）。
        // 报「被占用」前再 ping 一次，避免把自家正在启动的实例误分类为其他程序占用。
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        if let Ok(info) = daemon::ipc_ping(&port).await {
            println!("此端口已有 aProxy 在运行，无需重复启动：");
            println!(
                "  pid {}  监听 http://{}  v{}",
                info.pid, info.listen_addr, info.version
            );
            println!("查看实例: aproxy status    停止: aproxy stop {port}");
            return;
        }
        eprintln!("{}", bind_error_message(&listen_addr, &e));
        std::process::exit(1);
    }

    if cli.foreground {
        serve_forever(cfg, &cfg_path, false).await;
        return;
    }

    // 后台：分离子进程承载服务，命令行参数原样转发 + --daemon-child 标记。
    // --config 的值统一改写为本进程解析出的 cfg_path（绝对路径）：普通启动时
    // 这只是把相对路径转绝对（注册表与子进程的文件读取不依赖工作目录）；
    // `start <别名|路径>` 时命令行里根本没有 --config，别名解析出的路径只有
    // 本进程知道，必须在此注入，否则守护子进程会回退默认配置。
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let cfg_str = cfg_path.display().to_string();
    match args.iter().position(|a| a == "--config") {
        Some(i) => {
            if let Some(val) = args.get_mut(i + 1) {
                *val = cfg_str;
            }
        }
        None => match args.iter().position(|a| a.starts_with("--config=")) {
            Some(i) => args[i] = format!("--config={cfg_str}"),
            None => {
                args.push("--config".to_string());
                args.push(cfg_str);
            }
        },
    }
    args.push("--daemon-child".to_string());
    let exe = std::env::current_exe().expect("无法定位自身可执行文件");
    let pid = match daemon::spawn_detached(&exe, &args) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("启动后台进程失败: {e}");
            std::process::exit(1);
        }
    };

    // 端口 0：实际端口由子进程绑定时才确定，父进程无从按已知端口轮询就绪——
    // 跳过等待（注册表会记录 actual_addr），提示用户用 status 查看真实地址
    if port == "0" {
        println!("aProxy 正在后台启动（listen 端口为 0，实际端口由系统分配）");
        println!("  pid: {pid}");
        println!("请稍后执行 aproxy status 查看实际监听地址。");
        return;
    }

    // 就绪判定用 IPC ping 而非 TCP connect：管道名含端口，只有本守护子进程会
    // 创建 `aproxy-<port>`——connect 成功无法区分「我们的子进程」与「任何抢占
    // 端口的监听者」，曾导致对已死子进程/第三方进程误报启动成功。轮询至 8 秒
    // （子进程冷启动受 Defender 扫描等影响可能偏慢）。
    let log_path = daemon::logs_dir().join(format!("{port}.log"));
    let startup_log_path = daemon::logs_dir().join("startup.log");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    loop {
        if let Ok(info) = daemon::ipc_ping(&port).await {
            println!("aProxy 已在后台启动");
            println!("  pid: {}", info.pid);
            println!("  监听: http://{}", info.listen_addr);
            println!("  Base URL: {}", mask_base_url(&cfg.base_url));
            if !cfg.base_url.is_empty() {
                let base = cfg.base_url.trim_end_matches('/');
                println!(
                    "  提示: 将你的 API base URL 指向 http://{}",
                    info.listen_addr
                );
                println!("        上游路径与查询参数将完整透传到 {base}/<path>?<query>");
            }
            println!("  配置: {}", cfg_path.display());
            println!("  日志: {}", log_path.display());
            println!("查看实例: aproxy status    停止: aproxy stop {port}");
            break;
        }
        if std::time::Instant::now() > deadline {
            // 子进程无控制台，失败原因只可能落盘：配置/绑定错误写 startup.log，
            // 运行日志在端口日志——两个位置都要指给用户
            eprintln!("后台进程未在预期时间内就绪（pid {pid}），启动失败的原因通常记录在:");
            eprintln!("  {}", startup_log_path.display());
            eprintln!("  {}", log_path.display());
            std::process::exit(1);
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    // 看护者出簇（watchdog 总开关，settings 层）：实例就绪后确保存在看护者。
    // 选举规范防止多实例并发 start 时的重复拉起：claim 有效让位；缺席时仅
    // 存活实例 PID 最小者有权 spawn；claim 原子接管最终裁决唯一性。
    ensure_watchdog_if_enabled();
}

/// 确保看护者在任（settings.watchdog 开启时）。
/// 失败静默（看护者缺失只是失去自愈保护，不应让 start 失败）。
fn ensure_watchdog_if_enabled() {
    if !aproxy::settings::load().watchdog {
        return;
    }
    let run_dir = daemon::run_dir();
    let fresh = aproxy::watchdog::heartbeat_fresh_secs(&aproxy::settings::load());
    if aproxy::watchdog::claim_is_in_effect_in(&run_dir, fresh, aproxy::watchdog::now_secs()) {
        return; // 已有在任看护者
    }
    if !aproxy::watchdog::this_process_may_spawn_watchdog_in(&run_dir) {
        return; // 有更小 PID 的存活实例，拉起是它的事（其自检/该实例 start 兜底）
    }
    aproxy::watchdog::remove_claim_in(&run_dir); // 清理无效残留 claim
    let args = aproxy::watchdog::watchdog_spawn_args();
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    match daemon::spawn_detached(&exe, &args) {
        Ok(pid) => tracing::info!(pid, "看护者已随实例启动拉起"),
        Err(e) => tracing::warn!(error = %e, "看护者拉起失败（守护自检会补种）"),
    }
}
