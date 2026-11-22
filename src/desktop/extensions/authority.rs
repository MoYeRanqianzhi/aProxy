//! 持久授权与易失租约分开：磁盘只能证明过去的决定，不能用来兑换某个窗口里尚未回答的请求。
use super::{DesktopHome, Payload, Snapshot, format, tree::Dir};
use aproxy_ipc::desktop::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::oneshot;

const FILE: &str = "extension-state.json";
const BYTES: usize = 256 * 1024;
const ROW_BYTES: usize = 160 * 1024;
const ROWS: usize = 128;
const TTL: Duration = Duration::from_secs(60);
fn state(reason: ExtStateReason) -> Fault {
    Fault::DesktopExtensionState { reason }
}
fn io() -> Fault {
    state(ExtStateReason::Io)
}
fn corrupt() -> Fault {
    state(ExtStateReason::Corrupt)
}
fn denied() -> Fault {
    Fault::DesktopExtensionDenied {}
}
fn changed() -> Fault {
    Fault::DesktopChanged {}
}
fn now() -> SafeUint {
    SafeUint::saturating(aproxy_ipc::registry::now_millis())
}
fn token() -> R<String> {
    use std::fmt::Write;
    let mut raw = [0u8; 32];
    getrandom::fill(&mut raw).map_err(|_| Fault::DesktopUnavailable {})?;
    let mut out = String::with_capacity(64);
    for b in raw {
        write!(out, "{b:02x}").expect("String");
    }
    Ok(out)
}
fn hex(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
}
fn key(k: &ExtKey) -> bool {
    format::key(&k.id, 48) && !k.id.ends_with('-') && hex(&k.digest)
}
fn public_text(s: &str) -> bool {
    !s.is_empty() && s.len()<=512 && !s.chars().any(|c|c.is_control()||matches!(c,'\u{061c}'|'\u{200e}'|'\u{200f}'|'\u{202a}'..='\u{202e}'|'\u{2066}'..='\u{2069}'))
}
fn target_ok(t: &TargetView) -> bool {
    match t {
        TargetView::InstanceStart {
            path, label, port, ..
        } => *port != 0 && public_text(path) && public_text(label),
        TargetView::InstanceStop { run } | TargetView::InstanceRestart { run } => {
            run.port != 0 && public_text(&run.instance_id)
        }
        TargetView::InstallUpdate { version, .. } => {
            version.len() <= 64 && semver::Version::parse(version).is_ok()
        }
    }
}
fn unfinished(outcome: ExtOutcome) -> bool {
    matches!(outcome, ExtOutcome::Pending | ExtOutcome::Executing)
}
#[derive(Clone, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Disk {
    schema_version: u32,
    enabled: BTreeMap<String, String>,
    grants: Vec<Grant>,
    audit: Vec<Audit>,
}
impl Default for Disk {
    fn default() -> Self {
        Self {
            schema_version: 1,
            enabled: Default::default(),
            grants: vec![],
            audit: vec![],
        }
    }
}
impl Disk {
    fn enabled(&self, k: &ExtKey) -> bool {
        self.enabled.get(&k.id) == Some(&k.digest)
    }
    fn validate(&self) -> bool {
        let mut verbs = BTreeSet::new();
        let mut ids = BTreeSet::new();
        self.schema_version == 1
            && self.enabled.len() <= 24
            && self
                .enabled
                .iter()
                .all(|(id, d)| format::key(id, 48) && !id.ends_with('-') && hex(d))
            && self.grants.len() <= 72
            && self.grants.iter().all(|g| {
                self.enabled(&g.key)
                    && g.permission != Permission::InstallUpdate
                    && hex(&g.grant_id)
                    && ids.insert(&g.grant_id)
                    && verbs.insert((&g.key, g.permission))
            })
            && self.audit.len() <= ROWS
            && serde_json::to_vec(&self.audit).is_ok_and(|v| v.len() <= ROW_BYTES)
            && self
                .audit
                .iter()
                .filter(|a| a.outcome == ExtOutcome::Pending)
                .count()
                <= 2
            && self
                .audit
                .iter()
                .filter(|a| a.outcome == ExtOutcome::Executing)
                .count()
                <= 2
            && self.audit.iter().all(|a| {
                key(&a.key)
                    && a.version.len() <= 64
                    && semver::Version::parse(&a.version).is_ok()
                    && hex(&a.operation_id)
                    && ids.insert(&a.operation_id)
                    && a.target
                        .as_ref()
                        .is_none_or(|t| t.permission() == a.op && target_ok(t))
                    && a.finished_at.is_none() == unfinished(a.outcome)
                    && match &a.authority {
                        Authority::None => {
                            matches!(a.outcome, ExtOutcome::Pending | ExtOutcome::Denied)
                        }
                        Authority::Grant { grant_id } => {
                            hex(grant_id)
                                && a.op != Permission::InstallUpdate
                                && a.target.is_some()
                                && !matches!(a.outcome, ExtOutcome::Pending | ExtOutcome::Denied)
                        }
                        Authority::Confirmation {
                            decision_id,
                            created_grant_id,
                        } => {
                            hex(decision_id)
                                && created_grant_id
                                    .as_ref()
                                    .is_none_or(|g| hex(g) && a.op != Permission::InstallUpdate)
                                && a.target.is_some()
                                && !matches!(a.outcome, ExtOutcome::Pending | ExtOutcome::Denied)
                        }
                    }
                    && a.code.as_ref().is_none_or(|c| known_fault(c))
            })
    }
    // 内存中的 token 从未写盘；pending 行本身就是不可兑换的描述符。只淘汰已结束的旧行。
    fn trim(&mut self) -> R<()> {
        while self.audit.len() > ROWS
            || serde_json::to_vec(&self.audit).map_err(|_| io())?.len() > ROW_BYTES
        {
            let i = self
                .audit
                .iter()
                .position(|r| !unfinished(r.outcome))
                .ok_or(Fault::DesktopExtensionLimit {})?;
            self.audit.remove(i);
        }
        Ok(())
    }
    fn finish(&mut self, id: &str, outcome: ExtOutcome, code: Option<String>) {
        if let Some(a) = self.audit.iter_mut().find(|a| a.operation_id == id) {
            a.outcome = outcome;
            a.code = code;
            a.finished_at = Some(now());
        }
    }
}
fn known_fault(s: &str) -> bool {
    matches!(
        s,
        "desktop_extension_package"
            | "desktop_extension_state"
            | "desktop_extension_denied"
            | "desktop_extension_runtime"
            | "desktop_extension_limit"
            | "desktop_bad_input"
            | "desktop_unavailable"
            | "desktop_window"
            | "desktop_cursor"
            | "desktop_changed"
            | "desktop_history"
            | "desktop_gone"
            | "desktop_cancelled"
            | "desktop_busy"
            | "desktop_target"
            | "desktop_external_acl"
            | "desktop_conflict"
            | "desktop_binary"
            | "desktop_install_protocol"
            | "desktop_unresponsive"
            | "desktop_force_refused"
            | "desktop_no_spec"
            | "desktop_port"
            | "desktop_config"
            | "desktop_start"
            | "desktop_stop"
            | "desktop_state_io"
            | "desktop_state_reset"
            | "desktop_limit"
            | "desktop_registration"
            | "install_in_progress"
            | "install_version_not_found"
            | "install_below_floor"
            | "install_download_failed"
            | "install_checksum_mismatch"
            | "install_probe_failed"
            | "install_swap_failed"
            | "install_rolled_back"
            | "install_failed"
            | "install_unmanaged_instances"
            | "install_restart_failed"
            | "install_foreground_instances"
            | "install_precheck_failed"
    )
}
struct Store {
    _guard: crate::desktop::state::Store,
    path: PathBuf,
    _parent: Dir,
}
impl Store {
    fn open(home: &Path) -> R<(Self, Disk)> {
        let guard =
            crate::desktop::state::Store::open(home, StateFile::Preferences).map_err(|_| io())?;
        let dir = Dir::state_dir(&home.join("desktop")).map_err(|_| io())?;
        let path = home.join("desktop").join(FILE);
        let disk = match dir.node(FILE).map_err(|_| io())? {
            None => Disk::default(),
            Some((file, stamp, false)) if stamp.bytes <= BYTES as u64 => {
                let mut b = Vec::new();
                file.take((BYTES + 1) as u64)
                    .read_to_end(&mut b)
                    .map_err(|_| io())?;
                if b.len() > BYTES {
                    return Err(corrupt());
                }
                #[derive(Deserialize)]
                struct Version {
                    #[serde(rename = "schemaVersion")]
                    v: u64,
                }
                if serde_json::from_slice::<Version>(&b)
                    .map_err(|_| corrupt())?
                    .v
                    > 1
                {
                    return Err(state(ExtStateReason::TooNew));
                }
                let d: Disk = serde_json::from_slice(&b).map_err(|_| corrupt())?;
                if !d.validate() {
                    return Err(corrupt());
                }
                d
            }
            Some(_) => return Err(corrupt()),
        };
        Ok((
            Self {
                _guard: guard,
                path,
                _parent: dir,
            },
            disk,
        ))
    }
    fn save(&self, d: &mut Disk) -> R<()> {
        d.trim()?;
        if !d.validate() {
            return Err(corrupt());
        }
        let b = serde_json::to_vec(d).map_err(|_| io())?;
        if b.len() > BYTES {
            return Err(Fault::DesktopExtensionLimit {});
        }
        crate::fsx::write_atomic_private(&self.path, &b).map_err(|_| io())
    }
}
struct Lease {
    id: String,
    principal: Principal,
    listing: Arc<Mutex<Option<[u8; 16]>>>,
}
struct Waiting {
    consent: Consent,
    id: String,
    until: Instant,
    send: Option<oneshot::Sender<ExtDecision>>,
}
#[derive(Default)]
pub(in crate::desktop) struct Runtime {
    recovered: bool,
    notify: Option<ExtensionNotify>,
    inventory: Option<Inventory>,
    session: Option<String>,
    qualified: bool,
    lease: Option<Lease>,
    pending: BTreeMap<String, Waiting>,
    executing: BTreeSet<String>,
}
impl Runtime {
    fn notify(&self, event: DesktopEvent) {
        if let Some(n) = &self.notify {
            n(event)
        }
    }
    fn retire(&mut self) {
        if self.lease.take().is_some() {
            self.notify(DesktopEvent::ExtensionsChanged);
        }
        self.pending.clear();
        self.notify(DesktopEvent::ExtensionConsentsChanged);
    }
    fn support(&self) -> ScriptSupport {
        if self.qualified {
            ScriptSupport::Available
        } else {
            ScriptSupport::Unverified
        }
    }
    fn reconcile(&mut self, d: &mut Disk, s: &Snapshot) {
        d.enabled.retain(|id, digest| {
            s.packages
                .iter()
                .any(|p| &p.principal.key.id == id && &p.principal.key.digest == digest)
        });
        d.grants.retain(|g| {
            d.enabled.get(&g.key.id) == Some(&g.key.digest)
                && s.packages
                    .iter()
                    .any(|p| p.principal.key == g.key && p.permissions.contains(&g.permission))
        });
        if self
            .lease
            .as_ref()
            .is_some_and(|l| !d.enabled(&l.principal.key))
        {
            self.retire()
        }
    }
    fn cleanup(&mut self, d: &mut Disk) {
        self.pending.retain(|_, w| w.until > Instant::now());
        for a in &mut d.audit {
            let lost = match a.outcome {
                ExtOutcome::Pending => !self.pending.values().any(|w| w.id == a.operation_id),
                ExtOutcome::Executing => {
                    !self.recovered || !self.executing.contains(&a.operation_id)
                }
                _ => false,
            };
            if lost {
                if a.outcome == ExtOutcome::Executing {
                    a.outcome = ExtOutcome::Unknown;
                } else {
                    a.outcome = ExtOutcome::Denied;
                    a.code = Some("desktop_extension_denied".into())
                }
                a.finished_at = Some(now());
            }
        }
    }
}
impl DesktopHome {
    fn transaction<T>(
        &self,
        r: &mut Runtime,
        f: impl FnOnce(&mut Runtime, &mut Disk) -> R<T>,
    ) -> R<T> {
        let active_before = r.executing.clone();
        let work = || {
            let (store, mut disk) = Store::open(&self.home)?;
            let before = disk.clone();
            r.cleanup(&mut disk);
            let result = f(r, &mut disk);
            r.cleanup(&mut disk);
            if before != disk {
                store.save(&mut disk)?;
                if before.audit != disk.audit {
                    r.notify(DesktopEvent::ExtensionAuditChanged);
                    r.notify(DesktopEvent::ExtensionConsentsChanged)
                }
                if before.enabled != disk.enabled || before.grants != disk.grants {
                    r.notify(DesktopEvent::ExtensionsChanged)
                }
            }
            r.recovered = true;
            Ok(result)
        };
        match work() {
            Ok(result) => result,
            Err(e) => {
                r.executing = active_before;
                r.retire();
                Err(e)
            }
        }
    }
    /// 绑定真实窗口的全新连接；是否支持脚本来自本机安装好的边界，而不取自页面参数。
    pub fn extension_attach(
        &self,
        session: String,
        support: ScriptSupport,
        notify: ExtensionNotify,
    ) {
        let mut r = self.extensions.lock().unwrap();
        r.retire();
        r.session = Some(session);
        r.notify = Some(notify);
        r.qualified = support == ScriptSupport::Available;
    }
    /// 关闭或换页立即取消未决回答，即使窗口因安装任务暂时只隐藏。
    pub fn extension_detach(&self, session: Option<&str>) {
        let mut r = self.extensions.lock().unwrap();
        if session.is_none() || session == r.session.as_deref() {
            r.retire();
            r.session = None;
            r.qualified = false;
        }
    }
    /// 私有状态损坏时仍列出包；错误同行返回，不重置文件也不保留旧授权。
    pub fn extension_inventory(&self) -> Inventory {
        let mut r = self.extensions.lock().unwrap();
        let mut s = self.extension_snapshot(None);
        let support = r.support();
        let res = self.transaction(&mut r, |r, d| {
            r.reconcile(d, &s);
            for p in &mut s.packages {
                p.enabled = d.enabled(&p.principal.key)
                    && (r.qualified || p.principal.kind == ExtKind::Theme);
                if p.enabled {
                    p.grants = d
                        .grants
                        .iter()
                        .filter(|g| g.key == p.principal.key)
                        .cloned()
                        .collect();
                    p.grants.sort_by_key(|g| (g.created_at, g.grant_id.clone()));
                }
            }
            Ok(())
        });
        if res.is_err() {
            for p in &mut s.packages {
                p.enabled = false;
                p.grants.clear()
            }
        }
        let inv = Inventory {
            packages: s.packages,
            invalid: s.invalid,
            script_support: support,
            state_fault: res.err(),
        };
        if r.inventory.as_ref() != Some(&inv) {
            r.inventory = Some(inv.clone());
            r.notify(DesktopEvent::ExtensionsChanged)
        }
        inv
    }
    /// 只启用当前摘要；切换主题或吉祥物与撤销旧授权是同一次私有文件提交。
    pub fn extension_enable(&self, k: &ExtKey, enabled: bool) -> R<()> {
        let mut r = self.extensions.lock().unwrap();
        let s = self.extension_snapshot(None);
        self.transaction(&mut r, |r, d| {
            r.reconcile(d, &s);
            let p = s
                .packages
                .iter()
                .find(|p| &p.principal.key == k)
                .ok_or_else(changed)?;
            if enabled {
                if p.principal.kind != ExtKind::Theme && !r.qualified {
                    return Err(Fault::DesktopExtensionRuntime {});
                }
                if p.principal.kind != ExtKind::Page {
                    for other in &s.packages {
                        if other.principal.kind == p.principal.kind {
                            d.enabled.remove(&other.principal.key.id);
                        }
                    }
                }
                d.enabled.insert(k.id.clone(), k.digest.clone());
            } else {
                d.enabled.remove(&k.id);
            }
            r.reconcile(d, &s);
            Ok(())
        })
    }
    /// 只交付同一次核验留下的字节；页面和吉祥物共用一个脚本槽，主题不取得租约。
    pub fn extension_open(&self, session: &str, k: &ExtKey) -> R<Bundle> {
        let mut r = self.extensions.lock().unwrap();
        let mut s = self.extension_snapshot(Some(k));
        self.transaction(&mut r, |r, d| {
            r.reconcile(d, &s);
            if r.session.as_deref() != Some(session) {
                return Err(changed());
            }
            let p = s
                .packages
                .iter()
                .find(|p| &p.principal.key == k)
                .ok_or_else(changed)?;
            if p.principal.kind != ExtKind::Theme && !r.qualified {
                return Err(Fault::DesktopExtensionRuntime {});
            }
            if !d.enabled(k) {
                return Err(denied());
            }
            match s.payload.take().ok_or_else(changed)? {
                Payload::Theme(theme) => Ok(Bundle::Theme { theme }),
                Payload::Script(module, assets) => {
                    if r.lease.is_some() {
                        return Err(Fault::DesktopBusy {});
                    }
                    let lease = token()?;
                    r.lease = Some(Lease {
                        id: lease.clone(),
                        principal: p.principal.clone(),
                        listing: Default::default(),
                    });
                    let b = ScriptBundle {
                        lease,
                        module_b64: STANDARD.encode(module),
                        assets: assets
                            .into_iter()
                            .map(|(key, data)| ExtensionAsset {
                                key,
                                data_b64: STANDARD.encode(data),
                            })
                            .collect(),
                    };
                    Ok(if p.principal.kind == ExtKind::Page {
                        Bundle::Page(b)
                    } else {
                        Bundle::Mascot(b)
                    })
                }
            }
        })
    }
    /// 幂等关闭匹配的租约；迟到的旧页面不能关闭后来挂载的包。
    pub fn extension_close(&self, id: &str) {
        let mut r = self.extensions.lock().unwrap();
        if r.lease.as_ref().is_some_and(|l| l.id == id) {
            r.retire()
        }
    }
    /// 单独的配置选择观察域，不使宿主页的配置列表失效。
    pub fn extension_reader(&self, id: &str) -> R<Self> {
        let r = self.extensions.lock().unwrap();
        let l = r.lease.as_ref().filter(|l| l.id == id).ok_or_else(denied)?;
        Ok(Self {
            listing: l.listing.clone(),
            ..self.clone()
        })
    }
    /// 审计按逆插入顺序读取，磁盘里的旧字段不能绕过当前凭据的脱敏边界。
    pub fn extension_audit(&self) -> R<Vec<Audit>> {
        let mut r = self.extensions.lock().unwrap();
        let secrets = self.display_secrets().ok_or_else(denied)?;
        self.transaction(&mut r, |_, d| {
            Ok(d.audit
                .iter()
                .rev()
                .filter(|a| safe_projection(*a, &secrets))
                .cloned()
                .collect())
        })
    }
    /// 只撤销当前包的指定记录；使所有尚未越过执行栅栏的同范围请求失效。
    pub fn extension_revoke(&self, k: &ExtKey, grant: &str) -> R<()> {
        let mut r = self.extensions.lock().unwrap();
        let s = self.extension_snapshot(None);
        self.transaction(&mut r, |r, d| {
            r.reconcile(d, &s);
            let i = d
                .grants
                .iter()
                .position(|g| &g.key == k && g.grant_id == grant)
                .ok_or_else(changed)?;
            let g = d.grants.remove(i);
            r.pending.retain(|_, w| {
                w.consent.principal.key != *k || w.consent.target.permission() != g.permission
            });
            Ok(())
        })
    }
}
fn safe_projection<T: Serialize>(v: &T, secrets: &aproxy_ipc::redact::Secrets) -> bool {
    fn check(v: &serde_json::Value, s: &aproxy_ipc::redact::Secrets) -> bool {
        match v {
            serde_json::Value::String(t) => s.find(t.as_bytes()).is_none(),
            serde_json::Value::Array(a) => a.iter().all(|v| check(v, s)),
            serde_json::Value::Object(m) => m.values().all(|v| check(v, s)),
            _ => true,
        }
    }
    serde_json::to_value(v).is_ok_and(|v| check(&v, secrets))
}

