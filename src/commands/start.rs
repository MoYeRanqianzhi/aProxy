//! `aproxy start [别名|路径]`：启动代理；以及后台实例的入口
//! `aproxy __serve`。
//!
//! 父进程侧的动作（配置解析与校验、端口预检、后台拉起与就绪等待）在操作层
//! `aproxy::ops::start`，这里把它们串起来并把结果说给用户；服务本身由
//! `crate::server::serve_forever` 承载（后台实例经 [`handle_serve_cmd`] 进入其中）。

#[path = "../i18n/start.rs"]
pub(crate) mod text;
use aproxy_ipc::i18n::current;

use std::path::{Path, PathBuf};

use aproxy::cli::StartArgs;
use aproxy::daemon;
use aproxy::ops::start::{
    ConfigError, Preflight, PreflightError, launch, load_lenient, overrides_from_cli, preflight,
    resolve_runtime_config,
};
use aproxy::ops::target::resolve_config_target;
use aproxy::spec;
use serde_json::json;

use super::{Failure, port_value, print_doc};
use crate::server::{ServeFailure, report_config_error, report_launch, serve_forever};

/// `aproxy start [别名|路径]`：后台启动实例；--foreground 前台运行。
///
/// target 解析：settings.json 别名表优先（`aproxy alias add` 管理），
/// 其次按配置文件路径；都落空则报错。
///
/// 端口冲突的情况严格区分（同端口已有实例 / 同端口不同地址的另一实例 / 绑定失败），
/// 判定在 `aproxy::ops::start::preflight`。
pub(crate) async fn handle_start_cmd(mut args: StartArgs) -> u8 {
    let json = args.json;
    // `--api-key -`：密钥从 stdin 读一行，不出现在命令行上
    if args.api_key.as_deref() == Some("-") {
        match crate::util::read_secret_line() {
            Ok(key) => args.api_key = Some(key),
            Err(e) => return e.report(json),
        }
    }
    let target = args.target.clone();
    // 给了 target 就不需要默认配置——失效的 default_config 只拦无 target 的启动
    let cfg_path = match target.as_deref() {
        None => match super::resolve_cfg_path(args.config.as_deref()) {
            Ok(path) => path,
            Err(f) => return f.report(json),
        },
        Some(t) => match resolve_config_target(t) {
            Some(p) => p,
            None => {
                return Failure::rendered(
                    "target_not_found",
                    |lang| text::unknown(lang, t),
                    json!({"target": t}),
                )
                .report(json);
            }
        },
    };
    // 这个实例有没有配置文件：命令行指定的、别名或路径给出的，必须在（打错路径时明确
    // 报错，而不是悄悄按空配置启动）；什么都没给、默认的 config.toml 也不在，就是一个只靠
    // 覆盖旗标与 settings.json 全局默认启动的实例
    let explicit = args.config.is_some() || target.is_some();
    let cfg_file: Option<&Path> = (explicit || cfg_path.exists()).then_some(cfg_path.as_path());
    let overrides = overrides_from_cli(&args);
    let cfg = match resolve_runtime_config(&overrides, cfg_file) {
        Ok(c) => c,
        Err(e) => return config_failure(&e).report(json),
    };
    let listen_addr = cfg.listen_addr.clone();
    let port = daemon::port_of(&listen_addr).to_string();

    // 非回环监听告警（不阻断启动——对外开放可能正是用户的意图）。后台实例自己还会把它
    // 写进日志与 startup.log，见 handle_serve_cmd
    if let Some(w) = cfg.listen_exposure_warning(current()) {
        eprintln!("{}", super::config_text::warning(current(), &w));
    }

    // 端口预检（见 aproxy::ops::start::preflight）：同端口同地址已有本 home 的实例就不
    // 重复启动；同端口不同地址的实例与本进程不能并存；绑定不了则按错误类别说明
    match preflight(&listen_addr).await {
        Ok(Preflight::Clear) => {}
        Ok(Preflight::BecameReady(info)) => {
            if json {
                print_doc(start_document("started", &info));
            } else {
                println!(
                    "{}",
                    text::ready(current(), info.instance.pid, &info.instance.listen_addr)
                );
            }
            return 0;
        }
        Err(PreflightError::Startup(e)) => {
            return super::spawn_failure(&e, |lang| e.render(lang)).report(json);
        }
        Ok(Preflight::AlreadyRunning(info)) => {
            if json {
                print_doc(start_document("already_running", &info));
                return 0;
            }
            println!("{}", text::already(current()));
            println!(
                "{}",
                text::existing(
                    current(),
                    info.instance.pid,
                    &info.instance.listen_addr,
                    &info.instance.version
                )
            );
            println!("{}", text::manage(current(), &port));
            return 0;
        }
        Err(PreflightError::PortHeldByOtherAddress {
            port,
            listen_addr,
            other_listen_addr,
        }) => {
            return Failure::rendered(
                "port_in_use",
                |lang| text::other_address(lang, &port, &other_listen_addr, &listen_addr),
                json!({"port": port_value(&port), "owner": "aproxy"}),
            )
            .report(json);
        }
        Err(PreflightError::Bind { listen_addr, error }) => {
            return super::bind_failure(&listen_addr, &error).report(json);
        }
    }

    if args.foreground {
        return match serve_forever(cfg, cfg_file, None).await {
            Ok(()) => 0,
            Err(ServeFailure::Launch(reason)) => {
                report_launch(reason, None);
                1
            }
            Err(ServeFailure::Format(e)) => Failure::handshake(&e).report(json),
        };
    }

    // 后台：把这次启动写成拉起规格，交给分离的子进程（见 aproxy::spawn）。覆盖旗标与环境
    // 里的代理变量都记在规格里，以后 restart、restore、看护者重拉用的是同一份。子进程提前
    // 退出时立即报告，附上 startup.log 本次新增的内容（子进程没有控制台，配置错误、绑定
    // 失败的原因只写在那里）。日志文件名在这里生成一次，重拉之后续写同一个文件
    let exe = std::env::current_exe().expect("cannot locate the current executable");
    let log_path = crate::server::resolve_daemon_log_path(cfg.log_file.as_deref());
    let spec = spec::capture(cfg_file, overrides, &log_path);
    let info = match launch(&exe, &spec).await {
        Ok(info) => info,
        Err(e) => {
            return super::spawn_failure(&e, |lang| {
                format!("{}{}", e.render(lang), e.excerpt(lang))
            })
            .report(json);
        }
    };
    if json {
        print_doc(start_document("started", &info));
        return 0;
    }
    let port = daemon::port_of(&info.instance.listen_addr).to_string();
    println!("{}", text::started(current()));
    println!("  pid: {}", info.instance.pid);
    println!("{}", text::listen(current(), &info.instance.listen_addr));
    println!(
        "{}",
        text::base(current(), &aproxy_ipc::redact::url(&cfg.base_url))
    );
    if !cfg.base_url.is_empty() {
        // 与上一行同一出口：这一行同样会被整段粘贴求助
        let base = aproxy_ipc::redact::url(cfg.base_url.trim_end_matches('/'));
        println!("{}", text::client(current(), &info.instance.listen_addr));
        println!("{}", text::upstream(current(), &base));
    }
    // 仅转发模式是放弃重试保障的取舍，启动提示里必须显式说出来
    // （父进程的 cfg 已完成 settings 注入，取值与后台实例一致）
    if cfg.forward_only_enabled() {
        println!("{}", text::forward(current()));
    }
    // 外部转换器改变了「原样透传」的承诺，启动提示必须显式说出来
    if !cfg.formats.is_empty() {
        println!("{}", text::formats(current(), cfg.formats.len()));
    }
    match cfg_file {
        Some(path) => println!("{}", text::config(current(), path.display())),
        None => println!("{}", text::no_config(current())),
    }
    if let Some(log_path) = &info.instance.log_path {
        println!("{}", text::log(current(), log_path));
    }
    println!("{}", text::manage(current(), &port));
    0
}

