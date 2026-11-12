//! A quota admission lease is independent of native execution and recovery ownership.
use super::{AuthReason, ProbeError};
use super::{Availability, AvailabilityState};
use crate::runtime::api::events;
use crate::{
    error::{Error, Result},
    runtime::{
        Core,
        domain::{Agent, AgentState, MessageState, ProviderEventKind},
    },
};

// Authentication failures are retried after five minutes so repaired login can recover.
const AUTHENTICATION_RETRY_SECONDS: u64 = 300;
// Unknown quota reset times retain the one-hour retry interval.
const QUOTA_RETRY_SECONDS: u64 = 3600;

pub(super) fn retry_seconds(reason: Option<AuthReason>) -> u64 {
    if reason == Some(AuthReason::Authentication) {
        AUTHENTICATION_RETRY_SECONDS
    } else {
        QUOTA_RETRY_SECONDS
    }
}
impl Core {
    /// Caller supplies exact terminal input evidence, or already-verified source cleanup.
    pub(in crate::runtime) fn release_auth_probe(
        &self,
        agent: &mut Agent,
        message: Option<&str>,
    ) -> Result<()> {
        let mut value = self
            .store()
            .availability(&agent.auth, agent.auth_generation)?;
        if value.probe_error().is_none()
            && value.key == agent.execution_key
            && value.probe_id().is_some()
            && value.probe_id() == agent.auth_probe_id.as_deref()
            && value.probe_agent() == Some(&agent.id)
            && message.is_none_or(|id| value.probe_message() == Some(id))
        {
            value.clear_probe();

            self.store().save_revised(&mut value)?;
        }
        agent.auth_probe_id = None;
        Ok(())
    }
    pub(in crate::runtime) fn reconcile_auth_probes(&self, now: u64) -> Result<usize> {
        let candidates = self.store().probe_candidates(now)?;
        let count = candidates.len();
        for mut value in candidates {
            #[expect(clippy::expect_used, reason = "Query requires a complete probe.")]
            let id = value.probe_agent().expect("probe owner");
            if self.running().contains(id) {
                continue;
            }
            let owner = match self.store().agent_or_purged(id) {
                Ok(owner) => owner,
                Err(Error::UnknownAgent { .. }) => {
                    self.invalid_probe(&mut value, ProbeError::MissingOwner)?;
                    continue;
                }
                Err(error) => return Err(error),
            };
            if let Some(owner) = owner {
                if owner.auth != value.profile
                    || owner.auth_generation != value.generation
                    || owner.auth_probe_id.as_deref() != value.probe_id()
                    || owner.execution_key != value.key
                {
                    self.invalid_probe(&mut value, ProbeError::OwnerBindingMismatch)?;
                    continue;
                }
                #[expect(clippy::expect_used, reason = "Query requires a complete probe.")]
                let message = value.probe_message().expect("probe input");
                let input = self.store().probe_input(id, message)?;
                let clean = owner.conversation_clean && owner.recovery_fence.is_none();
                // Terminal compaction may remove input only after legitimate cleanup.
                if input.is_none() && !(owner.terminal() && clean) {
                    self.invalid_probe(&mut value, ProbeError::MissingInput)?;
                    continue;
                }
                let exact = input.is_some_and(|s| {
                    matches!(
                        s,
                        MessageState::Accepted
                            | MessageState::Completed
                            | MessageState::Failed
                            | MessageState::Interrupted
                            | MessageState::Undeliverable
                    )
                });
                if !exact && !clean && value.probe_deadline().is_none_or(|at| at > now) {
                    continue;
                }
                // Expiry relaxes only quota admission: never clear a fence, change
                // delivery uncertainty, or release physical provider capacity.
            }
            value.clear_probe();

            self.store().save_revised(&mut value)?;
        }
        Ok(count)
    }
    fn invalid_probe(&self, value: &mut Availability, reason: ProbeError) -> Result<()> {
        #[expect(clippy::expect_used, reason = "Caller checked complete probe.")]
        let probe = value.probe.as_mut().expect("probe candidate");
        probe.error = Some(reason);

        self.store().save_revised(value)?;
        self.store().auth_event(
            &value.profile,
            value.generation,
            ProviderEventKind::LoginProbeInvalid,
            events::LoginProbeInvalid {
                profile: &value.profile,
                generation: value.generation,
                probe_id: value.probe_id(),
                reason,
            },
        )
    }
    pub(in crate::runtime) fn recover_auth_cutoffs(&mut self) -> Result<()> {
        let mut after = String::new();
        loop {
            let ids = self.store().cutoff_agents(&after)?;
            let Some(last) = ids.last() else {
                return Ok(());
            };
            after.clone_from(last);
            self.atomic(move |core| {
                for id in ids {
                    let mut agent = core.store().agent(&id)?;
                    core.auth_uncertain_cutoff(&mut agent)?;
                    core.store().save(&agent)?;
                }
                Ok(())
            })?;
        }
    }
    /// Same-binding recovery does not need catalog discovery or a new conversation.
    pub(in crate::runtime) fn unpark_auth_waiter(&self, agent: &mut Agent) -> Result<bool> {
        if self.running().contains(&agent.id)
            || agent.recovery_fence.is_some()
            || (!agent.conversation_clean && agent.native_input_attempted)
            || agent
                .auth_switch_pending
                .as_ref()
                .is_some_and(|intent| intent.reason == AuthReason::Manual)
        {
            return Ok(false);
        }
        let value = self
            .store()
            .availability(&agent.auth, agent.auth_generation)?;
        if value.state != AvailabilityState::Available
            || !value.usable(crate::state::now(), Some(&agent.id))
        {
            if agent.branch_unbound() {
                agent.auth_checked_revision = self.store().auth_revision()?;
                agent.quota_wait_until = value.deadline();
                self.store().save(agent)?;
            }
            return Ok(false);
        }
        agent.clear_auth_wait(self.store().auth_revision()?);
        // A waiting agent that resumes on its own login has kept that login.
        if let Some(decision) = &mut agent.auth_decision
            && decision.reason == super::AuthChoice::Waiting
        {
            decision.reason = super::AuthChoice::Sticky;
        }
        agent.state = AgentState::Stopped;
        agent.error = None;
        self.store().save(agent)?;
        Ok(true)
    }
}
