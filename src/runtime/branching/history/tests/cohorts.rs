use super::*;

fn id(number: u8) -> String {
    format!("00000000-0000-4000-8000-{number:012x}")
}

fn assistant(number: u8, parent: u8, message: &str, tool: &str) -> Value {
    json!({"type":"assistant","uuid":id(number),"parentUuid":id(parent),
        "message":{"role":"assistant","id":message,"content":[
            {"type":"tool_use","id":tool,"name":"Bash","input":{"command":"true"}}]}})
}

fn result(number: u8, source: u8, tool: &str) -> Value {
    json!({"type":"user","uuid":id(number),"parentUuid":id(source),
        "sourceToolAssistantUUID":id(source),"message":{"role":"user","content":[
            {"type":"tool_result","tool_use_id":tool,"content":"done"}]}})
}

fn prefix() -> Vec<Value> {
    vec![
        row(&id(1), None, "source input"),
        assistant(2, 1, "msg-one", "tool-a"),
        assistant(3, 2, "msg-one", "tool-b"),
        result(4, 3, "tool-b"), // This result also satisfies ordinary linear ancestry.
        result(5, 2, "tool-a"), // Native parallel result back-edge.
        json!({"type":"assistant","uuid":id(6),"parentUuid":id(5),
            "message":{"role":"assistant","id":"msg-final", "content":[
                {"type":"text","text":"source finished"}]}}),
    ]
}

fn parsed(values: &[Value]) -> Result<Vec<Dialogue>> {
    decode(
        values
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n")
            .as_bytes(),
    )
}

#[test]
fn native_parallel_results_keep_the_complete_exact_dialogue_prefix() {
    let source = prefix();
    let before = parsed(&source).unwrap();
    assert_eq!(before.len(), 6);
    let mut fork = source.clone();
    fork.push(row(&id(7), Some(&id(6)), "fresh branch input"));
    extends(&before, &parsed(&fork).unwrap()).unwrap();
    assert_eq!(before[4].parent.as_deref(), Some(id(2).as_str()));
    fork[3]["message"]["content"][0]["content"] = json!("edited copied result");
    assert!(extends(&before, &parsed(&fork).unwrap()).is_err());
}

#[test]
fn native_attachment_links_are_validated_without_becoming_dialogue() {
    let mut source = prefix();
    source[1]["parentUuid"] = json!(id(8));
    source.insert(
        1,
        json!({"type":"attachment","uuid":id(8),"parentUuid":id(1)}),
    );
    assert_eq!(parsed(&source).unwrap().len(), 6);
    for (key, bad) in [
        ("uuid", json!("bad")),
        ("parentUuid", json!("bad")),
        ("parentUuid", json!(id(9))),
        ("type", json!("unknown-metadata")),
    ] {
        let mut broken = source.clone();
        broken[1][key] = bad;
        assert!(parsed(&broken).is_err(), "{key}");
    }
    let mut duplicate = source.clone();
    duplicate.insert(2, source[1].clone());
    assert!(parsed(&duplicate).is_err());
    source[1]["parentUuid"] = json!(id(9));
    source.insert(
        2,
        json!({"type":"attachment","uuid":id(9),"parentUuid":id(8)}),
    );
    assert!(parsed(&source).is_err());
}

#[test]
fn a_parallel_result_needs_the_matching_current_source_and_tool() {
    for (key, bad) in [
        ("sourceToolAssistantUUID", json!(id(3))),
        ("sourceToolAssistantUUID", Value::Null),
        ("parentUuid", json!(id(1))),
    ] {
        let mut broken = prefix();
        broken[4][key] = bad;
        assert!(parsed(&broken).is_err(), "{key}");
    }
    let mut missing = prefix();
    missing[4]
        .as_object_mut()
        .unwrap()
        .retain(|key, _| key != "sourceToolAssistantUUID");
    assert!(parsed(&missing).is_err());
    let mut wrong_tool = prefix();
    wrong_tool[4]["message"]["content"][0]["tool_use_id"] = json!("tool-b");
    assert!(parsed(&wrong_tool).is_err());
}

#[test]
fn mixed_or_arbitrary_user_back_edges_cannot_masquerade_as_results() {
    let mut mixed = prefix();
    mixed[4]["message"]["content"]
        .as_array_mut()
        .unwrap()
        .push(json!({"type":"text","text":"unproved extra dialogue"}));
    assert!(parsed(&mixed).is_err());
    let mut arbitrary = prefix();
    arbitrary[4] = row(&id(5), Some(&id(2)), "disconnected user dialogue");
    assert!(parsed(&arbitrary).is_err());
}

#[test]
fn ordinary_rule_results_consume_ids_before_later_parallel_duplicates() {
    let mut duplicate = prefix();
    duplicate[4] = result(5, 3, "tool-b");
    assert!(parsed(&duplicate).is_err());
    let mut repeated_block = prefix();
    let block = repeated_block[4]["message"]["content"][0].clone();
    repeated_block[4]["message"]["content"]
        .as_array_mut()
        .unwrap()
        .push(block);
    assert!(parsed(&repeated_block).is_err());
    let mut repeated_call = prefix();
    repeated_call[2]["message"]["content"][0]["id"] = json!("tool-a");
    assert!(parsed(&repeated_call).is_err());
}

#[test]
fn ordinary_user_or_a_new_assistant_message_ends_the_old_cohort() {
    let mut user = prefix();
    user.insert(4, row(&id(8), Some(&id(4)), "new ordinary user input"));
    assert!(parsed(&user).is_err());
    let mut assistant = prefix();
    assistant[2]["message"]["id"] = json!("msg-other");
    assert!(parsed(&assistant).is_err());
}

#[test]
fn absent_null_and_non_string_message_ids_cannot_form_a_cohort() {
    for invalid in [Value::Null, json!(17), json!(false), json!("")] {
        let mut broken = prefix();
        broken[1]["message"]["id"] = invalid.clone();
        broken[2]["message"]["id"] = invalid;
        assert!(parsed(&broken).is_err());
    }
    let mut missing = prefix();
    for value in &mut missing[1..=2] {
        value["message"]
            .as_object_mut()
            .unwrap()
            .retain(|key, _| key != "id");
    }
    assert!(parsed(&missing).is_err());
}

#[test]
fn parallel_proofs_do_not_admit_duplicate_ids_or_unknown_parents() {
    let mut duplicate = prefix();
    duplicate[4]["uuid"] = json!(id(4));
    assert!(parsed(&duplicate).is_err());
    let mut unknown = prefix();
    unknown[1]["parentUuid"] = json!(id(9));
    assert!(parsed(&unknown).is_err());
    let mut malformed = prefix();
    malformed[4]["uuid"] = json!("not-a-uuid");
    assert!(parsed(&malformed).is_err());
}
