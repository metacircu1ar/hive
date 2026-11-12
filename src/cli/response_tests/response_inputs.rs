//! Response inputs regressions.
use super::*;

#[test]
fn direct_body_rejections_keep_selected_file_context_without_rereading() {
    use clap::Parser;
    for (command, flag) in [
        ("spawn", "--body-file"),
        ("send", "--body-file"),
        ("complete", "--result-file"),
    ] {
        let error = runtime::api::InputContext::Send.error(
            hive_input::Code::InvalidValue,
            "/body",
            "a body string",
        );
        let stub = Stub::new(vec![json!({"error":error})]);
        let path = stub.root().join("PRIVATE-body.txt");
        fs::write(&path, "private body").unwrap();
        let mut args = vec![
            "hivectl",
            "--state-dir",
            stub.root().to_str().unwrap(),
            command,
        ];
        if command == "spawn" {
            args.extend(["--cwd", stub.root().to_str().unwrap()]);
        } else {
            args.push("a");
        }
        args.extend([flag, path.to_str().unwrap()]);
        let cli = super::super::Cli::try_parse_from(args).unwrap();
        let prepared = super::super::dispatch::Prepared::new(cli).unwrap();
        fs::remove_file(&path).unwrap();
        let outcome = super::super::dispatch::execute(prepared).unwrap();
        assert!(outcome.input.is_some());
        assert_eq!(outcome.value["error"]["field"], flag);
        assert!(!outcome.value.to_string().contains("PRIVATE"));
        assert!(!outcome.value.to_string().contains("private body"));
        assert_eq!(stub.finish()[0]["args"]["body"], "private body");
    }
}

#[test]
fn typed_input_metadata_survives_sync_async_and_accepted_call_outcomes() {
    let failure = input_failure();
    let stub = Stub::new(vec![failure.clone()]);
    let response = runtime::client(stub.root(), "sync", Request::Ping).unwrap();
    assert_eq!(response, failure);
    stub.finish();
    let stub = Stub::new(vec![failure.clone()]);
    let error = crate::calls::executor()
        .unwrap()
        .block_on(crate::calls::exchange::<Value>(
            stub.root(),
            "async",
            &Request::Ping,
            tokio::time::Instant::now() + Duration::from_secs(1),
        ))
        .unwrap_err();
    assert!(error.input().is_some());
    assert_eq!(serde_json::to_value(error).unwrap(), failure["error"]);
    stub.finish();

    let stub = Stub::new(vec![
        json!({"message_id":"message","recipient":"agent","delivery_state":"accepted"}),
        failure.clone(),
    ]);
    let result = crate::calls::executor()
        .unwrap()
        .block_on(crate::calls::send(
            stub.root(),
            "retry",
            runtime::api::SendArgs {
                target: "agent".to_owned().into(),
                body: "work".to_owned().into(),
                mode: DeliveryMode::NextTurn.into(),
                attachments: runtime::api::Input::Missing,
                delay_seconds: runtime::api::Input::Missing,
            },
            1,
            |_| {},
        ));
    assert_eq!(
        result.outcome,
        runtime::api::CallOutcome::ObservationUnavailable
    );
    let outcome = super::super::CommandOutcome::from(result);
    assert!(outcome.input.is_none());
    assert_eq!(outcome.value["message_id"], "message");
    assert_eq!(outcome.value["request_id"], "retry");
    assert_eq!(outcome.value["error"], failure["error"]);
    stub.finish();

    let stub = Stub::new(vec![
        gc_page(10, false, Some(json!({"ended_at":1,"id":"a"}))),
        failure.clone(),
    ]);
    let outcome = super::super::gc::run_before(
        stub.root(),
        10,
        crate::runtime::api::CleanupOptions::default(),
    );
    assert!(outcome.input.is_none());
    assert_eq!(outcome.value["gc_progress"]["acknowledged_pages"], 1);
    assert_eq!(outcome.value["error"], failure["error"]);
    stub.finish();
}

