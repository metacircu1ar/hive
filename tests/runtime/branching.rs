use super::*;

fn branch(f: &Harness, source: &str, body: &str) -> String {
    let response = f.spawned(json!({"op":"spawn","from_agent":source,"body":body}));
    response.agent_id.as_str().into()
}
fn finished_startup(f: &Harness, id: &str) -> AgentView {
    let page = f.events(id);
    let mut spawned = page
        .events
        .iter()
        .filter(|event| event.agent_id == id && event.kind == "spawned");
    let event = spawned.next().expect("agent has a spawn event");
    let message = event.data["initial_message_id"].as_str().unwrap();
    assert!(spawned.next().is_none(), "multiple spawn events for {id}");
    finished_input(f, id, message)
}

fn finished_input(f: &Harness, id: &str, message: &str) -> AgentView {
    let agent = f.wait(id, |a| {
        a.last_finished_turn
            .as_ref()
            .is_some_and(|turn| turn.message.as_deref() == Some(message))
            && a.last_finished_turn
                .as_ref()
                .is_some_and(|turn| turn.outcome == "completed")
            && a.turn_id.is_none()
            && a.state == "idle"
    });
    let messages = f.messages(id);
    assert!(
        messages
            .messages
            .iter()
            .any(|m| m.id == message && m.recipient == id && m.state == "completed"),
        "input {message} for {id} did not complete: {}",
        serde_json::to_value(messages).unwrap()
    );
    let events = f.events(id);
    assert!(
        events.events.iter().any(|event| event.agent_id == id
            && event.kind == "turn_finished"
            && event.data["message_id"] == message
            && event.data["status"] == "completed"
            && event.data["turn_id"] == agent.last_finished_turn.as_ref().unwrap().turn),
        "input {message} for {id} has no matching completion event: {}",
        serde_json::to_value(events).unwrap()
    );
    agent
}
fn native_history(f: &Harness, agent: &AgentView, claude: bool) -> Value {
    let thread = agent.thread_id.as_deref().unwrap();
    if claude {
        let state = State::new(f.root.clone()).unwrap();
        let binding = state
            .lookup(
                "claude",
                &Profile {
                    provider: Provider::Claude,
                    method: Method::Subscription,
                    api_key_env: None,
                    api_key_file: None,
                },
            )
            .unwrap()
            .unwrap();
        serde_json::from_slice(
            &fs::read(
                binding
                    .directory
                    .join("provider")
                    .join(format!("mock-history-{thread}")),
            )
            .unwrap(),
        )
        .unwrap()
    } else {
        let value: Value = serde_json::from_slice(
            &fs::read(f.provider_home.join(format!("{thread}.json"))).unwrap(),
        )
        .unwrap();
        value["history"].clone()
    }
}

provider_matrix!(
    branch_idle_history_is_independent_and_resumes_the_child_on_both_providers,
    branch_idle_history_is_independent_and_resumes_the_child_on_both_providers_case
);

