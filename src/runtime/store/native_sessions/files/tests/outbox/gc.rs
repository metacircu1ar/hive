//! Page budgets for actual owned files, without scanning other sessions.
use super::*;

fn shared_claude(store: &Store, home: &Path, generation: uuid::Uuid, id: &str) {
    let agent = AgentBuilder::new(id)
        .configure(|a| {
            a.profile.provider = Provider::Claude;
            a.auth_generation = generation;
            a.thread_id = Some(SESSION.into());
            a.state = AgentState::Stopped;
            a.task_outcome = TaskOutcome::Completed;
            a.conversation_clean = true;
            a.ended_at = Some(1);
        })
        .build();
    store
        .atomic(|s| {
            s.save_at(&agent, 1)?;
            s.record_native_path(
                &agent,
                SESSION,
                &home.join(format!("projects/project/{SESSION}.jsonl")),
            )
        })
        .unwrap();
}

#[test]
fn shared_ancestors_cross_pages_without_scanning_an_oversized_final_owner() {
    use crate::runtime::{domain::PurgeBlockerReason, store::native_sessions::preflight::Planner};
    let temp = crate::runtime::test_directory();
    let generation = uuid::Uuid::new_v4();
    let home = home(temp.path(), generation);
    let transcript = write(&home, &format!("projects/project/{SESSION}.jsonl"));
    let backup = write(&home, &format!("file-history/{SESSION}/backup"));
    let store = open(temp.path());
    let count = crate::limits::GC_CANDIDATES + 2;
    for i in 0..count {
        shared_claude(&store, &home, generation, &format!("owner-{i:04}"));
    }
    let mut args: GcArgs = serde_json::from_value(serde_json::json!({"before":10})).unwrap();
    for page in 0..2 {
        DIRECTORY_VISITS.set(0);
        let result = store
            .atomic(|s| {
                s.gc_with_planner(
                    &format!("page-{page}"),
                    &args,
                    &HashSet::new(),
                    &mut Planner::new(Some(100), Some(1)),
                )
            })
            .unwrap();
        // First page reaches the candidate limit; second reclaims the penultimate
        // owner, but cannot admit the two-file last owner.
        assert_eq!(
            result.stats.deleted_agents,
            if page == 0 { (count - 2) as u64 } else { 1 }
        );
        assert!(result.blockers.is_empty());
        assert_eq!(DIRECTORY_VISITS.get(), usize::from(page != 0));
        assert!(transcript.exists() && backup.exists());
        args.after = result.next_after.unwrap().into();
    }
    let last = store
        .atomic(|s| {
            s.gc_with_planner(
                "last",
                &args,
                &HashSet::new(),
                &mut Planner::new(Some(100), Some(1)),
            )
        })
        .unwrap();
    assert_eq!(last.stats.deleted_agents, 0);
    assert_eq!(
        last.blockers[0].reason,
        PurgeBlockerReason::NativeCleanupLimit
    );
    assert!(last.next_after.is_none());
    purge(
        &store,
        "explicit",
        &format!("owner-{:04}", count - 1),
        false,
        false,
    );
    drop(store);
    let store = open(temp.path());
    store.finish_native_deletions().unwrap();
    assert!(!transcript.exists() && !backup.exists());
}

#[test]
fn last_owner_is_charged_in_the_same_page_and_preview_matches_selection() {
    use crate::runtime::store::native_sessions::preflight::Planner;
    for cap in [1, 2] {
        let temp = crate::runtime::test_directory();
        let generation = uuid::Uuid::new_v4();
        let home = home(temp.path(), generation);
        let transcript = write(&home, &format!("projects/project/{SESSION}.jsonl"));
        let backup = write(&home, &format!("file-history/{SESSION}/backup"));
        let store = open(temp.path());
        for id in ["a", "b"] {
            shared_claude(&store, &home, generation, id);
        }
        let run = |request, dry_run| {
            let args: GcArgs =
                serde_json::from_value(serde_json::json!({"before":10,"dry_run":dry_run})).unwrap();
            store
                .atomic(|s| {
                    s.gc_with_planner(
                        request,
                        &args,
                        &HashSet::new(),
                        &mut Planner::new(Some(100), Some(cap)),
                    )
                })
                .unwrap()
        };
        let preview = run("preview", true);
        assert!(transcript.exists() && backup.exists());
        let real = run("real", false);
        assert_eq!(preview.selected_preview, real.selected_preview);
        assert_eq!(preview.stats.eligible_agents, real.stats.deleted_agents);
        assert_eq!(real.stats.deleted_agents, cap as u64);
        assert_eq!(preview.native_cleanup.native_preview.is_empty(), cap == 1);
        store.finish_native_deletions().unwrap();
        assert_eq!(transcript.exists(), cap == 1);
        assert_eq!(backup.exists(), cap == 1);
        if cap == 1 {
            assert!(store.agent("b").is_ok());
        }
    }
}

