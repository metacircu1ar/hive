//! Byte checks against the expressions frozen before S2.
use super::*;
mod original;

#[test]
fn query_bytes_match_before_wrapping() {
    for (actual, expected) in [
        (MEMBERS, original::MEMBERS),
        (TASK_OWNERS, original::TASK_OWNERS),
    ] {
        assert_eq!(actual.as_bytes(), expected.as_bytes());
    }
}

#[test]
fn open_join_preview_uses_the_ordered_partial_index() {
    let db = rusqlite::Connection::open_in_memory().unwrap();
    db.execute_batch(
        "CREATE TABLE joins(id TEXT, requester TEXT, state TEXT);
        CREATE INDEX joins_open_requester ON joins(requester,id)
        WHERE state IN ('collecting','deadline_passed');",
    )
    .unwrap();
    let mut query = db.prepare(&format!("EXPLAIN QUERY PLAN {OPEN}")).unwrap();
    let plan: Vec<String> = query
        .query_map(rusqlite::params!["caller", 1025], |r| r.get(3))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert!(
        plan.iter()
            .any(|line| line.contains("joins_open_requester")),
        "{plan:?}"
    );
    assert!(
        !plan.iter().any(|line| line.contains("TEMP B-TREE")),
        "{plan:?}"
    );
}
