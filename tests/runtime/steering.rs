use super::*;

fn setup(scenario: Value) -> Harness {
    let mut f = Harness::builder(Provider::Codex).scenario(scenario).build();
    f.start();
    f
}

fn initial_completed(f: &Harness, agent: &str) {
    let messages = f.mailbox(agent);
    let initial = messages
        .messages
        .iter()
        .min_by_key(|message| message.seq)
        .expect("spawn creates the initial message");
    delivery(f, agent, &initial.id, "completed");
}

pub(super) fn delivery(
    f: &Harness,
    agent: &str,
    message: &str,
    expected: &str,
) -> harness::views::MessageView {
    let mut last_row = None;
    support::wait::try_wait_until(
        Duration::from_secs(10),
        support::wait::POLL_INTERVAL,
        || {
            let page = f.mailbox(agent);
            let row = page
                .messages
                .iter()
                .find(|row| row.id == message)
                .unwrap()
                .clone();
            let matched = row.state == expected;
            last_row = Some(row);
            matched.then(|| last_row.as_ref().unwrap().clone())
        },
    )
    .unwrap_or_else(|| panic!("{last_row:?}"))
}

#[test]
fn steering_passes_queued_input_in_order_without_starting_or_interrupting_a_turn() {
    let mut f = setup(json!({}));
    let id = f.spawn("hold");
    let active = f.wait(&id, |a| a.state == "running");
    assert_eq!(active.steering_supported, Some(true));
    let batch = f.batch::<harness::views::SendView>(json!({"op":"batch","operations":[
        {"op":"send","target":id,"body":"queued","request_id":"queued"},
        {"op":"send","target":id,"body":"first","mode":"steer","request_id":"first"},
        {"op":"send","target":id,"body":"second","mode":"steer","request_id":"second"}
    ]}));
    for message in &batch.results[1..] {
        let row = delivery(&f, &id, message.message_id.as_str(), "submitted");
        assert_eq!(row.mode.as_deref(), Some("steer"));
        assert_eq!(
            Some(&row.steering_target.as_ref().unwrap().turn_id),
            active.turn_id.as_ref()
        );
        assert_eq!(
            Some(&row.steering_target.as_ref().unwrap().execution_id),
            active.execution_id.as_ref()
        );
    }
    delivery(&f, &id, batch.results[0].message_id.as_str(), "accepted");
    let events = f.read(&id);
    let records = events.events;
    assert_eq!(
        records.iter().filter(|e| e.kind == "turn_started").count(),
        1
    );
    assert!(!records.iter().any(|e| e.kind == "control_requested"));
    let ids: Vec<_> = records
        .iter()
        .filter(|e| e.kind == "steering_submitted")
        .map(|e| e.data["message_id"].clone())
        .collect();
    assert_eq!(
        ids,
        vec![
            json!(batch.results[1].message_id),
            json!(batch.results[2].message_id)
        ]
    );
    let finish = json!({"op":"send","target":id,"body":"finish","mode":"steer"});
    let accepted = f.sent_id("finish", finish.clone());
    delivery(&f, &id, accepted.message_id.as_str(), "completed");
    delivery(&f, &id, batch.results[0].message_id.as_str(), "completed");
    let replay = f.sent_id("finish", finish);
    assert_eq!(replay.message_id, accepted.message_id);
    assert_eq!(
        serde_json::to_value(&replay.steering_target).unwrap(),
        serde_json::to_value(&accepted.steering_target).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&replay).unwrap(),
        serde_json::to_value(&accepted).unwrap()
    );
    let conflict = f.call_id(
        "finish",
        json!({"op":"send","target":id,"body":"finish","mode":"next-turn"}),
    );
    let conflict = support::reply::Failure::parse(conflict);
    assert_eq!(conflict.error.code, "request_conflict");
    f.shutdown();
}

#[test]
fn steering_handles_completion_before_ack_and_agent_tool_delivery() {
    for (scenario, initial) in [
        (json!({"finish_before_steer_ack":true}), "hold"),
        (json!({}), "steer self"),
    ] {
        let mut f = setup(scenario);
        let id = f.spawn(initial);
        if initial == "hold" {
            f.wait(&id, |a| a.state == "running");
            let message = f.call(json!({"op":"send","target":id,"body":"guidance","mode":"steer"}));
            delivery(
                &f,
                &id,
                message["message_id"].as_str().unwrap(),
                "completed",
            );
        }
        initial_completed(&f, &id);
        f.wait(&id, |a| a.state == "idle");
        let messages = f.mailbox(&id);
        let steer = messages
            .messages
            .iter()
            .find(|m| m.mode.as_deref() == Some("steer"))
            .unwrap();
        // Idle observes turn completion; the steering ACK can settle afterward.
        let steer = delivery(&f, &id, &steer.id, "completed");
        assert_eq!(steer.state, "completed");
        if initial == "steer self" {
            assert_eq!(steer.sender, id);
        }
        let events = f.read(&id);
        assert_eq!(
            events
                .events
                .iter()
                .filter(|e| e.kind == "turn_started")
                .count(),
            1
        );
        f.shutdown();
    }
}

