use super::*;

const MAX_BYTES: i64 = 16 * 1024 * 1024;

#[test]
fn failed_recovery_rolls_back_uncertain_outbox_and_its_execution_ending() {
    let store = setup();
    let a = crate::runtime::test_support::AgentBuilder::new("active")
        .configure(|a| {
            a.execution_id = Some("execution".into());
            a.turn_id = Some("turn".into());
            a.state = crate::runtime::domain::AgentState::Running;
        })
        .build();
    store.save(&a).unwrap();
    store
        .event(&a, EventKind::ExecutionStarting, json!({}))
        .unwrap();
    store
        .test_connection()
        .execute_batch(
            "CREATE TEMP TRIGGER fail_state BEFORE UPDATE ON agents \
        WHEN NEW.state='stopped' BEGIN SELECT RAISE(ABORT,'injected failure'); END;",
        )
        .unwrap();
    assert_eq!(store.recover_agents().unwrap_err().code(), "database_error");
    assert!(queued(&store).is_empty());
    assert_eq!(
        store.agent("active").unwrap().state,
        crate::runtime::domain::AgentState::Running
    );
    let count: i64 = store
        .test_connection()
        .query_row(
            "SELECT count(*) FROM events WHERE kind='execution_stopped'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
    store
        .test_connection()
        .execute_batch("DROP TRIGGER fail_state")
        .unwrap();
    store.recover_agents().unwrap();
    store.recover_agents().unwrap();
    assert_eq!(queued(&store).len(), 1);
}

fn queued(store: &Store) -> Vec<String> {
    store
        .sql()
        .cached()
        .query_map(
            "SELECT body FROM comb_outbox ORDER BY sequence",
            [],
            |row| row.get(0),
        )
        .unwrap()
}
fn stored_event(store: &Store, kind: EventKind) -> String {
    store
        .query_row_cached(
            "SELECT data FROM events WHERE kind=?1 ORDER BY seq DESC LIMIT 1",
            [kind],
            |row| row.get(0),
        )
        .unwrap()
}

#[test]
fn full_submission_and_overflow_rows_keep_their_literal_canonical_bytes() {
    let store = setup();
    let evidence = Evidence {
        kind: EvidenceKind::Review,
        summary: "Public review.".into(),
        files: vec!["src/lib.rs".into()],
        commit: Some("abcdef0".into()),
        verdict: Some(crate::runtime::collectors::Verdict::NoFindings),
    };
    store
        .execute_cached("UPDATE comb_collectors SET bytes=?1", [MAX_BYTES])
        .unwrap();
    let admission = store.submit_evidence(&Caller::Human, &evidence).unwrap();
    assert_eq!(
        admission.destinations.get("shared"),
        Some(&Disposition::Full)
    );
    assert_eq!(
        stored_event(&store, EventKind::EvidenceSubmitted),
        concat!(
            "{\"actor\":\"human\",\"evidence\":{\"commit\":\"abcdef0\",",
            "\"files\":[\"src/lib.rs\"],\"kind\":\"review\",\"summary\":\"Public review.\",",
            "\"verdict\":\"no_findings\"},\"execution\":null,\"role\":null,",
            "\"source\":\"review\",\"turn\":null}"
        )
    );
    assert_eq!(
        stored_event(&store, EventKind::CollectorOverflow),
        format!(
            "{{\"destination\":\"shared\",\"first_skipped_event\":{}}}",
            admission.event
        )
    );
}

#[test]
fn evidence_capacity_uses_the_exact_queued_body_at_both_byte_edges() {
    let baseline = setup();
    baseline
        .submit_evidence(&Caller::Human, &evidence())
        .unwrap();
    let len = i64::try_from(queued(&baseline).remove(0).len()).unwrap();
    for (delta, expected) in [
        (-1, Disposition::Queued),
        (0, Disposition::Queued),
        (1, Disposition::Full),
    ] {
        let store = setup();
        store
            .execute_cached(
                "UPDATE comb_collectors SET bytes=?1",
                [MAX_BYTES - len + delta],
            )
            .unwrap();
        let result = store.submit_evidence(&Caller::Human, &evidence()).unwrap();
        assert_eq!(result.destinations.get("shared"), Some(&expected));
        let bodies = queued(&store);
        if expected == Disposition::Queued {
            assert_eq!(bodies.len(), 1);
            assert_eq!(i64::try_from(bodies[0].len()).unwrap(), len);
        } else {
            assert!(bodies.is_empty());
        }
    }
}

fn seed_gap(store: &Store, bytes: i64) {
    store
        .execute_cached(
            concat!(
                "UPDATE comb_collectors SET gap_count=2,gap_first=4,",
                "gap_last=5,bytes=?1 WHERE name='shared'"
            ),
            [bytes],
        )
        .unwrap();
}
#[test]
fn gap_capacity_uses_the_exact_queued_body_at_both_byte_edges() {
    let baseline = setup();
    seed_gap(&baseline, 0);
    baseline.collector_gap("shared").unwrap();
    let len = i64::try_from(queued(&baseline).remove(0).len()).unwrap();
    for delta in [-1, 0, 1] {
        let store = setup();
        seed_gap(&store, MAX_BYTES - len + delta);
        store.collector_gap("shared").unwrap();
        let bodies = queued(&store);
        if delta <= 0 {
            assert_eq!(bodies.len(), 1);
            assert_eq!(i64::try_from(bodies[0].len()).unwrap(), len);
            assert_eq!(store.collector_status().unwrap()[0].gap_count, 0);
        } else {
            assert!(bodies.is_empty());
            assert_eq!(store.collector_status().unwrap()[0].gap_count, 2);
        }
    }
}