/// 即使原始 invoke 的 future 随窗口消失，已提交的执行也必须有终态记录；调用者只等后台事务。
impl DesktopHome {
    /// 返回同一窗口当前可回答的原生描述；凭据变化后不可显示的请求按拒绝处理。
    pub fn extension_pending(&self, session: &str) -> R<Vec<Consent>> {
        let mut r = self.extensions.lock().unwrap();
        let secrets = self.display_secrets();
        self.transaction(&mut r, |r, _| {
            if r.session.as_deref() != Some(session) {
                return Err(changed());
            }
            r.pending.retain(|_, w| {
                secrets
                    .as_ref()
                    .is_some_and(|s| safe_projection(&w.consent, s))
            });
            let mut v: Vec<_> = r
                .pending
                .values()
                .filter(|w| w.send.is_some())
                .map(|w| w.consent.clone())
                .collect();
            v.sort_by_key(|c| (c.requested_at, c.decision_id.clone()));
            Ok(v)
        })
    }
    /// 答复仅唤醒原始调用，不能改变目标、另行执行或消费两次。
    pub fn extension_answer(&self, session: &str, token: &str, decision: ExtDecision) -> R<()> {
        let mut r = self.extensions.lock().unwrap();
        self.transaction(&mut r, |r, _| {
            if r.session.as_deref() != Some(session) {
                return Err(changed());
            }
            let w = r.pending.get_mut(token).ok_or_else(changed)?;
            let send = w.send.take().ok_or_else(changed)?;
            if decision == ExtDecision::Remember && !w.consent.can_remember {
                let _ = send.send(ExtDecision::Deny);
                return Err(Fault::DesktopBadInput {});
            }
            send.send(decision).map_err(|_| changed())?;
            r.notify(DesktopEvent::ExtensionConsentsChanged);
            Ok(())
        })
    }
    /// 在 blocking 池完成目录读取和私有文件事务，不在运行时线程上等待文件锁。
    async fn authority_job<T: Send + 'static>(
        &self,
        f: impl FnOnce(Self) -> R<T> + Send + 'static,
    ) -> R<T> {
        let h = self.clone();
        tokio::task::spawn_blocking(move || f(h))
            .await
            .map_err(|_| Fault::DesktopUnavailable {})?
    }
    /// 启动单次事务。授权与被批准的操作不分成两个能从页面分别调用的步骤。
    pub async fn extension_write(
        &self,
        lease: String,
        op: ExtensionWrite,
        slot: InstallSlot,
        progress: Progress,
    ) -> R<ExtensionResult> {
        let h = self.clone();
        tokio::spawn(async move { h.run_write(lease, op, slot, progress).await })
            .await
            .map_err(|_| Fault::DesktopUnavailable {})?
    }
}
struct Admission {
    id: String,
    principal: Principal,
    requested: SafeUint,
    verb: Permission,
}
impl Admission {
    fn row(&self, target: Option<TargetView>) -> Audit {
        Audit {
            operation_id: self.id.clone(),
            key: self.principal.key.clone(),
            version: self.principal.version.clone(),
            op: self.verb,
            target,
            requested_at: self.requested,
            finished_at: None,
            authority: Authority::None,
            outcome: ExtOutcome::Pending,
            code: None,
        }
    }
}
fn fault_code(f: &Fault) -> String {
    serde_json::to_value(f).expect("typed fault")["code"]
        .as_str()
        .expect("fault tag")
        .to_owned()
}
fn refusal(d: &mut Disk, a: &Admission, target: Option<TargetView>, fault: Fault) -> Fault {
    if !d.audit.iter().any(|r| r.operation_id == a.id) {
        d.audit.push(a.row(target));
    }
    d.finish(&a.id, ExtOutcome::Denied, Some(fault_code(&fault)));
    fault
}
enum PermissionSource {
    Grant(String),
    Answer {
        token: String,
        decision: String,
        deadline: Instant,
        rx: oneshot::Receiver<ExtDecision>,
    },
}
impl DesktopHome {
    fn eligible(&self, r: &mut Runtime, d: &mut Disk, lease: &str, a: &Admission) -> R<()> {
        let s = self.extension_snapshot(None);
        r.reconcile(d, &s);
        if !r.qualified
            || !d.enabled(&a.principal.key)
            || !r
                .lease
                .as_ref()
                .is_some_and(|l| l.id == lease && l.principal == a.principal)
            || !s
                .packages
                .iter()
                .any(|p| p.principal == a.principal && p.permissions.contains(&a.verb))
        {
            return Err(denied());
        }
        Ok(())
    }
    async fn run_write(
        &self,
        lease: String,
        op: ExtensionWrite,
        slot: InstallSlot,
        progress: Progress,
    ) -> R<ExtensionResult> {
        let principal = self
            .extensions
            .lock()
            .unwrap()
            .lease
            .as_ref()
            .filter(|l| l.id == lease)
            .map(|l| l.principal.clone())
            .ok_or_else(denied)?;
        let a = Arc::new(Admission {
            id: token()?,
            principal,
            requested: now(),
            verb: op.permission(),
        });
        let fresh = a.clone();
        let k = lease.clone();
        self.authority_job(move |h| {
            let mut r = h.extensions.lock().unwrap();
            h.transaction(&mut r, |r, d| {
                h.eligible(r, d, &k, &fresh)
                    .map_err(|e| refusal(d, &fresh, None, e))
            })
        })
        .await?;
        let reader = self.extension_reader(&lease);
        let initial = match &reader {
            Ok(h) => h.prepare_extension(&lease, &op, None).await,
            Err(e) => Err(e.clone()),
        };
        let admit = a.clone();
        let k = lease.clone();
        let (plan, source) = self
            .authority_job(move |h| {
                let mut r = h.extensions.lock().unwrap();
                h.transaction(&mut r, |r, d| {
                    let plan = initial.map_err(|e| refusal(d, &admit, None, e))?;
                    h.eligible(r, d, &k, &admit)
                        .map_err(|e| refusal(d, &admit, None, e))?;
                    let target = plan.view.clone();
                    if !target_ok(&target)
                        || !h
                            .display_secrets()
                            .is_some_and(|s| safe_projection(&target, &s))
                    {
                        return Err(refusal(d, &admit, None, denied()));
                    }
                    if let Some(g) = d
                        .grants
                        .iter()
                        .find(|g| g.key == admit.principal.key && g.permission == admit.verb)
                    {
                        return Ok((plan, PermissionSource::Grant(g.grant_id.clone())));
                    }
                    if r.pending.len() >= 2 {
                        return Err(refusal(d, &admit, Some(target), Fault::DesktopBusy {}));
                    }
                    let t = token()?;
                    let decision = token()?;
                    let deadline = Instant::now() + TTL;
                    let (tx, rx) = oneshot::channel();
                    let c = Consent {
                        token: t.clone(),
                        decision_id: decision.clone(),
                        principal: admit.principal.clone(),
                        target: target.clone(),
                        requested_at: admit.requested,
                        expires_at: SafeUint::saturating(now().get() + 60000),
                        can_remember: admit.verb != Permission::InstallUpdate,
                    };
                    r.pending.insert(
                        t.clone(),
                        Waiting {
                            consent: c,
                            id: admit.id.clone(),
                            until: deadline,
                            send: Some(tx),
                        },
                    );
                    d.audit.push(admit.row(Some(target)));
                    Ok((
                        plan,
                        PermissionSource::Answer {
                            token: t,
                            decision,
                            deadline,
                            rx,
                        },
                    ))
                })
            })
            .await?;
        let reader = reader?;
        let (answer, decision_id, grant_id, choice) = match source {
            PermissionSource::Grant(g) => (None, None, Some(g), ExtDecision::Once),
            PermissionSource::Answer {
                token,
                decision,
                deadline,
                rx,
            } => {
                let choice = tokio::time::timeout_at(deadline.into(), rx)
                    .await
                    .ok()
                    .and_then(Result::ok)
                    .unwrap_or(ExtDecision::Deny);
                (Some(token), Some(decision), None, choice)
            }
        };
        let latest = if choice == ExtDecision::Deny {
            Err(denied())
        } else {
            reader.prepare_extension(&lease, &op, Some(&plan)).await
        };
        let complete = a.clone();
        let k = lease.clone();
        let (plan, _slot) = self
            .authority_job(move |h| {
                let mut r = h.extensions.lock().unwrap();
                h.transaction(&mut r, |r, d| {
                    let check = (|| {
                        h.eligible(r, d, &k, &complete)?;
                        let approved = match answer {
                            Some(ref t) => r.pending.remove(t).is_some_and(|w| {
                                w.id == complete.id && w.until > Instant::now() && w.send.is_none()
                            }),
                            None => d.grants.iter().any(|g| {
                                Some(&g.grant_id) == grant_id.as_ref()
                                    && g.permission == complete.verb
                                    && g.key == complete.principal.key
                            }),
                        };
                        if !approved || choice == ExtDecision::Deny {
                            return Err(denied());
                        }
                        let final_plan = latest?;
                        if !plan.same(&final_plan) {
                            return Err(changed());
                        }
                        if !h
                            .display_secrets()
                            .is_some_and(|s| safe_projection(&plan.view, &s))
                        {
                            return Err(denied());
                        }
                        if r.executing.len() >= 2 {
                            return Err(Fault::DesktopBusy {});
                        }
                        let guard = if complete.verb == Permission::InstallUpdate {
                            Some(slot()?)
                        } else {
                            None
                        };
                        let authority = if let Some(grant_id) = grant_id {
                            Authority::Grant { grant_id }
                        } else {
                            let created = if choice == ExtDecision::Remember {
                                if complete.verb == Permission::InstallUpdate {
                                    return Err(denied());
                                }
                                if let Some(existing) = d.grants.iter().find(|g| {
                                    g.key == complete.principal.key && g.permission == complete.verb
                                }) {
                                    Some(existing.grant_id.clone())
                                } else {
                                    let g = token()?;
                                    d.grants.push(Grant {
                                        grant_id: g.clone(),
                                        created_at: now(),
                                        key: complete.principal.key.clone(),
                                        permission: complete.verb,
                                    });
                                    Some(g)
                                }
                            } else {
                                None
                            };
                            Authority::Confirmation {
                                decision_id: decision_id.ok_or_else(denied)?,
                                created_grant_id: created,
                            }
                        };
                        Ok((authority, guard))
                    })();
                    let (authority, guard) = check.map_err(|e| refusal(d, &complete, None, e))?;
                    let row = if let Some(i) =
                        d.audit.iter().position(|a| a.operation_id == complete.id)
                    {
                        &mut d.audit[i]
                    } else {
                        d.audit.push(complete.row(Some(plan.view.clone())));
                        d.audit.last_mut().unwrap()
                    };
                    row.authority = authority;
                    row.outcome = ExtOutcome::Executing;
                    row.finished_at = None;
                    row.code = None;
                    r.executing.insert(complete.id.clone());
                    Ok((plan, guard))
                })
            })
            .await?;
        let result = reader.execute_extension(plan, progress).await;
        let outcome = if result.is_ok() {
            ExtOutcome::Succeeded
        } else {
            ExtOutcome::Failed
        };
        let code = result.as_ref().err().map(fault_code);
        self.authority_job(move |h| {
            let mut r = h.extensions.lock().unwrap();
            let saved = h.transaction(&mut r, |r, d| {
                r.executing.remove(&a.id);
                d.finish(&a.id, outcome, code);
                Ok(())
            });
            r.executing.remove(&a.id);
            saved
        })
        .await?;
        result
    }
}

