//! `aproxy logs [PORT|别名] [-n N] [-f]`：打印实例的守护日志的最近若干行，`-f` 时接着跟踪。
//!
//! 退出码：输出完毕 0；前台实例没有日志文件 1（`no_log_file`）；多个实例而没指定目标 2
//! （`target_ambiguous`）；目标没在运行、也没有记录可读 4（`target_not_running`），别名不存在
//! 也是 4（`target_not_found`）。

use aproxy::daemon::{self, InstanceStatus};
use aproxy::ops::target::{config_path_key, resolve_config_target};
use aproxy::spec::RestoreEntry;
use serde_json::json;

use super::{Failure, port_value};

/// logs 要读的日志从哪来。
enum Source {
    /// 在运行的实例：日志路径向它本人要（日志文件按启动随机命名，或是配置里的 log_file，
    /// 不能按端口拼）
    Running(Box<InstanceStatus>),
    /// 没在运行、但留着期望状态记录的实例：读记录里的日志路径，那里是它上次运行（多半是
    /// 崩溃）的现场
    Stopped { port: String, log_path: String },
}

/// `aproxy logs [PORT|别名]`：打印实例的守护日志的最近 `lines` 行；`follow` 为真时接着实时
/// 输出新写进来的。默认打印完就退出：调用它的 agent 与脚本不会被挂住。
/// 目标的语义与 stop 一致：单实例可省略 target；多实例必须指定；别名按配置文件
/// 匹配运行实例（端口变了别名依然有效）；不支持 all（一次只读一个实例的日志）。
pub(crate) async fn handle_logs_cmd(target: Option<String>, lines: usize, follow: bool) {
    // logs 没有 --json：失败只写 stderr、给退出码
    let source = match locate(target.as_deref()).await {
        Ok(source) => source,
        Err(failure) => failure.exit(false),
    };
    let (port, log_path, running) = match source {
        Source::Running(info) => {
            let port = daemon::port_of(&info.instance.listen_addr).to_string();
            let Some(log_path) = info.instance.log_path else {
                // 前台实例经 IPC 上报的 log_path 为 null：日志在它的控制台
                Failure::new(
                    "no_log_file",
                    format!(
                        "端口 {port} 的实例以前台模式运行（--foreground），日志输出在它的控制台，没有日志文件。"
                    ),
                    json!({"port": port_value(&port)}),
                )
                .exit(false)
            };
            if follow {
                eprintln!(
                    "正在连接 pid {}（端口 {port}）——Ctrl+C 退出。",
                    info.instance.pid
                );
            }
            (port, log_path, true)
        }
        Source::Stopped { port, log_path } => {
            if !std::path::Path::new(&log_path).exists() {
                Failure::new(
                    "target_not_running",
                    format!(
                        "端口 {port} 的实例没在运行；它的期望状态记录指向的日志文件不存在: {log_path}"
                    ),
                    json!({"target": port}),
                )
                .exit(false)
            }
            eprintln!(
                "端口 {port} 的实例没在运行，以下是它上次运行的日志{}。",
                if follow {
                    "（没在运行，不跟随）"
                } else {
                    ""
                }
            );
            (port, log_path, false)
        }
    };
    let path = std::path::PathBuf::from(log_path);
    // 说明写到 stderr：stdout 上只有日志本身，接在管道后面的程序拿到的就是日志行
    eprintln!("日志文件: {}", path.display());
    if let Err(e) = print_log_file(&path, &port, lines, follow && running).await {
        eprintln!("{e}");
        crate::util::exit(1);
    }
}

