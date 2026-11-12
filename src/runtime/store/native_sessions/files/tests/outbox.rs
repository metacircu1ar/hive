mod gc;
mod prune;
mod renumbering;
mod retention;
use super::*;
use crate::runtime::{
    api::{GcArgs, NativeHistory, PurgeOptions},
    domain::{Agent, AgentState, TaskOutcome},
    store::Store,
    test_support::AgentBuilder,
};
use std::collections::HashSet;

fn open(root: &Path) -> Store {
    state::directory(&root.join("runtime")).unwrap();
    Store::open(&root.join("runtime/runtime.sqlite")).unwrap()
}
fn agent(store: &Store, id: &str, generation: uuid::Uuid, thread: &str) -> Agent {
    let a = AgentBuilder::new(id)
        .configure(|a| {
            a.state = AgentState::Stopped;
            a.task_outcome = TaskOutcome::Completed;
            a.conversation_clean = true;
            a.ended_at = Some(1);
            a.auth_generation = generation;
            a.thread_id = Some(thread.into());
        })
        .build();
    store
        .atomic(|s| {
            s.save_at(&a, 1)?;
            bind_fixture(s, &a, thread)
        })
        .unwrap();
    a
}
fn bind_fixture(store: &Store, agent: &Agent, thread: &str) -> Result<()> {
    store.record_native_path(
        agent,
        thread,
        &store
            .native_home(agent.auth_generation)?
            .join(rollout(thread)),
    )
}
fn purge(
    store: &Store,
    request: &str,
    id: &str,
    keep: bool,
    dry_run: bool,
) -> crate::runtime::api::PurgeResponse {
    store
        .atomic(|s| {
            s.purge(
                request,
                id,
                PurgeOptions {
                    recursive: false,
                    dry_run,
                    keep_native_history: keep,
                },
                &HashSet::new(),
            )
        })
        .unwrap()
}

#[test]
fn commit_precedes_unlink_and_reopened_recovery_finishes_without_an_agent() {
    let temp = crate::runtime::test_directory();
    let generation = uuid::Uuid::new_v4();
    let home = home(temp.path(), generation);
    let owned = write(&home, &rollout(SESSION));
    let peer = write(&home, &rollout(PEER));
    let store = open(temp.path());
    agent(&store, "worker", generation, SESSION);
    agent(&store, "branch", generation, PEER);
    purge(&store, "purge", "worker", false, false);
    assert_eq!(store.agent("worker").unwrap_err().code(), "agent_purged");
    assert!(owned.exists());
    assert!(store.native_cleanup_pending("purge").unwrap());
    drop(store);
    let recovered = open(temp.path());
    recovered.finish_native_deletions().unwrap();
    recovered.finish_native_deletions().unwrap();
    let receipt = recovered.native_cleanup_receipt("purge").unwrap().unwrap();
    assert_eq!(receipt.native_history, NativeHistory::Deleted);
    assert_eq!(receipt.native_sessions_deleted, 1);
    assert_eq!(receipt.native_bytes_deleted, 15);
    assert!(!recovered.native_cleanup_pending("purge").unwrap());
    assert!(!owned.exists());
    assert!(peer.exists());
    assert!(recovered.agent("branch").is_ok());
}

#[test]
fn each_unlink_or_directory_barrier_failure_keeps_the_outbox_retryable() {
    for step in [
        Step::BeforeRemove,
        Step::AfterRemove,
        Step::BeforeDirectorySync,
        Step::AfterDirectorySync,
    ] {
        let temp = crate::runtime::test_directory();
        let generation = uuid::Uuid::new_v4();
        let home = home(temp.path(), generation);
        let owned = write(&home, &rollout(SESSION));
        let store = open(temp.path());
        agent(&store, "worker", generation, SESSION);
        purge(&store, "purge", "worker", false, false);
        let error = with_hook(
            move |at| {
                if at == step {
                    Err(io::Error::from_raw_os_error(libc::EIO))
                } else {
                    Ok(())
                }
            },
            || store.finish_native_deletions(),
        )
        .unwrap_err();
        assert_eq!(error.io_raw_os_error(), Some(libc::EIO));
        assert!(store.native_cleanup_pending("purge").unwrap());
        assert_eq!(
            store
                .native_cleanup_receipt("purge")
                .unwrap()
                .unwrap()
                .native_sessions_deleted,
            0
        );
        store.finish_native_deletions().unwrap();
        let receipt = store.native_cleanup_receipt("purge").unwrap().unwrap();
        assert_eq!(
            receipt.native_sessions_deleted + receipt.native_sessions_absent,
            1
        );
        assert_eq!(
            receipt.native_sessions_deleted,
            u64::from(step == Step::BeforeRemove)
        );
        assert!(!store.native_cleanup_pending("purge").unwrap());
        assert!(!owned.exists());
    }
}

