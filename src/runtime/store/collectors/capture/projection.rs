//! Event-specific borrowed projections preserve absence and diagnostic decisions.
use crate::decode::{field, object_or_default};
use serde::{Deserialize, Deserializer, de::DeserializeOwned};
use serde_json::Value;

pub(super) fn read<'a, T: Deserialize<'a> + Default>(data: &'a Value) -> T {
    crate::decode::object(data).unwrap_or_default()
}

/// Missing optional evidence never invokes its diagnostic observer.
pub(super) fn resolve<T: DeserializeOwned>(
    raw: Option<Value>,
    observe: impl FnOnce(serde_json::Result<T>) -> Option<T>,
) -> Option<T> {
    raw.and_then(|value| observe(serde_json::from_value(value)))
}

fn present<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(d).map(Some)
}

#[derive(Default, Deserialize)]
#[serde(default)]
pub(super) struct Turn<'a> {
    #[serde(borrow, deserialize_with = "field")]
    pub turn_id: Option<&'a str>,
    #[serde(borrow, deserialize_with = "field")]
    pub status: Option<&'a str>,
}
#[derive(Default, Deserialize)]
#[serde(default)]
pub(super) struct Recovery<'a> {
    #[serde(borrow, deserialize_with = "field")]
    pub turn_id: Option<&'a str>,
    #[serde(borrow, deserialize_with = "field")]
    pub reason: Option<&'a str>,
    #[serde(borrow, deserialize_with = "field")]
    pub outcome: Option<&'a str>,
}
#[derive(Default, Deserialize)]
#[serde(default)]
pub(super) struct Failure<'a> {
    #[serde(borrow, deserialize_with = "field")]
    pub turn_id: Option<&'a str>,
    #[serde(deserialize_with = "object_or_default")]
    pub error: FailureCode,
}
#[derive(Default, Deserialize)]
#[serde(default)]
pub(super) struct FailureCode {
    #[serde(deserialize_with = "present")]
    pub code: Option<Value>,
}
#[derive(Default, Deserialize)]
#[serde(default)]
pub(super) struct Answer<'a> {
    #[serde(deserialize_with = "present")]
    pub action: Option<Value>,
    #[serde(borrow, deserialize_with = "field")]
    pub approval_id: Option<&'a str>,
    #[serde(borrow, deserialize_with = "object_or_default")]
    pub origin: Origin<'a>,
}
#[derive(Default, Deserialize)]
#[serde(default)]
pub(super) struct Origin<'a> {
    #[serde(borrow, deserialize_with = "field")]
    pub execution: Option<&'a str>,
    #[serde(borrow, deserialize_with = "field")]
    pub turn: Option<&'a str>,
}
