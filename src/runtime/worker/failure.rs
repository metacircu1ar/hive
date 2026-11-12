//! Failure for this component.
use super::{
    Agent, AgentState, BranchFailure, BranchPhase, Core, EXECUTION_RETRY_DELAY, Error, EventKind,
    RecoveryFence, Result, Shared, events,
};

pub(super) async fn finish_failure(
    shared: &Shared,
    id: String,
    error: Error,
    cleaned: bool,
    reserved: Option<String>,
) {
    let original = error.clone();
    let agent_id = id.clone();
    let recorded = shared
        .with(move |core| record_failure(core, &id, error, cleaned, reserved.as_deref()))
        .await;
    match recorded {
        Ok(true) => {
            tracing::debug!(name: "worker.execution_retry",
                 agent_id = agent_id.as_str(),
                 code = original.code(),
                 error = &original as &(dyn std::error::Error + 'static),
                 confirmed = cleaned,
                 decision = "retry_unsubmitted");
            // Cleanup released provider/turn permits; retain the worker claim briefly
            // so a safely rejected pre-exec branch cannot spin repeated launches.
            tokio::time::sleep(EXECUTION_RETRY_DELAY).await;
        }
        Ok(false) => {
            execution_failed(&agent_id, &original, cleaned);
        }
        Err(secondary) => {
            execution_failed(&agent_id, &original, cleaned);
            shared.stop_after_failure(&secondary);
            tracing::error!(name: "worker.failure_record_failed",
                 agent_id = agent_id.as_str(),
                 code = secondary.code(),
                 primary = &original as &(dyn std::error::Error + 'static),
                 secondary = &secondary as &(dyn std::error::Error + 'static),
                 sqlite_code = super::super::store::sqlite_code(&secondary),
                 decision = "shutdown_retain_evidence");
        }
    }
}

fn execution_failed(agent: &str, error: &Error, cleaned: bool) {
    tracing::error!(name: "worker.execution_failed",
         agent_id = agent,
         code = error.code(),
         error = error as &(dyn std::error::Error + 'static),
         confirmed = cleaned,
         decision = "record_failure");
}

pub(super) fn record_failure(
    core: &mut Core,
    id: &str,
    error: Error,
    cleaned: bool,
    reserved: Option<&str>,
) -> Result<bool> {
    let mut agent = core.store().agent(id)?;
    mark_uncreated_clean(core, &mut agent, &error, cleaned, reserved)?;
    if retryable_failure(core, &mut agent, &error, cleaned, reserved)? {
        return Ok(true);
    }
    if !cleaned || matches!(error, Error::RecoveryRequired { .. }) {
        agent.recovery_fence = Some(RecoveryFence::Failure(error.to_string()));
    }
    agent.state = if cleaned && agent.terminal() {
        AgentState::Stopped
    } else {
        AgentState::Failed
    };
    agent.error = Some(error.code().into());
    agent.disable_steering();
    core.auth_uncertain_cutoff(&mut agent)?;
    let failed_turn = agent.turn_id.clone();
    agent.clear_turn();
    agent.control = None;
    agent.completion_pending = None;
    agent.completion_pending_by = None;
    core.store().uncertain(id)?;
    core.store().save(&agent)?;
    if agent.branch_unbound() {
        core.store()
            .fail_branch(&mut agent, BranchFailure::Error(error.kind()), false)?;
    }
    core.store().event_typed(
        &agent,
        EventKind::ExecutionFailed,
        events::ExecutionFailure {
            error: &error,
            turn_id: failed_turn.as_deref(),
        },
    )?;
    Ok(false)
}

fn mark_uncreated_clean(
    core: &Core,
    agent: &mut Agent,
    error: &Error,
    cleaned: bool,
    reserved: Option<&str>,
) -> Result<()> {
    // Claude reserves its thread name at spawn, then records its provider before
    // native exec. Codex records a creation attempt before its first thread RPC.
    let uncreated = match agent.profile.provider {
        crate::config::Provider::Codex => agent.thread_id.is_none(),
        crate::config::Provider::Claude => agent.provider_process.is_none(),
    };
    if cleaned
        && !matches!(error, Error::RecoveryRequired { .. })
        && agent.recovery_fence.is_none()
        && reserved.is_some_and(|execution| agent.execution_id.as_deref() == Some(execution))
        && uncreated
        && !agent.native_input_attempted
        && !core.store().has_native_history_or_attempt(&agent.id)?
    {
        agent.conversation_clean = true;
    }
    Ok(())
}

fn retryable_failure(
    core: &Core,
    agent: &mut Agent,
    error: &Error,
    cleaned: bool,
    reserved: Option<&str>,
) -> Result<bool> {
    if cleaned
        && crate::state::is_key_changed(error)
        && !agent.terminal()
        && !agent.paused
        && agent.control.is_none()
    {
        agent.state = AgentState::Stopped;
        agent.error = None;
        core.store().save(agent)?;
        if reserved.is_some_and(|execution| agent.execution_id.as_deref() == Some(execution)) {
            core.store().event_typed(
                agent,
                EventKind::ExecutionStopped,
                events::ExecutionStopped::Unsubmitted {
                    reason: "credential_changed_before_submission",
                    error,
                },
            )?;
        }
        return Ok(true);
    }
    if cleaned
        && error.input_was_not_submitted()
        && agent.branch_phase == Some(BranchPhase::ReadyToFork)
        && agent.recovery_fence.is_none()
        && !agent.terminal()
        && !agent.paused
        && agent.control.is_none()
    {
        agent.state = AgentState::Stopped;
        agent.error = None;
        core.store().save(agent)?;
        core.store().event_typed(
            agent,
            EventKind::ExecutionStopped,
            events::ExecutionStopped::Unsubmitted {
                reason: "branch_creation_not_submitted",
                error,
            },
        )?;
        return Ok(true);
    }
    Ok(false)
}
