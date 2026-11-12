//! Input collection preserves lazy batch validation and replay exclusion.
use super::*;
use serde_json::{Value, json};

mod resume;

#[test]
fn m5b_invalid_branch_source_precedes_lookup_and_does_not_capture_it() {
    let temp = crate::runtime::test_directory();
    let core = crate::runtime::test_support::CoreBuilder::new(temp.path()).build();
    for source in [
        String::new(),
        "x".repeat(crate::limits::CONTROL_INPUT_BYTES + 1),
    ] {
        let envelope =
            RequestEnvelope::new(json!({"op":"spawn","from_agent":source,"body":"review"}));
        let input = RequestInput::capture(&core, &Caller::Human, &envelope);
        assert!(input.sources.is_empty());
        let Request::Spawn(args) = &envelope.request else {
            panic!()
        };
        let error = input.spawn_inputs(args).resolved.err().unwrap();
        assert!(matches!(error, Error::Input { .. }));
        assert_eq!(crate::api::value(error)["field"], "/from_agent");
    }
}

fn member(id: &str, raw: &Value) -> Value {
    let mut item = raw.clone();
    item.as_object_mut()
        .unwrap()
        .insert("request_id".into(), id.into());
    item
}

fn keys(core: &Core, envelope: &RequestEnvelope) -> Vec<String> {
    RequestInput::capture(core, &Caller::Human, envelope)
        .operations
        .into_iter()
        .map(|input| input.key)
        .collect()
}

#[test]
fn batch_collection_preserves_order_and_skips_invalid_replayed_and_conflicting_members() {
    let temp = crate::runtime::test_directory();
    let core = crate::runtime::test_support::CoreBuilder::new(temp.path()).build();
    let first = json!({"op":"spawn","role":"first","body":"a","extra":"retained"});
    let last = json!({"op":"spawn","role":"last","body":"b"});
    let envelope = RequestEnvelope::new(json!({"op":"batch","operations":[
        member("first", &first),
        7,
        {"op":"batch","request_id":"invalid-nesting","operations":[]},
        {"op":"terminate","request_id":"no-preflight","target":"agent"},
        member("last", &last)
    ]}));
    assert_eq!(
        keys(&core, &envelope),
        [first.to_string(), last.to_string()]
    );
    core.store()
        .remember(
            "human",
            "first",
            &first.to_string(),
            &json!({"recorded":true}),
        )
        .unwrap();
    assert_eq!(keys(&core, &envelope), [last.to_string()]);
    let conflict = RequestEnvelope::new(json!({"op":"batch","operations":[
        {"op":"spawn","request_id":"first","role":"different","body":"a"},
        member("last", &last)
    ]}));
    assert_eq!(keys(&core, &conflict), [last.to_string()]);
}
