//! 启动一个实例的父进程一侧：解析出运行配置、预检端口、后台拉起并等就绪。
//!
//! 服务本身（守护子进程进入的主循环）在二进制一侧的 `server.rs`；这里只管「把它拉起来」。

use std::path::{Path, PathBuf};

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

/// 启动路径上配置解析不出一份能用的配置的原因。`Display` 是给人看的完整说明（逐条列出
/// 诊断与修法），写到 stderr 与 startup.log 的就是它。
#[derive(Debug)]
pub enum ConfigError {
    /// 给了路径而文件不在
    Missing(PathBuf),
    /// settings.json 有 error 级诊断：实例不带着内置默认悄悄启动
    Settings {
        file: PathBuf,
        diagnostics: Vec<aproxy_ipc::diag::Diagnostic>,
    },
    /// 配置文件在，却读不出来
    Read {
        file: PathBuf,
        error: std::io::Error,
    },
    /// 解析出来的配置有 error 级诊断。`file` 是配置文件，没有配置文件时为 None
    Invalid {
        file: Option<PathBuf>,
        diagnostics: Vec<aproxy_ipc::diag::Diagnostic>,
    },
}

impl ConfigError {
    /// error 级的诊断；文件不在、读不出来时没有
    pub fn diagnostics(&self) -> &[aproxy_ipc::diag::Diagnostic] {
        match self {
            Self::Settings { diagnostics, .. } | Self::Invalid { diagnostics, .. } => diagnostics,
            Self::Missing(_) | Self::Read { .. } => &[],
        }
    }
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing(path) => write!(f, "配置文件不存在: {}", path.display()),
            Self::Settings { file, diagnostics } => {
                write!(f, "settings.json 有错误，实例没有启动:")?;
                for d in diagnostics {
                    write!(
                        f,
                        "\n  {}",
                        render_diagnostic(d, aproxy_ipc::i18n::Lang::En)
                    )?;
                }
                write!(
                    f,
                    "\n\n用 aproxy settings show 查看，aproxy settings unset <键> 删掉出问题的键；也可以删除 {} 回到内置默认",
                    file.display()
                )
            }
            Self::Read { file, error } => {
                write!(f, "读取配置文件失败: {error}\n文件: {}", file.display())
            }
            Self::Invalid { file, diagnostics } => {
                write!(f, "配置错误:")?;
                for d in diagnostics {
                    write!(
                        f,
                        "\n  {}",
                        render_diagnostic(d, aproxy_ipc::i18n::Lang::En)
                    )?;
                }
                write!(
                    f,
                    "\n\n请修改后重试。上游地址可以这样设置:\n  aproxy config set base_url https://api.anthropic.com"
                )?;
                if let Some(path) = file {
                    write!(f, "\n  或手动编辑 {}", path.display())?;
                }
                Ok(())
            }
        }
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
) -> Result<Config, ConfigError> {
    resolve_runtime_config_in(&aproxy_ipc::home::home(), overrides, cfg_path)
}

