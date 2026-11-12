use super::*;
use serde_json::{Value, json};

fn models(tokens: u64) -> ModelUsage {
    ModelUsage(BTreeMap::from([(
        "sonnet[1m]".into(),
        ModelCounters {
            input_tokens: tokens,
            output_tokens: tokens,
            cache_read_input_tokens: tokens,
            cache_creation_input_tokens: tokens,
            web_search_requests: Some(tokens),
        },
    )]))
}
#[test]
fn successful_snapshots_compare_only_the_same_session_and_do_not_sum() {
    let first = NativeUsage::new(
        crate::runtime::domain::NativeObservation {
            session_id: "session".into(),
            turn_id: "one".into(),
            outcome: ProviderTurnStatus::Completed,
            models: models(10),
            observed_at: 1,
        },
        None,
    );
    let failure = NativeUsage::new(
        crate::runtime::domain::NativeObservation {
            session_id: "session".into(),
            turn_id: "two".into(),
            outcome: ProviderTurnStatus::Failed,
            models: models(0),
            observed_at: 2,
        },
        Some(&first),
    );
    assert_eq!(failure.reset, None);
    let next = NativeUsage::new(
        crate::runtime::domain::NativeObservation {
            session_id: "session".into(),
            turn_id: "three".into(),
            outcome: ProviderTurnStatus::Completed,
            models: models(11),
            observed_at: 3,
        },
        Some(&first),
    );
    assert_eq!(next.reset, Some(false));
    assert_eq!(next.models.0["sonnet[1m]"].input_tokens, 11);
    let reset = NativeUsage::new(
        crate::runtime::domain::NativeObservation {
            session_id: "session".into(),
            turn_id: "four".into(),
            outcome: ProviderTurnStatus::Completed,
            models: models(1),
            observed_at: 4,
        },
        Some(&next),
    );
    assert_eq!(reset.reset, Some(true));
    let removed = NativeUsage::new(
        crate::runtime::domain::NativeObservation {
            session_id: "session".into(),
            turn_id: "five".into(),
            outcome: ProviderTurnStatus::Completed,
            models: ModelUsage::default(),
            observed_at: 5,
        },
        Some(&next),
    );
    assert_eq!(removed.reset, Some(true));
    let fork = NativeUsage::new(
        crate::runtime::domain::NativeObservation {
            session_id: "fork".into(),
            turn_id: "six".into(),
            outcome: ProviderTurnStatus::Completed,
            models: models(1),
            observed_at: 6,
        },
        Some(&next),
    );
    assert_eq!(fork.reset, Some(false));
}
#[test]
fn model_map_accepts_the_existing_model_syntax_and_rejects_partial_or_oversized_maps() {
    let counters = json!({"input_tokens":1,"output_tokens":2,"cache_read_input_tokens":0,"cache_creation_input_tokens":0});
    let valid: ModelUsage = serde_json::from_value(
        json!({"provider/model:revision@alias":counters,"sonnet[1m]":counters}),
    )
    .unwrap();
    assert_eq!(valid.0.len(), 2);
    for bad in [
        json!({"valid":counters,"invalid name":counters}),
        json!({"valid":counters,"bad":{"input_tokens":-1}}),
        json!([]),
    ] {
        assert!(serde_json::from_value::<ModelUsage>(bad).is_err());
    }
    let many: BTreeMap<_, _> = (0..33)
        .map(|n| (format!("model-{n}"), counters.clone()))
        .collect();
    assert!(serde_json::from_value::<ModelUsage>(json!(many)).is_err());
    let duplicate = r#"{"sonnet":{"input_tokens":1,"output_tokens":0,"cache_read_input_tokens":0,"cache_creation_input_tokens":0},"sonnet":{"input_tokens":2,"output_tokens":0,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}"#;
    assert!(serde_json::from_str::<ModelUsage>(duplicate).is_err());
}

#[test]
fn saved_agent_usage_requires_a_successful_snapshot_with_a_reset_observation() {
    let mut agent = crate::runtime::test_support::agent();
    agent.native_usage = Some(NativeUsage::new(
        crate::runtime::domain::NativeObservation {
            session_id: "session".into(),
            turn_id: "turn".into(),
            outcome: ProviderTurnStatus::Completed,
            models: models(10),
            observed_at: 1,
        },
        None,
    ));
    let saved = serde_json::to_value(agent).unwrap();
    serde_json::from_value::<crate::runtime::domain::Agent>(saved.clone()).unwrap();
    for (key, value) in [("outcome", json!("failed")), ("reset", Value::Null)] {
        let mut invalid = saved.clone();
        invalid["native_usage"][key] = value;
        assert!(serde_json::from_value::<crate::runtime::domain::Agent>(invalid).is_err());
    }
}
