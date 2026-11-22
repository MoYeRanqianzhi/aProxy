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
