//! Session negotiation, stage inputs and replies. Optional fields are never emitted merely to fill a slot.
use crate::{
    Data, DecideRequest, Fragment, ReportField, Request, RequestMeta, RequestPatch, Response,
    ResponsePatch,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Format-specific overrides remain an open object; aproxy validates each leaf independently.
pub type Overrides = serde_json::Map<String, serde_json::Value>;
/// Preserve the distinction between a missing digest and an explicitly incomplete upload.
fn present<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(d).map(Some)
}

/// Serde's default enum reader also accepts externally tagged objects. Protocol tokens
/// are strings, so reject the alternative spelling before matching their vocabulary.
macro_rules! string_token {
    ($ty:ident, $($word:literal => $variant:ident),+ $(,)?) => {
        impl TryFrom<String> for $ty {
            type Error = &'static str;
            fn try_from(word: String) -> Result<Self, Self::Error> {
                match word.as_str() { $($word => Ok(Self::$variant),)+ _ => Err(concat!("unknown ",stringify!($ty)," token")) }
            }
        }
    };
}
string_token!(BodyNeed, "none"=>None, "digest"=>Digest, "full"=>Full);
string_token!(Process, "spawn"=>Spawn, "persistent"=>Persistent);
string_token!(Delivery, "retry"=>Retry, "forward"=>Forward);
string_token!(Decision, "retry"=>Retry, "stop"=>Stop);

/// How much of the original request a stage needs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case", try_from = "String")]
pub enum BodyNeed {
    /// Do not send a body or digest.
    None,
    /// Send only the SHA-256 of the raw client bytes.
    Digest,
    /// Include the request body in the HTTP object.
    Full,
}

/// Largest wait a program can propose for one retry, in milliseconds (24 hours).
pub const MAX_DELAY_MS: u64 = 86_400_000;

/// A program's retry delay, validated before a decide vote can take effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u64")]
#[non_exhaustive]
pub struct RetryDelay(u64);
impl RetryDelay {
    /// Reject an out-of-range wait rather than silently changing the program's decision.
    pub fn new(ms: u64) -> Result<Self, &'static str> {
        ms.try_into()
    }
    /// Return the validated number of milliseconds.
    pub fn millis(self) -> u64 {
        self.0
    }
}
impl TryFrom<u64> for RetryDelay {
    type Error = &'static str;
    fn try_from(ms: u64) -> Result<Self, Self::Error> {
        if ms <= MAX_DELAY_MS {
            Ok(Self(ms))
        } else {
            Err("delay_ms exceeds 86400000")
        }
    }
}

/// Worker lifetime selected by the instance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case", try_from = "String")]
pub enum Process {
    /// One stage message per process after hello.
    Spawn,
    /// Process ordered messages until stdin closes.
    Persistent,
}

/// Active delivery mode of this request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case", try_from = "String")]
pub enum Delivery {
    /// Buffer and retry before delivery.
    Retry,
    /// Deliver incrementally without pre-committing to retries.
    Forward,
}

/// Disposition of a finished attempt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case", try_from = "String")]
pub enum Decision {
    /// Make another attempt.
    Retry,
    /// Return the upstream or supplied response.
    Stop,
}

/// The immutable session offer; stage names and features are open vocabularies.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Hello {
    /// Versions that the instance can speak after hello.
    pub protocols: Vec<u32>,
    /// Instance version for diagnostics.
    pub aproxy_version: String,
    /// Configured chain entry, not an executable path.
    pub name: String,
    /// Expected lifetime of this worker.
    pub process: Process,
    /// Stages this entry is configured to receive.
    pub stages: Vec<String>,
    /// Slot assigned to this process.
    pub worker_id: u32,
    /// Optional behavior supported by the instance.
    pub features: Vec<String>,
    /// Configuration keys accepted from this program.
    pub overrides: Vec<String>,
}
impl Hello {
    /// Construct the wire object from its required context.
    pub fn new(
        aproxy_version: String,
        name: String,
        process: Process,
        stages: Vec<String>,
        worker_id: u32,
    ) -> Self {
        Self {
            protocols: vec![1],
            aproxy_version,
            name,
            process,
            stages,
            worker_id,
            features: Vec::new(),
            overrides: Vec::new(),
        }
    }
}

