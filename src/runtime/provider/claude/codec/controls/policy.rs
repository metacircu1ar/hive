//! Permission policy after native control identity and admission checks.
use super::{Codec, Result, Scope, Shell, parse, wire};
use crate::{
    runtime::provider::claude::{SteeringPermit, read_only_git},
    shell::TOKEN_PREFIX,
};
use serde::Deserialize;
use serde_json::Value;
use std::path::Path;

impl Codec {
    pub(super) fn policy_reason(&self, name: &str, scope: Scope) -> &'static str {
        if scope != Scope::Parent && name.starts_with("mcp__hive__") {
            "Native children cannot call Hive or Comb tools; ask the parent to perform this action."
        } else if wire::NativeTool::parse(name) == wire::NativeTool::Agent {
            super::super::tasks::AGENT_REASON
        } else if name == "Bash" && self.config.shell() == Shell::ReadOnlyGit {
            "Read-only agents may run one allowlisted read-only git command without shell syntax."
        } else {
            "Hive agent permissions"
        }
    }
    pub(super) fn allowed_input(
        &self,
        name: &str,
        input: &mut Value,
        scope: Scope,
    ) -> Result<bool> {
        if wire::NativeTool::parse(name) == wire::NativeTool::Agent {
            if scope != Scope::Parent || self.config.shell() == Shell::None {
                return Ok(false);
            }
            let Some(normalized) = self.native_agent_input(input)? else {
                return Ok(false);
            };
            *input = normalized;
            return Ok(true);
        }
        self.allowed(name, input, scope)
    }
    fn allowed(&self, full_name: &str, input: &Value, scope: Scope) -> Result<bool> {
        self.active_id()?;
        let name = wire::NativeTool::parse(full_name);
        if scope != Scope::Parent
            && (full_name.starts_with("mcp__hive__")
                || matches!(name, wire::NativeTool::Skill | wire::NativeTool::Agent))
        {
            return Ok(false);
        }
        if matches!(self.steering_permit, Some(SteeringPermit::Canary)) {
            let args: PolicyInput = parse(input.clone())?;
            return Ok(name == wire::NativeTool::Read
                && args
                    .file_path
                    .as_deref()
                    .is_some_and(|path| Path::new(path) == self.config.cwd.join("fixed.txt")));
        }
        if let Some(tool) = full_name.strip_prefix("mcp__hive__") {
            return Ok(crate::runtime::tool_registry::advertised(
                self.config.tool_catalog,
                tool,
                self.config.comb_reviewer,
            ));
        }
        if self.config.mcp_servers.keys().any(|server| {
            full_name
                .strip_prefix(&format!("mcp__{server}__"))
                .is_some_and(|tool| !tool.is_empty())
        }) {
            return Ok(true);
        }
        // A skill only adds instructions; every tool it then calls is checked
        // here. A forked skill cannot start another skill: a second fork would
        // be a second task.
        if name == wire::NativeTool::Skill {
            return Ok(self.config.skills_enabled());
        }
        let args: PolicyInput = parse(input.clone())?;
        Ok(self.native_permission(name, args))
    }
    fn native_permission(&self, name: wire::NativeTool, args: PolicyInput) -> bool {
        match name {
            wire::NativeTool::Read | wire::NativeTool::Glob | wire::NativeTool::Grep => true,
            wire::NativeTool::Bash => {
                !args.run_in_background
                    && (!args.unsandboxed || self.config.shell() == Shell::Unrestricted)
                    && args.command.is_some_and(|command| {
                        !command.contains(TOKEN_PREFIX)
                            && match self.config.shell() {
                                Shell::Full | Shell::Unrestricted => true,
                                Shell::ReadOnlyGit => read_only_git::parse(&command).is_some(),
                                Shell::None => false,
                            }
                    })
            }
            wire::NativeTool::Edit | wire::NativeTool::Write if args.notebook_path.is_none() => {
                args.file_path
                    .is_some_and(|path| self.config.writable(&path))
            }
            wire::NativeTool::NotebookEdit if args.file_path.is_none() => args
                .notebook_path
                .is_some_and(|path| self.config.writable(&path)),
            _ => false,
        }
    }
    pub(super) fn coordination_capacity(&self, name: &str, id: Option<&str>) -> bool {
        if !matches!(
            wire::NativeTool::parse(name),
            wire::NativeTool::Agent | wire::NativeTool::Skill
        ) {
            return true;
        }
        id.is_some_and(|id| self.tools.owners.get(id).is_some_and(|owner| owner.allowed))
            || self.tools.tasks.can_open(name)
    }
    pub(super) fn shell_permission(
        &mut self,
        id: Option<&str>,
        input: &Value,
        scope: Scope,
    ) -> bool {
        if self.config.shell() == Shell::None {
            return false;
        }
        let Some(args) = crate::observability::optional(
            serde_json::from_value::<PolicyInput>(input.clone()),
            "claude_shell_policy",
        ) else {
            return false;
        };
        if args.run_in_background
            || (args.unsandboxed && self.config.shell() != Shell::Unrestricted)
        {
            return false;
        }
        let Some(command) = args.command else {
            return false;
        };
        let mut matching = self.tools.shell_permits.iter_mut().filter(|(key, permit)| {
            id.is_none_or(|id| *key == id)
                && permit.scope == scope
                && !permit.permission_used
                && permit.command == command
                && self.guard.unregistered(&permit.token)
        });
        let Some((_, permit)) = matching.next() else {
            return false;
        };
        if matching.next().is_some() {
            return false;
        }
        permit.permission_used = true;
        true
    }
}
#[derive(Deserialize, Default)]
struct PolicyInput {
    command: Option<String>,
    file_path: Option<String>,
    notebook_path: Option<String>,
    #[serde(default)]
    run_in_background: bool,
    #[serde(default, rename = "dangerouslyDisableSandbox")]
    unsandboxed: bool,
}
