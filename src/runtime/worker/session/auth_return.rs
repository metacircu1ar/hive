//! One-shot current subscription evidence at the final API input boundary.
use super::{Agent, ExecutionHandle, OptionsSelection, Result, TurnAdmission, models};
use crate::{
    config::Method,
    runtime::{
        Core,
        auth_fallback::{AuthReason, prepare},
    },
};

struct Candidate {
    auth: String,
    generation: uuid::Uuid,
    execution: Option<String>,
    group: String,
    revision: u64,
}

pub(super) async fn claim(
    execution: ExecutionHandle<'_>,
    loaded: OptionsSelection,
    provider_version: Option<String>,
) -> Result<Option<TurnAdmission>> {
    for _ in 0..crate::runtime::preflight::MAX_ATTEMPTS {
        let input = execution.agent(|core, agent| capture(core, agent)).await?;
        let proof = match input {
            Some((candidate, input)) => {
                Some((candidate, execution.shared.preflight.auth(input).await?))
            }
            None => None,
        };
        let tickets = proof.as_ref().map(|(_, prepared)| prepared.tickets());
        let loaded = loaded.clone();
        let provider_version = provider_version.clone();
        let result = execution
            .agent_retained(tickets, move |core, agent| {
                if !apply(core, agent, proof.as_ref())? {
                    return Ok(None);
                }
                models::claim(core, agent, &loaded, provider_version.as_deref()).map(Some)
            })
            .await?;
        if result.is_some() {
            return Ok(result);
        }
    }
    // The caller releases the turn permit and schedules its own bounded retry.
    Ok(None)
}

fn needs_check(core: &Core, agent: &Agent) -> Result<bool> {
    Ok(agent.profile.method == Method::ApiKey
        && agent.auth_group.is_some()
        && agent.auth_switch_pending.is_none()
        && !agent.branch_unbound()
        && !models::claim_blocked(core, agent)
        && core.store().auth_revision()? > agent.auth_checked_revision)
}

fn capture(core: &Core, agent: &Agent) -> Result<Option<(Candidate, prepare::Input)>> {
    if !needs_check(core, agent)? {
        return Ok(None);
    }
    let Some(group) = &agent.auth_group else {
        return Ok(None);
    };
    Ok(Some((
        Candidate {
            auth: agent.auth.clone(),
            generation: agent.auth_generation,
            execution: agent.execution_id.clone(),
            group: group.clone(),
            revision: core.store().auth_revision()?,
        },
        prepare::Input::capture(core, group.clone()),
    )))
}

fn apply(
    core: &Core,
    agent: &mut Agent,
    proof: Option<&(Candidate, prepare::Prepared)>,
) -> Result<bool> {
    if !needs_check(core, agent)? {
        return Ok(true);
    }
    let Some((candidate, prepared)) = proof else {
        return Ok(false);
    };
    if agent.auth != candidate.auth
        || agent.auth_generation != candidate.generation
        || agent.execution_id != candidate.execution
        || agent.auth_group.as_deref() != Some(candidate.group.as_str())
        || core.store().auth_revision()? != candidate.revision
    {
        return Ok(false);
    }
    let mut conclusive = true;
    for member in &prepared.members {
        let binding = match &member.binding {
            Ok(binding) => binding,
            Err(error) => {
                crate::observability::observation_failed("auth_return_binding", error);
                conclusive = false;
                continue;
            }
        };
        if binding.profile.method == Method::Subscription {
            let value = core.store().availability(&member.auth, binding.id)?;
            if value.confirmed_at.is_some() && value.usable(crate::state::now(), None) {
                core.request_auth_boundary(agent, candidate.group.clone(), AuthReason::UsageLimit)?;
                return Ok(true);
            }
        }
    }
    if conclusive {
        agent.auth_checked_revision = candidate.revision;
        core.store().save(agent)?;
    }
    Ok(true)
}

#[cfg(test)]
mod tests;
