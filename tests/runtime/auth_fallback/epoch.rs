use super::*;

fn warm_main(f: &Harness) -> AgentView {
    let warm = f.spawned(json!({"op":"spawn","role":"implementor","auth":"main",
        "cwd":f.tmp.path(),"body":"reply: warm catalog"}));
    let agent = f.settled(&warm.agent_id);
    f.ok(json!({"op":"stop","target":warm.agent_id}));
    f.wait(&warm.agent_id, |agent| agent.state == "stopped");
    wait_until(CATALOG_TIMEOUT, POLL_INTERVAL, "retire warm worker", || {
        let resources = f.ok(json!({"op":"ping"}));
        (resources["runtime_resources"]["reserved_shards"] == 0).then_some(())
    });
    agent
}

#[test]
fn unrelated_login_restoration_keeps_a_gated_bootstrap_turn_and_cached_catalog() {
    let (mut f, _second) = grouped(4);
    f.edit_config(|config| {
        config["runtime"]["sessions_per_codex_process"] = json!(1);
        config["runtime"]["warm_providers"] = json!(0);
        config["runtime"]["linger_seconds"] = json!(0);
        config["runtime"]["cohort_min_seconds"] = json!(0);
    });
    f.start();
    let warm = warm_main(&f);
    let gate = f.tmp.path().join("bootstrap-release");
    let waiting = f.tmp.path().join("bootstrap-waiting");
    let release = support::ReleaseFile::new(&gate);
    f.set_scenario(
        Provider::Codex,
        json!({"catalog_models":["gpt-5.6-terra",
        "gpt-5.6-sol","gpt-5.6-luna","new-key-only"],
        "model_list_gate":gate,"model_list_waiting":waiting}),
    );
    let spawned = f.spawned(json!({"op":"spawn","role":"implementor","auth":"main",
        "cwd":f.tmp.path(),"body":"reply: gated bootstrap"}));
    let gated_pid = wait_until(
        CATALOG_TIMEOUT,
        POLL_INTERVAL,
        "worker bootstrap gate",
        || match fs::read_to_string(&waiting) {
            Ok(value) => Some(value.parse::<u32>().unwrap()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => panic!("cannot read bootstrap gate: {error}"),
        },
    );
    f.ok(json!({"op":"login_restored","auth":"second"}));
    release.release();
    let done = f.settled(&spawned.agent_id);
    let mailbox = f.mailbox(&spawned.agent_id);
    assert_eq!(mailbox.messages.len(), 1);
    assert_eq!(mailbox.messages[0].id, spawned.message_id);
    assert_eq!(mailbox.messages[0].state, "completed");
    assert_eq!(
        done.provider_pid,
        Some(gated_pid),
        "gate must be this worker"
    );
    assert_eq!(done.auth, "main");
    assert_eq!(done.auth_generation, warm.auth_generation);
    assert!(done.recovery_fence.is_none() && done.error.is_none());
    let turn = done.last_finished_turn.as_ref().unwrap();
    assert_eq!(turn.message.as_deref(), Some(spawned.message_id.as_str()));
    assert_eq!(turn.outcome, "completed");
    let delivered: Vec<_> = inputs(&f.provider_home)
        .into_iter()
        .filter(|input| input["body"] == "reply: gated bootstrap")
        .collect();
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0]["thread"], turn.thread);
    assert_cached_catalog(&f, &spawned.agent_id);
    f.shutdown();
}

fn assert_cached_catalog(f: &Harness, agent: &str) {
    let failure = f.expect_err(
        json!({"op":"set_options","target":agent,
        "options":{"model":"new-key-only","effort":"medium"}}),
        "invalid_value",
    );
    assert_eq!(failure.error.field.as_deref(), Some("/options/model"));
    assert_eq!(
        failure.error.expected.as_deref(),
        Some("a model in the provider catalog")
    );
}
