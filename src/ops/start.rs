//! 启动一个实例的父进程一侧：解析出运行配置、预检端口、后台拉起并等就绪。
//!
//! 服务本身（守护子进程进入的主循环）在二进制一侧的 `server.rs`；这里只管「把它拉起来」。

use std::path::Path;

use crate::cli::Cli;
use crate::config::{self, Config, mask_base_url};
use crate::daemon::{self, InstanceStatus};
use crate::settings;
use crate::spawn::{self, SpawnError};

/// 启动路径共用的配置解析：显式配置路径必须存在（打错路径时报明确错误而非
/// 静默回退默认配置）、CLI 覆盖参数、normalize、validate。
pub fn resolve_runtime_config(cli: &Cli, cfg_path: &Path) -> Result<Config, String> {
    if cli.config.is_some() && !cfg_path.exists() {
        return Err(format!("指定的配置文件不存在: {}", cfg_path.display()));
    }
    // 严格加载：TOML 语法错误在此明确报出——宽松版会把解析失败静默回退成
    // 默认配置（base_url 空），下方的 validate 报错于是变成「base_url 不能
    // 为空」，用户明明写了却被告知没写，排障方向被带偏（实测实锤）
    let mut cfg = config::load_from_strict(cfg_path)?;
    // CLI 覆盖在严格加载之上应用（覆盖参数不需要文件先合法）
    cfg = apply_cli_overrides(cfg, cli);
    // settings.json 全局默认注入：toml 显式值 > settings 值 > 内置默认。
    // get_or_insert 只在 toml 未显式配置时写入 settings 值——这正是
    // 「settings 公用默认、toml 按实例覆盖」的优先级实现点。
    // bounded_retry_paths 是 settings 层第一个能让 start 失败的值（非法正则），
    // 注入前记录来源：validate 报错时若值来自 settings，下方「位置: toml」
    // 的指引找不到问题，必须点名真实来源。
    let bounded_paths_from_settings = cfg.bounded_retry_paths.is_none();
    let keepalive_trigger_from_settings = cfg.keepalive_trigger.is_none();
    {
        let s = settings::load();
        cfg.max_body_mb.get_or_insert(s.max_body_mb);
        cfg.disk_cache.get_or_insert(s.disk_cache);
        cfg.forward_only.get_or_insert(s.forward_only);
        cfg.bounded_retry_paths
            .get_or_insert_with(|| s.bounded_retry_paths.clone());
        cfg.allowed_hosts
            .get_or_insert_with(|| s.allowed_hosts.clone());
        cfg.allowed_origins
            .get_or_insert_with(|| s.allowed_origins.clone());
        cfg.keepalive_trigger
            .get_or_insert_with(|| s.keepalive_trigger.clone());
    }
    // listen_addr 必须带端口（port_of 取最后一个 ':' 之后）：缺端口/端口越界的
    // bind 失败不是占用，提前拦截给出明确错误，避免被误诊为「被其他程序占用」
    if daemon::port_of(&cfg.listen_addr).parse::<u16>().is_err() {
        return Err(format!(
            "listen_addr 缺少端口或端口无效: {}（示例: 127.0.0.1:12345）",
            cfg.listen_addr
        ));
    }
    // validate 的错误消息会内嵌 proxy / base_url 原文（URL 里可能带 user:pass
    // 或查询串），转给用户（以及守护的 startup.log）前打码，与日志/展示处的
    // 保密策略同一出口
    cfg.validate().map_err(|msg| {
        let mut msg = msg;
        for raw in [cfg.proxy.as_deref(), Some(cfg.base_url.as_str())]
            .into_iter()
            .flatten()
            .filter(|s| !s.is_empty())
        {
            msg = msg.replace(raw, &mask_base_url(raw));
        }
        let msg = if bounded_paths_from_settings && msg.contains("bounded_retry_paths") {
            format!("{msg}\n（该值来自 settings.json 的 bounded_retry_paths 全局默认，不在上述 toml 中）")
        } else if keepalive_trigger_from_settings && msg.contains("keepalive_trigger") {
            format!("{msg}\n（该值来自 settings.json 的 keepalive_trigger 全局默认，不在上述 toml 中）")
        } else {
            msg
        };
        format!(
            "配置错误: {msg}\n位置: {}\n\n请执行以下任一操作后重试:\n  aproxy config --baseurl https://api.anthropic.com\n  或手动编辑 {}",
            cfg_path.display(),
            cfg_path.display()
        )
    })?;
    Ok(cfg)
}

/// 宽松加载配置并应用 CLI 覆盖参数（不做校验）：守护子进程日志初始化共用，
/// 保证日志解析对 listen_addr 的认知与启动路径一致（优先级 CLI --log-file、
/// config log_file、内置随机名）。宽松版适用于日志初始化（此刻配置尚未严格
/// 校验，解析失败仍要能解析出日志路径）；启动的严格校验路径用
/// `load_from_strict` 加 [`resolve_runtime_config`]。
pub fn load_with_cli_overrides(cli: &Cli, cfg_path: &Path) -> Config {
    let cfg = config::load_from(cfg_path);
    apply_cli_overrides(cfg, cli)
}

/// 在已加载的配置上应用 CLI 覆盖参数并 normalize。启动严格路径与宽松日志
/// 初始化路径共用，保证两者的覆盖语义一致。
fn apply_cli_overrides(mut cfg: Config, cli: &Cli) -> Config {
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
    if let Some(f) = &cli.log_file {
        cfg.log_file = Some(f.clone());
    }
    cfg.normalized()
}

