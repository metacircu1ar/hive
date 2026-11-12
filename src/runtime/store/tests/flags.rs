//! Corrupt SQL flags must never become plausible false values on any projection.
use super::*;

#[test]
fn direct_listing_child_and_purge_paths_reject_invalid_sql_boolean_storage() {
    use rusqlite::types::Value;
    let temporary = crate::runtime::test_directory();
    let store = Store::open(&temporary.path().join("db")).unwrap();
    let mut child = agent("child", Some("parent"));
    child.lifetime = crate::config::Lifetime::WithParent;
    store.save(&agent("parent", None)).unwrap();
    for field in ["paused", "expired"] {
        for value in [
            Value::Null,
            Value::Integer(2),
            Value::Real(0.5),
            Value::Text("invalid".into()),
            Value::Blob(vec![0]),
        ] {
            store.save(&child).unwrap();
            store
                .connection
                .pragma_update(None, "ignore_check_constraints", true)
                .unwrap();
            store
                .connection
                .execute(
                    &format!("UPDATE agents SET {field}=?1 WHERE id='child'"),
                    [&value],
                )
                .unwrap();
            store
                .connection
                .pragma_update(None, "ignore_check_constraints", false)
                .unwrap();
            assert_eq!(store.agent("child").unwrap_err().code(), "database_error");
            assert_eq!(
                store.agent_page(0, 100, None, None).err().unwrap().code(),
                "database_error"
            );
            // The dependent query intentionally selects only expired=0; use paused
            // corruption to exercise its projection without changing that policy.
            if field == "paused" {
                assert_eq!(
                    store.children("parent").unwrap_err().code(),
                    "database_error"
                );
            }
            let error = store
                .purge(
                    "request",
                    "child",
                    api::PurgeOptions {
                        recursive: false,
                        dry_run: true,
                        keep_native_history: true,
                    },
                    &HashSet::new(),
                )
                .err()
                .unwrap();
            assert_eq!(error.code(), "database_error");
        }
    }
}
