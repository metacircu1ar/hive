//! Prepare and record one input before activating its native turn.
use super::super::OptionsSelection;
use super::{
    ActiveTurn, AgentState, Error, Input, Instant, Message, MessageState, Result, SessionLoop,
    TurnAdmission, models,
};
use crate::{
    config::{Method, Provider},
    runtime::{api::events, domain::AuthCutoff},
};

// Retry a churned auth snapshot without releasing the native conversation.
const AUTH_RETURN_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(30);

struct RecordedStart {
    branching: bool,
    message: Message,
    selection: OptionsSelection,
}

impl SessionLoop<'_> {
    pub(in crate::runtime::worker::session) async fn record_branch_input(
        &mut self,
        path: Option<&std::path::Path>,
    ) -> Result<()> {
        if !self.rpc.codex() && self.state.branch_unbound {
            let path = path.map(std::path::Path::to_path_buf);
            self.execution
                .agent(move |core, agent| {
                    if agent.branch_unbound() {
                        let thread = agent.thread_id.as_deref().ok_or_else(|| {
                            Error::session_mismatch("Missing Claude branch reservation.")
                        })?;
                        core.store().record_branch_native(agent, thread)?;
                        if let Some(path) = &path {
                            core.store().record_native_path(agent, thread, path)?;
                        }
                    }
                    Ok(())
                })
                .await?;
        }
        Ok(())
    }
    pub(in crate::runtime::worker::session) async fn start_message(
        &mut self,
        placement: usize,
    ) -> Result<()> {
        let loaded = self.selection.clone();
        let provider_version = self.rpc.provider_version().map(str::to_owned);
        let admission = if self.state.billing == Method::ApiKey {
            super::super::auth_return::claim(self.execution, loaded, provider_version).await?
        } else {
            Some(
                self.execution
                    .agent(move |core, agent| {
                        models::claim(core, agent, &loaded, provider_version.as_deref())
                    })
                    .await?,
            )
        };
        let Some(admission) = admission else {
            self.permit.take();
            tokio::time::sleep(AUTH_RETURN_RETRY_DELAY).await;
            self.notify.notify_one();
            return Ok(());
        };
        if let TurnAdmission::Submit {
            message,
            selection,
            previously_attempted,
        } = admission
        {
            self.submit_message(placement, *message, selection, previously_attempted)
                .await?;
        } else {
            self.relaunch = matches!(admission, TurnAdmission::Relaunch);
            self.permit.take();
        }
        Ok(())
    }

    async fn submit_message(
        &mut self,
        placement: usize,
        message: Message,
        selection: OptionsSelection,
        previously_attempted: bool,
    ) -> Result<()> {
        let prepared = self.prepare_attachments(message, &selection).await;
        self.rpc.activity(
            crate::runtime::scheduler::Activity {
                class: placement,
                busy: true,
                waiting: false,
            },
            true,
        );
        let (message, result) = match prepared {
            Ok(prepared) => (
                Some(prepared.message),
                self.rpc.call(prepared.request).await,
            ),
            Err(error) => (None, Err(error.before_submission())),
        };
        if !previously_attempted
            && result.as_ref().is_err_and(|e| {
                matches!(e, Error::ProviderDraining { .. }) || e.input_was_not_submitted()
            })
        {
            // This exact error is emitted before begin_request/codec.send.
            // Only this first, proven-unsent attempt may release its reservation.
            self.execution
                .agent(|core, agent| {
                    agent.native_input_attempted = false;
                    core.store().save(agent)
                })
                .await?;
        }
        let result = result?;
        let message = message
            .ok_or_else(|| Error::state_inconsistent("Submitted attachment input is missing."))?;
        let turn = Input::<String>::from_value(result.raw_turn_id().clone())
            .protocol_text()?
            .to_owned();
        let saved_turn = turn.clone();
        self.record_branch_input(result.thread_path()).await?;
        self.activate_message(message, turn, saved_turn, selection)
            .await
    }

    async fn activate_message(
        &mut self,
        message: Message,
        turn: String,
        saved_turn: String,
        selection: OptionsSelection,
    ) -> Result<()> {
        // The writer returns ownership after recording, so no input or attachment copy
        // is needed to keep the active turn's original message.
        let RecordedStart {
            branching,
            message,
            selection,
        } = self.record_started(message, saved_turn, selection).await?;
        if branching {
            self.rpc.commit_branch(&turn).await?;
        }
        self.selection = selection;
        self.applied_turn_selection = self.selection.clone();
        self.active = Some(ActiveTurn {
            message,
            turn_id: turn,
            started: Instant::now(),
        });
        self.interrupt_sent = false;
        self.stop_deadline = None;
        Ok(())
    }

    async fn record_started(
        &mut self,
        saved: Message,
        saved_turn: String,
        selection: OptionsSelection,
    ) -> Result<RecordedStart> {
        self.execution
            .agent(move |core, agent| {
                let branching = bind_started_branch(core, agent)?;
                if agent
                    .auth_cutoff
                    .as_ref()
                    .is_some_and(|cutoff| cutoff.turn() == saved_turn)
                {
                    agent.auth_cutoff = Some(AuthCutoff::Bound {
                        turn: saved_turn.clone(),
                        message: saved.id.clone(),
                    });
                } else {
                    agent.clear_cutoff();
                }
                agent.turn_id = Some(saved_turn.clone());
                agent.state = AgentState::Running;
                agent.reserved_turn = None;
                agent.last_accepted_selection = Some(selection.clone());
                agent.applied_options_revision = Some(selection.revision);
                core.store()
                    .message_state(&saved, MessageState::Submitted)?;
                core.store().save(agent)?;
                core.store().turn_started(
                    agent,
                    events::TurnStarted {
                        attachments: &saved.attachments,
                        turn_id: &saved_turn,
                        message_id: &saved.id,
                        sender: &saved.sender,
                        selection: events::TurnSelection {
                            model: &selection.model,
                            effort: &selection.effort,
                            options_revision: selection.revision,
                            selection_verification: if agent.profile.provider == Provider::Claude {
                                "session_verified"
                            } else {
                                "requested_unverified"
                            },
                        },
                    },
                )?;
                Ok(RecordedStart {
                    branching,
                    message: saved,
                    selection,
                })
            })
            .await
    }
}

fn bind_started_branch(
    core: &crate::runtime::Core,
    agent: &mut crate::runtime::domain::Agent,
) -> Result<bool> {
    let branching = agent.branch_unbound() && agent.profile.provider == Provider::Claude;
    if branching {
        let thread = agent
            .thread_id
            .clone()
            .ok_or_else(|| Error::session_mismatch("Missing Claude branch reservation."))?;
        core.store().bind_branch(agent, &thread)?;
    }
    Ok(branching)
}
