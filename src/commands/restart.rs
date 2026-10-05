//! `aproxy restart [PORT|all|ALIAS|idle [SECS]]`：重启运行中的实例。
//!
//! 语义边界（用户定调）：**只负责重启，不负责启动**——未运行的 target 提示
//! 「未启动」并退出 1，绝不顺手拉起。重启 = 对每个目标实例 预检 → stop（原
//! 参数或 --force）→ 用注册表/恢复记录中的原始启动参数立即 start（与看门狗
//! 重拉同一 `respawn` 语义：IPC 就绪判定后才报成功）。
//!
//! **先预检、后停止**：restart 的主要用途是「改完 config.toml 让它生效」，
//! 配置笔误恰恰最常出现在这一步。若先停旧实例、再由新守护去读配置，配置
//! 有错时新守护秒死，而旧实例的 .restore 已随优雅退出删除——看门狗与
//! `aproxy restore` 都无从恢复，一个健康的服务就此下线。因此 stop 之前先按
//! 新守护将走的同一条解析路径（start 的 resolve_runtime_config）干跑一遍，
//! 换端口时再探测新端口可否绑定；预检不通过就不碰旧实例并报出原因。
//! `restart all` 逐实例预检，失败的跳过、其余照常重启，最后汇总并以非零退出。

use std::path::PathBuf;

use aproxy::daemon;
use clap::Parser;

use crate::cli::Cli;
use crate::commands::start::resolve_runtime_config;
use crate::commands::stop::{StopMode, resolve_stop_targets, stop_instance};
use crate::server::bind_error_message;

/// 单个实例重启失败所处的阶段——决定旧实例此刻是否还在运行，汇总提示
/// 据此措辞（用户最关心的是「服务还在不在」）。
enum RestartFailure {
    /// 预检未通过：旧实例未被触碰，保持运行
    Precheck(String),
    /// 停止旧实例未确认：旧实例可能仍在运行，未启动新实例（不冒险叠加启动）
    Stop,
    /// 旧实例已停止，新实例未能启动/就绪：该端口的服务已中断
    Start(String),
}

/// `aproxy restart` 入口。target/threshold/force 语义与 stop 完全一致。
pub(crate) async fn handle_restart_cmd(
    target: Option<String>,
    threshold: Option<u64>,
    force: bool,
) {
    let mode = if force {
        StopMode::Force
    } else {
        StopMode::Graceful
    };
    let targets = resolve_stop_targets(target, threshold, force).await;
    if targets.is_empty() {
        return; // 无运行实例：resolve 已输出提示（restart 不启动新实例）
    }
    let mut failures: Vec<(String, RestartFailure)> = Vec::new();
    for info in &targets {
        let port = daemon::port_of(&info.listen_addr).to_string();
        if let Err(failure) = restart_instance(info, mode).await {
            match &failure {
                RestartFailure::Precheck(reason) => {
                    eprintln!("端口 {port} 未重启：预检未通过，旧实例保持运行。\n{reason}");
                }
                RestartFailure::Stop => {
                    eprintln!(
                        "端口 {port} 停止未确认，重启中止（未启动新实例，以免与旧实例争抢端口）。"
                    );
                }
                RestartFailure::Start(reason) => {
                    eprintln!("端口 {port} 重启失败（旧实例已停止，该端口服务已中断）：{reason}");
                }
            }
            failures.push((port, failure));
        }
    }
    if failures.is_empty() {
        return;
    }
    // 多实例时汇总：逐条失败信息可能已被后续实例的输出冲散
    if targets.len() > 1 {
        eprintln!(
            "\n{} 个实例中有 {} 个重启失败（已跳过，其余照常重启）：",
            targets.len(),
            failures.len()
        );
        for (port, failure) in &failures {
            let state = match failure {
                RestartFailure::Precheck(_) => "预检未通过，旧实例保持运行",
                RestartFailure::Stop => "停止未确认，未启动新实例",
                RestartFailure::Start(_) => "旧实例已停止、新实例未就绪，服务已中断",
            };
            eprintln!("  端口 {port}：{state}");
        }
    }
    std::process::exit(1);
}

