use super::*;

#[test]
fn parallel_children_do_not_acknowledge_input_or_supply_parent_output() {
    let mut f = Native::new(Sandbox::ReadOnly);
    let turn = f.0.turn();
    for n in 0..3 {
        let owner = format!("owner-{n}");
        f.open(&owner);
        // Both output and child replay may precede task_started.
        for frame in [
            json!({"type":"assistant", "session_id":f.0.id, "parent_tool_use_id":owner,
                "error":"rate_limit", "message":{"model":CHILD_MODEL, "content":[{"type":"text", "text":"private child reply"}]}}),
            json!({"type":"stream_event", "session_id":f.0.id, "parent_tool_use_id":owner,
                "event":{"type":"content_block_delta", "delta":{"type":"text_delta", "text":"private child stream"}}}),
            json!({"type":"user", "session_id":f.0.id, "parent_tool_use_id":owner, "isReplay":true, "uuid":turn}),
        ] {
            f.receive(frame).unwrap();
        }
        f.task(&format!("child-{n}"), &owner).unwrap();
    }
    for n in 0..3 {
        f.post(None, &format!("owner-{n}"), "Agent");
    }
    f.0.assistant(
        None,
        MODEL,
        json!([{"type":"text", "text":"parent final answer"}]),
    );
    f.finish(&turn).unwrap();
    let frames = f.drain();
    let serialized = serde_json::to_string(&frames).unwrap();
    assert!(!serialized.contains("private child"));
    assert!(!frames.iter().any(|f| f["method"] == "error"));
    assert!(serialized.contains("parent final answer"));
    assert_eq!(frames.last().unwrap()["method"], "turn/completed");
    assert!(f.0.codec.quiescent());
    // Same conversation remains usable; explicit parent evidence is retained.
    assert_ne!(f.0.turn(), turn);
}

#[test]
fn owner_closure_holds_terminal_until_late_registration_without_reopening_work() {
    let mut f = Native::new(Sandbox::ReadOnly);
    let turn = f.0.turn();
    f.open("owner");
    f.announce(Some("owner"), "read", "Read", json!({"file_path":"/tmp/x"}));
    assert!(
        f.pre(Some("child"), "read", "Read", json!({"file_path":"/tmp/x"}))
            .is_empty()
    );
    f.post(None, "owner", "Agent");
    f.finish(&turn).unwrap();
    assert!(!f.0.codec.quiescent());
    assert!(!f.drain().iter().any(|f| f["method"] == "turn/completed"));
    let replies = f.task("child", "owner").unwrap();
    assert!(replies.is_empty());
    assert_eq!(f.drain().last().unwrap()["method"], "turn/completed");
    assert!(f.0.codec.quiescent());
    f.0.turn();
    let late = f.pre(Some("child"), "late", "Read", json!({"file_path":"/tmp/x"}));
    assert_eq!(decision(&late), "deny");
}

#[test]
fn cancelled_registration_does_not_erase_observed_child_execution() {
    let mut f = Native::new(Sandbox::ReadOnly);
    let turn = f.0.turn();
    f.open("owner");
    assert!(
        f.pre(Some("child"), "read", "Read", json!({"file_path":"/tmp/x"}))
            .is_empty()
    );
    f.receive(json!({"type":"control_cancel_request", "request_id":"pre-read"}))
        .unwrap();
    f.post(None, "owner", "Agent");
    f.finish(&turn).unwrap();
    assert!(!f.0.codec.quiescent());
    assert!(!f.drain().iter().any(|f| f["method"] == "turn/completed"));
    f.task("child", "owner").unwrap();
    assert!(f.0.codec.quiescent());
}

#[test]
fn duplicate_unowned_background_or_nested_task_registration_is_rejected() {
    for patch in [
        json!({"tool_use_id":"unowned"}),
        json!({"is_backgrounded":true}),
        json!({"is_backgrounded":null}),
        json!({"spawn_depth":2}),
        json!({"spawn_depth":null}),
        json!({"task_type":"remote_agent"}),
    ] {
        let mut f = Native::new(Sandbox::ReadOnly);
        f.0.turn();
        f.open("owner");
        let mut frame = json!({"type":"system", "subtype":"task_started", "session_id":f.0.id,
            "tool_use_id":"owner", "task_id":"child", "task_type":"local_agent", "spawn_depth":1, "is_backgrounded":false});
        for (key, value) in patch.as_object().unwrap() {
            frame[key] = value.clone();
        }
        assert!(f.receive(frame).is_err(), "{patch}");
    }
    let mut f = Native::new(Sandbox::ReadOnly);
    f.0.turn();
    f.open("owner");
    f.task("child", "owner").unwrap();
    assert!(f.task("child", "owner").is_err());
}

