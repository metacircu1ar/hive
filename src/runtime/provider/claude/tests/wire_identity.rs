//! Wire identity regressions.
use super::*;

#[test]
fn echo_ledger_overflow_keeps_prior_echoes_and_fails_explicitly() {
    let mut f = Fixture::new();
    for id in 0..256 {
        f.codec
            .receive(
                json!({"type":"control_request","request_id":format!("ping-{id}"),
            "request":{"subtype":"mcp_message","server_name":"hive",
            "message":{"jsonrpc":"2.0","id":id,"method":"ping"}}}),
            )
            .unwrap();
    }
    let extra = f
        .codec
        .receive(json!({"type":"control_request","request_id":"overflow",
        "request":{"subtype":"mcp_message","server_name":"hive",
        "message":{"jsonrpc":"2.0","id":256,"method":"ping"}}}))
        .unwrap_err();
    assert_eq!(extra.code(), "output_limit");
    let echo = json!({"type":"control_response","response":{"subtype":"success","request_id":"ping-0",
        "response":{"mcp_response":{"jsonrpc":"2.0","id":0,"result":{}}}}});
    assert!(f.codec.receive(echo).unwrap().is_empty());
}

#[test]
fn acknowledged_input_must_not_reappear_in_interrupt_cancellation_queue() {
    let mut f = Fixture::new();
    let turn = f.turn();
    let request = f
        .codec
        .send(outgoing::request(4, outgoing::interrupt(&f.id, &turn)))
        .unwrap();
    assert!(
        f.codec
            .receive(json!({"type":"control_response","response":{
        "subtype":"success","request_id":request[0]["request_id"],
        "response":{"cancelled":[turn],"still_queued":[]}}}))
            .is_err()
    );
}

#[test]
fn open_claude_wire_tags_classify_unknown_strings_without_accepting_other_shapes() {
    macro_rules! check {
        ($kind:ty, $known:expr, $variant:expr, $other:expr) => {{
            assert_eq!(serde_json::from_value::<$kind>(json!($known)).unwrap(), $variant);
            for unknown in ["future", ""] {
                assert_eq!(serde_json::from_value::<$kind>(json!(unknown)).unwrap(), $other);
            }
            for malformed in [json!(null), json!(7), json!([]), json!({$known:null})] {
                assert!(serde_json::from_value::<$kind>(malformed).is_err());
            }
        }};
    }
    check!(
        super::super::wire::ResultSubtype,
        "success",
        super::super::wire::ResultSubtype::Success,
        super::super::wire::ResultSubtype::Other
    );
    check!(
        super::super::wire::TerminalReason,
        "completed",
        super::super::wire::TerminalReason::Completed,
        super::super::wire::TerminalReason::Other
    );
    check!(
        super::super::wire::SystemSubtype,
        "init",
        super::super::wire::SystemSubtype::Init,
        super::super::wire::SystemSubtype::Other
    );
    check!(
        super::super::wire::OriginKind,
        "human",
        super::super::wire::OriginKind::Human,
        super::super::wire::OriginKind::Other
    );
    check!(
        super::super::wire::HookEvent,
        "PostToolUse",
        super::super::wire::HookEvent::PostToolUse,
        super::super::wire::HookEvent::Other
    );
    check!(
        super::super::wire::CallbackId,
        "hive-guard",
        super::super::wire::CallbackId::Guard,
        super::super::wire::CallbackId::Other
    );
}

#[test]
fn native_replayed_control_replies_are_exact_echoes_not_new_acknowledgements() {
    let mut f = Fixture::new();
    let replies = f.codec.receive(json!({"type":"control_request","request_id":"native-1","request":{
        "subtype":"mcp_message","server_name":"hive","message":{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2024-11-05"}}}})).unwrap();
    assert!(f.codec.receive(replies[0].clone()).unwrap().is_empty());
    assert!(f.codec.pop().is_none());
    assert!(f.codec.receive(replies[0].clone()).is_err());
    let replies = f.codec.receive(json!({"type":"control_request","request_id":"native-2","request":{
        "subtype":"mcp_message","server_name":"hive","message":{"jsonrpc":"2.0","method":"notifications/initialized"}}})).unwrap();
    let mut forged = replies[0].clone();
    forged["response"]["response"] = json!({"different":true});
    assert!(f.codec.receive(forged).is_err());
    assert!(f.codec.receive(replies[0].clone()).unwrap().is_empty());
}

#[test]
fn native_terminal_shapes_bind_only_to_our_acknowledged_turn() {
    for (subtype, error, reason, interrupt, expected) in [
        ("success", false, Some("completed"), false, "completed"),
        ("success", true, Some("completed"), false, "failed"),
        ("error_max_turns", true, Some("max_turns"), false, "failed"),
        ("error_during_execution", true, None, true, "interrupted"),
        (
            "success",
            false,
            Some("aborted_streaming"),
            false,
            "interrupted",
        ),
        (
            "success",
            false,
            Some("aborted_tools"),
            false,
            "interrupted",
        ),
        ("success", false, Some("completed"), true, "completed"),
    ] {
        let mut f = Fixture::new();
        let turn = f.turn();
        let control = interrupt.then(|| {
            f.codec
                .send(outgoing::request(4, outgoing::interrupt(&f.id, &turn)))
                .unwrap()
        });
        f.codec.receive(json!({"type":"result","session_id":f.id,"subtype":subtype,"is_error":error,"terminal_reason":reason})).unwrap();
        assert_eq!(f.codec.pop().unwrap()["params"]["turn"]["status"], expected);
        if let Some(control) = control {
            f.codec.receive(json!({"type":"control_response","response":{"subtype":"success","request_id":control[0]["request_id"]}})).unwrap();
        }
        assert!(f.codec.quiescent());
    }
    for extra in [
        json!({"user_message_uuid":"wrong"}),
        json!({"origin":{"kind":"peer"}}),
        json!({"terminal_reason":"background_requested"}),
    ] {
        let mut f = Fixture::new();
        f.turn();
        let mut result = json!({"type":"result","session_id":f.id,"subtype":"success","is_error":false,"terminal_reason":"completed"});
        result
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        assert!(f.codec.receive(result).is_err());
        assert!(!f.codec.quiescent());
    }
}
