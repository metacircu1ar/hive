//! Typed response exports and command acknowledgements.
use crate::error::Error;
use serde::Serialize;
use serde_json::Value;

mod auth;
pub(crate) use auth::{
    AuthAvailability, AuthChangedResponse, AuthGroup, AuthGroups, AuthMember, KeySourceReply,
};
mod calls;
mod gc;
mod native_history;
pub(crate) use native_history::{
    NATIVE_PREVIEW_LIMIT, NativeCleanup, NativeFilePreview, NativeHistory, NativeSessionPreview,
    NativeWarningReason,
};
mod history;
mod read;
mod reply;
mod roles;
pub(crate) use calls::{CallOutcome, MessageResult, OutputSelection};
pub(crate) use gc::{GC_PREVIEW, GcPage, GcStats};
pub(crate) use history::ReadOutput;
pub(crate) use read::DaemonReply;
pub(crate) use reply::Response;
pub(crate) use roles::RolesResponse;

pub(crate) use crate::api::value;

/// Measure a typed page row without discarding its type. Object ordering cannot
/// change encoded length; the eventual transport still sorts keys through Value.
pub(crate) fn encoded_len(response: &impl Serialize) -> usize {
    #[expect(clippy::expect_used, reason = "Page fields serialize to JSON.")]
    serde_json::to_vec(response)
        .expect("runtime responses must be JSON serializable")
        .len()
}

#[derive(Serialize)]
pub(crate) struct ErrorResponse {
    pub error: Error,
}
#[derive(serde::Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BatchResponse<R = Value> {
    pub results: Vec<R>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default, deserialize_with = "crate::decode::optional_object")]
    pub join: Option<super::joins::JoinAccepted>,
}
mod acknowledgements;
pub(crate) use acknowledgements::{
    CompleteResponse, ControlOperation, ControlResponse, ResponseStatus, SpawnResponse,
    StatusResponse, TerminateResponse,
};
#[derive(Serialize)]
pub(crate) struct OptionsChangedResponse {
    pub agent_id: String,
    pub desired: super::super::domain::OptionsSelection,
    pub takes_effect: std::collections::BTreeMap<String, &'static str>,
}
#[derive(Serialize)]
pub(crate) struct NameChangedResponse<S = String> {
    pub agent_id: S,
    pub name: Option<S>,
}
#[derive(Serialize)]
pub(crate) struct ModelsResponse {
    pub models: Value,
}
#[derive(Serialize)]
pub(crate) struct HistoryExpiredResponse<'a> {
    pub error: Error,
    pub pruned_through: &'a Value,
}
#[derive(Serialize)]
pub(crate) struct WaitEventResponse<'a> {
    pub agent: &'a Value,
    pub event: &'a Value,
}
#[derive(Serialize)]
pub(crate) struct WaitObservationResponse<'a> {
    pub wait: &'static str,
    pub agent_id: &'a str,
    pub agent: &'a Value,
}

#[derive(Serialize)]
pub(crate) struct CompletionResult<'a> {
    pub agent_id: &'a str,
    pub result: Option<&'a str>,
    pub result_selection: Option<crate::runtime::domain::OutputSelection>,
    pub result_read_target: Option<&'a str>,
}

mod agents;
pub(crate) use agents::{
    AgentResponse, AttachedMessage, ListedAgent, ListingFields, SteeringVerification,
    VerificationSelection, listed_agent,
};

mod status;
pub(crate) use status::{
    Capabilities, CombBacklog, ControlReply, PingResponse, ProviderCapabilities, RuntimeResources,
    SteeringPolicy, StorageDiagnostics, WriterObservation,
};

mod messaging;
pub(crate) use messaging::{
    MessageRecord, MessagesPage, RedirectTarget, SendResponse, SteeringTarget,
};

mod pages;
pub(crate) use pages::{AgentsPage, EventRecord, EventsPage, HistoryPage, OutputResponse};

mod maintenance;
pub(crate) use maintenance::{PurgeBlocker, PurgeCounts, PurgeResponse};

mod live;
pub(crate) use live::{AttachReply, AttachResponse, TreeReply, TreeResponse, TreeRow};

pub(crate) use read::objects;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RequiredBuild {
    Comb,
}
/// A public error envelope whose provider-specific payload is already typed.
#[cfg(feature = "comb")]
#[derive(serde::Serialize)]
pub(crate) struct ValueError<T> {
    pub error: T,
}

#[derive(serde::Serialize)]
pub(crate) struct UsageReply<T> {
    pub usage: T,
}

#[cfg(feature = "comb")]
#[derive(serde::Serialize)]
pub(crate) struct CollectorReply<T> {
    pub collectors: T,
}
