//! Typed constraints for the deliberately small advertised-schema subset.
//! Opaque navigation and references stay in the original schema; these records
//! reject unknown constraints without interpreting or cloning their payloads.
use crate::decode;
use serde::{
    Deserialize, Deserializer,
    de::{IgnoredAny, SeqAccess, Visitor},
};
use serde_json::Value;
use std::{borrow::Cow, fmt};

#[derive(Default)]
enum Type<'a> {
    #[default]
    Missing,
    Name(Cow<'a, str>),
    Names(Vec<Text<'a>>),
}
#[derive(Deserialize)]
struct Text<'a>(#[serde(borrow, deserialize_with = "optional_text")] Option<Cow<'a, str>>);
impl<'de: 'a, 'a> Deserialize<'de> for Type<'a> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Kind;
        impl<'de> Visitor<'de> for Kind {
            type Value = Type<'de>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a type name or array")
            }
            fn visit_borrowed_str<E: serde::de::Error>(
                self,
                s: &'de str,
            ) -> Result<Self::Value, E> {
                Ok(Type::Name(Cow::Borrowed(s)))
            }
            fn visit_str<E: serde::de::Error>(self, s: &str) -> Result<Self::Value, E> {
                Ok(Type::Name(Cow::Owned(s.to_owned())))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, a: A) -> Result<Self::Value, A::Error> {
                Vec::deserialize(serde::de::value::SeqAccessDeserializer::new(a)).map(Type::Names)
            }
        }
        d.deserialize_any(Kind)
    }
}
impl Type<'_> {
    fn literal(&self, expected: &str) -> bool {
        matches!(self, Self::Name(name) if name == expected)
    }
    fn contains(&self, expected: &str) -> bool {
        self.literal(expected)
            || matches!(self, Self::Names(names)
            if names.iter().any(|name| name.0.as_deref() == Some(expected)))
    }
}
fn optional_text<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Cow<'de, str>>, D::Error> {
    decode::text(d).map(Some).or_else(|_| Ok(None))
}
fn yes() -> bool {
    true
}
fn present<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    IgnoredAny::deserialize(d).map(|_| true)
}
fn choices<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<Text<'de>>>, D::Error> {
    Vec::deserialize(d).map(Some)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Object<'a> {
    #[serde(default, rename = "type", borrow)]
    kind: Type<'a>,
    #[serde(default)]
    properties: IgnoredAny,
    #[serde(default)]
    required: Vec<&'a str>,
    #[serde(default = "yes", rename = "additionalProperties")]
    additional: bool,
    #[serde(default)]
    description: IgnoredAny,
    #[serde(default)]
    title: IgnoredAny,
    #[serde(default)]
    default: IgnoredAny,
}
pub(super) fn object(node: &Value, supplied: &[&str]) -> bool {
    decode::object::<_, Object<'_>>(node).is_ok_and(|shape| {
        let Object {
            kind,
            required,
            additional,
            properties,
            description,
            title,
            default,
        } = shape;
        let _ = (properties, description, title, default);
        kind.literal("object") && additional && required.iter().all(|name| supplied.contains(name))
    })
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FreeObject<'a> {
    #[serde(default, rename = "type", borrow)]
    kind: Type<'a>,
    #[serde(default = "yes", rename = "additionalProperties")]
    additional: bool,
    #[serde(default)]
    description: IgnoredAny,
    #[serde(default)]
    title: IgnoredAny,
    #[serde(default)]
    default: IgnoredAny,
}
pub(super) fn unconstrained(node: &Value) -> bool {
    node == &Value::Bool(true)
        || decode::object::<_, FreeObject<'_>>(node).is_ok_and(|shape| {
            let FreeObject {
                kind,
                additional,
                description,
                title,
                default,
            } = shape;
            let _ = (description, title, default);
            additional && (matches!(kind, Type::Missing) || kind.literal("object"))
        })
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Discriminator<'a> {
    #[serde(default, rename = "type", borrow)]
    kind: Type<'a>,
    #[serde(default, rename = "enum", borrow, deserialize_with = "choices")]
    choices: Option<Vec<Text<'a>>>,
    #[serde(default)]
    description: IgnoredAny,
    #[serde(default)]
    title: IgnoredAny,
    #[serde(default)]
    default: IgnoredAny,
}
pub(super) fn discriminator(node: &Value) -> bool {
    decode::object::<_, Discriminator<'_>>(node).is_ok_and(|shape| {
        let Discriminator {
            kind,
            choices,
            description,
            title,
            default,
        } = shape;
        let _ = (description, title, default);
        kind.literal("string")
            && choices.is_none_or(|values| {
                values
                    .iter()
                    .any(|value| value.0.as_deref() == Some("function"))
            })
    })
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Field<'a> {
    #[serde(default, rename = "type", borrow)]
    kind: Type<'a>,
    #[serde(default, deserialize_with = "present")]
    items: bool,
    #[serde(default)]
    description: IgnoredAny,
    #[serde(default)]
    title: IgnoredAny,
    #[serde(default)]
    default: IgnoredAny,
}
pub(super) fn field(node: &Value, expected: &str) -> bool {
    decode::object::<_, Field<'_>>(node).is_ok_and(|shape| {
        let Field {
            kind,
            items,
            description,
            title,
            default,
        } = shape;
        let _ = (description, title, default);
        (!items || expected == "array") && kind.contains(expected)
    })
}

#[cfg(test)]
mod tests;
