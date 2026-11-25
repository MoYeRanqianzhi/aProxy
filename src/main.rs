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

/// 诊断不是命令结果。管道读端关闭时忽略写错误，让 stdout 上独立的文档与退出码照常交付。
macro_rules! eprintln {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        let _ = writeln!(std::io::stderr().lock(), $($arg)*);
    }};
}

#[path = "i18n/boot.rs"]
mod boot;
pub(crate) mod commands;
pub(crate) mod server;
pub(crate) mod util;

use clap::FromArgMatches;
use std::io::Write as _;

use aproxy::cli::{Cli, Commands};

use aproxy::settings;
use aproxy_ipc::i18n::{Lang, current};

pub(crate) fn main() -> std::process::ExitCode {
    // 永久兼容契约 C5（见 aproxy::VERSION_LINE）：安装器以 `<二进制> --version` 认
    // 版本，这一行必须先于一切副作用输出——不切控制台代码页、不读 settings 与
    // home、不建运行时、不拉起续作，以后解析参数之前要读的任何东西（例如按 settings
    // 判定界面语言）也都排在它后面。
    if version_requested() {
        print_version_line();
        return std::process::ExitCode::SUCCESS;
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

    if let Some(settings) = terminal_settings(std::env::args_os().skip(1)) {
        aproxy_ipc::i18n::init_terminal(|| settings.then(aproxy_ipc::home::resolve_home)?.ok());
    }
    let args = std::env::args_os();
    // LaunchServices 直接启动包内的原生可执行文件，没有命令行参数。只在这个进程消费
    // 应用包的标记；从窗口启动的命令仍按普通 CLI 解析。
    #[cfg(all(target_os = "macos", feature = "desktop"))]
    let args = {
        let app = std::env::var_os("APROXY_BUNDLE_LAUNCH").is_some_and(|v| v == "1");
        // SAFETY: 尚无其他线程读取环境。
        unsafe { std::env::remove_var("APROXY_BUNDLE_LAUNCH") };
        let launch = app && args.len() == 1;
        args.chain(launch.then(|| std::ffi::OsString::from("desktop")))
    };
    let cli = Cli::from_arg_matches_mut(&mut aproxy::cli::command_tree().get_matches_from(args))
        .unwrap_or_else(|e| e.exit());
    // `-V`/`--version` 与别的参数写在一起（`aproxy -V start`）时走到这里，输出同一行。
    // 这种写法要靠 clap 才分得清 `-V` 是版本参数还是别的参数的取值，所以排在解析之后；
    // 安装器只用单独的 `--version`，由上面第一句兑现「先于一切副作用」
    if cli.version {
        print_version_line();
        return std::process::ExitCode::SUCCESS;
    }

    // 桌面界面在建 tokio 运行时之前分派：Tauri 的事件循环要占住主线程（macOS 硬性要求），
    // 而在 block_on 里的主线程处于异步上下文，Tauri 回调里任何 block_on 都会 panic。它自己
    // 建运行时、交给 Tauri 后再进事件循环，见 commands::desktop。不读配置与 settings，界面
    // 自己按 home 寻址
    if let Some(Commands::Desktop { installed_home }) = &cli.command {
        if *installed_home {
            // 只有已安装镜像由菜单用这个参数拉起；后续窗口、实例与看护者继承同一 home。
            let exe = std::env::current_exe().expect("current executable");
            let Some(home) = exe.parent().and_then(std::path::Path::parent) else {
                return std::process::ExitCode::FAILURE;
            };
            // SAFETY: 尚未启动日志线程、事件循环或异步运行时。
            unsafe { std::env::set_var(aproxy_ipc::home::HOME_ENV, home) };
        }
        commands::desktop::handle_desktop_cmd();
    }

    // 内部入口 `__serve`、`__watchdog` 的 home 由命令行的 `--home` 定：拉起方操作的是哪个
    // home，这个进程就属于哪个 home。写进本进程的 APROXY_HOME，之后一切按 home 寻址的
    // 代码（以及它再拉起的子进程）看到的都是它
    if let Some(Commands::Serve { home, .. } | Commands::Watchdog { home }) = &cli.command {
        // SAFETY: 此刻进程里只有主线程——运行时还没建，也没有拉起任何线程
        unsafe { std::env::set_var("APROXY_HOME", home) };
    }

    // 命令先把退出状态交回主线程，再让运行时撤销 I/O、回收工作和阻塞线程；
    // Windows 不应在这些线程仍可能持有管道或分配器状态时执行进程析构。
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("could not create the Tokio runtime");
    let code = runtime.block_on(run(cli));
    drop(runtime);
    commands::finish_json(i32::from(code));
    std::process::ExitCode::from(code)
}

/// home 还未被接受，不能查台账或设置；按已解析的命令构造它约定的 JSON 错误。
fn invalid_home_output(
    command: Option<&Commands>,
    value: &std::ffi::OsStr,
) -> (Option<serde_json::Value>, i32) {
    let mode = commands::JsonOutput::for_command(command);
    let error = serde_json::json!({"code":"home_invalid","message":home_error_text(value, Lang::En),"details":{"value":value.to_string_lossy()}});
    (
        mode.map(|m| m.error_document(error)),
        if mode.is_some() {
            i32::from(aproxy_ipc::code::EXIT_USAGE)
        } else {
            1
        },
    )
}

/// C1 只会拒绝非空相对路径或报告找不到用户目录。按输入区分这两种原因，不解析冻结的文案。
fn home_error_text(value: &std::ffi::OsStr, lang: Lang) -> String {
    use aproxy_ipc::home::{HOME_ENV, USER_DIR_ENV};
    if value.to_str().is_some_and(|s| s.trim().is_empty()) {
        boot::missing_home(lang, USER_DIR_ENV, HOME_ENV)
    } else {
        boot::relative_home(lang, HOME_ENV, &value.to_string_lossy())
    }
}

/// 除 `--version` 与 `desktop` 之外的一切命令，在 tokio 运行时里执行。
async fn run(cli: Cli) -> u8 {
    let output = commands::JsonOutput::for_command(cli.command.as_ref());
    if let Some(mode) = output {
        mode.bind();
    }
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
    let resolved = match &cli.command {
        Some(Commands::RestoreAtLogin { home }) => Ok(home.clone()),
        _ => aproxy_ipc::home::resolve_home(),
    };
    let home = match resolved {
        Ok(home) => home,
        Err(_) => {
            let value = std::env::var_os(aproxy_ipc::home::HOME_ENV).unwrap_or_default();
            let lang = if matches!(
                cli.command,
                Some(Commands::Serve { .. } | Commands::Watchdog { .. })
            ) {
                Lang::En
            } else {
                current()
            };
            let e = home_error_text(&value, lang);
            let (document, exit) = invalid_home_output(cli.command.as_ref(), &value);
            if let Some(doc) = document {
                commands::print_doc(doc);
            }
            let _ = writeln!(std::io::stderr(), "[ERROR] {e}");
            return exit as u8;
        }
    };

    // 后台实例：按交接文件里的拉起规格承载服务，日志由它自己按规格初始化。排在
    // settings 检查与续作拉起之前：这两件事都是给坐在终端前的人准备的，实例自己在解析
    // 配置时检查 settings，结果写进 startup.log
    if let Some(Commands::Serve { spec, .. }) = &cli.command {
        return commands::start::handle_serve_cmd(spec).await;
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

    // 迁移必须见到磁盘原文，不能在旧设置上提前退出或触发安装续作。
    if let Some(Commands::Migrate {
        paths,
        dry_run,
        json,
    }) = cli.command
    {
        return commands::migrate::run(&home, &paths, dry_run, json);
    }

    // settings.json 的检查，每次运行都做。文件无效时（不认识的键、类型不对、schema 更高……）：
    // - `settings` 的各个子命令与 doctor 照常运行：它们就是用来查看与修复它的，自己会报告；
    // - stop、status、install 照常运行、用内置默认，把错误作为告警打印：任何时候都要能停
    //   实例、看状态、装上能修好问题的版本（别名这时不可用，按端口或 all 寻址）；
    // - 其余命令以退出码 3 结束，说清怎样修——带着一份没生效的设置去做事，比不做更糟。
    let settings_path = settings::settings_path_in(&home);
    let loaded = settings::read(&settings_path);
    let reports_itself = matches!(
        cli.command,
        Some(Commands::Doctor { .. } | Commands::Settings { .. })
    );
    if !loaded.is_valid() && !reports_itself {
        let tolerated = matches!(
            cli.command,
            Some(
                Commands::Status { .. }
                    | Commands::Stop { .. }
                    | Commands::Install(_)
                    | Commands::Skill { .. }
            )
        );
        // 诊断接收端可能已经关闭；它不能让 stdout 上独立的命令结果也丢失。
        let level = if tolerated { "WARN" } else { "ERROR" };
        let _ = writeln!(
            std::io::stderr(),
            "[{level}] {}",
            boot::settings_errors(current())
        );
        for d in loaded.diagnostics.iter().filter(|d| d.is_error()) {
            let _ = writeln!(
                std::io::stderr(),
                "[{level}]   {}",
                aproxy::ops::start::render_diagnostic(d, current())
            );
        }
        if tolerated {
            let _ = writeln!(
                std::io::stderr(),
                "[{level}] {}",
                boot::using_defaults(current())
            );
        } else {
            let legacy = commands::settings::print_repair_hints(&settings_path, &loaded);
            if let Some(mode) = output {
                let mut message = boot::settings_errors(Lang::En).to_owned();
                if legacy {
                    message.push('\n');
                    message.push_str(
                        &aproxy_ipc::i18n::diagnostic::Reason::LegacyConfig.render(Lang::En),
                    );
                }
                commands::print_doc(mode.error_document(serde_json::json!({"code":"config_invalid","message":message,"details":{"diagnostics":loaded.diagnostics}})));
            }
            return aproxy_ipc::code::EXIT_CONFIG;
        }
    }
    // 指向不存在的文件的别名：start/stop 按这个名字操作会失败，每次运行都提醒。doctor 会
    // 汇总输出，这里跳过以免重复
    if loaded.is_valid() && !matches!(cli.command, Some(Commands::Doctor { .. })) {
        for (name, path) in settings::missing_alias_targets(&loaded.settings) {
            let e = boot::missing_alias(current(), name, path);
            let _ = writeln!(std::io::stderr(), "[ERROR] {e}");
        }
    }

    // 只有要改动实例或设置的请求才顺带恢复安装；观察现场的命令不应改变它。
    // halted 表示原因不会自行消失，必须让用户排除原因后显式安装。
    if cli.command.as_ref().is_some_and(resumes_install)
        && aproxy::install::state::load_in(&home)
            .is_some_and(|s| !(s.phase == aproxy::install::state::InstallPhase::Failed && s.halted))
        && let Ok(exe) = std::env::current_exe()
    {
        let args = ["install".to_owned(), "--continue".to_owned()];
        if matches!(cli.command, Some(Commands::RestoreAtLogin { .. })) {
            let env = [("APROXY_HOME".to_owned(), Some(home.display().to_string()))];
            let _ = aproxy::daemon::spawn_detached_with(
                &exe,
                &args,
                &aproxy::daemon::Launch {
                    env: &env,
                    cwd: Some(&home),
                },
            );
        } else {
            let _ = aproxy::daemon::spawn_detached(&exe, &args);
        }
    }

    // 子命令分派
    match cli.command {
        Some(Commands::Status { idle, busy, json }) => {
            commands::status::handle_status_cmd(idle, busy, json).await
        }
        Some(Commands::Start(args)) => return commands::start::handle_start_cmd(args).await,
        Some(Commands::Stop {
            target,
            threshold,
            force,
            json,
        }) => return commands::stop::handle_stop_cmd(target, threshold, force, json).await,
        Some(Commands::Restart {
            launch,
            formats,
            target,
            threshold,
            force,
            json,
        }) => {
            return commands::restart::handle_restart_cmd(
                target,
                threshold,
                force,
                formats,
                launch.resolve("restart"),
                json,
            )
            .await;
        }
        Some(Commands::Logs {
            target,
            lines,
            follow,
        }) => commands::logs::handle_logs_cmd(target, lines, follow).await,
        Some(Commands::Restore { json, launch }) => {
            return commands::restore::handle_restore_cmd(&home, launch.resolve("restore"), json)
                .await;
        }
        Some(Commands::RestoreAtLogin { .. }) => {
            return commands::restore::handle_restore_cmd(
                &home,
                aproxy::spec::Launch::new("login"),
                false,
            )
            .await;
        }
        Some(Commands::Alias { cmd }) => commands::alias::handle_alias_cmd(cmd),
        Some(Commands::Migrate { .. }) => {
            unreachable!("migration handled before settings preflight")
        }
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
        Some(Commands::Skill { cmd }) => return commands::skill::run(cmd).await,
        Some(Commands::Desktop { .. }) => {
            unreachable!("desktop was dispatched before the runtime was built")
        }
        Some(Commands::Serve { .. } | Commands::Watchdog { .. }) => {
            unreachable!("internal entry points were handled before subcommand dispatch")
        }
        // 不带子命令的调用在参数解析时已经打印帮助并以 2 退出；走到这里的只有版本参数与
        // 别的参数写在一起的情形，上面已经处理
        None => unreachable!("invocation without a subcommand ended during argument parsing"),
    }
    0
}

/// 输出版本行（永久兼容契约 C5，见 aproxy::VERSION_LINE）。写失败（管道已关闭、终端已
/// 断开）一律忽略：C5 规定退出码是 0，而 `println!` 写失败会 panic、退出码变成 101。读够
/// 首行就关掉管道（`| head -n 1`）的安装器与脚本是常态，不是意外。
fn print_version_line() {
    use std::io::Write as _;
    let _ = writeln!(std::io::stdout(), "{}", aproxy::VERSION_LINE);
}

/// 先辨认输出给机器的入口，连系统语言也不查询。显式查看帮助仍是在给终端排版。
fn terminal_settings(mut args: impl Iterator<Item = std::ffi::OsString>) -> Option<bool> {
    let command = args.find(|s| s != "--version" && s != "-V");
    let rest: Vec<_> = args.take_while(|s| s != "--").collect();
    let has = |flag: &str| rest.iter().any(|s| s == flag);
    match command.as_deref().and_then(|s| s.to_str()) {
        Some("install" | "upgrade") if has("--force") => Some(false),
        _ if has("--help") || has("-h") => Some(true),
        Some("install" | "upgrade") if has("--continue") => None,
        Some("__serve" | "__watchdog" | "restore-at-login") => None,
        Some("skill") if rest.first().is_some_and(|s| s == "__verify-package") => None,
        _ => Some(true),
    }
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
        | Commands::Restore { .. }
        | Commands::RestoreAtLogin { .. } => true,
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
        | Commands::Migrate { .. }
        | Commands::Find { .. }
        | Commands::Doctor { .. }
        | Commands::Install(_)
        | Commands::Skill { .. }
        | Commands::Desktop { .. }
        | Commands::Serve { .. }
        | Commands::Watchdog { .. } => false,
    }
}

#[cfg(test)]
mod home_output_tests {
    use super::*;
    #[test]
    fn bootstrap_never_reads_force_or_worker_settings() {
        for (args, allowed) in [
            ("install --force", Some(false)),
            ("-V upgrade latest --force --help", Some(false)),
            ("install --download-proxy=--force", Some(true)),
            ("install -- --force", Some(true)),
            ("skill __verify-package missing v", None),
            ("upgrade --continue", None),
            ("stop --force", Some(true)),
            ("__serve --spec myfile", None),
            ("__watchdog --help", Some(true)),
            ("restore-at-login --home dir", None),
        ] {
            assert_eq!(
                terminal_settings(args.split_whitespace().map(Into::into)),
                allowed,
                "{args}"
            );
        }
    }

    /// 用 C1 的纯函数拒绝相对 home，不改变测试进程或任何子进程的真实 home。
    #[test]
    fn status_json_home_failure_returns_its_document_before_dispatch() {
        let value = std::ffi::OsStr::new("relative");
        assert!(aproxy_ipc::home::home_from(Some(value), || unreachable!()).is_err());
        use clap::Parser;
        let cli = Cli::try_parse_from(["aproxy", "status", "--json"]).unwrap();
        let (doc, exit) = invalid_home_output(cli.command.as_ref(), value);
        assert_eq!(exit, 2);
        let doc = doc.unwrap();
        assert_eq!(doc["error"]["code"], "home_invalid");
        assert_eq!(doc["error"]["details"]["value"], "relative");
    }
}
