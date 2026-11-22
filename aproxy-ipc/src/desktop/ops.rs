//! Typed, home-bound operations injected into the window. Only Rust selects paths,
//! executables and processes. None of the DTOs in this module are CLI documents.
use super::{Fault, Issue, SafeUint, StateFile, TomlIssue};
use serde::{Deserialize, Deserializer, Serialize};
use std::{future::Future, pin::Pin, sync::Arc};

/// Opaque comparison handle; its string has no path or process semantics.
pub type Token = String;

/// Run identity as observed by the page. Re-resolve in the same home before use.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Run {
    /// Local listening port.
    pub port: u16,
    /// Server-issued generation, compared before any control operation.
    pub instance_id: String,
}

impl Run {
    /// Build a comparison key for the view. The view independently verifies current
    /// membership and the connected peer; this conversion grants no authority.
    pub fn key(&self) -> crate::view::RunKey {
        crate::view::RunKey {
            port: self.port,
            instance_id: self.instance_id.clone(),
        }
    }
}
/// A request remains tied to its original run, even after port reuse.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ref {
    /// Owner of this request.
    pub run: Run,
    /// Instance-generated request ID.
    pub request_id: String,
}

/// Kind of observed start choice; no arbitrary file argument exists.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub enum ChoiceKind {
    /// `default` in the application contract.
    #[serde(rename = "default")]
    Default,
    /// `config` in the application contract.
    #[serde(rename = "config")]
    Config,
    /// `alias` in the application contract.
    #[serde(rename = "alias")]
    Alias,
}

/// A member returned by the last configuration listing.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartTarget {
    /// How the backend must re-resolve the observation.
    pub kind: ChoiceKind,
    /// Private choice-table handle.
    pub id: Token,
}

/// A file form either selects a listed choice or follows a verified run's actual file.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ConfigTarget {
    /// The observed implicit or configured default.
    Default { id: Token },
    /// A discovered file, compared to the current listing.
    Config { id: Token },
    /// A registered alias, not the CLI path fallback.
    Alias { id: Token },
    /// A file used by this exact run.
    Run {
        run: Run,
        #[serde(default, rename = "expectedFile")]
        expected_file: Option<Token>,
    },
}

/// One registered configuration choice; path and label are display text only.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Choice {
    /// Return this observation when editing or starting.
    pub target: StartTarget,
    /// Redacted display label.
    pub label: String,
    /// Redacted resolved display path.
    pub path: String,
    /// Whether a regular file exists.
    pub exists: bool,
    /// Matches the effective default configuration.
    pub is_default: bool,
}

/// Source of the effective configuration value.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub enum Source {
    /// `override` in the application contract.
    #[serde(rename = "override")]
    Override,
    /// `toml` in the application contract.
    #[serde(rename = "toml")]
    Toml,
    /// `settings` in the application contract.
    #[serde(rename = "settings")]
    Settings,
    /// `env` in the application contract.
    #[serde(rename = "env")]
    Env,
    /// `default` in the application contract.
    #[serde(rename = "default")]
    Default,
}

/// Individually editable string-valued configuration keys.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub enum TextKey {
    /// `base_url` in the application contract.
    #[serde(rename = "base_url")]
    BaseUrl,
    /// `listen_addr` in the application contract.
    #[serde(rename = "listen_addr")]
    ListenAddr,
    /// `api_key` in the application contract.
    #[serde(rename = "api_key")]
    ApiKey,
    /// `proxy` in the application contract.
    #[serde(rename = "proxy")]
    Proxy,
    /// `proxy_username` in the application contract.
    #[serde(rename = "proxy_username")]
    ProxyUsername,
    /// `proxy_password` in the application contract.
    #[serde(rename = "proxy_password")]
    ProxyPassword,
    /// `keepalive.heartbeat` in the application contract.
    #[serde(rename = "keepalive.heartbeat")]
    KeepaliveHeartbeat,
}

/// Keys that accept only nonnegative safe integer seconds.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub enum SecondsKey {
    /// `timeouts.connect_secs` in the application contract.
    #[serde(rename = "timeouts.connect_secs")]
    TimeoutsConnectSecs,
    /// `timeouts.first_byte_secs` in the application contract.
    #[serde(rename = "timeouts.first_byte_secs")]
    TimeoutsFirstByteSecs,
    /// `timeouts.read_idle_secs` in the application contract.
    #[serde(rename = "timeouts.read_idle_secs")]
    TimeoutsReadIdleSecs,
    /// `keepalive.interval_secs` in the application contract.
    #[serde(rename = "keepalive.interval_secs")]
    KeepaliveIntervalSecs,
}

