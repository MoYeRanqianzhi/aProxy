//! 启动一个实例的父进程一侧：解析出运行配置、预检端口、后台拉起并等就绪。
//!
//! 服务本身（守护子进程进入的主循环）在二进制一侧的 `server.rs`；这里只管「把它拉起来」。

use std::path::Path;

use crate::cli::StartArgs;
use crate::config::{self, Config, Overrides};
use crate::daemon::{self, InstanceStatus};
use crate::settings;
use crate::spawn::{self, SpawnError};
use crate::spec::InstanceSpec;

/// `aproxy start` 命令行上给的覆盖。
pub fn overrides_from_cli(args: &StartArgs) -> Overrides {
    Overrides {
        base_url: args.base_url.clone(),
        listen_addr: args.listen_addr.clone(),
        proxy: args.proxy.clone(),
        api_key: args.api_key.clone(),
        log_file: args.log_file.clone(),
    }
}

/// 启动路径共用的配置解析：走分层解析（覆盖 > config.toml > settings 的全局默认 > 内置
/// 默认）。`cfg_path` 为 None 表示没有配置文件，只有另外三层。有 error 级诊断时不启动，
/// Err 里逐条列出：哪个文件、哪个键、第几行。
///
/// 给了路径而文件不在时同样是错误：调用方给出路径，就是认定这个文件应当在（命令行指定的、
/// 别名指向的、拉起规格里记下的），悄悄按空配置启动只会得到一个行为不对的实例。
pub fn resolve_runtime_config(
    overrides: &Overrides,
    cfg_path: Option<&Path>,
) -> Result<Config, String> {
    if let Some(path) = cfg_path
        && !path.exists()
    {
        return Err(format!("配置文件不存在: {}", path.display()));
    }
    // 实例只在启动时读 settings.json：它无效时不带着内置默认悄悄启动——那样全局默认里的
    // 上游、限额都不生效，用户看到的是一个行为不对的实例，而不是一条说得清的错误
    let settings_path = settings::settings_path();
    let loaded = settings::read(&settings_path);
    if !loaded.is_valid() {
        let mut msg = String::from("settings.json 有错误，实例没有启动:");
        for d in loaded.diagnostics.iter().filter(|d| d.is_error()) {
            msg.push_str("\n  ");
            msg.push_str(&render_diagnostic(d));
        }
        msg.push_str(&format!(
            "\n\n用 aproxy settings show 查看，aproxy settings unset <键> 删掉出问题的键；也可以删除 {} 回到内置默认",
            settings_path.display()
        ));
        return Err(msg);
    }
    let resolved = resolve_layers(overrides, cfg_path).map_err(|e| {
        format!(
            "读取配置文件失败: {e}\n文件: {}",
            cfg_path.unwrap_or_else(|| Path::new("")).display()
        )
    })?;
    // 告警级的诊断（例如全局默认的凭据没有被继承）与扣下的凭据，记进日志
    for d in resolved.diagnostics.iter().filter(|d| !d.is_error()) {
        tracing::warn!(code = d.code, "{}", render_diagnostic(d));
    }
    for item in &resolved.withheld {
        tracing::info!(
            key = %item.key,
            reason = item.reason,
            default_target = %item.default_target,
            target = %item.target,
            "全局默认里的这项凭据没有继承：它属于另一个目标"
        );
    }
    if resolved.is_valid() {
        return Ok(resolved.config);
    }
    let mut msg = String::from("配置错误:");
    for d in resolved.errors() {
        msg.push_str("\n  ");
        msg.push_str(&render_diagnostic(d));
    }
    msg.push_str(
        "\n\n请修改后重试。上游地址可以这样设置:\n  aproxy config --baseurl https://api.anthropic.com",
    );
    if let Some(path) = cfg_path {
        msg.push_str(&format!("\n  或手动编辑 {}", path.display()));
    }
    Err(msg)
}

/// 一条诊断写成一行：`文件:行:列: 键: 说明 [码]`，没有的部分省略。
pub fn render_diagnostic(d: &aproxy_ipc::diag::Diagnostic) -> String {
    let mut line = d.file.clone();
    if let Some(l) = d.line {
        line.push_str(&format!(":{l}"));
        if let Some(c) = d.column {
            line.push_str(&format!(":{c}"));
        }
    }
    line.push_str(": ");
    if let Some(key) = &d.key {
        line.push_str(&format!("{key}: "));
    }
    line.push_str(&format!("{} [{}]", d.message, d.code));
    line
}

/// 分层解析一份配置：覆盖、config.toml、settings.json 的全局默认、内置默认。没有配置文件
/// （`cfg_path` 为 None）时只有另外三层。
fn resolve_layers(
    overrides: &Overrides,
    cfg_path: Option<&Path>,
) -> std::io::Result<config::Resolved> {
    let defaults =
        config::resolve::Defaults::from_settings(&settings::load(), &settings::settings_path());
    match cfg_path {
        Some(path) => config::resolve(path, overrides, &defaults),
        None => Ok(config::resolve::resolve_without_file(overrides, &defaults)),
    }
}

/// 宽松地解析配置（不因诊断而失败）：后台实例初始化日志时用。此刻配置还没有校验过，
/// 即使它有问题，也要能定出日志写到哪里，这样问题才有地方记。启动的严格路径是
/// [`resolve_runtime_config`]，两者走同一条解析管线，对监听地址与日志路径的认知一致。
pub fn load_lenient(overrides: &Overrides, cfg_path: Option<&Path>) -> Config {
    match resolve_layers(overrides, cfg_path) {
        Ok(resolved) => resolved.config,
        Err(e) => {
            tracing::warn!(error = %e, "读取配置文件失败，使用默认配置");
            Config::default()
        }
    }
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

/// 后台拉起实例并等它就绪（见 [`crate::spawn`]：按新 pid 定位实际端口，监听端口 0 由
/// 系统分配时同样等到就绪、报出实际地址；子进程提前退出时立即报告，附上 startup.log 本次
/// 新增的内容）。实例就绪后顺带确保看护者在任。
pub async fn launch(exe: &Path, spec: &InstanceSpec) -> Result<InstanceStatus, SpawnError> {
    let home = aproxy_ipc::home::home();
    let live = spawn::spawn_from_spec(&home, exe, spec, spawn::READY_TIMEOUT).await?;
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
    if crate::watchdog::watchdog_in_effect_in(&run_dir, fresh, crate::watchdog::now_millis()) {
        return; // 已有在任看护者
    }
    if !crate::watchdog::this_process_may_spawn_watchdog_in(&run_dir) {
        return; // 有更小 PID 的存活实例，拉起是它的事（其自检/该实例 start 兜底）
    }
    // 无效的看护者记录不在这里删：新看护者接管时自己处理，它要靠记录找到假死的前任
    match crate::watchdog::spawn_watchdog(exe, &aproxy_ipc::home::home()) {
        Ok(pid) => tracing::info!(pid, "看护者已随实例启动拉起"),
        Err(e) => tracing::warn!(error = %e, "看护者拉起失败（守护自检会补种）"),
    }
}
