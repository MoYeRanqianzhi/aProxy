//! `aproxy`（无子命令）/ `aproxy start [别名|路径]`：启动代理。
//!
//! 父进程侧的动作（配置解析与校验、端口预检、后台拉起与就绪等待）在操作层
//! `aproxy::ops::start`，这里把它们串起来并把结果说给用户；服务本身由
//! `crate::server::serve_forever` 承载（守护子进程直接进入其中）。

use std::path::PathBuf;

use aproxy::cli::Cli;
use aproxy::config::mask_base_url;
use aproxy::daemon;
use aproxy::ops::start::{Preflight, PreflightError, launch, preflight, resolve_runtime_config};
use aproxy::ops::target::resolve_config_target;

use crate::server::{bind_error_message, report_config_error, serve_forever};

/// `aproxy`（无子命令）/ `aproxy start [别名|路径]`：后台启动守护进程；
/// --foreground 前台运行。
///
/// target 解析：settings.json 别名表优先（`aproxy alias add` 管理），
/// 其次按配置文件路径；都落空则报错。
///
/// 端口冲突的情况严格区分（同端口已有实例 / 同端口不同地址的另一实例 / 绑定失败），
/// 判定在 `aproxy::ops::start::preflight`。
pub(crate) async fn handle_start_cmd(
    cli: &Cli,
    cfg_path: Result<PathBuf, String>,
    target: Option<String>,
) {
    // start <别名|路径>：别名解析出的路径只在本进程可见，守护子进程的命令行
    // 里必须显式带上 --config（拉起时注入），否则子进程会回退默认配置。
    // 给了 target 就不需要默认配置——失效的 default_config 只拦无 target 的启动
    let cfg_path = match target.as_deref() {
        None => super::require_cfg_path(cfg_path),
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

    // 非回环监听告警（不阻断启动——对外开放可能正是用户的意图）。三个去向
    // 各管一种读者：start 父进程 / 前台实例打到 stderr（当场敲命令的人）；
    // 守护子进程写守护日志与 startup.log——看门狗重拉、restore、install 滚动
    // 重启拉起的实例没有控制台，startup.log 是这些启动唯一的留痕处。
    // report_config_error 是 startup.log 的唯一写入口（含 logs 目录不可写时
    // 的回退），告警复用它而不另起一套落盘逻辑。
    if let Some(w) = cfg.listen_exposure_warning() {
        if cli.daemon_child {
            tracing::warn!("{w}");
            report_config_error(&format!("警告: {w}"), true);
        } else {
            eprintln!("警告: {w}");
        }
    }

    // 守护子进程：直接承载服务，不再走启动预检/后台 spawn（父进程已做）
    if cli.daemon_child {
        serve_forever(cfg, &cfg_path, true).await;
        return;
    }

    // 端口预检（见 aproxy::ops::start::preflight）：同端口同地址已有本 home 的实例就不
    // 重复启动；同端口不同地址的实例与本进程不能并存；绑定不了则按错误类别说明
    match preflight(&listen_addr).await {
        Ok(Preflight::Clear) => {}
        Ok(Preflight::AlreadyRunning(info)) => {
            println!("此端口已有 aProxy 在运行，无需重复启动：");
            println!(
                "  pid {}  监听 http://{}  v{}",
                info.instance.pid, info.instance.listen_addr, info.instance.version
            );
            println!("查看实例: aproxy status    停止: aproxy stop {port}");
            return;
        }
        Err(PreflightError::PortHeldByOtherAddress {
            port,
            listen_addr,
            other_listen_addr,
        }) => {
            eprintln!(
                "端口 {port} 已被监听地址 {other_listen_addr} 的 aProxy 实例使用（本进程将监听 {listen_addr}），两者不能并存。"
            );
            eprintln!("实例按端口号区分，请为其中一个更换监听地址/端口。");
            std::process::exit(1);
        }
        Err(PreflightError::Bind { listen_addr, error }) => {
            eprintln!("{}", bind_error_message(&listen_addr, &error));
            std::process::exit(1);
        }
    }

    if cli.foreground {
        serve_forever(cfg, &cfg_path, false).await;
        return;
    }

    // 后台：分离子进程承载服务，命令行参数原样转发（`--config` 改写为上面解析出的路径，
    // 见 aproxy::ops::start::launch）。子进程提前退出时立即报告，附上 startup.log 本次
    // 新增的内容（子进程没有控制台，配置错误、绑定失败的原因只写在那里）。日志文件按启动
    // 随机命名（或用户自定义），客户端不拼路径——就绪应答里带着实例上报的 log_path。
    let exe = std::env::current_exe().expect("无法定位自身可执行文件");
    let args: Vec<String> = std::env::args().skip(1).collect();
    let info = match launch(&exe, args, &cfg_path).await {
        Ok(info) => info,
        Err(e) => {
            let excerpt = e.startup_log().map(|l| l.excerpt()).unwrap_or_default();
            eprintln!("{e}{excerpt}");
            std::process::exit(1);
        }
    };
    let actual_port = daemon::port_of(&info.instance.listen_addr).to_string();
    println!("aProxy 已在后台启动");
    println!("  pid: {}", info.instance.pid);
    println!("  监听: http://{}", info.instance.listen_addr);
    println!("  Base URL: {}", mask_base_url(&cfg.base_url));
    if !cfg.base_url.is_empty() {
        // 与上一行同一出口：这一行同样会被整段粘贴求助
        let base = mask_base_url(cfg.base_url.trim_end_matches('/'));
        println!(
            "  提示: 将你的 API base URL 指向 http://{}",
            info.instance.listen_addr
        );
        println!("        上游路径与查询参数将完整透传到 {base}/<path>?<query>");
    }
    // 仅转发模式是放弃重试保障的取舍，启动提示里必须显式说出来
    // （父进程的 cfg 已完成 settings 注入，取值与守护子进程一致）
    if cfg.forward_only_enabled() {
        println!("  仅转发模式：不缓冲、不重试");
    }
    // 外部转换器改变了「原样透传」的承诺，启动提示必须显式说出来
    if cfg.request_transform.is_some() || cfg.response_transform.is_some() {
        println!(
            "  外部转换器：请求/响应将交给 format 程序改写（转换失败：请求侧 502 不重试、响应侧透传原样）"
        );
    }
    println!("  配置: {}", cfg_path.display());
    println!("  日志: {}", info.instance.log_path);
    println!("查看实例: aproxy status    停止: aproxy stop {actual_port}");
}
