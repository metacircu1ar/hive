use super::*;
use serde_json::json;

#[test]
fn native_denial_names_are_static_lowercase_tokens_in_the_actual_backend() {
    const CHILD: &str = "HIVE_ISOLATED_CLAUDE_DENIAL";
    if std::env::var_os(CHILD).is_none() {
        let (_, module) = module_path!().split_once("::").unwrap();
        let name = concat!(
            "native_denial_names_are_static_lowercase_tokens",
            "_in_the_actual_backend"
        );
        crate::test_support::run_isolated(
            crate::test_support::rerun_isolated(&format!("{module}::{name}"), false)
                .env(CHILD, "1"),
        );
        return;
    }
    let directory = crate::runtime::test_directory();
    let path = directory.path().join("denials.jsonl");
    let (backend, guard) = hive_log::start(&path, hive_log::Config::default()).unwrap();
    let count = std::sync::atomic::AtomicU64::new(0);
    backend.scope(|| {
        log_denied("Bash", "permission", &count);
        log_denied("NotebookEdit", "permission", &count);
        log_denied("Read", "permission", &count);
        log_denied("private-tool-canary", "permission", &count);
    });
    assert_eq!(
        guard.finish(std::time::Duration::from_secs(2)).state,
        hive_log::Shutdown::Drained
    );
    let text = std::fs::read_to_string(path).unwrap();
    assert!(!text.contains("private-tool-canary"));
    let rows: Vec<Value> = text
        .lines()
        .map(|r| serde_json::from_str(r).unwrap())
        .collect();
    assert_eq!(rows.len(), 3, "{text}");
    for (row, method) in rows.iter().zip(["bash", "notebook_edit", "other"]) {
        assert_eq!(row["event"], "tool_policy.denied");
        assert_eq!(row["method"], method);
    }
}

fn guard_input(session: &str, agent: Option<&str>) -> wire::Hook {
    serde_json::from_value(json!({
        "hook_event_name":"PreToolUse", "session_id":session,
        "agent_id":agent, "tool_name":"Read", "tool_input":{}
    }))
    .unwrap()
}

#[test]
fn closed_guard_validates_identity_but_denies_before_turn_and_tool_id_requirements() {
    let mut fixture = crate::runtime::provider::claude::tests::Fixture::new();
    let session = fixture.codec.config.id.clone();
    fixture.codec.guard.seal();
    // No active turn and no tool_use_id: a valid closed guard still returns its denial.
    let payload = fixture
        .codec
        .hook(
            wire::CallbackId::Guard,
            guard_input(&session, None),
            Scope::Parent,
        )
        .unwrap();
    let Payload::Hook(reply) = payload else {
        panic!("expected closed-guard hook denial")
    };
    assert!(matches!(
        reply.hook_specific_output.permission_decision,
        control::PermissionDecision::Deny
    ));
    assert_eq!(
        reply.hook_specific_output.permission_decision_reason,
        "Agent is shutting down."
    );
    assert!(reply.hook_specific_output.updated_input.is_none());
    assert!(fixture.codec.turn.active.is_none());
    assert!(fixture.codec.tools.active.is_empty());
    assert!(fixture.codec.tools.owners.is_empty());
    assert!(fixture.codec.tools.shell_permits.is_empty());
    assert!(fixture.codec.guard.clean());
    let mut invalid_tool = guard_input(&session, None);
    invalid_tool.tool_use_id = Some(String::new());
    for invalid in [
        guard_input(&session, Some("")),
        guard_input("wrong-session", None),
        invalid_tool,
    ] {
        assert!(
            fixture
                .codec
                .hook(wire::CallbackId::Guard, invalid, Scope::Parent)
                .is_err()
        );
    }
}

fn parity(payload: Payload) {
    let old = control::reply("request", crate::api::value(&payload)).to_string();
    let new = control::reply("request", payload).to_string();
    assert_eq!(new, old);
}

#[test]
fn permissions_hooks_guidance_and_empty_replies_keep_full_native_envelope_bytes() {
    parity(Payload::empty());
    parity(Payload::Permission(control::Permission::Allow {
        input: json!({"text":"é\n\"\0","n":u64::MAX,"nested":[null,1.0]}),
    }));
    parity(Payload::Permission(control::Permission::Deny {
        message: "reason",
    }));
    for updated_input in [None, Some(Value::Null), Some(json!({"command":"opaque"}))] {
        parity(Payload::Hook(control::HookOutput {
            hook_specific_output: control::HookDecision {
                permission_decision: control::PermissionDecision::Deny,
                updated_input,
                permission_decision_reason: "reason",
            },
        }));
    }
    parity(Payload::Guidance(super::super::emitted::Guidance {
        hook_specific_output: super::super::emitted::Context {
            additional_context: "é\n\"\0".into(),
        },
    }));
    assert_eq!(
        control::reply("request", Payload::empty()).to_string(),
        concat!(
            "{\"response\":{\"request_id\":\"request\",\"response\":{},",
            "\"subtype\":\"success\"},\"type\":\"control_response\"}"
        )
    );
}
