use super::*;

provider_matrix!(
    lifetimes_terminate_active_bound_children_on_both_providers_and_survive_restart,
    lifetimes_terminate_active_bound_children_on_both_providers_and_survive_restart_case
);

fn lifetimes_terminate_active_bound_children_on_both_providers_and_survive_restart_case(
    provider: Provider,
) {
    let use_claude = provider == Provider::Claude;
    let mut f = if use_claude {
        claude::fixture()
    } else {
        Harness::new()
    };
    f.start();
    let parent = if use_claude {
        claude::spawn(&f, "spawn bound child")
    } else {
        f.spawn("spawn bound child")
    };
    f.wait(&parent, |a| {
        a.state == "idle"
            && f.mailbox(&parent)
                .messages
                .iter()
                .any(|m| m.sender == "human" && m.state == "completed")
    });
    let all = f.listed(json!({"op":"agents","parent":parent}));
    let children = &all.agents;
    assert_eq!(children.len(), 1, "{all:?}");
    let child = children[0].id.as_str().to_owned();
    let running = f.wait(&child, |a| a.state == "running");
    assert_eq!(running.lifetime, hive::config::Lifetime::WithParent);
    assert_eq!(running.parent_id.as_deref(), Some(parent.as_str()));
    let queued = f.sent(json!({"op":"send","target":child,"body":"queued input"}));
    let args = json!({"op":"terminate","target":parent});
    let accepted = f.call_id("end-parent", args.clone());
    assert!(accepted.get("error").is_none(), "{accepted}");
    let expired = f.wait(&child, |a| a.state == "stopped");
    assert!(expired.expired);
    assert_eq!(expired.task_outcome, "open");
    let result = f.message_result(json!({"op":"message_result","message_id":queued.message_id}));
    assert_eq!(result.delivery_state, "undeliverable");
    let events = f.read(&child);
    assert_expired_child_stopped(&events, &parent);
    f.wait(&parent, |a| a.state == "stopped");
    f.shutdown();
    f.start();
    assert_eq!(f.call_id("end-parent", args.clone()), accepted);
    assert_eq!(
        support::reply::Failure::parse(f.call(args)).error.code,
        "agent_completed"
    );
    assert_eq!(
        support::reply::Failure::parse(f.call(json!({"op":"resume","target":child})))
            .error
            .code,
        "agent_completed"
    );
    f.shutdown();
    assert!(f.reports().iter().all(|r| r["confirmed"] == true));
}

fn assert_expired_child_stopped(events: &harness::views::EventsView, parent: &str) {
    let entries = &events.events;
    let expiry: Vec<_> = entries
        .iter()
        .filter(|e| e.kind == "lifetime_expired")
        .collect();
    assert_eq!(expiry.len(), 1);
    assert_eq!(
        expiry[0].data,
        json!({"parent_id":parent,"reason":"parent_terminated"})
    );
    let stopped = entries
        .iter()
        .find(|e| e.kind == "execution_stopped")
        .unwrap();
    assert!(
        entries.iter().any(|e| {
            matches!(
                e.kind.as_str(),
                "provider_shutdown" | "provider_session_released"
            ) && e.execution_id == stopped.execution_id
                && e.data["confirmed"] == true
                && e.data["quiescent"] == true
                && e.seq < stopped.seq
        }),
        "{events:?}"
    );
}

#[test]
fn lifetimes_cli_spawn_overrides_role_default_and_rejects_bound_human_spawns() {
    let mut f = Harness::new();
    f.start();
    let body = f.tmp.path().join("body.txt");
    fs::write(&body, "idle").unwrap();
    for (role, lifetime, expected) in [
        ("reviewer", "independent", None),
        ("implementor", "with_parent", Some("invalid_value")),
        ("implementor", "persistent", Some("invalid_value")),
    ] {
        let output = support::command(env!("CARGO_BIN_EXE_hivectl"))
            .arg("--state-dir")
            .arg(&f.root)
            .args(["spawn", role, "--cwd"])
            .arg(f.tmp.path())
            .arg("--body-file")
            .arg(&body)
            .args(["--lifetime", lifetime])
            .output()
            .unwrap();
        let result: Value = serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&output.stderr)));
        if let Some(expected) = expected {
            assert_eq!(output.status.code(), Some(2));
            let result = support::reply::Failure::parse(result);
            assert_eq!(result.error.field.as_deref().unwrap(), "--lifetime");
            assert_eq!(result.error.code, expected, "{result:?}");
        } else {
            assert!(output.status.success(), "{result:?}");
            let id = result["agent_id"].as_str().unwrap();
            let a = f.wait(id, |a| a.state == "idle");
            assert_eq!(a.lifetime, hive::config::Lifetime::Independent);
            assert!(a.parent_id.is_none());
        }
    }
    f.shutdown();
}

#[test]
fn accepted_spawn_replays_after_directory_removal_and_wire_parent_rule_is_preserved() {
    let mut f = Harness::new();
    f.start();
    let cwd = f.tmp.path().join("disposable-workspace");
    fs::create_dir(&cwd).unwrap();
    let request = json!({"op":"spawn","role":"implementor","cwd":cwd,"body":"idle"});
    let first = f.call_id("existing-cwd", request.clone());
    assert!(first.get("error").is_none(), "{first}");
    let id = first["agent_id"].as_str().unwrap();
    f.wait(id, |a| a.state == "idle");
    f.ok(json!({"op":"stop","target":id}));
    f.wait(id, |a| a.state == "stopped");
    fs::remove_dir(&cwd).unwrap();
    assert_eq!(f.call_id("existing-cwd", request), first);
    let rejected = f.call(json!({"op":"spawn","role":"implementor","cwd":f.tmp.path(),
        "body":"idle","lifetime":"with_parent"}));
    let rejected = support::reply::Failure::parse(rejected);
    assert_eq!(rejected.error.code, "parent_required");
    f.shutdown();
}