/// Dotted key for the keepalive trigger.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub enum TriggerKey {
    /// `keepalive.trigger` in the application contract.
    #[serde(rename = "keepalive.trigger")]
    KeepaliveTrigger,
}

/// Only supported trigger values may be set.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub enum TriggerValue {
    /// `any` in the application contract.
    #[serde(rename = "any")]
    Any,
    /// `accept` in the application contract.
    #[serde(rename = "accept")]
    Accept,
    /// `body-stream` in the application contract.
    #[serde(rename = "body-stream")]
    BodyStream,
}

/// Header maps whose individual members may be edited.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub enum HeaderTable {
    /// `extra_headers` in the application contract.
    #[serde(rename = "extra_headers")]
    ExtraHeaders,
    /// `override_headers` in the application contract.
    #[serde(rename = "override_headers")]
    OverrideHeaders,
}

/// A scalar key for an unset. Arbitrary file paths or TOML keys cannot be decoded.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ScalarKey {
    /// A text-valued key, possibly write-only.
    Text(TextKey),
    /// A timeout or interval.
    Seconds(SecondsKey),
    /// The fixed trigger enumeration.
    Trigger(TriggerKey),
}
/// Each accepted key determines its value type before entering the operation.
#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum Assignment {
    /// Full replacement text may contain a secret; never included in a result or a diagnostic.
    Text { key: TextKey, value: String },
    /// No implicit string or fractional conversion.
    Seconds { key: SecondsKey, value: SafeUint },
    /// A known enumeration value, not an arbitrary file literal.
    Trigger {
        key: TriggerKey,
        value: TriggerValue,
    },
}
/// Narrow edits; patch contents flow from the window to the backend only.
#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Edit {
    /// Change a single scalar.
    Set(Assignment),
    /// Remove a scalar override, possibly revealing an inherited value.
    Unset { key: ScalarKey },
    /// Change exactly one header. Its value is always write-only.
    SetHeader {
        key: HeaderTable,
        name: String,
        value: String,
    },
    /// Remove one local member without modifying inherited headers.
    UnsetHeader { key: HeaderTable, name: String },
}
/// Write-only values are identified explicitly rather than disguised as editable strings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Cell {
    /// No effective value.
    None,
    /// Presence only; no hint or clear text.
    Secret { set: bool },
    /// Display text after known-secret substitution.
    Text { text: String },
    /// A file boolean, not a text conversion.
    Bool { value: bool },
    /// Only arbitrary file integers are represented as decimal strings.
    Integer { decimal: String },
    /// Ordered file entries after schema projection.
    List { items: Vec<Cell> },
    /// Dynamic keys are data, not properties of the bridge object.
    Map { entries: Vec<MapCell> },
}

/// A projected name/value pair within a read-only tree.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MapCell {
    /// Redacted file key.
    pub name: String,
    /// Projected value.
    pub value: Cell,
}

/// Address of one editable member in the selected configuration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HeaderKey {
    /// Which file map.
    pub table: HeaderTable,
    /// Original header name if safe to show.
    pub name: String,
}

/// One effective configuration row and its provenance.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigField {
    /// Dotted file key for display.
    pub key: String,
    /// Projected value or secret presence.
    pub value: Cell,
    /// Absent for a missing key.
    pub source: Option<Source>,
    /// Address only for a safe header name.
    pub header: Option<HeaderKey>,
    /// Whether a localized control can accept an edit.
    pub editable: bool,
}

/// Why inherited credentials did not follow a changed target.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub enum WithheldReason {
    /// `base-url-differs` in the application contract.
    #[serde(rename = "base-url-differs")]
    BaseUrlDiffers,
    /// `proxy-differs` in the application contract.
    #[serde(rename = "proxy-differs")]
    ProxyDiffers,
}

/// Origin of a withheld default.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub enum SettingsSource {
    /// `settings` in the application contract.
    #[serde(rename = "settings")]
    Settings,
}

/// Explain withheld inheritance without disclosing the credential.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Withheld {
    /// Display key.
    pub key: String,
    /// Source of this default.
    pub from: SettingsSource,
    /// Why it does not apply.
    pub reason: WithheldReason,
    /// Masked original destination.
    pub default_target: String,
    /// Masked effective destination.
    pub target: String,
}

