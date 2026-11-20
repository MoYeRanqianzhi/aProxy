//! 各角色共享的配置边界：解析错误不引用 TOML 源行，避免把用户填错位置的密钥送入日志。
use aproxy_envelope::{Action, Headers, Report, ReportField, StandardAction};
use serde::de::DeserializeOwned;
use std::path::{Path, PathBuf};
use url::Url;

/// 同一故障在本地诊断中用中文，在所有协议 1 消费者可见的 fail.reason 中用英文。
/// 两者都只接受静态文案，解析器、环境变量和原始输入不能进入诊断。
pub struct Fault {
    /// 给操作者的 stderr 说明。
    pub local: &'static str,
    /// 不随运行环境语言改变的协议说明。
    pub wire: &'static str,
}
impl Fault {
    /// 在出错的位置一起指定两条消息，避免从一种语言的文本反查另一种。
    pub fn new(local: &'static str, wire: &'static str) -> Self {
        Self { local, wire }
    }
}

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

/// 跨边界转交前按代理日志与历史的同一规则删除全部凭据头，其余元数据保持原样。
pub fn strip_credentials(h: &mut Headers) {
    h.retain(|name, _| !aproxy_ipc::redact::is_secret_header(name));
}

/// 配置指定的凭据头既要能写入 HTTP 请求，也要能在请求历史里仅凭头名就被脱敏。
pub fn credential_header(name: &str) -> Result<http::HeaderName, &'static str> {
    let header = http::HeaderName::from_bytes(name.as_bytes()).map_err(|_| "鉴权头名称无效")?;
    if !aproxy_ipc::redact::is_secret_header(header.as_str()) {
        return Err("鉴权头必须使用请求历史会脱敏的名称");
    }
    Ok(header)
}

/// 凭据取自用户指名的环境变量，只进入将发给上游的请求头。
pub fn credential(env: &str, scheme: &str) -> Result<String, Fault> {
    if env.is_empty() || env.contains(['=', '\0']) {
        return Err(Fault::new(
            "密钥环境变量名无效",
            "invalid credential environment variable name",
        ));
    }
    let key = std::env::var(env)
        .ok()
        .filter(|v| !v.is_empty())
        .ok_or(Fault::new(
            "密钥环境变量未设置",
            "credential environment variable is unset",
        ))?;
    let value = if scheme.is_empty() {
        key
    } else {
        format!("{scheme} {key}")
    };
    http::HeaderValue::from_str(&value)
        .map_err(|_| Fault::new("鉴权头的值无效", "invalid authentication header value"))?;
    Ok(value)
}

/// 报告只收类型化的非敏感事实；不插入请求、响应或认证头内容。
pub fn report(summary: &str, action: StandardAction) -> ReportField {
    let mut r = Report::new(summary.into());
    r.actions.push(Action::new(action, None));
    ReportField::Present(r)
}
