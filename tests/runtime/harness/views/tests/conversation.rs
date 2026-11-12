use super::super::{BatchView, EventsView, HistoryView, SendView, calls, live};
use super::{Value, decode, json};

fn message_result() -> Value {
    json!({"not_before":null,"message_id":"m","recipient":"a",
        "delivery_state":"completed","outcome":"replied","execution_id":null,
        "thread_id":null,"turn_id":null,"events_after":null,"output":null,
        "output_selection":"final_answer","output_partial":false,"queued_ahead":null,
        "queue_count_exact":true,"active_input":false,"recipient_state":"idle",
        "paused":false,"delivery_error":null,"extension":{"retained":true}})
}

#[test]
fn message_result_requires_all_nullable_slots_and_preserves_output() {
    let wire = message_result();
    let reply: calls::MessageResultView = decode::object(wire.clone()).unwrap();
    assert!(reply.output.is_none());
    assert_eq!(serde_json::to_value(reply).unwrap(), wire);
    for field in [
        "not_before",
        "execution_id",
        "thread_id",
        "turn_id",
        "events_after",
        "output",
        "queued_ahead",
        "delivery_error",
    ] {
        let mut missing = wire.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(decode::object::<_, calls::MessageResultView>(missing).is_err());
    }
    let mut output = wire.clone();
    output["output"] = json!({"text":"answer","extension":[null, false]});
    let reply: calls::MessageResultView = decode::object(output.clone()).unwrap();
    assert_eq!(serde_json::to_value(reply).unwrap(), output);
    let mut malformed = wire;
    malformed["held_for_build"] = Value::Null;
    assert!(decode::object::<_, calls::MessageResultView>(malformed).is_err());
}

#[test]
fn blocking_reply_keeps_required_nulls_and_object_results_and_errors() {
    let wire = json!({"outcome":"observation_unavailable","message_id":null,
        "request_id":"r","result":null,"error":{"code":"result_expired",
        "message":"Unavailable","received":null},"extension":true});
    let reply: calls::CallView = decode::object(wire.clone()).unwrap();
    assert!(reply.result.is_none());
    assert_eq!(serde_json::to_value(reply).unwrap(), wire);
    for field in ["message_id", "request_id", "result", "error"] {
        let mut missing = wire.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(decode::object::<_, calls::CallView>(missing).is_err());
    }
    for field in ["result", "error"] {
        let mut malformed = wire.clone();
        malformed[field] = json!([]);
        assert!(decode::object::<_, calls::CallView>(malformed).is_err());
    }
    let success = json!({"outcome":"replied","message_id":"m","request_id":"r",
        "result":message_result(),"error":null});
    let reply: calls::CallView = decode::object(success.clone()).unwrap();
    assert_eq!(serde_json::to_value(reply).unwrap(), success);
}

#[test]
fn homogeneous_batches_keep_join_presence_and_reject_nonobject_rows() {
    let wire = json!({"results":[{"message_id":"m","recipient":"a",
        "delivery_state":"accepted","extension":true}],"error":null,"extension":3});
    let batch: BatchView<SendView> = decode::object(wire.clone()).unwrap();
    assert!(batch.join.is_none());
    assert_eq!(serde_json::to_value(batch).unwrap(), wire);
    for join in [Value::Null, json!([])] {
        let mut malformed = wire.clone();
        malformed["join"] = join;
        assert!(decode::object::<_, BatchView<SendView>>(malformed).is_err());
    }
    let mut malformed = wire;
    malformed["results"][0] = json!(["m", "accepted", "a"]);
    assert!(decode::object::<_, BatchView<SendView>>(malformed).is_err());
}

#[test]
fn live_pages_keep_absence_false_and_private_fields_for_privacy_checks() {
    let wire = json!({"cursor":null,"items":[{"seq":1,"at_ms":2,"kind":"text",
        "text":"answer","extension":"echoed input"}],"resync":true,
        "durable_changed":false,"extension":true});
    let page: live::LiveView = decode::object(wire.clone()).unwrap();
    assert!(page.cursor.is_none());
    assert!(page.items[0].truncated.is_none());
    assert!(
        serde_json::to_string(&page.items[0])
            .unwrap()
            .contains("echoed input")
    );
    assert_eq!(serde_json::to_value(page).unwrap(), wire);
    let mut explicit = wire.clone();
    explicit["items"][0]["truncated"] = json!(false);
    let page: live::LiveView = decode::object(explicit.clone()).unwrap();
    assert_eq!(page.items[0].truncated, Some(false));
    assert_eq!(serde_json::to_value(page).unwrap(), explicit);
    for field in ["turn_id", "item_id", "tool", "text", "status", "truncated"] {
        let mut malformed = wire.clone();
        malformed["items"][0][field] = Value::Null;
        assert!(decode::object::<_, live::LiveView>(malformed).is_err());
    }
    let mut missing = wire;
    missing.as_object_mut().unwrap().remove("cursor");
    assert!(decode::object::<_, live::LiveView>(missing).is_err());
}

#[test]
fn backward_history_keeps_null_older_distinct_from_forward_cursor() {
    let wire = json!({"events":[],"older":null,"pruned_through":7,
        "history_expired":true,"extension":false});
    let page: HistoryView = decode::object(wire.clone()).unwrap();
    assert!(page.older.is_none());
    assert!(page.history_expired);
    assert_eq!(page.pruned_through, 7);
    assert_eq!(serde_json::to_value(page).unwrap(), wire);
    assert!(decode::object::<_, EventsView>(wire.clone()).is_err());
    let mut forward = wire;
    forward.as_object_mut().unwrap().remove("older");
    forward["cursor"] = json!(12);
    assert!(decode::object::<_, HistoryView>(forward.clone()).is_err());
    let page: EventsView = decode::object(forward.clone()).unwrap();
    assert_eq!(page.cursor, 12);
    assert_eq!(serde_json::to_value(page).unwrap(), forward);
}
