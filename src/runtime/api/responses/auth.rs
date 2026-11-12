//! Login selection and availability response records.
use crate::{
    config::{AuthMode, Method, Provider},
    runtime::auth_fallback::Availability,
};
use serde::Serialize;

#[derive(Serialize)]
pub(crate) struct KeySourceReply {
    #[serde(flatten)]
    pub report: crate::auth::Report,
    pub availability: Vec<Availability>,
}

#[derive(Serialize)]
pub(crate) struct AuthChangedResponse<S = String> {
    pub agent_id: S,
    pub auth: S,
    pub auth_group: Option<S>,
    pub availability: Availability,
    pub takes_effect: &'static str,
}
#[derive(Serialize)]
pub(crate) struct AuthAvailability {
    pub availability: Vec<Availability>,
}
#[derive(Serialize)]
pub(crate) struct AuthGroups {
    pub groups: Vec<AuthGroup>,
}
#[derive(Serialize)]
pub(crate) struct AuthGroup {
    pub name: String,
    pub provider: Provider,
    pub mode: AuthMode,
    pub members: Vec<AuthMember>,
    pub paid_tail: Vec<AuthMember>,
}
#[derive(Serialize)]
pub(crate) struct AuthMember {
    pub profile: String,
    pub method: Method,
    /// Present only in balanced groups; ordered groups ignore weights.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weight: Option<u16>,
    pub availability: Vec<Availability>,
    pub bound_agents: u64,
    pub active_turns: u64,
}
