//! Typed batch and durable-history pages share only their actual wire structure.
use super::{EventView, Map, Value, decode};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
#[serde(bound(deserialize = "T: serde::de::DeserializeOwned"))]
pub(crate) struct BatchView<T> {
    #[serde(deserialize_with = "decode::objects")]
    pub results: Vec<T>,
    #[serde(
        default,
        deserialize_with = "decode::present_object",
        skip_serializing_if = "Option::is_none"
    )]
    pub join: Option<super::joins::JoinAccepted>,
    #[serde(
        default,
        deserialize_with = "decode::present",
        skip_serializing_if = "Option::is_none"
    )]
    pub error: Option<Value>,
    #[serde(flatten)]
    other: Map<String, Value>,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct HistoryView {
    #[serde(deserialize_with = "decode::objects")]
    pub events: Vec<EventView>,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub older: Option<u64>,
    pub pruned_through: u64,
    pub history_expired: bool,
    #[serde(flatten)]
    other: Map<String, Value>,
}