/// 重启单个实例：捕获原始启动参数 → 预检 → stop → spawn → 就绪判定。
/// 就绪判定按新 pid 定位（见 daemon::wait_spawned_instance_ready）——配置
/// 改端口后依然能正确确认。
async fn restart_instance(
    info: &daemon::InstanceRecord,
    mode: StopMode,
) -> Result<(), RestartFailure> {
    let port = daemon::port_of(&info.listen_addr).to_string();

    // 原始启动参数在 .restore（守护 bind 时写入、优雅退出即删）——必须在
    // stop 之前捕获，否则 --api-key/--listen 等仅本次生效的参数会丢失。
    // .restore 缺失（记录丢失或损坏后被清理）回退到注册表的 config_path。
    let args: Vec<String> = daemon::list_restore_entries()
        .into_iter()
        .find(|e| e.port == port)
        .map(|e| e.args)
        .unwrap_or_else(|| vec!["--config".to_string(), info.config_path.clone()]);

    let cfg_path = precheck_restart(info, &port, &args)
        .await
        .map_err(RestartFailure::Precheck)?;

    if !stop_instance(info, mode).await {
        return Err(RestartFailure::Stop);
    }

    // 记下 startup.log 当前长度：新实例若启动即死，只展示它这次写下的内容
    // （startup.log 是全部实例共用的追加文件，整段尾部可能是陈年旧账）
    let log_mark = startup_log_len();
    let exe = std::env::current_exe().expect("无法定位自身可执行文件");
    let mut child_args = args;
    child_args.push("--daemon-child".to_string());
    let new_pid = daemon::spawn_detached(&exe, &child_args).map_err(|e| {
        RestartFailure::Start(format!(
            "spawn 守护进程失败: {e}{}",
            recovery_hint(&cfg_path)
        ))
    })?;

    // 就绪等待 8 秒（同 start/看门狗重拉）。按新 pid 在注册表定位而非 ping
    // 旧端口：restart 的用途之一就是「改了 config.toml（含换端口）后让新配置
    // 生效」，新守护监听哪个端口由它读到的配置决定（实测踩坑：改端口重启
    // 误报「未就绪」而实例已在新端口运行）；顺带覆盖 --listen 0。新实例提前
    // 退出时立即报告并展示 startup.log 的新增内容，不再空等满 8 秒。
    match daemon::wait_spawned_instance_ready(
        &daemon::run_dir(),
        new_pid,
        std::time::Duration::from_secs(8),
    )
    .await
    {
        Ok(live) => {
            let new_port = daemon::port_of(&live.instance.listen_addr).to_string();
            println!("已重启（端口 {new_port}）");
            println!("  pid: {}（旧 pid {}）", live.instance.pid, info.pid);
            println!("  监听: http://{}", live.instance.listen_addr);
            Ok(())
        }
        Err(daemon::SpawnNotReady::Exited) => Err(RestartFailure::Start(format!(
            "新实例（pid {new_pid}）启动后立即退出{}{}",
            startup_log_excerpt(log_mark),
            recovery_hint(&cfg_path)
        ))),
        Err(daemon::SpawnNotReady::TimedOut) => Err(RestartFailure::Start(format!(
            "新实例（pid {new_pid}）未在预期时间内就绪{}{}",
            startup_log_excerpt(log_mark),
            recovery_hint(&cfg_path)
        ))),
    }
}

