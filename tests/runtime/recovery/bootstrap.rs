use super::*;

#[test]
fn rejected_bootstrap_leaves_a_fresh_agent_clean_and_purgeable() {
    let mut f = Harness::builder(Provider::Codex)
        .runtime(json!({"sessions_per_codex_process":1,"linger_seconds":0,"warm_providers":0}))
        .build();
    f.start();
    let warm = f.spawn("idle");
    f.settled(&warm);
    f.ok(json!({"op":"terminate","target":warm}));
    f.wait(&warm, |agent| agent.state == "stopped");
    f.set_scenario(Provider::Codex, json!({"reject_account_read":true}));
    let agent = f.spawn("idle");
    let failed = f.wait_timeout(&agent, Duration::from_secs(30), |view| {
        view.state == "failed"
    });
    assert_eq!(failed.error.as_deref(), Some("provider_request_failed"));
    assert!(failed.execution_id.is_some());
    assert!(failed.provider_id.is_none() && failed.thread_id.is_none());
    assert!(failed.conversation_clean && failed.recovery_fence.is_none());
    f.ok(json!({"op":"terminate","target":agent}));
    f.wait(&agent, |view| view.state == "stopped");
    f.wait_purgeable(&agent);
    f.shutdown();
}

#[test]
fn termination_during_bootstrap_does_not_create_a_native_thread() {
    let mut f = Harness::builder(Provider::Codex)
        .runtime(json!({"sessions_per_codex_process":1,"linger_seconds":0,"warm_providers":0}))
        .build();
    f.start();
    let warm = f.spawn("idle");
    f.settled(&warm);
    f.ok(json!({"op":"terminate","target":warm}));
    f.wait(&warm, |agent| agent.state == "stopped");
    let selections = f.provider_home.join("thread-selections.jsonl");
    let before = fs::read_to_string(&selections).unwrap().lines().count();
    let gate = f.tmp.path().join("bootstrap-release");
    let waiting = f.tmp.path().join("bootstrap-waiting");
    let release = support::ReleaseFile::new(&gate);
    f.set_scenario(
        Provider::Codex,
        json!({"model_list_gate":gate,"model_list_waiting":waiting}),
    );
    let agent = f.spawn("idle");
    support::wait::wait_until(
        Duration::from_secs(20),
        Duration::from_millis(20),
        "worker bootstrap gate",
        || waiting.exists().then_some(()),
    );
    f.ok(json!({"op":"terminate","target":agent}));
    release.release();
    let stopped = f.wait_timeout(&agent, Duration::from_secs(30), |view| {
        view.state == "stopped" && view.conversation_clean
    });
    assert_eq!(stopped.task_outcome, "cancelled");
    assert!(stopped.thread_id.is_none());
    assert_eq!(
        fs::read_to_string(selections).unwrap().lines().count(),
        before
    );
    f.shutdown();
}
