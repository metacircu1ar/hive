//! Native Agent arguments cannot introduce teammates, isolation or background work.
use super::super::{Codec, Result, protocol};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

const AGENT_TYPES: usize = 256;
pub(in crate::runtime::provider::claude::codec) const AGENT_REASON: &str = "Native agents \
    accept only description, prompt, subagent_type, model and \
    run_in_background:false. Use Hive for visible delegation; named, nested, \
    isolated and background agents are unavailable.";

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AgentInput {
    description: String,
    prompt: String,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    subagent_type: Option<String>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    model: Option<String>,
    #[serde(default)]
    run_in_background: bool,
}

fn present<'de, D, T>(deserializer: D) -> std::result::Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

impl Codec {
    pub(in crate::runtime::provider::claude::codec) fn native_agent_input(
        &self,
        input: &Value,
    ) -> Result<Option<Value>> {
        // A typed policy denial is the model-visible outcome, not a provider failure.
        let Ok(args) = serde_json::from_value::<AgentInput>(input.clone()) else {
            return Ok(None);
        };
        let init = self.initialization.as_ref().ok_or_else(protocol)?;
        let kind = args.subagent_type.as_deref().unwrap_or("general-purpose");
        if args.run_in_background
            || args.prompt.trim().is_empty()
            || args.description.trim().is_empty()
            || init.agents.len() > AGENT_TYPES
            || !init.agents.iter().any(|agent| agent.name == kind)
            || args.model.as_ref().is_some_and(|model| {
                !init
                    .models
                    .iter()
                    .any(|entry| &entry.value == model || &entry.resolved == model)
            })
        {
            return Ok(None);
        }
        // Normalize the foreground choice explicitly; the native environment also
        // disables background tasks. The final native executable fixture checks both.
        serde_json::to_value(args)
            .map(Some)
            .map_err(|cause| protocol().with_source(cause))
    }

    pub(in crate::runtime::provider::claude::codec) fn child_model(
        &self,
        model: &str,
    ) -> Result<()> {
        let init = self.initialization.as_ref().ok_or_else(protocol)?;
        if super::super::wire::ModelKind::parse(model) == super::super::wire::ModelKind::Synthetic
            || init.models.iter().any(|entry| {
                entry.resolved.trim_end_matches("[1m]") == model.trim_end_matches("[1m]")
            })
        {
            return Ok(());
        }
        Err(crate::error::Error::model_mismatch(
            "Claude selected a different model.",
        ))
    }
}