#[test]
fn task_cannot_background_itself_or_outlive_the_parent_result() {
    let mut f = Native::new(Sandbox::ReadOnly);
    let turn = f.0.turn();
    f.open("owner");
    f.task("child", "owner").unwrap();
    assert!(
        f.receive(
            json!({"type":"system", "subtype":"task_updated", "session_id":f.0.id,
        "task_id":"child", "patch":{"is_backgrounded":true}})
        )
        .is_err()
    );
    assert!(f.finish(&turn).is_err());
}

#[test]
fn clean_boundary_requests_provider_recycling_before_identity_exhaustion() {
    let mut f = Native::new(Sandbox::ReadOnly);
    let turn = f.0.turn();
    for n in 0..2100 {
        let id = format!("read-{n}");
        f.announce(None, &id, "Read", json!({"file_path":"/tmp/x"}));
        assert_eq!(
            decision(&f.pre(None, &id, "Read", json!({"file_path":"/tmp/x"}))),
            "allow"
        );
        f.post(None, &id, "Read");
        f.drain();
    }
    assert!(!f.0.codec.recycle_ready());
    f.finish(&turn).unwrap();
    assert!(f.0.codec.recycle_ready());
}

#[test]
fn interrupt_denies_held_controls_and_retires_registered_children() {
    let mut f = Native::new(Sandbox::ReadOnly);
    let turn = f.0.turn();
    f.open("owner");
    f.task("child", "owner").unwrap();
    // A missing root identity waits rather than being inferred from absence.
    assert!(
        f.pre(None, "unidentified", "Read", json!({"file_path":"/tmp/x"}))
            .is_empty()
    );
    let replies =
        f.0.codec
            .send(outgoing::request(99, outgoing::interrupt(&f.0.id, &turn)))
            .unwrap();
    assert_eq!(decision(&replies), "deny");
    let interrupt = replies.last().unwrap()["request_id"].clone();
    f.receive(
        json!({"type":"control_response", "response":{"subtype":"success", "request_id":interrupt,
        "response":{"still_queued":[], "cancelled":[]}}}),
    )
    .unwrap();
    f.receive(
        json!({"type":"result", "session_id":f.0.id, "subtype":"success", "is_error":false,
        "terminal_reason":"aborted_tools", "user_message_uuid":turn}),
    )
    .unwrap();
    assert_eq!(
        f.drain().last().unwrap()["params"]["turn"]["status"],
        "interrupted"
    );
    assert!(f.0.codec.quiescent());
}

#[test]
fn held_permissions_are_bounded_by_count_and_bytes() {
    let mut f = Native::new(Sandbox::ReadOnly);
    f.0.turn();
    f.open("owner");
    for n in 0..64 {
        assert!(
            f.pre(Some("child"), &format!("read-{n}"), "Read", json!({}))
                .is_empty()
        );
    }
    assert_eq!(
        decision(&f.pre(Some("child"), "overflow", "Read", json!({}))),
        "deny"
    );
    let mut f = Native::new(Sandbox::ReadOnly);
    f.0.turn();
    f.open("owner");
    let oversized = json!({"file_path":"x".repeat(2 * 1024 * 1024)});
    assert_eq!(
        decision(&f.pre(Some("child"), "oversized", "Read", oversized)),
        "deny"
    );
}

#[test]
fn early_skill_and_agent_results_keep_their_owner_before_task_registration() {
    for (name, args) in [("Agent", input()), ("Skill", json!({"skill":"review"}))] {
        let mut f = Native::new(Sandbox::ReadOnly);
        let turn = f.0.turn();
        f.announce(None, "owner", name, args.clone());
        assert_eq!(decision(&f.pre(None, "owner", name, args)), "allow");
        f.announce(Some("owner"), "read", "Read", json!({"file_path":"/tmp/x"}));
        f.receive(
            json!({"type":"user", "session_id":f.0.id, "parent_tool_use_id":"owner", "message":{
            "content":[{"type":"tool_result", "tool_use_id":"read", "content":"private result"}]}}),
        )
        .unwrap();
        assert!(
            f.pre(Some("child"), "next", "Read", json!({"file_path":"/tmp/x"}))
                .is_empty()
        );
        assert_eq!(decision(&f.task("child", "owner").unwrap()), "allow");
        f.post(Some("child"), "next", "Read");
        f.post(None, "owner", name);
        f.finish(&turn).unwrap();
        assert!(f.0.codec.quiescent());
    }
}

