//! Allowlisted projection of history from a verified, source-redacting peer. Extra
//! fields on a peer object are never copied through to the bundled window.
use super::{Fault, HistoryCode, Outcome, R, Ref, Run, SafeUint};
use crate::{
    redact::{self, Secrets},
    view,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Recorded request phase, not a UI animation state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Phase {
    /// `reading_body` at the source.
    ReadingBody,
    /// `transforming` at the source.
    Transforming,
    /// `attempting` at the source.
    Attempting,
    /// `backoff` at the source.
    Backoff,
    /// `replaying` at the source.
    Replaying,
    /// `forwarding` at the source.
    Forwarding,
    /// `done` at the source.
    Done,
    /// `unknown` at the source.
    Unknown,
}
impl Phase {
    fn classify(s: &str) -> Self {
        match s {
            "reading_body" => Self::ReadingBody,
            "transforming" => Self::Transforming,
            "attempting" => Self::Attempting,
            "backoff" => Self::Backoff,
            "replaying" => Self::Replaying,
            "forwarding" => Self::Forwarding,
            "done" => Self::Done,
            _ => Self::Unknown,
        }
    }
}

/// Selected proxy delivery mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Delivery {
    /// `forward` at the source.
    Forward,
    /// `retry` at the source.
    Retry,
    /// `unknown` at the source.
    Unknown,
}
impl Delivery {
    pub(super) fn classify(s: &str) -> Self {
        match s {
            "forward" => Self::Forward,
            "retry" => Self::Retry,
            _ => Self::Unknown,
        }
    }
}

/// Recorded retry decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DecisionKind {
    /// `retry` at the source.
    Retry,
    /// `stop` at the source.
    Stop,
    /// `unknown` at the source.
    Unknown,
}
impl DecisionKind {
    fn classify(s: &str) -> Self {
        match s {
            "retry" => Self::Retry,
            "stop" => Self::Stop,
            _ => Self::Unknown,
        }
    }
}

/// Stage which produced the report.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Stage {
    /// `request` at the source.
    Request,
    /// `retry` at the source.
    Retry,
    /// `decide` at the source.
    Decide,
    /// `response` at the source.
    Response,
    /// `stream` at the source.
    Stream,
    /// `heartbeat` at the source.
    Heartbeat,
    /// `notify` at the source.
    Notify,
    /// `unknown` at the source.
    Unknown,
}
impl Stage {
    fn classify(s: &str) -> Self {
        match s {
            "request" => Self::Request,
            "retry" => Self::Retry,
            "decide" => Self::Decide,
            "response" => Self::Response,
            "stream" => Self::Stream,
            "heartbeat" => Self::Heartbeat,
            "notify" => Self::Notify,
            _ => Self::Unknown,
        }
    }
}

/// What can be read from the peer body store.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Retention {
    /// `retained` at the source.
    Retained,
    /// `same_as_previous` at the source.
    SameAsPrevious,
    /// `evicted` at the source.
    Evicted,
    /// `not_retained` at the source.
    NotRetained,
    /// `unknown` at the source.
    Unknown,
}
impl Retention {
    fn classify(s: &str) -> Self {
        match s {
            "retained" => Self::Retained,
            "same_as_previous" => Self::SameAsPrevious,
            "evicted" => Self::Evicted,
            "not_retained" => Self::NotRetained,
            _ => Self::Unknown,
        }
    }
}

/// Reason for non-retention without internal error text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Loss {
    /// `budget_disabled` at the source.
    BudgetDisabled,
    /// `exceeds_budget` at the source.
    ExceedsBudget,
    /// `decode_failed` at the source.
    DecodeFailed,
    /// `unknown` at the source.
    Unknown,
}
impl Loss {
    fn classify(s: &str) -> Self {
        match s {
            "budget_disabled" => Self::BudgetDisabled,
            "exceeds_budget" => Self::ExceedsBudget,
            "decode_failed" => Self::DecodeFailed,
            _ => Self::Unknown,
        }
    }
}

