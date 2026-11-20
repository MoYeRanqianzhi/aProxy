//! aProxy — 本地 API 代理，无限重试保障 agent 工作流不中断。
//!
//! 本文件只是进程入口：控制台代码页设置、配置文件定位、日志初始化与子命令分派。
//! CLI 定义在库的 `aproxy::cli`，各子命令处理在 `commands/`，服务主循环在 `server.rs`，
//! 共用小工具在 `util.rs`。
//!
//! 入口的顺序：先兑现永久兼容契约 C5 的单独 `--version`，再解析参数、在建 tokio 运行时
//! 之前分派 `desktop`（桌面界面的事件循环要占住主线程，而且不能处在异步上下文里），其余
//! 命令都在手建的多线程运行时里 `block_on` 执行（与原先的 `#[tokio::main]` 等价）。

#[cfg(windows)]
use windows_sys::Win32::System::Console::{GetConsoleOutputCP, SetConsoleOutputCP};

mod commands;
mod server;
mod util;

use clap::Parser;
use std::io::Write as _;

use aproxy::cli::{Cli, Commands};

use aproxy::settings;

fn main() {
    // 永久兼容契约 C5（见 aproxy::VERSION_LINE）：安装器以 `<二进制> --version` 认
    // 版本，这一行必须先于一切副作用输出——不切控制台代码页、不读 settings 与
    // home、不建运行时、不拉起续作，以后解析参数之前要读的任何东西（例如按 settings
    // 判定界面语言）也都排在它后面。
    if version_requested() {
        print_version_line();
        return;
    }

    // Windows 控制台默认代码页（如 936/GBK）会把 Rust 输出的 UTF-8 中文显示为
    // 乱码——status/stop/logs 等所有面向用户的输出都是中文。切换到 UTF-8
    // （65001）；无控制台的守护子进程上调用失败被忽略，无副作用。
    #[cfg(windows)]
    unsafe {
        if GetConsoleOutputCP() != 65001 {
            SetConsoleOutputCP(65001);
        }
    }

    let cli = Cli::parse();
    // `-V`/`--version` 与别的参数写在一起（`aproxy -V start`）时走到这里，输出同一行。
    // 这种写法要靠 clap 才分得清 `-V` 是版本参数还是别的参数的取值，所以排在解析之后；
    // 安装器只用单独的 `--version`，由上面第一句兑现「先于一切副作用」
    if cli.version {
        print_version_line();
        return;
    }

    // 桌面界面在建 tokio 运行时之前分派：Tauri 的事件循环要占住主线程（macOS 硬性要求），
    // 而在 block_on 里的主线程处于异步上下文，Tauri 回调里任何 block_on 都会 panic。它自己
    // 建运行时、交给 Tauri 后再进事件循环，见 commands::desktop。不读配置与 settings，界面
    // 自己按 home 寻址
    if cli.command == Some(Commands::Desktop) {
        commands::desktop::handle_desktop_cmd();
    }

    // 内部入口 `__serve`、`__watchdog` 的 home 由命令行的 `--home` 定：拉起方操作的是哪个
    // home，这个进程就属于哪个 home。写进本进程的 APROXY_HOME，之后一切按 home 寻址的
    // 代码（以及它再拉起的子进程）看到的都是它
    if let Some(Commands::Serve { home, .. } | Commands::Watchdog { home }) = &cli.command {
        // SAFETY: 此刻进程里只有主线程——运行时还没建，也没有拉起任何线程
        unsafe { std::env::set_var("APROXY_HOME", home) };
    }

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("无法建立 tokio 运行时")
        .block_on(run(cli));
}

