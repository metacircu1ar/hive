//! Saved output selection keeps existing field semantics without untyped bookkeeping.
use super::invalid_saved_output as invalid;
use crate::error::Result;
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum StoredOutput {
    Text(String),
    Artifact(String),
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Projection<'a> {
    #[serde(borrow, deserialize_with = "crate::decode::field")]
    text: Option<&'a str>,
    #[serde(borrow, deserialize_with = "crate::decode::object_or_default")]
    artifact: Artifact<'a>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Artifact<'a> {
    #[serde(borrow, deserialize_with = "crate::decode::field")]
    id: Option<&'a str>,
}

fn json(encoded: &str) -> Result<Value> {
    serde_json::from_str(encoded).map_err(|cause| invalid().with_source(cause))
}

fn project(value: &Value) -> Projection<'_> {
    crate::decode::object(value).unwrap_or_default()
}

pub(super) fn select(encoded: &str) -> Result<StoredOutput> {
    let value = json(encoded)?;
    let projection = project(&value);
    if let Some(id) = projection.artifact.id {
        Ok(StoredOutput::Artifact(id.to_owned()))
    } else {
        projection
            .text
            .map(|text| StoredOutput::Text(text.to_owned()))
            .ok_or_else(invalid)
    }
}

/// An artifact contains the selected payload, never another reference to follow.
pub(super) fn text(encoded: &str) -> Result<String> {
    let value = json(encoded)?;
    project(&value).text.map(str::to_owned).ok_or_else(invalid)
}

#[cfg(test)]
mod tests;