use crate::desktop::instances::{config_error, start_error, stop_error};
use crate::{
    config, daemon,
    ops::{restart, start, stop},
    spawn, spec,
};
struct Prepared {
    view: TargetView,
    action: Action,
}
enum Action {
    Start {
        spec: spec::InstanceSpec,
        revision: [u8; 32],
        addr: String,
    },
    Stop(daemon::InstanceRecord),
    Restart {
        info: daemon::InstanceRecord,
        entry: spec::RestoreEntry,
        revision: [u8; 32],
    },
    Install(String),
}
impl Prepared {
    fn same(&self, other: &Self) -> bool {
        self.view == other.view
            && match (&self.action, &other.action) {
                (
                    Action::Start {
                        spec: a,
                        revision: x,
                        ..
                    },
                    Action::Start {
                        spec: b,
                        revision: y,
                        ..
                    },
                ) => a == b && x == y,
                (
                    Action::Restart {
                        entry: a,
                        revision: x,
                        ..
                    },
                    Action::Restart {
                        entry: b,
                        revision: y,
                        ..
                    },
                ) => a == b && x == y,
                (Action::Stop(_), Action::Stop(_)) => true,
                (Action::Install(a), Action::Install(b)) => a == b,
                _ => false,
            }
    }
}
impl DesktopHome {
    async fn prepare_extension(
        &self,
        lease: &str,
        op: &ExtensionWrite,
        previous: Option<&Prepared>,
    ) -> R<Prepared> {
        match op {
            ExtensionWrite::Start(t) => {
                let o = self.resolve_choice(t).await?;
                let exact_path = o.path.to_str().ok_or_else(denied)?.to_owned();
                if o.choice.path.contains("***") || o.choice.label.contains("***") {
                    return Err(denied());
                }
                let file = (!o.implicit || o.path.exists()).then_some(o.path.as_path());
                let ov = config::Overrides::default();
                let cfg = start::resolve_runtime_config_in(&self.home, &ov, file)
                    .map_err(|e| config_error(&e))?;
                let port = daemon::port_of(&cfg.listen_addr)
                    .parse()
                    .map_err(|_| denied())?;
                let path = match previous.map(|p| &p.action) {
                    Some(Action::Start { spec, .. }) => PathBuf::from(&spec.log_path),
                    _ => start::daemon_log_path_in(&self.home, cfg.log_file.as_deref()),
                };
                Ok(Prepared {
                    view: TargetView::InstanceStart {
                        kind: t.kind,
                        label: o.choice.label,
                        path: exact_path,
                        port,
                    },
                    action: Action::Start {
                        spec: spec::capture(file, ov, &path),
                        revision: cfg.launch_revision(),
                        addr: cfg.listen_addr,
                    },
                })
            }
            ExtensionWrite::Stop(run) => Ok(Prepared {
                view: TargetView::InstanceStop { run: run.clone() },
                action: Action::Stop(self.selected_run(run, false).await?),
            }),
            ExtensionWrite::Restart(run) => {
                let info = self.selected_run(run, false).await?;
                let entry = spec::read_restore_in(&self.home.join("run"), &run.port.to_string())
                    .ok()
                    .flatten()
                    .ok_or(Fault::DesktopNoSpec { port: run.port })?;
                let cfg = start::resolve_runtime_config_in(
                    &self.home,
                    &entry.spec.overrides,
                    entry.spec.config_file().as_deref(),
                )
                .map_err(|e| config_error(&e))?;
                Ok(Prepared {
                    view: TargetView::InstanceRestart { run: run.clone() },
                    action: Action::Restart {
                        info,
                        entry,
                        revision: cfg.launch_revision(),
                    },
                })
            }
            ExtensionWrite::Install(check) => Ok(Prepared {
                view: self.checked_offer(lease, check).await?,
                action: Action::Install(check.clone()),
            }),
        }
    }
    async fn execute_extension(&self, p: Prepared, progress: Progress) -> R<ExtensionResult> {
        match p.action {
            Action::Install(check) => self
                .install_update(check, progress)
                .await
                .map(ExtensionResult::Installed),
            Action::Stop(info) => {
                let port = daemon::port_of(&info.listen_addr)
                    .parse()
                    .map_err(|_| changed())?;
                stop::stop_in(
                    &self.home,
                    &info,
                    stop::StopMode::Graceful,
                    stop::Intent::Stop,
                    stop::Authority::Observed,
                )
                .await
                .map_err(|e| stop_error(port, e))?;
                Ok(ExtensionResult::Stopped)
            }
            Action::Start {
                spec,
                revision,
                addr,
            } => {
                let port = daemon::port_of(&addr).parse().map_err(|_| changed())?;
                let status = match start::preflight_in(&self.home, &addr)
                    .await
                    .map_err(|_| Fault::DesktopPort { port })?
                {
                    start::Preflight::AlreadyRunning(v) | start::Preflight::BecameReady(v) => *v,
                    start::Preflight::Clear => {
                        let v = spawn::spawn_checked(&self.home, &self.exe, &spec, revision)
                            .await
                            .map_err(|e| start_error(e, false))?
                            .ok_or_else(changed)?;
                        start::ensure_watchdog_if_enabled(&self.home, &self.exe);
                        v
                    }
                };
                self.ready(status.instance).map(ExtensionResult::Run)
            }
            Action::Restart {
                info,
                entry,
                revision,
            } => {
                let port = daemon::port_of(&info.listen_addr);
                let run = self.home.join("run");
                restart::precheck(&self.home, &info, port, &entry.spec)
                    .await
                    .map_err(|_| changed())?;
                if spec::read_restore_in(&run, port).ok().flatten().as_ref() != Some(&entry)
                    || !start::resolve_runtime_config_in(
                        &self.home,
                        &entry.spec.overrides,
                        entry.spec.config_file().as_deref(),
                    )
                    .is_ok_and(|cfg| cfg.launch_revision() == revision)
                {
                    return Err(changed());
                }
                stop::stop_in(
                    &self.home,
                    &info,
                    stop::StopMode::Graceful,
                    stop::Intent::Restart,
                    stop::Authority::Observed,
                )
                .await
                .map_err(|e| stop_error(port.parse().unwrap_or_default(), e))?;
                let status = spawn::spawn_checked(&self.home, &self.exe, &entry.spec, revision)
                    .await
                    .map_err(|e| start_error(e, true))?
                    .ok_or_else(changed)?;
                daemon::retire_moved_port_records_in(
                    &run,
                    port,
                    daemon::port_of(&status.instance.listen_addr),
                );
                self.ready(status.instance).map(ExtensionResult::Run)
            }
        }
    }
}