/// Worker selection and capabilities, returned before any stage can be sent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(try_from = "serde_json::Map<String, serde_json::Value>")]
pub struct HelloReply {
    /// Version of the hello framing itself, always one.
    pub v: u32,
    /// Echo of the opening message sequence.
    pub seq: u64,
    /// Selected protocol for subsequent messages.
    pub protocol: u32,
    /// All stages implemented by the worker.
    pub stages: Vec<String>,
    /// Amount of client body required by each stage.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub request_body: BTreeMap<String, BodyNeed>,
    /// Feature names understood by the program.
    pub features: Vec<String>,
    /// Program identity for diagnostics.
    pub name: String,
    /// Program version for the instance snapshot.
    pub version: String,
}
impl TryFrom<serde_json::Map<String, serde_json::Value>> for HelloReply {
    type Error = serde_json::Error;
    fn try_from(mut obj: serde_json::Map<String, serde_json::Value>) -> Result<Self, Self::Error> {
        use serde::de::Error;
        // Consume only known fields. Positional arrays and scalars are not hello frames.
        fn field<T: serde::de::DeserializeOwned>(
            obj: &mut serde_json::Map<String, serde_json::Value>,
            key: &'static str,
        ) -> Result<T, serde_json::Error> {
            serde_json::from_value(
                obj.remove(key)
                    .ok_or_else(|| serde_json::Error::missing_field(key))?,
            )
        }
        let request_body = match obj.remove("request_body") {
            None | Some(serde_json::Value::Null) => BTreeMap::new(),
            Some(v) => {
                let mut needs: serde_json::Map<String, serde_json::Value> =
                    serde_json::from_value(v)?;
                let mut known = BTreeMap::new();
                for stage in ["request", "retry", "decide", "notify"] {
                    if let Some(value) = needs.remove(stage) {
                        let need = serde_json::from_value(value)?;
                        if stage == "notify" && need == BodyNeed::Full {
                            return Err(serde_json::Error::custom(
                                "notify cannot request a full body",
                            ));
                        }
                        known.insert(stage.to_owned(), need);
                    }
                }
                known
            }
        };
        Ok(Self {
            v: field(&mut obj, "v")?,
            seq: field(&mut obj, "seq")?,
            protocol: field(&mut obj, "protocol")?,
            stages: field(&mut obj, "stages")?,
            request_body,
            features: field(&mut obj, "features")?,
            name: field(&mut obj, "name")?,
            version: field(&mut obj, "version")?,
        })
    }
}
impl HelloReply {
    /// Construct the wire object from its required context.
    pub fn new(stages: Vec<String>, name: String, version: String) -> Self {
        Self {
            v: 1,
            seq: 0,
            protocol: 1,
            stages,
            request_body: BTreeMap::new(),
            features: Vec::new(),
            name,
            version,
        }
    }
}

/// Request identity and private program state shared across stages.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Context {
    /// Request identity shared by the whole chain.
    pub request_id: String,
    /// Opaque state from this program for this request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// Digest if negotiated; explicit null means the body was incomplete.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub digest: Option<Option<String>>,
}
impl Context {
    /// Construct the wire object from its required context.
    pub fn new(request_id: String) -> Self {
        Self {
            request_id,
            state: None,
            digest: None,
        }
    }
}

/// Client-side metadata, unaffected by upstream rewrites.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Client {
    /// Original request path including the query.
    pub path: String,
    /// Whether the original client requested streaming under the configured trigger.
    /// Omitted when unknown at dispatch, never a substitute for `false`. A duplex
    /// response can start while the upload is still in progress. Request layers that
    /// need the exact value before routing should negotiate `digest` or `full`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
}
impl Client {
    /// Construct the wire object from its required context.
    pub fn new(path: String, stream: Option<bool>) -> Self {
        Self { path, stream }
    }
}

