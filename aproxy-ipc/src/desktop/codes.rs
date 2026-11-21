//! Fixed vocabularies for data from other processes. A localized key can only be a
//! compiled variant; arbitrary peer strings and arbitrary `details` never cross.
use super::SafeUint;
use serde::{
    Deserialize, Serialize,
    de::value::{Error, StrDeserializer},
};

/// Attempt or terminal reason on a history row, decision or card.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryCode {
    /// unrecognized history or diagnostic code; localize by context, do not pass peer text through.
    Unknown,
    /// The Host header is not in the allowed set.
    InboundHostRejected,
    /// The request carries an Origin that is not allowed.
    InboundOriginRejected,
    /// The request body exceeds max_request_mb.
    RequestTooLarge,
    /// Reading the client's request body failed midway.
    ClientBodyAborted,
    /// Upgrade (WebSocket) requests are not supported.
    UpgradeUnsupported,
    /// Writing the request or response body to disk failed.
    LocalStorageFailed,
    /// The upstream response exceeds max_response_mb.
    ResponseTooLarge,
    /// The attempts allowed by a retry rule or a per-request override are used up.
    RetryExhausted,
    /// A format's decide stage stopped the retries.
    RetryStopped,
    /// A request-side format answered fail with retry:false.
    FormatRejected,
    /// A required format's circuit is open, so a request-direction stage cannot use it.
    FormatUnavailable,
    /// A forward request's request-side format failed or asked for a retry, and the request was not upgraded to retry.
    FormatFailed,
    /// A forward request's upstream attempt failed and the request was not upgraded to retry.
    UpstreamFailed,
    /// The stream is already committed, the upstream's content encoding cannot be carried, and decoding failed.
    DecodeFailed,
    /// The instance is shutting down.
    ShuttingDown,
    /// A defect in aproxy itself.
    Internal,
    /// Could not connect to the upstream: DNS, refused, TLS.
    UpstreamConnectFailed,
    /// Connecting to the upstream timed out.
    UpstreamConnectTimeout,
    /// The request was sent but no response headers arrived in time.
    UpstreamFirstByteTimeout,
    /// Too long passed between two reads of the response body.
    UpstreamReadIdleTimeout,
    /// The response body broke off midway.
    UpstreamBodyInterrupted,
    /// The upstream answered 4xx or 5xx.
    UpstreamStatus,
    /// The upstream answered 2xx but the body is an error.
    UpstreamErrorBody,
    /// A format failed at the process level or produced invalid output.
    FormatError,
    /// A format answered fail with retry:true.
    FormatFail,
    /// A format's decide stage judged a successful attempt as failed.
    FormatDecision,
}

impl HistoryCode {
    /// Classify a source literal before it can become a page localization key.
    pub fn classify(name: &str) -> Self {
        Self::deserialize(StrDeserializer::<Error>::new(name)).unwrap_or(Self::Unknown)
    }
}

/// Configuration diagnostic suitable for localization without parser prose.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagCode {
    /// unrecognized history or diagnostic code; localize by context, do not pass peer text through.
    Unknown,
    /// TOML or JSON syntax error.
    ConfigSyntax,
    /// A value has the wrong type.
    ConfigType,
    /// A value is not allowed.
    ConfigInvalidValue,
    /// A required key is missing.
    ConfigMissingKey,
    /// A key that is not recognized.
    ConfigUnknownKey,
    /// A form whose shape is defined but which is not implemented yet.
    ConfigNotSupported,
    /// A combination of settings that exclude each other.
    ConfigConflict,
    /// A secret placeholder written back does not match any original value.
    SecretPlaceholderUnmatched,
    /// A credential in the settings defaults was not inherited because the target differs, and this config has none of its own.
    DefaultCredentialWithheld,
    /// The defaults' header tables contain non-secret headers, which are sent to every target.
    DefaultHeaderShared,
    /// The default delivery is forward while the chain has a decide program or an optional request program: when they are unavailable, requests are not retried.
    ForwardDefaultUnguarded,
    /// The default delivery is forward and the chain has a response program; that layer gathers each response, so those responses are not streamed. Declare stream instead where the program supports it.
    ForwardResponseBuffered,
    /// settings.json has a schema newer than this version understands.
    SettingsSchemaTooNew,
    /// A format's handshake failed at startup.
    FormatHandshakeFailed,
    /// A format does not support a stage the config declares for it.
    FormatStageUnsupported,
    /// The group or others can access the home or a config file.
    FilePermissionsTooOpen,
    /// The config listens on a non-loopback address, so other hosts can reach the proxy.
    ListenNotLoopback,
    /// Several config files listen on the same port, so they cannot run at the same time.
    PortShared,
    /// A directory listed in settings.json `config_dirs` does not exist.
    ConfigDirMissing,
    /// A config file named by an alias or by `default_config` does not exist or cannot be read.
    ConfigFileUnreadable,
    /// settings.json sets `watchdog.heartbeat_secs` so high that a hung instance is detected only after minutes.
    WatchdogSlowHangDetection,
    /// settings.json sets `watchdog.max_restarts` to 0, so the watchdog never restarts a crashed or hung instance.
    WatchdogRestartsDisabled,
    /// The desired-state record needed for the rolling restart cannot be read.
    InstallRestoreUnreadable,
    /// The instance configuration needed for the rolling restart is missing or unreadable.
    InstallConfigUnreadable,
}

