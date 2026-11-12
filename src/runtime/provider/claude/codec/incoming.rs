//! Decode Claude frames and reconcile user, system, and result evidence.
use super::limits::CANCELLED_CALLBACK_LIMIT;
use super::tasks::Scope;
use super::wire::emitted;
use super::{Codec, ProviderTurnStatus, Result, ServerMethod, Value, parse, protocol, wire};
use crate::runtime::provider::protocol::codes;
use std::mem;

struct UserFrame {
    uuid: Option<String>,
    session: String,
    parent: wire::Parent,
    replay: bool,
    message: Option<wire::UserMessage>,
}

struct ResultFrame {
    session_id: String,
    model_usage: Option<crate::runtime::domain::ModelUsage>,
    subtype: wire::ResultSubtype,
    is_error: bool,
    user_message_uuid: Option<String>,
    terminal_reason: Option<wire::TerminalReason>,
    origin: Option<wire::Origin>,
}

impl Codec {
    fn user(&mut self, frame: UserFrame) -> Result<()> {
        let UserFrame {
            uuid,
            session,
            parent,
            replay,
            message,
        } = frame;
        let session = session.as_str();
        self.check_session(session)?;
        let Some(scope) = self.frame_scope(&parent)? else {
            return Ok(());
        };
        if replay && scope != Scope::Parent {
            return Ok(());
        }
        if replay {
            return self.acknowledge(uuid, session, parent.into_option());
        }
        self.check_session(session)?;
        // A fork's prompt and tool results arrive under its Skill call; their
        // tool results settle like the agent's own.
        if let Some(message) = message {
            for block in message.content.blocks {
                if block.tool_result() {
                    let id = block.tool_use_id().ok_or_else(protocol)?;
                    self.user_tool_result(id, &block.raw, scope)?;
                }
            }
        }
        Ok(())
    }
    fn user_tool_result(&mut self, id: &str, result: &Value, scope: Scope) -> Result<()> {
        if self
            .tools
            .owners
            .get(id)
            .is_some_and(|owner| owner.scope != scope)
            || self
                .tools
                .shell_permits
                .get(id)
                .is_some_and(|permit| permit.scope != scope)
        {
            return Err(protocol());
        }
        self.tools.active.remove(id);
        if scope == Scope::Parent {
            self.close_owner(id);
        }
        if let Some(permit) = self.tools.shell_permits.remove(id) {
            self.guard.revoke(&permit.token);
        }
        let turn = self.active_id()?;
        self.frames.notice(
            ServerMethod::ItemCompleted,
            emitted::ItemParams {
                thread_id: &self.config.id,
                turn_id: &turn,
                item: emitted::ToolResult { id, result },
            },
        );
        Ok(())
    }
    pub(crate) fn receive(&mut self, value: Value) -> Result<Vec<Value>> {
        // Capture execution evidence before typed parsing can fail. Initialization
        // metadata and exact transport echoes cannot execute an agent task.
        let metadata = wire::PreinputMetadata::read(&value);
        if !self.turn.input.enqueued && !metadata.allowed() {
            self.turn.input.execution_observed = true;
            return Err(protocol());
        }
        if metadata.kind == Some(wire::FrameKind::ControlResponse) && self.steering_echo(&value) {
            self.echoes.pending.retain(|reply| reply != &value);
            return Ok(vec![]);
        }
        if metadata.kind == Some(wire::FrameKind::ControlResponse) {
            let echo = self.echoes.pending.iter().position(|reply| reply == &value);
            if let Some(index) = echo {
                self.echoes.pending.remove(index);
                return Ok(vec![]);
            }
        }
        if self.hold_branch_frame(&value)? {
            return Ok(vec![]);
        }
        let mut output = self.receive_inner(value)?;
        output.extend(self.resolve_native_controls()?);
        self.settle();
        self.remember_replies(&output)?;
        Ok(output)
    }
    // Keep one dispatch over the complete typed frame; each branch retains its
    // payload ownership until its dedicated handler has settled.
    pub(super) fn receive_inner(&mut self, value: Value) -> Result<Vec<Value>> {
        match parse::<wire::Frame>(value)? {
            wire::Frame::ControlResponse { response } => self.control_result(response)?,
            wire::Frame::ControlRequest {
                request_id,
                request,
            } => return self.incoming_control(request_id, request),
            wire::Frame::ControlCancelRequest { request_id } => {
                self.cancel_control(&request_id)?;
            }
            wire::Frame::System { subtype, fields } => self.system(subtype, fields)?,
            wire::Frame::User {
                uuid,
                session_id,
                parent_tool_use_id,
                replay,
                message,
            } => self.user(UserFrame {
                uuid,
                session: session_id,
                parent: parent_tool_use_id,
                replay,
                message,
            })?,
            wire::Frame::Assistant {
                error,
                message,
                session_id,
                parent_tool_use_id,
            } => {
                self.assistant_error(error, &session_id, parent_tool_use_id.as_deref());
                self.assistant(message, &session_id, parent_tool_use_id)?;
            }
            wire::Frame::StreamEvent {
                session_id,
                parent_tool_use_id,
                user_message_uuid,
                event,
            } => self.stream(&session_id, parent_tool_use_id, user_message_uuid, event)?,
            wire::Frame::Result {
                session_id,
                model_usage,
                subtype,
                is_error,
                user_message_uuid,
                terminal_reason,
                _num_turns: _,
                origin,
            } => {
                let replies = self.cancel_native_controls();
                self.result_frame(ResultFrame {
                    session_id,
                    model_usage,
                    subtype,
                    is_error,
                    user_message_uuid,
                    terminal_reason,
                    origin,
                })?;
                return Ok(replies);
            }
            wire::Frame::RateLimitEvent {
                session_id,
                rate_limit_info,
            } => {
                self.rate_limit(&session_id, rate_limit_info);
            }
            wire::Frame::Other => {}
        }
        Ok(vec![])
    }
    fn cancel_control(&mut self, request_id: &str) -> Result<()> {
        self.tools.pending.cancel(request_id);
        self.expire_steering(false, Some(request_id));
        self.echoes
            .pending
            .retain(|frame| wire::Decision::read(frame).request_id() != Some(request_id));
        self.cancel_callback(request_id)
    }
    fn rate_limit(&mut self, session_id: &str, rate_limit_info: Value) {
        if session_id == self.config.id {
            // Project in the shard; no raw quota snapshot is ever persisted.
            self.frames.notice(
                ServerMethod::ClaudeRateLimits,
                emitted::RateLimits {
                    info: rate_limit_info,
                    thread_id: session_id,
                    turn_id: self.turn.active.as_ref().map(|t| t.id.as_str()),
                },
            );
        }
    }