/// 端口预检通过之后的两种情形。
#[derive(Debug, Clone)]
pub enum Preflight {
    /// 端口空着，可以启动
    Clear,
    /// 同一监听地址上已有本 home 的实例在运行：不必重复启动
    AlreadyRunning(Box<InstanceStatus>),
}

/// 端口预检不通过。
#[derive(Debug)]
pub enum PreflightError {
    /// 这个端口已被**另一个监听地址**上的本 home 实例使用。实例按端口号区分，两者
    /// 不能并存
    PortHeldByOtherAddress {
        port: String,
        listen_addr: String,
        other_listen_addr: String,
    },
    /// 绑定不了，原因见 `error`（被别的程序占用、无权限或端口被系统保留、地址不可用）
    Bind {
        listen_addr: String,
        error: std::io::Error,
    },
}

/// 启动之前的端口预检。控制通道走 IPC，不触碰代理端口：
/// - IPC ping 通且监听地址一致 → 同端口已有 aProxy 实例，不重复启动；
/// - IPC ping 通但监听地址不同 → 同端口不同地址的另一实例，实例键（端口号）无法区分，
///   明确拒绝；
/// - TCP bind 失败 → 把 io 错误交回调用方分类。
pub async fn preflight(listen_addr: &str) -> Result<Preflight, PreflightError> {
    let port = daemon::port_of(listen_addr).to_string();
    // 预检 1：本 home 同端口是否已有 aProxy 实例（IPC 探测，不经代理端口）。
    // 端点按 home + 端口命名，同端口不同监听地址的另一实例也会应答——此时实例键
    // （端口号）无法区分两者，注册表与 IPC 端点会互相顶替（stop 会停错实例），
    // 必须明确拒绝。别的 home 的同端口实例这里看不到，由预检 2 的 bind 失败拦下。
    if let Ok(info) = daemon::ipc_ping(&port).await {
        if info.instance.listen_addr == listen_addr {
            return Ok(Preflight::AlreadyRunning(Box::new(info)));
        }
        return Err(PreflightError::PortHeldByOtherAddress {
            port,
            listen_addr: listen_addr.to_string(),
            other_listen_addr: info.instance.listen_addr,
        });
    }

    // 预检 2：端口能否绑定（探测 listener 随即释放，端口让给守护子进程）
    if let Err(error) = tokio::net::TcpListener::bind(listen_addr).await {
        // bind 探测失败与子进程 IPC 管道建立之间存在时序窗口：上一条 start 的
        // 实例可能 TCP 已就绪但管道/注册表尚未落盘（预检 1 才会 ping 不通）。
        // 报「被占用」前再 ping 一次，避免把自家正在启动的实例误分类为其他程序占用。
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        if let Ok(info) = daemon::ipc_ping(&port).await {
            return Ok(Preflight::AlreadyRunning(Box::new(info)));
        }
        return Err(PreflightError::Bind {
            listen_addr: listen_addr.to_string(),
            error,
        });
    }
    Ok(Preflight::Clear)
}

/// 后台拉起实例并等它就绪：分离子进程承载服务，命令行参数原样转发。
///
/// `args` 是本次启动的命令行（不含程序名）。其中 `--config` 的值统一改写为 `cfg_path`
/// （绝对路径）：普通启动时这只是把相对路径转绝对（注册表与子进程的文件读取不依赖工作
/// 目录）；按别名或路径启动时命令行里根本没有 `--config`，解析出的路径只有调用方知道，
/// 必须在此注入，否则守护子进程会回退默认配置。
///
/// 就绪判定与「启动即死」的报告见 [`crate::spawn`]：按新 pid 在注册表里定位实际端口（监听
/// 端口 0 由系统分配时同样等到就绪、报出实际地址），子进程提前退出时立即报告，附上
/// startup.log 本次新增的内容。实例就绪后顺带确保看护者在任。
pub async fn launch(
    exe: &Path,
    mut args: Vec<String>,
    cfg_path: &Path,
) -> Result<InstanceStatus, SpawnError> {
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
    let live = spawn::spawn_instance(&daemon::run_dir(), exe, &args, spawn::READY_TIMEOUT).await?;
    // 看护者出簇（watchdog 总开关，settings 层）：实例就绪后确保存在看护者。
    // 选举规范防止多实例并发 start 时的重复拉起：claim 有效让位；缺席时仅
    // 存活实例 PID 最小者有权 spawn；claim 原子接管最终裁决唯一性。
    ensure_watchdog_if_enabled(exe);
    Ok(live)
}

/// 确保看护者在任（settings.watchdog 开启时）。
/// 失败静默（看护者缺失只是失去自愈保护，不应让 start 失败）。
fn ensure_watchdog_if_enabled(exe: &Path) {
    let settings = settings::load();
    if !settings.watchdog {
        return;
    }
    let run_dir = daemon::run_dir();
    let fresh = crate::watchdog::heartbeat_fresh_secs(&settings);
    if crate::watchdog::claim_is_in_effect_in(&run_dir, fresh, crate::watchdog::now_secs()) {
        return; // 已有在任看护者
    }
    if !crate::watchdog::this_process_may_spawn_watchdog_in(&run_dir) {
        return; // 有更小 PID 的存活实例，拉起是它的事（其自检/该实例 start 兜底）
    }
    crate::watchdog::remove_claim_in(&run_dir); // 清理无效残留 claim
    let args = crate::watchdog::watchdog_spawn_args();
    match daemon::spawn_detached(exe, &args) {
        Ok(pid) => tracing::info!(pid, "看护者已随实例启动拉起"),
        Err(e) => tracing::warn!(error = %e, "看护者拉起失败（守护自检会补种）"),
    }
}
