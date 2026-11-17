//! `aproxy`（无子命令）/ `aproxy start [别名|路径]`：启动代理；以及后台实例的入口
//! `aproxy __serve`。
//!
//! 父进程侧的动作（配置解析与校验、端口预检、后台拉起与就绪等待）在操作层
//! `aproxy::ops::start`，这里把它们串起来并把结果说给用户；服务本身由
//! `crate::server::serve_forever` 承载（后台实例经 [`handle_serve_cmd`] 进入其中）。

use std::path::{Path, PathBuf};

use aproxy::cli::Cli;
use aproxy::daemon;
use aproxy::ops::start::{
    Preflight, PreflightError, launch, load_lenient, overrides_from_cli, preflight,
    resolve_runtime_config,
};
use aproxy::ops::target::resolve_config_target;
use aproxy::spec::{self, InstanceSpec};

use crate::server::{bind_error_message, report_config_error, serve_forever};

/// `aproxy`（无子命令）/ `aproxy start [别名|路径]`：后台启动实例；
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
    // 这个实例有没有配置文件：命令行指定的、别名或路径给出的，必须在（打错路径时明确
    // 报错，而不是悄悄按空配置启动）；什么都没给、默认的 config.toml 也不在，就是一个只靠
    // 覆盖旗标与 settings.json 全局默认启动的实例
    let explicit = cli.config.is_some() || target.is_some();
    let cfg_file: Option<&Path> = (explicit || cfg_path.exists()).then_some(cfg_path.as_path());
    let overrides = overrides_from_cli(cli);
    let cfg = match resolve_runtime_config(&overrides, cfg_file) {
        Ok(c) => c,
        Err(msg) => {
            eprintln!("{msg}");
            std::process::exit(1);
        }
    };
    let listen_addr = cfg.listen_addr.clone();
    let port = daemon::port_of(&listen_addr).to_string();

    // 非回环监听告警（不阻断启动——对外开放可能正是用户的意图）。后台实例自己还会把它
    // 写进日志与 startup.log，见 handle_serve_cmd
    if let Some(w) = cfg.listen_exposure_warning() {
        eprintln!("警告: {w}");
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
        serve_forever(cfg, cfg_file, None).await;
        return;
    }

    // 后台：把这次启动写成拉起规格，交给分离的子进程（见 aproxy::spawn）。覆盖旗标与环境
    // 里的代理变量都记在规格里，以后 restart、restore、看护者重拉用的是同一份。子进程提前
    // 退出时立即报告，附上 startup.log 本次新增的内容（子进程没有控制台，配置错误、绑定
    // 失败的原因只写在那里）。日志文件名在这里生成一次，重拉之后续写同一个文件
    let exe = std::env::current_exe().expect("无法定位自身可执行文件");
    let log_path = crate::server::resolve_daemon_log_path(cfg.log_file.as_deref());
    let spec = InstanceSpec::capture(cfg_file, overrides, &log_path);
    let info = match launch(&exe, &spec).await {
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
    println!("  Base URL: {}", aproxy_ipc::redact::url(&cfg.base_url));
    if !cfg.base_url.is_empty() {
        // 与上一行同一出口：这一行同样会被整段粘贴求助
        let base = aproxy_ipc::redact::url(cfg.base_url.trim_end_matches('/'));
        println!(
            "  提示: 将你的 API base URL 指向 http://{}",
            info.instance.listen_addr
        );
        println!("        上游路径与查询参数将完整透传到 {base}/<path>?<query>");
    }
    // 仅转发模式是放弃重试保障的取舍，启动提示里必须显式说出来
    // （父进程的 cfg 已完成 settings 注入，取值与后台实例一致）
    if cfg.forward_only_enabled() {
        println!("  仅转发模式：不缓冲、不重试");
    }
    // 外部转换器改变了「原样透传」的承诺，启动提示必须显式说出来
    if cfg.request_transform.is_some() || cfg.response_transform.is_some() {
        println!(
            "  外部转换器：请求/响应将交给 format 程序改写（转换失败：请求侧 502 不重试、响应侧透传原样）"
        );
    }
    match cfg_file {
        Some(path) => println!("  配置: {}", path.display()),
        None => println!("  配置: （没有配置文件，使用命令行参数与全局默认）"),
    }
    if let Some(log_path) = &info.instance.log_path {
        println!("  日志: {log_path}");
    }
    println!("查看实例: aproxy status    停止: aproxy stop {actual_port}");
}

/// `aproxy __serve --home <dir> --spec <file>`：后台实例的入口。读交接文件里的拉起规格，
/// 按它解析配置、初始化日志，然后承载服务直到停止。
///
/// 这个进程没有控制台：在日志初始化之前出的错（规格读不出来）与配置错误都写进
/// startup.log，拉起方在子进程退出后从那里摘录原因。
pub(crate) async fn handle_serve_cmd(spec_path: &Path) -> ! {
    let mut spec = match spec::read_spec_file(spec_path) {
        Ok(spec) => spec,
        Err(e) => {
            report_config_error(
                &format!("cannot read the launch spec {}: {e}", spec_path.display()),
                true,
            );
            crate::util::exit(1)
        }
    };
    let cfg_file = spec.config_file();
    let cfg_file = cfg_file.as_deref();

    // 日志初始化排在校验之前：此刻配置还没有严格校验，用宽松解析定出日志写到哪里
    // （配置或覆盖里的 log_file 优先，否则用规格里记下的文件），校验不过的原因才有处可记
    let lenient = load_lenient(&spec.overrides, cfg_file);
    let log_path = match lenient.log_file.as_deref() {
        Some(custom) => crate::server::resolve_daemon_log_path(Some(custom)),
        None => PathBuf::from(&spec.log_path),
    };
    crate::server::init_daemon_logging(&log_path);

    let mut cfg = match resolve_runtime_config(&spec.overrides, cfg_file) {
        Ok(cfg) => cfg,
        Err(msg) => {
            report_config_error(&msg, true);
            crate::util::exit(1)
        }
    };
    // 配置解析出的端口是 0（由系统分配）而规格里钉着上次拿到的端口：回到那个端口，客户端
    // 配的就是它。配置已经改成固定端口时以配置为准，钉住的端口作废
    if daemon::port_of(&cfg.listen_addr) == "0" {
        if let Some(pinned) = spec.pinned_port {
            cfg.listen_addr = replace_port(&cfg.listen_addr, pinned);
        }
    } else {
        spec.pinned_port = None;
    }
    // 重拉、restore、安装的滚动重启拉起的实例没有人在看控制台，startup.log 是这些启动
    // 唯一的留痕处
    if let Some(w) = cfg.listen_exposure_warning() {
        tracing::warn!("{w}");
        report_config_error(&format!("警告: {w}"), true);
    }
    serve_forever(cfg, cfg_file, Some(spec)).await;
    crate::util::exit(0)
}

/// 把监听地址里的端口换成 `port`（地址部分原样保留，含 IPv6 的方括号）。
fn replace_port(listen_addr: &str, port: u16) -> String {
    match listen_addr.rsplit_once(':') {
        Some((host, _)) => format!("{host}:{port}"),
        None => listen_addr.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replace_port_keeps_the_host_part() {
        assert_eq!(replace_port("127.0.0.1:0", 40123), "127.0.0.1:40123");
        assert_eq!(replace_port("[::1]:0", 40123), "[::1]:40123");
        assert_eq!(replace_port("0.0.0.0:0", 9), "0.0.0.0:9");
    }
}
