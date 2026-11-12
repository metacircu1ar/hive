//! The lightweight agents page deliberately has no status-only defaults.
use super::{Map, Value, decode};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct AgentsView {
    #[serde(deserialize_with = "decode::objects")]
    pub agents: Vec<ListedAgentView>,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub next_after: Option<u64>,
    #[serde(flatten)]
    other: Map<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct ListedAgentView {
    pub id: String,
    pub state: String,
    pub task_outcome: String,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub parent_id: Option<String>,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub role: Option<String>,
    #[serde(
        default,
        deserialize_with = "decode::present",
        skip_serializing_if = "Option::is_none"
    )]
    pub name: Option<String>,
    #[serde(
        default,
        deserialize_with = "decode::present",
        skip_serializing_if = "Option::is_none"
    )]
    pub native_usage: Option<Value>,
    pub model: String,
    pub effort: String,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub thread_id: Option<String>,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub execution_id: Option<String>,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub turn_id: Option<String>,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub control: Option<String>,
    pub expired: bool,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub error: Option<String>,
    #[serde(deserialize_with = "decode::string_enum")]
    pub lifetime: hive::config::Lifetime,
    #[serde(flatten)]
    other: Map<String, Value>,
}