/// home 还未被接受，不能查台账或设置；按已解析的命令构造它约定的 JSON 错误。
fn invalid_home_output(
    command: Option<&Commands>,
    error: &str,
    value: &str,
) -> (Option<serde_json::Value>, i32) {
    match command {
        Some(Commands::Status { json: true, .. }) => (
            Some(
                serde_json::json!({"error":{"code":"home_invalid","message":"APROXY_HOME does not satisfy the home rules.","details":{"value":value}}}),
            ),
            i32::from(aproxy_ipc::code::EXIT_USAGE),
        ),
        Some(Commands::Install(args)) if args.json => {
            let error = serde_json::json!({"code":"install_failed","message":error,"details":{}});
            let body = if args.check {
                serde_json::json!({"error":error})
            } else {
                serde_json::json!({"event":"result","outcome":"failed","version":null,"error":error})
            };
            (Some(body), 1)
        }
        _ => (None, 1),
    }
}

/// 除 `--version` 与 `desktop` 之外的一切命令，在 tokio 运行时里执行。
async fn run(cli: Cli) {
    // 强制安装（永久兼容契约 C7、C8，见 aproxy::install::force）在这里分流，排在定位
    // 配置文件、settings 检查与拉起续作之前：它不读配置与 settings.json——要修的
    // 可能正是它们，损坏的 settings.json 不能挡住修复它的手段；它也不能拉起续作，
    // 续作要推进的那个安装正是它要清掉的
    if let Some(Commands::Install(args)) = &cli.command
        && args.force
    {
        server::init_force_logging();
        commands::install::handle_force_cmd(args).await;
    }

    // 永久兼容契约 C1：APROXY_HOME 设了却不是绝对路径（或者连用户目录都取不到）时，一切
    // 要读 home 的命令都拒绝运行——相对路径随工作目录解析成不同的 home，在这里报错比在
    // 错误的目录里读写强。`--version`、`--help` 不读 home，在上面已经结束；强制安装在
    // 上面自己报告（退出码 10）
    if let Err(e) = aproxy_ipc::home::resolve_home() {
        let value = std::env::var_os(aproxy_ipc::home::HOME_ENV).unwrap_or_default();
        let (document, exit) =
            invalid_home_output(cli.command.as_ref(), &e, &value.to_string_lossy());
        if let Some(doc) = document {
            commands::print_doc(doc);
        }
        let _ = writeln!(std::io::stderr(), "[ERROR] {e}");
        std::process::exit(exit);
    }

    // 后台实例：按交接文件里的拉起规格承载服务，日志由它自己按规格初始化。排在
    // settings 检查与续作拉起之前：这两件事都是给坐在终端前的人准备的，实例自己在解析
    // 配置时检查 settings，结果写进 startup.log
    if let Some(Commands::Serve { spec, .. }) = &cli.command {
        commands::start::handle_serve_cmd(spec).await;
    }

    // 前台实例没有日志文件，控制台就是它的日志；分离拉起的看护者沿用这一模式，输出不继承。
    // 普通命令的 stdout 留给文字或 JSON 文档，诊断日志写 stderr，便于单独捕获。
    if matches!(&cli.command, Some(Commands::Start(args)) if args.foreground)
        || matches!(cli.command, Some(Commands::Watchdog { .. }))
    {
        server::init_foreground_logging();
    } else {
        server::init_cli_logging();
    }

    // 看护者：进入看护主循环（claim 接管 + 收养 + 健康扫描 + 重拉）。
    if let Some(Commands::Watchdog { .. }) = &cli.command {
        // 进程台账（永久兼容契约 C3）：看护者在拉起任何子进程（重拉实例、续作安装）之前
        // 登记，强制安装据此先于实例结束它，免得它把刚结束的实例当成崩溃重新拉起
        let _ledger = util::register_in_ledger(
            &aproxy_ipc::home::home(),
            aproxy_ipc::ledger::ROLE_WATCHDOG,
            None,
            None,
            None,
        );
        let cfg = aproxy::watchdog::WatchdogConfig::from_settings();
        aproxy::watchdog::serve(cfg).await;
        // 主循环返回 = 本看护者到此为止（让位、选举落败或闲置自灭）。硬退而不是从 main
        // 返回：运行时析构会等后台任务，持有无限循环任务的进程会退不出去
        util::exit(0);
    }

    // settings.json 的检查，每次运行都做。文件无效时（不认识的键、类型不对、schema 更高……）：
    // - `settings` 的各个子命令与 doctor 照常运行：它们就是用来查看与修复它的，自己会报告；
    // - stop、status、install 照常运行、用内置默认，把错误作为告警打印：任何时候都要能停
    //   实例、看状态、装上能修好问题的版本（别名这时不可用，按端口或 all 寻址）；
    // - 其余命令以退出码 3 结束，说清怎样修——带着一份没生效的设置去做事，比不做更糟。
    let settings_path = settings::settings_path();
    let loaded = settings::read(&settings_path);
    let reports_itself = matches!(
        cli.command,
        Some(Commands::Doctor { .. } | Commands::Settings { .. })
    );
    if !loaded.is_valid() && !reports_itself {
        let tolerated = matches!(
            cli.command,
            Some(Commands::Status { .. } | Commands::Stop { .. } | Commands::Install(_))
        );
        // 诊断接收端可能已经关闭；它不能让 stdout 上独立的命令结果也丢失。
        let level = if tolerated { "WARN" } else { "ERROR" };
        let _ = writeln!(std::io::stderr(), "[{level}] settings.json 有错误:");
        for d in loaded.diagnostics.iter().filter(|d| d.is_error()) {
            let _ = writeln!(
                std::io::stderr(),
                "[{level}]   {}",
                aproxy::ops::start::render_diagnostic(d)
            );
        }
        if tolerated {
            let _ = writeln!(
                std::io::stderr(),
                "[{level}] 本次按内置默认运行（别名不可用，请按端口或 all 指定实例）。用 aproxy settings show 查看、aproxy settings unset <键> 修复"
            );
        } else {
            commands::settings::print_repair_hints(&settings_path);
            util::exit(aproxy_ipc::code::EXIT_CONFIG as i32);
        }
    }
    // 指向不存在的文件的别名：start/stop 按这个名字操作会失败，每次运行都提醒。doctor 会
    // 汇总输出，这里跳过以免重复
    if loaded.is_valid() && !matches!(cli.command, Some(Commands::Doctor { .. })) {
        for e in settings::missing_alias_targets(&loaded.settings) {
            let _ = writeln!(std::io::stderr(), "[ERROR] {e}");
        }
    }

    // 只有要改动实例或设置的请求才顺带恢复安装；观察现场的命令不应改变它。
    // halted 表示原因不会自行消失，必须让用户排除原因后显式安装。
    if cli.command.as_ref().is_some_and(resumes_install)
        && aproxy::install::state::load()
            .is_some_and(|s| !(s.phase == aproxy::install::state::InstallPhase::Failed && s.halted))
        && let Ok(exe) = std::env::current_exe()
    {
        let _ = aproxy::daemon::spawn_detached(
            &exe,
            &["install".to_string(), "--continue".to_string()],
        );
    }

    // 子命令分派
    match cli.command {
        Some(Commands::Status { idle, busy, json }) => {
            commands::status::handle_status_cmd(idle, busy, json).await
        }
        Some(Commands::Start(args)) => commands::start::handle_start_cmd(args).await,
        Some(Commands::Stop {
            target,
            threshold,
            force,
            json,
        }) => commands::stop::handle_stop_cmd(target, threshold, force, json).await,
        Some(Commands::Restart {
            target,
            threshold,
            force,
            json,
        }) => commands::restart::handle_restart_cmd(target, threshold, force, json).await,
        Some(Commands::Logs {
            target,
            lines,
            follow,
        }) => commands::logs::handle_logs_cmd(target, lines, follow).await,
        Some(Commands::Restore { json }) => commands::restore::handle_restore_cmd(json).await,
        Some(Commands::Alias { cmd }) => commands::alias::handle_alias_cmd(cmd),
        Some(Commands::Doctor { json }) => commands::doctor::handle_doctor_cmd(json).await,
        Some(Commands::Find {
            aliased,
            unaliased,
            query,
            port,
            json,
        }) => commands::find::handle_find_cmd(aliased, unaliased, query, port, json),
        Some(Commands::Config(args)) => commands::config::handle_config_cmd(args).await,
        Some(Commands::Settings { cmd }) => commands::settings::handle_settings_cmd(cmd).await,
        Some(Commands::Install(args)) => commands::install::handle_install_cmd(args).await,
        Some(Commands::Desktop) => unreachable!("desktop 在 main 里、建运行时之前已经分派"),
        Some(Commands::Serve { .. } | Commands::Watchdog { .. }) => {
            unreachable!("内部入口在子命令分派之前已经处理")
        }
        // 不带子命令的调用在参数解析时已经打印帮助并以 2 退出；走到这里的只有版本参数与
        // 别的参数写在一起的情形，上面已经处理
        None => unreachable!("没有子命令的调用在参数解析时已经结束"),
    }
}

