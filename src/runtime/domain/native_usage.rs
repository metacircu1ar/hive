//! Bounded session counters. These are snapshots, never account totals or deltas.
use super::ProviderTurnStatus;
use serde::{Deserialize, Deserializer, Serialize, de};
use std::{collections::BTreeMap, fmt, marker::PhantomData};

const MAX_MODELS: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelCounters {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_search_requests: Option<u64>,
}
impl ModelCounters {
    fn decreased_from(&self, old: &Self) -> bool {
        self.input_tokens < old.input_tokens
            || self.output_tokens < old.output_tokens
            || self.cache_read_input_tokens < old.cache_read_input_tokens
            || self.cache_creation_input_tokens < old.cache_creation_input_tokens
            || self
                .web_search_requests
                .zip(old.web_search_requests)
                .is_some_and(|(new, old)| new < old)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ModelUsage(#[serde(deserialize_with = "model_map")] pub BTreeMap<String, ModelCounters>);

pub(crate) fn model_map<'de, D, T>(deserializer: D) -> Result<BTreeMap<String, T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct Models<T>(PhantomData<T>);
    impl<'de, T: Deserialize<'de>> de::Visitor<'de> for Models<T> {
        type Value = BTreeMap<String, T>;
        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a bounded native model counter map")
        }
        fn visit_map<A: de::MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            let mut models = BTreeMap::new();
            while let Some(name) = map.next_key::<String>()? {
                hive_input::validate::model(&name).map_err(de::Error::custom)?;
                if models.len() == MAX_MODELS || models.contains_key(&name) {
                    return Err(de::Error::custom("Invalid native model counter map."));
                }
                models.insert(name, map.next_value()?);
            }
            Ok(models)
        }
    }
    deserializer.deserialize_map(Models(PhantomData))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeUsage {
    pub session_id: String,
    pub turn_id: String,
    pub observed_at: u64,
    pub outcome: ProviderTurnStatus,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    pub reset: Option<bool>,
    pub models: ModelUsage,
}

pub(crate) struct NativeObservation {
    pub session_id: String,
    pub turn_id: String,
    pub outcome: ProviderTurnStatus,
    pub models: ModelUsage,
    pub observed_at: u64,
}

impl NativeUsage {
    pub(crate) fn new(observation: NativeObservation, previous: Option<&Self>) -> Self {
        let NativeObservation {
            session_id,
            turn_id,
            outcome,
            models,
            observed_at,
        } = observation;
        let reset = (outcome == ProviderTurnStatus::Completed).then(|| {
            previous
                .filter(|old| old.session_id == session_id)
                .is_some_and(|old| {
                    old.models.0.iter().any(|(name, old)| {
                        models.0.get(name).is_none_or(|new| new.decreased_from(old))
                    })
                })
        });
        Self {
            session_id,
            turn_id,
            observed_at,
            outcome,
            reset,
            models,
        }
    }
    pub(crate) fn successful(&self) -> bool {
        self.outcome == ProviderTurnStatus::Completed && self.reset.is_some()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeSessionUsage {
    pub agent_id: String,
    #[serde(flatten)]
    pub usage: NativeUsage,
}

#[cfg(test)]
mod tests;
