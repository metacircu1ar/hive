//! Batch replay regressions.
use super::*;

#[test]
fn ordinary_batch_cannot_overwrite_its_outer_replay_receipt() {
    let temp = crate::runtime::test_directory();
    let mut core = core(temp.path());
    core.store().save(&agent("caller")).unwrap();
    core.store().save(&agent("recipient")).unwrap();
    let send = |id| json!({"op":"send","target":"recipient","body":"work","request_id":id});
    let batch = json!({"op":"batch","operations":[send("first"),send("outer")]});
    assert_error(
        call(&mut core, "caller", "outer", batch),
        batch::at_index(
            Error::request_conflict("Batch operations must not reuse the batch request ID."),
            1,
        ),
    );
    for id in ["first", "outer"] {
        assert!(!core.store().request_used("caller", id).unwrap());
    }
    assert!(
        core.store()
            .messages("recipient", None, 200)
            .map(crate::api::value)
            .unwrap()["messages"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn request_id_rules_agree_for_hosted_tools_and_batch_items() {
    for (id, accepted) in [
        ("retry 1".into(), true),
        ("é".repeat(64), true),
        ("é".repeat(65), false),
        ("x".repeat(129), false),
        ("private\nidentity".into(), false),
        ("private\u{85}identity".into(), false),
        (String::new(), false),
    ] {
        let args = json!({"target":"a", "body":"hello", "request_id":id});
        assert_eq!(validate_tool("hive_send", &args).is_ok(), accepted);
        let mut item = args;
        item["op"] = json!("send");
        let decoded: BatchItem = serde_json::from_value(item.clone()).unwrap();
        assert_eq!(decoded.operation().is_ok(), accepted);
        assert_eq!(
            validate_tool(
                "hive_batch",
                &json!({"request_id":"outer", "operations":[item]})
            )
            .is_ok(),
            accepted
        );
    }
}

#[test]
fn replay_uses_original_fields_and_precedes_lifecycle_and_validation() {
    let temp = crate::runtime::test_directory();
    let mut core = core(temp.path());
    core.store().save(&agent("caller")).unwrap();
    core.store().save(&agent("recipient")).unwrap();
    let raw = json!({"op":"send","target":"recipient","body":"work","extra":{"retained":true}});
    let envelope = RequestEnvelope::new(raw.clone());
    assert_eq!(envelope.fingerprint(), raw.to_string());
    let accepted = call(&mut core, "caller", "send", raw.clone()).unwrap();
    let mut caller = core.store().agent("caller").unwrap();
    caller.task_outcome = TaskOutcome::Completed;
    core.store().save(&caller).unwrap();
    assert_eq!(
        call(&mut core, "caller", "send", raw.clone()).unwrap(),
        accepted
    );
    for changed in [
        json!({"op":"send","target":"recipient","body":"work"}),
        json!({
            "op": "send",
            "target": "recipient",
            "body": "work",
            "extra": {
                "retained": true
            },
            "mode": null
        }),
        json!({}),
    ] {
        assert_error(
            call(&mut core, "caller", "send", changed),
            Error::request_conflict("Request ID was already used with different arguments."),
        );
    }
    assert_error(
        call(
            &mut core,
            "caller",
            "new",
            json!({"op":"send","mode":"unknown"}),
        ),
        completed(),
    );
    // Read-only dispatch bypasses both the conflicting mutation ID and lifecycle.
    assert!(
        call(
            &mut core,
            "caller",
            "send",
            json!({"op":"status","target":"recipient"})
        )
        .is_ok()
    );
    assert_eq!(
        core.store()
            .messages("recipient", None, 200)
            .map(crate::api::value)
            .unwrap()["messages"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn batch_validation_is_sequential_and_failed_batches_rollback_inner_replay() {
    let temp = crate::runtime::test_directory();
    let mut core = core(temp.path());
    core.store().save(&agent("caller")).unwrap();
    core.store().save(&agent("recipient")).unwrap();
    assert_batch_rollback(&mut core);
    assert_batch_operation_shape(&mut core);
}

#[test]
fn batch_inner_and_outer_replays_survive_caller_termination() {
    let temp = crate::runtime::test_directory();
    let mut core = core(temp.path());
    core.store().save(&agent("caller")).unwrap();
    core.store().save(&agent("recipient")).unwrap();
    let send =
        json!({"op":"send","target":"recipient","body":"work","request_id":"inner","extra":true});
    let batch = json!({
        "op": "batch",
        "operations": [
            send,
            {
                "op": "terminate",
                "target": "caller",
                "request_id": "cancel"
            },
            send
        ]
    });
    let accepted = call(&mut core, "caller", "batch", batch.clone()).unwrap();
    assert_eq!(accepted["results"].as_array().unwrap().len(), 3);
    assert_eq!(accepted["results"][0], accepted["results"][2]);
    assert_eq!(
        core.store().agent("caller").unwrap().task_outcome,
        TaskOutcome::Cancelled
    );
    assert_eq!(call(&mut core, "caller", "batch", batch).unwrap(), accepted);
    assert_eq!(
        core.store()
            .messages("recipient", None, 200)
            .map(crate::api::value)
            .unwrap()["messages"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        core.store()
            .prior(
                "caller",
                "inner",
                &json!({"op":"send","target":"recipient","body":"work","extra":true}).to_string()
            )
            .unwrap()
            .unwrap(),
        accepted["results"][0]
    );
}

#[test]
fn internal_restoration_replay_uses_a_distinct_caller_namespace() {
    use crate::runtime::{core::preflight::Facts, hub::Caller};
    let tmp = crate::runtime::test_directory();
    let mut core = core(tmp.path());
    let request = RequestEnvelope::new(json!({"op":"login_restored", "auth":"main"}));
    let body = request.fingerprint();
    core.store()
        .remember(
            Caller::Human.name(),
            "same-id",
            &body,
            &json!({"owner":"human"}),
        )
        .unwrap();
    assert!(
        core.replay(&Caller::System, "same-id", &request)
            .unwrap()
            .is_none()
    );
    core.store()
        .remember(
            Caller::System.name(),
            "same-id",
            &body,
            &json!({"owner":"system"}),
        )
        .unwrap();
    for (caller, owner) in [(Caller::Human, "human"), (Caller::System, "system")] {
        assert_eq!(
            core.dispatch(&caller, "same-id", &request, &Facts::default())
                .map(crate::api::value)
                .unwrap(),
            json!({"owner":owner})
        );
        let changed = RequestEnvelope::new(json!({"op":"login_restored", "auth":"other"}));
        assert!(matches!(
            core.replay(&caller, "same-id", &changed),
            Err(Error::RequestConflict { .. })
        ));
    }
    assert!(
        core.replay(&Caller::test_agent("agent"), "same-id", &request)
            .unwrap()
            .is_none()
    );
}
