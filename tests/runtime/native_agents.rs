//! Parent-only output and lifecycle across private native tasks; synthetic wire.
use super::*;

#[test]
fn joined_native_tasks_return_parent_results_once() {
    let mut f = Harness::builder(Provider::Claude).build();
    f.start();
    let batch = f.batch::<harness::views::SpawnView>(json!({"op":"batch","join":{},"operations":[
        {"op":"spawn","role":"claude","cwd":f.tmp.path(),"body":"native:agent","one_shot":true,
            "request_id":"native-one"},
        {"op":"spawn","role":"claude","cwd":f.tmp.path(),"body":"native:agent","one_shot":true,
            "request_id":"native-two"}]}));
    let join = batch.join.as_ref().unwrap().join_id.as_str();
    let done = support::wait::wait_until(
        support::wait::TEST_TIMEOUT,
        support::wait::POLL_INTERVAL,
        "native one-shot join",
        || {
            let row = f.join_snapshot(json!({"op":"read","target":format!("join:{join}")}));
            (row.state == "completed").then_some(row)
        },
    );
    let members = &done.members;
    assert_eq!(members.len(), 2);
    assert_ne!(members[0].target, members[1].target);
    for member in members {
        assert_eq!(member.outcome, "completed");
        assert_eq!(member.output.as_ref().unwrap().text, "native-parent-result");
    }
    assert_eq!(
        serde_json::to_value(&done).unwrap(),
        serde_json::to_value(f.join_snapshot(json!({"op":"read","target":format!("join:{join}")})))
            .unwrap()
    );
    f.shutdown();
}

#[test]
fn branch_after_private_native_work_keeps_source_files_until_the_last_owner_is_purged() {
    let mut f = Harness::builder(Provider::Claude).build();
    f.start();
    let source = claude::spawn(&f, "native:agent");
    claude::answered(&f, &source, "native-parent-result");
    let original = f.settled(&source);
    let transcript = f.native_file(Provider::Claude, original.thread_id.as_deref().unwrap());
    let fork = f.spawned(json!({"op":"spawn","from_agent":source,"body":"reply:branch"}));
    let child = fork.agent_id.as_str();
    claude::answered(&f, child, "reply:branch");
    let branched = f.settled(child);
    assert_eq!(branched.branch_phase.as_deref(), Some("bound"));
    assert_ne!(branched.thread_id, original.thread_id);
    f.ok(json!({"op":"terminate","target":source}));
    f.wait_purgeable(&source);
    let removed = f.ok(json!({"op":"purge","target":source}));
    assert_eq!(removed["native_sessions_retained"], 1, "{removed}");
    assert!(transcript.exists());
    f.ok(json!({"op":"stop","target":child}));
    f.wait(child, |agent| agent.state == "stopped");
    f.ok(json!({"op":"send","target":child,"body":"reply:resumed"}));
    f.resume_when_released(child);
    claude::answered(&f, child, "reply:resumed");
    assert_eq!(f.settled(child).thread_id, branched.thread_id);
    f.ok(json!({"op":"terminate","target":child}));
    f.wait_purgeable(child);
    let last = f.ok(json!({"op":"purge","target":child}));
    assert_eq!(last["native_sessions_deleted"], 2, "{last}");
    assert!(!transcript.exists());
    f.shutdown();
}

#[test]
fn one_shot_native_task_keeps_only_parent_output_and_persists_usage() {
    let mut f = Harness::builder(Provider::Claude).build();
    let home = f.provider_home(Provider::Claude);
    Harness::write_scenario(
        &home,
        json!({"model_usage":{"sonnet[1m]":{
        "inputTokens":12,"outputTokens":8,"cacheReadInputTokens":4,"cacheCreationInputTokens":0,
            "costUSD":99}}}),
    );
    f.start();
    let spawned = f.spawned(json!({"op":"spawn","role":"claude","cwd":f.tmp.path(),
        "body":"native:agent","one_shot":true}));
    let id = spawned.agent_id.as_str();
    let done = f.wait(id, |agent| {
        agent.task_outcome == "completed" && agent.state == "stopped"
    });
    assert_eq!(done.result.as_deref(), Some("native-parent-result"));
    assert_eq!(
        done.native_usage.as_ref().unwrap()["models"]["sonnet[1m]"]["input_tokens"],
        12
    );
    assert!(
        !serde_json::to_value(&done.native_usage)
            .unwrap()
            .to_string()
            .contains("costUSD")
    );
    let list = f.listed(json!({"op":"agents"}));
    assert!(list.agents.iter().all(|agent| agent.native_usage.is_none()));
    let history = f.events(id);
    assert!(!history.events.iter().any(|event| {
        event.kind == "assistant_message"
            && event.data["text"]
                .as_str()
                .is_some_and(|text| text.contains("PRIVATE_NATIVE_REPLY"))
    }));
    let before = done.native_usage.clone();
    f.shutdown();
    f.start();
    assert_eq!(f.status(id).native_usage, before);
    f.shutdown();
}

#[test]
fn private_native_tasks_end_with_interrupt_stop_pause_and_redirect() {
    for operation in ["interrupt", "stop", "redirect"] {
        let mut f = Harness::builder(Provider::Claude).build();
        f.start();
        let spawned = f.spawned(
            json!({"op":"spawn","role":"claude","cwd":f.tmp.path(),"body":"native:agent-hold"}),
        );
        let id = spawned.agent_id.as_str();
        support::wait::wait_until(
            support::wait::TEST_TIMEOUT,
            support::wait::POLL_INTERVAL,
            "native child startup",
            || {
                f.provider_home(Provider::Claude)
                    .join("native-agent-ready")
                    .exists()
                    .then_some(())
            },
        );
        let thread = f.status(id).thread_id.clone();
        if operation == "redirect" {
            f.ok(json!({"op":"send","target":id,"mode":"redirect","body":"reply:redirected"}));
            claude::answered(&f, id, "reply:redirected");
        } else {
            f.ok(json!({"op":operation,"target":id}));
            f.wait(id, |agent| {
                agent.turn_id.is_none() && (operation != "stop" || agent.state == "stopped")
            });
            if operation == "stop" {
                assert!(f.status(id).paused);
                f.resume_when_released(id);
            }
            f.ok(json!({"op":"send","target":id,"body":"reply:continued"}));
            claude::answered(&f, id, "reply:continued");
        }
        let continued = f.settled(id);
        assert!(continued.error.is_none(), "{continued:?}");
        assert_eq!(continued.thread_id, thread);
        f.ok(json!({"op":"terminate","target":id}));
        f.shutdown();
    }
}
