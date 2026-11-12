//! Response cursors regressions.
use super::*;

#[test]
fn daemon_response_uses_valid_cursors_in_following_requests() {
    for cursor in [0, 42, u64::MAX] {
        let page = event_page(vec![], cursor);
        let stub = Stub::new(vec![page.clone(), page]);
        let mut waiter = waiter(stub.root(), "turn-finished");
        assert!(waiter.poll_events(&json!({})).unwrap().is_none());
        assert_eq!(waiter.after, cursor);
        assert!(waiter.poll_events(&json!({})).unwrap().is_none());
        assert_eq!(waiter.after, cursor);
        let requests = stub.finish();
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests[0]["args"],
            json!({"op":"read","target":"agent-id","after":41,"limit":200})
        );
        assert_eq!(
            requests[1]["args"],
            json!({"op":"read","target":"agent-id","after":cursor,"limit":200})
        );
    }
}

#[test]
fn daemon_response_first_finished_event_precedes_terminal_agent_state() {
    let agent = status_agent("cancelled", "failed", true);
    let first = event("turn_finished", 17, json!({"unknown":[null,2]}));
    let events = event_page(
        vec![
            event("assistant_message", 16, json!({"raw":"provider"})),
            first.clone(),
            event("turn_finished", 18, json!({"other":true})),
        ],
        100,
    );
    let stub = Stub::new(vec![
        json!({"agent":agent,"attachment_store_bytes":0}),
        events,
    ]);
    let result = waiter(stub.root(), "turn-finished").run().unwrap();
    assert!(result.success);
    assert_eq!(result.value, json!({"agent":agent,"event":first}));
    assert_status_then_read(&stub.finish());
}

#[test]
fn daemon_response_event_errors_and_history_precede_matching_events() {
    let agent = status_agent("completed", "failed", false);
    let response = json!({"error":{"code":"unknown_agent","message":"No agent has this ID."}});
    let stub = Stub::new(vec![
        json!({"agent":agent,"attachment_store_bytes":0}),
        response.clone(),
    ]);
    let result = waiter(stub.root(), "turn-finished").run().unwrap();
    assert!(!result.success);
    assert_eq!(result.value, response);
    assert_status_then_read(&stub.finish());
    for pruned in [0, 21, u64::MAX] {
        let mut response = event_page(vec![event("turn_finished", 17, json!({}))], 100);
        response["history_expired"] = json!(true);
        response["pruned_through"] = json!(pruned);
        let stub = Stub::new(vec![
            json!({"agent":agent,"attachment_store_bytes":0}),
            response,
        ]);
        let result = waiter(stub.root(), "turn-finished").run().unwrap();
        assert!(!result.success);
        assert_eq!(
            result.value,
            json!({"error":{"code":"history_expired",
            "message":"Requested event history was compacted."},"pruned_through":pruned})
        );
        assert_status_then_read(&stub.finish());
    }
}

#[test]
fn daemon_response_rejects_malformed_events_without_advancing_cursor() {
    let good_event = event("turn_finished", 17, json!({}));
    let good_page = event_page(vec![good_event.clone()], 42);
    let mut malformed = vec![Value::Null, json!({}), json!([])];
    for key in good_page.as_object().unwrap().keys() {
        let mut missing = good_page.clone();
        missing.as_object_mut().unwrap().remove(key);
        malformed.push(missing);
    }
    for key in good_event.as_object().unwrap().keys() {
        let mut missing = good_event.clone();
        missing.as_object_mut().unwrap().remove(key);
        malformed.push(event_page(vec![missing], 42));
    }
    for field in ["cursor", "pruned_through"] {
        for value in [
            Value::Null,
            json!("7"),
            json!(-1),
            json!(7.0),
            json!(true),
            json!({}),
            json!([]),
        ] {
            let mut page = good_page.clone();
            page[field] = value;
            malformed.push(page);
        }
    }
    for (field, value) in [
        ("events", json!({})),
        ("events", Value::Null),
        ("events", json!("turn_finished")),
        ("history_expired", json!("true")),
        ("unexpected", json!(true)),
    ] {
        let mut page = good_page.clone();
        page[field] = value;
        malformed.push(page);
    }
    for bad in [
        Value::Null,
        json!(["turn_finished"]),
        json!({"type":"future"}),
        {
            let mut e = good_event.clone();
            e["type"] = json!({"turn_finished":null});
            e
        },
        {
            let mut e = good_event.clone();
            e["type"] = json!("future");
            e
        },
        {
            let mut e = good_event.clone();
            e["extension"] = json!(true);
            e
        },
    ] {
        // Invalid rows after a match must still be rejected.
        malformed.push(event_page(vec![good_event.clone(), bad], 42));
    }
    for response in malformed {
        let stub = Stub::new(vec![response.clone()]);
        let mut waiter = waiter(stub.root(), "turn-finished");
        let error = waiter.poll_events(&json!({})).err().unwrap();
        assert_eq!(error.code(), "daemon_protocol_error", "{response}");
        assert_eq!(waiter.after, 41);
        assert_eq!(stub.finish().len(), 1);
    }
}

#[test]
fn stalled_cli_preserves_optional_presence_and_cursor() {
    use clap::Parser;
    let stub = Stub::new(vec![json!({"groups":[],"next_cursor":null})]);
    let parsed = super::super::Cli::try_parse_from([
        "hivectl",
        "--state-dir",
        stub.root().to_str().unwrap(),
        "agents",
        "--stalled",
        "30m",
        "--stalled-after",
        r#"{"idle_since":10,"root_id":"root"}"#,
    ])
    .unwrap();
    assert!(
        super::super::dispatch::execute(super::super::dispatch::Prepared::new(parsed).unwrap())
            .unwrap()
            .success
    );
    let requests = stub.finish();
    let args = &requests[0]["args"];
    assert_eq!(args["stalled_seconds"], 1800);
    assert_eq!(
        args["stalled_after"],
        json!({"idle_since":10,"root_id":"root"})
    );
    for key in ["after", "parent", "auth", "limit"] {
        assert!(args.get(key).is_none(), "{requests:?}");
    }
    for extras in [
        vec!["--after", "0"],
        vec!["--parent", "a"],
        vec!["--auth", "main"],
    ] {
        let mut args = vec!["hivectl", "agents", "--stalled", "30m"];
        args.extend(extras);
        assert!(super::super::Cli::try_parse_from(args).is_err());
    }
    assert!(
        super::super::Cli::try_parse_from([
            "hivectl",
            "agents",
            "--stalled-after",
            r#"{"idle_since":1,"root_id":"a"}"#
        ])
        .is_err()
    );
}
