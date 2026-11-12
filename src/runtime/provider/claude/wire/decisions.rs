//! Borrowed routing metadata. The native Value remains authoritative evidence.
use crate::runtime::provider::protocol::lenient;
use serde::Deserialize;
use serde_json::Value;
use std::borrow::Cow;

#[derive(Default, Deserialize)]
pub(crate) struct Decision<'a> {
    #[serde(default, rename = "type", borrow, deserialize_with = "lenient::text")]
    kind: Option<Cow<'a, str>>,
    #[serde(default, borrow, deserialize_with = "lenient::text")]
    subtype: Option<Cow<'a, str>>,
    #[serde(default, rename = "isReplay", deserialize_with = "lenient::optional")]
    replay: Option<bool>,
    #[serde(default, borrow, deserialize_with = "lenient::defaulting")]
    request: Request<'a>,
    #[serde(default, borrow, deserialize_with = "lenient::defaulting")]
    response: Reply<'a>,
}
#[derive(Default, Deserialize)]
struct Request<'a> {
    #[serde(default, borrow, deserialize_with = "lenient::text")]
    subtype: Option<Cow<'a, str>>,
    #[serde(default, borrow, deserialize_with = "lenient::defaulting")]
    message: Message<'a>,
}
#[derive(Default, Deserialize)]
struct Message<'a> {
    #[serde(default, borrow, deserialize_with = "lenient::text")]
    method: Option<Cow<'a, str>>,
}
#[derive(Default, Deserialize)]
struct Reply<'a> {
    #[serde(default, borrow, deserialize_with = "lenient::text")]
    subtype: Option<Cow<'a, str>>,
    #[serde(default, borrow, deserialize_with = "lenient::text")]
    request_id: Option<Cow<'a, str>>,
    #[serde(default, borrow, deserialize_with = "lenient::defaulting")]
    response: Payload<'a>,
}
#[derive(Default, Deserialize)]
struct Payload<'a> {
    #[serde(
        default,
        rename = "hookSpecificOutput",
        borrow,
        deserialize_with = "lenient::defaulting"
    )]
    hook: Hook<'a>,
}
#[derive(Default, Deserialize)]
struct Hook<'a> {
    #[serde(
        default,
        rename = "additionalContext",
        borrow,
        deserialize_with = "lenient::text"
    )]
    context: Option<Cow<'a, str>>,
}
impl<'a> Decision<'a> {
    pub(crate) fn read(raw: &'a Value) -> Self {
        lenient::defaulting(raw).unwrap_or_default()
    }
    /// Fork verification allows metadata independently of pre-input admission.
    pub(crate) fn branch_metadata(&self) -> bool {
        match self.kind.as_deref() {
            Some("control_response" | "control_cancel_request") => true,
            Some("system") => self.subtype.as_deref() == Some("init"),
            Some("user") => self.replay == Some(true),
            Some("control_request") => {
                self.request.subtype.as_deref() == Some("mcp_message")
                    && matches!(
                        self.request.message.method.as_deref(),
                        Some(
                            "initialize"
                                | "notifications/initialized"
                                | "ping"
                                | "tools/list"
                                | "notifications/cancelled"
                        )
                    )
            }
            _ => false,
        }
    }
    pub(crate) fn successful_reply(&self) -> bool {
        self.kind.as_deref() == Some("control_response")
            && self.response.subtype.as_deref() == Some("success")
    }
    pub(crate) fn has_guidance(&self) -> bool {
        self.response.response.hook.context.is_some()
    }
    pub(crate) fn request_id(&self) -> Option<&str> {
        self.response.request_id.as_deref()
    }
}

#[cfg(test)]
mod tests;
