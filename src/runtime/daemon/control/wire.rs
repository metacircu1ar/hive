//! Canonical control encoding; committed replay trees keep their owned fast path.
use crate::{
    error::Result,
    native_sandbox::Report,
    runtime::{api, live, provider},
};
use serde::Serialize;
use serde_json::Value;

#[derive(Serialize)]
#[serde(untagged)]
pub(super) enum Payload {
    Core(api::Response),
    Probe(Report),
    Live(live::Page),
    Usage(api::UsageReply<Vec<provider::usage::Report>>),
    Tree(api::TreeReply<api::Response>),
    Attach(api::AttachReply<api::Response>),
    Error(api::ErrorResponse),
}

pub(super) struct Output {
    payload: Payload,
    metadata: api::ControlReply,
}
impl Output {
    pub(super) fn plain(payload: impl Into<Payload>) -> Self {
        Self {
            payload: payload.into(),
            metadata: api::ControlReply::default(),
        }
    }
    pub(super) fn core(payload: api::Response, metadata: api::ControlReply) -> Self {
        Self {
            payload: Payload::Core(payload),
            metadata,
        }
    }
}

macro_rules! payload_from {
    ($($variant:ident($kind:ty)),+ $(,)?) => {
        $(impl From<$kind> for Payload {
            fn from(payload: $kind) -> Self { Self::$variant(payload) }
        })+
    };
}
payload_from! {
    Probe(Report), Live(live::Page), Error(api::ErrorResponse),
    Usage(api::UsageReply<Vec<provider::usage::Report>>),
    Tree(api::TreeReply<api::Response>), Attach(api::AttachReply<api::Response>),
}

pub(super) fn encode(result: Result<Output>, request_id: Option<String>) -> Value {
    let mut output = match result {
        Ok(output) => output,
        Err(error) => Output::plain(api::ErrorResponse { error }),
    };
    if request_id.is_some() {
        output.metadata.request_id = request_id;
    }
    match output.payload {
        Payload::Core(api::Response::Recorded(value)) => recorded(value, output.metadata),
        payload => api::value(output.metadata.with_payload(payload)),
    }
}

fn recorded(mut response: Value, metadata: api::ControlReply) -> Value {
    if metadata.is_empty() {
        return response;
    }
    let Value::Object(fields) = api::value(metadata) else {
        unreachable!("control decorations serialize as an object");
    };
    #[expect(clippy::expect_used, reason = "Decorated replies are objects.")]
    response
        .as_object_mut()
        .expect("control replies are objects")
        .extend(fields);
    response
}

#[cfg(test)]
mod tests;
