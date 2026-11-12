//! Session evidence survives owner removal; unsupported paths never reach JSON.
use super::*;
mod corruption;

fn owner(store: &Store, id: &str, provider: Provider, generation: uuid::Uuid, ended: u64) -> Agent {
    let a = AgentBuilder::new(id)
        .configure(|a| {
            a.profile.provider = provider;
            a.auth_generation = generation;
            a.thread_id = Some(SESSION.into());
            a.state = AgentState::Stopped;
            a.task_outcome = TaskOutcome::Completed;
            a.conversation_clean = true;
            a.ended_at = Some(ended);
        })
        .build();
    store
        .atomic(|s| {
            s.save_at(&a, ended)?;
            s.record_native_binding(&a, SESSION)
        })
        .unwrap();
    a
}

#[test]
fn shared_paths_and_verification_survive_purge_keep_gc_and_restart() {
    for provider in [Provider::Codex, Provider::Claude] {
        for evidence in ["missing", "obsolete", "unverified"] {
            for operation in ["purge", "keep", "gc"] {
                shared_owner_case(provider, evidence, operation);
            }
        }
    }
}

fn shared_owner_case(provider: Provider, evidence: &str, operation: &str) {
    let temp = crate::runtime::test_directory();
    let generation = uuid::Uuid::new_v4();
    let home = home(temp.path(), generation);
    let actual = match provider {
        Provider::Codex => rollout(SESSION),
        Provider::Claude => format!("projects/original/{SESSION}.jsonl"),
    };
    let transcript = write(&home, &actual);
    let store = open(temp.path());
    let departing = owner(&store, "departing", provider, generation, 1);
    let survivor = owner(&store, "survivor", provider, generation, 20);
    store
        .atomic(|s| {
            s.record_native_path(&departing, SESSION, &transcript)?;
            survivor_evidence(s, &survivor, &home, &actual, evidence)
        })
        .unwrap();
    let before: i64 = store
        .query_row_cached(
            "SELECT count(*) FROM native_session_paths WHERE agent='survivor'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let preview = purge(&store, "preview", "departing", false, true);
    assert_eq!(preview.native_cleanup.native_sessions_retained, 1);
    assert_eq!(
        store
            .query_row_cached(
                "SELECT count(*) FROM native_session_paths WHERE agent='survivor'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        before,
        "dry-run must not transfer evidence"
    );
    if operation == "gc" {
        let args: GcArgs = serde_json::from_value(serde_json::json!({"before":10})).unwrap();
        let page = store
            .atomic(|s| s.gc("first", &args, &HashSet::new()))
            .unwrap();
        assert_eq!(page.stats.deleted_agents, 1);
    } else {
        purge(&store, "first", "departing", operation == "keep", false);
    }
    store.finish_native_deletions().unwrap();
    assert!(transcript.exists());
    drop(store);
    let store = open(temp.path());
    let verified: bool = store
        .query_row_cached(
            "SELECT verified FROM native_session_paths WHERE agent='survivor' AND path=?1",
            [&actual],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        verified,
        "the surviving owner must retain observed location evidence"
    );
    purge(&store, "last", "survivor", false, false);
    store.finish_native_deletions().unwrap();
    let receipt = store.native_cleanup_receipt("last").unwrap().unwrap();
    assert_eq!(
        receipt.native_sessions_deleted, 1,
        "{provider:?}/{evidence}/{operation}"
    );
    assert_eq!(receipt.native_sessions_absent, 0);
    assert_eq!(receipt.native_warning_count, 0);
    assert!(!transcript.exists());
}

fn survivor_evidence(
    store: &Store,
    survivor: &Agent,
    home: &Path,
    actual: &str,
    evidence: &str,
) -> Result<()> {
    match evidence {
        "obsolete" => {
            let old = match survivor.profile.provider {
                Provider::Codex => {
                    format!("sessions/2025/01/01/rollout-old-{SESSION}.jsonl")
                }
                Provider::Claude => format!("projects/obsolete/{SESSION}.jsonl"),
            };
            store.record_native_path(survivor, SESSION, &home.join(old))?;
        }
        "unverified" => {
            store.execute_cached(
                super::super::super::super::queries::RECORD_PATH,
                rusqlite::params![
                    survivor.id,
                    survivor.auth_generation.to_string(),
                    survivor.profile.provider.as_str(),
                    SESSION,
                    actual,
                    false
                ],
            )?;
        }
        _ => {}
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn unsupported_artifact(home: &Path, location: &str) -> PathBuf {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};
    let parent = if location == "todo" {
        directory(home, Path::new("todos"))
    } else {
        directory(
            home,
            &Path::new("projects/project")
                .join(SESSION)
                .join("tool-results"),
        )
    };
    let name = if location == "todo" {
        [format!("{SESSION}-agent-").as_bytes(), &[0xff], b".json"].concat()
    } else {
        vec![0xff]
    };
    let unsupported = parent.join(OsString::from_vec(name));
    if location == "directory" {
        state::directory(&unsupported).unwrap();
    } else {
        atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
            .write(&unsupported, b"unsupported name\n")
            .unwrap();
    }
    unsupported
}

#[cfg(target_os = "linux")]
#[test]
fn non_utf8_artifacts_are_retained_by_preview_purge_and_gc() {
    for location in ["file", "directory"] {
        for operation in ["purge", "gc"] {
            let temp = crate::runtime::test_directory();
            let generation = uuid::Uuid::new_v4();
            let home = home(temp.path(), generation);
            let transcript = write(&home, &format!("projects/project/{SESSION}.jsonl"));
            let unsupported = unsupported_artifact(&home, location);
            let store = open(temp.path());
            let a = owner(&store, "worker", Provider::Claude, generation, 1);
            store
                .atomic(|s| s.record_native_path(&a, SESSION, &transcript))
                .unwrap();
            let preview = purge(&store, "preview", "worker", false, true);
            assert_eq!(
                preview.native_cleanup.native_warnings[0].reason,
                Warning::UnsafePath
            );
            assert!(preview.native_cleanup.native_preview[0].files.is_empty());
            serde_json::to_value(&preview).unwrap();
            if operation == "gc" {
                let args: GcArgs =
                    serde_json::from_value(serde_json::json!({"before":10})).unwrap();
                let page = store
                    .atomic(|s| s.gc("remove", &args, &HashSet::new()))
                    .unwrap();
                assert_eq!(page.stats.deleted_agents, 1);
                serde_json::to_value(&page).unwrap();
            } else {
                serde_json::to_value(purge(&store, "remove", "worker", false, false)).unwrap();
            }
            store.finish_native_deletions().unwrap();
            let receipt = store.native_cleanup_receipt("remove").unwrap().unwrap();
            assert_eq!(receipt.native_history, NativeHistory::Partial);
            assert_eq!(receipt.native_sessions_retained, 1);
            assert_eq!(receipt.native_warnings[0].reason, Warning::UnsafePath);
            assert!(!store.native_cleanup_pending("remove").unwrap());
            assert!(transcript.exists());
            assert!(unsupported.exists());
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
fn unproved_non_utf8_shared_todo_is_not_enumerated_or_deleted() {
    let temp = crate::runtime::test_directory();
    let generation = uuid::Uuid::new_v4();
    let home = home(temp.path(), generation);
    let transcript = write(&home, &format!("projects/project/{SESSION}.jsonl"));
    let unknown = unsupported_artifact(&home, "todo");
    let store = open(temp.path());
    let a = owner(&store, "worker", Provider::Claude, generation, 1);
    store
        .atomic(|s| s.record_native_path(&a, SESSION, &transcript))
        .unwrap();
    let preview = purge(&store, "preview", "worker", false, true);
    assert_eq!(preview.native_cleanup.native_warning_count, 0);
    assert_eq!(preview.native_cleanup.native_preview[0].files.len(), 1);
    purge(&store, "remove", "worker", false, false);
    store.finish_native_deletions().unwrap();
    assert!(!transcript.exists());
    assert!(unknown.exists());
}