/// A short-lived projector bound to all known credentials for the observed data. The
/// separate name matcher accounts for HTTP header names normalized to lowercase.
pub struct Projection<'a> {
    secrets: &'a Secrets,
    names: Secrets,
}
impl<'a> Projection<'a> {
    /// Supply resolved secrets (including overwritten values), never a serialized JSON document.
    pub fn new(secrets: &'a Secrets) -> Self {
        Self {
            secrets,
            names: secrets.for_header_names(),
        }
    }
    pub(super) fn text(&self, s: &str) -> String {
        self.secrets.replace(s)
    }
    fn name(&self, s: &str) -> String {
        let lower = s.to_ascii_lowercase();
        if self.names.find(lower.as_bytes()).is_some() {
            self.text(&self.names.replace(&lower))
        } else {
            self.text(s)
        }
    }
    /// A bookmark can address an evicted row but cannot invent an arbitrary peer
    /// Keep a bounded immutable summary only after projecting a verified record.
    /// Cutting may remove the rest of a credential, so mask a matching prefix too.
    pub fn bookmark(&self, row: Row) -> super::Bookmark {
        let cut = |s: String| {
            if s.len() <= 256 {
                return s;
            }
            let end = s.floor_char_boundary(256);
            let bytes = self.secrets.replace_cut_prefix(&s.as_bytes()[..end]);
            let mut out = String::from_utf8_lossy(&bytes).into_owned();
            out.truncate(out.floor_char_boundary(256));
            out
        };
        super::Bookmark {
            r#ref: row.r#ref,
            started_at: row.started_at,
            method: cut(row.method),
            path: cut(row.path),
            model: row.model.map(cut),
            outcome: row.outcome,
            attempts: row.attempts,
        }
    }
    /// argument. Request IDs consist of this exact run followed by a canonical sequence.
    pub fn check_ref(&self, reference: &Ref) -> R<view::RunKey> {
        let key = reference.run.key();
        self.run(&key).ok_or(Fault::DesktopChanged {})?;
        let seq = reference
            .request_id
            .strip_prefix(&key.instance_id)
            .and_then(|s| s.strip_prefix('-'))
            .and_then(|s| s.parse::<u64>().ok());
        if self.secrets.find(reference.request_id.as_bytes()).is_some()
            || !seq.is_some_and(|n| reference.request_id == format!("{}-{n}", key.instance_id))
        {
            return Err(Fault::DesktopBadInput {});
        }
        Ok(key)
    }
    /// The process-generated 16 lowercase hex digits are a precondition, not authority.
    pub fn run(&self, k: &view::RunKey) -> Option<Run> {
        (k.instance_id.len() == 16
            && k.instance_id
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            && self.secrets.find(k.instance_id.as_bytes()).is_none())
        .then(|| Run {
            port: k.port,
            instance_id: k.instance_id.clone(),
        })
    }
}
fn time(ms: u64) -> R<SafeUint> {
    SafeUint::try_from(ms).map_err(|_| Fault::DesktopHistory {})
}
fn opt_time(ms: Option<u64>) -> R<Option<SafeUint>> {
    ms.map(time).transpose()
}
fn outcome(s: &str) -> Outcome {
    match s {
        "in_flight" => Outcome::InFlight,
        "ok" => Outcome::Ok,
        "upstream_error" => Outcome::UpstreamError,
        "aproxy_error" => Outcome::AproxyError,
        "format_response" => Outcome::FormatResponse,
        "client_gone" => Outcome::ClientGone,
        _ => Outcome::Unknown,
    }
}
/// Retry decision without program-private data.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Decision {
    /// Rule identifier or redacted format name.
    pub by: String,
    /// Whether to retry or stop.
    pub decision: DecisionKind,
    /// Actual backoff time.
    pub delay_ms: Option<SafeUint>,
    /// Explicit rerouting decision if present.
    pub reroute: Option<bool>,
}

