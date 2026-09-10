//! `aproxy restart [PORT|all|ALIAS|idle [SECS]]`：重启运行中的实例。
//!
//! 语义边界（用户定调）：**只负责重启，不负责启动**——未运行的 target 提示
//! 「未启动」并退出 1，绝不顺手拉起。重启 = 对每个目标实例 stop（原参数或
//! --force）→ 用注册表/恢复记录中的原始启动参数立即 start（与看门狗重拉
//! 同一 `respawn` 语义：IPC 就绪判定后才报成功）。

use aproxy::daemon;

use crate::commands::stop::{StopMode, resolve_stop_targets, stop_instance};

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
    let targets = resolve_stop_targets(target, threshold).await;
    if targets.is_empty() {
        return; // 无运行实例：resolve 已输出提示（restart 不启动新实例）
    }
    for info in &targets {
        restart_instance(info, mode).await;
    }
}

/// 重启单个实例：捕获原始启动参数 → stop → spawn → IPC 就绪判定。
/// 就绪判定按新 pid 定位（见循环处注释）——配置改端口后依然能正确确认。
async fn restart_instance(info: &daemon::InstanceInfo, mode: StopMode) {
    let port = daemon::port_of(&info.listen_addr).to_string();
    let cfg_path = info.config_path.clone();

    // 原始启动参数在 .restore（守护 bind 时写入、优雅退出即删）——必须在
    // stop 之前捕获，否则 --api-key/--listen 等仅本次生效的参数会丢失。
    // .restore 缺失（旧版本守护/记录异常）回退到注册表的 config_path。
    let restore_args: Option<Vec<String>> = daemon::list_restore_entries()
        .into_iter()
        .find(|e| e.port == port)
        .map(|e| e.args);

    if !stop_instance(info, mode).await {
        // 优雅停止未确认退出/无响应：不冒险叠加启动（端口可能仍被占）
        eprintln!("端口 {port} 停止未确认，重启中止。");
        std::process::exit(1);
    }

    // 配置文件被删的实例无法忠实重启（与 restore 清理记录同语义）
    if !std::path::Path::new(&cfg_path).is_file() {
        eprintln!("配置文件已不存在（{cfg_path}），无法重启该实例。");
        std::process::exit(1);
    }

    let exe = std::env::current_exe().expect("无法定位自身可执行文件");
    let mut args = restore_args.unwrap_or_else(|| vec!["--config".to_string(), cfg_path.clone()]);
    args.push("--daemon-child".to_string());
    let new_pid = match daemon::spawn_detached(&exe, &args) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("重启失败（spawn 守护进程失败）: {e}");
            std::process::exit(1);
        }
    };

    // 就绪等待 8 秒（同 start/看门狗重拉）。判定按新 pid 在注册表定位而非
    // ping 旧端口：restart 的用途之一就是「改了 config.toml（含换端口）后让
    // 新配置生效」，新守护监听哪个端口由它读到的配置决定，旧端口 ping 不到
    // 并不代表失败（实测踩坑：改端口重启误报「未就绪」而实例已在新端口运行）。
    // pid 来自 spawn 返回值，唯一可靠锚点；顺带覆盖 --listen 0（系统分配端口）
    // 的场景。用 list_instances_in 而非 list_instances：后者附带孤儿日志清理，
    // 在 APROXY_HOME/APROXY_RUN_DIR 重定向场景（测试/多主目录）live 清单与
    // 真实 logs 目录不一致，会把用户实例的日志误判为孤儿删除。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    loop {
        if let Some(new_info) = daemon::list_instances_in(&daemon::run_dir())
            .await
            .into_iter()
            .find(|i| i.pid == new_pid)
        {
            let new_port = daemon::port_of(&new_info.listen_addr).to_string();
            println!("已重启（端口 {new_port}）");
            println!("  pid: {}（旧 pid {}）", new_info.pid, info.pid);
            println!("  监听: http://{}", new_info.listen_addr);
            return;
        }
        if std::time::Instant::now() > deadline {
            // 端口可能已变（配置改动），旧端口拼出的日志路径不可靠——指向
            // startup.log（bind/配置错误原因落盘处）与 logs 目录两处
            eprintln!(
                "重启后实例（pid {new_pid}）未在预期时间内就绪（配置 {cfg_path}），原因通常记录在:"
            );
            eprintln!("  {}", daemon::logs_dir().join("startup.log").display());
            eprintln!("  {}", daemon::logs_dir().display());
            std::process::exit(1);
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}
