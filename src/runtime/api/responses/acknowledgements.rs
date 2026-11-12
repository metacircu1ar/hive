//! Closed acknowledgement values; fixed tags cannot disagree with their payload.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ResponseStatus {
    #[cfg(feature = "comb")]
    Updated,
    Stopping,
    Stopped,
    Running,
}
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ControlOperation {
    Stop,
    Resume,
    Interrupt,
}
#[derive(Serialize)]
#[serde(tag = "completion", rename = "pending_turn_boundary")]
pub(crate) struct CompleteResponse<S = String> {
    pub agent_id: S,
}
#[derive(Serialize)]
#[serde(tag = "termination", rename = "requested")]
pub(crate) struct TerminateResponse {
    pub agent_id: String,
}
#[derive(Serialize)]
#[serde(tag = "status", rename = "requested")]
pub(crate) struct ControlResponse {
    pub agent_id: String,
    pub operation: ControlOperation,
}
#[derive(Serialize)]
pub(crate) struct StatusResponse {
    pub status: ResponseStatus,
}
#[derive(Serialize)]
#[serde(tag = "status", rename = "accepted")]
pub(crate) struct SpawnResponse {
    pub agent_id: String,
    pub message_id: String,
}

impl<'de> Deserialize<'de> for SpawnResponse {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(tag = "status", deny_unknown_fields)]
        enum Wire {
            #[serde(rename = "accepted")]
            Accepted {
                agent_id: String,
                message_id: String,
            },
        }
        let Wire::Accepted {
            agent_id,
            message_id,
        } = Wire::deserialize(d)?;
        Ok(Self {
            agent_id,
            message_id,
        })
    }
}