/// A report action cannot smuggle arbitrary JSON through its data or discriminator.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(
    tag = "type",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum Action {
    /// Protocol identifiers, not a program to execute.
    ProtocolConvert {
        from: String,
        to: String,
        label: Option<String>,
    },
    /// The public alias, never the key value.
    KeySelect {
        key_id: String,
        label: Option<String>,
    },
    /// Original and selected model if known.
    Route {
        channel: String,
        model_from: Option<String>,
        model_to: Option<String>,
        label: Option<String>,
    },
    /// An operation and header name; no value.
    HeaderEdit {
        name: String,
        op: EditOp,
        label: Option<String>,
    },
    /// An operation and JSON pointer; no body data.
    BodyEdit {
        path: String,
        op: EditOp,
        label: Option<String>,
    },
    /// Unknown or malformed action; retain only its scrubbed label.
    Other { label: Option<String> },
}
/// Only the public modification kind, not its operands.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EditOp {
    /// Set a value.
    Set,
    /// Remove a value.
    Remove,
}
/// Source-redacted report with only recognized action fields.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    /// Redacted format name.
    pub format: String,
    /// Stage that produced this report.
    pub stage: Stage,
    /// Redacted report message.
    pub summary: String,
    /// Typed-by-discriminator, source-redacted report actions.
    pub actions: Vec<Action>,
}

/// One actual attempt; an omitted attempt is never fabricated.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Try {
    /// One-based attempt number.
    pub attempt: u32,
    /// Start time in Unix milliseconds.
    pub started_at: SafeUint,
    /// Elapsed milliseconds if finished.
    pub duration_ms: Option<SafeUint>,
    /// Status from upstream when available.
    pub upstream_status: Option<u16>,
    /// Failure code if this attempt failed.
    pub code: Option<HistoryCode>,
    /// Bytes received from upstream.
    pub response_bytes: SafeUint,
    /// Retry decision if one has been made.
    pub decision: Option<Decision>,
    /// Bounded reports for this attempt.
    pub reports: Vec<Report>,
}

/// One summary scoped to its verified run; never contains headers or body bytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    /// Stable pair of run and original request ID.
    pub r#ref: Ref,
    /// Descending sequence within this run, not a clock.
    pub seq: SafeUint,
    /// Run-local record mutation counter, independent of subscription delivery order.
    pub revision: SafeUint,
    /// Receipt time in Unix milliseconds.
    pub started_at: SafeUint,
    /// Terminal time in Unix milliseconds; absent while in flight.
    pub ended_at: Option<SafeUint>,
    /// Redacted client method.
    pub method: String,
    /// Redacted path, including masked query values.
    pub path: String,
    /// Redacted requested model if recognized.
    pub model: Option<String>,
    /// Redacted User-Agent header if present.
    pub user_agent: Option<String>,
    /// Whether the client requested streaming.
    pub stream: bool,
    /// Selected delivery mode.
    pub delivery: Delivery,
    /// Whether delivery switched from forward to retry.
    pub upgraded: bool,
    /// Current or terminal outcome.
    pub outcome: Outcome,
    /// Terminal error code when applicable.
    pub code: Option<HistoryCode>,
    /// HTTP status delivered to the client.
    pub client_status: Option<u16>,
    /// Request lifecycle phase.
    pub phase: Phase,
    /// Scheduled retry time in Unix milliseconds.
    pub next_attempt_at: Option<SafeUint>,
    /// Number of upstream attempts.
    pub attempts: u32,
    /// Attempts omitted from the bounded trace.
    pub tries_omitted: u32,
    /// Retained attempts in execution order.
    pub tries: Vec<Try>,
    /// Whether keepalive is active.
    pub keepalive: bool,
    /// Heartbeat count.
    pub heartbeats: SafeUint,
    /// Client request size.
    pub request_bytes: SafeUint,
    /// Response size delivered.
    pub response_bytes: SafeUint,
    /// Redacted request, response and stream reports.
    pub reports: Vec<Report>,
    /// Whether the instance shortened this record.
    pub truncated: bool,
}

