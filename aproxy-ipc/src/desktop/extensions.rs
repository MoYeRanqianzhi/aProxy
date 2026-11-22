//! Public projections for local extension packages. None of these identifiers is a filesystem path.
use super::{Fault, SafeUint};
use serde::{Deserialize, Serialize};

/// Selects the interpreter, not an authority level.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExtKind {
    /// One navigation page.
    Page,
    /// A render/icon module in the same isolated frame.
    Mascot,
    /// Declarative colors and bounded appearance settings.
    Theme,
}

/// The complete set of attributable operations. Config has no write capability.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Permission {
    /// Start one verified choice.
    InstanceStart,
    /// Gracefully stop one verified run.
    InstanceStop,
    /// Restart a run with its verified desired state.
    InstanceRestart,
    /// Execute the exact current offer, always with one-shot consent.
    InstallUpdate,
}

/// Bounded reason for rejecting a whole package or inventory.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InvalidReason {
    /// Too many root entries.
    PackageCount,
    /// Read failure or concurrent replacement.
    Io,
    /// Unsafe component or alias.
    Path,
    /// Link, reparse point or mount crossing.
    Link,
    /// Wrong filesystem object type.
    FileType,
    /// Tree too deep.
    Depth,
    /// Too many files.
    FileCount,
    /// One file exceeds its budget.
    FileBytes,
    /// Combined file bytes exceed the package budget.
    PackageBytes,
    /// Missing or unexpected member.
    Layout,
    /// Invalid module or text encoding.
    Text,
    /// Invalid, ambiguous or overdeep JSON.
    Json,
    /// Unknown manifest format.
    ManifestVersion,
    /// Invalid known manifest value.
    Field,
    /// Invalid theme string.
    Theme,
    /// A safe projection was not possible.
    PrivateData,
}

/// Fixed display selector, never a hostile filename or JSON key.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExtField {
    /// Extensions root.
    Root,
    /// Package directory.
    Directory,
    /// Manifest document.
    Manifest,
    /// Format discriminator.
    ManifestVersion,
    /// Stable directory identity.
    Id,
    /// Untrusted display label.
    Name,
    /// Package version.
    Version,
    /// Interpreter selection.
    Kind,
    /// Entrypoint.
    Entry,
    /// Declared write vocabulary.
    Permissions,
    /// Bundled data files.
    Assets,
    /// Declarative theme data.
    Theme,
}

/// Reason private authority cannot be used.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExtStateReason {
    /// State cannot be read or atomically replaced.
    Io,
    /// State is malformed or internally inconsistent.
    Corrupt,
    /// A later schema must not be overwritten.
    TooNew,
}

/// Byte-exact installed principal, also checked on disk before every authorization.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExtKey {
    /// Validated package directory id.
    pub id: String,
    /// Hex SHA-256 of the path-framed complete snapshot.
    pub digest: String,
}
/// Information safe to display outside the sandbox. The name asserts no publisher identity.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Principal {
    /// Exact immutable identity, independent of package version.
    #[serde(flatten)]
    pub key: ExtKey,
    /// Cleaned display name.
    pub name: String,
    /// Validated semantic version.
    pub version: String,
    /// Whether code or declarative data is present.
    pub kind: ExtKind,
}
/// Remembered decision for one control verb of one exact package in this home.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Grant {
    /// Package whose manifest declared the verb.
    #[serde(flatten)]
    pub key: ExtKey,
    /// Opaque record selector, not authority to execute.
    pub grant_id: String,
    /// Never InstallUpdate in persistent state.
    pub permission: Permission,
    /// Time of the explicit user decision.
    pub created_at: SafeUint,
}
/// Settings row; copying the files does not enable the package.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageInfo {
    /// Identity and plain-text label for enablement and consent.
    #[serde(flatten)]
    pub principal: Principal,
    /// Requested capabilities, not granted capabilities.
    pub permissions: Vec<Permission>,
    /// Set only for an explicitly enabled matching digest.
    pub enabled: bool,
    /// Explicitly remembered grants for this digest.
    pub grants: Vec<Grant>,
}
/// Invalid directories are reported as a whole, never loaded partially.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct InvalidPackage {
    /// Absent if unsafe to display or the entire root was rejected.
    pub id: Option<String>,
    /// Machine-readable rejection with a fixed field selector.
    #[serde(flatten)]
    pub fault: Fault,
}