fn branch_idle_history_is_independent_and_resumes_the_child_on_both_providers_case(
    provider: Provider,
) {
    let claude = provider == Provider::Claude;
    let mut f = if claude {
        super::claude::fixture()
    } else {
        Harness::new()
    };
    f.start();
    let source = if claude {
        super::claude::spawn(&f, "reply:prefix")
    } else {
        f.spawn("reply:prefix")
    };
    let before = finished_startup(&f, &source);
    fs::write(f.tmp.path().join("role.md"), "changed after source spawn").unwrap();
    let request = json!({"op":"spawn","from_agent":source,"body":"branch recall"});
    let accepted = f.call_id("same-branch", request.clone());
    let child = accepted["agent_id"]
        .as_str()
        .unwrap_or_else(|| panic!("{accepted}"));
    assert_eq!(f.call_id("same-branch", request), accepted);
    let later = f.sent(json!({"op":"send","target":source,"body":"reply:later"}));
    let forked = finished_startup(&f, child);
    finished_input(&f, &source, later.message_id.as_str());
    assert_eq!(forked.branch_phase.as_deref(), Some("bound"));
    assert_eq!(
        forked.branched_from_agent_id.as_deref(),
        Some(source.as_str())
    );
    assert!(forked.parent_id.is_none());
    assert_eq!(forked.model, before.model);
    assert_ne!(forked.thread_id, before.thread_id);
    let context = f.status_reply(json!({"op":"status","target":child,"instructions":true}));
    assert_eq!(context.agent.instructions.as_deref(), Some("Test role\n\n"));
    let history = native_history(&f, &forked, claude);
    assert_branch_history(&history);
    f.ok(json!({"op":"stop","target":child}));
    f.wait(child, |a| a.state == "stopped");
    let queued = f.sent(json!({"op":"send","target":child,"body":"reply:child-only"}));
    f.resume_when_released(child);
    let resumed = finished_input(&f, child, queued.message_id.as_str());
    assert_eq!(forked.thread_id, resumed.thread_id);
    assert_eq!(resumed.branch_phase.as_deref(), Some("bound"));
    let source_file = f.native_file(provider, before.thread_id.as_deref().unwrap());
    let child_file = f.native_file(provider, forked.thread_id.as_deref().unwrap());
    f.ok(json!({"op":"terminate","target":source}));
    f.wait_purgeable(&source);
    let removed = f.ok(json!({"op":"purge","target":source}));
    // Claude branches retain inherited source sessions as shared ownership.
    assert_eq!(
        removed["native_sessions_deleted"],
        u64::from(!claude),
        "{removed}"
    );
    assert_eq!(source_file.exists(), claude);
    if claude {
        assert_eq!(removed["native_sessions_retained"], 1);
        assert_eq!(removed["native_warnings"][0]["reason"], "shared_session");
    }
    assert!(child_file.exists());
    f.shutdown();
    f.start();
    let queued = f.sent(json!({"op":"send","target":child,"body":"reply:after-restart"}));
    f.resume_after_wakeup(child);
    let restarted = finished_input(&f, child, queued.message_id.as_str());
    assert_eq!(restarted.thread_id, forked.thread_id);
    f.shutdown();
}

fn assert_branch_history(history: &Value) {
    let history = history.as_array().unwrap();
    assert!(history.iter().any(|v| v == "reply:prefix"), "{history:?}");
    assert!(!history.iter().any(|v| v == "reply:later"), "{history:?}");
}

provider_matrix!(
    branch_self_on_first_turn_progresses_with_one_resident_slot,
    branch_self_on_first_turn_progresses_with_one_resident_slot_case
);

fn branch_self_on_first_turn_progresses_with_one_resident_slot_case(provider: Provider) {
    let claude = provider == Provider::Claude;
    let mut f = if claude {
        super::claude::fixture()
    } else {
        Harness::new()
    };
    f.edit_config(|config| {
        config["runtime"] = json!({"max_loaded":1,"max_providers":1,"max_active_turns":1,
                "max_starting":1,"sessions_per_codex_process":1,"idle_seconds":0});
    });
    f.start();
    let source = if claude {
        super::claude::spawn(&f, "branch self")
    } else {
        f.spawn("branch self")
    };
    f.wait(&source, |a| a.state == "stopped" && a.branch_hold == 0);
    let agents = f.listed(json!({"op":"agents","parent":source}));
    let list = &agents.agents;
    assert_eq!(list.len(), 1, "{agents:?}");
    let child = list[0].id.as_str();
    let forked = finished_startup(&f, child);
    assert_eq!(
        forked.branched_from_agent_id.as_deref(),
        Some(source.as_str())
    );
    assert_eq!(forked.parent_id.as_deref(), Some(source.as_str()));
    assert!(
        native_history(&f, &forked, claude)
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "branch self")
    );
    f.shutdown();
}

