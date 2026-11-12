//! Durable latest-attempt observations shared by completion, joins and waiters.
mod output;
mod queries;
use super::{Agent, Queries, Store};
use crate::{
    error::{Error, Result},
    runtime::domain::{AgentState, AuthCutoff, Control, OutputSelection},
};
use rusqlite::params;

crate::runtime::domain::domain_enum!(AutomaticContinuation {
    Runnable => "runnable", Waiting => "waiting", None => "none",
});
crate::runtime::domain::domain_enum!(OneShotWaiting {
    Question => "question", Join => "join", Children => "children",
    Branch => "branch", Input => "input", None => "none",
});

pub(super) fn invalid_saved_output() -> Error {
    Error::database_error("Invalid saved message output.")
}

pub(super) struct TaskTurn {
    pub message: Option<String>,
    pub thread: Option<String>,
    pub outcome: Option<String>,
}

impl Queries {
    pub(crate) fn one_shot_waiting(&self, agent: &Agent) -> Result<OneShotWaiting> {
        if !agent.one_shot || agent.terminal() {
            return Ok(OneShotWaiting::None);
        }
        self.query_row_cached(queries::WAITING, [&agent.id], |r| r.get(0))
    }
    pub(crate) fn automatic_continuation(&self, agent: &Agent) -> Result<AutomaticContinuation> {
        use AutomaticContinuation::{None, Runnable, Waiting};
        let (blocked, claimed, pending, scheduled): (bool, bool, bool, bool) = self
            .query_row_cached(queries::ADMISSION, [&agent.id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })?;
        if automatic_disabled(agent, blocked) {
            return Ok(None);
        }
        // Startup first commits uncertainty and only then reconciles this exact
        // persisted cutoff. It must remain armed across that transaction boundary.
        if let Some(AuthCutoff::Bound { turn, message }) = &agent.auth_cutoff
            && (agent.auth_group.is_some() || agent.profile.method == crate::config::Method::ApiKey)
            && self.cutoff_input(agent, turn)?.as_deref() == Some(message.as_str())
        {
            return Ok(Waiting);
        }
        // A dirty/live session delays same-binding unpark, but the auth service
        // can arm a switch and retry after release. It is still automatic recovery.
        let quota_path =
            agent.waiting_reason == Some(crate::runtime::auth_fallback::WaitingReason::Quota);
        if (agent.auth_switch_pending.is_some() || quota_path) && agent.recovery_fence.is_none() {
            return Ok(Waiting);
        }
        if agent.state == AgentState::Failed || agent.waiting_reason.is_some() {
            return Ok(None);
        }
        if !(pending || scheduled) {
            return Ok(None);
        }
        if agent.branch_hold > 0
            || matches!(
                agent.branch_phase,
                Some(
                    crate::runtime::branching::BranchPhase::WaitingForSource
                        | crate::runtime::branching::BranchPhase::Forking
                )
            )
        {
            return Ok(Waiting);
        }
        if agent.control.is_some() {
            return Ok(None);
        }
        let admitted = matches!(
            agent.state,
            AgentState::Starting | AgentState::Idle | AgentState::Stopped
        ) || claimed && agent.state == AgentState::Running;
        Ok(if !admitted {
            None
        } else if scheduled && !pending {
            Waiting
        } else {
            Runnable
        })
    }
    pub(super) fn task_turn(&self, id: &str) -> Result<Option<TaskTurn>> {
        self.query_opt_cached(queries::TURN, [id], |r| {
            Ok(TaskTurn {
                message: r.get(0)?,
                thread: r.get(1)?,
                outcome: r.get(2)?,
            })
        })
    }
    pub(crate) fn successful_task_message(&self, agent: &Agent) -> Result<Option<String>> {
        let Some(turn) = self.task_turn(&agent.id)? else {
            return Ok(None);
        };
        if turn.outcome.as_deref() != Some("completed")
            || turn.thread != agent.thread_id
            || turn.thread.is_none()
            || agent.reserved_turn.is_some()
            || agent.turn_id.is_some()
        {
            return Ok(None);
        }
        if agent.last_finished_turn.as_ref().is_none_or(|finished| {
            finished.message != turn.message
                || Some(&finished.thread) != agent.thread_id.as_ref()
                || finished.outcome != crate::runtime::domain::ProviderTurnStatus::Completed
        }) {
            return Ok(None);
        }
        Ok(turn.message)
    }
    pub(crate) fn selected_text(&self, message: &str) -> Result<(String, OutputSelection)> {
        let Some((raw, final_answer)) = self.query_opt_cached(queries::OUTPUT, [message], |r| {
            Ok((r.get::<_, Option<String>>(0)?, r.get::<_, bool>(1)?))
        })?
        else {
            return Err(invalid_saved_output());
        };
        let Some(raw) = raw else {
            return Ok((String::new(), OutputSelection::NoText));
        };
        let text = match output::select(&raw)? {
            output::StoredOutput::Artifact(id) => self.task_artifact(&id)?,
            output::StoredOutput::Text(text) => text,
        };
        let selection = if text.is_empty() {
            OutputSelection::NoText
        } else if final_answer {
            OutputSelection::FinalAnswer
        } else {
            OutputSelection::LastAssistantMessage
        };
        Ok((text, selection))
    }
    pub(crate) fn completion_text(&self, agent: &Agent) -> Result<String> {
        let Some(target) = &agent.result_read_target else {
            return Ok(agent.result.clone().unwrap_or_default());
        };
        let id = crate::runtime::domain::ReadTarget::parse(target)
            .artifact()
            .ok_or_else(invalid_saved_output)?;
        self.task_artifact(id)
    }
    fn task_artifact(&self, id: &str) -> Result<String> {
        let mut encoded = String::new();
        let mut offset = 0;
        loop {
            let page = self.output(id, offset, 65_536)?;
            encoded.push_str(&page.text);
            if page.eof {
                break;
            }
            offset = page.next_offset;
        }
        output::text(&encoded)
    }
}

