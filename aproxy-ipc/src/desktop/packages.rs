//! The package boundary separates local byte validation from optional publisher facts.
use super::Task;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// Themes contain only appearance data; extensions execute code in the main page.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PackageKind {
    /// Data-only appearance.
    Theme,
    /// Executable package with an ES module entry.
    Extension,
}
/// Cleaned display text, retaining language choices for the page.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Text {
    /// One label in the author's language.
    Plain(String),
    /// Language tags mapped to display labels.
    Localized(BTreeMap<String, String>),
}
/// Validated, bounded manifest fields. Unknown keys stay on disk, not in the DTO.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageManifest {
    /// Supported manifest format.
    pub schema: u8,
    /// Selects the entry file and allowed resources.
    pub kind: PackageKind,
    /// Portable installation directory name.
    pub id: String,
    /// Human-readable package label.
    pub name: Text,
    /// Version compared using SemVer precedence.
    pub version: String,
    /// Publisher's self-reported name.
    pub author: String,
    /// Human-readable summary.
    pub description: Text,
    /// Self-reported source, not proof of origin.
    pub repository: Option<String>,
    /// License expression supplied by the publisher.
    pub license: Option<String>,
    /// Paired format programs.
    pub formats: Vec<String>,
    /// Disclosed network hosts, not an access control list.
    pub network: Vec<String>,
    /// Disclosed write categories, not permissions.
    pub writes: Vec<String>,
}
/// Source facts are about the checked bytes, never a source path to reopen later.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum PackageSource {
    /// Hash covers the entire original zip, not the extracted tree.
    File {
        name: String,
        sha256: String,
        bytes: u64,
    },
    /// An unpacked tree has no release-file digest.
    Directory { name: String },
}
/// File inventory sorted by the slash-separated package-relative path.
#[derive(Debug, Clone, Serialize)]
pub struct PackageFile {
    /// Package-relative name.
    pub path: String,
    /// Actual extracted size.
    pub bytes: u64,
}
/// Prior installation, so the page can warn about a downgrade before committing.
#[derive(Debug, Clone, Serialize)]
pub struct Installed {
    /// Existing manifest version.
    pub version: String,
    /// Whether the existing extension is enabled.
    pub enabled: bool,
}
/// One checked and staged package awaiting the page's decisions.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageCheck {
    /// Single-use window-local handle.
    pub token: String,
    /// Cleaned manifest.
    pub manifest: PackageManifest,
    /// Installed file inventory.
    pub files: Vec<PackageFile>,
    /// Actual sum of unpacked file lengths.
    pub total_bytes: u64,
    /// Source facts available without network access.
    pub source: PackageSource,
    /// Existing package, when this check would replace it.
    pub installed: Option<Installed>,
    /// Parsed appearance object; the page validates its fields.
    pub theme: Option<Map<String, Value>>,
}
/// A portable localization key and bounded context; never includes absolute source paths.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageIssue {
    /// Stable package rejection category.
    pub reason: String,
    /// Recognized manifest field, if applicable.
    pub field: Option<String>,
    /// Cleaned package-relative path, if applicable.
    pub path: Option<String>,
    /// Budget name, when exceeded.
    pub limit: Option<String>,
    /// Maximum allowed for that budget.
    pub max: Option<u64>,
}
/// Package operations are home-bound. File and archive work runs off the event loop.
pub trait PackageOps: Send + Sync + 'static {
    /// Stage picked bytes without trusting the filename as a path.
    fn package_check_file(&self, name: String, bytes: Vec<u8>) -> Task<'_, PackageCheck>;
    /// Stage a user-selected local zip or directory.
    fn package_check_path(&self, path: String) -> Task<'_, PackageCheck>;
    /// Cancel only the specified check; retries are harmless.
    fn package_discard(&self, token: String) -> Task<'_, ()>;
}
