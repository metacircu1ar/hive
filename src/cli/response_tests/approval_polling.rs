//! Approval polling regressions.
use super::*;

#[test]
fn approval_poll_preserves_empty_operational_errors() {
    use clap::Parser;
    let failure = json!({"error":{"code":"provider_failed","message":""}});
    let stub = Stub::new(vec![failure.clone()]);
    let cli = super::super::Cli::try_parse_from([
        "hivectl",
        "--state-dir",
        stub.root().to_str().unwrap(),
        "approvals",
        "--wait",
        "--timeout",
        "1",
    ])
    .unwrap();
    let outcome =
        super::super::dispatch::execute(super::super::dispatch::Prepared::new(cli).unwrap())
            .unwrap();
    assert!(!outcome.success);
    assert!(outcome.input.is_none());
    assert_eq!(outcome.value, failure);
    assert_eq!(stub.finish().len(), 1);
}

#[test]
fn approval_wait_keeps_bounded_polling_and_input_timeout_outcomes_distinct() {
    use clap::Parser;
    let error = json!({"error":{"code":"invalid_value",
        "message":"Invalid approval arguments.",
        "field":"/agent",
        "expected":"nonempty agent ID",
        "examples":[r#"{"op":"approvals","limit":100}"#]}});
    let stub = Stub::new(vec![error]);
    let cli = super::super::Cli::try_parse_from([
        "hivectl",
        "--state-dir",
        stub.root().to_str().unwrap(),
        "approvals",
        "--wait",
        "--timeout",
        "1",
    ])
    .unwrap();
    let outcome =
        super::super::dispatch::execute(super::super::dispatch::Prepared::new(cli).unwrap())
            .unwrap();
    assert!(!outcome.success);
    assert!(outcome.input.is_some());
    assert_eq!(outcome.value["error"]["field"], "--agent");
    let requests = stub.finish();
    assert_eq!(requests.len(), 1);
    assert_eq!(outcome.value["request_id"], requests[0]["id"]);
    let stub = Stub::raw_with_delay(vec![None], Duration::from_millis(1200));
    let cli = super::super::Cli::try_parse_from([
        "hivectl",
        "--state-dir",
        stub.root().to_str().unwrap(),
        "approvals",
        "--wait",
        "--timeout",
        "1",
    ])
    .unwrap();
    let start = Instant::now();
    let outcome =
        super::super::dispatch::execute(super::super::dispatch::Prepared::new(cli).unwrap())
            .unwrap();
    assert!(!outcome.success);
    assert!(outcome.input.is_none());
    assert_eq!(outcome.value["error"]["code"], "timeout");
    assert!(start.elapsed() < Duration::from_secs(3));
    let requests = stub.finish();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["args"]["op"], "approvals");
}

#[test]
fn approval_preparation_preserves_cursor_options_and_remote_response_flags() {
    use clap::Parser;
    let stub = Stub::new(vec![json!({"approvals":[],"next_after":null})]);
    let cli = super::super::Cli::try_parse_from([
        "hivectl",
        "--state-dir",
        stub.root().to_str().unwrap(),
        "approvals",
        "--all",
        "--after",
        r#"{"created_at":7,"id":"opaque"}"#,
        "--limit",
        "200",
    ])
    .unwrap();
    let prepared = super::super::dispatch::Prepared::new(cli).unwrap();
    assert!(super::super::dispatch::execute(prepared).unwrap().success);
    let requests = stub.finish();
    assert_eq!(
        requests[0]["args"],
        json!({"op":"approvals","all":true,"after":{"created_at":7,"id":"opaque"},"limit":200})
    );
    for action in ["--accept", "--decline", "--cancel"] {
        let stub = Stub::new(vec![json!({"request_id":"retry",
                 "error":{"code":"invalid_value",
                "message":"Invalid action.",
                "field":"/action",
                "expected":"supported action",
                "examples":["hivectl respond APPROVAL_ID --accept"]}})]);
        let cli = super::super::Cli::try_parse_from([
            "hivectl",
            "--state-dir",
            stub.root().to_str().unwrap(),
            "respond",
            "00000000-0000-4000-8000-000000000001",
            action,
            "--request-id",
            "retry",
        ])
        .unwrap();
        let result =
            super::super::dispatch::execute(super::super::dispatch::Prepared::new(cli).unwrap())
                .unwrap();
        assert_eq!(result.value["error"]["field"], action);
        assert_eq!(result.value["request_id"], "retry");
        assert!(result.input.is_some());
        stub.finish();
    }
}
