//! Bound controls that overtake their registration or positive parent evidence.
use super::super::{Codec, Result, protocol, wire};
use super::{REGISTRATION_WAIT, Scope};
use serde_json::Value;
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

const HELD_LIMIT: usize = super::super::limits::MCP_CALLBACK_LIMIT;
const HELD_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Copy)]
pub(in crate::runtime::provider::claude::codec) enum Registration {
    Ready(Scope),
    Waiting,
    Denied(&'static str),
}

struct Held {
    id: String,
    request: wire::ControlRequest,
    deadline: Instant,
    bytes: usize,
}

#[derive(Default)]
pub(in crate::runtime::provider::claude::codec) struct PendingControls {
    rows: VecDeque<Held>,
    bytes: usize,
}

impl PendingControls {
    pub(in crate::runtime::provider::claude::codec) fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
    pub(in crate::runtime::provider::claude::codec) fn identities(&self) -> usize {
        self.rows.len() * 3
    }
    pub(in crate::runtime::provider::claude::codec) fn wait(&self, timeout: Duration) -> Duration {
        self.rows
            .iter()
            .map(|row| row.deadline.saturating_duration_since(Instant::now()))
            .fold(timeout, Duration::min)
    }
    pub(in crate::runtime::provider::claude::codec) fn cancel(&mut self, id: &str) {
        self.rows.retain(|row| {
            if row.id != id {
                return true;
            }
            self.bytes -= row.bytes;
            false
        });
    }
}

fn context(request: &wire::ControlRequest) -> Option<(&str, Option<&str>, Option<&str>)> {
    match request {
        wire::ControlRequest::CanUseTool {
            tool_name,
            tool_use_id,
            agent_id,
            ..
        } => Some((tool_name, tool_use_id.as_deref(), agent_id.as_deref())),
        wire::ControlRequest::HookCallback {
            callback_id: wire::CallbackId::Guard,
            input,
        } if input.event == wire::HookEvent::PreToolUse => Some((
            input.tool_name.as_deref()?,
            input.tool_use_id.as_deref(),
            input.agent_id.as_deref(),
        )),
        _ => None,
    }
}

fn bytes(id: &str, request: &wire::ControlRequest) -> usize {
    let overhead = context(request).map_or(0, |(name, tool, agent)| {
        name.len() + tool.map_or(0, str::len) + agent.map_or(0, str::len)
    });
    id.len()
        + overhead
        + match request {
            wire::ControlRequest::CanUseTool {
                input,
                decision_reason,
                blocked_path,
                ..
            } => {
                input.to_string().len()
                    + decision_reason.as_ref().map_or(0, |v| v.to_string().len())
                    + blocked_path.as_ref().map_or(0, |v| v.to_string().len())
            }
            wire::ControlRequest::HookCallback { input, .. } => {
                input.session_id.len() + input.tool_input.to_string().len()
            }
            _ => 0,
        }
}

impl Codec {
    pub(in crate::runtime::provider::claude::codec) fn control_registration(
        &mut self,
        request: &wire::ControlRequest,
    ) -> Result<Registration> {
        match context(request) {
            Some((name, tool, agent)) => self.permission_scope(name, tool, agent),
            None => Ok(Registration::Ready(Scope::Parent)),
        }
    }
    pub(in crate::runtime::provider::claude::codec) fn hold_native_control(
        &mut self,
        id: String,
        request: wire::ControlRequest,
    ) -> Result<Vec<Value>> {
        if !super::valid_id(&id) || self.tools.pending.rows.iter().any(|row| row.id == id) {
            return Err(protocol());
        }
        let bytes = bytes(&id, &request);
        if self.tools.pending.rows.len() >= HELD_LIMIT
            || self.tools.pending.bytes.saturating_add(bytes) > HELD_BYTES
        {
            return Ok(vec![Self::deny_native_control(
                &id,
                &request,
                "Native permission registration is full.",
            )]);
        }
        self.tracking_capacity(3)?;
        self.tools.pending.bytes += bytes;
        self.tools.pending.rows.push_back(Held {
            id,
            request,
            bytes,
            deadline: Instant::now() + REGISTRATION_WAIT,
        });
        Ok(vec![])
    }
    pub(in crate::runtime::provider::claude::codec) fn resolve_native_controls(
        &mut self,
    ) -> Result<Vec<Value>> {
        self.resolve_native_controls_at(Instant::now())
    }
    pub(in crate::runtime::provider::claude::codec) fn resolve_native_controls_at(
        &mut self,
        now: Instant,
    ) -> Result<Vec<Value>> {
        if self.tools.tasks.expired(now) {
            return Err(protocol());
        }
        let pending = std::mem::take(&mut self.tools.pending.rows);
        self.tools.pending.bytes = 0;
        let mut replies = Vec::new();
        for row in pending {
            let registration = self.control_registration(&row.request)?;
            if now >= row.deadline || self.admissions_closed() {
                replies.push(Self::deny_native_control(
                    &row.id,
                    &row.request,
                    "Native permission registration ended.",
                ));
            } else {
                match registration {
                    Registration::Ready(scope) => {
                        replies.extend(self.admitted_control(row.id, row.request, scope)?);
                    }
                    Registration::Denied(reason) => {
                        replies.push(Self::deny_native_control(&row.id, &row.request, reason));
                    }
                    Registration::Waiting => {
                        self.tools.pending.bytes += row.bytes;
                        self.tools.pending.rows.push_back(row);
                    }
                }
            }
        }
        Ok(replies)
    }
    pub(in crate::runtime::provider::claude::codec) fn cancel_native_controls(
        &mut self,
    ) -> Vec<Value> {
        self.tools.pending.bytes = 0;
        std::mem::take(&mut self.tools.pending.rows)
            .into_iter()
            .map(|row| {
                Self::deny_native_control(
                    &row.id,
                    &row.request,
                    "Native permission registration ended.",
                )
            })
            .collect()
    }
    pub(crate) fn poll_native_controls(&mut self) -> Result<Vec<Value>> {
        let replies = self.resolve_native_controls()?;
        self.remember_replies(&replies)?;
        Ok(replies)
    }
}
