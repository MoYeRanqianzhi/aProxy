//! CLI 侧共用小工具：进程退出与台账登记、时间戳与时长的人性化、从 stdin 读密钥。
//!
//! 密钥与 URL 里的凭据怎样显示不在这里：一律走 `aproxy_ipc::redact` 的统一出口。

/// 删掉本进程的台账条目（永久兼容契约 C3）后退出进程。`std::process::exit` 不跑析构，
/// `aproxy_ipc::ledger::Registration` 管不到这条路，所以登记过台账的进程一律经这里退出；没登记过
/// 的进程调用它也无妨（没有条目可删）。
pub(crate) fn exit(code: i32) -> ! {
    crate::commands::finish_json(code);
    aproxy_ipc::ledger::remove_own();
    std::process::exit(code)
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

/// 时长人性化（秒 → 中文）：uptime 与闲置时长共用
pub(crate) fn humanize_duration(secs: u64) -> String {
    if secs < 60 {
        format!("{secs} 秒")
    } else if secs < 3600 {
        format!("{} 分 {} 秒", secs / 60, secs % 60)
    } else if secs < 86400 {
        format!("{} 小时 {} 分", secs / 3600, (secs % 3600) / 60)
    } else {
        format!("{} 天 {} 小时", secs / 86400, (secs % 86400) / 3600)
    }
}

/// 从 stdin 读一行（去掉行尾的换行）：命令行上值写成 `-` 时用，密钥就不必出现在命令行与
/// shell 历史里。读不到（stdin 已关闭、不是文本）时返回给用户看的原因。
pub(crate) fn read_secret_line() -> Result<String, String> {
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .map_err(|e| format!("从 stdin 读取值失败: {e}"))?;
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
}
