//! aProxy — 本地 API 代理，无限重试保障 agent 工作流不中断。

use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::time::Duration;
use tracing_subscriber::EnvFilter;

use aproxy::config::{self, Config};
use aproxy::daemon;
use aproxy::doctor;
use aproxy::find;
use aproxy::settings;

#[derive(Parser, Debug, Clone)]
#[command(name = "aproxy", version, about = "Local API proxy with infinite retries", long_about = None)]
struct Cli {
    /// 配置文件路径（默认 ~/.aproxy/config.toml）。
    /// 多开不同配置的进程时各自指定，例如：
    /// aproxy --config ~/.aproxy/work.toml 与 aproxy --config ~/.aproxy/personal.toml
    /// （listen_addr 须互不相同）
    #[arg(long, value_name = "PATH", global = true)]
    config: Option<PathBuf>,

    /// 前台运行（日志输出到控制台，Ctrl+C 停止）；默认在后台运行，
    /// 日志写 ~/.aproxy/logs/<端口>.log，用 `aproxy status`/`aproxy stop` 管理
    #[arg(long)]
    foreground: bool,

    /// 上游 API base URL，覆盖配置文件中的 base_url
    #[arg(long, value_name = "URL")]
    baseurl: Option<String>,

    /// 监听地址，覆盖配置文件中的 listen_addr
    #[arg(long, value_name = "ADDR")]
    listen: Option<String>,

    /// 上游代理 URL（仅本次运行生效，不写入配置），覆盖配置文件中的 proxy
    #[arg(long, value_name = "URL")]
    proxy: Option<String>,

    /// 快捷 api_key（等效覆盖 Authorization: Bearer <key>，仅本次运行生效），覆盖配置文件中的 api_key
    /// （建议改用配置文件，命令行参数可被本机其他进程枚举）
    #[arg(long, value_name = "KEY")]
    api_key: Option<String>,

    /// [内部] 守护子进程标记：后台启动时父进程把它附加到子进程命令行，
    /// 子进程据此以守护模式运行（无控制台、日志写文件）。勿手动使用。
    /// 必须是 global：start 子命令转发时它出现在 `start` 之后。
    #[arg(long, hide = true, global = true)]
    daemon_child: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug, Clone, PartialEq)]
enum Commands {
    /// 列出运行中的 aProxy 实例（来自实例注册表，存活以 IPC 探测为准）
    Status,

    /// 启动代理（与直接运行 `aproxy` 相同），可指定配置别名或配置文件路径。
    /// 别名用 `aproxy alias add` 管理，如 `aproxy start openrouter`。
    Start {
        /// 配置别名（settings.json 中的别名表）或配置文件路径
        #[arg(value_name = "ALIAS|PATH")]
        target: Option<String>,
    },

    /// 停止运行中的实例。
    /// 单个实例时可直接 `aproxy stop`；多实例必须指定端口号、`all` 或配置别名：
    /// `aproxy stop 12345` / `aproxy stop all` / `aproxy stop openrouter`
    Stop {
        /// 端口号、all 或配置别名
        #[arg(value_name = "PORT|all|ALIAS")]
        target: Option<String>,
    },

    /// 连接到运行中的实例并实时输出其守护日志（Ctrl+C 退出）。
    /// 单个实例时可直接 `aproxy logs`；多实例必须指定端口号。
    /// 不支持 all：一次只能连接一个实例。
    Logs {
        /// 端口号
        #[arg(value_name = "PORT")]
        target: Option<String>,
    },

    /// 一键恢复先前在运行、因崩溃/断电/系统重启而消失的实例。
    /// 无可恢复实例时静默结束（适合配置为开机自启）。
    /// 已在运行的实例自动跳过；配置文件已不存在的记录被清理。
    Restore,

    /// 管理配置别名（存于 ~/.aproxy/settings.json，内部配置不建议手改）。
    /// 别名指向某个 config.toml，让多开配置可以用名字快捷启停。
    Alias {
        #[command(subcommand)]
        cmd: AliasCmd,
    },

    /// 全面体检全部配置（settings.json error 级检查 + 别名配置深入审查 +
    /// 配置目录 toml 扫描，均含端口冲突检测）
    Doctor,

    /// 从配置目录列表（settings.json 的 config_dirs + 默认两个）发现全部
    /// config（toml），支持按别名归属与关键字/端口过滤
    Find {
        /// 别名归属过滤：--aliased 只列已配别名的，--unaliased 只列未配的
        #[arg(long, conflicts_with = "unaliased")]
        aliased: bool,
        /// 只列未配置别名的
        #[arg(long)]
        unaliased: bool,
        /// 关键字过滤：匹配路径片段或别名（大小写不敏感）
        #[arg(value_name = "QUERY")]
        query: Option<String>,
        /// 按监听端口过滤
        #[arg(long, value_name = "PORT")]
        port: Option<u16>,
    },

    /// 查看或修改配置（配置文件位于 ~/.aproxy/config.toml）
    Config(ConfigArgs),
}

#[derive(Subcommand, Debug, Clone, PartialEq)]
enum AliasCmd {
    /// 添加/覆盖别名（path 省略时指向默认配置 ~/.aproxy/config.toml）
    Add {
        /// 别名（不能为 all 或纯数字——会被 start/stop 当作端口/保留字解析）
        name: String,
        /// 指向的 config.toml 路径（支持 ~ 展开）
        #[arg(value_name = "PATH")]
        path: Option<String>,
    },
    /// 删除别名
    Remove {
        /// 别名
        name: String,
    },
    /// 列出全部别名
    List,
}

