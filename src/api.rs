//! Shared JSON-compatible projection at serialization boundaries.
use serde::Serialize;
use serde_json::Value;

pub(crate) fn value(response: impl Serialize) -> Value {
    #[expect(clippy::expect_used, reason = "Response fields serialize to JSON.")]
    serde_json::to_value(response).expect("runtime responses must be JSON serializable")
}

/// Canonical JSON text, including quotes for strings and recursively sorted object keys.
/// Retain the Value projection: direct serialization can change struct order or f32 text.
pub(crate) fn json_text(response: impl Serialize) -> String {
    crate::api::value(response).to_string()
}

#[cfg(test)]
mod tests;
