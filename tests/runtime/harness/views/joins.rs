//! Joined replies distinguish closed snapshots from still-waiting observations.
use super::{Map, Value, decode};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct JoinAccepted {
    pub join_id: String,
    pub members: Vec<String>,
    #[serde(flatten)]
    other: Map<String, Value>,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct JoinOutput {
    pub text: String,
    #[serde(
        default,
        deserialize_with = "decode::present",
        skip_serializing_if = "Option::is_none"
    )]
    pub read_target: Option<String>,
    #[serde(flatten)]
    other: Map<String, Value>,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct JoinMember {
    #[serde(
        default,
        deserialize_with = "decode::present",
        skip_serializing_if = "Option::is_none"
    )]
    pub held_for_build: Option<String>,
    pub position: usize,
    pub message_id: String,
    pub target: String,
    pub outcome: String,
    #[serde(deserialize_with = "decode::nullable_object")]
    pub output: Option<JoinOutput>,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub output_selection: Option<String>,
    pub output_partial: bool,
    #[serde(deserialize_with = "decode::nullable_object")]
    pub task_completed: Option<JoinOutput>,
    pub delivered: bool,
    #[serde(flatten)]
    other: Map<String, Value>,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct JoinSnapshot {
    #[serde(
        default,
        deserialize_with = "decode::present",
        skip_serializing_if = "Option::is_none"
    )]
    pub held_for_build: Option<String>,
    pub join_id: String,
    pub requester: String,
    pub request_id: String,
    pub created_at: u64,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub deadline: Option<u64>,
    pub state: String,
    #[serde(deserialize_with = "decode::objects")]
    pub members: Vec<JoinMember>,
    #[serde(flatten)]
    other: Map<String, Value>,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct JoinObservation {
    pub wait: String,
    pub join_id: String,
    #[serde(deserialize_with = "decode::nullable_object")]
    pub join: Option<JoinSnapshot>,
    #[serde(
        default,
        deserialize_with = "decode::present_object",
        skip_serializing_if = "Option::is_none"
    )]
    pub error: Option<super::super::super::support::reply::Problem>,
    #[serde(flatten)]
    other: Map<String, Value>,
}