/// Effective file configuration on the next start, not private launch overrides.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigView {
    /// Compare the canonical file observed for this run; choice IDs already bind the file.
    pub file_token: Option<Token>,
    /// Redacted file display path.
    pub file: String,
    /// Projected scalar, list and header rows.
    pub fields: Vec<ConfigField>,
    /// Credentials withheld on target mismatch.
    pub withheld: Vec<Withheld>,
    /// Localizable diagnostics with no raw input.
    pub diagnostics: Vec<Issue>,
    /// Canonical Windows destination is not protected by the home ACL.
    pub external_acl: bool,
}

/// A dry-run of the complete patch; it never writes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Validation {
    /// Candidate introduces no blocking errors.
    pub can_save: bool,
    /// Candidate issues, including warnings.
    pub diagnostics: Vec<Issue>,
    /// Known conflict with a different run.
    pub occupied_port: Option<u16>,
    /// Outside-home Windows destination.
    pub external_acl: bool,
}

/// Outcome of a single locked configuration write.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Saved {
    /// File contents actually changed.
    pub changed: bool,
    /// Observed ports using the affected file.
    pub instances_to_restart: Vec<u16>,
}

/// One schema-selected read-only setting.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Setting {
    /// Published dotted field name.
    pub key: String,
    /// Projected effective value.
    pub value: Cell,
}

/// Global configuration, distinct from the window’s own preferences.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SettingsView {
    /// Effective watchdog setting.
    pub watchdog_enabled: bool,
    /// Settings not writable from the window.
    pub read_only: Vec<Setting>,
    /// Issues encountered while reading.
    pub diagnostics: Vec<Issue>,
}

/// Requested color scheme.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub enum Mode {
    /// `system` in the application contract.
    #[serde(rename = "system")]
    System,
    /// `light` in the application contract.
    #[serde(rename = "light")]
    Light,
    /// `dark` in the application contract.
    #[serde(rename = "dark")]
    Dark,
}

/// Intensity of the built-in palette.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub enum Palette {
    /// `soft` in the application contract.
    #[serde(rename = "soft")]
    Soft,
    /// `standard` in the application contract.
    #[serde(rename = "standard")]
    Standard,
    /// `vivid` in the application contract.
    #[serde(rename = "vivid")]
    Vivid,
}

/// Private presentation preferences, independent of settings.json.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Preferences {
    /// System or bounded BCP-47-shaped tag.
    pub lang: String,
    /// Requested appearance.
    pub mode: Mode,
    /// Built-in palette intensity.
    pub palette: Palette,
    /// Request Mica even if this system cannot display it.
    pub mica: bool,
    /// Bounded registry slug, interpreted by the page.
    pub mascot: String,
    /// Installed theme ID, or `builtin`.
    pub theme: String,
}

/// Partial update: only supplied keys are written to the editable document.
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreferencesPatch {
    /// Set `lang` only when the caller supplied it.
    #[serde(default, deserialize_with = "present")]
    pub lang: Option<String>,
    /// Set `mode` only when the caller supplied it.
    #[serde(default, deserialize_with = "present")]
    pub mode: Option<Mode>,
    /// Set `palette` only when the caller supplied it.
    #[serde(default, deserialize_with = "present")]
    pub palette: Option<Palette>,
    /// Set `mica` only when the caller supplied it.
    #[serde(default, deserialize_with = "present")]
    pub mica: Option<bool>,
    /// Set `mascot` only when the caller supplied it.
    #[serde(default, deserialize_with = "present")]
    pub mascot: Option<String>,
    /// Set `theme` only when the caller supplied it.
    #[serde(default, deserialize_with = "present")]
    pub theme: Option<String>,
}
fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Option<T>, D::Error> {
    T::deserialize(d).map(Some)
}

/// Request outcome recorded by the proxy, never inferred from status.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub enum Outcome {
    /// `in-flight` in the application contract.
    #[serde(rename = "in-flight")]
    InFlight,
    /// `ok` in the application contract.
    #[serde(rename = "ok")]
    Ok,
    /// `upstream-error` in the application contract.
    #[serde(rename = "upstream-error")]
    UpstreamError,
    /// `aproxy-error` in the application contract.
    #[serde(rename = "aproxy-error")]
    AproxyError,
    /// `format-response` in the application contract.
    #[serde(rename = "format-response")]
    FormatResponse,
    /// `client-gone` in the application contract.
    #[serde(rename = "client-gone")]
    ClientGone,
    /// `unknown` in the application contract.
    #[serde(rename = "unknown")]
    Unknown,
}

