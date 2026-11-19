//! Non-secret summaries of what a format program decided or changed.
use serde::{Deserialize, Serialize};

/// Human-readable summary and machine-readable actions for one stage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Report {
    /// Short English summary, with no credential material.
    pub summary: String,
    /// Ordered actions performed by the layer.
    pub actions: Vec<Action>,
}
impl Report {
    /// Construct the wire object from its required context.
    pub fn new(summary: String) -> Self {
        Self {
            summary,
            actions: Vec::new(),
        }
    }
}

/// An open action vocabulary: unknown types can still be rendered using their label.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Action {
    /// Machine-readable action type.
    #[serde(rename = "type")]
    pub kind: String,
    /// Display text for a custom or unrecognized action.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Source protocol of a conversion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// Destination protocol of a conversion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    /// Public alias of a selected credential, never its value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_id: Option<String>,
    /// Selected routing channel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    /// Original model before routing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_from: Option<String>,
    /// Model sent to the chosen channel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_to: Option<String>,
    /// Header name for a header edit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// JSON Pointer for a body edit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Edit operation: set or remove.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub op: Option<String>,
}
impl Action {
    /// Construct the wire object from its required context.
    pub fn new(kind: String) -> Self {
        Self {
            kind,
            label: None,
            from: None,
            to: None,
            key_id: None,
            channel: None,
            model_from: None,
            model_to: None,
            name: None,
            path: None,
            op: None,
        }
    }
}