/// Script execution is a property of this running window, never a page preference.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ScriptSupport {
    /// The native boundary and the shipped shell were both verified.
    Available,
    /// Declarative themes remain usable without a script engine qualification.
    Unverified,
}
/// Complete settings inventory even if persisted authority is unreadable.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Inventory {
    /// Packages with current exact identities.
    pub packages: Vec<PackageInfo>,
    /// Whole-package failures without attacker-controlled diagnostics.
    pub invalid: Vec<InvalidPackage>,
    /// Controls whether scripted rows can be enabled or opened.
    pub script_support: ScriptSupport,
    /// When present all rows are disabled and no grants apply.
    pub state_fault: Option<Fault>,
}
/// Inert bytes passed to the sandbox, never registered as resource URLs.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionAsset {
    /// Bounded manifest-local selector, without directory or suffix.
    pub key: String,
    /// Standard padded base64 of the captured file.
    pub data_b64: String,
}
/// Immutable module snapshot bound to exactly one Rust-issued lease.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptBundle {
    /// Private to the shipped host; never forward to the frame.
    pub lease: String,
    /// Standard padded base64 of the exact verified module.
    pub module_b64: String,
    /// No path names or arbitrary MIME types enter the host resource server.
    pub assets: Vec<ExtensionAsset>,
}
/// A theme needs neither script execution nor a lease.
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Bundle {
    /// Only validated appearance fields, not the source string.
    Theme { theme: String },
    /// Module for a visible navigation page.
    Page(ScriptBundle),
    /// Module for a visible mascot slot.
    Mascot(ScriptBundle),
}
/// A dialog describes resolved native data, never a target label proposed by a frame.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case", deny_unknown_fields)]
pub enum TargetView {
    /// Fully resolved and rechecked start choice.
    InstanceStart {
        kind: super::ChoiceKind,
        label: String,
        path: String,
        port: u16,
    },
    /// Graceful stop of one process generation.
    InstanceStop { run: super::Run },
    /// Restart of one generation using its checked desired state.
    InstanceRestart { run: super::Run },
    /// Exact offered candidate; the installer affects this whole home.
    InstallUpdate {
        version: String,
        channel: super::UpdateChannel,
        variant: super::Variant,
        scope: InstallScope,
    },
}
/// Installer scope is not a set of processes chosen by a package.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InstallScope {
    /// Existing home-wide rolling install operation.
    AllHomeInstances,
}
impl TargetView {
    /// The single manifest verb required for this resolved target.
    pub fn permission(&self) -> Permission {
        match self {
            Self::InstanceStart { .. } => Permission::InstanceStart,
            Self::InstanceStop { .. } => Permission::InstanceStop,
            Self::InstanceRestart { .. } => Permission::InstanceRestart,
            Self::InstallUpdate { .. } => Permission::InstallUpdate,
        }
    }
}
/// Trusted host dialog, not a message to an extension.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Consent {
    /// Single-use random answer token, never persisted.
    pub token: String,
    /// Non-redeemable audit correlation.
    pub decision_id: String,
    /// Exact package the user enabled.
    pub principal: Principal,
    /// Render this verified target outside the frame.
    pub target: TargetView,
    /// Unix milliseconds at admission.
    pub requested_at: SafeUint,
    /// Display timestamp; the actual deadline is monotonic.
    pub expires_at: SafeUint,
    /// Always false for install.
    pub can_remember: bool,
}
/// Result of an explicit action in the trusted page.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExtDecision {
    /// Default, including dialog dismissal.
    Deny,
    /// Covers this one exact plan.
    Once,
    /// Also remember the exact instance verb for this package in this home.
    Remember,
}
/// How a single attributed operation was authorized, with no redeemable tokens.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Authority {
    /// No positive decision.
    None,
    /// Covered by a previously remembered verb.
    Grant { grant_id: String },
    /// Approved by the user for this exact operation.
    Confirmation {
        decision_id: String,
        created_grant_id: Option<String>,
    },
}
/// Durable transition; an interrupted execution is never replayed.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExtOutcome {
    /// Waiting for a trusted decision.
    Pending,
    /// Authorization is durable but no terminal result has been saved.
    Executing,
    /// No side effect was admitted.
    Denied,
    /// The underlying operation completed successfully.
    Succeeded,
    /// The underlying operation returned a fault.
    Failed,
    /// Interruption prevents a reliable terminal claim.
    Unknown,
}
/// Bounded local activity row, separate from the transient answer token.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Audit {
    /// Random unique activity identifier, not authority.
    pub operation_id: String,
    /// Exact principal named by this record.
    #[serde(flatten)]
    pub key: ExtKey,
    /// Version at admission (the digest, not this label, selects the grant).
    pub version: String,
    /// Requested manifest verb.
    pub op: Permission,
    /// Absent for attempts rejected before resolving a safe target.
    pub target: Option<TargetView>,
    /// Admission time in Unix milliseconds.
    pub requested_at: SafeUint,
    /// Terminal time, if known.
    pub finished_at: Option<SafeUint>,
    /// Correlation with the explicit grant or user decision.
    pub authority: Authority,
    /// Executing is durable before the operation may begin.
    pub outcome: ExtOutcome,
    /// Stable fault identifier only; never arbitrary child or OS text.
    pub code: Option<String>,
}

