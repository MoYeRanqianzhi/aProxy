//! HTTP values are complete on input. In a reply, absence means inheritance,
//! including for bodies; an explicitly empty string or map clears that field.

use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Header names are lowercase. The wire protocol keeps only the first value.
pub type Headers = BTreeMap<String, String>;

/// Program responses may omit the header table or explicitly send null for it.
fn response_headers<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Headers, D::Error> {
    Ok(Option::<Headers>::deserialize(d)?.unwrap_or_default())
}

/// Define the two spellings of the same bytes without ever emitting both keys.
macro_rules! bytes_field {
    ($name:ident, $text:literal, $b64:literal, $doc:literal) => {
        #[doc = $doc]
        #[derive(Debug, Clone, Default, PartialEq, Eq)]
        #[non_exhaustive]
        pub enum $name {
            /// No bytes offered: keep the input unchanged when applying a patch.
            #[default]
            Absent,
            /// UTF-8 is carried literally, including the empty string.
            Text(String),
            /// Binary input is decoded once at the wire boundary.
            Binary(Vec<u8>),
        }
        impl $name {
            /// Choose the lossless wire representation without copying binary data.
            pub fn from_bytes(bytes: Vec<u8>) -> Self {
                match String::from_utf8(bytes) {
                    Ok(s) => Self::Text(s),
                    Err(e) => Self::Binary(e.into_bytes()),
                }
            }
            /// Borrow the decoded payload, preserving the distinction between absent and empty.
            pub fn as_bytes(&self) -> Option<&[u8]> {
                match self {
                    Self::Absent => None,
                    Self::Text(s) => Some(s.as_bytes()),
                    Self::Binary(b) => Some(b),
                }
            }
        }
        impl Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                use serde::ser::SerializeMap;
                let mut m = serializer.serialize_map(None)?;
                match self {
                    Self::Absent => (),
                    Self::Text(s) => m.serialize_entry($text, s)?,
                    Self::Binary(b) => m.serialize_entry($b64, &STANDARD.encode(b))?,
                };
                m.end()
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                #[derive(Deserialize)]
                struct Pair {
                    #[serde(rename=$text)]
                    text: Option<String>,
                    #[serde(rename=$b64)]
                    binary: Option<String>,
                }
                let p = Pair::deserialize(d)?;
                match (p.text, p.binary) {
                    (None, None) => Ok(Self::Absent),
                    (Some(s), None) => Ok(Self::Text(s)),
                    (None, Some(s)) => STANDARD
                        .decode(s)
                        .map(Self::Binary)
                        .map_err(serde::de::Error::custom),
                    _ => Err(serde::de::Error::custom(concat!(
                        $text,
                        " and ",
                        $b64,
                        " are mutually exclusive"
                    ))),
                }
            }
        }
    };
}
bytes_field!(
    Body,
    "body",
    "body_b64",
    "The entity body of an HTTP request, response or patch."
);
impl Body {
    /// Move an owned HTTP payload into its recipient without another full-size allocation.
    /// Omission and an explicitly empty payload remain distinct.
    pub fn into_bytes(self) -> Option<Vec<u8>> {
        match self {
            Self::Absent => None,
            Self::Text(s) => Some(s.into_bytes()),
            Self::Binary(b) => Some(b),
        }
    }
}
bytes_field!(
    Data,
    "data",
    "data_b64",
    "One stream fragment or heartbeat; boundaries are not HTTP body boundaries."
);

/// A stream input always contains bytes, even when the fragment is empty. Replies use
/// [`Data`] instead, since omission there means that the program supplied no output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(try_from = "Data")]
pub struct Fragment(Data);
impl Fragment {
    /// Select a lossless representation for bytes received from the previous layer.
    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        Self(Data::from_bytes(bytes))
    }
    /// Borrow the bytes without exposing the optional representation used in replies.
    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes().expect("fragment contains bytes")
    }
}
impl TryFrom<Data> for Fragment {
    type Error = &'static str;
    fn try_from(data: Data) -> Result<Self, Self::Error> {
        match data {
            Data::Absent => Err("expected data or data_b64"),
            present => Ok(Self(present)),
        }
    }
}