/// `aproxy config` 的参数集。独立成结构体以整体传递给处理函数，
/// 避免 main 与函数之间逐字段搬运十几个参数。
#[derive(clap::Args, Debug, Clone, PartialEq)]
struct ConfigArgs {
    /// 设置上游 base URL，例如 https://api.anthropic.com
    #[arg(long, value_name = "URL")]
    baseurl: Option<String>,
    /// 设置监听地址，例如 127.0.0.1:12345
    #[arg(long, value_name = "ADDR")]
    listen: Option<String>,
    /// 快捷设置 api_key（等效覆盖 Authorization: Bearer <key>）
    #[arg(long, value_name = "KEY")]
    api_key: Option<String>,
    /// 额外请求头（仅当未携带时追加），格式 key=value，可重复
    #[arg(long = "extra-header", value_name = "KEY=VALUE")]
    extra_headers: Vec<String>,
    /// 覆盖请求头（无条件覆盖），格式 key=value，可重复
    #[arg(long = "override-header", value_name = "KEY=VALUE")]
    override_headers: Vec<String>,
    /// 保活心跳间隔秒数，0 表示关闭
    #[arg(long, value_name = "SECS")]
    keepalive_secs: Option<u64>,
    /// 设置上游代理 URL，例如 http://127.0.0.1:7890 或 socks5://user:pass@127.0.0.1:7890
    #[arg(long, value_name = "URL")]
    proxy: Option<String>,
    /// 设置代理用户名（可选，优先于 URL 内嵌的用户名）
    #[arg(long, value_name = "USER")]
    proxy_username: Option<String>,
    /// 设置代理密码（可选，优先于 URL 内嵌的密码）
    #[arg(long, value_name = "PASS")]
    proxy_password: Option<String>,
    /// 清空已配置的 api_key
    #[arg(long)]
    clear_api_key: bool,
    /// 清空 extra/override 头
    #[arg(long)]
    clear_headers: bool,
    /// 清空代理配置（URL、用户名、密码）
    #[arg(long)]
    clear_proxy: bool,
    /// 把指定路径设为默认配置文件（写入 settings.json，不指定时默认
    /// ~/.aproxy/config.toml；支持 ~ 展开）
    #[arg(long, value_name = "PATH")]
    set_default: Option<String>,
    /// 取消默认配置文件设置（恢复用 ~/.aproxy/config.toml）
    #[arg(long)]
    clear_default: bool,
    /// 打印当前配置及文件路径
    #[arg(long)]
    show: bool,
}

#[tokio::main]
async fn main() {
    // Windows 控制台默认代码页（如 936/GBK）会把 Rust 输出的 UTF-8 中文显示为
    // 乱码——status/stop/logs 等所有面向用户的输出都是中文。切换到 UTF-8
    // （65001）；无控制台的守护子进程上调用失败被忽略，无副作用。
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Console::{GetConsoleOutputCP, SetConsoleOutputCP};
        if GetConsoleOutputCP() != 65001 {
            SetConsoleOutputCP(65001);
        }
    }

    let cli = Cli::parse();

    // 实际生效的配置文件路径：--config 显式指定 > settings.json 的 default_config
    // （用户可把日常主力配置换成任意文件）> 默认 ~/.aproxy/config.toml。
    // default_config 失效（文件被删/移动）必须在此明确报错：静默回退默认配置会让
    // 用户在错误的配置文件上排障（报错信息指向 config.toml，而他配置的是另一个文件）
    let cfg_path = match cli.config.clone() {
        Some(p) => p,
        None => {
            let s = settings::load();
            match &s.default_config {
                Some(p) => {
                    let path = settings::expand_path(p);
                    if !path.is_file() {
                        eprintln!(
                            "settings.json 指定的默认配置文件不存在: {}\n用 aproxy config --set-default <路径> 重新指定，或 aproxy config --clear-default 取消",
                            path.display()
                        );
                        std::process::exit(1);
                    }
                    path
                }
                None => config::config_path(),
            }
        }
    };

    // 日志初始化：守护子进程无控制台，写日志文件；其余走 stdout（RUST_LOG 可覆盖）
    if cli.daemon_child {
        // 此刻配置尚未严格校验，用宽松 load + CLI 覆盖取监听端口命名日志文件；
        // 校验失败的错误会写入 startup.log（见 report_config_error）。覆盖必须
        // 先于端口提取：--listen 改端口时日志名须与实际监听端口一致，否则父进程
        // 打印的日志路径指向一个永远不会被创建的文件
        let cfg = load_with_cli_overrides(&cli, &cfg_path);
        init_daemon_logging(&cfg.listen_addr);
    } else {
        init_stdout_logging();
    }

    // 守护子进程只承载服务，忽略转发来的子命令：start 父进程把完整命令行
    // （含 `start <别名>`）转发给子进程，若子进程再进入 Start 分支会递归
    // spawn 出套娃进程。分流必须先于子命令 match。
    if cli.daemon_child {
        handle_start_cmd(&cli, cfg_path, None).await;
        return;
    }

    // settings.json 检查（error 级）：每次软件运行都执行——别名损坏、JSON 语法
    // 错误等会严重影响 start/stop 按名字操作，必须立刻报出。不退出：管理命令
    // status/stop 不能因内部配置损坏而不可用。doctor 子命令会再次汇总（含分级），
    // 此处跳过避免重复输出。
    if cli.command != Some(Commands::Doctor) {
        settings::check_and_report();
    }

    // 子命令分派；无子命令 = 启动代理（默认后台）
    match cli.command {
        Some(Commands::Status) => handle_status_cmd().await,
        Some(Commands::Start { ref target }) => {
            let cli = cli.clone();
            handle_start_cmd(&cli, cfg_path, target.clone()).await;
        }
        Some(Commands::Stop { target }) => handle_stop_cmd(target).await,
        Some(Commands::Logs { target }) => handle_logs_cmd(target).await,
        Some(Commands::Restore) => handle_restore_cmd().await,
        Some(Commands::Alias { cmd }) => handle_alias_cmd(cmd),
        Some(Commands::Doctor) => handle_doctor_cmd().await,
        Some(Commands::Find {
            aliased,
            unaliased,
            query,
            port,
        }) => handle_find_cmd(aliased, unaliased, query, port),
        Some(Commands::Config(args)) => handle_config_cmd(cfg_path, args),
        None => handle_start_cmd(&cli, cfg_path, None).await,
    }
}

