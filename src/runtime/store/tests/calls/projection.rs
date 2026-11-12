use super::*;

fn saved(store: &Store) -> (Option<String>, bool) {
    store
        .query_row_cached(
            "SELECT output,output_final FROM call_results WHERE message_id='task'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap()
}
fn last_event(store: &Store) -> String {
    store
        .query_row_cached(
            concat!(
                "SELECT data FROM events WHERE kind='assistant_message' ",
                "ORDER BY seq DESC LIMIT 1"
            ),
            [],
            |row| row.get(0),
        )
        .unwrap()
}

#[test]
fn invalid_or_missing_metadata_never_binds_output_but_keeps_opaque_event_data() {
    let store = crate::runtime::test_support::TestStore::new();
    let mut agent = agent("a", None);
    store.save(&agent).unwrap();
    input(&store, &mut agent, "task");
    for data in [
        json!(null),
        json!(7),
        json!(["turn-task", "final_answer", "positional"]),
        json!({"turn_id":"turn-task","text":null}),
        json!({"turn_id":"turn-task","text":7}),
        json!({"turn_id":"turn-task","text":[]}),
        json!({"turn_id":"turn-task","text":{}}),
        json!({"turn_id":null,"text":"unbound"}),
        json!({"turn_id":7,"text":"unbound"}),
        json!({"turn_id":"wrong","text":"unbound"}),
        json!({"text":"unbound"}),
    ] {
        store
            .assistant_output(&agent, Some("task"), data.clone(), true)
            .unwrap();
        assert!(saved(&store).0.is_none());
        assert_eq!(last_event(&store), data.to_string());
    }
}

#[test]
fn malformed_phase_does_not_erase_text_and_unknown_fields_remain_in_saved_data() {
    let store = crate::runtime::test_support::TestStore::new();
    let mut agent = agent("a", None);
    store.save(&agent).unwrap();
    input(&store, &mut agent, "task");
    store.joins_possible.set(true);
    let data = json!({"turn_id":"turn-task","text":"é\n\0","phase":{"invalid":7},
        "future":{"keep":[null,true]}});
    store
        .assistant_output(&agent, Some("task"), data.clone(), true)
        .unwrap();
    assert_eq!(saved(&store), (Some(data.to_string()), false));
    assert_eq!(last_event(&store), data.to_string());
    assert_eq!(
        store.selected_text("task").unwrap(),
        (
            "é\n\0".into(),
            crate::runtime::domain::OutputSelection::LastAssistantMessage
        )
    );
}

#[test]
fn empty_text_is_present_and_duplicate_metadata_uses_the_last_value() {
    let store = crate::runtime::test_support::TestStore::new();
    let mut agent = agent("a", None);
    store.save(&agent).unwrap();
    input(&store, &mut agent, "task");
    let data: Value = serde_json::from_str(concat!(
        "{\"turn_id\":7,\"turn_id\":\"turn-task\",\"text\":[],\"text\":\"\",",
        "\"phase\":null,\"phase\":\"final_answer\"}"
    ))
    .unwrap();
    store
        .assistant_output(&agent, Some("task"), data.clone(), true)
        .unwrap();
    assert_eq!(saved(&store), (Some(data.to_string()), true));
    assert_eq!(
        store.selected_text("task").unwrap(),
        (
            String::new(),
            crate::runtime::domain::OutputSelection::NoText
        )
    );
}

#[test]
fn unverified_text_and_absent_message_do_not_update_a_bound_call() {
    let store = crate::runtime::test_support::TestStore::new();
    let mut agent = agent("a", None);
    store.save(&agent).unwrap();
    input(&store, &mut agent, "task");
    let data = json!({"turn_id":"turn-task","text":"text","phase":"final_answer"});
    for (message, model_text) in [(Some("task"), false), (None, true)] {
        store
            .assistant_output(&agent, message, data.clone(), model_text)
            .unwrap();
        assert!(saved(&store).0.is_none());
        assert_eq!(last_event(&store), data.to_string());
    }
}
