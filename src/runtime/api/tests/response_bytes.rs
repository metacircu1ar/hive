//! Literal acknowledgement bytes and canonical nested decorations.
use super::*;

#[test]
fn typed_listing_keeps_the_previous_public_projection_and_byte_budget() {
    let mut agent = crate::runtime::test_support::agent();
    agent.name = Some("name with é, \"quotes\" and\nnewline".into());
    agent.instructions = "private role instructions".into();
    agent.result = Some("status-only output".into());
    for approval in [None, Some("approval")] {
        let listing = listed_agent(
            &agent,
            Some(42),
            approval,
            crate::runtime::store::OneShotWaiting::None,
        );
        // The original listing projection removed these fields from public status.
        let mut previous = value(agent.clone().public());
        let fields = previous.as_object_mut().unwrap();
        for key in [
            "instructions",
            "profile",
            "completion_pending",
            "result",
            "handoff",
            "native_usage",
        ] {
            fields.remove(key);
        }
        let extras = value(ListingFields {
            one_shot_waiting: crate::runtime::store::OneShotWaiting::None,
            held_for_build: crate::runtime::features::required_build(&agent),
            idle_since: Some(42),
            provider: agent.profile.provider,
            billing: agent.profile.method,
            waiting_on: approval.map(|_| "human"),
            approval_id: approval.map(str::to_owned),
        });
        fields.extend(extras.as_object().unwrap().clone());
        let actual = value(&listing);
        assert_eq!(actual.to_string(), previous.to_string());
        assert_eq!(encoded_len(&listing), previous.to_string().len());
        assert_eq!(actual.get("approval_id").is_some(), approval.is_some());
        assert_eq!(actual.get("waiting_on").is_some(), approval.is_some());
        assert!(actual.get("native_usage").is_none());
    }
}

#[test]
fn typed_batch_and_recorded_reply_keep_canonical_nested_bytes() {
    let spawn = Response::from(SpawnResponse {
        agent_id: "agent".into(),
        message_id: "message".into(),
    });
    let recorded = Response::Recorded(value(NameChangedResponse {
        agent_id: "agent",
        name: None,
    }));
    let reply = Response::from(BatchResponse {
        results: vec![spawn, recorded],
        join: None,
    });
    assert_eq!(
        reply.into_value().to_string(),
        concat!(
            r#"{"results":[{"agent_id":"agent","message_id":"message","status":"accepted"},"#,
            r#"{"agent_id":"agent","name":null}]}"#,
        )
    );
}

#[test]
fn acknowledgement_bytes_remain_canonical() {
    let cases = [
        #[cfg(feature = "comb")]
        (
            value(StatusResponse {
                status: ResponseStatus::Updated,
            }),
            r#"{"status":"updated"}"#,
        ),
        (
            value(StatusResponse {
                status: ResponseStatus::Stopping,
            }),
            r#"{"status":"stopping"}"#,
        ),
        (
            value(StatusResponse {
                status: ResponseStatus::Stopped,
            }),
            r#"{"status":"stopped"}"#,
        ),
        (
            value(SpawnResponse {
                agent_id: "agent".into(),
                message_id: "message".into(),
            }),
            r#"{"agent_id":"agent","message_id":"message","status":"accepted"}"#,
        ),
        (
            value(TerminateResponse {
                agent_id: "agent".into(),
            }),
            r#"{"agent_id":"agent","termination":"requested"}"#,
        ),
        (
            value(CompleteResponse { agent_id: "agent" }),
            r#"{"agent_id":"agent","completion":"pending_turn_boundary"}"#,
        ),
    ];
    for (actual, expected) in cases {
        assert_eq!(actual.to_string(), expected);
    }
    for (operation, name) in [
        (ControlOperation::Stop, "stop"),
        (ControlOperation::Resume, "resume"),
        (ControlOperation::Interrupt, "interrupt"),
    ] {
        let actual = value(ControlResponse {
            agent_id: "agent".into(),
            operation,
        });
        assert_eq!(
            actual.to_string(),
            format!("{{\"agent_id\":\"agent\",\"operation\":\"{name}\",\"status\":\"requested\"}}")
        );
    }
}

#[test]
fn decorations_preserve_nested_order_and_explicit_nulls() {
    let response = value(NameChangedResponse {
        agent_id: "agent",
        name: None,
    });
    let actual = value(
        ControlReply {
            request_id: Some("request".into()),
            cleanup_pending: Some(false),
            runtime_resources: Some(json!({"z":null,"a":{"z":null,"a":1}})),
            native_cleanup: None,
        }
        .with_payload(response),
    );
    assert_eq!(
        actual.to_string(),
        concat!(
            r#"{"agent_id":"agent","cleanup_pending":false,"name":null,"request_id":"request","#,
            r#""runtime_resources":{"a":{"a":1,"z":null},"z":null}}"#,
        )
    );
}

#[test]
fn auth_event_modes_keep_exact_tags_and_optional_fields() {
    let switched = value(events::AuthSwitched {
        from: "a",
        to: "b",
        from_method: "subscription",
        to_method: "subscription",
        reason: crate::runtime::auth_fallback::AuthReason::Manual,
        auth_group: None,
        auth_decision: None,
    });
    assert_eq!(
        switched.to_string(),
        concat!(
            r#"{"from":"a","from_method":"subscription","mode":"handoff","reason":"manual","#,
            r#""to":"b","to_method":"subscription"}"#,
        )
    );
    let dropped = value(events::BranchContextDropped {
        source: "a",
        source_turn: "turn",
        reason: crate::runtime::auth_fallback::AuthReason::Manual,
    });
    assert_eq!(
        dropped.to_string(),
        r#"{"mode":"summary_handoff","reason":"manual","source":"a","source_turn":"turn"}"#
    );
}
