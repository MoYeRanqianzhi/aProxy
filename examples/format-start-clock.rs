//! 只在启动窗口测试中冻结服务进程的 Tokio 时钟。启动者和旁观者仍运行实际的命令逻辑。
#[path = "../src/main.rs"]
mod entry;
use clap::Parser;
use entry::{commands, server, util};
use std::time::Duration;

/// 服务端在家目录的闸门放行前不走时；只有超时场景显式推进到 hello 截止点。
fn main() -> std::process::ExitCode {
    use aproxy::cli::{Cli, Commands};
    let Some(Commands::Serve { home, spec }) = Cli::parse().command else {
        return entry::main();
    };
    assert_eq!(home, aproxy_ipc::home::home());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .start_paused(true)
        .build()
        .unwrap();
    let code = runtime.block_on(async move {
        let (done, cancelled) = std::sync::mpsc::channel::<()>();
        let advance = home.join("advance-hello");
        let resume = home.join("resume-clock");
        // 阻塞任务等待期间时钟不自动跳到下一定时器；服务提前退出时取消等待，不留下线程。
        let mut control = tokio::task::spawn_blocking(move || {
            loop {
                if advance.is_file() {
                    break true;
                }
                if resume.is_file() {
                    break false;
                }
                if cancelled.recv_timeout(Duration::from_millis(10))
                    != Err(std::sync::mpsc::RecvTimeoutError::Timeout)
                {
                    break false;
                }
            }
        });
        let service = commands::start::handle_serve_cmd(&spec);
        tokio::pin!(service);
        tokio::select! {
            code = &mut service => { drop(done); control.await.unwrap(); code }
            tick = &mut control => {
                if tick.unwrap() { tokio::time::advance(Duration::from_secs(11)).await; }
                tokio::time::resume();
                service.await
            }
        }
    });
    std::process::ExitCode::from(code)
}