impl DiagCode {
    /// Classify a source literal before it can become a page localization key.
    pub fn classify(name: &str) -> Self {
        Self::deserialize(StrDeserializer::<Error>::new(name)).unwrap_or(Self::Unknown)
    }
}

/// A classified installer failure; other child codes are not passed through.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallCode {
    /// Another install is in progress in this home.
    InstallInProgress,
    /// The requested version does not exist.
    InstallVersionNotFound,
    /// The requested version is older than the oldest one this home can be moved to.
    InstallBelowFloor,
    /// Downloading the binary failed.
    InstallDownloadFailed,
    /// The downloaded binary does not match its checksum.
    InstallChecksumMismatch,
    /// The new binary did not run, or reported a version other than the one being installed.
    InstallProbeFailed,
    /// Replacing the binary in bin/ failed.
    InstallSwapFailed,
    /// The install failed and the previous binary was put back.
    InstallRolledBack,
    /// The install failed for a reason that no more specific code covers.
    InstallFailed,
    /// Live instances started by an older version are in this home and cannot be restarted by this install.
    InstallUnmanagedInstances,
    /// An instance did not start on the new binary, so the rolling restart stopped.
    InstallRestartFailed,
    /// Live foreground instances are in this home; an install cannot restart a foreground instance.
    InstallForegroundInstances,
    /// The precheck before the rolling restart did not pass.
    InstallPrecheckFailed,
}

impl InstallCode {
    /// Classify a source literal before it can become a page localization key.
    pub fn classify(name: &str) -> Option<Self> {
        Self::deserialize(StrDeserializer::<Error>::new(name)).ok()
    }
}

/// Whether a diagnostic prevents saving or starting.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Severity {
    /// Error.
    Error,
    /// Warning.
    Warning,
    /// Info.
    Info,
}

/// Distinguish launch from readiness failure without leaking a command line.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StartReason {
    /// Spawn.
    Spawn,
    /// Not ready.
    NotReady,
}

/// Why an identity-checked stop did not complete.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StopReason {
    /// Still running.
    StillRunning,
    /// Unverified.
    Unverified,
    /// Permission.
    Permission,
    /// Failed.
    Failed,
}

/// The only private files accessible through the desktop state API.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StateFile {
    /// Preferences.
    Preferences,
    /// Bookmarks.
    Bookmarks,
}

/// Which published desktop storage bound was exceeded.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LimitResource {
    /// Bookmark count.
    BookmarkCount,
    /// Bookmark bytes.
    BookmarkBytes,
    /// Preference bytes.
    PreferenceBytes,
    /// Theme bytes.
    ThemeBytes,
}

/// Why the OS registration cannot be safely read or changed.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RegistrationReason {
    /// Unsupported path.
    UnsupportedPath,
    /// Permission.
    Permission,
    /// Binary missing.
    BinaryMissing,
    /// Entry mismatch.
    EntryMismatch,
    /// System unavailable.
    SystemUnavailable,
    /// Verification failed.
    VerificationFailed,
}

/// Redacted location and localization code, never a parser excerpt or input value.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Issue {
    /// Classify even when the peer is newer than the window.
    pub code: DiagCode,
    /// Whether the issue blocks a write.
    pub severity: Severity,
    /// Redacted document key, absent for a file-wide issue.
    pub key: Option<String>,
    /// Redacted display location, not authority for an operation.
    pub file: String,
    /// One-based source coordinates when supplied.
    pub line: Option<SafeUint>,
    /// One-based source column when supplied.
    pub column: Option<SafeUint>,
}

