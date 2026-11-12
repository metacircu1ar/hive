//! Authentication cutoffs and native turn-boundary requests.
use super::AuthReason;
use super::{SwitchIntent, WaitingReason};
use crate::runtime::api::events;
use crate::{
    error::Result,
    runtime::{
        Core,
        domain::{Agent, EventKind},
    },
};
use sha2::{Digest, Sha256};

impl Core {
    pub(in crate::runtime) fn request_auth_boundary(
        &self,
        agent: &mut Agent,
        target: String,
        reason: AuthReason,
    ) -> Result<()> {
        if agent.terminal() {
            return Ok(());
        }
        // A source hold delays execution, not the intent. Existing switch
        // admission waits for its release, independently of login availability.
        let previous = agent
            .auth_switch_pending
            .as_ref()
            .filter(|intent| {
                intent.target == target
                    && intent.reason == reason
                    && intent.source_generation == agent.auth_generation
                    && intent.source_execution == agent.execution_id
            })
            .cloned();
        agent.waiting_reason = Some(WaitingReason::Switch);
        agent.quota_wait_until = None;
        agent.auth_switch_pending = Some(SwitchIntent {
            id: previous
                .as_ref()
                .map_or_else(uuid::Uuid::new_v4, |intent| intent.id),
            target,
            reason,
            source_generation: agent.auth_generation,
            source_execution: agent.execution_id.clone(),
            continuation: previous.and_then(|intent| intent.continuation),
        });
        self.store().save(agent)?;
        self.store().deadlines_changed();
        Ok(())
    }

    pub(in crate::runtime) fn auth_uncertain_cutoff(&self, agent: &mut Agent) -> Result<()> {
        if let Some(crate::runtime::domain::AuthCutoff::Bound { turn, message }) =
            agent.auth_cutoff.clone()
        {
            if self.store().cutoff_input(agent, &turn)?.as_deref() == Some(message.as_str()) {
                self.auth_cutoff(agent, &message, &turn)?;
            }
            agent.clear_cutoff();
        }
        Ok(())
    }

    pub(in crate::runtime) fn auth_cutoff(
        &self,
        agent: &mut Agent,
        message: &str,
        turn: &str,
    ) -> Result<()> {
        if (agent.auth_group.is_none() && agent.profile.method != crate::config::Method::ApiKey)
            || agent.terminal()
        {
            return Ok(());
        }
        if agent.auth_switch_pending.is_none() {
            self.request_auth_boundary(
                agent,
                agent
                    .auth_group
                    .clone()
                    .unwrap_or_else(|| agent.auth.clone()),
                AuthReason::UsageLimit,
            )?;
        }
        // Identity is tied to the interrupted input, not subsequent wait/switch decisions.
        let origin = self.store().continuation_origin(&agent.id, message)?;
        let root = &origin.root;
        let attempt = origin.attempt;
        #[expect(clippy::expect_used, reason = "String/count tuple serializes.")]
        let identity =
            serde_json::to_vec(&(&agent.id, &root, attempt)).expect("continuation identity");
        let id = format!("continuation:{:x}", Sha256::digest(identity));
        let body = format!(
            concat!(
                "Your previous turn was cut off by a login limit ",
                "(message {root}, continuation attempt {attempt}). ",
                "Check your messages and the login handoff in your instructions, ",
                "if present, and continue your task. Do not reply to this message."
            ),
            root = root,
            attempt = attempt,
        );
        if self
            .store()
            .enqueue_continuation(&id, &agent.id, &body, &origin)?
        {
            self.store().event_typed(
                agent,
                EventKind::ContinuationQueued,
                events::ContinuationQueued {
                    message_id: &id,
                    interrupted_message_id: message,
                    original_message_id: root,
                    attempt,
                    turn_id: turn,
                },
            )?;
        }
        if let Some(intent) = &mut agent.auth_switch_pending
            && intent.reason == AuthReason::UsageLimit
        {
            intent.continuation = Some(super::CutoffContinuation {
                input: message.to_owned(),
                turn: turn.to_owned(),
                message: id,
            });
        }
        self.store().save(agent)?;
        Ok(())
    }
}
