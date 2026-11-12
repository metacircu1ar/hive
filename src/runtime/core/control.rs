//! Model selection and runtime control transitions.
use super::preflight;
use super::{
    AgentState, Caller, Control, Core, Error, EventKind, Facts, Result, eligible, invalid_request,
};
use crate::runtime::api;
use crate::runtime::api::InputContext as Op;
use crate::runtime::api::events;

use crate::runtime::api::ControlOperation;

impl ControlOperation {
    fn input_context(self) -> Op {
        match self {
            Self::Stop => Op::Stop,
            Self::Resume => Op::Resume,
            Self::Interrupt => Op::Interrupt,
        }
    }
}

impl Core {
    pub(super) fn set_options(
        &mut self,
        caller: &Caller,
        args: &api::SetOptionsArgs,
        facts: &Facts,
    ) -> Result<api::OptionsChangedResponse> {
        let mut agent = self
            .store
            .agent(args.target.required(Op::SetOptions, "/target")?)?;
        eligible(&agent)?;
        let options = args.selection(&agent)?;
        let snapshot = options.desired();
        let catalog = facts
            .model_catalog
            .as_ref()
            .ok_or_else(preflight::missing)?;
        args.check_catalog(&snapshot, agent.profile.provider, catalog)?;
        let model = snapshot
            .string("model")
            .ok_or_else(invalid_request)?
            .to_owned();
        let effort = snapshot
            .string("effort")
            .ok_or_else(invalid_request)?
            .to_owned();
        let previous = agent.desired_selection();
        // An explicit correction after a failed pre-send attempt may replace its
        // reservation. Accepted is the proof that this input was never submitted;
        // clean model relaunches while Starting/Stopped preserve their reservation.
        if agent.state == AgentState::Failed && self.store.reservation_accepted(&agent)? {
            agent.reserved_turn = None;
        }
        agent.agent_options = options;
        agent.model = model;
        agent.effort = effort;
        agent.model_key.clone_from(&facts.model_key);
        agent.options_revision = agent
            .options_revision
            .checked_add(1)
            .ok_or_else(invalid_request)?;
        if !self
            .store
            .admits_process_options(&agent, self.config.runtime.max_process_variants)?
        {
            return Err(Op::SetOptions.error(
                hive_input::Code::InvalidValue,
                "/options",
                "process settings within runtime.max_process_variants for this provider and login",
            ));
        }
        self.publish_options(caller, agent, previous)
    }

    fn publish_options(
        &self,
        caller: &Caller,
        agent: super::Agent,
        previous: crate::runtime::domain::OptionsSelection,
    ) -> Result<api::OptionsChangedResponse> {
        let desired = agent.desired_selection();
        let takes_effect = option_effects(agent.profile.provider, &previous, &desired)?;
        self.store.save(&agent)?;
        let previous_public = previous.public();
        let desired_public = desired.clone().public();
        self.store.event_typed(
            &agent,
            EventKind::OptionsChanged,
            events::OptionsChanged {
                by: caller,
                previous: &previous_public,
                desired: &desired_public,
                takes_effect: &takes_effect,
            },
        )?;
        Ok(api::OptionsChangedResponse {
            agent_id: agent.id,
            desired: desired_public,
            takes_effect,
        })
    }
    /// Change or clear a display name at once, ended agents included: a name
    /// is a label, so no turn boundary applies.
    pub(super) fn set_name(&mut self, args: &api::SetNameArgs) -> Result<api::NameChangedResponse> {
        let mut agent = self
            .store
            .agent(args.target.required(Op::SetName, "/target")?)?;
        let name = args.name()?;
        agent.name = name.map(str::to_owned);
        self.store.save(&agent)?;
        Ok(api::NameChangedResponse {
            agent_id: agent.id,
            name: agent.name,
        })
    }
    pub(super) fn control(
        &mut self,
        caller: &Caller,
        args: &api::TargetArgs,
        operation: ControlOperation,
        facts: &Facts,
    ) -> Result<api::ControlResponse> {
        let mut agent = self
            .store
            .agent(args.target.required(operation.input_context(), "/target")?)?;
        eligible(&agent)?;
        match operation {
            ControlOperation::Stop => {
                agent.paused = true;
                agent.control = Some(Control::Stop);
                if agent.branch_unbound() {
                    self.store.fail_branch(
                        &mut agent,
                        crate::runtime::branching::BranchFailure::BranchStopped,
                        false,
                    )?;
                }
            }
            ControlOperation::Resume => self.resume_agent(&mut agent, facts)?,
            ControlOperation::Interrupt => {
                if agent.turn_id.is_none() {
                    return Err(Error::no_active_turn("No active turn to interrupt."));
                }
                agent.control = Some(Control::Interrupt);
            }
        }
        self.store.save(&agent)?;
        self.store.event_typed(
            &agent,
            EventKind::ControlRequested,
            events::ControlRequested {
                operation,
                by: caller,
            },
        )?;
        Ok(api::ControlResponse {
            agent_id: agent.id,
            operation,
        })
    }
    fn resume_agent(&mut self, agent: &mut super::Agent, facts: &Facts) -> Result<()> {
        crate::runtime::features::require(agent)?;
        if agent.branch_unbound() {
            return Err(Error::agent_busy(concat!(
                "An unresolved or cancelled branch cannot resume as a fresh ",
                "conversation. Inspect branch status and terminate it if necessary.",
            )));
        }
        if agent.control.is_some()
            || matches!(
                agent.state,
                AgentState::Running | AgentState::Starting | AgentState::Stopping
            )
        {
            return Err(Error::agent_busy(
                "Wait for the current transition before resuming.",
            ));
        }
        if self.running.contains(&agent.id)
            && (agent.state == AgentState::Failed
                || agent.state == AgentState::Stopped && !agent.conversation_clean)
        {
            return Err(Error::agent_releasing(
                "The prior execution is still finishing cleanup; retry resume after it \
                    releases ownership.",
            ));
        }
        facts.resume(agent)?;
        if !self.running.contains(&agent.id) {
            self.release_auth_probe(agent, None)?;
        }
        agent.paused = false;
        agent.state = AgentState::Starting;
        agent.error = None;
        agent.recovery_fence = None;
        self.store.unblock_agent(&agent.id)?;
        if agent.waiting_reason.is_some() || agent.auth_switch_pending.is_some() {
            self.store.deadlines_changed();
        }
        Ok(())
    }
}

fn option_effects(
    provider: crate::config::Provider,
    previous: &crate::runtime::domain::OptionsSelection,
    desired: &crate::runtime::domain::OptionsSelection,
) -> Result<std::collections::BTreeMap<String, &'static str>> {
    let mut takes_effect = std::collections::BTreeMap::new();
    for key in previous
        .options
        .values
        .keys()
        .chain(desired.options.values.keys())
    {
        if previous.options.values.get(key) != desired.options.values.get(key)
            || previous.options.sources.get(key) != desired.options.sources.get(key)
        {
            takes_effect.insert(
                key.clone(),
                crate::options::catalog(provider)?
                    .entry(key)?
                    .scope
                    .takes_effect(),
            );
        }
    }
    Ok(takes_effect)
}