#[test]
fn work_budget_preserves_all_bindings_of_a_multi_generation_candidate() {
    use crate::runtime::store::native_sessions::preflight::Planner;
    let temp = crate::runtime::test_directory();
    let store = open(temp.path());
    let first = uuid::Uuid::new_v4();
    let second = uuid::Uuid::new_v4();
    let paths =
        [first, second].map(|generation| write(&home(temp.path(), generation), &rollout(SESSION)));
    let mut worker = agent(&store, "worker", first, SESSION);
    worker.auth_generation = second;
    store.atomic(|s| bind_fixture(s, &worker, SESSION)).unwrap();
    let args: GcArgs = serde_json::from_value(serde_json::json!({"before":10})).unwrap();
    let page = store
        .atomic(|s| {
            s.gc_with_planner(
                "small",
                &args,
                &HashSet::new(),
                &mut Planner::new(Some(4), Some(100)),
            )
        })
        .unwrap();
    assert_eq!(page.stats.deleted_agents, 0);
    assert_eq!(page.stats.blocked_agents, 1);
    assert!(store.agent("worker").is_ok());
    assert_eq!(
        store
            .query_row_cached("SELECT count(*) FROM native_session_paths", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert!(paths.iter().all(|path| path.exists()));
    purge(&store, "explicit", "worker", false, false);
    store.finish_native_deletions().unwrap();
    assert_eq!(
        store
            .native_cleanup_receipt("explicit")
            .unwrap()
            .unwrap()
            .native_sessions_deleted,
        2
    );
    assert!(paths.iter().all(|path| !path.exists()));
}

#[test]
fn gc_pages_bound_candidates_and_native_cleanup_batches() {
    let temp = crate::runtime::test_directory();
    let generation = uuid::Uuid::new_v4();
    let home = home(temp.path(), generation);
    let store = open(temp.path());
    let count = crate::limits::GC_CANDIDATES + 1;
    for i in 0..count {
        let id = uuid::Uuid::new_v4().to_string();
        write(&home, &rollout(&id));
        agent(&store, &format!("worker-{i:04}"), generation, &id);
    }
    let args: GcArgs = serde_json::from_value(serde_json::json!({"before": 10})).unwrap();
    DIRECTORY_VISITS.set(0);
    let page = store
        .atomic(|s| s.gc("page1", &args, &HashSet::new()))
        .unwrap();
    assert_eq!(page.stats.deleted_agents, (count - 1) as u64);
    assert_eq!(
        DIRECTORY_VISITS.get(),
        0,
        "direct Codex paths require no directory enumeration"
    );
    assert!(page.next_after.is_some());
    store.finish_native_deletions().unwrap();
    let receipt = store.native_cleanup_receipt("page1").unwrap().unwrap();
    assert_eq!(
        receipt.native_sessions_deleted,
        (count - 1) as u64,
        "{receipt:?}"
    );
    let mut args = args;
    args.after = page.next_after.unwrap().into();
    let page = store
        .atomic(|s| s.gc("page2", &args, &HashSet::new()))
        .unwrap();
    assert_eq!(page.stats.deleted_agents, 1);
    assert!(page.next_after.is_none());
    store.finish_native_deletions().unwrap();
    assert_eq!(
        store
            .native_cleanup_receipt("page2")
            .unwrap()
            .unwrap()
            .native_sessions_deleted,
        1
    );
}

fn claude_artifacts(
    store: &Store,
    home: &Path,
    generation: uuid::Uuid,
    name: &str,
    files: usize,
) -> Vec<PathBuf> {
    let session = uuid::Uuid::new_v4().to_string();
    let mut a = agent(store, name, generation, &session);
    a.profile.provider = Provider::Claude;
    store
        .atomic(|s| {
            s.save_at(&a, 1)?;
            s.record_native_path(
                &a,
                &session,
                &home.join(format!("projects/project/{session}.jsonl")),
            )
        })
        .unwrap();
    // The helper initially recorded a missing Codex session too. It is intentionally
    // retained in the binding history, exercising multiple bindings per agent.
    (0..files)
        .map(|i| write(home, &format!("file-history/{session}/backup-{i}")))
        .collect()
}

#[test]
fn gc_file_budget_defers_whole_agents_and_resumes_without_losing_ownership() {
    use crate::runtime::store::native_sessions::preflight::Planner;
    let temp = crate::runtime::test_directory();
    let generation = uuid::Uuid::new_v4();
    let home = home(temp.path(), generation);
    let store = open(temp.path());
    let paths: Vec<_> = (0..3)
        .map(|i| claude_artifacts(&store, &home, generation, &format!("worker-{i}"), 3))
        .collect();
    let mut args: GcArgs = serde_json::from_value(serde_json::json!({"before": 10})).unwrap();
    for i in 0..3 {
        let key = format!("page-{i}");
        let page = store
            .atomic(|s| {
                s.gc_with_planner(
                    &key,
                    &args,
                    &HashSet::new(),
                    &mut Planner::new(Some(100), Some(4)),
                )
            })
            .unwrap();
        assert_eq!(page.stats.deleted_agents, 1);
        assert_eq!(
            store
                .query_row_cached("SELECT count(*) FROM native_cleanup_files", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            3
        );
        for (later, files) in paths.iter().enumerate().skip(i + 1) {
            assert!(store.agent(&format!("worker-{later}")).is_ok());
            assert_eq!(
                store
                    .query_row_cached(
                        "SELECT count(*) FROM native_sessions WHERE agent=?1",
                        [format!("worker-{later}")],
                        |r| r.get::<_, i64>(0)
                    )
                    .unwrap(),
                2
            );
            assert!(files.iter().all(|path| path.exists()));
        }
        store.finish_native_deletions().unwrap();
        assert!(paths[i].iter().all(|path| !path.exists()));
        if i < 2 {
            args.after = page.next_after.unwrap().into();
        } else {
            assert!(page.next_after.is_none());
        }
    }
}

#[test]
fn gc_oversized_outbox_keeps_agent_and_bindings_for_explicit_purge() {
    use crate::runtime::{domain::PurgeBlockerReason, store::native_sessions::preflight::Planner};
    let (work, files) = (100, 2);
    let temp = crate::runtime::test_directory();
    let generation = uuid::Uuid::new_v4();
    let home = home(temp.path(), generation);
    let store = open(temp.path());
    let paths = claude_artifacts(&store, &home, generation, "worker", 3);
    let args: GcArgs = serde_json::from_value(serde_json::json!({"before": 10})).unwrap();
    let page = store
        .atomic(|s| {
            s.gc_with_planner(
                "page",
                &args,
                &HashSet::new(),
                &mut Planner::new(Some(work), Some(files)),
            )
        })
        .unwrap();
    assert_eq!(page.stats.deleted_agents, 0);
    assert_eq!(
        page.blockers[0].reason,
        PurgeBlockerReason::NativeCleanupLimit
    );
    assert!(page.next_after.is_none());
    assert!(store.agent("worker").is_ok());
    assert_eq!(
        store
            .query_row_cached("SELECT count(*) FROM native_sessions", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert!(!store.native_cleanup_pending("page").unwrap());
    assert!(paths.iter().all(|path| path.exists()));
    purge(&store, "explicit", "worker", false, false);
    store.finish_native_deletions().unwrap();
    assert!(paths.iter().all(|path| !path.exists()));
}

#[test]
fn keep_history_preview_and_real_gc_bypass_the_same_native_limits() {
    use crate::runtime::store::native_sessions::preflight::Planner;
    let temp = crate::runtime::test_directory();
    let generation = uuid::Uuid::new_v4();
    let home = home(temp.path(), generation);
    let store = open(temp.path());
    let paths: Vec<_> = ["first", "second"]
        .into_iter()
        .flat_map(|id| claude_artifacts(&store, &home, generation, id, 3))
        .collect();
    let args = |dry_run| {
        serde_json::from_value::<GcArgs>(serde_json::json!({
            "before":10,"dry_run":dry_run,"keep_native_history":true
        }))
        .unwrap()
    };
    let run = |request, dry_run| {
        store
            .atomic(|s| {
                s.gc_with_planner(
                    request,
                    &args(dry_run),
                    &HashSet::new(),
                    // Each candidate is individually over this cap. Keep must bypass it
                    // even in a preview, and cannot call a budgeted discovery on cache miss.
                    &mut Planner::new(Some(100), Some(2)),
                )
            })
            .unwrap()
    };
    let preview = run("preview", true);
    assert_eq!(preview.stats.eligible_agents, 2);
    assert_eq!(preview.stats.deleted_agents, 0);
    assert_eq!(preview.stats.blocked_agents, 0);
    assert!(preview.blockers.is_empty());
    assert!(preview.native_cleanup.native_preview.is_empty());
    assert!(store.agent("first").is_ok());
    assert!(store.agent("second").is_ok());
    let purge_preview = purge(&store, "purge-preview", "first", true, true);
    assert!(purge_preview.native_cleanup.native_preview.is_empty());
    let removed = run("remove", false);
    assert_eq!(removed.stats.deleted_agents, 2);
    assert_eq!(preview.selected_preview, removed.selected_preview);
    assert_eq!(preview.stats.eligible_agents, removed.stats.eligible_agents);
    assert_eq!(preview.stats.blocked_agents, removed.stats.blocked_agents);
    assert_eq!(preview.stats.work_rows, removed.stats.work_rows);
    assert_eq!(
        crate::api::value(preview.next_after),
        crate::api::value(removed.next_after)
    );
    assert_eq!(
        preview.native_cleanup.native_sessions_retained,
        removed.native_cleanup.native_sessions_retained
    );
    store.finish_native_deletions().unwrap();
    assert!(paths.iter().all(|path| path.exists()));
}

#[test]
fn oversized_first_candidate_does_not_starve_a_later_small_generation() {
    use crate::runtime::{domain::PurgeBlockerReason, store::native_sessions::preflight::Planner};
    let (work, files) = (100, 2);
    let temp = crate::runtime::test_directory();
    let store = open(temp.path());
    let large = uuid::Uuid::new_v4();
    let small = uuid::Uuid::new_v4();
    let kept = claude_artifacts(&store, &home(temp.path(), large), large, "a-large", 3);
    let owned = write(&home(temp.path(), small), &rollout(SESSION));
    agent(&store, "b-small", small, SESSION);
    let mut args: GcArgs = serde_json::from_value(serde_json::json!({"before":10})).unwrap();
    let page = store
        .atomic(|s| {
            s.gc_with_planner(
                "page1",
                &args,
                &HashSet::new(),
                &mut Planner::new(Some(work), Some(files)),
            )
        })
        .unwrap();
    assert_eq!(page.stats.examined_agents, 1);
    assert_eq!(page.stats.deleted_agents, 0);
    assert_eq!(page.blockers.len(), 1);
    assert_eq!(page.blockers[0].agent_id, "a-large");
    assert_eq!(
        page.blockers[0].reason,
        PurgeBlockerReason::NativeCleanupLimit
    );
    assert!(store.agent("b-small").is_ok());
    assert!(owned.exists());
    args.after = page.next_after.unwrap().into();
    let page = store
        .atomic(|s| {
            s.gc_with_planner(
                "page2",
                &args,
                &HashSet::new(),
                &mut Planner::new(Some(work), Some(files)),
            )
        })
        .unwrap();
    assert_eq!(page.stats.deleted_agents, 1);
    assert!(page.blockers.is_empty());
    assert!(page.next_after.is_none());
    store.finish_native_deletions().unwrap();
    assert!(!owned.exists());
    assert!(store.agent("a-large").is_ok());
    assert!(kept.iter().all(|path| path.exists()));
}