/// 输出版本行（永久兼容契约 C5，见 aproxy::VERSION_LINE）。写失败（管道已关闭、终端已
/// 断开）一律忽略：C5 规定退出码是 0，而 `println!` 写失败会 panic、退出码变成 101。读够
/// 首行就关掉管道（`| head -n 1`）的安装器与脚本是常态，不是意外。
fn print_version_line() {
    use std::io::Write as _;
    let _ = writeln!(std::io::stdout(), "{}", aproxy::VERSION_LINE);
}

/// 命令行恰好是 `aproxy --version` 或 `aproxy -V`（安装器探测版本就是这样调用的）。
/// 只认这两种整行写法：与别的参数写在一起时交给 clap，它分得清 `-V` 是版本参数
/// 还是另一个参数的取值。
fn version_requested() -> bool {
    let mut args = std::env::args_os().skip(1);
    matches!(
        (args.next(), args.next()),
        (Some(flag), None) if flag == "--version" || flag == "-V"
    )
}

/// 按实际子命令区分读写；入口自己分派的内部进程、安装及桌面都不触发 CLI 续作。
fn resumes_install(command: &Commands) -> bool {
    use aproxy::cli::{AliasCmd, ConfigCmd, SettingsCmd};
    match command {
        Commands::Start(_)
        | Commands::Stop { .. }
        | Commands::Restart { .. }
        | Commands::Restore { .. } => true,
        Commands::Alias { cmd } => match cmd {
            AliasCmd::Add { .. } | AliasCmd::Remove { .. } => true,
            AliasCmd::List { .. } => false,
        },
        Commands::Config(args) => match args.cmd {
            ConfigCmd::Set { .. } | ConfigCmd::Unset { .. } => true,
            ConfigCmd::Show { .. } | ConfigCmd::Get { .. } => false,
        },
        Commands::Settings { cmd } => match cmd {
            SettingsCmd::Set { .. } | SettingsCmd::Unset { .. } => true,
            SettingsCmd::Show { .. } | SettingsCmd::Get { .. } => false,
        },
        Commands::Status { .. }
        | Commands::Logs { .. }
        | Commands::Find { .. }
        | Commands::Doctor { .. }
        | Commands::Install(_)
        | Commands::Desktop
        | Commands::Serve { .. }
        | Commands::Watchdog { .. } => false,
    }
}

#[cfg(test)]
mod home_output_tests {
    use super::*;
    /// 用 C1 的纯函数拒绝相对 home，不改变测试进程或任何子进程的真实 home。
    #[test]
    fn status_json_home_failure_returns_its_document_before_dispatch() {
        let value = std::ffi::OsStr::new("relative");
        let error = aproxy_ipc::home::home_from(Some(value), || unreachable!()).unwrap_err();
        let cli = Cli::try_parse_from(["aproxy", "status", "--json"]).unwrap();
        let (doc, exit) =
            invalid_home_output(cli.command.as_ref(), &error, value.to_str().unwrap());
        assert_eq!(exit, 2);
        let doc = doc.unwrap();
        assert_eq!(doc["error"]["code"], "home_invalid");
        assert_eq!(doc["error"]["details"]["value"], "relative");
    }
}
