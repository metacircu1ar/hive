//! Automatic completion uses the latest successful attempt, never an idle guess.
use super::{Agent, AgentState, Core, EventKind, Result, api};
use crate::runtime::{api::events, store::OneShotWaiting};

impl Core {
    pub(crate) fn prepare_one_shot_completion(&self, agent: &mut Agent) -> Result<bool> {
        if !agent.one_shot
            || !crate::runtime::features::available(agent)
            || agent.terminal()
            || agent.paused
            || agent.control.is_some()
            || agent.completion_pending.is_some()
            || !matches!(agent.state, AgentState::Idle | AgentState::Stopped)
            || self.store.one_shot_waiting(agent)? != OneShotWaiting::None
        {
            return Ok(false);
        }
        // Auth admission concerns a future turn. A successful task with no
        // remaining work must finish before a deferred login switch can rebind it.
        let Some(message) = self.store.successful_task_message(agent)? else {
            return Ok(false);
        };
        let (text, selection) = self.store.selected_text(&message)?;
        let preview =
            self.store
                .text_preview(Some(&agent.id), None, &text, api::joins::MEMBER_BYTES)?;
        agent.completion_pending = Some(preview.text);
        agent.completion_pending_by = Some("one_shot".into());
        agent.result_selection = Some(selection);
        agent.result_read_target = preview.read_target;
        self.store.save(agent)?;
        self.store.event_typed(
            agent,
            EventKind::CompletionRequested,
            events::CompletionRequested { by: "one_shot" },
        )?;
        Ok(true)
    }

    /// Run after continuation/control decisions and before commit observation.
    /// Each pass consumes indexed dirty rows; completing a child can dirty its
    /// parent or publish a join result, which the next pass sees in this transaction.
    pub(crate) fn settle_tasks(&mut self) -> Result<()> {
        loop {
            self.store.settle_task_outcomes()?;
            self.store.settle_joins()?;
            let ids = self.store.dirty_tasks()?;
            if ids.is_empty() {
                return Ok(());
            }
            for id in ids {
                self.store.clear_task_dirty(&id)?;
                let mut agent = self.store.agent(&id)?;
                // Explicit completion belongs to the existing worker/scheduler
                // boundaries, including the turn/start RPC with no turn ID yet.
                if self.prepare_one_shot_completion(&mut agent)? {
                    let message = self.store.successful_task_message(&agent)?;
                    self.finish_task_turn(&mut agent, message.as_deref())?;
                }
            }
        }
    }
}
