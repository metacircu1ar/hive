//! Frozen S5 form field selection, with its array display text made explicit.
use super::*;
pub(super) fn fields(schema: &Value) -> Vec<Field> {
    let required: Vec<&str> = schema["required"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let mut fields: Vec<Field> = schema["properties"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(name, property)| {
            let kind = match property["type"].as_str() {
                _ if property["enum"].is_array() => Kind::Choice(
                    property["enum"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|choice| {
                            choice
                                .as_str()
                                .map_or_else(|| choice.to_string(), str::to_owned)
                        })
                        .collect(),
                ),
                Some("boolean") => Kind::Boolean,
                Some("integer") => Kind::Integer,
                Some("number") => Kind::Number,
                Some("array") => Kind::Array(property["items"].to_string()),
                _ => Kind::Text,
            };
            Field {
                name: name.clone(),
                kind,
                required: required.contains(&name.as_str()),
                value: String::new(),
            }
        })
        .collect();
    fields.sort_by_key(|field| !field.required);
    fields
}