#[test]
fn a_later_unlink_error_acknowledges_completed_sessions_and_bytes_before_retry() {
    let temp = crate::runtime::test_directory();
    let generation = uuid::Uuid::new_v4();
    let home = home(temp.path(), generation);
    let first = write(&home, &rollout(SESSION));
    let second = write(&home, &rollout(PEER));
    let store = open(temp.path());
    let worker = agent(&store, "worker", generation, SESSION);
    store.atomic(|s| bind_fixture(s, &worker, PEER)).unwrap();
    purge(&store, "purge", "worker", false, false);
    let calls = Cell::new(0);
    let error = with_hook(
        move |step| {
            if step == Step::BeforeRemove {
                calls.set(calls.get() + 1);
                if calls.get() == 2 {
                    return Err(io::Error::from_raw_os_error(libc::EIO));
                }
            }
            Ok(())
        },
        || store.finish_native_deletions(),
    )
    .unwrap_err();
    assert_eq!(error.io_raw_os_error(), Some(libc::EIO));
    assert!(!first.exists());
    assert!(second.exists());
    let receipt = store.native_cleanup_receipt("purge").unwrap().unwrap();
    assert_eq!(receipt.native_sessions_deleted, 1);
    assert_eq!(receipt.native_bytes_deleted, 15);
    assert_eq!(receipt.native_sessions_absent, 0);
    assert_eq!(
        store
            .query_row_cached("SELECT count(*) FROM native_cleanup_files", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    store.finish_native_deletions().unwrap();
    let receipt = store.native_cleanup_receipt("purge").unwrap().unwrap();
    assert_eq!(receipt.native_sessions_deleted, 2);
    assert_eq!(receipt.native_bytes_deleted, 30);
    assert_eq!(receipt.native_sessions_absent, 0);
    assert!(!second.exists());
}

#[test]
fn every_login_binding_is_cleaned_but_a_survivors_earlier_shared_binding_is_kept() {
    let temp = crate::runtime::test_directory();
    let first = uuid::Uuid::new_v4();
    let second = uuid::Uuid::new_v4();
    let old_home = home(temp.path(), first);
    let new_home = home(temp.path(), second);
    let shared = write(&old_home, &rollout(SESSION));
    let owned = write(&new_home, &rollout(SESSION));
    let peer = write(&new_home, &rollout(PEER));
    let store = open(temp.path());
    let mut worker = agent(&store, "worker", first, SESSION);
    let mut survivor = agent(&store, "survivor", first, SESSION);
    worker.auth_generation = second;
    survivor.auth_generation = second;
    survivor.thread_id = Some(PEER.into());
    store
        .atomic(|s| {
            s.save(&worker)?;
            bind_fixture(s, &worker, SESSION)?;
            s.save(&survivor)?;
            bind_fixture(s, &survivor, PEER)
        })
        .unwrap();
    purge(&store, "purge", "worker", false, false);
    store.finish_native_deletions().unwrap();
    let receipt = store.native_cleanup_receipt("purge").unwrap().unwrap();
    assert_eq!(receipt.native_history, NativeHistory::Partial);
    assert_eq!(receipt.native_sessions_deleted, 1);
    assert_eq!(receipt.native_sessions_retained, 1);
    assert_eq!(receipt.native_warnings[0].reason, Warning::SharedSession);
    assert!(shared.exists());
    assert!(!owned.exists());
    assert!(peer.exists());
}

#[test]
fn dry_run_previews_and_keep_flag_never_delete_native_files() {
    let temp = crate::runtime::test_directory();
    let generation = uuid::Uuid::new_v4();
    let home = home(temp.path(), generation);
    let owned = write(&home, &rollout(SESSION));
    let store = open(temp.path());
    agent(&store, "worker", generation, SESSION);
    let preview = purge(&store, "preview", "worker", false, true).native_cleanup;
    assert_eq!(preview.native_preview.len(), 1);
    assert_eq!(preview.native_preview[0].bytes, 15);
    assert_eq!(
        preview.native_preview[0].files[0].path,
        PathBuf::from(rollout(SESSION))
    );
    assert!(store.native_cleanup_receipt("preview").unwrap().is_none());
    assert!(store.agent("worker").is_ok());
    purge(&store, "keep", "worker", true, false);
    store.finish_native_deletions().unwrap();
    let receipt = store.native_cleanup_receipt("keep").unwrap().unwrap();
    assert_eq!(receipt.native_history, NativeHistory::Retained);
    assert_eq!(receipt.native_sessions_retained, 1);
    assert!(owned.exists());
}

#[test]
fn both_generations_are_deleted_without_touching_unrelated_sessions() {
    let temp = crate::runtime::test_directory();
    let store = open(temp.path());
    let first = uuid::Uuid::new_v4();
    home(temp.path(), first);
    let mut worker = agent(&store, "worker", first, SESSION);
    let mut owned = Vec::new();
    let mut peers = Vec::new();
    for generation in [first, uuid::Uuid::new_v4()] {
        let home = home(temp.path(), generation);
        owned.push(write(&home, &rollout(SESSION)));
        peers.push(write(&home, &rollout(PEER)));
        worker.auth_generation = generation;
        store
            .atomic(|s| {
                s.save(&worker)?;
                bind_fixture(s, &worker, SESSION)
            })
            .unwrap();
    }
    purge(&store, "both", "worker", false, false);
    store.finish_native_deletions().unwrap();
    let receipt = store.native_cleanup_receipt("both").unwrap().unwrap();
    assert_eq!(receipt.native_sessions_deleted, 2);
    assert_eq!(receipt.native_bytes_deleted, 30);
    assert!(owned.iter().all(|path| !path.exists()));
    assert!(peers.iter().all(|path| path.exists()));
}

#[test]
fn replaced_and_missing_files_and_unresolved_creations_have_honest_receipts() {
    let temp = crate::runtime::test_directory();
    let generation = uuid::Uuid::new_v4();
    let home = home(temp.path(), generation);
    let owned = write(&home, &rollout(SESSION));
    let store = open(temp.path());
    let mut a = agent(&store, "worker", generation, SESSION);
    a.execution_id = Some("unresolved".into());
    store.atomic(|s| s.begin_native_creation(&a)).unwrap();
    purge(&store, "replace", "worker", false, false);
    write(&home, &rollout(SESSION));
    store.finish_native_deletions().unwrap();
    let receipt = store.native_cleanup_receipt("replace").unwrap().unwrap();
    assert_eq!(receipt.native_history, NativeHistory::Partial);
    assert_eq!(receipt.native_sessions_retained, 1);
    assert!(
        receipt
            .native_warnings
            .iter()
            .any(|w| w.reason == Warning::IncompleteHistory)
    );
    assert!(
        receipt
            .native_warnings
            .iter()
            .any(|w| w.reason == Warning::IdentityChanged)
    );
    assert!(owned.exists());
    agent(&store, "missing", generation, PEER);
    purge(&store, "missing", "missing", false, false);
    assert_eq!(
        store
            .native_cleanup_receipt("missing")
            .unwrap()
            .unwrap()
            .native_sessions_absent,
        1
    );
}

#[test]
fn codex_reports_canonical_home_before_its_rollout_exists() {
    let temp = crate::runtime::test_directory();
    let real = directory(temp.path(), Path::new("real/state"));
    let alias = temp.path().join("alias");
    symlink(temp.path().join("real"), &alias).unwrap();
    let root = alias.join("state");
    let generation = uuid::Uuid::new_v4();
    let native_home = home(&root, generation);
    let store = open(&root);
    let a = AgentBuilder::new("worker")
        .configure(|a| {
            a.auth_generation = generation;
            a.state = AgentState::Stopped;
            a.task_outcome = TaskOutcome::Completed;
            a.ended_at = Some(1);
            a.conversation_clean = true;
        })
        .build();
    let native = native_home.canonicalize().unwrap().join(rollout(SESSION));
    assert!(!native.exists());
    store
        .atomic(|s| {
            s.save_at(&a, 1)?;
            s.record_native_path(&a, SESSION, &native)
        })
        .unwrap();
    let path: String = store
        .query_row_cached("SELECT path FROM native_session_paths", [], |r| r.get(0))
        .unwrap();
    assert_eq!(path, rollout(SESSION));
    write(&native_home, &rollout(SESSION));
    purge(&store, "canonical", "worker", false, false);
    store.finish_native_deletions().unwrap();
    assert!(!native.exists());
    assert!(real.exists());
    assert_eq!(
        store
            .native_cleanup_receipt("canonical")
            .unwrap()
            .unwrap()
            .native_sessions_deleted,
        1
    );
}
