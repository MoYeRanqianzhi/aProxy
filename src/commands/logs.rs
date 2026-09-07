//! `aproxy logs [PORT]`：实时跟踪运行实例的守护日志。

use aproxy::daemon;

/// `aproxy logs [PORT]`：连接到运行中的实例并实时输出其守护日志。
/// 语义与 stop 一致：单实例可省略端口；多实例必须指定；不支持 all
/// （一次只能连接一个实例，传入 all 视为端口解析失败）。
pub(crate) async fn handle_logs_cmd(target: Option<String>) {
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
        tokio::time::sleep(std::time::Duration::from_millis(POLL_MS)).await;
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
