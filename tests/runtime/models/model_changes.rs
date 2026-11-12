//! Model changes regressions.
use super::*;

#[test]
fn model_change_can_correct_a_rejected_unsent_reservation() {
    let mut f = Harness::builder(Provider::Codex)
        .scenario(json!({"catalog_once_model":"unknown"}))
        .build();
    f.start();
    let spawned = f.spawned(json!({"op":"spawn","role":"implementor","cwd":f.tmp.path(),
        "body":"hold","model":"unknown"}));
    let id = spawned.agent_id.as_str();
    let failed = f.wait(id, |a| a.state == "failed");
    assert_eq!(failed.error.as_deref(), Some("unknown_model"));
    assert_eq!(
        failed.reserved_turn.as_ref().unwrap().selection.model,
        "unknown"
    );
    change(&f, id, "gpt-5.6-luna", "medium");
    assert!(f.status(id).reserved_turn.is_none());
    assert!(f.resume_when_released(id)["error"].is_null());
    let active = f.wait(id, |a| a.state == "running");
    assert_eq!(
        active.turn_selection.as_ref().unwrap().model,
        "gpt-5.6-luna"
    );
    let messages = f.mailbox(id);
    assert_eq!(messages.messages.len(), 1);
    f.shutdown();
}

#[test]
fn model_change_is_durable_deduplicated_and_waits_for_the_next_turn() {
    let mut f = Harness::new();
    f.start();
    let id = f.spawn("hold");
    let running = f.wait(&id, |a| a.state == "running");
    let request = json!({"op":"set_options","target":id,"options":{"model":"gpt-5.6-luna",
        "effort":"medium"}});
    let changed = f.call_id("model-change", request.clone());
    assert!(changed["error"].is_null(), "{changed}");
    assert_eq!(f.call_id("model-change", request.clone()), changed);
    let pending = f.status(&id);
    assert_eq!(pending.model, "gpt-5.6-luna");
    assert_eq!(
        pending.turn_selection.as_ref().unwrap().model,
        "gpt-5.6-terra"
    );
    assert_eq!(pending.turn_id, running.turn_id);
    let rejected = f.expect_err(
        json!({"op":"set_options","target":id,"options":{"model":"unknown"}}),
        "invalid_value",
    );
    assert_eq!(rejected.error.field.as_deref().unwrap(), "/options/model");
    let rejected = f.expect_err(
        json!({"op":"set_options","target":id,"options":{"model":"gpt-5.6-sol","effort":"bad"}}),
        "invalid_value",
    );
    assert_eq!(rejected.error.field.as_deref().unwrap(), "/options/effort");
    f.ok(json!({"op":"interrupt","target":id}));
    f.wait(&id, |a| a.state == "idle");
    f.ok(json!({"op":"send","target":id,"body":"hold"}));
    let next = f.wait(&id, |a| a.state == "running");
    assert_eq!(next.thread_id, running.thread_id);
    assert_eq!(next.execution_id, running.execution_id);
    assert_eq!(next.turn_selection.as_ref().unwrap().model, "gpt-5.6-luna");
    assert_eq!(next.turn_selection.as_ref().unwrap().effort, "medium");
    let requests: Vec<Value> = fs::read_to_string(f.provider_home.join("turn-selections.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(requests[1]["model"], "gpt-5.6-luna");
    assert_eq!(requests[1]["effort"], "medium");
    f.ok(json!({"op":"stop","target":id}));
    f.wait(&id, |a| a.state == "stopped");
    f.shutdown();
    f.start();
    assert_eq!(f.call_id("model-change", request), changed);
    let stored = f.status(&id);
    assert_eq!(stored.options_revision, 1);
    assert_eq!(stored.model, "gpt-5.6-luna");
    let omitted = f.ok(json!({"op":"set_options","target":id,"options":{"model":"gpt-5.6-sol"}}));
    assert_eq!(omitted["desired"]["effort"], "medium", "{omitted}");
    assert!(f.status(&id).paused);
    f.shutdown();
}

#[test]
fn model_change_during_turn_start_keeps_request_and_audit_snapshot_together() {
    let mut f = Harness::new();
    let gate = f.tmp.path().join("start-gate");
    let _release_on_failure = support::ReleaseFile::new(&gate);
    f.set_scenario(Provider::Codex, json!({"turn_start_gate":gate}));
    f.start();
    let id = f.spawn("hold");
    f.wait(&id, |a| {
        a.turn_selection
            .as_ref()
            .is_some_and(|selection| selection.model == "gpt-5.6-terra")
    });
    change(&f, &id, "gpt-5.6-sol", "medium");
    fs::write(gate, "go").unwrap();
    let active = f.wait(&id, |a| a.state == "running");
    assert_eq!(active.turn_selection.as_ref().unwrap().revision, 0);
    assert_eq!(active.options_revision, 1);
    let events = f.read(&id);
    let started = events
        .events
        .iter()
        .find(|e| e.kind == "turn_started")
        .unwrap();
    assert_eq!(started.data["model"], "gpt-5.6-terra");
    assert_eq!(started.data["options_revision"], 0);
    assert_eq!(
        started.data["selection_verification"],
        "requested_unverified"
    );
    f.shutdown();
}