/// `start --json` 的文档：`outcome` 是 `started` 或 `already_running`，`instance` 是实例应答的
/// 状态（与 status 里的一项同形）。
fn start_document(outcome: &str, info: &daemon::InstanceStatus) -> serde_json::Value {
    json!({
        "outcome": outcome,
        "port": port_value(daemon::port_of(&info.instance.listen_addr)),
        "pid": info.instance.pid,
        "log_path": info.instance.log_path,
        "instance": info,
    })
}

/// 配置解析不出一份能用的配置：文件不在是「目标不存在」（4），有 error 级诊断是「配置
/// 无效」（3），读不出来是一般的失败（1）。
pub(crate) fn config_failure(error: &ConfigError) -> Failure {
    let reason = error.reason();
    let message = |lang| reason.render(lang);
    match error {
        ConfigError::Missing(path) => Failure::rendered(
            "target_not_found",
            message,
            json!({"target": path.display().to_string()}),
        ),
        ConfigError::Settings { diagnostics, .. } | ConfigError::Invalid { diagnostics, .. } => {
            Failure::rendered(
                "config_invalid",
                message,
                json!({"diagnostics": diagnostics}),
            )
        }
        ConfigError::Read { .. } => {
            Failure::rendered("start_failed", message, json!({"startup_log": []}))
        }
    }
}

/// `aproxy __serve --home <dir> --spec <file>`：后台实例的入口。读交接文件里的拉起规格，
/// 按它解析配置、初始化日志，然后承载服务直到停止。
///
/// 这个进程没有控制台：在日志初始化之前出的错（规格读不出来）与配置错误都写进
/// startup.log，拉起方在子进程退出后从那里摘录原因。
pub(crate) async fn handle_serve_cmd(spec_path: &Path) -> u8 {
    let mut spec = match spec::read_spec_file(spec_path) {
        Ok(spec) => spec,
        Err(e) => {
            report_launch(
                aproxy::spawn::startup::LaunchReason::SpecRead {
                    path: spec_path.display().to_string(),
                    cause: Box::new(e.diagnostic()),
                },
                Some(spec_path),
            );
            return 1;
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
        Err(e) => {
            report_launch(e.reason(), Some(spec_path));
            return 1;
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
    if let Some(w) = cfg.listen_exposure_warning(aproxy_ipc::i18n::Lang::En) {
        tracing::warn!("{w}");
        report_config_error(
            &super::config_text::warning(aproxy_ipc::i18n::Lang::En, &w),
            true,
        );
    }
    if let Err(failure) = serve_forever(cfg, cfg_file, Some(spec)).await {
        let e = match failure {
            ServeFailure::Launch(reason) => {
                report_launch(reason, Some(spec_path));
                return 1;
            }
            ServeFailure::Format(e) => e,
        };
        report_config_error(&e.to_string(), true);
        // 规格已读完，原文件仅本次拉起方可读。只覆盖已有文件：若父进程已超时清理，
        // 不得重新留下交接文件。父进程只在确认子进程退出之后读取错误。
        if let Ok(file) = std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(spec_path)
        {
            let _ = serde_json::to_writer(file, &e);
        }
        return e.exit_code();
    }
    0
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
