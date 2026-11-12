//! Deterministic native-wire cases, not recordings of real model behavior.
use super::*;

mod admission;
mod execution;
mod hosted;
mod lifecycle;

const MODEL: &str = "claude-sonnet-5";
const CHILD_MODEL: &str = "claude-haiku-5";

struct Native(Fixture);

impl Native {
    fn new(sandbox: Sandbox) -> Self {
        Self::with_steering(sandbox, None)
    }
    fn with_steering(sandbox: Sandbox, steering: Option<SteeringPermit>) -> Self {
        Self(Fixture::with_metadata(
            steering,
            Uuid::new_v4().to_string(),
            |config| {
                config.sandbox = sandbox;
                Arc::make_mut(&mut config.options)
                    .values
                    .insert("claude.skills".into(), json!(true));
                config.mcp_servers.insert(
                    "docs".into(),
                    crate::config::McpServer {
                        command: "fixture".into(),
                        args: vec![],
                        env: std::collections::BTreeMap::default(),
                    },
                );
            },
            |metadata| {
                metadata["models"].as_array_mut().unwrap().push(json!({
                "value":"haiku", "resolvedModel":CHILD_MODEL, "displayName":"Haiku", "description":"test", "supportedEffortLevels":["low"]
            }));
            },
        ))
    }
    fn receive(&mut self, frame: Value) -> Result<Vec<Value>> {
        let replies = self.0.codec.receive(frame)?;
        // The native transport echoes successful hook/control replies. Consume
        // those echoes, so long traces exercise identity bounds rather than the
        // separate small transport-echo bound.
        for reply in &replies {
            assert!(self.0.codec.receive(reply.clone())?.is_empty());
        }
        Ok(replies)
    }
    fn announce(&mut self, parent: Option<&str>, id: &str, name: &str, input: Value) {
        self.receive(json!({"type":"assistant", "session_id":self.0.id,
            "parent_tool_use_id":parent, "message":{"model":if parent.is_some() {CHILD_MODEL} else {MODEL},
            "content":[{"type":"tool_use", "id":id, "name":name, "input":input}]}})).unwrap();
    }
    fn pre(&mut self, agent: Option<&str>, id: &str, name: &str, input: Value) -> Vec<Value> {
        self.receive(
            json!({"type":"control_request", "request_id":format!("pre-{id}"), "request":{
            "subtype":"hook_callback", "callback_id":"hive-guard", "input":{
                "hook_event_name":"PreToolUse", "session_id":self.0.id, "tool_name":name,
                "tool_use_id":id, "agent_id":agent, "tool_input":input
            }}}),
        )
        .unwrap()
    }
    fn post(&mut self, agent: Option<&str>, id: &str, name: &str) {
        self.receive(
            json!({"type":"control_request", "request_id":format!("post-{id}"), "request":{
            "subtype":"hook_callback", "callback_id":"hive-finished", "input":{
                "hook_event_name":"PostToolUse", "session_id":self.0.id,
                "tool_name":name, "tool_use_id":id, "agent_id":agent
            }}}),
        )
        .unwrap();
    }
    fn task(&mut self, task: &str, owner: &str) -> Result<Vec<Value>> {
        self.receive(
            json!({"type":"system", "subtype":"task_started", "session_id":self.0.id,
            "task_id":task, "tool_use_id":owner, "task_type":"local_agent",
            "spawn_depth":1, "is_backgrounded":false}),
        )
    }
    fn open(&mut self, owner: &str) {
        let input = input();
        self.announce(None, owner, "Agent", input.clone());
        assert_eq!(decision(&self.pre(None, owner, "Agent", input)), "allow");
    }
    fn finish(&mut self, turn: &str) -> Result<Vec<Value>> {
        self.receive(
            json!({"type":"result", "session_id":self.0.id, "subtype":"success",
            "is_error":false, "terminal_reason":"completed", "user_message_uuid":turn}),
        )
    }
    fn drain(&mut self) -> Vec<Value> {
        std::iter::from_fn(|| self.0.codec.pop()).collect()
    }
}

fn input() -> Value {
    json!({"description":"Private calculation", "prompt":"Calculate privately.", "subagent_type":"general-purpose", "model":"haiku"})
}

fn decision(replies: &[Value]) -> &str {
    replies[0]["response"]["response"]["hookSpecificOutput"]["permissionDecision"]
        .as_str()
        .unwrap()
}
