//! Validate native Claude tool callbacks and synthesize control replies.
mod hooks;
mod policy;
use super::super::launch::Shell;
use super::limits::{CANCELLED_CALLBACK_LIMIT, MCP_CALLBACK_LIMIT};
use super::tasks::{Registration, Scope};
use super::wire::{control, mcp};
use super::{Codec, parse, protocol, wire};
use crate::error::Result;
use crate::runtime::provider::protocol::ServerMethod;
use crate::runtime::provider::protocol::codes;
use crate::runtime::provider::protocol::outgoing::ToolResponse;
use serde::Deserialize;
use serde_json::Value;

#[derive(serde::Serialize)]
#[serde(untagged)]
enum Payload {
    Permission(control::Permission),
    Hook(control::HookOutput),
    Guidance(super::emitted::Guidance),
    Empty(mcp::Empty),
}
impl Payload {
    fn empty() -> Self {
        Self::Empty(mcp::Empty {})
    }
}

fn denied(name: &str, phase: &'static str) {
    static DENIED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    log_denied(name, phase, &DENIED);
}

fn log_denied(name: &str, phase: &'static str, denied: &std::sync::atomic::AtomicU64) {
    // Only fixed native names enter logs; MCP and unknown names can contain private text.
    let method = match name {
        "Read" => "read",
        "Glob" => "glob",
        "Grep" => "grep",
        "Bash" => "bash",
        "Edit" => "edit",
        "Write" => "write",
        "NotebookEdit" => "notebook_edit",
        "Skill" => "skill",
        "Agent" => "agent",
        "Task" => "task",
        _ => "other",
    };
    if let Some(count) = crate::observability::sample(denied) {
        tracing::info!(name: "tool_policy.denied",
             provider = "claude",
             method = method,
             phase = phase,
             count = count);
    }
}

