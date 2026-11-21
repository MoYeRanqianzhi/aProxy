//! 文件配置与全局设置的窗口投影。读写都走应用自身的分层、验证与持锁写回，不信任显示路径。
use super::{DesktopHome, collect_config_secrets, collect_settings_secrets, instances::issues};
use crate::{
    config::{
        self,
        edit::{self, Change, EditError},
    },
    settings,
};
use aproxy_ipc::{
    desktop::*,
    enumerate::{Enumerated, Liveness},
    redact::{self, Secrets},
};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

const CONFIG_KEYS: &[&str] = &[
    "base_url",
    "listen_addr",
    "api_key",
    "proxy",
    "proxy_username",
    "proxy_password",
    "extra_headers",
    "override_headers",
    "timeouts.connect_secs",
    "timeouts.first_byte_secs",
    "timeouts.read_idle_secs",
    "keepalive.interval_secs",
    "keepalive.trigger",
    "keepalive.heartbeat",
    "max_request_mb",
    "max_response_mb",
    "disk_cache",
    "delivery",
    "allowed_hosts",
    "allowed_origins",
    "log_file",
    "retry.max_backoff_secs",
    "retry.rules",
    "history.body_budget_mb",
    "format",
];
const SETTINGS_KEYS: &[&str] = &[
    "schema",
    "language",
    "aliases",
    "default_config",
    "config_dirs",
    "defaults",
    "watchdog.heartbeat_secs",
    "watchdog.stale_after_cycles",
    "watchdog.max_restarts",
    "watchdog.idle_exit_secs",
    "logs.rotate_mb",
    "instances.idle_threshold_secs",
    "update.download_chain",
    "update.download_proxy",
    "update.skill_auto_update",
    "ui",
];
const FORMAT_KEYS: &[&str] = &[
    "name",
    "command",
    "args",
    "env",
    "process",
    "stages",
    "required",
    "pool_max",
    "worker_idle_secs",
    "timeout_secs",
    "decide_timeout_secs",
    "breaker_failures",
    "breaker_cooldown_secs",
];
fn fixed<T: serde::Serialize>(key: &T) -> String {
    serde_json::to_value(key)
        .expect("键可序列化")
        .as_str()
        .expect("键名是字符串")
        .to_owned()
}
fn at<'a>(v: &'a Value, k: &str) -> &'a Value {
    k.split('.').fold(v, |node, part| &node[part])
}
fn marked(s: &str) -> bool {
    s.contains("***") || s.contains("<secret>") || s.contains("<secret ")
}
fn present(v: &Value) -> Cell {
    Cell::Secret {
        set: v.as_str().is_some_and(|s| !s.is_empty()),
    }
}
fn cell(k: &str, v: &Value, s: &Secrets) -> Cell {
    if matches!(k, "api_key" | "proxy_username" | "proxy_password") {
        return present(v);
    }
    if k == "ui" {
        return Cell::Map { entries: vec![] };
    }
    match v {
        Value::Null => Cell::None,
        Value::Bool(value) => Cell::Bool { value: *value },
        Value::Number(n) if n.is_i64() || n.is_u64() => Cell::Integer {
            decimal: n.to_string(),
        },
        Value::Number(_) => Cell::None,
        Value::String(t) => Cell::Text {
            text: if matches!(
                k,
                "base_url" | "proxy" | "url" | "update.download_proxy" | "args"
            ) {
                s.replace(&redact::url(t))
            } else {
                s.replace(t)
            },
        },
        Value::Array(a) => {
            let mut operand = false;
            let items = a
                .iter()
                .map(|v| {
                    if k == "args"
                        && let Some(t) = v.as_str()
                    {
                        if std::mem::take(&mut operand) {
                            return present(v);
                        }
                        let (key, val) = t
                            .split_once('=')
                            .map_or((t, None), |(key, val)| (key, Some(val)));
                        if config::SECRET_ARG_FLAGS
                            .iter()
                            .any(|f| f.eq_ignore_ascii_case(key))
                        {
                            if val.is_some() {
                                return present(v);
                            }
                            operand = true;
                        }
                    }
                    cell(k, v, s)
                })
                .collect();
            Cell::List { items }
        }
        Value::Object(map) => {
            let mut sorted = map.iter().collect::<Vec<_>>();
            sorted.sort_unstable_by_key(|(name, _)| *name);
            let entries = sorted
                .into_iter()
                .filter_map(|(name, v)| {
                    if (k == "format" && !FORMAT_KEYS.contains(&name.as_str()))
                        || (k == "retry.rules"
                            && !matches!(name.as_str(), "path" | "status" | "max_attempts"))
                        || (k == "update.download_chain" && name != "url")
                    {
                        return None;
                    }
                    let sub = if matches!(k, "timeouts" | "keepalive" | "retry" | "history") {
                        format!("{k}.{name}")
                    } else {
                        name.to_owned()
                    };
                    if matches!(
                        k,
                        "defaults" | "timeouts" | "keepalive" | "retry" | "history"
                    ) && !CONFIG_KEYS
                        .iter()
                        .any(|key| *key == sub || key.starts_with(&format!("{sub}.")))
                    {
                        return None;
                    }
                    let value = if matches!(k, "env" | "extra_headers" | "override_headers") {
                        present(v)
                    } else if k == "aliases" {
                        cell("alias-path", v, s)
                    } else {
                        cell(&sub, v, s)
                    };
                    Some(MapCell {
                        name: if matches!(k, "extra_headers" | "override_headers") {
                            Projection::new(s).name(name)
                        } else {
                            s.replace(name)
                        },
                        value,
                    })
                })
                .collect();
            Cell::Map { entries }
        }
    }
}
fn read_optional(p: &Path) -> R<Option<String>> {
    match std::fs::read_to_string(p) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(Fault::DesktopUnavailable {}),
    }
}
struct Layers {
    settings: settings::Settings,
    raw: Value,
    diagnostics: Vec<aproxy_ipc::diag::Diagnostic>,
    defaults: config::resolve::Defaults,
}
impl Layers {
    fn load(home: &Path) -> R<Self> {
        let path = settings::settings_path_in(home);
        let text = read_optional(&path)?;
        let (settings, diagnostics) = settings::file::parse(text.as_deref().unwrap_or("{}"), &path);
        if diagnostics.iter().any(|d| d.is_error()) {
            return Err(Fault::DesktopConfig {
                diagnostics: issues(&diagnostics, None),
            });
        }
        let raw =
            settings::edit::effective_value(text.as_deref()).ok_or(Fault::DesktopUnavailable {})?;
        let defaults = config::resolve::Defaults::from_settings(&settings, &path);
        Ok(Self {
            settings,
            raw,
            defaults,
            diagnostics,
        })
    }
    fn secrets(&self, file: Option<&Value>) -> Secrets {
        let mut values = Vec::new();
        collect_settings_secrets(&self.raw, &mut values);
        if let Some(v) = file {
            collect_config_secrets(v, &mut values)
        }
        Secrets::new(values)
    }
    fn resolve(&self, p: &Path, text: Option<&str>) -> config::Resolved {
        config::resolve::resolve_text(
            text,
            p,
            &Default::default(),
            &self.defaults,
            config::resolve::env_proxy_is_set(),
        )
    }
}
fn external(home: &Path, p: &Path) -> R<bool> {
    if !cfg!(windows) {
        return Ok(false);
    }
    let root = home
        .canonicalize()
        .map_err(|_| Fault::DesktopUnavailable {})?;
    Ok(!p.starts_with(&root))
}
fn source(s: &config::resolve::Source) -> Source {
    use config::resolve::Source as S;
    match s {
        S::Override => Source::Override,
        S::Toml => Source::Toml,
        S::Default => Source::Default,
        S::Env => Source::Env,
        S::Settings => Source::Settings,
    }
}
fn fields(doc: &Value, res: &config::Resolved, s: &Secrets) -> Vec<ConfigField> {
    let mut result = Vec::new();
    for &k in CONFIG_KEYS {
        let v = at(doc, k);
        result.push(ConfigField {
            key: k.into(),
            value: cell(k, v, s),
            source: res.sources.get(k).map(source),
            header: None,
            editable: matches!(
                k,
                "base_url"
                    | "listen_addr"
                    | "api_key"
                    | "proxy"
                    | "proxy_username"
                    | "proxy_password"
            ) || k.starts_with("timeouts.")
                || k.starts_with("keepalive."),
        });
        if let Some(table) = match k {
            "extra_headers" => Some(HeaderTable::ExtraHeaders),
            "override_headers" => Some(HeaderTable::OverrideHeaders),
            _ => None,
        } && let Some(map) = v.as_object()
        {
            let mut sorted = map.iter().collect::<Vec<_>>();
            sorted.sort_unstable_by_key(|(n, _)| *n);
            let projection = Projection::new(s);
            for (n, value) in sorted {
                let key = format!("{k}.{}", config::quote_key(n));
                let safe = projection.name(n);
                let ok = safe == *n;
                result.push(ConfigField {
                    key: format!("{k}.{}", config::quote_key(&safe)),
                    value: present(value),
                    source: res.sources.get(&key).map(source),
                    header: ok.then(|| HeaderKey {
                        table,
                        name: n.clone(),
                    }),
                    editable: ok,
                });
            }
        }
    }
    result
}
fn changes(edits: &[Edit]) -> R<Vec<Change>> {
    if edits.len() > 128 || serde_json::to_vec(edits).map_or(true, |v| v.len() > 64 * 1024) {
        return Err(Fault::DesktopBadInput {});
    }
    let mut seen = BTreeSet::new();
    let mut result = Vec::new();
    for e in edits {
        let (key, val) = match e {
            Edit::Set(Assignment::Text { key, value }) => {
                if marked(value) {
                    return Err(Fault::DesktopBadInput {});
                }
                (fixed(key), Some(toml_edit::Value::from(value.as_str())))
            }
            Edit::Set(Assignment::Seconds { key, value }) => {
                (fixed(key), Some(toml_edit::Value::from(value.get() as i64)))
            }
            Edit::Set(Assignment::Trigger { key, value }) => (
                fixed(key),
                Some(toml_edit::Value::from(fixed(value).replace('-', "_"))),
            ),
            Edit::Unset { key } => (fixed(key), None),
            Edit::SetHeader { key, name, .. } | Edit::UnsetHeader { key, name } => {
                if http::HeaderName::from_bytes(name.as_bytes()).is_err() || marked(name) {
                    return Err(Fault::DesktopBadInput {});
                }
                let val = if let Edit::SetHeader { value, .. } = e {
                    if marked(value) || http::HeaderValue::from_str(value).is_err() {
                        return Err(Fault::DesktopBadInput {});
                    }
                    Some(toml_edit::Value::from(value.as_str()))
                } else {
                    None
                };
                (format!("{}.{}", fixed(key), config::quote_key(name)), val)
            }
        };
        if !seen.insert(key.to_ascii_lowercase()) {
            return Err(Fault::DesktopBadInput {});
        }
        result.push(match val {
            Some(value) => Change::Set { key, value },
            None => Change::Unset { key },
        });
    }
    Ok(result)
}
impl From<EditError> for FormError {
    fn from(e: EditError) -> Self {
        Self(match e {
            EditError::Invalid(ds) | EditError::Syntax(ds) => Fault::DesktopConfig {
                diagnostics: issues(&ds, None),
            },
            EditError::Conflict | EditError::LockBusy => Fault::DesktopConflict {},
            EditError::BadKey(_) => Fault::DesktopBadInput {},
            EditError::Io(_) => Fault::DesktopUnavailable {},
        })
    }
}
struct FormError(Fault);
impl From<Fault> for FormError {
    fn from(e: Fault) -> Self {
        Self(e)
    }
}
fn users(p: &Path, members: &[Enumerated]) -> Vec<u16> {
    members
        .iter()
        .filter(|m| {
            matches!(m.liveness, Liveness::Live | Liveness::Unresponsive)
                && m.instance
                    .as_ref()
                    .and_then(|r| r.config_path.as_ref())
                    .and_then(|p| Path::new(p).canonicalize().ok())
                    .as_deref()
                    == Some(p)
        })
        .map(|m| m.port)
        .collect()
}
fn inspect_patch(
    path: &Path,
    old: Option<&str>,
    layers: &Layers,
    edits: &[Change],
    members: &[Enumerated],
    outside: bool,
    context: &Secrets,
) -> R<(Validation, Option<String>)> {
    match edit::apply_to_text(old.unwrap_or(""), path, &layers.defaults, edits) {
        Ok(new) => {
            let next = layers.resolve(path, Some(&new));
            let previous = layers.resolve(path, old);
            let port = crate::daemon::port_of(&next.config.listen_addr)
                .parse::<u16>()
                .ok();
            let occupied = port.filter(|p| {
                *p != 0
                    && next.config.listen_addr != previous.config.listen_addr
                    && members
                        .iter()
                        .any(|m| m.port == *p && m.liveness != Liveness::Down)
                    && !users(path, members).contains(p)
            });
            let raw = config::view::file_value(&new);
            let s = layers
                .secrets(raw.as_ref())
                .union(&layers.secrets(old.and_then(config::view::file_value).as_ref()))
                .union(context);
            let diagnostics = issues(&next.diagnostics, next.is_valid().then_some(&s));
            Ok((
                Validation {
                    can_save: occupied.is_none(),
                    occupied_port: occupied,
                    external_acl: outside,
                    diagnostics,
                },
                Some(new),
            ))
        }
        Err(EditError::Syntax(ds) | EditError::Invalid(ds)) => Ok((
            Validation {
                can_save: false,
                occupied_port: None,
                external_acl: outside,
                diagnostics: issues(&ds, None),
            },
            None,
        )),
        Err(e) => Err(FormError::from(e).0),
    }
}
impl DesktopHome {
    async fn form_path(
        &self,
        t: &ConfigTarget,
        editing: bool,
    ) -> R<(PathBuf, Secrets, Option<Token>)> {
        let choice = match t {
            ConfigTarget::Default { id } => StartTarget {
                kind: ChoiceKind::Default,
                id: id.clone(),
            },
            ConfigTarget::Alias { id } => StartTarget {
                kind: ChoiceKind::Alias,
                id: id.clone(),
            },
            ConfigTarget::Config { id } => StartTarget {
                kind: ChoiceKind::Config,
                id: id.clone(),
            },
            ConfigTarget::Run { run, expected_file } => {
                let rec = self.selected_run(run, false).await?;
                let path = Path::new(rec.config_path.as_deref().ok_or(Fault::DesktopTarget {})?)
                    .canonicalize()
                    .map_err(|_| Fault::DesktopTarget {})?;
                let token = self.token(
                    &serde_json::to_vec(&(run, &path)).map_err(|_| Fault::DesktopTarget {})?,
                );
                if editing && expected_file.as_ref() != Some(&token) {
                    return Err(Fault::DesktopChanged {});
                }
                let sample = Enumerated {
                    port: run.port,
                    instance: Some(rec),
                    liveness: Liveness::Live,
                    reason: None,
                    status: None,
                    restore_pending: false,
                    restore_reason: None,
                };
                let facts = self
                    .inspect(&[sample])
                    .remove(&run.port)
                    .ok_or(Fault::DesktopUnavailable {})?;
                return Ok((path, (*facts.secrets).clone(), Some(token)));
            }
        };
        Ok((
            self.resolve_choice(&choice).await?.path,
            Secrets::default(),
            None,
        ))
    }
}
impl ConfigOps for DesktopHome {
    fn read_config(&self, target: ConfigTarget) -> Task<'_, ConfigView> {
        Box::pin(async move {
            let (path, context, file_token) = self.form_path(&target, false).await?;
            let layers = Layers::load(&self.home)?;
            let text = read_optional(&path)?;
            let resolved = layers.resolve(&path, text.as_deref());
            let raw = text.as_deref().and_then(config::view::file_value);
            let s = layers.secrets(raw.as_ref()).union(&context);
            let valid = text
                .as_deref()
                .is_none_or(|t| !config::file::parse(t, &path).1.iter().any(|d| d.is_error()));
            let doc = config::view::effective_value(&resolved, raw.as_ref());
            Ok(ConfigView {
                file_token,
                file: if valid {
                    s.replace(&path.to_string_lossy())
                } else {
                    "***".into()
                },
                fields: if valid {
                    fields(&doc, &resolved, &s)
                } else {
                    fields(&Value::Null, &resolved, &s)
                },
                withheld: if valid {
                    resolved
                        .withheld
                        .iter()
                        .map(|w| Withheld {
                            key: s.replace(&w.key),
                            from: SettingsSource::Settings,
                            reason: if w.reason == "proxy_differs" {
                                WithheldReason::ProxyDiffers
                            } else {
                                WithheldReason::BaseUrlDiffers
                            },
                            default_target: s.replace(&redact::url(&w.default_target)),
                            target: s.replace(&redact::url(&w.target)),
                        })
                        .collect()
                } else {
                    vec![]
                },
                diagnostics: issues(&resolved.diagnostics, valid.then_some(&s)),
                external_acl: external(&self.home, &path)?,
            })
        })
    }
    fn validate_config(&self, target: ConfigTarget, edits: Vec<Edit>) -> Task<'_, Validation> {
        Box::pin(async move {
            let (path, context, _) = self.form_path(&target, true).await?;
            let patch = changes(&edits)?;
            let members = aproxy_ipc::enumerate::enumerate_in(&self.home)
                .await
                .instances;
            let (v, _) = inspect_patch(
                &path,
                read_optional(&path)?.as_deref(),
                &Layers::load(&self.home)?,
                &patch,
                &members,
                external(&self.home, &path)?,
                &context,
            )?;
            Ok(v)
        })
    }
    fn write_config(&self, target: ConfigTarget, edits: Vec<Edit>, ack: bool) -> Task<'_, Saved> {
        Box::pin(async move {
            let (observed, _, _) = self.form_path(&target, true).await?;
            let patch = changes(&edits)?;
            let host = self.clone();
            let runtime = tokio::runtime::Handle::current();
            tokio::task::spawn_blocking(move || {
                let mut ports = Vec::new();
                let applied = edit::rewrite_locked(
                    &observed,
                    &host.home,
                    false,
                    || {},
                    |old, file| -> Result<String, FormError> {
                        let (actual, context, _) =
                            runtime.block_on(host.form_path(&target, true))?;
                        if actual != observed || actual != file {
                            return Err(Fault::DesktopChanged {}.into());
                        }
                        let outside = external(&host.home, file)?;
                        if outside && !ack {
                            return Err(Fault::DesktopExternalAcl {}.into());
                        }
                        let members = runtime
                            .block_on(aproxy_ipc::enumerate::enumerate_in(&host.home))
                            .instances;
                        let (v, new) = inspect_patch(
                            file,
                            old,
                            &Layers::load(&host.home)?,
                            &patch,
                            &members,
                            outside,
                            &context,
                        )?;
                        if let Some(port) = v.occupied_port {
                            return Err(Fault::DesktopPort { port }.into());
                        }
                        ports = users(file, &members);
                        new.ok_or_else(|| {
                            Fault::DesktopConfig {
                                diagnostics: v.diagnostics,
                            }
                            .into()
                        })
                    },
                )
                .map_err(|e| e.0)?;
                Ok(Saved {
                    changed: applied.changed,
                    instances_to_restart: if applied.changed { ports } else { vec![] },
                })
            })
            .await
            .map_err(|_| Fault::DesktopUnavailable {})?
        })
    }
    fn read_settings(&self) -> Task<'_, SettingsView> {
        Box::pin(async move {
            let l = Layers::load(&self.home)?;
            let s = l.secrets(None);
            Ok(SettingsView {
                watchdog_enabled: l.settings.watchdog,
                read_only: SETTINGS_KEYS
                    .iter()
                    .map(|k| Setting {
                        key: (*k).into(),
                        value: cell(k, at(&l.raw, k), &s),
                    })
                    .collect(),
                diagnostics: issues(&l.diagnostics, Some(&s)),
            })
        })
    }
    fn set_watchdog(&self, enabled: bool) -> Task<'_, bool> {
        Box::pin(async move {
            let home = self.home.clone();
            tokio::task::spawn_blocking(move || {
                Layers::load(&home)?;
                settings::edit::apply(
                    &settings::settings_path_in(&home),
                    &home,
                    &[settings::edit::Change::set("watchdog.enabled", enabled)],
                )
                .map_err(|e| FormError::from(e).0)?;
                Ok(Layers::load(&home)?.settings.watchdog)
            })
            .await
            .map_err(|_| Fault::DesktopUnavailable {})?
        })
    }
}
