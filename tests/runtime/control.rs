use super::*;
use std::io::{BufRead, BufReader, Write};

#[test]
fn shutdown_acknowledges_and_joins_an_unfinished_control_request() {
    let mut f = Harness::new();
    f.start();
    let mut slow =
        std::os::unix::net::UnixStream::connect(f.root.join("runtime/control.sock")).unwrap();
    slow.set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    slow.write_all(b"{").unwrap();
    // The later accepted connection completes while the earlier one still waits
    // for its newline. Shutdown must retain that handler through its read timeout.
    assert_eq!(f.call(json!({"op":"ping"}))["status"], "running");
    f.shutdown();
    let mut response = String::new();
    BufReader::new(slow).read_line(&mut response).unwrap();
    let response: Value = serde_json::from_str(&response).unwrap();
    let response = support::reply::Failure::parse(response);
    assert_eq!(response.error.code, "invalid_request");
    assert!(!f.root.join("runtime/owner.json").exists());
    // Hub and control ownership were released; immediate daemon restart succeeds.
    f.start();
    f.shutdown();
}

fn cli_request(f: &Harness, args: &[&str]) -> (bool, Value) {
    let output = support::command(env!("CARGO_BIN_EXE_hivectl"))
        .arg("--state-dir")
        .arg(&f.root)
        .args(args)
        .output()
        .unwrap();
    let value = serde_json::from_slice(&output.stdout).unwrap_or_else(|_| {
        panic!(
            "{} / {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    (output.status.success(), value)
}
fn control_events(f: &Harness, id: &str, operation: &str) -> usize {
    f.read(id)
        .events
        .iter()
        .filter(|event| event.kind == "control_requested" && event.data["operation"] == operation)
        .count()
}
provider_matrix!(
    cli_control_retry_keys_preserve_later_turns,
    cli_control_retry
);
fn hold_cli(f: &Harness, id: &str, key: &str) -> AgentView {
    let (ok, reply) = cli_request(f, &["send", id, "--body", "hold", "--request-id", key]);
    assert!(ok, "{reply}");
    f.wait(id, |a| a.state == "running")
}
fn cli_control_retry(provider: Provider) {
    let mut f = Harness::builder(provider).build();
    f.start();
    let id = if provider == Provider::Claude {
        claude::spawn(&f, "idle")
    } else {
        f.spawn("idle")
    };
    // Initial idle precedes the first queued turn; wait until it has finished.
    claude::answered(&f, &id, "turn_finished");
    f.wait(&id, |a| a.state == "idle");
    let interrupt = ["interrupt", &id, "--request-id", "interrupt-once"];
    let (ok, rejected) = cli_request(&f, &interrupt);
    let rejected = support::reply::Failure::parse(rejected);
    assert!(!ok);
    assert_eq!(rejected.error.code, "no_active_turn");
    hold_cli(&f, &id, "hold-1");
    // Rejection did not consume the key. Success now records one durable receipt.
    let accepted = cli_request(&f, &interrupt);
    assert!(accepted.0, "{}", accepted.1);
    f.wait(&id, |a| a.state == "idle");
    let second = hold_cli(&f, &id, "hold-2");
    assert_eq!(cli_request(&f, &interrupt), accepted);
    assert_eq!(control_events(&f, &id, "interrupt"), 1);
    let current = f.status(&id);
    assert_eq!(current.state, "running");
    assert_eq!(current.turn_id, second.turn_id);
    for changed in [
        ["stop", id.as_str(), "--request-id", "interrupt-once"],
        [
            "interrupt",
            "another-agent",
            "--request-id",
            "interrupt-once",
        ],
    ] {
        let (ok, error) = cli_request(&f, &changed);
        let error = support::reply::Failure::parse(error);
        assert!(!ok);
        assert_eq!(error.error.code, "request_conflict");
    }
    let stop = ["stop", &id, "--request-id", "stop-once"];
    let stopped = cli_request(&f, &stop);
    assert!(stopped.0);
    f.wait(&id, |a| a.state == "stopped");
    let resume = ["resume", &id, "--request-id", "resume-once"];
    let resumed = resume_cli(&f, &resume);
    assert!(resumed.0);
    f.wait(&id, |a| a.state == "idle");
    assert_eq!(cli_request(&f, &stop), stopped);
    assert_eq!(control_events(&f, &id, "stop"), 1);
    assert!(!f.status(&id).paused);
    hold_cli(&f, &id, "hold-3");
    assert_eq!(cli_request(&f, &resume), resumed);
    assert_eq!(control_events(&f, &id, "resume"), 1);
    assert_eq!(f.status(&id).state, "running");
    assert!(cli_request(&f, &["interrupt", &id, "--request-id", "finish-turn"]).0);
    f.wait(&id, |a| a.state == "idle");
    assert!(
        cli_request(
            &f,
            &[
                "complete",
                &id,
                "--body",
                "All checks passed.\n",
                "--request-id",
                "completed"
            ]
        )
        .0
    );
    f.wait(&id, |a| {
        a.task_outcome == "completed" && a.state == "stopped"
    });
    f.shutdown();
}
fn resume_cli(f: &Harness, request: &[&str]) -> (bool, Value) {
    support::wait::try_wait_until(
        support::wait::TEST_TIMEOUT,
        Duration::from_millis(30),
        || {
            let response = cli_request(f, request);
            if response.0 {
                return Some(response);
            }
            let failure = support::reply::Failure::parse(response.1);
            assert_eq!(failure.error.code, "agent_releasing", "{failure:?}");
            None
        },
    )
    .expect("prior execution released for CLI resume")
}

#[test]
fn malformed_control_fields_do_not_enqueue_or_disappear_into_defaults() {
    let mut f = Harness::new();
    f.start();
    let id = f.spawn("hold");
    f.wait(&id, |a| a.state == "running");
    let before = f.call(json!({"op":"messages","target":id}))["messages"]
        .as_array()
        .unwrap()
        .len();
    for (mut raw, field) in [
        (
            json!({"op":"send","target":id,"body":"private-canary"}),
            "mode",
        ),
        (json!({"op":"agents"}), "parent"),
        (json!({"op":"agents"}), "auth"),
        (json!({"op":"status","target":id}), "instructions"),
    ] {
        raw[field] = json!({"private-canary":true});
        let result = f.call(raw);
        let result = support::reply::Failure::parse(result);
        assert_eq!(result.error.code, "invalid_value");
        assert_eq!(result.error.field.as_deref().unwrap(), format!("/{field}"));
        assert!(
            !serde_json::to_value(&result)
                .unwrap()
                .to_string()
                .contains("private-canary")
        );
    }
    assert_eq!(
        f.call(json!({"op":"messages","target":id}))["messages"]
            .as_array()
            .unwrap()
            .len(),
        before
    );
    let output = support::command(env!("CARGO_BIN_EXE_hivectl"))
        .arg("--state-dir")
        .arg(&f.root)
        .args(["status", ""])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    let response = support::reply::Failure::parse(response);
    assert_eq!(response.error.field.as_deref().unwrap(), "TARGET");
    assert_eq!(response.error.code, "out_of_range");
    f.shutdown();
}

#[test]
fn cli_agent_filters_match_groups_and_report_unknown_selectors() {
    let mut f = Harness::builder(Provider::Codex)
        .auth_group("plans", &["main"])
        .auth_group("unused", &["second"])
        .build();
    f.start();
    let spawned = f.spawned(json!({"op":"spawn","role":"implementor","auth":"plans",
        "cwd":f.tmp.path(),"body":"idle"}));
    let id = spawned.agent_id.as_str();
    f.wait(id, |a| a.state == "idle");
    for auth in ["plans", "main"] {
        let (ok, result) = cli_request(&f, &["agents", "--auth", auth]);
        assert!(ok, "{result}");
        assert_eq!(result["agents"].as_array().unwrap().len(), 1);
        assert_eq!(result["agents"][0]["id"], id);
    }
    for auth in ["unused", "second"] {
        let (ok, result) = cli_request(&f, &["agents", "--auth", auth]);
        assert!(ok, "{result}");
        assert!(result["agents"].as_array().unwrap().is_empty());
    }
    for flag in ["--auth", "--parent"] {
        let output = support::command(env!("CARGO_BIN_EXE_hivectl"))
            .arg("--state-dir")
            .arg(&f.root)
            .args(["agents", flag, "private-canary"])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        let result: Value = serde_json::from_slice(&output.stdout).unwrap();
        let result = support::reply::Failure::parse(result);
        assert_eq!(result.error.code, "invalid_value");
        assert_eq!(result.error.field.as_deref().unwrap(), flag);
        assert!(
            !serde_json::to_value(&result)
                .unwrap()
                .to_string()
                .contains("private-canary")
        );
    }
    f.shutdown();
}
