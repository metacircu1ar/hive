//! Parent-bound lifetimes end atomically with the parent's durable terminal state.
use super::{
    Agent, AgentState, Caller, Control, Core, Error, EventKind, Lifetime, Result, TaskOutcome,
};
use super::{api, eligible};
use crate::runtime::api::InputContext as Op;
use crate::runtime::api::events;
use crate::runtime::store::CHILD_PAGE_ROWS;
use serde::Serialize;
use uuid::Uuid;

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ParentEnd {
    #[serde(rename = "parent_completed")]
    Completed,
    #[serde(rename = "parent_terminated")]
    Terminated,
    #[serde(rename = "parent_expired")]
    Expired,
}

#[derive(Clone, Copy)]
pub(super) enum TaskEnd<'a> {
    Completed(&'a str, Option<&'a str>),
    Terminated(&'a Caller),
    Expired { parent: &'a str, reason: ParentEnd },
}

impl Core {
    pub(super) fn purge(
        &self,
        request_id: &str,
        args: &crate::runtime::api::PurgeArgs,
    ) -> Result<crate::runtime::api::PurgeResponse> {
        let options = args.options()?;
        self.store.purge(
            request_id,
            args.target
                .required(crate::runtime::api::InputContext::Purge, "/target")?,
            options,
            &self.running,
        )
    }

    pub(super) fn end_task(&mut self, agent: &mut Agent, ending: TaskEnd<'_>) -> Result<bool> {
        let failure = self.task_end_state(agent, ending);
        self.store.save(agent)?;
        if agent.branch_unbound() {
            self.store.fail_branch(agent, failure, false)?;
        }
        // Completion is emitted before mailbox settlement; cancellation and expiry
        // are emitted after it. Keep that public event ordering unchanged.
        if let TaskEnd::Completed(result, turn_id) = ending {
            self.store.event_typed(
                agent,
                EventKind::TaskCompleted,
                events::TaskCompleted {
                    turn_id,
                    result,
                    completed_by: agent.completed_by.as_deref(),
                    result_selection: agent.result_selection,
                    result_read_target: agent.result_read_target.as_deref(),
                },
            )?;
        }
        let joined = self.store.settle_task_end(agent)?;
        self.store.undeliverable(&agent.id)?;
        self.store
            .close_pending_approval(agent, crate::approvals::ApprovalState::Withdrawn)?;
        match ending {
            TaskEnd::Completed(..) => Ok(()),
            TaskEnd::Terminated(caller) => self.store.event_typed(
                agent,
                EventKind::TerminationRequested,
                events::TerminationRequested { by: caller },
            ),
            TaskEnd::Expired { parent, reason } => self.store.event_typed(
                agent,
                EventKind::LifetimeExpired,
                events::LifetimeExpired {
                    parent_id: parent,
                    reason,
                },
            ),
        }?;
        Ok(joined)
    }

    fn task_end_state(
        &self,
        agent: &mut Agent,
        ending: TaskEnd<'_>,
    ) -> crate::runtime::branching::BranchFailure {
        use crate::runtime::branching::BranchFailure;
        let failure = match ending {
            TaskEnd::Completed(result, _) => {
                agent.task_outcome = TaskOutcome::Completed;
                agent.result = Some(result.to_owned());
                BranchFailure::BranchCompletedBeforeMaterialization
            }
            TaskEnd::Terminated(_) => {
                agent.task_outcome = TaskOutcome::Cancelled;
                agent.completion_pending = None;
                agent.completion_pending_by = None;
                agent.control = Some(Control::Stop);
                BranchFailure::BranchTerminated
            }
            TaskEnd::Expired { .. } => {
                agent.expired = true;
                agent.completion_pending = None;
                agent.completion_pending_by = None;
                agent.control = Some(Control::Stop);
                BranchFailure::BranchLifetimeExpired
            }
        };
        if !self.running.contains(&agent.id) {
            agent.state = AgentState::Stopped;
            agent.control = None;
        }
        failure
    }

    pub(in crate::runtime) fn check_lifetime(lifetime: Lifetime, parent: bool) -> Result<Lifetime> {
        if lifetime == Lifetime::WithParent && !parent {
            return Err(Error::parent_required(
                "This lifetime requires an agent parent.",
            ));
        }
        Ok(lifetime)
    }

    pub(super) fn expire_children(&mut self, root: &str, reason: ParentEnd) -> Result<()> {
        let mut pending = vec![(root.to_owned(), reason)];
        while let Some((parent, reason)) = pending.pop() {
            let children = self.store.children(&parent)?;
            // Continue wide parents one page at a time, preserving their reason.
            // Short pages are exhausted inside this single writer transaction.
            if children.len() == CHILD_PAGE_ROWS {
                pending.push((parent.clone(), reason));
            }
            for mut child in children {
                // Each validated row has one parent and a unique identity. Any
                // reachable cycle must contain the root; expiry before expansion
                // prevents repeat visits without retaining the whole subtree.
                if child.id == root {
                    return Err(Error::database_error("Cyclic agent lifetime relationship."));
                }
                self.end_task(
                    &mut child,
                    TaskEnd::Expired {
                        parent: &parent,
                        reason,
                    },
                )?;
                pending.push((child.id, ParentEnd::Expired));
            }
        }
        Ok(())
    }
}

impl Core {
    pub(super) fn complete(
        &mut self,
        caller: &Caller,
        args: &api::CompleteArgs,
    ) -> Result<api::CompleteResponse> {
        let target = if caller.is_human() {
            args.target.required(Op::Complete, "/target")?
        } else {
            caller.name()
        };
        let mut agent = self.store.agent(target)?;
        eligible(&agent)?;
        if agent.completion_pending.is_some() {
            return Err(Error::completion_pending(
                "Completion is already pending the turn boundary.",
            ));
        }
        // An agent's completion would cancel its own collection and expire the
        // with_parent workers it waits for; a human may still decide to.
        if !caller.is_human() && self.store.join_blocks_completion(target)? {
            return Err(Error::join_open(
                "A join you requested is still collecting, or its result is unread. End your \
                    turn; the result arrives as new input. Completing now would cancel the \
                    join and end its with_parent workers. After a join deadline passes and \
                    its partial result is read, completion is allowed.",
            ));
        }
        agent.completion_pending = Some(args.body.required(Op::Complete, "/body")?.to_string());
        agent.completion_pending_by = Some(caller.name().to_owned());
        agent.result_selection = None;
        agent.result_read_target = None;
        self.store.save(&agent)?;
        self.store.event_typed(
            &agent,
            EventKind::CompletionRequested,
            events::CompletionRequested { by: caller.name() },
        )?;
        Ok(api::CompleteResponse {
            agent_id: target.to_owned(),
        })
    }
    pub(super) fn terminate(
        &mut self,
        caller: &Caller,
        args: &api::TargetArgs,
    ) -> Result<api::TerminateResponse> {
        let mut agent = self.store.relative_target(
            caller,
            args.target.required(Op::Terminate, "/target")?,
            Op::Terminate,
        )?;
        eligible(&agent)?;
        self.end_task(&mut agent, TaskEnd::Terminated(caller))?;
        self.expire_children(&agent.id, ParentEnd::Terminated)?;
        Ok(api::TerminateResponse { agent_id: agent.id })
    }
    pub(in crate::runtime) fn finish_task(&mut self, agent: &mut Agent) -> Result<()> {
        self.finish_task_turn(agent, None)
    }
    pub(in crate::runtime) fn finish_task_turn(
        &mut self,
        agent: &mut Agent,
        message: Option<&str>,
    ) -> Result<()> {
        if agent.terminal() {
            if agent.completion_pending.take().is_some() {
                agent.completion_pending_by = None;
                self.store.save(agent)?;
            }
            return Ok(());
        }
        let Some(result) = agent.completion_pending.take() else {
            return Ok(());
        };
        agent.completed_by = agent.completion_pending_by.take();
        let joined = self.store.joined_completion(agent, message, &result)?;
        let turn = message.and_then(|message| {
            agent
                .last_finished_turn
                .as_ref()
                .filter(|turn| turn.message.as_deref() == Some(message))
                .map(|turn| turn.turn.clone())
        });
        let task_joined = self.end_task(agent, TaskEnd::Completed(&result, turn.as_deref()))?;
        if let Some(parent) = &agent.parent_id
            && let Some(parent_agent) = self.store.agent_or_purged(parent)?
            && eligible(&parent_agent).is_ok()
            && !(joined || task_joined)
        {
            let body = if agent.result_selection
                == Some(crate::runtime::domain::OutputSelection::NoText)
            {
                "Child task completed without final text.".to_owned()
            } else {
                let full = self.store.completion_text(agent)?;
                let preview =
                    self.store
                        .text_preview(Some(parent), None, &full, api::joins::MEMBER_BYTES)?;
                completion_notice(&preview.text, preview.read_target.as_deref())
            };
            self.store
                .enqueue(&Uuid::new_v4().to_string(), &agent.id, parent, &body)?;
        }
        self.expire_children(&agent.id, ParentEnd::Completed)
    }
}

fn completion_notice(text: &str, target: Option<&str>) -> String {
    match target {
        Some(target) => format!(
            "Child task completed: {text}\n[Truncated; read full output with hive_read \
            target={target}]"
        ),
        None => format!("Child task completed: {text}"),
    }
}

#[cfg(test)]
mod tests;
