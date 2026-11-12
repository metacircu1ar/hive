//! Message admission, steering and redirection.
use super::{AgentState, Caller, Core, Error, EventKind, MessageState, Result, eligible};
use crate::runtime::api::InputContext as Op;
use crate::runtime::{api::events, domain::DeliveryMode};
use crate::{runtime::api, state};
use uuid::Uuid;

impl Core {
    pub(super) fn send(
        &mut self,
        caller: &Caller,
        args: &api::SendArgs,
        attachments: &[crate::runtime::attachments::Attachment],
    ) -> Result<api::SendResponse> {
        let delay = crate::runtime::scheduled::delay(args)?;
        let not_before = delay
            .map(|n| crate::runtime::scheduled::not_before(state::wall_time(), n))
            .transpose()?;
        let mode = args.delivery_mode()?;
        let steering_files;
        let attachments = if mode == Some(DeliveryMode::Steer) {
            steering_files = steering_attachments(attachments);
            &steering_files
        } else {
            attachments
        };
        let target = args.target.required(Op::Send, "/target")?;
        let body = args.body.required(Op::Send, "/body")?;
        let steering = mode == Some(crate::runtime::domain::DeliveryMode::Steer);
        let redirect = mode == Some(crate::runtime::domain::DeliveryMode::Redirect);
        let delivery = Delivery {
            target,
            not_before,
            steering,
            redirect,
        };
        let (mut agent, steering_target) = delivery.admit(self, caller)?;
        let target = agent.id.clone();
        let message_id = Uuid::new_v4().to_string();
        let sender_turn_id = self.sender_turn_id(caller)?;
        let redirect_target = Delivery {
            target: &target,
            ..delivery
        }
        .enqueue(
            self,
            &mut agent,
            crate::runtime::store::Input::new(&message_id, caller.name(), &target, body),
        )?;
        self.store.attach(&message_id, attachments)?;
        if caller.is_human() && !steering {
            self.store.track_call(&message_id)?;
        }
        MessageEvidence {
            attachments,
            message_id,
            recipient: target,
            caller,
            body,
            sender_turn_id,
            not_before,
            steering_target,
            redirect,
            redirect_target,
        }
        .record(self, &agent)
    }
    fn sender_turn_id(&self, caller: &Caller) -> Result<Option<String>> {
        if caller.is_human() {
            Ok(None)
        } else {
            Ok(self.store.agent(caller.name())?.turn_id)
        }
    }
}

fn check_steering(agent: &super::Agent) -> Result<api::SteeringTarget> {
    if agent.paused || agent.state != AgentState::Running || agent.control.is_some() {
        return Err(Error::no_active_turn(
            "Steering requires an active, unpaused turn.",
        ));
    }
    let target = api::SteeringTarget::from_agent(agent).ok_or(Error::no_active_turn(
        "Steering requires an active, unpaused turn.",
    ))?;
    if !agent.steering.supported() {
        return Err(Error::unsupported_operation(
            "This provider session does not support steering.",
        ));
    }
    Ok(target)
}

struct Delivery<'a> {
    target: &'a str,
    not_before: Option<u64>,
    steering: bool,
    redirect: bool,
}
impl Delivery<'_> {
    fn admit(
        &self,
        core: &Core,
        caller: &Caller,
    ) -> Result<(super::Agent, Option<api::SteeringTarget>)> {
        let mut agent = core.store.relative_target(caller, self.target, Op::Send)?;
        eligible(&agent)?;
        let steering_target = self.steering.then(|| check_steering(&agent)).transpose()?;
        if super::super::features::available(&agent)
            && self.not_before.is_none()
            && !self.steering
            && agent.state == AgentState::Stopped
            && !agent.paused
        {
            agent.state = AgentState::Starting;
            core.store.save(&agent)?;
        }
        Ok((agent, steering_target))
    }

    fn enqueue(
        &self,
        core: &Core,
        agent: &mut super::Agent,
        input: crate::runtime::store::Input<'_>,
    ) -> Result<Option<api::RedirectTarget>> {
        Ok(if let Some(at) = self.not_before {
            core.store.enqueue_scheduled(input, at)?;
            None
        } else if self.redirect {
            core.store
                .enqueue_redirect(input.id, input.sender, agent, input.body)?
        } else if self.steering {
            core.store
                .enqueue_steering(input.id, input.sender, agent, input.body)?;
            None
        } else {
            core.store
                .enqueue_input(input, crate::runtime::store::Priority::Normal)?;
            None
        })
    }
}

struct MessageEvidence<'a> {
    attachments: &'a [crate::runtime::attachments::Attachment],
    message_id: String,
    recipient: String,
    caller: &'a Caller,
    body: &'a str,
    sender_turn_id: Option<String>,
    not_before: Option<u64>,
    steering_target: Option<api::SteeringTarget>,
    redirect: bool,
    redirect_target: Option<api::RedirectTarget>,
}
impl MessageEvidence<'_> {
    fn record(self, core: &Core, agent: &super::Agent) -> Result<api::SendResponse> {
        let evidence = events::MessageAccepted {
            attachments: self.attachments,
            id: &self.message_id,
            sender: self.caller,
            body: self.body,
            sender_turn_id: self.sender_turn_id.as_deref(),
            not_before: self.not_before,
            mode: if self.redirect {
                Some(DeliveryMode::Redirect)
            } else {
                self.steering_target.as_ref().map(|_| DeliveryMode::Steer)
            },
            steering_target: self.steering_target.as_ref(),
            redirect_target: self.redirect.then_some(&self.redirect_target),
        };
        core.store.event_typed(
            agent,
            if self.redirect {
                EventKind::RedirectAccepted
            } else {
                EventKind::MessageAccepted
            },
            evidence,
        )?;
        Ok(api::SendResponse {
            held_for_build: None,
            message_id: self.message_id,
            delivery_state: if self.not_before.is_some() {
                MessageState::Scheduled
            } else {
                MessageState::Accepted
            },
            not_before: self.not_before,
            recipient: self.recipient,
            steering_target: self.steering_target,
            redirect_target: self.redirect_target,
        })
    }
}

fn steering_attachments(
    attachments: &[crate::runtime::attachments::Attachment],
) -> Vec<crate::runtime::attachments::Attachment> {
    attachments
        .iter()
        .cloned()
        .map(|mut a| {
            a.delivery = crate::runtime::attachments::Delivery::Reference {
                reason: crate::runtime::attachments::ReferenceReason::Steering,
            };
            a
        })
        .collect::<Vec<_>>()
}
