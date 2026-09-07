//! CLI 侧共用小工具：时间戳、时长人性化、敏感值展示打码、key=value 解析。
//!
//! 全部是纯展示/纯解析函数（无 IO、无状态），供各子命令模块共用；
//! 与 lib 侧 `config::mask_base_url` 的打码策略保持一致（展示绝不泄露凭据）。

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

pub(crate) fn humanize_uptime(started_at: u64) -> String {
    humanize_duration(now_unix().saturating_sub(started_at))
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

/// 密钥类值的展示打码：保留前 6 个字符 + "***"（按字符截断，避免多字节字符在字节边界 panic）。
pub(crate) fn mask_secret(s: &str) -> String {
    let prefix: String = s.chars().take(6).collect();
    format!("{prefix}***")
}

/// 对代理 URL 中的密码打码（`http://user:***@host:port`），仅用于展示，绝不输出真实密码。
pub(crate) fn mask_proxy_url(raw: &str) -> String {
    // 解析失败时原样返回会泄露内嵌密码，只回显已隐去的安全占位
    let Ok(url) = url::Url::parse(raw) else {
        return "<无法解析的代理配置，已隐去>".to_string();
    };
    let user = url.username();
    if user.is_empty() && url.password().is_none() {
        return raw.to_string();
    }
    let host = url.host_str().unwrap_or("");
    let port = url.port().map(|p| format!(":{p}")).unwrap_or_default();
    // 仅用户名无密码时不渲染 ":***@"，避免让人误以为配置了密码
    let auth = if url.password().is_some() {
        format!("{user}:***@")
    } else {
        format!("{user}@")
    };
    let mut masked = format!("{}://{}{}{}", url.scheme(), auth, host, port);
    if let Some(q) = url.query() {
        masked.push('?');
        masked.push_str(q);
    }
    masked
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
    fn mask_proxy_url_variants() {
        // 无凭据原样返回
        assert_eq!(
            mask_proxy_url("http://127.0.0.1:7890"),
            "http://127.0.0.1:7890"
        );
        // 有密码打码
        assert_eq!(
            mask_proxy_url("http://alice:secret@127.0.0.1:7890"),
            "http://alice:***@127.0.0.1:7890"
        );
        // 仅用户名不加 ":***@"
        assert_eq!(
            mask_proxy_url("socks5://alice@127.0.0.1:1080"),
            "socks5://alice@127.0.0.1:1080"
        );
        // 解析失败回安全占位而非原文（原文可能含密码）
        assert_eq!(mask_proxy_url("not a url"), "<无法解析的代理配置，已隐去>");
        // query 保留
        assert_eq!(
            mask_proxy_url("http://alice:pw@h:1?p=x"),
            "http://alice:***@h:1?p=x"
        );
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
