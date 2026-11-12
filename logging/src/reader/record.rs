//! Borrow filtering fields without reconstructing or narrowing the retained record.
use serde::{Deserialize, Deserializer};
use serde_json::Value;

#[derive(Deserialize)]
pub(super) struct Record<'a> {
    #[serde(borrow)]
    pub event: &'a str,
    #[serde(default, borrow, deserialize_with = "text")]
    pub agent_id: Option<&'a str>,
    #[serde(default, borrow, deserialize_with = "text")]
    pub level: Option<&'a str>,
}
impl<'a> Record<'a> {
    pub fn read(value: &'a Value) -> Option<Self> {
        // Serde structs also accept positional arrays; diagnostic records never did.
        if !value.is_object() {
            return None;
        }
        Self::deserialize(value).ok()
    }
}

// Input is an already parsed Value, so a bad optional field cannot consume the next one.
fn text<'de, D: Deserializer<'de>>(input: D) -> Result<Option<&'de str>, D::Error> {
    Option::<&str>::deserialize(input).or(Ok(None))
}