/// 按 target 找到要读的日志。在运行的实例优先；找不到在运行的，再找它留下的期望状态记录。
async fn locate(target: Option<&str>) -> Result<Source, Failure> {
    if target == Some("all") {
        return Err(Failure::new(
            "usage",
            "aproxy logs 不支持 all：一次只读一个实例的日志，请指定端口号或配置别名。",
            json!({"reason": "logs_target_all"}),
        ));
    }
    let records = aproxy::spec::list_restore_in(&daemon::run_dir()).entries;
    let stopped = |entry: &RestoreEntry| Source::Stopped {
        port: entry.port.clone(),
        log_path: entry.spec.log_path.clone(),
    };
    match target {
        // 端口：直接按端口 IPC 定位，不依赖注册表（与 stop 相同）
        Some(target) if target.parse::<u16>().is_ok() => {
            let port = daemon::port_of(target).to_string();
            if let Ok(info) = daemon::ipc_ping(&port).await {
                return Ok(Source::Running(Box::new(info)));
            }
            match records.iter().find(|entry| entry.port == port) {
                Some(entry) => Ok(stopped(entry)),
                None => Err(Failure::new(
                    "target_not_running",
                    format!("端口 {port} 上没有运行中的 aProxy 实例，也没有它上次运行留下的记录。"),
                    json!({"target": port}),
                )),
            }
        }
        // 别名（或配置文件路径）：按配置文件匹配实例与记录，端口变了别名依然有效
        Some(target) => {
            let Some(cfg_path) = resolve_config_target(target) else {
                return Err(super::stop::unknown_target(target));
            };
            let key = config_path_key(&cfg_path.display().to_string());
            let instances = daemon::list_instances().await;
            if let Some(info) = instances.into_iter().find(|i| {
                i.instance
                    .config_path
                    .as_deref()
                    .is_some_and(|path| config_path_key(path) == key)
            }) {
                return Ok(Source::Running(Box::new(info)));
            }
            // 同一份配置留下的记录不止一条时（端口改过），取最近写下的那条
            let record = records
                .iter()
                .filter(|entry| {
                    entry
                        .spec
                        .config_path
                        .as_deref()
                        .is_some_and(|path| config_path_key(path) == key)
                })
                .max_by_key(|entry| entry.written_at);
            match record {
                Some(entry) => Ok(stopped(entry)),
                None => Err(Failure::new(
                    "target_not_running",
                    format!(
                        "别名 {target}（配置 {}）当前未在运行，也没有它上次运行留下的记录。",
                        cfg_path.display()
                    ),
                    json!({"target": target}),
                )),
            }
        }
        // 省略：只有一个实例在运行时就是它；一个都没有时，只有一条记录就读那条
        None => {
            let mut instances = daemon::list_instances().await;
            if instances.len() > 1 {
                let ports: Vec<String> = instances
                    .iter()
                    .map(|info| daemon::port_of(&info.instance.listen_addr).to_string())
                    .collect();
                return Err(ambiguous(
                    format!(
                        "有 {} 个实例在运行，必须指定端口号或配置别名（一次只读一个实例的日志）：",
                        ports.len()
                    ),
                    &ports,
                ));
            }
            if let Some(info) = instances.pop() {
                return Ok(Source::Running(Box::new(info)));
            }
            match records.as_slice() {
                [] => Err(Failure::new(
                    "target_not_running",
                    "没有运行中的 aProxy 实例，也没有实例上次运行留下的记录。",
                    json!({"target": null}),
                )),
                [entry] => Ok(stopped(entry)),
                _ => {
                    let ports: Vec<String> =
                        records.iter().map(|entry| entry.port.clone()).collect();
                    Err(ambiguous(
                        format!(
                            "没有运行中的实例；有 {} 个实例留下了期望状态记录，指定端口查看它上次运行的日志：",
                            ports.len()
                        ),
                        &ports,
                    ))
                }
            }
        }
    }
}

/// 没指定目标而可读的日志不止一份：`target_ambiguous`（2），说明里列出可以照抄的命令行。
fn ambiguous(head: String, ports: &[String]) -> Failure {
    let mut message = head;
    for port in ports {
        message.push_str(&format!("\n  aproxy logs {port}"));
    }
    let ports: Vec<_> = ports.iter().map(|port| port_value(port)).collect();
    Failure::new("target_ambiguous", message, json!({"ports": ports}))
}

/// 输出日志文件末尾的 `tail_lines` 行；`follow` 为真时随后轮询增量输出（tail -f 语义），
/// 直到实例退出（IPC ping 失败，内部已含 3 次重试判死）或 Ctrl+C。
/// 读文件是同步小量轮询（200ms），期间穿插 IPC 探活（约 1 秒一次）。
async fn print_log_file(
    path: &std::path::Path,
    port: &str,
    tail_lines: usize,
    follow: bool,
) -> Result<(), String> {
    use std::io::{Read, Seek, SeekFrom, Write};

    const POLL_MS: u64 = 200;
    // 末尾回读的窗口：每行按 1 KiB 留量，至少 64 KiB、至多 16 MiB（日志文件本身有轮转上限）
    const TAIL_BYTES_PER_LINE: u64 = 1024;
    const TAIL_WINDOW_MIN: u64 = 64 * 1024;
    const TAIL_WINDOW_MAX: u64 = 16 * 1024 * 1024;
    const PING_EVERY: u32 = 5; // 每 5 轮（约 1 秒）探活一次

    // 等待日志文件出现（实例刚启动时 IPC 已通、日志可能尚未落盘）。限时 5 秒：
    // 守护实例的日志文件在进程启动最先创建，5 秒未出现说明它永远不会有——
    // 典型是 --foreground 实例（日志走控制台，但同样注册注册表、ping 可通），
    // 不能在这里无限等待
    let wait_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
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
        tokio::time::sleep(std::time::Duration::from_millis(POLL_MS)).await;
    }

    // 回读文件末尾的一个窗口，丢弃可能被截断的残行，只显示最后 tail_lines 行
    let window = (tail_lines as u64)
        .saturating_mul(TAIL_BYTES_PER_LINE)
        .clamp(TAIL_WINDOW_MIN, TAIL_WINDOW_MAX);
    let mut file = std::fs::File::open(path).map_err(|e| format!("无法打开日志文件: {e}"))?;
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(window);
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
    let skip = lines.len().saturating_sub(tail_lines);
    for line in &lines[skip..] {
        println!("{}", String::from_utf8_lossy(line));
    }
    if !follow {
        return Ok(());
    }
    let mut pos = len;
    let mut pending: Vec<u8> = Vec::new();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    let mut polls = 0u32;
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(POLL_MS)).await;
        polls += 1;
        if polls.is_multiple_of(PING_EVERY) && daemon::ipc_ping(port).await.is_err() {
            // 实例已退出（ping 内部含判死重试）：冲掉残余半行后结束
            if !pending.is_empty() {
                let _ = out.write_all(&pending);
                let _ = out.flush();
            }
            eprintln!();
            eprintln!("实例已停止，日志跟踪结束。");
            return Ok(());
        }
        let cur_len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(pos);
        if cur_len < pos {
            // 文件被截断（守护启动时 >2MiB 清空）：从头重新跟踪
            eprintln!("── 日志文件已被截断，重新从头跟踪 ──");
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
