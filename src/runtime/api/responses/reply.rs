//! Owned command results stay typed until replay storage or transport encoding.
#[cfg(feature = "comb")]
use super::CollectorReply;
use super::{
    AgentResponse, AgentsPage, AttachResponse, AuthAvailability, AuthChangedResponse, AuthGroups,
    BatchResponse, CompleteResponse, ControlResponse, GcPage, MessageResult, MessagesPage,
    NameChangedResponse, OptionsChangedResponse, OutputResponse, PingResponse, PurgeResponse,
    ReadOutput, RolesResponse, SendResponse, Serialize, SpawnResponse, StatusResponse,
    TerminateResponse, TreeResponse, Value, value,
};

#[derive(Serialize)]
#[serde(untagged)]
pub(crate) enum Response {
    Spawn(SpawnResponse),
    Send(SendResponse),
    Complete(CompleteResponse),
    Terminate(TerminateResponse),
    Control(ControlResponse),
    Status(StatusResponse),
    Options(OptionsChangedResponse),
    Name(NameChangedResponse),
    Auth(AuthChangedResponse),
    Availability(AuthAvailability),
    Ask(crate::approvals::AskResponse),
    Respond(crate::approvals::RespondResponse),
    Batch(BatchResponse<Response>),
    Purge(PurgeResponse),
    Gc(GcPage),
    Groups(AuthGroups),
    Roles(RolesResponse),
    Agents(AgentsPage),
    Agent(Box<AgentResponse<crate::runtime::read_api::ApprovalStatus>>),
    Tree(TreeResponse),
    Attach(Box<AttachResponse>),
    Read(ReadOutput),
    Messages(MessagesPage),
    MessageResult(MessageResult),
    Approvals(crate::approvals::ApprovalsPage),
    Approval(crate::approvals::ApprovalResponse),
    Output(OutputResponse),
    Stalled(crate::runtime::api::StalledPage),
    Catalog(crate::options::CatalogListing),
    Ping(Box<PingResponse>),
    ReaderPing(Box<crate::runtime::reader::ReaderPing>),
    KeySource(super::auth::KeySourceReply),
    #[cfg(feature = "comb")]
    Evidence(crate::runtime::collectors::Admission),
    #[cfg(feature = "comb")]
    Collectors(CollectorReply<Vec<crate::runtime::collectors::Status>>),
    /// A canonical response already encoded for durable request deduplication.
    Recorded(Value),
}

impl Response {
    pub(crate) fn into_value(self) -> Value {
        match self {
            Self::Recorded(value) => value,
            reply => value(reply),
        }
    }

    pub(crate) fn message_id(&self) -> Option<&str> {
        match self {
            Self::Spawn(reply) => Some(&reply.message_id),
            Self::Send(reply) => Some(&reply.message_id),
            Self::Recorded(value) => value.get("message_id").and_then(Value::as_str),
            _ => None,
        }
    }
}

macro_rules! response_from {
    ($($variant:ident($kind:ty)),+ $(,)?) => {
        $(impl From<$kind> for Response {
            fn from(reply: $kind) -> Self { Self::$variant(reply) }
        })+
    };
}
response_from! {
    Spawn(SpawnResponse), Send(SendResponse), Complete(CompleteResponse),
    Terminate(TerminateResponse), Control(ControlResponse), Status(StatusResponse),
    Options(OptionsChangedResponse), Name(NameChangedResponse), Auth(AuthChangedResponse),
    Availability(AuthAvailability), Ask(crate::approvals::AskResponse),
    Respond(crate::approvals::RespondResponse), Batch(BatchResponse<Response>),
    Purge(PurgeResponse), Gc(GcPage),
    Groups(AuthGroups), Roles(RolesResponse), Agents(AgentsPage), Tree(TreeResponse),
    Read(ReadOutput), Messages(MessagesPage), MessageResult(MessageResult),
    Approvals(crate::approvals::ApprovalsPage), Approval(crate::approvals::ApprovalResponse),
    Output(OutputResponse), Stalled(crate::runtime::api::StalledPage),
    Catalog(crate::options::CatalogListing), KeySource(super::auth::KeySourceReply),
}

macro_rules! boxed_response_from {
    ($($variant:ident($kind:ty)),+ $(,)?) => {
        $(impl From<$kind> for Response {
            fn from(reply: $kind) -> Self { Self::$variant(Box::new(reply)) }
        })+
    };
}
boxed_response_from! {
    Agent(AgentResponse<crate::runtime::read_api::ApprovalStatus>), Attach(AttachResponse),
    Ping(PingResponse), ReaderPing(crate::runtime::reader::ReaderPing),
}

#[cfg(feature = "comb")]
impl From<crate::runtime::collectors::Admission> for Response {
    fn from(reply: crate::runtime::collectors::Admission) -> Self {
        Self::Evidence(reply)
    }
}

#[cfg(feature = "comb")]
impl From<CollectorReply<Vec<crate::runtime::collectors::Status>>> for Response {
    fn from(reply: CollectorReply<Vec<crate::runtime::collectors::Status>>) -> Self {
        Self::Collectors(reply)
    }
}