/// Small immutable summary captured from the verified view, never a body or header.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Bookmark {
    /// Original run and request ID.
    pub r#ref: Ref,
    /// Receipt time in Unix milliseconds.
    pub started_at: SafeUint,
    /// Bounded redacted method.
    pub method: String,
    /// Bounded redacted path.
    pub path: String,
    /// Bounded redacted model if known.
    pub model: Option<String>,
    /// Observed outcome at the time of pinning.
    pub outcome: Outcome,
    /// Attempt count at the time of pinning.
    pub attempts: u32,
}

/// Validated private window state plus notices for files reset after corruption.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DesktopState {
    /// Effective window preferences.
    pub preferences: Option<Preferences>,
    /// Ordered load list, absent when the document could not be read.
    pub enabled_extensions: Option<Vec<String>>,
    /// Non-destructive diagnostics for the editable TOML file.
    pub desktop_toml: Vec<TomlIssue>,
    /// Newest pins first.
    pub bookmarks: Vec<Bookmark>,
    /// Corrupt files reset with a visible notice.
    pub reset: Vec<StateFile>,
}

/// Trusted Rust-side change; the page cannot submit bookmark display data.
pub enum PinChange {
    /// Capture a projected verified request row.
    Pin(Bookmark),
    /// Remove exactly this reference; missing pins are a no-op.
    Unpin(Ref),
}

/// Observed mismatch, distinct from an absent registration.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub enum RegistrationIssue {
    /// `binary-missing` in the application contract.
    #[serde(rename = "binary-missing")]
    BinaryMissing,
    /// `entry-mismatch` in the application contract.
    #[serde(rename = "entry-mismatch")]
    EntryMismatch,
}

/// Read-back of this home’s actual user-level OS registration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Registration {
    /// Only true when executable and registration match.
    pub enabled: bool,
    /// Missing dependencies or unexpected command.
    pub issue: Option<RegistrationIssue>,
}

/// Build variant reported by the actual executable.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub enum Variant {
    /// `desktop` in the application contract.
    #[serde(rename = "desktop")]
    Desktop,
    /// `headless` in the application contract.
    #[serde(rename = "headless")]
    Headless,
}

/// Prepared host metadata; the installed binary may be a different version.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct About {
    /// Running executable version.
    pub version: String,
    /// Running executable variant.
    pub variant: Variant,
    /// Redacted display home.
    pub home: String,
}

/// Choice passed only to the installed check command.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub enum ChannelChoice {
    /// `auto` in the application contract.
    #[serde(rename = "auto")]
    Auto,
    /// `pre` in the application contract.
    #[serde(rename = "pre")]
    Pre,
}

/// Actual channel reported by the installed child.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub enum UpdateChannel {
    /// `stable` in the application contract.
    #[serde(rename = "stable")]
    Stable,
    /// `pre` in the application contract.
    #[serde(rename = "pre")]
    Pre,
}

/// Result of a completed check, not authorization to choose an arbitrary version.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateInfo {
    /// Actual child release channel.
    pub channel: UpdateChannel,
    /// Installed version at check time.
    pub current: String,
    /// Latest offered version in channel.
    pub latest: Option<String>,
    /// The child reported an eligible update.
    pub update_available: bool,
    /// Installed executable build variant.
    pub variant: Variant,
    /// Optional release timestamp in Unix milliseconds.
    pub published_at: Option<SafeUint>,
    /// Successful observation time.
    pub checked_at: SafeUint,
    /// Single-use token for the last eligible check.
    pub check: Option<Token>,
}

/// Last observed installation phase; it is not itself a terminal result.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub enum InstallPhase {
    /// `marking` in the application contract.
    #[serde(rename = "marking")]
    Marking,
    /// `downloading` in the application contract.
    #[serde(rename = "downloading")]
    Downloading,
    /// `downloaded` in the application contract.
    #[serde(rename = "downloaded")]
    Downloaded,
    /// `broadcasting` in the application contract.
    #[serde(rename = "broadcasting")]
    Broadcasting,
    /// `acked` in the application contract.
    #[serde(rename = "acked")]
    Acked,
    /// `swapping` in the application contract.
    #[serde(rename = "swapping")]
    Swapping,
    /// `swapped` in the application contract.
    #[serde(rename = "swapped")]
    Swapped,
    /// `restarting` in the application contract.
    #[serde(rename = "restarting")]
    Restarting,
    /// `verifying` in the application contract.
    #[serde(rename = "verifying")]
    Verifying,
    /// `cleaning` in the application contract.
    #[serde(rename = "cleaning")]
    Cleaning,
    /// `done` in the application contract.
    #[serde(rename = "done")]
    Done,
    /// `failed` in the application contract.
    #[serde(rename = "failed")]
    Failed,
    /// `aborted` in the application contract.
    #[serde(rename = "aborted")]
    Aborted,
    /// `unknown` in the application contract.
    #[serde(rename = "unknown")]
    Unknown,
}

