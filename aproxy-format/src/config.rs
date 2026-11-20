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

/// 工具提供已安全读取的文本，而工作进程仍按操作者原有的路径规则打开文件。
#[derive(Clone, Copy)]
pub enum Source<'a> {
    /// 真正的工作进程自行读取配置，保留对其部署文件系统的支持。
    Worker(&'a str),
    /// 离线工具只解析已经过大小和文件类型检查的同一份内容，不再按名称重开。
    Offline(&'a str),
}
/// 工具与工作进程使用同一套用户目录简写。
pub fn role_path(path: &str) -> Result<PathBuf, &'static str> {
    if let Some(rest) = path.strip_prefix("~/") {
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .ok_or("找不到用户目录")?;
        Ok(PathBuf::from(home).join(rest))
    } else {
        Ok(Path::new(path).into())
    }
}
/// 工具与工作进程共用配置校验，不向标准错误复制可能含凭据的路径或原文。
pub fn load<T: DeserializeOwned>(source: Source<'_>) -> Result<T, &'static str> {
    let owned;
    let text = match source {
        Source::Worker(path) => {
            owned = std::fs::read_to_string(role_path(path)?).map_err(|_| "无法读取角色配置")?;
            &owned
        }
        Source::Offline(text) => text,
    };
    let generic = "角色配置无效，请检查字段名称与类型";
    let de = toml::de::Deserializer::parse(text).map_err(|_| generic)?;
    serde_path_to_error::deserialize(de).map_err(|err| {
        // 路径中可能有用户自定义的头名、模型名或 JSON 指针。只从公开配置词表选一个诊断，绝不回显原段。
        macro_rules! known { ($key:expr; $($name:literal),+) => { match $key {$($name=>Some(concat!("角色配置字段无效：",$name)),)+ _=>None} }; }
        let mut field = None;
        for (depth, segment) in err.path().iter().enumerate() {
            let serde_path_to_error::Segment::Map { key } = segment else { continue; };
            field = known!(key.as_str();"aliases","auth_header","body_regex","carry_query","channel","cooldown_secs","cooldown_status","credential_env","decision","delay_ms","env","event","failure","failure_threshold","from","header","hold_secs","id","keys","max_entries","model","models","origin","overrides","path","path_prefix","pool","preserve_path","preset","priority","protocol","recovery_latency_ms","recovery_samples","recovery_success_ratio","regex","remove","remove_headers","reroute","respect_retry_after","response","rule","scheme","set","set_headers","status","sticky","strategy","stream_budget_bytes","to","ttl_secs","upstream","url","weight").or(field);
            // 根级 models 是改写映射，channel 中的 models 则是匹配器列表。
            // 表边界之后的成员名是操作者的数据，即便恰好与别处的字段同名也不能回显。
            if matches!(key.as_str(), "aliases" | "overrides" | "set" | "set_headers") || depth == 0 && key == "models" { break; }
        }
        field.unwrap_or(generic)
    })
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

/// 私有 state 受协议字节上限约束，两个字符串、分隔符与转义字节都算在内。
pub const STATE_BYTES: usize = 64 * 1024;
/// 只为可能装得下的字符串展开转义；明显超限时不分配第二份超长配置值。
pub fn state_string_bytes(value: &str) -> usize {
    if value.len() > STATE_BYTES {
        STATE_BYTES + 1
    } else {
        serde_json::to_string(value)
            .expect("字符串总能编码为 JSON")
            .len()
    }
}

/// 跨边界转交前按代理日志与历史的同一规则删除全部凭据头，其余元数据保持原样。
pub fn strip_credentials(h: &mut Headers) {
    h.retain(|name, _| !aproxy_ipc::redact::is_secret_header(name));
}

/// 配置指定的凭据头既要能写入 HTTP 请求，也要能在请求历史里仅凭头名就被脱敏。
pub fn credential_header(name: &str) -> Result<http::HeaderName, &'static str> {
    let header = http::HeaderName::from_bytes(name.as_bytes()).map_err(|_| "鉴权头名称无效")?;
    if aproxy_envelope::is_managed_request_header(header.as_str()) {
        return Err("鉴权头不能使用代理管理的名称");
    }
    if !aproxy_ipc::redact::is_secret_header(header.as_str()) {
        return Err("鉴权头必须使用请求历史会脱敏的名称");
    }
    Ok(header)
}

/// 工作进程继承真实环境；离线角色另外看到自己条目的环境，不修改整个检查进程的环境。
#[derive(Default, Clone)]
pub struct Variables {
    /// 只在这个角色内可见的覆盖，不能向其他角色传播。
    pub overlay: std::collections::BTreeMap<String, String>,
    /// 静态检查不要求提供密钥；实际执行或试跑遇到缺失凭据仍报错。
    pub structural: bool,
}
/// 凭据取自用户指名的环境变量，只进入将发给上游的请求头。
pub fn credential(env: &str, scheme: &str, variables: &Variables) -> Result<String, Fault> {
    if env.is_empty() || env.contains(['=', '\0']) {
        return Err(Fault::new(
            "密钥环境变量名无效",
            "invalid credential environment variable name",
        ));
    }
    let key = if variables.structural {
        Some("offline-placeholder".to_owned())
    } else {
        variables
            .overlay
            .get(env)
            .cloned()
            .or_else(|| std::env::var(env).ok())
    }
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
    if !http::HeaderValue::from_str(&value).is_ok_and(|h| h.to_str().is_ok()) {
        return Err(Fault::new(
            "鉴权头的值无效",
            "invalid authentication header value",
        ));
    }
    Ok(value)
}

/// 报告只收类型化的非敏感事实；不插入请求、响应或认证头内容。
pub fn report(summary: &str, action: StandardAction) -> ReportField {
    let mut r = Report::new(summary.into());
    r.actions.push(Action::new(action, None));
    ReportField::Present(r)
}
