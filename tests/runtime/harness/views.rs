use super::*;
use serde::{Deserialize, Serialize};
use serde_json::Map;

mod admission;
pub(crate) mod agent;
pub(crate) mod calls;
mod decode;
pub(crate) mod joins;
pub(crate) mod listing;
pub(crate) mod live;
mod pages;
mod tree;
pub(crate) use admission::{SendView, SpawnView};
pub(crate) use agent::AgentView;
pub(crate) use listing::AgentsView;
pub(crate) use pages::{BatchView, HistoryView};
pub(crate) use tree::TreeView;
#[cfg(test)]
mod tests;

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct StatusView {
    pub attachment_store_bytes: u64,
    #[serde(
        default,
        deserialize_with = "decode::present_object",
        skip_serializing_if = "Option::is_none"
    )]
    pub latest_attachments: Option<AttachedMessage>,
    #[serde(deserialize_with = "decode::object")]
    pub agent: AgentView,
    #[serde(flatten)]
    other: Map<String, Value>,
}
#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct AttachedMessage {
    pub message_id: String,
    #[serde(deserialize_with = "decode::objects")]
    pub attachments: Vec<Value>,
    #[serde(flatten)]
    other: Map<String, Value>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct EventView {
    pub seq: u64,
    pub agent_id: String,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub execution_id: Option<String>,
    pub timestamp: u64,
    #[serde(rename = "type")]
    pub kind: String,
    pub data: Value,
    #[serde(flatten)]
    other: Map<String, Value>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct EventsView {
    #[serde(deserialize_with = "decode::objects")]
    pub events: Vec<EventView>,
    pub cursor: u64,
    pub pruned_through: u64,
    pub history_expired: bool,
    #[serde(flatten)]
    other: Map<String, Value>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct MessageView {
    pub seq: u64,
    pub id: String,
    pub sender: String,
    pub recipient: String,
    pub body: String,
    pub state: String,
    #[serde(
        default,
        deserialize_with = "decode::present",
        skip_serializing_if = "Option::is_none"
    )]
    pub kind: Option<String>,
    #[serde(
        default,
        deserialize_with = "decode::present",
        skip_serializing_if = "Option::is_none"
    )]
    pub mode: Option<String>,
    #[serde(
        default,
        deserialize_with = "decode::present",
        skip_serializing_if = "Option::is_none"
    )]
    pub delivery_error: Option<String>,
    #[serde(
        default,
        deserialize_with = "decode::present",
        skip_serializing_if = "Option::is_none"
    )]
    pub not_before: Option<u64>,
    #[serde(
        default,
        deserialize_with = "decode::present",
        skip_serializing_if = "Option::is_none"
    )]
    pub attachments: Option<Vec<Value>>,
    #[serde(
        default,
        deserialize_with = "decode::present",
        skip_serializing_if = "Option::is_none"
    )]
    pub schedule_integrity: Option<String>,
    #[serde(
        default,
        deserialize_with = "decode::present_object",
        skip_serializing_if = "Option::is_none"
    )]
    pub steering_target: Option<MessageTarget>,
    #[serde(flatten)]
    other: Map<String, Value>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MessageTarget {
    pub execution_id: String,
    pub turn_id: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct MessagesView {
    #[serde(deserialize_with = "decode::objects")]
    pub messages: Vec<MessageView>,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub next_before: Option<u64>,
    #[serde(flatten)]
    other: Map<String, Value>,
}
impl Harness {
    pub(crate) fn join_snapshot(&self, args: Value) -> joins::JoinSnapshot {
        decode::object(self.ok(args)).unwrap()
    }
    pub(crate) fn batch<T: serde::de::DeserializeOwned>(&self, args: Value) -> BatchView<T> {
        decode::object(self.ok(args)).unwrap()
    }
    pub(crate) fn message_result(&self, args: Value) -> calls::MessageResultView {
        decode::object(self.ok(args)).unwrap()
    }
    pub(crate) fn live(&self, args: Value) -> live::LiveView {
        decode::object(self.ok(args)).unwrap()
    }
    pub(crate) fn attached(&self, args: Value) -> live::AttachView<EventsView> {
        decode::object(self.ok(args)).unwrap()
    }
    pub(crate) fn attached_history(&self, args: Value) -> live::AttachView<HistoryView> {
        decode::object(self.ok(args)).unwrap()
    }
    pub(crate) fn status_reply(&self, args: Value) -> StatusView {
        decode::object(self.ok(args)).unwrap()
    }
    pub(crate) fn tree(&self, args: Value) -> TreeView {
        decode::object(self.ok(args)).unwrap()
    }
    pub(crate) fn spawned(&self, args: Value) -> SpawnView {
        decode::object(self.ok(args)).unwrap()
    }
    pub(crate) fn spawned_id(&self, id: &str, args: Value) -> SpawnView {
        let response = self.call_id(id, args);
        assert!(
            response.get("error").is_none_or(Value::is_null),
            "{response}"
        );
        decode::object(response).unwrap()
    }
    pub(crate) fn sent(&self, args: Value) -> SendView {
        decode::object(self.ok(args)).unwrap()
    }
    pub(crate) fn sent_id(&self, id: &str, args: Value) -> SendView {
        let response = self.call_id(id, args);
        assert!(
            response.get("error").is_none_or(Value::is_null),
            "{response}"
        );
        decode::object(response).unwrap()
    }
    pub(crate) fn status(&self, id: &str) -> AgentView {
        let response = self.ok(json!({"op":"status","target":id}));
        assert!(response.is_object(), "invalid status envelope: {response}");
        let status: StatusView = serde_json::from_value(response).unwrap();
        status.agent
    }
    pub(crate) fn events(&self, id: &str) -> EventsView {
        let mut after = 0;
        let mut events = Vec::new();
        loop {
            let response = self.ok(json!({"op":"read","target":id,"after":after,"limit":200}));
            let mut page: EventsView = decode::object(response).unwrap();
            assert!(!page.history_expired, "test history was pruned for {id}");
            if page.events.is_empty() {
                page.events = events;
                return page;
            }
            assert!(page.cursor > after, "event cursor did not advance for {id}");
            after = page.cursor;
            events.append(&mut page.events);
        }
    }
    pub(crate) fn read(&self, id: &str) -> EventsView {
        self.events(id)
    }
    pub(crate) fn messages(&self, id: &str) -> MessagesView {
        let mut before = None;
        let mut messages = Vec::new();
        loop {
            let mut args = json!({"op":"messages","target":id,"limit":200});
            if let Some(cursor) = before {
                args["before"] = json!(cursor);
            }
            let mut page: MessagesView = decode::object(self.ok(args)).unwrap();
            messages.append(&mut page.messages);
            let Some(cursor) = page.next_before else {
                page.messages = messages;
                return page;
            };
            assert!(
                before.is_none_or(|previous| cursor < previous),
                "mailbox cursor did not advance for {id}"
            );
            before = Some(cursor);
        }
    }
    pub(crate) fn mailbox(&self, id: &str) -> MessagesView {
        self.messages(id)
    }
    pub(crate) fn listed(&self, args: Value) -> AgentsView {
        decode::object(self.ok(args)).unwrap()
    }
}
