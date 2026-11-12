use super::*;
use crate::runtime::provider::claude::tests::Fixture;
use serde_json::json;

fn owner_fixture(name: &str) -> Fixture {
    let mut f = Fixture::with_extensions(None, uuid::Uuid::new_v4().to_string(), |config| {
        std::sync::Arc::make_mut(&mut config.options)
            .values
            .insert("claude.skills".into(), json!(true));
    });
    f.turn();
    let session = f.codec.config.id.clone();
    let input = if name == "Agent" {
        json!({"description":"private", "prompt":"private"})
    } else {
        json!({"skill":"review"})
    };
    f.codec.receive(json!({"type":"assistant", "session_id":session, "parent_tool_use_id":null,
        "message":{"model":"claude-sonnet-5", "content":[{"type":"tool_use", "id":"owner", "name":name, "input":input}]}})).unwrap();
    f.codec.receive(json!({"type":"control_request", "request_id":"pre-owner", "request":{
        "subtype":"hook_callback", "callback_id":"hive-guard", "input":{"hook_event_name":"PreToolUse",
            "session_id":f.codec.config.id, "tool_use_id":"owner", "tool_name":name,
            "tool_input":input}}})).unwrap();
    f
}

fn pending_fixture() -> Fixture {
    let mut f = owner_fixture("Agent");
    assert!(f.codec.receive(json!({"type":"control_request", "request_id":"held", "request":{
        "subtype":"can_use_tool", "tool_name":"Read", "tool_use_id":"child-read", "agent_id":"child", "input":{}}})).unwrap().is_empty());
    f
}

#[test]
fn a_first_child_frame_after_owner_retirement_restores_bounded_codec_proof() {
    for name in ["Agent", "Skill"] {
        let mut f = owner_fixture(name);
        f.codec.close_foreground();
        assert!(!f.codec.tools.tasks.proof_pending());
        f.codec
            .receive(json!({"type":"assistant", "session_id":f.codec.config.id,
            "parent_tool_use_id":"owner", "message":{"model":"claude-sonnet-5", "content":[]}}))
            .unwrap();
        assert!(f.codec.tools.tasks.proof_pending());
        let error = f
            .codec
            .resolve_native_controls_at(Instant::now() + REGISTRATION_WAIT)
            .unwrap_err();
        assert_eq!(error.code(), "provider_protocol_error");
        assert!(!f.codec.quiescent());
    }
}

fn registration(owner: &str, task: &str) -> wire::System {
    serde_json::from_value(serde_json::json!({"tool_use_id":owner, "task_id":task,
        "task_type":"local_agent", "spawn_depth":1, "is_backgrounded":false}))
    .unwrap()
}

#[test]
fn observed_unbound_owner_times_out_without_forgetting_execution() {
    let mut tasks = Tasks::default();
    tasks.open("owner", "Agent").unwrap();
    tasks.frame_scope("owner").unwrap();
    let now = Instant::now();
    tasks.close("owner", now);
    assert!(tasks.proof_pending());
    assert!(!tasks.expired(now));
    assert!(tasks.expired(now + REGISTRATION_WAIT));
    tasks.register(&registration("owner", "child")).unwrap();
    assert!(!tasks.proof_pending());
    assert!(tasks.child_scope("child").is_none());
    assert!(tasks.retired_child("child"));
}

#[test]
fn an_unbound_control_cannot_be_cancelled_out_of_the_execution_proof() {
    let mut tasks = Tasks::default();
    tasks.open("owner", "Agent").unwrap();
    let now = Instant::now();
    tasks.observe_unresolved("child", now);
    tasks.close("owner", now);
    assert!(tasks.expired(now + REGISTRATION_WAIT));
    tasks.register(&registration("owner", "child")).unwrap();
    assert!(!tasks.proof_pending());
}

#[test]
fn inline_skill_and_an_agent_with_no_observed_child_can_settle_without_registration() {
    for name in ["Skill", "Agent"] {
        let mut tasks = Tasks::default();
        tasks.open("owner", name).unwrap();
        tasks.close("owner", Instant::now());
        assert!(!tasks.proof_pending());
        tasks
            .register(&registration("owner", "late-child"))
            .unwrap();
        assert!(tasks.retired_child("late-child"));
        assert!(tasks.frame_scope("owner").unwrap().is_none());
    }
}

#[test]
fn outstanding_owners_and_one_forked_skill_have_independent_bounds() {
    let mut tasks = Tasks::default();
    tasks.open("skill", "Skill").unwrap();
    assert!(!tasks.can_open("Skill"));
    for n in 1..TASK_LIMIT {
        tasks.open(&format!("agent-{n}"), "Agent").unwrap();
    }
    assert!(!tasks.can_open("Agent"));
    tasks.close("agent-1", Instant::now());
    assert!(tasks.can_open("Agent"));
    assert!(tasks.open("agent-1", "Agent").is_err());
}

#[test]
fn a_retired_owner_reconciles_at_most_one_native_task() {
    for register_before_close in [false, true] {
        let mut tasks = Tasks::default();
        tasks.open("owner", "Agent").unwrap();
        if register_before_close {
            tasks.register(&registration("owner", "child-a")).unwrap();
        }
        tasks.close("owner", Instant::now());
        if !register_before_close {
            tasks.register(&registration("owner", "child-a")).unwrap();
        }
        assert!(tasks.register(&registration("owner", "child-b")).is_err());
    }
}

#[test]
fn permission_timeout_denies_the_payload_but_keeps_unregistered_execution_evidence() {
    let mut f = pending_fixture();
    let replies = f
        .codec
        .resolve_native_controls_at(Instant::now() + REGISTRATION_WAIT)
        .unwrap();
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0]["response"]["request_id"], "held");
    assert_eq!(replies[0]["response"]["response"]["behavior"], "deny");
    assert!(f.codec.tools.pending.is_empty());
    assert!(f.codec.tools.tasks.proof_pending());
}

#[test]
fn a_missing_registration_produces_codec_failure_after_owner_settlement() {
    let mut f = pending_fixture();
    f.codec.close_foreground();
    let owner = f.codec.tools.tasks.owners.get_mut("owner").unwrap();
    owner.closed = Some(
        Instant::now()
            .checked_sub(REGISTRATION_WAIT)
            .expect("fixture time can subtract registration wait"),
    );
    assert!(f.codec.poll_native_controls().is_err());
    assert!(!f.codec.quiescent());
}

#[test]
fn hard_identity_exhaustion_is_an_explicit_output_limit() {
    let mut f = Fixture::new();
    f.turn();
    f.codec
        .tools
        .seen
        .extend((0..super::super::limits::SEEN_TOOL_LIMIT).map(|n| format!("retired-{n}")));
    let error = f
        .codec
        .origin("new", "Read", Scope::Parent, true)
        .unwrap_err();
    assert_eq!(error.code(), "output_limit");
}

#[test]
fn native_agent_denial_keeps_its_exact_message() {
    assert_eq!(
        super::admission::AGENT_REASON,
        "Native agents accept only description, prompt, subagent_type, model and \
        run_in_background:false. Use Hive for visible delegation; named, nested, \
        isolated and background agents are unavailable."
    );
}
