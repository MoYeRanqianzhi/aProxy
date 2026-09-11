//! aProxy — 本地 API 代理，无限重试保障 agent 工作流不中断。
//!
//! 本文件只是进程入口：控制台代码页设置、配置文件定位、日志初始化与子命令分派。
//! CLI 定义在 `cli.rs`，各子命令处理在 `commands/`，服务主循环在 `server.rs`，
//! 共用小工具在 `util.rs`。

#[cfg(windows)]
use windows_sys::Win32::System::Console::{GetConsoleOutputCP, SetConsoleOutputCP};

mod cli;
mod commands;
mod server;
mod util;

use clap::Parser;

use cli::{Cli, Commands};

use aproxy::config;
use aproxy::settings::{self, expand_path};

#[tokio::main]
async fn main() {
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
                    let path = expand_path(p);
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
        let cfg = commands::start::load_with_cli_overrides(&cli, &cfg_path);
        server::init_daemon_logging(&cfg.listen_addr);
    } else {
        server::init_stdout_logging();
    }

    // 守护子进程只承载服务，忽略转发来的子命令：start 父进程把完整命令行
    // （含 `start <别名>`）转发给子进程，若子进程再进入 Start 分支会递归
    // spawn 出套娃进程。分流必须先于子命令 match。
    if cli.daemon_child {
        commands::start::handle_start_cmd(&cli, cfg_path, None).await;
        return;
    }

    // 看护进程：进入看护主循环（claim 接管 + 收养 + 健康扫描 + 重拉）。
    // 同样忽略一切子命令（转发来的参数对看护者无意义）。
    if cli.daemon_watchdog {
        let cfg = aproxy::watchdog::WatchdogConfig::from_settings();
        aproxy::watchdog::serve(cfg).await;
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
        Some(Commands::Status { idle, busy }) => {
            commands::status::handle_status_cmd(idle, busy).await
        }
        Some(Commands::Start { ref target }) => {
            let cli = cli.clone();
            commands::start::handle_start_cmd(&cli, cfg_path, target.clone()).await;
        }
        Some(Commands::Stop {
            target,
            threshold,
            force,
        }) => commands::stop::handle_stop_cmd(target, threshold, force).await,
        Some(Commands::Restart {
            target,
            threshold,
            force,
        }) => commands::restart::handle_restart_cmd(target, threshold, force).await,
        Some(Commands::Logs { target }) => commands::logs::handle_logs_cmd(target).await,
        Some(Commands::Restore) => commands::restore::handle_restore_cmd().await,
        Some(Commands::Alias { cmd }) => commands::alias::handle_alias_cmd(cmd),
        Some(Commands::Doctor) => commands::doctor::handle_doctor_cmd().await,
        Some(Commands::Find {
            aliased,
            unaliased,
            query,
            port,
        }) => commands::find::handle_find_cmd(aliased, unaliased, query, port),
        Some(Commands::Config(args)) => commands::config::handle_config_cmd(cfg_path, args),
        Some(Commands::Install(args) | Commands::Upgrade(args)) => {
            commands::install::handle_install_cmd(args).await
        }
        None => commands::start::handle_start_cmd(&cli, cfg_path, None).await,
    }
}
