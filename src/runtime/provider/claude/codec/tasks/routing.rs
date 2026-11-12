//! Bind each admission to an exact foreground owner and preserve replay evidence.
use super::super::{Codec, Error, Result, ToolOrigin, limits, protocol, wire};
use super::{Registration, Scope, valid_id};
use std::time::{Duration, Instant};

impl Codec {
    pub(in crate::runtime::provider::claude::codec) fn admissions_closed(&self) -> bool {
        self.guard.sealed()
            || self.turn.completion.is_some()
            || self
                .turn
                .active
                .as_ref()
                .is_some_and(|turn| turn.interrupt_sent)
    }
    pub(in crate::runtime::provider::claude::codec) fn tracking_identities(&self) -> usize {
        self.tools.seen.len()
            + self.tools.origins.len()
            + self.tools.tasks.identities()
            + self.tools.pending.identities()
    }
    pub(in crate::runtime::provider::claude::codec) fn tracking_capacity(
        &self,
        extra: usize,
    ) -> Result<()> {
        if self.tracking_identities().saturating_add(extra) > limits::SEEN_TOOL_LIMIT {
            return Err(Error::output_limit(
                "Native Claude task tracking exceeded its bound.",
            ));
        }
        Ok(())
    }
    pub(crate) fn recycle_ready(&self) -> bool {
        self.protocol_quiescent()
            && self.guard.clean()
            && self.tracking_identities() >= limits::RECYCLE_IDENTITIES
    }
    pub(in crate::runtime::provider::claude::codec) fn frame_scope(
        &mut self,
        parent: &wire::Parent,
    ) -> Result<Option<Scope>> {
        match parent.as_deref() {
            Some(owner) => self.tools.tasks.frame_scope(owner),
            None if !parent.is_root() && self.tools.tasks.identities() != 0 => Err(protocol()),
            None => Ok(Some(Scope::Parent)),
        }
    }
    pub(in crate::runtime::provider::claude::codec) fn origin(
        &mut self,
        id: &str,
        name: &str,
        scope: Scope,
        positive_root: bool,
    ) -> Result<()> {
        self.active_id()?;
        if !valid_id(id) || name.len() > limits::TOOL_NAME_BYTES {
            return Err(protocol());
        }
        if let Some(origin) = self.tools.origins.get_mut(id) {
            if origin.name != name || origin.scope != scope {
                return Err(protocol());
            }
            origin.positive_root |= positive_root;
        } else {
            if self.tools.seen.contains(id) && !self.tools.owners.contains_key(id) {
                return Err(protocol());
            }
            self.tracking_capacity(1)?;
            self.tools.origins.insert(
                id.into(),
                ToolOrigin {
                    name: name.into(),
                    scope,
                    positive_root,
                },
            );
        }
        Ok(())
    }
    pub(in crate::runtime::provider::claude::codec) fn permission_scope(
        &mut self,
        tool: &str,
        id: Option<&str>,
        agent: Option<&str>,
    ) -> Result<Registration> {
        if id.is_some_and(|id| !valid_id(id)) || agent.is_some_and(|agent| !valid_id(agent)) {
            return Ok(Registration::Denied("Invalid native tool identity."));
        }
        // Closing admission forbids execution, but cannot erase new child
        // evidence that arrived late. Its exact registration still has to settle.
        if let Some(agent) = agent
            && !self.tools.tasks.retired_child(agent)
            && self.tools.tasks.child_scope(agent).is_none()
        {
            self.tracking_capacity(1)?;
            self.tools.tasks.observe_unresolved(agent, Instant::now());
        }
        if self.admissions_closed() {
            return Ok(Registration::Denied(
                "The current turn is no longer accepting tools.",
            ));
        }
        self.active_id()?;
        let origin = id.and_then(|id| self.tools.origins.get(id));
        if origin.is_some_and(|origin| origin.name != tool) {
            return Err(protocol());
        }
        if let Some(agent) = agent {
            return self.child_permission_scope(agent, origin);
        }
        if let Some(origin) = origin {
            if origin.scope != Scope::Parent {
                return Ok(Registration::Denied(
                    "A native child requires its registered agent identity.",
                ));
            }
            if origin.positive_root {
                return Ok(Registration::Ready(Scope::Parent));
            }
        }
        let protected = tool.starts_with("mcp__hive__")
            || matches!(
                wire::NativeTool::parse(tool),
                wire::NativeTool::Agent | wire::NativeTool::Skill
            );
        if protected || self.tools.tasks.identities() != 0 {
            return Ok(if id.is_some() {
                Registration::Waiting
            } else {
                Registration::Denied("Positive parent tool ownership is required.")
            });
        }
        Ok(Registration::Ready(Scope::Parent))
    }
    fn child_permission_scope(
        &self,
        agent: &str,
        origin: Option<&ToolOrigin>,
    ) -> Result<Registration> {
        if self.tools.tasks.retired_child(agent) {
            return Ok(Registration::Denied("The native subagent has ended."));
        }
        if let Some(scope) = self.tools.tasks.child_scope(agent) {
            if origin.is_some_and(|origin| origin.scope != scope) {
                return Err(protocol());
            }
            return Ok(Registration::Ready(scope));
        }
        Ok(if self.tools.tasks.unbound() {
            Registration::Waiting
        } else {
            Registration::Denied("No foreground task owns this native subagent.")
        })
    }
    pub(in crate::runtime::provider::claude::codec) fn ignore_finished_hook(
        &mut self,
        hook: &wire::Hook,
    ) -> Result<bool> {
        if let Some(agent) = &hook.agent_id {
            if self.tools.tasks.retired_child(agent) {
                return Ok(true);
            }
            if self.tools.tasks.child_scope(agent).is_none() {
                self.tracking_capacity(1)?;
                self.tools.tasks.observe_unresolved(agent, Instant::now());
                return Ok(true);
            }
        }
        Ok(self.turn.active.is_none()
            && hook
                .tool_use_id
                .as_ref()
                .is_some_and(|id| self.tools.seen.contains(id)))
    }
    pub(in crate::runtime::provider::claude::codec) fn settle_hook_owner(
        &mut self,
        id: &str,
        agent: Option<&str>,
    ) -> Result<bool> {
        if let Some(agent) = agent
            && self
                .tools
                .owners
                .get(id)
                .is_some_and(|owner| self.tools.tasks.child_scope(agent) != Some(owner.scope))
        {
            return Err(protocol());
        }
        self.tools.active.remove(id);
        // The agent's own Skill call settling ends its fork, so guidance
        // reaches the agent; a fork's tools never receive it.
        self.close_owner(id);
        Ok(self.tools.owners.remove(id).is_some_and(|owner| {
            owner.allowed
                && (owner.assistant_seen
                    || self
                        .tools
                        .origins
                        .get(id)
                        .is_some_and(|origin| origin.positive_root))
                && owner.scope == Scope::Parent
                && agent.is_none()
                && self
                    .turn
                    .active
                    .as_ref()
                    .is_some_and(|turn| turn.id == owner.turn)
        }))
    }
    pub(in crate::runtime::provider::claude::codec) fn close_owner(&mut self, id: &str) {
        let Some(scope) = self.tools.tasks.close(id, Instant::now()) else {
            return;
        };
        self.tools.owners.retain(|id, owner| {
            if owner.scope != scope {
                return true;
            }
            self.tools.active.remove(id);
            false
        });
        self.tools.shell_permits.retain(|_, permit| {
            if permit.scope != scope {
                return true;
            }
            // Registered commands remain owned by ShellGuard until their actual
            // processes have ended. revoke only withdraws unused admission.
            self.guard.revoke(&permit.token);
            false
        });
    }
    pub(in crate::runtime::provider::claude::codec) fn close_foreground(&mut self) {
        for id in self.tools.tasks.open_owners() {
            self.close_owner(&id);
            self.tools.active.remove(&id);
            if let Some(owner) = self.tools.owners.get_mut(&id) {
                owner.allowed = false;
            }
        }
    }
    pub(in crate::runtime::provider::claude::codec) fn register_task(
        &mut self,
        fields: &wire::System,
    ) -> Result<()> {
        self.check_session(fields.session_id.as_deref().ok_or_else(protocol)?)?;
        self.tracking_capacity(3)?;
        self.tools.tasks.register(fields)
    }
    pub(crate) fn native_control_wait(&self, timeout: Duration) -> Duration {
        self.tools.tasks.wait(self.tools.pending.wait(timeout))
    }
}