    fn assistant_error(
        &mut self,
        error: Option<wire::AssistantError>,
        session_id: &str,
        parent_tool_use_id: Option<&str>,
    ) {
        if session_id == self.config.id && parent_tool_use_id.is_none() {
            let kind = match error {
                Some(wire::AssistantError::AuthenticationFailed) => Some("unauthorized"),
                Some(wire::AssistantError::BillingError) => Some("billingError"),
                Some(wire::AssistantError::RateLimit) => Some("usageLimitExceeded"),
                _ => None,
            };
            if let Some(kind) = kind {
                self.frames.notice(
                    ServerMethod::Error,
                    emitted::ProviderError {
                        thread_id: session_id,
                        turn_id: self.turn.active.as_ref().map(|t| t.id.as_str()),
                        will_retry: false,
                        error: emitted::ErrorInfo {
                            codex_error_info: kind,
                        },
                    },
                );
            }
        }
    }
    fn result_frame(&mut self, frame: ResultFrame) -> Result<()> {
        if frame
            .origin
            .as_ref()
            .is_some_and(|origin| origin.kind != wire::OriginKind::Human)
            || frame.terminal_reason == Some(wire::TerminalReason::BackgroundRequested)
        {
            self.turn.input.execution_observed = true;
            return Err(protocol());
        }
        // receive() already validated startup-rejection metadata before typed parsing.
        if !self.turn.input.enqueued
            && self.turn.active.is_none()
            && self.tools.active.is_empty()
            && self.callbacks.requests.is_empty()
        {
            self.turn.input.rejected = true;
            for (_, (id, _)) in mem::take(&mut self.callbacks.controls) {
                self.frames.push_back(emitted::error(
                    id,
                    codes::SERVER_ERROR,
                    "Claude rejected session startup.",
                ));
            }
        } else {
            self.result(frame)?;
        }
        Ok(())
    }
    pub(super) fn cancel_callback(&mut self, request_id: &str) -> Result<()> {
        // Native cancellation does not undo an already-dispatched tool.
        // A late result is discarded using its bounded tombstone.
        for id in self.callbacks.matching_requests(|id, _| id == request_id) {
            self.callbacks.requests.remove(&id);
            self.callbacks.cancelled.insert(id);
            if self.callbacks.cancelled.len() > CANCELLED_CALLBACK_LIMIT {
                return Err(protocol());
            }
        }
        Ok(())
    }
    pub(super) fn system(
        &mut self,
        subtype: wire::SystemSubtype,
        fields: wire::System,
    ) -> Result<()> {
        if subtype == wire::SystemSubtype::Init {
            self.check_session(fields.session_id.as_deref().ok_or_else(protocol)?)?;
            self.check_model(fields.model.as_deref().ok_or_else(protocol)?)?;
            if fields.permission_mode != Some(wire::PermissionMode::Default) {
                return Err(protocol());
            }
            self.verified = true;
        } else if subtype == wire::SystemSubtype::TaskStarted {
            self.register_task(&fields)?;
        } else if matches!(
            subtype,
            wire::SystemSubtype::TaskNotification
                | wire::SystemSubtype::TaskProgress
                | wire::SystemSubtype::TaskUpdated
        ) {
            self.check_session(fields.session_id.as_deref().ok_or_else(protocol)?)?;
            self.tools.tasks.update(subtype, &fields)?;
        } else if subtype == wire::SystemSubtype::BackgroundTasksChanged {
            // Background work is disabled. Its appearance violates quiescence.
            return Err(protocol());
        }
        Ok(())
    }
    fn result(&mut self, frame: ResultFrame) -> Result<()> {
        let ResultFrame {
            session_id,
            subtype,
            is_error: error,
            user_message_uuid,
            terminal_reason: reason,
            model_usage,
            ..
        } = frame;
        let session = session_id.as_str();
        let uuid = user_message_uuid.as_deref();
        self.check_session(session)?;
        if self
            .turn
            .active
            .as_ref()
            .is_some_and(|turn| turn.interrupt_sent)
        {
            self.close_foreground();
        }
        if self.turn.completion.is_some() {
            return Err(protocol());
        }
        let turn = self.turn.active.as_ref().ok_or_else(protocol)?;
        if uuid.is_some_and(|id| id != turn.id)
            || (uuid.is_none() && !turn.acknowledged)
            || !self.callbacks.requests.is_empty()
            || !self.tools.active.is_empty()
            || self.tools.tasks.active()
        {
            return Err(protocol());
        }
        self.turn.terminal_error = error;
        if !turn.acknowledged {
            if !error {
                return Err(protocol());
            }
            self.reject_native_input(turn.request);
            return Ok(());
        }
        let status = if subtype == wire::ResultSubtype::Success
            && !error
            && reason == Some(wire::TerminalReason::Completed)
        {
            ProviderTurnStatus::Completed
        } else if matches!(
            reason,
            Some(wire::TerminalReason::AbortedStreaming | wire::TerminalReason::AbortedTools)
        ) || turn.interrupt_sent
        {
            ProviderTurnStatus::Interrupted
        } else {
            ProviderTurnStatus::Failed
        };
        let id = turn.id.clone();
        self.finish_result(id, status, model_usage);
        Ok(())
    }
    fn reject_native_input(&mut self, request: u64) {
        self.frames.push_back(emitted::error(
            request,
            codes::SERVER_ERROR,
            "Claude rejected this input.",
        ));
        self.turn.active = None;
    }
    fn finish_result(
        &mut self,
        id: String,
        status: ProviderTurnStatus,
        model_usage: Option<crate::runtime::domain::ModelUsage>,
    ) {
        self.flush_assistant_text();
        if status == ProviderTurnStatus::Completed {
            self.steering_success();
        }
        self.expire_steering(true, None);
        self.turn.model_usage = model_usage.map(|models| (status, models));
        self.turn.completion = Some(emitted::TerminalTurn { id, status });
        self.settle();
    }
}