/// Typed native input. There is deliberately no serde decoder or generic invoke for this enum.
#[derive(Clone)]
pub enum ExtensionWrite {
    /// Start an observed choice from the lease's own listing.
    Start(super::StartTarget),
    /// Gracefully stop one observed generation.
    Stop(super::Run),
    /// Relaunch one observed generation from its verified specification.
    Restart(super::Run),
    /// Consume an existing check, never initiate one.
    Install(String),
}
impl ExtensionWrite {
    /// Select the exact grant scope before resolving the target.
    pub fn permission(&self) -> Permission {
        match self {
            Self::Start(_) => Permission::InstanceStart,
            Self::Stop(_) => Permission::InstanceStop,
            Self::Restart(_) => Permission::InstanceRestart,
            Self::Install(_) => Permission::InstallUpdate,
        }
    }
}
/// Terminal result internal to the host, narrowed by the individual bridge command.
#[derive(Debug)]
pub enum ExtensionResult {
    /// Start or restart reports the new observed generation.
    Run(super::Run),
    /// Graceful stop completed.
    Stopped,
    /// Installer's verified terminal result.
    Installed(super::InstallEnd),
}
/// Reserve the same installer slot used by page actions, after authorization only.
pub type InstallSlot = std::sync::Arc<dyn Fn() -> super::R<Box<dyn Send + Sync>> + Send + Sync>;
/// Window invalidations carry neither answer tokens nor activity contents.
pub type ExtensionNotify = std::sync::Arc<dyn Fn(super::DesktopEvent) + Send + Sync>;

/// Home-bound extension boundary. Sync file operations must run on a blocking executor.
pub trait ExtensionOps: Send + Sync + 'static {
    /// Bind a fresh attachment and its trusted native qualification.
    fn bind_extensions(&self, session: String, support: ScriptSupport, notify: ExtensionNotify);
    /// Fence pending work synchronously, including hide-on-close.
    fn detach_extensions(&self, session: Option<&str>);
    /// Refresh the complete on-disk inventory without executing anything.
    fn inventory(&self) -> Inventory;
    /// Compare and set the exact enabled digest from the trusted settings page.
    fn enable_package(&self, key: &ExtKey, enabled: bool) -> super::R<()>;
    /// Capture and mount only one complete enabled identity.
    fn open_package(&self, session: &str, key: &ExtKey) -> super::R<Bundle>;
    /// Retire the matching lease only.
    fn close_package(&self, lease: &str);
    /// No filesystem work on history chunks; validate the current immutable mount.
    fn check_lease(&self, lease: &str) -> super::R<()>;
    /// Use a choice table private to this mount, never the page's nonce.
    fn package_choices(&self, lease: String) -> super::Task<'_, Vec<super::Choice>>;
    /// A form projected through the ordinary source-redacted config reader.
    fn package_config(
        &self,
        lease: String,
        target: super::ConfigTarget,
    ) -> super::Task<'_, super::ConfigView>;
    /// Return the last page-initiated check without starting an external process.
    fn package_offer(&self, lease: &str) -> super::R<Option<super::UpdateInfo>>;
    /// Remain pending through the decision and execution, without returning an execution ticket.
    fn attributed_write(
        &self,
        lease: String,
        op: ExtensionWrite,
        slot: InstallSlot,
        progress: super::Progress,
    ) -> super::Task<'_, ExtensionResult>;
    /// Only the attached trusted host can obtain redeemable answer tokens.
    fn pending_consents(&self, session: &str) -> super::R<Vec<Consent>>;
    /// Consume one exact answer, never accepting new arguments or an action identifier.
    fn answer_consent(&self, session: &str, token: &str, decision: ExtDecision) -> super::R<()>;
    /// Remove one selected durable verb grant.
    fn revoke_grant(&self, key: &ExtKey, id: &str) -> super::R<()>;
    /// Reproject recent activity against the current secret set.
    fn package_activity(&self) -> super::R<Vec<Audit>>;
}
