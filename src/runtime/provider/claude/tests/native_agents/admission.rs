use super::*;

#[test]
fn foreground_arguments_are_normalized_and_catalog_checked_on_both_paths() {
    for sandbox in [
        Sandbox::ReadOnly,
        Sandbox::WorkspaceWrite,
        Sandbox::FullAccess,
    ] {
        for name in ["Agent", "Task"] {
            let mut f = Native::new(sandbox);
            f.0.turn();
            f.announce(None, "owner", name, input());
            let replies = f.pre(None, "owner", name, input());
            assert_eq!(decision(&replies), "allow");
            let normalized =
                replies[0]["response"]["response"]["hookSpecificOutput"]["updatedInput"].clone();
            assert_eq!(normalized["run_in_background"], false);
            let permission = f.receive(json!({"type":"control_request", "request_id":"permission", "request":{
                "subtype":"can_use_tool", "tool_name":name, "tool_use_id":"owner", "input":normalized
            }})).unwrap();
            assert_eq!(permission[0]["response"]["response"]["behavior"], "allow");
        }
    }
}

#[test]
fn unsupported_or_malformed_native_arguments_are_model_visible_denials() {
    for (field, value) in [
        ("run_in_background", json!(true)),
        ("run_in_background", json!(null)),
        ("isolation", json!("worktree")),
        ("mode", json!("bypassPermissions")),
        ("name", json!("worker")),
        ("team_name", json!("team")),
        ("effort", json!("high")),
        ("unknown", json!(false)),
        ("subagent_type", json!("unoffered")),
        ("subagent_type", json!(null)),
        ("model", json!("unoffered")),
        ("model", json!(null)),
        ("prompt", json!(" ")),
        ("description", json!(3)),
    ] {
        let mut f = Native::new(Sandbox::FullAccess);
        f.0.turn();
        let mut args = input();
        args[field] = value;
        f.announce(None, "owner", "Agent", args.clone());
        assert_eq!(
            decision(&f.pre(None, "owner", "Agent", args)),
            "deny",
            "{field}"
        );
        assert!(f.task("child", "owner").is_err());
    }
}

#[test]
fn children_keep_the_parent_sandbox_and_external_mcp_but_cannot_delegate_or_call_hosted_tools() {
    for sandbox in [
        Sandbox::ReadOnly,
        Sandbox::WorkspaceWrite,
        Sandbox::FullAccess,
    ] {
        let mut f = Native::new(sandbox);
        f.0.turn();
        f.open("owner");
        f.task("child", "owner").unwrap();
        let inside = f.0.dir.path().canonicalize().unwrap().join("ordinary.txt");
        let cases = [
            ("Read", json!({"file_path":"/tmp/ordinary"}), true),
            (
                "Write",
                json!({"file_path":inside}),
                sandbox != Sandbox::ReadOnly,
            ),
            (
                "Write",
                json!({"file_path":"/outside/workspace/x"}),
                sandbox == Sandbox::FullAccess,
            ),
            ("Bash", json!({"command":"git status"}), true),
            (
                "Bash",
                json!({"command":"git status; touch x"}),
                sandbox != Sandbox::ReadOnly,
            ),
            (
                "Bash",
                json!({"command":"git status", "run_in_background":true}),
                false,
            ),
            (
                "Bash",
                json!({"command":"git status", "dangerouslyDisableSandbox":true}),
                sandbox == Sandbox::FullAccess,
            ),
            ("Agent", input(), false),
            ("Skill", json!({"skill":"review"}), false),
            ("mcp__hive__hive_agents", json!({}), false),
            ("mcp__hive__comb_get", json!({}), false),
            ("mcp__docs__lookup", json!({}), true),
            ("TaskOutput", json!({}), false),
            ("TaskStop", json!({}), false),
        ];
        for (n, (name, args, allowed)) in cases.into_iter().enumerate() {
            let id = format!("child-tool-{n}");
            f.announce(Some("owner"), &id, name, args.clone());
            let replies = f.pre(Some("child"), &id, name, args);
            assert_eq!(
                decision(&replies),
                if allowed { "allow" } else { "deny" },
                "{sandbox:?} {name}"
            );
            if allowed {
                f.post(Some("child"), &id, name);
            }
        }
    }
}

#[test]
fn child_controls_wait_for_exact_task_registration_even_when_registration_order_crosses() {
    let mut f = Native::new(Sandbox::ReadOnly);
    f.0.turn();
    f.open("owner-a");
    f.open("owner-b");
    for (owner, child, tool) in [
        ("owner-a", "child-a", "read-a"),
        ("owner-b", "child-b", "read-b"),
    ] {
        f.announce(Some(owner), tool, "Read", json!({"file_path":"/tmp/x"}));
        assert!(
            f.pre(Some(child), tool, "Read", json!({"file_path":"/tmp/x"}))
                .is_empty()
        );
    }
    let second = f.task("child-b", "owner-b").unwrap();
    assert_eq!(decision(&second), "allow");
    assert_eq!(second[0]["response"]["request_id"], "pre-read-b");
    let first = f.task("child-a", "owner-a").unwrap();
    assert_eq!(decision(&first), "allow");
    assert_eq!(first[0]["response"]["request_id"], "pre-read-a");
    // A child cannot reuse another task's tool identity.
    assert!(f.receive(json!({"type":"control_request", "request_id":"conflict", "request":{
        "subtype":"can_use_tool", "tool_name":"Read", "tool_use_id":"read-a", "agent_id":"child-b", "input":{}
    }})).is_err());
}

#[test]
fn parent_hosted_admission_waits_for_positive_root_evidence() {
    let mut f = Native::new(Sandbox::ReadOnly);
    f.0.turn();
    assert!(
        f.pre(None, "hosted", "mcp__hive__hive_agents", json!({}))
            .is_empty()
    );
    let replies = f.receive(json!({"type":"assistant", "session_id":f.0.id, "parent_tool_use_id":null,
        "message":{"model":MODEL, "content":[{"type":"tool_use", "id":"hosted", "name":"mcp__hive__hive_agents", "input":{}}]}})).unwrap();
    assert_eq!(decision(&replies), "allow");
}
