//! Status decoding regressions.
use super::*;

#[test]
fn daemon_response_status_requires_current_fields_and_known_enums() {
    let good = status_agent("completed", "failed", false);
    let mut invalid = vec![
        json!({}),
        json!({"agent":null}),
        json!({"agent":[]}),
        json!({"agent":17}),
        json!({"agent":good,"attachment_store_bytes":0,"extension":true}),
    ];
    for key in good.as_object().unwrap().keys() {
        let mut missing = good.clone();
        missing.as_object_mut().unwrap().remove(key);
        invalid.push(json!({"agent":missing,"attachment_store_bytes":0}));
    }
    for (key, value) in [
        ("state", json!("future")),
        ("task_outcome", json!("future")),
        ("state", json!({"failed":null})),
        ("automatic_continuation", json!("future")),
        ("one_shot_waiting", json!("future")),
        ("one_shot_waiting", json!({"question":null})),
        ("automatic_continuation", json!({"waiting":null})),
        ("task_outcome", json!({"completed":null})),
        ("expired", json!("true")),
        ("branch_hold", json!(null)),
        ("open_joins_truncated", json!(null)),
        ("open_joins_truncated", json!(true)),
        ("open_joins", json!(vec!["join"; 1025])),
        ("extension", json!(true)),
        ("graph_id", json!(null)),
        ("api_key_turns", json!(0)),
        ("provider", json!("claude")),
        ("billing", json!("api_key")),
        ("waiting_on", json!("robot")),
        ("approval_id", json!("unpaired")),
    ] {
        let mut agent = good.clone();
        agent[key] = value;
        invalid.push(json!({"agent":agent,"attachment_store_bytes":0}));
    }
    for response in invalid {
        let stub = Stub::new(vec![response.clone()]);
        let error = waiter(stub.root(), "completed").run().err().unwrap();
        assert_eq!(error.code(), "daemon_protocol_error", "{response}");
        assert_eq!(stub.finish().len(), 1);
    }
}

#[test]
fn daemon_response_accepts_current_status_enrichment_and_compact_fields() {
    let mut agent = status_agent("completed", "idle", false);
    agent["branch"] = json!({"source":"source","source_thread":"thread","source_execution":"exec",
        "source_turn":"turn","phase":"bound","deadline":null,"source_outcome":null,
        "source_handoff":null,"native_thread":"thread","error":null});
    agent["branch_dependents"] = json!(["child"]);
    agent["waiting_on"] = json!("human");
    agent["approval_id"] = json!("approval");
    agent["instructions"] = json!("current instructions");
    let response = json!({"agent":agent,
        "attachment_store_bytes":0,
        "steering_verification":[{"auth":"main",
        "model":"mock",

        "effort":"low","selection":"current_session","daemon_must_be_stopped":true,
        "enables_next_launch":true,"argv":["hivectl","verify"]}]});
    let stub = Stub::new(vec![response.clone()]);
    let result = waiter(stub.root(), "completed").run().unwrap();
    assert!(result.success);
    assert_eq!(
        result.value,
        json!({"agent_id":"agent-id","result":null,
        "result_selection":null,"result_read_target":null})
    );
    assert_eq!(stub.finish().len(), 1);
}

#[test]
fn daemon_response_status_decisions_preserve_terminal_precedence() {
    for (outcome, state, expired, terminal) in [
        ("cancelled", "failed", true, Some(false)),
        ("completed", "failed", false, Some(true)),
        ("completed", "failed", true, Some(true)),
        ("open", "failed", true, Some(false)),
        ("open", "failed", false, Some(false)),
        ("open", "idle", false, None),
    ] {
        let agent = status_agent(outcome, state, expired);
        let response = json!({"agent":agent,"attachment_store_bytes":0});
        let stub = Stub::new(vec![response.clone()]);
        let result = waiter(stub.root(), "completed").run().unwrap();
        if let Some(success) = terminal {
            assert_eq!(result.success, success);
            assert_eq!(
                result.value,
                if success {
                    json!({"agent_id":"agent-id","result":null,"result_selection":null,
                    "result_read_target":null})
                } else {
                    response
                }
            );
        } else {
            assert!(!result.success);
            assert_eq!(
                result.value,
                json!({"wait":"timeout","agent_id":"agent-id","agent":agent})
            );
        }
        assert_eq!(stub.finish().len(), 1);
    }
}

#[test]
fn failed_agent_wait_uses_server_continuation_fact_without_cancelling() {
    for continuation in ["none", "waiting", "runnable"] {
        let mut agent = status_agent("open", "failed", false);
        agent["automatic_continuation"] = json!(continuation);
        let response = json!({"agent":agent,"attachment_store_bytes":0});
        let stub = Stub::new(vec![response.clone()]);
        let result = waiter(stub.root(), "completed").run().unwrap();
        assert!(!result.success);
        assert_eq!(
            result.value,
            if continuation == "none" {
                response
            } else {
                json!({"wait":"timeout","agent_id":"agent-id","agent":agent})
            }
        );
        let requests = stub.finish();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["args"]["op"], "status");
    }
}