impl ExtensionOps for DesktopHome {
    fn bind_extensions(&self, session: String, support: ScriptSupport, notify: ExtensionNotify) {
        self.extension_attach(session, support, notify)
    }
    fn detach_extensions(&self, session: Option<&str>) {
        self.extension_detach(session)
    }
    fn package_hint(&self) -> u64 {
        self.hint()
    }
    fn inventory(&self) -> Inventory {
        self.extension_inventory()
    }
    fn enable_package(&self, key: &ExtKey, enabled: bool) -> R<()> {
        self.extension_enable(key, enabled)
    }
    fn open_package(&self, session: &str, key: &ExtKey) -> R<Bundle> {
        self.extension_open(session, key)
    }
    fn close_package(&self, lease: &str) {
        self.extension_close(lease)
    }
    fn check_lease(&self, lease: &str) -> R<()> {
        self.extension_reader(lease).map(|_| ())
    }
    fn package_choices(&self, lease: String) -> Task<'_, Vec<Choice>> {
        Box::pin(async move {
            let result = self.extension_reader(&lease)?.list_configs().await?;
            self.check_lease(&lease)?;
            Ok(result)
        })
    }
    fn package_config(&self, lease: String, target: ConfigTarget) -> Task<'_, ConfigView> {
        Box::pin(async move {
            let result = self.extension_reader(&lease)?.read_config(target).await?;
            self.check_lease(&lease)?;
            Ok(result)
        })
    }
    fn package_offer(&self, lease: &str) -> R<Option<UpdateInfo>> {
        self.extension_update_offer(lease)
    }
    fn attributed_write(
        &self,
        lease: String,
        op: ExtensionWrite,
        slot: InstallSlot,
        progress: Progress,
    ) -> Task<'_, ExtensionResult> {
        Box::pin(self.extension_write(lease, op, slot, progress))
    }
    fn pending_consents(&self, session: &str) -> R<Vec<Consent>> {
        self.extension_pending(session)
    }
    fn answer_consent(&self, session: &str, token: &str, decision: ExtDecision) -> R<()> {
        self.extension_answer(session, token, decision)
    }
    fn revoke_grant(&self, key: &ExtKey, grant: &str) -> R<()> {
        self.extension_revoke(key, grant)
    }
    fn package_activity(&self) -> R<Vec<Audit>> {
        self.extension_audit()
    }
}
