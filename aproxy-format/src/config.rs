//! 各角色共享的配置边界：解析错误不引用 TOML 源行，避免把用户填错位置的密钥送入日志。
use aproxy_envelope::{Action, Headers, Report, ReportField, StandardAction};
use serde::de::DeserializeOwned;
use std::path::{Path, PathBuf};
use url::Url;

pub(crate) use crate::role_text::Fault;
use crate::text;
use aproxy_ipc::i18n::Lang;

/// 角色加载错误有两种读者。只有三个离线文件类别和公开词表里的字段可从工具原样展示；
/// 工作进程可报告自己的加载原因，但其环境名等不能出现在静态检查的诊断中。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LoadIssue {
    /// 白名单中的字段名称，不来自用户的原文。
    Field(&'static str),
    /// 输入不是可安全读取的本地普通文件。
    File,
    /// 按已持有的文件句柄读取时超过限额。
    Size,
    /// 离线读取的角色 TOML 不是文本。
    Utf8,
    /// 仅供角色工作进程自己诊断，不向离线检查者转述。
    Other(String),
}
impl LoadIssue {
    /// 按分类挑选可交付的文字，不让任意带有相同前缀的环境名变成可信的字段诊断。
    pub(super) fn safe_issue(&self, lang: Lang) -> String {
        match self {
            Self::Field(field) => format!("{}{field}", text::field(lang)),
            Self::File => text::file(lang).into(),
            Self::Size => text::size(lang).into(),
            Self::Utf8 => text::utf8(lang).into(),
            Self::Other(_) => text::invalid_role(lang).into(),
        }
    }
    /// 直接运行角色时写入工作进程 stderr 的英文。
    pub(super) fn worker(self) -> String {
        match self {
            Self::Other(s) => s,
            other => other.safe_issue(Lang::En),
        }
    }
}
impl From<&str> for LoadIssue {
    fn from(s: &str) -> Self {
        Self::Other(s.into())
    }
}
impl From<String> for LoadIssue {
    fn from(s: String) -> Self {
        Self::Other(s)
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
            .ok_or("cannot locate the user directory")?;
        Ok(PathBuf::from(home).join(rest))
    } else {
        Ok(Path::new(path).into())
    }
}
/// 工具与工作进程共用配置校验，不向标准错误复制可能含凭据的路径或原文。
pub fn load<T: DeserializeOwned>(source: Source<'_>) -> Result<T, LoadIssue> {
    let owned;
    let text = match source {
        Source::Worker(path) => {
            owned = std::fs::read_to_string(role_path(path)?)
                .map_err(|_| "cannot read role configuration")?;
            &owned
        }
        Source::Offline(text) => text,
    };
    let generic = "invalid role configuration; check field names and types";
    let de = toml::de::Deserializer::parse(text).map_err(|_| generic)?;
    serde_path_to_error::deserialize(de).map_err(|err| {
        // 路径中可能有用户自定义的头名、模型名或 JSON 指针。只从公开配置词表选一个诊断，绝不回显原段。
        macro_rules! known { ($key:expr; $($name:literal),+) => { match $key {$($name=>Some($name),)+ _=>None} }; }
        let mut field = None;
        for (depth, segment) in err.path().iter().enumerate() {
            let serde_path_to_error::Segment::Map { key } = segment else { continue; };
            field = known!(key.as_str();"aliases","auth_header","body_regex","carry_query","channel","cooldown_secs","cooldown_status","credential_env","decision","delay_ms","env","event","failure","failure_threshold","from","header","hold_secs","id","keys","max_entries","model","models","origin","overrides","path","path_prefix","pool","preserve_path","preset","priority","protocol","recovery_latency_ms","recovery_samples","recovery_success_ratio","regex","remove","remove_headers","reroute","respect_retry_after","response","rule","scheme","set","set_headers","status","sticky","strategy","stream_budget_bytes","to","ttl_secs","upstream","url","weight").or(field);
            // 根级 models 是改写映射，channel 中的 models 则是匹配器列表。
            // 表边界之后的成员名是操作者的数据，即便恰好与别处的字段同名也不能回显。
            if matches!(key.as_str(), "aliases" | "overrides" | "set" | "set_headers") || depth == 0 && key == "models" { break; }
        }
        field.map(LoadIssue::Field).unwrap_or_else(||generic.into())
    })
}

/// 只允许绝对 HTTP(S) 目的地，不把 URL 自带的凭据或片段带到其他渠道。
pub fn endpoint(value: &str) -> Result<Url, &'static str> {
    let u = Url::parse(value).map_err(|_| Fault::UpstreamUrl.wire())?;
    if !matches!(u.scheme(), "http" | "https")
        || u.host().is_none()
        || !u.username().is_empty()
        || u.password().is_some()
        || u.fragment().is_some()
    {
        return Err(Fault::UpstreamUrl.wire());
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
            .expect("strings must serialize to JSON")
            .len()
    }
}