impl Row {
    /// Called only for a live completion notification. A page or replay can never
    /// authorize a celebration, even if its summary satisfies this predicate.
    pub fn rescued_at(&self) -> Option<SafeUint> {
        let end = self.ended_at?;
        (self.outcome == Outcome::Ok
            && self.attempts >= 2
            && (self.attempts >= 4 || end.get().saturating_sub(self.started_at.get()) >= 15_000))
            .then_some(end)
    }
}
/// The only outbound body selector; attempt is one-based for upstream messages.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum BodyPart {
    /// Client request body.
    Request,
    /// Sent request of this attempt.
    Sent { attempt: std::num::NonZeroU32 },
    /// Response of this attempt.
    Response { attempt: std::num::NonZeroU32 },
}
/// No hint from the source marker is forwarded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum HeaderValue {
    /// Non-secret header after substitution.
    Text(String),
    /// Only constructed with `secret: true` by the projector.
    Secret { secret: bool },
}
/// One header in source order; classify the original name before scrubbing it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Header {
    /// Header name with known secrets replaced.
    pub name: String,
    /// Redacted text or a secret placeholder, never a raw credential.
    pub value: HeaderValue,
}

/// Metadata available without reading any body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BodyInfo {
    /// Retained decoded size.
    pub bytes: SafeUint,
    /// Retention state, including shared bodies.
    pub state: Retention,
    /// Why the body was not retained.
    pub cause: Option<Loss>,
    /// Redacted wire content encoding.
    pub encoding: Option<String>,
}

/// Client headers and retention metadata, never a reconstructed raw request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    /// Recorded headers in source order.
    pub headers: Vec<Header>,
    /// Retention information; no body content.
    pub body: Option<BodyInfo>,
}

/// The request actually sent upstream on this attempt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Sent {
    /// Redacted sent method.
    pub method: String,
    /// Redacted upstream URL.
    pub url: String,
    /// Recorded headers in source order.
    pub headers: Vec<Header>,
    /// Retention information; no body content.
    pub body: Option<BodyInfo>,
}

/// The upstream response metadata, if one was received.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    /// Upstream HTTP status.
    pub upstream_status: u16,
    /// Recorded headers in source order.
    pub headers: Vec<Header>,
    /// Retention information; no body content.
    pub body: Option<BodyInfo>,
}

/// Source-scrubbed attempt diagnostic with a classified code and no details object.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttemptError {
    /// Machine-readable failure code.
    pub code: HistoryCode,
    /// Redacted diagnostic text.
    pub message: String,
}

/// Recorded metadata for one identified attempt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TryDetail {
    /// One-based attempt number.
    pub attempt: u32,
    /// Upstream request, if sent.
    pub sent: Option<Sent>,
    /// Upstream response, if received.
    pub response: Option<Response>,
    /// Reason a complete response was unavailable.
    pub error: Option<AttemptError>,
}

/// Metadata read on demand for one selected request; no body content.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Detail {
    /// Request state at the instant of this read.
    pub record: Row,
    /// Client headers and body metadata.
    pub request: Message,
    /// Details for the attempts the instance still retains.
    pub tries: Vec<TryDetail>,
}

