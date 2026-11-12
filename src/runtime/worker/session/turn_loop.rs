//! Drive worker admission, active turns, interruption, and deadlines.
mod input;
use super::steering::acknowledgement;
use super::{
    ActiveTurn, AdmissionEvent, AdmissionReadiness, AgentState, Control, Duration, Error,
    INTERRUPT_CONFIRMATION_TIMEOUT, IncomingFrame, Input, Instant, Message, MessageState,
    NO_DEADLINE_WAIT, Ordering, Result, SessionEvent, SessionLoop, TURN_TIMEOUT, TurnAdmission,
    handle, models, steering,
};
use crate::runtime::provider::claude::MAX_HELD;

#[derive(PartialEq, Eq)]
enum InterruptIntent {
    None,
    Requested,
    Stopping,
}

struct TurnRead {
    state: super::SessionState,
    guidance: Option<Message>,
    redirect: bool,
}

// Codex acknowledges guidance individually; Claude admits its bounded hook batch.
const CODEX_STEERING_WINDOW: usize = 1;
// Completion clears coordinating activity until the next admission refreshes placement.
const IDLE_ACTIVITY_CLASS: usize = 2;

impl SessionLoop<'_> {
    pub(super) async fn run(&mut self) -> Result<bool> {
        // Each turn orders fenced storage decisions, provider I/O and admission waits.
        loop {
            let TurnRead {
                state,
                guidance,
                redirect,
            } = self.read_turn().await?;
            self.state = state;
            let stopping = self.shutdown.load(Ordering::Relaxed)
                || self.execution.shared.stopping()
                || self.state.control == Some(Control::Stop)
                || self.state.terminal;
            let interrupt = if stopping {
                InterruptIntent::Stopping
            } else if self.state.control == Some(Control::Interrupt) || redirect {
                InterruptIntent::Requested
            } else {
                InterruptIntent::None
            };
            self.interrupt_if_requested(interrupt).await?;
            if self.should_stop(stopping)? {
                break;
            }
            if !stopping
                && !self.interrupt_sent
                && let Some(message) = guidance
            {
                self.start_steering(message).await?;
                continue;
            }
            if self.active.is_none()
                && self.pending_steering.is_empty()
                && !self.state.paused
                && !stopping
                && self.admit_work().await?
            {
                continue;
            }
            self.wait_for_event(stopping).await?;
        }
        Ok(self.active.is_some() || !self.pending_steering.is_empty())
    }

    async fn read_turn(&mut self) -> Result<TurnRead> {
        let poll_steering = self.active.is_some()
            && self.pending_steering.len()
                < if self.rpc.codex() {
                    CODEX_STEERING_WINDOW
                } else {
                    MAX_HELD
                };
        let redirect_target = self
            .active
            .as_ref()
            .filter(|_| !self.interrupt_sent)
            .map(|active| (active.message.id.clone(), active.turn_id.clone()));
        let read = self
            .execution
            .agent(move |core, agent| {
                if agent.completion_pending.is_some() && agent.turn_id.is_none() {
                    core.finish_task(agent)?;
                }
                let guidance = if poll_steering {
                    steering::queued(core, agent)?
                } else {
                    None
                };
                let redirect = if agent.redirect_pending
                    && let Some((message, turn)) = redirect_target
                {
                    #[expect(clippy::expect_used, reason = "Live execution fence checked.")]
                    core.store().claim_redirect(
                        &agent.id,
                        agent.execution_id.as_deref().expect("active execution"),
                        &message,
                        &turn,
                    )?
                } else {
                    false
                };
                Ok(TurnRead {
                    state: super::SessionState::from(&*agent),
                    guidance,
                    redirect,
                })
            })
            .await?;
        Ok(read)
    }

    async fn interrupt_if_requested(&mut self, intent: InterruptIntent) -> Result<()> {
        if intent != InterruptIntent::None && !self.interrupt_sent {
            if let Some(turn) = self.active.as_ref().map(|active| active.turn_id.clone()) {
                // Retire hosted callback ownership before native interruption can
                // emit its terminal result in the same read as the interrupt ACK.
                #[cfg(feature = "comb")]
                self.rpc.forget_comb_turn(&turn).await?;
                #[cfg(feature = "comb")]
                self.abandon_comb().await?;
                self.rpc.interrupt(self.thread_id, &turn).await?;
                self.interrupt_sent = true;
                self.stop_deadline = Some(Instant::now() + INTERRUPT_CONFIRMATION_TIMEOUT);
            } else if intent != InterruptIntent::Stopping {
                self.execution
                    .agent(|core, agent| {
                        if agent.control == Some(Control::Interrupt) {
                            agent.control = None;
                            core.store().save(agent)?;
                        }
                        Ok(())
                    })
                    .await?;
            }
        }
        Ok(())
    }

    pub(super) fn should_stop(&self, stopping: bool) -> Result<bool> {
        if self.active.is_none()
            && self.pending_steering.is_empty()
            && (stopping
                || self.rpc.draining()
                || self.relaunch
                || self.state.waiting_reason.is_some()
                || self.state.branch_hold > 0
                || (self.idle_seconds > 0
                    && self.idle_at.elapsed() >= Duration::from_secs(self.idle_seconds)))
        {
            return Ok(true);
        }
        if self.stop_deadline.is_some_and(|d| Instant::now() >= d) {
            if stopping {
                return Ok(true);
            }
            return Err(Error::interrupt_timeout(
                "Provider did not confirm interruption.",
            ));
        }
        Ok(false)
    }

    /// True means resume the outer loop, including its lifecycle recheck.
    pub(super) async fn admit_work(&mut self) -> Result<bool> {
        // Only hold a turn permit when an inbox actually contains work.
        let AdmissionReadiness { waiting, placement } = self
            .execution
            .agent(|core, agent| {
                Ok(AdmissionReadiness {
                    waiting: core.store().next_message(&agent.id)?.is_some(),
                    placement: core.store().placement(&agent.id)?,
                })
            })
            .await?;
        self.rpc.activity(
            crate::runtime::scheduler::Activity {
                class: placement,
                busy: false,
                waiting,
            },
            false,
        );
        if !waiting {
            self.admission.take();
        }
        if waiting && self.permit.is_none() && self.await_admission().await? {
            return Ok(true);
        }
        if self.permit.is_some() {
            self.start_message(placement).await?;
        }
        Ok(self.relaunch)
    }

    pub(super) async fn await_admission(&mut self) -> Result<bool> {
        let admission = self
            .admission
            .get_or_insert_with(|| Box::pin(self.turns.clone().acquire_owned()));
        let event = tokio::select! {
            permit = admission => AdmissionEvent::Permit(permit),
            frame = self.rpc.recv() => AdmissionEvent::Frame(frame),
            () = self.notify.notified() => AdmissionEvent::Wake,
            () = self.execution.shared.wait_stopped() => AdmissionEvent::Wake,
        };
        match event {
            AdmissionEvent::Permit(permit) => {
                *self.permit = Some(permit.map_err(|cause| {
                    Error::daemon_stopping("Turn scheduler stopped.").with_source(cause)
                })?);
                self.admission.take();
                Ok(false)
            }
            AdmissionEvent::Frame(frame) => {
                // Admission frames intentionally skip finished-turn bookkeeping.
                self.handle_frame(frame?).await?;
                Ok(true)
            }
            AdmissionEvent::Wake => Ok(true),
        }
    }

    pub(super) fn deadline(&self) -> Instant {
        let active_deadline = self
            .active
            .as_ref()
            .map(|ActiveTurn { started: start, .. }| *start + TURN_TIMEOUT);
        let deadline = active_deadline
            .into_iter()
            .chain(self.stop_deadline)
            .chain(
                self.pending_steering
                    .iter()
                    .filter_map(|pending| pending.deadline),
            )
            .min();
        // An idle session uses its idle expiry even when a stop deadline remains.
        let deadline =
            if self.active.is_none() && self.pending_steering.is_empty() && self.idle_seconds > 0 {
                Some(self.idle_at + Duration::from_secs(self.idle_seconds))
            } else {
                deadline
            };
        deadline.unwrap_or_else(|| Instant::now() + NO_DEADLINE_WAIT)
    }

    pub(super) async fn wait_for_event(&mut self, stopping: bool) -> Result<()> {
        let sleep = tokio::time::sleep_until(tokio::time::Instant::from_std(self.deadline()));
        let event = {
            #[cfg(feature = "comb")]
            let knowledge = async {
                let (index, result) = super::comb::response(&mut self.pending_comb).await;
                SessionEvent::Comb(index, result)
            };
            #[cfg(not(feature = "comb"))]
            let knowledge = std::future::pending::<SessionEvent>();
            tokio::select! {
                frame = self.rpc.recv() => SessionEvent::Frame(frame),
                event = knowledge => event,
                result = acknowledgement(&mut self.pending_steering) => {
                    SessionEvent::Steering(result)
                },

                () = self.notify.notified() => SessionEvent::Wake,
                () = self.execution.shared.wait_stopped(), if !stopping => SessionEvent::Wake,
                () = sleep => SessionEvent::Deadline,
            }
        };
        match event {
            SessionEvent::Frame(frame) => self.handle_running_frame(frame).await,
            SessionEvent::Steering(steering::Acknowledgement { index, result }) => {
                self.finish_steering(index, result).await
            }
            #[cfg(feature = "comb")]
            SessionEvent::Comb(index, result) => self.finish_comb(index, result).await,
            SessionEvent::Wake => Ok(()),
            SessionEvent::Deadline => self.check_turn_deadline(),
        }
    }

    pub(super) async fn handle_frame(&mut self, frame: IncomingFrame) -> Result<()> {
        #[cfg(feature = "comb")]
        if self.start_comb(&frame).await? {
            return Ok(());
        }
        handle(
            self.execution,
            self.rpc,
            self.thread_id,
            &mut self.active,
            frame,
        )
        .await
    }

    pub(super) async fn handle_running_frame(
        &mut self,
        frame: Result<IncomingFrame>,
    ) -> Result<()> {
        let frame = frame?;
        let completion = MessageState::from_provider_completion(frame.params().turn_status());
        let was_active = self.active.is_some();
        self.handle_frame(frame).await?;
        if was_active && self.active.is_none() {
            #[cfg(feature = "comb")]
            self.abandon_comb().await?;
            for pending in &mut self.pending_steering {
                pending.completion = Some(completion);
            }
            self.rpc.activity(
                crate::runtime::scheduler::Activity {
                    class: IDLE_ACTIVITY_CLASS,
                    busy: false,
                    waiting: false,
                },
                true,
            );
            self.permit.take();
            self.interrupt_sent = false;
            self.stop_deadline = None;
            self.idle_at = Instant::now();
        }
        Ok(())
    }

    pub(super) fn check_turn_deadline(&self) -> Result<()> {
        if self.pending_steering.iter().any(|p| {
            p.deadline
                .is_some_and(|deadline| Instant::now() >= deadline)
        }) {
            return Err(Error::provider_timeout(
                "Steering delivery did not settle within 30 seconds.",
            ));
        }
        if self
            .active
            .as_ref()
            .is_some_and(|ActiveTurn { started: start, .. }| start.elapsed() >= TURN_TIMEOUT)
        {
            return Err(Error::turn_timeout("Agent turn exceeded 30 minutes."));
        }
        Ok(())
    }
}
