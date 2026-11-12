//! Native identity corruption must fail before retention or evidence transfer.
use super::*;
use rusqlite::params;

#[test]
fn noncanonical_generation_rolls_back_earlier_retained_sessions_and_evidence() {
    for operation in ["purge", "gc"] {
        for dry_run in [false, true] {
            for shared in [false, true] {
                corrupt_generation_case(operation, dry_run, shared);
            }
        }
    }
}

fn corrupt_generation_case(operation: &str, dry_run: bool, shared: bool) {
    let temp = crate::runtime::test_directory();
    let generation = uuid::Uuid::parse_str("00000000-0000-4000-8000-000000000001").unwrap();
    let home = home(temp.path(), generation);
    let transcript = write(&home, &rollout(SESSION));
    let store = open(temp.path());
    let departing = owner(&store, "departing", Provider::Codex, generation, 1);
    store
        .record_native_path(&departing, SESSION, &transcript)
        .unwrap();
    if shared {
        owner(&store, "survivor", Provider::Codex, generation, 20);
    }
    for id in ["departing", "survivor"] {
        if id == "survivor" && !shared {
            continue;
        }
        store
            .execute_cached(
                concat!(
                    "INSERT INTO native_sessions(agent,generation,provider,thread) ",
                    "VALUES (?1,?2,'codex',?3)"
                ),
                params![id, "ABCDEFAB-0000-4000-8000-000000000002", SESSION],
            )
            .unwrap();
    }
    let error = store
        .atomic(|s| {
            if operation == "purge" {
                s.purge(
                    "corrupt",
                    "departing",
                    PurgeOptions {
                        recursive: false,
                        dry_run,
                        keep_native_history: true,
                    },
                    &HashSet::new(),
                )
                .map(|_| ())
            } else {
                let args: GcArgs = serde_json::from_value(serde_json::json!({
                    "before":10,"dry_run":dry_run,"keep_native_history":true
                }))
                .unwrap();
                s.gc("corrupt", &args, &HashSet::new()).map(|_| ())
            }
        })
        .unwrap_err();
    assert_eq!(
        error.code(),
        "database_error",
        "{operation} {dry_run} {shared}"
    );
    assert_eq!(error.message(), "Invalid native session cleanup record.");
    assert!(store.agent("departing").is_ok());
    assert!(transcript.exists());
    assert_rollback(&store);
}

fn assert_rollback(store: &Store) {
    let counts: (i64, i64, i64) = store
        .query_row_cached(
            concat!(
                "SELECT (SELECT count(*) FROM native_cleanup_jobs),",
                "(SELECT count(*) FROM native_cleanup_sessions),",
                "(SELECT count(*) FROM native_cleanup_files)"
            ),
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(counts, (0, 0, 0));
    let paths: (i64, i64) = store
        .query_row_cached(
            concat!(
                "SELECT count(*) FILTER (WHERE agent='departing'),",
                "count(*) FILTER (WHERE agent='survivor') FROM native_session_paths"
            ),
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        paths,
        (1, 0),
        "no evidence transfer survives the failed transaction"
    );
}