impl Store {
    pub(crate) fn reconcile_cutoff_intents(&self) -> Result<()> {
        loop {
            let ids: Vec<String> = self
                .sql()
                .cached()
                .query_column(queries::DISARMED_CUTOFFS, [])?;
            if ids.is_empty() {
                return Ok(());
            }
            for id in ids {
                let mut agent = self.agent(&id)?;
                agent.clear_auth_wait(agent.auth_checked_revision);
                self.save(&agent)?;
                self.deadlines_changed();
            }
        }
    }
    pub(super) fn track_task_input(&self, message: &str, agent: &str) -> Result<()> {
        self.execute_cached(
            queries::TRACK,
            params![message, agent, super::convert::now()?],
        )?;
        Ok(())
    }
    pub(crate) fn dirty_tasks(&self) -> Result<Vec<String>> {
        self.sql().cached().query_column(queries::DIRTY, [])
    }
    pub(crate) fn clear_task_dirty(&self, id: &str) -> Result<()> {
        self.execute_cached(queries::CLEAR_DIRTY, [id])?;
        Ok(())
    }
    pub(crate) fn settle_task_outcomes(&self) -> Result<()> {
        use crate::runtime::api::joins::JoinOutcome;
        self.reconcile_cutoff_intents()?;
        loop {
            let ids: Vec<String> = self
                .sql()
                .cached()
                .query_column(queries::OUTCOME_DIRTY, [])?;
            if ids.is_empty() {
                return Ok(());
            }
            for id in ids {
                self.execute_cached(queries::CLEAR_OUTCOME_DIRTY, [&id])?;
                let agent = self.agent(&id)?;
                if agent.terminal() {
                    self.settle_task_end(&agent)?;
                    continue;
                }
                // Joined output was independently snapshotted at settlement. A
                // settled/cancelled/purged requester no longer needs this source.
                if !self.has_task_join(&id)? {
                    continue;
                }
                let Some(turn) = self.task_turn(&id)? else {
                    continue;
                };
                let outcome = match turn.outcome.as_deref() {
                    Some("failed") => JoinOutcome::Failed,
                    Some("interrupted") => JoinOutcome::Interrupted,
                    Some("uncertain") => JoinOutcome::Uncertain,
                    Some("undeliverable") => JoinOutcome::Undeliverable,
                    _ => continue,
                };
                if self.automatic_continuation(&agent)? != AutomaticContinuation::None {
                    continue;
                }
                let (text, selection) = match turn.message {
                    Some(message) => self.selected_text(&message)?,
                    None => (String::new(), OutputSelection::NoText),
                };
                self.task_join_output(&agent, outcome, &text, Some(selection))?;
            }
        }
    }
}

fn automatic_disabled(agent: &Agent, blocked: bool) -> bool {
    if agent.terminal()
        || !crate::runtime::features::available(agent)
        || agent.paused
        || blocked
        || agent.completion_pending.is_some()
        || agent.control == Some(Control::Stop)
    {
        return true;
    }
    if matches!(
        agent.branch_phase,
        Some(
            crate::runtime::branching::BranchPhase::Failed
                | crate::runtime::branching::BranchPhase::Uncertain
        )
    ) {
        return true;
    }
    false
}
