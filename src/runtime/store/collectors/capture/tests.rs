use super::*;
use crate::{error::ErrorCode, runtime::test_support::AgentBuilder};
use serde_json::json;

#[test]
fn recovery_capture_requires_both_uncertainty_markers() {
    let agent = AgentBuilder::new("agent").build();
    for (reason, status, capture) in [
        ("daemon_restarted", "uncertain", true),
        ("daemon_restarted", "completed", false),
        ("ordinary_stop", "uncertain", false),
    ] {
        let data = json!({"reason":reason,"outcome":status,"turn_id":"old-turn"});
        let record = automatic(&agent, EventKind::ExecutionStopped, &data);
        assert_eq!(record.is_some(), capture);
        if let Some((source, record)) = record {
            assert_eq!(source, CollectionSource::TurnFailed);
            let record = crate::api::value(record);
            assert_eq!(record["turn"], "old-turn");
            assert_eq!(record["evidence"], json!({"outcome":"uncertain"}));
        }
    }
    let ordinary = crate::api::value(crate::runtime::api::events::ExecutionStopped::Outcome {
        task_outcome: crate::runtime::domain::TaskOutcome::Open,
    });
    assert!(automatic(&agent, EventKind::ExecutionStopped, &ordinary).is_none());
}

#[test]
fn evidence_and_overflow_history_wake_observers_without_worker_activation() {
    let directory = crate::runtime::test_directory();
    let core = crate::runtime::test_support::CoreBuilder::new(directory.path()).build();
    let store = core.store();
    store.changes();
    let evidence = Evidence {
        kind: crate::runtime::collectors::EvidenceKind::Correction,
        summary: "Keep this public finding".into(),
        files: vec![],
        commit: None,
        verdict: None,
    };
    store.submit_evidence(&Caller::Human, &evidence).unwrap();
    let changes = store.changes();
    assert!(changes.observed.agents.contains("human"));
    assert!(changes.agents.is_empty());
    store
        .atomic(|s| s.collector_overflow("human", "destination", 1))
        .unwrap();
    let changes = store.changes();
    assert!(changes.observed.agents.contains("human"));
    assert!(changes.agents.is_empty());
    store.begin().unwrap();
    store.collector_overflow("human", "rolled-back", 2).unwrap();
    store.rollback().unwrap();
    let count: i64 = store
        .test_connection()
        .query_row(
            "SELECT count(*) FROM events WHERE kind='collector_overflow'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn malformed_error_containers_are_silent_but_present_bad_codes_reach_the_observer() {
    for data in [
        json!(null),
        json!([]),
        json!(7),
        json!("x"),
        json!({}),
        json!({"error":null}),
        json!({"error":"x"}),
        json!({"error":5}),
        json!({"error":[]}),
        json!({"error":{}}),
    ] {
        let view: projection::Failure<'_> = projection::read(&data);
        let mut called = false;
        assert!(
            projection::resolve::<ErrorCode>(view.error.code, |_| {
                called = true;
                None
            })
            .is_none()
        );
        assert!(!called);
    }
    for code in [
        json!(null),
        json!(5),
        json!("unknown"),
        json!({"invalid_request":null}),
    ] {
        let data = json!({"error":{"code":code},"turn_id":7});
        let view: projection::Failure<'_> = projection::read(&data);
        let mut called = false;
        assert!(
            projection::resolve::<ErrorCode>(view.error.code, |result| {
                called = true;
                assert!(result.is_err());
                None
            })
            .is_none()
        );
        assert!(called);
        assert!(view.turn_id.is_none());
    }
}

#[test]
fn approval_absence_is_silent_and_present_bad_actions_are_observed() {
    for data in [json!(null), json!([]), json!({}), json!({"origin":7})] {
        let view: projection::Answer<'_> = projection::read(&data);
        let mut called = false;
        assert!(
            projection::resolve::<crate::approvals::ApprovalAction>(view.action, |_| {
                called = true;
                None
            })
            .is_none()
        );
        assert!(!called);
    }
    for action in [json!(null), json!(7), json!("unknown"), json!([])] {
        let data = json!({"action":action,"origin":{"execution":"execution","turn":7}});
        let view: projection::Answer<'_> = projection::read(&data);
        let mut called = false;
        assert!(
            projection::resolve::<crate::approvals::ApprovalAction>(view.action, |result| {
                called = true;
                assert!(result.is_err());
                None
            })
            .is_none()
        );
        assert!(called);
        assert_eq!(view.origin.execution, Some("execution"));
        assert!(view.origin.turn.is_none());
    }
}

#[test]
fn automatic_records_pin_attribution_nulls_and_omitted_error_codes() {
    let agent = AgentBuilder::new("agent")
        .configure(|agent| agent.role = None)
        .build();
    for (kind, data, source, outcome, code) in [
        (
            EventKind::TaskCompleted,
            json!([]),
            "task_completed",
            "completed",
            "",
        ),
        (
            EventKind::TurnFinished,
            json!({"status":"failed"}),
            "turn_failed",
            "failed",
            "",
        ),
        (
            EventKind::TurnFinished,
            json!({"status":"interrupted"}),
            "turn_failed",
            "interrupted",
            "",
        ),
        (
            EventKind::ExecutionFailed,
            json!({"error":{"code":"invalid_request"}}),
            "turn_failed",
            "failed",
            "\"error_code\":\"invalid_request\",",
        ),
    ] {
        let (_, record) = automatic(&agent, kind, &data).unwrap();
        let expected = format!(
            concat!(
                "{{\"actor\":\"agent\",\"evidence\":{{{}\"outcome\":\"{}\"}},",
                "\"execution\":null,\"role\":null,\"source\":\"{}\",\"turn\":null}}"
            ),
            code, outcome, source
        );
        assert_eq!(crate::api::value(record).to_string(), expected);
    }
    let data: Value = serde_json::from_str(concat!(
        "{\"action\":7,\"action\":\"accept\",\"approval_id\":\"id\",",
        "\"origin\":{\"turn\":\"turn\",\"execution\":\"execution\"}}"
    ))
    .unwrap();
    let (_, record) = automatic(&agent, EventKind::ApprovalClosed, &data).unwrap();
    assert_eq!(
        crate::api::value(record).to_string(),
        concat!(
            "{\"actor\":\"human\",\"approval_id\":\"id\",\"evidence\":{\"action\":\"accept\"},",
            "\"execution\":\"execution\",\"source\":\"approval_answer\",",
            "\"subject_agent\":\"agent\",\"subject_role\":null,\"turn\":\"turn\"}"
        )
    );
}

#[test]
fn event_specific_projection_keeps_valid_fields_and_last_duplicate_values() {
    let agent = AgentBuilder::new("agent")
        .configure(|agent| agent.role = None)
        .build();
    for raw in [
        r#"{"turn_id":{},"error":{"code":7,"code":"invalid_request"},"status":[]}"#,
        r#"{"status":{},"error":{"code":"invalid_request"},"turn_id":[]}"#,
    ] {
        let data: Value = serde_json::from_str(raw).unwrap();
        assert!(automatic(&agent, EventKind::ExecutionFailed, &data).is_some());
        assert!(automatic(&agent, EventKind::TurnFinished, &data).is_none());
    }
    for data in [
        json!({}),
        json!(null),
        json!({"status":"completed"}),
        json!({"status":7}),
    ] {
        assert!(automatic(&agent, EventKind::TurnFinished, &data).is_none());
    }
}
