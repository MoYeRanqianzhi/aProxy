//! Stable projections of already-redacted instance data. Unknown wire fields are not forwarded.
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Stable cache namespace; the listening port alone may be reused.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct RunKey {
    /// Listening port within this home.
    pub port: u16,
    /// Opaque instance-generated identifier for this run.
    pub instance_id: String,
}

/// Why request data cannot be exposed for an otherwise visible run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum HistoryUnavailable {
    /// Restart with a version that guarantees source-side redaction.
    SourceRedactionMissing,
}

/// Supported read paths derived from the verified snapshot's advertised operations.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ReadCapabilities {
    /// The instance supports both a subscription and the pages needed to join it.
    pub history_feed: bool,
    /// A detail document can be requested independently of the history feed.
    pub detail: bool,
    /// On-demand body chunks are supported.
    pub body: bool,
}

/// A discovered port, including stopped ports awaiting restoration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct InstanceSummary {
    /// Listening port, also present when there is no running process.
    pub port: u16,
    /// Recorded run, if the enumerator found a readable identity.
    pub run: Option<RunKey>,
    /// Process from that record; never an authorization to signal it.
    pub pid: Option<u32>,
    /// Version reported by this instance record.
    pub version: Option<String>,
    /// Run start time in Unix milliseconds, independent of the time it was discovered.
    pub started_at: Option<u64>,
    /// What the latest enumeration established.
    pub liveness: super::Presence,
    /// Whether this port is waiting for restoration.
    pub restore_pending: bool,
    /// Machine-readable reason restoration has not happened.
    pub restore_reason: Option<String>,
    /// Read operations supported by this run (before applying redaction policy).
    pub reads: ReadCapabilities,
    /// Machine-readable history restriction established by the verified snapshot.
    pub history_unavailable: Option<HistoryUnavailable>,
    /// Where state updates currently come from.
    pub feed: super::Feed,
    /// Verified operational state; absent when not live.
    pub state: Option<StateSnapshot>,
}

/// Operational snapshot for one run, independent of the subscription transport.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct StateSnapshot {
    /// Lifecycle phase; unrecognized phases are not evidence of death.
    pub state: String,
    /// Counters since this run started.
    pub activity: RunActivity,
}

/// Run-local activity; these counters never span restarts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct RunActivity {
    /// Last request time in Unix milliseconds.
    pub last_request_at: u64,
    /// Requests received.
    pub requests_total: u64,
    /// Attempts after the first.
    pub retries_total: u64,
    /// Requests that succeeded after retry.
    #[serde(default)]
    pub rescued_total: u64,
    /// Forward requests switched to retry mode.
    #[serde(default)]
    pub upgraded_total: u64,
    /// Unfinished deliveries.
    #[serde(default)]
    pub in_flight: u64,
    /// In-flight requests on a retry.
    #[serde(default)]
    pub retrying: u64,
    /// Failure counts by machine-readable code.
    #[serde(default)]
    pub by_code: std::collections::BTreeMap<String, u64>,
    /// Last source-redacted failure, if any occurred in this run.
    pub last_error: Option<LastFailure>,
    /// Rolling counters, oldest bucket first.
    #[serde(default)]
    pub buckets: Buckets,
}

/// Diagnostic data shared by polled and streamed snapshots.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct LastFailure {
    /// Stable failure code, empty for older peers without one.
    #[serde(default)]
    pub code: String,
    /// Code-specific context after instance-side redaction.
    #[serde(default)]
    pub details: Value,
    /// Source-redacted diagnostic, not localized display text.
    pub message: String,
    /// Failure time in Unix milliseconds.
    pub at: u64,
}

/// Bucket counts have a fixed width within a snapshot.
#[derive(Default, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Buckets {
    /// Seconds represented by one entry.
    pub width_secs: u64,
    /// Requests received per bucket.
    pub requests: Vec<u64>,
    /// Retry attempts per bucket.
    pub retries: Vec<u64>,
    /// Failed attempts per bucket.
    pub failures: Vec<u64>,
}

/// One complete list/event record, keyed by run and server sequence.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct RequestSummary {
    /// Run-local record mutation counter, independent of subscription delivery order.
    #[serde(default)]
    pub revision: u64,
    /// Server-generated identifier for detail and body reads.
    pub request_id: String,
    /// Sequence within this run; sorts newest first.
    pub request_seq: u64,
    /// Receipt time in Unix milliseconds.
    pub started_at: u64,
    /// Terminal time in Unix milliseconds; absent while in flight.
    pub ended_at: Option<u64>,
    /// Redacted client method.
    pub method: String,
    /// Redacted path, including masked query values.
    pub path: String,
    /// Redacted requested model if recognized.
    pub model: Option<String>,
    /// Whether the client requested streaming.
    pub stream: bool,
    /// Redacted User-Agent header if present.
    pub user_agent: Option<String>,
    /// Selected delivery mode.
    pub delivery: String,
    /// Whether delivery switched from forward to retry.
    pub upgraded: bool,
    /// Current or terminal outcome.
    pub outcome: String,
    /// Terminal error code when applicable.
    pub code: Option<String>,
    /// HTTP status delivered to the client.
    pub client_status: Option<u16>,
    /// Request lifecycle phase.
    pub phase: String,
    /// Scheduled retry time in Unix milliseconds.
    pub next_attempt_at: Option<u64>,
    /// Number of upstream attempts.
    pub attempts: u32,
    /// Attempts omitted from the bounded trace.
    pub tries_omitted: u32,
    /// Retained attempts in execution order.
    pub tries: Vec<AttemptSummary>,
    /// Whether keepalive is active.
    pub keepalive: bool,
    /// Heartbeat count.
    pub heartbeats: u64,
    /// Client request size.
    pub request_bytes: u64,
    /// Response size delivered.
    pub response_bytes: u64,
    /// Redacted request, response and stream reports.
    #[serde(default)]
    pub reports: Vec<FormatReport>,
    /// Whether the instance shortened this record.
    #[serde(default)]
    pub truncated: bool,
}

