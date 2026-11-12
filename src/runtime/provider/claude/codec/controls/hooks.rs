//! Guard and finished hook phases preserve exact turn and owner boundaries.
use super::super::{
    Observation, ShellPermit,
    limits::{ACTIVE_TOOL_LIMIT, CANARY_TOOL_LIMIT},
    tasks,
};
use super::{Codec, Payload, Result, Scope, Shell, control, denied, protocol, wire};
use crate::{
    error::Error,
    runtime::provider::claude::{SteeringPermit, read_only_git},
    shell::TOKEN_PREFIX,
};
use serde_json::Value;

#[cfg(test)]
mod tests;

fn shell_word(word: &str) -> String {
    format!("'{}'", word.replace('\'', "'\\''"))
}

fn supervised_shell(supervisor: &str, token: &str, command: &str) -> String {
    let admitted = format!("# {TOKEN_PREFIX}{token}\n{command}");
    format!(
        "# {TOKEN_PREFIX}{token}\nexec {} {}",
        shell_word(supervisor),
        shell_word(&admitted)
    )
}

impl Codec {
    pub(super) fn hook(
        &mut self,
        callback: wire::CallbackId,
        hook: wire::Hook,
        scope: Scope,
    ) -> Result<Payload> {
        self.check_session(&hook.session_id)?;
        if hook
            .agent_id
            .as_deref()
            .is_some_and(|id| !tasks::valid_id(id))
            || hook
                .tool_use_id
                .as_deref()
                .is_some_and(|id| !tasks::valid_id(id))
        {
            return Err(protocol());
        }
        if callback == wire::CallbackId::Finished && self.ignore_finished_hook(&hook)? {
            return Ok(Payload::empty());
        }
        if callback == wire::CallbackId::Guard
            && hook.event == wire::HookEvent::PreToolUse
            && self.admissions_closed()
        {
            return Ok(Payload::Hook(control::HookOutput {
                hook_specific_output: control::HookDecision {
                    permission_decision: control::PermissionDecision::Deny,
                    updated_input: None,
                    permission_decision_reason: "Agent is shutting down.",
                },
            }));
        }
        self.active_id()?;
        let tool_id = hook.tool_use_id.ok_or_else(protocol)?;
        match (callback, hook.event) {
            (wire::CallbackId::Guard, wire::HookEvent::PreToolUse) => {
                self.guard_tool(tool_id, hook.tool_name, hook.tool_input, scope)
            }
            (
                wire::CallbackId::Finished,
                wire::HookEvent::PostToolUse | wire::HookEvent::PostToolUseFailure,
            ) => self.finished_hook(&tool_id, hook.agent_id.as_deref(), hook.event),
            _ => Err(protocol()),
        }
    }
    fn finished_hook(
        &mut self,
        tool_id: &str,
        agent: Option<&str>,
        event: wire::HookEvent,
    ) -> Result<Payload> {
        let owned = self.settle_hook_owner(tool_id, agent)?;
        if let Some(permit) = self.tools.shell_permits.remove(tool_id) {
            self.guard.revoke(&permit.token);
        }
        // Steering guidance is for the agent, never a forked skill's subagent.
        if owned && event == wire::HookEvent::PostToolUse {
            return Ok(self
                .hook_guidance()
                .map_or_else(Payload::empty, Payload::Guidance));
        }
        Ok(Payload::empty())
    }
    fn guard_tool(
        &mut self,
        tool_id: String,
        tool_name: Option<String>,
        tool_input: Value,
        scope: Scope,
    ) -> Result<Payload> {
        self.canary_tool()?;
        let name = tool_name.ok_or_else(protocol)?;
        let mut input = tool_input;
        let capacity = self.coordination_capacity(&name, None);
        let allowed = capacity
            && self
                .allowed_input(&name, &mut input, scope)
                .inspect_err(|error| {
                    crate::observability::observation_failed("claude_hook_policy", error);
                })
                .unwrap_or(false);
        if !allowed {
            denied(&name, "hook");
        }
        if allowed {
            self.observe_tool(&tool_id, &name, Observation::Pre(scope))?;
            if let Some(owner) = self.tools.owners.get_mut(&tool_id) {
                owner.allowed = true;
            }
            if self.tools.active.len() >= ACTIVE_TOOL_LIMIT
                || !self.tools.active.insert(tool_id.clone())
            {
                return Err(protocol());
            }
            if scope == Scope::Parent {
                self.tracking_capacity(1)?;
                self.tools.tasks.open(&tool_id, &name)?;
            }
            if wire::NativeTool::parse(&name) == wire::NativeTool::Bash {
                self.wrap_shell_command(tool_id, &mut input, scope)?;
            }
        }
        Ok(Payload::Hook(control::HookOutput {
            hook_specific_output: control::HookDecision {
                permission_decision: if allowed {
                    control::PermissionDecision::Allow
                } else {
                    control::PermissionDecision::Deny
                },
                updated_input: Some(input),
                permission_decision_reason: if allowed {
                    "Hive agent permissions"
                } else {
                    self.policy_reason(&name, scope)
                },
            },
        }))
    }
    fn wrap_shell_command(
        &mut self,
        tool_id: String,
        input: &mut Value,
        scope: Scope,
    ) -> Result<()> {
        // Replace only the approved shell command; preserve every other native input field.
        let slot = input
            .as_object_mut()
            .and_then(|fields| fields.get_mut("command"))
            .ok_or_else(protocol)?;
        let command = slot.as_str().ok_or_else(protocol)?;
        // A read-only agent's git command runs only in its checked rendering.
        let command = if self.config.shell() == Shell::ReadOnlyGit {
            read_only_git::parse(command).ok_or_else(protocol)?.render()
        } else {
            command.to_owned()
        };
        let supervisor = self.guard.directory().join("hive-shell");
        let supervisor = supervisor.to_str().ok_or_else(protocol)?;
        let token = self.guard.expect()?;
        let command = supervised_shell(supervisor, &token, &command);
        self.tools.shell_permits.insert(
            tool_id,
            ShellPermit {
                token,
                command: command.clone(),
                permission_used: false,
                scope,
            },
        );
        *slot = Value::String(command);
        Ok(())
    }
    pub(super) fn canary_tool(&mut self) -> Result<()> {
        if matches!(self.steering_permit, Some(SteeringPermit::Canary)) {
            self.tools.canary_count += 1;
            if self.tools.canary_count > CANARY_TOOL_LIMIT {
                return Err(Error::output_limit(
                    "Native canary exceeded its tool-call limit.",
                ));
            }
        }
        Ok(())
    }
}
