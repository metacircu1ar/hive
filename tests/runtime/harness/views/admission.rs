//! Admissions retain their exact tags and provider-independent target identity.
use super::{Map, MessageTarget, Value, decode};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SpawnStatus {
    Accepted,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct SpawnView {
    #[serde(deserialize_with = "decode::string_enum")]
    pub status: SpawnStatus,
    pub agent_id: String,
    pub message_id: String,
    #[serde(
        default,
        deserialize_with = "decode::present",
        skip_serializing_if = "Option::is_none"
    )]
    pub held_for_build: Option<String>,
    #[serde(flatten)]
    other: Map<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct SendView {
    pub message_id: String,
    pub delivery_state: String,
    pub recipient: String,
    #[serde(
        default,
        deserialize_with = "decode::present",
        skip_serializing_if = "Option::is_none"
    )]
    pub held_for_build: Option<String>,
    #[serde(
        default,
        deserialize_with = "decode::present",
        skip_serializing_if = "Option::is_none"
    )]
    pub not_before: Option<u64>,
    #[serde(
        default,
        deserialize_with = "decode::present_object",
        skip_serializing_if = "Option::is_none"
    )]
    pub steering_target: Option<MessageTarget>,
    #[serde(
        default,
        deserialize_with = "decode::present_object",
        skip_serializing_if = "Option::is_none"
    )]
    pub redirect_target: Option<RedirectTarget>,
    #[serde(flatten)]
    other: Map<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[expect(
    clippy::struct_field_names,
    reason = "The public redirect identity keeps its three explicit id field names."
)]
pub(crate) struct RedirectTarget {
    pub execution_id: String,
    pub message_id: String,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub turn_id: Option<String>,
}