#[test]
fn retiring_one_owner_does_not_retire_its_running_sibling() {
    let mut f = Native::new(Sandbox::WorkspaceWrite);
    f.0.turn();
    f.open("first");
    f.open("second");
    f.task("child-a", "first").unwrap();
    f.task("child-b", "second").unwrap();
    f.post(None, "first", "Agent");
    assert_eq!(
        decision(&f.pre(Some("child-a"), "late-a", "Read", json!({}))),
        "deny"
    );
    assert_eq!(
        decision(&f.pre(
            Some("child-b"),
            "bash-b",
            "Bash",
            json!({"command":"echo alive"})
        )),
        "allow"
    );
    f.post(Some("child-b"), "bash-b", "Bash");
    f.post(None, "second", "Agent");
    assert!(f.0.codec.guard.clean());
}

#[test]
fn held_parent_result_fences_previously_announced_root_tools() {
    let mut f = Native::new(Sandbox::WorkspaceWrite);
    let turn = f.0.turn();
    f.open("owner");
    let args = json!({"file_path":f.0.dir.path().canonicalize().unwrap().join("late.txt")});
    f.announce(None, "late-write", "Write", args.clone());
    f.announce(Some("owner"), "child-read", "Read", json!({}));
    f.post(None, "owner", "Agent");
    f.finish(&turn).unwrap();
    assert_eq!(decision(&f.pre(None, "late-write", "Write", args)), "deny");
    f.task("child", "owner").unwrap();
    assert_eq!(f.drain().last().unwrap()["method"], "turn/completed");
}

#[test]
fn finished_hook_identity_is_bounded_and_stream_only_ids_cannot_be_reused() {
    let mut f = Native::new(Sandbox::ReadOnly);
    let turn = f.0.turn();
    assert!(
        f.receive(
            json!({"type":"control_request", "request_id":"oversized", "request":{
        "subtype":"hook_callback", "callback_id":"hive-finished", "input":{
            "hook_event_name":"PostToolUse", "session_id":f.0.id,
            "tool_use_id":"read", "agent_id":"x".repeat(1025)}}})
        )
        .is_err()
    );
    f.receive(json!({"type":"stream_event", "session_id":f.0.id, "parent_tool_use_id":null, "event":{
        "type":"content_block_start", "content_block":{"type":"tool_use", "id":"stream-only", "name":"Read", "input":{}}}})).unwrap();
    let interrupt =
        f.0.codec
            .send(outgoing::request(102, outgoing::interrupt(&f.0.id, &turn)))
            .unwrap();
    f.receive(
        json!({"type":"control_response", "response":{"subtype":"success",
        "request_id":interrupt[0]["request_id"], "response":{"still_queued":[], "cancelled":[]}}}),
    )
    .unwrap();
    f.receive(
        json!({"type":"result", "session_id":f.0.id, "subtype":"success", "is_error":false,
        "terminal_reason":"aborted_tools", "user_message_uuid":turn}),
    )
    .unwrap();
    let interrupted = f.drain();
    assert_eq!(
        interrupted.last().unwrap()["params"]["turn"]["status"],
        "interrupted"
    );
    f.0.turn();
    assert!(f.receive(json!({"type":"assistant", "session_id":f.0.id, "parent_tool_use_id":null,
        "message":{"model":MODEL, "content":[{"type":"tool_use", "id":"stream-only", "name":"Read", "input":{}}]}})).is_err());
}

