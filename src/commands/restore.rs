//! `aproxy restore`：一键恢复先前非正常退出的实例。

use aproxy::daemon;

/// `aproxy restore`：一键恢复先前非正常退出（崩溃/断电/系统重启）的实例。
///
/// 依据 run/<端口>.restore 恢复记录（守护 bind 成功时写入、优雅退出时删除）。
/// 空清单时静默成功而非报错——本命令的典型用法是配置为开机自启，重启后
/// 「之前全关了」是正常状态而非故障。幂等：已在运行的实例跳过，可重复执行。
pub(crate) async fn handle_restore_cmd() {
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
                // 就绪判定按新 pid 在注册表定位实际端口、再 IPC ping 确认（8 秒，
                // 冷启动受 Defender 扫描等影响可能偏慢）。不能 ping 记录里的端口：
                // 恢复用的是记录里的原参数，但实例监听哪个端口由它此刻读到的配置
                // 决定——用户改了 config.toml 端口却没 restart、或 listen 端口为 0，
                // 新实例都落在别的端口，ping 原端口只会误报「未就绪」
                let run_dir = daemon::run_dir();
                match daemon::wait_spawned_instance_ready(
                    &run_dir,
                    pid,
                    std::time::Duration::from_secs(8),
                )
                .await
                {
                    Ok(live) => {
                        let actual = daemon::port_of(&live.instance.listen_addr).to_string();
                        // 实例落在新端口：原端口的 .restore 是已死旧进程的残留，
                        // 新实例已按实际端口写了自己的记录。不清理的话 stop 新端口
                        // 后下一次 restore（或看护者）会凭它把实例再拉起来
                        if daemon::retire_moved_port_records_in(&run_dir, &entry.port, &actual) {
                            println!(
                                "已恢复 pid {pid}（端口 {actual}；配置的端口已从 {} 变更，原端口的恢复记录已清理）",
                                entry.port
                            );
                        } else {
                            println!("已恢复 pid {pid}（端口 {actual}）");
                        }
                    }
                    Err(why) => {
                        let what = match why {
                            daemon::SpawnNotReady::Exited => "启动后立即退出",
                            daemon::SpawnNotReady::TimedOut => "未在预期时间内就绪",
                        };
                        eprintln!(
                            "端口 {} 的实例（pid {pid}）{what}，原因通常记录在 {}",
                            entry.port,
                            daemon::logs_dir().join("startup.log").display()
                        );
                    }
                }
            }
            Err(e) => eprintln!("端口 {} 恢复失败: {e}", entry.port),
        }
    }
}
