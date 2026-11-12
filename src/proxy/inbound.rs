//! 入站来源校验：Host / Origin 白名单与 403（防 DNS 重绑定与网页跨站调用）。

use super::*;

/// 入站来源校验策略：Host 白名单（防 DNS 重绑定）与 Origin 白名单（防网页
/// 跨站调用）。语义的权威说明见 `Config::allowed_hosts` / `allowed_origins`。
///
/// 被拒的请求在 handler 第一步就以 403 本地返回：**绝不转发、绝不注入
/// api_key、绝不进入重试循环**。这是唯一不经上游就终结请求的入口，而它只作用
/// 于从未转发过的请求，「无限重试」对放行的请求毫无改变。
pub(super) struct InboundPolicy {
    /// None = 不做 Host 校验；Some = 只放行这些主机名（已归一：去端口、小写、
    /// IPv6 带方括号）
    pub(super) hosts: Option<Vec<String>>,
    /// None = 不做 Origin 校验；Some = 只放行这些 Origin（已归一：去末尾 `/`、
    /// 小写）。Some(空) = 拒绝一切携带 Origin 的请求（内置默认）
    pub(super) origins: Option<Vec<String>>,
}

/// 被拒请求的拒绝原因（决定 403 文案与日志）
pub(super) enum InboundRejection {
    /// Host 不在白名单（值为请求的 Host，已截断）
    Host(String),
    /// Origin 不在白名单（值为请求的 Origin，已截断）
    Origin(String),
}

/// 回环名单：Host 校验生效时恒放行。这三者只能指向本机，攻击者的域名无论
/// 怎样重绑定都不会以它们出现在 Host 头里。
pub(super) const LOOPBACK_HOSTS: [&str; 3] = ["localhost", "127.0.0.1", "[::1]"];

/// 主机名归一：去端口、小写；IPv6 统一为带方括号形态（Host 头里必然带括号，
/// 配置条目可能写成裸 `::1`）。Host 头与配置条目走同一函数，比较才对称。
/// 配置条目误写成 URL（`http://myhost:8080/`）时取其主机部分——Host 头里
/// 不会出现 `://`，这一步只对配置生效。
pub(super) fn normalize_host(raw: &str) -> String {
    let mut s = raw.trim().to_ascii_lowercase();
    if let Some((_, rest)) = s.split_once("://") {
        s = rest.split('/').next().unwrap_or("").to_string();
    }
    if let Some(rest) = s.strip_prefix('[') {
        return match rest.find(']') {
            Some(end) => format!("[{}]", &rest[..end]),
            None => s,
        };
    }
    if s.matches(':').count() >= 2 {
        return format!("[{s}]");
    }
    match s.split_once(':') {
        Some((host, _port)) => host.to_string(),
        None => s,
    }
}

/// Origin 归一：去首尾空白与末尾 `/`、小写（浏览器发出的 Origin 不带末尾
/// `/`，用户照抄地址栏时常带上）
pub(super) fn normalize_origin(raw: &str) -> String {
    raw.trim().trim_end_matches('/').to_ascii_lowercase()
}

/// 日志/文案里回显攻击者可控的头值：截断到固定长度，防止超长值撑爆日志行。
/// 调用方只传 `HeaderValue::to_str` 成功的值或 URI authority——两者都只含
/// 可见 ASCII（无控制字符注入面），按字节截断也不会切断多字节字符。
pub(super) fn truncate_for_display(s: &str) -> String {
    const MAX: usize = 128;
    if s.len() <= MAX {
        s.to_string()
    } else {
        format!("{}…", &s[..MAX])
    }
}

impl InboundPolicy {
    pub(super) fn from_config(config: &Config) -> Self {
        let wildcard = |list: &[String]| list.iter().any(|e| e.trim() == "*");
        let host_entries = config.allowed_hosts();
        let loopback = config.listen_is_loopback();
        let hosts = if wildcard(host_entries) || (!loopback && host_entries.is_empty()) {
            None
        } else {
            let mut set: Vec<String> = LOOPBACK_HOSTS.iter().map(|h| h.to_string()).collect();
            // 监听地址自身的主机部分：监听 127.0.0.2 / 局域网主机名时，客户端用的
            // 正是它。通配地址（0.0.0.0、[::]）不是任何客户端会写进 Host 的名字
            let listen_host = normalize_host(&config.listen_addr);
            if !listen_host.is_empty() && listen_host != "0.0.0.0" && listen_host != "[::]" {
                set.push(listen_host);
            }
            set.extend(
                host_entries
                    .iter()
                    .map(|h| normalize_host(h))
                    .filter(|h| !h.is_empty()),
            );
            Some(set)
        };
        let origin_entries = config.allowed_origins();
        let origins = if wildcard(origin_entries) {
            None
        } else {
            Some(origin_entries.iter().map(|o| normalize_origin(o)).collect())
        };
        Self { hosts, origins }
    }

