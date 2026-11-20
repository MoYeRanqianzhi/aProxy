//! 根包测试启动真正的官方角色，不从 PATH 或相邻包借用可执行文件。

/// 通过标准流运行和发布命令完全相同的角色实现。
fn main() {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() == Some("--replay-keys") {
        aproxy_format::replay_keys_at(
            &args.collect::<Vec<_>>(),
            std::io::stdin().lock(),
            std::io::stdout().lock(),
            std::time::Instant::now(),
            std::time::SystemTime::UNIX_EPOCH,
        )
        .unwrap();
    } else {
        aproxy_format::run();
    }
}
