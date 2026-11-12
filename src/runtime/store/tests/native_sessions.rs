//! Native ownership must outlive login switches, failed startup and event pruning.
use super::*;

fn rows(store: &Store, table: &str) -> i64 {
    store
        .query_row_cached(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

#[test]
fn native_bindings_retain_each_generation_and_acknowledge_only_its_attempt() {
    let store = crate::runtime::test_support::TestStore::new();
    let mut a = agent("worker", None);
    a.execution_id = Some("first".into());
    store
        .atomic(|s| {
            s.save(&a)?;
            s.begin_native_creation(&a)?;
            s.record_native_binding(&a, "first-native")
        })
        .unwrap();
    let first_generation = a.auth_generation;
    a.auth_generation = uuid::Uuid::new_v4();
    a.execution_id = Some("second".into());
    store
        .atomic(|s| {
            s.save(&a)?;
            s.begin_native_creation(&a)?;
            s.record_native_binding(&a, "second-native")?;
            s.record_native_binding(&a, "second-native")
        })
        .unwrap();
    assert_eq!(rows(&store, "native_sessions"), 2);
    assert_eq!(rows(&store, "native_session_attempts"), 0);
    let preserved: String = store
        .query_row_cached(
            "SELECT thread FROM native_sessions WHERE generation=?1",
            [first_generation.to_string()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(preserved, "first-native");
    a.execution_id = Some("third".into());
    store.atomic(|s| s.begin_native_creation(&a)).unwrap();
    // Clearing a different execution cannot erase an unresolved creation.
    a.execution_id = Some("fourth".into());
    store
        .atomic(|s| s.native_creation_not_submitted(&a))
        .unwrap();
    assert_eq!(rows(&store, "native_session_attempts"), 1);
    a.execution_id = Some("third".into());
    store
        .atomic(|s| s.native_creation_not_submitted(&a))
        .unwrap();
    assert_eq!(rows(&store, "native_session_attempts"), 0);
    assert_eq!(rows(&store, "native_sessions"), 2);
}

#[test]
fn native_binding_and_creation_acknowledgement_roll_back_together() {
    let store = crate::runtime::test_support::TestStore::new();
    let mut a = agent("worker", None);
    a.execution_id = Some("execution".into());
    store
        .atomic(|s| {
            s.save(&a)?;
            s.begin_native_creation(&a)
        })
        .unwrap();
    store.begin().unwrap();
    store.record_native_binding(&a, "native").unwrap();
    assert_eq!(rows(&store, "native_session_attempts"), 0);
    store.rollback().unwrap();
    assert_eq!(rows(&store, "native_sessions"), 0);
    assert_eq!(rows(&store, "native_session_attempts"), 1);
    // Ordinary saved agents with preallocated identities have no bound history.
    let mut unstarted = agent("unstarted", None);
    unstarted.thread_id = Some(uuid::Uuid::new_v4().to_string());
    store.atomic(|s| s.save(&unstarted)).unwrap();
    assert_eq!(rows(&store, "native_sessions"), 0);
}
