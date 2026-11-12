//! Native-wire contract tests; no provider executable or model service is run.
use super::*;
use crate::runtime::provider::protocol::outgoing;
use crate::{config::Sandbox, shell::guard::ShellGuard};
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;
mod usage;

pub(crate) struct Fixture {
    pub(crate) codec: Codec,
    dir: tempfile::TempDir,
    id: String,
    _launcher: Arc<crate::launch::LauncherCache>,
}

#[derive(Clone, Copy)]
enum TerminalCase {
    Completed,
    Interrupted,
    Failed,
}
impl TerminalCase {
    fn wire(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Interrupted => "aborted_streaming",
            Self::Failed => "failed",
        }
    }
    fn status(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Interrupted => "interrupted",
            Self::Failed => "failed",
        }
    }
}

impl Fixture {
    fn assistant(&mut self, id: Option<&str>, model: &str, content: Value) {
        self.codec
            .receive(
                json!({"type":"assistant","session_id":self.id,"parent_tool_use_id":null,
            "message":{"id":id,"model":model,"content":content}}),
            )
            .unwrap();
    }
    pub(super) fn new() -> Self {
        Self::with_steering(None)
    }
    fn with_steering(steering: Option<SteeringPermit>) -> Self {
        Self::with_id(steering, Uuid::new_v4().to_string())
    }
    pub(crate) fn with_id(steering: Option<SteeringPermit>, id: String) -> Self {
        Self::with_extensions(steering, id, |_| {})
    }
    pub(super) fn with_extensions(
        steering: Option<SteeringPermit>,
        id: String,
        configure: impl FnOnce(&mut SessionConfig),
    ) -> Self {
        Self::with_metadata(steering, id, configure, |_| {})
    }
    fn with_metadata(
        steering: Option<SteeringPermit>,
        id: String,
        configure: impl FnOnce(&mut SessionConfig),
        metadata: impl FnOnce(&mut Value),
    ) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut config = SessionConfig {
            sandbox: Sandbox::WorkspaceWrite,
            steering,
            ..SessionConfig::for_test(id.clone(), dir.path().canonicalize().unwrap())
        };
        configure(&mut config);
        let config = Arc::new(config);
        let launcher = crate::launch::test_cache().unwrap();
        let guard = ShellGuard::create(&config.cwd, &config.instructions, || launcher.executable())
            .unwrap();
        let mut codec = Codec::new(config, guard);
        let command = codec
            .send(outgoing::request(1, outgoing::initialize(false)))
            .unwrap();
        let control = &command[0]["request_id"];
        let mut initialization = json!({
            "account":{"subscriptionType":"Claude Team","apiProvider":"firstParty"},
            "agents":[{"name":"general-purpose"},{"name":"Explore"},{"name":"Plan"}],
            "models":[{"value":"sonnet","resolvedModel":"claude-sonnet-5","displayName":"Sonnet","description":"test","supportedEffortLevels":["low"]}]});
        metadata(&mut initialization);
        codec.receive(json!({"type":"control_response","response":{"subtype":"success","request_id":control,"response":initialization}})).unwrap();
        codec.pop();
        let command = codec
            .send(outgoing::request(2, outgoing::thread_resume(None, &id)))
            .unwrap();
        codec.receive(json!({"type":"control_response","response":{"subtype":"success","request_id":command[0]["request_id"],"response":{"applied":{"model":"claude-sonnet-5","effort":"low"}}}})).unwrap();
        let observed = codec.pop().unwrap();
        assert_eq!(observed["result"]["model"], "claude-sonnet-5");
        Self {
            codec,
            dir,
            id,
            _launcher: launcher,
        }
    }
    pub(crate) fn turn(&mut self) -> String {
        let outgoing = self
            .codec
            .send(outgoing::request(
                3,
                outgoing::turn_input(&self.id, "hello"),
            ))
            .unwrap();
        let turn = outgoing[0]["uuid"].as_str().unwrap().to_owned();
        self.codec.receive(json!({"type":"system","subtype":"init","session_id":self.id,"model":"claude-sonnet-5","permissionMode":"default"})).unwrap();
        self.codec
            .receive(json!({"type":"user","uuid":turn,"session_id":self.id,"parent_tool_use_id":null,"isReplay":true}))
            .unwrap();
        assert_eq!(self.codec.pop().unwrap()["id"], 3);
        assert_eq!(self.codec.pop().unwrap()["method"], "turn/started");
        turn
    }
    fn hook(&mut self, tool: &str, input: Value) -> Value {
        let reply=self.codec.receive(json!({"type":"control_request","request_id":"hook","request":{"subtype":"hook_callback","callback_id":"hive-guard","input":{
            "hook_event_name":"PreToolUse","session_id":self.id,"tool_name":tool,"tool_use_id":"tool-1","tool_input":input}}})).unwrap();
        reply[0]["response"]["response"]["hookSpecificOutput"].clone()
    }
    fn permission(&mut self, input: Value) -> Value {
        let reply=self.codec.receive(json!({"type":"control_request","request_id":"permission","request":{"subtype":"can_use_tool","tool_name":"Bash","tool_use_id":"tool-1","input":input}})).unwrap();
        reply[0]["response"]["response"]["behavior"].clone()
    }
}

mod branch;
mod skills;
mod steering;
mod streaming;

mod protocol_corpus;

mod envelopes;

mod attachments;

mod catalog_exit;
mod cleanup_evidence;
mod native_agents;
mod native_output;
mod sandbox_policy;
mod session_configuration;
mod shell_receipts;
mod tool_callbacks;
mod wire_identity;
