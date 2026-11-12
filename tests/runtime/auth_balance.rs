use super::*;

fn balanced(members: Value) -> Harness {
    let f = Harness::builder(Provider::Codex)
        .auth_group("plans", &["main", "second", "third"])
        .runtime(json!({"max_loaded":8,"max_providers":8,"max_starting":8,"max_active_turns":8}))
        .build();
    f.edit_config(|config| {
        config["auth_groups"]["plans"] = json!({"mode":"balanced","members":members});
        config["roles"]["implementor"]["auth"] = json!("plans");
    });
    f
}

fn spawn_request(f: &Harness, body: &str) -> Value {
    json!({"op":"spawn","role":"implementor","cwd":f.tmp.path(),"body":body,"request_id":body})
}

fn event(f: &Harness, id: &str, kind: &str) -> Value {
    f.events(id)
        .events
        .into_iter()
        .find(|e| e.kind == kind)
        .unwrap_or_else(|| panic!("missing {kind} event for {id}"))
        .data
}

fn idle(f: &Harness, id: &str) -> AgentView {
    f.wait(id, |a| a.state == "idle" && a.turn_id.is_none())
}

#[test]
fn balanced_spawns_rotate_split_batches_and_count_after_restart() {
    let mut f = balanced(json!(["main", "second", "third"]));
    f.start();
    let first: Vec<_> = ["reply: a", "reply: b", "reply: c"]
        .into_iter()
        .map(|body| f.spawn(body))
        .collect();
    let auths: Vec<_> = first.iter().map(|id| idle(&f, id).auth.clone()).collect();
    assert_eq!(auths, ["main", "second", "third"]);
    let decision = json!({"mode":"balanced","reason":"balanced"});
    assert_eq!(
        serde_json::to_value(&f.status(&first[1]).auth_decision).unwrap(),
        serde_json::to_value(&decision).unwrap()
    );
    let spawned = event(&f, &first[1], "spawned");
    assert_eq!(spawned["auth_group"], "plans");
    assert_eq!(spawned["auth_decision"], decision);
    f.shutdown();
    // Bound agents are durable load, so a restart continues the rotation.
    f.start();
    let next = f.spawn("reply: d");
    assert_eq!(idle(&f, &next).auth, "main");
    let batch = f.call_id(
        "balanced-batch",
        json!({"op":"batch","operations":[
            spawn_request(&f, "reply: e"),
            spawn_request(&f, "reply: f"),
            spawn_request(&f, "reply: g"),
            spawn_request(&f, "reply: h"),
        ]}),
    );
    assert!(batch["error"].is_null(), "{batch}");
    // Each assignment counts before the next one in the same transaction.
    let auths: Vec<_> = batch["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|result| {
            let agent = idle(&f, result["agent_id"].as_str().unwrap());
            assert_eq!(agent.auth_group.as_deref(), Some("plans"));
            agent.auth.clone()
        })
        .collect();
    assert_eq!(auths, ["second", "third", "main", "second"]);
    f.shutdown();
}

#[test]
fn balanced_limit_moves_only_agents_with_input_and_spreads_them() {
    let mut f = balanced(json!([{"profile":"main","weight":1000},"second","third"]));
    f.set_scenario(Provider::Codex, json!({"quota_body":"reply: limited"}));
    f.start();
    let resting = f.spawn("reply: resting");
    assert_eq!(idle(&f, &resting).auth, "main");
    let limited = f.spawn("reply: limited");
    // The cut-off turn leaves a continuation, so the agent moves at its boundary.
    let moved = f.wait(&limited, |a| {
        a.auth == "second" && a.state == "idle" && a.turn_id.is_none()
    });
    assert!(moved.handoff.as_deref().unwrap().contains("limited"));
    let limited_move = json!({"mode":"balanced","reason":"limited_move"});
    assert_eq!(
        serde_json::to_value(&moved.auth_decision).unwrap(),
        serde_json::to_value(&limited_move).unwrap()
    );
    let switched = event(&f, &limited, "auth_switched");
    assert_eq!(
        (&switched["auth_group"], &switched["auth_decision"]),
        (&json!("plans"), &limited_move)
    );
    // Without input the idle agent keeps its login and conversation.
    let before = f.status(&resting);
    assert_eq!(before.auth, "main");
    assert!(before.waiting_reason.is_none(), "{before:?}");
    assert!(
        !f.read(&resting)
            .events
            .iter()
            .any(|e| e.kind == "auth_switched")
    );
    // New input moves it to the lightest remaining member.
    let sent = f.sent(json!({"op":"send","target":resting,"body":"reply: later"}));
    let message = sent.message_id.as_str();
    let after = f.wait(&resting, |a| a.auth == "third");
    assert_ne!(after.thread_id, before.thread_id);
    assert_eq!(
        serde_json::to_value(&after.auth_decision).unwrap(),
        serde_json::to_value(&limited_move).unwrap()
    );
    support::wait::wait_until(
        support::wait::TEST_TIMEOUT,
        support::wait::POLL_INTERVAL,
        "moved input completion",
        || {
            (f.ok(json!({"op":"message_result","message_id":message}))["delivery_state"]
                == "completed")
                .then_some(())
        },
    );
    f.shutdown();
}