/// Successful terminal result, accepted only after exit status verification.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub enum InstallOutcome {
    /// `installed` in the application contract.
    #[serde(rename = "installed")]
    Installed,
    /// `up-to-date` in the application contract.
    #[serde(rename = "up-to-date")]
    UpToDate,
}

/// Verified terminal result from this child.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstallEnd {
    /// Distinguish actual installation from no-op.
    pub outcome: InstallOutcome,
    /// Version reported by the child, if any.
    pub version: Option<String>,
}

/// Installer’s observed per-port result.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub enum InstanceOutcome {
    /// `restarted` in the application contract.
    #[serde(rename = "restarted")]
    Restarted,
    /// `rolled-back` in the application contract.
    #[serde(rename = "rolled-back")]
    RolledBack,
    /// `failed` in the application contract.
    #[serde(rename = "failed")]
    Failed,
    /// `unknown` in the application contract.
    #[serde(rename = "unknown")]
    Unknown,
}

/// Result of an individual rolling restart.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstallInstance {
    /// Affected listener.
    pub port: u16,
    /// Outcome from the child.
    pub outcome: InstanceOutcome,
}

/// Most recent child byte counters, independent of a percentage.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Download {
    /// Bytes transferred.
    pub bytes: SafeUint,
    /// Expected length if known.
    pub total: Option<SafeUint>,
}

/// Whole observed job state, safe to coalesce in the window queue.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Job {
    /// Opaque local job identity.
    pub id: Token,
    /// Confirmed candidate version.
    pub requested_version: String,
    /// Unknown before plan, empty for no instances.
    pub targets: Option<Vec<u16>>,
    /// Latest observed phase.
    pub phase: Option<InstallPhase>,
    /// Unix milliseconds at that phase.
    pub phase_at: Option<SafeUint>,
    /// Last byte counts.
    pub download: Option<Download>,
    /// Latest outcome by port.
    pub instances: Vec<InstallInstance>,
    /// Verified successful terminal.
    pub end: Option<InstallEnd>,
    /// Projected failure after child exit.
    pub fault: Option<Fault>,
}