/// 停止旧实例之前的干跑预检：用新守护将走的同一条路径解析配置（原启动
/// 参数 → clap → resolve_runtime_config：严格 TOML 加载、CLI 覆盖、settings
/// 注入、validate），监听地址变了再探测新地址可否绑定。只读，无副作用
/// （bind 探测的 listener 随即释放）。通过返回新守护将使用的配置文件路径。
async fn precheck_restart(
    info: &daemon::InstanceRecord,
    old_port: &str,
    args: &[String],
) -> Result<PathBuf, String> {
    // 原参数就是守护当初的命令行（去掉 --daemon-child），新守护会照原样再
    // 解析一次——这里解析失败（例如旧版本留下的参数本版本已不认识），新守护
    // 必然同样秒死。错误信息不回显参数本身：其中可能含 --api-key 明文
    let cli =
        Cli::try_parse_from(std::iter::once("aproxy".to_string()).chain(args.iter().cloned()))
            .map_err(|e| format!("原启动参数无法被当前版本解析：{}", e.to_string().trim()))?;
    // 守护的配置路径：原参数里的 --config（start 总会注入绝对路径）；缺失时
    // 取注册表里该守护实际加载的路径
    let cfg_path = cli
        .config
        .clone()
        .unwrap_or_else(|| PathBuf::from(&info.config_path));
    if !cfg_path.is_file() {
        return Err(format!("配置文件已不存在（{}）", cfg_path.display()));
    }
    let cfg = resolve_runtime_config(&cli, &cfg_path)?;

    // 监听地址探测：只在地址变了时做（同地址必然被旧实例自己占着，stop 后
    // 才释放，此刻无从探测）。端口 0 由系统分配，无需探测
    let new_addr = cfg.listen_addr.as_str();
    let new_port = daemon::port_of(new_addr);
    if new_addr != info.listen_addr && new_port != "0" {
        // 换到的端口上已有 aProxy 实例：新守护 bind 必败（IPC 探测不触碰代理端口）
        if new_port != old_port
            && let Ok(other) = daemon::ipc_ping(new_port).await
        {
            return Err(format!(
                "新配置的监听端口 {new_port} 上已有 aProxy 实例（pid {}）在运行",
                other.instance.pid
            ));
        }
        if let Err(e) = tokio::net::TcpListener::bind(new_addr).await
            // 同端口只换地址（如 127.0.0.1 → 0.0.0.0）时「已占用」可能正是旧
            // 实例自己，stop 后即释放，不据此拦截；其他错误（地址不可用、权限
            // 不足/系统保留端口）与旧实例无关，照拦
            && !(new_port == old_port && e.kind() == std::io::ErrorKind::AddrInUse)
        {
            return Err(bind_error_message(new_addr, &e));
        }
    }
    Ok(cfg_path)
}

/// startup.log 当前字节长度（不存在 = 0）
fn startup_log_len() -> u64 {
    std::fs::metadata(daemon::logs_dir().join("startup.log"))
        .map(|m| m.len())
        .unwrap_or(0)
}

/// startup.log 自 `mark` 以来新增的内容（最多末尾 20 行），格式化为可直接
/// 拼进错误信息的片段。没有新增内容时退回指路（运行日志按启动随机命名，
/// 父进程无法预知文件名，只能指向目录）。文件比 mark 还短（被截断/重建）
/// 时从头读。
fn startup_log_excerpt(mark: u64) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let path = daemon::logs_dir().join("startup.log");
    let mut fresh = String::new();
    if let Ok(mut f) = std::fs::File::open(&path) {
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        let from = if len >= mark { mark } else { 0 };
        if f.seek(SeekFrom::Start(from)).is_ok() {
            let mut buf = Vec::new();
            let _ = f.read_to_end(&mut buf);
            fresh = String::from_utf8_lossy(&buf).into_owned();
        }
    }
    let lines: Vec<&str> = fresh.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        return format!(
            "，原因通常记录在:\n  {}\n  {}",
            path.display(),
            daemon::logs_dir().display()
        );
    }
    let tail = &lines[lines.len().saturating_sub(20)..];
    let mut out = format!("。{} 本次新增内容:", path.display());
    for line in tail {
        out.push_str("\n  ");
        out.push_str(line);
    }
    out
}

/// 新实例起不来时的恢复指引：旧实例的 .restore 已随停止删除（优雅退出时守护
/// 自删，强杀时 stop_instance 在终止前摘掉），看门狗与 aproxy restore 都不会
/// 自动拉起它，必须由用户修正后手动 start。
fn recovery_hint(cfg_path: &std::path::Path) -> String {
    format!(
        "\n修正上述问题后执行以下命令恢复服务:\n  aproxy start --config \"{}\"",
        cfg_path.display()
    )
}
