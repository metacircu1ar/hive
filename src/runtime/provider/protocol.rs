//! Provider decision views and wire serializers. Opaque evidence always uses the raw Value.
pub(crate) use crate::runtime::domain::wire_enum;

wire_enum!(ItemKind {
    AgentMessage => "agentMessage", CommandExecution => "commandExecution",
    FileChange => "fileChange", DynamicToolCall => "dynamicToolCall",
});
wire_enum!(UnsubscribeStatus {
    Unsubscribed => "unsubscribed", NotSubscribed => "notSubscribed", NotLoaded => "notLoaded",
});
wire_enum!(AccountKind { Chatgpt => "chatgpt", ApiKey => "apiKey" });
wire_enum!(CoordinationTool { Spawn => "hive_spawn", Batch => "hive_batch" });

// Native catalog request page size, independent of Hive history paging.
pub(crate) const MODEL_PAGE_LIMIT: i32 = 100;

pub(super) use crate::provider_support::wire::codes;
mod incoming;
pub(super) mod lenient;
pub(crate) use crate::provider_support::wire::outgoing;
pub(in crate::runtime) use lenient::drift;

pub(crate) use crate::provider_support::wire::{ClientMethod, ServerMethod};
pub(crate) use incoming::{IncomingFrame, Model, ParamsRef, Response, validate_model};

#[cfg(test)]
mod tests;