#[test]
fn steering_rejections_preserve_the_original_turn_and_record_precise_outcomes() {
    for (scenario, state, code) in [
        ("stale_steer", "stale_turn", "no_active_turn"),
        ("reject_steer", "failed", "provider_request_failed"),
        ("uncertain_steer", "uncertain", "steering_uncertain"),
    ] {
        let mut f = setup(json!({scenario:true}));
        let id = f.spawn("hold");
        let original = f.wait(&id, |a| a.state == "running");
        let args = json!({"op":"send","target":id,"body":"guidance","mode":"steer"});
        let message = f.call_id("guidance", args.clone());
        let row = delivery(&f, &id, message["message_id"].as_str().unwrap(), state);
        assert_eq!(row.delivery_error.as_deref(), Some(code));
        let replay = f.call_id("guidance", args);
        assert_eq!(replay, message);
        if scenario != "stale_steer" {
            let current = f.status_reply(json!({"op":"status","target":id}));
            assert_eq!(current.agent.turn_id, original.turn_id);
            assert_eq!(current.agent.state, "running");
        }
        f.shutdown();
    }
}

#[test]
fn idle_stopped_and_unsupported_sessions_reject_steering_without_queuing_work() {
    for scenario in [
        json!({}),
        json!({"no_steering":true}),
        json!({"unknown_steering_probe":true}),
    ] {
        let supports = scenario == json!({});
        let mut f = setup(scenario);
        let id = f.spawn(if supports { "idle" } else { "hold" });
        f.wait(&id, |a| {
            a.state == if supports { "idle" } else { "running" }
        });
        if supports {
            // An agent can be idle before its startup turn begins; steer only
            // once that turn has finished, so no turn is active.
            support::wait::wait_until(
                support::wait::TEST_TIMEOUT,
                support::wait::POLL_INTERVAL,
                "the startup turn to finish",
                || {
                    let finished = f
                        .events(&id)
                        .events
                        .iter()
                        .any(|e| e.kind == "turn_finished");
                    (finished && f.status(&id).state == "idle").then_some(())
                },
            );
        }
        let send = json!({"op":"send","target":id,"body":"guidance","mode":"steer"});
        let rejected = f.call(send.clone());
        let rejected = support::reply::Failure::parse(rejected);
        assert_eq!(
            rejected.error.code,
            if supports {
                "no_active_turn"
            } else {
                "unsupported_operation"
            }
        );
        f.ok(json!({"op":"stop","target":id}));
        f.wait(&id, |a| a.state == "stopped");
        assert_eq!(
            support::reply::Failure::parse(f.call(send)).error.code,
            "no_active_turn"
        );
        assert_eq!(
            f.call(json!({"op":"messages","target":id}))["messages"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        f.shutdown();
    }
}

#[test]
fn malformed_ack_and_provider_exit_leave_steering_uncertain() {
    for scenario in ["wrong_steer_ack", "exit_steer"] {
        let mut f = setup(json!({scenario:true}));
        let id = f.spawn("hold");
        f.wait(&id, |a| a.state == "running");
        let message = f.call(json!({"op":"send","target":id,"body":"guidance","mode":"steer"}));
        f.wait(&id, |a| a.state == "failed");
        delivery(
            &f,
            &id,
            message["message_id"].as_str().unwrap(),
            "uncertain",
        );
        f.shutdown();
    }
}

#[test]
fn pending_steering_ack_does_not_block_dynamic_tools_or_interruption() {
    for wait_for_interrupt in [false, true] {
        let mut f = Harness::new();
        let receipt = f.tmp.path().join("steering-received");
        let scenario = if wait_for_interrupt {
            json!({"steer_ack_after_interrupt":receipt})
        } else {
            json!({"steer_ack_after_tool":true})
        };
        f.set_scenario(Provider::Codex, scenario);
        f.start();
        let id = f.spawn("hold");
        f.wait(&id, |a| a.state == "running");
        let message = f.call(json!({"op":"send","target":id,"body":"guidance","mode":"steer"}));
        if wait_for_interrupt {
            support::wait::wait_until(
                Duration::from_secs(5),
                Duration::from_millis(10),
                "provider gate",
                || receipt.exists().then_some(()),
            );
            delivery(
                &f,
                &id,
                message["message_id"].as_str().unwrap(),
                "submitting",
            );
            f.ok(json!({"op":"interrupt","target":id}));
        }
        delivery(
            &f,
            &id,
            message["message_id"].as_str().unwrap(),
            if wait_for_interrupt {
                "interrupted"
            } else {
                "completed"
            },
        );
        f.wait(&id, |a| a.state == "idle");
        f.shutdown();
    }
}

#[test]
fn completion_expires_queued_guidance_and_retries_keep_original_targets_after_restart() {
    let mut f = setup(json!({"finish_before_steer_ack":true}));
    let id = f.spawn("hold");
    let original = f.wait(&id, |a| a.state == "running");
    let request = json!({"op":"batch","operations":[
        {"op":"send","target":id,"body":"first","mode":"steer","request_id":"first"},
        {"op":"send","target":id,"body":"late","mode":"steer","request_id":"late"}
    ]});
    let accepted = f.call_id("batch", request.clone());
    delivery(
        &f,
        &id,
        accepted["results"][0]["message_id"].as_str().unwrap(),
        "completed",
    );
    delivery(
        &f,
        &id,
        accepted["results"][1]["message_id"].as_str().unwrap(),
        "stale_turn",
    );
    f.shutdown();
    f.start();
    f.ok(json!({"op":"send","target":id,"body":"hold"}));
    let current = f.wait(&id, |a| a.state == "running");
    assert_ne!(original.turn_id, current.turn_id);
    let replay = f.call_id("batch", request);
    assert_eq!(replay, accepted);
    for i in 0..2 {
        assert_eq!(
            replay["results"][i]["message_id"],
            accepted["results"][i]["message_id"]
        );
        assert_eq!(
            replay["results"][i]["steering_target"]["turn_id"],
            serde_json::to_value(&original.turn_id).unwrap()
        );
    }
    f.shutdown();
}

#[test]
fn capability_probe_handles_input_validation_order_and_fences_untracked_turns() {
    for empty_first in [false, true] {
        let mut f = setup(json!({"empty_input_first":empty_first}));
        let id = f.spawn("hold");
        assert_eq!(
            f.wait(&id, |a| a.state == "running").steering_supported,
            Some(true)
        );
        let message = f.call(json!({"op":"send","target":id,"body":"finish","mode":"steer"}));
        delivery(
            &f,
            &id,
            message["message_id"].as_str().unwrap(),
            "completed",
        );
        f.shutdown();
    }
    for scenario in ["probe_stale", "probe_success"] {
        let mut f = setup(json!({scenario:true}));
        let id = f.spawn("hold");
        let failed = f.wait(&id, |a| a.state == "failed");
        assert!(
            matches!(
                failed.error.as_deref(),
                Some("provider_protocol_error" | "recovery_required")
            ),
            "{failed:?}"
        );
        assert_ne!(failed.steering_supported, Some(true));
        let events = f.read(&id);
        assert!(!events.events.iter().any(|e| e.kind == "turn_started"));
        f.shutdown();
        assert!(
            f.reports().iter().any(|r| r["confirmed"] == false),
            "{:?}",
            f.reports()
        );
    }
}

#[test]
fn stop_waits_for_pending_steering_ack_before_cleanup_and_resume() {
    let mut f = Harness::new();
    configure_runtime(
        &f,
        json!({"max_loaded":2,"max_providers":1,"sessions_per_codex_process":2,
            "max_starting":1,"max_active_turns":2}),
        false,
    );
    let receipt = f.tmp.path().join("received");
    let gate = f.tmp.path().join("ack");
    let trace = f.tmp.path().join("trace");
    f.set_scenario(
        Provider::Codex,
        json!({"steer_ack_after_interrupt":receipt,"steer_ack_gate":gate,"trace":trace}),
    );
    f.start();
    let id = f.spawn("hold");
    let original = f.wait(&id, |a| a.state == "running");
    let message = f.call(json!({"op":"send","target":id,"body":"guidance","mode":"steer"}));
    support::wait::wait_until(
        Duration::from_secs(5),
        Duration::from_millis(10),
        "provider gate",
        || receipt.exists().then_some(()),
    );
    f.ok(json!({"op":"stop","target":id}));
    f.wait(&id, |a| a.state == "idle" && a.turn_id.is_none());
    std::thread::sleep(Duration::from_millis(150));
    delivery(
        &f,
        &id,
        message["message_id"].as_str().unwrap(),
        "submitting",
    );
    let methods = fs::read_to_string(&trace).unwrap();
    // Interruption now cleans this thread's terminals immediately. Retirement
    // must still wait for the outstanding steering acknowledgement.
    assert!(!methods.contains("thread/unsubscribe"), "{methods}");
    fs::write(gate, "release").unwrap();
    f.wait(&id, |a| a.state == "stopped");
    delivery(
        &f,
        &id,
        message["message_id"].as_str().unwrap(),
        "interrupted",
    );
    let methods = fs::read_to_string(&trace).unwrap();
    for method in ["thread/backgroundTerminals/clean", "thread/unsubscribe"] {
        assert!(methods.contains(method), "{methods}");
    }
    f.resume_when_released(&(id));
    f.ok(json!({"op":"send","target":id,"body":"hold"}));
    let current = f.wait(&id, |a| a.state == "running");
    assert_ne!(current.execution_id, original.execution_id);
    f.shutdown();
}

#[test]
fn cli_delivers_steering_and_rejects_invalid_modes_as_argument_errors() {
    let mut f = setup(json!({}));
    let id = f.spawn("hold");
    f.wait(&id, |a| a.state == "running");
    let body = f.tmp.path().join("guidance.txt");
    fs::write(&body, "finish").unwrap();
    for mode in ["steer", "invalid"] {
        let output = support::command(env!("CARGO_BIN_EXE_hivectl"))
            .arg("--state-dir")
            .arg(&f.root)
            .args([
                "send",
                &id,
                "--mode",
                mode,
                "--request-id",
                "cli-steer",
                "--body-file",
            ])
            .arg(&body)
            .output()
            .unwrap();
        if mode == "steer" {
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let response: Value = serde_json::from_slice(&output.stdout).unwrap();
            delivery(
                &f,
                &id,
                response["message_id"].as_str().unwrap(),
                "completed",
            );
        } else {
            assert_eq!(output.status.code(), Some(2));
            let error: Value = serde_json::from_slice(&output.stdout).unwrap();
            let error = support::reply::Failure::parse(error);
            assert_eq!(error.error.code, "invalid_value");
            assert_eq!(error.error.field.as_deref().unwrap(), "--mode");
            assert!(!output.stderr.is_empty());
        }
    }
    assert_eq!(
        f.call(json!({"op":"ping"}))["capabilities"]["providers"][0]["steering_policy"],
        "native_probe"
    );
    f.shutdown();
}

#[test]
fn only_method_not_found_is_cached_and_only_within_one_provider_process() {
    for scenario in ["no_steering", "unknown_steering_probe", "supported"] {
        let mut f = Harness::new();
        configure_runtime(
            &f,
            json!({"max_loaded":2,"max_providers":1,"sessions_per_codex_process":2,
                "max_starting":1,"max_active_turns":1}),
            false,
        );
        let trace = f.tmp.path().join("trace");
        f.set_scenario(Provider::Codex, json!({scenario:true,"trace":trace}));
        f.start();
        let first = f.spawn("idle");
        initial_completed(&f, &first);
        let first = f.settled(&first);
        let second = f.spawn("idle");
        initial_completed(&f, &second);
        let second = f.settled(&second);
        assert_eq!(first.provider_id, second.provider_id);
        assert_eq!(
            second.steering_supported == Some(true),
            scenario == "supported"
        );
        let probes = || {
            fs::read_to_string(&trace)
                .unwrap()
                .lines()
                .filter(|m| *m == "turn/steer")
                .count()
        };
        let expected = if scenario == "no_steering" { 1 } else { 2 };
        assert_eq!(probes(), expected, "{scenario}");
        f.shutdown();
        f.start();
        let third = f.spawn("idle");
        initial_completed(&f, &third);
        let third = f.settled(&third);
        assert_ne!(first.provider_id, third.provider_id);
        assert_eq!(probes(), expected + 1, "{scenario}");
        f.shutdown();
    }
}

#[test]
fn capability_probe_releases_startup_capacity_but_holds_the_first_turn() {
    let mut f = Harness::new();
    configure_runtime(
        &f,
        json!({"max_loaded":2,"max_providers":2,"sessions_per_codex_process":1,"max_starting":1,
            "max_active_turns":2}),
        false,
    );
    let gate = f.tmp.path().join("probe-release");
    f.set_scenario(Provider::Codex, json!({"probe_gate":gate}));
    f.start();
    let first = f.spawn("hold");
    let second = f.spawn("hold");
    for id in [&first, &second] {
        let ready = f.wait_timeout(id, Duration::from_secs(5), |a| a.state == "idle");
        assert_ne!(ready.steering_supported, Some(true));
        assert!(ready.turn_id.is_none());
        assert_eq!(
            support::reply::Failure::parse(
                f.call(json!({"op":"send","target":id,"body":"guidance","mode":"steer"}))
            )
            .error
            .code,
            "no_active_turn"
        );
        let events = f.read(id);
        assert!(!events.events.iter().any(|e| e.kind == "turn_started"));
    }
    fs::write(gate, "release").unwrap();
    for id in [&first, &second] {
        assert_eq!(
            f.wait(id, |a| a.state == "running").steering_supported,
            Some(true)
        );
    }
    f.shutdown();
}
