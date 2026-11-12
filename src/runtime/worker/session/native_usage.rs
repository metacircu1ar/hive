//! Persist only session-bound parent usage; publish account views after commit.
use super::{ExecutionHandle, ParamsRef, Result, Session};
use crate::runtime::provider::error::protocol;
use crate::{
    config::Provider,
    runtime::domain::{EventKind, ModelUsage, NativeSessionUsage, NativeUsage, ProviderTurnStatus},
    state,
};
use serde::Deserialize;
struct Committed {
    auth: String,
    generation: uuid::Uuid,
    source: NativeSessionUsage,
    sequence: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Notice {
    thread_id: String,
    turn_id: String,
    outcome: ProviderTurnStatus,
    models: ModelUsage,
}

pub(super) async fn record(
    execution: ExecutionHandle<'_>,
    rpc: &Session,
    params: ParamsRef<'_>,
) -> Result<()> {
    if rpc.codex() {
        return Err(protocol());
    }
    let notice: Notice =
        crate::decode::object(params.raw()).map_err(|cause| protocol().with_source(cause))?;
    if !notice.outcome.is_terminal() {
        return Err(protocol());
    }
    let committed = execution
        .agent(move |core, agent| record_snapshot(core, agent, notice))
        .await?;
    if let Some(Committed {
        auth,
        generation,
        source,
        sequence,
    }) = committed
    {
        rpc.observe_native_usage(&auth, generation, source, sequence);
    }
    Ok(())
}

fn record_snapshot(
    core: &mut crate::runtime::Core,
    agent: &mut crate::runtime::domain::Agent,
    notice: Notice,
) -> Result<Option<Committed>> {
    if agent.profile.provider != Provider::Claude
        || agent.thread_id.as_deref() != Some(notice.thread_id.as_str())
        || agent.turn_id.as_deref() != Some(notice.turn_id.as_str())
    {
        return Ok(None);
    }
    let usage = NativeUsage::new(
        crate::runtime::domain::NativeObservation {
            session_id: notice.thread_id,
            turn_id: notice.turn_id,
            outcome: notice.outcome,
            models: notice.models,
            observed_at: state::now(),
        },
        agent.native_usage.as_ref(),
    );
    let sequence = core
        .store()
        .event_with_sequence(agent, EventKind::NativeUsage, &usage)?;
    if !usage.successful() {
        return Ok(None);
    }
    agent.native_usage = Some(usage.clone());
    core.store().save(agent)?;
    Ok(Some(Committed {
        auth: agent.auth.clone(),
        generation: agent.auth_generation,
        source: NativeSessionUsage {
            agent_id: agent.id.clone(),
            usage,
        },
        sequence,
    }))
}

#[cfg(test)]
mod tests;
