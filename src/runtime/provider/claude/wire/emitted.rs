//! Typed translated frames; embedded native arguments, results and receipts stay opaque.
use crate::runtime::{api, domain::ProviderTurnStatus, provider::protocol::ServerMethod};
use serde::Serialize;
use serde_json::Value;
use std::collections::VecDeque;

/// Complete translated frames become canonical JSON when entering this queue.
#[derive(Default)]
pub(crate) struct Frames(VecDeque<Value>);
impl Frames {
    pub(crate) fn reply(&mut self, id: u64, result: impl Serialize) {
        self.0.push_back(reply(id, result));
    }
    pub(crate) fn notice(&mut self, method: ServerMethod, params: impl Serialize) {
        self.0.push_back(notice(method, params));
    }
    pub(crate) fn call(&mut self, call: super::control::ToolCall<'_>) {
        self.0.push_back(api::value(call));
    }
    pub(crate) fn push_back(&mut self, frame: Value) {
        self.0.push_back(frame);
    }
    pub(crate) fn pop_front(&mut self) -> Option<Value> {
        self.0.pop_front()
    }
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[derive(Serialize)]
struct Reply<T> {
    id: u64,
    result: T,
}
#[derive(Serialize)]
struct Notice<T> {
    method: ServerMethod,
    params: T,
}
#[derive(Serialize)]
struct Rejection<'a> {
    id: u64,
    error: Failure<'a>,
}
#[derive(Serialize)]
struct Failure<'a> {
    code: i32,
    message: &'a str,
}
pub(crate) fn reply(id: u64, result: impl Serialize) -> Value {
    api::value(Reply { id, result })
}
pub(crate) fn notice(method: ServerMethod, params: impl Serialize) -> Value {
    api::value(Notice { method, params })
}
pub(crate) fn error(id: u64, code: i32, message: &str) -> Value {
    api::value(Rejection {
        id,
        error: Failure { code, message },
    })
}
#[derive(Serialize)]
pub(crate) struct Identity<'a> {
    pub id: &'a str,
}
#[derive(Serialize)]
pub(crate) struct TurnResult<T> {
    pub turn: T,
}
#[derive(Serialize)]
pub(crate) struct LocatedThread<'a> {
    pub id: &'a str,
    pub path: &'a std::path::Path,
}
#[derive(Serialize)]
pub(crate) struct VerifiedBranchTurn<'a> {
    pub turn: Identity<'a>,
    pub thread: LocatedThread<'a>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadTurn<'a, T> {
    pub thread_id: &'a str,
    pub turn: T,
}
#[derive(Serialize)]
pub(crate) struct TerminalTurn {
    pub id: String,
    pub status: ProviderTurnStatus,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ModelUsage<'a> {
    pub thread_id: &'a str,
    pub turn_id: &'a str,
    pub outcome: ProviderTurnStatus,
    pub models: crate::runtime::domain::ModelUsage,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ItemParams<'a, T> {
    pub thread_id: &'a str,
    pub turn_id: &'a str,
    pub item: T,
}
#[derive(Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub(crate) enum AssistantItem {
    AgentMessage {
        // The native message ID keys live text to its durable message.
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        text: String,
        verified_model: bool,
    },
    DynamicToolCall {
        id: String,
        tool: String,
        arguments: Value,
    },
}
#[derive(Serialize)]
#[serde(tag = "type", rename = "agentMessage")]
#[serde(rename_all = "camelCase")]
pub(crate) struct Summary {
    pub text: String,
    pub verified_model: bool,
    pub aggregation: &'static str,
    pub native_message_id: Option<String>,
}
#[derive(Serialize)]
#[serde(tag = "type", rename = "dynamicToolCall")]
pub(crate) struct ToolResult<'a> {
    pub id: &'a str,
    pub result: &'a Value,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RateLimits<'a> {
    pub info: Value,
    pub thread_id: &'a str,
    pub turn_id: Option<&'a str>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProviderError<'a> {
    pub thread_id: &'a str,
    pub turn_id: Option<&'a str>,
    pub will_retry: bool,
    pub error: ErrorInfo<'a>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ErrorInfo<'a> {
    pub codex_error_info: &'a str,
}
#[derive(Serialize)]
#[serde(tag = "type", rename = "user")]
pub(crate) struct User<'a> {
    pub uuid: &'a str,
    pub session_id: &'a str,
    pub message: UserText,
}
impl User<'_> {
    pub(crate) fn into_value(self) -> Value {
        api::value(self)
    }
}
#[derive(Serialize)]
#[serde(tag = "role", rename = "user")]
pub(crate) struct UserText {
    pub content: UserContent,
}
#[derive(Serialize)]
#[serde(untagged)]
pub(crate) enum UserContent {
    Text(String),
    Blocks(Vec<UserBlock>),
}
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum UserBlock {
    Text {
        text: String,
    },
    Image {
        source: crate::runtime::provider::protocol::outgoing::EncodedImage,
    },
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadSettings<'a> {
    pub thread: Identity<'a>,
    pub model: Option<&'a str>,
    pub reasoning_effort: &'a str,
    pub sandbox: &'a str,
    pub steering_disabled_reason: Option<crate::runtime::steering_canary::SteeringObservation>,
    pub steering_proof_error_code: Option<crate::error::ErrorCode>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Guidance {
    pub hook_specific_output: Context,
}
#[derive(Serialize)]
#[serde(tag = "hookEventName", rename = "PostToolUse")]
#[serde(rename_all = "camelCase")]
pub(crate) struct Context {
    pub additional_context: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SteeringDelivery<'a> {
    pub turn_id: &'a str,
    pub delivery_evidence: DeliveryEvidence<'a>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeliveryEvidence<'a> {
    pub kind: super::super::ReceiptMode,
    pub observation: &'a str,
    pub hook_reply: &'a Value,
}

#[cfg(test)]
mod tests;