#[test]
fn branch_captures_active_turn_and_cancellation_releases_source() {
    let mut f = Harness::new();
    f.start();
    let source = f.spawn("hold");
    let active = f.wait(&source, |a| a.state == "running");
    let child = branch(&f, &source, "branch recall");
    let waiting = f.status_reply(json!({"op":"status","target":child}));
    assert_eq!(
        waiting.agent.branch_phase.as_deref(),
        Some("waiting_for_source")
    );
    assert!(waiting.agent.provider_id.is_none());
    assert_eq!(
        waiting.agent.branch.as_ref().unwrap()["source_turn"],
        serde_json::to_value(&active.turn_id).unwrap()
    );
    f.ok(json!({"op":"stop","target":child}));
    assert_eq!(
        support::reply::Failure::parse(f.call(json!({"op":"resume","target":child})))
            .error
            .code,
        "agent_busy"
    );
    f.wait(&source, |a| a.branch_hold == 0);
    let child = branch(&f, &source, "branch recall");
    f.ok(json!({"op":"interrupt","target":source}));
    let forked = finished_startup(&f, &child);
    assert_eq!(
        serde_json::to_value(forked.branch.as_ref().map(|branch| &branch["source_turn"])).unwrap(),
        serde_json::to_value(&active.turn_id).unwrap()
    );
    assert_eq!(
        forked
            .branch
            .as_ref()
            .and_then(|branch| branch["source_outcome"].as_str()),
        Some("interrupted")
    );
    f.shutdown();
}