    /// 校验一个入站请求；放行返回 None。
    ///
    /// Host 取 Host 头（多值时逐个校验，任一不在白名单即拒）；没有 Host 头时
    /// 退回请求 URI 的 authority（HTTP/2 的 `:authority`）；两者都没有则放行——
    /// 浏览器发出的每个 HTTP/1.1 请求都带 Host（明文 HTTP 下浏览器不用 h2c，
    /// 不存在只有 `:authority` 的浏览器请求），无 Host 的请求只可能来自自己
    /// 拼报文的本机程序，DNS 重绑定这一威胁与它无关。非 ASCII 的 Host 一律拒绝。
    pub(super) fn check(&self, headers: &HeaderMap, uri: &http::Uri) -> Option<InboundRejection> {
        if let Some(allowed) = &self.hosts {
            let mut values: Vec<&str> = Vec::new();
            for v in headers.get_all(http::header::HOST) {
                match v.to_str() {
                    Ok(s) => values.push(s),
                    Err(_) => return Some(InboundRejection::Host("<非 ASCII>".to_string())),
                }
            }
            let authority = uri.authority().map(|a| a.as_str());
            if values.is_empty()
                && let Some(a) = authority
            {
                values.push(a);
            }
            for v in values {
                if !allowed.contains(&normalize_host(v)) {
                    return Some(InboundRejection::Host(truncate_for_display(v)));
                }
            }
        }
        if let Some(allowed) = &self.origins {
            for v in headers.get_all(http::header::ORIGIN) {
                let Ok(s) = v.to_str() else {
                    return Some(InboundRejection::Origin("<非 ASCII>".to_string()));
                };
                if !allowed.contains(&normalize_origin(s)) {
                    return Some(InboundRejection::Origin(truncate_for_display(s)));
                }
            }
        }
        None
    }
}

/// 入站拒绝的 403 响应：文案点名对应配置项与修复方法（用户只看得到这一句，
/// 必须能照着改好）。同时 warn 留痕——路径经脱敏（查询串可能带 key）。
pub(super) fn inbound_rejection_response(
    rejection: InboundRejection,
    method: &http::Method,
    uri: &http::Uri,
) -> Response {
    let path = crate::config::mask_base_url(uri.path_and_query().map_or("/", |pq| pq.as_str()));
    let body = match &rejection {
        InboundRejection::Host(host) => {
            tracing::warn!(method = %method, path = %path, host = %host, "入站请求被拒绝：Host 不在 allowed_hosts 白名单（未转发上游）");
            format!(
                "aProxy 拒绝了该请求（403，未转发上游）：Host「{host}」不在允许列表内。\n\
                 为防 DNS 重绑定，监听回环地址时默认只放行 localhost / 127.0.0.1 / [::1]。\n\
                 若这是你信任的客户端，请把该主机名（不含端口）加入 config.toml 的 allowed_hosts\n\
                 （或 settings.json 的 allowed_hosts 全局默认），例如 allowed_hosts = [\"主机名\"]；\n\
                 写 allowed_hosts = [\"*\"] 可关闭 Host 校验。\n"
            )
        }
        InboundRejection::Origin(origin) => {
            tracing::warn!(method = %method, path = %path, origin = %origin, "入站请求被拒绝：Origin 不在 allowed_origins 白名单（未转发上游）");
            format!(
                "aProxy 拒绝了该请求（403，未转发上游）：来自浏览器页面的请求（Origin「{origin}」）默认不放行，\n\
                 以防网页借本机代理调用上游、花费你的额度。\n\
                 若这是你信任的浏览器/Electron 客户端，请把该 Origin 原样加入 config.toml 的 allowed_origins\n\
                 （或 settings.json 的 allowed_origins 全局默认），例如 allowed_origins = [\"{origin}\"]；\n\
                 写 allowed_origins = [\"*\"] 可关闭 Origin 校验。\n"
            )
        }
    };
    (StatusCode::FORBIDDEN, body).into_response()
}
