//! Non-secret summaries of what a format program decided or changed.
use serde::{Deserialize, Serialize, de::Error};
use serde_json::{Map, Value};

/// A display-only field cannot invalidate edits, votes or state in the enclosing frame.
#[derive(Debug, Default, Clone, PartialEq, Serialize)]
#[serde(untagged)]
#[non_exhaustive]
pub enum ReportField {
    /// No report supplied; an explicit null has the same meaning.
    #[default]
    Absent,
    /// An invalid report is omitted from output, but the receiver should warn.
    Invalid,
    /// A valid report, possibly with individual malformed actions removed.
    Present(Report),
}
impl ReportField {
    /// Keep invalid display data off the wire, just like an absent report.
    pub fn is_omitted(&self) -> bool {
        !matches!(self, Self::Present(_))
    }
}
impl<'de> Deserialize<'de> for ReportField {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(match Option::<Value>::deserialize(d)? {
            None => Self::Absent,
            Some(v) => serde_json::from_value(v)
                .map(Self::Present)
                .unwrap_or(Self::Invalid),
        })
    }
}

/// Human-readable summary and machine-readable actions for one stage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "Map<String, Value>")]
#[non_exhaustive]
pub struct Report {
    /// Short English summary, with no credential material.
    pub summary: String,
    /// Valid actions in their original order.
    pub actions: Vec<Action>,
    /// Rejected entries are counted for diagnostics, never echoed or written to records.
    #[serde(skip)]
    pub discarded_actions: usize,
}
impl Report {
    /// Start a summary with an empty action list.
    pub fn new(summary: String) -> Self {
        Self {
            summary,
            actions: Vec::new(),
            discarded_actions: 0,
        }
    }
}
impl TryFrom<Map<String, Value>> for Report {
    type Error = serde_json::Error;
    fn try_from(mut fields: Map<String, Value>) -> Result<Self, Self::Error> {
        let Some(Value::String(summary)) = fields.remove("summary") else {
            return Err(Self::Error::custom("expected report summary string"));
        };
        let Some(Value::Array(actions)) = fields.remove("actions") else {
            return Err(Self::Error::custom("expected report actions array"));
        };
        let mut report = Report::new(summary);
        for action in actions {
            match serde_json::from_value(action) {
                Ok(a) => report.actions.push(a),
                Err(_) => report.discarded_actions += 1,
            }
        }
        Ok(report)
    }
}

/// Set or remove a header or body field; other operation names are not understood.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", try_from = "String")]
#[non_exhaustive]
pub enum EditOp {
    /// Assign a value at the selected name or path.
    Set,
    /// Delete the selected name or path.
    Remove,
}

impl TryFrom<String> for EditOp {
    type Error = &'static str;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        match value.as_str() {
            "set" => Ok(Self::Set),
            "remove" => Ok(Self::Remove),
            _ => Err("expected set or remove"),
        }
    }
}

/// Unlike an optional reply patch, a supplied action field must be a string, even if optional.
fn optional_string<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    String::deserialize(d).map(Some)
}

/// Required fields of a standard action travel together when constructed by a program.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum StandardAction {
    /// Translate the message between two named protocols.
    ProtocolConvert {
        /// Source protocol.
        from: String,
        /// Destination protocol.
        to: String,
    },
    /// Select a credential by its public alias, never its secret value.
    KeySelect {
        /// Key alias or identifier.
        key_id: String,
    },
    /// Choose a channel, optionally rewriting the requested model.
    Route {
        /// Public channel identifier.
        channel: String,
        /// Original model before rewriting.
        #[serde(
            default,
            deserialize_with = "optional_string",
            skip_serializing_if = "Option::is_none"
        )]
        model_from: Option<String>,
        /// Model sent to the selected channel.
        #[serde(
            default,
            deserialize_with = "optional_string",
            skip_serializing_if = "Option::is_none"
        )]
        model_to: Option<String>,
    },
    /// Edit a named HTTP header.
    HeaderEdit {
        /// Header name, never the header value.
        name: String,
        /// Operation applied to the header.
        op: EditOp,
    },
    /// Edit part of the request body.
    BodyEdit {
        /// JSON Pointer identifying the field.
        path: String,
        /// Operation applied at the pointer.
        op: EditOp,
    },
}

/// Unknown action types may still be displayed; only the type and label are kept.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged, try_from = "Map<String, Value>")]
#[non_exhaustive]
pub enum Action {
    /// A standard action with its required fields and an optional label.
    Standard {
        /// Structured facts safe to display.
        #[serde(flatten)]
        details: StandardAction,
        /// Optional human-readable description.
        #[serde(skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
    /// Free-form and future actions keep no fields apart from their name and display label.
    Other {
        /// Type as supplied by the program.
        #[serde(rename = "type")]
        kind: String,
        /// Optional display text.
        #[serde(skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
}
impl Action {
    /// Attach an optional display label to a typed standard action.
    pub fn new(details: StandardAction, label: Option<String>) -> Self {
        Self::Standard { details, label }
    }
    /// Construct a free-form entry with its required label.
    pub fn custom(label: String) -> Self {
        Self::Other {
            kind: "custom".into(),
            label: Some(label),
        }
    }
}
impl TryFrom<Map<String, Value>> for Action {
    type Error = serde_json::Error;
    fn try_from(mut fields: Map<String, Value>) -> Result<Self, Self::Error> {
        let label = fields
            .remove("label")
            .map(serde_json::from_value::<String>)
            .transpose()?;
        match fields.get("type").and_then(Value::as_str) {
            Some("custom") => Ok(Self::custom(
                label.ok_or_else(|| Self::Error::missing_field("label"))?,
            )),
            Some("protocol_convert" | "key_select" | "route" | "header_edit" | "body_edit") => Ok(
                Self::new(serde_json::from_value(Value::Object(fields))?, label),
            ),
            Some(kind) => Ok(Self::Other {
                kind: kind.to_owned(),
                label,
            }),
            None => Err(Self::Error::custom("expected action type string")),
        }
    }
}