/// Only the projected error object crosses this boundary.
pub type R<T> = Result<T, Fault>;
/// Object-safe asynchronous operation borrowing its injected home service.
pub type Task<'a, T> = Pin<Box<dyn Future<Output = R<T>> + Send + 'a>>;
/// Receives a complete projected replacement, never raw child output.
pub type Progress = Arc<dyn Fn(Job) + Send + Sync>;
/// Operations of one explicitly bound home. Implemented by the application library;
/// the Tauri shell knows neither the private configuration nor how to launch a process.
pub trait InstanceOps: Send + Sync + 'static {
    /// Enumerate discovered TOMLs, configured aliases including outside scan directories, and the default target.
    fn list_configs(&self) -> Task<'_, Vec<Choice>>;
    /// Re-resolve the offered choice inside this home, run the CLI's start preflight, launch with the running executable; return the verified run actually ready.
    fn start_instance(&self, target: StartTarget) -> Task<'_, Run>;
    /// Resolve by port, require same live identity at send; apply CLI stop intent (remove desire to run), never escalate.
    fn stop_instance(&self, run: Run) -> Task<'_, ()>;
    /// Require same live run and usable restore spec; success may be on a different port.
    fn restart_instance(&self, run: Run) -> Task<'_, Run>;
    /// Re-enumerate and match the exact run; require **unresponsive**; verify recorded PID + process creation marker at termination, not a page PID.
    fn force_stop_instance(&self, run: Run) -> Task<'_, ()>;
    /// Resolve only that port's pending desired record, compare its observed token, recheck absence/ownership and call restore.
    fn restore_instance(&self, port: u16, expected_restore: Token) -> Task<'_, Run>;
    /// Resolve only this port’s pending desire in the supplied home.
    fn forget_restore(&self, port: u16, expected_restore: Token) -> Task<'_, ()>;
}
/// Complete window service. Instance operations share the same home and authority
/// boundary as the configuration, update and registration methods.
pub trait ConfigOps: InstanceOps {
    /// Resolve allowed target afresh; resolve effective file layers exactly as the CLI (not a dump of live process memory).
    fn read_config(&self, target: ConfigTarget) -> Task<'_, ConfigView>;
    /// Validate typed patch against the on-disk target/defaults in memory.
    fn validate_config(&self, target: ConfigTarget, changes: Vec<Edit>) -> Task<'_, Validation>;
    /// Re-resolve and revalidate as part of locked read/modify/write, change only supplied keys and return affected ports.
    fn write_config(
        &self,
        target: ConfigTarget,
        changes: Vec<Edit>,
        acknowledge_external_acl: bool,
    ) -> Task<'_, Saved>;
    /// Read this home's current validated settings; no last-good substitution for a broken file.
    fn read_settings(&self) -> Task<'_, SettingsView>;
    /// Set only `watchdog.enabled` through settings edit; result is stored switch, not a claim a watchdog process is running.
    fn set_watchdog(&self, enabled: bool) -> Task<'_, bool>;
}
/// Interface state, installation and login operations of the same injected home.
pub trait StateOps: ConfigOps {
    /// Reload bookmark/preference projection.
    fn read_desktop_state(&self) -> Task<'_, DesktopState>;
    /// Validate supplied preference keys, merge under lock and return effective preferences.
    fn set_preferences(&self, changes: PreferencesPatch) -> Task<'_, Preferences>;
    /// Pin resolves the ref to a currently verified, redacted record (cache or metadata read); unpin compares stored identity only and also works offline.
    fn set_pinned(&self, change: PinChange) -> Task<'_, Vec<Bookmark>>;
    /// Use calling binary metadata and the injected home's display path.
    fn about(&self) -> R<About>;
}
/// Update checks and transactions share the home and window state already in use.
pub trait UpdateOps: StateOps {
    /// Close admission to checks and signal the active check to kill and reap its own child. Never cancel an install.
    fn close_checks(&self);
    /// Run only installed home executable, parse its check JSON, issue a confirmation token only for an available candidate.
    fn check_update(&self, channel: ChannelChoice) -> Task<'_, UpdateInfo>;
    /// Confirm last check token; install exactly the offered version, one job per window, observe `update-progress` and terminal promise.
    fn install_update(&self, check: Token, progress: Progress) -> Task<'_, InstallEnd>;
}
/// The complete injected desktop service, including explicit opt-in login registration.
pub trait DesktopOps: UpdateOps + super::ExtensionOps {
    /// Inspect this home's current-user OS registration and installed executable every time.
    fn read_autostart(&self) -> Task<'_, Registration>;
    /// Set/remove just this home's fixed registration, reread and verify.
    fn set_autostart(&self, enabled: bool) -> Task<'_, Registration>;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn config_inputs_are_typed_and_never_choose_a_path() {
        for input in [
            r#"{"op":"set","key":"timeouts.connect_secs","value":"2"}"#,
            r#"{"op":"set","key":"timeouts.connect_secs","value":-1}"#,
            r#"{"op":"set","key":"base_url","value":"ok","extra":1}"#,
            r#"{"op":"unset","key":"format.0.command"}"#,
        ] {
            assert!(serde_json::from_str::<Edit>(input).is_err());
        }
        assert!(
            serde_json::from_str::<Edit>(r#"{"op":"set","key":"timeouts.connect_secs","value":0}"#)
                .is_ok()
        );
        assert!(
            serde_json::from_str::<ConfigTarget>(
                r#"{"kind":"config","id":"choice","path":"outside"}"#
            )
            .is_err()
        );
    }
    #[test]
    fn selecting_builtin_is_not_an_omitted_patch() {
        let read = |s| serde_json::from_str::<PreferencesPatch>(s).unwrap();
        assert_eq!(read("{}").theme, None);
        assert_eq!(
            read(r#"{"theme":"builtin"}"#).theme.as_deref(),
            Some("builtin")
        );
        assert!(serde_json::from_str::<PreferencesPatch>(r#"{"theme":null}"#).is_err());
        assert!(serde_json::from_str::<PreferencesPatch>(r#"{"lang":null}"#).is_err());
    }
}
