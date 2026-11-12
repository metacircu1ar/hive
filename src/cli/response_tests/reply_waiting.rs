//! Reply waiting regressions.
use super::*;

#[test]
fn message_call_transport_failure_keeps_acceptance_identity_for_resume() {
    let stub = Stub::new(vec![
        json!({"message_id":"message","recipient":"agent","delivery_state":"accepted"}),
        json!({"not_a_result":true}),
    ]);
    let result = crate::calls::executor()
        .unwrap()
        .block_on(crate::calls::send(
            stub.root(),
            "retry-key",
            runtime::api::SendArgs {
                attachments: crate::runtime::api::Input::Missing,
                delay_seconds: runtime::api::Input::Missing,
                target: "agent".to_owned().into(),
                body: "work".to_owned().into(),
                mode: DeliveryMode::NextTurn.into(),
            },
            1,
            |_| {},
        ));
    assert_eq!(
        result.outcome,
        runtime::api::CallOutcome::ObservationUnavailable
    );
    assert_eq!(result.message_id.as_deref(), Some("message"));
    assert_eq!(result.request_id.as_deref(), Some("retry-key"));
    let requests = stub.finish();
    assert_eq!(requests[0]["id"], "retry-key");
    assert!(requests[0]["args"].get("timeout").is_none());
    assert_eq!(requests[1]["args"]["op"], "message_result");
    assert_eq!(requests[1]["args"]["message_id"], "message");
}

#[test]
fn message_wait_timeout_keeps_pending_delivery_and_never_sends_cancel() {
    let stub = Stub::new(vec![message_snapshot("accepted", "pending")]);
    let result = crate::calls::executor()
        .unwrap()
        .block_on(crate::calls::wait(stub.root(), "message", 1, |_| {}));
    assert_eq!(result.outcome, runtime::api::CallOutcome::Timeout);
    assert_eq!(
        result.result.unwrap().delivery_state,
        runtime::domain::MessageState::Accepted
    );
    let requests = stub.finish();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["args"]["op"], "message_result");
}

#[test]
fn message_wait_deadline_bounds_unresponsive_socket_reads() {
    let root = tempfile::Builder::new()
        .prefix("hl-call-")
        .tempdir_in("/tmp")
        .unwrap();
    fs::create_dir(root.path().join("runtime")).unwrap();
    let listener = UnixListener::bind(runtime::socket_path(root.path())).unwrap();
    let worker = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        thread::sleep(Duration::from_secs(2));
        drop(stream);
    });
    let started = Instant::now();
    let result = crate::calls::executor()
        .unwrap()
        .block_on(crate::calls::wait(root.path(), "message", 1, |_| {}));
    assert_eq!(result.outcome, runtime::api::CallOutcome::Timeout);
    assert!(started.elapsed() < Duration::from_millis(1800));
    worker.join().unwrap();
}

#[test]
fn join_wait_rejects_a_completed_result_for_another_identity() {
    let response = json!({"join_id":"wrong","requester":"human","request_id":"round","created_at":0,
        "deadline":null,"state":"completed","members":[]});
    let stub = Stub::new(vec![response]);
    let result = super::super::join_wait::run(stub.root(), "expected", 1).unwrap();
    assert!(!result.success);
    assert_eq!(result.value["wait"], "observation_unavailable");
    assert_eq!(result.value["join_id"], "expected");
    assert_eq!(result.value["error"]["code"], "daemon_protocol_error");
    assert_eq!(stub.finish()[0]["args"]["target"], "join:expected");
}

#[test]
fn direct_wait_input_errors_keep_wait_context_and_exit_two() {
    for event_read in [false, true] {
        let error = runtime::api::InputContext::Status.error(
            hive_input::Code::InvalidValue,
            "/target",
            "an agent ID",
        );
        let stub = Stub::new(vec![json!({"error":error})]);
        let outcome = if event_read {
            waiter(stub.root(), "turn-finished")
                .poll_events(&json!({}))
                .unwrap()
                .unwrap()
        } else {
            waiter(stub.root(), "completed").run().unwrap()
        };
        let input = outcome.input.unwrap();
        assert_eq!(input.value()["error"]["field"], "TARGET");
        let rendered =
            serde_json::from_value::<hive_input::InputError>(input.value()["error"].clone())
                .unwrap()
                .render(hive_input::Product::Hive)
                .unwrap();
        assert_eq!(rendered.exit_code, 2);
        assert!(
            input.value()["error"]["examples"][0]
                .as_str()
                .unwrap()
                .starts_with("hivectl wait ")
        );
        stub.finish();
    }
}