/// Machine-readable failure for retry, decide and abort.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Failure {
    /// Stable failure code.
    pub code: String,
    /// Code-specific fields such as upstream status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Overrides>,
}
impl Failure {
    /// Construct the wire object from its required context.
    pub fn new(code: String) -> Self {
        Self {
            code,
            details: None,
        }
    }
}

/// Information about the upcoming retry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Retry {
    /// Number of the attempt about to run.
    pub attempt: u32,
    /// Why the previous attempt did not succeed.
    pub last_failure: Failure,
    /// Elapsed request time in milliseconds.
    pub elapsed_ms: u64,
}
impl Retry {
    /// Construct the wire object from its required context.
    pub fn new(attempt: u32, last_failure: Failure, elapsed_ms: u64) -> Self {
        Self {
            attempt,
            last_failure,
            elapsed_ms,
        }
    }
}

/// Progress observed at a keepalive beat.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Heartbeat {
    /// One-based beat number.
    pub beat: u64,
    /// Time since waiting began.
    pub elapsed_ms: u64,
    /// Current upstream attempt.
    pub attempt: u32,
    /// Most recently observed failure, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_failure: Option<Failure>,
}
impl Heartbeat {
    /// Construct the wire object from its required context.
    pub fn new(beat: u64, elapsed_ms: u64, attempt: u32) -> Self {
        Self {
            beat,
            elapsed_ms,
            attempt,
            last_failure: None,
        }
    }
}

/// Input to a request layer, or the common part of a retry input.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct RequestMessage {
    /// Identity and private state for this entry.
    #[serde(flatten)]
    pub context: Context,
    /// Mode selected by preceding layers or configuration.
    pub delivery: Delivery,
    /// HTTP request after the previous layer.
    pub request: Request,
    /// Original client metadata.
    pub client: Client,
}
impl RequestMessage {
    /// Construct the wire object from its required context.
    pub fn new(context: Context, delivery: Delivery, request: Request, client: Client) -> Self {
        Self {
            context,
            delivery,
            request,
            client,
        }
    }
}

/// Request-direction input when rerunning the retry segment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct RetryMessage {
    /// Current request, delivery mode and client context.
    #[serde(flatten)]
    pub input: RequestMessage,
    /// Attempt and previous failure.
    pub retry: Retry,
}
impl RetryMessage {
    /// Construct the wire object from its required context.
    pub fn new(input: RequestMessage, retry: Retry) -> Self {
        Self { input, retry }
    }
}

/// The decision that would be used without a format vote.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Fallback {
    /// Retry or stop under current configuration.
    pub decision: Decision,
    /// Origin of the fallback decision.
    pub by: String,
    /// Wait before retry, absent when stopping.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delay_ms: Option<u64>,
}
impl Fallback {
    /// Construct the wire object from its required context.
    pub fn new(decision: Decision, by: String) -> Self {
        Self {
            decision,
            by,
            delay_ms: None,
        }
    }
}

/// Bounded upstream response supplied to a deciding program.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ObservedResponse {
    /// Status, headers and decoded preview.
    #[serde(flatten)]
    pub response: Response,
    /// Omitted byte count, null when the preview is complete.
    pub truncated: Option<u64>,
}
impl ObservedResponse {
    /// Construct the wire object from its required context.
    pub fn new(response: Response) -> Self {
        Self {
            response,
            truncated: None,
        }
    }
}

