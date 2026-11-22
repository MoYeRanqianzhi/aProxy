//! 绑定单个主目录的桌面业务逻辑。私有文件只在宿主侧打开，界面只见字段投影与不透明令牌。
mod forms;
mod instances;
mod login;
mod preferences;
mod state;
mod update;
use crate::{config, settings, spec};
use aproxy_ipc::{
    desktop::{Fault, LocalFacts, R},
    enumerate::Enumerated,
    redact::Secrets,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};

/// 一个窗口持有一个宿主：所有观察与之后的写入都由同一主目录和同一令牌密钥约束。
#[derive(Clone)]
pub struct DesktopHome {
    home: PathBuf,
    key: [u8; 32],
    exe: PathBuf,
    state_serial: Arc<tokio::sync::Mutex<()>>,
    login_test_slot: Option<String>,
    updates: Arc<std::sync::Mutex<update::Checks>>,
    listing: Arc<std::sync::Mutex<Option<[u8; 16]>>>,
}
impl DesktopHome {
    /// 启动命令传入已解析的绝对目录；不借助修改全局环境隔离不同窗口。
    pub fn new(home: PathBuf, exe: PathBuf) -> R<Self> {
        if !home.is_absolute() || !exe.is_absolute() {
            return Err(Fault::DesktopBadInput {});
        }
        let mut key = [0; 32];
        getrandom::fill(&mut key).map_err(|_| Fault::DesktopUnavailable {})?;
        let host = Self {
            home,
            key,
            exe,
            state_serial: Default::default(),
            login_test_slot: None,
            listing: Default::default(),
            updates: Default::default(),
        };
        Ok(host)
    }
    fn restore_token(&self, entry: &spec::RestoreEntry) -> String {
        // 外层只有固定长度的随机密钥与内部摘要，原始序列化及其无密钥散列均不离开宿主。
        let bytes = serde_json::to_vec(&(&entry.port, entry.written_at, &entry.spec))
            .expect("instance specification must serialize");
        self.token(&bytes)
    }
    fn token(&self, bytes: &[u8]) -> String {
        let digest = Sha256::new()
            .chain_update(self.key)
            .chain_update(Sha256::digest(bytes))
            .finalize();
        use std::fmt::Write;
        let mut token = String::with_capacity(digest.len() * 2);
        for byte in digest {
            write!(&mut token, "{byte:02x}").expect("writing to a String cannot fail");
        }
        token
    }
    /// 在发现线程里解析本次成员的敏感值、别名与恢复令牌。无法检查的文件不使页面降级为明文。
    pub fn inspect(&self, members: &[Enumerated]) -> BTreeMap<u16, LocalFacts> {
        let settings_path = settings::settings_path_in(&self.home);
        let raw = match std::fs::read(&settings_path) {
            Ok(b)
                if std::str::from_utf8(&b).is_ok_and(|text| {
                    !settings::file::parse(text, &settings_path)
                        .1
                        .iter()
                        .any(|d| d.is_error())
                }) =>
            {
                match serde_json::from_slice::<Value>(&b) {
                    Ok(v) => v,
                    Err(_) => return BTreeMap::new(),
                }
            }
            Ok(_) => return BTreeMap::new(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => serde_json::json!({}),
            Err(_) => return BTreeMap::new(),
        };
        let mut common = Vec::new();
        collect_settings_secrets(&raw, &mut common);
        let aliases = raw["aliases"].as_object();
        let run_dir = self.home.join("run");
        members
            .iter()
            .filter_map(|item| {
                let port = item.port.to_string();
                let desire = spec::read_restore_in(&run_dir, &port).ok()?;
                let mut secrets = common.clone();
                let mut headers = configured_headers(&raw["defaults"]);
                let mut paths = std::collections::BTreeSet::new();
                let record_path = item
                    .instance
                    .as_ref()
                    .and_then(|r| r.config_path.as_deref())
                    .map(Path::new);
                paths.extend(record_path);
                if let Some(e) = &desire {
                    let overrides = serde_json::to_value(&e.spec.overrides).ok()?;
                    headers.extend(configured_headers(&overrides));
                    collect_config_secrets(&overrides, &mut secrets);
                    paths.extend(e.spec.config_path.as_deref().map(Path::new));
                    for (k, v) in &e.spec.env {
                        if let Some(s) = v {
                            secrets.push(s.clone());
                            if k.to_ascii_lowercase().ends_with("proxy") {
                                collect_url(s, &mut secrets);
                            }
                        }
                    }
                }
                for path in paths {
                    if !path.is_absolute() {
                        return None;
                    }
                    match std::fs::read_to_string(path) {
                        Ok(text) => {
                            if config::file::parse(&text, path)
                                .1
                                .iter()
                                .any(|d| d.is_error())
                            {
                                return None;
                            }
                            let doc = config::view::file_value(&text)?;
                            headers.extend(configured_headers(&doc));
                            collect_config_secrets(&doc, &mut secrets)
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(_) => return None,
                    }
                }
                let mut names = Vec::new();
                if let (Some(target), Some(a)) =
                    (record_path.and_then(|p| p.canonicalize().ok()), aliases)
                {
                    let target = settings::path_match_key(&target.to_string_lossy());
                    for (k, v) in a {
                        if v.as_str()
                            .and_then(|s| settings::expand_path(s).canonicalize().ok())
                            .is_some_and(|p| {
                                settings::path_match_key(&p.to_string_lossy()) == target
                            })
                        {
                            names.push(k.clone());
                        }
                    }
                    names.sort();
                }
                Some((
                    item.port,
                    LocalFacts {
                        secrets: Arc::new(Secrets::new(secrets).with_headers(headers)),
                        aliases: names,
                        restore: desire
                            .as_ref()
                            .filter(|_| item.restore_pending)
                            .map(|e| self.restore_token(e)),
                    },
                ))
            })
            .collect()
    }
}

/// 原始层也要收集：继承被拒或被覆盖的凭据仍可能被复制进另一个普通字段。
fn collect_settings_secrets(raw: &Value, out: &mut Vec<String>) {
    collect_config_secrets(&raw["defaults"], out);
    if let Some(url) = raw["update"]["download_proxy"].as_str() {
        collect_url(url, out);
    }
    if let Some(chain) = raw["update"]["download_chain"].as_array() {
        for step in chain {
            if let Some(url) = step["url"].as_str() {
                collect_url(url, out);
            }
        }
    }
}
fn configured_headers(doc: &Value) -> Vec<String> {
    ["extra_headers", "override_headers"]
        .into_iter()
        .filter_map(|k| doc[k].as_object())
        .flat_map(|m| m.keys().cloned())
        .collect()
}
fn collect_config_secrets(doc: &Value, out: &mut Vec<String>) {
    for key in ["api_key", "proxy_username", "proxy_password"] {
        if let Some(s) = doc[key].as_str() {
            out.push(s.to_owned());
        }
    }
    for key in ["base_url", "proxy"] {
        if let Some(s) = doc[key].as_str() {
            collect_url(s, out);
        }
    }
    for key in ["extra_headers", "override_headers"] {
        if let Some(map) = doc[key].as_object() {
            for (k, v) in map {
                if let Some(s) = v.as_str() {
                    out.push(s.to_owned());
                    if aproxy_ipc::redact::is_secret_header(k)
                        && let Some((_, credential)) = s.trim().rsplit_once(' ')
                    {
                        out.push(credential.to_owned());
                    }
                }
            }
        }
    }
    if let Some(programs) = doc["format"].as_array() {
        for f in programs {
            if let Some(env) = f["env"].as_object() {
                out.extend(env.values().filter_map(Value::as_str).map(str::to_owned));
            }
            if let Some(args) = f["args"].as_array() {
                let mut operand = false;
                for arg in args.iter().filter_map(Value::as_str) {
                    if std::mem::take(&mut operand) {
                        out.push(arg.to_owned());
                        continue;
                    }
                    if url::Url::parse(arg).is_ok() {
                        collect_url(arg, out);
                    }
                    let (name, value) = arg
                        .split_once('=')
                        .map_or((arg, None), |(k, v)| (k, Some(v)));
                    if config::SECRET_ARG_FLAGS
                        .iter()
                        .any(|flag| flag.eq_ignore_ascii_case(name))
                    {
                        if let Some(v) = value {
                            out.push(v.to_owned());
                        } else {
                            operand = true;
                        }
                    }
                }
            }
        }
    }
}
fn collect_url(s: &str, out: &mut Vec<String>) {
    if let Ok(url) = url::Url::parse(s) {
        for value in std::iter::once(url.username()).chain(url.password()) {
            out.push(value.to_owned());
            out.push(
                percent_encoding::percent_decode_str(value)
                    .decode_utf8_lossy()
                    .into_owned(),
            );
        }
        out.extend(url.query_pairs().map(|(_, v)| v.into_owned()));
        if let Some(q) = url.query() {
            out.extend(
                q.split('&')
                    .filter_map(|p| p.split_once('=').map(|(_, v)| v.to_owned())),
            );
        }
    } else if !s.is_empty() {
        out.push(s.to_owned());
    }
}