impl Codec {
    #[cfg(feature = "comb")]
    pub(crate) fn forget_tool(&mut self, id: &Value) -> Result<()> {
        let id = id.as_u64().ok_or_else(protocol)?;
        self.callbacks.requests.remove(&id);
        self.callbacks.cancelled.remove(&id);
        #[cfg(feature = "comb")]
        self.callbacks.comb_turns.remove(&id);
        Ok(())
    }
    #[cfg(feature = "comb")]
    pub(crate) fn forget_comb_turn(&mut self, turn: &str) {
        self.callbacks.retired_comb_turn = Some(turn.into());
        let ids: Vec<_> = self
            .callbacks
            .comb_turns
            .iter()
            .filter(|(_, owner)| *owner == turn)
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            self.callbacks.requests.remove(&id);
            self.callbacks.cancelled.remove(&id);
            #[cfg(feature = "comb")]
            self.callbacks.comb_turns.remove(&id);
        }
    }
    pub(super) fn incoming_control(
        &mut self,
        id: String,
        request: wire::ControlRequest,
    ) -> Result<Vec<Value>> {
        match self.control_registration(&request)? {
            Registration::Waiting => self.hold_native_control(id, request),
            Registration::Denied(reason) => {
                Ok(vec![Self::deny_native_control(&id, &request, reason)])
            }
            Registration::Ready(scope) => self.admitted_control(id, request, scope),
        }
    }
    pub(super) fn admitted_control(
        &mut self,
        id: String,
        request: wire::ControlRequest,
        scope: Scope,
    ) -> Result<Vec<Value>> {
        let payload = match request {
            wire::ControlRequest::McpMessage {
                server_name,
                message,
            } => {
                if server_name != wire::McpServer::Hive {
                    return Err(protocol());
                }
                return self.mcp(id, message);
            }
            wire::ControlRequest::HookCallback { callback_id, input } => {
                self.hook(callback_id, input, scope)?
            }
            wire::ControlRequest::CanUseTool {
                tool_name,
                mut input,
                decision_reason,
                blocked_path,
                tool_use_id,
                agent_id: _,
            } => {
                let allowed = if self.admissions_closed() {
                    false
                } else if wire::NativeTool::parse(&tool_name) == wire::NativeTool::Bash {
                    self.shell_permission(tool_use_id.as_deref(), &input, scope)
                } else if !self.coordination_capacity(&tool_name, tool_use_id.as_deref()) {
                    false
                } else {
                    self.allowed_input(&tool_name, &mut input, scope)
                        .inspect_err(|error| {
                            crate::observability::observation_failed("claude_tool_policy", error);
                        })
                        .unwrap_or(false)
                };
                if allowed
                    && (self.config.shell() == Shell::Unrestricted
                        || (decision_reason.is_none() && blocked_path.is_none()))
                {
                    Payload::Permission(control::Permission::Allow { input })
                } else {
                    denied(&tool_name, "permission");
                    Payload::Permission(control::Permission::Deny {
                        message: self.policy_reason(&tool_name, scope),
                    })
                }
            }
            wire::ControlRequest::Other => {
                return Ok(vec![control::unsupported(&id)]);
            }
        };
        let mut reply = control::reply(&id, payload);
        self.emitted_guidance(&mut reply)?;
        Ok(vec![reply])
    }
    pub(super) fn deny_native_control(
        id: &str,
        request: &wire::ControlRequest,
        reason: &'static str,
    ) -> Value {
        let payload = match request {
            wire::ControlRequest::HookCallback { .. } => Payload::Hook(control::HookOutput {
                hook_specific_output: control::HookDecision {
                    permission_decision: control::PermissionDecision::Deny,
                    updated_input: None,
                    permission_decision_reason: reason,
                },
            }),
            _ => Payload::Permission(control::Permission::Deny { message: reason }),
        };
        control::reply(id, payload)
    }
    fn mcp(&mut self, control: String, request: wire::McpRequest) -> Result<Vec<Value>> {
        let result = match request.method {
            wire::McpMethod::Initialize => {
                let Some(protocol) = mcp::initialize_protocol(&request.params) else {
                    return Ok(vec![mcp::error(
                        &control,
                        request.id.as_ref(),
                        codes::INVALID_PARAMS,
                        "Invalid MCP protocol version.",
                    )]);
                };
                mcp::ResultBody::initialize(protocol)
            }
            wire::McpMethod::Initialized | wire::McpMethod::Ping => mcp::ResultBody::empty(),
            wire::McpMethod::Cancelled => {
                self.cancel_mcp(request.params)?;
                mcp::ResultBody::empty()
            }
            wire::McpMethod::ToolsList => {
                let mut tools =
                    Vec::<mcp::Definition>::deserialize(crate::runtime::tool_registry::tools(
                        self.config.tool_catalog,
                        self.config.comb_reviewer,
                    ))
                    .map_err(|cause| protocol().with_source(cause))?;
                tools
                    .iter_mut()
                    .for_each(mcp::Definition::native_annotations);
                mcp::ResultBody::Catalog(mcp::Catalog { tools })
            }
            wire::McpMethod::ToolsCall => return self.mcp_tool_call(control, request),
            wire::McpMethod::Other => {
                return Ok(vec![mcp::error(
                    &control,
                    request.id.as_ref(),
                    codes::METHOD_NOT_FOUND,
                    "Unsupported MCP method.",
                )]);
            }
        };
        Ok(vec![mcp::reply(
            &control,
            request.id.as_ref(),
            mcp::Outcome::Success { result },
        )])
    }
    fn cancel_mcp(&mut self, params: Value) -> Result<()> {
        #[derive(Deserialize)]
        struct Cancel {
            #[serde(rename = "requestId")]
            id: Value,
        }
        if let Some(cancel) = crate::observability::optional(
            serde_json::from_value::<Cancel>(params),
            "claude_mcp_cancel",
        ) {
            let ids = self.callbacks.matching_requests(|_, mcp| *mcp == cancel.id);
            for id in ids {
                self.callbacks.requests.remove(&id);
                self.callbacks.cancelled.insert(id);
            }
            if self.callbacks.cancelled.len() > CANCELLED_CALLBACK_LIMIT {
                return Err(protocol());
            }
        }
        Ok(())
    }
    fn mcp_tool_call(&mut self, control: String, request: wire::McpRequest) -> Result<Vec<Value>> {
        if matches!(
            self.steering_permit,
            Some(super::super::SteeringPermit::Canary)
        ) {
            self.canary_tool()?;
            return Ok(vec![mcp::error(
                &control,
                request.id.as_ref(),
                codes::SERVER_ERROR,
                "Native steering canary permits only its fixed file Read.",
            )]);
        }
        if self.admissions_closed() {
            return Ok(vec![mcp::error(
                &control,
                request.id.as_ref(),
                codes::SERVER_ERROR,
                "Agent is shutting down.",
            )]);
        }
        let turn = self.active_id()?;
        let Some(call) = crate::observability::optional(
            serde_json::from_value::<wire::ToolCall>(request.params),
            "claude_tool_call",
        ) else {
            return Ok(vec![mcp::error(
                &control,
                request.id.as_ref(),
                codes::INVALID_PARAMS,
                "Invalid tool call arguments.",
            )]);
        };
        #[cfg(feature = "comb")]
        if call.name.starts_with("comb_")
            && self.callbacks.retired_comb_turn.as_deref() == Some(&turn)
        {
            return Ok(vec![mcp::error(
                &control,
                request.id.as_ref(),
                codes::SERVER_ERROR,
                "This turn's Comb calls have been cancelled.",
            )]);
        }
        self.enqueue_tool_call(control, request.id, call, turn)
    }

    fn enqueue_tool_call(
        &mut self,
        control: String,
        mcp_id: Option<Value>,
        call: wire::ToolCall,
        turn: String,
    ) -> Result<Vec<Value>> {
        let mcp_id = mcp_id.ok_or_else(protocol)?;
        if self.callbacks.requests.len() >= MCP_CALLBACK_LIMIT {
            return Err(protocol());
        }
        self.callbacks.next_id += 1;
        let id = self.callbacks.next_id;
        let native_id =
            crate::runtime::tool_registry::request_id("claude", &turn, &(&control, &mcp_id));
        self.callbacks.requests.insert(id, (control, mcp_id));
        #[cfg(feature = "comb")]
        if call.name.starts_with("comb_") {
            self.callbacks.comb_turns.insert(id, turn.clone());
        }
        self.frames.call(control::ToolCall {
            id,
            method: ServerMethod::ToolCall,
            params: control::ToolArguments {
                thread_id: &self.config.id,
                turn_id: &turn,
                call_id: native_id,
                tool: call.name,
                arguments: call.arguments,
            },
        });
        Ok(vec![])
    }
    pub(super) fn tool_result(&mut self, frame: ToolResponse) -> Result<Vec<Value>> {
        let id = frame.id().as_u64().ok_or_else(protocol)?;
        #[cfg(feature = "comb")]
        self.callbacks.comb_turns.remove(&id);
        if self.callbacks.cancelled.remove(&id) {
            return Ok(vec![]);
        }
        let outcome = match frame {
            ToolResponse::Success { result, .. } => {
                let content = result
                    .content_items
                    .into_iter()
                    .map(|c| mcp::Text { text: c.text })
                    .collect();
                mcp::Outcome::Success {
                    result: mcp::ResultBody::Tool(mcp::ToolResult {
                        content,
                        is_error: !result.success,
                    }),
                }
            }
            ToolResponse::Failure { .. } => mcp::Outcome::Failure {
                error: mcp::Failure {
                    code: codes::INTERNAL_ERROR,
                    message: "Hive could not complete this tool request.",
                },
            },
        };
        let (control, mcp) = self.callbacks.requests.remove(&id).ok_or_else(protocol)?;
        Ok(vec![mcp::reply(&control, Some(&mcp), outcome)])
    }
}
#[cfg(test)]
mod tests;