/// 解析 start/stop 的 target：别名优先（settings.json），其次按配置文件路径。
/// 都落空返回 None（调用方报「未知的别名或配置文件」）。
fn resolve_config_target(target: &str) -> Option<PathBuf> {
    if let Some(p) = settings::resolve_alias(target) {
        return Some(PathBuf::from(p));
    }
    let p = settings::expand_path(target);
    if p.is_file() {
        return Some(p);
    }
    None
}

/// 配置路径的运行实例匹配键：下沉到 settings::path_match_key（别名匹配、
/// 配置目录去重、doctor 扫描共用同一比较规则）。
use settings::path_match_key as config_path_key;

/// 启动路径共用的配置解析：显式配置路径必须存在（打错路径时报明确错误而非
/// 静默回退默认配置）、CLI 覆盖参数、normalize、validate。
fn resolve_runtime_config(cli: &Cli, cfg_path: &std::path::Path) -> Result<Config, String> {
    if cli.config.is_some() && !cfg_path.exists() {
        return Err(format!("指定的配置文件不存在: {}", cfg_path.display()));
    }
    let cfg = load_with_cli_overrides(cli, cfg_path);
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
            Some(p) => msg.replace(p, &mask_proxy_url(p)),
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
fn load_with_cli_overrides(cli: &Cli, cfg_path: &std::path::Path) -> Config {
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
async fn handle_start_cmd(cli: &Cli, cfg_path: PathBuf, target: Option<String>) {
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
            return;
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
}

/// bind 失败分类：「地址被占用」/「权限不足或被系统保留」（Windows 上常见于
/// Hyper-V/WinNAT 的排除端口区间——netstat 查不到监听者，按占用排查会走弯路）/
/// 其他原因原样给出 io 错误。避免把非占用失败一律误诊为「被其他程序占用」。
fn bind_error_message(addr: &str, e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::AddrInUse => format!("端口 {addr} 被其他程序占用，无法启动"),
        std::io::ErrorKind::PermissionDenied => format!(
            "端口 {addr} 无法绑定：无权限或端口被系统保留（如 Hyper-V/WinNAT 排除区间，可用 netsh interface ipv4 show excludedportrange protocol=tcp 查看）"
        ),
        _ => format!("无法监听 {addr}: {e}"),
    }
}

/// 服务主循环：前台与守护子进程共用。bind、注册实例、启动 IPC 控制通道，
/// 一直服务到停止信号（Ctrl+C/SIGTERM 或 `aproxy stop` 经 IPC 触发）。
async fn serve_forever(cfg: Config, cfg_path: &std::path::Path, daemon_child: bool) {
    let listen_addr = cfg.listen_addr.clone();
    let base_url = cfg.base_url.clone();
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

    // 注册实例信息（bind 成功后才写，避免留下死记录）
    let info = daemon::InstanceInfo {
        pid: std::process::id(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        listen_addr: actual_addr.clone(),
        config_path: cfg_path.display().to_string(),
        base_url: base_url.clone(),
        started_at: now_unix(),
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

    // IPC 控制通道：ping/shutdown 走命名管道，与代理端口完全隔离
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let ipc_port = port.clone();
    let ipc_info = info.clone();
    tokio::spawn(async move {
        if let Err(e) = daemon::serve_ipc(ipc_port, stop_tx, ipc_info).await {
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

/// `aproxy status`：列出注册表中的实例（存活以 IPC 探测为准）。
async fn handle_status_cmd() {
    let instances = daemon::list_instances().await;
    if instances.is_empty() {
        println!("没有运行中的 aProxy 实例。");
        return;
    }
    println!("运行中的 aProxy 实例 ({}):", instances.len());
    for info in &instances {
        println!(
            "  端口 {}  pid {}  v{}  已运行 {}",
            daemon::port_of(&info.listen_addr),
            info.pid,
            info.version,
            humanize_uptime(info.started_at)
        );
        println!(
            "    监听 http://{}   上游 {}   配置 {}",
            info.listen_addr,
            mask_base_url(&info.base_url),
            info.config_path
        );
    }
}

/// `aproxy stop [PORT|all|ALIAS]`：单个实例可省略参数；多实例必须指定端口号、
/// all 或配置别名（按实例注册的 config_path 匹配）。
async fn handle_stop_cmd(target: Option<String>) {
    // 别名定位：非数字且非 all 的 target 先查别名表——按 config_path 匹配
    // 运行实例（别名不依赖端口号，端口变了别名依然有效）
    if let Some(target) = target.as_deref()
        && target != "all"
        && target.parse::<u16>().is_err()
    {
        match resolve_config_target(target) {
            Some(cfg_path) => {
                let key = config_path_key(&cfg_path.display().to_string());
                let instances = daemon::list_instances().await;
                let found = instances
                    .iter()
                    .find(|i| config_path_key(&i.config_path) == key);
                match found {
                    Some(info) => stop_instance(info).await,
                    None => {
                        println!("别名 {target}（配置 {}）当前未在运行。", cfg_path.display());
                        std::process::exit(1);
                    }
                }
                return;
            }
            None => {
                eprintln!(
                    "未知的别名或端口号: {target}\n用 aproxy alias list 查看已有别名，或 aproxy alias add {target} <路径> 添加"
                );
                std::process::exit(1);
            }
        }
    }

    // 指定端口时不依赖注册表：直接按端口 IPC 定位（注册表丢失也能停）
    if let Some(target) = target.as_deref()
        && target != "all"
    {
        let port = daemon::port_of(target).to_string();
        let info = match daemon::ipc_ping(&port).await {
            Ok(info) => info,
            Err(_) => {
                println!("端口 {port} 上没有运行中的 aProxy 实例。");
                println!("（若该端口被其他程序占用，与本工具无关）");
                std::process::exit(1);
            }
        };
        stop_instance(&info).await;
        return;
    }

    let instances = daemon::list_instances().await;
    match target.as_deref() {
        Some("all") => {
            if instances.is_empty() {
                println!("没有运行中的 aProxy 实例。");
                return;
            }
            for info in &instances {
                stop_instance(info).await;
            }
        }
        _ => match instances.len() {
            0 => println!("没有运行中的 aProxy 实例。"),
            1 => stop_instance(&instances[0]).await,
            n => {
                eprintln!("有 {n} 个实例在运行，必须指定端口号或 all：");
                for info in &instances {
                    eprintln!("  aproxy stop {}", daemon::port_of(&info.listen_addr));
                }
                std::process::exit(1);
            }
        },
    }
}

/// 停止单个实例：发 IPC shutdown，轮询确认退出（宽限 10 秒 + 余量）。
async fn stop_instance(info: &daemon::InstanceInfo) {
    let port = daemon::port_of(&info.listen_addr).to_string();
    match daemon::ipc_request(&port, &daemon::IpcRequest::Shutdown).await {
        Ok(_) => {
            if daemon::wait_until_gone(&port, std::time::Duration::from_secs(12)).await {
                println!("已停止 pid {}（端口 {}）", info.pid, port);
            } else {
                println!(
                    "pid {} 已收到停止请求但尚未退出，可用 aproxy status 稍后确认，或 taskkill /PID {} /F 强制结束",
                    info.pid, info.pid
                );
            }
        }
        Err(e) => println!("pid {} 无响应（可能已停止）: {}", info.pid, e),
    }
}

/// `aproxy restore`：一键恢复先前非正常退出（崩溃/断电/系统重启）的实例。
///
/// 依据 run/<端口>.restore 恢复记录（守护 bind 成功时写入、优雅退出时删除）。
/// 空清单时静默成功而非报错——本命令的典型用法是配置为开机自启，重启后
/// 「之前全关了」是正常状态而非故障。幂等：已在运行的实例跳过，可重复执行。
async fn handle_restore_cmd() {
    let entries = daemon::list_restore_entries();
    if entries.is_empty() {
        println!("没有需要恢复的实例。");
        return;
    }
    let exe = std::env::current_exe().expect("无法定位自身可执行文件");
    for entry in entries {
        // 幂等：记录存在但实例已被手工拉起（或同端口重启过）时，不重复启动
        if daemon::ipc_ping(&entry.port).await.is_ok() {
            println!("端口 {} 已在运行，跳过。", entry.port);
            continue;
        }
        // 配置文件已被删除的记录无法忠实恢复，清理之（下次 restore 不再重试）
        let cfg_arg = entry
            .args
            .iter()
            .position(|a| a == "--config")
            .and_then(|i| entry.args.get(i + 1));
        if let Some(cfg) = cfg_arg
            && !std::path::Path::new(cfg).exists()
        {
            println!(
                "端口 {} 的配置文件已不存在（{}），跳过并清理该记录。",
                entry.port, cfg
            );
            daemon::remove_restore_file(&entry.port);
            continue;
        }
        let mut args = entry.args.clone();
        args.push("--daemon-child".to_string());
        match daemon::spawn_detached(&exe, &args) {
            Ok(pid) => {
                // 与 start 相同的就绪判定：IPC ping 通才算恢复成功（轮询 8 秒，
                // 冷启动受 Defender 扫描等影响可能偏慢）
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
                loop {
                    if daemon::ipc_ping(&entry.port).await.is_ok() {
                        println!("已恢复 pid {pid}（端口 {}）", entry.port);
                        break;
                    }
                    if std::time::Instant::now() > deadline {
                        eprintln!(
                            "端口 {} 的实例（pid {pid}）未在预期时间内就绪，原因通常记录在 {}",
                            entry.port,
                            daemon::logs_dir().join("startup.log").display()
                        );
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            }
            Err(e) => eprintln!("端口 {} 恢复失败: {e}", entry.port),
        }
    }
}

/// `aproxy alias add/remove/list`：别名管理，存于 settings.json（内部配置，
/// 唯一；toml 配置可多份平行并存）。
fn handle_alias_cmd(cmd: AliasCmd) {
    match cmd {
        AliasCmd::Add { name, path } => {
            if let Err(e) = settings::validate_alias_name(&name) {
                eprintln!("别名无效: {e}");
                std::process::exit(1);
            }
            let path = match path {
                Some(p) => settings::expand_path(&p),
                None => config::config_path(),
            };
            if !path.exists() {
                eprintln!("配置文件不存在: {}", path.display());
                std::process::exit(1);
            }
            let mut s = settings::load();
            let replacing = s.aliases.contains_key(&name);
            s.aliases.insert(name.clone(), path.display().to_string());
            match settings::save(&s) {
                Ok(()) => {
                    if replacing {
                        println!("已更新别名 {} -> {}", name, path.display());
                    } else {
                        println!("已添加别名 {} -> {}", name, path.display());
                    }
                    println!("启动: aproxy start {name}    停止: aproxy stop {name}");
                }
                Err(e) => {
                    eprintln!("保存 settings.json 失败: {e}");
                    std::process::exit(1);
                }
            }
        }
        AliasCmd::Remove { name } => {
            let mut s = settings::load();
            match s.aliases.remove(&name) {
                Some(path) => match settings::save(&s) {
                    Ok(()) => println!("已删除别名 {name}（原指向 {path}）"),
                    Err(e) => {
                        eprintln!("保存 settings.json 失败: {e}");
                        std::process::exit(1);
                    }
                },
                None => {
                    eprintln!("别名 {name} 不存在。用 aproxy alias list 查看。");
                    std::process::exit(1);
                }
            }
        }
        AliasCmd::List => {
            let s = settings::load();
            if s.aliases.is_empty() {
                println!("暂无别名。添加: aproxy alias add <名称> <config.toml 路径>");
                return;
            }
            println!("别名 ({}):", s.aliases.len());
            let mut names: Vec<_> = s.aliases.iter().collect();
            names.sort();
            for (name, path) in names {
                println!("  {name} -> {path}");
            }
        }
    }
}

/// `aproxy doctor`：配置体检。settings.json 的 error 级检查每次软件运行都会
/// 执行；别名配置深入审查与目录 toml 扫描（warning 级）只在此处执行。
async fn handle_doctor_cmd() {
    println!("aProxy 配置体检");
    println!("════════════════");
    let report = doctor::run(&settings::settings_path());
    if report.is_clean() {
        println!("全部配置正常，未发现问题。");
        return;
    }
    report.print();
    if report.error_count() > 0 {
        println!(
            "\nerror 级问题会影响按名字的 start/stop 等操作，建议先修复；warning 级仅供参考。"
        );
    }
}

/// `aproxy find [查询] [--aliased|--unaliased] [--port 端口]`：
/// 从配置目录列表发现全部配置并列出。
fn handle_find_cmd(aliased: bool, unaliased: bool, query: Option<String>, port: Option<u16>) {
    let settings = settings::load();
    let filter = if aliased {
        find::AliasFilter::Aliased
    } else if unaliased {
        find::AliasFilter::Unaliased
    } else {
        find::AliasFilter::All
    };
    let mut items: Vec<_> = find::discover(&settings)
        .into_iter()
        .filter(|d| match filter {
            find::AliasFilter::All => true,
            find::AliasFilter::Aliased => !d.aliases.is_empty(),
            find::AliasFilter::Unaliased => d.aliases.is_empty(),
        })
        .filter(|d| query.as_deref().is_none_or(|q| d.matches_query(q)))
        .filter(|d| port.is_none_or(|p| d.matches_port(p)))
        .collect();
    // --port 过滤下不可解析的配置必然不匹配，无需特判；空结果统一提示
    if let Some(p) = port {
        items.retain(|d| d.matches_port(p));
    }
    find::print_list(&items);
}

/// `aproxy logs [PORT]`：连接到运行中的实例并实时输出其守护日志。
/// 语义与 stop 一致：单实例可省略端口；多实例必须指定；不支持 all
/// （一次只能连接一个实例，传入 all 视为端口解析失败）。
async fn handle_logs_cmd(target: Option<String>) {
    if target.as_deref() == Some("all") {
        eprintln!("aproxy logs 不支持 all：一次只能连接一个实例，请指定端口号。");
        std::process::exit(1);
    }
    // 指定端口时不依赖注册表：直接按端口 IPC 定位（与 stop 相同）
    let info = if let Some(target) = target.as_deref() {
        let port = daemon::port_of(target).to_string();
        match daemon::ipc_ping(&port).await {
            Ok(info) => info,
            Err(_) => {
                println!("端口 {port} 上没有运行中的 aProxy 实例。");
                std::process::exit(1);
            }
        }
    } else {
        let instances = daemon::list_instances().await;
        match instances.len() {
            0 => {
                println!("没有运行中的 aProxy 实例。");
                return;
            }
            1 => instances.into_iter().next().unwrap(),
            n => {
                eprintln!("有 {n} 个实例在运行，必须指定端口号（一次只能连接一个）：");
                for info in &instances {
                    eprintln!("  aproxy logs {}", daemon::port_of(&info.listen_addr));
                }
                std::process::exit(1);
            }
        }
    };

    let port = daemon::port_of(&info.listen_addr).to_string();
    let path = daemon::logs_dir().join(format!("{port}.log"));
    println!("正在连接 pid {}（端口 {}）——Ctrl+C 退出。", info.pid, port);
    println!("  日志文件: {}", path.display());
    println!("────────── 最近日志 ──────────");

    if let Err(e) = follow_log_file(&path, &port).await {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

/// 跟踪日志文件：先输出末尾若干行（tail -f 语义），随后轮询增量输出；
/// 实例退出（IPC ping 失败，内部已含 3 次重试判死）或 Ctrl+C 结束。
/// 读文件是同步小量轮询（200ms），期间穿插 IPC 探活（约 1 秒一次）。
async fn follow_log_file(path: &std::path::Path, port: &str) -> Result<(), String> {
    use std::io::{Read, Seek, SeekFrom, Write};

    const POLL_MS: u64 = 200;
    const TAIL_WINDOW: u64 = 8 * 1024; // 首屏最多回读 8KB
    const TAIL_LINES: usize = 30; // 首屏最多显示 30 行
    const PING_EVERY: u32 = 5; // 每 5 轮（约 1 秒）探活一次

    // 等待日志文件出现（实例刚启动时 IPC 已通、日志可能尚未落盘）。限时 5 秒：
    // 守护实例的日志文件在进程启动最先创建，5 秒未出现说明它永远不会有——
    // 典型是 --foreground 实例（日志走控制台，但同样注册注册表、ping 可通），
    // 不能在这里无限等待
    let wait_deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if path.exists() {
            break;
        }
        if daemon::ipc_ping(port).await.is_err() {
            return Err("实例已退出，日志文件未生成。".to_string());
        }
        if std::time::Instant::now() > wait_deadline {
            return Err(format!(
                "日志文件迟迟未生成: {}（该实例可能以 --foreground 运行，日志输出在它的控制台）",
                path.display()
            ));
        }
        tokio::time::sleep(Duration::from_millis(POLL_MS)).await;
    }

    // 首屏：回读文件末尾 TAIL_WINDOW 字节，丢弃可能被截断的残行，只显示最后 TAIL_LINES 行
    let mut file = std::fs::File::open(path).map_err(|e| format!("无法打开日志文件: {e}"))?;
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(TAIL_WINDOW);
    file.seek(SeekFrom::Start(start))
        .map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).map_err(|e| e.to_string())?;
    if start > 0 {
        // 丢弃残行（回读起点未必落在行边界；找不到 \n 则整个窗口都是残行，全部丢弃）
        if let Some(nl) = buf.iter().position(|&b| b == b'\n') {
            buf.drain(..=nl);
        } else {
            buf.clear();
        }
    }
    let mut lines: Vec<&[u8]> = buf.split(|&b| b == b'\n').collect();
    if lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop(); // 文件以 \n 结尾时 split 会产生末尾空段，丢弃
    }
    let skip = lines.len().saturating_sub(TAIL_LINES);
    for line in &lines[skip..] {
        println!("{}", String::from_utf8_lossy(line));
    }
    let mut pos = len;
    let mut pending: Vec<u8> = Vec::new();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    let mut polls = 0u32;
    loop {
        tokio::time::sleep(Duration::from_millis(POLL_MS)).await;
        polls += 1;
        if polls.is_multiple_of(PING_EVERY) && daemon::ipc_ping(port).await.is_err() {
            // 实例已退出（ping 内部含判死重试）：冲掉残余半行后结束
            if !pending.is_empty() {
                let _ = out.write_all(&pending);
                let _ = out.flush();
            }
            println!();
            println!("实例已停止，日志跟踪结束。");
            return Ok(());
        }
        let cur_len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(pos);
        if cur_len < pos {
            // 文件被截断（守护启动时 >2MiB 清空）：从头重新跟踪
            println!("── 日志文件已被截断，重新从头跟踪 ──");
            pos = 0;
            pending.clear();
        }
        if cur_len > pos {
            let Ok(mut f) = std::fs::File::open(path) else {
                continue;
            };
            if f.seek(SeekFrom::Start(pos)).is_err() {
                continue;
            }
            let mut chunk = Vec::with_capacity((cur_len - pos) as usize);
            if f.read_to_end(&mut chunk).is_err() {
                continue;
            }
            pos += chunk.len() as u64;
            pending.extend_from_slice(&chunk);
            // 只输出完整行：半行留在 pending（UTF-8 多字节字符跨轮也不会撕裂）
            if let Some(idx) = pending.iter().rposition(|&b| b == b'\n') {
                let _ = out.write_all(&pending[..=idx]);
                let _ = out.flush();
                pending.drain(..=idx);
            }
        }
    }
}

/// 配置错误报告：前台/父进程走 stderr；守护子进程无控制台，错误写
/// startup.log——就绪等待超时的提示虽指向端口日志，但配置错误发生在
/// 日志初始化之前，只有这里能留下线索。logs 目录不可写时退回 config 目录，
/// 尽量留下线索，两级都失败才放弃。
fn report_config_error(msg: &str, daemon_child: bool) {
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

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 日志时间戳（无 chrono 依赖，仅用于 startup.log 行前缀）
fn chrono_like_timestamp() -> String {
    // 简单格式化：秒级 Unix 时间足以定位启动失败的顺序
    format!("unix+{}s", now_unix())
}

fn humanize_uptime(started_at: u64) -> String {
    let secs = now_unix().saturating_sub(started_at);
    if secs < 60 {
        format!("{secs} 秒")
    } else if secs < 3600 {
        format!("{} 分 {} 秒", secs / 60, secs % 60)
    } else if secs < 86400 {
        format!("{} 小时 {} 分", secs / 3600, (secs % 3600) / 60)
    } else {
        format!("{} 天 {} 小时", secs / 86400, (secs % 86400) / 3600)
    }
}

fn init_stdout_logging() {
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
fn init_daemon_logging(listen_addr: &str) {
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

/// 密钥类值的展示打码：保留前 6 个字符 + "***"（按字符截断，避免多字节字符在字节边界 panic）。
fn mask_secret(s: &str) -> String {
    let prefix: String = s.chars().take(6).collect();
    format!("{prefix}***")
}

/// 对代理 URL 中的密码打码（`http://user:***@host:port`），仅用于展示，绝不输出真实密码。
fn mask_proxy_url(raw: &str) -> String {
    // 解析失败时原样返回会泄露内嵌密码，只回显已隐去的安全占位
    let Ok(url) = url::Url::parse(raw) else {
        return "<无法解析的代理配置，已隐去>".to_string();
    };
    let user = url.username();
    if user.is_empty() && url.password().is_none() {
        return raw.to_string();
    }
    let host = url.host_str().unwrap_or("");
    let port = url.port().map(|p| format!(":{p}")).unwrap_or_default();
    // 仅用户名无密码时不渲染 ":***@"，避免让人误以为配置了密码
    let auth = if url.password().is_some() {
        format!("{user}:***@")
    } else {
        format!("{user}@")
    };
    let mut masked = format!("{}://{}{}{}", url.scheme(), auth, host, port);
    if let Some(q) = url.query() {
        masked.push('?');
        masked.push_str(q);
    }
    masked
}

fn parse_kv(s: &str) -> Option<(String, String)> {
    let (k, v) = s.split_once('=')?;
    let k = k.trim();
    let v = v.trim();
    if k.is_empty() {
        return None;
    }
    Some((k.to_string(), v.to_string()))
}

/// base_url 展示打码：下沉到 config::mask_base_url（find/doctor 等 lib 侧共用）。
use config::mask_base_url;

/// 展示前守卫：配置文件存在但解析失败时 load() 会静默回退默认值，`--show` 打印的
/// 「当前配置」并非用户文件的真实内容——至少要警告，避免误导排障。
fn warn_if_config_broken(path: &std::path::Path) {
    if let Ok(content) = std::fs::read_to_string(path)
        && let Err(e) = toml::from_str::<aproxy::config::Config>(&content)
    {
        eprintln!("警告: 配置文件解析失败，以下展示的是回退默认值而非文件内容");
        eprintln!("解析错误: {e}");
    }
}

fn handle_config_cmd(path: PathBuf, args: ConfigArgs) {
    let ConfigArgs {
        baseurl,
        listen,
        api_key,
        extra_headers,
        override_headers,
        keepalive_secs,
        proxy,
        proxy_username,
        proxy_password,
        clear_api_key,
        clear_headers,
        clear_proxy,
        set_default,
        clear_default,
        show,
    } = args;

    // 默认配置文件指向存于 settings.json（内部配置），与 config.toml 内容无关，
    // 独立处理后再进入 toml 内容编辑流程
    if set_default.is_some() || clear_default {
        if set_default.is_some() && clear_default {
            eprintln!("--set-default 与 --clear-default 不能同时使用");
            std::process::exit(1);
        }
        let mut s = settings::load();
        if let Some(p) = set_default {
            let abs = settings::expand_path(&p);
            if !abs.is_file() {
                eprintln!("配置文件不存在: {}", abs.display());
                std::process::exit(1);
            }
            s.default_config = Some(abs.display().to_string());
            match settings::save(&s) {
                Ok(()) => println!("已把 {} 设为默认配置文件", abs.display()),
                Err(e) => {
                    eprintln!("保存 settings.json 失败: {e}");
                    std::process::exit(1);
                }
            }
        } else {
            s.default_config = None;
            match settings::save(&s) {
                Ok(()) => println!("已取消默认配置文件设置（恢复用 ~/.aproxy/config.toml）"),
                Err(e) => {
                    eprintln!("保存 settings.json 失败: {e}");
                    std::process::exit(1);
                }
            }
        }
        return;
    }

    let mut cfg = config::load_from(&path);

    let mut changed = false;
    if let Some(u) = baseurl {
        cfg.base_url = u.trim_end_matches('/').to_string();
        changed = true;
    }
    if let Some(l) = listen {
        cfg.listen_addr = l;
        changed = true;
    }
    if clear_api_key {
        cfg.api_key = None;
        changed = true;
    } else if let Some(k) = api_key {
        if k.trim().is_empty() {
            eprintln!("api_key 不能为空（用 --clear-api-key 清空）");
            std::process::exit(1);
        }
        cfg.api_key = Some(k.trim().to_string());
        changed = true;
    }
    if clear_headers {
        cfg.extra_headers.clear();
        cfg.override_headers.clear();
        changed = true;
    }
    for kv in extra_headers {
        let Some((k, v)) = parse_kv(&kv) else {
            eprintln!("extra-header 格式错误，应为 key=value，得到: {kv}");
            std::process::exit(1);
        };
        cfg.extra_headers.insert(k, v);
        changed = true;
    }
    for kv in override_headers {
        let Some((k, v)) = parse_kv(&kv) else {
            eprintln!("override-header 格式错误，应为 key=value，得到: {kv}");
            std::process::exit(1);
        };
        cfg.override_headers.insert(k, v);
        changed = true;
    }
    if let Some(s) = keepalive_secs {
        cfg.keepalive_interval_secs = s;
        changed = true;
    }
    if clear_proxy {
        cfg.proxy = None;
        cfg.proxy_username = None;
        cfg.proxy_password = None;
        changed = true;
    } else if let Some(p) = proxy {
        if p.trim().is_empty() {
            eprintln!("proxy 不能为空（用 --clear-proxy 清空）");
            std::process::exit(1);
        }
        cfg.proxy = Some(p.trim().to_string());
        changed = true;
    }
    if let Some(u) = proxy_username {
        if u.trim().is_empty() {
            eprintln!("proxy-username 不能为空（用 --clear-proxy 清空）");
            std::process::exit(1);
        }
        cfg.proxy_username = Some(u.trim().to_string());
        changed = true;
    }
    if let Some(p) = proxy_password {
        if p.trim().is_empty() {
            eprintln!("proxy-password 不能为空（用 --clear-proxy 清空）");
            std::process::exit(1);
        }
        cfg.proxy_password = Some(p.trim().to_string());
        changed = true;
    }

    if changed {
        // 保存前守卫：配置文件存在但解析失败时 load() 会静默回退默认值，
        // 直接保存会把用户手写（或损坏）的配置整个覆盖掉——拒绝修改并报告解析错误。
        if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(content) => {
                    if let Err(e) = toml::from_str::<aproxy::config::Config>(&content) {
                        eprintln!(
                            "现有配置文件解析失败，拒绝覆盖（请先修复或删除 {}）",
                            path.display()
                        );
                        eprintln!("解析错误: {e}");
                        std::process::exit(1);
                    }
                }
                Err(e) => {
                    eprintln!("读取配置文件失败: {e}");
                    std::process::exit(1);
                }
            }
        }
        cfg = cfg.normalized();
        // 保存前校验已设置的 base_url 格式（允许为空——首次配置可分多次完成，中间态
        // 合法）；否则 query 形式的 base_url 会被存盘，直到下次启动才报错
        if !cfg.base_url.is_empty()
            && let Err(msg) = cfg.validate_base_url()
        {
            eprintln!("base_url 无效，未保存: {msg}");
            std::process::exit(1);
        }
        match config::save_to(&path, &cfg) {
            Ok(()) => println!("已保存配置到 {}", path.display()),
            Err(e) => {
                eprintln!("保存配置失败: {e}");
                std::process::exit(1);
            }
        }
    }

    if show || !changed {
        // 重新加载以展示最终值
        warn_if_config_broken(&path);
        let cfg = config::load_from(&path).normalized();
        println!("配置文件: {}", path.display());
        println!("base_url     = \"{}\"", mask_base_url(&cfg.base_url));
        println!("listen_addr  = \"{}\"", cfg.listen_addr);
        println!(
            "api_key      = {}",
            cfg.api_key
                .as_deref()
                .map(mask_secret)
                .map(|s| format!("\"{s}\""))
                .unwrap_or_else(|| "(未设置)".to_string())
        );
        println!("keepalive_interval_secs = {}", cfg.keepalive_interval_secs);
        // 0 表示所有重试零延迟，语义特殊，提示出来
        if cfg.max_retry_backoff_secs == 0 {
            println!("max_retry_backoff_secs = 0（所有重试零延迟）");
        } else {
            println!("max_retry_backoff_secs = {}", cfg.max_retry_backoff_secs);
        }
        println!("spool_limit_mb          = {}", cfg.spool_limit_mb);
        // 0 表示不设限，语义特殊，提示出来
        if cfg.connect_timeout_secs == 0 {
            println!("connect_timeout_secs    = 0（不设限）");
        } else {
            println!("connect_timeout_secs    = {}", cfg.connect_timeout_secs);
        }
        if cfg.read_timeout_secs == 0 {
            println!("read_timeout_secs       = 0（不设限）");
        } else {
            println!("read_timeout_secs       = {}", cfg.read_timeout_secs);
        }
        println!(
            "proxy          = {}",
            cfg.proxy
                .as_deref()
                .map(mask_proxy_url)
                .unwrap_or_else(|| "(未设置)".to_string())
        );
        println!(
            "proxy_username = {}",
            cfg.proxy_username.as_deref().unwrap_or("(未设置)")
        );
        println!(
            "proxy_password = {}",
            cfg.proxy_password
                .as_deref()
                .map(|_| "***".to_string())
                .unwrap_or_else(|| "(未设置)".to_string())
        );
        if cfg.extra_headers.is_empty() {
            println!("extra_headers    = (空)");
        } else {
            println!("extra_headers:");
            for (k, v) in &cfg.extra_headers {
                // 头值常含完整鉴权令牌，与 api_key 一致做截断打码
                println!("  {k} = \"{}\"", mask_secret(v));
            }
        }
        if cfg.override_headers.is_empty() {
            println!("override_headers = (空)");
        } else {
            println!("override_headers:");
            for (k, v) in &cfg.override_headers {
                println!("  {k} = \"{}\"", mask_secret(v));
            }
        }
        if cfg.base_url.is_empty() {
            println!();
            println!("提示: base_url 为空，请设置:");
            println!("  aproxy config --baseurl https://api.anthropic.com");
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_secret_keeps_prefix_and_masks_rest() {
        // 保留前 6 个字符
        assert_eq!(mask_secret("sk-ant-api03-abcdef"), "sk-ant***");
        assert_eq!(mask_secret("abc"), "abc***");
        assert_eq!(mask_secret(""), "***");
    }

    #[test]
    fn mask_secret_multibyte_no_panic() {
        // 按 char 截断，多字节字符不会在字节边界 panic
        assert_eq!(mask_secret("你好世界，测试"), "你好世界，测***");
    }

    #[test]
    fn mask_proxy_url_variants() {
        // 无凭据原样返回
        assert_eq!(
            mask_proxy_url("http://127.0.0.1:7890"),
            "http://127.0.0.1:7890"
        );
        // 有密码打码
        assert_eq!(
            mask_proxy_url("http://alice:secret@127.0.0.1:7890"),
            "http://alice:***@127.0.0.1:7890"
        );
        // 仅用户名不加 ":***@"
        assert_eq!(
            mask_proxy_url("socks5://alice@127.0.0.1:1080"),
            "socks5://alice@127.0.0.1:1080"
        );
        // 解析失败回安全占位而非原文（原文可能含密码）
        assert_eq!(mask_proxy_url("not a url"), "<无法解析的代理配置，已隐去>");
        // query 保留
        assert_eq!(
            mask_proxy_url("http://alice:pw@h:1?p=x"),
            "http://alice:***@h:1?p=x"
        );
    }

    #[test]
    fn mask_base_url_variants() {
        // 无凭据原样返回（绝大多数情况）
        assert_eq!(
            mask_base_url("https://api.anthropic.com/v1"),
            "https://api.anthropic.com/v1"
        );
        // 内嵌密码打码，路径保留
        assert_eq!(
            mask_base_url("https://alice:secret@example.com/anthropic"),
            "https://alice:***@example.com/anthropic"
        );
        // 解析失败原样返回（base_url 已过 validate，此处不会发生）
        assert_eq!(mask_base_url("::bad::"), "::bad::");
    }

    #[test]
    fn parse_kv_variants() {
        assert_eq!(
            parse_kv("x-key = some value"),
            Some(("x-key".into(), "some value".into()))
        );
        // 值中允许 '='：只按第一个 '=' 切分
        assert_eq!(
            parse_kv("authorization=Bearer a=b"),
            Some(("authorization".into(), "Bearer a=b".into()))
        );
        assert_eq!(parse_kv("no-equals"), None);
        assert_eq!(parse_kv("  =value"), None);
    }
}