#[test]
#[cfg(feature = "comb")]
fn evidence_remote_errors_keep_cli_source_and_request_identity_without_reopening() {
    use clap::Parser;
    for file_source in [false, true] {
        for (pointer, flag) in [
            (
                "/summary",
                if file_source {
                    "--summary-file"
                } else {
                    "--summary"
                },
            ),
            ("/files/1", "--file"),
            ("/commit", "--commit"),
            ("/verdict", "--verdict"),
            ("/kind", "KIND"),
        ] {
            let stub = Stub::new(vec![json!({"request_id":"evidence-retry", "error":{
                "code":"invalid_value", "message":"Invalid public evidence.", "field":pointer,
                "expected":"valid public evidence",
                     "examples":[r#"{"op":"evidence","kind":"review","summary":"Public","verdict":"findings"}"#]}})]);
            let source = stub.root().join("PRIVATE\nsummary");
            std::fs::write(&source, "public").unwrap();
            let parsed = super::super::Cli::try_parse_from([
                "hivectl",
                "--state-dir",
                stub.root().to_str().unwrap(),
                "evidence",
                "correction",
                if file_source {
                    "--summary-file"
                } else {
                    "--summary"
                },
                if file_source {
                    source.to_str().unwrap()
                } else {
                    "public"
                },
                "--request-id",
                "evidence-retry",
            ])
            .unwrap();
            let prepared = super::super::dispatch::Prepared::new(parsed).unwrap();
            std::fs::remove_file(source).unwrap();
            let response = super::super::dispatch::execute(prepared).unwrap();
            assert!(!response.success);
            assert!(response.input.is_some());
            assert_eq!(response.value["error"]["field"], flag);
            assert_eq!(response.value["request_id"], "evidence-retry");
            assert!(!response.value.to_string().contains("PRIVATE"));
            let requests = stub.finish();
            assert_eq!(requests[0]["id"], "evidence-retry");
            assert_eq!(requests[0]["args"]["summary"], "public");
            assert!(requests[0]["args"].get("verdict").is_none());
            assert!(requests[0]["args"].get("commit").is_none());
            assert!(!requests[0].to_string().contains("PRIVATE"));
        }
    }
}

#[test]
fn control_input_fields_map_to_real_cli_arguments_and_examples() {
    for (raw, pointer, field, command) in [
        (
            json!({"op":"status","target":"a"}),
            "/target",
            "TARGET",
            "status",
        ),
        (
            json!({"op":"status","target":"a"}),
            "/instructions",
            "--instructions",
            "status",
        ),
        (json!({"op":"spawn"}), "/body", "--body", "spawn"),
        (json!({"op":"spawn"}), "/from_agent", "--from", "spawn"),
        (json!({"op":"spawn"}), "/auth", "--auth", "spawn"),
        (json!({"op":"spawn"}), "/cwd", "--cwd", "spawn"),
        (json!({"op":"send"}), "/mode", "--mode", "send"),
        (
            json!({"op":"set_options"}),
            "/options/model",
            "--option",
            "set-options",
        ),
        (json!({"op":"set_auth"}), "/auth", "AUTH", "set-auth"),
        (json!({"op":"complete"}), "/body", "--body", "complete"),
        (json!({"op":"terminate"}), "/target", "TARGET", "terminate"),
        (json!({"op":"stop"}), "/target", "TARGET", "stop"),
        (json!({"op":"resume"}), "/target", "TARGET", "resume"),
        (json!({"op":"interrupt"}), "/target", "TARGET", "interrupt"),
        (json!({"op":"purge"}), "/recursive", "--recursive", "purge"),
        (json!({"op":"agents"}), "/auth", "--auth", "agents"),
        (
            json!({"op":"agents"}),
            "/stalled_after",
            "--stalled-after",
            "agents",
        ),
        (
            json!({"op":"batch"}),
            "/operations/2/body",
            "--operations-file",
            "batch",
        ),
        (
            json!({"op":"batch"}),
            "/operations/2/attachments",
            "--operations-file",
            "batch",
        ),
    ] {
        assert_control_input(raw, pointer, field, command);
    }
}

#[cfg(feature = "comb")]
#[test]
fn collector_input_fields_map_to_cli_arguments_and_examples() {
    for (raw, pointer, field, command) in [
        (
            json!({"op":"collector_admin", "action":"discard", "destination":"shared"}),
            "/destination",
            "DESTINATION",
            "collectors discard",
        ),
        (
            json!({"op":"collector_admin",
                 "action":"release_uncertain",
                 "destination":"shared",
                 "item":"item"}),
            "/item",
            "ITEM",
            "collectors release-uncertain",
        ),
        (
            json!({"op":"collector_admin",
                 "action":"rebind",
                 "destination":"shared",
                 "comb_id":"uuid"}),
            "/comb_id",
            "COMB_ID",
            "collectors rebind",
        ),
    ] {
        assert_control_input(raw, pointer, field, command);
    }
}

#[cfg(feature = "comb")]
#[test]
fn collector_remote_rejects_pointers_unused_by_the_requested_action() {
    for pointer in ["/item", "/comb_id", "/action", "/unknown"] {
        let error = runtime::api::InputContext::CollectorDiscard.error(
            hive_input::Code::InvalidValue,
            pointer,
            "a valid value",
        );
        let stub = Stub::new(vec![json!({"error":error})]);
        let request = runtime::api::RequestEnvelope::new(
            json!({"op":"collector_admin", "action":"discard", "destination":"shared"}),
        )
        .request;
        assert_eq!(
            remote_response(stub.root(), "retry", request)
                .err()
                .unwrap()
                .code(),
            "daemon_protocol_error"
        );
        stub.finish();
    }
}
