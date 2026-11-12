//! Message-bound observations retain required nulls and dynamic turn output.
use super::{Map, Value, decode};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "Independent public call-result flags keep their exact wire fields."
)]
pub(crate) struct MessageResultView {
    #[serde(
        default,
        deserialize_with = "decode::present",
        skip_serializing_if = "Option::is_none"
    )]
    pub held_for_build: Option<String>,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub not_before: Option<u64>,
    pub message_id: String,
    pub recipient: String,
    pub delivery_state: String,
    pub outcome: String,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub execution_id: Option<String>,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub thread_id: Option<String>,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub turn_id: Option<String>,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub events_after: Option<u64>,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub output: Option<Value>,
    pub output_selection: String,
    pub output_partial: bool,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub queued_ahead: Option<u64>,
    pub queue_count_exact: bool,
    pub active_input: bool,
    pub recipient_state: String,
    pub paused: bool,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub delivery_error: Option<String>,
    #[serde(flatten)]
    other: Map<String, Value>,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct CallView {
    pub outcome: String,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub message_id: Option<String>,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub request_id: Option<String>,
    #[serde(deserialize_with = "decode::nullable_object")]
    pub result: Option<MessageResultView>,
    #[serde(deserialize_with = "decode::nullable_object")]
    pub error: Option<super::super::super::support::reply::Problem>,
    #[serde(flatten)]
    other: Map<String, Value>,
}
