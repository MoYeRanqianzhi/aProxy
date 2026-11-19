//! CLI 侧共用小工具：时间戳、时长人性化、敏感值展示打码、key=value 解析。
//!
//! 全部是纯展示/纯解析函数（无 IO、无状态），供各子命令模块共用。
//! URL 形态的凭据（base_url、代理 URL 等）一律走 lib 侧的统一出口
//! `aproxy_ipc::redact::url`；本模块只管不透明的密钥值（api_key、头值等）。

/// 删掉本进程的台账条目（永久兼容契约 C3）后退出进程。`std::process::exit` 不跑析构，
/// `aproxy_ipc::ledger::Registration` 管不到这条路，所以登记过台账的进程一律经这里退出；没登记过
/// 的进程调用它也无妨（没有条目可删）。
pub(crate) fn exit(code: i32) -> ! {
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

/// 密钥类值的展示打码：保留前 6 个字符 + "***"（按字符截断，避免多字节字符在字节边界 panic）。
pub(crate) fn mask_secret(s: &str) -> String {
    let prefix: String = s.chars().take(6).collect();
    format!("{prefix}***")
}

/// 解析 `key=value` 形式的命令行参数（--extra-header/--override-header 共用）：
/// 只按第一个 `=` 切分（值中允许出现 `=`），键去空白且不得为空。
pub(crate) fn parse_kv(s: &str) -> Option<(String, String)> {
    let (k, v) = s.split_once('=')?;
    let k = k.trim();
    let v = v.trim();
    if k.is_empty() {
        return None;
    }
    Some((k.to_string(), v.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_secret_keeps_prefix_and_masks_rest() {
        // 保留前 6 个字符
        assert_eq!(mask_secret("sk-ant-api03-abcdef"), "sk-ant***");
        assert_eq!(mask_secret("abc"), "abc***");
        assert_eq!(mask_secret(""), "***");
    }

    #[test]
    fn mask_secret_multibyte_no_panic() {
        // 按 char 截断，多字节字符不会在字节边界 panic
        assert_eq!(mask_secret("你好世界，测试"), "你好世界，测***");
    }

    #[test]
    fn parse_kv_variants() {
        assert_eq!(
            parse_kv("x-key = some value"),
            Some(("x-key".into(), "some value".into()))
        );
        // 值中允许 '='：只按第一个 '=' 切分
        assert_eq!(
            parse_kv("authorization=Bearer a=b"),
            Some(("authorization".into(), "Bearer a=b".into()))
        );
        assert_eq!(parse_kv("no-equals"), None);
        assert_eq!(parse_kv("  =value"), None);
    }
}
