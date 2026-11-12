//! Response envelopes regressions.
use super::*;

#[test]
fn envelope_identity_is_not_mapped_to_an_artifact_id() {
    for (request, field) in [
        #[cfg(feature = "comb")]
        (
            json!({"op":"collector_admin", "action":"discard", "destination":"shared"}),
            Some("--request-id"),
        ),
        (json!({"op":"output"}), None),
        (json!({"op":"send"}), Some("--request-id")),
        #[cfg(feature = "comb")]
        (
            json!({"op":"evidence","kind":"correction","summary":"Reviewed correction."}),
            Some("--request-id"),
        ),
    ] {
        let error = runtime::api::InputContext::Control.error(
            hive_input::Code::InvalidRequestId,
            "/id",
            "a request ID",
        );
        let stub = Stub::new(vec![json!({"error":error})]);
        let result = remote_response(
            stub.root(),
            "retry",
            runtime::api::RequestEnvelope::new(request).request,
        );
        if let Some(field) = field {
            assert_eq!(result.unwrap().value["error"]["field"], field);
        } else {
            assert_eq!(result.err().unwrap().code(), "daemon_protocol_error");
        }
        stub.finish();
    }
}

#[test]
fn both_clients_reject_raw_duplicate_error_fields() {
    for raw in [
        r#"{"error":{"code":"provider_failed","code":"provider_failed","message":"x"}}"#,
        r#"{"error":{"code":"out_of_range","message":"x","field":"/limit","field":"/after","expected":"integer","examples":["hivectl agents"]}}"#,
        r#"{"error":{"code":"provider_failed","message":"x"},"error":{"code":"provider_failed","message":"x"}}"#,
    ] {
        let stub = Stub::raw(vec![Some(raw.into())]);
        assert_eq!(
            runtime::client(stub.root(), "sync", Request::Ping)
                .unwrap_err()
                .code(),
            "daemon_protocol_error"
        );
        stub.finish();
        let stub = Stub::raw(vec![Some(raw.into())]);
        let error = crate::calls::executor()
            .unwrap()
            .block_on(crate::calls::exchange::<Value>(
                stub.root(),
                "async",
                &Request::Ping,
                tokio::time::Instant::now() + Duration::from_secs(1),
            ))
            .unwrap_err();
        assert_eq!(error.code(), crate::error::ErrorCode::DaemonProtocol);
        stub.finish();
    }
}

#[test]
fn external_client_rejects_wrong_formats_and_malformed_error_envelopes() {
    for response in [
        json!({"unexpected":true,
            "artifact_id":"artifact",
            "offset":0,
            "text":"text",
            "next_offset":4,
            "bytes":4,
            "eof":true}),
        json!({"text":"missing required fields"}),
        json!({"error":null}),
        json!({"error":{"message":"missing code"}}),
    ] {
        let stub = Stub::new(vec![response]);
        let error = crate::calls::executor()
            .unwrap()
            .block_on(crate::calls::exchange::<runtime::api::OutputResponse>(
                stub.root(),
                "output",
                &Request::Ping,
                tokio::time::Instant::now() + Duration::from_secs(1),
            ))
            .unwrap_err();
        assert_eq!(error.code(), crate::error::ErrorCode::DaemonProtocol);
        stub.finish();
    }
}

#[test]
fn daemon_response_rejects_malformed_envelopes_and_preserves_valid_errors() {
    let response = json!({"error":{"code":"unknown_agent","message":"No agent has this ID."}});
    let stub = Stub::new(vec![response.clone()]);
    let result = remote_response(stub.root(), "request-id", Request::Ping).unwrap();
    assert!(!result.success);
    assert_eq!(result.value, response);
    assert_eq!(
        stub.finish(),
        vec![json!({"protocol":1,"id":"request-id","args":{"op":"ping"}})]
    );
    let stub = Stub::new(vec![response.clone()]);
    let result = waiter(stub.root(), "turn-finished").run().unwrap();
    assert!(!result.success);
    assert_eq!(result.value, response);
    assert_eq!(stub.finish().len(), 1);
    let mut malformed = vec![
        Value::Null,
        json!(false),
        json!(17),
        json!("raw"),
        json!([]),
        json!([{"error":null}]),
        json!({"error":{"code":"x","message":"m"},"extra":true}),
        json!({"error":{"code":"x","message":"m","extra":true}}),
    ];
    malformed.extend(
        [
            Value::Null,
            json!(false),
            json!(0),
            json!("failure"),
            json!([]),
            json!({}),
            json!({"code":"x"}),
            json!({"message":"m"}),
            json!(["x", "m"]),
        ]
        .into_iter()
        .map(|error| json!({"error":error})),
    );
    for response in malformed {
        let stub = Stub::new(vec![response.clone()]);
        let error = remote_response(stub.root(), "request-id", Request::Ping)
            .err()
            .unwrap();
        assert_eq!(error.code(), "daemon_protocol_error", "{response}");
        assert_eq!(stub.finish().len(), 1);
        let stub = Stub::new(vec![response.clone()]);
        let error = waiter(stub.root(), "turn-finished").run().err().unwrap();
        assert_eq!(error.code(), "daemon_protocol_error", "{response}");
        assert_eq!(stub.finish().len(), 1);
    }
}