/// Structured rejection for bundled window commands. Empty details are `{}`, not
/// null. OS/IPC/serde messages can quote secrets and are not variants of this type.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(
    tag = "code",
    content = "details",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum Fault {
    /// Malformed arguments; never quote input.
    DesktopBadInput {},
    /// Required local service or IO unavailable.
    DesktopUnavailable {},
    /// Native window operation failed (adapter normalization).
    DesktopWindow {},
    /// Page cursor is stale, invalid, reused or wrong scope.
    DesktopCursor {},
    /// The target generation, desired record or checked update changed.
    DesktopChanged {},
    /// History is unsafe, unsupported or unreadable.
    DesktopHistory {},
    /// The request or body is no longer available.
    DesktopGone {},
    /// Detail selection was closed or superseded.
    DesktopCancelled {},
    /// An exclusive operation is already running.
    DesktopBusy {},
    /// Selected config or desired target is missing or has no file.
    DesktopTarget {},
    /// External Windows config write requires explicit warning acknowledgement.
    DesktopExternalAcl {},
    /// Write lock busy or conflict with an external edit.
    DesktopConflict {},
    /// Installed home executable unavailable.
    DesktopBinary {},
    /// Child stream/terminal/exit did not satisfy the update contract.
    DesktopInstallProtocol {},
    /// A graceful operation cannot reach this run.
    DesktopUnresponsive { port: u16 },
    /// Target is not positively identified as unresponsive.
    DesktopForceRefused { port: u16 },
    /// Restart needs a readable desired-state specification.
    DesktopNoSpec { port: u16 },
    /// The requested listener is unavailable.
    DesktopPort { port: u16 },
    /// Configuration/settings have blocking diagnostics.
    DesktopConfig { diagnostics: Vec<Issue> },
    /// Launch or readiness failed; oldStopped describes the restart consequence.
    DesktopStart {
        reason: StartReason,
        old_stopped: bool,
    },
    /// Stop did not complete or could not be verified.
    DesktopStop { port: u16, reason: StopReason },
    /// Private desktop file could not be accessed/replaced.
    DesktopStateIo { file: StateFile },
    /// Corrupt state was replaced; reread before retrying the mutation.
    DesktopStateReset { file: StateFile },
    /// A bookmark or preference count/size limit was exceeded.
    DesktopLimit {
        resource: LimitResource,
        max: SafeUint,
    },
    /// The per-user registration could not be inspected, changed or verified.
    DesktopRegistration { reason: RegistrationReason },
    /// Another install is in progress in this home.
    InstallInProgress {},
    /// The requested version does not exist.
    InstallVersionNotFound {},
    /// The requested version is older than the oldest one this home can be moved to.
    InstallBelowFloor {},
    /// Downloading the binary failed.
    InstallDownloadFailed {},
    /// The downloaded binary does not match its checksum.
    InstallChecksumMismatch {},
    /// The new binary did not run, or reported a version other than the one being installed.
    InstallProbeFailed {},
    /// Replacing the binary in bin/ failed.
    InstallSwapFailed {},
    /// The install failed and the previous binary was put back.
    InstallRolledBack {},
    /// The install failed for a reason that no more specific code covers.
    InstallFailed {},
    /// Live instances started by an older version are in this home and cannot be restarted by this install.
    InstallUnmanagedInstances {},
    /// An instance did not start on the new binary, so the rolling restart stopped.
    InstallRestartFailed { port: u16 },
    /// Live foreground instances are in this home; an install cannot restart a foreground instance.
    InstallForegroundInstances { ports: Vec<u16> },
    /// The precheck before the rolling restart did not pass.
    InstallPrecheckFailed { port: u16, diagnostics: Vec<Issue> },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::code::{Family, TABLE};
    #[test]
    fn every_pass_through_family_requires_classification() {
        for entry in TABLE {
            let f = entry.families;
            assert_eq!(
                HistoryCode::classify(entry.name) != HistoryCode::Unknown,
                f.contains(&Family::Terminal) || f.contains(&Family::Attempt),
                "{}",
                entry.name
            );
            assert_eq!(
                DiagCode::classify(entry.name) != DiagCode::Unknown,
                f.contains(&Family::Diagnostic),
                "{}",
                entry.name
            );
            assert_eq!(
                InstallCode::classify(entry.name).is_some(),
                f.contains(&Family::Install),
                "{}",
                entry.name
            );
        }
        assert_eq!(
            HistoryCode::classify("private peer text"),
            HistoryCode::Unknown
        );
        assert_eq!(DiagCode::classify("private parser text"), DiagCode::Unknown);
        assert!(InstallCode::classify("private child text").is_none());
    }
    #[test]
    fn fault_shapes_never_include_decoder_or_system_messages() {
        assert_eq!(
            serde_json::to_value(Fault::DesktopBadInput {}).unwrap(),
            serde_json::json!({"code":"desktop_bad_input","details":{}})
        );
        assert_eq!(
            serde_json::to_value(Fault::DesktopStart {
                reason: StartReason::NotReady,
                old_stopped: true
            })
            .unwrap(),
            serde_json::json!({"code":"desktop_start","details":{"reason":"not-ready","oldStopped":true}})
        );
    }
}