/// A chance to vote after seeing an attempt result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct DecideMessage {
    /// Request identity and state.
    #[serde(flatten)]
    pub context: Context,
    /// Original client metadata; independent of any upstream rewrites.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client: Option<Client>,
    /// Completed attempt number.
    pub attempt: u32,
    /// Elapsed time since request start.
    pub elapsed_ms: u64,
    /// Mode before the decision.
    pub delivery: Delivery,
    /// Success or failure of the attempt.
    pub outcome: String,
    /// Why the attempt failed; absent on success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<Failure>,
    /// Metadata and any negotiated request body.
    pub request: DecideRequest,
    /// Available only if the upstream sent a response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<ObservedResponse>,
    /// Local retry decision before any program voted.
    pub fallback: Fallback,
}
impl DecideMessage {
    /// Construct the wire object from its required context.
    pub fn new(
        context: Context,
        attempt: u32,
        elapsed_ms: u64,
        delivery: Delivery,
        outcome: String,
        request: DecideRequest,
        fallback: Fallback,
    ) -> Self {
        Self {
            context,
            client: None,
            attempt,
            elapsed_ms,
            delivery,
            outcome,
            failure: None,
            request,
            response: None,
            fallback,
        }
    }
}

/// A successful buffered response, ready for reverse chain traversal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ResponseMessage {
    /// Request identity and private state.
    #[serde(flatten)]
    pub context: Context,
    /// Original client metadata; independent of any upstream rewrites.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client: Option<Client>,
    /// Final upstream request metadata.
    pub request: RequestMeta,
    /// Response to be delivered after patching.
    pub response: Response,
}
impl ResponseMessage {
    /// Construct the wire object from its required context.
    pub fn new(context: Context, request: RequestMeta, response: Response) -> Self {
        Self {
            context,
            client: None,
            request,
            response,
        }
    }
}

/// An individual heartbeat moving from upstream-side to client-side layers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct HeartbeatMessage {
    /// State is read-only on heartbeat.
    #[serde(flatten)]
    pub context: Context,
    /// Progress information for this beat.
    pub heartbeat: Heartbeat,
    /// Original client protocol hints.
    pub client: Client,
    /// Previous layer output or configured static bytes.
    #[serde(flatten)]
    pub data: Fragment,
}
impl HeartbeatMessage {
    /// Construct the wire object from its required context.
    pub fn new(context: Context, heartbeat: Heartbeat, client: Client, data: Fragment) -> Self {
        Self {
            context,
            heartbeat,
            client,
            data,
        }
    }
}

/// Start processing a single response on a dedicated worker.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct StreamOpen {
    /// Request identity and private state.
    #[serde(flatten)]
    pub context: Context,
    /// Original client metadata; independent of any upstream rewrites.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client: Option<Client>,
    /// Final upstream request metadata.
    pub request: RequestMeta,
    /// Status and headers before stream conversion.
    pub response: Response,
}
impl StreamOpen {
    /// Construct the wire object from its required context.
    pub fn new(context: Context, request: RequestMeta, response: Response) -> Self {
        Self {
            context,
            client: None,
            request,
            response,
        }
    }
}

/// An event on one worker-owned stream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case", tag = "event")]
pub enum StreamEvent {
    /// Begin conversion with request and response headers.
    Open(StreamOpen),
    /// Convert one fragment of the decoded byte stream.
    Data(Fragment),
    /// Flush end-of-stream output.
    Close,
    /// Produce a protocol-correct error ending.
    Abort {
        /// The upstream attempt's failure code, an inner layer's `format_error` or
        /// `format_fail`, or `client_gone` when the client disconnected. The latter
        /// is an outcome, not an error code. Treat unknown codes as a plain abort.
        reason: Failure,
    },
}

/// One ordered event, identified even without repeating request metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct StreamMessage {
    /// Request ID and attempt joined by a slash.
    pub stream_id: String,
    /// Event and its specific payload.
    #[serde(flatten)]
    pub event: StreamEvent,
}
impl StreamMessage {
    /// Construct the wire object from its required context.
    pub fn new(stream_id: String, event: StreamEvent) -> Self {
        Self { stream_id, event }
    }
}

