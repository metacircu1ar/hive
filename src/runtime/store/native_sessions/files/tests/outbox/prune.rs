//! Credential pruning distinguishes resumable owners from discarded history.
use super::*;
use crate::config::{Method, Profile};

fn retired(root: &Path) -> (state::State, state::Binding, uuid::Uuid) {
    let state = state::State::new(root.to_path_buf()).unwrap();
    let old = state
        .register(
            "main",
            &Profile {
                provider: Provider::Codex,
                method: Method::Subscription,
                api_key_env: None,
                api_key_file: None,
            },
        )
        .unwrap();
    state.recovery_report(&old).unwrap();
    let repair = state.begin_repair("main", &old.profile).unwrap();
    state.finish_repair(&repair, true, None).unwrap();
    (state, old, repair.binding.id)
}

#[test]
fn saved_agent_and_pending_cleanup_independently_pin_retired_credentials() {
    let _fork = crate::os::fork_guard().unwrap();
    let temp = crate::runtime::test_directory();
    let root = temp.path().join("state");
    let (state, old, _) = retired(&root);
    let owned = write(&old.directory.join("provider"), &rollout(SESSION));
    let store = open(&root);
    agent(&store, "branch-still-using-old-home", old.id, SESSION);
    drop(store);
    let error = state.prune("main", old.id).unwrap_err();
    assert!(error.to_string().contains("pinned by saved agents"));
    assert!(owned.exists());
    let store = open(&root);
    purge(
        &store,
        "delete",
        "branch-still-using-old-home",
        false,
        false,
    );
    drop(store);
    let error = state.prune("main", old.id).unwrap_err();
    assert!(error.to_string().contains("pending native cleanup"));
    assert!(owned.exists());
    let store = open(&root);
    store.finish_native_deletions().unwrap();
    drop(store);
    assert!(!owned.exists());
    state.prune("main", old.id).unwrap();
    // A completed cleanup receipt is retained without pinning the generation.
    state.prune("main", old.id).unwrap();
}

#[test]
fn switched_agent_historical_bindings_and_attempts_do_not_pin_the_old_generation() {
    let _fork = crate::os::fork_guard().unwrap();
    let temp = crate::runtime::test_directory();
    let root = temp.path().join("state");
    let (state, old, current) = retired(&root);
    let owned = write(&old.directory.join("provider"), &rollout(SESSION));
    let store = open(&root);
    let mut a = agent(&store, "coordinator", old.id, SESSION);
    a.execution_id = Some("discarded-attempt".into());
    store.atomic(|s| s.begin_native_creation(&a)).unwrap();
    a.auth_generation = current;
    a.thread_id = None;
    a.execution_id = None;
    store.atomic(|s| s.save_at(&a, 1)).unwrap();
    drop(store);
    state.prune("main", old.id).unwrap();
    assert!(!owned.exists());
    let store = open(&root);
    assert!(store.agent("coordinator").is_ok());
    purge(&store, "later", "coordinator", false, false);
    store.finish_native_deletions().unwrap();
    let receipt = store.native_cleanup_receipt("later").unwrap().unwrap();
    assert_eq!(receipt.native_sessions_absent, 1);
    assert!(!store.native_cleanup_pending("later").unwrap());
}