/// One retained upstream attempt.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct AttemptSummary {
    /// One-based attempt number.
    pub attempt: u32,
    /// Start time in Unix milliseconds.
    pub started_at: u64,
    /// Elapsed milliseconds if finished.
    pub duration_ms: Option<u64>,
    /// Status from upstream when available.
    pub upstream_status: Option<u16>,
    /// Failure code if this attempt failed.
    pub code: Option<String>,
    /// Bytes received from upstream.
    pub response_bytes: u64,
    /// Retry decision if one has been made.
    pub decision: Option<Decision>,
    /// Bounded reports for this attempt.
    #[serde(default)]
    pub reports: Vec<FormatReport>,
}

/// Retry decision recorded by the instance.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Decision {
    /// Rule identifier or redacted format name.
    pub by: String,
    /// Whether to retry or stop.
    pub decision: String,
    /// Actual backoff time.
    pub delay_ms: Option<u64>,
    /// Explicit rerouting decision if present.
    pub reroute: Option<bool>,
}

/// Bounded, source-redacted format report.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct FormatReport {
    /// Redacted format name.
    pub format: String,
    /// Stage that produced this report.
    pub stage: String,
    /// Redacted report message.
    pub summary: String,
    /// Typed-by-discriminator, source-redacted report actions.
    pub actions: Vec<Value>,
}

/// On-demand detail; it describes bodies but never contains their bytes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct RequestDetail {
    /// Request state at the instant of this read.
    pub record: RequestSummary,
    /// Client headers and body metadata.
    pub request: MessageDetail,
    /// Details for the attempts the instance still retains.
    pub tries: Vec<AttemptDetail>,
}

/// Recorded sent and received heads of one attempt.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct AttemptDetail {
    /// One-based attempt number.
    pub attempt: u32,
    /// Upstream request, if sent.
    pub sent: Option<SentDetail>,
    /// Upstream response, if received.
    pub response: Option<ResponseDetail>,
    /// Reason a complete response was unavailable.
    pub error: Option<AttemptError>,
}

/// Source-redacted explanation of an unsuccessful attempt.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct AttemptError {
    /// Machine-readable failure code.
    pub code: String,
    /// Redacted diagnostic text.
    pub message: String,
    /// Code-specific redacted details.
    pub details: Value,
}

/// Client request metadata.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct MessageDetail {
    /// Recorded headers in source order.
    pub headers: Vec<Header>,
    /// Retention information; no body content.
    pub body: Option<BodyInfo>,
}

/// Metadata for the actual upstream request.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SentDetail {
    /// Redacted sent method.
    pub method: String,
    /// Redacted upstream URL.
    pub url: String,
    /// Recorded headers in source order.
    pub headers: Vec<Header>,
    /// Retention information; no body content.
    pub body: Option<BodyInfo>,
}

/// Metadata for the actual upstream response.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ResponseDetail {
    /// Upstream HTTP status.
    pub upstream_status: u16,
    /// Recorded headers in source order.
    pub headers: Vec<Header>,
    /// Retention information; no body content.
    pub body: Option<BodyInfo>,
}

/// Header as recorded by the instance, including repeated names.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Header {
    /// Header name with known secrets replaced.
    pub name: String,
    /// Redacted text or a secret placeholder, never a raw credential.
    pub value: Value,
}

/// Metadata available without reading the body.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct BodyInfo {
    /// Retained decoded size.
    pub bytes: u64,
    /// Retention state, including shared bodies.
    pub state: String,
    /// Why the body was not retained.
    pub cause: Option<String>,
    /// Redacted wire content encoding.
    pub encoding: Option<String>,
}

/// One on-demand read from the redacted body store.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct BodyChunk {
    /// Total available when read.
    pub total_bytes: u64,
    /// Byte offset of this chunk.
    pub offset: u64,
    /// Base64-encoded redacted bytes.
    pub data_b64: String,
    /// Whether more bytes are available now.
    pub more: bool,
    /// Whether the instance has finished recording the body.
    pub complete: bool,
}
