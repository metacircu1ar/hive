//! Header validation retains exact strings while the message body stays opaque.
use super::{Result, refused};
use crate::decode::{field, object_or_default};
use serde::{
    Deserialize, Deserializer,
    de::{IgnoredAny, SeqAccess, Visitor},
};
use serde_json::Value;
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    User,
    Assistant,
}
impl Kind {
    pub(super) fn read(raw: Option<&str>) -> Option<Self> {
        match raw {
            Some("user") => Some(Self::User),
            Some("assistant") => Some(Self::Assistant),
            _ => None,
        }
    }
}

#[derive(Default)]
pub(super) enum Parent<'a> {
    #[default]
    Missing,
    Root,
    Id(&'a str),
    Invalid,
}
fn parent<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Parent<'de>, D::Error> {
    Option::<&str>::deserialize(d)
        .map(|value| value.map_or(Parent::Root, Parent::Id))
        .or(Ok(Parent::Invalid))
}

#[derive(Default, Deserialize)]
#[serde(default)]
pub(super) struct Header<'a> {
    #[serde(rename = "isSidechain")]
    pub sidechain: bool,
    #[serde(rename = "type", borrow, deserialize_with = "field")]
    pub kind: Option<&'a str>,
    #[serde(borrow, deserialize_with = "field")]
    uuid: Option<&'a str>,
    #[serde(rename = "parentUuid", borrow, deserialize_with = "parent")]
    parent: Parent<'a>,
    #[serde(borrow, deserialize_with = "object_or_default")]
    message: Message<'a>,
}
impl<'a> Header<'a> {
    pub(super) fn read(value: &'a Value) -> Result<Self> {
        match value {
            Value::Object(_) => crate::decode::object(value).map_err(|_| refused()),
            _ => Ok(Self::default()),
        }
    }
    pub(super) fn id(&self) -> Option<&'a str> {
        self.uuid
    }
    pub(super) fn identity(&self) -> Result<(&'a str, Option<&'a str>)> {
        let id = self.uuid.ok_or_else(refused)?;
        uuid::Uuid::parse_str(id).map_err(|cause| refused().with_source(cause))?;
        let parent = match self.parent {
            Parent::Root => None,
            Parent::Id(parent) => {
                uuid::Uuid::parse_str(parent).map_err(|cause| refused().with_source(cause))?;
                Some(parent)
            }
            Parent::Missing | Parent::Invalid => return Err(refused()),
        };
        Ok((id, parent))
    }
    pub(super) fn valid_message(&self) -> bool {
        self.message.role == self.kind && self.message.content.0
    }
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Message<'a> {
    #[serde(borrow, deserialize_with = "field")]
    role: Option<&'a str>,
    #[serde(deserialize_with = "field")]
    content: Content,
}

#[derive(Default)]
struct Content(bool);
impl<'de> Deserialize<'de> for Content {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct Shape;
        impl<'de> Visitor<'de> for Shape {
            type Value = Content;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("string or array")
            }
            fn visit_str<E: serde::de::Error>(self, _: &str) -> std::result::Result<Content, E> {
                Ok(Content(true))
            }
            fn visit_seq<S: SeqAccess<'de>>(
                self,
                mut seq: S,
            ) -> std::result::Result<Content, S::Error> {
                while seq.next_element::<IgnoredAny>()?.is_some() {}
                Ok(Content(true))
            }
        }
        d.deserialize_any(Shape)
    }
}
