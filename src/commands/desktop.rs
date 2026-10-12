//! `aproxy desktop`：打开桌面界面。
//!
//! 界面是一个独立的长期运行进程，关掉窗口就退出；守护进程永不加载界面。界面以后经 IPC
//! 与实例通信、按显式 home 寻址，不开新的网络端口。这个版本的界面是预览版，里面全是演示
//! 数据（窗口与前端见 aproxy-desktop）。
//!
//! 带界面的构建（desktop 特性）在这里检查、登记台账、建 tokio 运行时，然后把主线程交给
//! Tauri 的事件循环；无界面的 headless 构建也认得这个子命令（帮助里隐藏），只说明这个版本
//! 没有界面、怎样换装，不让用户看到 clap 的「unrecognized subcommand」。
//!
//! 只有非测试构建引用 aproxy_desktop（`#[cfg(all(feature = "desktop", not(test)))]`）：
//! bin 的单元测试程序一旦链接 Tauri，在 Windows 上缺 Common-Controls 清单时于 main 之前就
//! 退出（RT-05，见 aproxy-desktop/Cargo.toml）。单元测试从不执行 main，测试构建里这个子命令
//! 用的是 headless 的那份实现，只为编译通过。

use std::io::Write as _;

/// 无界面构建执行 `aproxy desktop` 时的说明。
#[cfg(any(test, not(feature = "desktop")))]
fn headless_message() -> &'static str {
    if cfg!(target_env = "musl") {
        // musl 只发布 headless：带界面的版本要动态链接 webkit2gtk，跑不了 musl 版的意义
        "这是无界面的 headless 版本的 aproxy，没有桌面界面。musl 构建只发布 headless 版本；\
         在 glibc 的 Linux 发行版上，可以改装 gnu 构建的带界面版本。"
    } else {
        "这是无界面的 headless 版本的 aproxy，没有桌面界面。要用桌面界面，换装带界面的版本：\n  \
         aproxy install --force --variant desktop\n\
         强制安装会结束这个 home 下的全部 aproxy 进程（经它转发的在途请求会中断），装好后按原参数恢复实例。"
    }
}

#[cfg(not(all(feature = "desktop", not(test))))]
pub(crate) fn handle_desktop_cmd() -> ! {
    let _ = writeln!(std::io::stderr(), "{}", headless_message());
    std::process::exit(1)
}

#[cfg(all(feature = "desktop", not(test)))]
pub(crate) fn handle_desktop_cmd() -> ! {
    use aproxy::{ledger, settings};

    // 写失败一律忽略：Windows 上从快捷方式启动时，控制台紧接着就被释放（见下）
    crate::server::init_force_logging();

    // Windows 上 aproxy.exe 是控制台程序。从快捷方式、资源管理器启动时，系统专门为它新建了
    // 一个控制台窗口：立刻释放，窗口随之消失（只闪一下），之后的说明改用对话框。从终端启动时
    // 控制台是与终端共用的：检查都在这里做完（出错就在终端里说明），再分离出一个不带控制台
    // 的子进程承载界面，本进程退出，终端立即可用——分离出的子进程自己有一个看不见的控制台，
    // 走的是前一种情形。
    #[cfg(windows)]
    let attached = console::state();
    #[cfg(windows)]
    let dialogs = attached == console::Console::Exclusive && console::free();
    #[cfg(not(windows))]
    let dialogs = false;
    let fail = |text: &str| -> ! {
        report(text, true, dialogs);
        std::process::exit(1)
    };

    let home = settings::resolve_home().unwrap_or_else(|e| fail(&e));

    // 同一个 home 只开一个界面。alpha 还不会把已开的窗口切到前台，只说明一下
    let me = std::process::id();
    if let Some(running) = ledger::read_all_in(&home)
        .into_iter()
        .map(|found| found.entry)
        .find(|e| e.role == ledger::ROLE_DESKTOP && e.pid != me && ledger::is_alive(e))
    {
        report(
            &format!(
                "aProxy 桌面界面已经在运行（pid {}）。这个版本还不会把它切到前台，请在任务栏里找到它。",
                running.pid
            ),
            false,
            dialogs,
        );
        std::process::exit(0);
    }

    if let Err(e) = aproxy_desktop::preflight() {
        fail(&e.to_string());
    }

    #[cfg(windows)]
    if attached == console::Console::Shared
        && let Ok(exe) = std::env::current_exe()
    {
        match aproxy::daemon::spawn_detached(&exe, &["desktop".to_string()]) {
            Ok(pid) => {
                report(
                    &format!("已打开 aProxy 桌面界面（pid {pid}）。"),
                    false,
                    false,
                );
                std::process::exit(0);
            }
            Err(e) => fail(&format!("无法在后台打开桌面界面：{e}")),
        }
    }

    // 永久兼容契约 C3：在拉起任何子进程之前登记，强制安装据此找到并结束界面进程
    let _ledger = ledger::register(&home, ledger::ROLE_DESKTOP, None, None, None);

    // 与 main 里其余命令一样的兜底：有残留的安装状态就静默拉起续作（看护者不在时靠它）
    if aproxy::install::state::load().is_some()
        && let Ok(exe) = std::env::current_exe()
    {
        let _ = aproxy::daemon::spawn_detached(
            &exe,
            &["install".to_string(), "--continue".to_string()],
        );
    }

    // 与原先 #[tokio::main] 相同的多线程运行时。enter 之后主线程上 Handle::current() 可用；
    // 运行时本身交给 Tauri，它的异步任务跑在这里的工作线程上，事件循环占住主线程
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|e| fail(&format!("无法建立 tokio 运行时：{e}")));
    let _entered = runtime.enter();
    let code = match aproxy_desktop::run(aproxy_desktop::Options {
        data_dir: home.join("desktop").join("webview"),
        runtime: runtime.handle().clone(),
    }) {
        Ok(code) => code,
        Err(e) => {
            report(&e.to_string(), true, dialogs);
            1
        }
    };
    // 删掉台账条目再退出（std::process::exit 不跑析构，守卫管不到这条路）
    ledger::exit(code)
}

