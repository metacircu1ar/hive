//! Shell receipts regressions.
use super::*;

#[test]
fn shell_permission_requires_exact_issued_input_once() {
    let mut f = Fixture::new();
    f.turn();
    assert_eq!(f.permission(json!({"command":"echo hi"})), "deny");
    let output = f.hook("Bash", json!({"command":"echo hi"}));
    assert_eq!(output["permissionDecision"], "allow");
    let allowed = output["updatedInput"].clone();
    let mut forged = allowed.clone();
    forged["command"] = json!(format!(
        "{}; echo extra",
        forged["command"].as_str().unwrap()
    ));
    assert_eq!(f.permission(forged), "deny");
    {
        let id = json!("other-tool");
        let reply = f.codec.receive(json!({"type":"control_request","request_id":"bad-id",
            "request":{"subtype":"can_use_tool","tool_name":"Bash","tool_use_id":id,"input":allowed}})).unwrap();
        assert_eq!(reply[0]["response"]["response"]["behavior"], "deny");
    }
    assert_eq!(f.permission(allowed.clone()), "allow");
    assert_eq!(f.permission(allowed), "deny");
}

#[test]
fn read_only_bash_runs_only_the_rendered_git_command_once() {
    let mut f = Fixture::with_extensions(None, Uuid::new_v4().to_string(), |config| {
        config.sandbox = Sandbox::ReadOnly;
    });
    f.turn();
    for refused in [
        json!({"command":"echo hi"}),
        json!({"command":"git status; touch pwned"}),
        json!({"command":"git status","run_in_background":true}),
        json!({"command":"git status","dangerouslyDisableSandbox":true}),
    ] {
        let output = f.hook("Bash", refused.clone());
        assert_eq!(output["permissionDecision"], "deny", "{refused}");
        assert_eq!(
            output["permissionDecisionReason"],
            "Read-only agents may run one allowlisted read-only git command without shell syntax."
        );
    }
    let output = f.hook("Bash", json!({"command":"git status --porcelain"}));
    assert_eq!(output["permissionDecision"], "allow");
    let allowed = output["updatedInput"].clone();
    let command = allowed["command"].as_str().unwrap();
    // The agent's text never reaches the shell: only the checked rendering does.
    assert!(
        command.ends_with(
            "\nGIT_OPTIONAL_LOCKS=0 GIT_PAGER=cat PAGER=cat GIT_TERMINAL_PROMPT=0 \
             GIT_NO_LAZY_FETCH=1 exec git --no-pager -c core.fsmonitor=false '\\''status'\\'' '\\''--porcelain'\\'''"
        ),
        "{command}"
    );
    assert_eq!(
        f.permission(json!({"command":"git status --porcelain"})),
        "deny"
    );
    assert_eq!(f.permission(allowed.clone()), "allow");
    assert_eq!(f.permission(allowed), "deny");
}

#[test]
fn sealed_guard_denies_late_tools_without_creating_obligations() {
    let mut f = Fixture::new();
    let turn = f.turn();
    f.codec.guard.seal();
    assert_eq!(
        f.hook("Bash", json!({"command":"echo forbidden"}))["permissionDecision"],
        "deny"
    );
    assert_eq!(f.permission(json!({"command":"echo forbidden"})), "deny");
    assert!(f.codec.guard.clean());
    // Completing the turn proves no tool obligation was inserted.
    f.codec.receive(json!({"type":"result","session_id":f.id,"subtype":"success","is_error":false,"terminal_reason":"completed","user_message_uuid":turn})).unwrap();
    assert!(f.codec.quiescent());
}

#[test]
fn shell_permission_without_native_id_requires_exact_unique_unspent_envelope() {
    let mut f = Fixture::new();
    f.turn();
    let input = f.hook("Bash", json!({"command":"echo hi"}))["updatedInput"].clone();
    for expected in ["allow", "deny"] {
        let reply = f
            .codec
            .receive(json!({"type":"control_request","request_id":"missing-id",
            "request":{"subtype":"can_use_tool","tool_name":"Bash","input":input}}))
            .unwrap();
        assert_eq!(reply[0]["response"]["response"]["behavior"], expected);
    }
}