#[test]
fn steering_uses_a_parent_boundary_while_other_children_remain_open() {
    let mut f = Native::with_steering(
        Sandbox::ReadOnly,
        Some(SteeringPermit::Verified(ReceiptMode::NativeEcho)),
    );
    let turn = f.0.turn();
    f.open("owner");
    f.task("child", "owner").unwrap();
    f.0.codec
        .send(outgoing::request(
            101,
            outgoing::steer(&f.0.id, &turn, "parent guidance"),
        ))
        .unwrap();
    for (parent, agent, id) in [
        (Some("owner"), Some("child"), "child-read"),
        (None, None, "root-read"),
    ] {
        f.announce(parent, id, "Read", json!({"file_path":"/tmp/x"}));
        assert_eq!(
            decision(&f.pre(agent, id, "Read", json!({"file_path":"/tmp/x"}))),
            "allow"
        );
        let replies = f.receive(json!({"type":"control_request", "request_id":format!("post-{id}"), "request":{
            "subtype":"hook_callback", "callback_id":"hive-finished", "input":{
                "hook_event_name":"PostToolUse", "session_id":f.0.id, "tool_use_id":id, "tool_name":"Read", "agent_id":agent}}})).unwrap();
        assert_eq!(
            replies[0].to_string().contains("parent guidance"),
            parent.is_none()
        );
    }
}

#[test]
fn refusing_late_child_admission_after_interrupt_still_retains_execution_evidence() {
    let mut f = Native::new(Sandbox::ReadOnly);
    let turn = f.0.turn();
    f.open("owner");
    let interrupt =
        f.0.codec
            .send(outgoing::request(103, outgoing::interrupt(&f.0.id, &turn)))
            .unwrap();
    f.receive(json!({"type":"control_response", "response":{"subtype":"success", "request_id":interrupt[0]["request_id"],
        "response":{"still_queued":[], "cancelled":[]}}})).unwrap();
    assert_eq!(
        decision(&f.pre(Some("late-child"), "late-read", "Read", json!({}))),
        "deny"
    );
    f.receive(
        json!({"type":"result", "session_id":f.0.id, "subtype":"success", "is_error":false,
        "terminal_reason":"aborted_tools", "user_message_uuid":turn}),
    )
    .unwrap();
    assert!(
        !f.drain()
            .iter()
            .any(|frame| frame["method"] == "turn/completed")
    );
    f.task("late-child", "owner").unwrap();
    assert_eq!(f.drain().last().unwrap()["method"], "turn/completed");
    assert!(f.0.codec.quiescent());
}

#[test]
fn the_first_child_frame_after_owner_closure_or_interrupt_keeps_registration_pending() {
    for name in ["Agent", "Skill"] {
        for interrupted in [false, true] {
            for kind in ["user", "assistant", "stream_event"] {
                late_first_frame(name, interrupted, kind);
            }
        }
    }
}

fn late_first_frame(name: &str, interrupted: bool, kind: &str) {
    let mut f = Native::new(Sandbox::ReadOnly);
    let turn = f.0.turn();
    let args = if name == "Agent" {
        input()
    } else {
        json!({"skill":"review"})
    };
    f.announce(None, "owner", name, args.clone());
    assert_eq!(decision(&f.pre(None, "owner", name, args)), "allow");
    if interrupted {
        let sent =
            f.0.codec
                .send(outgoing::request(104, outgoing::interrupt(&f.0.id, &turn)))
                .unwrap();
        f.receive(json!({"type":"control_response", "response":{"subtype":"success", "request_id":sent[0]["request_id"],
            "response":{"still_queued":[], "cancelled":[]}}})).unwrap();
    } else {
        f.post(None, "owner", name);
    }
    f.receive(json!({"type":kind, "session_id":f.0.id, "parent_tool_use_id":"owner", "isReplay":true, "uuid":turn,
        "message":{"model":CHILD_MODEL, "content":[{"type":"text", "text":"late private child"}]},
        "event":{"type":"content_block_delta", "delta":{"type":"text_delta", "text":"late private stream"}}})).unwrap();
    f.receive(json!({"type":"result", "session_id":f.0.id, "subtype":"success", "is_error":false,
        "terminal_reason":if interrupted {"aborted_tools"} else {"completed"}, "user_message_uuid":turn})).unwrap();
    assert!(!f.0.codec.quiescent());
    assert!(
        !f.drain()
            .iter()
            .any(|frame| frame["method"] == "turn/completed")
    );
    f.task("late-child", "owner").unwrap();
    let completed = f.drain();
    assert_eq!(
        completed.last().unwrap()["params"]["turn"]["status"],
        if interrupted {
            "interrupted"
        } else {
            "completed"
        }
    );
    assert!(f.0.codec.quiescent());
}
