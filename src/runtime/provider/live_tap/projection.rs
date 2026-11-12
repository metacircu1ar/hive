//! Temporary borrowed projections; only emitted live items allocate owned text.
use super::super::protocol::lenient::{self, Present};
use serde::Deserialize;
use serde_json::Value;
use std::borrow::Cow;

#[derive(Default, Deserialize)]
pub(super) struct Method<'a> {
    #[serde(default, borrow, deserialize_with = "lenient::text")]
    pub method: Option<Cow<'a, str>>,
}
#[derive(Default, Deserialize)]
pub(super) struct Frame<'a> {
    #[serde(default, borrow, deserialize_with = "lenient::defaulting")]
    pub params: DeltaParams<'a>,
}
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct DeltaParams<'a> {
    #[serde(default, borrow, deserialize_with = "lenient::text")]
    pub thread_id: Option<Cow<'a, str>>,
    #[serde(default, borrow, deserialize_with = "lenient::text")]
    pub turn_id: Option<Cow<'a, str>>,
    #[serde(default, borrow, deserialize_with = "lenient::text")]
    pub item_id: Option<Cow<'a, str>>,
    #[serde(default, borrow, deserialize_with = "lenient::text")]
    pub delta: Option<Cow<'a, str>>,
}
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Params<'a> {
    #[serde(default, borrow, deserialize_with = "lenient::text")]
    pub turn_id: Option<Cow<'a, str>>,
    #[serde(default, borrow, deserialize_with = "lenient::present_object")]
    pub turn: Present<Turn<'a>>,
    #[serde(default, borrow, deserialize_with = "lenient::defaulting")]
    pub item: Item<'a>,
}
#[derive(Clone, Default, Deserialize)]
pub(super) struct Turn<'a> {
    #[serde(default, borrow, deserialize_with = "lenient::text")]
    pub id: Option<Cow<'a, str>>,
    #[serde(default, borrow, deserialize_with = "lenient::text")]
    pub status: Option<Cow<'a, str>>,
}
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Item<'a> {
    #[serde(default, rename = "type", borrow, deserialize_with = "lenient::text")]
    pub kind: Option<Cow<'a, str>>,
    #[serde(default, borrow, deserialize_with = "lenient::text")]
    pub id: Option<Cow<'a, str>>,
    #[serde(default, borrow, deserialize_with = "lenient::text")]
    pub status: Option<Cow<'a, str>>,
    #[serde(default, borrow, deserialize_with = "lenient::text")]
    pub tool: Option<Cow<'a, str>>,
    #[serde(default, borrow, deserialize_with = "lenient::text")]
    pub server: Option<Cow<'a, str>>,
    #[serde(default, borrow, deserialize_with = "lenient::optional")]
    pub command: Option<Command<'a>>,
    #[serde(default, borrow, deserialize_with = "lenient::elements")]
    pub changes: Option<Vec<Change<'a>>>,
    #[serde(default, deserialize_with = "lenient::optional")]
    pub exit_code: Option<i64>,
}
#[derive(Deserialize)]
#[serde(untagged)]
pub(super) enum Command<'a> {
    Text(#[serde(borrow, deserialize_with = "crate::decode::text")] Cow<'a, str>),
    Parts(#[serde(borrow)] Vec<Text<'a>>),
}
#[derive(Deserialize)]
pub(super) struct Text<'a>(
    #[serde(borrow, deserialize_with = "lenient::text")] Option<Cow<'a, str>>,
);
#[derive(Default, Deserialize)]
pub(super) struct Change<'a> {
    #[serde(default, borrow, deserialize_with = "lenient::text")]
    path: Option<Cow<'a, str>>,
}
impl Command<'_> {
    fn summary(&self) -> String {
        match self {
            Self::Text(text) => text.to_string(),
            Self::Parts(parts) => parts
                .iter()
                .filter_map(|part| part.0.as_deref())
                .collect::<Vec<_>>()
                .join(" "),
        }
    }
}
impl Item<'_> {
    pub(super) fn tool(&self) -> Option<(String, Option<String>)> {
        Some(match self.kind.as_deref()? {
            "commandExecution" => (
                "command".into(),
                self.command.as_ref().map(Command::summary),
            ),
            "fileChange" => (
                "file_change".into(),
                self.changes.as_ref().map(|changes| {
                    changes
                        .iter()
                        .filter_map(|change| change.path.as_deref())
                        .collect::<Vec<_>>()
                        .join(", ")
                }),
            ),
            "dynamicToolCall" => (self.tool.as_deref()?.to_owned(), None),
            "mcpToolCall" => (
                format!("{}/{}", self.server.as_deref()?, self.tool.as_deref()?),
                None,
            ),
            _ => return None,
        })
    }
}
impl<'a> Frame<'a> {
    pub(super) fn read(value: &'a Value) -> Self {
        lenient::defaulting(value).unwrap_or_default()
    }
}
impl<'a> Params<'a> {
    pub(super) fn read(value: &'a Value) -> Self {
        lenient::defaulting(value).unwrap_or_default()
    }
}

impl<'a> Method<'a> {
    pub(super) fn read(value: &'a Value) -> Self {
        lenient::defaulting(value).unwrap_or_default()
    }
}