/// 跨边界转交前按代理日志与历史的同一规则删除全部凭据头，其余元数据保持原样。
pub fn strip_credentials(h: &mut Headers) {
    h.retain(|name, _| !aproxy_ipc::redact::is_secret_header(name));
}

/// 配置指定的凭据头既要能写入 HTTP 请求，也要能在请求历史里仅凭头名就被脱敏。
pub fn credential_header(name: &str) -> Result<http::HeaderName, &'static str> {
    let header = http::HeaderName::from_bytes(name.as_bytes())
        .map_err(|_| "invalid authentication header name")?;
    if aproxy_ipc::redact::is_managed_request_header(header.as_str()) {
        return Err("authentication headers must not use a proxy-managed name");
    }
    if !aproxy_ipc::redact::is_secret_header(header.as_str()) {
        return Err("authentication headers must use a name redacted in request history");
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
impl Variables {
    /// 与启动子进程时的 envs 覆盖次序一致；Windows 环境名按系统序数比较，不按当前语言排序。
    fn get(&self, name: &str) -> Option<&String> {
        #[cfg(not(windows))]
        {
            self.overlay.get(name)
        }
        #[cfg(windows)]
        {
            use windows_sys::Win32::Globalization::{CSTR_EQUAL, CompareStringOrdinal};
            let needle: Vec<_> = name.encode_utf16().collect();
            self.overlay.iter().rev().find_map(|(key, value)| {
                let key: Vec<_> = key.encode_utf16().collect();
                // SAFETY: 长度按 UTF-16 单元计数，两个缓冲区均活过整个调用。
                let same = unsafe {
                    CompareStringOrdinal(
                        key.as_ptr(),
                        key.len().try_into().ok()?,
                        needle.as_ptr(),
                        needle.len().try_into().ok()?,
                        1,
                    )
                };
                (same == CSTR_EQUAL).then_some(value)
            })
        }
    }
}
/// 环境变量名是系统接口的一部分，空名、等号与 NUL 不能表示一个凭据来源。
pub fn credential_name(env: &str) -> Result<(), Fault> {
    if env.is_empty() || env.contains(['=', '\0']) {
        return Err(Fault::CredentialName);
    }
    Ok(())
}
/// 凭据取自用户指名的环境变量，只进入将发给上游的请求头。
pub fn credential(env: &str, scheme: &str, variables: &Variables) -> Result<String, Fault> {
    credential_name(env)?;
    let key = if variables.structural {
        Some("offline-placeholder".to_owned())
    } else {
        variables
            .get(env)
            .cloned()
            .or_else(|| std::env::var(env).ok())
    }
    .filter(|v| !v.is_empty())
    .ok_or(Fault::CredentialMissing)?;
    let value = if scheme.is_empty() {
        key
    } else {
        format!("{scheme} {key}")
    };
    if !http::HeaderValue::from_str(&value).is_ok_and(|h| h.to_str().is_ok()) {
        return Err(Fault::AuthValue);
    }
    Ok(value)
}

/// 报告只收类型化的非敏感事实；不插入请求、响应或认证头内容。
pub fn report(summary: &str, action: StandardAction) -> ReportField {
    let mut r = Report::new(summary.into());
    r.actions.push(Action::new(action, None));
    ReportField::Present(r)
}

/// Windows 创建的子进程只有一个不分大小写的变量槽；试跑覆盖顺序须与 Command::envs 相同。
#[cfg(all(test, windows))]
#[test]
fn credential_overlay_uses_windows_name_ordering() {
    let variables = Variables {
        overlay: [
            ("T06_Ä_KEY".into(), "first".into()),
            ("t06_ä_key".into(), "last".into()),
        ]
        .into(),
        structural: false,
    };
    assert_eq!(
        credential("T06_Ä_KEY", "", &variables).map_err(|e| e.wire()),
        Ok("last".into())
    );
}

/// 即使外部诊断伪装成公开字段错误，显示端也只信类型，不信前缀。
#[test]
fn offline_diagnostics_only_reveal_typed_fields() {
    let private = LoadIssue::Other("角色配置字段无效：operator-secret".into());
    for (lang, expected) in [
        (Lang::En, "Invalid role configuration field: model"),
        (Lang::Zh, "角色配置字段无效：model"),
    ] {
        assert_eq!(LoadIssue::Field("model").safe_issue(lang), expected);
        assert!(!private.safe_issue(lang).contains("operator-secret"));
    }
}
