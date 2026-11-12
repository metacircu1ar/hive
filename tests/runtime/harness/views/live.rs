//! Human observation pages retain omitted item fields and nullable cursor receipts.
use super::{AgentView, Map, Value, decode};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct LiveView {
    #[serde(deserialize_with = "decode::nullable_object")]
    pub cursor: Option<Value>,
    #[serde(deserialize_with = "decode::objects")]
    pub items: Vec<LiveItem>,
    pub resync: bool,
    pub durable_changed: bool,
    #[serde(flatten)]
    other: Map<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct LiveItem {
    pub seq: u64,
    pub at_ms: u64,
    pub kind: String,
    #[serde(
        default,
        deserialize_with = "decode::present",
        skip_serializing_if = "Option::is_none"
    )]
    pub turn_id: Option<String>,
    #[serde(
        default,
        deserialize_with = "decode::present",
        skip_serializing_if = "Option::is_none"
    )]
    pub item_id: Option<String>,
    #[serde(
        default,
        deserialize_with = "decode::present",
        skip_serializing_if = "Option::is_none"
    )]
    pub tool: Option<String>,
    #[serde(
        default,
        deserialize_with = "decode::present",
        skip_serializing_if = "Option::is_none"
    )]
    pub text: Option<String>,
    #[serde(
        default,
        deserialize_with = "decode::present",
        skip_serializing_if = "Option::is_none"
    )]
    pub status: Option<String>,
    #[serde(
        default,
        deserialize_with = "decode::present",
        skip_serializing_if = "Option::is_none"
    )]
    pub truncated: Option<bool>,
    #[serde(flatten)]
    other: Map<String, Value>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(bound(deserialize = "T: serde::de::DeserializeOwned"))]
pub(crate) struct AttachView<T> {
    #[serde(deserialize_with = "decode::object")]
    pub agent: AgentView,
    #[serde(deserialize_with = "decode::object")]
    pub events: T,
    pub queued: u64,
    pub mail: u64,
    pub children: u64,
    #[serde(deserialize_with = "decode::object")]
    pub live: Value,
    #[serde(flatten)]
    other: Map<String, Value>,
}