impl Projection<'_> {
    /// Retry decision without program-private data.
    fn decision(&self, r: view::Decision) -> Decision {
        Decision {
            by: self.text(&r.by),
            decision: DecisionKind::classify(&r.decision),
            delay_ms: r.delay_ms.map(SafeUint::saturating),
            reroute: r.reroute,
        }
    }

    fn action(&self, v: Value) -> Action {
        let text = |k: &str| v.get(k)?.as_str().map(|s| self.text(s));
        let label = text("label");
        let op = match v["op"].as_str() {
            Some("set") => Some(EditOp::Set),
            Some("remove") => Some(EditOp::Remove),
            _ => None,
        };
        (|| {
            Some(match v["type"].as_str()? {
                "protocol_convert" => Action::ProtocolConvert {
                    from: text("from")?,
                    to: text("to")?,
                    label: label.clone(),
                },
                "key_select" => Action::KeySelect {
                    key_id: text("key_id")?,
                    label: label.clone(),
                },
                "route" => Action::Route {
                    channel: text("channel")?,
                    model_from: text("model_from"),
                    model_to: text("model_to"),
                    label: label.clone(),
                },
                "header_edit" => Action::HeaderEdit {
                    name: self.name(v["name"].as_str()?),
                    op: op?,
                    label: label.clone(),
                },
                "body_edit" => Action::BodyEdit {
                    path: text("path")?,
                    op: op?,
                    label: label.clone(),
                },
                _ => return None,
            })
        })()
        .unwrap_or(Action::Other { label })
    }

    /// Source-redacted report with only recognized action fields.
    fn report(&self, r: view::FormatReport) -> Report {
        Report {
            format: self.text(&r.format),
            stage: Stage::classify(&r.stage),
            summary: self.text(&r.summary),
            actions: r.actions.into_iter().map(|a| self.action(a)).collect(),
        }
    }

    /// One actual attempt; an omitted attempt is never fabricated.
    fn attempt(&self, r: view::AttemptSummary) -> R<Try> {
        Ok(Try {
            attempt: r.attempt,
            started_at: time(r.started_at)?,
            duration_ms: r.duration_ms.map(SafeUint::saturating),
            upstream_status: r.upstream_status,
            code: r.code.as_deref().map(HistoryCode::classify),
            response_bytes: SafeUint::saturating(r.response_bytes),
            decision: r.decision.map(|v| self.decision(v)),
            reports: r.reports.into_iter().map(|v| self.report(v)).collect(),
        })
    }

    /// One summary scoped to its verified run; never contains headers or body bytes.
    pub fn row(&self, key: &view::RunKey, r: view::RequestSummary) -> R<Row> {
        let run = self.run(key).ok_or(Fault::DesktopChanged {})?;
        if r.request_id != format!("{}-{}", run.instance_id, r.request_seq)
            || self.secrets.find(r.request_id.as_bytes()).is_some()
        {
            return Err(Fault::DesktopChanged {});
        }
        let reference = Ref {
            run,
            request_id: r.request_id,
        };
        Ok(Row {
            r#ref: reference,
            seq: SafeUint::saturating(r.request_seq),
            revision: SafeUint::saturating(r.revision),
            started_at: time(r.started_at)?,
            ended_at: opt_time(r.ended_at)?,
            method: self.text(&r.method),
            path: self.text(&redact::url(&r.path)),
            model: r.model.as_deref().map(|s| self.text(s)),
            user_agent: r.user_agent.as_deref().map(|s| self.text(s)),
            stream: r.stream,
            delivery: Delivery::classify(&r.delivery),
            upgraded: r.upgraded,
            outcome: outcome(&r.outcome),
            code: r.code.as_deref().map(HistoryCode::classify),
            client_status: r.client_status,
            phase: Phase::classify(&r.phase),
            next_attempt_at: opt_time(r.next_attempt_at)?,
            attempts: r.attempts,
            tries_omitted: r.tries_omitted,
            tries: r
                .tries
                .into_iter()
                .map(|v| self.attempt(v))
                .collect::<R<_>>()?,
            keepalive: r.keepalive,
            heartbeats: SafeUint::saturating(r.heartbeats),
            request_bytes: SafeUint::saturating(r.request_bytes),
            response_bytes: SafeUint::saturating(r.response_bytes),
            reports: r.reports.into_iter().map(|v| self.report(v)).collect(),
            truncated: r.truncated,
        })
    }

    /// One header in source order; classify the original name before scrubbing it.
    fn header(&self, r: view::Header) -> Header {
        let value = match r.value.as_str() {
            Some(s) if !redact::is_secret_header(&r.name) => HeaderValue::Text(self.text(s)),
            _ => HeaderValue::Secret { secret: true },
        };
        Header {
            name: self.name(&r.name),
            value,
        }
    }

    /// Metadata available without reading any body.
    fn body_info(&self, r: view::BodyInfo) -> BodyInfo {
        BodyInfo {
            bytes: SafeUint::saturating(r.bytes),
            state: Retention::classify(&r.state),
            cause: r.cause.as_deref().map(Loss::classify),
            encoding: r.encoding.as_deref().map(|s| self.text(s)),
        }
    }

    /// Client headers and retention metadata, never a reconstructed raw request.
    fn message(&self, r: view::MessageDetail) -> Message {
        Message {
            headers: r.headers.into_iter().map(|v| self.header(v)).collect(),
            body: r.body.map(|v| self.body_info(v)),
        }
    }

    /// The request actually sent upstream on this attempt.
    fn sent(&self, r: view::SentDetail) -> Sent {
        Sent {
            method: self.text(&r.method),
            url: self.text(&redact::url(&r.url)),
            headers: r.headers.into_iter().map(|v| self.header(v)).collect(),
            body: r.body.map(|v| self.body_info(v)),
        }
    }

    /// The upstream response metadata, if one was received.
    fn response(&self, r: view::ResponseDetail) -> Response {
        Response {
            upstream_status: r.upstream_status,
            headers: r.headers.into_iter().map(|v| self.header(v)).collect(),
            body: r.body.map(|v| self.body_info(v)),
        }
    }

    /// Source-scrubbed attempt diagnostic with a classified code and no details object.
    fn attempt_error(&self, r: view::AttemptError) -> AttemptError {
        AttemptError {
            code: HistoryCode::classify(&r.code),
            message: self.text(&r.message),
        }
    }

    /// Recorded metadata for one identified attempt.
    fn try_detail(&self, r: view::AttemptDetail) -> TryDetail {
        TryDetail {
            attempt: r.attempt,
            sent: r.sent.map(|v| self.sent(v)),
            response: r.response.map(|v| self.response(v)),
            error: r.error.map(|v| self.attempt_error(v)),
        }
    }

    /// Metadata read on demand for one selected request; no body content.
    pub fn detail(&self, key: &view::RunKey, r: view::RequestDetail) -> R<Detail> {
        Ok(Detail {
            record: self.row(key, r.record)?,
            request: self.message(r.request),
            tries: r.tries.into_iter().map(|v| self.try_detail(v)).collect(),
        })
    }
}

