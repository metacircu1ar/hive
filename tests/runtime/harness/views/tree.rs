//! Compact live tree observations are distinct from status and list projections.
use super::{Map, Value, decode};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct TreeView {
    #[serde(deserialize_with = "decode::object")]
    pub version: Value,
    #[serde(deserialize_with = "decode::objects")]
    pub agents: Vec<TreeAgent>,
    pub truncated: bool,
    #[serde(flatten)]
    other: Map<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct TreeAgent {
    pub id: String,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub parent_id: Option<String>,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub role: Option<String>,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub name: Option<String>,
    pub state: String,
    pub task_outcome: String,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub waiting_reason: Option<String>,
    pub auth: String,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub auth_group: Option<String>,
    #[serde(deserialize_with = "decode::nullable_object")]
    pub auth_decision: Option<Value>,
    pub model: String,
    pub effort: String,
    #[serde(deserialize_with = "decode::string_enum")]
    pub sandbox: hive::config::Sandbox,
    pub one_shot: bool,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub ended_at: Option<u64>,
    pub children: u64,
    pub queued: u64,
    pub mail: u64,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub approval_id: Option<String>,
    #[serde(flatten)]
    other: Map<String, Value>,
}
