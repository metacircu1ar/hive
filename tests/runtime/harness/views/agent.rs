//! Public status projections; private storage records are never used as wire decoders.
use super::{Map, Value, decode};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "The public status contract contains independent lifecycle and admission flags."
)]
pub(crate) struct AgentView {
    pub id: String,
    pub state: String,
    pub task_outcome: String,
    pub tool_catalog: Value,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub role: Option<String>,
    #[serde(
        default,
        deserialize_with = "decode::present",
        skip_serializing_if = "Option::is_none"
    )]
    pub name: Option<String>,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub parent_id: Option<String>,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub branched_from_agent_id: Option<String>,
    pub branch_hold: u64,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub branch_phase: Option<String>,
    #[serde(deserialize_with = "decode::nullable_object")]
    pub last_finished_turn: Option<FinishedTurn>,
    #[serde(deserialize_with = "decode::nullable_object")]
    pub last_attempted_turn: Option<AttemptedTurn>,
    #[serde(deserialize_with = "decode::nullable_object")]
    pub native_usage: Option<Value>,
    pub auth: String,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub auth_group: Option<String>,
    #[serde(deserialize_with = "decode::nullable_object")]
    pub auth_decision: Option<Value>,
    pub api_fallback: bool,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub waiting_reason: Option<String>,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub quota_wait_until: Option<u64>,
    #[serde(deserialize_with = "decode::nullable_object")]
    pub auth_switch_pending: Option<Value>,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub handoff: Option<String>,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub turn_auth_revision: Option<u64>,
    #[serde(deserialize_with = "decode::nullable_exact_object")]
    pub execution_key: Option<hive::state::KeyAuthority>,
    #[serde(deserialize_with = "decode::nullable_exact_object")]
    pub model_key: Option<hive::state::KeyAuthority>,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub auth_probe_id: Option<String>,
    pub auth_checked_revision: u64,
    pub auth_generation: String,
    #[serde(
        default,
        deserialize_with = "decode::present",
        skip_serializing_if = "Option::is_none"
    )]
    pub recovery_fence: Option<String>,
    pub conversation_clean: bool,
    #[serde(deserialize_with = "decode::exact_object")]
    pub profile: hive::config::Profile,
    pub model: String,
    pub effort: String,
    pub options_revision: u64,
    #[serde(deserialize_with = "decode::exact_object")]
    pub agent_options: hive::options::OptionsView,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub applied_options_revision: Option<u64>,
    #[serde(deserialize_with = "decode::nullable_object")]
    pub reserved_turn: Option<Reservation>,
    #[serde(deserialize_with = "decode::nullable_object")]
    pub session_selection: Option<Selection>,
    #[serde(deserialize_with = "decode::nullable_object")]
    pub last_accepted_selection: Option<Selection>,
    #[serde(deserialize_with = "decode::nullable_object")]
    pub turn_selection: Option<Selection>,
    #[serde(
        default,
        deserialize_with = "decode::present",
        skip_serializing_if = "Option::is_none"
    )]
    pub redirect_pending: Option<bool>,
    pub cwd: String,
    #[serde(
        default,
        deserialize_with = "decode::present",
        skip_serializing_if = "Option::is_none"
    )]
    pub instructions: Option<String>,
    #[serde(deserialize_with = "decode::string_enum")]
    pub lifetime: hive::config::Lifetime,
    pub one_shot: bool,
    #[serde(deserialize_with = "decode::string_enum")]
    pub sandbox: hive::config::Sandbox,
    pub paused: bool,
    pub expired: bool,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub thread_id: Option<String>,
    #[serde(
        default,
        deserialize_with = "decode::present",
        skip_serializing_if = "Option::is_none"
    )]
    pub native_input_attempted: Option<bool>,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub execution_id: Option<String>,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub turn_id: Option<String>,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub control: Option<String>,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub completion_pending: Option<String>,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub completion_pending_by: Option<String>,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub result: Option<String>,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub completed_by: Option<String>,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub result_selection: Option<String>,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub result_read_target: Option<String>,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub error: Option<String>,
    pub created_at: u64,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub ended_at: Option<u64>,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub auth_cutoff_turn: Option<String>,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub auth_cutoff_message: Option<String>,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub provider_id: Option<String>,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub provider_pid: Option<u32>,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub provider_spawn_before: Option<u64>,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub provider_spawn_after: Option<u64>,
    #[serde(
        default,
        deserialize_with = "decode::present",
        skip_serializing_if = "Option::is_none"
    )]
    pub steering_supported: Option<bool>,
    #[serde(
        default,
        deserialize_with = "decode::present",
        skip_serializing_if = "Option::is_none"
    )]
    pub steering_disabled_reason: Option<String>,
    #[serde(
        default,
        deserialize_with = "decode::present",
        skip_serializing_if = "Option::is_none"
    )]
    pub steering_proof_error_code: Option<String>,
    #[serde(deserialize_with = "decode::nullable_object")]
    pub comb: Option<Value>,
    pub mcp_servers: Vec<String>,
    pub automatic_continuation: String,
    pub one_shot_waiting: String,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub idle_since: Option<u64>,
    #[serde(deserialize_with = "decode::string_enum")]
    pub provider: hive::config::Provider,
    #[serde(deserialize_with = "decode::string_enum")]
    pub billing: hive::config::Method,
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
    pub waiting_on: Option<String>,
    #[serde(
        default,
        deserialize_with = "decode::present",
        skip_serializing_if = "Option::is_none"
    )]
    pub approval_id: Option<String>,
    #[serde(
        default,
        deserialize_with = "decode::present",
        skip_serializing_if = "Option::is_none"
    )]
    pub open_joins: Option<Vec<String>>,
    pub open_joins_truncated: bool,
    #[serde(
        default,
        deserialize_with = "decode::present_object",
        skip_serializing_if = "Option::is_none"
    )]
    pub branch: Option<Value>,
    #[serde(
        default,
        deserialize_with = "decode::present",
        skip_serializing_if = "Option::is_none"
    )]
    pub branch_dependents: Option<Vec<String>>,
    #[serde(flatten)]
    other: Map<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FinishedTurn {
    pub thread: String,
    pub execution: String,
    pub turn: String,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub message: Option<String>,
    pub outcome: String,
    pub at: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AttemptedTurn {
    pub message: String,
    pub thread: String,
    pub execution: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Selection {
    pub model: String,
    pub effort: String,
    pub revision: u64,
    #[serde(deserialize_with = "decode::exact_object")]
    pub options: hive::options::Snapshot,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Reservation {
    pub message_id: String,
    #[serde(deserialize_with = "decode::object")]
    pub selection: Selection,
}