/// Exactly one source-redacted chunk. Base64 is a transport encoding, not redaction.
#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Chunk {
    /// Retained size at read time.
    pub total_bytes: SafeUint,
    /// Echo of the requested byte offset, not a character position.
    pub offset: SafeUint,
    /// Padded standard base64 of the requested bytes only.
    pub data_b64: String,
    /// More retained bytes are available now.
    pub more: bool,
    /// The peer has finished writing the logical body.
    pub complete: bool,
}
impl Chunk {
    /// Check the peer's offset, byte count and encoding before forwarding its string.
    /// Do not substitute secrets in base64 text: the peer scrubbed bytes before storage.
    pub fn project(r: view::BodyChunk, offset: SafeUint, limit: u64) -> R<Self> {
        let invalid = || Fault::DesktopHistory {};
        let remaining = r
            .total_bytes
            .checked_sub(offset.get())
            .ok_or_else(invalid)?;
        let bytes = limit.min(remaining);
        let text = r.data_b64.as_bytes();
        if !(1..=crate::wire::MAX_BODY_READ).contains(&limit)
            || r.offset != offset.get()
            || r.more != (bytes < remaining)
            || text.len() as u64 != bytes.div_ceil(3) * 4
        {
            return Err(invalid());
        }
        let padding = ((3 - bytes % 3) % 3) as usize;
        let (encoded, pad) = text.split_at(text.len() - padding);
        if !encoded
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/'))
            || !pad.iter().all(|b| *b == b'=')
        {
            return Err(invalid());
        }
        Ok(Self {
            total_bytes: SafeUint::saturating(r.total_bytes),
            offset,
            data_b64: r.data_b64,
            more: r.more,
            complete: r.complete,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    const ID: &str = "0123456789abcdef";
    fn key() -> view::RunKey {
        view::RunKey {
            port: 80,
            instance_id: ID.into(),
        }
    }
    fn summary() -> Value {
        json!({"request_id":format!("{ID}-1"),"request_seq":1,"started_at":1000,"ended_at":16000,"method":"GET","path":"/a?key=do-not-copy","stream":false,"delivery":"retry","upgraded":false,"outcome":"ok","phase":"done","attempts":2,"tries_omitted":0,"tries":[],"keepalive":false,"heartbeats":0,"request_bytes":0,"response_bytes":0})
    }
    #[test]
    fn details_project_values_before_json_escaping() {
        let secret = r#"quoted"\credential"#;
        let secrets = Secrets::new([secret]);
        let projector = Projection::new(&secrets);
        let mut r = summary();
        r["model"] = json!(secret);
        r["code"] = json!("private-peer-code");
        r["reports"] = json!([{ "format":secret,"stage":"private-stage","summary":secret,"actions":[{"type":"custom","label":secret,"private":"not public"},{"type":"header_edit","name":"example","op":"remove","value":"not public"}]}]);
        let raw = json!({"record":r,"request":{"headers":[{"name":"authorization","value":"short"},{"name":"x","value":{"secret":true,"hint":"not public"}},{"name":"x","value":secret}],"body":null},"tries":[]});
        let d = projector
            .detail(&key(), serde_json::from_value(raw).unwrap())
            .unwrap();
        let v = serde_json::to_value(d).unwrap();
        for private in [
            "short",
            "do-not-copy",
            "private-peer-code",
            "private-stage",
            "not public",
            "credential",
        ] {
            assert!(!v.to_string().contains(private), "{private}");
        }
        assert_eq!(v["request"]["headers"][0]["value"], json!({"secret":true}));
        assert_eq!(v["record"]["ref"]["requestId"], format!("{ID}-1"));
        assert_eq!(v["record"]["reports"][0]["actions"][0]["type"], "other");
        assert_eq!(v["record"]["code"], "unknown");
        assert!(Projection::new(&Secrets::new([ID])).run(&key()).is_none());
    }
    #[test]
    fn rescue_condition_needs_a_retry_and_a_terminal_ok() {
        let p = Secrets::default();
        let p = Projection::new(&p);
        let mut r = p
            .row(&key(), serde_json::from_value(summary()).unwrap())
            .unwrap();
        assert!(r.rescued_at().is_some());
        r.attempts = 1;
        assert!(r.rescued_at().is_none());
        r.ended_at = Some(SafeUint::saturating(1500));
        r.attempts = 3;
        assert!(r.rescued_at().is_none());
        r.attempts = 4;
        assert!(r.rescued_at().is_some());
        r.outcome = Outcome::UpstreamError;
        assert!(r.rescued_at().is_none());
        r.outcome = Outcome::Ok;
        r.ended_at = None;
        assert!(r.rescued_at().is_none());
    }
    #[test]
    fn chunk_rejects_extra_bytes_bad_padding_and_wrong_positions() {
        let chunk = |s: &str, offset: u64| view::BodyChunk {
            total_bytes: 3,
            offset,
            data_b64: s.into(),
            more: true,
            complete: false,
        };
        assert!(Chunk::project(chunk("YQ==", 0), SafeUint::default(), 1).is_ok());
        for (s, off) in [
            ("YWI=", 0),
            ("YQ==", 1),
            ("%Q==", 0),
            ("Y===", 0),
            ("YQ==YQ==", 0),
        ] {
            assert!(Chunk::project(chunk(s, off), SafeUint::default(), 1).is_err());
        }
    }
}
