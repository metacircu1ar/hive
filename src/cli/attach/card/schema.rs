//! Lenient presentation of response schemas; approval validation stays authoritative.
use super::{Field, Kind};
use crate::cli::views;
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Default, Deserialize)]
struct Schema<'a> {
    #[serde(default, borrow, deserialize_with = "views::field")]
    required: Vec<Name<'a>>,
    #[serde(default, borrow, deserialize_with = "views::field")]
    properties: BTreeMap<&'a str, Property<'a>>,
}
#[derive(Deserialize)]
struct Name<'a>(#[serde(borrow, deserialize_with = "views::field")] Option<&'a str>);
#[derive(Default, Deserialize)]
struct PropertyFields<'a> {
    #[serde(default, rename = "type", borrow, deserialize_with = "views::field")]
    kind: Option<&'a str>,
    #[serde(default, rename = "enum", deserialize_with = "views::field")]
    choices: Option<Vec<Value>>,
    #[serde(default)]
    items: Value,
}
struct Property<'a>(PropertyFields<'a>);
impl<'de: 'a, 'a> Deserialize<'de> for Property<'a> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        views::object(d).map(Self)
    }
}
pub(super) fn fields(raw: &Value) -> Vec<Field> {
    let schema: Schema<'_> = views::read(raw);
    let mut fields: Vec<_> = schema
        .properties
        .into_iter()
        .map(|(name, Property(property))| {
            let kind = match (property.kind, property.choices) {
                (_, Some(choices)) => Kind::Choice(
                    choices
                        .into_iter()
                        .map(|choice| {
                            choice
                                .as_str()
                                .map_or_else(|| choice.to_string(), str::to_owned)
                        })
                        .collect(),
                ),
                (Some("boolean"), _) => Kind::Boolean,
                (Some("integer"), _) => Kind::Integer,
                (Some("number"), _) => Kind::Number,
                (Some("array"), _) => Kind::Array(property.items.to_string()),
                _ => Kind::Text,
            };
            Field {
                name: name.to_owned(),
                kind,
                required: schema
                    .required
                    .iter()
                    .any(|required| required.0 == Some(name)),
                value: String::new(),
            }
        })
        .collect();
    fields.sort_by_key(|field| !field.required);
    fields
}