/// 告诉用户一件事：打到终端（错误到 stderr），控制台已经释放时再弹对话框。写失败一律忽略。
#[cfg(all(feature = "desktop", not(test)))]
fn report(text: &str, error: bool, dialog: bool) {
    if error {
        let _ = writeln!(std::io::stderr(), "[ERROR] {text}");
    } else {
        let _ = writeln!(std::io::stdout(), "{text}");
    }
    #[cfg(windows)]
    if dialog {
        aproxy_desktop::message_box(text, error);
    }
    #[cfg(not(windows))]
    let _ = dialog;
}

/// 本进程与控制台的关系（Windows）。
#[cfg(all(windows, feature = "desktop", not(test)))]
mod console {
    use windows_sys::Win32::System::Console::{FreeConsole, GetConsoleProcessList};

    #[derive(Clone, Copy, PartialEq, Eq)]
    pub(super) enum Console {
        /// 与别的进程（终端里的 shell）共用：从终端启动
        Shared,
        /// 只有本进程：控制台是启动时系统专门为本进程新建的（快捷方式、资源管理器、运行框）
        Exclusive,
        /// 没有控制台
        Absent,
    }

    pub(super) fn state() -> Console {
        // 只关心是不是不止一个，缓冲区小了也照样返回总数
        let mut pids = [0u32; 2];
        match unsafe { GetConsoleProcessList(pids.as_mut_ptr(), pids.len() as u32) } {
            0 => Console::Absent,
            1 => Console::Exclusive,
            _ => Console::Shared,
        }
    }

    /// 释放控制台（只有本进程用它时，它的窗口随之关闭）。之后写标准输出与标准错误都静默
    /// 成功（std 在 Windows 上把无效句柄的写入当作成功）。
    pub(super) fn free() -> bool {
        unsafe { FreeConsole() != 0 }
    }
}

#[cfg(test)]
mod tests {
    use crate::cli::{Cli, Commands};
    use clap::{CommandFactory, Parser};

    #[test]
    fn desktop_is_a_subcommand_in_every_build() {
        let cli = Cli::try_parse_from(["aproxy", "desktop"]).expect("desktop 必须能解析");
        assert_eq!(cli.command, Some(Commands::Desktop));
    }

    /// 带界面的构建在帮助里列出 desktop；headless 构建隐藏它（仍然认得，见上一个测试）
    #[test]
    fn desktop_is_listed_in_help_only_with_the_desktop_feature() {
        let help = Cli::command().render_help().to_string();
        let listed = help
            .lines()
            .any(|line| line.trim_start().starts_with("desktop "));
        assert_eq!(listed, cfg!(feature = "desktop"), "{help}");
    }

    /// headless 构建的说明要给出能照着执行的换装命令（musl 没有带界面的版本，不给）
    #[test]
    fn headless_message_names_the_way_out() {
        let text = super::headless_message();
        assert!(text.contains("headless"), "{text}");
        if !cfg!(target_env = "musl") {
            assert!(
                text.contains("aproxy install --force --variant desktop"),
                "{text}"
            );
        }
    }
}
