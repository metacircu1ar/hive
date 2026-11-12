//! Exact native bindings, independent of the current login and compactable events.
mod cleanup;
mod files;
mod locations;
mod planning;
mod preflight;
mod queries;
use super::{Agent, Store};
use crate::error::{Error, Result};
pub(in crate::runtime) use cleanup::unlink_native_files;
pub(super) use preflight::Planner as NativePlanner;
use rusqlite::params;

impl Store {
    /// Whether any native conversation or unresolved creation belongs to this agent.
    pub(crate) fn has_native_history_or_attempt(&self, agent: &str) -> Result<bool> {
        self.query_row_cached(queries::HAS_HISTORY_OR_ATTEMPT, [agent], |row| row.get(0))
    }

    /// Commit before a request that may create an unknown native conversation.
    pub(crate) fn begin_native_creation(&self, agent: &Agent) -> Result<()> {
        let execution = agent.execution_id.as_deref().ok_or_else(|| {
            Error::database_error("Native session creation has no owning execution.")
        })?;
        self.execute_cached(
            queries::BEGIN_CREATION,
            params![
                agent.id,
                agent.auth_generation.to_string(),
                agent.profile.provider.as_str(),
                execution,
            ],
        )?;
        Ok(())
    }

    /// Only a proven unsubmitted request may clear an unknown creation attempt.
    pub(crate) fn native_creation_not_submitted(&self, agent: &Agent) -> Result<()> {
        self.clear_native_creation(agent)
    }

    fn clear_native_creation(&self, agent: &Agent) -> Result<()> {
        self.execute_cached(
            queries::CLEAR_CREATION,
            params![
                agent.id,
                agent.auth_generation.to_string(),
                agent.profile.provider.as_str(),
                agent.execution_id,
            ],
        )?;
        Ok(())
    }

    /// The acknowledgement is recorded even if later model validation fails.
    pub(crate) fn record_native_binding(&self, agent: &Agent, thread: &str) -> Result<()> {
        self.inherit_branch_native(agent)?;
        self.execute_cached(
            queries::RECORD_BINDING,
            params![
                agent.id,
                agent.auth_generation.to_string(),
                agent.profile.provider.as_str(),
                thread,
            ],
        )?;
        self.clear_native_creation(agent)
    }

    /// Called inside the launch/bind writer transaction, before any source hold release.
    pub(crate) fn inherit_branch_native(&self, child: &Agent) -> Result<()> {
        use crate::{config::Provider, runtime::branching::BranchPhase};
        if child.profile.provider != Provider::Claude {
            return Ok(());
        }
        let Some(intent) = self.branch_intent(&child.id)? else {
            return Ok(());
        };
        if intent.phase != BranchPhase::Forking {
            return Ok(());
        }
        let source = self.agent(&intent.source)?;
        let bound: bool = self.query_row_cached(
            queries::SOURCE_BOUND,
            params![
                source.id,
                child.auth_generation.to_string(),
                intent.source_thread
            ],
            |row| row.get(0),
        )?;
        if source.auth_generation != child.auth_generation
            || source.profile.provider != Provider::Claude
            || source.thread_id.as_deref() != Some(intent.source_thread.as_str())
            || !bound
        {
            return Err(Error::state_inconsistent(
                "Agent branch gate and native capture record disagree.",
            ));
        }
        let generation = child.auth_generation.to_string();
        let args = params![child.id, source.id, generation];
        self.execute_cached(queries::INHERIT_CLAUDE_BINDINGS, args)?;
        self.execute_cached(queries::INHERIT_CLAUDE_PATHS, args)?;
        Ok(())
    }
}