/// Only the method and upstream URL accompany a response or stream open.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct RequestMeta {
    /// HTTP verb at the end of request processing.
    pub method: String,
    /// Final upstream URL, including its query.
    pub url: String,
}
impl RequestMeta {
    /// Construct a metadata view that cannot leak request headers or body bytes.
    pub fn new(method: String, url: String) -> Self {
        Self { method, url }
    }
}

/// A deciding program can negotiate a request body but never receives the header table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct DecideRequest {
    /// Method and URL identify the request behind this attempt.
    #[serde(flatten)]
    pub metadata: RequestMeta,
    /// Bytes only when this program negotiated a full request body at decide.
    #[serde(flatten)]
    pub body: Body,
}
impl DecideRequest {
    /// Supply the negotiated body, or [`Body::Absent`] for a metadata-only input.
    pub fn new(metadata: RequestMeta, body: Body) -> Self {
        Self { metadata, body }
    }
}

/// A request as observed at this position in the chain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Request {
    /// HTTP verb.
    pub method: String,
    /// Absolute upstream URL.
    pub url: String,
    /// Complete header table, including an empty table when no headers remain.
    #[serde(default, deserialize_with = "response_headers")]
    pub headers: Headers,
    /// Omitted when the program did not negotiate a body.
    #[serde(flatten)]
    pub body: Body,
}
impl Request {
    /// Construct a full HTTP object with an empty header table and no body.
    pub fn new(method: String, url: String) -> Self {
        Self {
            method,
            url,
            headers: Headers::new(),
            body: Body::Absent,
        }
    }
    /// Apply one layer, replacing rather than merging the supplied headers.
    pub fn apply(&mut self, p: RequestPatch) {
        if let Some(v) = p.method {
            self.method = v;
        }
        if let Some(v) = p.url {
            self.url = v;
        }
        if let Some(headers) = p.headers {
            self.headers = headers;
        }
        if p.body != Body::Absent {
            self.body = p.body;
        }
    }
}

/// A request patch; every omitted member is inherited.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[non_exhaustive]
pub struct RequestPatch {
    /// HTTP verb.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    /// Absolute upstream URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Complete header table; omitted in metadata-only views.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<Headers>,
    /// Omitted when the program did not negotiate a body.
    #[serde(flatten)]
    pub body: Body,
}
impl RequestPatch {
    /// An empty patch leaves all input fields untouched.
    pub fn new() -> Self {
        Self::default()
    }
}

/// The upstream status and headers from a previous attempt, never its body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ResponseHead {
    /// HTTP status from the upstream, even if reading the body subsequently failed.
    pub status: u16,
    /// The same complete header table supplied to deciding programs.
    #[serde(deserialize_with = "response_headers")]
    pub headers: Headers,
}
impl ResponseHead {
    /// Construct a header-only observation without retaining any entity bytes.
    pub fn new(status: u16, headers: Headers) -> Self {
        Self { status, headers }
    }
}

/// A response as observed at this position in the chain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Response {
    /// HTTP status code.
    pub status: u16,
    /// Complete header table, including an empty table when no headers remain.
    #[serde(default, deserialize_with = "response_headers")]
    pub headers: Headers,
    /// Omitted when the program did not negotiate a body.
    #[serde(flatten)]
    pub body: Body,
}
impl Response {
    /// Construct a full HTTP object with an empty header table and no body.
    pub fn new(status: u16) -> Self {
        Self {
            status,
            headers: Headers::new(),
            body: Body::Absent,
        }
    }
    /// Apply one layer, replacing rather than merging the supplied headers.
    pub fn apply(&mut self, p: ResponsePatch) {
        if let Some(v) = p.status {
            self.status = v;
        }
        if let Some(headers) = p.headers {
            self.headers = headers;
        }
        if p.body != Body::Absent {
            self.body = p.body;
        }
    }
}

/// A response patch; every omitted member is inherited.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[non_exhaustive]
pub struct ResponsePatch {
    /// HTTP status code.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    /// Complete header table; omitted in metadata-only views.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<Headers>,
    /// Omitted when the program did not negotiate a body.
    #[serde(flatten)]
    pub body: Body,
}
impl ResponsePatch {
    /// An empty patch leaves all input fields untouched.
    pub fn new() -> Self {
        Self::default()
    }
}
