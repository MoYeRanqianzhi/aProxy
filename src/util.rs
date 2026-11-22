//! CLI 侧共用小工具：进程退出与台账登记、时间戳与时长的人性化、从 stdin 读密钥。
//!
//! 密钥与 URL 里的凭据怎样显示不在这里：一律走 `aproxy_ipc::redact` 的统一出口。

#[path = "i18n/terminal.rs"]
mod text;
use aproxy_ipc::i18n::current;

/// 删掉本进程的台账条目（永久兼容契约 C3）后退出进程。`std::process::exit` 不跑析构，
/// `aproxy_ipc::ledger::Registration` 管不到这条路，所以登记过台账的进程一律经这里退出；没登记过
/// 的进程调用它也无妨（没有条目可删）。
pub(crate) fn exit(code: i32) -> ! {
    crate::commands::finish_json(code);
    aproxy_ipc::ledger::remove_own();
    std::process::exit(code)
}

/// 仅在已耗尽停机宽限并回收子进程时放弃仍未完成的任务。Windows 的普通进程退出会
/// 在其他线程被终止后继续运行 DLL 清理；此处不能等待阻塞线程，也不能让清理代码再使用其堆。
pub(crate) fn exit_after_shutdown_deadline() -> ! {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
        aproxy_ipc::ledger::remove_own();
        // SAFETY: 伪句柄始终代表本进程，不按 PID 打开或终止任何其他进程。
        unsafe {
            TerminateProcess(GetCurrentProcess(), 0);
        }
        std::process::abort();
    }
    #[cfg(not(windows))]
    exit(0)
}

/// 把本进程登记进台账（永久兼容契约 C3，见 `aproxy_ipc::ledger::register`），登记不成时
/// 告警。登记不成不是错误，进程照常运行，只是强制安装看不见它——所以要留下一条日志。
pub(crate) fn register_in_ledger(
    home: &std::path::Path,
    role: &str,
    port: Option<u16>,
    label: Option<&str>,
    config: Option<&std::path::Path>,
) -> aproxy_ipc::ledger::Registration {
    // 登记会往 home 里写（home 不存在时还会把它建出来）：先保证它只让属主访问
    aproxy::fsx::ensure_private_home_for(home);
    let registration = aproxy_ipc::ledger::register(home, role, port, label, config);
    if let Some(failure) = registration.failure() {
        tracing::warn!(
            error = %failure,
            role,
            "failed to write the process ledger entry; continuing without it (install --force will not see this process)"
        );
    }
    registration
}

/// 当前 Unix 秒（系统时钟早于 epoch 时回退 0——仅用于展示，不影响逻辑）
pub(crate) fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 日志时间戳（无 chrono 依赖，仅用于 startup.log 行前缀）
pub(crate) fn chrono_like_timestamp() -> String {
    // 简单格式化：秒级 Unix 时间足以定位启动失败的顺序
    format!("unix+{}s", now_unix())
}

/// 时长人性化（以秒为输入）：uptime 与闲置时长共用
pub(crate) fn humanize_duration(secs: u64) -> String {
    if secs < 60 {
        text::seconds(current(), secs)
    } else if secs < 3600 {
        text::minutes(current(), secs / 60, secs % 60)
    } else if secs < 86400 {
        text::hours(current(), secs / 3600, (secs % 3600) / 60)
    } else {
        text::days(current(), secs / 86400, (secs % 86400) / 3600)
    }
}

/// 从 stdin 读一行（去掉行尾的换行）：命令行上值写成 `-` 时用，密钥就不必出现在命令行与
/// shell 历史里。读不到（stdin 已关闭、不是文本）时返回给用户看的原因。
pub(crate) fn read_secret_line() -> Result<String, crate::commands::Failure> {
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).map_err(|e| {
        crate::commands::Failure::rendered(
            "command_failed",
            |lang| text::stdin(lang, &e),
            serde_json::json!({}),
        )
    })?;
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
}
