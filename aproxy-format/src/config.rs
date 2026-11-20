//! 各角色共享的配置边界：解析错误不引用 TOML 源行，避免把用户填错位置的密钥送入日志。
use aproxy_envelope::{Action, Headers, Report, ReportField, StandardAction};
use serde::de::DeserializeOwned;
use std::path::{Path, PathBuf};
use url::Url;

/// 工具与工作进程都按相同规则读取配置，不向标准错误复制可能含凭据的路径或原文。
pub fn load<T: DeserializeOwned>(path: &str) -> Result<T, &'static str> {
    let path = if let Some(rest) = path.strip_prefix("~/") {
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .ok_or("找不到用户目录")?;
        PathBuf::from(home).join(rest)
    } else {
        Path::new(path).into()
    };
    toml::from_str(&std::fs::read_to_string(path).map_err(|_| "无法读取角色配置")?)
        .map_err(|_| "角色配置无效，请检查字段名称与类型")
}

/// 只允许绝对 HTTP(S) 目的地，不把 URL 自带的凭据或片段带到其他渠道。
pub fn endpoint(value: &str) -> Result<Url, &'static str> {
    let u = Url::parse(value).map_err(|_| "上游地址无效")?;
    if !matches!(u.scheme(), "http" | "https")
        || u.host().is_none()
        || !u.username().is_empty()
        || u.password().is_some()
        || u.fragment().is_some()
    {
        return Err("上游地址无效");
    }
    Ok(u)
}

/// 整表替换鉴权时去掉常用凭据与 Cookie，其余头沿链传递。
pub fn strip_credentials(h: &mut Headers) {
    for name in [
        "authorization",
        "x-api-key",
        "api-key",
        "x-goog-api-key",
        "cookie",
        "proxy-authorization",
    ] {
        h.remove(name);
    }
}

/// 凭据取自用户指名的环境变量，只进入将发给上游的请求头。
pub fn credential(env: &str, scheme: &str) -> Result<String, &'static str> {
    if env.is_empty() || env.contains(['=', '\0']) {
        return Err("密钥环境变量名无效");
    }
    let key = std::env::var(env)
        .ok()
        .filter(|v| !v.is_empty())
        .ok_or("密钥环境变量未设置")?;
    let value = if scheme.is_empty() {
        key
    } else {
        format!("{scheme} {key}")
    };
    http::HeaderValue::from_str(&value).map_err(|_| "鉴权头的值无效")?;
    Ok(value)
}

/// 报告只收类型化的非敏感事实；不插入请求、响应或认证头内容。
pub fn report(summary: &str, action: StandardAction) -> ReportField {
    let mut r = Report::new(summary.into());
    r.actions.push(Action::new(action, None));
    ReportField::Present(r)
}