/// Final outcome delivered asynchronously to interested workers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Completion {
    /// Identity, final state and negotiated digest.
    #[serde(flatten)]
    pub context: Context,
    /// Original client metadata; independent of any upstream rewrites.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client: Option<Client>,
    /// Final mode of this request.
    pub delivery: Delivery,
    /// Whether this request switched from forward to retry.
    pub upgraded: bool,
    /// Total attempted deliveries.
    pub attempts: u32,
    /// Last upstream status, null when none arrived.
    pub upstream_status: Option<u16>,
    /// Null for non-streaming requests.
    pub stream_completed: Option<bool>,
    /// Total request lifetime.
    pub duration_ms: u64,
    /// Same outcome vocabulary as request records.
    pub outcome: String,
    /// Terminal code, null for success.
    pub code: Option<String>,
}
impl Completion {
    /// Construct the wire object from its required context.
    pub fn new(
        context: Context,
        delivery: Delivery,
        attempts: u32,
        duration_ms: u64,
        outcome: String,
    ) -> Self {
        Self {
            context,
            client: None,
            delivery,
            upgraded: false,
            attempts,
            upstream_status: None,
            stream_completed: None,
            duration_ms,
            outcome,
            code: None,
        }
    }
}

/// Reserved notification only for workers negotiating attempt events.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct AttemptEnd {
    /// Request identity and private state.
    #[serde(flatten)]
    pub context: Context,
    /// Completed attempt number.
    pub attempt: u32,
    /// Timestamp in Unix milliseconds.
    pub at: u64,
    /// Result of the attempt.
    pub outcome: String,
    /// Failure detail if applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<Failure>,
    /// Upstream metadata if available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<Response>,
}
impl AttemptEnd {
    /// Construct the wire object from its required context.
    pub fn new(context: Context, attempt: u32, at: u64, outcome: String) -> Self {
        Self {
            context,
            attempt,
            at,
            outcome,
            failure: None,
            response: None,
        }
    }
}

/// Notifications are separately negotiated from the request path.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case", tag = "event")]
pub enum NotifyEvent {
    /// Final request outcome.
    RequestEnd(Completion),
    /// Reserved attempt observation; requires the attempt feature.
    AttemptEnd(AttemptEnd),
}

/// Reserved admission queue observation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Queue {
    /// Requests waiting in this queue.
    pub length: u64,
    /// Position of this request.
    pub position: u64,
    /// Time spent waiting.
    pub waited_ms: u64,
}
impl Queue {
    /// Construct the wire object from its required context.
    pub fn new(length: u64, position: u64, waited_ms: u64) -> Self {
        Self {
            length,
            position,
            waited_ms,
        }
    }
}

/// Reserved admission input; it is not a protocol 1 runtime capability.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct AdmitMessage {
    /// Identity of the waiting request.
    #[serde(flatten)]
    pub context: Context,
    /// Upcoming attempt.
    pub attempt: u32,
    /// Upstream request metadata.
    pub request: RequestMeta,
    /// Current queue observation.
    pub queue: Queue,
}
impl AdmitMessage {
    /// Construct the wire object from its required context.
    pub fn new(context: Context, attempt: u32, request: RequestMeta, queue: Queue) -> Self {
        Self {
            context,
            attempt,
            request,
            queue,
        }
    }
}

/// Reserved batch allowance from an admission worker.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Grant {
    /// Number of permits.
    pub count: u64,
    /// Validity interval.
    pub ttl_ms: u64,
}
impl Grant {
    /// Construct the wire object from its required context.
    pub fn new(count: u64, ttl_ms: u64) -> Self {
        Self { count, ttl_ms }
    }
}

/// Stage discriminator on an instance-to-worker frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case", tag = "stage")]
pub enum Stage {
    /// Immutable session offer.
    Hello(Hello),
    /// First request-direction pass.
    Request(RequestMessage),
    /// Retry segment pass.
    Retry(RetryMessage),
    /// Decision after an attempt.
    Decide(DecideMessage),
    /// Buffered response patching.
    Response(ResponseMessage),
    /// Incremental response processing.
    Stream(StreamMessage),
    /// Keepalive conversion.
    Heartbeat(HeartbeatMessage),
    /// Asynchronous observation.
    Notify(NotifyEvent),
    /// Reserved admission example; never send without negotiation.
    Admit(AdmitMessage),
}