#[test]
fn branch_claude_version_gate_rejects_unsupported_branches_and_resumes() {
    let mut f = super::claude::fixture();
    let binary = f.tmp.path().join("claude");
    let current = fs::read_to_string(&binary).unwrap();
    let old = current.replace("\"2.1.287\"", "\"2.1.236\"");
    assert_ne!(
        old, current,
        "the unsupported-version mutation must change the fixture"
    );
    f.start();
    let source = super::claude::spawn(&f, "reply:prefix");
    finished_startup(&f, &source);
    fs::write(&binary, &old).unwrap();
    let denied = f.call(json!({"op":"spawn","from_agent":source,"body":"branch recall"}));
    let denied = support::reply::Failure::parse(denied);
    assert_eq!(denied.error.code, "unsupported_provider", "{denied:?}");
    assert!(denied.error.message.as_str().contains("2.1.285"));
    assert_eq!(
        f.call(json!({"op":"agents"}))["agents"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        f.call(json!({"op":"status","target":source}))["agent"]["branch_hold"],
        0
    );
    f.shutdown();
    fs::write(&binary, &current).unwrap();
    f.start();
    let child = branch(&f, &source, "branch recall");
    let before = finished_startup(&f, &child);
    f.ok(json!({"op":"stop","target":child}));
    f.wait(&child, |a| a.state == "stopped");
    fs::write(&binary, &old).unwrap();
    let queued = f.sent(json!({"op":"send","target":child,"body":"reply:after-resume"}));
    f.resume_after_wakeup(&child);
    let denied = f.wait(&child, |a| a.state == "failed");
    assert_eq!(denied.error.as_deref(), Some("unsupported_provider"));
    assert_eq!(denied.thread_id, before.thread_id);
    assert!(denied.recovery_fence.is_none());
    let page = f.read(&child);
    assert_eq!(
        page.events
            .iter()
            .filter(|e| e.kind == "turn_started")
            .count(),
        1
    );
    f.shutdown();
    fs::write(binary, current).unwrap();
    f.start();
    f.resume_after_wakeup(&child);
    assert_eq!(
        finished_input(&f, &child, queued.message_id.as_str()).thread_id,
        before.thread_id
    );
    f.shutdown();
}

#[test]
fn branch_codex_proven_spawn_failure_retries_the_same_input_and_cut() {
    // This fault injection requires a new native exec, not an existing shard.
    let mut f = Harness::builder(Provider::Codex)
        .runtime(json!({"sessions_per_codex_process":1}))
        .build();
    f.start();
    let source = f.spawn("hold");
    let active = f.wait(&source, |a| a.state == "running");
    let child = branch(&f, &source, "branch recall");
    // Keep the catalog's executable identity stable but make native exec fail
    // before Python starts. The already-running source can finish normally.
    let interpreter = f.tmp.path().join("python3");
    let original = fs::read_link(&interpreter).unwrap();
    fs::remove_file(&interpreter).unwrap();
    f.ok(json!({"op":"interrupt","target":source}));
    let unsent = f.wait(&child, |a| a.state == "stopped");
    assert_eq!(unsent.branch_phase.as_deref(), Some("ready_to_fork"));
    assert!(unsent.recovery_fence.is_none());
    assert!(
        unsent
            .branch
            .as_ref()
            .map(|branch| &branch["native_thread"])
            .is_none_or(Value::is_null)
    );
    std::os::unix::fs::symlink(original, &interpreter).unwrap();
    let fork = finished_startup(&f, &child);
    assert_eq!(fork.branch_phase.as_deref(), Some("bound"));
    assert_eq!(
        serde_json::to_value(fork.branch.as_ref().map(|branch| &branch["source_turn"])).unwrap(),
        serde_json::to_value(&active.turn_id).unwrap()
    );
    let page = f.read(&child);
    assert_eq!(
        page.events
            .iter()
            .filter(|e| e.kind == "branch_ready")
            .count(),
        1
    );
    assert_eq!(
        page.events
            .iter()
            .filter(|e| e.kind == "turn_started")
            .count(),
        1
    );
    f.shutdown();
}

#[test]
fn branch_claude_proven_unstarted_retry_is_paced_and_cancellable() {
    let mut f = super::claude::fixture();
    f.start();
    let source = super::claude::spawn(&f, "reply:prefix");
    finished_startup(&f, &source);
    // Native launch configuration deterministically refuses protected metadata.
    let cwd = f.tmp.path().join(".git");
    fs::create_dir_all(&cwd).unwrap();
    let response =
        f.spawned(json!({"op":"spawn","from_agent":source,"cwd":cwd,"body":"branch recall"}));
    let child = response.agent_id.as_str();
    let first = f.wait(child, |a| a.state == "stopped");
    assert_eq!(first.branch_phase.as_deref(), Some("ready_to_fork"));
    assert!(first.recovery_fence.is_none());
    let began = Instant::now();
    let second = f.wait(child, |a| {
        a.state == "stopped" && a.execution_id != first.execution_id
    });
    assert!(began.elapsed() >= Duration::from_secs(4));
    assert_eq!(second.branch_phase.as_deref(), Some("ready_to_fork"));
    let page = f.read(child);
    assert_eq!(
        page.events
            .iter()
            .filter(|e| e.kind == "execution_starting")
            .count(),
        2
    );
    assert!(
        second
            .branch
            .as_ref()
            .map(|branch| &branch["native_thread"])
            .is_none_or(Value::is_null)
    );
    f.ok(json!({"op":"stop","target":child}));
    f.wait(&source, |a| a.branch_hold == 0);
    let stopped = f.status_reply(json!({"op":"status","target":child}));
    assert_eq!(stopped.agent.branch_phase.as_deref(), Some("failed"));
    assert!(stopped.agent.recovery_fence.is_none());
    f.shutdown();
}

#[test]
fn branch_retries_only_recognized_precreation_rejection() {
    let mut f = Harness::new();
    f.set_scenario(
        Provider::Codex,
        json!({"fork_rejections":["lastTurnId '{turn}' identifies an in-progress turn",
            "lastTurnId '{turn}' was not found in the source thread", "turn not found: {turn}",
                "turn {turn} does not have persisted rollout positions"]}),
    );
    f.start();
    let source = f.spawn("reply:prefix");
    finished_startup(&f, &source);
    let child = branch(&f, &source, "branch recall");
    assert_eq!(
        finished_startup(&f, &child).branch_phase.as_deref(),
        Some("bound")
    );
    assert_eq!(
        fs::read_to_string(f.provider_home.join("fork-rejected")).unwrap(),
        "4"
    );
    f.shutdown();
}

#[test]
fn branch_cancel_racing_native_ack_retains_identity_without_reopening() {
    let mut f = Harness::new();
    f.set_scenario(Provider::Codex, json!({"fork_hold_after_creation":true}));
    f.start();
    let source = f.spawn("reply:prefix");
    finished_startup(&f, &source);
    let child = branch(&f, &source, "branch recall");
    support::wait::wait_until(
        Duration::from_secs(15),
        Duration::from_millis(20),
        "fork was not attempted",
        || f.provider_home.join("fork-created").exists().then_some(()),
    );
    let native = fs::read_to_string(f.provider_home.join("fork-created")).unwrap();
    f.ok(json!({"op":"stop","target":child}));
    fs::write(f.provider_home.join("fork-release"), "release").unwrap();
    let failed = f.wait(&child, |a| {
        a.branch_phase.as_deref() == Some("uncertain")
            && a.thread_id.as_deref() == Some(native.as_str())
            && a.conversation_clean
    });
    assert_eq!(
        failed
            .branch
            .as_ref()
            .and_then(|branch| branch["native_thread"].as_str()),
        Some(native.as_str())
    );
    assert_eq!(
        support::reply::Failure::parse(f.call(json!({"op":"resume","target":child})))
            .error
            .code,
        "agent_busy"
    );
    let events = f.read(&child);
    assert!(
        !events
            .events
            .iter()
            .any(|event| event.kind == "branch_ready"),
        "{events:?}"
    );
    f.shutdown();
}

#[test]
fn branch_overrides_keep_source_binding_and_do_not_inherit_parent_lifetime() {
    let mut f = Harness::new();
    f.start();
    let source = f.spawn("reply:prefix");
    let before = finished_startup(&f, &source);
    for (extra, code) in [
        (json!({"provider":"claude"}), "binding_conflict"),
        (json!({"auth":"other"}), "binding_conflict"),
        (json!({"lifetime":"with_parent"}), "parent_required"),
    ] {
        let mut args = json!({"op":"spawn","from_agent":source,"body":"reply:child"});
        args.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        assert_eq!(
            support::reply::Failure::parse(f.call(args)).error.code,
            code
        );
    }
    let result = f.spawned(json!({"op":"spawn","from_agent":source,"role":"reviewer",
        "lifetime":"independent","model":"gpt-5.6-terra","effort":"medium","instructions":"",
            "body":"reply:child"}));
    let child = result.agent_id.as_str();
    let forked = finished_startup(&f, child);
    assert_eq!(forked.model, "gpt-5.6-terra");
    assert_eq!(forked.effort, "medium");
    // Branches retain their source's sandbox even when the role defaults differ.
    assert_eq!(forked.sandbox, before.sandbox);
    assert_eq!(forked.auth_generation, before.auth_generation);
    assert_eq!(forked.lifetime, hive::config::Lifetime::Independent);
    assert_eq!(
        f.call(json!({"op":"status","target":child,"instructions":true}))["agent"]["instructions"]
            .as_str()
            .unwrap_or_default(),
        ""
    );
    f.shutdown();
}

#[test]
fn branch_cli_inherits_cwd_and_drops_source_handoff_ownership_claims() {
    let mut f = Harness::new();
    f.start();
    let source = f.spawn("reply:prefix");
    let before = finished_startup(&f, &source);
    f.shutdown();
    // Model the saved auth handoff's ownership sections, which are intentionally
    // broader than native delivery receipts. They must not be copied as instructions.
    let db = rusqlite::Connection::open(f.root.join("runtime/runtime.sqlite")).unwrap();
    db.execute(
        "UPDATE agents SET data=json_set(data,'$.handoff',?2) WHERE id=?1",
        rusqlite::params![
            source,
            "You are source\nRecent inputs: secret-unsent\nChildren: old-child\nPending \
                human approval: \
                old-approval"
        ],
    )
    .unwrap();
    drop(db);
    f.start();
    let task = f.tmp.path().join("branch-task.txt");
    fs::write(&task, "branch recall").unwrap();
    let output = support::command(cli_binary())
        .arg("--state-dir")
        .arg(&f.root)
        .args(["spawn", "--from", &source, "--body-file"])
        .arg(task)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    let child = response["agent_id"].as_str().unwrap();
    let forked = finished_startup(&f, child);
    assert_eq!(forked.cwd, before.cwd);
    assert!(forked.handoff.is_none());
    let history: Value = serde_json::from_slice(
        &fs::read(
            f.provider_home
                .join(format!("{}.json", forked.thread_id.as_deref().unwrap())),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(
        !history["instructions"]
            .as_str()
            .unwrap()
            .contains("secret-unsent")
    );
    f.shutdown();
}
