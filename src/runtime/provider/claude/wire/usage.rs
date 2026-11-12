//! Native camel-case usage accepts only counters; cost fields are discarded.
use crate::runtime::domain::{ModelCounters, ModelUsage, native_usage::model_map};
use serde::{Deserialize, Deserializer};
use std::collections::BTreeMap;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Counters {
    input_tokens: u64,
    output_tokens: u64,
    cache_read_input_tokens: u64,
    cache_creation_input_tokens: u64,
    #[serde(default)]
    web_search_requests: Option<u64>,
}
#[derive(Deserialize)]
struct Models(#[serde(deserialize_with = "model_map")] BTreeMap<String, Counters>);
pub(super) fn optional<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<ModelUsage>, D::Error> {
    let raw = serde_json::Value::deserialize(deserializer)?;
    // Invalid optional metadata is rejected in full, independently of turn validity.
    let Ok(models) = Models::deserialize(raw) else {
        return Ok(None);
    };
    Ok(Some(ModelUsage(
        models
            .0
            .into_iter()
            .map(|(name, native)| {
                (
                    name,
                    ModelCounters {
                        input_tokens: native.input_tokens,
                        output_tokens: native.output_tokens,
                        cache_read_input_tokens: native.cache_read_input_tokens,
                        cache_creation_input_tokens: native.cache_creation_input_tokens,
                        web_search_requests: native.web_search_requests,
                    },
                )
            })
            .collect(),
    )))
}