/// A single frame written to worker stdin, terminated by a newline in transport.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Message {
    /// Protocol version, always one for hello.
    pub v: u32,
    /// Sequence number within this worker session.
    pub seq: u64,
    /// Tagged stage input.
    #[serde(flatten)]
    pub stage: Stage,
}
impl Message {
    /// Construct the wire object from its required context.
    pub fn new(seq: u64, stage: Stage) -> Self {
        Self { v: 1, seq, stage }
    }
}

/// A deliberate program rejection rather than a process error.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(from = "ReceivedFail")]
#[non_exhaustive]
pub struct Fail {
    /// Whether to enter the normal retry loop.
    pub retry: bool,
    /// Short English explanation safe to show to the client.
    pub reason: String,
    /// Optional program-specific machine-readable code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// Set only on receipt of an invalid optional code; receivers warn without echoing it.
    #[serde(skip)]
    pub discarded_code: bool,
    /// Optional client-protocol response when stopping.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<Response>,
}
/// Receive the diagnostic code separately: a bad optional field cannot undo a rejection.
#[derive(Deserialize)]
struct ReceivedFail {
    /// Whether the operation may be retried.
    retry: bool,
    /// Program's explanation.
    reason: String,
    /// Raw optional field, including a value of the wrong JSON type.
    code: Option<serde_json::Value>,
    /// Optional terminal bytes and metadata.
    response: Option<Response>,
}
impl From<ReceivedFail> for Fail {
    fn from(input: ReceivedFail) -> Self {
        let supplied = input.code.is_some();
        let code = match input.code {
            Some(serde_json::Value::String(s))
                if !s.is_empty()
                    && s.len() <= 64
                    && s.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-')) =>
            {
                Some(s)
            }
            _ => None,
        };
        Self {
            retry: input.retry,
            reason: input.reason,
            discarded_code: supplied && code.is_none(),
            code,
            response: input.response,
        }
    }
}
impl Fail {
    /// Construct the wire object from its required context.
    pub fn new(retry: bool, reason: String) -> Self {
        Self {
            retry,
            reason,
            discarded_code: false,
            code: None,
            response: None,
        }
    }
}

/// Reply to one stage; an omitted patch, vote or state does not change the input.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Reply {
    /// Negotiated protocol version.
    pub v: u32,
    /// Sequence number copied from the message.
    pub seq: u64,
    /// Replacement state; empty clears it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// Public per-stage summary and actions.
    #[serde(default, skip_serializing_if = "ReportField::is_omitted")]
    pub report: ReportField,
    /// Edits to the outgoing request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<RequestPatch>,
    /// Response edits or stop response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<ResponsePatch>,
    /// Explicit format decision to reject a request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fail: Option<Fail>,
    /// Proposed configuration changes for this request only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overrides: Option<Overrides>,
    /// First recognized request-stage vote wins; unknown values are ignored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery: Option<serde_json::Value>,
    /// Optional decide vote.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<Decision>,
    /// Delay proposed for a retry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delay_ms: Option<RetryDelay>,
    /// Vote on rerunning the retry segment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reroute: Option<bool>,
    /// Client-facing ending if this stream layer dies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub abort_tail: Option<String>,
    /// Heartbeat or stream output bytes.
    #[serde(flatten)]
    pub data: Data,
    /// Reserved admission vote.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admit: Option<bool>,
    /// Reserved wait before another admission check.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
    /// Reserved admission explanation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Reserved admission permits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant: Option<Grant>,
}
impl Reply {
    /// Construct the wire object from its required context.
    pub fn new(seq: u64) -> Self {
        Self {
            v: 1,
            seq,
            state: None,
            report: ReportField::Absent,
            request: None,
            response: None,
            fail: None,
            overrides: None,
            delivery: None,
            decision: None,
            delay_ms: None,
            reroute: None,
            abort_tail: None,
            data: Data::Absent,
            admit: None,
            retry_after_ms: None,
            reason: None,
            grant: None,
        }
    }
}
