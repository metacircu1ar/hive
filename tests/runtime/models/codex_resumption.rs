//! Codex resumption regressions.
use super::*;

#[test]
fn codex_session_change_drains_a_process_that_retains_the_unsubscribed_thread() {
    let mut f = Harness::builder(Provider::Codex)
        .scenario(json!({
            "provider_version":"0.163.0",
            "retain_loaded_after_unsubscribe":true
        }))
        .build();
    f.start();
    let id = f.spawn("hold");
    let original = f.wait(&id, |a| a.state == "running");
    let first: Value = serde_json::from_str(
        fs::read_to_string(f.provider_home.join("thread-selections.jsonl"))
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    let old_pid = u32::try_from(first["pid"].as_u64().unwrap()).unwrap();
    let old_process = hive::state::ProcessIdentity::read(old_pid)
        .unwrap()
        .unwrap();

    f.ok(json!({"op":"set_options","target":id,
        "options":{"codex.auto_compact_token_limit":200_000}}));
    f.ok(json!({"op":"send","target":id,"body":"hold"}));
    // The resident source retains its startup scenario. The destination reads
    // this update, allowing ordinary cleanup after the fencing assertion.
    f.update_scenario(Provider::Codex, |scenario| {
        scenario["retain_loaded_after_unsubscribe"] = json!(false);
    });
    f.ok(json!({"op":"interrupt","target":id}));
    let reconfigured = f.wait_timeout(&id, Duration::from_secs(20), |a| {
        a.state == "running" && a.execution_id != original.execution_id
    });
    assert_eq!(reconfigured.thread_id, original.thread_id);
    assert!(old_process.absent().unwrap());
    let starts: Vec<Value> = fs::read_to_string(f.provider_home.join("thread-selections.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(starts.len(), 2, "{starts:?}");
    assert_ne!(starts[0]["pid"], starts[1]["pid"]);
    assert_eq!(starts[1]["method"], "thread/resume");
    assert_eq!(
        starts[1]["config"]["model_auto_compact_token_limit"],
        200_000
    );
    f.shutdown();
}

#[test]
fn process_option_change_waits_for_unload_before_resuming_elsewhere() {
    #[derive(Debug, serde::Deserialize)]
    struct ThreadSelection {
        method: String,
        thread: String,
        at_ns: u64,
    }
    let mut f = Harness::builder(Provider::Codex)
        .runtime(json!({"sessions_per_codex_process":4,"max_providers":2,"idle_seconds":600}))
        .scenario(json!({"unload_after_unsubscribe_ms":12000}))
        .build();
    f.start();
    let sibling = f.spawn("idle");
    f.settled(&sibling);
    let agent = f.spawn("idle");
    let before = f.settled(&agent);
    let thread = before.thread_id.as_deref().unwrap();
    f.ok(json!({"op":"set_options","target":agent,
        "options":{"codex.request_compression":true}}));
    f.ok(json!({"op":"send","target":agent,"body":"idle"}));
    let after = f.wait_timeout(&agent, Duration::from_secs(30), |a| {
        a.state == "idle" && a.execution_id != before.execution_id
    });
    assert_eq!(after.thread_id, before.thread_id);
    assert_ne!(after.provider_id, before.provider_id);
    let unloaded: u64 = fs::read_to_string(f.provider_home.join(format!("unloaded-{thread}")))
        .unwrap()
        .parse()
        .unwrap();
    let records: Vec<ThreadSelection> =
        fs::read_to_string(f.provider_home.join("thread-selections.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
    let resumed = records
        .iter()
        .find(|row| row.method == "thread/resume" && row.thread == thread)
        .unwrap();
    assert!(resumed.at_ns >= unloaded, "{records:?}");
    let sibling_after = f.status(&sibling);
    assert_eq!(sibling_after.state, "idle");
    assert_eq!(sibling_after.provider_id, before.provider_id);
    f.shutdown();
    f.assert_no_retire_failure();
}

#[test]
fn unloaded_codex_resumes_verified_selection_then_changes_the_next_turn() {
    let mut f = Harness::builder(Provider::Codex)
        .scenario(json!({"resume_ignores_selection":true}))
        .build();
    f.start();
    let id = f.spawn("hold");
    let original = f.wait(&id, |a| a.state == "running");
    f.ok(json!({"op":"stop","target":id}));
    f.wait(&id, |a| a.state == "stopped");
    change(&f, &id, "gpt-5.6-luna", "medium");
    f.ok(json!({"op":"send","target":id,"body":"hold"}));
    f.resume_when_released(&(id));
    let active = f.wait(&id, |a| a.state == "running");
    assert_eq!(active.thread_id, original.thread_id);
    assert_ne!(active.execution_id, original.execution_id);
    assert_eq!(
        serde_json::to_value(&active.session_selection).unwrap(),
        serde_json::to_value(&original.session_selection).unwrap()
    );
    assert_eq!(
        active.turn_selection.as_ref().unwrap().model,
        "gpt-5.6-luna"
    );
    assert_eq!(active.turn_selection.as_ref().unwrap().effort, "medium");
    let requests: Vec<Value> = fs::read_to_string(f.provider_home.join("turn-selections.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(requests[1]["model"], "gpt-5.6-luna");
    assert_eq!(requests[1]["effort"], "medium");
    f.shutdown();
}

#[test]
fn codex_resume_verifies_known_pairs_and_rejects_unknown_selection() {
    for (persist, model, effort, error) in [
        (false, None, None, None),
        (true, None, None, None),
        (true, Some("gpt-5.6-sol"), None, Some("model_mismatch")),
        (
            true,
            Some("gpt-5.6-terra"),
            Some("medium"),
            Some("model_mismatch"),
        ),
        (
            true,
            Some("gpt-5.6-luna"),
            Some("high"),
            Some("effort_mismatch"),
        ),
    ] {
        check_codex_resume(persist, model, effort, error);
    }
}
