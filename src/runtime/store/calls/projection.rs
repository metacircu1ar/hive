//! Borrow assistant metadata while retaining its complete opaque event payload.
use serde::Deserialize;
use serde_json::Value;

#[derive(Default, Deserialize)]
#[serde(default)]
pub(super) struct Metadata<'a> {
    #[serde(borrow, deserialize_with = "crate::decode::field")]
    pub turn_id: Option<&'a str>,
    #[serde(borrow, deserialize_with = "crate::decode::field")]
    pub phase: Option<&'a str>,
    #[serde(borrow, deserialize_with = "crate::decode::field")]
    pub text: Option<&'a str>,
}

pub(super) fn read(value: &Value) -> Metadata<'_> {
    crate::decode::object(value).unwrap_or_default()
}