/// 按启动者给定的主目录读取全局默认，校验与实际分层使用同一次读取的设置。
pub fn resolve_runtime_config_in(
    home: &Path,
    overrides: &Overrides,
    cfg_path: Option<&Path>,
) -> Result<Config, ConfigError> {
    if let Some(path) = cfg_path
        && !path.exists()
    {
        return Err(ConfigError::Missing(path.to_path_buf()));
    }
    // 实例只在启动时读 settings.json：它无效时不带着内置默认悄悄启动——那样全局默认里的
    // 上游、限额都不生效，用户看到的是一个行为不对的实例，而不是一条说得清的错误
    let settings_path = settings::settings_path_in(home);
    let loaded = settings::read(&settings_path);
    if !loaded.is_valid() {
        return Err(ConfigError::Settings {
            file: settings_path,
            diagnostics: loaded
                .diagnostics
                .into_iter()
                .filter(|d| d.is_error())
                .collect(),
        });
    }
    let resolved =
        resolve_layers(overrides, cfg_path, &loaded.settings, &settings_path).map_err(|error| {
            ConfigError::Read {
                file: cfg_path.unwrap_or_else(|| Path::new("")).to_path_buf(),
                error,
            }
        })?;
    // 告警级的诊断（例如全局默认的凭据没有被继承）与扣下的凭据，记进日志
    for d in resolved.diagnostics.iter().filter(|d| !d.is_error()) {
        tracing::warn!(
            code = d.code,
            "{}",
            render_diagnostic(d, aproxy_ipc::i18n::Lang::En)
        );
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
    Err(ConfigError::Invalid {
        file: cfg_path.map(Path::to_path_buf),
        diagnostics: resolved.errors().cloned().collect(),
    })
}

/// 一条诊断写成一行：`文件:行:列: 键: 说明 [码]`，没有的部分省略。
pub fn render_diagnostic(d: &aproxy_ipc::diag::Diagnostic, lang: aproxy_ipc::i18n::Lang) -> String {
    render_diagnostic_fields(
        &d.file,
        d.line,
        d.column,
        d.key.as_deref(),
        &d.message.render(lang),
        d.code,
    )
}

/// 也能排版接手版本留在状态里的诊断：码是文本，不要求等待者认识更高版本的码。
pub fn render_diagnostic_fields(
    file: &str,
    row: Option<u32>,
    column: Option<u32>,
    key: Option<&str>,
    message: &str,
    code: &str,
) -> String {
    let mut line = file.to_owned();
    if let Some(l) = row {
        line.push_str(&format!(":{l}"));
        if let Some(c) = column {
            line.push_str(&format!(":{c}"));
        }
    }
    line.push_str(": ");
    if let Some(key) = key {
        line.push_str(&format!("{key}: "));
    }
    line.push_str(&format!("{message} [{code}]"));
    line
}

/// 分层解析一份配置：覆盖、config.toml、settings.json 的全局默认、内置默认。没有配置文件
/// （`cfg_path` 为 None）时只有另外三层。
fn resolve_layers(
    overrides: &Overrides,
    cfg_path: Option<&Path>,
    settings: &settings::Settings,
    path: &Path,
) -> std::io::Result<config::Resolved> {
    let defaults = config::resolve::Defaults::from_settings(settings, path);
    match cfg_path {
        Some(path) => config::resolve(path, overrides, &defaults),
        None => Ok(config::resolve::resolve_without_file(overrides, &defaults)),
    }
}

/// 宽松地解析配置（不因诊断而失败）：后台实例初始化日志时用。此刻配置还没有校验过，
/// 即使它有问题，也要能定出日志写到哪里，这样问题才有地方记。启动的严格路径是
/// [`resolve_runtime_config`]，两者走同一条解析管线，对监听地址与日志路径的认知一致。
pub fn load_lenient(overrides: &Overrides, cfg_path: Option<&Path>) -> Config {
    let path = settings::settings_path();
    match resolve_layers(overrides, cfg_path, &settings::load_from(&path), &path) {
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
    /// 到达时尚在启动，现已等到它完成握手。
    BecameReady(Box<InstanceStatus>),
}

/// 端口预检不通过。
#[derive(Debug)]
pub enum PreflightError {
    /// 同地址原实例在握手期间退出或未能及时就绪，不再另开一份重复的启动。
    Startup(SpawnError),
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

/// 两次端口预检看到同一状态时走同一条路径，不把 starting 伪装成已就绪。
async fn existing_instance(
    home: &Path,
    listen_addr: &str,
    info: InstanceStatus,
) -> Result<Preflight, PreflightError> {
    if info.instance.listen_addr != listen_addr {
        return Err(PreflightError::PortHeldByOtherAddress {
            port: daemon::port_of(listen_addr).to_owned(),
            listen_addr: listen_addr.to_owned(),
            other_listen_addr: info.instance.listen_addr,
        });
    }
    if info.state == daemon::InstanceState::Starting {
        return spawn::wait_existing(home, &info.instance)
            .await
            .map(|i| Preflight::BecameReady(Box::new(i)))
            .map_err(PreflightError::Startup);
    }
    Ok(Preflight::AlreadyRunning(Box::new(info)))
}

/// 启动之前的端口预检。控制通道走 IPC，不触碰代理端口：
/// - IPC ping 通且监听地址一致 → 同端口已有 aProxy 实例，不重复启动；
/// - IPC ping 通但监听地址不同 → 同端口不同地址的另一实例，实例键（端口号）无法区分，
///   明确拒绝；
/// - TCP bind 失败 → 把 io 错误交回调用方分类。
pub async fn preflight(listen_addr: &str) -> Result<Preflight, PreflightError> {
    preflight_in(&aproxy_ipc::home::home(), listen_addr).await
}

/// 端口预检只探测指定主目录的控制通道，不把其他 home 的实例误认为已有实例。
pub async fn preflight_in(home: &Path, listen_addr: &str) -> Result<Preflight, PreflightError> {
    let run_dir = home.join("run");
    let port = daemon::port_of(listen_addr).to_string();
    // 预检 1：本 home 同端口是否已有 aProxy 实例（IPC 探测，不经代理端口）。
    // 端点按 home + 端口命名，同端口不同监听地址的另一实例也会应答——此时实例键
    // （端口号）无法区分两者，注册表与 IPC 端点会互相顶替（stop 会停错实例），
    // 必须明确拒绝。别的 home 的同端口实例这里看不到，由预检 2 的 bind 失败拦下。
    if let Ok(info) = daemon::ipc_ping_in(&run_dir, &port).await {
        return existing_instance(home, listen_addr, info).await;
    }

    // 预检 2：端口能否绑定（探测 listener 随即释放，端口让给守护子进程）
    if let Err(error) = tokio::net::TcpListener::bind(listen_addr).await {
        // bind 探测失败与子进程 IPC 管道建立之间存在时序窗口：上一条 start 的
        // 实例可能 TCP 已就绪但管道/注册表尚未落盘（预检 1 才会 ping 不通）。
        // 报「被占用」前再 ping 一次，避免把自家正在启动的实例误分类为其他程序占用。
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        if let Ok(info) = daemon::ipc_ping_in(&run_dir, &port).await {
            return existing_instance(home, listen_addr, info).await;
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
    launch_in(&aproxy_ipc::home::home(), exe, spec).await
}

/// 实例和看护者都显式绑定同一主目录；子进程不依赖界面进程的工作目录。
pub async fn launch_in(
    home: &Path,
    exe: &Path,
    spec: &InstanceSpec,
) -> Result<InstanceStatus, SpawnError> {
    let live = spawn::spawn_from_spec(home, exe, spec, spawn::READY_TIMEOUT).await?;
    // 看护者出簇（watchdog 总开关，settings 层）：实例就绪后确保存在看护者。
    // 选举规范防止多实例并发 start 时的重复拉起：claim 有效让位；缺席时仅
    // 存活实例 PID 最小者有权 spawn；claim 原子接管最终裁决唯一性。
    ensure_watchdog_if_enabled(home, exe);
    Ok(live)
}

/// 确保看护者在任（settings.watchdog 开启时）。
/// 失败静默（看护者缺失只是失去自愈保护，不应让 start 失败）。
fn ensure_watchdog_if_enabled(home: &Path, exe: &Path) {
    let settings = settings::load_from(&settings::settings_path_in(home));
    if !settings.watchdog {
        return;
    }
    let run_dir = home.join("run");
    let fresh = crate::watchdog::heartbeat_fresh_secs(&settings);
    if crate::watchdog::watchdog_in_effect_in(&run_dir, fresh, crate::watchdog::now_millis()) {
        return; // 已有在任看护者
    }
    if !crate::watchdog::this_process_may_spawn_watchdog_in(&run_dir) {
        return; // 有更小 PID 的存活实例，拉起是它的事（其自检/该实例 start 兜底）
    }
    // 无效的看护者记录不在这里删：新看护者接管时自己处理，它要靠记录找到假死的前任
    match crate::watchdog::spawn_watchdog(exe, home) {
        Ok(pid) => tracing::info!(pid, "看护者已随实例启动拉起"),
        Err(e) => tracing::warn!(error = %e, "看护者拉起失败（守护自检会补种）"),
    }
}

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
pub fn daemon_log_path_in(home: &Path, log_file: Option<&str>) -> PathBuf {
    match log_file {
        Some(raw) => {
            // 只展开 `~`，不用 settings::expand_path：后者按当前工作目录绝对化，
            // 而守护进程的工作目录不可靠——相对路径一律相对 APROXY_HOME
            let expanded = PathBuf::from(config::expand_tilde(raw));
            if expanded.is_absolute() {
                expanded
            } else {
                home.join(expanded)
            }
        }
        None => home.join("logs").join(generate_random_log_name()),
    }
}
