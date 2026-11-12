//! Shared operation types and adapters for the three request boundaries.
mod approvals;
pub(crate) mod batch;
pub(crate) mod events;
mod input;
pub(crate) mod joins;
pub(crate) mod pagination;
mod request;
mod responses;
pub(crate) use responses::{KeySourceReply, ReadOutput, Response, encoded_len};
pub(crate) use responses::{
    NATIVE_PREVIEW_LIMIT, NativeCleanup, NativeFilePreview, NativeHistory, NativeSessionPreview,
    NativeWarningReason,
};
pub(in crate::runtime) mod selection;
mod stalls;
mod tools;
pub(crate) use stalls::{STALLED_PREVIEW, StalledCursor};
pub(crate) use stalls::{StalledGroup, StalledPage};
pub(crate) use tools::native_parallel_read;

pub(crate) use approvals::{
    ApprovalAction, ApprovalArgs, ApprovalCursor, ApprovalInput, ApprovalOptions, ApprovalsArgs,
    AskArgs, AskContent, RespondArgs,
};
pub(crate) use input::{Input, InputContext};
pub(crate) use request::{
    AgentsArgs, AttachArgs, AuthArgs, BatchArgs, BatchItem, BatchKind, BatchOperation,
    CleanupOptions, CompleteArgs, GcArgs, GcCursor, GcOptions, KeySourceArgs, LiveArgs,
    MessageResultArgs, MessagesArgs, OptionsCatalogArgs, OutputArgs, PurgeArgs, PurgeOptions,
    ReadArgs, Request, RequestEnvelope, SendArgs, SetAuthArgs, SetNameArgs, SetOptionsArgs,
    SpawnArgs, StatusArgs, TargetArgs, TreeArgs, UsageArgs,
};
pub(crate) use responses::{
    AgentResponse, AgentsPage, AttachReply, AttachResponse, AttachedMessage, AuthAvailability,
    AuthChangedResponse, AuthGroup, AuthGroups, AuthMember, BatchResponse, CallOutcome,
    Capabilities, CombBacklog, CompleteResponse, ControlOperation, ControlReply, ControlResponse,
    DaemonReply, ErrorResponse, EventRecord, EventsPage, GC_PREVIEW, GcPage, GcStats,
    HistoryExpiredResponse, HistoryPage, ListingFields, MessageRecord, MessageResult, MessagesPage,
    ModelsResponse, NameChangedResponse, OptionsChangedResponse, OutputResponse, OutputSelection,
    PingResponse, ProviderCapabilities, PurgeBlocker, PurgeCounts, PurgeResponse, RedirectTarget,
    RequiredBuild, ResponseStatus, RolesResponse, RuntimeResources, SendResponse, SpawnResponse,
    StatusResponse, SteeringPolicy, SteeringTarget, SteeringVerification, StorageDiagnostics,
    TerminateResponse, TreeReply, TreeResponse, TreeRow, UsageReply, VerificationSelection,
    WaitEventResponse, WaitObservationResponse, WriterObservation, listed_agent, value,
};
#[cfg(feature = "comb")]
pub(crate) use responses::{CollectorReply, ValueError};
pub(crate) use tools::{
    contextualize as tool_input_context, error as tool_input_error, tools, validate_tool,
};

#[cfg(test)]
// Test fixtures and assertions deliberately panic on failure.
mod tests;
